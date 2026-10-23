use super::*;
use crate::{
    content::{Condition, ConditionEffect, Grounds},
    realm_map,
};
use rand::RngCore;
use std::{path::PathBuf, time::Duration};

pub fn setup(repaired: bool) -> (Content, Room) {
    let path = PathBuf::from(
        std::env::var_os(if repaired {
            "BUILD100_CONTENT_REPAIRED_DIR"
        } else {
            "BUILD100_CONTENT_ORIGINAL_DIR"
        })
        .expect("qualified content"),
    );
    let c = Content::load(&path).unwrap();
    let g = Arc::new(Grounds::load(&path).unwrap());
    let map = realm_map::realm(100, &c, g).unwrap();
    let mut room = Room::from_map(Arc::new(map), &c, 1_000_000).unwrap();
    room.entities.clear();
    (c, room)
}
pub fn attach(room: &mut Room, c: &Content, id: i32) {
    let mut p = Character::new(c, 0x30e, format!("Spell{id}"), 3).unwrap();
    (p.x, p.y) = room.landing();
    room.attach(id, p, c).unwrap();
}
pub fn cast(owner: i32, slot: u8, target: i32, cursor: (f32, f32)) -> Packet {
    packet(15, |w| {
        w.i32(owner)
            .u8(slot)
            .i32(target)
            .f32(cursor.0)
            .f32(cursor.1);
    })
}
pub fn aim(room: &Room, id: i32) -> (f32, f32) {
    let p = room.character(id);
    (p.x + 2., p.y)
}
fn fire(c: &Content, room: &Room, id: u8) -> Packet {
    let p = room.character(1);
    let item = c.get(p.stats[&8] as i16).unwrap();
    packet(8, |w| {
        w.u8(id)
            .i32(1)
            .u8(c
                .named(&item.normal_projectile(0).unwrap().visual)
                .unwrap()
                .id as u8)
            .i16(item.id)
            .f32(p.x)
            .f32(p.y)
            .f32(0.)
            .i16(32767);
    })
}
pub fn snapshot(room: &Room) -> (Vec<(i32, u8, i32)>, u64) {
    (
        room.bullets
            .iter()
            .map(|b| (b.owner, b.id, b.damage))
            .collect(),
        room.rng.clone().next_u64(),
    )
}
fn carry(p: &Character) -> [u64; 4] {
    let c = p.resources_carry.unwrap();
    [
        c.hp_remainder.to_bits(),
        c.mp_remainder.to_bits(),
        c.hp_roundoff.to_bits(),
        c.mp_roundoff.to_bits(),
    ]
}
fn reject(c: &Content, room: &Room, input: Packet) -> String {
    let mut candidate = room.clone();
    let before = candidate.character(1).clone();
    let error = candidate.handle(1, input, c).err().expect("must reject");
    assert_eq!(candidate.character(1), &before);
    assert_eq!(snapshot(&candidate), snapshot(room));
    assert_eq!(
        candidate.players[&1]
            .weapon
            .pending
            .as_ref()
            .map(|b| &b.shots),
        room.players[&1].weapon.pending.as_ref().map(|b| &b.shots)
    );
    format!("{error:#}")
}

