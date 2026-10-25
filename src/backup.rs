use crate::{content, store};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{io::Read, path::Path};

// Deserialization ignores unknown fields; exporting their original JSON could leak credentials.
fn known_fields(value: &Value, saved: &Value) -> bool {
    match (value, saved) {
        (Value::Object(input), Value::Object(output)) => input.iter().all(|(key, value)| {
            output
                .get(key)
                .is_some_and(|saved| known_fields(value, saved))
        }),
        (Value::Array(input), Value::Array(output)) => {
            input.len() == output.len() && input.iter().zip(output).all(|(a, b)| known_fields(a, b))
        }
        _ => true,
    }
}

fn validate(values: Vec<Value>, legacy: bool) -> Result<()> {
    for (index, value) in values.iter().enumerate() {
        ensure!(
            value.is_object(),
            "Unsupported character payload at record {index}"
        );
        let character = if legacy {
            store::decode_legacy_character(&value.to_string(), [0; 8])
        } else {
            store::decode_character(&value.to_string(), [0; 8])
        }
        .map_err(|_| anyhow::anyhow!("Unsupported character payload at record {index}"))?;
        ensure!(
            character.items.len() == 12
                && character.base.iter().all(|n| *n >= 0)
                && known_fields(value, &serde_json::to_value(character)?),
            "Unsupported character payload fields or inventory at record {index}"
        );
    }
    println!(
        "{}",
        serde_json::json!({"validated_characters": values.len()})
    );
    Ok(())
}

