use super::*;

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Purpose {
    Equipment,
    Experience,
    Speed,
}
impl Purpose {
    fn description(self) -> &'static str {
        match self {
            Self::Equipment => "look for better gear together",
            Self::Experience => "level up together",
            Self::Speed => "hunt for a Snake Pit and earn a Speed potion",
        }
    }
    pub(super) fn public_ready(self, p: &crate::world::view::VisibleObject, c: &Content) -> bool {
        self != Self::Speed
            || (p.level.is_some_and(|l| l >= 14)
                && p.max_hp >= 350
                && p.hp * 4 >= p.max_hp * 3
                && p.equipped.first().is_some_and(|k| {
                    *k >= 0 && c.objects[&(*k as u16)].tier.is_some_and(|t| t >= 4)
                }))
    }
    pub(super) fn ready(self, v: &PlayerView, c: &Content) -> bool {
        ready(v, c) && (self != Self::Speed || progression::ready(v, c, self.portal()))
    }
    pub fn portal(self) -> u16 {
        match self {
            Self::Equipment | Self::Experience => 0x717,
            Self::Speed => 0x718,
        }
    }
}
#[derive(Clone, Debug)]
pub(super) struct Member {
    pub account: i32,
    pub name: String,
}
#[derive(Clone, Debug)]
pub(super) struct Adventure {
    pub members: Vec<Member>,
    pub purpose: Purpose,
    pub until: u64,
    pub finished: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Phase {
    Seeking,
    Preparing,
    Adventuring,
    Regrouping,
    Finished,
}
impl Adventure {
    pub(super) fn phase(&self, v: &PlayerView) -> Phase {
        if self.finished {
            Phase::Finished
        } else if matches!(v.map.name.as_str(), "Nexus" | "Vault") {
            Phase::Preparing
        } else if self.members.iter().any(|m| {
            m.account != v.session.account
                && !v.objects.iter().any(|o| o.account == Some(m.account))
        }) {
            Phase::Regrouping
        } else {
            Phase::Adventuring
        }
    }
}
#[derive(Clone, Debug)]
pub(super) struct Offer {
    pub from: Member,
    pub to: Member,
    pub purpose: Purpose,
    pub until: u64,
    pub text: String,
    pub(super) answer_started: bool,
}
#[derive(Clone, Debug)]
pub(super) enum Plan {
    Invite { peer: Member, purpose: Purpose },
    Answer(Offer),
}
#[derive(Default)]
pub(crate) struct Society {
    actors: BTreeSet<i32>,
    offers: Vec<Offer>,
    trips: Vec<Adventure>,
    cooldown: BTreeMap<i32, u64>,
    friends: BTreeMap<i32, BTreeSet<i32>>,
    ignored: BTreeMap<i32, BTreeSet<i32>>,
    next_call: u64,
    pub(super) speech: dialogue::PublicSpeech,
}
impl Society {
    pub(super) fn register(&mut self, account: i32, lists: crate::store::AccountLists) {
        self.actors.insert(account);
        // Ordinary lock lists survive avatar death and already participate in release backups.
        self.friends
            .insert(account, lists[0].iter().copied().take(4).collect());
        self.ignored.insert(account, lists[1].clone());
    }
    pub(super) fn unregister(&mut self, account: i32) {
        self.leave(account);
        self.actors.remove(&account);
        self.friends.remove(&account);
        self.ignored.remove(&account);
        self.cooldown.remove(&account);
    }
    pub(super) fn leave(&mut self, account: i32) {
        self.offers
            .retain(|o| o.from.account != account && o.to.account != account);
        for trip in &mut self.trips {
            trip.members.retain(|p| p.account != account);
        }
        self.trips.retain(|t| !t.members.is_empty());
        tracing::info!(account, "autonomous adventure released");
    }
    fn expire(&mut self, now: u64) {
        self.offers.retain(|o| o.until > now);
        self.trips.retain(|t| {
            if t.until <= now {
                tracing::info!(
                    members = ?t.members,
                    finished = t.finished,
                    "autonomous adventure expired"
                );
                false
            } else {
                true
            }
        });
    }
    pub(super) fn group(&mut self, account: i32, now: u64) -> Option<Adventure> {
        self.expire(now);
        self.trips
            .iter()
            .find(|t| t.members.iter().any(|p| p.account == account))
            .cloned()
    }
    fn occupied(&self, account: i32) -> bool {
        self.trips
            .iter()
            .any(|t| t.members.iter().any(|m| m.account == account))
    }
    pub(super) fn outgoing(&mut self, account: i32, now: u64) -> Option<Offer> {
        self.expire(now);
        self.offers
            .iter()
            .find(|o| o.from.account == account)
            .cloned()
    }
    pub(super) fn next(
        &mut self,
        brain: &Brain,
        v: &PlayerView,
        content: &Content,
        language: bool,
    ) -> Option<Plan> {
        let now = v.elapsed_ms;
        self.expire(now);
        if brain.agreement.is_some() || v.lifecycle.busy {
            return None;
        }
        let controlled = self.actors.contains(&v.session.account);
        if let Some(o) = self
            .offers
            .iter_mut()
            .find(|o| o.to.account == v.session.account && controlled && !o.answer_started)
        {
            if !v.objects.iter().any(|p| {
                p.account == Some(o.from.account) && p.position.distance(v.position) <= 12.
            }) {
                return None;
            }
            o.answer_started = true;
            self.cooldown.insert(v.session.account, now + 90000);
            return Some(Plan::Answer(o.clone()));
        }
        if now < self.next_call
            || self
                .cooldown
                .get(&v.session.account)
                .is_some_and(|t| *t > now)
            || self
                .offers
                .iter()
                .any(|o| o.from.account == v.session.account)
            || !ready(v, content)
        {
            return None;
        }
        let trip = self
            .trips
            .iter()
            .find(|t| t.members.iter().any(|m| m.account == v.session.account));
        if trip.is_some_and(|t| t.finished || t.members.len() >= 6) {
            return None;
        }
        let purpose = if let Some(trip) = trip {
            trip.purpose
        } else if [0, 2].iter().any(|s| {
            v.character.items[*s] < 0
                || content.objects[&(v.character.items[*s] as u16)]
                    .tier
                    .is_some_and(|t| t < 4)
        }) {
            Purpose::Equipment
        } else if v.character.level < 14 {
            Purpose::Experience
        } else if progression::ready(v, content, 0x718)
            && v.character.base_stats[4] < content.classes[&v.character.class].caps[4]
        {
            Purpose::Speed
        } else {
            return None;
        };
        let peer = v
            .objects
            .iter()
            .filter(|p| {
                p.account.is_some_and(|a| {
                    a != v.session.account
                        && !self.occupied(a)
                        && self.cooldown.get(&a).is_none_or(|until| *until <= now)
                        && (language || self.actors.contains(&a))
                }) && !self
                    .ignored
                    .get(&v.session.account)
                    .is_some_and(|list| p.account.is_some_and(|a| list.contains(&a)))
                    && purpose.public_ready(p, content)
                    && p.hp > 0
                    && p.position.distance(v.position) <= 10.
                    && !self.offers.iter().any(|o| {
                        Some(o.to.account) == p.account || Some(o.from.account) == p.account
                    })
            })
            .max_by_key(|p| {
                let a = p.account.expect("filtered identity");
                let familiar = self
                    .friends
                    .get(&v.session.account)
                    .is_some_and(|f| f.contains(&a));
                let variation = ((now / 90000)
                    ^ (a as u64).wrapping_mul(2654435761)
                    ^ v.session.account as u64)
                    % 64;
                (
                    (if familiar { 24 } else { 0 }) + variation,
                    std::cmp::Reverse(p.id),
                )
            })?;
        self.next_call = now + 5000;
        self.cooldown.insert(v.session.account, now + 90000);
        Some(Plan::Invite {
            peer: Member {
                account: peer.account.expect("filtered identity"),
                name: peer.name.clone(),
            },
            purpose,
        })
    }
    pub(super) fn validate(&self, owner: i32, plan: &Plan, now: u64) -> Result<()> {
        let purpose = match plan {
            Plan::Invite { purpose, .. } => *purpose,
            Plan::Answer(o) => o.purpose,
        };
        let organizer = match plan {
            Plan::Invite { .. } => owner,
            Plan::Answer(o) => o.from.account,
        };
        ensure!(
            self.trips
                .iter()
                .find(|t| t.members.iter().any(|m| m.account == organizer))
                .is_none_or(|t| !t.finished && t.members.len() < 6 && t.purpose == purpose),
            "outing no longer recruiting for this purpose"
        );
        match plan {
            Plan::Invite { peer, .. } => ensure!(
                owner != peer.account
                    && !self.occupied(peer.account)
                    && !self.offers.iter().any(|o| o.until > now
                        && [o.from.account, o.to.account]
                            .iter()
                            .any(|a| *a == owner || *a == peer.account)),
                "invitation no longer available"
            ),
            Plan::Answer(offer) => ensure!(
                self.offers
                    .iter()
                    .any(|o| o.from.account == offer.from.account
                        && o.to.account == offer.to.account
                        && o.until == offer.until
                        && o.purpose == offer.purpose
                        && (owner == o.from.account || owner == o.to.account)
                        && o.until > now)
                    && !self.occupied(offer.to.account),
                "invitation expired or participants committed elsewhere"
            ),
        }
        Ok(())
    }
    pub(super) fn apply(
        &mut self,
        owner: Member,
        plan: Plan,
        joined: bool,
        text: String,
        now: u64,
    ) {
        match plan {
            Plan::Invite { peer, purpose } => {
                tracing::info!(
                    account = owner.account,
                    peer = peer.account,
                    ?purpose,
                    text,
                    "autonomous adventure offered"
                );
                self.offers.push(Offer {
                    from: owner,
                    to: peer,
                    purpose,
                    until: now + 60000,
                    text,
                    answer_started: false,
                });
            }
            Plan::Answer(offer) => {
                self.offers.retain(|o| {
                    o.from.account != offer.from.account && o.to.account != offer.to.account
                });
                if joined {
                    tracing::info!(account=offer.from.account,peer=offer.to.account,?offer.purpose,"autonomous adventure joined");
                    if let Some(trip) = self
                        .trips
                        .iter_mut()
                        .find(|t| t.members.iter().any(|p| p.account == offer.from.account))
                    {
                        trip.members.push(offer.to);
                    } else {
                        self.trips.push(Adventure {
                            members: vec![offer.from, offer.to],
                            purpose: offer.purpose,
                            until: now + 600000,
                            finished: false,
                        });
                    }
                } else {
                    tracing::info!(account = owner.account, "autonomous adventure declined");
                }
            }
        }
    }
    pub(super) fn remember(&mut self, account: i32, peer: i32) -> bool {
        let friends = self
            .friends
            .get_mut(&account)
            .expect("registered social account");
        if friends.contains(&peer) || friends.len() >= 4 {
            return false;
        }
        friends.insert(peer);
        true
    }
    pub(super) fn completed(
        &mut self,
        account: i32,
        v: &PlayerView,
        previous: [u64; 2],
    ) -> Vec<i32> {
        let Some(trip) = self
            .trips
            .iter_mut()
            .find(|t| t.members.iter().any(|p| p.account == account))
        else {
            return vec![];
        };
        let n = usize::from(trip.purpose == Purpose::Speed);
        if !v.lifecycle.alive
            || v.lifecycle.death_pending
            || [v.character.pirate_caves, v.character.snake_pits][n] <= previous[n]
        {
            return vec![];
        }
        let peers: Vec<_> = trip
            .members
            .iter()
            .filter(|p| {
                p.account != account
                    && v.objects
                        .iter()
                        .any(|o| o.account == Some(p.account) && o.hp > 0)
            })
            .map(|p| p.account)
            .collect();
        trip.finished = true;
        trip.until = trip.until.min(v.elapsed_ms + 10000);
        tracing::info!(account, ?peers, "autonomous adventure objective cleared");
        peers
    }
}
pub(super) fn policy_reply(
    request: &dialogue::Request,
    intent: dialogue::Intent,
) -> Result<dialogue::Reply> {
    let text = match (&request.plan, intent) {
        (Some(Plan::Invite { purpose, .. }), dialogue::Intent::Invite) => {
            format!("{}, want to {}?", request.name, purpose.description())
        }
        (Some(Plan::Answer(_)), dialogue::Intent::Join) => "yes, let's go together".into(),
        (Some(Plan::Answer(_)), dialogue::Intent::Decline) => {
            "not ready, need to recover first".into()
        }
        _ => anyhow::bail!("no policy speech for this intention"),
    };
    crate::world::chat::validate(&text)?;
    Ok(dialogue::Reply {
        intent,
        reply: text,
    })
}
pub(super) fn ready(v: &PlayerView, content: &Content) -> bool {
    v.character.level >= 1
        && matches!(v.map.name.as_str(), "Realm" | "Nexus")
        && v.character.hp * 4 >= v.character.stats[0] * 3
        && v.character.mp * 3 >= v.character.stats[1]
        && !v.objects.iter().any(|o| {
            content.objects[&o.kind].enemy && o.hp > 0 && o.position.distance(v.position) < 6.
        })
}
impl Brain {
    pub(super) fn adventure_request(
        &self,
        plan: Plan,
        v: &PlayerView,
        content: &Content,
    ) -> Result<dialogue::Request> {
        let (peer, intents, event) = match &plan {
            Plan::Invite { peer, purpose } => (
                peer,
                vec![dialogue::Intent::Invite],
                format!(
                    "Invite {} to {}. This is your own plan, not a player command. Begin with the intended recipient's name followed by comma and space, once. Do not claim they already agreed.",
                    peer.name,
                    purpose.description()
                ),
            ),
            Plan::Answer(o) => (
                &o.from,
                vec![if o.purpose.ready(v, content) {
                    dialogue::Intent::Join
                } else {
                    dialogue::Intent::Decline
                }],
                format!(
                    "{} invited you: {}. Join means share the activity while making your own travel and combat choices. Decline if not ready.",
                    o.from.name, o.text
                ),
            ),
        };
        let actor = v
            .objects
            .iter()
            .find(|o| o.account == Some(peer.account))
            .context("invitation peer no longer visible")?;
        let payload = serde_json::json!({"bot":{"name":self.profile.name,"world":v.map.name,"level":v.character.level,"hp":v.character.hp,"max_hp":v.character.stats[0],"mp":v.character.mp,"priorities":{"recovery":self.profile.recovery,"resources":self.profile.resources,"companions":self.profile.companions}},"speaker":{"name":peer.name,"level":actor.level},"allowed_intents":intents,"message":event});
        Ok(dialogue::Request {
            speaker: actor.id,
            name: peer.name.clone(),
            session: self.session,
            created: std::time::Instant::now(),
            payload,
            plan: Some(plan),
        })
    }
}
