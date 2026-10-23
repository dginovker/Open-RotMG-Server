use super::*;
use crate::{content::Grounds, realm_map};
use std::time::Duration;

fn room(tutorial: bool) -> (Content, Room) {
    let path = std::env::var_os("BUILD100_CLIENT_DIR")
        .expect("realm_entry requires qualified BUILD100_CLIENT_DIR");
    let path = std::path::Path::new(&path);
    let content = Content::load(path).unwrap();
    let grounds = Arc::new(Grounds::load(path).unwrap());
    let map = if tutorial {
        realm_map::tutorial(100, &content, grounds)
    } else {
        realm_map::realm(100, &content, grounds)
    }
    .unwrap();
    let mut p = Character::new(&content, 0x30e, "Entry".into(), 1).unwrap();
    p.x = map.spawn.0;
    p.y = map.spawn.1;
    let mut room = Room::from_map(Arc::new(map), &content, 1_000_000).unwrap();
    room.attach(1, p, &content).unwrap();
    (content, room)
}
fn movement(x: f32, y: f32) -> Packet {
    packet(5, |w| {
        w.f32(x).f32(y);
    })
}
fn position(room: &mut Room, x: f32, y: f32) {
    let a = room.players.get_mut(&1).unwrap();
    a.character.x = x;
    a.character.y = y;
    a.move_at = Instant::now() - Duration::from_secs(1);
    a.move_credit = 0.5;
}
#[test]
fn realm_entry_elapsed_speed_no_tolerance_bank_collision_sweep_and_axis_slide() {
    let (c, mut r) = room(true);
    position(&mut r, 4.5, 5.5);
    r.players.get_mut(&1).unwrap().move_at = Instant::now() - Duration::from_millis(100);
    r.handle(1, movement(5.3, 5.5), &c).unwrap();
    assert!(r.handle(1, movement(5.8, 5.5), &c).is_err());
    let mut malformed = movement(1., 5.5);
    malformed.body[..4].copy_from_slice(&f32::NAN.to_be_bytes());
    assert!(r.handle(1, malformed, &c).is_err());
    assert!(r.handle(1, movement(32., 5.5), &c).is_err());
    position(&mut r, 12.5, 5.5);
    r.handle(1, movement(14.5, 5.5), &c).unwrap();
    assert_eq!((r.character(1).x, r.character(1).y), (12.5, 5.5));
    position(&mut r, 12.5, 5.5);
    r.handle(1, movement(13.5, 5.8), &c).unwrap();
    assert_eq!((r.character(1).x, r.character(1).y), (12.5, 5.8));
    // July water/snow/ice do not alter elapsed input speed.
    let (c, r) = room(false);
    let map = r.map.as_ref().unwrap();
    for kind in [0x72, 0x73, 0x6b] {
        let (i, _) = map
            .tiles()
            .iter()
            .enumerate()
            .find(|(i, t)| {
                t.ground == kind
                    && map.can_move(
                        ((*i % 256) as f32 + 0.2, (*i / 256) as f32 + 0.2),
                        ((*i % 256) as f32 + 0.4, (*i / 256) as f32 + 0.2),
                    )
            })
            .unwrap();
        let (x, y) = ((i % 256) as f32 + 0.2, (i / 256) as f32 + 0.2);
        let mut test = r.clone();
        position(&mut test, x, y);
        test.players.get_mut(&1).unwrap().move_at = Instant::now();
        test.handle(1, movement(x + 0.2, y), &c).unwrap();
    }
}
#[test]
fn realm_entry_lava_500ms_authority_no_defense_or_double_advisory_and_once_death() {
    let (c, mut r) = room(false);
    let map = r.map.as_ref().unwrap();
    let i = map.tiles().iter().position(|t| t.ground == 0x70).unwrap();
    position(&mut r, (i % 256) as f32 + 0.5, (i / 256) as f32 + 0.5);
    r.character_mut(1).stats.insert(21, 50_000);
    let hp = r.character(1).stats[&1];
    r.publication(1, &[], &c).unwrap();
    r.advance(0.49, &c).unwrap();
    assert_eq!(r.character(1).stats[&1], hp);
    r.advance(0.02, &c).unwrap();
    assert!((30..=60).contains(&(hp - r.character(1).stats[&1])));
    let hp = r.character(1).stats[&1];
    r.handle(1, damage_packet(1, 0, 0, 60), &c).unwrap();
    assert_eq!(r.character(1).stats[&1], hp);
    r.character_mut(1).stats.insert(1, 1);
    let events = r.advance(0.5, &c).unwrap();
    assert!(!r.character(1).alive);
    let out = r.publication(1, &events, &c).unwrap();
    assert!(
        !out.iter().any(|p| p.id == 9),
        "local lava animation must not receive a second damage echo"
    );
    assert_eq!(
        out.iter()
            .filter(|p| p.id == 11 && Reader::new(&p.body).i32().unwrap() == 1)
            .count(),
        1
    );
    let out = r.publication(1, &[], &c).unwrap();
    assert!(
        !out.iter()
            .any(|p| p.id == 11 && Reader::new(&p.body).i32().unwrap() == 1)
    );
    r.advance(2., &c).unwrap();
    assert_eq!(r.character(1).stats[&1], 0);
}
#[test]
fn realm_entry_real_tutorial_guns_chickens_and_embedded_bag_pickup_once_per_item() {
    let (c, mut r) = room(true);
    position(&mut r, 16.5, 13.5);
    let hp = r.character(1).stats[&1];
    let mut shots = BTreeMap::new();
    for _ in 0..110 {
        for p in r.advance(0.05, &c).unwrap() {
            if p.id == 8 {
                let mut read = Reader::new(&p.body);
                read.u8().unwrap();
                let owner = read.i32().unwrap();
                assert_eq!(read.u8().unwrap(), 0);
                let kind = read.i16().unwrap();
                read.f32().unwrap();
                read.f32().unwrap();
                let angle = read.f32().unwrap();
                assert!((1_000_000..1 << 24).contains(&owner));
                shots.insert(kind, angle);
            }
        }
    }
    for (kind, degrees) in [(0x802, 90_f32), (0x803, 270.), (0x804, 0.), (0x805, 180.)] {
        assert!((shots[&kind] - degrees.to_radians()).abs() < 0.0001);
    }
    assert!(
        r.character(1).stats[&1] < hp,
        "real gun projectiles must damage player"
    );
    let chicken = r.entities.values().find(|e| e.kind == 0x801).unwrap().id;
    r.entities.remove(&chicken);
    r.advance(1.1, &c).unwrap();
    assert_eq!(r.entities.values().filter(|e| e.kind == 0x801).count(), 2);
    assert_eq!(r.character(1).stats[&6], 0);
    let bag = r
        .entities
        .values()
        .find(|e| e.kind == 0x806)
        .unwrap()
        .clone();
    assert_eq!(
        bag.stats[&8],
        c.named("Tutorial Loot Bag").unwrap().equipment[0]
    );
    position(&mut r, bag.x - 0.5, bag.y);
    let slot = (4..12)
        .find(|s| r.character(1).stats[&(8 + s)] == -1)
        .unwrap();
    let swap = || {
        packet(14, |w| {
            w.i32(bag.id)
                .u8(0)
                .i16(bag.stats[&8] as i16)
                .i32(1)
                .u8(slot)
                .i16(-1);
        })
    };
    r.handle(1, swap(), &c).unwrap();
    assert!(r.handle(1, swap(), &c).is_err());
    assert_eq!(r.character(1).stats[&(8 + slot)], bag.stats[&8]);
    r.advance(1.1, &c).unwrap();
    let bags: Vec<_> = r.entities.values().filter(|e| e.kind == 0x806).collect();
    assert_eq!(bags.len(), 1);
    assert_ne!(bags[0].id, bag.id);
    assert!(!r.character(1).tutorial_complete);
    assert_eq!(r.character(1).stats[&6], 0);
}
#[test]
fn realm_entry_interest_makes_before_shoot_tick_drop_and_reentry_without_unknown_ids() {
    let (c, mut r) = room(false);
    let mut known = BTreeSet::new();
    let mut ground = BTreeSet::new();
    for pos in [r.landing(), (81., 190.5), (112.8, 160.5), r.landing()] {
        position(&mut r, pos.0, pos.1);
        let out = r.publication(1, &[], &c).unwrap();
        assert!(out.iter().filter(|p| p.id == 12).count() <= 1089);
        for p in out {
            let mut read = Reader::new(&p.body);
            match p.id {
                12 => {
                    let x = read.i16().unwrap();
                    let y = read.i16().unwrap();
                    assert!(ground.insert((x, y)));
                }
                10 => {
                    read.i16().unwrap();
                    read.utf().unwrap();
                    assert!(known.insert(read.i32().unwrap()));
                }
                11 => {
                    assert!(known.remove(&read.i32().unwrap()));
                }
                13 => {
                    let n = read.i16().unwrap();
                    for _ in 0..n {
                        let id = read.i32().unwrap();
                        assert!(known.contains(&id));
                        read.f32().unwrap();
                        read.f32().unwrap();
                        let n = read.i16().unwrap();
                        for _ in 0..n {
                            read.u8().unwrap();
                            read.i32().unwrap();
                        }
                    }
                }
                _ => panic!("unexpected {}", p.id),
            }
        }
    }
    assert!(ground.len() < 65536);
    assert!(known.contains(&1));
    let (c, mut tutorial) = room(true);
    position(&mut tutorial, 16.5, 13.5);
    let events = tutorial.advance(3.01, &c).unwrap();
    let out = tutorial.publication(1, &events, &c).unwrap();
    let mut known = BTreeSet::new();
    for p in out {
        let mut r = Reader::new(&p.body);
        if p.id == 10 {
            r.i16().unwrap();
            r.utf().unwrap();
            known.insert(r.i32().unwrap());
        }
        if p.id == 8 {
            r.u8().unwrap();
            assert!(known.contains(&r.i32().unwrap()));
        }
    }
}

