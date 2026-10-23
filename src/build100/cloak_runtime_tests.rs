use super::*;
use crate::{
    content::Grounds,
    content::{Activation, Condition, ConditionEffect},
    realm_map,
};
use std::path::PathBuf;

fn setup(repaired: bool, tutorial: bool) -> (Content, Room) {
    let path = PathBuf::from(
        std::env::var_os(if repaired {
            "BUILD100_CONTENT_REPAIRED_DIR"
        } else {
            "BUILD100_CONTENT_ORIGINAL_DIR"
        })
        .expect("qualified content required"),
    );
    let c = Content::load(&path).unwrap();
    let g = Arc::new(Grounds::load(&path).unwrap());
    let map = if tutorial {
        realm_map::tutorial(100, &c, g)
    } else {
        realm_map::realm(100, &c, g)
    }
    .unwrap();
    let mut r = Room::from_map(Arc::new(map), &c, 1_000_000).unwrap();
    let mut p = Character::new(&c, 0x300, "Cloak".into(), 1).unwrap();
    (p.x, p.y) = r.landing();
    r.attach(1, p, &c).unwrap();
    (c, r)
}
fn use_item(owner: i32, slot: u8, target: i32, x: f32, y: f32) -> Packet {
    packet(15, |w| {
        w.i32(owner).u8(slot).i32(target).f32(x).f32(y);
    })
}
fn cloak() -> Packet {
    use_item(1, 1, 1, 10000., -10000.)
}
fn carry_bits(p: &Character) -> [u64; 4] {
    let c = p.resources_carry.unwrap();
    [
        c.hp_remainder.to_bits(),
        c.mp_remainder.to_bits(),
        c.hp_roundoff.to_bits(),
        c.mp_roundoff.to_bits(),
    ]
}
fn wire_stats(packets: &[Packet], packet_id: u8, id: i32) -> BTreeMap<u8, i32> {
    for p in packets.iter().filter(|p| p.id == packet_id) {
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
            let count = r.i16().unwrap();
            let stats = (0..count)
                .map(|_| (r.u8().unwrap(), r.i32().unwrap()))
                .collect();
            if found == id {
                return stats;
            }
        }
    }
    panic!("missing status {packet_id}/{id}");
}
fn reject(c: &Content, r: &Room, p: Packet) -> String {
    let mut candidate = r.clone();
    let before = candidate.character(1).clone();
    let error = candidate.handle(1, p, c).err().expect("must reject");
    assert_eq!(candidate.character(1), &before);
    assert_eq!(candidate.bullets.len(), r.bullets.len());
    format!("{error:#}")
}

#[test]
fn cloak_runtime_original_repaired_root_cost_duration_and_source_mask() {
    assert_eq!(Condition::Invisible as u8, 13);
    assert_eq!(combat::ConditionMask::bit(Condition::Invisible), 4096);
    for (repaired, duration) in [(false, 30.), (true, 3.)] {
        let (c, mut r) = setup(repaired, false);
        assert_eq!(c.get(0xa56).unwrap().mp_cost, Some(80));
        assert_eq!(c.get(0xa56).unwrap().effects[0].duration_seconds, duration);
        let before = r.character(1).clone();
        let events = r.handle(1, cloak(), &c).unwrap();
        let after = r.character(1);
        assert_eq!(after.stats[&4], before.stats[&4] - 80);
        assert_eq!(after.stats[&25], 4096);
        assert_eq!(
            after.action_state.as_ref().unwrap().conditions.expiries[12],
            Some(f64::from(duration))
        );
        assert_eq!(
            after.action_state.as_ref().unwrap().ability_used_seconds,
            Some(0.)
        );
        assert_eq!(carry_bits(after), carry_bits(&before));
        assert_eq!(after.stats[&9], before.stats[&9]);
        let published = r.publication(1, &events, &c).unwrap();
        assert_eq!(wire_stats(&published, 10, 1)[&29], 4096);
        assert_eq!(wire_stats(&published, 13, 1)[&4], 20);
        r.advance(f64::from(duration) - 0.125, &c).unwrap();
        assert_eq!(r.character(1).stats[&25], 4096);
        r.advance(0.125, &c).unwrap();
        assert_eq!(r.character(1).stats[&25], 0);
        assert_eq!(r.character(1).action_state, Some(ActionState::default()));
        for kind in [0xa57, 0xa58, 0xa59] {
            let mut candidate = r.clone();
            candidate
                .character_mut(1)
                .stats
                .extend([(9, kind), (4, 100)]);
            candidate
                .character_mut(1)
                .resources_carry
                .as_mut()
                .unwrap()
                .mp_remainder = 0.;
            candidate
                .character_mut(1)
                .resources_carry
                .as_mut()
                .unwrap()
                .mp_roundoff = 0.;
            candidate.handle(1, cloak(), &c).unwrap();
            assert_eq!(
                candidate
                    .character(1)
                    .action_state
                    .as_ref()
                    .unwrap()
                    .conditions
                    .expiries[12],
                Some(5.)
            );
            assert_eq!(candidate.character(1).stats[&4], 20);
        }
    }
}

