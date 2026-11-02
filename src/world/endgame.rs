use super::*;
use crate::endgame::{CASTLE, CELLAR, HEROES, INCANTATION, LOCKED_CELLAR, RealmCycle, RealmStage};
use rand::seq::IteratorRandom;
mod allocation;

// Gameplay documentation specifies two minutes, unlike modified forks' 20s: https://wikiwiki.jp/rmd/イベント#jded12e1
pub(super) const REMAINING_TAUNT_MS: u64 = 120_000;

#[derive(Default)]
pub(super) struct RealmDirectory {
    pub realms: BTreeMap<i32, RealmRecord>,
    pub(super) forced: BTreeMap<i32, ForcedRoute>,
    preparing: Option<Preparation>,
    demand_since: Option<u64>,
    empty_since: BTreeMap<i32, u64>,
}
enum Preparation {
    Replacement(RealmPreparation),
    Demand(allocation::DemandPreparation),
}
struct RealmPreparation {
    origin: i32,
    castle: i32,
    realm: i32,
    rng_before: rand::rngs::StdRng,
    realm_started: Arc<std::sync::atomic::AtomicBool>,
    result: tokio::sync::oneshot::Receiver<Result<(World, World, RealmCycle)>>,
}
pub(super) struct RealmRecord {
    pub cycle: RealmCycle,
    pub portal: Option<i32>,
    pub name: usize,
    pub enabled: bool,
    replacement: Option<i32>,
    pub castle: Option<i32>,
    pub pending_transfers: BTreeSet<i32>,
    publish_at: Option<u64>,
    retry_at: u64,
    next_taunt: u64,
}
impl RealmRecord {
    fn new(cycle: RealmCycle, now: u64) -> Self {
        Self {
            cycle,
            portal: None,
            name: 0,
            enabled: true,
            replacement: None,
            castle: None,
            pending_transfers: BTreeSet::new(),
            publish_at: None,
            retry_at: 0,
            next_taunt: now + REMAINING_TAUNT_MS,
        }
    }
}
#[derive(Clone, Copy)]
pub(super) struct ForcedRoute {
    pub origin: i32,
    destination: i32,
    select_when_paused: bool,
    frozen: bool,
}
impl RealmDirectory {
    pub(super) fn references_world(&self, id: i32) -> bool {
        self.realms.contains_key(&id)
            || self
                .realms
                .values()
                .any(|r| r.castle == Some(id) && !r.pending_transfers.is_empty())
            || self
                .forced
                .values()
                .any(|r| r.origin == id || r.destination == id)
    }
}
pub struct ForcedTransfer {
    pub player: i32,
    pub output: Output,
    pub destination: Option<i32>,
    pub key_time: i32,
    pub key: Vec<u8>,
    pub name: String,
}
impl ForcedTransfer {
    pub fn packet(&self) -> Packet {
        let mut o = Writer::new();
        let Some(destination) = self.destination else {
            // Selected 123.5.1 FAILURE 5 completes the scene without reconnecting a paused member.
            o.i32(5)
                .utf("Realm closed while you were paused. Select your character to continue.");
            return Packet { id: 0, body: o.0 };
        };
        o.utf(&self.name)
            .utf("")
            .i32(2050)
            .i32(destination)
            .i32(self.key_time)
            .i16(self.key.len() as i16);
        o.0.extend_from_slice(&self.key);
        Packet { id: 39, body: o.0 }
    }
}
impl Directory {
    pub fn stage_unlock(&mut self, plan: InventoryPlan) -> Result<InventoryPlan> {
        let Some(portal) = plan.unlock else {
            return Ok(plan);
        };
        let PortalPlan { locked, opened } = portal;
        ensure!(!self.portals.contains_key(&opened), "portal already staged");
        let origin = self.location(plan.player)?;
        let w = &self.worlds[&origin];
        let p = &w.players[&plan.player];
        ensure!(
            p.pending
                && p.character.revision == plan.revision
                && p.character.account == plan.account
                && p.character.id == plan.character,
            "portal owner reservation changed"
        );
        let kind = w
            .entities
            .get(&opened)
            .context("staged portal missing")?
            .kind;
        let template = if let Some(locked) = locked {
            ensure!(
                w.entities
                    .get(&locked)
                    .is_some_and(|e| e.kind == LOCKED_CELLAR && e.reserved && e.published),
                "locked portal reservation missing"
            );
            ensure!(kind == 0x700, "Incantation staged the wrong portal");
            Some(CELLAR)
        } else {
            crate::dungeon::DUNGEONS
                .iter()
                .chain(crate::endgame::DUNGEONS)
                .find(|(portal, _, _, _)| *portal == kind)
                .map(|(_, template, _, _)| *template)
        };
        ensure!(
            w.entities
                .get(&opened)
                .is_some_and(|e| e.kind == kind && e.reserved && !e.published),
            "unpublished portal missing"
        );
        let destination = match template
            .map(|template| {
                let route = self.dungeon_return_from(origin)?;
                self.instance(template, None, Some(route))
            })
            .transpose()
        {
            Ok(destination) => destination,
            Err(error) => {
                let w = self.worlds.get_mut(&origin).unwrap();
                w.discard_unlock(locked, opened)?;
                w.players.get_mut(&plan.player).unwrap().pending = false;
                return Err(error);
            }
        };
        self.portals.insert(
            opened,
            Portal {
                origin,
                destination,
                expires: self.tick + portal.lifetime(),
            },
        );
        tracing::info!(
            player = plan.player,
            opened,
            destination,
            template,
            origin,
            kind,
            "portal binding staged; consumable not committed or portal published yet"
        );
        Ok(plan)
    }
    pub fn finish_inventory(&mut self, mut plan: InventoryPlan, result: Result<i32>) -> Result<()> {
        let Some(portal) = plan.unlock.take() else {
            return self
                .resolve(Target::Player(plan.player))?
                .finish_inventory(plan, result);
        };
        let PortalPlan { locked, opened } = portal;
        let player = plan.player;
        let origin = self.location(player)?;
        let w = &self.worlds[&origin];
        let p = &w.players[&player];
        ensure!(
            p.pending
                && p.character.revision == plan.revision
                && p.character.account == plan.account
                && p.character.id == plan.character,
            "stale or foreign portal completion"
        );
        ensure!(
            locked.is_none_or(|id| w
                .entities
                .get(&id)
                .is_some_and(|e| e.reserved && e.kind == LOCKED_CELLAR))
                && w.entities
                    .get(&opened)
                    .is_some_and(|e| e.reserved && !e.published),
            "portal reservation lost or already published"
        );
        let binding = self
            .portals
            .get(&opened)
            .context("staged portal binding lost")?;
        ensure!(binding.origin == origin, "staged portal origin changed");
        let destination = binding.destination;
        if result.is_err() {
            self.worlds
                .get_mut(&origin)
                .unwrap()
                .discard_unlock(locked, opened)?;
            self.portals.remove(&opened);
            if let Some(destination) = destination {
                ensure!(
                    self.worlds[&destination].players.is_empty()
                        && !self.grants.values().any(|g| g.destination == destination),
                    "unpublished dungeon acquired a participant"
                );
                self.worlds.remove(&destination);
                self.idle.remove(&destination);
            }
            return self
                .worlds
                .get_mut(&origin)
                .unwrap()
                .finish_inventory(plan, result);
        }
        self.worlds
            .get_mut(&origin)
            .unwrap()
            .finish_inventory(plan, result)?;
        let w = self.worlds.get_mut(&origin).unwrap();
        if let Some(locked) = locked {
            w.entities
                .get_mut(&locked)
                .context("committed locked portal lost")?
                .reserved = false;
            w.remove_entity(locked);
        }
        let e = w
            .entities
            .get_mut(&opened)
            .context("committed portal lost")?;
        e.reserved = false;
        e.published = true;
        let kind = e.kind;
        let dungeon = e.name.clone();
        self.portals.get_mut(&opened).unwrap().expires = self.tick + portal.lifetime();
        let name = &w.players[&player].character.name;
        if locked.is_some() {
            // Period unlock feedback belongs after commit, never reservation: https://www.youtube.com/watch?v=H44UToO5faY&t=9
            let mut note = Writer::new();
            note.i32(player)
                .utf(&format!("Unlocked by {name}"))
                .u32(0xff00ff00);
            for p in w
                .players
                .values()
                .filter(|p| !p.departing && p.persistence_failure.is_none())
            {
                send(p, wire::NOTIFICATION, note.0.clone());
            }
            w.broadcast_text("", &format!("{dungeon} unlocked by {name}"));
        } else {
            w.broadcast_text("", &format!("{dungeon} opened by {name}!"));
        }
        let p = &w.players[&player];
        tracing::info!(
            player,
            account = p.character.account,
            character = p.character.id,
            revision = p.character.revision,
            opened,
            kind,
            destination,
            origin,
            expires_tick = self.portals[&opened].expires,
            "consumable committed; bound dungeon portal published"
        );
        Ok(())
    }
    fn chamber_step(&mut self, ready: &[i32]) {
        let now = self.tick * 50;
        let castles: Vec<_> = self
            .worlds
            .summaries()
            .filter(|(id, w)| {
                (ready.contains(id) || !w.needs_coordinator())
                    && w.chamber_at.is_some_and(|at| at <= now)
            })
            .map(|(id, _)| *id)
            .collect();
        let markers: Vec<_> = castles
            .into_iter()
            .flat_map(|origin| {
                self.worlds[&origin]
                    .entities
                    .values()
                    .filter(move |e| e.kind == 0xd7a && e.next_aux <= now)
                    .map(move |e| (origin, e.id, e.pos))
            })
            .collect();
        let origins: Vec<_> = markers.iter().map(|(origin, _, _)| *origin).collect();
        for (origin, marker, pos) in markers {
            let result = (|| -> Result<()> {
                ensure!(
                    self.worlds[&origin].walkable(pos),
                    "authored Chamber entrance position blocked"
                );
                let destination = self.instance(crate::endgame::CHAMBER, None, None)?;
                let portal =
                    match self
                        .worlds
                        .get_mut(&origin)
                        .unwrap()
                        .spawn_entity(0xd7b, pos, vec![])
                    {
                        Ok(portal) => portal,
                        Err(error) => {
                            self.worlds.remove(&destination);
                            self.idle.remove(&destination);
                            return Err(error);
                        }
                    };
                self.portals.insert(
                    portal,
                    Portal {
                        origin,
                        destination: Some(destination),
                        expires: self.tick + 2400,
                    },
                );
                let w = self.worlds.get_mut(&origin).unwrap();
                w.remove_entity(marker);
                w.oryx_notice("You have defeated my Guardians. Face me in my Chamber!");
                tracing::info!(
                    origin,
                    portal,
                    destination,
                    "Guardian pair earned its bound Chamber portal"
                );
                Ok(())
            })();
            if let Err(error) = result {
                self.worlds
                    .get_mut(&origin)
                    .unwrap()
                    .entities
                    .get_mut(&marker)
                    .unwrap()
                    .next_aux = self.tick * 50 + 5000;
                tracing::error!(origin,%error,"earned Chamber binding failed; retry scheduled without publishing a portal");
            }
        }
        for origin in origins {
            self.worlds.release_world(&origin);
        }
    }
    pub(crate) fn prepare_realm_closure(&mut self, origin: i32) -> Result<(i32, Pos)> {
        ensure!(
            self.prepared_seed.is_some(),
            "closure setup requires prepared state"
        );
        ensure!(self.clock.paused(), "closure setup requires a paused clock");
        ensure!(
            self.worlds
                .values()
                .all(|w| w.players.is_empty() && w.reserved_slots() == 0),
            "closure setup requires no enrolled players or reservations"
        );
        let record = self
            .endgame
            .realms
            .get_mut(&origin)
            .context("closure Realm missing")?;
        ensure!(
            record.cycle.admits() && record.enabled,
            "closure setup requires an open Realm"
        );
        let world = self
            .worlds
            .get_mut(&origin)
            .context("closure world missing")?;
        let hero = record
            .cycle
            .remaining
            .iter()
            .copied()
            .find(|id| {
                world
                    .entities
                    .get(id)
                    .is_some_and(|e| e.kind == 0x67d && e.hp > 0)
            })
            .context("closure setup needs a living Cyclops God")?;
        let position = world.entities[&hero].pos;
        ensure!(
            world.walkable(position),
            "prepared final hero arrival is blocked"
        );
        world.entities.get_mut(&hero).unwrap().hp = 1;
        // Native positions need client acknowledgement; prepare ordinary arrival before enrollment instead.
        Arc::make_mut(&mut world.map).spawns = vec![(position.x, position.y)];
        record.cycle.remaining = BTreeSet::from([hero]);
        world.event_reserves.clear();
        tracing::info!(
            realm = origin,
            hero,
            "prepared final Realm hero and arrival; native defeat still required"
        );
        Ok((hero, position))
    }
    pub(super) fn bind_realm_heroes(&mut self, origin: i32, heroes: BTreeSet<i32>) -> Result<()> {
        let world = self.worlds.get(&origin).context("closure Realm missing")?;
        ensure!(
            world.name == "Realm" && world.admission_open,
            "closure roster requires an open Realm"
        );
        ensure!(
            !self.endgame.realms.contains_key(&origin),
            "Realm roster already bound"
        );
        for id in &heroes {
            let hero = world.entities.get(id).context("closure hero missing")?;
            ensure!(
                (HEROES.contains(&hero.kind)
                    || crate::quest::root(hero.kind) && hero.quest == Some(*id))
                    && hero.hp > 0,
                "closure identity is not a living qualifying hero"
            );
        }
        let cycle = RealmCycle::new(heroes)?;
        tracing::info!(
            origin,
            heroes = cycle.remaining.len(),
            "finite Realm closure roster bound"
        );
        self.endgame
            .realms
            .insert(origin, RealmRecord::new(cycle, self.tick * 50));
        Ok(())
    }
    pub(super) fn endgame_step(&mut self, ready: &[i32]) {
        self.chamber_step(ready);
        let now = self.tick * 50;
        let due:Vec<_>=self.worlds.summaries().filter(|(id,world)| {
            ready.contains(id) && world.critical || (ready.contains(id) || !world.needs_coordinator()) && self.endgame.realms.get(id).is_some_and(|record| {
                record.enabled && record.cycle.admits() && now>=record.next_taunt
                    || matches!(record.cycle.stage, RealmStage::Closing{deadline}|RealmStage::Sealed{deadline} if now>=deadline)
            })
        }).map(|(id,_)|*id).collect();
        for origin in &due {
            let world = self.worlds.get_mut(origin).expect("endgame source missing");
            let deaths = std::mem::take(&mut world.critical_deaths);
            let Some(record) = self.endgame.realms.get_mut(origin) else {
                self.worlds.release_world(origin);
                continue;
            };
            let cycle = &mut record.cycle;
            for id in deaths {
                if cycle.admits() && cycle.remaining.contains(&id) {
                    match world.earn_realm_event(id) {
                        Ok(realm_events::EventAdmission::Spawned(event)) => {
                            assert!(
                                cycle.remaining.insert(event),
                                "summoned event already counted"
                            );
                        }
                        Ok(realm_events::EventAdmission::Waiting) => {
                            world.critical_deaths.push(id);
                            continue;
                        }
                        Err(error) => {
                            tracing::error!(origin, hero=id, %error, "earned Realm event initialization refused; retained for retry");
                            world.critical_deaths.push(id);
                            continue;
                        }
                        Ok(realm_events::EventAdmission::Miss) => {}
                    }
                }
                let killer = world.critical_killers.remove(&id);
                if cycle.defeated(id, now) {
                    if let Some((kind, player)) = killer {
                        world.realm_killed_notice(kind, &player);
                    }
                    world.last_realm_enemy_notice(&cycle.remaining);
                    tracing::info!(
                        origin,
                        hero = id,
                        remaining = cycle.remaining.len(),
                        "qualifying Realm hero defeated"
                    );
                    if !cycle.admits() {
                        world.admission_open = false;
                        world.oryx_notice(
                            "I HAVE CLOSED THIS REALM! YOU WILL NOT LIVE TO SEE THE LIGHT OF DAY!",
                        );
                        self.grants.retain(|_, g| g.destination != *origin);
                        world.admissions.clear();
                    }
                }
            }
            if record.enabled && cycle.admits() && now >= record.next_taunt {
                record.next_taunt = now + REMAINING_TAUNT_MS;
                if let Some((kind, count, text)) = world.remaining_taunt(&cycle.remaining) {
                    world.oryx_notice(&text);
                    tracing::info!(world = origin, time_ms = now, kind, count, %text, "Oryx remaining encounters announced");
                }
            }
            while cycle.advance(now) {
                tracing::info!(origin, now, stage = ?cycle.stage, "Realm closure deadline reached");
                if matches!(cycle.stage, RealmStage::Sealed { .. }) {
                    for text in [
                        "MY MINIONS HAVE FAILED ME!",
                        "BUT NOW YOU SHALL FEEL MY WRATH",
                        "COME MEET YOUR DOOM AT THE WALLS OF MY CASTLE!",
                    ] {
                        world.oryx_notice(text);
                    }
                    // The selected client starts camera jitter at 14; 13 needs a target: https://github.com/Zolmex/kithio-as3-source/blob/d15f321edcce0ef890001890c6991c25a1b0f718/Rotmg123.5.1/src/com/company/assembleegameclient/net/_1f.as#L900-L908
                    let quake = cooperative::effect(14, -1, Pos { x: 0., y: 0. });
                    for p in world.players.values() {
                        send(p, wire::SHOW_EFFECT, quake.clone());
                    }
                }
            }
            // Preparation owns its inputs; even closing Realms must run while another loan is pending.
            self.worlds.release_world(origin);
        }
        let transferring: Vec<_> = self
            .endgame
            .realms
            .iter()
            .filter(|(_, record)| {
                record.cycle.stage == RealmStage::Transferring
                    && record.replacement.is_none()
                    && self.tick >= record.retry_at
            })
            .map(|(id, _)| *id)
            .collect();
        for origin in transferring {
            if let Err(error) = self.replace_realm(origin) {
                self.endgame.realms.get_mut(&origin).unwrap().retry_at = self.tick + 100;
                tracing::error!(origin, %error, "Realm transfer/replacement withheld; retry in five seconds");
            }
        }
        let publishing: Vec<_> = self
            .endgame
            .realms
            .iter()
            .filter(|(_, record)| record.publish_at.is_some_and(|tick| self.tick >= tick))
            .map(|(origin, record)| (*origin, record.replacement.unwrap()))
            .collect();
        for (origin, replacement) in publishing {
            self.endgame.realms.get_mut(&replacement).unwrap().enabled = true;
            self.worlds.get_mut(&replacement).unwrap().admission_open = true;
            self.endgame.realms.get_mut(&origin).unwrap().publish_at = None;
            tracing::info!(
                origin,
                replacement,
                "safe collective departure completed; replacement published"
            );
        }
        self.endgame.forced.retain(|player, route| {
            self.worlds
                .summary(&route.origin)
                .is_some_and(|w| w.players.contains_key(player))
        });
        let retired: Vec<_> = self
            .endgame
            .realms
            .iter()
            .filter(|(_, r)| {
                r.replacement.is_some() && r.publish_at.is_none() && r.pending_transfers.is_empty()
            })
            .map(|(id, _)| *id)
            .collect();
        for origin in retired {
            if self.realm_dependencies_clear(origin) {
                self.remove_empty_realm(origin);
            }
        }
    }
    pub(crate) fn realm_departed(&mut self, origin: i32, player: i32) {
        let Some(record) = self.endgame.realms.get_mut(&origin) else {
            return;
        };
        if record.pending_transfers.remove(&player) && record.pending_transfers.is_empty() {
            record.publish_at = Some(self.tick + crate::endgame::REPLACEMENT_DELAY_MS / 50);
            tracing::info!(
                origin,
                publish_tick = record.publish_at,
                "all collective recipients durably departed; replacement delay started"
            );
        }
    }
    fn replace_realm(&mut self, origin: i32) -> Result<()> {
        ensure!(self.worlds.contains_key(&origin), "closing Realm missing");
        if matches!(self.endgame.preparing, Some(Preparation::Demand(_))) {
            return Ok(());
        }
        if let Some(Preparation::Replacement(preparation)) = &self.endgame.preparing {
            if preparation.origin != origin {
                return Ok(());
            }
        } else {
            let castle_id = self.next_world;
            let realm_id = castle_id
                .checked_add(1)
                .context("world identity exhausted")?;
            let next = realm_id
                .checked_add(1)
                .context("world identity exhausted")?;
            ensure!(
                !self.worlds.contains_key(&castle_id) && !self.worlds.contains_key(&realm_id),
                "world identity collision"
            );
            let rng_before = self.realm_rng.clone();
            let castle = self.construction(CASTLE, "Oryx's Castle", false, None);
            let realm = self.construction("realm-shore", "Realm", false, None);
            let realm_started = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let started = realm_started.clone();
            let result = match self.worlds.prepare_realm(origin, self.tick, move || {
                let castle = castle()?;
                started.store(true, std::sync::atomic::Ordering::Release);
                let realm = realm()?;
                let cycle = RealmCycle::new(realm.hero_identities())?;
                Ok((castle, realm, cycle))
            }) {
                Ok(result) => result,
                Err(error) => {
                    self.realm_rng = rng_before;
                    return Err(error);
                }
            };
            // One unpublished build preserves Realm selection/publication order without occupying simulation workers.
            self.endgame.preparing = Some(Preparation::Replacement(RealmPreparation {
                origin,
                castle: castle_id,
                realm: realm_id,
                rng_before,
                realm_started,
                result,
            }));
            self.next_world = next;
        }
        let Some(Preparation::Replacement(preparation)) = self.endgame.preparing.as_mut() else {
            unreachable!("replacement owns construction");
        };
        let result = match preparation.result.try_recv() {
            Ok(result) => result,
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => return Ok(()),
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                self.endgame.preparing.take();
                bail!("Realm preparation worker stopped without returning its result");
            }
        };
        let Some(Preparation::Replacement(preparation)) = self.endgame.preparing.take() else {
            unreachable!("replacement owns construction");
        };
        if !preparation
            .realm_started
            .load(std::sync::atomic::Ordering::Acquire)
        {
            self.realm_rng = preparation.rng_before;
        }
        let (castle, realm, next_cycle) = result?;
        let (castle_id, realm_id) = (preparation.castle, preparation.realm);
        let name = (1..=super::realm_portal::NAMES.len())
            .map(|offset| {
                (self.endgame.realms[&origin].name + offset) % super::realm_portal::NAMES.len()
            })
            .find(|name| self.endgame.realms.values().all(|r| r.name != *name))
            .context("no unused Realm name")?;
        let players: Vec<_> = self.worlds[&origin]
            .players
            .iter()
            .filter(|(_, p)| {
                p.character.alive
                    && p.character.hp > 0
                    && !p.death_pending
                    && !p.departing
                    && p.persistence_failure.is_none()
                    && p.tx
                        .as_ref()
                        .is_some_and(|tx| !tx.packets.is_closed() && !tx.lost.borrow().is_some())
            })
            .map(|(id, _)| *id)
            .collect();
        let old_portal = self.endgame.realms[&origin]
            .portal
            .context("closing Realm has no exact Nexus portal binding")?;
        let nexus = self.worlds.get_mut(&-2).context("Nexus missing")?;
        let entrance = nexus
            .entities
            .get(&old_portal)
            .context("closing Realm entrance missing")?;
        ensure!(
            entrance.kind == 0x712,
            "closing Realm entrance kind changed"
        );
        let portal = self.install_realm_portal(realm_id, Some(old_portal))?;
        let mut realm = realm;
        realm.admission_open = false;
        self.worlds.insert(castle_id, castle);
        self.worlds.insert(realm_id, realm);
        self.idle.insert(castle_id, self.tick);
        let mut record = RealmRecord::new(next_cycle, self.tick * 50);
        record.portal = Some(portal);
        record.name = name;
        record.enabled = false;
        self.endgame.realms.insert(realm_id, record);
        let record = self.endgame.realms.get_mut(&origin).unwrap();
        record.replacement = Some(realm_id);
        record.castle = Some(castle_id);
        record.pending_transfers = players.iter().copied().collect();
        if players.is_empty() {
            record.publish_at = Some(self.tick + crate::endgame::REPLACEMENT_DELAY_MS / 50);
        }
        for player in &players {
            self.endgame.forced.insert(
                *player,
                ForcedRoute {
                    origin,
                    destination: castle_id,
                    select_when_paused: true,
                    frozen: false,
                },
            );
        }
        tracing::info!(
            origin,
            castle = castle_id,
            replacement = realm_id,
            recipients = players.len(),
            "Realm closed; collective Castle route and fresh Realm installed"
        );
        Ok(())
    }
    pub(super) fn return_to_nexus(&mut self, player: i32, origin: i32, frozen: bool) {
        self.endgame.forced.insert(
            player,
            ForcedRoute {
                origin,
                destination: -2,
                select_when_paused: false,
                frozen,
            },
        );
    }

    pub(crate) fn cancel_forced_transfer(&mut self, transfer: &ForcedTransfer) {
        if let Some(grant) = self.grants.remove(&transfer.key) {
            self.worlds
                .get_mut(&grant.destination)
                .unwrap()
                .admissions
                .remove(&transfer.key);
            tracing::error!(
                origin = grant.origin,
                destination = grant.destination,
                player = transfer.player,
                "durable collective departure failed; reservation revoked and origin retained for quarantine"
            );
        }
    }
    pub fn forced_transfer(&mut self, player: i32) -> Result<Option<ForcedTransfer>> {
        let Some(&ForcedRoute {
            origin,
            destination,
            select_when_paused,
            frozen,
        }) = self.endgame.forced.get(&player)
        else {
            return Ok(None);
        };
        let Some(p) = self
            .worlds
            .get(&origin)
            .and_then(|w| w.players.get(&player))
        else {
            self.endgame.forced.remove(&player);
            return Ok(None);
        };
        if !p.character.alive
            || p.character.hp <= 0
            || p.death_pending
            || p.departing && !frozen
            || p.persistence_failure.is_some()
        {
            self.endgame.forced.remove(&player);
            return Ok(None);
        }
        if p.pending || p.experience_queued > 0 || p.clears_queued.iter().any(|count| *count > 0) {
            return Ok(None);
        }
        let destination = (!select_when_paused || !p.paused).then_some(destination);
        let output =
            p.tx.clone()
                .context("collective transfer recipient lacks an owned RECONNECT output")?;
        if output.packets.is_closed() || output.lost.borrow().is_some() {
            self.endgame.forced.remove(&player);
            tracing::info!(
                player,
                origin,
                "disconnected recipient skipped without erasing its owned avatar"
            );
            return Ok(None);
        }
        let (account, character) = (p.character.account, p.character.id);
        self.retire_transfer_grants();
        if destination.is_some() && self.grants.len() >= 64 {
            return Ok(None);
        }
        let (key_time, key, name) = match destination {
            Some(destination) => self.issue(account, character, destination)?,
            None => (0, Vec::new(), String::new()),
        };
        let world = self.worlds.get_mut(&origin).unwrap();
        world.cancel_trade(player, "Trade cancelled by Realm closure.");
        let p = world.players.get_mut(&player).unwrap();
        // Freeze in the same actor job as grant issuance, before an enemy can kill the departing avatar.
        p.departing = true;
        p.tx = None;
        p.bot_goal = None;
        p.bot_aim = None;
        self.endgame.forced.remove(&player);
        Ok(Some(ForcedTransfer {
            player,
            output,
            destination,
            key_time,
            key,
            name,
        }))
    }
}
impl World {
    pub(super) fn populate_realm_heroes(
        &mut self,
        sites: &[crate::terrain::realms::Site],
    ) -> Result<()> {
        // GPL-3.0 requested counts, not authenticated 123.5.1 density (data/REALM-SRC-LICENSE):
        // https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/realm/setpieces/SetPieces.cs
        // Unlike its 50-attempt skips, our protected allocator fulfills requests or errors.
        for (kind, terrains, range) in [(HEROES[0], &[1][..], 3..5), (HEROES[1], &[3, 4][..], 4..7)]
        {
            let authored = sites.iter().filter(|site| site.kind == kind).count();
            let requested: usize = self.rng.gen_range(range);
            let additional = requested.saturating_sub(authored);
            tracing::info!(
                kind,
                authored,
                requested,
                additional,
                "Realm hero site census"
            );
            self.populate_landmark_heroes(kind, terrains, additional)?;
        }
        Ok(())
    }
    fn populate_landmark_heroes(&mut self, kind: u16, terrains: &[u8], count: usize) -> Result<()> {
        for _ in 0..count {
            let pos = self.place_named_setpiece(kind, terrains)?;
            self.spawn_entity(kind, pos, vec![])?;
        }
        tracing::info!(kind, count, "finite landmark heroes placed");
        Ok(())
    }
    pub(super) fn hero_identities(&self) -> BTreeSet<i32> {
        self.entities
            .values()
            .filter(|e| {
                HEROES.contains(&e.kind) || (crate::quest::root(e.kind) && e.quest == Some(e.id))
            })
            .map(|e| e.id)
            .collect()
    }
    pub(super) fn prepare_endgame(&mut self) -> Result<()> {
        if self.name == "Oryx's Castle" {
            for e in self.entities.matching_mut(|e| e.kind == 0xd7d) {
                e.condition |= crate::condition::bit(crate::condition::INVINCIBLE);
                e.published = false;
            }
            ensure!(
                crate::endgame::GUARDIANS.iter().all(|kind| self
                    .entities
                    .values()
                    .filter(|e| e.kind == *kind)
                    .count()
                    == 1),
                "Castle requires the authored Guardian pair"
            );
            let markers: Vec<_> = self
                .entities
                .values()
                .filter(|e| e.kind == 0xd7a)
                .map(|e| e.id)
                .collect();
            ensure!(
                markers.len() == 1,
                "Castle requires one authored Guardian taskmaster"
            );
            let e = self.entities.get_mut(&markers[0]).unwrap();
            e.published = false;
            e.condition |= crate::condition::bit(crate::condition::INVINCIBLE);
        }
        if matches!(self.name.as_str(), "Oryx's Chamber" | "Wine Cellar") {
            let boss = if self.name == "Wine Cellar" {
                crate::endgame::ORYX2
            } else {
                crate::endgame::ORYX
            };
            ensure!(
                self.entities.values().filter(|e| e.kind == boss).count() == 1,
                "endgame room requires one selected boss"
            );
            for e in self.entities.matching_mut(|e| e.kind == 0x71d) {
                e.published = false;
            }
        }
        if self.name == "Wine Cellar" {
            // Build 122 added destructible barrels (https://web.archive.org/web/20130605124909/forums.wildshadow.com/node/41768); their room-corner placement is a reconstruction.
            let open = |x: i32, y: i32| {
                self.walkable(Pos {
                    x: x as f32 + 0.5,
                    y: y as f32 + 0.5,
                })
            };
            let corners: Vec<_> = (0..self.map.height)
                .flat_map(|y| (0..self.map.width).map(move |x| (x, y)))
                .filter(|&(x, y)| {
                    let cell = &self.map.cells[(y * self.map.width + x) as usize];
                    cell.object.is_none()
                        && cell.region != 1
                        && open(x, y)
                        && [(-1, -1), (-1, 1), (1, -1), (1, 1)]
                            .iter()
                            .any(|&(dx, dy)| {
                                !open(x - dx, y)
                                    && !open(x, y - dy)
                                    && open(x + dx, y)
                                    && open(x, y + dy)
                                    && open(x + dx, y + dy)
                            })
                })
                .collect();
            ensure!(!corners.is_empty(), "Wine Cellar map has no barrel corners");
            for &(x, y) in &corners {
                let pos = Pos {
                    x: x as f32 + 0.5,
                    y: y as f32 + 0.5,
                };
                self.spawn_entity(0xde5, pos, vec![])?;
            }
            tracing::info!(barrels = corners.len(), "reconstructed Wine Barrels placed");
        }
        Ok(())
    }
    pub(super) fn begin_unlock(
        &mut self,
        id: i32,
        slot: Slot,
        item: &crate::content::Object,
    ) -> Result<InventoryPlan> {
        ensure!(
            item.kind == INCANTATION && item.activations == 1,
            "unsupported portal unlock consumable"
        );
        let (dungeon, locked_name) = item
            .unlock_portal
            .as_ref()
            .context("unlock activation metadata missing")?;
        ensure!(
            dungeon == "Wine Cellar" && locked_name == &self.content.get(LOCKED_CELLAR)?.name,
            "Incantation activation contradicts selected Cellar portal"
        );
        ensure!(
            self.name == "Oryx's Chamber" && self.cleared,
            controller("Wine Cellar requires a defeated Oryx")
        );
        let p = &self.players[&id];
        let locked = self
            .entities
            .values()
            .filter(|e| e.kind == LOCKED_CELLAR && e.published && p.pos.distance(e.pos) <= 3.)
            .min_by(|a, b| p.pos.distance(a.pos).total_cmp(&p.pos.distance(b.pos)))
            .ok_or_else(|| controller("no locked Wine Cellar portal nearby"))?;
        ensure!(
            !locked.reserved,
            controller("another Incantation already reserves this portal")
        );
        let (locked, pos) = (locked.id, locked.pos);
        self.reserve_portal(id, slot, 0x700, Some(locked), pos, dungeon.clone())
    }
    pub(super) fn begin_create(
        &mut self,
        id: i32,
        slot: Slot,
        item: &crate::content::Object,
    ) -> Result<InventoryPlan> {
        ensure!(
            item.consumable && item.activations == 1,
            "invalid dungeon key activation"
        );
        let name = item
            .create_object
            .as_ref()
            .context("key portal identity missing")?;
        let portal = self
            .content
            .objects
            .values()
            .find(|p| &p.name == name)
            .context("key references an unknown portal")?;
        ensure!(
            portal.class == "Portal" && portal.dungeon_name.is_some(),
            "key target is not a dungeon portal"
        );
        let dungeon = portal
            .dungeon_name
            .clone()
            .context("key portal dungeon name missing")?;
        self.reserve_portal(id, slot, portal.kind, None, self.players[&id].pos, dungeon)
    }
    fn reserve_portal(
        &mut self,
        id: i32,
        slot: Slot,
        kind: u16,
        locked: Option<i32>,
        pos: Pos,
        name: String,
    ) -> Result<InventoryPlan> {
        let usable = crate::dungeon::DUNGEONS
            .iter()
            .chain(crate::endgame::DUNGEONS)
            .any(|(portal, _, _, _)| *portal == kind);
        let mut items = self.players[&id].character.items.clone();
        items[slot.slot] = -1;
        let opened = self.spawn_entity(kind, pos, vec![])?;
        let e = self.entities.get_mut(&opened).unwrap();
        e.name = name;
        e.extra_stats.retain(|(stat, _)| *stat != 37);
        e.extra_stats.push((37, i32::from(usable)));
        e.published = false;
        e.reserved = true;
        if let Some(locked) = locked {
            self.entities.get_mut(&locked).unwrap().reserved = true;
        }
        self.players.get_mut(&id).unwrap().pending = true;
        let mut plan = self.plan(id, items, None);
        plan.unlock = Some(PortalPlan { locked, opened });
        Ok(plan)
    }
    fn discard_unlock(&mut self, locked: Option<i32>, opened: i32) -> Result<()> {
        ensure!(
            self.entities
                .get(&opened)
                .is_some_and(|e| !e.published && e.reserved),
            "cannot discard a published portal"
        );
        self.entities.get_mut(&opened).unwrap().reserved = false;
        self.remove_entity(opened);
        if let Some(locked) = locked {
            self.entities
                .get_mut(&locked)
                .context("reserved locked portal lost")?
                .reserved = false;
        }
        Ok(())
    }
    fn remaining_taunt(&self, remaining: &BTreeSet<i32>) -> Option<(u16, usize, String)> {
        let mut families: BTreeMap<u16, BTreeSet<i32>> = BTreeMap::new();
        for e in self.actors().filter(|e| e.hp > 0) {
            let family = match e.kind {
                0x91b | 0x91c => 0x91b,
                0x91f | 0x920 => 0x91f,
                0x928 | 0x92d => 0x928,
                0x675 | 0x6c1 | 0x6c2 => 0x675,
                kind if HEROES.contains(&kind)
                    || crate::quest::EVENT_BOSSES.contains(&kind)
                    || kind == 0x678 =>
                {
                    kind
                }
                _ => continue,
            };
            let tag = match *e.quest {
                Some(tag) => tag,
                None => e.id,
            };
            if remaining.contains(&tag) {
                families.entry(family).or_default().insert(tag);
            }
        }
        let (&kind, ids) = families.iter().choose(&mut rand::thread_rng())?;
        let count = ids.len();
        let text = match (kind, count == 1) {
            (0x91b, false) => format!("I am invincible while my {count} Liches still stand!"),
            (0x91b, true) => "My final Lich will protect me forever!".into(),
            (0x91f, false) => {
                format!("Mortal scum! My {count} Ent Ancients will defend me forever!")
            }
            (0x91f, true) => "My final Ent Ancient will destroy you all!".into(),
            (0x928, false) => {
                format!("My {count} Ghost Kings give me more than enough protection!")
            }
            (0x928, true) => "My final Ghost King is untouchable!".into(),
            (0x675, false) => format!("Maggots! My {count} Phoenix Lords will burn you to ash!"),
            (0x675, true) => "My final Phoenix Lord will never fall!".into(),
            (0x678, false) => format!("You have no hope against my {count} Oasis Giants!"),
            (0x678, true) => "A powerful Oasis Giant still fights for me!".into(),
            (0x668, false) => format!("Fools! There is no escape from my {count} Red Demons!"),
            (0x668, true) => "My final Red Demon is unassailable!".into(),
            (0x67d, false) => format!("Cretins! I have {count} Cyclops Gods to guard me!"),
            (0x67d, true) => "My last Cyclops God will smash you to pieces!".into(),
            (0xd56, false) => format!("Imbeciles! My {count} Skull Shrines make me invincible!"),
            (0xd56, true) => "Pathetic fools! A Skull Shrine guards me!".into(),
            (0xd59, false) => format!("Filthy vermin! My {count} Cube Gods will exterminate you!"),
            (0xd59, true) => "Worthless mortals! A mighty Cube God defends me!".into(),
            (0xd5f, false) => format!("Wretched creatures! {count} Pentaracts remain!"),
            (0xd5f, true) => "Ignorant fools! A Pentaract guards me still!".into(),
            (0xd54, false) => format!(
                "You dull-spirited apes! You shall pose no challenge for {count} Grand Sphinxes!"
            ),
            (0xd54, true) => {
                "Gaze upon the beauty of the Grand Sphinx and feel your last hopes drain away."
                    .into()
            }
            (0xd50, _) => "My Lord of the Lost Lands will make short work of you!".into(),
            (0xd61, _) => {
                "My Hermit God's thousand tentacles shall drag you to a watery grave!".into()
            }
            (0xe37, _) => "My Ghost Ship will send you to a watery grave.".into(),
            _ => unreachable!("unsupported Oryx announcement family {kind:#x}"),
        };
        Some((kind, count, text))
    }

