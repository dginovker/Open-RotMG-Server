use super::*;
use crate::{content::Grounds, realm_map};
use std::path::PathBuf;

fn setup() -> (Content, Room) {
    let path = PathBuf::from(
        std::env::var_os("BUILD100_CONTENT_ORIGINAL_DIR").expect("qualified content required"),
    );
    let c = Content::load(&path).unwrap();
    let g = Arc::new(Grounds::load(&path).unwrap());
    let map = realm_map::realm(100, &c, g).unwrap();
    let mut r = Room::from_map(Arc::new(map), &c, 1_000_000).unwrap();
    super::shore_runtime_tests::isolate_population(&mut r);
    let mut p = Character::new(&c, 0x30e, "Resources".into(), 1).unwrap();
    (p.x, p.y) = r.landing();
    p.stats.extend([(1, 50), (4, 50)]);
    r.attach(1, p, &c).unwrap();
    (c, r)
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
fn swap(a: (u8, i16), b: (u8, i16)) -> Packet {
    packet(14, |w| {
        w.i32(1).u8(a.0).i16(a.1).i32(1).u8(b.0).i16(b.1);
    })
}
fn potion() -> Packet {
    packet(15, |w| {
        w.i32(1).u8(4).i32(-1).f32(0.).f32(0.);
    })
}

#[test]
fn resources_runtime_ordinary_rates_partition_and_f64_boundary() {
    let (c, r) = setup();
    let mut whole = r.clone();
    let mut split = r.clone();
    whole.advance(1., &c).unwrap();
    for _ in 0..8 {
        split.advance(0.125, &c).unwrap();
    }
    // Wizard26=12,27=12: 2.5 HP/s and (0.8+12/16.7) MP/s, independent of the helper.
    assert_eq!(
        (whole.character(1).stats[&1], whole.character(1).stats[&4]),
        (52, 51)
    );
    let carry = whole.character(1).resources_carry.unwrap();
    assert_eq!(carry.hp_remainder, 0.5);
    assert_eq!(carry.mp_remainder, (0.8 + 12. / 16.7) - 1.);
    assert_eq!((carry.hp_roundoff, carry.mp_roundoff), (0., 0.));
    assert_eq!(
        carry_bits(whole.character(1)),
        carry_bits(split.character(1))
    );
    assert_eq!(whole.character(1).stats, split.character(1).stats);

    let mut below = r.clone();
    let boundary = 0.4_f64;
    let just_below = f64::from_bits(boundary.to_bits() - 1);
    assert_eq!(just_below as f32, boundary as f32);
    below.advance(just_below, &c).unwrap();
    assert_eq!(below.character(1).stats[&1], 50);
    let before = below.character(1).resources_carry.unwrap();
    assert!(before.hp_remainder > 0.999999999999999);
    assert!(before.hp_roundoff != 0.);
    below.advance(boundary - just_below, &c).unwrap();
    assert_eq!(below.character(1).stats[&1], 51);
    let after = below.character(1).resources_carry.unwrap();
    assert!(after.hp_remainder > 0. && after.hp_remainder < 1e-15);

    let mut tiny = r;
    tiny.advance(1e-20, &c).unwrap();
    assert_eq!(tiny.character(1).stats[&1], 50);
    assert_eq!(
        tiny.character(1).resources_carry.unwrap().hp_remainder,
        2.5e-20
    );
}

#[test]
fn resources_runtime_fractional_save_roundtrip_preserves_all_words() {
    let (c, mut r) = setup();
    for _ in 0..10 {
        r.advance(0.1, &c).unwrap();
    }
    let p = r.character(1);
    // Exact binary rational 2.5 * f64(0.1) * 10 leaves this high/low pair, not epsilon-snapped 0.5.
    assert_eq!((p.stats[&1], p.stats[&4]), (52, 51));
    assert_eq!(p.resources_carry.unwrap().hp_remainder, 0.5000000000000001);
    assert_eq!(
        p.resources_carry.unwrap().hp_roundoff,
        2.7755575615628914e-17
    );
    assert_ne!(p.resources_carry.unwrap().hp_roundoff, 0.);
    assert_ne!(p.resources_carry.unwrap().mp_roundoff, 0.);
    let json = serde_json::to_string(p).unwrap();
    let loaded: Character = serde_json::from_str(&json).unwrap();
    assert_eq!(carry_bits(p), carry_bits(&loaded), "{json}");
    let mut restored = r.clone();
    restored.detach(1);
    restored.attach(1, loaded, &c).unwrap();
    r.advance(0.1, &c).unwrap();
    restored.advance(0.1, &c).unwrap();
    assert_eq!(r.character(1), restored.character(1));
    for step in 0..100 {
        r.advance(0.050000001, &c).unwrap();
        let json = serde_json::to_string(r.character(1)).unwrap();
        let loaded: Character = serde_json::from_str(&json).unwrap();
        assert_eq!(
            carry_bits(r.character(1)),
            carry_bits(&loaded),
            "step {step}: {json}"
        );
    }
}

#[test]
fn resources_runtime_saved_format_matrix_and_explicit_legacy_zero_migration() {
    let (c, r) = setup();
    let p = r.character(1);
    let original = serde_json::to_value(p).unwrap();
    let attach = |value: serde_json::Value| -> Result<Character> {
        let p = serde_json::from_value(value)?;
        let mut candidate = r.clone();
        candidate.detach(1);
        candidate.attach(1, p, &c)?;
        Ok(candidate.character(1).clone())
    };
    assert_eq!(attach(original.clone()).unwrap(), *p);
    for version in [None, Some(0)] {
        let mut legacy = original.clone();
        legacy.as_object_mut().unwrap().remove("resources_carry");
        if let Some(version) = version {
            legacy["resources_version"] = version.into();
        } else {
            legacy.as_object_mut().unwrap().remove("resources_version");
        }
        let mut null_legacy = legacy.clone();
        null_legacy["resources_carry"] = serde_json::Value::Null;
        assert!(attach(null_legacy).is_err());
        let migrated = attach(legacy).unwrap();
        assert_eq!(migrated.resources_version, 1);
        assert_eq!(migrated.resources_carry, Some(ResourceCarry::ZERO));
        assert_eq!(
            attach(serde_json::to_value(&migrated).unwrap()).unwrap(),
            migrated
        );
    }
    for version in [0, 2, 255] {
        let mut bad = original.clone();
        bad["resources_version"] = version.into();
        assert!(attach(bad.clone()).is_err());
        if version != 0 {
            bad.as_object_mut().unwrap().remove("resources_carry");
            assert!(attach(bad).is_err());
        }
    }
    let mut unversioned = original.clone();
    unversioned
        .as_object_mut()
        .unwrap()
        .remove("resources_version");
    assert!(attach(unversioned).is_err());
    let mut null_version = original.clone();
    null_version["resources_version"] = serde_json::Value::Null;
    assert!(attach(null_version).is_err());
    let mut missing = original.clone();
    missing.as_object_mut().unwrap().remove("resources_carry");
    assert!(attach(missing).is_err());
    let mut null = original.clone();
    null["resources_carry"] = serde_json::Value::Null;
    assert!(attach(null).is_err());
    for word in ["hp_remainder", "mp_remainder", "hp_roundoff", "mp_roundoff"] {
        let mut partial = original.clone();
        partial["resources_carry"]
            .as_object_mut()
            .unwrap()
            .remove(word);
        assert!(attach(partial).is_err());
        let mut null_word = original.clone();
        null_word["resources_carry"][word] = serde_json::Value::Null;
        assert!(attach(null_word).is_err());
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut p = p.clone();
            let carry = p.resources_carry.as_mut().unwrap();
            match word {
                "hp_remainder" => carry.hp_remainder = value,
                "mp_remainder" => carry.mp_remainder = value,
                "hp_roundoff" => carry.hp_roundoff = value,
                "mp_roundoff" => carry.mp_roundoff = value,
                _ => unreachable!(),
            }
            let mut candidate = r.clone();
            candidate.detach(1);
            assert!(candidate.attach(1, p, &c).is_err());
        }
    }
    for (word, value) in [
        ("hp_remainder", 1.),
        ("mp_remainder", -0.1),
        ("hp_roundoff", 0.25),
        ("mp_roundoff", 0.25),
    ] {
        let mut bad = original.clone();
        bad["resources_carry"][word] = value.into();
        assert!(attach(bad).is_err());
    }
    for (id, value) in [(1, -1), (1, 101), (1, 0), (4, -1), (4, 101)] {
        let mut bad = original.clone();
        bad["stats"][id.to_string()] = value.into();
        assert!(attach(bad).is_err());
    }
    let mut full = original.clone();
    full["stats"]["1"] = 100.into();
    full["resources_carry"]["hp_remainder"] = 0.5.into();
    assert!(attach(full).is_err());
    let mut full = original.clone();
    full["stats"]["4"] = 100.into();
    full["resources_carry"]["mp_remainder"] = 0.5.into();
    assert!(attach(full).is_err());
    let mut dead = original.clone();
    dead["alive"] = false.into();
    dead["stats"]["1"] = 0.into();
    assert!(attach(dead.clone()).is_ok());
    dead["resources_carry"]["hp_roundoff"] = 1e-20.into();
    assert!(attach(dead).is_err());
    let mut overflow = original.to_string();
    overflow = overflow.replace("\"hp_remainder\":0.0", "\"hp_remainder\":1e999");
    assert!(serde_json::from_str::<Character>(&overflow).is_err());
}

