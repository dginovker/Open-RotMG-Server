mod input;

#[cfg(test)]
mod password_tests;
mod pictures;
use crate::{
    content::{Content, WIZARD},
    crypto::Keys,
    store::{InventoryWrite, Store, xml_escape},
    wire::{self, Codec, Packet, Reader, Writer},
    world::{Action, Handle, InventoryPlan, Pos, Slot, TradeAction, TradePlan, World},
};
use anyhow::{Context, Result, bail, ensure};
use axum::{
    Json, Router,
    extract::{
        ConnectInfo, DefaultBodyLimit, Form, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::{FutureExt, SinkExt, StreamExt};
use input::Incoming;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashMap},
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
pub(crate) mod lifecycle;
pub use lifecycle::Owner;
use lifecycle::{CloseReason, Stage};
impl App {
    fn server_usage(&self) -> Result<f64> {
        ensure!(
            (1..=85).contains(&self.max_players),
            "invalid server capacity: {}",
            self.max_players
        );
        // Admission includes claims before avatars exist and retains them through failed saves.
        let occupied = self
            .owners
            .lock()
            .map_err(|_| anyhow::anyhow!("ownership registry poisoned"))?
            .len();
        ensure!(
            occupied <= self.max_players,
            "ownership exceeds capacity: {occupied}/{}",
            self.max_players
        );
        let usage = occupied as f64 / self.max_players as f64;
        tracing::debug!(
            occupied,
            capacity = self.max_players,
            usage,
            "advertised admission usage"
        );
        Ok(usage)
    }
    pub(crate) fn begin_admission(&self) {
        // The owners lock orders this irreversible latch against capacity preparation.
        let _owners = self.owners.lock().expect("ownership registry poisoned");
        self.admission_started.store(true, Ordering::Relaxed);
    }
}
use tokio::sync::mpsc;
mod account_actions;
#[cfg(test)]
pub(crate) use account_actions::tests::fixture as test_app;

pub(crate) mod browser_link;
pub(crate) mod diagnostics;
mod gold;
mod guilds;
mod hiscores;
pub(crate) mod testing;
mod updates;
use account_actions::{discord_announce, discord_kick, discord_recover, reset_password};
use hiscores::{collection, discord_link, hiscores};
#[derive(Clone)]
pub struct App {
    pub started: std::time::Instant,
    pub testing: Option<testing::Testing>,
    pub diagnostics: Arc<diagnostics::Diagnostics>,
    pub store: Store,
    pub world: Handle,
    pub keys: Arc<Keys>,
    pub content: Arc<Content>,
    pub swf: Arc<Vec<u8>>,
    pub origin: String,
    pub sfx: PathBuf,
    pub max_players: usize,
    pub behind_proxy: bool,
    pub shore_bots: bool,
    pub bot_population: usize,
    pub bot_session_seconds: [u64; 2],
    pub dialogue: Option<Arc<crate::bots::dialogue::Service>>,
    pub society: Arc<Mutex<crate::bots::adventures::Society>>,
    pub cctv: Arc<crate::bots::cctv::Cctv>,
    pub requests: Arc<Mutex<HashMap<IpAddr, (std::time::Instant, u32)>>>,
    pub picture_reads: Arc<Mutex<HashMap<IpAddr, (std::time::Instant, u32)>>>,
    pub connections: Arc<tokio::sync::Semaphore>,
    pub control: Option<String>,
    pub discord_link_token: Option<String>,
    pub(crate) browser_links: Arc<browser_link::Service>,
    pub owners: Arc<Mutex<BTreeMap<i32, lifecycle::Claim>>>,
    pub(crate) admission_started: Arc<AtomicBool>,
    pub authentication: Arc<tokio::sync::RwLock<()>>,
    pub guild_actions: Arc<tokio::sync::Mutex<()>>,
    pub gold_updates: Arc<tokio::sync::Mutex<()>>,
    pub bots: Arc<Mutex<BTreeMap<i32, Owner>>>,
    #[cfg(test)]
    pub quarantined: Arc<Mutex<Vec<Owner>>>,
    pub shutdown: tokio::sync::watch::Sender<bool>,
    pub(crate) admission: Arc<lifecycle::drain::Admission>,
}
fn error(e: anyhow::Error) -> Response {
    tracing::error!(error=%format_args!("{e:#}"),"request failed");
    (
        StatusCode::BAD_REQUEST,
        [("content-type", "application/xml")],
        format!("<Error>{}</Error>", xml_escape(&e.to_string())),
    )
        .into_response()
}
pub(crate) fn request_ip(a: &App, headers: &HeaderMap, peer: SocketAddr) -> Result<IpAddr> {
    if let Some(origin) = headers.get("origin") {
        ensure!(
            origin.to_str()? == a.origin,
            "unapproved request origin: {}; expected {}",
            origin.to_str()?,
            a.origin
        );
    }
    let ip = if a.behind_proxy {
        headers
            .get("x-forwarded-for")
            .context("proxy must supply X-Forwarded-For")?
            .to_str()?
            .parse::<IpAddr>()
            .context("proxy must supply one client IP")?
    } else {
        peer.ip()
    };
    Ok(ip)
}
fn admit(a: &App, headers: &HeaderMap, peer: SocketAddr) -> Result<bool> {
    let ip = request_ip(a, headers, peer)?;
    Ok(rate_allow(
        &mut a.requests.lock().expect("request registry poisoned"),
        ip,
        std::time::Instant::now(),
    ))
}
fn rate_allow(
    requests: &mut HashMap<IpAddr, (std::time::Instant, u32)>,
    ip: IpAddr,
    now: std::time::Instant,
) -> bool {
    requests.retain(|_, (start, _)| now.duration_since(*start).as_secs() < 60);
    if requests.len() >= 4096 && !requests.contains_key(&ip) {
        return false;
    }
    let (_, count) = requests.entry(ip).or_insert((now, 0));
    *count = count.saturating_add(1);
    if *count == 21 {
        tracing::warn!(%ip, "guest entry rate exceeded");
    }
    *count <= 20
}
fn limited() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [("content-type", "application/xml"), ("retry-after", "60")],
        "<Error>Too many guest entry requests; retry in one minute.</Error>",
    )
        .into_response()
}
async fn chars(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    match admit(&a, &headers, peer) {
        Ok(true) => {}
        Ok(false) => return limited(),
        Err(e) => return error(e),
    }
    let result: Result<(String, Option<String>)> = async {
        let mut browser = browser_link::native(&a, &headers, &query).await?;
        let guid = form.get("guid").context("guid is required")?.clone();
        ensure!(
            form.get("secret").is_none_or(|s| s.is_empty()),
            "external platform authentication is not implemented"
        );
        let password = form.get("password").context("password required")?.clone();
        let _authentication = a.authentication.read().await;
        let registered = !password.is_empty();
        let account = a.store.characters(guid, password).await?;
        if registered && let Some(browser) = browser.as_mut() {
            browser.authenticate(a.store.browser_identity(account.id).await?);
        }
        let feed = a.store.updates().await?;
        let xml = account.xml(&feed.news_xml(&a.origin), a.server_usage()?);
        Ok((xml, browser.and_then(|b| b.cookie)))
    }
    .await;
    match result {
        Ok((xml, cookie)) => browser_link::with_cookie(
            &a,
            ([("content-type", "application/xml")], xml).into_response(),
            cookie,
        ),
        Err(e) if e.is::<crate::store::InvalidCredentials>() => {
            tracing::info!("legacy character-list credentials rejected");
            (
                [("content-type", "application/xml")],
                "<Error>Account credentials not valid</Error>",
            )
                .into_response()
        }
        Err(e) => error(e),
    }
}
async fn delete_character(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    match admit(&a, &headers, peer) {
        Ok(true) => {}
        Ok(false) => return limited(),
        Err(e) => return error(e),
    }
    let result: Result<()> = async {
        let character: i32 = form
            .get("charId")
            .context("charId required")?
            .parse()
            .context("invalid charId")?;
        ensure!(character > 0, "invalid charId");
        ensure!(
            form.get("secret").is_none_or(|s| s.is_empty()),
            "external platform authentication is not implemented"
        );
        let guid = form.get("guid").context("guid required")?.clone();
        let password = form.get("password").context("password required")?.clone();
        let _authentication = a.authentication.read().await;
        let account = a.store.authenticate(guid, password).await?;
        let _claim = a.claim(account)?;
        lifecycle::account(&a, vec![account], "delete_character", move |a| async move {
            lifecycle::stage(Stage::Store);
            a.store.delete_character(account, character).await
        })
        .await
    }
    .await;
    let xml = match result {
        Ok(()) => "<Success/>".into(),
        Err(e) => {
            tracing::warn!(error=%e, "character deletion rejected");
            let message = if e.is::<crate::store::InvalidCredentials>() {
                "Account credentials not valid".into()
            } else {
                e.to_string()
            };
            format!("<Error>{}</Error>", xml_escape(&message))
        }
    };
    (
        [
            ("content-type", "application/xml"),
            ("cache-control", "no-store"),
        ],
        xml,
    )
        .into_response()
}
async fn account_request(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    axum::extract::Path(operation): axum::extract::Path<String>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    match admit(&a, &headers, peer) {
        Ok(true) => {}
        Ok(false) => return limited(),
        Err(e) => return error(e),
    }
    let result: Result<i32> = async {
        let guid = form.get("guid").context("guid required")?.clone();
        if operation == "changePassword" {
            ensure!(
                form.get("secret").is_none_or(|s| s.is_empty()),
                "external platform authentication is not implemented"
            );
            let password = form.get("password").context("password required")?.clone();
            ensure!(!password.is_empty(), crate::store::InvalidCredentials);
            let verifier = a
                .store
                .prepare_password(
                    form.get("newPassword")
                        .context("newPassword required")?
                        .clone(),
                )
                .await?;
            // Fence the current verifier through ownership and commit, without upgrading a read lock.
            let authentication = a.authentication.clone().write_owned().await;
            let id = a.store.authenticate(guid, password).await?;
            let _claim = a.claim(id)?;
            lifecycle::account(&a, vec![id], "change_password", move |a| async move {
                let _authentication = authentication;
                lifecycle::stage(Stage::Store);
                a.store.change_password(id, verifier).await
            })
            .await?;
            return Ok(id);
        }
        let _authentication = a.authentication.read().await;
        let id = match operation.as_str() {
            "register" => {
                a.store
                    .register(
                        guid,
                        form.get("newGUID").context("newGUID required")?.clone(),
                        form.get("newPassword")
                            .context("newPassword required")?
                            .clone(),
                    )
                    .await?
            }
            "verify" => {
                let password = form.get("password").context("password required")?.clone();
                ensure!(!password.is_empty(), "email and password required");
                a.store.authenticate(guid, password).await?
            }
            "setName" => {
                let id = a
                    .store
                    .authenticate(
                        guid,
                        form.get("password").context("password required")?.clone(),
                    )
                    .await?;
                let _claim = a.claim(id)?;
                let name = form.get("name").context("name required")?.clone();
                lifecycle::account(&a, vec![id], "choose_name", move |a| async move {
                    lifecycle::stage(Stage::Store);
                    a.store.choose_name(id, name).await
                })
                .await?;
                id
            }
            "purchaseCharSlot" => {
                ensure!(
                    form.get("secret").is_none_or(|s| s.is_empty()),
                    "external platform authentication is not implemented"
                );
                let timer: i32 = form
                    .get("ignore")
                    .context("ignore timer required")?
                    .parse()
                    .context("invalid signed-32-bit ignore timer")?;
                let password = form.get("password").context("password required")?.clone();
                ensure!(
                    !password.is_empty(),
                    "Register an account before buying character slots."
                );
                let id = a.store.authenticate(guid, password).await?;
                let _claim = a.claim(id)?;
                lifecycle::account(
                    &a,
                    vec![id],
                    "purchase_character_slot",
                    move |a| async move {
                        lifecycle::stage(Stage::Store);
                        a.store.purchase_character_slot(id, timer).await
                    },
                )
                .await?;
                id
            }
            _ => bail!("account operation not implemented"),
        };
        Ok(id)
    }
    .await;
    let xml = match result {
        Ok(id) => {
            tracing::info!(account = id, operation, "account request completed");
            "<Success/>".into()
        }
        Err(e) => {
            tracing::warn!(error=%e, operation, "account request rejected");
            let tag = if e.is::<crate::store::SlotReplay>() {
                "FatalError"
            } else {
                "Error"
            };
            format!("<{tag}>{}</{tag}>", xml_escape(&e.to_string()))
        }
    };
    // The original account service carries semantic failures in XML, not HTTP error pages.
    (
        [
            ("content-type", "application/xml"),
            ("cache-control", "no-store"),
        ],
        xml,
    )
        .into_response()
}
async fn legends(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    match admit(&a, &headers, peer) {
        Ok(true) => {}
        Ok(false) => return limited(),
        Err(e) => return error(e),
    }
    let result: Result<String> = async {
        let span = form.get("timespan").context("timespan required")?.clone();
        let account = form
            .get("accountId")
            .context("accountId required")?
            .parse()?;
        let character = form.get("charId").context("charId required")?.parse()?;
        a.store.legends(span, account, character).await
    }
    .await;
    match result {
        Ok(xml) => (
            [
                ("content-type", "application/xml"),
                ("cache-control", "no-store"),
            ],
            xml,
        )
            .into_response(),
        Err(e) => error(e),
    }
}
async fn fame(State(a): State<App>, Form(form): Form<HashMap<String, String>>) -> Response {
    let result: Result<String> = async {
        let account = form
            .get("accountId")
            .context("accountId required")?
            .parse()?;
        let character = form.get("charId").context("charId required")?.parse()?;
        a.store.fame(account, character).await
    }
    .await;
    match result {
        Ok(xml) => ([("content-type", "application/xml")], xml).into_response(),
        Err(e) => error(e),
    }
}
async fn swf(State(a): State<App>) -> Response {
    (
        [
            ("content-type", "application/x-shockwave-flash"),
            ("cache-control", "no-store"),
        ],
        a.swf.as_ref().clone(),
    )
        .into_response()
}
async fn critical_incident(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = hiscores::private_denial(&a, peer, &headers) {
        return response;
    }
    match a.world.supervisor.latest() {
        // The durable record also contains private account fences.
        Ok(incident) => Json(serde_json::json!({"incident":incident.map(|i| serde_json::json!({
            "id":i.id,"unix_ms":i.unix_ms,"process":i.process,"revision":i.revision,
            "component":i.component,"failure":i.failure,"location":i.location,"context":i.context,
            "fences_preserved":i.fences_preserved,
        }))})).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, self::error(error)).into_response(),
    }
}
async fn health(State(a): State<App>) -> Response {
    let started = a.started;
    match a.world.directory(move |d| Ok(d.health(started))).await {
        Ok(mut v) => {
            v["bot_dialogue"] = crate::bots::dialogue::health(a.dialogue.as_deref());
            Json(v).into_response()
        }
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE, error(e)).into_response(),
    }
}
async fn websocket(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<DiagnosticQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let origin = headers.get("origin").and_then(|v| v.to_str().ok());
    if origin != Some(a.origin.as_str()) {
        tracing::warn!(?origin, expected=%a.origin, "unapproved WebSocket origin");
        return (
            StatusCode::FORBIDDEN,
            "unapproved WebSocket origin; inspect the server log for received and expected origins",
        )
            .into_response();
    }
    match admit(&a, &headers, peer) {
        Ok(true) => {}
        Ok(false) => return limited(),
        Err(e) => return error(e),
    }
    let Ok(permit) = a.connections.clone().try_acquire_owned() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "game connection limit reached",
        )
            .into_response();
    };
    let browser = match query
        .diagnostics
        .as_deref()
        .map(|key| a.diagnostics.browser(key))
        .transpose()
    {
        Ok(browser) => browser,
        Err(e) => return error(e),
    };
    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().take(512).collect());
    let entry = match lifecycle::drain::entry(&a) {
        Ok(entry) => entry,
        Err(error) => return hiscores::json_error(StatusCode::SERVICE_UNAVAILABLE, error),
    };
    ws.max_message_size(1_048_576).on_upgrade(move |socket| {
        entry.scope(async move {
            let _permit = permit;
            connection(socket, a, user_agent, browser).await
        })
    })
}
#[derive(Deserialize)]
struct DiagnosticQuery {
    diagnostics: Option<String>,
}
fn fail(description: &str) -> Packet {
    let mut o = Writer::new();
    o.i32(0).utf(description);
    Packet { id: 0, body: o.0 }
}
struct NativeReader<'a>(Reader<'a>);
impl<'a> NativeReader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        self.0.take(n).context(CloseReason::Protocol)
    }
    fn done(&self) -> Result<()> {
        self.0.done().context(CloseReason::Protocol)
    }
}
macro_rules! native_readers {
    ($($method:ident: $ty:ty),*) => { impl NativeReader<'_> { $(fn $method(&mut self) -> Result<$ty> { self.0.$method().context(CloseReason::Protocol) })* } };
}
native_readers!(u8: u8, i16: i16, i32: i32, f32: f32, utf: String);
impl<'a> std::ops::Deref for NativeReader<'a> {
    type Target = Reader<'a>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for NativeReader<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

fn slot(r: &mut Reader<'_>) -> Result<Slot> {
    Ok(Slot {
        object: r.i32()?,
        slot: r.u8()? as usize,
        kind: r.i16()? as i32,
    })
}
async fn persist(a: &App, plan: InventoryPlan) -> Result<()> {
    lifecycle::stage(Stage::Store);
    if let Some(purchase) = &plan.vault_purchase {
        let _gold = a.gold_updates.lock().await;
        let result = a
            .store
            .purchase_vault(
                InventoryWrite {
                    id: plan.character,
                    account: plan.account,
                    revision: plan.revision,
                    items: plan.items.clone(),
                },
                purchase.chest,
            )
            .await;
        lifecycle::stage(Stage::Publication);
        a.world
            .for_player(plan.player)
            .call(move |w| w.finish_vault_purchase(plan, result))
            .await?;
        lifecycle::stage(Stage::Settled);
        return Ok(());
    }
    if let Some((_, purchase)) = &plan.purchase {
        let _gold = a.gold_updates.lock().await;
        let result = a
            .store
            .purchase(
                InventoryWrite {
                    id: plan.character,
                    account: plan.account,
                    revision: plan.revision,
                    items: plan.items.clone(),
                },
                purchase.clone(),
            )
            .await;
        lifecycle::stage(Stage::Publication);
        a.world
            .for_player(plan.player)
            .call(move |w| w.finish_purchase(plan, result))
            .await?;
        lifecycle::stage(Stage::Settled);
        return Ok(());
    }
    lifecycle::stage(Stage::Reservation);
    let plan = a.world.directory(move |d| d.stage_unlock(plan)).await?;
    lifecycle::stage(Stage::Store);
    let collector = plan.collected.map(|_| plan.account);
    let result = if let Some(pet) = plan.companion.as_ref().filter(|pet| pet.permanent) {
        a.store
            .select_companion(
                InventoryWrite {
                    id: plan.character,
                    account: plan.account,
                    revision: plan.revision,
                    items: plan.items.clone(),
                },
                pet.source.slot,
                u16::try_from(pet.source.kind)?,
                pet.selected,
                a.content.clone(),
            )
            .await
    } else if let Some(slot) = plan.xp_boost {
        a.store
            .consume_xp_booster(
                InventoryWrite {
                    id: plan.character,
                    account: plan.account,
                    revision: plan.revision,
                    items: plan.items.clone(),
                },
                (slot.object == plan.player).then_some(slot.slot),
                plan.vaults.clone(),
            )
            .await
    } else if let Some((slot, tex1, tex2)) = plan.appearance {
        let write = InventoryWrite {
            id: plan.character,
            account: plan.account,
            revision: plan.revision,
            items: plan.items.clone(),
        };
        let kind = u16::try_from(slot.kind)?;
        if slot.object == plan.player {
            a.store
                .consume_dye(write, slot.slot, kind, [tex1, tex2])
                .await
        } else {
            let reserved = plan.clone();
            match a
                .world
                .for_player(plan.player)
                .call(move |w| w.validate_ground_dye(&reserved))
                .await
            {
                Ok(()) => a.store.consume_ground_dye(write, kind, [tex1, tex2]).await,
                Err(error) => Err(error),
            }
        }
    } else if let Some((snapshot, slot, index, amount)) = &plan.stat_growth {
        a.store
            .consume_stat(
                snapshot.clone(),
                (slot.object == plan.player).then_some(slot.slot),
                *index,
                *amount,
                plan.vaults.clone(),
            )
            .await
    } else if let Some(source) = plan.potion {
        ensure!(
            source.object == plan.player
                && plan.bags.is_empty()
                && plan.vaults.is_empty()
                && plan.collected.is_none(),
            "inventory potion source must be the consuming actor"
        );
        a.store
            .consume_inventory_potion(
                InventoryWrite {
                    id: plan.character,
                    account: plan.account,
                    revision: plan.revision,
                    items: plan.items.clone(),
                },
                source.slot,
                u16::try_from(source.kind)?,
            )
            .await
    } else {
        a.store
            .exchange_collected(
                vec![InventoryWrite {
                    id: plan.character,
                    account: plan.account,
                    revision: plan.revision,
                    items: plan.items.clone(),
                }],
                plan.vaults.clone(),
                plan.collected,
            )
            .await
            .and_then(|mut revisions| {
                revisions
                    .pop()
                    .context("inventory commit returned no revision")
            })
    };
    lifecycle::stage(Stage::Publication);
    a.world
        .directory(move |d| d.finish_inventory(plan, result))
        .await?;
    if let Some(account) = collector {
        gold::sync(a, account).await?;
    }
    lifecycle::stage(Stage::Settled);
    Ok(())
}
pub(crate) async fn inventory(
    a: &App,
    id: i32,
    prepare: impl FnOnce(&mut World) -> Result<Option<InventoryPlan>> + Send + 'static,
) -> Result<Result<bool>> {
    lifecycle::player(a, id, "inventory", move |a| async move {
        lifecycle::stage(Stage::Reservation);
        let prepared = a.world.for_player(id).call(move |w| Ok(prepare(w))).await?;
        match prepared {
            Ok(Some(plan)) => {
                persist(&a, plan).await?;
                Ok(Ok(true))
            }
            Ok(None) => Ok(Ok(false)),
            Err(error) if lifecycle::rule_rejection(&error) => {
                lifecycle::stage(Stage::Rejected);
                Ok(Err(error))
            }
            Err(error) => Err(error),
        }
    })
    .await
}

enum CreditStep {
    Done,
    Waiting(std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>),
    Progress(Box<(lifecycle::Operation, i32, crate::store::Character)>),
}
async fn settle_giver_credits(a: &App, id: i32) -> Result<()> {
    let account = a
        .world
        .for_player(id)
        .call(move |w| {
            Ok(w.players
                .get(&id)
                .context("credit giver missing")?
                .character
                .account)
        })
        .await?;
    let failure = lifecycle::failure_reporter(a, &[account], "causal_giver")?;
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let app = a.clone();
            let step = a
                .world
                .directory(move |d| {
                    let origin = d.location(id)?;
                    let p = &d.worlds[&origin].players[&id];
                    ensure!(
                        p.departing || p.death_pending,
                        "causal giver has not sealed terminal authority"
                    );
                    let giver = (p.character.account, id);
                    let character = p.character.id;
                    let recipients: Vec<_> = d
                        .worlds
                        .iter()
                        .flat_map(|(scope, w)| {
                            w.credit_recipients(character)
                                .into_iter()
                                .map(move |(recipient, account)| (*scope, recipient, account))
                        })
                        .collect();
                    let Some((scope, recipient, account)) = recipients.first().copied() else {
                        return Ok(CreditStep::Done);
                    };
                    match lifecycle::reserve_credit_recipient(&app, giver, (account, recipient))? {
                        lifecycle::CreditAdmission::Waiting(wait) => Ok(CreditStep::Waiting(wait)),
                        lifecycle::CreditAdmission::Reserved(guard) => {
                            let snapshot = d
                                .worlds
                                .get_mut(&scope)
                                .unwrap()
                                .pending_progress_where(|player, _| player == recipient)
                                .pop()
                                .context(
                                    "causal recipient has no admissible progress; source retained",
                                )?
                                .1;
                            Ok(CreditStep::Progress(Box::new((
                                *guard, recipient, snapshot,
                            ))))
                        }
                    }
                })
                .await?;
            match step {
                CreditStep::Done => return Ok(()),
                CreditStep::Waiting(wait) => wait.await,
                CreditStep::Progress(progress) => {
                    let (guard, recipient, snapshot) = *progress;
                    lifecycle::complete(guard, move |a| async move {
                        persist_progress_inner(&a, recipient, snapshot).await
                    })
                    .await?;
                }
            }
        }
    })
    .await
    .context("causal recipient completion deadline exceeded; sources retained")
    .and_then(|r| r);
    if let Err(error) = &result {
        failure(format!("{error:#}"));
    }
    result
}
async fn refresh_giver_credits(a: &App, id: i32) -> Result<()> {
    let character = a
        .world
        .for_player(id)
        .call(move |w| Ok(w.players[&id].character.id))
        .await?;
    let count = a.store.assist_count(character).await?;
    a.world
        .for_player(id)
        .call(move |w| w.refresh_giver_credits(id, count))
        .await
}

