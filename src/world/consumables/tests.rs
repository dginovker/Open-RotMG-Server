use super::*;

#[test]
fn potion_feedback_uses_light_blue_for_mana_and_green_for_health_after_commit() {
    let data = std::path::Path::new("data");
    let content = Arc::new(Content::load(data).unwrap());
    let map = Arc::new(Map::load(&data.join("nexus.wmap"), &content).unwrap());
    for (mana, deficit, quiet, gain) in [
        (true, 150, false, 100),
        (true, 25, false, 25),
        (true, 0, false, 0),
        (true, 150, true, 0),
        (false, 25, false, 25),
    ] {
        let mut world = World::new(content.clone(), map.clone()).unwrap();
        let base = content.classes[&crate::content::WIZARD].caps;
        let kind = if mana { 0xa23 } else { 0xa22 };
        let character: Character = serde_json::from_value(serde_json::json!({
            "schema":1,"lifetime":crate::store::FameLifetime::complete(),
            "class":crate::content::WIZARD,"base":base,
            "id":1,"account":1,"name":"Potion","level":20,"exp":0,
            "fame":0,"hp":base[0],"mp":base[1],"revision":0,"alive":true,
            "items":[-1,-1,-1,-1,kind,-1,-1,-1,-1,-1,-1,-1]
        }))
        .unwrap();
        let (packets, mut received) = mpsc::channel(64);
        let (lost, _loss) = watch::channel(None);
        let id = world
            .enter(character, Some(Output { packets, lost }))
            .unwrap();
        let p = world.players.get_mut(&id).unwrap();
        p.known.insert(id);
        if mana {
            p.character.mp -= deficit;
        } else {
            p.character.hp -= deficit;
        }
        if quiet {
            world
                .apply_condition(id, crate::condition::QUIET, 1000)
                .unwrap();
        }
        let p = &world.players[&id];
        let before = if mana { p.character.mp } else { p.character.hp };
        while received.try_recv().is_ok() {}
        let slot = Slot {
            object: id,
            slot: 4,
            kind,
        };
        let plan = world
            .use_item(id, slot, world.players[&id].pos)
            .unwrap()
            .unwrap();
        assert!(
            received.try_recv().is_err(),
            "feedback must wait for commit"
        );
        assert_eq!(world.players[&id].character.items[4], kind);
        // This packet-level test supplies the inventory commit receipt, not a database durability proof.
        world.finish_inventory(plan, Ok(1)).unwrap();
        let p = &world.players[&id];
        assert_eq!(p.character.items[4], -1);
        assert_eq!(p.character.revision, 1);
        assert_eq!(
            if mana { p.character.mp } else { p.character.hp },
            before + gain
        );
        let notifications: Vec<_> = std::iter::from_fn(|| received.try_recv().ok())
            .filter(|packet| matches!(packet.id, wire::NOTIFICATION | wire::SHOW_EFFECT))
            .collect();
        assert_eq!(notifications.len(), if gain > 0 { 2 } else { 0 });
        if gain > 0 {
            assert_eq!(notifications[0].id, wire::NOTIFICATION);
            let mut reader = wire::Reader::new(&notifications[0].body);
            assert_eq!(reader.i32().unwrap(), id);
            assert_eq!(reader.utf().unwrap(), format!("+{gain}"));
            assert_eq!(
                reader.i32().unwrap() as u32,
                if mana { 0xff6084e0 } else { 0xff00ff00 }
            );
            reader.done().unwrap();
            assert_eq!(notifications[1].id, wire::SHOW_EFFECT);
            let mut reader = wire::Reader::new(&notifications[1].body);
            assert_eq!(reader.u8().unwrap(), 1);
            assert_eq!(reader.i32().unwrap(), id);
            for _ in 0..4 {
                assert_eq!(reader.f32().unwrap(), 0.);
            }
            assert_eq!(reader.i32().unwrap() as u32, 0xffffffff);
            reader.done().unwrap();
        }
        assert!(world.use_item(id, slot, world.players[&id].pos).is_err());
        assert_eq!(world.players[&id].character.revision, 1);
        assert!(received.try_recv().is_err());
    }
}
