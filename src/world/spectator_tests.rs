use super::spectator::*;
use super::*;
use crate::wire::Reader;

fn status(out: &mut Writer, id: i32, fields: &[(u8, i32)]) {
    out.i32(id).f32(1.5).f32(2.5).i16(fields.len() as i16);
    for &(key, value) in fields {
        out.u8(key).i32(value);
    }
}

#[test]
fn spectator_projection_redacts_human_backpacks_containers_wallets_and_nonvisual_packets() {
    let content = Content::load(std::path::Path::new("data")).unwrap();
    let container = content
        .objects
        .values()
        .find(|o| o.class == "Container")
        .unwrap()
        .kind;
    let secret = 0x12345678i32;
    let mut update = Writer::new();
    update.i16(0).i16(3).i16(crate::content::WIZARD as i16);
    status(
        &mut update,
        7,
        &[(0, 100), (8, 2590), (12, 2594), (35, secret), (38, secret)],
    );
    update.i16(crate::content::WIZARD as i16);
    status(
        &mut update,
        8,
        &[
            (8, 2711),
            (12, secret),
            (19, secret),
            (39, secret),
            (120, secret),
        ],
    );
    update.i16(container as i16);
    status(
        &mut update,
        9,
        &[(0, 100), (8, secret), (12, secret), (54, secret)],
    );
    update.i16(0);
    let mut projection = Projection::new(7);
    let filtered = projection
        .packet(
            Packet {
                id: wire::UPDATE,
                body: update.0,
            },
            &content,
        )
        .unwrap()
        .unwrap();
    assert!(!filtered.body.windows(4).any(|b| b == secret.to_be_bytes()));
    let mut expected = Writer::new();
    expected.i16(0).i16(3).i16(crate::content::WIZARD as i16);
    status(&mut expected, 7, &[(0, 100), (8, 2590), (12, 2594)]);
    expected.i16(crate::content::WIZARD as i16);
    status(&mut expected, 8, &[(8, 2711)]);
    expected.i16(container as i16);
    status(&mut expected, 9, &[(0, 100)]);
    expected.i16(0);
    assert_eq!(filtered.body, expected.0);
    let mut tick = Writer::new();
    tick.i32(1).i32(100).i16(3);
    status(&mut tick, 7, &[(12, 2595), (35, secret)]);
    status(&mut tick, 8, &[(8, 2711), (12, secret)]);
    status(&mut tick, 9, &[(8, secret), (12, secret)]);
    let filtered = projection
        .packet(
            Packet {
                id: wire::NEW_TICK,
                body: tick.0,
            },
            &content,
        )
        .unwrap()
        .unwrap();
    assert!(!filtered.body.windows(4).any(|b| b == secret.to_be_bytes()));
    for id in [wire::ACCOUNT_LIST, 41, 18, 33, 37, 47, 48, 0, 255] {
        assert!(
            projection
                .packet(
                    Packet {
                        id,
                        body: b"PRIVATE-TELL-GUILD-TRADE-CREDENTIAL-SENTINEL".to_vec()
                    },
                    &content
                )
                .unwrap()
                .is_none()
        );
    }
    let text = |actor, recipient: &str| {
        let mut w = Writer::new();
        w.utf("Player")
            .i32(actor)
            .i32(0)
            .u8(5)
            .utf(recipient)
            .utf("PRIVATE-SENTINEL")
            .utf("");
        Packet {
            id: wire::TEXT,
            body: w.0,
        }
    };
    for packet in [text(8, "Triceps"), text(-1, ""), text(8, "*Guild*")] {
        assert!(projection.packet(packet, &content).unwrap().is_none());
    }
    assert!(projection.packet(text(8, ""), &content).unwrap().is_some());
    let malformed = Packet {
        id: wire::NEW_TICK,
        body: vec![0; 2],
    };
    assert!(projection.packet(malformed, &content).is_err());
}

