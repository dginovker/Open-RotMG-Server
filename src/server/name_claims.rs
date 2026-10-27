use super::*;

// Callers hold name_claims through Store commit/publication; autonomous admission takes the same lock.
pub(super) async fn prepare(
    a: &App,
    account: i32,
    name: String,
    allow_paid: bool,
) -> Result<std::result::Result<Vec<i32>, &'static str>> {
    let stopped = match a
        .store
        .prepare_name_choice(account, name, allow_paid)
        .await?
    {
        Ok(bots) => bots,
        Err(reason) => return Ok(Err(reason)),
    };
    for bot in &stopped {
        if let Err(error) = a.stop_persona(*bot).await {
            if let Some(crate::store::Rejected(reason)) = error.downcast_ref() {
                return Ok(Err(*reason));
            }
            let message =
                format!("Could not release persona account {bot}; name was not claimed: {error:#}");
            return Err(error.context(message));
        }
        if let Some((persona, player)) = a.cctv.account_session(*bot) {
            tracing::error!(
                account = *bot,
                persona,
                player,
                "name claim refused: CCTV session remains after account stop"
            );
            return Ok(Err(
                "Persona session cleanup is incomplete; name was not claimed. Operator inspection required.",
            ));
        }
    }
    Ok(Ok(stopped))
}

pub(super) async fn native(
    a: &App,
    account: i32,
    player: Option<i32>,
    name: String,
    tx: &mpsc::Sender<Packet>,
) -> Result<()> {
    let names = a.name_claims.clone().lock_owned().await;
    if let Some(id) = player
        && let Err(reason) = a
            .world
            .for_player(id)
            .call(move |w| w.check_name_choice(id, account))
            .await?
    {
        return reply(tx, Some(reason));
    }
    let stopped = match prepare(a, account, name.clone(), player.is_some()).await? {
        Ok(stopped) => stopped,
        Err(reason) => return reply(tx, Some(reason)),
    };
    let rejection = lifecycle::account(
        a,
        vec![account],
        "native_name_choice",
        move |a| async move {
            let _names = names;
            let _gold = a.gold_updates.lock().await;
            lifecycle::stage(Stage::Reservation);
            let reservation = if let Some(id) = player {
                match a
                    .world
                    .for_player(id)
                    .call(move |w| w.begin_name_choice(id, account))
                    .await?
                {
                    Ok(plan) => Some((id, plan)),
                    Err(reason) => {
                        lifecycle::stage(Stage::Rejected);
                        return Ok(Some(reason));
                    }
                }
            } else {
                None
            };
            lifecycle::stage(Stage::Store);
            let result = a
                .store
                .claim_name_choice(account, name.clone(), reservation.is_some(), stopped)
                .await;
            lifecycle::stage(Stage::Publication);
            let rejection = if let Some((id, plan)) = reservation {
                a.world
                    .for_player(id)
                    .call(move |w| w.finish_name_choice(plan, name, result))
                    .await?
            } else {
                match result? {
                    crate::store::NameChoice::Rejected(reason) => Some(reason),
                    crate::store::NameChoice::Committed { .. } => None,
                }
            };
            lifecycle::stage(Stage::Settled);
            Ok(rejection)
        },
    )
    .await?;
    reply(tx, rejection)
}

fn reply(tx: &mpsc::Sender<Packet>, rejection: Option<&'static str>) -> Result<()> {
    let mut out = Writer::new();
    out.u8(u8::from(rejection.is_none()))
        .utf(rejection.unwrap_or(""));
    enqueue_reply(
        tx,
        Packet {
            id: 20,
            body: out.0,
        },
    )
}
