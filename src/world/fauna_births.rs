use super::*;

pub(super) struct Birth {
    owner: i32,
    pub(super) camp: usize,
    pub(super) slot: usize,
    kind: u16,
    previous: Option<i32>,
    position: Pos,
    deadline: u64,
    lifetime: Option<u64>,
}
impl World {
    pub(super) fn fauna_wave_mechanics(&mut self, id: i32, target: Option<Pos>) -> Result<()> {
        let e = &self.entities[&id];
        let kind = e.kind;
        let pos = e.pos;
        let camp = e.camp.context("fauna wave lacks an owned family")?;
        if kind == 0x950 {
            let count = self
                .entities
                .values()
                .filter(|e| e.camp == Some(camp) && e.kind == 0x951)
                .count();
            let pending = self.fauna_births.iter().any(|b| b.camp == camp);
            if e.stage.is_none()
                || self.now >= e.next_aux
                || self.now >= e.reaction_until && count < 3 && !pending
            {
                let positions = (0..5)
                    .map(|i| {
                        let a = i as f32 * std::f32::consts::TAU / 5.;
                        Pos {
                            x: pos.x + 4. * a.cos(),
                            y: pos.y + 4. * a.sin(),
                        }
                    })
                    .collect::<Vec<_>>();
                self.fauna_toss(id, 0x951, &positions, true, None)?;
                let e = self.entities.get_mut(&id).unwrap();
                e.stage = Some(1);
                e.next_aux = self.now + 10000;
                e.reaction_until = self.now + 2000;
            }
            return Ok(());
        }
        ensure!(kind == 0x957, "unsupported fauna wave owner");
        if e.stage.is_some() && self.now < e.next_aux {
            return Ok(());
        }
        let phase = match e.stage {
            None | Some(2) => 0,
            Some(0) => 1,
            Some(1) => 2,
            _ => bail!("invalid Urgle wave phase"),
        };
        if phase == 0 && target.is_none_or(|p| pos.distance(p) >= 14.) {
            return Ok(());
        }
        let waves: &[(u16, &[(f32, f32)])] = match phase {
            0 => &[(0x958, &[(230., 8.), (180., 9.), (140., 8.)])],
            1 => &[(0x959, &[(200., 7.), (240., 9.), (280., 9.), (320., 7.)])],
            2 => &[
                (0x960, &[(45., 1.), (45., 6.), (225., 10.), (225., 5.)]),
                (0x961, &[(135., 1.), (135., 6.), (315., 10.), (315., 5.)]),
            ],
            _ => unreachable!(),
        };
        for (kind, landings) in waves {
            let positions = landings
                .iter()
                .map(|(angle, radius)| {
                    let a = angle.to_radians();
                    Pos {
                        x: pos.x + radius * a.cos(),
                        y: pos.y + radius * a.sin(),
                    }
                })
                .collect::<Vec<_>>();
            self.fauna_toss(id, *kind, &positions, false, Some(10000))?;
        }
        let e = self.entities.get_mut(&id).unwrap();
        e.stage = Some(phase);
        e.next_aux = self.now + if phase == 2 { 3400 } else { 1000 };
        Ok(())
    }
    pub(super) fn pending_fauna_count(&self, terrain: u8) -> usize {
        self.fauna_births
            .iter()
            .filter(|b| {
                self.camps[b.camp].family.terrain == terrain
                    && self.population[b.slot]
                        .entity
                        .is_none_or(|id| !self.entities.contains_key(&id))
            })
            .count()
    }
    pub(super) fn fauna_toss(
        &mut self,
        owner: i32,
        kind: u16,
        positions: &[Pos],
        replace: bool,
        lifetime: Option<u64>,
    ) -> Result<usize> {
        let camp = self.entities[&owner]
            .camp
            .context("fauna toss lacks owned population slots")?;
        let family = self.camps[camp].family;
        ensure!(
            family.leader == self.entities[&owner].kind,
            "fauna toss owner identity mismatch"
        );
        let start = self.camps[camp].start;
        let slots = (start + 1..start + family.size())
            .filter(|slot| {
                self.population[*slot].kind == kind
                    && (replace
                        || self.population[*slot]
                            .entity
                            .is_none_or(|id| !self.entities.contains_key(&id)))
                    && !self.fauna_births.iter().any(|b| b.slot == *slot)
            })
            .collect::<Vec<_>>();
        let mut admitted = 0;
        for (slot, position) in slots.into_iter().zip(positions.iter().copied()) {
            if !self.walkable(position) {
                tracing::debug!(
                    owner,
                    kind,
                    x = position.x,
                    y = position.y,
                    "fauna toss refused: blocked landing"
                );
                continue;
            }
            if self.population[slot]
                .entity
                .is_none_or(|id| !self.entities.contains_key(&id))
                && !self.population_space(family.terrain, 1)?
            {
                break;
            }
            ensure!(
                self.fauna_births.len() < 256,
                "pending fauna birth budget exhausted"
            );
            self.telegraph_throw(owner, position, 0xffffbf00);
            self.fauna_births.push(Birth {
                owner,
                camp,
                slot,
                kind,
                previous: self.population[slot].entity,
                position,
                deadline: self.now + 1500,
                lifetime,
            });
            admitted += 1;
        }
        tracing::debug!(
            owner,
            kind,
            requested = positions.len(),
            admitted,
            "fauna tosses admitted within owned slot and regional budgets"
        );
        Ok(admitted)
    }
    fn land_fauna(&mut self, birth: Birth) -> Result<()> {
        let slot = self
            .population
            .get(birth.slot)
            .context("admitted birth slot disappeared")?;
        ensure!(
            slot.camp == Some(birth.camp) && slot.kind == birth.kind,
            "admitted birth ownership changed"
        );
        ensure!(
            slot.entity.is_none() || slot.entity == birth.previous,
            "admitted birth slot reused by another entity"
        );
        ensure!(
            self.walkable(birth.position),
            "admitted fauna landing became blocked"
        );
        if let Some(old) = slot.entity
            && self.entities.contains_key(&old)
        {
            self.remove_entity(old);
        }
        self.place_member(birth.camp, birth.slot, birth.position)?;
        let id = self.population[birth.slot]
            .entity
            .context("landed fauna lacks entity")?;
        let e = self.entities.get_mut(&id).unwrap();
        e.expires.set(
            birth.lifetime.map(|ms| self.now + ms),
            id,
            &mut self.timed_ids,
        );
        e.activated = Some(self.now);
        if matches!(birth.kind, 0x958 | 0x959 | 0x960 | 0x961) {
            e.condition |= crate::condition::bit(crate::condition::INVULNERABLE);
        }
        tracing::debug!(
            owner = birth.owner,
            entity = id,
            kind = birth.kind,
            "admitted fauna toss landed independently of owner survival"
        );
        Ok(())
    }
    pub(super) fn fauna_birth_step(&mut self) {
        let mut i = 0;
        while i < self.fauna_births.len() {
            if self.fauna_births[i].deadline > self.now {
                i += 1;
                continue;
            }
            let birth = self.fauna_births.remove(i);
            let (owner, kind, position) = (birth.owner, birth.kind, birth.position);
            if let Err(error) = self.land_fauna(birth) {
                tracing::error!(owner,kind,x=position.x,y=position.y,%error,"admitted fauna birth refused");
            }
        }
    }
}
