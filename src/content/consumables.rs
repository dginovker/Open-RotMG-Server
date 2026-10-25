use super::*;

#[derive(Clone, Debug, Serialize)]
pub struct Companion {
    pub object: String,
    pub permanent: bool,
    pub cooldown: u64,
}

pub(super) fn companion(n: Node<'_, '_>) -> Result<Option<Companion>> {
    if child(n, "Consumable").is_none() && child(n, "InvUse").is_none() {
        return Ok(None);
    }
    n.children()
        .find(|a| a.has_tag_name("Activate") && matches!(a.text(), Some("Pet" | "PermaPet")))
        .map(|a| {
            let permanent = a.text() == Some("PermaPet");
            Ok(Companion {
                object: a
                    .attribute("objectId")
                    .context("companion objectId missing")?
                    .to_owned(),
                permanent,
                cooldown: if permanent {
                    abilities::duration(a, "cooldown")?
                } else {
                    0
                },
            })
        })
        .transpose()
}

impl Content {
    pub fn named(&self, name: &str) -> Result<&Object> {
        let mut matches = self.objects.values().filter(|o| o.name == name);
        let object = matches
            .next()
            .with_context(|| format!("unknown selected object {name}"))?;
        ensure!(
            matches.next().is_none(),
            "ambiguous selected object name {name}"
        );
        Ok(object)
    }
    pub fn companion_kind(&self, name: &str) -> Result<u16> {
        let object = self.named(name)?;
        ensure!(
            object.class == "Character" && !object.enemy,
            "companion target must be a friendly character"
        );
        Ok(object.kind)
    }
    pub fn permanent_companion(&self, kind: u16) -> Result<()> {
        let object = self.get(kind)?;
        ensure!(
            object.class == "Character"
                && !object.enemy
                && self.objects.values().any(|item| item
                    .companion
                    .as_ref()
                    .is_some_and(|c| c.permanent && c.object == object.name)),
            "unrecognized saved companion"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub enum ConsumableEffect {
    Heal {
        amount: i32,
        range: Option<f32>,
    },
    Mana {
        amount: i32,
        range: Option<f32>,
    },
    Boost {
        index: usize,
        amount: i32,
        duration: u64,
        range: Option<f32>,
    },
    Condition {
        effect: u8,
        duration: u64,
    },
    Clear {
        effect: u8,
        check: Option<u8>,
    },
    Cleanse,
    Unsupported(String),
}

pub(super) fn parse(n: Node<'_, '_>) -> Result<Vec<ConsumableEffect>> {
    if child(n, "Consumable").is_none() && child(n, "InvUse").is_none() {
        return Ok(Vec::new());
    }
    use abilities::{damage, duration, effect, positive};
    n.children()
        .filter(|a| a.has_tag_name("Activate"))
        .map(|a| {
            let name = a.text().context("empty item activation")?;
            Ok(match name {
                "Heal" | "HealNova" => ConsumableEffect::Heal {
                    amount: damage(a, "amount")?,
                    range: if name == "HealNova" {
                        Some(positive(a, "range")?)
                    } else {
                        None
                    },
                },
                "Magic" | "MagicNova" => ConsumableEffect::Mana {
                    amount: damage(a, "amount")?,
                    range: if name == "MagicNova" {
                        Some(positive(a, "range")?)
                    } else {
                        None
                    },
                },
                "StatBoostSelf" | "StatBoostAura" => ConsumableEffect::Boost {
                    index: {
                        let stat: u8 =
                            a.attribute("stat").context("boost stat missing")?.parse()?;
                        STATS
                            .iter()
                            .position(|(id, _)| *id == stat)
                            .context("unsupported boost stat")?
                    },
                    amount: damage(a, "amount")?,
                    duration: duration(a, "duration")?,
                    range: if name == "StatBoostAura" {
                        Some(positive(a, "range")?)
                    } else {
                        None
                    },
                },
                "ConditionEffectSelf" => ConsumableEffect::Condition {
                    effect: effect(
                        a.attribute("effect")
                            .context("consumable condition missing")?,
                    )?,
                    duration: duration(a, "duration")?,
                },
                "ClearConditionEffectSelf" => ConsumableEffect::Clear {
                    effect: effect(a.attribute("effect").context("clear condition missing")?)?,
                    check: a.attribute("checkExistingEffect").map(effect).transpose()?,
                },
                "RemoveNegativeConditionsSelf" => ConsumableEffect::Cleanse,
                other => ConsumableEffect::Unsupported(other.to_owned()),
            })
        })
        .collect()
}