async fn persist_progress_inner(a: &App, id: i32, snapshot: crate::store::Character) -> Result<()> {
    let account = snapshot.account;
    let reservation = snapshot.clone();
    let assists = a
        .world
        .for_player(id)
        .call(move |w| w.progress_assists(id, &reservation))
        .await?;
    lifecycle::stage(Stage::Store);
    let result = a
        .store
        .progress_with_assists(snapshot.clone(), assists)
        .await
        .context("progress persistence failed");
    lifecycle::stage(Stage::Publication);
    let crossed_rank = a
        .world
        .for_player(id)
        .call(move |w| {
            let rank = w.class_quest_rank(id)?;
            w.committed(id, result)?;
            w.finish_progress(id, snapshot)?;
            Ok(w.class_quest_rank(id)? > rank)
        })
        .await?;
    if crossed_rank {
        gold::sync(a, account).await?;
    }
    Ok(())
}

async fn persist_death_inner(
    a: &App,
    id: i32,
    ch: crate::store::Character,
    killer: String,
) -> Result<()> {
    lifecycle::stage(Stage::Store);
    let slot = ch
        .items
        .iter()
        .take(4)
        .position(|kind| *kind >= 0 && a.content.objects[&(*kind as u16)].resurrects);
    if let Some(slot) = slot {
        let result = a
            .store
            .resurrect(ch, slot, a.content.clone())
            .await
            .context("resurrection persistence failed");
        lifecycle::stage(Stage::Publication);
        a.world
            .directory(move |d| d.finish_resurrection(id, result))
            .await
    } else {
        let result = a
            .store
            .death(ch, killer)
            .await
            .context("death persistence failed");
        lifecycle::stage(Stage::Publication);
        a.world
            .for_player(id)
            .call(move |w| {
                w.committed(id, result)?;
                w.finish_death(id)
            })
            .await
    }
}
pub(crate) async fn persist_trade(a: &App, plan: TradePlan) -> Result<()> {
    let report_failure = lifecycle::failure_reporter(
        a,
        &plan.plans.iter().map(|p| p.account).collect::<Vec<_>>(),
        "trade",
    )?;
    let result = std::panic::AssertUnwindSafe(async {
        lifecycle::stage(Stage::Store);
        let id = plan.plans[0].player;
        let writes = plan
            .plans
            .iter()
            .map(|p| InventoryWrite {
                id: p.character,
                account: p.account,
                revision: p.revision,
                items: p.items.clone(),
            })
            .collect();
        let result = a
            .store
            .exchange_trade(plan.transaction.clone(), writes, plan.delivery.clone())
            .await;
        lifecycle::stage(Stage::Publication);
        a.world
            .for_player(id)
            .call(move |w| w.finish_trade(plan, result))
            .await
    })
    .catch_unwind()
    .await;
    match result {
        Ok(Ok(())) => {
            lifecycle::stage(Stage::Settled);
            Ok(())
        }
        Ok(Err(error)) => {
            report_failure(format!("{error:#}"));
            Err(error)
        }
        Err(panic) => {
            report_failure("trade completion task panicked".into());
            std::panic::resume_unwind(panic)
        }
    }
}

