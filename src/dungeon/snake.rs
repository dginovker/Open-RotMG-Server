use crate::content::{Cell, Content, Map};
use anyhow::{Context, Result, ensure};
use rand::{Rng, SeedableRng, seq::SliceRandom};

pub const BOSS: u16 = 0x917;
pub const PET: u16 = 0x918;
pub const SWARM: u16 = 0x919;
pub const GUARD: u16 = 0xe26;
pub const BUTTON: u16 = 0xe27;
pub const SPAWNER: u16 = 0xe28;
pub const DART: u16 = 0xe25;
pub const GUARD_RAGE_HP: i32 = 3300;
const TREASURE_ENTRY_RISE: i32 = 12;

// The modified 7.0 server provides explicit timings, not an authenticated 123.5.1 AI script.
pub fn phase(elapsed: u64) -> (u64, u64) {
    let cycle = elapsed / 31_000;
    let t = elapsed % 31_000;
    let (phase, age) = match t {
        0..8000 => (0, t),
        8000..25000 => (1, t - 8000),
        _ => (2, t - 25000),
    };
    (cycle * 3 + phase, age)
}

pub fn initial_condition(kind: u16) -> i32 {
    match kind {
        BOSS => crate::condition::bit(crate::condition::INVULNERABLE),
        BUTTON | SPAWNER | DART => crate::condition::bit(crate::condition::INVINCIBLE),
        _ => 0,
    }
}

pub fn shield(elapsed: u64) -> bool {
    let (stage, age) = phase(elapsed);
    match stage % 3 {
        0 => age % 3000 < 2000,
        1 => age >= 16000 || !(1000..2000).contains(&(age % 3000)),
        _ => (1500..3500).contains(&age) || age >= 5000,
    }
}

