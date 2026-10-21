use super::*;
use crate::world::{TeleportDecline, teleport_runtime_tests::admission};

fn teleport(target: i32) -> Packet {
    packet(28, |w| {
        w.i32(target);
    })
}
fn completion(out: &[Packet], success: bool) {
    let done: Vec<_> = out.iter().enumerate().filter(|(_, p)| p.id == 29).collect();
    assert_eq!(done.len(), 1);
    assert!(done[0].1.body.is_empty());
    let relocations: Vec<_> = out.iter().enumerate().filter(|(_, p)| p.id == 18).collect();
    assert_eq!(relocations.len(), usize::from(success));
    if success {
        assert!(relocations[0].0 < done[0].0);
    }
}
#[derive(Debug, PartialEq)]
struct State {
    character: Character,
    admission: (Instant, f32, Option<Instant>),
    projectiles: (Vec<(i32, u8, i32)>, u64),
    saved: Character,
}
fn state(s: &Simulation, session: u64) -> State {
    let owner = &s.sessions[&session];
    let room = &s.worlds[&owner.world];
    let id = owner.avatar.unwrap();
    State {
        character: room.character(id).clone(),
        admission: admission(room, id),
        projectiles: world::spell_runtime_tests::snapshot(room),
        saved: character(&s.db, &owner.guid).unwrap().unwrap(),
    }
}
async fn assert_decline(h: &Harness, a: &mut Client, b: &mut Client, target: i32) {
    let session = a.session;
    let before = h.inspect(move |s| state(s, session)).await;
    assert!(h.input(session, teleport(target)).await.is_err());
    assert_eq!(h.inspect(move |s| state(s, session)).await, before);
    completion(&drain(a), false);
    assert!(!drain(b).iter().any(|p| matches!(p.id, 18 | 29)));
}

#[tokio::test]
async fn teleport_actor_far_success_requester_one_ordered_batch_peers_no29_and_reconnect_reset() {
    let h = Harness::normal();
    let (mut a, _) = h.enter(A).await;
    let (mut b, _) = h.enter(B).await;
    drain(&mut a);
    h.place(b.session, (81., 190.5)).await;
    let session = a.session;
    let before = h.inspect(move |s| state(s, session)).await;
    h.input(session, teleport(b.avatar)).await.unwrap();
    let batch = a.output.try_recv().unwrap();
    completion(&batch, true);
    assert!(a.output.try_recv().is_err());
    let goto = batch.iter().position(|p| p.id == 18).unwrap();
    assert!(batch[..goto].iter().any(|p| p.id == 12));
    assert!(batch[..goto].iter().any(|p| p.id == 10));
    let mut r = Reader::new(&batch[goto].body);
    assert_eq!(
        (r.i32().unwrap(), r.f32().unwrap(), r.f32().unwrap()),
        (a.avatar, 81., 190.5)
    );
    r.done().unwrap();
    assert!(!drain(&mut b).iter().any(|p| p.id == 29));
    let after = h.inspect(move |s| state(s, session)).await;
    let mut expected = before.character;
    (expected.x, expected.y) = (81., 190.5);
    assert_eq!(after.character, expected);
    assert_eq!(after.saved, expected);
    assert_eq!(after.admission.1, before.admission.1);
    assert_eq!(after.projectiles, before.projectiles);
    assert!(after.admission.2.is_some());
    let target = b.avatar;
    assert_decline(&h, &mut a, &mut b, target).await;
    let old_id = a.avatar;
    h.detach(session).await;
    drain(&mut b);
    let mut next = h.join(A, 0).await.unwrap();
    h.load(&mut next).await;
    assert_ne!(next.avatar, old_id);
    let session = next.session;
    assert_eq!(
        h.inspect(move |s| state(s, session).admission.2).await,
        None
    );
    let landing = h.inspect(|s| s.worlds[&0].landing()).await;
    let saved = character(&h.db, A).unwrap().unwrap();
    assert_eq!((saved.x, saved.y), landing);
    assert_eq!(saved.stats, expected.stats);
    h.input(session, teleport(next.avatar)).await.unwrap();
    completion(&drain(&mut next), true);
    assert!(!drain(&mut b).iter().any(|p| p.id == 29));
    h.stop().await;
}

