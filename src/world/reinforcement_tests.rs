use super::*;

#[test]
fn tracked_children_preserve_wave_selection_deadlines_and_lifecycle() {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let mut map = Map::load(std::path::Path::new("data/tutorial.wmap"), &content).unwrap();
    for cell in &mut map.cells {
        cell.ground = 0x61;
        cell.object = None;
    }
    let mut w = World::new(content, Arc::new(map)).unwrap();
    let parent = w
        .spawn_entity(0x668, Pos { x: 10.5, y: 50.5 }, vec![])
        .unwrap();
    let other = w
        .spawn_entity(0x668, Pos { x: 10.5, y: 65.5 }, vec![])
        .unwrap();
    let check = |w: &World| {
        for owner in [parent, other] {
            for kind in [0x669, 0x66a, 0x66b, 0x122] {
                assert_eq!(
                    w.summoned_children(owner, kind)
                        .map(|e| e.id)
                        .collect::<Vec<_>>(),
                    w.entities
                        .values()
                        .filter(|e| e.parent == Some(owner) && e.kind == kind)
                        .map(|e| e.id)
                        .collect::<Vec<_>>()
                );
                assert_eq!(
                    w.summoned_children(owner, kind)
                        .filter(|e| w.actor_ids.contains(&e.id))
                        .count(),
                    w.actors()
                        .filter(|e| e.parent == Some(owner) && e.kind == kind)
                        .count()
                );
            }
        }
    };
    let existing = w
        .spawn_entity(0x669, Pos { x: 12.5, y: 50.5 }, vec![])
        .unwrap();
    let expires = w
        .spawn_entity(0x669, Pos { x: 12.5, y: 51.5 }, vec![])
        .unwrap();
    let unrelated = w
        .spawn_entity(0x669, Pos { x: 12.5, y: 65.5 }, vec![])
        .unwrap();
    let scenery = w
        .spawn_entity(0x122, Pos { x: 18.5, y: 50.5 }, vec![])
        .unwrap();
    assert!(!w.actor_ids.contains(&scenery));
    for id in [existing, expires, scenery] {
        w.entities
            .get_mut(&id)
            .unwrap()
            .parent
            .set(Some(parent), id, &mut w.timed_ids);
    }
    w.entities
        .get_mut(&unrelated)
        .unwrap()
        .expires
        .set(Some(9999), unrelated, &mut w.timed_ids);
    w.entities.get_mut(&existing).unwrap().activated = Some(7);
    w.entities
        .get_mut(&expires)
        .unwrap()
        .expires
        .set(Some(9000), expires, &mut w.timed_ids);
    check(&w);
    w.entities
        .get_mut(&scenery)
        .unwrap()
        .parent
        .set(Some(other), scenery, &mut w.timed_ids);
    check(&w);
    w.entities
        .get_mut(&scenery)
        .unwrap()
        .parent
        .set(None, scenery, &mut w.timed_ids);
    check(&w);
    w.now = 5000;
    let wave = finale::Wave {
        kind: 0x669,
        count: 3,
        radius: 3.,
        lifetime: 60000,
    };
    w.finale_wave(parent, wave).unwrap();
    check(&w);
    assert_eq!(w.summoned_children(parent, 0x669).count(), 3);
    assert_eq!(*w.entities[&existing].expires, Some(65000));
    assert_eq!(w.entities[&existing].activated, Some(7));
    assert_eq!(*w.entities[&expires].expires, Some(9000));
    assert_eq!(w.entities[&expires].activated, None);
    assert_eq!(*w.entities[&unrelated].parent, None);
    assert_eq!(*w.entities[&unrelated].expires, Some(9999));
    let born = w
        .summoned_children(parent, 0x669)
        .find(|e| e.id != existing && e.id != expires)
        .unwrap()
        .id;
    assert_eq!(*w.entities[&born].expires, Some(65000));
    assert_eq!(w.entities[&born].activated, Some(5000));
    w.remove_entity(expires);
    check(&w);
    let replacement = w.transform_fauna(existing, 0x66a).unwrap();
    assert_eq!(*w.entities[&replacement].parent, Some(parent));
    check(&w);
    w.now = 65001;
    w.mechanic_expiry();
    assert!(!w.entities.contains_key(&born));
    check(&w);
}
