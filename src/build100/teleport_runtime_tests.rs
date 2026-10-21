use super::*;
use rand::RngCore;
use std::time::Duration;

fn setup() -> (Content, Room) {
    let (c, mut r) = spell_runtime_tests::setup(false);
    for id in [1, 2] {
        spell_runtime_tests::attach(&mut r, &c, id);
    }
    (c, r)
}
fn teleport(target: i32) -> Packet {
    packet(28, |w| {
        w.i32(target);
    })
}
pub(crate) fn admission(r: &Room, id: i32) -> (Instant, f32, Option<Instant>) {
    let a = &r.players[&id];
    (a.move_at, a.move_credit, a.teleport_ready)
}
fn unchanged(before: &Room, after: &Room) {
    for (&id, a) in &before.players {
        let b = &after.players[&id];
        assert_eq!(a.character, b.character);
        assert_eq!(admission(before, id), admission(after, id));
        assert_eq!(a.known_ground, b.known_ground);
        assert_eq!(a.known_objects, b.known_objects);
        assert_eq!(a.weapon.started, b.weapon.started);
        assert_eq!(a.weapon.last_received, b.weapon.last_received);
    }
    assert_eq!(
        spell_runtime_tests::snapshot(before),
        spell_runtime_tests::snapshot(after)
    );
}

#[test]
fn teleport_runtime_missing_opcode_self_has_authoritative_goto() {
    let (c, mut r) = setup();
    let now = Instant::now();
    let out = r.handle_at(1, teleport(1), &c, now, now).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].id, 18);
    let mut read = Reader::new(&out[0].body);
    assert_eq!(read.i32().unwrap(), 1);
    assert_eq!(read.f32().unwrap(), r.character(1).x);
    assert_eq!(read.f32().unwrap(), r.character(1).y);
    read.done().unwrap();
    assert_eq!(
        r.players[&1].teleport_ready,
        Some(now + Duration::from_secs(10))
    );
}

#[test]
fn teleport_runtime_strict_signed_body_malformed_even_after_death_no_mutation() {
    let (c, template) = setup();
    for dead in [false, true] {
        for len in [0, 1, 2, 3, 5, 8] {
            let mut r = template.clone();
            if dead {
                r.character_mut(1).alive = false;
                r.character_mut(1).stats.insert(1, 0);
            }
            let before = r.clone();
            let now = Instant::now();
            let error = r
                .handle_at(
                    1,
                    Packet {
                        id: 28,
                        body: vec![0; len],
                    },
                    &c,
                    now,
                    now,
                )
                .err()
                .unwrap();
            assert!(!error.is::<TeleportDecline>());
            unchanged(&before, &r);
        }
    }
    for target in [i32::MIN, -1, i32::MAX, 0x12345678] {
        let request = teleport(target);
        assert_eq!(
            super::super::protocol::teleport(&request.body).unwrap(),
            target
        );
        let mut r = template.clone();
        let now = Instant::now();
        assert!(
            r.handle_at(1, request, &c, now, now)
                .err()
                .unwrap()
                .is::<TeleportDecline>()
        );
        unchanged(&template, &r);
    }
}

#[test]
fn teleport_runtime_target_dead_detached_and_cooldown_declines_are_atomic() {
    let (c, template) = setup();
    let now = Instant::now();
    for case in 0..4 {
        let mut r = template.clone();
        match case {
            0 => {
                r.players.remove(&2);
            }
            1 => {
                r.character_mut(2).alive = false;
                r.character_mut(2).stats.insert(1, 0);
            }
            2 => {
                r.character_mut(1).alive = false;
                r.character_mut(1).stats.insert(1, 0);
            }
            3 => {
                r.players.get_mut(&1).unwrap().teleport_ready = Some(now + Duration::from_secs(1));
            }
            _ => unreachable!(),
        }
        let before = r.clone();
        assert!(
            r.handle_at(1, teleport(2), &c, now, now)
                .err()
                .unwrap()
                .is::<TeleportDecline>()
        );
        unchanged(&before, &r);
    }
}

