use super::*;
use crate::world::view::{PlayerMarker, VisibleObject};

#[test]
fn constructed_combat_travel_keeps_a_direction_instead_of_reversing() {
    for profile in ARCHETYPES {
        let (content, mut view) = quest_tests::fixture();
        view.character.level = 20;
        view.character.mp = 0;
        view.position = Pos { x: 60.5, y: 60.5 };
        let projectile = &content.objects[&(view.character.items[0] as u16)].projectiles[&0];
        let range = projectile.speed * projectile.lifetime / 10000.;
        view.objects[0].position = Pos {
            x: view.position.x + range * 0.8,
            y: view.position.y,
        };
        let mut brain = Brain::new(profile, &view);
        let mut previous = None;
        let mut reversals = 0;
        let mut travelled = 0.;
        for _ in 0..60 {
            let decision = brain.decide(&view, &content).unwrap();
            assert!(
                matches!(decision.goal, Goal::Fight(2) | Goal::Hunt(2)),
                "{}: {:?}",
                profile.name,
                decision.goal
            );
            assert!(decision.journey.is_none());
            let target = decision.movement.expect("ordinary combat movement");
            let length = view.position.distance(target);
            assert!(length > 0.);
            let direction = Pos {
                x: (target.x - view.position.x) / length,
                y: (target.y - view.position.y) / length,
            };
            if let Some(prior) = previous {
                let prior: Pos = prior;
                reversals += usize::from(prior.x * direction.x + prior.y * direction.y < -0.5);
            }
            previous = Some(direction);
            let step = length.min(combat::speed(&view) * profile.reaction as f32 * 0.05);
            view.position.x += direction.x * step;
            view.position.y += direction.y * step;
            travelled += step;
            brain.goal = decision.goal;
            brain.destination = decision.movement;
            view.tick += profile.reaction;
            view.time_ms += profile.reaction * 50;
            view.elapsed_ms += profile.reaction * 50;
        }
        assert!(travelled > 20.);
        assert!(
            reversals <= 1,
            "{} reversed {reversals} times during unthreatened constructed travel",
            profile.name
        );
    }
}

#[test]
fn constructed_recovery_bypasses_retained_combat_steering() {
    let (content, mut view) = quest_tests::fixture();
    view.character.level = 20;
    view.character.mp = 0;
    view.position = Pos { x: 60.5, y: 60.5 };
    view.objects[0].position = Pos { x: 64.5, y: 60.5 };
    view.character.hp = (view.character.stats[0] as f32 * ARCHETYPES[0].recovery * 0.75) as i32;
    let mut decisions = Vec::new();
    for heading in [0., std::f32::consts::PI] {
        let mut brain = Brain::new(ARCHETYPES[0], &view);
        brain.steering = Some((2, heading));
        let decision = brain.decide(&view, &content).unwrap();
        assert_eq!(decision.goal, Goal::Recover);
        assert!(decision.journey.is_none());
        assert!(brain.steering.is_none());
        decisions.push(decision.movement.expect("ordinary safe recovery backstep"));
    }
    assert!(decisions[0].distance(decisions[1]) < 0.001);
    assert!(
        decisions[0].distance(view.objects[0].position)
            > view.position.distance(view.objects[0].position)
    );
}

#[test]
fn constructed_held_steering_respects_walls_and_paralysis() {
    let (_, mut view) = quest_tests::fixture();
    view.position = Pos { x: 60.5, y: 60.5 };
    view.objects[0].position = Pos { x: 64.5, y: 60.5 };
    let mut grid = Grid::terrain(&view);
    for y in 58..=62 {
        grid.walkable.remove(&(61, y));
    }
    let perception = combat::Perception::attend(&view, &pursuits::Cognition::default(), false);
    let steering = Some(combat::Steering {
        heading: 0.,
        distance: 2.5,
        clearance: 0.7,
    });
    let next = combat::movement(
        &view,
        &grid,
        &[&view.objects[0]],
        Some((view.objects[0].position, 5.)),
        false,
        &perception,
        steering,
    )
    .expect("safe direction along the wall");
    assert!(grid.segment(view.position, next));
    assert!(next.x < 61.);
    view.character.conditions |= crate::content::PARALYZED;
    assert!(
        combat::movement(
            &view,
            &grid,
            &[&view.objects[0]],
            Some((view.objects[0].position, 5.)),
            false,
            &perception,
            steering
        )
        .is_none()
    );
    assert_eq!(combat::speed(&view), 0.);
}

