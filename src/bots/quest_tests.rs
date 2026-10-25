use super::*;
use crate::world::view::{Lifecycle, MapInfo, OwnCharacter, Session, VisibleObject};

pub(super) fn fixture() -> (Content, PlayerView) {
    let content = Content::load(std::path::Path::new("data")).unwrap();
    let geometry = crate::navigation::Geometry::new(
        (128, 128),
        vec![1; 128 * 128],
        &BTreeMap::from([(1, 1.)]),
    );
    let tiles = crate::navigation::Tiles::new(
        geometry,
        (0..128).flat_map(|x| (0..128).map(move |y| (x, y))),
    );
    let stats = [500, 200, 35, 10, 25, 30, 30, 25];
    let view = PlayerView {
        session: Session {
            world: -3,
            player: 1,
            account: 1,
            character: 1,
        },
        tick: 100,
        time_ms: 5000,
        clock_at: std::time::Instant::now(),
        elapsed_ms: 5000,
        position: Pos { x: 10.5, y: 10.5 },
        quest: Some(2),
        map: MapInfo {
            name: "Realm".into(),
            width: 128,
            height: 128,
            allow_teleport: true,
        },
        character: OwnCharacter {
            conditions: 0,
            class: crate::content::WIZARD,
            level: 14,
            experience: 20000,
            hp: stats[0],
            mp: stats[1],
            stats,
            base_stats: stats,
            items: vec![0xa9e, 0xa2e, 0xa80, -1, -1, -1, -1, -1, -1, -1, -1, -1],
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
        player_markers: vec![],
        projectiles: vec![],
        dungeon_entries: vec![],
        objects: vec![VisibleObject {
            id: 2,
            account: None,
            kind: 0x678,
            name: "Oasis Giant".into(),
            position: Pos { x: 10.5, y: 90.5 },
            hp: 650,
            max_hp: 650,
            level: None,
            equipped: vec![],
            contents: vec![],
            blocks: false,
            usable: false,
            condition_mask: Some(0),
        }],
    };
    (content, view)
}

#[test]
fn published_realm_quest_displaces_retained_roaming() {
    let (content, mut view) = fixture();
    for profile in ARCHETYPES {
        let mut brain = Brain::new(profile, &view);
        brain.goal = Goal::Venture;
        assert!(progression::ready(&view, &content, 0x718));
        let decision = brain.decide(&view, &content).unwrap();
        assert_eq!(decision.goal, Goal::Hunt(2), "{}", profile.name);
        let movement = decision.movement.expect("travel toward published quest");
        assert!(movement.y > view.position.y && (movement.x - view.position.x).abs() < 0.1);
    }
    view.objects[0].position.y = 30.5;
    let mut brain = Brain::new(ARCHETYPES[0], &view);
    brain.goal = Goal::Venture;
    assert_eq!(brain.decide(&view, &content).unwrap().goal, Goal::Hunt(2));
}

#[test]
fn quest_still_yields_to_unsuitability_recovery_and_supplies() {
    let (content, mut view) = fixture();
    view.objects[0].kind = 0x91f;
    view.objects[0].max_hp = 300000;
    assert!(!progression::can_fight(&view, &content, &view.objects[0]));
    assert_ne!(
        Brain::new(ARCHETYPES[0], &view)
            .decide(&view, &content)
            .unwrap()
            .goal,
        Goal::Hunt(2)
    );
    view.objects[0].kind = 0x678;
    view.objects[0].max_hp = 650;
    view.objects[0].condition_mask = Some(crate::condition::bit(crate::condition::INVULNERABLE));
    assert_ne!(
        Brain::new(ARCHETYPES[0], &view)
            .decide(&view, &content)
            .unwrap()
            .goal,
        Goal::Hunt(2)
    );
    view.objects[0].condition_mask = Some(0);
    let mut brain = Brain::new(ARCHETYPES[0], &view);
    brain.goal = Goal::Hunt(2);
    view.character.hp = 100;
    let recovery = brain.decide(&view, &content).unwrap();
    assert_eq!(recovery.goal, Goal::Recover);
    assert!(matches!(recovery.journey, Some(Journey::Nexus)));
    brain.goal = recovery.goal;
    view.character.hp = 500;
    view.tick += 61;
    view.time_ms += 3050;
    assert_eq!(brain.decide(&view, &content).unwrap().goal, Goal::Hunt(2));
    view.objects.push(VisibleObject {
        id: 3,
        account: None,
        kind: 0x500,
        name: String::new(),
        position: view.position,
        hp: 1,
        max_hp: 1,
        level: None,
        equipped: vec![],
        contents: vec![0xa22],
        blocks: false,
        usable: false,
        condition_mask: None,
    });
    assert_eq!(
        Brain::new(ARCHETYPES[0], &view)
            .decide(&view, &content)
            .unwrap()
            .goal,
        Goal::Loot(3)
    );
}

fn recovery_scene(hp: i32) -> (Content, PlayerView) {
    let (content, mut view) = fixture();
    view.character.hp = hp;
    view.character.items[1] = -1;
    view.objects[0].position = Pos { x: 16.5, y: 10.5 };
    (content, view)
}

#[test]
fn noncritical_recovery_steps_back_or_rests_locally() {
    let (content, mut view) = recovery_scene(250);
    let decision = Brain::new(ARCHETYPES[0], &view)
        .decide(&view, &content)
        .unwrap();
    assert_eq!(decision.goal, Goal::Recover);
    assert!(decision.journey.is_none());
    let step = decision.movement.expect("available backstep");
    assert!(
        step.distance(view.objects[0].position) > view.position.distance(view.objects[0].position)
    );
    view.objects.clear();
    view.quest = None;
    let quiet = Brain::new(ARCHETYPES[0], &view)
        .decide(&view, &content)
        .unwrap();
    assert_eq!(quiet.goal, Goal::Recover);
    assert!(quiet.journey.is_none() && quiet.movement.is_none());
}

#[test]
fn recovery_emergency_boundary_retains_personality() {
    let (content, mut view) = recovery_scene(125);
    let cautious = Brain::new(ARCHETYPES[0], &view)
        .decide(&view, &content)
        .unwrap();
    assert!(matches!(cautious.journey, Some(Journey::Nexus)));
    let tolerant = Brain::new(ARCHETYPES[1], &view)
        .decide(&view, &content)
        .unwrap();
    assert_eq!(tolerant.goal, Goal::Recover);
    assert!(tolerant.journey.is_none() && tolerant.movement.is_some());
    view.character.hp = 50;
    assert!(matches!(
        Brain::new(ARCHETYPES[1], &view)
            .decide(&view, &content)
            .unwrap()
            .journey,
        Some(Journey::Nexus)
    ));
}

#[test]
fn recovery_requires_executable_clear_backstep() {
    let (content, mut view) = recovery_scene(250);
    let exits = |view: &PlayerView| {
        matches!(
            Brain::new(ARCHETYPES[0], view)
                .decide(view, &content)
                .unwrap()
                .journey,
            Some(Journey::Nexus)
        )
    };
    view.character.conditions = crate::content::PARALYZED;
    assert!(exits(&view));
    view.character.conditions = 0;
    let known = view.tiles.clone();
    let geometry = crate::navigation::Geometry::new(
        (128, 128),
        vec![1; 128 * 128],
        &BTreeMap::from([(1, 1.)]),
    );
    view.tiles = Arc::new(crate::navigation::Tiles::new(
        geometry,
        [tile(view.position)],
    ));
    assert!(exits(&view));
    view.tiles = known;
    view.projectiles
        .push(crate::world::view::VisibleProjectile {
            owner: 2,
            id: 0,
            start: view.objects[0].position,
            position: view.position,
            angle: std::f32::consts::PI,
            speed: 60.,
            lifetime_ms: 2400.,
            amplitude: 0.,
            frequency: 0.,
            passes_cover: false,
        });
    assert!(exits(&view));
}

#[test]
fn recovery_preserves_healing_and_close_outmatched_precedence() {
    let (content, mut view) = recovery_scene(50);
    view.character.items[4] = 0xa22;
    let healing = Brain::new(ARCHETYPES[0], &view)
        .decide(&view, &content)
        .unwrap();
    assert!(healing.journey.is_none());
    assert!(matches!(healing.item, Some(Action::Use { slot: 4, .. })));
    view.objects[0].kind = 0x91f;
    view.objects[0].max_hp = 300000;
    view.objects[0].position.x = 13.5;
    let outmatched = Brain::new(ARCHETYPES[0], &view)
        .decide(&view, &content)
        .unwrap();
    assert!(matches!(outmatched.journey, Some(Journey::Nexus)));
    view.character.items[4] = -1;
    view.character.class = crate::content::PRIEST;
    view.character.items[0] = 0xa3d;
    view.character.items[1] = 0xa31;
    let pending = Brain::new(ARCHETYPES[0], &view)
        .decide(&view, &content)
        .unwrap();
    assert!(pending.journey.is_none());
    assert!(matches!(pending.item, Some(Action::Use { slot: 1, .. })));
}

#[test]
fn recovery_rejects_centerline_clear_but_blocked_endpoint_footprint() {
    let (content, mut view) = recovery_scene(250);
    let mut ground = vec![1; 128 * 128];
    ground[10 * 128 + 8] = 255;
    let geometry =
        crate::navigation::Geometry::new((128, 128), ground, &BTreeMap::from([(1, 1.), (255, 0.)]));
    view.tiles = Arc::new(crate::navigation::Tiles::new(
        geometry.clone(),
        (0..128).flat_map(|x| (0..128).map(move |y| (x, y))),
    ));
    let grid = Grid::terrain(&view);
    let step = Pos { x: 9.25, y: 10.5 };
    assert!(grid.segment(view.position, step));
    assert!(!recovery_step_valid(&view, &content, &grid, step));
    assert!(recovery_step_valid(
        &view,
        &content,
        &grid,
        Pos { x: 9.5, y: 10.5 }
    ));
    let decision = Brain::new(ARCHETYPES[0], &view)
        .decide(&view, &content)
        .unwrap();
    assert!(matches!(decision.journey, Some(Journey::Nexus)));
    assert!(decision.movement.is_some_and(|p| p.distance(step) < 0.001));
    view.tiles = Arc::new(crate::navigation::Tiles::new(
        geometry,
        (0..128)
            .flat_map(|x| (0..128).map(move |y| (x, y)))
            .filter(|p| *p != (8, 10)),
    ));
    assert!(!recovery_step_valid(
        &view,
        &content,
        &Grid::terrain(&view),
        step
    ));
    assert!(matches!(
        Brain::new(ARCHETYPES[0], &view)
            .decide(&view, &content)
            .unwrap()
            .journey,
        Some(Journey::Nexus)
    ));
}

#[test]
fn recovery_footprint_preserves_half_cell_and_object_boundaries() {
    let (content, mut view) = recovery_scene(250);
    let grid = Grid::terrain(&view);
    let step = Pos { x: 9.25, y: 10.5 };
    assert!(recovery_step_valid(&view, &content, &grid, step));
    assert!(!recovery_step_valid(
        &view,
        &content,
        &grid,
        Pos { x: 0.25, y: 10.5 }
    ));
    assert!(recovery_step_valid(
        &view,
        &content,
        &grid,
        Pos { x: 0.5, y: 10.5 }
    ));
    assert!(!recovery_step_valid(
        &view,
        &content,
        &grid,
        Pos {
            x: f32::NAN,
            y: 10.5
        }
    ));
    view.objects[0].kind = content
        .objects
        .values()
        .find(|o| o.full_occupy)
        .unwrap()
        .kind;
    view.objects[0].position = Pos { x: 8.5, y: 10.5 };
    assert!(!recovery_step_valid(&view, &content, &grid, step));
    view.objects[0].kind = content
        .objects
        .values()
        .find(|o| o.blocks && !o.full_occupy)
        .unwrap()
        .kind;
    assert!(recovery_step_valid(&view, &content, &grid, step));
    view.objects[0].position = step;
    assert!(!recovery_step_valid(&view, &content, &grid, step));
}

fn familiar_social_scene(world: &str) -> (Content, PlayerView, Brain) {
    let (content, mut v) = fixture();
    v.map.name = world.into();
    v.session.world = if world == "Nexus" { -2 } else { -3 };
    v.character.level = 5;
    v.character.items[4..6].fill(0xa22);
    v.quest = None;
    let peer = &mut v.objects[0];
    peer.id = 2;
    peer.account = Some(2);
    peer.kind = crate::content::PRIEST;
    peer.name = "Peer".into();
    peer.position = Pos { x: 2.5, y: 10.5 };
    peer.hp = 500;
    peer.max_hp = 500;
    peer.level = Some(20);
    peer.equipped = vec![0xa3d, 0xa31, -1, -1];
    if world == "Nexus" {
        v.objects.push(VisibleObject {
            id: 3,
            account: None,
            kind: 0x712,
            name: "Test (0/85)".into(),
            position: Pos { x: 20.5, y: 10.5 },
            hp: 0,
            max_hp: 0,
            level: None,
            equipped: vec![],
            contents: vec![],
            blocks: false,
            usable: true,
            condition_mask: None,
        });
    }
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    brain.mind.market.name = brain.profile.name.into();
    brain.mind.social.episodes.push_back(
        serde_json::from_value(serde_json::json!({
            "account":2,"name":"Peer","own_character":v.session.character,
            "interaction":"HealedBy","amount":20
        }))
        .unwrap(),
    );
    v.tick += 400;
    v.time_ms += 20_000;
    v.elapsed_ms += 20_000;
    (content, v, brain)
}

#[test]
fn nexus_familiar_peer_does_not_beat_open_realm_entry() {
    let (c, v, mut brain) = familiar_social_scene("Nexus");
    brain.goal = Goal::Follow(2);
    let d = brain.decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Portal(3));
    assert!(d.movement.unwrap().x > v.position.x);
}