#[test]
fn teleport_runtime_cooldown_exact_acceptance_boundary_rejection_does_not_extend() {
    let (c, mut r) = setup();
    let received = Instant::now();
    let accepted = received + Duration::from_secs(60);
    r.handle_at(1, teleport(2), &c, received, accepted).unwrap();
    let before = r.clone();
    let ready = accepted + Duration::from_secs(10);
    assert!(
        r.handle_at(
            1,
            teleport(2),
            &c,
            received,
            ready - Duration::from_nanos(1)
        )
        .err()
        .unwrap()
        .is::<TeleportDecline>()
    );
    unchanged(&before, &r);
    r.handle_at(1, teleport(2), &c, received, ready).unwrap();
    assert_eq!(
        r.players[&1].teleport_ready,
        Some(ready + Duration::from_secs(10))
    );
}

#[test]
fn teleport_runtime_paused_predicate_without_fabricating_normal_timers() {
    use crate::content::Condition;
    assert!(teleport_unpaused(combat::ConditionMask::default()).is_ok());
    let paused =
        combat::ConditionMask::from_bits(combat::ConditionMask::bit(Condition::Paused)).unwrap();
    assert!(
        teleport_unpaused(paused)
            .err()
            .unwrap()
            .is::<TeleportDecline>()
    );
    let (c, mut r) = setup();
    r.character_mut(1).stats.insert(25, paused.bits() as i32);
    let before = r.clone();
    let now = Instant::now();
    assert!(
        !r.handle_at(1, teleport(2), &c, now, now)
            .err()
            .unwrap()
            .is::<TeleportDecline>()
    );
    unchanged(&before, &r);
}

#[test]
fn teleport_runtime_invalid_authoritative_destination_is_invariant_not_decline() {
    let (c, template) = setup();
    let blocked = template
        .map
        .as_ref()
        .unwrap()
        .tiles()
        .iter()
        .enumerate()
        .find(|(i, _)| {
            !template
                .map
                .as_ref()
                .unwrap()
                .walkable((*i % 256) as i32, (*i / 256) as i32)
        })
        .unwrap()
        .0;
    for pos in [
        (f32::NAN, 5.),
        (f32::INFINITY, 5.),
        (-1., 5.),
        (256., 5.),
        ((blocked % 256) as f32 + 0.5, (blocked / 256) as f32 + 0.5),
    ] {
        let mut r = template.clone();
        (r.character_mut(2).x, r.character_mut(2).y) = pos;
        let now = Instant::now();
        let before = admission(&r, 1);
        assert!(
            !r.handle_at(1, teleport(2), &c, now, now)
                .err()
                .unwrap()
                .is::<TeleportDecline>()
        );
        assert_eq!(admission(&r, 1), before);
        assert_eq!(r.character(1), template.character(1));
    }
}

fn shot(c: &Content, r: &Room, id: u8, origin: (f32, f32)) -> Packet {
    let kind = r.character(1).stats[&8] as i16;
    let visual = c
        .named(&c.get(kind).unwrap().normal_projectile(0).unwrap().visual)
        .unwrap()
        .id as u8;
    packet(8, |w| {
        w.u8(id)
            .i32(1)
            .u8(visual)
            .i16(kind)
            .f32(origin.0)
            .f32(origin.1)
            .f32(0.)
            .i16(0);
    })
}

