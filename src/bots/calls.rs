use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Activity {
    Help,
    Gods,
    Demon,
    Sphinx,
    Snake,
}
impl Activity {
    fn parse(text: &str) -> Option<Self> {
        let text = text.to_ascii_lowercase();
        let words = text.split_whitespace().collect::<Vec<_>>();
        match words.as_slice() {
            ["snake"] | ["snake", "pit"] | ["snake", "here"] | ["tp", "snake"] => Some(Self::Snake),
            ["tp"] => Some(Self::Help),
            ["tp", "gods"] => Some(Self::Gods),
            ["tp", "demon"] => Some(Self::Demon),
            ["tp", "sphynix" | "sphinx"] => Some(Self::Sphinx),
            _ => None,
        }
    }
}
struct Seen {
    world: i32,
    account: i32,
    activity: Activity,
    until: u64,
}
struct Opportunity {
    world: i32,
    actor: i32,
    account: i32,
    name: String,
    activity: Activity,
    until: u64,
}
#[derive(Default)]
pub(super) struct Calls {
    seen: VecDeque<Seen>,
    opportunity: Option<Opportunity>,
}
impl Calls {
    pub(super) fn observe(&mut self, v: &PlayerView) {
        self.seen
            .retain(|s| s.world == v.session.world && v.elapsed_ms < s.until);
        if self
            .opportunity
            .as_ref()
            .is_some_and(|o| o.world != v.session.world || v.elapsed_ms >= o.until)
        {
            let o = self.opportunity.take().expect("expired call exists");
            tracing::info!(account=v.session.account,caller=o.account,target=o.actor,world=o.world,activity=?o.activity,"public activity call expired");
        }
    }
}
impl Brain {
    #[cfg(test)]
    fn call_ready(
        &self,
        v: &PlayerView,
        c: &Content,
        activity: Activity,
        caller: i32,
    ) -> Result<bool> {
        Ok(self.call_rejection(v, c, activity, caller)?.is_none())
    }
    fn speed_motive(&self, v: &PlayerView, c: &Content) -> Option<&'static str> {
        if v.character.base_stats[4] < c.classes[&v.character.class].caps[4] {
            Some("max_speed")
        } else if self.mind.market.wants_speed(v) {
            Some("speed_for_market")
        } else {
            None
        }
    }
    fn call_rejection(
        &self,
        v: &PlayerView,
        c: &Content,
        activity: Activity,
        caller: i32,
    ) -> Result<Option<&'static str>> {
        if !v.map.allow_teleport
            || !v.lifecycle.alive
            || v.lifecycle.busy
            || v.lifecycle.death_pending
            || v.lifecycle.departing
            || v.lifecycle.quarantined
            || !matches!(v.lifecycle.output, OutputState::Live)
        {
            return Ok(Some("lifecycle_or_map"));
        }
        if v.character.items[0] < 0 || v.character.hp * 4 < v.character.stats[0] * 3 {
            return Ok(Some("health_or_weapon"));
        }
        if v.time_ms < self.next_teleport
            || v.tick < self.recover_until
            || matches!(self.goal, Goal::Recover | Goal::Dodge | Goal::Remedy(_))
        {
            return Ok(Some("cooldown_or_recovering"));
        }
        if self.group.is_some()
            || self.agreement.is_some()
            || self.companion.is_some()
            || self.proposing
            || self
                .offered
                .as_ref()
                .is_some_and(|o| o.until > v.elapsed_ms)
            || self.mind.market.trade.is_some()
            || self
                .mind
                .market
                .bargain
                .as_ref()
                .is_some_and(|b| b.resolution.is_none())
        {
            return Ok(Some("social_or_trade_commitment"));
        }
        let interested = match activity {
            Activity::Help => {
                (self.profile.companions >= 6. || self.mind.social.known(caller))
                    && progression::ready(v, c, 0x717)
            }
            Activity::Gods | Activity::Demon => {
                v.map.name == "Realm" && progression::ready(v, c, 0x718)
            }
            Activity::Sphinx => v.map.name == "Realm" && events::ready(v, c),
            Activity::Snake => v.map.name == "Realm" && progression::ready(v, c, 0x718),
        };
        if !interested {
            return Ok(Some("activity_readiness"));
        }
        if activity == Activity::Snake {
            if self.speed_motive(v, c).is_none() {
                return Ok(Some("no_speed_need"));
            }
            if !v.character.items.iter().skip(4).any(|k| *k == -1) {
                return Ok(Some("no_reward_space"));
            }
            if !v.character.items.iter().skip(4).any(|k| *k == 0xa22) {
                return Ok(Some("no_health_reserve"));
            }
        }
        let weapon = c.get(v.character.items[0] as u16)?;
        let projectile = weapon
            .projectiles
            .get(&0)
            .context("weapon projectile missing")?;
        let range = projectile.speed * projectile.lifetime / 10000.;
        if v.objects.iter().any(|o| {
            if o.hp <= 0 || !c.objects[&o.kind].enemy {
                return false;
            }
            let distance = o.position.distance(v.position);
            let can_fight = progression::can_fight(v, c, o);
            let engaged = matches!(&self.goal, Goal::Fight(target) | Goal::Hunt(target) if *target == o.id)
                && distance < range * 1.3;
            // Seeing the advertised activity is not itself an ongoing fight.
            distance <= 5.
                || (!can_fight && distance < 8.)
                || (can_fight && progression::vulnerable(o) && (distance < range || engaged))
        }) {
            return Ok(Some("nearby_combat_or_danger"));
        }
        for bag in v
            .objects
            .iter()
            .filter(|o| c.objects[&o.kind].class == "Container")
        {
            if progression::pickup(v, c, bag, self.mind.future_class)?.is_some() {
                return Ok(Some("useful_loot"));
            }
        }
        Ok(None)
    }
    pub(super) fn receive_call(
        &mut self,
        v: &PlayerView,
        c: &Content,
        actor: i32,
        name: &str,
        recipient: &str,
        text: &str,
    ) -> Result<()> {
        let Some(activity) = Activity::parse(text) else {
            return Ok(());
        };
        if !recipient.is_empty()
            || actor <= 0
            || actor == v.session.player
            || self.session != (v.session.world, v.session.player, v.session.character)
        {
            tracing::info!(
                account = v.session.account,
                actor,
                name,
                world = v.session.world,
                ?activity,
                reason = "not_public_or_current_session",
                "public activity call declined"
            );
            return Ok(());
        }
        let Some(peer) = v.player_markers.iter().find(|p| {
            p.id == actor && p.name == name && p.account > 0 && p.account != v.session.account
        }) else {
            tracing::info!(
                account = v.session.account,
                actor,
                name,
                ?activity,
                reason = "unbound_speaker",
                "public activity call declined"
            );
            return Ok(());
        };
        let caller = peer.account;
        self.calls.observe(v);
        if self
            .calls
            .seen
            .iter()
            .any(|s| s.world == v.session.world && s.account == caller && s.activity == activity)
        {
            return Ok(());
        }
        if self.calls.seen.len() >= 32 {
            tracing::info!(
                account = v.session.account,
                caller,
                actor,
                world = v.session.world,
                ?activity,
                reason = "dedup_capacity",
                "public activity call declined"
            );
            return Ok(());
        }
        self.calls.seen.push_back(Seen {
            world: v.session.world,
            account: caller,
            activity,
            until: v
                .elapsed_ms
                .checked_add(60_000)
                .context("public call dedup deadline overflow")?,
        });
        let rejection = if self.calls.opportunity.is_some() {
            Some("another_call_pending")
        } else if !crowds::survivable(v, peer.position) {
            Some("unsafe_realm_destination")
        } else if v.objects.iter().find(|p| p.id == actor).is_some_and(|p| {
            p.account != Some(caller)
                || p.name != name
                || p.equipped.is_empty()
                || p.hp <= 0
                || p.hp * 4 < p.max_hp * 3
        }) {
            Some("caller_unfit")
        } else if peer.position.distance(v.position) <= 7. {
            Some("already_near_caller")
        } else {
            self.call_rejection(v, c, activity, caller)?
        };
        if let Some(reason) = rejection {
            tracing::info!(
                account = v.session.account,
                caller,
                actor,
                ?activity,
                reason,
                "public activity call declined"
            );
            return Ok(());
        }
        if self.rng.gen_bool(0.5) {
            self.calls.opportunity = Some(Opportunity {
                world: v.session.world,
                actor,
                account: caller,
                name: name.into(),
                activity,
                until: v
                    .elapsed_ms
                    .checked_add(30_000)
                    .context("public call opportunity deadline overflow")?,
            });
            tracing::info!(
                account = v.session.account,
                caller,
                actor,
                world = v.session.world,
                ?activity,
                motive = if activity == Activity::Snake {
                    self.speed_motive(v, c)
                } else {
                    Some("shared_activity")
                },
                "public activity call accepted"
            );
        } else {
            tracing::info!(
                account = v.session.account,
                caller,
                actor,
                world = v.session.world,
                ?activity,
                reason = "chance",
                "public activity call declined"
            );
        }
        Ok(())
    }
    pub(super) fn call_target(&mut self, v: &PlayerView, c: &Content) -> Result<Option<i32>> {
        self.calls.observe(v);
        let Some(o) = self.calls.opportunity.take() else {
            return Ok(None);
        };
        // Public identity, not the claimed activity, authorizes the ordinary target lookup.
        let target = v
            .player_markers
            .iter()
            .find(|p| p.id == o.actor && p.account == o.account && p.name == o.name);
        let rejection = if o.world != v.session.world || target.is_none() {
            Some("lost_public_identity")
        } else if v.objects.iter().find(|p| p.id == o.actor).is_some_and(|p| {
            p.hp <= 0
                || p.hp * 4 < p.max_hp * 3
                || p.equipped.is_empty()
                || p.account != Some(o.account)
                || p.name != o.name
        }) {
            Some("caller_unfit")
        } else if target.is_some_and(|p| !crowds::survivable(v, p.position)) {
            Some("unsafe_realm_destination")
        } else if target.is_some_and(|p| p.position.distance(v.position) <= 7.) {
            Some("already_near_caller")
        } else {
            self.call_rejection(v, c, o.activity, o.account)?
        };
        if let Some(reason) = rejection {
            tracing::info!(account=v.session.account,caller=o.account,target=o.actor,activity=?o.activity,reason,"public activity call declined");
            return Ok(None);
        }
        let marker = target.expect("validated call marker").clone();
        self.crowd.heed_call(v, marker);
        tracing::info!(account=v.session.account,caller=o.account,target=o.actor,world=v.session.world,activity=?o.activity,"public activity call dispatched");
        Ok(Some(o.actor))
    }
}

#[cfg(test)]
mod tests;
