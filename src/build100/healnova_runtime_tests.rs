use super::spell_runtime_tests::{cast, setup, snapshot};
use super::*;

fn attach(room: &mut Room, c: &Content, id: i32, kind: i16, hp: i32) {
    let mut p = Character::new(c, kind, format!("Heal{id}"), 3).unwrap();
    (p.x, p.y) = room.landing();
    p.stats.insert(1, hp);
    room.attach(id, p, c).unwrap();
}

#[test]
fn healnova_runtime_light_heal_pays_once_and_heals_caster_and_two_classes() {
    let (c, mut room) = setup(true);
    attach(&mut room, &c, 1, 0x310, 10);
    attach(&mut room, &c, 2, 0x30e, 30);
    attach(&mut room, &c, 3, 0x300, 80);
    let before = snapshot(&room);
    let out = room.handle(1, cast(1, 1, 1, (0., 0.)), &c).unwrap();
    for id in 1..=3 {
        assert_eq!(
            room.character(id).stats[&1],
            if id == 3 { 150 } else { 100 }
        );
    }
    assert_eq!(room.character(1).stats[&4], 40);
    assert_eq!(room.character(1).stats[&9], 0xa31);
    assert_eq!(
        room.character(1)
            .action_state
            .as_ref()
            .unwrap()
            .ability_used_seconds,
        Some(0.)
    );
    assert_eq!(snapshot(&room), before);
    assert_eq!(out.iter().map(|p| p.id).collect::<Vec<_>>(), [13]);
}

fn characters(room: &Room) -> Vec<(i32, Character)> {
    room.players
        .iter()
        .map(|(&id, a)| (id, a.character.clone()))
        .collect()
}
fn reject(c: &Content, room: &Room, input: Packet) {
    let mut candidate = room.clone();
    assert!(candidate.handle(1, input, c).is_err());
    assert_eq!(characters(&candidate), characters(room));
    assert_eq!(snapshot(&candidate), snapshot(room));
    for &id in room.players.keys() {
        assert_eq!(
            super::teleport_runtime_tests::admission(&candidate, id),
            super::teleport_runtime_tests::admission(room, id)
        );
        assert_eq!(
            candidate.players[&id].weapon.started,
            room.players[&id].weapon.started
        );
    }
}

#[test]
fn healnova_runtime_original_repaired_three_parsed_contracts_and_starter() {
    use crate::content::Action;
    for repaired in [false, true] {
        let (c, _) = setup(repaired);
        let p = Character::new(&c, 0x310, "Priest".into(), 1).unwrap();
        assert_eq!(c.get(0x310).unwrap().slots[1], 4);
        assert_eq!(
            (p.stats[&0], p.stats[&1], p.stats[&3], p.stats[&4]),
            (100, 100, 100, 100)
        );
        assert_eq!((p.stats[&8], p.stats[&9]), (0xa04, 0xa31));
        for (id, amount, range, cost) in [
            (0xa31, 100, 10., 60),
            (0xa32, 200, 10., 80),
            (0xa33, 500, 15., 100),
        ] {
            let item = c.get(id).unwrap();
            assert_eq!(item.slot_type, Some(4));
            assert!(!item.consumable && item.effects.is_empty());
            assert_eq!(item.mp_cost, Some(cost));
            assert_eq!(item.activations.len(), 1);
            assert!(
                matches!(item.activations[0].action().unwrap(), Action::HealNova {range:r,healing} if r==range && healing==amount)
            );
        }
    }
}

