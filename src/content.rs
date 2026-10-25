use anyhow::{Context, Result, bail, ensure};
use roxmltree::Node;
mod abilities;
pub use abilities::Ability;
mod consumables;
pub use consumables::{Companion, ConsumableEffect};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::Read, path::Path};

pub const WIZARD: u16 = 0x030e;
pub const PRIEST: u16 = 0x0310;

pub const SLOWED: i32 = 1 << 3;
pub const STUNNED: i32 = 1 << 6;
pub const PARALYZED: i32 = 1 << 13;
pub const SPEEDY: i32 = 1 << 14;
pub const STATS: [(u8, &str); 8] = [
    (0, "MaxHitPoints"),
    (3, "MaxMagicPoints"),
    (20, "Attack"),
    (21, "Defense"),
    (22, "Speed"),
    (28, "Dexterity"),
    (26, "HpRegen"),
    (27, "MpRegen"),
];
#[derive(Clone, Debug)]
pub struct Class {
    pub base: [i32; 8],
    pub caps: [i32; 8],
    pub growth: [(i32, i32); 8],
    pub requires: Vec<(u16, i32)>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Projectile {
    pub speed: f32,
    pub lifetime: f32,
    pub min: i32,
    pub max: i32,
    pub amplitude: f32,
    pub frequency: f32,
    pub passes_cover: bool,
    pub multi_hit: bool,
    pub armor_piercing: bool,
    pub wavy: bool,
    pub parametric: bool,
    pub boomerang: bool,
    pub magnitude: f32,
    pub effects: Vec<(u8, u64)>,
}
impl Projectile {
    pub fn offset(&self, id: u8, angle: f32, elapsed: f32) -> (f32, f32) {
        let parity = if id.is_multiple_of(2) {
            0.
        } else {
            std::f32::consts::PI
        };
        let mut distance = elapsed * self.speed / 10000.;
        if self.wavy {
            let direction = angle
                + std::f32::consts::PI / 64.
                    * (parity + 6. * std::f32::consts::PI * elapsed / 1000.).sin();
            return (distance * direction.cos(), distance * direction.sin());
        }
        if self.parametric {
            let phase = elapsed / self.lifetime * std::f32::consts::TAU;
            let a = phase.sin() * if id.is_multiple_of(2) { -1. } else { 1. };
            let b = (phase * 2.).sin() * if id % 4 < 2 { 1. } else { -1. };
            return (
                (a * angle.cos() - b * angle.sin()) * self.magnitude,
                (a * angle.sin() + b * angle.cos()) * self.magnitude,
            );
        }
        if self.boomerang && elapsed > self.lifetime / 2. {
            distance = (self.lifetime - elapsed) * self.speed / 10000.;
        }
        let wave = self.amplitude
            * (parity + std::f32::consts::TAU * self.frequency * elapsed / self.lifetime).sin();
        (
            distance * angle.cos() - wave * angle.sin(),
            distance * angle.sin() + wave * angle.cos(),
        )
    }
}
impl Object {
    pub fn is_potion(&self) -> bool {
        self.potion
            && self.consumable
            && (self.stat_increase.is_some()
                || self.consumable_effects.iter().any(|effect| {
                    matches!(
                        effect,
                        ConsumableEffect::Heal { .. }
                            | ConsumableEffect::Mana { .. }
                            | ConsumableEffect::Boost { .. }
                    )
                }))
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct SpawnGroup {
    pub mean: usize,
    pub deviation: usize,
    pub min: usize,
    pub max: usize,
}
impl SpawnGroup {
    fn parse(n: Node<'_, '_>) -> Result<Self> {
        let value = |tag| -> Result<usize> {
            Ok(child(n, tag)
                .and_then(|v| v.text())
                .context("incomplete spawn group")?
                .trim()
                .parse()?)
        };
        let group = Self {
            mean: value("Mean")?,
            deviation: value("StdDev")?,
            min: value("Min")?,
            max: value("Max")?,
        };
        ensure!(
            group.min > 0 && group.min <= group.mean && group.mean <= group.max && group.max <= 32,
            "invalid spawn group bounds"
        );
        Ok(group)
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Object {
    pub kind: u16,
    pub name: String,
    pub class: String,
    pub hp: i32,
    pub size: i32,
    pub mp: i32,
    pub attack: i32,
    pub defense: i32,
    pub speed: i32,
    pub dexterity: i32,
    pub vitality: i32,
    pub wisdom: i32,
    pub equipment: Vec<i32>,
    pub slots: Vec<i32>,
    pub slot_type: i32,
    pub fame_bonus: u32,
    pub tier: Option<u8>,
    pub soulbound: bool,
    pub bag_type: u8,
    pub put_normal: bool,
    pub put_soulbound: bool,
    pub bonuses: Vec<(u8, i32)>,
    pub unsupported_equipment: bool,
    pub projectiles: BTreeMap<u8, Projectile>,
    pub shots: i32,
    pub arc: f32,
    pub rate: f32,
    pub mana_cost: i32,
    pub activate: String,
    pub heal: i32,
    pub stat_increase: Option<(u8, i32)>,
    pub unlock_portal: Option<(String, String)>,
    pub create_object: Option<String>,
    pub dungeon_name: Option<String>,
    pub consumable: bool,
    pub potion: bool,
    pub resurrects: bool,
    pub textures: [Option<u32>; 2],
    pub successor: Option<String>,
    pub doses: Option<u8>,
    pub consumable_effects: Vec<ConsumableEffect>,
    pub xp_boost: bool,
    pub companion: Option<Companion>,
    pub heal_range: f32,
    pub activations: usize,
    pub abilities: Vec<Ability>,
    pub cooldown_ms: u64,
    pub enemy: bool,
    pub god: bool,
    pub cube: bool,
    pub oryx: bool,
    pub stasis_immune: bool,
    pub leach_health: bool,
    pub terrain: u8,
    pub spawn_weight: Option<f32>,
    pub spawn_group: Option<SpawnGroup>,
    pub per_realm_max: Option<usize>,
    pub quest: bool,
    pub quest_level: Option<i32>,
    pub quest_window: (u8, u8),
    pub xp_multiplier: f32,
    pub blocks: bool,
    pub full_occupy: bool,
    pub occupy_square: bool,
    pub enemy_occupy_square: bool,
    pub static_object: bool,
    pub protects_ground: bool,
    pub destructible: bool,
}
impl Object {
    pub fn passive_destructible(&self) -> bool {
        matches!(self.kind, 0x122 | 0x162 | 0x181f)
    }
}
#[derive(Clone)]
pub struct Hazard {
    pub name: String,
    pub min: i32,
    pub max: i32,
}
#[derive(Clone)]
pub struct Content {
    pub(crate) xml_sha256: [String; 2],
    pub objects: BTreeMap<u16, Object>,
    pub grounds: BTreeMap<u8, f32>,
    pub ground_names: BTreeMap<u8, String>,
    pub pushes: BTreeMap<u8, (f32, f32)>,
    pub hazards: BTreeMap<u8, Hazard>,
    pub classes: BTreeMap<u16, Class>,
}
fn child<'a, 'i>(n: Node<'a, 'i>, tag: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.has_tag_name(tag))
}
fn texture(n: Node<'_, '_>, tag: &str) -> Result<Option<u32>> {
    child(n, tag)
        .map(|_| {
            let value = text(n, tag);
            Ok(if let Some(hex) = value.strip_prefix("0x") {
                u32::from_str_radix(hex, 16)?
            } else {
                value.parse()?
            })
        })
        .transpose()
}
fn text(n: Node<'_, '_>, tag: &str) -> String {
    child(n, tag)
        .and_then(|c| c.text())
        .unwrap_or("")
        .trim()
        .to_owned()
}
// Omitted optional fields have the client's XML defaults, not substitute content.
fn number(n: Node<'_, '_>, tag: &str, default: f32) -> Result<f32> {
    match child(n, tag).and_then(|c| c.text()) {
        None => Ok(default),
        Some(v) => {
            let x: f32 = v.trim().parse().with_context(|| {
                format!("{} {tag}: {v}", n.attribute("id").unwrap_or("projectile"))
            })?;
            ensure!(x.is_finite(), "non-finite XML number");
            Ok(x)
        }
    }
}
fn list(n: Node<'_, '_>, tag: &str) -> Result<Vec<i32>> {
    let s = text(n, tag);
    if s.is_empty() {
        return Ok(vec![]);
    }
    s.split(',')
        .map(|v| {
            let v = v.trim();
            if let Some(h) = v.strip_prefix("0x") {
                Ok(i32::from_str_radix(h, 16)?)
            } else {
                Ok(v.parse()?)
            }
        })
        .collect()
}
fn kind(s: &str) -> Result<u16> {
    if let Some(h) = s.strip_prefix("0x") {
        Ok(u16::from_str_radix(h, 16)?)
    } else {
        Ok(s.parse()?)
    }
}
fn fame_bonus(n: Node<'_, '_>) -> Result<u32> {
    let mut fields = n.children().filter(|c| c.has_tag_name("FameBonus"));
    let Some(field) = fields.next() else {
        return Ok(0);
    };
    ensure!(
        fields.next().is_none(),
        "duplicate FameBonus on {:?}",
        n.attribute("id")
    );
    ensure!(
        !field.children().any(|c| c.is_element()),
        "nested FameBonus on {:?}",
        n.attribute("id")
    );
    field
        .text()
        .context("empty FameBonus")?
        .trim()
        .parse()
        .with_context(|| format!("invalid FameBonus on {:?}", n.attribute("id")))
}

impl Content {
    pub fn load(dir: &Path) -> Result<Self> {
        let data = std::fs::read_to_string(dir.join("objects.xml"))?;
        let objects_sha256 = format!("{:x}", Sha256::digest(data.as_bytes()));
        let doc = roxmltree::Document::parse(&data)?;
        let mut objects = BTreeMap::new();
        for n in doc.root_element().children().filter(|n| n.is_element()) {
            let id = kind(n.attribute("type").context("object has no type")?)?;
            let mut projectiles = BTreeMap::new();
            for p in n.children().filter(|p| p.has_tag_name("Projectile")) {
                let index = match p.attribute("id") {
                    Some(index) => index.parse::<u8>()?,
                    None => 0,
                };
                let fixed = number(p, "Damage", 0.)? as i32;
                let projectile = Projectile {
                    speed: number(p, "Speed", 100.)?,
                    lifetime: number(p, "LifetimeMS", 0.)?,
                    min: number(p, "MinDamage", fixed as f32)? as i32,
                    max: number(p, "MaxDamage", fixed as f32)? as i32,
                    amplitude: number(p, "Amplitude", 0.)?,
                    frequency: number(p, "Frequency", 1.)?,
                    passes_cover: child(p, "PassesCover").is_some(),
                    multi_hit: child(p, "MultiHit").is_some(),
                    armor_piercing: child(p, "ArmorPiercing").is_some(),
                    wavy: child(p, "Wavy").is_some(),
                    parametric: child(p, "Parametric").is_some(),
                    boomerang: child(p, "Boomerang").is_some(),
                    magnitude: number(p, "Magnitude", 3.)?,
                    effects: p
                        .children()
                        .filter(|e| e.has_tag_name("ConditionEffect"))
                        .map(|e| -> Result<_> {
                            let index = crate::condition::projectile_effect(
                                e.text().context("condition name missing")?,
                            )?;
                            let duration = std::time::Duration::try_from_secs_f32(
                                e.attribute("duration")
                                    .context("condition duration missing")?
                                    .parse()?,
                            )?
                            .as_millis()
                            .try_into()?;
                            ensure!(duration > 0, "non-positive projectile condition duration");
                            Ok((index, duration))
                        })
                        .collect::<Result<_>>()?,
                };
                ensure!(
                    projectiles.insert(index, projectile).is_none(),
                    "duplicate projectile {index} on object {id:#x}"
                );
            }
            let o = Object {
                kind: id,
                name: n.attribute("id").context("object has no name")?.into(),
                class: text(n, "Class"),
                hp: number(n, "MaxHitPoints", 0.)? as i32,
                size: number(n, "Size", 100.)? as i32,
                mp: number(n, "MaxMagicPoints", 0.)? as i32,
                attack: number(n, "Attack", 0.)? as i32,
                defense: number(n, "Defense", 0.)? as i32,
                speed: number(n, "Speed", 0.)? as i32,
                dexterity: number(n, "Dexterity", 0.)? as i32,
                vitality: number(n, "HpRegen", 0.)? as i32,
                wisdom: number(n, "MpRegen", 0.)? as i32,
                equipment: list(n, "Equipment")?,
                slots: list(n, "SlotTypes")?,
                slot_type: number(n, "SlotType", 0.)? as i32,
                fame_bonus: fame_bonus(n)?,
                tier: child(n, "Tier")
                    .map(|_| text(n, "Tier").parse::<u8>())
                    .transpose()?,
                soulbound: child(n, "Soulbound").is_some(),
                bag_type: number(n, "BagType", 0.)? as u8,
                put_normal: child(n, "CanPutNormalObjects").is_some(),
                put_soulbound: child(n, "CanPutSoulboundObjects").is_some(),
                bonuses: n
                    .children()
                    .filter(|x| {
                        x.has_tag_name("ActivateOnEquip") && x.text() == Some("IncrementStat")
                    })
                    .map(|x| -> Result<(u8, i32)> {
                        Ok((
                            x.attribute("stat")
                                .context("equipment stat missing")?
                                .parse()?,
                            x.attribute("amount")
                                .context("equipment amount missing")?
                                .parse()?,
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?,
                unsupported_equipment: n.children().any(|x| {
                    x.has_tag_name("ActivateOnEquip") && x.text() != Some("IncrementStat")
                }),
                projectiles,
                shots: number(n, "NumProjectiles", 1.)? as i32,
                arc: number(n, "ArcGap", 11.25)?,
                rate: number(n, "RateOfFire", 1.)?,
                mana_cost: number(n, "MpCost", 0.)? as i32,
                activate: text(n, "Activate"),
                heal: child(n, "Activate")
                    .and_then(|p| p.attribute("amount"))
                    .map(str::parse)
                    .transpose()?
                    .unwrap_or(0),
                stat_increase: n
                    .children()
                    .find(|p| p.has_tag_name("Activate") && p.text() == Some("IncrementStat"))
                    .map(|p| -> Result<(u8, i32)> {
                        Ok((
                            p.attribute("stat")
                                .context("potion stat missing")?
                                .parse()?,
                            p.attribute("amount")
                                .context("potion amount missing")?
                                .parse()?,
                        ))
                    })
                    .transpose()?,
                unlock_portal: n
                    .children()
                    .find(|p| p.has_tag_name("Activate") && p.text() == Some("UnlockPortal"))
                    .map(|p| -> Result<(String, String)> {
                        Ok((
                            p.attribute("dungeonName")
                                .context("unlock dungeonName missing")?
                                .to_owned(),
                            p.attribute("lockedName")
                                .context("unlock lockedName missing")?
                                .to_owned(),
                        ))
                    })
                    .transpose()?,
                create_object: n
                    .children()
                    .find(|p| p.has_tag_name("Activate") && p.text() == Some("Create"))
                    .map(|p| {
                        p.attribute("id")
                            .context("Create activation id missing")
                            .map(str::to_owned)
                    })
                    .transpose()?,
                dungeon_name: child(n, "DungeonName").map(|_| text(n, "DungeonName")),
                consumable: child(n, "Consumable").is_some(),
                potion: child(n, "Potion").is_some(),
                xp_boost: child(n, "XpBoost").is_some(),
                resurrects: child(n, "Resurrects").is_some(),
                textures: [texture(n, "Tex1")?, texture(n, "Tex2")?],
                successor: child(n, "SuccessorId").map(|_| text(n, "SuccessorId")),
                doses: if child(n, "Doses").is_some() {
                    let doses: u8 = text(n, "Doses").parse()?;
                    ensure!(doses > 0, "invalid consumable dose count");
                    Some(doses)
                } else if child(n, "Consumable").is_some() {
                    Some(1)
                } else {
                    None
                },
                consumable_effects: consumables::parse(n)
                    .with_context(|| format!("item activations {id:#x}"))?,
                companion: consumables::companion(n)?,
                heal_range: child(n, "Activate")
                    .and_then(|p| p.attribute("range"))
                    .map(str::parse)
                    .transpose()?
                    .unwrap_or(0.),
                activations: n.children().filter(|p| p.has_tag_name("Activate")).count(),
                abilities: abilities::parse(n).with_context(|| format!("ability {id:#x}"))?,
                cooldown_ms: std::time::Duration::try_from_secs_f32(number(n, "Cooldown", 0.5)?)?
                    .as_millis()
                    .try_into()?,
                enemy: child(n, "Enemy").is_some(),
                god: child(n, "God").is_some(),
                cube: child(n, "Cube").is_some(),
                oryx: child(n, "Oryx").is_some(),
                stasis_immune: child(n, "StasisImmune").is_some(),
                leach_health: child(n, "LeachHealth").is_some(),
                terrain: match text(n, "Terrain").as_str() {
                    "" => 0,
                    "Mountains" => 1,
                    "HighSand" => 2,
                    "HighPlains" => 3,
                    "HighForest" => 4,
                    "MidSand" => 5,
                    "MidPlains" => 6,
                    "MidForest" => 7,
                    "LowSand" => 8,
                    "LowPlains" => 9,
                    "LowForest" => 10,
                    "ShoreSand" => 11,
                    "ShorePlains" => 12,
                    terrain => bail!("unknown selected terrain {terrain}"),
                },
                spawn_weight: child(n, "SpawnProb")
                    .map(|v| -> Result<f32> {
                        let weight: f32 =
                            v.text().context("SpawnProb has no value")?.trim().parse()?;
                        ensure!(
                            weight.is_finite() && (0.0..=1.0).contains(&weight),
                            "invalid SpawnProb"
                        );
                        Ok(weight)
                    })
                    .transpose()?,
                spawn_group: child(n, "Spawn").map(SpawnGroup::parse).transpose()?,
                per_realm_max: child(n, "PerRealmMax")
                    .map(|v| -> Result<usize> {
                        Ok(v.text()
                            .context("PerRealmMax has no value")?
                            .trim()
                            .parse()?)
                    })
                    .transpose()?,
                quest: child(n, "Quest").is_some(),
                quest_level: child(n, "Level")
                    .map(|_| text(n, "Level").parse())
                    .transpose()?,
                quest_window: crate::quest::window(id),
                xp_multiplier: number(n, "XpMult", 1.)?,
                blocks: child(n, "OccupySquare").is_some() || child(n, "FullOccupy").is_some(),
                full_occupy: child(n, "FullOccupy").is_some(),
                occupy_square: child(n, "OccupySquare").is_some(),
                enemy_occupy_square: child(n, "EnemyOccupySquare").is_some(),
                static_object: child(n, "Static").is_some(),
                protects_ground: child(n, "ProtectFromGroundDamage").is_some(),
                destructible: child(n, "Enemy").is_some(),
            };
            ensure!(
                objects.insert(id, o).is_none(),
                "duplicate object type {id:#x}"
            );
        }
        let mut classes = BTreeMap::new();
        for n in doc
            .root_element()
            .children()
            .filter(|n| child(*n, "Player").is_some())
        {
            let id = kind(n.attribute("type").context("class type missing")?)?;
            let mut definition = Class {
                base: [0; 8],
                caps: [0; 8],
                growth: [(0, 0); 8],
                requires: vec![],
            };
            for (i, (_, tag)) in STATS.iter().enumerate() {
                let stat = child(n, tag).context("class base stat missing")?;
                definition.base[i] = stat
                    .text()
                    .context("class base stat empty")?
                    .trim()
                    .parse()?;
                definition.caps[i] = stat
                    .attribute("max")
                    .context("class cap missing")?
                    .parse()?;
                let growth = n
                    .children()
                    .find(|p| p.has_tag_name("LevelIncrease") && p.text() == Some(tag))
                    .context("class growth missing")?;
                definition.growth[i] = (
                    growth
                        .attribute("min")
                        .context("growth minimum missing")?
                        .parse()?,
                    growth
                        .attribute("max")
                        .context("growth maximum missing")?
                        .parse()?,
                );
                let (min, max) = definition.growth[i];
                ensure!(
                    definition.base[i] >= 0
                        && definition.caps[i] >= definition.base[i]
                        && min >= 0
                        && max >= min,
                    "invalid class stat {id:#x} {tag}"
                );
            }
            for unlock in n.children().filter(|p| p.has_tag_name("UnlockLevel")) {
                let name = unlock.text().context("unlock class missing")?;
                let parent = objects
                    .values()
                    .find(|o| o.name == name && o.class == "Player")
                    .context("unknown unlock class")?;
                let level: i32 = unlock
                    .attribute("level")
                    .context("unlock level missing")?
                    .parse()?;
                ensure!((1..=20).contains(&level), "invalid unlock level");
                definition.requires.push((parent.kind, level));
            }
            classes.insert(id, definition);
        }
        ensure!(classes.len() == 13, "unexpected player class count");
        let data = std::fs::read_to_string(dir.join("grounds.xml"))?;
        let grounds_sha256 = format!("{:x}", Sha256::digest(data.as_bytes()));
        let doc = roxmltree::Document::parse(&data)?;
        let mut grounds = BTreeMap::new();
        let mut ground_names = BTreeMap::new();
        let mut pushes = BTreeMap::new();
        let mut hazards = BTreeMap::new();
        for n in doc.root_element().children().filter(|n| n.is_element()) {
            let id = u8::try_from(kind(n.attribute("type").context("ground type missing")?)?)?;
            let speed = if child(n, "NoWalk").is_some() {
                0.
            } else {
                number(n, "Speed", 1.)?
            };
            let push = if child(n, "Push").is_some() {
                let animation =
                    child(n, "Animate").context("pushing ground lacks animation vector")?;
                let mut vector = [0.; 2];
                for (i, axis) in ["dx", "dy"].iter().enumerate() {
                    if let Some(value) = animation.attribute(*axis) {
                        let velocity: f32 = value.parse()?;
                        ensure!(velocity.is_finite(), "non-finite ground push");
                        vector[i] = -velocity / 1000.;
                    }
                }
                (vector[0], vector[1])
            } else {
                (0., 0.)
            };
            let min = number(n, "MinDamage", 0.)? as i32;
            let max = number(n, "MaxDamage", 0.)? as i32;
            ensure!(min >= 0 && max >= min, "invalid ground damage {id:#x}");
            if max > 0 {
                hazards.insert(
                    id,
                    Hazard {
                        name: n.attribute("id").context("hazard name missing")?.into(),
                        min,
                        max,
                    },
                );
            }
            pushes.insert(id, push);
            ground_names.insert(id, n.attribute("id").context("ground name missing")?.into());
            ensure!(
                grounds.insert(id, speed).is_none(),
                "duplicate ground type {id:#x}"
            );
        }
        ensure!(
            objects.len() == 2201 && grounds.len() == 173,
            "unexpected 123.5.1 content counts"
        );
        let out = Self {
            xml_sha256: [objects_sha256, grounds_sha256],
            objects,
            grounds,
            ground_names,
            pushes,
            hazards,
            classes,
        };
        for (kind, name, class, hp, size, full) in [
            (0x122, "Destructible Grey Wall", "Wall", 100, 100, true),
            (0x162, "Destructible Blue Wall", "Wall", 100, 100, true),
            (0x181f, "Tombstone", "GameObject", 1000, 120, false),
        ] {
            let o = out.get(kind)?;
            ensure!(
                o.name == name
                    && o.class == class
                    && o.hp == hp
                    && o.size == size
                    && o.static_object
                    && o.enemy
                    && o.destructible
                    && o.blocks
                    && o.occupy_square
                    && o.enemy_occupy_square
                    && o.full_occupy == full
                    && o.xp_multiplier == 0.
                    && o.defense == 0
                    && !o.stasis_immune
                    && !o.protects_ground
                    && !o.quest
                    && o.projectiles.is_empty()
                    && o.equipment.is_empty(),
                "selected passive scenery signature changed for {kind:#x}"
            );
        }
        let w = out.get(WIZARD)?;
        ensure!(
            out.unlocked(WIZARD, &BTreeMap::new())?,
            "Wizard must be unlocked for a new account"
        );
        ensure!(
            w.hp > 0 && w.equipment.len() == 12 && w.slots.len() == 12,
            "invalid Wizard definition"
        );
        Ok(out)
    }
    pub fn unlocked(&self, class: u16, records: &BTreeMap<u16, i32>) -> Result<bool> {
        let class = self.classes.get(&class).context("unknown player class")?;
        Ok(class
            .requires
            .iter()
            .all(|(kind, level)| records.get(kind).is_some_and(|best| best >= level)))
    }
    pub fn get(&self, k: u16) -> Result<&Object> {
        self.objects
            .get(&k)
            .with_context(|| format!("unknown object type {k:#x}"))
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Cell {
    pub ground: u8,
    pub object: Option<u16>,
    pub config: String,
    pub terrain: u8,
    pub region: u8,
}
#[derive(Clone, Debug, Serialize)]
pub struct Map {
    pub width: i32,
    pub height: i32,
    pub cells: Vec<Cell>,
    pub spawns: Vec<(f32, f32)>,
}
struct MapReader {
    data: Vec<u8>,
    pos: usize,
}
impl MapReader {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        ensure!(n <= self.data.len() - self.pos, "truncated map");
        let p = self.pos;
        self.pos += n;
        Ok(&self.data[p..p + n])
    }
    fn short(&mut self) -> Result<i16> {
        Ok(i16::from_le_bytes(self.take(2)?.try_into()?))
    }
    fn int(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn string(&mut self) -> Result<String> {
        let mut n = 0usize;
        for shift in (0..35).step_by(7) {
            let b = self.byte()?;
            n |= ((b & 127) as usize) << shift;
            if b & 128 == 0 {
                return Ok(std::str::from_utf8(self.take(n)?)?.to_owned());
            }
        }
        bail!("map string length overflow")
    }
}
impl Map {
    pub fn load(path: &Path, content: &Content) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        Self::decode(&bytes, content).with_context(|| format!("load map {}", path.display()))
    }
    pub(crate) fn decode(bytes: &[u8], content: &Content) -> Result<Self> {
        let map = Self::decode_cells(bytes, content)?;
        ensure!(!map.spawns.is_empty(), "map has no spawn cells");
        Ok(map)
    }
    pub(crate) fn decode_cells(bytes: &[u8], content: &Content) -> Result<Self> {
        Self::decode_bounded_cells(bytes, content, false)
    }
    pub(crate) fn decode_realm_cells(
        bytes: &[u8],
        content: &Content,
        dimensions: [usize; 2],
    ) -> Result<Self> {
        Self::decode_bounded_cells(bytes, content, dimensions == [2100, 2100])
    }
    fn decode_bounded_cells(
        bytes: &[u8],
        content: &Content,
        source_local_2100: bool,
    ) -> Result<Self> {
        let mut data = Vec::new();
        flate2::read::ZlibDecoder::new(bytes)
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut data)?;
        ensure!(
            data.len() <= 16 * 1024 * 1024,
            "decompressed map exceeds 16 MiB"
        );
        let mut r = MapReader { data, pos: 0 };
        let count = r.short()?;
        ensure!(count > 0, "empty map dictionary");
        let mut dict = Vec::new();
        for _ in 0..count {
            let ground = u8::try_from(r.short()?).context("map ground out of range")?;
            ensure!(
                content.grounds.contains_key(&ground),
                "unknown map ground {ground:#x}"
            );
            let name = r.string()?;
            let object = if name.is_empty() {
                None
            } else {
                let mut matches = content.objects.values().filter(|o| o.name == name);
                let object = matches
                    .next()
                    .with_context(|| format!("unknown map object {name}"))?;
                ensure!(matches.next().is_none(), "ambiguous map object {name}");
                Some(object.kind)
            };
            let config = r.string()?;
            let terrain = r.byte()?;
            let region = r.byte()?;
            dict.push(Cell {
                ground,
                object,
                config,
                terrain,
                region,
            });
        }
        let width = r.int()?;
        let height = r.int()?;
        ensure!(
            width > 0
                && height > 0
                && ((width <= 2048 && height <= 2048)
                    || (source_local_2100 && width == 2100 && height == 2100)),
            "map dimensions {width}x{height} exceed ordinary limit 2048; admitted 2100-square Realm: {source_local_2100}"
        );
        let mut cells = Vec::new();
        let mut spawns = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let i = usize::try_from(r.short()?).context("map dictionary index out of range")?;
                let c = dict.get(i).context("invalid map dictionary index")?.clone();
                if c.region == 1 {
                    spawns.push((x as f32 + 0.5, y as f32 + 0.5));
                }
                cells.push(c);
            }
        }
        ensure!(r.pos == r.data.len(), "trailing map data");
        Ok(Self {
            width,
            height,
            cells,
            spawns,
        })
    }
    pub fn cell(&self, x: f32, y: f32) -> Option<&Cell> {
        if x < 0. || y < 0. || x >= self.width as f32 || y >= self.height as f32 {
            return None;
        }
        self.cells
            .get(y.floor() as usize * self.width as usize + x.floor() as usize)
    }
}
