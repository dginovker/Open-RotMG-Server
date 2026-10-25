use super::*;

use crate::bots::pursuits::market::Delivery;
use crate::store::{GiftAcceptance, GiftParticipant, InventoryWrite};

pub(super) struct Trade {
    transaction: String,
    deliveries: [Option<Delivery>; 2],
    players: [i32; 2],
    items: [Vec<i32>; 2],
    offers: [[bool; 12]; 2],
    accepted: [bool; 2],
    // The client's countdown uses getTimer(), not simulation ticks: https://github.com/moistosaurus/realm-cli/blob/a13c855dc5e09ee301fd0a66fcd303cfb5dff74e/src/com/company/assembleegameclient/ui/TradeButton.as
    ready: Instant,
    pending: bool,
}
pub struct TradePlan {
    pub transaction: String,
    pub(crate) delivery: Option<Delivery>,
    key: i32,
    pub plans: [InventoryPlan; 2],
}
#[derive(Default)]
pub(crate) struct GiftOutcome {
    pub plan: Option<TradePlan>,
    pub acceptance: Option<GiftAcceptance>,
}

fn exchanged_items(items: &[Vec<i32>; 2], offers: &[[bool; 12]; 2]) -> Result<[Vec<i32>; 2]> {
    let mut inventories = items.clone();
    for side in 0..2 {
        for slot in 4..12 {
            if offers[side][slot] {
                inventories[side][slot] = -1;
            }
        }
    }
    for side in 0..2 {
        for slot in 4..12 {
            if offers[1 - side][slot] {
                let destination = inventories[side][4..]
                    .iter()
                    .position(|item| *item == -1)
                    .ok_or_else(|| controller("trade recipient inventory is full"))?
                    + 4;
                inventories[side][destination] = items[1 - side][slot];
            }
        }
    }
    Ok(inventories)
}
#[derive(Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum TradeAction {
    Request {
        name: String,
    },
    Change {
        offer: [bool; 12],
    },
    Accept {
        mine: [bool; 12],
        theirs: [bool; 12],
    },
    Cancel,
}
fn offer(out: &mut Writer, values: &[bool; 12]) {
    out.i16(12);
    for value in values {
        out.u8(u8::from(*value));
    }
}
impl World {
    pub(crate) fn revoke_autonomous_trade(&mut self, id: i32, session: [i32; 3]) -> Result<bool> {
        let p = self.players.get(&id).context("autonomous trader missing")?;
        ensure!(
            p.controller == Controller::Autonomous && session[1..] == [id, p.character.id],
            "autonomous revocation controller/session changed"
        );
        if p.pending {
            return Ok(false);
        }
        self.cancel_trade(id, "Negotiated consent revoked; fresh agreement required.");
        tracing::info!(
            player = id,
            ?session,
            "market native acceptance revoked before policy update"
        );
        Ok(true)
    }
    pub(crate) fn autonomous_trade(
        &mut self,
        id: i32,
        session: [i32; 3],
        action: TradeAction,
        delivery: Delivery,
    ) -> Result<Option<TradePlan>> {
        let p = self.players.get(&id).context("autonomous trader missing")?;
        ensure!(
            p.controller == Controller::Autonomous && session[1..] == [id, p.character.id],
            "autonomous trade controller/session changed"
        );
        let side = delivery
            .terms
            .accounts
            .iter()
            .position(|a| *a == p.character.account)
            .context("market consent belongs to another account")?;
        ensure!(
            delivery == delivery.terms.delivery(delivery.ordinal)?
                && delivery.terms.expires > crate::bots::pursuits::market::now(),
            "stale market installment"
        );
        let meeting = Pos {
            x: delivery.terms.meeting[1] as f32 + 0.5,
            y: delivery.terms.meeting[2] as f32 + 0.5,
        };
        ensure!(
            session[0] == delivery.terms.meeting[0] && p.pos.distance(meeting) < 2.,
            "autonomous trade is outside agreed rendezvous"
        );
        let peer = self
            .players
            .iter()
            .find(|(_, q)| {
                q.character.account == delivery.terms.accounts[1 - side]
                    && q.character.name == delivery.terms.names[1 - side]
            })
            .context("market counterparty unavailable")?;
        ensure!(p.known.contains(peer.0), "market counterparty not observed");
        match action {
            TradeAction::Request { name } => {
                ensure!(
                    name == delivery.terms.names[1 - side],
                    "market request names another account"
                );
                self.request_trade(id, name)?;
            }
            TradeAction::Change { offer } => self.change_trade(id, offer)?,
            TradeAction::Accept { mine, theirs } => {
                let key = self.trade_key(id).context("market trade missing")?;
                let index = self.trades[&key]
                    .players
                    .iter()
                    .position(|p| *p == id)
                    .unwrap();
                self.trades.get_mut(&key).unwrap().deliveries[index] = Some(delivery);
                return self.accept_trade(id, mine, theirs);
            }
            TradeAction::Cancel => {
                self.cancel_trade(id, "Market bargain ended.");
            }
        }
        Ok(None)
    }

