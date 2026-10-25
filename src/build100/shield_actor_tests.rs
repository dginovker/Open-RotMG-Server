use super::*;
use crate::world::shield_runtime_tests as shield;

#[tokio::test]
async fn shield_actor_paid_living_contacts_real_advisory_repair_once_only_and_normal_load() {
    let h = Harness::normal();
    let mut a = equipped_shooter(&h, A, 0, 0xa08).await;
    let (mut b, _) = h.enter(B).await;
    let avatar = a.avatar;
    let targets = h
        .inspect(move |s| {
            shield::contact_fixture(s.worlds.get_mut(&0).unwrap(), &s.content, avatar, 1000)
        })
        .await;
    h.tick(0.).await.unwrap();
    drain(&mut a);
    drain(&mut b);
    h.input(a.session, shot(&h, &a, 96).await).await.unwrap();
    assert!(drain(&mut a).is_empty());
    let peer = drain(&mut b);
    assert_eq!(shot_ids(&peer, avatar), [96]);
    let mut wire = Reader::new(&peer.iter().find(|p| p.id == 8).unwrap().body);
    assert_eq!(
        (
            wire.u8().unwrap(),
            wire.i32().unwrap(),
            wire.u8().unwrap(),
            wire.i16().unwrap()
        ),
        (96, avatar, 0, 0xa08)
    );
    let paid = character(&h.db, A).unwrap().unwrap();
    assert_eq!(
        (paid.stats[&4], paid.stats[&25], paid.stats[&21]),
        (20, 0, 0)
    );
    h.input(a.session, cast(avatar, 1, avatar, (0., 0.)))
        .await
        .unwrap();
    assert_eq!(character(&h.db, A).unwrap().unwrap(), paid);
    drain(&mut a);
    drain(&mut b);
    h.tick(0.05).await.unwrap();
    for client in [&mut a, &mut b] {
        let events = drain(client);
        let mut wire = Reader::new(&events.iter().find(|p| p.id == 9).unwrap().body);
        assert_eq!(
            (wire.i32().unwrap(), wire.u8().unwrap(), wire.u8().unwrap()),
            (targets[0], 7, 3)
        );
        assert!((30..70).contains(&wire.i16().unwrap()));
        assert_eq!((wire.u8().unwrap(), wire.i32().unwrap()), (0, avatar));
        wire.done().unwrap();
    }
    let first = h
        .inspect(move |s| shield::contact_state(&s.worlds[&0], targets))
        .await;
    assert!((931..=970).contains(&first.0[0]));
    assert_eq!(first.0[1], 1000);
    assert_eq!(first.1, [Some(3.), None]);
    assert_eq!(first.2, std::collections::BTreeSet::from([targets[0]]));
    for condition in [1, 7, 1] {
        let before = state(&h).await;
        h.input(
            a.session,
            world::prediction_runtime_tests::report(targets[0], avatar, 96, condition),
        )
        .await
        .unwrap();
        let events = drain(&mut a);
        if condition == 1 {
            assert_eq!(
                world::prediction_runtime_tests::target_packets(&events, targets[0]),
                [11, 10]
            );
            assert_eq!(quiver::stats(&events, 10, targets[0])[&29], 64);
        } else {
            assert!(events.is_empty());
        }
        assert!(drain(&mut b).is_empty());
        assert_eq!(state(&h).await, before);
    }
    h.tick(0.).await.unwrap();
    let repeated = h
        .inspect(move |s| shield::contact_state(&s.worlds[&0], targets))
        .await;
    assert_eq!(repeated, first);
    for client in [&mut a, &mut b] {
        assert!(drain(client).iter().all(|p| p.id != 9));
    }
    h.tick(0.05).await.unwrap();
    let second = h
        .inspect(move |s| shield::contact_state(&s.worlds[&0], targets))
        .await;
    assert_eq!(second.0, [first.0[0], first.0[0]]);
    assert_eq!(second.1, [Some(2.95), Some(3.)]);
    assert_eq!(second.2, std::collections::BTreeSet::from(targets));
    for client in [&mut a, &mut b] {
        let events = drain(client);
        assert_eq!(events.iter().filter(|p| p.id == 9).count(), 1);
        assert!(shot_ids(&events, avatar).is_empty());
        for target in targets {
            assert_eq!(quiver::stats(&events, 13, target)[&29], 64);
        }
    }
    assert_eq!(character(&h.db, A).unwrap().unwrap().stats[&6], 0);
    h.detach(a.session).await;
    let mut again = h.join(A, 0).await.unwrap();
    let loaded = h.load(&mut again).await;
    assert_eq!(quiver::stats(&loaded, 10, again.avatar)[&21], 2);
    assert_eq!(character(&h.db, A).unwrap().unwrap().stats[&21], 0);
    assert!(shot_ids(&loaded, avatar).is_empty());
    assert!(
        h.input(again.session, cast(again.avatar, 1, again.avatar, (0., 0.)))
            .await
            .is_err()
    );
    h.stop().await;
}

