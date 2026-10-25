use super::quiver_runtime_tests::{bow, snapshot, stats, target};
use super::spell_runtime_tests::{cast, setup};
use super::*;
use crate::content::{Condition, ConditionEffect};
use rand::RngCore;

pub(crate) fn knight(room: &mut Room, c: &Content, id: i32) {
    let mut p = Character::new(c, 0x31e, format!("Knight{id}"), 1).unwrap();
    (p.x, p.y) = room.landing();
    room.attach(id, p, c).unwrap();
}
pub(crate) fn fire(room: &Room, owner: i32, id: u8) -> Packet {
    let p = room.character(owner);
    native(owner, id, p.stats[&9] as i16, (p.x + 0.3, p.y), 0., 32767)
}
fn native(owner: i32, id: u8, item: i16, pos: (f32, f32), angle: f32, claim: i16) -> Packet {
    packet(8, |w| {
        w.u8(id).i32(owner).u8(27).i16(item);
        for value in [pos.0, pos.1, angle] {
            w.0.extend(value.to_be_bytes());
        }
        w.i16(claim);
    })
}
fn reject(c: &Content, room: &Room, input: Packet) {
    let mut candidate = room.clone();
    let before = snapshot(&candidate);
    assert!(candidate.handle(1, input, c).is_err());
    assert_eq!(snapshot(&candidate), before);
}
fn hits(events: &[Packet]) -> Vec<i32> {
    events
        .iter()
        .filter(|p| p.id == 9)
        .map(|p| Reader::new(&p.body).i32().unwrap())
        .collect()
}

#[test]
fn shield_runtime_original_repaired_exact_represented_shape_and_context_fail_closed() {
    for repaired in [false, true] {
        let (mut c, mut room) = setup(repaired);
        knight(&mut room, &c, 1);
        let original = c.get(0xa08).unwrap().clone();
        let p = original.shoot_ability_projectile(0x31e).unwrap();
        assert_eq!(
            (original.slot_type, original.mp_cost, original.root_damage),
            (Some(5), Some(80), Some((30, 70)))
        );
        assert_eq!(original.bonuses, [(21, 2)]);
        assert_eq!((p.min, p.max, p.speed, p.lifetime), (30, 70, 16., 0.2));
        assert_eq!(
            (&*p.visual, p.continues, p.features.as_slice()),
            (
                "Silver Shield",
                true,
                &["ConditionEffect".to_owned(), "Continues".to_owned()][..]
            )
        );
        assert_eq!(
            p.effects,
            [ConditionEffect {
                condition: Condition::Stunned,
                duration_seconds: 3.
            }]
        );
        assert_eq!(p.effects, original.effects);
        assert_eq!(c.named(&p.visual).unwrap().id, 27);
        assert_eq!(Condition::Stunned as u8, 7);
        assert_eq!(combat::ConditionMask::bit(Condition::Stunned), 64);
        assert!(original.normal_projectile(0).is_err());
        assert!(original.arena_projectile(0).is_err());
        assert!(original.quiver_projectile().is_err());
        for case in 0..32 {
            let item = c.objects.get_mut(&0xa08).unwrap();
            match case {
                0 => item.class = "Character".into(),
                1 => item.slot_type = Some(15),
                2 => item.consumable = true,
                3 => item.activations.clear(),
                4 => item.activations.push(item.activations[0].clone()),
                5 => {
                    item.activations[0]
                        .attributes
                        .insert("unknown".into(), "1".into());
                }
                6 => item.activations[0].name = "ConditionEffectSelf".into(),
                7 => item.num_projectiles = 2,
                8 => item.arc_gap_radians = 0.,
                9 => item.rate_of_fire = Some(1.),
                10 => item.mp_cost = Some(79),
                11 => item.root_damage = Some((31, 70)),
                12 => item.bonuses.clear(),
                13 => item.bonuses.push((21, 2)),
                14 => item.bonuses[0].1 = 4,
                15 => item.projectiles.push(item.projectiles[0].clone()),
                16 => item.projectiles[0].0 = 1,
                17 => item.projectiles[0].1.visual = "Red Arrow".into(),
                18 => item.projectiles[0].1.speed = 15.,
                19 => item.projectiles[0].1.lifetime = 1.,
                20 => item.projectiles[0].1.min = 31,
                21 => item.projectiles[0].1.max = 71,
                22 => item.projectiles[0].1.wavy = true,
                23 => item.projectiles[0].1.amplitude = 1.,
                24 => item.projectiles[0].1.frequency = 2.,
                25 => item.projectiles[0].1.continues = false,
                26 => item.projectiles[0].1.features.reverse(),
                27 => item.projectiles[0].1.features.push("Unknown".into()),
                28 => item.projectiles[0].1.effects[0].condition = Condition::Dazed,
                29 => item.projectiles[0].1.effects.push(item.effects[0].clone()),
                30 => item.effects[0].duration_seconds = 4.,
                31 => item.effects.push(item.effects[0].clone()),
                _ => unreachable!(),
            }
            reject(&c, &room, fire(&room, 1, 1));
            c.objects.insert(0xa08, original.clone());
        }
        for class in [0x307, 0x30e, 0x310] {
            assert!(original.shoot_ability_projectile(class).is_err());
        }
        for item in [0xa09, 0xa0a, 0xa0b, 0xa0c, 0xa61] {
            assert!(
                c.get(item)
                    .unwrap()
                    .shoot_ability_projectile(0x31e)
                    .is_err()
            );
            room.character_mut(1).stats.insert(9, i32::from(item));
            reject(&c, &room, fire(&room, 1, 1));
        }
        for item in [0xa1a, 0x600, 0xa2e] {
            let mut ordinary = c.get(item).unwrap().clone();
            ordinary.projectiles[0].1 = p.clone();
            assert!(ordinary.normal_projectile(0).is_err());
            assert!(ordinary.arena_projectile(0).is_err());
            assert!(ordinary.shoot_ability_projectile(0x31e).is_err());
        }
        room.character_mut(1).stats.insert(9, 0xa08);
        let visual = c.objects.remove(&27).unwrap();
        reject(&c, &room, fire(&room, 1, 1));
        c.objects.insert(27, visual);
        let original_visual = c.objects[&27].clone();
        c.objects.get_mut(&27).unwrap().class = "Equipment".into();
        reject(&c, &room, fire(&room, 1, 1));
        c.objects.insert(27, original_visual.clone());
        c.objects.get_mut(&27).unwrap().id = 28;
        let mut wrong = fire(&room, 1, 1);
        wrong.body[5] = 28;
        reject(&c, &room, wrong);
        c.objects.insert(27, original_visual);
        assert_eq!(
            room.character(1)
                .equipment_stats(&c)
                .unwrap()
                .get(21)
                .unwrap(),
            2
        );
        assert_eq!(room.character(1).stats[&21], 0);
    }
}

