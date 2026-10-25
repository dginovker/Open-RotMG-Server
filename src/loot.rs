use crate::{content::Content, dungeon::beach};
use anyhow::{Result, ensure};
use rand::{Rng, seq::SliceRandom};
#[derive(Clone, Copy)]
pub(crate) enum Pick {
    Item(u16),
    Tier(u8, &'static [i32]),
}
pub(crate) const WEAPONS: &[i32] = &[1, 2, 3, 8, 17];
pub(crate) const ABILITIES: &[i32] = &[4, 5, 11, 12, 13, 15, 16, 18, 19, 20, 21, 22, 23];
pub(crate) const ARMOR: &[i32] = &[6, 7, 14];
pub(crate) const RINGS: &[i32] = &[9];
pub(crate) fn fauna_private_table(kind: u16) -> &'static [(f64, Pick)] {
    use Pick::Tier;
    match kind {
        0x20d => &[(0.005, Pick::Item(0xa51))],
        0x952 => &[(0.03, Tier(2, RINGS))],
        0x949 | 0x950 => &[(0.03, Tier(6, WEAPONS)), (0.03, Tier(6, ARMOR))],
        0x945 => &[
            (0.05, Tier(6, WEAPONS)),
            (0.05, Tier(6, ARMOR)),
            (0.05, Tier(2, RINGS)),
            (0.03, Tier(3, RINGS)),
            (0.03, Tier(3, ABILITIES)),
        ],
        0x903 => &[(0.01, Tier(2, RINGS))],
        // Leading Trapped soul-bag entries keep first-success white odds: https://github.com/trapped/rotmg_svr/blob/593e490803fe0ad168ead408cacd5287d778acfe/wServer/logic/db/BehaviorDb.Crystal.cs#L674-L677
        0x941 => &[(0.02, Pick::Item(0xb3f)), (0.02, Pick::Item(0xa03))],
        0x924 => &[(0.001, Pick::Item(0xc07))],
        0x957 => &[
            (0.02, Tier(5, WEAPONS)),
            (0.01, Tier(6, WEAPONS)),
            (0.005, Tier(7, WEAPONS)),
            (0.02, Tier(5, ARMOR)),
            (0.01, Tier(6, ARMOR)),
            (0.005, Tier(7, ARMOR)),
            (0.015, Tier(3, RINGS)),
            (0.015, Tier(3, ABILITIES)),
        ],
        _ => &[],
    }
}
fn table(kind: u16) -> &'static [(f64, Pick)] {
    use Pick::{Item, Tier};
    match kind {
        beach::BOSS => &[
            (0.1, Item(beach::DRINKS[0])),
            (0.1, Item(beach::DRINKS[1])),
            (0.1, Item(beach::DRINKS[2])),
            (0.1, Item(beach::DRINKS[3])),
        ],
        crate::dungeon::beach::BUM => &[(1., Item(0xc2e))],
        crate::dungeon::manor::BOSS | crate::dungeon::manor::COFFIN => &[
            (0.5, Item(0xc21)),
            (0.01, Item(0xc1d)),
            (0.01, Item(0xc1f)),
            (0.05, Item(0xc20)),
            (0.005, Item(0xc09)),
            (0.01, Item(0xc22)),
            (0.005, Item(0x722)),
            (0.03, Tier(9, WEAPONS)),
            (0.03, Tier(4, ABILITIES)),
        ],
        crate::dungeon::manor::KAGE => &[
            (0.1, Tier(6, WEAPONS)),
            (0.1, Tier(7, WEAPONS)),
            (0.05, Tier(8, WEAPONS)),
            (0.1, Tier(6, ARMOR)),
            (0.1, Tier(7, ARMOR)),
            (0.05, Tier(8, ARMOR)),
            (0.1, Tier(3, ABILITIES)),
            (0.05, Tier(4, ABILITIES)),
            (0.1, Tier(3, RINGS)),
            (0.05, Tier(4, RINGS)),
            (0.1, Item(0xa22)),
            (0.1, Item(0xa23)),
            (0.005, Item(0x722)),
        ],
        0x1728 => &[(0.01, Item(0xc22)), (0.03, Tier(8, WEAPONS))],
        0x1729 => &[(0.05, Tier(6, WEAPONS))],
        kind if crate::dungeon::manor::ROSTER.contains(&kind)
            || kind == crate::dungeon::manor::BOOKCASE =>
        {
            &[]
        }
        crate::dungeon::davy::BOSS => &[
            (0.2, Tier(3, RINGS)),
            (0.2, Tier(7, ARMOR)),
            (0.2, Tier(8, WEAPONS)),
            (0.1, Tier(4, ABILITIES)),
            (0.1, Tier(8, ARMOR)),
            (0.05, Tier(4, RINGS)),
            (0.03, Tier(9, ARMOR)),
            (0.03, Tier(5, ABILITIES)),
            (0.03, Tier(9, WEAPONS)),
            (0.02, Tier(10, ARMOR)),
            (0.02, Tier(10, WEAPONS)),
            (0.01, Tier(11, ARMOR)),
            (0.01, Tier(11, WEAPONS)),
            (0.01, Tier(5, RINGS)),
            (0.01, Item(0xc29)),
            (0.01, Item(0xc28)),
            (0.01, Item(0xc2a)),
            (0.01, Item(0xc27)),
            (0.01, Item(0x722)),
            (0.5, Item(0xa35)),
        ],
        0xe31 | 0xe3a | 0xe42 => &[
            (0.03, Item(0xa22)),
            (0.03, Item(0xa23)),
            (0.01, Item(0xc25)),
        ],
        0xe43 => &[
            (0.05, Tier(7, WEAPONS)),
            (0.05, Tier(7, ARMOR)),
            (0.02, Tier(3, RINGS)),
            (0.03, Item(0xa23)),
            (0.01, Item(0xc25)),
        ],
        0xe44 => &[
            (0.05, Tier(7, WEAPONS)),
            (0.03, Tier(8, WEAPONS)),
            (0.05, Tier(7, ARMOR)),
            (0.02, Tier(3, RINGS)),
            (0.03, Item(0xa23)),
        ],
        0xe45 => &[
            (0.03, Tier(4, RINGS)),
            (0.05, Tier(8, WEAPONS)),
            (0.05, Tier(7, ARMOR)),
            (0.03, Tier(3, ABILITIES)),
            (0.03, Tier(3, RINGS)),
            (0.03, Item(0xa23)),
            (0.01, Item(0xc25)),
        ],
        kind if crate::dungeon::davy::ROSTER.contains(&kind) || kind == 0x182e => &[],
        // Trapped's ordered boss pool is reconstruction evidence, not official 123.5.1 RNG.
        crate::dungeon::tomb::BES => &[
            (0.009, Item(0xba2)),
            (0.005, Item(0xc1e)),
            (0.005, Item(crate::endgame::INCANTATION)),
            (1., Item(0xae9)),
        ],
        crate::dungeon::tomb::GEB => &[
            (0.05, Item(0xba1)),
            (0.015, Item(crate::endgame::INCANTATION)),
            (1., Item(0xae9)),
        ],
        crate::dungeon::tomb::NUT => &[
            (0.009, Item(0xba0)),
            (0.005, Item(crate::endgame::INCANTATION)),
            (1., Item(0xae9)),
        ],
        crate::dungeon::tomb::SARCOPHAGUS => &[
            (0.15, Item(0xa22)),
            (0.15, Item(0xa23)),
            (0.15, Item(0xb0e)),
            (0.15, Item(0xb0d)),
            (0.15, Item(0xb0c)),
        ],
        crate::dungeon::tomb::TREASURE => &[
            (0.3, Item(0xa1f)),
            (0.3, Item(0xa21)),
            (0.3, Item(0xa20)),
            (0.08, Tier(9, ARMOR)),
            (0.08, Tier(9, WEAPONS)),
            (0.06, Tier(10, ARMOR)),
            (0.06, Tier(10, WEAPONS)),
            (0.08, Tier(4, RINGS)),
        ],
        0xd2a | 0xd2b => &[(0.2, Item(0xa22)), (0.2, Item(0xa23))],
        0xd2d..=0xd30 => &[(0.3, Item(0xa22)), (0.3, Item(0xa23))],
        0xd10..=0xd1b | 0xd3a..=0xd3c => &[(0.01, Item(0xa22))],
        kind if crate::dungeon::tomb::contains(kind) => &[],
        0x6c4 => &[
            (0.10, Tier(2, WEAPONS)),
            (0.10, Tier(2, ARMOR)),
            (0.10, Tier(1, RINGS)),
            (0.10, Tier(1, ABILITIES)),
            (0.03, Item(0xa22)),
            (0.02, Item(0xa23)),
        ],
        crate::dungeon::lab::BOSS => &[
            (0.005, Item(0xc30)),
            (0.005, Item(0xc32)),
            (0.01, Item(0xc31)),
            (0.005, Item(crate::endgame::INCANTATION)),
            (0.2, Tier(8, WEAPONS)),
            (0.15, Tier(9, WEAPONS)),
            (0.1, Tier(10, WEAPONS)),
            (0.05, Tier(11, WEAPONS)),
            (0.2, Tier(8, ARMOR)),
            (0.15, Tier(9, ARMOR)),
            (0.1, Tier(10, ARMOR)),
            (0.05, Tier(11, ARMOR)),
            (0.15, Tier(4, ABILITIES)),
            (0.1, Tier(5, ABILITIES)),
        ],
        kind if crate::dungeon::lab::ROSTER.contains(&kind) => &[],
        crate::dungeon::jungle::BOSS => &[
            (1., Item(0xc1a)),
            (0.4, Item(0xc17)),
            (0.25, Item(0xc15)),
            (0.3, Item(0xc16)),
            (0.4, Item(0xc14)),
            (0.003, Item(0x722)),
        ],
        crate::dungeon::jungle::SHAMAN
        | crate::dungeon::jungle::WARRIOR
        | crate::dungeon::jungle::HUNTER => &[(0.1, Item(0xc1a))],
        kind if crate::dungeon::jungle::ROSTER.contains(&kind) => &[],
        crate::dungeon::spider::BOSS => {
            &[(0.2, Item(0xa16)), (0.2, Item(0x708)), (0.2, Item(0x709))]
        }
        0x21d..=0x220 => &[(0.2, Item(0x707))],
        0x21b | 0x21c | 0x937..=0x93f => &[],
        crate::dungeon::trench::HERMIT => {
            &[(0.1, Item(0xa34)), (0.1, Item(0xa4c)), (0.004, Item(0xc08))]
        }
        // BehaviorDb.Sphinx.cs / Oasis.cs, realm-src@1855c017 (GPL-3.0; data/REALM-SRC-LICENSE), NR-Core and Skilly.
        0xd54 => &[(0.1, Item(0xa34)), (0.1, Item(0xa35)), (0.004, Item(0xc08))],
        0xd62 => &[(0.1, Item(0xa22)), (0.1, Item(0xa23))],
        0x679 | 0x67c => &[(0.05, Item(0xa23))],
        0x67a | 0x67b => &[(0.05, Item(0xa22))],
        0x1705 | crate::dungeon::trench::THESSAL => &[
            (0.3, Item(0xc12)),
            (0.01, Item(0xc10)),
            (0.03, Item(0xc1c)),
            (0.02, Item(0x722)),
            (0.04, Item(0xc18)),
            (0.04, Item(0xc13)),
        ],
        0xd63..=0xd65 | 0x1700..=0x1704 | 0x1707..=0x170e => &[],
        0x623 => &[
            (0.1, Tier(3, WEAPONS)),
            (0.05, Tier(4, WEAPONS)),
            (0.1, Tier(3, ARMOR)),
            (0.05, Tier(4, ARMOR)),
            (0.05, Tier(1, ABILITIES)),
            (0.05, Tier(1, RINGS)),
        ],
        // Current reconstruction from Trapped's older Midland table, not 123.5.1 rate fidelity.
        0x945 => &[
            (0.10, Tier(5, WEAPONS)),
            (0.10, Tier(5, ARMOR)),
            (0.03, Item(0xa22)),
            (0.02, Item(0xa23)),
            (0.05, Tier(2, ABILITIES)),
        ],
        0x955 => &[
            (0.15, Tier(3, WEAPONS)),
            (0.15, Tier(3, ARMOR)),
            (0.15, Tier(1, ABILITIES)),
            (0.15, Tier(1, RINGS)),
        ],
        // Trapped 593e490 engineering tables, not authenticated 123.5.1 probabilities.
        0x952 => &[(0.05, Tier(5, WEAPONS)), (0.05, Tier(5, ARMOR))],
        0x949 | 0x950 => &[
            (0.10, Tier(3, WEAPONS)),
            (0.05, Tier(4, WEAPONS)),
            (0.10, Tier(3, ARMOR)),
            (0.05, Tier(4, ARMOR)),
        ],
        0x637 => &[
            (0.10, Tier(3, WEAPONS)),
            (0.05, Tier(4, WEAPONS)),
            (0.03, Tier(5, WEAPONS)),
            (0.10, Tier(3, ARMOR)),
            (0.05, Tier(4, ARMOR)),
            (0.03, Tier(5, ARMOR)),
            (0.05, Tier(1, RINGS)),
            (0.05, Tier(1, ABILITIES)),
            (0.03, Item(0xa22)),
            (0.02, Item(0xa23)),
        ],
        0x205 | 0x20d | 0x20f | 0x202 | 0x635 | 0x636 | 0x639 | 0x6c5 | 0x7f22 | 0x6c7 | 0x6c9
        | 0x6d1 | 0x951 => &[(0.03, Item(0xa22)), (0.02, Item(0xa23))],
        0x903 | 0x209 => &[(0.04, Item(0xa22)), (0.01, Item(0xa23))],
        0x904 => &[(0.03, Item(0xa23)), (0.01, Tier(5, WEAPONS))],
        0x204 => &[
            (0.01, Tier(2, ARMOR)),
            (0.03, Item(0xa22)),
            (0.02, Item(0xa23)),
        ],
        // GPL-3.0 realm-src@1855c017, BehaviorDb.{LotLL,GhostShip}.cs; NR-Core and Skilly.
        0xd50 => &[
            (0.15, Tier(8, WEAPONS)),
            (0.1, Tier(9, WEAPONS)),
            (0.07, Tier(10, WEAPONS)),
            (0.05, Tier(11, WEAPONS)),
            (0.15, Tier(4, ABILITIES)),
            (0.07, Tier(5, ABILITIES)),
            (0.2, Tier(8, ARMOR)),
            (0.15, Tier(9, ARMOR)),
            (0.1, Tier(10, ARMOR)),
            (0.07, Tier(11, ARMOR)),
            (0.04, Tier(12, ARMOR)),
            (0.15, Tier(3, RINGS)),
            (0.07, Tier(4, RINGS)),
            (0.03, Tier(5, RINGS)),
            (0.1, Item(0xa20)),
            (0.1, Item(0xa1f)),
            (0.1, Item(0xa34)),
            (0.1, Item(0xa35)),
            (0.1, Item(0xa21)),
            (0.1, Item(0xa4c)),
            (0.004, Item(0xc0f)),
        ],
        0xd52 | 0xd53 => &[(0.1, Item(0xa22)), (0.1, Item(0xa23))],
        0xe37 => &[
            (0.1, Item(0xa35)),
            (0.12, Tier(8, WEAPONS)),
            (0.1, Tier(9, WEAPONS)),
            (0.12, Tier(8, ARMOR)),
            (0.1, Tier(9, ARMOR)),
            (0.12, Tier(3, ABILITIES)),
            (0.1, Tier(4, ABILITIES)),
            (0.07, Tier(4, RINGS)),
            (0.06, Tier(10, WEAPONS)),
            (0.04, Tier(11, WEAPONS)),
            (0.06, Tier(10, ARMOR)),
            (0.04, Tier(11, ARMOR)),
            (0.04, Tier(5, ABILITIES)),
            (0.04, Tier(5, RINGS)),
            (0.001, Item(crate::endgame::INCANTATION)),
        ],
        // Behavior coverage alone does not establish new drop probabilities.
        kind if crate::encounter::fauna::ROSTER.contains(&kind)
            || crate::encounter::landmarks::ROSTER.contains(&kind) =>
        {
            &[]
        }
        // Tombstone rewards are a no-drop reconstruction; selected XML establishes zero XP, not loot.
        0x181f => &[],
        0x122 | 0x162 | 0x163 | 0xd57 | 0xd58 | 0xd5b..=0xd5f => &[],
        0xd5a => &[(0.05, Item(0xa3b))],
        0xd56 | 0xd59 | 0xd60 => &[
            (0.15, Tier(8, WEAPONS)),
            (0.1, Tier(9, WEAPONS)),
            (0.07, Tier(10, WEAPONS)),
            (0.05, Tier(11, WEAPONS)),
            (0.15, Tier(4, ABILITIES)),
            (0.07, Tier(5, ABILITIES)),
            (0.2, Tier(8, ARMOR)),
            (0.15, Tier(9, ARMOR)),
            (0.1, Tier(10, ARMOR)),
            (0.07, Tier(11, ARMOR)),
            (0.04, Tier(12, ARMOR)),
            (0.15, Tier(3, RINGS)),
            (0.07, Tier(4, RINGS)),
            (0.03, Tier(5, RINGS)),
            (0.1, Item(0xa1f)),
            (0.1, Item(0xa20)),
            (0.1, Item(0xa21)),
            (0.1, Item(0xa34)),
            (0.1, Item(0xa35)),
            (0.1, Item(0xa4c)),
        ],
        0x91b | 0x91f | 0x928 | 0x675 | 0x6c2 | 0x923 => &[],
        0x676 => &[(0.05, Item(0xa23))],
        0x677 => &[(0.05, Item(0xa22))],
        0x65f | 0x660 | 0x661 | 0x922 => &[(0.02, Item(0xa23))],
        0x921 | 0x91d | 0x91e | 0x929 | 0x92a | 0x92b | 0x92c | 0x924 => &[(0.03, Item(0xa23))],
        0x6c1 => &[(0.0001, Item(crate::endgame::INCANTATION))],
        0x920 => &[
            (0.3, Tier(6, WEAPONS)),
            (0.1, Tier(7, WEAPONS)),
            (0.3, Tier(6, ARMOR)),
            (0.1, Tier(7, ARMOR)),
            (0.95, Tier(1, ABILITIES)),
            (0.25, Tier(2, ABILITIES)),
            (0.05, Tier(3, ABILITIES)),
            (0.15, Tier(2, RINGS)),
            (0.04, Tier(3, RINGS)),
            (0.7, Item(0xa22)),
            (0.7, Item(0xa23)),
        ],
        0x91c => &[
            (0.3, Tier(5, WEAPONS)),
            (0.2, Tier(6, WEAPONS)),
            (0.05, Tier(7, WEAPONS)),
            (0.3, Tier(5, ARMOR)),
            (0.2, Tier(6, ARMOR)),
            (0.05, Tier(7, ARMOR)),
            (0.9, Tier(1, ABILITIES)),
            (0.15, Tier(2, ABILITIES)),
            (0.02, Tier(3, ABILITIES)),
            (0.11, Tier(2, RINGS)),
            (0.01, Tier(3, RINGS)),
            (0.4, Item(0xa22)),
            (0.4, Item(0xa23)),
        ],
        0x92d => &[
            (0.3, Tier(7, WEAPONS)),
            (0.1, Tier(8, WEAPONS)),
            (0.3, Tier(7, ARMOR)),
            (0.1, Tier(8, ARMOR)),
            (0.7, Tier(2, ABILITIES)),
            (0.16, Tier(3, ABILITIES)),
            (0.02, Tier(4, ABILITIES)),
            (0.25, Tier(2, RINGS)),
            (0.08, Tier(3, RINGS)),
            (0.7, Item(0xa22)),
            (0.7, Item(0xa23)),
        ],
        0x600 => &[(0.2, Tier(1, WEAPONS)), (0.03, Item(0xa22))],
        0x601 => &[(0.2, Tier(1, ARMOR)), (0.03, Item(0xa22))],
        0x602 | 0x604 => &[(0.03, Item(0xa22)), (0.02, Item(0xa23))],
        0x603 | 0x954 => &[],
        0x6d2..=0x6d7 => &[],
        0x624 => &[(0.03, Tier(5, WEAPONS)), (0.03, Item(0xa23))],
        0x625 => &[(0.03, Tier(2, ABILITIES)), (0.03, Item(0xa23))],
        0x626 => &[(0.03, Tier(5, ARMOR)), (0.03, Item(0xa23))],
        0x62d | 0x631 => &[(0.03, Tier(5, WEAPONS)), (0.03, Item(0xa22))],
        0x62e => &[(0.03, Tier(5, ARMOR)), (0.03, Item(0xa22))],
        0x62f => &[(0.03, Item(0xa22)), (0.01, Tier(2, RINGS))],
        0x629 | 0x62b => &[(0.03, Item(0xa22))],
        0x62a | 0x647 | 0x648 => &[(0.03, Item(0xa23))],
        0x62c => &[
            (0.2, Tier(3, WEAPONS)),
            (0.1, Tier(4, WEAPONS)),
            (0.05, Tier(5, WEAPONS)),
            (0.2, Tier(3, ARMOR)),
            (0.1, Tier(4, ARMOR)),
            (0.05, Tier(5, ARMOR)),
            (0.05, Tier(2, ABILITIES)),
            (0.03, Item(0xa22)),
            (0.01, Tier(2, RINGS)),
        ],
        0x6c6 => &[
            (0.1, Tier(4, WEAPONS)),
            (0.05, Tier(3, WEAPONS)),
            (0.1, Tier(4, ARMOR)),
            (0.05, Tier(3, ARMOR)),
            (0.03, Item(0xa22)),
            (0.02, Item(0xa23)),
        ],
        0x900 => &[
            (0.04, Item(0xa22)),
            (0.01, Item(0xa23)),
            (0.01, Tier(3, ARMOR)),
        ],
        0x901 => &[
            (0.04, Item(0xa22)),
            (0.01, Item(0xa23)),
            (0.01, Tier(4, ABILITIES)),
        ],
        0x902 => &[
            (0.04, Item(0xa22)),
            (0.01, Item(0xa23)),
            (0.01, Tier(2, RINGS)),
        ],
        0x612 | 0x613 | 0x618 => &[(0.03, Item(0xa22))],
        0x619 => &[(0.03, Item(0xa23))],
        0x953 => &[
            (0.2, Tier(1, WEAPONS)),
            (0.1, Tier(2, WEAPONS)),
            (0.2, Tier(1, ARMOR)),
            (0.03, Item(0xa22)),
            (0.02, Item(0xa23)),
        ],
        0x614 => &[
            (0.2, Tier(3, WEAPONS)),
            (0.2, Tier(3, ARMOR)),
            (0.2, Tier(1, RINGS)),
            (0.2, Tier(1, ABILITIES)),
            (0.03, Item(0xa22)),
            (0.02, Item(0xa23)),
        ],
        0x61a => &[
            (0.2, Tier(2, WEAPONS)),
            (0.1, Tier(3, WEAPONS)),
            (0.2, Tier(2, ARMOR)),
            (0.1, Tier(3, ARMOR)),
            (0.1, Tier(1, RINGS)),
            (0.1, Tier(1, ABILITIES)),
            (0.03, Item(0xa23)),
        ],
        0x60e..=0x610 => &[(0.03, Item(0xa22)), (0.02, Item(0xa23))],
        0x687..=0x689 => &[(0.01, Item(0xa22))],
        0x68a..=0x68e => &[(0.1, Tier(1, ABILITIES))],
        0x68f => &[(0.1, Tier(1, WEAPONS))],
        0x683..=0x686 => &[
            (0.3, Tier(1, ARMOR)),
            (0.3, Tier(1, RINGS)),
            (0.2, Tier(2, WEAPONS)),
            (0.1, Tier(3, WEAPONS)),
            (0.01, Item(0xa50)),
        ],
        0x927 => &[
            (0.01, Item(0xa50)),
            (0.15, Tier(1, ARMOR)),
            (0.5, Tier(1, RINGS)),
            (0.2, Tier(2, ARMOR)),
            (0.2, Tier(2, WEAPONS)),
            (0.25, Tier(3, ARMOR)),
            (0.3, Tier(3, WEAPONS)),
            (0.3, Tier(4, ARMOR)),
            (0.4, Tier(4, WEAPONS)),
        ],
        0x172
        | 0x173
        | 0x175..=0x179
        | 0x653
        | 0xd07..=0xd0a
        | 0xda2
        | 0xda3
        | 0x918
        | 0x919
        | crate::dungeon::snake::BUTTON
        | crate::dungeon::snake::SPAWNER
        | crate::dungeon::snake::DART => &[],
        0xdaa..=0xdaf => &[(0.25, Item(0xa22)), (0.25, Item(0xa23))],
        0x605..=0x607
        | 0x60a
        | 0x60c
        | 0x615
        | 0x616
        | 0x61c..=0x61e
        | 0x63a..=0x63c
        | 0x63f
        | 0x642..=0x644
        | 0x64a..=0x64d
        | 0xd91..=0xd94
        | 0xd96
        | 0xd97
        | 0xd9b..=0xd9f
        | 0xda4..=0xda6
        | 0xdb0..=0xdb2 => &[(0.05, Item(0xa22)), (0.03, Item(0xa23))],
        0x60d => &[
            (0.10, Tier(3, WEAPONS)),
            (0.10, Tier(3, ARMOR)),
            (0.10, Tier(1, RINGS)),
            (0.10, Tier(1, ABILITIES)),
            (0.03, Item(0xa22)),
        ],
        0x609 | 0x617 => &[
            (0.2, Tier(2, WEAPONS)),
            (0.2, Tier(2, ARMOR)),
            (0.2, Tier(1, RINGS)),
            (0.2, Tier(1, ABILITIES)),
            (0.03, Item(0xa22)),
            (0.02, Item(0xa23)),
        ],
        0x61f | 0x632 | 0x633 | 0x63e => &[
            (0.1, Tier(3, WEAPONS)),
            (0.05, Tier(4, WEAPONS)),
            (0.1, Tier(3, ARMOR)),
            (0.05, Tier(4, ARMOR)),
            (0.05, Tier(1, RINGS)),
            (0.05, Tier(1, ABILITIES)),
        ],
        0x640 | 0x645 | 0x646 | 0x649 | 0x64f | 0x650 | 0x905 => &[
            (0.16, Tier(5, WEAPONS)),
            (0.08, Tier(6, WEAPONS)),
            (0.16, Tier(4, ARMOR)),
            (0.08, Tier(5, ARMOR)),
            (0.04, Tier(6, ARMOR)),
            (0.05, Tier(3, RINGS)),
            (0.03, Tier(3, ABILITIES)),
        ],
        0x906 | 0x907 => &[
            (0.04, Item(0xa22)),
            (0.01, Item(0xa23)),
            (0.01, Tier(6, WEAPONS)),
        ],
        0x651..=0x659 | 0xd05 | 0x91a | 0x6d8 => &[
            (0.02, Tier(6, WEAPONS)),
            (0.01, Tier(7, WEAPONS)),
            (0.005, Tier(8, WEAPONS)),
            (0.02, Tier(6, ARMOR)),
            (0.01, Tier(7, ARMOR)),
            (0.005, Tier(8, ARMOR)),
            (0.015, Tier(3, RINGS)),
            (0.02, Tier(4, ABILITIES)),
        ],
        0x223..=0x229 => &[
            (0.1, Item(0xa3f)),
            (0.05, Tier(3, RINGS)),
            (0.05, Tier(6, WEAPONS)),
        ],
        0xd00..=0xd04 => &[(0.02, Tier(5, WEAPONS)), (0.05, Item(0xa23))],
        0xd95 | 0xd98..=0xd9a => &[
            (0.2, Tier(5, ARMOR)),
            (0.2, Tier(6, WEAPONS)),
            (0.1, Tier(7, WEAPONS)),
            (0.1, Tier(6, ARMOR)),
            (0.1, Tier(3, ABILITIES)),
            (0.1, Tier(3, RINGS)),
        ],
        0x917 | crate::dungeon::snake::GUARD => &[
            (1., Item(0xa21)),
            (0.05, Item(crate::endgame::INCANTATION)),
            (0.05, Item(0xc04)),
            (0.05, Item(0xa3e)),
            (0.05, Item(0xa40)),
            (0.05, Item(0xa41)),
            (0.2, Tier(9, WEAPONS)),
            (0.3, Tier(8, ARMOR)),
        ],
        0xd06 => &[
            (1., Item(0xa4c)),
            (0.01, Item(0xa4b)),
            (0.01, Item(0xc03)),
            (0.01, Item(0xa5a)),
            (0.15, Tier(7, ARMOR)),
            (0.124, Tier(4, ABILITIES)),
            (0.11, Tier(3, RINGS)),
        ],
        0xd90 => &[
            (1., Item(0xa35)),
            (0.005, Item(crate::endgame::INCANTATION)),
            (0.009, Item(0xc02)),
            (0.3, Tier(7, WEAPONS)),
            (0.2, Tier(8, WEAPONS)),
            (0.2, Tier(4, ABILITIES)),
            (0.3, Tier(3, RINGS)),
        ],
        0x66d => &[
            (0.03, Item(0xa22)),
            (0.02, Item(0xa23)),
            (0.01, Item(0xa59)),
            (0.01, Item(0xaa7)),
        ],
        0x66e => &[
            (0.03, Item(0xa22)),
            (0.02, Item(0xa23)),
            (0.01, Item(0xa1c)),
            (0.01, Item(0xa12)),
        ],
        0x66f => &[
            (0.03, Item(0xa22)),
            (0.02, Item(0xa23)),
            (0.01, Item(0xa3b)),
            (0.01, Item(0xa0a)),
        ],
        0x670 => &[
            (0.03, Item(0xa22)),
            (0.02, Item(0xa23)),
            (0.01, Item(0xa2f)),
            (0.01, Item(0xa3d)),
            (0.01, Item(0xa9d)),
            (0.01, Item(0xa5f)),
            (0.01, Item(0xaae)),
            (0.01, Item(0xab5)),
        ],
        0x671 => &[
            (0.03, Item(0xa22)),
            (0.02, Item(0xa23)),
            (0.01, Item(0xa17)),
            (0.01, Item(0xa6a)),
        ],
        0x672 => &[
            (0.03, Item(0xa22)),
            (0.02, Item(0xa23)),
            (0.01, Item(0xa3c)),
            (0.01, Item(0xa4e)),
            (0.01, Item(0xa64)),
            (0.01, Item(0xa32)),
        ],
        0x908 => &[(0.04, Item(0xa22)), (0.06, Item(0xa23))],
        0x90b => &[(0.1, Item(0xa22)), (0.1, Item(0xa23))],
        0x909 => &[],
        // Preserve #244's paired private-potion contract; historical allocation is separate: https://github.com/OpenRotMG/Open-RotMG-Server/issues/244
        crate::dungeon::abyss::MALPHAS => &[
            (1., Item(0xa34)),
            (1., Item(0xa20)),
            (0.005, Item(crate::endgame::INCANTATION)),
            (0.001, Item(0xc01)),
            (0.8, Tier(7, WEAPONS)),
            (0.11, Tier(9, WEAPONS)),
            (0.05, Tier(10, WEAPONS)),
            (0.8, Tier(6, ARMOR)),
            (0.4, Tier(8, ARMOR)),
            (0.11, Tier(9, ARMOR)),
            (0.5, Tier(3, ABILITIES)),
            (0.06, Tier(4, ABILITIES)),
            (0.5, Tier(3, RINGS)),
            (0.06, Tier(4, RINGS)),
        ],
        crate::dungeon::abyss::IDOL => &[
            (1., Item(0xa34)),
            // Restore Trapped's DEF candidate using our VIT guarantee, not its ordered weights: https://github.com/trapped/rotmg_svr/blob/593e490803fe0ad168ead408cacd5287d778acfe/wServer/logic/db/BehaviorDb.Abyss.cs#L224-L235
            (1., Item(0xa20)),
            (0.005, Item(crate::endgame::INCANTATION)),
            (0.001, Item(0xc01)),
            (0.8, Tier(7, WEAPONS)),
            (0.4, Tier(8, WEAPONS)),
            (0.11, Tier(9, WEAPONS)),
            (0.05, Tier(10, WEAPONS)),
            (0.8, Tier(7, ARMOR)),
            (0.4, Tier(8, ARMOR)),
            (0.11, Tier(9, ARMOR)),
            (0.05, Tier(10, ARMOR)),
            (0.5, Tier(3, ABILITIES)),
            (0.06, Tier(4, ABILITIES)),
            (0.5, Tier(3, RINGS)),
            (0.06, Tier(4, RINGS)),
        ],
        kind if crate::endgame::ROSTER.contains(&kind) => crate::endgame::loot(kind),
        _ => panic!("loot requested without encounter profile"),
    }
}
fn candidates(content: &Content, pick: Pick) -> Vec<u16> {
    match pick {
        Pick::Item(kind) => vec![kind],
        Pick::Tier(tier, slots) => content
            .objects
            .values()
            .filter(|o| o.tier == Some(tier) && slots.contains(&o.slot_type))
            .map(|o| o.kind)
            .collect(),
    }
}
pub fn collectibles(content: &Content) -> std::collections::BTreeSet<u16> {
    content
        .objects
        .keys()
        .copied()
        .filter(|kind| crate::encounter::profile(*kind).is_some())
        .flat_map(|kind| {
            table(kind)
                .iter()
                .chain(fauna_private_table(kind))
                .flat_map(|(_, pick)| candidates(content, *pick))
                .chain(bonus(kind).iter().map(|(_, item)| *item))
        })
        .chain([0x678, 0x67d, 0x668].into_iter().flat_map(|kind| {
            chest_table(kind)
                .into_iter()
                .flat_map(|(_, pick)| candidates(content, pick))
        }))
        .collect()
}
fn bonus(kind: u16) -> &'static [(f64, u16)] {
    match kind {
        0xd56 => &[
            (0.001, 0xc0b),
            (0.01, 0x121e),
            (0.01, 0x131e),
            (0.01, 0x1241),
            (0.01, 0x1341),
            (0.01, 0x120e),
            (0.01, 0x130e),
        ],
        0xd59 => &[(0.001, 0xc0a)],
        0xd60 => &[(0.004, 0xc06)],
        0x1705 => &[(0.04, 0xaea)],
        0x651 | 0x652 | 0x657 | 0x905..=0x907 => &[(0.015, 0xa1f)],
        0x654 | 0x659 | 0x91a => &[(0.015, 0xa21)],
        0x655 | 0x656 | 0x658 | 0x6d8 => &[(0.015, 0xa20)],
        _ => &[],
    }
}
pub fn validate(content: &Content, kind: u16) -> Result<()> {
    for (_, item) in bonus(kind) {
        let item = content.get(*item)?;
        ensure!(
            matches!(item.class.as_str(), "Equipment" | "Dye"),
            "invalid bonus loot {:#x}: {}",
            item.kind,
            item.class
        );
        if matches!(item.kind, 0xa1f | 0xa20 | 0xa21 | 0xaea) {
            ensure!(
                item.stat_increase.is_some(),
                "bonus potion is not a selected stat increase"
            );
        }
    }
    for (_, pick) in table(kind).iter().chain(fauna_private_table(kind)) {
        let items = candidates(content, *pick);
        ensure!(
            !items.is_empty(),
            "encounter {kind:#x} has an empty selected-XML loot tier"
        );
        for item in items {
            let o = content.get(item)?;
            ensure!(
                o.class == "Equipment" && o.bag_type <= 4,
                "invalid encounter loot {item:#x}"
            );
        }
    }
    Ok(())
}
pub fn roll(content: &Content, kind: u16, rng: &mut impl Rng) -> Option<u16> {
    roll_table(content, table(kind), rng)
}
fn roll_table(content: &Content, picks: &[(f64, Pick)], rng: &mut impl Rng) -> Option<u16> {
    for (chance, pick) in picks {
        if rng.gen_bool(*chance) {
            return Some(
                *candidates(content, *pick)
                    .choose(rng)
                    .expect("validated nonempty loot candidates"),
            );
        }
    }
    None
}
pub fn rewards(content: &Content, kind: u16, rng: &mut impl Rng) -> Vec<u16> {
    let mut items = if kind == crate::dungeon::tomb::TREASURE
        || crate::quest::final_root(kind)
        || matches!(kind, crate::dungeon::trench::HERMIT | 0xd50 | 0xd52 | 0xd53)
        || matches!(kind, 0x66d..=0x672 | 0x908 | 0x90b)
        || matches!(kind, 0xd60 | 0x1705 | crate::dungeon::trench::THESSAL)
        || matches!(
            kind,
            0x917
                | crate::dungeon::snake::GUARD
                | 0xd06
                | 0xd90
                | 0xd78
                | 0xd79
                | crate::dungeon::spider::BOSS
                | crate::dungeon::lab::BOSS
                | crate::dungeon::jungle::BOSS
                | crate::dungeon::beach::BOSS
                | crate::dungeon::manor::BOSS
                | crate::dungeon::manor::COFFIN
                | crate::dungeon::manor::KAGE
                | crate::dungeon::davy::BOSS
                | crate::dungeon::abyss::MALPHAS
                | crate::dungeon::abyss::IDOL
                | crate::endgame::ORYX
                | crate::endgame::ORYX2
        ) {
        let mut items = Vec::new();
        for (chance, pick) in table(kind) {
            if rng.gen_bool(*chance) {
                items.push(
                    *candidates(content, *pick)
                        .choose(rng)
                        .expect("validated loot candidates"),
                );
            }
        }
        items
    } else {
        roll(content, kind, rng).into_iter().collect()
    };
    for &(chance, item) in bonus(kind) {
        if rng.gen_bool(chance) {
            items.push(item);
        }
    }
    items
}
pub fn fauna_private_rewards(
    content: &Content,
    kind: u16,
    contributors_by_damage: &[(i32, i32)],
    rng: &mut impl Rng,
) -> Vec<(i32, u16)> {
    let multiplier = match kind {
        0x20d | 0x952 | 0x903 | 0x924 => 1,
        0x941 => 8,
        0x949 | 0x950 | 0x945 | 0x957 => 2,
        _ => unreachable!("unsupported dual-channel fauna kind {kind:#x}"),
    };
    let owners: Vec<_> = contributors_by_damage
        .iter()
        .filter(|(_, damage)| *damage > 1)
        .map(|(account, _)| *account)
        .collect();
    let mut rewards = Vec::new();
    let mut successes = 0;
    for _ in 0..owners.len() * multiplier {
        if let Some(item) = roll_table(content, fauna_private_table(kind), rng) {
            // Allocation and the cap count successful rolls, including deduplicated items.
            let reward = (owners[successes % owners.len()], item);
            if !rewards.contains(&reward) {
                rewards.push(reward);
            }
            successes += 1;
            if successes == 8 {
                break;
            }
        }
    }
    rewards
}
pub fn private(kind: u16) -> bool {
    crate::dungeon::tomb::BOSSES.contains(&kind)
        || matches!(
            kind,
            crate::dungeon::tomb::TREASURE | crate::dungeon::tomb::SARCOPHAGUS
        )
        || crate::quest::final_root(kind)
        || kind == crate::dungeon::davy::BOSS
        || matches!(kind, crate::dungeon::trench::HERMIT | 0xd50)
        || matches!(
            kind,
            0x679..=0x67c | 0xd5a | 0xd60 | 0x1705 | crate::dungeon::trench::THESSAL
        )
        || matches!(kind, 0x649 | 0x91a | 0x6d8 | 0x905..=0x907 | 0x683..=0x686 | 0x927 | 0x640 | 0x645 | 0x646 | 0x64f | 0x650..=0x659
        | 0xd05 | 0x917 | crate::dungeon::snake::GUARD | 0xd06 | 0xd90 | 0xd95 | 0xd98..=0xd9a
        | crate::dungeon::jungle::BOSS | crate::dungeon::beach::BOSS
        | crate::dungeon::manor::BOSS | crate::dungeon::manor::COFFIN | crate::dungeon::manor::KAGE
        | crate::dungeon::abyss::MALPHAS | crate::dungeon::abyss::IDOL
        | 0x668 | 0x67d | 0xd78 | 0xd79 | 0xd7f | crate::endgame::ORYX | crate::endgame::ORYX2)
}
fn chest_table(kind: u16) -> Vec<(f64, Pick)> {
    use Pick::Tier;
    if kind == crate::dungeon::jungle::COIL {
        return vec![
            (0.3, Tier(4, WEAPONS)),
            (0.2, Tier(5, WEAPONS)),
            (0.3, Tier(4, ARMOR)),
            (0.2, Tier(5, ARMOR)),
            (0.25, Tier(1, ABILITIES)),
            (0.15, Tier(2, ABILITIES)),
            (0.3, Tier(2, RINGS)),
            (0.2, Tier(3, RINGS)),
            (0.5, Tier(1, &[10])),
            (0.5, Tier(1, &[10])),
            (0.5, Tier(1, &[10])),
        ];
    }
    let (weapon, armor) = match kind {
        0x675 | 0x678 => (5, 4),
        0x67d => (6, 5),
        0x668 => (7, 6),
        _ => unreachable!("selected landmark chest"),
    };
    let ring = if matches!(kind, 0x675 | 0x678) { 1 } else { 2 };
    [
        (0.3, Tier(weapon, WEAPONS)),
        (0.2, Tier(weapon + 1, WEAPONS)),
        (0.1, Tier(weapon + 2, WEAPONS)),
        (0.3, Tier(armor, ARMOR)),
        (0.2, Tier(armor + 1, ARMOR)),
        (0.1, Tier(armor + 2, ARMOR)),
        (0.3, Tier(2, ABILITIES)),
        (0.2, Tier(3, ABILITIES)),
        (0.1, Tier(4, ABILITIES)),
        (0.25, Tier(ring, RINGS)),
        (0.15, Tier(ring + 1, RINGS)),
        (0.5, Tier(1, &[10])),
    ]
    .into_iter()
    .filter(|(_, pick)| !matches!(kind, 0x675 | 0x678) || !matches!(pick, Tier(4, ABILITIES)))
    .collect()
}
pub fn landmark_chest(content: &Content, kind: u16, rng: &mut impl Rng) -> Result<Vec<i32>> {
    let mut consideration = Vec::new();
    for (chance, pick) in chest_table(kind) {
        let pool = candidates(content, pick);
        ensure!(
            !pool.is_empty(),
            "landmark {kind:#x} chest has an empty selected tier"
        );
        // MobDrops.cs expands p/n per item; selected BTree order is not authenticated source order.
        let probability = chance / pool.len() as f64;
        consideration.extend(pool.into_iter().map(|id| (probability, id)));
    }
    // TempleA/B.cs (realm-src@1855c017; data/REALM-SRC-LICENSE) use GetLoots(3, 8) as a success ceiling.
    let minimum_limit = if kind == crate::dungeon::jungle::COIL {
        3
    } else {
        5
    };
    let maximum = rng.gen_range(minimum_limit..8);
    let mut items = Vec::new();
    for (chance, id) in consideration {
        if rng.gen_bool(chance) {
            items.push(i32::from(id));
            if items.len() == maximum {
                break;
            }
        }
    }
    items.resize(8, -1);
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{
        SeedableRng,
        rngs::{StdRng, mock::StepRng},
    };

    #[test]
    fn coil_chest_limits_successes_without_filling_failed_rolls() {
        let content = Content::load(std::path::Path::new("data")).unwrap();
        for (kind, limit) in [
            (crate::dungeon::jungle::COIL, 3),
            (0x675, 5),
            (0x678, 5),
            (0x67d, 5),
            (0x668, 5),
        ] {
            let items = landmark_chest(&content, kind, &mut StepRng::new(0, 0)).unwrap();
            assert_eq!(items.len(), 8);
            assert_eq!(
                items.iter().filter(|item| **item >= 0).count(),
                limit,
                "{kind:#x}"
            );
            let empty = landmark_chest(&content, kind, &mut StepRng::new(0, u64::MAX)).unwrap();
            assert_eq!(empty, vec![-1; 8], "{kind:#x}");
        }
    }

    #[test]
    fn landmark_chests_roll_each_selected_item_with_divided_probability() {
        let content = Content::load(std::path::Path::new("data")).unwrap();
        let signature = |kind| {
            chest_table(kind)
                .into_iter()
                .map(|(probability, pick)| {
                    let Pick::Tier(tier, slots) = pick else {
                        panic!("unexpected fixed item in landmark {kind:#x} table")
                    };
                    (probability, tier, slots)
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(signature(0x675), signature(0x678));
        assert_eq!(candidates(&content, Pick::Tier(4, WEAPONS)).len(), 5);
        assert_eq!(candidates(&content, Pick::Tier(4, ARMOR)).len(), 3);
        for (kind, tiers) in [
            (crate::dungeon::jungle::COIL, vec![4, 4, 4]),
            (0x675, vec![4, 4, 4, 5, 5]),
            (0x678, vec![4, 4, 4, 5, 5]),
            (0x67d, vec![5, 5, 5, 6, 6]),
            (0x668, vec![6, 6, 6, 7, 7]),
        ] {
            let items =
                landmark_chest(&content, kind, &mut StepRng::new(0x1000000000000000, 0)).unwrap();
            let actual: Vec<_> = items
                .into_iter()
                .filter(|item| *item >= 0)
                .map(|id| {
                    let item = &content.objects[&(id as u16)];
                    assert!(ARMOR.contains(&item.slot_type), "{kind:#x}: {id:#x}");
                    item.tier.unwrap()
                })
                .collect();
            assert_eq!(actual, tiers, "{kind:#x}");
        }
    }

    #[test]
    fn white_bag_equipment_has_a_boss_source_unless_unreferenced() {
        let content = Content::load(std::path::Path::new("data")).unwrap();
        let sources = collectibles(&content);
        let missing: Vec<_> = content
            .objects
            .values()
            .filter(|o| o.class == "Equipment" && o.bag_type == 4 && !sources.contains(&o.kind))
            .map(|o| o.name.as_str())
            .collect();
        assert_eq!(
            missing,
            [
                "Amulet of Resurrection",
                "Admin Sword",
                "Penetrating Blast Spell",
                "Prism of Dancing Swords",
                "Skull of Endless Torment",
                "Thousand Shot",
                "America Ring",
            ]
        );
        for kind in [0x941, 0x924] {
            let rewards =
                fauna_private_rewards(&content, kind, &[(7, 101)], &mut StepRng::new(0, 0));
            assert_eq!(
                rewards.len(),
                1,
                "{kind:#x}: private and deduplicated per owner"
            );
            assert!(rewards[0].0 == 7 && content.objects[&rewards[0].1].bag_type == 4);
        }
    }

    #[test]
    fn abyss_malphas_rewards_include_both_stat_potions() {
        let content = Content::load(std::path::Path::new("data")).unwrap();
        for seed in 0..64 {
            let items = rewards(
                &content,
                crate::dungeon::abyss::MALPHAS,
                &mut StdRng::seed_from_u64(seed),
            );
            assert!(
                items.contains(&0xa34) && items.contains(&0xa20),
                "seed={seed} items={items:?}"
            );
        }
    }

    #[test]
    fn abyss_idol_rewards_include_both_stat_potions() {
        let content = Content::load(std::path::Path::new("data")).unwrap();
        assert!(private(crate::dungeon::abyss::IDOL));
        for seed in 0..64 {
            let items = rewards(
                &content,
                crate::dungeon::abyss::IDOL,
                &mut StdRng::seed_from_u64(seed),
            );
            for potion in [0xa20, 0xa34] {
                assert_eq!(
                    items.iter().filter(|item| **item == potion).count(),
                    1,
                    "seed={seed} potion={potion:#x} items={items:?}"
                );
            }
        }
    }
}
