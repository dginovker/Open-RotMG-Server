use super::*;
use crate::content::Grounds;
use rand::RngCore;
use std::path::PathBuf;

pub fn load(repaired: bool) -> (Content, Arc<Grounds>) {
    let path = PathBuf::from(
        std::env::var_os(if repaired {
            "BUILD100_CONTENT_REPAIRED_DIR"
        } else {
            "BUILD100_CONTENT_ORIGINAL_DIR"
        })
        .expect("qualified shore content"),
    );
    (
        Content::load(&path).unwrap(),
        Arc::new(Grounds::load(&path).unwrap()),
    )
}
fn realm(repaired: bool, seed: u64) -> (Content, Room) {
    let (c, g) = load(repaired);
    let map = Arc::new(crate::realm_map::realm(seed, &c, g).unwrap());
    let room = Room::from_map(map, &c, 1_000_000).unwrap();
    (c, room)
}
pub fn attach(room: &mut Room, c: &Content, id: i32, kind: i16) {
    let mut p = Character::new(c, kind, format!("Shore{id}"), 1).unwrap();
    (p.x, p.y) = room.landing();
    room.attach(id, p, c).unwrap();
}
// Approved old subsystem fixtures do not model autonomous combat; keep their map/static actors intact.
pub fn isolate_population(room: &mut Room) {
    room.entities
        .retain(|_, e| e.provenance != Provenance::Population);
    room.shore = None;
}
fn one_actor(room: &mut Room) -> i32 {
    let id = *room.shore.as_ref().unwrap().actors.keys().next().unwrap();
    room.entities
        .retain(|&key, e| e.provenance != Provenance::Population || key == id);
    room.shore
        .as_mut()
        .unwrap()
        .actors
        .retain(|&key, _| key == id);
    id
}
fn shots(events: &[Packet], owner: i32) -> Vec<Packet> {
    events
        .iter()
        .filter(|p| p.id == 8)
        .filter_map(|p| {
            let mut r = Reader::new(&p.body);
            r.u8().unwrap();
            (r.i32().unwrap() == owner).then(|| Packet {
                id: p.id,
                body: p.body.clone(),
            })
        })
        .collect()
}
pub fn effective_stats(room: &Room, content: &Content, avatar: i32) -> Stats {
    room.character(avatar).equipment_stats(content).unwrap()
}
pub fn snapshot(room: &Room) -> String {
    let shore = room.shore.as_ref().map(|s| {
        (
            &s.sites,
            s.next_replenish.to_bits(),
            s.actors
                .iter()
                .map(|(&id, b)| {
                    (
                        id,
                        b.home.0.to_bits(),
                        b.home.1.to_bits(),
                        b.next_shot.to_bits(),
                    )
                })
                .collect::<Vec<_>>(),
        )
    });
    let entities: Vec<_> = room
        .entities
        .values()
        .map(|e| {
            (
                e.id,
                e.kind,
                e.x.to_bits(),
                e.y.to_bits(),
                e.age.to_bits(),
                &e.stats,
                format!("{:?}", e.provenance),
            )
        })
        .collect();
    let bullets: Vec<_> = room
        .bullets
        .iter()
        .map(|b| {
            (
                b.owner,
                b.id,
                &b.victims,
                b.origin.0.to_bits(),
                b.origin.1.to_bits(),
                b.age.to_bits(),
                b.x.to_bits(),
                b.y.to_bits(),
                b.damage,
                b.life.to_bits(),
            )
        })
        .collect();
    let players: Vec<_> = room
        .players
        .iter()
        .map(|(&id, a)| (id, &a.character))
        .collect();
    format!(
        "{:?}",
        (
            shore,
            entities,
            bullets,
            players,
            room.elapsed.to_bits(),
            room.next_id,
            &room.server_projectile_cursors,
            &room.enemy_stuns,
            room.rng.clone().next_u64()
        )
    )
}

