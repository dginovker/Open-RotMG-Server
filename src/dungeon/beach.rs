use crate::{
    content::{Cell, Content, Map},
    encounter::{Motion, Profile, nr_speed},
};
use anyhow::{Result, ensure};

pub const BOSS: u16 = 0xe56;
pub const BUM: u16 = 0xe55;
pub const DRINKS: [u16; 4] = [0xc39, 0xc3a, 0xc3b, 0xc3c];
pub const TAUNTS: [&str; 11] = [
    "Oh no, Mixcoatl is my brother, I prefer partying to fighting.",
    "Lets have a fun-time in the sun-shine!",
    "Nothing like relaxin' on the beach.",
    "Chillin' is the name of the game!",
    "I hope you're having a good time!",
    "How do you like my shades?",
    "EVERYBODY BOOGEY!",
    "What a beautiful day!",
    "Whoa there!",
    "Oh SNAP!",
    "Ho!",
];
pub fn profile(kind: u16) -> Profile {
    Profile {
        motion: if kind == BUM {
            Motion::WanderHome { radius: 2.5 }
        } else {
            Motion::Stationary
        },
        speed: if kind == BUM { nr_speed(0.2) } else { 0. },
        interval: 0,
        range: 0.,
    }
}

// The archived 123.5.0 guide establishes the island, towels, partial wooden stage and four braziers.
// https://web.archive.org/web/20121201000000id_/http://forums.wildshadow.com/wiki/realm-mad-god/beachzone
pub fn generate(content: &Content) -> Result<Map> {
    let (width, height) = (136, 127);
    let mut cells = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let (dx, dy) = ((x as f32 - 70.) / 1.07, y as f32 - 65.);
            let angle = dy.atan2(dx);
            let r = dx.hypot(dy) + 1.5 * (angle * 3.).sin() + (angle * 5.).cos();
            let ground = if r > 65. {
                0xff
            } else if r > 51. {
                0xb1
            } else if r > 44. {
                0xb2
            } else if r > 34. {
                0xb3
            } else if r > 22. {
                0xf5
            } else if r > 20. {
                0xbe
            } else {
                0x23
            };
            cells.push(Cell {
                ground,
                object: None,
                config: String::new(),
                region: 0,
                terrain: 0,
            });
        }
    }
    let mut set = |x: i32, y: i32, ground: u8, object| {
        cells[(y * width + x) as usize] = Cell {
            ground,
            object,
            config: String::new(),
            region: 0,
            terrain: 0,
        };
    };
    for n in 0..16 {
        let a = n as f32 * std::f32::consts::TAU / 16.;
        let (x, y) = ((70. + 29. * a.cos()) as i32, (65. + 27. * a.sin()) as i32);
        set(x, y, 0xf5, Some(0x18a7));
        for dx in [-2, 2] {
            set(x + dx, y + 1, if n % 2 == 0 { 0xb4 } else { 0xb5 }, None);
        }
    }
    for y in 60..=72 {
        for x in 65..=77 {
            if (x as f32 - 71.).hypot(y as f32 - 66.) <= 6. {
                set(x, y, 0xf5, None);
            }
        }
    }
    for (x, y) in [(70, 66), (71, 65), (72, 66), (71, 67)] {
        set(x, y, 0xb6, None);
    }
    for (x, y) in [(68, 63), (74, 63), (68, 69), (74, 69)] {
        set(x, y, 0xf5, Some(0xdc6));
    }
    set(71, 66, 0xb6, Some(BOSS));
    set(44, 65, 0xf5, Some(0x703));
    for cell in &cells {
        ensure!(
            content.grounds.contains_key(&cell.ground),
            "Beachzone ground missing: {:#x}",
            cell.ground
        );
        if let Some(kind) = cell.object {
            content.get(kind)?;
        }
    }
    Ok(Map {
        width,
        height,
        cells,
        spawns: vec![(46.5, 65.5)],
    })
}
