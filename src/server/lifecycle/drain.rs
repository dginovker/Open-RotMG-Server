use super::*;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const DEADLINE: Duration = Duration::from_secs(15);

pub(crate) struct Admission {
    state: Mutex<AdmissionState>,
    stop: tokio::sync::watch::Sender<u64>,
}
pub(super) struct AdmissionState {
    instance: String,
    pub(super) generation: u64,
    current: Option<DrainStatus>,
    entries: usize,
    http_entries: usize,
    failure: Option<String>,
}
impl Default for Admission {
    fn default() -> Self {
        use rand::RngCore;
        let mut bytes = [0; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        Self {
            state: Mutex::new(AdmissionState {
                instance: bytes.iter().map(|b| format!("{b:02x}")).collect(),
                generation: 0,
                current: None,
                entries: 0,
                http_entries: 0,
                failure: None,
            }),
            stop: tokio::sync::watch::channel(0).0,
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Receipt {
    instance: String,
    generation: u64,
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct DrainStatus {
    receipt: Receipt,
    state: &'static str,
    errors: Vec<String>,
}

// The same lock covers every controller/HTTP entry and the claim insertion below.
tokio::task_local! { static ADMITTED: (Arc<Admission>, u64); }
pub(crate) struct Entry {
    admission: Arc<Admission>,
    generation: u64,
    http: bool,
}
impl Entry {
    pub(crate) async fn scope<F: Future>(self, work: F) -> F::Output {
        ADMITTED
            .scope((self.admission.clone(), self.generation), async {
                let _entry = self;
                work.await
            })
            .await
    }
}
fn current_entry(a: &App, state: &AdmissionState) -> Result<()> {
    ensure!(
        !ADMITTED
            .try_with(
                |(admission, generation)| Arc::ptr_eq(admission, &a.admission)
                    && *generation != state.generation
            )
            .unwrap_or(false),
        controller("accepted entry interrupted by drain")
    );
    Ok(())
}
impl Drop for Entry {
    fn drop(&mut self) {
        let mut state = self.admission.state.lock().expect("admission poisoned");
        state.entries -= 1;
        if self.http {
            state.http_entries -= 1;
        }
    }
}
pub(crate) fn entry(a: &App) -> Result<Entry> {
    enter(a, false)
}
fn enter(a: &App, http: bool) -> Result<Entry> {
    let mut state = a.admission.state.lock().expect("admission poisoned");
    ensure!(
        state.current.is_none() && !*a.shutdown.borrow(),
        controller("server admission is frozen")
    );
    current_entry(a, &state)?;
    // Controller concurrency is already bounded by connections/claims, independently of HTTP load.
    if http {
        ensure!(
            state.http_entries < a.max_players * 2,
            controller("accepted HTTP entry limit reached")
        );
        state.http_entries += 1;
    }
    state.entries += 1;
    Ok(Entry {
        admission: a.admission.clone(),
        generation: state.generation,
        http,
    })
}
pub(crate) fn stopping(a: &App) -> tokio::sync::watch::Receiver<u64> {
    a.admission.stop.subscribe()
}
pub(super) fn controller_admission(
    a: &App,
    expected: u64,
) -> Result<std::sync::MutexGuard<'_, AdmissionState>> {
    let state = a.admission.state.lock().expect("admission poisoned");
    ensure!(
        state.current.is_none() && state.generation == expected && !*a.shutdown.borrow(),
        controller("controller entry interrupted by drain")
    );
    Ok(state)
}
pub(crate) fn open(a: &App) -> bool {
    a.admission
        .state
        .lock()
        .expect("admission poisoned")
        .current
        .is_none()
        && !*a.shutdown.borrow()
}
pub(super) fn admission(a: &App) -> Result<std::sync::MutexGuard<'_, AdmissionState>> {
    let state = a.admission.state.lock().expect("admission poisoned");
    ensure!(
        state.current.is_none() && !*a.shutdown.borrow(),
        controller("server admission is frozen")
    );
    current_entry(a, &state)?;
    Ok(state)
}

pub(crate) fn prepare(a: &App) -> DrainStatus {
    let mut state = a.admission.state.lock().expect("admission poisoned");
    if let Some(current) = &state.current {
        return current.clone();
    }
    state.generation = state
        .generation
        .checked_add(1)
        .expect("drain generation exhausted");
    let status = DrainStatus {
        receipt: Receipt {
            instance: state.instance.clone(),
            generation: state.generation,
        },
        state: "preparing",
        errors: Vec::new(),
    };
    state.current = Some(status.clone());
    a.admission.stop.send_replace(state.generation);
    let deadline = tokio::time::Instant::now() + DEADLINE;
    let app = a.clone();
    let receipt = status.receipt.clone();
    tokio::spawn(async move {
        let result = AssertUnwindSafe(drain(&app, deadline)).catch_unwind().await;
        let errors = match result {
            Ok(errors) => errors,
            Err(_) => vec!["drain supervisor panicked; accepted ownership retained".into()],
        };
        let mut state = app.admission.state.lock().expect("admission poisoned");
        if let Some(current) = &mut state.current
            && current.receipt == receipt
        {
            current.state = if errors.is_empty() { "ready" } else { "failed" };
            current.errors = errors;
        }
    });
    status
}
fn status(a: &App, receipt: &Receipt) -> Result<DrainStatus> {
    let state = a.admission.state.lock().expect("admission poisoned");
    let current = state.current.as_ref().context("no active drain")?;
    ensure!(current.receipt == *receipt, "stale drain receipt");
    Ok(current.clone())
}
fn cancel(a: &App, receipt: &Receipt) -> Result<DrainStatus> {
    let mut state = a.admission.state.lock().expect("admission poisoned");
    ensure!(
        !*a.shutdown.borrow(),
        "server termination cannot be cancelled"
    );
    let current = state.current.as_ref().context("no active drain")?;
    ensure!(current.receipt == *receipt, "stale drain receipt");
    // Cancellation cannot leave a drain supervisor free to retire newly admitted controllers.
    ensure!(
        current.state != "preparing",
        "drain is still settling; inspect status before cancellation"
    );
    let mut result = state.current.take().expect("checked drain");
    result.state = "cancelled";
    Ok(result)
}
pub(crate) async fn wait(a: &App, receipt: &Receipt) -> Result<DrainStatus> {
    loop {
        let current = status(a, receipt)?;
        if current.state != "preparing" {
            return Ok(current);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
pub(crate) async fn terminate(a: &App) -> Result<()> {
    let prepared = prepare(a);
    let result = wait(a, &prepared.receipt).await?;
    ensure!(
        result.state == "ready",
        "shutdown drain failed: {}",
        result.errors.join("; ")
    );
    Ok(())
}

async fn drain(a: &App, deadline: tokio::time::Instant) -> Vec<String> {
    let mut closing = false;
    loop {
        let entries = a
            .admission
            .state
            .lock()
            .expect("admission poisoned")
            .entries;
        if entries == 0 && !closing {
            closing = true;
            if let Err(error) = super::super::close_scripts(a) {
                return vec![format!("script drain: {error:#}")];
            }
            let avatars = {
                let owners = a.owners.lock().expect("ownership registry poisoned");
                for claim in owners.values() {
                    if let Some(control) = &claim.control {
                        control.cancel.send_replace(true);
                    }
                }
                owners
                    .iter()
                    .filter_map(|(id, c)| {
                        c.avatar.map(|avatar| (*id, c.generation.clone(), avatar))
                    })
                    .collect::<Vec<_>>()
            };
            for (account, generation, avatar) in avatars {
                if let Err(error) = begin_finalization(
                    a,
                    account,
                    &generation,
                    avatar,
                    CloseReason::Shutdown,
                    &tokio::runtime::Handle::current(),
                    None,
                ) {
                    // A concurrent canonical close may have already published and released this claim.
                    let owners = a.owners.lock().expect("ownership registry poisoned");
                    if owners
                        .get(&account)
                        .is_some_and(|c| Arc::ptr_eq(&c.generation, &generation))
                    {
                        return vec![format!(
                            "account={account} avatar={avatar} drain finalization: {error:#}"
                        )];
                    }
                }
            }
        }
        let (empty, settled, mut errors, detail) = {
            let owners = a.owners.lock().expect("ownership registry poisoned");
            let errors = owners
                .values()
                .filter_map(|c| c.failure.clone())
                .collect::<Vec<_>>();
            let settled = owners.values().all(|c| {
                !c.pending
                    && c.finalization
                        .as_ref()
                        .is_none_or(|f| f.done.borrow().is_some())
            });
            let detail = owners.iter().map(|(id,c)| format!("account={id} session={:p} avatar={:?} pending={} owner_present={} closing={}", Arc::as_ptr(&c.generation), c.avatar, c.pending, c.owner_present, c.finalization.is_some())).collect::<Vec<_>>().join("; ");
            (owners.is_empty(), settled, errors, detail)
        };
        errors.extend(
            a.admission
                .state
                .lock()
                .expect("admission poisoned")
                .failure
                .clone(),
        );
        if entries == 0 && empty {
            return errors;
        }
        if closing && settled && !errors.is_empty() {
            return errors;
        }
        if tokio::time::Instant::now() >= deadline {
            errors.push(format!(
                "drain deadline exceeded; entries={entries}; {detail}; accepted work remains owned"
            ));
            return errors;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route("/internal/lifecycle/prepare", post(prepare_request))
        .route("/internal/lifecycle/status", post(status_request))
        .route("/internal/lifecycle/cancel", post(cancel_request))
}
async fn prepare_request(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if super::super::hiscores::authorize(&a, peer, &headers).is_err() {
        return super::super::hiscores::json_error(
            StatusCode::FORBIDDEN,
            "lifecycle authorization denied",
        );
    }
    Json(prepare(&a)).into_response()
}
async fn status_request(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(receipt): Json<Receipt>,
) -> Response {
    receipt_request(&a, peer, &headers, &receipt, false)
}
async fn cancel_request(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(receipt): Json<Receipt>,
) -> Response {
    receipt_request(&a, peer, &headers, &receipt, true)
}
fn receipt_request(
    a: &App,
    peer: SocketAddr,
    headers: &HeaderMap,
    receipt: &Receipt,
    cancelling: bool,
) -> Response {
    if super::super::hiscores::authorize(a, peer, headers).is_err() {
        return super::super::hiscores::json_error(
            StatusCode::FORBIDDEN,
            "lifecycle authorization denied",
        );
    }
    match if cancelling {
        cancel(a, receipt)
    } else {
        status(a, receipt)
    } {
        Ok(status) => Json(status).into_response(),
        Err(error) => super::super::hiscores::json_error(StatusCode::CONFLICT, error),
    }
}

pub(crate) async fn http_entry(
    State(a): State<App>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let guard = match enter(&a, true) {
        Ok(guard) => guard,
        Err(error) => {
            return super::super::hiscores::json_error(StatusCode::SERVICE_UNAVAILABLE, error);
        }
    };
    // Accepted HTTP work (including pre-account guest creation) outlives a disconnected caller.
    match tokio::spawn(guard.scope(async move {
        let result = AssertUnwindSafe(next.run(request)).catch_unwind().await;
        match result {
            Ok(response) => response,
            Err(_) => {
                let error = "accepted HTTP handler panicked; outcome uncertain";
                a.admission
                    .state
                    .lock()
                    .expect("admission poisoned")
                    .failure
                    .get_or_insert_with(|| error.into());
                report_failure(error, "HTTP completion fenced");
                super::super::hiscores::json_error(StatusCode::INTERNAL_SERVER_ERROR, error)
            }
        }
    }))
    .await
    {
        Ok(response) => response,
        Err(error) => super::super::hiscores::json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("accepted HTTP task failed: {error}"),
        ),
    }
}
