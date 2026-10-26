use super::*;

async fn habitat(seed: u64) -> World {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let map = Map {
        width: 128,
        height: 128,
        cells: vec![
            crate::content::Cell {
                ground: 0x61,
                object: None,
                config: String::new(),
                terrain: 1,
                region: 0,
            };
            128 * 128
        ],
        spawns: vec![(1.5, 1.5)],
    };
    let store = crate::store::Store::open(std::path::Path::new(":memory:"), &content).unwrap();
    let account = store.list("god-reproduction".into()).await.unwrap();
    let character = store
        .create(account.id, content.get(crate::content::WIZARD).unwrap())
        .await
        .unwrap();
    let mut w = World::new(content, Arc::new(map)).unwrap();
    w.encounter = true;
    w.rng = rand::rngs::StdRng::seed_from_u64(seed);
    w.configure_population().unwrap();
    // Reproduction must work without the regional low-water replacement trigger.
    w.population_regions[1].target = 0;
    w.population_regions[1].limit = 64;
    let id = w.enter(character, None).unwrap();
    w.players.get_mut(&id).unwrap().pos = Pos { x: 84., y: 64.5 };
    w
}

fn parent(w: &mut World, kind: u16, pos: Pos) -> i32 {
    let entity = w.spawn_entity(kind, pos, vec![]).unwrap();
    let slot = w.population.len();
    w.population.push(Spawn {
        kind,
        pos,
        entity: Some(entity),
        camp: None,
    });
    w.mark_population(entity, slot).unwrap();
    w.entities.get_mut(&entity).unwrap().condition |=
        crate::condition::bit(crate::condition::PARALYZED)
            | crate::condition::bit(crate::condition::STUNNED);
    entity
}

#[tokio::test]
async fn ordinary_god_reproduction_uses_active_minute_not_regional_refill() {
    let mut w = habitat(12351).await;
    let center = Pos { x: 64.5, y: 64.5 };
    let id = parent(&mut w, 0x651, center);
    let player = *w.players.keys().next().unwrap();
    let character = w.players[&player].character.clone();
    w.now += 60000;
    w.npc_step();
    assert_eq!(w.population_count(1), 1);
    w.now += 50;
    w.npc_step();
    assert_eq!(w.population_count(1), 2);
    let child = w.actors().find(|e| e.id != id).unwrap();
    assert_eq!(child.kind, 0x651);
    assert!((child.pos.distance(center) - 5.).abs() < 0.0001);
    assert!(child.parent.is_none());
    assert!(child.population_slot.is_some());
    assert!(child.damage.is_empty() && child.quest.is_none());
    assert_eq!(w.population_regions[1].target, 0);
    assert_eq!(w.players[&player].character, character);
}

fn attempt(w: &mut World, id: i32) -> population::Reproduction {
    w.god_reproduction.insert(id, 0);
    w.reproduce_god(id, 50).unwrap().unwrap()
}

#[tokio::test]
async fn reproduction_retains_initial_player_clearance_while_parent_is_active() {
    let mut w = habitat(41).await;
    let center = Pos { x: 64.5, y: 64.5 };
    let id = parent(&mut w, 0x651, center);
    let player = *w.players.keys().next().unwrap();
    w.players.get_mut(&player).unwrap().pos = center;
    w.now += 50;
    w.npc_step();
    assert!(w.entities[&id].activated.is_some());
    let actors = w.actor_ids.clone();
    assert_eq!(attempt(&mut w, id), population::Reproduction::NoSite);
    assert_eq!(w.actor_ids, actors);
    assert_eq!(w.population_count(1), 1);
    assert_eq!(w.population.len(), 1);
    assert_eq!(w.god_reproduction[&id], 60000);

    let clear_player = Pos { x: 84., y: 64.5 };
    assert!(center.distance(clear_player) < 20.);
    w.players.get_mut(&player).unwrap().pos = clear_player;
    w.god_reproduction.insert(id, 0);
    w.now += 50;
    w.npc_step();
    assert_eq!(w.actor_ids.len(), actors.len() + 1);
    assert_eq!(w.population_count(1), 2);
    assert_eq!(w.god_reproduction[&id], 60000);
    let child = w.actors().find(|e| e.id != id).unwrap();
    assert_eq!(child.kind, 0x651);
    assert!(child.pos.distance(clear_player) >= 12.);
    assert!((child.pos.distance(center) - 5.).abs() < 0.0001);
}

