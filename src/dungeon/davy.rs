use crate::{
    content::{Cell, Content, Map},
    encounter::{Motion, Profile, nr_speed},
};
use anyhow::{Result, ensure};
use rand::{Rng, SeedableRng, seq::SliceRandom};

pub const NAME: &str = "Davy Jones's Locker";
pub const TEMPLATE: &str = "davy-jones-locker";
pub const BOSS: u16 = 0xe32;
pub const UNLIT: u16 = 0xe35;
pub const LIT: u16 = 0xe36;
pub const KEYS: [u16; 4] = [0xe3e, 0xe3f, 0xe40, 0xe41];
pub const COLORS: [&str; 4] = ["purple", "green", "red", "yellow"];
pub const DOORS: [u16; 4] = [0x186a, 0x186c, 0x1868, 0x186e];
pub const LAMPS: [(i32, i32); 8] = [
    (-4, -4),
    (3, -4),
    (-6, -1),
    (5, -1),
    (-6, 3),
    (5, 3),
    (-4, 6),
    (3, 6),
];
pub const CREW: [u16; 7] = [0xe31, 0xe3a, 0xe44, 0xe2f, 0xe45, 0xe42, 0xe43];
pub const ROSTER: [u16; 12] = [
    BOSS, UNLIT, LIT, 0xe50, 0xe48, 0xe31, 0xe3a, 0xe44, 0xe2f, 0xe45, 0xe42, 0xe43,
];

pub fn door_color(kind: u16) -> Option<usize> {
    DOORS
        .iter()
        .position(|door| kind == *door || kind == *door + 1)
}
pub fn profile(kind: u16) -> Option<Profile> {
    let follow = |rate, interval| {
        (
            Motion::Follow {
                sight: 8.,
                stop: 1.,
                idle: nr_speed(0.1),
                duration: 0,
                cooldown: 0,
            },
            rate,
            interval,
            10.,
        )
    };
    let (motion, rate, interval, range) = match kind {
        BOSS => (Motion::WanderHome { radius: 3. }, 0.2, 50, 12.),
        UNLIT | 0x182e => (Motion::Stationary, 0., 0, 0.),
        LIT => (Motion::Stationary, 0., 50, 10.),
        0xe44 => (
            Motion::Chase {
                sight: 8.,
                stop: 2.,
            },
            0.35,
            4000,
            10.,
        ),
        0xe48 => (
            Motion::Orbit {
                anchor: crate::encounter::Anchor::Parent,
                radius: 3.,
            },
            0.3,
            50,
            10.,
        ),
        0xe45 => (
            Motion::Chase {
                sight: 13.,
                stop: 6.,
            },
            0.2,
            50,
            13.,
        ),
        0xe31 | 0xe50 => follow(0.6, 2000),
        0xe3a => follow(0.25, 1750),
        0xe2f => follow(0.55, 1750),
        0xe42 => follow(0.4, 2500),
        0xe43 => follow(0.3, 2000),
        _ => return None,
    };
    Some(Profile {
        motion,
        speed: if rate == 0. { 0. } else { nr_speed(rate) },
        interval,
        range,
    })
}
pub fn initial_condition(kind: u16) -> i32 {
    if kind == 0xe48 {
        return crate::condition::bit(crate::condition::INVINCIBLE);
    }
    if matches!(kind, BOSS | LIT | 0xe50) || KEYS.contains(&kind) {
        crate::condition::bit(crate::condition::INVULNERABLE)
    } else {
        0
    }
}

