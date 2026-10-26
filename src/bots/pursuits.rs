use super::*;
use serde::{Deserialize, Serialize};

pub(crate) mod market;

const HEALTH: i32 = 0xa22;
const RESERVE: usize = 2;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct Cognition {
    pub attention: usize,
    pub velocity_step: f32,
    pub directions: usize,
    pub pickup_clearance: f32,
}
impl Default for Cognition {
    fn default() -> Self {
        Self {
            attention: 8,
            velocity_step: 0.25,
            directions: 16,
            pickup_clearance: 1.05,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct ItemFact {
    pub kind: i32,
    pub assessments: u8,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
enum Step {
    Seeking,
    Collect,
    MakeSpace,
    Return,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Opportunity {
    world: i32,
    runtime: u64,
    observed_object: i32,
    area: (f32, f32),
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Activity {
    id: u64,
    character: i32,
    opportunity: Option<Opportunity>,
    step: Step,
    carried: usize,
    revision: i32,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct Experience {
    pub activity: u64,
    pub reason: String,
    pub binding: [i32; 4],
    pub session: u64,
    pub revision: i32,
    pub carried: usize,
    pub deficit: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct Mind {
    pub schema: u8,
    pub account: i32,
    pub revision: i64,
    #[serde(skip)]
    binding: u64,
    #[serde(skip)]
    runtime: u64,
    pub cognition: Cognition,
    pub facts: Vec<ItemFact>,
    #[serde(default)]
    pub future_class: Option<u16>,
    #[serde(default)]
    pub market: market::Market,
    #[serde(default)]
    pub social: social::Life,
    #[serde(default)]
    pub events: events::Events,
    next_activity: u64,
    activity: Option<Activity>,
    pub history: VecDeque<Experience>,
}
impl Mind {
    pub(crate) fn new(account: i32) -> Self {
        Self {
            schema: 1,
            account,
            revision: 0,
            binding: 0,
            runtime: 0,
            cognition: Cognition::default(),
            facts: vec![],
            future_class: None,
            market: market::Market::default(),
            social: social::Life::default(),
            events: events::Events::default(),
            next_activity: 1,
            activity: None,
            history: VecDeque::new(),
        }
    }
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == 1 && self.account > 0 && self.revision >= 0,
            "invalid account mind binding/schema"
        );
        ensure!(
            self.history.len() <= 32 && self.facts.len() <= 64 && self.next_activity > 0,
            "account mind exceeds bounds"
        );
        ensure!(
            self.facts
                .iter()
                .all(|f| (0..=65535).contains(&f.kind) && f.assessments <= 2)
                && self
                    .facts
                    .iter()
                    .map(|f| f.kind)
                    .collect::<BTreeSet<_>>()
                    .len()
                    == self.facts.len(),
            "invalid or duplicate learned item facts"
        );
        ensure!(
            self.history.iter().all(|h| h.binding[0] == self.account
                && h.carried <= 8
                && h.deficit == RESERVE.saturating_sub(h.carried)
                && h.reason.len() <= 128),
            "invalid bounded account experience"
        );
        if let Some(a) = &self.activity {
            ensure!(
                a.id > 0
                    && a.id < self.next_activity
                    && a.character > 0
                    && (a.step == Step::Seeking) == a.opportunity.is_none()
                    && a.opportunity.as_ref().is_none_or(|o| o.observed_object > 0
                        && o.area.0.is_finite()
                        && o.area.1.is_finite())
                    && a.carried <= 8
                    && a.revision >= 0,
                "invalid supply activity evidence"
            );
        }
        self.market.validate(self.account)?;
        self.social.validate(self.account)?;
        self.events.validate()?;
        let c = &self.cognition;
        ensure!(
            (1..=32).contains(&c.attention)
                && [4, 8, 16].contains(&c.directions)
                && c.velocity_step.is_finite()
                && (0.125..=4.).contains(&c.velocity_step)
                && c.pickup_clearance.is_finite()
                && (0.1..=2.).contains(&c.pickup_clearance),
            "invalid cognitive estimates"
        );
        Ok(())
    }
    fn record(&mut self, v: &PlayerView, reason: &str, activity: u64) {
        let (count, revision) = match &self.activity {
            Some(a) if a.id == activity => (a.carried, a.revision),
            _ => (carried(v), v.character.inventory_revision),
        };
        let event = Experience {
            activity,
            reason: reason.into(),
            binding: [
                v.session.account,
                v.session.character,
                v.session.world,
                v.session.player,
            ],
            session: self.binding,
            revision,
            carried: count,
            deficit: RESERVE.saturating_sub(count),
        };
        if self.history.back() == Some(&event) {
            return;
        }
        tracing::info!(event=%serde_json::to_string(&event).expect("experience serialization"), "autonomous pursuit transition");
        self.history.push_back(event);
        if self.history.len() > 32 {
            self.history.pop_front();
        }
    }
    pub(super) fn end(&mut self, v: &PlayerView, reason: &str) {
        match self.activity.take() {
            Some(a) => self.record(v, reason, a.id),
            None if reason == "character_died_steps_invalidated" => self.record(v, reason, 0),
            None => {}
        }
    }
    pub(super) fn bind(&mut self, v: &PlayerView, runtime: u64) -> Result<()> {
        self.validate()?;
        self.market.bind();
        self.social.bind();
        self.events.bind();
        self.binding = rand::random();
        self.runtime = runtime;
        ensure!(
            self.account == v.session.account,
            "account mind cannot cross accounts"
        );
        if self
            .activity
            .as_ref()
            .is_some_and(|a| a.character != v.session.character)
        {
            self.end(v, "character_changed_old_steps_invalidated");
        }
        match &self.activity {
            Some(a) => self.record(
                v,
                if a.opportunity.is_some() {
                    "rebound_revalidate_area_not_object"
                } else {
                    "rebound_seeking_supply_opportunity"
                },
                a.id,
            ),
            None => self.record(v, "account_session_bound", 0),
        }
        self.reconcile(v)?;
        if self
            .activity
            .as_ref()
            .and_then(|a| a.opportunity.as_ref())
            .is_some_and(|o| o.runtime != runtime)
        {
            self.lose_opportunity(v, "original_world_not_restored_after_restart");
        }
        Ok(())
    }
    fn lose_opportunity(&mut self, v: &PlayerView, reason: &str) {
        if let Some(a) = self.activity.as_mut() {
            let opportunity = a.opportunity.take();
            a.step = Step::Seeking;
            let id = a.id;
            tracing::info!(
                account = self.account,
                activity = id,
                ?opportunity,
                reason,
                "autonomous supply opportunity invalidated; project retained"
            );
            self.record(v, reason, id);
        }
    }
    pub(super) fn reconcile(&mut self, v: &PlayerView) -> Result<()> {
        ensure!(
            self.account == v.session.account,
            "pursuit observation account changed"
        );
        if self
            .activity
            .as_ref()
            .is_some_and(|a| a.character != v.session.character)
        {
            self.end(v, "character_changed_old_steps_invalidated");
        }
        if !settled(v) {
            return Ok(());
        }
        if let Some(a) = self.activity.as_mut() {
            ensure!(
                v.character.inventory_revision >= a.revision,
                "pursuit holdings revision regressed"
            );
            let count = carried(v);
            // A pending character-bound effect can still invalidate an apparent reserve.
            if a.carried != count || a.revision != v.character.inventory_revision {
                a.carried = count;
                a.revision = v.character.inventory_revision;
                let id = a.id;
                self.record(v, "holdings_reconciled", id);
            }
            if count >= RESERVE {
                self.end(v, "reserve_carried_satisfied");
            }
        }
        Ok(())
    }
    pub(super) fn inspecting(&self, v: &PlayerView, content: &Content) -> bool {
        v.character
            .items
            .iter()
            .skip(4)
            .filter(|k| **k >= 0)
            .any(|kind| {
                let item = &content.objects[&(*kind as u16)];
                content.objects[&v.character.class]
                    .slots
                    .iter()
                    .take(4)
                    .any(|slot| *slot == item.slot_type)
                    && !self
                        .facts
                        .iter()
                        .any(|f| f.kind == *kind && f.assessments >= 2)
            })
    }
    pub(super) fn assess(&mut self, v: &PlayerView, content: &Content) -> Result<Option<Action>> {
        let class = content.get(v.character.class)?;
        for (from, kind) in v
            .character
            .items
            .iter()
            .enumerate()
            .skip(4)
            .filter(|(_, k)| **k >= 0)
        {
            let item = content.get(*kind as u16)?;
            let Some(to) = class
                .slots
                .iter()
                .take(4)
                .position(|slot| *slot == item.slot_type)
            else {
                continue;
            };
            if item.unsupported_equipment {
                continue;
            }
            let known = self.facts.iter().position(|f| f.kind == *kind);
            let index = match known {
                Some(index) => index,
                None => {
                    if self.facts.len() == 64 {
                        self.facts.remove(0);
                    }
                    self.facts.push(ItemFact {
                        kind: *kind,
                        assessments: 0,
                    });
                    self.facts.len() - 1
                }
            };
            let evidence = self.facts[index].assessments;
            if evidence < 2 {
                self.facts[index].assessments += 1;
                self.record(
                    v,
                    &format!("inspect_item_{kind}_evidence_{}", evidence + 1),
                    0,
                );
                // Inspection competes for attention but finishes after two attended assessments.
                return Ok(None);
            }
            let old = v.character.items[to];
            if progression::quality(v, content, *kind)? > progression::quality(v, content, old)? {
                self.record(v, &format!("assessed_equip_{kind}_over_{old}"), 0);
                return Ok(Some(Action::Swap {
                    from: own(v, from),
                    to: own(v, to),
                }));
            }
        }
        Ok(None)
    }
    pub(super) fn supply(
        &mut self,
        v: &PlayerView,
        content: &Content,
        grid: &Grid,
    ) -> Result<Option<Decision>> {
        self.reconcile(v)?;
        if !settled(v) {
            return Ok(None);
        }
        if self
            .activity
            .as_ref()
            .is_none_or(|a| a.opportunity.is_none())
            && carried(v) < RESERVE
            && v.map.name == "Realm"
            && let Some(bag) = v.objects.iter().find(|o| {
                content.objects[&o.kind].class == "Container" && o.contents.contains(&HEALTH)
            })
        {
            let opportunity = Opportunity {
                world: v.session.world,
                runtime: self.runtime,
                observed_object: bag.id,
                area: (bag.position.x, bag.position.y),
            };
            let (id, reason) = if let Some(a) = self.activity.as_mut() {
                a.opportunity = Some(opportunity);
                a.step = Step::Collect;
                (a.id, "observed_supply_opportunity_replanned")
            } else {
                let id = self.next_activity;
                self.next_activity = self
                    .next_activity
                    .checked_add(1)
                    .context("activity identity exhausted")?;
                self.activity = Some(Activity {
                    id,
                    character: v.session.character,
                    opportunity: Some(opportunity),
                    step: Step::Collect,
                    carried: carried(v),
                    revision: v.character.inventory_revision,
                });
                (id, "observed_supply_opportunity_reserve_two")
            };
            self.record(v, reason, id);
        }
        let Some(a) = self.activity.clone() else {
            return Ok(None);
        };
        let Some(opportunity) = &a.opportunity else {
            return Ok(None);
        };
        let mut d = Decision {
            goal: Goal::Wait,
            movement: None,
            aim: None,
            item: None,
            journey: None,
            chat: None,
        };
        if a.step == Step::MakeSpace {
            if v.map.name == "Vault" {
                if v.character.items.iter().skip(4).any(|k| *k == -1) {
                    self.activity.as_mut().unwrap().step = Step::Return;
                    self.record(v, "space_committed_retaining_supply", a.id);
                    d.journey = Some(Journey::Nexus);
                } else if let Some(bank) = progression::bank(v, content, grid, self.future_class)? {
                    if matches!(bank.goal, Goal::Bank(_)) {
                        return Ok(Some(bank));
                    }
                    self.lose_opportunity(v, "no_bankable_spare_or_vault_capacity");
                    return Ok(None);
                }
            } else if v.map.name == "Nexus" {
                return self.portal(v, grid, 0x720, a.id);
            } else {
                d.journey = Some(Journey::Nexus);
            }
            return Ok(Some(d));
        }
        if v.map.name == "Nexus" {
            return self.portal(v, grid, 0x712, a.id);
        }
        if v.session.world != opportunity.world {
            self.lose_opportunity(v, "original_realm_unavailable");
            return Ok(None);
        }
        let area = Pos {
            x: opportunity.area.0,
            y: opportunity.area.1,
        };
        // A remembered identity is evidence to match, never a slot/action handle from another session.
        let observed = v.objects.iter().find(|o| {
            o.id == opportunity.observed_object
                && o.position.distance(area) < 1.
                && content.objects[&o.kind].class == "Container"
                && o.contents.contains(&HEALTH)
        });
        if let Some(bag) = observed {
            let Some(to) = v
                .character
                .items
                .iter()
                .enumerate()
                .skip(4)
                .find(|(_, k)| **k == -1)
                .map(|(slot, _)| slot)
            else {
                self.activity.as_mut().unwrap().step = Step::MakeSpace;
                self.record(v, "capacity_interruption", a.id);
                d.journey = Some(Journey::Nexus);
                return Ok(Some(d));
            };
            d.goal = Goal::Loot(bag.id);
            if v.position.distance(bag.position) < 1.4 {
                let slot = bag
                    .contents
                    .iter()
                    .position(|k| *k == HEALTH)
                    .context("observed supply vanished")?;
                d.item = Some(Action::Swap {
                    from: Slot {
                        object: bag.id,
                        slot,
                        kind: HEALTH,
                    },
                    to: own(v, to),
                });
            } else {
                d.movement = grid.towards(v.position, bag.position);
            }
        } else if v.position.distance(area) <= 3. {
            self.lose_opportunity(v, "observed_opportunity_missing_or_expired");
            return Ok(None);
        } else {
            d.goal = Goal::Revisit(tile(area).0, tile(area).1);
            d.movement = grid.towards(v.position, area);
            if d.movement.is_none() {
                self.lose_opportunity(v, "remembered_area_unreachable");
                return Ok(None);
            }
        }
        Ok(Some(d))
    }
    fn portal(
        &mut self,
        v: &PlayerView,
        grid: &Grid,
        kind: u16,
        activity: u64,
    ) -> Result<Option<Decision>> {
        let Some(p) = v.objects.iter().find(|o| o.kind == kind) else {
            return Ok(None);
        };
        if !p.usable {
            self.lose_opportunity(v, "observed_required_entrance_unavailable");
            return Ok(None);
        }
        let near = v.position.distance(p.position) < 1.4;
        self.record(v, "continuing_via_observed_entrance", activity);
        Ok(Some(Decision {
            goal: Goal::Portal(p.id),
            movement: if near {
                None
            } else {
                grid.towards(v.position, p.position)
            },
            aim: None,
            item: None,
            journey: near.then_some(Journey::Portal(p.id)),
            chat: None,
        }))
    }
}
fn settled(v: &PlayerView) -> bool {
    v.lifecycle.alive
        && !v.lifecycle.busy
        && !v.lifecycle.death_pending
        && !v.lifecycle.departing
        && !v.lifecycle.quarantined
        && matches!(v.lifecycle.output, OutputState::Live)
}
fn carried(v: &PlayerView) -> usize {
    v.character
        .items
        .iter()
        .skip(4)
        .filter(|k| **k == HEALTH)
        .count()
}