#[test]
fn shield_runtime_mp_readiness_signed_carry_unscaled_rng_and_no_player_stun() {
    let (c, mut room) = setup(false);
    knight(&mut room, &c, 1);
    for mp in [79, 80, 81] {
        let mut r = room.clone();
        r.character_mut(1).stats.insert(4, mp);
        let input = fire(&r, 1, 1);
        if mp < 80 {
            reject(&c, &r, input);
        } else {
            r.handle(1, input, &c).unwrap();
            assert_eq!(r.character(1).stats[&4], mp - 80);
        }
    }
    for elapsed in [0.499, 0.500, 0.501] {
        let mut r = room.clone();
        let input = fire(&r, 1, 96);
        r.handle(1, input, &c).unwrap();
        r.handle(1, cast(1, 1, 1, (0., 0.)), &c).unwrap();
        r.advance(elapsed, &c).unwrap();
        r.character_mut(1).stats.insert(4, 100);
        r.character_mut(1).resources_carry = Some(ResourceCarry::ZERO);
        let input = fire(&r, 1, 1);
        if elapsed < 0.5 {
            reject(&c, &r, input);
        } else {
            r.handle(1, input, &c).unwrap();
            assert_eq!(r.character(1).stats[&4], 20);
        }
    }
    let mut damage = None;
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
        assert_eq!(r.character(1).stats[&25], 0);
        assert_eq!(
            *damage.get_or_insert(r.bullets[0].damage),
            r.bullets[0].damage
        );
    }
    for condition in [
        Condition::Quiet,
        Condition::Paused,
        Condition::Stunned,
        Condition::Weak,
        Condition::Damaging,
    ] {
        let mut r = room.clone();
        r.character_mut(1)
            .stats
            .insert(25, combat::ConditionMask::bit(condition) as i32);
        reject(&c, &r, fire(&r, 1, 1));
    }
}