#[test]
fn realm_entry_projectile_cover_static_rock_stops_staff_and_protects_target_beyond() {
    for dt in [0.05, 0.1] {
        for cover in [true, false] {
            let (c, mut r) = room(true);
            position(&mut r, 23.5, 23.5);
            let generator = r.entities.values().find(|e| e.kind == 0x807).unwrap().id;
            // Non-Static Tutorial generators do not register Square.obj_; use supplied Static cover.
            r.entities.get_mut(&generator).unwrap().kind = c.named("Rock Grey").unwrap().id;
            if !cover {
                r.entities.remove(&generator);
            }
            let chicken = r.entities.values_mut().find(|e| e.kind == 0x801).unwrap();
            let target = chicken.id;
            chicken.x = 25.1;
            chicken.y = 23.975529;
            let hp = chicken.stats[&1];
            let weapon = r.character(1).stats[&8] as i16;
            let visual = c
                .named(&c.get(weapon).unwrap().arena_projectile(0).unwrap().visual)
                .unwrap()
                .id;
            for bullet_id in [2, 3] {
                r.handle(
                    1,
                    packet(8, |w| {
                        w.u8(bullet_id)
                            .i32(1)
                            .u8(visual as u8)
                            .i16(weapon)
                            .f32(23.8)
                            .f32(23.5)
                            .f32(0.)
                            .i16(32767);
                    }),
                    &c,
                )
                .unwrap();
            }
            r.advance(dt, &c).unwrap();
            if cover {
                assert!(
                    r.bullets.iter().all(|b| b.owner != 1),
                    "staff must stop in/crossing the static rock cell; dt={dt}"
                );
                assert!(
                    !r.entities[&generator].stats.contains_key(&1),
                    "scenery must not receive HP damage"
                );
            }
            if dt < 0.1 {
                r.advance(0.05, &c).unwrap();
            }
            if cover {
                assert_eq!(
                    r.entities[&target].stats[&1], hp,
                    "target beyond static rock must remain unharmed; dt={dt}"
                );
            } else {
                assert!(
                    !r.entities.contains_key(&target),
                    "the same unobstructed staff shot must reach the target; dt={dt}"
                );
            }
        }
    }
}

