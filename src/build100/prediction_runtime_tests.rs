use super::*;

pub(crate) fn report(target: i32, source: i32, bullet: u8, condition: u8) -> Packet {
    packet(9, |w| {
        w.i32(target)
            .u8(condition)
            .u8(255)
            .i16(i16::MIN)
            .u8(bullet)
            .i32(source);
    })
}
fn setup() -> (Content, Room, i32) {
    let (c, template) = spell_runtime_tests::setup(false);
    let mut r = Room::from_map(template.map.unwrap(), &c, 1_000_000).unwrap();
    for id in [1, 2] {
        spell_runtime_tests::attach(&mut r, &c, id);
    }
    let enemy = r.entities.values().find(|e| e.kind == 0x600).unwrap().id;
    for id in [1, 2] {
        r.publication(id, &[], &c).unwrap();
        assert!(r.players[&id].known_objects.contains(&enemy));
    }
    (c, r, enemy)
}
pub(crate) fn target_packets(out: &[Packet], target: i32) -> Vec<u8> {
    out.iter()
        .filter_map(|p| {
            let mut read = Reader::new(&p.body);
            match p.id {
                10 => {
                    read.i16().unwrap();
                    read.utf().unwrap();
                }
                11 => {}
                _ => return None,
            }
            (read.i32().unwrap() == target).then_some(p.id)
        })
        .collect()
}

#[test]
fn prediction_runtime_known_living_enemy_self_only_drop_then_make() {
    let (c, mut r, enemy) = setup();
    let cursor = spell_runtime_tests::aim(&r, 1);
    r.handle(1, spell_runtime_tests::cast(1, 1, 1, cursor), &c)
        .unwrap();
    let before = r.clone();
    let events = r.handle(1, report(enemy, 1, 96, 1), &c).unwrap();
    assert!(events.is_empty());
    authority_unchanged(&before, &r);
    assert!(r.players[&2].enemy_view_repair.is_none());
    let out = r.publication(1, &events, &c).unwrap();
    assert_eq!(target_packets(&out, enemy), vec![11, 10]);
    assert!(out.iter().all(|p| matches!(p.id, 10 | 11)));
    assert!(r.players[&1].known_objects.contains(&enemy));
    assert!(r.players[&1].enemy_view_repair.is_none());
    authority_unchanged(&before, &r);
    assert!(r.publication(2, &events, &c).unwrap().is_empty());
    assert!(r.publication(1, &[], &c).unwrap().is_empty());
}

pub(crate) fn authority_unchanged(before: &Room, after: &Room) {
    assert_eq!(before.enemy_slows, after.enemy_slows);
    assert_eq!(
        shore_runtime_tests::snapshot(before),
        shore_runtime_tests::snapshot(after)
    );
    assert_eq!(
        (
            before.id_limit,
            before.next_gift,
            before.shot_clock,
            before.next_wave,
            before.next_tick,
            before.portal,
            before.destination
        ),
        (
            after.id_limit,
            after.next_gift,
            after.shot_clock,
            after.next_wave,
            after.next_tick,
            after.portal,
            after.destination
        )
    );
    for (&id, a) in &before.players {
        let b = &after.players[&id];
        assert_eq!(
            (a.move_at, a.move_credit, a.teleport_ready, a.hazard_at),
            (b.move_at, b.move_credit, b.teleport_ready, b.hazard_at)
        );
        assert_eq!(
            (
                a.weapon.started,
                a.weapon.last_received,
                a.weapon.interval_ms
            ),
            (
                b.weapon.started,
                b.weapon.last_received,
                b.weapon.interval_ms
            )
        );
        let pending = |a: &Avatar| {
            a.weapon.pending.as_ref().map(|p| {
                format!(
                    "{:?}",
                    (
                        p.first_received,
                        p.due,
                        &p.weapon,
                        p.equipment,
                        p.attack,
                        p.conditions,
                        p.origin,
                        &p.shots
                    )
                )
            })
        };
        assert_eq!(pending(a), pending(b));
    }
    for (a, b) in before.bullets.iter().zip(&after.bullets) {
        assert_eq!(
            (a.amplitude, a.frequency, a.vx, a.vy),
            (b.amplitude, b.frequency, b.vx, b.vy)
        );
        match (&a.flight, &b.flight) {
            (Flight::Arena, Flight::Arena) => {}
            (
                Flight::Normal {
                    projectile: a,
                    angle: aa,
                },
                Flight::Normal {
                    projectile: b,
                    angle: ba,
                },
            ) => {
                assert_eq!(format!("{a:?}"), format!("{b:?}"));
                assert_eq!(aa, ba);
            }
            _ => panic!("projectile flight changed"),
        }
    }
}

