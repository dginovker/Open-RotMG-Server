use super::*;
use crate::world::view::{Lifecycle, MapInfo, OwnCharacter, PlayerMarker, Session, VisibleObject};

fn adventure(brain: &mut Brain, v: &PlayerView, c: &Content) -> Decision {
    brain
        .mind
        .intentions
        .as_mut()
        .unwrap()
        .observe(v, c, brain.profile.identity)
        .unwrap();
    let enemies: Vec<_> = v
        .objects
        .iter()
        .filter(|o| c.objects[&o.kind].enemy)
        .collect();
    let s = Scene {
        nearest: enemies.last().copied(),
        quarry: enemies.last().copied(),
        enemies,
        perception: combat::Perception::of(&brain.mind.cognition),
        inspecting: false,
        range: 6.,
        passes_cover: false,
        health: v.character.hp as f32 / v.character.stats[0] as f32,
        threatened: false,
        hit: 0,
        retreat: false,
        outmatched: false,
        grid: Arc::new(Grid::terrain(v)),
        remedy: None,
    };
    brain
        .adventure(
            v,
            c,
            &s,
            Decision {
                goal: Goal::Wait,
                movement: None,
                aim: None,
                item: None,
                journey: None,
                chat: None,
            },
        )
        .unwrap()
}

#[test]
fn geared_rushers_pass_trash_but_inexperienced_or_unprepared_bots_clear() {
    let (c, mut v) = fixture();
    v.map.name = "Snake Pit".into();
    v.map.template = Some("snake-pit".into());
    v.character.level = 20;
    v.character.stats[3] = 25;
    v.objects[0].kind = 0x917;
    v.objects[0].condition_mask = Some(crate::condition::bit(crate::condition::INVULNERABLE));
    v.objects.push(VisibleObject {
        id: 3,
        account: None,
        kind: 0x225,
        name: String::new(),
        position: Pos { x: 12.5, y: 10.5 },
        hp: c.objects[&0x225].hp,
        max_hp: c.objects[&0x225].hp,
        level: None,
        equipped: vec![],
        contents: vec![],
        blocks: false,
        usable: false,
        condition_mask: Some(0),
    });
    let mut brain = Brain::new(ARCHETYPES[1], &v);
    brain.goal = Goal::Fight(3);
    assert_eq!(adventure(&mut brain, &v, &c).goal, Goal::Hunt(2));
    brain.mind.cognition.directions = 4;
    assert_eq!(adventure(&mut brain, &v, &c).goal, Goal::Fight(3));
    brain.mind.cognition.directions = 16;
    v.character.stats[3] = 10;
    assert_eq!(adventure(&mut brain, &v, &c).goal, Goal::Fight(3));
    v.character.stats[3] = 25;
    v.character.level = 14;
    assert_eq!(adventure(&mut brain, &v, &c).goal, Goal::Fight(3));
    v.character.level = 20;
    v.character.hp = 300;
    assert_eq!(adventure(&mut brain, &v, &c).goal, Goal::Fight(3));
    v.character.hp = 500;
    v.map.name = "Realm".into();
    v.map.template = Some("realm".into());
    assert_eq!(adventure(&mut brain, &v, &c).goal, Goal::Fight(3));
    v.map.name = "Undead Lair".into();
    v.map.template = Some("undead-lair".into());
    v.objects[0].kind = 0x678;
    v.objects[0].condition_mask = Some(0);
    let rushing = adventure(&mut brain, &v, &c);
    assert_eq!(rushing.goal, Goal::Hunt(2));
    assert!(
        rushing.aim.is_some(),
        "shoot nearby trash while moving to the quest"
    );
    v.objects[0].position = Pos { x: 10.5, y: 14.5 };
    assert_eq!(adventure(&mut brain, &v, &c).goal, Goal::Fight(2));
    v.map.name = "Snake Pit".into();
    v.map.template = Some("snake-pit".into());
    v.objects[0].position = Pos { x: 10.5, y: 90.5 };
    v.objects[1].kind = 0x223;
    v.character.level = 14;
    v.character.stats[3] = 10;
    brain.mind.cognition.directions = 4;
    assert_eq!(adventure(&mut brain, &v, &c).goal, Goal::Hunt(2));
    v.character.hp = 100;
    let mut hurt = Brain::new(ARCHETYPES[1], &v);
    assert_eq!(hurt.decide(&v, &c).unwrap().goal, Goal::Recover);
}

#[test]
fn portal_callers_wait_instead_of_entering_on_the_calling_decision() {
    let (c, mut v) = fixture();
    v.objects[0].kind = 0x718;
    v.objects[0].usable = true;
    v.objects[0].position = v.position;
    v.objects[0].hp = 0;
    v.quest = None;
    let mut callers = 0;
    let mut silent = 0;
    for seed in 1..=32 {
        v.session.account = seed;
        let mut brain = Brain::new(ARCHETYPES[1], &v);
        let first = adventure(&mut brain, &v, &c);
        assert_eq!(first.goal, Goal::Portal(2));
        if first.chat.is_some() {
            callers += 1;
            assert!(first.journey.is_none());
            assert_eq!(brain.portal, Some((2, u64::MAX)));
            brain.goal = first.goal;
            assert!(adventure(&mut brain, &v, &c).journey.is_none());
            brain.portal = Some((2, v.time_ms + 4000));
            v.time_ms += 3999;
            assert!(adventure(&mut brain, &v, &c).journey.is_none());
            v.time_ms += 1;
            assert!(matches!(
                adventure(&mut brain, &v, &c).journey,
                Some(Journey::Portal(2))
            ));
        } else {
            silent += 1;
            assert!(matches!(first.journey, Some(Journey::Portal(2))));
        }
    }
    assert!(callers > 0 && silent > 0);
}

#[test]
fn nexus_realm_portal_approach_survives_a_detour_out_of_sight() {
    let (c, mut v) = fixture();
    v.map.name = "Nexus".into();
    v.map.template = Some("nexus".into());
    v.position = center((21, 24));
    v.quest = None;
    let portal = &mut v.objects[0];
    portal.kind = 0x712;
    portal.name = "Medusa (20/85)".into();
    portal.position = center((40, 24));
    portal.usable = true;
    let mut ground = vec![255; 128 * 128];
    for (x, y) in
        (20..=40)
            .map(|x| (x, 22))
            .chain([(20, 23), (20, 24), (21, 24), (40, 23), (40, 24)])
    {
        ground[y * 128 + x] = 1;
    }
    v.tiles = Arc::new(crate::navigation::Tiles::new(
        crate::navigation::Geometry::new((128, 128), ground, &BTreeMap::from([(1, 1.), (255, 0.)])),
        (0..128).flat_map(|x| (0..128).map(move |y| (x, y))),
    ));
    let mut brain = Brain::new(ARCHETYPES[1], &v);
    brain.goal = Goal::Explore(40, 22);
    brain.travel = Some((brain.goal.clone(), v.time_ms + 30_000, center((40, 22))));
    let selected = adventure(&mut brain, &v, &c);
    assert_eq!(selected.goal, Goal::Portal(2));
    assert_eq!(selected.movement.map(tile), Some((20, 24)));
    brain.goal = selected.goal;

    v.position = selected.movement.unwrap();
    let portal = v.objects.pop().unwrap();
    let detour = adventure(&mut brain, &v, &c);
    assert_eq!(detour.goal, Goal::Portal(2));
    assert_eq!(detour.movement.map(tile), Some((20, 23)));
    assert!(detour.journey.is_none());

    v.position = center((40, 23));
    let disappeared = adventure(&mut brain, &v, &c);
    assert_ne!(disappeared.goal, Goal::Portal(2));
    assert!(disappeared.journey.is_none());
    v.objects.push(portal);
    assert!(matches!(
        adventure(&mut brain, &v, &c).journey,
        Some(Journey::Portal(2))
    ));
}

#[test]
fn nexus_explore_enters_its_target_tile_before_retargeting() {
    let (c, mut v) = fixture();
    v.map.name = "Nexus".into();
    v.map.template = Some("nexus".into());
    v.position = Pos {
        x: 114.17,
        y: 49.40,
    };
    v.quest = None;
    v.objects.clear();
    let mut brain = Brain::new(ARCHETYPES[1], &v);
    let target = (113, 49);
    brain.goal = Goal::Explore(target.0, target.1);

    let approach = adventure(&mut brain, &v, &c);
    assert_eq!(approach.goal, Goal::Explore(target.0, target.1));
    v.position = approach.movement.expect("exploration still has a step");
    assert_eq!(tile(v.position), target);

    let onward = adventure(&mut brain, &v, &c);
    assert!(matches!(onward.goal, Goal::Explore(..)));
    assert_ne!(onward.goal, approach.goal);
}

fn nexus_explore_fixture(
    start: TilePoint,
    known: impl Fn(TilePoint) -> bool,
) -> (Content, PlayerView) {
    let (c, mut v) = fixture();
    let map = crate::content::Map::load(std::path::Path::new("data/nexus.wmap"), &c).unwrap();
    v.map.name = "Nexus".into();
    v.map.template = Some("nexus".into());
    (v.map.width, v.map.height) = (map.width, map.height);
    v.position = center(start);
    v.quest = None;
    v.objects.clear();
    let ground = map
        .cells
        .into_iter()
        .map(|cell| {
            if cell.object.is_some_and(|kind| c.objects[&kind].blocks) {
                255
            } else {
                cell.ground
            }
        })
        .collect();
    v.tiles = Arc::new(crate::navigation::Tiles::new(
        crate::navigation::Geometry::new((map.width, map.height), ground, &c.grounds),
        (0..map.width as i16)
            .flat_map(|x| (0..map.height as i16).map(move |y| (x, y)))
            .filter(|&p| known(p)),
    ));
    (c, v)
}

