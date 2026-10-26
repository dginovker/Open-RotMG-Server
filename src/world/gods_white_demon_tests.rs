use super::*;

async fn arena(name: &str) -> (World, i32, i32) {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let store = crate::store::Store::open(std::path::Path::new(":memory:"), &content).unwrap();
    let account = store.list("white-demon-combat".into()).await.unwrap();
    let character = store
        .create(account.id, content.get(crate::content::WIZARD).unwrap())
        .await
        .unwrap();
    let map = Map {
        width: 64,
        height: 64,
        cells: vec![
            crate::content::Cell {
                ground: 0x61,
                object: None,
                config: String::new(),
                terrain: 0,
                region: 0,
            };
            64 * 64
        ],
        spawns: vec![(32.5, 24.5)],
    };
    let mut world = World::new(content, Arc::new(map)).unwrap();
    world.name = name.into();
    world.encounter = true;
    world.safe = false;
    world.rng = rand::rngs::StdRng::seed_from_u64(651);
    let player = world.enter(character, None).unwrap();
    let demon = world
        .spawn_entity(0x651, Pos { x: 32.5, y: 32.5 }, vec![])
        .unwrap();
    (world, player, demon)
}

fn step(world: &mut World, now: u64) -> Vec<Bullet> {
    world.now = now;
    world.npc_step();
    std::mem::take(&mut world.bullets)
}

fn assert_fan(shots: &[Bullet], heading: f32, gap: f32) {
    assert_eq!(shots.len(), 3);
    for (shot, offset) in shots.iter().zip([-gap, 0., gap]) {
        assert!((shot.angle - heading - offset.to_radians()).abs() < 1e-5);
        assert_eq!(shot.source_kind, 0x651);
        assert_eq!(shot.damage, 45);
        assert_eq!(shot.spec.speed, 50.);
        assert_eq!(shot.spec.lifetime, 2000.);
        assert!(shot.spec.armor_piercing);
        assert!(shot.spec.multi_hit);
    }
}

#[tokio::test]
async fn white_demon_immediate_shot_then_quantized_cooldown_and_twenty_degree_fan() {
    let (mut world, _, demon) = arena("Realm").await;
    world
        .apply_condition(demon, condition::PARALYZED, 10_000)
        .unwrap();
    let mut births = vec![];
    for now in (50..=2250).step_by(50) {
        let shots = step(&mut world, now);
        if !shots.is_empty() {
            assert_fan(&shots, -std::f32::consts::FRAC_PI_2, 20.);
            assert!(shots.iter().all(|b| b.owner == demon));
            births.push(now);
        }
    }
    assert_eq!(births, [50, 600, 1150, 1700, 2250]);
}

#[tokio::test]
async fn white_demon_strict_shoot_radius_consumes_empty_attempts_without_reentry_reset() {
    let (mut world, player, demon) = arena("Realm").await;
    world
        .apply_condition(demon, condition::PARALYZED, 10_000)
        .unwrap();
    for now in (50..=1150).step_by(50) {
        let distance = if now == 50 || now == 600 { 10. } else { 9.99 };
        world.players.get_mut(&player).unwrap().pos.y = 32.5 - distance;
        let shots = step(&mut world, now);
        assert_eq!(shots.len(), if now == 1150 { 3 } else { 0 }, "{now}");
    }
    let activated = world.entities[&demon].activated;
    world.players.get_mut(&player).unwrap().pos.y = 17.5;
    for now in (1200..=1700).step_by(50) {
        assert!(step(&mut world, now).is_empty());
    }
    world.players.get_mut(&player).unwrap().pos.y = 24.5;
    assert!(step(&mut world, 1750).is_empty());
    for now in (1800..=2250).step_by(50) {
        assert_eq!(step(&mut world, now).len(), if now == 2250 { 3 } else { 0 });
    }
    assert_eq!(world.entities[&demon].activated, activated);
}

#[tokio::test]
async fn white_demon_stun_holds_a_ready_shot_but_does_not_restart_cooldown() {
    let (mut world, _, demon) = arena("Realm").await;
    world
        .apply_condition(demon, condition::PARALYZED, 10_000)
        .unwrap();
    assert_eq!(step(&mut world, 50).len(), 3);
    world
        .apply_condition(demon, condition::STUNNED, 950)
        .unwrap();
    for now in (100..1000).step_by(50) {
        assert!(step(&mut world, now).is_empty());
    }
    assert_eq!(step(&mut world, 1000).len(), 3);
    for now in (1050..=1550).step_by(50) {
        assert_eq!(step(&mut world, now).len(), if now == 1550 { 3 } else { 0 });
    }
}

