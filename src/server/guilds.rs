use super::*;

use crate::store::GuildAction;
use std::time::Instant;

pub(super) async fn request(
    State(a): State<App>,
    axum::extract::Path(operation): axum::extract::Path<String>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    match admit(&a, &headers, peer) {
        Ok(true) => {}
        Ok(false) => return limited(),
        Err(e) => return error(e),
    }
    let result: Result<(String, bool)> = async {
        let guid = form.get("guid").context("guid required")?.clone();
        let password = form.get("password").context("password required")?.clone();
        ensure!(
            form.get("secret").is_none_or(|s| s.is_empty()),
            "External platform authentication is not implemented."
        );
        let _authentication = a.authentication.read().await;
        let account = a.store.authenticate(guid, password).await?;
        let _guild = a.guild_actions.lock().await;
        match operation.as_str() {
            "listMembers" => {
                let offset = form.get("offset").context("offset required")?.parse()?;
                let count = form.get("num").context("num required")?.parse()?;
                Ok((a.store.guild_roster(account, offset, count).await?, true))
            }
            "getBoard" => Ok((a.store.guild_board(account, None).await?, false)),
            "setBoard" => Ok((
                a.store
                    .guild_board(
                        account,
                        Some(form.get("board").context("board required")?.clone()),
                    )
                    .await?,
                false,
            )),
            _ => bail!("Unknown guild operation."),
        }
    }
    .await;
    match result {
        Ok((body, xml)) => (
            [
                (
                    "content-type",
                    if xml {
                        "application/xml"
                    } else {
                        "text/plain; charset=utf-8"
                    },
                ),
                ("cache-control", "no-store"),
            ],
            body,
        )
            .into_response(),
        Err(e) if e.is::<crate::store::InvalidCredentials>() => (
            [("content-type", "application/xml")],
            "<Error>Account credentials not valid</Error>",
        )
            .into_response(),
        Err(e) => error(e),
    }
}

pub(super) fn chat_command(text: &str) -> Option<(u8, &str)> {
    let command = text.split_whitespace().next()?;
    let kind = if command.eq_ignore_ascii_case("/invite") {
        wire::GUILD_INVITE
    } else if command.eq_ignore_ascii_case("/join") {
        wire::JOIN_GUILD
    } else {
        return None;
    };
    Some((kind, text.trim_start()[command.len()..].trim()))
}

pub(super) async fn merchant(a: &App, player: i32, body: &[u8]) -> Result<bool> {
    let mut r = Reader::new(body);
    let merchant = r.i32()?;
    r.done()?;
    a.world
        .for_player(player)
        .call(move |w| {
            // Invalid targets must receive the shop's rejection without disconnecting.
            let Some(target) = w.entities.get(&merchant) else {
                return Ok(false);
            };
            Ok(w.content.get(target.kind)?.class == "GuildMerchant")
        })
        .await
}

enum Request {
    Create(String),
    Invite(String),
    Join(String),
    Rank(String, i32),
    Remove(String),
    Buy(i32),
}