#[tokio::test]
async fn teleport_actor_missing_detached_dead_target_dead_caster_decline_once_without_sql() {
    for case in 0..4 {
        let h = Harness::normal();
        let (mut a, _) = h.enter(A).await;
        let (mut b, _) = h.enter(B).await;
        drain(&mut a);
        let target = b.avatar;
        match case {
            0 => {}
            1 => h.detach(b.session).await,
            2 | 3 => {
                let session = if case == 2 { b.session } else { a.session };
                h.inspect(move |s| {
                    let owner = &s.sessions[&session];
                    let (world, id) = (owner.world, owner.avatar.unwrap());
                    let mut candidate = s.worlds[&world].clone();
                    candidate.character_mut(id).alive = false;
                    candidate.character_mut(id).stats.insert(1, 0);
                    s.commit(&candidate, &s.worlds[&world]).unwrap();
                    s.worlds.insert(world, candidate);
                })
                .await;
            }
            _ => unreachable!(),
        }
        drain(&mut a);
        drain(&mut b);
        h.db.lock()
            .unwrap()
            .execute_batch("BEGIN IMMEDIATE")
            .unwrap();
        assert_decline(
            &h,
            &mut a,
            &mut b,
            if case == 0 { i32::MIN } else { target },
        )
        .await;
        h.db.lock().unwrap().execute_batch("ROLLBACK").unwrap();
        h.stop().await;
    }
}

#[tokio::test]
async fn teleport_actor_target_requires_current_owner_and_generation() {
    for case in 0..3 {
        let h = Harness::normal();
        let (mut a, _) = h.enter(A).await;
        let (mut b, _) = h.enter(B).await;
        drain(&mut a);
        let peer = b.session;
        h.inspect(move |s| match case {
            0 => {
                s.owners.remove(B);
            }
            1 => {
                s.sessions.get_mut(&peer).unwrap().generation = Some(99);
            }
            2 => {
                s.sessions.get_mut(&peer).unwrap().avatar = None;
            }
            _ => unreachable!(),
        })
        .await;
        let target = b.avatar;
        assert_decline(&h, &mut a, &mut b, target).await;
        h.stop().await;
    }
}

#[tokio::test]
async fn teleport_actor_actual_room_not_map_kind_rejects_foreign_tutorial_and_realm() {
    let h = Harness::normal();
    let (mut a, _) = h.enter_route(A, -1).await;
    let (mut b, _) = h.enter_route(B, -1).await;
    let target = b.avatar;
    assert_decline(&h, &mut a, &mut b, target).await;
    h.input(a.session, teleport(a.avatar)).await.unwrap();
    completion(&drain(&mut a), true);
    assert!(drain(&mut b).is_empty());
    h.detach(b.session).await;
    let mut b = h.join(B, 0).await.unwrap();
    h.load(&mut b).await;
    let target = b.avatar;
    let session = a.session;
    let received = Instant::now();
    let error = h
        .inspect(move |s| {
            s.input_at(
                session,
                teleport(target),
                received,
                received + Duration::from_secs(10),
            )
            .err()
            .unwrap()
            .is::<TeleportDecline>()
        })
        .await;
    assert!(error);
    completion(&drain(&mut a), false);
    assert!(drain(&mut b).is_empty());
    h.stop().await;
}

