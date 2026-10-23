use super::spell_runtime_tests::{cast, setup};
use super::*;
use rand::RngCore;

pub(crate) fn archer(room: &mut Room, c: &Content, id: i32, item: i16) {
    let mut p = Character::new(c, 0x307, format!("Archer{id}"), 1).unwrap();
    (p.x, p.y) = room.landing();
    p.stats.insert(9, i32::from(item));
    room.attach(id, p, c).unwrap();
}
pub(crate) fn fire(room: &Room, owner: i32, id: u8) -> Packet {
    let p = room.character(owner);
    native(owner, id, p.stats[&9] as i16, (p.x + 0.3, p.y), 0., 32767)
}
fn native(owner: i32, id: u8, kind: i16, origin: (f32, f32), angle: f32, claim: i16) -> Packet {
    packet(8, |w| {
        w.u8(id)
            .i32(owner)
            .u8(if kind == 0xa63 { 9 } else { 10 })
            .i16(kind);
        for value in [origin.0, origin.1, angle] {
            w.0.extend(value.to_be_bytes());
        }
        w.i16(claim);
    })
}
#[test]
fn quiver_runtime_native_paid_shoot8_then_nonpaying_use15_original_repaired() {
    for repaired in [false, true] {
        for (item, cost, min) in [(0xa61, 60, 50), (0xa62, 80, 70), (0xa63, 80, 90)] {
            let (c, mut room) = setup(repaired);
            archer(&mut room, &c, 1, item);
            let mut rng = room.rng.clone();
            let expected = (f64::from(min) + 20. * rng.gen_range(0.0..1.0)) as i32;
            let input = fire(&room, 1, 96);
            let events = room
                .handle(1, input, &c)
                .expect("native quiver Shoot8 must pay immediately");
            assert_eq!(events.iter().filter(|p| p.id == 8).count(), 1);
            assert_eq!(room.character(1).stats[&4], 100 - cost);
            assert_eq!(room.character(1).stats[&25], 0);
            assert_eq!(room.bullets.len(), 1);
            assert_eq!((room.bullets[0].id, room.bullets[0].damage), (96, expected));
            assert_eq!(room.rng.clone().next_u64(), rng.next_u64());
            let paid = room.character(1).clone();
            let after8 = room.rng.clone().next_u64();
            let events = room.handle(1, cast(1, 1, 1, (0.5, 0.5)), &c).unwrap();
            assert!(events.iter().any(|p| p.id == 13));
            assert!(!events.iter().any(|p| p.id == 8));
            assert_eq!(room.character(1), &paid);
            assert_eq!(room.rng.clone().next_u64(), after8);
            assert_eq!(room.bullets.len(), 1);
            assert!(room.handle(1, cast(1, 1, 1, (0.5, 0.5)), &c).is_err());
        }
    }
}

