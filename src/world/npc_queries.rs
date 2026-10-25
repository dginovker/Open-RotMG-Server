use super::*;

impl World {
    pub(super) fn npc_candidates(&mut self, audience: &[Pos]) -> Vec<i32> {
        assert!(
            audience.iter().all(|p| p.x.is_finite() && p.y.is_finite()),
            "NPC audience contains nonfinite positions"
        );
        let all_global = self.npc_global_ids.len() == self.actor_ids.len();
        let mut candidates = Vec::new();
        let (mut requested, mut cells) = (0, 0);
        // When every actor is global, a cell query would only repeat required work.
        if !all_global {
            candidates.extend(self.npc_global_ids.iter().copied());
            let width = self.map.width as usize;
            let mut rows = Vec::new();
            let mut rects = Vec::new();
            for p in audience {
                let x0 = ((p.x - 20.) as i32).saturating_sub(1).max(0);
                let y0 = ((p.y - 20.) as i32).saturating_sub(1).max(0);
                let x1 = ((p.x + 20.) as i32)
                    .saturating_add(1)
                    .min(self.map.width - 1);
                let y1 = ((p.y + 20.) as i32)
                    .saturating_add(1)
                    .min(self.map.height - 1);
                if x0 > x1 || y0 > y1 {
                    continue;
                }
                rects.push((x0 as usize, y0 as usize, x1 as usize, y1 as usize));
                for y in y0..=y1 {
                    let start = y as usize * width + x0 as usize;
                    let end = y as usize * width + x1 as usize + 1;
                    requested += end - start;
                    rows.push((start, end));
                }
            }
            rows.sort_unstable();
            let mut merged: Vec<(usize, usize)> = Vec::new();
            for (start, end) in rows {
                if let Some(last) = merged.last_mut()
                    && start <= last.1
                {
                    last.1 = last.1.max(end);
                } else {
                    merged.push((start, end));
                }
            }
            cells += merged.iter().map(|(start, end)| end - start).sum::<usize>();
            // Actor sections never contain scenery; the exact row intervals still decide membership.
            for &(x0, y0, x1, y1) in &rects {
                for id in self.cells.actors_in(x0, y0, x1, y1) {
                    let pos = self.entities[&id].pos;
                    let cell = pos.y as usize * width + pos.x as usize;
                    let i = merged.partition_point(|(start, _)| *start <= cell);
                    if i > 0 && cell < merged[i - 1].1 {
                        candidates.push(id);
                    }
                }
            }
            candidates.sort_unstable();
            candidates.dedup();
        }
        let examined = if all_global {
            self.actor_ids.len()
        } else {
            candidates.len()
        };
        let mut dormant = Vec::new();
        let mut select = |e: &Entity| {
            if e.population_slot.is_some()
                && e.activated.is_none()
                && e.stage.is_none()
                && e.expires.is_none()
                && e.next_shot <= self.now
                && e.next_aux == 0
                && e.reaction_until == 0
                && e.blocked_since.is_none()
                && e.charge.is_none()
                && e.script.is_none()
                && audience.iter().all(|p| e.pos.distance(*p) >= 20.)
            {
                dormant.push(e.id);
                return false;
            }
            let object = &self.content.objects[&e.kind];
            object.class != "Wall"
                && !matches!(e.kind, 0xd5f | 0xd60)
                && (object.enemy || matches!(e.kind, 0x173 | 0xd7d))
        };
        // Fully awake worlds need the fused traversal, not an extra materialized ID pass.
        if all_global {
            candidates = self.actors().filter(|e| select(e)).map(|e| e.id).collect();
        } else {
            candidates
                .retain(|id| select(self.entities.get(id).expect("NPC candidate entity missing")));
        }
        for id in dormant {
            self.npc_global_ids.remove(&id);
        }
        if !all_global {
            self.npc_global_ids.extend(candidates.iter().copied());
        }
        let mut performance = self.performance.borrow_mut();
        for (name, amount) in [
            ("npc_queries", 1),
            ("npc_query_candidates", examined),
            ("npc_query_cells", cells),
            ("npc_query_requested_cells", requested),
            ("npc_selected", candidates.len()),
        ] {
            performance.count(name, amount as u64);
        }
        candidates
    }
}
