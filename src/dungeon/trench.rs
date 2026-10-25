use crate::{
    content::{Cell, Content, Map},
    encounter::{Motion, Profile},
};
use anyhow::{Result, ensure};
use rand::{Rng, SeedableRng};

pub const HERMIT: u16 = 0xd61;
pub const THESSAL: u16 = 0x1706;
pub const VENT: u16 = 0x731;
pub const ROSTER: &[u16] = &[
    0xd61, 0xd62, 0xd63, 0xd64, 0xd65, 0x1700, 0x1702, 0x1703, 0x1704, 0x1705, 0x1706, 0x1707,
    0x1708, 0x1709, 0x170a, 0x170b, 0x170c, 0x170d, 0x170e,
];

pub fn profile(kind: u16) -> Option<Profile> {
    ROSTER.contains(&kind).then_some(Profile {
        motion: Motion::Stationary,
        speed: 0.,
        interval: 50,
        range: 30.,
    })
}

pub fn validate(content: &Content, kind: u16) -> Result<()> {
    ensure!(ROSTER.contains(&kind), "unknown ocean encounter {kind:#x}");
    let indices: &[u8] = match kind {
        HERMIT | THESSAL | 0x1707 => &[0, 1, 2, 3],
        0x1700 => &[0, 1, 2],
        0xd62 | 0xd64 | 0x1708 | 0x170a | 0x170c | 0x170e => &[0, 1],
        0xd65 | 0x1704 | 0x1705 | 0x170d => &[],
        _ => &[0],
    };
    for index in indices {
        ensure!(
            content.get(kind)?.projectiles.contains_key(index),
            "ocean encounter {kind:#x} lacks selected projectile {index}"
        );
    }
    Ok(())
}
pub fn generate(content: &Content, seed: u64) -> Result<Map> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let mut map = Map {
        width: 110,
        height: 100,
        cells: vec![
            Cell {
                ground: 0xff,
                object: None,
                config: String::new(),
                terrain: 0,
                region: 0
            };
            11000
        ],
        spawns: vec![(12.5, 12.5)],
    };
    // The original procedural generator is unavailable; reconstruct its connected underwater caverns.
    let rooms: [(i32, i32); 13] = [
        (12, 12),
        (32, 12),
        (52, 12),
        (72, 12),
        (92, 12),
        (92, 32),
        (72, 32),
        (52, 32),
        (32, 32),
        (12, 32),
        (12, 52),
        (32, 52),
        (64, 76),
    ];
    let index = |x: i32, y: i32| (y * 110 + x) as usize;
    for (i, &(cx, cy)) in rooms.iter().enumerate() {
        let radius = if i == 12 { 17 } else { rng.gen_range(5..=8) };
        for y in cy - radius..=cy + radius {
            for x in cx - radius..=cx + radius {
                if (x - cx).pow(2) + (y - cy).pow(2) <= radius * radius {
                    map.cells[index(x, y)].ground = 0x2e;
                }
            }
        }
    }
    for pair in rooms.windows(2) {
        let (mut x, mut y) = pair[0];
        let (tx, ty) = pair[1];
        let horizontal = rng.gen_bool(0.5);
        let mut steps = 0;
        loop {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    map.cells[index(x + dx, y + dy)].ground = 0x2e;
                }
            }
            if steps % 6 == 0 {
                map.cells[index(x, y)].object = Some(VENT);
            }
            if (x, y) == (tx, ty) {
                break;
            }
            if x != tx && (horizontal || y == ty) {
                x += (tx - x).signum();
            } else {
                y += (ty - y).signum();
            }
            steps += 1;
        }
    }
    for (i, &(x, y)) in rooms.iter().enumerate().skip(1).take(11) {
        let pack: &[u16] = match i % 4 {
            0 => &[0x1700, 0x1708, 0x1708],
            1 => &[0x170a, 0x1709, 0x1709],
            2 => &[0x170c, 0x1708, 0x1708],
            _ => &[0x170d, 0x170e, 0x170e],
        };
        for (j, kind) in pack.iter().enumerate() {
            map.cells[index(x - 2 + j as i32 * 2, y + 2)].object = Some(*kind);
        }
        map.cells[index(x, y)].object = Some(VENT);
        for &(dx, dy, kind) in &[(-3, -2, 0x170f), (3, -2, 0x1710)] {
            map.cells[index(x + dx, y + dy)].object = Some(kind);
        }
    }
    for y in 1..99 {
        for x in 1..109 {
            if map.cells[index(x, y)].ground == 0xff
                && [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]
                    .iter()
                    .any(|&(x, y)| map.cells[index(x, y)].ground == 0x2e)
            {
                map.cells[index(x, y)].object = Some(0x1713);
            }
        }
    }
    for (dx, dy) in [(14, 0), (-14, 0), (0, 14), (0, -14)] {
        map.cells[index(64 + dx, 76 + dy)].object = Some(VENT);
    }
    map.cells[index(64, 76)].object = Some(THESSAL);
    map.cells[index(12, 12)].object = Some(VENT);
    map.cells[index(12, 10)].object = Some(0x703);
    map.cells[index(12, 12)].region = 1;
    for c in &map.cells {
        ensure!(
            content.grounds.contains_key(&c.ground),
            "unknown Ocean Trench ground"
        );
        if let Some(kind) = c.object {
            content.get(kind)?;
        }
    }
    tracing::info!(
        seed,
        rooms = rooms.len(),
        "generated reconstructed Ocean Trench layout"
    );
    Ok(map)
}