pub(super) async fn native(
    a: &App,
    id: i32,
    kind: u8,
    r: &mut Reader<'_>,
    tx: &mpsc::Sender<Packet>,
    now: Instant,
) -> Result<()> {
    let request = match kind {
        wire::CREATE_GUILD => Request::Create(r.utf()?),
        wire::GUILD_INVITE => Request::Invite(r.utf()?),
        wire::JOIN_GUILD => Request::Join(r.utf()?),
        wire::CHANGE_GUILD_RANK => Request::Rank(r.utf()?, r.i32()?),
        wire::GUILD_REMOVE => Request::Remove(r.utf()?),
        50 => Request::Buy(r.i32()?),
        _ => bail!("Invalid guild request."),
    };
    r.done()?;
    let _guild = a.guild_actions.lock().await;
    let mut scope = None;
    let mut committed = false;
    let result: Result<String>=async {
        let (account,actor,guild)=a.world.directory(move|d|d.guild_actor(id)).await?;
        scope=Some((account,guild));
        let (action,message)=match request {
            Request::Invite(name)=>{
                ensure!(!name.is_empty(),"Usage: /invite <player name>.");
                a.world.directory(move|d|d.guild_invite(id,&name,now)).await?;
                return Ok("Guild invitation sent.".into());
            },
            Request::Create(name)=>{
                a.world.directory(move|d|d.guild_creation(id)).await?;
                (GuildAction::Create(name),format!("{actor} founded the guild."))
            },
            Request::Join(name)=>{
                ensure!(!name.is_empty(),"Usage: /join <guild name>.");
                let (guild,inviter)=a.world.directory(move|d|d.guild_invitation(id,&name,now)).await?;
                (GuildAction::Join{guild,inviter},format!("{actor} joined the guild."))
            },
            Request::Rank(name,rank)=>{
                let label=match rank {0=>"Initiate",10=>"Member",20=>"Officer",30=>"Leader",_=>bail!("Invalid guild rank.")};
                let message=format!("{name} is now a {label}.");
                (GuildAction::Rank{name,rank},message)
            },
            Request::Remove(name)=>{
                let message=if name.eq_ignore_ascii_case(&actor){format!("{actor} left the guild.")}else{format!("{name} was removed from the guild by {actor}.")};
                (GuildAction::Remove(name),message)
            },
            Request::Buy(merchant)=>{
                let level=a.world.directory(move|d|d.guild_upgrade_level(id,merchant)).await?;
                (GuildAction::Upgrade{level},"Guild Hall upgrade purchased. It will take effect after the hall has been empty for a few minutes.".into())
            },
        };
        let (guild,target)=a.store.change_guild(account,action).await?;
        committed=true;
        let presentation=a.store.presentation(target).await?;
        let notice=message.clone();
        a.world.directory(move|d|{d.sync_guild(target,presentation,kind==wire::CREATE_GUILD)?;d.guild_notice(guild,&notice);Ok(())}).await?;
        Ok(message)
    }.await;
    let result = match result {
        Err(error) if committed || error.downcast_ref::<rusqlite::Error>().is_some() => {
            let (account, guild) =
                scope.context("Guild persistence failure lost its actor context")?;
            let reason = format!("Guild storage/publication failed; review required: {error:#}");
            tracing::error!(account,guild,error=%reason,"guild sessions quarantined after persistence failure");
            a.world
                .directory(move |d| d.guild_fault(account, guild, reason))
                .await?;
            return Err(
                error.context("Guild state requires review before this session can continue")
            );
        }
        other => other,
    };
    let mut out = Writer::new();
    if kind == 50 {
        match result {
            Ok(message) => {
                out.i32(0).utf(&message);
            }
            Err(e) => {
                tracing::warn!(player=id,error=%e,"guild purchase rejected");
                out.i32(9).utf(&e.to_string());
            }
        }
        return enqueue_reply(
            tx,
            Packet {
                id: 27,
                body: out.0,
            },
        );
    }
    if matches!(
        kind,
        wire::CREATE_GUILD | wire::CHANGE_GUILD_RANK | wire::GUILD_REMOVE
    ) {
        match result {
            Ok(_) => {
                out.u8(1).utf("");
            }
            Err(e) => {
                tracing::warn!(player=id,error=%e,"guild change rejected");
                out.u8(0).utf(&e.to_string());
            }
        }
        enqueue_reply(
            tx,
            Packet {
                id: wire::GUILD_RESULT,
                body: out.0,
            },
        )
    } else {
        let packet = match result {
            Ok(message) if kind == wire::GUILD_INVITE => crate::world::chat_info(&message),
            Ok(_) => return Ok(()),
            Err(e) => {
                tracing::warn!(player=id,error=%e,"guild invitation/join rejected");
                crate::world::chat_error(&e.to_string())
            }
        };
        enqueue_reply(tx, packet)
    }
}
