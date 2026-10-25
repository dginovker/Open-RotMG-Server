use super::*;

async fn fixture() -> (Directory, i32, i32, i32, i32) {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let store = crate::store::Store::open(std::path::Path::new(":memory:"), &content).unwrap();
    let mut d = Directory::test_with_seed(std::path::Path::new("data"), content, 20, 423).unwrap();
    let (x, y) = d.worlds[&-3].map.spawns[0];
    let pos = Pos { x, y };
    let portal = d.drop_dungeon_portal(-3, 0x718, pos).unwrap();
    let destination = d.portals[&portal].destination.unwrap();
    let mut ids = vec![];
    for i in 0..3 {
        let account = store.list(format!("entry-{i}")).await.unwrap();
        let ch = store
            .create(account.id, d.content.get(crate::content::WIZARD).unwrap())
            .await
            .unwrap();
        let w = d.worlds.get_mut(&-3).unwrap();
        let id = w.enter(ch, None).unwrap();
        w.players.get_mut(&id).unwrap().pos = pos;
        ids.push(id);
    }
    for id in &ids {
        d.worlds.get_mut(&-3).unwrap().updates(*id, true).unwrap();
    }
    (d, ids[0], ids[1], portal, destination)
}
fn depart(d: &mut Directory, entrant: i32, portal: i32) -> (Character, i32, Vec<u8>, i32) {
    let (account, character, destination) = d.destination(entrant, Some(portal)).unwrap();
    let (time, key, _) = d.issue(account, character, destination).unwrap();
    d.bind_exit(entrant, Some(portal), &key).unwrap();
    let mut reservation = d.reserve_transfer(key.clone()).unwrap();
    let snapshot = d
        .begin_source_departure(entrant, Some(&key))
        .unwrap()
        .unwrap();
    d.accept_source_write(entrant, &key).unwrap();
    d.finish_source_departure(entrant, snapshot.clone(), Some(key.clone()))
        .unwrap();
    reservation.disarm();
    (snapshot, time, key, destination)
}
fn arrive(
    d: &mut Directory,
    snapshot: Character,
    time: i32,
    key: Vec<u8>,
    destination: i32,
) -> i32 {
    let (_, arrival) = d
        .consume(snapshot.account, destination, time, key.clone())
        .unwrap();
    d.admit_transfer(arrival, snapshot.account, snapshot.id, destination)
        .unwrap();
    d.worlds
        .get_mut(&destination)
        .unwrap()
        .enter_admitted(
            snapshot,
            None,
            None,
            AccountPresentation {
                guild: None,
                credits: 0,
                fame: 0,
                best_fame: BTreeMap::new(),
            },
            Some(key),
        )
        .unwrap()
}

#[tokio::test]
async fn dungeon_entry_success_is_local_public_and_once_for_all_controller_types() {
    for controller in [Controller::Human, Controller::Autonomous] {
        let (mut d, entrant, observer, portal, destination) = fixture().await;
        d.worlds
            .get_mut(&-3)
            .unwrap()
            .players
            .get_mut(&entrant)
            .unwrap()
            .controller = controller;
        let name = d.worlds[&-3].players[&entrant].character.name.clone();
        let lifetime = if matches!(controller, Controller::Human) {
            LIFETIME_MS
        } else {
            100
        };
        d.portals.get_mut(&portal).unwrap().expires = d.tick + lifetime / 50;
        let (ch, time, key, _) = depart(&mut d, entrant, portal);
        assert!(d.player_view(observer).unwrap().dungeon_entries.is_empty());
        let account = ch.account;
        let id = arrive(&mut d, ch, time, key.clone(), destination);
        // Destination admission and insertion alone are not publication.
        assert!(d.player_view(observer).unwrap().dungeon_entries.is_empty());
        assert_eq!(d.complete_dungeon_entry(&key, id).unwrap(), 2);
        let view = d.player_view(observer).unwrap();
        let e = &view.dungeon_entries[0];
        assert_eq!(
            (
                e.sequence,
                e.source_world,
                e.portal,
                e.portal_kind,
                e.entrant,
                e.account
            ),
            (entrant, -3, portal, 0x718, entrant, account)
        );
        assert_eq!(e.name, name);
        assert_eq!(e.expires_ms - e.observed_ms, lifetime);
        assert!(
            d.worlds[&-3]
                .player_view(-3, observer, e.expires_ms)
                .unwrap()
                .dungeon_entries
                .is_empty()
        );
        assert!(d.worlds[&-3].entities.contains_key(&portal));
        assert_eq!(d.complete_dungeon_entry(&key, id).unwrap(), 0);
        assert_eq!(d.player_view(observer).unwrap().dungeon_entries.len(), 1);
        d.tick += LIFETIME_MS / 50;
        assert!(d.player_view(observer).unwrap().dungeon_entries.is_empty());
    }
}

