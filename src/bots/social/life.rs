use super::*;
use serde::{Deserialize, Serialize};

const HEALTH: i32 = 0xa22;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) enum Interaction {
    HealedBy,
    ReceivedHealth,
    GaveHealth,
    Refused,
    Recognized,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Episode {
    pub account: i32,
    pub name: String,
    pub own_character: i32,
    pub interaction: Interaction,
    pub amount: i32,
}
#[derive(Clone, Debug, PartialEq)]
struct Gift {
    account: i32,
    player: i32,
    name: String,
    giving: bool,
    until: u64,
    session: [i32; 3],
    requested: bool,
    consent: bool,
    acceptance: Option<crate::store::GiftAcceptance>,
    cancelled: bool,
}
impl Gift {
    fn matches(&self, acceptance: &crate::store::GiftAcceptance, v: &PlayerView) -> bool {
        self.session == [v.session.world, v.session.player, v.session.character]
            && acceptance.session == self.session
            && acceptance.own.account == v.session.account
            && acceptance.own.character == v.session.character
            && acceptance.peer.account == self.account
            && acceptance.peer_player == self.player
            && acceptance.peer_name == self.name
            && acceptance.giving == self.giving
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct Life {
    pub episodes: VecDeque<Episode>,
    #[serde(skip)]
    gift: Option<Gift>,
    #[serde(skip)]
    outgoing: Option<String>,
    #[serde(skip)]
    heal_notice: i32,
    #[serde(skip)]
    heal_tick: u64,
    #[serde(skip)]
    heal_origin: Option<(f32, f32)>,
    #[serde(skip)]
    heal_request: Option<(i32, u64)>,
    #[serde(skip)]
    next_request: u64,
    #[serde(skip)]
    waiting: Option<(i32, u64)>,
    #[serde(skip)]
    recognized: BTreeSet<i32>,
    #[serde(skip)]
    private_trial: Option<(i32, u64)>,
}
fn health(v: &PlayerView) -> usize {
    v.character
        .items
        .iter()
        .skip(4)
        .filter(|k| **k == HEALTH)
        .count()
}
impl Life {
    pub(in crate::bots) fn gift_active(&self) -> bool {
        self.gift.is_some()
    }
    pub(in crate::bots) fn bind(&mut self) {
        self.gift = None;
        self.outgoing = None;
        self.heal_notice = 0;
        self.heal_tick = 0;
        self.heal_origin = None;
        self.heal_request = None;
        self.waiting = None;
        self.next_request = 0;
        self.recognized.clear();
        self.private_trial = None;
    }
    pub(crate) fn validate(&self, own: i32) -> Result<()> {
        ensure!(
            self.episodes.len() <= 32
                && self.episodes.iter().all(|e| e.account > 0
                    && e.account != own
                    && !e.name.is_empty()
                    && e.name.len() <= 16
                    && e.own_character > 0
                    && e.amount >= 0),
            "invalid factual acquaintance episodes"
        );
        Ok(())
    }
    fn remember(
        &mut self,
        v: &PlayerView,
        account: i32,
        name: String,
        interaction: Interaction,
        amount: i32,
    ) {
        let episode = Episode {
            account,
            name,
            own_character: v.session.character,
            interaction,
            amount,
        };
        if self.episodes.back() == Some(&episode) {
            return;
        }
        tracing::info!(
            account = v.session.account,
            character = v.session.character,
            peer = account,
            ?episode,
            "autonomous factual social episode"
        );
        self.episodes.push_back(episode);
        if self.episodes.len() > 32 {
            self.episodes.pop_front();
        }
    }
    pub(in crate::bots) fn known(&self, account: i32) -> bool {
        self.episodes.iter().any(|e| {
            e.account == account
                && matches!(
                    e.interaction,
                    Interaction::HealedBy | Interaction::ReceivedHealth | Interaction::GaveHealth
                )
        })
    }
    fn wants_health(v: &PlayerView) -> bool {
        health(v) < 2 && v.character.items.iter().skip(4).any(|k| *k < 0)
    }
    fn generous(
        &self,
        v: &PlayerView,
        p: &crate::world::view::VisibleObject,
        willingness: f32,
    ) -> bool {
        health(v) > 2
            && v.character.hp * 4 >= v.character.stats[0] * 3
            && (willingness >= 6. || p.account.is_some_and(|a| self.known(a)))
    }
    pub(in crate::bots) fn receive(
        &mut self,
        packet: &Packet,
        v: &PlayerView,
        own_name: &str,
        willingness: f32,
        incoming_trade: bool,
    ) -> Result<bool> {
        if incoming_trade && packet.id == wire::TEXT {
            tracing::info!(
                account = v.session.account,
                reason = "active_incoming_trade",
                "canonical social gift uptake declined"
            );
            return Ok(false);
        }
        if self.heal_tick != v.tick {
            self.heal_notice = 0;
            self.heal_origin = None;
        }
        let mut r = Reader::new(&packet.body);
        if packet.id == wire::NOTIFICATION {
            let target = r.i32()?;
            let text = r.utf()?;
            let color = r.i32()? as u32;
            r.done()?;
            if target == v.session.player
                && color == 0xff00ff00
                && let Some(amount) = text.strip_prefix('+')
            {
                self.heal_notice = amount.parse().context("invalid native healing amount")?;
                self.heal_tick = v.tick;
            }
            return Ok(false);
        }
        if packet.id == wire::SHOW_EFFECT {
            let effect = r.u8()?;
            let target = r.i32()?;
            let a = Pos {
                x: r.f32()?,
                y: r.f32()?,
            };
            r.f32()?;
            r.f32()?;
            r.i32()?;
            r.done()?;
            if effect == 1 && target == v.session.player && self.heal_notice > 0 {
                self.heal_origin = Some((a.x, a.y));
            }
            if effect == 5
                && self.heal_notice > 0
                && let Some(origin) = self.heal_origin.take()
            {
                if let Some(p) = v.objects.iter().find(|p| {
                    p.id == target
                        && p.kind == PRIEST
                        && p.position.distance(Pos {
                            x: origin.0,
                            y: origin.1,
                        }) < 0.5
                        && p.hp > 0
                }) && let Some(account) = p.account
                {
                    let amount = self.heal_notice;
                    self.remember(v, account, p.name.clone(), Interaction::HealedBy, amount);
                    self.outgoing = Some(format!("{}, thanks for the heal", p.name));
                }
                self.heal_notice = 0;
            }
            return Ok(false);
        }
        if packet.id == 12 && self.gift.is_some() {
            let result = r.i32()?;
            r.utf()?;
            r.done()?;
            let g = self.gift.take().unwrap();
            self.outgoing = None;
            tracing::info!(
                account = v.session.account,
                peer = g.account,
                result,
                "gift ended without a verified transaction receipt"
            );
            return Ok(false);
        }
        if packet.id != wire::TEXT {
            return Ok(false);
        }
        let name = r.utf()?;
        let actor = r.i32()?;
        r.i32()?;
        r.u8()?;
        let recipient = r.utf()?;
        let text = r.utf()?;
        r.utf()?;
        r.done()?;
        if actor == v.session.player || !(recipient.is_empty() || recipient == own_name) {
            return Ok(false);
        }
        let Some(p) = v
            .objects
            .iter()
            .find(|p| p.id == actor && p.name == name && p.hp > 0 && !p.equipped.is_empty())
        else {
            return Ok(false);
        };
        let Some(account) = p.account else {
            return Ok(false);
        };
        let Some(body) = text.strip_prefix(&format!("{own_name}, ")) else {
            return Ok(false);
        };
        match body {
            "heal please" => {
                self.heal_request = Some((account, v.elapsed_ms + 10000));
                if v.character.class != PRIEST || v.character.items[1] < 0 {
                    return Ok(true);
                }
            }
            "spare health?" => {
                if self.gift.is_some() {
                    return Ok(true);
                }
                if self.generous(v, p, willingness) {
                    self.gift = Some(Gift {
                        account,
                        player: actor,
                        name: name.clone(),
                        giving: true,
                        consent: false,
                        acceptance: None,
                        cancelled: false,
                        until: v.elapsed_ms + 15000,
                        session: [v.session.world, v.session.player, v.session.character],
                        requested: false,
                    });
                    self.outgoing = Some(format!("{name}, take health"));
                } else {
                    self.outgoing = Some(format!("{name}, keeping my supplies"));
                    self.remember(v, account, name, Interaction::Refused, 0);
                }
            }
            "take health" => {
                if self.gift.is_some() {
                    return Ok(true);
                }
                if Self::wants_health(v) {
                    self.gift = Some(Gift {
                        account,
                        player: actor,
                        name: name.clone(),
                        giving: false,
                        consent: true,
                        acceptance: None,
                        cancelled: false,
                        until: v.elapsed_ms + 15000,
                        session: [v.session.world, v.session.player, v.session.character],
                        requested: false,
                    });
                    self.outgoing = Some(format!("{name}, yes health"));
                } else {
                    self.outgoing = Some(format!("{name}, no thanks"));
                    self.remember(v, account, name, Interaction::Refused, 0);
                }
            }
            "yes health" => {
                if let Some(g) = self
                    .gift
                    .as_mut()
                    .filter(|g| g.giving && g.account == account && g.player == actor)
                {
                    g.consent = true;
                }
            }
            "no thanks" | "keeping my supplies" => {
                if let Some(g) = self
                    .gift
                    .as_mut()
                    .filter(|g| g.account == account && g.player == actor)
                {
                    g.cancelled = true;
                    self.outgoing = None;
                }
                self.next_request = v.elapsed_ms + 60000;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    pub(in crate::bots) fn healing(
        &self,
        v: &PlayerView,
        c: &Content,
        willingness: f32,
    ) -> Result<Option<Action>> {
        let reserve = v.character.items[1] >= 0
            && v.character.mp < c.get(v.character.items[1] as u16)?.mana_cost * 2
            && v.character.hp * 4 >= v.character.stats[0] * 3;
        super::healing_selected(v, c, |p| {
            !reserve
                && (p.account.is_some_and(|a| {
                    self.known(a)
                        || self
                            .heal_request
                            .is_some_and(|(who, until)| a == who && v.elapsed_ms < until)
                }) || willingness >= 6.)
        })
    }
    pub(in crate::bots) fn trying_loadout(&mut self, v: &PlayerView) -> bool {
        if v.character.items[0] < 0 {
            return false;
        }
        let (weapon, since) = *self.private_trial.get_or_insert_with(|| {
            tracing::info!(
                account = v.session.account,
                character = v.session.character,
                weapon = v.character.items[0],
                revision = v.character.inventory_revision,
                "autonomous private loadout trial before next outing"
            );
            (v.character.items[0], v.elapsed_ms)
        });
        weapon == v.character.items[0] && v.elapsed_ms.saturating_sub(since) < 1500
    }
    pub(in crate::bots) fn decide(
        &mut self,
        v: &PlayerView,
        grid: &Grid,
        willingness: f32,
        banking: bool,
    ) -> Option<Decision> {
        if self.gift.is_some() {
            return Some(idle());
        }
        if v.elapsed_ms >= self.next_request
            && let Some(p) = v.objects.iter().find(|p| {
                p.hp > 0
                    && p.account.is_some_and(|a| self.known(a))
                    && v.position.distance(p.position) < 10.
            })
        {
            if v.character.hp * 2 < v.character.stats[0] && p.kind == PRIEST {
                self.outgoing = Some(format!("{}, heal please", p.name));
            } else if Self::wants_health(v) {
                self.outgoing = Some(format!("{}, spare health?", p.name));
            }
            self.next_request = v.elapsed_ms + 60000;
        }
        if let Some(p) = v.objects.iter().find(|p| {
            p.account
                .is_some_and(|a| self.known(a) && !self.recognized.contains(&a))
                && p.hp > 0
        }) {
            let account = p.account.unwrap();
            self.recognized.insert(account);
            self.outgoing
                .get_or_insert_with(|| format!("{}, good to see you again", p.name));
            self.remember(v, account, p.name.clone(), Interaction::Recognized, 0);
        }
        if v.map.name == "Nexus"
            && !banking
            && let Some(p) = v
                .objects
                .iter()
                .filter(|p| p.kind == 0x712)
                .max_by_key(|p| p.usable)
        {
            if !p.usable {
                let (portal, since) = *self.waiting.get_or_insert((p.id, v.elapsed_ms));
                if portal != p.id {
                    self.waiting = Some((p.id, v.elapsed_ms));
                }
                if v.elapsed_ms == since {
                    self.outgoing = Some(
                        if willingness >= 6. {
                            "Realm is full, looking for a Vault"
                        } else {
                            "Realm is full, waiting here"
                        }
                        .into(),
                    );
                    tracing::info!(
                        account = v.session.account,
                        portal = p.id,
                        name = p.name,
                        "autonomous full entrance observed"
                    );
                }
                if willingness >= 6. && v.elapsed_ms.saturating_sub(since) >= 5000 {
                    if let Some(vault) = v.objects.iter().find(|p| p.kind == 0x720 && p.usable) {
                        let near = v.position.distance(vault.position) < 1.4;
                        let mut d = idle();
                        d.goal = Goal::Portal(vault.id);
                        d.movement = if near {
                            None
                        } else {
                            grid.towards(v.position, vault.position)
                        };
                        d.journey = near.then_some(Journey::Portal(vault.id));
                        return Some(d);
                    }
                    let next = grid
                        .reachable(v.position)
                        .iter()
                        .copied()
                        .filter(|t| grid.frontier(*t, v.map.width, v.map.height))
                        .min_by(|a, b| {
                            center(*a)
                                .distance(v.position)
                                .total_cmp(&center(*b).distance(v.position))
                        });
                    if let Some(next) = next {
                        let mut d = idle();
                        d.goal = Goal::Explore(next.0, next.1);
                        d.movement = grid.towards(v.position, center(next));
                        return Some(d);
                    }
                }
                return Some(idle());
            }
            if self.waiting.take().is_some() {
                tracing::info!(
                    account = v.session.account,
                    portal = p.id,
                    name = p.name,
                    "autonomous vacancy observed; reconsidering Realm entry"
                );
            }
        }
        if v.map.name != "Nexus"
            && v.character.level <= 5
            && let Some(p) = v.objects.iter().find(|p| {
                p.hp > 0
                    && p.account.is_some_and(|a| self.known(a))
                    && v.position.distance(p.position) > 4.
            })
        {
            let mut d = idle();
            d.goal = Goal::Follow(p.id);
            d.movement = grid.towards(v.position, p.position);
            return Some(d);
        }
        None
    }
}

fn idle() -> Decision {
    Decision {
        goal: Goal::Wait,
        movement: None,
        aim: None,
        item: None,
        journey: None,
        chat: None,
    }
}

async fn recover_gift_receipt(
    app: &crate::server::App,
    brain: &mut Brain,
    v: &PlayerView,
) -> Result<bool> {
    let session = [v.session.world, v.session.player, v.session.character];
    if let Some(g) = brain.mind.social.gift.clone()
        && let Some(acceptance) = g.acceptance.clone()
    {
        ensure!(
            g.matches(&acceptance, v),
            "gift acceptance belongs to another offer/session"
        );
        let verified = app.store.gift_committed(acceptance.clone()).await?;
        let current = app
            .world
            .directory({
                let acceptance = acceptance.clone();
                move |d| {
                    ensure!(
                        d.location(session[1])? == session[0],
                        "gift acceptance world changed"
                    );
                    d.worlds[&session[0]].gift_acceptance_current(&acceptance)
                }
            })
            .await?;
        if verified {
            let settled = app.world.player_view(session[1]).await?;
            ensure!(
                [
                    settled.session.world,
                    settled.session.player,
                    settled.session.character
                ] == session
                    && settled.session.account == acceptance.own.account,
                "gift receipt completion session changed"
            );
            if !settled.lifecycle.busy
                && !settled.lifecycle.quarantined
                && !settled.lifecycle.departing
                && settled.lifecycle.alive
                && settled.character.inventory_revision > acceptance.own.revision
            {
                ensure!(
                    brain.mind.social.gift.as_ref() == Some(&g),
                    "gift offer changed while verifying its receipt"
                );
                brain.mind.social.gift = None;
                brain.mind.social.outgoing = None;
                brain.mind.social.remember(
                    &settled,
                    g.account,
                    g.name.clone(),
                    if g.giving {
                        Interaction::GaveHealth
                    } else {
                        Interaction::ReceivedHealth
                    },
                    1,
                );
                if !g.giving {
                    brain.mind.social.outgoing = Some(format!("{}, thanks for the health", g.name));
                }
                tracing::info!(account=settled.session.account,peer=g.account,transaction=%acceptance.transaction,"gift exact transaction receipt applied once");
                return Ok(false);
            }
        }
        if !verified && !current {
            tracing::info!(account=v.session.account,peer=g.account,transaction=%acceptance.transaction,"gift native acceptance invalidated before commitment");
            brain.mind.social.gift.as_mut().unwrap().acceptance = None;
        }
        return Ok(verified || current);
    }
    Ok(false)
}

pub(in crate::bots) async fn receive_life(
    app: &crate::server::App,
    brain: &mut Brain,
    packet: &Packet,
    v: &PlayerView,
) -> Result<bool> {
    let session = [v.session.world, v.session.player, v.session.character];
    ensure!(
        session == [brain.session.0, brain.session.1, brain.session.2]
            && v.session.account == brain.mind.account,
        "social packet belongs to a replaced session"
    );
    if matches!(packet.id, 12 | 23 | 67) {
        if packet.id == 12
            && brain
                .mind
                .social
                .gift
                .as_ref()
                .is_some_and(|g| g.acceptance.is_some())
        {
            let mut r = Reader::new(&packet.body);
            r.i32()?;
            r.utf()?;
            r.done()?;
        }
        let protected = recover_gift_receipt(app, brain, v).await?;
        // A legacy result cannot identify the native transaction whose receipt is still pending.
        if packet.id == 12 && protected {
            return Ok(false);
        }
    }
    brain.mind.social.receive(
        packet,
        v,
        &brain.mind.market.name,
        brain.profile.companions,
        brain.mind.market.incoming.is_some(),
    )
}

pub(in crate::bots) async fn cancel_gift(
    app: &crate::server::App,
    brain: &mut Brain,
    v: &PlayerView,
) -> Result<bool> {
    let Some(g) = brain.mind.social.gift.as_mut() else {
        return Ok(false);
    };
    g.cancelled = true;
    // A mutual native accept may already own the commit; keep its receipt binding until completion.
    if !super::revoke_native_market(app, brain).await? {
        return Ok(true);
    }
    if recover_gift_receipt(app, brain, v).await? {
        return Ok(true);
    }
    brain.mind.social.gift = None;
    brain.mind.social.outgoing = None;
    brain.mind.market.trade = None;
    Ok(false)
}

pub(in crate::bots) async fn life(
    app: &crate::server::App,
    brain: &mut Brain,
    v: &PlayerView,
) -> Result<bool> {
    let binding = [v.session.world, v.session.player, v.session.character];
    ensure!(
        binding == [brain.session.0, brain.session.1, brain.session.2]
            && v.session.account == brain.mind.account
            && brain
                .mind
                .social
                .gift
                .as_ref()
                .is_none_or(|g| g.session == binding),
        "gift action belongs to a replaced session"
    );
    if brain.mind.social.gift.as_ref().is_some_and(|g| g.cancelled) {
        return cancel_gift(app, brain, v).await;
    }
    if v.elapsed_ms >= brain.mind.market.next_message_ms
        && let Some(text) = brain.mind.social.outgoing.take()
    {
        let id = v.session.player;
        let received = app.world.clock.now();
        match app
            .world
            .directory(move |d| d.chat(id, text, received))
            .await
        {
            Ok(()) => {
                tracing::info!(
                    account = v.session.account,
                    "canonical gift protocol published without generation"
                );
                brain.mind.market.next_message_ms = v.elapsed_ms + 1500;
            }
            Err(error) => {
                tracing::warn!(account=v.session.account,%error,"social publication rejected; no assent inferred");
                return cancel_gift(app, brain, v).await;
            }
        }
    }
    let Some(mut g) = brain.mind.social.gift.clone() else {
        return Ok(false);
    };
    let peer = v
        .objects
        .iter()
        .find(|p| p.id == g.player && p.account == Some(g.account) && p.name == g.name && p.hp > 0);
    let allowed = v.elapsed_ms < g.until
        && peer.is_some_and(|p| {
            v.position.distance(p.position) < 2.
                && if g.giving {
                    brain.mind.social.generous(v, p, brain.profile.companions)
                } else {
                    Life::wants_health(v)
                }
        });
    if !allowed {
        return cancel_gift(app, brain, v).await;
    }
    use crate::world::TradeAction;
    let action = if let Some(t) = &mut brain.mind.market.trade {
        if t.peer != g.name {
            Some(TradeAction::Cancel)
        } else {
            let donor = usize::from(!g.giving);
            let mut offer = [false; 12];
            if let Some(slot) = t.items[donor]
                .iter()
                .enumerate()
                .skip(4)
                .find(|(_, k)| **k == HEALTH)
                .map(|(i, _)| i)
            {
                offer[slot] = true;
            }
            if !offer.iter().any(|b| *b) {
                Some(TradeAction::Cancel)
            } else {
                let (mine, theirs) = if g.giving {
                    (offer, [false; 12])
                } else {
                    ([false; 12], offer)
                };
                if !t.changed || t.offers[0] != mine {
                    t.changed = true;
                    t.offers[0] = mine;
                    t.ready = v.time_ms + 3000;
                    Some(TradeAction::Change { offer: mine })
                } else if t.offers[1] == theirs
                    && v.time_ms >= t.ready
                    && v.clock_at >= t.ready_at
                    && g.acceptance.is_none()
                {
                    Some(TradeAction::Accept { mine, theirs })
                } else {
                    None
                }
            }
        }
    } else if !g.requested && g.consent && brain.mind.social.outgoing.is_none() {
        g.requested = true;
        Some(TradeAction::Request {
            name: g.name.clone(),
        })
    } else {
        None
    };
    brain.mind.social.gift = Some(g.clone());
    if let Some(action) = action {
        let cancel = matches!(action, TradeAction::Cancel);
        let changed = matches!(action, TradeAction::Change { .. });
        let id = v.session.player;
        let session = [v.session.world, id, v.session.character];
        let result = app
            .world
            .directory(move |d| {
                ensure!(d.location(id)? == session[0], "gift action world changed");
                d.worlds
                    .get_mut(&session[0])
                    .context("gift world missing")?
                    .autonomous_gift(id, session, g.player, (g.account, g.name), g.giving, action)
            })
            .await;
        match result {
            Ok(outcome) => {
                if let Some(acceptance) = outcome.acceptance {
                    let gift = brain
                        .mind
                        .social
                        .gift
                        .as_mut()
                        .context("gift disappeared during native acceptance")?;
                    ensure!(
                        gift.matches(&acceptance, v),
                        "native gift acceptance counterparty/session changed"
                    );
                    tracing::info!(account=v.session.account,peer=gift.account,transaction=%acceptance.transaction,"gift native acceptance witness bound; not a payment receipt");
                    gift.acceptance = Some(acceptance);
                }
                if let Some(plan) = outcome.plan {
                    crate::server::persist_trade(app, plan).await?;
                } else if changed {
                    let acknowledged = app.world.player_view(id).await?;
                    let t = brain
                        .mind
                        .market
                        .trade
                        .as_mut()
                        .context("gift trade disappeared after offer acknowledgement")?;
                    t.ready = acknowledged.time_ms + 3000;
                    t.ready_at = acknowledged.clock_at + std::time::Duration::from_millis(3000);
                }
            }
            Err(error) => {
                tracing::warn!(account=v.session.account,%error,"guarded gift rejected");
                return cancel_gift(app, brain, v).await;
            }
        }
        if cancel {
            return cancel_gift(app, brain, v).await;
        }
    }
    Ok(brain.mind.social.gift.is_some())
}
