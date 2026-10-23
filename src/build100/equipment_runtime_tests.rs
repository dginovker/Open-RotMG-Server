use super::*;
use crate::{content::Grounds, realm_map};
use std::{path::PathBuf, time::Duration};

fn content(repaired: bool) -> (Content, Arc<Grounds>) {
    let variable = if repaired {
        "BUILD100_CONTENT_REPAIRED_DIR"
    } else {
        "BUILD100_CONTENT_ORIGINAL_DIR"
    };
    let path =
        PathBuf::from(std::env::var_os(variable).unwrap_or_else(|| panic!("{variable} required")));
    (
        Content::load(&path).unwrap(),
        Arc::new(Grounds::load(&path).unwrap()),
    )
}
fn room(content: &Content, grounds: Arc<Grounds>, tutorial: bool) -> Room {
    let map = if tutorial {
        realm_map::tutorial(100, content, grounds)
    } else {
        realm_map::realm(100, content, grounds)
    }
    .unwrap();
    Room::from_map(Arc::new(map), content, 1_000_000).unwrap()
}
fn attach(room: &mut Room, content: &Content, id: i32, kind: i16) {
    let mut p = Character::new(content, kind, format!("Avatar{id}"), 1).unwrap();
    (p.x, p.y) = room.landing();
    room.attach(id, p, content).unwrap();
}
fn swap(a: (i32, u8, i16), b: (i32, u8, i16)) -> Packet {
    packet(14, |w| {
        w.i32(a.0).u8(a.1).i16(a.2).i32(b.0).u8(b.1).i16(b.2);
    })
}
fn use_potion() -> Packet {
    packet(15, |w| {
        w.i32(1).u8(4).i32(-1).f32(4.5).f32(5.5);
    })
}
fn status_stats(r: &mut Reader) -> (i32, BTreeMap<u8, i32>) {
    let id = r.i32().unwrap();
    r.f32().unwrap();
    r.f32().unwrap();
    let count = r.i16().unwrap();
    (
        id,
        (0..count)
            .map(|_| (r.u8().unwrap(), r.i32().unwrap()))
            .collect(),
    )
}
fn projected(packets: &[Packet], packet_id: u8, id: i32) -> BTreeMap<u8, i32> {
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
            let (got, stats) = status_stats(&mut r);
            if got == id {
                return stats;
            }
        }
    }
    panic!("missing packet {packet_id} avatar {id}")
}

#[test]
fn equipment_runtime_all_seven_original_repaired_starting_loadouts_and_ring_projection() {
    // Root ActivateOnEquip: Worn Cloak10→2, Old Helm500→2, Wooden Shield2; bow's nested ATK5 is inert.
    let classes = [
        (0x300, [150, 100, 10, 0, 15, 15, 10, 15], 10, 2),
        (0x307, [130, 100, 12, 0, 12, 50, 10, 12], 0, 0),
        (0x30e, [100, 100, 12, 0, 10, 12, 12, 15], 0, 0),
        (0x310, [100, 100, 10, 0, 12, 10, 15, 12], 0, 0),
        (0x31d, [200, 100, 15, 0, 7, 10, 10, 10], 500, 2),
        (0x31e, [200, 100, 15, 0, 7, 10, 10, 10], 2, 2),
        (0x31f, [200, 100, 12, 0, 7, 10, 10, 10], 0, 0),
    ];
    for repaired in [false, true] {
        let (c, g) = content(repaired);
        for (kind, values, original_def, repaired_def) in classes {
            let mut r = room(&c, g.clone(), true);
            attach(&mut r, &c, 1, kind);
            let permanent = r.character(1).clone();
            for (id, value) in [0, 3, 20, 21, 22, 26, 27, 28].into_iter().zip(values) {
                assert_eq!(permanent.stats[&id], value);
            }
            let mut expected = permanent.stats.clone();
            // Original Gs._C8 reads conditions from wire29, not durable25.
            assert_eq!(expected.remove(&25), Some(0));
            expected.insert(29, 0);
            expected.insert(21, if repaired { repaired_def } else { original_def });
            assert_eq!(projected(&[r.arrival(1, &c).unwrap()], 10, 1), expected);
            let initial = r
                .publication(
                    1,
                    &[packet(1, |w| {
                        w.i32(1);
                    })],
                    &c,
                )
                .unwrap();
            assert_eq!(projected(&initial, 10, 1), expected);
            assert_eq!(projected(&initial, 13, 1), expected);
            assert_eq!(r.character(1), &permanent);
            r.character_mut(1).stats.insert(11, 0xa4f);
            expected.insert(11, 0xa4f);
            if repaired {
                *expected.get_mut(&21).unwrap() += 1;
            } else {
                *expected.get_mut(&20).unwrap() += 5;
            }
            assert_eq!(projected(&[r.arrival(1, &c).unwrap()], 10, 1), expected);
            assert_eq!(r.character(1).stats[&20], values[2]);
            assert_eq!(r.character(1).stats[&21], 0);
            let mut fixture = Room::new(100, 0, 2);
            fixture.attach(1, r.character(1).clone(), &c).unwrap();
            assert_eq!(
                projected(&[fixture.arrival(1, &c).unwrap()], 10, 1),
                r.character(1).stats
            );
        }
    }
}

