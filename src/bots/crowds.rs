use super::*;
use crate::world::view::PlayerMarker;
mod cooperation;
pub(super) use cooperation::available as cooperative_available;

pub(super) struct Crowd {
    pub knows_minimap: bool,
    contacts: BTreeMap<i32, (PlayerMarker, u64)>,
    approaching: Option<(i32, i32)>,
    company: BTreeMap<(i32, i32), (u64, bool)>,
    cooperation: cooperation::Cooperation,
}
impl Crowd {
    pub fn new(knows_minimap: bool) -> Self {
        Self {
            knows_minimap,
            contacts: BTreeMap::new(),
            approaching: None,
            company: BTreeMap::new(),
            cooperation: cooperation::Cooperation::default(),
        }
    }
    pub fn observe(&mut self, v: &PlayerView, content: &Content) {
        self.cooperation.observe(
            cooperation::binding(v),
            v.elapsed_ms,
            v.objects
                .iter()
                .filter(|p| content.objects[&p.kind].enemy)
                .map(|p| (p.id, p.kind)),
        );
        self.cooperation
            .company(v.elapsed_ms, |peer| cooperation::visible(v, peer), |_| true);
    }
    pub fn receive(&mut self, packet: &Packet, v: &PlayerView, content: &Content) -> Result<()> {
        self.cooperation.receive(
            packet,
            cooperation::binding(v),
            v.elapsed_ms,
            |id| {
                v.objects
                    .iter()
                    .find(|p| p.id == id && content.objects[&p.kind].enemy)
                    .map(|p| p.kind)
            },
            |id| {
                v.objects
                    .iter()
                    .find(|p| p.id == id && p.hp > 0)
                    .and_then(|p| p.account)
            },
        )
    }
    pub fn cooperator(
        &mut self,
        v: &PlayerView,
        eligible: impl Fn(Pos) -> bool,
    ) -> Option<PlayerMarker> {
        let ((id, account), position) = self.cooperation.company(
            v.elapsed_ms,
            |peer| cooperation::visible(v, peer),
            eligible,
        )?;
        let name = v
            .objects
            .iter()
            .find(|p| p.id == id)
            .expect("validated cooperative peer missing")
            .name
            .clone();
        Some(PlayerMarker {
            id,
            account,
            position,
            name,
        })
    }
    pub fn len(&self) -> usize {
        self.contacts.len()
    }
    pub fn keeping_company(&self, id: i32) -> bool {
        self.company
            .iter()
            .any(|((peer, _), (_, active))| *peer == id && *active)
    }
    pub fn heed_call(&mut self, v: &PlayerView, marker: PlayerMarker) {
        let id = marker.id;
        self.contacts.insert(id, (marker, v.time_ms));
        self.approach_selected(id);
    }
    pub fn approach_selected(&mut self, id: i32) {
        let (peer, seen) = self
            .contacts
            .get(&id)
            .expect("selected passive crowd cue missing");
        let key = (peer.id, peer.account);
        // Bound the whole outing, including a moving stranger we never catch up with.
        if self.company.len() < 4 && !self.company.contains_key(&key) {
            self.company.insert(key, (*seen + 90_000, true));
            self.approaching = Some(key);
        }
    }
    pub fn attend(
        &mut self,
        v: &PlayerView,
        content: &Content,
        cognition: &pursuits::Cognition,
        inspecting: bool,
    ) {
        let published = |(id, account): &(i32, i32)| {
            v.player_markers
                .iter()
                .any(|p| p.id == *id && p.account == *account)
        };
        self.company
            .retain(|_, (until, active)| *active || v.time_ms < *until);
        for (peer, (until, active)) in &mut self.company {
            if !*active {
                continue;
            }
            let marker = v.player_markers.iter().find(|p| (p.id, p.account) == *peer);
            let reason = if v.time_ms >= *until {
                Some("company_window_ended")
            } else if marker.is_none() {
                Some("lost_public_marker")
            } else if marker.is_some_and(|p| p.position.distance(v.position) > 36.) {
                Some("separated")
            } else {
                None
            };
            if let Some(reason) = reason {
                *active = false;
                *until = v.time_ms + 30_000;
                tracing::info!(
                    account = v.session.account,
                    target = peer.0,
                    peer = peer.1,
                    reason,
                    "autonomous crowd company released"
                );
            }
        }
        if self.approaching.is_some_and(|peer| {
            !published(&peer) || self.company.get(&peer).is_none_or(|(_, active)| !*active)
        }) {
            self.approaching = None;
        }
        self.contacts.retain(|id, (_, seen)| {
            v.time_ms.saturating_sub(*seen) < 6000 && v.player_markers.iter().any(|p| p.id == *id)
        });
        let occupied = v
            .objects
            .iter()
            .filter(|o| content.objects[&o.kind].enemy && o.hp > 0)
            .count()
            + usize::from(inspecting);
        let budget = cognition.attention.saturating_sub(occupied);
        let mut cues: Vec<_> = v
            .player_markers
            .iter()
            .filter(|p| self.knows_minimap || v.objects.iter().any(|o| o.id == p.id))
            .collect();
        cues.sort_by(|a, b| {
            a.position
                .distance(v.position)
                .total_cmp(&b.position.distance(v.position))
                .then(a.id.cmp(&b.id))
        });
        for (index, cue) in cues.into_iter().enumerate() {
            let peer = (cue.id, cue.account);
            if index >= budget && self.approaching != Some(peer) && !self.keeping_company(cue.id) {
                continue;
            }
            if !self.contacts.contains_key(&cue.id) {
                tracing::info!(
                    account = v.session.account,
                    peer = cue.account,
                    object = cue.id,
                    world = v.session.world,
                    "autonomous crowd cue noticed"
                );
            }
            self.contacts.insert(cue.id, (cue.clone(), v.time_ms));
            if self.approaching == Some((cue.id, cue.account))
                && let Some(reached) = v.objects.iter().find(|p| {
                    p.id == cue.id
                        && p.account == Some(cue.account)
                        && p.hp > 0
                        && p.position.distance(v.position) <= 7.
                })
            {
                tracing::info!(
                    account = v.session.account,
                    target = cue.id,
                    peer = cue.account,
                    distance = reached.position.distance(v.position),
                    reason = "approach_arrived_shared_xp",
                    "autonomous crowd company joined"
                );
                self.approaching = None;
            }
        }
    }
    pub fn destination<'a>(
        &'a self,
        v: &PlayerView,
        group: Option<&adventures::Adventure>,
        eligible_position: impl Fn(Pos) -> bool,
    ) -> Option<&'a PlayerMarker> {
        if !matches!(v.map.name.as_str(), "Realm" | "Pirate Cave" | "Snake Pit") {
            return None;
        }
        self.attended_destination(v.time_ms, v.position, v.session.account, group, |p| {
            survivable(v, p) && eligible_position(p)
        })
    }
    fn attended_destination(
        &self,
        time_ms: u64,
        position: Pos,
        account: i32,
        group: Option<&adventures::Adventure>,
        eligible_position: impl Fn(Pos) -> bool,
    ) -> Option<&PlayerMarker> {
        self.contacts
            .values()
            .filter(|(p, seen)| {
                *seen == time_ms
                    && eligible_position(p.position)
                    && (self.company.len() < 4 || self.company.contains_key(&(p.id, p.account)))
                    && self
                        .company
                        .get(&(p.id, p.account))
                        .is_none_or(|(_, active)| *active)
            })
            .min_by(|(a, _), (b, _)| {
                // Different accounts approach different edges; nobody owns the outing's destination.
                let score = |p: &PlayerMarker| {
                    let shared = group.is_some_and(|g| {
                        !g.finished && g.members.iter().any(|m| m.account == p.account)
                    }) || self.contacts.values().any(|(q, seen)| {
                        *seen == time_ms && q.id != p.id && q.position.distance(p.position) < 14.
                    });
                    p.position.distance(position) - if shared { 8. } else { 0. }
                        + ((p.account as u32 ^ account as u32).wrapping_mul(2654435761) % 9) as f32
                };
                // Small score changes do not justify abandoning company already being approached.
                let approaching = |p: &PlayerMarker| {
                    self.approaching == Some((p.id, p.account)) || self.keeping_company(p.id)
                };
                approaching(b)
                    .cmp(&approaching(a))
                    .then_with(|| score(a).total_cmp(&score(b)))
                    .then(a.id.cmp(&b.id))
            })
            .map(|(p, _)| p)
    }
}