// The 2012 guide establishes sequential shared keys and touch-open doors, not the lost maze generator.
// https://wikiwiki.jp/rmd/Davy%20Jones%27s%20Locker
pub fn generate(content: &Content, seed: u64) -> Result<Map> {
    const SIDE: usize = 5;
    const PITCH: i32 = 26;
    const WIDTH: i32 = 132;
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let mut parents = [None; SIDE * SIDE];
    let mut depths = [0; SIDE * SIDE];
    let mut seen = [false; SIDE * SIDE];
    let mut stack = vec![0];
    seen[0] = true;
    while let Some(&room) = stack.last() {
        let (x, y) = (room % SIDE, room / SIDE);
        let mut candidates: Vec<_> = [
            (x as i32 - 1, y as i32),
            (x as i32 + 1, y as i32),
            (x as i32, y as i32 - 1),
            (x as i32, y as i32 + 1),
        ]
        .into_iter()
        .filter(|(x, y)| (0..SIDE as i32).contains(x) && (0..SIDE as i32).contains(y))
        .map(|(x, y)| y as usize * SIDE + x as usize)
        .filter(|n| !seen[*n])
        .collect();
        candidates.shuffle(&mut rng);
        if let Some(&next) = candidates.first() {
            parents[next] = Some(room);
            depths[next] = depths[room] + 1;
            seen[next] = true;
            stack.push(next);
        } else {
            stack.pop();
        }
    }
    let boss = (1..SIDE * SIDE).max_by_key(|n| depths[*n]).unwrap();
    let max_depth = depths[boss];
    let zone = |room: usize| depths[room] * 5 / (max_depth + 1);
    let center = |room: usize| {
        (
            14 + (room % SIDE) as i32 * PITCH,
            14 + (room / SIDE) as i32 * PITCH,
        )
    };
    let index = |x: i32, y: i32| (y * WIDTH + x) as usize;
    let mut map = Map {
        width: WIDTH,
        height: WIDTH,
        cells: vec![
            Cell {
                ground: 0xfe,
                object: None,
                config: String::new(),
                terrain: 0,
                region: 0
            };
            (WIDTH * WIDTH) as usize
        ],
        spawns: vec![],
    };
    for room in 0..SIDE * SIDE {
        let (cx, cy) = center(room);
        let radius = if room == boss {
            10
        } else {
            rng.gen_range(6..=8)
        };
        let floor = [0x7c, 0x7d, 0x7e][rng.gen_range(0..3)];
        for y in cy - radius..=cy + radius {
            for x in cx - radius..=cx + radius {
                if room == boss && (x - cx).abs() + (y - cy).abs() > 16 {
                    continue;
                }
                let c = &mut map.cells[index(x, y)];
                c.ground = if room == boss {
                    if (-3..=2).contains(&(x - cx)) && (-2..=4).contains(&(y - cy)) {
                        0x7f
                    } else {
                        0xa6
                    }
                } else {
                    floor
                };
                c.region = zone(room) as u8 + 2;
            }
        }
        if room != 0 && room != boss {
            let pack = [
                CREW[0],
                CREW[1],
                CREW[2 + room % 5],
                CREW[rng.gen_range(2..7)],
            ];
            for ((dx, dy), kind) in [(-3, -3), (3, -3), (-3, 3), (3, 3)].into_iter().zip(pack) {
                map.cells[index(cx + dx, cy + dy)].object = Some(kind);
            }
            for (dx, dy, kind) in [(-5, 0, 0x1834), (5, 0, 0x1832), (0, -5, 0x182e)] {
                map.cells[index(cx + dx, cy + dy)].object = Some(kind);
            }
        }
    }
    for (room, parent) in parents.iter().enumerate().skip(1) {
        let parent = parent.expect("connected Davy maze");
        let ((x, y), (px, py)) = (center(room), center(parent));
        for yy in y.min(py)..=y.max(py) + 1 {
            for xx in x.min(px)..=x.max(px) + 1 {
                let c = &mut map.cells[index(xx, yy)];
                if c.ground == 0xfe {
                    c.ground = 0x7e;
                }
                c.object = None;
            }
        }
        if zone(room) != zone(parent) {
            ensure!(zone(room) == zone(parent) + 1, "maze skips a key stage");
            for half in 0..2 {
                let (dx, dy) = if x == px { (half, 0) } else { (0, half) };
                map.cells[index((x + px) / 2 + dx, (y + py) / 2 + dy)].object =
                    Some(DOORS[zone(parent)] + half as u16);
            }
        }
    }
    for y in 1..WIDTH - 1 {
        for x in 1..WIDTH - 1 {
            if map.cells[index(x, y)].ground == 0xfe
                && (-1..=1)
                    .any(|dy| (-1..=1).any(|dx| map.cells[index(x + dx, y + dy)].ground != 0xfe))
            {
                map.cells[index(x, y)].object = Some(0x1826);
            }
        }
    }
    for (color, key) in KEYS.iter().enumerate() {
        let candidates: Vec<_> = (1..SIDE * SIDE).filter(|r| zone(*r) == color).collect();
        ensure!(
            !candidates.is_empty(),
            "Davy maze has no room for key {color}"
        );
        let room = *candidates.choose(&mut rng).unwrap();
        let (x, y) = center(room);
        map.cells[index(x, y)].object = Some(*key);
    }
    let (bx, by) = center(boss);
    map.cells[index(bx, by)].object = Some(BOSS);
    for (dx, dy) in LAMPS {
        map.cells[index(bx + dx, by + dy)].object = Some(UNLIT);
    }
    let (sx, sy) = center(0);
    map.spawns.push((sx as f32 + 0.5, sy as f32 + 0.5));
    map.cells[index(sx, sy)].region = 1;
    map.cells[index(sx - 2, sy)].object = Some(0x703);
    for cell in &map.cells {
        ensure!(
            content.grounds.contains_key(&cell.ground),
            "unknown Davy ground"
        );
        if let Some(kind) = cell.object {
            content.get(kind)?;
        }
    }
    tracing::info!(
        seed,
        rooms = SIDE * SIDE,
        stages = 5,
        "generated reconstructed Davy key maze"
    );
    Ok(map)
}