#[test]
fn prediction_runtime_current_full_status_not_saved_prediction_or_stat25() {
    let (c, mut r, enemy) = setup();
    r.handle(1, report(enemy, 1, 1, 1), &c).unwrap();
    let e = r.entities.get_mut(&enemy).unwrap();
    e.kind = 0x801;
    e.x += 0.25;
    e.y += 0.5;
    e.stats.extend([(1, 3), (29, 1 << 3)]);
    assert!(!e.stats.contains_key(&25));
    let expected = make(enemy, e.kind, "", e.x, e.y, &e.stats);
    r.enemy_slows.insert(enemy, 3.);
    let before = r.clone();
    let out = r.publication(1, &[], &c).unwrap();
    assert_eq!(target_packets(&out, enemy), vec![11, 10]);
    assert_eq!(out.iter().find(|p| p.id == 10).unwrap().body, expected.body);
    authority_unchanged(&before, &r);
}

#[test]
fn prediction_runtime_ineligible_reports_are_advisory_without_mutation() {
    let (c, template, enemy) = setup();
    for case in 0..18 {
        let mut r = template.clone();
        let (mut target, mut source, mut bullet, mut condition) = (enemy, 1, 1, 1);
        match case {
            0 => condition = 0,
            1 => condition = 4,
            2 => condition = 255,
            3 => source = 2,
            4 => source = -1,
            5 => bullet = 0,
            6 => bullet = 97,
            7 => bullet = 255,
            8 => target = 1,
            9 => target = 2,
            10 => target = i32::MAX,
            11 => {
                r.entities.remove(&enemy);
            }
            12 => {
                r.entities.get_mut(&enemy).unwrap().stats.insert(1, 0);
            }
            13 => {
                r.entities.get_mut(&enemy).unwrap().stats.remove(&1);
            }
            14 => {
                r.players.get_mut(&1).unwrap().known_objects.remove(&enemy);
            }
            15 => r.entities.get_mut(&enemy).unwrap().x += 100.,
            16 => r.entities.get_mut(&enemy).unwrap().kind = c.named("Palm Tree 1").unwrap().id,
            17 => r.map = None,
            _ => unreachable!(),
        }
        let before = r.clone();
        assert!(
            r.handle(1, report(target, source, bullet, condition), &c)
                .unwrap()
                .is_empty()
        );
        assert!(r.players[&1].enemy_view_repair.is_none(), "case {case}");
        authority_unchanged(&before, &r);
    }
}

#[test]
fn prediction_runtime_strict_full_body_no_partial_request() {
    let (c, template, enemy) = setup();
    let valid = report(enemy, 1, 1, 1);
    assert_eq!(valid.body.len(), 13);
    for dead in [false, true] {
        let mut template = template.clone();
        if dead {
            template.character_mut(1).alive = false;
            template.character_mut(1).stats.insert(1, 0);
        }
        for len in (0..13).chain([14, 20]) {
            let mut r = template.clone();
            let mut body = valid.body.clone();
            body.resize(len, 0);
            assert!(r.handle(1, Packet { id: 9, body }, &c).is_err());
            assert_eq!(view_state(&r, 1), view_state(&template, 1));
            authority_unchanged(&template, &r);
        }
        if dead {
            let mut r = template.clone();
            assert!(r.handle(1, report(enemy, 1, 1, 1), &c).unwrap().is_empty());
            assert_eq!(view_state(&r, 1), view_state(&template, 1));
            authority_unchanged(&template, &r);
        }
    }
    for amount in [i16::MIN, 0, i16::MAX] {
        let mut r = template.clone();
        let input = packet(9, |w| {
            w.i32(enemy).u8(1).u8(0).i16(amount).u8(1).i32(1);
        });
        r.handle(1, input, &c).unwrap();
        assert_eq!(
            target_packets(&r.publication(1, &[], &c).unwrap(), enemy),
            vec![11, 10]
        );
        authority_unchanged(&template, &r);
    }
}

