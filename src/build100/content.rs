use anyhow::{Context, Result, ensure};
use roxmltree::Node;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Clone, Debug)]
pub struct Object {
    pub id: i16,
    pub name: String,
    pub class: String,
    pub stats: BTreeMap<u8, i32>,
    pub growth: Vec<(u8, i32, i32, i32)>,
    pub slots: Vec<i32>,
    pub equipment: Vec<i32>,
    pub slot_type: Option<i32>,
    pub projectiles: Vec<(u8, Projectile)>,
    pub activations: Vec<Activation>,
    pub root_damage: Option<(i32, i32)>,
    pub rate_of_fire: Option<f32>,
    pub mp_cost: Option<i32>,
    pub num_projectiles: u8,
    pub arc_gap_radians: f32,
    pub bonuses: Vec<(u8, i32)>,
    pub effects: Vec<ConditionEffect>,
    pub xp_multiplier: f32,
    pub loot: Vec<Loot>,
    pub consumable: bool,
    pub occupy_square: bool,
    pub is_static: bool,
    pub is_enemy: bool,
}
#[derive(Clone, Debug)]
pub struct Projectile {
    pub visual: String,
    pub min: i32,
    pub max: i32,
    pub speed: f32,
    pub lifetime: f32,
    pub amplitude: f32,
    pub frequency: f32,
    pub wavy: bool,
    pub continues: bool,
    pub effects: Vec<ConditionEffect>,
    pub features: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct Activation {
    pub name: String,
    pub attributes: BTreeMap<String, String>,
}
impl Projectile {
    // July Armor Pierce is condition 17 (including duration zero), not a later XML flag.
    pub fn armor_piercing(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.condition == Condition::ArmorPierce)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Condition {
    Nothing,
    Dead,
    Quiet,
    Weak,
    Slowed,
    Sick,
    Dazed,
    Stunned,
    Blind,
    Hallucinating,
    Drunk,
    Confused,
    Knockback,
    Invisible,
    Paralyzed,
    Jumpy,
    Bleeding,
    ArmorPierce,
    Healing,
    Damaging,
    Berserk,
    Paused,
}
impl Condition {
    fn parse(name: &str) -> Result<Self> {
        use Condition::*;
        Ok(match name {
            "Nothing" => Nothing,
            "Dead" => Dead,
            "Quiet" => Quiet,
            "Weak" => Weak,
            "Slowed" => Slowed,
            "Sick" => Sick,
            "Dazed" => Dazed,
            "Stunned" => Stunned,
            "Blind" => Blind,
            "Hallucinating" => Hallucinating,
            "Drunk" => Drunk,
            "Confused" => Confused,
            "Knockback" => Knockback,
            "Invisible" => Invisible,
            "Paralyzed" => Paralyzed,
            "Jumpy" => Jumpy,
            "Bleeding" => Bleeding,
            "Armor Pierce" => ArmorPierce,
            "Healing" => Healing,
            "Damaging" => Damaging,
            "Berserk" => Berserk,
            "Paused" => Paused,
            _ => anyhow::bail!("unknown July condition {name}"),
        })
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct ConditionEffect {
    pub condition: Condition,
    pub duration_seconds: f32,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Loot {
    EmptyContainer,
    Item { name: String, probability: f32 },
}
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    IncrementStat { stat: u8, amount: i32 },
    Shoot,
    BulletNova,
    HealNova { range: f32, healing: i32 },
    ConditionEffectSelf,
    ConditionEffectAura { range: f32 },
    RemoveNegativeConditions { range: f32, healing: i32 },
    Teleport,
    OpenPortal { dungeon_name: String },
}
impl Activation {
    pub fn action(&self) -> Result<Action> {
        let fields: &[&str] = match self.name.as_str() {
            "IncrementStat" => &["stat", "amount"],
            "HealNova" | "RemoveNegativeConditions" => &["range", "healing"],
            "ConditionEffectAura" => &["range"],
            "OpenPortal" => &["dungeonName"],
            "Shoot" | "BulletNova" | "ConditionEffectSelf" | "Teleport" => &[],
            name => anyhow::bail!("unsupported build100 activation {name}"),
        };
        ensure!(
            self.attributes.keys().all(|k| fields.contains(&k.as_str())),
            "{}: unsupported activation attribute",
            self.name
        );
        for field in fields {
            ensure!(
                self.attributes.contains_key(*field),
                "{} missing {field}",
                self.name
            );
        }
        let positive = |key: &str| -> Result<i32> {
            let value = self.attributes[key].parse()?;
            ensure!(value > 0, "{}: invalid {key}", self.name);
            Ok(value)
        };
        let range = || -> Result<f32> {
            let value: f32 = self.attributes["range"].parse()?;
            ensure!(
                value.is_finite() && value > 0.0,
                "{}: invalid range",
                self.name
            );
            Ok(value)
        };
        Ok(match self.name.as_str() {
            "IncrementStat" => Action::IncrementStat {
                stat: stat_id(&self.attributes["stat"], true)?,
                amount: positive("amount")?,
            },
            "Shoot" => Action::Shoot,
            "BulletNova" => Action::BulletNova,
            "HealNova" => Action::HealNova {
                range: range()?,
                healing: positive("healing")?,
            },
            "RemoveNegativeConditions" => Action::RemoveNegativeConditions {
                range: range()?,
                healing: positive("healing")?,
            },
            "ConditionEffectSelf" => Action::ConditionEffectSelf,
            "ConditionEffectAura" => Action::ConditionEffectAura { range: range()? },
            "Teleport" => Action::Teleport,
            "OpenPortal" => {
                let dungeon_name = &self.attributes["dungeonName"];
                ensure!(!dungeon_name.trim().is_empty(), "empty dungeonName");
                Action::OpenPortal {
                    dungeon_name: dungeon_name.clone(),
                }
            }
            _ => unreachable!(),
        })
    }
}
impl Object {
    fn unique_projectile(&self, id: u8) -> Result<&Projectile> {
        let mut matches = self.projectiles.iter().filter(|(key, _)| *key == id);
        let (_, projectile) = matches
            .next()
            .with_context(|| format!("{} missing projectile {id}", self.name))?;
        ensure!(
            matches.next().is_none(),
            "{}: ambiguous duplicate projectile {id}",
            self.name
        );
        Ok(projectile)
    }
    pub fn arena_projectile(&self, id: u8) -> Result<&Projectile> {
        let projectile = self.unique_projectile(id)?;
        ensure!(
            projectile.features.is_empty(),
            "{} projectile {id} requires unsupported arena semantics: {:?}",
            self.name,
            projectile.features
        );
        Ok(projectile)
    }
    pub fn normal_projectile(&self, id: u8) -> Result<&Projectile> {
        let projectile = self.unique_projectile(id)?;
        // Nested equip activations are inert in the original Projectile; ordinary Continues has no admitted multihit authority.
        ensure!(
            !projectile.continues
                && projectile
                    .features
                    .iter()
                    .all(|f| matches!(f.as_str(), "Wavy" | "ConditionEffect" | "ActivateOnEquip"))
                && projectile
                    .effects
                    .iter()
                    .all(|e| e.condition == Condition::ArmorPierce
                        && e.duration_seconds.is_finite()
                        && e.duration_seconds >= 0.),
            "{} projectile {id} requires unsupported normal projectile semantics: {:?}, effects={:?}, Continues={}",
            self.name,
            projectile.features,
            projectile.effects,
            projectile.continues
        );
        ensure!(
            0 <= projectile.min
                && projectile.min <= projectile.max
                && projectile.max <= i32::from(i16::MAX),
            "{} projectile {id}: invalid damage range",
            self.name
        );
        ensure!(
            projectile.lifetime > 0.,
            "{} projectile {id}: nonpositive lifetime",
            self.name
        );
        crate::rules::combat::projectile_position(projectile, (0., 0.), 0., 1, 0.)?;
        Ok(projectile)
    }
    pub fn quiver_projectile(&self) -> Result<&Projectile> {
        let (dex, condition, visual, continues) = match self.id {
            0xa61 | 0xa62 => (0, Condition::Slowed, "Red Arrow", false),
            0xa63 => (2, Condition::Slowed, "Green Arrow", true),
            0xa64 => (3, Condition::Paralyzed, "Blue Arrow", false),
            0xa65 => (5, Condition::Paralyzed, "Gold Arrow", true),
            _ => anyhow::bail!("unsupported normal quiver shape"),
        };
        ensure!(
            self.class == "Equipment"
                && self.slot_type == Some(15)
                && !self.consumable
                && self.activations.len() == 1
                && self.activations[0].action()? == Action::Shoot
                && self.num_projectiles == 1
                && self.rate_of_fire.is_none()
                && self.arc_gap_radians == std::f32::consts::PI / 16.
                && (if dex == 0 {
                    self.bonuses.is_empty()
                } else {
                    self.bonuses == [(28, dex)]
                })
                && self.projectiles.len() == 1,
            "unsupported normal quiver shape"
        );
        let projectile = self.unique_projectile(0)?;
        let effects = [ConditionEffect {
            condition,
            duration_seconds: 3.,
        }];
        ensure!(
            self.effects == effects
                && projectile.effects == effects
                && projectile.continues == continues
                && projectile.visual == visual
                && (if continues {
                    projectile.features == ["ConditionEffect", "Continues"]
                } else {
                    projectile.features == ["ConditionEffect"]
                })
                && !projectile.wavy
                && projectile.amplitude == 0.
                && projectile.frequency == 1.
                && projectile.speed == 15.
                && projectile.lifetime == 1.
                && self.root_damage == Some((projectile.min, projectile.max))
                && projectile.min >= 0
                && projectile.min <= projectile.max
                && projectile.max <= i32::from(i16::MAX)
                && self.mp_cost.is_some_and(|cost| cost >= 0),
            "unsupported normal quiver root/projectile semantics"
        );
        Ok(projectile)
    }
    pub fn shoot_ability_projectile(&self, class: i16) -> Result<&Projectile> {
        if self.id != 0xa08 {
            ensure!(class == 0x307, "normal quiver requires Archer");
            return self.quiver_projectile();
        }
        ensure!(
            class == 0x31e
                && self.class == "Equipment"
                && self.slot_type == Some(5)
                && !self.consumable
                && self.activations.len() == 1
                && self.activations[0].action()? == Action::Shoot
                && self.mp_cost == Some(80)
                && self.root_damage == Some((30, 70))
                && self.bonuses == [(21, 2)]
                && self.num_projectiles == 1
                && self.arc_gap_radians == std::f32::consts::PI / 16.
                && self.rate_of_fire.is_none()
                && self.projectiles.len() == 1,
            "unsupported Wooden Shield/Knight Shoot shape"
        );
        let projectile = self.unique_projectile(0)?;
        let effects = [ConditionEffect {
            condition: Condition::Stunned,
            duration_seconds: 3.,
        }];
        ensure!(
            self.effects == effects
                && projectile.effects == effects
                && projectile.visual == "Silver Shield"
                && projectile.min == 30
                && projectile.max == 70
                && projectile.speed == 16.
                && projectile.lifetime == 0.2
                && !projectile.wavy
                && projectile.amplitude == 0.
                && projectile.frequency == 1.
                && projectile.continues
                && projectile.features == ["ConditionEffect", "Continues"],
            "unsupported Wooden Shield root/projectile semantics"
        );
        Ok(projectile)
    }
    pub fn single_activation(&self) -> Result<&Activation> {
        ensure!(
            self.activations.len() == 1,
            "{} requires exactly one arena activation, found {}",
            self.name,
            self.activations.len()
        );
        Ok(&self.activations[0])
    }
}
pub struct Content {
    pub objects: BTreeMap<i16, Object>,
}
fn text<'a>(n: Node<'a, 'a>, name: &str) -> Option<&'a str> {
    n.children()
        .find(|c| c.has_tag_name(name))
        .and_then(|c| c.text())
}
fn required<'a>(n: Node<'a, 'a>, name: &str) -> Result<&'a str> {
    ensure!(
        n.children().filter(|c| c.has_tag_name(name)).count() <= 1,
        "duplicate {name}"
    );
    text(n, name)
        .filter(|s| !s.trim().is_empty())
        .with_context(|| {
            format!(
                "{} missing {name}",
                n.attribute("id").unwrap_or(n.tag_name().name())
            )
        })
}
fn ints(value: &str) -> Result<Vec<i32>> {
    value
        .split(',')
        .map(|s| {
            let s = s.trim();
            Ok(if let Some(hex) = s.strip_prefix("0x") {
                i32::from_str_radix(hex, 16)?
            } else {
                s.parse()?
            })
        })
        .collect()
}
fn scalar<T: std::str::FromStr>(n: Node<'_, '_>, tag: &str) -> Result<Option<T>>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let mut nodes = n.children().filter(|c| c.has_tag_name(tag));
    let Some(value) = nodes.next() else {
        return Ok(None);
    };
    ensure!(nodes.next().is_none(), "duplicate {tag}");
    ensure!(
        value.children().all(|c| !c.is_element()),
        "nested {tag} operand"
    );
    Ok(Some(
        value
            .text()
            .with_context(|| format!("empty {tag}"))?
            .trim()
            .parse()
            .with_context(|| format!("invalid {tag}"))?,
    ))
}
fn damage(n: Node<'_, '_>) -> Result<Option<(i32, i32)>> {
    let min = scalar::<i32>(n, "MinDamage")?;
    let max = scalar::<i32>(n, "MaxDamage")?;
    match (min, max) {
        (None, None) => Ok(None),
        (Some(min), Some(max)) if min >= 0 && min <= max && max <= i32::from(i16::MAX) => {
            Ok(Some((min, max)))
        }
        _ => anyhow::bail!("incomplete or invalid damage range"),
    }
}
fn stat_id(value: &str, current: bool) -> Result<u8> {
    let stat = value.parse()?;
    ensure!(
        [0, 3, 20, 21, 22, 26, 27, 28].contains(&stat) || (current && [1, 4].contains(&stat)),
        "unknown stat ID {value}"
    );
    Ok(stat)
}
fn bonuses(n: Node<'_, '_>) -> Result<Vec<(u8, i32)>> {
    n.children()
        .filter(|c| c.has_tag_name("ActivateOnEquip"))
        .map(|a| {
            ensure!(
                a.text() == Some("IncrementStat") && a.children().all(|c| !c.is_element()),
                "unsupported ActivateOnEquip action"
            );
            ensure!(
                a.attributes()
                    .all(|a| ["stat", "amount"].contains(&a.name())),
                "unsupported ActivateOnEquip attribute"
            );
            Ok((
                stat_id(
                    a.attribute("stat").context("equipment missing stat")?,
                    false,
                )?,
                a.attribute("amount")
                    .context("equipment missing amount")?
                    .parse()?,
            ))
        })
        .collect()
}
fn effects(n: Node<'_, '_>) -> Result<Vec<ConditionEffect>> {
    n.children()
        .filter(|c| c.has_tag_name("ConditionEffect"))
        .map(|e| {
            ensure!(
                e.attributes().all(|a| a.name() == "duration")
                    && e.children().all(|c| !c.is_element()),
                "unsupported ConditionEffect operand"
            );
            let duration_seconds: f32 = e
                .attribute("duration")
                .context("condition missing duration")?
                .parse()?;
            ensure!(
                duration_seconds.is_finite() && duration_seconds >= 0.0,
                "invalid condition duration"
            );
            Ok(ConditionEffect {
                condition: Condition::parse(e.text().context("empty ConditionEffect")?)?,
                duration_seconds,
            })
        })
        .collect()
}
fn flag(n: Node<'_, '_>, tag: &str) -> Result<bool> {
    let mut nodes = n.children().filter(|c| c.has_tag_name(tag));
    let Some(value) = nodes.next() else {
        return Ok(false);
    };
    ensure!(
        nodes.next().is_none()
            && value.attributes().len() == 0
            && value.children().all(|c| !c.is_element())
            && value.text().is_none_or(|s| s.trim().is_empty()),
        "invalid {tag} flag"
    );
    Ok(true)
}
impl Content {
    pub fn load(path: &Path) -> Result<Self> {
        let xml = std::fs::read_to_string(path.join("objects.xml"))?;
        Self::parse(&xml)
    }
    pub fn parse(xml: &str) -> Result<Self> {
        let doc = roxmltree::Document::parse(xml)?;
        ensure!(
            doc.root_element().has_tag_name("Objects"),
            "expected Objects root"
        );
        let mut objects = BTreeMap::new();
        let mut names = BTreeSet::new();
        for n in doc.root_element().children().filter(Node::is_element) {
            ensure!(
                n.has_tag_name("Object"),
                "unexpected content element {}",
                n.tag_name().name()
            );
            let ids = ints(n.attribute("type").context("object missing type")?)?;
            ensure!(
                ids.len() == 1 && ids[0] >= 0,
                "expected one nonnegative object type"
            );
            let id = i16::try_from(ids[0])?;
            let name = n.attribute("id").context("object missing id")?;
            ensure!(
                !name.is_empty() && names.insert(name),
                "empty or duplicate object name {name}"
            );
            let object = (|| -> Result<Object> {
                let mut stats = BTreeMap::new();
                for (tag, stat) in [
                    ("MaxHitPoints", 0),
                    ("Size", 2),
                    ("MaxMagicPoints", 3),
                    ("Attack", 20),
                    ("Defense", 21),
                    ("Speed", 22),
                    ("Dexterity", 28),
                    ("HpRegen", 26),
                    ("MpRegen", 27),
                ] {
                    if let Some(value) = scalar::<i32>(n, tag)? {
                        ensure!(value >= 0, "negative {tag}");
                        if let Some(cap) = n
                            .children()
                            .find(|c| c.has_tag_name(tag))
                            .and_then(|c| c.attribute("max"))
                        {
                            ensure!(
                                cap.parse::<i32>()
                                    .with_context(|| format!("invalid {tag} cap"))?
                                    >= value,
                                "invalid {tag} cap"
                            );
                        }
                        stats.insert(stat, value);
                    }
                }
                let mut projectiles = Vec::new();
                for p in n.children().filter(|c| c.has_tag_name("Projectile")) {
                    ensure!(
                        p.attributes().all(|a| a.name() == "id"),
                        "unsupported projectile attribute"
                    );
                    let projectile_id = p
                        .attribute("id")
                        .map(str::parse::<u8>)
                        .transpose()?
                        .unwrap_or(0);
                    let fixed = scalar::<i32>(p, "Damage")?;
                    let range = damage(p)?;
                    ensure!(
                        fixed.is_none() || range.is_none(),
                        "conflicting projectile damage definitions"
                    );
                    let (min, max) = range
                        .or(fixed.map(|d| (d, d)))
                        .context("missing projectile damage")?;
                    // Validate inert metadata without promoting it to root equipment bonuses.
                    bonuses(p)?;
                    let projectile = Projectile {
                        visual: required(p, "ObjectId")?.into(),
                        min,
                        max,
                        speed: scalar::<f32>(p, "Speed")?.context("projectile missing Speed")?
                            / 10.0,
                        lifetime: scalar::<f32>(p, "LifetimeMS")?
                            .context("projectile missing LifetimeMS")?
                            / 1000.0,
                        amplitude: scalar(p, "Amplitude")?.unwrap_or(0.0),
                        frequency: scalar(p, "Frequency")?.unwrap_or(1.0),
                        wavy: flag(p, "Wavy")?,
                        continues: flag(p, "Continues")?,
                        effects: effects(p)?,
                        features: p
                            .children()
                            .filter(Node::is_element)
                            .filter(|c| {
                                ![
                                    "ObjectId",
                                    "Damage",
                                    "MinDamage",
                                    "MaxDamage",
                                    "Speed",
                                    "LifetimeMS",
                                    "Amplitude",
                                    "Frequency",
                                ]
                                .contains(&c.tag_name().name())
                            })
                            .map(|c| c.tag_name().name().to_owned())
                            .collect(),
                    };
                    ensure!(
                        min >= 0 && min <= max && max <= i32::from(i16::MAX),
                        "{name}: invalid projectile damage range"
                    );
                    ensure!(
                        projectile.speed.is_finite()
                            && projectile.speed > 0.0
                            && projectile.lifetime.is_finite()
                            && projectile.lifetime > 0.0
                            && projectile.amplitude.is_finite()
                            && projectile.frequency.is_finite(),
                        "{name}: invalid projectile motion"
                    );
                    projectiles.push((projectile_id, projectile));
                }
                let activations: Vec<Activation> = n
                    .children()
                    .filter(|c| c.has_tag_name("Activate"))
                    .map(|a| -> Result<_> {
                        ensure!(
                            a.children().all(|c| !c.is_element()),
                            "unsupported nested Activate operand"
                        );
                        let activation = Activation {
                            name: a.text().context("empty Activate")?.into(),
                            attributes: a
                                .attributes()
                                .map(|a| (a.name().into(), a.value().into()))
                                .collect(),
                        };
                        activation
                            .action()
                            .with_context(|| format!("Activate {}", activation.name))?;
                        Ok(activation)
                    })
                    .collect::<Result<Vec<_>>>()?;
                let mut growth = vec![];
                for n_growth in n.children().filter(|c| c.has_tag_name("LevelIncrease")) {
                    let field = n_growth.text().context("empty LevelIncrease")?;
                    let stat = match field {
                        "MaxHitPoints" => 0,
                        "MaxMagicPoints" => 3,
                        "Attack" => 20,
                        "Defense" => 21,
                        "Speed" => 22,
                        "Dexterity" => 28,
                        "HpRegen" => 26,
                        "MpRegen" => 27,
                        _ => anyhow::bail!("unknown growth field {field}"),
                    };
                    let cap = n
                        .children()
                        .find(|c| c.has_tag_name(field))
                        .and_then(|c| c.attribute("max"))
                        .context("growth field missing cap")?
                        .parse()?;
                    let min = n_growth
                        .attribute("min")
                        .context("growth missing min")?
                        .parse()?;
                    let max = n_growth
                        .attribute("max")
                        .context("growth missing max")?
                        .parse()?;
                    ensure!(
                        min >= 0
                            && min <= max
                            && stats
                                .get(&stat)
                                .is_some_and(|base| *base >= 0 && *base <= cap),
                        "invalid growth range/cap"
                    );
                    ensure!(
                        !growth.iter().any(|(s, _, _, _)| *s == stat),
                        "duplicate growth stat {stat}"
                    );
                    growth.push((stat, min, max, cap));
                }
                let root_damage = damage(n).context("root damage")?;
                let rate_of_fire: Option<f32> = scalar(n, "RateOfFire")?;
                ensure!(
                    rate_of_fire.is_none_or(|v| v.is_finite() && v > 0.0),
                    "invalid RateOfFire"
                );
                let mp_cost: Option<i32> = scalar(n, "MpCost")?;
                ensure!(mp_cost.is_none_or(|v| v >= 0), "invalid MpCost");
                // Player.use reads ArcGap as radians; only absence uses PI/16.
                let num_projectiles = scalar::<u8>(n, "NumProjectiles")?.unwrap_or(1);
                ensure!(num_projectiles > 0, "invalid NumProjectiles");
                let arc_gap_radians: f32 =
                    scalar(n, "ArcGap")?.unwrap_or(std::f32::consts::PI / 16.0);
                ensure!(
                    arc_gap_radians.is_finite() && arc_gap_radians >= 0.0,
                    "invalid ArcGap"
                );
                let xp_multiplier: f32 = scalar(n, "XpMult")?.unwrap_or(1.0);
                ensure!(
                    xp_multiplier.is_finite() && xp_multiplier >= 0.0,
                    "invalid XpMult"
                );
                let effects = effects(n)?;
                for a in &activations {
                    match a.action()? {
                        Action::Shoot | Action::BulletNova => ensure!(
                            root_damage.is_some() && !projectiles.is_empty(),
                            "{} missing root damage/projectile",
                            a.name
                        ),
                        Action::ConditionEffectSelf | Action::ConditionEffectAura { .. } => {
                            ensure!(
                                !effects.is_empty(),
                                "{} missing root ConditionEffect",
                                a.name
                            )
                        }
                        _ => {}
                    }
                }
                let loot = n
                    .children()
                    .filter(|c| c.has_tag_name("Loot"))
                    .map(|l| -> Result<_> {
                        ensure!(
                            l.children().all(|c| !c.is_element()),
                            "unsupported nested Loot"
                        );
                        match l.text().map(str::trim).filter(|s| !s.is_empty()) {
                            None => {
                                ensure!(
                                    l.attributes().len() == 0
                                        && required(n, "Class")? == "Container",
                                    "invalid empty Loot marker"
                                );
                                Ok(Loot::EmptyContainer)
                            }
                            Some(name) => {
                                ensure!(
                                    l.attributes().all(|a| a.name() == "prob"),
                                    "unsupported Loot attribute"
                                );
                                let probability: f32 =
                                    l.attribute("prob").context("Loot missing prob")?.parse()?;
                                ensure!(
                                    probability.is_finite() && (0.0..=1.0).contains(&probability),
                                    "invalid Loot probability"
                                );
                                Ok(Loot::Item {
                                    name: name.into(),
                                    probability,
                                })
                            }
                        }
                    })
                    .collect::<Result<Vec<_>>>()?;
                let object = Object {
                    id,
                    name: n.attribute("id").context("object missing id")?.into(),
                    class: required(n, "Class")?.into(),
                    stats,
                    growth,
                    slots: scalar::<String>(n, "SlotTypes")?
                        .as_deref()
                        .map(ints)
                        .transpose()?
                        .unwrap_or_default(),
                    equipment: scalar::<String>(n, "Equipment")?
                        .as_deref()
                        .map(ints)
                        .transpose()?
                        .unwrap_or_default(),
                    slot_type: scalar(n, "SlotType")?,
                    projectiles,
                    activations,
                    root_damage,
                    rate_of_fire,
                    mp_cost,
                    num_projectiles,
                    arc_gap_radians,
                    bonuses: bonuses(n)?,
                    effects,
                    xp_multiplier,
                    loot,
                    consumable: n.children().any(|c| c.has_tag_name("Consumable")),
                    occupy_square: n.children().any(|c| c.has_tag_name("OccupySquare")),
                    is_static: n.children().any(|c| c.has_tag_name("Static")),
                    is_enemy: n.children().any(|c| c.has_tag_name("Enemy")),
                };
                ensure!(
                    [
                        "Character",
                        "Equipment",
                        "GameObject",
                        "Projectile",
                        "Wall",
                        "Player",
                        "Portal",
                        "Container"
                    ]
                    .contains(&object.class.as_str()),
                    "{name}: unsupported object class {}",
                    object.class
                );
                Ok(object)
            })()
            .with_context(|| format!("object {id:#x} {name}"))?;
            ensure!(
                objects.insert(id, object).is_none(),
                "duplicate object type {id}"
            );
        }
        let content = Self { objects };
        for name in [
            "Wizard",
            "Pirate",
            "Loot Bag",
            "Health Potion",
            "Energy Staff",
        ] {
            content.named(name)?;
        }
        for object in content.objects.values() {
            for (_, projectile) in &object.projectiles {
                ensure!(
                    content
                        .named(&projectile.visual)
                        .with_context(|| format!("{} projectile visual", object.name))?
                        .class
                        == "Projectile",
                    "{}: visual is not a Projectile",
                    object.name
                );
            }
            for entry in &object.loot {
                if let Loot::Item { name, .. } = entry {
                    ensure!(
                        content
                            .named(name)
                            .with_context(|| format!("{} Loot", object.name))?
                            .class
                            == "Equipment",
                        "{}: Loot item {name} is not Equipment",
                        object.name
                    );
                }
            }
            for &item in &object.equipment {
                if item != -1 {
                    content.get(i16::try_from(item)?)?;
                }
            }
        }
        Ok(content)
    }
    pub fn get(&self, id: i16) -> Result<&Object> {
        self.objects
            .get(&id)
            .with_context(|| format!("unknown build100 object type {id:#x}"))
    }
    pub fn named(&self, name: &str) -> Result<&Object> {
        self.objects
            .values()
            .find(|o| o.name == name)
            .with_context(|| format!("build100 content missing {name}"))
    }
}

#[derive(Debug)]
pub struct Ground {
    pub id: u8,
    pub name: String,
    pub no_walk: bool,
    pub sink: bool,
    pub damage: Option<(i32, i32)>,
}
#[derive(Debug)]
pub struct Grounds {
    pub types: BTreeMap<u8, Ground>,
}
impl Grounds {
    pub fn load(path: &Path) -> Result<Self> {
        Self::parse(&std::fs::read_to_string(path.join("grounds.xml"))?)
    }
    pub fn parse(xml: &str) -> Result<Self> {
        let doc = roxmltree::Document::parse(xml)?;
        ensure!(
            doc.root_element().has_tag_name("GroundTypes"),
            "expected GroundTypes root"
        );
        let mut types = BTreeMap::new();
        for n in doc.root_element().children().filter(Node::is_element) {
            ensure!(
                n.has_tag_name("Ground"),
                "unexpected ground element {}",
                n.tag_name().name()
            );
            let ids = ints(n.attribute("type").context("ground missing type")?)?;
            ensure!(ids.len() == 1, "expected one ground type");
            let id = u8::try_from(ids[0]).context("ground type outside July byte range")?;
            let name = n
                .attribute("id")
                .filter(|s| !s.is_empty())
                .context("ground missing id")?;
            let min = text(n, "MinDamage").map(str::parse::<i32>).transpose()?;
            let max = text(n, "MaxDamage").map(str::parse::<i32>).transpose()?;
            let damage = match (min, max) {
                (None, None) => None,
                (Some(min), Some(max)) if min >= 0 && min <= max => Some((min, max)),
                _ => anyhow::bail!("{name}: incomplete or invalid ground damage range"),
            };
            ensure!(
                !n.children()
                    .any(|c| c.has_tag_name("Speed") || c.has_tag_name("SpeedModifier")),
                "{name}: terrain speed modifiers are not July rules"
            );
            let ground = Ground {
                id,
                name: name.into(),
                no_walk: n.children().any(|c| c.has_tag_name("NoWalk")),
                sink: n.children().any(|c| c.has_tag_name("Sink")),
                damage,
            };
            ensure!(
                types.insert(id, ground).is_none(),
                "duplicate ground type {id:#x}"
            );
        }
        ensure!(!types.is_empty(), "empty build100 grounds");
        Ok(Self { types })
    }
    pub fn get(&self, id: u8) -> Result<&Ground> {
        self.types
            .get(&id)
            .with_context(|| format!("unknown build100 ground type {id:#x}"))
    }
}

#[cfg(test)]
#[path = "content_tests.rs"]
mod tests;
