use super::*;
use crate::world::prediction_runtime_tests::{
    authority_unchanged, report, target_packets, view_state,
};

async fn pair() -> (Harness, Client, Client, i32) {
    let h = Harness::normal();
    let (mut a, initial) = h.enter(A).await;
    let (mut b, peer) = h.enter(B).await;
    let enemy = objects(&initial, 0x600)[0];
    assert!(objects(&peer, 0x600).contains(&enemy));
    assert_eq!(target_packets(&initial, enemy), vec![10]);
    drain(&mut a);
    drain(&mut b);
    (h, a, b, enemy)
}
fn rows(db: &Database) -> Vec<(String, String)> {
    let db = db.lock().unwrap();
    db.prepare("SELECT guid,state FROM build100_characters ORDER BY guid")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[tokio::test]
async fn prediction_actor_serialized_self_only_repair_no_character_writes_or_later_replay() {
    let (h, mut a, mut b, enemy) = pair().await;
    let before = h.inspect(|s| s.worlds[&0].clone()).await;
    let saved = rows(&h.db);
    h.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_prediction_writes BEFORE UPDATE ON build100_characters BEGIN SELECT RAISE(ABORT,'view repair must not write Character'); END;").unwrap();
    h.input(a.session, report(enemy, a.avatar, 1, 1))
        .await
        .unwrap();
    let out = drain(&mut a);
    assert_eq!(target_packets(&out, enemy), vec![11, 10]);
    assert_eq!(out.len(), 2);
    assert!(drain(&mut b).is_empty());
    assert_eq!(rows(&h.db), saved);
    let after = h.inspect(|s| s.worlds[&0].clone()).await;
    authority_unchanged(&before, &after);
    assert_eq!(view_state(&before, a.avatar), view_state(&after, a.avatar));
    assert_eq!(view_state(&before, b.avatar), view_state(&after, b.avatar));
    h.db.lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_prediction_writes")
        .unwrap();
    h.tick(0.2).await.unwrap();
    assert!(target_packets(&drain(&mut a), enemy).is_empty());
    assert!(target_packets(&drain(&mut b), enemy).is_empty());
    h.stop().await;
}

#[tokio::test]
async fn prediction_actor_actual_sql_begin_failure_preserves_candidate_view_and_outputs() {
    let (h, mut a, mut b, enemy) = pair().await;
    let before = h.inspect(|s| s.worlds[&0].clone()).await;
    let saved = rows(&h.db);
    h.db.lock()
        .unwrap()
        .execute_batch("BEGIN IMMEDIATE")
        .unwrap();
    let (session, avatar) = (a.session, a.avatar);
    assert!(
        h.inspect(move |s| {
            let now = Instant::now();
            s.input_at(session, report(enemy, avatar, 1, 1), now, now)
                .unwrap_err()
                .is::<StorageFailure>()
        })
        .await
    );
    let after = h.inspect(|s| s.worlds[&0].clone()).await;
    authority_unchanged(&before, &after);
    assert_eq!(view_state(&before, a.avatar), view_state(&after, a.avatar));
    assert_eq!(view_state(&before, b.avatar), view_state(&after, b.avatar));
    assert_eq!(rows(&h.db), saved);
    assert!(drain(&mut a).is_empty() && drain(&mut b).is_empty());
    h.db.lock().unwrap().execute_batch("ROLLBACK").unwrap();
    h.input(a.session, report(enemy, a.avatar, 1, 1))
        .await
        .unwrap();
    assert_eq!(target_packets(&drain(&mut a), enemy), vec![11, 10]);
    h.stop().await;
}

#[tokio::test]
async fn prediction_actor_sql_begin_failure_is_fatal_through_actual_dispatcher() {
    let (h, mut a, mut b, enemy) = pair().await;
    let saved = rows(&h.db);
    h.db.lock()
        .unwrap()
        .execute_batch("BEGIN IMMEDIATE")
        .unwrap();
    assert!(
        h.input(a.session, report(enemy, a.avatar, 1, 1))
            .await
            .unwrap_err()
            .contains("fatal build100 persistence")
    );
    assert!(drain(&mut a).is_empty() && drain(&mut b).is_empty());
    assert_eq!(rows(&h.db), saved);
    h.db.lock().unwrap().execute_batch("ROLLBACK").unwrap();
    drop(h.commands);
    assert!(h.task.await.unwrap().unwrap_err().is::<StorageFailure>());
}

#[tokio::test]
async fn prediction_actor_postcommit_full_or_closed_queue_detaches_no_delivery_or_replay() {
    for closed in [false, true] {
        let (h, mut a, mut b, enemy) = pair().await;
        let saved = rows(&h.db);
        let session = a.session;
        if closed {
            a.output.close();
        } else {
            h.inspect(move |s| {
                for _ in 0..OUTPUT_CAPACITY {
                    s.sessions[&session].output.try_send(vec![]).unwrap();
                }
            })
            .await;
        }
        h.input(a.session, report(enemy, a.avatar, 1, 1))
            .await
            .unwrap();
        assert!(!h.inspect(move |s| s.sessions.contains_key(&session)).await);
        assert!(drain(&mut a).is_empty());
        assert_eq!(rows(&h.db), saved);
        let peer = drain(&mut b);
        assert!(target_packets(&peer, enemy).is_empty());
        assert_eq!(target_packets(&peer, a.avatar), vec![11]);
        let mut again = h.join(A, 0).await.unwrap();
        let initial = h.load(&mut again).await;
        assert_eq!(target_packets(&initial, enemy), vec![10]);
        h.tick(0.2).await.unwrap();
        assert!(target_packets(&drain(&mut again), enemy).is_empty());
        h.stop().await;
    }
}

#[tokio::test]
async fn prediction_actor_detach_discards_unpublished_request_reconnect_is_normal_make() {
    let (h, a, mut b, enemy) = pair().await;
    let (session, avatar) = (a.session, a.avatar);
    h.inspect(move |s| {
        let mut candidate = s.worlds[&0].clone();
        candidate
            .handle(avatar, report(enemy, avatar, 1, 1), &s.content)
            .unwrap();
        assert_eq!(view_state(&candidate, avatar).1, Some(enemy));
        s.commit(&candidate, &s.worlds[&0]).unwrap();
        s.worlds.insert(0, candidate);
    })
    .await;
    h.detach(session).await;
    assert!(target_packets(&drain(&mut b), enemy).is_empty());
    let mut again = h.join(A, 0).await.unwrap();
    let initial = h.load(&mut again).await;
    assert_eq!(target_packets(&initial, enemy), vec![10]);
    let avatar = again.avatar;
    assert!(
        h.inspect(move |s| view_state(&s.worlds[&0], avatar).1.is_none())
            .await
    );
    h.tick(0.2).await.unwrap();
    assert!(target_packets(&drain(&mut again), enemy).is_empty());
    h.stop().await;
}

#[tokio::test]
async fn prediction_actor_foreign_room_unknown_target_never_makes_or_drops_player() {
    let h = Harness::normal();
    let (mut a, _) = h.enter_route(A, -1).await;
    let (mut b, initial) = h.enter(B).await;
    let enemy = objects(&initial, 0x600)[0];
    h.input(a.session, report(enemy, a.avatar, 1, 1))
        .await
        .unwrap();
    h.input(a.session, report(a.avatar, a.avatar, 1, 1))
        .await
        .unwrap();
    h.input(a.session, report(b.avatar, a.avatar, 1, 1))
        .await
        .unwrap();
    assert!(drain(&mut a).is_empty() && drain(&mut b).is_empty());
    h.stop().await;
}