pub(super) fn survivable(v: &PlayerView, position: Pos) -> bool {
    let center = Pos {
        x: v.map.width as f32 / 2.,
        y: v.map.height as f32 / 2.,
    };
    let depth = position.distance(center) / (v.map.width.min(v.map.height) as f32 / 2.);
    // Keep PR514's public Realm-depth gates for every voluntary teleport destination.
    v.map.name != "Realm"
        || depth >= 0.55
        || v.character.level >= 20
        || (depth >= 0.35 && v.character.level >= 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn company() -> Crowd {
        let mut crowd = Crowd::new(true);
        for (id, account, x, y) in [(10, 1, 7.5, 0.), (20, 3, 9., 5.)] {
            crowd.contacts.insert(
                id,
                (
                    PlayerMarker {
                        id,
                        account,
                        name: format!("peer{account}"),
                        position: Pos { x, y },
                    },
                    1000,
                ),
            );
        }
        crowd
    }

    fn destination(crowd: &Crowd, x: f32) -> Option<i32> {
        crowd
            .attended_destination(1000, Pos { x, y: 0. }, 2, None, |_| true)
            .map(|p| p.id)
    }

    #[test]
    fn selected_company_survives_a_better_scoring_peer() {
        let mut crowd = company();
        assert_eq!(destination(&crowd, 0.), Some(10));
        crowd.approach_selected(10);
        crowd.contacts.get_mut(&10).unwrap().0.position.x = 10.;
        assert_eq!(destination(&crowd, 2.5), Some(10));
        crowd.approaching = None;
        crowd.company.clear();
        assert_eq!(destination(&crowd, 2.5), Some(20));
    }

    #[test]
    fn blocked_approach_yields_to_reachable_company() {
        let mut crowd = company();
        crowd.approach_selected(10);
        crowd.contacts.get_mut(&10).unwrap().0.position.x = 10.;
        assert_eq!(destination(&crowd, 2.5), Some(10));
        let blocked = BTreeSet::from([tile(Pos { x: 10., y: 0. })]);
        let peer = crowd.attended_destination(1000, Pos { x: 2.5, y: 0. }, 2, None, |p| {
            !blocked.contains(&tile(p))
        });
        assert_eq!(peer.map(|p| p.id), Some(20));
    }

    #[test]
    fn persistence_does_not_override_freshness_completion_or_company_eligibility() {
        let mut crowd = company();
        crowd.approach_selected(10);
        crowd.contacts.get_mut(&10).unwrap().1 = 999;
        assert_eq!(destination(&crowd, 0.), Some(20));
        crowd.contacts.get_mut(&10).unwrap().1 = 1000;
        crowd.company.insert((10, 1), (2000, false));
        assert_eq!(destination(&crowd, 0.), Some(20));
        crowd.company.clear();
        crowd.contacts.get_mut(&10).unwrap().0.position.x = 100.;
        assert_eq!(destination(&crowd, 0.), Some(10));
        crowd.contacts.remove(&10);
        assert_eq!(destination(&crowd, 0.), Some(20));
    }
}
