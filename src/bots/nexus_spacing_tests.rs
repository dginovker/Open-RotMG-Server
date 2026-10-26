use super::*;
use crate::world::view::PlayerMarker;

fn fixture() -> (Content, PlayerView) {
    let (content, mut v) = quest_tests::fixture();
    v.map.name = "Nexus".into();
    v.session.world = -2;
    v.position = Pos { x: 78.5, y: 72.5 };
    v.objects.clear();
    v.quest = None;
    v.player_markers = [
        v.position,
        Pos { x: 79.5, y: 73.5 },
        Pos { x: 76.5, y: 72.5 },
    ]
    .into_iter()
    .enumerate()
    .map(|(i, position)| PlayerMarker {
        id: i as i32 + 20,
        account: i as i32 + 20,
        name: format!("Peer{i}"),
        position,
    })
    .collect();
    (content, v)
}

#[test]
fn initial_nexus_linger_walks_clear_of_observed_peers() {
    let (content, v) = fixture();
    for profile in [ARCHETYPES[0], ARCHETYPES[1]] {
        let mut brain = Brain::new(profile, &v);
        let d = brain.decide(&v, &content).unwrap();
        assert_eq!(brain.activity, Activity::Linger);
        let p = d
            .movement
            .expect("initial linger must address known player overlap");
        let spot = brain.nexus_spot.unwrap();
        assert!(spot.distance(v.position) <= 7.);
        assert!(
            v.player_markers
                .iter()
                .all(|m| spot.distance(m.position) >= 2.)
        );
        assert!(Grid::terrain(&v).segment(v.position, p));
        assert!(d.journey.is_none() && d.item.is_none());
    }
}

#[test]
fn initial_nexus_linger_holds_destination_and_stops_on_arrival() {
    let (content, mut v) = fixture();
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    let first = brain.decide(&v, &content).unwrap();
    let spot = brain.nexus_spot.unwrap();
    for _ in 0..3 {
        v.tick += 5;
        v.time_ms += 250;
        let d = brain.decide(&v, &content).unwrap();
        assert_eq!(brain.nexus_spot.unwrap().distance(spot), 0.);
        assert_eq!(d.movement.unwrap().distance(first.movement.unwrap()), 0.);
    }
    v.position = spot;
    v.tick += 5;
    let d = brain.decide(&v, &content).unwrap();
    assert!(d.movement.is_none());
    assert_eq!(d.goal, Goal::Linger);
    assert_eq!(brain.nexus_spot.unwrap().distance(spot), 0.);
    v.time_ms = brain.nexus_until;
    v.tick += 5;
    brain.decide(&v, &content).unwrap();
    assert!(brain.nexus_spot.is_none());
    assert_ne!(brain.activity, Activity::Linger);
}

fn known(v: &mut PlayerView, points: Vec<TilePoint>) {
    let geometry = crate::navigation::Geometry::new(
        (128, 128),
        vec![1; 128 * 128],
        &BTreeMap::from([(1, 1.)]),
    );
    v.tiles = Arc::new(crate::navigation::Tiles::new(geometry, points));
}

#[test]
fn initial_nexus_linger_ignores_own_marker_and_waits_when_no_room() {
    let (content, mut v) = fixture();
    known(&mut v, vec![(78, 72), (82, 72)]);
    v.player_markers.truncate(1);
    v.player_markers[0].id = v.session.player;
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    let d = brain.decide(&v, &content).unwrap();
    assert_eq!(d.goal, Goal::Linger);
    assert!(d.movement.is_none());
    assert_eq!(brain.nexus_spot.unwrap().distance(v.position), 0.);
    v.player_markers[0].id = 20;
    v.tick += 5;
    let d = brain.decide(&v, &content).unwrap();
    assert_eq!(d.goal, Goal::NexusNoRoom);
    assert!(d.movement.is_none() && d.journey.is_none());
    assert!(
        brain.nexus_spot.is_none(),
        "disconnected tile is not a reachable spot"
    );
    assert!(cctv::reason(brain.activity, &d, &v, &content).contains("no clear reachable"));
}

