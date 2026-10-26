use super::*;
pub(super) struct Birth {
    owner: i32,
    tag: i32,
    home: Pos,
    position: Pos,
    kind: u16,
    maximum: usize,
    deadline: u64,
}
impl World {
    pub(super) fn pending_named(&self, tag: i32, kind: Option<u16>) -> usize {
        self.named_births
            .iter()
            .filter(|b| b.tag == tag && kind.is_none_or(|kind| b.kind == kind))
            .count()
    }
    pub(super) fn named_toss(
        &mut self,
        owner: i32,
        kind: u16,
        maximum: usize,
        positions: &[Pos],
    ) -> Result<usize> {
        crate::encounter::validate(&self.content, kind)?;
        let e = &self.entities[&owner];
        let tag = e.quest.context("named toss lacks encounter identity")?;
        let home = e.home;
        if e.conditions(self.now) & crate::condition::bit(crate::condition::STUNNED) != 0 {
            return Ok(0);
        }
        let mut admitted = 0;
        for &position in positions {
            if !self.walkable(position) {
                tracing::debug!(owner, kind, "named toss refused: blocked landing");
                continue;
            }
            let own = self
                .entities
                .values()
                .filter(|e| e.quest == Some(tag))
                .count()
                + self.pending_named(tag, None);
            let live = self
                .entities
                .values()
                .filter(|e| e.quest == Some(tag) && e.kind == kind)
                .count()
                + self.pending_named(tag, Some(kind));
            if own >= 32 || live >= maximum {
                break;
            }
            ensure!(
                self.entities
                    .values()
                    .filter(|e| e.parent.is_some() || e.quest.is_some())
                    .count()
                    + self.named_births.len()
                    + self.abyss_pending_count()
                    < 256,
                "shared summon budget exhausted before toss"
            );
            if kind != 0xe3b {
                self.telegraph_throw(owner, position, 0xffffbf00);
            }
            self.named_births.push(Birth {
                owner,
                tag,
                home,
                position,
                kind,
                maximum,
                deadline: self.now + 1500,
            });
            admitted += 1;
        }
        tracing::debug!(
            owner,
            kind,
            requested = positions.len(),
            admitted,
            "named tosses admitted within owned and shared budgets"
        );
        Ok(admitted)
    }
    pub(super) fn configure_landmark_child(&mut self, id: i32) {
        let e = self.entities.get_mut(&id).unwrap();
        match e.kind {
            0x679..=0x67c => {
                e.expires.set(None, id, &mut self.timed_ids);
            }
            0xdc0 | 0xdc1 => {
                e.expires.set(None, id, &mut self.timed_ids);
                e.home = e.pos;
            }
            0xd52 => {
                e.script
                    .get_or_insert_with(landmark_ai::Script::default)
                    .no_experience = true
            }
            0x942 => e
                .expires
                .set(Some(self.now + 17000), id, &mut self.timed_ids),
            0xd55 => {
                e.expires.set(None, id, &mut self.timed_ids);
                e.stage = None;
                e.condition |= crate::condition::bit(crate::condition::INVULNERABLE);
            }
            0x943 => {
                e.expires.set(None, id, &mut self.timed_ids);
                e.condition |= crate::condition::bit(crate::condition::INVULNERABLE);
            }
            0xe3b => {
                e.expires.set(None, id, &mut self.timed_ids);
                e.condition |= crate::condition::bit(crate::condition::INVINCIBLE);
            }
            0xe4f => e
                .expires
                .set(Some(self.now + 10000), id, &mut self.timed_ids),
            _ => {}
        }
    }
    fn land_named(&mut self, birth: Birth) -> Result<()> {
        ensure!(
            self.walkable(birth.position),
            "admitted named landing became blocked"
        );
        let old = self
            .entities
            .values()
            .filter(|e| e.quest == Some(birth.tag) && e.kind == birth.kind)
            .map(|e| e.id)
            .collect::<BTreeSet<_>>();
        self.quest_birth(
            (birth.tag, birth.home, birth.position),
            birth.kind,
            birth.maximum,
            1,
            0.,
        )?;
        let mut born = self
            .entities
            .values()
            .filter(|e| e.quest == Some(birth.tag) && e.kind == birth.kind && !old.contains(&e.id))
            .map(|e| e.id);
        let id = born
            .next()
            .context("admitted named toss did not create an entity")?;
        ensure!(
            born.next().is_none(),
            "one named toss created multiple entities"
        );
        self.configure_landmark_child(id);
        tracing::debug!(
            owner = birth.owner,
            entity = id,
            kind = birth.kind,
            "named toss landed independently of owner survival"
        );
        Ok(())
    }
    pub(super) fn named_birth_step(&mut self) {
        let mut i = 0;
        while i < self.named_births.len() {
            if self.named_births[i].deadline > self.now {
                i += 1;
                continue;
            }
            let birth = self.named_births.remove(i);
            let (owner, kind, position) = (birth.owner, birth.kind, birth.position);
            if let Err(error) = self.land_named(birth) {
                tracing::error!(owner,kind,x=position.x,y=position.y,%error,"admitted named birth refused");
            }
        }
    }
}
