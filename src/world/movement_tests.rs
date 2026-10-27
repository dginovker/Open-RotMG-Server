use super::*;
use std::time::Duration;

fn fixture(ground: u8, wall: bool) -> (World, i32) {
    let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data");
    let content = Arc::new(Content::load(&data).unwrap());
    let mut map = Map {
        width: 32,
        height: 32,
        cells: vec![
            crate::content::Cell {
                ground,
                object: None,
                config: String::new(),
                terrain: 0,
                region: 0,
            };
            32 * 32
        ],
        spawns: vec![(8.5, 8.5)],
    };
    if wall {
        map.cells[8 * 32 + 9].ground = 255;
    }
    let mut world = World::new(content, Arc::new(map)).unwrap();
    let character: Character = serde_json::from_value(serde_json::json!({
        "schema":1,"lifetime":crate::store::FameLifetime::complete(),
        "class":0x300,"base":[200,100,15,0,75,15,15,10],
        "id":1,"account":1,"name":"Movement","level":20,"exp":0,
        "fame":0,"hp":200,"mp":100,"revision":0,"alive":true,
        "items":[0xa14,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1]
    }))
    .unwrap();
    let id = world.enter(character, None).unwrap();
    let p = world.players.get_mut(&id).unwrap();
    p.controller = Controller::Human;
    p.last_move = 1000;
    p.last_move_at = 1000;
    p.movement_credit = 0.78;
    (world, id)
}

fn native(
    world: &mut World,
    id: i32,
    x: f32,
    y: f32,
    time: i32,
    received: u64,
) -> Result<NativeInput> {
    world.native_move_received(
        id,
        Pos { x, y },
        (time, world.attack_epoch + Duration::from_millis(received)),
    )
}

#[test]
fn native_movement_rejects_speed_forged_time_and_blocked_paths_without_mutation() {
    let mut accepted = Vec::new();
    for (label, wall, x, time, received) in [
        ("speed", false, 12.5, 1100, 1100),
        ("forged clock", false, 10.5, 1350, 1000),
        ("blocked path", true, 10.5, 1350, 1350),
    ] {
        let (mut world, id) = fixture(0x18, wall);
        let before = world.players[&id].pos;
        let result = native(&mut world, id, x, 8.5, time, received);
        let Err(error) = result else {
            accepted.push(label);
            continue;
        };
        assert!(
            crate::server::lifecycle::rule_rejection(&error),
            "{error:#}"
        );
        let p = &world.players[&id];
        assert_eq!(
            (
                p.pos.x,
                p.pos.y,
                p.last_move,
                p.last_move_at,
                p.movement_credit
            ),
            (before.x, before.y, 1000, 1000, 0.78),
            "{label}"
        );
    }
    assert!(
        accepted.is_empty(),
        "invalid native movements accepted: {accepted:?}"
    );
}

#[test]
fn native_movement_charges_credit_and_keeps_bounded_lag_tolerance() {
    let (mut world, id) = fixture(0x18, false);
    native(&mut world, id, 9.46, 8.5, 1100, 1100).unwrap();
    assert!((world.players[&id].movement_credit - 0.78).abs() < 0.001);
    native(&mut world, id, 10.42, 8.5, 1200, 1200).unwrap();
    native(&mut world, id, 11.38, 8.5, 1300, 1200).unwrap_err();
    let (mut world, id) = fixture(0x18, false);
    native(&mut world, id, 11.5, 8.5, 1350, 1350).unwrap();
    assert!(world.players[&id].movement_credit < 1.);
    assert!(native(&mut world, id, 16.5, 8.5, 2350, 2350).is_err());
}

#[test]
fn native_movement_uses_stats_conditions_and_terrain() {
    for (ground, speed_stat, condition, distance) in [
        (0x18, 75, 0, 0.96),
        (0x18, 15, 0, 0.512),
        (0x18, 75, SLOWED, 0.4),
        (0x18, 75, SPEEDY, 1.44),
        (0x18, 75, SLOWED | SPEEDY, 0.4),
        (0x17, 75, 0, 0.672),
        (0xa1, 75, 0, 0.71),
        (0xa2, 75, 0, 1.21),
    ] {
        let (mut world, id) = fixture(ground, false);
        let p = world.players.get_mut(&id).unwrap();
        p.character.base[4] = speed_stat;
        p.lab_conditions = condition;
        native(&mut world, id, 8.5 + distance, 8.5, 1100, 1100).unwrap();
        let (mut world, id) = fixture(ground, false);
        let p = world.players.get_mut(&id).unwrap();
        p.character.base[4] = speed_stat;
        p.lab_conditions = condition;
        assert!(native(&mut world, id, 8.5 + distance + 0.4, 8.5, 1100, 1100).is_err());
    }
}

#[test]
fn native_movement_sentinel_immobilization_and_goto_keep_authority() {
    let (mut world, id) = fixture(0x18, false);
    native(&mut world, id, -1., -1., 1100, 1100).unwrap();
    assert_eq!(world.players[&id].pos.x, 8.5);
    assert_eq!(world.players[&id].last_move, 1100);
    let (tx, mut rx) = mpsc::channel(16);
    let (lost, _) = watch::channel(None);
    world.players.get_mut(&id).unwrap().tx = Some(Output { packets: tx, lost });
    world.players.get_mut(&id).unwrap().lab_conditions = PARALYZED;
    native(&mut world, id, 9., 8.5, 1200, 1200).unwrap();
    assert_eq!(world.players[&id].pos.x, 8.5);
    assert_eq!(rx.try_recv().unwrap().id, wire::GOTO);
    assert_eq!(
        native(&mut world, id, 20., 8.5, 1300, 1300).unwrap(),
        NativeInput::DiscardedUntilGotoAck
    );
    assert_eq!(world.players[&id].last_move, 1200);
    world.acknowledge_goto(id, 1300).unwrap();
    world.players.get_mut(&id).unwrap().lab_conditions = 0;
    world.players.get_mut(&id).unwrap().pos = Pos { x: 20.5, y: 20.5 };
    native(&mut world, id, 21., 20.5, 1400, 1400).unwrap();
    world.players.get_mut(&id).unwrap().paused = true;
    native(&mut world, id, 21.5, 20.5, 1500, 1500).unwrap();
    assert_eq!(world.players[&id].pos.x, 21.);
    for (paused, condition) in [(true, 0), (false, PARALYZED)] {
        let (mut world, id) = fixture(0xa1, false);
        let p = world.players.get_mut(&id).unwrap();
        p.paused = paused;
        p.lab_conditions = condition;
        native(&mut world, id, -1., -1., 1500, 1500).unwrap();
        assert_eq!(world.players[&id].pos.x, 8.5);
        assert_eq!(world.players[&id].movement_credit, 0.78);
    }
}

#[test]
fn native_movement_preserves_half_cell_corner_and_rejects_void_crossing() {
    let (mut world, id) = fixture(0x18, false);
    Arc::make_mut(&mut world.map).cells[9 * 32 + 9].ground = 255;
    native(&mut world, id, 9., 8.5, 1100, 1100).unwrap();
    native(&mut world, id, 9.4, 8.5, 1200, 1200).unwrap();
    assert!(native(&mut world, id, 9.4, 8.6, 1300, 1300).is_err());
}
