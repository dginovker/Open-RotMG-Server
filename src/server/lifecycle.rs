pub(crate) mod drain;
use super::*;
use futures_util::FutureExt;
use std::{future::Future, panic::AssertUnwindSafe};
use tracing::instrument::WithSubscriber;

fn report_failure(message: &str, context: &'static str) {
    // A lost handoff drops its guard on the World worker; reporting must not kill that worker.
    if std::panic::catch_unwind(AssertUnwindSafe(|| {
        tracing::error!(error=%message, "{context}");
    }))
    .is_err()
    {
        use std::io::Write;
        let _ = writeln!(
            std::io::stderr(),
            "lifecycle diagnostic sink panicked; {context}: {message}"
        );
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    Preparation,
    Reservation,
    Store,
    Publication,
    Retirement,
    Rejected,
    Settled,
    Panic,
}
impl Stage {
    fn uncertain(self) -> bool {
        !matches!(self, Self::Preparation | Self::Rejected | Self::Settled)
    }
}
impl std::fmt::Display for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Preparation => "preparation",
            Self::Reservation => "reservation",
            Self::Store => "store",
            Self::Publication => "publication",
            Self::Retirement => "retirement",
            Self::Rejected => "rejected",
            Self::Settled => "settled",
            Self::Panic => "panic",
        })
    }
}

type Completion = Option<std::result::Result<(), String>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseReason {
    Protocol,
    Transport,
    Controller,
    Output,
    Deadline,
    Shutdown,
    Internal,
}
impl std::fmt::Display for CloseReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for CloseReason {}
pub(crate) fn protocol(message: &'static str) -> anyhow::Error {
    anyhow::Error::new(CloseReason::Protocol).context(message)
}
pub(crate) fn controller(message: &'static str) -> anyhow::Error {
    anyhow::Error::new(CloseReason::Controller).context(message)
}
pub(crate) fn rule_rejection(error: &anyhow::Error) -> bool {
    error.downcast_ref::<CloseReason>() == Some(&CloseReason::Controller)
        || error.is::<crate::store::Rejected>()
        || error.is::<crate::store::SlotReplay>()
}
fn rejected(error: &anyhow::Error, stage: Stage) -> bool {
    error.is::<crate::store::Rejected>()
        || error.is::<crate::store::SlotReplay>()
        || matches!(stage, Stage::Preparation | Stage::Reservation)
            && error
                .downcast_ref::<CloseReason>()
                .is_some_and(|r| matches!(r, CloseReason::Protocol | CloseReason::Controller))
}

#[derive(Debug)]
pub(crate) struct NestedFinalization;
impl std::fmt::Display for NestedFinalization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("finalization must begin outside an owned account operation")
    }
}
impl std::error::Error for NestedFinalization {}

#[derive(Clone)]
struct Finalization {
    avatar: i32,
    transfer: Option<Vec<u8>>,
    done: tokio::sync::watch::Receiver<Completion>,
}
struct FinalizationGuard {
    registry: Arc<Mutex<BTreeMap<i32, Claim>>>,
    expected: Vec<(i32, Arc<()>)>,
    completion: tokio::sync::watch::Sender<Completion>,
    transfer: Option<crate::world::TransferReservation>,
    finished: bool,
    #[cfg(test)]
    _task: LiveTask,
}
impl Drop for FinalizationGuard {
    fn drop(&mut self) {
        if let Some(transfer) = &self.transfer {
            transfer.revoke();
        }
        if !self.finished {
            let error = "finalization executor abandoned completion".to_string();
            fence_owned(
                &self.registry,
                &self.expected,
                "finalization",
                Stage::Retirement,
                error.clone(),
            );
            self.completion.send_replace(Some(Err(error)));
        }
    }
}
pub(crate) struct Finalized(tokio::sync::watch::Receiver<Completion>);
impl Finalized {
    pub(crate) async fn wait(mut self) -> Result<()> {
        loop {
            if let Some(result) = self.0.borrow().clone() {
                return result.map_err(anyhow::Error::msg);
            }
            self.0
                .changed()
                .await
                .context("finalization supervisor lost its result")?;
        }
    }
}

struct LeaseCleanup {
    app: App,
    runtime: tokio::runtime::Handle,
}

#[derive(Clone)]
pub(crate) struct SessionControl {
    autonomous: bool,
    account: i32,
    generation: Arc<()>,
    registry: std::sync::Weak<Mutex<BTreeMap<i32, Claim>>>,
    pub(super) cancel: tokio::sync::watch::Sender<bool>,
    pub(super) done: tokio::sync::watch::Receiver<Completion>,
    completion: tokio::sync::watch::Sender<Completion>,
}

impl SessionControl {
    pub(crate) fn complete(self, result: Result<()>) {
        if !self.autonomous {
            self.publish(result);
            return;
        }
        let Some(registry) = self.registry.upgrade() else {
            self.publish(Err(anyhow::anyhow!(
                "controller completion lost ownership registry"
            )));
            report_failure(
                "controller completion lost ownership registry",
                "autonomous completion failed",
            );
            return;
        };
        let mut owners = registry.lock().expect("ownership registry poisoned");
        let Some(claim) = owners
            .get_mut(&self.account)
            .filter(|c| Arc::ptr_eq(&c.generation, &self.generation))
        else {
            let error = format!(
                "account={} stale autonomous completion generation",
                self.account
            );
            report_failure(&error, "autonomous completion failed");
            self.publish(Err(anyhow::Error::msg(error)));
            return;
        };
        if let Err(error) = &result {
            let message = fence(
                claim,
                self.account,
                "autonomous_completion",
                Stage::Retirement,
                format!("{error:#}"),
            );
            report_failure(&message, "autonomous completion failed");
        }
        self.publish(result);
        finish(&mut owners, self.account);
    }
    fn publish(&self, result: Result<()>) {
        self.completion.send_if_modified(|current| {
            if current.is_some() {
                return false;
            }
            *current = Some(result.map_err(|error| format!("{error:#}")));
            true
        });
    }
}

