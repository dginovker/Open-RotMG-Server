use super::hiscores::{json_error, json_result, private_denial};
use super::*;
use crate::store::browser_link::BrowserIdentity;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicI32, AtomicU8, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

fn cookie_name(secure: bool) -> &'static str {
    if secure {
        "__Host-openrotmg-browser"
    } else {
        "openrotmg-browser"
    }
}
const TTL: Duration = Duration::from_secs(300);
const TAB_TTL: Duration = Duration::from_secs(180);

fn random() -> String {
    format!(
        "{:032x}{:032x}",
        rand::random::<u128>(),
        rand::random::<u128>()
    )
}
fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn token(value: &str) -> Result<()> {
    ensure!(
        value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid or expired link"
    );
    Ok(())
}
fn origin(a: &App, headers: &HeaderMap) -> Result<()> {
    ensure!(
        headers.get("origin").and_then(|v| v.to_str().ok()) == Some(&a.origin),
        "account request origin denied"
    );
    Ok(())
}
fn cookie(headers: &HeaderMap, secure: bool) -> Option<String> {
    headers
        .get("cookie")?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|part| {
            let (name, value) = part.trim().split_once('=')?;
            (name == cookie_name(secure) && token(value).is_ok()).then(|| hash(value))
        })
}

pub(crate) struct OAuth {
    client_id: String,
    secret: String,
    authorize: String,
    exchange: String,
    user: String,
    client: reqwest::Client,
}
impl OAuth {
    fn new(client_id: String, secret: String) -> Result<Self> {
        ensure!(
            (17..=20).contains(&client_id.len()) && client_id.bytes().all(|v| v.is_ascii_digit()),
            "invalid Discord OAuth application ID"
        );
        ensure!(
            !secret.trim().is_empty(),
            "Discord OAuth credential is empty"
        );
        Ok(Self {
            client_id,
            secret: secret.trim().into(),
            authorize: "https://discord.com/oauth2/authorize".into(),
            exchange: "https://discord.com/api/oauth2/token".into(),
            user: "https://discord.com/api/v10/users/@me".into(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
        })
    }
    async fn identify(&self, code: &str, callback: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct Access {
            access_token: String,
            token_type: String,
            scope: String,
        }
        #[derive(Deserialize)]
        struct User {
            id: String,
        }
        let response = self
            .client
            .post(&self.exchange)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", callback),
                ("client_id", &self.client_id),
                ("client_secret", &self.secret),
            ])
            .send()
            .await
            .context("Discord token exchange unavailable")?;
        ensure!(
            response.status().is_success(),
            "Discord token exchange rejected ({})",
            response.status()
        );
        let access: Access = response
            .json()
            .await
            .context("invalid Discord token response")?;
        ensure!(
            access.token_type.eq_ignore_ascii_case("bearer")
                && access.scope.split_whitespace().any(|v| v == "identify"),
            "Discord identify authorization missing"
        );
        let response = self
            .client
            .get(&self.user)
            .bearer_auth(access.access_token)
            .send()
            .await
            .context("Discord identity service unavailable")?;
        ensure!(
            response.status().is_success(),
            "Discord identity verification rejected ({})",
            response.status()
        );
        let user: User = response
            .json()
            .await
            .context("invalid Discord identity response")?;
        Ok(user.id)
    }
}

