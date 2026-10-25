use super::*;

#[derive(Default)]
pub(super) struct Terrain {
    map: std::sync::Weak<Map>,
    content: std::sync::Weak<Content>,
    pub(super) tiles: Arc<crate::navigation::Tiles>,
    geometry: Option<Arc<crate::navigation::Geometry>>,
    pub(super) rebuilds: u64,
    pub(super) incremental_tiles: u64,
    pub(super) cloned_snapshot_tiles: u64,
}
impl Terrain {
    pub(super) fn discover(
        &mut self,
        map: &Arc<Map>,
        content: &Arc<Content>,
        points: &[(i16, i16, u8)],
        known: usize,
    ) {
        if points.is_empty()
            || !self.map.ptr_eq(&Arc::downgrade(map))
            || !self.content.ptr_eq(&Arc::downgrade(content))
        {
            return;
        }
        assert_eq!(
            self.tiles.len() + points.len(),
            known,
            "navigation discovery delta missing"
        );
        Arc::make_mut(&mut self.tiles).discover(
            self.geometry
                .as_ref()
                .expect("navigation producer missing geometry"),
            points.iter().map(|&(x, y, _)| (x, y)),
        );
        self.incremental_tiles += points.len() as u64;
    }
}

#[derive(Default)]
pub(super) struct GeometryCache {
    map: std::sync::Weak<Map>,
    content: std::sync::Weak<Content>,
    geometry: Option<Arc<crate::navigation::Geometry>>,
}
impl GeometryCache {
    fn get(&mut self, map: &Arc<Map>, content: &Arc<Content>) -> Arc<crate::navigation::Geometry> {
        if !self.map.ptr_eq(&Arc::downgrade(map)) || !self.content.ptr_eq(&Arc::downgrade(content))
        {
            self.geometry = Some(crate::navigation::Geometry::new(
                (map.width, map.height),
                map.cells.iter().map(|c| c.ground).collect(),
                &content.grounds,
            ));
            self.map = Arc::downgrade(map);
            self.content = Arc::downgrade(content);
        }
        self.geometry
            .as_ref()
            .expect("navigation geometry not initialized")
            .clone()
    }
}

#[derive(Debug, Serialize)]
pub struct Session {
    pub world: i32,
    pub player: i32,
    pub account: i32,
    pub character: i32,
}

#[derive(Clone, Debug, Serialize)]
pub struct ObservedDungeonEntry {
    // A globally allocated source actor can complete only one departure; no destination identity leaks.
    pub sequence: i32,
    pub source_world: i32,
    pub portal: i32,
    pub portal_kind: u16,
    pub entrant: i32,
    pub account: i32,
    pub name: String,
    pub observed_ms: u64,
    pub expires_ms: u64,
}

