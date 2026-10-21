use super::*;
use crate::content::Grounds;

#[path = "teleport_actor_tests.rs"]
mod teleport_actor_tests;

impl Harness {
    fn normal() -> Self {
        Self::normal_with_db(Arc::new(Mutex::new(
            database(std::path::Path::new(":memory:")).unwrap(),
        )))
    }
    fn normal_with_db(db: Database) -> Self {
        let path = std::env::var_os("BUILD100_CLIENT_DIR")
            .expect("realm_entry tests require qualified BUILD100_CLIENT_DIR");
        let path = std::path::Path::new(&path);
        let simulation = Simulation::normal(
            Content::load(path).unwrap(),
            Arc::new(Grounds::load(path).unwrap()),
            db.clone(),
            100,
        )
        .unwrap();
        let (commands, receiver) = mpsc::channel(64);
        let task = tokio::spawn(simulation.run(receiver, false));
        Self { commands, db, task }
    }
    async fn enter_route(&self, guid: &str, route: i32) -> (Client, Vec<Packet>) {
        let mut c = self.join(guid, route).await.unwrap();
        self.input(c.session, create()).await.unwrap();
        let out = drain(&mut c);
        c.avatar = Reader::new(&out.iter().find(|p| p.id == 1).unwrap().body)
            .i32()
            .unwrap();
        (c, out)
    }
    async fn place(&self, session: u64, position: (f32, f32)) {
        self.inspect(move |s| {
            let session = &s.sessions[&session];
            let world = session.world;
            let id = session.avatar.unwrap();
            let mut room = s.worlds[&world].clone();
            let p = room.character_mut(id);
            p.x = position.0;
            p.y = position.1;
            s.commit(&room, &s.worlds[&world]).unwrap();
            s.worlds.insert(world, room);
        })
        .await;
    }
    async fn near_exit(&self, c: &Client) {
        self.place(c.session, (27.5, 27.5)).await;
        let session = c.session;
        self.inspect(move |s| s.refresh_lease(session)).await;
    }
}
fn map_description(out: &[Packet]) -> (i32, i32, String) {
    let mut r = Reader::new(&out.iter().find(|p| p.id == 24).expect("MapInfo").body);
    (r.i32().unwrap(), r.i32().unwrap(), r.utf().unwrap())
}

#[tokio::test]
async fn realm_entry_private_tutorial_shared_realm_and_bounded_initial_publication() {
    let h = Harness::normal();
    let (mut a, pa) = h.enter_route(A, -1).await;
    let (mut b, pb) = h.enter_route(B, -1).await;
    assert_eq!(map_description(&pa), (32, 32, "Tutorial".into()));
    assert_eq!(map_description(&pb), (32, 32, "Tutorial".into()));
    assert_eq!(objects(&pa, 0x30e), vec![a.avatar]);
    assert_eq!(objects(&pb, 0x30e), vec![b.avatar]);
    assert_ne!(
        h.inspect(move |s| s.sessions[&a.session].world).await,
        h.inspect(move |s| s.sessions[&b.session].world).await
    );
    assert!(h.join(A, -1).await.is_err());
    assert!(h.join(A, 0).await.is_err()); // No reachable live exit.
    assert!(h.join(A, 999).await.is_err());
    h.detach(a.session).await;
    h.detach(b.session).await;
    drain(&mut a);
    drain(&mut b);
    let mut a = h.join(A, 0).await.unwrap();
    let pa = h.load(&mut a).await;
    let mut b = h.join(B, 0).await.unwrap();
    let pb = h.load(&mut b).await;
    assert_eq!(
        map_description(&pa),
        (256, 256, "Realm of the Mad God".into())
    );
    assert_eq!(objects(&pb, 0x30e), vec![a.avatar, b.avatar]);
    assert_eq!(objects(&drain(&mut a), 0x30e), vec![b.avatar]);
    assert!((1..=1089).contains(&pa.iter().filter(|p| p.id == 12).count()));
    let create = pa.iter().position(|p| p.id == 1).unwrap();
    let make = pa.iter().position(|p| p.id == 10).unwrap();
    assert!(create < make);
    assert_eq!(
        h.inspect(|s| (s.worlds.len(), s.worlds[&0].players.len()))
            .await,
        (1, 2)
    );
    h.stop().await;
}

#[tokio::test]
async fn realm_entry_portal_zero_both_close_orders_completion_committed_only_on_arrival() {
    for close_first in [false, true] {
        let h = Harness::normal();
        let (mut old, _) = h.enter_route(A, -1).await;
        h.near_exit(&old).await;
        if close_first {
            h.detach(old.session).await;
        }
        let mut next = h.join(A, 0).await.unwrap();
        assert!(!character(&h.db, A).unwrap().unwrap().tutorial_complete);
        h.load(&mut next).await;
        let p = character(&h.db, A).unwrap().unwrap();
        assert!(p.tutorial_complete);
        assert_eq!(p.generation, 1);
        assert_eq!(p.stats[&6], 0);
        let expected = p.clone();
        h.detach(old.session).await; // Late exact old token must never release next.
        assert!(h.input(old.session, load()).await.is_err());
        assert!(h.join(A, 0).await.is_err());
        assert!(h.join(A, -1).await.is_err());
        assert_eq!(h.inspect(|s| s.sessions.len()).await, 1);
        drain(&mut old);
        drain(&mut next);
        h.detach(next.session).await;
        // No owner: game0 is a lawful saved entry, not a replayable completion grant.
        let mut again = h.join(A, 0).await.unwrap();
        h.load(&mut again).await;
        assert_eq!(character(&h.db, A).unwrap().unwrap(), expected);
        assert_eq!(h.inspect(|s| s.worlds.len()).await, 1);
        h.stop().await;
    }
}

#[tokio::test]
async fn realm_entry_expired_disconnected_exit_is_saved_admission_without_completion() {
    let h = Harness::normal();
    let (mut old, _) = h.enter_route(A, -1).await;
    h.near_exit(&old).await;
    h.detach(old.session).await;
    h.tick(5.).await.unwrap();
    drain(&mut old);
    assert_eq!(
        h.inspect(|s| (s.leases.len(), s.worlds.len())).await,
        (0, 1)
    );
    let mut next = h.join(A, 0).await.unwrap();
    h.load(&mut next).await;
    assert!(!character(&h.db, A).unwrap().unwrap().tutorial_complete);
    h.detach(next.session).await;
    let mut tutorial = h.join(A, -1).await.unwrap();
    h.load(&mut tutorial).await;
    h.near_exit(&tutorial).await;
    let next = h.join(A, 0).await.unwrap(); // Commit has not happened: socket lost before selection.
    h.detach(next.session).await;
    assert!(!character(&h.db, A).unwrap().unwrap().tutorial_complete);
    let mut saved = h.join(A, 0).await.unwrap();
    h.load(&mut saved).await;
    assert!(!character(&h.db, A).unwrap().unwrap().tutorial_complete);
    h.stop().await;
}

#[tokio::test]
async fn realm_entry_saved_hp_inventory_shore_restart_and_dead_generation_rejection() {
    let h = Harness::normal();
    let (mut old, _) = h.enter_route(A, -1).await;
    let mut p = character(&h.db, A).unwrap().unwrap();
    p.stats.insert(1, 37);
    p.stats.insert(12, 0xa4f);
    p.x = 27.5;
    p.y = 27.5;
    let state = serde_json::to_string(&p).unwrap();
    h.db.lock()
        .unwrap()
        .execute(
            "UPDATE build100_characters SET state=? WHERE guid=?",
            [state, A.into()],
        )
        .unwrap();
    let db = h.db.clone();
    h.stop().await;
    drain(&mut old);
    let h = Harness::normal_with_db(db);
    let mut saved = h.join(A, 0).await.unwrap();
    h.load(&mut saved).await;
    let got = character(&h.db, A).unwrap().unwrap();
    assert_eq!(got.stats, p.stats);
    assert_ne!((got.x, got.y), (p.x, p.y));
    assert_eq!((got.x, got.y), h.inspect(|s| s.worlds[&0].landing()).await);
    let session = saved.session;
    h.inspect(move |s| {
        let world = s.sessions[&session].world;
        let id = s.sessions[&session].avatar.unwrap();
        let mut room = s.worlds[&world].clone();
        room.character_mut(id).alive = false;
        room.character_mut(id).stats.insert(1, 0);
        s.commit(&room, &s.worlds[&world]).unwrap();
        s.worlds.insert(world, room);
    })
    .await;
    assert!(h.join(A, 0).await.is_err());
    h.detach(saved.session).await;
    let mut dead = h.join(A, 0).await.unwrap();
    assert!(h.input(dead.session, load()).await.is_err());
    drain(&mut dead);
    h.input(dead.session, create()).await.unwrap();
    assert_eq!(character(&h.db, A).unwrap().unwrap().generation, 2);
    assert!(!character(&h.db, A).unwrap().unwrap().tutorial_complete);
    h.stop().await;
}

#[tokio::test]
async fn realm_entry_initial_overflow_and_cascade_never_publish_ghost_avatar() {
    let h = Harness::normal();
    let (mut witness, _) = h.enter(A).await;
    let slow = h.join_capacity(B, 0, 1).await.unwrap(); // MapInfo occupies its only output slot.
    h.input(slow.session, create()).await.unwrap();
    let out = drain(&mut witness);
    assert!(objects(&out, 0x30e).is_empty());
    assert_eq!(
        h.inspect(|s| (s.sessions.len(), s.worlds[&0].players.len()))
            .await,
        (1, 1)
    );
    assert!(character(&h.db, B).unwrap().unwrap().alive); // Durable creation precedes output.
    let mut other = h.join(B, 0).await.unwrap();
    h.load(&mut other).await;
    drain(&mut witness);
    drop(other.output);
    h.tick(0.2).await.unwrap();
    let out = drain(&mut witness);
    assert!(
        out.iter()
            .any(|p| p.id == 11 && Reader::new(&p.body).i32().unwrap() == other.avatar)
    );
    assert_eq!(h.inspect(|s| s.owners.len()).await, 1);
    h.stop().await;
}

#[tokio::test]
async fn realm_entry_private_maps_retire_and_dynamic_game_ids_do_not_partition_owner_ids() {
    let h = Harness::normal();
    h.inspect(|s| s.next_world = 30_000_000).await;
    for i in 0..20 {
        let mut c = h.join(A, -1).await.unwrap();
        h.input(c.session, if i == 0 { create() } else { load() })
            .await
            .unwrap();
        drain(&mut c);
        let (game, ids) = h
            .inspect(move |s| {
                let game = s.sessions[&c.session].world;
                (
                    game,
                    s.worlds[&game].players.keys().copied().collect::<Vec<_>>(),
                )
            })
            .await;
        assert!(game > 1 << 24);
        assert!(ids.iter().all(|id| *id < 1 << 24));
        h.detach(c.session).await;
        assert_eq!(h.inspect(|s| s.worlds.len()).await, 1);
    }
    h.stop().await;
}