#[tokio::test]
async fn reproduction_density_includes_parent_and_other_camps_but_excludes_exact_radius() {
    use population::Reproduction;
    let mut w = habitat(7).await;
    let center = Pos { x: 64.5, y: 64.5 };
    let id = parent(&mut w, 0x651, center);
    parent(&mut w, 0x651, Pos { x: 66.5, y: 64.5 });
    let edge = parent(&mut w, 0x651, Pos { x: 74.499, y: 64.5 });
    assert_eq!(attempt(&mut w, id), Reproduction::Density);
    assert_eq!(w.god_reproduction[&id], 60000);
    w.position_entity(edge, Pos { x: 74.5, y: 64.5 }).unwrap();
    parent(&mut w, 0x655, Pos { x: 63.5, y: 64.5 });
    assert!(matches!(attempt(&mut w, id), Reproduction::Born(_)));
}

#[tokio::test]
async fn reproduction_cluster_and_supported_species_respect_local_maxima() {
    use population::Reproduction;
    let mut w = habitat(11).await;
    for (kind, maximum) in [
        (0x651, 3),
        (0x652, 2),
        (0x654, 3),
        (0x655, 3),
        (0x656, 3),
        (0x657, 3),
        (0x658, 2),
        (0x659, 3),
    ] {
        let ids: Vec<_> = (0..maximum)
            .map(|i| {
                parent(
                    &mut w,
                    kind,
                    Pos {
                        x: 50.5 + i as f32,
                        y: 50.5,
                    },
                )
            })
            .collect();
        for id in &ids {
            assert_eq!(attempt(&mut w, *id), Reproduction::Density);
        }
        for id in ids {
            w.remove_entity(id);
        }
    }
    for kind in [0x653, 0x91a, 0x6d8, 0x905] {
        let id = parent(&mut w, kind, Pos { x: 50.5, y: 50.5 });
        assert_eq!(w.reproduce_god(id, 60001).unwrap(), None);
        assert!(!w.god_reproduction.contains_key(&id));
        w.remove_entity(id);
    }
    let scripted = w
        .spawn_entity(0x651, Pos { x: 50.5, y: 50.5 }, vec![])
        .unwrap();
    assert_eq!(w.reproduce_god(scripted, 60001).unwrap(), None);
}

#[tokio::test]
async fn reproduction_clock_freezes_outside_simulation_and_in_stasis() {
    let mut w = habitat(17).await;
    let id = parent(&mut w, 0x651, Pos { x: 64.5, y: 64.5 });
    let player = *w.players.keys().next().unwrap();
    w.now += 50;
    w.npc_step();
    assert_eq!(w.god_reproduction[&id], 59950);
    w.players.get_mut(&player).unwrap().pos = Pos { x: 100.5, y: 100.5 };
    w.now += 120000;
    w.npc_step();
    assert_eq!(w.god_reproduction[&id], 59950);
    w.players.get_mut(&player).unwrap().pos = Pos { x: 84., y: 64.5 };
    w.entities.get_mut(&id).unwrap().condition |= crate::condition::bit(crate::condition::STASIS);
    w.now += 120000;
    w.npc_step();
    assert_eq!(w.god_reproduction[&id], 59950);
    w.entities.get_mut(&id).unwrap().condition &= !crate::condition::bit(crate::condition::STASIS);
    w.now += 59950;
    w.npc_step();
    assert_eq!(w.population_count(1), 1);
    w.now += 50;
    w.npc_step();
    assert_eq!(w.population_count(1), 2);
    assert_eq!(w.god_reproduction[&id], 60000);
}

#[tokio::test]
async fn reproduction_refuses_capacity_unsafe_habitat_and_arrivals_without_retry_bursts() {
    use population::Reproduction;
    let mut w = habitat(23).await;
    let id = parent(&mut w, 0x651, Pos { x: 64.5, y: 64.5 });
    w.population_regions[1].limit = 1;
    assert_eq!(attempt(&mut w, id), Reproduction::Capacity);
    w.population_regions[1].limit = 64;
    for cell in &mut Arc::make_mut(&mut w.map).cells {
        cell.terrain = 2;
    }
    assert_eq!(attempt(&mut w, id), Reproduction::NoSite);
    for cell in &mut Arc::make_mut(&mut w.map).cells {
        cell.terrain = 1;
        cell.object = Some(0x150);
    }
    assert_eq!(attempt(&mut w, id), Reproduction::NoSite);
    for cell in &mut Arc::make_mut(&mut w.map).cells {
        cell.object = None;
    }
    w.setpieces.push(realm_setpieces::Area {
        x: 55,
        y: 55,
        width: 20,
        height: 20,
    });
    assert_eq!(attempt(&mut w, id), Reproduction::NoSite);
    w.setpieces.clear();
    Arc::make_mut(&mut w.map).spawns = vec![(64.5, 64.5)];
    assert_eq!(attempt(&mut w, id), Reproduction::NoSite);
    assert_eq!(w.population_count(1), 1);
    assert_eq!(w.population.len(), 1);
    Arc::make_mut(&mut w.map).spawns = vec![(1.5, 1.5)];
    assert_eq!(w.reproduce_god(id, 60000).unwrap(), None);
    assert!(matches!(
        w.reproduce_god(id, 50).unwrap(),
        Some(Reproduction::Born(_))
    ));
    w.position_entity(id, Pos { x: 0.5, y: 0.5 }).unwrap();
    assert_eq!(attempt(&mut w, id), Reproduction::NoSite);
}