#[test]
fn healnova_runtime_declared_strict_ranges_half_range_diagonal_and_cursor_independence() {
    for (item, range) in [(0xa31, 10_f32), (0xa33, 15_f32)] {
        let (c, mut room) = setup(true);
        attach(&mut room, &c, 1, 0x310, 50);
        attach(&mut room, &c, 2, 0x30e, 1);
        room.character_mut(1).stats.insert(9, item);
        let center = (100_f32, 100_f32);
        (room.character_mut(1).x, room.character_mut(1).y) = center;
        for offset in [
            range / 2. - 0.01,
            range / 2.,
            range / 2. + 0.01,
            range - 0.01,
            range,
            range + 0.01,
        ] {
            for cursor in [(0., 0.), (-9999., 9999.)] {
                let mut r = room.clone();
                (r.character_mut(2).x, r.character_mut(2).y) = (100. + offset, 100.);
                r.handle(1, cast(1, 1, 1, cursor), &c).unwrap();
                assert_eq!(
                    r.character(2).stats[&1],
                    if offset < range { 100 } else { 1 },
                    "item={item} offset={offset}"
                );
            }
        }
        let mut r = room.clone();
        (r.character_mut(2).x, r.character_mut(2).y) = (106., 108.);
        r.handle(1, cast(1, 1, 1, (0., 0.)), &c).unwrap();
        assert_eq!(r.character(2).stats[&1], if range == 10. { 1 } else { 100 });
    }
}

#[test]
fn healnova_runtime_clipping_carry_effective_max_backpack_and_peer_timers_unchanged() {
    let (c, mut room) = setup(true);
    attach(&mut room, &c, 1, 0x310, 1);
    attach(&mut room, &c, 2, 0x30e, 1);
    attach(&mut room, &c, 3, 0x300, 100);
    room.character_mut(1).stats.extend([
        (9, 0xa33),
        (0, 1000),
        (1, 400),
        (3, 200),
        (4, 130),
        (15, 0xa4f),
    ]);
    room.character_mut(2).stats.extend([(11, 0xa4f), (1, 100)]);
    for id in 1..=3 {
        if id != 1 {
            room.character_mut(id).stats.insert(4, 50);
        }
        room.character_mut(id).resources_carry = Some(ResourceCarry {
            hp_remainder: -0.25,
            hp_roundoff: -1e-20,
            mp_remainder: 0.125,
            mp_roundoff: 1e-20,
        });
    }
    let peer_state = room.character_mut(2).action_state.as_mut().unwrap();
    peer_state.ability_used_seconds = Some(0.);
    let before = characters(&room);
    let admissions: Vec<_> = (1..=3)
        .map(|id| super::teleport_runtime_tests::admission(&room, id))
        .collect();
    room.handle(1, cast(1, 1, 1, (0., 0.)), &c).unwrap();
    for (id, mut expected) in before {
        let stats = expected.equipment_stats(&c).unwrap();
        let mut v = expected
            .vitals(&stats)
            .unwrap()
            .heal(500, &stats, expected.resource_conditions().unwrap())
            .unwrap();
        if id == 1 {
            v.mp -= 100;
            expected.action_state.as_mut().unwrap().ability_used_seconds = Some(0.);
        }
        expected.store_vitals(v);
        assert_eq!(room.character(id), &expected);
        assert_eq!(
            super::teleport_runtime_tests::admission(&room, id),
            admissions[(id - 1) as usize]
        );
    }
    assert_eq!(room.character(1).stats[&1], 900);
    assert_eq!(room.character(1).stats[&0], 1000);
    assert_eq!(
        room.character(1).resources_carry.unwrap().hp_remainder,
        -0.25
    );
    assert_eq!(
        room.character(2).stats[&1],
        room.character(2)
            .equipment_stats(&c)
            .unwrap()
            .get(0)
            .unwrap()
    );
    assert_eq!(room.character(2).resources_carry.unwrap().hp_roundoff, 0.);
}

#[test]
fn healnova_runtime_full_caster_peer_all_full_paid_and_tick_projection() {
    let (c, mut base) = setup(true);
    attach(&mut base, &c, 1, 0x310, 100);
    attach(&mut base, &c, 2, 0x30e, 20);
    for full_peer in [false, true] {
        let mut r = base.clone();
        if full_peer {
            r.character_mut(2).stats.insert(1, 100);
        }
        let out = r.handle(1, cast(1, 1, 1, (0., 0.)), &c).unwrap();
        assert_eq!(r.character(1).stats[&4], 40);
        assert_eq!(r.character(2).stats[&1], 100);
        let mut reader = Reader::new(&out[0].body);
        assert_eq!(out[0].id, 13);
        let count = reader.i16().unwrap();
        for _ in 0..count {
            let id = reader.i32().unwrap();
            reader.f32().unwrap();
            reader.f32().unwrap();
            let n = reader.i16().unwrap();
            let stats: BTreeMap<_, _> = (0..n)
                .map(|_| (reader.u8().unwrap(), reader.i32().unwrap()))
                .collect();
            assert_eq!(stats[&1], 100);
            assert_eq!(stats[&4], if id == 1 { 40 } else { 100 });
            assert!(!stats.contains_key(&25));
        }
        reader.done().unwrap();
    }
}