#[derive(Serialize)]
pub struct PlayerView {
    pub session: Session,
    pub tick: u64,
    pub time_ms: u64,
    #[serde(skip)]
    pub(crate) clock_at: Instant,
    pub elapsed_ms: u64,
    pub position: Pos,
    pub quest: Option<i32>,
    pub map: MapInfo,
    pub character: OwnCharacter,
    pub lifecycle: Lifecycle,
    pub tiles: Arc<crate::navigation::Tiles>,
    pub objects: Vec<VisibleObject>,
    pub player_markers: Vec<PlayerMarker>,
    pub projectiles: Vec<VisibleProjectile>,
    pub dungeon_entries: Vec<ObservedDungeonEntry>,
}
#[derive(Serialize)]
pub struct MapInfo {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub allow_teleport: bool,
}
#[derive(Serialize)]
pub struct OwnCharacter {
    pub conditions: i32,
    pub class: u16,
    pub level: i32,
    pub experience: i32,
    pub hp: i32,
    pub mp: i32,
    pub stats: [i32; 8],
    pub base_stats: [i32; 8],
    pub items: Vec<i32>,
    pub inventory_revision: i32,
    pub pirate_caves: u64,
    pub snake_pits: u64,
    pub ability_ready_ms: u64,
}
#[derive(Serialize)]
pub struct Lifecycle {
    pub alive: bool,
    pub death_pending: bool,
    pub departing: bool,
    pub busy: bool,
    pub quarantined: bool,
    pub output: OutputState,
}
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputState {
    Detached,
    Live,
    Lost,
}
#[derive(Clone, Serialize)]
pub struct Tile {
    pub x: i16,
    pub y: i16,
    pub ground: u8,
    pub ground_speed: f32,
}
#[derive(Clone, Debug, Serialize)]
pub struct PlayerMarker {
    pub id: i32,
    pub account: i32,
    pub name: String,
    pub position: Pos,
}
#[derive(Serialize)]
pub struct VisibleObject {
    pub id: i32,
    // Entity IDs change on travel and death; acquaintances bind the public account identity.
    pub account: Option<i32>,
    pub kind: u16,
    pub name: String,
    pub position: Pos,
    pub hp: i32,
    pub max_hp: i32,
    pub level: Option<i32>,
    pub equipped: Vec<i32>,
    pub contents: Vec<i32>,
    pub blocks: bool,
    pub usable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition_mask: Option<i32>,
}
#[derive(Serialize)]
pub struct VisibleProjectile {
    pub owner: i32,
    pub id: u8,
    pub start: Pos,
    pub position: Pos,
    pub angle: f32,
    pub speed: f32,
    pub lifetime_ms: f32,
    pub amplitude: f32,
    pub frequency: f32,
    pub passes_cover: bool,
}

