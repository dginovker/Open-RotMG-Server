use super::*;
use crate::world::view::VisibleObject;

fn fixture() -> (Content, PlayerView) {
    let (c, mut v) = crate::bots::quest_tests::fixture();
    v.objects.clear();
    v.quest = None;
    (c, v)
}
fn bag(id: i32, item: i32, x: f32) -> VisibleObject {
    VisibleObject {
        id,
        account: None,
        kind: 0x500,
        name: String::new(),
        position: Pos { x, y: 10.5 },
        hp: 0,
        max_hp: 0,
        level: None,
        equipped: vec![],
        contents: vec![item],
        blocks: false,
        usable: false,
        condition_mask: None,
    }
}
fn advance(v: &mut PlayerView, ms: u64) {
    v.tick += ms / 50;
    v.time_ms += ms;
    v.elapsed_ms += ms;
}
fn intentions(brain: &Brain) -> &Intentions {
    brain.mind.intentions.as_ref().unwrap()
}

#[test]
fn main_goal_changes_real_loot_choice_and_survives_safety() {
    let (c, mut v) = fixture();
    v.objects = vec![bag(3, 0xa1f, 11.5), bag(4, 0xa21, 18.5)];
    let mut leveling = Brain::new(ARCHETYPES[0], &v);
    assert_eq!(leveling.decide(&v, &c).unwrap().goal, Goal::Loot(3));
    assert_eq!(leveling.intent["goal"]["label"], "Reach level 20");
    assert_eq!(leveling.intent["mode"], "optional");
    v.character.level = 20;
    let mut maxing = Brain::new(ARCHETYPES[0], &v);
    assert_eq!(maxing.decide(&v, &c).unwrap().goal, Goal::Loot(4));
    assert_eq!(maxing.intent["goal"]["label"], "Max permanent Speed");
    assert_eq!(maxing.intent["mode"], "progress");
    let goal = maxing.intent["goal"].clone();
    v.character.hp = 10;
    advance(&mut v, 500);
    let d = maxing.decide(&v, &c).unwrap();
    assert!(matches!(d.goal, Goal::Recover));
    assert_eq!(maxing.intent["goal"], goal);
    assert_eq!(maxing.intent["mode"], "necessary");
    assert!(!maxing.intent["resume"].is_null());
}

#[test]
fn detour_needs_committed_item_then_main_time_and_progress_before_reentry() {
    let (c, mut v) = fixture();
    v.objects.push(bag(3, 0xa1f, 11.5));
    let mut b = Brain::new(ARCHETYPES[0], &v);
    let d = b.decide(&v, &c).unwrap();
    assert!(matches!(d.item, Some(Action::Swap { .. })));
    let token = intentions(&b).main.clone();
    advance(&mut v, 500);
    b.decide(&v, &c).unwrap();
    assert!(
        intentions(&b).detour.is_some(),
        "dispatch is not collection"
    );
    v.character.items[4] = 0xa1f;
    v.character.inventory_revision += 1;
    advance(&mut v, 500);
    let d = b.decide(&v, &c).unwrap();
    assert_ne!(d.goal, Goal::Loot(3));
    assert!(intentions(&b).detour.is_none());
    assert_eq!(intentions(&b).main, token);
    assert!(!intentions(&b).eligible(&v, &Goal::Loot(3), false));
    v.character.items[4] = -1;
    v.character.base_stats[2] += 1;
    v.character.inventory_revision += 1;
    advance(&mut v, 500);
    b.decide(&v, &c).unwrap();
    advance(&mut v, MAIN_MS + 500);
    b.decide(&v, &c).unwrap();
    assert!(
        !intentions(&b).eligible(&v, &Goal::Loot(3), false),
        "walking/dispatch alone cannot rearm opportunities"
    );
    v.character.experience += 10;
    advance(&mut v, 500);
    b.decide(&v, &c).unwrap();
    assert!(
        intentions(&b).detour.is_some(),
        "actual main XP plus main work rearms one opportunity"
    );
}

#[test]
fn optional_timeout_is_an_actual_choice_change_not_just_a_label() {
    let (c, mut v) = fixture();
    v.objects.push(bag(3, 0xa1f, 11.5));
    let mut b = Brain::new(ARCHETYPES[0], &v);
    assert_eq!(b.decide(&v, &c).unwrap().goal, Goal::Loot(3));
    advance(&mut v, DETOUR_MS);
    let d = b.decide(&v, &c).unwrap();
    assert_ne!(d.goal, Goal::Loot(3));
    assert_eq!(b.intent["mode"], "progress");
    assert!(
        b.intent["reason"]
            .as_str()
            .unwrap()
            .contains("time budget exhausted")
    );
    assert!(intentions(&b).detour.is_none());
    b.mind.validate().unwrap();
}