#[tokio::test]
async fn realm_entry_tutorial_pickup_is_committed_before_output_replay_and_exit_keep_inventory() {
    let h = Harness::normal();
    let (mut c, _) = h.enter_route(A, -1).await;
    h.place(c.session, (24., 24.5)).await;
    h.tick(1.1).await.unwrap();
    let bag = objects(&drain(&mut c), 0x806)[0];
    let pickup = || {
        packet(14, |w| {
            w.i32(bag).u8(0).i16(0xa4f).i32(c.avatar).u8(5).i16(-1);
        })
    };
    h.input(c.session, pickup()).await.unwrap();
    assert_eq!(character(&h.db, A).unwrap().unwrap().stats[&13], 0xa4f);
    assert!(h.input(c.session, pickup()).await.is_err());
    drain(&mut c);
    h.tick(1.1).await.unwrap();
    let bags = objects(&drain(&mut c), 0x806);
    assert_eq!(bags.len(), 1);
    assert_ne!(bags[0], bag);
    let stats = character(&h.db, A).unwrap().unwrap().stats;
    h.near_exit(&c).await;
    let mut next = h.join(A, 0).await.unwrap();
    h.load(&mut next).await;
    let p = character(&h.db, A).unwrap().unwrap();
    assert_eq!(p.stats, stats);
    assert!(p.tutorial_complete);
    h.stop().await;
}

#[tokio::test]
async fn realm_entry_dead_stale_forged_portal_zero_and_arrival_storage_failure() {
    let h = Harness::normal();
    let (mut c, _) = h.enter_route(A, -1).await;
    assert!(h.join(A, 0).await.is_err()); // A GUID and game0 cannot evict an unreachable live owner.
    h.near_exit(&c).await;
    let mut saved = character(&h.db, A).unwrap().unwrap();
    saved.generation += 1;
    let state = serde_json::to_string(&saved).unwrap();
    h.db.lock()
        .unwrap()
        .execute(
            "UPDATE build100_characters SET state=? WHERE guid=?",
            [state, A.into()],
        )
        .unwrap();
    assert!(h.join(A, 0).await.is_err());
    saved.generation -= 1;
    saved.alive = false;
    saved.stats.insert(1, 0);
    let state = serde_json::to_string(&saved).unwrap();
    h.db.lock()
        .unwrap()
        .execute(
            "UPDATE build100_characters SET state=? WHERE guid=?",
            [state, A.into()],
        )
        .unwrap();
    assert!(h.join(A, 0).await.is_err());
    h.stop().await;
    drain(&mut c);

    let h = Harness::normal();
    let (mut c, _) = h.enter_route(A, -1).await;
    h.near_exit(&c).await;
    let mut next = h.join(A, 0).await.unwrap();
    drain(&mut next);
    h.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_arrival BEFORE UPDATE ON build100_characters BEGIN SELECT RAISE(FAIL,'injected arrival failure'); END").unwrap();
    assert!(h.input(next.session, load()).await.is_err());
    assert!(drain(&mut next).is_empty());
    assert!(!character(&h.db, A).unwrap().unwrap().tutorial_complete);
    drop(h.commands);
    assert!(h.task.await.unwrap().unwrap_err().is::<StorageFailure>());
    drain(&mut c);
}

fn native_move(x: f32, y: f32) -> Packet {
    packet(5, |w| {
        w.f32(x).f32(y);
    })
}
fn tick_count(packets: &[Packet]) -> usize {
    packets.iter().filter(|p| p.id == 13).count()
}

#[tokio::test]
async fn realm_entry_native_feedback_tick_move_client_loop_quiesces_without_clock() {
    let h = Harness::normal();
    let (a, pa) = h.enter_route(A, -1).await;
    let (b, pb) = h.enter_route(B, -1).await;
    for packets in [&pa, &pb] {
        assert_eq!(tick_count(packets), 1);
        let create = packets.iter().position(|p| p.id == 1).unwrap();
        let ground = packets.iter().position(|p| p.id == 12).unwrap();
        let make = packets.iter().position(|p| p.id == 10).unwrap();
        let tick = packets.iter().position(|p| p.id == 13).unwrap();
        assert!(create < ground && ground < make && make < tick);
    }
    let mut clients = [a, b];
    let mut pending = [pa, pb];
    let mut replies = 0;
    // The unchanged client answers each server Tick with one Move, even while standing still.
    for _ in 0..8 {
        let counts = pending.each_ref().map(|p| tick_count(p));
        if counts == [0, 0] {
            break;
        }
        for i in 0..2 {
            for _ in 0..counts[i] {
                h.input(clients[i].session, native_move(4.5, 5.5))
                    .await
                    .unwrap();
                replies += 1;
            }
            pending[i] = drain(&mut clients[i]);
        }
    }
    assert_eq!(
        pending.each_ref().map(|p| tick_count(p)),
        [0, 0],
        "native Tick→Move responses must not self-excite without a clock event"
    );
    assert_eq!(replies, 2, "only the two initial Ticks require replies");
    h.tick(0.05).await.unwrap();
    for client in &mut clients {
        assert_eq!(tick_count(&drain(client)), 0);
    }
    for dt in [0.15, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2] {
        h.tick(dt).await.unwrap();
        for client in &mut clients {
            assert_eq!(
                tick_count(&drain(client)),
                1,
                "each due clock event must still publish one Tick"
            );
            h.input(client.session, native_move(4.5, 5.5))
                .await
                .unwrap();
            assert_eq!(tick_count(&drain(client)), 0);
        }
    }
    assert_eq!(
        h.inspect(|s| (s.sessions.len(), s.owners.len())).await,
        (2, 2)
    );
    h.stop().await;
}

#[tokio::test]
async fn realm_entry_native_feedback_move_keeps_interest_for_mover_and_other_recipient() {
    let h = Harness::normal();
    let (mut a, _) = h.enter(A).await;
    let (mut b, _) = h.enter(B).await;
    drain(&mut a);
    let far = h
        .inspect(|s| {
            let map = s.worlds[&0].map.as_ref().unwrap();
            map.objects()
                .iter()
                .flat_map(|p| {
                    let (x, y) = p.position();
                    [(x + 1., y), (x - 1., y), (x, y + 1.), (x, y - 1.)]
                })
                .find(|&(x, y)| {
                    (x - map.spawn.0).hypot(y - map.spawn.1) > 40.
                        && map.walkable(x.floor() as i32, y.floor() as i32)
                })
                .unwrap()
        })
        .await;
    h.place(a.session, far).await;
    h.input(a.session, native_move(far.0, far.1)).await.unwrap();
    let moved = drain(&mut a);
    let observed = drain(&mut b);
    assert!(
        moved.iter().any(|p| p.id == 12),
        "movement must publish newly visible ground"
    );
    assert!(
        moved.iter().any(|p| p.id == 10),
        "movement must publish nearby objects"
    );
    assert!(
        moved
            .iter()
            .any(|p| p.id == 11 && Reader::new(&p.body).i32().unwrap() == b.avatar)
    );
    assert!(
        observed
            .iter()
            .any(|p| p.id == 11 && Reader::new(&p.body).i32().unwrap() == a.avatar)
    );
    assert_eq!((tick_count(&moved), tick_count(&observed)), (0, 0));
    h.input(a.session, native_move(far.0, far.1)).await.unwrap();
    assert!(drain(&mut a).is_empty());
    assert!(drain(&mut b).is_empty());
    let shore = h.inspect(|s| s.worlds[&0].landing()).await;
    h.place(a.session, shore).await;
    h.input(a.session, native_move(shore.0, shore.1))
        .await
        .unwrap();
    let returned = drain(&mut a);
    let observed = drain(&mut b);
    assert!(objects(&returned, 0x30e).contains(&b.avatar));
    assert!(objects(&observed, 0x30e).contains(&a.avatar));
    assert_eq!((tick_count(&returned), tick_count(&observed)), (0, 0));
    h.tick(0.2).await.unwrap();
    assert_eq!(tick_count(&drain(&mut a)), 1);
    assert_eq!(tick_count(&drain(&mut b)), 1);
    h.stop().await;
}

#[tokio::test]
async fn realm_entry_native_feedback_explicit_stats_and_off_clock_death_still_tick() {
    let h = Harness::normal();
    let (mut a, _) = h.enter(A).await;
    let (mut b, _) = h.enter(B).await;
    drain(&mut a);
    let session = a.session;
    h.inspect(move |s| {
        let id = s.sessions[&session].avatar.unwrap();
        let mut room = s.worlds[&0].clone();
        room.character_mut(id).stats.insert(1, 20);
        s.commit(&room, &s.worlds[&0]).unwrap();
        s.worlds.insert(0, room);
    })
    .await;
    let shore = h.inspect(|s| s.worlds[&0].landing()).await;
    h.input(
        a.session,
        packet(15, |w| {
            w.i32(a.avatar).u8(4).i32(-1).f32(shore.0).f32(shore.1);
        }),
    )
    .await
    .unwrap();
    assert_eq!(tick_count(&drain(&mut a)), 1);
    assert_eq!(tick_count(&drain(&mut b)), 1);
    let saved = character(&h.db, A).unwrap().unwrap();
    assert_eq!(saved.stats[&1], 100);
    assert_eq!(saved.stats[&12], -1);
    for client in [&mut a, &mut b] {
        h.input(client.session, native_move(shore.0, shore.1))
            .await
            .unwrap();
        assert_eq!(tick_count(&drain(client)), 0);
    }
    let lava = h
        .inspect(|s| {
            let map = s.worlds[&0].map.as_ref().unwrap();
            let (w, _) = map.dimensions();
            let i = map.tiles().iter().position(|t| t.ground == 0x70).unwrap();
            (
                (i % usize::from(w)) as f32 + 0.5,
                (i / usize::from(w)) as f32 + 0.5,
            )
        })
        .await;
    h.place(a.session, lava).await;
    let session = a.session;
    h.inspect(move |s| {
        let id = s.sessions[&session].avatar.unwrap();
        let mut room = s.worlds[&0].clone();
        room.character_mut(id).stats.insert(1, 1);
        s.commit(&room, &s.worlds[&0]).unwrap();
        s.worlds.insert(0, room);
    })
    .await;
    // These clock events leave the next regular Tick at0.6s; lava death happens at0.5s.
    for dt in [0.4, 0.01] {
        h.tick(dt).await.unwrap();
        drain(&mut a);
        drain(&mut b);
    }
    h.tick(0.09).await.unwrap();
    let death = drain(&mut a);
    let other = drain(&mut b);
    assert_eq!(tick_count(&death), 1);
    assert_eq!(tick_count(&other), 1);
    let tick = death.iter().position(|p| p.id == 13).unwrap();
    let drop = death
        .iter()
        .position(|p| p.id == 11 && Reader::new(&p.body).i32().unwrap() == a.avatar)
        .unwrap();
    assert!(
        tick < drop,
        "terminal authoritative status must precede own Drop"
    );
    let saved = character(&h.db, A).unwrap().unwrap();
    assert!(!saved.alive);
    assert_eq!(saved.stats[&1], 0);
    h.input(a.session, native_move(lava.0, lava.1))
        .await
        .unwrap();
    assert_eq!(tick_count(&drain(&mut a)), 0);
    assert_eq!(tick_count(&drain(&mut b)), 0);
    h.stop().await;
}

fn equipment_swap(a: (i32, u8, i16), b: (i32, u8, i16)) -> Packet {
    packet(14, |w| {
        w.i32(a.0).u8(a.1).i16(a.2).i32(b.0).u8(b.1).i16(b.2);
    })
}
fn equipment_wire(packets: &[Packet], packet_id: u8, avatar: i32) -> BTreeMap<u8, i32> {
    for p in packets.iter().filter(|p| p.id == packet_id) {
        let mut r = Reader::new(&p.body);
        let count = if packet_id == 10 {
            r.i16().unwrap();
            r.utf().unwrap();
            1
        } else {
            r.i16().unwrap()
        };
        for _ in 0..count {
            let id = r.i32().unwrap();
            r.f32().unwrap();
            r.f32().unwrap();
            let n = r.i16().unwrap();
            let stats = (0..n)
                .map(|_| (r.u8().unwrap(), r.i32().unwrap()))
                .collect();
            if id == avatar {
                return stats;
            }
        }
    }
    panic!("missing avatar {avatar} packet {packet_id}")
}

