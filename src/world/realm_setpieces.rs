// GPL-3.0 reference; data/REALM-SRC-LICENSE: moistosaurus/realm-src@1855c017eab870d13f0b61b1cf0b9e66a01922ff
// wServer/realm/setpieces/{Grove,LichyTemple,Tower,Pyre,Castle,LavaFissure,SkullShrine,CubeGod,Pentaract,Sphinx,Temple,TempleA,TempleB,Oasis,Graveyard}.cs; NR-Core and Skilly.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct Area {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: i32,
    pub(super) height: i32,
}
#[derive(Clone)]
pub(super) struct EventSite {
    pub(super) area: Area,
    pub(super) cells: Vec<(usize, crate::content::Cell)>,
    scenery: Vec<Entity>,
}
impl Area {
    pub(super) fn distance(&self, p: Pos) -> f32 {
        let dx = (self.x as f32 - p.x)
            .max(p.x - (self.x + self.width) as f32)
            .max(0.);
        let dy = (self.y as f32 - p.y)
            .max(p.y - (self.y + self.height) as f32)
            .max(0.);
        dx.hypot(dy)
    }
    pub(super) fn separate(&self, other: &Self) -> bool {
        self.x + self.width + 2 <= other.x
            || other.x + other.width + 2 <= self.x
            || self.y + self.height + 2 <= other.y
            || other.y + other.height + 2 <= self.y
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum GroundWrite {
    Keep,
    Set(u8),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BossAnchor {
    Marker,
    Lich,
    TempleA,
    TempleB,
}
struct Piece {
    width: usize,
    height: usize,
    cells: Vec<u8>,
    temple_ground: Vec<GroundWrite>,
    boss: (usize, usize),
    boss_anchor: BossAnchor,
}
impl Piece {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![0; width * height],
            temple_ground: Vec::new(),
            boss: (width / 2, height / 2),
            boss_anchor: BossAnchor::Marker,
        }
    }
    fn set(&mut self, x: usize, y: usize, value: u8) {
        self.cells[y * self.width + x] = value;
    }
    fn boss_position(&self, kind: u16, area: Area) -> Pos {
        // Community coordinates are not universally cell-centered or rotated with the stencil.
        let offset = match self.boss_anchor {
            BossAnchor::Lich | BossAnchor::TempleA => 0.,
            BossAnchor::TempleB => 0.5,
            BossAnchor::Marker => match kind {
                0x91f => 1.,
                0x67d | 0x928 | 0x925 => 0.,
                _ => 0.5,
            },
        };
        Pos {
            x: (area.x + self.boss.0 as i32) as f32 + offset,
            y: (area.y + self.boss.1 as i32) as f32 + offset,
        }
    }
    fn rotate(self) -> Self {
        let mut out = Self::new(self.height, self.width);
        if !self.temple_ground.is_empty() {
            assert_eq!(self.temple_ground.len(), self.cells.len());
            out.temple_ground = vec![GroundWrite::Keep; self.cells.len()];
        }
        for y in 0..self.height {
            for x in 0..self.width {
                out.set(self.height - 1 - y, x, self.cells[y * self.width + x]);
                if !self.temple_ground.is_empty() {
                    out.temple_ground[x * out.width + self.height - 1 - y] =
                        self.temple_ground[y * self.width + x];
                }
            }
        }
        out.boss_anchor = self.boss_anchor;
        out.boss = match self.boss_anchor {
            BossAnchor::Marker => (self.height - 1 - self.boss.1, self.boss.0),
            BossAnchor::Lich | BossAnchor::TempleA | BossAnchor::TempleB => self.boss,
        };
        out
    }
    fn build(kind: u16, rng: &mut rand::rngs::StdRng) -> Self {
        let mut p = match kind {
            crate::dungeon::tomb::SPHINX => {
                let mut p = Self::new(81, 81);
                for y in 0..81 {
                    for x in 0..81 {
                        let dx = x as f32 - 40.5;
                        let dy = y as f32 - 40.5;
                        if dx.hypot(dy) + rng.gen_range(-2.0..2.0) <= 35. {
                            p.set(x, y, 1);
                        }
                    }
                }
                let center = [
                    0x1ffff_u32,
                    0x7ffc,
                    0x1ff0,
                    0x3ff8,
                    0x3ff8,
                    0x3bb8,
                    0x380,
                    0x100,
                    0x100,
                ];
                for y in 0_usize..17 {
                    for x in 0..17 {
                        if center[y.abs_diff(8)] & (1 << x) != 0 {
                            p.set(32 + x, 32 + y, 2);
                        }
                    }
                }
                for distance in [4, 10] {
                    for dx in [-distance, distance] {
                        for dy in [-distance, distance] {
                            p.set(
                                (40 + dx) as usize,
                                (40 + dy) as usize,
                                if distance == 4 { 3 } else { 4 },
                            );
                        }
                    }
                }
                for (dx, dy) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
                    for (r, s) in [(14, 0), (13, 0), (13, -1), (13, 1)] {
                        p.set(
                            (40 + dx * r - dy * s) as usize,
                            (40 + dy * r + dx * s) as usize,
                            4,
                        );
                    }
                }
                p
            }
            crate::dungeon::beach::BUM => {
                let mut p = Self::new(42, 42);
                for y in 0..42 {
                    for x in 0..42 {
                        let r = (x as f32 - 21.).hypot(y as f32 - 21.);
                        let mark = if r <= 3. {
                            if r > 1.5 && rng.gen_ratio(1, 5) { 4 } else { 1 }
                        } else if r <= 18. {
                            2
                        } else if r < 20. + rng.gen_range(0.0..2.) {
                            3
                        } else {
                            0
                        };
                        p.set(x, y, mark);
                    }
                }
                p
            }
            crate::dungeon::manor::KAGE => {
                let mut p = Self::new(65, 65);
                for y in 1..64 {
                    for x in 1..64 {
                        let radius = (x as f32 - 32.).hypot(y as f32 - 32.);
                        if radius < 28. + rng.gen_range(0.0..4.) {
                            // Trees also occupy the grave field: realm-src@1855c017eab870d13f0b61b1cf0b9e66a01922ff, common/resources/worlds/SP_KageKami.jm.
                            let mark = if rng.gen_ratio(1, if radius > 19. { 7 } else { 30 }) {
                                3
                            } else if rng.gen_ratio(1, 15) {
                                4
                            } else if rng.gen_ratio(1, 30) {
                                5
                            } else if radius > 24. {
                                2
                            } else {
                                1
                            };
                            p.set(x, y, mark);
                        }
                    }
                }
                for n in 10..55 {
                    p.set(32, n, 6);
                    p.set(n, 32, 6);
                }
                for y in (20..=44).step_by(6) {
                    for x in (20..=44).step_by(6) {
                        p.set(x, y, 7);
                    }
                }
                p.boss = (20 + rng.gen_range(0..5) * 6, 20 + rng.gen_range(0..5) * 6);
                p
            }
            0xdc2 => {
                let mut p = Self::new(60, 60);
                for y in 0..60 {
                    for x in 0..60 {
                        let (dx, dy) = ((x as f32 - 30.).abs() / 30., (y as f32 - 30.).abs() / 30.);
                        if dx + rng.gen_range(0.0..0.3) < 0.9
                            && dy + rng.gen_range(0.0..0.3) < 0.9
                            && !rng.gen_ratio(1, 50)
                        {
                            p.set(
                                x,
                                y,
                                if rng.r#gen::<f32>() < (1. - dx.hypot(dy)).powi(2) {
                                    1
                                } else {
                                    5
                                },
                            );
                        }
                    }
                }
                // Temple.cs writes objects over independent ground, including deliberately unwritten holes.
                p.temple_ground = p
                    .cells
                    .iter()
                    .map(|mark| match mark {
                        0 => GroundWrite::Keep,
                        1 => GroundWrite::Set(0xc5),
                        5 => GroundWrite::Set(0x56),
                        _ => unreachable!("invalid Temple ground marker"),
                    })
                    .collect();
                let bas = if rng.gen_bool(0.5) {
                    p.boss_anchor = BossAnchor::TempleA;
                    for n in 0..20 {
                        p.set(17 + n, 17, if n == 19 { 3 } else { 2 });
                        p.set(17 + n, 18, 3);
                        p.set(17, 17 + n, if n == 19 { 3 } else { 2 });
                        if n > 0 {
                            p.set(18, 17 + n, 3);
                        }
                    }
                    for n in 0..19 {
                        p.set(25 + n, 42, 3);
                        p.set(25 + n, 43, if n == 0 { 3 } else { 2 });
                        if n < 18 {
                            p.set(42, 25 + n, 3);
                        }
                        p.set(43, 25 + n, if n == 0 { 3 } else { 2 });
                    }
                    for (x, y) in [
                        (22, 22),
                        (38, 22),
                        (22, 38),
                        (38, 38),
                        (26, 26),
                        (34, 26),
                        (26, 34),
                        (34, 34),
                    ] {
                        p.set(x, y, 4);
                    }
                    17
                } else {
                    p.boss_anchor = BossAnchor::TempleB;
                    for n in 0..23 {
                        if (10..=12).contains(&n) {
                            continue;
                        }
                        for t in [16, 17, 37, 38] {
                            p.set(16 + n, t, 3);
                            p.set(t, 16 + n, 3);
                        }
                    }
                    for n in [7, 8, 9, 13, 14, 15] {
                        for t in [15, 39] {
                            p.set(16 + n, t, 2);
                            p.set(t, 16 + n, 2);
                        }
                    }
                    for y in 0..4 {
                        for x in 0..4 {
                            p.set(21 + x * 4, 21 + y * 4, 4);
                        }
                    }
                    p.boss = (27, 27);
                    16
                };
                for y in 0..60 {
                    for x in 0..60 {
                        if p.cells[y * 60 + x] == 5
                            && ((x > 5 && x < bas)
                                || (x < 55 && x > 60 - bas)
                                || (y > 5 && y < bas)
                                || (y < 55 && y > 60 - bas))
                        {
                            let r: f32 = rng.r#gen();
                            if r > 0.33 {
                                p.set(
                                    x,
                                    y,
                                    if r > 0.6 {
                                        6
                                    } else if r > 0.35 {
                                        7
                                    } else {
                                        8
                                    },
                                );
                            }
                        }
                    }
                }
                p
            }
            0x91f => {
                let mut p = Self::new(25, 25);
                let radius = rng.gen_range(20..=25) / 2;
                let mut border = Vec::new();
                for y in 0..25 {
                    for x in 0..25 {
                        let r = (x as f32 - 12.5).hypot(y as f32 - 12.5);
                        if r <= radius as f32 {
                            p.set(x, y, 1);
                            if radius as f32 - r < 1.5 {
                                border.push((x, y));
                            }
                        }
                    }
                }
                border.shuffle(rng);
                for &(x, y) in border.iter().take(border.len().div_ceil(2)) {
                    p.set(x, y, 2);
                }
                p
            }
            0x91b => {
                let mut p = Self::new(25, 26);
                p.boss = (13, 13);
                p.boss_anchor = BossAnchor::Lich;
                for x in 2..23 {
                    for y in 1..24 {
                        p.set(x, y, u8::from(!rng.gen_ratio(1, 10)));
                    }
                }
                for y in 1..24 {
                    p.set(2, y, 2);
                    p.set(22, y, 2);
                }
                for x in 2..23 {
                    p.set(x, 23, 2);
                }
                for x in 0..3 {
                    for y in 0..3 {
                        p.set(x + 1, y, 2);
                        p.set(x + 21, y, 2);
                    }
                }
                for x in 0..5 {
                    for y in 0..5 {
                        if (x == 0 || x == 4) && (y == 0 || y == 4) {
                            continue;
                        }
                        p.set(x, y + 21, 2);
                        p.set(x + 20, y + 21, 2);
                    }
                }
                for y in 0..6 {
                    p.set(9, 4 + 3 * y, 4);
                    p.set(15, 4 + 3 * y, 4);
                }
                for cell in &mut p.cells {
                    if *cell < 2 {
                        continue;
                    }
                    let n: f32 = rng.r#gen();
                    if n < 0.1 {
                        *cell = 1;
                    } else if n < 0.4 {
                        *cell += 1;
                    }
                }
                p
            }
            0x928 => {
                let mut p = Self::new(27, 27);
                let quarter = [
                    "............XX",
                    "........XXXXXX",
                    "......XXXXXXXX",
                    ".....XXXX=====",
                    "....XXX=======",
                    "...XXX========",
                    "..XXX=========",
                    "..XX==========",
                    ".XXX==========",
                    ".XX===========",
                    ".XX===========",
                    ".XX===========",
                    "XXX===========",
                    "XXX===========",
                ];
                for y in 0..27 {
                    for x in 0..27 {
                        let c = quarter[y.min(26 - y)].as_bytes()[x.min(26 - x)];
                        p.set(
                            x,
                            y,
                            match c {
                                b'X' => 2,
                                b'=' => 1,
                                _ => 0,
                            },
                        );
                    }
                }
                for y in 1..4 {
                    for x in 8..19 {
                        p.set(x, y, 1);
                    }
                }
                for x in 12..15 {
                    p.set(x, 0, 1);
                }
                p
            }
            0x675 => {
                let mut p = Self::new(30, 30);
                for y in 0..30 {
                    for x in 0..30 {
                        let r = (x as f32 - 15.).hypot(y as f32 - 15.) + rng.gen_range(-2.0..2.0);
                        if r <= 10. {
                            p.set(x, y, 1);
                        }
                    }
                }
                p
            }
            0x678 => {
                let mut p = Self::new(30, 30);
                let mut border = Vec::new();
                for y in 0..30 {
                    for x in 0..30 {
                        let r = (x as f32 - 15.).hypot(y as f32 - 15.);
                        if r <= 13. {
                            p.set(x, y, if r <= 10. && r > 3. { 2 } else { 1 });
                        }
                        if (r <= 10. && r > 9.) || (r <= 3. && r > 2.) {
                            border.push((x, y));
                        }
                    }
                }
                border.shuffle(rng);
                // Keep cardinal approaches open without underplanting the reference's half-border count.
                for &(x, y) in border
                    .iter()
                    .filter(|&&(x, y)| x != 15 && y != 15)
                    .take(border.len().div_ceil(2))
                {
                    p.set(x, y, 3);
                }
                p
            }
            0x67d => {
                let mut p = Self::new(31, 40);
                for (ox, oy) in [(0, 0), (18, 0), (0, 27), (18, 27)] {
                    for x in 0..13 {
                        for y in 0..13 {
                            if ((x == 0 || x == 12) && !(3..=9).contains(&y))
                                || ((y == 0 || y == 12) && !(3..=9).contains(&x))
                            {
                                continue;
                            }
                            p.set(ox + x, oy + y, 2);
                        }
                    }
                }
                for x in 3..28 {
                    for y in 3..37 {
                        if !(6..=24).contains(&x) || !(6..=33).contains(&y) {
                            p.set(x, y, 2);
                        }
                    }
                }
                for x in 7..24 {
                    for y in 7..33 {
                        p.set(x, y, u8::from(!rng.gen_ratio(1, 3)));
                    }
                }
                for (ox, oy) in [(3, 3), (21, 3), (3, 30), (21, 30)] {
                    for x in 0..7 {
                        for y in 0..7 {
                            if ((x == 0 || x == 6) && y != 3) || ((y == 0 || y == 6) && x != 3) {
                                continue;
                            }
                            p.set(ox + x, oy + y, 4);
                        }
                    }
                }
                for x in 6..25 {
                    p.set(x, 6, 4);
                    p.set(x, 33, 4);
                }
                for y in 6..34 {
                    p.set(6, y, 4);
                    p.set(24, y, 4);
                }
                for x in 13..18 {
                    for y in 3..7 {
                        p.set(x, y, 6);
                    }
                }
                for c in &mut p.cells {
                    if *c < 2 {
                        continue;
                    }
                    let n: f32 = rng.r#gen();
                    if *c == 6 {
                        if n < 0.4 {
                            *c = 0;
                        }
                    } else if n < 0.1 {
                        *c = 1;
                    } else if n < 0.4 {
                        *c += 1;
                    }
                }
                p.set(15, 27, 7);
                p.set(15, 20, 8);
                p.boss = (15, 20);
                p
            }
            0x925 => {
                let mut p = Self::new(23, 35);
                for y in 0..35 {
                    for x in 0..23 {
                        let floor = u8::from(!rng.gen_ratio(1, 3));
                        p.set(x, y, if x % 22 == 0 || y % 34 == 0 { 2 } else { floor });
                    }
                }
                let mut graves = Vec::new();
                for y in 0..11 {
                    for x in 0..7 {
                        let (x, y) = (2 + 3 * x, 2 + 3 * y);
                        if rng.gen_ratio(2, 3) {
                            p.set(x, y, 4);
                        } else {
                            graves.push((x, y));
                        }
                    }
                }
                for c in p.cells.iter_mut().filter(|c| **c == 2) {
                    let n: f32 = rng.r#gen();
                    if n < 0.1 {
                        *c = 1;
                    } else if n < 0.4 {
                        *c = 3;
                    }
                }
                let (x, y) = *graves.choose(rng).expect("Graveyard left no open grave");
                p.set(x, y, 7);
                p.set(x + 1, y, 8);
                p.boss = (x + 1, y);
                p
            }
            0x668 => {
                let mut p = Self::new(40, 40);
                for x in 0..40 {
                    let t = x as f32 / 40. * std::f32::consts::PI;
                    let low =
                        ((t - 2. * t.sin() / 5.5) / std::f32::consts::PI * 40.).ceil() as usize;
                    let high = ((t + t.sin() / 5.5) / std::f32::consts::PI * 40.).floor() as usize;
                    for y in low..high {
                        p.set(x, y, if rng.gen_ratio(1, 5) { 2 } else { 1 });
                    }
                }
                p
            }
            0xd56 => {
                let mut p = Self::new(33, 33);
                for x in 0..33 {
                    for y in 0..33 {
                        if (x as f32 - 16.).abs() / 16.5 + rng.gen_range(0.0..0.3) < 0.95
                            && (y as f32 - 16.).abs() / 16.5 + rng.gen_range(0.0..0.3) < 0.95
                        {
                            p.set(x, y, 1);
                        }
                    }
                }
                for _ in 0..2 {
                    for x in 12..21 {
                        for y in 4..29 {
                            p.set(x, y, 2);
                        }
                    }
                    p = p.rotate();
                }
                for _ in 0..2 {
                    for x in 13..20 {
                        for y in 5..28 {
                            p.set(x, y, 4);
                        }
                    }
                    p = p.rotate();
                }
                for _ in 0..4 {
                    for x in 13..20 {
                        for y in 5..7 {
                            p.set(x, y, 3);
                        }
                    }
                    p = p.rotate();
                }
                for _ in 0..4 {
                    for (x, y) in [(13, 7), (19, 7), (13, 10), (19, 10)] {
                        p.set(x, y, if rng.gen_ratio(1, 3) { 6 } else { 5 });
                    }
                    p = p.rotate();
                }
                let erosion = super::skull_erosion::SkullErosion::new(rng.r#gen());
                for x in 0..33 {
                    for y in 0..33 {
                        if erosion.erodes(x, y) {
                            p.set(x, y, 0);
                        }
                    }
                }
                p
            }
            // FSoD@6fd20aa realm/setpieces/GhostShip.cs authors water and beach, never its tree markers.
            0xe37 => {
                let mut p = Self::new(50, 50);
                for y in 0..50 {
                    for x in 0..50 {
                        let radius = (x as f32 - 25.).hypot(y as f32 - 25.);
                        if radius <= 17. {
                            p.set(x, y, if radius <= 14. { 2 } else { 1 });
                        }
                    }
                }
                p
            }
            0x935 | 0xd59 | 0xd50 => Self::new(5, 5),
            0xd5f => {
                let mut p = Self::new(41, 41);
                for n in 0..5 {
                    let a = n as f32 * std::f32::consts::TAU / 5.;
                    let ox = (a.cos() * 15. + 17.) as usize;
                    let oy = (a.sin() * 15. + 17.) as usize;
                    for x in 0..7 {
                        for y in 0..7 {
                            if !(matches!(x, 0 | 6) && matches!(y, 0 | 1 | 5 | 6)
                                || matches!(x, 1 | 5) && matches!(y, 0 | 6))
                            {
                                p.set(ox + x, oy + y, 1);
                            }
                        }
                    }
                    p.set(ox + 3, oy + 3, 2);
                }
                p
            }
            _ => unreachable!("named setpiece kind validated by caller"),
        };
        if matches!(kind, 0x91b | 0x928 | 0x67d | 0x668 | 0xdc2 | 0x925) {
            for _ in 0..rng.gen_range(0..4) {
                p = p.rotate();
            }
        }
        if kind == 0x668 {
            p.boss = (20, 20);
        }
        if kind == 0x928 {
            // The selected primary assigns its spawn-only marker after rotating the tiles.
            p.boss = (19, 13);
            p.set(19, 13, 0);
        } else if !matches!(
            kind,
            0x91b
                | 0x935
                | 0xd59
                | 0xd50
                | 0xd5f
                | 0xd56
                | 0xe37
                | 0x67d
                | 0x925
                | crate::dungeon::tomb::SPHINX
        ) {
            p.set(p.boss.0, p.boss.1, if kind == 0x668 { 2 } else { 1 });
            if kind == 0xdc2 {
                p.temple_ground[p.boss.1 * p.width + p.boss.0] = GroundWrite::Set(0xc5);
            }
        }
        p
    }
    fn tile(kind: u16, mark: u8) -> (u8, Option<u16>) {
        match (kind, mark) {
            (crate::dungeon::tomb::SPHINX, 1) => (0xf5, None),
            (crate::dungeon::tomb::SPHINX, 2) => (0xf7, None),
            (crate::dungeon::tomb::SPHINX, 3) => (0xf7, Some(crate::dungeon::tomb::WALL)),
            (crate::dungeon::tomb::SPHINX, 4) => (0xf5, Some(crate::dungeon::tomb::WALL)),
            (crate::dungeon::beach::BUM, 1 | 3) => (0x72, None),
            (crate::dungeon::beach::BUM, 2) => (0x73, None),
            (crate::dungeon::beach::BUM, 4) => (0x72, Some(0x18e)),
            (crate::dungeon::manor::KAGE, 1) => (0x57, None),
            (crate::dungeon::manor::KAGE, 2) => (0x56, None),
            (crate::dungeon::manor::KAGE, 3) => (0x57, Some(0x181e)),
            (crate::dungeon::manor::KAGE, 4) => (0x57, Some(0x181b)),
            (crate::dungeon::manor::KAGE, 5) => (0x57, Some(0x181c)),
            (crate::dungeon::manor::KAGE, 6) => (0xf1, None),
            (crate::dungeon::manor::KAGE, 7) => (0x57, Some(crate::dungeon::manor::GRAVE)),
            (0xdc2, 1) => (0xc5, None),
            (0xdc2, 2) => (0xc5, Some(0x1f4)),
            (0xdc2, 3) => (0xc5, Some(0x1f2)),
            (0xdc2, 4) => (0xc5, Some(0x1f3)),
            (0xdc2, 5) => (0x56, None),
            (0xdc2, 6) => (0x56, Some(0xdd4)),
            (0xdc2, 7) => (0x56, Some(0xdd5)),
            (0xdc2, 8) => (0x56, Some(0xdda)),
            (0x91f, 1) => (0x46, None),
            (0x91f, 2) => (0x46, Some(0x194)),
            (0x91b, 1) => (0xad, None),
            (0x91b, 2) => (0xad, Some(0x160)),
            (0x91b, 3) => (0xad, Some(0x162)),
            (0x91b, 4) => (0xad, Some(0x17d)),
            (0x91b, 5) => (0xad, Some(0x182)),
            (0x928, 1) => (0x60, None),
            (0x928, 2) => (0x60, Some(0x120)),
            (0x675 | 0xd5f, 1) | (0xd5f, 2) => (0xf2, None),
            (0x678, 1) => (0x46, None),
            (0x678, 2) => (0x73, None),
            (0x678, 3) => (0x46, Some(0x18e)),
            (0x67d, 1 | 7) => (0x60, None),
            (0x67d, 2) => (0x73, None),
            (0x67d, 3) => (0xbc, None),
            (0x67d, 4) => (0x60, Some(0x120)),
            (0x67d, 5) => (0x60, Some(0x122)),
            (0x67d, 6) => (0xcb, None),
            (0x925, 1 | 7) => (0x48, None),
            (0x925, 2) => (0x48, Some(0x120)),
            (0x925, 3) => (0x48, Some(0x122)),
            (0x925, 4) => (0x48, Some(0x184)),
            (0x668, 1) => (0xf4, None),
            (0x668, 2) => (0xf4, Some(0x153)),
            (0xd56, 1) => (0x57, None),
            (0xd56, 2) => (0x19, None),
            (0xd56, 3) => (0x18, None),
            (0xd56, 4) => (0xd6, None),
            (0xd56, 5) => (0xd6, Some(0x17d)),
            (0xd56, 6) => (0xd6, Some(0x182)),
            (0xd59, 1) => (0x60, None),
            (0xe37, 1) => (0xed, None),
            (0xe37, 2) => (0xfa, None),
            _ => unreachable!("setpiece marker has no selected tile"),
        }
    }
}
impl World {
    pub(super) fn spawn_landmark_chests(&mut self, kind: u16, pos: Pos) -> Result<()> {
        let items = crate::loot::landmark_chest(&self.content, kind, &mut self.rng)?;
        // Independent period rolls can exceed one container; never discard earned items.
        for chunk in items.chunks(8) {
            let mut slots = chunk.to_vec();
            slots.resize(8, -1);
            self.spawn_entity(0x501, pos, slots)?;
        }
        tracing::info!(kind, count = items.len(), "landmark chest rewards created");
        Ok(())
    }
    pub(super) fn setpiece_scenery_clear(&self, index: usize) -> bool {
        self.cells.ids(index).iter().all(|id| {
            let e = &self.entities[id];
            let object = &self.content.objects[&e.kind];
            object.static_object
                && !object.enemy
                && !matches!(object.class.as_str(), "Container" | "Portal")
                && e.items.is_empty()
                && !e.reserved
                && e.parent.is_none()
                && e.expires.is_none()
                && e.quest.is_none()
                && e.population_slot.is_none()
                && !e.extra_stats.iter().any(|(kind, _)| *kind == 54)
        })
    }
    pub(super) fn event_site_clear(&self, site: &EventSite) -> bool {
        site.cells
            .iter()
            .all(|(index, _)| self.setpiece_scenery_clear(*index))
    }
    pub(super) fn capture_event_site(
        &self,
        area: Area,
        indices: impl IntoIterator<Item = usize>,
    ) -> Result<EventSite> {
        let mut site = EventSite {
            area,
            cells: indices
                .into_iter()
                .map(|i| (i, self.map.cells[i].clone()))
                .collect(),
            scenery: vec![],
        };
        ensure!(
            self.event_site_clear(&site),
            "event site contains a live or owned actor"
        );
        site.scenery = site
            .cells
            .iter()
            .flat_map(|(i, _)| self.cells.ids(*i).iter().copied())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|id| self.entities[&id].clone())
            .collect();
        Ok(site)
    }
    pub(super) fn swap_event_site(&mut self, incoming: &EventSite) -> Result<EventSite> {
        let previous =
            self.capture_event_site(incoming.area, incoming.cells.iter().map(|(i, _)| *i))?;
        let retiring: BTreeSet<_> = previous.scenery.iter().map(|e| e.id).collect();
        ensure!(
            incoming
                .scenery
                .iter()
                .all(|e| !self.entities.contains_key(&e.id) || retiring.contains(&e.id)),
            "reserved event scenery identity reused"
        );
        for id in retiring {
            self.remove_entity(id);
        }
        for (index, cell) in &incoming.cells {
            Arc::make_mut(&mut self.map).cells[*index] = cell.clone();
        }
        for entity in &incoming.scenery {
            let id = entity.id;
            assert!(
                self.entities.insert(id, entity.clone()).is_none(),
                "event scenery identity changed after preflight"
            );
            self.index_actor(id);
        }
        Ok(previous)
    }
    pub(super) fn outside_setpieces(&self, pos: Pos) -> bool {
        // Most Realm cells are beyond the bounding box, so avoid millions of hypot calls.
        self.setpieces.iter().all(|a| {
            pos.x <= a.x as f32 - 4.
                || pos.x >= (a.x + a.width) as f32 + 4.
                || pos.y <= a.y as f32 - 4.
                || pos.y >= (a.y + a.height) as f32 + 4.
                || a.distance(pos) >= 4.
        })
    }
    pub(super) fn retains_authored_lava(&self) -> bool {
        matches!(
            self.realm_layout.as_deref(),
            Some(
                "classic-world-01-reconstruction"
                    | "classic-world-02-reconstruction"
                    | "classic-world-03-reconstruction"
                    | "classic-world-05-reconstruction"
                    | "classic-world-09-reconstruction"
                    | "classic-world-11-reconstruction"
            )
        )
    }
    fn setpiece_cell_available(
        &self,
        cell: &crate::content::Cell,
        untouched: bool,
        retain_authored_lava: bool,
    ) -> bool {
        // Transparent stencils must not bypass source-bound whole-site lava protection.
        !(retain_authored_lava && cell.ground == 0xf4)
            && (untouched
                || cell.region == 0
                    && cell
                        .object
                        .is_none_or(|kind| !self.content.objects[&kind].full_occupy)
                    && self.content.grounds[&cell.ground] > 0.
                    && !matches!(cell.ground, 0x72 | 0x73))
    }
    pub(super) fn place_named_setpiece(&mut self, kind: u16, terrains: &[u8]) -> Result<Pos> {
        ensure!(
            self.players.is_empty() && self.now == 0,
            "setpieces require an unentered Realm"
        );
        let retain_authored_lava = self.retains_authored_lava();
        let piece = Piece::build(kind, &mut self.rng);
        ensure!(
            kind != 0xdc2 || piece.temple_ground.len() == piece.cells.len(),
            "Temple ground plane is incomplete"
        );
        for write in &piece.temple_ground {
            if let GroundWrite::Set(ground) = write {
                ensure!(
                    self.content.grounds.contains_key(ground),
                    "Temple missing selected ground {ground:#x}"
                );
            }
        }
        for mark in piece
            .cells
            .iter()
            .copied()
            .filter(|m| *m != 0 && !(matches!(kind, 0x67d | 0x925) && *m == 8))
        {
            let (ground, object) = Piece::tile(kind, mark);
            ensure!(
                self.content.grounds.contains_key(&ground),
                "setpiece {kind:#x} missing selected ground {ground:#x}"
            );
            if let Some(object) = object {
                ensure!(
                    self.content.get(object)?.static_object,
                    "setpiece scenery must be static"
                );
            }
        }
        // Reserved camp positions must remain usable on later reproduction/replacement cycles.
        let protected: Vec<_> = self
            .population
            .iter()
            .map(|s| s.pos)
            .chain(
                self.entities
                    .values()
                    .filter(|e| {
                        let o = &self.content.objects[&e.kind];
                        o.enemy || !o.static_object
                    })
                    .map(|e| e.pos),
            )
            .collect();
        let quest_positions: Vec<_> = self
            .entities
            .values()
            .filter_map(|e| e.quest.map(|_| e.pos))
            .collect();
        let mut candidates: Vec<_> = self
            .map
            .cells
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                terrains
                    .contains(&c.terrain)
                    .then_some((i % self.map.width as usize, i / self.map.width as usize))
            })
            .collect();
        candidates.shuffle(&mut self.rng);
        let chest_cell = if matches!(kind, 0x67d | 0x925) {
            Some(
                piece
                    .cells
                    .iter()
                    .position(|mark| *mark == 7)
                    .context("setpiece missing chest marker")?,
            )
        } else {
            None
        };
        let area = candidates
            .into_iter()
            .map(|(x, y)| Area {
                x: x as i32 - (piece.width / 2) as i32,
                y: y as i32 - (piece.height / 2) as i32,
                width: piece.width as i32,
                height: piece.height as i32,
            })
            .find(|a| {
                let boss = piece.boss_position(kind, *a);
                a.x > 0
                    && a.y > 0
                    && a.x + a.width < self.map.width
                    && a.y + a.height < self.map.height
                    && (!matches!(kind, 0x67d | 0x928 | 0x925) || self.walkable(boss))
                    && (kind != 0xd56
                        || piece.cells[piece.boss.1 * piece.width + piece.boss.0] != 0
                        || self.walkable(boss)
                            && self.setpiece_scenery_clear(boss.y as usize * self.map.width as usize + boss.x as usize))
                    && (!matches!(kind, 0x91b | 0xd5f)
                        || piece.cells[piece.boss.1 * piece.width + piece.boss.0] != 0
                        || self.walkable(boss))
                    && chest_cell.is_none_or(|i| self.walkable(Pos {
                        x: (a.x + (i % piece.width) as i32) as f32 + 0.5,
                        y: (a.y + (i / piece.width) as i32) as f32 + 0.5,
                    }))
                    && (!matches!(kind, 0x935 | 0xd59 | 0xd50)
                        || self.walkable(Pos {
                            x: (a.x + a.width / 2) as f32 + 0.5,
                            y: (a.y + a.height / 2) as f32 + 0.5,
                        }))
                    && self.setpieces.iter().all(|p| a.separate(p))
                    && self
                        .map
                        .spawns
                        .iter()
                        .all(|(x, y)| a.distance(Pos { x: *x, y: *y }) >= 64.)
                    && protected.iter().all(|p| a.distance(*p) >= 4.)
                    // Offset anchors must retain habitat at the actual destination, not only the sampled center.
                    && (!matches!(kind, 0x91b | 0x91f | 0xdc2 | 0x67d | 0x928 | 0x925)
                        || self.map.cell(boss.x, boss.y).is_some_and(|c| terrains.contains(&c.terrain)))
                    && (!matches!(kind, 0x91b | 0x67d | 0xd5f | 0x925)
                        || self.setpiece_scenery_clear(boss.y as usize * self.map.width as usize + boss.x as usize))
                    && quest_positions.iter().all(|p| p.distance(boss) >= 32.)
                    && (a.y..a.y + a.height).all(|y| {
                        (a.x..a.x + a.width).all(|x| {
                            let index = (y * self.map.width + x) as usize;
                            let c = &self.map.cells[index];
                            let mark = piece.cells[(y - a.y) as usize * piece.width + (x - a.x) as usize];
                            let untouched = matches!(
                                kind,
                                0xdc2 | crate::dungeon::beach::BUM | crate::dungeon::tomb::SPHINX
                            ) && mark == 0;
                            self.setpiece_cell_available(c, untouched, retain_authored_lava)
                                && (kind != 0x928 || self.setpiece_scenery_clear(index))
                                && (!matches!((kind, mark), (0x67d, 5 | 6) | (0x91b | 0x925, 3))
                                    || self.walkable(Pos { x: x as f32 + 0.5, y: y as f32 + 0.5 })
                                        && self.setpiece_scenery_clear(index))
                        })
                    })
            })
            .with_context(|| {
                format!(
                    "Realm cannot place {kind:#x} setpiece {}x{} in its selected habitat (layout={:?}, retain_authored_lava={retain_authored_lava})",
                    piece.width, piece.height, self.realm_layout
                )
            })?;
        if crate::quest::EVENT_BOSSES.contains(&kind) {
            let indices = piece
                .cells
                .iter()
                .enumerate()
                .filter(|(_, mark)| **mark != 0)
                .map(|(i, _)| {
                    ((area.y as usize + i / piece.width) * self.map.width as usize)
                        + area.x as usize
                        + i % piece.width
                });
            let original = self.capture_event_site(area, indices)?;
            self.event_sites.push(original);
        }
        for y in 0..piece.height {
            for x in 0..piece.width {
                let mark = piece.cells[y * piece.width + x];
                // Castle and Graveyard boss8 enter independently, without replacing their native cells.
                if mark == 0 || matches!(kind, 0x67d | 0x925) && mark == 8 {
                    continue;
                }
                let pos = Pos {
                    x: area.x as f32 + x as f32 + 0.5,
                    y: area.y as f32 + y as f32 + 0.5,
                };
                if matches!(kind, 0x67d | 0x925) && mark == 7 {
                    self.spawn_landmark_chests(kind, pos)?;
                    continue;
                }
                let (ground, object) = Piece::tile(kind, mark);
                let index = pos.y as usize * self.map.width as usize + pos.x as usize;
                if matches!((kind, mark), (0x67d, 5 | 6) | (0x91b | 0x925, 3)) {
                    // The community renderers retain native scenery beneath these ground-only writes.
                    Arc::make_mut(&mut self.map).cells[index].ground = ground;
                    if let Some(object) = object {
                        self.spawn_entity(object, pos, vec![])?;
                    }
                    continue;
                }
                let old: Vec<_> = self.cells.ids(index).to_vec();
                for id in old {
                    self.remove_entity(id);
                }
                let cell = &mut Arc::make_mut(&mut self.map).cells[index];
                cell.ground = if kind == 0xdc2 {
                    match piece.temple_ground[y * piece.width + x] {
                        GroundWrite::Keep => cell.ground,
                        GroundWrite::Set(ground) => ground,
                    }
                } else {
                    ground
                };
                cell.object = object;
                // Pyre clones native tiles, changing ground/object but retaining their raw configuration.
                if kind != 0x675 {
                    cell.config.clear();
                }
                let connection = (object == Some(crate::dungeon::tomb::WALL)).then(|| {
                    [(0, -1), (1, 0), (0, 1), (-1, 0)]
                        .into_iter()
                        .fold(0_i32, |bytes, (dx, dy)| {
                            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                            let joined = nx >= 0
                                && ny >= 0
                                && nx < piece.width as i32
                                && ny < piece.height as i32
                                && matches!(
                                    piece.cells[ny as usize * piece.width + nx as usize],
                                    3 | 4
                                );
                            (bytes << 8) | if joined { 1 } else { 2 }
                        })
                });
                if let Some(connection) = connection {
                    cell.config = format!("conn:{connection}");
                }
                if (x, y) == piece.boss && matches!(kind, 0x668 | 0x675 | 0x678) {
                    self.spawn_landmark_chests(kind, pos)?;
                }
                if let Some(object) = object {
                    let id = self.spawn_entity(object, pos, vec![])?;
                    if let Some(connection) = connection {
                        self.entities
                            .get_mut(&id)
                            .unwrap()
                            .extra_stats
                            .push((41, connection));
                    }
                    if object == 0x194 || kind == 0x678 && object == 0x18e {
                        let size = if self.rng.gen_bool(0.5) { 120 } else { 140 };
                        self.entities.get_mut(&id).unwrap().size = size;
                        Arc::make_mut(&mut self.map).cells[index].config = format!("size:{size}");
                    }
                }
            }
        }
        let pos = piece.boss_position(kind, area);
        tracing::info!(
            kind,
            layout = ?self.realm_layout,
            retain_authored_lava,
            x = area.x,
            y = area.y,
            width = area.width,
            height = area.height,
            "named Realm boss setpiece placed"
        );
        self.setpieces.push(area);
        Ok(pos)
    }
}