#[test]
fn teleport_runtime_far_warp_preserves_weapon_bullets_rng_resources_and_distance_credit() {
    let (c, mut r) = setup();
    let now = Instant::now();
    let origin = (r.character(1).x, r.character(1).y);
    // Staff's two-shot group exercises a paid prior burst and a still-incomplete reservation.
    r.character_mut(1).stats.insert(8, 0xa98);
    for (id, time) in [(1, now), (2, now), (3, now + Duration::from_secs(1))] {
        r.handle_at(1, shot(&c, &r, id, origin), &c, time, time)
            .unwrap();
    }
    assert_eq!(r.bullets.len(), 2);
    assert!(r.players[&1].weapon.pending.is_some());
    let destination = (81., 190.5);
    assert!(r.map.as_ref().unwrap().walkable(81, 190));
    (r.character_mut(2).x, r.character_mut(2).y) = destination;
    r.players.get_mut(&1).unwrap().move_credit = 0.07;
    r.players.get_mut(&1).unwrap().move_at = now - Duration::from_secs(100);
    let before = r.clone();
    let accepted = now + Duration::from_secs(1);
    r.handle_at(1, teleport(2), &c, now, accepted).unwrap();
    let mut expected = before.character(1).clone();
    (expected.x, expected.y) = destination;
    assert_eq!(r.character(1), &expected);
    assert_eq!(
        admission(&r, 1),
        (accepted, 0.07, Some(accepted + Duration::from_secs(10)))
    );
    assert_eq!(
        spell_runtime_tests::snapshot(&r),
        spell_runtime_tests::snapshot(&before)
    );
    let a = &before.players[&1].weapon;
    let b = &r.players[&1].weapon;
    assert_eq!(
        (a.started, a.last_received, a.interval_ms),
        (b.started, b.last_received, b.interval_ms)
    );
    let a = a.pending.as_ref().unwrap();
    let b = b.pending.as_ref().unwrap();
    assert_eq!(
        (
            a.first_received,
            a.due,
            a.origin,
            &a.shots,
            a.equipment,
            a.attack,
            a.conditions
        ),
        (
            b.first_received,
            b.due,
            b.origin,
            &b.shots,
            b.equipment,
            b.attack,
            b.conditions
        )
    );
    let saved_rng = r.rng.clone().next_u64();
    let error = r
        .handle_at(1, shot(&c, &r, 4, origin), &c, accepted, accepted)
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("origin"));
    assert_eq!(r.bullets.len(), 2);
    assert_eq!(r.rng.clone().next_u64(), saved_rng);
    let mut old_origin = r.players[&1].weapon.clone();
    old_origin.pending.as_mut().unwrap().shots.push((4, 0.));
    let error = r.fund_weapon(1, old_origin, &c, accepted).err().unwrap();
    assert!(format!("{error:#}").contains("origin invalid before funding"));
    assert_eq!(r.bullets.len(), 2);
    assert_eq!(r.rng.clone().next_u64(), saved_rng);
    assert_eq!(
        r.players[&1].weapon.started,
        before.players[&1].weapon.started
    );
    assert_eq!(
        r.players[&1].weapon.pending.as_ref().unwrap().shots.len(),
        1
    );
}

#[test]
fn teleport_runtime_warp_never_funds_queued_old_position_move_or_regrants_credit() {
    let (c, mut r) = setup();
    let old = (r.character(1).x, r.character(1).y);
    (r.character_mut(2).x, r.character_mut(2).y) = (81., 190.5);
    r.players.get_mut(&1).unwrap().move_credit = 0.01;
    r.players.get_mut(&1).unwrap().move_at = Instant::now() - Duration::from_secs(100);
    let accepted = Instant::now();
    r.handle_at(1, teleport(2), &c, accepted, accepted).unwrap();
    let before = r.clone();
    let error = r
        .handle(
            1,
            packet(5, |w| {
                w.f32(old.0).f32(old.1);
            }),
            &c,
        )
        .err()
        .unwrap();
    assert!(error.is::<MoveBudget>());
    unchanged(&before, &r);
    r.handle_at(
        1,
        teleport(1),
        &c,
        accepted,
        accepted + Duration::from_secs(10),
    )
    .unwrap();
    assert_eq!(r.players[&1].move_credit, 0.01);
    assert_eq!(r.players[&1].move_at, accepted + Duration::from_secs(10));
}

