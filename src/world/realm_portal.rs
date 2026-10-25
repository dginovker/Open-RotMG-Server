use super::*;

pub(super) const NAMES: &[&str] = &[
    "Medusa", "Beholder", "Cyclops", "Djinn", "Ogre", "Blob", "Sprite", "Demon", "Golem", "Flayer",
    "Spider", "Imp",
];

impl Directory {
    pub(crate) fn prepare_realm_capacity(&mut self, limit: usize) -> Result<i32> {
        ensure!(
            self.clock.paused(),
            "capacity setup requires a paused clock"
        );
        ensure!(
            self.worlds
                .values()
                .all(|w| w.players.is_empty() && w.reserved_slots() == 0),
            "capacity setup requires no enrolled players"
        );
        let target = -3;
        let realm = self
            .worlds
            .get_mut(&target)
            .context("active Realm missing")?;
        ensure!(realm.name == "Realm", "active world is not a Realm");
        ensure!(
            limit >= 1 && limit < realm.max_players,
            "capacity must lower the active Realm limit"
        );
        realm.max_players = limit;
        self.sync_realm_portal();
        Ok(target)
    }
    pub(super) fn install_realm_portal(&mut self, slot: usize) -> Result<i32> {
        let nexus = self.worlds.get_mut(&-2).context("Nexus missing")?;
        let mut positions: Vec<Pos> = Vec::new();
        for pos in nexus.map.cells.iter().enumerate().filter_map(|(i, c)| {
            let pos = Pos {
                x: (i % nexus.map.width as usize) as f32 + 0.5,
                y: (i / nexus.map.width as usize) as f32 + 0.5,
            };
            (c.region == 2 && c.object.is_none() && nexus.walkable(pos)).then_some(pos)
        }) {
            if positions.iter().all(|other| other.distance(pos) >= 4.) {
                positions.push(pos);
            }
            if positions.len() > slot {
                break;
            }
        }
        let pos = *positions
            .get(slot)
            .context("Nexus has insufficient separated walkable Realm portal regions")?;
        let id = nexus.spawn_entity(0x712, pos, vec![])?;
        nexus.entities.get_mut(&id).unwrap().published = false;
        Ok(id)
    }
    pub(super) fn sync_realm_portal(&mut self) {
        for (origin, record) in &self.endgame.realms {
            let Some(id) = record.portal else { continue };
            let realm = self.worlds.summary(origin).expect("Realm metadata missing");
            let count = realm.occupancy;
            let limit = realm.max_players;
            let open = record.enabled && realm.admission_open;
            let name = format!("{} ({count}/{limit})", NAMES[record.name]);
            let stats = vec![(37, i32::from(open && count < limit))];
            let nexus = self.worlds.summary(&-2).expect("Nexus metadata missing");
            let display = nexus
                .portal_displays
                .get(&id)
                .expect("Realm entrance metadata missing");
            if display.0 != name || display.1 != stats || display.2 != open {
                let portal = self
                    .worlds
                    .get_mut(&-2)
                    .expect("Nexus missing")
                    .entities
                    .get_mut(&id)
                    .expect("Realm entrance missing");
                tracing::info!(realm = origin, %name, open, "Realm portal updated");
                portal.name = name;
                portal.extra_stats = stats;
                portal.published = open;
            }
        }
    }
}