#[test]
fn equipment_runtime_root_backpack_all_bonus_stats_self_peer_interest_and_feedback_guard() {
    let (c, g) = content(false);
    let mut r = room(&c, g, true);
    attach(&mut r, &c, 1, 0x30e);
    attach(&mut r, &c, 2, 0x31d);
    r.character_mut(1).stats.insert(10, 0xa5f); // Robe of the Invoker: ATK5 DEF4 MP40.
    r.character_mut(1).stats.insert(11, 0x708); // Spider's Eye Ring: HpRegen3 MpRegen3.
    r.character_mut(1).stats.insert(13, 0xa66); // Backpack Old Helm contributes no DEF500.
    let before = r.character(1).clone();
    let mut expected = before.stats.clone();
    assert_eq!(expected.remove(&25), Some(0));
    expected.insert(29, 0);
    expected.extend([(3, 140), (20, 17), (21, 4), (26, 15), (27, 15)]);
    for viewer in [1, 2] {
        let out = r.publication(viewer, &[r.tick(&c).unwrap()], &c).unwrap();
        assert_eq!(projected(&out, 10, 1), expected);
        assert_eq!(projected(&out, 13, 1), expected);
        assert!(
            !r.publication(viewer, &[], &c)
                .unwrap()
                .iter()
                .any(|p| p.id == 13)
        );
    }
    let events = r
        .handle(
            1,
            packet(5, |w| {
                w.f32(before.x).f32(before.y);
            }),
            &c,
        )
        .unwrap();
    assert!(events.is_empty());
    assert!(
        !r.publication(1, &events, &c)
            .unwrap()
            .iter()
            .any(|p| p.id == 13)
    );
    r.character_mut(2).x = 30.5;
    r.character_mut(2).y = 1.5;
    assert!(
        r.publication(2, &[], &c)
            .unwrap()
            .iter()
            .any(|p| p.id == 11 && Reader::new(&p.body).i32().unwrap() == 1)
    );
    (r.character_mut(2).x, r.character_mut(2).y) = (before.x, before.y);
    assert_eq!(
        projected(&r.publication(2, &[], &c).unwrap(), 10, 1),
        expected
    );
    assert_eq!(r.character(1), &before);

    // Actual Iron Quiver DEX2 publishes, but this seam does not execute cadence or regen.
    r.detach(1);
    attach(&mut r, &c, 1, 0x307);
    r.character_mut(1).stats.insert(9, 0xa63);
    assert_eq!(projected(&[r.arrival(1, &c).unwrap()], 10, 1)[&28], 14);
    assert_eq!(projected(&[r.arrival(1, &c).unwrap()], 10, 1)[&20], 12);
}

