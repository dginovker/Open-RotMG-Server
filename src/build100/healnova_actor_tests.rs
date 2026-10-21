use super::*;
use crate::world::spell_runtime_tests::cast;

const C: &str = "cccccccccccccccccccccccccccccccccccccccc";

async fn priest(h: &Harness, guid: &str, route: i32) -> Client {
    let mut a = h.join(guid, route).await.unwrap();
    h.input(
        a.session,
        packet(3, |w| {
            w.i16(0x310).utf("Priest");
        }),
    )
    .await
    .unwrap();
    a.avatar = Reader::new(&drain(&mut a).iter().find(|p| p.id == 1).unwrap().body)
        .i32()
        .unwrap();
    a
}
async fn injure(h: &Harness, session: u64, hp: i32) {
    h.inspect(move |s| {
        let owner = &s.sessions[&session];
        let (world, id) = (owner.world, owner.avatar.unwrap());
        let mut room = s.worlds[&world].clone();
        room.character_mut(id).stats.insert(1, hp);
        s.commit(&room, &s.worlds[&world]).unwrap();
        s.worlds.insert(world, room);
    })
    .await;
}
#[derive(Debug, PartialEq)]
struct State {
    players: Vec<(i32, Character)>,
    rows: Vec<(String, String)>,
    projectiles: (Vec<(i32, u8, i32)>, u64),
    admissions: Vec<(Instant, f32, Option<Instant>)>,
}
fn state(s: &Simulation, world: i32) -> State {
    let room = &s.worlds[&world];
    let db = s.db.lock().unwrap();
    let mut query = db
        .prepare("SELECT guid,state FROM build100_characters ORDER BY guid")
        .unwrap();
    let rows = query
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    State {
        players: room
            .players
            .iter()
            .map(|(&id, a)| (id, a.character.clone()))
            .collect(),
        rows,
        projectiles: world::spell_runtime_tests::snapshot(room),
        admissions: room
            .players
            .keys()
            .map(|&id| world::teleport_runtime_tests::admission(room, id))
            .collect(),
    }
}
async fn rejected(h: &Harness, a: &mut Client, b: &mut Client) {
    let before = h.inspect(|s| state(s, 0)).await;
    let error = h
        .input(a.session, cast(a.avatar, 1, a.avatar, (0., 0.)))
        .await
        .unwrap_err();
    assert!(!error.contains("fatal build100 persistence"), "{error}");
    assert_eq!(h.inspect(|s| state(s, 0)).await, before);
    assert!(drain(a).is_empty() && drain(b).is_empty());
}

#[tokio::test]
async fn healnova_actor_three_rows_persist_projection_and_ordinary_load_reconnect() {
    let h = Harness::normal();
    let mut a = priest(&h, A, 0).await;
    let (mut b, _) = h.enter(B).await;
    let mut d = priest(&h, C, 0).await;
    for (session, hp) in [(a.session, 10), (b.session, 20), (d.session, 30)] {
        injure(&h, session, hp).await;
    }
    drain(&mut a);
    drain(&mut b);
    drain(&mut d);
    h.input(a.session, cast(a.avatar, 1, a.avatar, (0., 0.)))
        .await
        .unwrap();
    let after = h.inspect(|s| state(s, 0)).await;
    for (id, p) in &after.players {
        assert_eq!(p.stats[&1], 100);
        let guid = if *id == a.avatar {
            A
        } else if *id == b.avatar {
            B
        } else {
            C
        };
        assert_eq!(character(&h.db, guid).unwrap().as_ref(), Some(p));
    }
    assert_eq!(character(&h.db, A).unwrap().unwrap().stats[&4], 40);
    for client in [&mut a, &mut b, &mut d] {
        let out = drain(client);
        assert!(out.iter().any(|p| p.id == 13));
        assert!(!out.iter().any(|p| matches!(p.id, 8 | 18 | 29)));
    }
    h.tick(0.125).await.unwrap();
    let saved = character(&h.db, A).unwrap().unwrap();
    h.detach(a.session).await;
    let mut again = h.join(A, 0).await.unwrap();
    h.load(&mut again).await;
    assert_eq!(character(&h.db, A).unwrap().unwrap(), saved);
    assert!(
        h.input(again.session, cast(again.avatar, 1, again.avatar, (0., 0.)))
            .await
            .is_err()
    );
    h.stop().await;
}

