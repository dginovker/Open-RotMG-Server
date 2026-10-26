use crate::{
    content::{Cell, Content, Map},
    encounter::{Anchor, Motion, Profile},
};
use anyhow::{Result, ensure};
use rand::{Rng, SeedableRng, seq::SliceRandom};

const MAX_ROOMS: usize = 30;
// Skipped bat rooms retain their broods; leave headroom for Ruthven's overlapping cohorts.
pub const SUMMON_LIMIT: usize = (MAX_ROOMS + 2) * 4 * 20;

pub const BOSS: u16 = 0x1720;
pub const BAT: u16 = 0x1721;
pub const CREATURE: u16 = 0x1722;
pub const SWARM: u16 = 0x1723;
pub const ESCORT: u16 = 0x1724;
pub const DECOY: u16 = 0x1725;
pub const CHASER: u16 = 0x1726;
pub const COFFIN: u16 = 0x1737;
pub const FERATU: u16 = 0x1738;
pub const BOOKCASE: u16 = 0x181a;
pub const ROCK: u16 = 0x172a;
pub const KAGE: u16 = 0x6db;
pub const MINE: u16 = 0x6da;
pub const GRAVE: u16 = 0x181f;
pub const ROSTER: [u16; 18] = [
    BOSS, BAT, CREATURE, SWARM, ESCORT, DECOY, CHASER, 0x1727, 0x1728, 0x1729, ROCK, 0x172b,
    0x172c, COFFIN, FERATU, KAGE, MINE, GRAVE,
];

// Phoenix supplies reconstruction timers; the 2012 guide establishes timed return, not a coffin kill gate.
// https://web.archive.org/web/20120516045129id_/http://forums.wildshadow.com/wiki/realm-mad-god/manor-immortals
pub fn phase(elapsed: u64) -> (u64, u64) {
    let t = elapsed % 21750;
    let (part, start) = if t < 9100 {
        (0, 0)
    } else if t < 16750 {
        (1, 9100)
    } else {
        (2, 16750)
    };
    (elapsed / 21750 * 3 + part, t - start)
}
pub fn blades(hp: i32) -> usize {
    (3 + (15000 - hp).max(0) as usize / 1000).min(18)
}
pub fn initial_condition(kind: u16) -> i32 {
    use crate::condition::{ARMORED, INVINCIBLE, bit};
    match kind {
        0x1729 => bit(ARMORED),
        // Size-zero anchors must not absorb native shots or show protection icons.
        SWARM | DECOY => bit(INVINCIBLE),
        _ => 0,
    }
}
pub fn profile(kind: u16) -> Profile {
    use Motion::*;
    let (motion, speed, range) = match kind {
        BOSS => (WanderHome { radius: 1. }, 1., 30.),
        BAT | CHASER | 0x1727 => (
            Chase {
                sight: 20.,
                stop: if kind == BAT { 2. } else { 0. },
            },
            8.,
            1.,
        ),
        ESCORT => (
            Protect {
                anchor: Anchor::Parent,
                distance: 4.,
            },
            8.,
            1.,
        ),
        SWARM => (
            Protect {
                anchor: Anchor::Parent,
                distance: 0.3,
            },
            8.,
            0.,
        ),
        DECOY => (
            Chase {
                sight: 20.,
                stop: 2.,
            },
            6.,
            0.,
        ),
        CREATURE | COFFIN | ROCK => (Stationary, 0., 10.),
        KAGE | MINE | GRAVE => (Stationary, 0., 20.),
        0x1728 => (
            Chase {
                sight: 10.,
                stop: 1.,
            },
            2.,
            10.,
        ),
        0x1729 => (
            Chase {
                sight: 10.,
                stop: 1.,
            },
            2.,
            10.,
        ),
        0x172b | 0x172c => (
            Charge {
                sight: 6.,
                cooldown: 0,
                delay: 0,
                stop: 1.,
                idle: 0.,
            },
            6.,
            if kind == 0x172b { 10. } else { 9. },
        ),
        FERATU => (WanderHome { radius: 1. }, 1., 8.),
        _ => unreachable!("unregistered Manor actor {kind:#x}"),
    };
    Profile {
        motion,
        // Phoenix integrates legacy speed/1.5; these are not NR-Core movement rates.
        speed: speed / 1.5,
        range,
        interval: 100,
    }
}

#[derive(Clone, Copy)]
struct Room {
    x: i32,
    y: i32,
    parent: usize,
    depth: usize,
    radius: i32,
    secret: bool,
}
impl Room {
    fn center(self) -> (i32, i32) {
        (16 + self.x * 32, 16 + self.y * 32)
    }
}
fn cell(map: &mut Map, x: i32, y: i32) -> &mut Cell {
    &mut map.cells[(y * map.width + x) as usize]
}

