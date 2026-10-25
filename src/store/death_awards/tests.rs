use super::*;

async fn setup() -> (std::path::PathBuf, Content, Store, Character) {
    let content = Content::load(Path::new("data")).unwrap();
    let path = std::env::temp_dir().join(format!(
        "death-awards-{}-{}.db",
        std::process::id(),
        rand::random::<u64>()
    ));
    let store = Store::open(&path, &content).unwrap();
    let owner = store.list("death-awards".into()).await.unwrap();
    let ch = store
        .create(owner.id, content.get(WIZARD).unwrap())
        .await
        .unwrap();
    (path, content, store, ch)
}

async fn equip(store: &Store, ch: &mut Character, items: Vec<i32>) {
    ch.revision = store
        .exchange(
            vec![InventoryWrite {
                id: ch.id,
                account: ch.account,
                revision: ch.revision,
                items: items.clone(),
            }],
            vec![],
        )
        .await
        .unwrap()[0];
    ch.items = items;
}
fn fatal(ch: &mut Character, fame: i32) {
    ch.fame = fame;
    ch.hp = 0;
    ch.alive = false;
}

#[tokio::test]
async fn equipment_death_zero_one_multiple_backpack_and_checked_boundaries() {
    let (_, _, store, mut ch) = setup().await;
    for (base, equipment, expected) in [
        (19, vec![-1; 12], 0),
        (
            19,
            vec![0xaa2, -1, -1, -1, 0xc01, -1, -1, -1, -1, -1, -1, -1],
            0,
        ),
        (
            19,
            vec![0xaa2, -1, -1, 0xabf, 0xc01, -1, -1, -1, -1, -1, -1, -1],
            1,
        ),
        (
            100,
            vec![0xaa2, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1],
            3,
        ),
    ] {
        ch.items = equipment;
        ch.fame = base;
        assert_eq!(super::equipment(&ch, &store.content).unwrap(), expected);
    }
    ch.fame = 0;
    assert_eq!(super::equipment(&ch, &store.content).unwrap(), 0);
    ch.fame = i32::MAX;
    assert!(super::equipment(&ch, &store.content).is_err());
    ch.items.fill(-1);
    assert_eq!(super::equipment(&ch, &store.content).unwrap(), 0);
    ch.items[0] = 65535;
    assert!(super::equipment(&ch, &store.content).is_err());
    ch.items[0] = -1;
    ch.items[3] = 0xb3e;
    assert!(super::equipment(&ch, &store.content).is_err());
    ch.items[3] = -1;
    ch.items[0] = 0xaa2;
    let mut huge = (*store.content).clone();
    huge.objects.get_mut(&0xaa2).unwrap().fame_bonus = u32::MAX;
    assert!(super::equipment(&ch, &huge).is_err());
}