#[test]
fn initial_nexus_linger_revalidates_spot_and_cannot_cross_sessions() {
    let (content, mut v) = fixture();
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    brain.decide(&v, &content).unwrap();
    let old = brain.nexus_spot.unwrap();
    v.player_markers[0].position = old;
    v.tick += 5;
    brain.decide(&v, &content).unwrap();
    assert!(brain.nexus_spot.unwrap().distance(old) >= 2.);
    let blocked = brain.nexus_spot.unwrap();
    brain.blocked.insert(tile(blocked), v.tick + 100);
    v.tick += 5;
    brain.decide(&v, &content).unwrap();
    assert!(brain.nexus_spot.unwrap().distance(blocked) > 0.);
    v.session.world = -3;
    v.tick += 5;
    assert!(brain.decide(&v, &content).is_err());
    assert!(Brain::new(ARCHETYPES[0], &v).nexus_spot.is_none());
}

#[test]
fn initial_nexus_linger_yields_to_safety_invites_groups_and_native_trade() {
    let (content, mut v) = fixture();
    v.character.hp = 100;
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    assert_eq!(brain.decide(&v, &content).unwrap().goal, Goal::Recover);
    assert_eq!(brain.activity, Activity::Survive);
    assert!(brain.nexus_spot.is_none());
    v.character.hp = 500;
    let (_, mut hostile) = quest_tests::fixture();
    hostile.objects[0].position = Pos { x: 84.5, y: 72.5 };
    v.objects = hostile.objects;
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    assert!(brain.decide(&v, &content).unwrap().movement.is_none());
    assert!(brain.nexus_spot.is_none());
    v.objects.clear();
    v.character.mp = 0;
    v.character.items[4] = 0xadd;
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    let d = brain.decide(&v, &content).unwrap();
    assert!(d.item.is_some() && d.movement.is_none() && brain.nexus_spot.is_none());
    v.character.mp = 200;
    v.character.items[4] = -1;
    for busy in [true, false] {
        let mut brain = Brain::new(ARCHETYPES[0], &v);
        v.lifecycle.busy = busy;
        brain.proposing = !busy;
        let d = brain.decide(&v, &content).unwrap();
        assert_eq!(
            brain.activity,
            if busy {
                Activity::Busy
            } else {
                Activity::Social
            }
        );
        assert!(d.movement.is_none() && brain.nexus_spot.is_none());
    }
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    brain.group = Some(adventures::Adventure {
        members: vec![],
        purpose: adventures::Purpose::Experience,
        until: v.elapsed_ms + 60_000,
        finished: false,
    });
    assert!(brain.decide(&v, &content).unwrap().movement.is_none());
    assert!(brain.nexus_spot.is_none());
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    brain.agreement = Some(dialogue::Agreement {
        partner: "Peer".into(),
        player: 20,
        waiting: Some(v.position),
    });
    brain.invitation_until = v.time_ms + 60_000;
    assert_eq!(
        brain.decide(&v, &content).unwrap().goal,
        Goal::Gather(v.session.player)
    );
    assert!(brain.nexus_spot.is_none());
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    let mut w = wire::Writer::new();
    for (index, items) in [v.character.items.clone(), vec![-1; 12]].iter().enumerate() {
        if index == 1 {
            w.utf("Peer");
        }
        w.i16(12);
        for item in items {
            w.i32(*item).i32(0).u8(0).u8(0);
        }
    }
    brain
        .mind
        .market
        .receive(&Packet { id: 67, body: w.0 }, &v, &content, None)
        .unwrap();
    let d = brain.decide(&v, &content).unwrap();
    assert_eq!(brain.activity, Activity::Trade);
    assert!(d.movement.is_none() && brain.nexus_spot.is_none());
}
