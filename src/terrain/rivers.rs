use super::mix;
use crate::content::Cell;
use anyhow::{Context, Result, ensure};
use delaunator::{EMPTY, Point, triangulate};
use std::collections::VecDeque;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Rivers {
    pub headwaters: usize,
    pub edges: usize,
    pub confluences: usize,
    pub bends: usize,
}

fn index(x: i32, y: i32, size: i32) -> Option<usize> {
    if x < 0 || y < 0 || x >= size || y >= size {
        return None;
    }
    Some((y * size + x) as usize)
}
fn line(a: &Point, b: &Point) -> impl Iterator<Item = (i32, i32)> {
    let (x, y, dx, dy) = (a.x, a.y, b.x - a.x, b.y - a.y);
    // Co-circular triangles may share a circumcenter.
    let steps = (dx.abs().max(dy.abs()) * 2.).ceil().max(1.) as usize;
    (0..=steps).map(move |i| {
        let t = i as f64 / steps as f64;
        ((x + dx * t).floor() as i32, (y + dy * t).floor() as i32)
    })
}
fn center(a: &Point, b: &Point, c: &Point) -> Result<Point> {
    let (dx, dy, ex, ey) = (b.x - a.x, b.y - a.y, c.x - a.x, c.y - a.y);
    let d = 2. * (dx * ey - dy * ex);
    ensure!(d != 0., "degenerate drainage triangle");
    let (u, v) = (dx * dx + dy * dy, ex * ex + ey * ey);
    let p = Point {
        x: a.x + (ey * u - dy * v) / d,
        y: a.y + (dx * v - ex * u) / d,
    };
    ensure!(
        p.x.is_finite() && p.y.is_finite(),
        "non-finite drainage corner"
    );
    Ok(p)
}