#[test]
fn prediction_runtime_publication_revalidates_death_interest_and_life() {
    let (c, template, enemy) = setup();
    for case in 0..6 {
        let mut r = template.clone();
        r.handle(1, report(enemy, 1, 1, 1), &c).unwrap();
        match case {
            0 => {
                r.entities.remove(&enemy);
            }
            1 => r.entities.get_mut(&enemy).unwrap().x += 100.,
            2 => {
                r.entities.get_mut(&enemy).unwrap().stats.insert(1, 0);
            }
            3 => r.entities.get_mut(&enemy).unwrap().kind = c.named("Palm Tree 1").unwrap().id,
            4 => {
                r.character_mut(1).alive = false;
                r.character_mut(1).stats.insert(1, 0);
            }
            5 => {
                r.players.get_mut(&1).unwrap().known_objects.remove(&enemy);
                r.entities.remove(&enemy);
            }
            _ => unreachable!(),
        }
        let out = r.publication(1, &[drop_object(enemy)], &c).unwrap();
        assert_eq!(
            target_packets(&out, enemy),
            if case <= 1 { vec![11] } else { vec![] },
            "case {case}"
        );
        assert!(r.players[&1].enemy_view_repair.is_none());
        assert!(target_packets(&r.publication(1, &[], &c).unwrap(), enemy).is_empty());
    }
}

#[test]
fn prediction_runtime_real_lethal_collision_wins_once_without_reintroduction() {
    let (c, mut r, enemy) = setup();
    r.handle(1, report(enemy, 1, 1, 1), &c).unwrap();
    let p = r.character(1);
    let origin = (p.x, p.y);
    let e = r.entities.get_mut(&enemy).unwrap();
    (e.x, e.y) = (origin.0 + 1., origin.1);
    r.bullets.push(Bullet {
        flight: Flight::Normal {
            projectile: c.get(0xa97).unwrap().normal_projectile(0).unwrap().clone(),
            angle: 0.,
        },
        owner: 1,
        id: 1,
        origin,
        age: 0.,
        amplitude: 0.,
        frequency: 0.,
        x: origin.0,
        y: origin.1,
        vx: 0.,
        vy: 0.,
        damage: 100,
        life: 1.,
    });
    let events = r.advance(0.1, &c).unwrap();
    assert!(!r.entities.contains_key(&enemy));
    let earned = r.character(1).clone();
    let out = r.publication(1, &events, &c).unwrap();
    assert_eq!(target_packets(&out, enemy), vec![11]);
    assert!(r.players[&1].enemy_view_repair.is_none());
    assert_eq!(r.character(1), &earned);
    assert!(target_packets(&r.publication(1, &[], &c).unwrap(), enemy).is_empty());
}

#[test]
fn prediction_runtime_new_make_before_events_own_damage_preserved_no_tick_feedback() {
    let (c, mut r, enemy) = setup();
    r.handle(1, report(enemy, 1, 1, 1), &c).unwrap();
    let mut new = r.entities[&enemy].clone();
    new.id = 1_499_999;
    let new_id = new.id;
    r.entities.insert(new_id, new);
    let events = vec![
        damage_packet(enemy, 1, 1, 2),
        damage_packet(new_id, 1, 2, 2),
    ];
    let out = r.publication(1, &events, &c).unwrap();
    assert_eq!(target_packets(&out, enemy), vec![11, 10]);
    assert_eq!(target_packets(&out, new_id), vec![10]);
    let last_make = out.iter().rposition(|p| p.id == 10).unwrap();
    let first_damage = out.iter().position(|p| p.id == 9).unwrap();
    let tick = out.iter().position(|p| p.id == 13).unwrap();
    assert!(last_make < first_damage && first_damage < tick);
    assert_eq!(out.iter().filter(|p| p.id == 9).count(), 2);
    let peer = r.publication(2, &events, &c).unwrap();
    assert!(target_packets(&peer, enemy).is_empty());
    assert_eq!(target_packets(&peer, new_id), vec![10]);
    assert!(r.publication(1, &[], &c).unwrap().is_empty());
    let p = r.character(1);
    let movement = packet(5, |w| {
        w.f32(p.x).f32(p.y);
    });
    let events = r.handle(1, movement, &c).unwrap();
    assert!(r.publication(1, &events, &c).unwrap().is_empty());
    let tick = r.tick(&c).unwrap();
    let out = r.publication(1, &[tick], &c).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].id, 13);
}

pub(crate) fn view_state(r: &Room, viewer: i32) -> (BTreeSet<i32>, Option<i32>) {
    let a = &r.players[&viewer];
    (a.known_objects.clone(), a.enemy_view_repair)
}