fn trade_offer(r: &mut Reader<'_>) -> Result<[bool; 12]> {
    ensure!(r.i16()? == 12, "trade offer must contain twelve slots");
    let mut values = [false; 12];
    for value in &mut values {
        let raw = r.u8()?;
        ensure!(raw <= 1, "invalid trade offer boolean");
        *value = raw == 1;
    }
    Ok(values)
}
fn player_text(r: &mut Reader<'_>) -> Result<String> {
    let text = r.utf()?;
    r.done()?;
    Ok(text)
}
struct PreparedTransfer {
    packet: Packet,
    reservation: crate::world::TransferReservation,
}
#[derive(Default)]
struct Session {
    account: Option<Owner>,
    player: Option<i32>,
    expected: Option<i32>,
    arrival: Option<crate::world::TransferArrival>,
    admission: Option<Vec<u8>>,
    reconnecting: bool,
    transfer: Option<PreparedTransfer>,
    runtime: Option<tokio::runtime::Handle>,
    diagnostics: diagnostics::ConnectionDiagnostics,
}
fn collision_report(other: bool, r: &mut Reader<'_>) -> Result<()> {
    // Reports cannot grant hits: the actor advances and resolves projectiles itself.
    r.i32()?;
    r.u8()?;
    r.i32()?;
    if other {
        r.i32()?;
    }
    r.done()
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplyQueueError {
    Full,
    Closed,
}
impl std::fmt::Display for ReplyQueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Full => "connection reply queue is full",
            Self::Closed => "connection reply queue is closed",
        })
    }
}
impl std::error::Error for ReplyQueueError {}

fn enqueue_reply(tx: &mpsc::Sender<Packet>, packet: Packet) -> Result<()> {
    // This handler's caller is also the only consumer, so waiting for capacity deadlocks.
    tx.try_send(packet).map_err(|error| {
        match error {
            mpsc::error::TrySendError::Full(_) => ReplyQueueError::Full,
            mpsc::error::TrySendError::Closed(_) => ReplyQueueError::Closed,
        }
        .into()
    })
}

// Packets whose only effect is on the sender's World; a queued run of them shares one world job.
const INPUTS: [u8; 12] = [7, 11, 14, 22, 24, 38, 48, 51, 59, 64, 66, 76];
type Input = Box<dyn FnOnce(&mut World) -> Result<()> + Send>;

fn input(
    packet: &Packet,
    player: Option<i32>,
    received: std::time::Instant,
    testing: bool,
) -> Result<Option<Input>> {
    let mut r = NativeReader(Reader::new(&packet.body));
    let character = |what| player.ok_or_else(|| lifecycle::protocol(what));
    let input: Input = match packet.id {
        wire::UPDATE_ACK => {
            r.done()?;
            let Some(id) = player.filter(|_| testing) else {
                return Ok(None);
            };
            Box::new(move |w| {
                w.players
                    .get_mut(&id)
                    .context("native UPDATE_ACK player missing")?
                    .native_update_acked = true;
                Ok(())
            })
        }
        wire::MOVE => {
            let tick = r.i32()?;
            let time = r.i32()?;
            let pos = Pos {
                x: r.f32()?,
                y: r.f32()?,
            };
            let count = r.i16()?;
            ensure!(
                (0..=128).contains(&count),
                lifecycle::protocol("movement history too long")
            );
            for _ in 0..count {
                r.i32()?;
                r.f32()?;
                r.f32()?;
            }
            r.done()?;
            let id = character("MOVE before character")?;
            Box::new(move |w| {
                w.acknowledge_tick(id, tick)?;
                match w.native_move_received(id, pos, (time, received)) {
                    Ok(_) => Ok(()),
                    Err(e) if lifecycle::rule_rejection(&e) => {
                        w.correct_position(id, time, &e.to_string())
                    }
                    Err(error) => Err(error),
                }
            })
        }
        wire::PLAYER_SHOOT => {
            let time = r.i32()?;
            let bullet = r.u8()?;
            let kind = r.i16()? as u16;
            let pos = Pos {
                x: r.f32()?,
                y: r.f32()?,
            };
            let angle = r.f32()?;
            r.done()?;
            let id = character("shoot before character")?;
            Box::new(move |w| {
                match w.native_shoot_received(id, (time, received), bullet, kind, pos, angle) {
                    Ok(_) => Ok(()),
                    Err(error)
                        if error
                            .downcast_ref::<crate::world::RejectedNativeShot>()
                            .is_some() =>
                    {
                        w.correct_native_shot(id, time, bullet, kind, &format!("{error:#}"))
                    }
                    Err(error) => Err(error),
                }
            })
        }
        64 => {
            let time = r.i32()?;
            let pos = Pos {
                x: r.f32()?,
                y: r.f32()?,
            };
            r.done()?;
            let id = character("ground damage before character")?;
            Box::new(move |w| w.ground_damage(id, time, pos))
        }
        22 => {
            let time = r.i32()?;
            r.done()?;
            let id = character("SHOOT ACK before character")?;
            Box::new(move |w| w.acknowledge_shoot(id, time))
        }
        24 => {
            let bullet = r.u8()?;
            let owner = r.i32()?;
            r.done()?;
            let id = character("hit report before character")?;
            Box::new(move |w| w.native_player_hit(id, owner, bullet))
        }
        14 => {
            let time = r.i32()?;
            r.done()?;
            let id = character("GOTO ACK before character")?;
            Box::new(move |w| w.acknowledge_goto(id, time))
        }
        59 => {
            character("area acknowledgement before character")?;
            r.i32()?;
            r.f32()?;
            r.f32()?;
            r.done()?;
            return Ok(None);
        }
        66 | 51 => {
            collision_report(packet.id == 66, &mut r).context(CloseReason::Protocol)?;
            return Ok(None);
        }
        76 => {
            r.i32()?;
            let bullet = r.u8()?;
            let target = r.i32()?;
            let killed = r.u8()?;
            r.done()?;
            ensure!(
                killed <= 1,
                lifecycle::protocol("invalid predicted enemy death flag")
            );
            let id = character("enemy hit report before character")?;
            if killed == 0 {
                return Ok(None);
            }
            Box::new(move |w| w.native_predicted_enemy_death(id, bullet, target))
        }
        48 => {
            r.done()?;
            return Ok(None);
        }
        id => unreachable!("packet {id} is not native input"),
    };
    Ok(Some(input))
}

