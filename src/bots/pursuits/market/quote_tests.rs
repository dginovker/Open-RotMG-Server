use super::tests::fixture;
use super::*;

#[test]
fn market_mind_mixed_stock_prioritizes_potion_quotes_before_life_gear() {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    let assassin = c.objects.values().find(|o| o.name == "Assassin").unwrap();
    assert_eq!(c.get(0xa0e).unwrap().tier, Some(8));
    for name in ["Vkm", "MoeFonzo"] {
        let speed_price = (3 + style(name) % 3) as u8;
        let life_price = (5 + (style(name) / 3) % 3) as u8;
        for (backpack, expected) in [
            ([DEFENSE; 8], [(DEFENSE, life_price), (LIFE, 1)]),
            ([SPEED; 8], [(SPEED, speed_price), (DEFENSE, 1)]),
            (
                [DEFENSE, -1, -1, -1, -1, -1, -1, -1],
                [(DEFENSE, 1), (SPEED, speed_price)],
            ),
        ] {
            let (mut m, mut v) = fixture(&c, backpack);
            v.character.class = assassin.kind;
            v.character.items[..4].copy_from_slice(&assassin.equipment[..4]);
            v.character.base_stats = c.classes[&assassin.kind].base;
            v.character.stats = v.character.base_stats;
            v.character.hp = v.character.stats[0];
            v.character.mp = v.character.stats[1];
            super::session_tests::terrain(&mut v);
            m.name = name.into();
            m.quote = Some([
                Goods {
                    kind: LIFE,
                    quantity: 1,
                },
                Goods {
                    kind: 0xa0e,
                    quantity: 1,
                },
            ]);
            m.vault = Some(vec![LIFE, -1, -1, -1, -1, -1, -1, -1]);
            m.session = Some(MarketSession {
                anchor: tile(v.position),
                until: v.elapsed_ms + 60_000,
                next_move: v.elapsed_ms + 10_000,
                destination: None,
            });
            assert!(m.wanted(&v, &c, None, 0xa0e).unwrap() > 0);
            let items = v.character.items.clone();
            let vault = m.vault.clone();
            let mut brain = Brain::new(ARCHETYPES[0], &v);
            brain.mind.market = m;
            brain.decide(&v, &c).unwrap();
            let m = &brain.mind.market;
            let q = m.quote.as_ref().unwrap();
            assert_eq!(
                q.each_ref().map(|g| (g.kind, g.quantity)),
                expected,
                "{name}"
            );
            assert_eq!(brain.activity, crate::bots::Activity::Trade);
            assert!(m.count(&v, q[0].kind) >= usize::from(q[0].quantity));
            assert!(m.wanted(&v, &c, None, q[1].kind).unwrap() >= q[1].quantity);
            assert!(
                m.message_current(m.outgoing.as_ref().unwrap(), &v, &c, None)
                    .unwrap()
            );
            assert_eq!(v.character.items, items);
            assert_eq!(m.vault, vault);
            assert!(m.bargain.is_none() && m.trade.is_none() && m.incoming.is_none());
        }
    }
}

#[test]
fn life_buyers_choose_different_real_upgrades_without_changing_quotes_each_tick() {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    let mut wants = BTreeSet::new();
    for name in ["A", "B", "C"] {
        let (mut m, v) = fixture(&c, [LIFE, -1, -1, -1, -1, -1, -1, -1]);
        m.name = name.into();
        let q = m.quote(&v, &c, None).unwrap().unwrap();
        assert_eq!(m.quote(&v, &c, None).unwrap().unwrap(), q);
        assert!(m.wanted(&v, &c, None, q[1].kind).unwrap() > 0);
        wants.insert(q[1].kind);
    }
    assert!(
        wants.len() >= 3,
        "all Life buyers want the same gear: {wants:?}"
    );
}

#[test]
fn gear_sellers_choose_different_owned_spares_and_never_offer_equipped_items() {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    let mut sales = BTreeSet::new();
    for name in ["A", "B"] {
        let (mut m, v) = fixture(&c, [0xad3, 0xaf9, -1, -1, -1, -1, -1, -1]);
        m.name = name.into();
        let q = m.quote(&v, &c, None).unwrap().unwrap();
        assert_eq!(m.quote(&v, &c, None).unwrap().unwrap(), q);
        assert!(v.character.items[4..].contains(&q[0].kind));
        assert!(!v.character.items[..4].contains(&q[0].kind));
        assert!(fair(&q, &c));
        sales.insert(q[0].kind);
    }
    assert_eq!(sales.len(), 2, "every seller selected the same first spare");
}

#[test]
fn market_life_buy_quote_uses_the_existing_life_sell_tier_and_real_upgrade_need() {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    let (m, v) = fixture(&c, [LIFE, -1, -1, -1, -1, -1, -1, -1]);
    let q = m.quote(&v, &c, None).unwrap().unwrap();
    let gear = c.get(q[1].kind as u16).unwrap();
    assert_eq!(
        q[0],
        Goods {
            kind: LIFE,
            quantity: 1
        }
    );
    assert!(
        gear.tier.unwrap() >= 10,
        "{} is not Life-priced by the selling policy",
        gear.name
    );
    assert!(m.wanted(&v, &c, None, q[1].kind).unwrap() > 0);
    let slot = c
        .get(v.character.class)
        .unwrap()
        .slots
        .iter()
        .position(|s| *s == gear.slot_type)
        .unwrap();
    assert!(
        progression::quality(&v, &c, q[1].kind).unwrap()
            > progression::quality(&v, &c, v.character.items[slot]).unwrap()
    );
    let (seller, stock) = fixture(&c, [0xa0e, -1, -1, -1, -1, -1, -1, -1]);
    let sale = seller.quote(&stock, &c, None).unwrap().unwrap();
    assert_eq!(sale[0].kind, 0xa0e);
    assert_eq!(
        sale[1].kind, DEFENSE,
        "selected XML T8 remains Defense-priced"
    );
}