#[test]
fn shield_runtime_native_geometry_ids_wrapping_pending_weapons_and_receipts() {
    let (mut c, mut room) = setup(false);
    knight(&mut room, &c, 1);
    let (x, y) = (room.character(1).x, room.character(1).y);
    for id in [0, 97, 255] {
        reject(&c, &room, native(1, id, 0xa08, (x, y), 0., 0));
    }
    reject(&c, &room, native(2, 1, 0xa08, (x, y), 0., 0));
    for origin in [
        (x + 2., y),
        (x - 2., y),
        (-0.1, y),
        (256., y),
        (x, f32::NAN),
        (f32::INFINITY, y),
    ] {
        reject(&c, &room, native(1, 1, 0xa08, origin, 0., 0));
    }
    for angle in [
        f32::NAN,
        f32::INFINITY,
        std::f32::consts::PI.next_up(),
        -std::f32::consts::PI.next_up(),
    ] {
        reject(&c, &room, native(1, 1, 0xa08, (x, y), angle, 0));
    }
    for angle in [std::f32::consts::PI, -std::f32::consts::PI, 0.37] {
        let mut a = room.clone();
        let mut b = room.clone();
        a.handle(1, native(1, 1, 0xa08, (x + 1.999, y), angle, i16::MIN), &c)
            .unwrap();
        b.handle(1, native(1, 1, 0xa08, (x + 1.999, y), angle, i16::MAX), &c)
            .unwrap();
        assert_eq!(snapshot(&a), snapshot(&b));
    }
    let input = fire(&room, 1, 1);
    assert_eq!(input.body.len(), 22);
    for len in 0..22 {
        reject(
            &c,
            &room,
            Packet {
                id: 8,
                body: input.body[..len].to_vec(),
            },
        );
    }
    let mut extra = fire(&room, 1, 1);
    extra.body.push(0);
    reject(&c, &room, extra);
    for visual in [0, 10, 255] {
        let mut input = fire(&room, 1, 1);
        input.body[5] = visual;
        reject(&c, &room, input);
    }
    reject(&c, &room, cast(1, 1, 1, (0., 0.)));
    let input = bow(&room, &c, 95);
    room.handle(1, input, &c).unwrap();
    reject(&c, &room, fire(&room, 1, 95));
    let input = fire(&room, 1, 96);
    room.handle(1, input, &c).unwrap();
    reject(&c, &room, bow(&room, &c, 96));
    reject(&c, &room, fire(&room, 1, 1));
    for input in [
        cast(2, 1, 1, (0., 0.)),
        cast(1, 0, 1, (0., 0.)),
        cast(1, 1, 2, (0., 0.)),
        {
            let mut p = cast(1, 1, 1, (0., 0.));
            p.body[9..13].copy_from_slice(&f32::NAN.to_be_bytes());
            p
        },
    ] {
        reject(&c, &room, input);
    }
    let ack = cast(1, 1, 1, (0., 0.));
    for len in 0..17 {
        reject(
            &c,
            &room,
            Packet {
                id: 15,
                body: ack.body[..len].to_vec(),
            },
        );
    }
    let mut extra = cast(1, 1, 1, (0., 0.));
    extra.body.push(0);
    reject(&c, &room, extra);
    let mut wrong = room.clone();
    wrong.character_mut(1).stats.insert(9, 0xa09);
    reject(&c, &wrong, cast(1, 1, 1, (0., 0.)));
    room.handle(1, ack, &c).unwrap();
    reject(&c, &room, cast(1, 1, 1, (0., 0.)));
    let input = bow(&room, &c, 1);
    room.handle(1, input, &c).unwrap();
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
    let input = fire(&room, 1, 1);
    room.handle(1, input, &c).unwrap();
    assert_eq!(room.bullets[0].id, 1);
    room.players.get_mut(&1).unwrap().shoot_ability_ack = None;
    room.bullets.clear();
    let kind = room.character(1).stats[&8] as i16;
    c.objects.get_mut(&kind).unwrap().num_projectiles = 2;
    let input = bow(&room, &c, 10);
    room.handle(1, input, &c).unwrap();
    assert_eq!(
        room.players[&1]
            .weapon
            .pending
            .as_ref()
            .unwrap()
            .shots
            .len(),
        1
    );
    reject(&c, &room, fire(&room, 1, 11));
}

#[test]
fn shield_runtime_wooden_native_paid_shoot_then_nonpaying_ack() {
    for repaired in [false, true] {
        let (c, mut room) = setup(repaired);
        let mut p = Character::new(&c, 0x31e, "Knight".into(), 1).unwrap();
        (p.x, p.y) = room.landing();
        room.attach(1, p, &c).unwrap();
        let p = room.character(1);
        let input = packet(8, |w| {
            w.u8(96).i32(1).u8(27).i16(0xa08);
            for value in [p.x + 0.3, p.y, 0.] {
                w.0.extend(value.to_be_bytes());
            }
            w.i16(32767);
        });
        let mut rng = room.rng.clone();
        let damage = (30. + 40. * rng.gen_range(0.0..1.0)) as i32;
        let events = room
            .handle(1, input, &c)
            .expect("Wooden Shield paid Shoot8");
        assert_eq!(events.iter().filter(|p| p.id == 8).count(), 1);
        assert_eq!(room.character(1).stats[&4], 20);
        assert_eq!(room.character(1).stats[&25], 0);
        assert_eq!(room.bullets.len(), 1);
        assert_eq!((room.bullets[0].id, room.bullets[0].damage), (96, damage));
        assert_eq!(room.rng.clone().next_u64(), rng.next_u64());
        let paid = room.character(1).clone();
        let after8 = room.rng.clone().next_u64();
        room.handle(1, cast(1, 1, 1, (0.5, 0.5)), &c).unwrap();
        assert_eq!(room.character(1), &paid);
        assert_eq!(room.rng.clone().next_u64(), after8);
        assert_eq!(room.bullets.len(), 1);
        assert!(room.handle(1, cast(1, 1, 1, (0.5, 0.5)), &c).is_err());
    }
}

