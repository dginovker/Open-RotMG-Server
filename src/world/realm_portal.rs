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
    pub(super) fn install_realm_portal(
        &mut self,
        realm: i32,
        retiring: Option<i32>,
    ) -> Result<i32> {
        let nexus = self.worlds.get_mut(&-2).context("Nexus missing")?;
        let entrances: Vec<_> = nexus
            .entities
            .values()
            .filter(|e| e.kind == 0x712 && Some(e.id) != retiring)
            .map(|e| e.pos)
            .collect();
        // Region sampling: https://github.com/Zolmex/kithio-as3-source/blob/d15f321edcce0ef890001890c6991c25a1b0f718/Server%20Source/wServer/realm/RealmPortalMonitor.cs#L36-L47
        // Four-tile clearance is reconstruction/usability policy, not a historical constant.
        let positions: Vec<_> = nexus
            .map
            .cells
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let pos = Pos {
                    x: (i % nexus.map.width as usize) as f32 + 0.5,
                    y: (i / nexus.map.width as usize) as f32 + 0.5,
                };
                (c.region == 2
                    && c.object.is_none()
                    && nexus.walkable(pos)
                    && entrances.iter().all(|other| other.distance(pos) >= 4.))
                .then_some(pos)
            })
            .collect();
        let pos = *positions.choose(&mut nexus.rng).with_context(|| {
            format!(
                "Realm {realm}: Nexus has no separated objectless walkable region-2 entrance cell"
            )
        })?;
        let id = nexus.spawn_entity(0x712, pos, vec![])?;
        nexus.entities.get_mut(&id).unwrap().published = false;
        tracing::info!(
            realm,
            portal = id,
            x = pos.x,
            y = pos.y,
            "Realm entrance allocated"
        );
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

#[cfg(test)]
#[test]
fn realm_portal_counts_reservations_and_retires_closed_entrances() {
    let path = std::path::Path::new("data");
    let content = Arc::new(Content::load(path).unwrap());
    let mut d = Directory::test_with_seed(path, content, 1, 901).unwrap();
    d.grow_realm_for_test();
    let portal = d.endgame.realms[&-3].portal.unwrap();
    let other = d.endgame.realms[&1].portal.unwrap();
    let names: BTreeSet<_> = d.endgame.realms.values().map(|r| r.name).collect();
    assert_eq!(names.len(), 2);
    assert_eq!(d.worlds[&-2].entities[&portal].extra_stats, vec![(37, 1)]);
    assert!(d.worlds[&-2].entities[&portal].name.ends_with("(0/1)"));
    d.issue(1, 1, -3).unwrap();
    d.sync_realm_portal();
    let entrance = &d.worlds[&-2].entities[&portal];
    assert!(entrance.published);
    assert!(entrance.name.ends_with("(1/1)"));
    assert_eq!(entrance.extra_stats, vec![(37, 0)]);
    assert!(d.worlds[&-2].entities[&other].name.ends_with("(0/1)"));
    assert_eq!(d.worlds[&-2].entities[&other].extra_stats, vec![(37, 1)]);
    assert!(d.issue(2, 2, -3).unwrap_err().to_string().contains("full"));
    d.worlds.get_mut(&-3).unwrap().admission_open = false;
    d.sync_realm_portal();
    // Closed Realms disappear in the primary comparator; LockedPortal is a different client definition.
    // https://github.com/Zolmex/kithio-as3-source/blob/d15f321edcce0ef890001890c6991c25a1b0f718/Server%20Source/wServer/realm/RealmPortalMonitor.cs#L80-L89
    assert!(!d.worlds[&-2].entities[&portal].published);
    assert!(d.worlds[&-2].entities[&other].published);
    assert!(
        d.issue(2, 2, -3)
            .unwrap_err()
            .to_string()
            .contains("closed")
    );
    d.tick += 2400;
    d.retire_transfer_grants();
    d.worlds.get_mut(&-3).unwrap().admission_open = true;
    d.sync_realm_portal();
    let entrance = &d.worlds[&-2].entities[&portal];
    assert!(entrance.published);
    assert!(entrance.name.ends_with("(0/1)"));
    assert_eq!(entrance.extra_stats, vec![(37, 1)]);
}

