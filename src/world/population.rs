use super::*;

pub(super) struct Role {
    pub kind: u16,
    pub maximum: usize,
    pub initial: usize,
    pub interval: Option<u64>,
}
pub(super) struct Family {
    pub leader: u16,
    pub terrain: u8,
    pub roles: &'static [Role],
    pub random_initial: bool,
}
impl Family {
    pub(super) fn owner(&self, kind: u16) -> u16 {
        match (self.leader, kind) {
            (0x640, 0x63a..=0x63c) => 0x63e,
            (0x650, 0x64a..=0x64d) => 0x64f,
            (0x206, 0x206) => 0x209,
            _ => self.leader,
        }
    }
    pub(super) fn size(&self) -> usize {
        1 + self.roles.iter().map(|r| r.maximum).sum::<usize>()
    }
    pub(super) fn initial(&self) -> usize {
        1 + self
            .roles
            .iter()
            .map(|r| {
                if self.random_initial {
                    r.maximum
                } else {
                    r.initial
                }
            })
            .sum::<usize>()
    }
    fn roll_initial(&self, rng: &mut impl Rng) -> Vec<usize> {
        self.roles
            .iter()
            .map(|r| {
                if self.random_initial {
                    rng.gen_range(r.initial..=r.maximum)
                } else {
                    r.initial
                }
            })
            .collect()
    }
}
macro_rules! family {
    (@script $leader:expr,$terrain:expr; $( $kind:expr,$maximum:expr );+ $(;)?) => {
        Family{leader:$leader,terrain:$terrain,random_initial:false,roles:&[$(Role{kind:$kind,maximum:$maximum,initial:0,interval:None}),+]}
    };
    (@random $leader:expr,$terrain:expr; $( $kind:expr,$maximum:expr,$initial:expr,$interval:expr );+ $(;)?) => {
        Family { leader:$leader, terrain:$terrain, random_initial:true, roles:&[$(Role {kind:$kind,maximum:$maximum,initial:$initial,interval:Some($interval)}),+] }
    };
    (@once $leader:expr,$terrain:expr; $( $kind:expr,$count:expr );+ $(;)?) => {
        Family {leader:$leader,terrain:$terrain,random_initial:false,roles:&[$(Role{kind:$kind,maximum:$count,initial:$count,interval:None}),+]}
    };
    ($leader:expr,$terrain:expr; $( $kind:expr,$maximum:expr,$initial:expr,$interval:expr );+ $(;)?) => {
        Family { leader:$leader, terrain:$terrain, random_initial:false, roles:&[$(Role {kind:$kind,maximum:$maximum,initial:$initial,interval:Some($interval)}),+] }
    };
}
pub(super) const FAMILIES: &[Family] = &[
    family!(0x601,11; 0x600,5,2,10000; 0x601,4,0,10000),
    family!(0x604,11; 0x603,10,5,10000),
    family!(0x953,11; 0x954,5,3,8000),
    family!(0x60d,8; 0x60a,2,2,10000; 0x60c,3,3,8000),
    family!(0x617,9; 0x616,4,2,12000; 0x615,3,2,6000),
    family!(0x61a,9; 0x619,4,2,12000; 0x618,3,2,6000),
    family!(0x609,10; 0x605,2,2,15000; 0x606,4,4,7000; 0x607,1,1,8000),
    family!(0x614,10; 0x612,7,3,12000; 0x613,7,3,12000),
    family!(0x652,1; 0x653,5,3,10000),
    family!(@once 0x905,1; 0x906,1; 0x907,1),
    family!(0x62c,6; 0x629,3,2,10000; 0x62a,2,1,10000; 0x62b,2,1,10000),
    family!(0x61f,7; 0x61c,3,2,10000; 0x61d,2,1,10000; 0x61e,2,1,10000),
    family!(0x649,2; 0x647,4,3,10000; 0x648,3,2,10000),
    family!(0x640,3; 0x63e,1,1,10000; 0x63f,1,1,10000; 0x63a,3,2,10000; 0x63b,3,2,10000; 0x63c,3,2,10000),
    family!(@once 0x900,3; 0x901,1; 0x902,2),
    family!(@once 0x645,4; 0x642,2; 0x643,4; 0x644,2),
    family!(0x650,4; 0x64f,1,1,10000; 0x64a,4,2,10000; 0x64b,3,2,10000; 0x64c,2,1,10000; 0x64d,2,1,10000),
    family!(@once 0x632,5; 0x6d2,1; 0x6d5,1; 0x6d6,1; 0x6d7,1),
    family!(@once 0x633,5; 0x6d2,1; 0x6d5,1; 0x6d6,1; 0x6d7,1),
    Family {
        leader: 0x62d,
        terrain: 7,
        random_initial: false,
        roles: &[
            Role {
                kind: 0x62d,
                maximum: 2,
                initial: 0,
                interval: Some(10000),
            },
            Role {
                kind: 0x6d2,
                maximum: 1,
                initial: 1,
                interval: None,
            },
            Role {
                kind: 0x6d5,
                maximum: 1,
                initial: 1,
                interval: None,
            },
            Role {
                kind: 0x6d6,
                maximum: 1,
                initial: 1,
                interval: None,
            },
            Role {
                kind: 0x6d7,
                maximum: 1,
                initial: 1,
                interval: None,
            },
        ],
    },
    family!(@once 0x62e,7; 0x6d2,1; 0x6d5,1; 0x6d6,1; 0x6d7,1),
    family!(@once 0x62f,6; 0x6d2,1; 0x6d5,1; 0x6d6,1; 0x6d7,1),
    family!(@once 0x631,6; 0x6d2,1; 0x6d5,1; 0x6d6,1; 0x6d7,1),
    family!(@random 0x623,5; 0x622,3,1,10000; 0x620,3,1,10000),
    family!(@random 0x947,6; 0x949,2,1,10000; 0x948,5,2,10000),
    family!(@once 0x945,7; 0x946,1),
    family!(@random 0x637,7; 0x635,3,1,10000; 0x636,2,1,10000; 0x639,2,1,10000),
    family!(0x202,7; 0x202,2,0,45000),
    family!(0x206,4; 0x206,2,0,60000),
    family!(@script 0x20e,6; 0x20f,5),
    family!(@script 0x950,6; 0x951,5),
    family!(@script 0x957,3; 0x958,3; 0x959,4; 0x960,4; 0x961,4),
    family!(@once 0x903,2; 0x904,5),
    family!(@script 0x7f00,9; 0x7f01,5),
];
pub(super) struct Camp {
    pub start: usize,
    pub family: &'static Family,
    next: Vec<Option<u64>>,
}