#[test]
fn shore_runtime_constructor_source_cap_seeded_sites_and_mode_separation() {
    for repaired in [false, true] {
        for seed in [0, 1, 100, 256, u64::MAX] {
            let (c, r) = realm(repaired, seed);
            let pop = r.shore.as_ref().unwrap();
            assert_eq!(pop.actors.len(), 4);
            assert_eq!(pop.next_replenish, 10.);
            let map = r.map.as_ref().unwrap();
            let mut homes = vec![];
            for (&id, b) in &pop.actors {
                let e = &r.entities[&id];
                assert_eq!(e.kind, 0x600);
                assert_eq!(e.provenance, Provenance::Population);
                assert_eq!((e.stats[&0], e.stats[&1], e.stats[&21]), (5, 5, 0));
                assert_eq!((e.x, e.y), b.home);
                assert!((6. ..=12.).contains(&distance(e.x, e.y, map.spawn.0, map.spawn.1)));
                assert_eq!(
                    map.tile(e.x as i32, e.y as i32).unwrap().region,
                    Region::ShoreSand
                );
                assert!(map.walkable(e.x as i32, e.y as i32));
                assert!(map.ground(e.x as i32, e.y as i32).unwrap().damage.is_none());
                assert!(homes.iter().all(|&(x, y)| distance(e.x, e.y, x, y) >= 2.5));
                homes.push(b.home);
            }
            assert_eq!(
                r.rng.clone().next_u64(),
                StdRng::seed_from_u64(seed).next_u64()
            );
            let (_, again) = realm(repaired, seed);
            assert_eq!(again.shore.as_ref().unwrap().sites, pop.sites);
            let pirate = c.get(0x600).unwrap();
            assert_eq!(progression::enemy_xp(pirate).unwrap(), 1);
            let p = pirate.normal_projectile(0).unwrap();
            assert_eq!((p.min, p.max, p.speed, p.lifetime), (5, 5, 6., 0.6));
        }
        let (c, g) = load(repaired);
        let map = Arc::new(crate::realm_map::tutorial(100, &c, g).unwrap());
        let tutorial = Room::from_map(map, &c, 1_500_000).unwrap();
        assert!(tutorial.shore.is_none());
        assert!(
            !tutorial
                .entities
                .values()
                .any(|e| e.provenance == Provenance::Population)
        );
        assert!(Room::new(100, 0, 2).shore.is_none());
    }
}

#[test]
fn shore_runtime_invalid_static_sites_and_metadata_fail_constructor() {
    let (mut c, g) = load(true);
    let mut map = crate::realm_map::realm(100, &c, g.clone()).unwrap();
    map.spawn = (0.5, 0.5);
    let error = Room::from_map(Arc::new(map), &c, 1_000_000)
        .err()
        .expect("invalid shore");
    assert!(
        error
            .to_string()
            .contains("four separated static-valid ShoreSand sites")
    );
    let map = Arc::new(crate::realm_map::realm(100, &c, g).unwrap());
    c.objects.get_mut(&0x600).unwrap().stats.remove(&0);
    assert!(Room::from_map(map, &c, 1_000_000).is_err());
}

#[test]
fn shore_runtime_global_replenishment_cadence_occupancy_and_no_catchup() {
    let (c, mut r) = realm(true, 100);
    attach(&mut r, &c, 1, 0x30e);
    let old: Vec<_> = r.shore.as_ref().unwrap().actors.keys().copied().collect();
    for id in &old {
        r.entities.remove(id);
    }
    r.advance(9.5, &c).unwrap();
    assert!(r.shore.as_ref().unwrap().actors.is_empty());
    r.advance(0.5, &c).unwrap();
    assert_eq!(r.shore.as_ref().unwrap().actors.len(), 4);
    assert_eq!(r.shore.as_ref().unwrap().next_replenish, 20.);
    for &id in r.shore.as_ref().unwrap().actors.keys() {
        let e = &r.entities[&id];
        let p = r.character(1);
        assert!(distance(e.x, e.y, p.x, p.y) >= 3.);
        assert!(!old.contains(&id));
    }
    let ids: Vec<_> = r.shore.as_ref().unwrap().actors.keys().copied().collect();
    for id in ids {
        r.entities.remove(&id);
    }
    let site = r.shore.as_ref().unwrap().sites[0];
    // A finite test-owned candidate list makes every selected location occupied, not an alternate spawn mode.
    r.shore.as_mut().unwrap().sites = vec![site];
    (r.character_mut(1).x, r.character_mut(1).y) = site;
    let before_rng = r.rng.clone().next_u64();
    r.advance(10., &c).unwrap();
    assert!(r.shore.as_ref().unwrap().actors.is_empty());
    assert_eq!(r.rng.clone().next_u64(), before_rng);
    assert_eq!(r.shore.as_ref().unwrap().next_replenish, 30.);
    (r.character_mut(1).x, r.character_mut(1).y) = r.landing();
    r.advance(0., &c).unwrap();
    assert!(r.shore.as_ref().unwrap().actors.is_empty());
    r.advance(100., &c).unwrap();
    assert_eq!(r.shore.as_ref().unwrap().actors.len(), 1);
    assert_eq!(r.shore.as_ref().unwrap().next_replenish, 130.);
}