#[tokio::test]
async fn teleport_actor_malformed_unselected_stale_wrong_owner_and_generation_no_completion() {
    let h = Harness::normal();
    let mut a = h.join(A, 0).await.unwrap();
    drain(&mut a);
    assert!(h.input(a.session, teleport(1)).await.is_err());
    assert!(drain(&mut a).is_empty());
    h.input(a.session, create()).await.unwrap();
    let out = drain(&mut a);
    a.avatar = Reader::new(&out.iter().find(|p| p.id == 1).unwrap().body)
        .i32()
        .unwrap();
    let (mut b, _) = h.enter(B).await;
    drain(&mut a);
    let session = a.session;
    let before = h.inspect(move |s| state(s, session)).await;
    for size in [0, 1, 2, 3, 5, 8] {
        assert!(
            h.input(
                session,
                Packet {
                    id: 28,
                    body: vec![0; size]
                }
            )
            .await
            .is_err()
        );
        assert_eq!(h.inspect(move |s| state(s, session)).await, before);
        assert!(drain(&mut a).is_empty());
    }
    for case in 0..2 {
        h.inspect(move |s| {
            if case == 0 {
                s.owners.insert(A.into(), 999);
            } else {
                s.sessions.get_mut(&session).unwrap().generation = Some(99);
            }
        })
        .await;
        assert!(h.input(session, teleport(b.avatar)).await.is_err());
        assert!(drain(&mut a).is_empty());
        h.inspect(move |s| {
            s.owners.insert(A.into(), session);
            s.sessions.get_mut(&session).unwrap().generation = Some(1);
        })
        .await;
        assert_eq!(h.inspect(move |s| state(s, session)).await, before);
    }
    h.detach(session).await;
    drain(&mut b);
    let mut next = h.join(A, 0).await.unwrap();
    h.load(&mut next).await;
    drain(&mut b);
    assert!(h.input(session, teleport(b.avatar)).await.is_err());
    assert!(drain(&mut next).is_empty());
    assert_decline(&h, &mut b, &mut next, a.avatar).await;
    h.stop().await;
}

#[tokio::test]
async fn teleport_actor_invariant_error_not_gameplay_completion() {
    let h = Harness::normal();
    let (mut a, _) = h.enter(A).await;
    let (mut b, _) = h.enter(B).await;
    drain(&mut a);
    let peer = b.session;
    h.inspect(move |s| {
        let id = s.sessions[&peer].avatar.unwrap();
        s.worlds.get_mut(&0).unwrap().character_mut(id).x = -1.;
    })
    .await;
    let session = a.session;
    let before = h.inspect(move |s| state(s, session)).await;
    assert!(
        h.input(session, teleport(b.avatar))
            .await
            .unwrap_err()
            .contains("illegal authoritative")
    );
    assert_eq!(h.inspect(move |s| state(s, session)).await, before);
    assert!(drain(&mut a).is_empty() && drain(&mut b).is_empty());
    h.stop().await;
}

#[tokio::test]
async fn teleport_actor_sql_begin_self_no_row_and_far_write_failure_no_install_or_completion() {
    for case in 0..3 {
        let h = Harness::normal();
        let (mut a, _) = h.enter(A).await;
        let (mut b, _) = h.enter(B).await;
        drain(&mut a);
        h.place(b.session, (81., 190.5)).await;
        let session = a.session;
        let before = h.inspect(move |s| state(s, session)).await;
        h.db.lock().unwrap().execute_batch(if case == 2 {
            "CREATE TRIGGER fail_teleport BEFORE UPDATE ON build100_characters BEGIN SELECT RAISE(ABORT,'injected teleport write'); END;"
        } else { "BEGIN IMMEDIATE" }).unwrap();
        let target = if case == 0 { a.avatar } else { b.avatar };
        assert!(
            h.inspect(move |s| {
                let now = Instant::now();
                s.input_at(session, teleport(target), now, now)
                    .unwrap_err()
                    .is::<StorageFailure>()
            })
            .await
        );
        assert_eq!(h.inspect(move |s| state(s, session)).await, before);
        assert!(drain(&mut a).is_empty() && drain(&mut b).is_empty());
        h.db.lock()
            .unwrap()
            .execute_batch(if case == 2 {
                "DROP TRIGGER fail_teleport"
            } else {
                "ROLLBACK"
            })
            .unwrap();
        h.input(session, teleport(target)).await.unwrap();
        completion(&drain(&mut a), true);
        assert!(!drain(&mut b).iter().any(|p| p.id == 29));
        h.stop().await;
    }
}