#[test]
fn nexus_explore_follows_the_frontier_past_the_first_hedge_waypoint() {
    let start = (42, 31);
    let (c, mut v) = nexus_explore_fixture(start, |(x, y)| {
        // Both ends of the hedge are known, but just a few tiles into each arm.
        x == 42 && (31..=51).contains(&y)
            || y == 31 && (42..=45).contains(&x)
            || y == 51 && (42..=48).contains(&x)
    });
    let mut brain = Brain::new(ARCHETYPES[1], &v);
    brain.heading = 0.;
    let waypoint = (45, 51);
    let frontier = (48, 51);
    for (leg, limit) in [(waypoint, 23), (frontier, 3)] {
        let grid = Grid::terrain(&v);
        assert_eq!(grid.path(tile(v.position), leg).unwrap().len(), limit);
        for _ in 0..limit {
            if tile(v.position) == leg {
                break;
            }
            brain.visits.insert(tile(v.position), v.tick);
            let decision = adventure(&mut brain, &v, &c);
            assert_eq!(
                decision.goal,
                Goal::Explore(leg.0, leg.1),
                "wrong leg from {:?}; returning to {start:?} would reverse the route",
                tile(v.position),
            );
            brain.goal = decision.goal;
            v.position = decision
                .movement
                .expect("exploration leg must make progress");
            v.tick += 1;
            v.time_ms += 100;
        }
        assert_eq!(tile(v.position), leg);
    }
}

#[test]
fn nexus_explore_leaves_the_hedge_courtyard_in_one_bounded_leg() {
    let (c, v) = nexus_explore_fixture((74, 75), |(x, y)| {
        [(74, 75), (76, 73)]
            .into_iter()
            .any(|(a, b)| (x - a).pow(2) + (y - b).pow(2) <= 400)
    });
    let mut brain = Brain::new(ARCHETYPES[1], &v);
    brain.heading = 2.14;
    let decision = adventure(&mut brain, &v, &c);
    let Goal::Explore(x, y) = decision.goal else {
        panic!("expected Nexus exploration, got {:?}", decision.goal);
    };
    assert!(y < 69, "selected another courtyard tile ({x}, {y})");
    assert_eq!(
        Grid::terrain(&v)
            .path(tile(v.position), (x, y))
            .unwrap()
            .len(),
        23
    );
    assert!(decision.movement.is_some_and(|p| p.y < v.position.y));
}

#[test]
fn explore_prefers_local_ground_over_the_far_side_of_a_hairpin() {
    let (c, mut v) = fixture();
    v.map.name = "Nexus".into();
    v.map.template = Some("nexus".into());
    v.quest = None;
    v.objects.clear();
    let mut ground = vec![255; 128 * 128];
    for y in 10..=35 {
        ground[y * 128 + 10] = 1;
        ground[y * 128 + 14] = 1;
    }
    ground[35 * 128 + 10..=35 * 128 + 14].fill(1);
    v.tiles = Arc::new(crate::navigation::Tiles::new(
        crate::navigation::Geometry::new((128, 128), ground, &BTreeMap::from([(1, 1.), (255, 0.)])),
        (0..128).flat_map(|x| (0..128).map(move |y| (x, y))),
    ));
    let mut brain = Brain::new(ARCHETYPES[1], &v);
    brain.heading = std::f32::consts::FRAC_PI_4;
    for y in 10..=34 {
        brain.visits.insert((10, y), v.tick);
    }
    let decision = adventure(&mut brain, &v, &c);
    let Goal::Explore(x, y) = decision.goal else {
        panic!("expected local exploration, got {:?}", decision.goal);
    };
    assert_eq!(x, 10, "picked a target around the hairpin: ({x}, {y})");
    assert!(decision.movement.is_some_and(|p| p.y > v.position.y));
}

#[test]
fn travelling_realm_hunter_shoots_passing_enemy_without_changing_course() {
    let (c, mut v) = fixture();
    v.objects[0].position = center((28, 10));
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    let first = adventure(&mut brain, &v, &c);
    assert_eq!(first.goal, Goal::Hunt(2));
    brain.goal = first.goal;
    v.position = first.movement.expect("quest approach started");
    v.tick += 5;
    v.time_ms += 250;
    let kind = 0x223;
    v.objects.push(VisibleObject {
        id: 3,
        account: None,
        kind,
        name: c.objects[&kind].name.clone(),
        position: Pos {
            x: v.position.x - 3.,
            y: v.position.y,
        },
        hp: c.objects[&kind].hp,
        max_hp: c.objects[&kind].hp,
        level: None,
        equipped: vec![],
        contents: vec![],
        blocks: false,
        usable: false,
        condition_mask: Some(0),
    });
    let passing = adventure(&mut brain, &v, &c);
    assert_eq!(passing.goal, Goal::Hunt(2));
    assert!(passing.movement.is_some_and(|p| p.x > v.position.x));
    assert!(passing.aim.is_some_and(|p| p.x < v.position.x));
}

#[test]
fn realm_god_hunt_keeps_its_approach_when_a_construct_crosses_range() {
    let (c, mut v) = fixture();
    v.quest = None;
    v.character.level = 20;
    v.objects = [(2, 0x905), (3, 0x906)]
        .into_iter()
        .map(|(id, kind)| VisibleObject {
            id,
            kind,
            name: c.objects[&kind].name.clone(),
            hp: c.objects[&kind].hp,
            max_hp: c.objects[&kind].hp,
            equipped: vec![],
            contents: vec![],
            ..v.objects[0]
        })
        .collect();
    let mut brain = Brain::new(ARCHETYPES[1], &v);
    let decisions = [(9.5, 8.9), (9.5, 8.), (9.5, 8.9), (6., 8.)].map(|(rock, wood)| {
        v.objects[0].position = Pos {
            x: v.position.x + rock,
            y: v.position.y,
        };
        v.objects[1].position = Pos {
            x: v.position.x,
            y: v.position.y + wood,
        };
        v.tick += 4;
        v.time_ms += 200;
        let decision = brain.decide(&v, &c).unwrap();
        brain.goal = decision.goal.clone();
        decision
    });
    assert_eq!(
        decisions.each_ref().map(|d| d.goal.clone()),
        [Goal::Hunt(2), Goal::Hunt(2), Goal::Hunt(2), Goal::Fight(2)]
    );
    let passing = &decisions[1];
    assert!(passing.movement.is_some_and(|p| p.x > v.position.x));
    assert!(passing.aim.is_some_and(|p| p.y > v.position.y));
}

#[test]
fn realm_hunt_keeps_a_route_that_already_dodges_the_shot() {
    let (c, mut v) = fixture();
    v.objects[0].position = center((28, 10));
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    let first = brain.decide(&v, &c).unwrap();
    assert_eq!(first.goal, Goal::Hunt(2));
    brain.goal = first.goal;
    v.tick += 6;
    v.time_ms += 300;

    let shot = crate::world::view::VisibleProjectile {
        source_kind: 0x654,
        already_hit_player: false,
        owner: 2,
        id: 1,
        start: center((10, 7)),
        position: center((10, 7)),
        angle: std::f32::consts::FRAC_PI_2,
        speed: 80.,
        lifetime_ms: 2000.,
        amplitude: 0.,
        frequency: 0.,
        passes_cover: false,
        damage: 20,
        armor_piercing: false,
    };
    v.projectiles.push(shot);
    let continuing = brain.decide(&v, &c).unwrap();
    assert_eq!(continuing.goal, Goal::Hunt(2));
    let route = first.movement.expect("the hunter started toward the quest");
    let step = continuing.movement.expect("the hunter keeps moving");
    assert!(
        step.distance(route) < 0.01,
        "route {route:?} already dodges, got {step:?}"
    );

    v.tick += 6;
    v.time_ms += 300;
    v.projectiles[0].start = center((14, 10));
    v.projectiles[0].position = center((14, 10));
    v.projectiles[0].angle = std::f32::consts::PI;
    let dodging = brain.decide(&v, &c).unwrap();
    assert_eq!(dodging.goal, Goal::Hunt(2));
    let dodge = dodging
        .movement
        .expect("the hunter dodges across the route");
    assert!(combat::clearance(&v, dodge) > combat::clearance(&v, route) + 0.5);
}