#[test]
fn equipment_runtime_maxima_no_free_vitals_potion_clamps_bag_swaps_and_reconnect_no_stacking() {
    let (c, g) = content(false);
    let mut r = room(&c, g, true);
    attach(&mut r, &c, 1, 0x30e);
    let base = r.character(1).stats.clone();
    r.character_mut(1).stats.extend([(13, 0xa27), (14, 0xa5f)]);
    r.handle(1, swap((1, 5, 0xa27), (1, 3, -1)), &c).unwrap();
    r.handle(1, swap((1, 6, 0xa5f), (1, 2, -1)), &c).unwrap();
    assert_eq!(
        (r.character(1).stats[&1], r.character(1).stats[&4]),
        (100, 100)
    );
    assert_eq!(
        (r.character(1).stats[&0], r.character(1).stats[&3]),
        (100, 100)
    );
    r.handle(1, use_potion(), &c).unwrap();
    assert_eq!(
        (r.character(1).stats[&1], r.character(1).stats[&12]),
        (140, -1)
    );
    r.character_mut(1).stats.insert(4, 135);
    let saved: Character =
        serde_json::from_str(&serde_json::to_string(r.character(1)).unwrap()).unwrap();
    r.detach(1);
    r.attach(1, saved, &c).unwrap();
    assert_eq!(r.character(1).stats[&1], 140); // Valid saved HP may exceed the permanent maximum.
    r.advance(1.01, &c).unwrap();
    let bag = r
        .entities
        .values()
        .find(|e| e.kind == 0x806)
        .unwrap()
        .clone();
    (r.character_mut(1).x, r.character_mut(1).y) = (bag.x, bag.y);
    let original_ring = bag.stats[&8] as i16;
    r.handle(1, swap((1, 3, 0xa27), (bag.id, 0, original_ring)), &c)
        .unwrap();
    assert_eq!(r.character(1).stats[&1], 100);
    assert_eq!(r.entities[&bag.id].stats[&8], 0xa27);
    r.handle(1, swap((1, 2, 0xa5f), (1, 6, -1)), &c).unwrap();
    assert_eq!(r.character(1).stats[&4], 100);
    for _ in 0..8 {
        r.handle(1, swap((1, 3, original_ring), (bag.id, 0, 0xa27)), &c)
            .unwrap();
        assert_eq!(r.character(1).stats[&1], 100);
        let saved = serde_json::to_string(r.character(1)).unwrap();
        r.detach(1);
        r.attach(1, serde_json::from_str(&saved).unwrap(), &c)
            .unwrap();
        let projection = projected(&[r.arrival(1, &c).unwrap()], 10, 1);
        assert_eq!((projection[&0], projection[&20]), (140, 12));
        r.handle(1, swap((1, 3, 0xa27), (bag.id, 0, original_ring)), &c)
            .unwrap();
        assert_eq!(r.entities[&bag.id].stats[&8], 0xa27);
    }
    for id in [0, 3, 20, 21, 22, 26, 27, 28, 5, 6, 7] {
        assert_eq!(r.character(1).stats[&id], base[&id]);
    }
    r.character_mut(1).alive = false;
    r.character_mut(1).stats.insert(1, 0);
    assert!(r.handle(1, use_potion(), &c).unwrap().is_empty());
    assert_eq!(r.character(1).stats[&1], 0);
}

