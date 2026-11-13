use super::*;
fn fixture() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE accounts(id INTEGER PRIMARY KEY,name TEXT NOT NULL,guid TEXT);CREATE TABLE credentials(account INTEGER PRIMARY KEY,email TEXT UNIQUE,password_hash TEXT);CREATE TABLE characters(account INTEGER,hp INTEGER);CREATE TABLE password_resets(account INTEGER);CREATE TABLE email_password_resets(account INTEGER);CREATE TABLE recovery_codes(account INTEGER);CREATE TABLE discord_codes(account INTEGER);INSERT INTO accounts VALUES(1,'First','guest1'),(2,'Second','guest2');INSERT INTO characters VALUES(1,123),(2,456);").unwrap();
    db
}
#[test]
fn local_edits_keep_other_accounts_and_progress_and_reject_stale_or_duplicate_names() {
    let mut db = fixture();
    let empty = std::collections::BTreeMap::new();
    let first = local_accounts(&db, &empty).unwrap();
    assert!(
        first
            .iter()
            .all(|a| a.password.is_none() && a.username.is_none())
    );
    let mut edit = LocalAccountEdit {
        account: 1,
        expected: first[0].expected.clone(),
        username: "New@local.invalid".into(),
        password: "private-local-password".into(),
    };
    let tx = db.transaction().unwrap();
    edit_local_account(&tx, &edit).unwrap();
    tx.commit().unwrap();
    let verifier: String = db
        .query_row(
            "SELECT password_hash FROM credentials WHERE account=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let passwords =
        std::collections::BTreeMap::from([(password_key(&verifier), edit.password.clone())]);
    let listed = local_accounts(&db, &passwords).unwrap();
    assert_eq!(listed[0].username.as_deref(), Some("new@local.invalid"));
    assert_eq!(listed[0].password.as_deref(), Some(edit.password.as_str()));
    assert!(listed[1].username.is_none());
    let tx = db.transaction().unwrap();
    assert!(edit_local_account(&tx, &edit).is_err());
    drop(tx);
    edit.account = 2;
    edit.expected = listed[1].expected.clone();
    let tx = db.transaction().unwrap();
    assert!(edit_local_account(&tx, &edit).is_err());
    drop(tx);
    edit.account = 1;
    edit.expected = listed[0].expected.clone();
    edit.password = "another-local-password".into();
    let tx = db.transaction().unwrap();
    edit_local_account(&tx, &edit).unwrap();
    tx.commit().unwrap();
    assert!(
        local_accounts(&db, &passwords).unwrap()[0]
            .password
            .is_none()
    );
    assert_eq!(
        db.query_row("SELECT sum(hp) FROM characters", [], |r| r.get::<_, i32>(0))
            .unwrap(),
        579
    );
    assert_eq!(
        db.query_row("SELECT guid FROM accounts WHERE id=1", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "guest1"
    );
    edit.password = "tiny".into();
    assert!(validate_local_edit(&edit).is_err());
    edit.password = "valid-password".into();
    edit.username = "not-an-email".into();
    assert!(validate_local_edit(&edit).is_err());
}
