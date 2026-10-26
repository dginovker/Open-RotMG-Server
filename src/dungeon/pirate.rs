use crate::content::{Cell, Content, Map};
use anyhow::{Context, Result, ensure};
use rand::{Rng, SeedableRng, seq::SliceRandom};

pub const BOSS: u16 = 0x927;
const WALL: u16 = 0x1ce;
const PALM: u16 = 0x18e;
const EXIT: u16 = 0x703;
const FLOOR: u8 = 0x0c;
const SAND: u8 = 0xbd;
const WATER: u8 = 0x73;
const COMPOSITE: u8 = 0xfd;
const SPACE: u8 = 0xfe;
const MAX_SIDE: i32 = 336;
const MAX_ACTORS: usize = 103;
const OFFICERS: &[u16] = &[0x683, 0x684, 0x685, 0x686];
const PIRATES: &[u16] = &[0x687, 0x688, 0x689];
const PETS: &[u16] = &[0x68a, 0x68b, 0x68c, 0x68d, 0x68e, 0x68f];
const DIRECTIONS: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];

// Reconstruction, not official RNG/distributions. Comparator: realm-src 1855c017,
// DungeonGen/Templates/PirateCave, Copyright (C) 2015 creepylava, AGPL-3.0-or-later.
// Uses this server's Rust room/corridor and wall-configuration patterns, not a C# port or assets.
pub fn generate(content: &Content, seed: u64) -> Result<Map> {
    build(content, seed).with_context(|| format!("Pirate Cave generation seed {seed}"))
}