#[tokio::test]
async fn equipment_runtime_actor_real_tutorial_bag_equipment_transfer_peer_and_repeated_reload() {
    let h = Harness::normal();
    let (mut a, _) = h.enter_route(A, -1).await;
    h.place(a.session, (24.5, 24.5)).await;
    h.tick(1.01).await.unwrap();
    let bag = objects(&drain(&mut a), 0x806)[0];
    h.input(
        a.session,
        equipment_swap((bag, 0, 0xa4f), (a.avatar, 3, -1)),
    )
    .await
    .unwrap();
    let out = drain(&mut a);
    assert_eq!(equipment_wire(&out, 13, a.avatar)[&20], 17);
    let saved = character(&h.db, A).unwrap().unwrap();
    assert_eq!(
        (saved.stats[&20], saved.stats[&21], saved.stats[&11]),
        (12, 0, 0xa4f)
    );
    assert_eq!(
        h.inspect(|s| {
            let room = s.worlds.values().find(|r| !r.players.is_empty()).unwrap();
            room.players.len()
        })
        .await,
        1
    );
    assert!(
        h.input(
            a.session,
            equipment_swap((bag, 0, 0xa4f), (a.avatar, 5, -1))
        )
        .await
        .is_err()
    );
    assert_eq!(character(&h.db, A).unwrap().unwrap(), saved);
    assert!(drain(&mut a).is_empty());
    h.near_exit(&a).await;
    let mut a = h.join(A, 0).await.unwrap();
    let out = h.load(&mut a).await;
    assert_eq!(equipment_wire(&out, 10, a.avatar)[&20], 17);
    assert_eq!(equipment_wire(&out, 13, a.avatar)[&20], 17);
    let (mut b, out) = h.enter_route(B, 0).await;
    assert_eq!(equipment_wire(&out, 10, a.avatar)[&20], 17);
    let peer_before = character(&h.db, B).unwrap().unwrap();
    drain(&mut a);
    for _ in 0..3 {
        h.input(
            a.session,
            equipment_swap((a.avatar, 3, 0xa4f), (a.avatar, 5, -1)),
        )
        .await
        .unwrap();
        for out in [drain(&mut a), drain(&mut b)] {
            assert_eq!(equipment_wire(&out, 13, a.avatar)[&20], 12);
        }
        h.input(
            a.session,
            equipment_swap((a.avatar, 3, -1), (a.avatar, 5, 0xa4f)),
        )
        .await
        .unwrap();
        for out in [drain(&mut a), drain(&mut b)] {
            let stats = equipment_wire(&out, 13, a.avatar);
            assert_eq!(
                (stats[&20], stats[&21], stats[&11], stats[&13]),
                (17, 0, 0xa4f, -1)
            );
        }
        h.detach(a.session).await;
        drain(&mut b);
        a = h.join(A, 0).await.unwrap();
        let out = h.load(&mut a).await;
        assert_eq!(equipment_wire(&out, 10, a.avatar)[&20], 17);
        assert_eq!(equipment_wire(&drain(&mut b), 10, a.avatar)[&20], 17);
        let saved = character(&h.db, A).unwrap().unwrap();
        assert_eq!((saved.stats[&20], saved.stats[&21]), (12, 0));
        assert_eq!((8..20).filter(|slot| saved.stats[slot] == 0xa4f).count(), 1);
        assert_eq!(
            (saved.stats[&5], saved.stats[&6], saved.stats[&7]),
            (50, 0, 1)
        );
        assert!(saved.tutorial_complete);
        assert_eq!(character(&h.db, B).unwrap().unwrap(), peer_before);
    }
    // A fresh Simulation reads the same durable permanent fields, not projected wire bonuses.
    h.detach(a.session).await;
    let db = h.db.clone();
    h.stop().await;
    let h = Harness::normal_with_db(db);
    let mut a = h.join(A, 0).await.unwrap();
    let out = h.load(&mut a).await;
    assert_eq!(equipment_wire(&out, 10, a.avatar)[&20], 17);
    assert_eq!(character(&h.db, A).unwrap().unwrap().stats[&20], 12);
    h.stop().await;
}

#[tokio::test]
async fn equipment_runtime_actor_vitals_commit_reload_clamp_and_malformed_saved_rejection() {
    let h = Harness::normal();
    let mut p = Character::new(
        &h.inspect(|s| Content {
            objects: s.content.objects.clone(),
        })
        .await,
        0x30e,
        "Equipped".into(),
        1,
    )
    .unwrap();
    p.stats.extend([(12, 0xa27), (13, 0xa5f), (14, 0xa22)]);
    h.db.lock()
        .unwrap()
        .execute(
            "INSERT INTO build100_characters(guid,state) VALUES(?,?)",
            [A, &serde_json::to_string(&p).unwrap()],
        )
        .unwrap();
    let mut a = h.join(A, 0).await.unwrap();
    h.load(&mut a).await;
    let (mut b, _) = h.enter_route(B, 0).await;
    drain(&mut a);
    let peer_before = character(&h.db, B).unwrap().unwrap();
    for (backpack, item, equipped) in [(4, 0xa27, 3), (5, 0xa5f, 2)] {
        h.input(
            a.session,
            equipment_swap((a.avatar, backpack, item), (a.avatar, equipped, -1)),
        )
        .await
        .unwrap();
        let out = drain(&mut a);
        let stats = equipment_wire(&out, 13, a.avatar);
        assert_eq!((stats[&1], stats[&4]), (100, 100));
        assert_eq!(equipment_wire(&drain(&mut b), 13, a.avatar), stats);
    }
    h.input(
        a.session,
        packet(15, |w| {
            w.i32(a.avatar).u8(6).i32(-1).f32(0.).f32(0.);
        }),
    )
    .await
    .unwrap();
    assert_eq!(equipment_wire(&drain(&mut a), 13, a.avatar)[&1], 140);
    assert_eq!(equipment_wire(&drain(&mut b), 13, a.avatar)[&1], 140);
    let saved = character(&h.db, A).unwrap().unwrap();
    assert_eq!(
        (
            saved.stats[&0],
            saved.stats[&3],
            saved.stats[&1],
            saved.stats[&14]
        ),
        (100, 100, 140, -1)
    );
    h.detach(a.session).await;
    drain(&mut b);
    a = h.join(A, 0).await.unwrap();
    let out = h.load(&mut a).await;
    assert_eq!(equipment_wire(&out, 10, a.avatar)[&1], 140);
    assert_eq!(equipment_wire(&drain(&mut b), 10, a.avatar)[&0], 140);
    h.input(
        a.session,
        equipment_swap((a.avatar, 3, 0xa27), (a.avatar, 4, -1)),
    )
    .await
    .unwrap();
    for out in [drain(&mut a), drain(&mut b)] {
        let stats = equipment_wire(&out, 13, a.avatar);
        assert_eq!(
            (stats[&0], stats[&1], stats[&3], stats[&4]),
            (100, 100, 140, 100)
        );
    }
    let saved = character(&h.db, A).unwrap().unwrap();
    assert_eq!(
        (
            saved.stats[&0],
            saved.stats[&1],
            saved.stats[&12],
            saved.stats[&11]
        ),
        (100, 100, 0xa27, -1)
    );
    assert_eq!(character(&h.db, B).unwrap().unwrap(), peer_before);
    h.detach(a.session).await;
    drain(&mut b);
    let mut bad = saved.clone();
    bad.stats.insert(1, 101); // Ring is in backpack: this saved currentHP is invalid.
    h.db.lock()
        .unwrap()
        .execute(
            "UPDATE build100_characters SET state=? WHERE guid=?",
            [&serde_json::to_string(&bad).unwrap(), A],
        )
        .unwrap();
    a = h.join(A, 0).await.unwrap();
    drain(&mut a);
    assert!(
        h.input(a.session, load())
            .await
            .unwrap_err()
            .contains("current HP/alive")
    );
    assert!(drain(&mut a).is_empty());
    assert!(drain(&mut b).is_empty());
    assert_eq!(character(&h.db, A).unwrap().unwrap(), bad);
    assert_eq!(h.inspect(move |s| s.worlds[&0].players.len()).await, 1);
    h.stop().await;
}

#[tokio::test]
async fn equipment_runtime_actor_sql_failure_keeps_equipment_hp_bag_and_peer_before_output() {
    let h = Harness::normal();
    let (mut a, _) = h.enter_route(A, -1).await;
    h.place(a.session, (24.5, 24.5)).await;
    h.tick(1.01).await.unwrap();
    let bag = objects(&drain(&mut a), 0x806)[0];
    // The ordinary tutorial bag's ring gets equipped, then swaps back into that same bag.
    h.input(
        a.session,
        equipment_swap((bag, 0, 0xa4f), (a.avatar, 3, -1)),
    )
    .await
    .unwrap();
    drain(&mut a);
    let before = character(&h.db, A).unwrap().unwrap();
    h.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_equipment BEFORE UPDATE ON build100_characters BEGIN SELECT RAISE(ABORT,'equipment commit failure'); END;").unwrap();
    // Inspect the production candidate/commit method before run's fatal-storage boundary exits.
    let (error, unchanged, wire_attack) = h
        .inspect(move |s| {
            let world = s.sessions[&a.session].world;
            let before = s.worlds[&world].character(a.avatar).clone();
            let error = s
                .input(
                    a.session,
                    equipment_swap((a.avatar, 3, 0xa4f), (bag, 0, -1)),
                    Instant::now(),
                )
                .unwrap_err();
            let unchanged = s.worlds[&world].character(a.avatar) == &before;
            let out = s
                .worlds
                .get_mut(&world)
                .unwrap()
                .publication(a.avatar, &[], &s.content)
                .unwrap();
            // It was already known, so arrival explicitly inspects the same wire projection.
            assert!(!out.iter().any(|p| p.id == 13));
            let arrival = s.worlds[&world].arrival(a.avatar, &s.content).unwrap();
            (
                format!("{error:#}"),
                unchanged,
                equipment_wire(&[arrival], 10, a.avatar)[&20],
            )
        })
        .await;
    assert!(error.contains("equipment commit failure"));
    assert!(unchanged);
    assert_eq!(wire_attack, 17);
    assert!(drain(&mut a).is_empty());
    assert_eq!(character(&h.db, A).unwrap().unwrap(), before);
    h.db.lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_equipment;")
        .unwrap();
    h.input(
        a.session,
        equipment_swap((a.avatar, 3, 0xa4f), (bag, 0, -1)),
    )
    .await
    .unwrap();
    assert_eq!(equipment_wire(&drain(&mut a), 13, a.avatar)[&20], 12);
    // A successful retry using the original expected empty bag slot proves failure did not mutate it.
    let saved = character(&h.db, A).unwrap().unwrap();
    assert_eq!((saved.stats[&11], saved.stats[&20]), (-1, 12));
    h.stop().await;

    let h = Harness::normal();
    let (mut a, _) = h.enter_route(A, 0).await;
    let (mut b, _) = h.enter_route(B, 0).await;
    h.inspect(move |s| {
        let mut candidate = s.worlds[&0].clone();
        candidate
            .character_mut(a.avatar)
            .stats
            .extend([(11, 0xa27), (1, 140)]);
        s.commit(&candidate, &s.worlds[&0]).unwrap();
        s.worlds.insert(0, candidate);
    })
    .await;
    drain(&mut a);
    drain(&mut b);
    let before_a = character(&h.db, A).unwrap().unwrap();
    let before_b = character(&h.db, B).unwrap().unwrap();
    h.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_equipment BEFORE UPDATE ON build100_characters BEGIN SELECT RAISE(ABORT,'equipment clamp failure'); END;").unwrap();
    let error = h
        .input(
            a.session,
            equipment_swap((a.avatar, 3, 0xa27), (a.avatar, 5, -1)),
        )
        .await
        .unwrap_err();
    assert!(error.contains("equipment clamp failure"));
    assert!(drain(&mut a).is_empty());
    assert!(drain(&mut b).is_empty());
    assert_eq!(character(&h.db, A).unwrap().unwrap(), before_a);
    assert_eq!(character(&h.db, B).unwrap().unwrap(), before_b);
    let db = h.db.clone();
    assert!(h.task.await.unwrap().is_err());
    db.lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_equipment;")
        .unwrap();
    let h = Harness::normal_with_db(db);
    let mut a = h.join(A, 0).await.unwrap();
    let out = h.load(&mut a).await;
    let stats = equipment_wire(&out, 10, a.avatar);
    assert_eq!((stats[&0], stats[&1], stats[&11]), (140, 140, 0xa27));
    assert_eq!(character(&h.db, A).unwrap().unwrap(), before_a);
    h.stop().await;
}

