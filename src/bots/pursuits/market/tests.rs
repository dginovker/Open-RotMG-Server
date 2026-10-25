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
fn terms(goods: [Goods; 2]) -> Terms {
    Terms {
        version: 1,
        accounts: [2, 1],
        names: ["Peer".into(), "Trader".into()],
        goods,
        meeting: [-2, 5, 5],
        expires: now() + 120,
        batch: 2,
    }
}
fn message(text: String) -> Packet {
    let mut w = wire::Writer::new();
    w.utf("Peer")
        .i32(20)
        .i32(0)
        .u8(0)
        .utf("")
        .utf(&text)
        .utf("");
    Packet {
        id: wire::TEXT,
        body: w.0,
    }
}

#[test]
fn only_unsolicited_local_ads_are_optional_without_rewriting_terms() {
    let mut message = Message {
        text: "sell 2589x2; want 2595".into(),
        channel: Channel::Local,
        consent: false,
    };
    assert_eq!(message.optional_public(), Some(message.text.as_str()));
    assert_eq!(message.chat(), message.text);
    message.channel = Channel::Guild;
    assert!(message.optional_public().is_none());
    message.channel = Channel::Tell("Peer".into());
    assert!(message.optional_public().is_none());
    message.channel = Channel::Local;
    for verb in ["offer", "counter", "agree", "no deal:"] {
        message.text = format!("Peer, {verb} exact terms");
        assert!(message.optional_public().is_none());
        assert_eq!(message.chat(), message.text);
    }
    message.text = "sell 2589x2; want 2595".into();
    message.consent = true;
    assert!(message.optional_public().is_none());
}

#[test]
fn market_named_ads_resolve_exact_items_without_inventing_consent() {
    let mut c = Content::load(std::path::Path::new("data")).unwrap();
    let (mut market, v) = fixture(&c, [0xa23, 0xa23, 0xa23, -1, -1, -1, -1, -1]);
    market
        .receive(
            &message("sell Health Potion x2; want Magic Potion".into()),
            &v,
            &c,
            None,
        )
        .unwrap();
    let bargain = market.bargain.as_ref().unwrap();
    assert_eq!(
        bargain.terms.goods,
        [
            Goods {
                kind: 0xa23,
                quantity: 1
            },
            Goods {
                kind: 0xa22,
                quantity: 1
            }
        ]
    );
    assert!(!bargain.own_consent && !bargain.agreed && bargain.peer_consent.is_none());
    for ad in [
        "2594x2; want 2595",
        "Unknown Potion x1; want Magic Potion",
        "Health Potion x0; want Magic Potion",
        "Health Potion x17; want Magic Potion",
        "Health Potion x1; want Unknown Potion",
        "Ent Ancient x1; want Magic Potion",
    ] {
        assert!(parse_ad(ad, &c).is_none(), "{ad}");
    }
    let mut duplicate = c.get(0xa22).unwrap().clone();
    duplicate.kind = u16::MAX;
    c.objects.insert(duplicate.kind, duplicate);
    assert!(parse_ad("Health Potion x2; want Magic Potion", &c).is_none());
}

#[test]
fn market_counter_reduces_useful_offer_without_inventing_goods_or_consent() {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    let (mut market, v) = fixture(&c, [0xa1d, 0xa1d, 0xa22, 0xa22, -1, -1, -1, -1]);
    let offered = terms([
        Goods {
            kind: 0xa9d,
            quantity: 2,
        },
        Goods {
            kind: 0xa1d,
            quantity: 2,
        },
    ]);
    assert!(!market.suitable(&v, &c, None, &offered).unwrap());
    market.replace(offered.clone());
    market.bargain.as_mut().unwrap().own_consent = true;
    market.bargain.as_mut().unwrap().peer_consent = Some([-2, 20, 0]);
    market
        .receive(&message(offered.text("Trader", "counter")), &v, &c, None)
        .unwrap();
    let counter = market.bargain.as_ref().unwrap();
    assert_eq!(
        counter.terms.goods,
        [
            Goods {
                kind: 0xa1d,
                quantity: 1
            },
            Goods {
                kind: 0xa9d,
                quantity: 1
            }
        ]
    );
    counter.terms.validate().unwrap();
    assert!(market.suitable(&v, &c, None, &counter.terms).unwrap());
    assert!(!counter.own_consent && counter.peer_consent.is_none() && !counter.agreed);
    assert!(market.revoke_trade);
    assert!(
        market
            .history
            .iter()
            .all(|b| !b.own_consent && b.peer_consent.is_none())
    );
    assert!(!market.outgoing.as_ref().unwrap().consent);
    market
        .receive(&message(offered.text("Trader", "agree")), &v, &c, None)
        .unwrap();
    assert!(market.bargain.as_ref().unwrap().peer_consent.is_none());
}

#[test]
fn market_counter_preserves_rate_reserves_and_rejects_unusable_or_unheld_goods() {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    for (backpack, received, payment, expected) in [
        (
            [0xa1d, 0xa1d, 0xa22, 0xa22, -1, -1, -1, -1],
            (0xa23, 6),
            (0xa1d, 3),
            Some((1, 2)),
        ),
        (
            [0xa22, 0xa22, 0xa22, -1, -1, -1, -1, -1],
            (0xa23, 4),
            (0xa22, 2),
            Some((1, 2)),
        ),
        (
            [0xa1d, 0xa1d, 0xa22, 0xa22, -1, -1, -1, -1],
            (0xa9d, 2),
            (0xa22, 2),
            None,
        ),
        (
            [0xa1d, 0xa1d, 0xa22, 0xa22, -1, -1, -1, -1],
            (0xa1d, 2),
            (0xa22, 2),
            None,
        ),
        (
            [0xa1d, 0xa1d, 0xa22, 0xa22, -1, -1, -1, -1],
            (0xa9d, 2),
            (0xa1d, 3),
            None,
        ),
        (
            [0xa1d, 0xa1d, 0xa22, 0xa22, -1, -1, -1, -1],
            (0xa9d, 2),
            (0xa97, 2),
            None,
        ),
        (
            [0xa1d, 0xa1d, 0xa22, 0xa22, -1, -1, -1, -1],
            (0xa9d, 2),
            (0xa3d, 2),
            None,
        ),
    ] {
        let (mut market, v) = fixture(&c, backpack);
        let offered = terms([
            Goods {
                kind: received.0,
                quantity: received.1,
            },
            Goods {
                kind: payment.0,
                quantity: payment.1,
            },
        ]);
        market
            .receive(&message(offered.text("Trader", "offer")), &v, &c, None)
            .unwrap();
        if let Some((pay, receive)) = expected {
            let counter = &market.bargain.as_ref().unwrap().terms;
            assert_eq!(
                counter.goods,
                [
                    Goods {
                        kind: payment.0,
                        quantity: pay
                    },
                    Goods {
                        kind: received.0,
                        quantity: receive
                    }
                ]
            );
            assert!(market.suitable(&v, &c, None, counter).unwrap());
            assert_eq!(
                u16::from(pay) * u16::from(received.1),
                u16::from(receive) * u16::from(payment.1)
            );
        } else {
            assert!(
                market.bargain.is_none(),
                "unexpected bargain for {received:?}/{payment:?}"
            );
            assert!(market.outgoing.as_ref().unwrap().text.contains("no deal"));
        }
    }
}
