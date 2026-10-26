use crate::content::{Cell, Content, Map};
use anyhow::{Result, ensure};
use std::collections::BTreeSet;
type PillarOffsets = &'static [(i32, i32)];
use rand::{Rng, SeedableRng, seq::SliceRandom};

pub const MALPHAS: u16 = 0x90a;
pub const IDOL: u16 = 0xe1d;
pub const ROSTER: &[u16] = &[
    0x66d, 0x66e, 0x66f, 0x670, 0x671, 0x672, 0x908, 0x909, MALPHAS, 0x90b, IDOL,
];
pub const LAVA: u8 = 0x70;
pub const FLOOR: u8 = 0x3c;
pub const BRIDGE: u16 = 0x153;

#[derive(Clone, Copy)]
struct Room {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    depth: usize,
    tag: u8,
}
impl Room {
    fn center(self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }
    fn overlaps(self, b: Self) -> bool {
        self.x <= b.x + b.w
            && b.x <= self.x + self.w
            && self.y <= b.y + b.h
            && b.y <= self.y + self.h
    }
}
#[derive(Clone, Copy)]
struct Edge {
    parent: usize,
    child: usize,
    a: (i32, i32),
    b: (i32, i32),
}

fn attach(
    rooms: &[Room],
    parent: usize,
    size: (i32, i32, u8),
    door: (bool, bool, i32),
) -> (Room, Edge) {
    let (w, h, tag) = size;
    let (horizontal, positive, offset) = door;
    let p = rooms[parent];
    let inset = if horizontal { h / 2 } else { w / 2 };
    let mut r = Room {
        x: p.x,
        y: p.y,
        w,
        h,
        depth: p.depth + 1,
        tag,
    };
    let (a, b) = if horizontal {
        r.x = if positive { p.x + p.w + 2 } else { p.x - w - 2 };
        r.y = p.y + offset - inset;
        (
            (if positive { p.x + p.w - 1 } else { p.x }, p.y + offset),
            (if positive { r.x } else { r.x + w - 1 }, p.y + offset),
        )
    } else {
        r.y = if positive { p.y + p.h + 2 } else { p.y - h - 2 };
        r.x = p.x + offset - inset;
        (
            (p.x + offset, if positive { p.y + p.h - 1 } else { p.y }),
            (p.x + offset, if positive { r.y } else { r.y + h - 1 }),
        )
    };
    (
        r,
        Edge {
            parent,
            child: rooms.len(),
            a,
            b,
        },
    )
}
fn index(map: &Map, x: i32, y: i32) -> usize {
    (y * map.width + x) as usize
}

struct LavaField {
    cells: Vec<f32>,
    width: i32,
    height: i32,
}
impl LavaField {
    fn new(rng: &mut rand::rngs::StdRng, width: i32, height: i32) -> Self {
        let (cw, ch) = ((width + 3) / 4 + 2, (height + 3) / 4 + 2);
        let mut cells = vec![0f32; (cw * ch) as usize];
        let span = cw.min(ch) as f32;
        let radius = rng.gen_range(span / 3.0..span * 2.0 / 3.0);
        for _ in 0..200 {
            let (cx, cy) = (rng.gen_range(0..cw), rng.gen_range(0..ch));
            let mut aspect = rng.gen_range(1.0..4.0);
            if rng.gen_bool(0.5) {
                aspect = 1.0 / aspect;
            }
            for x in 0..cw {
                for y in 0..ch {
                    let (dx, dy) = ((x - cx) as f32, (y - cy) as f32);
                    let z = radius * radius - (dx * dx / aspect + dy * dy * aspect);
                    if z > 0. {
                        cells[(y * cw + x) as usize] += z / (radius * radius);
                    }
                }
            }
        }
        let low = cells.iter().copied().fold(f32::MAX, f32::min);
        let high = cells.iter().copied().fold(f32::MIN, f32::max);
        let range = (high - low).max(f32::EPSILON);
        for cell in &mut cells {
            *cell = ((*cell - low) / range).powi(2);
        }
        Self {
            cells,
            width: cw,
            height: ch,
        }
    }
    fn at(&self, x: i32, y: i32) -> f32 {
        let (fx, fy) = (x as f32 / 4.0, y as f32 / 4.0);
        let (x0, y0) = (fx.floor() as i32, fy.floor() as i32);
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let at = |x: i32, y: i32| {
            self.cells
                [(y.clamp(0, self.height - 1) * self.width + x.clamp(0, self.width - 1)) as usize]
        };
        let (a, b) = (at(x0, y0), at(x0 + 1, y0));
        let (c, d) = (at(x0, y0 + 1), at(x0 + 1, y0 + 1));
        let top = a + (b - a) * tx;
        let bottom = c + (d - c) * tx;
        top + (bottom - top) * ty
    }
    fn ground(&self, x: i32, y: i32) -> u8 {
        // Alternating height bands, not a fixed lava quota: https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/DungeonGen/Templates/Abyss/Overlay.cs#L98-L107
        if ((self.at(x, y) * 255.) as u8 / 10).is_multiple_of(2) {
            LAVA
        } else {
            FLOOR
        }
    }
}
fn path(map: &mut Map, mut a: (i32, i32), b: (i32, i32), protected: &mut BTreeSet<usize>) {
    loop {
        let i = index(map, a.0, a.1);
        protected.insert(i);
        if map.cells[i].ground == LAVA {
            map.cells[i].object = Some(BRIDGE);
        }
        if a == b {
            break;
        }
        if a.0 != b.0 {
            a.0 += (b.0 - a.0).signum();
        } else {
            a.1 += (b.1 - a.1).signum();
        }
    }
}