pub(super) fn fixture() -> (Content, PlayerView) {
    let content = crate::world::test_content();
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
        inputs: None,
        session: Session {
            world: -3,
            player: 1,
            account: 1,
            character: 1,
        },
        tick: 100,
        time_ms: 5000,
        clock_at: std::time::Instant::now(),
        bot_damage_3s: 0,
        volley_margin_ms: Arc::default(),
        recent_market_peers: vec![],
        elapsed_ms: 5000,
        position: Pos { x: 10.5, y: 10.5 },
        quest: Some(2),
        map: MapInfo {
            name: "Realm".into(),
            template: Some("realm".into()),
            width: 128,
            height: 128,
            allow_teleport: true,
            layout: None,
            godlands: Arc::default(),
            godland_route: Arc::default(),
            // Godlands sightings anywhere on this small fixture count as the Godlands.
            godland_tiles: Arc::new(
                (0..128)
                    .flat_map(|x| (0..128).map(move |y| (x, y)))
                    .collect(),
            ),
        },
        character: OwnCharacter {
            conditions: 0,
            class: crate::content::WIZARD,
            level: 14,
            experience: 20000,
            fame: 20,
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
        throws: vec![],
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
fn recovering_wizard_nova_respects_target_protection_and_mana() {
    use crate::{
        condition::{INVINCIBLE, INVULNERABLE, PARALYZED, STASIS, STUNNED, bit},
        content::{Cell, Map},
        store::Character,
        world::{Controller, World},
    };
    for (protection, mana, casts) in [
        (0, 200, true),
        (bit(INVULNERABLE), 200, false),
        (bit(INVINCIBLE), 200, false),
        (bit(STASIS), 200, false),
        (0, 29, false),
    ] {
        let (content, mut view) = fixture();
        view.character.hp = 225;
        view.character.mp = mana;
        // Hold the sole target still so only the spell can change its HP.
        let conditions = protection | bit(PARALYZED) | bit(STUNNED);
        view.objects[0].position = Pos { x: 14.5, y: 10.5 };
        view.objects[0].condition_mask = Some(conditions);
        let content = Arc::new(content);
        let mut world = World::new(
            content.clone(),
            Arc::new(Map {
                elevation: None,
                width: 128,
                height: 128,
                cells: vec![
                    Cell {
                        ground: 1,
                        object: None,
                        config: String::new(),
                        terrain: 0,
                        region: 0,
                    };
                    128 * 128
                ],
                spawns: vec![(view.position.x, view.position.y)],
            }),
        )
        .unwrap();
        let mut character = Character::fresh(content.get(view.character.class).unwrap()).unwrap();
        character.name = "Recoverer".into();
        character.id = view.session.character;
        character.account = view.session.account;
        character.base = view.character.base_stats;
        character.items = view.character.items.clone();
        character.hp = view.character.hp;
        character.mp = mana;
        let player = world.enter(character, None).unwrap();
        world.players.get_mut(&player).unwrap().controller = Controller::Autonomous;
        let target = world
            .spawn_entity(view.objects[0].kind, view.objects[0].position, vec![])
            .unwrap();
        world.entities.get_mut(&target).unwrap().condition = conditions;
        view.session.player = player;
        view.objects[0].id = target;
        view.quest = Some(target);
        let mut brain = Brain::new(ARCHETYPES[0], &view);
        let decision = brain.decide(&view, &content).unwrap();
        assert!(brain.recover_until > view.tick);
        assert_eq!(brain.activity, Activity::Survive);
        assert_eq!(decision.goal, Goal::Recover);
        assert!(
            decision.journey.is_none(),
            "Nexus must not mask the retreat cast"
        );
        // Live: 32 of 32 sampled Recover frames never fired; a 2012 player backing off kept shooting.
        assert_eq!(
            decision.aim.is_some(),
            protection == 0,
            "retreat shoots back only at a target it can hurt"
        );
        let backstep = decision.movement.expect("viable retreat step");
        assert!(
            backstep.distance(view.objects[0].position)
                > view.position.distance(view.objects[0].position)
        );
        world
            .autonomous_action(
                player,
                Action::Move {
                    x: backstep.x,
                    y: backstep.y,
                },
            )
            .unwrap();
        assert_eq!(
            decision.item.is_some(),
            casts,
            "protection={protection}, mana={mana}"
        );
        if let Some(action) = decision.item {
            assert!(matches!(action, Action::Use { slot: 1, .. }));
            world.autonomous_action(player, action).unwrap();
        }
        let spell = content.get(0xa2e).unwrap();
        assert_eq!(spell.mana_cost, 30);
        assert_eq!(world.bullets.len(), if casts { 20 } else { 0 });
        assert_eq!(
            world.players[&player].character.mp,
            mana - if casts { 30 } else { 0 }
        );
        let ready = world.players[&player].ability_ready_ms();
        assert_eq!(ready > 0, casts);
        let hp = world.entities[&target].hp;
        for _ in 0..=(spell.projectiles[&0].lifetime as u64 / 50) {
            world.step();
            assert_eq!(world.entities[&target].condition & protection, protection);
        }
        assert!(world.players[&player].pos.distance(backstep) < 0.1);
        // XML Oasis minions keep shooting; only the nova's own projectiles must have expired.
        assert!(world.bullets.iter().all(|b| b.owner != player));
        if casts {
            assert!(world.entities[&target].hp < hp);
        } else {
            assert_eq!(world.entities[&target].hp, hp);
        }
        assert_eq!(
            world.players[&player].character.shots,
            if casts { 20 } else { 0 }
        );
        assert_eq!(world.players[&player].ability_ready_ms(), ready);
        assert!(world.players[&player].character.mp >= mana - if casts { 30 } else { 0 });
    }
}

fn rusher_fixture() -> (Content, PlayerView) {
    let (content, mut view) = fixture();
    view.map.name = "Abyss of Demons".into();
    view.map.template = Some("abyss-of-demons".into());
    view.objects[0].kind = 0x927;
    view.objects[0].position = Pos { x: 110.5, y: 10.5 };
    view.player_markers.push(PlayerMarker {
        id: 3,
        account: 3,
        name: "Rusher".into(),
        position: Pos { x: 100.5, y: 10.5 },
    });
    (content, view)
}

#[test]
fn dungeon_quest_uses_a_public_rusher_without_abandoning_its_goal() {
    let (content, mut view) = rusher_fixture();
    for world in [
        "pirate-cave",
        "snake-pit",
        "sprite-world",
        "undead-lair",
        "abyss-of-demons",
        "ocean-trench",
    ] {
        view.map.template = Some(world.into());
        let mut brain = Brain::new(ARCHETYPES[0], &view);
        let decision = brain.decide(&view, &content).unwrap();
        assert_eq!(decision.goal, Goal::Hunt(2), "{world}");
        assert!(
            matches!(decision.journey, Some(Journey::Regroup(3))),
            "{world}: {:?}",
            decision.journey
        );
        view.tick += 10;
        view.time_ms += 100;
        view.elapsed_ms += 100;
        assert!(
            brain.decide(&view, &content).unwrap().journey.is_none(),
            "{world}: cooldown"
        );
    }
}

#[test]
fn dungeon_rusher_does_not_override_safety_or_teleport_policy() {
    for case in 0..8 {
        let (content, mut v) = rusher_fixture();
        let mut profile = ARCHETYPES[0];
        match case {
            0 => v.map.allow_teleport = false,
            1 => v.player_markers.clear(),
            2 => v.player_markers[0].position = Pos { x: 5.5, y: 10.5 },
            3 => v.player_markers[0].position = Pos { x: 20.5, y: 10.5 },
            4 => v.character.hp = 100,
            5 => profile.knows_minimap = false,
            6 => {
                let mut enemy = fixture().1.objects.pop().unwrap();
                enemy.id = 4;
                enemy.kind = 0x927;
                enemy.position = Pos { x: 12.5, y: 10.5 };
                v.objects.push(enemy);
            }
            7 => v.player_markers[0].position = v.objects[0].position,
            _ => unreachable!(),
        }
        let decision = Brain::new(profile, &v).decide(&v, &content).unwrap();
        assert!(
            !matches!(decision.journey, Some(Journey::Regroup(_))),
            "case {case}: {:?}",
            decision.journey
        );
    }
}

#[test]
fn wavelength_vault_errand_banks_before_leaving() {
    let (c, mut v) = fixture();
    v.map.name = "Nexus".into();
    v.map.template = Some("nexus".into());
    v.quest = None;
    let (sword, potion) = (0xa47, 0xa1f);
    let stat = progression::potion_index(&c, potion).unwrap().unwrap();
    v.character.base_stats[stat] = c.classes[&v.character.class].caps[stat];
    v.character.items[4..].copy_from_slice(&[sword, potion, potion, -1, -1, -1, -1, -1]);
    let mut chest = v.objects.remove(0);
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    brain.mind.market.trader = true;
    brain.decide(&v, &c).unwrap();
    assert_eq!(
        brain.mind.market.reserved_kind(),
        Some(potion),
        "a funded quote opened a Nexus market session"
    );
    brain
        .plan
        .start(plan::now(), plan::Owner::Bank, "Vault".into());
    v.map.name = "Vault".into();
    v.map.template = Some("vault".into());
    chest.kind = 0x504;
    chest.hp = 0;
    chest.position = Pos { x: 11.5, y: 10.5 };
    chest.contents = vec![-1; 8];
    v.objects.push(chest);
    v.tick += 5;
    let deposit = brain.decide(&v, &c).unwrap();
    assert_eq!(deposit.goal, Goal::Bank(2));
    assert!(matches!(&deposit.item, Some(Action::Swap { from, .. }) if from.kind == sword));
    assert!(
        deposit.journey.is_none(),
        "the Vault is not left before banking"
    );
    v.character.items[4] = -1;
    v.objects[0].contents[0] = sword;
    v.character.inventory_revision += 1;
    v.tick += 5;
    let leave = brain.decide(&v, &c).unwrap();
    assert!(
        brain
            .plan
            .events
            .iter()
            .any(|e| e.kind == plan::Kind::Completed && e.owner == Some(plan::Owner::Bank))
    );
    assert!(
        matches!(leave.journey, Some(Journey::Nexus)),
        "quote stock stays carried and the market visit resumes: {:?}",
        leave.journey
    );
}

#[test]
fn band_edge_journeys_do_not_preempt_each_other() {
    // Smoke runs: Venture pulled back across a region edge every other decision (Explore, then Revisit).
    for revisit in [None, Some(Pos { x: 30.5, y: 10.5 })] {
        let (c, mut v) = fixture();
        v.quest = None;
        v.character.level = 11;
        v.position = Pos { x: 15.5, y: 10.5 };
        v.objects[0].kind = c
            .objects
            .values()
            .find(|o| o.enemy && (5..=7).contains(&o.terrain))
            .unwrap()
            .kind;
        v.objects[0].position = Pos { x: 2.5, y: 10.5 };
        let mut brain = Brain::new(ARCHETYPES[0], &v);
        brain
            .crowd
            .bands
            .observe(&v, &c, &mut brain.mind.godlands, false, "Tester");
        v.objects.clear();
        brain.heading = 0.;
        brain.hunting = revisit;
        v.time_ms += 1_000_000;
        let mut goals = Vec::new();
        for _ in 0..12 {
            v.tick += 5;
            v.time_ms += 250;
            v.elapsed_ms += 250;
            let d = brain.decide(&v, &c).unwrap();
            v.position = d.movement.expect("the bot keeps travelling");
            goals.push(format!("{:?}", d.goal));
        }
        let kinds: Vec<_> = goals.iter().map(|g| g.split('(').next().unwrap()).collect();
        let switches = kinds.windows(2).filter(|w| w[0] != w[1]).count();
        assert!(switches <= 3, "journeys alternate: {goals:?}");
    }
}

#[test]
fn phoenix_guarded_chest_is_left_until_the_guard_moves_away() {
    let (c, mut v) = fixture();
    v.quest = None;
    v.character.level = 20;
    let chest = center((20, 10));
    let phoenix = &c.objects[&0x675];
    v.objects[0].kind = phoenix.kind;
    v.objects[0].name.clone_from(&phoenix.name);
    v.objects[0].hp = phoenix.hp;
    v.objects[0].max_hp = phoenix.hp;
    v.objects[0].position = chest;
    v.objects.push(VisibleObject {
        id: 3,
        kind: 0x501,
        position: chest,
        hp: 0,
        max_hp: 0,
        name: c.objects[&0x501].name.clone(),
        equipped: vec![],
        contents: vec![0xa22],
        ..v.objects[0]
    });
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    let goals = [10, 13, 10].map(|x| {
        v.position = center((x, 10));
        v.tick += 5;
        v.time_ms += 250;
        let d = brain.decide(&v, &c).unwrap();
        brain.goal = d.goal.clone();
        d.goal
    });
    assert!(!goals.contains(&Goal::Loot(3)), "{goals:?}");
    assert_eq!(goals[1], Goal::Fight(2));

    v.objects[0].position = center((32, 10));
    for x in [15, 19] {
        v.position = center((x, 10));
        v.tick += 5;
        v.time_ms += 250;
        let d = brain.decide(&v, &c).unwrap();
        assert_eq!(d.goal, Goal::Loot(3));
        brain.goal = d.goal;
        if x == 15 {
            assert!(d.movement.is_some_and(|p| p.distance(chest) < 5.));
        } else {
            assert!(matches!(d.item, Some(Action::Swap { from, .. }) if from.object == 3));
        }
    }
}

#[test]
fn a_fight_just_outside_weapon_range_does_not_restart_distant_loot() {
    let (c, mut v) = fixture();
    v.quest = None;
    v.character.level = 20;
    v.character.items[1] = -1;
    let projectile = &c.objects[&(v.character.items[0] as u16)].projectiles[&0];
    let range = projectile.speed * projectile.lifetime / 10000.;
    v.objects[0].position = Pos {
        x: v.position.x + range - 0.25,
        y: v.position.y,
    };
    let bag = center((10, 13));
    v.objects.push(VisibleObject {
        id: 3,
        kind: 0x500,
        position: bag,
        hp: 0,
        max_hp: 0,
        name: c.objects[&0x500].name.clone(),
        equipped: vec![],
        contents: vec![0xa22],
        ..v.objects[0]
    });
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    let fight = brain.decide(&v, &c).unwrap();
    assert_eq!(fight.goal, Goal::Fight(2));
    assert!(fight.aim.is_some());
    brain.goal = fight.goal;

    v.position.x -= 0.5;
    v.tick += 6;
    v.time_ms += 300;
    let fight = brain.decide(&v, &c).unwrap();
    assert_eq!(fight.goal, Goal::Fight(2));
    assert!(fight.movement.is_some_and(|p| p.x > v.position.x));
    assert!(fight.item.is_none());

    v.position = bag;
    v.tick += 6;
    v.time_ms += 300;
    let nearby = brain.decide(&v, &c).unwrap();
    assert_eq!(nearby.goal, Goal::Loot(3));
    assert!(matches!(nearby.item, Some(Action::Swap { from, .. }) if from.object == 3));
}

#[test]
fn loot_approach_crosses_a_venture_band_edge_and_collects_the_chest() {
    let (c, mut v) = fixture();
    v.quest = None;
    v.character.level = 11;
    v.position = Pos {
        x: 56.5465,
        y: 42.5,
    };
    let observed = &mut v.objects[0];
    observed.kind = c
        .objects
        .values()
        .find(|o| o.enemy && bands::tier(o.terrain) == 2)
        .unwrap()
        .kind;
    observed.position = Pos {
        x: 71.8853,
        y: 52.6978,
    };
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    brain.crowd.bands.observe(
        &v,
        &c,
        &mut brain.mind.godlands,
        false,
        brain.profile.identity,
    );
    let chest = &mut v.objects[0];
    chest.kind = 0x501;
    chest.hp = 0;
    chest.position = Pos { x: 49.5, y: 59.5 };
    chest.contents = vec![0xa22];
    let geometry = crate::navigation::Geometry::new(
        (128, 128),
        (0..128 * 128)
            .map(|i| match (i % 128, i / 128) {
                (56, 43) | (60, 45) | (55, 46) => 255,
                _ => 1,
            })
            .collect(),
        &BTreeMap::from([(1, 1.), (255, 0.)]),
    );
    v.tiles = Arc::new(crate::navigation::Tiles::new(
        geometry,
        (0..128).flat_map(|x| (0..128).map(move |y| (x, y))),
    ));
    assert!(brain.crowd.bands.venture(&v).is_none());
    for step in 0..20 {
        let d = adventure(&mut brain, &v, &c);
        assert_eq!(d.goal, Goal::Loot(2), "step {step} at {:?}", v.position);
        if matches!(d.item, Some(Action::Swap { from, .. }) if from.object == 2) {
            return;
        }
        v.position = d.movement.expect("approach continues until the swap");
        if step == 0 {
            assert_eq!(tile(v.position), (55, 42));
            assert!(brain.crowd.bands.venture(&v).is_some());
        }
        brain.goal = d.goal;
        v.time_ms += 250;
        v.tick += 5;
    }
    panic!("never reached the chest from the band edge");
}

#[test]
fn unreachable_hunt_is_travel_that_aborts_not_engagement() {
    let (c, mut v) = fixture();
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    // The bot never gets closer, as when held movement cannot reach the quest; the target shifts a tile.
    let mut aborted = false;
    for step in 0..20 {
        v.tick += 5;
        v.time_ms += 10_000;
        v.elapsed_ms += 10_000;
        v.objects[0].position.x = 10.5 + (step % 2) as f32;
        if aborted {
            // Whatever journey follows ends; only remembering the target prevents re-choosing it.
            brain.travel = None;
        }
        let goal = brain.decide(&v, &c).unwrap().goal;
        if step == 0 {
            assert_eq!(goal, Goal::Hunt(2));
        }
        assert!(
            !aborted || goal != Goal::Hunt(2),
            "step {step}: the aborted target is re-chosen"
        );
        aborted |= brain
            .plan
            .events
            .iter()
            .any(|e| e.reason == "travel deadline passed");
    }
    assert!(!brain.plan.engaged, "an out-of-range hunt is not a fight");
    assert!(
        brain
            .plan
            .events
            .iter()
            .any(|e| e.kind == plan::Kind::Aborted && e.reason == "travel deadline passed")
    );
}

#[test]
fn realm_wedge_loops_but_nexus_spawn_congestion_does_not() {
    for (map, looping) in [("nexus", false), ("realm", true)] {
        let (c, mut v) = fixture();
        v.map.template = Some(map.into());
        v.quest = None;
        v.objects.clear();
        let mut brain = Brain::new(ARCHETYPES[0], &v);
        for _ in 0..3 {
            brain.destination = Some(Pos { x: 20.5, y: 10.5 });
            v.tick += 31;
            brain.decide(&v, &c).unwrap();
        }
        let flag = brain.plan.flag(plan::now());
        assert_eq!(
            flag.as_ref().is_some_and(|f| f["kind"] == "looping"),
            looping,
            "{map}: {flag:?}"
        );
    }
}

#[test]
fn threatened_bot_beside_health_bag_handles_the_threat_instead_of_leaving() {
    let (c, mut v) = fixture();
    v.quest = None;
    let mut bag = v.objects.remove(0);
    bag.id = 9;
    bag.kind = 0x500;
    bag.hp = 0;
    bag.contents = vec![0xa22];
    bag.position = Pos { x: 11.5, y: 10.5 };
    v.objects.push(bag);
    v.projectiles.push(crate::world::view::VisibleProjectile {
        source_kind: 0x654,
        already_hit_player: false,
        owner: 2,
        id: 1,
        start: Pos { x: 13.5, y: 10.5 },
        position: Pos { x: 12.5, y: 10.5 },
        angle: std::f32::consts::PI,
        speed: 100.,
        lifetime_ms: 2000.,
        amplitude: 0.,
        frequency: 0.,
        passes_cover: false,
        damage: 0,
        armor_piercing: false,
    });
    let mut brain = Brain::new(ARCHETYPES[1], &v);
    let d = brain.decide(&v, &c).unwrap();
    assert!(
        d.journey.is_none(),
        "a visible supply is not a reason to leave"
    );
}

#[test]
fn explorer_joins_a_crowded_realm_event_by_teleporting_outside_the_fight() {
    // Live: level 20s kept exploring past their quest and routed around any event they could not solo, even in a crowd.
    let (c, mut v) = fixture();
    v.character.level = 20;
    v.objects[0].kind = 0xd50;
    v.objects[0].hp = 70000;
    v.objects[0].max_hp = 70000;
    v.player_markers = (3..7)
        .map(|id| PlayerMarker {
            id,
            account: id,
            name: format!("P{id}"),
            position: Pos {
                x: 8.5 + id as f32,
                y: 85.5,
            },
        })
        .collect();
    v.player_markers.push(PlayerMarker {
        id: 7,
        account: 7,
        name: "Outside".into(),
        position: Pos { x: 10.5, y: 66.5 },
    });
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    brain.goal = Goal::Explore(20, 10);
    brain.travel = Some((Goal::Explore(20, 10), u64::MAX, Pos { x: 20.5, y: 10.5 }));
    let d = brain.decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Hunt(2));
    assert!(
        matches!(d.journey, Some(Journey::Regroup(7))),
        "{:?}",
        d.journey
    );
    // Waiting out the teleport cooldown, the bot walks toward the event instead of around it.
    v.tick += 10;
    v.time_ms += 100;
    v.elapsed_ms += 100;
    let d = brain.decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Hunt(2));
    assert!(
        d.movement.is_some_and(|m| m.y > v.position.y),
        "{:?}",
        d.movement
    );
}

