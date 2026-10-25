use crate::content::Cell;
#[cfg(test)]
use crate::content::{Content, Map};
use anyhow::Result;
#[cfg(test)]
use anyhow::{Context, ensure};
pub(crate) mod prepare;
pub(crate) mod realms;
#[cfg(test)]
mod rivers;

fn mix(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    value ^ (value >> 31)
}
pub(crate) fn noise(seed: u64, x: f64, y: f64) -> f64 {
    let (ix, iy) = (x.floor() as u64, y.floor() as u64);
    let sample = |x: u64, y: u64| {
        (mix(seed ^ x.wrapping_mul(0x9e3779b97f4a7c15) ^ y.wrapping_mul(0xd1b54a32d192ed03)) >> 11)
            as f64
            / ((1u64 << 53) as f64)
    };
    let smooth = |v: f64| v * v * (3. - 2. * v);
    let (fx, fy) = (smooth(x.fract()), smooth(y.fract()));
    let a = sample(ix, iy) * (1. - fx) + sample(ix + 1, iy) * fx;
    let b = sample(ix, iy + 1) * (1. - fx) + sample(ix + 1, iy + 1) * fx;
    a * (1. - fy) + b * fy
}
#[cfg(test)]
fn biome(depth: f64, moisture: f64, radius: f64) -> (u8, u8) {
    if depth < -4. {
        return (0xbc, 0);
    }
    if depth < 0. {
        return (0x72, 0);
    }
    if depth < 4. {
        return (0x73, 0);
    }
    if depth < 12. {
        return (0xbe, 11);
    }
    let lowland = match moisture {
        m if m > 5. / 7. => (0x57, 10),
        m if m > 4. / 7. => (0x56, 10),
        m if m > 3. / 7. => (0x48, 9),
        _ => (0xbd, 8),
    };
    if depth < 20. && moisture > 3. / 7. {
        return (lowland.0, 12);
    }
    let elevation = depth / radius;
    let variant = if moisture > 5. / 7. {
        2
    } else if moisture > 3. / 7. {
        1
    } else {
        0
    };
    if elevation >= 0.5 {
        (if variant == 2 { 0x6a } else { 0x60 }, 1)
    } else if elevation > 0.35 {
        ([0xbd, 0x23, 0x47][variant], [2, 3, 4][variant])
    } else if elevation > 0.15 {
        ([0xbd, 0x46, 0x56][variant], [5, 6, 7][variant])
    } else {
        lowland
    }
}

fn decorate(cell: &mut Cell, random: u64) -> Result<()> {
    let trees: &[u16] = &[0x100, 0x101, 0x105, 0x106];
    let floor: &[u16] = &[
        0x102, 0x108, 0x109, 0x110, 0x111, 0x112, 0x1a1, 0x1a2, 0x1a3, 0x1a4, 0x1a5, 0x1a6, 0x1a7,
        0x1a8,
    ];
    let cactus: &[u16] = &[
        0x197, 0x198, 0x199, 0x19a, 0x19b, 0x19c, 0x19d, 0x19e, 0x19f,
    ];
    // Ground, not monster tier, distinguishes desert, forest and snowy scenery.
    let (density, share, primary, secondary): (f64, f64, &[u16], &[u16]) = match cell.ground {
        0x60 => (0.01, 1., &[0x107], &[0x107]),
        0xbd => (0.10, 1., cactus, cactus),
        0x48 => (0.15, 0.15, trees, floor),
        0x46 => (0.14, 0.08, &[0x100, 0x101], floor),
        0x56 => (0.22, 0.75, trees, floor),
        0x57 => (0.27, 0.80, trees, floor),
        0x23 => (
            0.19,
            0.30,
            &[0x185, 0x186, 0x187, 0x188, 0x189],
            &[
                0x100, 0x101, 0x102, 0x103, 0x108, 0x109, 0x110, 0x111, 0x112, 0x1a1, 0x1a2, 0x1a3,
                0x1a4, 0x1a5, 0x1a6, 0x1a7, 0x1a8, 0x1aa, 0x1ab, 0x1ac, 0x1ad, 0x1ae, 0x1af, 0x1b0,
                0x1b1,
            ],
        ),
        0x47 => (
            0.13,
            0.30,
            &[0x101, 0x102, 0x105, 0x106],
            &[
                0x108, 0x1aa, 0x1ab, 0x1ac, 0x1ad, 0x1ae, 0x1af, 0x1b0, 0x1b1, 0x1b3, 0x1b4, 0x1b5,
                0x1b6, 0x1b7, 0x1b8, 0x1b9, 0x1ba,
            ],
        ),
        0x72 => (0.10, 1., &[0x113, 0x114], &[0x113, 0x114]),
        0xbc | 0x73 | 0xbe | 0x6a | 0xd0 => return Ok(()),
        ground => anyhow::bail!("Realm scenery has no ground palette {ground:#x}"),
    };
    let roll = (random >> 11) as f64 / ((1u64 << 53) as f64);
    if roll >= density {
        return Ok(());
    }
    let palette = if roll < density * share {
        primary
    } else {
        secondary
    };
    let kind = palette[(mix(random) % palette.len() as u64) as usize];
    cell.object = Some(kind);
    if matches!(kind, 0x100..=0x104 | 0x185..=0x18d | 0x197..=0x19f) {
        cell.config = format!("size:{}", 130 + (mix(random ^ 1) % 7) * 5);
    }
    Ok(())
}

