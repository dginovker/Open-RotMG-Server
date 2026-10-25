use super::*;
use crate::world::view::{Lifecycle, MapInfo, OwnCharacter, PlayerMarker, Session, VisibleObject};
use rand::{RngCore, rngs::mock::StepRng};

fn fixture(account: i32) -> (Content, PlayerView, Brain) {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    let geometry =
        crate::navigation::Geometry::new((32, 32), vec![1; 1024], &BTreeMap::from([(1, 1.)]));
    let tiles =
        crate::navigation::Tiles::new(geometry, (0..32).flat_map(|x| (0..32).map(move |y| (x, y))));
    let stats = [500, 200, 35, 10, 25, 30, 30, 25];
    let v = PlayerView {
        session: Session {
            world: -2,
            player: 1,
            account,
            character: 1,
        },
        tick: 100,
        time_ms: 5000,
        elapsed_ms: 5000,
        clock_at: std::time::Instant::now(),
        position: Pos { x: 10.5, y: 10.5 },
        quest: None,
        map: MapInfo {
            name: "Nexus".into(),
            width: 32,
            height: 32,
            allow_teleport: true,
        },
        character: OwnCharacter {
            conditions: 0,
            class: crate::content::WIZARD,
            level: 20,
            experience: 0,
            hp: 500,
            mp: 200,
            stats,
            base_stats: stats,
            items: vec![
                0xa9e, 0xa2e, 0xa80, -1, 0xa22, 0xa22, -1, -1, -1, -1, -1, -1,
            ],
            inventory_revision: 0,
            pirate_caves: 0,
            snake_pits: 0,
            ability_ready_ms: 0,
        },
        lifecycle: Lifecycle {
            alive: true,
            death_pending: false,
            departing: false,
            busy: false,
            quarantined: false,
            output: OutputState::Live,
        },
        tiles: Arc::new(tiles),
        projectiles: vec![],
        dungeon_entries: vec![],
        objects: vec![VisibleObject {
            id: 40,
            account: Some(40),
            kind: crate::content::WIZARD,
            name: "Caller".into(),
            position: Pos { x: 10.5, y: 18.5 },
            hp: 500,
            max_hp: 500,
            level: Some(20),
            equipped: vec![0xa97, 0xa2e, -1, -1],
            contents: vec![],
            blocks: false,
            usable: true,
            condition_mask: None,
        }],
        player_markers: vec![PlayerMarker {
            id: 40,
            account: 40,
            name: "Caller".into(),
            position: Pos { x: 10.5, y: 18.5 },
        }],
    };
    let b = Brain::new(ARCHETYPES[5], &v);
    (c, v, b)
}
fn packet(actor: i32, name: &str, recipient: &str, text: &str) -> Packet {
    let mut w = wire::Writer::new();
    w.utf(name)
        .i32(actor)
        .i32(0)
        .u8(5)
        .utf(recipient)
        .utf(text)
        .utf("");
    Packet {
        id: wire::TEXT,
        body: w.0,
    }
}
fn pending(b: &mut Brain, v: &PlayerView) {
    b.calls.opportunity = Some(Opportunity {
        world: v.session.world,
        actor: 40,
        account: 40,
        name: "Caller".into(),
        activity: Activity::Help,
        until: v.elapsed_ms + 30_000,
    });
}

fn realm_fixture() -> (Content, PlayerView, Brain) {
    let (c, mut v, mut b) = fixture(4);
    v.map.name = "Realm".into();
    v.session.world = -3;
    b.session.0 = -3;
    (c, v, b)
}
fn red_demon(c: &Content, v: &mut PlayerView, distance: f32) {
    let definition = c.get(0x668).unwrap();
    v.objects.push(VisibleObject {
        id: 50,
        account: None,
        kind: 0x668,
        name: definition.name.clone(),
        position: Pos {
            x: v.position.x,
            y: v.position.y + distance,
        },
        hp: definition.hp,
        max_hp: definition.hp,
        level: None,
        equipped: vec![],
        contents: vec![],
        blocks: false,
        usable: false,
        condition_mask: Some(0),
    });
}

#[test]
fn public_call_distant_advertised_enemy_allows_receive_and_dispatch() {
    for visible_at_receipt in [true, false] {
        let (c, mut v, mut b) = realm_fixture();
        let projectile = &c.get(v.character.items[0] as u16).unwrap().projectiles[&0];
        let range = projectile.speed * projectile.lifetime / 10000.;
        assert!((range - 8.55).abs() < 0.01);
        assert!(v.projectiles.is_empty());
        assert_eq!(b.goal, Goal::Wait);
        if visible_at_receipt {
            red_demon(&c, &mut v, 18.);
        }
        let mut expected = b.rng.clone();
        assert!(expected.gen_bool(0.5));
        b.receive(packet(40, "Caller", "", "tp demon"), &v, false, &c)
            .unwrap();
        assert!(b.calls.opportunity.is_some());
        assert_eq!(b.rng.clone().next_u64(), expected.next_u64());
        if !visible_at_receipt {
            red_demon(&c, &mut v, 18.);
        }
        let enemy = v.objects.last().unwrap();
        assert!(enemy.position.distance(v.position) > range);
        assert!(progression::can_fight(&v, &c, enemy));
        let next = b.rng.clone().next_u64();
        assert_eq!(b.call_target(&v, &c).unwrap(), Some(40));
        assert_eq!(b.call_target(&v, &c).unwrap(), None);
        assert_eq!(b.rng.clone().next_u64(), next);
    }
}

