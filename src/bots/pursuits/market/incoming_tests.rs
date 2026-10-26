use super::tests::fixture;
use super::*;
use crate::world::TradeAction;
fn opened(m: &mut Market, v: &PlayerView, c: &Content) {
    m.quote = m.quote(v, c, None).unwrap();
    m.incoming_request("Peer".into(), v);
    assert!(matches!(
        m.incoming_action(v, c, None).unwrap(),
        Some(TradeAction::Request { .. })
    ));
    m.trade = Some(SessionTrade {
        binding: [-2, 10, 1],
        peer: "Peer".into(),
        items: [
            v.character.items.clone(),
            [vec![-1; 4], vec![DEFENSE, -1, -1, -1, -1, -1, -1, -1]].concat(),
        ],
        tradeable: [true; 12],
        offers: [[false; 12]; 2],
        ready: v.time_ms,
        ready_at: v.clock_at,
        changed: false,
    });
}
#[test]
fn incoming_human_exchange_selects_advertised_real_goods_and_requires_payment() {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    let (mut m, mut v) = fixture(&c, [SPEED, SPEED, SPEED, SPEED, -1, -1, -1, -1]);
    opened(&mut m, &v, &c);
    match m.incoming_action(&v, &c, None).unwrap().unwrap() {
        TradeAction::Change { offer } => assert_eq!(offer.iter().filter(|b| **b).count(), 4),
        _ => panic!("must offer advertised goods, not interpret an exchange as a gift"),
    }
    assert!(m.incoming_action(&v, &c, None).unwrap().is_none());
    m.trade.as_mut().unwrap().offers[1][4] = true;
    v.time_ms += 3000;
    v.clock_at += Duration::from_millis(3000);
    match m.incoming_action(&v, &c, None).unwrap().unwrap() {
        TradeAction::Accept { mine, theirs } => {
            assert_eq!(mine.iter().filter(|b| **b).count(), 4);
            assert_eq!(theirs.iter().filter(|b| **b).count(), 1);
        }
        _ => panic!("matching WINDOW exchange should accept after countdown"),
    }
    assert!(
        m.bargain.is_none(),
        "UI acceptance does not fabricate a settlement"
    );
}
#[test]
fn market_five_speed_for_defense_with_life_stock_still_requires_a_fresh_native_window() {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    let (mut m, mut v) = fixture(&c, [SPEED, SPEED, SPEED, SPEED, SPEED, LIFE, -1, -1]);
    m.name = "A".into();
    assert!(m.incoming_action(&v, &c, None).unwrap().is_none());
    let items = v.character.items.clone();
    opened(&mut m, &v, &c);
    assert_eq!(
        m.quote.as_ref().unwrap()[0],
        Goods {
            kind: SPEED,
            quantity: 5
        }
    );
    assert_eq!(
        m.quote.as_ref().unwrap()[1],
        Goods {
            kind: DEFENSE,
            quantity: 1
        }
    );
    assert!(
        matches!(m.incoming_action(&v, &c, None).unwrap(), Some(TradeAction::Change { offer }) if offer.iter().filter(|b| **b).count() == 5 && !offer[9])
    );
    assert!(m.incoming_action(&v, &c, None).unwrap().is_none());
    m.trade.as_mut().unwrap().offers[1][4] = true;
    assert!(m.incoming_action(&v, &c, None).unwrap().is_none());
    v.time_ms += 3000;
    v.clock_at += Duration::from_millis(3000);
    assert!(
        matches!(m.incoming_action(&v, &c, None).unwrap(), Some(TradeAction::Accept { mine, theirs }) if mine.iter().filter(|b| **b).count() == 5 && !mine[9] && theirs[4])
    );
    assert_eq!(v.character.items, items);
    assert!(
        m.bargain.is_none(),
        "window acceptance cannot invent a durable receipt"
    );
    v.character.inventory_revision += 1;
    v.character.items[4] = -1;
    assert!(matches!(
        m.incoming_action(&v, &c, None).unwrap(),
        Some(TradeAction::Cancel)
    ));
}

#[test]
fn incoming_incompatible_window_refuses_contextually_and_never_gives_wanted_potions_free() {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    for case in 0..6 {
        let (mut m, mut v) = fixture(&c, [SPEED, SPEED, SPEED, SPEED, -1, -1, -1, -1]);
        opened(&mut m, &v, &c);
        m.incoming_action(&v, &c, None).unwrap();
        m.trade.as_mut().unwrap().offers[1][4] = true;
        match case {
            0 => m.trade.as_mut().unwrap().items[1][4] = 0xa22,
            1 => v.character.items[4] = -1,
            2 => v.objects[0].account = Some(99),
            3 => v.lifecycle.departing = true,
            4 => v.elapsed_ms += 60_001,
            5 => v.character.hp = 1,
            _ => unreachable!(),
        }
        v.time_ms += 3000;
        v.clock_at += Duration::from_millis(3000);
        assert!(
            matches!(
                m.incoming_action(&v, &c, None).unwrap(),
                Some(TradeAction::Cancel)
            ),
            "case {case}"
        );
        if case == 0 {
            assert_eq!(m.outgoing.as_ref().unwrap().text, "nty need 1 def");
        }
    }
    // A bot buying speed offers its held Defense, rather than accepting the Speed for free.
    let (mut m, mut v) = fixture(&c, [DEFENSE, -1, -1, -1, -1, -1, -1, -1]);
    opened(&mut m, &v, &c);
    m.trade.as_mut().unwrap().items[1][4..8].fill(SPEED);
    m.incoming_action(&v, &c, None).unwrap();
    m.trade.as_mut().unwrap().offers[1][4..8].fill(true);
    v.time_ms += 3000;
    v.clock_at += Duration::from_millis(3000);
    assert!(
        matches!(m.incoming_action(&v,&c,None).unwrap(),Some(TradeAction::Accept{mine,..}) if mine[4])
    );
}