#[tokio::test]
async fn equipment_death_atomic_wallet_guild_rollback_and_concurrent_retry() {
    let (path, content, store, mut ch) = setup().await;
    store.rename(ch.account, "Awardchief".into()).await.unwrap();
    fatal(&mut ch, 1000);
    store
        .death(ch.clone(), "guild funding".into())
        .await
        .unwrap();
    store
        .change_guild(ch.account, GuildAction::Create("Award Guild".into()))
        .await
        .unwrap();
    let mut ch = store
        .create(ch.account, content.get(WIZARD).unwrap())
        .await
        .unwrap();
    equip(
        &store,
        &mut ch,
        vec![0xaa2, -1, -1, 0xabf, -1, -1, -1, -1, -1, -1, -1, -1],
    )
    .await;
    let saved = ch.clone();
    fatal(&mut ch, 19);
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER reject_award BEFORE UPDATE OF fame ON guild_members BEGIN SELECT RAISE(ABORT,'forced guild storage failure'); END;").unwrap();
    assert!(store.death(ch.clone(), "rollback".into()).await.is_err());
    assert_eq!(
        store.load(ch.account, ch.id).await.unwrap().items,
        saved.items
    );
    assert_eq!(store.presentation(ch.account).await.unwrap().fame, 232);
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM deaths WHERE character=?1",
            [ch.id],
            |r| r.get::<_, i32>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(store.guild(ch.account).await.unwrap().earned, 0);
    db.execute_batch("DROP TRIGGER reject_award").unwrap();
    let other = Store::open(&path, &content).unwrap();
    let (a, b) = tokio::join!(
        store.death(ch.clone(), "one".into()),
        other.death(ch.clone(), "two".into())
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(store.presentation(ch.account).await.unwrap().fame, 252);
    assert_eq!(
        store.list("death-awards".into()).await.unwrap().earned_fame,
        1252
    );
    let guild = store.guild(ch.account).await.unwrap();
    assert_eq!((guild.fame, guild.earned), (20, 20));
    assert_eq!(
        db.query_row(
            "SELECT fame FROM guild_members WHERE account=?1",
            [ch.account],
            |r| r.get::<_, i32>(0)
        )
        .unwrap(),
        20
    );
    assert_eq!(
        db.query_row(
            "SELECT equipment_fame FROM deaths WHERE character=?1",
            [ch.id],
            |r| r.get::<_, i32>(0)
        )
        .unwrap(),
        1
    );
    assert!(store.load(ch.account, ch.id).await.is_err());
}

#[tokio::test]
async fn equipment_death_rejects_stale_gear_foreign_owner_and_lost_progress() {
    let (_, _, store, mut ch) = setup().await;
    let mut stale = ch.clone();
    equip(
        &store,
        &mut ch,
        vec![0xaa2, -1, -1, 0xabf, -1, -1, -1, -1, -1, -1, -1, -1],
    )
    .await;
    fatal(&mut stale, 19);
    assert!(store.death(stale, "stale revision".into()).await.is_err());
    let mut wrong = ch.clone();
    wrong.items[0] = -1;
    fatal(&mut wrong, 19);
    assert!(
        store
            .death(wrong, "stale transient gear".into())
            .await
            .is_err()
    );
    let mut wrong = ch.clone();
    wrong.account += 1;
    fatal(&mut wrong, 19);
    assert!(store.death(wrong, "foreign".into()).await.is_err());
    let mut old = ch.clone();
    ch.exp = 20000;
    ch.fame = 20;
    store.progress(ch.clone()).await.unwrap();
    fatal(&mut old, 19);
    assert!(store.death(old, "lost earned XP".into()).await.is_err());
    fatal(&mut ch, 20);
    store
        .death(ch.clone(), "final canonical equipment".into())
        .await
        .unwrap();
    assert_eq!(store.presentation(ch.account).await.unwrap().fame, 48);
}

#[tokio::test]
async fn equipment_death_legacy_migration_preserves_snapshots_wallet_and_paid_vaults() {
    let (path, content, store, mut ch) = setup().await;
    assert_eq!(
        store
            .register(
                "death-awards".into(),
                "awards@example.test".into(),
                "test-password".into()
            )
            .await
            .unwrap(),
        ch.account
    );
    let db = Connection::open(&path).unwrap();
    db.execute("UPDATE accounts SET credits=600 WHERE id=?1", [ch.account])
        .unwrap();
    ch.revision = store
        .purchase_vault(
            InventoryWrite {
                id: ch.id,
                account: ch.account,
                revision: ch.revision,
                items: ch.items.clone(),
            },
            1,
        )
        .await
        .unwrap()
        .unwrap()
        .0;
    let mut chest = store.vault(ch.account, 1).await.unwrap();
    chest.items[0] = 0xc01;
    ch.revision = store
        .exchange(
            vec![InventoryWrite {
                id: ch.id,
                account: ch.account,
                revision: ch.revision,
                items: ch.items.clone(),
            }],
            vec![chest],
        )
        .await
        .unwrap()[0];
    fatal(&mut ch, 19);
    // Explicit base-only paid fixture preserves the pre-policy schema13 migration contract.
    let raw = serde_json::to_string(&ch).unwrap();
    let paid = DeathFameReceipt::paid(&ch, 0, ReceiptPolicy::EquipmentOnly).unwrap();
    let tx = db.unchecked_transaction().unwrap();
    tx.execute(
        "UPDATE characters SET alive=0,stats=?2,items=?3,revision=revision+1 WHERE id=?1",
        params![ch.id, raw, serde_json::to_string(&[-1; 12]).unwrap()],
    )
    .unwrap();
    tx.execute("INSERT INTO deaths(character,snapshot,killed_by,equipment_fame,receipt) VALUES(?1,?2,'old base-only fixture',0,?3)",params![ch.id,raw,serde_json::to_string(&paid).unwrap()]).unwrap();
    tx.execute(
        "UPDATE account_fame SET balance=19,earned=19 WHERE account=?1",
        [ch.account],
    )
    .unwrap();
    tx.commit().unwrap();
    let snapshot: String = db
        .query_row(
            "SELECT snapshot FROM deaths WHERE character=?1",
            [ch.id],
            |r| r.get(0),
        )
        .unwrap();
    let vaults = store.vaults(ch.account).await.unwrap();
    drop(store);
    for table in ["bot_minds", "trade_transactions"] {
        assert_eq!(
            db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "historical schema13 fixture cannot discard community records"
        );
        db.execute_batch(&format!("DROP TABLE {table};")).unwrap();
    }
    db.execute_batch("DROP TABLE legacy_claims; DROP TABLE level_credits; ALTER TABLE deaths DROP COLUMN receipt; ALTER TABLE deaths DROP COLUMN equipment_fame; DROP TABLE pictures; DROP TABLE picture_sequence; PRAGMA user_version=13;")
        .unwrap();
    let store = Store::open(&path, &content).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT snapshot FROM deaths WHERE character=?1",
            [ch.id],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        snapshot
    );
    assert_eq!(
        db.query_row(
            "SELECT equipment_fame FROM deaths WHERE character=?1",
            [ch.id],
            |r| r.get::<_, i32>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(store.presentation(ch.account).await.unwrap().fame, 19);
    assert_eq!(
        serde_json::to_value(store.vaults(ch.account).await.unwrap()).unwrap(),
        serde_json::to_value(vaults).unwrap()
    );
    drop(store);
    db.execute_batch("ALTER TABLE deaths DROP COLUMN equipment_fame")
        .unwrap();
    assert!(
        Store::open(&path, &content).is_err(),
        "missing current award cannot masquerade as legacy"
    );
    db.execute_batch("ALTER TABLE deaths ADD COLUMN equipment_fame INTEGER NOT NULL DEFAULT 0")
        .unwrap();
    assert!(
        Store::open(&path, &content).is_err(),
        "current award cannot default silently"
    );
}

#[tokio::test]
async fn death_schema_rejects_noncanonical_current_metadata_before_any_write() {
    for (old, new) in [
        ("character INTEGER PRIMARY KEY", "character INTEGER"),
        (
            "character INTEGER PRIMARY KEY",
            "character TEXT PRIMARY KEY",
        ),
        (
            "character INTEGER PRIMARY KEY",
            "character INTEGER NOT NULL PRIMARY KEY",
        ),
        (
            "character INTEGER PRIMARY KEY",
            "character INTEGER PRIMARY KEY DEFAULT 0",
        ),
        ("snapshot TEXT NOT NULL", "snapshot BLOB NOT NULL"),
        ("snapshot TEXT NOT NULL", "snapshot TEXT"),
        (
            "snapshot TEXT NOT NULL",
            "snapshot TEXT NOT NULL DEFAULT '{}'",
        ),
        ("killed_by TEXT NOT NULL", "killed_by BLOB NOT NULL"),
        ("killed_by TEXT NOT NULL", "killed_by TEXT"),
        (
            "killed_by TEXT NOT NULL",
            "killed_by TEXT NOT NULL DEFAULT ''",
        ),
        (
            "died_at INTEGER NOT NULL DEFAULT(unixepoch())",
            "died_at TEXT NOT NULL DEFAULT(unixepoch())",
        ),
        (
            "died_at INTEGER NOT NULL DEFAULT(unixepoch())",
            "died_at INTEGER DEFAULT(unixepoch())",
        ),
        (
            "died_at INTEGER NOT NULL DEFAULT(unixepoch())",
            "died_at INTEGER NOT NULL",
        ),
        (
            "died_at INTEGER NOT NULL DEFAULT(unixepoch())",
            "died_at INTEGER NOT NULL DEFAULT 0",
        ),
        (
            "equipment_fame INTEGER NOT NULL",
            "equipment_fame TEXT NOT NULL",
        ),
        ("equipment_fame INTEGER NOT NULL", "equipment_fame INTEGER"),
        (
            "equipment_fame INTEGER NOT NULL",
            "equipment_fame INTEGER NOT NULL DEFAULT 0",
        ),
        (
            "equipment_fame INTEGER NOT NULL",
            "equipment_fame INTEGER NOT NULL GENERATED ALWAYS AS (0) STORED",
        ),
    ] {
        let (path, content, store, mut ch) = setup().await;
        fatal(&mut ch, 19);
        store.death(ch, "preserved death".into()).await.unwrap();
        drop(store);
        let db = Connection::open(&path).unwrap();
        let sql: String = db
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name='deaths'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut changed = sql.replace(old, new);
        if new == "equipment_fame TEXT NOT NULL" {
            changed = changed.replace(" CHECK(typeof(equipment_fame)='integer' AND equipment_fame BETWEEN 0 AND 2147483647)", "");
        }
        assert_ne!(sql, changed);
        let columns = if new.contains("GENERATED") {
            "character,snapshot,killed_by,died_at,receipt"
        } else {
            "character,snapshot,killed_by,died_at,equipment_fame,receipt"
        };
        db.execute_batch(&format!("ALTER TABLE deaths RENAME TO preserved_deaths; {changed}; INSERT INTO deaths({columns}) SELECT {columns} FROM preserved_deaths; DROP TABLE preserved_deaths; CREATE INDEX deaths_by_time ON deaths(died_at);")).unwrap();
        if new == "character INTEGER" {
            db.execute_batch("INSERT INTO deaths SELECT * FROM deaths; UPDATE account_fame SET balance=38,earned=38;").unwrap();
        }
        // A deferred fixture WAL checkpoint must not race the byte-for-byte preflight assertion.
        db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        let before = std::fs::read(&path).unwrap();
        let error = Store::open(&path, &content).err().expect(new);
        assert!(
            error.to_string().contains("death") && error.to_string().contains("metadata"),
            "{new}: {error:#}"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "schema/version/data changed for {new}"
        );
        drop(db);
        std::fs::remove_file(path).unwrap();
    }
}

fn history() -> DeathHistory {
    DeathHistory {
        retained_deaths: 1,
        other_same_class_max_level: 20,
        legacy_claimed: false,
        prior_paid_max: i32::MAX,
    }
}
fn empty_balances() -> PayoutBalances {
    PayoutBalances {
        wallet: (0, 0),
        guild: None,
    }
}
fn kinds(receipt: &DeathFameReceipt) -> Vec<BonusKind> {
    use BonusKind::*;
    receipt.validate().unwrap();
    let mut subtotal = receipt.base;
    for row in &receipt.rows {
        let (rate, flat) = match row.kind {
            Ancestor => (10, 20),
            LegacyBuilder => (10, 10),
            Pacifist | Thirsty | Mundane | BootsOnTheGround => (25, 0),
            Explorer | Cartographer => (5, 0),
            TunnelRat | EnemyOfTheGods | SlayerOfTheGods | OryxSlayer | Accurate | Sharpshooter
            | Sniper | TeamPlayer | LeaderOfMen | DoerOfDeeds | FriendOfTheCubes | WellEquipped
            | FirstBorn => (10, 0),
        };
        assert_eq!(row.amount, subtotal * rate / 100 + flat, "{:?}", row.kind);
        subtotal += row.amount;
    }
    assert_eq!(subtotal, receipt.total);
    receipt.rows.iter().map(|row| row.kind).collect()
}

#[tokio::test]
async fn all_death_fame_phase1_ordered_rules() {
    use BonusKind::*;
    let (_, content, _, mut ch) = setup().await;
    ch.items.fill(-1);
    ch.fame = 100;
    ch.level = 2;
    let mut lifetime = FameLifetime::known_only();
    let first = DeathHistory {
        retained_deaths: 0,
        other_same_class_max_level: 0,
        legacy_claimed: false,
        prior_paid_max: 0,
    };
    let mut equipment_content = content.clone();
    equipment_content
        .objects
        .get_mut(&0xaa2)
        .unwrap()
        .fame_bonus = 10;
    ch.items[0] = 0xaa2;
    let receipt = evaluate(&ch, &lifetime, first, &equipment_content, empty_balances()).unwrap();
    assert_eq!(
        kinds(&receipt),
        [Ancestor, LegacyBuilder, WellEquipped, FirstBorn]
    );
    assert_eq!(
        receipt.rows.iter().map(|r| r.amount).collect::<Vec<_>>(),
        [30, 23, 15, 16]
    );
    assert_eq!(receipt.total, 184);
    ch.items[4] = 0xc01;
    assert_eq!(
        evaluate(&ch, &lifetime, first, &equipment_content, empty_balances()).unwrap(),
        receipt
    );
    ch.items.fill(-1);
    for level in [1, 2, 19, 20] {
        ch.level = level;
        for maximum in [0, level, 20] {
            for claimed in [false, true] {
                let h = DeathHistory {
                    other_same_class_max_level: maximum,
                    legacy_claimed: claimed,
                    ..history()
                };
                assert_eq!(
                    kinds(&evaluate(&ch, &lifetime, h, &content, empty_balances()).unwrap())
                        .contains(&LegacyBuilder),
                    level >= 2 && level > maximum && !claimed
                );
            }
        }
    }
    ch.level = 20;
    for prior in [99, 100, 101] {
        let h = DeathHistory {
            prior_paid_max: prior,
            ..history()
        };
        assert_eq!(
            kinds(&evaluate(&ch, &lifetime, h, &content, empty_balances()).unwrap())
                .contains(&FirstBorn),
            prior < 100
        );
    }
    let negative = [
        Pacifist,
        Thirsty,
        Mundane,
        BootsOnTheGround,
        FriendOfTheCubes,
    ];
    for level in [19, 20] {
        ch.level = level;
        lifetime = FameLifetime::complete();
        assert_eq!(
            kinds(&evaluate(&ch, &lifetime, history(), &content, empty_balances()).unwrap()),
            if level == 20 {
                negative.to_vec()
            } else {
                vec![]
            }
        );
        lifetime.shots = 1;
        lifetime.projectile_hits = 1;
        lifetime.inventory_potion_used = true;
        lifetime.ability_used = true;
        lifetime.teleported = true;
        lifetime.cube_kills = 1;
        lifetime.kills = 1;
        let rows = kinds(&evaluate(&ch, &lifetime, history(), &content, empty_balances()).unwrap());
        assert!(negative.iter().all(|k| !rows.contains(k)));
    }
    for (threshold, kind) in [(10, EnemyOfTheGods), (50, SlayerOfTheGods)] {
        for gods in [threshold - 1, threshold, threshold + 1] {
            for level in [19, 20] {
                ch.level = level;
                lifetime = FameLifetime::complete();
                lifetime.kills = 100;
                lifetime.god_kills = gods;
                let rows = kinds(
                    &evaluate(&ch, &lifetime, history(), &content, empty_balances()).unwrap(),
                );
                assert_eq!(rows.contains(&kind), level == 20 && gods > threshold);
                if gods == 51 && level == 20 {
                    assert!(rows.contains(&EnemyOfTheGods));
                }
            }
        }
    }
    for (threshold, kind) in [(25, Accurate), (50, Sharpshooter), (75, Sniper)] {
        for hits in [threshold - 1, threshold, threshold + 1] {
            for level in [19, 20] {
                ch.level = level;
                lifetime = FameLifetime::complete();
                lifetime.shots = 100;
                lifetime.projectile_hits = hits;
                let rows = kinds(
                    &evaluate(&ch, &lifetime, history(), &content, empty_balances()).unwrap(),
                );
                assert_eq!(rows.contains(&kind), level == 20 && hits > threshold);
                if hits == 76 && level == 20 {
                    assert!(rows.contains(&Accurate) && rows.contains(&Sharpshooter));
                }
            }
        }
    }
    ch.level = 1;
    for (threshold, kind) in [
        (1_000_000, Explorer),
        (4_000_000, Cartographer),
        (100, TeamPlayer),
        (1000, LeaderOfMen),
        (1000, DoerOfDeeds),
    ] {
        for count in [threshold - 1, threshold, threshold + 1] {
            lifetime = FameLifetime::known_only();
            match kind {
                Explorer | Cartographer => lifetime.tiles = count,
                TeamPlayer | LeaderOfMen => lifetime.level_assists = count,
                DoerOfDeeds => lifetime.qualified_quests = count,
                _ => unreachable!(),
            }
            let rows =
                kinds(&evaluate(&ch, &lifetime, history(), &content, empty_balances()).unwrap());
            assert_eq!(rows.contains(&kind), count > threshold);
            if kind == Cartographer && count > threshold {
                assert!(rows.contains(&Explorer));
            }
            if kind == LeaderOfMen && count > threshold {
                assert!(rows.contains(&TeamPlayer));
            }
        }
    }
    lifetime = FameLifetime::known_only();
    for dungeon in crate::dungeon::HistoricalDungeon::ALL {
        assert!(
            !kinds(&evaluate(&ch, &lifetime, history(), &content, empty_balances()).unwrap())
                .contains(&TunnelRat)
        );
        lifetime.realm_exits.insert(dungeon);
    }
    // This is a ten-exit rule fixture, not evidence the missing worlds are playable.
    assert_eq!(
        kinds(&evaluate(&ch, &lifetime, history(), &content, empty_balances()).unwrap()),
        [TunnelRat]
    );
    for dungeon in crate::dungeon::HistoricalDungeon::ALL {
        lifetime.realm_exits.remove(&dungeon);
        assert!(
            !kinds(&evaluate(&ch, &lifetime, history(), &content, empty_balances()).unwrap())
                .contains(&TunnelRat)
        );
        lifetime.realm_exits.insert(dungeon);
    }
    assert_eq!(crate::dungeon::DUNGEONS.len(), 13);
    use crate::dungeon::{HistoricalDungeon, tomb};
    assert_eq!(
        HistoricalDungeon::ALL,
        [
            HistoricalDungeon::PirateCave,
            HistoricalDungeon::SpiderDen,
            HistoricalDungeon::ForbiddenJungle,
            HistoricalDungeon::SnakePit,
            HistoricalDungeon::SpriteWorld,
            HistoricalDungeon::UndeadLair,
            HistoricalDungeon::AbyssOfDemons,
            HistoricalDungeon::TombOfTheAncients,
            HistoricalDungeon::OceanTrench,
            HistoricalDungeon::ManorOfTheImmortals,
        ]
    );
    assert_eq!(
        HistoricalDungeon::from_world("Tomb of the Ancients"),
        Some(HistoricalDungeon::TombOfTheAncients)
    );
    let tomb_registrations: Vec<_> = crate::dungeon::DUNGEONS
        .iter()
        .filter(|(portal, _, name, _)| *portal == tomb::PORTAL || *name == "Tomb of the Ancients")
        .copied()
        .collect();
    assert_eq!(
        tomb_registrations,
        [(
            tomb::PORTAL,
            "tomb-of-the-ancients",
            "Tomb of the Ancients",
            tomb::ANCHOR
        )]
    );
    lifetime = FameLifetime::known_only();
    lifetime.oryx_kills = 1;
    lifetime.kills = 1;
    assert_eq!(
        kinds(&evaluate(&ch, &lifetime, history(), &content, empty_balances()).unwrap()),
        [OryxSlayer]
    );
    ch.fame = 0;
    ch.items[0] = 0xaa2;
    let zero_equipment = evaluate(
        &ch,
        &FameLifetime::known_only(),
        history(),
        &equipment_content,
        empty_balances(),
    )
    .unwrap();
    assert_eq!(
        zero_equipment.rows,
        [BonusRow {
            kind: WellEquipped,
            amount: 0
        }]
    );
    ch.items.fill(-1);
    let zero = evaluate(&ch, &lifetime, history(), &content, empty_balances()).unwrap();
    assert_eq!(
        zero.rows,
        [BonusRow {
            kind: OryxSlayer,
            amount: 0
        }]
    );
    assert_eq!(zero.total, 0);
    lifetime = FameLifetime::known_only();
    ch.fame = 1;
    let zero_first = evaluate(
        &ch,
        &lifetime,
        DeathHistory {
            prior_paid_max: 0,
            ..history()
        },
        &content,
        empty_balances(),
    )
    .unwrap();
    assert_eq!(
        zero_first.rows,
        [BonusRow {
            kind: FirstBorn,
            amount: 0
        }]
    );
    ch.fame = 0;
    let zero = evaluate(&ch, &lifetime, first, &content, empty_balances()).unwrap();
    assert_eq!(
        zero.rows,
        [
            BonusRow {
                kind: Ancestor,
                amount: 20
            },
            BonusRow {
                kind: FirstBorn,
                amount: 2
            }
        ]
    );
}

#[tokio::test]
async fn all_death_fame_phase1_known_only_history() {
    use BonusKind::*;
    let (_, content, _, mut ch) = setup().await;
    ch.level = 20;
    ch.fame = 100;
    ch.items.fill(-1);
    ch.hits = u64::MAX;
    ch.shots = 1;
    ch.quests = u64::MAX;
    let mut l = FameLifetime::known_only();
    assert!(
        evaluate(&ch, &l, history(), &content, empty_balances())
            .unwrap()
            .rows
            .is_empty()
    );
    l.kills = 1;
    l.god_kills = 1;
    l.shots = 1;
    l.projectile_hits = 1;
    l.oryx_kills = 1;
    l.tiles = 4_000_001;
    l.level_assists = 1001;
    l.qualified_quests = 1001;
    assert_eq!(
        kinds(&evaluate(&ch, &l, history(), &content, empty_balances()).unwrap()),
        [
            OryxSlayer,
            Explorer,
            Cartographer,
            TeamPlayer,
            LeaderOfMen,
            DoerOfDeeds
        ]
    );
    l = FameLifetime::complete();
    let rows = kinds(&evaluate(&ch, &l, history(), &content, empty_balances()).unwrap());
    assert!(!rows.contains(&Accurate) && !rows.contains(&EnemyOfTheGods));
    for bad in [0, 2, 255] {
        l.version = bad;
        assert!(evaluate(&ch, &l, history(), &content, empty_balances()).is_err());
    }
    l.version = 1;
    l.projectile_hits = 1;
    assert!(l.validate().is_err());
    l = FameLifetime::complete();
    l.god_kills = 1;
    assert!(l.validate().is_err());
    let raw = serde_json::to_value(FameLifetime::complete()).unwrap();
    for field in ["version", "completeness", "shots", "qualified_quests"] {
        let mut malformed = raw.clone();
        malformed.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<FameLifetime>(malformed).is_err());
    }
    for version in [0, 2, 255] {
        let mut malformed = raw.clone();
        malformed["version"] = serde_json::json!(version);
        assert!(serde_json::from_value::<FameLifetime>(malformed).is_err());
    }
    let mut malformed = raw;
    malformed["completeness"] = serde_json::json!("future");
    assert!(serde_json::from_value::<FameLifetime>(malformed).is_err());
}

#[tokio::test]
async fn all_death_fame_phase1_receipt_validation() {
    use BonusKind::*;
    let valid = DeathFameReceipt {
        policy: ReceiptPolicy::Historical,
        version: 1,
        base: 100,
        rows: vec![
            BonusRow {
                kind: Ancestor,
                amount: 30,
            },
            BonusRow {
                kind: LegacyBuilder,
                amount: 23,
            },
            BonusRow {
                kind: WellEquipped,
                amount: 15,
            },
            BonusRow {
                kind: FirstBorn,
                amount: 16,
            },
        ],
        total: 184,
    };
    valid.validate().unwrap();
    assert_eq!(
        serde_json::from_str::<DeathFameReceipt>(&serde_json::to_string(&valid).unwrap()).unwrap(),
        valid
    );
    for mutate in 0..8 {
        let mut bad = valid.clone();
        match mutate {
            0 => bad.version = 0,
            1 => bad.version = 2,
            2 => bad.base = -1,
            3 => bad.total = -1,
            4 => bad.rows[0].amount = -1,
            5 => bad.total += 1,
            6 => bad.rows.swap(0, 1),
            7 => bad.rows.insert(0, bad.rows[0].clone()),
            _ => unreachable!(),
        }
        assert!(bad.validate().is_err(), "mutation {mutate}");
    }
    let mut bad = valid.clone();
    bad.rows = vec![
        BonusRow {
            kind: Ancestor,
            amount: 0
        };
        22
    ];
    assert!(bad.validate().is_err());
    let mut bad = valid.clone();
    bad.base = i32::MAX;
    assert!(bad.validate().is_err());
    for version in [0, 2, 255] {
        let mut raw = serde_json::to_value(&valid).unwrap();
        raw["version"] = serde_json::json!(version);
        assert!(serde_json::from_value::<DeathFameReceipt>(raw).is_err());
    }
    let mut too_large = serde_json::to_value(&valid).unwrap();
    too_large["rows"][0]["amount"] = serde_json::json!(2147483648_u64);
    assert!(serde_json::from_value::<DeathFameReceipt>(too_large).is_err());
    let mut raw = serde_json::to_value(&valid).unwrap();
    raw["rows"][0]["kind"] = serde_json::json!("unknown");
    assert!(serde_json::from_value::<DeathFameReceipt>(raw).is_err());
    let (_, content, _, mut ch) = setup().await;
    ch.fame = i32::MAX;
    ch.items.fill(-1);
    let l = FameLifetime::known_only();
    assert!(
        evaluate(
            &ch,
            &l,
            DeathHistory {
                retained_deaths: 0,
                ..history()
            },
            &content,
            empty_balances()
        )
        .is_err()
    );
    ch.fame = 100;
    for balances in [
        PayoutBalances {
            wallet: (i32::MAX, i32::MAX),
            guild: None,
        },
        PayoutBalances {
            wallet: (0, i32::MAX),
            guild: None,
        },
        PayoutBalances {
            wallet: (0, 0),
            guild: Some((i32::MAX, i32::MAX)),
        },
        PayoutBalances {
            wallet: (0, 0),
            guild: Some((0, i32::MAX)),
        },
    ] {
        assert!(evaluate(&ch, &l, history(), &content, balances).is_err());
    }
    for h in [
        DeathHistory {
            other_same_class_max_level: -1,
            ..history()
        },
        DeathHistory {
            other_same_class_max_level: 21,
            ..history()
        },
        DeathHistory {
            prior_paid_max: -1,
            ..history()
        },
    ] {
        assert!(evaluate(&ch, &l, h, &content, empty_balances()).is_err());
    }
}

#[tokio::test]
async fn all_death_fame_phase2_schema17_roundtrip_corruption() {
    let (path, content, store, mut ch) = setup().await;
    assert_eq!(ch.lifetime.completeness, LifetimeCompleteness::Complete);
    let mut missing = serde_json::to_value(&ch).unwrap();
    missing.as_object_mut().unwrap().remove("lifetime");
    assert!(serde_json::from_value::<Character>(missing).is_err());
    let mut future = serde_json::to_value(&ch).unwrap();
    future["schema"] = serde_json::json!(2);
    assert!(serde_json::from_value::<Character>(future).is_err());
    for field in ["version", "projectile_hits"] {
        let mut bad = serde_json::to_value(&ch).unwrap();
        bad["lifetime"][field] = serde_json::json!(99);
        assert!(serde_json::from_value::<Character>(bad.clone()).is_err());
        assert!(decode_legacy_character(&bad.to_string(), ch.base).is_err());
    }
    fatal(&mut ch, 100);
    store
        .death(ch.clone(), "phase2 receipt".into())
        .await
        .unwrap();
    let before = store.fame(ch.account, ch.id).await.unwrap();
    drop(store);
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
            .unwrap(),
        18
    );
    let original: String = db
        .query_row(
            "SELECT receipt FROM deaths WHERE character=?1",
            [ch.id],
            |r| r.get(0),
        )
        .unwrap();
    let receipt: DeathFameReceipt = serde_json::from_str(&original).unwrap();
    assert_eq!(receipt.policy, ReceiptPolicy::Historical);
    receipt.validate().unwrap();
    let reopened = Store::open(&path, &content).unwrap();
    assert_eq!(reopened.fame(ch.account, ch.id).await.unwrap(), before);
    drop(reopened);
    for mutation in 0..8 {
        let mut raw: serde_json::Value = serde_json::from_str(&original).unwrap();
        match mutation {
            0 => raw["version"] = serde_json::json!(2),
            1 => raw["policy"] = serde_json::json!("unknown"),
            2 => raw["total"] = serde_json::json!(101),
            3 => raw["rows"] = serde_json::json!([{"kind":"unknown","amount":0}]),
            4 => raw["rows"] = serde_json::json!([{"kind":"well_equipped","amount":-1}]),
            5 => {
                raw["rows"] = serde_json::json!([{"kind":"well_equipped","amount":0},{"kind":"well_equipped","amount":0}])
            }
            6 => {
                raw["rows"] = serde_json::json!([{"kind":"first_born","amount":0},{"kind":"ancestor","amount":0}])
            }
            7 => raw["rows"] = serde_json::json!([{"kind":"ancestor","amount":0}]),
            _ => unreachable!(),
        }
        db.execute("UPDATE deaths SET receipt=?1", [raw.to_string()])
            .unwrap();
        assert!(
            Store::open(&path, &content).is_err(),
            "receipt mutation {mutation}"
        );
        db.execute("UPDATE deaths SET receipt=?1", [&original])
            .unwrap();
    }
    let mut raw: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&ch).unwrap()).unwrap();
    raw.as_object_mut().unwrap().remove("lifetime");
    db.execute("UPDATE deaths SET snapshot=?1", [raw.to_string()])
        .unwrap();
    assert!(
        Store::open(&path, &content).is_err(),
        "current receipt cannot authorize a missing lifetime"
    );
    db.execute(
        "UPDATE deaths SET snapshot=?1",
        [serde_json::to_string(&ch).unwrap()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO legacy_claims(account,class,character) VALUES(?1,?2,?3)",
        params![ch.account, ch.class, ch.id],
    )
    .unwrap();
    assert!(
        Store::open(&path, &content).is_err(),
        "equipment receipt cannot manufacture a Legacy claim"
    );
    db.execute("DELETE FROM legacy_claims", []).unwrap();
    db.pragma_update(None, "foreign_keys", false).unwrap();
    db.execute(
        "INSERT INTO level_credits(recipient,reached_level,giver) VALUES(?1,2,999999)",
        [ch.id],
    )
    .unwrap();
    assert!(
        Store::open(&path, &content).is_err(),
        "orphan giver must reject startup"
    );
    db.execute("DELETE FROM level_credits", []).unwrap();
    db.pragma_update(None, "foreign_keys", true).unwrap();
    assert!(Store::open(&path, &content).is_ok());
    // An explicit prepared receipt isolates claim storage from the runtime history comparator.
    ch.level = 2;
    ch.exp = 50;
    let paid = evaluate(
        &ch,
        &ch.lifetime,
        DeathHistory {
            other_same_class_max_level: 0,
            ..history()
        },
        &content,
        empty_balances(),
    )
    .unwrap();
    assert_eq!(
        paid.rows,
        [BonusRow {
            kind: BonusKind::LegacyBuilder,
            amount: 20
        }]
    );
    let snapshot = serde_json::to_string(&ch).unwrap();
    db.execute(
        "UPDATE deaths SET snapshot=?1,receipt=?2",
        params![snapshot, serde_json::to_string(&paid).unwrap()],
    )
    .unwrap();
    db.execute("UPDATE characters SET stats=?1", [&snapshot])
        .unwrap();
    db.execute("UPDATE account_fame SET balance=120,earned=120", [])
        .unwrap();
    db.execute(
        "INSERT INTO legacy_claims(account,class,character) VALUES(?1,?2,?3)",
        params![ch.account, ch.class, ch.id],
    )
    .unwrap();
    let reopened = Store::open(&path, &content).unwrap();
    assert!(
        reopened
            .fame(ch.account, ch.id)
            .await
            .unwrap()
            .contains("<TotalFame>120</TotalFame>")
    );
    drop(reopened);
    db.execute("UPDATE legacy_claims SET class=class+1", [])
        .unwrap();
    assert!(
        Store::open(&path, &content).is_err(),
        "foreign class claim rejected"
    );
}

