use super::*;
use crate::content::{Cell, game};

#[path = "journal_tests.rs"]
mod journal_tests;

fn authored(name: &str, id: u16, body: &str) -> String {
    format!(
        r#"<Object type="{id}" id="{name}" disposition="active" provenance="authored" reason="native World regression" origin="independent fixture">{body}</Object>"#
    )
}

fn spawn(w: &mut World, kind: u16, x: f32, y: f32) -> i32 {
    w.spawn_entity(kind, Pos { x, y }, vec![]).unwrap()
}

fn flat_map(size: usize) -> Map {
    Map {
        elevation: None,
        width: size as i32,
        height: size as i32,
        cells: vec![
            Cell {
                ground: 0x18,
                object: None,
                config: String::new(),
                terrain: 0,
                region: 0
            };
            size * size
        ],
        spawns: vec![(64.5, 64.5)],
    }
}

fn flat_nonrealm(content: Arc<Content>) -> World {
    World::in_domain(content, Arc::new(flat_map(128)), WorldDomain::NonRealm).unwrap()
}

pub(crate) fn drain_events(w: &mut World) -> Result<()> {
    if w.pending_terminal_events().is_empty() && w.delivery_events().is_empty() {
        return Ok(());
    }
    std::thread::scope(|scope| {
        scope.spawn(||->Result<()> {
        let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        runtime.block_on(async {
            let path=std::env::temp_dir().join(format!("world-terminal-{}-{}.db",std::process::id(),w.xml.instance));
            let store=crate::store::Store::open(&path,&w.content)?;
            let db=rusqlite::Connection::open(&path)?;
            for p in w.players.values() {
                // The Store holds the last committed history; shots fired since are not yet saved.
                let ch=&crate::store::Character{lifetime:p.committed_lifetime.clone(),..p.character.clone()};
                db.execute("INSERT OR IGNORE INTO accounts(id,guid,name) VALUES(?1,?2,?3)",rusqlite::params![ch.account,format!("world-component-{}",ch.account),ch.name])?;
                db.execute("INSERT OR IGNORE INTO account_fame VALUES(?1,0,0)",[ch.account])?;
                db.execute("INSERT OR IGNORE INTO vaults(account,chest,items) VALUES(?1,0,'[-1,-1,-1,-1,-1,-1,-1,-1]')",[ch.account])?;
                db.execute("INSERT INTO characters(id,account,stats,items,revision,alive) SELECT ?1,?2,?3,?4,?5,?6 WHERE NOT EXISTS(SELECT 1 FROM characters WHERE id=?1)",rusqlite::params![ch.id,ch.account,serde_json::to_string(ch)?,serde_json::to_string(&ch.items)?,ch.revision,ch.alive])?;
            }
            drop(db);
            for event in w.pending_terminal_events() {
                let record=store.coordinate_terminal(event.clone()).await?;
                w.finish_terminal_event(record)?;
                store.publish_world_event(event.clone()).await?;
                w.acknowledge_terminal_publication(event.id);
            }
            for event in w.delivery_events() {
                let expiry=w.entitlement_expiries(&event);
                if !expiry.is_empty(){store.expire_world_entitlements(event.clone(),expiry).await?;}
                let rows=store.unpicked_entitlements(event.clone()).await?;
                w.publish_entitlements(event,rows)?;
            }
            Ok(())
        })
    }).join().expect("shared real-journal test drain panicked")
    })
}

fn fixture(body: &str) -> (World, i32, i32) {
    let mut content = crate::world::test_content();
    let mut rules = authored("Pirate", 0x600, body)
        + &authored("Piratess", 0x601, "<Behavior>RemainVisible</Behavior>");
    for (kind, service) in [(6, "projectile"), (0x504, "container"), (0x307, "player")] {
        let name = &content.objects[&kind].name;
        rules += &format!(
            r#"<Object type="{kind}" id="{name}" disposition="native" nativeService="{service}" provenance="authored" origin="selected native fixture service" reason="explicit bounded World fixture dependency"/>"#
        );
    }
    let inputs = crate::content::game::test_inputs::selected([
        (
            "game/123.5.1/game.xml",
            br#"<Game schema="1"><Include file="rules.xml"/><Root object="Pirate"/></Game>"#
                .to_vec(),
        ),
        (
            "game/123.5.1/rules.xml",
            format!("<Objects>{rules}</Objects>").into_bytes(),
        ),
    ]);
    content.game = Some(Arc::new(game::compile_fixture(&content, inputs).unwrap()));
    let mut w = flat_nonrealm(Arc::new(content));
    let mut ch = Character::fresh(w.content.get(0x307).unwrap()).unwrap();
    ch.account = 1;
    ch.id = 1;
    let player = w.enter(ch, None).unwrap();
    w.players.get_mut(&player).unwrap().pos = Pos { x: 65.5, y: 64.5 };
    let id = spawn(&mut w, 0x600, 64.5, 64.5);
    (w, player, id)
}

