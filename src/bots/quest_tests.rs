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
    assert_eq!(adventure(&mut brain, &v, &c).goal, Goal::Fight(3));
    v.map.name = "Undead Lair".into();
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
        elapsed_ms: 5000,
        position: Pos { x: 10.5, y: 10.5 },
        quest: Some(2),
        map: MapInfo {
            name: "Realm".into(),
            width: 128,
            height: 128,
            allow_teleport: true,
            layout: None,
            godlands: Arc::default(),
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
        assert!(world.bullets.is_empty());
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
        "Pirate Cave",
        "Snake Pit",
        "Sprite World",
        "Undead Lair",
        "Abyss of Demons",
        "Ocean Trench",
    ] {
        view.map.name = world.into();
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
    v.quest = None;
    let (falchion, potion) = (0xa6e, 0xa1f);
    let stat = progression::potion_index(&c, potion).unwrap().unwrap();
    v.character.base_stats[stat] = c.classes[&v.character.class].caps[stat];
    v.character.items[4..].copy_from_slice(&[falchion, potion, potion, -1, -1, -1, -1, -1]);
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
    chest.kind = 0x504;
    chest.hp = 0;
    chest.position = Pos { x: 11.5, y: 10.5 };
    chest.contents = vec![-1; 8];
    v.objects.push(chest);
    v.tick += 5;
    let deposit = brain.decide(&v, &c).unwrap();
    assert_eq!(deposit.goal, Goal::Bank(2));
    assert!(matches!(&deposit.item, Some(Action::Swap { from, .. }) if from.kind == falchion));
    assert!(
        deposit.journey.is_none(),
        "the Vault is not left before banking"
    );
    v.character.items[4] = -1;
    v.objects[0].contents[0] = falchion;
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
            .observe(&v, &c, &mut brain.mind.godlands, false);
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
    for (map, looping) in [("Nexus", false), ("Realm", true)] {
        let (c, mut v) = fixture();
        v.map.name = map.into();
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
fn arrow_follower_seeds_an_uncrowded_realm_event() {
    // Live: Lord of the Lost Lands was skipped 16 times in 25 minutes because nobody would start the crowd.
    let (c, mut v) = fixture();
    v.character.level = 20;
    v.objects[0].kind = 0xd50;
    v.objects[0].hp = 70000;
    v.objects[0].max_hp = 70000;
    let mut profile = ARCHETYPES[0];
    profile.role = persona::Role::Newbie;
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
            .observe(&v, &c, &mut brain.mind.godlands, veteran);
        let d = brain.decide(&v, &c).unwrap();
        assert_eq!(d.goal, goal, "remembered {remembered} veteran {veteran}");
        if goal == Goal::Venture {
            let step = d.movement.expect("heading for the Godlands");
            assert!(step.distance(godlands) < v.position.distance(godlands));
        }
    }
}

#[test]
fn full_bag_does_not_leave_the_realm_to_bank_into_a_full_vault() {
    // Live: 59 of 70 level-20 banking trips came back with the same full bag, every five minutes.
    let falchion = 0xa6e;
    for full in [false, true] {
        let (c, mut v) = fixture();
        v.quest = None;
        v.objects.clear();
        v.character.items[4..].copy_from_slice(&[falchion; 8]);
        let (_, mut vault) = fixture();
        vault.map.name = "Vault".into();
        vault.objects[0].kind = 0x504;
        vault.objects[0].contents = vec![if full { falchion } else { -1 }; 8];
        let mut brain = Brain::new(ARCHETYPES[0], &v);
        brain.mind.market.observe(&vault);
        let d = brain.decide(&v, &c).unwrap();
        assert_eq!(
            matches!(d.journey, Some(Journey::Nexus)),
            !full,
            "full Vault {full}: {:?}",
            d.goal
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
    v.projectiles.push(crate::world::view::VisibleProjectile {
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
        .observe(&v, &c, &mut brain.mind.godlands, false);
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
fn travel_keeps_its_chosen_edge_when_another_becomes_slightly_nearer() {
    // Live snapshots: on open ground a trip's step flipped between waypoints NW and SW as tiles were revealed.
    let (_, mut v) = fixture();
    let grid = |extra: bool, v: &mut PlayerView| {
        let geometry = crate::navigation::Geometry::new(
            (128, 128),
            vec![1; 128 * 128],
            &BTreeMap::from([(1, 1.)]),
        );
        let base = (0..=10).flat_map(|x| (0..128).map(move |y| (x, y)));
        let prongs = (11..=20).flat_map(|x| [(x, 40), (x, 60)]);
        let reveal = extra.then_some((21, 60));
        v.tiles = Arc::new(crate::navigation::Tiles::new(
            geometry,
            base.chain(prongs).chain(reveal),
        ));
        Grid::terrain(v)
    };
    v.position = Pos { x: 10.5, y: 50.5 };
    let target = Pos { x: 100.5, y: 50.5 };
    let mut kept = None;
    let first = grid(false, &mut v)
        .towards_kept(v.position, target, &mut kept)
        .unwrap();
    let second = grid(true, &mut v)
        .towards_kept(v.position, target, &mut kept)
        .unwrap();
    assert_eq!(
        first.y > v.position.y,
        second.y > v.position.y,
        "{first:?} then {second:?}"
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
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    brain.mind.godlands.insert(
        "classic-world-02-reconstruction".into(),
        BTreeSet::from([tile(west), tile(east)]),
    );
    brain
        .crowd
        .bands
        .observe(&v, &c, &mut brain.mind.godlands, false);
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
fn a_venture_trip_ends_at_its_destination() {
    // Live snapshot: a bot about a tile from its kept Venture destination kept wandering around it.
    let (c, mut v) = fixture();
    v.character.level = 20;
    v.quest = None;
    v.objects.clear();
    v.position = Pos { x: 64.5, y: 64.5 };
    v.map.layout = Some("classic-world-02-reconstruction".into());
    let (reached, next) = (Pos { x: 65.5, y: 64.5 }, Pos { x: 110.5, y: 64.5 });
    let mut brain = Brain::new(ARCHETYPES[0], &v);
    brain.mind.godlands.insert(
        "classic-world-02-reconstruction".into(),
        BTreeSet::from([tile(next)]),
    );
    brain
        .crowd
        .bands
        .observe(&v, &c, &mut brain.mind.godlands, false);
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
    // A wounded Knight out of sword reach keeps backing off until no enemy presses it.
    v.objects.truncate(1);
    v.character.class = 0x031e;
    v.character.items[..3].copy_from_slice(&[0xa82, -1, -1]);
    v.character.hp = 300;
    let mut brain = Brain::new(ARCHETYPES[1], &v);
    for _ in 0..40 {
        let d = brain.decide(&v, &c).unwrap();
        if d.goal != Goal::Recover {
            break;
        }
        v.position = d.movement.expect("a recovering player keeps moving");
        brain.goal = d.goal;
        (v.tick, v.time_ms, v.elapsed_ms) = (v.tick + 1, v.time_ms + 50, v.elapsed_ms + 50);
    }
    assert!(v.position.distance(v.objects[0].position) > 15.);
}