#[test]
fn healnova_runtime_complete_unsupported_shape_and_admission_are_atomic() {
    use crate::content::{Condition, ConditionEffect};
    let (c, mut base) = setup(true);
    attach(&mut base, &c, 1, 0x310, 10);
    attach(&mut base, &c, 2, 0x30e, 20);
    for input in [
        cast(2, 1, 1, (0., 0.)),
        cast(1, 0, 1, (0., 0.)),
        cast(1, 1, 2, (0., 0.)),
    ] {
        reject(&c, &base, input);
    }
    for value in [f32::NAN, f32::INFINITY] {
        let mut nonfinite = cast(1, 1, 1, (0., 0.));
        nonfinite.body[9..13].copy_from_slice(&value.to_be_bytes());
        reject(&c, &base, nonfinite);
    }
    let mut trailing = cast(1, 1, 1, (0., 0.));
    trailing.body.push(0);
    reject(&c, &base, trailing);
    let mut short = cast(1, 1, 1, (0., 0.));
    short.body.pop();
    reject(&c, &base, short);
    for kind in [0xa5b, 0xa52, 0xa53, 0xa54, 0xa55, 0xa3f, -1] {
        let mut r = base.clone();
        r.character_mut(1).stats.insert(9, kind);
        reject(&c, &r, cast(1, 1, 1, (0., 0.)));
    }
    for case in 0..7 {
        let mut altered = Content {
            objects: c.objects.clone(),
        };
        let item = altered.objects.get_mut(&0xa31).unwrap();
        match case {
            0 => item.activations.push(item.activations[0].clone()),
            1 => item.effects.push(ConditionEffect {
                condition: Condition::Invisible,
                duration_seconds: 1.,
            }),
            2 => item
                .activations
                .push(c.get(0xa5b).unwrap().activations[1].clone()),
            3 => {
                item.activations[0] = c.get(0xa5b).unwrap().activations[1].clone();
            }
            4 => item.activations[0]
                .attributes
                .insert("range".into(), "NaN".into())
                .map(|_| ())
                .unwrap(),
            5 => item.mp_cost = None,
            6 => item.consumable = true,
            _ => unreachable!(),
        }
        reject(&altered, &base, cast(1, 1, 1, (0., 0.)));
    }
    for mp in [0, 59] {
        let mut r = base.clone();
        r.character_mut(1).stats.insert(4, mp);
        reject(&c, &r, cast(1, 1, 1, (0., 0.)));
    }
    let mut r = base.clone();
    r.character_mut(1).stats.extend([(9, -1), (13, 0xa31)]);
    reject(&c, &r, cast(1, 5, 1, (0., 0.)));
    let mut r = base.clone();
    r.character_mut(1).kind = 0x30e;
    reject(&c, &r, cast(1, 1, 1, (0., 0.)));
}

#[test]
fn healnova_runtime_bad_recipient_or_authoritative_position_aborts_before_any_mutation() {
    let (c, mut base) = setup(true);
    attach(&mut base, &c, 1, 0x310, 10);
    attach(&mut base, &c, 2, 0x30e, 20);
    for case in 0..8 {
        let mut r = base.clone();
        let peer = r.character_mut(2);
        match case {
            0 => peer.x = f32::NAN,
            1 => peer.y = f32::INFINITY,
            2 => peer.x = -1.,
            3 => peer.x = 256.,
            4 => peer.resources_carry.as_mut().unwrap().hp_roundoff = f64::NAN,
            5 => {
                peer.stats.insert(29, 0);
            }
            6 => {
                peer.stats.insert(1, 101);
            }
            7 => {
                peer.stats.insert(9, 0xa31);
            }
            _ => unreachable!(),
        }
        // NaN cannot be compared with PartialEq; compare its stable serialized representation.
        let before = serde_json::to_string(&characters(&r)).unwrap();
        let rng = snapshot(&r);
        assert!(r.handle(1, cast(1, 1, 1, (0., 0.)), &c).is_err());
        assert_eq!(serde_json::to_string(&characters(&r)).unwrap(), before);
        assert_eq!(snapshot(&r), rng);
    }
}

