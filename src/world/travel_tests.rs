use super::*;

fn rejects<T>(result: Result<T>, message: &str) {
    let error = result.err().expect("invalid transfer succeeded");
    assert!(error.to_string().contains(message), "{error:#}");
}

#[test]
fn arachna_realm_exit_preserves_parent_or_explicit_random_route() {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let mut d = Directory::test_with_seed(std::path::Path::new("data"), content, 2, 1308).unwrap();
    for origin in [-3, -2] {
        let route = d.dungeon_return_from(origin).unwrap();
        let dungeon = d.instance("spider-den", None, Some(route)).unwrap();
        let world = d.worlds.get_mut(&dungeon).unwrap();
        let mut ch = Character::fresh(world.content.get(0x30e).unwrap()).unwrap();
        ch.account = origin + 4;
        ch.id = origin + 4;
        let player = world.enter(ch, None).unwrap();
        let exit = world.spawn_exit(0x704, world.players[&player].pos).unwrap();
        world.players.get_mut(&player).unwrap().pos = world.entities[&exit].pos;
        let before = world.players[&player].character.clone();
        let (account, character, destination) = d.destination(player, Some(exit)).unwrap();
        assert_eq!((account, character), (before.account, before.id));
        if origin == -2 {
            assert!(matches!(route, DungeonReturn::Random));
            assert!(d.endgame.realms[&destination].enabled);
            assert!(d.worlds[&destination].admission_open);
            assert_eq!(d.worlds[&destination].name, "Realm");
        } else {
            assert_eq!(destination, origin);
            d.worlds.get_mut(&origin).unwrap().admission_open = false;
            rejects(d.destination(player, Some(exit)), "has closed. Use Nexus");
            assert_eq!(d.destination(player, None).unwrap().2, -2);
            assert_eq!(d.worlds[&dungeon].players[&player].character, before);
            assert!(d.grants.is_empty());
            d.worlds.get_mut(&origin).unwrap().admission_open = true;
        }
    }
}

#[test]
fn travel_denials_bindings_expiry_and_once_only_admission() {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let mut d = Directory::test_with_seed(std::path::Path::new("data"), content, 2, 791).unwrap();
    d.clock.set_paused(true);
    let dungeon = d
        .instance("snake-pit", None, Some(DungeonReturn::Realm(-3)))
        .unwrap();
    let mut ch = Character::fresh(d.content.get(0x310).unwrap()).unwrap();
    ch.account = 1;
    ch.id = 1;
    let id = d
        .worlds
        .get_mut(&dungeon)
        .unwrap()
        .enter(ch.clone(), None)
        .unwrap();
    let exit = d.worlds[&dungeon]
        .entities
        .values()
        .find(|e| e.kind == 0x703)
        .unwrap()
        .id;
    let pos = d.worlds[&dungeon].entities[&exit].pos;
    d.worlds
        .get_mut(&dungeon)
        .unwrap()
        .players
        .get_mut(&id)
        .unwrap()
        .pos = pos;
    let before = d.worlds[&dungeon].players[&id].character.clone();
    assert_eq!(d.destination(id, Some(exit)).unwrap(), (1, 1, -3));
    d.worlds
        .get_mut(&dungeon)
        .unwrap()
        .players
        .get_mut(&id)
        .unwrap()
        .death_pending = true;
    rejects(d.destination(id, None), "character cannot transfer");
    d.worlds
        .get_mut(&dungeon)
        .unwrap()
        .players
        .get_mut(&id)
        .unwrap()
        .death_pending = false;
    let vault = d.instance("vault", Some(2), None).unwrap();
    rejects(d.issue(1, 1, vault), "private destination");
    rejects(d.issue(1, 1, i32::MAX), "destination missing");
    d.worlds.get_mut(&-3).unwrap().max_players = 1;
    let mut occupant = ch.clone();
    occupant.account = 2;
    occupant.id = 2;
    let other = d
        .worlds
        .get_mut(&-3)
        .unwrap()
        .enter(occupant, None)
        .unwrap();
    rejects(d.issue(1, 1, -3), "destination full");
    d.worlds.get_mut(&-3).unwrap().admission_open = false;
    rejects(d.destination(id, Some(exit)), "has closed. Use Nexus");
    rejects(d.issue(1, 1, -3), "Realm is closed");
    assert_eq!(d.destination(id, None).unwrap(), (1, 1, -2));
    let source = &d.worlds[&dungeon].players[&id].character;
    assert_eq!(source, &before);
    assert!(d.grants.is_empty());
    assert!(d.worlds[&-3].admissions.is_empty());
    d.worlds.get_mut(&-3).unwrap().admission_open = true;
    d.worlds
        .get_mut(&-3)
        .unwrap()
        .players
        .remove(&other)
        .unwrap();
    let (time, key, _) = d.issue(1, 1, -3).unwrap();
    for (account, destination, stamp) in [(2, -3, time), (1, -2, time), (1, -3, time + 1)] {
        rejects(
            d.consume(account, destination, stamp, key.clone()),
            "binding mismatch",
        );
    }
    rejects(d.consume(1, -3, time, key.clone()), "origin still owns");
    d.tick += 2400;
    rejects(d.consume(1, -3, time, key.clone()), "grant expired");
    d.retire_transfer_grants();
    assert!(d.grants.is_empty());
    assert!(d.worlds[&-3].admissions.is_empty());
    let (time, key, _) = d.issue(1, 1, -3).unwrap();
    d.worlds
        .get_mut(&dungeon)
        .unwrap()
        .players
        .remove(&id)
        .unwrap();
    let (character, arrival) = d.consume(1, -3, time, key.clone()).unwrap();
    assert_eq!(character, ch.id);
    d.admit_transfer(arrival, 1, 1, -3).unwrap();
    rejects(d.consume(1, -3, time, key), "already used");
}