#[test]
fn public_call_active_combat_vetoes_without_roll_or_redraw() {
    for distance in [6., 10.] {
        let (c, mut v, mut b) = realm_fixture();
        red_demon(&c, &mut v, distance);
        b.goal = Goal::Fight(50);
        let next = b.rng.clone().next_u64();
        b.receive(packet(40, "Caller", "", "tp demon"), &v, false, &c)
            .unwrap();
        assert!(b.calls.opportunity.is_none());
        assert_eq!(b.rng.clone().next_u64(), next);
        v.objects.pop();
        b.goal = Goal::Wait;
        b.receive(packet(40, "Caller", "", "tp demon"), &v, false, &c)
            .unwrap();
        assert!(b.calls.opportunity.is_none());
        assert_eq!(b.rng.clone().next_u64(), next);

        let (c, mut v, mut b) = realm_fixture();
        b.receive(packet(40, "Caller", "", "tp demon"), &v, false, &c)
            .unwrap();
        assert!(b.calls.opportunity.is_some());
        let next = b.rng.clone().next_u64();
        red_demon(&c, &mut v, distance);
        b.goal = Goal::Fight(50);
        assert_eq!(b.call_target(&v, &c).unwrap(), None);
        v.objects.pop();
        b.goal = Goal::Wait;
        assert_eq!(b.call_target(&v, &c).unwrap(), None);
        assert_eq!(b.rng.clone().next_u64(), next);
    }
}

#[test]
fn public_call_aliases_are_exact_and_activity_claims_not_coordinates() {
    for (text, activity) in [
        ("tp", Activity::Help),
        (" TP ", Activity::Help),
        ("tp gods", Activity::Gods),
        ("TP  DEMON", Activity::Demon),
        ("tp sphynix", Activity::Sphinx),
        ("tp sphinx", Activity::Sphinx),
    ] {
        assert_eq!(Activity::parse(text), Some(activity));
    }
    for text in [
        "/tp",
        "tp elsewhere",
        "tp demon 10 20",
        "tp sphinx pls",
        "help",
        "notp",
    ] {
        assert_eq!(Activity::parse(text), None);
    }
}

#[test]
fn public_call_rolls_once_and_deduplicates_accepted_and_declined_accounts() {
    let mut outcomes = BTreeSet::new();
    for account in 1..=32 {
        let (c, v, mut b) = fixture(account);
        let mut expected = b.rng.clone();
        let accepted = expected.gen_bool(0.5);
        assert!(
            !b.receive(packet(40, "Caller", "", "tp"), &v, false, &c)
                .unwrap()
        );
        assert_eq!(b.calls.opportunity.is_some(), accepted);
        assert_eq!(b.rng.clone().next_u64(), expected.next_u64());
        let next = b.rng.clone().next_u64();
        b.receive(packet(40, "Caller", "", " TP "), &v, false, &c)
            .unwrap();
        assert_eq!(b.calls.seen.len(), 1);
        assert_eq!(b.rng.clone().next_u64(), next);
        assert_eq!(b.call_target(&v, &c).unwrap(), accepted.then_some(40));
        assert_eq!(b.call_target(&v, &c).unwrap(), None);
        assert_eq!(b.rng.clone().next_u64(), next);
        outcomes.insert(accepted);
        eprintln!("prepared_native_account={account} public_help_acceptance={accepted}");
    }
    assert_eq!(outcomes, BTreeSet::from([false, true]));
}