#[test]
fn resources_runtime_equipped_regen_cap_rebase_potion_and_shape_atomicity() {
    let (c, mut r) = setup();
    r.character_mut(1)
        .stats
        .extend([(11, 0x708), (13, 0xa27), (14, 0xa5f)]);
    r.advance(0.125, &c).unwrap();
    let carry = r.character(1).resources_carry.unwrap();
    // Equipped Spider's Eye contributes +3 to both resource stats (backpack contributes zero).
    assert_eq!(carry.hp_remainder, (1. + 15. / 8.) * 0.125);
    assert_eq!(carry.mp_remainder, (0.8 + 15. / 16.7) * 0.125);
    let bits = carry_bits(r.character(1));
    r.handle(1, swap((3, 0x708), (5, 0xa27)), &c).unwrap();
    r.handle(1, swap((2, -1), (6, 0xa5f)), &c).unwrap();
    assert_eq!(
        (r.character(1).stats[&1], r.character(1).stats[&4]),
        (50, 50)
    );
    assert_eq!(carry_bits(r.character(1)), bits);
    r.character_mut(1).stats.extend([(1, 130), (4, 130)]);
    r.handle(1, swap((3, 0xa27), (5, 0x708)), &c).unwrap();
    let carry = r.character(1).resources_carry.unwrap();
    assert_eq!(r.character(1).stats[&1], 100);
    assert_eq!((carry.hp_remainder, carry.hp_roundoff), (0., 0.));
    assert_eq!(carry.mp_remainder.to_bits(), bits[1]);
    r.handle(1, swap((2, 0xa5f), (6, -1)), &c).unwrap();
    assert_eq!(r.character(1).stats[&4], 100);
    assert_eq!(r.character(1).resources_carry, Some(ResourceCarry::ZERO));
    let full = r.character(1).clone();
    for _ in 0..10 {
        r.advance(0.1, &c).unwrap();
    }
    assert_eq!(r.character(1), &full);
    r.character_mut(1).stats.extend([(1, 99), (12, 0xa22)]);
    r.character_mut(1).resources_carry = Some(ResourceCarry {
        hp_remainder: -0.25,
        hp_roundoff: 1e-20,
        mp_remainder: 0.,
        mp_roundoff: 0.,
    });
    r.handle(1, potion(), &c).unwrap();
    assert_eq!(r.character(1).stats[&1], 100);
    assert_eq!(r.character(1).stats[&12], -1);
    assert_eq!(r.character(1).resources_carry, Some(ResourceCarry::ZERO));
    r.character_mut(1).stats.extend([(1, 50), (12, 0xa22)]);
    r.character_mut(1)
        .resources_carry
        .as_mut()
        .unwrap()
        .mp_roundoff = 0.5;
    let before = r.character(1).clone();
    assert!(r.handle(1, potion(), &c).is_err());
    assert_eq!(r.character(1), &before);
    assert!(r.handle(1, swap((3, 0x708), (5, 0xa27)), &c).is_err());
    assert_eq!(r.character(1), &before);
}