#[test]
fn realm_entry_projectile_cover_guns_leave_their_own_occupied_origin() {
    let (c, mut r) = room(true);
    position(&mut r, 16.5, 13.5);
    for _ in 0..61 {
        r.advance(0.05, &c).unwrap();
    }
    let owners: BTreeSet<_> = r
        .entities
        .values()
        .filter(|e| (0x802..=0x805).contains(&e.kind))
        .map(|e| e.id)
        .collect();
    assert_eq!(owners.len(), 4);
    for owner in &owners {
        let shot = r
            .bullets
            .iter()
            .find(|b| b.owner == *owner)
            .expect("gun shot must survive inside its own origin cell");
        let gun = &r.entities[owner];
        assert_eq!(
            (shot.x.floor(), shot.y.floor()),
            (gun.x.floor(), gun.y.floor())
        );
    }
    // Keep the exclusion covered even though supplied Tutorial guns are themselves non-Static.
    let owner = *owners.first().unwrap();
    r.entities.get_mut(&owner).unwrap().kind = c.named("Grey Wall").unwrap().id;
    let shot = r.bullets.iter().find(|b| b.owner == owner).unwrap();
    assert!(
        projectile_cover_entry(&r.entities, &c, shot, shot.origin)
            .unwrap()
            .is_none()
    );
    r.advance(0.15, &c).unwrap();
    for owner in owners {
        let shot = r
            .bullets
            .iter()
            .find(|b| b.owner == owner)
            .expect("gun shot must leave its own cover");
        let gun = &r.entities[&owner];
        assert_ne!(
            (shot.x.floor(), shot.y.floor()),
            (gun.x.floor(), gun.y.floor())
        );
    }
}