#[tokio::test]
async fn healnova_actor_whole_room_current_ownership_full_unchanged_peers_fail_closed() {
    for case in 0..12 {
        let h = Harness::normal();
        let mut a = priest(&h, A, 0).await;
        let (mut b, _) = h.enter(B).await;
        injure(&h, a.session, 10).await;
        drain(&mut a);
        drain(&mut b);
        let (caster, peer, avatar) = (a.session, b.session, b.avatar);
        h.inspect(move |s| match case {
            0 => {
                s.owners.remove(B);
            }
            1 => {
                s.owners.insert(B.into(), caster);
            }
            2 => {
                s.sessions.get_mut(&peer).unwrap().generation = Some(99);
            }
            3 => {
                s.sessions.get_mut(&peer).unwrap().world = 999;
            }
            4 => {
                s.sessions.get_mut(&peer).unwrap().avatar = None;
            }
            5 => {
                s.sessions.get_mut(&peer).unwrap().avatar = Some(99999);
            }
            6 => {
                s.sessions.get_mut(&caster).unwrap().generation = Some(99);
            }
            7 => {
                s.sessions.get_mut(&caster).unwrap().avatar = Some(avatar);
            }
            8 => {
                s.owners.remove(A);
            }
            9 => {
                s.owners.insert("duplicate-guid".into(), peer);
            }
            10 => {
                s.sessions.remove(&peer);
            }
            11 => {
                s.sessions.get_mut(&caster).unwrap().world = 999;
            }
            _ => unreachable!(),
        })
        .await;
        rejected(&h, &mut a, &mut b).await;
        h.stop().await;
    }
}

#[tokio::test]
async fn healnova_actor_orphan_selected_session_and_duplicate_avatar_are_rejected() {
    for duplicate in [false, true] {
        let h = Harness::normal();
        let mut a = priest(&h, A, 0).await;
        let (mut b, _) = h.enter(B).await;
        let pending = h.join(C, 0).await.unwrap();
        drain(&mut a);
        drain(&mut b);
        let (session, avatar) = (pending.session, b.avatar);
        h.inspect(move |s| {
            let p = s.sessions.get_mut(&session).unwrap();
            p.avatar = Some(if duplicate { avatar } else { 99999 });
            p.generation = Some(1);
        })
        .await;
        rejected(&h, &mut a, &mut b).await;
        h.stop().await;
    }
}

#[tokio::test]
async fn healnova_actor_last_recipient_write_and_separate_sql_begin_rollback_all_rows() {
    for begin in [false, true] {
        let h = Harness::normal();
        let mut a = priest(&h, A, 0).await;
        let (mut b, _) = h.enter(B).await;
        let (mut d, _) = h.enter(C).await;
        for session in [a.session, b.session, d.session] {
            injure(&h, session, 10).await;
        }
        drain(&mut a);
        drain(&mut b);
        drain(&mut d);
        let before = h.inspect(|s| state(s, 0)).await;
        let sql = if begin {
            "BEGIN IMMEDIATE".into()
        } else {
            format!(
                "CREATE TRIGGER fail_last_heal BEFORE UPDATE ON build100_characters WHEN OLD.guid='{C}' BEGIN SELECT RAISE(ABORT,'injected last heal row'); END;"
            )
        };
        h.db.lock().unwrap().execute_batch(&sql).unwrap();
        let (session, avatar) = (a.session, a.avatar);
        assert!(
            h.inspect(move |s| {
                let now = Instant::now();
                s.input_at(session, cast(avatar, 1, avatar, (0., 0.)), now, now)
                    .unwrap_err()
                    .is::<StorageFailure>()
            })
            .await
        );
        assert_eq!(h.inspect(|s| state(s, 0)).await, before);
        assert!(drain(&mut a).is_empty() && drain(&mut b).is_empty() && drain(&mut d).is_empty());
        h.db.lock()
            .unwrap()
            .execute_batch(if begin {
                "ROLLBACK"
            } else {
                "DROP TRIGGER fail_last_heal"
            })
            .unwrap();
        h.input(a.session, cast(a.avatar, 1, a.avatar, (0., 0.)))
            .await
            .unwrap();
        assert_eq!(character(&h.db, A).unwrap().unwrap().stats[&4], 40);
        assert_eq!(character(&h.db, C).unwrap().unwrap().stats[&1], 100);
        h.stop().await;
    }
}

