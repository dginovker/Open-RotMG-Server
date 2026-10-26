use crate::encounter::{Motion, Profile};
use anyhow::{Result, ensure};

pub const ROOTS: [u16; 5] = [0x91f, 0x91b, 0x928, 0x675, 0x678];
pub const EVENTS: [u16; 3] = [0xd56, 0xd59, 0xd5f];
pub const EVENT_BOSSES: [u16; 7] = [0xd56, 0xd59, 0xd5f, 0xd50, 0xd54, 0xd61, 0xe37];
pub fn root(kind: u16) -> bool {
    ROOTS.contains(&kind) || EVENT_BOSSES.contains(&kind)
}

/// Character level windows of the quest monsters, as (kind, first level, last level).
///
/// The RotMG quest-monster reference lists each quest monster's quest level and the
/// character levels it is offered to; the pinned 2013 comparator carries the same windows.
/// Quest monsters without a documented window stay available at every character level.
pub const WINDOWS: &[(u16, u8, u8)] = &[
    // Wandering quest enemies: a quest level of five either side, as the reference states.
    (0x604, 1, 6),  // Scorpion Queen
    (0x953, 1, 7),  // Bandit Leader
    (0x617, 1, 8),  // Hobbit Mage
    (0x6c4, 1, 8),  // Giant Crab
    (0x609, 1, 8),  // Elf Wizard
    (0x61a, 1, 9),  // Undead Hobbit Mage
    (0x60d, 1, 9),  // Sandsman King
    (0x614, 1, 9),  // Goblin Mage
    (0x623, 1, 10), // Desert Werewolf
    (0x61f, 1, 10), // Dwarf King
    (0x955, 1, 10), // Swarm
    (0x950, 1, 11), // Shambling Sludge
    (0x952, 1, 11), // Great Lizard
    (0x947, 3, 20), // Wasp Queen
    (0x945, 3, 20), // Horned Drake
    // Setpiece bosses and Heroes of Oryx.
    (0x925, 1, 11),  // Deathmage
    (0xdc2, 6, 20),  // Great Coil Snake
    (0x91b, 5, 20),  // Lich
    (0x91c, 7, 20),  // Actual Lich
    (0x91f, 5, 20),  // Ent Ancient
    (0x920, 7, 20),  // Actual Ent Ancient
    (0x678, 8, 20),  // Oasis Giant
    (0x675, 9, 20),  // Phoenix Lord
    (0x928, 10, 20), // Ghost King
    (0x92d, 10, 20), // Actual Ghost King
    (0x67d, 10, 20), // Cyclops God
    (0x668, 15, 20), // Red Demon
    (0xd56, 15, 20), // Skull Shrine
    (0xd59, 15, 20), // Cube God
    (0xd5f, 15, 20), // Pentaract
    (0xd54, 15, 20), // Grand Sphinx
    (0xd50, 15, 20), // Lord of the Lost Lands
    (0xd61, 15, 20), // Hermit God
    (0xe37, 15, 20), // Ghost Ship
];
pub fn window(kind: u16) -> (u8, u8) {
    WINDOWS
        .iter()
        .find(|(k, _, _)| *k == kind)
        .map_or((1, 20), |(_, min, max)| (*min, *max))
}
pub const ROSTER: &[u16] = &[
    0x91b, 0x91c, 0x91d, 0x91e, 0x91f, 0x920, 0x921, 0x922, 0x923, 0x924, 0x928, 0x929, 0x92a,
    0x92b, 0x92c, 0x92d, 0x675, 0x676, 0x677, 0x6c1, 0x6c2, 0x65f, 0x660, 0x661, 0xd56, 0xd57,
    0xd58, 0xd59, 0xd5a, 0xd5b, 0xd5c, 0xd5d, 0xd5e, 0xd5f, 0xd60, 0xd54, 0xd55,
];
pub fn final_root(kind: u16) -> bool {
    matches!(
        kind,
        0x920
            | 0x91c
            | 0x92d
            | 0x6c1
            | 0x678
            | 0xd54
            | 0xd56
            | 0xd59
            | 0xd5f
            | crate::dungeon::trench::HERMIT
            | 0xe37
    )
}
pub fn profile(kind: u16) -> Option<Profile> {
    ROSTER.contains(&kind).then_some(Profile {
        motion: Motion::Wander,
        speed: 3.,
        interval: 50,
        range: match kind {
            0xd56 | 0xd59 => 30.,
            0xd57 | 0xd58 => 12.,
            0xd5a..=0xd5c => 10.,
            0xd5d => 9.,
            _ => 15.,
        },
    })
}
pub fn validate(content: &crate::content::Content, kind: u16) -> Result<()> {
    ensure!(
        ROSTER.contains(&kind),
        "unselected Realm quest object {kind:#x}"
    );
    let object = content.get(kind)?;
    ensure!(object.enemy && object.hp > 0, "invalid Realm quest enemy");
    if kind == 0xd54 {
        ensure!(
            object.per_realm_max == Some(1),
            "Grand Sphinx requires selected PerRealmMax=1"
        );
    }
    let indexes: &[u8] = match kind {
        0x6c2 | 0xd5e..=0xd60 => &[],
        0xd54 => &[0, 1, 2],
        0xd55 => &[0, 1],
        0x920 => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
        0x91b | 0x91c | 0x91d | 0x6c1 | 0xd59 | 0xd5a | 0xd5c => &[0, 1],
        _ => &[0],
    };
    for index in indexes {
        ensure!(
            object.projectiles.contains_key(index),
            "quest {kind:#x} lacks selected projectile {index}"
        );
    }
    Ok(())
}
pub fn ring(index: u8, count: usize, angle: f32) -> Vec<(u8, f32)> {
    (0..count)
        .map(|n| {
            (
                index,
                angle + n as f32 * std::f32::consts::TAU / count as f32,
            )
        })
        .collect()
}
pub fn fan(index: u8, count: usize, degrees: f32, heading: f32) -> Vec<(u8, f32)> {
    (0..count)
        .map(|n| {
            (
                index,
                heading + (n as f32 - (count - 1) as f32 / 2.) * degrees.to_radians(),
            )
        })
        .collect()
}