#[tokio::test]
async fn dungeon_entry_capture_rejects_unseen_invisible_private_or_wrong_portal() {
    for case in 0..11 {
        let (mut d, entrant, observer, portal, _) = fixture().await;
        let w = d.worlds.get_mut(&-3).unwrap();
        match case {
            0 => {
                w.players.get_mut(&observer).unwrap().known.remove(&entrant);
            }
            1 => {
                w.players.get_mut(&observer).unwrap().known.remove(&portal);
            }
            2 => {
                w.players.get_mut(&observer).unwrap().pos.x += 40.;
            }
            3 => {
                w.players
                    .get_mut(&entrant)
                    .unwrap()
                    .statuses
                    .insert(crate::condition::INVISIBLE, (0, u64::MAX));
            }
            4 => {
                w.owner = Some(w.players[&entrant].character.account);
            }
            5 => {
                w.guild = Some((1, 0));
            }
            6 => {
                w.entities.get_mut(&portal).unwrap().published = false;
            }
            7 => {
                w.players.get_mut(&entrant).unwrap().character.hp = 0;
            }
            8 | 9 => {}
            10 => {
                w.entities.get_mut(&portal).unwrap().condition =
                    crate::condition::bit(crate::condition::INVISIBLE);
            }
            _ => unreachable!(),
        }
        let account = d.worlds[&-3].players[&entrant].character.account;
        let character = d.worlds[&-3].players[&entrant].character.id;
        let destination = d.portals[&portal].destination.unwrap();
        let (_, key, _) = d.issue(account, character, destination).unwrap();
        d.bind_exit(entrant, Some(portal), &key).unwrap();
        if case == 9 {
            d.worlds.get_mut(&destination).unwrap().owner = Some(account);
        }
        if case == 8 {
            d.grants.get_mut(&key).unwrap().entry_portal = Some(portal + 1000);
        }
        let witness = d.capture_dungeon_entry(entrant, &key);
        assert!(
            witness
                .as_ref()
                .is_none_or(|w| !w.observers.iter().any(|(id, _, _)| *id == observer)),
            "case {case}"
        );
    }
}

#[tokio::test]
async fn dungeon_entry_delivery_revalidates_observer_and_portal_and_bounds_backlog() {
    for case in 0..14 {
        let (mut d, entrant, mut observer, portal, destination) = fixture().await;
        let (ch, time, key, _) = depart(&mut d, entrant, portal);
        let id = arrive(&mut d, ch, time, key.clone(), destination);
        let w = d.worlds.get_mut(&-3).unwrap();
        match case {
            0 => {
                w.players.get_mut(&observer).unwrap().character.id += 1;
            }
            1 => {
                w.players.get_mut(&observer).unwrap().pos.x += 40.;
            }
            2 => {
                w.players.get_mut(&observer).unwrap().known.remove(&portal);
            }
            3 => {
                w.remove_entity(portal);
            }
            4 => {
                w.players.get_mut(&observer).unwrap().departing = true;
            }
            5 => {
                d.portals.get_mut(&portal).unwrap().expires = d.tick;
            }
            6 => {
                d.tick += LIFETIME_MS / 50;
            }
            7 => {
                let p = w.players.get_mut(&observer).unwrap();
                p.dungeon_entries = (0..CAPACITY)
                    .map(|i| view::ObservedDungeonEntry {
                        sequence: -(i as i32),
                        source_world: -3,
                        portal,
                        portal_kind: 0x718,
                        entrant,
                        account: 1,
                        name: "Old".into(),
                        observed_ms: 0,
                        expires_ms: LIFETIME_MS,
                    })
                    .collect();
            }
            8 | 9 => {
                let ch = w.players[&observer].character.clone();
                let pos = w.players[&observer].pos;
                w.players.remove(&observer);
                let target = if case == 8 { -3 } else { -2 };
                let target = d.worlds.get_mut(&target).unwrap();
                observer = target.enter(ch, None).unwrap();
                target.players.get_mut(&observer).unwrap().pos = pos;
                target.updates(observer, true).unwrap();
            }
            10 => {
                d.portals.get_mut(&portal).unwrap().origin = -2;
            }
            11 => {
                d.worlds.get_mut(&destination).unwrap().owner = Some(1);
            }
            12 => {
                w.guild = Some((1, 0));
            }
            13 => {
                w.entities.get_mut(&portal).unwrap().condition =
                    crate::condition::bit(crate::condition::INVISIBLE);
            }
            _ => unreachable!(),
        }
        d.complete_dungeon_entry(&key, id).unwrap();
        assert!(
            !d.player_view(observer)
                .unwrap()
                .dungeon_entries
                .iter()
                .any(|e| e.sequence == entrant),
            "case {case}"
        );
    }
}