async fn seed_resources(h: &Harness, version: Option<u8>) {
    let mut p = Character::new(
        &h.inspect(|s| Content {
            objects: s.content.objects.clone(),
        })
        .await,
        0x30e,
        "Resources".into(),
        1,
    )
    .unwrap();
    p.stats.extend([(1, 50), (4, 50)]);
    let mut value = serde_json::to_value(&p).unwrap();
    if let Some(version) = version {
        value["resources_version"] = version.into();
    } else {
        value.as_object_mut().unwrap().remove("resources_version");
    }
    if version != Some(1) {
        value.as_object_mut().unwrap().remove("resources_carry");
    }
    h.db.lock()
        .unwrap()
        .execute(
            "INSERT INTO build100_characters(guid,state) VALUES(?,?)",
            [A, &value.to_string()],
        )
        .unwrap();
}
fn resource_carry_bits(p: &Character) -> [u64; 4] {
    let c = p.resources_carry.unwrap();
    [
        c.hp_remainder.to_bits(),
        c.mp_remainder.to_bits(),
        c.hp_roundoff.to_bits(),
        c.mp_roundoff.to_bits(),
    ]
}
async fn tick_resources_duration(h: &Harness, duration: Duration) -> Result<(), String> {
    let (reply, result) = oneshot::channel();
    h.commands
        .send(Command::Tick(duration, reply))
        .await
        .unwrap();
    result.await.unwrap()
}

#[tokio::test]
async fn resources_runtime_actor_migrates_once_committed_before_output_without_offline_gain() {
    for version in [None, Some(0)] {
        let h = Harness::normal();
        seed_resources(&h, version).await;
        h.db.lock().unwrap().execute_batch("CREATE TABLE resource_migrations(count INTEGER); CREATE TRIGGER count_resource_migration AFTER UPDATE ON build100_characters WHEN json_extract(OLD.state,'$.resources_version') IS NOT 1 AND json_extract(NEW.state,'$.resources_version') = 1 BEGIN INSERT INTO resource_migrations VALUES(1); END;").unwrap();
        let mut a = h.join(A, 0).await.unwrap();
        drain(&mut a);
        // Unattached elapsed simulation time is not character regeneration time.
        tick_resources_duration(&h, Duration::from_secs(3))
            .await
            .unwrap();
        assert!(
            character(&h.db, A)
                .unwrap()
                .unwrap()
                .resources_carry
                .is_none()
        );
        let out = h.load(&mut a).await;
        let migrated = character(&h.db, A).unwrap().unwrap();
        assert_eq!((migrated.stats[&1], migrated.stats[&4]), (50, 50));
        assert_eq!(migrated.resources_version, 1);
        assert_eq!(resource_carry_bits(&migrated), [0; 4]);
        assert_eq!(equipment_wire(&out, 10, a.avatar)[&1], 50);
        h.detach(a.session).await;
        // Attach still persists normally, but the known-legacy transition occurs only once.
        a = h.join(A, 0).await.unwrap();
        h.load(&mut a).await;
        assert_eq!(character(&h.db, A).unwrap().unwrap(), migrated);
        assert_eq!(
            h.db.lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM resource_migrations", [], |row| row
                    .get::<_, i32>(0))
                .unwrap(),
            1
        );
        h.stop().await;
    }
}

#[tokio::test]
async fn resources_runtime_actor_fractional_only_commits_reconnect_restart_and_exact_duration() {
    let h = Harness::normal();
    seed_resources(&h, Some(1)).await;
    let mut a = h.join(A, 0).await.unwrap();
    h.load(&mut a).await;
    let before = character(&h.db, A).unwrap().unwrap();
    // Duration's original f64 conversion, not 0.1f32: HP increment is 0.25 plus its product low word.
    tick_resources_duration(&h, Duration::from_millis(100))
        .await
        .unwrap();
    assert!(
        drain(&mut a).is_empty(),
        "fraction-only updates must not add a Tick"
    );
    let saved = character(&h.db, A).unwrap().unwrap();
    assert_eq!(saved.stats, before.stats);
    let carry = saved.resources_carry.unwrap();
    assert_eq!(carry.hp_remainder, 0.25);
    assert_eq!(carry.hp_roundoff, 1.3877787807814457e-17);
    assert_eq!(carry.mp_remainder, (0.8 + 12. / 16.7) * 0.1);
    assert_ne!(carry.mp_roundoff, 0.);
    assert_eq!(
        h.inspect(move |s| resource_carry_bits(s.worlds[&0].character(a.avatar)))
            .await,
        resource_carry_bits(&saved)
    );
    h.detach(a.session).await;
    tick_resources_duration(&h, Duration::from_secs(10))
        .await
        .unwrap();
    a = h.join(A, 0).await.unwrap();
    h.load(&mut a).await;
    assert_eq!(character(&h.db, A).unwrap().unwrap(), saved);
    assert_eq!(
        h.inspect(move |s| resource_carry_bits(s.worlds[&0].character(a.avatar)))
            .await,
        resource_carry_bits(&saved)
    );
    h.detach(a.session).await;
    let db = h.db.clone();
    h.stop().await;
    let h = Harness::normal_with_db(db);
    tick_resources_duration(&h, Duration::from_secs(3600))
        .await
        .unwrap();
    let mut a = h.join(A, 0).await.unwrap();
    h.load(&mut a).await;
    assert_eq!(character(&h.db, A).unwrap().unwrap(), saved);
    assert_eq!(
        h.inspect(move |s| resource_carry_bits(s.worlds[&0].character(a.avatar)))
            .await,
        resource_carry_bits(&saved)
    );
    tick_resources_duration(&h, Duration::from_millis(100))
        .await
        .unwrap();
    assert_eq!(
        character(&h.db, A)
            .unwrap()
            .unwrap()
            .resources_carry
            .unwrap()
            .hp_remainder,
        0.5
    );
    let out = drain(&mut a);
    assert_eq!(out.iter().filter(|p| p.id == 13).count(), 1);
    h.input(
        a.session,
        packet(5, |w| {
            w.f32(saved.x).f32(saved.y);
        }),
    )
    .await
    .unwrap();
    assert!(
        drain(&mut a).is_empty(),
        "empty Move must not produce feedback"
    );
    h.stop().await;
}

#[tokio::test]
async fn resources_runtime_actor_fractional_sql_failure_no_output_no_mutation_and_fatal() {
    let h = Harness::normal();
    seed_resources(&h, Some(1)).await;
    let mut a = h.join(A, 0).await.unwrap();
    h.load(&mut a).await;
    let (mut b, _) = h.enter_route(B, 0).await;
    drain(&mut a);
    let before = character(&h.db, A).unwrap().unwrap();
    let before_b = character(&h.db, B).unwrap().unwrap();
    h.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_resources BEFORE UPDATE ON build100_characters BEGIN SELECT RAISE(ABORT,'fractional resources commit failure'); END;").unwrap();
    // Inspect the production tick/commit before its run-loop fatal-storage boundary.
    let (error, unchanged) = h
        .inspect(move |s| {
            let before = s.worlds[&0]
                .players
                .iter()
                .map(|(&id, a)| (id, a.character.clone()))
                .collect::<BTreeMap<_, _>>();
            let error = s.tick(Duration::from_millis(50)).unwrap_err();
            let after = s.worlds[&0]
                .players
                .iter()
                .map(|(&id, a)| (id, a.character.clone()))
                .collect::<BTreeMap<_, _>>();
            (format!("{error:#}"), before == after)
        })
        .await;
    assert!(error.contains("fractional resources commit failure"));
    assert!(unchanged);
    assert!(drain(&mut a).is_empty());
    assert!(drain(&mut b).is_empty());
    assert_eq!(character(&h.db, A).unwrap().unwrap(), before);
    assert_eq!(character(&h.db, B).unwrap().unwrap(), before_b);
    let error = tick_resources_duration(&h, Duration::from_millis(50))
        .await
        .unwrap_err();
    assert!(error.contains("fractional resources commit failure"));
    assert!(drain(&mut a).is_empty());
    assert!(drain(&mut b).is_empty());
    assert_eq!(character(&h.db, A).unwrap().unwrap(), before);
    assert_eq!(character(&h.db, B).unwrap().unwrap(), before_b);
    let db = h.db.clone();
    assert!(h.task.await.unwrap().is_err());
    db.lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_resources;")
        .unwrap();
    let h = Harness::normal_with_db(db);
    let mut a = h.join(A, 0).await.unwrap();
    h.load(&mut a).await;
    assert_eq!(character(&h.db, A).unwrap().unwrap(), before);
    h.stop().await;
}

#[tokio::test]
async fn resources_runtime_actor_migration_sql_failure_leaves_legacy_and_no_avatar_output() {
    let h = Harness::normal();
    seed_resources(&h, None).await;
    let before = character(&h.db, A).unwrap().unwrap();
    let mut a = h.join(A, 0).await.unwrap();
    drain(&mut a);
    h.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_resources BEFORE UPDATE ON build100_characters BEGIN SELECT RAISE(ABORT,'resources migration commit failure'); END;").unwrap();
    let error = h.input(a.session, load()).await.unwrap_err();
    assert!(error.contains("resources migration commit failure"));
    assert!(drain(&mut a).is_empty());
    assert_eq!(character(&h.db, A).unwrap().unwrap(), before);
    let db = h.db.clone();
    assert!(h.task.await.unwrap().is_err());
    db.lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_resources;")
        .unwrap();
    let h = Harness::normal_with_db(db);
    let mut a = h.join(A, 0).await.unwrap();
    h.load(&mut a).await;
    let p = character(&h.db, A).unwrap().unwrap();
    assert_eq!(p.resources_version, 1);
    assert_eq!(resource_carry_bits(&p), [0; 4]);
    assert_eq!(p.stats, before.stats);
    h.stop().await;
}

