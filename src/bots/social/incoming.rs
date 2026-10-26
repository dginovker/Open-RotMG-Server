use super::*;

pub(super) async fn incoming(
    app: &crate::server::App,
    brain: &mut Brain,
    v: &PlayerView,
) -> Result<bool> {
    let Some(_) = brain.mind.market.incoming.as_ref() else {
        return Ok(false);
    };
    let action = brain
        .mind
        .market
        .incoming_action(v, &app.content, brain.mind.future_class)?;
    if let Some(action) = action {
        let g = brain
            .mind
            .market
            .incoming
            .clone()
            .context("incoming exchange disappeared")?;
        let cancel = matches!(action, crate::world::TradeAction::Cancel);
        let opening = matches!(action, crate::world::TradeAction::Request { .. });
        let changed = matches!(action, crate::world::TradeAction::Change { .. });
        let id = v.session.player;
        let session = g.session;
        let result = app
            .world
            .directory(move |d| {
                ensure!(
                    d.location(id)? == session[0],
                    "incoming trade world changed"
                );
                d.worlds
                    .get_mut(&session[0])
                    .context("incoming trade world missing")?
                    .autonomous_incoming_trade(
                        id,
                        session,
                        g.player,
                        (g.account, g.name),
                        action,
                        g.quote,
                    )
            })
            .await;
        match result {
            Ok(Some(plan)) => crate::server::persist_trade(app, plan).await?,
            Ok(None) => {
                if changed && let Some(trade) = brain.mind.market.trade.as_mut() {
                    trade.ready_at = app.world.clock.now() + std::time::Duration::from_millis(3000);
                }
                if opening {
                    tracing::info!(
                        account = v.session.account,
                        "market native exchange opened; WINDOW offers remain authoritative"
                    );
                }
            }
            Err(error) => {
                tracing::warn!(account=v.session.account,%error,"market incoming action rejected; no settlement inferred");
                app.world
                    .for_player(id)
                    .call(move |w| {
                        w.cancel_trade(id, "Incoming trade availability changed.");
                        Ok(())
                    })
                    .await?;
                brain.mind.market.end_incoming(v);
                brain.mind.market.trade = None;
                return Ok(false);
            }
        }
        if cancel {
            brain.mind.market.end_incoming(v);
            brain.mind.market.trade = None;
            return Ok(false);
        }
    }
    Ok(brain.mind.market.incoming.is_some())
}
