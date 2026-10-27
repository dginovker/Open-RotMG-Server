use crate::content::{Cell, Content, Map};
use anyhow::{Context, Result, ensure};
use rand::{Rng, SeedableRng, seq::SliceRandom};

const TREES: [u16; 5] = [0x175, 0x176, 0x177, 0x178, 0x179];
const SPRITES: [u16; 5] = [0xd00, 0xd01, 0xd02, 0xd03, 0xd04];
type Point = (i32, i32);
const DIRECTIONS: [Point; 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
const PITCH: i32 = 16;
const ROOM: i32 = 6;
const ARENA: i32 = 15;

pub fn generate(content: &Content, seed: u64) -> Result<Map> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let (mut rooms, arena) = (0..64)
        .find_map(|_| spine(&mut rng))
        .context("Sprite World arena never fit")?;
    let boss = rooms.len();
    let mut edges: Vec<_> = (0..boss).map(|i| (i, i + 1)).collect();
    rooms.push(arena);
    for _ in 0..rng.gen_range(2..=5) {
        let choices: Vec<_> = rooms
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != 0 && *i != boss)
            .flat_map(|(i, (x, y))| {
                DIRECTIONS
                    .iter()
                    .map(move |(dx, dy)| (i, (x + dx * PITCH, y + dy * PITCH)))
            })
            .filter(|(_, p)| !rooms.contains(p) && apart(*p, arena))
            .collect();
        let (parent, pos) = *choices
            .choose(&mut rng)
            .context("Sprite World branches exhausted")?;
        edges.push((parent, rooms.len()));
        rooms.push(pos);
    }
    let reach = |i: usize| if i == boss { ARENA } else { ROOM };
    let shift = |axis: fn(&Point) -> i32| {
        let low = rooms.iter().enumerate().map(|(i, p)| axis(p) - reach(i));
        12 - PITCH * (10 + low.min().unwrap()).div_euclid(PITCH)
    };
    let (ox, oy) = (shift(|p| p.0), shift(|p| p.1));
    let centers: Vec<_> = rooms.iter().map(|(x, y)| (x + ox, y + oy)).collect();
    let extent = |axis: fn(&Point) -> i32| {
        let high = centers.iter().enumerate().map(|(i, p)| axis(p) + reach(i));
        high.max().unwrap() + 3
    };
    let (width, height) = (extent(|p| p.0), extent(|p| p.1));
    let mut map = Map {
        width,
        height,
        cells: vec![
            Cell {
                ground: 0xfe,
                object: None,
                config: String::new(),
                terrain: 0,
                region: 0,
            };
            (width * height) as usize
        ],
        spawns: vec![],
    };
    for (i, &(cx, cy)) in centers.iter().enumerate() {
        let radius = if i == boss {
            ARENA
        } else if i == 0 {
            6
        } else {
            rng.gen_range(3..=5)
        };
        for y in cy - radius..=cy + radius {
            for x in cx - radius..=cx + radius {
                paint(&mut map, x, y);
            }
        }
    }
    for &(a, b) in &edges {
        let ((x0, y0), (x1, y1)) = (centers[a], centers[b]);
        for y in y0.min(y1)..=y0.max(y1) + 1 {
            for x in x0.min(x1)..=x0.max(x1) + 1 {
                paint(&mut map, x, y);
            }
        }
    }
    let (sx, sy) = centers[0];
    let spawn = &mut map.cells[(sy * width + sx) as usize];
    spawn.region = 1;
    spawn.object = Some(0x70d);
    map.spawns = vec![(sx as f32 + 0.5, sy as f32 + 0.5)];
    let (bx, by) = centers[boss];
    map.cells[(by * width + bx) as usize].object = Some(0xd06);
    map.cells[(by * width + bx + 3) as usize].object = Some(0x70e);
    // The guide bounds the count, not its odds; 20% rolls tune rarity for #678, not recovered RNG:
    // https://web.archive.org/web/20120531054927id_/http://forums.wildshadow.com/wiki/realm-mad-god/sprite-world
    let gods = (0..5).filter(|_| rng.gen_ratio(1, 5)).count();
    let mut hosts: Vec<_> = centers
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != 0 && *i != boss)
        .map(|(_, center)| *center)
        .collect();
    hosts.shuffle(&mut rng);
    for (cx, cy) in hosts.into_iter().take(gods) {
        place(&mut map, cx + 2, cy - 2, 0xd05);
    }
    for (room, &(cx, cy)) in centers.iter().enumerate() {
        if room == 0 || room == boss {
            continue;
        }
        for (n, &(dx, dy)) in [(-2, 0), (2, 1), (0, 2), (-1, -2)].iter().enumerate() {
            place(&mut map, cx + dx, cy + dy, SPRITES[n % SPRITES.len()]);
        }
        for (dx, dy) in [(-3, -3), (3, -3), (-3, 3), (3, 3)] {
            place(
                &mut map,
                cx + dx,
                cy + dy,
                TREES[rng.gen_range(0..TREES.len())],
            );
        }
    }
    // Reconstruct the irregular bands in this later map, not an authenticated 2012 generator:
    // https://github.com/ossimc82/fabiano-swagger-of-doom/blob/6fd20aad4a7905b13f25389c68368a942a2b68cb/wServer/realm/worlds/maps/spriteworld.wmap
    for (i, &(cx, cy)) in centers.iter().enumerate() {
        let radius = if i == boss { ARENA } else { PITCH / 2 };
        for _ in 0..rng.gen_range(radius..=radius * 2) {
            let color = rng.gen_range(0xa1..=0xa4);
            let length = rng.gen_range(3..=radius * 2 + 1);
            let thickness = rng.gen_range(1..=3);
            let (w, h) = if rng.gen_bool(0.5) {
                (length, thickness)
            } else {
                (thickness, length)
            };
            let x0 = rng.gen_range(cx - radius..=cx + radius - w + 1);
            let y0 = rng.gen_range(cy - radius..=cy + radius - h + 1);
            for y in y0..y0 + h {
                for x in x0..x0 + w {
                    let cell = &mut map.cells[(y * width + x) as usize];
                    if cell.ground != 0xfe && ((x - sx).abs() > 1 || (y - sy).abs() > 1) {
                        cell.ground = color;
                    }
                }
            }
        }
    }
    ensure!(map.spawns.len() == 1, "Sprite World lost its entrance");
    ensure!(
        map.cells.iter().filter(|c| c.object == Some(0xd06)).count() == 1
            && map.cells.iter().filter(|c| c.object == Some(0x70e)).count() == 1
            && map.cells.iter().filter(|c| c.object == Some(0x70d)).count() == 1,
        "Sprite World lost a required portal or boss"
    );
    for cell in &map.cells {
        ensure!(
            content.grounds.contains_key(&cell.ground),
            "unknown Sprite World ground"
        );
        if let Some(kind) = cell.object {
            content.get(kind)?;
        }
    }
    tracing::info!(
        seed,
        rooms = rooms.len(),
        gods,
        floor_pattern = "SeededBands",
        width,
        height,
        "generated varied Sprite World"
    );
    Ok(map)
}