#[test]
fn healnova_runtime_condition_helpers_and_normal_invalid_saves_stay_rejected() {
    use crate::content::Condition;
    let (c, mut r) = setup(true);
    attach(&mut r, &c, 1, 0x310, 10);
    let p = r.character(1);
    let stats = p.equipment_stats(&c).unwrap();
    let v = p.vitals(&stats).unwrap();
    for condition in [
        Condition::Sick,
        Condition::Paused,
        Condition::Quiet,
        Condition::Stunned,
    ] {
        let mask = combat::ConditionMask::from_bits(combat::ConditionMask::bit(condition)).unwrap();
        assert_eq!(
            healnova_recipient(true, mask),
            condition != Condition::Paused
        );
        assert!(!healnova_recipient(false, mask));
        assert_eq!(
            v.heal(100, &stats, mask).unwrap().hp,
            if condition == Condition::Sick {
                10
            } else {
                100
            }
        );
        assert_eq!(
            combat::ability_ready(c.get(0xa31).unwrap(), 100, 100, mask, 0., None).unwrap(),
            !matches!(condition, Condition::Quiet | Condition::Paused)
        );
        for timer in [false, true] {
            let mut bad = r.clone();
            let p = bad.character_mut(1);
            p.stats.insert(25, mask.bits() as i32);
            if timer {
                p.action_state.as_mut().unwrap().conditions.expiries[condition as usize - 1] =
                    Some(1.);
            }
            reject(&c, &bad, cast(1, 1, 1, (0., 0.)));
            let p = bad.character(1).clone();
            bad.detach(1);
            assert!(bad.attach(1, p, &c).is_err());
        }
    }
    let sick =
        combat::ConditionMask::from_bits(combat::ConditionMask::bit(Condition::Sick)).unwrap();
    let mut debt = v;
    debt.hp_remainder = -0.25;
    debt.hp_roundoff = -1e-20;
    assert_eq!(debt.heal(100, &stats, sick).unwrap(), debt);
}

#[test]
fn healnova_runtime_cooldown_exact_boundary_regeneration_control_reconnect_and_current_slot() {
    let (c, mut r) = setup(true);
    attach(&mut r, &c, 1, 0x310, 10);
    r.character_mut(1).stats.extend([(3, 500), (4, 500)]);
    r.handle(1, cast(1, 1, 1, (0., 0.)), &c).unwrap();
    for _ in 0..3 {
        reject(&c, &r, cast(1, 1, 1, (0., 0.)));
    }
    r.advance(0.125, &c).unwrap();
    let p = r.character(1).clone();
    let (_, mut restored) = setup(true);
    restored.attach(1, p.clone(), &c).unwrap();
    assert_eq!(restored.character(1), &p);
    reject(&c, &restored, cast(1, 1, 1, (0., 0.)));
    let below = 0.5_f64.next_down();
    restored.advance(below - 0.125, &c).unwrap();
    reject(&c, &restored, cast(1, 1, 1, (0., 0.)));
    restored.advance(0.5 - below, &c).unwrap();
    let control = restored.clone();
    restored.character_mut(1).stats.insert(9, 0xa32);
    restored.handle(1, cast(1, 1, 1, (0., 0.)), &c).unwrap();
    assert_eq!(
        restored.character(1).stats[&4],
        control.character(1).stats[&4] - 80
    );
    assert_eq!(
        restored.character(1).resources_carry.unwrap().mp_remainder,
        control.character(1).resources_carry.unwrap().mp_remainder
    );
}