// Keep the shrine dry until activation: legacy reconstruction (creepylava, 2015), https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/DungeonGen/Templates/Abyss/template.jm
fn treasure_room(map: &mut Map, room: Room) {
    for y in 0..21 {
        for x in 0..15 {
            let chamber = match y {
                0 | 14 => (1..=3).contains(&x) || (11..=13).contains(&x),
                1..=3 | 11..=12 => true,
                4..=10 => (1..=13).contains(&x),
                13 => x != 5 && x != 9,
                _ => false,
            };
            let entrance = (14..=20).contains(&y)
                && if y == 14 || y == 20 {
                    (6..=8).contains(&x)
                } else {
                    (5..=9).contains(&x)
                };
            if chamber || entrance {
                let i = index(map, room.x + x, room.y + y);
                map.cells[i].ground = if entrance {
                    FLOOR
                } else if (6..=8).contains(&x) && (6..=8).contains(&y) {
                    0x35
                } else {
                    0x36
                };
                map.cells[i].region = room.tag;
            }
        }
    }
    for (x, y) in [
        (5, 1),
        (9, 1),
        (1, 5),
        (13, 5),
        (1, 9),
        (13, 9),
        (5, 16),
        (9, 16),
        (5, 18),
        (9, 18),
    ] {
        let i = index(map, room.x + x, room.y + y);
        map.cells[i].object = Some(if x == 5 && y >= 16 { 0x183 } else { 0x17e });
    }
    let i = index(map, room.x + 7, room.y + 7);
    map.cells[i].object = Some(IDOL);
}

fn room_candidates(rooms: &[Room], size: (i32, i32, u8)) -> Vec<(Room, Edge)> {
    let (w, h, tag) = size;
    let mut candidates = Vec::new();
    for parent in 0..rooms.len() {
        if rooms[parent].tag != 0 || parent == 0 && rooms.len() > 1 {
            continue;
        }
        for horizontal in [true, false] {
            let offsets = if parent == 0 {
                12..30
            } else {
                2..(if horizontal {
                    rooms[parent].h
                } else {
                    rooms[parent].w
                }) - 2
            };
            for offset in offsets {
                for positive in [true, false] {
                    if tag >= 16 && (horizontal || positive) {
                        continue;
                    }
                    let (r, edge) =
                        attach(rooms, parent, (w, h, tag), (horizontal, positive, offset));
                    if rooms.iter().any(|other| r.overlaps(*other)) {
                        continue;
                    }
                    let corridor = Room {
                        x: edge.a.0.min(edge.b.0) - 1,
                        y: edge.a.1.min(edge.b.1) - 1,
                        w: (edge.a.0 - edge.b.0).abs() + 3,
                        h: (edge.a.1 - edge.b.1).abs() + 3,
                        depth: 0,
                        tag: 0,
                    };
                    if rooms
                        .iter()
                        .enumerate()
                        .any(|(i, room)| i != parent && corridor.overlaps(*room))
                    {
                        continue;
                    }
                    candidates.push((r, edge));
                }
            }
        }
    }
    candidates
}

