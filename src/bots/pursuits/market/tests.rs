use super::*;
use crate::world::view::{Lifecycle, MapInfo, OutputState, OwnCharacter, Session, VisibleObject};

pub(super) fn fixture(c: &Content, backpack: [i32; 8]) -> (Market, PlayerView) {
    let stats = c.classes[&crate::content::WIZARD].base;
    let v = PlayerView {
        session: Session {
            world: -2,
            player: 10,
            account: 1,
            character: 1,
        },
        tick: 1,
        time_ms: 1000,
        clock_at: Instant::now(),
        elapsed_ms: 1000,
        position: Pos { x: 5.5, y: 5.5 },
        quest: None,
        map: MapInfo {
            name: "Nexus".into(),
            width: 20,
            height: 20,
            allow_teleport: false,
        },
        character: OwnCharacter {
            conditions: 0,
            class: crate::content::WIZARD,
            level: 20,
            experience: 0,
            fame: 0,
            hp: stats[0],
            mp: stats[1],
            stats,
            base_stats: stats,
            items: [vec![0xa97, 0xa2e, -1, -1], backpack.to_vec()].concat(),
            inventory_revision: 0,
            pirate_caves: 0,
            snake_pits: 0,
            ability_ready_ms: 0,
        },
        lifecycle: Lifecycle {
            alive: true,
            death_pending: false,
            departing: false,
            busy: false,
            quarantined: false,
            output: OutputState::Live,
        },
        tiles: Arc::default(),
        objects: vec![VisibleObject {
            id: 20,
            account: Some(2),
            kind: crate::content::WIZARD,
            name: "Peer".into(),
            position: Pos { x: 5.5, y: 5.5 },
            hp: 100,
            max_hp: 100,
            level: Some(20),
            equipped: vec![0xa97, 0xa2e, -1, -1],
            contents: vec![],
            blocks: false,
            usable: true,
            condition_mask: None,
        }],
        player_markers: vec![],
        projectiles: vec![],
        dungeon_entries: vec![],
    };
    (
        Market {
            name: "Trader".into(),
            ..Market::default()
        },
        v,
    )
}
fn message(text: &str) -> Packet {
    let mut w = wire::Writer::new();
    w.utf("Peer").i32(20).i32(0).u8(0).utf("").utf(text).utf("");
    Packet {
        id: wire::TEXT,
        body: w.0,
    }
}
#[test]
fn market_quotes_share_human_slang_grammar_without_chat_consent() {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    for text in [
        "wts 4 spd for 1 def",
        "SELLING 4 SPEED = DEF",
        "wtb def for 4 spd",
        "selling 4 spd for def @Peer",
    ] {
        let quote = parse_quote(text, &c).unwrap();
        assert_eq!(
            quote,
            [
                Goods {
                    kind: SPEED,
                    quantity: 4
                },
                Goods {
                    kind: DEFENSE,
                    quantity: 1
                }
            ]
        );
    }
    for text in [
        "sell 2593x4; want 2592",
        "Peer, agree abc p0 r0",
        "wts 0 spd for def",
        "wts 9 spd for def",
        "wts spd for spd",
        "wts fake for def",
    ] {
        assert!(parse_quote(text, &c).is_none(), "{text}");
    }
    let (mut market, v) = fixture(&c, [DEFENSE, -1, -1, -1, -1, -1, -1, -1]);
    assert!(
        market
            .receive(&message("wts 4 spd for def"), &v, &c, None)
            .unwrap()
    );
    assert!(market.incoming.is_some());
    assert!(market.bargain.is_none());
    assert!(market.trade.is_none());
    assert!(market.outgoing.is_none());
}
#[test]
fn market_real_stock_and_desires_drive_speed_defense_life_gear_chain() {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    for (backpack, give, want) in [
        ([SPEED, SPEED, SPEED, SPEED, -1, -1, -1, -1], SPEED, DEFENSE),
        (
            [
                DEFENSE, DEFENSE, DEFENSE, DEFENSE, DEFENSE, DEFENSE, DEFENSE, -1,
            ],
            DEFENSE,
            LIFE,
        ),
        ([LIFE, -1, -1, -1, -1, -1, -1, -1], LIFE, 0),
    ] {
        let (m, v) = fixture(&c, backpack);
        let q = m.quote(&v, &c, None).unwrap().unwrap();
        assert_eq!(q[0].kind, give);
        if want != 0 {
            assert_eq!(q[1].kind, want);
        } else {
            assert!(m.wanted(&v, &c, None, q[1].kind).unwrap() > 0);
        }
        assert!(m.count(&v, q[0].kind) >= usize::from(q[0].quantity));
    }
    let (m, v) = fixture(&c, [-1; 8]);
    assert!(m.quote(&v, &c, None).unwrap().is_none());
    let (mut m, v) = fixture(&c, [DEFENSE, -1, -1, -1, -1, -1, -1, -1]);
    m.quote = m.quote(&v, &c, None).unwrap();
    assert!(m.wants_speed(&v));
    m.quote = None;
    assert!(!m.wants_speed(&v));
    let mut prices = BTreeSet::new();
    for name in ["A", "B", "C"] {
        m.name = name.into();
        prices.insert(m.quote(&v, &c, None).unwrap().unwrap()[1].quantity);
    }
    assert_eq!(prices, BTreeSet::from([3, 4, 5]));
}
#[test]
fn market_ui_success_is_not_a_settlement_receipt_and_receipts_cannot_regress() {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    let (mut m, v) = fixture(&c, [SPEED; 8]);
    m.replace(Terms {
        version: 1,
        accounts: [1, 2],
        names: ["Trader".into(), "Peer".into()],
        goods: [
            Goods {
                kind: SPEED,
                quantity: 4,
            },
            Goods {
                kind: DEFENSE,
                quantity: 1,
            },
        ],
        meeting: [-2, 5, 5],
        expires: now() + 120,
        batch: 4,
    });
    let mut out = wire::Writer::new();
    out.i32(0).utf("Trade successful.");
    m.trade_packet(
        &Packet {
            id: 12,
            body: out.0,
        },
        &v,
    )
    .unwrap();
    assert_eq!(m.bargain.as_ref().unwrap().confirmed, 0);
    let receipt = m.bargain.as_ref().unwrap().terms.delivery(0).unwrap();
    m.recover(vec![receipt]).unwrap();
    assert_eq!(m.bargain.as_ref().unwrap().confirmed, 1);
    assert_eq!(
        m.bargain.as_ref().unwrap().resolution.as_deref(),
        Some("settled_all_installments_transaction_identified")
    );
    assert!(m.recover(vec![]).is_err());
}

