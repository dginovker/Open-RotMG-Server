use crate::{
    content::{Cell, Content, Map},
    encounter::{Motion, Profile},
};
use anyhow::{Context, Result, ensure};
use rand::{Rng, SeedableRng, seq::SliceRandom};
use std::f32::consts::TAU;

pub const PORTAL: u16 = 0x72c;
pub const SPHINX: u16 = 0xd54;
pub const NUT: u16 = 0xd26;
pub const GEB: u16 = 0xd27;
pub const BES: u16 = 0xd28;
pub const BOSSES: [u16; 3] = [NUT, GEB, BES];
pub const STATUES: [u16; 3] = [0xd3d, 0xd3e, 0xd3f];
pub const ANCHOR: u16 = 0xd25;
pub const SARCOPHAGUS: u16 = 0xd29;
pub const TREASURE: u16 = 0xd2c;
pub const SWITCH: u16 = 0xd37;
pub const TRAP: u16 = 0xd38;
pub const DISABLED_TRAP: u16 = 0xd39;
pub const EXIT: u16 = 0xd40;
pub const QUICKSAND: u8 = 0xbf;
pub const MISSING_TILE: u8 = 0xe3;
pub const WALL: u16 = 0x1dd;

pub fn immortal(kind: u16) -> bool {
    matches!(kind, ANCHOR | 0xd31..=0xd39 | 0xd3d..=0xd3f)
}

pub fn initial_condition(kind: u16) -> i32 {
    if matches!(kind, ANCHOR | SWITCH | TRAP | 0xd3d..=0xd3f) {
        crate::condition::bit(crate::condition::INVULNERABLE)
    } else if BOSSES.contains(&kind) {
        crate::condition::bit(crate::condition::ARMORED)
    } else {
        0
    }
}

pub fn contains(kind: u16) -> bool {
    (0xd10..=0xd3f).contains(&kind)
}

pub fn profile(kind: u16) -> Option<Profile> {
    contains(kind).then_some(Profile {
        motion: Motion::Stationary,
        speed: 0.,
        interval: 50,
        range: 30.,
    })
}

pub fn validate(content: &Content, kind: u16) -> Result<()> {
    ensure!(contains(kind), "unknown Tomb actor {kind:#x}");
    let count = match kind {
        NUT => 9,
        GEB => 6,
        BES => 5,
        0xd11 | 0xd13 | 0xd15 | 0xd3c => 4,
        0xd3a => 3,
        0xd1a | 0xd1b | 0xd1c..=0xd1e | 0xd2a | 0xd2b | 0xd3b => 2,
        ANCHOR | TREASURE | 0xd2d..=0xd30 | SWITCH..=0xd3f => 0,
        _ => 1,
    };
    for index in 0..count {
        ensure!(
            content.get(kind)?.projectiles.contains_key(&index),
            "Tomb actor {kind:#x} lacks selected projectile {index}"
        );
    }
    Ok(())
}

fn center(room: usize) -> (i32, i32) {
    (16 + room as i32 % 5 * 32, 16 + room as i32 / 5 * 32)
}
fn index(x: i32, y: i32) -> usize {
    (y * 164 + x) as usize
}
fn adjacent(a: usize, b: usize) -> bool {
    let (x, y) = (a % 5, a / 5);
    let (u, v) = (b % 5, b / 5);
    x.abs_diff(u) + y.abs_diff(v) == 1
        || ((a == 12 || b == 12) && x.abs_diff(u) == 1 && y.abs_diff(v) == 1)
}
fn put(map: &mut Map, x: i32, y: i32, kind: u16) -> Result<()> {
    let cell = &mut map.cells[index(x, y)];
    ensure!(
        cell.ground != 0xff && cell.object.is_none(),
        "blocked Tomb placement {kind:#x} at {x},{y}"
    );
    cell.object = Some(kind);
    Ok(())
}

