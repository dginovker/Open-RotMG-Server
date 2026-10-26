use super::*;
use crate::encounter::{Anchor, Motion};

impl World {
    pub(super) fn transform_fauna(&mut self, id: i32, kind: u16) -> Result<i32> {
        crate::encounter::validate(&self.content, kind)?;
        let old = &self.entities[&id];
        let slot = old.population_slot;
        ensure!(
            old.camp.is_none() || slot.is_some(),
            "transforming family member lacks reserved slot"
        );
        let new = self.spawn_entity(kind, old.pos, vec![])?;
        let mut old = self.remove_entity(id);
        let e = self.entities.get_mut(&new).unwrap();
        e.home = old.home;
        e.camp = old.camp;
        e.parent.set(*old.parent, new, &mut self.timed_ids);
        e.quest = old.quest;
        e.damage = std::mem::take(&mut old.damage);
        e.expires.set(*old.expires, new, &mut self.timed_ids);
        e.activated = Some(self.now);
        if let Some(slot) = slot {
            self.population[slot].entity = Some(new);
            self.mark_population(new, slot)?;
        }
        tracing::debug!(
            entity = id,
            replacement = new,
            kind,
            "fauna transformed with original damage contributions and population slot"
        );
        Ok(new)
    }
    fn fauna_planewalk(&mut self, id: i32, target: Option<Pos>, radius: f32) -> Result<()> {
        let e = &self.entities[&id];
        if e.conditions(self.now) & crate::condition::bit(crate::condition::PARALYZED) != 0 {
            return Ok(());
        }
        let Some(to) = target.filter(|p| e.pos.distance(*p) < radius && e.home.distance(*p) <= 16.)
        else {
            return Ok(());
        };
        ensure!(self.walkable(to), "fauna planewalk endpoint is blocked");
        let old = e.pos;
        let cell = |p: Pos| p.y as usize * self.map.width as usize + p.x as usize;
        ensure!(
            self.cells.remove(cell(old), id),
            "planewalking NPC absent from spatial index"
        );
        self.cells
            .insert(cell(to), id, self.actor_ids.contains(&id));
        let e = self.entities.get_mut(&id).unwrap();
        e.pos = to;
        e.goal = None;
        e.last_step = Pos { x: 0., y: 0. };
        tracing::debug!(
            entity = id,
            x = to.x,
            y = to.y,
            "fauna planewalk reached captured player position"
        );
        Ok(())
    }
    pub(super) fn fauna_death(&mut self, dead: &Entity) -> Result<()> {
        match dead.kind {
            0x6c8 | 0x206 => {
                let kind = if dead.kind == 0x6c8 { 0x6c9 } else { 0x207 };
                let slot = dead.population_slot;
                ensure!(
                    dead.camp.is_none() || slot.is_some(),
                    "death transformation lacks reserved population slot"
                );
                let id = self.spawn_entity(kind, dead.pos, vec![])?;
                let e = self.entities.get_mut(&id).unwrap();
                e.home = dead.home;
                e.camp = dead.camp;
                e.parent.set(*dead.parent, id, &mut self.timed_ids);
                e.quest = dead.quest;
                e.expires.set(*dead.expires, id, &mut self.timed_ids);
                e.activated = Some(self.now);
                if let Some(slot) = slot {
                    self.population[slot].entity = Some(id);
                    self.mark_population(id, slot)?;
                }
                tracing::debug!(
                    entity = dead.id,
                    replacement = id,
                    kind,
                    "death successor starts with its own damage ledger in the original slot"
                );
            }
            0x20e => {
                let camp = dead
                    .camp
                    .context("Green Slime split lacks its owned population slots")?;
                ensure!(
                    self.camps[camp].family.leader == 0x20e,
                    "Green Slime split has another family identity"
                );
                let start = self.camps[camp].start + 1;
                let requested = self.rng.gen_range(3..=5);
                let mut spawned = 0;
                for slot in start..start + requested {
                    ensure!(
                        self.population[slot].entity.is_none(),
                        "Green Slime split slot already occupied"
                    );
                    if !self.population_space(6, 1)? {
                        break;
                    }
                    self.place_member(camp, slot, dead.pos)?;
                    spawned += 1;
                }
                tracing::debug!(
                    entity = dead.id,
                    camp,
                    requested,
                    spawned,
                    "bounded Green Slime death split admitted"
                );
            }
            _ => {}
        }
        Ok(())
    }
    pub(super) fn fauna_mechanics(
        &mut self,
        id: i32,
        target: Option<Pos>,
        elapsed: u64,
    ) -> Result<()> {
        let kind = self.entities[&id].kind;
        if matches!(kind, 0x950 | 0x957) {
            return self.fauna_wave_mechanics(id, target);
        }
        if matches!(kind, 0x903 | 0x904) {
            return self.flamer_mechanics(id, target);
        }
        if kind == 0x955 {
            self.swarm_mechanics(id, target, elapsed);
            return Ok(());
        }
        if matches!(kind, 0x958 | 0x959 | 0x960 | 0x961) {
            let e = self.entities.get_mut(&id).unwrap();
            e.condition |= crate::condition::bit(crate::condition::INVULNERABLE);
            if e.expires.is_none() {
                e.expires
                    .set(Some(self.now + 10000), id, &mut self.timed_ids);
            }
            return Ok(());
        }
        let e = &self.entities[&id];
        let paralyzed =
            e.conditions(self.now) & crate::condition::bit(crate::condition::PARALYZED) != 0;
        if matches!(kind, 0x635 | 0x636 | 0x945 | 0x952) && !paralyzed && self.now >= e.next_aux {
            let choice = u64::from(self.rng.gen_bool(0.5));
            let e = self.entities.get_mut(&id).unwrap();
            e.stage = Some(choice);
            e.next_aux = self.now + 500;
        }
        if kind == 0x952 && elapsed % 8100 >= 7100 && self.now >= self.entities[&id].reaction_until
        {
            self.fauna_planewalk(id, target, 8.)?;
            self.entities.get_mut(&id).unwrap().reaction_until = self.now + 8100 - elapsed % 8100;
        }
        if kind == 0x639 && elapsed >= 7000 && self.now >= self.entities[&id].next_aux {
            self.fauna_planewalk(id, target, 5.)?;
            self.entities.get_mut(&id).unwrap().next_aux = self.now + 7000;
        }
        let e = &self.entities[&id];
        match e.kind {
            0x207 | 0x208 if elapsed >= 60000 => {
                self.transform_fauna(id, e.kind + 1)?;
            }
            0x20d if e.stage != Some(1) && target.is_some_and(|p| e.pos.distance(p) < 2.) => {
                let e = self.entities.get_mut(&id).unwrap();
                e.stage = Some(1);
                e.expires.set(Some(self.now + 50), id, &mut self.timed_ids);
                tracing::debug!(
                    entity = id,
                    "Gray Blob armed its once-only proximity ring and withdrawal"
                );
            }
            0x6c9 => {
                let phase = elapsed / 1000 % 2;
                if e.stage != Some(phase) {
                    self.entities.get_mut(&id).unwrap().stage = Some(phase);
                    self.flash_inland(id, if phase == 0 { 0xffffff00 } else { 0xffff0000 }, 1., 1.);
                }
            }
            0x6c5 if elapsed >= 5000 => {
                self.transform_fauna(id, 0x7f22)?;
            }
            0x7f22 if elapsed >= 10000 => {
                self.transform_fauna(id, 0x6c5)?;
            }
            0x623 if e.hp < 200 && e.stage != Some(1) => {
                let e = self.entities.get_mut(&id).unwrap();
                e.stage = Some(1);
                e.size = 150;
                self.flash_inland(id, 0xffff0000, 0.5, 1000000.);
                tracing::debug!(entity = id, "wounded Desert Werewolf enraged");
            }
            _ => {}
        }
        Ok(())
    }
    pub(super) fn fauna_movement(
        &self,
        id: i32,
        target: Option<Pos>,
        elapsed: u64,
    ) -> Result<(Motion, f32)> {
        let e = &self.entities[&id];
        if e.pos.distance(e.home) > 16. {
            return Ok((
                Motion::Protect {
                    anchor: Anchor::Home,
                    distance: 1.,
                },
                8.,
            ));
        }
        if e.kind == 0x955 {
            return Ok(self.swarm_movement(id, target, elapsed));
        }
        if matches!(e.kind, 0x903 | 0x904) && e.stage != Some(0) {
            return Ok((Motion::Stationary, 0.));
        }
        if matches!(e.kind, 0x635 | 0x636) && target.is_some_and(|p| e.pos.distance(p) < 10.) {
            if e.stage == Some(1) {
                return Ok((
                    Motion::Orbit {
                        anchor: Anchor::Player,
                        radius: 3.,
                    },
                    10.,
                ));
            }
            if target.is_some_and(|p| e.pos.distance(p) > 2.) {
                return Ok((
                    Motion::Chase {
                        sight: 10.,
                        stop: 2.,
                    },
                    10.,
                ));
            }
        }
        let owner = match e.kind {
            0x635 | 0x636 => Some((0x637, 3., 10.)),
            0x620 | 0x622 => Some((0x623, 2., 4.)),
            0x948 | 0x949 => Some((0x947, 2., 8.)),
            0x946 => Some((0x945, 1., 6.)),
            _ => None,
        };
        if let Some((kind, distance, speed)) = owner {
            let leader = if e.camp.is_some() {
                self.camp_member(id, kind)?
            } else {
                None
            };
            if let Some(leader) = leader {
                let gap = e.pos.distance(self.entities[&leader].pos);
                if gap < 10. && matches!(e.kind, 0x948 | 0x949) {
                    return Ok((
                        Motion::Orbit {
                            anchor: Anchor::CampEntity(leader),
                            radius: distance,
                        },
                        speed,
                    ));
                }
                if gap > distance && gap < 10. {
                    return Ok((
                        Motion::Protect {
                            anchor: Anchor::CampEntity(leader),
                            distance,
                        },
                        speed,
                    ));
                }
            }
            if e.kind == 0x946 && e.hp < 400 && target.is_some_and(|p| e.pos.distance(p) < 16.) {
                return Ok((Motion::Retreat { distance: 16. }, 6.));
            }
            return Ok((Motion::Wander, 4.));
        }
        if matches!(e.kind, 0x945 | 0x952) && e.stage == Some(1) {
            return Ok((Motion::Wander, 4.));
        }
        match e.kind {
            0x6c4 if elapsed % 2000 >= 1000 => return Ok((Motion::Stationary, 0.)),
            0x6c7 if elapsed % 3500 < 2500 && target.is_some_and(|p| e.pos.distance(p) < 10.) => {
                return Ok((
                    Motion::Orbit {
                        anchor: Anchor::Player,
                        radius: 2.,
                    },
                    8.,
                ));
            }
            0x6d1 if elapsed % 4000 < 2000 && target.is_some_and(|p| e.pos.distance(p) < 10.) => {
                return Ok((
                    Motion::Orbit {
                        anchor: Anchor::Player,
                        radius: 3.,
                    },
                    8.,
                ));
            }
            _ => {}
        }
        let (motion, speed) = crate::encounter::movement(e.kind, e.hp, elapsed);
        if let Motion::Chase { sight, stop } = motion
            && target.is_none_or(|p| e.pos.distance(p) <= stop || e.pos.distance(p) >= sight)
        {
            return Ok((Motion::Wander, 4.));
        }
        Ok((motion, speed))
    }
}