pub struct Claim {
    generation: Arc<()>,
    control: Option<SessionControl>,
    avatar: Option<i32>,
    script_avatar: Option<i32>,
    pending: bool,
    owner_present: bool,
    failure: Option<String>,
    idle: Arc<tokio::sync::Notify>,
    finalization: Option<Finalization>,
}

impl Claim {
    pub(super) fn matches_avatar(&self, avatar: i32) -> bool {
        self.avatar == Some(avatar)
            || self
                .finalization
                .as_ref()
                .is_some_and(|f| f.avatar == avatar)
    }
}

pub struct Owner {
    registry: Arc<Mutex<BTreeMap<i32, Claim>>>,
    pub(super) id: i32,
    generation: Arc<()>,
    pub(super) registered: bool,
    admitted: u64,
    pub(super) cancel: Option<tokio::sync::watch::Receiver<bool>>,
    cleanup: Option<LeaseCleanup>,
}

fn finish(owners: &mut BTreeMap<i32, Claim>, id: i32) {
    let Some(claim) = owners.get(&id) else { return };
    if !claim.owner_present
        && !claim.pending
        && claim.avatar.is_none()
        && claim.failure.is_none()
        && claim.control.as_ref().is_none_or(|control| {
            !control.autonomous || matches!(*control.done.borrow(), Some(Ok(())))
        })
        && claim
            .finalization
            .as_ref()
            .is_none_or(|f| matches!(*f.done.borrow(), Some(Ok(()))))
    {
        let claim = owners.remove(&id).expect("checked claim");
        if let Some(control) = claim.control
            && !control.autonomous
        {
            control.complete(Ok(()));
        }
    }
}