pub(super) fn carve(cells: &mut [Cell], heights: &[f64], seed: u64, size: i32) -> Result<Rivers> {
    let side = size / 16;
    let spacing = size as f64 / side as f64;
    let mut sites = Vec::new();
    for y in 0..=side {
        for x in 0..=side {
            let n = (y * (side + 1) + x) as u64;
            let jitter = |axis| {
                let r = mix(seed ^ n ^ axis);
                ((r >> 11) as f64 / ((1u64 << 53) as f64) - 0.5) * spacing * 0.7
            };
            let border = x == 0 || y == 0 || x == side || y == side;
            sites.push(Point {
                x: x as f64 * spacing
                    + if border {
                        0.
                    } else {
                        jitter(0x510e527fade682d1)
                    },
                y: y as f64 * spacing
                    + if border {
                        0.
                    } else {
                        jitter(0x9b05688c2b3e6c1f)
                    },
            });
        }
    }
    let triangles = triangulate(&sites);
    ensure!(
        !triangles.triangles.is_empty(),
        "Realm drainage triangulation is empty"
    );
    let corners = triangles
        .triangles
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| center(&sites[t[0]], &sites[t[1]], &sites[t[2]]))
        .collect::<Result<Vec<_>>>()?;
    let tiles: Vec<_> = corners
        .iter()
        .map(|p| index(p.x.floor() as i32, p.y.floor() as i32, size))
        .collect();
    // Hull circumcenters outside the viewport are ocean outlets, not inland sinks.
    let water: Vec<_> = tiles
        .iter()
        .map(|i| match i {
            Some(i) => matches!(cells[*i].ground, 0xbc | 0x72 | 0x73),
            None => true,
        })
        .collect();
    let allowed: Vec<_> = tiles
        .iter()
        .map(|i| i.is_none_or(|i| cells[i].terrain != 1))
        .collect();
    let mut graph = vec![Vec::new(); corners.len()];
    for (edge, &twin) in triangles.halfedges.iter().enumerate() {
        if twin == EMPTY || edge > twin {
            continue;
        }
        let (a, b) = (edge / 3, twin / 3);
        if !allowed[a] || !allowed[b] {
            continue;
        }
        let (from, to) = if water[a] { (b, a) } else { (a, b) };
        let clear = (water[a] && water[b])
            || line(&corners[from], &corners[to]).all(|(x, y)| match index(x, y, size) {
                Some(i) => {
                    cells[i].terrain != 1
                        && (water[to] || !matches!(cells[i].ground, 0xbc | 0x72 | 0x73))
                }
                None => water[to],
            });
        if clear {
            graph[a].push(b);
            graph[b].push(a);
        }
    }
    let mut distance = vec![usize::MAX; corners.len()];
    let mut queue = VecDeque::new();
    for (i, sea) in water.iter().enumerate() {
        if *sea {
            distance[i] = 0;
            queue.push_back(i);
        }
    }
    ensure!(!queue.is_empty(), "Realm drainage has no coastal outlet");
    while let Some(i) = queue.pop_front() {
        for &next in &graph[i] {
            if distance[next] == usize::MAX {
                distance[next] = distance[i] + 1;
                queue.push_back(next);
            }
        }
    }
    let downstream: Vec<_> = graph
        .iter()
        .enumerate()
        .map(|(i, neighbors)| {
            neighbors
                .iter()
                .copied()
                .filter(|n| distance[*n] < distance[i])
                .min_by_key(|n| mix(seed ^ ((i as u64) << 32) ^ *n as u64))
        })
        .collect();
    let mut heads: Vec<_> = tiles
        .iter()
        .enumerate()
        .filter_map(|(i, tile)| {
            tile.filter(|t| {
                matches!(cells[*t].terrain, 2..=10)
                    && (0.28..0.45).contains(&heights[*t])
                    && distance[i] != usize::MAX
            })
            .map(|_| i)
        })
        .collect();
    heads.sort_by_key(|i| mix(seed ^ *i as u64));
    let headwaters = (size / 64) as usize;
    ensure!(
        heads.len() >= headwaters,
        "Realm drainage has too few connected headwaters"
    );
    let mut flow = vec![0usize; corners.len()];
    for head in heads.into_iter().take(headwaters) {
        let mut at = head;
        while !water[at] {
            flow[at] += 1;
            at = downstream[at].context("Realm drainage corner has no coastal path")?;
        }
    }
    let base_water: Vec<_> = cells
        .iter()
        .map(|c| matches!(c.ground, 0xbc | 0x72 | 0x73))
        .collect();
    let mut upstream = vec![0; corners.len()];
    let mut edges = 0;
    let mut bends = 0;
    for (i, amount) in flow.into_iter().enumerate().filter(|(_, f)| *f > 0) {
        let next = downstream[i].context("flowing Realm corner has no downstream edge")?;
        upstream[next] += 1;
        edges += 1;
        if let Some(out) = downstream[next] {
            let (ux, uy) = (
                corners[next].x - corners[i].x,
                corners[next].y - corners[i].y,
            );
            let (vx, vy) = (
                corners[out].x - corners[next].x,
                corners[out].y - corners[next].y,
            );
            if (ux * vy - uy * vx).abs() > 0.2 * ux.hypot(uy) * vx.hypot(vy) {
                bends += 1;
            }
        }
        let width = if amount > 1 { 6 } else { 3 };
        for (x, y) in line(&corners[i], &corners[next]) {
            let Some(at) = index(x, y, size) else {
                break;
            };
            if base_water[at] {
                break;
            }
            for ny in y - width / 2..y - width / 2 + width {
                for nx in x - width / 2..x - width / 2 + width {
                    if let Some(j) = index(nx, ny, size)
                        && !base_water[j]
                        && cells[j].terrain != 1
                    {
                        cells[j].ground = 0x73;
                        cells[j].terrain = 0;
                    }
                }
            }
        }
    }
    Ok(Rivers {
        headwaters,
        edges,
        confluences: upstream.into_iter().filter(|n| *n > 1).count(),
        bends,
    })
}