struct Pending {
    discord: String,
    account: AtomicI32,
    expires: Instant,
    used: AtomicU8,
}
struct Tab {
    seen: Instant,
    identity: Option<BrowserIdentity>,
}
struct Flow {
    state: String,
    pending: Arc<Pending>,
    identity: BrowserIdentity,
}
#[derive(Default)]
struct Browser {
    tabs: HashMap<String, Tab>,
    flow: Option<Flow>,
    // A newer flow must cancel callbacks already waiting on Discord's network response.
    authorization: Option<String>,
    completion: Option<(String, String, Arc<Pending>)>,
}
impl Browser {
    fn touch(&mut self, tab: &str) -> Result<&mut Tab> {
        token(tab)?;
        self.tabs
            .retain(|id, t| id == tab || t.seen.elapsed() < TAB_TTL);
        ensure!(
            self.tabs.contains_key(tab) || self.tabs.len() < 8,
            "too many game tabs in this browser"
        );
        let current = self.tabs.entry(tab.into()).or_insert_with(|| Tab {
            seen: Instant::now(),
            identity: None,
        });
        current.seen = Instant::now();
        Ok(current)
    }
    fn identity(&self) -> Result<Option<BrowserIdentity>> {
        let identities: Vec<_> = self
            .tabs
            .values()
            .filter(|t| t.seen.elapsed() < TAB_TTL)
            .filter_map(|t| t.identity.as_ref())
            .collect();
        let Some(first) = identities.first() else {
            return Ok(None);
        };
        ensure!(
            identities.iter().all(|i| *i == *first),
            "different game accounts are open; close the other game tab"
        );
        Ok(Some((*first).clone()))
    }
}
#[derive(Default)]
struct Registry {
    browsers: HashMap<String, Arc<AsyncMutex<Browser>>>,
    pending: HashMap<String, Arc<Pending>>,
}
impl Registry {
    fn container(&mut self, key: String) -> Result<Arc<AsyncMutex<Browser>>> {
        if let Some(browser) = self.browsers.get(&key) {
            return Ok(browser.clone());
        }
        self.browsers.retain(|_, b| match b.try_lock() {
            Err(_) => true,
            Ok(b) => b.tabs.values().any(|t| t.seen.elapsed() < TAB_TTL),
        });
        ensure!(
            self.browsers.len() < 256,
            "browser authentication capacity reached"
        );
        let browser = Arc::new(AsyncMutex::new(Browser::default()));
        self.browsers.insert(key, browser.clone());
        Ok(browser)
    }
}
#[derive(Default)]
pub(crate) struct Service {
    registry: Mutex<Registry>,
    oauth: Option<OAuth>,
    secure: bool,
}
impl Service {
    pub(crate) fn from_env(public_origin: &str) -> Result<Self> {
        let optional = |name| -> Result<Option<String>> {
            match std::env::var(name) {
                Ok(value) => Ok(Some(value)),
                Err(std::env::VarError::NotPresent) => Ok(None),
                Err(error) => Err(error.into()),
            }
        };
        let id = optional("DISCORD_OAUTH_APPLICATION_ID")?;
        let file = optional("DISCORD_OAUTH_SECRET_FILE")?;
        let oauth = match (id, file) {
            (None, None) => None,
            (Some(id), Some(file)) => Some(OAuth::new(
                id,
                std::fs::read_to_string(file).context("cannot read Discord OAuth credential")?,
            )?),
            _ => bail!(
                "Discord OAuth requires both DISCORD_OAUTH_APPLICATION_ID and DISCORD_OAUTH_SECRET_FILE"
            ),
        };
        Ok(Self {
            oauth,
            secure: public_origin.starts_with("https:"),
            ..Self::default()
        })
    }
    fn session(&self, headers: &HeaderMap) -> Result<(Arc<AsyncMutex<Browser>>, Option<String>)> {
        let (key, secret) = match cookie(headers, self.secure) {
            Some(key) => (key, None),
            None => {
                let secret = random();
                (hash(&secret), Some(secret))
            }
        };
        let browser = self
            .registry
            .lock()
            .expect("browser registry poisoned")
            .container(key)?;
        Ok((browser, secret))
    }
    fn container(&self, headers: &HeaderMap) -> Result<Arc<AsyncMutex<Browser>>> {
        let key = cookie(headers, self.secure).context("sign into the game in this browser")?;
        // A restart discards this container, not the native credentials; identity must be proven again.
        self.registry
            .lock()
            .expect("browser registry poisoned")
            .container(key)
    }
    fn browser(&self, headers: &HeaderMap) -> Result<Arc<AsyncMutex<Browser>>> {
        let key = cookie(headers, self.secure).context("sign into the game in this browser")?;
        self.registry
            .lock()
            .expect("browser registry poisoned")
            .browsers
            .get(&key)
            .cloned()
            .context("game browser session expired; reopen the game")
    }
}