#[cfg(test)]
#[test]
fn realm_entrance_seeded_legal_stationary_and_bounded() {
    let path = std::path::Path::new("data");
    let content = Arc::new(Content::load(path).unwrap());
    let mut d = Directory::prepared(path, content.clone(), 16, 901, 2).unwrap();
    let mut same = Directory::prepared(path, content, 16, 901, 2).unwrap();
    d.grow_realm_for_test();
    same.grow_realm_for_test();
    let positions = |d: &Directory| {
        d.endgame
            .realms
            .values()
            .map(|r| d.worlds[&-2].entities[&r.portal.unwrap()].pos)
            .collect::<Vec<_>>()
    };
    let initial = positions(&d);
    assert!(
        initial
            .iter()
            .zip(positions(&same))
            .all(|(a, b)| a.distance(b) == 0.)
    );
    assert!(initial[0].distance(initial[1]) >= 4.);
    for pos in &initial {
        let nexus = &d.worlds[&-2];
        let cell = &nexus.map.cells[pos.y as usize * nexus.map.width as usize + pos.x as usize];
        assert_eq!(cell.region, 2);
        assert!(cell.object.is_none() && nexus.walkable(*pos));
        assert_eq!(pos.x.fract(), 0.5);
        assert_eq!(pos.y.fract(), 0.5);
    }
    let old = d.endgame.realms[&-3].portal.unwrap();
    let old_pos = d.worlds[&-2].entities[&old].pos;
    let mut choices = BTreeSet::new();
    for seed in 0..16 {
        d.worlds.get_mut(&-2).unwrap().rng = rand::rngs::StdRng::seed_from_u64(seed);
        let replacement = d.install_realm_portal(10, Some(old)).unwrap();
        let pos = d.worlds[&-2].entities[&replacement].pos;
        choices.insert((pos.x as i32, pos.y as i32));
        assert!(pos.distance(initial[1]) >= 4.);
        assert!(
            positions(&d)
                .iter()
                .zip(&initial)
                .all(|(a, b)| a.distance(*b) == 0.),
            "existing entrances moved"
        );
        d.worlds.get_mut(&-2).unwrap().remove_entity(replacement);
    }
    assert!(choices.len() > 1, "different seeds still use a fixed slot");
    let nexus = d.worlds.get_mut(&-2).unwrap();
    let old_index = old_pos.y as usize * nexus.map.width as usize + old_pos.x as usize;
    for (i, cell) in Arc::make_mut(&mut nexus.map).cells.iter_mut().enumerate() {
        cell.region = if i == old_index { 2 } else { 0 };
    }
    let count = nexus.entities.len();
    assert!(
        d.install_realm_portal(10, None)
            .unwrap_err()
            .to_string()
            .contains("no separated")
    );
    assert_eq!(d.worlds[&-2].entities.len(), count);
    let replacement = d.install_realm_portal(10, Some(old)).unwrap();
    assert_eq!(
        d.worlds[&-2].entities[&replacement].pos.distance(old_pos),
        0.,
        "legal old-tile reuse refused"
    );
    assert!(
        d.install_realm_portal(11, Some(old)).is_err(),
        "unpublished replacement must reserve space"
    );
    d.worlds.get_mut(&-2).unwrap().remove_entity(replacement);
    let nexus = d.worlds.get_mut(&-2).unwrap();
    Arc::make_mut(&mut nexus.map).cells[old_index].object = Some(0x100);
    assert!(
        d.install_realm_portal(10, Some(old)).is_err(),
        "object-bearing region accepted"
    );
    let blocked_ground = *d.worlds[&-2]
        .content
        .grounds
        .iter()
        .find(|(_, speed)| **speed <= 0.)
        .unwrap()
        .0;
    let cell = &mut Arc::make_mut(&mut d.worlds.get_mut(&-2).unwrap().map).cells[old_index];
    cell.object = None;
    cell.ground = blocked_ground;
    assert!(
        d.install_realm_portal(10, Some(old)).is_err(),
        "unwalkable region accepted"
    );
}
