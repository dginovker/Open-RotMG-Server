use super::*;
use crate::store::{InventoryWrite, Store};

#[tokio::test]
async fn carried_stock_is_withdrawn_not_reminted_after_banking_and_roster_loss() {
    let content = Content::load(std::path::Path::new("data")).unwrap();
    let p = PERSONAS
        .iter()
        .find(|p| p.role == Role::Trader && p.stars >= 26)
        .unwrap();
    let kind = starting_stock(p, &content).unwrap().unwrap();
    let build = || {
        life(
            p,
            &content,
            &mut rand::rngs::StdRng::seed_from_u64(seed(p.identity)),
        )
        .unwrap()
    };
    let dir = std::env::temp_dir().join(format!("persona-stock-{:016x}", rand::random::<u64>()));
    std::fs::create_dir(&dir).unwrap();
    let store = Store::open(&dir.join("game.db"), &content).unwrap();
    let (roster, vault) = build();
    let expected = vault.iter().flatten().filter(|item| **item == kind).count();
    let mut ch = store
        .persona_character(p.identity.into(), roster, vault, 0, Some(kind))
        .await
        .unwrap();
    assert!(ch.items[7..].contains(&kind));
    for cycle in 0..3 {
        let mut vaults = store.vaults(ch.account).await.unwrap();
        assert_eq!(
            vaults
                .iter()
                .flat_map(|v| &v.items)
                .chain(&ch.items)
                .filter(|item| **item == kind)
                .count(),
            expected
        );
        let carried = ch.items.iter().filter(|item| **item == kind).count();
        let original_vaults = vaults.clone();
        for item in ch.items.iter_mut().filter(|item| **item == kind) {
            if cycle != 2 {
                *vaults
                    .iter_mut()
                    .flat_map(|v| &mut v.items)
                    .find(|item| **item == -1)
                    .unwrap() = *item;
            }
            *item = -1;
        }
        store
            .exchange(
                vec![InventoryWrite {
                    id: ch.id,
                    account: ch.account,
                    revision: ch.revision,
                    items: ch.items.clone(),
                }],
                vaults
                    .into_iter()
                    .zip(original_vaults)
                    .filter_map(|(now, old)| (now.items != old.items).then_some(now))
                    .collect(),
            )
            .await
            .unwrap();
        let db = rusqlite::Connection::open(dir.join("game.db")).unwrap();
        let guid: String = db
            .query_row("SELECT guid FROM accounts WHERE id=?1", [ch.account], |r| {
                r.get(0)
            })
            .unwrap();
        drop(db);
        for living in store.list(guid).await.unwrap().characters {
            store.delete_character(ch.account, living.id).await.unwrap();
        }
        let (roster, vault) = build();
        ch = store
            .persona_character(p.identity.into(), roster, vault, 0, Some(kind))
            .await
            .unwrap();
        if cycle == 2 {
            let retained = store
                .vaults(ch.account)
                .await
                .unwrap()
                .iter()
                .flat_map(|v| &v.items)
                .chain(&ch.items)
                .filter(|item| **item == kind)
                .count();
            assert_eq!(
                retained,
                expected - carried,
                "lost stock was manufactured again"
            );
        }
    }
}
