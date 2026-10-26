use super::{Controller, Directory, Job, Target, World};
use anyhow::{Context, Result, bail, ensure};
use std::{
    cell::OnceCell,
    collections::BTreeMap,
    ops::{Index, IndexMut},
    sync::{Arc, Condvar, Mutex, RwLock},
    time::Instant,
};
use tokio::sync::{mpsc, watch};
mod actor;
mod pool;
#[cfg(test)]
mod tests;
pub(super) use pool::Pool;

#[derive(Clone)]
pub(super) struct Resident {
    pub account: i32,
    pub character: i32,
    pub controller: Controller,
}
type PortalDisplay = (String, Vec<(u8, i32)>, bool);
#[derive(Clone)]
pub(super) struct Snapshot {
    pub players: BTreeMap<i32, Resident>,
    pub admissions_empty: bool,
    pub entities: usize,
    pub actors: usize,
    pub bullets: usize,
    pub now: u64,
    pub tick: u64,
    pub occupancy: usize,
    pub max_players: usize,
    pub admission_open: bool,
    pub bags_empty: bool,
    pub critical: bool,
    pub portals_pending: bool,
    pub chamber_at: Option<u64>,
    pub portal_displays: BTreeMap<i32, PortalDisplay>,
    pub published: Instant,
}
impl Snapshot {
    fn capture(world: &World) -> Self {
        Self {
            players: world
                .players
                .iter()
                .map(|(id, p)| {
                    (
                        *id,
                        Resident {
                            account: p.character.account,
                            character: p.character.id,
                            controller: p.controller,
                        },
                    )
                })
                .collect(),
            admissions_empty: world.admissions.is_empty(),
            entities: world.entities.len(),
            actors: world.actor_ids.len(),
            bullets: world.bullets.len(),
            now: world.now,
            tick: world.tick,
            occupancy: world.occupancy(),
            max_players: world.max_players,
            admission_open: world.admission_open,
            bags_empty: world.bag_expiry.is_empty(),
            critical: !world.critical_deaths.is_empty(),
            portals_pending: !world.portal_requests.is_empty(),
            chamber_at: if world.name == "Oryx's Castle" && world.cleared {
                world
                    .actors()
                    .filter(|e| e.kind == 0xd7a)
                    .map(|e| e.next_aux)
                    .min()
            } else {
                None
            },
            portal_displays: if world.name == "Nexus" {
                world
                    .actors()
                    .filter(|e| e.kind == 0x712)
                    .map(|e| (e.id, (e.name.clone(), e.extra_stats.clone(), e.published)))
                    .collect()
            } else {
                BTreeMap::new()
            },
            published: Instant::now(),
        }
    }
    pub(super) fn needs_coordinator(&self) -> bool {
        self.critical || self.portals_pending
    }
}
#[derive(Default)]
pub(super) struct Routing {
    pub(super) tick: std::sync::atomic::AtomicU64,
    worlds: RwLock<BTreeMap<i32, Arc<Slot>>>,
    publication: RwLock<()>,
    players: Arc<RwLock<BTreeMap<i32, i32>>>,
}
impl Routing {
    pub(super) fn resolve(&self, target: Target) -> Result<Arc<Slot>> {
        let _publication = self.publication.read().expect("world publication poisoned");
        let id = match target {
            Target::World(id) => id,
            Target::Player(id) => *self
                .players
                .read()
                .expect("player routing poisoned")
                .get(&id)
                .context("player missing from world routing")?,
        };
        let slot = self
            .worlds
            .read()
            .expect("world routing poisoned")
            .get(&id)
            .cloned()
            .context("world no longer exists")?;
        ensure!(!slot.retired(), "world {id} is retired");
        Ok(slot)
    }
    pub(super) fn player(&self, id: i32) -> Result<Option<Arc<Slot>>> {
        let _publication = self.publication.read().expect("world publication poisoned");
        let Some(scope) = self
            .players
            .read()
            .expect("player routing poisoned")
            .get(&id)
            .copied()
        else {
            return Ok(None);
        };
        let slot = self
            .worlds
            .read()
            .expect("world routing poisoned")
            .get(&scope)
            .cloned()
            .context("routed spectator world is missing")?;
        ensure!(!slot.retired(), "spectator world is retired");
        Ok(Some(slot))
    }
    pub(super) fn ids(&self) -> Vec<i32> {
        self.worlds
            .read()
            .expect("world routing poisoned")
            .keys()
            .copied()
            .collect()
    }
}
enum Owner {
    Available(Box<World>),
    Directory,
    Working,
    Poisoned,
    Retired,
}
struct State {
    owner: Owner,
    directory_waiting: bool,
    holder: Option<(&'static str, u64, Instant)>,
}
pub(super) struct Slot {
    pub(super) id: i32,
    state: Mutex<State>,
    changed: Condvar,
    available: tokio::sync::Notify,
    retired: watch::Sender<bool>,
    snapshot: RwLock<Arc<Snapshot>>,
    attribution: Mutex<crate::performance::attribution::Attribution>,
    routes: Arc<RwLock<BTreeMap<i32, i32>>>,
    pub(super) mailbox: mpsc::Sender<Job<World>>,
    receiver: Mutex<Option<mpsc::Receiver<Job<World>>>>,
}
impl Slot {
    fn retired(&self) -> bool {
        *self.retired.borrow()
    }
    fn directory(&self) -> Result<Box<World>> {
        let mut state = self.state.lock().expect("world ownership lock poisoned");
        state.directory_waiting = true;
        loop {
            match state.owner {
                Owner::Available(_) => {
                    state.directory_waiting = false;
                    let (operation, id) = crate::performance::attribution::Context::current();
                    state.holder = Some((operation, id, Instant::now()));
                    let Owner::Available(world) =
                        std::mem::replace(&mut state.owner, Owner::Directory)
                    else {
                        unreachable!()
                    };
                    return Ok(world);
                }
                Owner::Working => {
                    state = self
                        .changed
                        .wait(state)
                        .expect("world ownership wait poisoned")
                }
                Owner::Directory => bail!("world {} already loaned to the Directory", self.id),
                Owner::Poisoned => bail!(
                    "world {} worker failed; damaged state cannot be borrowed",
                    self.id
                ),
                Owner::Retired => bail!("world {} is retired", self.id),
            }
        }
    }
    fn restore(&self, world: Box<World>) -> Result<()> {
        let _context = crate::supervision::world(self.id, world.tick);
        let tick = world.tick;
        let snapshot = Arc::new(Snapshot::capture(&world));
        let mut state = self.state.lock().expect("world ownership lock poisoned");
        ensure!(
            matches!(state.owner, Owner::Directory | Owner::Working),
            "world {} returned without its ownership",
            self.id
        );
        if self.retired() {
            state.owner = Owner::Retired;
        } else {
            let old = self
                .snapshot
                .read()
                .expect("world snapshot poisoned")
                .clone();
            if !old.players.keys().eq(snapshot.players.keys()) {
                let mut routes = self.routes.write().expect("player routing poisoned");
                for id in old
                    .players
                    .keys()
                    .filter(|id| !snapshot.players.contains_key(id))
                {
                    if routes.get(id) == Some(&self.id) {
                        routes.remove(id);
                    }
                }
                for id in snapshot.players.keys() {
                    routes.insert(*id, self.id);
                }
            }
            *self.snapshot.write().expect("world snapshot poisoned") = snapshot;
            state.owner = Owner::Available(world);
        }
        let returned = Instant::now();
        let holder = state.holder.take();
        drop(state);
        self.changed.notify_all();
        self.available.notify_waiters();
        if let Some((operation, id, since)) = holder {
            let mut sample = crate::performance::attribution::Sample::new(operation);
            sample.id = id;
            sample.kind = "directory_loan";
            sample.tick = tick;
            sample.stages[4] = returned.duration_since(since);
            self.record_attribution(sample);
        }
        Ok(())
    }
    fn record_attribution(&self, sample: Box<crate::performance::attribution::Sample>) {
        let report = {
            let mut attribution = self.attribution.lock().expect("world attribution poisoned");
            attribution.record(sample);
            attribution.report()
        };
        if let Some(attribution) = report {
            crate::performance::attribution::Attribution::emit(attribution, Some(self.id));
        }
    }
    fn try_work(
        &self,
        observed_owner: &mut Option<(&'static str, u64)>,
    ) -> Result<Option<Box<World>>> {
        let mut state = self.state.lock().expect("world ownership lock poisoned");
        if observed_owner.is_none() {
            *observed_owner = state.holder.map(|(operation, id, _)| (operation, id));
        }
        if state.directory_waiting {
            return Ok(None);
        }
        match state.owner {
            Owner::Available(_) => {
                let Owner::Available(world) = std::mem::replace(&mut state.owner, Owner::Working)
                else {
                    unreachable!()
                };
                Ok(Some(world))
            }
            Owner::Directory | Owner::Working => Ok(None),
            Owner::Poisoned => bail!("world {} failed; damaged state cannot execute", self.id),
            Owner::Retired => Ok(None),
        }
    }
    fn poison(&self) {
        // Poison recovery here only wakes borrowers to fail; it never resumes damaged gameplay.
        let mut state = self.state.lock().unwrap_or_else(|error| {
            tracing::error!(
                world = self.id,
                "world ownership metadata poisoned during termination"
            );
            error.into_inner()
        });
        state.owner = Owner::Poisoned;
        self.changed.notify_all();
        self.available.notify_waiters();
    }
    fn retire(&self) {
        self.retired.send_replace(true);
        self.available.notify_waiters();
        self.changed.notify_all();
    }
    fn latest(&self) -> Arc<Snapshot> {
        self.snapshot
            .read()
            .expect("world snapshot poisoned")
            .clone()
    }
}
struct Entry {
    slot: Arc<Slot>,
    local: OnceCell<Box<World>>,
}
impl Entry {
    fn world(&self) -> &World {
        // A global job retains its loans until completion, including when ordinary references expire.
        self.local.get_or_init(|| {
            self.slot
                .directory()
                .expect("world ownership acquisition failed")
        })
    }
    fn world_mut(&mut self) -> &mut World {
        self.world();
        self.local.get_mut().expect("Directory world loan missing")
    }
    fn release(&mut self) {
        if let Some(world) = self.local.take() {
            self.slot
                .restore(world)
                .expect("returning Directory world ownership failed");
        }
    }
    fn snapshot(&self) -> Arc<Snapshot> {
        match self.local.get() {
            Some(world) => Arc::new(Snapshot::capture(world)),
            None => self.slot.latest(),
        }
    }
}
#[derive(Clone)]
struct Execution {
    runtime: tokio::runtime::Handle,
    supervisor: crate::supervision::Supervisor,
    pool: Arc<Pool>,
    clock: crate::clock::Clock,
    coordinator: mpsc::WeakSender<Job<Directory>>,
}
#[derive(Default)]
pub struct Worlds {
    entries: BTreeMap<i32, Entry>,
    pub(super) routing: Arc<Routing>,
    execution: Option<Execution>,
}
impl Worlds {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn get(&self, id: &i32) -> Option<&World> {
        self.entries.get(id).map(Entry::world)
    }
    pub fn get_mut(&mut self, id: &i32) -> Option<&mut World> {
        self.entries.get_mut(id).map(Entry::world_mut)
    }
    pub fn insert(&mut self, id: i32, world: World) -> Option<World> {
        let previous = self.remove(&id);
        let (mailbox, receiver) = mpsc::channel(256);
        let slot = Arc::new(Slot {
            id,
            state: Mutex::new(State {
                owner: Owner::Directory,
                directory_waiting: false,
                holder: None,
            }),
            changed: Condvar::new(),
            available: tokio::sync::Notify::new(),
            retired: watch::channel(false).0,
            snapshot: RwLock::new(Arc::new(Snapshot::capture(&world))),
            attribution: Mutex::default(),
            routes: self.routing.players.clone(),
            mailbox,
            receiver: Mutex::new(Some(receiver)),
        });
        self.routing
            .worlds
            .write()
            .expect("world routing poisoned")
            .insert(id, slot.clone());
        if let Some(execution) = &self.execution {
            actor::start(&slot, execution.clone()).expect("new world actor could not start");
        }
        self.entries.insert(
            id,
            Entry {
                slot,
                local: OnceCell::from(Box::new(world)),
            },
        );
        previous
    }
    pub fn remove(&mut self, id: &i32) -> Option<World> {
        self.entries.remove(id).map(|entry| {
            let world = match entry.local.into_inner() {
                Some(world) => world,
                None => entry
                    .slot
                    .directory()
                    .expect("retiring world ownership failed"),
            };
            entry.slot.retire();
            entry
                .slot
                .state
                .lock()
                .expect("world ownership lock poisoned")
                .owner = Owner::Retired;
            self.routing
                .worlds
                .write()
                .expect("world routing poisoned")
                .remove(id);
            self.routing
                .players
                .write()
                .expect("player routing poisoned")
                .retain(|_, world| world != id);
            *world
        })
    }
    pub(super) fn start(
        &mut self,
        clock: crate::clock::Clock,
        supervisor: crate::supervision::Supervisor,
        coordinator: mpsc::WeakSender<Job<Directory>>,
    ) -> Result<()> {
        ensure!(self.execution.is_none(), "world execution started twice");
        let workers = std::thread::available_parallelism()
            .context("simulation CPU allocation unavailable")?
            .get();
        let execution = Execution {
            runtime: tokio::runtime::Handle::current(),
            pool: Pool::new(supervisor.clone(), workers)?,
            supervisor,
            clock,
            coordinator,
        };
        for entry in self.entries.values() {
            actor::start(&entry.slot, execution.clone())?;
        }
        tracing::info!(
            workers,
            worlds = self.entries.len(),
            "simulation CPU pool started"
        );
        self.execution = Some(execution);
        self.release();
        Ok(())
    }
    pub fn contains_key(&self, id: &i32) -> bool {
        self.entries.contains_key(id)
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn keys(&self) -> impl Iterator<Item = &i32> {
        self.entries.keys()
    }
    pub fn iter(&self) -> Iter<'_> {
        Iter(self.entries.iter())
    }
    pub fn iter_mut(&mut self) -> IterMut<'_> {
        IterMut(self.entries.iter_mut())
    }
    pub fn values(&self) -> impl Iterator<Item = &World> {
        self.entries.values().map(Entry::world)
    }
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut World> {
        self.entries.values_mut().map(Entry::world_mut)
    }
    pub(super) fn summaries(&self) -> impl Iterator<Item = (&i32, Arc<Snapshot>)> {
        self.entries.iter().map(|(id, e)| (id, e.snapshot()))
    }
    pub(super) fn summary(&self, id: &i32) -> Option<Arc<Snapshot>> {
        self.entries.get(id).map(Entry::snapshot)
    }
    pub(super) fn release(&mut self) {
        // Route readers must not observe only half of a multi-world transaction's publication.
        let _publication = self
            .routing
            .publication
            .write()
            .expect("world publication poisoned");
        for entry in self.entries.values_mut() {
            entry.release();
        }
    }
    pub(super) fn release_world(&mut self, id: &i32) {
        self.entries
            .get_mut(id)
            .expect("released world missing")
            .release();
    }
}
impl Drop for Worlds {
    fn drop(&mut self) {
        for entry in self.entries.values() {
            entry.slot.retire();
        }
        self.routing
            .worlds
            .write()
            .expect("world routing poisoned")
            .clear();
        self.routing
            .players
            .write()
            .expect("player routing poisoned")
            .clear();
    }
}
impl Index<&i32> for Worlds {
    type Output = World;
    fn index(&self, id: &i32) -> &World {
        self.get(id).expect("world identity missing")
    }
}
impl IndexMut<&i32> for Worlds {
    fn index_mut(&mut self, id: &i32) -> &mut World {
        self.get_mut(id).expect("world identity missing")
    }
}
pub struct Iter<'a>(std::collections::btree_map::Iter<'a, i32, Entry>);
impl<'a> Iterator for Iter<'a> {
    type Item = (&'a i32, &'a World);
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(id, e)| (id, e.world()))
    }
}
impl<'a> IntoIterator for &'a Worlds {
    type Item = (&'a i32, &'a World);
    type IntoIter = Iter<'a>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
pub struct IterMut<'a>(std::collections::btree_map::IterMut<'a, i32, Entry>);
impl<'a> Iterator for IterMut<'a> {
    type Item = (&'a i32, &'a mut World);
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(id, e)| (id, e.world_mut()))
    }
}
impl<'a> IntoIterator for &'a mut Worlds {
    type Item = (&'a i32, &'a mut World);
    type IntoIter = IterMut<'a>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}
