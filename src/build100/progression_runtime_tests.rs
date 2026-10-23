use super::*;
use crate::{
    content::{Grounds, Loot},
    realm_map,
};
use rand::RngCore;
use std::path::Path;

pub fn setup() -> (Content, Room) {
    qualified(false)
}
fn qualified(repaired: bool) -> (Content, Room) {
    let path = std::env::var_os(if repaired {
        "BUILD100_CONTENT_REPAIRED_DIR"
    } else {
        "BUILD100_CONTENT_ORIGINAL_DIR"
    })
    .expect("qualified content");
    let c = Content::load(Path::new(&path)).unwrap();
    let grounds = Arc::new(Grounds::load(Path::new(&path)).unwrap());
    let map = realm_map::realm(100, &c, grounds).unwrap();
    let room = Room::from_map(Arc::new(map), &c, 1_000_000).unwrap();
    (c, room)
}
pub fn attach(room: &mut Room, c: &Content, id: i32) {
    let mut p = Character::new(c, 0x30e, format!("Earned{id}"), 7).unwrap();
    (p.x, p.y) = room.landing();
    room.attach(id, p, c).unwrap();
}
// Test-owned encounters use actual ordinary metadata, not production ecology or earned native evidence.
pub fn encounter(room: &mut Room, c: &Content, owner: i32, shots: usize) -> i32 {
    room.entities.clear();
    room.bullets.clear();
    let (x, y) = (room.character(owner).x + 1., room.character(owner).y);
    let def = c.named("Pirate").unwrap();
    let mut stats = def.stats.clone();
    stats.insert(1, stats[&0]);
    let id = room.allocate_entity().unwrap();
    room.entities.insert(
        id,
        Entity {
            id,
            kind: def.id,
            x,
            y,
            stats,
            age: 0.,
            provenance: Provenance::Map,
        },
    );
    for bullet in 0..shots {
        room.bullets.push(Bullet {
            victims: BTreeSet::new(),
            flight: tests::normal_flight(0.1, 2.),
            owner,
            id: bullet as u8,
            origin: (x, y),
            age: 0.,
            amplitude: 0.,
            frequency: 1.,
            x,
            y,
            vx: 0.,
            vy: 0.,
            damage: 100,
            life: 1.,
        });
    }
    id
}
pub fn bags(room: &Room) -> Vec<(i32, [i32; 8])> {
    room.entities
        .values()
        .filter(|e| e.kind == 0x500)
        .map(|e| (e.id, std::array::from_fn(|s| e.stats[&(8 + s as u8)])))
        .collect()
}
pub fn reward_state(room: &Room) -> (i32, usize, u64) {
    (
        room.next_id,
        room.entities.len(),
        room.rng.clone().next_u64(),
    )
}
pub fn exhaust_loot_ids(room: &mut Room) {
    room.id_limit = room.next_id + 1;
}
pub fn lethal_enemy_bullet(room: &mut Room, player: i32) {
    let p = room.character(player);
    let (x, y) = (p.x, p.y);
    room.bullets.push(Bullet {
        victims: BTreeSet::new(),
        flight: tests::normal_flight(0.1, 2.),
        owner: room.next_id,
        id: 254,
        origin: (x, y),
        age: 0.,
        amplitude: 0.,
        frequency: 1.,
        x,
        y,
        vx: 0.,
        vy: 0.,
        damage: 1000,
        life: 1.,
    });
}
pub fn guaranteed(c: &mut Content, count: usize) {
    c.objects.get_mut(&0x600).unwrap().loot = vec![
        Loot::Item {
            name: "Health Potion".into(),
            probability: 1.
        };
        count
    ];
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

#[test]
fn progression_runtime_original_ui_per_level_save_validation_and_old_cohort() {
    let (c, r) = setup();
    for level in [1, 2, 3, 19, 20] {
        let mut p = Character::new(&c, 0x30e, "Levels".into(), 7).unwrap();
        p.stats.extend([
            (5, progression::xp_goal(level).unwrap()),
            (6, if level == 20 { 0 } else { 1 }),
            (7, i32::from(level)),
        ]);
        let mut room = r.clone();
        room.attach(1, p.clone(), &c).unwrap();
        assert_eq!(
            room.wire_stats(room.character(1), &c).unwrap()[&6],
            p.stats[&6]
        );
        for (id, value) in [
            (5, -1),
            (6, -1),
            (6, progression::xp_goal(level).unwrap()),
            (7, 0),
            (7, 21),
        ] {
            let mut bad = p.clone();
            bad.stats.insert(id, value);
            assert!(
                r.clone().attach(1, bad, &c).is_err(),
                "stat{id}={value} level{level}"
            );
        }
        if level == 20 {
            p.stats.insert(6, 1);
            assert!(r.clone().attach(1, p, &c).is_err());
        }
    }
    let mut old = Character::new(&c, 0x30e, "Old".into(), 3).unwrap();
    old.resources_version = 0;
    old.resources_carry = None;
    old.action_state = None;
    let mut room = r.clone();
    room.attach(1, old, &c).unwrap();
    assert_eq!(room.character(1).progress(&c).unwrap().xp, 0);
    for stat in [0, 3, 20, 21, 22, 26, 27, 28, 5, 6, 7] {
        let mut bad = room.character(1).clone();
        bad.stats.remove(&stat);
        assert!(r.clone().attach(1, bad, &c).is_err());
    }
    let mut bad = room.character(1).clone();
    bad.stats.insert(0, 5000);
    assert!(r.clone().attach(1, bad, &c).is_err());
}

#[test]
fn progression_runtime_growth_exact_deltas_equipment_carry_clock_and_reconnect() {
    let (mut c, mut r) = setup();
    guaranteed(&mut c, 1);
    attach(&mut r, &c, 1);
    r.character_mut(1)
        .stats
        .extend([(6, 49), (1, 40), (4, 30), (11, 0xa27)]);
    // A live supported cooldown/Invisible state belongs to this actor, not its progress adapter.
    let mut conditions = combat::Conditions::default();
    conditions
        .refresh(
            &super::super::content::ConditionEffect {
                condition: super::super::content::Condition::Invisible,
                duration_seconds: 3.,
            },
            0.125,
        )
        .unwrap();
    let p = r.character_mut(1);
    p.stats.insert(25, 4096);
    p.action_state = Some(ActionState {
        version: 1,
        conditions: conditions.snapshot(),
        ability_used_seconds: Some(0.125),
    });
    p.resources_carry = Some(ResourceCarry {
        hp_remainder: 0.25,
        mp_remainder: 0.375,
        hp_roundoff: 1e-20,
        mp_roundoff: 2e-20,
    });
    let before = p.clone();
    encounter(&mut r, &c, 1, 2);
    r.advance(0., &c).unwrap();
    let after = r.character(1);
    assert_eq!(
        (after.stats[&5], after.stats[&6], after.stats[&7]),
        (150, 0, 2)
    );
    assert_eq!(
        after.stats[&1] - before.stats[&1],
        after.stats[&0] - before.stats[&0]
    );
    assert_eq!(
        after.stats[&4] - before.stats[&4],
        after.stats[&3] - before.stats[&3]
    );
    assert!(after.stats[&1] < after.equipment_stats(&c).unwrap().get(0).unwrap());
    assert_eq!(r.wire_stats(after, &c).unwrap()[&0], after.stats[&0] + 40);
    assert_eq!(carry(after), carry(&before));
    assert_eq!(after.action_state, before.action_state);
    assert_eq!(after.generation, 7);
    let saved: Character = serde_json::from_str(&serde_json::to_string(after).unwrap()).unwrap();
    let mut reconnected = setup().1;
    reconnected.attach(10, saved.clone(), &c).unwrap();
    assert_eq!(reconnected.character(10), &saved);
    assert_eq!(
        reconnected.wire_stats(&saved, &c).unwrap(),
        r.wire_stats(after, &c).unwrap()
    );
}

#[test]
fn progression_runtime_caps_multi_level_and_level_twenty_stop() {
    let (mut c, mut r) = setup();
    c.objects.get_mut(&0x600).unwrap().stats.insert(0, 200_000);
    attach(&mut r, &c, 1);
    let class = c.get(0x30e).unwrap();
    for &(stat, _, _, cap) in &class.growth {
        r.character_mut(1).stats.insert(stat, cap);
    }
    r.character_mut(1).stats.extend([(1, 20), (4, 10)]);
    let before = r.character(1).clone();
    encounter(&mut r, &c, 1, 1);
    r.entities.values_mut().next().unwrap().stats.insert(1, 5);
    r.advance(0., &c).unwrap();
    assert_eq!(
        (
            r.character(1).stats[&5],
            r.character(1).stats[&6],
            r.character(1).stats[&7]
        ),
        (1950, 0, 20)
    );
    for &(stat, _, _, cap) in &class.growth {
        assert_eq!(r.character(1).stats[&stat], cap);
    }
    assert_eq!(
        (r.character(1).stats[&1], r.character(1).stats[&4]),
        (before.stats[&1], before.stats[&4])
    );
    let maxed = r.character(1).clone();
    encounter(&mut r, &c, 1, 1);
    r.entities.values_mut().next().unwrap().stats.insert(1, 5);
    r.advance(0., &c).unwrap();
    assert_eq!(r.character(1), &maxed);
}

#[test]
fn progression_runtime_real_room_range_living_attribution_and_once_only() {
    let (mut c, mut r) = setup();
    guaranteed(&mut c, 1);
    for id in 1..=4 {
        attach(&mut r, &c, id);
    }
    let target = encounter(&mut r, &c, 1, 3);
    let e = r.entities[&target].clone();
    r.character_mut(2).x = e.x + 16.;
    r.character_mut(3).x = e.x + 15.99;
    r.character_mut(4).alive = false;
    r.character_mut(4).stats.insert(1, 0);
    r.advance(0., &c).unwrap();
    assert_eq!(
        (
            r.character(1).stats[&6],
            r.character(2).stats[&6],
            r.character(3).stats[&6],
            r.character(4).stats[&6]
        ),
        (1, 0, 1, 0)
    );
    assert_eq!(bags(&r).len(), 1);
    let before = reward_state(&r);
    r.handle(1, damage_packet(target, 1, 0, 32000), &c).unwrap();
    r.advance(0., &c).unwrap();
    assert_eq!(reward_state(&r), before);
    assert_eq!(r.character(1).stats[&6], 1);
    let mut without_owner = setup().1;
    attach(&mut without_owner, &c, 1);
    let target = encounter(&mut without_owner, &c, 1, 0);
    let enemy = without_owner.entities.remove(&target).unwrap();
    without_owner
        .normal_rewards(&enemy, -1, &c, &mut vec![])
        .unwrap();
    assert_eq!(without_owner.character(1).stats[&6], 0);
    assert!(bags(&without_owner).is_empty());
}

#[test]
fn progression_runtime_source_metadata_provenance_and_teaching_are_not_rewards() {
    let (mut c, mut r) = setup();
    guaranteed(&mut c, 1);
    attach(&mut r, &c, 1);
    let target = encounter(&mut r, &c, 1, 0);
    let enemy = r.entities[&target].clone();
    for provenance in [
        Provenance::Summoned { source: target - 1 },
        Provenance::Fixture,
        Provenance::Loot { source: target - 1 },
    ] {
        let mut prop = enemy.clone();
        prop.provenance = provenance;
        r.normal_rewards(&prop, 1, &c, &mut vec![]).unwrap();
    }
    for kind in [0x802, 0x803, 0x804, 0x805, 0x800, 0x806] {
        let mut prop = enemy.clone();
        prop.kind = kind;
        r.normal_rewards(&prop, 1, &c, &mut vec![]).unwrap();
    }
    let path = std::env::var_os("BUILD100_CONTENT_ORIGINAL_DIR").unwrap();
    let grounds = Arc::new(Grounds::load(Path::new(&path)).unwrap());
    let map = realm_map::tutorial(100, &c, grounds).unwrap();
    let mut tutorial = Room::from_map(Arc::new(map), &c, 1_500_000).unwrap();
    attach(&mut tutorial, &c, 1);
    tutorial.entities.retain(|_, e| e.kind != 0x801);
    tutorial.elapsed = 1.;
    tutorial.teaching_objects(&c).unwrap();
    let chicken = tutorial
        .entities
        .values()
        .find(|e| e.kind == 0x801)
        .unwrap()
        .clone();
    assert!(
        matches!(chicken.provenance, Provenance::Summoned { source } if tutorial.entities[&source].kind == 0x800)
    );
    tutorial
        .normal_rewards(&chicken, 1, &c, &mut vec![])
        .unwrap();
    assert_eq!(tutorial.character(1).stats[&6], 0);
    assert_eq!(
        tutorial
            .entities
            .values()
            .filter(|e| e.kind == 0x806)
            .count(),
        1
    );
    assert_eq!(r.character(1).stats[&6], 0);
    assert!(bags(&r).is_empty());
}

#[test]
fn progression_runtime_independent_loot_order_duplicates_empty_and_capacity() {
    let (mut c, base) = setup();
    for count in [0, 8, 9, 64] {
        guaranteed(&mut c, count);
        let mut r = base.clone();
        attach(&mut r, &c, 1);
        encounter(&mut r, &c, 1, 1);
        let mut expected_rng = r.rng.clone();
        let expected =
            progression::roll_loot(c.get(0x600).unwrap(), &c, &mut expected_rng).unwrap();
        r.advance(0., &c).unwrap();
        assert_eq!(bags(&r).len(), count.div_ceil(8));
        for ((_, actual), expected) in bags(&r).into_iter().zip(expected) {
            assert_eq!(actual, expected.map(|v| v.map_or(-1, i32::from)));
        }
        assert_eq!(r.rng.clone().next_u64(), expected_rng.next_u64());
    }
    let mut r = base.clone();
    attach(&mut r, &c, 1);
    c.objects.get_mut(&0x600).unwrap().loot = vec![
        Loot::EmptyContainer,
        Loot::Item {
            name: "Dirk".into(),
            probability: 0.5,
        },
        Loot::Item {
            name: "Health Potion".into(),
            probability: 0.,
        },
        Loot::Item {
            name: "Dirk".into(),
            probability: 1.,
        },
        Loot::Item {
            name: "Dirk".into(),
            probability: 0.5,
        },
    ];
    encounter(&mut r, &c, 1, 1);
    let mut rng = r.rng.clone();
    let expected = progression::roll_loot(c.get(0x600).unwrap(), &c, &mut rng).unwrap();
    r.advance(0., &c).unwrap();
    assert_eq!(bags(&r)[0].1, expected[0].map(|v| v.map_or(-1, i32::from)));
    assert_eq!(r.rng.clone().next_u64(), rng.next_u64());
    for failure in 0..4 {
        let mut c = Content {
            objects: c.objects.clone(),
        };
        guaranteed(&mut c, if failure == 0 { 65 } else { 1 });
        let mut candidate = base.clone();
        attach(&mut candidate, &c, 1);
        encounter(&mut candidate, &c, 1, 1);
        if failure == 1 {
            c.objects.get_mut(&0x600).unwrap().loot.push(Loot::Item {
                name: "Missing".into(),
                probability: 1.,
            });
        }
        if failure == 2 {
            candidate.id_limit = candidate.next_id + 1;
        }
        if failure == 3 {
            candidate.character_mut(1).stats.insert(6, 50);
        }
        assert!(candidate.advance(0., &c).is_err());
    }
}

#[test]
fn progression_runtime_all_seven_classes_original_repaired_permanent_growth_and_cleanup() {
    for repaired in [false, true] {
        let (mut c, base) = qualified(repaired);
        guaranteed(&mut c, 1);
        for kind in [0x300, 0x307, 0x30e, 0x310, 0x31d, 0x31e, 0x31f] {
            let mut r = base.clone();
            let mut p = Character::new(&c, kind, "Classes".into(), 2).unwrap();
            (p.x, p.y) = r.landing();
            p.stats.extend([(6, 49), (1, 30), (4, 20)]);
            let before = p.clone();
            r.attach(1, p, &c).unwrap();
            encounter(&mut r, &c, 1, 1);
            r.advance(0., &c).unwrap();
            let after = r.character(1);
            assert_eq!(
                (after.stats[&5], after.stats[&6], after.stats[&7]),
                (150, 0, 2)
            );
            assert_eq!(after.stats[&1] - 30, after.stats[&0] - before.stats[&0]);
            assert_eq!(after.stats[&4] - 20, after.stats[&3] - before.stats[&3]);
            after.progress(&c).unwrap();
            assert_eq!(bags(&r).len(), 1);
            r.advance(60.25, &c).unwrap();
            assert!(bags(&r).is_empty());
        }
    }
}