fn pirate(room: &mut Room, c: &Content) -> i32 {
    let (x, y) = (room.character(1).x, room.character(1).y);
    let id = target(room, c, (x + 2.5, y), 1000);
    room.shore = Some(ShorePopulation {
        sites: vec![],
        next_replenish: 100.,
        actors: BTreeMap::from([(
            id,
            ShorePirate {
                home: (x + 4., y),
                next_shot: 0.,
            },
        )]),
    });
    id
}
fn npc_shots(events: &[Packet], owner: i32) -> usize {
    events
        .iter()
        .filter(|p| p.id == 8)
        .filter(|p| {
            let mut r = Reader::new(&p.body);
            r.u8().unwrap();
            r.i32().unwrap() == owner
        })
        .count()
}

#[test]
fn shield_runtime_stun_whole_entry_zero_coarse_equality_keeps_pirate_movement_and_readiness() {
    let (c, mut base) = setup(false);
    knight(&mut base, &c, 1);
    let id = pirate(&mut base, &c);
    for returning in [false, true] {
        for (remaining, dt) in [(0., 0.1), (0.025, 0.1), (0.1, 0.1), (3., 4.), (3., 0.)] {
            let mut normal = base.clone();
            if returning {
                normal.character_mut(1).x += 40.;
            }
            let mut held = normal.clone();
            held.enemy_stuns.insert(id, remaining);
            let ordinary = normal.advance(dt, &c).unwrap();
            let events = held.advance(dt, &c).unwrap();
            let a = &normal.entities[&id];
            let b = &held.entities[&id];
            assert_eq!((a.x, a.y, a.age), (b.x, b.y, b.age));
            assert_eq!(
                (normal.elapsed, normal.shot_clock, normal.next_tick),
                (held.elapsed, held.shot_clock, held.next_tick)
            );
            assert_eq!(
                normal.shore.as_ref().unwrap().actors[&id].next_shot,
                held.shore.as_ref().unwrap().actors[&id].next_shot
            );
            assert_eq!(
                normal.shore.as_ref().unwrap().next_replenish,
                held.shore.as_ref().unwrap().next_replenish
            );
            assert_eq!(
                npc_shots(&events, id),
                if remaining > 0. {
                    0
                } else {
                    npc_shots(&ordinary, id)
                }
            );
            if !returning {
                assert_eq!(npc_shots(&ordinary, id), 1);
            }
            assert_eq!(
                held.enemy_stuns.get(&id).copied(),
                (remaining > dt as f32).then_some((remaining - dt as f32).max(0.))
            );
        }
    }
}

#[test]
fn shield_runtime_actual_pirate_and_turret_fire_suppression_clocks_existing_bullets_and_resume() {
    for turret in [false, true] {
        let (c, mut room) = setup(false);
        knight(&mut room, &c, 1);
        let id = pirate(&mut room, &c);
        if turret {
            room.shore = None;
            let e = room.entities.get_mut(&id).unwrap();
            e.kind = 0x802;
            e.stats = c.get(0x802).unwrap().stats.clone();
            assert!(!e.stats.contains_key(&1));
            e.age = 3.;
        }
        assert_eq!(npc_shots(&room.advance(0., &c).unwrap(), id), 1);
        let bullet = room.bullets.iter().find(|b| b.owner == id).unwrap().clone();
        room.enemy_stuns.insert(id, 0.125);
        if turret {
            room.entities.get_mut(&id).unwrap().age = 3.;
        } else {
            room.shore
                .as_mut()
                .unwrap()
                .actors
                .get_mut(&id)
                .unwrap()
                .next_shot = 0.;
        }
        let events = room.advance(0.125, &c).unwrap();
        assert_eq!(npc_shots(&events, id), 0);
        let advanced = room
            .bullets
            .iter()
            .find(|b| b.owner == id && b.id == bullet.id)
            .unwrap();
        assert!(advanced.age > bullet.age);
        assert_ne!((advanced.x, advanced.y), (bullet.x, bullet.y));
        assert!(!room.enemy_stuns.contains_key(&id));
        if !turret {
            assert_eq!(
                room.entity_wire_stats(&room.entities[&id], &c).unwrap()[&29],
                0
            );
        }
        assert_eq!(
            npc_shots(&room.advance(0.1, &c).unwrap(), id),
            0,
            "no catch-up shot after expiry"
        );
        let events = room.advance(if turret { 2.9 } else { 2.5 }, &c).unwrap();
        assert_eq!(npc_shots(&events, id), 1);
    }
}