fn scene(level: i32) -> (Content, PlayerView) {
    let (c, mut v) = quest_tests::fixture();
    v.character.level = level;
    v.character.base_stats[4] = c.classes[&v.character.class].caps[4];
    v.character.items[4..6].fill(0xa22);
    v.quest = None;
    v.objects.clear();
    (c, v)
}

fn peer(v: &mut PlayerView, id: i32, position: Pos) {
    v.player_markers.push(PlayerMarker {
        id,
        account: id,
        name: format!("peer{id}"),
        position,
    });
    v.objects.push(VisibleObject {
        id,
        account: Some(id),
        kind: crate::content::WIZARD,
        name: format!("peer{id}"),
        position,
        hp: 500,
        max_hp: 500,
        level: Some(20),
        equipped: vec![],
        contents: vec![],
        blocks: false,
        usable: false,
        condition_mask: Some(0),
    });
}

#[test]
fn constructed_three_brain_arrival_resumes_independent_routes() {
    let separated = [
        Pos { x: 10.5, y: 10.5 },
        Pos { x: 19., y: 10.5 },
        Pos { x: 15., y: 18. },
    ];
    let arrived = [
        Pos { x: 10.5, y: 10.5 },
        Pos { x: 14., y: 10.5 },
        Pos { x: 12., y: 13.5 },
    ];
    for account in 1..=3 {
        let (c, mut v) = scene(5);
        v.session.account = account;
        v.session.player = account;
        let target = account % 3 + 1;
        v.position = separated[(account - 1) as usize];
        peer(&mut v, target, separated[(target - 1) as usize]);
        let mut b = Brain::new(ARCHETYPES[0], &v);
        let d = b.decide(&v, &c).unwrap();
        assert_eq!(d.goal, Goal::Follow(target));
        assert!(d.movement.is_some());
        b.goal = d.goal;
        v.position = arrived[(account - 1) as usize];
        v.player_markers[0].position = arrived[(target - 1) as usize];
        v.objects[0].position = arrived[(target - 1) as usize];
        v.time_ms += 500;
        v.elapsed_ms += 500;
        v.tick += 10;
        let d = b.decide(&v, &c).unwrap();
        assert!(
            !matches!(d.goal, Goal::Follow(_)),
            "idle chain member {account}"
        );
        assert!(d.movement.is_some(), "an ordinary independent route exists");
    }
}

#[test]
fn passive_company_yields_to_suitable_distant_quest() {
    let (c, mut v) = scene(20);
    peer(&mut v, 3, Pos { x: 19., y: 10.5 });
    let mut b = Brain::new(ARCHETYPES[0], &v);
    b.inland_reached = true;
    let d = b.decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Follow(3));
    b.goal = d.goal;
    let enemy = quest_tests::fixture().1.objects.remove(0);
    v.quest = Some(enemy.id);
    v.objects.push(enemy);
    v.tick += 10;
    v.time_ms += 500;
    v.elapsed_ms += 500;
    let d = b.decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Hunt(2));
    assert!(d.movement.unwrap().y > v.position.y);
}

#[test]
fn uncommitted_company_is_bounded_for_leveling_and_fame_not_only_speed() {
    for level in [5, 20] {
        let (c, mut v) = scene(level);
        peer(&mut v, 3, Pos { x: 19., y: 10.5 });
        let mut b = Brain::new(ARCHETYPES[0], &v);
        b.inland_reached = true;
        assert_eq!(b.decide(&v, &c).unwrap().goal, Goal::Follow(3));
        assert_eq!(
            b.intent["mode"], "optional",
            "L{level} company must not count as progress"
        );
        v.time_ms += 90_001;
        v.elapsed_ms += 90_001;
        v.tick += 1800;
        assert!(
            !matches!(b.decide(&v, &c).unwrap().goal, Goal::Follow(_)),
            "L{level}"
        );
    }
}