#[test]
fn exist_and_none_exist_use_other_live_entities_of_the_requested_kinds() -> Result<()> {
    let mut w = flat_nonrealm(Arc::new(crate::world::test_content()));
    let id = spawn(&mut w, 0x600, 10.5, 10.5);
    let scope = Scope {
        identity: w.xml_identity(id)?,
        issuer: id,
        order: None,
        listener: None,
        generation: 0,
    };
    let exists = |w: &World, kinds: &[u16]| behavior::World::exists(w, scope, kinds);
    assert!(!exists(&w, &[0x600, 0x601])?);
    let other = spawn(&mut w, 0x601, 15.5, 10.5);
    assert!(exists(&w, &[0x600, 0x601])?);
    w.entities.get_mut(&other).unwrap().hp = 0;
    assert!(!exists(&w, &[0x601])?);
    w.entities.get_mut(&other).unwrap().hp = 1;
    assert!(exists(&w, &[0x601])?);
    w.remove_entity(other);
    assert!(!exists(&w, &[0x601])?);

    let bag = w.insert_native_entity(
        0x500,
        NativeService::Container,
        Pos { x: 20.5, y: 10.5 },
        vec![],
        "existence predicate test",
    )?;
    assert_eq!(w.entities[&bag].hp, 0);
    assert!(exists(&w, &[0x500])?);
    w.entities.set_kind(bag, 0x503);
    assert!(!exists(&w, &[0x500])?);
    assert!(exists(&w, &[0x503])?);
    w.remove_entity(bag);
    assert!(!exists(&w, &[0x503])?);
    Ok(())
}

#[test]
fn medusa_grenade_sends_throw_then_aoe_and_applies_damage_on_landing() {
    let mut w = flat_nonrealm(Arc::new(crate::world::test_content()));
    let mut ch = Character::fresh(w.content.get(0x307).unwrap()).unwrap();
    ch.base[0] = 500;
    ch.hp = ch.base[0];
    ch.account = 1;
    ch.id = 1;
    let (packets, mut inbox) = mpsc::channel(64);
    let (lost, _) = watch::channel(None);
    let player = w.enter(ch, Some(Output { packets, lost })).unwrap();
    let target = w.players[&player].pos;
    let hp = w.players[&player].character.hp;
    let defense = w.defense(player).unwrap();
    let medusa = spawn(&mut w, 0x654, target.x - 5., target.y);
    let mut drain = || std::iter::from_fn(|| inbox.try_recv().ok()).collect::<Vec<_>>();
    drain();

    w.now = 50;
    w.xml_step().unwrap();
    let packets = drain();
    let (throw_index, thrown) = packets
        .iter()
        .enumerate()
        .find(|(_, p)| p.id == wire::SHOW_EFFECT)
        .expect("throw packet");
    assert!(packets[..throw_index].iter().any(|p| p.id == wire::UPDATE));
    let mut effect = wire::Reader::new(&thrown.body);
    assert_eq!(effect.u8().unwrap(), 4);
    assert_eq!(effect.i32().unwrap(), medusa);
    assert_eq!(effect.f32().unwrap(), target.x);
    assert_eq!(effect.f32().unwrap(), target.y);
    effect.take(8).unwrap();
    assert_eq!(effect.i32().unwrap() as u32, 0xffff0000);
    effect.done().unwrap();
    assert!(packets.iter().all(|p| p.id != wire::AOE));

    w.remove_entity(medusa);
    w.now += 1499;
    w.xml_step().unwrap();
    assert_eq!(w.players[&player].character.hp, hp);
    assert!(drain().iter().all(|p| p.id != wire::AOE));
    w.now += 1;
    w.xml_step().unwrap();
    assert_eq!(
        w.players[&player].character.hp,
        hp - crate::condition::damage(150, defense, 0, false)
    );
    let packets = drain();
    let impacts: Vec<_> = packets.iter().filter(|p| p.id == wire::AOE).collect();
    assert_eq!(
        impacts.len(),
        1,
        "Medusa damage landed without the native AOE packet"
    );
    let mut impact = wire::Reader::new(&impacts[0].body);
    assert_eq!(impact.f32().unwrap(), target.x);
    assert_eq!(impact.f32().unwrap(), target.y);
    assert_eq!(impact.f32().unwrap(), 4.);
    assert_eq!(impact.i16().unwrap(), 150);
    assert_eq!(impact.u8().unwrap(), 0);
    assert_eq!(impact.f32().unwrap(), 0.);
    assert_eq!(impact.i16().unwrap(), 0x654);
    impact.done().unwrap();
}