#[test]
fn realm_entry_projectile_cover_stops_enemy_shots_at_static_rock_not_nowalk_ground() {
    let (c, mut r) = room(true);
    position(&mut r, 9.2, 21.5);
    let hole = r.entities.values().find(|e| e.kind == 0x800).unwrap().id;
    r.entities.get_mut(&hole).unwrap().kind = c.named("Rock Grey").unwrap().id;
    let owner = r.entities.values().find(|e| e.kind == 0x804).unwrap().id;
    let hp = r.character(1).stats[&1];
    let shot = Bullet {
        victims: BTreeSet::new(),
        flight: tests::normal_flight(14., 1.),
        owner,
        id: 250,
        origin: (7.8, 21.5),
        age: 0.,
        amplitude: 0.,
        frequency: 1.,
        x: 7.8,
        y: 21.5,
        vx: 14.,
        vy: 0.,
        damage: 5,
        life: 1.,
    };
    let mut unobstructed = r.clone();
    unobstructed.entities.remove(&hole);
    for room in [&mut r, &mut unobstructed] {
        room.bullets.push(shot.clone());
        room.advance(0.1, &c).unwrap();
    }
    assert!(r.bullets.iter().all(|b| b.id != 250));
    assert_eq!(r.character(1).stats[&1], hp);
    assert_eq!(unobstructed.character(1).stats[&1], hp - 5);

    let (c, mut r) = room(false);
    assert!(r.map.as_ref().unwrap().ground(0, 0).unwrap().no_walk);
    r.bullets.push(Bullet {
        victims: BTreeSet::new(),
        flight: tests::normal_flight(1., 1.),
        owner: 1,
        id: 251,
        origin: (0.2, 0.5),
        age: 0.,
        amplitude: 0.,
        frequency: 1.,
        x: 0.2,
        y: 0.5,
        vx: 1.,
        vy: 0.,
        damage: 5,
        life: 1.,
    });
    r.advance(0.1, &c).unwrap();
    assert!(
        r.bullets.iter().any(|b| b.id == 251),
        "NoWalk ground alone is not projectile cover"
    );
}