#[test]
fn cloak_runtime_cooldown_500ms_exact_boundary_refresh_max_duplicates_and_carry() {
    let (mut c, mut r) = setup(false, false);
    r.character_mut(1)
        .stats
        .extend([(3, 500), (4, 300), (1, 50)]);
    r.advance(0.1, &c).unwrap();
    let before = r.character(1).clone();
    assert!(carry_bits(&before)[2..].iter().any(|&v| v != 0));
    let item = c.objects.get_mut(&0xa56).unwrap();
    item.activations.push(item.activations[0].clone());
    item.effects.extend([
        ConditionEffect {
            condition: Condition::Invisible,
            duration_seconds: 1.,
        },
        item.effects[0].clone(),
    ]);
    r.handle(1, cloak(), &c).unwrap();
    assert_eq!(r.character(1).stats[&4], 220);
    assert_eq!(carry_bits(r.character(1)), carry_bits(&before));
    assert_eq!(
        r.character(1)
            .action_state
            .as_ref()
            .unwrap()
            .conditions
            .expiries
            .iter()
            .flatten()
            .count(),
        1
    );
    assert!(reject(&c, &r, cloak()).contains("cooldown"));
    let below = 0.5_f64.next_down();
    r.advance(below, &c).unwrap();
    assert!(reject(&c, &r, cloak()).contains("cooldown"));
    r.advance(0.5 - below, &c).unwrap();
    c.objects.get_mut(&0xa56).unwrap().effects = vec![ConditionEffect {
        condition: Condition::Invisible,
        duration_seconds: 1.,
    }];
    r.handle(1, cloak(), &c).unwrap();
    assert_eq!(
        r.character(1)
            .action_state
            .as_ref()
            .unwrap()
            .conditions
            .expiries[12],
        Some(30.)
    );
    r.advance(0.5, &c).unwrap();
    c.objects.get_mut(&0xa56).unwrap().effects[0].duration_seconds = 40.;
    r.handle(1, cloak(), &c).unwrap();
    assert_eq!(r.character(1).stats[&25], 4096);
    assert_eq!(
        r.character(1)
            .action_state
            .as_ref()
            .unwrap()
            .conditions
            .expiries[12],
        Some(41.)
    );
    assert_eq!(r.character(1).stats[&4], 61); // ordinary MP regeneration, three exact root charges
}