async fn handle_inputs(
    a: &App,
    packets: Vec<(Packet, std::time::Instant)>,
    session: &Session,
) -> Result<()> {
    let testing = a.testing.is_some();
    let inputs = packets
        .iter()
        .map(|(packet, received)| {
            Ok((
                packet.id,
                input(packet, session.player, *received, testing)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let Some(id) = session.player else {
        return Ok(());
    };
    a.world
        .call(move |w| {
            for (packet, input) in inputs {
                let p = w.players.get(&id).context("player missing")?;
                // Resurrection is alive again, but its old scene must stay inert until the owned RECONNECT.
                if p.native_transferring()
                    || matches!(packet, 7 | 38) && (p.death_pending || !p.character.alive)
                {
                    tracing::debug!(
                        player = id,
                        packet,
                        "ignored queued command from dying or transferring character"
                    );
                } else if let Some(input) = input {
                    input(w)?;
                }
            }
            Ok(())
        })
        .await
}

async fn handle(
    a: &mut App,
    packets: Vec<(Packet, std::time::Instant)>,
    session: &mut Session,
    tx: &mpsc::Sender<Packet>,
    lost: &tokio::sync::watch::Sender<Option<&'static str>>,
) -> Result<()> {
    let entry = lifecycle::drain::entry(a)?;
    if let (Some(owner), Some(player)) = (&session.account, session.player) {
        owner.bind(player)?;
    }
    let mut owned_session = std::mem::take(session);
    let reply = tx.clone();
    let tx = tx.clone();
    let lost = lost.clone();
    let accounts = owned_session
        .account
        .as_ref()
        .map(|owner| vec![owner.id])
        .unwrap_or_default();
    let (app, completed_session, result) =
        lifecycle::account(a, accounts, "native_packet", move |mut app| async move {
            let result = if INPUTS.contains(&packets[0].0.id) {
                entry
                    .scope(handle_inputs(&app, packets, &owned_session))
                    .await
            } else {
                let Ok([(packet, received)]) = <[_; 1]>::try_from(packets) else {
                    unreachable!("only native input is batched")
                };
                entry
                    .scope(handle_inner(
                        &mut app,
                        packet,
                        &mut owned_session,
                        &tx,
                        &lost,
                        received,
                    ))
                    .await
            };
            if let (Err(error), Some(owner)) = (&result, &owned_session.account) {
                lifecycle::observe(&app, owner.id, "native_packet", error);
            }
            Ok((app, owned_session, result))
        })
        .await?;
    *a = app;
    *session = completed_session;
    result?;
    if let Some(transfer) = session.transfer.take() {
        let id = session
            .player
            .context("prepared transfer lost its avatar")?;
        lifecycle::finalize_transfer(a, id, transfer.reservation)?
            .wait()
            .await?;
        session.player = None;
        // Admission must be released before RECONNECT can reach an immediate replacement HELLO.
        session
            .account
            .take()
            .context("transfer ownership missing")?
            .release();
        enqueue_reply(&reply, transfer.packet)?;
        session.reconnecting = true;
    }
    Ok(())
}

async fn handle_inner(
    a: &mut App,
    packet: Packet,
    session: &mut Session,
    tx: &mpsc::Sender<Packet>,
    lost: &tokio::sync::watch::Sender<Option<&'static str>>,
    received: std::time::Instant,
) -> Result<()> {
    let Session {
        account,
        player,
        expected,
        arrival,
        admission,
        reconnecting,
        transfer: _,
        runtime,
        diagnostics,
    } = session;
    if *reconnecting {
        tracing::debug!(
            packet = packet.id,
            "discarded in-flight input after durable departure"
        );
        return Ok(());
    }
    // Resurrection is alive again, but its old scene must stay inert until the owned RECONNECT.
    if let Some(id) = *player
        && a.world
            .call(move |w| {
                let p = w.players.get(&id).context("player missing")?;
                Ok(p.native_transferring()
                    || matches!(packet.id, 7 | 38 | 30 | 65 | 33 | 42 | 3)
                        && (p.death_pending || !p.character.alive))
            })
            .await?
    {
        tracing::debug!(
            player = id,
            packet = packet.id,
            "ignored queued command from dying or transferring character"
        );
        return Ok(());
    }
    macro_rules! ensure_protocol {
        ($condition:expr, $($detail:tt)*) => { ensure!($condition, anyhow::anyhow!($($detail)*).context(CloseReason::Protocol)); };
    }
    let mut r = NativeReader(Reader::new(&packet.body));
    if packet.id == wire::BUY {
        let mut shape = NativeReader(Reader::new(&packet.body));
        shape.i32()?;
        shape.done()?;
    }
    match packet.id {
        wire::HELLO => {
            ensure_protocol!(account.is_none(), "duplicate HELLO");
            let version = r.utf()?;
            ensure_protocol!(version == "123.5.1", "unsupported client build {version}");
            let game = r.i32()?;
            let guid = a.keys.decrypt(&r.utf()?).context(CloseReason::Protocol)?;
            let password = a.keys.decrypt(&r.utf()?).context(CloseReason::Protocol)?;
            let secret = a.keys.decrypt(&r.utf()?).context(CloseReason::Protocol)?;
            ensure_protocol!(
                secret.is_empty(),
                "external platform authentication is not implemented"
            );
            let keytime = r.i32()?;
            let n = usize::try_from(r.i16()?).context(CloseReason::Protocol)?;
            ensure_protocol!(n == 0 || n == 16, "invalid transfer key size");
            let key = r.take(n)?.to_vec();
            ensure_protocol!(r.i32()? == 0, "map editor JSON not supported");
            for _ in 0..4 {
                r.utf()?;
            }
            r.done()?;
            let authentication = a.authentication.read().await;
            let registered = !password.is_empty();
            let id = a.store.authenticate(guid, password).await?;
            let mut owner = a.claim_session(id)?;
            if let Some(runtime) = runtime {
                owner.arm_cleanup(a, runtime.clone());
            }
            owner.registered = registered;
            *account = Some(owner);
            drop(authentication);
            lifecycle::account(a, vec![id], "login", move |a| async move {
                lifecycle::stage(Stage::Store);
                a.store.compete(id).await
            })
            .await?;
            let prepared = a
                .testing
                .as_ref()
                .map(|t| t.native_entry(id))
                .transpose()?
                .flatten();
            let transfer_key = (!key.is_empty()).then(|| key.clone());
            let (destination, character, transfer_arrival) = a
                .world
                .directory(move |d| {
                    if key.is_empty() {
                        if let Some((world, _)) = prepared {
                            ensure!(
                                d.worlds.contains_key(&world),
                                "prepared native world no longer exists"
                            );
                            Ok((world, None, None))
                        } else {
                            Ok((d.initial(game)?, None, None))
                        }
                    } else {
                        let (character, arrival) = d.consume(id, game, keytime, key)?;
                        Ok((game, Some(character), Some(arrival)))
                    }
                })
                .await?;
            *expected = character;
            *arrival = transfer_arrival;
            *admission = transfer_key;
            diagnostics.world = Some(destination);
            a.world = a.world.in_world(destination);
            let body = a.world.call(|w| Ok(w.map_info())).await?;
            enqueue_reply(
                tx,
                Packet {
                    id: wire::MAP_INFO,
                    body,
                },
            )?;
            tracing::info!(
                account = id,
                requested_world = game,
                actual_world = destination,
                "guest authenticated"
            );
        }
        wire::CREATE | wire::LOAD => {
            let _guild = a.guild_actions.lock().await;
            ensure_protocol!(player.is_none(), "character already loaded");
            let account = account
                .as_ref()
                .ok_or_else(|| lifecycle::protocol("HELLO required before character selection"))?
                .id;
            let ch = if packet.id == wire::CREATE {
                ensure_protocol!(
                    expected.is_none(),
                    "transfer requires LOAD of the existing character"
                );
                let kind = r.i16()? as u16;
                r.done()?;
                let class = a.content.get(kind).context(CloseReason::Protocol)?;
                ensure_protocol!(class.class == "Player", "unknown playable class");
                lifecycle::stage(Stage::Store);
                a.store.create(account, class).await?
            } else {
                let id = r.i32()?;
                r.done()?;
                ensure_protocol!(
                    expected.is_none_or(|bound| bound == id),
                    "transfer character mismatch"
                );
                a.store.load(account, id).await.map_err(|error| {
                    if matches!(
                        error.downcast_ref::<rusqlite::Error>(),
                        Some(rusqlite::Error::QueryReturnedNoRows)
                    ) {
                        error.context(CloseReason::Protocol)
                    } else {
                        error
                    }
                })?
            };
            let output = crate::world::Output {
                packets: tx.clone(),
                lost: lost.clone(),
            };
            let vault = if a.world.call(|w| Ok(w.name == "Vault")).await? {
                Some(a.store.vaults(account).await?)
            } else {
                None
            };
            let lists = a.store.account_lists(account).await?;
            let presentation = a.store.presentation(account).await?;
            let prepared = a
                .testing
                .as_ref()
                .map(|t| t.native_entry(account))
                .transpose()?
                .flatten();
            let ticket = admission.clone();
            let bind = lifecycle::binding(a, account)?;
            let transfer_arrival = arrival.take();
            let destination = diagnostics.world.context("LOAD destination missing")?;
            lifecycle::stage(Stage::Publication);
            *player = Some(
                a.world
                    .directory(move |d| {
                        let entry_key = transfer_arrival
                            .as_ref()
                            .map(|_| ticket.clone().context("arrival admission key missing"))
                            .transpose()?;
                        if let Some(arrival) = transfer_arrival {
                            d.admit_transfer(arrival, account, ch.id, destination)?;
                        }
                        let w = d
                            .worlds
                            .get_mut(&destination)
                            .context("LOAD world missing")?;
                        if let Some(vault) = vault {
                            w.load_vaults(vault)?;
                        }
                        let id = w.enter_admitted(
                            ch,
                            Some(output),
                            prepared.map(|(_, pos)| pos),
                            presentation,
                            ticket,
                        )?;
                        bind(id)?;
                        w.sync_account_lists(id, Some(lists))?;
                        if let Some(key) = entry_key {
                            d.complete_dungeon_entry(&key, id)?;
                        }
                        Ok(id)
                    })
                    .await?,
            );
            if expected.is_none() {
                let id = player.context("guild sign-on player missing")?;
                a.world.directory(move |d| d.guild_sign_on(id)).await?;
            }
            gold::sync(a, account).await?;
            if let Some(testing) = &a.testing {
                testing.native_entered(account)?;
            }
        }
        wire::CREATE_GUILD
        | wire::GUILD_INVITE
        | wire::JOIN_GUILD
        | wire::CHANGE_GUILD_RANK
        | wire::GUILD_REMOVE => {
            let mut shape = NativeReader(Reader::new(&packet.body));
            shape.utf()?;
            if packet.id == wire::CHANGE_GUILD_RANK {
                shape.i32()?;
            }
            shape.done()?;
            let id = player.ok_or_else(|| lifecycle::protocol("guild action before character"))?;
            guilds::native(a, id, packet.id, &mut r, tx, received).await?;
        }
        50 if guilds::merchant(
            a,
            player.ok_or_else(|| lifecycle::protocol("purchase before character"))?,
            &packet.body,
        )
        .await? =>
        {
            let id =
                player.ok_or_else(|| lifecycle::protocol("guild purchase before character"))?;
            guilds::native(a, id, packet.id, &mut r, tx, received).await?;
        }
        wire::PLAYER_TEXT => {
            let text = player_text(&mut r).context(CloseReason::Protocol)?;
            if text.trim().eq_ignore_ascii_case("/tutorial") {
                crate::world::validate_chat(&text)?;
                return reconnect(a, session, Reconnect::Tutorial, received).await;
            }
            if let Some((kind, name)) = guilds::chat_command(&text) {
                let id =
                    player.ok_or_else(|| lifecycle::protocol("guild command before character"))?;
                let mut body = Writer::new();
                body.utf(name);
                guilds::native(a, id, kind, &mut Reader::new(&body.0), tx, received).await?;
                return Ok(());
            }
            let guild_chat = text
                .split_whitespace()
                .next()
                .is_some_and(|s| s.eq_ignore_ascii_case("/g") || s.eq_ignore_ascii_case("/guild"));
            // A revoked member must not receive chat between the database commit and cache publication.
            let _guild = if guild_chat {
                Some(a.guild_actions.lock().await)
            } else {
                None
            };
            let id = player.ok_or_else(|| lifecycle::protocol("chat before character"))?;

            if text.split_whitespace().next() == Some("/bug") {
                let message = text.trim()["/bug".len()..].trim();
                let owner = account
                    .as_ref()
                    .context("authenticated session required")?
                    .id;
                match diagnostics::report(a, owner, id, message, diagnostics, None).await {
                    Ok(_) => enqueue_reply(
                        tx,
                        crate::world::chat_info(
                            "Bug report submitted, but bug reporting has moved ~ please use \"Report Bug\" at the bottom of the website instead!",
                        ),
                    )?,
                    Err(error) => {
                        tracing::error!(account=owner, player=id, error=%error, "bug report failed");
                        enqueue_reply(
                            tx,
                            crate::world::chat_info(&format!("Could not save bug report: {error}")),
                        )?;
                    }
                }
            } else if matches!(
                text.trim(),
                "/discord" | "/discord recovery" | "/discord norecovery"
            ) {
                let result = async {
                    ensure!(a.discord_link_token.is_some(), lifecycle::controller("Discord linking is not configured"));
                    let owner = account.as_ref().context("authenticated session required")?;
                    ensure!(owner.registered, lifecycle::controller("sign into your registered account again before Discord account actions"));
                    let owner = owner.id;
                    lifecycle::stage(Stage::Store);
                    let reply = match text.trim() {
                        "/discord recovery" => {
                            let code = a.store.issue_recovery_code(owner).await?;
                            format!("Use /recover code:{code} with your linked Discord account within five minutes. This ENABLES that Discord account to reset your game password. Keep this code private. Use /discord norecovery to revoke permission.")
                        }
                        "/discord norecovery" => {
                            a.store.disable_recovery(owner).await?;
                            "Discord password recovery disabled. All recovery codes and reset links revoked.".into()
                        }
                        _ => "Use /link in Discord to link your game account.".into(),
                    };
                    lifecycle::stage(Stage::Settled);
                    Ok::<_, anyhow::Error>(reply)
                }.await;
                let reply = match result {
                    Ok(text) => text,
                    Err(error) if lifecycle::rule_rejection(&error) => {
                        lifecycle::stage(Stage::Rejected);
                        error.to_string()
                    }
                    Err(error) => return Err(error),
                };
                enqueue_reply(tx, crate::world::chat_private(&reply))?;
            } else if matches!(
                text.split_whitespace()
                    .next()
                    .map(str::to_ascii_lowercase)
                    .as_deref(),
                Some("/ignore" | "/unignore")
            ) {
                let result = async {
                    crate::world::validate_chat(&text)?;
                    a.world
                        .directory(move |d| d.chat_permit(id, received))
                        .await?;
                    let args: Vec<_> = text.split_whitespace().collect();
                    ensure!(
                        args.len() == 2,
                        lifecycle::controller("Usage: /ignore <player> or /unignore <player>")
                    );
                    let owner = account
                        .as_ref()
                        .context("authenticated session required")?
                        .id;
                    let target = a.store.account_named(args[1].into()).await?;
                    let add = args[0].eq_ignore_ascii_case("/ignore");
                    lifecycle::stage(Stage::Store);
                    let lists = a.store.edit_account_list(owner, 1, add, target).await?;
                    lifecycle::stage(Stage::Publication);
                    a.world
                        .call(move |w| w.sync_account_lists(id, Some(lists)))
                        .await?;
                    lifecycle::stage(Stage::Settled);
                    Ok::<_, anyhow::Error>(if add {
                        "Player ignored."
                    } else {
                        "Player unignored."
                    })
                }
                .await;
                let reply = match result {
                    Ok(text) => crate::world::chat_info(text),
                    Err(error) if lifecycle::rule_rejection(&error) => {
                        lifecycle::stage(Stage::Rejected);
                        crate::world::chat_error(&error.to_string())
                    }
                    Err(error) => return Err(error),
                };
                enqueue_reply(tx, reply)?;
            } else if text
                .split_whitespace()
                .next()
                .is_some_and(|s| s.eq_ignore_ascii_case("/swap"))
            {
                let result = async {
                    crate::world::validate_chat(&text)?;
                    a.world
                        .directory(move |d| d.chat_permit(id, received))
                        .await?;
                    let owner = account.as_ref().context("authenticated session required")?;
                    ensure!(
                        owner.registered,
                        lifecycle::controller("Register your account before swapping Fame.")
                    );
                    let fame = match text.split_whitespace().collect::<Vec<_>>()[..] {
                        [_, amount] => amount.parse::<i32>().ok().filter(|f| *f > 0 && f % 2 == 0),
                        _ => None,
                    }
                    .ok_or_else(|| {
                        lifecycle::controller(
                            "Usage: /swap <fame>. 2 Fame buys 1 Gold, so use an even amount.",
                        )
                    })?;
                    let account = owner.id;
                    let _gold = a.gold_updates.lock().await;
                    lifecycle::stage(Stage::Store);
                    let (fame_left, gold) = a.store.swap_fame(account, fame).await?;
                    tracing::info!(account, fame, fame_left, gold, "fame swapped for gold");
                    lifecycle::stage(Stage::Publication);
                    a.world
                        .for_player(id)
                        .call(move |w| {
                            let p = w.players.get_mut(&id).context("fame swap player missing")?;
                            (p.presentation.fame, p.presentation.credits) = (fame_left, gold);
                            Ok(())
                        })
                        .await?;
                    lifecycle::stage(Stage::Settled);
                    Ok::<_, anyhow::Error>(format!("Swapped {fame} Fame for {} Gold.", fame / 2))
                }
                .await;
                let reply = match result {
                    Ok(text) => crate::world::chat_info(&text),
                    Err(error) if lifecycle::rule_rejection(&error) => {
                        lifecycle::stage(Stage::Rejected);
                        crate::world::chat_error(&error.to_string())
                    }
                    Err(error) => return Err(error),
                };
                enqueue_reply(tx, reply)?;
            } else if let Err(error) = a.world.directory(move |d| d.chat(id, text, received)).await
            {
                if !lifecycle::rule_rejection(&error) {
                    return Err(error);
                }
                tracing::warn!(player=id, error=%error, "local chat rejected");
                enqueue_reply(tx, crate::world::chat_error(&error.to_string()))?;
            }
        }
        wire::EDIT_ACCOUNT_LIST => {
            let list = r.i32()?;
            let add = r.u8()?;
            ensure_protocol!(add <= 1, "invalid account list operation");
            let target = r.i32()?;
            r.done()?;
            let id =
                player.ok_or_else(|| lifecycle::protocol("account list edit before character"))?;
            let owner = account
                .as_ref()
                .context("authenticated session required")?
                .id;
            let result = async {
                ensure_protocol!((0..=1).contains(&list), "invalid account list");
                let target = a
                    .world
                    .call(move |w| w.account_list_target(id, target))
                    .await?;
                lifecycle::stage(Stage::Store);
                let lists = a
                    .store
                    .edit_account_list(owner, list, add != 0, target)
                    .await?;
                lifecycle::stage(Stage::Publication);
                a.world
                    .call(move |w| w.sync_account_lists(id, Some(lists)))
                    .await?;
                lifecycle::stage(Stage::Settled);
                Ok::<_, anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                if !lifecycle::rule_rejection(&error) {
                    return Err(error);
                }
                tracing::warn!(player=id, list, target, error=%error, "account list edit rejected");
                // The selected client only adds to ACCOUNTLIST 0; a fresh session clears an optimistic lock.
                if list == 0 && add != 0 {
                    return Err(
                        error.context("Lock change failed; reconnect to refresh your player list")
                    );
                }
                lifecycle::stage(Stage::Publication);
                a.world
                    .call(move |w| w.sync_account_lists(id, None))
                    .await?;
                lifecycle::stage(Stage::Settled);
                enqueue_reply(tx, crate::world::chat_error(&error.to_string()))?;
            }
        }
        wire::BUY => {
            let merchant = r.i32()?;
            r.done()?;
            let id = player.ok_or_else(|| lifecycle::protocol("purchase before character"))?;
            let registered = account
                .as_ref()
                .ok_or_else(|| lifecycle::protocol("purchase before HELLO"))?
                .registered;
            lifecycle::stage(Stage::Reservation);
            let plan = if registered {
                a.world.call(move |w| w.buy(id, merchant)).await
            } else {
                Err(lifecycle::controller(
                    "Register and sign in before buying items.",
                ))
            };
            match plan {
                Ok(plan) => persist(a, plan).await?,
                Err(error) if lifecycle::rule_rejection(&error) => {
                    lifecycle::stage(Stage::Rejected);
                    tracing::warn!(player=id, merchant, error=%error, "purchase rejected");
                    enqueue_reply(tx, crate::world::shop::reply(false, &error.to_string()))?;
                }
                Err(error) => return Err(error),
            }
        }
        wire::TELEPORT => {
            let target = r.i32()?;
            r.done()?;
            let id = player.ok_or_else(|| lifecycle::protocol("teleport before character"))?;
            if let Err(error) = a
                .world
                .directory(move |d| d.teleport(id, target, received))
                .await
            {
                if !lifecycle::rule_rejection(&error) {
                    return Err(error);
                }
                tracing::warn!(player=id, target, error=%error, "teleport rejected");
                enqueue_reply(tx, crate::world::chat_error(&error.to_string()))?;
            }
        }
        wire::PONG => {
            let nonce = r.i32()?;
            r.i32()?;
            r.done()?;
            diagnostics.pong(nonce);
            if let (Some(testing), Some(player)) = (&a.testing, player) {
                testing.acknowledge_native(nonce, *player);
            }
        }
        30 => {
            let time = r.i32()?;
            let s = slot(&mut r).context(CloseReason::Protocol)?;
            let pos = Pos {
                x: r.f32()?,
                y: r.f32()?,
            };
            r.done()?;
            let id = player.ok_or_else(|| lifecycle::protocol("item use before character"))?;
            lifecycle::stage(Stage::Reservation);
            match a
                .world
                .call(move |w| w.native_use_item(id, time, s, pos))
                .await
            {
                Ok(Some(plan)) => persist(a, plan).await?,
                Ok(None) => {}
                Err(error) if lifecycle::rule_rejection(&error) => {
                    lifecycle::stage(Stage::Rejected);
                    tracing::warn!(player=id, item=s.kind, slot=s.slot, error=%error, "item use rejected");
                    enqueue_reply(tx, crate::world::chat_error(&error.to_string()))?;
                    let mut out = Writer::new();
                    out.i32(1);
                    enqueue_reply(tx, Packet { id: 4, body: out.0 })?;
                }
                Err(error) => return Err(error),
            }
        }
        21 => {
            let name = r.utf()?;
            r.done()?;
            let id = player.ok_or_else(|| lifecycle::protocol("trade request before character"))?;
            a.world.call(move |w| w.request_trade(id, name)).await?;
        }
        1 => {
            r.done()?;
            let id = player.ok_or_else(|| lifecycle::protocol("trade cancel before character"))?;
            a.world
                .call(move |w| {
                    w.cancel_trade(id, "Trade cancelled.");
                    Ok(())
                })
                .await?;
        }
        37 => {
            let values = trade_offer(&mut r).context(CloseReason::Protocol)?;
            r.done()?;
            let id = player.ok_or_else(|| lifecycle::protocol("trade change before character"))?;
            a.world.call(move |w| w.change_trade(id, values)).await?;
        }
        57 => {
            let mine = trade_offer(&mut r).context(CloseReason::Protocol)?;
            let theirs = trade_offer(&mut r).context(CloseReason::Protocol)?;
            r.done()?;
            let id = player.ok_or_else(|| lifecycle::protocol("trade accept before character"))?;
            lifecycle::stage(Stage::Reservation);
            if let Some(plan) = a
                .world
                .call(move |w| w.accept_trade(id, mine, theirs))
                .await?
            {
                persist_trade(a, plan).await?;
            }
        }
        35 => {
            let slot = slot(&mut r).context(CloseReason::Protocol)?;
            r.done()?;
            let id =
                player.ok_or_else(|| lifecycle::protocol("inventory drop before character"))?;
            lifecycle::stage(Stage::Reservation);
            let plan = a.world.call(move |w| w.drop_item(id, slot)).await?;
            persist(a, plan).await?;
        }
        65 => {
            r.i32()?;
            r.f32()?;
            r.f32()?;
            let from = slot(&mut r).context(CloseReason::Protocol)?;
            let to = slot(&mut r).context(CloseReason::Protocol)?;
            r.done()?;
            let id =
                player.ok_or_else(|| lifecycle::protocol("inventory swap before character"))?;
            lifecycle::stage(Stage::Reservation);
            let plan = a.world.call(move |w| w.swap(id, from, to)).await?;
            persist(a, plan).await?;
        }
        33 => {
            let name = r.utf()?;
            r.done()?;
            let account = account
                .as_ref()
                .ok_or_else(|| lifecycle::protocol("name before HELLO"))?
                .id;
            let _gold = a.gold_updates.lock().await;
            lifecycle::stage(Stage::Reservation);
            let reservation = if let Some(id) = *player {
                match a
                    .world
                    .for_player(id)
                    .call(move |w| w.begin_name_choice(id, account))
                    .await?
                {
                    Ok(plan) => Some((id, plan)),
                    Err(reason) => {
                        lifecycle::stage(Stage::Rejected);
                        tracing::info!(account, reason, "name choice rejected before reservation");
                        let mut o = Writer::new();
                        o.u8(0).utf(reason);
                        enqueue_reply(tx, Packet { id: 20, body: o.0 })?;
                        return Ok(());
                    }
                }
            } else {
                None
            };
            lifecycle::stage(Stage::Store);
            let result = a
                .store
                .name_choice(account, name.clone(), reservation.is_some())
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
            if let Some(reason) = rejection {
                tracing::info!(account, reason, "name choice rejected without charge");
            }
            let mut o = Writer::new();
            o.u8(u8::from(rejection.is_none()))
                .utf(rejection.unwrap_or(""));
            enqueue_reply(tx, Packet { id: 20, body: o.0 })?;
        }
        42 | 3 => {
            let _guild = a.guild_actions.lock().await;
            let portal = if packet.id == 3 { Some(r.i32()?) } else { None };
            r.done()?;
            player.ok_or_else(|| lifecycle::protocol("transfer before character"))?;
            let guild_portal = a
                .world
                .call(move |w| {
                    Ok(portal
                        .and_then(|p| w.entities.get(&p))
                        .is_some_and(|e| e.kind == 0x72f))
                })
                .await?;
            if guild_portal {
                let guild = a
                    .store
                    .guild(
                        account
                            .as_ref()
                            .ok_or_else(|| lifecycle::protocol("guild portal before HELLO"))?
                            .id,
                    )
                    .await?;
                a.world.directory(move |d| d.guild_instance(guild)).await?;
            }
            reconnect(a, session, Reconnect::Portal(portal), received).await?;
        }
        _ => {
            return Err(anyhow::anyhow!(CloseReason::Protocol).context(format!(
                "packet {} is not implemented in this development build",
                packet.id
            )));
        }
    }
    Ok(())
}

enum Reconnect {
    Portal(Option<i32>),
    Tutorial,
}

async fn reconnect(
    a: &App,
    session: &mut Session,
    request: Reconnect,
    received: std::time::Instant,
) -> Result<()> {
    let id = session.player.context("transfer before character")?;
    let transfer = a
        .world
        .directory(move |d| {
            let portal = match &request {
                Reconnect::Portal(portal) => *portal,
                Reconnect::Tutorial => None,
            };
            let (account, character, destination) = match request {
                Reconnect::Tutorial => {
                    d.chat_permit(id, received)?;
                    let p = &d.worlds[&d.location(id)?].players[&id];
                    (p.character.account, p.character.id, d.initial(-1)?)
                }
                Reconnect::Portal(portal) => {
                    if portal.is_none() && d.location(id)? == -2 {
                        tracing::debug!(
                            player = id,
                            "redundant Nexus escape acknowledged without transfer"
                        );
                        return Ok(None);
                    }
                    d.destination(id, portal)?
                }
            };
            let world = &d.worlds[&destination];
            if world.occupancy() >= world.max_players {
                let message = crate::world::chat_info("Destination is full.");
                crate::world::send(
                    &d.worlds[&d.location(id)?].players[&id],
                    message.id,
                    message.body,
                );
                return Ok(None);
            }
            let (keytime, key, name) = d.issue(account, character, destination)?;
            let reservation = match d
                .bind_exit(id, portal, &key)
                .and_then(|()| d.reserve_transfer(key.clone()))
            {
                Ok(reservation) => reservation,
                Err(primary) => {
                    if let Err(cleanup) = d.cancel_admission(account, key) {
                        return Err(primary
                            .context(format!("transfer admission cleanup failed: {cleanup:#}")));
                    }
                    return Err(primary);
                }
            };
            Ok(Some((destination, keytime, key, name, reservation)))
        })
        .await?;
    let Some((destination, keytime, key, name, reservation)) = transfer else {
        return Ok(());
    };

    let mut o = Writer::new();
    o.utf(&name)
        .utf("")
        .i32(2050)
        .i32(destination)
        .i32(keytime)
        .i16(key.len() as i16);
    o.0.extend(&key);
    session.transfer = Some(PreparedTransfer {
        packet: Packet { id: 39, body: o.0 },
        reservation,
    });
    tracing::debug!(
        player = id,
        destination,
        "transfer prepared; retirement required before reconnect"
    );
    Ok(())
}

async fn connection(
    socket: WebSocket,
    a: App,
    user_agent: Option<String>,
    browser: Option<Arc<Mutex<diagnostics::Browser>>>,
) {
    let (wsout, wsin) = socket.split();
    connection_io(wsout, wsin, a, user_agent, browser).await;
}
async fn send_frame<S: futures_util::Sink<Message> + Unpin>(
    output: &mut S,
    message: Message,
    lost: &mut tokio::sync::watch::Receiver<Option<&'static str>>,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    cancel: Option<tokio::sync::watch::Receiver<bool>>,
    drain: &mut tokio::sync::watch::Receiver<u64>,
) -> std::result::Result<(), CloseReason> {
    tokio::select! {
        biased;
        _ = drain.changed() => Err(CloseReason::Shutdown),
        _ = async { if lost.borrow().is_none() { let _ = lost.changed().await; } } => Err(CloseReason::Output),
        _ = async { if !*shutdown.borrow() { let _ = shutdown.changed().await; } } => Err(CloseReason::Shutdown),
        _ = async { match cancel { Some(mut cancel) => { if !*cancel.borrow() { let _ = cancel.changed().await; } }, None => std::future::pending::<()>().await } } => Err(CloseReason::Controller),
        sent = tokio::time::timeout(std::time::Duration::from_secs(5), output.send(message)) => match sent {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(CloseReason::Transport),
            Err(_) => Err(CloseReason::Deadline),
        }
    }
}

// Actor-awaiting handlers can starve a one-packet drain: https://docs.rs/tokio/1.53.2/tokio/macro.select.html#fairness
fn outgoing_burst(
    first: Packet,
    rx: &mut mpsc::Receiver<Packet>,
) -> impl Iterator<Item = (Packet, usize)> + '_ {
    let ready = rx.len();
    std::iter::once((first, ready + 1)).chain(
        std::iter::from_fn(move || rx.try_recv().ok().map(|packet| (packet, rx.len() + 1)))
            .take(ready),
    )
}

#[cfg(test)]
#[test]
fn predicted_enemy_deaths_reach_the_owning_world_without_granting_hits() {
    assert!(INPUTS.contains(&76), "predicted enemy deaths are discarded");
    let packet = |killed| {
        let mut w = Writer::new();
        w.i32(1000).u8(1).i32(42).u8(killed);
        Packet { id: 76, body: w.0 }
    };
    let now = std::time::Instant::now();
    assert!(input(&packet(1), Some(1), now, true).unwrap().is_some());
    assert!(input(&packet(0), Some(1), now, true).unwrap().is_none());
    assert!(input(&packet(2), Some(1), now, true).is_err());
    assert!(input(&packet(1), None, now, true).is_err());
    let mut trailing = packet(1);
    trailing.body.push(0);
    assert!(input(&trailing, Some(1), now, true).is_err());
}

#[cfg(test)]
#[test]
fn cloud_volleys_drain_before_more_actor_work_without_buffering_or_starving_input() {
    let (tx, mut rx) = mpsc::channel::<Packet>(64);
    let packet = |id, body| Packet { id, body };
    let mut written = Vec::new();
    let mut expected = Vec::new();
    for phase in 0..8 {
        for cloud in 0..10 {
            let body = vec![phase, cloud];
            tx.try_send(packet(19, body.clone()))
                .expect("cloud volleys filled the unchanged 64-packet output queue");
            expected.push((19, body));
        }
        tx.try_send(packet(wire::NEW_TICK, vec![phase])).unwrap();
        expected.push((wire::NEW_TICK, vec![phase]));
        let first = rx.try_recv().unwrap();
        written.extend(outgoing_burst(first, &mut rx).map(|(packet, _)| (packet.id, packet.body)));
    }
    assert_eq!(written, expected);
    assert!(rx.is_empty());

    for n in 1..=2 {
        tx.try_send(packet(19, vec![n])).unwrap();
    }
    let first = rx.try_recv().unwrap();
    let mut burst = outgoing_burst(first, &mut rx);
    assert_eq!(tx.capacity(), 63);
    tx.try_send(packet(19, vec![99])).unwrap();
    assert_eq!(burst.next().unwrap().0.body, [1]);
    assert_eq!(tx.capacity(), 62);
    assert_eq!(burst.next().unwrap().0.body, [2]);
    assert!(burst.next().is_none());
    drop(burst);
    assert_eq!(rx.try_recv().unwrap().body, [99]);
    for _ in 0..64 {
        tx.try_send(packet(19, vec![])).unwrap();
    }
    assert!(matches!(
        tx.try_send(packet(19, vec![])),
        Err(mpsc::error::TrySendError::Full(_))
    ));
}

async fn connection_io<S, I>(
    mut wsout: S,
    wsin: I,
    mut a: App,
    user_agent: Option<String>,
    browser: Option<Arc<Mutex<diagnostics::Browser>>>,
) where
    S: futures_util::Sink<Message> + Unpin,
    I: futures_util::Stream<Item = std::result::Result<Message, axum::Error>>
        + Send
        + Unpin
        + 'static,
{
    let mut drain = lifecycle::drain::stopping(&a);
    let Ok(_connection) = lifecycle::drain::entry(&a) else {
        return;
    };
    let (tx, mut rx) = mpsc::channel::<Packet>(64);
    let (lost, mut lost_rx) = tokio::sync::watch::channel(None);
    // Actor work and output backpressure must not manufacture late, clustered input receipts.
    let (mut incoming, input_task) = input::receive(wsin, lost.clone(), a.world.clock.clone());
    let mut shutdown = a.shutdown.subscribe();
    let deadline = tokio::time::sleep(std::time::Duration::from_secs(10));
    tokio::pin!(deadline);
    let mut codec = Codec::server();
    let mut session = Session {
        runtime: Some(tokio::runtime::Handle::current()),
        ..Session::default()
    };
    session.diagnostics.user_agent = user_agent;
    let (report_sender, mut report_requests) = mpsc::channel(1);
    if let Some(browser) = &browser {
        browser.lock().expect("browser poisoned").reports = Some(report_sender);
    }
    session.diagnostics.browser = browser;
    let mut reconnect_sent = false;
    let mut deferred = None;
    let mut ping = tokio::time::interval(std::time::Duration::from_secs(5));
    let mut serial = 0;
    let mut transfers = tokio::time::interval(std::time::Duration::from_millis(250));
    let mut testing_stop = a.testing.as_ref().map(|t| t.stopped.subscribe());
    let mut terminal_drain = false;
    let mut close_reason = CloseReason::Transport;
    macro_rules! send {
        ($message:expr) => {
            send_frame(
                &mut wsout,
                $message,
                &mut lost_rx,
                &mut shutdown,
                session.account.as_ref().and_then(|o| o.cancel.clone()),
                &mut drain,
            )
            .await
            .map_err(|reason| {
                close_reason = reason;
            })
        };
    }
    'connection: loop {
        if lost_rx.borrow().is_some() {
            close_reason = CloseReason::Output;
            break;
        }
        if *shutdown.borrow()
            || drain.has_changed().expect("live admission sender lost")
            || !lifecycle::drain::open(&a)
        {
            close_reason = CloseReason::Shutdown;
            break;
        }
        if session
            .account
            .as_ref()
            .and_then(|o| o.cancel.as_ref())
            .is_some_and(|c| *c.borrow())
        {
            close_reason = CloseReason::Controller;
            break;
        }
        tokio::select! {
            Some(request) = report_requests.recv() => diagnostics::from_session(&a, &session, request).await,
            _=async { match testing_stop.as_mut() { Some(stop) => { if stop.borrow().is_none() { let _ = stop.changed().await; } }, None => std::future::pending::<()>().await } } => {
                let player=session.player;
                let terminal=a.world.directory(move |d|Ok(d.worlds.values().flat_map(|w|w.players.iter()).any(|(id,p)|Some(*id)==player && (!p.character.alive || p.death_pending)))).await;
                match terminal {
                    // Ruffle can discard its queued DEATH frame if this socket closes first.
                    Ok(true) => {
                        terminal_drain=true;testing_stop=None;
                        deadline.as_mut().reset(tokio::time::Instant::now()+std::time::Duration::from_secs(5));
                    }
                    Ok(false) => {tracing::warn!("native test control stopped; closing living game connection");break;}
                    Err(error) => {tracing::error!(%error,"terminal native drain inspection failed");break;}
                }
            },
            event=async { match deferred.take() { Some(event) => Some(event), None => incoming.recv().await } }=>match event {
                Some(Incoming::Command(packet, received, bytes))=>{
                    if incoming.is_closed() { break; }
                    if a.testing.as_ref().is_some_and(|t| t.stopped.borrow().is_some()) { if terminal_drain {continue;} break; }
                    if !session.reconnecting {
                        let id=packet.id;
                        let queue_before=incoming.len();
                        let (mut packets, mut _bytes) = (vec![(packet, received)], vec![bytes]);
                        // Serial world round trips per packet cannot keep up with boss volleys while ticks overrun.
                        while INPUTS.contains(&id) && let Ok(next) = incoming.try_recv() {
                            match next {
                                Incoming::Command(packet, received, bytes) if INPUTS.contains(&packet.id) => { packets.push((packet, received)); _bytes.push(bytes); }
                                other => { deferred = Some(other); break; }
                            }
                        }
                        session.diagnostics.packets_in += packets.len() as u64;
                        for (packet, _) in &packets { session.diagnostics.processed(packet.id); }
                        let handling = std::time::Instant::now();
                        let queued=a.world.clock.now().saturating_duration_since(received);
                        let requests=Arc::new(Mutex::new(crate::performance::Requests::default()));
                        let result=crate::performance::REQUEST.scope(requests.clone(),handle(&mut a,packets,&mut session,&tx,&lost)).await;
                        session.diagnostics.handled(diagnostics::CommandSample {packet:id,queued,handling:handling.elapsed(),queue_before,queue_after:incoming.len(),failed:result.is_err(),requests:requests.lock().expect("request timings poisoned").clone()});
                        if let Err(e)=result {
                            let rejected = lifecycle::rule_rejection(&e);
                            close_reason=if e.is::<ReplyQueueError>() { CloseReason::Output } else if rejected { CloseReason::Controller } else { e.downcast_ref::<CloseReason>().copied().unwrap_or(CloseReason::Internal) };
                            if close_reason == CloseReason::Internal && let Some(owner) = &session.account {
                                owner.failed(format!("unclassified native command failure: {e:#}"));
                            }
                            tracing::error!(account=session.account.as_ref().map(|o|o.id),player=session.player,packet=id,error=%format_args!("{e:#}"),"game command rejected");
                            if e.is::<ReplyQueueError>() { close_reason=CloseReason::Output;break 'connection; }
                            if e.downcast_ref::<CloseReason>() == Some(&CloseReason::Protocol) {
                                close_reason=CloseReason::Protocol;
                                let f=fail(&e.to_string());
                                let _=send!(Message::Binary(codec.frame(f.id,f.body).into()));
                                break 'connection;
                            }
                            if rejected && matches!(id,1|21|37|57) && let Some(player) = session.player {
                                let reason = e.to_string();
                                let closed = a.world.for_player(player).call(move |w| Ok(w.cancel_trade(player, &reason))).await;
                                match closed { Ok(true) => {}, Ok(false) => { let mut out = Writer::new(); out.i32(1).utf(&e.to_string()); if send!(Message::Binary(codec.frame(12,out.0).into())).is_err() {break 'connection;} }, Err(error) => { tracing::error!(%error,"trade rejection notification failed"); break 'connection; } }
                                continue;
                            }
                            if rejected && matches!(id,30|35|65) {
                                let mut o=Writer::new();o.i32(1);
                                if send!(Message::Binary(codec.frame(4,o.0).into())).is_err(){break 'connection;}
                                continue;
                            }
                            let f=fail(&e.to_string());
                            let _=send!(Message::Binary(codec.frame(f.id,f.body).into()));
                            break 'connection;
                        }
                    }
                },
                Some(Incoming::Ping(bytes))=>{if send!(Message::Pong(bytes)).is_err(){break}},
                None=>break,
            },
            _=transfers.tick(),if session.player.is_some() && !session.reconnecting=>{
                let player = session.player.expect("guarded loaded player");
                match automatic_transfer(&a, player).await {
                    Ok(Some(transfer)) => {
                        if transfer.player!=player || !transfer.output.packets.same_channel(&tx) {
                            tracing::error!(player,"automatic transfer recipient differs from owned session");
                            break;
                        }
                        session.player = None;
                        session.reconnecting = true;
                        session.account.take();
                        let packet = transfer.packet();
                        // Retired-world frames must not follow RECONNECT into a replacement scene.
                        while rx.try_recv().is_ok() {}
                        if send!(Message::Binary(codec.frame(packet.id,packet.body).into())).is_err(){break;}
                        reconnect_sent=true;
                        deadline.as_mut().reset(tokio::time::Instant::now()+std::time::Duration::from_secs(10));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::error!(player,%error,"automatic durable transfer failed");
                        let f=fail(&error.to_string());
                        let _=send!(Message::Binary(codec.frame(f.id,f.body).into()));
                        break;
                    }
                }
            },
            _=async {
                match session.account.as_ref().and_then(|o| o.cancel.clone()) {
                    Some(mut cancel) => { if !*cancel.borrow() { let _ = cancel.changed().await; } }
                    None => std::future::pending::<()>().await,
                }
            }=>{tracing::info!(account=session.account.as_ref().map(|o|o.id),"closing requested game session");break;},
            _=shutdown.changed()=>{close_reason=CloseReason::Shutdown;break;},
            _=drain.changed()=>{close_reason=CloseReason::Shutdown;break;},
            _=lost_rx.changed()=>{close_reason=CloseReason::Output;tracing::warn!(player=session.player,"closing slow game connection before its state diverges");break;},
            _=&mut deadline,if terminal_drain || session.player.is_none() && (!session.reconnecting || reconnect_sent)=>{close_reason=CloseReason::Deadline;tracing::warn!(reconnecting=session.reconnecting,reconnect_sent,terminal_drain,"game selection, reconnect-close or terminal-drain deadline exceeded");break;},
            outgoing=rx.recv()=>{
                let Some(packet)=outgoing else {break};
                for (packet, queued) in outgoing_burst(packet, &mut rx) {
                    let reconnect=packet.id==39;
                    session.diagnostics.queue_high_water = session.diagnostics.queue_high_water.max(queued);
                    let nonce = (packet.id==wire::PING && packet.body.len()==4).then(|| i32::from_be_bytes(packet.body[..4].try_into().unwrap()));
                    let frame = codec.frame(packet.id,packet.body);
                    if let (Some(testing),Some(player),Some(nonce)) = (&a.testing,session.player,nonce) { testing.note_native_frame(nonce,player,&frame); }
                    let sending = std::time::Instant::now();
                    if send!(Message::Binary(frame.into())).is_err(){break 'connection;}
                    session.diagnostics.packets_out += 1;
                    session.diagnostics.last_socket_send=Some(std::time::Instant::now());
                    session.diagnostics.max_send_ms = session.diagnostics.max_send_ms.max(sending.elapsed().as_secs_f64()*1000.0);
                    // Ruffle can discard unread TCP bytes when the proxy closes before its next frame.
                    if reconnect {reconnect_sent=true;deadline.as_mut().reset(tokio::time::Instant::now()+std::time::Duration::from_secs(10));break;}
                    if (terminal_drain || session.player.is_none() && (!session.reconnecting || reconnect_sent)) && deadline.is_elapsed()
                        || testing_stop.as_ref().is_some_and(|stop| stop.borrow().is_some()) {break;}
                }
            },
            _=ping.tick()=>{if session.account.is_some() && !a.world.clock.paused(){serial+=1;let mut o=Writer::new();o.i32(serial);
                session.diagnostics.sent(serial);
                if send!(Message::Binary(codec.frame(wire::PING,o.0).into())).is_err(){break;}}}
        }
    }
    // The input task closes its queue right after naming a cause, so the loop can see the closed queue first.
    if close_reason == CloseReason::Transport && lost_rx.borrow().is_some() {
        close_reason = CloseReason::Output;
    }
    input_task.abort();
    // Publish custody before diagnostics; neither a cancelled socket task nor its sink owns cleanup.
    let cancelled_admission = session
        .admission
        .take()
        .zip(session.account.as_ref().map(|owner| owner.id));
    let dropped_account = session
        .account
        .as_ref()
        .filter(|_| !reconnect_sent && close_reason != CloseReason::Shutdown)
        .map(|owner| owner.id);
    let finalization = session
        .account
        .take()
        .map(|owner| owner.close(&a, close_reason));
    if let Some(id) = session.player {
        let diagnostic = diagnostics::disconnect(
            &a,
            id,
            dropped_account,
            &session.diagnostics,
            serde_json::json!({"reason":close_reason.to_string(),"cause":*lost_rx.borrow(),
            "input_queue_packets":incoming.len(),"output_queue_packets":rx.len(),"last_frame_age_ms":input_task.last_frame_age_ms()}),
        );
        if std::panic::AssertUnwindSafe(diagnostic)
            .catch_unwind()
            .await
            .is_err()
        {
            eprintln!(
                "native disconnect diagnostics panicked; finalization remains owned for avatar={id}"
            );
        }
    }
    if let Some((key, account)) = cancelled_admission
        && let Err(error) = a
            .world
            .directory(move |d| d.cancel_admission(account, key))
            .await
    {
        tracing::error!(account,%error,"disconnected admission cancellation failed");
    }
    if let Some(result) = finalization {
        let result = match result {
            Ok(Some(finalization)) => finalization.wait().await,
            Ok(None) => Ok(()),
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            tracing::error!(player=?session.player,error=%error,"disconnect finalization failed; account fenced");
        }
    }
}
pub(crate) async fn automatic_transfer(
    a: &App,
    id: i32,
) -> Result<Option<crate::world::ForcedTransfer>> {
    let transfer = a.world.directory(move |d| d.forced_transfer(id)).await?;
    if let Some(transfer) = transfer {
        if let Err(error) = depart(a, id).await {
            return a
                .world
                .directory(move |d| {
                    d.cancel_forced_transfer(&transfer);
                    Err(error)
                })
                .await;
        }
        return Ok(Some(transfer));
    }
    Ok(None)
}
pub(crate) async fn depart(a: &App, id: i32) -> Result<()> {
    // Legacy owned adapter continuations retain custody until their Phase2 conversion.
    if lifecycle::nested_operation() {
        return lifecycle::player(a, id, "departure", move |a| async move {
            depart_inner(&a, id).await
        })
        .await;
    }
    if !a
        .owners
        .lock()
        .expect("ownership registry poisoned")
        .values()
        .any(|c| c.matches_avatar(id))
    {
        lifecycle::player(a, id, "departure_binding", |_| async { Ok(()) }).await?;
    }
    lifecycle::finalize(a, id, CloseReason::Controller)?
        .wait()
        .await
}
enum DepartureStep {
    Save(crate::store::Character),
    Progress(crate::store::Character),
    Death(crate::store::Character, String),
    Waiting,
}
async fn depart_inner(a: &App, id: i32) -> Result<()> {
    depart_inner_with_transfer(a, id, None).await
}
async fn depart_inner_with_transfer(a: &App, id: i32, key: Option<Vec<u8>>) -> Result<()> {
    a.world
        .for_player(id)
        .call(move |w| w.stop_controller(id))
        .await?;
    a.world
        .directory(move |d| {
            let p = &d.worlds[&d.location(id)?].players[&id];
            ensure!(
                d.worlds
                    .values()
                    .all(|w| w.credit_recipients(p.character.id).is_empty()),
                "causal prerequisites require finalization outside an owned giver operation"
            );
            Ok(())
        })
        .await?;
    refresh_giver_credits(a, id).await?;
    lifecycle::stage(Stage::Reservation);
    let origin = a.world.directory(move |d| d.location(id)).await?;
    loop {
        lifecycle::stage(Stage::Reservation);
        let transfer = key.clone();
        let step = a
            .world
            .directory(move |d| {
                if let Some(ch) = d.begin_source_departure(id, transfer.as_deref())? {
                    return Ok(DepartureStep::Save(ch));
                }
                let scope = d.location(id)?;
                let w = d.worlds.get_mut(&scope).unwrap();
                // Departure already owns this generation, so the collector cannot admit its prerequisites.
                if let Some((_, snapshot)) =
                    w.pending_progress_where(|player, _| player == id).pop()
                {
                    return Ok(DepartureStep::Progress(snapshot));
                }
                if let Some((_, ch, killer)) =
                    w.pending_deaths_where(|player, _| player == id).pop()
                {
                    return Ok(DepartureStep::Death(ch, killer));
                }
                Ok(DepartureStep::Waiting)
            })
            .await?;
        let ch = match step {
            DepartureStep::Progress(snapshot) => {
                persist_progress_inner(a, id, snapshot).await?;
                continue;
            }
            DepartureStep::Death(ch, killer) => {
                persist_death_inner(a, id, ch, killer).await?;
                continue;
            }
            DepartureStep::Waiting => {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                continue;
            }
            DepartureStep::Save(ch) => ch,
        };
        let retirement = lifecycle::retirement(a, ch.account, id)?;
        lifecycle::stage(Stage::Store);
        let snapshot = ch.clone();
        if let Some(key) = key.clone() {
            a.world
                .directory(move |d| d.accept_source_write(id, &key))
                .await?;
        }
        let result = if ch.alive {
            a.store
                .progress(ch)
                .await
                .context("departure persistence failed")
        } else {
            Ok(())
        };
        lifecycle::stage(Stage::Publication);
        a.world
            .directory(move |d| {
                let scope = d.location(id)?;
                d.worlds.get_mut(&scope).unwrap().committed(id, result)?;
                d.finish_source_departure(id, snapshot, key)
            })
            .await?;
        lifecycle::stage(Stage::Retirement);
        retirement()?;
        a.world
            .directory(move |d| {
                d.realm_departed(origin, id);
                Ok(())
            })
            .await?;
        lifecycle::stage(Stage::Settled);
        return Ok(());
    }
}
#[derive(Deserialize)]
struct BotCreate {
    guid: String,
    world: i32,
}
async fn bot_create(
    State(a): State<App>,
    headers: HeaderMap,
    Json(input): Json<BotCreate>,
) -> Response {
    let result: Result<serde_json::Value> = async {
        control(&a, &headers)?;
        let acc = a.store.list(input.guid).await?;
        let mut owner = a.claim(acc.id)?;
        let account = acc.id;
        lifecycle::account(&a, vec![account], "script_entry", move |a| async move {
            owner.arm_cleanup(&a, tokio::runtime::Handle::current());
            lifecycle::stage(Stage::Store);
            let ch = if acc.characters.is_empty() {
                a.store.create(acc.id, a.content.get(WIZARD)?).await?
            } else {
                ensure!(acc.characters.len() == 1, "unexpected character count");
                acc.characters[0].clone()
            };
            let bind = lifecycle::binding(&a, acc.id)?;
            lifecycle::stage(Stage::Reservation);
            let destination = a.world.directory(move |d| d.initial(input.world)).await?;
            lifecycle::stage(Stage::Store);
            let presentation = a.store.presentation(acc.id).await?;
            lifecycle::stage(Stage::Publication);
            let id = a
                .world
                .in_world(destination)
                .call(move |w| {
                    let id = w.enter_at(ch, None, None, presentation)?;
                    bind(id)?;
                    Ok(id)
                })
                .await?;
            let mut bots = a.bots.lock().expect("bot ownership registry poisoned");
            ensure!(
                !bots.contains_key(&id),
                "script avatar already has a controller owner"
            );
            owner.publish_script(id)?;
            bots.insert(id, owner);
            lifecycle::stage(Stage::Settled);
            Ok(id)
        })
        .await
        .map(|id| serde_json::json!({"player":id}))
    }
    .await;
    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => error(e),
    }
}
fn control(a: &App, headers: &HeaderMap) -> Result<()> {
    ensure!(
        !*a.shutdown.borrow() && lifecycle::drain::open(a),
        "server admission is frozen"
    );
    let token = a.control.as_ref().context("script control is disabled")?;
    ensure!(
        headers.get("authorization").and_then(|v| v.to_str().ok())
            == Some(format!("Bearer {token}").as_str()),
        "invalid control token"
    );
    Ok(())
}
#[derive(Deserialize)]
struct BotCommand {
    player: i32,
    action: Action,
}
async fn bot_action(
    State(a): State<App>,
    headers: HeaderMap,
    Json(input): Json<BotCommand>,
) -> Response {
    let result: Result<serde_json::Value> = async {
        control(&a, &headers)?;
        let id = input.player;
        inventory(&a, id, move |w| w.action(id, input.action)).await??;
        a.world.for_player(id).call(move |w| w.observe(id)).await
    }
    .await;
    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => error(e),
    }
}
#[derive(Deserialize)]
struct BotTrade {
    player: i32,
    action: TradeAction,
}
async fn bot_trade(
    State(a): State<App>,
    headers: HeaderMap,
    Json(input): Json<BotTrade>,
) -> Response {
    let result: Result<serde_json::Value> = async {
        control(&a, &headers)?;
        let id = input.player;
        lifecycle::player(&a, id, "trade", move |a| async move {
            lifecycle::stage(Stage::Reservation);
            let prepared = a
                .world
                .for_player(id)
                .call(move |w| Ok(w.script_trade(id, input.action)))
                .await?;
            match prepared {
                Ok(Some(plan)) => persist_trade(&a, plan).await,
                Ok(None) => Ok(()),
                Err(error) => {
                    if lifecycle::rule_rejection(&error) {
                        lifecycle::stage(Stage::Rejected);
                    }
                    Err(error)
                }
            }
        })
        .await?;
        a.world.for_player(id).call(move |w| w.observe(id)).await
    }
    .await;
    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => error(e),
    }
}
#[derive(Deserialize)]
struct ScriptStep {
    at_ms: u64,
    action: Option<Action>,
}
#[derive(Deserialize)]
struct Script {
    player: i32,
    steps: Vec<ScriptStep>,
}
async fn script(State(a): State<App>, headers: HeaderMap, Json(input): Json<Script>) -> Response {
    let result: Result<Vec<serde_json::Value>> = async {
        control(&a, &headers)?;
        ensure!(
            !input.steps.is_empty() && input.steps.len() <= 512,
            "script needs 1–512 steps"
        );
        let mut previous = 0;
        for s in &input.steps {
            ensure!(
                s.at_ms >= previous && s.at_ms <= 120000,
                "script times must be ordered and at most 120 seconds"
            );
            previous = s.at_ms;
        }
        let start = tokio::time::Instant::now();
        let mut shutdown=a.shutdown.subscribe();
        let mut drain=lifecycle::drain::stopping(&a);
        let mut frames = Vec::new();
        for step in input.steps {
            tokio::select!{_=tokio::time::sleep_until(start+std::time::Duration::from_millis(step.at_ms))=>{},_=shutdown.changed()=>bail!("server is shutting down"),_=drain.changed()=>bail!("server admission is frozen")};
            let id = input.player;
            if let Some(action) = step.action {
                inventory(&a, id, move |w| w.action(id, action)).await??;
            }
            frames.push(a.world.for_player(id).call(move |w| w.observe(id)).await?);
        }
        Ok(frames)
    }
    .await;
    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => error(e),
    }
}
#[derive(Deserialize)]
struct MapView {
    player: Option<i32>,
}
async fn map(State(a): State<App>, headers: HeaderMap, Query(view): Query<MapView>) -> Response {
    let world = match view.player {
        Some(id) => a.world.for_player(id),
        None => a.world.in_world(-1),
    };
    let result: Result<serde_json::Value> = async {
        control(&a, &headers)?;
        world.call(move |w| w.inspect_map(view.player)).await
    }
    .await;
    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => error(e),
    }
}
#[derive(Deserialize)]
struct BotPlayer {
    player: i32,
}
async fn observe(
    State(a): State<App>,
    headers: HeaderMap,
    Json(input): Json<BotPlayer>,
) -> Response {
    let result: Result<serde_json::Value> = async {
        control(&a, &headers)?;
        a.world
            .directory(move |d| {
                let scope = d.location(input.player)?;
                let mut frame = d.worlds[&scope].observe(input.player)?;
                frame["player_view"] = serde_json::to_value(d.player_view(input.player)?)?;
                Ok(frame)
            })
            .await
    }
    .await;
    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => error(e),
    }
}
async fn bot_leave(
    State(a): State<App>,
    headers: HeaderMap,
    Json(input): Json<BotPlayer>,
) -> Response {
    let result: Result<()> = async {
        control(&a, &headers)?;
        if let Some(finalization) = close_script(&a, input.player)? {
            finalization.wait().await?;
        }
        Ok(())
    }
    .await;
    match result {
        Ok(()) => Json(serde_json::json!({"saved":true})).into_response(),
        Err(e) => error(e),
    }
}
fn close_script(a: &App, player: i32) -> Result<Option<lifecycle::Finalized>> {
    // Serialize map custody with registry publication, so a duplicate cannot see neither.
    let mut bots = a.bots.lock().expect("bot ownership registry poisoned");
    match bots.remove(&player) {
        Some(owner) => owner.close(a, CloseReason::Controller),
        None => lifecycle::join_script(a, player).map(Some),
    }
}
fn close_scripts(a: &App) -> Result<Vec<lifecycle::Finalized>> {
    let mut bots = a.bots.lock().expect("bot ownership registry poisoned");
    let mut completions = lifecycle::script_finalizations(a);
    for id in bots.keys().copied().collect::<Vec<_>>() {
        if let Some(completion) = bots
            .remove(&id)
            .expect("script owner")
            .close(a, CloseReason::Shutdown)?
        {
            completions.push(completion);
        }
    }
    Ok(completions)
}