pub(crate) fn snapshot(room: &Room) -> String {
    let players: Vec<_> = room
        .players
        .iter()
        .map(|(&id, a)| {
            (
                id,
                a.quiver_ack,
                a.move_at,
                a.move_credit,
                a.teleport_ready,
                a.hazard_at,
                a.weapon.started,
                a.weapon.last_received,
                a.weapon.interval_ms,
                a.weapon.pending.as_ref().map(|p| {
                    format!(
                        "{:?}",
                        (
                            p.first_received,
                            p.due,
                            &p.weapon,
                            p.equipment,
                            p.attack,
                            p.conditions,
                            p.origin,
                            &p.shots
                        )
                    )
                }),
            )
        })
        .collect();
    format!(
        "{:?}",
        (
            shore_runtime_tests::snapshot(room),
            players,
            &room.enemy_slows
        )
    )
}
pub(crate) fn acknowledgement(room: &Room, owner: i32) -> Option<i16> {
    room.players[&owner].quiver_ack
}
fn reject(c: &Content, room: &Room, input: Packet) -> String {
    let mut candidate = room.clone();
    let before = snapshot(&candidate);
    let error = candidate.handle(1, input, c).err().expect("must reject");
    assert_eq!(snapshot(&candidate), before);
    format!("{error:#}")
}
fn condition(room: &mut Room, condition: crate::content::Condition) {
    room.character_mut(1)
        .stats
        .insert(25, combat::ConditionMask::bit(condition) as i32);
}
#[test]
fn quiver_runtime_exact_tuples_and_contextual_whitelist_reject_compound_deferred() {
    use crate::content::{Action, Condition};
    for repaired in [false, true] {
        let (mut c, mut room) = setup(repaired);
        for (item, bounds, cost) in [(0xa61, (50, 70), 60), (0xa62, (70, 90), 80)] {
            let item = c.get(item).unwrap();
            let p = item.quiver_projectile().unwrap();
            assert_eq!(item.root_damage, Some(bounds));
            assert_eq!(item.mp_cost, Some(cost));
            assert_eq!(item.activations[0].action().unwrap(), Action::Shoot);
            assert_eq!(
                (p.min, p.max, p.speed, p.lifetime),
                (bounds.0, bounds.1, 15., 1.)
            );
            assert_eq!(p.effects[0].condition, Condition::Slowed);
            assert_eq!(p.effects[0].duration_seconds, 3.);
            assert!(item.normal_projectile(0).is_err());
            assert!(item.arena_projectile(0).is_err());
            for u in [0., 0.5, 1_f64.next_down()] {
                assert_eq!(
                    combat::continuous_damage(
                        combat::DamageSource::Root(item),
                        combat::AttackScaling::Unscaled,
                        u
                    )
                    .unwrap(),
                    (f64::from(bounds.0) + f64::from(bounds.1 - bounds.0) * u) as i32
                );
            }
        }
        archer(&mut room, &c, 1, 0xa61);
        for kind in [0xa64, 0xa65] {
            room.character_mut(1).stats.insert(9, kind);
            reject(&c, &room, fire(&room, 1, 1));
        }
        room.character_mut(1).stats.insert(9, 0xa61);
        for case in 0..18 {
            let original = c.get(0xa61).unwrap().clone();
            let item = c.objects.get_mut(&0xa61).unwrap();
            match case {
                0 => item.activations.push(item.activations[0].clone()),
                1 => item.activations[0]
                    .attributes
                    .insert("unknown".into(), "1".into())
                    .map(drop)
                    .unwrap_or(()),
                2 => item.projectiles.push(item.projectiles[0].clone()),
                3 => item.projectiles[0].0 = 1,
                4 => item.projectiles[0].1.continues = true,
                5 => item.projectiles[0].1.effects[0].condition = Condition::Paralyzed,
                6 => item.effects[0].duration_seconds = 4.,
                7 => item.projectiles[0].1.effects[0].duration_seconds = 4.,
                8 => item.projectiles[0].1.features.push("Unknown".into()),
                9 => item.num_projectiles = 2,
                10 => item.root_damage = Some((1, 3)),
                11 => item.mp_cost = None,
                12 => item.projectiles[0].1.visual = "Green Arrow".into(),
                13 => item.projectiles[0].1.speed = 16.,
                14 => item.projectiles[0].1.lifetime = 2.,
                15 => item.effects.push(item.effects[0].clone()),
                16 => item.rate_of_fire = Some(1.),
                17 => item.projectiles[0].1.amplitude = 1.,
                _ => unreachable!(),
            }
            reject(&c, &room, fire(&room, 1, 1));
            c.objects.insert(0xa61, original);
        }
        // Slowed on an ordinary bow/NPC remains unsupported; no global effect whitelist change.
        for id in [0xa1a, 0x600, 0xa2e] {
            let mut item = c.get(id).unwrap().clone();
            item.projectiles[0].1.effects = c.get(0xa61).unwrap().effects.clone();
            assert!(item.normal_projectile(0).is_err());
            assert!(item.quiver_projectile().is_err());
        }
    }
}
#[test]
fn quiver_runtime_native_geometry_body_owner_id_visual_and_claim_boundaries() {
    for item in [0xa61, 0xa63] {
        let (c, mut room) = setup(false);
        archer(&mut room, &c, 1, item);
        let (x, y) = (room.character(1).x, room.character(1).y);
        for id in [0, 97, 255] {
            reject(&c, &room, native(1, id, item, (x, y), 0., 0));
        }
        reject(&c, &room, native(2, 1, item, (x, y), 0., 0));
        for origin in [
            (x + 2., y),
            (x - 2., y),
            (-0.1, y),
            (256., y),
            (x, f32::NAN),
            (f32::INFINITY, y),
        ] {
            reject(&c, &room, native(1, 1, item, origin, 0., 0));
        }
        for angle in [
            f32::NAN,
            f32::INFINITY,
            std::f32::consts::PI.next_up(),
            -std::f32::consts::PI.next_up(),
        ] {
            reject(&c, &room, native(1, 1, item, (x, y), angle, 0));
        }
        let p = fire(&room, 1, 1);
        assert_eq!(p.body.len(), 22);
        for len in 0..22 {
            reject(
                &c,
                &room,
                Packet {
                    id: 8,
                    body: p.body[..len].to_vec(),
                },
            );
        }
        let mut trailing = fire(&room, 1, 1);
        trailing.body.push(0);
        reject(&c, &room, trailing);
        let mut visual = fire(&room, 1, 1);
        visual.body[5] = if item == 0xa63 { 10 } else { 9 };
        reject(&c, &room, visual);
        for angle in [std::f32::consts::PI, -std::f32::consts::PI, 0.37] {
            let mut r = room.clone();
            let origin = (x + 1.999, y);
            r.handle(1, native(1, 1, item, origin, angle, i16::MIN), &c)
                .unwrap();
            assert_eq!(r.bullets[0].origin, origin);
            let Flight::Normal { angle: stored, .. } = r.bullets[0].flight else {
                panic!("normal")
            };
            assert_eq!(stored, f64::from(angle));
            let mut other = room.clone();
            other
                .handle(1, native(1, 1, item, origin, angle, i16::MAX), &c)
                .unwrap();
            assert_eq!(r.bullets[0].damage, other.bullets[0].damage);
        }
    }
}
#[test]
fn quiver_runtime_payment_resources_cooldown_conditions_unscaled_and_signed_carry() {
    use crate::content::Condition;
    for (kind, cost) in [(0xa61, 60), (0xa62, 80), (0xa63, 80)] {
        let (c, mut room) = setup(false);
        archer(&mut room, &c, 1, kind);
        for mp in [cost - 1, cost, cost + 1] {
            let mut r = room.clone();
            r.character_mut(1).stats.insert(4, mp);
            if mp < cost {
                reject(&c, &r, fire(&r, 1, 1));
            } else {
                let p = fire(&r, 1, 1);
                r.handle(1, p, &c).unwrap();
                assert_eq!(r.character(1).stats[&4], mp - cost);
            }
        }
        for elapsed in [0.499, 0.500, 0.501] {
            let mut r = room.clone();
            let p = fire(&r, 1, 1);
            r.handle(1, p, &c).unwrap();
            r.handle(1, cast(1, 1, 1, (0., 0.)), &c).unwrap();
            r.advance(elapsed, &c).unwrap();
            r.character_mut(1).stats.insert(4, 100); // identical-time regen control
            r.character_mut(1).resources_carry = Some(ResourceCarry::ZERO);
            if elapsed < 0.5 {
                reject(&c, &r, fire(&r, 1, 2));
            } else {
                let p = fire(&r, 1, 2);
                r.handle(1, p, &c).unwrap();
                assert_eq!(r.character(1).stats[&4], 100 - cost);
            }
        }
        for effect in [
            Condition::Quiet,
            Condition::Paused,
            Condition::Stunned,
            Condition::Weak,
            Condition::Damaging,
        ] {
            let mut r = room.clone();
            condition(&mut r, effect);
            reject(&c, &r, fire(&r, 1, 1));
            let mask =
                combat::ConditionMask::from_bits(combat::ConditionMask::bit(effect)).unwrap();
            assert_eq!(
                combat::ability_ready(c.get(kind).unwrap(), 100, 100, mask, 0., None).unwrap(),
                !matches!(effect, Condition::Quiet | Condition::Paused)
            );
        }
        let mut damages = vec![];
        for attack in [0, 12, 75] {
            let mut r = room.clone();
            let p = r.character_mut(1);
            p.stats.extend([(20, attack), (4, 99), (1, 99)]);
            p.resources_carry = Some(ResourceCarry {
                hp_remainder: 0.25,
                mp_remainder: 0.25,
                hp_roundoff: -1e-20,
                mp_roundoff: -1e-20,
            });
            let carry = p.resources_carry;
            let input = fire(&r, 1, 1);
            r.handle(1, input, &c).unwrap();
            assert_eq!(r.character(1).resources_carry, carry);
            damages.push(r.bullets[0].damage);
            assert!(
                !r.character(1)
                    .resource_conditions()
                    .unwrap()
                    .contains(Condition::Slowed)
            );
        }
        assert!(damages.iter().all(|d| *d == damages[0]));
    }
}
#[test]
fn quiver_runtime_acknowledgement_missing_duplicate_mismatch_intervening_inputs_and_no_timeout() {
    let (c, mut room) = setup(false);
    archer(&mut room, &c, 1, 0xa61);
    reject(&c, &room, cast(1, 1, 1, (0., 0.)));
    let p = fire(&room, 1, 1);
    room.handle(1, p, &c).unwrap();
    reject(&c, &room, fire(&room, 1, 2));
    for p in [
        cast(1, 1, 2, (0., 0.)),
        cast(2, 1, 1, (0., 0.)),
        cast(1, 0, 1, (0., 0.)),
        {
            let mut p = cast(1, 1, 1, (0., 0.));
            p.body[9..13].copy_from_slice(&f32::NAN.to_be_bytes());
            p
        },
    ] {
        reject(&c, &room, p);
    }
    let mut changed = room.clone();
    changed.character_mut(1).stats.insert(9, 0xa62);
    reject(&c, &changed, cast(1, 1, 1, (0., 0.)));
    let position = (room.character(1).x, room.character(1).y);
    room.handle(
        1,
        packet(5, |w| {
            w.f32(position.0).f32(position.1);
        }),
        &c,
    )
    .unwrap();
    room.handle(1, prediction_runtime_tests::report(99999, 1, 1, 4), &c)
        .unwrap();
    room.handle(
        1,
        packet(23, |w| {
            w.i32(7);
        }),
        &c,
    )
    .unwrap();
    room.advance(8., &c).unwrap();
    assert_eq!(acknowledgement(&room, 1), Some(0xa61));
    let paid = room.character(1).clone();
    room.handle(1, cast(1, 1, 1, (0.5, 0.5)), &c).unwrap();
    assert_eq!(room.character(1), &paid);
    reject(&c, &room, cast(1, 1, 1, (0., 0.)));
}
#[test]
fn quiver_runtime_successful_swap_drop_teleport_detach_invalidate_declines_preserve() {
    let (c, mut room) = setup(false);
    archer(&mut room, &c, 1, 0xa61);
    archer(&mut room, &c, 2, 0xa61);
    let p = fire(&room, 1, 1);
    room.handle(1, p, &c).unwrap();
    room.character_mut(1).stats.insert(12, -1);
    let swap = |a, b| {
        packet(14, |w| {
            w.i32(1).u8(1).i16(a).i32(1).u8(4).i16(b);
        })
    };
    reject(&c, &room, swap(0xa62, -1));
    reject(
        &c,
        &room,
        packet(19, |w| {
            w.i32(1).u8(1).i16(0xa62);
        }),
    );
    reject(
        &c,
        &room,
        packet(28, |w| {
            w.i32(9999);
        }),
    );
    for operation in [
        swap(0xa61, -1),
        packet(19, |w| {
            w.i32(1).u8(1).i16(0xa61);
        }),
        packet(28, |w| {
            w.i32(2);
        }),
    ] {
        let mut r = room.clone();
        r.handle(1, operation, &c).unwrap();
        assert_eq!(acknowledgement(&r, 1), None);
    }
    let paid = room.character(1).clone();
    room.detach(1);
    room.attach(1, paid.clone(), &c).unwrap();
    assert_eq!(room.character(1), &paid);
    assert_eq!(acknowledgement(&room, 1), None);
    reject(&c, &room, cast(1, 1, 1, (0., 0.)));
}
fn bow(room: &Room, c: &Content, id: u8) -> Packet {
    let p = room.character(1);
    let kind = p.stats[&8] as i16;
    let visual = c
        .named(&c.get(kind).unwrap().normal_projectile(0).unwrap().visual)
        .unwrap()
        .id;
    packet(8, |w| {
        w.u8(id)
            .i32(1)
            .u8(visual as u8)
            .i16(kind)
            .f32(p.x + 0.3)
            .f32(p.y)
            .f32(0.)
            .i16(0);
    })
}
#[test]
fn quiver_runtime_shared_native_ids_wrap_retired_reuse_live_and_reserved_burst_collisions() {
    for item in [0xa61, 0xa63] {
        let (mut c, mut room) = setup(false);
        archer(&mut room, &c, 1, item);
        let p = bow(&room, &c, 95);
        room.handle(1, p, &c).unwrap();
        reject(&c, &room, fire(&room, 1, 95));
        let p = fire(&room, 1, 96);
        room.handle(1, p, &c).unwrap();
        reject(&c, &room, bow(&room, &c, 96));
        room.handle(1, cast(1, 1, 1, (0., 0.)), &c).unwrap();
        let p = bow(&room, &c, 1);
        room.handle(1, p, &c).unwrap();
        assert_eq!(
            room.players[&1].weapon.pending.as_ref().unwrap().shots[0].0,
            1
        );
        reject(&c, &room, fire(&room, 1, 1));
        reject(&c, &room, fire(&room, 1, 2));
        room.cancel_weapon(1);
        room.advance(1., &c).unwrap();
        room.character_mut(1).stats.insert(4, 100);
        room.character_mut(1).resources_carry = Some(ResourceCarry::ZERO);
        let p = fire(&room, 1, 96);
        room.handle(1, p, &c).unwrap();
        assert_eq!(room.bullets.len(), 1);
        assert!(room.server_projectile_cursors.is_empty());
        let (x, y) = (room.character(1).x, room.character(1).y);
        // A partial multi-arrow weapon cannot be interleaved with a native ability.
        let mut r = room.clone();
        r.bullets.clear();
        r.players.get_mut(&1).unwrap().quiver_ack = None;
        let kind = r.character(1).stats[&8] as i16;
        c.objects.get_mut(&kind).unwrap().num_projectiles = 2;
        let p = bow(&r, &c, 10);
        r.handle(1, p, &c).unwrap();
        assert_eq!(
            r.players[&1].weapon.pending.as_ref().unwrap().shots.len(),
            1
        );
        reject(&c, &r, native(1, 11, item, (x, y), 0., 0));
    }
}