    pub(crate) fn gift_acceptance_current(&self, acceptance: &GiftAcceptance) -> Result<bool> {
        let id = acceptance.session[1];
        let p = self
            .players
            .get(&id)
            .context("gift acceptance actor missing")?;
        ensure!(
            p.controller == Controller::Autonomous
                && p.character.account == acceptance.own.account
                && p.character.id == acceptance.own.character
                && acceptance.session[2] == p.character.id,
            "gift acceptance controller/session changed"
        );
        let Some(key) = self.trade_key(id) else {
            return Ok(false);
        };
        let t = &self.trades[&key];
        let side = usize::from(t.players[1] == id);
        Ok(t.transaction == acceptance.transaction
            && t.accepted[side]
            && t.players[1 - side] == acceptance.peer_player
            && p.character.revision == acceptance.own.revision
            && self.players.get(&acceptance.peer_player).is_some_and(|q| {
                q.character.account == acceptance.peer.account
                    && q.character.id == acceptance.peer.character
                    && q.character.revision == acceptance.peer.revision
            }))
    }
    pub(crate) fn autonomous_gift(
        &mut self,
        id: i32,
        session: [i32; 3],
        peer: i32,
        peer_binding: (i32, String),
        giving: bool,
        action: TradeAction,
    ) -> Result<GiftOutcome> {
        let p = self.players.get(&id).context("gift actor missing")?;
        ensure!(
            p.controller == Controller::Autonomous && session[1..] == [id, p.character.id],
            "gift session/controller changed"
        );
        let q = self
            .players
            .get(&peer)
            .context("gift recipient unavailable")?;
        ensure!(
            p.known.contains(&peer) && p.pos.distance(q.pos) < 2.,
            "gift contact out of reach"
        );
        ensure!(
            q.character.account == peer_binding.0 && q.character.name == peer_binding.1,
            "gift observed counterparty binding changed"
        );
        let peer_name = q.character.name.clone();
        let peer_id = peer;
        match action {
            TradeAction::Request { name } => {
                ensure!(name == peer_name, "gift request names another contact");
                self.request_trade(id, name)?;
            }
            TradeAction::Change { offer } => self.change_trade(id, offer)?,
            TradeAction::Accept { mine, theirs } => {
                let key = self.trade_key(id).context("gift trade missing")?;
                let t = &self.trades[&key];
                ensure!(
                    t.players.contains(&peer) && t.deliveries == [None, None],
                    "gift trade belongs to another transaction"
                );
                let side = usize::from(t.players[1] == id);
                let masks = if giving {
                    [mine, theirs]
                } else {
                    [theirs, mine]
                };
                let donor = if giving { side } else { 1 - side };
                ensure!(
                    masks[1].iter().all(|b| !b) && masks[0].iter().filter(|b| **b).count() == 1,
                    "gift terms changed"
                );
                ensure!(
                    masks[0]
                        .iter()
                        .enumerate()
                        .all(|(i, b)| !*b || (i >= 4 && t.items[donor][i] == 0xa22)),
                    "gift is not one held health potion"
                );
                let own = GiftParticipant {
                    account: p.character.account,
                    character: p.character.id,
                    revision: p.character.revision,
                };
                let peer = GiftParticipant {
                    account: q.character.account,
                    character: q.character.id,
                    revision: q.character.revision,
                };
                let inventories = exchanged_items(&t.items, &t.offers)?;
                let writes = t
                    .players
                    .iter()
                    .enumerate()
                    .map(|(i, player)| {
                        let ch = &self.players[player].character;
                        InventoryWrite {
                            id: ch.id,
                            account: ch.account,
                            revision: ch.revision,
                            items: inventories[i].clone(),
                        }
                    })
                    .collect::<Vec<_>>();
                let writes_hash =
                    GiftAcceptance::payload_hash(giving, &serde_json::to_string(&writes)?);
                let transaction = t.transaction.clone();
                let plan = self.accept_trade(id, mine, theirs)?;
                let acceptance = GiftAcceptance {
                    transaction,
                    session,
                    own,
                    peer,
                    peer_player: peer_id,
                    peer_name,
                    giving,
                    writes_hash,
                };
                return Ok(GiftOutcome {
                    plan,
                    acceptance: Some(acceptance),
                });
            }
            TradeAction::Cancel => {
                self.cancel_trade(id, "Gift declined or contact changed.");
            }
        }
        Ok(GiftOutcome::default())
    }
    pub(crate) fn autonomous_incoming_trade(
        &mut self,
        id: i32,
        session: [i32; 3],
        peer: i32,
        binding: (i32, String),
        action: TradeAction,
    ) -> Result<Option<TradePlan>> {
        let p = self.players.get(&id).context("incoming trader missing")?;
        ensure!(
            p.controller == Controller::Autonomous && session[1..] == [id, p.character.id],
            "incoming trade controller/session changed"
        );
        if matches!(action, TradeAction::Cancel) {
            self.cancel_trade(id, "Incoming gift declined or contact changed.");
            return Ok(None);
        }
        self.trade_ready(id)?;
        self.trade_ready(peer)?;
        let q = &self.players[&peer];
        ensure!(
            self.name == "Nexus"
                && p.known.contains(&peer)
                && p.pos.distance(q.pos) < 2.
                && q.character.account == binding.0
                && q.character.name == binding.1,
            "incoming trade observed contact/world changed"
        );
        match action {
            TradeAction::Request { name } => {
                ensure!(
                    name == binding.1
                        && p.character.items.iter().skip(4).any(|k| *k < 0)
                        && self
                            .trade_requests
                            .get(&peer)
                            .is_some_and(|(to, until)| *to == id && *until > self.now),
                    "incoming request expired or capacity changed"
                );
                self.request_trade(id, name)?;
            }
            TradeAction::Accept { mine, theirs } => {
                let key = self.trade_key(id).context("incoming trade missing")?;
                let t = &self.trades[&key];
                ensure!(
                    t.players.contains(&peer)
                        && t.deliveries == [None, None]
                        && mine.iter().all(|b| !b)
                        && theirs.iter().any(|b| *b),
                    "incoming gift must be free and nonempty, without negotiated deliveries"
                );
                return self.accept_trade(id, mine, theirs);
            }
            _ => bail!("incoming request cannot authorize bot goods"),
        }
        Ok(None)
    }
    pub fn script_trade(&mut self, id: i32, action: TradeAction) -> Result<Option<TradePlan>> {
        ensure!(
            self.players
                .get(&id)
                .context("script trader missing")?
                .controller
                == Controller::Qa,
            "trade control requires a script-owned player"
        );
        if let Some(key) = self.trade_key(id) {
            ensure!(
                self.trades[&key]
                    .players
                    .iter()
                    .all(|id| self.players[id].controller == Controller::Qa),
                "trade control cannot change a browser player's trade"
            );
        }
        match action {
            TradeAction::Request { name } => {
                ensure!(
                    self.players
                        .values()
                        .any(|p| p.character.name == name && p.controller == Controller::Qa),
                    "script trade target must be script-owned"
                );
                self.request_trade(id, name)?;
            }
            TradeAction::Change { offer } => self.change_trade(id, offer)?,
            TradeAction::Accept { mine, theirs } => return self.accept_trade(id, mine, theirs),
            TradeAction::Cancel => {
                self.cancel_trade(id, "Trade cancelled.");
            }
        }
        Ok(None)
    }
    pub(super) fn trade_key(&self, id: i32) -> Option<i32> {
        self.trades
            .iter()
            .find(|(_, trade)| trade.players.contains(&id))
            .map(|(key, _)| *key)
    }
    fn trade_ready(&self, id: i32) -> Result<()> {
        let p = self.players.get(&id).context("trade player missing")?;
        ensure!(
            p.persistence_failure.is_none(),
            "trade player busy, dead, departing or quarantined"
        );
        ensure!(
            p.active() && !p.pending,
            controller("trade player busy, dead, departing or quarantined")
        );
        Ok(())
    }
    fn tradeable(&self, id: i32, slot: usize) -> Result<bool> {
        let item = self.players[&id].character.items[slot];
        Ok(slot >= 4 && item >= 0 && !self.content.get(u16::try_from(item)?)?.soulbound)
    }
    fn trade_items(&self, out: &mut Writer, id: i32) -> Result<()> {
        let p = &self.players[&id];
        let slots = &self.content.get(p.character.class)?.slots;
        ensure!(
            slots.len() == 12 && p.character.items.len() == 12,
            "invalid trade inventory shape"
        );
        out.i16(12);
        for (slot, item) in p.character.items.iter().enumerate() {
            out.i32(*item)
                .i32(slots[slot])
                .u8(u8::from(self.tradeable(id, slot)?))
                .u8(0);
        }
        Ok(())
    }
    pub fn request_trade(&mut self, id: i32, name: String) -> Result<()> {
        ensure!(
            !name.is_empty() && name.len() <= 16,
            controller("invalid trade target name")
        );
        self.trade_ready(id)?;
        ensure!(self.trade_key(id).is_none(), controller("already trading"));
        // Trades resolve names in this world, not melee range: https://github.com/trapped/rotmg_svr/blob/593e490803fe0ad168ead408cacd5287d778acfe/wServer/realm/entities/player/Player.Trade.cs#L42-L75
        let targets: Vec<_> = self
            .players
            .iter()
            .filter(|(_, p)| p.character.name == name)
            .map(|(id, _)| *id)
            .collect();
        ensure!(
            targets.len() == 1 && targets[0] != id,
            controller("trade target missing, ambiguous or self")
        );
        let target = targets[0];
        self.trade_ready(target)?;
        ensure!(
            self.trade_key(target).is_none(),
            controller("target already trading")
        );
        if self
            .trade_requests
            .get(&target)
            .is_some_and(|(to, expires)| *to == id && *expires > self.now)
        {
            let players = [id, target];
            let mut starts = Vec::new();
            for (mine, theirs) in [(id, target), (target, id)] {
                let mut out = Writer::new();
                self.trade_items(&mut out, mine)?;
                out.utf(&self.players[&theirs].character.name);
                self.trade_items(&mut out, theirs)?;
                starts.push((mine, out.0));
            }
            self.trade_requests
                .retain(|from, (to, _)| !players.contains(from) && !players.contains(to));
            self.trades.insert(
                id,
                Trade {
                    transaction: format!("{:032x}", rand::random::<u128>()),
                    deliveries: [None, None],
                    players,
                    items: players.map(|id| self.players[&id].character.items.clone()),
                    offers: [[false; 12]; 2],
                    accepted: [false; 2],
                    ready: self.clock.now() + Duration::from_millis(3000),
                    pending: false,
                },
            );
            for (mine, body) in starts {
                send(&self.players[&mine], 67, body);
            }
            tracing::info!(player = id, partner = target, "native trade started");
        } else {
            self.trade_requests.insert(id, (target, self.now + 20_000));
            let mut out = Writer::new();
            out.utf(&self.players[&id].character.name);
            send(&self.players[&target], 61, out.0);
            tracing::info!(
                player = id,
                partner = target,
                "reciprocal trade request sent"
            );
        }
        Ok(())
    }
    pub fn cancel_trade(&mut self, id: i32, reason: &str) -> bool {
        self.trade_requests
            .retain(|from, (to, _)| *from != id && *to != id);
        let Some(key) = self.trade_key(id) else {
            return false;
        };
        if self.trades[&key].pending {
            tracing::info!(player=id,%reason,"trade cancellation deferred until atomic commit finishes");
            return false;
        }
        let trade = self.trades.remove(&key).unwrap();
        for player in trade.players {
            let mut out = Writer::new();
            // Internal reasons stay in logs; players see the original server's text.
            out.i32(1).utf("Trade canceled!");
            send(&self.players[&player], 12, out.0);
        }
        tracing::info!(players=?trade.players,%reason,"trade cancelled without inventory mutation");
        true
    }
    pub fn change_trade(&mut self, id: i32, values: [bool; 12]) -> Result<()> {
        let key = self
            .trade_key(id)
            .ok_or_else(|| controller("no active trade"))?;
        ensure!(!self.trades[&key].pending, "trade commit pending");
        self.trade_ready(id)?;
        let side = self.trades[&key]
            .players
            .iter()
            .position(|p| *p == id)
            .unwrap();
        for (slot, selected) in values.iter().enumerate() {
            ensure!(
                !selected || self.tradeable(id, slot)?,
                controller("empty, equipped or Soulbound trade offer")
            );
        }
        let trade = self.trades.get_mut(&key).unwrap();
        ensure!(
            self.players[&id].character.items == trade.items[side],
            "trade inventory changed"
        );
        trade.offers[side] = values;
        trade.accepted = [false; 2];
        trade.deliveries = [None, None];
        trade.ready = self.clock.now() + Duration::from_millis(3000);
        let other = trade.players[1 - side];
        let mut out = Writer::new();
        offer(&mut out, &values);
        send(&self.players[&other], 23, out.0);
        Ok(())
    }
    pub fn accept_trade(
        &mut self,
        id: i32,
        mine: [bool; 12],
        theirs: [bool; 12],
    ) -> Result<Option<TradePlan>> {
        let key = self
            .trade_key(id)
            .ok_or_else(|| controller("no active trade"))?;
        let trade = &self.trades[&key];
        ensure!(!trade.pending, "trade commit pending or offer settling");
        ensure!(
            self.clock.now() >= trade.ready,
            controller("trade commit pending or offer settling")
        );
        let side = trade.players.iter().position(|p| *p == id).unwrap();
        ensure!(
            mine == trade.offers[side] && theirs == trade.offers[1 - side],
            controller("stale trade acceptance")
        );
        for (index, player) in trade.players.iter().enumerate() {
            self.trade_ready(*player)?;
            ensure!(
                self.players[player].character.items == trade.items[index],
                "trade inventory changed"
            );
        }
        let trade = self.trades.get_mut(&key).unwrap();
        trade.accepted[side] = true;
        let other = trade.players[1 - side];
        let mut out = Writer::new();
        offer(&mut out, &trade.offers[1 - side]);
        offer(&mut out, &trade.offers[side]);
        send(&self.players[&other], 18, out.0);
        if !trade.accepted.iter().all(|accepted| *accepted) {
            return Ok(None);
        }
        let delivery = trade.deliveries.iter().flatten().next().cloned();
        if let Some(delivery) = &delivery {
            ensure!(
                delivery.terms.expires > crate::bots::pursuits::market::now(),
                "market installment expired before mutual native acceptance"
            );
            for index in 0..2 {
                let p = &self.players[&trade.players[index]];
                ensure!(
                    p.controller != Controller::Autonomous
                        || trade.deliveries[index].as_ref() == Some(delivery),
                    "autonomous installment consents differ"
                );
                let side = delivery
                    .terms
                    .accounts
                    .iter()
                    .position(|a| *a == p.character.account)
                    .context("installment counterparty changed")?;
                let offered = trade.items[index]
                    .iter()
                    .zip(trade.offers[index])
                    .filter(|(_, selected)| *selected)
                    .map(|(k, _)| *k)
                    .collect::<Vec<_>>();
                ensure!(
                    offered.len() == delivery.quantities[side] as usize
                        && offered
                            .iter()
                            .all(|k| *k == delivery.terms.goods[side].kind),
                    "native offered goods differ from negotiated installment"
                );
            }
        }
        let transaction = trade.transaction.clone();
        let players = trade.players;
        let inventories = exchanged_items(&trade.items, &trade.offers)?;
        let plans = [
            self.plan(players[0], inventories[0].clone(), None),
            self.plan(players[1], inventories[1].clone(), None),
        ];
        for player in players {
            self.players.get_mut(&player).unwrap().pending = true;
        }
        self.trades.get_mut(&key).unwrap().pending = true;
        Ok(Some(TradePlan {
            key,
            plans,
            transaction,
            delivery,
        }))
    }
    pub fn finish_trade(&mut self, plan: TradePlan, result: Result<Vec<i32>>) -> Result<()> {
        let result = result.and_then(|revisions| {
            ensure!(
                revisions.len() == 2
                    && self
                        .trades
                        .get(&plan.key)
                        .context("reserved trade missing")?
                        .pending,
                "trade completion shape or state mismatch"
            );
            for (index, write) in plan.plans.iter().enumerate() {
                let p = self
                    .players
                    .get(&write.player)
                    .context("reserved trade participant missing")?;
                ensure!(
                    p.pending
                        && p.character.revision == write.revision
                        && p.character.id == write.character
                        && p.character.account == write.account
                        && write.revision.checked_add(1) == Some(revisions[index]),
                    "reserved trade owner or revision changed"
                );
            }
            Ok(revisions)
        });
        let revisions = match result {
            Ok(revisions) => revisions,
            Err(error) => {
                for write in &plan.plans {
                    if let Some(p) = self.players.get_mut(&write.player) {
                        p.quarantine(write.player, format!("{error:#}"));
                    }
                }
                return Err(error);
            }
        };
        for (index, write) in plan.plans.into_iter().enumerate() {
            let p = self.players.get_mut(&write.player).unwrap();
            p.character.items = write.items;
            p.character.revision = revisions[index];
            p.pending = false;
        }
        let trade = self.trades.remove(&plan.key).unwrap();
        for player in trade.players {
            let mut out = Writer::new();
            out.i32(0).utf("Trade successful.");
            send(&self.players[&player], 12, out.0);
        }
        tracing::info!(players=?trade.players,revisions=?revisions,"two-owner inventory exchange committed");
        Ok(())
    }
    pub(super) fn trade_step(&mut self) {
        self.trade_requests
            .retain(|_, (_, expires)| *expires > self.now);
        let interrupted: Vec<_> = self
            .trades
            .values()
            .filter(|t| !t.pending && t.players.iter().any(|id| !self.players[id].active()))
            .map(|t| t.players[0])
            .collect();
        for id in interrupted {
            self.cancel_trade(id, "Trade interrupted by departure or death.");
        }
    }
}
#[cfg(test)]
mod community_tests {
    use super::*;
    use crate::store::{InventoryWrite, Store};