pub(crate) async fn persistence_tick(a: &App) -> Result<()> {
    // Commit progress across worlds before death settlement so causal recipients stay durable.
    for world in a.world.worlds() {
        let app = a.clone();
        let Some(progress) = world
            .registered_world(move |world| {
                let mut owned = BTreeMap::new();
                let mut failure = None;
                let plans = world.pending_progress_where(|id, account| {
                    match lifecycle::reserve_player(&app, account, id, "progress") {
                        Ok(Some(guard)) => {
                            owned.insert(id, guard);
                            true
                        }
                        Ok(None) => false,
                        Err(error) => {
                            failure = Some(error);
                            false
                        }
                    }
                });
                if let Some(error) = failure {
                    return Err(error);
                }
                Ok(plans
                    .into_iter()
                    .map(|(id, snapshot)| {
                        (
                            owned.remove(&id).expect("reserved progress owner"),
                            id,
                            snapshot,
                        )
                    })
                    .collect::<Vec<_>>())
            })
            .await?
        else {
            continue;
        };
        for (guard, id, snapshot) in progress {
            if let Err(error) = lifecycle::complete(guard, move |a| async move {
                persist_progress_inner(&a, id, snapshot).await
            })
            .await
            {
                tracing::error!(player=id,error=%format_args!("{error:#}"),"progress completion failed");
            }
        }
    }
    let mut dying = Vec::new();
    for world in a.world.worlds() {
        let app = a.clone();
        if let Some(candidates) = world
            .registered_world(move |world| {
                let mut candidates = Vec::new();
                for (id, p) in world
                    .players
                    .iter()
                    .filter(|(_, p)| p.death_pending && !p.death_started)
                {
                    if lifecycle::collector_death_candidate(&app, p.character.account, *id)? {
                        candidates.push(*id);
                    }
                }
                Ok(candidates)
            })
            .await?
        {
            dying.extend(candidates);
        }
    }
    for id in dying {
        settle_giver_credits(a, id).await?;
        refresh_giver_credits(a, id).await?;
    }
    for world in a.world.worlds() {
        let app = a.clone();
        let Some(pending) = world
            .registered_world(move |world| {
                let mut owned = BTreeMap::new();
                let mut failure = None;
                let plans =
                    world.pending_deaths_where(|id, account| {
                        match lifecycle::reserve_player(&app, account, id, "death") {
                            Ok(Some(guard)) => {
                                owned.insert(id, guard);
                                true
                            }
                            Ok(None) => false,
                            Err(error) => {
                                failure = Some(error);
                                false
                            }
                        }
                    });
                if let Some(error) = failure {
                    return Err(error);
                }
                Ok(plans
                    .into_iter()
                    .map(|(id, ch, killer)| {
                        (
                            owned.remove(&id).expect("reserved death owner"),
                            id,
                            ch,
                            killer,
                        )
                    })
                    .collect::<Vec<_>>())
            })
            .await?
        else {
            continue;
        };
        for (guard, id, ch, killer) in pending {
            if let Err(error) = lifecycle::complete(guard, move |a| async move {
                persist_death_inner(&a, id, ch, killer).await
            })
            .await
            {
                tracing::error!(player=id,error=%format_args!("{error:#}"),"death completion failed");
            }
        }
    }
    Ok(())
}
fn client_assets(client: &std::path::Path) -> Router<App> {
    let mut router = Router::new();
    for (route, file) in [
        ("/", "index.html"),
        ("/index.html", "index.html"),
        ("/launcher.js", "launcher.js"),
        ("/draw.html", "draw.html"),
        ("/native-texture-editor.swf", "native-texture-editor.swf"),
    ] {
        router = router.route_service(
            route,
            tower_http::services::ServeFile::new(client.join(file)),
        );
    }
    router.nest_service(
        "/vendor",
        tower_http::services::ServeDir::new(client.join("vendor")),
    )
}

