use crate::{
    content::{Cell, Content, Map},
    encounter::{Motion, Profile, nr_speed},
};
use anyhow::{Result, ensure};
use rand::{Rng, SeedableRng};

pub const BOSS: u16 = 0x976;
pub const HEXXER: u16 = 0x1e15;
pub const DEHEXXER: u16 = 0x1e16;
pub const CAGE: u16 = 0x892;
pub const BUBBLE: u16 = 0x1e22;
pub const ROSTER: &[u16] = &[
    BOSS, 0x978, 0x979, 0x980, 0x981, 0x982, 0x983, 0x990, 0x991, 0x993, 0x994, 0x995, 0x996,
    0x12aa, 0x1e23, 0x1e24, 0x1e25, HEXXER, DEHEXXER, BUBBLE,
];

pub fn initial_condition(kind: u16) -> i32 {
    if matches!(
        kind,
        HEXXER | DEHEXXER | BUBBLE | 0x990 | 0x991 | 0x993..=0x996
    ) {
        crate::condition::bit(crate::condition::INVINCIBLE)
    } else {
        0
    }
}

pub fn profile(kind: u16) -> Profile {
    let (motion, speed, interval, range) = match kind {
        HEXXER | DEHEXXER | BUBBLE => (Motion::Stationary, 0., 0, 0.),
        BOSS => (Motion::WanderHome { radius: 5. }, nr_speed(0.5), 0, 12.),
        0x978 => (Motion::Stationary, 0., 50, 30.),
        0x990 | 0x991 => (Motion::Stationary, 0., 3000, 32.),
        0x993..=0x996 => (Motion::Stationary, 0., 1000, 30.),
        0x12aa => (Motion::Stationary, 0., 1000, 10.),
        0x980 | 0x1e24 => (
            Motion::Chase {
                sight: 10.,
                stop: 1.,
            },
            nr_speed(0.6),
            800,
            10.,
        ),
        0x981 | 0x1e23 => (Motion::Wander, nr_speed(0.5), 800, 10.),
        0x979 | 0x1e25 | 0x982 | 0x983 => (Motion::Wander, nr_speed(0.5), 1000, 10.),
        _ => panic!("unregistered Mad Lab actor {kind:#x}"),
    };
    Profile {
        motion,
        speed,
        interval,
        range,
    }
}

pub fn validate(content: &Content, kind: u16) -> Result<()> {
    ensure!(
        ROSTER.contains(&kind),
        "unregistered Mad Lab actor {kind:#x}"
    );
    let indices: &[u8] = match kind {
        HEXXER | DEHEXXER | BUBBLE => &[],
        0x980 | 0x1e24 => &[0, 1, 2],
        0x982 => &[0, 1],
        0x983 => &[0, 1, 2, 3],
        _ => &[0],
    };
    for index in indices {
        ensure!(
            content.get(kind)?.projectiles.contains_key(index),
            "Mad Lab actor {kind:#x} lacks selected projectile {index}"
        );
    }
    Ok(())
}

pub fn generate(content: &Content, seed: u64) -> Result<Map> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let size = 80;
    let mut map = Map {
        width: size,
        height: size,
        cells: vec![
            Cell {
                ground: 0xff,
                object: None,
                config: String::new(),
                terrain: 0,
                region: 0
            };
            (size * size) as usize
        ],
        spawns: vec![(12.5, 66.5)],
    };
    // No authenticated generator was recovered: this connected layout is original reconstruction.
    let rooms: [(i32, i32); 6] = [(12, 66), (12, 48), (32, 48), (32, 30), (56, 30), (56, 12)];
    let at = |x: i32, y: i32| (y * size + x) as usize;
    for (i, &(x, y)) in rooms.iter().enumerate() {
        let radius = if i == 5 { 10 } else { 6 };
        for yy in y - radius..=y + radius {
            for xx in x - radius..=x + radius {
                map.cells[at(xx, yy)].ground = if i % 2 == 0 { 0xd3 } else { 0xd2 };
            }
        }
    }
    for pair in rooms.windows(2) {
        let ((x, y), (tx, ty)) = (pair[0], pair[1]);
        for yy in y.min(ty) - 1..=y.max(ty) + 1 {
            for xx in x.min(tx) - 1..=x.max(tx) + 1 {
                map.cells[at(xx, yy)].ground = 0xd3;
            }
        }
    }
    for &(x, y) in &rooms[1..5] {
        for (j, (dx, dy)) in [(-3, -3), (3, -3), (-3, 3), (3, 3)].into_iter().enumerate() {
            map.cells[at(x + dx, y + dy)].object =
                Some([0x979, 0x980, 0x981, 0x982, 0x983][(j + rng.gen_range(0..5)) % 5]);
        }
    }
    for (x, y, kind, ground) in [
        (8, 48, HEXXER, 0xa9),
        (16, 48, DEHEXXER, 0xa7),
        (52, 30, HEXXER, 0xa9),
        (60, 30, DEHEXXER, 0xa7),
    ] {
        for yy in y - 1..=y + 1 {
            for xx in x - 1..=x + 1 {
                map.cells[at(xx, yy)].ground = ground;
            }
        }
        map.cells[at(x, y)].object = Some(kind);
    }
    for (dx, dy, kind) in [
        (-7, -7, 0x993),
        (7, -7, 0x994),
        (-7, 7, 0x995),
        (7, 7, 0x996),
        (-9, 0, CAGE),
        (9, 0, CAGE),
        (0, -9, CAGE),
        (0, 9, CAGE),
    ] {
        map.cells[at(56 + dx, 12 + dy)].object = Some(kind);
    }
    map.cells[at(56, 12)].object = Some(BOSS);
    map.cells[at(56, 11)].object = Some(BUBBLE);
    map.cells[at(12, 66)].region = 1;
    map.cells[at(12, 68)].object = Some(0x703);
    for y in 1..size - 1 {
        for x in 1..size - 1 {
            if map.cells[at(x, y)].ground == 0xff
                && [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]
                    .iter()
                    .any(|&(x, y)| map.cells[at(x, y)].ground != 0xff)
            {
                map.cells[at(x, y)].object = Some(0x188c);
            }
        }
    }
    for cell in &map.cells {
        ensure!(
            content.grounds.contains_key(&cell.ground),
            "unknown Mad Lab ground"
        );
        if let Some(kind) = cell.object {
            content.get(kind)?;
        }
    }
    tracing::info!(seed, "generated reconstructed Mad Lab layout");
    Ok(map)
}
