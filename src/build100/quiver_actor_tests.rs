use super::*;
use crate::world::{quiver_runtime_tests as quiver, spell_runtime_tests::cast};

async fn archer(h: &Harness, guid: &str, route: i32) -> Client {
    archer_with_item(h, guid, route, 0xa61).await
}
async fn archer_with_item(h: &Harness, guid: &str, route: i32, item: i16) -> Client {
    let mut a = h.join(guid, route).await.unwrap();
    h.input(
        a.session,
        packet(3, |w| {
            w.i16(0x307).utf("Archer");
        }),
    )
    .await
    .unwrap();
    a.avatar = Reader::new(&drain(&mut a).iter().find(|p| p.id == 1).unwrap().body)
        .i32()
        .unwrap();
    if item != 0xa61 {
        let session = a.session;
        h.inspect(move |s| {
            let world = s.sessions[&session].world;
            let before = &s.worlds[&world];
            let mut candidate = before.clone();
            candidate
                .character_mut(a.avatar)
                .stats
                .insert(9, i32::from(item));
            s.commit(&candidate, before).unwrap();
            s.worlds.insert(world, candidate);
        })
        .await;
    }
    a
}
async fn shot(h: &Harness, a: &Client, id: u8) -> Packet {
    let (session, avatar) = (a.session, a.avatar);
    h.inspect(move |s| quiver::fire(&s.worlds[&s.sessions[&session].world], avatar, id))
        .await
}
fn rows(db: &Database) -> Vec<(String, String)> {
    db.lock()
        .unwrap()
        .prepare("SELECT guid,state FROM build100_characters ORDER BY guid")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}