fn spine(rng: &mut rand::rngs::StdRng) -> Option<(Vec<Point>, Point)> {
    let mut rooms = vec![(0, 0)];
    for _ in 1..rng.gen_range(5..=8) {
        let (x, y) = *rooms.last()?;
        let next: Vec<_> = DIRECTIONS
            .iter()
            .map(|(dx, dy)| (x + dx * PITCH, y + dy * PITCH))
            .filter(|p| !rooms.contains(p))
            .collect();
        rooms.push(*next.choose(rng)?);
    }
    let (x, y) = *rooms.last()?;
    // The longer final bridge keeps the arena from absorbing the platforms beside it.
    let arenas: Vec<_> = DIRECTIONS
        .iter()
        .map(|(dx, dy)| (x + dx * (PITCH + 8), y + dy * (PITCH + 8)))
        .filter(|&a| rooms.iter().all(|&r| apart(r, a)))
        .collect();
    Some((rooms, *arenas.choose(rng)?))
}

fn apart(room: Point, arena: Point) -> bool {
    let gap = ROOM + ARENA + 2;
    (room.0 - arena.0).abs() > gap || (room.1 - arena.1).abs() > gap
}

fn paint(map: &mut Map, x: i32, y: i32) {
    map.cells[(y * map.width + x) as usize].ground = 0xa0;
}

