use anyhow::{Result, bail};
use std::collections::BTreeMap;

pub const QUIET: u8 = 2;
pub const WEAK: u8 = 3;
pub const SLOWED: u8 = 4;
pub const SICK: u8 = 5;
pub const DAZED: u8 = 6;
pub const STUNNED: u8 = 7;
pub const INVISIBLE: u8 = 13;
pub const PARALYZED: u8 = 14;
pub const SPEEDY: u8 = 15;
pub const BLEEDING: u8 = 16;
pub const HEALING: u8 = 18;
pub const DAMAGING: u8 = 19;
pub const BERSERK: u8 = 20;
pub const STASIS: u8 = 22;
pub const STASIS_IMMUNE: u8 = 23;
pub const INVINCIBLE: u8 = 24;
pub const INVULNERABLE: u8 = 25;
pub const ARMORED: u8 = 26;
pub const ARMOR_BROKEN: u8 = 27;
pub const HEXED: u8 = 28;
pub const fn bit(index: u8) -> i32 {
    1 << (index - 1)
}
pub fn mask(timers: &BTreeMap<u8, (u64, u64)>, now: u64) -> i32 {
    timers
        .iter()
        .filter(|(_, (since, deadline))| *since <= now && *deadline > now)
        .fold(0, |mask, (index, _)| mask | bit(*index))
}
pub fn projectile_effect(name: &str) -> Result<u8> {
    Ok(match name {
        "Quiet" => QUIET,
        "Weak" => WEAK,
        "Slowed" => SLOWED,
        "Sick" => SICK,
        "Dazed" => DAZED,
        "Stunned" => STUNNED,
        "Blind" => 8,
        "Hallucinating" => 9,
        "Drunk" => 10,
        "Confused" => 11,
        "Paralyzed" => PARALYZED,
        "Bleeding" => BLEEDING,
        "Armor Broken" => ARMOR_BROKEN,
        _ => bail!("unsupported selected projectile condition {name}"),
    })
}
pub const fn protected(flags: i32) -> bool {
    flags & (bit(INVINCIBLE) | bit(INVULNERABLE) | bit(STASIS)) != 0
}
pub fn damage(raw: i32, defense: i32, flags: i32, piercing: bool) -> i32 {
    if protected(flags) || raw == 0 {
        return 0;
    }
    let defense = if piercing || flags & bit(ARMOR_BROKEN) != 0 {
        0
    } else if flags & bit(ARMORED) != 0 {
        defense * 2
    } else {
        defense
    };
    (raw - defense).max(raw * 15 / 100)
}