async fn state(h: &Harness) -> (String, Vec<(String, String)>) {
    (
        h.inspect(|s| quiver::snapshot(&s.worlds[&0])).await,
        rows(&h.db),
    )
}
fn shot_ids(out: &[Packet], owner: i32) -> Vec<u8> {
    out.iter()
        .filter(|p| p.id == 8)
        .filter_map(|p| {
            let mut r = Reader::new(&p.body);
            let id = r.u8().unwrap();
            (r.i32().unwrap() == owner).then_some(id)
        })
        .collect()
}
#[tokio::test]
async fn quiver_actor_one_payment_suppressed_caster_one_peer_shoot_then_mp_status_no_replay() {
    for item in [0xa61, 0xa63] {
        let remaining = if item == 0xa61 { 40 } else { 20 };
        let h = Harness::normal();
        let mut a = archer_with_item(&h, A, 0, item).await;
        let (mut b, _) = h.enter(B).await;
        drain(&mut a);
        drain(&mut b);
        h.input(a.session, shot(&h, &a, 96).await).await.unwrap();
        assert!(drain(&mut a).is_empty());
        let peer = drain(&mut b);
        assert_eq!(shot_ids(&peer, a.avatar), [96]);
        let mut wire = Reader::new(&peer.iter().find(|p| p.id == 8).unwrap().body);
        assert_eq!(
            (
                wire.u8().unwrap(),
                wire.i32().unwrap(),
                wire.u8().unwrap(),
                wire.i16().unwrap()
            ),
            (96, a.avatar, 0, item)
        );
        let paid = character(&h.db, A).unwrap().unwrap();
        assert_eq!(paid.stats[&4], remaining);
        let session = a.session;
        assert!(
            !h.inspect(move |s| s.deferred_weapons.contains_key(&session))
                .await
        );
        h.input(a.session, cast(a.avatar, 1, a.avatar, (0.5, 0.5)))
            .await
            .unwrap();
        assert_eq!(character(&h.db, A).unwrap().unwrap(), paid);
        let id = a.avatar;
        for c in [&mut a, &mut b] {
            let out = drain(c);
            assert!(shot_ids(&out, id).is_empty());
            assert!(out.iter().any(|p| p.id == 13));
        }
        let before = state(&h).await;
        assert!(
            h.input(a.session, cast(a.avatar, 1, a.avatar, (0., 0.)))
                .await
                .is_err()
        );
        assert_eq!(state(&h).await, before);
        assert!(drain(&mut a).is_empty() && drain(&mut b).is_empty());
        h.detach(a.session).await;
        let mut again = h.join(A, 0).await.unwrap();
        let loaded = h.load(&mut again).await;
        let bonus = if item == 0xa63 { 2 } else { 0 };
        assert_eq!(
            quiver::stats(&loaded, 10, again.avatar)[&28],
            paid.stats[&28] + bonus
        );
        assert_eq!(
            character(&h.db, A).unwrap().unwrap().stats[&28],
            paid.stats[&28]
        );
        assert_eq!(character(&h.db, A).unwrap().unwrap().stats[&4], remaining);
        assert!(
            h.input(again.session, cast(again.avatar, 1, again.avatar, (0., 0.)))
                .await
                .is_err()
        );
        h.stop().await;
    }
}
#[tokio::test]
async fn quiver_actor_marker_survives_real_pong_move_tick_and_advisory_report_without_timeout() {
    let h = Harness::normal();
    let mut a = archer(&h, A, 0).await;
    h.input(a.session, shot(&h, &a, 1).await).await.unwrap();
    h.tick(5.).await.unwrap();
    let out = drain(&mut a);
    let serial = serial(&out);
    h.input(
        a.session,
        packet(23, |w| {
            w.i32(serial);
        }),
    )
    .await
    .unwrap();
    let (session, id) = (a.session, a.avatar);
    let position = h
        .inspect(move |s| {
            let p = s.worlds[&0].character(id);
            (p.x, p.y)
        })
        .await;
    h.input(
        session,
        packet(5, |w| {
            w.f32(position.0).f32(position.1);
        }),
    )
    .await
    .unwrap();
    h.input(
        session,
        world::prediction_runtime_tests::report(999999, id, 1, 4),
    )
    .await
    .unwrap();
    let before = character(&h.db, A).unwrap().unwrap();
    assert_eq!(
        h.inspect(move |s| quiver::acknowledgement(&s.worlds[&0], id))
            .await,
        Some(0xa61)
    );
    h.input(session, cast(id, 1, id, (0.5, 0.5))).await.unwrap();
    assert_eq!(character(&h.db, A).unwrap().unwrap(), before);
    h.stop().await;
}
#[tokio::test]
async fn quiver_actor_sql_begin_at_each_phase_and_write_at8_preserve_actual_prephase() {
    for item in [0xa61, 0xa63] {
        let remaining = if item == 0xa61 { 40 } else { 20 };
        for phase in [8, 15] {
            for begin in [false, true] {
                let h = Harness::normal();
                let mut a = archer_with_item(&h, A, 0, item).await;
                let (mut b, _) = h.enter(B).await;
                if phase == 15 {
                    h.input(a.session, shot(&h, &a, 1).await).await.unwrap();
                }
                drain(&mut a);
                drain(&mut b);
                let before = state(&h).await;
                h.db.lock().unwrap().execute_batch(if begin {"BEGIN IMMEDIATE"} else {"CREATE TRIGGER fail_quiver BEFORE UPDATE ON build100_characters BEGIN SELECT RAISE(ABORT,'injected quiver write'); END;"}).unwrap();
                let input = if phase == 8 {
                    shot(&h, &a, 1).await
                } else {
                    cast(a.avatar, 1, a.avatar, (0., 0.))
                };
                let session = a.session;
                let failed = h
                    .inspect(move |s| {
                        let now = Instant::now();
                        s.input_at(session, input, now, now)
                            .err()
                            .map(|e| e.is::<StorageFailure>())
                    })
                    .await;
                if phase == 15 && !begin {
                    // A receipt changes no Character row: an armed write-abort trigger must not fire.
                    assert_eq!(failed, None);
                    assert_eq!(rows(&h.db), before.1);
                    let id = a.avatar;
                    assert_eq!(
                        h.inspect(move |s| quiver::acknowledgement(&s.worlds[&0], id))
                            .await,
                        None
                    );
                    assert_eq!(character(&h.db, A).unwrap().unwrap().stats[&4], remaining);
                } else {
                    assert_eq!(failed, Some(true));
                    assert_eq!(state(&h).await, before);
                    assert!(drain(&mut a).is_empty() && drain(&mut b).is_empty());
                }
                h.db.lock()
                    .unwrap()
                    .execute_batch(if begin {
                        "ROLLBACK"
                    } else {
                        "DROP TRIGGER fail_quiver"
                    })
                    .unwrap();
                if phase == 8 {
                    h.input(a.session, shot(&h, &a, 1).await).await.unwrap();
                }
                if phase != 15 || begin {
                    h.input(a.session, cast(a.avatar, 1, a.avatar, (0., 0.)))
                        .await
                        .unwrap();
                }
                assert_eq!(character(&h.db, A).unwrap().unwrap().stats[&4], remaining);
                h.stop().await;
            }
        }
    }
}
#[tokio::test]
async fn quiver_actor_actual_dispatcher_storage_failure_fatal_before8_or_after_paid8() {
    for item in [0xa61, 0xa63] {
        let remaining = if item == 0xa61 { 40 } else { 20 };
        for phase in [8, 15] {
            for begin in [false, true] {
                if phase == 15 && !begin {
                    continue;
                }
                let h = Harness::normal();
                let mut a = archer_with_item(&h, A, 0, item).await;
                if phase == 15 {
                    h.input(a.session, shot(&h, &a, 1).await).await.unwrap();
                }
                drain(&mut a);
                h.db.lock().unwrap().execute_batch(if begin {"BEGIN IMMEDIATE"} else {"CREATE TRIGGER fail_quiver BEFORE UPDATE ON build100_characters BEGIN SELECT RAISE(ABORT,'injected quiver write'); END;"}).unwrap();
                let p = if phase == 8 {
                    shot(&h, &a, 1).await
                } else {
                    cast(a.avatar, 1, a.avatar, (0., 0.))
                };
                assert!(
                    h.input(a.session, p)
                        .await
                        .unwrap_err()
                        .contains("fatal build100 persistence")
                );
                assert_eq!(
                    character(&h.db, A).unwrap().unwrap().stats[&4],
                    if phase == 8 { 100 } else { remaining }
                );
                assert!(drain(&mut a).is_empty());
                h.db.lock()
                    .unwrap()
                    .execute_batch(if begin {
                        "ROLLBACK"
                    } else {
                        "DROP TRIGGER fail_quiver"
                    })
                    .unwrap();
                drop(h.commands);
                assert!(h.task.await.unwrap().unwrap_err().is::<StorageFailure>());
            }
        }
    }
}
#[tokio::test]
async fn quiver_actor_current_whole_room_owner_generation_corruption_fails_closed_at8_and15() {
    for phase in [8, 15] {
        for case in 0..8 {
            let h = Harness::normal();
            let mut a = archer(&h, A, 0).await;
            let (mut b, _) = h.enter(B).await;
            if phase == 15 {
                h.input(a.session, shot(&h, &a, 1).await).await.unwrap();
            }
            drain(&mut a);
            drain(&mut b);
            let p = if phase == 8 {
                shot(&h, &a, 1).await
            } else {
                cast(a.avatar, 1, a.avatar, (0., 0.))
            };
            let (caster, peer, avatar) = (a.session, b.session, b.avatar);
            h.inspect(move |s| match case {
                0 => {
                    s.owners.remove(A);
                }
                1 => {
                    s.owners.insert(B.into(), caster);
                }
                2 => s.sessions.get_mut(&peer).unwrap().generation = Some(99),
                3 => s.sessions.get_mut(&caster).unwrap().generation = Some(99),
                4 => s.sessions.get_mut(&peer).unwrap().world = 999,
                5 => s.sessions.get_mut(&peer).unwrap().avatar = Some(99999),
                6 => s.sessions.get_mut(&caster).unwrap().avatar = Some(avatar),
                7 => {
                    s.sessions.remove(&peer);
                }
                _ => unreachable!(),
            })
            .await;
            let before = state(&h).await;
            assert!(h.input(a.session, p).await.is_err());
            assert_eq!(state(&h).await, before);
            assert!(drain(&mut a).is_empty() && drain(&mut b).is_empty());
            h.stop().await;
        }
    }
}
#[tokio::test]
async fn quiver_actor_postcommit_full_closed_peer8_and_caster15_detach_without_refund_or_replay() {
    for item in [0xa61, 0xa63] {
        let remaining = if item == 0xa61 { 40 } else { 20 };
        for caster in [false, true] {
            for closed in [false, true] {
                let h = Harness::normal();
                let mut a = archer_with_item(&h, A, 0, item).await;
                let (mut b, _) = h.enter(B).await;
                drain(&mut a);
                drain(&mut b);
                if caster {
                    h.input(a.session, shot(&h, &a, 1).await).await.unwrap();
                    drain(&mut b);
                }
                let failed = if caster { a.session } else { b.session };
                if closed {
                    if caster {
                        a.output.close();
                    } else {
                        b.output.close();
                    }
                } else {
                    h.inspect(move |s| {
                        for _ in 0..OUTPUT_CAPACITY {
                            s.sessions[&failed].output.try_send(vec![]).unwrap();
                        }
                    })
                    .await;
                }
                let input = if caster {
                    cast(a.avatar, 1, a.avatar, (0., 0.))
                } else {
                    shot(&h, &a, 1).await
                };
                h.input(a.session, input).await.unwrap();
                assert_eq!(character(&h.db, A).unwrap().unwrap().stats[&4], remaining);
                assert!(!h.inspect(move |s| s.sessions.contains_key(&failed)).await);
                if !caster {
                    h.input(a.session, cast(a.avatar, 1, a.avatar, (0., 0.)))
                        .await
                        .unwrap();
                }
                let guid = if caster { A } else { B };
                let mut again = h.join(guid, 0).await.unwrap();
                let out = h.load(&mut again).await;
                assert!(shot_ids(&out, a.avatar).is_empty());
                if caster {
                    assert!(
                        h.input(again.session, cast(again.avatar, 1, again.avatar, (0., 0.)))
                            .await
                            .is_err()
                    );
                }
                h.stop().await;
            }
        }
    }
}
#[tokio::test]
async fn quiver_actor_caster8_suppression_defers_failed_output_to_status_and_never_refunds() {
    for item in [0xa61, 0xa63] {
        let remaining = if item == 0xa61 { 40 } else { 20 };
        let h = Harness::normal();
        let mut a = archer_with_item(&h, A, 0, item).await;
        let (mut b, _) = h.enter(B).await;
        drain(&mut a);
        drain(&mut b);
        a.output.close();
        h.input(a.session, shot(&h, &a, 1).await).await.unwrap();
        assert_eq!(shot_ids(&drain(&mut b), a.avatar), [1]);
        let session = a.session;
        assert!(h.inspect(move |s| s.sessions.contains_key(&session)).await);
        h.input(a.session, cast(a.avatar, 1, a.avatar, (0., 0.)))
            .await
            .unwrap();
        assert!(!h.inspect(move |s| s.sessions.contains_key(&session)).await);
        assert_eq!(character(&h.db, A).unwrap().unwrap().stats[&4], remaining);
        h.stop().await;
    }
}
#[tokio::test]
async fn quiver_actor_disconnect_or_transfer_after8_keep_payment_and_forget_marker() {
    for item in [0xa61, 0xa63] {
        let remaining = if item == 0xa61 { 40 } else { 20 };
        for transfer in [false, true] {
            let h = Harness::normal();
            let mut a = archer_with_item(&h, A, if transfer { -1 } else { 0 }, item).await;
            if transfer {
                h.near_exit(&a).await;
            }
            h.input(a.session, shot(&h, &a, 1).await).await.unwrap();
            drain(&mut a);
            if !transfer {
                h.detach(a.session).await;
            }
            let mut again = h.join(A, 0).await.unwrap();
            h.load(&mut again).await;
            assert_ne!(a.avatar, again.avatar);
            assert_eq!(character(&h.db, A).unwrap().unwrap().stats[&4], remaining);
            assert!(
                h.input(again.session, cast(again.avatar, 1, again.avatar, (0., 0.)))
                    .await
                    .is_err()
            );
            h.stop().await;
        }
    }
}

