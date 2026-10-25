use super::*;

#[test]
fn pirate_instances_generate_replayable_maps_without_a_static_template() {
    let path = std::path::Path::new("data");
    let content = Arc::new(Content::load(path).unwrap());
    let mut d = Directory::test_with_seed(path, content, 20, 423).unwrap();
    d.prepared_seed = Some(423);
    assert!(!d.templates.contains_key("pirate-cave"));
    let mut layouts = Vec::new();
    for seed in [17, 17, 18] {
        let id = d.prepared_instance_with_seed("pirate-cave", seed).unwrap();
        let w = &d.worlds[&id];
        assert_eq!(w.name, "Pirate Cave");
        assert!(matches!(w.return_route, Some(DungeonReturn::Random)));
        assert_eq!(
            w.entities
                .values()
                .filter(|e| e.kind == crate::dungeon::pirate::BOSS)
                .count(),
            1
        );
        layouts.push((id, serde_json::to_value(w.map.as_ref()).unwrap()));
    }
    assert_ne!(layouts[0].0, layouts[1].0);
    assert_eq!(layouts[0].1, layouts[1].1);
    assert_ne!(layouts[0].1, layouts[2].1);
}

#[tokio::test]
async fn generated_pirate_portal_shares_one_instance_and_keeps_current_exit() {
    let path = std::path::Path::new("data");
    let content = Arc::new(Content::load(path).unwrap());
    let store = crate::store::Store::open(std::path::Path::new(":memory:"), &content).unwrap();
    let mut d = Directory::test_with_seed(path, content, 20, 423).unwrap();
    d.prepared_seed = Some(423);
    let (x, y) = d.worlds[&-3].map.spawns[0];
    let portal = d.drop_dungeon_portal(-3, 0x717, Pos { x, y }).unwrap();
    let cave = d.portals[&portal].destination.unwrap();
    assert!(matches!(
        d.worlds[&cave].return_route,
        Some(DungeonReturn::Realm(-3))
    ));
    let entrance = d.worlds[&-3].entities[&portal].pos;
    for i in 0..2 {
        let account = store.list(format!("generated-pirate-{i}")).await.unwrap();
        let character = store
            .create(account.id, d.content.get(crate::content::WIZARD).unwrap())
            .await
            .unwrap();
        let id = d
            .worlds
            .get_mut(&-3)
            .unwrap()
            .enter(character, None)
            .unwrap();
        d.worlds
            .get_mut(&-3)
            .unwrap()
            .players
            .get_mut(&id)
            .unwrap()
            .pos = entrance;
        assert_eq!(d.destination(id, Some(portal)).unwrap().2, cave);
    }
    let account = store.list("generated-pirate-return".into()).await.unwrap();
    let character = store
        .create(account.id, d.content.get(crate::content::WIZARD).unwrap())
        .await
        .unwrap();
    let player = d
        .worlds
        .get_mut(&cave)
        .unwrap()
        .enter(character, None)
        .unwrap();
    let exit = d.worlds[&cave]
        .entities
        .values()
        .find(|e| e.kind == 0x703)
        .unwrap()
        .id;
    // Source-Realm metadata does not override Pirate Cave's explicit Nexus exit.
    assert_eq!(d.destination(player, Some(exit)).unwrap().2, -2);
}