#[test]
fn quest_hunter_seeds_an_uncrowded_realm_event_that_godlands_farmers_leave() {
    // Live: Lord of the Lost Lands was skipped 16 times in 25 minutes because nobody would start the crowd.
    let (c, mut v) = fixture();
    v.character.level = 20;
    v.objects[0].kind = 0xd50;
    v.objects[0].hp = 70000;
    v.objects[0].max_hp = 70000;
    let mut profile = ARCHETYPES[0];
    assert_ne!(
        Brain::new(profile, &v).decide(&v, &c).unwrap().goal,
        Goal::Hunt(2)
    );
    profile.identity = "Gnosis";
    assert!(quests::hunts_quests(&profile));
    let d = Brain::new(profile, &v).decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Hunt(2));
    assert!(
        d.movement.is_some_and(|m| m.y > v.position.y),
        "{:?}",
        d.movement
    );
}

#[test]
fn veteran_carrying_tradeable_loot_travels_to_the_nexus_to_trade_it() {
    // Live: 1 bot in 58 wanted anything, and loot was only offered when banking happened to pass the Nexus.
    let (c, mut v) = fixture();
    v.character.level = 20;
    v.objects.clear();
    let cap = c.classes[&v.character.class].caps[2];
    v.character.base_stats[2] = cap;
    v.character.stats[2] = cap;
    v.character.items[4] = 0xa1f;
    v.character.items[5] = 0xa1f;
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    let d = brain.decide(&v, &c).unwrap();
    assert!(
        matches!(d.journey, Some(Journey::Nexus)),
        "{:?} {:?}",
        d.goal,
        d.journey
    );
    assert_eq!(brain.plan.owner(), Some(plan::Owner::Trade));
    assert!(brain.mind.market.sought);
}

