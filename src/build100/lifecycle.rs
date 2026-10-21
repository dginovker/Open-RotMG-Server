use crate::{
    content::{Content, Grounds},
    protocol,
    realm_map::{self, Kind},
    wire::{Packet, Writer},
    world::{Character, Room},
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};

pub type Database = Arc<Mutex<Connection>>;
pub type Output = mpsc::Sender<Vec<Packet>>;
pub const OUTPUT_CAPACITY: usize = 8;
const MAX_SESSIONS: usize = 16;
const MAX_LEASES: usize = 32;
#[derive(Debug)]
struct StorageFailure;
impl std::fmt::Display for StorageFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("fatal build100 persistence failure")
    }
}
impl std::error::Error for StorageFailure {}
const LEASE_TIME: Duration = Duration::from_secs(5);
const PING_INTERVAL: Duration = Duration::from_secs(5);
const PING_TIMEOUT: Duration = Duration::from_secs(10);
const INPUT_TIMEOUT: Duration = Duration::from_secs(60);

pub enum Command {
    Join {
        hello: protocol::Hello,
        output: Output,
        reply: oneshot::Sender<Result<u64, String>>,
    },
    Input {
        session: u64,
        packet: Packet,
        received: Instant,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Detach {
        session: u64,
    },
    #[cfg(test)]
    Tick(Duration, oneshot::Sender<Result<(), String>>),
    #[cfg(test)]
    Inspect(Box<dyn FnOnce(&mut Simulation) + Send>),
}
struct Session {
    guid: String,
    world: i32,
    avatar: Option<i32>,
    generation: Option<i32>,
    output: Output,
    last_input: Duration,
    next_ping: Duration,
    ping: Option<(i32, Duration)>,
    transfer_generation: Option<i32>,
    tutorial_exit: bool,
}
struct PendingMove {
    packet: Packet,
    received: Instant,
    due: Instant,
    boundary: Instant,
}
// MOVE shares the established one-reply slot; it must never bypass weapon backpressure.
struct DeferredWeapon {
    movement: Option<PendingMove>,
    world: i32,
    avatar: i32,
    generation: i32,
    reply: oneshot::Sender<Result<(), String>>,
}
struct DestinationLease {
    origin: u64,
    world: i32,
    portal: i32,
    destination: i32,
    generation: i32,
    expires: Duration,
}
pub struct Simulation {
    entry: Option<(Arc<Grounds>, u64)>,
    next_world: i32,
    content: Content,
    db: Database,
    worlds: BTreeMap<i32, Room>,
    sessions: BTreeMap<u64, Session>,
    owners: HashMap<String, u64>,
    leases: HashMap<String, DestinationLease>,
    next_session: u64,
    next_avatar: i32,
    serial: i32,
    now: Duration,
    deferred_weapons: BTreeMap<u64, DeferredWeapon>,
}
pub fn character(db: &Database, guid: &str) -> Result<Option<Character>> {
    let json: Option<String> = db
        .lock()
        .expect("database mutex poisoned")
        .query_row(
            "SELECT state FROM build100_characters WHERE guid=?",
            [guid],
            |r| r.get(0),
        )
        .optional()?;
    json.map(|s| Ok(serde_json::from_str(&s)?)).transpose()
}
impl Simulation {
    pub fn new(content: Content, db: Database) -> Self {
        Self {
            entry: None,
            next_world: 0,
            content,
            db,
            worlds: BTreeMap::from([
                (1, Room::new(100, 1_000_000, 2)),
                (2, Room::new(200, 2_000_000, 1)),
            ]),
            sessions: BTreeMap::new(),
            owners: HashMap::new(),
            leases: HashMap::new(),
            next_session: 0,
            next_avatar: 0,
            serial: 0,
            now: Duration::ZERO,
            deferred_weapons: BTreeMap::new(),
        }
    }
    pub fn normal(
        content: Content,
        grounds: Arc<Grounds>,
        db: Database,
        seed: u64,
    ) -> Result<Self> {
        let map = Arc::new(realm_map::realm(seed, &content, grounds.clone())?);
        let room = Room::from_map(map, &content, 1_000_000)?;
        let mut simulation = Self::new(content, db);
        simulation.worlds = BTreeMap::from([(0, room)]);
        simulation.entry = Some((grounds, seed));
        tracing::info!(
            world = 0,
            seed,
            "build100 reconstructed shared realm generated"
        );
        Ok(simulation)
    }
    fn retire_private_maps(&mut self) {
        if self.entry.is_none() {
            return;
        }
        self.worlds.retain(|&world, room| {
            world == 0
                || !room.players.is_empty()
                || self.sessions.values().any(|s| s.world == world)
                || self
                    .leases
                    .values()
                    .any(|l| l.world == world && self.now < l.expires)
        });
    }
    fn emit(&self, session: u64, packets: Vec<Packet>) -> bool {
        if packets.is_empty() {
            return true;
        }
        let Some(s) = self.sessions.get(&session) else {
            return false;
        };
        if let Err(error) = s.output.try_send(packets) {
            tracing::warn!(session, world=s.world, generation=s.generation, error=%error, "build100 bounded output rejected; detaching consumer");
            return false;
        }
        true
    }
    fn broadcast(&mut self, world: i32, except: Option<u64>, packets: Vec<Packet>) -> Result<()> {
        self.broadcast_input(world, except, packets, None)
    }
    fn broadcast_input(
        &mut self,
        world: i32,
        except: Option<u64>,
        packets: Vec<Packet>,
        teleport_requester: Option<u64>,
    ) -> Result<()> {
        let mut pending = VecDeque::from([(world, except, packets, teleport_requester)]);
        while let Some((world, except, packets, teleport_requester)) = pending.pop_front() {
            let recipients: Vec<_> = self
                .sessions
                .iter()
                .filter(|(id, s)| Some(**id) != except && s.world == world && s.avatar.is_some())
                .map(|(&id, _)| id)
                .collect();
            let mut batches = vec![];
            for id in recipients {
                let mut output = if self.entry.is_some() {
                    let avatar = self.sessions[&id].avatar.expect("loaded viewer");
                    self.worlds
                        .get_mut(&world)
                        .context("publication world missing")?
                        .publication(avatar, &packets, &self.content)?
                } else {
                    packets
                        .iter()
                        .map(|p| Packet {
                            id: p.id,
                            body: p.body.clone(),
                        })
                        .collect()
                };
                if teleport_requester == Some(id) {
                    output.push(Packet {
                        id: 29,
                        body: vec![],
                    });
                }
                batches.push((id, output));
            }
            let mut failed = vec![];
            for (id, output) in batches {
                if !self.emit(id, output) {
                    failed.push(id);
                }
            }
            // Finish each fan-out before publishing removals, so an arrival cannot follow its Drop.
            for id in failed {
                if let Some((world, out)) = self.remove_session(id, false) {
                    pending.push_back((world, None, out, None));
                }
            }
        }
        Ok(())
    }
    fn refresh_lease(&mut self, session: u64) {
        let Some(s) = self.sessions.get(&session) else {
            return;
        };
        let Some(avatar) = s.avatar else {
            return;
        };
        match self.worlds[&s.world].reachable_portal(avatar) {
            Some((portal, destination)) => {
                if !self.leases.contains_key(&s.guid) && self.leases.len() >= MAX_LEASES {
                    tracing::warn!(session, "build100 destination lease capacity reached");
                    return;
                }
                self.leases.insert(
                    s.guid.clone(),
                    DestinationLease {
                        origin: session,
                        world: s.world,
                        portal,
                        destination,
                        generation: s.generation.expect("loaded generation"),
                        expires: self.now + LEASE_TIME,
                    },
                );
            }
            None => {
                self.leases.remove(&s.guid);
            }
        }
    }
    fn detach(&mut self, session: u64, preserve_lease: bool) -> Result<()> {
        if let Some((world, out)) = self.remove_session(session, preserve_lease) {
            self.broadcast(world, None, out)?;
        }
        self.retire_private_maps();
        Ok(())
    }
    fn remove_session(&mut self, session: u64, preserve_lease: bool) -> Option<(i32, Vec<Packet>)> {
        if preserve_lease {
            self.refresh_lease(session);
        }
        let s = self.sessions.remove(&session)?;
        if let Some(waiting) = self.deferred_weapons.remove(&session) {
            let action = if waiting.movement.is_some() {
                "MOVE"
            } else {
                "weapon"
            };
            tracing::debug!(target: "build100_move", session, action, "deferred input cancelled: owner detached");
            let _ = waiting
                .reply
                .send(Err(format!("{action} cancelled: owner detached")));
        }
        if self.owners.get(&s.guid) == Some(&session) {
            self.owners.remove(&s.guid);
        }
        if !preserve_lease
            && self
                .leases
                .get(&s.guid)
                .is_some_and(|l| l.origin == session)
        {
            self.leases.remove(&s.guid);
        }
        let out = s.avatar.map(|avatar| {
            let packets = self
                .worlds
                .get_mut(&s.world)
                .expect("registered world")
                .detach(avatar);
            (s.world, packets)
        });
        tracing::info!(
            session,
            world = s.world,
            generation = s.generation,
            "build100 owner detached"
        );
        out
    }
    fn join(&mut self, hello: protocol::Hello, output: Output) -> Result<u64> {
        ensure!(
            self.sessions.len() < MAX_SESSIONS || self.owners.contains_key(&hello.guid),
            "session capacity reached"
        );
        let mut transfer_generation = None;
        let mut tutorial_exit = false;
        let normal = self.entry.is_some();
        let is_transfer = hello.game_id > 0
            || (normal && hello.game_id == 0 && self.owners.contains_key(&hello.guid));
        let world = if is_transfer {
            ensure!(
                self.worlds.contains_key(&hello.game_id),
                "unknown destination"
            );
            if let Some(&origin) = self.owners.get(&hello.guid) {
                self.refresh_lease(origin);
            }
            let lease = self
                .leases
                .get(&hello.guid)
                .context("destination requires reachable live portal lease")?;
            ensure!(
                lease.destination == hello.game_id && self.now < lease.expires,
                "stale or mismatched destination lease"
            );
            let saved = character(&self.db, &hello.guid)?.context("transfer character missing")?;
            ensure!(
                saved.alive && saved.generation == lease.generation,
                "dead or stale transfer generation"
            );
            if let Some(&owner) = self.owners.get(&hello.guid) {
                ensure!(owner == lease.origin, "transfer owner changed");
                let s = &self.sessions[&owner];
                ensure!(
                    s.world == lease.world
                        && s.generation == Some(lease.generation)
                        && self.worlds[&s.world]
                            .reachable_portal(s.avatar.context("origin not loaded")?)
                            == Some((lease.portal, lease.destination)),
                    "origin portal is no longer reachable"
                );
            }
            transfer_generation = Some(lease.generation);
            tutorial_exit = normal
                && self.worlds[&lease.world]
                    .map
                    .as_ref()
                    .is_some_and(|m| m.kind == Kind::Tutorial);
            let origin = lease.origin;
            self.leases.remove(&hello.guid);
            self.detach(origin, false)?;
            tracing::info!(
                origin,
                world = hello.game_id,
                entry = "portal",
                "build100 exact owner handoff"
            );
            hello.game_id
        } else if normal && hello.game_id == -1 {
            ensure!(
                !self.owners.contains_key(&hello.guid),
                "this character is already connected"
            );
            self.leases.remove(&hello.guid);
            self.retire_private_maps();
            ensure!(
                self.worlds.len() <= MAX_SESSIONS,
                "private Tutorial capacity reached"
            );
            self.next_world = self
                .next_world
                .checked_add(1)
                .context("dynamic game ID exhausted")?;
            let world = self.next_world;
            let (grounds, seed) = self.entry.as_ref().expect("normal entry");
            let seed = seed.wrapping_add(world as u64);
            let map = Arc::new(realm_map::tutorial(seed, &self.content, grounds.clone())?);
            let base = (3..=19)
                .map(|n| n * 500_000)
                .find(|base| self.worlds.values().all(|r| r.entity_partition() != *base))
                .context("private map entity capacity reached")?;
            self.worlds
                .insert(world, Room::from_map(map, &self.content, base)?);
            tracing::info!(
                world,
                seed,
                "build100 reconstructed private Tutorial generated"
            );
            world
        } else {
            ensure!(
                hello.game_id == 0 || (!normal && hello.game_id == -1),
                "unknown entry route"
            );
            ensure!(
                !self.owners.contains_key(&hello.guid),
                "this character is already connected"
            );
            if normal {
                let saved = character(&self.db, &hello.guid)?;
                let lease = self.leases.remove(&hello.guid);
                let reason = if let Some(l) = lease {
                    if l.destination == 0
                        && self.now < l.expires
                        && saved
                            .as_ref()
                            .is_some_and(|p| p.alive && p.generation == l.generation)
                    {
                        transfer_generation = Some(l.generation);
                        tutorial_exit = self
                            .worlds
                            .get(&l.world)
                            .and_then(|r| r.map.as_ref())
                            .is_some_and(|m| m.kind == Kind::Tutorial);
                        "portal"
                    } else if self.now >= l.expires {
                        "expired"
                    } else {
                        "generation/destination mismatch"
                    }
                } else {
                    "none"
                };
                tracing::info!(
                    world = 0,
                    entry = if transfer_generation.is_some() {
                        "portal"
                    } else {
                        "saved"
                    },
                    reason,
                    "build100 realm admission"
                );
                0
            } else {
                self.leases.remove(&hello.guid);
                1
            }
        };
        self.next_session = self
            .next_session
            .checked_add(1)
            .context("session ID exhausted")?;
        let session = self.next_session;
        self.owners.insert(hello.guid.clone(), session);
        self.sessions.insert(
            session,
            Session {
                guid: hello.guid,
                world,
                avatar: None,
                generation: None,
                output,
                last_input: self.now,
                next_ping: self.now + PING_INTERVAL,
                ping: None,
                transfer_generation,
                tutorial_exit,
            },
        );
        let mut w = Writer::new();
        let (width, height, name) = self.worlds[&world].description(world);
        w.i32(width).i32(height).utf(&name).i32(0);
        if !self.emit(session, vec![Packet { id: 24, body: w.0 }]) {
            self.detach(session, false)?;
            anyhow::bail!("join output unavailable");
        }
        tracing::info!(session, world, "build100 HELLO accepted");
        Ok(session)
    }
    fn commit(&self, candidate: &Room, before: &Room) -> Result<()> {
        let mut db = self.db.lock().expect("database mutex poisoned");
        let tx = db.transaction().context("begin build100 world mutation")?;
        for (&id, avatar) in &candidate.players {
            if before
                .players
                .get(&id)
                .is_some_and(|a| a.character == avatar.character)
            {
                continue;
            }
            let s = self
                .sessions
                .values()
                .find(|s| s.avatar == Some(id))
                .context("avatar lacks exclusive session")?;
            let state = serde_json::to_string(&avatar.character)?;
            tx.execute("INSERT INTO build100_characters(guid,state) VALUES(?,?) ON CONFLICT(guid) DO UPDATE SET state=excluded.state", [&s.guid,&state]).context("persist build100 avatar mutation")?;
        }
        tx.commit().context("commit build100 world mutation")?;
        Ok(())
    }
    fn validate_room_owners(&self, world: i32) -> Result<()> {
        let room = self.worlds.get(&world).context("selected world missing")?;
        for (&avatar, attached) in &room.players {
            let mut bindings = self
                .sessions
                .iter()
                .filter(|(_, s)| s.avatar == Some(avatar));
            let (id, owner) = bindings
                .next()
                .context("Room avatar lacks current session")?;
            ensure!(
                bindings.next().is_none()
                    && owner.world == world
                    && owner.generation == Some(attached.character.generation)
                    && self.owners.get(&owner.guid) == Some(id)
                    && self
                        .owners
                        .values()
                        .filter(|owner_id| *owner_id == id)
                        .count()
                        == 1,
                "Room avatar current ownership invariant failed: avatar {avatar}"
            );
        }
        for owner in self.sessions.values().filter(|s| s.world == world) {
            if let Some(avatar) = owner.avatar {
                ensure!(
                    room.players.contains_key(&avatar),
                    "orphan selected Room binding"
                );
            }
        }
        Ok(())
    }
    fn input(&mut self, session: u64, packet: Packet, received: Instant) -> Result<()> {
        self.input_at(session, packet, received, Instant::now())
    }
    fn input_at(
        &mut self,
        session: u64,
        packet: Packet,
        received: Instant,
        now: Instant,
    ) -> Result<()> {
        let s = self.sessions.get(&session).context("stale session token")?;
        ensure!(
            self.owners.get(&s.guid) == Some(&session),
            "session does not own character"
        );
        if packet.id == protocol::PONG {
            let serial = protocol::pong(&packet.body)?;
            ensure!(
                s.ping
                    .is_some_and(|(expected, deadline)| serial == expected && self.now < deadline),
                "unsolicited, stale or false Pong"
            );
            self.sessions
                .get_mut(&session)
                .expect("checked session")
                .ping = None;
            return Ok(());
        }
        let world = s.world;
        if matches!(packet.id, 8 | 15) {
            let room = self.worlds.get(&world).context("selected world missing")?;
            if room.map.is_some() {
                ensure!(s.avatar.is_some(), "character selection required");
                self.validate_room_owners(world)?;
            }
        }
        let teleport_target = if packet.id == 28 {
            let avatar = s.avatar.context("character selection required")?;
            let attached = self.worlds[&world]
                .players
                .get(&avatar)
                .context("selected avatar missing")?;
            ensure!(
                s.generation == Some(attached.character.generation),
                "selected generation no longer attached"
            );
            Some(protocol::teleport(&packet.body)?)
        } else {
            None
        };
        let player_move = packet.id == 5;
        let _move_trace = player_move.then(|| {
            tracing::debug_span!(
                target: "build100_move",
                "MOVE owner",
                session,
                world,
                avatar = s.avatar,
                generation = s.generation,
                received = ?received,
                owner_now = ?now
            )
            .entered()
        });
        let player_shot = packet.id == 8;
        let _trace = player_shot.then(|| {
            tracing::debug_span!(
                "build100_weapon_input",
                session,
                world,
                avatar = s.avatar,
                generation = s.generation,
                bullet = packet.body.first().copied(),
                received_age_ms = received.elapsed().as_secs_f64() * 1000.0
            )
            .entered()
        });
        let candidate_started = Instant::now();
        if player_shot {
            tracing::debug!("build100 weapon candidate begin");
        }
        if player_move {
            tracing::debug!(target: "build100_move", candidate_started=?candidate_started, "MOVE candidate begin");
        }
        let mut candidate = self.worlds[&world].clone();
        let mut output;
        let mut arrival = None;
        if packet.id == 3 || packet.id == 25 {
            ensure!(s.avatar.is_none(), "character already loaded");
            let previous = character(&self.db, &s.guid)?;
            let mut character = match protocol::Selection::parse(&packet)? {
                protocol::Selection::Create { kind, name } => {
                    ensure!(
                        !previous.as_ref().is_some_and(|p| p.alive),
                        "load the living character before creating another"
                    );
                    Character::new(
                        &self.content,
                        kind,
                        name,
                        previous.map_or(Ok(1), |p| {
                            p.generation
                                .checked_add(1)
                                .context("character generation exhausted")
                        })?,
                    )?
                }
                protocol::Selection::Load { character_id } => {
                    ensure!(character_id == 0, "unknown saved character");
                    let p = previous.context("saved character missing")?;
                    ensure!(p.alive, "character is permanently dead");
                    p
                }
            };
            if let Some(generation) = s.transfer_generation {
                ensure!(
                    character.generation == generation,
                    "selection does not match transfer generation"
                );
            }
            let (x, y) = candidate.landing();
            character.x = x;
            character.y = y;
            if s.tutorial_exit {
                character.tutorial_complete = true;
            }
            self.next_avatar = self
                .next_avatar
                .checked_add(1)
                .context("avatar ID exhausted")?;
            ensure!(self.next_avatar < 1_000_000, "avatar ID space exhausted");
            let id = self.next_avatar;
            let generation = character.generation;
            output = candidate.attach(id, character, &self.content)?;
            arrival = Some(candidate.arrival(id, &self.content)?);
            let s = self.sessions.get_mut(&session).expect("checked session");
            s.avatar = Some(id);
            s.generation = Some(generation);
        } else {
            let result = candidate
                .handle_at(
                    s.avatar.context("character selection required")?,
                    packet,
                    &self.content,
                    received,
                    now,
                )
                .and_then(|output| {
                    if let Some(target) = teleport_target {
                        let generation = candidate
                            .players
                            .get(&target)
                            .context("accepted teleport target missing")?
                            .character
                            .generation;
                        let owned = self.sessions.iter().any(|(id, owner)| {
                            owner.world == world
                                && owner.avatar == Some(target)
                                && owner.generation == Some(generation)
                                && self.owners.get(&owner.guid) == Some(id)
                        });
                        if !owned {
                            return Err(crate::world::TeleportDecline::TargetUnavailable.into());
                        }
                    }
                    Ok(output)
                });
            if player_shot {
                tracing::debug!(elapsed_ms=candidate_started.elapsed().as_secs_f64()*1000.0,success=result.is_ok(),output_count=result.as_ref().ok().map(Vec::len),error=?result.as_ref().err(),"build100 weapon candidate end");
            }
            if player_move {
                tracing::debug!(target: "build100_move", candidate_ended=?Instant::now(), accepted=result.is_ok(), error=?result.as_ref().err(), "MOVE candidate end; not committed or installed");
            }
            output = match result {
                Ok(output) => output,
                Err(error) => {
                    if teleport_target.is_some()
                        && error.is::<crate::world::TeleportDecline>()
                        && !self.emit(
                            session,
                            vec![Packet {
                                id: 29,
                                body: vec![],
                            }],
                        )
                    {
                        self.detach(session, false)?;
                    }
                    return Err(error);
                }
            };
        }
        let commit_started = Instant::now();
        if player_shot {
            tracing::debug!("build100 weapon commit begin");
        }
        // A storage error is fatal to the actor: never acknowledge or publish a candidate that did not commit.
        let committed = self.commit(&candidate, &self.worlds[&world]);
        if player_move {
            tracing::debug!(target: "build100_move", commit_started=?commit_started, commit_ended=?Instant::now(), committed=committed.is_ok(), "MOVE SQL commit end; not installed");
        }
        if player_shot {
            tracing::debug!(
                elapsed_ms = commit_started.elapsed().as_secs_f64() * 1000.0,
                success = committed.is_ok(),
                "build100 weapon commit end"
            );
        }
        if let Err(error) = committed {
            if arrival.is_some() {
                let s = self.sessions.get_mut(&session).expect("checked session");
                s.avatar = None;
                s.generation = None;
            }
            return Err(error.context(StorageFailure));
        }
        self.worlds.insert(world, candidate);
        if player_move {
            tracing::debug!(target: "build100_move", installed=?Instant::now(), "MOVE committed candidate installed; before publication");
        }
        if arrival.is_some() && self.entry.is_some() {
            let avatar = self.sessions[&session].avatar.expect("loaded avatar");
            output = self
                .worlds
                .get_mut(&world)
                .expect("registered world")
                .publication(avatar, &output, &self.content)?;
        }
        self.sessions
            .get_mut(&session)
            .expect("checked session")
            .last_input = self.now;
        self.refresh_lease(session);
        if let Some(arrival) = arrival {
            let s = &self.sessions[&session];
            tracing::info!(
                session,
                world,
                avatar = s.avatar,
                generation = s.generation,
                "build100 avatar durably attached"
            );
            if self.emit(session, output) {
                self.broadcast(world, Some(session), vec![arrival])?;
            } else {
                self.detach(session, false)?;
            }
        } else {
            let publication_started = Instant::now();
            if player_shot {
                tracing::debug!(
                    output_count = output.len(),
                    "build100 weapon publication begin; committed candidate installed"
                );
            }
            let published = self.broadcast_input(
                world,
                player_shot.then_some(session),
                output,
                teleport_target.map(|_| session),
            );
            if player_shot {
                tracing::debug!(
                    elapsed_ms = publication_started.elapsed().as_secs_f64() * 1000.0,
                    success = published.is_ok(),
                    "build100 weapon publication end; bounded enqueue only, not peer delivery"
                );
            }
            published?;
        }
        self.cancel_lost_weapons();
        Ok(())
    }
    fn waiting_owner(&self, session: u64, waiting: &DeferredWeapon) -> bool {
        self.sessions.get(&session).is_some_and(|s| {
            s.world == waiting.world
                && s.avatar == Some(waiting.avatar)
                && s.generation == Some(waiting.generation)
                && self.owners.get(&s.guid) == Some(&session)
        }) && self
            .worlds
            .get(&waiting.world)
            .and_then(|r| r.players.get(&waiting.avatar))
            .is_some_and(|p| p.character.alive && p.character.generation == waiting.generation)
    }
    fn cancel_lost_weapons(&mut self) {
        let lost: Vec<_> = self
            .deferred_weapons
            .iter()
            .filter(|(session, w)| {
                w.reply.is_closed()
                    || !self.waiting_owner(**session, w)
                    || (w.movement.is_none()
                        && self.worlds[&w.world].weapon_deadline(w.avatar).is_none())
            })
            .map(|(&id, _)| id)
            .collect();
        for session in lost {
            let w = self
                .deferred_weapons
                .remove(&session)
                .expect("waiting weapon");
            if w.movement.is_none()
                && let Some(room) = self.worlds.get_mut(&w.world)
                && room
                    .players
                    .get(&w.avatar)
                    .is_some_and(|p| p.character.generation == w.generation)
            {
                room.cancel_weapon(w.avatar);
            }
            let action = if w.movement.is_some() {
                "MOVE"
            } else {
                "weapon"
            };
            tracing::warn!(
                session,
                avatar = w.avatar,
                action,
                "build100 waiting input cancelled: owner/life changed"
            );
            if w.movement.is_some() {
                tracing::debug!(target: "build100_move", session, avatar=w.avatar, "MOVE wait cancelled: reply/owner/life changed");
            }
            let _ = w
                .reply
                .send(Err(format!("{action} cancelled: owner/life changed")));
        }
    }
    fn deferred_deadline(&self, w: &DeferredWeapon) -> Option<Instant> {
        match &w.movement {
            Some(m) => Some(m.due),
            None => self.worlds.get(&w.world)?.weapon_deadline(w.avatar),
        }
    }
    fn weapon_deadline(&self) -> Option<Instant> {
        self.deferred_weapons
            .values()
            .filter_map(|w| self.deferred_deadline(w))
            .min()
    }
    fn release_weapons(&mut self, now: Instant) -> Result<()> {
        self.cancel_lost_weapons();
        let due = self
            .deferred_weapons
            .iter()
            .find(|(_, w)| self.deferred_deadline(w).is_some_and(|d| now >= d))
            .map(|(&id, _)| id);
        if let Some(session) = due {
            if self.deferred_weapons[&session].movement.is_some() {
                return self.release_move(session);
            }
            let w = &self.deferred_weapons[&session];
            let (world, avatar) = (w.world, w.avatar);
            let mut candidate = self.worlds[&world].clone();
            let result = candidate.release_weapon(avatar, &self.content, now);
            if result.is_err() {
                candidate.cancel_weapon(avatar);
            }
            self.commit(&candidate, &self.worlds[&world])
                .context(StorageFailure)?;
            self.worlds.insert(world, candidate);
            let (packets, reply) = match result {
                Ok(p) => (p, Ok(())),
                Err(e) => (vec![], Err(format!("weapon cancelled at funding: {e:#}"))),
            };
            if !packets.is_empty() {
                self.broadcast(world, Some(session), packets)?;
            }
            if let Some(w) = self.deferred_weapons.remove(&session) {
                tracing::debug!(session,avatar,published=?Instant::now(),success=reply.is_ok(),"build100 deferred weapon committed/published; replying");
                let _ = w.reply.send(reply);
            }
        }
        Ok(())
    }
    fn release_move(&mut self, session: u64) -> Result<()> {
        let m = self.deferred_weapons[&session]
            .movement
            .as_ref()
            .expect("waiting MOVE");
        let (packet, received, boundary) = (
            Packet {
                id: m.packet.id,
                body: m.packet.body.clone(),
            },
            m.received,
            m.boundary,
        );
        tracing::debug!(target: "build100_move", session, due=?m.due, boundary=?boundary, wake=?Instant::now(), "MOVE wait wake; fresh validation required");
        let result = self.input(session, packet, received);
        if result
            .as_ref()
            .err()
            .is_some_and(|e| e.is::<StorageFailure>())
        {
            return result;
        }
        if let Some(due) = result
            .as_ref()
            .err()
            .and_then(|e| e.downcast_ref::<crate::world::MoveBudget>())
            .and_then(|budget| budget.deadline(boundary))
        {
            self.deferred_weapons
                .get_mut(&session)
                .expect("waiting MOVE")
                .movement
                .as_mut()
                .expect("waiting MOVE")
                .due = due;
            tracing::debug!(target: "build100_move", session, ?due, ?boundary, "MOVE still unfunded; retry within original boundary");
            return Ok(());
        }
        if let Some(w) = self.deferred_weapons.remove(&session) {
            tracing::debug!(target: "build100_move", session, success=result.is_ok(), error=?result.as_ref().err(), "MOVE wait finished; replying");
            let _ = w.reply.send(result.map_err(|e| format!("{e:#}")));
        }
        Ok(())
    }
    fn submit_input(
        &mut self,
        session: u64,
        packet: Packet,
        received: Instant,
        reply: oneshot::Sender<Result<(), String>>,
    ) -> Result<()> {
        if self.deferred_weapons.contains_key(&session) {
            let _ = reply.send(Err(
                "complete action awaiting reply; session dispatcher must backpressure".into(),
            ));
            return Ok(());
        }
        let movement = (packet.id == 5 && packet.body.len() == 8).then(|| Packet {
            id: packet.id,
            body: packet.body.clone(),
        });
        let result = self.input(session, packet, received);
        if let Some(budget) = result
            .as_ref()
            .err()
            .and_then(|e| e.downcast_ref::<crate::world::MoveBudget>())
        {
            let boundary = budget.wait_boundary();
            if let Some(due) = budget.deadline(boundary) {
                let s = &self.sessions[&session];
                self.deferred_weapons.insert(
                    session,
                    DeferredWeapon {
                        movement: Some(PendingMove {
                            packet: movement.expect("validated MOVE"),
                            received,
                            due,
                            boundary,
                        }),
                        world: s.world,
                        avatar: s.avatar.expect("validated avatar"),
                        generation: s.generation.expect("validated generation"),
                        reply,
                    },
                );
                tracing::debug!(target: "build100_move", session, ?received, ?due, ?boundary, "MOVE waiting for existing elapsed budget; no install/SQL/publication");
                return Ok(());
            }
        }
        self.reply_input(session, result, reply)
    }
    fn reply_input(
        &mut self,
        session: u64,
        result: Result<()>,
        reply: oneshot::Sender<Result<(), String>>,
    ) -> Result<()> {
        let fatal = result
            .as_ref()
            .err()
            .is_some_and(|e| e.is::<StorageFailure>());
        let waiting = if result.is_ok() {
            self.sessions
                .get(&session)
                .and_then(|s| Some((s.world, s.avatar?, s.generation?)))
                .filter(|(world, avatar, _)| self.worlds[world].weapon_deadline(*avatar).is_some())
        } else {
            None
        };
        if let (Ok(()), Some((world, avatar, generation))) = (&result, waiting) {
            ensure!(
                !self.deferred_weapons.contains_key(&session),
                "duplicate deferred weapon response"
            );
            self.deferred_weapons.insert(
                session,
                DeferredWeapon {
                    movement: None,
                    world,
                    avatar,
                    generation,
                    reply,
                },
            );
        } else {
            let _ = reply.send(result.as_ref().map(|_| ()).map_err(|e| format!("{e:#}")));
        }
        if fatal { result } else { Ok(()) }
    }
    fn tick(&mut self, dt: Duration) -> Result<()> {
        self.now += dt;
        let expired: Vec<_> = self
            .sessions
            .iter()
            .filter(|(_, s)| {
                s.output.is_closed()
                    || self.now - s.last_input >= INPUT_TIMEOUT
                    || s.ping.is_some_and(|(_, deadline)| self.now >= deadline)
            })
            .map(|(&id, _)| id)
            .collect();
        for session in expired {
            let Some(s) = self.sessions.get(&session) else {
                continue;
            };
            let closed = s.output.is_closed();
            tracing::info!(session, closed, "build100 inactive/closed consumer cleanup");
            self.detach(session, closed)?;
        }
        self.leases.retain(|_, l| {
            let live=self.now<l.expires;
            if !live {tracing::info!(origin=l.origin,world=l.world,generation=l.generation,reason="expired","build100 portal authorization expired; game0 now requires independent saved admission");}
            live
        });
        self.retire_private_maps();
        let ids: Vec<_> = self.sessions.keys().copied().collect();
        let mut failed = vec![];
        for session in ids {
            let Some(s) = self.sessions.get_mut(&session) else {
                continue;
            };
            if s.ping.is_none() && self.now >= s.next_ping {
                self.serial = self
                    .serial
                    .checked_add(1)
                    .context("Ping serial exhausted")?;
                s.ping = Some((self.serial, self.now + PING_TIMEOUT));
                s.next_ping = self.now + PING_INTERVAL;
                let mut w = Writer::new();
                w.i32(self.serial);
                if !self.emit(session, vec![Packet { id: 22, body: w.0 }]) {
                    failed.push(session);
                }
            }
        }
        for session in failed {
            self.detach(session, false)?;
        }
        let worlds: Vec<_> = self.worlds.keys().copied().collect();
        for world in worlds {
            let mut candidate = self.worlds[&world].clone();
            let out = candidate.advance(dt.as_secs_f64(), &self.content)?;
            self.commit(&candidate, &self.worlds[&world])
                .context(StorageFailure)?;
            self.worlds.insert(world, candidate);
            self.broadcast(world, None, out)?;
        }
        self.cancel_lost_weapons();
        Ok(())
    }
    pub async fn run(
        mut self,
        mut commands: mpsc::Receiver<Command>,
        realtime: bool,
    ) -> Result<()> {
        let mut timer = tokio::time::interval(Duration::from_millis(50));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_tick = tokio::time::Instant::now();
        loop {
            let deadline = self.weapon_deadline();
            tokio::select! {
                biased;
                _=async { match deadline {
                    Some(d)=>tokio::time::sleep_until(d.into()).await,
                    None=>std::future::pending::<()>().await,
                }} => self.release_weapons(Instant::now())?,
                _=timer.tick(), if realtime => {
                    let now=tokio::time::Instant::now();
                    self.tick(now.duration_since(last_tick))?;
                    last_tick=now;
                },
                command=commands.recv()=>match command {
                    Some(Command::Join{hello,output,reply})=>{let _=reply.send(self.join(hello,output).map_err(|e|format!("{e:#}")));}
                    Some(Command::Input{session,packet,received,reply})=>{
                        self.submit_input(session,packet,received,reply)?;
                    }
                    Some(Command::Detach{session})=>self.detach(session,true)?,
                    #[cfg(test)]
                    Some(Command::Tick(dt,reply))=>{let result=self.tick(dt);let failed=result.is_err();let _=reply.send(result.as_ref().map(|_|()).map_err(|e|format!("{e:#}")));if failed{return result;}}
                    #[cfg(test)]
                    Some(Command::Inspect(f))=>f(&mut self),
                    None=>return Ok(()),
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "move_pacing_tests.rs"]
mod move_pacing_tests;

#[cfg(test)]
#[path = "cadence_owner_tests.rs"]
mod cadence_owner_tests;

#[cfg(test)]
#[path = "shared_lifecycle.rs"]
mod tests;

#[cfg(test)]
mod progression_owner_tests {
    use super::*;
    use crate::{database, wire::Reader, world::progression_runtime_tests as rewards};
    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn packet(id: u8, f: impl FnOnce(&mut Writer)) -> Packet {
        let mut w = Writer::new();
        f(&mut w);
        Packet { id, body: w.0 }
    }
    fn setup(count: usize) -> Simulation {
        let (mut content, _) = rewards::setup();
        rewards::guaranteed(&mut content, count);
        let path = std::env::var_os("BUILD100_CONTENT_ORIGINAL_DIR").unwrap();
        let grounds = Arc::new(Grounds::load(std::path::Path::new(&path)).unwrap());
        let db = Arc::new(Mutex::new(
            database(std::path::Path::new(":memory:")).unwrap(),
        ));
        Simulation::normal(content, grounds, db, 100).unwrap()
    }
    fn enter(s: &mut Simulation, guid: &str) -> (u64, i32, mpsc::Receiver<Vec<Packet>>) {
        let (out, receiver) = mpsc::channel(OUTPUT_CAPACITY);
        let id = s
            .join(
                protocol::Hello {
                    guid: guid.into(),
                    game_id: 0,
                },
                out,
            )
            .unwrap();
        s.input(
            id,
            packet(3, |w| {
                w.i16(0x30e).utf("Earned");
            }),
            Instant::now(),
        )
        .unwrap();
        (id, s.sessions[&id].avatar.unwrap(), receiver)
    }
    fn drain(receiver: &mut mpsc::Receiver<Vec<Packet>>) -> Vec<Packet> {
        let mut packets = vec![];
        while let Ok(batch) = receiver.try_recv() {
            packets.extend(batch);
        }
        packets
    }
    fn prepare(s: &mut Simulation, ids: &[i32]) {
        let before = s.worlds[&0].clone();
        let room = s.worlds.get_mut(&0).unwrap();
        for &id in ids {
            room.character_mut(id)
                .stats
                .extend([(6, 49), (1, 40), (4, 30)]);
        }
        s.commit(&s.worlds[&0], &before).unwrap();
        rewards::encounter(s.worlds.get_mut(&0).unwrap(), &s.content, ids[0], 3);
    }
    fn tick_stats(packets: &[Packet]) -> BTreeMap<i32, BTreeMap<u8, i32>> {
        let mut statuses = BTreeMap::new();
        for p in packets.iter().filter(|p| p.id == 13) {
            let mut r = Reader::new(&p.body);
            for _ in 0..r.i16().unwrap() {
                let id = r.i32().unwrap();
                r.f32().unwrap();
                r.f32().unwrap();
                let stats = (0..r.i16().unwrap())
                    .map(|_| (r.u8().unwrap(), r.i32().unwrap()))
                    .collect();
                statuses.insert(id, stats);
            }
        }
        statuses
    }
    #[test]
    fn progression_runtime_owner_group_commit_peer_stats_pickup_conservation_and_reconnect() {
        let mut s = setup(1);
        let (a, avatar_a, mut out_a) = enter(&mut s, A);
        let (b, avatar_b, mut out_b) = enter(&mut s, B);
        prepare(&mut s, &[avatar_a, avatar_b]);
        drain(&mut out_a);
        drain(&mut out_b);
        s.tick(Duration::ZERO).unwrap();
        let events_a = drain(&mut out_a);
        let events_b = drain(&mut out_b);
        for (&avatar, guid) in [(&avatar_a, A), (&avatar_b, B)] {
            let saved = character(&s.db, guid).unwrap().unwrap();
            assert_eq!(saved, *s.worlds[&0].character(avatar));
            assert_eq!(
                (saved.stats[&5], saved.stats[&6], saved.stats[&7]),
                (150, 0, 2)
            );
            for events in [&events_a, &events_b] {
                let stats = tick_stats(events);
                for stat in [0, 1, 3, 4, 5, 6, 7] {
                    assert_eq!(stats[&avatar][&stat], saved.stats[&stat]);
                }
            }
        }
        let bags = rewards::bags(&s.worlds[&0]);
        assert_eq!(bags.len(), 1);
        let bag = bags[0].0;
        let slot = (4..12)
            .find(|slot| s.worlds[&0].character(avatar_a).stats[&(8 + slot)] == -1)
            .unwrap();
        let pickup = |avatar| {
            packet(14, |w| {
                w.i32(bag).u8(0).i16(0xa22).i32(avatar).u8(slot).i16(-1);
            })
        };
        s.input(a, pickup(avatar_a), Instant::now()).unwrap();
        assert!(s.input(b, pickup(avatar_b), Instant::now()).is_err());
        assert_eq!(
            character(&s.db, A).unwrap().unwrap().stats[&(8 + slot)],
            0xa22
        );
        assert_eq!(character(&s.db, B).unwrap().unwrap().stats[&(8 + slot)], -1);
        s.tick(Duration::ZERO).unwrap();
        assert!(rewards::bags(&s.worlds[&0]).is_empty());
        let saved = character(&s.db, A).unwrap().unwrap();
        s.detach(a, false).unwrap();
        let (out, mut receiver) = mpsc::channel(OUTPUT_CAPACITY);
        let new = s
            .join(
                protocol::Hello {
                    guid: A.into(),
                    game_id: 0,
                },
                out,
            )
            .unwrap();
        s.input(
            new,
            packet(25, |w| {
                w.i32(0).utf("Advisory");
            }),
            Instant::now(),
        )
        .unwrap();
        assert_eq!(
            s.worlds[&0].character(s.sessions[&new].avatar.unwrap()),
            &saved
        );
        assert!(!drain(&mut receiver).is_empty());
        assert_eq!(saved.generation, 1);
    }
    #[test]
    fn progression_runtime_owner_failed_group_sql_rolls_back_growth_bags_rng_output_and_retry() {
        let mut s = setup(9);
        let (_, a, mut out_a) = enter(&mut s, A);
        let (_, b, mut out_b) = enter(&mut s, B);
        prepare(&mut s, &[a, b]);
        drain(&mut out_a);
        drain(&mut out_b);
        let before = s.worlds[&0].clone();
        let before_a = character(&s.db, A).unwrap().unwrap();
        let before_b = character(&s.db, B).unwrap().unwrap();
        s.db.lock().unwrap().execute_batch(&format!("CREATE TRIGGER fail_group BEFORE UPDATE ON build100_characters WHEN NEW.guid='{B}' BEGIN SELECT RAISE(FAIL,'test group commit failure'); END;")).unwrap();
        assert!(s.tick(Duration::ZERO).unwrap_err().is::<StorageFailure>());
        assert_eq!(character(&s.db, A).unwrap().unwrap(), before_a);
        assert_eq!(character(&s.db, B).unwrap().unwrap(), before_b);
        assert_eq!(s.worlds[&0].character(a), &before_a);
        assert_eq!(s.worlds[&0].character(b), &before_b);
        assert_eq!(
            rewards::reward_state(&s.worlds[&0]),
            rewards::reward_state(&before)
        );
        assert!(rewards::bags(&s.worlds[&0]).is_empty());
        assert!(drain(&mut out_a).is_empty() && drain(&mut out_b).is_empty());
        let mut expected = before;
        expected.advance(0., &s.content).unwrap();
        s.db.lock()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_group")
            .unwrap();
        s.tick(Duration::ZERO).unwrap();
        assert_eq!(s.worlds[&0].character(a), expected.character(a));
        assert_eq!(s.worlds[&0].character(b), expected.character(b));
        assert_eq!(rewards::bags(&s.worlds[&0]), rewards::bags(&expected));
        assert_eq!(
            rewards::reward_state(&s.worlds[&0]),
            rewards::reward_state(&expected)
        );
        let once = character(&s.db, A).unwrap().unwrap();
        s.tick(Duration::ZERO).unwrap();
        assert_eq!(character(&s.db, A).unwrap().unwrap(), once);
    }
    #[test]
    fn progression_runtime_owner_validation_capacity_rollback_and_other_instance_isolation() {
        for failure in 0..4 {
            let mut s = setup(if failure == 0 { 65 } else { 1 });
            let (_, a, mut out) = enter(&mut s, A);
            prepare(&mut s, &[a]);
            drain(&mut out);
            if failure == 1 {
                s.content
                    .objects
                    .get_mut(&0x600)
                    .unwrap()
                    .loot
                    .push(crate::content::Loot::Item {
                        name: "Missing".into(),
                        probability: 1.,
                    });
            }
            if failure == 2 {
                s.worlds
                    .get_mut(&0)
                    .unwrap()
                    .character_mut(a)
                    .stats
                    .insert(6, 50);
            }
            if failure == 3 {
                rewards::exhaust_loot_ids(s.worlds.get_mut(&0).unwrap());
            }
            let before = s.worlds[&0].clone();
            let durable = character(&s.db, A).unwrap().unwrap();
            assert!(s.tick(Duration::ZERO).is_err());
            assert_eq!(s.worlds[&0].character(a), before.character(a));
            assert_eq!(
                rewards::reward_state(&s.worlds[&0]),
                rewards::reward_state(&before)
            );
            assert_eq!(character(&s.db, A).unwrap().unwrap(), durable);
            assert!(drain(&mut out).is_empty());
        }
        let mut s = setup(0);
        let (_, a, mut out_a) = enter(&mut s, A);
        let (_, b, mut out_b) = enter(&mut s, B);
        let p = s
            .worlds
            .get_mut(&0)
            .unwrap()
            .players
            .remove(&b)
            .unwrap()
            .character;
        let map = Arc::new(
            realm_map::realm(101, &s.content, s.entry.as_ref().unwrap().0.clone()).unwrap(),
        );
        let mut other = Room::from_map(map, &s.content, 2_000_000).unwrap();
        other.attach(b, p, &s.content).unwrap();
        s.worlds.insert(1, other);
        let session = s.owners[B];
        s.sessions.get_mut(&session).unwrap().world = 1;
        prepare(&mut s, &[a]);
        drain(&mut out_a);
        drain(&mut out_b);
        let durable = character(&s.db, B).unwrap().unwrap();
        s.tick(Duration::ZERO).unwrap();
        assert_eq!(character(&s.db, B).unwrap().unwrap(), durable);
        assert!(drain(&mut out_b).iter().all(|p| p.id != 9));
        assert_eq!(character(&s.db, A).unwrap().unwrap().stats[&7], 2);
    }
    #[test]
    fn progression_runtime_owner_death_is_durable_and_replacement_never_inherits_rewards() {
        let mut s = setup(1);
        let (session, a, mut output) = enter(&mut s, A);
        prepare(&mut s, &[a]);
        drain(&mut output);
        s.tick(Duration::ZERO).unwrap();
        drain(&mut output);
        assert_eq!(character(&s.db, A).unwrap().unwrap().stats[&7], 2);
        rewards::lethal_enemy_bullet(s.worlds.get_mut(&0).unwrap(), a);
        s.tick(Duration::ZERO).unwrap();
        let dead = character(&s.db, A).unwrap().unwrap();
        assert!(!dead.alive);
        assert_eq!(dead.stats[&1], 0);
        assert_eq!(dead.stats[&7], 2);
        assert!(drain(&mut output).iter().any(|p| p.id == 11));
        s.detach(session, false).unwrap();
        let (out, _receiver) = mpsc::channel(OUTPUT_CAPACITY);
        let new = s
            .join(
                protocol::Hello {
                    guid: A.into(),
                    game_id: 0,
                },
                out,
            )
            .unwrap();
        assert!(
            s.input(
                new,
                packet(25, |w| {
                    w.i32(0).utf("Advisory");
                }),
                Instant::now()
            )
            .is_err()
        );
        s.input(
            new,
            packet(3, |w| {
                w.i16(0x30e).utf("Replacement");
            }),
            Instant::now(),
        )
        .unwrap();
        let replacement = character(&s.db, A).unwrap().unwrap();
        assert_eq!(replacement.generation, dead.generation + 1);
        assert_eq!(
            (
                replacement.stats[&5],
                replacement.stats[&6],
                replacement.stats[&7]
            ),
            (50, 0, 1)
        );
        assert!(replacement.alive);
        assert_eq!(
            replacement.stats[&0],
            s.content.get(0x30e).unwrap().stats[&0]
        );
    }
}

#[cfg(test)]
mod spell_owner_tests {
    use super::*;
    use crate::{database, wire::Reader, world::spell_runtime_tests as spell};
    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    fn packet(id: u8, f: impl FnOnce(&mut Writer)) -> Packet {
        let mut w = Writer::new();
        f(&mut w);
        Packet { id, body: w.0 }
    }
    fn setup() -> Simulation {
        let (content, _) = spell::setup(true);
        let path = std::env::var_os("BUILD100_CONTENT_REPAIRED_DIR").unwrap();
        let grounds = Arc::new(Grounds::load(std::path::Path::new(&path)).unwrap());
        let db = Arc::new(Mutex::new(
            database(std::path::Path::new(":memory:")).unwrap(),
        ));
        Simulation::normal(content, grounds, db, 100).unwrap()
    }
    fn enter(s: &mut Simulation, guid: &str) -> (u64, i32, mpsc::Receiver<Vec<Packet>>) {
        let (output, receive) = mpsc::channel(OUTPUT_CAPACITY);
        let session = s
            .join(
                protocol::Hello {
                    guid: guid.into(),
                    game_id: 0,
                },
                output,
            )
            .unwrap();
        s.input(
            session,
            packet(3, |w| {
                w.i16(0x30e).utf("Spell");
            }),
            Instant::now(),
        )
        .unwrap();
        (session, s.sessions[&session].avatar.unwrap(), receive)
    }
    fn drain(out: &mut mpsc::Receiver<Vec<Packet>>) -> Vec<Packet> {
        let mut packets = vec![];
        while let Ok(batch) = out.try_recv() {
            packets.extend(batch);
        }
        packets
    }
    fn fire(s: &Simulation, avatar: i32, id: u8) -> Packet {
        let p = s.worlds[&0].character(avatar);
        packet(8, |w| {
            w.u8(id)
                .i32(avatar)
                .u8(41)
                .i16(0xa97)
                .f32(p.x)
                .f32(p.y)
                .f32(0.)
                .i16(0);
        })
    }
    fn shots(events: &[Packet]) -> Vec<(u8, i32, i16)> {
        events
            .iter()
            .filter(|p| p.id == 8)
            .map(|p| {
                let mut r = Reader::new(&p.body);
                let id = r.u8().unwrap();
                let owner = r.i32().unwrap();
                assert_eq!(r.u8().unwrap(), 0);
                (id, owner, r.i16().unwrap())
            })
            .collect()
    }
    fn mp(events: &[Packet], avatar: i32) -> i32 {
        for p in events.iter().filter(|p| p.id == 13) {
            let mut r = Reader::new(&p.body);
            for _ in 0..r.i16().unwrap() {
                let id = r.i32().unwrap();
                r.f32().unwrap();
                r.f32().unwrap();
                for _ in 0..r.i16().unwrap() {
                    let stat = r.u8().unwrap();
                    let value = r.i32().unwrap();
                    if id == avatar && stat == 4 {
                        return value;
                    }
                }
            }
        }
        panic!("missing caster MP");
    }
    #[test]
    fn spell_runtime_owner_paid_cast_commit_caster_peer_repeated_ids_and_native_wrap() {
        let mut s = setup();
        let (session, a, mut out_a) = enter(&mut s, A);
        let (_, _, mut out_b) = enter(&mut s, B);
        drain(&mut out_a);
        drain(&mut out_b);
        let cursor = spell::aim(&s.worlds[&0], a);
        s.input(session, spell::cast(a, 1, a, cursor), Instant::now())
            .unwrap();
        assert_eq!(character(&s.db, A).unwrap().unwrap().stats[&4], 60);
        for events in [drain(&mut out_a), drain(&mut out_b)] {
            assert_eq!(
                shots(&events),
                (97..117).map(|id| (id, a, 0xa2e)).collect::<Vec<_>>()
            );
            assert_eq!(mp(&events, a), 60);
        }
        let receipt = Instant::now();
        let first = fire(&s, a, 96);
        s.input(session, first, receipt).unwrap();
        let second = fire(&s, a, 1);
        s.input(session, second, receipt).unwrap();
        assert!(shots(&drain(&mut out_a)).is_empty());
        assert_eq!(shots(&drain(&mut out_b)), [(96, a, 0xa97), (1, a, 0xa97)]);
        s.tick(Duration::from_millis(500)).unwrap();
        drain(&mut out_a);
        drain(&mut out_b);
        s.input(session, spell::cast(a, 1, a, cursor), Instant::now())
            .unwrap();
        assert_eq!(character(&s.db, A).unwrap().unwrap().stats[&4], 20);
        for events in [drain(&mut out_a), drain(&mut out_b)] {
            let output = shots(&events);
            assert_eq!(
                output,
                (117..137).map(|id| (id, a, 0xa2e)).collect::<Vec<_>>()
            );
            assert!(output.iter().any(|&(id, _, _)| id >= 128));
            assert_eq!(mp(&events, a), 20);
        }
        let keys: Vec<_> = spell::snapshot(&s.worlds[&0])
            .0
            .into_iter()
            .map(|(owner, id, _)| (i32::from(id) << 24) | owner)
            .collect();
        assert_eq!(
            keys.iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            keys.len()
        );
        assert!(keys.iter().any(|&key| key < 0));
        assert!(!keys.contains(&a));
    }
    #[test]
    fn spell_runtime_owner_failed_sql_has_no_mp_cooldown_ids_rng_or_output_then_exact_retry() {
        let mut s = setup();
        let (session, a, mut out_a) = enter(&mut s, A);
        let (_, _, mut out_b) = enter(&mut s, B);
        drain(&mut out_a);
        drain(&mut out_b);
        let cursor = spell::aim(&s.worlds[&0], a);
        let before = s.worlds[&0].clone();
        let saved = character(&s.db, A).unwrap().unwrap();
        s.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_spell BEFORE UPDATE ON build100_characters BEGIN SELECT RAISE(FAIL,'spell commit failure'); END;").unwrap();
        assert!(
            s.input(session, spell::cast(a, 1, a, cursor), Instant::now())
                .unwrap_err()
                .is::<StorageFailure>()
        );
        assert_eq!(s.worlds[&0].character(a), &saved);
        assert_eq!(character(&s.db, A).unwrap().unwrap(), saved);
        assert_eq!(spell::snapshot(&s.worlds[&0]), spell::snapshot(&before));
        assert!(drain(&mut out_a).is_empty() && drain(&mut out_b).is_empty());
        let mut expected = before;
        expected
            .handle_at(
                a,
                spell::cast(a, 1, a, cursor),
                &s.content,
                Instant::now(),
                Instant::now(),
            )
            .unwrap();
        s.db.lock()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_spell")
            .unwrap();
        s.input(session, spell::cast(a, 1, a, cursor), Instant::now())
            .unwrap();
        assert_eq!(s.worlds[&0].character(a), expected.character(a));
        assert_eq!(spell::snapshot(&s.worlds[&0]), spell::snapshot(&expected));
        drain(&mut out_a);
        drain(&mut out_b);
        let committed = character(&s.db, A).unwrap().unwrap();
        assert!(
            s.input(session, spell::cast(a, 1, a, cursor), Instant::now())
                .is_err()
        );
        let reserved = fire(&s, a, 97);
        assert!(s.input(session, reserved, Instant::now()).is_err());
        assert_eq!(character(&s.db, A).unwrap().unwrap(), committed);
        assert!(drain(&mut out_a).is_empty() && drain(&mut out_b).is_empty());
    }
    #[test]
    fn spell_runtime_owner_reconnect_keeps_paid_carry_cooldown_no_offline_catchup() {
        let mut s = setup();
        let (session, a, mut out) = enter(&mut s, A);
        let cursor = spell::aim(&s.worlds[&0], a);
        s.input(session, spell::cast(a, 1, a, cursor), Instant::now())
            .unwrap();
        drain(&mut out);
        s.tick(Duration::from_millis(125)).unwrap();
        drain(&mut out);
        let saved = character(&s.db, A).unwrap().unwrap();
        s.detach(session, false).unwrap();
        let (output, mut out) = mpsc::channel(OUTPUT_CAPACITY);
        let new = s
            .join(
                protocol::Hello {
                    guid: A.into(),
                    game_id: 0,
                },
                output,
            )
            .unwrap();
        s.input(
            new,
            packet(25, |w| {
                w.i32(0).utf("Advisory");
            }),
            Instant::now(),
        )
        .unwrap();
        let avatar = s.sessions[&new].avatar.unwrap();
        assert_eq!(character(&s.db, A).unwrap().unwrap(), saved);
        assert_eq!(s.worlds[&0].character(avatar), &saved);
        assert!(spell::snapshot(&s.worlds[&0]).0.is_empty());
        drain(&mut out);
        assert!(
            s.input(new, spell::cast(a, 1, a, cursor), Instant::now())
                .is_err()
        );
        assert_eq!(character(&s.db, A).unwrap().unwrap(), saved);
        assert!(
            s.input(new, spell::cast(avatar, 1, avatar, cursor), Instant::now())
                .is_err()
        );
        assert_eq!(character(&s.db, A).unwrap().unwrap(), saved);
        s.tick(Duration::from_millis(375)).unwrap();
        drain(&mut out);
        let control = character(&s.db, A).unwrap().unwrap();
        s.input(new, spell::cast(avatar, 1, avatar, cursor), Instant::now())
            .unwrap();
        let paid = character(&s.db, A).unwrap().unwrap();
        assert_eq!(paid.stats[&4], control.stats[&4] - 40);
        assert_eq!(paid.resources_carry, control.resources_carry);
        assert_eq!(paid.generation, saved.generation);
    }
    #[test]
    fn spell_runtime_owner_namespace_capacity_failure_keeps_funding_rng_and_publication() {
        let mut s = setup();
        crate::world::shore_runtime_tests::isolate_population(s.worlds.get_mut(&0).unwrap());
        let (session, avatar, mut output) = enter(&mut s, A);
        let before = s.worlds[&0].clone();
        s.worlds
            .get_mut(&0)
            .unwrap()
            .character_mut(avatar)
            .stats
            .extend([(3, 1000), (4, 1000)]);
        s.commit(&s.worlds[&0], &before).unwrap();
        s.content.objects.get_mut(&0xa2e).unwrap().projectiles[0]
            .1
            .lifetime = 10.;
        let cursor = spell::aim(&s.worlds[&0], avatar);
        drain(&mut output);
        for _ in 0..7 {
            s.input(
                session,
                spell::cast(avatar, 1, avatar, cursor),
                Instant::now(),
            )
            .unwrap();
            drain(&mut output);
            s.tick(Duration::from_millis(500)).unwrap();
            drain(&mut output);
        }
        let saved = character(&s.db, A).unwrap().unwrap();
        let state = spell::snapshot(&s.worlds[&0]);
        assert_eq!(state.0.len(), 140);
        assert!(
            s.input(
                session,
                spell::cast(avatar, 1, avatar, cursor),
                Instant::now()
            )
            .unwrap_err()
            .to_string()
            .contains("capacity")
        );
        assert_eq!(character(&s.db, A).unwrap().unwrap(), saved);
        assert_eq!(s.worlds[&0].character(avatar), &saved);
        assert_eq!(spell::snapshot(&s.worlds[&0]), state);
        assert!(drain(&mut output).is_empty());
    }
}

#[cfg(test)]
mod projectile_id_reuse_owner_tests {
    use super::*;
    use crate::{database, wire::Reader, world::spell_runtime_tests as spell};
    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    fn setup() -> Simulation {
        let (content, _) = spell::setup(true);
        let path = std::env::var_os("BUILD100_CONTENT_REPAIRED_DIR").unwrap();
        let grounds = Arc::new(Grounds::load(std::path::Path::new(&path)).unwrap());
        Simulation::normal(
            content,
            grounds,
            Arc::new(Mutex::new(
                database(std::path::Path::new(":memory:")).unwrap(),
            )),
            100,
        )
        .unwrap()
    }
    fn enter(
        s: &mut Simulation,
        guid: &str,
        game_id: i32,
    ) -> (u64, i32, mpsc::Receiver<Vec<Packet>>) {
        let (output, receive) = mpsc::channel(OUTPUT_CAPACITY);
        let session = s
            .join(
                protocol::Hello {
                    guid: guid.into(),
                    game_id,
                },
                output,
            )
            .unwrap();
        let mut w = Writer::new();
        w.i16(0x30e).utf("Rotate");
        s.input(session, Packet { id: 3, body: w.0 }, Instant::now())
            .unwrap();
        (session, s.sessions[&session].avatar.unwrap(), receive)
    }
    fn drain(out: &mut mpsc::Receiver<Vec<Packet>>) -> Vec<Packet> {
        let mut packets = vec![];
        while let Ok(batch) = out.try_recv() {
            packets.extend(batch);
        }
        packets
    }
    fn ids(events: &[Packet], owner: i32) -> Vec<u8> {
        events
            .iter()
            .filter(|p| p.id == 8)
            .filter_map(|p| {
                let mut r = Reader::new(&p.body);
                let id = r.u8().unwrap();
                (r.i32().unwrap() == owner).then_some(id)
            })
            .collect()
    }
    #[test]
    fn id_reuse_fix_owner_second_cast_after_expiry_publishes_rotated_ids() {
        let mut s = setup();
        crate::world::shore_runtime_tests::isolate_population(s.worlds.get_mut(&0).unwrap());
        let (session, avatar, mut output) = enter(&mut s, A, 0);
        let b = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let (peer_session, peer, mut peer_output) = enter(&mut s, b, 0);
        drain(&mut output);
        drain(&mut peer_output);
        let cursor = spell::aim(&s.worlds[&0], avatar);
        s.input(
            session,
            spell::cast(avatar, 1, avatar, cursor),
            Instant::now(),
        )
        .unwrap();
        for events in [drain(&mut output), drain(&mut peer_output)] {
            assert_eq!(ids(&events, avatar), (97..117).collect::<Vec<_>>());
        }
        s.tick(Duration::from_secs(1)).unwrap();
        drain(&mut output);
        drain(&mut peer_output);
        assert!(spell::snapshot(&s.worlds[&0]).0.is_empty());
        assert_eq!(
            spell::server_cursors(&s.worlds[&0]),
            BTreeMap::from([(avatar, 117)])
        );
        let before = s.worlds[&0].clone();
        let saved = character(&s.db, A).unwrap().unwrap();
        s.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_id_cursor BEFORE UPDATE ON build100_characters BEGIN SELECT RAISE(FAIL,'cursor commit failure'); END;").unwrap();
        assert!(
            s.input(
                session,
                spell::cast(avatar, 1, avatar, cursor),
                Instant::now()
            )
            .unwrap_err()
            .is::<StorageFailure>()
        );
        assert_eq!(character(&s.db, A).unwrap().unwrap(), saved);
        assert_eq!(s.worlds[&0].character(avatar), &saved);
        assert_eq!(
            spell::server_cursors(&s.worlds[&0]),
            spell::server_cursors(&before)
        );
        assert_eq!(spell::snapshot(&s.worlds[&0]), spell::snapshot(&before));
        assert!(drain(&mut output).is_empty() && drain(&mut peer_output).is_empty());
        let mut expected = before;
        expected
            .handle_at(
                avatar,
                spell::cast(avatar, 1, avatar, cursor),
                &s.content,
                Instant::now(),
                Instant::now(),
            )
            .unwrap();
        s.db.lock()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_id_cursor")
            .unwrap();
        s.input(
            session,
            spell::cast(avatar, 1, avatar, cursor),
            Instant::now(),
        )
        .unwrap();
        assert_eq!(s.worlds[&0].character(avatar), expected.character(avatar));
        assert_eq!(spell::snapshot(&s.worlds[&0]), spell::snapshot(&expected));
        assert_eq!(
            spell::server_cursors(&s.worlds[&0]),
            spell::server_cursors(&expected)
        );
        for events in [drain(&mut output), drain(&mut peer_output)] {
            assert_eq!(ids(&events, avatar), (117..137).collect::<Vec<_>>());
        }
        let peer_cursor = spell::aim(&s.worlds[&0], peer);
        s.input(
            peer_session,
            spell::cast(peer, 1, peer, peer_cursor),
            Instant::now(),
        )
        .unwrap();
        for events in [drain(&mut output), drain(&mut peer_output)] {
            assert_eq!(ids(&events, peer), (97..117).collect::<Vec<_>>());
        }
        assert_eq!(
            spell::server_cursors(&s.worlds[&0]),
            BTreeMap::from([(avatar, 137), (peer, 117)])
        );
        s.tick(Duration::from_secs(1)).unwrap();
        drain(&mut output);
        drain(&mut peer_output);
        s.detach(session, false).unwrap();
        drain(&mut peer_output);
        assert_eq!(
            spell::server_cursors(&s.worlds[&0]),
            BTreeMap::from([(peer, 117)])
        );
        let (send, mut receive) = mpsc::channel(OUTPUT_CAPACITY);
        let fresh = s
            .join(
                protocol::Hello {
                    guid: A.into(),
                    game_id: 0,
                },
                send,
            )
            .unwrap();
        let mut w = Writer::new();
        w.i32(0).utf("Advisory");
        s.input(fresh, Packet { id: 25, body: w.0 }, Instant::now())
            .unwrap();
        let new_avatar = s.sessions[&fresh].avatar.unwrap();
        assert_ne!(new_avatar, avatar);
        assert_eq!(
            character(&s.db, A).unwrap().unwrap().generation,
            saved.generation
        );
        drain(&mut receive);
        drain(&mut peer_output);
        s.tick(Duration::from_secs(13)).unwrap();
        drain(&mut receive);
        drain(&mut peer_output);
        let regenerated = character(&s.db, A).unwrap().unwrap();
        s.input(
            fresh,
            spell::cast(new_avatar, 1, new_avatar, cursor),
            Instant::now(),
        )
        .unwrap();
        assert_eq!(
            ids(&drain(&mut receive), new_avatar),
            (97..117).collect::<Vec<_>>()
        );
        let paid = character(&s.db, A).unwrap().unwrap();
        assert_eq!(paid.stats[&4], regenerated.stats[&4] - 40);
        assert_eq!(paid.resources_carry, regenerated.resources_carry);
        assert!(!spell::server_cursors(&s.worlds[&0]).contains_key(&avatar));
    }

    #[test]
    fn id_reuse_fix_owner_death_replacement_and_retired_worlds_drop_cursor_history() {
        let mut s = setup();
        let (session, avatar, mut output) = enter(&mut s, A, 0);
        drain(&mut output);
        let cursor = spell::aim(&s.worlds[&0], avatar);
        s.input(
            session,
            spell::cast(avatar, 1, avatar, cursor),
            Instant::now(),
        )
        .unwrap();
        drain(&mut output);
        crate::world::progression_runtime_tests::lethal_enemy_bullet(
            s.worlds.get_mut(&0).unwrap(),
            avatar,
        );
        s.tick(Duration::ZERO).unwrap();
        drain(&mut output);
        assert!(!character(&s.db, A).unwrap().unwrap().alive);
        assert!(spell::server_cursors(&s.worlds[&0]).is_empty());
        s.detach(session, false).unwrap();
        let (session, replacement, mut output) = enter(&mut s, A, 0);
        drain(&mut output);
        assert_ne!(replacement, avatar);
        assert_eq!(character(&s.db, A).unwrap().unwrap().generation, 2);
        s.input(
            session,
            spell::cast(replacement, 1, replacement, cursor),
            Instant::now(),
        )
        .unwrap();
        assert_eq!(
            ids(&drain(&mut output), replacement),
            (97..117).collect::<Vec<_>>()
        );
        let mut s = setup();
        let (session, _, mut output) = enter(&mut s, A, -1);
        let old_world = s.sessions[&session].world;
        drain(&mut output);
        for _ in 0..2 {
            s.tick(Duration::from_secs(3)).unwrap();
            drain(&mut output);
        }
        let cursors = spell::server_cursors(&s.worlds[&old_world]);
        assert_eq!(cursors.len(), 4);
        assert!(cursors.values().all(|&next| next == 3));
        s.detach(session, false).unwrap();
        assert!(!s.worlds.contains_key(&old_world));
        let (session, _, mut output) =
            enter(&mut s, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", -1);
        let fresh_world = s.sessions[&session].world;
        assert_ne!(fresh_world, old_world);
        assert!(spell::server_cursors(&s.worlds[&fresh_world]).is_empty());
        drain(&mut output);
        s.tick(Duration::from_secs(3)).unwrap();
        let events = drain(&mut output);
        assert!(events.iter().any(|p| p.id == 8));
        let cursors = spell::server_cursors(&s.worlds[&fresh_world]);
        assert_eq!(cursors.len(), 4);
        for (owner, next) in cursors {
            assert_eq!(next, 2);
            // Interest may omit an unseen owner; every published first gun shot still starts at one.
            assert!(ids(&events, owner).iter().all(|&id| id == 1));
        }
    }
    #[test]
    fn id_reuse_fix_owner_capacity_failure_keeps_cursor_until_real_expiry_and_retry() {
        let mut s = setup();
        crate::world::shore_runtime_tests::isolate_population(s.worlds.get_mut(&0).unwrap());
        let (session, avatar, mut output) = enter(&mut s, A, 0);
        let before = s.worlds[&0].clone();
        s.worlds
            .get_mut(&0)
            .unwrap()
            .character_mut(avatar)
            .stats
            .extend([(3, 1000), (4, 1000)]);
        s.commit(&s.worlds[&0], &before).unwrap();
        s.content.objects.get_mut(&0xa2e).unwrap().projectiles[0]
            .1
            .lifetime = 10.;
        let cursor = spell::aim(&s.worlds[&0], avatar);
        drain(&mut output);
        for _ in 0..7 {
            s.input(
                session,
                spell::cast(avatar, 1, avatar, cursor),
                Instant::now(),
            )
            .unwrap();
            drain(&mut output);
            s.tick(Duration::from_millis(500)).unwrap();
            drain(&mut output);
        }
        let saved = character(&s.db, A).unwrap().unwrap();
        let snapshot = spell::snapshot(&s.worlds[&0]);
        assert_eq!(snapshot.0.len(), 140);
        assert_eq!(
            spell::server_cursors(&s.worlds[&0]),
            BTreeMap::from([(avatar, 237)])
        );
        assert!(
            s.input(
                session,
                spell::cast(avatar, 1, avatar, cursor),
                Instant::now()
            )
            .unwrap_err()
            .to_string()
            .contains("capacity")
        );
        assert_eq!(
            spell::server_cursors(&s.worlds[&0]),
            BTreeMap::from([(avatar, 237)])
        );
        assert_eq!(spell::snapshot(&s.worlds[&0]), snapshot);
        assert_eq!(s.worlds[&0].character(avatar), &saved);
        assert_eq!(character(&s.db, A).unwrap().unwrap(), saved);
        assert!(drain(&mut output).is_empty());
        s.tick(Duration::from_secs(10)).unwrap();
        drain(&mut output);
        assert!(spell::snapshot(&s.worlds[&0]).0.is_empty());
        assert_eq!(
            spell::server_cursors(&s.worlds[&0]),
            BTreeMap::from([(avatar, 237)])
        );
        let control = character(&s.db, A).unwrap().unwrap();
        s.input(
            session,
            spell::cast(avatar, 1, avatar, cursor),
            Instant::now(),
        )
        .unwrap();
        assert_eq!(
            ids(&drain(&mut output), avatar),
            [(237..=255).collect::<Vec<_>>(), vec![97]].concat()
        );
        assert_eq!(
            spell::server_cursors(&s.worlds[&0]),
            BTreeMap::from([(avatar, 98)])
        );
        assert_eq!(
            character(&s.db, A).unwrap().unwrap().stats[&4],
            control.stats[&4] - 40
        );
    }
    #[test]
    fn id_reuse_fix_owner_npc_cursors_and_rng_roll_back_on_sql_begin_failure() {
        let mut s = setup();
        let (session, _, mut output) = enter(&mut s, A, -1);
        let world = s.sessions[&session].world;
        drain(&mut output);
        let before = s.worlds[&world].clone();
        s.db.lock().unwrap().execute_batch("BEGIN;").unwrap();
        assert!(
            s.tick(Duration::from_secs(3))
                .unwrap_err()
                .is::<StorageFailure>()
        );
        assert_eq!(
            spell::server_cursors(&s.worlds[&world]),
            spell::server_cursors(&before)
        );
        assert_eq!(spell::snapshot(&s.worlds[&world]), spell::snapshot(&before));
        assert!(drain(&mut output).is_empty());
        s.db.lock().unwrap().execute_batch("ROLLBACK;").unwrap();
        let mut expected = before;
        expected.advance(3., &s.content).unwrap();
        s.tick(Duration::from_secs(3)).unwrap();
        assert_eq!(
            spell::server_cursors(&s.worlds[&world]),
            spell::server_cursors(&expected)
        );
        assert_eq!(
            spell::snapshot(&s.worlds[&world]),
            spell::snapshot(&expected)
        );
        let cursors = spell::server_cursors(&s.worlds[&world]);
        assert_eq!(cursors.len(), 4);
        assert!(cursors.values().all(|&next| next == 2));
        let events = drain(&mut output);
        assert!(events.iter().any(|p| p.id == 8));
        for owner in cursors.keys() {
            assert!(ids(&events, *owner).iter().all(|&id| id == 1));
        }
    }
}

#[cfg(test)]
mod shore_owner_tests {
    use super::*;
    use crate::{
        database,
        wire::Reader,
        world::{
            progression_runtime_tests as loot, shore_runtime_tests as shore,
            spell_runtime_tests as spell,
        },
    };
    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    fn setup() -> Simulation {
        let (c, g) = shore::load(true);
        Simulation::normal(
            c,
            g,
            Arc::new(Mutex::new(
                database(std::path::Path::new(":memory:")).unwrap(),
            )),
            100,
        )
        .unwrap()
    }
    fn packet(id: u8, f: impl FnOnce(&mut Writer)) -> Packet {
        let mut w = Writer::new();
        f(&mut w);
        Packet { id, body: w.0 }
    }
    fn enter(s: &mut Simulation, guid: &str) -> (u64, i32, mpsc::Receiver<Vec<Packet>>) {
        let (output, receive) = mpsc::channel(OUTPUT_CAPACITY);
        let session = s
            .join(
                protocol::Hello {
                    guid: guid.into(),
                    game_id: 0,
                },
                output,
            )
            .unwrap();
        s.input(
            session,
            packet(3, |w| {
                w.i16(0x30e).utf("EarnShore");
            }),
            Instant::now(),
        )
        .unwrap();
        (session, s.sessions[&session].avatar.unwrap(), receive)
    }
    fn drain(out: &mut mpsc::Receiver<Vec<Packet>>) -> Vec<Packet> {
        let mut all = vec![];
        while let Ok(batch) = out.try_recv() {
            all.extend(batch);
        }
        all
    }
    fn keepalive(
        s: &mut Simulation,
        session: u64,
        output: &mut mpsc::Receiver<Vec<Packet>>,
    ) -> Vec<Packet> {
        let events = drain(output);
        for p in events.iter().filter(|p| p.id == 22) {
            let serial = crate::protocol::pong(&p.body).unwrap();
            s.input(
                session,
                packet(23, |w| {
                    w.i32(serial);
                }),
                Instant::now(),
            )
            .unwrap();
        }
        let avatar = s.sessions[&session].avatar.unwrap();
        let p = s.worlds[&0].character(avatar);
        let (x, y) = (p.x, p.y);
        s.input(
            session,
            packet(5, |w| {
                w.f32(x).f32(y);
            }),
            Instant::now(),
        )
        .unwrap();
        events
    }
    fn xp(events: &[Packet], avatar: i32) -> Option<(i32, i32, i32)> {
        let mut result = None;
        for p in events.iter().filter(|p| p.id == 13) {
            let mut r = Reader::new(&p.body);
            for _ in 0..r.i16().unwrap() {
                let id = r.i32().unwrap();
                r.f32().unwrap();
                r.f32().unwrap();
                let stats: BTreeMap<_, _> = (0..r.i16().unwrap())
                    .map(|_| (r.u8().unwrap(), r.i32().unwrap()))
                    .collect();
                if id == avatar {
                    result = Some((stats[&5], stats[&6], stats[&7]));
                }
            }
        }
        result
    }

    #[test]
    fn shore_runtime_owner_death_replenishment_group_rewards_sql_rollback_and_exact_retry() {
        let mut s = setup();
        let (a, avatar, mut out) = enter(&mut s, A);
        let (b, peer, mut peer_out) = enter(&mut s, B);
        keepalive(&mut s, a, &mut out);
        keepalive(&mut s, b, &mut peer_out);
        assert_eq!(shore::actors(&s.worlds[&0]).len(), 4);
        s.tick(Duration::from_millis(9900)).unwrap();
        keepalive(&mut s, a, &mut out);
        keepalive(&mut s, b, &mut peer_out);
        let (enemy, cursor) = shore::actors(&s.worlds[&0])[0];
        s.input(a, spell::cast(avatar, 1, avatar, cursor), Instant::now())
            .unwrap();
        keepalive(&mut s, a, &mut out);
        keepalive(&mut s, b, &mut peer_out);
        let before = s.worlds[&0].clone();
        let saved_a = character(&s.db, A).unwrap().unwrap();
        let saved_b = character(&s.db, B).unwrap().unwrap();
        let mut expected = before.clone();
        expected.advance(0.1, &s.content).unwrap();
        assert!(!shore::actors(&expected).iter().any(|(id, _)| *id == enemy));
        assert_eq!(shore::actors(&expected).len(), 4);
        assert!(expected.character(avatar).stats[&6] > 0);
        s.db.lock().unwrap().execute_batch(&format!("CREATE TRIGGER fail_shore BEFORE UPDATE ON build100_characters WHEN NEW.guid='{B}' BEGIN SELECT RAISE(FAIL,'shore reward commit failure'); END;")).unwrap();
        assert!(
            s.tick(Duration::from_millis(100))
                .unwrap_err()
                .is::<StorageFailure>()
        );
        assert_eq!(shore::snapshot(&s.worlds[&0]), shore::snapshot(&before));
        assert_eq!(character(&s.db, A).unwrap().unwrap(), saved_a);
        assert_eq!(character(&s.db, B).unwrap().unwrap(), saved_b);
        assert!(drain(&mut out).is_empty() && drain(&mut peer_out).is_empty());
        s.db.lock()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_shore")
            .unwrap();
        s.tick(Duration::from_millis(100)).unwrap();
        assert_eq!(shore::snapshot(&s.worlds[&0]), shore::snapshot(&expected));
        let original_ids: Vec<_> = shore::actors(&before)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let born: Vec<_> = shore::actors(&s.worlds[&0])
            .into_iter()
            .map(|(id, _)| id)
            .filter(|id| !original_ids.contains(id))
            .collect();
        assert!(!born.is_empty());
        assert!(
            !spell::snapshot(&s.worlds[&0])
                .0
                .iter()
                .any(|(owner, _, _)| born.contains(owner))
        );
        for (id, guid) in [(avatar, A), (peer, B)] {
            assert_eq!(
                &character(&s.db, guid).unwrap().unwrap(),
                s.worlds[&0].character(id)
            );
        }
        for events in [drain(&mut out), drain(&mut peer_out)] {
            for id in &born {
                let made = events
                    .iter()
                    .filter(|p| p.id == 10)
                    .filter(|p| {
                        let mut read = Reader::new(&p.body);
                        read.i16().unwrap();
                        read.utf().unwrap();
                        read.i32().unwrap() == *id
                    })
                    .count();
                assert_eq!(
                    made, 1,
                    "newly replenished owner must be published before any later AI action"
                );
            }
            assert_eq!(
                xp(&events, avatar),
                Some((50, expected.character(avatar).stats[&6], 1))
            );
            assert_eq!(
                xp(&events, peer),
                Some((50, expected.character(peer).stats[&6], 1))
            );
        }
        let earned = character(&s.db, A).unwrap().unwrap().stats[&6];
        s.tick(Duration::ZERO).unwrap();
        assert_eq!(character(&s.db, A).unwrap().unwrap().stats[&6], earned);
    }

    #[test]
    fn shore_runtime_owner_earns_level_and_stochastic_loot_pickup_gear_reconnect_without_forcing_rng()
     {
        let mut s = setup();
        let (a, avatar, mut out) = enter(&mut s, A);
        let (b, peer, mut peer_out) = enter(&mut s, B);
        keepalive(&mut s, a, &mut out);
        keepalive(&mut s, b, &mut peer_out);
        let mut kills = 0;
        let mut growth_rollback = false;
        let mut contested = false;
        let mut equipped = None;
        for _ in 0..200 {
            s.tick(Duration::from_millis(500)).unwrap();
            keepalive(&mut s, a, &mut out);
            keepalive(&mut s, b, &mut peer_out);
            for _ in 0..4 {
                if s.worlds[&0].character(avatar).stats[&4] >= 40
                    && !shore::actors(&s.worlds[&0]).is_empty()
                {
                    break;
                }
                s.tick(Duration::from_secs(10)).unwrap();
                keepalive(&mut s, a, &mut out);
                keepalive(&mut s, b, &mut peer_out);
            }
            for _ in 0..120 {
                let p = s.worlds[&0].character(avatar);
                if p.stats[&1] < p.stats[&0] {
                    break;
                }
                s.tick(Duration::from_millis(50)).unwrap();
                keepalive(&mut s, a, &mut out);
                keepalive(&mut s, b, &mut peer_out);
            }
            assert!(s.worlds[&0].character(avatar).alive && s.worlds[&0].character(peer).alive);
            let p = s.worlds[&0].character(avatar).clone();
            assert!(p.stats[&4] >= 40);
            let live = shore::actors(&s.worlds[&0]);
            let (_, cursor) = *live
                .iter()
                .min_by(|a, b| {
                    ((a.1.0 - p.x).hypot(a.1.1 - p.y))
                        .total_cmp(&((b.1.0 - p.x).hypot(b.1.1 - p.y)))
                })
                .expect("ordinary shore encounter");
            s.input(a, spell::cast(avatar, 1, avatar, cursor), Instant::now())
                .unwrap();
            keepalive(&mut s, a, &mut out);
            keepalive(&mut s, b, &mut peer_out);
            let before = s.worlds[&0].character(avatar).clone();
            let mut expected = s.worlds[&0].clone();
            expected.advance(0., &s.content).unwrap();
            if !growth_rollback && expected.character(avatar).stats[&7] > before.stats[&7] {
                let state = shore::snapshot(&s.worlds[&0]);
                let saved_a = character(&s.db, A).unwrap().unwrap();
                let saved_b = character(&s.db, B).unwrap().unwrap();
                s.db.lock().unwrap().execute_batch(&format!("CREATE TRIGGER fail_growth BEFORE UPDATE ON build100_characters WHEN NEW.guid='{B}' BEGIN SELECT RAISE(FAIL,'earned shore growth failure'); END;")).unwrap();
                assert!(s.tick(Duration::ZERO).unwrap_err().is::<StorageFailure>());
                assert_eq!(shore::snapshot(&s.worlds[&0]), state);
                assert_eq!(character(&s.db, A).unwrap().unwrap(), saved_a);
                assert_eq!(character(&s.db, B).unwrap().unwrap(), saved_b);
                assert!(drain(&mut out).is_empty() && drain(&mut peer_out).is_empty());
                s.db.lock()
                    .unwrap()
                    .execute_batch("DROP TRIGGER fail_growth")
                    .unwrap();
                growth_rollback = true;
            }
            s.tick(Duration::ZERO).unwrap();
            assert_eq!(shore::snapshot(&s.worlds[&0]), shore::snapshot(&expected));
            let survivors = shore::actors(&s.worlds[&0]);
            let dead = live
                .iter()
                .filter(|(id, _)| !survivors.iter().any(|(other, _)| other == id))
                .count() as i32;
            assert!(dead > 0);
            kills += dead;
            let after = character(&s.db, A).unwrap().unwrap();
            let mut expected_xp = before.stats[&6] + dead;
            let mut level = before.stats[&7];
            let mut goal = before.stats[&5];
            while level < 20 && expected_xp >= goal {
                expected_xp -= goal;
                level += 1;
                goal = 50 + 100 * (level - 1);
            }
            for (guid, id) in [(A, avatar), (B, peer)] {
                let saved = character(&s.db, guid).unwrap().unwrap();
                assert_eq!(
                    (saved.stats[&5], saved.stats[&6], saved.stats[&7]),
                    (goal, expected_xp, level)
                );
                assert_eq!(&saved, s.worlds[&0].character(id));
            }
            assert_eq!(
                after.stats[&1] - before.stats[&1],
                after.stats[&0] - before.stats[&0]
            );
            assert_eq!(
                after.stats[&4] - before.stats[&4],
                after.stats[&3] - before.stats[&3]
            );
            assert_eq!(after.resources_carry, before.resources_carry);
            for events in [
                keepalive(&mut s, a, &mut out),
                keepalive(&mut s, b, &mut peer_out),
            ] {
                assert_eq!(xp(&events, avatar), Some((goal, expected_xp, level)));
                assert_eq!(xp(&events, peer), Some((goal, expected_xp, level)));
            }
            let nearby = loot::bags(&s.worlds[&0]);
            for (bag, items) in nearby {
                for (slot, item) in items
                    .into_iter()
                    .enumerate()
                    .filter(|(_, item)| *item != -1)
                {
                    let kind = i16::try_from(item).unwrap();
                    let gear = s.content.get(kind).unwrap().slot_type == Some(17);
                    if contested && (!gear || equipped.is_some()) {
                        continue;
                    }
                    let backpack = (4..12)
                        .find(|slot| s.worlds[&0].character(avatar).stats[&(8 + slot)] == -1)
                        .unwrap();
                    let count = |s: &Simulation| {
                        [avatar, peer]
                            .into_iter()
                            .map(|id| {
                                (8..20)
                                    .filter(|slot| s.worlds[&0].character(id).stats[slot] == item)
                                    .count()
                            })
                            .sum::<usize>()
                            + loot::bags(&s.worlds[&0])
                                .iter()
                                .map(|(_, items)| {
                                    items.iter().filter(|value| **value == item).count()
                                })
                                .sum::<usize>()
                    };
                    let conserved = count(&s);
                    let pickup = |owner| {
                        packet(14, |w| {
                            w.i32(bag)
                                .u8(slot as u8)
                                .i16(kind)
                                .i32(owner)
                                .u8(backpack)
                                .i16(-1);
                        })
                    };
                    // Range remains actual bag authority; a remote drop is not teleported into reach.
                    let result = s.input(a, pickup(avatar), Instant::now());
                    if let Err(error) = result {
                        assert!(error.to_string().contains("too far"));
                        continue;
                    }
                    assert!(s.input(b, pickup(peer), Instant::now()).is_err());
                    contested = true;
                    assert_eq!(
                        character(&s.db, A).unwrap().unwrap().stats[&(8 + backpack)],
                        item
                    );
                    assert_eq!(
                        character(&s.db, B).unwrap().unwrap().stats[&(8 + backpack)],
                        -1
                    );
                    assert_eq!(count(&s), conserved);
                    if gear && equipped.is_none() {
                        let old = i16::try_from(s.worlds[&0].character(avatar).stats[&8]).unwrap();
                        s.input(
                            a,
                            packet(14, |w| {
                                w.i32(avatar)
                                    .u8(backpack)
                                    .i16(kind)
                                    .i32(avatar)
                                    .u8(0)
                                    .i16(old);
                            }),
                            Instant::now(),
                        )
                        .unwrap();
                        equipped = Some(item);
                    }
                    assert_eq!(count(&s), conserved);
                    keepalive(&mut s, a, &mut out);
                    keepalive(&mut s, b, &mut peer_out);
                }
            }
            if kills >= 50 && contested && equipped.is_some() {
                break;
            }
        }
        assert!(
            kills >= 50 && growth_rollback && contested && equipped.is_some(),
            "kills={kills} growth_rollback={growth_rollback} contested={contested} equipped={equipped:?}"
        );
        let saved = character(&s.db, A).unwrap().unwrap();
        assert!(saved.stats[&7] >= 2);
        assert_eq!(saved.stats[&8], equipped.unwrap());
        let effective = shore::effective_stats(&s.worlds[&0], &s.content, avatar);
        let kind = i16::try_from(equipped.unwrap()).unwrap();
        let item = s.content.get(kind).unwrap();
        let (number, gap) = (item.num_projectiles, item.arc_gap_radians);
        let visual = s
            .content
            .named(&item.normal_projectile(0).unwrap().visual)
            .unwrap()
            .id as u8;
        keepalive(&mut s, a, &mut out);
        keepalive(&mut s, b, &mut peer_out);
        for index in 0..number {
            s.input(
                a,
                packet(8, |w| {
                    w.u8(index + 1)
                        .i32(avatar)
                        .u8(visual)
                        .i16(kind)
                        .f32(saved.x + 0.3)
                        .f32(saved.y)
                        .f32(f32::from(index) * gap)
                        .i16(0);
                }),
                Instant::now(),
            )
            .unwrap();
        }
        assert_eq!(
            spell::snapshot(&s.worlds[&0])
                .0
                .iter()
                .filter(|(owner, id, damage)| *owner == avatar && *id <= number && *damage > 0)
                .count(),
            usize::from(number)
        );
        let peer_shots: Vec<_> = drain(&mut peer_out)
            .into_iter()
            .filter(|p| p.id == 8)
            .map(|p| {
                let mut read = Reader::new(&p.body);
                (
                    read.u8().unwrap(),
                    read.i32().unwrap(),
                    read.u8().unwrap(),
                    read.i16().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            peer_shots,
            (1..=number)
                .map(|id| (id, avatar, 0, kind))
                .collect::<Vec<_>>()
        );
        assert_eq!(character(&s.db, A).unwrap().unwrap(), saved);
        s.detach(a, false).unwrap();
        keepalive(&mut s, b, &mut peer_out);
        let (output, mut reconnected) = mpsc::channel(OUTPUT_CAPACITY);
        let session = s
            .join(
                protocol::Hello {
                    guid: A.into(),
                    game_id: 0,
                },
                output,
            )
            .unwrap();
        s.input(
            session,
            packet(25, |w| {
                w.i32(0).utf("Advisory");
            }),
            Instant::now(),
        )
        .unwrap();
        assert_eq!(character(&s.db, A).unwrap().unwrap(), saved);
        assert_eq!(
            s.worlds[&0].character(s.sessions[&session].avatar.unwrap()),
            &saved
        );
        assert_eq!(
            shore::effective_stats(
                &s.worlds[&0],
                &s.content,
                s.sessions[&session].avatar.unwrap()
            ),
            effective
        );
        assert!(!drain(&mut reconnected).is_empty());
        eprintln!(
            "shore earned source proof: kills={kills}, Wizard weapon={:#x}, level={}",
            equipped.unwrap(),
            saved.stats[&7]
        );
    }

    #[test]
    fn shore_runtime_owner_real_pirate_damage_permadeath_replacement_and_instance_isolation() {
        let mut s = setup();
        let (a, avatar, mut output) = enter(&mut s, A);
        keepalive(&mut s, a, &mut output);
        let mut damage_packets = 0;
        for _ in 0..1200 {
            if !s.worlds[&0].character(avatar).alive {
                break;
            }
            s.tick(Duration::from_millis(50)).unwrap();
            let events = keepalive(&mut s, a, &mut output);
            damage_packets += events.iter().filter(|p| p.id == 9).count();
        }
        let dead = character(&s.db, A).unwrap().unwrap();
        assert!(!dead.alive && damage_packets > 0);
        assert_eq!(dead.stats[&1], 0);
        assert_eq!((dead.stats[&6], dead.stats[&7]), (0, 1));
        assert_eq!(&dead, s.worlds[&0].character(avatar));
        s.detach(a, false).unwrap();
        let (_, replacement, mut output) = enter(&mut s, A);
        drain(&mut output);
        let fresh = character(&s.db, A).unwrap().unwrap();
        assert!(fresh.alive);
        assert_eq!(fresh.generation, dead.generation + 1);
        assert_ne!(replacement, avatar);
        assert_eq!(
            (fresh.stats[&5], fresh.stats[&6], fresh.stats[&7]),
            (50, 0, 1)
        );
        let mut s = setup();
        let (output, mut receive) = mpsc::channel(OUTPUT_CAPACITY);
        let private = s
            .join(
                protocol::Hello {
                    guid: B.into(),
                    game_id: -1,
                },
                output,
            )
            .unwrap();
        s.input(
            private,
            packet(3, |w| {
                w.i16(0x30e).utf("Private");
            }),
            Instant::now(),
        )
        .unwrap();
        assert_ne!(s.sessions[&private].world, 0);
        assert!(s.worlds[&0].players.is_empty());
        let before = shore::actors(&s.worlds[&0]);
        let random_before = spell::snapshot(&s.worlds[&0]);
        drain(&mut receive);
        s.tick(Duration::from_millis(500)).unwrap();
        assert_eq!(shore::actors(&s.worlds[&0]), before);
        assert_eq!(spell::snapshot(&s.worlds[&0]), random_before);
        assert!(spell::server_cursors(&s.worlds[&0]).is_empty());
    }
}