#[test]
fn explicit_follow_and_useful_loot_keep_their_priorities() {
    let (c, mut v) = scene(5);
    peer(&mut v, 3, Pos { x: 19., y: 10.5 });
    let mut b = Brain::new(ARCHETYPES[0], &v);
    b.companion = Some(3);
    b.invitation_until = v.time_ms + 120_000;
    assert_eq!(b.decide(&v, &c).unwrap().goal, Goal::Follow(3));
    let mut bag = quest_tests::fixture().1.objects.remove(0);
    bag.id = 4;
    bag.kind = 0x500;
    bag.account = None;
    bag.contents = vec![0xa22];
    bag.position = v.position;
    v.character.items[4..6].fill(-1);
    v.objects.push(bag);
    assert_eq!(
        Brain::new(ARCHETYPES[0], &v).decide(&v, &c).unwrap().goal,
        Goal::Loot(4)
    );
}

#[test]
fn equipped_twenty_leaves_passive_company_for_published_realm_event() {
    let (c, mut v) = scene(20);
    peer(&mut v, 3, Pos { x: 19., y: 10.5 });
    let mut b = Brain::new(ARCHETYPES[0], &v);
    b.inland_reached = true;
    let d = b.decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Follow(3));
    b.goal = d.goal;
    let mut event = quest_tests::fixture().1.objects.remove(0);
    event.id = 4;
    event.kind = 0xd56;
    event.name = "Skull Shrine".into();
    event.max_hp = c.objects[&event.kind].hp;
    event.hp = event.max_hp;
    assert!(events::ready(&v, &c));
    assert!(progression::can_fight(&v, &c, &event));
    v.quest = Some(event.id);
    v.objects.push(event);
    v.tick += 10;
    v.time_ms += 500;
    v.elapsed_ms += 500;
    let d = b.decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Hunt(4));
    assert!(d.movement.unwrap().y > v.position.y);
    assert!(d.journey.is_none(), "no blind event teleport");
    v.character.hp = 50;
    v.character.items[4..6].fill(-1);
    v.tick += 10;
    v.time_ms += 500;
    v.elapsed_ms += 500;
    assert_eq!(b.decide(&v, &c).unwrap().goal, Goal::Recover);
}

fn hunting_scene(level: i32, known: bool) -> (Content, PlayerView) {
    let (c, mut v) = quest_tests::fixture();
    v.character.level = level;
    v.character.items[4..6].fill(0xa22);
    v.quest = None;
    v.objects.clear();
    let mut ground = vec![1; 128 * 128];
    for y in 18..30 {
        for x in 18..30 {
            ground[y * 128 + x] = 0x6a;
        }
    }
    let geometry = crate::navigation::Geometry::new(
        (128, 128),
        ground,
        &BTreeMap::from([(1, 1.), (0x6a, 1.)]),
    );
    v.tiles = Arc::new(crate::navigation::Tiles::new(
        geometry,
        (0..128)
            .flat_map(|x| (0..128).map(move |y| (x, y)))
            .filter(|(x, y)| known || !(18..30).contains(x) || !(18..30).contains(y)),
    ));
    (c, v)
}

#[test]
fn equipped_twenty_returns_to_known_godlands_after_first_inland_trip() {
    let (c, v) = hunting_scene(20, true);
    for profile in ARCHETYPES {
        let mut b = Brain::new(profile, &v);
        b.inland_reached = true;
        let d = b.decide(&v, &c).unwrap();
        assert_eq!(d.goal, Goal::Venture, "{}", profile.name);
        let p = d.movement.expect("ordinary known-terrain route");
        assert!(p.x > v.position.x || p.y > v.position.y);
        assert!(d.journey.is_none());
    }
}

#[test]
fn godlands_preference_requires_own_readiness_and_discovery() {
    for variant in 0..5 {
        let (c, mut v) = hunting_scene(if variant == 0 { 10 } else { 20 }, variant != 4);
        match variant {
            1 => v.character.items[0] = 0xa97,
            2 => {
                v.character.hp = 100;
                v.character.items[4..6].fill(-1);
            }
            3 => v.character.stats[3] = 0,
            _ => {}
        }
        let mut b = Brain::new(ARCHETYPES[0], &v);
        b.inland_reached = true;
        let d = b.decide(&v, &c).unwrap();
        assert_ne!(d.goal, Goal::Venture, "variant{variant}");
        if variant == 2 {
            assert_eq!(d.goal, Goal::Recover);
        }
    }
}

