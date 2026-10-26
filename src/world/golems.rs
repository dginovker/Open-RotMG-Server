use super::*;
use crate::encounter::{Anchor, Motion};
pub(super) const ROOTS: &[u16] = &[0x62d, 0x62e, 0x62f, 0x631, 0x632, 0x633];
#[cfg(test)]
thread_local! { static QUERY_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

impl World {
    pub(super) fn golem_movement(&self, id: i32, elapsed: u64) -> Result<(Motion, f32)> {
        let e = &self.entities[&id];
        let camp = e.camp.context("golem lacks its bounded satellite camp")?;
        if ROOTS.contains(&e.kind) {
            return Ok(if e.pos.distance(e.home) > 16. {
                (
                    Motion::Protect {
                        anchor: Anchor::Home,
                        distance: 1.,
                    },
                    10.,
                )
            } else {
                crate::encounter::movement(e.kind, e.hp, elapsed)
            });
        }
        let nearest = |a: &&Entity, b: &&Entity| {
            e.pos
                .distance(a.pos)
                .total_cmp(&e.pos.distance(b.pos))
                .then_with(|| a.id.cmp(&b.id))
        };
        let root = self
            .camp_members(camp)
            .filter(|x| {
                #[cfg(test)]
                QUERY_VISITS.with(|visits| visits.set(visits.get() + 1));
                ROOTS.contains(&x.kind)
            })
            .min_by(nearest);
        if matches!(e.kind, 0x6d5..=0x6d7)
            && let Some(hub) = self
                .camp_members(camp)
                .filter(|x| {
                    #[cfg(test)]
                    QUERY_VISITS.with(|visits| visits.set(visits.get() + 1));
                    matches!(x.kind, 0x6d2..=0x6d4)
                })
                .min_by(nearest)
        {
            return Ok((
                Motion::StrictOrbit {
                    anchor: Anchor::CampEntity(hub.id),
                    radius: 1.,
                },
                10.,
            ));
        }
        Ok(match root {
            Some(root) => (
                Motion::StrictOrbit {
                    anchor: Anchor::CampEntity(root.id),
                    radius: 3.,
                },
                5.,
            ),
            None => (Motion::Stationary, 0.),
        })
    }
    pub(super) fn orphan_satellites(&mut self, removed: &Entity) {
        if !ROOTS.contains(&removed.kind) {
            return;
        }
        let Some(camp) = removed.camp else {
            return;
        };
        if self.camp_members(camp).any(|e| ROOTS.contains(&e.kind)) {
            return;
        }
        let satellites: Vec<_> = self
            .camp_members(camp)
            .filter(|e| matches!(e.kind, 0x6d2..=0x6d7))
            .map(|e| e.id)
            .collect();
        for id in satellites {
            self.entities.get_mut(&id).unwrap().expires.set(
                Some(self.now + 30000),
                id,
                &mut self.timed_ids,
            );
            self.npc_global_ids.insert(id);
        }
        tracing::debug!(
            camp,
            until = self.now + 30000,
            "orphaned owned golem satellites scheduled to decay"
        );
    }
}