#[test]
fn realm_entry_projectile_order_target_in_front_of_static_cover_hits_with_or_without_cover() {
    for cover in [false, true] {
        let (c, mut r) = room(true);
        position(&mut r, 7.2, 21.5);
        let hole = r.entities.values().find(|e| e.kind == 0x800).unwrap().id;
        r.entities.get_mut(&hole).unwrap().kind = c.named("Rock Grey").unwrap().id;
        if !cover {
            r.entities.remove(&hole);
        }
        let chicken = r.entities.values_mut().find(|e| e.kind == 0x801).unwrap();
        chicken.x = 7.9;
        chicken.y = 21.5;
        let target = chicken.id;
        let weapon = r.character(1).stats[&8] as i16;
        let visual = c
            .named(&c.get(weapon).unwrap().arena_projectile(0).unwrap().visual)
            .unwrap()
            .id;
        for bullet_id in [2, 3] {
            r.handle(
                1,
                packet(8, |w| {
                    w.u8(bullet_id)
                        .i32(1)
                        .u8(visual as u8)
                        .i16(weapon)
                        .f32(7.5)
                        .f32(21.5)
                        .f32(0.)
                        .i16(32767);
                }),
                &c,
            )
            .unwrap();
        }
        r.advance(0.05, &c).unwrap();
        assert!(
            !r.entities.contains_key(&target),
            "eligible chicken in front of static cover must be hit; cover={cover}"
        );
        assert!(r.bullets.iter().all(|b| b.owner != 1));
        if cover {
            assert!(
                r.entities.contains_key(&hole),
                "the registered cover itself must not be damaged"
            );
        }
    }
}

#[test]
fn realm_entry_native_occupancy_generator_crossing_keeps_position_and_following_shot_valid() {
    let (c, mut r) = room(true);
    position(&mut r, 23.107666, 23.376188);
    // Original a4 Move5 coordinates and inter-packet elapsed milliseconds, not a native replay claim.
    for (ms, x, y) in [
        (152, 23.525372, 23.376188),
        (245, 24.68356, 23.376188),
        (145, 25.352839, 23.376188),
        (210, 26.354385, 23.376188),
        (246, 26.554659, 23.332554),
    ] {
        r.players.get_mut(&1).unwrap().move_at = Instant::now() - Duration::from_millis(ms);
        r.handle(1, movement(x, y), &c).unwrap();
        assert_eq!(
            (r.character(1).x, r.character(1).y),
            (x, y),
            "server/client must agree after generator passage"
        );
    }
    let p = r.character(1);
    let (x, y) = (p.x, p.y);
    let weapon = p.stats[&8] as i16;
    let visual = c
        .named(&c.get(weapon).unwrap().arena_projectile(0).unwrap().visual)
        .unwrap()
        .id;
    for bullet_id in [2, 3] {
        r.handle(
            1,
            packet(8, |w| {
                w.u8(bullet_id)
                    .i32(1)
                    .u8(visual as u8)
                    .i16(weapon)
                    .f32(x)
                    .f32(y)
                    .f32(0.)
                    .i16(32767);
            }),
            &c,
        )
        .unwrap();
    }
    assert!(r.bullets.iter().any(|b| b.owner == 1 && b.origin == (x, y)));
}

#[test]
fn realm_entry_native_occupancy_diagonal_nowalk_uses_native_y_first_slide() {
    let (c, mut r) = room(true);
    position(&mut r, 12.8, 7.2);
    let map = r.map.as_ref().unwrap();
    assert!(!map.can_move((12.8, 7.2), (13.2, 6.8)));
    assert!(map.can_move((12.8, 7.2), (12.8, 6.8)));
    assert!(map.can_move((12.8, 7.2), (13.2, 7.2)));
    r.handle(1, movement(13.2, 6.8), &c).unwrap();
    assert_eq!((r.character(1).x, r.character(1).y), (12.8, 6.8));
}

#[test]
fn realm_entry_native_occupancy_projectile_registration_distinguishes_generator_from_static_cover()
{
    for registered in [false, true] {
        let (c, mut r) = room(true);
        position(&mut r, 23.5, 23.5);
        if registered {
            let generator = r.entities.values_mut().find(|e| e.kind == 0x807).unwrap();
            generator.kind = c.named("Rock Grey").unwrap().id;
        }
        let weapon = r.character(1).stats[&8] as i16;
        let visual = c
            .named(&c.get(weapon).unwrap().arena_projectile(0).unwrap().visual)
            .unwrap()
            .id;
        for bullet_id in [2, 3] {
            r.handle(
                1,
                packet(8, |w| {
                    w.u8(bullet_id)
                        .i32(1)
                        .u8(visual as u8)
                        .i16(weapon)
                        .f32(23.8)
                        .f32(23.5)
                        .f32(0.)
                        .i16(32767);
                }),
                &c,
            )
            .unwrap();
        }
        r.advance(0.05, &c).unwrap();
        assert_eq!(
            r.bullets.iter().any(|b| b.owner == 1),
            !registered,
            "only an actual registered OccupySquare object supplies cover"
        );
    }
}

