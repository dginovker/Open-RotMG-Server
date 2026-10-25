use super::*;

fn held_weapon(class: u16, weapon: u16) -> (World, i32) {
    let data = std::path::Path::new("data");
    let content = Arc::new(Content::load(data).unwrap());
    let map = Arc::new(Map::load(&data.join("nexus.wmap"), &content).unwrap());
    let mut world = World::new(content, map).unwrap();
    let base = world.content.classes[&class].caps;
    let character: Character = serde_json::from_value(serde_json::json!({
        "schema":1,"lifetime":crate::store::FameLifetime::complete(),
        "class":class,"base":base,
        "id":1,"account":1,"name":"Weapon","level":20,"exp":0,
        "fame":0,"hp":base[0],"mp":base[1],"revision":0,"alive":true,
        "items":[weapon,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1]
    }))
    .unwrap();
    let id = world.enter(character, None).unwrap();
    world.players.get_mut(&id).unwrap().controller = Controller::Qa;
    world.action(id, Action::Shoot { angle: 0.4 }).unwrap();
    (world, id)
}

#[test]
fn held_double_bow_spreads_from_one_muzzle_at_normal_cadence() {
    let (mut world, id) = held_weapon(0x307, 0xa1d);
    let position = world.players[&id].pos;
    let mana = world.players[&id].character.mp;
    let weapon = world.content.get(0xa1d).unwrap();
    assert_eq!(weapon.shots, 2);
    let half_arc = weapon.arc.to_radians() / 2.;
    assert!(half_arc > 0.);
    let cooldown = reconciliation::attack_cooldown(
        &world.players[&id],
        &world.content,
        world.now,
        weapon.rate,
    )
    .unwrap();
    let volley_ticks = ((cooldown as u64 - 2).div_ceil(50)) as usize;
    world.players.get_mut(&id).unwrap().next_bullet = 127;
    let mut volleys = 0;
    for tick in 0..20 {
        world.step();
        let shots: Vec<_> = world
            .bullets
            .iter()
            .filter(|b| b.owner == id && b.born == world.now)
            .collect();
        if tick % volley_ticks == 0 {
            assert_eq!(shots.len(), 2);
            for (i, shot) in shots.iter().enumerate() {
                assert!(
                    (shot.angle - (0.4 + if i == 0 { -half_arc } else { half_arc })).abs() < 1e-5
                );
                assert!(
                    shot.start.distance(Pos {
                        x: position.x + 0.3 * 0.4_f32.cos(),
                        y: position.y + 0.3 * 0.4_f32.sin()
                    }) < 1e-5
                );
                assert_eq!(shot.id, ((127 + volleys * 2 + i) % 128) as u8);
            }
            volleys += 1;
        } else {
            assert!(shots.is_empty());
        }
        assert_eq!(world.players[&id].character.shots, volleys as u64 * 2);
        assert!(world.players[&id].pos.distance(position) < 1e-5);
        assert_eq!(world.players[&id].character.mp, mana);
    }
    assert!(volleys > 1);
}

#[test]
fn held_zero_arc_staff_keeps_common_aim_muzzle_and_opposite_wave_parity() {
    let (mut world, id) = held_weapon(crate::content::WIZARD, 0xa97);
    let position = world.players[&id].pos;
    let mana = world.players[&id].character.mp;
    assert_eq!(world.content.get(0xa97).unwrap().arc, 0.);
    world.step();
    let shots: Vec<_> = world.bullets.iter().filter(|b| b.owner == id).collect();
    assert_eq!(shots.len(), 2);
    assert_eq!((shots[0].id, shots[1].id), (1, 2));
    let mut waves = Vec::new();
    for shot in &shots {
        assert!((shot.angle - 0.4).abs() < 1e-5);
        assert!(
            shot.start.distance(Pos {
                x: position.x + 0.3 * 0.4_f32.cos(),
                y: position.y + 0.3 * 0.4_f32.sin()
            }) < 1e-5
        );
        let (x, y) = shot.spec.offset(shot.id, shot.angle, 100.);
        waves.push(-x * shot.angle.sin() + y * shot.angle.cos());
    }
    assert!(waves[0].abs() > 0.1);
    assert!((waves[0] + waves[1]).abs() < 1e-5);
    assert_eq!(world.players[&id].character.mp, mana);
}

#[test]
fn held_weapon_is_blocked_by_stunned_but_not_quiet() {
    for condition in [crate::condition::STUNNED, crate::condition::QUIET] {
        let (mut world, id) = held_weapon(0x307, 0xa1d);
        let mana = world.players[&id].character.mp;
        world.apply_condition(id, condition, 5000).unwrap();
        world.step();
        let stunned = condition == crate::condition::STUNNED;
        assert_eq!(
            world.bullets.iter().filter(|b| b.owner == id).count(),
            if stunned { 0 } else { 2 }
        );
        assert_eq!(
            world.players[&id].character.shots,
            if stunned { 0 } else { 2 }
        );
        assert_eq!(
            world.players[&id].character.mp,
            if stunned { mana } else { 0 }
        );
        assert_eq!(world.players[&id].next_bullet, 3);
    }
}
