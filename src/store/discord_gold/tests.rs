use super::*;

const TARGET: &str = "1558662335076376578";
const ACTOR: &str = "1558662335076376579";
const REQUEST: &str = "1558662335076376580";

#[tokio::test]
async fn discord_gold_preserves_wallet_spending_and_receipts_across_relink_and_restart() {
    let content = Content::load(Path::new("data")).unwrap();
    let dir = std::env::temp_dir().join(format!("discord-gold-{}", rand::random::<u64>()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("game.db");
    let s = Store::open(&path, &content).unwrap();
    let account = s
        .register(
            "owner".into(),
            "owner@example.org".into(),
            "password".into(),
        )
        .await
        .unwrap();
    assert!(
        s.grant_discord_gold(TARGET.into(), 100, REQUEST.into(), ACTOR.into())
            .await
            .is_err()
    );
    let code = s.issue_discord_code(account).await.unwrap();
    s.redeem_discord_code(code, TARGET.into()).await.unwrap();
    assert_eq!(s.gold(account).await.unwrap().balance, 200);
    for (amount, request, actor) in [
        (0, REQUEST, ACTOR),
        (-1, REQUEST, ACTOR),
        (100, "bad", ACTOR),
        (100, REQUEST, "bad"),
    ] {
        assert!(
            s.grant_discord_gold(TARGET.into(), amount, request.into(), actor.into())
                .await
                .is_err()
        );
    }
    let grant = || s.grant_discord_gold(TARGET.into(), 100, REQUEST.into(), ACTOR.into());
    let (first, duplicate) = tokio::join!(grant(), grant());
    assert_eq!(first.unwrap(), (account, 300));
    assert_eq!(duplicate.unwrap(), (account, 300));
    assert!(
        s.grant_discord_gold(TARGET.into(), 101, REQUEST.into(), ACTOR.into())
            .await
            .is_err()
    );
    assert!(
        s.grant_discord_gold(TARGET.into(), 100, REQUEST.into(), TARGET.into())
            .await
            .is_err()
    );
    let db = Connection::open(&path).unwrap();
    db.execute(
        "UPDATE accounts SET credits=credits-50 WHERE id=?1",
        [account],
    )
    .unwrap();
    s.unlink_discord(TARGET.into()).await.unwrap();
    let other = s
        .register(
            "other".into(),
            "other@example.org".into(),
            "password".into(),
        )
        .await
        .unwrap();
    let code = s.issue_discord_code(other).await.unwrap();
    s.redeem_discord_code(code, TARGET.into()).await.unwrap();
    assert_eq!(grant().await.unwrap(), (account, 250));
    assert_eq!(s.gold(other).await.unwrap().balance, 200);
    assert!(
        s.grant_discord_gold(
            TARGET.into(),
            i32::MAX,
            "1558662335076376581".into(),
            ACTOR.into()
        )
        .await
        .is_err()
    );
    let cap = i32::MAX - 200;
    s.grant_discord_gold(
        TARGET.into(),
        cap,
        "1558662335076376581".into(),
        ACTOR.into(),
    )
    .await
    .unwrap();
    assert!(
        s.grant_discord_gold(TARGET.into(), 1, "1558662335076376582".into(), ACTOR.into())
            .await
            .is_err()
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM discord_gold_grants", [], |r| r
            .get::<_, i32>(0))
            .unwrap(),
        2
    );
    drop(s);
    let s = Store::open(&path, &content).unwrap();
    assert_eq!(
        s.grant_discord_gold(TARGET.into(), 100, REQUEST.into(), ACTOR.into())
            .await
            .unwrap(),
        (account, 250)
    );
    assert_eq!(s.gold(other).await.unwrap().balance, i32::MAX);
    drop(s);
    db.execute("DROP TABLE discord_gold_grants", []).unwrap();
    assert!(
        Store::open(&path, &content)
            .err()
            .unwrap()
            .to_string()
            .contains("schema-19 Discord gold receipts")
    );
    db.pragma_update(None, "user_version", 18).unwrap();
    let s = Store::open(&path, &content).unwrap();
    assert_eq!(s.gold(account).await.unwrap().balance, 250);
    assert_eq!(s.gold(other).await.unwrap().balance, i32::MAX);
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
            .unwrap(),
        19
    );
}
