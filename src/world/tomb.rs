// Behavior follows a reconstruction, not recovered official code: https://github.com/trapped/rotmg_svr/blob/593e490803fe0ad168ead408cacd5287d778acfe/wServer/logic/db/BehaviorDb.Tomb.cs; selected 123.5.1 XML takes precedence.
use super::*;
mod ai;

use crate::encounter::tomb as weapons;
use crate::{
    condition,
    dungeon::tomb as d,
    encounter::{Anchor, Motion, nr_speed},
};
use std::f32::consts::TAU;

#[derive(Clone, Debug, Default, Serialize)]
pub(super) struct State {
    pub anchor: Option<i32>,
    pub producer: Option<i32>,
    pub peers: Option<[i32; 3]>,
    pub guard: Option<(i32, u64)>,
    pub press_since: Option<u64>,
    pub next_heal: u64,
    pub next_spawn: u64,
    pub initialized: bool,
    pub artifacts_unlocked: bool,
    pub completion_pos: Option<Pos>,
    pub paralyzed: bool,
}
impl State {
    pub fn for_kind(kind: u16) -> Option<Self> {
        d::contains(kind).then(Self::default)
    }
    pub fn suspend(&mut self, delta: u64) {
        self.next_heal += delta;
        self.next_spawn += delta;
        self.press_since = None;
    }
}