#[test]
fn all_death_fame_phase2_schema17_exact_metadata() {
    for (table, old, new) in [
        ("deaths", "receipt TEXT NOT NULL", "receipt TEXT"),
        (
            "legacy_claims",
            "class INTEGER NOT NULL",
            "class TEXT NOT NULL",
        ),
        (
            "level_credits",
            "PRIMARY KEY(recipient,reached_level)",
            "PRIMARY KEY(reached_level,recipient)",
        ),
        ("level_credits_by_giver", "(giver)", "(recipient)"),
    ] {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE accounts(id INTEGER PRIMARY KEY); CREATE TABLE characters(id INTEGER PRIMARY KEY)").unwrap();
        for (name, sql) in [
            ("deaths", DEATHS_SQL),
            ("legacy_claims", CLAIMS_SQL),
            ("level_credits", CREDITS_SQL),
            ("level_credits_by_giver", CREDITS_INDEX_SQL),
        ] {
            let definition = if name == table {
                sql.replace(old, new)
            } else {
                sql.to_owned()
            };
            if name == table {
                assert_ne!(sql, definition);
            }
            db.execute_batch(&definition).unwrap();
        }
        assert!(check_schema(&db, 17).is_err(), "metadata {table}");
    }
}

#[tokio::test]
async fn all_death_fame_phase2_old_paid_bytes_unchanged() {
    let (path, content, store, mut ch) = setup().await;
    equip(
        &store,
        &mut ch,
        vec![0xaa2, -1, -1, 0xabf, -1, -1, -1, -1, -1, -1, -1, -1],
    )
    .await;
    fatal(&mut ch, 19);
    store.death(ch.clone(), "old paid".into()).await.unwrap();
    let wallet = 20;
    drop(store);
    let db = Connection::open(&path).unwrap();
    // This is a schema15 equipment-only paid fixture, not a downgrade of a runtime payment.
    db.execute(
        "UPDATE account_fame SET balance=20,earned=20 WHERE account=?",
        [ch.account],
    )
    .unwrap();
    let snapshot: String = db
        .query_row("SELECT snapshot FROM deaths", [], |r| r.get(0))
        .unwrap();
    let mut old: serde_json::Value = serde_json::from_str(&snapshot).unwrap();
    old.as_object_mut().unwrap().remove("lifetime");
    old["tiles"] = serde_json::json!(4_000_001_u64);
    let old = serde_json::to_string_pretty(&old).unwrap();
    db.execute(
        "UPDATE deaths SET snapshot=?1,died_at=1700000000,equipment_fame=1",
        [&old],
    )
    .unwrap();
    db.execute("UPDATE characters SET stats=?1", [&old])
        .unwrap();
    db.execute_batch("DROP TABLE legacy_claims; DROP TABLE level_credits; ALTER TABLE deaths DROP COLUMN receipt; PRAGMA user_version=15;").unwrap();
    for _ in 0..2 {
        let reopened = Store::open(&path, &content).unwrap();
        assert_eq!(
            reopened.presentation(ch.account).await.unwrap().fame,
            wallet
        );
        let (bytes, equipment, date, raw): (String, i32, i64, String) = db
            .query_row(
                "SELECT snapshot,equipment_fame,died_at,receipt FROM deaths",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!((bytes, equipment, date), (old.clone(), 1, 1700000000));
        let receipt: DeathFameReceipt = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            (receipt.policy, receipt.total),
            (ReceiptPolicy::LegacyPaid, 20)
        );
        assert!(
            reopened
                .fame(ch.account, ch.id)
                .await
                .unwrap()
                .contains("<TotalFame>20</TotalFame>")
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM legacy_claims", [], |r| r
                .get::<_, i32>(0))
                .unwrap(),
            0
        );
        let current: String = db
            .query_row("SELECT stats FROM characters", [], |r| r.get(0))
            .unwrap();
        let migrated: Character = serde_json::from_str(&current).unwrap();
        assert_eq!(
            migrated.lifetime.completeness,
            LifetimeCompleteness::KnownOnly
        );
        assert_eq!(migrated.lifetime.tiles, 4_000_001);
        drop(reopened);
    }
}

