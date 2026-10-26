use super::*;
use serde::{Deserialize, Serialize};

const SAFE_ID: u64 = 9_007_199_254_740_991;
const DETOUR_MS: u64 = 90_000;
const MAIN_MS: u64 = 60_000;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Objective {
    Reach20,
    Speed,
    Fame,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Main {
    id: u64,
    character: i32,
    objective: Objective,
    target: i32,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Opportunity {
    Loot {
        object: i32,
        item: i32,
        count: usize,
        revision: i32,
    },
    Company {
        object: i32,
    },
    Dungeon {
        portal: i32,
        destination: String,
        entered: bool,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, Eq)]
struct Detour {
    runtime: u64,
    world: i32,
    remaining_ms: u64,
    opportunity: Opportunity,
}
impl PartialEq for Detour {
    fn eq(&self, other: &Self) -> bool {
        self.runtime == other.runtime
            && self.world == other.world
            && self.opportunity == other.opportunity
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Intentions {
    next_id: u64,
    main: Option<Main>,
    detour: Option<Detour>,
    main_remaining_ms: u64,
    needs_progress: bool,
    return_to_main: bool,
    checkpoint: [i32; 3],
    transition: String,
    #[serde(skip)]
    runtime: u64,
    #[serde(skip)]
    observed: Option<u64>,
    #[serde(skip)]
    progressing: bool,
}
// Clock bookkeeping must not trigger a durable Store write on every decision.
impl PartialEq for Intentions {
    fn eq(&self, other: &Self) -> bool {
        self.next_id == other.next_id
            && self.main == other.main
            && self.detour == other.detour
            && (self.main_remaining_ms == 0) == (other.main_remaining_ms == 0)
            && self.needs_progress == other.needs_progress
            && self.return_to_main == other.return_to_main
            && self.checkpoint == other.checkpoint
            && self.transition == other.transition
    }
}
impl Default for Intentions {
    fn default() -> Self {
        Self {
            next_id: 1,
            main: None,
            detour: None,
            main_remaining_ms: 0,
            needs_progress: false,
            return_to_main: false,
            checkpoint: [0; 3],
            transition: "not yet selected".into(),
            runtime: 0,
            observed: None,
            progressing: false,
        }
    }
}
fn progress(v: &PlayerView) -> [i32; 3] {
    [
        v.character.experience,
        v.character.base_stats[4],
        v.character.fame,
    ]
}
fn count(v: &PlayerView, item: i32) -> usize {
    v.character
        .items
        .iter()
        .skip(4)
        .filter(|k| **k == item)
        .count()
}
impl Intentions {
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            self.next_id > 0
                && self.next_id <= SAFE_ID
                && self.main_remaining_ms <= MAIN_MS
                && self.checkpoint.iter().all(|n| *n >= 0)
                && self.transition.len() <= 128
                && (self.main.is_some()
                    || (self.detour.is_none()
                        && self.main_remaining_ms == 0
                        && !self.needs_progress
                        && !self.return_to_main)),
            "invalid main intention bounds"
        );
        if let Some(m) = &self.main {
            ensure!(
                m.id > 0
                    && m.id < self.next_id
                    && m.character > 0
                    && match m.objective {
                        Objective::Reach20 => m.target == 20,
                        Objective::Speed => (1..=255).contains(&m.target),
                        Objective::Fame => m.target > 0 && m.target % 200 == 0,
                    },
                "invalid character main goal"
            );
        }
        if let Some(d) = &self.detour {
            ensure!(
                self.main.is_some()
                    && (1..=DETOUR_MS).contains(&d.remaining_ms)
                    && match &d.opportunity {
                        Opportunity::Loot {
                            object,
                            item,
                            count,
                            revision,
                        } =>
                            *object > 0
                                && (0..=65535).contains(item)
                                && *count <= 8
                                && *revision >= 0,
                        Opportunity::Company { object } => *object > 0,
                        Opportunity::Dungeon {
                            portal,
                            destination,
                            ..
                        } =>
                            *portal > 0
                                && crate::dungeon::DUNGEONS
                                    .iter()
                                    .any(|(_, _, name, _)| *name == destination.as_str()),
                    },
                "invalid optional detour evidence"
            );
        }
        Ok(())
    }
    pub(super) fn bind(&mut self, v: &PlayerView, runtime: u64) {
        self.runtime = runtime;
        self.observed = None;
        self.progressing = false;
        if self
            .main
            .as_ref()
            .is_some_and(|m| m.character != v.session.character)
        {
            self.end("character changed; old goal and steps invalidated");
        }
        if let Some(d) = &mut self.detour {
            if d.runtime == runtime
                && self
                    .main
                    .as_ref()
                    .is_some_and(|m| m.character == v.session.character)
                && let Opportunity::Dungeon {
                    destination,
                    entered,
                    ..
                } = &mut d.opportunity
                && *destination == v.map.name
            {
                *entered = true;
            } else {
                self.finish_detour(v, "detour aborted: session changed; resume main goal");
            }
        }
    }
    pub(super) fn end(&mut self, reason: &str) {
        self.main = None;
        self.detour = None;
        self.main_remaining_ms = 0;
        self.needs_progress = false;
        self.return_to_main = false;
        self.transition = reason.into();
        self.progressing = false;
    }
    fn finish_detour(&mut self, v: &PlayerView, reason: &str) {
        self.detour = None;
        self.main_remaining_ms = MAIN_MS;
        self.needs_progress = true;
        self.checkpoint = progress(v);
        self.transition = reason.into();
        tracing::info!(
            character = v.session.character,
            goal = self.main.as_ref().map(|m| m.id),
            reason,
            "autonomous optional detour ended; resuming main goal"
        );
    }
    pub(super) fn observe(&mut self, v: &PlayerView, c: &Content) -> Result<()> {
        let elapsed = match self.observed.replace(v.elapsed_ms) {
            Some(last) => v
                .elapsed_ms
                .checked_sub(last)
                .context("main intention observation clock regressed")?,
            None => 0,
        };
        if self.progressing {
            self.main_remaining_ms = self.main_remaining_ms.saturating_sub(elapsed);
        }
        self.progressing = false;
        if self
            .main
            .as_ref()
            .is_some_and(|m| m.character != v.session.character)
        {
            self.end("character changed; old goal and steps invalidated");
        }
        if !v.lifecycle.alive || v.lifecycle.death_pending {
            self.end("character died; old goal and steps invalidated");
            return Ok(());
        }
        if let Some(d) = self.detour.as_mut() {
            d.remaining_ms = d.remaining_ms.saturating_sub(elapsed);
            let ended = match &mut d.opportunity {
                Opportunity::Loot {
                    object,
                    item,
                    count: before,
                    revision,
                } => {
                    if !v.lifecycle.busy
                        && v.character.inventory_revision > *revision
                        && count(v, *item) > *before
                    {
                        Some("detour completed: collected item committed; resume main goal")
                    } else if d.world != v.session.world
                        || !v
                            .objects
                            .iter()
                            .any(|o| o.id == *object && o.contents.contains(item))
                    {
                        Some("detour aborted: observed loot unavailable; resume main goal")
                    } else {
                        None
                    }
                }
                Opportunity::Company { object } => {
                    match v.objects.iter().find(|o| o.id == *object && o.hp > 0) {
                        Some(o)
                            if d.world == v.session.world
                                && o.position.distance(v.position) <= 4.5 =>
                        {
                            Some("detour completed: reached company; resume main goal")
                        }
                        Some(_) if d.world == v.session.world => None,
                        _ => Some("detour aborted: company no longer visible; resume main goal"),
                    }
                }
                Opportunity::Dungeon {
                    destination,
                    entered,
                    ..
                } => {
                    if v.map.name == *destination {
                        *entered = true;
                        None
                    } else if *entered {
                        Some("detour completed: left dungeon; resume main goal")
                    } else if d.world != v.session.world {
                        Some("detour aborted: entrance changed; resume main goal")
                    } else {
                        None
                    }
                }
            };
            if let Some(reason) = ended {
                self.finish_detour(v, reason);
            } else if d.remaining_ms == 0 {
                let inside = matches!(&d.opportunity, Opportunity::Dungeon { destination, .. } if *destination == v.map.name);
                self.finish_detour(v, "detour aborted: time budget exhausted; resume main goal");
                self.return_to_main = inside;
            }
        }
        if matches!(v.map.name.as_str(), "Nexus" | "Realm") {
            self.return_to_main = false;
        }
        // Busy snapshots can precede the committed character write.
        if v.lifecycle.busy || v.lifecycle.departing || v.lifecycle.quarantined {
            return Ok(());
        }
        if self.main.as_ref().is_some_and(|m| match m.objective {
            Objective::Reach20 => v.character.level >= m.target,
            Objective::Speed => v.character.base_stats[4] >= m.target,
            Objective::Fame => v.character.fame >= m.target,
        }) {
            tracing::info!(
                character = v.session.character,
                goal = self.main.as_ref().map(|m| m.id),
                "autonomous main goal completed by observed character progress"
            );
            // Progress can finish a milestone mid-dungeon; it cannot erase that excursion's exit budget.
            let excursion = self.detour.take().filter(|detour| {
                matches!(
                    detour.opportunity,
                    Opportunity::Dungeon { entered: true, .. }
                )
            });
            let returning = self.return_to_main;
            self.end("main goal completed by observed character progress");
            self.detour = excursion;
            self.return_to_main = returning;
        }
        if self.main.is_none() {
            let cap = c.classes[&v.character.class].caps[4];
            let (objective, target) = if v.character.level < 20 {
                (Objective::Reach20, 20)
            } else if v.character.base_stats[4] < cap {
                (Objective::Speed, cap)
            } else {
                (
                    Objective::Fame,
                    v.character
                        .fame
                        .checked_div(200)
                        .and_then(|n| n.checked_add(1))
                        .and_then(|n| n.checked_mul(200))
                        .context("personal base fame milestone exhausted")?,
                )
            };
            let id = self.next_id;
            ensure!(id < SAFE_ID, "main goal identity exhausted");
            self.next_id += 1;
            self.main = Some(Main {
                id,
                character: v.session.character,
                objective,
                target,
            });
            self.transition = "main goal selected from own character progression".into();
            self.checkpoint = progress(v);
            tracing::info!(
                character = v.session.character,
                goal = id,
                ?objective,
                target,
                "autonomous main goal selected"
            );
        }
        if let Some(m) = &self.main {
            ensure!(
                m.objective != Objective::Speed
                    || m.target == c.classes[&v.character.class].caps[4],
                "saved Speed goal disagrees with class cap"
            );
        }
        Ok(())
    }
    fn available(&self, v: &PlayerView) -> bool {
        self.main_remaining_ms == 0
            && (!self.needs_progress
                || self.main.as_ref().is_some_and(|main| match main.objective {
                    Objective::Reach20 => v.character.experience > self.checkpoint[0],
                    Objective::Speed => v.character.base_stats[4] > self.checkpoint[1],
                    Objective::Fame => v.character.fame > self.checkpoint[2],
                }))
    }
    fn optional(&self, v: &PlayerView, goal: &Goal, committed: bool) -> bool {
        if committed {
            return false;
        }
        match goal {
            Goal::Loot(id) => v.objects.iter().find(|o| o.id == *id).is_some_and(|o| {
                !o.contents
                    .iter()
                    .filter(|k| **k >= 0)
                    .all(|k| matches!(*k, 0xa22..=0xa23) || self.speed() && *k == 0xa21)
            }),
            // Company is a detour from leveling and fame too, not a substitute for progress.
            Goal::Follow(_) => true,
            Goal::Portal(id) if v.map.name == "Realm" => {
                v.objects.iter().find(|p| p.id == *id).is_some_and(|p| {
                    crate::dungeon::DUNGEONS
                        .iter()
                        .any(|(kind, _, _, _)| *kind == p.kind)
                        && !(self.speed() && p.kind == 0x718)
                        && !(self
                            .main
                            .as_ref()
                            .is_some_and(|m| m.objective == Objective::Reach20)
                            && p.kind == 0x717)
                })
            }
            _ => false,
        }
    }
    pub(super) fn speed(&self) -> bool {
        self.main
            .as_ref()
            .is_some_and(|m| m.objective == Objective::Speed)
    }
    fn matches(&self, goal: &Goal) -> bool {
        self.detour
            .as_ref()
            .is_some_and(|d| match (&d.opportunity, goal) {
                (Opportunity::Loot { object, .. }, Goal::Loot(id))
                | (Opportunity::Company { object }, Goal::Follow(id)) => object == id,
                (
                    Opportunity::Dungeon {
                        portal, entered, ..
                    },
                    Goal::Portal(id),
                ) => *entered || portal == id,
                (Opportunity::Dungeon { entered: true, .. }, _) => true,
                _ => false,
            })
    }
    pub(super) fn eligible(&self, v: &PlayerView, goal: &Goal, committed: bool) -> bool {
        !self.optional(v, goal, committed)
            || self.matches(goal)
            || self.detour.is_none() && self.available(v)
    }
    pub(super) fn bonus(&self, v: &PlayerView, goal: &Goal) -> f32 {
        let retained = if self.matches(goal) { 8. } else { 0. };
        retained
            + if self.speed() {
                match goal {
                    Goal::Portal(id)
                        if v.objects.iter().any(|p| p.id == *id && p.kind == 0x718) =>
                    {
                        10.
                    }
                    Goal::Loot(id)
                        if v.objects.iter().any(|b| {
                            b.id == *id
                                && b.contents.contains(&0xa21)
                                && b.contents
                                    .iter()
                                    .filter(|k| **k >= 0)
                                    .all(|k| matches!(*k, 0xa21..=0xa23))
                        }) =>
                    {
                        30.
                    }
                    _ => 0.,
                }
            } else {
                0.
            }
    }
    pub(super) fn selected(
        &mut self,
        v: &PlayerView,
        c: &Content,
        goal: &Goal,
        committed: bool,
        future: Option<u16>,
    ) -> Result<()> {
        if self.detour.is_none() && self.optional(v, goal, committed) {
            ensure!(
                self.available(v),
                "optional detour selected before main progress"
            );
            let opportunity = match goal {
                Goal::Loot(id) => {
                    let bag = v
                        .objects
                        .iter()
                        .find(|o| o.id == *id)
                        .context("optional bag missing")?;
                    let Some(Action::Swap { from, .. }) = progression::pickup(v, c, bag, future)?
                    else {
                        anyhow::bail!("optional bag has no collectible item");
                    };
                    let item = from.kind;
                    Opportunity::Loot {
                        object: *id,
                        item,
                        count: count(v, item),
                        revision: v.character.inventory_revision,
                    }
                }
                Goal::Follow(id) => Opportunity::Company { object: *id },
                Goal::Portal(id) => {
                    let p = v
                        .objects
                        .iter()
                        .find(|p| p.id == *id)
                        .context("optional entrance missing")?;
                    let (_, _, name, _) = crate::dungeon::DUNGEONS
                        .iter()
                        .find(|(kind, _, _, _)| *kind == p.kind)
                        .context("optional dungeon missing")?;
                    Opportunity::Dungeon {
                        portal: *id,
                        destination: (*name).into(),
                        entered: false,
                    }
                }
                _ => anyhow::bail!("unsupported optional choice"),
            };
            self.detour = Some(Detour {
                runtime: self.runtime,
                world: v.session.world,
                remaining_ms: DETOUR_MS,
                opportunity,
            });
            self.transition = "one bounded opportunity before returning to main progression".into();
            tracing::info!(character=v.session.character,goal=self.main.as_ref().map(|m|m.id),detour=?self.detour,"autonomous optional detour started");
        }
        Ok(())
    }
    pub(super) fn expired(&self) -> bool {
        self.return_to_main
    }
    pub(super) fn payload(
        &mut self,
        v: &PlayerView,
        activity: Activity,
        d: &Decision,
        step: String,
        committed: bool,
    ) -> serde_json::Value {
        let (goal, plan) = self.main.as_ref().map_or((serde_json::Value::Null, "No character goal selected"), |m| {
            let (label, current, unit, plan) = match m.objective {
                Objective::Reach20 => ("Reach level 20".into(), v.character.level, "level", "Gain shared XP in suitable fights and Pirate Caves"),
                Objective::Speed => ("Max permanent Speed".into(), v.character.base_stats[4], "Speed", "Farm Snake Pit and consume Speed potions toward the class cap"),
                Objective::Fame => (format!("Earn {} base fame", m.target), v.character.fame, "base fame", "Gain ordinary XP from suitable fights"),
            };
            (serde_json::json!({"id":m.id,"label":label,"current":current,"target":m.target,"unit":unit}), plan)
        });
        let optional = self.matches(&d.goal);
        let necessary = committed
            || !(matches!(
                activity,
                Activity::Adventure | Activity::Event | Activity::Linger
            ) || activity == Activity::Social && optional)
            || matches!(d.goal, Goal::Recover | Goal::Dodge | Goal::Remedy(_));
        let optional = !necessary && optional;
        let reason = if necessary {
            match activity {
                Activity::Survive => {
                    "Observed danger or health requires a safety reflex before continuing"
                }
                Activity::Busy => "Wait for pending ordinary work before another action",
                Activity::Trade => {
                    "Maintain the existing market exchange using real stock and receipts"
                }
                Activity::Equip => "Assess or equip observed own gear before continuing",
                Activity::Supply => "Restore the carried health reserve from observed supplies",
                Activity::Consume => "Use a carried stat potion before continuing",
                Activity::Bank => "Organize own retained holdings through normal Vault transfers",
                Activity::Call => "Respond to the existing observed call",
                _ => "Honor existing company or social coordination before resuming",
            }
        } else if optional {
            match &self.detour.as_ref().expect("matching detour").opportunity {
                Opportunity::Loot { .. } => {
                    "Collect useful observed loot within one bounded detour"
                }
                Opportunity::Company { .. } => {
                    "Briefly meet visible company before resuming main progression"
                }
                Opportunity::Dungeon { .. } => "Try the observed dungeon within one bounded detour",
            }
        } else if self.detour.is_some() {
            "Main progression currently outranks the pending opportunity"
        } else {
            &self.transition
        };
        let mode = if necessary {
            "necessary"
        } else if optional {
            "optional"
        } else {
            "progress"
        };
        self.progressing = mode == "progress";
        serde_json::json!({"goal":goal,"plan":plan,"step":step,"mode":mode,
            "reason":reason,
            "resume":if mode == "progress" { None } else { Some(plan) }})
    }
}

#[cfg(test)]
mod tests;