fn routes(a: &App, client: PathBuf) -> Router {
    let app = Router::new()
        .route(
            "/legacy-api/picture/save",
            post(pictures::save).layer(DefaultBodyLimit::max(24 * 1024)),
        )
        .route(
            "/legacy-api/picture/get",
            post(pictures::get_picture).layer(DefaultBodyLimit::max(2048)),
        )
        .route(
            "/legacy-api/picture/{operation}",
            post(pictures::request).layer(DefaultBodyLimit::max(2048)),
        )
        .route("/legacy-api/char/list", post(chars))
        .route("/legacy-api/char/fame", post(fame))
        .route("/legacy-api/char/delete", post(delete_character))
        .route("/legacy-api/account/{operation}", post(account_request))
        .route("/legacy-api/fame/list", post(legends))
        .route("/legacy-api/guild/{operation}", post(guilds::request))
        .nest_service("/sfx", tower_http::services::ServeDir::new(a.sfx.clone()))
        .route("/client.swf", get(swf))
        .route("/game", get(websocket))
        .route(
            "/api/diagnostics/{key}",
            post(diagnostics::browser_sample).layer(DefaultBodyLimit::max(4096)),
        )
        .route(
            "/api/bug-reports/{id}/diagnostics.json",
            get(diagnostics::attachment),
        )
        .route(
            "/api/diagnostics/{key}/report",
            post(diagnostics::submit).layer(DefaultBodyLimit::max(1_500_000)),
        )
        .route("/api/bug-reports/{id}/status", get(diagnostics::status))
        .route(
            "/api/bug-reports/{id}/screenshot.png",
            get(diagnostics::screenshot),
        )
        .route("/internal/discord/bugs", get(diagnostics::pending))
        .route("/internal/discord/bugs/{id}", post(diagnostics::receipt))
        .route("/api/updates", get(updates::feed))
        .route(
            "/internal/discord/updates",
            post(updates::sync).layer(DefaultBodyLimit::max(4 * 1024 * 1024)),
        )
        .route("/api/hiscores", get(hiscores))
        .route("/api/collection/{account}", get(collection))
        .route("/internal/discord/announce", post(discord_announce))
        .route("/internal/discord/link", post(discord_link))
        .route("/internal/discord/link/start", post(browser_link::issue))
        .route("/internal/discord/link/status", post(browser_link::status))
        .route("/internal/discord/unlink", post(browser_link::unlink))
        .route("/api/account/browser", post(browser_link::browser))
        .route("/api/account/browser/close", post(browser_link::close))
        .route("/api/account/link/start", post(browser_link::start))
        .route("/api/account/link/complete", post(browser_link::complete))
        .route("/api/account/link/callback", get(browser_link::callback))
        .route("/internal/discord/kick", post(discord_kick))
        .route("/internal/discord/recover", post(discord_recover))
        .route("/api/account/recover", post(reset_password));
    app.route("/dev/testing", post(testing::command))
        .route("/dev/join", post(bot_create))
        .route("/dev/action", post(bot_action))
        .route("/dev/trade", post(bot_trade))
        .route("/dev/script", post(script))
        .route("/dev/map", get(map))
        .route("/dev/observe", post(observe))
        .route("/dev/leave", post(bot_leave))
        .route_layer(axum::middleware::from_fn_with_state(
            a.clone(),
            lifecycle::drain::http_entry,
        ))
        .merge(lifecycle::drain::routes())
        .route("/health", get(health))
        .route("/api/bots", get(crate::bots::cctv::list))
        .route("/api/bots/{name}/stream", get(crate::bots::cctv::stream))
        .route(
            "/internal/discord/critical-incident",
            get(critical_incident),
        )
        .merge(client_assets(&client))
        .layer(DefaultBodyLimit::max(1_048_576))
        .with_state(a.clone())
}

