use super::*;

// Population dependence is period evidence; these thresholds/dwells are selected reconstruction:
// https://web.archive.org/web/20120602011655/http://forums.wildshadow.com:80/wiki/realm-mad-god/nexus
const GROW_MS: u64 = 30_000;
const RETIRE_MS: u64 = 300_000;

pub(super) struct DemandPreparation {
    realm: i32,
    result: tokio::sync::oneshot::Receiver<Result<World>>,
    canceled: bool,
    closure_wait: Option<u64>,
}

impl Directory {
    fn lineage_count(&self) -> usize {
        self.endgame
            .realms
            .values()
            .filter(|r| r.replacement.is_none())
            .count()
            + usize::from(matches!(
                self.endgame.preparing,
                Some(Preparation::Demand(_))
            ))
    }
    fn open_realms(&self) -> Vec<(i32, Arc<execution::Snapshot>)> {
        self.endgame
            .realms
            .iter()
            .filter(|(_, r)| r.enabled && r.cycle.admits())
            .filter_map(|(id, _)| {
                let w = self.worlds.summary(id).expect("Realm metadata missing");
                w.admission_open.then_some((*id, w))
            })
            .collect()
    }
    fn realm_pressure(&self) -> bool {
        let open = self.open_realms();
        !open.is_empty()
            && open
                .iter()
                .all(|(_, w)| w.occupancy >= (w.max_players * 4).div_ceil(5))
    }
    fn closure_needs_builder(&self) -> Option<i32> {
        self.endgame
            .realms
            .iter()
            .find(|(_, r)| r.cycle.stage == RealmStage::Transferring && r.replacement.is_none())
            .map(|(id, _)| *id)
    }
    pub(in crate::world) fn realm_allocation_step(&mut self) {
        if let Err(error) = self.allocate_realm_step() {
            tracing::error!(%error, "demand Realm construction failed; no entrance published");
        }
        let now = self.tick * 50;
        let open = self.open_realms();
        let candidates: Vec<_> = self
            .endgame
            .realms
            .keys()
            .copied()
            .filter(|id| *id > 0)
            .collect();
        for id in candidates.into_iter().rev() {
            let r = &self.endgame.realms[&id];
            let eligible = self.lineage_count() > 1
                && r.enabled
                && r.cycle.admits()
                && r.replacement.is_none()
                && r.castle.is_none()
                && r.pending_transfers.is_empty()
                && r.publish_at.is_none()
                && open.iter().any(|(other, _)| *other != id)
                && open
                    .iter()
                    .filter(|(other, _)| *other != id)
                    .all(|(_, w)| w.occupancy <= w.max_players * 3 / 5)
                && self
                    .worlds
                    .summary(&id)
                    .expect("Realm metadata missing")
                    .admission_open
                && self.realm_dependencies_clear(id);
            if !eligible {
                self.endgame.empty_since.remove(&id);
                continue;
            }
            let since = *self.endgame.empty_since.entry(id).or_insert(now);
            if now - since >= RETIRE_MS {
                // The guard loans the actual World, not just its last actor snapshot.
                if self.realm_dependencies_clear(id) {
                    self.remove_empty_realm(id);
                    break;
                }
            }
        }
    }
    fn allocate_realm_step(&mut self) -> Result<()> {
        let now = self.tick * 50;
        let pressure = self.realm_pressure();
        let closure = self.closure_needs_builder();
        if let Some(Preparation::Demand(build)) = self.endgame.preparing.as_mut() {
            build.canceled |= !pressure || closure.is_some();
            if let Some(origin) = closure
                && build.closure_wait.is_none()
            {
                build.closure_wait = Some(now);
                tracing::info!(
                    origin,
                    demand_realm = build.realm,
                    job = "realm_builder",
                    "closure waiting for canceled in-flight demand constructor; phase clocks continue"
                );
            }
            let result = match build.result.try_recv() {
                Ok(result) => result,
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => return Ok(()),
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.endgame.preparing.take();
                    bail!("demand Realm preparation worker stopped without returning its result");
                }
            };
            let Some(Preparation::Demand(build)) = self.endgame.preparing.take() else {
                unreachable!()
            };
            self.endgame.demand_since = None;
            tracing::info!(realm = build.realm, canceled = build.canceled,
                closure_wait_ms = build.closure_wait.map(|since| now - since),
                error = ?result.as_ref().err().map(|e| format!("{e:#}")), "demand constructor completed");
            let mut world = result.context("demand Realm constructor")?;
            if build.canceled || !pressure || closure.is_some() {
                tracing::info!(
                    realm = build.realm,
                    lineages = self.lineage_count(),
                    physical_realms = self.endgame.realms.len(),
                    "unexposed never-admitted demand Realm discarded"
                );
                return Ok(());
            }
            world.admission_open = false;
            let cycle = RealmCycle::new(world.hero_identities())?;
            let previous = self
                .endgame
                .realms
                .iter()
                .next_back()
                .context("no maintained Realm")?
                .1
                .name;
            let name = (1..=super::super::realm_portal::NAMES.len())
                .map(|offset| (previous + offset) % super::super::realm_portal::NAMES.len())
                .find(|name| self.endgame.realms.values().all(|r| r.name != *name))
                .context("no unused Realm name for demand allocation")?;
            self.worlds.insert(build.realm, world);
            let portal = match self.install_realm_portal(build.realm, None) {
                Ok(portal) => portal,
                Err(error) => {
                    self.worlds.remove(&build.realm);
                    return Err(error.context("demand Realm entrance allocation"));
                }
            };
            let mut record = RealmRecord::new(cycle, now);
            record.name = name;
            record.portal = Some(portal);
            self.endgame.realms.insert(build.realm, record);
            self.worlds.get_mut(&build.realm).unwrap().admission_open = true;
            self.worlds.release_world(&build.realm);
            tracing::info!(
                realm = build.realm,
                portal,
                name,
                lineages = self.lineage_count(),
                physical_realms = self.endgame.realms.len(),
                "sustained Realm pressure published one demand entrance"
            );
            return Ok(());
        }
        if !pressure
            || closure.is_some()
            || self.lineage_count() >= self.realm_limit
            || self.endgame.preparing.is_some()
        {
            self.endgame.demand_since = None;
            return Ok(());
        }
        let since = *self.endgame.demand_since.get_or_insert(now);
        if now - since < GROW_MS {
            return Ok(());
        }
        self.endgame.demand_since = None;
        let realm = self.next_world;
        self.next_world = realm.checked_add(1).context("world identity exhausted")?;
        ensure!(
            !self.worlds.contains_key(&realm),
            "demand world identity collision"
        );
        let construct = self.construction("realm-shore", "Realm", false, None);
        let result = self.worlds.prepare_realm(realm, self.tick, construct)?;
        self.endgame.preparing = Some(Preparation::Demand(DemandPreparation {
            realm,
            result,
            canceled: false,
            closure_wait: None,
        }));
        tracing::info!(realm, job = "realm_builder", lineages = self.lineage_count(), physical_realms = self.endgame.realms.len(), occupancy = ?self.open_realms().iter().map(|(id,w)| (*id,w.occupancy,w.max_players)).collect::<Vec<_>>(), "sustained Realm pressure started one bounded demand build");
        Ok(())
    }
    pub(super) fn realm_dependencies_clear(&self, id: i32) -> bool {
        if self.grants.values().any(|g| g.origin == Some(id) || g.destination == id
            || g.entry_witness.as_ref().is_some_and(|w| w.source == id)
            || g.exit.as_ref().is_some_and(|exit| exit.source == id))
            || self.portals.values().any(|p| p.origin == id || p.destination == Some(id))
            || self.worlds.summaries().any(|(_, w)| matches!(w.return_route, Some(DungeonReturn::Realm(origin)) if origin == id))
            || self.endgame.forced.values().any(|r| r.origin == id || r.destination == id)
            || self.endgame.realms.iter().any(|(other, r)| *other != id && (r.replacement == Some(id) || r.castle == Some(id)))
            || self.endgame.preparing.as_ref().is_some_and(|p| match p {
                Preparation::Replacement(p) => [p.origin, p.castle, p.realm].contains(&id),
                Preparation::Demand(p) => p.realm == id,
            }) { return false; }
        let snapshot = self
            .worlds
            .summary(&id)
            .expect("retiring Realm metadata missing");
        if !snapshot.players.is_empty()
            || !snapshot.admissions_empty
            || !snapshot.bags_empty
            || snapshot.critical
            || snapshot.portals_pending
        {
            return false;
        }
        let w = self.worlds.get(&id).expect("retiring Realm missing");
        w.players.is_empty()
            && w.admissions.is_empty()
            && w.bag_expiry.is_empty()
            && w.portal_requests.is_empty()
            && w.critical_deaths.is_empty()
            && w.critical_killers.is_empty()
            && w.entities.values().all(|e| !e.reserved)
            && w.trades.is_empty()
            && w.trade_requests.is_empty()
    }
    pub(super) fn remove_empty_realm(&mut self, origin: i32) {
        if let Some(portal) = self.endgame.realms[&origin].portal {
            self.worlds
                .get_mut(&-2)
                .expect("Nexus missing")
                .remove_entity(portal);
        }
        self.worlds.remove(&origin);
        self.idle.remove(&origin);
        self.endgame.realms.remove(&origin);
        self.endgame.empty_since.remove(&origin);
        tracing::info!(
            origin,
            lineages = self.lineage_count(),
            physical_realms = self.endgame.realms.len(),
            "dependency-free empty Realm removed through owned lifecycle"
        );
    }
}