fn choose_room(
    rooms: &[Room],
    size: (i32, i32, u8),
    seed: u64,
    number: i32,
    rng: &mut impl Rng,
) -> Result<(Room, Edge)> {
    let tag = size.2;
    let mut candidates = room_candidates(rooms, size);
    if tag == 1
        && let Some(depth) = candidates.iter().map(|(r, _)| r.depth).max()
    {
        candidates.retain(|(r, _)| r.depth == depth);
    }
    let chosen = *candidates
        .choose_weighted(rng, |(_, e)| {
            if tag == 0 && e.parent + 1 == rooms.len() {
                32
            } else {
                1
            }
        })
        .map_err(|error| {
            anyhow::anyhow!("Abyss seed {seed}: room {number} has no valid frontier: {error}")
        })?;
    Ok(chosen)
}

fn place_scenery(
    map: &mut Map,
    rooms: &[Room],
    protected: &BTreeSet<usize>,
    seed: u64,
) -> Result<()> {
    for (number, room) in rooms.iter().enumerate().filter(|(n, _)| *n == 0) {
        let (cx, cy) = room.center();
        // Fixed independent reconstruction; the boss offsets were explicitly amended to preserve guards and paths.
        let (intact, broken): (PillarOffsets, PillarOffsets) = (
            &[
                (-10, -8),
                (-10, -2),
                (-10, 2),
                (-10, 8),
                (10, -8),
                (10, -2),
                (10, 2),
                (10, 8),
            ],
            &[(-11, -8), (-11, 8), (11, -8), (11, 8)],
        );
        for (offsets, kind) in [(intact, 0x17e), (broken, 0x183)] {
            for &(dx, dy) in offsets {
                let (x, y) = (cx + dx, cy + dy);
                ensure!(
                    x >= room.x
                        && x < room.x + room.w
                        && y >= room.y
                        && y < room.y + room.h
                        && dx.abs() > 1
                        && dy.abs() > 1,
                    "Abyss seed {seed}: room {number} scenery {kind:#x} cell ({x},{y}) violates room/axis clearance"
                );
                let i = index(map, x, y);
                ensure!(
                    map.cells[i].object.is_none() && !protected.contains(&i),
                    "Abyss seed {seed}: room {number} scenery {kind:#x} collision at ({x},{y}): object={:?}, required_path={}",
                    map.cells[i].object,
                    protected.contains(&i)
                );
                map.cells[i].object = Some(kind);
            }
        }
    }
    Ok(())
}

fn layout(seed: u64, rng: &mut impl Rng) -> Result<(Vec<Room>, Vec<Edge>, i32)> {
    let mut rooms = vec![Room {
        x: 0,
        y: 0,
        w: 42,
        h: 42,
        depth: 0,
        tag: 0,
    }];
    let mut edges = Vec::new();
    // Keep the original draw width: changing the integer type changes the seeded RNG stream.
    let count: i32 = rng.gen_range(36..=52);
    let mut treasures = 0;
    while treasures < 3 && rng.gen_bool(0.2) {
        treasures += 1;
    }
    // The 2012 footage establishes the form, not the lost server's seed distribution.
    for number in 1..count + treasures + 1 {
        let (w, h, tag) = if number == count + treasures {
            (16, 16, 1)
        } else if number >= count {
            (15, 21, 16 + (number - count) as u8)
        } else {
            (8, 8, 0)
        };
        let (r, edge) = choose_room(&rooms, (w, h, tag), seed, number, rng)?;
        rooms.push(r);
        edges.push(edge);
    }
    let min_x = rooms.iter().map(|r| r.x).min().unwrap() - 3;
    let min_y = rooms.iter().map(|r| r.y).min().unwrap() - 3;
    for r in &mut rooms {
        r.x -= min_x;
        r.y -= min_y;
    }
    for e in &mut edges {
        e.a.0 -= min_x;
        e.b.0 -= min_x;
        e.a.1 -= min_y;
        e.b.1 -= min_y;
    }
    Ok((rooms, edges, treasures))
}