pub fn generate(content: &Content, seed: u64) -> Result<Map> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let mut treasure_rng = rand::rngs::StdRng::seed_from_u64(seed ^ 0x7472_6561_7375_7265);
    let mut attempts = 0;
    let (edges, ordinary, spaces, half, centers) = loop {
        attempts += 1;
        ensure!(
            attempts <= 100,
            "Snake Pit rooms could not be separated for seed {seed}"
        );
        let directions = [(1, 0), (-1, 0), (0, 1), (0, -1)];
        let mut rooms = vec![(0i32, 0i32)];
        let mut edges = Vec::new();
        for parent in 0..6 {
            let (x, y) = rooms[parent];
            let choices: Vec<_> = directions
                .iter()
                .map(|(dx, dy)| (x + dx, y + dy))
                .filter(|p| !rooms.contains(p))
                .collect();
            rooms.push(
                *choices
                    .choose(&mut rng)
                    .context("Snake Pit spine exhausted")?,
            );
            edges.push((parent, parent + 1));
        }
        for _ in 0..rng.gen_range(3..=8) {
            let choices: Vec<_> = rooms
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != 0 && *i != 6)
                .flat_map(|(i, (x, y))| {
                    directions.iter().map(move |(dx, dy)| (i, (x + dx, y + dy)))
                })
                .filter(|(_, p)| !rooms.contains(p))
                .collect();
            let (parent, pos) = *choices
                .choose(&mut rng)
                .context("Snake Pit branches exhausted")?;
            edges.push((parent, rooms.len()));
            rooms.push(pos);
        }
        let ordinary = rooms.len();
        let mut candidates: Vec<_> = (1..ordinary)
            .filter(|&i| i != 6)
            .flat_map(|i| [-1, 1].map(|dx| (i, (rooms[i].0 + dx, rooms[i].1))))
            .filter(|&(parent, pos)| {
                !rooms.contains(&pos)
                    && rooms.iter().enumerate().all(|(i, &(x, y))| {
                        i == parent || (x - pos.0).abs() + (y - pos.1).abs() > 1
                    })
            })
            .collect();
        candidates.shuffle(&mut treasure_rng);
        let mut spaces: Vec<(usize, (i32, i32))> = Vec::new();
        for candidate in candidates {
            if spaces
                .iter()
                .all(|&(_, (x, y))| (x - candidate.1.0).abs() + (y - candidate.1.1).abs() > 1)
            {
                spaces.push(candidate);
                if spaces.len() == 3 {
                    break;
                }
            }
        }
        for &(parent, pos) in &spaces[..treasure_rng.gen_range(0..=spaces.len())] {
            edges.push((parent, rooms.len()));
            rooms.push(pos);
        }
        let half: Vec<(i32, i32)> = (0..rooms.len())
            .map(|i| match i {
                6 => (15, 15),
                i if i >= ordinary => (7, 16),
                _ => {
                    let r = rng.gen_range(7..=9);
                    (r, r)
                }
            })
            .collect();
        let mut centers = vec![(0, 0); rooms.len()];
        for &(a, b) in &edges {
            let (dx, dy) = (rooms[b].0 - rooms[a].0, rooms[b].1 - rooms[a].1);
            // Size necks from room edges; the 2012 footage shows short links, not long grid gaps: https://www.youtube.com/watch?v=gWLn80rNsxU&t=60
            let step = 1 + rng.gen_range(5..=7);
            centers[b] = (
                centers[a].0 + dx * (half[a].0 + half[b].0 + step),
                centers[a].1 + dy * (half[a].1 + half[b].1 + step),
            );
        }
        let body_y = |i: usize| {
            centers[i].1
                - if i >= ordinary {
                    TREASURE_ENTRY_RISE
                } else {
                    0
                }
        };
        let apart = |i: usize, j: usize| {
            (centers[i].0 - centers[j].0).abs() >= half[i].0 + half[j].0 + 3
                || (body_y(i) - body_y(j)).abs() >= half[i].1 + half[j].1 + 3
        };
        if (0..rooms.len()).all(|i| (0..i).all(|j| apart(i, j))) {
            break (edges, ordinary, spaces, half, centers);
        }
    };
    let min_x = centers.iter().map(|p| p.0).min().unwrap();
    let min_y = centers
        .iter()
        .enumerate()
        .map(|(i, p)| {
            p.1 - if i >= ordinary {
                TREASURE_ENTRY_RISE
            } else {
                0
            }
        })
        .min()
        .unwrap();
    let centers: Vec<_> = centers
        .iter()
        .map(|(x, y)| (x - min_x + 20, y - min_y + 20))
        .collect();
    let width = centers.iter().map(|p| p.0).max().unwrap() + 21;
    let height = centers.iter().map(|p| p.1).max().unwrap() + 21;
    let mut map = Map {
        width,
        height,
        cells: vec![
            Cell {
                ground: 0xff,
                object: None,
                config: String::new(),
                terrain: 0,
                region: 0
            };
            (width * height) as usize
        ],
        spawns: vec![(centers[0].0 as f32 + 0.5, centers[0].1 as f32 + 0.5)],
    };
    // The April 2012 run shows a notched square; sizes match the pinned FSoD map: https://www.youtube.com/watch?v=gWLn80rNsxU&t=120
    let boss = |x: i32, y: i32| x.abs().max(y.abs()) <= 15 && x.abs().min(y.abs()) < 12;
    for (i, &(cx, cy)) in centers.iter().enumerate() {
        if i >= ordinary {
            treasure_room(&mut map, cx, cy);
            continue;
        }
        let radius = half[i].0;
        for y in -radius..=radius {
            for x in -radius..=radius {
                let inside = if i == 6 {
                    boss(x, y)
                } else {
                    x * x + y * y <= radius * radius
                };
                if !inside {
                    continue;
                }
                let cell = &mut map.cells[((cy + y) * width + cx + x) as usize];
                cell.ground = if i == 6
                    && [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]
                        .iter()
                        .any(|&(x, y)| !boss(x, y))
                {
                    0x32
                } else {
                    match rng.gen_range(0..10) {
                        0 => 0x1e,
                        1 => 0x1f,
                        _ => 0xca,
                    }
                };
            }
        }
        if i != 0 && i != 6 {
            for (dx, dy) in [(-4, 0), (4, 0)] {
                map.cells[((cy + dy) * width + cx + dx) as usize].object = Some(0x173);
            }
            for (dx, dy) in [(-4, -4), (4, -4), (-4, 4), (4, 4)] {
                map.cells[((cy + dy) * width + cx + dx) as usize].object =
                    Some(rng.gen_range(0x225..=0x229));
            }
        }
    }
    // Narrow bands follow the later map; the original 123.5.1 generator is unrecovered.
    // https://github.com/ossimc82/fabiano-swagger-of-doom/blob/6fd20aad4a7905b13f25389c68368a942a2b68cb/wServer/realm/worlds/maps/snakepit.wmap
    for &(a, b) in &edges {
        let ((x0, y0), (x1, y1)) = (centers[a], centers[b]);
        for y in
            y0.min(y1) - if y0 == y1 { 2 } else { 0 }..=y0.max(y1) + if y0 == y1 { 1 } else { 0 }
        {
            for x in x0.min(x1) - if x0 == x1 { 2 } else { 0 }
                ..=x0.max(x1) + if x0 == x1 { 1 } else { 0 }
            {
                let cell = &mut map.cells[(y * width + x) as usize];
                if cell.ground == 0xff {
                    cell.ground = 0x16;
                    cell.object = Some(0x172);
                }
            }
        }
    }
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let index = (y * width + x) as usize;
            if map.cells[index].ground == 0xff
                && [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]
                    .iter()
                    .any(|(x, y)| map.cells[(y * width + x) as usize].ground != 0xff)
            {
                map.cells[index].object = Some(match rng.gen_range(0..10) {
                    0 => 0x1ec,
                    1 => 0x1ed,
                    _ => 0x170,
                });
            }
        }
    }
    let (sx, sy) = centers[0];
    for y in -2i32..=2 {
        for x in -2i32..=2 {
            if x.abs() + y.abs() <= 2 {
                map.cells[((sy + y) * width + sx + x) as usize].ground = 0x32;
            }
        }
    }
    map.cells[(sy * width + sx) as usize].region = 1;
    map.cells[(sy * width + sx) as usize].object = Some(0x703);
    let (bx, by) = centers[6];
    // Boss cover is permanent, unlike tunnel blocks: https://bluenosersguide.weebly.com/snake-pit.html
    for (dx, dy) in [(-6, 0), (6, 0), (0, -6), (0, 6)] {
        for along in -2..=2 {
            for thick in 0..2 {
                let (x, y) = if dx == 0 {
                    (bx + along, by + dy + thick * dy.signum())
                } else {
                    (bx + dx + thick * dx.signum(), by + along)
                };
                map.cells[(y * width + x) as usize].ground = 0x16;
                map.cells[(y * width + x) as usize].object = Some(0x170);
            }
        }
    }
    map.cells[(by * width + bx) as usize].object = Some(BOSS);
    for cell in &map.cells {
        ensure!(
            content.grounds.contains_key(&cell.ground),
            "unknown Snake Pit ground"
        );
        if let Some(kind) = cell.object {
            content.get(kind)?;
        }
    }
    tracing::info!(
        seed,
        rooms = centers.len(),
        treasure_rooms = centers.len() - ordinary,
        attempts,
        treasure_slots = spaces.len(),
        width,
        height,
        "generated branching Snake Pit with diggable tunnels"
    );
    Ok(map)
}