#[test]
fn resources_runtime_death_ordering_hp_reset_mp_freeze_and_fixture_isolation() {
    let (c, mut r) = setup();
    r.character_mut(1).stats.insert(1, 1);
    r.bullets.push(Bullet {
        victims: BTreeSet::new(),
        flight: tests::normal_flight(0.1, 2.),
        owner: 999,
        id: 1,
        origin: r.landing(),
        age: 0.,
        amplitude: 0.,
        frequency: 1.,
        x: r.landing().0,
        y: r.landing().1,
        vx: 0.,
        vy: 0.,
        damage: 1,
        life: 2.,
    });
    r.advance(0.5, &c).unwrap();
    // Regen precedes the hit: 1 + trunc(2.5*0.5) - 1 stays alive.
    assert_eq!(r.character(1).stats[&1], 1);
    assert!(r.character(1).alive);
    r.advance(0.1, &c).unwrap();
    assert_ne!(r.character(1).resources_carry.unwrap().hp_roundoff, 0.);
    let mut lethal = r.bullets.clone();
    lethal.push(Bullet {
        victims: BTreeSet::new(),
        flight: tests::normal_flight(0.1, 2.),
        owner: 999,
        id: 2,
        origin: r.landing(),
        age: 0.,
        amplitude: 0.,
        frequency: 1.,
        x: r.landing().0,
        y: r.landing().1,
        vx: 0.,
        vy: 0.,
        damage: 100,
        life: 2.,
    });
    r.bullets = lethal;
    r.advance(0.125, &c).unwrap();
    let dead = r.character(1).clone();
    assert!(!dead.alive);
    let carry = dead.resources_carry.unwrap();
    assert_eq!((carry.hp_remainder, carry.hp_roundoff), (0., 0.));
    assert!(carry.mp_remainder > 0.);
    r.advance(1., &c).unwrap();
    assert_eq!(r.character(1), &dead);
    assert!(r.handle(1, potion(), &c).unwrap().is_empty());

    let (c, mut lava) = setup();
    let i = lava
        .map
        .as_ref()
        .unwrap()
        .tiles()
        .iter()
        .position(|t| t.ground == 0x70)
        .unwrap();
    let (w, _) = lava.map.as_ref().unwrap().dimensions();
    let p = lava.character_mut(1);
    (p.x, p.y) = (
        (i % usize::from(w)) as f32 + 0.5,
        (i / usize::from(w)) as f32 + 0.5,
    );
    p.stats.insert(1, 1);
    p.resources_carry.as_mut().unwrap().hp_remainder = 0.125;
    p.resources_carry.as_mut().unwrap().hp_roundoff = 1e-20;
    lava.advance(0.5, &c).unwrap();
    assert!(!lava.character(1).alive);
    assert_eq!(lava.character(1).resources_carry.unwrap().hp_remainder, 0.);
    assert_eq!(lava.character(1).resources_carry.unwrap().hp_roundoff, 0.);

    let mut fixture = Room::new(100, 0, 2);
    let mut legacy = dead;
    legacy.alive = true;
    legacy.stats.extend([(1, 50), (4, 50)]);
    legacy.resources_version = 0;
    legacy.resources_carry = None;
    (legacy.x, legacy.y) = (16.5, 16.5);
    fixture.attach(1, legacy, &c).unwrap();
    fixture.entities.clear();
    fixture.next_wave = 999.;
    let before = fixture.character(1).clone();
    fixture.advance(1., &c).unwrap();
    assert_eq!(fixture.character(1), &before);
    let json = serde_json::to_string(&before).unwrap();
    assert!(!json.contains("resources_carry"));
    assert_eq!(serde_json::from_str::<Character>(&json).unwrap(), before);
}

