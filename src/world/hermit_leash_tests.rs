use super::*;
use crate::{
    content::Cell,
    encounter::{Anchor, Motion},
};

#[test]
fn hermit_attack_leash_fits_the_central_dark_water_pool() {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let map = Map {
        width: 128,
        height: 128,
        cells: vec![
            Cell {
                ground: 0x46,
                object: None,
                config: String::new(),
                terrain: 6,
                region: 0
            };
            128 * 128
        ],
        spawns: vec![(1.5, 1.5)],
    };
    let mut world = World::new(content, Arc::new(map)).unwrap();
    world.populate_hermit().unwrap();
    let id = world
        .entities
        .values()
        .find(|e| e.kind == crate::dungeon::trench::HERMIT)
        .unwrap()
        .id;
    let home = world.entities[&id].home;
    let target = Pos {
        x: home.x,
        y: home.y + 7.,
    };
    for stage in [4, 5] {
        world.entities.get_mut(&id).unwrap().stage = Some(stage);
        for radius in [0., 2.99, 3., 3.2, 4.9, 5.5] {
            for n in 0..64 {
                let angle = n as f32 * std::f32::consts::TAU / 64.;
                let pos = Pos {
                    x: home.x + radius * angle.cos(),
                    y: home.y + radius * angle.sin(),
                };
                world.position_entity(id, pos).unwrap();
                let (motion, rate) = world.ocean_movement(id, Some(target), 0);
                assert_eq!(rate, crate::encounter::nr_speed(0.3));
                if radius > 3. {
                    assert_eq!(
                        motion,
                        Motion::Protect {
                            anchor: Anchor::Home,
                            distance: 0.
                        },
                        "stage {stage} at radius {radius}"
                    );
                } else if radius < 3. {
                    assert_eq!(motion, Motion::Wander);
                }
                assert_eq!(world.ocean_movement(id, None, 0), (Motion::Stationary, 0.));
            }
        }
        for n in 0..360 {
            world.position_entity(id, home).unwrap();
            let angle = n as f32 * std::f32::consts::TAU / 360.;
            let outward = Pos {
                x: home.x + 8. * angle.cos(),
                y: home.y + 8. * angle.sin(),
            };
            for _ in 0..80 {
                let (motion, rate) = world.ocean_movement(id, Some(target), 0);
                let destination = match motion {
                    Motion::Wander => outward,
                    Motion::Protect {
                        anchor: Anchor::Home,
                        ..
                    } => home,
                    other => panic!("unexpected Hermit motion: {other:?}"),
                };
                if let Err(error) =
                    world.move_entity_colliding(id, destination, rate / 1.5 * 0.05, true)
                {
                    assert_eq!(error.to_string(), "NPC movement blocked on both axes");
                }
                let pos = world.entities[&id].pos;
                assert_eq!(
                    world.map.cell(pos.x, pos.y).unwrap().ground,
                    0xbc,
                    "stage {stage} left dark water at {n} degrees: {pos:?}"
                );
            }
        }
    }
    let land = Pos { x: 1.5, y: 1.5 };
    assert!(world.walkable(land));
    assert!(!world.ocean_walkable(crate::dungeon::trench::HERMIT, land));
    for kind in [0xd62, 0xd63, 0xd64] {
        assert!(world.ocean_walkable(kind, land));
        assert!(world.ocean_walkable(kind, home));
    }
}
