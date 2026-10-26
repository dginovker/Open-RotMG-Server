use super::*;
use crate::dungeon::{
    beach::BUM,
    lab::{DEHEXXER, HEXXER},
};

#[tokio::test]
async fn world_effect_index_preserves_births_removal_transform_and_live_effects() {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let mut map = Map::load(std::path::Path::new("data/tutorial.wmap"), &content).unwrap();
    for cell in &mut map.cells {
        cell.ground = 0x61;
        cell.object = None;
    }
    for (x, kind) in [(10, HEXXER), (11, DEHEXXER), (12, BUM), (13, 0x671)] {
        map.cells[10 * map.width as usize + x].object = Some(kind);
    }
    map.spawns = vec![(20.5, 20.5)];
    let mut world = World::new(content.clone(), Arc::new(map)).unwrap();
    let check = |world: &World| {
        let expected: Vec<_> = world
            .actors()
            .filter(|e| matches!(e.kind, BUM | HEXXER | DEHEXXER))
            .map(|e| (e.id, e.kind, e.pos.x.to_bits(), e.pos.y.to_bits()))
            .collect();
        let actual: Vec<_> = world
            .world_effect_actors()
            .map(|e| (e.id, e.kind, e.pos.x.to_bits(), e.pos.y.to_bits()))
            .collect();
        assert_eq!(actual, expected);
    };
    check(&world);
    assert_eq!(world.world_effect_ids.len(), 3);
    let extra = world
        .spawn_entity(HEXXER, Pos { x: 18.5, y: 25.5 }, vec![])
        .unwrap();
    check(&world);
    world.remove_entity(extra);
    check(&world);
    let ordinary = world
        .spawn_entity(0x671, Pos { x: 18.5, y: 30.5 }, vec![])
        .unwrap();
    let bum = world.transform_fauna(ordinary, BUM).unwrap();
    check(&world);
    world.transform_fauna(bum, 0x671).unwrap();
    check(&world);

    let store = crate::store::Store::open(std::path::Path::new(":memory:"), &content).unwrap();
    let account = store.list("world-effects".into()).await.unwrap();
    let ch = store
        .create(account.id, content.get(crate::content::WIZARD).unwrap())
        .await
        .unwrap();
    let player = world.enter(ch, None).unwrap();
    world.players.get_mut(&player).unwrap().pos = Pos { x: 10.5, y: 10.5 };
    world.lab_step();
    assert_eq!(
        world.players[&player].lab_conditions, 0,
        "blue wins overlapping pools"
    );
    let blue = world
        .world_effect_actors()
        .find(|e| e.kind == DEHEXXER)
        .unwrap()
        .id;
    world.remove_entity(blue);
    world.lab_step();
    assert_ne!(world.players[&player].lab_conditions, 0);
    world
        .spawn_entity(DEHEXXER, Pos { x: 10.5, y: 10.5 }, vec![])
        .unwrap();
    world.lab_step();
    assert_eq!(world.players[&player].lab_conditions, 0);
    world.beach_notice();
    assert!(
        world
            .world_effect_actors()
            .filter(|e| e.kind == BUM)
            .all(|e| e.stage == Some(0))
    );
    let announced: Vec<_> = world
        .world_effect_actors()
        .filter(|e| e.kind == BUM)
        .map(|e| (e.id, e.stage))
        .collect();
    world.beach_notice();
    assert_eq!(
        world
            .world_effect_actors()
            .filter(|e| e.kind == BUM)
            .map(|e| (e.id, e.stage))
            .collect::<Vec<_>>(),
        announced
    );
    check(&world);
}
