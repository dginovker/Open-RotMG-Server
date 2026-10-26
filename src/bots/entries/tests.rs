use super::*;
use rand::{RngCore, rngs::mock::StepRng};

fn fixture() -> (Content, PlayerView, Brain) {
    let (c, mut v) = quest_tests::fixture();
    v.quest = None;
    v.objects.clear();
    v.objects.push(crate::world::view::VisibleObject {
        id: 3,
        account: None,
        kind: 0x717,
        name: "Pirate Cave".into(),
        position: Pos { x: 11.5, y: 10.5 },
        hp: 0,
        max_hp: 0,
        level: None,
        equipped: vec![],
        contents: vec![],
        blocks: false,
        usable: true,
        condition_mask: Some(0),
    });
    v.dungeon_entries.push(ObservedDungeonEntry {
        sequence: 40,
        source_world: v.session.world,
        portal: 3,
        portal_kind: 0x717,
        entrant: 40,
        account: 2,
        name: "Peer".into(),
        observed_ms: 4900,
        expires_ms: 14900,
    });
    let b = Brain::new(ARCHETYPES[0], &v);
    (c, v, b)
}

#[test]
fn witnessed_entry_declines_latch_per_account_and_portal_not_per_poll() {
    let (_, mut v, _) = fixture();
    let mut entries = Entries::default();
    let eligible = BTreeSet::from([3]);
    assert_eq!(
        entries.choose(&v, &eligible, &mut StepRng::new(u64::MAX, 0)),
        None
    );
    assert_eq!(entries.choose(&v, &eligible, &mut StepRng::new(0, 0)), None);
    v.dungeon_entries[0].sequence = 41;
    v.dungeon_entries[0].entrant = 41;
    assert_eq!(entries.choose(&v, &eligible, &mut StepRng::new(0, 0)), None);
    v.dungeon_entries[0].account = 3;
    assert_eq!(
        entries.choose(&v, &eligible, &mut StepRng::new(0, 0)),
        Some(3)
    );
    assert_eq!(entries.seen.len(), 2);
    entries.target = None;
    v.objects[0].id = 4;
    v.dungeon_entries[0].portal = 4;
    assert_eq!(
        entries.choose(&v, &BTreeSet::from([4]), &mut StepRng::new(0, 0)),
        Some(4)
    );
}

#[test]
fn witnessed_entry_two_distinct_entrants_get_independent_draws_until_commitment() {
    let (_, mut v, _) = fixture();
    let mut second = v.dungeon_entries[0].clone();
    second.account = 3;
    second.entrant = 41;
    second.sequence = 41;
    v.dungeon_entries.push(second);
    let mut entries = Entries::default();
    // First draw refuses; the second draw wraps to zero and accepts the next entrant.
    let mut rng = StepRng::new(u64::MAX, 1);
    assert_eq!(entries.choose(&v, &BTreeSet::from([3]), &mut rng), Some(3));
    assert_eq!(entries.seen.len(), 2);
    let mut rng = StepRng::new(7, 1);
    assert_eq!(entries.choose(&v, &BTreeSet::from([3]), &mut rng), Some(3));
    assert_eq!(rng.next_u64(), 7);
}

#[test]
fn witnessed_entry_requires_current_local_portal_and_bounded_fresh_event() {
    let (_, mut v, _) = fixture();
    let event = v.dungeon_entries[0].clone();
    for case in 0..7 {
        v.dungeon_entries = vec![event.clone()];
        match case {
            0 => v.dungeon_entries.clear(),
            1 => v.dungeon_entries[0].source_world += 1,
            2 => v.dungeon_entries[0].expires_ms = v.elapsed_ms,
            3 => v.dungeon_entries[0].observed_ms = v.elapsed_ms + 1,
            4 => v.dungeon_entries[0].account = v.session.account,
            5 => v.dungeon_entries[0].portal_kind = 0x718,
            6 => v.dungeon_entries[0].portal = 99,
            _ => unreachable!(),
        }
        let mut rng = StepRng::new(7, 1);
        let mut entries = Entries::default();
        assert_eq!(
            entries.choose(&v, &BTreeSet::from([3]), &mut rng),
            None,
            "case{case}"
        );
        assert_eq!(rng.next_u64(), 7);
        assert!(entries.seen.is_empty());
    }
    v.dungeon_entries = (2..260)
        .map(|account| {
            let mut e = event.clone();
            e.account = account;
            e
        })
        .collect();
    let mut entries = Entries::default();
    assert_eq!(
        entries.choose(&v, &BTreeSet::from([3]), &mut StepRng::new(u64::MAX, 0)),
        None
    );
    assert_eq!(entries.seen.len(), 256);
    assert_eq!(
        entries.choose(&v, &BTreeSet::from([3]), &mut StepRng::new(0, 0)),
        None
    );
}