impl Owner {
    pub(crate) fn control(
        &mut self,
        autonomous: bool,
    ) -> Result<(tokio::sync::watch::Receiver<bool>, SessionControl)> {
        let (cancel, receiver) = tokio::sync::watch::channel(false);
        let (completion, done) = tokio::sync::watch::channel(None);
        let mut owners = self.registry.lock().expect("ownership registry poisoned");
        let claim = owners
            .get_mut(&self.id)
            .context("controller has no account claim")?;
        ensure!(
            Arc::ptr_eq(&claim.generation, &self.generation),
            "stale controller generation"
        );
        ensure!(claim.control.is_none(), "controller already registered");
        let control = SessionControl {
            autonomous,
            account: self.id,
            generation: self.generation.clone(),
            registry: Arc::downgrade(&self.registry),
            cancel,
            done,
            completion,
        };
        claim.control = Some(control.clone());
        self.cancel = Some(receiver.clone());
        Ok((receiver, control))
    }
    pub(crate) fn admit_controller(&self, a: &App) -> Result<()> {
        if let Err(error) = drain::controller_admission(a, self.admitted) {
            stage(Stage::Settled);
            return Err(error);
        }
        Ok(())
    }
    // A map-held Owner must move into owned work first, so its App clone cannot point back to it.
    pub(crate) fn arm_cleanup(&mut self, app: &App, runtime: tokio::runtime::Handle) {
        self.cleanup = Some(LeaseCleanup {
            app: app.clone(),
            runtime,
        });
    }
    pub(super) fn publish_script(&mut self, avatar: i32) -> Result<()> {
        let app = self
            .cleanup
            .as_ref()
            .context("script entry cleanup is not armed")?
            .app
            .clone();
        let admitted = drain::controller_admission(&app, self.admitted);
        let mut owners = self.registry.lock().expect("ownership registry poisoned");
        let claim = owners
            .get_mut(&self.id)
            .context("script has no account claim")?;
        ensure!(
            Arc::ptr_eq(&claim.generation, &self.generation),
            "stale script generation"
        );
        ensure!(
            claim.avatar == Some(avatar),
            "script avatar differs from account lease"
        );
        let admission = match admitted {
            Ok(admission) => admission,
            Err(error) => {
                // The bound reply is acknowledged; armed cleanup inherits retirement, not a live controller.
                stage(Stage::Settled);
                return Err(error);
            }
        };
        claim.script_avatar = Some(avatar);
        drop(admission);
        self.cleanup = None;
        Ok(())
    }
    pub(crate) fn close(self, app: &App, reason: CloseReason) -> Result<Option<Finalized>> {
        let result = (|| {
            ensure!(!nested_operation(), NestedFinalization);
            let runtime = tokio::runtime::Handle::try_current()
                .context("session close requires a running server executor")?;
            let avatar = {
                let owners = self.registry.lock().expect("ownership registry poisoned");
                let claim = owners
                    .get(&self.id)
                    .context("session close has no account claim")?;
                ensure!(
                    Arc::ptr_eq(&claim.generation, &self.generation),
                    "stale session close generation"
                );

                ensure!(
                    claim.avatar.is_some() || !claim.pending,
                    "cannot close an unpublished accepted entry; custody must remain in owned entry work"
                );
                let avatar = claim
                    .avatar
                    .or_else(|| claim.finalization.as_ref().map(|f| f.avatar));
                ensure!(
                    claim.failure.is_none() || claim.avatar.is_some(),
                    "cannot close fenced account without an avatar: {:?}",
                    claim.failure
                );
                ensure!(
                    avatar.is_some() || !claim.pending,
                    "cannot close an unpublished accepted entry"
                );
                avatar
            };
            avatar
                .map(|avatar| {
                    begin_finalization(
                        app,
                        self.id,
                        &self.generation,
                        avatar,
                        reason,
                        &runtime,
                        None,
                    )
                })
                .transpose()
        })();
        if let Err(error) = &result {
            self.failed(format!("{error:#}"));
        }
        drop(self);
        result
    }
    pub(super) fn bind(&self, avatar: i32) -> Result<()> {
        bind(&self.registry, self.id, &self.generation, avatar)
    }
    pub(super) fn release(self) {
        drop(self);
    }
    pub(crate) fn failed(&self, error: String) {
        fence_owned(
            &self.registry,
            &[(self.id, self.generation.clone())],
            "session",
            Stage::Retirement,
            error,
        );
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        let mut owners = self.registry.lock().expect("ownership registry poisoned");
        let Some(claim) = owners
            .get_mut(&self.id)
            .filter(|c| Arc::ptr_eq(&c.generation, &self.generation))
        else {
            return;
        };
        claim.owner_present = false;
        let avatar = claim.avatar;
        let diagnostic = if self.cleanup.is_none()
            && claim.finalization.is_none()
            && claim.avatar.is_some()
            && !claim.pending
            && claim.failure.is_none()
        {
            let message = format!(
                "account={} session={:p} avatar={:?} operation=session stage=retirement: lease retained without acknowledged retirement",
                self.id,
                Arc::as_ptr(&self.generation),
                claim.avatar
            );
            if let Some(control) = &claim.control {
                control.completion.send_replace(Some(Err(message.clone())));
            }
            Some(message)
        } else {
            None
        };
        finish(&mut owners, self.id);
        drop(owners);
        if let (Some(cleanup), Some(avatar)) = (&self.cleanup, avatar)
            && let Err(error) = begin_finalization(
                &cleanup.app,
                self.id,
                &self.generation,
                avatar,
                CloseReason::Transport,
                &cleanup.runtime,
                None,
            )
        {
            fence_owned(
                &self.registry,
                &[(self.id, self.generation.clone())],
                "native_drop",
                Stage::Retirement,
                format!("{error:#}"),
            );
        }
        if !std::thread::panicking()
            && let Some(message) = diagnostic
        {
            report_failure(&message, "loaded lease retained");
        }
    }
}
fn bind(
    registry: &Arc<Mutex<BTreeMap<i32, Claim>>>,
    account: i32,
    generation: &Arc<()>,
    avatar: i32,
) -> Result<()> {
    let mut owners = registry.lock().expect("ownership registry poisoned");
    let claim = owners
        .get_mut(&account)
        .context("avatar has no account claim")?;
    ensure!(
        Arc::ptr_eq(&claim.generation, generation),
        "stale session generation"
    );
    ensure!(
        claim.avatar.is_none_or(|id| id == avatar),
        "account already owns another avatar"
    );
    ensure!(claim.failure.is_none(), "account ownership is fenced");
    if claim.avatar.is_none() {
        if let Some(finalization) = &claim.finalization {
            ensure!(
                matches!(*finalization.done.borrow(), Some(Ok(()))),
                "previous avatar finalization is not complete"
            );
        }
        claim.finalization = None;
        claim.script_avatar = None;
    }
    claim.avatar = Some(avatar);
    Ok(())
}
fn fence(claim: &mut Claim, account: i32, operation: &str, stage: Stage, error: String) -> String {
    let message = format!(
        "account={account} session={:p} avatar={:?} operation={operation} stage={stage}: {error}",
        Arc::as_ptr(&claim.generation),
        claim.avatar
    );
    if claim.failure.is_none() {
        claim.failure = Some(message.clone());
    }
    if let Some(control) = &claim.control {
        control
            .completion
            .send_replace(Some(Err(claim.failure.clone().expect("fenced claim"))));
    }
    message
}
fn fence_owned(
    registry: &Arc<Mutex<BTreeMap<i32, Claim>>>,
    generations: &[(i32, Arc<()>)],
    operation: &str,
    stage: Stage,
    error: String,
) {
    let reports = {
        let mut owners = registry.lock().expect("ownership registry poisoned");
        generations
            .iter()
            .filter_map(|(account, generation)| {
                owners
                    .get_mut(account)
                    .filter(|c| Arc::ptr_eq(&c.generation, generation))
                    .map(|claim| fence(claim, *account, operation, stage, error.clone()))
            })
            .collect::<Vec<_>>()
    };
    for message in reports {
        report_failure(&message, "owned completion fenced");
    }
}
fn generations(a: &App, accounts: &[i32]) -> Result<Vec<(i32, Arc<()>)>> {
    let owners = a.owners.lock().expect("ownership registry poisoned");
    accounts
        .iter()
        .map(|id| {
            Ok((
                *id,
                owners
                    .get(id)
                    .context("operation has no account claim")?
                    .generation
                    .clone(),
            ))
        })
        .collect()
}