#[test]
fn bot_escapes_a_volley_that_would_kill_it_before_its_health_looks_low() {
    // Live: Cube God volleys took 100-300 HP at once, so escaping below a quarter of max health came too late.
    let (c, mut v) = fixture();
    v.quest = None;
    v.objects.clear();
    v.character.hp = 300;
    v.projectiles.push(crate::world::view::VisibleProjectile {
        source_kind: 0x654,
        already_hit_player: false,
        owner: 2,
        id: 1,
        start: Pos { x: 13.5, y: 10.5 },
        position: Pos { x: 11., y: 10.5 },
        angle: std::f32::consts::PI,
        speed: 100.,
        lifetime_ms: 2000.,
        amplitude: 0.,
        frequency: 0.,
        passes_cover: false,
        damage: 400,
        armor_piercing: false,
    });
    let mut brain = Brain::new(ARCHETYPES[1], &v);
    let d = brain.decide(&v, &c).unwrap();
    assert!(matches!(d.journey, Some(Journey::Nexus)), "{:?}", d.goal);
    assert!(
        brain
            .plan
            .events
            .iter()
            .any(|e| e.reason == "lethal_volley")
    );
}

#[test]
fn bots_escape_before_a_second_volley_when_a_potion_cannot_cover_it() {
    let (c, mut v) = fixture();
    v.quest = None;
    v.objects[0].position = Pos { x: 16.5, y: 10.5 };
    v.character.mp = 0;
    v.character.items[4] = 0xa22;
    v.character.items[5] = 0xa23;
    v.projectiles.push(crate::world::view::VisibleProjectile {
        source_kind: 0x659,
        already_hit_player: false,
        owner: 2,
        id: 1,
        start: v.objects[0].position,
        position: Pos { x: 12.5, y: 10.5 },
        angle: std::f32::consts::PI,
        speed: 50.,
        lifetime_ms: 2700.,
        amplitude: 0.,
        frequency: 0.,
        passes_cover: false,
        damage: 120,
        armor_piercing: false,
    });
    for (world, template, level, enemy) in [
        ("Realm", "realm", 8, 0x650),
        ("Snake Pit", "snake-pit", 14, 0x225),
        ("Realm", "realm", 20, 0x659),
    ] {
        v.map.name = world.into();
        v.map.template = Some(template.into());
        v.character.level = level;
        v.objects[0].kind = enemy;
        // Each hit does 110, so a potion can buy a safe retreat at 260 HP, but not at 200 HP.
        for (hp, sick, escape) in [(200, false, true), (260, true, true), (260, false, false)] {
            v.character.hp = hp;
            v.character.conditions = if sick {
                crate::condition::bit(crate::condition::SICK)
            } else {
                0
            };
            let mut brain = Brain::new(ARCHETYPES[1], &v);
            let d = brain.decide(&v, &c).unwrap();
            assert_eq!(d.goal, Goal::Recover);
            assert_eq!(
                matches!(d.journey, Some(Journey::Nexus)),
                escape,
                "{world} level {level}"
            );
            assert_eq!(
                v.volley_margin_ms
                    .swap(0, std::sync::atomic::Ordering::Relaxed),
                if escape { v.time_ms + 1 } else { 0 }
            );
            if escape {
                assert!(matches!(d.item, Some(Action::Use { slot: 5, .. })));
                assert!(
                    brain
                        .plan
                        .events
                        .iter()
                        .any(|e| e.reason == "volley_margin")
                );
            } else {
                assert!(matches!(d.item, Some(Action::Use { slot: 4, .. })));
                assert!(
                    d.movement
                        .is_some_and(|p| p.distance(v.objects[0].position) > 6.)
                );
            }
        }
    }
}

#[test]
fn escape_forecast_ranks_threat_kinds_by_damage_and_counts_projectiles() {
    let (content, mut v) = fixture();
    v.character.stats[3] = 0;
    for (id, (source_kind, damage)) in [
        (0x659, 10),
        (0x659, 10),
        (0x91a, 50),
        (0x650, 30),
        (0x650, 30),
    ]
    .into_iter()
    .enumerate()
    {
        v.projectiles.push(crate::world::view::VisibleProjectile {
            source_kind,
            already_hit_player: false,
            owner: 20,
            id: id as u8,
            start: v.position,
            position: v.position,
            angle: 0.,
            speed: 100.,
            lifetime_ms: 500.,
            amplitude: 0.,
            frequency: 0.,
            passes_cover: false,
            damage,
            armor_piercing: false,
        });
    }
    v.throws.push(crate::world::view::VisibleThrow {
        source_kind: 0x654,
        position: v.position,
        radius: 4.,
        damage: 90,
        lands_ms: v.time_ms + 200,
    });
    assert_eq!(combat::incoming(&v), 220);
    assert_eq!(
        combat::forecast_threats(&v, &content),
        [("Medusa", 0, 90), ("Lizard God", 2, 60), ("Djinn", 1, 50),]
    );
}

#[test]
fn escape_forecast_counts_djinn_projectiles_only_once_per_player() {
    let (content, mut v) = fixture();
    let kind = 0x91a;
    let shot = &content.objects[&kind].projectiles[&0];
    assert!(shot.multi_hit);
    for id in 1..=2 {
        v.projectiles.push(crate::world::view::VisibleProjectile {
            source_kind: kind,
            already_hit_player: id == 1,
            owner: 20,
            id,
            start: v.position,
            position: v.position,
            angle: 0.,
            speed: shot.speed,
            lifetime_ms: shot.lifetime,
            amplitude: shot.amplitude,
            frequency: shot.frequency,
            passes_cover: shot.passes_cover,
            damage: shot.max,
            armor_piercing: shot.armor_piercing,
        });
    }
    assert_eq!(combat::incoming(&v), 50);
    assert_eq!(combat::forecast_threats(&v, &content), [("Djinn", 1, 50)]);
}

#[test]
fn low_health_requires_an_attacker_nearby_to_use_the_reserve() {
    let (c, mut v) = fixture();
    v.quest = None;
    v.character.level = 8;
    v.character.hp = 35;
    v.objects[0].kind = 0x650;
    v.objects[0].position = Pos { x: 16.5, y: 10.5 };
    let mut brain = Brain::new(ARCHETYPES[1], &v);
    let d = brain.decide(&v, &c).unwrap();
    assert!(matches!(d.journey, Some(Journey::Nexus)));
    assert!(
        brain
            .plan
            .events
            .iter()
            .any(|e| e.reason == "under_fire_low_health")
    );

    v.objects[0].position.x = 30.5;
    v.tick += 5;
    v.time_ms += 250;
    let d = brain.decide(&v, &c).unwrap();
    assert!(d.journey.is_none(), "a remembered hit is not a volley");
}

#[test]
fn bot_standing_in_lava_steps_out_and_dodges_a_shown_bomb() {
    // Live: lava killed 16 and Medusa 17 level 20s a day; neither the ground nor the bomb's landing circle was avoided.
    let (c, mut v) = fixture();
    v.quest = None;
    v.objects.clear();
    let lava = |x: i16, y: i16| (8..=11).contains(&x) && (9..=11).contains(&y);
    let geometry = crate::navigation::Geometry::new(
        (128, 128),
        (0..128 * 128)
            .map(|i| {
                if lava((i % 128) as i16, (i / 128) as i16) {
                    0xf4
                } else {
                    1
                }
            })
            .collect(),
        &BTreeMap::from([(1, 1.), (0xf4, 0.666)]),
    );
    v.tiles = Arc::new(crate::navigation::Tiles::new(
        geometry,
        (0..128).flat_map(|x| (0..128).map(move |y| (x, y))),
    ));
    let d = Brain::new(ARCHETYPES[1], &v).decide(&v, &c).unwrap();
    let out = d.movement.expect("movement out of lava");
    assert!(!lava(tile(out).0, tile(out).1), "{out:?}");
    assert!(out.distance(v.position) <= 2., "{out:?}");

    let (c, mut v) = fixture();
    v.quest = None;
    v.objects.clear();
    v.throws.push(crate::world::view::VisibleThrow {
        source_kind: 0x654,
        position: Pos { x: 11.5, y: 10.5 },
        radius: 4.,
        damage: 150,
        lands_ms: v.time_ms + 1000,
    });
    let d = Brain::new(ARCHETYPES[1], &v).decide(&v, &c).unwrap();
    assert!(
        d.movement.is_some_and(|m| m.x < v.position.x),
        "{:?}",
        d.movement
    );
}