fn target(room: &mut Room, c: &Content, position: (f32, f32), hp: i32) -> i32 {
    let id = room.allocate_entity().unwrap();
    let def = c.get(0x600).unwrap();
    let mut stats = def.stats.clone();
    stats.extend([(0, hp), (1, hp)]);
    room.entities.insert(
        id,
        Entity {
            id,
            kind: 0x600,
            x: position.0,
            y: position.1,
            stats,
            age: 0.,
            provenance: Provenance::Population,
        },
    );
    id
}
pub(crate) fn stats(out: &[Packet], packet_id: u8, id: i32) -> BTreeMap<u8, i32> {
    for p in out.iter().filter(|p| p.id == packet_id) {
        let mut r = Reader::new(&p.body);
        let count = if packet_id == 10 {
            r.i16().unwrap();
            r.utf().unwrap();
            1
        } else {
            r.i16().unwrap()
        };
        for _ in 0..count {
            let found = r.i32().unwrap();
            r.f32().unwrap();
            r.f32().unwrap();
            let n = r.i16().unwrap();
            let data = (0..n)
                .map(|_| (r.u8().unwrap(), r.i32().unwrap()))
                .collect();
            if found == id {
                return data;
            }
        }
    }
    panic!("missing {packet_id}/{id}")
}
#[test]
fn quiver_runtime_first_swept_hit_damage9_slow_projection_next_step_refresh_expiry() {
    let (c, mut room) = setup(false);
    archer(&mut room, &c, 1, 0xa61);
    room.shore = None;
    let (x, y) = (room.character(1).x, room.character(1).y);
    let first = target(&mut room, &c, (x + 2., y), 1000);
    let second = target(&mut room, &c, (x + 3., y), 1000);
    room.enemy_slows.insert(first, 10.);
    let p = fire(&room, 1, 1);
    room.handle(1, p, &c).unwrap();
    let damage = room.bullets[0].damage;
    let events = room.advance(0.2, &c).unwrap();
    assert!(room.bullets.is_empty());
    assert_eq!(room.entities[&first].stats[&1], 1000 - damage);
    assert_eq!(room.entities[&second].stats[&1], 1000);
    assert_eq!(room.enemy_slows[&first], 3.); // overwrite, including a previously longer remainder
    assert!(!room.enemy_slows.contains_key(&second));
    let mut r = Reader::new(&events.iter().find(|p| p.id == 9).unwrap().body);
    assert_eq!(
        (r.i32().unwrap(), r.u8().unwrap(), r.u8().unwrap()),
        (first, 4, 3)
    );
    assert_eq!(r.i16().unwrap(), damage as i16);
    assert_eq!((r.u8().unwrap(), r.i32().unwrap()), (1, 1));
    r.done().unwrap();
    let out = room.publication(1, &events, &c).unwrap();
    assert_eq!(stats(&out, 10, first)[&29], 8);
    assert_eq!(stats(&out, 13, first)[&29], 8);
    assert!(!stats(&out, 13, first).contains_key(&25));
    assert_eq!(room.character(1).stats[&25], 0);
    room.advance(2.875, &c).unwrap();
    assert_eq!(room.enemy_slows[&first], 0.125);
    let events = room.advance(0.125, &c).unwrap();
    assert!(!room.enemy_slows.contains_key(&first));
    assert_eq!(
        stats(&room.publication(1, &events, &c).unwrap(), 13, first)[&29],
        0
    );
    assert_eq!(room.entities[&first].stats[&1], 1000 - damage);
}
#[test]
fn quiver_runtime_slow_halves_chase_return_and_wander_whole_step_then_clears_clocks_unscaled() {
    let (c, mut base) = setup(false);
    archer(&mut base, &c, 1, 0xa61);
    let (x, y) = (base.character(1).x, base.character(1).y);
    let id = target(&mut base, &c, (x + 2.5, y), 1000);
    base.shore = Some(ShorePopulation {
        sites: vec![],
        actors: BTreeMap::from([(
            id,
            ShorePirate {
                home: (x + 4., y),
                next_shot: 0.,
            },
        )]),
        next_replenish: 100.,
    });
    for returning in [false, true] {
        let mut normal = base.clone();
        if returning {
            normal.character_mut(1).x = x + 40.;
        }
        for remaining in [0.025, 0.1, 0.] {
            let mut slow = normal.clone();
            slow.enemy_slows.insert(id, remaining);
            let before = (normal.entities[&id].x, normal.entities[&id].y);
            let mut n = normal.clone();
            let ordinary = n.advance(0.1, &c).unwrap();
            let slowed = slow.advance(0.1, &c).unwrap();
            let d = distance(before.0, before.1, n.entities[&id].x, n.entities[&id].y);
            let ds = distance(
                before.0,
                before.1,
                slow.entities[&id].x,
                slow.entities[&id].y,
            );
            let speed = if returning { 2.96 } else { 6.29 };
            assert!((d - speed * 0.1).abs() < 0.00002, "{d}");
            let factor = if remaining > 0. { 0.5 } else { 1. };
            assert!((ds - d * factor).abs() < 0.00002, "{ds}/{d}");
            assert!(!slow.enemy_slows.contains_key(&id));
            assert_eq!(n.entities[&id].age, slow.entities[&id].age);
            assert_eq!(
                n.shore.as_ref().unwrap().actors[&id].next_shot,
                slow.shore.as_ref().unwrap().actors[&id].next_shot
            );
            assert_eq!(
                ordinary.iter().filter(|p| p.id == 8).count(),
                slowed.iter().filter(|p| p.id == 8).count()
            );
        }
    }
    let mut n = base.clone();
    n.shore = None;
    n.entities.get_mut(&id).unwrap().kind = 0x801;
    let home = target(&mut n, &c, (x + 2.5, y), 5);
    n.entities.get_mut(&home).unwrap().kind = 0x800;
    let mut slow = n.clone();
    slow.enemy_slows.insert(id, 3.);
    let before = (n.entities[&id].x, n.entities[&id].y);
    n.advance(0.1, &c).unwrap();
    slow.advance(0.1, &c).unwrap();
    let d = distance(before.0, before.1, n.entities[&id].x, n.entities[&id].y);
    let ds = distance(
        before.0,
        before.1,
        slow.entities[&id].x,
        slow.entities[&id].y,
    );
    assert!((d - 0.04).abs() < 0.00002);
    assert!((ds - d * 0.5).abs() < 0.00002);
}
#[test]
fn quiver_runtime_cover_tie_age_lifetime_first_death_rewards_and_cleanup() {
    let (c, mut base) = setup(false);
    archer(&mut base, &c, 1, 0xa61);
    base.shore = None;
    let (x, y) = (base.character(1).x, base.character(1).y);
    for (age, delta, hit) in [
        (0., 0.1, true),
        (0., 1., false),
        (0.9, 0.1, false),
        (0.5, 0.4, true),
    ] {
        let mut room = base.clone();
        let id = target(&mut room, &c, (x + 1.5, y), 5);
        let p = fire(&room, 1, 1);
        room.handle(1, p, &c).unwrap();
        room.bullets[0].age = age;
        room.advance(delta, &c).unwrap();
        assert_eq!(!room.entities.contains_key(&id), hit);
        assert_eq!(room.character(1).stats[&6], if hit { 1 } else { 0 });
        room.advance(0., &c).unwrap();
        assert_eq!(room.character(1).stats[&6], if hit { 1 } else { 0 });
        assert!(!room.enemy_slows.contains_key(&id));
    }
    for tie in [false, true] {
        let mut room = base.clone();
        let pos = if tie { (x + 0.3, y) } else { (x + 3., y) };
        let id = target(&mut room, &c, pos, 1000);
        let wall = c
            .objects
            .values()
            .find(|o| o.is_static && o.occupy_square)
            .unwrap();
        let cover = room.allocate_entity().unwrap();
        room.entities.insert(
            cover,
            Entity {
                id: cover,
                kind: wall.id,
                x: if tie { x } else { x + 1. },
                y,
                stats: wall.stats.clone(),
                age: 0.,
                provenance: Provenance::Map,
            },
        );
        let p = fire(&room, 1, 1);
        room.handle(1, p, &c).unwrap();
        room.advance(0.2, &c).unwrap();
        assert_eq!(room.entities[&id].stats[&1] < 1000, tie);
        assert_eq!(room.enemy_slows.contains_key(&id), tie);
        assert!(room.bullets.is_empty());
        assert!(!room.enemy_slows.contains_key(&cover));
    }
    // Removal/new population IDs never inherit a transient timer.
    let id = target(&mut base, &c, (x + 1., y), 1000);
    base.enemy_slows.insert(id, 3.);
    base.entities.remove(&id);
    base.advance(0., &c).unwrap();
    assert!(!base.enemy_slows.contains_key(&id));
    let replacement = target(&mut base, &c, (x + 1., y), 1000);
    assert_ne!(replacement, id);
    assert_eq!(
        base.entity_wire_stats(&base.entities[&replacement], &c)
            .unwrap()[&29],
        0
    );
}
#[test]
fn quiver_runtime_death_clears_receipt_projectile_damage_and_ground() {
    let (c, mut base) = setup(false);
    archer(&mut base, &c, 1, 0xa61);
    base.shore = None;
    let input = fire(&base, 1, 1);
    base.handle(1, input, &c).unwrap();
    let (x, y) = (base.character(1).x, base.character(1).y);
    let id = target(&mut base, &c, (x, y), 5);
    base.bullets.push(Bullet {
        victims: BTreeSet::new(),
        flight: tests::normal_flight(1., 1.),
        owner: id,
        id: 1,
        origin: (x, y),
        x,
        y,
        age: 0.,
        amplitude: 0.,
        frequency: 1.,
        vx: 0.,
        vy: 0.,
        damage: 1000,
        life: 1.,
    });
    base.advance(0., &c).unwrap();
    assert!(!base.character(1).alive);
    assert_eq!(acknowledgement(&base, 1), None);
}

