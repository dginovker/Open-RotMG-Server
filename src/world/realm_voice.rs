// GPL-3.0 reference: realm-src@1855c017 wServer/realm/Oryx.cs; NR-Core and Skilly.
use super::*;

pub(super) fn family_name(kind: u16) -> Option<&'static str> {
    Some(match kind {
        0x668 => "Red Demon",
        0x67d => "Cyclops God",
        0x91f | 0x920 => "Ent Ancient",
        0x91b | 0x91c => "Lich",
        0x928 | 0x92d => "Ghost King",
        0x675 | 0x6c1 | 0x6c2 => "Phoenix Lord",
        0x678 => "Oasis Giant",
        0xd56 => "Skull Shrine",
        0xd59 => "Cube God",
        0xd5f => "Pentaract",
        0xd54 => "Grand Sphinx",
        0xd61 => "Hermit God",
        0xd50 => "Lord of the Lost Lands",
        0xe37 => "Ghost Ship",
        _ => return None,
    })
}
impl World {
    pub(super) fn realm_killed_notice(&self, kind: u16, player: &str) {
        let text = match kind {
            0xd56 => format!("{player}, you will rue the day you dared to defile my Skull Shrine!"),
            0xd59 => format!(
                "{player}, you may have destroyed my Cube God but you will never defeat me!"
            ),
            0xd5f => format!("{player}, by destroying my Pentaract you have sealed your own doom!"),
            0xd54 => format!(
                "My Grand Sphinx had lived for thousands of years! You, {player}, will not survive the day!"
            ),
            0xd61 => format!(
                "My Hermit God was more than you'll ever be, {player}. I will kill you myself!"
            ),
            0xd50 => format!(
                "You win this time, {player}, but mark my words:  You will fall before the day is done."
            ),
            0xe37 => format!("How could a creature like {player} defeat my dreaded Ghost Ship?!"),
            // The comparator has no named-kill lines for ordinary heroes; this is explicit reconstruction.
            _ => format!(
                "{player}, you will pay for defeating my {}!",
                family_name(kind).expect("tracked Realm hero family")
            ),
        };
        self.oryx_notice(&text);
    }
    pub(super) fn last_realm_enemy_notice(&self, remaining: &BTreeSet<i32>) {
        if remaining.len() != 1 {
            return;
        }
        let root = *remaining.first().unwrap();
        if let Some(name) = self
            .actors()
            .filter(|e| e.hp > 0 && (e.id == root || e.quest == Some(root)))
            .find_map(|e| family_name(e.kind))
        {
            self.oryx_notice(&format!("A {name} still guards me!"));
        }
    }
}
