use super::*;

pub(super) async fn incoming(
    app: &crate::server::App,
    brain: &mut Brain,
    v: &PlayerView,
) -> Result<bool> {
    let Some(g) = brain.mind.market.incoming.clone() else {
        return Ok(false);
    };
    let action = brain
        .mind
        .market
        .incoming_action(v, &app.content, brain.mind.future_class)?;
    if let Some(action) = action {
        let cancel = matches!(action, crate::world::TradeAction::Cancel);
        let opening = matches!(action, crate::world::TradeAction::Request { .. });
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
                    .autonomous_incoming_trade(id, session, g.player, (g.account, g.name), action)
            })
            .await;
        match result {
            Ok(Some(plan)) => crate::server::persist_trade(app, plan).await?,
            Ok(None) => {
                if opening {
                    tracing::info!(
                        account = v.session.account,
                        "market incoming request opened with empty offer"
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