impl App {
    pub(crate) fn claim(&self, id: i32) -> Result<Owner> {
        self.try_claim(id)?.context("development world full")
    }
    pub(crate) fn try_claim(&self, id: i32) -> Result<Option<Owner>> {
        let _admission = drain::admission(self)?;
        let mut owners = self.owners.lock().expect("ownership registry poisoned");
        self.world.supervisor.check_account(id)?;
        if owners.len() >= self.max_players {
            return Ok(None);
        }
        ensure!(
            !owners.contains_key(&id),
            "account already has an active session"
        );
        let generation = Arc::new(());
        self.admission_started
            .store(true, std::sync::atomic::Ordering::Relaxed);
        owners.insert(
            id,
            Claim {
                generation: generation.clone(),
                control: None,
                avatar: None,
                script_avatar: None,
                pending: false,
                owner_present: true,
                failure: None,
                idle: Arc::default(),
                finalization: None,
            },
        );
        Ok(Some(Owner {
            registry: self.owners.clone(),
            id,
            generation,
            registered: false,
            admitted: _admission.generation,
            cancel: None,
            cleanup: None,
        }))
    }
    pub(super) fn claim_session(&self, id: i32) -> Result<Owner> {
        let mut owner = self.claim(id)?;
        owner.control(false)?;
        Ok(owner)
    }
    pub(crate) async fn stop_persona(&self, account: i32) -> Result<()> {
        self.world.supervisor.check_account(account)?;
        let control = {
            let mut owners = self.owners.lock().expect("ownership registry poisoned");
            match owners.get_mut(&account) {
                None => return Ok(()),
                Some(claim) => {
                    ensure!(
                        claim.failure.is_none(),
                        "persona account is fenced: {:?}",
                        claim.failure
                    );
                    let control = claim.control.clone().ok_or(crate::store::Rejected(
                        "Persona admission is busy; retry name choice shortly",
                    ))?;
                    ensure!(
                        control.autonomous,
                        "name collision is not an autonomous session"
                    );
                    if control.cancel.is_closed() {
                        let message = fence(
                            claim,
                            account,
                            "persona_completion",
                            Stage::Retirement,
                            "controller completion is unavailable; operator inspection required"
                                .into(),
                        );
                        report_failure(&message, "name claim refused");
                        return Err(crate::store::Rejected("Persona controller completion is unavailable; name was not claimed. Operator inspection required.").into());
                    }
                    control.cancel.send_replace(true);
                    control
                }
            }
        };
        wait_control(control).await?;
        self.world.supervisor.check_account(account)?;
        Ok(())
    }
    pub(super) async fn kick(&self, account: i32) -> Result<bool> {
        let control = self
            .owners
            .lock()
            .expect("ownership registry poisoned")
            .get(&account)
            .map(|c| (c.control.clone(), c.avatar));
        let Some((control, avatar)) = control else {
            return Ok(false);
        };
        let control =
            control.context("Account is busy outside a game connection; retry shortly")?;
        if !control.autonomous
            && let Some(avatar) = avatar
        {
            finalize(self, avatar, CloseReason::Controller)?;
        }
        control.cancel.send_replace(true);
        wait_control(control).await?;
        Ok(true)
    }
}

async fn wait_control(mut control: SessionControl) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            if let Some(result) = control.done.borrow().clone() {
                result.map_err(anyhow::Error::msg)?;
                return Ok(());
            }
            control
                .done
                .changed()
                .await
                .context("Session closed without a save acknowledgement")?;
        }
    })
    .await
    .context("Disconnect save is still pending; no successful stop was confirmed")?
}

tokio::task_local! { static FINALIZING: (); }
pub(super) fn nested_operation() -> bool {
    ACCOUNTS
        .try_with(|owned| !owned.is_empty())
        .unwrap_or(false)
}

async fn wait_pending(a: &App, account: i32, generation: &Arc<()>) -> Result<()> {
    loop {
        let wait = {
            let owners = a.owners.lock().expect("ownership registry poisoned");
            let claim = owners.get(&account).context("closing claim missing")?;
            ensure!(
                Arc::ptr_eq(&claim.generation, generation),
                "stale closing generation"
            );
            if claim.pending {
                let mut wait = Box::pin(claim.idle.clone().notified_owned());
                wait.as_mut().enable();
                Some(wait)
            } else {
                None
            }
        };
        match wait {
            Some(wait) => wait.await,
            None => return Ok(()),
        }
    }
}
async fn stop_owned_avatar(a: &App, account: i32, generation: &Arc<()>, avatar: i32) -> Result<()> {
    let registry = a.owners.clone();
    let generation = generation.clone();
    a.world
        .directory(move |d| {
            let owners = registry.lock().expect("ownership registry poisoned");
            let claim = owners
                .get(&account)
                .context("controller stop has no claim")?;
            ensure!(
                Arc::ptr_eq(&claim.generation, &generation),
                "stale controller-stop generation"
            );
            ensure!(claim.avatar == Some(avatar), "stale controller-stop avatar");
            let world = d
                .worlds
                .get_mut(&d.location(avatar)?)
                .context("controller world missing")?;
            ensure!(
                world.players[&avatar].character.account == account,
                "controller-stop account differs"
            );
            world.stop_controller(avatar)
        })
        .await
}