#[test]
fn quiver_runtime_capacity_before_payment_rng_marker_and_actual_bounds() {
    let (c, mut base) = setup(false);
    archer(&mut base, &c, 1, 0xa61);
    let mut template = base.clone();
    let input = fire(&template, 1, 96);
    template.handle(1, input, &c).unwrap();
    let bullet = template.bullets.pop().unwrap();
    for (total, owner_count, accepted) in [
        (4095, 0, true),
        (4096, 0, false),
        (255, 255, true),
        (256, 256, false),
    ] {
        let mut r = base.clone();
        r.bullets = (0..total)
            .map(|index| Bullet {
                owner: if index < owner_count { 1 } else { 2 },
                id: 97,
                ..bullet.clone()
            })
            .collect();
        let input = fire(&r, 1, 96);
        if accepted {
            r.handle(1, input, &c).unwrap();
            assert_eq!(r.bullets.len(), total + 1);
            assert_eq!(r.character(1).stats[&4], 40);
        } else {
            reject(&c, &r, input);
        }
    }
    for (position, origin, accepted) in [
        ((0.3, 0.3), (0., 0.), true),
        ((0.3, 0.3), (-0.01, 0.3), false),
        ((255.5, 255.5), (255.99, 255.99), true),
        ((255.5, 255.5), (256., 255.5), false),
    ] {
        let mut r = base.clone();
        (r.character_mut(1).x, r.character_mut(1).y) = position;
        let p = native(1, 1, 0xa61, origin, 0., 0);
        if accepted {
            r.handle(1, p, &c).unwrap();
        } else {
            reject(&c, &r, p);
        }
    }
}
#[test]
fn quiver_runtime_ground_death_clears_marker_and_predicted_repair_retains_current_slow() {
    let (c, mut r) = setup(false);
    archer(&mut r, &c, 1, 0xa61);
    r.shore = None;
    let input = fire(&r, 1, 1);
    r.handle(1, input, &c).unwrap();
    let p = r.character(1);
    let position = (p.x + 2., p.y);
    let id = target(&mut r, &c, position, 1000);
    let events = r.advance(0.2, &c).unwrap();
    r.publication(1, &events, &c).unwrap();
    let before = snapshot(&r);
    let events = r
        .handle(1, prediction_runtime_tests::report(id, 1, 1, 1), &c)
        .unwrap();
    let out = r.publication(1, &events, &c).unwrap();
    assert_eq!(prediction_runtime_tests::target_packets(&out, id), [11, 10]);
    assert_eq!(stats(&out, 10, id)[&29], 8);
    assert_eq!(snapshot(&r), before);
    let map = r.map.as_ref().unwrap();
    let (w, h) = map.dimensions();
    let hazard = (0..i32::from(h))
        .flat_map(|y| (0..i32::from(w)).map(move |x| (x, y)))
        .find(|&(x, y)| {
            map.ground(x, y)
                .unwrap()
                .damage
                .is_some_and(|(min, _)| min > 0)
        })
        .unwrap();
    let p = r.character_mut(1);
    (p.x, p.y) = (hazard.0 as f32 + 0.5, hazard.1 as f32 + 0.5);
    p.stats.insert(1, 1);
    r.advance(0.5, &c).unwrap();
    assert!(!r.character(1).alive);
    assert_eq!(acknowledgement(&r, 1), None);
}