#[tokio::test]
async fn shield_actor_contact_begin_write_rollback_full_authority_and_fatal_dispatcher() {
    for begin in [false, true] {
        for dispatcher in [false, true] {
            let h = Harness::normal();
            let mut a = equipped_shooter(&h, A, 0, 0xa08).await;
            let (mut b, _) = h.enter(B).await;
            let avatar = a.avatar;
            let targets = h
                .inspect(move |s| {
                    world::progression_runtime_tests::guaranteed(&mut s.content, 1);
                    shield::contact_fixture(s.worlds.get_mut(&0).unwrap(), &s.content, avatar, 5)
                })
                .await;
            h.input(a.session, shot(&h, &a, 96).await).await.unwrap();
            h.input(a.session, cast(avatar, 1, avatar, (0., 0.)))
                .await
                .unwrap();
            assert_eq!(character(&h.db, A).unwrap().unwrap().stats[&4], 20);
            drain(&mut a);
            drain(&mut b);
            let before = state(&h).await;
            h.inspect(move |s| {
                let room = &s.worlds[&0];
                let mut candidate = room.clone();
                let events = candidate.advance(0.1, &s.content).unwrap();
                shield::assert_reward_contact(&candidate, targets);
                assert_eq!(events.iter().filter(|p| p.id == 9).count(), 2);
                assert!(candidate.character(avatar).stats[&6] > room.character(avatar).stats[&6]);
                quiver::assert_iron_reward_candidate(room, &candidate);
                assert_ne!(quiver::snapshot(room), quiver::snapshot(&candidate));
            })
            .await;
            h.db.lock().unwrap().execute_batch(if begin { "BEGIN IMMEDIATE" } else { "CREATE TRIGGER fail_shield_contact BEFORE UPDATE ON build100_characters BEGIN SELECT RAISE(ABORT,'injected shield contact write'); END;" }).unwrap();
            if dispatcher {
                assert!(
                    h.tick(0.1)
                        .await
                        .unwrap_err()
                        .contains("fatal build100 persistence")
                );
                assert_eq!(rows(&h.db), before.1);
                assert!(drain(&mut a).is_empty() && drain(&mut b).is_empty());
                h.db.lock()
                    .unwrap()
                    .execute_batch(if begin {
                        "ROLLBACK"
                    } else {
                        "DROP TRIGGER fail_shield_contact"
                    })
                    .unwrap();
                drop(h.commands);
                assert!(h.task.await.unwrap().unwrap_err().is::<StorageFailure>());
            } else {
                assert!(
                    h.inspect(|s| s
                        .tick(Duration::from_secs_f64(0.1))
                        .unwrap_err()
                        .is::<StorageFailure>())
                        .await
                );
                assert_eq!(state(&h).await, before);
                assert!(drain(&mut a).is_empty() && drain(&mut b).is_empty());
                h.db.lock()
                    .unwrap()
                    .execute_batch(if begin {
                        "ROLLBACK"
                    } else {
                        "DROP TRIGGER fail_shield_contact"
                    })
                    .unwrap();
                h.tick(0.1).await.unwrap();
                h.inspect(move |s| shield::assert_reward_contact(&s.worlds[&0], targets))
                    .await;
                for client in [&mut a, &mut b] {
                    let out = drain(client);
                    assert_eq!(out.iter().filter(|p| p.id == 9).count(), 1);
                    assert!(shot_ids(&out, avatar).is_empty());
                }
                let committed = character(&h.db, A).unwrap().unwrap();
                assert!(committed.stats[&6] > 0);
                assert!(committed.stats[&4] < 80);
                h.tick(0.).await.unwrap();
                assert_eq!(character(&h.db, A).unwrap().unwrap(), committed);
                for client in [&mut a, &mut b] {
                    assert!(drain(client).iter().all(|p| p.id != 9));
                }
                h.stop().await;
            }
        }
    }
}

#[tokio::test]
async fn shield_actor_failed_swap_drop_teleport_keep_paid_marker_until_success() {
    for operation in [14, 19, 28] {
        let h = Harness::normal();
        let mut a = equipped_shooter(&h, A, 0, 0xa08).await;
        let (mut b, _) = h.enter(B).await;
        h.input(a.session, shot(&h, &a, 1).await).await.unwrap();
        drain(&mut a);
        drain(&mut b);
        let before = state(&h).await;
        let input = match operation {
            14 => packet(14, |w| {
                w.i32(a.avatar).u8(1).i16(0xa08).i32(a.avatar).u8(5).i16(-1);
            }),
            19 => packet(19, |w| {
                w.i32(a.avatar).u8(1).i16(0xa08);
            }),
            28 => packet(28, |w| {
                w.i32(b.avatar);
            }),
            _ => unreachable!(),
        };
        h.db.lock()
            .unwrap()
            .execute_batch("BEGIN IMMEDIATE")
            .unwrap();
        let session = a.session;
        let copy = Packet {
            id: input.id,
            body: input.body.clone(),
        };
        assert!(
            h.inspect(move |s| {
                let now = Instant::now();
                s.input_at(session, copy, now, now)
                    .unwrap_err()
                    .is::<StorageFailure>()
            })
            .await
        );
        assert_eq!(state(&h).await, before);
        assert!(drain(&mut a).is_empty() && drain(&mut b).is_empty());
        h.db.lock().unwrap().execute_batch("ROLLBACK").unwrap();
        h.input(a.session, input).await.unwrap();
        assert!(
            h.input(a.session, cast(a.avatar, 1, a.avatar, (0., 0.)))
                .await
                .is_err()
        );
        assert_eq!(character(&h.db, A).unwrap().unwrap().stats[&4], 20);
        h.stop().await;
    }
}