#[test]
fn optional_missing_object_aborts_and_cannot_be_replaced_by_a_new_opportunity() {
    let (c, mut v) = fixture();
    let mut i = Intentions::default();
    i.observe(&v, &c).unwrap();
    v.objects.push(bag(3, 0xa1f, 11.5));
    i.selected(&v, &c, &Goal::Loot(3), false, None).unwrap();
    v.objects = vec![bag(4, 0xa1f, 11.5)];
    advance(&mut v, 500);
    i.observe(&v, &c).unwrap();
    assert!(i.detour.is_none());
    assert!(!i.eligible(&v, &Goal::Loot(4), false));
}

#[test]
fn one_detour_only_and_accepted_agreements_are_not_suppressed() {
    let (c, mut v) = fixture();
    let mut i = Intentions::default();
    i.observe(&v, &c).unwrap();
    v.objects = vec![bag(3, 0xa1f, 11.5), bag(4, 0xa1f, 12.5)];
    i.selected(&v, &c, &Goal::Loot(3), false, None).unwrap();
    assert!(i.eligible(&v, &Goal::Loot(3), false));
    assert!(!i.eligible(&v, &Goal::Loot(4), false));
    assert!(i.eligible(&v, &Goal::Loot(4), true));
    advance(&mut v, DETOUR_MS);
    i.observe(&v, &c).unwrap();
    assert!(i.detour.is_none());
    assert!(i.eligible(&v, &Goal::Loot(4), true));
    assert!(!i.eligible(&v, &Goal::Loot(4), false));
}

#[test]
fn bounded_dungeon_survives_same_runtime_travel_but_exits_on_budget() {
    let (c, mut v) = fixture();
    let mut i = Intentions::default();
    i.bind(&v, 7);
    i.observe(&v, &c).unwrap();
    let mut portal = bag(3, -1, 11.5);
    portal.kind = 0x890;
    v.objects.push(portal);
    i.selected(&v, &c, &Goal::Portal(3), false, None).unwrap();
    v.map.name = "Mad Lab".into();
    v.session.world = 40;
    v.session.player = 10;
    i.bind(&v, 7);
    i.observe(&v, &c).unwrap();
    assert!(matches!(
        i.detour.as_ref().unwrap().opportunity,
        Opportunity::Dungeon { entered: true, .. }
    ));
    advance(&mut v, DETOUR_MS);
    i.observe(&v, &c).unwrap();
    assert!(i.expired());
    i.validate().unwrap();
    let mut b = Brain::new(ARCHETYPES[0], &v);
    b.mind.intentions = Some(i);
    assert!(matches!(
        b.decide(&v, &c).unwrap().journey,
        Some(Journey::Nexus)
    ));
    assert!(
        b.intent["reason"]
            .as_str()
            .unwrap()
            .contains("time budget exhausted")
    );
}

#[test]
fn completing_a_goal_inside_optional_dungeon_preserves_its_bounded_return() {
    for elapsed in [1000, DETOUR_MS] {
        let (c, mut v) = fixture();
        v.character.level = 19;
        let mut i = Intentions::default();
        i.bind(&v, 7);
        i.observe(&v, &c).unwrap();
        let old = i.main.as_ref().unwrap().id;
        let mut portal = bag(3, -1, 11.5);
        portal.kind = 0x719;
        v.objects.push(portal);
        i.selected(&v, &c, &Goal::Portal(3), false, None).unwrap();
        v.map.name = crate::dungeon::DUNGEONS
            .iter()
            .find(|(kind, _, _, _)| *kind == 0x719)
            .unwrap()
            .2
            .into();
        v.session.world = 40;
        i.bind(&v, 7);
        i.observe(&v, &c).unwrap();
        v.character.level = 20;
        advance(&mut v, elapsed);
        i.observe(&v, &c).unwrap();
        assert!(i.main.as_ref().unwrap().id > old);
        assert_eq!(i.main.as_ref().unwrap().objective, Objective::Speed);
        if elapsed < DETOUR_MS {
            assert_eq!(
                i.detour
                    .as_ref()
                    .expect("goal rollover must retain the current excursion deadline")
                    .remaining_ms,
                DETOUR_MS - elapsed
            );
        } else {
            assert!(
                i.expired(),
                "goal rollover must not erase the deadline's required return"
            );
            let mut brain = Brain::new(ARCHETYPES[0], &v);
            brain.mind.intentions = Some(i);
            assert!(matches!(
                brain.decide(&v, &c).unwrap().journey,
                Some(Journey::Nexus)
            ));
        }
    }
}