#[test]
fn quiver_runtime_iron_native8_paid_once_and_continuing_two_targets() {
    for repaired in [false, true] {
        let (c, mut room) = setup(repaired);
        archer(&mut room, &c, 1, 0xa63);
        room.shore = None;
        let p = room.character(1);
        let (x, y) = (p.x, p.y);
        let a = target(&mut room, &c, (x + 1., y), 500);
        let b = target(&mut room, &c, (x + 2., y), 500);
        let input = fire(&room, 1, 96);
        room.handle(1, input, &c)
            .expect("Iron native8 must be supported");
        assert_eq!(room.character(1).stats[&4], 20);
        let paid = room.character(1).clone();
        room.handle(1, cast(1, 1, 1, (0., 0.)), &c).unwrap();
        assert_eq!(room.character(1), &paid);
        let damage = room.bullets[0].damage;
        let events = room.advance(0.2, &c).unwrap();
        assert_eq!(room.bullets.len(), 1);
        assert_eq!(room.bullets[0].victims, BTreeSet::from([a, b]));
        assert_eq!(room.bullets[0].id, 96);
        for id in [a, b] {
            assert_eq!(room.entities[&id].stats[&1], 500 - damage);
            assert_eq!(room.enemy_slows[&id], 3.);
        }
        assert_eq!(events.iter().filter(|p| p.id == 9).count(), 2);
    }
}