#[test]
fn shield_runtime_contact_after_ai_next_advance_overwrites_and_independent_timer_unions_clear() {
    for previous in [0.125, 10.] {
        let (c, mut room) = setup(false);
        knight(&mut room, &c, 1);
        let id = pirate(&mut room, &c);
        let x = room.character(1).x;
        room.entities.get_mut(&id).unwrap().x = x + 1.5;
        room.enemy_stuns.insert(id, previous);
        room.enemy_slows.insert(id, 10.);
        room.enemy_paralysis.insert(id, 10.);
        let input = fire(&room, 1, 96);
        room.handle(1, input, &c).unwrap();
        let events = room.advance(0.05, &c).unwrap();
        assert_eq!(hits(&events), [id]);
        assert_eq!(room.enemy_stuns[&id], 3.);
        assert_eq!(room.enemy_slows[&id], 9.95);
        assert_eq!(room.enemy_paralysis[&id], 9.95);
        assert_eq!(
            room.entity_wire_stats(&room.entities[&id], &c).unwrap()[&29],
            8264
        );
        assert_eq!(room.character(1).stats[&25], 0);
        room.bullets.clear();
        for (slow, paralyzed, mask) in [
            (true, true, 8264),
            (true, false, 72),
            (false, true, 8256),
            (false, false, 64),
        ] {
            let mut r = room.clone();
            if !slow {
                r.enemy_slows.clear();
            }
            if !paralyzed {
                r.enemy_paralysis.clear();
            }
            assert_eq!(
                r.entity_wire_stats(&r.entities[&id], &c).unwrap()[&29],
                mask
            );
        }
        room.enemy_slows.insert(id, 0.025);
        room.enemy_paralysis.insert(id, 0.125);
        room.advance(0.025, &c).unwrap();
        assert!(!room.enemy_slows.contains_key(&id));
        assert_eq!(
            room.entity_wire_stats(&room.entities[&id], &c).unwrap()[&29],
            8256
        );
        room.advance(0.125, &c).unwrap();
        assert!(!room.enemy_paralysis.contains_key(&id));
        assert_eq!(
            room.entity_wire_stats(&room.entities[&id], &c).unwrap()[&29],
            64
        );
        room.enemy_stuns.insert(id, 0.125);
        room.advance(0.125, &c).unwrap();
        assert!(!room.enemy_stuns.contains_key(&id));
        let events = room.advance(0.2, &c).unwrap();
        let out = room.publication(1, &events, &c).unwrap();
        assert_eq!(stats(&out, 13, id)[&29], 0);
    }
    let (c, mut room) = setup(false);
    knight(&mut room, &c, 1);
    let id = pirate(&mut room, &c);
    room.entities.get_mut(&id).unwrap().x = room.character(1).x + 1.5;
    let input = fire(&room, 1, 96);
    room.handle(1, input, &c).unwrap();
    let events = room.advance(0.05, &c).unwrap();
    assert_eq!(hits(&events), [id]);
    assert_eq!(
        npc_shots(&events, id),
        1,
        "new contact occurs after AI firing"
    );
    room.shore
        .as_mut()
        .unwrap()
        .actors
        .get_mut(&id)
        .unwrap()
        .next_shot = 0.;
    let mut normal = room.clone();
    normal.enemy_stuns.clear();
    let events = room.advance(0.05, &c).unwrap();
    let ordinary = normal.advance(0.05, &c).unwrap();
    assert_eq!(npc_shots(&events, id), 0);
    assert_eq!(npc_shots(&ordinary, id), 1);
    assert_eq!(
        (room.entities[&id].x, room.entities[&id].y),
        (normal.entities[&id].x, normal.entities[&id].y)
    );
}

pub(crate) fn contact_fixture(room: &mut Room, c: &Content, owner: i32, hp: i32) -> [i32; 2] {
    room.shore = None;
    room.entities.clear();
    let (x, y) = (room.character(owner).x, room.character(owner).y);
    [
        target(room, c, (x + 1., y), hp),
        target(room, c, (x + 2.5, y), 1000),
    ]
}
pub(crate) fn contact_state(
    room: &Room,
    targets: [i32; 2],
) -> ([i32; 2], [Option<f32>; 2], BTreeSet<i32>) {
    (
        targets.map(|id| room.entities[&id].stats[&1]),
        targets.map(|id| room.enemy_stuns.get(&id).copied()),
        room.bullets[0].victims.clone(),
    )
}