pub async fn run(args: &[String]) -> Result<bool> {
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--backup-validate"] | ["--backup-validate", "legacy"] => {
            let mut input = Vec::new();
            std::io::stdin()
                .take(32 * 1024 * 1024 + 1)
                .read_to_end(&mut input)?;
            ensure!(
                input.len() <= 32 * 1024 * 1024,
                "Backup character input exceeds 32 MiB"
            );
            validate(serde_json::from_slice(&input)?, args.len() == 2)?;
        }
        ["--backup-validate-community"] | ["--backup-validate", "community"] => {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct MindRow {
                account: i32,
                revision: i64,
                mind: String,
            }
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct TradeRow {
                id: String,
                owners: String,
                writes: String,
                revisions: String,
                delivery: Option<String>,
                market_key: Option<String>,
            }
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Community {
                bot_minds: Vec<MindRow>,
                trade_transactions: Vec<TradeRow>,
            }
            let mut input = Vec::new();
            std::io::stdin()
                .take(32 * 1024 * 1024 + 1)
                .read_to_end(&mut input)?;
            ensure!(
                input.len() <= 32 * 1024 * 1024,
                "Backup community input exceeds 32 MiB"
            );
            let state: Community = serde_json::from_slice(&input)
                .map_err(|_| anyhow::anyhow!("Unsupported community backup payload"))?;
            for row in &state.bot_minds {
                let value: Value = serde_json::from_str(&row.mind)?;
                let mind: crate::bots::pursuits::Mind = serde_json::from_str(&row.mind)?;
                mind.validate()?;
                ensure!(
                    row.revision > 0
                        && mind.account == row.account
                        && mind.revision == row.revision
                        && known_fields(&value, &serde_json::to_value(&mind)?),
                    "Invalid backup account mind"
                );
            }
            for row in &state.trade_transactions {
                let owners: Vec<i32> = serde_json::from_str(&row.owners)?;
                let value: Value = serde_json::from_str(&row.writes)?;
                let writes: Vec<store::InventoryWrite> = serde_json::from_str(&row.writes)?;
                let revisions: Vec<i32> = serde_json::from_str(&row.revisions)?;
                ensure!(
                    row.id.len() == 32
                        && row.id.bytes().all(|b| b.is_ascii_hexdigit())
                        && writes.len() == 2
                        && revisions.len() == 2
                        && owners == writes.iter().map(|w| w.account).collect::<Vec<_>>()
                        && owners[0] != owners[1]
                        && known_fields(&value, &serde_json::to_value(&writes)?)
                        && writes.iter().zip(revisions).all(|(w, revision)| w.id > 0
                            && w.account > 0
                            && (0..i32::MAX).contains(&w.revision)
                            && revision == w.revision + 1
                            && w.items.len() == 12
                            && w.items.iter().all(|i| (-1..=65535).contains(i))),
                    "Invalid backup trade receipt"
                );
                if let Some(raw) = &row.delivery {
                    let value: Value = serde_json::from_str(raw)?;
                    let delivery: crate::bots::pursuits::market::Delivery =
                        serde_json::from_str(raw)?;
                    ensure!(
                        delivery == delivery.terms.delivery(delivery.ordinal)?
                            && delivery.terms.accounts.iter().all(|a| owners.contains(a))
                            && row.market_key.as_deref()
                                == Some(
                                    format!("{:x}:{}", delivery.terms.version, delivery.ordinal)
                                        .as_str()
                                )
                            && known_fields(&value, &serde_json::to_value(&delivery)?),
                        "Invalid backup market receipt"
                    );
                } else {
                    ensure!(row.market_key.is_none(), "Backup gift has a market key");
                }
            }
            let receipt = if args.len() == 2 {
                serde_json::json!({"validated_minds": state.bot_minds.len(),
                    "validated_trades": state.trade_transactions.len()})
            } else {
                serde_json::json!({"validated_minds": state.bot_minds.len(),
                    "validated_trade_receipts": state.trade_transactions.len()})
            };
            println!("{receipt}");
        }
        ["--backup-validate-pictures"] => {
            use base64::Engine;
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Picture {
                id: i32,
                account: i32,
                name: String,
                data_type: i32,
                tags: String,
                png: String,
            }
            let mut input = Vec::new();
            std::io::stdin()
                .take(32 * 1024 * 1024 + 1)
                .read_to_end(&mut input)?;
            ensure!(
                input.len() <= 32 * 1024 * 1024,
                "Backup picture input exceeds 32 MiB"
            );
            let pictures: Vec<Picture> = serde_json::from_slice(&input)?;
            store::pictures::validate_capacity(0, pictures.len())?;
            let mut owners = std::collections::HashMap::new();
            for picture in &pictures {
                let owned = owners.entry(picture.account).or_insert(0);
                *owned += 1;
                store::pictures::validate_capacity(*owned, pictures.len())?;
                ensure!(
                    picture.id > 0
                        && picture.account > 0
                        && picture.png.len() <= store::pictures::MAX_PNG.div_ceil(3) * 4,
                    "Invalid backup picture identity or size"
                );
                store::pictures::metadata(&picture.name, picture.data_type, &picture.tags)?;
                let png = base64::engine::general_purpose::STANDARD.decode(&picture.png)?;
                store::pictures::validate_png(picture.data_type, &png)?;
            }
            println!(
                "{}",
                serde_json::json!({"validated_pictures": pictures.len()})
            );
        }
        ["--backup-validate-updates"] => {
            let mut input = Vec::new();
            std::io::stdin()
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut input)?;
            ensure!(
                input.len() <= 4 * 1024 * 1024,
                "Backup update input exceeds 4 MiB"
            );
            let mut snapshot: store::UpdateSnapshot = serde_json::from_slice(&input)
                .map_err(|_| anyhow::anyhow!("Unsupported update snapshot payload"))?;
            snapshot.validate()?;
            println!("{}", serde_json::json!({"validated_updates": true}));
        }
        ["--backup-fixture", directory, data] => {
            let directory = Path::new(directory);
            std::fs::create_dir(directory).context("Backup fixture requires a new directory")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
            }
            let content = content::Content::load(Path::new(data))?;
            let store = store::Store::open(&directory.join("game.db"), &content)?;
            for alive in [true, false] {
                let guid = format!("BACKUP_FIXTURE_SECRET_{alive}");
                let account = store.list(guid.clone()).await?;
                store
                    .register(
                        guid,
                        format!("backup-{alive}@example.invalid"),
                        "backup-fixture-password".into(),
                    )
                    .await?;
                store
                    .save_bot_mind(crate::bots::pursuits::Mind::new(account.id))
                    .await?;
                store
                    .save_picture(
                        account.id,
                        "Backup Art".into(),
                        2,
                        "backup, fixture".into(),
                        fixture_png(),
                        true,
                    )
                    .await?;
                if !alive {
                    let removed = store
                        .save_picture(
                            account.id,
                            "Removed Art".into(),
                            2,
                            "".into(),
                            fixture_png(),
                            true,
                        )
                        .await?;
                    store.delete_picture(account.id, removed).await?;
                }
                let code = store.issue_discord_code(account.id).await?;
                store
                    .redeem_discord_code(code, format!("1234567890123456{}", i32::from(alive)))
                    .await?;
                if alive {
                    store.reward_first_bug(account.id).await?;
                } else {
                    store
                        .gold_notified(account.id, vec![store::gold::DISCORD.into()])
                        .await?;
                }
                let mut character = store
                    .create(account.id, content.get(content::WIZARD)?)
                    .await?;
                character.gain_experience(&content, 400, &mut rand::thread_rng())?;
                character.dungeons = [1, 2, 3];
                character.pirate_caves = 4;
                character.abysses = 5;
                character.trenches = 6;
                character.jungles = 7;
                character.manors = 8;
                character.tombs = 9;
                store.progress(character.clone()).await?;
                let mut items = character.items.clone();
                items[4] = 0xa22;
                items[5] = 0xa23;
                store
                    .exchange(
                        vec![store::InventoryWrite {
                            id: character.id,
                            account: account.id,
                            revision: character.revision,
                            items,
                        }],
                        Vec::new(),
                    )
                    .await?;
                character = store.load(account.id, character.id).await?;
                if alive {
                    rusqlite::Connection::open(directory.join("game.db"))?
                        .execute("UPDATE accounts SET credits=1100 WHERE id=?1", [account.id])?;
                    store.purchase_character_slot(account.id, i32::MIN).await?;
                    character.revision = store
                        .purchase_vault(
                            store::InventoryWrite {
                                id: character.id,
                                account: account.id,
                                revision: character.revision,
                                items: character.items.clone(),
                            },
                            1,
                        )
                        .await?
                        .context("funded backup chest purchase declined")?
                        .0;
                    let mut first = store.vault(account.id, 0).await?;
                    let mut second = store.vault(account.id, 1).await?;
                    first.items[0] = character.items[4];
                    second.items[0] = character.items[5];
                    character.items[4] = -1;
                    character.items[5] = -1;
                    store
                        .exchange(
                            vec![store::InventoryWrite {
                                id: character.id,
                                account: account.id,
                                revision: character.revision,
                                items: character.items.clone(),
                            }],
                            vec![first, second],
                        )
                        .await?;
                    store
                        .create(account.id, content.get(content::WIZARD)?)
                        .await?;
                } else {
                    let peer: (i32, i32) = rusqlite::Connection::open(directory.join("game.db"))?.query_row(
                        "SELECT account,id FROM characters WHERE account<>?1 AND alive=1 ORDER BY id LIMIT 1",
                        [account.id], |r| Ok((r.get(0)?, r.get(1)?)))?;
                    let peer = store.load(peer.0, peer.1).await?;
                    let writes = [&peer, &character]
                        .map(|ch| store::InventoryWrite {
                            id: ch.id,
                            account: ch.account,
                            revision: ch.revision,
                            items: ch.items.clone(),
                        })
                        .to_vec();
                    let revisions = store
                        .exchange_trade("1".repeat(32), writes.clone(), None)
                        .await?;
                    ensure!(
                        store.exchange_trade("1".repeat(32), writes, None).await? == revisions,
                        "backup fixture receipt replay changed revisions"
                    );
                    character.revision = revisions[1];
                    character.fame = 11000;
                    character.alive = false;
                    character.hp = 0;
                    store
                        .death(character, "Backup contract fixture".into())
                        .await?;
                    let mut funding = store
                        .create_prepared(
                            account.id,
                            content.get(content::WIZARD)?,
                            Some("Backupchief".into()),
                        )
                        .await?;
                    store
                        .change_guild(
                            account.id,
                            store::GuildAction::Create("Backup Guild".into()),
                        )
                        .await?;
                    funding.items[0] = 0xaa2;
                    funding.items[3] = 0xabf;
                    funding.revision = store
                        .exchange(
                            vec![store::InventoryWrite {
                                id: funding.id,
                                account: funding.account,
                                revision: funding.revision,
                                items: funding.items.clone(),
                            }],
                            vec![],
                        )
                        .await?[0];
                    funding.fame = 12500;
                    funding.hp = 0;
                    funding.alive = false;
                    store.death(funding, "Guild backup fixture".into()).await?;
                    store
                        .change_guild(account.id, store::GuildAction::Upgrade { level: 1 })
                        .await?;
                    store
                        .guild_board(account.id, Some("Welcome & prepare!\n日本語 🧙".into()))
                        .await?;
                }
            }
            store
                .sync_updates(store::UpdateSnapshot {
                    guild_id: "12345678901234567".into(),
                    channel_id: "23456789012345678".into(),
                    posts: vec![store::UpdatePost {
                        id: "34567890123456789".into(),
                        content: "Backup update\nPersisted public text".into(),
                        published_at: 1700000000,
                        edited_at: None,
                        attachments: vec![],
                    }],
                })
                .await?;
            println!("{}", serde_json::json!({"fixture_characters": 4}));
        }
        ["--backup-load", database, data] => {
            ensure!(
                Path::new(database).is_file(),
                "Restored game database missing"
            );
            let content = content::Content::load(Path::new(data))?;
            let store = store::Store::open(Path::new(database), &content)?;
            let rows = {
                let db = rusqlite::Connection::open_with_flags(
                    database,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )?;
                let mut query =
                    db.prepare("SELECT account,id FROM characters WHERE alive=1 ORDER BY id")?;
                query
                    .query_map([], |row| Ok((row.get::<_, i32>(0)?, row.get::<_, i32>(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            let picture_ids = {
                let db = rusqlite::Connection::open(database)?;
                let mut q = db.prepare("SELECT id FROM pictures ORDER BY id")?;
                q.query_map([], |r| r.get::<_, i32>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            for id in &picture_ids {
                store.picture(*id).await?;
            }
            store.updates().await?;
            for (account, id) in &rows {
                store.vaults(*account).await?;
                store.load(*account, *id).await?.effective(&content)?;
            }
            println!(
                "{}",
                serde_json::json!({"loaded_living_characters": rows.len(), "loaded_pictures": picture_ids.len()})
            );
        }
        ["--backup-exercise-vault", database, data] => {
            exercise_vault(Path::new(database), Path::new(data)).await?;
        }
        ["--backup-exercise-pictures", database, data] => {
            let content = content::Content::load(Path::new(data))?;
            let store = store::Store::open(Path::new(database), &content)?;
            let db = rusqlite::Connection::open(database)?;
            let (id, owner): (i32, i32) = db.query_row(
                "SELECT id,account FROM pictures ORDER BY id LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let other: i32 = db.query_row(
                "SELECT id FROM accounts WHERE id<>?1 LIMIT 1",
                [owner],
                |r| r.get(0),
            )?;
            let next: i64 =
                db.query_row("SELECT next_id FROM picture_sequence WHERE id=1", [], |r| {
                    r.get(0)
                })?;
            ensure!(
                store.picture(id).await? == fixture_png(),
                "restored fixture artwork changed"
            );
            ensure!(
                store.delete_picture(other, id).await.is_err(),
                "restored art ownership bypass"
            );
            let created = store
                .save_picture(
                    owner,
                    "Restored Art".into(),
                    2,
                    "restored".into(),
                    fixture_png(),
                    true,
                )
                .await?;
            ensure!(
                i64::from(created) == next,
                "restored picture sequence reused a deleted ID"
            );
            ensure!(
                store
                    .save_picture(
                        owner,
                        "Restored Art".into(),
                        3,
                        "updated".into(),
                        fixture_png(),
                        true
                    )
                    .await?
                    == created,
                "restored overwrite changed ID"
            );
            store.delete_picture(owner, created).await?;
            drop(store);
            let reopened = store::Store::open(Path::new(database), &content)?;
            ensure!(
                reopened.picture(created).await.is_err()
                    && reopened.picture(id).await? == fixture_png(),
                "restored deletion/retrieval failed on reopen"
            );
            println!(
                "{}",
                serde_json::json!({"exercised_pictures":true,"deleted_id":created,"next_id":next+1})
            );
        }
        ["--backup-exercise-death", database, data] => {
            exercise_death(Path::new(database), Path::new(data)).await?;
        }
        [command, ..] if command.starts_with("--backup-") => {
            anyhow::bail!("Invalid backup command arguments")
        }
        _ => return Ok(false),
    }
    Ok(true)
}

async fn exercise_vault(database: &Path, data: &Path) -> Result<()> {
    use crate::world::{Pos, Slot, World};
    use std::sync::Arc;
    let content = Arc::new(content::Content::load(data)?);
    let store = store::Store::open(database, &content)?;
    let (account,id): (i32,i32) = rusqlite::Connection::open(database)?.query_row(
        "SELECT a.id,c.id FROM accounts a JOIN characters c ON c.account=a.id WHERE a.vault_mask=3 AND c.alive=1 ORDER BY c.id LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let ch = store.load(account, id).await?;
    let original = store.vaults(account).await?;
    let mut inventory = ch.items.clone();
    for v in &original {
        inventory.extend(&v.items);
    }
    inventory.sort();
    let mut world = World::with_ids(
        content.clone(),
        Arc::new(content::Map::load(&data.join("vault.wmap"), &content)?),
        Arc::new(std::sync::atomic::AtomicI32::new(1000)),
    )?;
    world.name = "Vault".into();
    world.owner = Some(account);
    world.load_vaults(original)?;
    let first = world
        .entities
        .values()
        .find(|e| e.kind == 0x504 && e.items[0] == 0xa22)
        .context("first restored fixture chest missing potion")?
        .id;
    let second = world
        .entities
        .values()
        .find(|e| e.kind == 0x504 && e.items[0] == 0xa23)
        .context("second restored fixture chest missing potion")?
        .id;
    let pos = world.entities[&first].pos;
    let other = world.entities[&second].pos;
    ensure!(
        pos.distance(other) < 3.,
        "fixture chest transfer exceeds native reach"
    );
    let player = world.enter_at(ch, None, Some(pos), store.presentation(account).await?)?;
    for ((from, source), (to, target)) in [
        ((first, 0), (second, 0)),
        ((second, 0), (player, 4)),
        ((player, 4), (first, 1)),
    ] {
        world.players.get_mut(&player).unwrap().pos = Pos {
            x: (pos.x + other.x) / 2.,
            y: (pos.y + other.y) / 2.,
        };
        let slot = |object, slot| Slot {
            object,
            slot,
            kind: if object == player {
                world.players[&player].character.items[slot]
            } else {
                world.entities[&object].items[slot]
            },
        };
        let plan = world.swap(player, slot(from, source), slot(to, target))?;
        let revisions = store
            .exchange(
                vec![store::InventoryWrite {
                    id: plan.character,
                    account: plan.account,
                    revision: plan.revision,
                    items: plan.items.clone(),
                }],
                plan.vaults.clone(),
            )
            .await?;
        world.finish_inventory(plan, Ok(revisions[0]))?;
    }
    let saved = store.vaults(account).await?;
    let mut after = store.load(account, id).await?.items;
    for v in &saved {
        after.extend(&v.items);
    }
    after.sort();
    ensure!(
        inventory == after,
        "restored vault exercise lost or duplicated items"
    );
    ensure!(
        saved[0].items[..2] == [0xa23, 0xa22] && saved[1].items == vec![-1; 8],
        "restored transfer/deposit/withdraw outcome mismatch"
    );
    drop(store);
    let reopened = store::Store::open(database, &content)?;
    ensure!(
        serde_json::to_value(reopened.vaults(account).await?)? == serde_json::to_value(saved)?,
        "restored chest state changed on reopen"
    );
    println!(
        "{}",
        serde_json::json!({"exercised_vault_chests":2,"transfers":1,"withdrawals":1,"deposits":1,"items_conserved":true})
    );
    Ok(())
}

async fn exercise_death(database: &Path, data: &Path) -> Result<()> {
    let content = content::Content::load(data)?;
    let store = store::Store::open(database, &content)?;
    let db = rusqlite::Connection::open(database)?;
    let account: i32 =
        db.query_row("SELECT account FROM guild_members WHERE rank=40", [], |r| {
            r.get(0)
        })?;
    let before = store.presentation(account).await?;
    let earned: i32 = db.query_row(
        "SELECT earned FROM account_fame WHERE account=?1",
        [account],
        |r| r.get(0),
    )?;
    let guild = store.guild(account).await?;
    let contribution: i32 = db.query_row(
        "SELECT fame FROM guild_members WHERE account=?1",
        [account],
        |r| r.get(0),
    )?;
    let mut ch = store.create(account, content.get(content::WIZARD)?).await?;
    let givers = {
        let mut q = db.prepare(
            "SELECT id FROM characters WHERE alive=1 AND account<>?1 ORDER BY id LIMIT 2",
        )?;
        q.query_map([account], |r| r.get::<_, i32>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    ensure!(
        givers.len() == 2,
        "restored credit exercise requires two retained living givers"
    );
    ch.gain_experience(&content, 200, &mut rand::thread_rng())?;
    let credits = vec![
        store::LevelAssist {
            reached_level: 2,
            giver: givers[0],
        },
        store::LevelAssist {
            reached_level: 3,
            giver: givers[1],
        },
    ];
    store
        .progress_with_assists(ch.clone(), credits.clone())
        .await?;
    ch.items[0] = 0xaa2;
    ch.items[3] = 0xabf;
    ch.revision = store
        .exchange(
            vec![store::InventoryWrite {
                id: ch.id,
                account,
                revision: ch.revision,
                items: ch.items.clone(),
            }],
            vec![],
        )
        .await?[0];
    ch.fame = 19;
    ch.hp = 0;
    ch.alive = false;
    store
        .death(ch.clone(), "Restored backup contract".into())
        .await?;
    drop(store);
    let reopened = store::Store::open(database, &content)?;
    let archived: (String, i32, i64, String) = db.query_row(
        "SELECT snapshot,equipment_fame,died_at,receipt FROM deaths WHERE character=?1",
        [ch.id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let receipt = store::death_awards::decode_receipt(&archived.3)?;
    ensure!(
        receipt.policy == store::death_awards::ReceiptPolicy::Historical
            && receipt.base == 19
            && receipt.total == 20,
        "restored paid JSON receipt mismatch"
    );
    for credit in &credits {
        ensure!(
            reopened.assist_count(credit.giver).await? >= 1,
            "restored causal credit missing"
        );
    }
    ensure!(
        archived.0 == serde_json::to_string(&ch)? && archived.1 == 1 && archived.2 > 0,
        "restored death archive mismatch"
    );
    let after = reopened.presentation(account).await?;
    let after_earned: i32 = db.query_row(
        "SELECT earned FROM account_fame WHERE account=?1",
        [account],
        |r| r.get(0),
    )?;
    let after_guild = reopened.guild(account).await?;
    let after_contribution: i32 = db.query_row(
        "SELECT fame FROM guild_members WHERE account=?1",
        [account],
        |r| r.get(0),
    )?;
    ensure!(
        after_earned == earned + 20
            && after.fame == before.fame + 20
            && after_guild.fame == guild.fame + 20
            && after_guild.earned == guild.earned + 20
            && after_contribution == contribution + 20,
        "restored death payout ledger mismatch"
    );
    let cleared: (bool, String) = db.query_row(
        "SELECT alive,items FROM characters WHERE id=?1",
        [ch.id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    ensure!(
        !cleared.0 && serde_json::from_str::<Vec<i32>>(&cleared.1)? == vec![-1; 12],
        "restored death inventory not cleared"
    );
    let detail = reopened.fame(account, ch.id).await?;
    ensure!(
        detail.contains("<BaseFame>19</BaseFame>")
            && detail.contains("<TotalFame>20</TotalFame>")
            && detail
                .contains("<Bonus id=\"Well Equipped\" desc=\"Bonus for equipment\">1</Bonus>"),
        "restored death detail mismatch"
    );
    let legends = reopened.legends("all".into(), account, ch.id).await?;
    let doc = roxmltree::Document::parse(&legends)?;
    ensure!(
        doc.descendants().any(
            |n| n.attribute("charId") == Some(ch.id.to_string().as_str())
                && n.children()
                    .any(|c| c.has_tag_name("TotalFame") && c.text() == Some("20"))
        ),
        "restored death Legends mismatch"
    );
    println!(
        "{}",
        serde_json::json!({"exercised_death": ch.id, "base_fame":19,"equipment_fame":1,"total_fame":20,"receipt":receipt,"level_credits":credits.iter().map(|c|serde_json::json!({"recipient":ch.id,"reached_level":c.reached_level,"giver":c.giver})).collect::<Vec<_>>()})
    );
    Ok(())
}

fn fixture_png() -> Vec<u8> {
    use std::io::Write;
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::from(8_u32.to_be_bytes());
    header.extend(8_u32.to_be_bytes());
    header.extend([8, 6, 0, 0, 0]);
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&[0; 264])
        .expect("fixture PNG compression");
    for (kind, data) in [
        (b"IHDR", header),
        (b"IDAT", encoder.finish().expect("fixture PNG compression")),
        (b"IEND", vec![]),
    ] {
        png.extend((data.len() as u32).to_be_bytes());
        png.extend(kind);
        png.extend(&data);
        let mut crc = crc32fast::Hasher::new();
        crc.update(kind);
        crc.update(&data);
        png.extend(crc.finalize().to_be_bytes());
    }
    png
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_market_ad_hint_remains_known_without_allowing_opaque_fields() {
        let mut mind = crate::bots::pursuits::Mind::new(17);
        mind.revision = 1;
        let mut legacy = serde_json::to_value(mind).unwrap();
        let market = legacy["market"].as_object_mut().unwrap();
        market.remove("quote");
        market.remove("preparing");
        market.insert("next_ad".into(), serde_json::json!(123456));
        let decoded: crate::bots::pursuits::Mind = serde_json::from_value(legacy.clone()).unwrap();
        decoded.validate().unwrap();
        let saved = serde_json::to_value(decoded).unwrap();
        assert_eq!(saved["market"]["next_ad"], 123456);
        assert!(known_fields(&legacy, &saved));
        legacy["market"]["credential"] = serde_json::json!("must-not-export");
        assert!(!known_fields(&legacy, &saved));
    }
}