#[test]
fn nexus_exclusion_preserves_realm_follow_and_explicit_agreements() {
    let (c, v, mut brain) = familiar_social_scene("Realm");
    assert_eq!(brain.decide(&v, &c).unwrap().goal, Goal::Follow(2));
    for waiting in [None, Some(Pos { x: 10.5, y: 10.5 })] {
        let (c, v, mut brain) = familiar_social_scene("Nexus");
        brain.agreement = Some(dialogue::Agreement {
            partner: "Peer".into(),
            player: 2,
            waiting,
        });
        brain.invitation_until = v.time_ms + 120_000;
        let d = brain.decide(&v, &c).unwrap();
        assert_eq!(
            d.goal,
            if waiting.is_some() {
                Goal::Gather(v.session.player)
            } else {
                Goal::Follow(2)
            }
        );
        assert!(d.journey.is_none());
    }
}

#[test]
fn nexus_exclusion_preserves_active_business_and_admitted_market() {
    let (c, v, mut brain) = familiar_social_scene("Nexus");
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
        .receive(&Packet { id: 67, body: w.0 }, &v, &c, None)
        .unwrap();
    let d = brain.decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Wait);
    assert!(d.journey.is_none());

    let (c, mut v, mut brain) = familiar_social_scene("Nexus");
    v.character.items[4] = 0xa1d;
    v.character.items[5] = 0xa1d;
    v.character.items[6..8].fill(0xa22);
    let grid = Grid::terrain(&v);
    assert!(
        brain
            .mind
            .market
            .decide(
                &v,
                &c,
                None,
                &grid,
                &mut rand::rngs::mock::StepRng::new(0, 1)
            )
            .unwrap()
            .is_some()
    );
    let d = brain.decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Wait);
    assert!(d.journey.is_none());

    let (c, mut v, mut brain) = familiar_social_scene("Nexus");
    v.character.items[6] = 0xa22;
    let grid = Grid::terrain(&v);
    brain
        .mind
        .market
        .decide(
            &v,
            &c,
            None,
            &grid,
            &mut rand::rngs::mock::StepRng::new(u64::MAX, 1),
        )
        .unwrap();
    let mut w = wire::Writer::new();
    w.utf("Peer")
        .i32(2)
        .i32(0)
        .u8(0)
        .utf("")
        .utf("Triceps, spare health?")
        .utf("");
    brain
        .mind
        .social
        .receive(
            &Packet {
                id: wire::TEXT,
                body: w.0,
            },
            &v,
            "Triceps",
            3.,
            brain.mind.market.incoming.is_some(),
        )
        .unwrap();
    let d = brain.decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Wait);
    assert!(d.journey.is_none());
}

