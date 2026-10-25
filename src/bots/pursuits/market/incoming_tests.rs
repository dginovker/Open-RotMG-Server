use super::tests::fixture;
use super::*;
use crate::world::TradeAction;

fn invite(m: &mut Market, v: &PlayerView) {
    let mut w = wire::Writer::new();
    w.utf("Peer");
    m.trade_packet(&Packet { id: 61, body: w.0 }, v).unwrap();
}
fn opened(m: &mut Market, v: &PlayerView, c: &Content) {
    invite(m, v);
    assert!(matches!(
        m.incoming_action(v, c, None).unwrap(),
        Some(TradeAction::Request { .. })
    ));
    m.trade = Some(SessionTrade {
        binding: [-2, 10, 1],
        peer: "Peer".into(),
        items: [
            v.character.items.clone(),
            [vec![-1; 4], vec![0xa22; 8]].concat(),
        ],
        tradeable: [true; 12],
        offers: [[false; 12]; 2],
        ready: v.time_ms,
        ready_at: v.clock_at,
        changed: false,
    });
}
#[test]
fn incoming_request_opens_independently_of_market_session_without_offering_stock() {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    let (mut m, v) = fixture(&c, [0xa1d, -1, -1, -1, -1, -1, -1, -1]);
    m.session_used = true;
    opened(&mut m, &v, &c);
    assert!(m.session.is_none());
    assert!(m.bargain.is_none());
    assert!(m.incoming_action(&v, &c, None).unwrap().is_none());
    m.trade.as_mut().unwrap().offers[1][4] = true;
    match m.incoming_action(&v, &c, None).unwrap().unwrap() {
        TradeAction::Accept { mine, theirs } => {
            assert_eq!(mine, [false; 12]);
            assert_eq!(theirs.iter().filter(|b| **b).count(), 1);
        }
        _ => panic!("useful free gift should be accepted"),
    }
    m.trade.as_mut().unwrap().ready_at += Duration::from_secs(3);
    assert!(m.incoming_action(&v, &c, None).unwrap().is_none());
}
#[test]
fn incoming_requests_preserve_capacity_contact_lifecycle_and_bargains() {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    for case in 0..10 {
        let (mut m, mut v) = fixture(&c, [-1; 8]);
        match case {
            0 => v.character.items[4..].fill(0xa22),
            1 => v.map.name = "Realm".into(),
            2 => v.lifecycle.busy = true,
            3 => v.lifecycle.alive = false,
            4 => v.lifecycle.death_pending = true,
            5 => v.lifecycle.departing = true,
            6 => v.lifecycle.quarantined = true,
            7 => v.objects[0].position.x += 3.,
            8 => v.objects[0].hp = 0,
            9 => v.objects[0].account = None,
            _ => unreachable!(),
        }
        invite(&mut m, &v);
        assert!(m.incoming.is_none(), "case {case}");
    }
    let (mut m, mut v) = fixture(&c, [-1; 8]);
    m.replace(Terms {
        version: 1,
        accounts: [1, 2],
        names: ["Trader".into(), "Peer".into()],
        goods: [
            Goods {
                kind: 0xa1d,
                quantity: 1,
            },
            Goods {
                kind: 0xa22,
                quantity: 1,
            },
        ],
        meeting: [-2, 5, 5],
        expires: now() + 120,
        batch: 1,
    });
    invite(&mut m, &v);
    assert!(
        m.incoming.is_none(),
        "an invite cannot replace negotiated consent"
    );
    m.bargain = None;
    opened(&mut m, &v, &c);
    v.objects[0].account = Some(3);
    assert!(matches!(
        m.incoming_action(&v, &c, None).unwrap(),
        Some(TradeAction::Cancel)
    ));
}
#[test]
fn canonical_health_gift_policy_preserves_actual_need_and_reserves() {
    use crate::bots::social::Life;
    let c = Content::load(std::path::Path::new("data")).unwrap();
    for (backpack, text, active) in [
        ([-1; 8], "take health", true),
        ([0xa22, 0xa22, -1, -1, -1, -1, -1, -1], "take health", false),
        (
            [0xa22, 0xa22, -1, -1, -1, -1, -1, -1],
            "spare health?",
            false,
        ),
        (
            [0xa22, 0xa22, 0xa22, -1, -1, -1, -1, -1],
            "spare health?",
            true,
        ),
    ] {
        let (_, v) = fixture(&c, backpack);
        let mut w = wire::Writer::new();
        w.utf("Peer")
            .i32(20)
            .i32(0)
            .u8(0)
            .utf("")
            .utf(&format!("Trader, {text}"))
            .utf("");
        let packet = Packet {
            id: wire::TEXT,
            body: w.0,
        };
        for incoming in [false, true] {
            let mut life = Life::default();
            assert_eq!(
                life.receive(&packet, &v, "Trader", 6., incoming).unwrap(),
                !incoming
            );
            assert_eq!(
                life.gift_active(),
                active && !incoming,
                "{text} with {backpack:?}, incoming={incoming}"
            );
        }
    }
}
#[test]
fn incoming_requests_do_not_refresh_lock_or_bypass_native_request_cooldown() {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    let (mut m, mut v) = fixture(&c, [-1; 8]);
    opened(&mut m, &v, &c);
    v.elapsed_ms += 59_000;
    invite(&mut m, &v);
    v.elapsed_ms += 1_001;
    assert!(matches!(
        m.incoming_action(&v, &c, None).unwrap(),
        Some(TradeAction::Cancel)
    ));
    m.trade = None;
    m.end_incoming(&v);
    invite(&mut m, &v);
    assert!(m.incoming.is_none());
    v.elapsed_ms += 20_000;
    invite(&mut m, &v);
    assert!(m.incoming.is_some());
}
#[test]
fn incoming_gifts_reject_junk_excess_deficits_capacity_and_bot_goods() {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    for case in 0..5 {
        let (mut m, mut v) = fixture(&c, [-1; 8]);
        opened(&mut m, &v, &c);
        let t = m.trade.as_mut().unwrap();
        t.offers[1][4] = true;
        match case {
            0 => t.items[1][4] = 0xa97, // Same weapon is not an upgrade.
            1 => {
                t.offers[1][5] = true;
                t.offers[1][6] = true;
            }
            2 => v.character.items[4..].fill(0xa22),
            3 => t.offers[0][4] = true,
            4 => t.offers[1][0] = true,
            _ => unreachable!(),
        }
        assert!(
            matches!(
                m.incoming_action(&v, &c, None).unwrap(),
                Some(TradeAction::Cancel)
            ),
            "case {case}"
        );
    }
}