#[test]
fn company_detour_ends_on_observed_arrival_and_clock_bookkeeping_is_not_progress() {
    let (c, mut v) = fixture();
    v.character.level = 20;
    let mut i = Intentions::default();
    i.observe(&v, &c).unwrap();
    let before = i.clone();
    advance(&mut v, 500);
    i.observe(&v, &c).unwrap();
    assert_eq!(
        before, i,
        "clock-only observation must not cause a durable write"
    );
    let mut peer = bag(3, -1, 18.5);
    peer.kind = v.character.class;
    peer.hp = 500;
    v.objects.push(peer);
    i.selected(&v, &c, &Goal::Follow(3), false, None).unwrap();
    v.objects[0].position = v.position;
    advance(&mut v, 500);
    i.observe(&v, &c).unwrap();
    assert!(i.detour.is_none());
    assert!(i.transition.contains("reached company"));
    v.elapsed_ms -= 1;
    assert!(i.observe(&v, &c).is_err());
}

#[test]
fn restart_aborts_object_handles_preserves_goal_and_cooldown() {
    let (c, mut v) = fixture();
    let mut mind = pursuits::Mind::new(v.session.account);
    mind.bind(&v, 1).unwrap();
    let i = mind.intentions.as_mut().unwrap();
    i.observe(&v, &c).unwrap();
    let token = i.main.clone();
    v.objects.push(bag(3, 0xa1f, 11.5));
    i.selected(&v, &c, &Goal::Loot(3), false, None).unwrap();
    let raw = serde_json::to_string(&mind).unwrap();
    let mut loaded: pursuits::Mind = serde_json::from_str(&raw).unwrap();
    loaded.validate().unwrap();
    loaded.bind(&v, 2).unwrap();
    let i = loaded.intentions.as_ref().unwrap();
    assert_eq!(i.main, token);
    assert!(i.detour.is_none());
    assert!(!i.eligible(&v, &Goal::Loot(3), false));
}

#[test]
fn permanent_speed_uses_class_cap_not_gear_and_fame_uses_native_own_fact() {
    let (c, mut v) = fixture();
    v.character.level = 20;
    let cap = c.classes[&v.character.class].caps[4];
    v.character.stats[4] = cap + 10;
    let mut i = Intentions::default();
    i.observe(&v, &c).unwrap();
    assert_eq!(i.main.as_ref().unwrap().objective, Objective::Speed);
    assert_eq!(i.main.as_ref().unwrap().target, cap);
    let speed_id = i.main.as_ref().unwrap().id;
    v.character.base_stats[4] = cap;
    v.character.fame = 199;
    v.character.experience = 999_999;
    i.observe(&v, &c).unwrap();
    let fame = i.main.clone().unwrap();
    assert_eq!(fame.objective, Objective::Fame);
    assert_eq!(fame.target, 200);
    assert!(fame.id > speed_id);
    let d = Decision {
        goal: Goal::Wait,
        movement: None,
        aim: None,
        item: None,
        journey: None,
        chat: None,
    };
    let payload = i.payload(&v, Activity::Adventure, &d, "waiting".into(), false);
    assert_eq!(payload["goal"]["current"], 199);
    assert_eq!(payload["goal"]["label"], "Earn 200 base fame");
    v.lifecycle.busy = true;
    v.character.fame = 200;
    i.observe(&v, &c).unwrap();
    assert_eq!(
        i.main.as_ref().unwrap(),
        &fame,
        "uncommitted snapshots cannot complete goals"
    );
    v.lifecycle.busy = false;
    i.observe(&v, &c).unwrap();
    assert_eq!(i.main.as_ref().unwrap().target, 400);
    let next = i.main.clone();
    i.observe(&v, &c).unwrap();
    assert_eq!(i.main, next, "no per-decision token churn");
    v.character.fame = i32::MAX;
    assert!(i.observe(&v, &c).is_err());
}

#[test]
fn death_and_replacement_cannot_inherit_character_goal_or_detour() {
    let (c, mut v) = fixture();
    let mut mind = pursuits::Mind::new(v.session.account);
    mind.intentions.as_mut().unwrap().observe(&v, &c).unwrap();
    let old = mind.intentions.as_ref().unwrap().main.clone().unwrap();
    mind.end(&v, "character_died_steps_invalidated");
    assert!(mind.intentions.as_ref().unwrap().main.is_none());
    v.session.character += 1;
    mind.intentions.as_mut().unwrap().observe(&v, &c).unwrap();
    let new = mind.intentions.as_ref().unwrap().main.clone().unwrap();
    assert_ne!(old.character, new.character);
    assert!(new.id > old.id);
    v.session.character += 1;
    mind.bind(&v, 9).unwrap();
    assert!(
        mind.intentions.as_ref().unwrap().main.is_none(),
        "bind must invalidate before persisting a replacement"
    );
}

