#[tokio::test]
async fn full_admission_is_retryable_without_fencing_or_stealing_a_human_slot() {
    let (_dir, mut app) = super::super::account_actions::tests::fixture();
    app.max_players = 1;
    let human = app.store.list("admission-human".into()).await.unwrap().id;
    let waiting = app.store.list("admission-waiting".into()).await.unwrap().id;
    let owner = app.claim(human).unwrap();
    assert!(app.try_claim(waiting).unwrap().is_none());
    assert_eq!(
        app.claim(waiting).err().unwrap().to_string(),
        "development world full"
    );
    assert_eq!(app.owners.lock().unwrap().len(), 1);
    assert!(!app.world.supervisor.account_fenced(waiting).unwrap());
    drop(owner);
    assert!(app.try_claim(waiting).unwrap().is_some());
}
