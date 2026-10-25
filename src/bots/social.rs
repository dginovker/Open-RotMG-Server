use super::*;
use crate::content::PRIEST;
use crate::world::view::VisibleObject;
mod incoming;
mod life;
pub(crate) use life::Life;
pub(super) use life::{cancel_gift as interrupt_gift, life, receive_life};

pub(super) fn companions<'a>(
    v: &'a PlayerView,
    group: Option<&adventures::Adventure>,
) -> Vec<&'a VisibleObject> {
    v.objects
        .iter()
        .filter(|o| {
            o.hp > 0
                && !o.equipped.is_empty()
                && group.is_some_and(|g| {
                    !g.finished && g.members.iter().any(|m| Some(m.account) == o.account)
                })
        })
        .collect()
}

fn healing_selected(
    v: &PlayerView,
    content: &Content,
    eligible: impl Fn(&VisibleObject) -> bool,
) -> Result<Option<Action>> {
    if v.character.class != PRIEST
        || v.character.items[1] < 0
        || v.time_ms < v.character.ability_ready_ms
        || v.character.conditions & crate::condition::bit(crate::condition::QUIET) != 0
    {
        return Ok(None);
    }
    let tome = content.get(v.character.items[1] as u16)?;
    if tome.activate != "HealNova" || v.character.mp < tome.mana_cost {
        return Ok(None);
    }
    let own_need = v.character.stats[0] - v.character.hp;
    let needs_cleanse = events::shared(&v.map.name)
        && tome
            .abilities
            .iter()
            .any(|a| matches!(a, crate::content::Ability::Cleanse { .. }))
        && v.character.conditions
            & (crate::content::PARALYZED | crate::content::STUNNED | crate::content::SLOWED)
            != 0;
    let needs_healing = needs_cleanse
        || own_need >= (tome.heal.min(v.character.stats[0]) as f32 * 0.35) as i32
        || v.objects.iter().any(|o| {
            !o.equipped.is_empty()
                && eligible(o)
                && o.hp > 0
                && o.max_hp - o.hp >= (tome.heal.min(o.max_hp) as f32 * 0.35) as i32
                && v.position.distance(o.position) <= tome.heal_range
        });
    Ok(needs_healing.then_some(Action::Use {
        slot: 1,
        x: v.position.x,
        y: v.position.y,
    }))
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum MarketUptake {
    Unrelated,
    Applied,
    PendingCommit,
}

pub(super) async fn recover_market(app: &crate::server::App, brain: &mut Brain) -> Result<()> {
    if let Some(b) = &brain.mind.market.bargain {
        let receipts = app
            .store
            .market_receipts(brain.mind.account, b.terms.version)
            .await?;
        brain.mind.market.recover(receipts)?;
    }
    Ok(())
}
async fn revoke_native_market(app: &crate::server::App, brain: &Brain) -> Result<bool> {
    let session = [brain.session.0, brain.session.1, brain.session.2];
    let id = session[1];
    app.world
        .directory(move |d| {
            ensure!(
                d.location(id)? == session[0],
                "market revocation world changed"
            );
            d.worlds
                .get_mut(&session[0])
                .context("market world missing")?
                .revoke_autonomous_trade(id, session)
        })
        .await
}
pub(super) async fn revoke_market(app: &crate::server::App, brain: &mut Brain) -> Result<bool> {
    if !brain.mind.market.revoke_trade {
        return Ok(true);
    }
    if !revoke_native_market(app, brain).await? {
        return Ok(false);
    }
    recover_market(app, brain).await?;
    brain.mind.market.revoke_trade = false;
    brain.mind.market.trade = None;
    Ok(true)
}
pub(super) async fn receive_market(
    app: &crate::server::App,
    brain: &mut Brain,
    packet: &Packet,
    view: &PlayerView,
) -> Result<MarketUptake> {
    ensure!(
        [
            view.session.world,
            view.session.player,
            view.session.character
        ] == [brain.session.0, brain.session.1, brain.session.2]
            && view.session.account == brain.mind.account,
        "market packet belongs to a replaced session"
    );
    if packet.id == 61 && brain.mind.social.gift_active() {
        tracing::info!(
            account = view.session.account,
            reason = "active_gift",
            "market incoming request policy"
        );
        return Ok(MarketUptake::Applied);
    }
    let before = brain.mind.clone();
    if packet.id == wire::TEXT && !view.lifecycle.busy {
        recover_market(app, brain).await?;
    }
    let mut next = brain.mind.market.clone();
    let handled = next.receive(packet, view, &app.content, brain.mind.future_class)?;
    if handled && next.revoke_trade {
        if !revoke_native_market(app, brain).await? {
            if before != brain.mind {
                brain.mind.revision = app.store.save_bot_mind(brain.mind.clone()).await?;
            }
            return Ok(MarketUptake::PendingCommit);
        }
        // The other accept may have committed before the revocation job: receipts, never holdings, decide.
        recover_market(app, brain).await?;
        let id = brain.session.1;
        let current = app.world.player_view(id).await?;
        // A sender bound at receipt cannot undo its withdrawal by leaving during the commit fence.
        next = brain.mind.market.clone();
        ensure!(
            next.receive_observed(
                packet,
                &current,
                &view.objects,
                &app.content,
                brain.mind.future_class
            )?,
            "market message changed classification at revocation fence"
        );
        next.revoke_trade = false;
    }
    if handled {
        brain.mind.market = next;
    }
    if before != brain.mind {
        brain.mind.revision = app.store.save_bot_mind(brain.mind.clone()).await?;
    }
    Ok(if handled {
        MarketUptake::Applied
    } else {
        MarketUptake::Unrelated
    })
}

pub(super) async fn market(
    app: &crate::server::App,
    brain: &mut Brain,
    v: &PlayerView,
) -> Result<bool> {
    use crate::bots::pursuits::market::Goods;
    use crate::world::TradeAction;
    let before = brain.mind.clone();
    if !revoke_market(app, brain).await? {
        return Ok(true);
    }
    let incoming_active = if brain.mind.market.incoming.is_some() {
        incoming::incoming(app, brain, v).await?
    } else {
        false
    };
    brain.mind.market.observe(v);
    if let Some(b) = &brain.mind.market.bargain {
        let receipts = app
            .store
            .market_receipts(v.session.account, b.terms.version)
            .await?;
        brain.mind.market.recover(receipts)?;
    }
    if let Some(message) = brain
        .mind
        .market
        .outgoing
        .clone()
        .filter(|_| v.elapsed_ms >= brain.mind.market.next_message_ms)
    {
        if !brain
            .mind
            .market
            .message_current(&message, v, &app.content, brain.mind.future_class)?
        {
            brain.mind.market.outgoing = None;
            tracing::info!(
                account = v.session.account,
                "market speech discarded: intent or held goods changed"
            );
        } else if let Some(service) = &app.dialogue {
            match service.market_wording(
                &brain.mind.market.name,
                &message,
                &app.content,
                serde_json::json!({"sociability":brain.profile.companions,
                    "patience":brain.profile.reaction,"resources":brain.profile.resources}),
            ) {
                Ok(Some(words)) => {
                    let mut generated = message.clone();
                    generated.text = words;
                    let text = generated.chat();
                    let id = v.session.player;
                    let session = [v.session.world, id, v.session.character];
                    let revision = v.character.inventory_revision;
                    let market = brain.mind.market.clone();
                    let content = app.content.clone();
                    let future = brain.mind.future_class;
                    let received = app.world.clock.now();
                    let published = app
                        .world
                        .directory(move |d| {
                            let current = d.player_view(id)?;
                            ensure!(
                                [current.session.world, id, current.session.character] == session
                                    && current.character.inventory_revision == revision
                                    && market
                                        .message_current(&message, &current, &content, future)?,
                                "market speech state changed before publication"
                            );
                            d.chat(id, text, received)
                        })
                        .await;
                    match published {
                        Ok(()) => {
                            brain.mind.market.outgoing = None;
                            brain.mind.market.next_message_ms = v.elapsed_ms + 1500;
                            tracing::info!(
                                account = v.session.account,
                                text = generated.text,
                                "generated inventory-bound market speech published"
                            );
                        }
                        Err(error) => {
                            brain.mind.market.outgoing = None;
                            tracing::warn!(account=v.session.account, %error, "market speech not published");
                        }
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    brain.mind.market.outgoing = None;
                    tracing::error!(account=v.session.account, %error, "market generation failed; no substitute speech");
                }
            }
        } else {
            brain.mind.market.outgoing = None;
            tracing::warn!(
                account = v.session.account,
                "market speech unavailable: dialogue service disabled; no substitute speech"
            );
        }
    }
    if before != brain.mind {
        brain.mind.revision = app.store.save_bot_mind(brain.mind.clone()).await?;
    }
    if incoming_active {
        return Ok(brain.mind.market.trade.is_some()
            || brain.mind.market.incoming.as_ref().is_some_and(|g| {
                v.objects
                    .iter()
                    .any(|p| p.id == g.player && p.position.distance(v.position) < 2.)
            }));
    }
    let Some(b) = brain
        .mind
        .market
        .bargain
        .clone()
        .filter(|b| b.resolution.is_none())
    else {
        return Ok(false);
    };
    let side = b
        .terms
        .accounts
        .iter()
        .position(|a| *a == v.session.account)
        .context("foreign negotiated account")?;
    let peer = 1 - side;
    // Historical terms permit an inspection request, not a silent renewal of native acceptance.
    let bound = v.objects.iter().any(|p| {
        p.account == Some(b.terms.accounts[peer])
            && p.name == b.terms.names[peer]
            && p.hp > 0
            && v.position.distance(p.position) < 2.
    });
    let meeting = Pos {
        x: b.terms.meeting[1] as f32 + 0.5,
        y: b.terms.meeting[2] as f32 + 0.5,
    };
    if !(b.agreed || b.confirmed > 0)
        || !bound
        || b.terms.expires <= pursuits::market::now()
        || v.session.world != b.terms.meeting[0]
        || v.position.distance(meeting) >= 2.
    {
        if brain.mind.market.trade.is_some() {
            brain.mind.market.revoke_trade = true;
            return Ok(!revoke_market(app, brain).await?);
        }
        return Ok(false);
    }
    let delivery = b.terms.delivery(b.confirmed)?;
    let session = [v.session.world, v.session.player, v.session.character];
    let action = if let Some(t) = brain.mind.market.trade.as_mut() {
        if t.binding != session || t.peer != b.terms.names[peer] {
            Some(TradeAction::Cancel)
        } else {
            let select = |items: &[i32], goods: &Goods, n: u8| -> Option<[bool; 12]> {
                let mut values = [false; 12];
                let mut count = 0;
                for (i, k) in items.iter().enumerate().skip(4) {
                    if count < n && *k == goods.kind {
                        values[i] = true;
                        count += 1;
                    }
                }
                (count == n).then_some(values)
            };
            let mine = select(&t.items[0], &b.terms.goods[side], delivery.quantities[side]);
            let theirs = select(&t.items[1], &b.terms.goods[peer], delivery.quantities[peer]);
            match (mine, theirs) {
                (Some(mine), Some(theirs))
                    if mine.iter().zip(t.tradeable).all(|(a, b)| !*a || b) =>
                {
                    if !t.changed || t.offers[0] != mine {
                        t.changed = true;
                        t.offers[0] = mine;
                        t.ready = v.time_ms + 3000;
                        Some(TradeAction::Change { offer: mine })
                    } else if t.offers[1] == theirs
                        && v.time_ms >= t.ready
                        && v.clock_at >= t.ready_at
                    {
                        Some(TradeAction::Accept { mine, theirs })
                    } else {
                        None
                    }
                }
                _ => Some(TradeAction::Cancel),
            }
        }
    } else {
        let count = v
            .character
            .items
            .iter()
            .skip(4)
            .filter(|k| **k == b.terms.goods[side].kind)
            .count();
        let free = v.character.items.iter().skip(4).filter(|k| **k < 0).count();
        if count < delivery.quantities[side] as usize
            || free + (delivery.quantities[side] as usize) < delivery.quantities[peer] as usize
        {
            return Ok(false);
        }
        if !brain.mind.market.request_due(v.elapsed_ms) {
            return Ok(false);
        }
        Some(TradeAction::Request {
            name: b.terms.names[peer].clone(),
        })
    };
    if let Some(action) = action {
        let changed = matches!(action, TradeAction::Change { .. });
        let id = v.session.player;
        let outcome = app
            .world
            .directory(move |d| {
                ensure!(
                    d.location(id)? == session[0],
                    "autonomous market world/session changed"
                );
                d.worlds
                    .get_mut(&session[0])
                    .context("market world missing")?
                    .autonomous_trade(id, session, action, delivery)
            })
            .await;
        match outcome {
            Ok(Some(plan)) => crate::server::persist_trade(app, plan).await?,
            Ok(None) if changed => {
                brain
                    .mind
                    .market
                    .trade
                    .as_mut()
                    .context("market trade disappeared after offer acknowledgement")?
                    .ready_at = app.world.clock.now() + std::time::Duration::from_millis(3000);
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(account=v.session.account,%error,"market guarded trade rejected; no settlement inferred");
                app.world
                    .for_player(id)
                    .call(move |w| {
                        w.cancel_trade(id, "Market terms or availability changed.");
                        Ok(())
                    })
                    .await?;
                brain.mind.market.trade = None;
            }
        }
        return Ok(true);
    }
    // A bank trip or lost contact must not hold the other controller's movement lock.
    Ok(brain.mind.market.trade.is_some())
}