pub fn generate(content: &Content, seed: u64) -> Result<Map> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let mut rooms = vec![Room {
        x: 3,
        y: 3,
        parent: 0,
        depth: 0,
        radius: 6,
        secret: false,
    }];
    for _ in 1..rng.gen_range(24..=MAX_ROOMS) {
        let mut frontier = vec![];
        for (i, r) in rooms.iter().enumerate() {
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let (x, y) = (r.x + dx, r.y + dy);
                if (0..7).contains(&x)
                    && (0..7).contains(&y)
                    && !rooms.iter().any(|r| r.x == x && r.y == y)
                {
                    frontier.push(Room {
                        x,
                        y,
                        parent: i,
                        depth: r.depth + 1,
                        radius: rng.gen_range(6..=9),
                        secret: false,
                    });
                }
            }
        }
        rooms.push(
            *frontier
                .choose(&mut rng)
                .expect("Manor grid has an unoccupied frontier"),
        );
    }
    let boss = (1..rooms.len()).max_by_key(|i| rooms[*i].depth).unwrap();
    rooms[boss].radius = 13;
    let mut leaves: Vec<_> = (1..rooms.len())
        .filter(|i| *i != boss && !rooms.iter().any(|r| r.parent == *i))
        .collect();
    leaves.shuffle(&mut rng);
    for i in leaves.into_iter().take(rng.gen_range(0..=3)) {
        rooms[i].secret = true;
        rooms[i].radius = 5;
    }
    tracing::info!(
        seed,
        rooms = rooms.len(),
        boss_depth = rooms[boss].depth,
        secret_rooms = rooms.iter().filter(|r| r.secret).count(),
        "Manor layout generated"
    );
    let mut map = Map {
        width: 224,
        height: 224,
        cells: vec![
            Cell {
                ground: 0xff,
                object: None,
                config: String::new(),
                terrain: 0,
                region: 0
            };
            224 * 224
        ],
        spawns: vec![],
    };
    let mut coffins = vec![];
    for (i, r) in rooms.iter().enumerate() {
        let (cx, cy) = r.center();
        for y in cy - r.radius..=cy + r.radius {
            for x in cx - r.radius..=cx + r.radius {
                let c = cell(&mut map, x, y);
                c.ground = if (cx - 5..=cx + 6).contains(&x)
                    && (cy - 4..=cy + 7).contains(&y)
                    && i == boss
                {
                    0x49
                } else {
                    [0xcd, 0xce, 0xcf][i % 3]
                };
                c.region = i as u8 + 2;
            }
        }
        if i != 0 && i != boss {
            if r.secret {
                coffins.extend([(cx - 2, cy), (cx + 2, cy)]);
            } else if rng.gen_ratio(1, 5) {
                coffins.push((cx, cy));
            } else {
                let family = if rng.gen_bool(0.3) {
                    0x1727
                } else {
                    *[0x1728, 0x1729, 0x172b, 0x172c].choose(&mut rng).unwrap()
                };
                for (dx, dy) in [(-3, -3), (3, -3), (-3, 3), (3, 3)] {
                    cell(&mut map, cx + dx, cy + dy).object = Some(family);
                }
            }
        }
        for (dx, dy) in [
            (-r.radius + 1, -r.radius + 1),
            (r.radius - 1, -r.radius + 1),
            (-r.radius + 1, r.radius - 1),
            (r.radius - 1, r.radius - 1),
        ] {
            cell(&mut map, cx + dx, cy + dy).object = Some(0x180d);
        }
        if i != boss && !r.secret {
            for dx in [-2, -1, 0, 1, 2] {
                let shelf = cell(&mut map, cx + dx, cy - r.radius);
                shelf.object = Some(BOOKCASE);
                // Early Manor distinguished its hidden doors by accepting damage only on those shelves.
                shelf.config = format!(
                    "eff:{:x}",
                    crate::condition::bit(crate::condition::INVINCIBLE)
                );
            }
        }
    }
    for r in rooms.iter().skip(1) {
        let (cx, cy) = r.center();
        let p = rooms[r.parent];
        let (px, py) = p.center();
        for y in cy.min(py)..=cy.max(py) {
            for x in cx.min(px)..=cx.max(px) {
                for d in -1..=1 {
                    let (x, y) = if cx == px { (x + d, y) } else { (x, y + d) };
                    let c = cell(&mut map, x, y);
                    if c.ground == 0xff {
                        c.ground = 0x6c;
                    }
                    c.object = None;
                    c.config.clear();
                }
            }
        }
        if r.secret {
            let (sx, sy) = ((px - cx).signum(), (py - cy).signum());
            for d in -1..=1 {
                let door = cell(
                    &mut map,
                    px - sx * (p.radius + 1) + sy * d,
                    py - sy * (p.radius + 1) + sx * d,
                );
                door.object = Some(BOOKCASE);
                door.region = 250;
            }
        }
    }
    for y in 1..223 {
        for x in 1..223 {
            if cell(&mut map, x, y).ground == 0xff
                && (-1..=1).any(|dy| {
                    (-1..=1).any(|dx| map.cells[((y + dy) * 224 + x + dx) as usize].ground != 0xff)
                })
            {
                cell(&mut map, x, y).object = Some(if (x + y) % 7 == 0 { 0x180b } else { 0x180a });
            }
        }
    }
    for (x, y) in coffins {
        cell(&mut map, x, y).object = Some(COFFIN);
    }
    let (bx, by) = rooms[boss].center();
    cell(&mut map, bx, by).object = Some(BOSS);
    let (sx, sy) = rooms[0].center();
    let c = cell(&mut map, sx, sy);
    c.object = Some(0x703);
    c.region = 1;
    map.spawns.push((sx as f32 + 0.5, sy as f32 + 0.5));
    for c in &map.cells {
        ensure!(
            content.grounds.contains_key(&c.ground),
            "unknown Manor ground {:#x}",
            c.ground
        );
        if let Some(k) = c.object {
            content.get(k)?;
        }
    }
    tracing::info!(
        seed,
        rooms = rooms.len(),
        boss_depth = rooms[boss].depth,
        secrets = rooms.iter().filter(|r| r.secret).count(),
        "generated reconstructed Manor"
    );
    Ok(map)
}