#[test]
fn shield_runtime_continued_contacts_repair_reentry_tombstones_then_new_paid_projectile() {
    let (c, mut room) = setup(false);
    knight(&mut room, &c, 1);
    knight(&mut room, &c, 2);
    let targets = contact_fixture(&mut room, &c, 1, 1000);
    for id in [1, 2] {
        room.publication(id, &[], &c).unwrap();
    }
    let input = fire(&room, 1, 96);
    room.handle(1, input, &c).unwrap();
    room.handle(1, cast(1, 1, 1, (0., 0.)), &c).unwrap();
    let damage = room.bullets[0].damage;
    let events = room.advance(0.05, &c).unwrap();
    assert_eq!(hits(&events), [targets[0]]);
    let mut wire = Reader::new(&events.iter().find(|p| p.id == 9).unwrap().body);
    assert_eq!(
        (
            wire.i32().unwrap(),
            wire.u8().unwrap(),
            wire.u8().unwrap(),
            wire.i16().unwrap(),
            wire.u8().unwrap(),
            wire.i32().unwrap()
        ),
        (targets[0], 7, 3, damage as i16, 0, 1)
    );
    wire.done().unwrap();
    assert_eq!(room.bullets[0].id, 96);
    for condition in [1, 7, 1] {
        let before = room.clone();
        let events = room
            .handle(
                1,
                prediction_runtime_tests::report(targets[0], 1, 96, condition),
                &c,
            )
            .unwrap();
        let out = room.publication(1, &events, &c).unwrap();
        if condition == 1 {
            assert_eq!(
                prediction_runtime_tests::target_packets(&out, targets[0]),
                [11, 10]
            );
            assert_eq!(stats(&out, 10, targets[0])[&29], 64);
        }
        assert!(room.publication(2, &events, &c).unwrap().is_empty());
        prediction_runtime_tests::authority_unchanged(&before, &room);
    }
    assert!(hits(&room.advance(0., &c).unwrap()).is_empty());
    room.entities.get_mut(&targets[0]).unwrap().x = room.bullets[0].x;
    assert!(hits(&room.advance(0., &c).unwrap()).is_empty());
    assert_eq!(room.entities[&targets[0]].stats[&1], 1000 - damage);
    assert_eq!(hits(&room.advance(0.05, &c).unwrap()), [targets[1]]);
    assert_eq!(room.bullets[0].victims, BTreeSet::from(targets));
    room.character_mut(1).x += 40.;
    room.publication(1, &[], &c).unwrap();
    room.character_mut(1).x -= 40.;
    room.publication(1, &[], &c).unwrap();
    assert_eq!(room.bullets[0].victims, BTreeSet::from(targets));
    room.entities.remove(&targets[0]);
    let position = (room.bullets[0].x, room.bullets[0].y);
    let replacement = target(&mut room, &c, position, 1000);
    assert_ne!(replacement, targets[0]);
    assert_eq!(hits(&room.advance(0., &c).unwrap()), [replacement]);
    assert!(!room.enemy_stuns.contains_key(&targets[0]));
    assert_eq!(
        room.bullets[0].victims,
        BTreeSet::from([targets[0], targets[1], replacement])
    );
    room.advance(0.5, &c).unwrap();
    room.character_mut(1).stats.insert(4, 100);
    room.character_mut(1).resources_carry = Some(ResourceCarry::ZERO);
    let input = fire(&room, 1, 1);
    room.handle(1, input, &c).unwrap();
    assert!(room.bullets[0].victims.is_empty());
    assert_eq!(
        hits(&room.advance(0.1, &c).unwrap()),
        [replacement, targets[1]]
    );
}

#[test]
fn shield_runtime_continued_cover_sorted_entry_ties_buffered_kill_reservations_once_rewards() {
    let (mut c, mut room) = setup(false);
    progression_runtime_tests::guaranteed(&mut c, 1);
    for owner in [1, 2] {
        knight(&mut room, &c, owner);
    }
    let targets = contact_fixture(&mut room, &c, 1, 5);
    let (x, y) = (room.character(1).x, room.character(1).y);
    let far = target(&mut room, &c, (x + 2., y), 1000);
    let near = target(&mut room, &c, (x + 1.5, y), 1000);
    let tie = target(&mut room, &c, (x + 1.5, y), 1000);
    let cover = target(&mut room, &c, (x + 0.5, y), 1000);
    room.entities.get_mut(&cover).unwrap().kind = c
        .objects
        .values()
        .find(|o| o.is_static && o.occupy_square)
        .unwrap()
        .id;
    for owner in [1, 2] {
        let input = fire(&room, owner, 96);
        room.handle(owner, input, &c).unwrap();
    }
    let mut once = room.clone();
    once.bullets.pop();
    once.advance(0.15, &c).unwrap();
    let total: i32 = room.bullets.iter().map(|b| b.damage).sum();
    let events = room.advance(0.15, &c).unwrap();
    assert_eq!(
        hits(&events),
        [
            targets[0], near, tie, far, targets[1], near, tie, far, targets[1]
        ]
    );
    for b in &room.bullets {
        assert_eq!(
            b.victims,
            BTreeSet::from([targets[0], targets[1], far, near, tie])
        );
    }
    assert!(!room.entities.contains_key(&targets[0]));
    assert!(!room.enemy_stuns.contains_key(&targets[0]));
    assert_eq!(room.entities[&targets[1]].stats[&1], 1000 - total);
    for owner in [1, 2] {
        assert_eq!(
            room.character(owner).stats[&6],
            once.character(owner).stats[&6]
        );
    }
    assert_eq!(room.rng.clone().next_u64(), once.rng.clone().next_u64());
    assert_eq!(room.next_id, once.next_id);
    assert_eq!(
        progression_runtime_tests::bags(&room),
        progression_runtime_tests::bags(&once)
    );
    assert!(!progression_runtime_tests::bags(&room).is_empty());
    assert!(hits(&room.advance(0., &c).unwrap()).is_empty());
}