impl World {
    pub(super) fn populate_tomb_entry(&mut self) -> Result<()> {
        // One selected PerRealmMax site feeds the shared director, which reserves and earns the actor.
        let pos = self.place_named_setpiece(d::SPHINX, &[1])?;
        let id = self.spawn_entity(d::SPHINX, pos, vec![])?;
        self.entities
            .get_mut(&id)
            .unwrap()
            .quest
            .set(Some(id), id, &mut self.quest_ids);
        tracing::info!(
            entity = id,
            kind = d::SPHINX,
            x = pos.x,
            y = pos.y,
            "finite Tomb entrance Sphinx site prepared"
        );
        Ok(())
    }
    pub(super) fn tomb_bindings(&mut self) -> Result<()> {
        let anchors: Vec<_> = self
            .actors()
            .filter(|e| e.kind == d::ANCHOR)
            .map(|e| e.id)
            .collect();
        if anchors.is_empty() {
            return Ok(());
        }
        ensure!(
            anchors.len() == 1,
            "Tomb must have exactly one arena anchor"
        );
        let anchor = anchors[0];
        for kind in d::STATUES {
            ensure!(
                self.actors().filter(|e| e.kind == kind).count() == 1,
                "Tomb statue {kind:#x} is missing or duplicated"
            );
        }
        ensure!(
            self.actors().any(|e| e.kind == d::SARCOPHAGUS),
            "Tomb has no objective sarcophagi"
        );
        let ids: Vec<_> = self
            .actors()
            .filter(|e| d::contains(e.kind))
            .map(|e| e.id)
            .collect();
        for id in ids {
            let e = &self.entities[&id];
            let bound = if d::STATUES.contains(&e.kind) {
                Some(anchor)
            } else if matches!(e.kind, 0xd2a | 0xd2b | d::TRAP) {
                let kind = if e.kind == d::TRAP {
                    d::SWITCH
                } else {
                    d::SARCOPHAGUS
                };
                let radius = if e.kind == d::TRAP { 6. } else { 10. };
                let owners: Vec<_> = self
                    .actors()
                    .filter(|other| other.kind == kind && other.pos.distance(e.pos) < radius)
                    .map(|e| e.id)
                    .collect();
                ensure!(
                    owners.len() == 1,
                    "Tomb actor {id} has {} candidate room owners",
                    owners.len()
                );
                Some(owners[0])
            } else {
                None
            };
            self.entities
                .get_mut(&id)
                .unwrap()
                .tomb
                .as_mut()
                .context("Tomb actor state missing")?
                .anchor = bound;
        }
        Ok(())
    }
    fn tomb_spawn(
        &mut self,
        producer: Option<i32>,
        origin: Pos,
        kind: u16,
        count: usize,
        radius: f32,
        anchor: Option<i32>,
    ) -> Result<Vec<i32>> {
        crate::encounter::validate(&self.content, kind)?;
        crate::loot::validate(&self.content, kind)?;
        ensure!(count > 0 && count <= 16, "invalid Tomb summon count");
        let owned = self
            .actors()
            .filter(|e| {
                e.parent.is_some()
                    || e.quest.is_some()
                    || e.tomb.as_ref().is_some_and(|b| b.producer.is_some())
            })
            .count();
        ensure!(
            owned + self.named_births.len() + (if producer.is_some() { count } else { 0 }) <= 256,
            "Tomb summon budget exhausted"
        );
        let positions: Vec<_> = (0..count)
            .map(|n| {
                let angle = n as f32 * TAU / count as f32;
                Pos {
                    x: origin.x + radius * angle.cos(),
                    y: origin.y + radius * angle.sin(),
                }
            })
            .collect();
        ensure!(
            positions.iter().all(|p| self.walkable(*p)),
            "Tomb summon site is blocked, producer {producer:?}, kind {kind:#x}"
        );
        let mut ids = Vec::with_capacity(count);
        for position in positions {
            let id = self.insert_entity(kind, position, vec![])?;
            let brain = self
                .entities
                .get_mut(&id)
                .unwrap()
                .tomb
                .as_mut()
                .context("Tomb summon state missing")?;
            // Room occupants and artifacts survive their summoner; generic parent expiry must not erase them.
            brain.producer = producer;
            brain.anchor = anchor;
            ids.push(id);
        }
        Ok(ids)
    }
    fn tomb_owned(&self, producer: i32, kind: u16) -> Vec<i32> {
        self.actors()
            .filter(|e| {
                e.kind == kind
                    && e.tomb
                        .as_ref()
                        .is_some_and(|b| b.producer == Some(producer))
            })
            .map(|e| e.id)
            .collect()
    }
    pub(super) fn tomb_guard(&self, id: i32) -> Option<i32> {
        let (caller, phase) = self.entities[&id].tomb.as_ref()?.guard?;
        self.entities
            .get(&caller)
            .filter(|e| d::BOSSES.contains(&e.kind) && e.stage == Some(phase))
            .map(|e| e.id)
    }
    fn tomb_say(&self, id: i32, text: &str) {
        let name = match self.entities[&id].kind {
            d::NUT => "Nut",
            d::GEB => "Geb",
            d::BES => "Bes",
            _ => "Sarcophagus",
        };
        self.broadcast_text(name, text);
    }
    fn tomb_enter(&mut self, id: i32, phase: u64, shield: bool) -> Result<()> {
        let kind = self.entities[&id].kind;
        let (rate, radius) = if matches!(phase, weapons::SPECIAL_TWO | weapons::SPECIAL_THREE) {
            (0.9, 10.)
        } else {
            let slot = usize::try_from(phase.min(6))?;
            match kind {
                d::NUT => (
                    [0.6, 0.6, 0.7, 0.6, 0.6, 0.6, 0.6][slot],
                    [5., 5., 4., 6., 7., 4., 5.][slot],
                ),
                d::GEB => (0.6, [5., 5., 5., 5., 5., 4., 4.][slot]),
                d::BES => (
                    [0.6, 0.6, 0.7, 0.6, 0.7, 0.6, 0.6][slot],
                    [5., 5., 5., 3., 5., 7., 7.][slot],
                ),
                _ => bail!("Tomb phase entered by non-boss {kind:#x}"),
            }
        };
        let orbit = realm_events::Orbit {
            speed: nr_speed(rate * self.rng.gen_range(0.9..=1.1)),
            radius: radius + self.rng.gen_range(-0.5..=0.5),
            direction: if phase == 0 {
                -1.
            } else {
                self.entities[&id]
                    .orbit
                    .context("Tomb phase lost its orbit direction")?
                    .direction
            },
        };
        let e = self.entities.get_mut(&id).unwrap();
        e.stage = Some(phase);
        e.activated = Some(self.now);
        e.next_shot = self.now;
        e.orbit = Some(orbit);
        e.condition &=
            !(condition::bit(condition::ARMORED) | condition::bit(condition::INVULNERABLE));
        if kind == d::BES || phase == 0 {
            e.condition |= condition::bit(condition::ARMORED);
        }
        if shield {
            e.condition |= condition::bit(condition::INVULNERABLE);
        }
        if phase == weapons::RAGE {
            e.tomb.as_mut().context("Tomb boss state missing")?.guard = None;
        }
        if matches!(phase, 5 | 6) {
            let brain = self
                .entities
                .get_mut(&id)
                .unwrap()
                .tomb
                .as_mut()
                .context("Tomb boss state missing")?;
            brain.artifacts_unlocked = true;
            brain.next_spawn = self.now;
        }
        if matches!(phase, weapons::SPECIAL_TWO | weapons::SPECIAL_THREE) {
            self.apply_condition(id, condition::ARMOR_BROKEN, weapons::SPECIAL_ARMOR_BREAK_MS)?;
        }
        if phase == weapons::RAGE {
            self.flash_inland(id, 0xffff0000, 1., 9000001.);
        }
        let text = match phase {
            0 => Some(if kind == d::BES {
                "THIS WILL NOW BE YOUR TOMB!"
            } else if kind == d::NUT {
                "YOU HAVE AWAKENED US!"
            } else {
                "ENOUGH OF YOUR VANDALISM!"
            }),
            1 => Some("Impudence! I am an immortal, I needn't take you seriously."),
            3 => Some(if kind == d::BES {
                "Nut, disable our foes!"
            } else {
                "Bes, protect me at once!"
            }),
            4 => Some(if kind == d::BES || kind == d::NUT {
                "Geb, eradicate these cretins from our tomb!"
            } else {
                "Nut, disable our foes!"
            }),
            5 => Some(if kind == d::BES {
                "Nut, let them wish they were dead!"
            } else if kind == d::NUT {
                "My artifacts shall make your lethargic lives end much more swiftly!"
            } else {
                "My artifacts shall destroy you from your soul to your flesh!"
            }),
            6 if kind == d::BES => {
                Some("My artifacts shall prove my wall of defense is impenetrable!")
            }
            weapons::RAGE => Some(if kind == d::BES {
                "The end of your path is here!"
            } else if kind == d::NUT {
                "This cannot be! You shall not succeed!"
            } else {
                "Argh! You shall pay for your crimes!"
            }),
            _ => None,
        };
        if let Some(text) = text {
            self.tomb_say(id, text);
        }
        if matches!(phase, 3 | 4) {
            let helper = match (kind, phase) {
                (d::BES, 3) => 0,
                (d::BES, 4) | (d::NUT, 4) => 1,
                (d::GEB, 4) => 0,
                _ => 2,
            };
            let peers = self.entities[&id]
                .tomb
                .as_ref()
                .context("Tomb boss state missing")?
                .peers
                .context("Tomb boss identities missing")?;
            let helper = peers[helper];
            if self.entities.contains_key(&helper)
                && self.entities[&helper].stage != Some(weapons::RAGE)
                && self.tomb_guard(helper).is_none()
            {
                self.entities
                    .get_mut(&helper)
                    .unwrap()
                    .tomb
                    .as_mut()
                    .context("Tomb protector state missing")?
                    .guard = Some((id, phase));
            }
        }
        tracing::info!(entity = id, kind, phase, shield, "Tomb boss phase entered");
        Ok(())
    }
    fn tomb_boss_step(&mut self, id: i32) -> Result<()> {
        let e = &self.entities[&id];
        let kind = e.kind;
        let phase = e.stage.context("Tomb boss phase missing")?;
        let age = self.now - e.activated.context("Tomb boss phase clock missing")?;
        let hp = e.hp;
        let desired = weapons::phase_for_hp(kind, hp)?;
        let shield = if phase == 1 {
            2000
        } else if (3..=weapons::RAGE).contains(&phase) {
            weapons::SHIELD_MS
        } else {
            0
        };
        if age >= shield {
            self.entities.get_mut(&id).unwrap().condition &=
                !condition::bit(condition::INVULNERABLE);
        }
        if matches!(phase, weapons::SPECIAL_TWO | weapons::SPECIAL_THREE) {
            if desired == weapons::RAGE {
                self.tomb_enter(id, weapons::RAGE, true)?;
            } else if age >= weapons::SPECIAL_MS {
                let destination = desired.max(phase - 5);
                self.tomb_enter(
                    id,
                    destination,
                    destination != phase - 5 && destination >= 3,
                )?;
            }
        } else if age >= shield && desired > phase {
            let next = phase + 1;
            self.tomb_enter(id, next, next != 2)?;
        } else if matches!(phase, 3 | 4) && age >= weapons::SPECIAL_AFTER_MS {
            self.tomb_enter(id, phase + 5, false)?;
        }
        Ok(())
    }
    fn tomb_awaken(&mut self) -> Result<()> {
        for kind in d::BOSSES {
            crate::encounter::validate(&self.content, kind)?;
            crate::loot::validate(&self.content, kind)?;
        }
        for kind in d::STATUES {
            let statues: Vec<_> = self.actors().filter(|e| e.kind == kind).collect();
            ensure!(
                statues.len() == 1 && self.walkable(statues[0].pos),
                "Tomb awakening preflight rejected statue {kind:#x}"
            );
        }
        let mut births = Vec::new();
        for (i, kind) in d::STATUES.into_iter().enumerate() {
            let statues: Vec<_> = self
                .actors()
                .filter(|e| e.kind == kind)
                .map(|e| e.id)
                .collect();
            ensure!(statues.len() == 1, "Tomb awakening lost statue {kind:#x}");
            let id = statues[0];
            let pos = self.entities[&id].pos;
            let anchor = self.entities[&id]
                .tomb
                .as_ref()
                .context("Tomb statue state missing")?
                .anchor
                .context("Tomb statue anchor missing")?;
            self.remove_entity(id);
            let new = self.tomb_spawn(None, pos, d::BOSSES[i], 1, 0., Some(anchor))?[0];
            births.push(new);
        }
        let peers: [i32; 3] = births
            .try_into()
            .map_err(|_| anyhow::anyhow!("Tomb awakening did not produce all three bosses"))?;
        for id in peers {
            self.entities
                .get_mut(&id)
                .unwrap()
                .tomb
                .as_mut()
                .context("Tomb boss state missing")?
                .peers = Some(peers);
        }
        tracing::info!(
            ?peers,
            "all objective sarcophagi defeated; Tomb bosses awakened"
        );
        Ok(())
    }
    fn tomb_heal(&mut self, id: i32, amount: i32) -> Result<()> {
        let e = self
            .entities
            .get_mut(&id)
            .context("Tomb heal target missing")?;
        if e.conditions(self.now) & condition::bit(condition::SICK) != 0 {
            return Ok(());
        }
        let delta = (self.content.objects[&e.kind].hp - e.hp).min(amount).max(0);
        if delta == 0 {
            return Ok(());
        }
        e.hp += delta;
        let mut out = Writer::new();
        out.i32(id).utf(&format!("+{delta}")).u32(0xff00ff00);
        for player in self.players.values().filter(|p| p.known.contains(&id)) {
            send(player, wire::NOTIFICATION, out.0.clone());
        }
        Ok(())
    }
    pub(super) fn tomb_death(&mut self, e: &Entity) -> Result<()> {
        if e.kind == d::SARCOPHAGUS && !self.actors().any(|e| e.kind == d::SARCOPHAGUS) {
            self.tomb_awaken()?;
        }
        if d::BOSSES.contains(&e.kind) {
            let anchor = e
                .tomb
                .as_ref()
                .context("defeated Tomb boss state missing")?
                .anchor
                .context("defeated Tomb boss anchor missing")?;
            let root = self
                .entities
                .get_mut(&anchor)
                .context("Tomb completion anchor missing")?;
            for (character, damage) in &e.damage {
                let value = root.damage.entry(*character).or_default();
                *value = value
                    .checked_add(*damage)
                    .context("Tomb participation damage overflow")?;
            }
            if !self
                .actors()
                .any(|e| d::BOSSES.contains(&e.kind) || d::STATUES.contains(&e.kind))
            {
                self.entities
                    .get_mut(&anchor)
                    .unwrap()
                    .tomb
                    .as_mut()
                    .context("Tomb completion state missing")?
                    .completion_pos = Some(e.pos);
                self.defeat_entity(anchor)?;
            }
        }
        if e.kind == 0xd1a {
            let count = self.rng.gen_range(5..=8);
            self.tomb_spawn(Some(e.id), e.pos, 0xd1b, count, 0., None)?;
        }
        if (0xd2d..=0xd30).contains(&e.kind) {
            self.tomb_spawn(Some(e.id), e.pos, 0xd1b, 1, 0., None)?;
        }
        Ok(())
    }
}