pub fn generate(content: &Content, seed: u64) -> Result<Map> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let rooms: Vec<usize> = (0..25).filter(|r| ![7, 11, 13, 17].contains(r)).collect();
    let mut joined = vec![12];
    let mut links = Vec::new();
    while joined.len() < rooms.len() {
        let frontier: Vec<_> = joined
            .iter()
            .flat_map(|a| rooms.iter().map(move |b| (*a, *b)))
            .filter(|(a, b)| !joined.contains(b) && adjacent(*a, *b))
            .collect();
        let &(a, b) = frontier
            .choose(&mut rng)
            .context("Tomb room graph disconnected")?;
        links.push((a, b));
        joined.push(b);
    }
    let degree = |room: usize, links: &[(usize, usize)]| {
        links
            .iter()
            .filter(|(a, b)| *a == room || *b == room)
            .count()
    };
    while rooms
        .iter()
        .filter(|r| **r != 12 && degree(**r, &links) == 1)
        .count()
        > 5
    {
        let choices: Vec<_> = rooms
            .iter()
            .filter(|r| **r != 12 && degree(**r, &links) == 1)
            .flat_map(|a| rooms.iter().map(move |b| (*a, *b)))
            .filter(|(a, b)| {
                adjacent(*a, *b) && !links.contains(&(*a, *b)) && !links.contains(&(*b, *a))
            })
            .collect();
        let edge = *choices
            .choose(&mut rng)
            .context("Tomb dead end cannot be connected")?;
        links.push(edge);
    }
    let mut events: Vec<_> = rooms
        .iter()
        .copied()
        .filter(|r| *r != 12 && degree(*r, &links) == 1)
        .collect();
    let mut other: Vec<_> = rooms
        .iter()
        .copied()
        .filter(|r| *r != 12 && !events.contains(r))
        .collect();
    other.shuffle(&mut rng);
    events.extend(other.into_iter().take(5 - events.len()));
    events.shuffle(&mut rng);
    // Prefer the September 2012 guide's guaranteed treasure over later rare-room rules.
    events.truncate(rng.gen_range(3..=5));
    let choices: Vec<_> = rooms
        .iter()
        .copied()
        .filter(|r| *r != 12 && !events.contains(r))
        .collect();
    let treasure = *choices
        .choose(&mut rng)
        .context("Tomb treasure room site missing")?;
    let treasures = [treasure];
    events.push(treasure);
    let mut map = Map {
        width: 164,
        height: 164,
        cells: vec![
            Cell {
                ground: 0xff,
                object: None,
                config: String::new(),
                terrain: 0,
                region: 0
            };
            164 * 164
        ],
        spawns: vec![(80.5, 81.5)],
    };
    // The original generator was not recovered; do not import unlicensed community maps.
    for &room in &rooms {
        let (cx, cy) = center(room);
        let (rx, ry) = if room == 12 {
            (19, 19)
        } else if events.contains(&room) {
            (10, 10)
        } else {
            (rng.gen_range(7..=10), rng.gen_range(7..=10))
        };
        for y in cy - ry..=cy + ry {
            for x in cx - rx..=cx + rx {
                if treasures.contains(&room) {
                    if (x - cx).pow(2) + (y - cy).pow(2) <= 100 {
                        map.cells[index(x, y)].ground = 0xe4;
                    }
                } else {
                    map.cells[index(x, y)].ground = if rng.gen_bool(0.15) { 0xe2 } else { 0xe0 };
                }
            }
        }
    }
    for &(a, b) in &links {
        let (mut x, mut y) = center(a);
        let (tx, ty) = center(b);
        let horizontal = rng.gen_bool(0.5);
        while (x, y) != (tx, ty) {
            for dy in -1..=0 {
                for dx in -1..=0 {
                    map.cells[index(x + dx, y + dy)].ground = 0xe0;
                }
            }
            if x != tx && (horizontal || y == ty) {
                x += (tx - x).signum();
            } else {
                y += (ty - y).signum();
            }
        }
    }
    for &room in &rooms {
        let (cx, cy) = center(room);
        if room == 12 {
            // Sandy tracks match selected Missing Tile, not a sinking moat: https://www.youtube.com/watch?v=Igu8lFDRHg0&t=20s (123.4.4 footage; this footprint remains reconstructed).
            for y in cy - 18..=cy + 18 {
                for x in cx - 18..=cx + 18 {
                    let (dx, dy) = ((x - cx).abs(), (y - cy).abs());
                    if (64..=169).contains(&(dx * dx + dy * dy)) {
                        map.cells[index(x, y)].ground = MISSING_TILE;
                    }
                }
            }
            for (dx, dy) in [(-15, -13), (15, -13), (-15, 13), (15, 13)] {
                for y in cy + dy - 1..=cy + dy + 1 {
                    for x in cx + dx - 1..=cx + dx + 1 {
                        if x != cx + dx || y != cy + dy {
                            map.cells[index(x, y)].object = Some(WALL);
                        }
                        map.cells[index(x, y)].ground = 0xff;
                    }
                }
            }
            for (dx, dy) in [(-7, -7), (7, -7), (-7, 7), (7, 7)] {
                put(&mut map, cx + dx, cy + dy, WALL)?;
            }
            put(&mut map, cx, cy, ANCHOR)?;
            put(&mut map, cx, cy + 1, EXIT)?;
            for (dx, dy, kind) in [
                (-3, -2, STATUES[0]),
                (-2, 4, STATUES[1]),
                (4, 0, STATUES[2]),
            ] {
                put(&mut map, cx + dx, cy + dy, kind)?;
            }
            map.cells[index(cx, cy + 1)].region = 1;
            continue;
        }
        if !treasures.contains(&room) {
            let bend = rng.gen_range(0..7);
            for y in cy - 6..=cy + 6 {
                for x in cx - 6..=cx + 6 {
                    if (x - cx + 2 * (y - cy) + bend).rem_euclid(13) < 3 {
                        map.cells[index(x, y)].ground = MISSING_TILE;
                    }
                }
            }
        }
        if events.contains(&room) {
            let treasure = treasures.contains(&room);
            if !treasure {
                for y in cy - 9..=cy + 9 {
                    for x in cx - 9..=cx + 9 {
                        map.cells[index(x, y)].ground = MISSING_TILE;
                    }
                }
            }
            put(
                &mut map,
                cx,
                cy,
                if treasure { TREASURE } else { SARCOPHAGUS },
            )?;
            if !treasure {
                let count = rng.gen_range(8..=12);
                for n in 0..count {
                    let angle = n as f32 * TAU / count as f32;
                    let radius = if n % 2 == 0 { 3. } else { 5. };
                    put(
                        &mut map,
                        cx + (radius * angle.cos()).round() as i32,
                        cy + (radius * angle.sin()).round() as i32,
                        if n % 2 == 0 { 0xd2a } else { 0xd2b },
                    )?;
                }
            }
            continue;
        }
        let family = room % 7;
        if family == 0 {
            put(&mut map, cx, cy, SWITCH)?;
            for (dx, dy) in [
                (-4, -4),
                (0, -5),
                (4, -4),
                (5, 0),
                (4, 4),
                (0, 5),
                (-4, 4),
                (-5, 0),
            ] {
                put(&mut map, cx + dx, cy + dy, TRAP)?;
            }
        } else {
            let pack: &[u16] = match family {
                1 => &[0xd11, 0xd13, 0xd15],
                2 => &[0xd19, 0xd19],
                3 => &[0xd3a, 0xd3b, 0xd3c],
                4 => &[0xd1a, 0xd1a, 0xd1b],
                5 => &[0xd31, 0xd32, 0xd33],
                _ => &[0xd34, 0xd35, 0xd36],
            };
            for (n, kind) in pack.iter().enumerate() {
                put(&mut map, cx - 3 + n as i32 * 3, cy, *kind)?;
            }
        }
    }
    for &(a, b) in &links {
        let (ax, ay) = center(a);
        let (bx, by) = center(b);
        if ax == bx || ay == by {
            let (x, y) = ((ax + bx) / 2, (ay + by) / 2);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let c = &mut map.cells[index(x + dx, y + dy)];
                    if c.ground != 0xff && c.object.is_none() {
                        c.ground = QUICKSAND;
                    }
                }
            }
        }
    }
    for y in 1..163 {
        for x in 1..163 {
            if map.cells[index(x, y)].ground == 0xff
                && map.cells[index(x, y)].object.is_none()
                && [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]
                    .into_iter()
                    .any(|(x, y)| map.cells[index(x, y)].ground != 0xff)
            {
                map.cells[index(x, y)].object = Some(WALL);
            }
        }
    }
    // ConnectedWall.draw requires a valid shape before its first frame, including isolated pillars.
    for y in 0..map.height {
        for x in 0..map.width {
            if map.cells[index(x, y)].object != Some(WALL) {
                continue;
            }
            let mut connection = 0_u32;
            for (nx, ny) in [(x, y - 1), (x + 1, y), (x, y + 1), (x - 1, y)] {
                let joined = (0..map.width).contains(&nx)
                    && (0..map.height).contains(&ny)
                    && map.cells[index(nx, ny)].object == Some(WALL);
                connection = (connection << 8) | if joined { 1 } else { 2 };
            }
            map.cells[index(x, y)].config = format!("conn:{connection}");
        }
    }
    for c in &map.cells {
        ensure!(
            content.grounds.contains_key(&c.ground),
            "unknown Tomb ground {:#x}",
            c.ground
        );
        if let Some(kind) = c.object {
            content.get(kind)?;
        }
    }
    tracing::info!(
        seed,
        rooms = rooms.len(),
        treasures = treasures.len(),
        sarcophagi = map
            .cells
            .iter()
            .filter(|c| c.object == Some(SARCOPHAGUS))
            .count(),
        "generated reconstructed Tomb layout"
    );
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, VecDeque};

    #[test]
    fn footage_layout_keeps_selected_floors_hollow_pillars_and_connected_two_tile_passages() {
        let content = Content::load(std::path::Path::new("data")).unwrap();
        assert_eq!(content.grounds[&MISSING_TILE], 0.9);
        assert_eq!(content.grounds[&QUICKSAND], 0.6);
        for seed in 0..32 {
            let map = generate(&content, seed).unwrap();
            let count = |kind| map.cells.iter().filter(|c| c.object == Some(kind)).count();
            assert!((3..=5).contains(&count(SARCOPHAGUS)));
            assert_eq!(count(TREASURE), 1);
            assert_eq!(count(ANCHOR), 1);
            assert_eq!(count(EXIT), 1);
            for kind in STATUES {
                assert_eq!(count(kind), 1);
            }
            assert_eq!(map.cells[index(80, 70)].ground, MISSING_TILE);
            assert!(map.cells.iter().any(|c| c.ground == QUICKSAND));
            for (dx, dy) in [(-15, -13), (15, -13), (-15, 13), (15, 13)] {
                for y in -1..=1 {
                    for x in -1..=1 {
                        let c = &map.cells[index(80 + dx + x, 80 + dy + y)];
                        assert_eq!(c.ground, 0xff);
                        assert_eq!(c.object, (x != 0 || y != 0).then_some(WALL));
                    }
                }
            }
            for c in map.cells.iter().filter(|c| c.object == Some(WALL)) {
                let conn: u32 = c.config.strip_prefix("conn:").unwrap().parse().unwrap();
                assert!(conn.to_be_bytes().into_iter().all(|v| matches!(v, 1 | 2)));
            }
            let passable = |x: i32, y: i32| {
                (0..map.width).contains(&x) && (0..map.height).contains(&y) && {
                    let c = &map.cells[index(x, y)];
                    content.grounds[&c.ground] > 0. && c.object != Some(WALL)
                }
            };
            let mut seen = BTreeSet::from([(80, 81)]);
            let mut queue = VecDeque::from([(80, 81)]);
            while let Some((x, y)) = queue.pop_front() {
                for p in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
                    if passable(p.0, p.1) && seen.insert(p) {
                        queue.push_back(p);
                    }
                }
            }
            for (i, c) in map.cells.iter().enumerate() {
                if c.object.is_some_and(|kind| kind != WALL) {
                    assert!(
                        seen.contains(&(i as i32 % map.width, i as i32 / map.width)),
                        "seed {seed}, cell {i}"
                    );
                }
                if c.object == Some(SARCOPHAGUS) {
                    assert_eq!(c.ground, MISSING_TILE, "seed {seed}, Sarcophagus cell {i}");
                }
            }
            let mut passages = 0;
            for room in (0..25).filter(|r| ![7, 11, 12, 13, 17].contains(r)) {
                let (cx, cy) = center(room);
                if room % 5 < 4 && passable(cx + 16, cy) {
                    assert_eq!((-3..=3).filter(|dy| passable(cx + 16, cy + dy)).count(), 2);
                    passages += 1;
                }
                if room / 5 < 4 && passable(cx, cy + 16) {
                    assert_eq!((-3..=3).filter(|dx| passable(cx + dx, cy + 16)).count(), 2);
                    passages += 1;
                }
            }
            assert!(passages > 0);
        }
    }
}
