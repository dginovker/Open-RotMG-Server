use super::*;

#[tokio::test]
async fn abyss_brutes_round_a_doorway_corner_instead_of_stacking_on_it() {
    // Generated Abyss doorway where pre-jitter Brutes pinned onto one wall point.
    const ROWS: [&str; 15] = [
        "###........#########",
        "###........##.......",
        "###........##.......",
        "###........##.......",
        "###.................",
        "###.................",
        "###.................",
        "###........##.......",
        "#####...#####.......",
        "#####...############",
        "##........##########",
        "##........##########",
        "##........##########",
        "##........##########",
        "##........##########",
    ];
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let mut map = Map::load(std::path::Path::new("data/tutorial.wmap"), &content).unwrap();
    let goal = Pos { x: 8.5, y: 11.5 };
    map.spawns = vec![(goal.x, goal.y)];
    for (i, cell) in map.cells.iter_mut().enumerate() {
        let (x, y) = (i % map.width as usize, i / map.width as usize);
        let open = ROWS.get(y).and_then(|row| row.as_bytes().get(x)) == Some(&b'.');
        cell.ground = 0x61;
        cell.object = (!open).then_some(0x150);
    }
    let map = Arc::new(map);
    let store = crate::store::Store::open(std::path::Path::new(":memory:"), &content).unwrap();
    let account = store.list("chase-corner".into()).await.unwrap();
    let ch = store
        .create(account.id, content.get(crate::content::WIZARD).unwrap())
        .await
        .unwrap();
    for seed in 0..64 {
        let mut w = World::new(content.clone(), map.clone()).unwrap();
        for id in w.entities.keys().copied().collect::<Vec<_>>() {
            if !w.content.objects[&w.entities[&id].kind].static_object {
                w.remove_entity(id);
            }
        }
        w.encounter = true;
        w.rng = rand::rngs::StdRng::seed_from_u64(seed);
        w.enter(ch.clone(), None).unwrap();
        let brutes = [
            w.spawn_entity(0x671, Pos { x: 13.5, y: 3.5 }, vec![])
                .unwrap(),
            w.spawn_entity(0x672, Pos { x: 10.5, y: 6.5 }, vec![])
                .unwrap(),
        ];
        let mut arrived = [false; 2];
        for _ in 0..400 {
            w.now += 50;
            w.npc_step();
            for (arrived, id) in arrived.iter_mut().zip(brutes) {
                *arrived |= w.entities[&id].pos.y > 10.;
            }
        }
        assert_eq!(
            arrived,
            [true; 2],
            "seed {seed}: Brutes pinned at {:?}",
            brutes.map(|id| w.entities[&id].pos)
        );
    }
}
