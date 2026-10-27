use super::*;
use crate::endgame::{CASTLE, CELLAR, HEROES, INCANTATION, LOCKED_CELLAR, RealmCycle, RealmStage};
use rand::seq::IteratorRandom;

// Gameplay documentation specifies two minutes, unlike modified forks' 20s: https://wikiwiki.jp/rmd/イベント#jded12e1
pub(super) const REMAINING_TAUNT_MS: u64 = 120_000;

#[derive(Default)]
pub(super) struct RealmDirectory {
    pub realms: BTreeMap<i32, RealmRecord>,
    pub(super) forced: BTreeMap<i32, ForcedRoute>,
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
        if locked.is_none() {
            let name = w.players[&player].character.name.clone();
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
                    let quake = cooperative::effect(13, -1, Pos { x: 0., y: 0. });
                    for p in world.players.values() {
                        send(p, wire::SHOW_EFFECT, quake.clone());
                    }
                }
            }
            // Ordinary maintenance must not retain this loan while waiting for another Realm.
            if cycle.stage != RealmStage::Transferring {
                self.worlds.release_world(origin);
            }
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
            .filter(|(id, record)| {
                record.replacement.is_some()
                    && record.publish_at.is_none()
                    && record.pending_transfers.is_empty()
                    && self
                        .worlds
                        .summary(id)
                        .expect("retiring Realm metadata missing")
                        .players
                        .is_empty()
                    && !self
                        .grants
                        .values()
                        .any(|g| g.origin == Some(**id) || g.destination == **id)
                    && !self
                        .endgame
                        .forced
                        .values()
                        .any(|route| route.origin == **id)
            })
            .map(|(id, _)| *id)
            .collect();
        for origin in retired {
            self.portals.retain(|_, p| p.origin != origin);
            if let Some(portal) = self.endgame.realms[&origin].portal {
                self.worlds.get_mut(&-2).unwrap().remove_entity(portal);
            }
            self.worlds.remove(&origin);
            self.idle.remove(&origin);
            self.endgame.realms.remove(&origin);
            tracing::info!(origin, "empty retired Realm removed");
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
        self.worlds.get(&origin).context("closing Realm missing")?;
        let castle = self.make(CASTLE, "Oryx's Castle", false, None)?;
        let realm = self.make("realm-shore", "Realm", false, None)?;
        let next_cycle = RealmCycle::new(realm.hero_identities())?;
        let name = (1..=super::realm_portal::NAMES.len())
            .map(|offset| {
                (self.endgame.realms[&origin].name + offset) % super::realm_portal::NAMES.len()
            })
            .find(|name| self.endgame.realms.values().all(|r| r.name != *name))
            .context("no unused Realm name")?;
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
        let portal = nexus.spawn_entity(0x712, entrance.pos, vec![])?;
        nexus.entities.get_mut(&portal).unwrap().published = false;
        let mut realm = realm;
        realm.admission_open = false;
        self.worlds.insert(castle_id, castle);
        self.worlds.insert(realm_id, realm);
        self.next_world = next;
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

    #[cfg(test)]
    pub fn forced_transfer_candidates(&self) -> Vec<i32> {
        self.endgame.forced.keys().copied().collect()
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
    pub(super) fn populate_realm_heroes(&mut self) -> Result<()> {
        // GPL-3.0 requested counts, not authenticated 123.5.1 density (data/REALM-SRC-LICENSE):
        // https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/realm/setpieces/SetPieces.cs
        // Unlike its 50-attempt skips, our protected allocator fulfills requests or errors.
        for (kind, terrains, range) in [(HEROES[0], &[1][..], 3..5), (HEROES[1], &[3, 4][..], 4..7)]
        {
            let count = self.rng.gen_range(range);
            self.populate_landmark_heroes(kind, terrains, count)?;
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
    world
        .enter(character, Some(Output { packets, lost }))
        .unwrap();
    while inbox.try_recv().is_ok() {}
    directory.worlds.insert(-3, world);
    directory.endgame.realms.remove(&-3);
    directory
        .bind_realm_heroes(-3, BTreeSet::from([root]))
        .unwrap();
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
}

#[cfg(test)]
#[test]
fn castle_ground_only_writes_preserve_native_scenery() {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let mut witnessed = BTreeSet::new();
    let (mut bridges, mut walls) = (0, 0);
    for seed in 0..16 {
        let original = Arc::new(Map {
            width: 192,
            height: 192,
            cells: vec![
                crate::content::Cell {
                    ground: 0x47,
                    object: Some(0x105),
                    config: "name:native;size:137".into(),
                    terrain: 3,
                    region: 0,
                };
                192 * 192
            ],
            spawns: vec![(1., 1.)],
        });
        let mut world = World::new(content.clone(), original.clone()).unwrap();
        let native_ids: Vec<_> = (0..192 * 192).map(|i| world.cells.ids(i)[0]).collect();
        let template = match &world.entities[&native_ids[0]].state {
            entity_store::EntityState::Shared(t) => t.clone(),
            _ => panic!("native Bush does not use shared scenery state"),
        };
        world.rng = rand::rngs::StdRng::seed_from_u64(seed);
        world.populate_landmark_heroes(0x67d, &[3], 1).unwrap();
        let chest = world.entities.values().find(|e| e.kind == 0x501).unwrap();
        let root = world.entities.values().find(|e| e.kind == 0x67d).unwrap();
        assert_eq!(world.setpieces.len(), 1);
        assert_eq!(
            world.entities.values().filter(|e| e.kind == 0x501).count(),
            1
        );
        assert!(chest.published && chest.items.len() == 8 && world.walkable(chest.pos));
        assert_eq!((chest.pos.x.fract(), chest.pos.y.fract()), (0.5, 0.5));
        assert_eq!((root.pos.x.fract(), root.pos.y.fract()), (0., 0.));
        assert!(root.pos.distance(chest.pos) > 6.);
        let assert_native = |pos: Pos, ground: u8, added: Option<i32>| {
            let index = pos.y as usize * 192 + pos.x as usize;
            let actual = &world.map.cells[index];
            let before = &original.cells[index];
            assert_eq!(
                (
                    actual.ground,
                    actual.object,
                    &actual.config,
                    actual.terrain,
                    actual.region
                ),
                (
                    ground,
                    before.object,
                    &before.config,
                    before.terrain,
                    before.region
                ),
                "seed {seed}"
            );
            let native = &world.entities[&native_ids[index]];
            assert_eq!(
                (native.kind, native.name.as_str(), native.size),
                (0x105, "native", 137)
            );
            assert_eq!((native.pos.x, native.pos.y), (pos.x, pos.y));
            assert!(
                matches!(&native.state, entity_store::EntityState::Shared(t) if Arc::ptr_eq(t, &template))
            );
            assert_eq!(
                world.cells.ids(index).len(),
                1 + usize::from(added.is_some())
            );
            assert!(world.cells.ids(index).contains(&native.id));
            if let Some(id) = added {
                assert!(world.cells.ids(index).contains(&id));
            }
        };
        assert_native(chest.pos, 0x47, Some(chest.id));
        let wall_sites: Vec<_> = world
            .entities
            .values()
            .filter(|e| e.kind == 0x122)
            .map(|e| (e.id, e.pos))
            .collect();
        for &(id, pos) in &wall_sites {
            assert_native(pos, 0x60, Some(id));
            assert!(
                world.entities[&id].hp > 0 && world.entities[&id].published && !world.walkable(pos)
            );
            walls += 1;
        }
        for (i, _) in world
            .map
            .cells
            .iter()
            .enumerate()
            .filter(|(_, c)| c.ground == 0xcb)
        {
            let pos = Pos {
                x: (i % 192) as f32 + 0.5,
                y: (i / 192) as f32 + 0.5,
            };
            assert_native(pos, 0xcb, None);
            assert!(world.walkable(pos));
            bridges += 1;
        }
        assert_native(
            Pos {
                x: root.pos.x + 0.5,
                y: root.pos.y + 0.5,
            },
            0x47,
            Some(root.id),
        );
        let area = world.setpieces[0];
        witnessed.insert((
            area.width,
            area.height,
            ((chest.pos.x - area.x as f32) * 2.) as i32,
            ((chest.pos.y - area.y as f32) * 2.) as i32,
        ));
        if let Some((wall, pos)) = wall_sites.first().copied() {
            let index = pos.y as usize * 192 + pos.x as usize;
            world.damage_enemy(-1, wall, 1000, true, &[], None).unwrap();
            assert!(!world.entities.contains_key(&wall) && world.walkable(pos));
            assert_eq!(world.cells.ids(index), &[native_ids[index]]);
            let cell = &world.map.cells[index];
            assert_eq!(
                (cell.ground, cell.object, cell.config.as_str()),
                (0x60, Some(0x105), "name:native;size:137")
            );
            assert!(
                matches!(&world.entities[&native_ids[index]].state, entity_store::EntityState::Shared(t) if Arc::ptr_eq(t, &template))
            );
        }
    }
    assert!(bridges > 0 && walls > 0);
    assert_eq!(
        witnessed,
        BTreeSet::from([
            (31, 40, 31, 55),
            (40, 31, 25, 31),
            (31, 40, 31, 25),
            (40, 31, 55, 31)
        ])
    );
}

#[cfg(test)]
#[test]
fn landmark_hero_boundaries_fit_published_layouts() {
    let data = std::path::Path::new("data");
    let content = Arc::new(Content::load(data).unwrap());
    let mut cyclops_anchors = BTreeSet::new();
    for source in crate::terrain::realms::sources(data).unwrap() {
        for counts in [[3, 4], [4, 6]] {
            let birth_map = source.open(&content).unwrap();
            let mut world = World::new(content.clone(), birth_map.clone()).unwrap();
            world.realm_layout = Some(source.name().to_owned());
            world.rng = rand::rngs::StdRng::seed_from_u64(12351);
            for (kind, terrains, count) in [
                (HEROES[0], &[1][..], counts[0]),
                (HEROES[1], &[3, 4][..], counts[1]),
            ] {
                world
                    .populate_landmark_heroes(kind, terrains, count)
                    .unwrap();
                assert_eq!(
                    world.entities.values().filter(|e| e.kind == kind).count(),
                    count,
                    "{}: {kind:#x}",
                    source.name()
                );
            }
            let ids = world.hero_identities();
            assert_eq!(ids.len(), counts.iter().sum::<usize>());
            for id in &ids {
                let e = &world.entities[id];
                assert!(e.published && e.hp > 0 && e.quest.is_none() && e.camp.is_none());
                if e.kind == HEROES[1] {
                    let area = world
                        .setpieces
                        .iter()
                        .find(|a| a.distance(e.pos) == 0.)
                        .unwrap();
                    let chest = world
                        .entities
                        .values()
                        .find(|c| c.kind == 0x501 && area.distance(c.pos) == 0.)
                        .unwrap();
                    assert_eq!((chest.pos.x.fract(), chest.pos.y.fract()), (0.5, 0.5));
                    assert!(world.walkable(chest.pos));
                    for pos in [chest.pos, e.pos] {
                        let index = pos.y as usize * world.map.width as usize + pos.x as usize;
                        let original = &birth_map.cells[index];
                        let current = &world.map.cells[index];
                        assert_eq!(
                            (
                                current.ground,
                                current.object,
                                &current.config,
                                current.terrain,
                                current.region
                            ),
                            (
                                original.ground,
                                original.object,
                                &original.config,
                                original.terrain,
                                original.region
                            )
                        );
                    }
                    let local = match (
                        area.width,
                        area.height,
                        ((chest.pos.x - area.x as f32) * 2.) as i32,
                        ((chest.pos.y - area.y as f32) * 2.) as i32,
                    ) {
                        (31, 40, 31, 55) => (15, 20),
                        (40, 31, 25, 31) => (19, 15),
                        (31, 40, 31, 25) => (15, 19),
                        (40, 31, 55, 31) => (20, 15),
                        other => panic!("unexpected rotated Castle/chest {other:?}"),
                    };
                    let expected = ((area.x + local.0) as f32, (area.y + local.1) as f32);
                    assert_eq!((e.pos.x, e.pos.y), expected, "{}: Cyclops", source.name());
                    assert_eq!((e.home.x, e.home.y), expected);
                    assert!(
                        world.walkable(e.pos)
                            && matches!(world.map.cell(e.pos.x, e.pos.y).unwrap().terrain, 3 | 4)
                    );
                    let index = e.pos.y as usize * world.map.width as usize + e.pos.x as usize;
                    assert!(world.cells.ids(index).contains(id));
                    assert!(e.pos.distance(chest.pos) > 6. && chest.items.len() == 8);
                    cyclops_anchors.insert(local);
                }
                assert!(
                    ids.iter()
                        .filter(|other| *other != id)
                        .all(|other| e.pos.distance(world.entities[other].pos) >= 32.)
                );
            }
            let mut cycle = RealmCycle::new(ids.clone()).unwrap();
            for id in ids {
                assert!(cycle.defeated(id, 1000));
                assert!(!cycle.defeated(id, 1000));
            }
            assert!(
                matches!(cycle.stage, RealmStage::Closing { deadline } if deadline == 1000 + crate::endgame::CLOSE_DELAY_MS)
            );
        }
    }
    assert_eq!(
        cyclops_anchors,
        BTreeSet::from([(15, 20), (19, 15), (15, 19), (20, 15)])
    );
}