#[tokio::test]
async fn teleport_actor_storage_failure_is_fatal_at_dispatch_boundary() {
    let h = Harness::normal();
    let (mut a, _) = h.enter(A).await;
    h.db.lock()
        .unwrap()
        .execute_batch("BEGIN IMMEDIATE")
        .unwrap();
    assert!(
        h.input(a.session, teleport(a.avatar))
            .await
            .unwrap_err()
            .contains("fatal build100 persistence")
    );
    assert!(drain(&mut a).is_empty());
    h.db.lock().unwrap().execute_batch("ROLLBACK").unwrap();
    drop(h.commands);
    assert!(h.task.await.unwrap().unwrap_err().is::<StorageFailure>());
}

#[tokio::test]
async fn teleport_actor_postcommit_full_or_closed_requester_or_peer_keeps_durable_warp_and_removes()
{
    for requester in [false, true] {
        for closed in [false, true] {
            let h = Harness::normal();
            let (mut a, _) = h.enter(A).await;
            let (mut b, _) = h.enter(B).await;
            drain(&mut a);
            h.place(b.session, (81., 190.5)).await;
            let session = a.session;
            let failed = if requester { a.session } else { b.session };
            if closed {
                if requester {
                    a.output.close();
                } else {
                    b.output.close();
                }
            } else {
                h.inspect(move |s| {
                    let out = &s.sessions[&failed].output;
                    for _ in 0..OUTPUT_CAPACITY {
                        assert!(out.try_send(vec![]).is_ok());
                    }
                })
                .await;
            }
            h.input(session, teleport(b.avatar)).await.unwrap();
            let saved = character(&h.db, A).unwrap().unwrap();
            assert_eq!((saved.x, saved.y), (81., 190.5));
            assert!(saved.alive);
            assert_eq!(
                h.inspect(move |s| (s.sessions.contains_key(&failed), s.worlds[&0].players.len()))
                    .await,
                (false, 1)
            );
            let out = if requester {
                drain(&mut b)
            } else {
                drain(&mut a)
            };
            if requester {
                assert!(!out.iter().any(|p| p.id == 29));
            } else {
                completion(&out, true);
            }
            let goto = out.iter().position(|p| p.id == 18).unwrap();
            let removed = if requester { a.avatar } else { b.avatar };
            let drop = out
                .iter()
                .position(|p| p.id == 11 && Reader::new(&p.body).i32().unwrap() == removed)
                .unwrap();
            assert!(goto < drop);
            assert!(!out[drop..].iter().any(|p| p.id == 18));
            if !requester {
                assert!(
                    h.inspect(move |s| state(s, session).admission.2.is_some())
                        .await
                );
                let mut survivor = h.join(B, 0).await.unwrap();
                h.load(&mut survivor).await;
                assert!(!drain(&mut a).iter().any(|p| p.id == 29));
            }
            h.stop().await;
        }
    }
}

fn native_shot(s: &Simulation, avatar: i32, id: u8) -> Packet {
    let p = s.worlds[&0].character(avatar);
    let kind = p.stats[&8] as i16;
    let visual = s
        .content
        .named(
            &s.content
                .get(kind)
                .unwrap()
                .normal_projectile(0)
                .unwrap()
                .visual,
        )
        .unwrap()
        .id as u8;
    packet(8, |w| {
        w.u8(id)
            .i32(avatar)
            .u8(visual)
            .i16(kind)
            .f32(p.x + 0.3)
            .f32(p.y)
            .f32(0.)
            .i16(0);
    })
}

