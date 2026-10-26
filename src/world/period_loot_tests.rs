use super::*;

#[tokio::test]
async fn period_loot_keeps_ownership_threshold_boundaries_and_repeated_rewards() {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let map = Arc::new(Map::load(std::path::Path::new("data/tutorial.wmap"), &content).unwrap());
    let store = crate::store::Store::open(std::path::Path::new(":memory:"), &content).unwrap();
    let mut w = World::new(content, map).unwrap();
    for id in w.entities.keys().copied().collect::<Vec<_>>() {
        w.remove_entity(id);
    }
    let map = Arc::make_mut(&mut w.map);
    map.spawns = vec![(1.5, 1.5)];
    for cell in &mut map.cells {
        cell.ground = 0x61;
        cell.object = None;
    }
    for i in 0..3 {
        let account = store.list(format!("period-loot-{i}")).await.unwrap();
        let ch = store
            .create(account.id, w.content.get(crate::content::WIZARD).unwrap())
            .await
            .unwrap();
        w.enter(ch, None).unwrap();
    }
    let owners: Vec<_> = w
        .players
        .values()
        .map(|p| (p.character.account, p.character.id))
        .collect();
    let mut boundary_potion = false;
    let mut low_damage_equipment = false;
    let mut duplicate_potion = false;
    let mut overflow_split = false;
    for (kind, damage) in [(0x655, [180, 179, 0]), (0xd59, [100_000, 100_000, 0])] {
        let root = w
            .spawn_entity(kind, Pos { x: 3.5, y: 3.5 }, vec![])
            .unwrap();
        let mut defeated = w.entities[&root].clone();
        defeated.damage = owners
            .iter()
            .zip(damage)
            .map(|((_, ch), n)| (*ch, n))
            .collect();
        for seed in 0..512 {
            for bag in w.bag_expiry.keys().copied().collect::<Vec<_>>() {
                w.remove_entity(bag);
            }
            w.rng = rand::rngs::StdRng::seed_from_u64(seed);
            w.drop_loot(&defeated).unwrap();
            let mut rewards = BTreeMap::<i32, Vec<i32>>::new();
            for bag in w.bag_expiry.keys().map(|id| &w.entities[id]) {
                let owner = bag
                    .extra_stats
                    .iter()
                    .find(|(stat, _)| *stat == 54)
                    .expect("private reward")
                    .1;
                assert!(owners[..2].iter().any(|(account, _)| *account == owner));
                assert_eq!(bag.items.len(), 8);
                rewards
                    .entry(owner)
                    .or_default()
                    .extend(bag.items.iter().copied().filter(|id| *id >= 0));
            }
            if kind == 0x655 {
                assert!(
                    !rewards
                        .get(&owners[1].0)
                        .is_some_and(|items| items.contains(&0xa20))
                );
                boundary_potion |= rewards
                    .get(&owners[0].0)
                    .is_some_and(|items| items.contains(&0xa20));
                low_damage_equipment |= rewards.contains_key(&owners[1].0);
            } else {
                for items in rewards.values() {
                    duplicate_potion |= items.iter().any(|item| {
                        w.content.objects[&(*item as u16)].slot_type == 10
                            && items.iter().filter(|other| *other == item).count() > 1
                    });
                    overflow_split |= items.len() > 8;
                }
            }
        }
        w.remove_entity(root);
    }
    let root = w
        .spawn_entity(0x600, Pos { x: 3.5, y: 3.5 }, vec![])
        .unwrap();
    let mut defeated = w.entities[&root].clone();
    defeated.damage = owners[..2].iter().map(|(_, ch)| (*ch, 1)).collect();
    for seed in 0..64 {
        for bag in w.bag_expiry.keys().copied().collect::<Vec<_>>() {
            w.remove_entity(bag);
        }
        let expected: Vec<_> = crate::loot::rewards(
            &w.content,
            0x600,
            &mut rand::rngs::StdRng::seed_from_u64(seed),
        )
        .into_iter()
        .map(i32::from)
        .collect();
        w.rng = rand::rngs::StdRng::seed_from_u64(seed);
        w.drop_loot(&defeated).unwrap();
        let actual: Vec<_> = w
            .bag_expiry
            .keys()
            .flat_map(|id| {
                let bag = &w.entities[id];
                assert!(
                    !bag.extra_stats.iter().any(|(stat, _)| *stat == 54),
                    "public drops stay public"
                );
                bag.items.iter().copied().filter(|id| *id >= 0)
            })
            .collect();
        assert_eq!(
            actual, expected,
            "one public attempt per kill, not per contributor"
        );
    }
    let mut chest_overflow = false;
    for seed in 0..4096 {
        for id in w.entities.keys().copied().collect::<Vec<_>>() {
            w.remove_entity(id);
        }
        let expected = crate::loot::landmark_chest(
            &w.content,
            0x678,
            &mut rand::rngs::StdRng::seed_from_u64(seed),
        )
        .unwrap();
        w.rng = rand::rngs::StdRng::seed_from_u64(seed);
        w.spawn_landmark_chests(0x678, Pos { x: 3.5, y: 3.5 })
            .unwrap();
        let actual: Vec<_> = w
            .entities
            .values()
            .flat_map(|e| {
                assert_eq!(e.kind, 0x501);
                assert_eq!(e.items.len(), 8);
                e.items.iter().copied().filter(|id| *id >= 0)
            })
            .collect();
        assert_eq!(actual, expected);
        if actual.len() > 8 {
            chest_overflow = true;
            break;
        }
    }
    assert!(
        chest_overflow,
        "multiple chest containers preserve every successful roll"
    );
    assert!(boundary_potion, "exact potion damage qualifies");
    assert!(
        low_damage_equipment,
        "equipment does not require potion damage"
    );
    assert!(
        duplicate_potion,
        "three potion attempts must not deduplicate successes"
    );
    assert!(
        overflow_split,
        "successful independent rewards split into multiple bags"
    );
}

