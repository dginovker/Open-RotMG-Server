use super::*;

#[tokio::test]
async fn hermit_bags_use_its_eight_drop_sites_and_keep_solo_potions() {
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
    let account = store.list("hermit-loot".into()).await.unwrap();
    let ch = store
        .create(account.id, w.content.get(crate::content::WIZARD).unwrap())
        .await
        .unwrap();
    let character = ch.id;
    w.enter(ch, None).unwrap();
    let home = Pos { x: 8., y: 7.5 };
    let root = w
        .insert_entity(crate::dungeon::trench::HERMIT, home, vec![])
        .unwrap();
    let sites: Vec<_> = super::ocean_setpiece::HERMIT_SPAWNERS
        .iter()
        .map(|(x, y)| (home.x + *x as f32 - 15.5, home.y + *y as f32 - 13.))
        .collect();
    let mut seen = BTreeSet::new();
    for seed in 0..64 {
        for bag in w.bag_expiry.keys().copied().collect::<Vec<_>>() {
            w.remove_entity(bag);
        }
        let mut defeated = w.entities[&root].clone();
        defeated.damage = [(character, 55_000)].into();
        w.rng = rand::rngs::StdRng::seed_from_u64(seed);
        w.drop_loot(&defeated).unwrap();
        let mut items = vec![];
        for bag in w.bag_expiry.keys().map(|id| &w.entities[id]) {
            let site = (bag.pos.x, bag.pos.y);
            assert!(sites.contains(&site), "seed {seed}: bag at {site:?}");
            seen.insert((site.0.to_bits(), site.1.to_bits()));
            items.extend(bag.items.iter().copied().filter(|item| *item >= 0));
        }
        for potion in [0xa34, 0xa4c] {
            assert!(items.contains(&potion), "seed {seed}: {items:?}");
        }
    }
    assert_eq!(seen.len(), 8, "Hermit bags must reach every drop site");
}