#[test]
fn spell_runtime_original_repaired_nested_unscaled_root_cost_cursor_rays_and_carry() {
    for (repaired, cost) in [(false, 0), (true, 40)] {
        let (c, mut room) = setup(repaired);
        attach(&mut room, &c, 1);
        let p = room.character_mut(1);
        p.stats.extend([(20, 75), (1, 40)]);
        p.resources_carry = Some(ResourceCarry {
            hp_remainder: 0.25,
            mp_remainder: 0.,
            hp_roundoff: 1e-20,
            mp_roundoff: 0.,
        });
        let before = p.clone();
        let cursor = aim(&room, 1);
        let mut expected_rng = room.rng.clone();
        let events = room.handle(1, cast(1, 1, 1, cursor), &c).unwrap();
        assert_eq!(room.character(1).stats[&4], 100 - cost);
        assert_eq!(room.character(1).stats[&1], 40);
        assert_eq!(carry(room.character(1)), carry(&before));
        assert_eq!(
            room.character(1)
                .action_state
                .as_ref()
                .unwrap()
                .ability_used_seconds,
            Some(0.)
        );
        let projectiles: Vec<_> = events.iter().filter(|p| p.id == 8).collect();
        assert_eq!(projectiles.len(), 20);
        for (index, event) in projectiles.into_iter().enumerate() {
            let mut r = Reader::new(&event.body);
            assert_eq!(r.u8().unwrap(), 97 + index as u8);
            assert_eq!(r.i32().unwrap(), 1);
            assert_eq!(r.u8().unwrap(), 0);
            assert_eq!(r.i16().unwrap(), 0xa2e);
            assert_eq!((r.f32().unwrap(), r.f32().unwrap()), cursor);
            assert_eq!(
                r.f32().unwrap(),
                (index as f64 * std::f64::consts::TAU / 20.) as f32
            );
            let u: f64 = expected_rng.gen_range(0.0..1.0);
            assert_eq!(r.i16().unwrap(), (30. + 20. * u) as i16);
            r.done().unwrap();
        }
        assert_eq!(room.rng.clone().next_u64(), expected_rng.next_u64());
        assert!(reject(&c, &room, cast(1, 1, 1, cursor)).contains("cooldown"));
        let below = 0.5_f64.next_down();
        room.advance(below, &c).unwrap();
        assert!(reject(&c, &room, cast(1, 1, 1, cursor)).contains("cooldown"));
        room.advance(0.5 - below, &c).unwrap();
        let mp = room.character(1).stats[&4];
        room.handle(1, cast(1, 1, 1, cursor), &c).unwrap();
        assert_eq!(room.character(1).stats[&4], mp - cost);
        assert_eq!(room.bullets.len(), 40);
        assert!(room.bullets.iter().any(|b| b.id >= 128));
        let saved: Character =
            serde_json::from_str(&serde_json::to_string(room.character(1)).unwrap()).unwrap();
        let mut reconnected = setup(repaired).1;
        reconnected.attach(2, saved.clone(), &c).unwrap();
        assert_eq!(reconnected.character(2), &saved);
        assert!(
            reconnected
                .handle(2, cast(2, 1, 2, cursor), &c)
                .err()
                .expect("rejected")
                .to_string()
                .contains("cooldown")
        );
    }
}

#[test]
fn spell_runtime_native96_wrap_reserved_domain_and_held_staff_coexists_with_nova() {
    let (c, mut room) = setup(true);
    attach(&mut room, &c, 1);
    let base = Instant::now();
    for index in 0..55_u32 {
        let now = base + Duration::from_millis(u64::from(index) * 360);
        let first = (index * 2 % 96 + 1) as u8;
        for id in [first, first % 96 + 1] {
            let input = fire(&c, &room, id);
            let events = room.handle_at(1, input, &c, now, now).unwrap();
            if id == first {
                assert!(events.is_empty());
            } else {
                assert_eq!(
                    events.iter().map(|p| p.body[0]).collect::<Vec<_>>(),
                    [first, first % 96 + 1]
                );
            }
        }
        room.advance(0.36, &c).unwrap();
    }
    let now = base + Duration::from_secs(30);
    for id in [0, 97, 128, 255] {
        let p = fire(&c, &room, id);
        assert!(reject(&c, &room, p).contains("1..=96"));
    }
    room.bullets.clear();
    let p = fire(&c, &room, 96);
    assert!(room.handle_at(1, p, &c, now, now).unwrap().is_empty());
    let p = fire(&c, &room, 2);
    let before = snapshot(&room);
    let mut rejected = room.clone();
    assert!(
        rejected
            .handle_at(1, p, &c, now, now)
            .err()
            .expect("rejected")
            .to_string()
            .contains("modulo96")
    );
    assert_eq!(snapshot(&rejected), before);
    let p = fire(&c, &room, 1);
    room.handle_at(1, p, &c, now, now).unwrap();
    let cursor = aim(&room, 1);
    room.handle(1, cast(1, 1, 1, cursor), &c).unwrap();
    assert_eq!(
        room.bullets.iter().map(|b| b.id).collect::<Vec<_>>(),
        [vec![96, 1], (97..117).collect()].concat()
    );
}

