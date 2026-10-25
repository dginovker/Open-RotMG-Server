use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::bots) struct Incoming {
    pub session: [i32; 3],
    pub player: i32,
    pub account: i32,
    pub name: String,
    until: u64,
    requested: bool,
}
impl Market {
    pub(super) fn incoming_request(&mut self, name: String, v: &PlayerView) {
        let reason = if self.incoming.is_some() || self.trade.is_some() {
            "already_trading"
        } else if self
            .bargain
            .as_ref()
            .is_some_and(|b| b.resolution.is_none())
        {
            "active_bargain"
        } else if v.elapsed_ms < self.next_request_ms {
            "request_cooldown"
        } else if v.map.name != "Nexus" {
            "outside_nexus"
        } else if !super::super::settled(v) {
            "not_ready"
        } else if !v.character.items.iter().skip(4).any(|k| *k < 0) {
            "no_capacity"
        } else if let Some(p) = v.objects.iter().find(|p| {
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
                name: name.clone(),
                until: v.elapsed_ms + 20_000,
                requested: false,
            });
            "eligible_request"
        } else {
            "unobserved_or_out_of_reach"
        };
        tracing::info!(
            account = v.session.account,
            peer = name,
            reason,
            "market incoming request policy"
        );
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
        let reason = if g.session != [v.session.world, v.session.player, v.session.character]
            || v.map.name != "Nexus"
            || !super::super::settled(v)
        {
            Some("session_or_lifecycle_changed")
        } else if v.elapsed_ms >= g.until {
            Some("expired")
        } else if self
            .bargain
            .as_ref()
            .is_some_and(|b| b.resolution.is_none())
        {
            Some("active_bargain")
        } else if !v.objects.iter().any(|p| {
            p.id == g.player
                && p.account == Some(g.account)
                && p.name == g.name
                && p.hp > 0
                && v.position.distance(p.position) < 2.
        }) {
            Some("contact_changed")
        } else {
            None
        };
        if let Some(reason) = reason {
            tracing::info!(
                account = v.session.account,
                peer = g.account,
                reason,
                "market incoming trade declined"
            );
            return Ok(Some(TradeAction::Cancel));
        }
        let Some(t) = self.trade.as_ref() else {
            if g.requested {
                return Ok(None);
            }
            let name = g.name.clone();
            let g = self.incoming.as_mut().unwrap();
            g.requested = true;
            g.until = v.elapsed_ms + 60_000;
            return Ok(Some(TradeAction::Request { name }));
        };
        if t.binding != g.session || t.peer != g.name || t.offers[0].iter().any(|b| *b) {
            tracing::info!(
                account = v.session.account,
                reason = "binding_or_own_goods_changed",
                "market incoming trade declined"
            );
            return Ok(Some(TradeAction::Cancel));
        }
        let mut quantities = BTreeMap::<i32, u8>::new();
        for (slot, offered) in t.offers[1].iter().enumerate() {
            if *offered {
                if slot < 4 || t.items[1][slot] < 0 {
                    tracing::info!(
                        account = v.session.account,
                        reason = "invalid_gift_slot",
                        "market incoming trade declined"
                    );
                    return Ok(Some(TradeAction::Cancel));
                }
                *quantities.entry(t.items[1][slot]).or_default() += 1;
            }
        }
        if quantities.is_empty() {
            return Ok(None);
        }
        let free = v.character.items.iter().skip(4).filter(|k| **k < 0).count();
        let reason = if quantities.values().map(|q| *q as usize).sum::<usize>() > free {
            Some("gift_exceeds_capacity")
        } else {
            let mut unwanted = false;
            for (kind, quantity) in quantities {
                if self.wanted(v, c, future, kind)? < quantity {
                    unwanted = true;
                }
            }
            unwanted.then_some("gift_not_needed")
        };
        if let Some(reason) = reason {
            tracing::info!(
                account = v.session.account,
                peer = g.account,
                reason,
                "market incoming trade declined"
            );
            return Ok(Some(TradeAction::Cancel));
        }
        Ok(
            (v.time_ms >= t.ready && v.clock_at >= t.ready_at).then_some(TradeAction::Accept {
                mine: [false; 12],
                theirs: t.offers[1],
            }),
        )
    }
}