#[test]
fn spectator_preserves_public_merchant_and_connection_stats_without_promoting_player_or_unknown_fields()
 {
    let content = Content::load(std::path::Path::new("data")).unwrap();
    let merchant = content
        .objects
        .values()
        .find(|o| o.class == "Merchant")
        .unwrap()
        .kind;
    let public = [
        (34, 0xa97),
        (36, 100),
        (37, 0),
        (40, 1),
        (41, 3),
        (42, 2),
        (43, 20),
        (44, 1),
        (45, 2),
        (55, 3),
    ];
    let secret = 0x12345678;
    let mut fields = public.to_vec();
    fields.extend([
        (8, secret),
        (35, secret),
        (38, secret),
        (39, secret),
        (54, secret),
    ]);
    let mut update = Writer::new();
    update.i16(0).i16(3).i16(crate::content::WIZARD as i16);
    status(&mut update, 7, &public);
    update.i16(crate::content::WIZARD as i16);
    status(&mut update, 8, &public);
    update.i16(merchant as i16);
    status(&mut update, 9, &fields);
    update.i16(0);
    let mut projection = Projection::new(7);
    let filtered = projection
        .packet(
            Packet {
                id: wire::UPDATE,
                body: update.0,
            },
            &content,
        )
        .unwrap()
        .unwrap();
    let mut expected = Writer::new();
    expected.i16(0).i16(3).i16(crate::content::WIZARD as i16);
    status(&mut expected, 7, &[]);
    expected.i16(crate::content::WIZARD as i16);
    status(&mut expected, 8, &[]);
    expected.i16(merchant as i16);
    status(&mut expected, 9, &public);
    expected.i16(0);
    assert_eq!(filtered.body, expected.0);
    let mut tick = Writer::new();
    tick.i32(1).i32(100).i16(2);
    status(&mut tick, 9, &public);
    status(&mut tick, 99, &public);
    let filtered = projection
        .packet(
            Packet {
                id: wire::NEW_TICK,
                body: tick.0,
            },
            &content,
        )
        .unwrap()
        .unwrap();
    let mut expected = Writer::new();
    expected.i32(1).i32(100).i16(2);
    status(&mut expected, 9, &public);
    status(&mut expected, 99, &[]);
    assert_eq!(filtered.body, expected.0);
    let mut drop = Writer::new();
    drop.i16(0).i16(0).i16(1).i32(9);
    projection
        .packet(
            Packet {
                id: wire::UPDATE,
                body: drop.0,
            },
            &content,
        )
        .unwrap();
    let mut tick = Writer::new();
    tick.i32(2).i32(100).i16(1);
    status(&mut tick, 9, &public);
    let filtered = projection
        .packet(
            Packet {
                id: wire::NEW_TICK,
                body: tick.0,
            },
            &content,
        )
        .unwrap()
        .unwrap();
    let mut expected = Writer::new();
    expected.i32(2).i32(100).i16(1);
    status(&mut expected, 9, &[]);
    assert_eq!(filtered.body, expected.0);
}

#[test]
fn perspective_visibility_is_read_only_and_rejects_human_stale_unknown_or_departing_actors() {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let map = Arc::new(Map::load(std::path::Path::new("data/tutorial.wmap"), &content).unwrap());
    let mut w = World::new(content.clone(), map).unwrap();
    let mut actors = Vec::new();
    for account in 1..=2 {
        let mut ch = Character::fresh(content.get(crate::content::WIZARD).unwrap()).unwrap();
        ch.id = account;
        ch.account = account;
        ch.name = format!("Player{account}");
        let id = w.enter(ch, None).unwrap();
        w.players.get_mut(&id).unwrap().controller = Controller::Autonomous;
        actors.push(id);
    }
    let (source, target) = ((actors[0], 1), (actors[1], 2));
    w.updates(source.0, true).unwrap();
    let before: Vec<_> = w
        .players
        .values()
        .map(|p| serde_json::to_value(&p.character).unwrap())
        .collect();
    assert!(w.perspective_visible(source, target));
    assert!(!w.perspective_visible((source.0, 99), target));
    assert!(!w.perspective_visible(source, (target.0, 99)));
    assert!(!w.perspective_visible(source, source));
    w.players.get_mut(&target.0).unwrap().controller = Controller::Human;
    assert!(!w.perspective_visible(source, target));
    w.players.get_mut(&target.0).unwrap().controller = Controller::Autonomous;
    w.players.get_mut(&target.0).unwrap().departing = true;
    assert!(!w.perspective_visible(source, target));
    w.players.get_mut(&target.0).unwrap().departing = false;
    w.players
        .get_mut(&source.0)
        .unwrap()
        .known
        .remove(&target.0);
    assert!(!w.perspective_visible(source, target));
    w.players.get_mut(&source.0).unwrap().known.insert(target.0);
    assert!(w.perspective_visible(source, target));
    assert_eq!(
        before,
        w.players
            .values()
            .map(|p| serde_json::to_value(&p.character).unwrap())
            .collect::<Vec<_>>()
    );
    w.players.remove(&target.0);
    assert!(!w.perspective_visible(source, target));
}