#[tokio::test]
async fn white_demon_follow_acquires_below_ten_stops_at_seven_and_resumes_above_eight() {
    let (mut world, _, demon) = arena("Realm").await;
    let motion = world.god_movement(demon, None, 0).unwrap();
    assert!(matches!(
        motion,
        (
            Motion::Follow {
                sight: 10.,
                stop: 7.,
                duration: 0,
                cooldown: 0,
                ..
            },
            _
        )
    ));
    assert!((motion.1 - encounter::nr_speed(1.)).abs() < 1e-5);
    let origin = world.entities[&demon].pos;
    for (distance, acquired) in [
        (10., false),
        (9.99, true),
        (7., false),
        (8., false),
        (8.01, true),
        (10., false),
        (8., true),
    ] {
        let target = Pos {
            x: origin.x + distance,
            y: origin.y,
        };
        assert_eq!(
            world
                .follow_goal(demon, Some(target), motion.0, 50)
                .is_some(),
            acquired,
            "{distance}"
        );
    }
    let start = world.entities[&demon].pos;
    assert!(!step(&mut world, 50).is_empty());
    assert!(world.entities[&demon].pos.distance(start) > 0.);
    world
        .apply_condition(demon, condition::PARALYZED, 1000)
        .unwrap();
    let stopped = world.entities[&demon].pos;
    step(&mut world, 100);
    assert_eq!(world.entities[&demon].pos.distance(stopped), 0.);
    world.entities.get_mut(&demon).unwrap().pos.x = origin.x + 25.;
    assert!(matches!(
        world.god_movement(demon, None, 0).unwrap().0,
        Motion::Protect {
            anchor: Anchor::Home,
            distance: 1.
        }
    ));
}

#[tokio::test]
async fn white_demon_aim_uses_compiled_angular_history_not_linear_interception() {
    let (mut world, player, demon) = arena("Realm").await;
    world
        .apply_condition(demon, condition::PARALYZED, 10_000)
        .unwrap();
    for now in (50..=1150).step_by(50) {
        let x = if now <= 600 {
            now as f32 / 1000.
        } else {
            0.6 - (now - 600) as f32 / 1000.
        };
        world.players.get_mut(&player).unwrap().pos.x = 32.5 + x;
        let shots = step(&mut world, now);
        if now == 600 || now == 1150 {
            let heading = (-8f32).atan2(x);
            let old_heading = (-8f32).atan2(if now == 600 { 0.55 } else { 0.1 });
            let predicted = heading + (heading - old_heading) / 0.1 * 0.5;
            assert_fan(&shots, predicted, 20.);
        }
    }
}

#[tokio::test]
async fn white_demon_abyss_retains_its_independent_fifteen_degree_fan_and_clock() {
    let (mut world, player, demon) = arena("Abyss of Demons").await;
    assert!(matches!(
        world
            .god_movement(demon, Some(world.players[&player].pos), 0)
            .unwrap(),
        (
            Motion::Chase {
                sight: 9.,
                stop: 4.
            },
            6.
        )
    ));
    world
        .apply_condition(demon, condition::PARALYZED, 10_000)
        .unwrap();
    let mut births = vec![];
    for now in (50..=1050).step_by(50) {
        let shots = step(&mut world, now);
        if !shots.is_empty() {
            assert_fan(&shots, -std::f32::consts::FRAC_PI_2, 15.);
            births.push(now);
        }
    }
    assert_eq!(births, [50, 550, 1050]);
}

#[tokio::test]
async fn white_demon_offscreen_suspends_remaining_cooldown_without_rearming() {
    let (mut world, player, demon) = arena("Realm").await;
    world
        .apply_condition(demon, condition::PARALYZED, 10_000)
        .unwrap();
    assert_eq!(step(&mut world, 50).len(), 3);
    assert!(step(&mut world, 100).is_empty());
    world.players.get_mut(&player).unwrap().pos.y = 2.5;
    for now in (150..=650).step_by(50) {
        assert!(step(&mut world, now).is_empty());
    }
    assert_eq!(world.entities[&demon].activated, Some(600));
    world.players.get_mut(&player).unwrap().pos.y = 24.5;
    for now in (700..=1150).step_by(50) {
        assert_eq!(step(&mut world, now).len(), if now == 1150 { 3 } else { 0 });
    }
}