fn place(map: &mut Map, x: i32, y: i32, kind: u16) {
    let cell = &mut map.cells[(y * map.width + x) as usize];
    if (0xa0..=0xa4).contains(&cell.ground) && cell.object.is_none() {
        cell.object = Some(kind);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn native_sprite_gods_are_uncommon_and_bounded() {
        let content = Content::load(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data"
        )))
        .unwrap();
        let mut counts = [0; 6];
        for seed in 0..1024 {
            let map = generate(&content, seed).unwrap();
            let gods = map.cells.iter().filter(|c| c.object == Some(0xd05)).count();
            counts[gods] += 1;
        }
        eprintln!("Native Sprite God population histogram (0..=5): {counts:?}");
        assert!(counts[0] > 300, "empty dungeons should be common");
        assert!(
            counts[3..].iter().sum::<usize>() < 100,
            "large groups should be rare"
        );
        assert!(counts[1] > counts[2] && counts[2] > counts[3]);
        assert!(
            counts[4] + counts[5] > 0,
            "retain the historical upper range"
        );
    }

    #[test]
    fn sprite_floor_uses_reference_palette() {
        let content = Content::load(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data"
        )))
        .unwrap();
        let mut patterns = BTreeSet::new();
        for seed in 0..16 {
            let map = generate(&content, seed).unwrap();
            assert_eq!(
                serde_json::to_value(&map).unwrap(),
                serde_json::to_value(generate(&content, seed).unwrap()).unwrap()
            );
            let (sx, sy) = map.spawns[0];
            let (sx, sy) = (sx as i32, sy as i32);
            patterns.insert(
                (sy - 6..=sy + 6)
                    .flat_map(|y| (sx - 6..=sx + 6).map(move |x| (x, y)))
                    .map(|(x, y)| map.cells[(y * map.width + x) as usize].ground)
                    .collect::<Vec<_>>(),
            );
            assert_eq!(map.cells[(sy * map.width + sx) as usize].ground, 0xa0);
            assert_eq!(
                map.cells.iter().map(|c| c.ground).collect::<BTreeSet<_>>(),
                BTreeSet::from([0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xfe]),
                "seed {seed}"
            );
        }
        assert!(
            patterns.len() > 1,
            "conveyors repeat the same entrance pattern"
        );
    }

    #[test]
    fn limon_arena_is_a_separate_thirty_one_tile_platform() {
        let content = Content::load(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data"
        )))
        .unwrap();
        for seed in 0..256 {
            let map = generate(&content, seed).unwrap();
            let at = |x: i32, y: i32| &map.cells[(y * map.width + x) as usize];
            let i = map
                .cells
                .iter()
                .position(|c| c.object == Some(0xd06))
                .unwrap() as i32;
            let (lx, ly) = (i % map.width, i / map.width);
            for y in ly - 15..=ly + 15 {
                for x in lx - 15..=lx + 15 {
                    assert!((0xa0..=0xa4).contains(&at(x, y).ground), "seed {seed}");
                }
            }
            let ring = (lx - 16..=lx + 16)
                .flat_map(|x| [(x, ly - 16), (x, ly + 16)])
                .chain((ly - 15..=ly + 15).flat_map(|y| [(lx - 16, y), (lx + 16, y)]))
                .filter(|&(x, y)| at(x, y).ground != 0xfe)
                .count();
            assert_eq!(ring, 2, "seed {seed}: arena joined another platform");
        }
    }
}
