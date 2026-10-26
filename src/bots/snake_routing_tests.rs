use super::*;
use crate::{store::Character, world::Directory};

fn snake() -> (Directory, i32, i32, tokio::sync::mpsc::Receiver<Packet>) {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let mut directory =
        Directory::prepared(std::path::Path::new("data"), content.clone(), 20, 423, 2).unwrap();
    let world = directory
        .prepared_instance_with_seed("snake-pit", 0)
        .unwrap();
    let mut character = Character::fresh(content.get(0x300).unwrap()).unwrap();
    character.id = 1;
    character.account = 1;
    character.name = "Navigator".into();
    character.level = 20;
    character.base = [500, 200, 35, 10, 25, 30, 30, 25];
    character.hp = 500;
    character.mp = 200;
    character.items[1] = -1;
    let w = directory.worlds.get_mut(&world).unwrap();
    let (packets, output) = tokio::sync::mpsc::channel(4096);
    let (lost, _) = tokio::sync::watch::channel(None);
    let player = w
        .enter(character, Some(crate::world::Output { packets, lost }))
        .unwrap();
    w.players.get_mut(&player).unwrap().controller = crate::world::Controller::Autonomous;
    w.updates(player, true).unwrap();
    (directory, world, player, output)
}

#[test]
fn dormant_snake_quest_approaches_and_digs_observed_tunnel() {
    let (mut directory, world, player, _output) = snake();
    let view = directory.player_view(player).unwrap();
    let boss = view.quest.expect("ordinary quest arrow publishes Stheno");
    let target = view.objects.iter().find(|o| o.id == boss).unwrap();
    assert_eq!(target.kind, crate::dungeon::snake::BOSS);
    assert!(!progression::vulnerable(target));
    assert!(view.objects.iter().any(|o| o.kind == 0x172));
    assert!(!view.objects.iter().any(|o| matches!(o.kind, 0x704 | 0x71d)));
    let mut brain = Brain::new(ARCHETYPES[0], &view);
    let mut decision = brain
        .decide(&view, &directory.worlds[&world].content)
        .unwrap();
    assert!(
        matches!(decision.goal, Goal::Break(_)),
        "expected quest-directed tunnel clearing, got {:?}",
        decision.goal
    );
    let start = view.position;
    let wall = match decision.goal {
        Goal::Break(id) => id,
        _ => unreachable!(),
    };
    assert_eq!(directory.worlds[&world].entities[&wall].kind, 0x172);
    let wall_hp = directory.worlds[&world].entities[&wall].hp;
    for _ in 0..240 {
        brain.goal = decision.goal.clone();
        brain.destination = decision.movement;
        let w = directory.worlds.get_mut(&world).unwrap();
        w.autonomous_action(player, Action::Stop).unwrap();
        if let Some(position) = decision.movement {
            w.autonomous_action(
                player,
                Action::Move {
                    x: position.x,
                    y: position.y,
                },
            )
            .unwrap();
        }
        if let Some(angle) = decision.aim {
            assert!(matches!(decision.goal, Goal::Break(id) if w.entities[&id].kind == 0x172));
            w.autonomous_action(player, Action::Shoot { angle })
                .unwrap();
        }
        w.step();
        if w.entities.get(&wall).is_none_or(|e| e.hp < wall_hp) {
            assert!(
                w.players[&player].pos.distance(start) > 0.5,
                "normal movement reached a firing waypoint"
            );
            assert_eq!(
                w.entities[&boss].hp, 7500,
                "dormant shielded boss takes no damage"
            );
            return;
        }
        let view = directory.player_view(player).unwrap();
        decision = brain
            .decide(&view, &directory.worlds[&world].content)
            .unwrap();
    }
    panic!("normal movement/shooting never damaged the observed tunnel wall");
}

#[test]
fn snake_navigation_does_not_invent_quests_or_break_immutable_cover() {
    let (directory, world, player, _output) = snake();
    let mut view = directory.player_view(player).unwrap();
    let content = &directory.worlds[&world].content;
    let boss = view.quest.unwrap();
    view.objects.retain(|o| o.id != boss);
    view.quest = None;
    let decision = Brain::new(ARCHETYPES[0], &view)
        .decide(&view, content)
        .unwrap();
    assert!(!matches!(decision.goal, Goal::Hunt(_) | Goal::Break(_)));
    assert!(decision.aim.is_none());
    view.quest = Some(boss);
    let decision = Brain::new(ARCHETYPES[0], &view)
        .decide(&view, content)
        .unwrap();
    assert!(!matches!(decision.goal, Goal::Hunt(_) | Goal::Break(_)));
    assert!(decision.aim.is_none());
    let mut view = directory.player_view(player).unwrap();
    for wall in view.objects.iter_mut().filter(|o| o.kind == 0x172) {
        wall.kind = 0x170;
    }
    let decision = Brain::new(ARCHETYPES[0], &view)
        .decide(&view, content)
        .unwrap();
    assert!(!matches!(decision.goal, Goal::Break(_)));
    assert!(decision.aim.is_none());
    let (content, mut close) = quest_tests::fixture();
    close.map.name = "Snake Pit".into();
    close.objects[0].kind = crate::dungeon::snake::BOSS;
    close.objects[0].position = Pos { x: 14.5, y: 10.5 };
    close.objects[0].max_hp = 7500;
    for condition in [
        crate::condition::INVULNERABLE,
        crate::condition::INVINCIBLE,
        crate::condition::STASIS,
    ] {
        close.objects[0].condition_mask = Some(crate::condition::bit(condition));
        let decision = Brain::new(ARCHETYPES[0], &close)
            .decide(&close, &content)
            .unwrap();
        assert!(
            decision.aim.is_none(),
            "navigation cannot authorize attacks against protected Stheno"
        );
        assert!(
            decision.item.is_none(),
            "no damaging ability against protected Stheno"
        );
    }
}

#[test]
fn clear_cover_orders_only_actionable_walls() {
    let (content, mut view) = quest_tests::fixture();
    view.position = Pos { x: 1.5, y: 1.5 };
    view.objects = [(2, Pos { x: 4.5, y: 1.5 }), (3, Pos { x: 6.5, y: 6.5 })]
        .into_iter()
        .map(|(id, position)| crate::world::view::VisibleObject {
            id,
            account: None,
            kind: 0x172,
            name: String::new(),
            position,
            hp: 100,
            max_hp: 100,
            level: None,
            equipped: vec![],
            contents: vec![],
            blocks: true,
            usable: false,
            condition_mask: Some(0),
        })
        .collect();
    let mut grid = Grid::terrain(&view);
    for point in [(4, 1), (6, 6), (5, 6), (7, 6), (6, 5), (6, 7)] {
        grid.walkable.remove(&point);
    }
    for wall in &view.objects {
        assert!(sight(&view, &grid, wall.position, false, Some(wall.id)));
    }
    for point in [(5, 6), (7, 6), (6, 5), (6, 7)] {
        assert!(grid.next(view.position, center(point)).is_none());
    }
    let destination = Pos { x: 9.5, y: 9.5 };
    view.objects[0].hp = 0;
    assert!(progression::clear_cover(&view, &content, &grid, destination, 4., false).is_none());
    view.objects[0].hp = 100;
    let decision = progression::clear_cover(&view, &content, &grid, destination, 4., false)
        .expect("unapproachable distant wall must not suppress shootable cover");
    assert_eq!(decision.goal, Goal::Break(2));
    assert_eq!(decision.aim, Some(0.));
    assert!(decision.movement.is_none());
}