#[test]
fn quiver_runtime_continues_damage9_nonretiring_presentation_not_native_acceptance() {
    let (c, mut room) = setup(false);
    archer(&mut room, &c, 1, 0xa63);
    room.shore = None;
    let p = room.character(1);
    let position = (p.x + 1., p.y);
    let victim = target(&mut room, &c, position, 500);
    let input = fire(&room, 1, 96);
    room.handle(1, input, &c).unwrap();
    let damage = room.bullets[0].damage;
    let events = room.advance(0.1, &c).unwrap();
    let mut r = Reader::new(&events.iter().find(|p| p.id == 9).unwrap().body);
    assert_eq!(
        (r.i32().unwrap(), r.u8().unwrap(), r.u8().unwrap()),
        (victim, 4, 3)
    );
    assert_eq!(r.i16().unwrap(), damage as i16);
    let (presentation, owner) = (r.u8().unwrap(), r.i32().unwrap());
    assert!(
        !(owner >= 0 && presentation > 0),
        "original M8 removes positive owner/bullet before looking up victim"
    );
    assert_eq!((owner, presentation), (1, 0));
    assert_eq!(room.bullets[0].id, 96);
}

fn hit_targets(events: &[Packet]) -> Vec<i32> {
    events
        .iter()
        .filter(|p| p.id == 9)
        .map(|p| Reader::new(&p.body).i32().unwrap())
        .collect()
}

#[test]
fn quiver_runtime_iron_shape_visual_dex_and_noncontinuing_wire_unchanged() {
    for repaired in [false, true] {
        let (mut c, mut room) = setup(repaired);
        archer(&mut room, &c, 1, 0xa63);
        let iron = c.get(0xa63).unwrap().clone();
        let projectile = iron.quiver_projectile().unwrap();
        assert_eq!(
            (iron.root_damage, iron.mp_cost, &iron.bonuses),
            (Some((90, 110)), Some(80), &vec![(28, 2)])
        );
        assert_eq!(
            (
                projectile.min,
                projectile.max,
                projectile.speed,
                projectile.lifetime
            ),
            (90, 110, 15., 1.)
        );
        assert_eq!(
            (
                &*projectile.visual,
                projectile.continues,
                &projectile.features
            ),
            (
                "Green Arrow",
                true,
                &vec!["ConditionEffect".to_string(), "Continues".to_string()]
            )
        );
        assert_eq!(c.named(&projectile.visual).unwrap().id, 9);
        assert_eq!(
            c.get(room.character(1).stats[&8] as i16)
                .unwrap()
                .normal_projectile(0)
                .unwrap()
                .visual,
            projectile.visual
        );
        assert!(iron.normal_projectile(0).is_err());
        assert!(iron.arena_projectile(0).is_err());
        for case in 0..14 {
            let item = c.objects.get_mut(&0xa63).unwrap();
            match case {
                0 => item.bonuses.clear(),
                1 => item.bonuses.push((28, 2)),
                2 => item.bonuses[0].1 = 3,
                3 => item.bonuses[0].0 = 20,
                4 => item.projectiles[0].1.continues = false,
                5 => item.projectiles[0].1.features.reverse(),
                6 => item.projectiles[0].1.features.push("Continues".into()),
                7 => item.projectiles[0].1.visual = "Red Arrow".into(),
                8 => {
                    item.projectiles[0].1.effects[0].condition =
                        crate::content::Condition::Paralyzed
                }
                9 => item.root_damage = Some((89, 110)),
                10 => item.projectiles[0].1.features.push("Unknown".into()),
                11 => item.activations.push(item.activations[0].clone()),
                12 => item.mp_cost = None,
                13 => item.projectiles.push(item.projectiles[0].clone()),
                _ => unreachable!(),
            }
            reject(&c, &room, fire(&room, 1, 1));
            c.objects.insert(0xa63, iron.clone());
        }
        let green = c.objects.remove(&9).unwrap();
        reject(&c, &room, fire(&room, 1, 1));
        c.objects.insert(9, green);
        let mut wrong_visual = fire(&room, 1, 1);
        wrong_visual.body[5] = 10;
        reject(&c, &room, wrong_visual);
        let base_dex = room.character(1).stats[&28];
        for _ in 0..3 {
            let p = room.character(1).clone();
            let effective = p.equipment_stats(&c).unwrap();
            assert_eq!(effective.get(28).unwrap(), base_dex + 2);
            assert_eq!(room.wire_stats(&p, &c).unwrap()[&28], base_dex + 2);
            assert!(
                combat::weapon_interval_ms(
                    c.get(p.stats[&8] as i16).unwrap(),
                    base_dex + 2,
                    combat::ConditionMask::default()
                )
                .unwrap()
                    < combat::weapon_interval_ms(
                        c.get(p.stats[&8] as i16).unwrap(),
                        base_dex,
                        combat::ConditionMask::default()
                    )
                    .unwrap()
            );
            room.detach(1);
            room.attach(1, p, &c).unwrap();
            assert_eq!(room.character(1).stats[&28], base_dex);
        }
        for item in [0xa61, 0xa62] {
            let mut r = room.clone();
            r.shore = None;
            r.character_mut(1).stats.insert(9, item);
            let position = (r.character(1).x + 1., r.character(1).y);
            let victim = target(&mut r, &c, position, 500);
            let input = fire(&r, 1, 95);
            r.handle(1, input, &c).unwrap();
            let damage = r.bullets[0].damage;
            let events = r.advance(0.1, &c).unwrap();
            assert!(r.bullets.is_empty());
            let expected = packet(9, |w| {
                w.i32(victim).u8(4).u8(3).i16(damage as i16).u8(95).i32(1);
            });
            assert_eq!(
                events.iter().find(|p| p.id == 9).unwrap().body,
                expected.body
            );
        }
    }
}