#[test]
fn healnova_runtime_dead_detached_nonplayers_and_wall_do_not_define_recipient_set() {
    let (c, mut r) = setup(true);
    attach(&mut r, &c, 1, 0x310, 10);
    attach(&mut r, &c, 2, 0x30e, 20);
    attach(&mut r, &c, 3, 0x300, 20);
    r.character_mut(2).alive = false;
    r.character_mut(2).stats.insert(1, 0);
    let dead = r.character(2).clone();
    r.detach(3);
    // This Realm point is authoritative map space, not a walkability/line-of-sight test.
    (r.character_mut(1).x, r.character_mut(1).y) = (0.5, 0.5);
    attach(&mut r, &c, 4, 0x30e, 20);
    (r.character_mut(4).x, r.character_mut(4).y) = (1.5, 0.5);
    let entities: Vec<_> = r
        .entities
        .values()
        .map(|e| (e.id, e.stats.clone()))
        .collect();
    r.handle(1, cast(1, 1, 1, (0., 0.)), &c).unwrap();
    assert_eq!(r.character(2), &dead);
    assert!(!r.players.contains_key(&3));
    assert_eq!(r.character(4).stats[&1], 100);
    assert_eq!(
        r.entities
            .values()
            .map(|e| (e.id, e.stats.clone()))
            .collect::<Vec<_>>(),
        entities
    );
}

#[test]
fn healnova_runtime_wall_separated_walkable_peers_and_mutually_invisible_peers_heal() {
    use crate::{content::Grounds, realm_map};
    let (c, mut r) = setup(true);
    let path = std::env::var_os("BUILD100_CONTENT_REPAIRED_DIR").unwrap();
    let map = realm_map::tutorial(
        100,
        &c,
        Arc::new(Grounds::load(std::path::Path::new(&path)).unwrap()),
    )
    .unwrap();
    let (from, to) = ((8.5, 9.5), (8.5, 15.5));
    assert!(map.walkable(8, 9) && map.walkable(8, 15));
    assert!(!map.can_move(from, to));
    let mut tutorial = Room::from_map(Arc::new(map), &c, 1_000_000).unwrap();
    tutorial.entities.clear();
    attach(&mut tutorial, &c, 1, 0x310, 10);
    attach(&mut tutorial, &c, 2, 0x30e, 20);
    (tutorial.character_mut(1).x, tutorial.character_mut(1).y) = from;
    (tutorial.character_mut(2).x, tutorial.character_mut(2).y) = to;
    tutorial.handle(1, cast(1, 1, 1, (0., 0.)), &c).unwrap();
    assert_eq!(tutorial.character(2).stats[&1], 100);
    for id in 1..=3 {
        attach(&mut r, &c, id, if id == 1 { 0x310 } else { 0x30e }, 10);
    }
    for (id, x) in [(1, 100.), (2, 91.), (3, 109.)] {
        (r.character_mut(id).x, r.character_mut(id).y) = (x, 100.);
    }
    assert!(distance(r.character(2).x, 100., r.character(3).x, 100.) > 16.);
    r.handle(1, cast(1, 1, 1, (0., 0.)), &c).unwrap();
    assert_eq!(
        (r.character(2).stats[&1], r.character(3).stats[&1]),
        (100, 100)
    );
}

#[test]
fn healnova_runtime_each_root_cost_boundary_and_masquerading_cloak_shape_rejects() {
    let (c, mut base) = setup(true);
    attach(&mut base, &c, 1, 0x310, 10);
    for (kind, cost) in [(0xa31, 60), (0xa32, 80), (0xa33, 100)] {
        for mp in [cost - 1, cost, cost + 1] {
            let mut r = base.clone();
            r.character_mut(1)
                .stats
                .extend([(9, kind), (3, 200), (4, mp)]);
            if mp < cost {
                reject(&c, &r, cast(1, 1, 1, (0., 0.)));
            } else {
                r.handle(1, cast(1, 1, 1, (0., 0.)), &c).unwrap();
                assert_eq!(r.character(1).stats[&4], mp - cost);
            }
        }
    }
    let mut altered = Content {
        objects: c.objects.clone(),
    };
    let cloak = c.get(0xa56).unwrap();
    let item = altered.objects.get_mut(&0xa31).unwrap();
    item.activations = cloak.activations.clone();
    item.effects = cloak.effects.clone();
    reject(&altered, &base, cast(1, 1, 1, (0., 0.)));
}
