use super::tests::fixture;
use super::*;
use rand::rngs::{StdRng, mock::StepRng};

pub(super) fn terrain(v: &mut PlayerView) -> Grid {
    let geometry =
        crate::navigation::Geometry::new((20, 20), vec![0; 400], &BTreeMap::from([(0, 1.)]));
    v.tiles = Arc::new(crate::navigation::Tiles::new(
        geometry,
        (0..20).flat_map(|x| (0..20).map(move |y| (x, y))),
    ));
    Grid::terrain(v)
}
#[test]
fn market_sessions_scatter_on_walkable_nexus_tiles_and_are_bounded() {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    let mut anchors = BTreeSet::new();
    let mut admitted = 0;
    for seed in 0..80 {
        let (mut m, mut v) = fixture(&c, [SPEED, SPEED, SPEED, SPEED, -1, -1, -1, -1]);
        let grid = terrain(&mut v);
        let mut rng = StdRng::seed_from_u64(seed);
        m.decide(&v, &c, None, &grid, &mut rng).unwrap();
        if let Some(session) = m.session.clone() {
            admitted += 1;
            anchors.insert(session.anchor);
            assert!(grid.segment(v.position, center(session.anchor)));
            assert!((60_000..=240_000).contains(&(session.until - v.elapsed_ms)));
            v.elapsed_ms = session.until;
            assert!(m.decide(&v, &c, None, &grid, &mut rng).unwrap().is_none());
            assert!(m.session.is_none());
            assert!(m.decide(&v, &c, None, &grid, &mut rng).unwrap().is_none());
        }
    }
    assert!(admitted > 0 && admitted < 40);
    assert!(anchors.len() > 4);
}
#[test]
fn market_ad_cadence_repeats_truthfully_and_changed_stock_invalidates_pending_generation() {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    let (mut m, mut v) = fixture(&c, [SPEED, SPEED, SPEED, SPEED, -1, -1, -1, -1]);
    let grid = terrain(&mut v);
    m.decide(&v, &c, None, &grid, &mut StepRng::new(0, 1))
        .unwrap();
    v.position = center(m.session.as_ref().unwrap().anchor);
    m.decide(&v, &c, None, &grid, &mut StepRng::new(0, 1))
        .unwrap();
    let ad = m.outgoing.take().unwrap();
    assert_eq!(ad.text, "wts 4 spd for 1 def");
    assert!(m.message_current(&ad, &v, &c, None).unwrap());
    let due = m.next_ad;
    assert!((6000..=12000).contains(&(due - v.elapsed_ms)));
    v.elapsed_ms = due - 1;
    m.decide(&v, &c, None, &grid, &mut StepRng::new(0, 1))
        .unwrap();
    assert!(m.outgoing.is_none());
    v.elapsed_ms = due;
    m.guild = true;
    m.decide(&v, &c, None, &grid, &mut StepRng::new(0, 1))
        .unwrap();
    assert_eq!(m.outgoing.as_ref().unwrap().text, ad.text);
    assert_eq!(m.outgoing.as_ref().unwrap().channel, Channel::Guild);
    v.character.items[4] = -1;
    assert!(!m.message_current(&ad, &v, &c, None).unwrap());
    v.character.items[4] = DEFENSE;
    assert!(!m.message_current(&ad, &v, &c, None).unwrap());
    v.elapsed_ms = m.session.as_ref().unwrap().until;
    assert!(!m.message_current(&ad, &v, &c, None).unwrap());
}
#[test]
fn market_known_vault_goods_are_prepared_through_normal_banking() {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    let (mut m, mut v) = fixture(&c, [-1; 8]);
    m.vault = Some(vec![SPEED, SPEED, SPEED, SPEED, -1, -1, -1, -1]);
    v.objects[0].id = 21;
    v.objects[0].kind = 0x720;
    v.objects[0].usable = true;
    let grid = terrain(&mut v);
    let d = m
        .decide(&v, &c, None, &grid, &mut StepRng::new(0, 1))
        .unwrap()
        .unwrap();
    assert!(matches!(d.journey, Some(Journey::Portal(21))));
    assert!(
        m.outgoing.is_none(),
        "never advertise goods before withdrawing the actual batch"
    );
    v.map.name = "Vault".into();
    v.objects[0].kind = 0x504;
    v.objects[0].contents = m.vault.clone().unwrap();
    let d = m
        .decide(&v, &c, None, &grid, &mut StepRng::new(0, 1))
        .unwrap()
        .unwrap();
    assert!(matches!(d.item, Some(Action::Swap { .. })));
}

#[test]
fn market_bank_preparation_frees_real_capacity_and_aborts_unavailable_travel() {
    let c = Content::load(std::path::Path::new("data")).unwrap();
    let (mut m, mut v) = fixture(&c, [0xa1d; 8]);
    m.vault = Some(vec![SPEED, SPEED, SPEED, SPEED, -1, -1, -1, -1]);
    let grid = terrain(&mut v);
    assert!(
        m.decide(&v, &c, None, &grid, &mut StepRng::new(0, 1))
            .unwrap()
            .is_none()
    );
    assert!(
        !m.reserves_stock(),
        "missing Vault portal releases ordinary activity, not indefinite reservation"
    );
    m.session_used = false;
    v.objects[0].kind = 0x720;
    v.objects[0].usable = true;
    assert!(
        m.decide(&v, &c, None, &grid, &mut StepRng::new(0, 1))
            .unwrap()
            .is_some()
    );
    assert!(m.preparing);
    m.bind();
    assert!(
        m.preparing && m.quote.is_some(),
        "a legitimate bank intent survives ordinary world rebind"
    );
    v.map.name = "Vault".into();
    v.objects[0].kind = 0x504;
    v.objects[0].contents = m.vault.clone().unwrap();
    let d = m
        .decide(&v, &c, None, &grid, &mut StepRng::new(0, 1))
        .unwrap()
        .unwrap();
    assert!(
        matches!(d.item,Some(Action::Swap{from,..}) if from.object==v.session.player),
        "bank room before withdrawing actual potions"
    );
    v.map.name = "Realm".into();
    m.observe(&v);
    assert!(!m.preparing);
}