    impl World {}
    async fn fixture() -> (World, Store, [i32; 2], [mpsc::Receiver<Packet>; 2]) {
        let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
        let store = Store::open(std::path::Path::new(":memory:"), &content).unwrap();
        let map =
            Arc::new(Map::load(std::path::Path::new("data/tutorial.wmap"), &content).unwrap());
        let mut w = World::new(content, map).unwrap();
        let mut players = Vec::new();
        let mut receivers = Vec::new();
        for guid in ["trade-a", "trade-b"] {
            let account = store.list(guid.into()).await.unwrap();
            let character = store
                .create(account.id, w.content.get(crate::content::WIZARD).unwrap())
                .await
                .unwrap();
            let (packets, receiver) = mpsc::channel(64);
            let (lost, _) = watch::channel(None);
            players.push(w.enter(character, Some(Output { packets, lost })).unwrap());
            receivers.push(receiver);
        }
        let ids = [players[0], players[1]];
        let plan = w
            .swap(
                ids[1],
                Slot {
                    object: ids[1],
                    slot: 0,
                    kind: 0xa97,
                },
                Slot {
                    object: ids[1],
                    slot: 5,
                    kind: -1,
                },
            )
            .unwrap();
        let revision = store
            .exchange(
                vec![InventoryWrite {
                    id: plan.character,
                    account: plan.account,
                    revision: plan.revision,
                    items: plan.items.clone(),
                }],
                vec![],
            )
            .await
            .unwrap()[0];
        w.finish_inventory(plan, Ok(revision)).unwrap();
        let mut receivers: [mpsc::Receiver<Packet>; 2] = receivers.try_into().ok().unwrap();
        for r in &mut receivers {
            while r.try_recv().is_ok() {}
        }
        (w, store, ids, receivers)
    }
    fn start(w: &mut World, players: [i32; 2]) {
        w.request_trade(players[0], w.players[&players[1]].character.name.clone())
            .unwrap();
        w.request_trade(players[1], w.players[&players[0]].character.name.clone())
            .unwrap();
    }
    async fn offers(w: &mut World, players: [i32; 2]) -> [[bool; 12]; 2] {
        let mut offers = [[false; 12]; 2];
        offers[0][4] = true;
        offers[1][5] = true;
        w.change_trade(players[0], offers[0]).unwrap();
        w.change_trade(players[1], offers[1]).unwrap();
        tokio::time::sleep(Duration::from_millis(3050)).await;
        offers
    }
    fn writes(plan: &TradePlan) -> Vec<InventoryWrite> {
        plan.plans
            .iter()
            .map(|p| InventoryWrite {
                id: p.character,
                account: p.account,
                revision: p.revision,
                items: p.items.clone(),
            })
            .collect()
    }
    #[tokio::test]
    async fn nearby_autonomous_trade_request_reaches_controller_without_assuming_consent() {
        let (mut w, _, ids, mut receivers) = fixture().await;
        w.players.get_mut(&ids[1]).unwrap().pos.x += 3.;
        w.players.get_mut(&ids[1]).unwrap().controller = Controller::Autonomous;
        w.request_trade(ids[0], w.players[&ids[1]].character.name.clone())
            .unwrap();
        assert_eq!(w.trade_requests[&ids[0]].0, ids[1]);
        assert_eq!(receivers[1].try_recv().unwrap().id, 61);
        assert!(w.trades.is_empty(), "a request is not reciprocal consent");
    }
    #[tokio::test]
    async fn incoming_free_gift_opens_only_real_requests_and_commits_without_bot_goods() {
        let (mut w, store, ids, _) = fixture().await;
        w.name = "Nexus".into();
        w.players.get_mut(&ids[1]).unwrap().controller = Controller::Autonomous;
        w.updates(ids[1], true).unwrap();
        let p = &w.players[&ids[1]];
        let session = [-2, ids[1], p.character.id];
        let bot_before = p.character.items.clone();
        let binding = (
            w.players[&ids[0]].character.account,
            w.players[&ids[0]].character.name.clone(),
        );
        let request = || TradeAction::Request {
            name: binding.1.clone(),
        };
        assert!(
            w.autonomous_incoming_trade(ids[1], session, ids[0], binding.clone(), request())
                .is_err()
        );
        w.request_trade(ids[0], w.players[&ids[1]].character.name.clone())
            .unwrap();
        w.autonomous_incoming_trade(ids[1], session, ids[0], binding.clone(), request())
            .unwrap();
        let mut gift = [false; 12];
        gift[4] = true;
        w.change_trade(ids[0], gift).unwrap();
        assert!(
            w.autonomous_incoming_trade(
                ids[1],
                session,
                ids[0],
                binding.clone(),
                TradeAction::Change { offer: gift }
            )
            .is_err()
        );
        assert!(
            w.autonomous_incoming_trade(
                ids[1],
                session,
                ids[0],
                binding.clone(),
                TradeAction::Accept {
                    mine: gift,
                    theirs: gift
                }
            )
            .is_err()
        );
        assert!(
            w.autonomous_incoming_trade(
                ids[1],
                session,
                ids[0],
                binding.clone(),
                TradeAction::Accept {
                    mine: [false; 12],
                    theirs: gift
                }
            )
            .is_err()
        );
        tokio::time::sleep(Duration::from_millis(3050)).await;
        w.autonomous_incoming_trade(
            ids[1],
            session,
            ids[0],
            binding,
            TradeAction::Accept {
                mine: [false; 12],
                theirs: gift,
            },
        )
        .unwrap();
        let plan = w.accept_trade(ids[0], gift, [false; 12]).unwrap().unwrap();
        let transaction = plan.transaction.clone();
        let revisions = store
            .exchange_trade(transaction, writes(&plan), None)
            .await
            .unwrap();
        w.finish_trade(plan, Ok(revisions)).unwrap();
        assert_eq!(w.players[&ids[1]].character.items[5], bot_before[5]);
        for id in ids {
            let p = &w.players[&id];
            let saved = store
                .load(p.character.account, p.character.id)
                .await
                .unwrap();
            assert_eq!(saved.items, p.character.items);
            assert_eq!(saved.revision, p.character.revision);
        }
    }
    #[tokio::test]
    async fn trade_settling_uses_pause_aware_elapsed_time_not_world_ticks() {
        let (mut w, _, ids, _) = fixture().await;
        w.players.get_mut(&ids[1]).unwrap().pos.x += 3.;
        start(&mut w, ids);
        w.players.get_mut(&ids[1]).unwrap().pos.x += 8.;
        w.trade_step();
        assert_eq!(w.trades.len(), 1);
        let mut masks = [[false; 12]; 2];
        masks[0][4] = true;
        masks[1][5] = true;
        w.change_trade(ids[0], masks[0]).unwrap();
        w.change_trade(ids[1], masks[1]).unwrap();
        let tick_time = w.now;
        w.clock.set_paused(true);
        tokio::time::sleep(Duration::from_millis(3050)).await;
        assert!(w.accept_trade(ids[0], masks[0], masks[1]).is_err());
        w.clock.set_paused(false);
        tokio::time::sleep(Duration::from_millis(3050)).await;
        assert_eq!(
            w.now, tick_time,
            "no simulation ticks in this slow-world fixture"
        );
        assert!(
            w.accept_trade(ids[0], masks[0], masks[1])
                .expect("client countdown elapsed while world ticks lagged")
                .is_none()
        );
        w.players.get_mut(&ids[1]).unwrap().departing = true;
        w.trade_step();
        assert!(w.trades.is_empty());
    }