fn normal_bow_shot(variable: &str, speed: f32) {
    let path = std::env::var_os(variable).unwrap_or_else(|| panic!("{variable} required"));
    let path = std::path::Path::new(&path);
    let content = Content::load(path).unwrap();
    let bow = content.get(0xa1a).unwrap();
    assert_eq!(bow.projectiles[0].1.features, ["ActivateOnEquip"]);
    let xml = std::fs::read_to_string(path.join("objects.xml")).unwrap();
    let doc = roxmltree::Document::parse(&xml).unwrap();
    let raw = doc
        .descendants()
        .find(|n| n.attribute("id") == Some("Poor Quality Bow"))
        .unwrap();
    assert!(!raw.children().any(|n| n.has_tag_name("ActivateOnEquip")));
    let inert = raw
        .children()
        .find(|n| n.has_tag_name("Projectile"))
        .unwrap()
        .children()
        .find(|n| n.has_tag_name("ActivateOnEquip"))
        .unwrap();
    assert_eq!(
        (
            inert.attribute("stat"),
            inert.attribute("amount"),
            inert.text()
        ),
        (Some("20"), Some("5"), Some("IncrementStat"))
    );
    let grounds = Arc::new(Grounds::load(path).unwrap());
    let map = Arc::new(realm_map::tutorial(100, &content, grounds).unwrap());
    let mut archer = Character::new(&content, 0x307, "Rowan".into(), 1).unwrap();
    (archer.x, archer.y) = map.spawn;
    assert_eq!(archer.stats[&8], 0xa1a);
    assert_eq!(archer.stats[&20], 12);
    let before = archer.clone();
    let owner = 37;
    let origin = (archer.x + 0.3, archer.y);
    let visual = content.named(&bow.projectiles[0].1.visual).unwrap().id as u8;
    let fire = |packet_owner, sub| {
        packet(8, |w| {
            w.u8(9)
                .i32(packet_owner)
                .u8(sub)
                .i16(0xa1a)
                .f32(origin.0)
                .f32(origin.1)
                .f32(0.)
                .i16(32767);
        })
    };
    let mut room = Room::from_map(map, &content, 1_000_000).unwrap();
    room.attach(owner, archer, &content).unwrap();
    assert!(
        room.handle(owner, fire(owner + 1, visual), &content)
            .is_err()
    );
    assert!(
        room.handle(owner, fire(owner, visual ^ 1), &content)
            .is_err()
    );
    assert!(room.bullets.is_empty());
    let events = room.handle(owner, fire(owner, visual), &content).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, 8);
    let bullet = room.bullets.iter().find(|b| b.owner == owner).unwrap();
    assert_eq!(
        (bullet.id, bullet.origin, bullet.vx, bullet.vy),
        (9, origin, speed, 0.)
    );
    // Player.use Number rounding can reach370 before truncation at ATK12 factor .74.
    assert!((if speed == 36. { 333..=370 } else { 7..=29 }).contains(&bullet.damage));
    let mut read = Reader::new(&events[0].body);
    assert_eq!(
        (read.u8().unwrap(), read.i32().unwrap(), read.u8().unwrap()),
        (9, owner, 0)
    );
    assert_eq!(read.i16().unwrap(), 0xa1a);
    assert_eq!(
        (
            read.f32().unwrap(),
            read.f32().unwrap(),
            read.f32().unwrap()
        ),
        (origin.0, origin.1, 0.)
    );
    assert_eq!(read.i16().unwrap(), bullet.damage as i16);
    read.done().unwrap();
    let published = room.publication(owner, &events, &content).unwrap();
    let make = published
        .iter()
        .position(|p| {
            if p.id != 10 {
                return false;
            }
            let mut read = Reader::new(&p.body);
            read.i16().unwrap();
            read.utf().unwrap();
            read.i32().unwrap() == owner
        })
        .unwrap();
    let shot = published.iter().position(|p| p.id == 8).unwrap();
    assert!(make < shot);
    assert_eq!(published[shot].body, events[0].body);
    assert!(room.handle(owner, fire(owner, visual), &content).is_err());
    assert_eq!(room.bullets.iter().filter(|b| b.owner == owner).count(), 1);
    room.advance(0.05, &content).unwrap();
    let bullet = room.bullets.iter().find(|b| b.owner == owner).unwrap();
    assert!((bullet.x - (origin.0 + speed * 0.05)).abs() < 0.00001);
    assert_eq!(room.character(owner), &before);
    let mut fixture = Room::new(100, 0, 2);
    fixture.attach(owner, before.clone(), &content).unwrap();
    let error = fixture
        .handle(owner, fire(owner, visual), &content)
        .err()
        .expect("fixture accepted inert metadata");
    assert!(error.to_string().contains("unsupported arena semantics"));
    assert!(fixture.bullets.is_empty());
    assert_eq!(fixture.character(owner), &before);
}

