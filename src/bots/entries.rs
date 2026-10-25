use super::*;
use crate::world::view::ObservedDungeonEntry;

#[derive(Default)]
pub(super) struct Entries {
    seen: BTreeSet<(i32, i32, i32)>,
    target: Option<ObservedDungeonEntry>,
}
impl Entries {
    fn choose(
        &mut self,
        v: &PlayerView,
        eligible: &BTreeSet<i32>,
        rng: &mut impl Rng,
    ) -> Option<i32> {
        if self.target.as_ref().is_some_and(|e| {
            e.source_world != v.session.world
                || e.expires_ms <= v.elapsed_ms
                || !v
                    .objects
                    .iter()
                    .any(|p| p.id == e.portal && p.kind == e.portal_kind && p.usable)
        }) {
            self.target = None;
        }
        if let Some(e) = &self.target {
            return eligible.contains(&e.portal).then_some(e.portal);
        }
        for e in &v.dungeon_entries {
            if e.source_world != v.session.world
                || e.observed_ms > v.elapsed_ms
                || e.expires_ms <= v.elapsed_ms
                || e.account <= 0
                || e.account == v.session.account
                || !eligible.contains(&e.portal)
                || !v
                    .objects
                    .iter()
                    .any(|p| p.id == e.portal && p.kind == e.portal_kind)
                || self.seen.contains(&(e.source_world, e.portal, e.account))
            {
                continue;
            }
            // Retain refusals for this binding, rather than evicting them into fresh rolls.
            if self.seen.len() == 256 {
                tracing::warn!(
                    account = v.session.account,
                    "witnessed dungeon choice capacity reached"
                );
                break;
            }
            self.seen.insert((e.source_world, e.portal, e.account));
            let joining = rng.gen_bool(0.5);
            tracing::info!(
                account = v.session.account,
                entrant = e.account,
                portal = e.portal,
                joining,
                "witnessed dungeon entry choice"
            );
            if joining {
                self.target = Some(e.clone());
                return Some(e.portal);
            }
        }
        None
    }
}

impl Brain {
    pub(super) fn witnessed_portal(
        &mut self,
        v: &PlayerView,
        content: &Content,
        choices: &[(f32, Goal, Pos)],
        range: f32,
    ) -> Option<i32> {
        if v.lifecycle.busy
            || self.agreement.is_some()
            || self.companion.is_some()
            || self.group.is_some()
            || self.proposing
            || self
                .offered
                .as_ref()
                .is_some_and(|o| o.until > v.elapsed_ms)
            || self.mind.social.gift_active()
            || self.mind.market.incoming.is_some()
            || self.mind.market.trade.is_some()
            || self
                .mind
                .market
                .bargain
                .as_ref()
                .is_some_and(|b| b.resolution.is_none())
            || v.character.items[0] < 0
            || v.character.hp * 4 < v.character.stats[0] * 3
            || choices.iter().any(|(score, goal, pos)| {
                !self.blocked.contains_key(&tile(*pos))
                    && (matches!(goal, Goal::Loot(_))
                        || matches!(goal, Goal::Fight(_) | Goal::Hunt(_))
                            && pos.distance(v.position) < range
                        || *score >= 95. && !matches!(goal, Goal::Portal(_)))
            })
            || v.objects.iter().any(|o| {
                o.hp > 0 && content.objects[&o.kind].enemy && o.position.distance(v.position) <= 5.
            })
        {
            return None;
        }
        let eligible = v
            .objects
            .iter()
            .filter(|p| {
                p.usable
                    && !self.blocked.contains_key(&tile(p.position))
                    && crate::dungeon::DUNGEONS
                        .iter()
                        .any(|(kind, _, _, _)| *kind == p.kind)
                    && match p.kind {
                        0x717 | 0x719 | 0x733 | 0x742 => progression::ready(v, content, 0x717),
                        0x890 | 0x730 | crate::dungeon::tomb::PORTAL => events::ready(v, content),
                        _ => progression::ready(v, content, 0x718),
                    }
            })
            .map(|p| p.id)
            .collect();
        self.entries.choose(v, &eligible, &mut self.rng)
    }
}

#[cfg(test)]
mod tests;