#[test]
fn quiver_runtime_iron_separate_contacts_reentry_repair_tombstones_replacement_and_later_payment() {
    let (c, mut room) = setup(false);
    for id in [1, 2] {
        archer(&mut room, &c, id, 0xa63);
    }
    room.shore = None;
    let (x, y) = (room.character(1).x, room.character(1).y);
    let a = target(&mut room, &c, (x + 1., y), 500);
    let b = target(&mut room, &c, (x + 5., y), 500);
    for id in [1, 2] {
        room.publication(id, &[], &c).unwrap();
    }
    let input = fire(&room, 1, 96);
    room.handle(1, input, &c).unwrap();
    room.handle(1, cast(1, 1, 1, (0., 0.)), &c).unwrap();
    let damage = room.bullets[0].damage;
    assert_eq!(hit_targets(&room.advance(0.05, &c).unwrap()), [a]);
    assert!(hit_targets(&room.advance(0., &c).unwrap()).is_empty());
    for condition in [1, 4, 1] {
        let before = room.clone();
        let events = room
            .handle(1, prediction_runtime_tests::report(a, 1, 96, condition), &c)
            .unwrap();
        let out = room.publication(1, &events, &c).unwrap();
        if condition == 1 {
            assert_eq!(prediction_runtime_tests::target_packets(&out, a), [11, 10]);
            assert_eq!(stats(&out, 10, a)[&29], 8);
        }
        assert!(room.publication(2, &events, &c).unwrap().is_empty());
        prediction_runtime_tests::authority_unchanged(&before, &room);
    }
    assert!(hit_targets(&room.advance(0.05, &c).unwrap()).is_empty());
    // A fresh native instance or movement back into overlap never replaces Room identity.
    room.entities.get_mut(&a).unwrap().x = room.bullets[0].x;
    assert!(hit_targets(&room.advance(0., &c).unwrap()).is_empty());
    assert_eq!(room.enemy_slows[&a], 2.95);
    assert_eq!(room.entities[&a].stats[&1], 500 - damage);
    assert_eq!(hit_targets(&room.advance(0.2, &c).unwrap()), [b]);
    assert_eq!(room.bullets[0].victims, BTreeSet::from([a, b]));
    let victims = room.bullets[0].victims.clone();
    room.character_mut(1).x += 40.;
    room.publication(1, &[], &c).unwrap();
    assert!(!room.players[&1].known_objects.contains(&b));
    room.character_mut(1).x = x;
    room.publication(1, &[], &c).unwrap();
    assert_eq!(room.bullets[0].victims, victims);
    room.entities.remove(&a);
    let position = (room.bullets[0].x, room.bullets[0].y);
    let replacement = target(&mut room, &c, position, 500);
    assert_ne!(replacement, a);
    assert_eq!(hit_targets(&room.advance(0., &c).unwrap()), [replacement]);
    assert!(!room.enemy_slows.contains_key(&a));
    assert_eq!(room.bullets[0].victims, BTreeSet::from([a, b, replacement]));
    assert!(hit_targets(&room.advance(0., &c).unwrap()).is_empty());
    room.advance(0.7, &c).unwrap();
    assert!(room.bullets.is_empty());
    room.character_mut(1).stats.insert(4, 80);
    let input = fire(&room, 1, 1);
    room.handle(1, input, &c).unwrap();
    assert!(room.bullets[0].victims.is_empty());
    let second_damage = room.bullets[0].damage;
    assert_eq!(
        hit_targets(&room.advance(0.3, &c).unwrap()),
        [replacement, b]
    );
    assert_eq!(room.entities[&b].stats[&1], 500 - damage - second_damage);
    assert_eq!(room.enemy_slows[&b], 3.);
}

#[test]
fn quiver_runtime_iron_cover_entry_order_ties_tangency_t0_and_expiry_before_contacts() {
    let (c, mut base) = setup(false);
    archer(&mut base, &c, 1, 0xa63);
    base.shore = None;
    let (x, y) = (base.character(1).x, base.character(1).y);
    for cover_offset in [0., 1., 2., 4.] {
        let mut room = base.clone();
        let initial = target(&mut room, &c, (x + 0.3, y), 500);
        let far = target(&mut room, &c, (x + 3., y), 500);
        let near = target(&mut room, &c, (x + 1., y), 500);
        let tie = target(&mut room, &c, (x + 1., y), 500);
        let wall = c
            .objects
            .values()
            .find(|o| o.is_static && o.occupy_square)
            .unwrap();
        let id = target(&mut room, &c, (x + cover_offset, y), 500);
        room.entities.get_mut(&id).unwrap().kind = wall.id;
        let input = fire(&room, 1, 1);
        room.handle(1, input, &c).unwrap();
        assert_eq!(
            hit_targets(&room.advance(0.25, &c).unwrap()),
            [initial, near, tie, far]
        );
        assert_eq!(room.bullets.len(), 1);
        assert!(!room.bullets[0].victims.contains(&id));
    }
    let mut room = base.clone();
    let tangent = target(&mut room, &c, (x + 0.5, 0.), 500);
    let inside = target(&mut room, &c, (x, 0.65), 500);
    let dead = target(&mut room, &c, (x, 0.65), 0);
    let input = native(1, 1, 0xa63, (x, 0.65), 0., 0);
    room.character_mut(1).y = 0.65;
    room.handle(1, input, &c).unwrap();
    assert_eq!(hit_targets(&room.advance(0., &c).unwrap()), [inside]);
    assert!(hit_targets(&room.advance(0.2, &c).unwrap()).is_empty());
    assert_eq!(room.entities[&tangent].stats[&1], 500);
    assert!(!room.bullets[0].victims.contains(&dead));
    for (age, dt, hit) in [
        (0., 1., false),
        (0.9, 0.1, false),
        (0., 1.2, false),
        (0.5, 0.4, true),
    ] {
        let mut room = base.clone();
        let id = target(&mut room, &c, (x + 2., y), 500);
        let input = fire(&room, 1, 1);
        room.handle(1, input, &c).unwrap();
        room.bullets[0].age = age;
        let events = room.advance(dt, &c).unwrap();
        assert_eq!(hit_targets(&events).contains(&id), hit);
        assert_eq!(room.bullets.is_empty(), !hit);
        assert!(hit_targets(&room.advance(0., &c).unwrap()).is_empty());
    }
}

