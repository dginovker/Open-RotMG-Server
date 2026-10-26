use crate::content::{Cell, Content, Map};
use anyhow::{Context, Result, ensure};
use rand::{Rng, SeedableRng, seq::SliceRandom};

#[derive(Clone, Copy)]
struct Room {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    depth: usize,
    round: bool,
}
impl Room {
    fn center(self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }
    fn overlaps(self, b: Self, gap: i32) -> bool {
        self.x <= b.x + b.w + gap
            && b.x <= self.x + self.w + gap
            && self.y <= b.y + b.h + gap
            && b.y <= self.y + self.h + gap
    }
}

// The 2012 example layout leaves void around chambers instead of packing them wall to wall: https://web.archive.org/web/20120425182543/http://forums.wildshadow.com/wiki/realm-mad-god/undead-lair
const GAP: i32 = 4;

// A lair room holds one enemy group, as in the map this server shipped before the
// procedural generator (data/undead-lair.wmap): skeletons, mummies, constructs, slimes,
// reapers and vampires each keep their own rooms.
const FAMILIES: [&[u16]; 6] = [
    &[0xd91, 0xd92, 0xd93, 0xd94, 0xd95],
    &[0xd96, 0xd97, 0xd98],
    &[0xd99, 0xd9a],
    &[0xd9b, 0xd9e],
    &[0xda4],
    &[0xda5, 0xda6],
];