// The registry owns this result before spawning; dropping any waiter cannot cancel retirement.
fn begin_finalization(
    a: &App,
    account: i32,
    generation: &Arc<()>,
    avatar: i32,
    reason: CloseReason,
    runtime: &tokio::runtime::Handle,
    mut transfer: Option<crate::world::TransferReservation>,
) -> Result<Finalized> {
    let (completion, done) = tokio::sync::watch::channel(None);
    let initial_failure = {
        let mut owners = a.owners.lock().expect("ownership registry poisoned");
        let claim = owners
            .get_mut(&account)
            .context("finalization has no account claim")?;
        ensure!(
            Arc::ptr_eq(&claim.generation, generation),
            "stale finalization generation"
        );
        if let Some(previous) = &claim.finalization {
            ensure!(previous.avatar == avatar, "stale finalization avatar");
            ensure!(
                transfer
                    .as_ref()
                    .is_none_or(|t| previous.transfer.as_deref() == Some(t.key())),
                "finalization belongs to another transfer"
            );
            if let Some(t) = &mut transfer {
                t.disarm();
            }
            return Ok(Finalized(previous.done.clone()));
        }
        ensure!(
            claim.avatar == Some(avatar),
            "finalization avatar differs from account lease"
        );

        claim.finalization = Some(Finalization {
            avatar,
            transfer: transfer.as_ref().map(|t| t.key().to_vec()),
            done: done.clone(),
        });
        claim.failure.clone()
    };
    let app = a.clone();
    let expected = vec![(account, generation.clone())];
    let request = crate::performance::REQUEST.try_with(Arc::clone).ok();
    let mut guard = FinalizationGuard {
        registry: a.owners.clone(),
        expected: expected.clone(),
        completion,
        transfer,
        finished: false,
        #[cfg(test)]
        _task: LiveTask::new(),
    };
    let transfer_key = guard.transfer.as_ref().map(|t| t.key().to_vec());
    let work = async move {
        let mut stop_acknowledged = false;
        let result = AssertUnwindSafe(async {
            tokio::time::timeout(
                std::time::Duration::from_secs(15),
                FINALIZING.scope((), async {
                    let result = if let Some(cause) = &initial_failure {
                        stop_owned_avatar(&app, account, &expected[0].1, avatar).await?;
                        stop_acknowledged = true;
                        wait_pending(&app, account, &expected[0].1).await?;
                        Err(anyhow::Error::msg(cause.clone()))
                    } else {
                        stop_owned_avatar(&app, account, &expected[0].1, avatar).await?;
                        stop_acknowledged = true;
                        super::settle_giver_credits(&app, avatar).await?;
                        self::account(&app, vec![account], "finalization", move |app| async move {
                            super::depart_inner_with_transfer(&app, avatar, transfer_key).await
                        })
                        .await
                    };
                    if let Err(error) = &result {
                        fence_owned(
                            &app.owners,
                            &expected,
                            "finalization",
                            Stage::Retirement,
                            format!("reason={reason}: {error:#}"),
                        );
                        if !stop_acknowledged {
                            stop_owned_avatar(&app, account, &expected[0].1, avatar).await?;
                            stop_acknowledged = true;
                            wait_pending(&app, account, &expected[0].1).await?;
                        }
                    }
                    result
                }),
            )
            .await
            .context("finalization deadline exceeded; accepted work remains owned")?
        })
        .catch_unwind()
        .await;
        let result = match result {
            Ok(result) => result,
            Err(_) => Err(anyhow::anyhow!("finalization supervisor panicked")),
        };
        let result = result.map_err(|error| {
            fence_owned(
                &app.owners,
                &expected,
                "finalization",
                Stage::Retirement,
                format!("reason={reason}: {error:#}"),
            );
            let original = app
                .owners
                .lock()
                .expect("ownership registry poisoned")
                .get(&account)
                .filter(|c| Arc::ptr_eq(&c.generation, &expected[0].1))
                .and_then(|c| c.failure.clone());
            let cause = match original.or(initial_failure) {
                Some(cause) => cause,
                None => format!("{error:#}"),
            };
            let pending = app
                .owners
                .lock()
                .expect("ownership registry poisoned")
                .get(&account)
                .filter(|c| Arc::ptr_eq(&c.generation, &expected[0].1))
                .is_some_and(|c| c.pending);
            format!(
                "{cause}; controller_stop={}; pending_work={pending}; finalization_error={error:#}",
                if stop_acknowledged {
                    "acknowledged"
                } else {
                    "unverified"
                }
            )
        });
        if let Some(transfer) = &mut guard.transfer {
            if result.is_ok() {
                transfer.disarm();
            } else {
                transfer.revoke();
            }
        }
        // Keep closing identity discoverable until the canonical result is actually published.
        let mut owners = app.owners.lock().expect("ownership registry poisoned");
        guard.completion.send_replace(Some(result));
        for (account, generation) in &expected {
            if owners
                .get(account)
                .is_some_and(|c| Arc::ptr_eq(&c.generation, generation))
            {
                finish(&mut owners, *account);
            }
        }
        drop(owners);
        guard.finished = true;
        drop(guard);
    };
    runtime.spawn(
        async move {
            match request {
                Some(request) => crate::performance::REQUEST.scope(request, work).await,
                None => work.await,
            }
        }
        .with_current_subscriber(),
    );
    Ok(Finalized(done))
}

pub(super) fn join_script(a: &App, avatar: i32) -> Result<Finalized> {
    let owners = a.owners.lock().expect("ownership registry poisoned");
    owners
        .values()
        .filter(|c| c.script_avatar == Some(avatar))
        .filter_map(|c| c.finalization.as_ref())
        .find(|f| f.avatar == avatar)
        .map(|f| Finalized(f.done.clone()))
        .context("not a closing script-owned player")
}