#[test]
fn recovered_realm_movement_uses_source_speed_and_gods_follow_at_seven_to_ten_tiles() {
    let mut map = flat_map(128);
    let mut elevation = vec![0; map.cells.len()];
    for y in 48..80 {
        elevation[y * 128 + 48..y * 128 + 80].fill(254);
    }
    elevation[16 * 128 + 32] = 254;
    elevation[32 * 128 + 16] = 254;
    map.elevation = Some(elevation.into());
    let mut w = World::in_domain(
        Arc::new(crate::world::test_content()),
        Arc::new(map),
        WorldDomain::Realm,
    )
    .unwrap();
    let mut ch = Character::fresh(w.content.get(crate::content::WIZARD).unwrap()).unwrap();
    ch.account = 1;
    ch.id = 1;
    let player = w.enter(ch, None).unwrap();
    w.players.get_mut(&player).unwrap().newbie_until = 0;

    for (kind, player_gap, speed, state_delay) in [
        (0x60a, 9., 2., 0),     // Lowlands Sandsman Archer, without its anchor.
        (0x625, 9., 7., 0),     // Midlands Ice Sprite.
        (0x651, 9., 5., 0),     // White Demon.
        (0x652, 9., 2., 0),     // Sprite God wanders rather than following.
        (0x654, 9., 5., 0),     // Medusa.
        (0x655, 9., 5., 0),     // Ent God.
        (0x656, 9., 5., 0),     // Beholder.
        (0x657, 9., 5., 0),     // Flying Brain.
        (0x658, 9., 5., 0),     // Slime God.
        (0x659, 9., 5., 0),     // Ghost God.
        (0x91a, 6., 3.5, 2000), // Djinn follows in Wait1 after the opening Bullet1 phase.
        (0x6d8, 9., 4., 50),    // Leviathan transitions to its opening Move.
    ] {
        let id = spawn(&mut w, kind, 64.5, 64.5);
        let start = w.now;
        let mut traveled = 0.;
        for elapsed in (0..=1000 + state_delay).step_by(50) {
            let old = w.entities[&id].pos;
            w.players.get_mut(&player).unwrap().pos = Pos {
                x: old.x + player_gap,
                y: old.y,
            };
            w.now = start + elapsed;
            w.xml_step().unwrap();
            let pos = w.entities[&id].pos;
            if elapsed > state_delay {
                let step = old.distance(pos);
                assert!(
                    (step - speed * 0.05).abs() < 0.0001,
                    "{kind:#x} at {elapsed}ms: {step}"
                );
                if speed == 5. {
                    assert!(
                        (pos.x - old.x - step).abs() < 0.0001,
                        "{kind:#x} did not pursue east"
                    );
                }
                traveled += step;
            }
        }
        assert!(
            (traveled - speed).abs() < 0.001,
            "{kind:#x}: {traveled} tiles/s"
        );
        w.remove_entity(id);
    }

    let id = spawn(&mut w, 0x654, 16.5, 16.5);
    w.players.get_mut(&player).unwrap().pos = Pos { x: 16.5, y: 25.5 };
    w.xml_step().unwrap();
    w.now += 50;
    w.xml_step().unwrap();
    let pos = w.entities[&id].pos;
    assert_eq!(
        (pos.x, pos.y),
        (16.625, 16.5),
        "StayAbove breaks equal-distance cross-block ties by y before x"
    );
}