#[test]
fn quiver_runtime_iron_buffered_dead_skip_keeps_reservation_and_once_rewards() {
    let (c, mut room) = setup(false);
    for id in [1, 2] {
        archer(&mut room, &c, id, 0xa63);
    }
    room.shore = None;
    let (x, y) = (room.character(1).x, room.character(1).y);
    let dead = target(&mut room, &c, (x + 1., y), 5);
    let survivor = target(&mut room, &c, (x + 2., y), 500);
    for owner in [1, 2] {
        let input = fire(&room, owner, 1);
        room.handle(owner, input, &c).unwrap();
    }
    let mut once = room.clone();
    once.bullets.pop();
    once.advance(0.2, &c).unwrap();
    let total: i32 = room.bullets.iter().map(|b| b.damage).sum();
    let events = room.advance(0.2, &c).unwrap();
    assert_eq!(hit_targets(&events), [dead, survivor, survivor]);
    for bullet in &room.bullets {
        assert_eq!(bullet.victims, BTreeSet::from([dead, survivor]));
    }
    assert!(!room.entities.contains_key(&dead));
    assert!(!room.enemy_slows.contains_key(&dead));
    assert_eq!(room.entities[&survivor].stats[&1], 500 - total);
    for id in [1, 2] {
        assert_eq!(room.character(id).stats[&6], once.character(id).stats[&6]);
    }
    assert_eq!(room.rng.clone().next_u64(), once.rng.clone().next_u64());
    assert_eq!(room.next_id, once.next_id);
    assert_eq!(
        room.entities.values().filter(|e| e.kind == 0x500).count(),
        once.entities.values().filter(|e| e.kind == 0x500).count()
    );
    let xp = room.character(1).stats[&6];
    assert!(hit_targets(&room.advance(0., &c).unwrap()).is_empty());
    assert_eq!(room.character(1).stats[&6], xp);
}

#[test]
fn quiver_runtime_iron_both_actual_victims_slow_next_step_overwrite_expiry_mask_cleanup() {
    let (c, mut room) = setup(false);
    archer(&mut room, &c, 1, 0xa63);
    room.shore = None;
    let (x, y) = (room.character(1).x, room.character(1).y);
    let a = target(&mut room, &c, (x + 3., y), 500);
    let b = target(&mut room, &c, (x + 5., y), 500);
    for id in [a, b] {
        room.enemy_slows.insert(id, 10.);
    }
    let input = fire(&room, 1, 1);
    room.handle(1, input, &c).unwrap();
    assert_eq!(hit_targets(&room.advance(0.4, &c).unwrap()), [a, b]);
    room.shore = Some(ShorePopulation {
        sites: vec![],
        next_replenish: 100.,
        actors: [a, b]
            .into_iter()
            .map(|id| {
                (
                    id,
                    ShorePirate {
                        home: (room.entities[&id].x, y),
                        next_shot: 0.,
                    },
                )
            })
            .collect(),
    });
    let before = room.clone();
    let mut normal = room.clone();
    normal.enemy_slows.clear();
    let ordinary = normal.advance(0.1, &c).unwrap();
    let slowed = room.advance(0.1, &c).unwrap();
    for id in [a, b] {
        let origin = &before.entities[&id];
        let n = &normal.entities[&id];
        let s = &room.entities[&id];
        let d = distance(origin.x, origin.y, n.x, n.y);
        let ds = distance(origin.x, origin.y, s.x, s.y);
        assert!((d - 0.629).abs() < 0.00002);
        assert!((ds - 0.5 * d).abs() < 0.00002);
        assert_eq!(s.age, n.age);
        assert_eq!(
            room.shore.as_ref().unwrap().actors[&id].next_shot,
            normal.shore.as_ref().unwrap().actors[&id].next_shot
        );
        assert_eq!(room.enemy_slows[&id], 2.9);
        assert_eq!(room.entity_wire_stats(s, &c).unwrap()[&29], 8);
    }
    assert_eq!(
        ordinary.iter().filter(|p| p.id == 8).count(),
        slowed.iter().filter(|p| p.id == 8).count()
    );
    room.advance(2.775, &c).unwrap();
    assert_eq!(room.enemy_slows[&a], 0.125);
    room.advance(0.125, &c).unwrap();
    for id in [a, b] {
        assert!(!room.enemy_slows.contains_key(&id));
        assert_eq!(
            room.entity_wire_stats(&room.entities[&id], &c).unwrap()[&29],
            0
        );
    }
    assert_eq!(room.character(1).stats[&25], 0);
    room.entities.remove(&a);
    room.advance(0., &c).unwrap();
    let replacement = target(&mut room, &c, (x + 3., y), 500);
    assert_eq!(
        room.entity_wire_stats(&room.entities[&replacement], &c)
            .unwrap()[&29],
        0
    );
}

pub(crate) fn iron_collision_fixture(room: &mut Room, c: &Content, owner: i32) -> [i32; 2] {
    room.shore = None;
    room.entities.clear();
    let (x, y) = (room.character(owner).x, room.character(owner).y);
    [
        target(room, c, (x + 1., y), 5),
        target(room, c, (x + 2., y), 500),
    ]
}
pub(crate) fn assert_iron_collision(room: &Room, targets: [i32; 2]) {
    assert!(!room.entities.contains_key(&targets[0]));
    assert!(!room.enemy_slows.contains_key(&targets[0]));
    assert!(room.entities[&targets[1]].stats[&1] < 500);
    assert_eq!(room.enemy_slows[&targets[1]], 3.);
    assert_eq!(room.bullets.len(), 1);
    assert_eq!(room.bullets[0].victims, BTreeSet::from(targets));
}
pub(crate) fn assert_iron_reward_candidate(before: &Room, candidate: &Room) {
    assert_ne!(
        before.rng.clone().next_u64(),
        candidate.rng.clone().next_u64()
    );
    assert!(candidate.next_id > before.next_id);
    assert!(candidate.entities.values().any(|e| e.kind == 0x500));
}