fn treasure_room(map: &mut Map, cx: i32, cy: i32) {
    // Floor rails, a button alcove and a lower side entry are visible here; dimensions are reconstructed: https://www.youtube.com/watch?v=o90GtXTf9Mw&t=720
    let cy = cy - TREASURE_ENTRY_RISE;
    let inside = |x: i32, y: i32| {
        y.abs() <= 16
            && x.abs()
                <= match y {
                    ..=-11 | 7..=9 => 2,
                    _ => 7,
                }
    };
    for dy in -16i32..=16 {
        for dx in -7i32..=7 {
            if !inside(dx, dy) {
                continue;
            }
            let edge = [(dx - 1, dy), (dx + 1, dy), (dx, dy - 1), (dx, dy + 1)]
                .iter()
                .any(|&(x, y)| !inside(x, y));
            map.cells[((cy + dy) * map.width + cx + dx) as usize].ground = if edge {
                0x32
            } else if dx.abs() <= 1 && (dy + 12).abs() <= 1 {
                0xc3
            } else {
                0xca
            };
        }
    }
    map.cells[((cy - 12) * map.width + cx) as usize].object = Some(BUTTON);
    map.cells[(cy * map.width + cx) as usize].object = Some(SPAWNER);
    for dy in [-4, -2, 0, 2, 4] {
        for dx in [-7, 7] {
            map.cells[((cy + dy) * map.width + cx + dx) as usize].object = Some(DART);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, VecDeque};

    #[test]
    fn treasure_rooms_have_walkable_rails_and_a_lower_entry() {
        let content = Content::load(std::path::Path::new("data")).unwrap();
        let mut treasures = 0;
        for seed in 0..512 {
            let map = generate(&content, seed).unwrap();
            for (i, _) in map
                .cells
                .iter()
                .enumerate()
                .filter(|(_, c)| c.object == Some(SPAWNER))
            {
                let (cx, cy) = (i as i32 % map.width, i as i32 / map.width);
                treasures += 1;
                let inside = |x: i32, y: i32| {
                    y.abs() <= 16
                        && x.abs()
                            <= match y {
                                ..=-11 | 7..=9 => 2,
                                _ => 7,
                            }
                };
                assert!(cx > 7 && cy > 16 && cy + 16 < map.height - 1);
                for dy in -16..=16 {
                    for dx in -7..=7 {
                        let cell = &map.cells[((cy + dy) * map.width + cx + dx) as usize];
                        assert_eq!(
                            cell.ground != 0xff,
                            inside(dx, dy),
                            "seed {seed} outline {dx},{dy}"
                        );
                        if !inside(dx, dy) {
                            continue;
                        }
                        let edge = [(dx - 1, dy), (dx + 1, dy), (dx, dy - 1), (dx, dy + 1)]
                            .iter()
                            .any(|&(x, y)| !inside(x, y));
                        assert_eq!(cell.ground == 0x32, edge, "seed {seed} rail {dx},{dy}");
                        assert_ne!(
                            cell.object,
                            Some(0x170),
                            "seed {seed} blocked room {dx},{dy}"
                        );
                    }
                }
                assert_eq!(
                    map.cells[((cy - 12) * map.width + cx) as usize].object,
                    Some(BUTTON)
                );
            }
        }
        assert!(treasures > 0);
    }

    #[test]
    fn stheno_room_is_a_notched_square() {
        let content = Content::load(std::path::Path::new("data")).unwrap();
        for seed in 0..8 {
            let map = generate(&content, seed).unwrap();
            let boss = map
                .cells
                .iter()
                .position(|c| c.object == Some(BOSS))
                .unwrap() as i32;
            let (bx, by) = (boss % map.width, boss / map.width);
            for dy in -16i32..=16 {
                for dx in -16i32..=16 {
                    let (near, far) = (dx.abs().min(dy.abs()), dx.abs().max(dy.abs()));
                    let cell = &map.cells[((by + dy) * map.width + bx + dx) as usize];
                    let inside = far <= 15 && near < 12;
                    let edge = inside && (far == 15 || near == 11 && far >= 12);
                    if far <= 15 {
                        assert_eq!(cell.ground != 0xff, inside, "seed {seed} at {dx},{dy}");
                    } else if near > 2 {
                        assert_eq!(cell.ground, 0xff, "seed {seed} outside {dx},{dy}");
                    }
                    assert_eq!(cell.ground == 0x32, edge, "seed {seed} border {dx},{dy}");
                }
            }
        }
    }

    #[test]
    fn connectors_are_short_four_tile_necks_that_reach_every_room_after_digging() {
        let content = Content::load(std::path::Path::new("data")).unwrap();
        for seed in 0..512 {
            let map = generate(&content, seed).unwrap();
            let count = |kind| map.cells.iter().filter(|c| c.object == Some(kind)).count();
            assert_eq!(count(BOSS), 1);
            assert_eq!(count(0x703), 1);
            assert_eq!(count(GUARD), 0);
            assert!(count(SPAWNER) <= 3);
            assert_eq!(count(BUTTON), count(SPAWNER));
            let pythons = map
                .cells
                .iter()
                .filter(|c| matches!(c.object, Some(0x225..=0x229)))
                .count();
            assert!((32..=52).contains(&pythons) && pythons % 4 == 0);
            assert_eq!(count(0x173), pythons / 2);
            assert_eq!(map.spawns.len(), 1);
            let neighbors = |i: usize| {
                let (x, y) = (i as i32 % map.width, i as i32 / map.width);
                [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]
                    .into_iter()
                    .filter(|&(x, y)| (0..map.width).contains(&x) && (0..map.height).contains(&y))
                    .map(|(x, y)| (y * map.width + x) as usize)
                    .collect::<Vec<_>>()
            };
            let mut soft: BTreeSet<_> = map
                .cells
                .iter()
                .enumerate()
                .filter(|(_, c)| c.object == Some(0x172))
                .map(|(i, _)| i)
                .collect();
            assert!(!soft.is_empty());
            while let Some(start) = soft.pop_first() {
                let mut queue = VecDeque::from([start]);
                let (mut x0, mut x1, mut y0, mut y1) = (map.width, 0, map.height, 0);
                while let Some(i) = queue.pop_front() {
                    let (x, y) = (i as i32 % map.width, i as i32 / map.width);
                    x0 = x0.min(x);
                    x1 = x1.max(x);
                    y0 = y0.min(y);
                    y1 = y1.max(y);
                    for n in neighbors(i) {
                        if soft.remove(&n) {
                            queue.push_back(n);
                        }
                    }
                }
                let (short, long) = (
                    (x1 - x0 + 1).min(y1 - y0 + 1),
                    (x1 - x0 + 1).max(y1 - y0 + 1),
                );
                assert!(
                    short == 4 && (5..=9).contains(&long),
                    "seed {seed}, soft component {x0},{y0}..{x1},{y1}"
                );
            }
            let passable = |i: usize| {
                content.grounds[&map.cells[i].ground] > 0.
                    && !matches!(map.cells[i].object, Some(0x170 | 0x1ec | 0x1ed))
            };
            let (x, y) = map.spawns[0];
            let start = y.floor() as usize * map.width as usize + x.floor() as usize;
            let mut seen = BTreeSet::from([start]);
            let mut queue = VecDeque::from([start]);
            while let Some(i) = queue.pop_front() {
                for n in neighbors(i) {
                    if passable(n) && seen.insert(n) {
                        queue.push_back(n);
                    }
                }
            }
            for i in 0..map.cells.len() {
                assert!(
                    !passable(i) || seen.contains(&i),
                    "seed {seed}, disconnected post-dig floor {i}"
                );
            }
        }
    }
}