#[tokio::test]
async fn god_potion_rewards_keep_damage_boundaries_and_private_ownership() {
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
    for i in 0..2 {
        let account = store.list(format!("god-potion-loot-{i}")).await.unwrap();
        let ch = store
            .create(account.id, w.content.get(crate::content::WIZARD).unwrap())
            .await
            .unwrap();
        w.enter(ch, None).unwrap();
    }
    let owners: Vec<_> = w
        .players
        .values()
        .map(|p| (p.character.account, p.character.id))
        .collect();
    for (kind, potion, required) in [(0xd05, 0xa1f, 360), (0x91a, 0xa21, 180)] {
        assert_eq!(w.content.objects[&kind].hp * 18 / 100, required);
        let mut equipment = Vec::new();
        for damage in [required - 1, required, required + 1] {
            for bag in w.bag_expiry.keys().copied().collect::<Vec<_>>() {
                w.remove_entity(bag);
            }
            let root = w
                .spawn_entity(kind, Pos { x: 3.5, y: 3.5 }, vec![])
                .unwrap();
            w.entities
                .get_mut(&root)
                .unwrap()
                .damage
                .insert(owners[0].1, damage);
            // This seed succeeds on both equipment and the 1.5% potion roll.
            w.rng = rand::rngs::StdRng::seed_from_u64(190);
            w.defeat_entity(root).unwrap();
            assert!(!w.entities.contains_key(&root));
            let mut rewards = Vec::new();
            for bag in w.bag_expiry.keys().map(|id| &w.entities[id]) {
                assert_eq!(bag.items.len(), 8);
                assert!(bag.extra_stats.contains(&(54, owners[0].0)));
                assert!(bag.accessible(owners[0].0));
                assert!(!bag.accessible(owners[1].0));
                rewards.extend(bag.items.iter().copied().filter(|item| *item >= 0));
            }
            assert_eq!(
                rewards.contains(&potion),
                damage >= required,
                "{kind:#x} potion qualification at {damage} damage"
            );
            equipment.push(
                rewards
                    .into_iter()
                    .filter(|item| *item != potion)
                    .collect::<Vec<_>>(),
            );
        }
        assert!(
            !equipment[0].is_empty(),
            "subthreshold damage still earns gear"
        );
        assert!(
            equipment.iter().all(|items| *items == equipment[0]),
            "potion qualification must not change gear"
        );
    }
}
