use super::*;

fn room() -> (World, i32) {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let mut map = Map {
        width: 48,
        height: 44,
        cells: vec![
            crate::content::Cell {
                ground: 0x16,
                object: None,
                config: String::new(),
                terrain: 0,
                region: 0,
            };
            48 * 44
        ],
        spawns: vec![(8.5, 20.5)],
    };
    for y in 0..44 {
        for x in [12, 13] {
            map.cells[y * 48 + x].object = Some(0x172);
        }
    }
    for x in [12, 13] {
        map.cells[19 * 48 + x].ground = 0xff;
        map.cells[19 * 48 + x].object = None;
    }
    map.cells[20 * 48 + 16].object = Some(0x225);
    let mut world = World::new(content, Arc::new(map)).unwrap();
    world.name = "Snake Pit".into();
    let base = world.content.classes[&crate::content::WIZARD].caps;
    let ch: Character = serde_json::from_value(serde_json::json!({
        "schema":1,"lifetime":crate::store::FameLifetime::complete(),
        "class":crate::content::WIZARD,"base":base,"id":1,"account":1,
        "name":"Sight","level":20,"exp":0,"fame":0,"hp":base[0],"mp":base[1],
        "revision":0,"alive":true,"items":[-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1]
    }))
    .unwrap();
    let id = world.enter(ch, None).unwrap();
    (world, id)
}

#[test]
fn snake_sight_hides_tiles_and_objects_until_walls_are_destroyed_without_moving() {
    let (mut w, id) = room();
    let mut peer = w.players[&id].character.clone();
    peer.id = 2;
    peer.account = 2;
    peer.name = "Peer".into();
    let peer = w.enter(peer, None).unwrap();
    w.updates(peer, true).unwrap();
    w.updates(id, true).unwrap();
    let front = w.entities_at(Pos { x: 12.5, y: 20.5 }).next().unwrap().id;
    let back = w.entities_at(Pos { x: 13.5, y: 20.5 }).next().unwrap().id;
    let snake = w.entities_at(Pos { x: 16.5, y: 20.5 }).next().unwrap().id;
    assert!(w.players[&id].tiles.contains(&(12, 20)));
    assert!(!w.players[&id].tiles.contains(&(13, 20)));
    assert!(!w.players[&id].tiles.contains(&(16, 20)));
    assert!(w.players[&id].known.contains(&front));
    assert!(!w.players[&id].known.contains(&back));
    assert!(!w.players[&id].known.contains(&snake));
    let original = w.players[&id].tiles.clone();
    let count = w.players[&id].character.tiles;
    w.updates(id, false).unwrap();
    assert_eq!(w.players[&id].character.tiles, count);
    w.defeat_entity(front).unwrap();
    w.updates(id, false).unwrap();
    assert!(w.players[&id].tiles.contains(&(13, 20)));
    assert!(w.players[&id].known.contains(&back));
    assert!(!w.players[&id].tiles.contains(&(16, 20)));
    w.defeat_entity(back).unwrap();
    w.updates(id, false).unwrap();
    w.updates(peer, false).unwrap();
    assert!(w.players[&peer].tiles.contains(&(16, 20)));
    assert!(w.players[&id].tiles.contains(&(16, 20)));
    assert!(w.players[&id].known.contains(&snake));
    assert!(original.is_subset(&w.players[&id].tiles));
    assert_eq!(
        w.players[&id].character.tiles as usize,
        w.players[&id].tiles.len()
    );
    assert!(!w.players[&id].tiles.contains(&(29, 20)));
    assert!(w.players[&id].tiles.contains(&(0, 20)));
    let closed = w
        .spawn_entity(0x170, Pos { x: 12.5, y: 20.5 }, vec![])
        .unwrap();
    w.updates(id, false).unwrap();
    assert!(!w.players[&id].known.contains(&snake));
    assert!(w.players[&id].tiles.contains(&(16, 20)));
    w.remove_entity(closed);
    w.updates(id, false).unwrap();
    assert!(w.players[&id].known.contains(&snake));
}

#[test]
fn other_worlds_keep_circular_discovery_and_snake_grates_do_not_block_sight() {
    let (mut w, id) = room();
    w.name = "Nexus".into();
    w.updates(id, true).unwrap();
    assert!(w.players[&id].tiles.contains(&(16, 20)));
    let (mut w, id) = room();
    let wall_ids: Vec<_> = w
        .entities
        .values()
        .filter(|e| e.kind == 0x172)
        .map(|e| e.id)
        .collect();
    for wall in wall_ids {
        w.remove_entity(wall);
    }
    w.spawn_entity(0x173, Pos { x: 12.5, y: 20.5 }, vec![])
        .unwrap();
    w.updates(id, true).unwrap();
    assert!(w.players[&id].tiles.contains(&(16, 20)));
}