#[test]
fn public_call_rejects_private_spoofed_world_and_ineligible_messages_without_rng() {
    let (c, mut v, mut b) = fixture(3);
    let next = b.rng.clone().next_u64();
    for (actor, name, recipient) in [
        (40, "Caller", "Pixl"),
        (40, "Caller", "*Guild*"),
        (-1, "Caller", ""),
        (1, "Caller", ""),
        (40, "Spoof", ""),
    ] {
        b.receive(packet(actor, name, recipient, "tp"), &v, false, &c)
            .unwrap();
    }
    v.session.world = -3;
    b.receive(packet(40, "Caller", "", "tp"), &v, false, &c)
        .unwrap();
    v.session.world = -2;
    v.character.hp = 100;
    b.receive(packet(40, "Caller", "", "tp"), &v, false, &c)
        .unwrap();
    v.character.hp = 500;
    b.receive(packet(40, "Caller", "", "tp"), &v, false, &c)
        .unwrap();
    assert!(b.calls.opportunity.is_none());
    assert_eq!(b.calls.seen.len(), 1);
    assert_eq!(b.rng.clone().next_u64(), next);
    v.elapsed_ms += 60_000;
    b.receive(packet(40, "Caller", "", "tp"), &v, false, &c)
        .unwrap();
    assert_eq!(b.calls.seen.len(), 1);
    assert_ne!(b.rng.clone().next_u64(), next);
}

#[test]
fn public_call_activity_readiness_and_sphinx_alias_dedup_reuse_existing_policy() {
    let (c, mut v, mut b) = fixture(3);
    assert!(b.call_ready(&v, &c, Activity::Help, 40).unwrap());
    assert!(!b.call_ready(&v, &c, Activity::Gods, 40).unwrap());
    b.profile = ARCHETYPES[0];
    assert!(!b.call_ready(&v, &c, Activity::Help, 40).unwrap());
    b.profile = ARCHETYPES[5];
    v.map.name = "Realm".into();
    for activity in [Activity::Gods, Activity::Demon, Activity::Sphinx] {
        assert!(b.call_ready(&v, &c, activity, 40).unwrap());
    }
    v.character.level = 13;
    assert!(!b.call_ready(&v, &c, Activity::Gods, 40).unwrap());
    v.character.level = 19;
    assert!(!b.call_ready(&v, &c, Activity::Sphinx, 40).unwrap());
    v.character.level = 20;
    b.receive(packet(40, "Caller", "", "tp sphynix"), &v, false, &c)
        .unwrap();
    let next = b.rng.clone().next_u64();
    b.receive(packet(40, "Caller", "", "tp sphinx"), &v, false, &c)
        .unwrap();
    assert_eq!(b.rng.clone().next_u64(), next);
    assert_eq!(b.calls.seen.len(), 1);
}

#[test]
fn public_call_opportunity_expires_binds_identity_and_preserves_local_priorities() {
    let (c, mut v, mut b) = fixture(3);
    pending(&mut b, &v);
    v.elapsed_ms += 30_000;
    assert_eq!(b.call_target(&v, &c).unwrap(), None);
    pending(&mut b, &v);
    v.player_markers[0].account = 41;
    assert_eq!(b.call_target(&v, &c).unwrap(), None);
    v.player_markers[0].account = 40;
    pending(&mut b, &v);
    v.objects[0].hp = 0;
    assert_eq!(b.call_target(&v, &c).unwrap(), None);
    v.objects[0].hp = 500;
    pending(&mut b, &v);
    b.next_teleport = v.time_ms + 1;
    assert_eq!(b.call_target(&v, &c).unwrap(), None);
    b.next_teleport = 0;
    pending(&mut b, &v);
    b.companion = Some(20);
    assert_eq!(b.call_target(&v, &c).unwrap(), None);
    b.companion = None;
    pending(&mut b, &v);
    v.objects[0].position = v.position;
    v.player_markers[0].position = v.position;
    assert_eq!(b.call_target(&v, &c).unwrap(), None);
    v.objects[0].position = Pos { x: 10.5, y: 18.5 };
    v.player_markers[0].position = v.objects[0].position;
    pending(&mut b, &v);
    b.nexus_until = 0;
    v.objects.push(VisibleObject {
        id: 50,
        account: None,
        kind: 0x500,
        name: String::new(),
        position: v.position,
        hp: 1,
        max_hp: 1,
        level: None,
        equipped: vec![],
        contents: vec![0xa9f],
        blocks: false,
        usable: false,
        condition_mask: None,
    });
    let d = b.decide(&v, &c).unwrap();
    assert!(matches!(d.goal, Goal::Loot(50)));
    assert!(!matches!(d.journey, Some(Journey::Regroup(_))));
    v.objects.pop();
    v.tick += 1;
    pending(&mut b, &v);
    v.character.items[6] = 0xa1d;
    let grid = b.navigation_grid(&v, &c, &[]);
    // Spare stock only makes a visit eligible; this case needs an admitted commitment.
    let admitted = b
        .mind
        .market
        .decide(&v, &c, b.mind.future_class, &grid, &mut StepRng::new(0, 1))
        .unwrap()
        .expect("fixture must admit an autonomous market session");
    assert_eq!(admitted.goal, Goal::Wait);
    let d = b.decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Wait);
    assert!(d.journey.is_none());
    assert!(b.calls.opportunity.is_some());
}