#[tokio::test]
async fn healnova_actor_invalid_recipient_serialization_inputs_no_sql_or_success() {
    for case in 0..4 {
        let h = Harness::normal();
        let mut a = priest(&h, A, 0).await;
        let (mut b, _) = h.enter(B).await;
        injure(&h, a.session, 10).await;
        drain(&mut a);
        drain(&mut b);
        let avatar = b.avatar;
        h.inspect(move |s| {
            let p = s.worlds.get_mut(&0).unwrap().character_mut(avatar);
            match case {
                0 => {
                    p.stats.insert(1, 101);
                }
                1 => {
                    p.stats.insert(
                        25,
                        crate::rules::combat::ConditionMask::bit(crate::content::Condition::Sick)
                            as i32,
                    );
                }
                2 => {
                    p.stats.insert(29, 0);
                }
                3 => {
                    p.resources_version = 99;
                }
                _ => unreachable!(),
            }
        })
        .await;
        h.db.lock()
            .unwrap()
            .execute_batch("BEGIN IMMEDIATE")
            .unwrap();
        rejected(&h, &mut a, &mut b).await;
        h.db.lock().unwrap().execute_batch("ROLLBACK").unwrap();
        h.stop().await;
    }
}

#[tokio::test]
async fn healnova_actor_storage_failure_fatal_at_actual_dispatch_all_full_paid_cast() {
    let h = Harness::normal();
    let mut a = priest(&h, A, 0).await;
    h.db.lock()
        .unwrap()
        .execute_batch("BEGIN IMMEDIATE")
        .unwrap();
    assert!(
        h.input(a.session, cast(a.avatar, 1, a.avatar, (0., 0.)))
            .await
            .unwrap_err()
            .contains("fatal build100 persistence")
    );
    assert!(drain(&mut a).is_empty());
    assert_eq!(character(&h.db, A).unwrap().unwrap().stats[&4], 100);
    h.db.lock().unwrap().execute_batch("ROLLBACK").unwrap();
    drop(h.commands);
    assert!(h.task.await.unwrap().unwrap_err().is::<StorageFailure>());
}

#[tokio::test]
async fn healnova_actor_postcommit_full_closed_caster_and_healed_peer_keep_paid_state_detach_no_replay()
 {
    for caster in [false, true] {
        for closed in [false, true] {
            let h = Harness::normal();
            let mut a = priest(&h, A, 0).await;
            let (mut b, _) = h.enter(B).await;
            injure(&h, a.session, 10).await;
            injure(&h, b.session, 20).await;
            drain(&mut a);
            drain(&mut b);
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
            h.input(a.session, cast(a.avatar, 1, a.avatar, (0., 0.)))
                .await
                .unwrap();
            let pa = character(&h.db, A).unwrap().unwrap();
            let pb = character(&h.db, B).unwrap().unwrap();
            assert_eq!((pa.stats[&1], pa.stats[&4], pb.stats[&1]), (100, 40, 100));
            assert!(!h.inspect(move |s| s.sessions.contains_key(&failed)).await);
            let out = if caster { drain(&mut b) } else { drain(&mut a) };
            let tick = out.iter().position(|p| p.id == 13).unwrap();
            let drop = out.iter().position(|p| p.id == 11).unwrap();
            assert!(tick < drop);
            let guid = if caster { A } else { B };
            let mut again = h.join(guid, 0).await.unwrap();
            h.load(&mut again).await;
            assert_eq!(
                character(&h.db, guid).unwrap().unwrap(),
                if caster { pa } else { pb }
            );
            h.stop().await;
        }
    }
}

#[tokio::test]
async fn healnova_actor_same_kind_private_tutorials_never_cross_heal() {
    let h = Harness::normal();
    let mut a = priest(&h, A, -1).await;
    let (mut b, _) = h.enter_route(B, -1).await;
    injure(&h, a.session, 10).await;
    injure(&h, b.session, 20).await;
    drain(&mut a);
    drain(&mut b);
    let before = character(&h.db, B).unwrap().unwrap();
    h.input(a.session, cast(a.avatar, 1, a.avatar, (0., 0.)))
        .await
        .unwrap();
    assert_eq!(character(&h.db, A).unwrap().unwrap().stats[&4], 40);
    assert_eq!(character(&h.db, B).unwrap().unwrap(), before);
    assert!(drain(&mut b).is_empty());
    h.stop().await;
}

