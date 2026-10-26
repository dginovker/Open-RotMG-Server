use super::*;

async fn fixture(path: &Path) -> (Store, Character) {
    let content = Content::load(Path::new("data")).unwrap();
    let s = Store::open(path, &content).unwrap();
    let account = s.list("dye-consumables".into()).await.unwrap();
    let mut ch = s
        .create(account.id, content.get(WIZARD).unwrap())
        .await
        .unwrap();
    ch.items = vec![
        0xa97, 0xa2e, 0xa5c, 0xa4f, 0x1000, 0xa50, 0xa50, 0xa50, 0xa50, 0xa50, 0xa50, 0xa50,
    ];
    ch.revision = s.exchange(vec![write(&ch)], vec![]).await.unwrap()[0];
    let id = ch.id;
    s.call(move |db| {
        db.execute(
            "UPDATE characters SET stats=json_set(stats,'$.tex1',17,'$.tex2',23) WHERE id=?1",
            [id],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let ch = s.load(ch.account, ch.id).await.unwrap();
    (s, ch)
}

fn write(ch: &Character) -> InventoryWrite {
    InventoryWrite {
        id: ch.id,
        account: ch.account,
        revision: ch.revision,
        items: ch.items.clone(),
    }
}

async fn snapshot(s: &Store, id: i32) -> (String, String, i32, bool) {
    s.call(move |db| {
        Ok(db.query_row(
            "SELECT stats,items,revision,alive FROM characters WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?)
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn ground_dyes_and_cloths_preserve_full_inventory_and_other_channel_durably() {
    let path = std::env::temp_dir().join(format!("ground-dye-{}.db", rand::random::<u64>()));
    let (s, mut ch) = fixture(&path).await;
    let inventory = ch.items.clone();
    assert!(inventory.iter().all(|item| *item != -1));
    for (kind, channel, texture) in [
        (0x1000, 0, 0x01f0f8ff),
        (0x1100, 1, 0x01f0f8ff),
        (0x1200, 0, 0x04000000),
        (0x1300, 1, 0x04000000),
    ] {
        let mut expected = [ch.tex1, ch.tex2];
        expected[channel] = texture;
        let request = write(&ch);
        let revision = s
            .consume_ground_dye(request.clone(), kind, expected)
            .await
            .unwrap();
        assert_eq!(revision, ch.revision + 1);
        assert!(
            s.consume_ground_dye(request, kind, expected)
                .await
                .unwrap_err()
                .to_string()
                .contains("stale")
        );
        ch.revision = revision;
        [ch.tex1, ch.tex2] = expected;
        let content = Content::load(Path::new("data")).unwrap();
        let reopened = Store::open(&path, &content).unwrap();
        let saved = reopened.load(ch.account, ch.id).await.unwrap();
        assert_eq!(
            serde_json::to_value(&saved).unwrap(),
            serde_json::to_value(&ch).unwrap()
        );
        assert_eq!(saved.items, inventory);
        ch = saved;
    }
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn dye_sources_reject_invalid_reservations_without_mutating_character() {
    let (s, ch) = fixture(Path::new(":memory:")).await;
    let before = snapshot(&s, ch.id).await;
    for ground in [false, true] {
        for case in 0..14 {
            let mut request = write(&ch);
            if !ground {
                request.items[4] = -1;
            }
            let mut kind = 0x1000;
            let mut expected = [0x01f0f8ff, ch.tex2];
            let error = match case {
                0 => {
                    request.account += 1;
                    "foreign"
                }
                1 => {
                    request.id += 1;
                    "conflict"
                }
                2 => {
                    request.revision += 1;
                    "stale"
                }
                3 => {
                    request.revision = -1;
                    "revision"
                }
                4 => {
                    request.revision = i32::MAX;
                    "revision"
                }
                5 => {
                    request.items.pop();
                    "inventory"
                }
                6 => {
                    request.items[11] = 65536;
                    "inventory"
                }
                7 => {
                    request.items[11] = -2;
                    "inventory"
                }
                8 => {
                    request.items[11] = -1;
                    if ground {
                        "preserve inventory"
                    } else {
                        "reserved slot"
                    }
                }
                9 => {
                    kind = 0xa50;
                    if ground {
                        "not a selected dye or cloth"
                    } else {
                        "item reservation"
                    }
                }
                10 => {
                    expected[0] = 99;
                    "appearance reservation"
                }
                11 => {
                    expected[1] += 1;
                    "appearance reservation"
                }
                12 => {
                    request.items[4] = if ground { -1 } else { 0x1000 };
                    if ground {
                        "preserve inventory"
                    } else {
                        "invalid dye inventory"
                    }
                }
                13 => {
                    request.items[4] = 0x1100;
                    if ground {
                        "preserve inventory"
                    } else {
                        "invalid dye inventory"
                    }
                }
                _ => unreachable!(),
            };
            let result = if ground {
                s.consume_ground_dye(request, kind, expected).await
            } else {
                s.consume_dye(request, 4, kind, expected).await
            };
            let message = result.unwrap_err().to_string();
            assert!(
                message.contains(error),
                "ground={ground} case={case}: {message}"
            );
            assert_eq!(snapshot(&s, ch.id).await, before);
        }
    }
    let id = ch.id;
    s.call(move |db| {
        db.execute("UPDATE characters SET alive=0 WHERE id=?1", [id])?;
        Ok(())
    })
    .await
    .unwrap();
    let dead = snapshot(&s, ch.id).await;
    assert!(
        s.consume_ground_dye(write(&ch), 0x1000, [0x01f0f8ff, ch.tex2])
            .await
            .unwrap_err()
            .to_string()
            .contains("dead")
    );
    let mut request = write(&ch);
    request.items[4] = -1;
    assert!(
        s.consume_dye(request, 4, 0x1000, [0x01f0f8ff, ch.tex2])
            .await
            .unwrap_err()
            .to_string()
            .contains("dead")
    );
    assert_eq!(snapshot(&s, ch.id).await, dead);
}

#[tokio::test]
async fn inventory_dyes_and_cloths_consume_only_the_validated_slot_atomically() {
    let (s, mut ch) = fixture(Path::new(":memory:")).await;
    for (kind, channel, texture) in [
        (0x1000, 0, 0x01f0f8ff),
        (0x1100, 1, 0x01f0f8ff),
        (0x1200, 0, 0x04000000),
        (0x1300, 1, 0x04000000),
    ] {
        let mut request = write(&ch);
        request.items[4] = i32::from(kind);
        ch.revision = s.exchange(vec![request.clone()], vec![]).await.unwrap()[0];
        ch.items = request.items;
        let before = snapshot(&s, ch.id).await;
        let mut expected = [ch.tex1, ch.tex2];
        expected[channel] = texture;
        let mut request = write(&ch);
        request.items[4] = -1;
        for slot in [5, 12, usize::MAX] {
            assert!(
                s.consume_dye(request.clone(), slot, kind, expected)
                    .await
                    .is_err()
            );
            assert_eq!(snapshot(&s, ch.id).await, before);
        }
        ch.revision = s
            .consume_dye(request.clone(), 4, kind, expected)
            .await
            .unwrap();
        assert_eq!(ch.revision, request.revision + 1);
        ch.items[4] = -1;
        [ch.tex1, ch.tex2] = expected;
        let saved = s.load(ch.account, ch.id).await.unwrap();
        assert_eq!(
            serde_json::to_value(saved).unwrap(),
            serde_json::to_value(&ch).unwrap()
        );
        assert!(
            s.consume_dye(request, 4, kind, expected)
                .await
                .unwrap_err()
                .to_string()
                .contains("stale")
        );
    }
}

#[tokio::test]
async fn dye_transaction_failure_rolls_back_appearance_inventory_and_revision() {
    let (s, ch) = fixture(Path::new(":memory:")).await;
    s.call(|db| {
        // AFTER forces the real UPDATE to run before SQLite aborts the transaction.
        db.execute_batch("CREATE TRIGGER dye_fail AFTER UPDATE ON characters BEGIN SELECT RAISE(ABORT,'injected dye transaction failure'); END;")?;
        Ok(())
    }).await.unwrap();
    let before = snapshot(&s, ch.id).await;
    for ground in [false, true] {
        let mut request = write(&ch);
        if !ground {
            request.items[4] = -1;
        }
        let result = if ground {
            s.consume_ground_dye(request, 0x1000, [0x01f0f8ff, ch.tex2])
                .await
        } else {
            s.consume_dye(request, 4, 0x1000, [0x01f0f8ff, ch.tex2])
                .await
        };
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("injected dye transaction failure")
        );
        assert_eq!(snapshot(&s, ch.id).await, before);
    }
    s.call(|db| {
        db.execute_batch("DROP TRIGGER dye_fail")?;
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(
        s.consume_ground_dye(write(&ch), 0x1000, [0x01f0f8ff, ch.tex2])
            .await
            .unwrap(),
        ch.revision + 1
    );
}