#[test]
fn level_twenty_heads_for_its_remembered_godlands_past_lower_enemies() {
    // Live: level 20s entered each Realm knowing nothing of its Godlands, followed midland arrows and stopped for every enemy in range.
    let (c, mut v) = fixture();
    v.character.level = 20;
    v.map.layout = Some("classic-world-02-reconstruction".into());
    v.objects.push(VisibleObject {
        id: 3,
        account: None,
        kind: 0x617,
        name: "Hobbit Mage".into(),
        position: Pos { x: 13.5, y: 10.5 },
        hp: 100,
        max_hp: 100,
        level: None,
        equipped: vec![],
        contents: vec![],
        blocks: false,
        usable: false,
        condition_mask: Some(0),
    });
    let godlands = Pos { x: 100.5, y: 100.5 };
    v.map.godlands = Arc::new(vec![godlands]);
    v.map.godland_tiles = Arc::new(BTreeSet::from([tile(godlands)]));
    // Live: 76% of Realm entries remembered nothing; a veteran knows a map it has played before.
    let geared = v.character.items[0];
    // Live: a level 20 with a tier-3 weapon never set out, though its own band lies on the way to the Godlands.
    let weak = c
        .objects
        .values()
        .find(|o| o.slot_type == c.objects[&(geared as u16)].slot_type && o.tier == Some(3))
        .map(|o| i32::from(o.kind))
        .unwrap();
    for (remembered, veteran, weapon, goal) in [
        (true, false, geared, Goal::Venture),
        (false, true, geared, Goal::Venture),
        (false, true, weak, Goal::Venture),
        (false, false, geared, Goal::Fight(3)),
    ] {
        v.character.items[0] = weapon;
        let mut brain = Brain::new(ARCHETYPES[0], &v);
        if remembered {
            brain.mind.godlands.insert(
                "classic-world-02-reconstruction".into(),
                BTreeSet::from([tile(godlands)]),
            );
        }
        brain
            .crowd
            .bands
            .observe(&v, &c, &mut brain.mind.godlands, veteran, "Tester");
        let d = brain.decide(&v, &c).unwrap();
        assert_eq!(d.goal, goal, "remembered {remembered} veteran {veteran}");
        if goal == Goal::Venture {
            let step = d.movement.expect("heading for the Godlands");
            assert!(step.distance(godlands) < v.position.distance(godlands));
        }
    }
}

#[test]
fn unsellable_gear_leaves_the_vault_for_a_nexus_bag() {
    let (falchion, sword) = (0xa6e, 0xa47);
    let (c, mut v) = fixture();
    v.map.name = "Vault".into();
    v.map.template = Some("vault".into());
    v.objects[0].kind = 0x504;
    v.objects[0].position = v.position;
    v.objects[0].contents = vec![falchion, sword, -1, -1, -1, -1, -1, -1];
    let d = progression::bank(&v, &c, &Grid::terrain(&v), None, None)
        .unwrap()
        .unwrap();
    assert!(matches!(d.item, Some(Action::Swap { from, .. }) if from.kind == falchion));
    v.character.items[4] = sword;
    assert!(
        progression::stash(&v, &c, None).unwrap(),
        "priced gear is banked"
    );
    v.character.items[4] = falchion;
    assert!(!progression::stash(&v, &c, None).unwrap());
    v.map.name = "Nexus".into();
    v.map.template = Some("nexus".into());
    v.objects.clear();
    let d = Brain::new(ARCHETYPES[0], &v).decide(&v, &c).unwrap();
    assert!(matches!(d.item, Some(Action::Drop { slot }) if slot.kind == falchion));
}

#[test]
fn full_bag_does_not_leave_the_realm_to_bank_into_a_full_vault() {
    // Live: 59 of 70 level-20 banking trips came back with the same full bag, every five minutes.
    let sword = 0xa47;
    for full in [false, true] {
        let (c, mut v) = fixture();
        v.quest = None;
        v.objects.clear();
        v.character.items[4..].copy_from_slice(&[sword; 8]);
        let (_, mut vault) = fixture();
        vault.map.name = "Vault".into();
        vault.map.template = Some("vault".into());
        vault.objects[0].kind = 0x504;
        vault.objects[0].contents = vec![if full { sword } else { -1 }; 8];
        let mut brain = Brain::new(ARCHETYPES[0], &v);
        brain.mind.market.observe(&vault);
        let d = brain.decide(&v, &c).unwrap();
        assert!(
            matches!(d.journey, Some(Journey::Nexus))
                && (!full || brain.plan.owner() != Some(plan::Owner::Bank)),
            "full Vault {full}: {:?} {:?}",
            d.goal,
            brain.plan.owner()
        );
    }
}

#[test]
fn dodging_keeps_the_trip_and_still_makes_progress() {
    // Live: 35-40% of level-20 decisions were "Dodge", which replaced the trip to the Godlands and stalled it.
    let (c, mut v) = fixture();
    v.character.level = 20;
    v.quest = None;
    v.objects.clear();
    v.map.layout = Some("classic-world-02-reconstruction".into());
    let godlands = Pos { x: 100.5, y: 100.5 };
    v.map.godland_tiles = Arc::new(BTreeSet::from([tile(godlands)]));
    v.projectiles.push(crate::world::view::VisibleProjectile {
        source_kind: 0x654,
        already_hit_player: false,
        owner: 2,
        id: 1,
        start: Pos { x: 13.5, y: 10.5 },
        position: Pos { x: 11.5, y: 10.5 },
        angle: std::f32::consts::PI,
        speed: 40.,
        lifetime_ms: 2000.,
        amplitude: 0.,
        frequency: 0.,
        passes_cover: false,
        damage: 10,
        armor_piercing: false,
    });
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    brain.mind.godlands.insert(
        "classic-world-02-reconstruction".into(),
        BTreeSet::from([tile(godlands)]),
    );
    brain
        .crowd
        .bands
        .observe(&v, &c, &mut brain.mind.godlands, false, "Tester");
    let d = brain.decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Venture);
    let step = d.movement.expect("a dodging step");
    assert!(
        step.distance(godlands) < v.position.distance(godlands),
        "{step:?}"
    );
    assert!(
        combat::clearance(&v, step) > combat::clearance(&v, v.position),
        "{step:?}"
    );
}

#[test]
fn travel_toward_unseen_ground_heads_for_one_edge_and_gives_up_there() {
    // Live: about a dozen bots shuffled between two tiles for minutes while "heading to an observed band".
    let (_, v) = fixture();
    let geometry = crate::navigation::Geometry::new(
        (128, 128),
        vec![1; 128 * 128],
        &BTreeMap::from([(1, 1.)]),
    );
    let tiles = crate::navigation::Tiles::new(
        geometry,
        (0..=20).flat_map(|x| (0..128).map(move |y| (x, y))),
    );
    let mut v = v;
    v.tiles = Arc::new(tiles);
    let grid = Grid::terrain(&v);
    let target = Pos { x: 100.5, y: 50.5 };
    let steps: Vec<_> = [15.5, 16.5, 15.5]
        .into_iter()
        .map(|x| {
            grid.towards(Pos { x, y: 50.5 }, target)
                .expect("an edge ahead")
        })
        .collect();
    assert!(steps.iter().all(|s| s.x > 16.), "{steps:?}");
    assert!(grid.towards(Pos { x: 19.5, y: 50.5 }, target).is_none());
}

#[test]
fn a_hurt_bot_backs_off_only_near_enemies() {
    // Live: 3 of 8 bots "backing off to recover" were standing in the Nexus.
    for nexus in [false, true] {
        let (c, mut v) = fixture();
        v.quest = None;
        v.character.hp = v.character.stats[0] / 10;
        if nexus {
            v.map.name = "Nexus".into();
            v.map.template = Some("nexus".into());
            v.objects.clear();
        } else {
            v.objects[0].position = Pos { x: 14.5, y: 10.5 };
        }
        let d = Brain::new(ARCHETYPES[1], &v).decide(&v, &c).unwrap();
        assert_eq!(
            d.goal == Goal::Recover,
            !nexus,
            "nexus {nexus}: {:?}",
            d.goal
        );
    }
}

#[test]
fn travel_keeps_its_chosen_edge_until_another_is_much_nearer() {
    // Live snapshots: on open ground a trip's step flipped between waypoints NW and SW as tiles were revealed.
    let (_, mut v) = fixture();
    let grid = |reveal: &[(i16, i16)], v: &mut PlayerView| {
        let geometry = crate::navigation::Geometry::new(
            (128, 128),
            vec![1; 128 * 128],
            &BTreeMap::from([(1, 1.)]),
        );
        let base = (0..=10).flat_map(|x| (0..128).map(move |y| (x, y)));
        let prongs = (11..=20).flat_map(|x| [(x, 40), (x, 60)]);
        v.tiles = Arc::new(crate::navigation::Tiles::new(
            geometry,
            base.chain(prongs).chain(reveal.iter().copied()),
        ));
        Grid::terrain(v)
    };
    v.position = Pos { x: 10.5, y: 50.5 };
    let target = Pos { x: 100.5, y: 50.5 };
    let mut kept = None;
    let first = grid(&[], &mut v)
        .towards_kept(v.position, target, &mut kept)
        .unwrap();
    let second = grid(&[(21, 60)], &mut v)
        .towards_kept(v.position, target, &mut kept)
        .unwrap();
    assert_eq!(
        first.y > v.position.y,
        second.y > v.position.y,
        "{first:?} then {second:?}"
    );
    let other = if second.y > v.position.y { 40 } else { 60 };
    let longer: Vec<_> = (21..=40)
        .map(|x| (x, other))
        .chain((other == 40).then_some((21, 60)))
        .collect();
    let third = grid(&longer, &mut v)
        .towards_kept(v.position, target, &mut kept)
        .unwrap();
    assert_ne!(
        second.y > v.position.y,
        third.y > v.position.y,
        "a much nearer edge replaces the kept one: {second:?} then {third:?}"
    );
}