#[tokio::test]
async fn resources_runtime_actor_full_caps_no_fraction_writes_and_original_five_hz_feedback_guard()
{
    let h = Harness::normal();
    h.inspect(|s| {
        crate::world::shore_runtime_tests::isolate_population(s.worlds.get_mut(&0).unwrap())
    })
    .await;
    let (mut a, _) = h.enter_route(A, 0).await;
    let before = character(&h.db, A).unwrap().unwrap();
    h.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_full_resources BEFORE UPDATE ON build100_characters BEGIN SELECT RAISE(ABORT,'full resources must not change'); END;").unwrap();
    for _ in 0..20 {
        tick_resources_duration(&h, Duration::from_millis(50))
            .await
            .unwrap();
    }
    let out = drain(&mut a);
    assert_eq!(out.iter().filter(|p| p.id == 13).count(), 5);
    assert_eq!(character(&h.db, A).unwrap().unwrap(), before);
    h.input(
        a.session,
        packet(5, |w| {
            w.f32(before.x).f32(before.y);
        }),
    )
    .await
    .unwrap();
    assert!(drain(&mut a).is_empty());
    h.stop().await;
}

fn cloak_use(avatar: i32) -> Packet {
    packet(15, |w| {
        w.i32(avatar).u8(1).i32(avatar).f32(0.).f32(0.);
    })
}
async fn enter_cloak(h: &Harness, guid: &str, route: i32) -> Client {
    let mut a = h.join(guid, route).await.unwrap();
    h.input(
        a.session,
        packet(3, |w| {
            w.i16(0x300).utf("Rogue");
        }),
    )
    .await
    .unwrap();
    let out = drain(&mut a);
    a.avatar = Reader::new(&out.iter().find(|p| p.id == 1).unwrap().body)
        .i32()
        .unwrap();
    a
}
async fn seed_legacy_cloak(h: &Harness) {
    let c = h
        .inspect(|s| Content {
            objects: s.content.objects.clone(),
        })
        .await;
    let mut p = Character::new(&c, 0x300, "Legacy".into(), 1).unwrap();
    p.stats.extend([(1, 50), (4, 90)]);
    p.resources_carry = Some(world::ResourceCarry {
        hp_remainder: 0.25,
        mp_remainder: 0.125,
        hp_roundoff: 1e-18,
        mp_roundoff: 1e-18,
    });
    p.action_state = None;
    h.db.lock()
        .unwrap()
        .execute(
            "INSERT INTO build100_characters(guid,state) VALUES(?,?)",
            [A, &serde_json::to_string(&p).unwrap()],
        )
        .unwrap();
}

#[tokio::test]
async fn cloak_runtime_actor_paid_use_peer_rejection_and_exact_expiry_committed() {
    let h = Harness::normal();
    let mut a = enter_cloak(&h, A, 0).await;
    let (mut b, _) = h.enter_route(B, 0).await;
    drain(&mut a);
    tick_resources_duration(&h, Duration::from_millis(100))
        .await
        .unwrap();
    drain(&mut a);
    drain(&mut b);
    let before = character(&h.db, A).unwrap().unwrap();
    h.input(a.session, cloak_use(a.avatar)).await.unwrap();
    let paid = character(&h.db, A).unwrap().unwrap();
    assert_eq!(paid.stats[&4], 20);
    assert_eq!(paid.stats[&25], 4096);
    assert_eq!(resource_carry_bits(&paid), resource_carry_bits(&before));
    assert_eq!(paid.stats[&9], before.stats[&9]);
    for out in [drain(&mut a), drain(&mut b)] {
        assert_eq!(equipment_wire(&out, 13, a.avatar)[&29], 4096);
        assert_eq!(equipment_wire(&out, 13, a.avatar)[&4], 20);
    }
    assert!(
        h.input(a.session, cloak_use(a.avatar))
            .await
            .unwrap_err()
            .contains("cooldown")
    );
    assert_eq!(character(&h.db, A).unwrap().unwrap(), paid);
    assert!(drain(&mut a).is_empty());
    assert!(drain(&mut b).is_empty());
    assert!(
        h.input(b.session, cloak_use(b.avatar))
            .await
            .unwrap_err()
            .contains("spell target too distant")
    );
    assert!(drain(&mut a).is_empty());
    assert!(drain(&mut b).is_empty());
    let duration = h
        .inspect(|s| f64::from(s.content.get(0xa56).unwrap().effects[0].duration_seconds))
        .await;
    tick_resources_duration(&h, Duration::from_secs_f64(duration))
        .await
        .unwrap();
    let expired = character(&h.db, A).unwrap().unwrap();
    assert_eq!(expired.stats[&25], 0);
    for out in [drain(&mut a), drain(&mut b)] {
        assert_eq!(equipment_wire(&out, 13, a.avatar)[&29], 0);
    }
    h.stop().await;
}

#[tokio::test]
async fn cloak_runtime_actor_detach_restart_pauses_effect_and_cooldown_without_reset() {
    let h = Harness::normal();
    let mut a = enter_cloak(&h, A, 0).await;
    h.input(a.session, cloak_use(a.avatar)).await.unwrap();
    drain(&mut a);
    tick_resources_duration(&h, Duration::from_millis(250))
        .await
        .unwrap();
    drain(&mut a);
    let saved = character(&h.db, A).unwrap().unwrap();
    let db = h.db.clone();
    h.detach(a.session).await;
    h.stop().await;
    let h = Harness::normal_with_db(db);
    tick_resources_duration(&h, Duration::from_secs(3600))
        .await
        .unwrap();
    assert_eq!(character(&h.db, A).unwrap().unwrap(), saved);
    let mut a = h.join(A, 0).await.unwrap();
    let out = h.load(&mut a).await;
    assert_eq!(equipment_wire(&out, 10, a.avatar)[&29], 4096);
    assert_eq!(character(&h.db, A).unwrap().unwrap(), saved);
    assert!(
        h.input(a.session, cloak_use(a.avatar))
            .await
            .unwrap_err()
            .contains("cooldown")
    );
    tick_resources_duration(&h, Duration::from_millis(249))
        .await
        .unwrap();
    drain(&mut a);
    let at499 = character(&h.db, A).unwrap().unwrap();
    let state = serde_json::to_value(&at499).unwrap();
    assert_eq!(state["action_state"]["ability_used_seconds"], 0.);
    tick_resources_duration(&h, Duration::from_millis(1))
        .await
        .unwrap();
    drain(&mut a);
    let at500 = character(&h.db, A).unwrap().unwrap();
    assert!(
        serde_json::to_value(&at500).unwrap()["action_state"]["ability_used_seconds"].is_null()
    );
    assert_eq!(at500.stats[&25], 4096);
    assert_eq!(at500.stats[&4], 20); // cooldown continuation never refunds/funds the next use
    assert!(
        h.input(a.session, cloak_use(a.avatar))
            .await
            .unwrap_err()
            .contains("insufficient MP")
    );
    h.stop().await;
}

#[tokio::test]
async fn cloak_runtime_actor_portal_transfer_pauses_remaining_state_and_no_cooldown_bypass() {
    let h = Harness::normal();
    let mut a = enter_cloak(&h, A, -1).await;
    h.input(a.session, cloak_use(a.avatar)).await.unwrap();
    drain(&mut a);
    tick_resources_duration(&h, Duration::from_millis(125))
        .await
        .unwrap();
    drain(&mut a);
    h.near_exit(&a).await;
    let before = character(&h.db, A).unwrap().unwrap();
    let mut next = h.join(A, 0).await.unwrap();
    h.load(&mut next).await;
    let after = character(&h.db, A).unwrap().unwrap();
    assert_eq!(after.action_state, before.action_state);
    assert_eq!(resource_carry_bits(&after), resource_carry_bits(&before));
    assert_eq!(after.stats, before.stats);
    assert!(after.tutorial_complete);
    assert!(
        h.input(next.session, cloak_use(next.avatar))
            .await
            .unwrap_err()
            .contains("cooldown")
    );
    h.detach(a.session).await;
    assert_eq!(character(&h.db, A).unwrap().unwrap(), after);
    h.stop().await;
}

#[tokio::test]
async fn cloak_runtime_actor_migration_once_and_idle_expired_state_no_sql_writes() {
    let h = Harness::normal();
    seed_legacy_cloak(&h).await;
    let before = character(&h.db, A).unwrap().unwrap();
    h.db.lock().unwrap().execute_batch("CREATE TABLE cloak_migrations(count INTEGER); CREATE TRIGGER count_cloak AFTER UPDATE ON build100_characters WHEN json_type(OLD.state,'$.action_state') IS NULL AND json_extract(NEW.state,'$.action_state.version') = 1 BEGIN INSERT INTO cloak_migrations VALUES(1); END;").unwrap();
    let mut a = h.join(A, 0).await.unwrap();
    h.load(&mut a).await;
    let migrated = character(&h.db, A).unwrap().unwrap();
    assert_eq!(migrated.stats, before.stats);
    assert_eq!(migrated.resources_version, 1);
    assert_eq!(resource_carry_bits(&migrated), resource_carry_bits(&before));
    assert!(migrated.action_state.is_some());
    h.detach(a.session).await;
    let mut a = h.join(A, 0).await.unwrap();
    h.load(&mut a).await;
    assert_eq!(character(&h.db, A).unwrap().unwrap(), migrated);
    assert_eq!(
        h.db.lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM cloak_migrations", [], |r| r
                .get::<_, i32>(0))
            .unwrap(),
        1
    );
    h.stop().await;

    let h = Harness::normal();
    let mut a = enter_cloak(&h, A, 0).await;
    h.input(a.session, cloak_use(a.avatar)).await.unwrap();
    drain(&mut a);
    tick_resources_duration(&h, Duration::from_secs(59))
        .await
        .unwrap();
    drain(&mut a);
    let before = character(&h.db, A).unwrap().unwrap();
    assert_eq!(
        (before.stats[&1], before.stats[&4], before.stats[&25]),
        (before.stats[&0], 100, 0)
    );
    assert_eq!(
        serde_json::to_value(&before).unwrap()["action_state"]["conditions"]["now_seconds"],
        0.
    );
    h.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_cloak_idle BEFORE UPDATE ON build100_characters BEGIN SELECT RAISE(ABORT,'expired idle must not write'); END;").unwrap();
    for _ in 0..4 {
        tick_resources_duration(&h, Duration::from_millis(50))
            .await
            .unwrap();
    }
    drain(&mut a);
    h.input(
        a.session,
        packet(5, |w| {
            w.f32(before.x).f32(before.y);
        }),
    )
    .await
    .unwrap();
    assert!(drain(&mut a).is_empty());
    assert_eq!(character(&h.db, A).unwrap().unwrap(), before);
    h.stop().await;
}