#[test]
fn market_trade_up_intent_farms_short_stock_instead_of_buying_its_own_speed_back() {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    let (mut m, mut v) = fixture(&c, [SPEED, SPEED, SPEED, SPEED, SPEED, SPEED, -1, -1]);
    m.quote = m.quote(&v, &c, None).unwrap();
    assert_eq!(m.quote.as_ref().unwrap()[0].kind, SPEED);
    v.character.items[4..].fill(-1);
    v.character.items[4] = SPEED;
    v.character.items[5] = SPEED;
    v.character.items[6] = DEFENSE;
    assert!(m.wants_speed(&v));
    assert!(
        m.quote(&v, &c, None).unwrap().is_none(),
        "do not undo a funded trade-up by buying the same speed back"
    );
    v.character.items[4..].fill(DEFENSE);
    assert_eq!(m.quote(&v, &c, None).unwrap().unwrap()[1].kind, LIFE);
    v.character.items[4..].fill(-1);
    v.character.base_stats = c.classes[&v.character.class].caps;
    for slot in 0..4 {
        let slot_type = c.get(v.character.class).unwrap().slots[slot];
        if let Some(item) = c
            .objects
            .values()
            .filter(|o| o.slot_type == slot_type && !o.unsupported_equipment && !o.consumable)
            .max_by_key(|o| o.tier)
        {
            v.character.items[slot] = i32::from(item.kind);
        }
    }
    // If another real upgrade is still possible, the target remains valid; otherwise it is retired.
    let needed = m.wanted(&v, &c, None, DEFENSE).unwrap() > 0;
    let grid = super::session_tests::terrain(&mut v);
    m.decide(
        &v,
        &c,
        None,
        &grid,
        &mut rand::rngs::mock::StepRng::new(0, 1),
    )
    .unwrap();
    assert_eq!(m.wants_speed(&v), needed);
    let (mut buyer, mut v) = fixture(&c, [DEFENSE, -1, -1, -1, -1, -1, -1, -1]);
    buyer.quote = buyer.quote(&v, &c, None).unwrap();
    assert_eq!(buyer.quote.as_ref().unwrap()[1].kind, SPEED);
    v.character.items[4..].fill(-1);
    v.character.items[4..8].fill(SPEED);
    assert!(
        buyer.quote(&v, &c, None).unwrap().is_none(),
        "use purchased Speed for the selected maxing goal instead of selling it back"
    );
    v.character.base_stats[4] = c.classes[&v.character.class].caps[4];
    assert_eq!(
        buyer.quote(&v, &c, None).unwrap().unwrap()[1].kind,
        DEFENSE,
        "actual excess Speed may trade up after maxing"
    );
}

#[test]
fn market_peer_ads_must_fund_the_actual_objective_not_reverse_its_currency_chain() {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    let (mut seller, v) = fixture(&c, [LIFE, -1, -1, -1, -1, -1, -1, -1]);
    assert_ne!(
        seller.quote(&v, &c, None).unwrap().unwrap()[1].kind,
        DEFENSE
    );
    seller
        .receive(&message("wtb life for 6 def"), &v, &c, None)
        .unwrap();
    assert!(
        seller.incoming.is_none(),
        "Life saved for an upgrade is not stock to turn back into Defense"
    );
    let (mut trader, v) = fixture(&c, [SPEED, SPEED, SPEED, SPEED, -1, -1, -1, -1]);
    trader
        .receive(&message("wts 1 def for 3 spd"), &v, &c, None)
        .unwrap();
    let q = trader.incoming.as_ref().unwrap().quote.as_ref().unwrap();
    assert_eq!(
        q[0],
        Goods {
            kind: SPEED,
            quantity: 3
        }
    );
    assert_eq!(
        q[1],
        Goods {
            kind: DEFENSE,
            quantity: 1
        }
    );
    assert_eq!(
        trader.quote.as_ref().unwrap()[0].kind,
        SPEED,
        "a better human price still retains the real trade-up motive"
    );
}