#[test]
fn a_venture_keeps_the_destination_it_started_with() {
    // Live snapshots: a bot crossing an 8-tile region edge re-chose its Venture destination and walked back.
    let (c, mut v) = fixture();
    v.character.level = 20;
    v.quest = None;
    v.objects.clear();
    v.position = Pos { x: 64.5, y: 64.5 };
    v.map.layout = Some("classic-world-02-reconstruction".into());
    let (west, east) = (Pos { x: 20.5, y: 64.5 }, Pos { x: 110.5, y: 64.5 });
    v.map.godland_tiles = Arc::new(BTreeSet::from([tile(west), tile(east)]));
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    brain.mind.godlands.insert(
        "classic-world-02-reconstruction".into(),
        BTreeSet::from([tile(west), tile(east)]),
    );
    brain
        .crowd
        .bands
        .observe(&v, &c, &mut brain.mind.godlands, false, "Tester");
    brain.goal = Goal::Venture;
    brain.travel = Some((Goal::Venture, u64::MAX, east));
    let d = brain.decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Venture);
    assert!(
        d.movement.is_some_and(|m| m.x > v.position.x),
        "{:?}",
        d.movement
    );
}

#[test]
fn realm_venture_crosses_the_band_edge_without_reversing_into_a_fight() {
    let (c, mut v) = fixture();
    v.character.level = 20;
    v.character.class = 0x307;
    v.character.items[..3].copy_from_slice(&[0xb02, -1, 0xaf9]);
    v.quest = None;
    v.position = center((30, 30));
    v.map.layout = Some("classic-world-05-reconstruction".into());
    let fringe = center((25, 26));
    let destination = center((17, 24));
    v.map.godland_tiles = Arc::new(BTreeSet::from([tile(fringe), tile(destination)]));
    v.map.godlands = Arc::new(vec![destination]);
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    brain
        .crowd
        .bands
        .observe(&v, &c, &mut brain.mind.godlands, true, "Tester");
    let first = brain.decide(&v, &c).unwrap();
    assert_eq!(first.goal, Goal::Venture);
    brain.goal = first.goal;

    // Kryz reached a thin Godlands fringe, turned back to an Ogre Mage, then restarted the trip.
    v.position = fringe;
    v.objects[0].kind = 0x642;
    v.objects[0].position = center((30, 27));
    v.tick += 5;
    v.time_ms += 250;
    let crossing = brain.decide(&v, &c).unwrap();
    assert_eq!(crossing.goal, Goal::Venture);
    assert!(crossing.movement.is_some_and(|p| p.x < v.position.x));
    assert!(crossing.aim.is_some_and(|p| p.x > v.position.x));

    v.position = destination;
    v.objects[0].position = center((20, 24));
    v.tick += 5;
    v.time_ms += 250;
    assert_eq!(brain.decide(&v, &c).unwrap().goal, Goal::Fight(2));
}

#[test]
fn a_godlands_fringe_fight_does_not_restart_an_arrived_venture() {
    let (c, mut v) = fixture();
    v.character.level = 20;
    v.quest = None;
    v.map.layout = Some("classic-world-05-reconstruction".into());
    let fringe = center((17, 10));
    v.map.godland_tiles = Arc::new(BTreeSet::from([tile(fringe)]));
    v.map.godlands = Arc::new(vec![fringe]);
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    brain
        .crowd
        .bands
        .observe(&v, &c, &mut brain.mind.godlands, true, "Bayleaf");
    let venture = brain.decide(&v, &c).unwrap();
    assert_eq!(venture.goal, Goal::Venture);
    brain.goal = venture.goal;

    v.position = fringe;
    v.objects[0].kind = 0x642;
    v.objects[0].position = center((15, 10));
    v.tick += 6;
    v.time_ms += 300;
    let fight = brain.decide(&v, &c).unwrap();
    assert_eq!(fight.goal, Goal::Fight(2));
    brain.goal = fight.goal;

    // Kiting across the fringe should not start the same trip back to it.
    v.position = center((16, 10));
    v.tick += 6;
    v.time_ms += 300;
    let fight = brain.decide(&v, &c).unwrap();
    assert_eq!(fight.goal, Goal::Fight(2));
    assert!(fight.aim.is_some());

    v.objects.clear();
    v.tick += 6;
    v.time_ms += 300;
    assert_eq!(brain.decide(&v, &c).unwrap().goal, Goal::Venture);
}

#[test]
fn a_venture_trip_ends_at_its_destination() {
    // Live snapshot: a bot about a tile from its kept Venture destination kept wandering around it.
    let (c, mut v) = fixture();
    v.character.level = 20;
    v.quest = None;
    v.objects.clear();
    v.position = Pos { x: 64.5, y: 64.5 };
    v.map.layout = Some("classic-world-02-reconstruction".into());
    let (reached, next) = (Pos { x: 65.5, y: 64.5 }, Pos { x: 110.5, y: 64.5 });
    v.map.godland_tiles = Arc::new(BTreeSet::from([tile(next)]));
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    brain.mind.godlands.insert(
        "classic-world-02-reconstruction".into(),
        BTreeSet::from([tile(next)]),
    );
    brain
        .crowd
        .bands
        .observe(&v, &c, &mut brain.mind.godlands, false, "Tester");
    brain.goal = Goal::Venture;
    brain.travel = Some((Goal::Venture, u64::MAX, reached));
    let d = brain.decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Venture);
    assert!(d.movement.is_some_and(|m| m.x > 66.), "{:?}", d.movement);
}

#[test]
fn level_20s_shoot_realm_event_bosses_and_back_off_out_of_reach() {
    let (c, mut v) = fixture();
    v.character.level = 20;
    v.character.stats[0] = 700;
    v.character.hp = 700;
    v.character.stats[3] = 25;
    v.objects[0].position = Pos { x: 10.5, y: 17.5 };
    v.objects.push(VisibleObject {
        id: 3,
        account: None,
        kind: 0xd5a,
        name: "Cube Overseer".into(),
        position: Pos { x: 13.5, y: 12.5 },
        hp: 1500,
        max_hp: 1500,
        level: None,
        equipped: vec![],
        contents: vec![],
        blocks: false,
        usable: false,
        condition_mask: Some(0),
    });
    // Cube God, Grand Sphinx, Lord of the Lost Lands.
    for (kind, hp) in [(0xd59, 10000), (0xd54, 55000), (0xd50, 70000)] {
        (v.objects[0].kind, v.objects[0].hp, v.objects[0].max_hp) = (kind, hp, hp);
        let d = Brain::new(ARCHETYPES[1], &v).decide(&v, &c).unwrap();
        assert_eq!(d.goal, Goal::Fight(2), "{kind:#x}");
        assert!(
            d.aim.unwrap().distance(v.objects[0].position) < 3.,
            "{kind:#x}"
        );
    }
    // Pentaract: the invincible core cannot be hurt, so its tower is the fight.
    v.objects[0].kind = 0xd5f;
    v.objects[0].condition_mask = Some(crate::condition::bit(crate::condition::INVINCIBLE));
    (v.objects[1].kind, v.objects[1].hp, v.objects[1].max_hp) = (0xd5e, 5000, 5000);
    let d = Brain::new(ARCHETYPES[1], &v).decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Fight(3));
    assert!(d.aim.unwrap().distance(v.objects[1].position) < 3.);
    v.objects[1].position = Pos { x: 10.5, y: 24.5 };
    let d = Brain::new(ARCHETYPES[1], &v).decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Hunt(3), "an out-of-range tower, not the core");
    (v.objects[0].kind, v.objects[0].condition_mask) = (0xd59, Some(0));
    // A wounded Knight out of sword reach backs off out of reach, holds while wounded, then returns.
    v.objects.truncate(1);
    v.character.class = 0x031e;
    v.character.items[..3].copy_from_slice(&[0xa82, -1, -1]);
    v.character.hp = 300;
    let mut brain = Brain::new(ARCHETYPES[1], &v);
    let step = |brain: &mut Brain, v: &mut PlayerView| {
        let d = brain.decide(v, &c).unwrap();
        brain.goal = d.goal.clone();
        (v.tick, v.time_ms, v.elapsed_ms) = (v.tick + 1, v.time_ms + 50, v.elapsed_ms + 50);
        d
    };
    for _ in 0..40 {
        let d = step(&mut brain, &mut v);
        assert_eq!(d.goal, Goal::Recover);
        let Some(m) = d.movement else { break };
        v.position = m;
    }
    assert!(v.position.distance(v.objects[0].position) > 15.);
    let held = step(&mut brain, &mut v);
    assert!(
        held.goal == Goal::Recover && held.movement.is_none(),
        "holds while wounded"
    );
    v.character.hp = 600;
    v.tick += 60;
    assert_ne!(step(&mut brain, &mut v).goal, Goal::Recover);
}

#[test]
fn godlands_farmer_keeps_going_past_a_snake_pit() {
    let (c, mut v) = fixture();
    let destination = (70, 70);
    v.quest = None;
    v.character.level = 20;
    v.map.layout = Some("layout".into());
    v.map.godlands = Arc::new(vec![center(destination)]);
    v.map.godland_tiles = Arc::new(BTreeSet::from([destination]));
    v.objects[0].kind = 0x718;
    v.objects[0].position = center((11, 10));
    v.objects[0].usable = true;
    assert!(progression::ready(&v, &c, 0x718));
    for (identity, enters) in [("Bayleaf", false), ("Gnosis", true)] {
        let mut profile = ARCHETYPES[0];
        profile.identity = identity;
        let mut brain = Brain::new(profile, &v);
        brain
            .crowd
            .bands
            .observe(&v, &c, &mut brain.mind.godlands, true, identity);
        let decision = brain.decide(&v, &c).unwrap();
        assert!(brain.mind.intentions.as_ref().unwrap().speed());
        if enters {
            assert!(matches!(decision.journey, Some(Journey::Portal(2))));
        } else {
            assert!(decision.journey.is_none());
            assert_eq!(decision.goal, Goal::Venture);
        }
    }
}