    pub(super) fn oryx_notice(&self, text: &str) {
        self.broadcast_text("#Oryx the Mad God", text);
    }
}

#[cfg(test)]
#[tokio::test]
async fn realm_death_credit_survives_portal_reward_failure() {
    let data = std::path::Path::new("data");
    let content = Arc::new(Content::load(data).unwrap());
    let store = crate::store::Store::open(std::path::Path::new(":memory:"), &content).unwrap();
    let account = store.list("closure-error-witness".into()).await.unwrap();
    let character = store
        .create(account.id, content.get(crate::content::WIZARD).unwrap())
        .await
        .unwrap();
    let mut directory = Directory::prepared(data, content.clone(), 16, 12351, 2).unwrap();
    directory.grow_realm_for_test();
    let mut map = Map {
        width: 40,
        height: 40,
        cells: vec![
            crate::content::Cell {
                ground: 0xbc,
                object: None,
                config: String::new(),
                terrain: 0,
                region: 0,
            };
            1600
        ],
        spawns: vec![(14.5, 20.5)],
    };
    map.cells[20 * 40 + 14].ground = 0x46;
    let mut world = World::with_ids(content, Arc::new(map), directory.ids.clone()).unwrap();
    world.name = "Realm".into();
    world.encounter = true;
    let root = world
        .insert_entity(
            crate::dungeon::trench::HERMIT,
            Pos { x: 20.5, y: 20.5 },
            vec![],
        )
        .unwrap();
    world
        .entities
        .get_mut(&root)
        .unwrap()
        .quest
        .set(Some(root), root, &mut world.quest_ids);
    let (packets, mut inbox) = mpsc::channel(64);
    let (lost, _) = watch::channel(None);
    let player = world
        .enter(character.clone(), Some(Output { packets, lost }))
        .unwrap();
    while inbox.try_recv().is_ok() {}
    directory.worlds.insert(-3, world);
    let portal = directory.endgame.realms.remove(&-3).unwrap().portal;
    directory
        .bind_realm_heroes(-3, BTreeSet::from([root]))
        .unwrap();
    directory.endgame.realms.get_mut(&-3).unwrap().portal = portal;
    let world = directory.worlds.get_mut(&-3).unwrap();
    let error = world.defeat_entity(root).unwrap_err();
    assert_eq!(error.to_string(), "Hermit has no reachable reward site");
    assert!(!world.entities.contains_key(&root));
    assert!(world.portal_requests.is_empty());
    directory.endgame_step(&[-3]);
    directory.endgame_step(&[-3]);
    let closed = chat::text_packet(
        "#Oryx the Mad God",
        -1,
        0,
        "I HAVE CLOSED THIS REALM! YOU WILL NOT LIVE TO SEE THE LIGHT OF DAY!",
    );
    assert_eq!(
        std::iter::from_fn(|| inbox.try_recv().ok())
            .filter(|p| p.id == closed.id && p.body == closed.body)
            .count(),
        1
    );
    assert!(!directory.worlds[&-3].admission_open);
    assert!(directory.endgame.realms[&-3].cycle.remaining.is_empty());
    assert!(directory.issue(account.id, character.id, -3).is_err());
    directory.tick = 2399;
    directory.endgame_step(&[-3]);
    assert!(directory.forced_transfer(player).unwrap().is_none());
    assert!(matches!(
        inbox.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    directory.tick = 2400;
    directory.endgame_step(&[-3]);
    let notices: Vec<_> = std::iter::from_fn(|| inbox.try_recv().ok()).collect();
    assert_eq!(notices.iter().filter(|p| p.id == wire::TEXT).count(), 3);
    let effects: Vec<_> = notices
        .iter()
        .filter(|p| p.id == wire::SHOW_EFFECT)
        .collect();
    assert_eq!(effects.len(), 1);
    let mut effect = wire::Reader::new(&effects[0].body);
    assert_eq!(effect.u8().unwrap(), 14, "selected-client camera jitter");
    assert_eq!(effect.i32().unwrap(), -1);
    for _ in 0..4 {
        assert_eq!(effect.f32().unwrap(), 0.);
    }
    assert_eq!(effect.i32().unwrap(), -1);
    effect.done().unwrap();
    assert!(directory.forced_transfer(player).unwrap().is_none());
    directory.endgame_step(&[-3]);
    assert!(matches!(
        inbox.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    directory.tick = 2559;
    directory.endgame_step(&[-3]);
    assert!(matches!(
        inbox.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    assert!(directory.endgame.realms[&-3].castle.is_none());
    let other_portal = directory.endgame.realms[&1].portal.unwrap();
    let other_pos = directory.worlds[&-2].entities[&other_portal].pos;
    directory.tick = 2560;
    for _ in 0..1000 {
        directory.endgame_step(&[-3]);
        if directory.endgame.realms[&-3].castle.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let record = &directory.endgame.realms[&-3];
    let castle = record
        .castle
        .expect("normal replacement construction completed");
    let replacement = record.replacement.unwrap();
    let entrance =
        &directory.worlds[&-2].entities[&directory.endgame.realms[&replacement].portal.unwrap()];
    assert!(entrance.pos.distance(other_pos) >= 4.);
    assert_eq!(
        directory.worlds[&-2].entities[&other_portal]
            .pos
            .distance(other_pos),
        0.
    );
    assert!(!directory.worlds[&replacement].admission_open);
    assert!(directory.forced_transfer(player).unwrap().is_none());
    let (owner, progress) = directory
        .worlds
        .get_mut(&-3)
        .unwrap()
        .pending_progress_where(|id, _| id == player)
        .pop()
        .expect("final Hermit XP must settle before transfer");
    store.progress(progress.clone()).await.unwrap();
    directory
        .worlds
        .get_mut(&-3)
        .unwrap()
        .finish_progress(owner, progress)
        .unwrap();
    let character = directory.worlds[&-3].players[&player].character.clone();
    let transfer = directory.forced_transfer(player).unwrap().unwrap();
    assert_eq!(transfer.destination, Some(castle));
    assert_eq!(transfer.name, "Oryx's Castle");
    assert_eq!(transfer.packet().id, 39);
    assert!(directory.forced_transfer(player).unwrap().is_none());
    assert!(
        directory
            .consume(account.id, castle, transfer.key_time, transfer.key.clone())
            .is_err()
    );
    let mut reservation = directory.reserve_transfer(transfer.key.clone()).unwrap();
    let snapshot = directory
        .begin_source_departure(player, Some(&transfer.key))
        .unwrap()
        .unwrap();
    directory
        .accept_source_write(player, &transfer.key)
        .unwrap();
    let persisted = store.progress(snapshot.clone()).await;
    directory
        .worlds
        .get_mut(&-3)
        .unwrap()
        .committed(player, persisted)
        .unwrap();
    directory
        .finish_source_departure(player, snapshot, Some(transfer.key.clone()))
        .unwrap();
    reservation.disarm();
    directory.realm_departed(-3, player);
    let (id, arrival) = directory
        .consume(account.id, castle, transfer.key_time, transfer.key.clone())
        .unwrap();
    assert_eq!(id, character.id);
    directory
        .admit_transfer(arrival, account.id, id, castle)
        .unwrap();
    let saved = store.load(account.id, id).await.unwrap();
    assert_eq!(saved, character);
    let arrived = directory
        .worlds
        .get_mut(&castle)
        .unwrap()
        .enter_admitted(
            saved,
            Some(transfer.output),
            None,
            store.presentation(account.id).await.unwrap(),
            store.is_legend(account.id).await.unwrap(),
            Some(transfer.key.clone()),
            Controller::Human,
        )
        .unwrap();
    let current = &directory.worlds[&castle].players[&arrived].character;
    assert!(
        current.tiles > character.tiles,
        "Castle entry discovers new tiles"
    );
    let mut expected = character;
    expected.lifetime.tiles += current.tiles - expected.tiles;
    expected.tiles = current.tiles;
    assert_eq!(*current, expected);
    assert!(
        directory
            .consume(account.id, castle, transfer.key_time, transfer.key)
            .is_err()
    );
    directory.tick = 2599;
    directory.endgame_step(&[castle]);
    assert!(!directory.worlds[&replacement].admission_open);
    directory.tick = 2600;
    directory.endgame_step(&[castle]);
    assert!(directory.worlds[&replacement].admission_open);
}

#[cfg(test)]
#[test]
fn prepare_realm_closure_preserves_other_cycle_and_refuses_reservations() {
    let path = std::path::Path::new("data");
    let content = Arc::new(Content::load(path).unwrap());
    let mut d = Directory::prepared(path, content, 16, 12351, 2).unwrap();
    d.clock.set_paused(true);
    d.grow_realm_for_test();
    let other = d.endgame.realms[&1].cycle.remaining.clone();
    let other_spawns = d.worlds[&1].map.spawns.clone();
    d.issue(7, 7, -3).unwrap();
    assert!(
        d.prepare_realm_closure(-3)
            .unwrap_err()
            .to_string()
            .contains("reservations")
    );
    d.tick += 2400;
    d.retire_transfer_grants();
    let (hero, position) = d.prepare_realm_closure(-3).unwrap();
    assert_eq!(d.worlds[&-3].map.spawns, vec![(position.x, position.y)]);
    assert_eq!(d.worlds[&1].map.spawns, other_spawns);
    assert_eq!(
        d.endgame.realms[&-3].cycle.remaining,
        BTreeSet::from([hero])
    );
    assert!(d.endgame.realms[&-3].cycle.admits() && d.worlds[&-3].admission_open);
    assert_eq!(d.endgame.realms[&1].cycle.remaining, other);
    assert!(d.endgame.realms[&1].cycle.admits() && d.worlds[&1].admission_open);
}