fn formation(center: Pos, size: usize) -> Vec<Pos> {
    (0..size)
        .map(|i| {
            if i == 0 {
                center
            } else {
                let angle = (i - 1) as f32 * std::f32::consts::TAU / (size - 1) as f32;
                Pos {
                    x: center.x + 2. * angle.cos(),
                    y: center.y + 2. * angle.sin(),
                }
            }
        })
        .collect()
}

fn member_positions(center: Pos, terrain: u8) -> Vec<Pos> {
    let inland = (2..=7).contains(&terrain);
    let angles = if inland { 16 } else { 32 };
    let rings = if inland { 4 } else { 1 };
    (0..angles * rings)
        .map(|i| {
            let angle = (i % angles) as f32 * std::f32::consts::TAU / angles as f32;
            let radius = if inland { (1 + i / angles) as f32 } else { 2. };
            Pos {
                x: center.x + radius * angle.cos(),
                y: center.y + radius * angle.sin(),
            }
        })
        .collect()
}
impl World {
    pub(super) fn camp_members(&self, camp: usize) -> impl Iterator<Item = &Entity> {
        let c = self.camps.get(camp).expect("ordinary camp index missing");
        self.population[c.start..c.start + c.family.size()]
            .iter()
            .enumerate()
            .filter_map(move |(offset, slot)| {
                assert_eq!(slot.camp, Some(camp), "population camp range changed");
                slot.entity.map(|id| {
                    let e = self.entities.get(&id).expect("live camp member missing");
                    assert_eq!(e.camp, Some(camp), "entity camp ownership changed");
                    assert_eq!(
                        e.population_slot,
                        Some(c.start + offset),
                        "entity population slot changed"
                    );
                    e
                })
            })
    }
    pub(super) fn camp_member(&self, id: i32, kind: u16) -> Result<Option<i32>> {
        let e = self
            .entities
            .get(&id)
            .context("camp member entity missing")?;
        let camp = e.camp.context("entity has no ordinary camp")?;
        let c = self
            .camps
            .get(camp)
            .context("ordinary camp index missing")?;
        Ok(self.population[c.start..c.start + c.family.size()]
            .iter()
            .filter(|s| s.kind == kind)
            .filter_map(|s| s.entity)
            .filter(|id| self.entities.contains_key(id))
            .min_by(|a, b| {
                self.entities[a]
                    .pos
                    .distance(e.pos)
                    .total_cmp(&self.entities[b].pos.distance(e.pos))
            }))
    }