#[test]
fn ticking_detour_and_cooldown_do_not_dirty_mind_and_unrelated_xp_does_not_rearm_speed() {
    let (c, mut v) = fixture();
    v.character.level = 20;
    let mut i = Intentions::default();
    i.observe(&v, &c).unwrap();
    v.objects.push(bag(3, 0xa1f, 11.5));
    i.selected(&v, &c, &Goal::Loot(3), false, None).unwrap();
    let before = i.clone();
    advance(&mut v, 500);
    i.observe(&v, &c).unwrap();
    assert!(i.detour.as_ref().unwrap().remaining_ms < before.detour.as_ref().unwrap().remaining_ms);
    assert_eq!(
        i, before,
        "ticking a detour must not trigger another Store write"
    );
    i.finish_detour(&v, "test abort");
    i.progressing = true;
    let before = i.clone();
    advance(&mut v, 500);
    i.observe(&v, &c).unwrap();
    assert!(i.main_remaining_ms < before.main_remaining_ms);
    assert_eq!(
        i, before,
        "ticking a cooldown must not trigger another Store write"
    );
    i.main_remaining_ms = 0;
    v.character.experience += 100;
    assert!(!i.available(&v), "XP is not permanent Speed progress");
    v.character.base_stats[4] += 1;
    assert!(i.available(&v));
    v.character.base_stats[4] = c.classes[&v.character.class].caps[4];
    i.observe(&v, &c).unwrap();
    assert!(i.detour.is_none() && !i.needs_progress && i.main_remaining_ms == 0);
    assert_eq!(i.main.as_ref().unwrap().objective, Objective::Fame);
}

#[test]
fn legacy_mind_explicitly_upgrades_but_malformed_new_state_is_rejected() {
    let (c, v) = fixture();
    let mut legacy = serde_json::to_value(pursuits::Mind::new(v.session.account)).unwrap();
    legacy["schema"] = serde_json::json!(1);
    legacy.as_object_mut().unwrap().remove("intentions");
    let mut mind: pursuits::Mind = serde_json::from_value(legacy).unwrap();
    mind.validate().unwrap();
    mind.bind(&v, 1).unwrap();
    assert_eq!(mind.schema, 2);
    mind.intentions.as_mut().unwrap().observe(&v, &c).unwrap();
    mind.validate().unwrap();
    let saved = serde_json::to_value(&mind).unwrap();
    for (field, value) in [
        ("next_id", serde_json::json!(SAFE_ID + 1)),
        ("main_remaining_ms", serde_json::json!(MAIN_MS + 1)),
    ] {
        let mut invalid = saved.clone();
        invalid["intentions"][field] = value;
        assert!(
            serde_json::from_value::<pursuits::Mind>(invalid)
                .unwrap()
                .validate()
                .is_err()
        );
    }
    let mut invalid = saved.clone();
    invalid["intentions"]["main"]["character"] = serde_json::json!(0);
    assert!(
        serde_json::from_value::<pursuits::Mind>(invalid)
            .unwrap()
            .validate()
            .is_err()
    );
    let mut invalid = saved;
    invalid["schema"] = serde_json::json!(1);
    assert!(
        serde_json::from_value::<pursuits::Mind>(invalid)
            .unwrap()
            .validate()
            .is_err()
    );
}

#[tokio::test]
async fn persisted_main_goal_and_detour_reopen_without_json_loss() {
    let (c, mut v) = fixture();
    let directory = std::env::temp_dir().join(format!("bot-intentions-{}", rand::random::<u64>()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("game.db");
    let store = crate::store::Store::open(&path, &c).unwrap();
    let account = store.list("intentions-reopen".into()).await.unwrap();
    v.session.account = account.id;
    let mut mind = pursuits::Mind::new(account.id);
    mind.bind(&v, 1).unwrap();
    mind.intentions.as_mut().unwrap().observe(&v, &c).unwrap();
    v.objects.push(bag(3, 0xa1f, 11.5));
    mind.intentions
        .as_mut()
        .unwrap()
        .selected(&v, &c, &Goal::Loot(3), false, None)
        .unwrap();
    mind.revision = store.save_bot_mind(mind.clone()).await.unwrap();
    drop(store);
    let reopened = crate::store::Store::open(&path, &c).unwrap();
    let saved = reopened.bot_mind(account.id).await.unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(saved).unwrap(),
        serde_json::to_value(mind).unwrap()
    );
    drop(reopened);
    std::fs::remove_dir_all(directory).unwrap();
}