#[tokio::test]
async fn reproduction_new_sprite_root_owns_its_family_and_reserves_full_cost() {
    use population::Reproduction;
    let mut w = habitat(29).await;
    let family = population::FAMILIES
        .iter()
        .find(|f| f.leader == 0x652)
        .unwrap();
    let center = Pos { x: 64.5, y: 64.5 };
    let positions = w.family_layout(center, family, &[]).unwrap();
    w.place_camp(family, positions).unwrap();
    let root = w.population[0].entity.unwrap();
    w.population_regions[1].limit = family.initial() * 2 - 1;
    assert_eq!(attempt(&mut w, root), Reproduction::Capacity);
    w.population_regions[1].limit += 1;
    let Reproduction::Born(child) = attempt(&mut w, root) else {
        panic!("Sprite root not born")
    };
    assert_eq!(w.population_count(1), family.initial() * 2);
    let camp = w.entities[&child].camp.unwrap();
    assert_ne!(w.entities[&root].camp, Some(camp));
    let members: Vec<_> = w.camp_members(camp).map(|e| e.id).collect();
    assert_eq!(members.len(), family.initial());
    assert!(members.iter().all(|id| w.entities[id].parent.is_none()));
    assert!(members.iter().all(|id| {
        w.players
            .values()
            .all(|p| w.entities[id].pos.distance(p.pos) >= 12.)
    }));
    w.remove_entity(root);
    assert!(members.iter().all(|id| w.entities.contains_key(id)));
}

#[tokio::test]
async fn reproduction_parent_death_cancels_clock_and_refill_counts_births_once() {
    let mut w = habitat(31).await;
    let id = parent(&mut w, 0x651, Pos { x: 64.5, y: 64.5 });
    assert!(matches!(
        attempt(&mut w, id),
        population::Reproduction::Born(_)
    ));
    let player = *w.players.keys().next().unwrap();
    let before = w.players[&player].character.kills;
    w.damage_enemy(player, id, 1000, true, &[], None).unwrap();
    assert!(!w.entities.contains_key(&id));
    assert!(!w.god_reproduction.contains_key(&id));
    assert_eq!(w.population_count(1), 1);
    assert_eq!(w.players[&player].character.kills, before + 1);
    assert_eq!(
        w.damage_enemy(player, id, 1000, true, &[], None).unwrap(),
        0
    );
    assert_eq!(w.players[&player].character.kills, before + 1);
    // One surviving birth is above the refill low-water mark; empty parent slots are compacted, not revived.
    w.population_regions[1].target = 1;
    w.now = w.population_next;
    w.repopulate().unwrap();
    assert_eq!(w.population_count(1), 1);
    assert_eq!(w.population.len(), 1);
    assert!(
        w.population
            .iter()
            .all(|s| s.entity.is_some_and(|live| live != id))
    );
    let live = w.population[0].entity.unwrap();
    assert_eq!(w.entities[&live].population_slot, Some(0));
    w.population_regions[1].target = 4;
    w.now = w.population_next;
    w.repopulate().unwrap();
    assert!(w.population_count(1) >= 4);
    assert!(w.population_count(1) <= w.population_regions[1].limit);
    assert_eq!(
        w.population_count(1),
        w.population.iter().filter(|s| s.entity.is_some()).count()
    );
    assert_eq!(w.players[&player].character.kills, before + 1);
}

#[tokio::test]
async fn reproduction_seeded_isolated_parents_replay_without_exceeding_shared_budget() {
    async fn replay(seed: u64) -> Vec<(u16, f32, f32)> {
        let mut w = habitat(seed).await;
        let a = parent(&mut w, 0x651, Pos { x: 45.5, y: 45.5 });
        let b = parent(&mut w, 0x651, Pos { x: 85.5, y: 85.5 });
        w.population_regions[1].limit = 4;
        assert!(matches!(
            attempt(&mut w, a),
            population::Reproduction::Born(_)
        ));
        assert!(matches!(
            attempt(&mut w, b),
            population::Reproduction::Born(_)
        ));
        assert_eq!(attempt(&mut w, a), population::Reproduction::Capacity);
        assert_eq!(w.population_count(1), 4);
        w.actors().map(|e| (e.kind, e.pos.x, e.pos.y)).collect()
    }
    for seed in 0..8 {
        assert_eq!(replay(seed).await, replay(seed).await, "seed {seed}");
    }
}