#[path = "quiver_transport_tests.rs"]
mod quiver_transport_tests;

#[tokio::test]
async fn quiver_actor_failed_swap_drop_teleport_transactions_retain_paid_marker_and_state() {
    for operation in [14, 19, 28] {
        let h = Harness::normal();
        let mut a = archer(&h, A, 0).await;
        let (mut b, _) = h.enter(B).await;
        h.input(a.session, shot(&h, &a, 1).await).await.unwrap();
        drain(&mut a);
        drain(&mut b);
        let before = state(&h).await;
        let input = match operation {
            14 => packet(14, |w| {
                w.i32(a.avatar).u8(1).i16(0xa61).i32(a.avatar).u8(5).i16(-1);
            }),
            19 => packet(19, |w| {
                w.i32(a.avatar).u8(1).i16(0xa61);
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
        assert!(
            h.inspect(move |s| {
                let now = Instant::now();
                s.input_at(session, input, now, now)
                    .unwrap_err()
                    .is::<StorageFailure>()
            })
            .await
        );
        assert_eq!(state(&h).await, before);
        assert!(drain(&mut a).is_empty() && drain(&mut b).is_empty());
        h.db.lock().unwrap().execute_batch("ROLLBACK").unwrap();
        h.input(a.session, cast(a.avatar, 1, a.avatar, (0., 0.)))
            .await
            .unwrap();
        assert_eq!(character(&h.db, A).unwrap().unwrap().stats[&4], 40);
        h.stop().await;
    }
}

#[tokio::test]
async fn quiver_actor_iron_advance_actual_character_write_failure_installs_no_candidate_or_output()
{
    for dispatcher in [false, true] {
        let h = Harness::normal();
        let mut a = archer_with_item(&h, A, 0, 0xa63).await;
        let (mut b, _) = h.enter(B).await;
        let avatar = a.avatar;
        let targets = h
            .inspect(move |s| {
                world::progression_runtime_tests::guaranteed(&mut s.content, 1);
                quiver::iron_collision_fixture(s.worlds.get_mut(&0).unwrap(), &s.content, avatar)
            })
            .await;
        h.input(a.session, shot(&h, &a, 96).await).await.unwrap();
        h.input(a.session, cast(a.avatar, 1, a.avatar, (0., 0.)))
            .await
            .unwrap();
        assert_eq!(character(&h.db, A).unwrap().unwrap().stats[&4], 20);
        drain(&mut a);
        drain(&mut b);
        let before = state(&h).await;
        h.inspect(move |s| {
            let room = &s.worlds[&0];
            let mut candidate = room.clone();
            let events = candidate.advance(0.2, &s.content).unwrap();
            quiver::assert_iron_collision(&candidate, targets);
            assert_eq!(events.iter().filter(|p| p.id == 9).count(), 2);
            assert!(candidate.character(avatar).stats[&6] > room.character(avatar).stats[&6]);
            assert_ne!(quiver::snapshot(&candidate), quiver::snapshot(room));
            quiver::assert_iron_reward_candidate(room, &candidate);
        })
        .await;
        h.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_iron_advance BEFORE UPDATE ON build100_characters BEGIN SELECT RAISE(ABORT,'injected Iron collision reward write'); END;").unwrap();
        if dispatcher {
            assert!(
                h.tick(0.2)
                    .await
                    .unwrap_err()
                    .contains("fatal build100 persistence")
            );
            assert_eq!(rows(&h.db), before.1);
            assert!(drain(&mut a).is_empty() && drain(&mut b).is_empty());
            drop(h.commands);
            assert!(h.task.await.unwrap().unwrap_err().is::<StorageFailure>());
        } else {
            assert!(
                h.inspect(|s| s
                    .tick(Duration::from_secs_f64(0.2))
                    .unwrap_err()
                    .is::<StorageFailure>())
                    .await
            );
            assert_eq!(state(&h).await, before);
            assert!(drain(&mut a).is_empty() && drain(&mut b).is_empty());
            h.db.lock()
                .unwrap()
                .execute_batch("DROP TRIGGER fail_iron_advance")
                .unwrap();
            h.tick(0.2).await.unwrap();
            h.inspect(move |s| quiver::assert_iron_collision(&s.worlds[&0], targets))
                .await;
            for client in [&mut a, &mut b] {
                let out = drain(client);
                // Publication omits the already removed victim's Damage9; its Drop still retires the view.
                assert_eq!(out.iter().filter(|p| p.id == 9).count(), 1);
                assert!(shot_ids(&out, avatar).is_empty());
            }
            assert!(character(&h.db, A).unwrap().unwrap().stats[&4] < 80);
            h.stop().await;
        }
    }
}
