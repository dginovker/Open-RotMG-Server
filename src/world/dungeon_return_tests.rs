use super::*;

async fn fixture(home: i32) -> (Directory, i32, i32, i32) {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let store = crate::store::Store::open(std::path::Path::new(":memory:"), &content).unwrap();
    let mut d = Directory::test_with_seed(std::path::Path::new("data"), content, 20, 423).unwrap();
    let pos = d.worlds[&home].map.spawns[0];
    let entrance = d
        .drop_dungeon_portal(home, 0x718, Pos { x: pos.0, y: pos.1 })
        .unwrap();
    let dungeon = d.portals[&entrance].destination.unwrap();
    let mut players = Vec::new();
    for (i, world) in [dungeon, home].into_iter().enumerate() {
        let account = store.list(format!("return-{i}")).await.unwrap();
        let ch = store
            .create(account.id, d.content.get(crate::content::WIZARD).unwrap())
            .await
            .unwrap();
        players.push(d.worlds.get_mut(&world).unwrap().enter(ch, None).unwrap());
    }
    d.worlds.get_mut(&home).unwrap().max_players = 1;
    let player = players[0];
    let pos = d.worlds[&dungeon].players[&player].pos;
    let exit = d
        .worlds
        .get_mut(&dungeon)
        .unwrap()
        .spawn_entity(0x704, pos, vec![])
        .unwrap();
    (d, dungeon, player, exit)
}

#[tokio::test]
async fn dungeon_return_retains_each_source_realm_after_entrance_expires() {
    for home in [-3, 1] {
        let (mut d, dungeon, player, exit) = fixture(home).await;
        d.tick = 601;
        d.step();
        assert!(d.portals.is_empty());
        let before = serde_json::to_value(&d.worlds[&dungeon].players[&player].character).unwrap();
        for kind in [0x704, 0x70e, 0x703] {
            d.worlds
                .get_mut(&dungeon)
                .unwrap()
                .entities
                .get_mut(&exit)
                .unwrap()
                .kind = kind;
            let (account, character, destination) = d.destination(player, Some(exit)).unwrap();
            assert_eq!(
                destination, home,
                "return portal {kind:#x} selected another Realm"
            );
            assert!(
                d.issue(account, character, destination).is_err(),
                "full source Realm admitted a return"
            );
            assert!(d.grants.is_empty());
            assert_eq!(
                serde_json::to_value(&d.worlds[&dungeon].players[&player].character).unwrap(),
                before
            );
        }
        d.worlds.get_mut(&home).unwrap().max_players = 20;
        let (account, character, destination) = d.destination(player, Some(exit)).unwrap();
        let (_, key, _) = d.issue(account, character, destination).unwrap();
        d.bind_exit(player, Some(exit), &key).unwrap();
        let _reservation = d.reserve_transfer(key.clone()).unwrap();
        assert_eq!(
            d.validate_source_exit(player, &key).unwrap(),
            Some(crate::dungeon::HistoricalDungeon::SnakePit)
        );
        d.worlds.get_mut(&home).unwrap().admission_open = false;
        assert!(d.destination(player, Some(exit)).is_err());
        assert!(d.validate_source_exit(player, &key).is_err());
        assert!(!d.worlds[&dungeon].players[&player].departing);
        assert_eq!(
            serde_json::to_value(&d.worlds[&dungeon].players[&player].character).unwrap(),
            before
        );
    }
}

#[tokio::test]
async fn dungeon_return_rejects_missing_origin_but_preserves_explicit_random_and_nexus_portals() {
    let (mut d, dungeon, player, exit) = fixture(1).await;
    assert!(matches!(
        d.dungeon_return_from(1).unwrap(),
        DungeonReturn::Realm(1)
    ));
    let inherited = d.dungeon_return_from(dungeon).unwrap();
    let child = d.instance("snake-pit", None, Some(inherited)).unwrap();
    assert!(matches!(
        d.dungeon_return_from(child).unwrap(),
        DungeonReturn::Realm(1)
    ));
    let fresh = d
        .instance("snake-pit", None, Some(d.dungeon_return_from(-2).unwrap()))
        .unwrap();
    assert!(matches!(
        d.dungeon_return_from(fresh).unwrap(),
        DungeonReturn::Random
    ));
    assert_eq!(d.dungeon_realm(&d.worlds[&fresh]).unwrap(), -3);
    d.prepared_seed = Some(423);
    let prepared = d.prepared_instance("snake-pit").unwrap();
    assert!(matches!(
        d.dungeon_return_from(prepared).unwrap(),
        DungeonReturn::Random
    ));
    d.worlds
        .get_mut(&dungeon)
        .unwrap()
        .entities
        .get_mut(&exit)
        .unwrap()
        .kind = 0x71c;
    assert_eq!(d.destination(player, Some(exit)).unwrap().2, -3);
    d.worlds
        .get_mut(&dungeon)
        .unwrap()
        .entities
        .get_mut(&exit)
        .unwrap()
        .kind = 0x712;
    assert_eq!(d.destination(player, Some(exit)).unwrap().2, -2);
    assert_eq!(d.destination(player, None).unwrap().2, -2);
    d.worlds.remove(&1);
    d.endgame.realms.remove(&1);
    d.worlds
        .get_mut(&dungeon)
        .unwrap()
        .entities
        .get_mut(&exit)
        .unwrap()
        .kind = 0x704;
    assert!(d.destination(player, Some(exit)).is_err());
    let count = d.worlds.len();
    assert!(d.instance("snake-pit", None, None).is_err());
    assert_eq!(d.worlds.len(), count);
    d.worlds.get_mut(&dungeon).unwrap().return_route = None;
    assert!(d.dungeon_return_from(dungeon).is_err());
    assert!(d.destination(player, Some(exit)).is_err());
}

#[tokio::test]
async fn dreadstump_completion_queues_credit_and_routes_realm_portal_exit() {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let store = crate::store::Store::open(std::path::Path::new(":memory:"), &content).unwrap();
    let mut d = Directory::test_with_seed(std::path::Path::new("data"), content, 20, 423).unwrap();
    let pos = d.worlds[&1].map.spawns[0];
    let entrance = d
        .drop_dungeon_portal(1, 0x717, Pos { x: pos.0, y: pos.1 })
        .unwrap();
    let cave = d.portals[&entrance].destination.unwrap();
    let account = store.list("dreadstump".into()).await.unwrap();
    let ch = store
        .create(account.id, d.content.get(crate::content::WIZARD).unwrap())
        .await
        .unwrap();
    let w = d.worlds.get_mut(&cave).unwrap();
    let player = w.enter(ch, None).unwrap();
    let mut boss = w
        .entities
        .values()
        .find(|e| e.kind == 0x927)
        .unwrap()
        .clone();
    w.players.get_mut(&player).unwrap().pos = boss.pos;
    boss.damage.insert(w.players[&player].character.id, 1);
    w.drop_loot(&boss).unwrap();
    assert_eq!(w.players[&player].clears_queued[0], 1);
    let exit = w.clear_exit.unwrap();
    assert_eq!(w.entities[&exit].kind, 0x704);
    w.entities.get_mut(&exit).unwrap().published = true;
    w.players.get_mut(&player).unwrap().pos = w.entities[&exit].pos;
    assert_eq!(d.destination(player, Some(exit)).unwrap().2, 1);
}