#[test]
fn spell_runtime_preflight_owner_slot_target_cursor_mana_semantics_and_capacity_are_atomic() {
    let (c, mut room) = setup(true);
    attach(&mut room, &c, 1);
    let cursor = aim(&room, 1);
    for p in [
        cast(2, 1, 1, cursor),
        cast(1, 0, 1, cursor),
        cast(1, 1, 2, cursor),
        cast(1, 1, 1, (-1., cursor.1)),
        cast(1, 1, 1, (room.character(1).x + 15., cursor.1)),
    ] {
        reject(&c, &room, p);
    }
    let mut nonfinite = cast(1, 1, 1, cursor);
    nonfinite.body[9..13].copy_from_slice(&f32::NAN.to_be_bytes());
    reject(&c, &room, nonfinite);
    let mut poor = room.clone();
    poor.character_mut(1).stats.insert(4, 39);
    assert!(reject(&c, &poor, cast(1, 1, 1, cursor)).contains("MP"));
    for mutation in 0..11 {
        let mut altered = Content {
            objects: c.objects.clone(),
        };
        let item = altered.objects.get_mut(&0xa2e).unwrap();
        match mutation {
            0 => item.activations.push(item.activations[0].clone()),
            1 => item.projectiles[0].1.continues = true,
            2 => item.projectiles[0].1.effects.push(ConditionEffect {
                condition: Condition::Slowed,
                duration_seconds: 0.,
            }),
            3 => item.projectiles[0].1.features.push("FutureMotion".into()),
            4 => item.projectiles[0].1.speed = f32::NAN,
            5 => item.projectiles[0].1.min = -1,
            6 => item.projectiles[0].0 = 1,
            7 => item.slot_type = Some(9),
            8 => item.mp_cost = None,
            9 => item.effects.push(ConditionEffect {
                condition: Condition::Invisible,
                duration_seconds: 1.,
            }),
            10 => item.projectiles[0].1.lifetime = 0.,
            _ => unreachable!(),
        }
        reject(&altered, &room, cast(1, 1, 1, cursor));
    }
    let mut altered = Content {
        objects: c.objects.clone(),
    };
    altered.objects.get_mut(&0xa2e).unwrap().projectiles[0]
        .1
        .lifetime = 10.;
    altered.objects.get_mut(&0xa2e).unwrap().mp_cost = Some(0);
    for _ in 0..7 {
        room.handle(1, cast(1, 1, 1, cursor), &altered).unwrap();
        room.advance(0.5, &altered).unwrap();
    }
    assert_eq!(room.bullets.len(), 140);
    assert!(reject(&altered, &room, cast(1, 1, 1, cursor)).contains("capacity"));
    let mut last = setup(true).1;
    attach(&mut last, &c, 1);
    let template = room.bullets[0].clone();
    last.bullets = (97..236)
        .map(|id| Bullet {
            id,
            ..template.clone()
        })
        .collect();
    let events = last.handle(1, cast(1, 1, 1, cursor), &c).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|p| p.id == 8)
            .map(|p| p.body[0])
            .collect::<Vec<_>>(),
        (236..=255).collect::<Vec<_>>()
    );
    assert_eq!(last.character(1).stats[&4], 60);
    for global in [false, true] {
        let mut full = setup(true).1;
        attach(&mut full, &c, 1);
        let template = room.bullets[0].clone();
        full.bullets = (0..if global { 4090 } else { 250 })
            .map(|i| Bullet {
                owner: if global { 999 } else { 1 },
                id: (i % 96 + 1) as u8,
                ..template.clone()
            })
            .collect();
        reject(&c, &full, cast(1, 1, 1, cursor));
    }
}