#[test]
fn shore_runtime_nearest_living_visible_boundaries_and_invalid_paused_fail_closed() {
    let (c, mut r) = realm(true, 100);
    let id = one_actor(&mut r);
    let home = r.shore.as_ref().unwrap().actors[&id].home;
    attach(&mut r, &c, 1, 0x300);
    attach(&mut r, &c, 2, 0x30e);
    for avatar in r.players.values_mut() {
        (avatar.character.x, avatar.character.y) = (home.0 + 2., home.1);
    }
    let target = |r: &Room| {
        shore_target(&r.players, &r.entities[&id], home)
            .unwrap()
            .map(|p| p.0)
    };
    assert_eq!(target(&r), Some(1));
    r.handle(1, spell_runtime_tests::cast(1, 1, 1, home), &c)
        .unwrap();
    assert_eq!(target(&r), Some(2));
    r.character_mut(2).alive = false;
    r.character_mut(2).stats.insert(1, 0);
    assert_eq!(target(&r), None);
    r.character_mut(2).alive = true;
    r.character_mut(2).stats.insert(1, 100);
    r.character_mut(2).x = home.0 + 10.;
    assert_eq!(target(&r), None);
    r.character_mut(2).x = (home.0 + 10.).next_down();
    assert_eq!(target(&r), Some(2));
    r.entities.get_mut(&id).unwrap().x = home.0 + 4.;
    r.character_mut(2).x = home.0 + 12.;
    assert_eq!(target(&r), Some(2));
    r.character_mut(2).x = (home.0 + 12.).next_up();
    assert_eq!(target(&r), None);
    r.character_mut(2).stats.insert(
        25,
        combat::ConditionMask::bit(crate::content::Condition::Paused) as i32,
    );
    assert!(shore_target(&r.players, &r.entities[&id], home).is_err());
    assert!(r.advance(0., &c).is_err());
}

#[test]
fn shore_runtime_chase_stop_return_idle_and_invisible_is_not_projectile_immunity() {
    let (c, mut r) = realm(true, 100);
    let id = one_actor(&mut r);
    let home = r.shore.as_ref().unwrap().actors[&id].home;
    attach(&mut r, &c, 1, 0x300);
    let map = r.map.as_ref().unwrap();
    let sign = [1., -1.]
        .into_iter()
        .find(|s| map.can_move(home, (home.0 + s * 13., home.1)))
        .expect("clear shore axis");
    r.character_mut(1).x = home.0 + sign * 8.;
    r.character_mut(1).y = home.1;
    r.advance(0.25, &c).unwrap();
    assert_eq!(r.entities[&id].x, home.0 + sign * (6.29_f32 * 0.25));
    r.advance(1., &c).unwrap();
    r.advance(0.25, &c).unwrap();
    assert_eq!(
        distance(
            r.entities[&id].x,
            r.entities[&id].y,
            r.character(1).x,
            r.character(1).y
        ),
        1.
    );
    r.entities.get_mut(&id).unwrap().x = home.0 + sign * 13.;
    r.character_mut(1).x = home.0 + sign * 11.;
    let events = r.advance(0.5, &c).unwrap();
    assert_eq!(r.entities[&id].x, home.0 + sign * (13. - 2.96_f32 * 0.5));
    assert!(shots(&events, id).is_empty());
    r.detach(1);
    r.advance(5., &c).unwrap();
    assert_eq!((r.entities[&id].x, r.entities[&id].y), home);
    let before = r.rng.clone().next_u64();
    r.advance(0.1, &c).unwrap();
    assert_eq!((r.entities[&id].x, r.entities[&id].y), home);
    assert_eq!(r.rng.clone().next_u64(), before);
    attach(&mut r, &c, 2, 0x300);
    r.character_mut(2).x = home.0 + sign * 2.;
    r.character_mut(2).y = home.1;
    let hp = r.character(2).stats[&1];
    let events = r.advance(0., &c).unwrap();
    assert_eq!(shots(&events, id).len(), 1);
    r.handle(2, spell_runtime_tests::cast(2, 1, 2, home), &c)
        .unwrap();
    r.advance(0.3, &c).unwrap();
    let damage = combat::projectile_defended_damage(
        5,
        r.character(2).equipment_stats(&c).unwrap().get(21).unwrap(),
        c.get(0x600).unwrap().normal_projectile(0).unwrap(),
    )
    .unwrap();
    assert!(damage > 0);
    assert_eq!(r.character(2).stats[&1], hp - damage);
    assert_eq!(r.character(2).stats[&25], 4096);
}