    #[tokio::test]
    async fn trade_offer_reset_keeps_elapsed_settling_and_atomic_commit_guards() {
        let (mut w, store, ids, _) = fixture().await;
        w.players.get_mut(&ids[1]).unwrap().pos.x += 3.;
        start(&mut w, ids);
        let masks = offers(&mut w, ids).await;
        assert!(
            w.accept_trade(ids[0], masks[0], masks[1])
                .unwrap()
                .is_none()
        );
        w.clock.set_paused(true);
        w.change_trade(ids[1], [false; 12]).unwrap();
        let trade = &w.trades[&w.trade_key(ids[0]).unwrap()];
        assert_eq!(trade.accepted, [false; 2]);
        assert_eq!(trade.ready - w.clock.now(), Duration::from_millis(3000));
        w.now += 10_000;
        tokio::time::sleep(Duration::from_millis(3050)).await;
        assert!(w.accept_trade(ids[0], masks[0], [false; 12]).is_err());
        w.clock.set_paused(false);
        tokio::time::sleep(Duration::from_millis(3050)).await;
        assert!(
            w.accept_trade(ids[0], masks[0], masks[1])
                .err()
                .unwrap()
                .to_string()
                .contains("stale trade acceptance")
        );
        assert!(
            w.accept_trade(ids[0], masks[0], [false; 12])
                .unwrap()
                .is_none()
        );
        let before = ids.map(|id| w.players[&id].character.items.clone());
        let plan = w
            .accept_trade(ids[1], [false; 12], masks[0])
            .unwrap()
            .unwrap();
        assert!(w.accept_trade(ids[1], [false; 12], masks[0]).is_err());
        assert!(w.change_trade(ids[0], masks[0]).is_err());
        assert!(!w.cancel_trade(ids[0], "pending commit"));
        assert_eq!(before, ids.map(|id| w.players[&id].character.items.clone()));
        let expected = plan.plans.each_ref().map(|p| {
            (
                p.player,
                p.account,
                p.character,
                p.revision + 1,
                p.items.clone(),
            )
        });
        let revisions = store
            .exchange_trade(plan.transaction.clone(), writes(&plan), None)
            .await
            .unwrap();
        w.finish_trade(plan, Ok(revisions)).unwrap();
        for (id, account, character, revision, items) in expected {
            let saved = store.load(account, character).await.unwrap();
            assert_eq!((saved.revision, saved.items), (revision, items.clone()));
            assert_eq!(w.players[&id].character.items, items);
        }
        assert!(w.accept_trade(ids[0], masks[0], [false; 12]).is_err());
    }