impl Directory {
    pub fn player_view(&self, id: i32) -> Result<PlayerView> {
        let scope = self.location(id)?;
        self.worlds[&scope].player_view(scope, id, self.tick * 50)
    }
}
impl super::Handle {
    pub async fn player_view(&self, id: i32) -> Result<PlayerView> {
        let routing = self.routing.clone();
        self.for_player(id)
            .call_scoped(move |world, scope| {
                world.player_view(
                    scope,
                    id,
                    routing.tick.load(std::sync::atomic::Ordering::Acquire) * 50,
                )
            })
            .await
    }
}
impl World {
    pub(super) fn player_view(&self, scope: i32, id: i32, elapsed_ms: u64) -> Result<PlayerView> {
        let started = crate::performance::Measure::start();
        let w = self;
        let p = w.players.get(&id).context("view player missing")?;
        let visible = w.visible(id);
        let objects = visible
            .intersection(&p.known)
            .filter(|entity| **entity != id)
            .map(|entity| {
                Ok(if let Some(q) = w.players.get(entity) {
                    VisibleObject {
                        id: *entity,
                        account: Some(q.character.account),
                        kind: q.character.class,
                        name: q.character.name.clone(),
                        position: q.pos,
                        hp: q.character.hp,
                        max_hp: q.effective(&w.content, w.now)?[0],
                        level: Some(q.character.level),
                        equipped: q.character.items[..4].to_vec(),
                        contents: vec![],
                        blocks: false,
                        usable: true,
                        condition_mask: None,
                    }
                } else {
                    let e = &w.entities[entity];
                    VisibleObject {
                        id: *entity,
                        account: None,
                        kind: e.kind,
                        name: e.name.clone(),
                        position: e.pos,
                        hp: e.hp,
                        max_hp: w.content.objects[&e.kind].hp,
                        level: None,
                        equipped: vec![],
                        contents: e.items.clone(),
                        blocks: w.content.objects[&e.kind].blocks,
                        usable: !e
                            .extra_stats
                            .iter()
                            .any(|(kind, value)| *kind == 37 && *value == 0),
                        condition_mask: Some(e.conditions(w.now)),
                    }
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let tiles = {
            let mut cache = p.view_tiles.borrow_mut();
            let map = Arc::downgrade(&w.map);
            let content = Arc::downgrade(&w.content);
            // Discovery only grows within a session. Weak identities invalidate copy-on-write changes without retaining an old map.
            if !cache.map.ptr_eq(&map) || !cache.content.ptr_eq(&content) {
                let refreshing = crate::performance::Measure::start();
                cache.rebuilds += 1;
                let geometry = w.navigation_geometry.borrow_mut().get(&w.map, &w.content);
                cache.tiles = Arc::new(crate::navigation::Tiles::new(
                    geometry.clone(),
                    p.tiles.iter().copied(),
                ));
                cache.geometry = Some(geometry);
                cache.map = map;
                cache.content = content;
                w.performance
                    .borrow_mut()
                    .work("terrain_rebuild", refreshing);
            }
            ensure!(
                cache.tiles.len() == p.tiles.len(),
                "navigation knowledge missed discovery delta"
            );
            cache.tiles.clone()
        };
        // A newly discovered shooter does not reveal its earlier, unreceived shots.
        let projectiles = w
            .bullets
            .iter()
            .filter(|b| {
                b.pos.distance(p.pos) < 20.
                    && p.known_since
                        .get(&b.owner)
                        .is_some_and(|since| b.born > *since)
            })
            .map(|b| VisibleProjectile {
                owner: b.owner,
                id: b.id,
                start: b.start,
                position: b.pos,
                angle: b.angle,
                speed: b.spec.speed,
                lifetime_ms: b.spec.lifetime,
                amplitude: b.spec.amplitude,
                frequency: b.spec.frequency,
                passes_cover: b.spec.passes_cover,
            })
            .collect();
        let view = PlayerView {
            session: Session {
                world: scope,
                player: id,
                account: p.character.account,
                character: p.character.id,
            },
            tick: w.tick,
            time_ms: w.now,
            clock_at: w.clock.now(),
            elapsed_ms,
            position: p.pos,
            quest: p.quest.filter(|quest| p.known.contains(quest)),
            map: MapInfo {
                name: w.name.clone(),
                width: w.map.width,
                height: w.map.height,
                allow_teleport: w.allow_teleport(),
            },
            character: OwnCharacter {
                conditions: p.conditions(w.now) | if p.paused { 1 << 20 } else { 0 },
                class: p.character.class,
                level: p.character.level,
                experience: p.character.exp,
                hp: p.character.hp,
                mp: p.character.mp,
                stats: p.effective(&w.content, w.now)?,
                base_stats: p.character.base,
                items: p.character.items.clone(),
                inventory_revision: p.character.revision,
                pirate_caves: p.character.pirate_caves,
                snake_pits: p.character.dungeons[0],
                ability_ready_ms: w
                    .now
                    .checked_add(p.ability_ready_ms.saturating_sub(w.ability_clock_ms()?))
                    .context("ability view cooldown overflow")?,
            },
            lifecycle: Lifecycle {
                alive: p.character.alive,
                death_pending: p.death_pending,
                departing: p.departing,
                busy: p.pending,
                quarantined: p.persistence_failure.is_some(),
                output: match &p.tx {
                    None => OutputState::Detached,
                    Some(tx) if tx.lost.borrow().is_some() || tx.packets.is_closed() => {
                        OutputState::Lost
                    }
                    Some(_) => OutputState::Live,
                },
            },
            tiles,
            objects,
            player_markers: w
                .player_markers(id)
                .map(|(id, q)| PlayerMarker {
                    id,
                    account: q.character.account,
                    name: q.character.name.clone(),
                    position: q.pos,
                })
                .collect(),
            projectiles,
            dungeon_entries: p
                .dungeon_entries
                .iter()
                .filter(|e| {
                    e.expires_ms > elapsed_ms
                        && e.source_world == scope
                        && dungeon_entry::observing(p)
                        && w.owner.is_none()
                        && w.guild.is_none()
                        && visible.contains(&e.portal)
                        && p.known.contains(&e.portal)
                        && w.entities.get(&e.portal).is_some_and(|portal| {
                            portal.published
                                && portal.kind == e.portal_kind
                                && portal.conditions(w.now)
                                    & crate::condition::bit(crate::condition::INVISIBLE)
                                    == 0
                        })
                })
                .cloned()
                .collect(),
        };
        w.performance.borrow_mut().work("perception", started);
        Ok(view)
    }
}