pub fn generate(content: &Content, seed: u64) -> Result<Map> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let (rooms, edges, treasures) = layout(seed, &mut rng)?;
    let width = rooms.iter().map(|r| r.x + r.w).max().unwrap() + 3;
    let height = rooms.iter().map(|r| r.y + r.h).max().unwrap() + 3;
    ensure!(
        width <= 1024 && height <= 1024,
        "Abyss seed {seed} exceeds bounded map dimensions"
    );
    let empty = Cell {
        ground: 0xfe,
        object: None,
        config: String::new(),
        terrain: 0,
        region: 0,
    };
    let mut map = Map {
        width,
        height,
        cells: vec![empty; (width * height) as usize],
        spawns: vec![],
    };
    let field = LavaField::new(&mut rng, width, height);
    for (number, room) in rooms.iter().enumerate() {
        for y in room.y..room.y + room.h {
            for x in room.x..room.x + room.w {
                let i = index(&map, x, y);
                map.cells[i].ground = if room.tag >= 16 {
                    0xfe
                } else {
                    field.ground(x, y)
                };
            }
        }
        if number == 0 {
            let (cx, cy) = room.center();
            for y in room.y..room.y + room.h {
                for x in room.x..room.x + room.w {
                    let i = index(&map, x, y);
                    let (dx, dy) = (
                        (x as f32 - cx as f32 + 0.5).abs(),
                        (y as f32 - cy as f32 + 0.5).abs(),
                    );
                    let radius = dx.max(dy);
                    let c = &mut map.cells[i];
                    if dx > 14.5 && dy > 14.5 || dx > 11.5 && dy > 17.5 || dy > 11.5 && dx > 17.5 {
                        c.ground = 0xfe;
                        c.object = None;
                        continue;
                    }
                    if c.ground != LAVA {
                        c.ground = if radius < 12. { 0x69 } else { 0x36 };
                    }
                    // Period footage and Build 122 establish the two squared bridge rings, not the seed distribution.
                    if (4.5..=5.5).contains(&radius) || (12.5..=13.5).contains(&radius) {
                        c.ground = LAVA;
                        c.object = Some(BRIDGE);
                    }
                }
            }
        }
        if room.tag >= 16 {
            treasure_room(&mut map, *room);
        }
    }
    // The reference leaves broken tiles on part of the lava so it can be crossed.
    for room in &rooms {
        if room.tag >= 16 || room.depth == 0 {
            continue;
        }
        for y in room.y..room.y + room.h {
            for x in room.x..room.x + room.w {
                let i = index(&map, x, y);
                if map.cells[i].ground == LAVA && map.cells[i].object.is_none() && rng.gen_bool(0.1)
                {
                    map.cells[i].object = Some(BRIDGE);
                }
            }
        }
    }
    let mut protected = BTreeSet::new();
    for edge in &edges {
        for y in edge.a.1.min(edge.b.1)..=edge.a.1.max(edge.b.1) {
            for x in edge.a.0.min(edge.b.0)..=edge.a.0.max(edge.b.0) {
                for side in -1..=1 {
                    let (x, y) = if edge.a.0 == edge.b.0 {
                        (x + side, y)
                    } else {
                        (x, y + side)
                    };
                    let i = index(&map, x, y);
                    if map.cells[i].region < 16 {
                        map.cells[i].ground = field.ground(x, y);
                    }
                }
            }
        }
        path(
            &mut map,
            rooms[edge.parent].center(),
            edge.a,
            &mut protected,
        );
        path(&mut map, edge.a, edge.b, &mut protected);
        if rooms[edge.child].tag < 16 {
            path(&mut map, edge.b, rooms[edge.child].center(), &mut protected);
        }
    }
    for room in &rooms {
        let (cx, cy) = room.center();
        if room.tag == 1 {
            // The reference start room holds the Portal of Cowardice at a random tile and
            // keeps the ground around it clear of lava.
            let (px, py) = (
                rng.gen_range(room.x + 2..room.x + room.w - 2),
                rng.gen_range(room.y + 2..room.y + room.h - 2),
            );
            for y in py - 1..=py + 1 {
                for x in px - 1..=px + 1 {
                    let i = index(&map, x, y);
                    map.cells[i].ground = FLOOR;
                    map.cells[i].object = None;
                }
            }
            let i = index(&map, px, py);
            map.cells[i].object = Some(0x703);
            map.cells[i].region = 1;
            map.spawns.push((px as f32 + 0.5, py as f32 + 0.5));
            path(&mut map, (px, py), (cx, cy), &mut protected);
        } else if room.depth > 0 && room.tag == 0 {
            for _ in 0..rng.gen_range(4..=7) {
                let x = rng.gen_range(room.x..room.x + room.w);
                let y = rng.gen_range(room.y..room.y + room.h);
                let i = index(&map, x, y);
                if map.cells[i].ground != LAVA && map.cells[i].object.is_none() {
                    map.cells[i].object = Some(rng.gen_range(0x66d..=0x672));
                }
            }
            for _ in 0..2 {
                let i = index(
                    &map,
                    rng.gen_range(room.x..room.x + room.w),
                    rng.gen_range(room.y..room.y + room.h),
                );
                if map.cells[i].ground != LAVA && map.cells[i].object.is_none() {
                    map.cells[i].object = Some(0x1fa);
                }
            }
        }
    }
    let (cx, cy) = rooms[0].center();
    let i = index(&map, cx, cy);
    map.cells[i].ground = 0x69;
    map.cells[i].object = Some(MALPHAS);
    for (dx, dy) in [(-4, -9), (5, -9), (-9, 0), (10, 0), (-4, 8), (5, 8)]
        .into_iter()
        .take(rng.gen_range(3..=6))
    {
        let i = index(&map, cx + dx, cy + dy);
        map.cells[i].object = Some(0x651);
    }
    place_scenery(&mut map, &rooms, &protected, seed)?;
    let mut walls = vec![];
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let i = index(&map, x, y);
            if map.cells[i].ground == 0xfe
                && (-1..=1).any(|dy| {
                    (-1..=1).any(|dx| map.cells[index(&map, x + dx, y + dy)].ground != 0xfe)
                })
            {
                walls.push(i);
            }
        }
    }
    for i in walls {
        map.cells[i].object = Some(if rng.gen_bool(0.1) { 0x151 } else { 0x150 });
    }
    for c in &map.cells {
        ensure!(
            content.grounds.contains_key(&c.ground),
            "unknown Abyss ground {:#x}",
            c.ground
        );
        if let Some(k) = c.object {
            content.get(k)?;
        }
    }
    tracing::info!(
        seed,
        rooms = rooms.len(),
        treasures,
        depth = rooms.last().unwrap().depth,
        width,
        height,
        "generated reconstructed 123.5.1 Abyss"
    );
    Ok(map)
}

