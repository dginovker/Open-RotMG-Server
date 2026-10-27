use super::*;

async fn reward_world() -> (World, crate::store::Store, Vec<(i32, i32)>) {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let map = Arc::new(Map::load(std::path::Path::new("data/tutorial.wmap"), &content).unwrap());
    let store = crate::store::Store::open(std::path::Path::new(":memory:"), &content).unwrap();
    let mut w = World::new(content, map).unwrap();
    for id in w.entities.keys().copied().collect::<Vec<_>>() {
        w.remove_entity(id);
    }
    w.encounter = true;
    let map = Arc::make_mut(&mut w.map);
    map.spawns = vec![(1.5, 1.5)];
    for cell in &mut map.cells {
        cell.ground = 0x61;
        cell.object = None;
    }
    for i in 0..6 {
        let account = store.list(format!("reward-accuracy-{i}")).await.unwrap();
        let character = store
            .create(account.id, w.content.get(crate::content::WIZARD).unwrap())
            .await
            .unwrap();
        w.enter(character, None).unwrap();
    }
    let owners = w
        .players
        .values()
        .map(|p| (p.character.account, p.character.id))
        .collect();
    (w, store, owners)
}

fn clear_bags(w: &mut World) {
    for id in w.bag_expiry.keys().copied().collect::<Vec<_>>() {
        w.remove_entity(id);
    }
}

fn white_demon_gear(content: &Content, seed: u64) -> Vec<i32> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let mut items = Vec::new();
    for (chance, tier, slots) in [
        (0.04, 6, crate::loot::WEAPONS),
        (0.02, 7, crate::loot::WEAPONS),
        (0.01, 8, crate::loot::WEAPONS),
        (0.08, 6, crate::loot::ARMOR),
        (0.04, 7, crate::loot::ARMOR),
        (0.02, 8, crate::loot::ARMOR),
        (0.01, 9, crate::loot::ARMOR),
        (0.015, 3, crate::loot::RINGS),
        (0.005, 4, crate::loot::RINGS),
        (0.02, 4, crate::loot::ABILITIES),
    ] {
        let candidates: Vec<_> = content
            .objects
            .values()
            .filter(|item| item.tier == Some(tier) && slots.contains(&item.slot_type))
            .collect();
        for item in &candidates {
            if rng.gen_bool(chance / candidates.len() as f64) {
                items.push(i32::from(item.kind));
            }
        }
    }
    items.sort_unstable();
    items
}

#[tokio::test]
async fn god_gear_death_roll_is_once_per_candidate_with_private_colored_bags() {
    let (mut w, _store, owners) = reward_world().await;
    let mut seeds = vec![0, 1, 2, 3];
    let multiple = (0..32768)
        .find(|seed| {
            white_demon_gear(&w.content, *seed)
                .iter()
                .filter(|kind| {
                    let item = &w.content.objects[&(**kind as u16)];
                    item.tier == Some(6) && crate::loot::WEAPONS.contains(&item.slot_type)
                })
                .count()
                > 1
        })
        .expect("terminating seeded search must exercise multiple successes in one tier");
    seeds.push(multiple);
    for contributors in [1, 2, 5] {
        for seed in &seeds {
            clear_bags(&mut w);
            let root = w
                .spawn_entity(0x651, Pos { x: 3.5, y: 3.5 }, vec![])
                .unwrap();
            w.entities.get_mut(&root).unwrap().damage = owners[..contributors]
                .iter()
                .map(|(_, character)| (*character, 1))
                .collect();
            w.rng = rand::rngs::StdRng::seed_from_u64(*seed);
            w.defeat_entity(root).unwrap();
            assert!(!w.entities.contains_key(&root));
            let mut actual = Vec::new();
            for bag in w.bag_expiry.keys().map(|id| &w.entities[id]) {
                assert_eq!(bag.items.len(), 8);
                assert_eq!(
                    owners[..contributors]
                        .iter()
                        .filter(|(account, _)| bag.accessible(*account))
                        .count(),
                    if bag.kind == 0x500 { contributors } else { 1 }
                );
                assert!(!bag.accessible(owners[5].0));
                actual.extend(bag.items.iter().copied().filter(|item| *item >= 0));
            }
            actual.sort_unstable();
            assert_eq!(
                actual,
                white_demon_gear(&w.content, *seed),
                "contributors={contributors}, seed={seed}"
            );
            assert_eq!(actual.iter().collect::<BTreeSet<_>>().len(), actual.len());
        }
    }
}