#[test]
fn nexus_familiarity_does_not_preempt_banking() {
    let (c, mut v, mut brain) = familiar_social_scene("Nexus");
    v.character.items[4] = 0xa1d;
    v.character.items[5] = 0xa1d;
    v.character.items[6..8].fill(0xa22);
    v.objects[1].kind = 0x720;
    v.objects[1].name = "Vault".into();
    brain
        .mind
        .market
        .decide(
            &v,
            &c,
            None,
            &Grid::terrain(&v),
            &mut rand::rngs::mock::StepRng::new(u64::MAX, 1),
        )
        .unwrap();
    let d = brain.decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Portal(3));
}

#[test]
fn market_reserved_stock_yields_to_reflexes_without_inventing_settlement() {
    use pursuits::market::{Bargain, Goods, Terms};
    let (content, mut view) = fixture();
    view.character.hp = 30;
    view.character.items[4] = 0xa21;
    let mut brain = Brain::new(ARCHETYPES[0], &view);
    brain.mind.market.bargain = Some(Bargain {
        terms: Terms {
            version: 1,
            accounts: [1, 2],
            names: ["Me".into(), "Peer".into()],
            goods: [
                Goods {
                    kind: 0xa21,
                    quantity: 1,
                },
                Goods {
                    kind: 0xa20,
                    quantity: 1,
                },
            ],
            meeting: [-2, 10, 10],
            expires: pursuits::market::now() + 120,
            batch: 1,
        },
        own_consent: true,
        agreed: true,
        peer_consent: Some([-2, 2, 1]),
        confirmed: 0,
        consent_revision: 0,
        rounds: 1,
        resolution: None,
    });
    let d = brain.decide(&view, &content).unwrap();
    assert_eq!(d.goal, Goal::Recover);
    assert!(matches!(d.journey, Some(Journey::Nexus)));
    assert!(d.item.is_none());
    assert_eq!(brain.mind.market.bargain.as_ref().unwrap().confirmed, 0);
    assert!(
        brain
            .mind
            .market
            .bargain
            .as_ref()
            .unwrap()
            .resolution
            .is_none()
    );
    view.character.hp = view.character.stats[0];
    view.tick += 100;
    view.time_ms += 5000;
    let d = brain.decide(&view, &content).unwrap();
    assert!(
        d.item.is_none(),
        "ordinary consumption/equipment cannot take the reserved Speed"
    );
}

#[test]
fn pirate_cave_bots_leave_through_the_realm_portal_exit() {
    let (content, mut view) = fixture();
    view.map.name = "Pirate Cave".into();
    view.quest = None;
    view.objects[0] = VisibleObject {
        id: 3,
        account: None,
        kind: 0x704,
        name: "Realm Portal".into(),
        position: Pos { x: 12.5, y: 10.5 },
        hp: 0,
        max_hp: 0,
        level: None,
        equipped: vec![],
        contents: vec![],
        blocks: false,
        usable: true,
        condition_mask: None,
    };
    let goal = |view: &PlayerView| {
        Brain::new(ARCHETYPES[0], view)
            .decide(view, &content)
            .unwrap()
            .goal
    };
    assert_eq!(goal(&view), Goal::Portal(3));
    view.objects[0].kind = 0x71d;
    assert_ne!(goal(&view), Goal::Portal(3));
}