#[tokio::test]
async fn cloak_runtime_actor_use_sql_failure_preserves_owner_peer_inventory_carry_and_fatal() {
    let h = Harness::normal();
    let mut a = enter_cloak(&h, A, 0).await;
    let (mut b, _) = h.enter_route(B, 0).await;
    tick_resources_duration(&h, Duration::from_millis(50))
        .await
        .unwrap();
    drain(&mut a);
    drain(&mut b);
    let before = character(&h.db, A).unwrap().unwrap();
    let peer = character(&h.db, B).unwrap().unwrap();
    h.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_cloak BEFORE UPDATE ON build100_characters BEGIN SELECT RAISE(ABORT,'paid cloak commit failure'); END;").unwrap();
    let (error, unchanged) = h
        .inspect(move |s| {
            let before = s.worlds[&0]
                .players
                .iter()
                .map(|(&id, a)| (id, a.character.clone()))
                .collect::<BTreeMap<_, _>>();
            let error = s
                .input(a.session, cloak_use(a.avatar), Instant::now())
                .unwrap_err();
            let after = s.worlds[&0]
                .players
                .iter()
                .map(|(&id, a)| (id, a.character.clone()))
                .collect::<BTreeMap<_, _>>();
            (format!("{error:#}"), before == after)
        })
        .await;
    assert!(unchanged && error.contains("paid cloak commit failure"));
    assert!(drain(&mut a).is_empty());
    assert!(drain(&mut b).is_empty());
    assert_eq!(character(&h.db, A).unwrap().unwrap(), before);
    assert_eq!(character(&h.db, B).unwrap().unwrap(), peer);
    assert!(
        h.input(a.session, cloak_use(a.avatar))
            .await
            .unwrap_err()
            .contains("paid cloak commit failure")
    );
    assert!(drain(&mut a).is_empty());
    assert!(drain(&mut b).is_empty());
    let db = h.db.clone();
    assert!(h.task.await.unwrap().is_err());
    assert_eq!(character(&db, A).unwrap().unwrap(), before);
    assert_eq!(character(&db, B).unwrap().unwrap(), peer);
}

#[tokio::test]
async fn cloak_runtime_actor_expiry_sql_failure_keeps_live_mask_and_fatal() {
    let h = Harness::normal();
    let mut a = enter_cloak(&h, A, 0).await;
    let (mut b, _) = h.enter_route(B, 0).await;
    h.input(a.session, cloak_use(a.avatar)).await.unwrap();
    drain(&mut a);
    drain(&mut b);
    let duration = h
        .inspect(|s| {
            Duration::from_secs_f64(f64::from(
                s.content.get(0xa56).unwrap().effects[0].duration_seconds,
            ))
        })
        .await;
    tick_resources_duration(&h, duration - Duration::from_millis(125))
        .await
        .unwrap();
    drain(&mut a);
    drain(&mut b);
    let before = character(&h.db, A).unwrap().unwrap();
    let peer = character(&h.db, B).unwrap().unwrap();
    let duration = Duration::from_millis(125);
    h.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_cloak BEFORE UPDATE ON build100_characters WHEN json_extract(OLD.state,'$.stats.\"25\"') = 4096 BEGIN SELECT RAISE(ABORT,'cloak expiry commit failure'); END;").unwrap();
    let (error, unchanged) = h
        .inspect(move |s| {
            let before = s.worlds[&0]
                .players
                .iter()
                .map(|(&id, a)| (id, a.character.clone()))
                .collect::<BTreeMap<_, _>>();
            let error = s.tick(duration).unwrap_err();
            let after = s.worlds[&0]
                .players
                .iter()
                .map(|(&id, a)| (id, a.character.clone()))
                .collect::<BTreeMap<_, _>>();
            (format!("{error:#}"), before == after)
        })
        .await;
    assert!(unchanged && error.contains("cloak expiry commit failure"));
    // Ping is session housekeeping, not a candidate MP/mask publication.
    for out in [drain(&mut a), drain(&mut b)] {
        assert!(!out.iter().any(|p| matches!(p.id, 9 | 10 | 11 | 13)));
    }
    assert_eq!(character(&h.db, A).unwrap().unwrap(), before);
    assert_eq!(character(&h.db, B).unwrap().unwrap(), peer);
    // The unchanged live world is driven to expiry again through the real fatal run boundary.
    assert!(
        tick_resources_duration(&h, duration)
            .await
            .unwrap_err()
            .contains("cloak expiry commit failure")
    );
    let db = h.db.clone();
    assert!(h.task.await.unwrap().is_err());
    assert_eq!(character(&db, A).unwrap().unwrap(), before);
    assert_eq!(character(&db, B).unwrap().unwrap(), peer);
}

#[tokio::test]
async fn cloak_runtime_actor_migration_sql_failure_no_partial_attach_or_peer_output_and_fatal() {
    let h = Harness::normal();
    seed_legacy_cloak(&h).await;
    let (mut b, _) = h.enter_route(B, 0).await;
    let before = character(&h.db, A).unwrap().unwrap();
    let peer = character(&h.db, B).unwrap().unwrap();
    let mut a = h.join(A, 0).await.unwrap();
    drain(&mut a);
    h.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_cloak BEFORE UPDATE ON build100_characters BEGIN SELECT RAISE(ABORT,'cloak migration commit failure'); END;").unwrap();
    let (error, unchanged) = h
        .inspect(move |s| {
            let error = s.input(a.session, load(), Instant::now()).unwrap_err();
            (
                format!("{error:#}"),
                s.worlds[&0].players.len() == 1 && s.sessions[&a.session].avatar.is_none(),
            )
        })
        .await;
    assert!(unchanged && error.contains("cloak migration commit failure"));
    assert!(drain(&mut a).is_empty());
    assert!(drain(&mut b).is_empty());
    assert_eq!(character(&h.db, A).unwrap().unwrap(), before);
    assert_eq!(character(&h.db, B).unwrap().unwrap(), peer);
    assert!(
        h.input(a.session, load())
            .await
            .unwrap_err()
            .contains("cloak migration commit failure")
    );
    let db = h.db.clone();
    assert!(h.task.await.unwrap().is_err());
    assert_eq!(character(&db, A).unwrap().unwrap(), before);
    assert_eq!(character(&db, B).unwrap().unwrap(), peer);
}

#[tokio::test]
async fn cloak_wire_actor_saved_alias29_rejected_before_attach_commit_or_peer_output() {
    let h = Harness::normal();
    seed_legacy_cloak(&h).await;
    let mut bad = character(&h.db, A).unwrap().unwrap();
    bad.stats.insert(29, 0);
    h.db.lock()
        .unwrap()
        .execute(
            "UPDATE build100_characters SET state=? WHERE guid=?",
            [serde_json::to_string(&bad).unwrap(), A.into()],
        )
        .unwrap();
    let (mut b, _) = h.enter_route(B, 0).await;
    let peer = character(&h.db, B).unwrap().unwrap();
    let mut a = h.join(A, 0).await.unwrap();
    drain(&mut a);
    let error = h.input(a.session, load()).await.unwrap_err();
    assert!(error.contains("wire condition alias stat29"), "{error}");
    assert!(drain(&mut a).is_empty());
    assert!(drain(&mut b).is_empty());
    assert_eq!(character(&h.db, A).unwrap().unwrap(), bad);
    assert_eq!(character(&h.db, B).unwrap().unwrap(), peer);
    assert_eq!(h.inspect(|s| s.worlds[&0].players.len()).await, 1);
    assert!(!h.task.is_finished());
    h.stop().await;
}

fn weapon_native_shot(avatar: i32, id: u8, origin: (f32, f32), angle: f32) -> Packet {
    packet(8, |w| {
        w.u8(id)
            .i32(avatar)
            .u8(41)
            .i16(0xa97)
            .f32(origin.0)
            .f32(origin.1)
            .f32(angle)
            .i16(32767);
    })
}
fn weapon_peer_shots(packets: &[Packet]) -> Vec<(u8, i32, u8, i16)> {
    packets
        .iter()
        .filter(|p| p.id == 8)
        .map(|p| {
            let mut r = Reader::new(&p.body);
            (
                r.u8().unwrap(),
                r.i32().unwrap(),
                r.u8().unwrap(),
                r.i16().unwrap(),
            )
        })
        .collect()
}

#[tokio::test]
async fn weapon_runtime_actor_buffered_final_invalid_owner_then_complete_peer_once_all_no_echo() {
    let h = Harness::normal();
    let (mut a, _) = h.enter_route(A, 0).await;
    let (mut b, _) = h.enter_route(B, 0).await;
    drain(&mut a);
    drain(&mut b);
    let saved = character(&h.db, A).unwrap().unwrap();
    let origin = (saved.x + 0.3, saved.y);
    h.input(a.session, weapon_native_shot(a.avatar, 5, origin, 0.0))
        .await
        .unwrap();
    assert!(drain(&mut a).is_empty() && drain(&mut b).is_empty());
    assert_eq!(character(&h.db, A).unwrap().unwrap(), saved);
    assert!(
        h.input(b.session, weapon_native_shot(a.avatar, 6, origin, 0.0))
            .await
            .is_err()
    );
    assert!(
        h.input(a.session, weapon_native_shot(a.avatar, 6, origin, 0.1))
            .await
            .is_err()
    );
    assert!(drain(&mut a).is_empty() && drain(&mut b).is_empty());
    h.input(a.session, weapon_native_shot(a.avatar, 6, origin, 0.0))
        .await
        .unwrap();
    assert!(weapon_peer_shots(&drain(&mut a)).is_empty());
    assert_eq!(
        weapon_peer_shots(&drain(&mut b)),
        [(5, a.avatar, 0, 0xa97), (6, a.avatar, 0, 0xa97)]
    );
    assert!(drain(&mut b).is_empty());
    assert_eq!(character(&h.db, A).unwrap().unwrap(), saved);
    h.stop().await;
}

#[tokio::test]
async fn weapon_runtime_actor_buffered_detach_new_owner_has_empty_group_and_death_cannot_fire() {
    let h = Harness::normal();
    h.inspect(|s| {
        crate::world::shore_runtime_tests::isolate_population(s.worlds.get_mut(&0).unwrap())
    })
    .await;
    let (mut old, _) = h.enter_route(A, 0).await;
    let (mut peer, _) = h.enter_route(B, 0).await;
    drain(&mut old);
    drain(&mut peer);
    let p = character(&h.db, A).unwrap().unwrap();
    let origin = (p.x + 0.3, p.y);
    h.input(old.session, weapon_native_shot(old.avatar, 9, origin, 0.0))
        .await
        .unwrap();
    h.detach(old.session).await;
    let mut a = h.join(A, 0).await.unwrap();
    h.load(&mut a).await;
    drain(&mut peer);
    assert!(
        h.input(old.session, weapon_native_shot(old.avatar, 10, origin, 0.0))
            .await
            .is_err()
    );
    h.input(a.session, weapon_native_shot(a.avatar, 10, origin, 0.0))
        .await
        .unwrap();
    assert!(drain(&mut a).is_empty() && drain(&mut peer).is_empty());
    let lava = h
        .inspect(|s| {
            let map = s.worlds[&0].map.as_ref().unwrap();
            let (w, _) = map.dimensions();
            let i = map.tiles().iter().position(|t| t.ground == 0x70).unwrap();
            (
                (i % usize::from(w)) as f32 + 0.5,
                (i / usize::from(w)) as f32 + 0.5,
            )
        })
        .await;
    h.place(a.session, lava).await;
    h.inspect(move |s| {
        let mut room = s.worlds[&0].clone();
        room.character_mut(a.avatar).stats.insert(1, 1);
        s.commit(&room, &s.worlds[&0]).unwrap();
        s.worlds.insert(0, room);
    })
    .await;
    h.tick(0.5).await.unwrap();
    assert!(!character(&h.db, A).unwrap().unwrap().alive);
    assert!(weapon_peer_shots(&drain(&mut a)).is_empty());
    assert!(weapon_peer_shots(&drain(&mut peer)).is_empty());
    h.input(a.session, weapon_native_shot(a.avatar, 11, origin, 0.0))
        .await
        .unwrap();
    assert!(drain(&mut a).is_empty() && drain(&mut peer).is_empty());
    h.stop().await;
}