#[derive(Default)]
struct View {
    ground: BTreeSet<(i16, i16)>,
    objects: BTreeSet<i32>,
}
impl View {
    fn replay(&mut self, packets: &[Packet]) {
        for p in packets {
            let mut r = Reader::new(&p.body);
            match p.id {
                12 => {
                    self.ground.insert((r.i16().unwrap(), r.i16().unwrap()));
                }
                10 => {
                    r.i16().unwrap();
                    r.utf().unwrap();
                    let id = r.i32().unwrap();
                    let pos = (
                        r.f32().unwrap().floor() as i16,
                        r.f32().unwrap().floor() as i16,
                    );
                    assert!(self.ground.contains(&pos));
                    assert!(self.objects.insert(id));
                }
                18 => {
                    assert!(self.objects.contains(&r.i32().unwrap()));
                    let pos = (
                        r.f32().unwrap().floor() as i16,
                        r.f32().unwrap().floor() as i16,
                    );
                    assert!(self.ground.contains(&pos));
                    r.done().unwrap();
                }
                11 => {
                    assert!(self.objects.remove(&r.i32().unwrap()));
                }
                13 => {
                    for _ in 0..r.i16().unwrap() {
                        assert!(self.objects.contains(&r.i32().unwrap()));
                        r.f32().unwrap();
                        r.f32().unwrap();
                        for _ in 0..r.i16().unwrap() {
                            r.u8().unwrap();
                            r.i32().unwrap();
                        }
                    }
                    r.done().unwrap();
                }
                _ => panic!("unexpected publication {}", p.id),
            }
        }
    }
}

#[test]
fn teleport_runtime_origin_destination_overlap_unknown_owner_and_square_publication_then_tick() {
    let (c, template) = setup();
    // 20-tile warp has origin-only, destination-only and overlap observers; 100-tile warp needs fresh terrain.
    for separation in [20., 100.] {
        let mut r = template.clone();
        let map = r.map.as_ref().unwrap();
        let origin = (1..255)
            .flat_map(|y| (1..(255 - separation as i32)).map(move |x| (x, y)))
            .find(|&(x, y)| {
                [0., separation, separation / 2.]
                    .iter()
                    .all(|dx| map.walkable((x as f32 + dx).floor() as i32, y))
            })
            .unwrap();
        let origin = (origin.0 as f32 + 0.5, origin.1 as f32 + 0.5);
        for (id, dx) in [(1, 0.), (2, separation), (3, 0.), (4, separation / 2.)] {
            if !r.players.contains_key(&id) {
                spell_runtime_tests::attach(&mut r, &c, id);
            }
            (r.character_mut(id).x, r.character_mut(id).y) = (origin.0 + dx, origin.1);
        }
        let mut views = BTreeMap::new();
        for id in 1..=4 {
            let mut view = View::default();
            view.replay(&r.publication(id, &[], &c).unwrap());
            views.insert(id, view);
        }
        let now = Instant::now();
        let events = r.handle_at(1, teleport(2), &c, now, now).unwrap();
        for id in 1..=4 {
            let out = r.publication(id, &events, &c).unwrap();
            assert!(!out.iter().any(|p| p.id == 29));
            let gets_goto = id == 1 || id == 2 || (id == 4 && separation == 20.);
            assert_eq!(out.iter().any(|p| p.id == 18), gets_goto);
            if id == 3 {
                assert!(
                    out.iter()
                        .any(|p| p.id == 11 && Reader::new(&p.body).i32().unwrap() == 1)
                );
            }
            views.get_mut(&id).unwrap().replay(&out);
            let tick = r.tick(&c).unwrap();
            views
                .get_mut(&id)
                .unwrap()
                .replay(&r.publication(id, &[tick], &c).unwrap());
        }
    }
}