pub fn initial_condition(kind: u16) -> i32 {
    if kind == MALPHAS {
        crate::condition::bit(crate::condition::INVULNERABLE)
    } else {
        0
    }
}

pub fn profile(kind: u16) -> crate::encounter::Profile {
    use crate::encounter::{Anchor, Motion::*, Profile};
    let (motion, speed, interval, range) = match kind {
        0x66d => (Wander, 12., 1000, 20.),
        0x66e..=0x670 => (
            Chase {
                sight: 11.,
                stop: 5.,
            },
            16.,
            1000,
            15.,
        ),
        0x671 | 0x672 => (
            Chase {
                sight: 11.,
                stop: 1.,
            },
            16.,
            500,
            4.8,
        ),
        0x908 => (
            Orbit {
                anchor: Anchor::Parent,
                radius: 9.,
            },
            6.,
            1200,
            5.,
        ),
        0x909 => (
            Chase {
                sight: 20.,
                stop: 0.,
            },
            6.,
            100,
            100.,
        ),
        0x90b => (
            Chase {
                sight: 15.,
                stop: 1.,
            },
            6.,
            200,
            4.,
        ),
        MALPHAS | IDOL => (Stationary, 0., 100, 20.),
        _ => unreachable!("unregistered Abyss enemy"),
    };
    Profile {
        motion,
        speed,
        interval,
        range,
    }
}
pub fn phase(elapsed: u64) -> u64 {
    match elapsed % 50200 {
        0..9000 => 0,
        9000..11500 => 1,
        11500..23500 => 2,
        23500..36500 => 3,
        36500..39000 => 4,
        39000..47000 => 5,
        _ => 6,
    }
}
