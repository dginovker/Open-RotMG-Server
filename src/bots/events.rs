use super::*;
use serde::{Deserialize, Serialize};

pub(super) fn shared(map: &str) -> bool {
    matches!(
        map,
        "Oryx's Castle" | "Oryx's Chamber" | "Wine Cellar" | "Mad Lab"
    )
}
pub(super) fn objective(kind: u16) -> bool {
    crate::endgame::GUARDIANS.contains(&kind)
        || matches!(
            kind,
            0xd7c | crate::endgame::ORYX | crate::endgame::ORYX2 | crate::dungeon::lab::BOSS
        )
}
pub(super) fn equipped(v: &PlayerView, c: &Content) -> bool {
    v.character.level >= 20
        && v.character.stats[0] >= 500
        && v.character.stats[3] >= 10
        && v.character.items[0] >= 0
        && c.objects[&(v.character.items[0] as u16)]
            .tier
            .is_some_and(|tier| tier >= 7)
}
pub(super) fn ready(v: &PlayerView, c: &Content) -> bool {
    equipped(v, c)
        && v.character.hp * 4 >= v.character.stats[0] * 3
        && (v.character.mp * 3 >= v.character.stats[1]
            || v.character
                .items
                .iter()
                .skip(4)
                .filter(|k| **k == 0xa22)
                .count()
                >= 2)
}
fn readiness_ability(v: &PlayerView, c: &Content) -> Result<bool> {
    if v.character.class != crate::content::PRIEST
        || v.character.items[1] < 0
        || v.character.hp * 4 >= v.character.stats[0] * 3
        || v.character.conditions & crate::condition::bit(crate::condition::QUIET) != 0
    {
        return Ok(false);
    }
    let tome = c.get(v.character.items[1] as u16)?;
    Ok(tome.activate == "HealNova"
        && v.character.mp >= tome.mana_cost
        && (v.character.conditions & crate::condition::bit(crate::condition::SICK) == 0
            || tome
                .abilities
                .iter()
                .any(|a| matches!(a, crate::content::Ability::Cleanse { .. }))))
}
fn readiness_item(v: &PlayerView, c: &Content) -> Result<Option<Action>> {
    if readiness_ability(v, c)? && v.time_ms >= v.character.ability_ready_ms {
        return Ok(Some(Action::Use {
            slot: 1,
            x: v.position.x,
            y: v.position.y,
        }));
    }
    for (slot, kind) in v
        .character
        .items
        .iter()
        .enumerate()
        .skip(4)
        .filter(|(_, k)| **k >= 0)
    {
        let item = c.get(*kind as u16)?;
        let hp = item.activate == "Heal"
            && v.character.hp * 4 < v.character.stats[0] * 3
            && v.character.conditions & crate::condition::bit(crate::condition::SICK) == 0;
        let mp = item.activate == "Magic"
            && v.character.mp * 3 < v.character.stats[1]
            && v.character.conditions & crate::condition::bit(crate::condition::QUIET) == 0;
        let cleanse = item
            .consumable_effects
            .iter()
            .any(|e| matches!(e, crate::content::ConsumableEffect::Cleanse))
            && (v.character.hp * 4 < v.character.stats[0] * 3
                && v.character.conditions & crate::condition::bit(crate::condition::SICK) != 0
                || v.character.mp * 3 < v.character.stats[1]
                    && v.character.conditions & crate::condition::bit(crate::condition::QUIET)
                        != 0);
        if item.consumable && (item.heal > 0 && (hp || mp) || cleanse) {
            return Ok(Some(Action::Use {
                slot,
                x: v.position.x,
                y: v.position.y,
            }));
        }
    }
    Ok(None)
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Pickup {
    character: i32,
    world: String,
    kind: i32,
    revision: i32,
}
#[derive(Clone, Debug, PartialEq)]
struct Outing {
    world: i32,
    character: i32,
    boss: Option<(i32, f32, f32)>,
    objective: Option<(i32, f32, f32, u64)>,
    opening: Option<(i32, u64)>,
    clear: Option<u64>,
    pending: Option<(usize, i32, i32)>,
    loot: Vec<i32>,
    reported: bool,
    status_reported: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct Events {
    pub(crate) knows_blue: bool,
    pickups: VecDeque<Pickup>,
    #[serde(skip)]
    outing: Option<Outing>,
    #[serde(skip)]
    advice: bool,
}
impl Events {
    #[cfg(test)]
    pub(super) fn postclear_observed(&self, v: &PlayerView) -> bool {
        self.outing.as_ref().is_some_and(|o| {
            o.world == v.session.world && o.character == v.session.character && o.clear.is_some()
        })
    }

    pub(super) fn bind(&mut self) {
        self.outing = None;
        self.advice = false;
    }
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            self.pickups.len() <= 16
                && self.pickups.iter().all(|p| p.character > 0
                    && shared(&p.world)
                    && (0..=65535).contains(&p.kind)
                    && p.revision > 0),
            "invalid own event pickup evidence"
        );
        Ok(())
    }
    pub(super) fn receive(&mut self, packet: &Packet, v: &PlayerView, name: &str) -> Result<()> {
        if packet.id != wire::TEXT || v.map.name != "Mad Lab" {
            return Ok(());
        }
        let mut r = Reader::new(&packet.body);
        let speaker = r.utf()?;
        let actor = r.i32()?;
        r.i32()?;
        r.u8()?;
        let recipient = r.utf()?;
        let text = r.utf()?;
        r.utf()?;
        r.done()?;
        if actor != v.session.player
            && (recipient.is_empty() || recipient == name)
            && v.objects
                .iter()
                .any(|p| p.id == actor && p.name == speaker && p.account.is_some() && p.hp > 0)
        {
            if text == "blue water clears the green pool" {
                self.knows_blue = true;
                tracing::info!(
                    account = v.session.account,
                    actor,
                    "autonomous received Lab remedy advice"
                );
            } else if self.knows_blue && text == "hexed; need help" {
                self.advice = true;
            }
        }
        Ok(())
    }
    pub(super) fn observe(&mut self, v: &PlayerView) {
        if !shared(&v.map.name) || !v.lifecycle.alive {
            self.outing = None;
            return;
        }
        if self
            .outing
            .as_ref()
            .is_none_or(|o| o.world != v.session.world || o.character != v.session.character)
        {
            self.outing = Some(Outing {
                world: v.session.world,
                character: v.session.character,
                boss: None,
                objective: None,
                opening: None,
                clear: None,
                pending: None,
                loot: vec![],
                reported: false,
                status_reported: false,
            });
            tracing::info!(account=v.session.account,character=v.session.character,world=%v.map.name,"autonomous shared event observed");
        }
        let o = self.outing.as_mut().unwrap();
        if let Some(target) = v
            .objects
            .iter()
            .filter(|p| objective(p.kind) && p.hp > 0)
            .max_by_key(|p| {
                (
                    p.kind == 0xd7c,
                    std::cmp::Reverse((v.position.distance(p.position) * 10.) as i32),
                )
            })
        {
            if o.objective.is_none_or(|(id, _, _, _)| id != target.id) {
                tracing::info!(
                    account = v.session.account,
                    object = target.id,
                    x = target.position.x,
                    y = target.position.y,
                    "autonomous shared objective area observed"
                );
            }
            o.objective = Some((target.id, target.position.x, target.position.y, v.time_ms));
        } else if o.objective.is_some_and(|(_, x, y, seen)| {
            v.time_ms.saturating_sub(seen) > 15000 || v.position.distance(Pos { x, y }) < 10.
        }) {
            tracing::info!(
                account = v.session.account,
                "autonomous shared objective missing or expired; abandoning remembered area"
            );
            o.objective = None;
        }
        let boss = match v.map.name.as_str() {
            "Oryx's Chamber" => Some(crate::endgame::ORYX),
            "Wine Cellar" => Some(crate::endgame::ORYX2),
            "Mad Lab" => Some(crate::dungeon::lab::BOSS),
            _ => None,
        };
        if let Some(boss) = v.objects.iter().find(|p| Some(p.kind) == boss && p.hp > 0) {
            o.boss = Some((boss.id, boss.position.x, boss.position.y));
        }
        // Disappearance alone is not a clear: only the earned, published exit is a completion cue.
        if o.boss.is_some()
            && o.clear.is_none()
            && v.objects
                .iter()
                .any(|p| p.usable && matches!(p.kind, 0x71d | 0x721 | 0x700 | 0x712))
        {
            o.clear = Some(v.time_ms);
            tracing::info!(account=v.session.account,world=%v.map.name,"autonomous shared clear cue observed");
        }
    }
    pub(super) fn remedy<'a>(
        &self,
        v: &'a PlayerView,
    ) -> Option<&'a crate::world::view::VisibleObject> {
        (self.knows_blue
            && v.map.name == "Mad Lab"
            && v.character.conditions & crate::condition::bit(crate::condition::HEXED) != 0)
            .then(|| {
                v.objects
                    .iter()
                    .find(|p| p.kind == crate::dungeon::lab::DEHEXXER)
            })
            .flatten()
    }
    pub(super) fn area(&self, v: &PlayerView) -> Option<Pos> {
        self.outing
            .as_ref()
            .filter(|o| {
                o.world == v.session.world
                    && o.character == v.session.character
                    && o.clear.is_none()
            })
            .and_then(|o| o.objective)
            .map(|(_, x, y, _)| Pos { x, y })
    }
    pub(super) fn committed(&mut self, v: &PlayerView) -> Result<()> {
        if let Some(o) = &mut self.outing
            && let Some((slot, kind, revision)) = o.pending.take()
        {
            ensure!(
                o.world == v.session.world
                    && o.character == v.session.character
                    && v.character.inventory_revision > revision
                    && v.character.items[slot] == kind,
                "committed event pickup does not match own guarded action"
            );
            let pickup = Pickup {
                character: v.session.character,
                world: v.map.name.clone(),
                kind,
                revision: v.character.inventory_revision,
            };
            tracing::info!(
                account = v.session.account,
                ?pickup,
                "autonomous own event loot committed"
            );
            self.pickups.push_back(pickup);
            if self.pickups.len() > 16 {
                self.pickups.pop_front();
            }
            o.loot.push(kind);
            if o.loot.len() > 8 {
                o.loot.remove(0);
            }
        }
        Ok(())
    }
    pub(super) fn rejected(&mut self) {
        if let Some(o) = &mut self.outing {
            o.pending = None;
        }
    }
    pub(super) fn collecting(&mut self, v: &PlayerView, action: &Action) {
        if let Some(o) = &mut self.outing
            && let Action::Swap { from, to } = action
            && from.object != v.session.player
            && to.object == v.session.player
        {
            o.pending = Some((to.slot, from.kind, v.character.inventory_revision));
        }
    }
    pub(super) fn decide(
        &mut self,
        v: &PlayerView,
        c: &Content,
        grid: &Grid,
        attention: usize,
    ) -> Result<Option<Decision>> {
        if !shared(&v.map.name) {
            return Ok(None);
        }
        let mut d = Decision {
            goal: Goal::Wait,
            movement: None,
            aim: None,
            item: None,
            journey: None,
            chat: None,
        };
        if !equipped(v, c) {
            d.goal = Goal::Recover;
            d.journey = Some(Journey::Nexus);
            tracing::info!(account=v.session.account,world=%v.map.name,"autonomous shared event declined; own equipment not ready");
            return Ok(Some(d));
        }
        if v.objects
            .iter()
            .any(|p| p.usable && matches!(p.kind, 0xd7b | 0x700))
            && !ready(v, c)
        {
            d.goal = Goal::Recover;
            if let Some(item) = readiness_item(v, c)? {
                d.item = Some(item);
                return Ok(Some(d));
            }
            let ability_pending = readiness_ability(v, c)?;
            let opening_active = self
                .outing
                .as_ref()
                .and_then(|o| o.opening)
                .is_some_and(|(_, began)| v.time_ms.saturating_sub(began) < 30000);
            if ability_pending || opening_active {
                return Ok(Some(d));
            }
            d.journey = Some(Journey::Nexus);
            let entrance = v
                .objects
                .iter()
                .find(|p| p.usable && matches!(p.kind, 0xd7b | 0x700))
                .unwrap();
            tracing::info!(account=v.session.account,character=v.session.character,world=%v.map.name,tick=v.tick,time_ms=v.time_ms,
                hp=v.character.hp,max_hp=v.character.stats[0],mp=v.character.mp,max_mp=v.character.stats[1],
                conditions=v.character.conditions,ability_ready_ms=v.character.ability_ready_ms,
                held_health=v.character.items.iter().skip(4).filter(|k|**k==0xa22).count(),
                held_magic=v.character.items.iter().skip(4).filter(|k|**k==0xa23).count(),
                entrance=entrance.id,kind=entrance.kind,usable=entrance.usable,ready=false,pending=v.lifecycle.busy,
                items=%serde_json::to_string(&v.character.items)?,remedy_available=false,
                ability_pending,opening_active,"autonomous next event declined; own readiness spent");
            return Ok(Some(d));
        }
        let o = self
            .outing
            .as_mut()
            .context("shared event observation missing")?;
        let hexed = v.character.conditions & crate::condition::bit(crate::condition::HEXED) != 0;
        if v.map.name == "Mad Lab"
            && hexed
            && self.knows_blue
            && let Some(blue) = v
                .objects
                .iter()
                .find(|p| p.kind == crate::dungeon::lab::DEHEXXER)
            && let Some(next) = grid.towards(v.position, blue.position)
        {
            d.goal = Goal::Remedy(blue.id);
            d.movement = Some(next);
            if !o.status_reported {
                d.chat = Some("hexed; trying blue water");
                o.status_reported = true;
            }
            return Ok(Some(d));
        }
        if hexed && !o.status_reported {
            d.chat = Some("hexed; need help");
            o.status_reported = true;
            return Ok(Some(d));
        }
        if self.advice {
            self.advice = false;
            d.chat = Some("blue water clears the green pool");
            return Ok(Some(d));
        }
        if v.map.name == "Oryx's Chamber"
            && !v.objects.iter().any(|p| p.kind == 0x700 && p.usable)
            && let Some(locked) = v
                .objects
                .iter()
                .find(|p| p.kind == crate::endgame::LOCKED_CELLAR)
            && let Some(slot) = v
                .character
                .items
                .iter()
                .position(|k| *k == i32::from(crate::endgame::INCANTATION))
        {
            let (portal, began) = *o.opening.get_or_insert_with(|| {
                tracing::info!(
                    account = v.session.account,
                    portal = locked.id,
                    slot,
                    "autonomous held Cellar opening considered"
                );
                (locked.id, v.time_ms)
            });
            if portal != locked.id
                || v.time_ms.saturating_sub(began) >= 30000
                || v.character.hp * 4 < v.character.stats[0] * 3
            {
                d.journey = Some(Journey::Nexus);
                tracing::info!(
                    account = v.session.account,
                    portal,
                    elapsed = v.time_ms.saturating_sub(began),
                    "autonomous Cellar opening abandoned; opportunity or readiness expired"
                );
            } else if v.position.distance(locked.position) > 3. {
                d.goal = Goal::Portal(locked.id);
                d.movement = grid.next(v.position, locked.position);
                if d.movement.is_none() {
                    d.journey = Some(Journey::Nexus);
                    tracing::info!(
                        account = v.session.account,
                        portal,
                        "autonomous observed Cellar opening has no known route; leaving"
                    );
                }
            } else if ready(v, c) {
                d.item = Some(Action::Use {
                    slot,
                    x: v.position.x,
                    y: v.position.y,
                });
            } else {
                d.goal = Goal::Recover;
            }
            return Ok(Some(d));
        }
        let Some(clear) = o.clear else {
            return Ok(None);
        };
        let mut loot_waiting = false;
        for bag in v
            .objects
            .iter()
            .filter(|p| c.objects[&p.kind].class == "Container")
        {
            loot_waiting |= progression::pickup(v, c, bag, None)?.is_some();
        }
        if loot_waiting {
            return Ok(None);
        }
        let elapsed = v.time_ms.saturating_sub(clear);
        let audience = v
            .objects
            .iter()
            .any(|p| p.account.is_some() && p.hp > 0 && v.position.distance(p.position) < 12.);
        let at_reward_site = o
            .boss
            .is_some_and(|(_, x, y)| v.position.distance(Pos { x, y }) < 6.);
        let has_bag = v.objects.iter().any(|p| {
            c.objects[&p.kind].class == "Container"
                && o.boss
                    .is_some_and(|(_, x, y)| p.position.distance(Pos { x, y }) < 3.)
        });
        if !o.reported
            && audience
            && elapsed >= 1000
            && (!o.loot.is_empty() || at_reward_site && !has_bag)
        {
            d.chat = Some(if o.loot.contains(&0xa35) {
                "got wis"
            } else if !o.loot.is_empty() {
                "got a drop"
            } else {
                "no drop here for me"
            });
            o.reported = true;
            return Ok(Some(d));
        }
        let inspecting = v
            .character
            .items
            .iter()
            .skip(4)
            .any(|k| *k >= 0 && c.objects[&(*k as u16)].tier.is_some());
        let linger = if !ready(v, c) {
            1000
        } else if inspecting {
            2000 + 500 * attention as u64
        } else if audience && !o.loot.is_empty() {
            3500
        } else {
            1500
        };
        if elapsed < linger {
            return Ok(Some(d));
        }
        if !v
            .objects
            .iter()
            .any(|p| p.usable && matches!(p.kind, 0x71d | 0x712 | 0x700))
        {
            d.journey = Some(Journey::Nexus);
            #[cfg(test)]
            tracing::info!(normal_view=%serde_json::to_string(v)?,clear_world=o.world,clear_character=o.character,clear_time_ms=clear,boss=?o.boss,pending=o.pending.is_some(),applicable_remedy=readiness_item(v,c)?.is_some(),loot_waiting,live_objective=v.objects.iter().any(|p|objective(p.kind)&&p.hp>0),"autonomous postclear own-view branch witness");
            return Ok(Some(d));
        }
        Ok(None)
    }
}
