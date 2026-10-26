use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::bots) struct Incoming {
    pub session: [i32; 3],
    pub player: i32,
    pub account: i32,
    pub name: String,
    pub quote: Option<[Goods; 2]>,
    until: u64,
    requested: bool,
    outbound: bool,
}
impl Incoming {
    pub(super) fn outbound(
        v: &PlayerView,
        player: i32,
        account: i32,
        name: String,
        quote: [Goods; 2],
    ) -> Self {
        Self {
            session: [v.session.world, v.session.player, v.session.character],
            player,
            account,
            name,
            quote: Some(quote),
            until: v.elapsed_ms + 60_000,
            requested: false,
            outbound: true,
        }
    }
}
impl Market {
    pub(super) fn incoming_request(&mut self, name: String, v: &PlayerView) {
        if self.incoming.is_some()
            || self.trade.is_some()
            || self.reserved_bargain()
            || v.elapsed_ms < self.next_request_ms
            || v.map.name != "Nexus"
            || !super::super::settled(v)
        {
            return;
        }
        if let Some(p) = v.objects.iter().find(|p| {
            p.name == name
                && p.account.is_some()
                && p.hp > 0
                && !p.equipped.is_empty()
                && v.position.distance(p.position) < 2.
        }) {
            self.incoming = Some(Incoming {
                session: [v.session.world, v.session.player, v.session.character],
                player: p.id,
                account: p.account.unwrap(),
                name,
                quote: self.quote.clone(),
                until: v.elapsed_ms + 20_000,
                requested: false,
                outbound: false,
            });
        }
    }
    pub(in crate::bots) fn end_incoming(&mut self, v: &PlayerView) {
        self.incoming = None;
        self.next_request_ms = v.elapsed_ms + 20_000;
    }
    pub(in crate::bots) fn incoming_action(
        &mut self,
        v: &PlayerView,
        c: &Content,
        future: Option<u16>,
    ) -> Result<Option<crate::world::TradeAction>> {
        use crate::world::TradeAction;
        let Some(g) = self.incoming.as_ref() else {
            return Ok(None);
        };
        let valid = g.session == [v.session.world, v.session.player, v.session.character]
            && v.map.name == "Nexus"
            && super::super::settled(v)
            && v.elapsed_ms < g.until
            && v.character.hp * 4 >= v.character.stats[0] * 3
            && !self.reserved_bargain();
        let peer = v.objects.iter().find(|p| {
            p.id == g.player && p.account == Some(g.account) && p.name == g.name && p.hp > 0
        });
        if !valid || peer.is_none() {
            return Ok(Some(TradeAction::Cancel));
        }
        if self.incoming.as_ref().unwrap().quote.is_none() {
            self.incoming.as_mut().unwrap().quote = self.quote(v, c, future)?;
        }
        let g = self.incoming.as_ref().unwrap();
        let Some(q) = g.quote.clone() else {
            return Ok(Some(TradeAction::Cancel));
        };
        if self.spare(v, c, future, q[0].kind)? < q[0].quantity {
            return Ok(Some(TradeAction::Cancel));
        }
        if peer.unwrap().position.distance(v.position) >= 2. {
            return Ok(None);
        }
        let Some(t) = self.trade.as_mut() else {
            if g.requested {
                return Ok(None);
            }
            let name = g.name.clone();
            let g = self.incoming.as_mut().unwrap();
            g.requested = true;
            g.until = v.elapsed_ms + 60_000;
            tracing::info!(
                account = v.session.account,
                peer = g.account,
                outbound = g.outbound,
                ?q,
                "market inspecting native exchange"
            );
            return Ok(Some(TradeAction::Request { name }));
        };
        if t.binding != g.session || t.peer != g.name || t.items[0] != v.character.items {
            return Ok(Some(TradeAction::Cancel));
        }
        let Some(mine) = select(&t.items[0], &q[0]) else {
            return Ok(Some(TradeAction::Cancel));
        };
        if mine.iter().zip(t.tradeable).any(|(a, b)| *a && !b) {
            return Ok(Some(TradeAction::Cancel));
        }
        let free = v.character.items.iter().skip(4).filter(|k| **k < 0).count();
        if free + usize::from(q[0].quantity) < usize::from(q[1].quantity) {
            return Ok(Some(TradeAction::Cancel));
        }
        if !t.changed || t.offers[0] != mine {
            t.changed = true;
            t.offers[0] = mine;
            t.ready = v.time_ms + 3000;
            t.ready_at = v.clock_at + Duration::from_millis(3000);
            return Ok(Some(TradeAction::Change { offer: mine }));
        }
        if t.offers[1].iter().all(|b| !*b) {
            return Ok(None);
        }
        let compatible = t.offers[1]
            .iter()
            .enumerate()
            .all(|(i, b)| !*b || (i >= 4 && t.items[1][i] == q[1].kind))
            && t.offers[1].iter().filter(|b| **b).count() == usize::from(q[1].quantity);
        if !compatible && v.time_ms >= t.ready && v.clock_at >= t.ready_at {
            self.queue(
                format!("nty need {}", goods_text(&q[1], c)?),
                Channel::Tell(g.name.clone()),
            );
            return Ok(Some(TradeAction::Cancel));
        }
        Ok(
            (compatible && v.time_ms >= t.ready && v.clock_at >= t.ready_at).then_some(
                TradeAction::Accept {
                    mine,
                    theirs: t.offers[1],
                },
            ),
        )
    }
}
fn select(items: &[i32], goods: &Goods) -> Option<[bool; 12]> {
    let mut mask = [false; 12];
    let mut count = 0;
    for (i, k) in items.iter().enumerate().skip(4) {
        if *k == goods.kind && count < goods.quantity {
            mask[i] = true;
            count += 1;
        }
    }
    (count == goods.quantity).then_some(mask)
}
