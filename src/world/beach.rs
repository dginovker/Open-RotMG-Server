use super::*;
use crate::{
    condition,
    dungeon::beach::{BOSS, BUM, TAUNTS},
};

impl World {
    pub(super) fn beach_notice(&mut self) {
        if self.players.is_empty() {
            return;
        }
        let unannounced = self
            .actors()
            .filter(|e| e.kind == BUM && e.stage.is_none())
            .map(|e| e.id)
            .collect::<Vec<_>>();
        for id in unannounced {
            self.entities.get_mut(&id).unwrap().stage = Some(0);
            self.broadcast_text(
                "Oryx the Mad God",
                "An elusive Beach Bum is hiding somewhere in the Realm.",
            );
        }
    }
    pub(super) fn populate_beach_bum(&mut self) -> Result<()> {
        let pos = self.place_named_setpiece(BUM, &[3, 4, 5, 6, 7])?;
        self.spawn_entity(BUM, pos, vec![])?;
        tracing::info!(
            x = pos.x,
            y = pos.y,
            "rare Beach Bum placed without quest marker"
        );
        Ok(())
    }

    // Silent recovery is described by the period guide; the later reference supplies the 5s reconstruction timer.
    // https://web.archive.org/web/20121201000000id_/http://forums.wildshadow.com/wiki/realm-mad-god/masked-party-god
    pub(super) fn beach_mechanics(&mut self, id: i32) -> Result<()> {
        if self.entities[&id].kind == BUM {
            return Ok(());
        }
        let e = self
            .entities
            .get_mut(&id)
            .context("Beachzone actor disappeared")?;
        ensure!(e.kind == BOSS, "unregistered Beachzone actor");
        let started = *e.activated.get_or_insert(self.now);
        if e.stage.is_none() {
            e.next_aux = self.now + 5000;
        }
        let beat = (self.now - started) / 500;
        let changed = e.stage != Some(beat);
        e.stage = Some(beat);
        if self.now >= e.next_aux
            && e.conditions(self.now) & condition::bit(condition::STUNNED) == 0
        {
            let before = e.hp;
            e.hp = self.content.objects[&BOSS].hp;
            e.next_aux = self.now + 5000;
            tracing::debug!(
                entity = id,
                before,
                after = e.hp,
                "Party God silently recovered"
            );
        }
        if changed {
            self.landmark_texture(id, 1 + (beat % 3) as i32);
            if let Some(text) = TAUNTS.get((beat % 12) as usize) {
                let packet = chat::text_packet("#Masked Party God", id, 5, text);
                for p in self.players.values().filter(|p| p.known.contains(&id)) {
                    send(p, packet.id, packet.body.clone());
                }
            }
        }
        Ok(())
    }
}
