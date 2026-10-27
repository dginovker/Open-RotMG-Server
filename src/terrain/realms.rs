use crate::content::{Content, Map};
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{collections::VecDeque, path::Path, sync::Arc};

use super::prepare::{
    ScopedLava, WORLD02_ID, WORLD03_ID, WORLD05_ID, WORLD09_ID, WORLD11_ID, bounded_read,
    read_recipe, safe_path, valid_hash, valid_id,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Entry {
    Baseline {
        id: String,
        path: String,
        sha256: String,
    },
    Reconstruction {
        recipe: String,
        sha256: String,
    },
}

pub(crate) struct Asset {
    pub(crate) id: String,
    pub(crate) bytes: Vec<u8>,
    sha256: String,
    profile: String,
    contract: String,
    expected_dimensions: [usize; 2],
    expected_content: Option<[String; 2]>,
    lava: Option<Box<ScopedLava>>,
}
impl Asset {
    fn validate_lava(&self, map: &Map, content: &Content, phase: &str) -> Result<()> {
        if let Some(lava) = &self.lava {
            lava.validate_loaded(map, content)
                .with_context(|| format!("{} lava support {phase}", self.contract))
        } else {
            ensure!(
                !matches!(
                    self.id.as_str(),
                    WORLD02_ID | WORLD03_ID | WORLD05_ID | WORLD09_ID | WORLD11_ID
                ) || !map.cells.iter().any(|c| c.ground == 244),
                "{} requires its source-exact lava capability {phase}",
                self.contract
            );
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Site {
    pub(crate) kind: u16,
    pub(crate) bounds: [i32; 4],
    pub(crate) spawn: [i32; 2],
    pub(crate) scenery: usize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Sites {
    layout: String,
    sha256: String,
    sites: Vec<Site>,
}

pub(crate) enum Source {
    Encoded(Asset),
    #[cfg(test)]
    Fixture(Arc<Map>),
}
impl Source {
    pub(crate) fn sites(&self) -> Result<Vec<Site>> {
        let asset = match self {
            Self::Encoded(asset) if asset.profile == "fsod" => return Ok(Vec::new()),
            Self::Encoded(asset) => asset,
            #[cfg(test)]
            Self::Fixture(_) => return Ok(Vec::new()),
        };
        let records: Vec<Sites> = serde_json::from_str(include_str!("../../data/realm-sites.json"))
            .context("parse authored quest-site registry")?;
        let mut matches = records.into_iter().filter(|r| r.layout == asset.id);
        let record = matches
            .next()
            .with_context(|| format!("{} has no authored quest-site registry", asset.id))?;
        ensure!(
            matches.next().is_none(),
            "duplicate quest-site registry for {}",
            asset.id
        );
        ensure!(
            record.sha256 == asset.sha256,
            "{} quest-site registry hash mismatch: expected {}, loaded {}",
            asset.id,
            record.sha256,
            asset.sha256
        );
        let groves = record
            .sites
            .iter()
            .filter(|site| site.kind == 0x91f)
            .count();
        ensure!(
            (17..25).contains(&groves),
            "{} has invalid authored Grove count {groves}",
            asset.id
        );
        Ok(record.sites)
    }
    pub(crate) fn name(&self) -> &str {
        match self {
            Self::Encoded(asset) => &asset.id,
            #[cfg(test)]
            Self::Fixture(_) => "procedural-test-fixture",
        }
    }
    pub(crate) fn open(&self, content: &Content) -> Result<Arc<Map>> {
        match self {
            Self::Encoded(asset) => {
                let name = &asset.id;
                let bytes = &asset.bytes;
                let started = std::time::Instant::now();
                if let Some(expected) = &asset.expected_content {
                    ensure!(
                        &content.xml_sha256 == expected,
                        "{}: loaded XML identity mismatch [objects, grounds]: expected {expected:?}, actual {:?}",
                        asset.contract,
                        content.xml_sha256
                    );
                }
                let mut map = Map::decode_realm_cells(bytes, content, asset.expected_dimensions)
                    .with_context(|| {
                        format!(
                            "decode Realm layout {} output SHA-256 {} expected dimensions {:?}",
                            asset.contract, asset.sha256, asset.expected_dimensions
                        )
                    })?;
                let [width, height] = asset.expected_dimensions;
                ensure!(
                    map.width == width as i32
                        && map.height == height as i32
                        && map.cells.len() == width * height,
                    "{} output SHA-256 {}: expected dimensions {:?} / {} cells, actual {:?} / {} cells",
                    asset.contract,
                    asset.sha256,
                    asset.expected_dimensions,
                    width * height,
                    [map.width, map.height],
                    map.cells.len()
                );
                asset.validate_lava(&map, content, "before arrivals")?;
                prepare_arrivals(&mut map, content)
                    .with_context(|| format!("prepare Realm layout {name}"))?;
                asset.validate_lava(&map, content, "after arrivals")?;
                tracing::info!(
                    layout = name,
                    sha256 = asset.sha256,
                    profile = asset.profile,
                    width = map.width,
                    height = map.height,
                    scenery = map.cells.iter().filter(|c| c.object.is_some()).count(),
                    elapsed_ms = started.elapsed().as_millis(),
                    "Realm layout opened"
                );
                Ok(Arc::new(map))
            }
            #[cfg(test)]
            Self::Fixture(map) => Ok(map.clone()),
        }
    }
}
pub(crate) fn sources(path: &Path) -> Result<Vec<Source>> {
    let catalog = safe_path(path, "realm-catalog.json")?;
    let entries: Vec<Entry> = serde_json::from_slice(&bounded_read(&catalog, 64 * 1024)?)
        .with_context(|| format!("parse Realm catalog {}", catalog.display()))?;
    ensure!(
        !entries.is_empty(),
        "empty Realm catalog {}",
        catalog.display()
    );
    let mut ids = std::collections::BTreeSet::new();
    entries
        .into_iter()
        .enumerate()
        .map(|(index, entry)| {
            let load = || -> Result<Source> {
                let (
                    id,
                    file,
                    expected,
                    profile,
                    contract,
                    expected_dimensions,
                    expected_content,
                    lava,
                ) = match entry {
                    Entry::Baseline {
                        id,
                        path: relative,
                        sha256,
                    } => {
                        ensure!(
                            !matches!(id.as_str(), WORLD05_ID | WORLD09_ID | WORLD11_ID),
                            "{id} requires its reviewed source recipe"
                        );
                        let contract = format!("{id} fsod baseline");
                        (
                            id,
                            safe_path(path, &relative)?,
                            sha256,
                            "fsod".to_owned(),
                            contract,
                            [2048, 2048],
                            None,
                            None,
                        )
                    }
                    Entry::Reconstruction { recipe, sha256 } => {
                        let root = safe_path(path, "realms")?;
                        let metadata = read_recipe(&root, &recipe).with_context(|| {
                            format!("Realm recipe {recipe} expected output SHA-256 {sha256}")
                        })?;
                        let file = safe_path(&root, metadata.output())?;
                        (
                            metadata.id().to_owned(),
                            file,
                            sha256,
                            metadata.profile().to_owned(),
                            metadata.contract(),
                            metadata.dimensions(),
                            metadata.content_identity(),
                            metadata.lava_scope(&root)?.map(Box::new),
                        )
                    }
                };
                ensure!(valid_id(&id), "invalid Realm catalog ID {id:?}");
                ensure!(ids.insert(id.clone()), "duplicate Realm catalog ID {id}");
                ensure!(
                    valid_hash(&expected),
                    "Realm {id} {}: invalid SHA-256 {expected:?}",
                    file.display()
                );
                let bytes = bounded_read(&file, 16 * 1024 * 1024).with_context(|| {
                    format!("Realm {id} {} expected SHA-256 {expected}", file.display())
                })?;
                let hash = format!("{:x}", Sha256::digest(&bytes));
                ensure!(
                    hash == expected,
                    "Realm {id} {} hash mismatch: expected {expected}, got {hash}",
                    file.display()
                );
                tracing::info!(layout = id, sha256 = hash, profile, "Realm asset verified");
                Ok(Source::Encoded(Asset {
                    id,
                    bytes,
                    sha256: hash,
                    profile,
                    contract,
                    expected_dimensions,
                    expected_content,
                    lava,
                }))
            };
            load().with_context(|| format!("Realm catalog {} entry {index}", catalog.display()))
        })
        .collect()
}

fn prepare_arrivals(map: &mut Map, content: &Content) -> Result<()> {
    let width = map.width as usize;
    let passable: Vec<_> = map
        .cells
        .iter()
        .map(|c| {
            content.grounds[&c.ground] > 0.
                && c.object.is_none_or(|kind| !content.objects[&kind].blocks)
        })
        .collect();
    let start = map
        .cells
        .iter()
        .enumerate()
        .filter(|(i, c)| c.terrain == 1 && passable[*i])
        .min_by_key(|(i, _)| {
            (i % width).abs_diff(width / 2) + (i / width).abs_diff(map.height as usize / 2)
        })
        .map(|(i, _)| i)
        .context("Realm has no walkable Godlands")?;
    // A remote beach island is not a usable arrival even when its surrounding nine cells are clear.
    let mut reachable = vec![false; map.cells.len()];
    let mut queue = VecDeque::from([start]);
    reachable[start] = true;
    while let Some(i) = queue.pop_front() {
        let x = i % width;
        for next in [
            i.wrapping_sub(width),
            i + width,
            if x > 0 { i - 1 } else { usize::MAX },
            if x + 1 < width { i + 1 } else { usize::MAX },
        ] {
            if next < reachable.len() && passable[next] && !reachable[next] {
                reachable[next] = true;
                queue.push_back(next);
            }
        }
    }
    for cell in &mut map.cells {
        if cell.region == 1 {
            cell.region = 0;
        }
    }
    map.spawns.clear();
    for (dx, dy) in [(0, 1), (1, 0), (0, -1), (-1, 0)] {
        let (x, y) = (1..map.height - 1)
            .flat_map(|y| (1..map.width - 1).map(move |x| (x, y)))
            .filter(|(x, y)| {
                let (rx, ry) = (x - map.width / 2, y - map.height / 2);
                dx * rx + dy * ry > (dx * ry - dy * rx).abs()
                    && reachable[(y * map.width + x) as usize]
                    && (y - 1..=y + 1).all(|ny| {
                        (x - 1..=x + 1).all(|nx| {
                            let cell = &map.cells[(ny * map.width + nx) as usize];
                            cell.terrain == 11 && content.grounds[&cell.ground] > 0.
                        })
                    })
            })
            .min_by_key(|(x, y)| {
                let (rx, ry) = (x - map.width / 2, y - map.height / 2);
                ((dx * ry - dy * rx).abs(), -(dx * rx + dy * ry))
            })
            .with_context(|| format!("Realm has no connected coastal arrival toward {dx},{dy}"))?;
        for ny in y - 1..=y + 1 {
            for nx in x - 1..=x + 1 {
                let cell = &mut map.cells[(ny * map.width + nx) as usize];
                cell.object = None;
                cell.config.clear();
                cell.region = 1;
            }
        }
        map.spawns.push((x as f32 + 0.5, y as f32 + 0.5));
    }
    Ok(())
}