#[test]
fn spell_runtime_source_absolute_sine_wavy_and_slow_nova_expire_before_collision() {
    for wavy in [false, true] {
        let (mut c, mut room) = setup(true);
        attach(&mut room, &c, 1);
        if wavy {
            let p = &mut c.objects.get_mut(&0xa97).unwrap().projectiles[0].1;
            p.wavy = true;
            p.features.push("Wavy".into());
        }
        let now = Instant::now();
        for id in [96, 1] {
            let p = fire(&c, &room, id);
            room.handle_at(1, p, &c, now, now).unwrap();
        }
        let before = room.bullets.clone();
        room.advance(0.125, &c).unwrap();
        for (b, initial) in room.bullets.iter().zip(&before) {
            let Flight::Normal { projectile, angle } = &b.flight else {
                panic!("normal flight")
            };
            let expected = combat::projectile_position(
                projectile,
                (f64::from(initial.origin.0), f64::from(initial.origin.1)),
                *angle,
                b.id,
                0.125,
            )
            .unwrap()
            .unwrap();
            assert_eq!((b.x, b.y), (expected.0 as f32, expected.1 as f32));
        }
        room.advance(0.875, &c).unwrap();
        assert!(room.bullets.is_empty());
    }
    let (c, mut room) = setup(true);
    attach(&mut room, &c, 1);
    let cursor = aim(&room, 1);
    room.handle(1, cast(1, 1, 1, cursor), &c).unwrap();
    assert_eq!(
        c.get(0xa2e).unwrap().normal_projectile(0).unwrap().speed,
        0.3
    );
    room.advance(0.5, &c).unwrap();
    assert_eq!(
        room.bullets[0].x,
        (f64::from(cursor.0) + f64::from(0.3_f32) * 0.5) as f32
    );
    assert_eq!(room.bullets[0].y, cursor.1);
    let id = room.allocate_entity().unwrap();
    let def = c.named("Pirate").unwrap();
    let mut stats = def.stats.clone();
    stats.insert(1, 5);
    room.entities.insert(
        id,
        Entity {
            id,
            kind: def.id,
            x: cursor.0 + 0.3,
            y: cursor.1,
            stats,
            age: 0.,
            provenance: Provenance::Map,
        },
    );
    room.advance(0.5, &c).unwrap();
    assert!(room.bullets.is_empty());
    assert_eq!(room.entities[&id].stats[&1], 5);
    assert_eq!(room.character(1).stats[&6], 0);
}

#[test]
fn spell_runtime_source_armor_pierce_damage_and_overlapping_nova_death_once() {
    for piercing in [false, true] {
        let (mut c, mut room) = setup(true);
        attach(&mut room, &c, 1);
        let cursor = aim(&room, 1);
        if piercing {
            c.objects.get_mut(&0xa2e).unwrap().projectiles[0]
                .1
                .effects
                .push(ConditionEffect {
                    condition: Condition::ArmorPierce,
                    duration_seconds: 0.,
                });
            c.objects.get_mut(&0xa2e).unwrap().projectiles[0]
                .1
                .features
                .push("ConditionEffect".into());
        }
        let id = room.allocate_entity().unwrap();
        let def = c.named("Pirate").unwrap();
        let mut stats = def.stats.clone();
        stats.extend([(0, 10000), (1, 10000), (21, 20)]);
        room.entities.insert(
            id,
            Entity {
                id,
                kind: def.id,
                x: cursor.0,
                y: cursor.1,
                stats,
                age: 0.,
                provenance: Provenance::Map,
            },
        );
        room.handle(1, cast(1, 1, 1, cursor), &c).unwrap();
        let total: i32 = room
            .bullets
            .iter()
            .map(|b| {
                combat::projectile_defended_damage(
                    b.damage,
                    20,
                    c.get(0xa2e).unwrap().normal_projectile(0).unwrap(),
                )
                .unwrap()
            })
            .sum();
        room.advance(0., &c).unwrap();
        assert_eq!(room.entities[&id].stats[&1], 10000 - total);
        assert!(room.bullets.is_empty());
        assert_eq!(room.character(1).stats[&6], 0);
    }
    let (c, mut room) = setup(true);
    attach(&mut room, &c, 1);
    let id = progression_runtime_tests::encounter(&mut room, &c, 1, 0);
    let cursor = (room.entities[&id].x, room.entities[&id].y);
    room.handle(1, cast(1, 1, 1, cursor), &c).unwrap();
    room.advance(0., &c).unwrap();
    assert!(!room.entities.contains_key(&id));
    assert_eq!(room.character(1).stats[&6], 1);
    room.advance(0., &c).unwrap();
    assert_eq!(room.character(1).stats[&6], 1);
}