#[test]
fn medusa_wander_repicks_within_distance_budget() {
    let mut map = flat_map(128);
    map.elevation = Some(vec![254; map.cells.len()].into());
    let mut w = World::in_domain(
        Arc::new(crate::world::test_content()),
        Arc::new(map),
        WorldDomain::Realm,
    )
    .unwrap();
    w.prepared_seed = Some(6);
    let mut ch = Character::fresh(w.content.get(crate::content::WIZARD).unwrap()).unwrap();
    ch.base[0] = 500;
    ch.hp = ch.base[0];
    ch.account = 1;
    ch.id = 1;
    let player = w.enter(ch, None).unwrap();
    let p = w.players.get_mut(&player).unwrap();
    p.newbie_until = 0;
    p.pos = Pos { x: 68.5, y: 64.5 };
    let id = spawn(&mut w, 0x654, 64.5, 64.5);
    w.xml_step().unwrap();
    let local = w.xml_game().definitions[&0x654]
        .states
        .iter()
        .flat_map(|s| &s.behaviors)
        .find(|b| b.kind == K::Wander)
        .unwrap()
        .local;
    let token = *w
        .xml
        .motions
        .keys()
        .find(|token| token.local == local)
        .unwrap();
    let start = w.entities[&id].pos;
    let mut prev = start;
    let mut leg = 0.;
    let mut completed = 0;
    let mut drift = 0.0_f32;
    for at in (50..=3000).step_by(50) {
        w.now = at;
        w.xml_step().unwrap();
        let pos = w.entities[&id].pos;
        let step = prev.distance(pos);
        assert!((step - 0.1).abs() < 0.0001, "speed at {at}ms: {step}");
        leg += step;
        assert!(leg <= 0.8001, "Wander leg traveled {leg} tiles at {at}ms");
        if !w.xml.motions.contains_key(&token) {
            assert!(leg >= 0.2999, "Wander leg ended after {leg} tiles");
            completed += 1;
            leg = 0.;
        }
        drift = drift.max(start.distance(pos));
        prev = pos;
    }
    assert!(completed >= 7, "only {completed} completed Wander legs");
    assert!(drift < 2.5, "seeded Medusa drifted {drift} tiles");
    assert!(eligible(&w.players[&player]));
    println!("Medusa completed {completed} Wander legs; peak drift {drift:.3} tiles");
}

#[test]
fn follow_cools_after_target_loss_or_duration_but_tracks_inside_stop_range() {
    for (duration, loses_target) in [("0", true), ("0.4", false)] {
        let (mut w, player, id) = fixture(&format!(
            r#"<Behavior range="2" speed="1" duration="{duration}" cooldown="0.2">Follow</Behavior>"#,
        ));
        w.players.get_mut(&player).unwrap().newbie_until = 0;
        for (at, target_x, actor_x) in [
            (0, 65.5, 64.5),    // Acquire within stop range, without moving.
            (250, 70.5, 65.75), // Integrate one 250ms simulated interval.
            (300, 66.5, 65.75), // An idle chase must not restart its duration.
            (350, 70.5, 66.),
            (400, if loses_target { 80. } else { 70.5 }, 66.),
            (450, 70.5, 66.),
            (550, 70.5, 66.),
            (600, 70.5, 66.25),
        ] {
            w.players.get_mut(&player).unwrap().pos.x = target_x;
            w.now = at;
            w.xml_step().unwrap();
            assert!(
                (w.entities[&id].pos.x - actor_x).abs() < 0.0001,
                "duration={duration} at={at}"
            );
        }
    }
}