#[test]
fn cloak_runtime_wire_owner_target_slot_malformed_and_mana_are_atomic() {
    let (c, r) = setup(false, false);
    for p in [
        use_item(2, 1, 1, 0., 0.),
        use_item(1, 1, -1, 0., 0.),
        use_item(1, 1, 2, 0., 0.),
        use_item(1, 12, 1, 0., 0.),
        use_item(1, 0, 1, 0., 0.),
    ] {
        reject(&c, &r, p);
    }
    for (offset, value) in [(9, f32::NAN), (13, f32::INFINITY), (13, f32::NEG_INFINITY)] {
        let mut p = cloak();
        p.body[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        reject(&c, &r, p);
    }
    for length in 0..17 {
        let mut p = cloak();
        p.body.truncate(length);
        reject(&c, &r, p);
    }
    let mut p = cloak();
    p.body.push(0);
    reject(&c, &r, p);
    let mut backpack = r.clone();
    backpack
        .character_mut(1)
        .stats
        .extend([(9, -1), (12, 0xa56)]);
    reject(&c, &backpack, cloak());
    reject(&c, &backpack, use_item(1, 4, 1, 0., 0.));
    for item in [-1, 0x7fff] {
        let mut stale = r.clone();
        stale.character_mut(1).stats.insert(9, item);
        reject(&c, &stale, cloak());
    }
    let mut poor = r.clone();
    poor.character_mut(1).stats.insert(4, 79);
    assert!(reject(&c, &poor, cloak()).contains("insufficient MP"));
}

#[test]
fn cloak_runtime_whole_semantics_prevalidate_compounds_unknown_effects_and_metadata() {
    let (c, r) = setup(false, false);
    for change in 0..16 {
        let mut c = Content {
            objects: c.objects.clone(),
        };
        let item = c.objects.get_mut(&0xa56).unwrap();
        match change {
            0 => item.activations.clear(),
            1 => item.activations.push(Activation {
                name: "Teleport".into(),
                attributes: BTreeMap::new(),
            }),
            2 => item.activations.insert(
                0,
                Activation {
                    name: "Teleport".into(),
                    attributes: BTreeMap::new(),
                },
            ),
            3 => item.activations.push(Activation {
                name: "Unrecognized".into(),
                attributes: BTreeMap::new(),
            }),
            4 => {
                item.activations[0]
                    .attributes
                    .insert("range".into(), "1".into());
            }
            5 => item.effects.clear(),
            6 => item.effects.push(ConditionEffect {
                condition: Condition::Berserk,
                duration_seconds: 1.,
            }),
            7 => item.effects[0].duration_seconds = 0.,
            8 => item.effects[0].duration_seconds = -1.,
            9 => item.effects[0].duration_seconds = f32::NAN,
            10 => item.effects[0].duration_seconds = f32::INFINITY,
            11 => item.mp_cost = None,
            12 => item.mp_cost = Some(-1),
            13 => item.consumable = true,
            14 => item.slot_type = Some(16),
            15 => item.class = "GameObject".into(),
            _ => unreachable!(),
        }
        reject(&c, &r, cloak());
    }
    for condition in [
        Condition::Nothing,
        Condition::Dead,
        Condition::ArmorPierce,
        Condition::Quiet,
        Condition::Healing,
        Condition::Bleeding,
        Condition::Sick,
        Condition::Berserk,
        Condition::Paused,
    ] {
        let mut c = Content {
            objects: c.objects.clone(),
        };
        c.objects.get_mut(&0xa56).unwrap().effects[0].condition = condition;
        reject(&c, &r, cloak());
    }
    let mut planewalker = r.clone();
    planewalker.character_mut(1).stats.insert(9, 0xa5a);
    assert!(reject(&c, &planewalker, cloak()).contains("Teleport"));
    for kind in [0x307, 0x30e, 0x310, 0x31d, 0x31e, 0x31f] {
        let mut candidate = r.clone();
        candidate.detach(1);
        let mut p = Character::new(&c, kind, "Unsupported".into(), 1).unwrap();
        if kind == 0x310 {
            p.stats.insert(9, 0xa5b);
        }
        (p.x, p.y) = candidate.landing();
        candidate.attach(1, p, &c).unwrap();
        let mut unsupported = Content {
            objects: c.objects.clone(),
        };
        if kind == 0x30e {
            let item = candidate.character(1).stats[&9] as i16;
            unsupported
                .objects
                .get_mut(&item)
                .unwrap()
                .effects
                .push(ConditionEffect {
                    condition: Condition::Healing,
                    duration_seconds: 1.,
                });
        }
        assert!(reject(&unsupported, &candidate, cloak()).contains("unsupported normal"));
    }
}

#[test]
fn cloak_runtime_root_effects_only_no_nested_promotion() {
    let (mut c, mut r) = setup(false, false);
    let mut projectile = c.get(0xa97).unwrap().projectiles[0].clone();
    projectile.1.effects.push(ConditionEffect {
        condition: Condition::Berserk,
        duration_seconds: 200.,
    });
    let item = c.objects.get_mut(&0xa56).unwrap();
    item.projectiles.push(projectile);
    r.handle(1, cloak(), &c).unwrap();
    assert_eq!(r.character(1).stats[&25], 4096);
    assert_eq!(
        r.character(1)
            .action_state
            .as_ref()
            .unwrap()
            .conditions
            .expiries[12],
        Some(30.)
    );
    let mut without_root = r.clone();
    without_root.character_mut(1).action_state = Some(ActionState::default());
    without_root
        .character_mut(1)
        .stats
        .extend([(25, 0), (4, 100)]);
    c.objects.get_mut(&0xa56).unwrap().effects.clear();
    reject(&c, &without_root, cloak());
}

#[test]
fn cloak_runtime_expiry_partitions_actual_mask_regen_and_exact_save_bits() {
    let (mut c, mut r) = setup(false, false);
    super::shore_runtime_tests::isolate_population(&mut r);
    c.objects.get_mut(&0xa56).unwrap().effects[0].duration_seconds = 0.25;
    r.character_mut(1).stats.insert(1, 50);
    r.handle(1, cloak(), &c).unwrap();
    let initial = r.character(1).clone();
    let mut split = r.clone();
    r.advance(0.75, &c).unwrap();
    for dt in [0.25, 0.25, 0.25] {
        split.advance(dt, &c).unwrap();
    }
    assert_eq!(r.character(1), split.character(1));
    let stats = initial.equipment_stats(&c).unwrap();
    let expected = initial
        .vitals(&stats)
        .unwrap()
        .regenerate(
            0.25,
            &stats,
            combat::ConditionMask::from_bits(4096).unwrap(),
        )
        .unwrap()
        .regenerate(0.25, &stats, combat::ConditionMask::default())
        .unwrap()
        .regenerate(0.25, &stats, combat::ConditionMask::default())
        .unwrap();
    assert_eq!(r.character(1).vitals(&stats).unwrap(), expected);
    let mut exact = split.clone();
    exact.character_mut(1).stats.insert(4, 100);
    exact
        .character_mut(1)
        .resources_carry
        .as_mut()
        .unwrap()
        .mp_remainder = 0.;
    exact
        .character_mut(1)
        .resources_carry
        .as_mut()
        .unwrap()
        .mp_roundoff = 0.;
    exact.handle(1, cloak(), &c).unwrap();
    exact.advance(0.25_f64.next_down(), &c).unwrap();
    assert_eq!(exact.character(1).stats[&25], 4096);
    let json = serde_json::to_string(exact.character(1)).unwrap();
    let loaded: Character = serde_json::from_str(&json).unwrap();
    assert_eq!(loaded, *exact.character(1));
    assert_eq!(carry_bits(&loaded), carry_bits(exact.character(1)));
    exact.detach(1);
    exact.advance(3600., &c).unwrap();
    exact.attach(1, loaded.clone(), &c).unwrap();
    assert_eq!(exact.character(1), &loaded);
    exact.advance(0.25 - 0.25_f64.next_down(), &c).unwrap();
    assert_eq!(exact.character(1).stats[&25], 0);
    assert!(
        exact
            .character(1)
            .action_state
            .as_ref()
            .unwrap()
            .ability_used_seconds
            .is_some()
    );
    reject(&c, &exact, cloak());
    exact.advance(0.25, &c).unwrap();
    assert_eq!(
        exact.character(1).action_state,
        Some(ActionState::default())
    );
}

#[test]
fn cloak_runtime_strict_saved_state_matrix_and_legacy_migration() {
    let (c, mut r) = setup(false, false);
    r.handle(1, cloak(), &c).unwrap();
    r.advance(0.125, &c).unwrap();
    let valid = serde_json::to_value(r.character(1)).unwrap();
    let attach = |value: serde_json::Value| -> Result<Character> {
        let p = serde_json::from_value(value)?;
        let mut candidate = r.clone();
        candidate.detach(1);
        candidate.attach(1, p, &c)?;
        Ok(candidate.character(1).clone())
    };
    assert_eq!(attach(valid.clone()).unwrap(), *r.character(1));
    for field in ["version", "conditions", "ability_used_seconds"] {
        let mut bad = valid.clone();
        bad["action_state"].as_object_mut().unwrap().remove(field);
        assert!(attach(bad).is_err(), "missing {field}");
    }
    for field in ["now_seconds", "expiries"] {
        let mut bad = valid.clone();
        bad["action_state"]["conditions"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(attach(bad).is_err());
    }
    for change in 0..17 {
        let mut bad = valid.clone();
        match change {
            0 => bad["action_state"] = serde_json::Value::Null,
            1 => bad["action_state"]["version"] = 2.into(),
            2 => bad["action_state"]["extra"] = 1.into(),
            3 => bad["action_state"]["conditions"]["extra"] = 1.into(),
            4 => bad["action_state"]["conditions"]["now_seconds"] = (-1).into(),
            5 => bad["action_state"]["conditions"]["expiries"][12] = 0.125.into(),
            6 => bad["action_state"]["conditions"]["expiries"][12] = 0.1.into(),
            7 => bad["action_state"]["conditions"]["expiries"][0] = 1.into(),
            8 => bad["action_state"]["conditions"]["expiries"][19] = 1.into(),
            9 => bad["stats"]["25"] = 0.into(),
            10 => {
                bad["stats"].as_object_mut().unwrap().remove("25");
            }
            11 => bad["stats"]["25"] = (-1).into(),
            12 => bad["action_state"]["ability_used_seconds"] = 1.into(),
            13 => bad["action_state"]["ability_used_seconds"] = (-1).into(),
            14 => {
                bad["alive"] = false.into();
                bad["stats"]["1"] = 0.into();
            }
            15 => {
                bad.as_object_mut().unwrap().remove("action_state");
            }
            16 => {
                bad["action_state"]["conditions"]["now_seconds"] = 0.5.into();
            }
            _ => unreachable!(),
        }
        assert!(attach(bad).is_err(), "case {change}");
    }
    for index in 0..21 {
        if index == 12 {
            continue;
        }
        let mut bad = valid.clone();
        bad["action_state"]["conditions"]["expiries"][index] = 1.into();
        assert!(attach(bad).is_err());
    }
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for field in 0..3 {
            let mut bad = r.character(1).clone();
            let state = bad.action_state.as_mut().unwrap();
            match field {
                0 => state.conditions.now_seconds = value,
                1 => state.conditions.expiries[12] = Some(value),
                _ => state.ability_used_seconds = Some(value),
            }
            let mut candidate = r.clone();
            candidate.detach(1);
            assert!(candidate.attach(1, bad, &c).is_err());
        }
    }
    let mut legacy = valid.clone();
    legacy.as_object_mut().unwrap().remove("action_state");
    legacy["stats"]["25"] = 0.into();
    let migrated = attach(legacy).unwrap();
    assert_eq!(migrated.action_state, Some(ActionState::default()));
    assert_eq!(migrated.resources_carry, r.character(1).resources_carry);
    assert_eq!(
        attach(serde_json::to_value(&migrated).unwrap()).unwrap(),
        migrated
    );
    let mut idle = serde_json::to_value(&migrated).unwrap();
    idle["action_state"]["conditions"]["now_seconds"] = 1.into();
    assert!(attach(idle).is_err());
}

#[test]
fn cloak_runtime_self_peer_interest_make_tick_and_expiry() {
    let (c, mut r) = setup(true, false);
    let mut peer = Character::new(&c, 0x30e, "Peer".into(), 1).unwrap();
    (peer.x, peer.y) = r.landing();
    r.attach(2, peer, &c).unwrap();
    r.publication(1, &[], &c).unwrap();
    r.publication(2, &[], &c).unwrap();
    let events = r.handle(1, cloak(), &c).unwrap();
    for id in [1, 2] {
        let out = r.publication(id, &events, &c).unwrap();
        assert_eq!(wire_stats(&out, 13, 1)[&29], 4096);
        assert_eq!(wire_stats(&out, 13, 1)[&4], 20);
    }
    let saved = r.character(1).clone();
    r.character_mut(1).x += 100.;
    assert!(
        r.publication(2, &[], &c)
            .unwrap()
            .iter()
            .any(|p| p.id == 11)
    );
    *r.character_mut(1) = saved;
    assert_eq!(
        wire_stats(&r.publication(2, &[], &c).unwrap(), 10, 1)[&29],
        4096
    );
    let events = r.advance(3., &c).unwrap();
    for id in [1, 2] {
        assert_eq!(
            wire_stats(&r.publication(id, &events, &c).unwrap(), 13, 1)[&29],
            0
        );
    }
}

#[test]
fn cloak_runtime_no_invulnerability_or_gun_suppression_death_clears_timers() {
    let (c, mut visible) = setup(false, true);
    let (x, y) = (16.5, 13.5);
    visible.character_mut(1).x = x;
    visible.character_mut(1).y = y;
    let mut invisible = visible.clone();
    invisible.handle(1, cloak(), &c).unwrap();
    let mut hits = 0;
    for _ in 0..110 {
        let a = visible.advance(0.05, &c).unwrap();
        let b = invisible.advance(0.05, &c).unwrap();
        let shots = |out: &[Packet]| {
            out.iter()
                .filter(|p| p.id == 8)
                .map(|p| p.body.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(shots(&a), shots(&b));
        hits += b
            .iter()
            .filter(|p| p.id == 9 && Reader::new(&p.body).i32().unwrap() == 1)
            .count();
        assert_eq!(
            visible.character(1).stats[&1],
            invisible.character(1).stats[&1]
        );
    }
    assert!(hits > 0, "Invisible must still receive fixed-gun damage");
    let mut lethal = invisible.clone();
    lethal.bullets.push(Bullet {
        victims: BTreeSet::new(),
        flight: tests::normal_flight(0.1, 2.),
        owner: 999,
        id: 1,
        origin: (x, y),
        age: 0.,
        amplitude: 0.,
        frequency: 1.,
        x,
        y,
        vx: 0.,
        vy: 0.,
        damage: 1000,
        life: 2.,
    });
    lethal.advance(0.125, &c).unwrap();
    assert!(!lethal.character(1).alive);
    assert_eq!(lethal.character(1).stats[&25], 0);
    assert_eq!(
        lethal.character(1).action_state,
        Some(ActionState::default())
    );
    assert_eq!(
        lethal.character(1).resources_carry.unwrap().hp_remainder,
        0.
    );
    let dead = lethal.character(1).clone();
    assert!(lethal.handle(1, cloak(), &c).unwrap().is_empty());
    lethal.advance(1., &c).unwrap();
    assert_eq!(lethal.character(1), &dead);
}

#[test]
fn cloak_runtime_shoot_does_not_remove_invisibility_and_idle_state_is_stable() {
    let (c, mut r) = setup(true, false);
    r.handle(1, cloak(), &c).unwrap();
    let p = r.character(1);
    let kind = p.stats[&8] as i16;
    let proj = c.get(kind).unwrap().normal_projectile(0).unwrap();
    let fire = packet(8, |w| {
        w.u8(5)
            .i32(1)
            .u8(c.named(&proj.visual).unwrap().id as u8)
            .i16(kind)
            .f32(p.x)
            .f32(p.y)
            .f32(0.)
            .i16(0);
    });
    r.handle(1, fire, &c).unwrap();
    assert_eq!(r.character(1).stats[&25], 4096);
    r.advance(100., &c).unwrap();
    let before = r.character(1).clone();
    assert_eq!(before.action_state, Some(ActionState::default()));
    for _ in 0..10 {
        r.advance(1., &c).unwrap();
    }
    assert_eq!(r.character(1), &before);
    let mut legacy = before;
    legacy.action_state = None;
    assert!(
        !serde_json::to_string(&legacy)
            .unwrap()
            .contains("action_state")
    );
}

#[test]
fn cloak_runtime_fractional_timestamp_cooldown_does_not_round_early_or_stall() {
    let (c, mut r) = setup(false, false);
    r.character_mut(1).stats.extend([(3, 500), (4, 400)]);
    r.handle(1, cloak(), &c).unwrap();
    r.advance(0.9, &c).unwrap();
    r.handle(1, cloak(), &c).unwrap();
    r.advance(0.5, &c).unwrap();
    let now = r
        .character(1)
        .action_state
        .as_ref()
        .unwrap()
        .conditions
        .now_seconds;
    assert!(now - 0.9 < 0.5);
    reject(&c, &r, cloak());
    r.advance(now.next_up() - now, &c).unwrap();
    r.handle(1, cloak(), &c).unwrap();
    assert_eq!(r.character(1).stats[&4], 161);
}

#[test]
fn cloak_runtime_saved_state_validation_guards_actions_frames_and_publication() {
    let (c, r) = setup(false, false);
    for index in 0..3 {
        let mut candidate = r.clone();
        let p = candidate.character_mut(1);
        match index {
            0 => p.action_state.as_mut().unwrap().conditions.expiries[19] = Some(1.),
            1 => p.action_state.as_mut().unwrap().conditions.expiries[12] = Some(1.),
            _ => p.action_state.as_mut().unwrap().ability_used_seconds = Some(1.),
        }
        let before = candidate.character(1).clone();
        reject(&c, &candidate, cloak());
        assert!(candidate.advance(0.1, &c).is_err());
        assert_eq!(candidate.character(1), &before);
        assert!(candidate.arrival(1, &c).is_err());
        assert!(candidate.publication(1, &[], &c).is_err());
    }
}

#[test]
fn cloak_runtime_lava_death_clears_live_action_state_without_refund() {
    let (c, mut r) = setup(false, false);
    r.handle(1, cloak(), &c).unwrap();
    let i = r
        .map
        .as_ref()
        .unwrap()
        .tiles()
        .iter()
        .position(|t| t.ground == 0x70)
        .unwrap();
    let (w, _) = r.map.as_ref().unwrap().dimensions();
    let p = r.character_mut(1);
    (p.x, p.y) = (
        (i % usize::from(w)) as f32 + 0.5,
        (i / usize::from(w)) as f32 + 0.5,
    );
    p.stats.insert(1, 1);
    r.advance(0.5, &c).unwrap();
    let dead = r.character(1).clone();
    assert!(!dead.alive);
    assert_eq!(dead.stats[&4], 20);
    assert_eq!(dead.stats[&25], 0);
    assert_eq!(dead.action_state, Some(ActionState::default()));
    assert_eq!(dead.resources_carry.unwrap().hp_roundoff, 0.);
    r.advance(10., &c).unwrap();
    assert_eq!(r.character(1), &dead);
}

#[test]
fn cloak_wire_original_c8_dispatch_self_peer_make_tick_arrival_interest_and_expiry() {
    // Original Gs._C8: StatData._Iw=29 -> switch14 -> ofs0124 -> GameObject._MI.
    let check = |out: &[Packet], packet_id, avatar, mask| {
        let stats = wire_stats(out, packet_id, avatar);
        assert_eq!(
            stats.get(&29),
            Some(&mask),
            "client condition dispatch requires wire29"
        );
        assert!(
            !stats.contains_key(&25),
            "internal25 must not leak as a wire condition alias"
        );
    };
    for repaired in [false, true] {
        let (c, mut r) = setup(repaired, false);
        let mut peer = Character::new(&c, 0x30e, "Peer".into(), 1).unwrap();
        (peer.x, peer.y) = r.landing();
        r.attach(2, peer, &c).unwrap();
        let initial_arrival = r.arrival(1, &c).unwrap();
        r.character_mut(1).stats.extend([(1, 50), (4, 90)]);
        r.advance(0.1, &c).unwrap();
        let before = r.character(1).clone();
        assert!(carry_bits(&before)[2..].iter().any(|&word| word != 0));
        let events = r.handle(1, cloak(), &c).unwrap();
        let paid = r.character(1).clone();
        let peer = r.character(2).clone();
        assert_eq!(paid.stats[&25], 4096);
        assert!(!paid.stats.contains_key(&29));
        assert_eq!(paid.stats[&4], before.stats[&4] - 80);
        assert_eq!(carry_bits(&paid), carry_bits(&before));
        check(&[r.arrival(1, &c).unwrap()], 10, 1, 4096);
        check(&[initial_arrival], 10, 1, 0);
        for viewer in [1, 2] {
            let out = r.publication(viewer, &events, &c).unwrap();
            for packet_id in [10, 13] {
                check(&out, packet_id, 1, 4096);
                check(&out, packet_id, 2, 0);
                let actual = wire_stats(&out, packet_id, 1);
                let mut expected = paid.stats.clone();
                expected.remove(&25);
                expected.insert(29, 4096);
                expected.extend(paid.equipment_stats(&c).unwrap().iter());
                assert_eq!(
                    actual, expected,
                    "all other wire stats must retain their projection"
                );
            }
        }
        assert_eq!(
            r.character(1),
            &paid,
            "publication must not change save/timers/MP/carry"
        );
        assert_eq!(r.character(2), &peer);
        let roundtrip: Character =
            serde_json::from_str(&serde_json::to_string(&paid).unwrap()).unwrap();
        assert_eq!(roundtrip, paid);
        assert!(!roundtrip.stats.contains_key(&29));
        let events = r.advance(0.25, &c).unwrap();
        let live = r.character(1).clone();
        for viewer in [1, 2] {
            check(&r.publication(viewer, &events, &c).unwrap(), 13, 1, 4096);
        }
        assert_eq!(r.character(1), &live);
        r.character_mut(1).x += 100.;
        assert!(
            r.publication(2, &[], &c)
                .unwrap()
                .iter()
                .any(|p| p.id == 11)
        );
        *r.character_mut(1) = live.clone();
        check(&r.publication(2, &[], &c).unwrap(), 10, 1, 4096);
        assert_eq!(r.character(1), &live);
        let duration = f64::from(c.get(0xa56).unwrap().effects[0].duration_seconds);
        let events = r.advance(duration - 0.25, &c).unwrap();
        let expired = r.character(1).clone();
        assert_eq!(expired.stats[&25], 0);
        assert!(!expired.stats.contains_key(&29));
        assert_eq!(expired.action_state, Some(ActionState::default()));
        for viewer in [1, 2] {
            check(&r.publication(viewer, &events, &c).unwrap(), 13, 1, 0);
        }
        check(&[r.arrival(1, &c).unwrap()], 10, 1, 0);
        assert_eq!(r.character(1), &expired);
    }
}

#[test]
fn cloak_wire_rejects_persistent29_without_normalizing_or_changing_fixture_projection() {
    let (c, mut r) = setup(true, false);
    r.handle(1, cloak(), &c).unwrap();
    for alias in [0, 4096, -1] {
        let mut candidate = r.clone();
        candidate.character_mut(1).stats.insert(29, alias);
        let before = candidate.character(1).clone();
        assert!(
            candidate.arrival(1, &c).is_err(),
            "persistent wire29 must be rejected"
        );
        assert!(candidate.publication(1, &[], &c).is_err());
        assert_eq!(candidate.character(1), &before);
        let fixture = Room::new(100, 0, 2);
        assert_eq!(fixture.wire_stats(&before, &c).unwrap(), before.stats);
    }
}
