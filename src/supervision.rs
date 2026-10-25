use anyhow::{Context, Result, bail, ensure};
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    future::Future,
    panic::AssertUnwindSafe,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
pub struct ContextIds {
    world: Option<i32>,
    tick: Option<u64>,
    npc: Option<(i32, u16)>,
}
thread_local! {
    static CONTEXT: Cell<ContextIds> = const { Cell::new(ContextIds { world: None, tick: None, npc: None }) };
    static PANIC: RefCell<Option<(String, ContextIds)>> = const { RefCell::new(None) };
}
pub struct Scope(ContextIds);
impl Drop for Scope {
    fn drop(&mut self) {
        CONTEXT.set(self.0);
    }
}
pub fn world(world: i32, tick: u64) -> Scope {
    Scope(CONTEXT.replace(ContextIds {
        world: Some(world),
        tick: Some(tick),
        npc: None,
    }))
}
pub fn npc(id: i32, kind: u16) -> Scope {
    let old = CONTEXT.get();
    CONTEXT.set(ContextIds {
        npc: Some((id, kind)),
        ..old
    });
    Scope(old)
}
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|p| format!("{}:{}:{}", p.file(), p.line(), p.column()));
        PANIC.with_borrow_mut(|value| *value = location.map(|p| (p, CONTEXT.get())));
        previous(info);
    }));
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Incident {
    pub id: String,
    pub unix_ms: u64,
    pub process: u32,
    pub revision: Option<String>,
    pub component: String,
    pub failure: String,
    pub location: Option<String>,
    pub context: ContextIds,
    pub fences_preserved: bool,
    pub fenced_accounts: Vec<i32>,
}
#[derive(Clone)]
pub struct Supervisor {
    fatal: watch::Sender<Option<Incident>>,
    path: Option<PathBuf>,
}
impl Default for Supervisor {
    fn default() -> Self {
        Self {
            fatal: watch::channel(None).0,
            path: None,
        }
    }
}
impl Supervisor {
    pub fn persisted(path: PathBuf) -> Result<Self> {
        let value = Self {
            path: Some(path),
            ..Self::default()
        };
        if let Some(incident) = value.latest()? {
            ensure!(
                incident.fences_preserved,
                "critical incident {} has incomplete account-fence preservation; operator inspection required",
                incident.id
            );
        }
        Ok(value)
    }
    fn save(&self, incident: &Incident) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        use std::io::Write;
        let temporary = path.with_extension("json.new");
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(&serde_json::to_vec(incident)?)?;
        file.sync_all()?;
        std::fs::rename(temporary, path)?;
        std::fs::File::open(path.parent().context("incident directory missing")?)?.sync_all()?;
        Ok(())
    }
    pub fn ensure_running(&self) -> Result<()> {
        ensure!(
            self.fatal.borrow().is_none(),
            "critical worker stopped; new ownership operations refused"
        );
        Ok(())
    }
    pub fn account_fenced(&self, account: i32) -> Result<bool> {
        Ok(self
            .latest()?
            .is_some_and(|incident| incident.fenced_accounts.contains(&account)))
    }
    pub fn check_account(&self, account: i32) -> Result<()> {
        ensure!(
            !self.account_fenced(account)?,
            "account ownership is durably fenced; operator inspection required"
        );
        self.ensure_running()
    }
    pub fn preserve_accounts(&self, incident: &mut Incident, accounts: &[i32]) -> Result<()> {
        incident.fenced_accounts.extend_from_slice(accounts);
        incident.fenced_accounts.sort_unstable();
        incident.fenced_accounts.dedup();
        incident.fences_preserved = true;
        self.save(incident)
    }
    pub fn spawn(
        &self,
        component: &'static str,
        work: impl Future<Output = Result<()>> + Send + 'static,
    ) {
        self.spawn_inner(component, work, false);
    }
    pub fn spawn_finite_on(
        &self,
        runtime: &tokio::runtime::Handle,
        component: &'static str,
        work: impl Future<Output = Result<()>> + Send + 'static,
    ) {
        self.spawn_on(runtime, component, work, true);
    }
    fn spawn_inner(
        &self,
        component: &'static str,
        work: impl Future<Output = Result<()>> + Send + 'static,
        finite: bool,
    ) {
        self.spawn_on(&tokio::runtime::Handle::current(), component, work, finite);
    }
    fn spawn_on(
        &self,
        runtime: &tokio::runtime::Handle,
        component: &'static str,
        work: impl Future<Output = Result<()>> + Send + 'static,
        finite: bool,
    ) {
        let supervisor = self.clone();
        runtime.spawn(async move {
            let outcome = AssertUnwindSafe(work).catch_unwind().await;
            if finite && matches!(outcome, Ok(Ok(()))) {
                return;
            }
            supervisor.terminate(component, outcome);
        });
    }
    pub fn spawn_finite_thread(
        &self,
        component: &'static str,
        work: impl FnOnce() -> Result<()> + Send + 'static,
    ) -> Result<()> {
        let supervisor = self.clone();
        std::thread::Builder::new()
            .name(component.into())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(AssertUnwindSafe(work));
                if !matches!(outcome, Ok(Ok(()))) {
                    supervisor.terminate(component, outcome);
                }
            })
            .context("critical worker thread could not start")?;
        Ok(())
    }
    pub fn blocking_work<R>(&self, component: &'static str, work: impl FnOnce() -> R) -> Result<R> {
        match std::panic::catch_unwind(AssertUnwindSafe(work)) {
            Ok(value) => Ok(value),
            Err(panic) => {
                self.terminate(component, Err(panic));
                bail!("critical {component} worker panicked")
            }
        }
    }
    pub fn failure(&self, component: &'static str, error: anyhow::Error) {
        self.terminate(component, Ok(Err(error)));
    }
    fn terminate(&self, component: &'static str, outcome: std::thread::Result<Result<()>>) {
        // Unwinding is observed only to terminate the service; the damaged worker is never resumed.
        let (failure, detail, panic) = match outcome {
            Ok(Ok(())) => (
                "exited",
                "critical worker exited unexpectedly".to_owned(),
                None,
            ),
            Ok(Err(error)) => ("failed", format!("{error:#}"), None),
            Err(error) => {
                let detail = error
                    .downcast_ref::<&str>()
                    .map(|v| (*v).to_owned())
                    .or_else(|| error.downcast_ref::<String>().cloned());
                (
                    "panicked",
                    format!("{detail:?}"),
                    PANIC.with_borrow_mut(Option::take),
                )
            }
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before Unix epoch")
            .as_millis() as u64;
        let mut incident = Incident {
            id: format!("{}-{now}", std::process::id()),
            unix_ms: now,
            process: std::process::id(),
            revision: option_env!("GITHUB_SHA").map(str::to_owned),
            component: component.into(),
            failure: failure.into(),
            location: panic.as_ref().map(|v| v.0.clone()),
            context: panic.map(|v| v.1).unwrap_or_default(),
            fences_preserved: false,
            fenced_accounts: Vec::new(),
        };
        tracing::error!(component, failure, error=%detail, incident=%incident.id, "critical worker terminated");
        self.fatal.send_if_modified(|current| {
                if current.is_some() { return false; }
                let mut save = || -> Result<()> {
                    if let Some(previous) = self.latest()? {
                        incident.fenced_accounts = previous.fenced_accounts;
                    }
                    self.save(&incident)
                };
                if let Err(error) = save() {
                    tracing::error!(%error, "critical incident persistence failed; automatic restart forbidden");
                    std::process::exit(78);
                }
                *current = Some(incident);
                true
            });
    }
    pub async fn failed(&self) -> Incident {
        let mut receiver = self.fatal.subscribe();
        loop {
            if let Some(incident) = receiver.borrow().clone() {
                return incident;
            }
            receiver
                .changed()
                .await
                .expect("supervisor sender retained");
        }
    }
    pub fn latest(&self) -> Result<Option<Incident>> {
        let Some(path) = &self.path else {
            return Ok(None);
        };
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(
                serde_json::from_slice(&bytes).context("invalid saved critical incident")?,
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => bail!("cannot read critical incident: {error}"),
        }
    }
}