#[test]
fn known_godlands_does_not_override_close_combat_or_supplies() {
    let (c, mut v) = hunting_scene(20, true);
    let mut enemy = quest_tests::fixture().1.objects.remove(0);
    enemy.position = Pos { x: 14.5, y: 10.5 };
    let enemy_id = enemy.id;
    v.objects.push(enemy);
    let mut b = Brain::new(ARCHETYPES[0], &v);
    b.inland_reached = true;
    assert_eq!(b.decide(&v, &c).unwrap().goal, Goal::Fight(enemy_id));
    v.objects[0].kind = 0x500;
    v.objects[0].contents = vec![0xa22];
    v.objects[0].position = v.position;
    v.character.items[4..6].fill(-1);
    let mut b = Brain::new(ARCHETYPES[0], &v);
    b.inland_reached = true;
    assert_eq!(b.decide(&v, &c).unwrap().goal, Goal::Loot(enemy_id));
}

#[test]
fn prepared_hunter_prefers_observed_suitable_god_over_empty_ground() {
    let (c, mut v) = hunting_scene(20, true);
    let mut god = quest_tests::fixture().1.objects.remove(0);
    god.kind = 0x651;
    god.max_hp = c.objects[&god.kind].hp;
    god.hp = god.max_hp;
    god.position = Pos { x: 35.5, y: 10.5 };
    assert!(progression::can_fight(&v, &c, &god));
    v.objects.push(god);
    let mut b = Brain::new(ARCHETYPES[0], &v);
    b.inland_reached = true;
    let d = b.decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Hunt(2));
    assert!(d.movement.unwrap().x > v.position.x);
    assert!(d.journey.is_none());
}

#[test]
fn constructed_godlands_route_keeps_progressing_into_discovered_hunting_terrain() {
    let (c, mut v) = hunting_scene(20, true);
    let mut b = Brain::new(ARCHETYPES[0], &v);
    b.inland_reached = true;
    for _ in 0..80 {
        let d = b.decide(&v, &c).unwrap();
        assert_eq!(d.goal, Goal::Venture);
        let p = d.movement.expect("known hunting route stays executable");
        let scale = 0.5 / p.distance(v.position).max(0.5);
        v.position.x += (p.x - v.position.x) * scale;
        v.position.y += (p.y - v.position.y) * scale;
        b.goal = d.goal;
        b.destination = d.movement;
        v.time_ms += 500;
        v.elapsed_ms += 500;
        v.tick += 10;
    }
    assert!((18.0..30.0).contains(&v.position.x));
    assert!((18.0..30.0).contains(&v.position.y));
}

#[test]
fn genuine_shared_damage_company_can_travel_then_yield_on_arrival() {
    let (c, mut v) = hunting_scene(20, true);
    peer(&mut v, 3, Pos { x: 19., y: 10.5 });
    v.objects.push(quest_tests::fixture().1.objects.remove(0));
    let mut b = Brain::new(ARCHETYPES[0], &v);
    b.inland_reached = true;
    b.crowd.observe(&v, &c);
    for shooter in [v.session.player, 3] {
        let mut w = wire::Writer::new();
        w.i32(2)
            .u8(0)
            .i16(12)
            .u8(u8::from(shooter == 3))
            .u8(0)
            .i32(shooter);
        b.crowd
            .receive(&Packet { id: 47, body: w.0 }, &v, &c)
            .unwrap();
    }
    v.objects.retain(|p| p.id != 2);
    let d = b.decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Follow(3));
    assert!(d.movement.is_some());
    assert!(d.journey.is_none());
    b.goal = d.goal;
    v.objects[0].position.x = 14.;
    v.player_markers[0].position.x = 14.;
    v.tick += 10;
    v.time_ms += 500;
    v.elapsed_ms += 500;
    let d = b.decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Venture);
    assert!(d.movement.is_some());
}

fn godlands_corridor(open: bool) -> Arc<crate::navigation::Tiles> {
    let mut ground = vec![1; 128 * 128];
    for y in 18..30 {
        for x in 18..30 {
            ground[y * 128 + x] = 0x6a;
        }
    }
    for i in 17..31 {
        for (x, y) in [(i, 17), (i, 30), (17, i), (30, i)] {
            ground[y * 128 + x] = 255;
        }
    }
    if open {
        ground[18 * 128 + 17] = 1;
    }
    let geometry = crate::navigation::Geometry::new(
        (128, 128),
        ground,
        &BTreeMap::from([(1, 1.), (0x6a, 1.), (255, 0.)]),
    );
    Arc::new(crate::navigation::Tiles::new(
        geometry,
        (0..128).flat_map(|x| (0..128).map(move |y| (x, y))),
    ))
}