pub(super) struct Native {
    browser: OwnedMutexGuard<Browser>,
    tab: String,
    pub(super) cookie: Option<String>,
}
impl Native {
    pub(super) fn authenticate(&mut self, identity: BrowserIdentity) {
        self.browser
            .tabs
            .get_mut(&self.tab)
            .expect("validated browser tab")
            .identity = Some(identity);
    }
}
pub(super) async fn native(
    a: &App,
    headers: &HeaderMap,
    query: &HashMap<String, String>,
) -> Result<Option<Native>> {
    let Some(tab) = query.get("browser") else {
        return Ok(None);
    };
    origin(a, headers)?;
    let (browser, cookie) = a.browser_links.session(headers)?;
    let mut browser = browser.lock_owned().await;
    browser.touch(tab)?.identity = None;
    Ok(Some(Native {
        browser,
        tab: tab.clone(),
        cookie,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BrowserRequest {
    tab: String,
}
pub(super) fn with_cookie(a: &App, mut response: Response, secret: Option<String>) -> Response {
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    if let Some(secret) = secret {
        response.headers_mut().insert(
            "set-cookie",
            format!(
                "{}={secret}; Path=/; HttpOnly; SameSite=Lax{}",
                cookie_name(a.browser_links.secure),
                if a.browser_links.secure {
                    "; Secure"
                } else {
                    ""
                }
            )
            .parse()
            .unwrap(),
        );
    }
    response
}
pub(super) async fn browser(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<BrowserRequest>,
) -> Response {
    if let Err(error) = origin(&a, &headers) {
        return json_error(StatusCode::FORBIDDEN, error);
    }
    json_result(
        async {
            token(&request.tab)?;
            if cookie(&headers, a.browser_links.secure).is_none() {
                ensure!(admit(&a, &headers, peer)?, "too many browser requests");
                return Ok(serde_json::json!({"state":"waiting"}));
            }
            a.browser_links
                .container(&headers)?
                .lock()
                .await
                .touch(&request.tab)?;
            Ok(serde_json::json!({"tab":request.tab}))
        }
        .await,
    )
}
pub(super) async fn close(
    State(a): State<App>,
    headers: HeaderMap,
    Json(request): Json<BrowserRequest>,
) -> Response {
    json_result(
        async {
            origin(&a, &headers)?;
            let tab = request.tab;
            a.browser_links
                .browser(&headers)?
                .lock()
                .await
                .tabs
                .remove(&tab);
            Ok(serde_json::json!({"closed":true}))
        }
        .await,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DiscordRequest {
    discord_id: String,
}
pub(super) async fn issue(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<DiscordRequest>,
) -> Response {
    if let Some(denied) = private_denial(&a, peer, &headers) {
        return denied;
    }
    json_result(
        async {
            ensure!(
                a.browser_links.oauth.is_some(),
                "Discord browser linking is not configured"
            );
            ensure!(
                (17..=20).contains(&request.discord_id.len())
                    && request.discord_id.bytes().all(|v| v.is_ascii_digit()),
                "invalid Discord identity"
            );
            let token = random();
            let mut registry = a
                .browser_links
                .registry
                .lock()
                .expect("browser registry poisoned");
            registry.pending.retain(|_, p| {
                if p.discord == request.discord_id || p.expires <= Instant::now() {
                    p.used.store(2, Ordering::SeqCst);
                    false
                } else {
                    true
                }
            });
            ensure!(
                registry.pending.len() < 1024,
                "Discord linking capacity reached"
            );
            registry.pending.insert(
                hash(&token),
                Arc::new(Pending {
                    discord: request.discord_id,
                    account: AtomicI32::new(0),
                    expires: Instant::now() + TTL,
                    used: AtomicU8::new(0),
                }),
            );
            Ok(serde_json::json!({"url":format!("{}/link/#{token}",a.origin)}))
        }
        .await,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StartRequest {
    token: String,
}
pub(super) async fn status(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<StartRequest>,
) -> Response {
    // Read-only polling must not exhaust the bot's shared mutation-attempt limit.
    if super::hiscores::authorize(&a, peer, &headers).is_err() {
        return json_error(StatusCode::FORBIDDEN, "account API authorization denied");
    }
    json_result(
        async {
            token(&request.token)?;
            let registry = a
                .browser_links
                .registry
                .lock()
                .expect("browser registry poisoned");
            let state = match registry.pending.get(&hash(&request.token)) {
                Some(p) if p.used.load(Ordering::SeqCst) == 3 => "linked",
                Some(p) if p.expires > Instant::now() && p.used.load(Ordering::SeqCst) < 2 => {
                    "pending"
                }
                _ => "expired",
            };
            Ok(serde_json::json!({"state":state}))
        }
        .await,
    )
}

pub(super) async fn unlink(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<DiscordRequest>,
) -> Response {
    if let Some(denied) = private_denial(&a, peer, &headers) {
        return denied;
    }
    json_result(
        async {
            // Fence in-flight OAuth callbacks and resets before revoking their grants.
            let _authentication = a.authentication.write().await;
            let account = a.store.unlink_discord(request.discord_id.clone()).await?;
            a.browser_links
                .registry
                .lock()
                .expect("browser registry poisoned")
                .pending
                .retain(|_, p| {
                    if p.discord != request.discord_id
                        && account != Some(p.account.load(Ordering::SeqCst))
                    {
                        return true;
                    }
                    p.used.store(2, Ordering::SeqCst);
                    false
                });
            Ok(serde_json::json!({"unlinked":account.is_some()}))
        }
        .await,
    )
}

pub(super) async fn start(
    State(a): State<App>,
    headers: HeaderMap,
    Json(request): Json<StartRequest>,
) -> Response {
    json_result(
        async {
            origin(&a, &headers)?;
            token(&request.token)?;
            let pending = a
                .browser_links
                .registry
                .lock()
                .expect("browser registry poisoned")
                .pending
                .get(&hash(&request.token))
                .cloned()
                .context("link expired; run /link in Discord again")?;
            ensure!(
                pending.expires > Instant::now() && pending.used.load(Ordering::SeqCst) == 0,
                "link expired or already used; run /link in Discord again"
            );
            let Some(browser) = cookie(&headers, a.browser_links.secure).and_then(|key| {
                a.browser_links
                    .registry
                    .lock()
                    .expect("browser registry poisoned")
                    .browsers
                    .get(&key)
                    .cloned()
            }) else {
                return Ok(serde_json::json!({"state":"signed_out"}));
            };
            let mut browser = browser.lock().await;
            let Some(identity) = browser.identity()? else {
                return Ok(serde_json::json!({"state":"signed_out"}));
            };
            let _authentication = a.authentication.read().await;
            ensure!(
                a.store.browser_identity(identity.account).await? == identity,
                "game authentication changed; sign in again"
            );
            pending.account.store(identity.account, Ordering::SeqCst);
            let oauth = a
                .browser_links
                .oauth
                .as_ref()
                .context("Discord browser linking is not configured")?;
            let state = random();
            let mut url = reqwest::Url::parse(&oauth.authorize)?;
            url.query_pairs_mut().extend_pairs(&[
                ("client_id", oauth.client_id.as_str()),
                (
                    "redirect_uri",
                    &format!("{}/api/account/link/callback", a.origin),
                ),
                ("response_type", "code"),
                ("scope", "identify"),
                ("prompt", "none"),
                ("state", &state),
            ]);
            let authorization = hash(&state);
            browser.authorization = Some(authorization.clone());
            browser.completion = None;
            browser.flow = Some(Flow {
                state: authorization,
                pending,
                identity,
            });
            Ok(serde_json::json!({"state":"authorize","url":url.as_str()}))
        }
        .await,
    )
}

#[derive(Deserialize)]
pub(super) struct Callback {
    state: Option<String>,
    code: Option<String>,
    iss: Option<String>,
    error: Option<String>,
}
pub(super) async fn callback(
    State(a): State<App>,
    headers: HeaderMap,
    Query(request): Query<Callback>,
) -> Response {
    let result: Result<String> = async {
        let state = request.state.context("Discord state missing")?;
        token(&state)?;
        let browser = a.browser_links.browser(&headers)?;
        // Cookie-bound OAuth state prevents a forwarded link granting an attacker's Discord recovery: https://docs.discord.com/developers/topics/oauth2#state-and-security
        let flow = {
            let mut browser = browser.lock().await;
            let flow = browser.flow.as_ref().context("Discord authorization expired")?;
            ensure!(flow.state == hash(&state), "Discord authorization state rejected");
            browser.flow.take().unwrap()
        };
        ensure!(request.error.is_none(), "Discord authorization was not completed");
        ensure!(request.iss.as_deref() == Some("https://discord.com"), "Discord authorization issuer rejected");
        let oauth = a.browser_links.oauth.as_ref().context("Discord browser linking is not configured")?;
        let discord = oauth.identify(&request.code.context("Discord authorization code missing")?, &format!("{}/api/account/link/callback", a.origin)).await?;
        ensure!(discord == flow.pending.discord, "Discord account differs from /link; sign into the correct Discord account and run /link again");
        let mut browser = browser.lock().await;
        ensure!(browser.authorization.as_ref() == Some(&flow.state), "Discord authorization replaced; run /link again");
        ensure!(browser.identity()?.as_ref() == Some(&flow.identity), "game account changed; run /link again");
        let _authentication = a.authentication.read().await;
        ensure!(flow.pending.expires > Instant::now() && flow.pending.used.compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst).is_ok(), "link expired or already used; run /link again");
        let linked: Result<String> = async {
            let name = a.store.link_browser_discord(flow.identity.clone(), discord).await?;
            gold::sync(&a, flow.identity.account).await?;
            Ok(name)
        }.await;
        flow.pending.used.store(if linked.is_ok() { 3 } else { 2 }, Ordering::SeqCst);
        let name = linked?;
        let receipt = random();
        browser.authorization = None;
        browser.completion = Some((hash(&receipt), name, flow.pending));
        Ok(receipt)
    }.await;
    let mut url =
        reqwest::Url::parse(&format!("{}/link/", a.origin)).expect("validated public origin");
    match result {
        Ok(receipt) => url.set_fragment(Some(&url::encoded("result", &receipt))),
        Err(error) => {
            tracing::warn!(error=%error, "browser Discord link not confirmed");
            url.set_fragment(Some(&url::encoded("error", &error.to_string())));
        }
    }
    let mut response = axum::response::Redirect::to(url.as_str()).into_response();
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert("referrer-policy", "no-referrer".parse().unwrap());
    response
}
pub(super) async fn complete(
    State(a): State<App>,
    headers: HeaderMap,
    Json(request): Json<StartRequest>,
) -> Response {
    json_result(
        async {
            origin(&a, &headers)?;
            token(&request.token)?;
            let browser = a.browser_links.browser(&headers)?;
            let mut browser = browser.lock().await;
            let (expected, _, pending) = browser
                .completion
                .as_ref()
                .context("link completion could not be confirmed; run /link again")?;
            ensure!(
                *expected == hash(&request.token) && pending.used.load(Ordering::SeqCst) == 3,
                "link completion receipt rejected"
            );
            let _authentication = a.authentication.read().await;
            ensure!(
                a.store.linked_account(pending.discord.clone()).await?
                    == pending.account.load(Ordering::SeqCst),
                "account was unlinked; run /link again"
            );
            let (_, name, _) = browser.completion.take().unwrap();
            Ok(serde_json::json!({"name":name}))
        }
        .await,
    )
}
mod url {
    pub(super) fn encoded(key: &str, value: &str) -> String {
        let mut u = reqwest::Url::parse("https://unused.invalid").unwrap();
        u.query_pairs_mut().append_pair(key, value);
        u.query().unwrap().to_owned()
    }
}

#[cfg(test)]
mod tests;