#[test]
fn spectator_snapshot_subscription_has_no_avatar_and_is_bounded_and_closed_on_retirement() {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let map = Arc::new(Map::load(std::path::Path::new("data/tutorial.wmap"), &content).unwrap());
    let mut w = World::new(content.clone(), map).unwrap();
    let mut ch = Character::fresh(content.get(crate::content::WIZARD).unwrap()).unwrap();
    ch.id = 1;
    ch.account = 1;
    ch.name = "Triceps".into();
    let player = w.enter(ch, None).unwrap();
    w.players.get_mut(&player).unwrap().controller = Controller::Autonomous;
    let quest = w
        .spawn_entity(1620, w.players[&player].pos, vec![])
        .unwrap();
    w.updates(player, true).unwrap();
    let own = w.players.get_mut(&player).unwrap();
    own.quest = Some(quest);
    own.oxygen = 61;
    own.character.xp_boost.active = true;
    let full = w.stats(player, player).unwrap();
    w.players
        .get_mut(&player)
        .unwrap()
        .tick_stats
        .encode(player, full);
    let before = serde_json::to_value(&w.players[&player].character).unwrap();
    let mut attached = w.attach_spectator(player).unwrap();
    assert_eq!(w.players.len(), 1);
    assert_eq!(
        serde_json::to_value(&w.players[&player].character).unwrap(),
        before
    );
    let mut projection = Projection::new(player);
    let mut seeded = BTreeMap::new();
    let mut seeded_quest = false;
    for packet in attached.snapshot {
        let filtered = projection.packet(packet, &content).unwrap().unwrap();
        if filtered.id == 34 {
            assert_eq!(filtered.body, quest.to_be_bytes());
            seeded_quest = true;
            continue;
        }
        let mut r = Reader::new(&filtered.body);
        let tiles = r.i16().unwrap();
        for _ in 0..tiles {
            r.i16().unwrap();
            r.i16().unwrap();
            r.u8().unwrap();
        }
        let objects = r.i16().unwrap();
        for _ in 0..objects {
            r.i16().unwrap();
            let actor = r.i32().unwrap();
            r.f32().unwrap();
            r.f32().unwrap();
            let fields = r.i16().unwrap();
            for _ in 0..fields {
                let key = r.u8().unwrap();
                if matches!(key, 31 | 62) {
                    r.utf().unwrap();
                } else {
                    let value = r.i32().unwrap();
                    if actor == player {
                        seeded.insert(key, value);
                    }
                }
            }
        }
    }
    assert!(seeded_quest);
    assert_eq!(seeded[&64], 61);
    assert_eq!(seeded[&65], 1);
    let mut departed = Character::fresh(content.get(crate::content::WIZARD).unwrap()).unwrap();
    departed.id = 2;
    departed.account = 2;
    departed.name = "Departed".into();
    let other = w.enter(departed, None).unwrap();
    w.updates(player, false).unwrap();
    assert!(w.players[&player].known.contains(&other));
    w.players.remove(&other);
    drop(w.attach_spectator(player).unwrap());
    while let Ok(packet) = attached.receiver.try_recv() {
        projection.packet(packet, &content).unwrap();
    }
    let mut private = Writer::new();
    private
        .utf("Player")
        .i32(player)
        .i32(0)
        .u8(0)
        .utf("Triceps")
        .utf("PRIVATE-TELL-SENTINEL")
        .utf("");
    send(&w.players[&player], wire::TEXT, private.0);
    assert!(
        projection
            .packet(attached.receiver.try_recv().unwrap(), &content)
            .unwrap()
            .is_none()
    );
    for _ in 0..300 {
        send(&w.players[&player], wire::GOTO, vec![0; 12]);
    }
    assert!(matches!(
        attached.receiver.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_))
    ));
    while attached.receiver.try_recv().is_ok() {}
    w.players.remove(&player);
    assert!(matches!(
        attached.receiver.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Closed)
    ));
}