fn phase5_receipt(db: &Connection, id: i32) -> DeathFameReceipt {
    decode_receipt(
        &db.query_row("SELECT receipt FROM deaths WHERE character=?", [id], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
    )
    .unwrap()
}
fn phase5_state(db: &Connection) -> Vec<String> {
    ["SELECT json_array(id,alive,stats,items,revision) FROM characters ORDER BY id","SELECT json_array(character,snapshot,receipt,died_at,equipment_fame) FROM deaths ORDER BY character","SELECT json_array(account,class,character) FROM legacy_claims ORDER BY account,class","SELECT json_array(account,balance,earned) FROM account_fame ORDER BY account","SELECT json_array(id,fame,earned) FROM guilds ORDER BY id","SELECT json_array(account,fame) FROM guild_members ORDER BY account"].into_iter().flat_map(|sql|db.prepare(sql).unwrap().query_map([],|r|r.get::<_,String>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()).collect()
}

#[tokio::test]
async fn all_death_fame_phase5_atomic_claims_receipt() {
    let (path, content, store, mut funding) = setup().await;
    store
        .rename(funding.account, "Fullchief".into())
        .await
        .unwrap();
    fatal(&mut funding, 1000);
    store
        .death(funding.clone(), "funding".into())
        .await
        .unwrap();
    store
        .change_guild(funding.account, GuildAction::Create("Full Guild".into()))
        .await
        .unwrap();
    let mut ch = store
        .create(funding.account, content.get(WIZARD).unwrap())
        .await
        .unwrap();
    equip(&store, &mut ch, vec![-1; 12]).await;
    ch.level = 2;
    ch.exp = 50;
    fatal(&mut ch, 100);
    let db = Connection::open(&path).unwrap();
    let initial = phase5_state(&db);
    let mut overflow = ch.clone();
    overflow.fame = i32::MAX;
    assert!(store.death(overflow, "overflow".into()).await.is_err());
    assert_eq!(phase5_state(&db), initial);
    let mut invalid = ch.clone();
    invalid.lifetime.projectile_hits = 1;
    assert!(
        store
            .death(invalid, "invalid collision history".into())
            .await
            .is_err()
    );
    assert_eq!(phase5_state(&db), initial);
    for (table, event) in [
        ("deaths", "INSERT"),
        ("legacy_claims", "INSERT"),
        ("account_fame", "UPDATE"),
        ("guilds", "UPDATE"),
        ("guild_members", "UPDATE"),
    ] {
        db.execute_batch(&format!("CREATE TRIGGER fail_full BEFORE {event} ON {table} BEGIN SELECT RAISE(ABORT,'injected {table} failure'); END;")).unwrap();
        assert!(
            store.death(ch.clone(), "rollback".into()).await.is_err(),
            "{table}"
        );
        assert_eq!(phase5_state(&db), initial, "{table}");
        db.execute_batch("DROP TRIGGER fail_full").unwrap();
    }
    let other = Store::open(&path, &content).unwrap();
    let (a, b) = tokio::join!(
        store.death(ch.clone(), "one".into()),
        other.death(ch.clone(), "two".into())
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let paid = phase5_receipt(&db, ch.id);
    assert_eq!(paid.policy, ReceiptPolicy::Historical);
    assert_eq!(paid.total, 120);
    assert_eq!(
        paid.rows,
        vec![BonusRow {
            kind: BonusKind::LegacyBuilder,
            amount: 20
        }]
    );
    assert_eq!(store.guild(ch.account).await.unwrap().fame, 120);
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM legacy_claims WHERE account=?",
            [ch.account],
            |r| r.get::<_, i32>(0)
        )
        .unwrap(),
        1
    );
    let old = phase5_state(&db);
    assert!(store.death(ch.clone(), "duplicate".into()).await.is_err());
    assert_eq!(phase5_state(&db), old);
    // Prepared final level-two snapshots compete against retained level-one records.
    let account = store
        .list("full-concurrent-history".into())
        .await
        .unwrap()
        .id;
    store
        .register(
            "full-concurrent-history".into(),
            "concurrent@example.invalid".into(),
            "fixture-password".into(),
        )
        .await
        .unwrap();
    db.execute("UPDATE accounts SET credits=1100 WHERE id=?", [account])
        .unwrap();
    store.purchase_character_slot(account, 1).await.unwrap();
    let mut left = store
        .create(account, content.get(WIZARD).unwrap())
        .await
        .unwrap();
    let mut right = store
        .create(account, content.get(WIZARD).unwrap())
        .await
        .unwrap();
    for c in [&mut left, &mut right] {
        equip(&store, c, vec![-1; 12]).await;
        c.level = 2;
        c.exp = 50;
        fatal(c, 100);
    }
    let (a, b) = tokio::join!(
        store.death(left.clone(), "left".into()),
        other.death(right.clone(), "right".into())
    );
    if let Err(error) = &a {
        assert!(
            format!("{error:#}").contains("database is locked"),
            "unexpected concurrent failure: {error:#}"
        );
        store
            .death(left.clone(), "retry-left".into())
            .await
            .unwrap();
    }
    if let Err(error) = &b {
        assert!(
            format!("{error:#}").contains("database is locked"),
            "unexpected concurrent failure: {error:#}"
        );
        store
            .death(right.clone(), "retry-right".into())
            .await
            .unwrap();
    }
    let receipts = [phase5_receipt(&db, left.id), phase5_receipt(&db, right.id)];
    for kind in [BonusKind::Ancestor, BonusKind::LegacyBuilder] {
        assert_eq!(
            receipts
                .iter()
                .flat_map(|r| r.rows.iter())
                .filter(|r| r.kind == kind)
                .count(),
            1
        );
    }
    assert_eq!(
        store.presentation(account).await.unwrap().fame,
        receipts.iter().map(|r| r.total).sum::<i32>()
    );
}

#[tokio::test]
async fn all_death_fame_phase5_prior_paid_first_born() {
    let (path, content, store, mut first) = setup().await;
    equip(&store, &mut first, vec![-1; 12]).await;
    fatal(&mut first, 100);
    store
        .death(first.clone(), "prior paid".into())
        .await
        .unwrap();
    let db = Connection::open(&path).unwrap();
    assert_eq!(phase5_receipt(&db, first.id).total, 143);
    for (base, expected, first_born) in [(140, 140, false), (144, 158, true), (158, 158, false)] {
        let mut ch = store
            .create_prepared(
                first.account,
                content.get(crate::content::PRIEST).unwrap(),
                None,
            )
            .await
            .unwrap();
        equip(&store, &mut ch, vec![-1; 12]).await;
        fatal(&mut ch, base);
        store.death(ch.clone(), "cross class".into()).await.unwrap();
        let r = phase5_receipt(&db, ch.id);
        assert_eq!(r.total, expected);
        assert_eq!(
            r.rows.iter().any(|r| r.kind == BonusKind::FirstBorn),
            first_born
        );
    }
    let mut deleted = store
        .create(first.account, content.get(WIZARD).unwrap())
        .await
        .unwrap();
    deleted.level = 3;
    deleted.exp = 200;
    store.progress(deleted.clone()).await.unwrap();
    store
        .delete_character(deleted.account, deleted.id)
        .await
        .unwrap();
    let mut ch = store
        .create(first.account, content.get(WIZARD).unwrap())
        .await
        .unwrap();
    ch.level = 2;
    ch.exp = 50;
    fatal(&mut ch, 1);
    store
        .death(ch.clone(), "deleted record".into())
        .await
        .unwrap();
    assert!(
        !phase5_receipt(&db, ch.id)
            .rows
            .iter()
            .any(|r| r.kind == BonusKind::LegacyBuilder)
    );
}

#[tokio::test]
async fn all_death_fame_phase5_paid_readers_and_sort() {
    let (path, content, store, mut high_base) = setup().await;
    equip(&store, &mut high_base, vec![-1; 12]).await;
    fatal(&mut high_base, 100);
    store
        .death(high_base.clone(), "higher base".into())
        .await
        .unwrap();
    let account = store.list("full-low-base".into()).await.unwrap().id;
    let mut low_base = store
        .create(account, content.get(WIZARD).unwrap())
        .await
        .unwrap();
    equip(&store, &mut low_base, vec![-1; 12]).await;
    low_base.level = 20;
    low_base.exp = 200000;
    fatal(&mut low_base, 90);
    store
        .death(low_base.clone(), "higher paid".into())
        .await
        .unwrap();
    let db = Connection::open(&path).unwrap();
    let paid = phase5_receipt(&db, low_base.id);
    assert_eq!(paid.total, 411);
    assert_eq!(store.presentation(account).await.unwrap().fame, 411);
    let detail = store.fame(account, low_base.id).await.unwrap();
    assert!(detail.contains("<TotalFame>411</TotalFame>"));
    for row in &paid.rows {
        assert!(detail.contains(&format!(
            "<Bonus id=\"{}\" desc=\"{}\">{}</Bonus>",
            row.kind.id(),
            row.kind.description(),
            row.amount
        )));
    }
    let list = store
        .legends("all".into(), account, low_base.id)
        .await
        .unwrap();
    assert!(
        list.find(&format!("charId=\"{}\"", low_base.id)).unwrap()
            < list.find(&format!("charId=\"{}\"", high_base.id)).unwrap()
    );
    let mut zero = store
        .create(account, content.get(WIZARD).unwrap())
        .await
        .unwrap();
    equip(&store, &mut zero, vec![-1; 12]).await;
    zero.level = 20;
    zero.exp = 200000;
    fatal(&mut zero, 0);
    store
        .death(zero.clone(), "eligible zeros".into())
        .await
        .unwrap();
    let receipt = phase5_receipt(&db, zero.id);
    assert_eq!(receipt.total, 0);
    assert_eq!(receipt.rows.len(), 5);
    assert!(receipt.rows.iter().all(|r| r.amount == 0));
    let xml = store.fame(account, zero.id).await.unwrap();
    assert_eq!(xml.matches("<Bonus ").count(), 5);
    drop(store);
    let reopened = Store::open(&path, &content).unwrap();
    assert_eq!(reopened.fame(account, low_base.id).await.unwrap(), detail);
}

#[tokio::test]
async fn all_death_fame_phase5_archived_counter_policy() {
    let (path, content, store, mut ch) = setup().await;
    ch.tombs = 9;
    ch.shots = 101;
    ch.hits = 102;
    ch.tiles = 103;
    ch.kills = 104;
    ch.quests = 105;
    ch.lifetime.shots = 7;
    ch.lifetime.projectile_hits = 3;
    ch.lifetime.tiles = 12;
    ch.lifetime.kills = 8;
    ch.lifetime.god_kills = 4;
    ch.lifetime.oryx_kills = 1;
    ch.lifetime.qualified_quests = 13;
    ch.lifetime.level_assists = 14;
    fatal(&mut ch, 10);
    store
        .death(ch.clone(), "counter policy".into())
        .await
        .unwrap();
    let db = Connection::open(&path).unwrap();
    let fields = [
        "Shots",
        "ShotsThatDamage",
        "TilesUncovered",
        "MonsterKills",
        "GodKills",
        "OryxKills",
        "QuestsCompleted",
        "LevelUpAssists",
    ];
    let before = phase5_state(&db);
    let xml = store.fame(ch.account, ch.id).await.unwrap();
    assert!(xml.contains("<TombsCompleted>9</TombsCompleted>"));
    for (tag, value) in fields.iter().zip([7, 3, 12, 8, 4, 1, 13, 14]) {
        assert!(xml.contains(&format!("<{tag}>{value}</{tag}>")));
    }
    assert_eq!(phase5_state(&db), before);
    for policy in [ReceiptPolicy::LegacyPaid, ReceiptPolicy::EquipmentOnly] {
        // Explicit historical paid fixtures, not new payments or policy conversion.
        let mut old = store
            .create(ch.account, content.get(WIZARD).unwrap())
            .await
            .unwrap();
        old.shots = 101;
        old.hits = 102;
        old.tiles = 103;
        old.kills = 104;
        old.quests = 105;
        old.lifetime = ch.lifetime.clone();
        fatal(&mut old, 10);
        let receipt = DeathFameReceipt::paid(&old, 0, policy).unwrap();
        let raw = serde_json::to_string(&old).unwrap();
        let tx = db.unchecked_transaction().unwrap();
        tx.execute(
            "UPDATE characters SET alive=0,stats=?2,items=?3 WHERE id=?1",
            params![old.id, raw, serde_json::to_string(&[-1; 12]).unwrap()],
        )
        .unwrap();
        tx.execute("INSERT INTO deaths(character,snapshot,killed_by,equipment_fame,receipt) VALUES(?1,?2,'labelled old paid fixture',0,?3)",params![old.id,raw,serde_json::to_string(&receipt).unwrap()]).unwrap();
        tx.execute(
            "UPDATE account_fame SET balance=balance+10,earned=earned+10 WHERE account=?1",
            [old.account],
        )
        .unwrap();
        tx.commit().unwrap();
        let before = phase5_state(&db);
        let xml = store.fame(old.account, old.id).await.unwrap();
        assert!(xml.contains("<TombsCompleted>0</TombsCompleted>"));
        for (tag, value) in fields.iter().zip([101, 102, 103, 104, 0, 0, 105, 0]) {
            assert!(xml.contains(&format!("<{tag}>{value}</{tag}>")));
        }
        assert_eq!(phase5_state(&db), before);
    }
    db.execute(
        "UPDATE deaths SET snapshot=json_set(snapshot,'$.lifetime.version',999) WHERE character=?",
        [ch.id],
    )
    .unwrap();
    assert!(
        store.fame(ch.account, ch.id).await.is_err(),
        "malformed Historical must not substitute generic counters"
    );
}