#[test]
fn isolated_known_godlands_does_not_suppress_reachable_company() {
    let (c, mut v) = hunting_scene(20, true);
    v.tiles = godlands_corridor(false);
    peer(&mut v, 3, Pos { x: 2.5, y: 10.5 });
    let mut b = Brain::new(ARCHETYPES[0], &v);
    b.inland_reached = true;
    let d = b.decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Follow(3));
    assert!(d.movement.unwrap().x < v.position.x);
}

#[test]
fn cached_godlands_revalidates_navigation_before_vetoing_reachable_company() {
    for obstruction in ["terrain", "cover", "enemy"] {
        let (c, mut v) = hunting_scene(20, true);
        v.tiles = godlands_corridor(true);
        let mut b = Brain::new(ARCHETYPES[0], &v);
        b.inland_reached = true;
        let first = b.decide(&v, &c).unwrap();
        assert_eq!(first.goal, Goal::Venture);
        let cached = b
            .godlands
            .expect("first decision caches reachable hunting ground");
        assert!(
            b.navigation
                .as_ref()
                .unwrap()
                .reachable(v.position)
                .contains(&tile(cached))
        );
        b.goal = first.goal;
        b.destination = first.movement;
        match obstruction {
            "terrain" => v.tiles = godlands_corridor(false),
            "cover" | "enemy" => {
                let mut obstacle = quest_tests::fixture().1.objects.remove(0);
                obstacle.id = 4;
                obstacle.position = Pos { x: 17.5, y: 18.5 };
                if obstruction == "cover" {
                    obstacle.kind = c
                        .objects
                        .values()
                        .find(|o| o.blocks && !o.enemy)
                        .unwrap()
                        .kind;
                    obstacle.blocks = true;
                } else {
                    obstacle.kind = 0x91f;
                    obstacle.max_hp = 300000;
                    assert!(!progression::can_fight(&v, &c, &obstacle));
                }
                v.objects.push(obstacle);
            }
            _ => unreachable!(),
        }
        peer(&mut v, 3, Pos { x: 2.5, y: 10.5 });
        v.tick += 10;
        v.time_ms += 500;
        v.elapsed_ms += 500;
        let next = b.decide(&v, &c).unwrap();
        assert_eq!(
            next.goal,
            Goal::Follow(3),
            "{obstruction} disconnects the retained destination"
        );
        assert!(next.movement.unwrap().x < v.position.x);
        assert!(next.journey.is_none());
        assert!(b.godlands.is_none());
    }
}

#[test]
fn cached_godlands_controls_class_resources_and_equipped_rookie_risk() {
    for class in [crate::content::WIZARD, crate::content::PRIEST] {
        let (c, mut v) = hunting_scene(20, true);
        v.character.class = class;
        let slot = c.objects[&class].slots[0];
        let weapon = c
            .objects
            .values()
            .find(|o| o.slot_type == slot && o.tier == Some(7) && !o.projectiles.is_empty())
            .unwrap()
            .kind;
        v.character.items.fill(-1);
        v.character.items[0] = i32::from(weapon);
        v.character.mp = 0;
        assert!(!events::ready(&v, &c));
        let mut b = Brain::new(ARCHETYPES[0], &v);
        b.inland_reached = true;
        assert_ne!(b.decide(&v, &c).unwrap().goal, Goal::Venture);
        v.character.mp = v.character.stats[1];
        let mut b = Brain::new(ARCHETYPES[0], &v);
        b.inland_reached = true;
        assert_eq!(b.decide(&v, &c).unwrap().goal, Goal::Venture);
    }
    let (c, mut v) = hunting_scene(20, true);
    v.character.hp = 400;
    let rookie = Profile {
        stars: 0,
        attention: 1,
        reaction: 20,
        recovery: 0.9,
        knows_minimap: false,
        ..ARCHETYPES[0]
    };
    assert!(events::ready(&v, &c));
    let mut b = Brain::new(rookie, &v);
    b.inland_reached = true;
    assert_eq!(b.decide(&v, &c).unwrap().goal, Goal::Recover);
    let mut b = Brain::new(ARCHETYPES[1], &v);
    b.inland_reached = true;
    assert_eq!(b.decide(&v, &c).unwrap().goal, Goal::Venture);
}
