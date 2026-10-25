use super::*;
use crate::wire::Writer;

fn state() -> Cooperation {
    let mut c = Cooperation::default();
    c.observe([1, 2, 3, 4], 0, [(10, 0x123), (11, 0x124)].into_iter());
    c
}
fn packet(target: i32, owner: i32, bullet: u8, damage: i16, killed: bool) -> Packet {
    let mut w = Writer::new();
    w.i32(target)
        .u8(0)
        .i16(damage)
        .u8(u8::from(killed))
        .u8(bullet)
        .i32(owner);
    Packet { id: 47, body: w.0 }
}
fn hit(c: &mut Cooperation, target: i32, owner: i32, bullet: u8, now: u64) {
    c.receive(
        &packet(target, owner, bullet, 12, false),
        [1, 2, 3, 4],
        now,
        |_| None,
        |id| (id == 5).then_some(6),
    )
    .unwrap();
}
fn together() -> Cooperation {
    let mut c = state();
    hit(&mut c, 10, 4, 1, 100);
    hit(&mut c, 10, 5, 2, 200);
    c
}

#[test]
fn actual_two_sided_receipts_allow_near_company_and_fresh_walk_follow() {
    let mut c = together();
    assert_eq!(c.recent, Some(((5, 6), 60_200)));
    for x in [1., 9.] {
        assert_eq!(
            c.company(500, |_| Ok(Pos { x, y: 0. }), |_| true)
                .unwrap()
                .1
                .x,
            x
        );
    }
    c.observe([1, 2, 3, 4], 1000, [(10, 0x123)].into_iter());
    assert_eq!(c.recent.unwrap().1, 60_200);
    hit(&mut c, 10, 5, 2, 1200);
    assert_eq!(c.recent.unwrap().1, 60_200);
    hit(&mut c, 10, 5, 3, 1500);
    assert_eq!(c.recent.unwrap().1, 61_500);
    assert!(
        c.company(61_500, |_| Ok(Pos { x: 1., y: 0. }), |_| true)
            .is_none()
    );
}

#[test]
fn solo_other_enemy_unknown_stale_zero_and_unseen_peer_do_not_invent_cooperation() {
    let mut c = state();
    hit(&mut c, 10, 4, 1, 100);
    hit(&mut c, 11, 5, 2, 200);
    hit(&mut c, 99, 5, 3, 300);
    assert!(c.recent.is_none());
    c.receive(
        &packet(10, 5, 4, 0, false),
        [1, 2, 3, 4],
        400,
        |_| None,
        |_| Some(6),
    )
    .unwrap();
    c.receive(
        &packet(10, 5, 5, 12, false),
        [1, 2, 3, 4],
        400,
        |_| None,
        |_| None,
    )
    .unwrap();
    c.receive(
        &packet(10, 0, 6, 12, false),
        [1, 2, 3, 4],
        400,
        |_| None,
        |_| Some(6),
    )
    .unwrap();
    assert!(c.recent.is_none());
    c.observe([1, 2, 3, 4], 10_101, [(10, 0x123)].into_iter());
    hit(&mut c, 10, 5, 7, 10_101);
    assert!(c.recent.is_none());
    assert!(
        c.receive(
            &Packet {
                id: 47,
                body: vec![]
            },
            [1, 2, 3, 4],
            10_101,
            |_| None,
            |_| None
        )
        .is_err()
    );
}

#[test]
fn cached_observed_target_can_finish_after_drop_without_treating_absence_as_kill() {
    let mut c = state();
    hit(&mut c, 10, 4, 1, 100);
    c.observe([1, 2, 3, 4], 200, std::iter::empty());
    assert!(c.recent.is_none());
    c.receive(
        &packet(10, 5, 2, 12, true),
        [1, 2, 3, 4],
        200,
        |_| None,
        |id| (id == 5).then_some(6),
    )
    .unwrap();
    assert_eq!(c.recent.unwrap().0, (5, 6));
    assert!(!c.enemies.contains_key(&10));
}

#[test]
fn session_enemy_identity_visibility_death_and_blocked_routes_release() {
    for reason in [
        "lost_public_visibility",
        "peer_dead",
        "peer_identity_changed",
    ] {
        let mut c = together();
        assert!(c.company(300, |_| Err(reason), |_| true).is_none());
        assert!(c.recent.is_none());
    }
    let mut c = together();
    assert!(
        c.company(300, |_| Ok(Pos { x: 9., y: 0. }), |_| false)
            .is_none()
    );
    for binding in [[9, 2, 3, 4], [1, 2, 9, 4], [1, 9, 3, 4], [1, 2, 3, 9]] {
        let mut c = together();
        c.observe(binding, 300, std::iter::empty());
        assert!(c.recent.is_none());
    }
    let mut c = state();
    hit(&mut c, 10, 4, 1, 100);
    c.observe([1, 2, 3, 4], 200, [(10, 0x999)].into_iter());
    hit(&mut c, 10, 5, 2, 200);
    assert!(c.recent.is_none());
}

#[test]
fn foreign_context_cannot_pair_and_first_valid_peer_is_retained() {
    let mut c = state();
    hit(&mut c, 10, 4, 1, 100);
    c.receive(
        &packet(10, 5, 2, 12, false),
        [9, 2, 3, 4],
        200,
        |_| None,
        |_| Some(6),
    )
    .unwrap();
    assert!(c.recent.is_none());
    hit(&mut c, 10, 5, 2, 200);
    hit(&mut c, 11, 4, 3, 300);
    c.receive(
        &packet(11, 7, 4, 12, false),
        [1, 2, 3, 4],
        400,
        |_| None,
        |id| (id == 7).then_some(8),
    )
    .unwrap();
    assert_eq!(c.recent, Some(((5, 6), 60_200)));
    let mut crowd = Crowd::new(true);
    crowd.completed.insert((5, 6));
    crowd.cooperation = c;
    assert!(
        crowd
            .cooperation
            .company(500, |_| Ok(Pos { x: 1., y: 0. }), |_| true)
            .is_some()
    );
}

#[test]
fn tracking_is_bounded_and_existing_useful_activity_wins() {
    let mut c = state();
    c.observe([1, 2, 3, 4], 100, (0..100).map(|id| (id, 0x123)));
    assert_eq!(c.enemies.len(), 8);
    c.observe([1, 2, 3, 4], 10_100, std::iter::empty());
    assert!(c.enemies.is_empty());
    let origin = Pos { x: 0., y: 0. };
    for (score, goal, x) in [
        (95., Goal::Portal(1), 20.),
        (97., Goal::Hunt(1), 20.),
        (40., Goal::Loot(1), 20.),
        (40., Goal::Fight(1), 4.),
    ] {
        assert!(!available(
            &[(score, goal, Pos { x, y: 0. })],
            origin,
            8.,
            |_| true
        ));
    }
    assert!(available(
        &[(78., Goal::Hunt(1), Pos { x: 20., y: 0. })],
        origin,
        8.,
        |_| true
    ));
}

#[test]
fn only_reachable_useful_loot_suppresses_valid_cooperative_follow() {
    let mut c = together();
    assert!(
        c.company(300, |_| Ok(Pos { x: 9., y: 0. }), |_| true)
            .is_some()
    );
    let bag = Pos { x: 20., y: 0. };
    let choices = [(40., Goal::Loot(1), bag)];
    let blocked = BTreeSet::from([tile(bag)]);
    let origin = Pos { x: 0., y: 0. };
    assert!(available(&choices, origin, 8., |p| !blocked.contains(&tile(p))));
    assert!(!available(&choices, origin, 8., |_| true));
}
