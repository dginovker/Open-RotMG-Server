use super::*;

use crate::store::Character;
use rand::{SeedableRng, rngs::StdRng};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::Path,
    time::{Duration, Instant},
};

#[cfg(test)]
mod guild_tests;
#[cfg(test)]
mod protection_tests;

const MAX_REQUEST_BYTES: usize = 8192;
const MAX_MUTATIONS: usize = 1024;

type NativePings = BTreeMap<i32, (i32, Option<String>, tokio::sync::oneshot::Sender<String>)>;
struct NativeLease {
    owner: String,
    expires: Instant,
    actors: Vec<i32>,
}
struct NativeControlRequest {
    trace: Arc<crate::performance::NativeControlTrace>,
    requests: Arc<Mutex<crate::performance::Requests>>,
    outcome: &'static str,
    error: Option<String>,
}
impl Drop for NativeControlRequest {
    fn drop(&mut self) {
        let message = if self.outcome == "dropped" {
            "native control request dropped"
        } else {
            "native control request completed"
        };
        tracing::info!(request=%self.trace.request,operation=%self.trace.operation,outcome=self.outcome,
            elapsed_ms=self.trace.started.elapsed().as_secs_f64()*1000.,
            world_requests=%self.requests.lock().expect("request timings poisoned").snapshot(),
            error=?self.error,"{message}");
    }
}