#[cfg(test)]
pub fn generate(content: &Content, seed: u64, size: i32) -> Result<Map> {
    ensure!(
        (192..=1024).contains(&size),
        "Realm size must be 192–1024 tiles"
    );
    let started = std::time::Instant::now();
    let center = size as f64 / 2.;
    let radius = size as f64 * 0.44;
    let phase = (mix(seed) >> 11) as f64 / ((1u64 << 53) as f64) * std::f64::consts::TAU;
    let mut cells = Vec::with_capacity((size * size) as usize);
    let mut heights = Vec::with_capacity((size * size) as usize);
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x as f64 + 0.5 - center, y as f64 + 0.5 - center);
            let angle = dy.atan2(dx);
            let coast = radius
                * (1. + 0.045 * (angle * 3. + phase).sin() + 0.025 * (angle * 5. - phase).cos());
            let depth = coast - dx.hypot(dy);
            let moisture = 0.7 * noise(seed, x as f64 / 48., y as f64 / 48.)
                + 0.3 * noise(seed ^ 0x6a09e667f3bcc909, x as f64 / 96., y as f64 / 96.);
            let (mut ground, mut terrain) = biome(depth, moisture, radius);
            heights.push(depth / radius);
            let ring = [0.10, 0.25, 0.42]
                .iter()
                .any(|h| (depth - radius * h).abs() < 1.3);
            // Contour roads let levelling players travel without spawning fresh encounters on them.
            if matches!(terrain, 2..=10) && ring {
                ground = 0xd0;
                terrain = 0;
            }
            cells.push(Cell {
                ground,
                object: None,
                config: String::new(),
                terrain,
                region: 0,
            });
        }
    }
    let streams = rivers::carve(&mut cells, &heights, seed, size)
        .with_context(|| format!("Realm drainage failed: seed={seed}, size={size}"))?;
    for (i, cell) in cells.iter_mut().enumerate() {
        decorate(cell, mix(seed ^ 0xbb67ae8584caa73b ^ i as u64))?;
    }
    let mut spawns = Vec::new();
    // Rivers may cut a cardinal axis, so search each coast after carving.
    for (dx, dy) in [(0, 1), (1, 0), (0, -1), (-1, 0)] {
        let (x, y) = (2..size - 2)
            .flat_map(|x| (2..size - 2).map(move |y| (x, y)))
            .filter(|(x, y)| {
                let (rx, ry) = (x - size / 2, y - size / 2);
                let along = dx * rx + dy * ry;
                along > (dx * ry - dy * rx).abs()
                    && (y - 1..=y + 1).all(|ny| {
                        (x - 1..=x + 1).all(|nx| cells[(ny * size + nx) as usize].terrain == 11)
                    })
            })
            .min_by_key(|(x, y)| {
                let (rx, ry) = (x - size / 2, y - size / 2);
                ((dx * ry - dy * rx).abs(), -(dx * rx + dy * ry))
            })
            .with_context(|| format!("generated Realm has no coastal spawn toward {dx},{dy}"))?;
        for ny in y - 1..=y + 1 {
            for nx in x - 1..=x + 1 {
                let cell = &mut cells[(ny * size + nx) as usize];
                cell.object = None;
                cell.config.clear();
                cell.region = 1;
            }
        }
        spawns.push((x as f32 + 0.5, y as f32 + 0.5));
    }
    let map = Map {
        width: size,
        height: size,
        cells,
        spawns,
    };
    for cell in &map.cells {
        ensure!(
            content.grounds.contains_key(&cell.ground),
            "generated Realm uses unknown ground {:#x}",
            cell.ground
        );
        if let Some(object) = cell.object {
            let definition = content.get(object)?;
            ensure!(
                definition.static_object && !definition.enemy,
                "Realm scenery is not static: {object:#x}"
            );
        }
    }
    tracing::info!(
        seed,
        size,
        spawn_points = map.spawns.len(),
        elapsed_ms = started.elapsed().as_millis(),
        roads = map.cells.iter().filter(|c| c.ground == 0xd0).count(),
        scenery = map.cells.iter().filter(|c| c.object.is_some()).count(),
        rocks = map.cells.iter().filter(|c| c.object == Some(0x107)).count(),
        rivers = streams.headwaters,
        river_edges = streams.edges,
        river_confluences = streams.confluences,
        river_bends = streams.bends,
        "generated Realm terrain; original generator unverified"
    );
    Ok(map)
}