// Period guides establish the room shapes, not the lost official generator's distributions.
pub fn generate(content: &Content, seed: u64) -> Result<Map> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let mut rooms = vec![Room {
        x: 0,
        y: 0,
        w: 27,
        h: 27,
        depth: 0,
        round: true,
    }];
    let mut edges: Vec<Room> = vec![];
    let mut entrance = None;
    for number in 1..rng.gen_range(32..=40) {
        let round = rng.gen_bool(0.1);
        // Broader chambers relieve crowding without adding long corridors: https://web.archive.org/web/20120425182543/http://forums.wildshadow.com/wiki/realm-mad-god/undead-lair
        let w = if round { 17 } else { rng.gen_range(13..=21) };
        let h = if round { w } else { rng.gen_range(13..=21) };
        let mut candidates = vec![];
        for (parent, p) in rooms.iter().enumerate() {
            if p.depth == 10 || entrance == Some(parent) || parent == 0 && number > 1 {
                continue;
            }
            let (cx, cy) = p.center();
            for horizontal in [true, false] {
                for positive in [true, false] {
                    for offset in [-2, 0, 2] {
                        let mut r = Room {
                            x: cx - w / 2,
                            y: cy - h / 2,
                            w,
                            h,
                            depth: p.depth + 1,
                            round,
                        };
                        let corridor = if horizontal {
                            r.x = if positive {
                                p.x + p.w + GAP
                            } else {
                                p.x - w - GAP
                            };
                            r.y += offset;
                            Room {
                                x: if positive { p.x + p.w - 3 } else { r.x + w - 3 },
                                y: cy + offset - 1,
                                w: GAP + 6,
                                h: 3,
                                depth: 0,
                                round: false,
                            }
                        } else {
                            r.y = if positive {
                                p.y + p.h + GAP
                            } else {
                                p.y - h - GAP
                            };
                            r.x += offset;
                            Room {
                                x: cx + offset - 1,
                                y: if positive { p.y + p.h - 3 } else { r.y + h - 3 },
                                w: 3,
                                h: GAP + 6,
                                depth: 0,
                                round: false,
                            }
                        };
                        if rooms.iter().any(|o| r.overlaps(*o, GAP - 1))
                            || edges.iter().any(|e| r.overlaps(*e, 0))
                            || rooms
                                .iter()
                                .enumerate()
                                .any(|(i, o)| i != parent && corridor.overlaps(*o, 0))
                        {
                            continue;
                        }
                        candidates.push((r, corridor));
                    }
                }
            }
        }
        if entrance.is_none() {
            let depth = candidates
                .iter()
                .map(|(r, _)| r.depth)
                .max()
                .context("Undead Lair has no frontier")?;
            candidates.retain(|(r, _)| r.depth == depth);
        }
        let (room, corridor) = *candidates
            .choose(&mut rng)
            .with_context(|| format!("Undead Lair seed {seed}: no placement for room {number}"))?;
        if room.depth == 10 && entrance.is_none() {
            entrance = Some(number);
        }
        rooms.push(room);
        edges.push(corridor);
    }
    let entrance = entrance.context("Undead Lair has no entrance ten rooms from boss")?;
    let min_x = rooms.iter().map(|r| r.x).min().unwrap() - 2;
    let min_y = rooms.iter().map(|r| r.y).min().unwrap() - 2;
    for room in rooms.iter_mut().chain(&mut edges) {
        room.x -= min_x;
        room.y -= min_y;
    }
    let width = rooms.iter().map(|r| r.x + r.w).max().unwrap() + 2;
    let height = rooms.iter().map(|r| r.y + r.h).max().unwrap() + 2;
    ensure!(
        width <= 512 && height <= 512,
        "Undead Lair seed {seed}: map exceeds bounds"
    );
    let mut map = Map {
        width,
        height,
        cells: vec![
            Cell {
                ground: 0xfe,
                object: None,
                config: String::new(),
                terrain: 0,
                region: 0
            };
            (width * height) as usize
        ],
        spawns: vec![],
    };
    for (number, room) in rooms.iter().enumerate() {
        let (cx, cy) = room.center();
        let mut sites = vec![];
        for y in room.y..room.y + room.h {
            for x in room.x..room.x + room.w {
                let (dx, dy) = (x - cx, y - cy);
                if room.round && dx * dx + dy * dy > (room.w / 2).pow(2) {
                    continue;
                }
                let i = (y * width + x) as usize;
                map.cells[i].region = number as u8 + 2; // room identity, region 1 marks the spawn
                let radius = dx * dx + dy * dy;
                map.cells[i].ground = if number == 0 && (13..=18).contains(&radius)
                    || (number == 0 || number == entrance) && radius <= 1
                {
                    0x65
                } else if rng.gen_ratio(1, 32) {
                    0xd4
                } else {
                    5
                };
                if number != 0 && number != entrance && dx.abs() > 2 && dy.abs() > 2 {
                    sites.push(i);
                }
            }
        }
        if number != 0 && number != entrance {
            sites.shuffle(&mut rng);
            let family = *FAMILIES.choose(&mut rng).unwrap();
            for _ in 0..rng.gen_range(6..=10) {
                let i = sites.pop().context("Undead Lair room has no enemy site")?;
                map.cells[i].object = Some(*family.choose(&mut rng).unwrap());
            }
            for kind in [0xda2, 0xda3, 0x1801, 0x1802, 0x1803] {
                if rng.gen_bool(0.5) {
                    let i = sites
                        .pop()
                        .context("Undead Lair room has no decoration site")?;
                    map.cells[i].object = Some(kind);
                }
            }
        }
    }
    for corridor in edges {
        for y in corridor.y..corridor.y + corridor.h {
            for x in corridor.x..corridor.x + corridor.w {
                let cell = &mut map.cells[(y * width + x) as usize];
                cell.ground = 5;
                cell.object = None;
            }
        }
    }
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let i = (y * width + x) as usize;
            if map.cells[i].ground == 0xfe
                && (-1..=1).any(|dy| {
                    (-1..=1)
                        .any(|dx| map.cells[((y + dy) * width + x + dx) as usize].ground != 0xfe)
                })
            {
                map.cells[i].object = Some(if rng.gen_ratio(1, 20) { 0x1eb } else { 0x120 });
            }
        }
    }
    let (bx, by) = rooms[0].center();
    map.cells[(by * width + bx) as usize].object = Some(0xd90);
    let (sx, sy) = rooms[entrance].center();
    let cell = &mut map.cells[(sy * width + sx) as usize];
    cell.ground = 0x65;
    cell.object = Some(0x703);
    cell.region = 1;
    map.spawns.push((sx as f32 + 0.5, sy as f32 + 0.5));
    // The 2012 footage shows dark endpoint motifs and eight boss candles: https://www.youtube.com/watch?v=SXTEcrN4bII&t=237s (tile raster reconstructed, not an original template).
    for (endpoint, x, y) in [("entrance", sx, sy), ("boss", bx, by)] {
        let candles: &[(i32, i32, u16)] = if endpoint == "boss" {
            &[
                (-4, 0, 0x1802),
                (4, 0, 0x1802),
                (0, -4, 0x1802),
                (0, 4, 0x1802),
                (-3, -3, 0x1802),
                (-3, 3, 0x1802),
                (3, -3, 0x1802),
                (3, 3, 0x1802),
            ]
        } else {
            &[(-2, 0, 0x1801), (2, 0, 0x1802)]
        };
        for &(dx, dy, kind) in candles {
            let (x, y) = (x + dx, y + dy);
            let cell = &mut map.cells[(y * width + x) as usize];
            ensure!(
                matches!(cell.ground, 5 | 0x65 | 0xd4) && cell.object.is_none(),
                "Undead Lair seed {seed}: {endpoint} candle requires empty floor at ({x}, {y}); ground={:#x}, object={:?}, region={}",
                cell.ground,
                cell.object,
                cell.region
            );
            cell.object = Some(kind);
        }
    }
    for cell in &map.cells {
        ensure!(
            content.grounds.contains_key(&cell.ground),
            "unknown Undead Lair ground"
        );
        if let Some(kind) = cell.object {
            content.get(kind)?;
        }
    }
    tracing::info!(
        seed,
        rooms = rooms.len(),
        width,
        height,
        "generated reconstructed Undead Lair"
    );
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet, VecDeque};

    #[test]
    fn endpoint_motifs_keep_eight_boss_candles_and_connected_room_families() {
        let content = Content::load(std::path::Path::new("data")).unwrap();
        assert_eq!(content.grounds[&0x65], content.grounds[&5]);
        for seed in 0..32 {
            let map = generate(&content, seed).unwrap();
            let locate = |kind| {
                let found: Vec<_> = map
                    .cells
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.object == Some(kind))
                    .map(|(i, _)| i)
                    .collect();
                assert_eq!(found.len(), 1, "seed {seed}, kind {kind:#x}");
                (found[0] as i32 % map.width, found[0] as i32 / map.width)
            };
            let boss = locate(0xd90);
            let spawn = locate(0x703);
            assert_eq!(
                map.spawns,
                vec![(spawn.0 as f32 + 0.5, spawn.1 as f32 + 0.5)]
            );
            let cell = |x, y| &map.cells[(y * map.width + x) as usize];
            for (point, region) in [(boss, 2), (spawn, 1)] {
                for (dx, dy) in [(0, 0), (-1, 0), (1, 0), (0, -1), (0, 1)] {
                    assert_eq!(cell(point.0 + dx, point.1 + dy).ground, 0x65);
                }
                assert_eq!(cell(point.0, point.1).region, region);
            }
            let mut candles = 0;
            for dy in -5_i32..=5 {
                for dx in -5_i32..=5 {
                    let c = cell(boss.0 + dx, boss.1 + dy);
                    assert_eq!(
                        c.ground == 0x65,
                        dx * dx + dy * dy <= 1 || (13..=18).contains(&(dx * dx + dy * dy))
                    );
                    let candle = (dx.abs() == 4 && dy == 0)
                        || (dy.abs() == 4 && dx == 0)
                        || (dx.abs() == 3 && dy.abs() == 3);
                    assert_eq!(c.object == Some(0x1802), candle);
                    candles += usize::from(candle);
                }
            }
            assert_eq!(candles, 8);
            assert!(!map.cells.iter().any(|c| c.ground == 0x61));
            let mut families = BTreeMap::<u8, Vec<u16>>::new();
            for c in &map.cells {
                if let Some(kind) = c.object.filter(|k| FAMILIES.iter().any(|f| f.contains(k))) {
                    families.entry(c.region).or_default().push(kind);
                }
            }
            assert!((30..=38).contains(&families.len()));
            for actors in families.values() {
                // Corridor carving can clear edge spawn sites, so six is not a final population minimum.
                assert!((1..=10).contains(&actors.len()));
                assert!(
                    FAMILIES
                        .iter()
                        .any(|f| actors.iter().all(|k| f.contains(k)))
                );
            }
            let passable = |x, y| {
                (0..map.width).contains(&x)
                    && (0..map.height).contains(&y)
                    && cell(x, y).ground != 0xfe
            };
            let mut seen = BTreeSet::from([spawn]);
            let mut queue = VecDeque::from([spawn]);
            while let Some((x, y)) = queue.pop_front() {
                for p in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
                    if passable(p.0, p.1) && seen.insert(p) {
                        queue.push_back(p);
                    }
                }
            }
            for y in 1..map.height - 1 {
                for x in 1..map.width - 1 {
                    assert!(
                        passable(x, y)
                            || !(passable(x - 1, y) && passable(x + 1, y)
                                || passable(x, y - 1) && passable(x, y + 1)),
                        "seed {seed}: chambers share a single wall at ({x}, {y})"
                    );
                }
            }
            assert!(seen.contains(&boss));
            assert_eq!(
                seen.len(),
                map.cells.iter().filter(|c| c.ground != 0xfe).count(),
                "seed {seed}"
            );
        }
    }
}