#[test]
fn nine_in_ten_level_20s_farm_the_godlands_in_ordinary_gear() {
    let (c, mut v) = fixture();
    v.character.level = 20;
    v.map.layout = Some("classic-world-02-reconstruction".into());
    let godlands = Pos { x: 100.5, y: 100.5 };
    v.map.godlands = Arc::new(vec![godlands]);
    v.map.godland_tiles = Arc::new(BTreeSet::from([tile(godlands)]));
    let staff = c.objects[&(v.character.items[0] as u16)].slot_type;
    v.character.items[0] = c
        .objects
        .values()
        .find(|o| o.slot_type == staff && o.tier == Some(3))
        .map(|o| i32::from(o.kind))
        .unwrap();
    let (mut hunters, mut farmers) = (0, 0);
    for profile in persona::PERSONAS.iter() {
        let mut brain = Brain::new(*profile, &v);
        brain
            .crowd
            .bands
            .observe(&v, &c, &mut BTreeMap::new(), true, profile.identity);
        farmers += usize::from(brain.crowd.bands.limit == 4);
        let hunts = quests::hunts_quests(profile);
        hunters += usize::from(hunts);
        let expected = if hunts { Goal::Hunt(2) } else { Goal::Venture };
        assert_eq!(
            brain.decide(&v, &c).unwrap().goal,
            expected,
            "{}",
            profile.name
        );
    }
    let share = hunters as f32 / persona::PERSONAS.len() as f32;
    assert!((0.08..0.12).contains(&share), "{hunters} quest hunters");
    let share = farmers as f32 / persona::PERSONAS.len() as f32;
    assert!(
        (0.85..0.95).contains(&share),
        "{farmers} Godlands farmers with a tier-3 staff"
    );
}

fn lava_chest() -> (Content, PlayerView) {
    let (c, mut v) = fixture();
    v.quest = None;
    v.character.level = 20;
    let chest_at = Pos { x: 60.5, y: 60.5 };
    let lava = |x: i16, y: i16| (58..=62).contains(&x) && (58..=62).contains(&y);
    let geometry = crate::navigation::Geometry::new(
        (128, 128),
        (0..128 * 128)
            .map(|i| {
                if lava((i % 128) as i16, (i / 128) as i16) {
                    0xf4
                } else {
                    1
                }
            })
            .collect(),
        &BTreeMap::from([(1, 1.), (0xf4, 0.666)]),
    );
    v.tiles = Arc::new(crate::navigation::Tiles::new(
        geometry,
        (0..128).flat_map(|x| (0..128).map(move |y| (x, y))),
    ));
    let chest = &mut v.objects[0];
    (chest.id, chest.kind, chest.hp, chest.position) = (3, 0x501, 0, chest_at);
    chest.contents = vec![0xa22, -1, -1, -1, -1, -1, -1, -1];
    v.objects.push(VisibleObject {
        id: 4,
        account: None,
        kind: 0x153,
        name: String::new(),
        position: chest_at,
        hp: 0,
        max_hp: 0,
        level: None,
        equipped: vec![],
        contents: vec![],
        blocks: false,
        usable: false,
        condition_mask: Some(0),
    });
    (c, v)
}

#[test]
fn healthy_bot_crosses_lava_and_picks_up_from_the_chest_at_both_distances() {
    let (c, mut v) = lava_chest();
    for distance in [30., 53.] {
        v.position = Pos {
            x: 60.5,
            y: 60.5 - distance,
        };
        v.character.hp = v.character.stats[0];
        let mut brain = Brain::new(ARCHETYPES[0], &v);
        let mut lava_entries = 0;
        let pickup = (0..55).find_map(|_| {
            v.tick += 5;
            v.time_ms += 250;
            v.elapsed_ms += 250;
            let d = brain.decide(&v, &c).unwrap();
            assert_eq!(d.goal, Goal::Loot(3), "{distance} tiles, {:?}", v.position);
            brain.goal = d.goal;
            if let Some(Action::Swap { from, .. }) = d.item {
                return Some((from.object, from.kind));
            }
            let next = d.movement.expect("move to the chest before picking up");
            if tile(next) != tile(v.position) && v.tiles.ground(tile(next)) == Some(0xf4) {
                lava_entries += 1;
                v.character.hp -= c.hazards[&0xf4].max;
            }
            v.position = next;
            None
        });
        assert_eq!(pickup, Some((3, 0xa22)));
        assert_eq!(lava_entries, 2);
        assert!(
            brain
                .navigation
                .as_ref()
                .unwrap()
                .walkable
                .contains(&(60, 60))
        );
        assert!(
            !brain
                .navigation
                .as_ref()
                .unwrap()
                .walkable
                .contains(&(61, 61))
        );
    }
}

#[test]
fn hurt_bot_sets_the_lava_chest_aside_at_both_distances() {
    let (c, mut v) = lava_chest();
    for distance in [30., 53.] {
        v.position = Pos {
            x: 60.5,
            y: 60.5 - distance,
        };
        v.character.hp = 340;
        let mut brain = Brain::new(ARCHETYPES[0], &v);
        let d = brain.decide(&v, &c).unwrap();
        assert_ne!(d.goal, Goal::Loot(3));
        assert!(d.item.is_none());
        assert_eq!(brain.unreachable.get(&3), Some(&(v.time_ms + 300_000)));
        v.character.hp = 500;
        v.tick += 200;
        v.time_ms += 10_000;
        assert_ne!(brain.decide(&v, &c).unwrap().goal, Goal::Loot(3));
    }
}

#[test]
fn lava_reflex_resumes_after_crossing_is_interrupted_and_respects_visible_protection() {
    let (c, mut v) = lava_chest();
    for loses_health in [true, false] {
        v.position = Pos { x: 60.5, y: 57.5 };
        v.character.hp = 500;
        let mut brain = Brain::new(ARCHETYPES[0], &v);
        let first = brain.decide(&v, &c).unwrap();
        assert_eq!(first.goal, Goal::Loot(3));
        brain.goal = first.goal;
        v.position = first.movement.expect("step into lava");
        let chest = if loses_health {
            v.character.hp = 249;
            None
        } else {
            Some(v.objects.remove(0))
        };
        v.tick += 5;
        assert_eq!(brain.decide(&v, &c).unwrap().goal, Goal::Dodge);
        if let Some(chest) = chest {
            v.objects.insert(0, chest);
        }
    }
    v.position = v.objects[0].position;
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    assert_ne!(brain.decide(&v, &c).unwrap().goal, Goal::Dodge);
    v.objects.pop();
    v.tick += 5;
    assert_eq!(brain.decide(&v, &c).unwrap().goal, Goal::Dodge);
}

#[test]
fn snake_pit_cover_attempt_expires_across_interrupting_hunts() {
    let (c, mut v) = fixture();
    v.map.name = "Snake Pit".into();
    v.map.template = Some("snake-pit".into());
    v.objects[0].kind = 0x917;
    v.objects[0].condition_mask = Some(crate::condition::bit(crate::condition::INVULNERABLE));
    let (blocked_quest, reachable_quest) = (center((24, 10)), center((23, 10)));
    let mut grounds = vec![1; 128 * 128];
    grounds[10 * 128 + 24] = 255;
    v.tiles = Arc::new(crate::navigation::Tiles::new(
        crate::navigation::Geometry::new((128, 128), grounds, &c.grounds),
        (0..128).flat_map(|x| (0..128).map(move |y| (x, y))),
    ));
    v.objects.push(VisibleObject {
        id: 7,
        kind: 0x172,
        name: String::new(),
        position: center((10, 13)),
        hp: 100,
        max_hp: 100,
        equipped: vec![],
        contents: vec![],
        blocks: true,
        condition_mask: Some(0),
        ..v.objects[0]
    });
    let mut brain = Brain::new(ARCHETYPES[1], &v);
    for step in 0..22 {
        let (position, goal) = if step % 2 == 0 {
            (blocked_quest, Goal::Break(7))
        } else {
            (reachable_quest, Goal::Hunt(2))
        };
        v.objects[0].position = position;
        let decision = adventure(&mut brain, &v, &c);
        assert_eq!(decision.goal, goal, "at second {step}");
        brain.goal = decision.goal;
        v.time_ms += 1_000;
        v.elapsed_ms += 1_000;
        v.tick += 20;
    }
    v.objects[0].position = blocked_quest;
    assert_ne!(adventure(&mut brain, &v, &c).goal, Goal::Break(7));
    assert!(brain.unreachable.contains_key(&7));
}

#[test]
fn cover_attempt_expires_only_while_the_wall_takes_no_damage() {
    let (_, mut v) = fixture();
    v.objects.push(VisibleObject {
        id: 7,
        account: None,
        kind: 0x172,
        name: String::new(),
        position: Pos { x: 12.5, y: 10.5 },
        hp: 100,
        max_hp: 100,
        level: None,
        equipped: vec![],
        contents: vec![],
        blocks: true,
        usable: false,
        condition_mask: Some(0),
    });
    let wall = v.objects.len() - 1;
    let mut brain = Brain::new(ARCHETYPES[1], &v);
    brain.goal = Goal::Break(7);
    brain.expire_cover_attempt(&v);
    v.time_ms += 19_000;
    v.objects[wall].hp = 90;
    brain.expire_cover_attempt(&v);
    v.time_ms += 19_000;
    brain.expire_cover_attempt(&v);
    assert!(
        !brain.unreachable.contains_key(&7),
        "damage refreshes the attempt"
    );
    v.time_ms += 1_000;
    brain.expire_cover_attempt(&v);
    assert!(
        brain.unreachable.contains_key(&7),
        "an undamaged wall is set aside"
    );
}

#[test]
fn stationary_enemy_shots_are_avoided_not_fatal() {
    let (c, mut v) = fixture();
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    let hazard = center((11, 10));
    v.projectiles.push(crate::world::view::VisibleProjectile {
        source_kind: 0xe38,
        already_hit_player: false,
        owner: 2,
        id: 1,
        start: hazard,
        position: hazard,
        angle: 0.,
        speed: 0.,
        lifetime_ms: 5000.,
        amplitude: 0.,
        frequency: 0.,
        passes_cover: false,
        damage: 20,
        armor_piercing: false,
    });
    brain.decide(&v, &c).unwrap();
    assert!(combat::predicted(&v.projectiles[0], 4.).is_some_and(|p| p.distance(hazard) == 0.));
}