pub(super) fn script_finalizations(a: &App) -> Vec<Finalized> {
    let owners = a.owners.lock().expect("ownership registry poisoned");
    owners
        .values()
        .filter(|c| !c.owner_present)
        .filter_map(|c| {
            c.finalization
                .as_ref()
                .filter(|f| c.script_avatar == Some(f.avatar))
        })
        .map(|f| Finalized(f.done.clone()))
        .collect()
}

pub(crate) fn finalize(a: &App, avatar: i32, reason: CloseReason) -> Result<Finalized> {
    finalize_with_transfer(a, avatar, reason, None)
}
pub(crate) fn finalize_transfer(
    a: &App,
    avatar: i32,
    transfer: crate::world::TransferReservation,
) -> Result<Finalized> {
    finalize_with_transfer(a, avatar, CloseReason::Controller, Some(transfer))
}
fn finalize_with_transfer(
    a: &App,
    avatar: i32,
    reason: CloseReason,
    transfer: Option<crate::world::TransferReservation>,
) -> Result<Finalized> {
    ensure!(!nested_operation(), NestedFinalization);
    let runtime = tokio::runtime::Handle::try_current()
        .context("finalization requires a running server executor")?;
    let (account, generation) = {
        let owners = a.owners.lock().expect("ownership registry poisoned");
        owners
            .iter()
            .find(|(_, c)| {
                c.avatar == Some(avatar)
                    || c.finalization.as_ref().is_some_and(|f| f.avatar == avatar)
            })
            .map(|(account, claim)| (*account, claim.generation.clone()))
            .context("finalization avatar has no matching session generation")?
    };
    begin_finalization(a, account, &generation, avatar, reason, &runtime, transfer)
}

pub(super) struct Operation {
    app: App,
    accounts: Vec<(i32, Arc<()>)>,
    operation: &'static str,
    stage: Arc<Mutex<Stage>>,
    completed: bool,
}
tokio::task_local! { static STAGE: Arc<Mutex<Stage>>; static ACCOUNTS: Vec<(i32, Arc<()>)>; }
pub(crate) fn stage(stage: Stage) {
    if tracing::enabled!(tracing::Level::DEBUG)
        && let Ok(accounts) =
            ACCOUNTS.try_with(|owned| owned.iter().map(|(id, _)| *id).collect::<Vec<_>>())
    {
        tracing::debug!(stage=%stage, ?accounts, "owned operation stage");
    }
    let _ = STAGE.try_with(|s| *s.lock().expect("completion stage poisoned") = stage);
}
impl Drop for Operation {
    fn drop(&mut self) {
        let stage = *self.stage.lock().expect("completion stage poisoned");
        let mut owners = self.app.owners.lock().expect("ownership registry poisoned");
        let mut reports = Vec::new();
        for (account, generation) in &self.accounts {
            if let Some(claim) = owners
                .get_mut(account)
                .filter(|c| Arc::ptr_eq(&c.generation, generation))
            {
                claim.pending = false;
                claim.idle.notify_waiters();
                if !self.completed {
                    reports.push(fence(
                        claim,
                        *account,
                        self.operation,
                        stage,
                        "completion task abandoned".into(),
                    ));
                }
                finish(&mut owners, *account);
            }
        }
        drop(owners);
        if !std::thread::panicking() {
            for message in reports {
                report_failure(&message, "owned completion abandoned");
            }
        }
    }
}

pub(crate) async fn account<T, F, Fut>(
    a: &App,
    accounts: Vec<i32>,
    operation: &'static str,
    work: F,
) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(App) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T>> + Send + 'static,
{
    let expected = generations(a, &accounts)?;
    if ACCOUNTS
        .try_with(|owned| {
            expected.iter().all(|(id, generation)| {
                owned
                    .iter()
                    .any(|(current, token)| current == id && Arc::ptr_eq(token, generation))
            })
        })
        .unwrap_or(false)
    {
        let result = work(a.clone()).await;
        if result.is_ok() {
            stage(Stage::Settled);
        }
        return result;
    }
    let guard = loop {
        let wait = {
            let owners = a.owners.lock().expect("ownership registry poisoned");
            for (id, generation) in &expected {
                ensure!(
                    owners
                        .get(id)
                        .is_some_and(|c| Arc::ptr_eq(&c.generation, generation)),
                    "stale operation generation"
                );
            }
            owners
                .values()
                .filter(|c| c.pending)
                .find(|c| expected.iter().any(|(_, g)| Arc::ptr_eq(g, &c.generation)))
                .map(|c| {
                    let mut wait = Box::pin(c.idle.clone().notified_owned());
                    wait.as_mut().enable();
                    wait
                })
        };
        if let Some(wait) = wait {
            wait.await;
            continue;
        }
        match reserve(a, &expected, operation) {
            Ok(guard) => break guard,
            Err(error) if error.is::<Busy>() => continue,
            Err(error) => return Err(error),
        }
    };
    *guard.stage.lock().expect("completion stage poisoned") = Stage::Preparation;
    complete(guard, work).await
}

#[derive(Debug)]
struct Busy;
impl std::fmt::Display for Busy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("account operation already pending")
    }
}
impl std::error::Error for Busy {}