#[test]
fn spell_runtime_enemy_nested_unscaled_source_and_death_clears_paid_action_not_mana() {
    let (mut c, mut room) = setup(true);
    attach(&mut room, &c, 1);
    let p = room.character(1).clone();
    let gun = c.objects.get_mut(&0x804).unwrap();
    gun.root_damage = Some((5000, 10000));
    gun.projectiles[0].1.min = 30;
    gun.projectiles[0].1.max = 50;
    let def = c.get(0x804).unwrap();
    let id = room.allocate_entity().unwrap();
    room.entities.insert(
        id,
        Entity {
            id,
            kind: def.id,
            x: p.x - 2.,
            y: p.y,
            stats: def.stats.clone(),
            age: 3.,
            provenance: Provenance::Map,
        },
    );
    let mut rng = room.rng.clone();
    let u: f64 = rng.gen_range(0.0..1.0);
    let raw = (30. + 20. * u) as i32;
    room.advance(0.25, &c).unwrap();
    assert_eq!(room.bullets[0].damage, raw);
    assert_eq!(room.rng.clone().next_u64(), rng.next_u64());
    room.advance(0.25, &c).unwrap();
    assert_eq!(room.character(1).stats[&1], 100 - raw);
    let cursor = aim(&room, 1);
    room.handle(1, cast(1, 1, 1, cursor), &c).unwrap();
    let mp = room.character(1).stats[&4];
    let pos = (room.character(1).x, room.character(1).y);
    room.bullets.push(Bullet {
        victims: BTreeSet::new(),
        flight: tests::normal_flight(0.1, 1.),
        owner: id,
        id: 250,
        origin: pos,
        age: 0.,
        amplitude: 0.,
        frequency: 1.,
        x: pos.0,
        y: pos.1,
        vx: 0.,
        vy: 0.,
        damage: 1000,
        life: 1.,
    });
    room.advance(0., &c).unwrap();
    assert!(!room.character(1).alive);
    assert_eq!(room.character(1).action_state, Some(ActionState::default()));
    assert_eq!(room.character(1).stats[&4], mp);
}

#[test]
fn spell_runtime_normal_npc_256_seconds_never_aliases_owner_or_live_shot_and_capacity_precedes_rng()
{
    let (c, mut room) = setup(true);
    attach(&mut room, &c, 1);
    let p = room.character(1).clone();
    let def = c.get(0x804).unwrap();
    let id = room.allocate_entity().unwrap();
    room.entities.insert(
        id,
        Entity {
            id,
            kind: def.id,
            x: p.x - 5.,
            y: p.y,
            stats: def.stats.clone(),
            age: 3.,
            provenance: Provenance::Map,
        },
    );
    room.elapsed = 256.;
    let events = room.advance(0., &c).unwrap();
    assert_eq!(events.iter().find(|p| p.id == 8).unwrap().body[0], 1);
    room.entities.get_mut(&id).unwrap().age = 3.;
    let events = room.advance(0., &c).unwrap();
    assert_eq!(events.iter().find(|p| p.id == 8).unwrap().body[0], 2);
    assert_eq!(
        room.bullets.iter().map(|b| b.id).collect::<Vec<_>>(),
        [1, 2]
    );
    let template = room.bullets[0].clone();
    room.bullets = (1..=255)
        .map(|id| Bullet {
            id,
            ..template.clone()
        })
        .collect();
    room.entities.get_mut(&id).unwrap().age = 3.;
    let before = snapshot(&room);
    let error = room.advance(0., &c).err().expect("must reject");
    assert!(
        error
            .to_string()
            .contains("NPC projectile live ID capacity")
    );
    assert_eq!(snapshot(&room), before);
}

fn id_reuse_shots(events: &[Packet], owner: i32) -> Vec<u8> {
    events
        .iter()
        .filter(|p| p.id == 8)
        .filter_map(|p| {
            let mut r = Reader::new(&p.body);
            let id = r.u8().unwrap();
            (r.i32().unwrap() == owner).then_some(id)
        })
        .collect()
}
fn id_reuse_gun(room: &mut Room, c: &Content, offset: f32) -> i32 {
    let p = room.character(1);
    let (x, y) = (p.x + offset, p.y);
    let def = c.get(0x804).unwrap();
    let id = room.allocate_entity().unwrap();
    room.entities.insert(
        id,
        Entity {
            id,
            kind: def.id,
            x,
            y,
            stats: def.stats.clone(),
            age: 3.,
            provenance: Provenance::Map,
        },
    );
    id
}