fn build(content: &Content, seed: u64) -> Result<Map> {
    for ground in [FLOOR, SAND, WATER, COMPOSITE, SPACE] {
        ensure!(
            content.grounds.contains_key(&ground),
            "unknown ground {ground:#x}"
        );
    }
    for kind in [BOSS, WALL, PALM, EXIT]
        .iter()
        .chain(OFFICERS)
        .chain(PIRATES)
        .chain(PETS)
    {
        content.get(*kind)?;
    }
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let steps = rng.gen_range(5..=7);
    let rotation = rng.gen_range(0..4);
    let mut sites = vec![(0, 0)];
    let mut edges = Vec::new();
    // A monotone spine cannot trap itself; finite frontier enumeration needs no retries.
    for parent in 0..steps {
        let (dx, dy) = DIRECTIONS[(rotation + rng.gen_range(0..2)) % 4];
        let (x, y) = sites[parent];
        sites.push((x + dx, y + dy));
        edges.push((parent, parent + 1));
    }
    let boss_room = steps;
    for _ in 0..rng.gen_range(3..=5) {
        let candidates: Vec<_> = sites
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != 0 && *i != boss_room)
            .flat_map(|(parent, &(x, y))| {
                DIRECTIONS
                    .iter()
                    .map(move |&(dx, dy)| (parent, (x + dx, y + dy)))
            })
            .filter(|&(_, (x, y))| {
                !sites.contains(&(x, y))
                    && sites
                        .iter()
                        .filter(|&&(sx, sy)| (sx - x).abs() + (sy - y).abs() == 1)
                        .count()
                        == 1
            })
            .collect();
        let &(parent, site) = candidates.choose(&mut rng).context("no room frontier")?;
        edges.push((parent, sites.len()));
        sites.push(site);
    }
    let spacing = rng.gen_range(21..=25);
    let min_x = sites.iter().map(|p| p.0).min().unwrap();
    let min_y = sites.iter().map(|p| p.1).min().unwrap();
    let centers: Vec<_> = sites
        .iter()
        .map(|&(x, y)| ((x - min_x) * spacing + 14, (y - min_y) * spacing + 14))
        .collect();
    let width = centers.iter().map(|p| p.0).max().unwrap() + 15;
    let height = centers.iter().map(|p| p.1).max().unwrap() + 15;
    ensure!(
        width <= MAX_SIDE && height <= MAX_SIDE,
        "map {width}x{height} exceeds {MAX_SIDE}x{MAX_SIDE}"
    );
    let mut map = Map {
        width,
        height,
        cells: vec![
            Cell {
                ground: SPACE,
                object: None,
                config: String::new(),
                terrain: 0,
                region: 0,
            };
            (width * height) as usize
        ],
        spawns: vec![],
    };
    let index = |x: i32, y: i32| (y * width + x) as usize;
    let mut populations = Vec::new();
    for (room, &(cx, cy)) in centers.iter().enumerate() {
        let round = room == 0 || room == boss_room;
        let (w, h) = if round {
            (21, 21)
        } else {
            (rng.gen_range(8..=14), rng.gen_range(8..=14))
        };
        let mut population = Vec::new();
        for y in cy - h / 2..cy + (h + 1) / 2 {
            for x in cx - w / 2..cx + (w + 1) / 2 {
                let (dx, dy) = (x - cx, y - cy);
                if round && dx * dx + dy * dy > 100 {
                    continue;
                }
                let cell = &mut map.cells[index(x, y)];
                cell.ground = if room == 0 { SAND } else { FLOOR };
                cell.region = room as u8 + 2;
                // Reserve the central cross for unobstructed two-tile doors and the boss.
                if room != 0
                    && dx.abs() > 1
                    && dy.abs() > 1
                    && if round {
                        dx * dx + dy * dy <= 64
                    } else {
                        x > cx - w / 2
                            && x < cx + (w + 1) / 2 - 1
                            && y > cy - h / 2
                            && y < cy + (h + 1) / 2 - 1
                    }
                {
                    population.push(index(x, y));
                }
            }
        }
        population.shuffle(&mut rng);
        populations.push(population);
    }
    for &(a, b) in &edges {
        let ((x0, y0), (x1, y1)) = (centers[a], centers[b]);
        for y in y0.min(y1) - i32::from(y0 == y1)..=y0.max(y1) {
            for x in x0.min(x1) - i32::from(x0 == x1)..=x0.max(x1) {
                let cell = &mut map.cells[index(x, y)];
                if cell.ground == SPACE {
                    cell.ground = FLOOR;
                }
            }
        }
    }
    // Snapshot each dilation so scan order cannot turn a margin into unbounded water.
    for _ in 0..2 {
        let previous = map.cells.clone();
        for y in 1..height - 1 {
            for x in 1..width - 1 {
                let i = index(x, y);
                if previous[i].ground == SPACE
                    && DIRECTIONS.iter().any(|&(dx, dy)| {
                        matches!(previous[index(x + dx, y + dy)].ground, FLOOR | SAND | WATER)
                    })
                {
                    map.cells[i].ground = WATER;
                }
            }
        }
    }
    let shoreline = map.cells.clone();
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            if shoreline[index(x, y)].ground == SPACE
                && (-1..=1)
                    .any(|dy| (-1..=1).any(|dx| shoreline[index(x + dx, y + dy)].ground != SPACE))
            {
                let cell = &mut map.cells[index(x, y)];
                cell.ground = COMPOSITE;
                cell.object = Some(WALL);
            }
        }
    }
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            if map.cells[index(x, y)].object == Some(WALL) {
                let connection = DIRECTIONS.iter().fold(0_u32, |conn, &(dx, dy)| {
                    (conn << 8)
                        | if map.cells[index(x + dx, y + dy)].object == Some(WALL) {
                            1
                        } else {
                            2
                        }
                });
                map.cells[index(x, y)].config = format!("conn:{connection}");
            }
        }
    }
    for (room, population) in populations.iter_mut().enumerate().skip(1) {
        let boss = room == boss_room;
        for (family, count) in [
            (OFFICERS, rng.gen_range(if boss { 4..=7 } else { 0..=1 })),
            (PIRATES, rng.gen_range(if boss { 4..=7 } else { 3..=5 })),
            (PETS, if boss { 0 } else { rng.gen_range(0..=2) }),
        ] {
            for _ in 0..count {
                let i = population.pop().context("room has no population site")?;
                map.cells[i].object = Some(*family.choose(&mut rng).unwrap());
            }
        }
    }
    let (sx, sy) = centers[0];
    for (dx, dy) in [(-7, 0), (7, 0), (0, -7), (0, 7)] {
        map.cells[index(sx + dx, sy + dy)].object = Some(PALM);
    }
    let spawn = &mut map.cells[index(sx, sy)];
    spawn.object = Some(EXIT);
    spawn.region = 1;
    map.spawns.push((sx as f32 + 0.5, sy as f32 + 0.5));
    let (bx, by) = centers[boss_room];
    map.cells[index(bx, by)].object = Some(BOSS);
    let actors = map
        .cells
        .iter()
        .filter(|c| c.object.is_some_and(|k| content.objects[&k].enemy))
        .count();
    ensure!(
        actors <= MAX_ACTORS,
        "population {actors} exceeds {MAX_ACTORS}"
    );
    tracing::info!(
        seed,
        width,
        height,
        cells = map.cells.len(),
        rooms = centers.len(),
        actors,
        boss_x = bx,
        boss_y = by,
        spawn_x = sx,
        spawn_y = sy,
        "generated reconstructed Pirate Cave"
    );
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, VecDeque};

    #[test]
    fn selected_pirate_identities_and_projectiles() {
        let content = Content::load(std::path::Path::new("data")).unwrap();
        for (kind, name) in [
            (FLOOR, "Brown Lines"),
            (SAND, "Light Sand"),
            (WATER, "Shallow Water"),
            (COMPOSITE, "Composite"),
            (SPACE, "Space"),
        ] {
            assert_eq!(content.ground_names[&kind], name);
        }
        for (kind, name) in [
            (WALL, "Cave Wall"),
            (PALM, "Palm Tree"),
            (EXIT, "Portal of Cowardice"),
            (BOSS, "Dreadstump the Pirate King"),
            (0x683, "Pirate Lieutenant"),
            (0x684, "Pirate Commander"),
            (0x685, "Pirate Captain"),
            (0x686, "Pirate Admiral"),
            (0x687, "Cave Pirate Brawler"),
            (0x688, "Cave Pirate Sailor"),
            (0x689, "Cave Pirate Veteran"),
            (0x68a, "Cave Pirate Moll"),
            (0x68b, "Cave Pirate Parrot"),
            (0x68c, "Cave Pirate Macaw"),
            (0x68d, "Cave Pirate Monkey"),
            (0x68e, "Cave Pirate Hunchback"),
            (0x68f, "Cave Pirate Cabin Boy"),
        ] {
            assert_eq!(content.get(kind).unwrap().name, name);
        }
        for (kind, damage, speed, lifetime) in [
            (0x683, 8, 180., 500.),
            (0x684, 10, 180., 500.),
            (0x685, 12, 180., 500.),
            (0x686, 14, 180., 500.),
            (0x687, 5, 60., 600.),
            (0x688, 6, 60., 600.),
            (0x689, 6, 60., 600.),
        ] {
            let object = content.get(kind).unwrap();
            assert!(object.enemy);
            assert_eq!(object.projectiles.len(), 1);
            let p = &object.projectiles[&0];
            assert_eq!(
                (p.min, p.max, p.speed, p.lifetime),
                (damage, damage, speed, lifetime)
            );
        }
        for (kind, name) in [(0x0006, "Blade"), (0x0033, "Black Ball")] {
            let projectile = content.get(kind).unwrap();
            assert_eq!(projectile.name, name);
            assert_eq!(projectile.class, "Projectile");
        }
        for &kind in PETS {
            assert!(content.objects[&kind].enemy);
            assert!(content.objects[&kind].projectiles.is_empty());
        }
        let boss = content.get(BOSS).unwrap();
        assert_eq!((boss.hp, boss.defense, boss.projectiles.len()), (800, 3, 2));
        for (id, damage, speed, lifetime) in [(0, 17, 75., 1500.), (1, 25, 130., 1100.)] {
            let p = &boss.projectiles[&id];
            assert_eq!(
                (p.min, p.max, p.speed, p.lifetime),
                (damage, damage, speed, lifetime)
            );
        }
    }

    #[test]
    fn seeded_replay_varies_topology_cells_and_population() {
        let content = Content::load(std::path::Path::new("data")).unwrap();
        let mut layouts = BTreeSet::new();
        let mut sizes = BTreeSet::new();
        let mut populations = BTreeSet::new();
        for seed in 0..32 {
            let map = generate(&content, seed).unwrap();
            let replay = generate(&content, seed).unwrap();
            assert_eq!(
                serde_json::to_vec(&map).unwrap(),
                serde_json::to_vec(&replay).unwrap()
            );
            sizes.insert((map.width, map.height));
            populations.insert(
                map.cells
                    .iter()
                    .filter_map(|c| c.object)
                    .filter(|k| content.objects[k].enemy)
                    .count(),
            );
            layouts.insert(map.cells.iter().map(|c| c.region).collect::<Vec<_>>());
        }
        assert_eq!(layouts.len(), 32);
        assert!(sizes.len() >= 8);
        assert!(populations.len() >= 8);
    }

    fn flood(start: (i32, i32), passable: impl Fn(i32, i32) -> bool) -> BTreeSet<(i32, i32)> {
        let mut seen = BTreeSet::from([start]);
        let mut queue = VecDeque::from([start]);
        while let Some((x, y)) = queue.pop_front() {
            for (dx, dy) in DIRECTIONS {
                let p = (x + dx, y + dy);
                if passable(p.0, p.1) && seen.insert(p) {
                    queue.push_back(p);
                }
            }
        }
        seen
    }

    #[test]
    fn connected_bounded_caves_keep_two_tile_doors_safe_arrival_and_native_walls() {
        let content = Content::load(std::path::Path::new("data")).unwrap();
        for seed in (0..64).chain([u64::MAX, 0x5621_2351]) {
            let map = generate(&content, seed).unwrap();
            assert!(map.width <= MAX_SIDE && map.height <= MAX_SIDE);
            assert_eq!(map.cells.len(), (map.width * map.height) as usize);
            let index = |x: i32, y: i32| (y * map.width + x) as usize;
            let count = |kind| map.cells.iter().filter(|c| c.object == Some(kind)).count();
            assert_eq!(count(BOSS), 1);
            assert_eq!(count(EXIT), 1);
            assert_eq!(map.spawns.len(), 1);
            assert_eq!(map.cells.iter().filter(|c| c.region == 1).count(), 1);
            let (sx, sy) = (map.spawns[0].0 as i32, map.spawns[0].1 as i32);
            assert_eq!(map.spawns[0], (sx as f32 + 0.5, sy as f32 + 0.5));
            assert_eq!(map.cells[index(sx, sy)].object, Some(EXIT));
            for y in sy - 5..=sy + 5 {
                for x in sx - 5..=sx + 5 {
                    let c = &map.cells[index(x, y)];
                    assert_eq!(c.ground, SAND);
                    assert!(c.object.is_none() || (x == sx && y == sy && c.object == Some(EXIT)));
                }
            }
            let passable = |x: i32, y: i32| {
                (0..map.width).contains(&x) && (0..map.height).contains(&y) && {
                    let c = &map.cells[index(x, y)];
                    content.grounds[&c.ground] > 0.
                        && !c.object.is_some_and(|k| content.objects[&k].blocks)
                }
            };
            let seen = flood((sx, sy), passable);
            let wide = |x, y| {
                (0..2).all(|dy| {
                    (0..2).all(|dx| {
                        passable(x + dx, y + dy)
                            && matches!(map.cells[index(x + dx, y + dy)].ground, FLOOR | SAND)
                    })
                })
            };
            assert!(wide(sx - 1, sy - 1));
            let wide_seen = flood((sx - 1, sy - 1), wide);
            let mut actors = 0;
            let mut water = 0;
            let mut walls = 0;
            let mut wall_start = None;
            let mut corridor_sections = 0;
            let mut room_actors = std::collections::BTreeMap::<u8, usize>::new();
            for (i, c) in map.cells.iter().enumerate() {
                assert!(content.grounds.contains_key(&c.ground));
                let (x, y) = (i as i32 % map.width, i as i32 / map.width);
                if passable(x, y) {
                    assert!(seen.contains(&(x, y)), "seed {seed}, disconnected {x},{y}");
                    assert!(x > 0 && y > 0 && x < map.width - 1 && y < map.height - 1);
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            assert_ne!(map.cells[index(x + dx, y + dy)].ground, SPACE);
                        }
                    }
                }
                if c.ground == FLOOR
                    && c.region == 0
                    && (-2..=2)
                        .all(|dy| (-2..=2).all(|dx| map.cells[index(x + dx, y + dy)].region == 0))
                {
                    let dry = |x, y| matches!(map.cells[index(x, y)].ground, FLOOR | SAND);
                    let horizontal = (-2..=2).filter(|dx| dry(x + dx, y)).count();
                    let vertical = (-2..=2).filter(|dy| dry(x, y + dy)).count();
                    assert!(
                        matches!((horizontal, vertical), (2, 5) | (5, 2)),
                        "seed {seed}, corridor width at {x},{y}"
                    );
                    corridor_sections += 1;
                }
                if c.ground == WATER {
                    water += 1;
                }
                if let Some(kind) = c.object {
                    let object = content.get(kind).unwrap();
                    assert!(matches!(kind, BOSS | WALL | PALM | EXIT | 0x683..=0x68f));
                    if object.enemy {
                        actors += 1;
                        assert_eq!(c.ground, FLOOR);
                        assert!(c.region >= 3);
                        *room_actors.entry(c.region).or_default() += 1;
                        assert!(
                            wide_seen.contains(&(x, y)),
                            "seed {seed}, narrow door to actor {x},{y}"
                        );
                    }
                    if kind == BOSS {
                        assert!(wide_seen.contains(&(x - 1, y - 1)));
                        assert!((x - sx).abs() + (y - sy).abs() >= 5 * 21);
                    }
                }
                if c.object == Some(WALL) {
                    walls += 1;
                    wall_start = Some((x, y));
                    assert_eq!(c.ground, COMPOSITE);
                    let conn: u32 = c.config.strip_prefix("conn:").unwrap().parse().unwrap();
                    for ((dx, dy), byte) in DIRECTIONS.into_iter().zip(conn.to_be_bytes()) {
                        assert_eq!(
                            byte,
                            if map.cells[index(x + dx, y + dy)].object == Some(WALL) {
                                1
                            } else {
                                2
                            }
                        );
                    }
                } else {
                    assert!(c.config.is_empty());
                }
            }
            assert!((30..=MAX_ACTORS).contains(&actors));
            assert!(water > 0 && walls > 0 && corridor_sections > 0);
            let boss_region = map
                .cells
                .iter()
                .find(|c| c.object == Some(BOSS))
                .unwrap()
                .region;
            assert!((9..=15).contains(&room_actors[&boss_region]));
            for (&region, &count) in &room_actors {
                if region != boss_region {
                    assert!((3..=8).contains(&count));
                }
            }
            let wall_seen = flood(wall_start.unwrap(), |x, y| {
                (0..map.width).contains(&x)
                    && (0..map.height).contains(&y)
                    && map.cells[index(x, y)].object == Some(WALL)
            });
            assert_eq!(
                wall_seen.len(),
                walls,
                "seed {seed}, disconnected wall boundary"
            );
        }
    }

    #[test]
    fn missing_selected_content_is_an_explicit_seeded_error() {
        let mut content = Content::load(std::path::Path::new("data")).unwrap();
        content.objects.remove(&BOSS);
        let error = generate(&content, 562).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("seed 562") && message.contains("0x927"));
        content.grounds.remove(&WATER);
        let error = generate(&content, 563).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("seed 563") && message.contains("0x73"));
    }
}