#[cfg(test)]
impl Directory {
    pub(crate) fn grow_realm_for_test(&mut self) {
        let tick = self.tick;
        let capacity = self.worlds[&-3].max_players;
        self.worlds.get_mut(&-3).unwrap().max_players = 1;
        let (_, key, _) = self.issue(900, 900, -3).unwrap();
        self.allocate_realm_step().unwrap();
        self.tick += GROW_MS / 50;
        self.allocate_realm_step().unwrap();
        self.allocate_realm_step().unwrap();
        self.grants.remove(&key);
        self.worlds.get_mut(&-3).unwrap().admissions.remove(&key);
        self.worlds.get_mut(&-3).unwrap().max_players = capacity;
        self.tick = tick;
        self.sync_realm_portal();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn directory(capacity: usize) -> Directory {
        let path = std::path::Path::new("data");
        Directory::test_with_seed(
            path,
            Arc::new(Content::load(path).unwrap()),
            capacity,
            12351,
        )
        .unwrap()
    }
    fn resident(d: &mut Directory, world: i32, account: i32) -> i32 {
        let mut ch = Character::fresh(d.content.get(0x30e).unwrap()).unwrap();
        ch.account = account;
        ch.id = account;
        d.worlds.get_mut(&world).unwrap().enter(ch, None).unwrap()
    }
    #[test]
    fn realm_allocation_boundary_reset_reservations_spare_and_ceiling() {
        let mut d = directory(72);
        assert_eq!(d.lineage_count(), 1);
        let mut players = Vec::new();
        for account in 1..=57 {
            players.push(resident(&mut d, -3, account));
        }
        d.allocate_realm_step().unwrap();
        assert!(d.endgame.demand_since.is_none());
        let (_, key, _) = d.issue(58, 58, -3).unwrap();
        assert!(d.realm_pressure());
        d.allocate_realm_step().unwrap();
        d.tick = 599;
        d.allocate_realm_step().unwrap();
        assert!(d.endgame.preparing.is_none());
        d.grants.remove(&key);
        d.worlds.get_mut(&-3).unwrap().admissions.remove(&key);
        d.allocate_realm_step().unwrap();
        assert!(d.endgame.demand_since.is_none());
        resident(&mut d, -3, 58);
        d.allocate_realm_step().unwrap();
        d.tick += 599;
        d.allocate_realm_step().unwrap();
        assert_eq!(d.lineage_count(), 1);
        d.tick += 1;
        d.allocate_realm_step().unwrap();
        assert_eq!(d.lineage_count(), 2, "build consumes its slot immediately");
        d.allocate_realm_step().unwrap();
        assert_eq!(d.endgame.realms.len(), 2);
        assert!(
            !d.realm_pressure(),
            "spare suppresses growth even when startup is crowded"
        );
        d.realm_limit = 3;
        for account in 100..158 {
            resident(&mut d, 1, account);
        }
        d.allocate_realm_step().unwrap();
        d.tick += 600;
        d.allocate_realm_step().unwrap();
        d.allocate_realm_step().unwrap();
        assert_eq!(d.lineage_count(), 3);
        d.tick += 600;
        d.allocate_realm_step().unwrap();
        assert!(d.endgame.preparing.is_none());
        assert_eq!(d.lineage_count(), 3);
    }
    #[test]
    fn realm_retirement_dwell_hysteresis_reset_and_owned_routing() {
        let mut d = directory(72);
        d.grow_realm_for_test();
        for account in 1..=44 {
            resident(&mut d, -3, account);
        }
        d.realm_allocation_step();
        assert!(
            d.endgame.empty_since.is_empty(),
            "44 is above floor60% of72"
        );
        let player = *d.worlds[&-3].players.keys().next().unwrap();
        d.worlds.get_mut(&-3).unwrap().players.remove(&player);
        d.realm_allocation_step();
        d.tick = 5999;
        d.realm_allocation_step();
        assert!(d.worlds.contains_key(&1));
        let child = d
            .instance("snake-pit", None, Some(DungeonReturn::Realm(1)))
            .unwrap();
        d.realm_allocation_step();
        assert!(d.endgame.empty_since.is_empty());
        d.tick = 6000;
        d.worlds.remove(&child);
        d.realm_allocation_step();
        d.tick = 11999;
        d.realm_allocation_step();
        assert!(d.worlds.contains_key(&1));
        d.tick = 12000;
        d.realm_allocation_step();
        assert!(!d.worlds.contains_key(&1));
        assert!(d.worlds.routing.resolve(Target::World(1)).is_err());
        assert_eq!(d.lineage_count(), 1);
        d.tick += 6000;
        d.realm_allocation_step();
        assert!(d.worlds.contains_key(&-3));
    }
    #[test]
    fn realm_retirement_dependency_pins_and_terminal_origin_cleanup() {
        let mut d = directory(72);
        d.grow_realm_for_test();
        assert!(d.realm_dependencies_clear(1));
        let player = resident(&mut d, 1, 10);
        d.worlds
            .get_mut(&1)
            .unwrap()
            .players
            .get_mut(&player)
            .unwrap()
            .death_pending = true;
        assert!(
            !d.realm_dependencies_clear(1),
            "disconnected death-pending resident pins world"
        );
        d.worlds.get_mut(&1).unwrap().players.remove(&player);
        let (_, key, _) = d.issue(11, 11, -2).unwrap();
        d.grants.get_mut(&key).unwrap().origin = Some(1);
        assert!(
            !d.realm_dependencies_clear(1),
            "grant origin pins even when destination elsewhere"
        );
        d.grants.remove(&key);
        d.worlds.get_mut(&-2).unwrap().admissions.remove(&key);
        let (_, key, _) = d.issue(11, 11, 1).unwrap();
        d.grants.remove(&key);
        assert!(
            !d.realm_dependencies_clear(1),
            "independent reservation pins"
        );
        d.worlds.get_mut(&1).unwrap().admissions.remove(&key);
        let child = d
            .instance("snake-pit", None, Some(DungeonReturn::Realm(1)))
            .unwrap();
        let nested = d
            .instance(
                "spider-den",
                None,
                Some(d.dungeon_return_from(child).unwrap()),
            )
            .unwrap();
        d.worlds.remove(&child);
        assert!(
            !d.realm_dependencies_clear(1),
            "nested extant return survives expired parent entrance"
        );
        d.worlds.remove(&nested);
        d.worlds.get_mut(&1).unwrap().bag_expiry.insert(123, 99999);
        assert!(!d.realm_dependencies_clear(1));
        d.worlds.get_mut(&1).unwrap().bag_expiry.clear();
        d.worlds
            .get_mut(&1)
            .unwrap()
            .portal_requests
            .push((0x703, Pos { x: 1., y: 1. }));
        assert!(!d.realm_dependencies_clear(1));
        d.worlds.get_mut(&1).unwrap().portal_requests.clear();
        d.worlds.get_mut(&1).unwrap().critical_deaths.push(123);
        assert!(!d.realm_dependencies_clear(1));
        d.worlds.get_mut(&1).unwrap().critical_deaths.clear();
        let entity = *d.worlds[&1].entities.keys().next().unwrap();
        d.worlds
            .get_mut(&1)
            .unwrap()
            .entities
            .get_mut(&entity)
            .unwrap()
            .reserved = true;
        assert!(!d.realm_dependencies_clear(1));
        d.worlds
            .get_mut(&1)
            .unwrap()
            .entities
            .get_mut(&entity)
            .unwrap()
            .reserved = false;
        d.endgame.forced.insert(
            11,
            ForcedRoute {
                origin: 1,
                destination: -2,
                select_when_paused: false,
                frozen: false,
            },
        );
        assert!(!d.realm_dependencies_clear(1));
        d.endgame.forced.clear();
        assert!(d.realm_dependencies_clear(1));
        // Same guard must retain a replaced predecessor without delaying replacement publication.
        d.endgame.realms.get_mut(&-3).unwrap().replacement = Some(1);
        d.endgame.realms.get_mut(&-3).unwrap().cycle.stage = RealmStage::Transferring;
        d.worlds.get_mut(&-3).unwrap().admission_open = false;
        let child = d
            .instance("snake-pit", None, Some(DungeonReturn::Realm(-3)))
            .unwrap();
        d.endgame_step(&[]);
        assert!(d.worlds.contains_key(&-3));
        assert!(d.worlds[&1].admission_open);
        assert_eq!(d.lineage_count(), 1);
        d.worlds.remove(&child);
        d.endgame_step(&[]);
        assert!(!d.worlds.contains_key(&-3));
        assert!(d.worlds.contains_key(&1));
    }
    #[test]
    fn realm_demand_cancellation_and_delayed_builder_closure_priority() {
        let mut d = directory(1);
        d.realm_limit = 3;
        d.grow_realm_for_test();
        let independent = resident(&mut d, 1, 11);
        let independent_character = d.worlds[&1].players[&independent].character.clone();
        let player = resident(&mut d, -3, 10);
        let (packets, _inbox) = mpsc::channel(64);
        let (lost, _) = watch::channel(None);
        d.worlds
            .get_mut(&-3)
            .unwrap()
            .players
            .get_mut(&player)
            .unwrap()
            .tx = Some(Output { packets, lost });
        let constructed = d.make("realm-shore", "Realm", false, None).unwrap();
        let (send, result) = tokio::sync::oneshot::channel();
        d.next_world = 3;
        d.endgame.preparing = Some(Preparation::Demand(DemandPreparation {
            realm: 2,
            result,
            canceled: false,
            closure_wait: None,
        }));
        d.endgame.realms.get_mut(&-3).unwrap().cycle.stage =
            RealmStage::Closing { deadline: 120_000 };
        d.worlds.get_mut(&-3).unwrap().admission_open = false;
        d.tick = 2400;
        d.endgame_step(&[-3]);
        assert_eq!(
            d.endgame.realms[&-3].cycle.stage,
            RealmStage::Sealed { deadline: 128_000 }
        );
        d.realm_allocation_step();
        d.tick = 2560;
        d.endgame_step(&[-3]);
        d.realm_allocation_step();
        assert_eq!(d.endgame.realms[&-3].cycle.stage, RealmStage::Transferring);
        assert!(d.endgame.realms[&-3].replacement.is_none());
        assert!(d.worlds[&-3].players.contains_key(&player));
        assert_eq!(d.lineage_count(), 3);
        send.send(Ok(constructed)).ok().unwrap();
        d.realm_allocation_step();
        assert!(!d.worlds.contains_key(&2));
        assert_eq!(d.lineage_count(), 2);
        d.endgame_step(&[-3]);
        assert!(
            d.endgame.realms[&-3].replacement.is_some(),
            "closure takes the freed construction owner next"
        );
        d.endgame_step(&[-3]);
        let replacement = d.endgame.realms[&-3].replacement.unwrap();
        assert_eq!(replacement, 4);
        assert!(!d.worlds[&replacement].admission_open);
        assert!(d.worlds[&-3].players.contains_key(&player));
        assert_eq!(d.lineage_count(), 2);
        assert!(d.endgame.realms[&-3].pending_transfers.contains(&player));
        assert_eq!(
            d.worlds[&1].players[&independent].character,
            independent_character
        );
        assert!(d.worlds[&1].admission_open);
        d.worlds.get_mut(&1).unwrap().admission_open = false;
        assert!(!d.realm_pressure(), "all closed is never demand");
    }
    #[test]
    fn realm_reservation_expiry_and_constructor_failure_reset_demand() {
        let mut d = directory(1);
        let (_, _, _) = d.issue(1, 1, -3).unwrap();
        d.tick = 2399;
        d.allocate_realm_step().unwrap();
        assert!(d.endgame.demand_since.is_some());
        d.tick = 2400;
        d.retire_transfer_grants();
        d.allocate_realm_step().unwrap();
        assert!(d.endgame.demand_since.is_none());
        resident(&mut d, -3, 2);
        let (send, result) = tokio::sync::oneshot::channel();
        d.endgame.preparing = Some(Preparation::Demand(DemandPreparation {
            realm: 1,
            result,
            canceled: false,
            closure_wait: None,
        }));
        send.send(Err(anyhow::anyhow!("test constructor failure")))
            .ok()
            .unwrap();
        assert!(
            d.allocate_realm_step()
                .unwrap_err()
                .to_string()
                .contains("constructor")
        );
        assert!(d.endgame.preparing.is_none());
        assert!(!d.worlds.contains_key(&1));
        assert_eq!(d.endgame.realms.len(), 1);
        let (send, result) = tokio::sync::oneshot::channel();
        d.endgame.preparing = Some(Preparation::Demand(DemandPreparation {
            realm: 2,
            result,
            canceled: false,
            closure_wait: None,
        }));
        drop(send);
        assert!(
            d.allocate_realm_step()
                .unwrap_err()
                .to_string()
                .contains("worker stopped")
        );
        assert!(d.endgame.preparing.is_none());
    }
    #[test]
    fn realm_retirement_newest_first_preserves_other_completed_dwell() {
        let mut d = directory(72);
        d.realm_limit = 3;
        d.grow_realm_for_test();
        let realm = d.make("realm-shore", "Realm", false, None).unwrap();
        let heroes = realm.hero_identities();
        d.worlds.insert(2, realm);
        d.bind_realm_heroes(2, heroes).unwrap();
        let portal = d.install_realm_portal(2, None).unwrap();
        d.endgame.realms.get_mut(&2).unwrap().portal = Some(portal);
        d.realm_allocation_step();
        d.tick = 6000;
        d.realm_allocation_step();
        assert!(!d.worlds.contains_key(&2));
        assert!(d.worlds.contains_key(&1));
        d.realm_allocation_step();
        assert!(!d.worlds.contains_key(&1));
        assert!(d.worlds.contains_key(&-3));
    }
}