#[test]
fn id_reuse_fix_nova_rotates_after_real_expiry_and_collision() {
    for collision in [false, true] {
        let (c, mut room) = setup(true);
        attach(&mut room, &c, 1);
        let cursor = if collision {
            let enemy = progression_runtime_tests::encounter(&mut room, &c, 1, 0);
            (room.entities[&enemy].x, room.entities[&enemy].y)
        } else {
            aim(&room, 1)
        };
        let first = room.handle(1, cast(1, 1, 1, cursor), &c).unwrap();
        assert_eq!(id_reuse_shots(&first, 1), (97..117).collect::<Vec<_>>());
        if collision {
            room.advance(0., &c).unwrap();
            assert_eq!(room.character(1).stats[&6], 1);
            room.advance(0.5, &c).unwrap();
        } else {
            room.advance(1., &c).unwrap();
        }
        assert!(room.bullets.is_empty());
        let control = room.character(1).clone();
        let second = room.handle(1, cast(1, 1, 1, cursor), &c).unwrap();
        assert_eq!(id_reuse_shots(&second, 1), (117..137).collect::<Vec<_>>());
        assert_eq!(room.character(1).stats[&4], control.stats[&4] - 40);
        assert_eq!(carry(room.character(1)), carry(&control));
    }
}

#[test]
fn id_reuse_fix_npc_rotates_after_actual_expiry_instead_of_restarting_at_one() {
    let (c, mut room) = setup(true);
    attach(&mut room, &c, 1);
    let gun = id_reuse_gun(&mut room, &c, 5.);
    for expected in 1..=3 {
        let events = room.advance(3., &c).unwrap();
        assert!(room.bullets.is_empty());
        assert_eq!(id_reuse_shots(&events, gun), [expected]);
    }
}

pub fn server_cursors(room: &Room) -> BTreeMap<i32, u8> {
    room.server_projectile_cursors.clone()
}

#[test]
fn id_reuse_fix_nova_cycles_255_to_97_and_skips_actual_live_ids() {
    for keep_first_live in [false, true] {
        let (mut c, mut room) = setup(false);
        attach(&mut room, &c, 1);
        if keep_first_live {
            c.objects.get_mut(&0xa2e).unwrap().projectiles[0].1.lifetime = 10.;
        }
        let cursor = aim(&room, 1);
        for cast_index in 0..8 {
            let events = room.handle(1, cast(1, 1, 1, cursor), &c).unwrap();
            let mut expected: Vec<_> = (0..20)
                .map(|ray| (97 + (cast_index * 20 + ray) % 159) as u8)
                .collect();
            if keep_first_live && cast_index == 7 {
                expected[19] = 117;
            }
            assert_eq!(id_reuse_shots(&events, 1), expected);
            if keep_first_live && cast_index == 0 {
                c.objects.get_mut(&0xa2e).unwrap().projectiles[0].1.lifetime = 1.;
            }
            if cast_index < 7 {
                room.advance(1., &c).unwrap();
            }
        }
        assert_eq!(
            server_cursors(&room)[&1],
            if keep_first_live { 118 } else { 98 }
        );
        if keep_first_live {
            assert!(room.bullets.iter().any(|b| b.id == 97 && b.age == 7.));
        }
    }
}