#[tokio::test]
async fn weapon_runtime_actor_sql_begin_failure_cannot_install_buffered_bullets_or_publish() {
    let h = Harness::normal();
    let (mut a, _) = h.enter_route(A, 0).await;
    let (mut b, _) = h.enter_route(B, 0).await;
    drain(&mut a);
    drain(&mut b);
    let saved = character(&h.db, A).unwrap().unwrap();
    let origin = (saved.x + 0.3, saved.y);
    h.input(a.session, weapon_native_shot(a.avatar, 8, origin, 0.0))
        .await
        .unwrap();
    // Shots need no durable writes, but even their empty transaction must commit before installation/output.
    h.db.lock()
        .unwrap()
        .execute_batch("BEGIN IMMEDIATE")
        .unwrap();
    let error = h
        .inspect(move |s| {
            s.input(
                a.session,
                weapon_native_shot(a.avatar, 9, origin, 0.0),
                Instant::now(),
            )
            .expect_err("SQL failure")
            .to_string()
        })
        .await;
    assert!(error.contains("fatal build100 persistence failure"));
    assert!(drain(&mut a).is_empty() && drain(&mut b).is_empty());
    assert_eq!(character(&h.db, A).unwrap().unwrap(), saved);
    h.db.lock().unwrap().execute_batch("ROLLBACK").unwrap();
    h.input(a.session, weapon_native_shot(a.avatar, 9, origin, 0.0))
        .await
        .unwrap();
    assert!(drain(&mut a).is_empty());
    assert_eq!(
        weapon_peer_shots(&drain(&mut b)),
        [(8, a.avatar, 0, 0xa97), (9, a.avatar, 0, 0xa97)]
    );
    h.stop().await;
}

#[tokio::test]
async fn weapon_runtime_actor_delayed_owner_uses_ingress_marks_and_same_frame_pair_not_processing_spacing()
 {
    let h = Harness::normal();
    let (mut a, _) = h.enter_route(A, 0).await;
    let (mut b, _) = h.enter_route(B, 0).await;
    h.inspect(move |s| {
        let mut room = s.worlds[&0].clone();
        room.character_mut(a.avatar).stats.insert(28, 75); // Source interval125ms exactly.
        s.commit(&room, &s.worlds[&0]).unwrap();
        s.worlds.insert(0, room);
    })
    .await;
    drain(&mut a);
    drain(&mut b);
    let p = character(&h.db, A).unwrap().unwrap();
    let origin = (p.x + 0.3, p.y);
    let receipt = Instant::now() - Duration::from_secs(1);
    let mut replies = vec![];
    for (id, offset) in [(1, 0), (2, 0), (3, 125), (4, 125)] {
        let (reply, result) = oneshot::channel();
        h.commands
            .send(Command::Input {
                session: a.session,
                packet: weapon_native_shot(a.avatar, id, origin, 0.0),
                received: receipt + Duration::from_millis(offset),
                reply,
            })
            .await
            .unwrap();
        replies.push(result);
    }
    for reply in replies {
        reply.await.unwrap().unwrap();
    }
    assert!(drain(&mut a).is_empty());
    assert_eq!(
        weapon_peer_shots(&drain(&mut b)),
        [
            (1, a.avatar, 0, 0xa97),
            (2, a.avatar, 0, 0xa97),
            (3, a.avatar, 0, 0xa97),
            (4, a.avatar, 0, 0xa97)
        ]
    );
    let (reply, result) = oneshot::channel();
    h.commands
        .send(Command::Input {
            session: a.session,
            packet: weapon_native_shot(a.avatar, 5, origin, 0.0),
            received: receipt,
            reply,
        })
        .await
        .unwrap();
    assert!(
        result
            .await
            .unwrap()
            .unwrap_err()
            .contains("clock moved backwards")
    );
    assert!(drain(&mut a).is_empty() && drain(&mut b).is_empty());
    h.stop().await;
}

fn inventory_drop_packet(avatar: i32, slot: u8, item: i16) -> Packet {
    packet(19, |w| {
        w.i32(avatar).u8(slot).i16(item);
    })
}

#[tokio::test]
async fn inventory_drop_actor_invalid_and_queued_duplicate_never_publish_or_multiply() {
    let h = Harness::normal();
    let (mut a, _) = h.enter_route(A, 0).await;
    let (mut b, _) = h.enter_route(B, 0).await;
    drain(&mut a);
    drain(&mut b);
    let before = character(&h.db, A).unwrap().unwrap();
    let peer = character(&h.db, B).unwrap().unwrap();
    let state = h
        .inspect(|s| crate::world::progression_runtime_tests::reward_state(&s.worlds[&0]))
        .await;
    for p in [
        Packet {
            id: 19,
            body: vec![0; 6],
        },
        Packet {
            id: 19,
            body: vec![0; 8],
        },
        inventory_drop_packet(b.avatar, 4, 0xa22),
        inventory_drop_packet(a.avatar, 255, 0xa22),
        inventory_drop_packet(a.avatar, 4, 0xa23),
        inventory_drop_packet(a.avatar, 5, -1),
    ] {
        assert!(h.input(a.session, p).await.is_err());
        assert!(drain(&mut a).is_empty() && drain(&mut b).is_empty());
        assert_eq!(character(&h.db, A).unwrap().unwrap(), before);
        assert_eq!(character(&h.db, B).unwrap().unwrap(), peer);
        assert_eq!(
            h.inspect(|s| crate::world::progression_runtime_tests::reward_state(&s.worlds[&0]))
                .await,
            state
        );
    }
    let mut replies = vec![];
    for _ in 0..2 {
        let (reply, result) = oneshot::channel();
        h.commands
            .send(Command::Input {
                received: Instant::now(),
                session: a.session,
                packet: inventory_drop_packet(a.avatar, 4, 0xa22),
                reply,
            })
            .await
            .unwrap();
        replies.push(result);
    }
    replies.remove(0).await.unwrap().unwrap();
    assert!(
        replies
            .remove(0)
            .await
            .unwrap()
            .unwrap_err()
            .contains("stale inventory drop")
    );
    let pa = drain(&mut a);
    let pb = drain(&mut b);
    assert_eq!(objects(&pa, 0x500).len(), 1);
    assert_eq!(objects(&pa, 0x500), objects(&pb, 0x500));
    let mut expected = before;
    expected.stats.insert(12, -1);
    assert_eq!(character(&h.db, A).unwrap().unwrap(), expected);
    assert_eq!(character(&h.db, B).unwrap().unwrap(), peer);
    h.stop().await;
}

#[tokio::test]
async fn inventory_drop_actor_sql_failure_keeps_candidate_room_rng_ids_and_wire_then_retry() {
    let h = Harness::normal();
    let (mut a, _) = h.enter_route(A, 0).await;
    let (mut b, _) = h.enter_route(B, 0).await;
    drain(&mut a);
    drain(&mut b);
    let before = character(&h.db, A).unwrap().unwrap();
    let peer = character(&h.db, B).unwrap().unwrap();
    h.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_drop BEFORE UPDATE ON build100_characters BEGIN SELECT RAISE(ABORT,'drop commit failure'); END;").unwrap();
    let error = h
        .inspect(move |s| {
            use crate::world::progression_runtime_tests::{bags, reward_state};
            let state = reward_state(&s.worlds[&0]);
            let loot = bags(&s.worlds[&0]);
            let owner = s.worlds[&0].character(a.avatar).clone();
            let peer = s.worlds[&0].character(b.avatar).clone();
            let error = s
                .input(
                    a.session,
                    inventory_drop_packet(a.avatar, 4, 0xa22),
                    Instant::now(),
                )
                .unwrap_err();
            assert_eq!(reward_state(&s.worlds[&0]), state);
            assert_eq!(bags(&s.worlds[&0]), loot);
            assert_eq!(s.worlds[&0].character(a.avatar), &owner);
            assert_eq!(s.worlds[&0].character(b.avatar), &peer);
            format!("{error:#}")
        })
        .await;
    assert!(error.contains("drop commit failure"));
    assert!(drain(&mut a).is_empty() && drain(&mut b).is_empty());
    assert_eq!(character(&h.db, A).unwrap().unwrap(), before);
    assert_eq!(character(&h.db, B).unwrap().unwrap(), peer);
    h.db.lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_drop;")
        .unwrap();
    h.input(a.session, inventory_drop_packet(a.avatar, 4, 0xa22))
        .await
        .unwrap();
    let pa = drain(&mut a);
    assert_eq!(objects(&pa, 0x500).len(), 1);
    assert_eq!(objects(&pa, 0x500), objects(&drain(&mut b), 0x500));
    h.stop().await;
}

#[tokio::test]
async fn inventory_drop_actor_fatal_sql_failure_no_equipment_clamp_or_success_on_either_peer() {
    let h = Harness::normal();
    let (mut a, _) = h.enter_route(A, 0).await;
    let (mut b, _) = h.enter_route(B, 0).await;
    h.inspect(move |s| {
        let mut room = s.worlds[&0].clone();
        room.character_mut(a.avatar)
            .stats
            .extend([(11, 0xa27), (1, 140)]);
        s.commit(&room, &s.worlds[&0]).unwrap();
        s.worlds.insert(0, room);
    })
    .await;
    drain(&mut a);
    drain(&mut b);
    let before = character(&h.db, A).unwrap().unwrap();
    let peer = character(&h.db, B).unwrap().unwrap();
    h.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_drop BEFORE UPDATE ON build100_characters BEGIN SELECT RAISE(ABORT,'fatal drop failure'); END;").unwrap();
    assert!(
        h.input(a.session, inventory_drop_packet(a.avatar, 3, 0xa27))
            .await
            .unwrap_err()
            .contains("fatal drop failure")
    );
    assert!(h.task.await.unwrap().is_err());
    assert!(drain(&mut a).is_empty() && drain(&mut b).is_empty());
    assert!(a.output.is_closed() && b.output.is_closed());
    assert_eq!(character(&h.db, A).unwrap().unwrap(), before);
    assert_eq!(character(&h.db, B).unwrap().unwrap(), peer);
    h.db.lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_drop;")
        .unwrap();
    let h = Harness::normal_with_db(h.db);
    let mut a = h.join(A, 0).await.unwrap();
    let out = h.load(&mut a).await;
    let stats = equipment_wire(&out, 10, a.avatar);
    assert_eq!((stats[&0], stats[&1], stats[&11]), (140, 140, 0xa27));
    assert!(objects(&out, 0x500).is_empty());
    h.stop().await;
}