#[test]
fn public_call_declined_market_visit_allows_calls_but_not_over_player_agreements() {
    let (c, mut v, mut b) = fixture(3);
    b.nexus_until = 0;
    v.character.items[6] = 0xa1d;
    let grid = b.navigation_grid(&v, &c, &[]);
    assert!(
        b.mind
            .market
            .decide(
                &v,
                &c,
                b.mind.future_class,
                &grid,
                &mut StepRng::new(u64::MAX, 1)
            )
            .unwrap()
            .is_none()
    );
    // Even an accepting draw cannot turn this declined visit into a commitment.
    assert!(
        b.mind
            .market
            .decide(&v, &c, b.mind.future_class, &grid, &mut StepRng::new(0, 1))
            .unwrap()
            .is_none()
    );
    pending(&mut b, &v);
    let d = b.decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Follow(40));
    assert!(matches!(d.journey, Some(Journey::Regroup(40))));
    assert!(b.calls.opportunity.is_none());
    b.next_teleport = 0;
    for waiting in [None, Some(v.position)] {
        b.agreement = Some(dialogue::Agreement {
            partner: "Caller".into(),
            player: 40,
            waiting,
        });
        assert!(!b.call_ready(&v, &c, Activity::Help, 40).unwrap());
        pending(&mut b, &v);
        assert_eq!(b.call_target(&v, &c).unwrap(), None);
    }
}

#[test]
fn public_call_dedup_window_is_bounded_without_evicting_live_refusals() {
    let (c, mut v, mut b) = fixture(3);
    v.character.hp = 100;
    for account in 40..72 {
        v.objects[0].account = Some(account);
        b.receive(packet(40, "Caller", "", "tp"), &v, false, &c)
            .unwrap();
    }
    assert_eq!(b.calls.seen.len(), 32);
    v.objects[0].account = Some(99);
    v.character.hp = 500;
    let next = b.rng.clone().next_u64();
    b.receive(packet(40, "Caller", "", "tp"), &v, false, &c)
        .unwrap();
    assert_eq!(b.calls.seen.len(), 32);
    assert!(b.calls.opportunity.is_none());
    assert_eq!(b.rng.clone().next_u64(), next);
    v.elapsed_ms += 60_000;
    b.receive(packet(40, "Caller", "", "tp"), &v, false, &c)
        .unwrap();
    assert_eq!(b.calls.seen.len(), 1);
}

#[tokio::test]
async fn public_call_ordinary_teleport_keeps_target_health_lifecycle_world_known_and_cooldown_guards()
 {
    use crate::{store::Store, world::Directory};
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let store = Store::open(std::path::Path::new(":memory:"), &content).unwrap();
    let mut d =
        Directory::test_with_seed(std::path::Path::new("data"), content.clone(), 8, 12351).unwrap();
    let mut ids = Vec::new();
    for (guid, world) in [
        ("call-guard-a", -2),
        ("call-guard-b", -2),
        ("call-other-world", -3),
    ] {
        let account = store.list(guid.into()).await.unwrap();
        let ch = store
            .create(account.id, content.get(crate::content::WIZARD).unwrap())
            .await
            .unwrap();
        ids.push(d.worlds.get_mut(&world).unwrap().enter(ch, None).unwrap());
    }
    let [a, b, other] = ids.as_slice() else {
        panic!("explicit three-player fixture")
    };
    d.worlds.get_mut(&-2).unwrap().updates(*a, true).unwrap();
    let now = std::time::Instant::now();
    assert!(d.teleport(*a, *other, now).is_err());
    d.worlds
        .get_mut(&-2)
        .unwrap()
        .players
        .get_mut(b)
        .unwrap()
        .character
        .hp = 0;
    assert!(d.teleport(*a, *b, now).is_err());
    d.worlds
        .get_mut(&-2)
        .unwrap()
        .players
        .get_mut(b)
        .unwrap()
        .character
        .hp = 100;
    d.worlds
        .get_mut(&-2)
        .unwrap()
        .players
        .get_mut(b)
        .unwrap()
        .pending = true;
    assert!(d.teleport(*a, *b, now).is_err());
    d.worlds
        .get_mut(&-2)
        .unwrap()
        .players
        .get_mut(b)
        .unwrap()
        .pending = false;
    d.worlds
        .get_mut(&-2)
        .unwrap()
        .players
        .get_mut(a)
        .unwrap()
        .known
        .remove(b);
    assert!(d.teleport(*a, *b, now).is_err());
    d.worlds.get_mut(&-2).unwrap().updates(*a, true).unwrap();
    let receipt = d.teleport(*a, *b, now).unwrap();
    assert_eq!(receipt.target, *b);
    assert_eq!(receipt.world, -2);
    assert!(
        d.teleport(*a, *b, now + std::time::Duration::from_secs(1))
            .is_err()
    );
    assert!(
        d.teleport(*a, *b, now + std::time::Duration::from_secs(10))
            .is_ok()
    );
}