#[test]
fn realm_entry_normal_projectile_original_bow_shot() {
    normal_bow_shot("BUILD100_CLIENT_DIR", 36.);
}

#[test]
fn realm_entry_normal_projectile_repaired_bow_shot() {
    normal_bow_shot("BUILD100_REPAIRED_CLIENT_DIR", 16.);
}

#[test]
fn realm_entry_normal_projectile_preserves_real_feature_and_identity_rejections() {
    let path = std::env::var_os("BUILD100_CLIENT_DIR").expect("BUILD100_CLIENT_DIR required");
    let content = Content::load(std::path::Path::new(&path)).unwrap();
    let bow = content.get(0xa1a).unwrap();
    let projectile = bow.normal_projectile(0).unwrap();
    assert_eq!((projectile.min, projectile.max), (10, 40));
    assert_eq!(projectile.features, ["ActivateOnEquip"]);
    assert!(bow.arena_projectile(0).is_err());
    for feature in ["Wavy", "ConditionEffect"] {
        let mut supported = bow.clone();
        let projectile = &mut supported.projectiles[0].1;
        projectile.features.push(feature.into());
        if feature == "Wavy" {
            projectile.wavy = true;
        } else {
            projectile.effects.push(crate::content::ConditionEffect {
                condition: crate::content::Condition::ArmorPierce,
                duration_seconds: 0.,
            });
        }
        assert!(supported.normal_projectile(0).is_ok());
        assert!(supported.arena_projectile(0).is_err());
    }
    for feature in ["Continues", "ArmorPierce", "UnknownFeature"] {
        let mut unsupported = bow.clone();
        unsupported.projectiles[0].1.features.push(feature.into());
        let error = unsupported.normal_projectile(0).unwrap_err().to_string();
        assert!(
            error.contains("unsupported normal projectile semantics") && error.contains(feature),
            "{error}"
        );
        assert!(unsupported.arena_projectile(0).is_err());
    }
    for id in [1, 255] {
        assert!(
            bow.normal_projectile(id)
                .unwrap_err()
                .to_string()
                .contains("missing projectile")
        );
        assert!(
            bow.arena_projectile(id)
                .unwrap_err()
                .to_string()
                .contains("missing projectile")
        );
    }
    let mut indexed = bow.clone();
    indexed.projectiles[0].0 = 1;
    assert!(indexed.normal_projectile(0).is_err());
    assert_eq!(indexed.normal_projectile(1).unwrap().min, 10);
    let mut duplicate = bow.clone();
    duplicate.projectiles.push(duplicate.projectiles[0].clone());
    for error in [
        duplicate.normal_projectile(0).unwrap_err(),
        duplicate.arena_projectile(0).unwrap_err(),
    ] {
        assert!(
            error
                .to_string()
                .contains("ambiguous duplicate projectile 0")
        );
    }
}