#[tokio::test]
async fn healnova_actor_corrupt_caster_world_submit_reply_preserves_original_error_and_deferred_state()
 {
    let h = Harness::normal();
    let mut a = priest(&h, A, 0).await;
    let (session, avatar) = (a.session, a.avatar);
    h.inspect(move |s| {
        s.sessions.get_mut(&session).unwrap().world = 999;
        let before = state(s, 0);
        let (reply, mut response) = oneshot::channel();
        s.submit_input(
            session,
            cast(avatar, 1, avatar, (0., 0.)),
            Instant::now(),
            reply,
        )
        .unwrap();
        assert_eq!(
            response.try_recv().unwrap().unwrap_err(),
            "selected world missing"
        );
        assert!(s.deferred_weapons.is_empty());
        assert_eq!(state(s, 0), before);
    })
    .await;
    assert!(drain(&mut a).is_empty());
    h.stop().await;
}

#[tokio::test]
async fn healnova_actor_serialized_swap_teleport_drop_death_detach_do_not_refund_or_reset_ability()
{
    let h = Harness::normal();
    let mut a = priest(&h, A, 0).await;
    let (mut b, _) = h.enter(B).await;
    let (session, avatar) = (a.session, a.avatar);
    h.inspect(move |s| {
        let mut room = s.worlds[&0].clone();
        room.character_mut(avatar)
            .stats
            .extend([(3, 500), (4, 500), (13, 0xa32)]);
        s.commit(&room, &s.worlds[&0]).unwrap();
        s.worlds.insert(0, room);
    })
    .await;
    drain(&mut a);
    drain(&mut b);
    h.input(session, cast(avatar, 1, avatar, (0., 0.)))
        .await
        .unwrap();
    let paid = character(&h.db, A).unwrap().unwrap();
    assert_eq!(paid.stats[&4], 440);
    h.input(
        session,
        packet(14, |w| {
            w.i32(avatar).u8(1).i16(0xa31).i32(avatar).u8(5).i16(0xa32);
        }),
    )
    .await
    .unwrap();
    let swapped = character(&h.db, A).unwrap().unwrap();
    assert_eq!((swapped.stats[&9], swapped.stats[&13]), (0xa32, 0xa31));
    assert_eq!(swapped.action_state, paid.action_state);
    assert_eq!(swapped.stats[&4], 440);
    assert!(
        h.input(session, cast(avatar, 1, avatar, (0., 0.)))
            .await
            .is_err()
    );
    h.input(
        session,
        packet(28, |w| {
            w.i32(b.avatar);
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        character(&h.db, A).unwrap().unwrap().action_state,
        paid.action_state
    );
    h.tick(0.5).await.unwrap();
    let before = character(&h.db, A).unwrap().unwrap();
    h.input(session, cast(avatar, 1, avatar, (0., 0.)))
        .await
        .unwrap();
    let healed = character(&h.db, A).unwrap().unwrap();
    assert_eq!(healed.stats[&4], before.stats[&4] - 80);
    h.input(
        session,
        packet(19, |w| {
            w.i32(avatar).u8(1).i16(0xa32);
        }),
    )
    .await
    .unwrap();
    let dropped = character(&h.db, A).unwrap().unwrap();
    assert_eq!(dropped.stats[&9], -1);
    assert_eq!(dropped.action_state, healed.action_state);
    assert_eq!(dropped.stats[&4], healed.stats[&4]);
    assert!(
        h.input(session, cast(avatar, 1, avatar, (0., 0.)))
            .await
            .is_err()
    );
    h.inspect(move |s| {
        let mut room = s.worlds[&0].clone();
        let p = room.character_mut(avatar);
        p.alive = false;
        p.stats.insert(1, 0);
        p.action_state = Some(world::ActionState::default());
        s.commit(&room, &s.worlds[&0]).unwrap();
        s.worlds.insert(0, room);
    })
    .await;
    drain(&mut a);
    drain(&mut b);
    let before = h.inspect(|s| state(s, 0)).await;
    h.input(session, cast(avatar, 1, avatar, (0., 0.)))
        .await
        .unwrap();
    assert_eq!(h.inspect(|s| state(s, 0)).await, before);
    assert!(
        drain(&mut a)
            .iter()
            .chain(drain(&mut b).iter())
            .all(|p| matches!(p.id, 11 | 13))
    );
    h.detach(session).await;
    assert!(
        h.input(session, cast(avatar, 1, avatar, (0., 0.)))
            .await
            .is_err()
    );
    assert!(!character(&h.db, A).unwrap().unwrap().alive);
    h.stop().await;
}