#[test]
fn shield_runtime_200ms_terminal_and_delayed_advances_retire_without_final_sweep() {
    let (c, mut base) = setup(false);
    knight(&mut base, &c, 1);
    for (first, second, hit) in [
        (0., 0.2, false),
        (0., 0.3, false),
        (0.1, 0.1, false),
        (0.1, 0.099, true),
    ] {
        let mut room = base.clone();
        room.shore = None;
        let (x, y) = (room.character(1).x, room.character(1).y);
        let victim = target(&mut room, &c, (x + 3., y), 1000);
        let input = fire(&room, 1, 1);
        room.handle(1, input, &c).unwrap();
        assert!(hits(&room.advance(first, &c).unwrap()).is_empty());
        assert_eq!(
            hits(&room.advance(second, &c).unwrap()).contains(&victim),
            hit
        );
        assert_eq!(room.bullets.is_empty(), !hit);
        assert_eq!(room.enemy_stuns.contains_key(&victim), hit);
    }
}

#[test]
fn shield_runtime_removal_replenishment_new_ids_and_paid_marker_success_boundaries() {
    let (c, mut base) = setup(false);
    knight(&mut base, &c, 1);
    knight(&mut base, &c, 2);
    let input = fire(&base, 1, 1);
    base.handle(1, input, &c).unwrap();
    for operation in [
        packet(14, |w| {
            w.i32(1).u8(1).i16(0xa08).i32(1).u8(5).i16(-1);
        }),
        packet(19, |w| {
            w.i32(1).u8(1).i16(0xa08);
        }),
        packet(28, |w| {
            w.i32(2);
        }),
    ] {
        let mut room = base.clone();
        room.handle(1, operation, &c).unwrap();
        assert_eq!(room.players[&1].shoot_ability_ack, None);
        reject(&c, &room, cast(1, 1, 1, (0., 0.)));
    }
    let paid = base.character(1).clone();
    base.detach(1);
    base.attach(1, paid.clone(), &c).unwrap();
    assert_eq!(base.character(1), &paid);
    assert_eq!(base.players[&1].shoot_ability_ack, None);
    reject(&c, &base, cast(1, 1, 1, (0., 0.)));
    let mut room = Room::from_map(base.map.unwrap(), &c, 1_000_000).unwrap();
    knight(&mut room, &c, 1);
    let old: Vec<_> = room
        .shore
        .as_ref()
        .unwrap()
        .actors
        .keys()
        .copied()
        .collect();
    for id in &old {
        room.enemy_stuns.insert(*id, 30.);
        room.entities.remove(id);
    }
    room.advance(10., &c).unwrap();
    assert!(room.enemy_stuns.is_empty());
    assert_eq!(room.shore.as_ref().unwrap().actors.len(), 4);
    for id in room.shore.as_ref().unwrap().actors.keys() {
        assert!(!old.contains(id));
        assert_eq!(
            room.entity_wire_stats(&room.entities[id], &c).unwrap()[&29],
            0
        );
    }
}

pub(crate) fn assert_reward_contact(room: &Room, targets: [i32; 2]) {
    assert!(!room.entities.contains_key(&targets[0]));
    assert!(!room.enemy_stuns.contains_key(&targets[0]));
    assert!(room.entities[&targets[1]].stats[&1] < 1000);
    assert_eq!(room.enemy_stuns[&targets[1]], 3.);
    assert_eq!(room.bullets.len(), 1);
    assert_eq!(room.bullets[0].victims, BTreeSet::from(targets));
}