#[tokio::test]
async fn inventory_drop_actor_peer_equipment_projection_pickup_reload_and_disk_restart() {
    let path = std::env::temp_dir().join(format!("build100-drop-{}.sqlite", rand::random::<u64>()));
    let db = Arc::new(Mutex::new(database(&path).unwrap()));
    let h = Harness::normal_with_db(db.clone());
    let (mut a, _) = h.enter_route(A, 0).await;
    let (mut b, _) = h.enter_route(B, 0).await;
    h.inspect(move |s| {
        let mut room = s.worlds[&0].clone();
        crate::world::shore_runtime_tests::isolate_population(&mut room);
        room.character_mut(a.avatar)
            .stats
            .extend([(11, 0xa27), (10, 0xa5f), (1, 140), (4, 135)]);
        s.commit(&room, &s.worlds[&0]).unwrap();
        s.worlds.insert(0, room);
    })
    .await;
    drain(&mut a);
    drain(&mut b);
    h.input(a.session, inventory_drop_packet(a.avatar, 3, 0xa27))
        .await
        .unwrap();
    let ring = objects(&drain(&mut a), 0x500)[0];
    let peer_out = drain(&mut b);
    assert_eq!(objects(&peer_out, 0x500), [ring]);
    let stats = equipment_wire(&peer_out, 13, a.avatar);
    assert_eq!(
        (
            stats[&0], stats[&1], stats[&3], stats[&4], stats[&20], stats[&21]
        ),
        (100, 100, 140, 135, 17, 4)
    );
    h.input(a.session, inventory_drop_packet(a.avatar, 2, 0xa5f))
        .await
        .unwrap();
    let out = drain(&mut a);
    let robe = objects(&out, 0x500)[0];
    let stats = equipment_wire(&out, 13, a.avatar);
    assert_eq!(
        (
            stats[&0], stats[&1], stats[&3], stats[&4], stats[&20], stats[&21]
        ),
        (100, 100, 100, 100, 12, 0)
    );
    assert_eq!(equipment_wire(&drain(&mut b), 13, a.avatar), stats);
    h.detach(a.session).await;
    let mut a = h.join(A, 0).await.unwrap();
    let out = h.load(&mut a).await;
    assert_eq!(equipment_wire(&out, 10, a.avatar)[&11], -1);
    assert!(objects(&out, 0x500).contains(&ring));
    drain(&mut b);
    // Other worlds cannot resolve even a valid, currently occupied bag ID.
    let (mut other, _) = h
        .enter_route("cccccccccccccccccccccccccccccccccccccccc", -1)
        .await;
    assert!(
        h.input(
            other.session,
            equipment_swap((ring, 0, 0xa27), (other.avatar, 5, -1))
        )
        .await
        .is_err()
    );
    assert!(drain(&mut other).is_empty());
    // Queue both real owner commands: one pickup commits, the stale loser gains nothing.
    let mut results = vec![];
    for (session, avatar) in [(b.session, b.avatar), (a.session, a.avatar)] {
        let (reply, result) = oneshot::channel();
        h.commands
            .send(Command::Input {
                received: Instant::now(),
                session,
                packet: equipment_swap((ring, 0, 0xa27), (avatar, 5, -1)),
                reply,
            })
            .await
            .unwrap();
        results.push(result);
    }
    results.remove(0).await.unwrap().unwrap();
    assert!(
        results
            .remove(0)
            .await
            .unwrap()
            .unwrap_err()
            .contains("stale inventory")
    );
    h.input(
        b.session,
        equipment_swap((robe, 0, 0xa5f), (b.avatar, 6, -1)),
    )
    .await
    .unwrap();
    h.input(
        b.session,
        equipment_swap((b.avatar, 5, 0xa27), (b.avatar, 3, -1)),
    )
    .await
    .unwrap();
    h.input(
        b.session,
        equipment_swap((b.avatar, 6, 0xa5f), (b.avatar, 2, -1)),
    )
    .await
    .unwrap();
    let peer_stats = equipment_wire(&drain(&mut b), 13, b.avatar);
    // The first pickup Tick precedes re-equipping; inspect the final authoritative arrival instead.
    assert_eq!(peer_stats[&1], 100);
    let out = h
        .inspect(move |s| s.worlds[&0].arrival(b.avatar, &s.content).unwrap())
        .await;
    let projected = equipment_wire(&[out], 10, b.avatar);
    assert_eq!(
        (
            projected[&0],
            projected[&1],
            projected[&3],
            projected[&4],
            projected[&20],
            projected[&21]
        ),
        (140, 100, 140, 100, 17, 4)
    );
    h.tick(0.).await.unwrap();
    assert!(
        h.inspect(|s| crate::world::progression_runtime_tests::bags(&s.worlds[&0]))
            .await
            .is_empty()
    );
    let owner = character(&db, A).unwrap().unwrap();
    let peer = character(&db, B).unwrap().unwrap();
    assert_eq!(
        (
            owner.stats[&11],
            owner.stats[&10],
            owner.stats[&0],
            owner.stats[&3]
        ),
        (-1, -1, 100, 100)
    );
    assert_eq!(
        (
            peer.stats[&11],
            peer.stats[&10],
            peer.generation,
            peer.alive
        ),
        (0xa27, 0xa5f, 1, true)
    );
    h.stop().await;
    drop(db);
    let db = Arc::new(Mutex::new(database(&path).unwrap()));
    let h = Harness::normal_with_db(db.clone());
    for (guid, expected) in [(A, owner), (B, peer)] {
        let mut c = h.join(guid, 0).await.unwrap();
        let out = h.load(&mut c).await;
        assert!(objects(&out, 0x500).is_empty());
        assert_eq!(character(&db, guid).unwrap().unwrap(), expected);
        h.detach(c.session).await;
    }
    h.stop().await;
    drop(db);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn inventory_drop_actor_queued_use_swap_and_death_obey_authoritative_order() {
    for use_first in [true, false] {
        let h = Harness::normal();
        let (mut a, _) = h.enter_route(A, 0).await;
        h.inspect(move |s| {
            let mut room = s.worlds[&0].clone();
            room.character_mut(a.avatar).stats.insert(1, 50);
            s.commit(&room, &s.worlds[&0]).unwrap();
            s.worlds.insert(0, room);
        })
        .await;
        drain(&mut a);
        let first = if use_first {
            packet(15, |w| {
                w.i32(a.avatar).u8(4).i32(-1).f32(0.).f32(0.);
            })
        } else {
            equipment_swap((a.avatar, 4, 0xa22), (a.avatar, 5, -1))
        };
        let mut results = vec![];
        for p in [first, inventory_drop_packet(a.avatar, 4, 0xa22)] {
            let (reply, result) = oneshot::channel();
            h.commands
                .send(Command::Input {
                    received: Instant::now(),
                    session: a.session,
                    packet: p,
                    reply,
                })
                .await
                .unwrap();
            results.push(result);
        }
        results.remove(0).await.unwrap().unwrap();
        assert!(results.remove(0).await.unwrap().is_err());
        assert!(objects(&drain(&mut a), 0x500).is_empty());
        let p = character(&h.db, A).unwrap().unwrap();
        assert_eq!(
            (p.stats[&12], p.stats[&13]),
            (-1, if use_first { -1 } else { 0xa22 })
        );
        if !use_first {
            h.input(a.session, inventory_drop_packet(a.avatar, 5, 0xa22))
                .await
                .unwrap();
            assert_eq!(objects(&drain(&mut a), 0x500).len(), 1);
        }
        h.stop().await;
    }
    let h = Harness::normal();
    let (mut a, _) = h.enter_route(A, 0).await;
    h.inspect(move |s| {
        let room = s.worlds.get_mut(&0).unwrap();
        crate::world::shore_runtime_tests::isolate_population(room);
        crate::world::progression_runtime_tests::lethal_enemy_bullet(room, a.avatar);
    })
    .await;
    let (tick_reply, tick_result) = oneshot::channel();
    let (drop_reply, drop_result) = oneshot::channel();
    h.commands
        .send(Command::Tick(Duration::from_millis(50), tick_reply))
        .await
        .unwrap();
    h.commands
        .send(Command::Input {
            received: Instant::now(),
            session: a.session,
            packet: inventory_drop_packet(a.avatar, 4, 0xa22),
            reply: drop_reply,
        })
        .await
        .unwrap();
    tick_result.await.unwrap().unwrap();
    drop_result.await.unwrap().unwrap(); // Existing postdeath policy discards in-flight input.
    let dead = character(&h.db, A).unwrap().unwrap();
    assert!(!dead.alive);
    assert_eq!(dead.stats[&12], 0xa22);
    assert!(objects(&drain(&mut a), 0x500).is_empty());
    assert!(
        h.inspect(|s| crate::world::progression_runtime_tests::bags(&s.worlds[&0]))
            .await
            .is_empty()
    );
    h.stop().await;
}

#[tokio::test]
async fn inventory_drop_actor_buffered_weapon_and_stale_session_cannot_release_or_drop() {
    let h = Harness::normal();
    let (mut old, _) = h.enter_route(A, 0).await;
    let (mut b, _) = h.enter_route(B, 0).await;
    drain(&mut old);
    drain(&mut b);
    let saved = character(&h.db, A).unwrap().unwrap();
    let origin = (saved.x + 0.3, saved.y);
    h.input(old.session, weapon_native_shot(old.avatar, 1, origin, 0.))
        .await
        .unwrap();
    assert!(drain(&mut b).is_empty());
    h.input(old.session, inventory_drop_packet(old.avatar, 0, 0xa97))
        .await
        .unwrap();
    let out = drain(&mut b);
    assert_eq!(objects(&out, 0x500).len(), 1);
    assert!(weapon_peer_shots(&out).is_empty());
    assert!(
        h.input(old.session, weapon_native_shot(old.avatar, 2, origin, 0.))
            .await
            .is_err()
    );
    assert!(weapon_peer_shots(&drain(&mut b)).is_empty());
    h.detach(old.session).await;
    let mut current = h.join(A, 0).await.unwrap();
    h.load(&mut current).await;
    drain(&mut b);
    assert_ne!(old.avatar, current.avatar);
    assert!(
        h.input(old.session, inventory_drop_packet(old.avatar, 4, 0xa22))
            .await
            .is_err()
    );
    assert!(
        h.input(current.session, inventory_drop_packet(old.avatar, 4, 0xa22))
            .await
            .is_err()
    );
    assert!(drain(&mut current).is_empty() && drain(&mut b).is_empty());
    assert_eq!(character(&h.db, A).unwrap().unwrap().stats[&12], 0xa22);
    assert_eq!(
        h.inspect(|s| crate::world::progression_runtime_tests::bags(&s.worlds[&0]))
            .await
            .len(),
        1
    );
    h.stop().await;
}

#[tokio::test]
async fn inventory_drop_actor_full_or_closed_output_cannot_undo_commit_or_replay_transfer() {
    for closed in [false, true] {
        let h = Harness::normal();
        let (mut a, _) = h.enter_route(A, 0).await;
        let (mut b, _) = h.enter_route(B, 0).await;
        drain(&mut a);
        drain(&mut b);
        if closed {
            a.output.close();
        } else {
            h.inspect(move |s| {
                let output = &s.sessions[&a.session].output;
                for _ in 0..OUTPUT_CAPACITY {
                    output.try_send(vec![]).unwrap();
                }
                assert!(output.try_send(vec![]).is_err());
            })
            .await;
        }
        h.input(a.session, inventory_drop_packet(a.avatar, 4, 0xa22))
            .await
            .unwrap();
        assert_eq!(character(&h.db, A).unwrap().unwrap().stats[&12], -1);
        assert!(
            !h.inspect(move |s| s.sessions.contains_key(&a.session))
                .await
        );
        assert!(drain(&mut a).is_empty());
        assert_eq!(objects(&drain(&mut b), 0x500).len(), 1);
        let mut current = h.join(A, 0).await.unwrap();
        let out = h.load(&mut current).await;
        assert_eq!(equipment_wire(&out, 10, current.avatar)[&12], -1);
        assert_eq!(objects(&out, 0x500).len(), 1);
        assert!(
            h.input(
                current.session,
                inventory_drop_packet(current.avatar, 4, 0xa22)
            )
            .await
            .is_err()
        );
        assert_eq!(
            h.inspect(|s| crate::world::progression_runtime_tests::bags(&s.worlds[&0]))
                .await
                .len(),
            1
        );
        h.stop().await;
    }
}