#[test]
fn id_reuse_fix_npc_cycles_255_to_one_skips_live_and_is_per_owner() {
    for keep_first_live in [false, true] {
        let (mut c, mut room) = setup(true);
        attach(&mut room, &c, 1);
        let gun = id_reuse_gun(&mut room, &c, 5.);
        if keep_first_live {
            c.objects.get_mut(&0x804).unwrap().projectiles[0].1.lifetime = 1000.;
        }
        for shot_index in 0..256 {
            let events = room
                .advance(
                    if keep_first_live && shot_index == 0 {
                        0.
                    } else {
                        3.
                    },
                    &c,
                )
                .unwrap();
            let expected = if shot_index == 255 && keep_first_live {
                2
            } else {
                (shot_index % 255 + 1) as u8
            };
            assert_eq!(id_reuse_shots(&events, gun), [expected]);
            if keep_first_live && shot_index == 0 {
                c.objects.get_mut(&0x804).unwrap().projectiles[0].1.lifetime = 3.;
            }
        }
        assert_eq!(
            server_cursors(&room)[&gun],
            if keep_first_live { 3 } else { 2 }
        );
    }
    let (c, mut room) = setup(true);
    attach(&mut room, &c, 1);
    let first = id_reuse_gun(&mut room, &c, 5.);
    let second = id_reuse_gun(&mut room, &c, 6.);
    let events = room.advance(3., &c).unwrap();
    assert_eq!(id_reuse_shots(&events, first), [1]);
    assert_eq!(id_reuse_shots(&events, second), [1]);
    room.entities.get_mut(&first).unwrap().age = 3.;
    let events = room.advance(0., &c).unwrap();
    assert_eq!(id_reuse_shots(&events, first), [2]);
    assert!(id_reuse_shots(&events, second).is_empty());
    assert_eq!(
        server_cursors(&room),
        BTreeMap::from([(first, 3), (second, 2)])
    );
    let (c, mut collision) = setup(true);
    attach(&mut collision, &c, 1);
    let gun = id_reuse_gun(&mut collision, &c, -1.);
    for (dt, expected) in [(0.25, 1), (2.75, 2)] {
        let events = collision.advance(dt, &c).unwrap();
        assert_eq!(id_reuse_shots(&events, gun), [expected]);
        assert!(collision.bullets.is_empty());
    }
}

#[test]
fn id_reuse_fix_capacity_and_admission_errors_do_not_advance_cursors_or_rng() {
    let (c, mut room) = setup(true);
    attach(&mut room, &c, 1);
    let cursor = aim(&room, 1);
    let mut unfunded = room.clone();
    unfunded.character_mut(1).stats.insert(4, 39);
    let before = snapshot(&unfunded);
    assert!(unfunded.handle(1, cast(1, 1, 1, cursor), &c).is_err());
    assert_eq!(snapshot(&unfunded), before);
    assert!(server_cursors(&unfunded).is_empty());
    room.handle(1, cast(1, 1, 1, cursor), &c).unwrap();
    let template = room.bullets[0].clone();
    room.advance(1., &c).unwrap();
    assert_eq!(server_cursors(&room), BTreeMap::from([(1, 117)]));
    for failure in 0..5 {
        let mut candidate = room.clone();
        if failure == 0 {
            candidate.bullets = (97..=255)
                .map(|id| Bullet {
                    id,
                    ..template.clone()
                })
                .collect();
        } else if failure == 1 {
            candidate.bullets = (0..4090)
                .map(|i| Bullet {
                    owner: 999,
                    id: (i % 255 + 1) as u8,
                    ..template.clone()
                })
                .collect();
        } else if failure == 2 {
            candidate.bullets = (0..250)
                .map(|i| Bullet {
                    id: (i % 96 + 1) as u8,
                    ..template.clone()
                })
                .collect();
        } else if failure == 3 {
            candidate.character_mut(1).stats.insert(4, 39);
        }
        let before = server_cursors(&candidate);
        let input = cast(1, 1, if failure == 4 { 2 } else { 1 }, cursor);
        reject(&c, &candidate, input);
        assert_eq!(server_cursors(&candidate), before);
        let mut failed = candidate.clone();
        assert!(
            failed
                .handle(1, cast(1, 1, if failure == 4 { 2 } else { 1 }, cursor), &c)
                .is_err()
        );
        assert_eq!(server_cursors(&failed), before);
    }
    let gun = id_reuse_gun(&mut room, &c, 5.);
    room.advance(0., &c).unwrap();
    let template = room
        .bullets
        .iter()
        .find(|b| b.owner == gun)
        .unwrap()
        .clone();
    room.bullets = (1..=255)
        .map(|id| Bullet {
            id,
            ..template.clone()
        })
        .collect();
    room.entities.get_mut(&gun).unwrap().age = 3.;
    let before = server_cursors(&room);
    let rng = room.rng.clone().next_u64();
    assert!(room.advance(0., &c).is_err());
    assert_eq!(server_cursors(&room), before);
    assert_eq!(room.rng.clone().next_u64(), rng);
}