#[derive(Debug)]
struct Fenced;
impl std::fmt::Display for Fenced {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("account ownership is fenced")
    }
}
impl std::error::Error for Fenced {}

pub(super) fn preserve_uncertain(
    a: &App,
    incident: &mut crate::supervision::Incident,
) -> Result<()> {
    let owners = a.owners.lock().map_err(|_| {
        anyhow::anyhow!("ownership registry poisoned; restart checkpoint incomplete")
    })?;
    let accounts: Vec<_> = owners
        .iter()
        .filter(|(_, claim)| {
            claim.pending
                || claim.failure.is_some()
                || claim.control.as_ref().is_some_and(|control| {
                    control.autonomous && !matches!(*control.done.borrow(), Some(Ok(())))
                })
                || claim
                    .finalization
                    .as_ref()
                    .is_some_and(|f| !matches!(*f.done.borrow(), Some(Ok(()))))
        })
        .map(|(id, _)| *id)
        .collect();
    a.world.supervisor.preserve_accounts(incident, &accounts)?;
    tracing::error!(accounts=accounts.len(), incident=%incident.id, "uncertain account fences preserved before restart");
    Ok(())
}

fn reserve(a: &App, expected: &[(i32, Arc<()>)], operation: &'static str) -> Result<Operation> {
    let mut owners = a.owners.lock().expect("ownership registry poisoned");
    a.world.supervisor.ensure_running()?;
    for (account, generation) in expected {
        let claim = owners
            .get(account)
            .context("operation has no account claim")?;
        ensure!(
            Arc::ptr_eq(&claim.generation, generation),
            "stale operation generation"
        );
        ensure!(!claim.pending, Busy);
        ensure!(claim.failure.is_none(), Fenced);
        ensure!(
            claim.finalization.is_none()
                || FINALIZING.try_with(|_| ()).is_ok()
                || claim.owner_present
                    && claim.avatar.is_none()
                    && claim
                        .finalization
                        .as_ref()
                        .is_some_and(|f| matches!(*f.done.borrow(), Some(Ok(())))),
            CloseReason::Controller
        );
    }
    for (account, _) in expected {
        owners.get_mut(account).expect("checked claim").pending = true;
    }
    Ok(Operation {
        app: a.clone(),
        accounts: expected.to_vec(),
        operation,
        stage: Arc::new(Mutex::new(Stage::Reservation)),
        completed: false,
    })
}

pub(super) enum CreditAdmission {
    Reserved(Box<Operation>),
    Waiting(std::pin::Pin<Box<dyn Future<Output = ()> + Send>>),
}
// Called only with an actor-verified sealed giver and its exact outstanding recipient source.
pub(super) fn reserve_credit_recipient(
    a: &App,
    giver: (i32, i32),
    recipient: (i32, i32),
) -> Result<CreditAdmission> {
    let mut owners = a.owners.lock().expect("ownership registry poisoned");
    a.world.supervisor.ensure_running()?;
    let source = owners.get(&giver.0).context("causal giver claim missing")?;
    ensure!(
        source.matches_avatar(giver.1) && source.failure.is_none(),
        "stale or fenced causal giver"
    );
    let target = owners
        .get(&recipient.0)
        .context("causal recipient claim missing")?;
    ensure!(
        giver.0 != recipient.0 && target.avatar == Some(recipient.1) && target.failure.is_none(),
        "stale or fenced causal recipient"
    );
    if target.pending {
        let mut wait = Box::pin(target.idle.clone().notified_owned());
        wait.as_mut().enable();
        return Ok(CreditAdmission::Waiting(wait));
    }
    let generation = target.generation.clone();
    // Closing recipients permit ONLY this proven progress prerequisite, never a new controller operation.
    owners.get_mut(&recipient.0).unwrap().pending = true;
    Ok(CreditAdmission::Reserved(Box::new(Operation {
        app: a.clone(),
        accounts: vec![(recipient.0, generation)],
        operation: "causal_progress",
        stage: Arc::new(Mutex::new(Stage::Reservation)),
        completed: false,
    })))
}

pub(super) fn collector_death_candidate(a: &App, account: i32, avatar: i32) -> Result<bool> {
    let owners = a.owners.lock().expect("ownership registry poisoned");
    let claim = owners
        .get(&account)
        .context("death candidate claim missing")?;
    ensure!(claim.matches_avatar(avatar), "stale death candidate avatar");
    Ok(!claim.pending && claim.failure.is_none() && claim.finalization.is_none())
}
pub(super) fn require_progress_owner(a: &App, account: i32, player: i32) -> Result<()> {
    let owners = a.owners.lock().expect("ownership registry poisoned");
    let claim = owners
        .get(&account)
        .context("native pause progress has no account claim")?;
    ensure!(
        claim.avatar == Some(player)
            && claim.owner_present
            && claim.pending
            && claim.failure.is_none()
            && claim.finalization.is_none(),
        "native pause progress owner is stale, unreserved, closing or fenced"
    );
    Ok(())
}

