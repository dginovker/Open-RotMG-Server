use super::*;

#[tokio::test]
async fn sphinx_potion_minima_are_private_and_shared_by_eligible_contributors() {
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
        let account = store.list(format!("sphinx-loot-{i}")).await.unwrap();
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
    let sphinx = crate::dungeon::tomb::SPHINX;
    let root = w
        .spawn_entity(sphinx, Pos { x: 3.5, y: 3.5 }, vec![])
        .unwrap();
    let mut only_shared_minimum = false;
    let mut boundary_rewarded = false;
    for damage in [[55_000, 0, 0], [54_450, 550, 549], [549, 0, 0]] {
        for seed in 0..32 {
            for bag in w.bag_expiry.keys().copied().collect::<Vec<_>>() {
                w.remove_entity(bag);
            }
            let mut defeated = w.entities[&root].clone();
            defeated.damage = owners
                .iter()
                .zip(damage)
                .map(|((_, character), damage)| (*character, damage))
                .collect();
            w.rng = rand::rngs::StdRng::seed_from_u64(seed);
            w.drop_loot(&defeated).unwrap();
            let mut rewards = BTreeMap::<i32, Vec<i32>>::new();
            for bag in w.bag_expiry.keys().map(|id| &w.entities[id]) {
                let owner = bag
                    .extra_stats
                    .iter()
                    .find(|(stat, _)| *stat == 54)
                    .expect("private Sphinx bag")
                    .1;
                assert!(
                    owners
                        .iter()
                        .zip(damage)
                        .any(|((account, _), damage)| *account == owner && damage >= 550)
                );
                rewards
                    .entry(owner)
                    .or_default()
                    .extend(bag.items.iter().copied().filter(|item| *item >= 0));
            }
            if damage[0] < 550 {
                assert!(rewards.is_empty());
                continue;
            }
            for item in [0xa34, 0xa35] {
                let count = rewards
                    .values()
                    .flatten()
                    .filter(|kind| **kind == item)
                    .count();
                assert!(
                    (1..=damage.iter().filter(|n| **n >= 550).count()).contains(&count),
                    "seed {seed}: {rewards:?}"
                );
            }
            assert!(rewards.values().all(
                |items| items.iter().filter(|kind| **kind == 0xa34).count() <= 1
                    && items.iter().filter(|kind| **kind == 0xa35).count() <= 1
            ));
            only_shared_minimum |=
                damage[1] == 550 && rewards.values().map(Vec::len).sum::<usize>() == 2;
            boundary_rewarded |= damage[1] == 550 && rewards.contains_key(&owners[1].0);
        }
    }
    assert!(
        only_shared_minimum,
        "required pair is encounter-wide, not per raider"
    );
    assert!(boundary_rewarded, "exactly one percent damage qualifies");
}