#[tokio::test]
async fn teleport_actor_deferred_weapon_slot_not_bypassed_cancelled_or_retimed() {
    let h = Harness::normal();
    let (mut a, _) = h.enter(A).await;
    let (mut b, _) = h.enter(B).await;
    drain(&mut a);
    let (session, avatar, target) = (a.session, a.avatar, b.avatar);
    h.inspect(move |s| {
        s.worlds
            .get_mut(&0)
            .unwrap()
            .character_mut(avatar)
            .stats
            .extend([(28, 17), (8, 0xa98)]);
        let base = Instant::now();
        let mut waiting = None;
        for (id, ns) in [(21, 0), (22, 77_104), (23, 335_496_014), (24, 335_937_358)] {
            let time = base + Duration::from_nanos(ns);
            let p = native_shot(s, avatar, id);
            let (reply, mut receive) = oneshot::channel();
            let result = s.input_at(session, p, time, time);
            s.reply_input(session, result, reply).unwrap();
            if id == 24 {
                waiting = Some(receive);
            } else {
                assert!(receive.try_recv().unwrap().is_ok());
            }
        }
        let before = state(s, session);
        let due = s.worlds[&0].weapon_deadline(avatar).unwrap();
        let (reply, mut response) = oneshot::channel();
        s.submit_input(session, teleport(target), base, reply)
            .unwrap();
        assert!(
            response
                .try_recv()
                .unwrap()
                .unwrap_err()
                .contains("backpressure")
        );
        assert_eq!(state(s, session), before);
        assert_eq!(s.worlds[&0].weapon_deadline(avatar), Some(due));
        assert!(waiting.as_mut().unwrap().try_recv().is_err());
        s.release_weapons(due).unwrap();
        assert!(waiting.as_mut().unwrap().try_recv().unwrap().is_ok());
        s.input_at(session, teleport(target), base, due).unwrap();
    })
    .await;
    completion(&drain(&mut a), true);
    assert!(!drain(&mut b).iter().any(|p| p.id == 29));
    h.stop().await;
}

#[tokio::test]
async fn teleport_actor_deferred_move_slot_preserves_original_packet_receipt_due_boundary() {
    let h = Harness::normal();
    let (mut a, _) = h.enter(A).await;
    let (mut b, _) = h.enter(B).await;
    drain(&mut a);
    let (session, target) = (a.session, b.avatar);
    h.inspect(move |s| {
        let p = state(s, session).character;
        s.input(
            session,
            packet(5, |w| {
                w.f32(p.x).f32(p.y);
            }),
            Instant::now(),
        )
        .unwrap();
        let (reply, mut waiting) = oneshot::channel();
        let received = Instant::now();
        s.submit_input(
            session,
            packet(5, |w| {
                w.f32(p.x + 0.8).f32(p.y);
            }),
            received,
            reply,
        )
        .unwrap();
        let pending = s.deferred_weapons[&session].movement.as_ref().unwrap();
        let before = (
            pending.packet.body.clone(),
            pending.received,
            pending.due,
            pending.boundary,
        );
        let saved = state(s, session);
        let (reply, mut response) = oneshot::channel();
        s.submit_input(session, teleport(target), received, reply)
            .unwrap();
        assert!(
            response
                .try_recv()
                .unwrap()
                .unwrap_err()
                .contains("backpressure")
        );
        let pending = s.deferred_weapons[&session].movement.as_ref().unwrap();
        assert_eq!(
            (
                pending.packet.body.clone(),
                pending.received,
                pending.due,
                pending.boundary
            ),
            before
        );
        assert_eq!(state(s, session), saved);
        assert!(waiting.try_recv().is_err());
        drop(waiting);
        s.cancel_lost_weapons();
        assert!(s.deferred_weapons.is_empty());
        s.input(session, teleport(target), Instant::now()).unwrap();
    })
    .await;
    completion(&drain(&mut a), true);
    assert!(!drain(&mut b).iter().any(|p| p.id == 29));
    h.stop().await;
}
