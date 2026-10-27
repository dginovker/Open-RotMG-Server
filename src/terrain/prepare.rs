use crate::content::{Cell, Content, Map};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

const SIDE: usize = 2048;
const CELLS: usize = SIDE * SIDE;
const OBJECTS: &str = "90ab4dbddadfc5adfa77f87746c376d6c611b7b3e02ec0ed8ab4f0ad91534b72";
const GROUNDS: &str = "436b596b062c141112790bb3702e4b25e639a5a27541f2a591c462cf9e1b15f7";

const SOURCE_LOCAL_IDENTITIES: [(&str, &str, usize); 10] = [
    ("classic-world-04-reconstruction", WORLD04_SOURCE, 2000),
    (
        "classic-world-08-reconstruction",
        "d0a544f4943991aa1265b7ebdfecb3a846215f297c1317ffb718ddd89e8d92bc",
        2000,
    ),
    (
        "classic-world-10-reconstruction",
        "d7f3b0d274f4f6b0cf6093a336b8adbfaf322e9ce6d07c970b3219a8d24afd48",
        2000,
    ),
    (
        "classic-world-13-reconstruction",
        "15cb5c88982275f3e72d4dc0bc9fb940177662013cacd6b76c1b95aeca46ecd1",
        2000,
    ),
    (
        "classic-world-06-reconstruction",
        "f5dcb4af75cce476d80bc59760df78ec880211ba9d3d2847225fc3d3a6b1b30a",
        2100,
    ),
    (WORLD05_ID, WORLD05_SOURCE, 2100),
    (
        "classic-world-07-reconstruction",
        "f449de66330c98caf611c8575a2d731baa27b47a2eac96d043a232de2ab00372",
        2100,
    ),
    (WORLD09_ID, WORLD09_SOURCE, 2100),
    (WORLD11_ID, WORLD11_SOURCE, 2100),
    (
        "classic-world-12-reconstruction",
        "fe91b3e5ecd63110f30455c6f63ff66b575483d2e39d40a5d4aaa33b838b73a0",
        2100,
    ),
];

const FULL_SOURCE_COMPAT_IDENTITIES: [(&str, &str, usize); 3] = [
    ("classic-world-01-reconstruction", WORLD01_SOURCE, 2100),
    (
        "classic-world-02-reconstruction",
        "b6db13154d1c78d905770305df725ac912e1de10dd3b52856326e8adc0ba2547",
        2100,
    ),
    (
        "classic-world-03-reconstruction",
        "861351094192223962bc38def3c127b08c3cf80b663dd6bffb770b09ce47136e",
        2100,
    ),
];

fn source_local_transform(dimensions: [usize; 2]) -> serde_json::Value {
    serde_json::json!({
        "kind": "source-local-v1",
        "source_dimensions": dimensions,
        "target_dimensions": dimensions,
        "mapping": "tile_xy=source_xy",
        "historical_coordinates": "UNKNOWN",
        "status": "One source pixel per engineered cell does not prove original extent, pitch or origin."
    })
}

impl ReviewedRecipe {
    fn strict_source(&self) -> bool {
        self.schema == 4 || matches!(self.id.as_str(), WORLD05_ID | WORLD09_ID | WORLD11_ID)
    }
    fn dimensions(&self) -> Result<[usize; 2]> {
        let source = SOURCE_LOCAL_IDENTITIES
            .iter()
            .find(|(id, hash, _)| *id == self.id || *hash == self.provenance.source_sha256);
        let expected = match self.schema {
            2 => {
                ensure!(
                    source.is_none(),
                    "reserved source-local identity {} requires schema 3, not schema 2 padding or alias",
                    self.id
                );
                [SIDE, SIDE]
            }
            3 => {
                ensure!(
                    source
                        .is_some_and(|(id, hash, _)| *id == self.id
                            && *hash == self.provenance.source_sha256),
                    "unapproved source-local identity {} source {}",
                    self.id,
                    self.provenance.source_sha256
                );
                let side = source.unwrap().2;
                [side, side]
            }
            4 => {
                let source = FULL_SOURCE_COMPAT_IDENTITIES
                    .iter()
                    .find(|(id, hash, _)| *id == self.id && *hash == self.provenance.source_sha256);
                let side = source
                    .with_context(|| {
                        format!(
                            "unapproved schema 4 source-local identity {} source {}",
                            self.id, self.provenance.source_sha256
                        )
                    })?
                    .2;
                [side, side]
            }
            schema => bail!("unsupported reviewed recipe schema {schema}"),
        };
        ensure!(
            [self.width, self.height] == expected,
            "{} reviewed-cells-v1 schema {} source {}: expected dimensions {expected:?}, actual {:?}",
            self.id,
            self.schema,
            self.provenance.source_sha256,
            [self.width, self.height]
        );
        Ok(expected)
    }
    fn audit_inputs(&self, root: &Path) -> Result<Vec<u8>> {
        let dimensions = self.dimensions()?;
        let p = &self.provenance;
        if self.strict_source() {
            strict_source_paths(
                root,
                &[
                    &self.inputs.dictionary.path,
                    &self.inputs.indices.path,
                    &p.policy.path,
                    &p.annotations.path,
                    &self.output,
                ],
            )?;
        }
        let policy = hashed_input(root, &p.policy.path, &p.policy.sha256, 64 * 1024)?;
        let annotations =
            hashed_input(root, &p.annotations.path, &p.annotations.sha256, 64 * 1024)?;
        if self.strict_source() {
            strict_source_audit(&policy, &annotations, self)?;
        }
        audit_scoped(
            &policy,
            &annotations,
            p,
            matches!(self.schema, 3 | 4).then_some(dimensions),
        )?;
        Ok(policy)
    }
}

const WORLD01_SOURCE: &str = "7404dfa562627e015160a88820017cfbcfb2c6de3676a18dcca3395bd4fc2cc1";
const WORLD01_LAVA_SUPPORT: &str =
    "93f3410a461d0daa70d6efc79938a5a0938f69018bd32e9c5667bf6ee25a2b17";

const WORLD04_SOURCE: &str = "b22ff9156730a358e39c83ed37d11dadf86f282cd16e663e905f19eaa0cd20f5";
const WORLD04_LAVA_SUPPORT: &str =
    "db7fa56ca8a4ea4396a75483b894262b66fe611b7fdf72b4b574f0999c1e8494";

pub(super) const WORLD05_ID: &str = "classic-world-05-reconstruction";
const WORLD05_SOURCE: &str = "9f57488fdccf12fc1ad5f8970d1feec746ea4e1bbc5ed6718fee7d3d4dbd55c0";
const WORLD05_LAVA_SUPPORT: &str =
    "4a899569757f859d8fe85b7ed06207ffa380b772ee02c87b212abc65904cd3ee";

pub(super) const WORLD09_ID: &str = "classic-world-09-reconstruction";
const WORLD09_SOURCE: &str = "726aa16890f7caaa73dc642b64901e94ed4f44cb974e5001c98897f1b8a89736";
const WORLD09_LAVA_SUPPORT: &str =
    "e1c603f40768fb8d49b945863f65d565bc944b5d2b68080cfa28ac45e0bab257";

