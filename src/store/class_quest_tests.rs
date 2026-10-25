use super::*;

fn fixture() -> (Content, Store) {
    let content = Content::load(Path::new("data")).unwrap();
    let store = Store::open(Path::new(":memory:"), &content).unwrap();
    (content, store)
}

async fn rogue(store: &Store, content: &Content, guid: &str) -> Character {
    let account = store.list(guid.into()).await.unwrap();
    let mut ch = store
        .create_prepared(account.id, content.get(0x300).unwrap(), None)
        .await
        .unwrap();
    ch.level = 20;
    ch.exp = 18_050;
    ch.fame = 18;
    store.progress(ch.clone()).await.unwrap();
    ch
}

async fn die(store: &Store, mut ch: Character) -> i32 {
    ch.alive = false;
    ch.hp = 0;
    let (account, id) = (ch.account, ch.id);
    store.death(ch, "Medusa".into()).await.unwrap();
    let xml = store.fame(account, id).await.unwrap();
    roxmltree::Document::parse(&xml)
        .unwrap()
        .descendants()
        .find(|n| n.has_tag_name("TotalFame"))
        .unwrap()
        .text()
        .unwrap()
        .parse()
        .unwrap()
}

#[tokio::test]
async fn death_bonuses_unlock_class_stars_from_validated_archives() {
    let (content, store) = fixture();
    let account = store.list("death-class-stars".into()).await.unwrap();
    let predecessor = store
        .create(account.id, content.get(crate::content::WIZARD).unwrap())
        .await
        .unwrap();
    assert!(die(&store, predecessor).await < 31);
    let mut ch = rogue(&store, &content, "death-class-stars").await;
    ch.lifetime.shots = 100;
    ch.lifetime.projectile_hits = 1;
    ch.lifetime.inventory_potion_used = true;
    ch.lifetime.ability_used = true;
    ch.lifetime.teleported = true;
    let mut living = store.presentation(ch.account).await.unwrap();
    living.best_fame.retain(|class, _| *class == ch.class);
    assert_eq!(living.best_fame[&ch.class], 18);
    assert_eq!(living.stars(), 0);
    let total = die(&store, ch.clone()).await;
    assert_eq!(total, 34);
    let mut dead = store.presentation(ch.account).await.unwrap();
    dead.best_fame.retain(|class, _| *class == ch.class);
    assert_eq!(dead.best_fame[&ch.class], total);
    assert_eq!(dead.stars(), 1);
    assert_eq!(dead.next_quest(ch.class, 0), 150);
    assert!(
        store
            .fame(ch.account, ch.id)
            .await
            .unwrap()
            .contains("<BaseFame>18</BaseFame>")
    );
    store
        .call(move |db| {
            db.execute(
                "UPDATE account_fame SET balance=0 WHERE account=?",
                [ch.account],
            )?;
            db.execute(
                "UPDATE characters SET stats=json_set(stats,'$.fame',99999) WHERE id=?",
                [ch.id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let retained = store.list("death-class-stars".into()).await.unwrap();
    assert_eq!(retained.fame, 0);
    assert_eq!(retained.best_fame[&0x300], total);
    assert!(
        retained
            .xml("", 0.)
            .contains(&format!("<BestFame>{total}</BestFame>"))
    );
    store.call(|db| {
        db.execute("UPDATE deaths SET receipt=json_set(receipt,'$.total',json_extract(receipt,'$.total')+1)", [])?;
        Ok(())
    }).await.unwrap();
    assert!(store.presentation(retained.id).await.is_err());
}

#[tokio::test]
async fn class_stars_keep_single_character_max_not_wallet_or_lifetime_sum() {
    let (content, store) = fixture();
    let first = rogue(&store, &content, "separate-class-stars").await;
    let account = first.account;
    let first_total = die(&store, first).await;
    let second = rogue(&store, &content, "separate-class-stars").await;
    let second_total = die(&store, second).await;
    assert_eq!(first_total, 151);
    assert!(second_total < first_total);
    assert!(first_total + second_total >= 150);
    let mut wizard = store
        .create(account, content.get(crate::content::WIZARD).unwrap())
        .await
        .unwrap();
    wizard.level = 20;
    wizard.exp = 20_000;
    wizard.fame = 20;
    store.progress(wizard).await.unwrap();
    let result = store.presentation(account).await.unwrap();
    assert_eq!(result.fame, first_total + second_total);
    assert_eq!(result.best_fame[&0x300], first_total.max(second_total));
    assert_eq!(result.best_fame[&crate::content::WIZARD], 20);
    assert_eq!(result.stars(), 3);
}
