use super::*;
#[cfg(test)]
mod tests;

const WINDOW: u64 = 10_000;
const COMPANY: u64 = 60_000;
type Peer = (i32, i32);

#[derive(Default)]
pub(super) struct Cooperation {
    binding: Option<[i32; 4]>,
    enemies: BTreeMap<i32, Enemy>,
    recent: Option<(Peer, u64)>,
}
struct Enemy {
    kind: u16,
    seen: u64,
    own: Option<u64>,
    peer: Option<(Peer, u64)>,
    hits: BTreeMap<(i32, u8), u64>,
}
impl Cooperation {
    pub fn observe(
        &mut self,
        binding: [i32; 4],
        now: u64,
        enemies: impl Iterator<Item = (i32, u16)>,
    ) {
        if self.binding != Some(binding) {
            self.release("session_changed");
            self.enemies.clear();
            self.binding = Some(binding);
        }
        self.enemies
            .retain(|_, e| now.saturating_sub(e.seen) < WINDOW);
        for (id, kind) in enemies {
            self.seen(id, kind, now);
        }
        if self.recent.is_some_and(|(_, until)| now >= until) {
            self.release("expired");
        }
    }
    fn seen(&mut self, id: i32, kind: u16, now: u64) {
        if self.enemies.get(&id).is_some_and(|e| e.kind != kind) {
            self.enemies.remove(&id);
        }
        if !self.enemies.contains_key(&id) && self.enemies.len() == 8 {
            let oldest = *self
                .enemies
                .iter()
                .min_by_key(|(_, e)| (e.own.is_some() || e.peer.is_some(), e.seen))
                .unwrap()
                .0;
            self.enemies.remove(&oldest);
        }
        self.enemies
            .entry(id)
            .or_insert_with(|| Enemy {
                kind,
                seen: now,
                own: None,
                peer: None,
                hits: BTreeMap::new(),
            })
            .seen = now;
    }
    pub fn receive(
        &mut self,
        packet: &Packet,
        binding: [i32; 4],
        now: u64,
        enemy: impl Fn(i32) -> Option<u16>,
        peer: impl Fn(i32) -> Option<i32>,
    ) -> Result<()> {
        if packet.id != 47 || self.binding != Some(binding) {
            return Ok(());
        }
        let mut r = Reader::new(&packet.body);
        let target = r.i32()?;
        for _ in 0..r.u8()? {
            r.u8()?;
        }
        let damage = r.i16()?;
        let killed = r.u8()? != 0;
        let bullet = r.u8()?;
        let shooter = r.i32()?;
        r.done()?;
        if damage <= 0 || shooter == 0 {
            return Ok(());
        }
        if let Some(kind) = enemy(target) {
            self.seen(target, kind, now);
        }
        let Some(e) = self
            .enemies
            .get_mut(&target)
            .filter(|e| now.saturating_sub(e.seen) < WINDOW)
        else {
            return Ok(());
        };
        e.hits.retain(|_, at| now.saturating_sub(*at) < WINDOW);
        if e.hits.contains_key(&(shooter, bullet)) {
            return Ok(());
        }
        if shooter == binding[3] {
            e.own = Some(now);
        } else if let Some(account) = peer(shooter).filter(|account| *account != binding[1]) {
            if e.peer.is_some_and(|((id, account), at)| {
                now.saturating_sub(at) >= WINDOW || peer(id) != Some(account)
            }) {
                e.peer = None;
            }
            if e.peer.is_none_or(|(who, _)| who == (shooter, account)) {
                e.peer = Some(((shooter, account), now));
            } else {
                return Ok(());
            }
        } else {
            return Ok(());
        }
        e.hits.insert((shooter, bullet), now);
        let together = e.own.zip(e.peer).filter(|(own, ((id, account), at))| {
            now.saturating_sub(*own) < WINDOW
                && now.saturating_sub(*at) < WINDOW
                && peer(*id) == Some(*account)
        });
        if let Some((own_hit_ms, (who, peer_hit_ms))) = together {
            let until = now
                .checked_add(COMPANY)
                .context("cooperative company deadline overflow")?;
            let accepted = self.recent.is_none_or(|(current, _)| current != who);
            self.recent = Some((who, until));
            if accepted {
                tracing::info!(
                    account = binding[1],
                    character = binding[2],
                    world = binding[0],
                    enemy = target,
                    peer = who.1,
                    object = who.0,
                    own_hit_ms,
                    peer_hit_ms,
                    shooter,
                    bullet,
                    damage,
                    killed,
                    until,
                    accepted,
                    "autonomous observed shared damage company"
                );
            }
        }
        if killed {
            self.enemies.remove(&target);
        }
        Ok(())
    }
    pub fn company(
        &mut self,
        now: u64,
        visible: impl Fn(Peer) -> std::result::Result<Pos, &'static str>,
        eligible: impl Fn(Pos) -> bool,
    ) -> Option<(Peer, Pos)> {
        let (peer, until) = self.recent?;
        let result = if now >= until {
            Err("expired")
        } else {
            visible(peer).and_then(|p| {
                if eligible(p) {
                    Ok(p)
                } else {
                    Err("route_blocked")
                }
            })
        };
        match result {
            Ok(position) => Some((peer, position)),
            Err(reason) => {
                self.release(reason);
                None
            }
        }
    }
    fn release(&mut self, reason: &'static str) {
        if let Some((peer, until)) = self.recent.take() {
            tracing::info!(
                account = self.binding.map(|b| b[1]),
                peer = peer.1,
                target = peer.0,
                until,
                reason,
                "autonomous cooperative company released"
            );
        }
    }
}

pub(super) fn binding(v: &PlayerView) -> [i32; 4] {
    [
        v.session.world,
        v.session.account,
        v.session.character,
        v.session.player,
    ]
}
pub(super) fn visible(
    v: &PlayerView,
    (id, account): Peer,
) -> std::result::Result<Pos, &'static str> {
    let p = v
        .objects
        .iter()
        .find(|p| p.id == id)
        .ok_or("lost_public_visibility")?;
    if p.account != Some(account) {
        return Err("peer_identity_changed");
    }
    if p.hp <= 0 {
        return Err("peer_dead");
    }
    Ok(p.position)
}

pub(in crate::bots) fn available(
    choices: &[(f32, Goal, Pos)],
    position: Pos,
    range: f32,
    eligible_position: impl Fn(Pos) -> bool,
) -> bool {
    !choices.iter().any(|(score, goal, target)| {
        eligible_position(*target)
            && (*score >= 95.
                || matches!(goal, Goal::Loot(_) | Goal::Hunt(_))
                || matches!(goal, Goal::Fight(_)) && position.distance(*target) < range)
    })
}