    pub(super) fn population_terrain(&self, slot: &Spawn) -> u8 {
        match slot.camp {
            Some(camp) => self.camps[camp].family.terrain,
            None => self.content.objects[&slot.kind].terrain,
        }
    }
    pub(super) fn population_count(&self, terrain: u8) -> usize {
        self.population_counts[terrain as usize]
    }
    pub(super) fn population_space(&self, terrain: u8, count: usize) -> Result<bool> {
        ensure!(
            (1..=12).contains(&terrain),
            "population has unsupported terrain {terrain}"
        );
        Ok(
            self.population_count(terrain) + self.pending_fauna_count(terrain) + count
                <= self.population_regions[terrain as usize].limit,
        )
    }
    pub(super) fn family_ground(&self, pos: Pos, terrain: u8) -> bool {
        self.map
            .cell(pos.x, pos.y)
            .is_some_and(|c| c.terrain == terrain && c.object.is_none())
            && self.walkable(pos)
            && self.outside_setpieces(pos)
            && self
                .entities_near(pos, 0.75)
                .all(|e| e.population_slot.is_none())
    }
    pub(super) fn place_member(&mut self, camp: usize, slot: usize, pos: Pos) -> Result<()> {
        let kind = if matches!(self.population[slot].kind, 0x6d2..=0x6d4) {
            self.rng.gen_range(0x6d2..=0x6d4)
        } else {
            self.population[slot].kind
        };
        self.population[slot].kind = kind;
        ensure!(
            self.population_space(self.camps[camp].family.terrain, 1)?,
            "family birth exceeds regional budget"
        );
        let id = self.spawn_entity(kind, pos, vec![])?;
        let home = self.population[self.camps[camp].start].pos;
        let e = self.entities.get_mut(&id).unwrap();
        // Group affiliation cannot use summon parenthood, which deletes surviving ordinary followers.
        e.camp = Some(camp);
        e.home = home;
        self.population[slot].entity = Some(id);
        self.mark_population(id, slot)?;
        tracing::debug!(camp, entity = id, kind, "family member created");
        Ok(())
    }
    pub(super) fn validate_family(&self, family: &Family) -> Result<()> {
        for kind in std::iter::once(family.leader).chain(family.roles.iter().map(|r| r.kind)) {
            ensure!(
                self.content.get(kind)?.terrain == family.terrain
                    || (self.content.get(kind)?.terrain == 0
                        && matches!(
                            kind,
                            0x6d2..=0x6d7 | 0x951 | 0x958 | 0x959 | 0x960 | 0x961 | 0x7f01
                        )
                        && kind != family.leader),
                "family contradicts XML habitat"
            );
            crate::encounter::validate(&self.content, kind)?;
            crate::loot::validate(&self.content, kind)?;
        }
        ensure!(
            family
                .roles
                .iter()
                .all(|r| r.initial <= r.maximum && r.interval.is_none_or(|interval| interval > 0)),
            "invalid family role bounds"
        );
        ensure!(family.size() <= 32, "family exceeds bounded slot count");
        Ok(())
    }
    pub(super) fn family_layout(
        &self,
        center: Pos,
        family: &Family,
        occupied: &[Pos],
    ) -> Option<Vec<Pos>> {
        if !(2..=7).contains(&family.terrain) {
            let positions = formation(center, family.size());
            return positions
                .iter()
                .all(|p| {
                    self.family_ground(*p, family.terrain)
                        && occupied.iter().all(|q| p.distance(*q) >= 0.75)
                })
                .then_some(positions);
        }
        // Thin, wooded inland habitats need a contour-aware footprint rather than a mandatory empty circle.
        let mut positions = vec![center];
        for p in member_positions(center, family.terrain) {
            if self.family_ground(p, family.terrain)
                && positions.iter().all(|q| p.distance(*q) >= 0.75)
                && occupied.iter().all(|q| p.distance(*q) >= 0.75)
            {
                positions.push(p);
            }
            if positions.len() == family.size() {
                return Some(positions);
            }
        }
        None
    }
    pub(super) fn place_camp(
        &mut self,
        family: &'static Family,
        positions: Vec<Pos>,
    ) -> Result<()> {
        ensure!(
            positions.len() == family.size(),
            "family footprint does not match reserved capacity"
        );
        ensure!(
            self.population_space(family.terrain, family.initial())?,
            "initial family exceeds regional budget"
        );
        let initials = family.roll_initial(&mut self.rng);
        let initial_count = 1 + initials.iter().sum::<usize>();
        let camp = self.camps.len();
        let start = self.population.len();
        self.camps.push(Camp {
            start,
            family,
            next: family
                .roles
                .iter()
                .map(|r| r.interval.map(|interval| self.now + interval))
                .collect(),
        });
        let members: Vec<_> = std::iter::once((family.leader, true))
            .chain(
                family
                    .roles
                    .iter()
                    .zip(&initials)
                    .flat_map(|(r, n)| (0..r.maximum).map(move |i| (r.kind, i < *n))),
            )
            .collect();
        for ((kind, initial), pos) in members.into_iter().zip(positions) {
            let slot = self.population.len();
            self.population.push(Spawn {
                kind,
                pos,
                entity: None,
                camp: Some(camp),
            });
            if initial {
                self.place_member(camp, slot, pos)?;
            }
        }
        tracing::debug!(
            camp,
            leader = family.leader,
            terrain = family.terrain,
            initial = initial_count,
            capacity = family.size(),
            "bounded family allocated"
        );
        Ok(())
    }

