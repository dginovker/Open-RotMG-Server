use super::*;

#[derive(Clone, Debug, Serialize)]
pub enum Ability {
    Shoot,
    Nova,
    Heal {
        amount: i32,
        range: f32,
    },
    Condition {
        effect: u8,
        duration: u64,
        range: Option<f32>,
    },
    Clear {
        effect: u8,
        check: Option<u8>,
        range: f32,
    },
    Cleanse {
        range: f32,
    },
    Poison {
        radius: f32,
        damage: i32,
        duration: u64,
    },
    Drain {
        radius: f32,
        damage: i32,
    },
    Trap {
        radius: f32,
        damage: i32,
        effect: u8,
        duration: u64,
    },
    Stasis {
        duration: u64,
    },
    Teleport {
        distance: f32,
    },
    Decoy {
        duration: u64,
        angle: f32,
    },
    Lightning {
        damage: i32,
        targets: usize,
        condition: Option<(u8, u64)>,
        color: u32,
    },
    Daze {
        radius: f32,
        damage: i32,
    },
    SpiritBomb,
}
pub(super) fn positive(n: Node<'_, '_>, name: &str) -> Result<f32> {
    let value: f32 = n
        .attribute(name)
        .with_context(|| format!("ability attribute {name} missing"))?
        .parse()?;
    ensure!(value.is_finite() && value > 0., "invalid ability {name}");
    Ok(value)
}
pub(super) fn damage(n: Node<'_, '_>, name: &str) -> Result<i32> {
    let value: i32 = n
        .attribute(name)
        .with_context(|| format!("ability attribute {name} missing"))?
        .parse()?;
    ensure!(
        value > 0 && value <= i16::MAX as i32,
        "invalid ability {name}"
    );
    Ok(value)
}
pub(super) fn duration(n: Node<'_, '_>, name: &str) -> Result<u64> {
    let ms = std::time::Duration::try_from_secs_f32(positive(n, name)?)?
        .as_millis()
        .try_into()?;
    ensure!(ms > 0 && ms <= 60000, "invalid ability duration");
    Ok(ms)
}
pub(super) fn effect(name: &str) -> Result<u8> {
    Ok(match name {
        "Invisible" => crate::condition::INVISIBLE,
        "Speedy" => 15,
        "Berserk" => crate::condition::BERSERK,
        "Healing" => crate::condition::HEALING,
        "Damaging" => crate::condition::DAMAGING,
        "Armored" => crate::condition::ARMORED,
        "Invulnerable" => crate::condition::INVULNERABLE,
        "Hexed" => 28,
        name => crate::condition::projectile_effect(name)?,
    })
}
pub(super) fn parse(n: Node<'_, '_>) -> Result<Vec<Ability>> {
    if !matches!(
        text(n, "SlotType").as_str(),
        "4" | "5" | "11" | "12" | "13" | "15" | "16" | "18" | "19" | "20" | "21" | "22" | "23"
    ) {
        return Ok(Vec::new());
    }
    n.children()
        .filter(|a| a.has_tag_name("Activate"))
        .map(|a| {
            Ok(match a.text().context("empty ability activation")? {
                "Shoot" => Ability::Shoot,
                "BulletNova" => Ability::Nova,
                "HealNova" => Ability::Heal {
                    amount: damage(a, "amount")?,
                    range: positive(a, "range")?,
                },
                "ConditionEffectSelf" | "ConditionEffectAura" => Ability::Condition {
                    effect: effect(a.attribute("effect").context("ability effect missing")?)?,
                    duration: duration(a, "duration")?,
                    range: if a.text() == Some("ConditionEffectAura") {
                        Some(positive(a, "range")?)
                    } else {
                        None
                    },
                },
                "ClearConditionEffectAura" => Ability::Clear {
                    effect: effect(a.attribute("effect").context("clear effect missing")?)?,
                    check: a.attribute("checkExistingEffect").map(effect).transpose()?,
                    range: positive(a, "range")?,
                },
                "RemoveNegativeConditions" => Ability::Cleanse {
                    range: positive(a, "range")?,
                },
                "PoisonGrenade" => Ability::Poison {
                    radius: positive(a, "radius")?,
                    damage: damage(a, "totalDamage")?,
                    duration: duration(a, "duration")?,
                },
                "VampireBlast" => Ability::Drain {
                    radius: positive(a, "radius")?,
                    damage: damage(a, "totalDamage")?,
                },
                "Trap" => Ability::Trap {
                    radius: positive(a, "radius")?,
                    damage: damage(a, "totalDamage")?,
                    effect: match a.attribute("condEffect") {
                        Some(name) => effect(name)?,
                        None => crate::condition::SLOWED,
                    },
                    duration: duration(a, "condDuration")?,
                },
                "StasisBlast" => Ability::Stasis {
                    duration: duration(a, "duration")?,
                },
                "Teleport" => Ability::Teleport {
                    distance: positive(a, "maxDistance")?,
                },
                "Decoy" => Ability::Decoy {
                    duration: duration(a, "duration")?,
                    angle: match a.attribute("angleOffset") {
                        Some(v) => {
                            let v: f32 = v.parse()?;
                            ensure!(v.is_finite(), "invalid decoy angle");
                            v.to_radians()
                        }
                        None => 0.,
                    },
                },
                "Lightning" => Ability::Lightning {
                    damage: damage(a, "totalDamage")?,
                    targets: {
                        let v: usize = a
                            .attribute("maxTargets")
                            .context("lightning maxTargets missing")?
                            .parse()?;
                        ensure!((1..=20).contains(&v), "invalid lightning targets");
                        v
                    },
                    condition: a
                        .attribute("condEffect")
                        .map(|name| {
                            Ok::<_, anyhow::Error>((effect(name)?, duration(a, "condDuration")?))
                        })
                        .transpose()?,
                    color: match a.attribute("color") {
                        Some(v) => {
                            u32::from_str_radix(
                                v.strip_prefix("0x").context("invalid lightning color")?,
                                16,
                            )? | 0xff000000
                        }
                        None => 0xffff0088,
                    },
                },
                "DazeBlast" => Ability::Daze {
                    radius: positive(a, "radius")?,
                    damage: damage(a, "totalDamage")?,
                },
                "Pet" if a.attribute("objectId") == Some("Spirit Prism Bomb") => {
                    Ability::SpiritBomb
                }
                name => bail!("unsupported class ability activation {name}"),
            })
        })
        .collect()
}
