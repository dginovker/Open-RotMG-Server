use super::*;

fn directory() -> Directory {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    Directory::test_with_seed(std::path::Path::new("data"), content, 20, 423).unwrap()
}

#[test]
fn map_info_selects_stars_only_for_sprite_world() {
    let mut d = directory();
    for (template, owner, background) in [
        ("sprite-world", None, 1),
        ("snake-pit", None, 0),
        ("vault", Some(1), 2),
        ("nexus", None, 2),
    ] {
        let world = d.make(template, template, false, owner).unwrap();
        let bytes = world.map_info();
        let mut packet = wire::Reader::new(&bytes);
        packet.i32().unwrap();
        packet.i32().unwrap();
        packet.utf().unwrap();
        packet.i32().unwrap();
        assert_eq!(packet.i32().unwrap(), background, "{template}");
    }
}

async fn character(store: &crate::store::Store, d: &Directory, name: &str) -> Character {
    let account = store.list(name.into()).await.unwrap();
    store
        .create(account.id, d.content.get(crate::content::WIZARD).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn dungeon_portals_expire_at_their_full_published_lifetime() {
    for (kind, key, lifetime) in [
        (0x717, None, 1200),
        (0x70c, None, 300),
        (0x717, Some(0x705), 1200),
    ] {
        let mut d = directory();
        let store =
            crate::store::Store::open(std::path::Path::new(":memory:"), &d.content).unwrap();
        let mut ch = character(&store, &d, "portal-timer").await;
        if let Some(key) = key {
            ch.items[4] = key;
        }
        let player = d.worlds.get_mut(&-3).unwrap().enter(ch, None).unwrap();
        let pos = d.worlds[&-3].players[&player].pos;
        d.tick = 1234;
        let portal = if let Some(key) = key {
            let w = d.worlds.get_mut(&-3).unwrap();
            let plan = w
                .use_item(
                    player,
                    Slot {
                        object: player,
                        slot: 4,
                        kind: key,
                    },
                    pos,
                )
                .unwrap()
                .unwrap();
            let portal = plan.unlock.unwrap().opened;
            let plan = d.stage_unlock(plan).unwrap();
            assert!(!d.worlds[&-3].entities[&portal].published);
            d.tick += 100;
            let revision = plan.revision + 1;
            d.finish_inventory(plan, Ok(revision)).unwrap();
            assert_eq!(d.worlds[&-3].players[&player].character.items[4], -1);
            portal
        } else {
            d.drop_dungeon_portal(-3, kind, pos).unwrap()
        };
        let born = d.tick;
        let destination = d.portals[&portal].destination.unwrap();
        assert_eq!(d.portals[&portal].expires, born + lifetime);
        d.worlds
            .get_mut(&-3)
            .unwrap()
            .players
            .get_mut(&player)
            .unwrap()
            .pos = d.worlds[&-3].entities[&portal].pos;
        for age in [lifetime / 2, lifetime - 1] {
            d.tick = born + age;
            d.coordinate(&[]);
            assert!(d.worlds[&-3].entities[&portal].published);
            assert_eq!(d.destination(player, Some(portal)).unwrap().2, destination);
        }
        d.tick = born + lifetime;
        assert!(d.destination(player, Some(portal)).is_err());
        d.coordinate(&[]);
        assert!(!d.portals.contains_key(&portal));
        assert!(!d.worlds[&-3].entities.contains_key(&portal));
        assert!(d.destination(player, Some(portal)).is_err());
        assert!(d.worlds.contains_key(&destination));
    }
}

#[tokio::test]
async fn god_defeats_publish_dungeons_past_twelve_worlds_and_preserve_vaults() {
    let mut d = directory();
    let store = crate::store::Store::open(std::path::Path::new(":memory:"), &d.content).unwrap();
    let vault_portal = d.worlds[&-2]
        .entities
        .values()
        .find(|e| e.kind == 0x720)
        .unwrap()
        .clone();
    let mut residents = Vec::new();
    for i in 0..7 {
        let ch = character(&store, &d, &format!("vault-{i}")).await;
        let player = d.worlds.get_mut(&-2).unwrap().enter(ch, None).unwrap();
        d.worlds
            .get_mut(&-2)
            .unwrap()
            .players
            .get_mut(&player)
            .unwrap()
            .pos = vault_portal.pos;
        let (account, character, vault) = d.destination(player, Some(vault_portal.id)).unwrap();
        let (time, key, _) = d.issue(account, character, vault).unwrap();
        let mut reservation = d.reserve_transfer(key.clone()).unwrap();
        let snapshot = d
            .begin_source_departure(player, Some(&key))
            .unwrap()
            .unwrap();
        d.accept_source_write(player, &key).unwrap();
        d.finish_source_departure(player, snapshot.clone(), Some(key.clone()))
            .unwrap();
        reservation.disarm();
        let (_, arrival) = d.consume(account, vault, time, key.clone()).unwrap();
        d.admit_transfer(arrival, account, character, vault)
            .unwrap();
        let player = d
            .worlds
            .get_mut(&vault)
            .unwrap()
            .enter_admitted(
                snapshot,
                None,
                None,
                AccountPresentation {
                    guild: None,
                    credits: 0,
                    fame: 0,
                    best_fame: BTreeMap::new(),
                },
                Some(key),
            )
            .unwrap();
        assert_eq!(d.instance("vault", Some(account), None).unwrap(), vault);
        assert!(
            d.issue(account + 1000, character, vault)
                .unwrap_err()
                .to_string()
                .contains("another account")
        );
        residents.push((
            vault,
            player,
            serde_json::to_value(&d.worlds[&vault].players[&player].character).unwrap(),
        ));
    }
    assert_eq!(d.worlds.len(), 12);
    let ch = character(&store, &d, "god-hunter").await;
    let player = d.worlds.get_mut(&-3).unwrap().enter(ch, None).unwrap();
    let pos = d.worlds[&-3].players[&player].pos;
    let mut portals = Vec::new();
    for (enemy, name, kind, chance, dungeon) in [
        (0x654, "Medusa", 0x71b, 0.1, "Abyss of Demons"),
        (0x659, "Ghost God", 0x71a, 0.17, "Undead Lair"),
        (0x657, "Flying Brain", 0x890, 0.17, "Mad Lab"),
        (0x653, "Sprite Child", 0x70c, 0.25, "Sprite World"),
    ] {
        assert_eq!(d.content.objects[&enemy].name, name);
        assert_eq!(crate::dungeon::portal_drop(enemy), Some((kind, chance)));
        for succeeds in [false, true, true] {
            let seed = (0..64)
                .find(|seed| rand::rngs::StdRng::seed_from_u64(*seed).gen_bool(chance) == succeeds)
                .unwrap();
            let before: BTreeSet<_> = d.portals.keys().copied().collect();
            let w = d.worlds.get_mut(&-3).unwrap();
            let target = w.spawn_entity(enemy, pos, vec![]).unwrap();
            w.damage_enemy(player, target, 1, true, &[], None).unwrap();
            assert!(w.portal_requests.is_empty());
            let hp = w.entities[&target].hp;
            w.rng = rand::rngs::StdRng::seed_from_u64(seed);
            w.damage_enemy(player, target, hp, true, &[], None).unwrap();
            assert!(!w.entities.contains_key(&target));
            assert_eq!(w.portal_requests.len(), usize::from(succeeds));
            w.damage_enemy(player, target, hp, true, &[], None).unwrap();
            assert_eq!(w.portal_requests.len(), usize::from(succeeds));
            // A successful death roll must create a portal, not just queue one: https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/logic/behaviors/DropPortalOnDeath.cs#L42-L44
            d.coordinate(&[-3]);
            let created: Vec<_> = d
                .portals
                .keys()
                .filter(|id| !before.contains(id))
                .copied()
                .collect();
            println!(
                "{name}: roll={succeeds}, published={}, loaded_worlds={}",
                created.len(),
                d.worlds.len()
            );
            assert_eq!(
                created.len(),
                usize::from(succeeds),
                "{name}: successful death roll lost during portal publication"
            );
            assert!(d.worlds[&-3].portal_requests.is_empty());
            for portal in created {
                let destination = d.portals[&portal].destination.unwrap();
                assert_eq!(d.worlds[&-3].entities[&portal].kind, kind);
                assert!(d.worlds[&-3].entities[&portal].published);
                assert_eq!(d.worlds[&destination].name, dungeon);
                assert!(matches!(
                    d.worlds[&destination].return_route,
                    Some(DungeonReturn::Realm(-3))
                ));
                let actors = d.worlds[&destination].actor_ids.clone();
                portals.push((portal, destination, actors));
            }
        }
    }
    assert_eq!(d.worlds.len(), 20);
    for (vault, player, before) in residents {
        assert_eq!(
            serde_json::to_value(&d.worlds[&vault].players[&player].character).unwrap(),
            before
        );
        assert!(!d.reclaimable_world(vault));
    }
    for (portal, destination, actors) in portals {
        assert_eq!(d.portals[&portal].destination, Some(destination));
        assert_eq!(d.worlds[&destination].actor_ids, actors);
        assert!(!d.reclaimable_world(destination));
    }
}

#[tokio::test]
async fn idle_reclamation_preserves_players_loot_portals_and_inflight_admissions() {
    let mut d = directory();
    let store = crate::store::Store::open(std::path::Path::new(":memory:"), &d.content).unwrap();
    let empty = d.instance("kitchen", None, None).unwrap();
    let occupied = d.instance("kitchen", None, None).unwrap();
    let loot = d.instance("kitchen", None, None).unwrap();
    let arrival_character = character(&store, &d, "inflight").await;
    let account = arrival_character.account;
    let reserved = d.instance("vault", Some(account), None).unwrap();
    let ch = character(&store, &d, "idle-resident").await;
    let character_id = ch.id;
    let player = d
        .worlds
        .get_mut(&occupied)
        .unwrap()
        .enter(ch, None)
        .unwrap();
    let (x, y) = d.worlds[&-3].map.spawns[0];
    let portal = d.drop_dungeon_portal(-3, 0x70c, Pos { x, y }).unwrap();
    let dungeon = d.portals[&portal].destination.unwrap();
    d.step_using(|_, _| {});
    d.tick = 6000;
    d.portals.get_mut(&portal).unwrap().expires = d.tick + 300;
    let (x, y) = d.worlds[&loot].map.spawns[0];
    let pos = Pos { x, y };
    let bag = d
        .worlds
        .get_mut(&loot)
        .unwrap()
        .loot_bag(pos, Some(account), vec![0xa22])
        .unwrap();
    let (time, key, _) = d.issue(account, arrival_character.id, reserved).unwrap();
    d.coordinate(&[]);
    assert!(
        d.worlds.contains_key(&empty),
        "idle lifetime is strictly more than 6000 ticks"
    );
    d.tick += 1;
    d.coordinate(&[]);
    assert!(!d.worlds.contains_key(&empty));
    for id in [occupied, loot, reserved, dungeon, -1, -2, -3, -4, 1] {
        assert!(d.worlds.contains_key(&id), "live world {id} was reclaimed");
    }
    assert_eq!(
        d.worlds[&occupied].players[&player].character.id,
        character_id
    );
    assert_eq!(d.worlds[&loot].entities[&bag].items[0], 0xa22);
    let (_, arrival) = d.consume(account, reserved, time, key.clone()).unwrap();
    d.admit_transfer(arrival, account, arrival_character.id, reserved)
        .unwrap();
    assert!(d.worlds[&reserved].admissions[&key].consumed);
    assert_eq!(d.idle[&reserved], d.tick);
    d.coordinate(&[]);
    assert!(
        d.worlds.contains_key(&reserved),
        "HELLO-to-LOAD admission was reclaimed"
    );
    d.tick = 6300;
    d.coordinate(&[]);
    assert!(!d.portals.contains_key(&portal));
    assert!(!d.worlds[&-3].entities.contains_key(&portal));
    assert!(!d.worlds.contains_key(&dungeon));
    d.cancel_admission(account, key).unwrap();
    d.worlds.get_mut(&loot).unwrap().now = 60_000;
    d.worlds.get_mut(&loot).unwrap().step();
    assert!(!d.worlds[&loot].entities.contains_key(&bag));
    d.coordinate(&[]);
    assert!(
        d.worlds.contains_key(&reserved),
        "arrival refreshed the idle lifetime"
    );
    assert!(!d.worlds.contains_key(&loot));
    d.tick = 12002;
    d.coordinate(&[]);
    assert!(!d.worlds.contains_key(&reserved));
    d.worlds
        .get_mut(&occupied)
        .unwrap()
        .begin_departure(player)
        .unwrap()
        .unwrap();
    d.worlds
        .get_mut(&occupied)
        .unwrap()
        .finish_departure(player)
        .unwrap();
    d.step_using(|_, _| {});
    let since = d.idle[&occupied];
    d.tick = since + 6000;
    d.coordinate(&[]);
    assert!(d.worlds.contains_key(&occupied));
    d.tick += 1;
    d.coordinate(&[]);
    assert!(!d.worlds.contains_key(&occupied));
}

#[test]
fn endgame_and_private_instance_creation_preserve_live_worlds_above_twelve() {
    let mut d = directory();
    let (x, y) = d.worlds[&-3].map.spawns[0];
    for _ in 0..7 {
        d.drop_dungeon_portal(-3, 0x70c, Pos { x, y }).unwrap();
    }
    assert_eq!(d.worlds.len(), 12);
    let bound: Vec<_> = d.portals.values().map(|p| p.destination.unwrap()).collect();
    d.worlds.get_mut(&-3).unwrap().admission_open = false;
    d.endgame.realms.get_mut(&-3).unwrap().cycle.stage = crate::endgame::RealmStage::Transferring;
    d.endgame_step(&[]);
    let castle = d.endgame.realms[&-3]
        .castle
        .expect("Castle and replacement Realm must be installed together above twelve live worlds");
    assert_eq!(d.worlds[&castle].name, "Oryx's Castle");
    assert_eq!(d.worlds.len(), 14);
    let replacement = *d
        .endgame
        .realms
        .keys()
        .find(|id| **id != -3 && **id != 1)
        .unwrap();
    assert!(!d.endgame.realms[&replacement].enabled);
    assert!(!d.worlds[&replacement].admission_open);
    let guild = crate::store::Guild {
        id: 7,
        name: "Lifetime".into(),
        level: 1,
        fame: 0,
        earned: 0,
        board: String::new(),
    };
    let hall = d.guild_instance(guild.clone()).unwrap();
    assert_eq!(d.guild_instance(guild.clone()).unwrap(), hall);
    assert_eq!(d.worlds[&hall].guild, Some((7, 1)));
    assert!(
        d.guild_instance(crate::store::Guild { level: 5, ..guild })
            .is_err()
    );
    assert!(d.prepared_instance(crate::endgame::CASTLE).is_err());
    d.prepared_seed = Some(423);
    let prepared = d.prepared_instance(crate::endgame::CASTLE).unwrap();
    assert_ne!(prepared, castle);
    assert_eq!(d.worlds.len(), 16);
    d.worlds.get_mut(&prepared).unwrap().cleared = true;
    d.endgame_step(&[prepared]);
    let chamber_portal = d
        .portals
        .iter()
        .find(|(_, p)| p.origin == prepared)
        .unwrap()
        .0;
    let chamber = d.portals[chamber_portal].destination.unwrap();
    assert_eq!(d.worlds[&chamber].name, "Oryx's Chamber");
    assert!(d.worlds[&prepared].entities[chamber_portal].published);
    assert!(
        !d.worlds[&prepared]
            .entities
            .values()
            .any(|e| e.kind == 0xd7a)
    );
    assert_eq!(d.worlds.len(), 17);
    for destination in bound {
        assert!(d.worlds.contains_key(&destination));
        assert!(!d.reclaimable_world(destination));
    }
    d.tick = crate::endgame::REPLACEMENT_DELAY_MS / 50;
    d.endgame_step(&[]);
    assert!(d.endgame.realms[&replacement].enabled);
    assert!(d.worlds[&replacement].admission_open);
    assert!(!d.worlds.contains_key(&-3));
    let worlds: Vec<_> = d.worlds.keys().copied().collect();
    d.next_world = i32::MAX;
    d.endgame.realms.get_mut(&1).unwrap().cycle.stage = crate::endgame::RealmStage::Transferring;
    d.endgame_step(&[]);
    assert!(d.endgame.realms[&1].castle.is_none());
    assert_eq!(d.worlds.keys().copied().collect::<Vec<_>>(), worlds);
    assert_eq!(d.next_world, i32::MAX);
}