    #[tokio::test]
    async fn community_market_expiry_is_revalidated_after_autonomous_native_acceptance_before_commit()
     {
        use crate::bots::pursuits::market::{Goods, Terms, now};
        let (mut w, store, ids, _) = fixture().await;
        w.players.get_mut(&ids[0]).unwrap().controller = Controller::Autonomous;
        w.updates(ids[0], true).unwrap();
        start(&mut w, ids);
        let masks = offers(&mut w, ids).await;
        let p = &w.players[&ids[0]];
        let t = Terms {
            version: 52,
            accounts: ids.map(|id| w.players[&id].character.account),
            names: ids.map(|id| w.players[&id].character.name.clone()),
            goods: [
                Goods {
                    kind: 0xa22,
                    quantity: 1,
                },
                Goods {
                    kind: 0xa97,
                    quantity: 1,
                },
            ],
            meeting: [-2, p.pos.x.floor() as i32, p.pos.y.floor() as i32],
            expires: now() + 2,
            batch: 1,
        };
        let session = [-2, ids[0], p.character.id];
        let d = t.delivery(0).unwrap();
        assert!(
            w.autonomous_trade(
                ids[0],
                session,
                TradeAction::Accept {
                    mine: masks[0],
                    theirs: masks[1]
                },
                d
            )
            .unwrap()
            .is_none()
        );
        while now() < t.expires {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
        assert!(
            w.accept_trade(ids[1], masks[1], masks[0])
                .err()
                .unwrap()
                .to_string()
                .contains("expired before mutual native acceptance")
        );
        assert!(
            store
                .market_receipts(t.accounts[0], t.version)
                .await
                .unwrap()
                .is_empty()
        );
    }
    #[tokio::test]
    async fn community_market_autonomous_trade_keeps_human_guards_and_binds_session_goods_and_death()
     {
        use crate::bots::pursuits::market::{Goods, Terms};
        let (mut w, store, ids, _) = fixture().await;
        let t = Terms {
            version: 51,
            accounts: ids.map(|id| w.players[&id].character.account),
            names: ids.map(|id| w.players[&id].character.name.clone()),
            goods: [
                Goods {
                    kind: 0xa22,
                    quantity: 1,
                },
                Goods {
                    kind: 0xa97,
                    quantity: 1,
                },
            ],
            meeting: [
                -2,
                w.players[&ids[0]].pos.x.floor() as i32,
                w.players[&ids[0]].pos.y.floor() as i32,
            ],
            expires: crate::bots::pursuits::market::now() + 120,
            batch: 1,
        };
        let d = t.delivery(0).unwrap();
        let session = [-2, ids[0], w.players[&ids[0]].character.id];
        assert!(
            w.autonomous_trade(
                ids[0],
                session,
                TradeAction::Request {
                    name: t.names[1].clone()
                },
                d.clone()
            )
            .is_err()
        );
        w.players.get_mut(&ids[0]).unwrap().controller = Controller::Autonomous;
        w.updates(ids[0], true).unwrap();
        let mut stale = session;
        stale[2] += 100;
        assert!(
            w.autonomous_trade(
                ids[0],
                stale,
                TradeAction::Request {
                    name: t.names[1].clone()
                },
                d.clone()
            )
            .is_err()
        );
        let mut elsewhere = d.clone();
        elsewhere.terms.meeting[1] += 10;
        assert!(
            w.autonomous_trade(
                ids[0],
                session,
                TradeAction::Request {
                    name: t.names[1].clone()
                },
                elsewhere
            )
            .is_err()
        );
        assert!(
            w.script_trade(
                ids[0],
                TradeAction::Request {
                    name: t.names[1].clone()
                }
            )
            .is_err()
        );
        start(&mut w, ids);
        let masks = offers(&mut w, ids).await;
        w.autonomous_trade(
            ids[0],
            session,
            TradeAction::Accept {
                mine: masks[0],
                theirs: masks[1],
            },
            d.clone(),
        )
        .unwrap();
        let plan = w.accept_trade(ids[1], masks[1], masks[0]).unwrap().unwrap();
        assert_eq!(plan.delivery, Some(d.clone()));
        let transaction = plan.transaction.clone();
        let writes = writes(&plan);
        w.players.get_mut(&ids[1]).unwrap().character.hp = 0;
        w.players.get_mut(&ids[1]).unwrap().death_pending = true;
        let result = store
            .exchange_trade(transaction, writes, Some(d))
            .await
            .unwrap();
        w.finish_trade(plan, Ok(result)).unwrap();
        let death = w.pending_deaths().pop().unwrap();
        assert_eq!(death.1.items[5], 0xa22);
        store.death(death.1.clone(), death.2).await.unwrap();
        w.finish_death(death.0).unwrap();
        assert!(store.load(death.1.account, death.1.id).await.is_err());
        assert_eq!(
            store
                .market_receipts(t.accounts[0], t.version)
                .await
                .unwrap()
                .len(),
            1
        );
    }
}