#[test]
fn shore_runtime_source_aim_cooldown_cyclic_ids_and_no_catchup() {
    let (c, mut r) = realm(true, 100);
    let id = one_actor(&mut r);
    let home = r.shore.as_ref().unwrap().actors[&id].home;
    attach(&mut r, &c, 1, 0x30e);
    r.character_mut(1).x = home.0 + 3.;
    r.character_mut(1).y = home.1;
    assert!(shots(&r.advance(0., &c).unwrap(), id).is_empty());
    r.character_mut(1).x = (home.0 + 3.).next_down();
    let events = r.advance(0., &c).unwrap();
    let cast = shots(&events, id);
    assert_eq!(cast.len(), 1);
    let mut read = Reader::new(&cast[0].body);
    assert_eq!(read.u8().unwrap(), 1);
    assert_eq!(read.i32().unwrap(), id);
    assert_eq!(read.u8().unwrap(), 0);
    assert_eq!(read.i16().unwrap(), 0x600);
    assert_eq!((read.f32().unwrap(), read.f32().unwrap()), home);
    assert_eq!(read.f32().unwrap(), 0.);
    assert_eq!(read.i16().unwrap(), 5);
    let below = 2.5_f32.next_down();
    assert!(shots(&r.advance(f64::from(below), &c).unwrap(), id).is_empty());
    let events = r
        .advance(f64::from(2.5_f32) - f64::from(below), &c)
        .unwrap();
    assert_eq!(shots(&events, id)[0].body[0], 2);
    let events = r.advance(20., &c).unwrap();
    assert_eq!(shots(&events, id).len(), 1);
    assert_eq!(shots(&events, id)[0].body[0], 3);
    assert_eq!(
        r.shore.as_ref().unwrap().actors[&id].next_shot,
        r.elapsed + 2.5
    );
}

pub fn actors(room: &Room) -> Vec<(i32, (f32, f32))> {
    room.shore
        .as_ref()
        .unwrap()
        .actors
        .keys()
        .map(|&id| (id, (room.entities[&id].x, room.entities[&id].y)))
        .collect()
}

#[test]
fn shore_runtime_spawn_player_and_actor_exclusions_use_exact_separate_bounds() {
    let (c, mut r) = realm(true, 100);
    let site = r.shore.as_ref().unwrap().sites[0];
    r.entities
        .retain(|_, e| e.provenance != Provenance::Population);
    r.shore.as_mut().unwrap().sites = vec![site];
    attach(&mut r, &c, 1, 0x30e);
    r.character_mut(1).x = site.0 + 3.;
    r.character_mut(1).y = site.1;
    r.advance(10., &c).unwrap();
    assert_eq!(actors(&r).len(), 1);
    assert_eq!(actors(&r)[0].1, site);
    r.entities
        .retain(|_, e| e.provenance != Provenance::Population);
    r.character_mut(1).x = site.0 + 4.;
    let def = c.get(0x600).unwrap();
    let id = r.allocate_entity().unwrap();
    let mut stats = def.stats.clone();
    stats.insert(1, 5);
    let bound = site.0 + 1.3;
    let below = if distance(site.0, site.1, bound, site.1) < 1.3 {
        bound
    } else {
        bound.next_down()
    };
    r.entities.insert(
        id,
        Entity {
            id,
            kind: def.id,
            x: below,
            y: site.1,
            stats,
            age: 0.,
            provenance: Provenance::Map,
        },
    );
    r.advance(10., &c).unwrap();
    assert!(actors(&r).is_empty());
    r.entities.get_mut(&id).unwrap().x = below.next_up();
    assert!(distance(site.0, site.1, r.entities[&id].x, site.1) >= 1.3);
    r.advance(10., &c).unwrap();
    assert_eq!(actors(&r).len(), 1);
    let (c, mut r) = realm(true, 100);
    for id in 1..=8 {
        attach(&mut r, &c, id, 0x30e);
    }
    r.advance(10., &c).unwrap();
    assert_eq!(actors(&r).len(), 4);
}