pub(super) fn reserve_player(
    a: &App,
    account: i32,
    player: i32,
    operation: &'static str,
) -> Result<Option<Operation>> {
    let expected = generations(a, &[account])?;
    match reserve(a, &expected, operation) {
        Ok(guard) => {
            bind(&a.owners, account, &guard.accounts[0].1, player)?;
            Ok(Some(guard))
        }
        Err(error) if error.is::<Busy>() || error.is::<Fenced>() || error.is::<CloseReason>() => {
            tracing::debug!(account, player, operation, error=%error, "completion preparation not admitted");
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

pub(super) async fn complete<T, F, Fut>(guard: Operation, work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(App) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T>> + Send + 'static,
{
    let app = guard.app.clone();
    let owned_accounts = guard.accounts.clone();
    let progress = guard.stage.clone();
    let operation = guard.operation;
    // This task is bounded by the claimed accounts and never aborted with its waiter.
    let completion = ACCOUNTS.scope(
        owned_accounts,
        STAGE.scope(progress, async move {
            #[cfg(test)]
            let _task = LiveTask::new();
            let mut guard = guard;
            let result = AssertUnwindSafe(async move { work(app).await })
                .catch_unwind()
                .await;
            let result = match result {
                Ok(result) => result,
                Err(panic) => {
                    stage(Stage::Panic);
                    let detail = panic
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("non-string panic payload");
                    Err(anyhow::anyhow!("owned completion task panicked: {detail}"))
                }
            };
            if let Err(error) = &result {
                let stage = *guard.stage.lock().expect("completion stage poisoned");
                if stage.uncertain() && !rejected(error, stage) {
                    fence_owned(
                        &guard.app.owners,
                        &guard.accounts,
                        operation,
                        stage,
                        format!("{error:#}"),
                    );
                }
            }
            guard.completed = true;
            drop(guard);
            result
        }),
    );
    let request = crate::performance::REQUEST.try_with(Arc::clone).ok();
    tokio::spawn(
        async move {
            match request {
                Some(request) => crate::performance::REQUEST.scope(request, completion).await,
                None => completion.await,
            }
        }
        .with_current_subscriber(),
    )
    .await
    .context("owned completion supervisor failed")?
}

pub(super) async fn player<T, F, Fut>(
    a: &App,
    player: i32,
    operation: &'static str,
    work: F,
) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(App) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T>> + Send + 'static,
{
    let account_id = a
        .world
        .for_player(player)
        .call(move |w| {
            Ok(w.players
                .get(&player)
                .context("operation player missing")?
                .character
                .account)
        })
        .await?;
    let generation = a
        .owners
        .lock()
        .expect("ownership registry poisoned")
        .get(&account_id)
        .context("operation has no account claim")?
        .generation
        .clone();
    bind(&a.owners, account_id, &generation, player)?;
    account(a, vec![account_id], operation, work).await
}

pub(super) fn retirement(
    a: &App,
    account: i32,
    player: i32,
) -> Result<impl FnOnce() -> Result<()> + Send + 'static> {
    let registry = a.owners.clone();
    let generation = registry
        .lock()
        .expect("ownership registry poisoned")
        .get(&account)
        .context("retirement has no claim")?
        .generation
        .clone();
    Ok(move || {
        let mut owners = registry.lock().expect("ownership registry poisoned");
        let claim = owners
            .get_mut(&account)
            .context("retired avatar has no claim")?;
        ensure!(
            Arc::ptr_eq(&claim.generation, &generation),
            "stale retirement generation"
        );
        ensure!(
            claim.avatar == Some(player),
            "retirement avatar differs from account lease"
        );
        ensure!(claim.failure.is_none(), "cannot retire fenced account");
        claim.avatar = None;
        Ok(())
    })
}

#[cfg(test)]
mod admission_tests;
#[cfg(test)]
mod restart_tests;
#[cfg(test)]
mod supervision_tests;

#[cfg(test)]
static LIVE_TASKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
struct LiveTask;
#[cfg(test)]
impl LiveTask {
    fn new() -> Self {
        LIVE_TASKS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self
    }
}
#[cfg(test)]
impl Drop for LiveTask {
    fn drop(&mut self) {
        LIVE_TASKS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
pub(crate) fn state(a: &App, account: i32) -> Option<(bool, Option<i32>, bool)> {
    a.owners
        .lock()
        .unwrap()
        .get(&account)
        .map(|c| (c.pending, c.avatar, c.failure.is_some()))
}

pub(crate) fn observe(a: &App, account: i32, operation: &'static str, error: &anyhow::Error) {
    let Ok(current_stage) = STAGE.try_with(|s| *s.lock().expect("completion stage poisoned"))
    else {
        return;
    };
    if !current_stage.uncertain() || rejected(error, current_stage) {
        return;
    }
    if let Ok(Some(generation)) =
        ACCOUNTS.try_with(|owned| owned.iter().find(|(id, _)| *id == account).cloned())
    {
        fence_owned(
            &a.owners,
            &[generation],
            operation,
            current_stage,
            format!("{error:#}"),
        );
    }
}

pub(crate) fn binding(
    a: &App,
    account: i32,
) -> Result<impl FnOnce(i32) -> Result<()> + Send + 'static> {
    let registry = a.owners.clone();
    let generation = registry
        .lock()
        .expect("ownership registry poisoned")
        .get(&account)
        .context("avatar has no account claim")?
        .generation
        .clone();
    Ok(move |avatar| bind(&registry, account, &generation, avatar))
}

pub(super) fn failure_reporter(
    a: &App,
    accounts: &[i32],
    operation: &'static str,
) -> Result<impl FnOnce(String) + Send + 'static> {
    let registry = a.owners.clone();
    let generations = generations(a, accounts)?;
    Ok(move |error: String| {
        fence_owned(
            &registry,
            &generations,
            operation,
            Stage::Publication,
            error,
        )
    })
}
