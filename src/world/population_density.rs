use super::*;

#[derive(Default)]
pub(super) struct Region {
    pub(super) target: usize,
    pub(super) limit: usize,
    sites: Vec<Pos>,
    kinds: Vec<u16>,
}

impl World {
    pub(super) fn configure_population(&mut self) -> Result<()> {
        ensure!(
            self.population.is_empty(),
            "cannot reconfigure a live population"
        );
        // These reconstruct the pinned Trapped area profile, not recovered official server limits.
        let divisors = [
            0, 100, 250, 300, 300, 200, 200, 200, 200, 250, 250, 100, 150,
        ];
        self.population_regions = std::array::from_fn(|_| Region::default());
        let mut area = [0usize; 13];
        for (i, cell) in self.map.cells.iter().enumerate() {
            let terrain = cell.terrain as usize;
            ensure!(terrain < area.len(), "unsupported Realm terrain {terrain}");
            if terrain == 0 {
                continue;
            }
            area[terrain] += 1;
            let pos = Pos {
                x: (i % self.map.width as usize) as f32 + 0.5,
                y: (i / self.map.width as usize) as f32 + 0.5,
            };
            if cell.object.is_none() && self.walkable(pos) && self.outside_setpieces(pos) {
                self.population_regions[terrain].sites.push(pos);
            }
        }
        for object in self.content.objects.values().filter(|o| {
            o.enemy
                && o.terrain > 0
                && o.spawn_weight.is_some_and(|w| w > 0.)
                && o.per_realm_max.is_none()
        }) {
            crate::encounter::validate(&self.content, object.kind)?;
            crate::loot::validate(&self.content, object.kind)?;
            if let Some(family) = population::FAMILIES
                .iter()
                .find(|f| f.leader == object.kind)
            {
                self.validate_family(family)?;
                let roots = match object.spawn_group {
                    Some(group) => group.max,
                    None => 1,
                };
                ensure!(
                    roots * family.initial() <= 32,
                    "population group exceeds birth headroom"
                );
            }
            self.population_regions[object.terrain as usize]
                .kinds
                .push(object.kind);
        }
        for terrain in 1..=12 {
            let region = &mut self.population_regions[terrain];
            region.target = area[terrain] / divisors[terrain];
            // Reserve a complete maximum-size birth burst without allowing unbounded reinforcements.
            region.limit = if region.target == 0 {
                0
            } else {
                region.target * 3 / 2 + 32
            };
            ensure!(
                region.target == 0 || !region.sites.is_empty() && !region.kinds.is_empty(),
                "terrain {terrain} lacks population sites or species"
            );
            tracing::info!(
                terrain,
                area = area[terrain],
                target = region.target,
                limit = region.limit,
                sites = region.sites.len(),
                species = region.kinds.len(),
                "Realm population reconstruction configured"
            );
        }
        Ok(())
    }
    pub(super) fn populate_realm_population(&mut self) -> Result<()> {
        self.configure_population()?;
        for terrain in 1..=12 {
            self.fill_population(terrain)?;
        }
        Ok(())
    }
    pub(super) fn mark_population(&mut self, id: i32, slot: usize) -> Result<()> {
        ensure!(
            self.population[slot].entity == Some(id),
            "population registration lost slot ownership"
        );
        let terrain = self.population_terrain(&self.population[slot]);
        ensure!((1..=12).contains(&terrain), "invalid population origin");
        let entity = self
            .entities
            .get_mut(&id)
            .context("population entity missing")?;
        ensure!(
            entity.population_slot.is_none(),
            "population entity registered twice"
        );
        ensure!(
            entity.quest.is_none(),
            "ordinary population cannot own a scripted quest"
        );
        ensure!(
            self.non_population_ids.remove(&id),
            "population actor missing from allocation index"
        );
        entity.population_slot = Some(slot);
        self.population_counts[terrain as usize] += 1;
        Ok(())
    }
    pub(super) fn sample_population_group(&mut self, kind: u16) -> usize {
        match self.content.objects[&kind].spawn_group {
            None => 1,
            Some(group) => {
                let u: f64 = self.rng.gen_range(f64::EPSILON..1.);
                let angle: f64 = self.rng.gen_range(0.0..std::f64::consts::TAU);
                let n = group.mean as f64
                    + group.deviation as f64 * (-2. * u.ln()).sqrt() * angle.sin();
                (n as usize).clamp(group.min, group.max)
            }
        }
    }
    fn spawn_population_group(&mut self, terrain: u8, kind: u16) -> Result<bool> {
        let family = population::FAMILIES.iter().find(|f| f.leader == kind);
        let roots = self.sample_population_group(kind);
        let cost = roots * family.map_or(1, |f| f.initial());
        if !self.population_space(terrain, cost)? {
            return Ok(false);
        }
        let center = *self.population_regions[terrain as usize]
            .sites
            .choose(&mut self.rng)
            .context("population has no candidate site")?;
        let clearance = if matches!(kind, 0x604 | 0x953) {
            64.
        } else {
            12.
        };
        let mut groups = Vec::with_capacity(roots);
        let mut occupied = Vec::new();
        for root in 0..roots {
            let mut layout = None;
            for _ in 0..if root == 0 { 1 } else { 32 } {
                let pos = if root == 0 {
                    center
                } else {
                    Pos {
                        x: center.x + self.rng.gen_range(-5.0..5.0),
                        y: center.y + self.rng.gen_range(-5.0..5.0),
                    }
                };
                if !self.family_ground(pos, terrain)
                    || occupied.iter().any(|q: &Pos| q.distance(pos) < 0.75)
                {
                    continue;
                }
                let positions = match family {
                    Some(family) => self.family_layout(pos, family, &occupied),
                    None => Some(vec![pos]),
                };
                if let Some(positions) = positions
                    && positions.iter().all(|pos| {
                        self.players.values().all(|p| p.pos.distance(*pos) >= 12.)
                            && self
                                .map
                                .spawns
                                .iter()
                                .all(|(x, y)| pos.distance(Pos { x: *x, y: *y }) >= clearance)
                    })
                {
                    layout = Some(positions);
                    break;
                }
            }
            let Some(positions) = layout else {
                return Ok(false);
            };
            occupied.extend(positions.iter().copied());
            groups.push(positions);
        }
        for positions in groups {
            match family {
                Some(family) => self.place_camp(family, positions)?,
                None => {
                    let pos = positions[0];
                    let entity = self.spawn_entity(kind, pos, vec![])?;
                    let slot = self.population.len();
                    self.population.push(Spawn {
                        kind,
                        pos,
                        entity: Some(entity),
                        camp: None,
                    });
                    self.mark_population(entity, slot)?;
                }
            }
        }
        Ok(true)
    }
    fn fill_population(&mut self, terrain: u8) -> Result<()> {
        let target = self.population_regions[terrain as usize].target;
        let kinds = self.population_regions[terrain as usize].kinds.clone();
        let (mut refused, mut stalled, mut groups) = (0, 0, 0);
        while self.population_count(terrain) < target && stalled < 128 {
            // SpawnProb's original selector is lost; relative XML weights are an explicit reconstruction choice.
            let kind = *kinds
                .choose_weighted(&mut self.rng, |kind| {
                    self.content.objects[kind]
                        .spawn_weight
                        .expect("validated spawn weight")
                })
                .context("invalid population weights")?;
            if self.spawn_population_group(terrain, kind)? {
                groups += 1;
                stalled = 0;
            } else {
                refused += 1;
                stalled += 1;
            }
        }
        tracing::info!(
            terrain,
            target,
            bodies = self.population_count(terrain),
            groups,
            refused,
            "Realm ordinary population allocated"
        );
        ensure!(
            self.population_count(terrain) >= target,
            "terrain {terrain} cannot fill population: {}/{} bodies after {stalled} consecutive refusals",
            self.population_count(terrain),
            target
        );
        Ok(())
    }
    fn compact_population(&mut self) -> Result<()> {
        let mut old: Vec<_> = std::mem::take(&mut self.population)
            .into_iter()
            .map(Some)
            .collect();
        let mut slots = vec![usize::MAX; old.len()];
        let mut camps = vec![usize::MAX; self.camps.len()];
        for (index, mut camp) in std::mem::take(&mut self.camps).into_iter().enumerate() {
            let range = camp.start..camp.start + camp.family.size();
            if !range
                .clone()
                .any(|i| old[i].as_ref().expect("camp slot missing").entity.is_some())
                && !self.fauna_births.iter().any(|b| b.camp == index)
            {
                continue;
            }
            let new_camp = self.camps.len();
            camps[index] = new_camp;
            camp.start = self.population.len();
            for i in range {
                let mut slot = old[i].take().context("population camp ranges overlap")?;
                let new_slot = self.population.len();
                slot.camp = Some(new_camp);
                slots[i] = new_slot;
                if let Some(id) = slot.entity {
                    let entity = self
                        .entities
                        .get_mut(&id)
                        .context("live population member missing")?;
                    entity.camp = Some(new_camp);
                    entity.population_slot = Some(new_slot);
                }
                self.population.push(slot);
            }
            self.camps.push(camp);
        }
        for (i, slot) in old.into_iter().enumerate() {
            if let Some(slot) = slot
                && slot.camp.is_none()
                && let Some(id) = slot.entity
            {
                slots[i] = self.population.len();
                self.entities
                    .get_mut(&id)
                    .context("live solo missing")?
                    .population_slot = Some(slots[i]);
                self.population.push(slot);
            }
        }
        // Admitted tosses own empty slots even after their parent dies; compaction must preserve that promise.
        for birth in &mut self.fauna_births {
            ensure!(
                camps[birth.camp] != usize::MAX && slots[birth.slot] != usize::MAX,
                "compaction lost an admitted birth"
            );
            birth.camp = camps[birth.camp];
            birth.slot = slots[birth.slot];
        }
        Ok(())
    }
    pub(super) fn repopulate(&mut self) -> Result<()> {
        if self.now >= self.population_next {
            self.population_next = self.now + 60000;
            self.compact_population()?;
            for terrain in 1..=12 {
                if self.population_count(terrain) * 4
                    < self.population_regions[terrain as usize].target * 3
                {
                    self.fill_population(terrain)?;
                }
            }
        }
        self.repopulate_families()
    }
}