pub async fn serve(a: App, bind: SocketAddr, client: PathBuf) -> Result<()> {
    let deaths = a.clone();
    a.world.supervisor.spawn("persistence", async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            persistence_tick(&deaths).await?;
        }
    });
    let listener = tokio::net::TcpListener::bind(bind).await.with_context(|| {
        format!("cannot listen on {bind}; another server may already use this port. Stop it and rerun ./run.sh")
    })?;
    if a.shore_bots {
        crate::bots::start(&a).await?;
    }
    let app = routes(&a, client);
    tracing::info!(address=%bind,origin=%a.origin,max_players=a.max_players,behind_proxy=a.behind_proxy,"game frontdoor listening");
    if !a.behind_proxy {
        eprintln!(
            "\nLocal server ready. Open: http://{bind}/\nLogs: the terminal and .local/server.log\n"
        );
    }
    let shutdown = a.clone();
    let failure = Arc::new(Mutex::new(None));
    let report = failure.clone();
    let (stopped, stop) = tokio::sync::watch::channel(false);
    let serving = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let result: Result<()> = async {
            shutdown_signal().await?;
            tracing::info!("saving active characters before shutdown");
            lifecycle::drain::prepare(&shutdown);
            shutdown.shutdown.send_replace(true);
            lifecycle::drain::terminate(&shutdown).await
        }
        .await;
        if let Err(e) = result {
            tracing::error!(error=%e,"shutdown persistence failed");
            *report.lock().expect("shutdown report poisoned") = Some(e);
        }
        stopped.send_replace(true);
    });
    supervise_serving(&a, async { serving.await }, &failure, stop).await
}
async fn supervise_serving(
    a: &App,
    serving: impl std::future::Future<Output = std::io::Result<()>>,
    failure: &Mutex<Option<anyhow::Error>>,
    mut stop: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let (result, checkpoint_attempted) = tokio::select! {
        mut incident = a.world.supervisor.failed() => {
            a.shutdown.send_replace(true);
            let critical = format!("critical {} worker {}; incident={}", incident.component, incident.failure, incident.id);
            let checkpoint = lifecycle::preserve_uncertain(a, &mut incident)
                .with_context(|| format!("{critical}; account-fence checkpoint failed"));
            (checkpoint.and(Err(anyhow::anyhow!(critical))), true)
        },
        result = serving => (result.map_err(anyhow::Error::from), false),
        _ = async { let _ = stop.changed().await; tokio::time::sleep(std::time::Duration::from_secs(1)).await; } => {
            (Err(anyhow::anyhow!("HTTP shutdown deadline exceeded")), false)
        }
    };
    let failure = failure.lock().expect("shutdown report poisoned").take();
    if let Some(error) = failure {
        // Retrying a critical checkpoint could hide its failure and discard the incident diagnostic.
        if checkpoint_attempted {
            let critical = result.expect_err("critical termination cannot succeed");
            return Err(error.context(format!("concurrent shutdown failure: {critical:#}")));
        }
        return failed_shutdown(a, error).await;
    }
    result
}
async fn failed_shutdown(a: &App, error: anyhow::Error) -> Result<()> {
    a.world
        .supervisor
        .failure("shutdown", anyhow::anyhow!("{error:#}"));
    let mut incident = a.world.supervisor.failed().await;
    if let Err(checkpoint) = lifecycle::preserve_uncertain(a, &mut incident) {
        return Err(error.context(format!(
            "shutdown account-fence checkpoint failed: {checkpoint:#}"
        )));
    }
    Err(error)
}
async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {v=tokio::signal::ctrl_c()=>v?,_=term.recv()=>{}};
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}

#[cfg(test)]
mod lifecycle_finalize_tests;
