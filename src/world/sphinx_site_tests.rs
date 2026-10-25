use super::*;

#[test]
fn sphinx_site_keeps_the_complete_reference_sand_star() {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let map = Map {
        width: 256,
        height: 256,
        cells: vec![
            crate::content::Cell {
                ground: 0x57,
                object: None,
                config: String::new(),
                terrain: 1,
                region: 0,
            };
            256 * 256
        ],
        spawns: vec![(1.5, 1.5)],
    };
    let original = Arc::new(map);
    let mut world = World::new(content, original.clone()).unwrap();
    world.rng = rand::rngs::StdRng::seed_from_u64(12351);
    let mut expected_rng = world.rng.clone();
    let boss = world
        .place_named_setpiece(crate::dungeon::tomb::SPHINX, &[1])
        .unwrap();
    let origin = (boss.x as usize - 40, boss.y as usize - 40);
    let rows: Vec<_> = "
        ........#........
        ........#........
        .......###.......
        ...###.###.###...
        ...###########...
        ...###########...
        ....#########....
        ..#############..
        #################
        ..#############..
        ....#########....
        ...###########...
        ...###########...
        ...###.###.###...
        .......###.......
        ........#........
        ........#........
    "
    .split_whitespace()
    .collect();
    for y in 0..81 {
        for x in 0..81 {
            let floor = (x as f32 - 40.5).hypot(y as f32 - 40.5)
                + expected_rng.gen_range(-2.0_f32..2.0)
                <= 35.;
            let expected = (32..49).contains(&x)
                && (32..49).contains(&y)
                && rows[y - 32].as_bytes()[x - 32] == b'#';
            let cell = &world.map.cells[(origin.1 + y) * 256 + origin.0 + x];
            assert_eq!(
                cell.ground,
                if expected {
                    0xf7
                } else if floor {
                    0xf5
                } else {
                    0x57
                },
                "local tile ({x},{y})"
            );
            assert_eq!((cell.terrain, cell.region), (1, 0));
            if !floor {
                assert!(cell.object.is_none() && cell.config.is_empty());
            }
        }
    }
    assert_eq!(
        world
            .entities
            .values()
            .filter(|e| e.kind == crate::dungeon::tomb::WALL)
            .count(),
        24
    );
    for wall in world
        .entities
        .values()
        .filter(|e| e.kind == crate::dungeon::tomb::WALL)
    {
        assert!(wall.published && !world.walkable(wall.pos));
        let index = wall.pos.y as usize * 256 + wall.pos.x as usize;
        assert!(world.cells.ids(index).contains(&wall.id));
        assert_eq!(world.map.cells[index].object, Some(wall.kind));
    }
    assert!(!Arc::ptr_eq(&original, &world.map));
    assert!(
        original
            .cells
            .iter()
            .all(|c| c.ground == 0x57 && c.object.is_none() && c.config.is_empty())
    );
}
