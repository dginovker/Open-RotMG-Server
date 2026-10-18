use anyhow::{Context, Result, ensure};
use roxmltree::Node;
use std::{collections::BTreeMap, path::Path};

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
    pub projectile: Option<Projectile>,
    pub activation: Option<(String, i32)>,
    pub consumable: bool,
}
#[derive(Clone, Debug)]
pub struct Projectile {
    pub visual: String,
    pub min: i32,
    pub max: i32,
    pub speed: f32,
    pub lifetime: f32,
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
    text(n, name).with_context(|| {
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
impl Content {
    pub fn load(path: &Path) -> Result<Self> {
        let xml = std::fs::read_to_string(path.join("objects.xml"))?;
        let doc = roxmltree::Document::parse(&xml)?;
        let mut objects = BTreeMap::new();
        for n in doc.root_element().children().filter(Node::is_element) {
            let id = i16::try_from(ints(n.attribute("type").context("object missing type")?)?[0])?;
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
                if let Some(value) = text(n, tag) {
                    stats.insert(stat, value.parse()?);
                }
            }
            let projectile = n
                .children()
                .find(|c| c.has_tag_name("Projectile"))
                .map(|p| -> Result<Projectile> {
                    let (min, max) = match text(p, "Damage") {
                        Some(d) => (d.parse()?, d.parse()?),
                        None => (
                            required(p, "MinDamage")?.parse()?,
                            required(p, "MaxDamage")?.parse()?,
                        ),
                    };
                    Ok(Projectile {
                        visual: required(p, "ObjectId")?.into(),
                        min,
                        max,
                        speed: required(p, "Speed")?.parse::<f32>()? / 10.0,
                        lifetime: required(p, "LifetimeMS")?.parse::<f32>()? / 1000.0,
                    })
                })
                .transpose()?;
            let activation = n
                .children()
                .find(|c| c.has_tag_name("Activate"))
                .map(|a| -> Result<_> {
                    let action = a.text().context("empty Activate")?;
                    let amount = if action == "IncrementStat" {
                        a.attribute("amount")
                            .context("IncrementStat missing amount")?
                            .parse()?
                    } else {
                        0
                    };
                    Ok((action.to_owned(), amount))
                })
                .transpose()?;
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
                ensure!(min <= max, "invalid growth range");
                growth.push((stat, min, max, cap));
            }
            let object = Object {
                id,
                name: n.attribute("id").context("object missing id")?.into(),
                class: required(n, "Class")?.into(),
                stats,
                growth,
                slots: text(n, "SlotTypes")
                    .map(ints)
                    .transpose()?
                    .unwrap_or_default(),
                equipment: text(n, "Equipment")
                    .map(ints)
                    .transpose()?
                    .unwrap_or_default(),
                slot_type: text(n, "SlotType").map(str::parse).transpose()?,
                projectile,
                activation,
                consumable: n.children().any(|c| c.has_tag_name("Consumable")),
            };
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