pub(super) const WORLD11_ID: &str = "classic-world-11-reconstruction";
const WORLD11_SOURCE: &str = "3d3b93937ca405b4f9ce1e28d11694d2f92654acd6a4b7a11cea6f65976bcb20";
const WORLD11_LAVA_SUPPORT: &str =
    "4f88fb5c98d4e7dbaa7fb6e5dde0dba336c559b9abbb99d3a8532dd2e0120b44";

pub(super) const WORLD02_ID: &str = "classic-world-02-reconstruction";
pub(super) const WORLD03_ID: &str = "classic-world-03-reconstruction";

pub(super) struct ScopedLava {
    world: u8,
    terrain: u8,
    side: usize,
    source_side: usize,
    offset: usize,
    pixels: usize,
    support: &'static str,
}
impl ScopedLava {
    fn for_recipe(
        r: &ReviewedRecipe,
        policy: &serde_json::Value,
        content: &Content,
    ) -> Result<Option<Self>> {
        let scope = Self::binding(r, policy)?;
        if let Some(scope) = scope.as_ref() {
            scope.validate_content(content)?;
        }
        Ok(scope)
    }
    fn binding(r: &ReviewedRecipe, policy: &serde_json::Value) -> Result<Option<Self>> {
        let world01 = r.id == "classic-world-01-reconstruction"
            || r.provenance.source_sha256 == WORLD01_SOURCE
            || policy.get("world01_lava").is_some();
        let world23 = match r.id.as_str() {
            WORLD02_ID => Some((
                2,
                "b6db13154d1c78d905770305df725ac912e1de10dd3b52856326e8adc0ba2547",
                864,
                "c3a7f0e93a5f3565943db894b9bbc65f68ee1828b2ebe65c91ab075682dd222b",
                "world02_lava",
            )),
            WORLD03_ID => Some((
                3,
                "861351094192223962bc38def3c127b08c3cf80b663dd6bffb770b09ce47136e",
                629,
                "48e579763ea271642a779435a3baa38a3656c0d2233ad95d7da70057c460260a",
                "world03_lava",
            )),
            _ => None,
        };
        let world04 = policy.get("world04_lava").is_some();
        let world05 = policy.get("world05_lava").is_some();
        let world09 = policy.get("world09_lava").is_some();
        let world11 = policy.get("world11_lava").is_some();
        ensure!(
            u8::from(world01)
                + u8::from(world23.is_some())
                + u8::from(world04)
                + u8::from(world05)
                + u8::from(world09)
                + u8::from(world11)
                <= 1,
            "mixed Realm lava scopes"
        );
        let (scope, id, source, key, schema, transform) = if world01 {
            let (side, offset, transform) = match r.schema {
                2 => (
                    SIDE,
                    26,
                    serde_json::json!({"crop":[26,26,2074,2074],"historical_coordinates":"UNKNOWN","source_dimensions":[2100,2100],"tile":"source_pixel-26"}),
                ),
                4 => (2100, 0, source_local_transform([2100, 2100])),
                schema => bail!("unapproved World1 lava recipe schema {schema}"),
            };
            (
                Self {
                    world: 1,
                    terrain: 1,
                    side,
                    source_side: 2100,
                    offset,
                    pixels: 703,
                    support: WORLD01_LAVA_SUPPORT,
                },
                "classic-world-01-reconstruction",
                WORLD01_SOURCE,
                "world01_lava",
                r.schema,
                transform,
            )
        } else if let Some((world, source, pixels, support, key)) = world23 {
            // Fissures require damaging Lava Blend, not Red Quad: https://github.com/Zolmex/kithio-as3-source/blob/d15f321edcce0ef890001890c6991c25a1b0f718/Server%20Source/wServer/realm/setpieces/LavaFissure.cs#L18
            (
                Self {
                    world,
                    terrain: 1,
                    side: 2100,
                    source_side: 2100,
                    offset: 0,
                    pixels,
                    support,
                },
                if world == 2 { WORLD02_ID } else { WORLD03_ID },
                source,
                key,
                4,
                source_local_transform([2100, 2100]),
            )
        } else if world04 {
            (
                Self {
                    world: 4,
                    terrain: 1,
                    side: 2000,
                    source_side: 2000,
                    offset: 0,
                    pixels: 506,
                    support: WORLD04_LAVA_SUPPORT,
                },
                "classic-world-04-reconstruction",
                WORLD04_SOURCE,
                "world04_lava",
                3,
                source_local_transform([2000, 2000]),
            )
        } else if world05 {
            (
                Self {
                    world: 5,
                    terrain: 0,
                    side: 2100,
                    source_side: 2100,
                    offset: 0,
                    pixels: 112,
                    support: WORLD05_LAVA_SUPPORT,
                },
                WORLD05_ID,
                WORLD05_SOURCE,
                "world05_lava",
                3,
                source_local_transform([2100, 2100]),
            )
        } else if world09 {
            (
                Self {
                    world: 9,
                    terrain: 1,
                    side: 2100,
                    source_side: 2100,
                    offset: 0,
                    pixels: 426,
                    support: WORLD09_LAVA_SUPPORT,
                },
                WORLD09_ID,
                WORLD09_SOURCE,
                "world09_lava",
                3,
                source_local_transform([2100, 2100]),
            )
        } else if world11 {
            (
                Self {
                    world: 11,
                    terrain: 0,
                    side: 2100,
                    source_side: 2100,
                    offset: 0,
                    pixels: 577,
                    support: WORLD11_LAVA_SUPPORT,
                },
                WORLD11_ID,
                WORLD11_SOURCE,
                "world11_lava",
                3,
                source_local_transform([2100, 2100]),
            )
        } else {
            return Ok(None);
        };
        ensure!(
            r.id == id
                && r.provenance.source_sha256 == source
                && r.schema == schema
                && r.width == scope.side
                && r.height == scope.side
                && r.content.objects == OBJECTS
                && r.content.grounds == GROUNDS
                && policy["source_sha256"] == source
                && policy["selected_client"] == "123.5.1"
                && policy["transform"] == transform
                && policy[key] == scope.choice(),
            "unapproved World{} lava source/content/transform/cell support",
            scope.world
        );
        Ok(Some(scope))
    }
    fn choice(&self) -> serde_json::Value {
        let mut choice = lava_choice(self.pixels, self.support);
        choice["cell"]["terrain"] = self.terrain.into();
        choice
    }
    fn validate_content(&self, content: &Content) -> Result<()> {
        ensure!(
            matches!(content.hazards.get(&244), Some(h) if h.name == "Lava Blend" && h.min == 30 && h.max == 60)
                && content.grounds.get(&244) == Some(&0.666)
                && content.pushes.get(&244) == Some(&(0.0, 0.0)),
            "selected World{} Lava Blend definition drift: hazard={:?}, speed={:?}, push={:?}",
            self.world,
            content.hazards.get(&244).map(|h| (&h.name, h.min, h.max)),
            content.grounds.get(&244),
            content.pushes.get(&244)
        );
        Ok(())
    }
    pub(super) fn validate_loaded(&self, map: &Map, content: &Content) -> Result<()> {
        self.validate_content(content)?;
        self.validate_support(map)
    }
    fn accepts(&self, c: &Cell) -> bool {
        c.ground == 244
            && c.object.is_none()
            && c.config.is_empty()
            && c.terrain == self.terrain
            && c.region == 0
    }
    fn validate_support(&self, map: &Map) -> Result<()> {
        ensure!(
            map.width == self.side as i32
                && map.height == self.side as i32
                && map.cells.len() == self.side * self.side,
            "invalid World{} lava map dimensions",
            self.world
        );
        let mut support = Sha256::new();
        let mut count = 0;
        for (i, c) in map
            .cells
            .iter()
            .enumerate()
            .filter(|(_, c)| c.ground == 244)
        {
            ensure!(
                self.accepts(c),
                "unapproved World{} lava cell at {},{}",
                self.world,
                i % self.side,
                i / self.side
            );
            support.update(
                (((i / self.side + self.offset) * self.source_side + i % self.side + self.offset)
                    as u32)
                    .to_le_bytes(),
            );
            count += 1;
        }
        let actual = format!("{:x}", support.finalize());
        ensure!(
            count == self.pixels && actual == self.support,
            "World{} lava support differs from approved source positions: expected {}/{}, actual {}/{}",
            self.world,
            self.pixels,
            self.support,
            count,
            actual
        );
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "profile")]
pub(super) enum Recipe {
    #[serde(rename = "classic-v1")]
    ClassicV1(ClassicRecipe),
    #[serde(rename = "reviewed-cells-v1")]
    ReviewedCellsV1(ReviewedRecipe),
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ClassicRecipe {
    schema: u8,
    id: String,
    width: usize,
    height: usize,
    orientation: Orientation,
    scenery_seed: u64,
    content: ContentIdentity,
    layers: Layers,
    output: String,
    provenance: Provenance,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReviewedRecipe {
    schema: u8,
    id: String,
    width: usize,
    height: usize,
    orientation: RowMajor,
    content: ContentIdentity,
    inputs: ReviewedInputs,
    output: String,
    provenance: ImageProvenance,
}
#[derive(Debug, Deserialize, Serialize)]
enum RowMajor {
    #[serde(rename = "row-major")]
    RowMajor,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReviewedInputs {
    dictionary: Layer,
    indices: Indices,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Indices {
    path: String,
    sha256: String,
    expanded_sha256: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ImageProvenance {
    kind: ImageKind,
    source_sha256: String,
    policy: Layer,
    annotations: Layer,
    status: String,
}
#[derive(Debug, Deserialize, Serialize)]
enum ImageKind {
    #[serde(rename = "image-guided-reconstruction-v1")]
    ImageGuidedReconstructionV1,
}
#[derive(Debug, Deserialize, Serialize)]
enum Orientation {
    #[serde(rename = "x-major")]
    XMajor,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ContentIdentity {
    objects: String,
    grounds: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Layers {
    elevation: Layer,
    moisture: Layer,
    overrides: Layer,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Layer {
    path: String,
    sha256: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Provenance {
    source_url: String,
    source_sha256: String,
    exporter_url: String,
    exporter_sha256: String,
    emulator: String,
    mode: String,
    seed_variant: String,
    status: String,
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(super) fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn safe_path(root: &Path, relative: &str) -> Result<PathBuf> {
    ensure!(!relative.is_empty(), "empty recipe path");
    let mut path = root.to_path_buf();
    for part in Path::new(relative).components() {
        let Component::Normal(name) = part else {
            bail!("unsafe recipe path {relative}");
        };
        path.push(name);
        match path.symlink_metadata() {
            Ok(meta) => ensure!(
                !meta.file_type().is_symlink(),
                "symlink recipe path {relative}"
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e).with_context(|| format!("inspect {}", path.display())),
        }
    }
    Ok(path)
}
pub(super) fn bounded_read(path: &Path, max: usize) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    ensure!(
        file.metadata()?.is_file(),
        "not a regular file: {}",
        path.display()
    );
    let mut bytes = Vec::new();
    file.take(max as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= max,
        "file exceeds {max} bytes: {}",
        path.display()
    );
    Ok(bytes)
}
pub(super) fn valid_id(id: &str) -> bool {
    id.split('-').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}
impl Recipe {
    pub(super) fn lava_scope(&self, root: &Path) -> Result<Option<ScopedLava>> {
        match self {
            Self::ReviewedCellsV1(r) if r.schema == 2 => Ok(None),
            Self::ReviewedCellsV1(r) => {
                let policy = r.audit_inputs(root)?;
                ScopedLava::binding(r, &serde_json::from_slice(&policy)?)
            }
            Self::ClassicV1(r) => {
                ensure!(
                    !matches!(r.id.as_str(), WORLD09_ID | WORLD11_ID),
                    "{} requires its reviewed source recipe",
                    r.id
                );
                Ok(None)
            }
        }
    }
    pub(super) fn id(&self) -> &str {
        match self {
            Self::ClassicV1(r) => &r.id,
            Self::ReviewedCellsV1(r) => &r.id,
        }
    }
    pub(super) fn output(&self) -> &str {
        match self {
            Self::ClassicV1(r) => &r.output,
            Self::ReviewedCellsV1(r) => &r.output,
        }
    }
    pub(super) fn profile(&self) -> &'static str {
        match self {
            Self::ClassicV1(_) => "classic-v1",
            Self::ReviewedCellsV1(_) => "reviewed-cells-v1",
        }
    }
    pub(super) fn content_identity(&self) -> Option<[String; 2]> {
        match self {
            Self::ReviewedCellsV1(r) if matches!(r.schema, 3 | 4) => {
                Some([r.content.objects.clone(), r.content.grounds.clone()])
            }
            _ => None,
        }
    }
    pub(super) fn dimensions(&self) -> [usize; 2] {
        match self {
            Self::ClassicV1(r) => [r.width, r.height],
            Self::ReviewedCellsV1(r) => [r.width, r.height],
        }
    }
    pub(super) fn contract(&self) -> String {
        let (schema, source) = match self {
            Self::ClassicV1(r) => (r.schema, &r.provenance.source_sha256),
            Self::ReviewedCellsV1(r) => (r.schema, &r.provenance.source_sha256),
        };
        format!(
            "{} {} schema {schema} source {source}",
            self.id(),
            self.profile()
        )
    }
    fn status(&self) -> &str {
        match self {
            Self::ClassicV1(r) => &r.provenance.status,
            Self::ReviewedCellsV1(r) => &r.provenance.status,
        }
    }
    fn validate(&self) -> Result<()> {
        match self {
            Self::ClassicV1(r) => r.validate(),
            Self::ReviewedCellsV1(r) => {
                r.dimensions()?;
                ensure!(valid_id(&r.id), "invalid Realm ID {}", r.id);
                ensure!(
                    r.content.objects == OBJECTS && r.content.grounds == GROUNDS,
                    "unsupported content identity"
                );
                ensure!(
                    r.output == format!("{}.wmap", r.id),
                    "output must match Realm ID"
                );
                for hash in [
                    &r.inputs.dictionary.sha256,
                    &r.inputs.indices.sha256,
                    &r.inputs.indices.expanded_sha256,
                    &r.provenance.source_sha256,
                    &r.provenance.policy.sha256,
                    &r.provenance.annotations.sha256,
                ] {
                    ensure!(valid_hash(hash), "invalid reviewed source hash");
                }
                ensure!(
                    r.provenance
                        .status
                        .starts_with("Approximate image-guided reconstruction"),
                    "missing approximate reconstruction provenance"
                );
                Ok(())
            }
        }
    }
    fn inputs(&self) -> Vec<&str> {
        match self {
            Self::ClassicV1(r) => vec![
                &r.layers.elevation.path,
                &r.layers.moisture.path,
                &r.layers.overrides.path,
            ],
            Self::ReviewedCellsV1(r) => vec![
                &r.inputs.dictionary.path,
                &r.inputs.indices.path,
                &r.provenance.policy.path,
                &r.provenance.annotations.path,
            ],
        }
    }
}
impl ClassicRecipe {
    fn validate(&self) -> Result<()> {
        ensure!(
            !SOURCE_LOCAL_IDENTITIES
                .iter()
                .any(|(id, hash, _)| *id == self.id || *hash == self.provenance.source_sha256),
            "reserved source-local identity {} requires reviewed schema 3",
            self.id
        );
        ensure!(
            self.schema == 1,
            "unsupported recipe schema {}",
            self.schema
        );
        ensure!(
            valid_id(&self.id),
            "invalid Realm ID {}: expected lowercase ASCII alphanumeric segments separated by hyphens",
            self.id
        );
        ensure!(
            self.width == SIDE && self.height == SIDE,
            "expected 2048-square layers"
        );
        ensure!(
            self.content.objects == OBJECTS && self.content.grounds == GROUNDS,
            "unsupported content identity"
        );
        ensure!(
            self.output == format!("{}.wmap", self.id),
            "output must match Realm ID"
        );
        let p = &self.provenance;
        ensure!(
            valid_hash(&p.source_sha256) && valid_hash(&p.exporter_sha256),
            "invalid provenance hash"
        );
        ensure!(
            p.source_url.starts_with("https://")
                && p.exporter_url.starts_with("https://")
                && !p.emulator.is_empty()
                && p.mode == "Perlin"
                && !p.seed_variant.is_empty()
                && !p.status.is_empty(),
            "incomplete export provenance"
        );
        Ok(())
    }
}
fn recipes(root: &Path) -> Result<BTreeMap<String, Recipe>> {
    let mut paths = std::fs::read_dir(root)
        .with_context(|| format!("discover recipes in {}", root.display()))?
        .map(|entry| Ok(entry?.path()))
        .collect::<Result<Vec<_>>>()?;
    paths.sort();
    let mut found = BTreeMap::new();
    for path in paths
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
    {
        let name = path
            .file_name()
            .context("recipe filename missing")?
            .to_str()
            .context("non-UTF8 recipe path")?;
        let recipe = read_recipe(root, name)?;
        ensure!(
            found.insert(recipe.id().to_owned(), recipe).is_none(),
            "duplicate Realm recipe ID"
        );
    }
    Ok(found)
}
pub(super) fn read_recipe(root: &Path, relative: &str) -> Result<Recipe> {
    let path = safe_path(root, relative)?;
    let recipe: Recipe = serde_json::from_slice(&bounded_read(&path, 64 * 1024)?)
        .with_context(|| format!("parse recipe {}", path.display()))?;
    recipe
        .validate()
        .with_context(|| format!("validate recipe {}: {}", path.display(), recipe.contract()))?;
    let mut paths = BTreeSet::new();
    paths.insert(path.clone());
    for relative in std::iter::once(recipe.output()).chain(recipe.inputs()) {
        let input =
            safe_path(root, relative).with_context(|| format!("recipe {}", path.display()))?;
        ensure!(
            paths.insert(input),
            "aliased recipe/input/output path {relative}"
        );
    }
    if let Recipe::ReviewedCellsV1(r) = &recipe
        && matches!(r.schema, 3 | 4)
    {
        if r.strict_source() {
            strict_source_paths(root, &[relative])?;
        }
        r.audit_inputs(root).with_context(|| recipe.contract())?;
    }
    Ok(recipe)
}
fn expand_layer(bytes: &[u8], count: usize, expected: &str) -> Result<Vec<u8>> {
    ensure!(
        count <= CELLS && valid_hash(expected),
        "invalid layer size/hash"
    );
    inflate(bytes, count, CELLS + 4096, expected)
}
fn inflate(bytes: &[u8], count: usize, compressed_bound: usize, expected: &str) -> Result<Vec<u8>> {
    ensure!(
        bytes.len() <= compressed_bound,
        "compressed layer exceeds bound"
    );
    let mut decoder = flate2::Decompress::new(true);
    let mut raw = Vec::with_capacity(count + 1);
    let status = decoder
        .decompress_vec(bytes, &mut raw, flate2::FlushDecompress::Finish)
        .context("inflate layer")?;
    ensure!(
        status == flate2::Status::StreamEnd,
        "incomplete or oversized layer stream"
    );
    ensure!(
        raw.len() == count,
        "layer length: expected {count}, got {}",
        raw.len()
    );
    ensure!(
        decoder.total_in() as usize == bytes.len(),
        "trailing compressed layer data"
    );
    let actual = sha256(&raw);
    ensure!(
        actual == expected,
        "layer checksum: expected {expected}, got {actual}"
    );
    Ok(raw)
}
fn load_layer(root: &Path, layer: &Layer) -> Result<Vec<u8>> {
    let path = safe_path(root, &layer.path)?;
    let bytes = bounded_read(&path, CELLS + 4096)?;
    expand_layer(&bytes, CELLS, &layer.sha256).with_context(|| format!("layer {}", path.display()))
}
fn transpose(raw: &[u8], width: usize, height: usize) -> Result<Vec<u8>> {
    ensure!(
        width > 0 && height > 0 && width <= SIDE && height <= SIDE && raw.len() == width * height,
        "invalid x-major dimensions/length"
    );
    Ok((0..height)
        .flat_map(|y| (0..width).map(move |x| raw[x * height + y]))
        .collect())
}

fn habitat(elevation: u8, moisture: u8) -> (u8, u8) {
    let wet = moisture as u16 * 7;
    if elevation >= 204 {
        (if wet > 5 * 255 { 0x6a } else { 0x60 }, 1)
    } else if elevation > 153 {
        if wet > 5 * 255 {
            (0x47, 4)
        } else if wet > 3 * 255 {
            (0x23, 3)
        } else {
            (0xbd, 2)
        }
    } else if elevation > 51 {
        if wet > 5 * 255 {
            (0x56, 7)
        } else if wet > 3 * 255 {
            (0x46, 6)
        } else {
            (0xbd, 5)
        }
    } else if wet > 5 * 255 {
        (0x57, 10)
    } else if wet > 4 * 255 {
        (0x56, 10)
    } else if wet > 3 * 255 {
        (0x48, 9)
    } else {
        (0xbd, 8)
    }
}
fn cell(elevation: u8, moisture: u8, override_byte: u8) -> Result<Cell> {
    ensure!(
        matches!(override_byte & 15, 0 | 1 | 3),
        "unknown polygon flags {override_byte:#x}"
    );
    let (ground, terrain) = match override_byte >> 4 {
        0 => habitat(elevation, moisture),
        // The generic demo's lava and ice are not authority for new gameplay hazards.
        1 | 2 => (0x73, 0),
        3 => (0x6a, 1),
        4 | 6 => (0xbc, 0),
        5 => (0xbc, 0),
        7 => (0x72, 0),
        8 => (0xbe, 11),
        9..=12 => (0xd0, 0),
        _ => bail!("unknown override {override_byte:#x}"),
    };
    Ok(Cell {
        ground,
        object: None,
        config: String::new(),
        terrain,
        region: 0,
    })
}
#[derive(Debug, Serialize)]
struct Mainland {
    removed_components: usize,
    removed_cells: usize,
}
fn neighbors(i: usize, width: usize) -> [usize; 4] {
    let x = i % width;
    [
        i.wrapping_sub(width),
        i + width,
        if x > 0 { i - 1 } else { usize::MAX },
        if x + 1 < width { i + 1 } else { usize::MAX },
    ]
}
fn normalize_mainland(cells: &mut [Cell], overrides: &[u8], width: usize) -> Result<Mainland> {
    let mut labels = vec![0usize; cells.len()];
    let mut counts = Vec::new();
    let mut queue = VecDeque::new();
    // Original ocean codes distinguish lakes from ocean even when their final grounds are identical.
    for start in 0..cells.len() {
        if labels[start] != 0 || overrides[start] >> 4 == 5 {
            continue;
        }
        let label = counts.len() + 1;
        let mut count = 0;
        labels[start] = label;
        queue.push_back(start);
        while let Some(i) = queue.pop_front() {
            count += 1;
            for next in neighbors(i, width) {
                if next < cells.len() && labels[next] == 0 && overrides[next] >> 4 != 5 {
                    labels[next] = label;
                    queue.push_back(next);
                }
            }
        }
        counts.push(count);
    }
    let largest = *counts.iter().max().context("no non-ocean mainland")?;
    ensure!(
        counts.iter().filter(|&&n| n == largest).count() == 1,
        "ambiguous largest mainland"
    );
    let mainland = counts
        .iter()
        .position(|&n| n == largest)
        .context("mainland label missing")?
        + 1;
    let mut removed_cells = 0;
    for (i, c) in cells.iter_mut().enumerate() {
        if labels[i] != 0 && labels[i] != mainland {
            *c = cell(0, 0, 0x50)?;
            removed_cells += 1;
        }
    }
    Ok(Mainland {
        removed_components: counts.len() - 1,
        removed_cells,
    })
}
fn assemble(
    e: &[u8],
    m: &[u8],
    o: &[u8],
    width: usize,
    height: usize,
    seed: u64,
) -> Result<(Map, Mainland)> {
    let e = transpose(e, width, height)?;
    let m = transpose(m, width, height)?;
    let o = transpose(o, width, height)?;
    let mut cells = (0..e.len())
        .map(|i| {
            cell(e[i], m[i], o[i]).with_context(|| format!("cell {},{}", i % width, i / width))
        })
        .collect::<Result<Vec<_>>>()?;
    let mainland = normalize_mainland(&mut cells, &o, width)?;
    let mut coast_distance = vec![u8::MAX; cells.len()];
    let mut queue = VecDeque::new();
    for (i, _) in cells.iter().enumerate().filter(|(_, c)| c.terrain == 11) {
        coast_distance[i] = 0;
        queue.push_back(i);
    }
    while let Some(i) = queue.pop_front() {
        let distance = coast_distance[i] + 1;
        if distance > 8 {
            continue;
        }
        for next in neighbors(i, width) {
            if next < cells.len() && coast_distance[next] > distance {
                coast_distance[next] = distance;
                queue.push_back(next);
            }
        }
    }
    for (i, c) in cells.iter_mut().enumerate() {
        if coast_distance[i] <= 8 && matches!(c.terrain, 9 | 10) && o[i] >> 4 == 0 {
            c.terrain = 12;
        }
        super::decorate(c, super::mix(seed ^ 0xbb67ae8584caa73b ^ i as u64))?;
    }
    Ok((
        Map {
            width: width as i32,
            height: height as i32,
            cells,
            spawns: vec![],
        },
        mainland,
    ))
}

type Entry = (u8, String, String, u8, u8);
fn entry(
    c: &Cell,
    content: &Content,
    validated_objects: &mut BTreeSet<u16>,
    lava: Option<&ScopedLava>,
) -> Result<Entry> {
    ensure!(
        content.grounds.contains_key(&c.ground)
            && (!content.hazards.contains_key(&c.ground) || lava.is_some_and(|l| l.accepts(c))),
        "unknown/hazardous output ground {:#x}",
        c.ground
    );
    ensure!(
        c.terrain <= 12 && c.region == 0,
        "unsupported output terrain/region"
    );
    let name = match c.object {
        Some(kind) => {
            let object = content.get(kind)?;
            ensure!(
                object.static_object && (!object.enemy || object.passive_destructible()),
                "non-scenery output object {kind:#x}"
            );
            if validated_objects.insert(kind) {
                ensure!(
                    content
                        .objects
                        .values()
                        .filter(|o| o.name == object.name)
                        .count()
                        == 1,
                    "ambiguous output object {}",
                    object.name
                );
            }
            ensure!(!object.name.is_empty(), "empty output object name");
            if !(c.config.is_empty()
                || kind == 0x1dd
                    && matches!(
                        c.config.as_str(),
                        "conn:16843010"
                            | "conn:16843265"
                            | "conn:16908545"
                            | "conn:16908802"
                            | "conn:33620225"
                            | "conn:33620482"
                            | "conn:33685762"
                            | "conn:33686017"
                            | "conn:33686018"
                    ))
            {
                let size: u16 = c
                    .config
                    .strip_prefix("size:")
                    .context("unsupported scenery configuration")?
                    .parse()?;
                ensure!(
                    ((matches!(kind, 0x100..=0x104 | 0x185..=0x18d | 0x197..=0x19f)
                        && (130..=160).contains(&size)
                        && size.is_multiple_of(5))
                        || (kind == 0x18e && matches!(size, 140 | 160))
                        || (kind == 0x194 && matches!(size, 120 | 140)))
                        && c.config == format!("size:{size}"),
                    "unsupported scenery size/configuration"
                );
            }
            object.name.clone()
        }
        None => {
            ensure!(c.config.is_empty(), "configuration without scenery");
            String::new()
        }
    };

    ensure!(
        name.len() <= 1024 && c.config.len() <= 1024,
        "output dictionary string exceeds limit"
    );
    Ok((c.ground, name, c.config.clone(), c.terrain, c.region))
}
fn encode(map: &Map, content: &Content) -> Result<(Vec<u8>, usize)> {
    encode_scoped(map, content, None)
}
fn encode_scoped(
    map: &Map,
    content: &Content,
    lava: Option<&ScopedLava>,
) -> Result<(Vec<u8>, usize)> {
    encode_reviewed(map, content, lava, None)
}
fn encode_reviewed(
    map: &Map,
    content: &Content,
    lava: Option<&ScopedLava>,
    recipe: Option<&ReviewedRecipe>,
) -> Result<(Vec<u8>, usize)> {
    let shape_matches = match recipe {
        Some(recipe) => [map.width as usize, map.height as usize] == recipe.dimensions()?,
        None => map.width <= SIDE as i32 && map.height <= SIDE as i32,
    };
    ensure!(
        map.width > 0
            && map.height > 0
            && shape_matches
            && map.cells.len() == map.width as usize * map.height as usize,
        "invalid output dimensions/cells"
    );
    if let Some(lava) = lava {
        lava.validate_support(map)?;
    }
    let mut entries = BTreeSet::new();
    let mut validated_objects = BTreeSet::new();
    for c in &map.cells {
        entries.insert(entry(c, content, &mut validated_objects, lava)?);
    }
    ensure!(
        !entries.is_empty() && entries.len() <= i16::MAX as usize,
        "output dictionary exceeds format limit"
    );
    let dict: BTreeMap<Entry, i16> = entries
        .into_iter()
        .enumerate()
        .map(|(i, e)| (e, i as i16))
        .collect();
    let mut raw = Vec::new();
    raw.extend((dict.len() as i16).to_le_bytes());
    for (ground, name, config, terrain, region) in dict.keys() {
        raw.extend((*ground as i16).to_le_bytes());
        for text in [name, config] {
            let mut length = text.len();
            while length >= 128 {
                raw.push((length as u8 & 127) | 128);
                length >>= 7;
            }
            raw.push(length as u8);
            raw.extend(text.as_bytes());
        }
        raw.extend([*terrain, *region]);
    }
    raw.extend(map.width.to_le_bytes());
    raw.extend(map.height.to_le_bytes());
    for c in &map.cells {
        let name = match c.object {
            Some(kind) => content.get(kind)?.name.clone(),
            None => String::new(),
        };
        raw.extend(dict[&(c.ground, name, c.config.clone(), c.terrain, c.region)].to_le_bytes());
    }
    ensure!(
        raw.len() <= 16 * 1024 * 1024,
        "output exceeds expanded WMAP limit"
    );
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(&raw)?;
    Ok((encoder.finish()?, dict.len()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewedCell {
    ground: u8,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    object: Option<u16>,
    config: String,
    terrain: u8,
    region: u8,
}
fn dictionary(bytes: &[u8], content: &Content) -> Result<Vec<Cell>> {
    dictionary_scoped(bytes, content, None)
}
fn dictionary_scoped(
    bytes: &[u8],
    content: &Content,
    lava: Option<&ScopedLava>,
) -> Result<Vec<Cell>> {
    ensure!(
        bytes.len() <= 64 * 1024,
        "reviewed dictionary exceeds bound"
    );
    let entries: Vec<ReviewedCell> =
        serde_json::from_slice(bytes).context("parse reviewed dictionary")?;
    ensure!(
        !entries.is_empty() && entries.len() <= i16::MAX as usize,
        "invalid reviewed dictionary count"
    );
    let cells: Vec<_> = entries
        .into_iter()
        .map(|c| Cell {
            ground: c.ground,
            object: c.object,
            config: c.config,
            terrain: c.terrain,
            region: c.region,
        })
        .collect();
    // Unused entries must pass the same content/configuration guards as output cells.
    let mut validated_objects = BTreeSet::new();
    for c in &cells {
        entry(c, content, &mut validated_objects, lava).context("validate reviewed dictionary")?;
    }
    Ok(cells)
}
fn lava_choice(pixels: usize, support: &str) -> serde_json::Value {
    serde_json::json!({"rgb":"d44a00","pixels":pixels,"support_sha256_le_u32":support,
        "cell":{"ground":244,"object":null,"config":"","terrain":1,"region":0}})
}
fn strict_source_paths(root: &Path, paths: &[&str]) -> Result<()> {
    let normalized: PathBuf = root.components().collect();
    ensure!(
        !root.as_os_str().is_empty()
            && normalized.as_os_str() == root.as_os_str()
            && root
                .components()
                .all(|c| matches!(c, Component::RootDir | Component::Normal(_))),
        "noncanonical strict-source root {}",
        root.display()
    );
    let absolute = std::path::absolute(root)?;
    for ancestor in absolute.ancestors() {
        ensure!(
            !ancestor.is_symlink(),
            "strict-source symlink ancestor {}",
            ancestor.display()
        );
    }
    for &relative in paths {
        let path = Path::new(relative);
        let normalized: PathBuf = path.components().collect();
        ensure!(
            normalized.to_str() == Some(relative)
                && path.components().all(|c| matches!(c, Component::Normal(_)))
                && !relative.is_empty(),
            "noncanonical strict-source path {relative}"
        );
        safe_path(&absolute, relative)?;
    }
    Ok(())
}
fn strict_keys(value: &serde_json::Value, expected: &[&str], label: &str) -> Result<()> {
    let actual = value
        .as_object()
        .with_context(|| format!("strict-source {label} must be an object"))?;
    ensure!(
        actual.len() == expected.len() && expected.iter().all(|k| actual.contains_key(*k)),
        "strict-source {label} keys: expected {expected:?}, actual {:?}",
        actual.keys().collect::<Vec<_>>()
    );
    Ok(())
}
struct UniqueJson;
impl<'de> serde::Deserialize<'de> for UniqueJson {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        d.deserialize_any(Self)
    }
}
impl<'de> serde::de::Visitor<'de> for UniqueJson {
    type Value = Self;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("JSON with unique object keys")
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(
        self,
        mut map: A,
    ) -> std::result::Result<Self, A::Error> {
        let mut keys = std::collections::BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(serde::de::Error::custom(format!(
                    "duplicate JSON key {key}"
                )));
            }
            map.next_value::<Self>()?;
        }
        Ok(Self)
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(
        self,
        mut seq: A,
    ) -> std::result::Result<Self, A::Error> {
        while seq.next_element::<Self>()?.is_some() {}
        Ok(Self)
    }
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> std::result::Result<Self, E> {
        Ok(Self)
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> std::result::Result<Self, E> {
        Ok(Self)
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> std::result::Result<Self, E> {
        Ok(Self)
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> std::result::Result<Self, E> {
        Err(E::custom("floating-point source metadata"))
    }
    fn visit_str<E: serde::de::Error>(self, _: &str) -> std::result::Result<Self, E> {
        Ok(Self)
    }
    fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Self, E> {
        Ok(Self)
    }
}
fn strict_source_audit(policy: &[u8], annotations: &[u8], recipe: &ReviewedRecipe) -> Result<()> {
    ensure!(
        recipe.schema == 4
            || (recipe.schema == 3
                && matches!(recipe.id.as_str(), WORLD05_ID | WORLD09_ID | WORLD11_ID)),
        "strict source audit requires an admitted source identity"
    );
    let _: UniqueJson = serde_json::from_slice(policy).context("strict-source policy keys")?;
    let _: UniqueJson =
        serde_json::from_slice(annotations).context("strict-source annotations keys")?;
    let policy: serde_json::Value = serde_json::from_slice(policy)?;
    let annotations: serde_json::Value = serde_json::from_slice(annotations)?;
    let mut keys = vec![
        "schema",
        "selected_client",
        "source_sha256",
        "transform",
        "audit",
    ];
    if recipe.id == "classic-world-01-reconstruction" {
        keys.push("world01_lava");
        ensure!(
            policy["world01_lava"] == lava_choice(703, WORLD01_LAVA_SUPPORT),
            "schema 4 World1 requires its exact703-cell lava choice"
        );
    }
    for (id, key) in [(WORLD02_ID, "world02_lava"), (WORLD03_ID, "world03_lava")] {
        if recipe.id == id {
            keys.push(key);
            ScopedLava::binding(recipe, &policy)?.context("authored fissure lava scope missing")?;
        }
    }
    if recipe.id == WORLD05_ID && policy.get("world05_lava").is_some() {
        keys.push("world05_lava");
        let mut choice = lava_choice(112, WORLD05_LAVA_SUPPORT);
        choice["cell"]["terrain"] = 0.into();
        ensure!(
            policy["world05_lava"] == choice,
            "World5 requires its exact112-cell terrain0 lava capability"
        );
    }
    if recipe.id == WORLD09_ID && policy.get("world09_lava").is_some() {
        keys.push("world09_lava");
        ensure!(
            policy["world09_lava"] == lava_choice(426, WORLD09_LAVA_SUPPORT),
            "World9 requires its exact426-cell lava capability"
        );
    }
    if recipe.id == WORLD11_ID && policy.get("world11_lava").is_some() {
        keys.push("world11_lava");
        let mut choice = lava_choice(577, WORLD11_LAVA_SUPPORT);
        choice["cell"]["terrain"] = 0.into();
        ensure!(
            policy["world11_lava"] == choice,
            "World11 requires its exact577-cell terrain0 lava capability"
        );
    }
    strict_keys(&policy, &keys, "policy")?;
    strict_keys(
        &policy["audit"],
        &["kind", "supports", "pixels"],
        "support audit",
    )?;
    strict_keys(
        &policy["transform"],
        &[
            "kind",
            "source_dimensions",
            "target_dimensions",
            "mapping",
            "historical_coordinates",
            "status",
        ],
        "transform",
    )?;
    strict_keys(
        &annotations,
        &["source_sha256", "policy_sha256", "transform", "unknown"],
        "annotations",
    )?;
    ensure!(
        policy["audit"]["pixels"].as_u64() == Some((recipe.width * recipe.height) as u64),
        "strict-source support audit must cover every source cell"
    );
    for row in annotations["unknown"]
        .as_array()
        .context("strict-source support rows required")?
    {
        strict_keys(
            row,
            &[
                "support",
                "pixels",
                "support_sha256",
                "status",
                "reason",
                "ground",
                "object",
                "config",
                "terrain",
                "region",
            ],
            "support row",
        )?;
    }
    Ok(())
}
fn hashed_input(root: &Path, path: &str, expected: &str, bound: usize) -> Result<Vec<u8>> {
    let bytes = bounded_read(&safe_path(root, path)?, bound)?;
    let actual = sha256(&bytes);
    ensure!(
        actual == expected,
        "input {path} checksum: expected {expected}, got {actual}"
    );
    Ok(bytes)
}

fn audit_scoped(
    policy: &[u8],
    annotations: &[u8],
    provenance: &ImageProvenance,
    source_local: Option<[usize; 2]>,
) -> Result<()> {
    let cell_count = match source_local {
        Some([width, height]) => width * height,
        None => CELLS,
    };
    let policy: serde_json::Value =
        serde_json::from_slice(policy).context("parse source policy")?;
    let annotations: serde_json::Value =
        serde_json::from_slice(annotations).context("parse source annotations")?;
    if let Some(dimensions) = source_local {
        ensure!(
            policy["schema"] == 2
                && policy["transform"] == source_local_transform(dimensions)
                && annotations["transform"] == source_local_transform(dimensions),
            "source-local-v1 requires exact policy/annotations transform and support audit schema 2"
        );
    }
    let (support_key, expected_count, expected_pixels) = match policy["schema"].as_u64() {
        Some(1) => ("component", 101, 20571),
        Some(2) => {
            ensure!(
                policy["source_sha256"] == provenance.source_sha256
                    && policy["audit"]["kind"] == "reviewed-supports-v1",
                "inconsistent reviewed support audit"
            );
            let count = policy["audit"]["supports"]
                .as_u64()
                .filter(|n| (1..=4096).contains(n))
                .context("invalid reviewed support count")?;
            let pixels = policy["audit"]["pixels"]
                .as_u64()
                .filter(|n| (1..=cell_count as u64).contains(n))
                .context("invalid reviewed support total")?;
            ("support", count, pixels)
        }
        _ => bail!("unsupported source audit schema"),
    };
    ensure!(
        annotations["policy_sha256"] == provenance.policy.sha256
            && annotations["source_sha256"] == provenance.source_sha256
            && policy["selected_client"] == "123.5.1"
            && policy["transform"].is_object()
            && annotations["transform"] == policy["transform"],
        "inconsistent source audit identity/transform"
    );
    let rows = annotations["unknown"]
        .as_array()
        .context("missing UNKNOWN decisions")?;
    ensure!(
        rows.len() as u64 == expected_count,
        "missing UNKNOWN decision"
    );
    let mut pixels = 0u64;
    for (i, row) in rows.iter().enumerate() {
        ensure!(
            row[support_key].as_u64() == Some(i as u64 + 1)
                && row["status"] == "REVIEWED_RECONSTRUCTION"
                && row["reason"].as_str().is_some_and(|s| !s.is_empty())
                && row["support_sha256"].as_str().is_some_and(valid_hash)
                && row["terrain"].as_u64().is_some_and(|t| t <= 12)
                && row["ground"].as_u64().is_some_and(|g| g <= 255),
            "unreviewed UNKNOWN component {}",
            i + 1
        );
        if source_local.is_some() {
            // Metadata typing is not proof of a source-position/RGB/alpha partition.
            let fields = ["ground", "object", "config", "terrain", "region"]
                .into_iter()
                .filter_map(|key| row.get(key).map(|value| (key.to_owned(), value.clone())))
                .collect::<serde_json::Map<_, _>>();
            let _: ReviewedCell = serde_json::from_value(serde_json::Value::Object(fields))
                .with_context(|| {
                    format!(
                        "source-local support {} requires all five typed fields",
                        i + 1
                    )
                })?;
        }
        pixels += row["pixels"]
            .as_u64()
            .filter(|p| *p > 0 && *p <= cell_count as u64)
            .context("invalid UNKNOWN support size")?;
    }
    ensure!(
        pixels == expected_pixels,
        "inconsistent UNKNOWN support total"
    );
    Ok(())
}
fn indexed_map(raw: &[u8], dictionary: &[Cell], dimensions: [usize; 2]) -> Result<Map> {
    let [width, height] = dimensions;
    let bytes = index_bytes(dimensions)?;
    ensure!(
        raw.len() == bytes,
        "invalid reviewed index length for {dimensions:?}: expected {bytes}, actual {}",
        raw.len()
    );
    let cells = raw
        .chunks_exact(2)
        .enumerate()
        .map(|(i, bytes)| {
            let index = u16::from_le_bytes([bytes[0], bytes[1]]) as usize;
            dictionary.get(index).cloned().with_context(|| {
                format!(
                    "UNKNOWN dictionary index {index} at {},{}",
                    i % width,
                    i / width
                )
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Map {
        width: width as i32,
        height: height as i32,
        cells,
        spawns: vec![],
    })
}
fn index_bytes([width, height]: [usize; 2]) -> Result<usize> {
    ensure!(
        width > 0
            && height > 0
            && ((width <= SIDE && height <= SIDE) || [width, height] == [2100, 2100]),
        "invalid reviewed dimensions {width}x{height}"
    );
    width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(2))
        .context("reviewed index byte count overflow")
}
fn reviewed_map(
    root: &Path,
    recipe: &ReviewedRecipe,
    content: &Content,
) -> Result<(Map, Option<ScopedLava>)> {
    let dimensions = recipe.dimensions()?;
    let bytes = index_bytes(dimensions)?;
    let inputs = &recipe.inputs;
    let cells = hashed_input(
        root,
        &inputs.dictionary.path,
        &inputs.dictionary.sha256,
        64 * 1024,
    )?;
    let policy = recipe.audit_inputs(root).with_context(|| {
        format!(
            "{} reviewed-cells-v1 schema {} source {}",
            recipe.id, recipe.schema, recipe.provenance.source_sha256
        )
    })?;
    let policy: serde_json::Value = serde_json::from_slice(&policy)?;
    let lava = ScopedLava::for_recipe(recipe, &policy, content)?;
    let dictionary = match lava.as_ref() {
        Some(lava) => dictionary_scoped(&cells, content, Some(lava))?,
        None => dictionary(&cells, content)?,
    };
    let indices = &inputs.indices;
    let compressed = hashed_input(root, &indices.path, &indices.sha256, bytes + 4096)?;
    let raw = inflate(&compressed, bytes, bytes + 4096, &indices.expanded_sha256)?;
    Ok((indexed_map(&raw, &dictionary, dimensions)?, lava))
}

pub(crate) fn run(args: &[String]) -> Result<bool> {
    if args.first().is_none_or(|a| a != "build-realm") {
        return Ok(false);
    }
    let mut data = None;
    let mut id = None;
    let mut check = false;
    let mut args = args[1..].iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data" if data.is_none() => {
                data = Some(PathBuf::from(args.next().context("--data requires DIR")?))
            }
            "--id" if id.is_none() => id = Some(args.next().context("--id requires ID")?.as_str()),
            "--check" if !check => check = true,
            _ => bail!("unknown/duplicate build-realm argument {arg}"),
        }
    }
    let data = data.context("build-realm requires --data DIR")?;
    let id = id.context("build-realm requires --id ID")?;
    let started = std::time::Instant::now();
    let root = safe_path(&data, "realms")?;
    let recipes = recipes(&root)?;
    let recipe = recipes
        .get(id)
        .with_context(|| format!("unknown Realm recipe ID {id}"))?;
    for (name, expected) in [("objects.xml", OBJECTS), ("grounds.xml", GROUNDS)] {
        let actual = sha256(&bounded_read(&safe_path(&data, name)?, 16 * 1024 * 1024)?);
        ensure!(
            actual == expected,
            "unsupported content {name}: expected {expected}, got {actual}"
        );
    }
    let content = Content::load(&data).context("load selected-client content")?;
    let (map, mainland, lava) = match recipe {
        Recipe::ClassicV1(r) => {
            let e = load_layer(&root, &r.layers.elevation)?;
            let m = load_layer(&root, &r.layers.moisture)?;
            let o = load_layer(&root, &r.layers.overrides)?;
            let (map, mainland) = assemble(&e, &m, &o, r.width, r.height, r.scenery_seed)?;
            (map, Some(mainland), None)
        }
        Recipe::ReviewedCellsV1(r) => {
            let (map, lava) =
                reviewed_map(&root, r, &content).with_context(|| recipe.contract())?;
            (map, None, lava)
        }
    };
    let prepared = started.elapsed();
    let (bytes, dictionary) = match recipe {
        Recipe::ReviewedCellsV1(r) => encode_reviewed(&map, &content, lava.as_ref(), Some(r))?,
        Recipe::ClassicV1(_) => encode(&map, &content)?,
    };
    let output = safe_path(&root, recipe.output())?;
    if check {
        ensure!(
            bounded_read(&output, 16 * 1024 * 1024)? == bytes,
            "rebuilt output differs: {}",
            output.display()
        );
    } else {
        std::fs::write(&output, &bytes).with_context(|| format!("write {}", output.display()))?;
    }
    let mut habitats = BTreeMap::<u8, usize>::new();
    for cell in &map.cells {
        *habitats.entry(cell.terrain).or_default() += 1;
    }
    println!(
        "{}",
        serde_json::json!({
            "id":id, "profile":recipe.profile(), "recipe_identity_sha256":sha256(&serde_json::to_vec(recipe)?),
            "status":if check {"checked"} else {"built"}, "output":output,
            "sha256":sha256(&bytes), "bytes":bytes.len(), "width":map.width, "height":map.height,
            "dictionary":dictionary, "habitats":habitats, "mainland":mainland,
            "scenery":map.cells.iter().filter(|c| c.object.is_some()).count(),
            "configured_scenery":map.cells.iter().filter(|c| !c.config.is_empty()).count(),
            "roads":map.cells.iter().filter(|c| c.ground == 0xd0).count(),
            "water":map.cells.iter().filter(|c| matches!(c.ground, 0xbc | 0x72 | 0x73)).count(),
            "prepare_ms":prepared.as_millis(), "total_ms":started.elapsed().as_millis(),
            "reconstruction":recipe.status(),
        })
    );
    Ok(true)
}

#[test]
fn authored_fissure_lava_rejects_missing_or_moved_cells() -> Result<()> {
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("data");
    let content = Content::load(&data)?;
    let root = data.join("realms");
    for name in ["world02.json", "world03.json"] {
        let Recipe::ReviewedCellsV1(recipe) = read_recipe(&root, name)? else {
            unreachable!()
        };
        let (mut map, scope) = reviewed_map(&root, &recipe, &content)?;
        let scope = scope.context("missing authored fissure lava")?;
        scope.validate_loaded(&map, &content)?;
        assert!(!map.cells.iter().any(|c| c.ground == 54));
        let lava = map.cells.iter().position(|c| c.ground == 244).unwrap();
        let rock = map
            .cells
            .iter()
            .position(|c| c.ground == 96 && c.object.is_none())
            .unwrap();
        map.cells[lava].ground = 96;
        assert!(scope.validate_loaded(&map, &content).is_err());
        map.cells[rock].ground = 244;
        assert!(scope.validate_loaded(&map, &content).is_err());
    }
    Ok(())
}