#[test]
fn equipment_runtime_malformed_attach_and_swap_fail_without_partial_inventory_or_clamp() {
    let (c, g) = content(false);
    let r = room(&c, g, true);
    let p = Character::new(&c, 0x30e, "Invalid".into(), 1).unwrap();
    let reject = |c: &Content, bad: Character| {
        let mut candidate = r.clone();
        assert!(candidate.attach(1, bad, c).is_err());
        assert!(candidate.players.is_empty());
    };
    let mut missing_attack = p.clone();
    missing_attack.stats.remove(&20);
    let error = r
        .clone()
        .attach(1, missing_attack, &c)
        .err()
        .expect("missing ATK accepted");
    let error = format!("{error:#}");
    assert!(error.contains("avatar Invalid class 0x30e generation 1 equipment stats"));
    assert!(error.contains("missing permanent stat 20"));
    for id in [0, 3, 20, 21, 22, 26, 27, 28] {
        let mut bad = p.clone();
        bad.stats.remove(&id);
        reject(&c, bad);
        let mut bad = p.clone();
        bad.stats.insert(id, -1);
        reject(&c, bad);
    }
    for (id, value) in [
        (0, 0),
        (1, 101),
        (1, 0),
        (1, -1),
        (4, 101),
        (4, -1),
        (8, -2),
        (13, 40000),
        (13, 0x7fff),
        (13, 0x30e),
        (9, 0xa66),
    ] {
        let mut bad = p.clone();
        bad.stats.insert(id, value);
        reject(&c, bad);
    }
    for id in 8..20 {
        let mut bad = p.clone();
        bad.stats.remove(&id);
        reject(&c, bad);
    }
    let mut bad = p.clone();
    bad.alive = false;
    reject(&c, bad);
    let mut bad = p.clone();
    bad.kind = 0x600;
    reject(&c, bad);
    for slots in [vec![17; 4], vec![17; 12], vec![0; 12]] {
        let mut altered = Content {
            objects: c.objects.clone(),
        };
        altered.objects.get_mut(&0x30e).unwrap().slots = slots;
        reject(&altered, p.clone());
    }
    for slot_type in [None, Some(99)] {
        let mut altered = Content {
            objects: c.objects.clone(),
        };
        altered.objects.get_mut(&0xa97).unwrap().slot_type = slot_type;
        reject(&altered, p.clone());
    }
    let mut bad = p.clone();
    bad.stats.extend([(11, 0xa27), (0, i32::MAX)]);
    reject(&c, bad);
    let mut altered = Content {
        objects: c.objects.clone(),
    };
    altered
        .objects
        .get_mut(&0xa97)
        .unwrap()
        .bonuses
        .push((99, 1));
    reject(&altered, p.clone());
    let mut r = r;
    r.attach(1, p, &c).unwrap();
    r.character_mut(1)
        .stats
        .extend([(13, 0xa27), (0, i32::MAX)]);
    let before = r.character(1).clone();
    assert!(r.handle(1, swap((1, 3, -1), (1, 5, 0xa27)), &c).is_err());
    assert_eq!(r.character(1), &before);
    // Overflow and malformed HP are rejected, not silently healed/clamped by a later equip.
    r.character_mut(1).stats.extend([(0, 100), (1, 101)]);
    let before = r.character(1).clone();
    assert!(r.handle(1, swap((1, 3, -1), (1, 5, 0xa27)), &c).is_err());
    assert_eq!(r.character(1), &before);
    r.character_mut(1).stats.extend([(1, 99), (13, 0xa27)]);
    let mut altered = Content {
        objects: c.objects.clone(),
    };
    altered.objects.get_mut(&0xa22).unwrap().activations[0]
        .attributes
        .insert("amount".into(), i32::MAX.to_string());
    let before = r.character(1).clone();
    assert!(r.handle(1, use_potion(), &altered).is_err());
    assert_eq!(r.character(1), &before);
}

#[test]
fn equipment_runtime_equipped_speed_drives_elapsed_allowance_and_bounded_credit() {
    let (c, g) = content(false);
    let mut equipped = room(&c, g, true);
    attach(&mut equipped, &c, 1, 0x30e);
    equipped.character_mut(1).stats.insert(11, 0xa2b); // +5 SPD, not capped at base75.
    let mut backpack = equipped.clone();
    backpack
        .character_mut(1)
        .stats
        .extend([(11, -1), (13, 0xa2b)]);
    for r in [&mut equipped, &mut backpack] {
        let a = r.players.get_mut(&1).unwrap();
        (a.character.x, a.character.y) = (4.5, 5.5);
        a.move_credit = 0.;
    }
    let movement = || {
        packet(5, |w| {
            w.f32(9.45).f32(5.5);
        })
    };
    backpack.players.get_mut(&1).unwrap().move_at = Instant::now() - Duration::from_secs(1);
    assert!(backpack.handle(1, movement(), &c).is_err());
    equipped.players.get_mut(&1).unwrap().move_at = Instant::now() - Duration::from_secs(1);
    equipped.handle(1, movement(), &c).unwrap();
    assert_eq!(equipped.character(1).x, 9.45);
    assert!((0.16..0.5).contains(&equipped.players[&1].move_credit));
    equipped.players.get_mut(&1).unwrap().move_at = Instant::now() - Duration::from_secs(1);
    equipped.handle(1, movement(), &c).unwrap();
    assert_eq!(equipped.players[&1].move_credit, 0.5);
    equipped.character_mut(1).stats.insert(22, 75);
    assert_eq!(
        projected(&[equipped.arrival(1, &c).unwrap()], 10, 1)[&22],
        80
    );
    assert_eq!(equipped.character(1).stats[&22], 75);
}