    pub(super) fn repopulate_families(&mut self) -> Result<()> {
        // Only player-near owners reproduce; querying occupied cells avoids scanning dormant camps.
        let active: BTreeSet<_> = self
            .players
            .values()
            .filter(|p| p.character.alive && !p.death_pending && !p.departing && !p.paused)
            .flat_map(|p| self.actors_near(p.pos, 20.))
            .filter(|e| e.population_slot.is_some())
            .filter_map(|e| e.camp)
            .collect();
        for camp in active {
            if self.fauna_births.iter().any(|birth| birth.camp == camp) {
                continue;
            }
            let start = self.camps[camp].start;
            let family = self.camps[camp].family;
            let slots = start..start + family.size();
            let mut from = start + 1;
            for (role, r) in family.roles.iter().enumerate() {
                let to = from + r.maximum;
                let Some(interval) = r.interval else {
                    from = to;
                    continue;
                };
                let owner_kind = family.owner(r.kind);
                let owner = slots
                    .clone()
                    .filter_map(|i| self.population[i].entity)
                    .find(|id| self.entities.get(id).is_some_and(|e| e.kind == owner_kind));
                let Some(leader) = owner else {
                    from = to;
                    continue;
                };
                let center = self.entities[&leader].pos;
                if !self.players.values().any(|p| {
                    p.character.alive
                        && !p.death_pending
                        && !p.departing
                        && !p.paused
                        && p.pos.distance(center) < 20.
                }) {
                    from = to;
                    continue;
                }
                let mut deadline =
                    self.camps[camp].next[role].context("reinforcing role lacks its clock")?;
                if family.leader == 0x206 {
                    deadline = deadline.max(
                        self.entities[&leader]
                            .activated
                            .context("adult dragon lacks its growth clock")?
                            + interval,
                    );
                }
                if self.now >= deadline {
                    self.camps[camp].next[role] = Some(self.now + interval);
                    if let Some(slot) = (from..to).find(|i| self.population[*i].entity.is_none()) {
                        if !self.population_space(family.terrain, 1)? {
                            tracing::debug!(camp, role, "reinforcement refused: regional budget");
                            from = to;
                            continue;
                        }
                        let pos = member_positions(center, family.terrain)
                            .into_iter()
                            .find(|p| {
                                self.family_ground(*p, family.terrain)
                                    && self.players.values().all(|q| q.pos.distance(*p) >= 0.75)
                                    && self
                                        .actors()
                                        .filter(|e| e.camp == Some(camp))
                                        .all(|e| e.pos.distance(*p) >= 0.75)
                            });
                        if let Some(pos) = pos {
                            self.place_member(camp, slot, pos)?;
                            tracing::debug!(
                                camp,
                                leader,
                                role,
                                kind = r.kind,
                                interval,
                                "family role reinforced"
                            );
                        } else {
                            tracing::debug!(
                                camp,
                                role,
                                "reinforcement refused: no clear habitat position"
                            );
                        }
                    }
                }
                from = to;
            }
        }
        Ok(())
    }
}