#[test]
fn shield_runtime_selected_original_repaired_artifact_scopes_order_and_unchanged_tuples() {
    let mut selected = vec![];
    for variable in [
        "BUILD100_CONTENT_ORIGINAL_DIR",
        "BUILD100_CONTENT_REPAIRED_DIR",
    ] {
        let path =
            std::path::PathBuf::from(std::env::var_os(variable).unwrap()).join("objects.xml");
        let xml = std::fs::read_to_string(path).unwrap();
        let doc = roxmltree::Document::parse(&xml).unwrap();
        for id in ["Wooden Shield", "Knight", "Silver Shield"] {
            let mut found = doc
                .descendants()
                .filter(|n| n.has_tag_name("Object") && n.attribute("id") == Some(id));
            let node = found.next().unwrap();
            assert!(found.next().is_none());
            if id == "Wooden Shield" {
                assert_eq!(
                    node.children()
                        .filter(|n| n.is_element())
                        .map(|n| n.tag_name().name())
                        .collect::<Vec<_>>(),
                    [
                        "Class",
                        "Item",
                        "Texture",
                        "SlotType",
                        "Description",
                        "StatDescription",
                        "MinDamage",
                        "MaxDamage",
                        "Sound",
                        "Projectile",
                        "ActivateOnEquip",
                        "ConditionEffect",
                        "Usable",
                        "MpCost",
                        "Activate",
                        "NumProjectiles"
                    ]
                );
                assert!(node.children().all(|n| !n.has_tag_name("ArcGap")));
                let projectile = node
                    .children()
                    .find(|n| n.has_tag_name("Projectile"))
                    .unwrap();
                assert_eq!(projectile.attributes().len(), 0);
                assert_eq!(
                    projectile
                        .children()
                        .filter(|n| n.is_element())
                        .map(|n| n.tag_name().name())
                        .collect::<Vec<_>>(),
                    [
                        "ObjectId",
                        "Speed",
                        "MinDamage",
                        "MaxDamage",
                        "LifetimeMS",
                        "ConditionEffect",
                        "Continues"
                    ]
                );
                let continues = projectile
                    .children()
                    .find(|n| n.has_tag_name("Continues"))
                    .unwrap();
                assert_eq!(continues.attributes().len(), 0);
                assert!(continues.children().next().is_none());
                let shoot = node
                    .children()
                    .find(|n| n.has_tag_name("Activate"))
                    .unwrap();
                assert_eq!((shoot.text(), shoot.attributes().len()), (Some("Shoot"), 0));
            }
            selected.push(xml[node.range()].to_owned());
        }
    }
    assert_eq!(selected[..3], selected[3..]);
}

#[test]
fn shield_runtime_capacity_and_projectile_ground_death_clear_paid_receipt() {
    let (c, mut base) = setup(false);
    knight(&mut base, &c, 1);
    let input = fire(&base, 1, 96);
    base.handle(1, input, &c).unwrap();
    let bullet = base.bullets[0].clone();
    for (total, own, accepted) in [
        (4095, 0, true),
        (4096, 0, false),
        (255, 255, true),
        (256, 256, false),
    ] {
        let mut room = base.clone();
        room.players.get_mut(&1).unwrap().shoot_ability_ack = None;
        *room.character_mut(1) = Character::new(&c, 0x31e, "Knight".into(), 1).unwrap();
        (room.character_mut(1).x, room.character_mut(1).y) = room.landing();
        room.bullets = (0..total)
            .map(|i| Bullet {
                owner: if i < own { 1 } else { 2 },
                id: 97,
                ..bullet.clone()
            })
            .collect();
        let input = fire(&room, 1, 96);
        if accepted {
            room.handle(1, input, &c).unwrap();
            assert_eq!(room.bullets.len(), total + 1);
            assert_eq!(room.character(1).stats[&4], 20);
        } else {
            reject(&c, &room, input);
        }
    }
    for ground in [false, true] {
        let mut room = base.clone();
        room.shore = None;
        let (x, y) = (room.character(1).x, room.character(1).y);
        if ground {
            let map = room.map.as_ref().unwrap();
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
            let p = room.character_mut(1);
            (p.x, p.y) = (hazard.0 as f32 + 0.5, hazard.1 as f32 + 0.5);
            p.stats.insert(1, 1);
        } else {
            let id = target(&mut room, &c, (x, y), 5);
            room.bullets.push(Bullet {
                owner: id,
                id: 1,
                flight: tests::normal_flight(1., 1.),
                damage: 1000,
                vx: 0.,
                vy: 0.,
                x,
                y,
                origin: (x, y),
                victims: BTreeSet::new(),
                age: 0.,
                life: 1.,
                amplitude: 0.,
                frequency: 1.,
            });
        }
        room.advance(if ground { 0.5 } else { 0. }, &c).unwrap();
        assert!(!room.character(1).alive);
        assert_eq!(room.players[&1].shoot_ability_ack, None);
    }
}