#[test]
fn equipment_runtime_attack_defense_projectile_effects_and_lava_bypass_fixture_isolation() {
    for repaired in [false, true] {
        let (c, g) = content(repaired);
        let mut r = room(&c, g.clone(), true);
        attach(&mut r, &c, 1, 0x30e);
        r.character_mut(1).stats.insert(11, 0xa29); // Ring of Greater Attack: root ATK5.
        let mut backpack = r.clone();
        backpack
            .character_mut(1)
            .stats
            .extend([(11, -1), (13, 0xa29)]);
        let weapon = c.get(0xa97).unwrap();
        let proj = weapon.normal_projectile(0).unwrap();
        let visual = c.named(&proj.visual).unwrap().id as u8;
        let random: f64 = r.rng.clone().gen_range(0.0..1.0);
        let root = 10.0 + 15.0 * random;
        let p = r.character(1).clone();
        let fire = || {
            packet(8, |w| {
                w.u8(7)
                    .i32(1)
                    .u8(visual)
                    .i16(0xa97)
                    .f32(p.x)
                    .f32(p.y)
                    .f32(0.)
                    .i16(32767);
            })
        };
        let mut second = fire();
        second.body[0] = 8;
        let mut second_backpack = fire();
        second_backpack.body[0] = 8;
        r.handle(1, fire(), &c).unwrap();
        backpack.handle(1, fire(), &c).unwrap();
        r.handle(1, second, &c).unwrap();
        backpack.handle(1, second_backpack, &c).unwrap();
        assert_eq!(
            r.bullets[0].damage,
            (root * (0.5 + 17.0 / 75.0 * 1.5)) as i32
        );
        assert_eq!(
            backpack.bullets[0].damage,
            (root * (0.5 + 12.0 / 75.0 * 1.5)) as i32
        );
        assert!(r.bullets[0].damage > backpack.bullets[0].damage);
        assert_eq!(
            (
                r.bullets[0].id,
                r.bullets[0].amplitude,
                r.bullets[0].frequency
            ),
            (7, 0.5, 2.)
        );
        let mut r = room(&c, g.clone(), true);
        attach(&mut r, &c, 1, 0x31d);
        r.entities.clear();
        let p = r.character(1);
        let bullet = Bullet {
            victims: BTreeSet::new(),
            flight: tests::normal_flight(0.1, 2.),
            owner: 999,
            id: 8,
            origin: (p.x, p.y),
            age: 0.,
            amplitude: 0.,
            frequency: 1.,
            x: p.x,
            y: p.y,
            vx: 0.,
            vy: 0.,
            damage: 100,
            life: 1.,
        };
        r.bullets.push(bullet.clone());
        let mut backpack = r.clone();
        backpack
            .character_mut(1)
            .stats
            .extend([(9, -1), (13, 0xa66)]);
        for test in [&mut r, &mut backpack] {
            test.advance(0.05, &c).unwrap();
        }
        assert_eq!(r.character(1).stats[&1], if repaired { 102 } else { 175 });
        assert_eq!(backpack.character(1).stats[&1], 100);
        assert_eq!(r.character(1).stats[&21], 0);
        let mut fixture = Room::new(100, 0, 2);
        let mut p = Character::new(&c, 0x31d, "Fixture".into(), 1).unwrap();
        (p.x, p.y) = bullet.origin;
        fixture.attach(1, p, &c).unwrap();
        fixture.entities.clear();
        fixture.next_wave = 999.;
        fixture.bullets.push(Bullet {
            flight: Flight::Arena,
            ..bullet
        });
        fixture.advance(0.05, &c).unwrap();
        assert_eq!(fixture.character(1).stats[&1], 100);

        let mut lava = room(&c, g, false);
        attach(&mut lava, &c, 1, 0x31d);
        let i = lava
            .map
            .as_ref()
            .unwrap()
            .tiles()
            .iter()
            .position(|t| t.ground == 0x70)
            .unwrap();
        (lava.character_mut(1).x, lava.character_mut(1).y) =
            ((i % 256) as f32 + 0.5, (i / 256) as f32 + 0.5);
        let mut backpack = lava.clone();
        backpack
            .character_mut(1)
            .stats
            .extend([(9, -1), (13, 0xa66)]);
        lava.advance(0.5, &c).unwrap();
        backpack.advance(0.5, &c).unwrap();
        assert_eq!(lava.character(1).stats[&1], backpack.character(1).stats[&1]);
        assert!((30..=60).contains(&(200 - lava.character(1).stats[&1])));
    }
}