#[test]
fn witnessed_entry_commitment_uses_ordinary_portal_journey_and_expires() {
    let (c, mut v, mut b) = fixture();
    let other = crate::world::view::VisibleObject {
        id: 4,
        account: None,
        kind: 0x717,
        name: "Other cave".into(),
        position: Pos { x: 15.5, y: 10.5 },
        hp: 0,
        max_hp: 0,
        level: None,
        equipped: vec![],
        contents: vec![],
        blocks: false,
        usable: true,
        condition_mask: Some(0),
    };
    v.objects.push(other);
    b.goal = Goal::Portal(4);
    let without = Brain::new(ARCHETYPES[0], &v);
    let mut without = Brain {
        goal: Goal::Portal(4),
        ..without
    };
    v.dungeon_entries.clear();
    assert_eq!(without.decide(&v, &c).unwrap().goal, Goal::Portal(4));
    v.dungeon_entries = fixture().1.dungeon_entries;
    assert_eq!(
        b.entries
            .choose(&v, &BTreeSet::from([3]), &mut StepRng::new(0, 0)),
        Some(3)
    );
    let d = b.decide(&v, &c).unwrap();
    assert_eq!(d.goal, Goal::Portal(3));
    assert!(matches!(d.journey, Some(Journey::Portal(3))));
    assert!(d.chat.is_none());
    v.elapsed_ms = 14900;
    assert_eq!(
        b.entries
            .choose(&v, &BTreeSet::from([3]), &mut StepRng::new(0, 0)),
        None
    );
    assert!(b.entries.target.is_none());
}

#[test]
fn witnessed_entry_all_public_dungeons_use_silent_ordinary_journeys() {
    for (kind, _, _, _) in crate::dungeon::DUNGEONS {
        let (c, mut v, _) = fixture();
        v.character.level = 20;
        assert!(events::ready(&v, &c));
        v.objects[0].kind = kind;
        v.dungeon_entries[0].portal_kind = kind;
        let mut b = Brain::new(ARCHETYPES[0], &v);
        b.entries
            .choose(&v, &BTreeSet::from([3]), &mut StepRng::new(0, 0));
        let d = b.decide(&v, &c).unwrap();
        assert_eq!(d.goal, Goal::Portal(3), "{kind:#x}");
        assert!(matches!(d.journey, Some(Journey::Portal(3))), "{kind:#x}");
        assert!(d.chat.is_none(), "{kind:#x}");
    }
}

#[test]
fn witnessed_entry_preserves_commitments_combat_loot_readiness_and_blocked_routes() {
    let (c, mut v, mut b) = fixture();
    let range = 8.;
    let empty = vec![];
    b.agreement = Some(dialogue::Agreement {
        partner: "Peer".into(),
        player: 40,
        waiting: Some(v.position),
    });
    assert_eq!(b.witnessed_portal(&v, &c, &empty, range), None);
    b.agreement = None;
    b.companion = Some(40);
    assert_eq!(b.witnessed_portal(&v, &c, &empty, range), None);
    b.companion = None;
    b.proposing = true;
    assert_eq!(b.witnessed_portal(&v, &c, &empty, range), None);
    b.proposing = false;
    v.lifecycle.busy = true;
    assert_eq!(b.witnessed_portal(&v, &c, &empty, range), None);
    v.lifecycle.busy = false;
    v.character.hp = 374;
    assert_eq!(b.witnessed_portal(&v, &c, &empty, range), None);
    v.character.hp = 500;
    v.character.level = 1;
    assert_eq!(b.witnessed_portal(&v, &c, &empty, range), None);
    v.character.level = 14;
    for goal in [Goal::Loot(7), Goal::Fight(7), Goal::Hunt(7)] {
        assert_eq!(
            b.witnessed_portal(&v, &c, &[(30., goal, v.position)], range),
            None
        );
    }
    assert_eq!(
        b.witnessed_portal(
            &v,
            &c,
            &[(97., Goal::Survey(50, 50), Pos { x: 50.5, y: 50.5 })],
            range
        ),
        None
    );
    b.blocked.insert(tile(v.objects[0].position), v.tick + 100);
    assert_eq!(b.witnessed_portal(&v, &c, &empty, range), None);
    b.blocked.clear();
    v.objects[0].usable = false;
    assert_eq!(b.witnessed_portal(&v, &c, &empty, range), None);
    assert!(b.entries.seen.is_empty());
    v.objects[0].usable = true;
    b.entries
        .choose(&v, &BTreeSet::from([3]), &mut StepRng::new(0, 0));
    assert_eq!(
        b.witnessed_portal(
            &v,
            &c,
            &[(30., Goal::Loot(7), Pos { x: 100.5, y: 100.5 })],
            range
        ),
        None
    );
    assert_eq!(
        b.witnessed_portal(
            &v,
            &c,
            &[(95., Goal::Portal(3), v.objects[0].position)],
            range
        ),
        Some(3)
    );
}