#[derive(Clone)]
pub struct Testing {
    root: PathBuf,
    gate: Arc<tokio::sync::Mutex<()>>,
    cancelled: Arc<Mutex<BTreeSet<String>>>,
    native_serial: Arc<std::sync::atomic::AtomicI32>,
    native_pings: Arc<Mutex<NativePings>>,
    runtime: String,
    lease: Arc<Mutex<Option<NativeLease>>>,
    pub(super) stopped: tokio::sync::watch::Sender<Option<String>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spawn {
    pub guid: String,
    pub class: u16,
    pub level: i32,
    pub seed: u64,
    pub equipment: [i32; 4],
    pub backpack: [i32; 8],
    pub world: i32,
    pub position: Pos,
    pub controller: Controller,
    pub profile: Option<String>,
    pub stats: Option<[i32; 8]>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Controller {
    Direct,
    Native,
    GameBot,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    id: String,
    command: Command,
}
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Capabilities,
    PrepareRealmCapacity {
        limit: usize,
    },
    Catalog {
        search: String,
        limit: usize,
    },
    WorldInfo {
        world: i32,
    },
    Spawn {
        spec: Spawn,
    },
    World {
        template: String,
        #[serde(default)]
        seed: Option<u64>,
    },
    Observe {
        actor: i32,
    },
    Inspect {
        actor: i32,
    },
    Act {
        actor: i32,
        action: Action,
        duration_ms: u64,
        #[serde(rename = "pauseAfter", default = "yes")]
        pause_after: bool,
    },
    Run {
        actor: i32,
        goal: Goal,
        max_game_ms: u64,
        #[serde(rename = "pauseAfter", default = "yes")]
        pause_after: bool,
    },
    Cheat {
        actor: i32,
        change: Cheat,
    },
    Pause {
        native_paused: Vec<i32>,
        #[serde(default)]
        native_sessions: Vec<(i32, i32)>,
    },
    Resume,
    NativeSync {
        actors: Vec<i32>,
    },
    NativeState {
        actors: Vec<i32>,
    },
    NativeLease {
        owner: String,
        operation: String,
        #[serde(default)]
        actors: Vec<i32>,
    },
    Result {
        request: String,
    },
    Trace {
        request: String,
        offset: usize,
        limit: usize,
    },
    Cancel {
        request: String,
    },
}
fn yes() -> bool {
    true
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Goal {
    Time { ms: u64 },
    Level { level: i32 },
    Kills { count: u64 },
    PirateCaves { count: u64 },
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Cheat {
    Stats {
        values: [i32; 8],
    },
    Item {
        slot: usize,
        item: i32,
    },
    Teleport {
        position: Pos,
    },
    StatModifier {
        index: usize,
        amount: i32,
        duration_ms: u64,
    },
    Guild {
        name: String,
        members: Vec<i32>,
    },
    Pause {
        paused: bool,
    },
    Protection {
        condition: Protection,
        duration_ms: u64,
    },
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Protection {
    Invincible,
    Stasis,
    Invulnerable,
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let temp = path.with_extension("tmp");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.sync_all()?;
    fs::rename(&temp, path)?;
    fs::File::open(path.parent().context("artifact directory missing")?)?.sync_all()?;
    Ok(())
}
fn valid_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 80
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)),
        "request ID must be 1–80 ASCII letters, numbers, '-' or '_'"
    );
    Ok(())
}
impl Testing {
    pub fn open(state: &Path) -> Result<Option<Self>> {
        if !state.join("testing.json").exists() {
            return Ok(None);
        }
        let marker: Value = serde_json::from_slice(&fs::read(state.join("testing.json"))?)?;
        ensure!(
            marker == json!({"schema":1,"provenance":"prepared"}),
            "invalid test-state marker"
        );
        let root = state.join("testing");
        fs::create_dir_all(&root)?;
        Ok(Some(Self {
            root,
            gate: Arc::default(),
            cancelled: Arc::default(),
            native_serial: Arc::new(std::sync::atomic::AtomicI32::new(-1)),
            native_pings: Arc::default(),
            runtime: format!("{:016x}", rand::random::<u64>()),
            lease: Arc::default(),
            stopped: tokio::sync::watch::channel(None).0,
        }))
    }
    pub(super) fn acknowledge_native(&self, nonce: i32, player: i32) {
        let mut pending = self
            .native_pings
            .lock()
            .expect("native acknowledgement registry poisoned");
        if pending
            .get(&nonce)
            .is_some_and(|(expected, hash, _)| *expected == player && hash.is_some())
            && let Some((_, Some(hash), sender)) = pending.remove(&nonce)
            && sender.send(hash).is_err()
        {
            tracing::warn!(
                nonce,
                player,
                "native acknowledgement arrived after its waiter closed"
            );
        }
    }
    // A configured target is not proof that it received this character's game stream.
    pub(super) fn note_native_frame(&self, nonce: i32, player: i32, frame: &[u8]) {
        let mut pending = self
            .native_pings
            .lock()
            .expect("native acknowledgement registry poisoned");
        if let Some((expected, hash, _)) = pending.get_mut(&nonce)
            && *expected == player
        {
            *hash = Some(format!("{:x}", Sha256::digest(frame)));
        }
    }
    fn path(&self, id: &str, suffix: &str) -> PathBuf {
        self.root.join(format!("{id}.{suffix}.json"))
    }
    fn registration(&self, actor: i32) -> Result<Spawn> {
        ensure!(actor > 0, "actor must be a positive durable character ID");
        Ok(serde_json::from_slice(
            &fs::read(self.path(&format!("actor-{actor}"), "spawn"))
                .context("actor was not spawned in this test session")?,
        )?)
    }
    pub fn native_entry(&self, account: i32) -> Result<Option<(i32, Pos)>> {
        let path = self.path(&format!("account-{account}"), "native");
        if !path.exists() {
            return Ok(None);
        }
        let entry: (i32, Pos) = serde_json::from_slice(&fs::read(path)?)?;
        Ok(Some(entry))
    }
    pub fn native_entered(&self, account: i32) -> Result<()> {
        let path = self.path(&format!("account-{account}"), "native");
        if path.exists() {
            fs::rename(&path, path.with_extension("consumed"))?;
        }
        Ok(())
    }
    fn record(&self, id: &str, event: Value) -> Result<()> {
        let path = self.root.join(format!("{id}.jsonl"));
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        serde_json::to_writer(&mut file, &event)?;
        file.write_all(b"\n")?;
        file.sync_data()?;
        Ok(())
    }
}
async fn resolve(a: &App, actor: i32) -> Result<i32> {
    a.world
        .directory(move |d| {
            let mut found = d
                .worlds
                .values()
                .flat_map(|w| w.players.iter())
                .filter(|(_, p)| p.character.id == actor)
                .map(|(id, _)| *id);
            let id = found
                .next()
                .context("selected character is not connected; no replacement was selected")?;
            ensure!(found.next().is_none(), "character has multiple sessions");
            Ok(id)
        })
        .await
}
async fn snapshot(a: &App, actor: i32, detail: bool) -> Result<Value> {
    let registration = a
        .testing
        .as_ref()
        .context("test session missing")?
        .registration(actor)?;
    let profile = registration.profile;
    let id = resolve(a, actor).await?;
    let content = a.content.clone();
    a.world.directory(move |d| {
        let view = d.player_view(id)?;
        let player = &d.worlds[&view.session.world].players[&id];
        let ch = &player.character;
        let controller = match player.controller { crate::world::Controller::Human => Controller::Native, crate::world::Controller::Qa => Controller::Direct, crate::world::Controller::Autonomous => Controller::GameBot };
        if detail { return Ok(json!({"view":view,"diagnostic":d.worlds[&view.session.world].observe(id)?,"privileged":true,"paused":d.clock.paused(),"pause_scope":"server","provenance":"prepared"})); }
        let item = |kind: i32| -> Result<Value> { if kind == -1 { Ok(Value::Null) } else { Ok(json!({"id":kind,"name":content.get(u16::try_from(kind)?)?.name})) } };
        let mut relevant = Vec::new();
        for object in &view.objects {
            let kind = content.get(object.kind)?;
            if kind.enemy || matches!(kind.class.as_str(), "Portal" | "Container" | "Player") { relevant.push(object); }
        }
        relevant.sort_by(|a,b|a.position.distance(view.position).total_cmp(&b.position.distance(view.position)));
        let nearby = relevant.iter().take(8).map(|o| Ok(json!({"id":o.id,"kind":o.kind,"name":if o.name.is_empty(){&content.get(o.kind)?.name}else{&o.name},"hp":o.hp,"position":o.position}))).collect::<Result<Vec<_>>>()?;
        let mut character = serde_json::to_value(&view.character)?;
        character["stats"] = json!({"max_hp":view.character.stats[0],"max_mp":view.character.stats[1],"attack":view.character.stats[2],"defense":view.character.stats[3],"speed":view.character.stats[4],"dexterity":view.character.stats[5],"vitality":view.character.stats[6],"wisdom":view.character.stats[7]});
        Ok(json!({"actor":actor,"controller":controller,"registered_profile":profile,"session":view.session,"world":view.map.name,"tick":view.tick,"game_ms":view.time_ms,"position":view.position,"character":character,"equipment":ch.items[..4].iter().map(|i|item(*i)).collect::<Result<Vec<_>>>()?,"lifecycle":view.lifecycle,"verification":{"shots":ch.shots,"kills":ch.kills,"pirate_caves":ch.pirate_caves},"nearby":nearby,"omitted_relevant_objects":relevant.len().saturating_sub(8),"paused":d.clock.paused(),"pause_scope":"server","knowledge_scope":"client_known_and_server_visible","provenance":"prepared"}))
    }).await
}
async fn native_sync(a: &App, t: &Testing, actors: Vec<i32>) -> Result<Value> {
    ensure!(
        !actors.is_empty()
            && actors.len() <= 8
            && actors.iter().collect::<BTreeSet<_>>().len() == actors.len(),
        "native synchronization requires 1–8 unique actors"
    );
    let mut acknowledgements = Vec::new();
    for actor in actors {
        let player = resolve(a, actor).await?;
        let nonce = t
            .native_serial
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        ensure!(
            nonce > i32::MIN && nonce < 0,
            "native synchronization serial budget exhausted"
        );
        let (sender, receiver) = tokio::sync::oneshot::channel();
        t.native_pings
            .lock()
            .expect("native acknowledgement registry poisoned")
            .insert(nonce, (player, None, sender));
        let result: Result<String> = async {
            a.world
                .directory(move |d| {
                    let view = d.player_view(player)?;
                    let p = &d.worlds[&view.session.world].players[&player];
                    ensure!(
                        p.controller == crate::world::Controller::Human,
                        "native synchronization requires the actual native controller"
                    );
                    let mut body = Writer::new();
                    body.i32(nonce);
                    crate::world::send(p, wire::PING, body.0);
                    Ok(())
                })
                .await?;
            tokio::time::timeout(Duration::from_secs(5), receiver)
                .await
                .context("native input acknowledgement timed out")?
                .context("native acknowledgement waiter closed")
        }
        .await;
        t.native_pings
            .lock()
            .expect("native acknowledgement registry poisoned")
            .remove(&nonce);
        let frame_sha256 = result?;
        acknowledgements
            .push(json!({"actor":actor,"player":player,"nonce":nonce,"frame_sha256":frame_sha256}));
    }
    Ok(json!({"acknowledged":acknowledgements}))
}
async fn native_state(a: &App, t: &Testing, actors: Vec<i32>) -> Result<Value> {
    ensure!(
        !actors.is_empty()
            && actors.len() <= 8
            && actors.iter().collect::<BTreeSet<_>>().len() == actors.len(),
        "native state requires 1–8 unique actors"
    );
    for actor in &actors {
        t.registration(*actor)?;
    }
    let runtime = t.runtime.clone();
    let stopped = t.stopped.borrow().clone();
    let mut state = a.world.directory(move |d| {
        let paused = d.clock.paused();
        ensure!(d.worlds.values().flat_map(|w| w.players.values()).filter(|p| p.controller == crate::world::Controller::Human).all(|p| actors.contains(&p.character.id)), "every connected native participant must be configured");
        let states = actors.into_iter().map(|actor| {
            for (world,w) in &d.worlds { for (id,p) in &w.players { if p.character.id == actor {
                let stage = if p.death_pending { "dying" } else if !p.character.alive { "dead" } else if p.native_transferring() { "transferring" } else if p.tx.as_ref().is_none_or(|o| o.packets.is_closed() || *o.lost.borrow()) { "lost" } else if p.pending || p.progress_started { "busy" } else if !p.native_ready(paused) { "loading" } else { "ready" };
                return json!({"actor":actor,"stage":stage,"player":id,"world":world,"world_name":w.name,"client_time":p.native_time()});
            } } }
            json!({"actor":actor,"stage":if d.character_transferring(actor){"transferring"}else{"disconnected"}})
        }).collect::<Vec<_>>();
        Ok(json!({"runtime":runtime,"paused":paused,"stopped":stopped,"actors":states}))
    }).await?;
    for actor in state["actors"]
        .as_array_mut()
        .context("native state actors missing")?
    {
        if actor["stage"] == "disconnected" || actor["stage"] == "transferring" {
            let id = actor["actor"]
                .as_i64()
                .context("native actor identity missing")? as i32;
            if !a
                .store
                .character_alive(id, t.registration(id)?.guid)
                .await?
            {
                actor["stage"] = "dead".into();
                actor["death_committed"] = true.into();
            }
        }
    }
    Ok(state)
}
fn halt_native(a: &App, t: &Testing, owner: &str, reason: String) -> Result<()> {
    a.world.clock.set_paused(true);
    t.stopped.send_replace(Some(reason.clone()));
    t.record(
        owner,
        json!({"event":"native_cohort_stopped","error":reason}),
    )
}
async fn native_lease(
    a: &App,
    t: &Testing,
    owner: String,
    operation: String,
    actors: Vec<i32>,
) -> Result<Value> {
    valid_id(&owner)?;
    ensure!(
        t.stopped.borrow().is_none(),
        "native control lease expired; this prepared session is terminal"
    );
    match operation.as_str() {
        "begin" => {
            if !actors.is_empty() {
                let state = native_state(a, t, actors.clone()).await?;
                ensure!(
                    state["actors"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|p| p["stage"] == "ready"),
                    "native lease requires every configured actor ready"
                );
            } else {
                ensure!(
                    !a.world
                        .directory(|d| Ok(d
                            .worlds
                            .values()
                            .flat_map(|w| w.players.values())
                            .any(|p| p.controller == crate::world::Controller::Human)))
                        .await?,
                    "native lease must name its connected cohort"
                );
            }
            {
                let mut lease = t.lease.lock().expect("native lease poisoned");
                ensure!(
                    lease.is_none(),
                    "another native operation owns the control lease"
                );
                t.record(
                    &owner,
                    json!({"event":"native_lease_started","ttl_ms":1500}),
                )?;
                *lease = Some(NativeLease {
                    owner: owner.clone(),
                    expires: Instant::now() + Duration::from_millis(1500),
                    actors,
                });
            }
            let (app, test, selected) = (a.clone(), t.clone(), owner.clone());
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    let expired = {
                        let lease = test.lease.lock().expect("native lease poisoned");
                        let Some(lease) = lease.as_ref().filter(|l| l.owner == selected) else {
                            return;
                        };
                        Instant::now() >= lease.expires
                    };
                    if expired {
                        let reason = format!(
                            "native control lease expired for {selected}; simulation stopped and native connections closed"
                        );
                        if let Err(error) = halt_native(&app, &test, &selected, reason) {
                            tracing::error!(%error,"lease expiry receipt failed after safe stop");
                        }
                        return;
                    }
                }
            });
        }
        "renew" => {
            let actors = {
                let lease = t.lease.lock().expect("native lease poisoned");
                let lease = lease.as_ref().context("native lease is not active")?;
                ensure!(lease.owner == owner, "native lease owner differs");
                lease.actors.clone()
            };
            if !actors.is_empty() {
                let state = native_state(a, t, actors).await?;
                if let Some(terminal) = state["actors"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|p| matches!(p["stage"].as_str(), Some("dead" | "dying" | "lost")))
                {
                    let reason =
                        format!("native participant is terminal during control: {terminal}");
                    halt_native(a, t, &owner, reason.clone())?;
                    bail!("{reason}");
                }
            }
            let mut lease = t.lease.lock().expect("native lease poisoned");
            let lease = lease.as_mut().context("native lease is not active")?;
            ensure!(
                lease.owner == owner && Instant::now() < lease.expires,
                "native lease owner changed or expired"
            );
            lease.expires = Instant::now() + Duration::from_millis(1500);
        }
        "halt" => {
            ensure!(
                t.lease
                    .lock()
                    .expect("native lease poisoned")
                    .as_ref()
                    .is_some_and(|l| l.owner == owner),
                "native lease owner differs"
            );
            halt_native(a,t,&owner,"native controller could not confirm its cohort; simulation stopped and connections closed".into())?;
        }
        "end" => {
            ensure!(
                a.world.clock.paused(),
                "cannot release native control while simulation is running"
            );
            let mut lease = t.lease.lock().expect("native lease poisoned");
            ensure!(
                lease.as_ref().is_some_and(|l| l.owner == owner),
                "native lease owner differs"
            );
            *lease = None;
            t.record(&owner, json!({"event":"native_lease_released"}))?;
        }
        _ => bail!("native lease operation must be begin, renew, halt or end"),
    }
    Ok(json!({"owner":owner,"operation":operation,"ttl_ms":1500}))
}
async fn pause(a: &App, native: &[i32], sessions: &[(i32, i32)]) -> Result<()> {
    let native = native.to_vec();
    ensure!(
        sessions.len() <= 8
            && sessions
                .iter()
                .map(|(actor, _)| actor)
                .collect::<BTreeSet<_>>()
                .len()
                == sessions.len(),
        "native pause requires at most eight unique actor/session pairs"
    );
    let sessions: BTreeMap<_, _> = sessions.iter().copied().collect();
    let app = a.clone();
    a.world.directory(move |d| {
        for (id,p) in d.worlds.values().flat_map(|w|w.players.iter()).filter(|(_,p)|p.controller == crate::world::Controller::Human) {
            ensure!(native.contains(&p.character.id), "native character {} needs coordinated Ruffle pause through the managed native client",p.character.id);
            ensure!(sessions.get(&p.character.id)==Some(id), "native pause session changed; complete the actual reconnect before suspending");
            if let Some(snapshot) = p.native_pause_progress()? {
                lifecycle::require_progress_owner(&app, snapshot.account, *id)?;
            }
        }
        d.clock.set_paused(true);
        Ok(())
    }).await?;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if a.world
                .directory(|d| {
                    for w in d.worlds.values() {
                        if !w.settled()? {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                })
                .await?
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .context("simulation stopped but persistence did not settle; inspect trace before resuming")?
}
async fn finish_action(
    a: &App,
    actor: i32,
    result: Result<Value>,
    pause_after: bool,
    nested: bool,
) -> Result<Value> {
    if !pause_after {
        return result;
    }
    match (result, pause(a, &[], &[]).await) {
        (Err(primary), Err(cleanup)) => {
            bail!("{primary:#}; pause cleanup also failed: {cleanup:#}")
        }
        (Err(primary), _) => Err(primary),
        (_, Err(cleanup)) => Err(cleanup),
        (Ok(mut value), Ok(())) => {
            let state = snapshot(a, actor, false).await?;
            if nested {
                value["state"] = state;
            } else {
                value = state;
            }
            Ok(value)
        }
    }
}
async fn ensure_server_pause(a: &App) -> Result<()> {
    a.world
        .directory(|d| {
            ensure!(
                !d.worlds
                    .values()
                    .flat_map(|w| w.players.values())
                    .any(|p| p.controller == crate::world::Controller::Human),
                "native participants require the managed native wrapper for pauseAfter"
            );
            Ok(())
        })
        .await
}
pub(crate) async fn spawn(a: &App, t: &Testing, spec: Spawn) -> Result<Value> {
    ensure!(
        a.world.clock.paused(),
        "spawn requires a paused prepared test session"
    );
    ensure!((1..=20).contains(&spec.level), "level must be 1–20");
    ensure!(
        !spec.guid.is_empty() && !spec.guid.contains('@') && spec.guid.len() <= 128,
        "use an explicit disposable guest GUID, not a registered account"
    );
    a.content.get(spec.class)?;
    ensure!(
        a.content.classes.contains_key(&spec.class),
        "unknown playable class"
    );
    let mut preview = Character {
        schema: 1,
        lifetime: crate::store::FameLifetime::complete(),
        class: spec.class,
        base: a.content.classes[&spec.class].base,
        tex1: 0,
        tex2: 0,
        pet_kind: None,
        id: 1,
        account: 1,
        name: String::new(),
        level: 1,
        exp: 0,
        xp_boost: Default::default(),
        fame: 0,
        hp: 1,
        mp: 1,
        items: spec.equipment.into_iter().chain(spec.backpack).collect(),
        revision: 0,
        alive: true,
        created_at: 0,
        shots: 0,
        hits: 0,
        kills: 0,
        quests: 0,
        tiles: 0,
        pirate_caves: 0,
        dungeons: [0; 3],
        jungles: 0,
        abysses: 0,
        trenches: 0,
        spider_dens: 0,
        mad_labs: 0,
        manors: 0,
        beaches: 0,
        davy_lockers: 0,
        tombs: 0,
    };
    preview.gain_experience(
        &a.content,
        crate::store::level_start(spec.level)?,
        &mut StdRng::seed_from_u64(spec.seed),
    )?;
    if let Some(stats) = spec.stats {
        preview.base = stats;
    }
    preview.effective(&a.content)?;
    for kind in preview.items.iter().filter(|k| **k != -1) {
        a.content.get(u16::try_from(*kind)?)?;
    }
    let (world, pos) = (spec.world, spec.position);
    a.world
        .directory(move |d| {
            let w = d
                .worlds
                .get(&world)
                .context("world missing; explicitly create the required world first")?;
            ensure!(
                pos.x.is_finite() && pos.y.is_finite() && w.walkable(pos),
                "spawn position is blocked or outside the world"
            );
            Ok(())
        })
        .await?;
    let name = if spec.controller == Controller::GameBot {
        let profile = spec
            .profile
            .as_deref()
            .context("game_bot requires an explicit actual gameplay profile")?;
        crate::bots::validate_test_class(spec.class, profile)?;
        Some(profile.to_owned())
    } else {
        ensure!(
            spec.profile.is_none(),
            "profile belongs only to the game_bot controller"
        );
        None
    };
    a.begin_admission();
    let account = a.store.list(spec.guid.clone()).await?;
    ensure!(
        account.characters.is_empty(),
        "GUID already has a living character; no implicit replacement or reset"
    );
    let mut owner = a.claim(account.id)?;
    let t = t.clone();
    super::lifecycle::account(a, vec![account.id], "qa_spawn", move |a| async move {
    owner.arm_cleanup(&a, tokio::runtime::Handle::current());
    super::lifecycle::stage(Stage::Store);
    let mut ch = a.store.create_prepared(account.id, a.content.get(spec.class)?, name).await?;
    ch.base = preview.base;
    ch.level = preview.level;
    ch.exp = preview.exp;
    ch.fame = preview.fame;
    ch.items = preview.items;
    let stats = ch.effective(&a.content)?;
    ch.hp = stats[0];
    ch.mp = stats[1];
    ch.revision = a
        .store
        .exchange(
            vec![InventoryWrite {
                id: ch.id,
                account: ch.account,
                revision: ch.revision,
                items: ch.items.clone(),
            }],
            Vec::new(),
        )
        .await?[0];
    a.store.progress(ch.clone()).await?;
    let actor = ch.id;
    write_json(&t.path(&format!("actor-{actor}"), "spawn"), &spec)?;
    match spec.controller {
        Controller::Native => {
            write_json(&t.path(&format!("account-{}", ch.account), "native"), &(world, pos))?;
            owner.release();
        },
        Controller::Direct => {
            let presentation = a.store.presentation(ch.account).await?;
            let bind = super::lifecycle::binding(&a, ch.account)?;
            super::lifecycle::stage(Stage::Publication);
            let id = a
                .world
                .in_world(world)
                .call(move |w| {
                    let id = w.enter_at(ch, None, Some(pos), presentation)?;
                    bind(id)?;
                    Ok(id)
                })
                .await?;
            let mut bots = a.bots.lock().expect("ownership registry poisoned");
            ensure!(!bots.contains_key(&id), "QA avatar already has a controller owner");
            owner.publish_script(id)?;
            bots.insert(id, owner);
        }
        Controller::GameBot => {
            crate::bots::start_test(
                &a,
                ch,
                world,
                pos,
                spec.profile
                    .as_deref()
                    .context("gameplay profile missing")?,
                owner,
            )
            .await?
        }
    }
    Ok(
        json!({"actor":actor,"account":account.id,"world":world,"controller":spec.controller,"provenance":"prepared","connected":spec.controller!=Controller::Native}),
    )
    }).await
}
async fn resume(a: &App, t: &Testing) -> Result<()> {
    ensure!(
        t.stopped.borrow().is_none(),
        "native control lease expired; this prepared session is terminal"
    );
    let leased = t
        .lease
        .lock()
        .expect("native lease poisoned")
        .as_ref()
        .is_some_and(|l| Instant::now() < l.expires);
    a.world
        .directory(move |d| {
            ensure!(
                leased
                    || !d
                        .worlds
                        .values()
                        .flat_map(|w| w.players.values())
                        .any(|p| p.controller == crate::world::Controller::Human),
                "native participants require an active control lease"
            );
            d.clock.set_paused(false);
            Ok(())
        })
        .await
}
async fn prepare_realm_capacity(a: &App, t: &Testing, limit: usize) -> Result<Value> {
    ensure!(
        a.world.clock.paused(),
        "capacity setup requires a paused prepared session"
    );
    ensure!(
        (1..a.max_players).contains(&limit),
        "capacity limit must be at least one and below global capacity"
    );
    let path = t.root.join("prepared-realm-capacity.json");
    ensure!(!path.exists(), "active Realm capacity was already prepared");
    ensure!(
        !fs::read_dir(&t.root)?
            .collect::<std::io::Result<Vec<_>>>()?
            .iter()
            .any(|e| e.file_name().as_encoded_bytes().ends_with(b".spawn.json")),
        "capacity setup must precede enrollment"
    );
    let owners = a.owners.clone();
    let admission_started = a.admission_started.clone();
    let global = a.max_players;
    let receipt = a
        .world
        .directory(move |d| {
            let claims = owners.lock().expect("ownership registry poisoned");
            ensure!(
                claims.is_empty(),
                "capacity setup requires no ownership claims"
            );
            ensure!(
                !admission_started.load(Ordering::Relaxed),
                "capacity setup must precede admission"
            );
            let world = d.prepare_realm_capacity(limit)?;
            let receipt =
                json!({"world":world,"limit":limit,"global_limit":global,"provenance":"prepared"});
            write_json(&path, &receipt)?;
            Ok(receipt)
        })
        .await?;
    Ok(receipt)
}
async fn prepare_guild(
    a: &App,
    t: &Testing,
    actor: i32,
    name: String,
    members: Vec<i32>,
) -> Result<Value> {
    ensure!(
        a.world.clock.paused(),
        "guild preparation requires an explicitly paused test session"
    );
    let mut actors = vec![actor];
    actors.extend(members);
    ensure!(
        (1..=50).contains(&actors.len())
            && actors.iter().collect::<BTreeSet<_>>().len() == actors.len(),
        "prepared guild requires 1–50 distinct registered actors"
    );
    let mut players = Vec::new();
    let mut accounts = Vec::new();
    for actor in &actors {
        t.registration(*actor)?;
        let player = resolve(a, *actor).await?;
        let account = a
            .world
            .for_player(player)
            .call(move |w| {
                Ok(w.players
                    .get(&player)
                    .context("guild player missing")?
                    .character
                    .account)
            })
            .await?;
        players.push(player);
        accounts.push(account);
    }
    let guild = super::lifecycle::account(a, accounts.clone(), "qa_guild", move |a| async move {
        super::lifecycle::stage(Stage::Reservation);
        for player in &players {
            let player = *player;
            a.world
                .for_player(player)
                .call(move |w| {
                    let p = w.players.get(&player).context("guild player missing")?;
                    ensure!(
                        p.character.alive
                            && !p.pending
                            && !p.progress_started
                            && !p.death_pending
                            && !p.departing,
                        super::lifecycle::controller(
                            "guild actor has pending gameplay or persistence"
                        )
                    );
                    Ok(())
                })
                .await?;
        }
        super::lifecycle::stage(Stage::Store);
        let presentations = a.store.create_prepared_guild(accounts, name).await?;
        let guild = presentations
            .first()
            .context("prepared guild has no founder")?
            .1
            .guild
            .clone()
            .context("prepared membership missing after commit")?;
        super::lifecycle::stage(Stage::Publication);
        a.world
            .directory(move |d| {
                for (account, presentation) in presentations {
                    d.sync_guild(account, presentation, false)?;
                }
                Ok(())
            })
            .await?;
        Ok(guild)
    })
    .await?;
    let mut state = snapshot(a, actor, false).await?;
    state["prepared_guild"] = json!({"unearned":true,"guild":guild,"actors":actors,"client_publication":"next_ordinary_unpaused_tick"});
    Ok(state)
}

async fn prepare_pause(a: &App, t: &Testing, actor: i32, paused: bool) -> Result<Value> {
    t.registration(actor)?;
    ensure!(
        a.world.clock.paused(),
        "actor pause preparation requires an explicitly paused test session"
    );
    let player = resolve(a, actor).await?;
    super::lifecycle::player(a, player, "qa_pause", move |a| async move {
        super::lifecycle::stage(Stage::Reservation);
        a.world
            .directory(move |d| d.prepare_actor_pause(player, paused))
            .await
    })
    .await?;
    let mut state = snapshot(a, actor, false).await?;
    state["prepared_pause"] =
        json!({"paused":paused,"client_publication":"next_ordinary_unpaused_tick"});
    Ok(state)
}

async fn execute(a: &App, t: &Testing, id: &str, command: Command) -> Result<Value> {
    ensure!(
        t.stopped.borrow().is_none()
            || matches!(
                command,
                Command::Capabilities
                    | Command::Observe { .. }
                    | Command::Inspect { .. }
                    | Command::NativeState { .. }
                    | Command::Result { .. }
                    | Command::Trace { .. }
                    | Command::Cancel { .. }
            ),
        "native control lease expired; this prepared session is terminal"
    );
    match command {
        Command::Capabilities => Ok(json!({"schema":1,"provenance":"prepared","pause_scope":"isolated server and all registered native clients","controllers":["direct","native","game_bot"],"game_bot_profiles":crate::bots::profiles(),"goals":["time","level","kills","pirate_caves"],"max_game_ms":600000,"trace_page_limit":40,"request_bytes_limit":MAX_REQUEST_BYTES,"mutation_limit":MAX_MUTATIONS,"starting_context_operations":["prepare_realm_capacity"],"prepared_realm_capacity":if t.root.join("prepared-realm-capacity.json").exists(){serde_json::from_slice::<Value>(&fs::read(t.root.join("prepared-realm-capacity.json"))?)?}else{Value::Null}})),
        Command::PrepareRealmCapacity{limit} => prepare_realm_capacity(a,t,limit).await,
        Command::Catalog{search,limit} => {ensure!(!search.is_empty() && search.len()<=80 && (1..=20).contains(&limit),"catalog needs search and limit 1–20");let matches=a.content.objects.values().filter(|o|o.name.to_lowercase().contains(&search.to_lowercase())).collect::<Vec<_>>();Ok(json!({"items":matches.iter().take(limit).map(|o|json!({"id":o.kind,"name":o.name,"class":o.class,"slot_type":o.slot_type,"unsupported_equipment":o.unsupported_equipment})).collect::<Vec<_>>(),"omitted":matches.len().saturating_sub(limit)}))},
        Command::WorldInfo{world} => a.world.in_world(world).call(|w|Ok(json!({"name":w.name,"width":w.map.width,"height":w.map.height,"realm_layout":w.realm_layout,"spawn_points":w.map.spawns,"prepared_seed":w.prepared_seed,"privileged":true,"encounter_markers":w.entities.values().filter(|e|crate::endgame::GUARDIANS.contains(&e.kind)||crate::dungeon::manor::ROSTER.contains(&e.kind)||e.kind==crate::dungeon::manor::BOOKCASE||matches!(e.kind,crate::endgame::ORYX|crate::endgame::ORYX2|0xd06|0xd7a|0xd7b|crate::dungeon::trench::VENT|crate::dungeon::trench::THESSAL|0x721|0x700|0xe32|0xe3e..=0xe41)).map(|e|json!({"id":e.id,"kind":e.kind,"hp":e.hp,"defense":w.content.objects[&e.kind].defense,"position":e.pos,"region":w.map.cells[(e.pos.y as i32*w.map.width+e.pos.x as i32) as usize].region})).collect::<Vec<_>>()}))).await,
        Command::Spawn{spec} => spawn(a,t,spec).await,
        Command::World{template,seed} => { let world=a.world.directory(move |d|match seed { Some(seed)=>d.prepared_instance_with_seed(&template,seed), None=>d.prepared_instance(&template) }).await?; Ok(json!({"world":world,"provenance":"prepared"})) },
        Command::Observe{actor} => {t.registration(actor)?;snapshot(a,actor,false).await},
        Command::Inspect{actor} => {t.registration(actor)?;snapshot(a,actor,true).await},
        Command::Resume => {resume(a,t).await?;Ok(json!({"paused":false}))},
        Command::NativeSync{actors} => native_sync(a,t,actors).await,
        Command::NativeState{actors} => native_state(a,t,actors).await,
        Command::NativeLease{owner,operation,actors} => {
            let trace = Arc::new(crate::performance::NativeControlTrace {request:id.into(),operation:operation.clone(),started:Instant::now()});
            let requests = Arc::new(Mutex::new(crate::performance::Requests {native_control:Some(trace.clone()),..Default::default()}));
            let mut lifecycle = NativeControlRequest {trace,requests:requests.clone(),outcome:"dropped",error:None};
            tracing::info!(request=id,%operation,"native control request received");
            let result = crate::performance::REQUEST.scope(requests,native_lease(a,t,owner,operation,actors)).await;
            lifecycle.outcome = if result.is_ok() {"ok"} else {"error"};
            lifecycle.error = result.as_ref().err().map(|error|format!("{error:#}"));
            result
        },
        Command::Pause{native_paused,native_sessions} => {pause(a,&native_paused,&native_sessions).await?;Ok(json!({"paused":true,"native_paused":native_paused}))},
        Command::Act{actor,action,duration_ms,pause_after} => {
            let spec=t.registration(actor)?;
            ensure!(spec.controller==Controller::Direct,"direct actions require a direct actor; use native input or the existing game-bot controller, not simultaneous controllers");
            ensure!(duration_ms<=10000,"single action duration exceeds 10 seconds; use a bounded run");
            if pause_after {ensure_server_pause(a).await?;}
            let before=snapshot(a,actor,false).await?;t.record(id,json!({"event":"before","state":before}))?;
            resume(a,t).await?;
            let result: Result<Value> = async {
                let player=resolve(a,actor).await?;
                super::inventory(a,player,move |w|w.action(player,action)).await??;
                tokio::time::sleep(Duration::from_millis(duration_ms)).await;
                a.world.for_player(player).call(move |w|w.action(player,Action::Stop)).await?;
                snapshot(a,actor,false).await
            }.await;
            finish_action(a,actor,result,pause_after,false).await
        },
        Command::Run{actor,goal,max_game_ms,pause_after} => {
            let spec=t.registration(actor)?;
            ensure!(spec.controller==Controller::GameBot,"run goals require the actual gameplay-bot controller");
            ensure!((50..=600000).contains(&max_game_ms),"max_game_ms must be 50–600000");
            match goal {Goal::Time{ms}=>ensure!(ms>0&&ms<=max_game_ms,"duration must fit deadline"),Goal::Level{level}=>ensure!((1..=20).contains(&level),"invalid target level"),Goal::Kills{count}|Goal::PirateCaves{count}=>ensure!(count>0,"count must be positive")}
            if pause_after {ensure_server_pause(a).await?;}
            let before=snapshot(a,actor,false).await?;t.record(id,json!({"event":"before","state":before}))?;
            let start_tick=a.world.directory(|d|Ok(d.tick)).await?;
            resume(a,t).await?;
            let wall=Instant::now();
            let result: Result<Value> = async {loop {
                ensure!(!t.cancelled.lock().expect("cancellation registry poisoned").contains(id),"run cancelled");
                ensure!(wall.elapsed()<Duration::from_millis(max_game_ms+30000),"wall deadline exceeded");
                let current=snapshot(a,actor,false).await?;
                ensure!(current["lifecycle"]["alive"]==true && current["lifecycle"]["death_pending"]==false,"selected actor died; no replacement was selected");
                ensure!(current["lifecycle"]["quarantined"]==false,"actor quarantined");
                let elapsed=(a.world.directory(|d|Ok(d.tick)).await?-start_tick)*50;
                let reached=match goal {
                    Goal::Time{ms}=>elapsed>=ms,
                    Goal::Level{level}=>current["character"]["level"].as_i64().context("level missing")?>=i64::from(level),
                    Goal::Kills{count}=>current["verification"]["kills"].as_u64().context("kills missing")?>=before["verification"]["kills"].as_u64().context("initial kills missing")?+count,
                    Goal::PirateCaves{count}=>current["verification"]["pirate_caves"].as_u64().context("cave count missing")?>=before["verification"]["pirate_caves"].as_u64().context("initial cave count missing")?+count,
                };
                if reached {return Ok(json!({"goal_reached":true,"game_ms":elapsed,"state":current}));}
                ensure!(elapsed<max_game_ms,"goal not reached before game deadline");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }}.await;
            finish_action(a,actor,result,pause_after,true).await
        },
        Command::Cheat{actor,change:Cheat::Guild{name,members}} => prepare_guild(a,t,actor,name,members).await,
        Command::Cheat{actor,change:Cheat::Pause{paused}} => prepare_pause(a,t,actor,paused).await,
        Command::Cheat{actor,change} => {
            t.registration(actor)?;
            ensure!(a.world.clock.paused(),"cheats require an explicitly paused test session");
            let player=resolve(a,actor).await?;
            super::lifecycle::player(a,player,"qa_cheat",move |a| async move {
            super::lifecycle::stage(Stage::Reservation);
            let mut ch=a.world.for_player(player).call(move |w| {let p=w.players.get(&player).context("player missing")?;ensure!(p.character.alive&&!p.pending&&!p.progress_started&&!p.death_pending&&!p.departing,"actor has pending gameplay or persistence");Ok(p.character.clone())}).await?;
            match change {
                Cheat::StatModifier{index,amount,duration_ms}=>{a.world.for_player(player).call(move |w| w.prepare_stat_modifier(player,index,amount,duration_ms)).await?;},
                Cheat::Protection{condition,duration_ms}=>{a.world.for_player(player).call(move |w| {
                    ensure!((1..=60000).contains(&duration_ms),super::lifecycle::controller("prepared protection duration must be 1–60000ms"));
                    let p=w.players.get(&player).context("player missing")?;
                    ensure!(!crate::condition::protected(p.conditions(w.now)),super::lifecycle::controller("player protection already active"));
                    let index=match condition {Protection::Invincible=>crate::condition::INVINCIBLE,Protection::Stasis=>crate::condition::STASIS,Protection::Invulnerable=>crate::condition::INVULNERABLE};
                    w.apply_condition(player,index,duration_ms)?;w.updates(player,true)
                }).await?;},
                Cheat::Teleport{position}=>{a.world.for_player(player).call(move |w| {ensure!(position.x.is_finite()&&position.y.is_finite()&&w.walkable(position),"teleport position invalid");let p=w.players.get_mut(&player).context("player missing")?;ensure!(p.controller!=crate::world::Controller::Human,"native teleport requires a verified client correction, not a server-only edit");p.pos=position;p.bot_goal=None;w.updates(player,true)}).await?;},
                change=>{
                    if let Cheat::Stats{values}=&change {let values=*values;a.world.for_player(player).call(move |w| w.validate_prepared_base_stats(player,values)).await?;}
                    super::lifecycle::stage(Stage::Store);
                    match change {
                        Cheat::Stats{values}=>{ch.base=values;let stats=ch.effective(&a.content)?;ch.hp=ch.hp.min(stats[0]);ch.mp=ch.mp.min(stats[1]);},
                        Cheat::Item{slot,item}=>{ensure!(slot<12,"invalid item slot");if item!=-1{a.content.get(u16::try_from(item)?)?;}ch.items[slot]=item;ch.effective(&a.content)?;ch.revision=a.store.exchange(vec![InventoryWrite{id:ch.id,account:ch.account,revision:ch.revision,items:ch.items.clone()}],Vec::new()).await?[0];},
                        Cheat::Teleport{..}|Cheat::StatModifier{..}|Cheat::Guild{..}|Cheat::Pause{..}|Cheat::Protection{..}=>unreachable!(),
                    }
                    let stats = ch.effective(&a.content)?;
                    ch.hp = ch.hp.min(stats[0]); ch.mp = ch.mp.min(stats[1]);
                    a.store.progress(ch.clone()).await?;
                    a.world.for_player(player).call(move |w|{w.players.get_mut(&player).context("player missing")?.character=ch;w.updates(player,true)}).await?;
                }
            }
            Ok(())
            }).await?;
            snapshot(a,actor,false).await
        },
        Command::Result{request}=>{valid_id(&request)?;let saved:Value=serde_json::from_slice(&fs::read(t.path(&request,"receipt"))?)?;if saved["status"]=="running" {Ok(json!({"status":"incomplete","error":"request has no confirmed result; inspect trace, do not replay effects"}))}else{Ok(saved.get("receipt").context("stored receipt missing")?.clone())}},
        Command::Trace{request,offset,limit}=>{valid_id(&request)?;ensure!((1..=40).contains(&limit),"trace limit must be 1–40");let raw=fs::read_to_string(t.root.join(format!("{request}.jsonl")))?;let lines=raw.lines().collect::<Vec<_>>();Ok(json!({"events":lines.iter().skip(offset).take(limit).map(|s|serde_json::from_str::<Value>(s)).collect::<std::result::Result<Vec<_>,_>>()?,"total":lines.len(),"next_offset":(offset+limit).min(lines.len())}))},
        Command::Cancel{request}=>{valid_id(&request)?;ensure!(t.path(&request,"receipt").is_file(),"cancel requires an accepted request; no cancellation was registered");t.cancelled.lock().expect("cancellation registry poisoned").insert(request);Ok(json!({"cancel_requested":true}))},
    }
}
pub async fn command(State(a): State<App>, headers: HeaderMap, Json(raw): Json<Value>) -> Response {
    let result:Result<Value>=async {
        control(&a,&headers)?;
        ensure!(serde_json::to_vec(&raw)?.len()<=MAX_REQUEST_BYTES,"request exceeds 8192-byte limit; rejected before acceptance");
        let t=a.testing.as_ref().context("testing interface requires an exclusively created --test-state directory")?;
        let request:Request=serde_json::from_value(raw.clone())?;valid_id(&request.id)?;
        if matches!(request.command,Command::Observe{..}|Command::Inspect{..}|Command::Catalog{..}|Command::WorldInfo{..}|Command::Result{..}|Command::Trace{..}|Command::Capabilities|Command::Cancel{..}|Command::NativeState{..}|Command::NativeSync{..}|Command::NativeLease{..}) {return execute(&a,t,&request.id,request.command).await;}
        let entry=super::lifecycle::drain::entry(&a)?;
        let guard=t.gate.clone().lock_owned().await;
        let a = a.clone();
        tokio::spawn(entry.scope(async move {
        let _guard = guard;
        let t=a.testing.as_ref().context("accepted QA request lost testing state")?;
        let path=t.path(&request.id,"receipt");
        if path.exists() {
            let previous:Value=serde_json::from_slice(&fs::read(path)?)?;
            ensure!(previous["request"]==raw,"request ID reused with different input");
            ensure!(previous["status"] != "running", "previous request has no confirmed result; inspect its trace rather than repeating its effects");
            return Ok(previous.get("receipt").context("stored receipt is malformed")?.clone());
        }
        let mutations=fs::read_dir(&t.root)?.collect::<std::io::Result<Vec<_>>>()?.iter().filter(|entry|entry.file_name().as_encoded_bytes().ends_with(b".receipt.json")).count();
        if mutations>=MAX_MUTATIONS {
            a.world.clock.set_paused(true);
            bail!("prepared mutation budget reached; server clock stopped, native clients require wrapper cleanup; preserve this state and use a new owned test state");
        }
        write_json(&path,&json!({"request":raw,"status":"running"}))?;
        t.record(&request.id,json!({"event":"accepted","request":raw}))?;
        let wall=Instant::now();let start=a.world.directory(|d|Ok(d.tick)).await?;
        let result=execute(&a,t,&request.id,request.command).await;
        let mut receipt=json!({"id":request.id,"status":if result.is_ok(){"succeeded"}else{"failed"},"wall_ms":wall.elapsed().as_millis(),"game_ms":(a.world.directory(|d|Ok(d.tick)).await?-start)*50,"paused":a.world.clock.paused(),"pause_scope":"server","provenance":"prepared","trace":t.root.join(format!("{}.jsonl",request.id))});
        match result {Ok(v)=>receipt["result"]=v,Err(e)=>receipt["error"]=format!("{e:#}").into()}
        t.record(&request.id,json!({"event":"result","receipt":receipt}))?;
        let durable=json!({"request":raw,"receipt":receipt});
        write_json(&path,&durable)?;
        Ok(receipt)
        })).await.context("accepted QA completion task panicked")?
    }.await;
    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"status":"failed","error":format!("{e:#}")})),
        )
            .into_response(),
    }
}
