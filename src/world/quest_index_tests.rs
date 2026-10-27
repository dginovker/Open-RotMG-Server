use super::*;

#[test]
fn quest_indexes_preserve_query_scopes_and_lifecycle() {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let mut map = Map::load(std::path::Path::new("data/tutorial.wmap"), &content).unwrap();
    for cell in &mut map.cells {
        cell.ground = 0x61;
        cell.object = None;
    }
    let mut w = World::new(content, Arc::new(map)).unwrap();
    let [root, other] = [50.5, 65.5].map(|y| {
        let id = w.spawn_entity(0x928, Pos { x: 10.5, y }, vec![]).unwrap();
        w.entities
            .get_mut(&id)
            .unwrap()
            .quest
            .set(Some(id), id, &mut w.quest_ids);
        assert!(!w.timed_ids.contains(&id));
        id
    });
    let check = |w: &World| {
        for tag in [root, other] {
            assert_eq!(
                w.quest_entities(tag).map(|e| e.id).collect::<Vec<_>>(),
                w.entities
                    .values()
                    .filter(|e| e.quest == Some(tag))
                    .map(|e| e.id)
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                w.quest_actors(tag).map(|e| e.id).collect::<Vec<_>>(),
                w.actors()
                    .filter(|e| e.quest == Some(tag))
                    .map(|e| e.id)
                    .collect::<Vec<_>>()
            );
            for kind in [0x928, 0x929, 0x92b, 0x669, 0x122] {
                assert_eq!(
                    w.landmark_members(tag, kind),
                    w.actors()
                        .filter(|e| e.quest == Some(tag) && e.kind == kind)
                        .map(|e| e.id)
                        .collect::<Vec<_>>()
                );
                assert_eq!(
                    w.quest_members(tag, &[kind]),
                    w.non_population_actors()
                        .filter(|e| e.quest == Some(tag) && e.kind == kind)
                        .map(|e| e.id)
                        .collect::<Vec<_>>()
                );
            }
        }
        assert_eq!(
            w.summon_count(),
            w.actors()
                .filter(|e| e.parent.is_some() || e.quest.is_some())
                .count()
        );
        assert_eq!(
            w.summon_entities().map(|e| e.id).collect::<Vec<_>>(),
            w.entities
                .values()
                .filter(|e| e.parent.is_some() || e.quest.is_some())
                .map(|e| e.id)
                .collect::<Vec<_>>()
        );
    };
    let center = w.entities[&root].pos;
    w.quest_birth((root, center, center), 0x929, 1, 1, 3.)
        .unwrap();
    let child = w.quest_members(root, &[0x929])[0];
    assert!(w.entities[&child].expires.is_none());
    assert!(!w.timed_ids.contains(&child));
    let expires = w
        .spawn_entity(0x669, Pos { x: 14.5, y: 56.5 }, vec![])
        .unwrap();
    w.entities
        .get_mut(&expires)
        .unwrap()
        .expires
        .set(Some(9000), expires, &mut w.timed_ids);
    let parented = w
        .spawn_entity(0x669, Pos { x: 14.5, y: 55.5 }, vec![])
        .unwrap();
    w.entities
        .get_mut(&parented)
        .unwrap()
        .parent
        .set(Some(root), parented, &mut w.timed_ids);
    let scenery = w
        .spawn_entity(0x122, Pos { x: 18.5, y: 55.5 }, vec![])
        .unwrap();
    assert!(!w.actor_ids.contains(&scenery));
    let e = w.entities.get_mut(&scenery).unwrap();
    e.quest.set(Some(root), scenery, &mut w.quest_ids);
    e.parent.set(Some(root), scenery, &mut w.timed_ids);
    check(&w);
    w.entities
        .get_mut(&child)
        .unwrap()
        .parent
        .set(Some(root), child, &mut w.timed_ids);
    check(&w);
    w.entities
        .get_mut(&child)
        .unwrap()
        .quest
        .set(None, child, &mut w.quest_ids);
    assert!(w.quest_ids.contains(&child));
    check(&w);
    w.entities
        .get_mut(&child)
        .unwrap()
        .quest
        .set(Some(other), child, &mut w.quest_ids);
    check(&w);
    // Membership helpers retain their existing distinction between all actors and scripted actors.
    assert!(w.non_population_ids.remove(&child));
    check(&w);
    w.non_population_ids.insert(child);
    let replacement = w.quest_transform(child, 0x92b).unwrap();
    assert_eq!(*w.entities[&replacement].quest, Some(other));
    assert!(!w.quest_ids.contains(&child));
    assert!(w.quest_ids.contains(&replacement));
    check(&w);
    w.remove_entity(scenery);
    assert!(!w.quest_ids.contains(&scenery));
    check(&w);
    w.now = w.entities[&replacement].expires.unwrap() + 1;
    w.mechanic_expiry();
    assert!(!w.entities.contains_key(&replacement));
    assert!(!w.quest_ids.contains(&replacement));
    check(&w);
}