#[tokio::test]
async fn dungeon_entry_failed_cancelled_dead_or_unbound_transfer_never_publishes() {
    for case in 0..9 {
        let (mut d, entrant, observer, portal, destination) = fixture().await;
        let (ch, time, key, _) = depart(&mut d, entrant, portal);
        match case {
            0 => {
                d.worlds.get_mut(&destination).unwrap().admission_open = false;
                assert!(
                    d.consume(ch.account, destination, time, key.clone())
                        .is_err()
                );
            }
            1 => {
                let (_, arrival) = d
                    .consume(ch.account, destination, time, key.clone())
                    .unwrap();
                d.admit_transfer(arrival, ch.account, ch.id, destination)
                    .unwrap();
                assert!(
                    d.worlds
                        .get_mut(&destination)
                        .unwrap()
                        .enter_admitted(
                            ch,
                            None,
                            Some(Pos { x: -100., y: -100. }),
                            AccountPresentation {
                                guild: None,
                                credits: 0,
                                fame: 0,
                                best_fame: BTreeMap::new()
                            },
                            Some(key.clone())
                        )
                        .is_err()
                );
            }
            2 => {
                d.cancel_admission(ch.account, key.clone()).unwrap();
            }
            3 => {
                let id = arrive(&mut d, ch, time, key.clone(), destination);
                d.worlds
                    .get_mut(&destination)
                    .unwrap()
                    .players
                    .get_mut(&id)
                    .unwrap()
                    .death_pending = true;
                assert!(d.complete_dungeon_entry(&key, id).is_err());
            }
            4 => {
                let id = arrive(&mut d, ch, time, key.clone(), destination);
                d.worlds
                    .get_mut(&destination)
                    .unwrap()
                    .players
                    .get_mut(&id)
                    .unwrap()
                    .character
                    .id += 1;
                assert!(d.complete_dungeon_entry(&key, id).is_err());
            }
            5 => {
                let (_, arrival) = d
                    .consume(ch.account, destination, time, key.clone())
                    .unwrap();
                d.admit_transfer(arrival, ch.account, ch.id, destination)
                    .unwrap();
                assert!(d.complete_dungeon_entry(&key, entrant).is_err());
            }
            6 => {
                let (_, arrival) = d
                    .consume(ch.account, destination, time, key.clone())
                    .unwrap();
                d.worlds.get_mut(&destination).unwrap().admission_open = false;
                assert!(
                    d.admit_transfer(arrival, ch.account, ch.id, destination)
                        .is_err()
                );
            }
            7 => {
                let id = arrive(&mut d, ch, time, key.clone(), destination);
                d.grants[&key]
                    .authority
                    .0
                    .fetch_or(TRANSFER_REVOKED, Ordering::SeqCst);
                assert!(d.complete_dungeon_entry(&key, id).is_err());
            }
            8 => {
                let (_, arrival) = d
                    .consume(ch.account, destination, time, key.clone())
                    .unwrap();
                d.retire_replacement(arrival, ch.account, ch.id, destination)
                    .unwrap();
                assert!(d.complete_dungeon_entry(&key, entrant).is_err());
            }
            _ => unreachable!(),
        }
        assert!(
            d.player_view(observer).unwrap().dungeon_entries.is_empty(),
            "case {case}"
        );
    }
    let (mut d, entrant, observer, _, _) = fixture().await;
    let ch = d.begin_source_departure(entrant, None).unwrap().unwrap();
    d.finish_source_departure(entrant, ch, None).unwrap();
    assert!(d.player_view(observer).unwrap().dungeon_entries.is_empty());
}