#[tokio::test]
async fn septavius_doom_fraction_qualifies_at_fifteen_percent_without_raising_other_gates() {
    let (mut w, _store, owners) = reward_world().await;
    assert_eq!(w.content.objects[&0xd90].hp, 8000);
    let seed = (0..32768)
        .find(|seed| {
            crate::loot::rewards(
                &w.content,
                0xd90,
                &mut rand::rngs::StdRng::seed_from_u64(*seed),
            )
            .contains(&0xc02)
        })
        .expect("bounded seeded search for the unchanged .001 Doom roll");
    for damage in [800, 801, 1199, 1200, 1201] {
        clear_bags(&mut w);
        let root = w
            .spawn_entity(0xd90, Pos { x: 3.5, y: 3.5 }, vec![])
            .unwrap();
        w.entities
            .get_mut(&root)
            .unwrap()
            .damage
            .insert(owners[0].1, damage);
        w.rng = rand::rngs::StdRng::seed_from_u64(seed);
        w.defeat_entity(root).unwrap();
        let mut items = Vec::new();
        for bag in w.bag_expiry.keys().map(|id| &w.entities[id]) {
            assert!(bag.accessible(owners[0].0));
            assert!(!bag.accessible(owners[1].0));
            items.extend(bag.items.iter().copied().filter(|item| *item >= 0));
        }
        assert_eq!(items.contains(&0xc02), damage >= 1200, "Doom at {damage}");
        assert_eq!(items.contains(&0xa35), damage > 800, "Wisdom at {damage}");
        let expected: Vec<_> = if damage > 800 {
            crate::loot::rewards(
                &w.content,
                0xd90,
                &mut rand::rngs::StdRng::seed_from_u64(seed),
            )
            .into_iter()
            .filter(|item| *item != 0xc02)
            .map(i32::from)
            .collect()
        } else {
            Vec::new()
        };
        assert_eq!(
            items
                .into_iter()
                .filter(|item| *item != 0xc02)
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[tokio::test]
async fn god_potions_do_not_erase_successful_equipment_drops() {
    let (mut w, _store, owners) = reward_world().await;
    for kind in [
        0x651, 0x652, 0x654, 0x655, 0x656, 0x657, 0x658, 0x659, 0x91a, 0x6d8, 0xd05,
    ] {
        for count in [1, 2] {
            let contributors: Vec<_> = owners[..count]
                .iter()
                .map(|(account, _)| (*account, crate::loot::potion_damage(kind).unwrap()))
                .collect();
            let (seed, loot) = (0..16384)
                .find_map(|seed| {
                    let loot = crate::loot::god_loot(
                        &w.content,
                        kind,
                        &contributors,
                        &mut rand::rngs::StdRng::seed_from_u64(seed),
                    );
                    (!loot.shared.is_empty() && loot.private.contains_key(&owners[0].0))
                        .then_some((seed, loot))
                })
                .expect("seeded death must exercise simultaneous equipment and potion rolls");
            let mut expected: Vec<_> = loot
                .shared
                .into_iter()
                .chain(loot.private.into_values().flatten())
                .map(i32::from)
                .collect();
            expected.sort_unstable();
            clear_bags(&mut w);
            let root = w
                .spawn_entity(kind, Pos { x: 3.5, y: 3.5 }, vec![])
                .unwrap();
            w.entities.get_mut(&root).unwrap().damage = owners[..count]
                .iter()
                .map(|(_, character)| (*character, crate::loot::potion_damage(kind).unwrap()))
                .collect();
            w.rng = rand::rngs::StdRng::seed_from_u64(seed);
            w.defeat_entity(root).unwrap();
            let mut actual: Vec<_> = w
                .bag_expiry
                .keys()
                .flat_map(|id| {
                    let bag = &w.entities[id];
                    assert!(
                        owners[..count]
                            .iter()
                            .any(|(account, _)| bag.accessible(*account))
                    );
                    assert!(!bag.accessible(owners[count].0));
                    bag.items.iter().copied().filter(|item| *item >= 0)
                })
                .collect();
            actual.sort_unstable();
            assert_eq!(
                actual, expected,
                "{kind:#x}, {count} contributors, seed {seed}: successful gear must survive a potion roll"
            );
        }
    }
}

#[tokio::test]
async fn god_bags_split_all_successes_reject_nonowners_and_commit_collection_once() {
    let (mut w, store, owners) = reward_world().await;
    let contributors: Vec<_> = owners[..5]
        .iter()
        .map(|(account, _)| (*account, 1))
        .collect();
    let loot = crate::loot::god_loot(
        &w.content,
        0x651,
        &contributors,
        &mut rand::rngs::mock::StepRng::new(0, 1),
    );
    let expected: BTreeSet<_> = loot.shared.iter().copied().map(i32::from).collect();
    assert!(expected.len() > 8);
    w.god_loot_bags(Pos { x: 1.5, y: 1.5 }, loot).unwrap();
    assert_eq!(w.bag_expiry.len(), expected.len().div_ceil(8));
    assert!(w.bag_expiry.len() <= 128);
    let actual: Vec<_> = w
        .bag_expiry
        .keys()
        .flat_map(|id| {
            let bag = &w.entities[id];
            assert_eq!(bag.items.len(), 8);
            assert!(bag.published);
            assert_eq!(w.bag_expiry[id], w.now + 60_000);
            assert_eq!(
                owners[..5]
                    .iter()
                    .filter(|(account, _)| bag.accessible(*account))
                    .count(),
                if bag.kind == 0x500 { 5 } else { 1 }
            );
            assert!(!bag.accessible(owners[5].0));
            bag.items.iter().copied().filter(|item| *item >= 0)
        })
        .collect();
    assert_eq!(actual.iter().copied().collect::<BTreeSet<_>>(), expected);
    assert_eq!(
        actual.len(),
        expected.len(),
        "no cap or duplication during packaging"
    );
    let bag = *w.bag_expiry.keys().next().unwrap();
    let item = w.entities[&bag].items[0];
    let player = |account| {
        w.players
            .iter()
            .find(|(_, p)| p.character.account == account)
            .map(|(id, _)| *id)
            .unwrap()
    };
    let (account, character) = *owners[..5]
        .iter()
        .find(|(account, _)| w.entities[&bag].accessible(*account))
        .unwrap();
    let winner = player(account);
    let peer = player(owners.iter().find(|(a, _)| *a != account).unwrap().0);
    let outsider = player(owners[5].0);
    w.players.get_mut(&winner).unwrap().controller = Controller::Human;
    let from = Slot {
        object: bag,
        slot: 0,
        kind: item,
    };
    let to = |id| Slot {
        object: id,
        slot: 5,
        kind: -1,
    };
    assert!(!w.visible(outsider).contains(&bag));
    assert!(w.visible(winner).contains(&bag));
    assert!(w.swap(outsider, from, to(outsider)).is_err());
    let plan = w.swap(winner, from, to(winner)).unwrap();
    assert_eq!(plan.collected, Some(item as u16));
    assert!(
        w.swap(peer, from, to(peer)).is_err(),
        "reserved shared slot cannot race"
    );
    let revision = store
        .exchange_collected(
            vec![crate::store::InventoryWrite {
                id: plan.character,
                account: plan.account,
                revision: plan.revision,
                items: plan.items.clone(),
            }],
            plan.vaults.clone(),
            plan.collected,
        )
        .await
        .unwrap()[0];
    w.finish_inventory(plan, Ok(revision)).unwrap();
    assert_eq!(w.entities[&bag].items[0], -1);
    assert!(!w.entities[&bag].drop_slots.contains(&0));
    assert!(
        w.swap(peer, from, to(peer)).is_err(),
        "committed item cannot be collected twice"
    );
    let saved = store.load(account, character).await.unwrap();
    assert_eq!(saved.items[5], item);
    assert_eq!(saved.items[4], 0xa22);
    assert_eq!(saved.revision, revision);
}

#[tokio::test]
async fn purple_god_bag_has_one_owner_even_when_a_bot_contributed() {
    let (mut w, _store, owners) = reward_world().await;
    w.rng = rand::rngs::StdRng::seed_from_u64(650);
    w.god_loot_bags(
        Pos { x: 1.5, y: 1.5 },
        crate::loot::GodLoot {
            shared: vec![0xa3c],
            shared_owners: owners[..2].iter().map(|(account, _)| *account).collect(),
            private: BTreeMap::new(),
        },
    )
    .unwrap();
    let bag = *w.bag_expiry.keys().next().unwrap();
    assert_eq!(w.entities[&bag].kind, 0x503);
    let eligible: Vec<_> = w.players.keys().copied().take(2).collect();
    let allowed: Vec<_> = eligible
        .iter()
        .copied()
        .filter(|id| w.entities[&bag].accessible(w.players[id].character.account))
        .collect();
    assert_eq!(
        allowed.len(),
        1,
        "purple loot must not be shared between contributors"
    );
    let owner = allowed[0];
    let peer = *eligible.iter().find(|id| **id != owner).unwrap();
    w.players.get_mut(&owner).unwrap().controller = Controller::Human;
    w.players.get_mut(&peer).unwrap().controller = Controller::Autonomous;
    let from = Slot {
        object: bag,
        slot: 0,
        kind: 0xa3c,
    };
    let to = |id| Slot {
        object: id,
        slot: 5,
        kind: -1,
    };
    assert!(w.visible(owner).contains(&bag));
    assert!(!w.visible(peer).contains(&bag));
    assert!(w.swap(peer, from, to(peer)).is_err());
    assert_eq!(w.entities[&bag].items[0], 0xa3c);
    assert!(!w.entities[&bag].reserved);
    let plan = w.swap(owner, from, to(owner)).unwrap();
    assert_eq!(plan.collected, Some(0xa3c));
}

#[tokio::test]
async fn god_shared_bags_reject_exhausted_capacity_without_an_extra_entity() {
    let (mut w, _store, owners) = reward_world().await;
    let pos = Pos { x: 1.5, y: 1.5 };
    for _ in 0..128 {
        w.loot_bag(pos, None, vec![0xa3c]).unwrap();
    }
    let entities = w.entities.len();
    let error = w
        .god_loot_bags(
            pos,
            crate::loot::GodLoot {
                shared: vec![0xa3c],
                shared_owners: vec![owners[0].0, owners[1].0],
                private: BTreeMap::new(),
            },
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("transient loot bag limit reached")
    );
    assert_eq!(w.bag_expiry.len(), 128);
    assert_eq!(w.entities.len(), entities);
}

#[tokio::test]
async fn god_brown_shared_collection_failed_receipt_retains_reservation_without_credit() {
    let (mut w, store, owners) = reward_world().await;
    w.god_loot_bags(
        Pos { x: 1.5, y: 1.5 },
        crate::loot::GodLoot {
            shared: vec![0xa22],
            shared_owners: vec![owners[0].0, owners[1].0],
            private: BTreeMap::new(),
        },
    )
    .unwrap();
    let bag = *w.bag_expiry.keys().next().unwrap();
    assert_eq!(w.entities[&bag].kind, 0x500);
    assert!(w.entities[&bag].accessible(owners[0].0));
    assert!(w.entities[&bag].accessible(owners[1].0));
    let player = |account| {
        w.players
            .iter()
            .find(|(_, p)| p.character.account == account)
            .map(|(id, _)| *id)
            .unwrap()
    };
    let winner = player(owners[0].0);
    let peer = player(owners[1].0);
    let outsider = player(owners[5].0);
    w.players.get_mut(&winner).unwrap().controller = Controller::Human;
    let from = Slot {
        object: bag,
        slot: 0,
        kind: 0xa22,
    };
    let to = |id| Slot {
        object: id,
        slot: 5,
        kind: -1,
    };
    let before = store.load(owners[0].0, owners[0].1).await.unwrap();
    let plan = w.swap(winner, from, to(winner)).unwrap();
    assert_eq!(plan.collected, Some(0xa22));
    let error = w
        .finish_inventory(
            plan,
            Err(anyhow::anyhow!("injected collection receipt failure")),
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected collection receipt failure")
    );
    assert!(w.players[&winner].persistence_failure.is_some());
    assert!(w.players[&winner].pending);
    assert!(w.entities[&bag].reserved);
    assert_eq!(w.entities[&bag].items[0], 0xa22);
    assert!(w.entities[&bag].drop_slots.contains(&0));
    assert_eq!(w.players[&winner].character.items, before.items);
    assert_eq!(w.players[&winner].character.revision, before.revision);
    assert!(w.swap(peer, from, to(peer)).is_err());
    assert!(w.swap(outsider, from, to(outsider)).is_err());
    let saved = store.load(owners[0].0, owners[0].1).await.unwrap();
    assert_eq!(saved.items, before.items);
    assert_eq!(saved.revision, before.revision);
    assert_eq!(saved.lifetime, before.lifetime);
}