#[test]
fn shore_runtime_real_static_sweep_prevents_endpoint_tunnel_and_rejects_outside_leash_slide() {
    let (c, mut r) = realm(true, 100);
    let id = one_actor(&mut r);
    attach(&mut r, &c, 1, 0x30e);
    let map = r.map.as_ref().unwrap().clone();
    let (from, goal) = map
        .objects()
        .iter()
        .find_map(|p| {
            let (x, y) = p.position();
            let from = (x - 2., y);
            let goal = (x + 4., y);
            (map.walkable(from.0 as i32, from.1 as i32)
                && map.walkable(goal.0 as i32, goal.1 as i32)
                && map.walkable((from.0 + 5.) as i32, from.1 as i32)
                && !map.can_move(from, (from.0 + 5., from.1)))
            .then_some((from, goal))
        })
        .expect("real static blocker with clear endpoint");
    {
        let e = r.entities.get_mut(&id).unwrap();
        e.x = from.0;
        e.y = from.1;
    }
    r.shore.as_mut().unwrap().actors.get_mut(&id).unwrap().home = from;
    r.character_mut(1).x = goal.0;
    r.character_mut(1).y = goal.1;
    r.advance(1., &c).unwrap();
    assert_eq!((r.entities[&id].x, r.entities[&id].y), from);
    let (from, home, goal) = map
        .objects()
        .iter()
        .find_map(|p| {
            let (x, y) = p.position();
            let from = (x + 1., y - 1.);
            let home = (from.0 - 11., from.1);
            let goal = (from.0 - 3., from.1 + 8.);
            let d = distance(from.0, from.1, goal.0, goal.1);
            let step = 6.29_f32;
            let to = (
                from.0 + (goal.0 - from.0) / d * step,
                from.1 + (goal.1 - from.1) / d * step,
            );
            let slide = (from.0, to.1);
            (map.walkable(home.0 as i32, home.1 as i32)
                && map.walkable(goal.0 as i32, goal.1 as i32)
                && !map.can_move(from, to)
                && map.can_move(from, slide)
                && distance(home.0, home.1, to.0, to.1) <= 12.
                && distance(home.0, home.1, slide.0, slide.1) > 12.)
                .then_some((from, home, goal))
        })
        .expect("real Y-slide that exits home disk");
    {
        let e = r.entities.get_mut(&id).unwrap();
        e.x = from.0;
        e.y = from.1;
    }
    r.shore.as_mut().unwrap().actors.get_mut(&id).unwrap().home = home;
    r.character_mut(1).x = goal.0;
    r.character_mut(1).y = goal.1;
    r.advance(1., &c).unwrap();
    assert_eq!((r.entities[&id].x, r.entities[&id].y), from);
}

