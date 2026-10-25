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
// The period wiki calls Wolfskin/Neophyte/Iron Mail T0; the selected client calls them T1.
// Armor rates below use selected-client tiers (wiki tier + 1), verified through all 39 named armors.
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
        // Trapped's ordered first-success soul bag: Wand, Sword, then Att/Def/Dex/Spd/Vit/Wis potions: https://github.com/trapped/rotmg_svr/blob/593e490803fe0ad168ead408cacd5287d778acfe/wServer/logic/db/BehaviorDb.Crystal.cs#L674-L684
        0x941 => &[
            (0.02, Pick::Item(0xb3f)),
            (0.02, Pick::Item(0xa03)),
            (0.5, Pick::Item(0xa1f)),
            (0.5, Pick::Item(0xa20)),
            (0.5, Pick::Item(0xa4c)),
            (0.5, Pick::Item(0xa21)),
            (0.5, Pick::Item(0xa34)),
            (0.5, Pick::Item(0xa35)),
        ],
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
pub(crate) fn period_dungeon(kind: u16) -> bool {
    matches!(
        kind,
        0x21d..=0x220 | 0x936 | 0x683..=0x68f | 0x927 | 0xd01..=0xd04
            | 0xd06 | 0x917 | 0xd90 | 0x66d..=0x672 | crate::dungeon::abyss::MALPHAS
    )
}
pub(crate) fn period_surface(kind: u16) -> bool {
    matches!(
        kind,
        0x202 | 0x204 | 0x205 | 0x209 | 0x20d | 0x20f
            | 0x600..=0x602 | 0x604 | 0x606 | 0x607 | 0x609 | 0x60a | 0x60c..=0x610
            | 0x614 | 0x615 | 0x617 | 0x618 | 0x61a | 0x61c..=0x61f
            | 0x624..=0x626 | 0x629..=0x62f | 0x631..=0x633
            | 0x63b | 0x63c | 0x63e..=0x640 | 0x642..=0x645 | 0x647..=0x64d
            | 0x64f | 0x650 | 0x900..=0x902
    )
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
        // https://web.archive.org/web/20120201110941/http://forums.wildshadow.com/wiki/realm-mad-god/arachna-spider-queen
        crate::dungeon::spider::BOSS => &[
            (0.99, Item(0x707)),
            (0.25, Item(0x707)),
            (0.23, Item(0x708)),
            (0.23, Item(0x709)),
            (0.99, Item(0xa16)),
        ],
        // https://web.archive.org/web/20120216023204/http://forums.wildshadow.com/wiki/realm-mad-god/brown-den-spider
        0x21d..=0x220 => &[(0.15, Item(0x707))],
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
        0x205 | 0x20d | 0x20f | 0x202 => &[(0.03, Item(0xa22)), (0.03, Item(0xa23))],
        0x635 | 0x636 | 0x639 | 0x6c5 | 0x7f22 | 0x6c7 | 0x6c9 | 0x6d1 | 0x951 => {
            &[(0.03, Item(0xa22)), (0.02, Item(0xa23))]
        }
        0x903 => &[(0.04, Item(0xa22)), (0.01, Item(0xa23))],
        0x209 => &[
            (0.03, Item(0xa22)),
            (0.03, Item(0xa23)),
            (0.01, Item(0xa54)),
        ],
        0x904 => &[(0.03, Item(0xa23)), (0.01, Tier(5, WEAPONS))],
        0x204 => &[
            (0.01, Tier(2, ARMOR)),
            (0.01, Item(0xa22)),
            (0.01, Item(0xa23)),
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
        // Item lists match the May-June 2012 Wild Shadow skeleton pages, e.g. https://web.archive.org/web/20120608231314/http://forums.wildshadow.com/wiki/realm-mad-god/skeleton-mage
        // Rates are realm-src@1855c017 community values (data/REALM-SRC-LICENSE), whose Mage lists Comet Staff twice; kept once:
        // https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/logic/db/BehaviorDb.Deathmage.cs
        0x65a => &[(0.02, Item(0xa01)), (0.02, Item(0xa15))],
        0x65b => &[
            (0.03, Item(0xa01)),
            (0.02, Item(0xa0a)),
            (0.02, Item(0xa67)),
        ],
        0x65c => &[
            (0.03, Item(0xa01)),
            (0.02, Item(0xa0b)),
            (0.01, Item(0xa57)),
            (0.01, Item(0xaa4)),
        ],
        0x65d => &[
            (0.02, Item(0xa05)),
            (0.02, Item(0xa99)),
            (0.02, Item(0xa2f)),
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
        // Wild Shadow wiki rates before the March 2012 edits removed them (Pentaract is per tower):
        // https://web.archive.org/web/20120117000123/http://forums.wildshadow.com:80/wiki/realm-mad-god/cube-god
        0xd56 | 0xd59 => &[
            (0.4, Tier(8, WEAPONS)),
            (0.2, Tier(9, WEAPONS)),
            (0.1, Tier(10, WEAPONS)),
            (0.05, Tier(11, WEAPONS)),
            (0.4, Tier(8, ARMOR)),
            (0.2, Tier(9, ARMOR)),
            (0.1, Tier(10, ARMOR)),
            (0.05, Tier(11, ARMOR)),
            (0.025, Tier(12, ARMOR)),
            (0.4, Tier(3, RINGS)),
            (0.1, Tier(4, RINGS)),
            (0.025, Tier(5, RINGS)),
            (0.2, Tier(4, ABILITIES)),
            (0.05, Tier(5, ABILITIES)),
            (0.5, Tier(2, &[10])),
            (0.5, Tier(2, &[10])),
            (0.5, Tier(2, &[10])),
        ],
        0xd60 => &[
            (0.08, Tier(8, WEAPONS)),
            (0.04, Tier(9, WEAPONS)),
            (0.02, Tier(10, WEAPONS)),
            (0.01, Tier(11, WEAPONS)),
            (0.08, Tier(8, ARMOR)),
            (0.04, Tier(9, ARMOR)),
            (0.02, Tier(10, ARMOR)),
            (0.01, Tier(11, ARMOR)),
            (0.005, Tier(12, ARMOR)),
            (0.08, Tier(3, RINGS)),
            (0.02, Tier(4, RINGS)),
            (0.005, Tier(5, RINGS)),
            (0.04, Tier(4, ABILITIES)),
            (0.01, Tier(5, ABILITIES)),
            (0.1, Tier(2, &[10])),
            (0.1, Tier(2, &[10])),
            (0.1, Tier(2, &[10])),
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
        0x624 => &[(0.02, Tier(5, WEAPONS)), (0.05, Item(0xa23))],
        0x625 => &[(0.04, Tier(2, ABILITIES)), (0.05, Item(0xa23))],
        0x626 => &[(0.01, Tier(6, ARMOR)), (0.05, Item(0xa23))],
        0x62d => &[(0.04, Tier(5, WEAPONS)), (0.03, Item(0xa22))],
        0x631 => &[
            (0.04, Tier(5, WEAPONS)),
            (0.03, Item(0xa22)),
            (0.03, Item(0xa23)),
        ],
        0x62e => &[(0.04, Tier(6, ARMOR)), (0.03, Item(0xa22))],
        0x632 => &[
            (0.1, Tier(3, WEAPONS)),
            (0.05, Tier(4, WEAPONS)),
            (0.04, Tier(6, ARMOR)),
            (0.05, Tier(1, RINGS)),
            (0.05, Tier(1, ABILITIES)),
            (0.03, Item(0xa23)),
        ],
        0x62f => &[(0.03, Item(0xa22)), (0.05, Tier(2, RINGS))],
        0x633 => &[
            (0.1, Tier(3, WEAPONS)),
            (0.05, Tier(4, WEAPONS)),
            (0.1, Tier(3, ARMOR)),
            (0.05, Tier(4, ARMOR)),
            (0.05, Tier(2, RINGS)),
            (0.05, Tier(1, ABILITIES)),
            (0.03, Item(0xa23)),
        ],
        0x629 | 0x62b => &[(0.03, Item(0xa22))],
        0x62a | 0x647 | 0x648 => &[(0.03, Item(0xa23))],
        // Orc King: Wild Shadow wiki, snapshot 20120207192955 (portals unchanged).
        0x62c => &[
            (0.16, Tier(3, WEAPONS)),
            (0.08, Tier(4, WEAPONS)),
            (0.04, Tier(5, WEAPONS)),
            (0.16, Tier(4, ARMOR)),
            (0.08, Tier(5, ARMOR)),
            (0.04, Tier(6, ARMOR)),
            (0.10, Tier(2, ABILITIES)),
            (0.03, Item(0xa22)),
            (0.05, Tier(2, RINGS)),
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
            (0.01, Tier(4, ARMOR)),
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
        0x612 | 0x613 => &[(0.03, Item(0xa22))],
        0x618 => &[(0.04, Item(0xa22))],
        0x619 => &[(0.03, Item(0xa23))],
        0x953 => &[
            (0.2, Tier(1, WEAPONS)),
            (0.1, Tier(2, WEAPONS)),
            (0.2, Tier(1, ARMOR)),
            (0.03, Item(0xa22)),
            (0.02, Item(0xa23)),
        ],
        0x614 => &[
            (0.30, Tier(3, WEAPONS)),
            (0.30, Tier(3, ARMOR)),
            (0.09, Tier(1, RINGS)),
            (0.38, Tier(1, ABILITIES)),
            (0.02, Item(0xa22)),
            (0.02, Item(0xa23)),
        ],
        0x61a => &[
            (0.30, Tier(3, WEAPONS)),
            (0.30, Tier(3, ARMOR)),
            (0.12, Tier(1, RINGS)),
            (0.39, Tier(1, ABILITIES)),
            (0.03, Item(0xa23)),
        ],
        0x60e..=0x610 => &[(0.04, Item(0xa22)), (0.04, Item(0xa23))],
        // https://web.archive.org/web/20111126104739/http://forums.wildshadow.com/wiki/realm-mad-god/cave-pirate-brawler
        0x687..=0x689 => &[(0.03, Item(0xa22))],
        // https://web.archive.org/web/20111126104744/http://forums.wildshadow.com/wiki/realm-mad-god/cave-pirate-cabin-boy
        0x68a..=0x68e => &[(0.2, Tier(1, ABILITIES))],
        0x68f => &[(0.1, Tier(1, WEAPONS)), (0.2, Tier(1, ABILITIES))],
        // https://web.archive.org/web/20111126105041/http://forums.wildshadow.com/wiki/realm-mad-god/pirate-lieutenant
        0x683..=0x686 => &[
            (0.2, Tier(1, ARMOR)),
            (0.1, Tier(2, ARMOR)),
            (0.05, Tier(1, RINGS)),
            (0.2, Tier(2, WEAPONS)),
            (0.1, Tier(3, WEAPONS)),
            (0.005, Item(0xa50)),
        ],
        // https://web.archive.org/web/20120217160929/http://forums.wildshadow.com/wiki/realm-mad-god/dreadstump-pirate-king
        0x927 => &[
            (0.05, Item(0xa50)),
            (0.4, Tier(1, ARMOR)),
            (0.1, Tier(1, RINGS)),
            (0.2, Tier(2, ARMOR)),
            (0.4, Tier(2, WEAPONS)),
            (0.1, Tier(3, ARMOR)),
            (0.2, Tier(3, WEAPONS)),
            (0.05, Tier(4, ARMOR)),
            (0.1, Tier(4, WEAPONS)),
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
        0x606 | 0x615 => &[(0.04, Item(0xa22)), (0.03, Item(0xa23))],
        0x61c | 0x61e | 0x63b | 0x64a..=0x64d => &[(0.03, Item(0xa22)), (0.03, Item(0xa23))],
        0x607 | 0x60a | 0x60c | 0x61d | 0x63c | 0x63f | 0x642..=0x644 => {
            &[(0.05, Item(0xa22)), (0.03, Item(0xa23))]
        }
        0x605
        | 0x616
        | 0x63a
        | 0xd91..=0xd94
        | 0xd96
        | 0xd97
        | 0xd9b..=0xd9f
        | 0xda4..=0xda6
        | 0xdb0..=0xdb2 => &[(0.05, Item(0xa22)), (0.03, Item(0xa23))],
        0x60d => &[
            (0.30, Tier(3, WEAPONS)),
            (0.30, Tier(3, ARMOR)),
            (0.11, Tier(1, RINGS)),
            (0.39, Tier(1, ABILITIES)),
            (0.04, Item(0xa22)),
        ],
        0x609 => &[
            (0.36, Tier(2, WEAPONS)),
            (0.10, Tier(3, WEAPONS)),
            (0.36, Tier(2, ARMOR)),
            (0.11, Tier(1, RINGS)),
            (0.39, Tier(1, ABILITIES)),
            (0.02, Item(0xa22)),
            (0.02, Item(0xa23)),
        ],
        0x617 => &[
            (0.30, Tier(2, WEAPONS)),
            (0.30, Tier(2, ARMOR)),
            (0.11, Tier(1, RINGS)),
            (0.39, Tier(1, ABILITIES)),
            (0.02, Item(0xa22)),
            (0.02, Item(0xa23)),
        ],
        // Dwarf King has no monster-page rates; tier pages fill this pool.
        0x61f => &[
            (0.20, Tier(2, WEAPONS)),
            (0.10, Tier(3, WEAPONS)),
            (0.50, Tier(4, WEAPONS)),
            (0.32, Tier(1, ARMOR)),
            (0.16, Tier(2, ARMOR)),
            (0.08, Tier(3, ARMOR)),
            (0.04, Tier(4, ARMOR)),
            (0.10, Tier(1, RINGS)),
            (0.10, Tier(1, ABILITIES)),
        ],
        0x63e => &[
            (0.1, Tier(3, WEAPONS)),
            (0.05, Tier(4, WEAPONS)),
            (0.1, Tier(3, ARMOR)),
            (0.05, Tier(4, ARMOR)),
            (0.05, Tier(1, RINGS)),
            (0.05, Tier(1, ABILITIES)),
            (0.03, Item(0xa22)),
        ],
        0x645 => &[
            (0.16, Tier(5, WEAPONS)),
            (0.08, Tier(6, WEAPONS)),
            (0.16, Tier(4, ARMOR)),
            (0.08, Tier(5, ARMOR)),
            (0.04, Tier(6, ARMOR)),
            (0.05, Tier(3, RINGS)),
            (0.03, Tier(3, ABILITIES)),
            (0.03, Item(0xa23)),
        ],
        0x64f => &[
            (0.16, Tier(5, WEAPONS)),
            (0.08, Tier(6, WEAPONS)),
            (0.16, Tier(4, ARMOR)),
            (0.08, Tier(5, ARMOR)),
            (0.04, Tier(6, ARMOR)),
            (0.05, Tier(3, RINGS)),
            (0.03, Tier(3, ABILITIES)),
            (0.03, Item(0xa22)),
        ],
        0x640 | 0x649 | 0x650 => &[
            (0.16, Tier(5, WEAPONS)),
            (0.08, Tier(6, WEAPONS)),
            (0.04, Tier(7, WEAPONS)),
            (0.16, Tier(5, ARMOR)),
            (0.08, Tier(6, ARMOR)),
            (0.04, Tier(7, ARMOR)),
            (0.05, Tier(3, RINGS)),
            (0.10, Tier(3, ABILITIES)),
            (0.005, Item(0xaa9)),
        ],
        0x646 | 0x905 => &[
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
        // Leviathan's monster page overrides the general Realm Gods tier-page rates:
        // https://web.archive.org/web/20120215050418/http://forums.wildshadow.com:80/wiki/realm-mad-god/weapons
        0x6d8 => &[
            (0.02, Tier(6, WEAPONS)),
            (0.01, Tier(7, WEAPONS)),
            (0.005, Tier(8, WEAPONS)),
            (0.02, Tier(6, ARMOR)),
            (0.02, Tier(7, ARMOR)),
            (0.01, Tier(8, ARMOR)),
            (0.005, Tier(9, ARMOR)),
            (0.0075, Tier(3, RINGS)),
            (0.005, Tier(4, RINGS)),
            (0.01, Tier(4, ABILITIES)),
        ],
        0x651 | 0x652 | 0x654..=0x659 | 0xd05 | 0x91a => &[
            (0.04, Tier(6, WEAPONS)),
            (0.02, Tier(7, WEAPONS)),
            (0.01, Tier(8, WEAPONS)),
            (0.08, Tier(6, ARMOR)),
            (0.04, Tier(7, ARMOR)),
            (0.02, Tier(8, ARMOR)),
            (0.01, Tier(9, ARMOR)),
            (0.015, Tier(3, RINGS)),
            (0.005, Tier(4, RINGS)),
            (0.02, Tier(4, ABILITIES)),
        ],
        0x223..=0x229 => &[
            (0.1, Item(0xa3f)),
            (0.05, Tier(3, RINGS)),
            (0.05, Tier(6, WEAPONS)),
        ],
        // https://web.archive.org/web/20120127132732/http://forums.wildshadow.com/wiki/realm-mad-god/sprites
        // Keep existing unlisted candidates; absence from the wiki is not disproof.
        0xd00 => &[(0.02, Tier(5, WEAPONS)), (0.05, Item(0xa23))],
        0xd01 => &[
            (0.02, Tier(5, WEAPONS)),
            (0.05, Item(0xa23)),
            (0.04, Tier(2, ABILITIES)),
        ],
        0xd02 => &[
            (0.02, Tier(5, WEAPONS)),
            (0.05, Item(0xa23)),
            (0.01, Tier(6, ARMOR)),
        ],
        0xd03 => &[
            (0.02, Tier(5, WEAPONS)),
            (0.03, Item(0xa23)),
            (0.03, Item(0xa4b)),
            (0.02, Item(0xa2d)),
        ],
        0xd04 => &[
            (0.02, Tier(5, WEAPONS)),
            (0.05, Item(0xa23)),
            (0.03, Item(0xa22)),
            (0.02, Item(0xa4d)),
        ],
        0xd95 | 0xd98..=0xd9a => &[
            (0.2, Tier(5, ARMOR)),
            (0.2, Tier(6, WEAPONS)),
            (0.1, Tier(7, WEAPONS)),
            (0.1, Tier(6, ARMOR)),
            (0.1, Tier(3, ABILITIES)),
            (0.1, Tier(3, RINGS)),
        ],
        // https://web.archive.org/web/20120118035706/http://forums.wildshadow.com/wiki/realm-mad-god/stheno-snake-queen
        0x917 => &[
            (1., Item(0xa21)),
            (0.006, Item(crate::endgame::INCANTATION)),
            (0.001, Item(0xc04)),
            (0.45, Item(0xa3e)),
            (0.45, Item(0xa40)),
            (0.45, Item(0xa41)),
            (0.3, Tier(9, WEAPONS)),
            (0.3, Tier(9, ARMOR)),
            (0.05, Tier(10, WEAPONS)),
            (0.05, Tier(10, ARMOR)),
        ],
        crate::dungeon::snake::GUARD => &[
            (1., Item(0xa21)),
            (0.05, Item(crate::endgame::INCANTATION)),
            (0.05, Item(0xc04)),
            (0.05, Item(0xa3e)),
            (0.05, Item(0xa40)),
            (0.05, Item(0xa41)),
            (0.2, Tier(9, WEAPONS)),
            (0.3, Tier(8, ARMOR)),
        ],
        // https://web.archive.org/web/20120114025209/http://forums.wildshadow.com/wiki/realm-mad-god/limon-sprite-god
        // The cloak rate is marked "???"; preserve its existing reconstruction.
        0xd06 => &[
            (1., Item(0xa4c)),
            (0.1, Item(0xa4b)),
            (0.006, Item(crate::endgame::INCANTATION)),
            (0.001, Item(0xc03)),
            (0.01, Item(0xa5a)),
            (0.05, Tier(7, ARMOR)),
            (0.05, Tier(3, ABILITIES)),
            (0.01, Tier(5, ABILITIES)),
            (0.024, Tier(4, ABILITIES)),
            (0.05, Tier(3, RINGS)),
            // Later comparator's optional bonus, not recovered 123.5.1 odds: https://github.com/ossimc82/fabiano-swagger-of-doom/blob/6fd20aad4a7905b13f25389c68368a942a2b68cb/wServer/logic/db/BehaviorDb.SpriteWorld.cs#L143-L145
            (0.05, Item(0xa20)),
        ],
        // https://web.archive.org/web/20120114021352/http://forums.wildshadow.com/wiki/realm-mad-god/septavius-ghost-god
        0xd90 => &[
            (1., Item(0xa35)),
            (0.006, Item(crate::endgame::INCANTATION)),
            (0.001, Item(0xc02)),
            (0.4, Tier(7, WEAPONS)),
            (0.2, Tier(8, WEAPONS)),
            (0.4, Tier(3, ABILITIES)),
            (0.15, Tier(4, ABILITIES)),
            (0.05, Tier(5, ABILITIES)),
            (0.4, Tier(3, RINGS)),
            (0.12, Tier(4, RINGS)),
        ],
        // https://web.archive.org/web/20120209020521/http://forums.wildshadow.com/wiki/realm-mad-god/imp-abyss
        0x66d => &[
            (0.02, Item(0xa22)),
            (0.02, Item(0xa23)),
            (0.01, Item(0xa59)),
            (0.01, Item(0xaa7)),
        ],
        // https://web.archive.org/web/20120216011240/http://forums.wildshadow.com/wiki/realm-mad-god/demon-abyss
        0x66e => &[
            (0.03, Item(0xa22)),
            (0.02, Item(0xa23)),
            (0.02, Item(0xa1c)),
            (0.02, Item(0xa12)),
        ],
        // https://web.archive.org/web/20120216044619/http://forums.wildshadow.com/wiki/realm-mad-god/demon-warrior-abyss
        0x66f => &[
            (0.03, Item(0xa22)),
            (0.02, Item(0xa23)),
            (0.02, Item(0xa3b)),
            (0.02, Item(0xa0a)),
        ],
        // https://web.archive.org/web/20120215021713/http://forums.wildshadow.com/wiki/realm-mad-god/demon-mage-abyss
        0x670 => &[
            (0.03, Item(0xa22)),
            (0.02, Item(0xa23)),
            (0.02, Item(0xa2f)),
            (0.02, Item(0xa3d)),
            (0.02, Item(0xa9d)),
            (0.02, Item(0xa5f)),
            (0.02, Item(0xaae)),
            (0.01, Item(0xab5)),
        ],
        // https://web.archive.org/web/20120215010915/http://forums.wildshadow.com/wiki/realm-mad-god/brute-abyss
        0x671 => &[
            (0.02, Item(0xa22)),
            (0.02, Item(0xa23)),
            (0.02, Item(0xa17)),
            (0.01, Item(0xa6a)),
        ],
        // https://web.archive.org/web/20120214232201/http://forums.wildshadow.com/wiki/realm-mad-god/brute-warrior-abyss
        0x672 => &[
            (0.03, Item(0xa22)),
            (0.02, Item(0xa23)),
            (0.02, Item(0xa3c)),
            (0.01, Item(0xa4e)),
            (0.01, Item(0xa64)),
            (0.02, Item(0xa32)),
        ],
        0x908 => &[(0.04, Item(0xa22)), (0.06, Item(0xa23))],
        0x90b => &[(0.1, Item(0xa22)), (0.1, Item(0xa23))],
        0x909 => &[],
        // https://web.archive.org/web/20120207191548/http://forums.wildshadow.com/wiki/realm-mad-god/archdemon-malphas
        // Preserve #244's paired private-potion contract; historical allocation is separate: https://github.com/OpenRotMG/Open-RotMG-Server/issues/244
        crate::dungeon::abyss::MALPHAS => &[
            (1., Item(0xa34)),
            (1., Item(0xa20)),
            (0.006, Item(crate::endgame::INCANTATION)),
            (0.001, Item(0xc01)),
            (0.7, Tier(7, WEAPONS)),
            (0.4, Tier(9, WEAPONS)),
            (0.15, Tier(10, WEAPONS)),
            (0.7, Tier(7, ARMOR)),
            (0.4, Tier(9, ARMOR)),
            (0.15, Tier(10, ARMOR)),
            (0.5, Tier(3, ABILITIES)),
            (0.3, Tier(4, ABILITIES)),
            (0.5, Tier(3, RINGS)),
            (0.3, Tier(4, RINGS)),
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
        .chain([0x678, 0x67d, 0x668, 0x925].into_iter().flat_map(|kind| {
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
        // Native's 1.5% follows period testimony, not the later 2% table: https://discord.com/channels/1558662335076376578/1558662336439386185/1563889316432973895
        0x651 | 0x652 | 0x657 | 0xd05 | 0x905..=0x907 => &[(0.015, 0xa1f)],
        0x659 => &[(0.015, 0xa21)],
        0x654 => &[(0.01, 0xa21)],
        0x91a => &[(0.02, 0xa21)],
        0x655 | 0x658 => &[(0.015, 0xa20)],
        0x656 => &[(0.01, 0xa20)],
        0x6d8 => &[(0.025, 0xa20)],
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
pub(crate) fn period_boss(kind: u16) -> bool {
    matches!(kind, 0xd56 | 0xd59 | 0xd60)
}
fn independent(kind: u16) -> bool {
    period_dungeon(kind)
        || period_surface(kind)
        || kind == crate::dungeon::tomb::TREASURE
        || crate::quest::final_root(kind)
        || matches!(kind, crate::dungeon::trench::HERMIT | 0xd50 | 0xd52 | 0xd53)
        || matches!(kind, 0x66d..=0x672 | 0x908 | 0x90b | 0x65a..=0x65d)
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
        )
        || matches!(kind, 0x651..=0x659 | 0xd05 | 0x91a | 0x6d8 | 0xd56 | 0xd59)
}
/// Stat potions from these Realm gods need the wiki's soulbound damage; other god loot needs any hit.
pub(crate) fn potion_damage(kind: u16) -> Option<i32> {
    match kind {
        0x651 | 0x655 | 0x657..=0x659 => Some(180),
        0x656 => Some(270),
        0x652 | 0x654 | 0x6d8 | 0xd05 => Some(360),
        0x91a => Some(0),
        _ => None,
    }
}
pub fn rewards(content: &Content, kind: u16, rng: &mut impl Rng) -> Vec<u16> {
    let mut items = if independent(kind) {
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
    // The June 2012 wiki's Deathmage Chest lists these tiers without rates; rates are realm-src Graveyard.cs:
    // https://web.archive.org/web/20120610073636id_/http://forums.wildshadow.com:80/wiki/realm-mad-god/deathmage
    // https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/realm/setpieces/Graveyard.cs
    if kind == 0x925 {
        return vec![
            (0.3, Tier(4, WEAPONS)),
            (0.2, Tier(5, WEAPONS)),
            (0.1, Tier(6, WEAPONS)),
            (0.3, Tier(3, ARMOR)),
            (0.2, Tier(4, ARMOR)),
            (0.1, Tier(5, ARMOR)),
            (0.3, Tier(1, ABILITIES)),
            (0.2, Tier(2, ABILITIES)),
            (0.2, Tier(3, ABILITIES)),
            (0.25, Tier(1, RINGS)),
            (0.15, Tier(2, RINGS)),
            (0.5, Tier(1, &[10])),
        ];
    }
    // Monster-page chest rates override the generic community-server pools.
    // https://web.archive.org/web/20120217151558/http://forums.wildshadow.com:80/wiki/realm-mad-god/oasis-giant
    let (weapon, armor, tier_rates, ring, ring_rates, ability_rates) = match kind {
        0x675 | 0x678 => (5, 5, [0.75, 0.375, 0.167], 1, [0.6, 0.15], [0.3, 0.075, 0.]),
        0x67d => (6, 6, [0.5, 0.25, 0.125], 2, [0.2, 0.05], [0.4, 0.1, 0.025]),
        0x668 => (7, 7, [0.5, 0.25, 0.125], 2, [0.4, 0.1], [0.8, 0.2, 0.05]),
        _ => unreachable!("selected landmark chest"),
    };
    [
        (tier_rates[0], Tier(weapon, WEAPONS)),
        (tier_rates[1], Tier(weapon + 1, WEAPONS)),
        (tier_rates[2], Tier(weapon + 2, WEAPONS)),
        (tier_rates[0], Tier(armor, ARMOR)),
        (tier_rates[1], Tier(armor + 1, ARMOR)),
        (tier_rates[2], Tier(armor + 2, ARMOR)),
        (ability_rates[0], Tier(2, ABILITIES)),
        (ability_rates[1], Tier(3, ABILITIES)),
        (ability_rates[2], Tier(4, ABILITIES)),
        (ring_rates[0], Tier(ring, RINGS)),
        (ring_rates[1], Tier(ring + 1, RINGS)),
        (1., Tier(1, &[10])),
        (1., Tier(1, &[10])),
    ]
    .into_iter()
    .filter(|(chance, _)| *chance > 0.)
    .collect()
}
pub fn landmark_chest(content: &Content, kind: u16, rng: &mut impl Rng) -> Result<Vec<i32>> {
    if !matches!(kind, crate::dungeon::jungle::COIL | 0x925) {
        let mut items = Vec::new();
        for (chance, pick) in chest_table(kind) {
            let pool = candidates(content, pick);
            ensure!(
                !pool.is_empty(),
                "landmark {kind:#x} chest has an empty selected tier"
            );
            if rng.gen_bool(chance) {
                items.push(i32::from(*pool.choose(rng).expect("validated chest tier")));
            }
        }
        return Ok(items);
    }
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
    // TempleA/B.cs and Graveyard.cs (realm-src@1855c017; data/REALM-SRC-LICENSE) use GetLoots(3, 8) as a success ceiling.
    let maximum = rng.gen_range(3..8);
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

    fn surface_rate(kind: u16, expected: Pick) -> Option<f64> {
        table(kind).iter().find_map(|(chance, pick)| {
            let same = match (*pick, expected) {
                (Pick::Item(a), Pick::Item(b)) => a == b,
                (Pick::Tier(a, slots_a), Pick::Tier(b, slots_b)) => a == b && slots_a == slots_b,
                _ => false,
            };
            same.then_some(*chance)
        })
    }

    #[test]
    fn wild_shadow_surface_monster_pages_correct_rates_and_wrong_tiers() {
        use Pick::{Item, Tier};
        for (kind, pick, chance) in [
            (0x202, Item(0xa23), 0.03),
            (0x204, Tier(2, ARMOR), 0.01),
            (0x204, Item(0xa22), 0.01),
            (0x209, Item(0xa54), 0.01),
            (0x601, Tier(1, ARMOR), 0.20),
            (0x606, Item(0xa22), 0.04),
            (0x609, Tier(2, WEAPONS), 0.36),
            (0x609, Tier(2, ARMOR), 0.36),
            (0x609, Item(0xa22), 0.02),
            (0x60d, Tier(3, ARMOR), 0.30),
            (0x60d, Tier(1, ABILITIES), 0.39),
            (0x60e, Item(0xa23), 0.04),
            (0x614, Tier(3, ARMOR), 0.30),
            (0x614, Tier(1, ABILITIES), 0.38),
            (0x617, Tier(2, ARMOR), 0.30),
            (0x618, Item(0xa22), 0.04),
            (0x61a, Tier(3, WEAPONS), 0.30),
            (0x61a, Tier(1, RINGS), 0.12),
            (0x61c, Item(0xa22), 0.03),
            (0x624, Tier(5, WEAPONS), 0.02),
            (0x625, Tier(2, ABILITIES), 0.04),
            (0x626, Tier(6, ARMOR), 0.01),
            (0x626, Item(0xa23), 0.05),
            (0x62c, Tier(2, ABILITIES), 0.10),
            (0x62c, Tier(2, RINGS), 0.05),
            (0x62d, Tier(5, WEAPONS), 0.04),
            (0x62e, Tier(6, ARMOR), 0.04),
            (0x62f, Tier(2, RINGS), 0.05),
            (0x631, Item(0xa23), 0.03),
            (0x632, Tier(6, ARMOR), 0.04),
            (0x633, Tier(2, RINGS), 0.05),
            (0x63b, Item(0xa22), 0.03),
            (0x63e, Item(0xa22), 0.03),
            (0x640, Tier(7, WEAPONS), 0.04),
            (0x645, Item(0xa23), 0.03),
            (0x649, Tier(3, ABILITIES), 0.10),
            (0x64a, Item(0xa22), 0.03),
            (0x64f, Item(0xa22), 0.03),
            (0x650, Item(0xaa9), 0.005),
            (0x900, Tier(4, ARMOR), 0.01),
        ] {
            assert!(period_surface(kind), "{kind:#x}");
            assert_eq!(surface_rate(kind, pick), Some(chance), "{kind:#x}");
        }
        for (kind, wrong_tier) in [
            (0x204, Tier(1, ARMOR)),
            (0x601, Tier(0, ARMOR)),
            (0x609, Tier(1, ARMOR)),
            (0x60d, Tier(2, ARMOR)),
            (0x614, Tier(2, ARMOR)),
            (0x617, Tier(1, ARMOR)),
            (0x61a, Tier(2, WEAPONS)),
        ] {
            assert_eq!(surface_rate(kind, wrong_tier), None, "{kind:#x}");
        }
    }

    #[test]
    fn wild_shadow_tier_pages_fill_dwarf_king_and_elf_wizard_gaps() {
        use Pick::Tier;
        for (kind, pick, chance) in [
            (0x609, Tier(3, WEAPONS), 0.10),
            (0x61f, Tier(2, WEAPONS), 0.20),
            (0x61f, Tier(3, WEAPONS), 0.10),
            (0x61f, Tier(4, WEAPONS), 0.50),
            (0x61f, Tier(1, ARMOR), 0.32),
            (0x61f, Tier(2, ARMOR), 0.16),
            (0x61f, Tier(3, ARMOR), 0.08),
            (0x61f, Tier(4, ARMOR), 0.04),
            (0x61f, Tier(1, RINGS), 0.10),
            (0x61f, Tier(1, ABILITIES), 0.10),
        ] {
            assert_eq!(surface_rate(kind, pick), Some(chance), "{kind:#x}");
        }
    }

    #[test]
    fn period_surface_preserves_unlisted_bonuses_and_excludes_other_seams() {
        for (kind, pick, chance) in [
            (0x606, Pick::Item(0xa23), 0.03),
            (0x607, Pick::Item(0xa22), 0.05),
            (0x631, Pick::Item(0xa22), 0.03),
            (0x632, Pick::Tier(3, WEAPONS), 0.10),
            (0x633, Pick::Tier(3, ARMOR), 0.10),
            (0x63e, Pick::Tier(4, WEAPONS), 0.05),
            (0x645, Pick::Tier(5, WEAPONS), 0.16),
            (0x64f, Pick::Tier(3, ABILITIES), 0.03),
        ] {
            assert_eq!(surface_rate(kind, pick), Some(chance), "{kind:#x}");
        }
        for kind in [
            0x651, 0x659, 0x6d8, 0x91a, 0xd05, 0x668, 0x675, 0x678, 0x67d, 0x91c, 0x920, 0x92d,
            0x927, 0x683, 0x936, 0x957,
        ] {
            assert!(!period_surface(kind), "{kind:#x}");
        }
        let content = Content::load(std::path::Path::new("data")).unwrap();
        for kind in content
            .objects
            .keys()
            .copied()
            .filter(|kind| period_surface(*kind))
        {
            validate(&content, kind).unwrap();
        }
    }

    #[test]
    fn period_dungeon_wiki_named_drop_entries() {
        for (kind, item, expected) in [
            (0x936, 0x707, vec![0.99, 0.25]),
            (0x936, 0xa16, vec![0.99]),
            (0x936, 0x708, vec![0.23]),
            (0x936, 0x709, vec![0.23]),
            (0x21d, 0x707, vec![0.15]),
            (0x220, 0x707, vec![0.15]),
            (0x683, 0xa50, vec![0.005]),
            (0x927, 0xa50, vec![0.05]),
            (0xd03, 0xa4b, vec![0.03]),
            (0xd03, 0xa2d, vec![0.02]),
            (0xd04, 0xa4d, vec![0.02]),
            (0xd06, 0xa4b, vec![0.1]),
            (0xd06, 0xc03, vec![0.001]),
            (0x917, 0xa3e, vec![0.45]),
            (0x917, 0xa40, vec![0.45]),
            (0x917, 0xa41, vec![0.45]),
            (0x917, 0xc04, vec![0.001]),
            (0xd90, 0xc02, vec![0.001]),
            (0x66d, 0xa22, vec![0.02]),
            (0x66e, 0xa1c, vec![0.02]),
            (0x66e, 0xa12, vec![0.02]),
            (0x66f, 0xa3b, vec![0.02]),
            (0x66f, 0xa0a, vec![0.02]),
            (0x670, 0xa2f, vec![0.02]),
            (0x670, 0xa3d, vec![0.02]),
            (0x670, 0xab5, vec![0.01]),
            (0x671, 0xa17, vec![0.02]),
            (0x672, 0xa3c, vec![0.02]),
            (0x672, 0xa32, vec![0.02]),
        ] {
            let actual: Vec<_> = table(kind)
                .iter()
                .filter_map(|(chance, pick)| {
                    matches!(pick, Pick::Item(id) if *id == item).then_some(*chance)
                })
                .collect();
            assert_eq!(actual, expected, "kind={kind:#x} item={item:#x}");
            assert!(period_dungeon(kind));
        }
    }

    #[test]
    fn period_dungeon_rows_validate_without_reclassifying_rewards() {
        let content = Content::load(std::path::Path::new("data")).unwrap();
        for kind in content
            .objects
            .keys()
            .copied()
            .filter(|k| period_dungeon(*k))
        {
            validate(&content, kind).unwrap();
        }
        for (kind, tier, slots, expected) in [
            (0x683, 1, ARMOR, 0.2),
            (0x683, 2, ARMOR, 0.1),
            (0x927, 1, ARMOR, 0.4),
            (0x927, 4, ARMOR, 0.05),
            (0x68f, 1, ABILITIES, 0.2),
            (0xd01, 2, ABILITIES, 0.04),
            (0xd02, 6, ARMOR, 0.01),
            (0xd06, 7, ARMOR, 0.05),
            (0xd06, 4, ABILITIES, 0.024),
            (0x917, 10, WEAPONS, 0.05),
            (0x917, 9, ARMOR, 0.3),
            (0x917, 10, ARMOR, 0.05),
            (0xd90, 5, ABILITIES, 0.05),
            (crate::dungeon::abyss::MALPHAS, 9, WEAPONS, 0.4),
            (crate::dungeon::abyss::MALPHAS, 7, ARMOR, 0.7),
            (crate::dungeon::abyss::MALPHAS, 9, ARMOR, 0.4),
            (crate::dungeon::abyss::MALPHAS, 10, ARMOR, 0.15),
        ] {
            assert!(
                table(kind).iter().any(|(chance, pick)| {
                    *chance == expected
                        && matches!(pick, Pick::Tier(t, s) if *t == tier && *s == slots)
                }),
                "{kind:#x} tier={tier}"
            );
        }
        for kind in [0x917, 0xd06, 0xd90, crate::dungeon::abyss::MALPHAS] {
            assert!(table(kind).iter().any(|(chance, pick)| {
                *chance == 0.006 && matches!(pick, Pick::Item(crate::endgame::INCANTATION))
            }));
        }
        // These reconstruction rows must not inherit the related boss's period rates.
        for kind in [crate::dungeon::snake::GUARD, crate::dungeon::abyss::IDOL] {
            assert!(!period_dungeon(kind));
            assert!(private(kind));
            assert!(
                table(kind)
                    .iter()
                    .any(|(_, pick)| { matches!(pick, Pick::Item(crate::endgame::INCANTATION)) })
            );
        }
        assert!(!private(crate::dungeon::spider::BOSS));
        assert!(!private(0xd03));
    }

    #[test]
    fn coil_chest_limits_successes_without_filling_failed_rolls() {
        let content = Content::load(std::path::Path::new("data")).unwrap();
        let kind = crate::dungeon::jungle::COIL;
        let items = landmark_chest(&content, kind, &mut StepRng::new(0, 0)).unwrap();
        assert_eq!(items.len(), 8);
        assert_eq!(items.iter().filter(|item| **item >= 0).count(), 3);
        let empty = landmark_chest(&content, kind, &mut StepRng::new(0, u64::MAX)).unwrap();
        assert_eq!(empty, vec![-1; 8]);
    }

    #[test]
    fn period_landmark_chests_roll_tiers_and_preserve_both_guaranteed_potions() {
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
        for kind in [0x675, 0x678, 0x67d, 0x668] {
            let all = landmark_chest(&content, kind, &mut StepRng::new(0, 0)).unwrap();
            assert_eq!(all.len(), chest_table(kind).len());
            assert!(
                all.len() > 8,
                "packaging must split, not truncate successful rolls"
            );
            for seed in 0..64 {
                let items =
                    landmark_chest(&content, kind, &mut StdRng::seed_from_u64(seed)).unwrap();
                assert_eq!(
                    items
                        .iter()
                        .filter(|id| content.objects[&(**id as u16)].slot_type == 10)
                        .count(),
                    2
                );
            }
        }
        assert_eq!(signature(0x675)[0].0, 0.75);
        assert_eq!(signature(0x67d)[0].0, 0.5);
        assert_eq!(signature(0x668)[0].0, 0.5);
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
    fn period_tables_roll_independently_and_keep_repeated_potions() {
        let content = Content::load(std::path::Path::new("data")).unwrap();
        for (kind, potion_count, weapon_tier) in [(0xd56, 3, 8), (0xd59, 3, 8), (0xd60, 3, 8)] {
            let items = rewards(&content, kind, &mut StepRng::new(0, 0));
            assert_eq!(
                items
                    .iter()
                    .filter(|id| content.objects[id].tier == Some(2)
                        && content.objects[id].slot_type == 10)
                    .count(),
                potion_count
            );
            assert!(
                items
                    .iter()
                    .any(|id| content.objects[id].tier == Some(weapon_tier)
                        && WEAPONS.contains(&content.objects[id].slot_type))
            );
            assert!(
                items.iter().any(|id| content.objects[id].tier == Some(12)),
                "wiki T11 armor maps to selected T12"
            );
        }
        let items = rewards(&content, 0x655, &mut StepRng::new(0, 0));
        assert!(items.contains(&0xa20));
        for (tier, slots) in [(6, WEAPONS), (7, WEAPONS), (8, WEAPONS), (4, RINGS)] {
            assert!(items.iter().any(|id| content.objects[id].tier == Some(tier)
                && slots.contains(&content.objects[id].slot_type)));
        }
        assert!(
            rewards(&content, 0x653, &mut StepRng::new(0, 0)).is_empty(),
            "Sprite Child is not its parent"
        );
        let chance = |kind, tier, slots: &[i32]| {
            table(kind).iter().find_map(|(chance, pick)| {
                matches!(pick, Pick::Tier(t, s) if *t == tier && *s == slots).then_some(*chance)
            })
        };
        assert_eq!(chance(0xd56, 8, WEAPONS), Some(0.4));
        assert_eq!(chance(0xd60, 8, WEAPONS), Some(0.08));
        assert_eq!(chance(0xd56, 12, ARMOR), Some(0.025));
        assert_eq!(chance(0xd60, 12, ARMOR), Some(0.005));
        assert_eq!(content.objects[&0xa12].tier, Some(8));
        assert_eq!(chance(crate::endgame::ORYX, 10, WEAPONS), Some(0.07));
        assert_eq!(chance(crate::endgame::ORYX2, 13, ARMOR), Some(0.05));
    }

    #[test]
    fn limon_defense_is_an_independent_five_percent_private_bonus() {
        let content = Content::load(std::path::Path::new("data")).unwrap();
        assert!(private(0xd06));
        let &(chance, _) = table(0xd06)
            .iter()
            .find(|(_, pick)| matches!(pick, Pick::Item(0xa20)))
            .expect("Limon's Defense drop is missing");
        assert_eq!(chance, 0.05);
        let threshold = (chance * u64::MAX as f64) as u64;
        for (roll, defense) in [(threshold - 1, true), (threshold, false)] {
            assert_eq!(StepRng::new(roll, 0).gen_bool(chance), defense);
        }
        let mut bonuses = 0;
        for seed in 0..1024 {
            let items = rewards(&content, 0xd06, &mut StdRng::seed_from_u64(seed));
            assert_eq!(items.iter().filter(|item| **item == 0xa4c).count(), 1);
            assert!(items.iter().filter(|item| **item == 0xa20).count() <= 1);
            bonuses += usize::from(items.contains(&0xa20));
        }
        assert!((1..1024).contains(&bonuses), "Defense must remain optional");
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