#[test]
fn resources_runtime_unsupported_or_unsnapshotted_masks_and_invalid_elapsed_errors() {
    let (c, r) = setup();
    for mask in [None, Some(1), Some(2), Some(4096), Some(1 << 17), Some(-1)] {
        let mut bad = r.character(1).clone();
        if let Some(mask) = mask {
            bad.stats.insert(25, mask);
        } else {
            bad.stats.remove(&25);
        }
        let mut candidate = r.clone();
        candidate.detach(1);
        assert!(candidate.attach(1, bad.clone(), &c).is_err());
        let mut candidate = r.clone();
        candidate.players.get_mut(&1).unwrap().character = bad;
        let before = candidate.character(1).clone();
        assert!(candidate.advance(0.1, &c).is_err());
        assert_eq!(candidate.character(1), &before);
        assert!(candidate.handle(1, potion(), &c).is_err());
        assert_eq!(candidate.character(1), &before);
    }
    for elapsed in [-1., f64::NAN, f64::INFINITY, f64::MAX] {
        let mut candidate = r.clone();
        assert!(candidate.advance(elapsed, &c).is_err());
        assert_eq!(candidate.elapsed, r.elapsed);
        assert_eq!(candidate.character(1), r.character(1));
    }
}

#[test]
fn resources_runtime_all_seven_ordinary_rates_and_non_saturating_potion_preserve_low_words() {
    let (mut c, r) = setup();
    for (kind, hp_regen, mp_regen) in [
        (0x300, 15., 10.),
        (0x307, 50., 10.),
        (0x30e, 12., 12.),
        (0x310, 10., 15.),
        (0x31d, 10., 10.),
        (0x31e, 10., 10.),
        (0x31f, 10., 10.),
    ] {
        let mut candidate = r.clone();
        candidate.detach(1);
        let mut p = Character::new(&c, kind, "Rates".into(), 1).unwrap();
        (p.x, p.y) = candidate.landing();
        p.stats.extend([(1, 50), (4, 50)]);
        candidate.attach(1, p, &c).unwrap();
        candidate.advance(0.5, &c).unwrap();
        let hp_delta: f64 = (1. + hp_regen / 8.) * 0.5;
        let mp_delta: f64 = (0.8 + mp_regen / 16.7) * 0.5;
        assert_eq!(
            candidate.character(1).stats[&1],
            50 + hp_delta.trunc() as i32
        );
        assert_eq!(
            candidate.character(1).stats[&4],
            50 + mp_delta.trunc() as i32
        );
        let carry = candidate.character(1).resources_carry.unwrap();
        assert_eq!(carry.hp_remainder, hp_delta.fract());
        assert_eq!(carry.mp_remainder, mp_delta.fract());
        assert_eq!((carry.hp_roundoff, carry.mp_roundoff), (0., 0.));
    }
    let mut r = r;
    for _ in 0..3 {
        r.advance(0.1, &c).unwrap();
    }
    let before = carry_bits(r.character(1));
    assert_ne!(before[2], 0);
    assert_ne!(before[3], 0);
    r.character_mut(1).stats.insert(12, 0xa22);
    c.objects.get_mut(&0xa22).unwrap().activations[0]
        .attributes
        .insert("amount".into(), "1".into());
    let hp = r.character(1).stats[&1];
    r.handle(1, potion(), &c).unwrap();
    assert_eq!(r.character(1).stats[&1], hp + 1);
    assert_eq!(carry_bits(r.character(1)), before);
    // A max-HP increase and a non-clamping removal preserve both low words too.
    r.character_mut(1).stats.insert(13, 0xa27);
    r.handle(1, swap((3, -1), (5, 0xa27)), &c).unwrap();
    assert_eq!(carry_bits(r.character(1)), before);
    r.handle(1, swap((3, 0xa27), (5, -1)), &c).unwrap();
    assert_eq!(carry_bits(r.character(1)), before);
    assert_eq!(r.character(1).stats[&1], hp + 1);
}