#[test]
fn shore_runtime_aim_has_no_los_gate_but_real_cover_stops_source_bullet() {
    let (c, mut r) = realm(true, 100);
    let id = one_actor(&mut r);
    attach(&mut r, &c, 1, 0x30e);
    let map = r.map.as_ref().unwrap().clone();
    let (from, goal) = map
        .objects()
        .iter()
        .find_map(|p| {
            let def = c.get(p.kind).unwrap();
            let (x, y) = p.position();
            let from = (x - 1., y);
            let goal = (x + 1., y);
            (def.is_static
                && def.occupy_square
                && map.walkable(from.0 as i32, from.1 as i32)
                && map.walkable(goal.0 as i32, goal.1 as i32)
                && !map.can_move(from, goal))
            .then_some((from, goal))
        })
        .expect("cover between clear nearby actors");
    {
        let e = r.entities.get_mut(&id).unwrap();
        e.x = from.0;
        e.y = from.1;
    }
    r.shore.as_mut().unwrap().actors.get_mut(&id).unwrap().home = from;
    r.character_mut(1).x = goal.0;
    r.character_mut(1).y = goal.1;
    let hp = r.character(1).stats[&1];
    let events = r.advance(0., &c).unwrap();
    assert_eq!(shots(&events, id).len(), 1);
    r.advance(0.2, &c).unwrap();
    assert!(!r.bullets.iter().any(|b| b.owner == id));
    assert_eq!(r.character(1).stats[&1], hp);
}

#[test]
fn shore_runtime_known_pirate_shoot_before_same_frame_death_is_published_before_drop() {
    let (c, mut r) = realm(true, 100);
    let id = one_actor(&mut r);
    let home = r.shore.as_ref().unwrap().actors[&id].home;
    attach(&mut r, &c, 1, 0x30e);
    attach(&mut r, &c, 2, 0x30e);
    r.character_mut(1).x = home.0 + 2.;
    r.character_mut(1).y = home.1;
    r.publication(1, &[], &c).unwrap();
    assert!(r.players[&1].known_objects.contains(&id));
    let cast = r
        .handle(1, spell_runtime_tests::cast(1, 1, 1, home), &c)
        .unwrap();
    r.publication(1, &cast, &c).unwrap();
    let mut live_outside = r.clone();
    live_outside.entities.get_mut(&id).unwrap().x = r.character(1).x + 20.;
    let owner = live_outside.entities[&id].clone();
    let shot = normal_npc_shot(
        &owner,
        0.,
        &c,
        &mut live_outside.bullets,
        &mut live_outside.server_projectile_cursors,
        &mut live_outside.rng,
    )
    .unwrap();
    let rejected = live_outside
        .publication(1, &[shot, drop_object(id)], &c)
        .unwrap();
    assert!(shots(&rejected, id).is_empty());
    let events = r.advance(0., &c).unwrap();
    assert_eq!(shots(&events, id).len(), 1);
    assert!(!r.entities.contains_key(&id));
    assert!(r.bullets.iter().any(|b| b.owner == id));
    let mut without_drop = r.clone();
    let missing_removal: Vec<_> = events
        .iter()
        .filter(|p| p.id != 11)
        .map(|p| Packet {
            id: p.id,
            body: p.body.clone(),
        })
        .collect();
    assert!(
        shots(
            &without_drop.publication(1, &missing_removal, &c).unwrap(),
            id
        )
        .is_empty()
    );
    let unknown = r.publication(2, &events, &c).unwrap();
    assert!(shots(&unknown, id).is_empty());
    assert!(
        !unknown
            .iter()
            .any(|p| p.id == 11 && Reader::new(&p.body).i32().unwrap() == id)
    );
    let published = r.publication(1, &events, &c).unwrap();
    assert_eq!(shots(&published, id).len(), 1);
    for events in [&published, &unknown] {
        assert!(!events.iter().any(|p| {
            if p.id != 10 {
                return false;
            }
            let mut read = Reader::new(&p.body);
            read.i16().unwrap();
            read.utf().unwrap();
            read.i32().unwrap() == id
        }));
    }
    let shoot = published.iter().position(|p| p.id == 8).unwrap();
    let drop = published
        .iter()
        .position(|p| p.id == 11 && Reader::new(&p.body).i32().unwrap() == id)
        .unwrap();
    assert!(shoot < drop);
    assert_eq!(
        published
            .iter()
            .filter(|p| p.id == 11 && Reader::new(&p.body).i32().unwrap() == id)
            .count(),
        1
    );
    let replay = r.publication(1, &events, &c).unwrap();
    assert!(shots(&replay, id).is_empty());
    assert!(
        !replay
            .iter()
            .any(|p| p.id == 11 && Reader::new(&p.body).i32().unwrap() == id)
    );
}
