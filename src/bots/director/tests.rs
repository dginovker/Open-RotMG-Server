use super::*;

#[test]
fn session_end_waits_for_pending_work_and_promised_company() {
    let (_, mut view) = crate::bots::quest_tests::fixture();
    let mut brain = Brain::new(ARCHETYPES[0], &view);
    assert!(can_finish(&brain, &view));
    view.lifecycle.busy = true;
    assert!(!can_finish(&brain, &view));
    view.lifecycle.busy = false;
    brain.agreement = Some(dialogue::Agreement {
        partner: "Peer".into(),
        player: 42,
        waiting: None,
    });
    assert!(!can_finish(&brain, &view));
    brain.agreement = None;
    brain.group = Some(adventures::Adventure {
        members: vec![],
        purpose: adventures::Purpose::Equipment,
        until: 9999,
        finished: false,
    });
    assert!(!can_finish(&brain, &view));
    brain.group.as_mut().unwrap().finished = true;
    assert!(can_finish(&brain, &view));
    brain.companion = Some(42);
    brain.invitation_until = view.time_ms + 60_000;
    assert!(!can_finish(&brain, &view));
    view.time_ms += 60_000;
    assert!(can_finish(&brain, &view));
}

#[tokio::test]
async fn retiring_a_preset_preserves_old_progress_and_creates_a_distinct_next_session() {
    use crate::store::{Character, FameLifetime, Store};
    let content = Content::load(std::path::Path::new("data")).unwrap();
    let dir = std::env::temp_dir().join(format!("persona-retire-{:016x}", rand::random::<u64>()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("game.db");
    let store = Store::open(&path, &content).unwrap();
    let mut snapshot = Character::fresh(content.get(crate::content::WIZARD).unwrap()).unwrap();
    snapshot.lifetime = FameLifetime::known_only();
    let identity = "persona:v1:Session";
    let old = store
        .persona_character(
            identity.into(),
            vec![snapshot.clone()],
            vec![[0xa1f; 8]],
            0,
            None,
        )
        .await
        .unwrap();
    assert!(
        store
            .retire_persona(identity.into(), old.account + 1)
            .await
            .is_err()
    );
    store
        .retire_persona(identity.into(), old.account)
        .await
        .unwrap();
    let next = store
        .persona_character(identity.into(), vec![snapshot], vec![[0xa34; 8]], 0, None)
        .await
        .unwrap();
    assert_ne!(old.account, next.account);
    assert_eq!(old.name, next.name);
    assert_eq!(
        serde_json::to_value(store.load(old.account, old.id).await.unwrap()).unwrap(),
        serde_json::to_value(&old).unwrap()
    );
    assert_eq!(
        store.vaults(old.account).await.unwrap()[0].items,
        vec![0xa1f; 8]
    );
    assert_eq!(
        store.vaults(next.account).await.unwrap()[0].items,
        vec![0xa34; 8]
    );
    drop(store);
    Store::open(&path, &content).unwrap();
}

#[tokio::test]
async fn retirement_reads_just_published_offers_and_group_acceptance() {
    let (_dir, app) = crate::server::test_app();
    let (_, mut view) = crate::bots::quest_tests::fixture();
    let mut brain = Brain::new(ARCHETYPES[0], &view);
    let own = adventures::Member {
        account: view.session.account,
        name: brain.profile.name.into(),
    };
    let peer = adventures::Member {
        account: own.account + 100,
        name: "Peer".into(),
    };
    app.society.lock().unwrap().apply(
        own.clone(),
        adventures::Plan::Invite {
            peer: peer.clone(),
            purpose: adventures::Purpose::Equipment,
        },
        false,
        "come gear?".into(),
        view.elapsed_ms,
    );
    assert!(brain.offered.is_none());
    assert!(!may_finish(&app, &mut brain, &view));
    let offer = brain.offered.take().unwrap();
    app.society.lock().unwrap().apply(
        peer,
        adventures::Plan::Answer(offer),
        true,
        "coming".into(),
        view.elapsed_ms,
    );
    assert!(brain.group.is_none());
    assert!(!may_finish(&app, &mut brain, &view));
    assert_eq!(brain.group.as_ref().unwrap().members.len(), 2);
    view.elapsed_ms += 600_000;
    assert!(may_finish(&app, &mut brain, &view));
}

#[tokio::test]
async fn drain_blocks_population_writes_and_shutdown_is_not_a_critical_exit() {
    let (_dir, mut app) = crate::server::test_app();
    app.bot_population = 1;
    let profile = persona::PERSONAS[0];
    let character = runtime::persona_character(&app, profile).await.unwrap();
    let (done, mut departed) = mpsc::channel(1);
    done.send(Departure {
        profile,
        account: character.account,
        retire: true,
    })
    .await
    .unwrap();
    let mut online = BTreeSet::from([profile.identity]);
    let mut resting = BTreeMap::new();
    lifecycle::drain::prepare(&app);
    maintain(&app, &done, &mut departed, &mut online, &mut resting)
        .await
        .unwrap();
    assert_eq!(departed.len(), 1);
    assert!(online.contains(profile.identity));
    assert_eq!(
        app.store
            .shore_account(profile.identity.into())
            .await
            .unwrap(),
        Some(character.account)
    );
    let receivers = app.shutdown.receiver_count();
    start(&app).await.unwrap();
    assert_eq!(app.shutdown.receiver_count(), receivers + 1);
    app.shutdown.send_replace(true);
    tokio::time::timeout(Duration::from_secs(2), async {
        while app.shutdown.receiver_count() != receivers {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(app.world.supervisor.latest().unwrap().is_none());
}