#[test]
fn xml_runtime_world_xml_only_cooldown_state_and_birth_mutations_change_observed_services() {
    fn candidate(name: &str, cooldown: &str, state: &str, children: usize) -> World {
        let (mut w, _, _) = fixture(&format!(
            r#"<Behavior childIds="Piratess" maxChildren="{children}" cooldown="10">MakeMinions</Behavior><Behavior type="auto" projectileId="0" cooldown="{cooldown}">Shoot</Behavior><State id="shield"><Behavior effect="Invulnerable">ConditionEffect</Behavior><Transition afterTime="{state}">open</Transition></State><State id="open"/>"#
        ));
        w.name = name.into();
        for at in [0, 50, 100, 150] {
            w.now = at;
            w.xml_step().unwrap();
        }
        w
    }
    let a = candidate("Fixture", "0.1", "0.1", 1);
    let renamed = candidate("Renamed Fixture", "0.1", "0.1", 1);
    let b = candidate("Fixture", "0.2", "0.2", 2);
    assert_eq!(a.bullets.len(), 2);
    assert_eq!(b.bullets.len(), 1);
    assert_eq!(a.entities.values().filter(|e| e.kind == 0x601).count(), 1);
    assert_eq!(b.entities.values().filter(|e| e.kind == 0x601).count(), 2);
    let root = |w: &World| {
        w.entities
            .values()
            .find(|e| e.kind == 0x600)
            .unwrap()
            .condition
            & crate::condition::bit(crate::condition::INVULNERABLE)
    };
    assert_eq!(root(&a), 0);
    assert_ne!(root(&b), 0);
    assert_eq!(renamed.bullets.len(), a.bullets.len());
    assert_eq!(renamed.entities.len(), a.entities.len());
    assert_eq!(root(&renamed), root(&a));
}

#[test]
fn xml_cutover_invalid_owned_key_route_does_not_reserve_or_spend() {
    let mut content = crate::world::test_content();
    let game = Arc::make_mut(content.game.as_mut().unwrap());
    let route = game.portal_routes.remove(&0x70c).unwrap();
    for bad in [
        None,
        Some(game::PortalDestination::Nexus),
        Some(game::PortalDestination::Instance("pirate-cave".into())),
    ] {
        let mut content = content.clone();
        if let Some(destination) = bad {
            let mut route = route.clone();
            route[0].destination = destination;
            Arc::make_mut(content.game.as_mut().unwrap())
                .portal_routes
                .insert(0x70c, route);
        }
        let mut world = flat_nonrealm(Arc::new(content));
        world.xml.template = Some("nexus".into());
        let mut ch = Character::fresh(world.content.get(crate::content::WIZARD).unwrap()).unwrap();
        ch.account = 1;
        ch.id = 1;
        ch.items[4] = 0x71f;
        let player = world.enter(ch, None).unwrap();
        let p = &world.players[&player];
        let (items, revision) = (p.character.items.clone(), p.character.revision);
        let (ids, entities) = (world.ids.load(Ordering::Relaxed), world.entities.len());
        let error = world
            .use_item(
                player,
                Slot {
                    object: player,
                    slot: 4,
                    kind: 0x71f,
                },
                p.pos,
            )
            .err()
            .unwrap();
        let error = format!("{error:#}");
        assert!(
            error.contains("owned portal preflight before inventory reservation")
                && error.contains("kind=0x70c"),
            "{error}"
        );
        let p = &world.players[&player];
        assert!(!p.pending);
        assert_eq!(
            (p.character.items.clone(), p.character.revision),
            (items, revision)
        );
        assert_eq!(
            (world.ids.load(Ordering::Relaxed), world.entities.len()),
            (ids, entities)
        );
    }
}

#[test]
fn child_counts_follow_adoption_reparenting_and_removal() {
    let (mut w, _, first) = fixture("<Behavior>RemainVisible</Behavior>");
    let second = spawn(&mut w, 0x600, 60.5, 60.5);
    let child = spawn(&mut w, 0x601, 62.5, 62.5);
    w.set_parent(child, Some(first));
    assert_eq!(
        (w.xml_owned_count(first), w.xml_owned_count(second)),
        (1, 0)
    );
    w.set_parent(child, Some(second));
    assert_eq!(
        (w.xml_owned_count(first), w.xml_owned_count(second)),
        (0, 1)
    );
    w.remove_entity(child);
    assert_eq!(w.xml_owned_count(second), 0);
}
