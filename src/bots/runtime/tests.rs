use super::*;
use crate::bots::pursuits::market::{Bargain, Goods, Terms};
use crate::store::InventoryWrite;

fn profile() -> Profile {
    let mut profile = ARCHETYPES[0];
    profile.name = "Trader";
    profile.recovery = 0.9;
    profile.companions = 6.;
    profile
}
pub(in crate::bots) async fn actor(
    app: &App,
    guid: &str,
    name: &str,
    backpack: [i32; 8],
) -> (Owner, i32, mpsc::Receiver<Packet>, Brain) {
    let account = app.store.list(guid.into()).await.unwrap().id;
    let ch = app
        .store
        .create(account, app.content.get(crate::content::WIZARD).unwrap())
        .await
        .unwrap();
    let mut items = ch.items.clone();
    items[4..].copy_from_slice(&backpack);
    app.store
        .exchange(
            vec![InventoryWrite {
                id: ch.id,
                account,
                revision: ch.revision,
                items,
            }],
            vec![],
        )
        .await
        .unwrap();
    let mut ch = app.store.load(account, ch.id).await.unwrap();
    ch.name = name.into();
    ch.base[0] = 500;
    ch.base[4] = app.content.classes[&ch.class].caps[4];
    ch.hp = 500;
    let owner = app.claim(account).unwrap();
    let (id, output, _, brain) = enter(app, profile(), ch, -2, None).await.unwrap();
    (owner, id, output, brain)
}
pub(in crate::bots) async fn nearby(app: &App, bot: i32, peer: i32) {
    app.world
        .directory(move |d| {
            let w = d.worlds.get_mut(&-2).unwrap();
            w.players.get_mut(&peer).unwrap().pos = w.players[&bot].pos;
            w.players.get_mut(&peer).unwrap().controller = crate::world::Controller::Human;
            w.updates(bot, true)?;
            w.updates(peer, true)
        })
        .await
        .unwrap();
}
async fn open(app: &App, bot: i32, peer: i32) {
    app.world
        .directory(move |d| {
            let w = d.worlds.get_mut(&-2).unwrap();
            let bot_name = w.players[&bot].character.name.clone();
            let peer_name = w.players[&peer].character.name.clone();
            w.request_trade(peer, bot_name)?;
            w.request_trade(bot, peer_name)
        })
        .await
        .unwrap();
}
async fn drain(app: &App, brain: &mut Brain, output: &mut mpsc::Receiver<Packet>) {
    let view = app.world.player_view(brain.session.1).await.unwrap();
    while let Ok(packet) = output.try_recv() {
        if !social::receive_life(app, brain, &packet, &view)
            .await
            .unwrap()
        {
            assert_ne!(
                social::receive_market(app, brain, &packet, &view)
                    .await
                    .unwrap(),
                social::MarketUptake::PendingCommit
            );
        }
    }
}
async fn advance(app: &App, millis: u64) {
    app.world.clock.set_paused(false);
    tokio::time::sleep(Duration::from_millis(millis)).await;
    app.world.clock.set_paused(true);
}
fn bargain(view: &PlayerView, peer: &PlayerView, goods: [Goods; 2]) -> Bargain {
    Bargain {
        terms: Terms {
            version: rand::random::<u64>().max(1),
            accounts: [view.session.account, peer.session.account],
            names: ["Trader".into(), "Peer".into()],
            goods,
            meeting: [
                -2,
                tile(view.position).0 as i32,
                tile(view.position).1 as i32,
            ],
            expires: pursuits::market::now() + 120,
            batch: 4,
        },
        own_consent: true,
        agreed: true,
        peer_consent: Some([-2, peer.session.player, peer.session.character]),
        confirmed: 0,
        consent_revision: 0,
        rounds: 1,
        resolution: None,
    }
}
async fn injure(app: &App, id: i32, hp: i32) {
    app.world
        .for_player(id)
        .call(move |w| {
            w.players.get_mut(&id).unwrap().character.hp = hp;
            Ok(())
        })
        .await
        .unwrap();
}
async fn assert_closed(app: &App, id: i32) {
    app.world
        .for_player(id)
        .call(move |w| {
            ensure!(
                w.change_trade(id, [false; 12]).is_err(),
                "survival did not revoke the native window"
            );
            Ok(())
        })
        .await
        .unwrap();
}
async fn consume_survival(app: &App, brain: &mut Brain) {
    let view = app.world.player_view(brain.session.1).await.unwrap();
    let decision = decide_before_social(app, brain, &view)
        .await
        .unwrap()
        .expect("trade or gift preempted survival");
    assert_eq!(brain.activity, Activity::Survive);
    assert_closed(app, view.session.player).await;
    let item = decision
        .item
        .expect("prepared injury requires held health potion");
    assert!(matches!(item, Action::Use { .. }));
    let id = view.session.player;
    assert!(
        server::inventory(app, id, move |w| w.autonomous_action(id, item))
            .await
            .unwrap()
            .unwrap()
    );
    let after = app.world.player_view(id).await.unwrap();
    assert_eq!(
        after.character.inventory_revision,
        view.character.inventory_revision + 1
    );
    assert!(after.character.hp > view.character.hp);
}

async fn incoming_window(
    app: &App,
    id: i32,
    peer: i32,
    brain: &mut Brain,
    output: &mut mpsc::Receiver<Packet>,
) {
    nearby(app, id, peer).await;
    app.world
        .for_player(peer)
        .call(move |w| w.request_trade(peer, "Trader".into()))
        .await
        .unwrap();
    drain(app, brain, output).await;
    assert!(brain.mind.market.incoming.is_some());
    for _ in 0..2 {
        advance(app, 400).await;
        let view = app.world.player_view(id).await.unwrap();
        assert!(
            decide_before_social(app, brain, &view)
                .await
                .unwrap()
                .is_none()
        );
        drain(app, brain, output).await;
    }
    assert_eq!(
        brain.mind.market.trade.as_ref().unwrap().offers[0]
            .iter()
            .filter(|b| **b)
            .count(),
        4
    );
}

#[tokio::test]
async fn runtime_market_empty_offer_has_bounded_negotiation_then_releases_controller() {
    let logs = DialogueLogs::default();
    let sink = logs.clone();
    let _subscriber = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .json()
            .with_writer(move || sink.clone())
            .finish(),
    );
    let (dir, app) = server::test_app();
    app.world.clock.set_paused(true);
    assert!(
        app.dialogue.is_none(),
        "modelOFF must not substitute canned speech"
    );
    let (_owner, id, mut output, mut brain) = actor(
        &app,
        "empty-market-bot",
        "Trader",
        [0xa21, 0xa21, 0xa21, 0xa21, -1, -1, -1, -1],
    )
    .await;
    // Even an unoffered payment is not permission to inspect or take human stock.
    let (_peer_owner, peer, mut peer_output, _) = actor(
        &app,
        "empty-market-human",
        "Peer",
        [0xa20, -1, -1, -1, -1, -1, -1, -1],
    )
    .await;
    incoming_window(&app, id, peer, &mut brain, &mut output).await;
    let before = app.world.player_view(id).await.unwrap();
    let human_before = app.world.player_view(peer).await.unwrap();
    advance(&app, 3100).await;
    let view = app.world.player_view(id).await.unwrap();
    assert!(
        decide_before_social(&app, &mut brain, &view)
            .await
            .unwrap()
            .is_none(),
        "empty is an inspection wait, not immediate refusal or gift consent"
    );
    advance(&app, 17_100).await;
    let view = app.world.player_view(id).await.unwrap();
    let released = decide_before_social(&app, &mut brain, &view).await.unwrap();
    assert!(
        released.is_some(),
        "empty human offer still holds controller after 20s negotiation window"
    );
    assert!(brain.mind.market.incoming.is_none());
    assert!(brain.mind.market.trade.is_none());
    assert_closed(&app, id).await;
    assert_eq!(
        app.world.player_view(id).await.unwrap().character.items,
        before.character.items
    );
    assert_eq!(
        app.world
            .player_view(id)
            .await
            .unwrap()
            .character
            .inventory_revision,
        before.character.inventory_revision
    );
    assert_eq!(
        app.world.player_view(peer).await.unwrap().character.items,
        human_before.character.items
    );
    assert_eq!(
        app.world
            .player_view(peer)
            .await
            .unwrap()
            .character
            .inventory_revision,
        human_before.character.inventory_revision
    );
    let mut cancelled = false;
    while let Ok(packet) = peer_output.try_recv() {
        if packet.id == 12 {
            let mut reader = Reader::new(&packet.body);
            assert_ne!(reader.i32().unwrap(), 0);
            assert_eq!(reader.utf().unwrap(), "Trade canceled!");
            reader.done().unwrap();
            cancelled = true;
        }
    }
    assert!(cancelled, "human must receive ordinary native cancellation");
    assert!(
        brain.mind.market.outgoing.is_none(),
        "modelOFF explicitly discards unavailable speech"
    );
    assert!(brain.mind.market.bargain.is_none());
    assert!(
        logs.records()
            .iter()
            .any(|r| r["fields"]["reason"] == "empty_offer_timeout")
    );
    assert!(logs.records().iter().any(|r| r["fields"]["message"]
        == "market speech unavailable: dialogue service disabled; no substitute speech"));
    drain(&app, &mut brain, &mut output).await;
    advance(&app, 400).await;
    let view = app.world.player_view(id).await.unwrap();
    assert!(
        decide_before_social(&app, &mut brain, &view)
            .await
            .unwrap()
            .is_some(),
        "next ordinary decision is no longer held by refused exchange"
    );
    for player in [id, peer] {
        lifecycle::finalize(&app, player, CloseReason::Controller)
            .unwrap()
            .wait()
            .await
            .unwrap();
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn runtime_market_valid_offer_waits_and_changed_offer_restarts_inspection_not_consent() {
    let (dir, app) = server::test_app();
    app.world.clock.set_paused(true);
    let (_owner, id, mut output, mut brain) = actor(
        &app,
        "changed-market-bot",
        "Trader",
        [0xa21, 0xa21, 0xa21, 0xa21, -1, -1, -1, -1],
    )
    .await;
    let (_peer_owner, peer, _peer_output, _) = actor(
        &app,
        "changed-market-human",
        "Peer",
        [0xa20, 0xa22, -1, -1, -1, -1, -1, -1],
    )
    .await;
    incoming_window(&app, id, peer, &mut brain, &mut output).await;
    let mut payment = [false; 12];
    payment[4] = true;
    app.world
        .for_player(peer)
        .call(move |w| w.change_trade(peer, payment))
        .await
        .unwrap();
    drain(&app, &mut brain, &mut output).await;
    advance(&app, 20_100).await;
    let view = app.world.player_view(id).await.unwrap();
    assert!(
        decide_before_social(&app, &mut brain, &view)
            .await
            .unwrap()
            .is_none(),
        "valid payment waits for human acceptance beyond empty-offer deadline"
    );
    let mine = brain.mind.market.trade.as_ref().unwrap().offers[0];
    app.world
        .for_player(peer)
        .call(move |w| w.change_trade(peer, [false; 12]))
        .await
        .unwrap();
    drain(&app, &mut brain, &mut output).await;
    advance(&app, 19_100).await;
    let view = app.world.player_view(id).await.unwrap();
    assert!(
        decide_before_social(&app, &mut brain, &view)
            .await
            .unwrap()
            .is_none(),
        "removing payment gets a fresh bounded inspection wait"
    );
    let mut wrong = [false; 12];
    wrong[5] = true;
    app.world
        .for_player(peer)
        .call(move |w| {
            w.change_trade(peer, wrong)?;
            ensure!(
                w.accept_trade(peer, wrong, mine).is_err(),
                "changed payment cannot inherit prior acceptance/countdown"
            );
            Ok(())
        })
        .await
        .unwrap();
    drain(&app, &mut brain, &mut output).await;
    advance(&app, 3100).await;
    let view = app.world.player_view(id).await.unwrap();
    assert!(
        decide_before_social(&app, &mut brain, &view)
            .await
            .unwrap()
            .is_some(),
        "unmatched selected goods must refuse and reassess"
    );
    assert_closed(&app, id).await;
    assert_eq!(
        app.world
            .player_view(id)
            .await
            .unwrap()
            .character
            .inventory_revision,
        view.character.inventory_revision
    );
    for player in [id, peer] {
        lifecycle::finalize(&app, player, CloseReason::Controller)
            .unwrap()
            .wait()
            .await
            .unwrap();
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn runtime_market_peer_disconnect_or_death_closes_unaccepted_window() {
    for death in [false, true] {
        let (dir, app) = server::test_app();
        app.world.clock.set_paused(true);
        let (_owner, id, mut output, mut brain) = actor(
            &app,
            "depart-market-bot",
            "Trader",
            [0xa21, 0xa21, 0xa21, 0xa21, -1, -1, -1, -1],
        )
        .await;
        let (_peer_owner, peer, _peer_output, _) =
            actor(&app, "depart-market-human", "Peer", [-1; 8]).await;
        incoming_window(&app, id, peer, &mut brain, &mut output).await;
        let human = app.world.player_view(peer).await.unwrap();
        if death {
            app.world
                .for_player(peer)
                .call(move |w| {
                    let p = w.players.get_mut(&peer).unwrap();
                    p.character.hp = 0;
                    p.death_pending = true;
                    Ok(())
                })
                .await
                .unwrap();
        }
        lifecycle::finalize(&app, peer, CloseReason::Transport)
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(
            app.store
                .load(human.session.account, human.session.character)
                .await
                .is_err(),
            death,
            "only committed death removes saved character"
        );
        drain(&app, &mut brain, &mut output).await;
        advance(&app, 400).await;
        let view = app.world.player_view(id).await.unwrap();
        assert!(
            decide_before_social(&app, &mut brain, &view)
                .await
                .unwrap()
                .is_some()
        );
        assert!(brain.mind.market.incoming.is_none());
        assert!(brain.mind.market.trade.is_none());
        assert_closed(&app, id).await;
        assert_eq!(
            app.world
                .player_view(id)
                .await
                .unwrap()
                .character
                .inventory_revision,
            view.character.inventory_revision
        );
        lifecycle::finalize(&app, id, CloseReason::Controller)
            .unwrap()
            .wait()
            .await
            .unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
async fn runtime_market_accepted_commit_is_settled_before_controller_reassessment() {
    let (dir, app) = server::test_app();
    app.world.clock.set_paused(true);
    let (_owner, id, mut output, mut brain) = actor(
        &app,
        "commit-market-bot",
        "Trader",
        [0xa21, 0xa21, 0xa21, 0xa21, -1, -1, -1, -1],
    )
    .await;
    let (_peer_owner, peer, _peer_output, _) = actor(
        &app,
        "commit-market-human",
        "Peer",
        [0xa20, -1, -1, -1, -1, -1, -1, -1],
    )
    .await;
    incoming_window(&app, id, peer, &mut brain, &mut output).await;
    let before = app.world.player_view(id).await.unwrap();
    let human_before = app.world.player_view(peer).await.unwrap();
    let mut payment = [false; 12];
    payment[4] = true;
    app.world
        .for_player(peer)
        .call(move |w| w.change_trade(peer, payment))
        .await
        .unwrap();
    drain(&app, &mut brain, &mut output).await;
    advance(&app, 3100).await;
    let view = app.world.player_view(id).await.unwrap();
    assert!(
        decide_before_social(&app, &mut brain, &view)
            .await
            .unwrap()
            .is_none()
    );
    let mine = brain.mind.market.trade.as_ref().unwrap().offers[0];
    let plan = app
        .world
        .for_player(peer)
        .call(move |w| {
            w.accept_trade(peer, payment, mine)?
                .context("human mutual acceptance must create actual commit plan")
        })
        .await
        .unwrap();
    let transaction = plan.transaction.clone();
    let writes: Vec<_> = plan
        .plans
        .iter()
        .map(|p| InventoryWrite {
            id: p.character,
            account: p.account,
            revision: p.revision,
            items: p.items.clone(),
        })
        .collect();
    advance(&app, 400).await;
    let busy = app.world.player_view(id).await.unwrap();
    assert!(busy.lifecycle.busy);
    assert!(
        decide_before_social(&app, &mut brain, &busy)
            .await
            .unwrap()
            .is_none(),
        "accepted work must fence reassessment"
    );
    server::persist_trade(&app, plan).await.unwrap();
    drain(&app, &mut brain, &mut output).await;
    let after = app.world.player_view(id).await.unwrap();
    let human_after = app.world.player_view(peer).await.unwrap();
    assert_eq!(
        after.character.inventory_revision,
        before.character.inventory_revision + 1
    );
    assert_eq!(
        human_after.character.inventory_revision,
        human_before.character.inventory_revision + 1
    );
    assert_eq!(
        &after.character.items[4..],
        &[0xa20, -1, -1, -1, -1, -1, -1, -1]
    );
    assert_eq!(
        &human_after.character.items[4..],
        &[0xa21, 0xa21, 0xa21, 0xa21, -1, -1, -1, -1]
    );
    let revisions = app
        .store
        .exchange_trade(transaction, writes, None)
        .await
        .unwrap();
    assert_eq!(
        revisions.len(),
        2,
        "immutable durable receipt permits idempotent settlement, not duplicate payment"
    );
    for current in [after, human_after] {
        let saved = app
            .store
            .load(current.session.account, current.session.character)
            .await
            .unwrap();
        assert_eq!(saved.items, current.character.items);
        assert_eq!(saved.revision, current.character.inventory_revision);
    }
    assert!(brain.mind.market.trade.is_none());
    assert!(brain.mind.market.incoming.is_none());
    for player in [id, peer] {
        lifecycle::finalize(&app, player, CloseReason::Controller)
            .unwrap()
            .wait()
            .await
            .unwrap();
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn runtime_coordinator_survival_preempts_active_incoming_gift_and_legacy_windows() {
    let (dir, app) = server::test_app();
    app.world.clock.set_paused(true);
    let (_owner, id, mut output, mut brain) = actor(
        &app,
        "runtime-survival-bot",
        "Trader",
        [0xa22, 0xa22, 0xa22, 0xa22, 0xa21, 0xa21, 0xa21, 0xa21],
    )
    .await;
    let (_peer_owner, peer, _peer_output, _) =
        actor(&app, "runtime-survival-peer", "Peer", [0xa20; 8]).await;
    nearby(&app, id, peer).await;
    let view = app.world.player_view(id).await.unwrap();
    let grid = Grid::terrain(&view);
    let target = view
        .tiles
        .points()
        .map(center)
        .find(|p| {
            (3.0..5.0).contains(&p.distance(view.position)) && grid.segment(view.position, *p)
        })
        .unwrap();
    let received = app.world.clock.now();
    app.world
        .directory(move |d| {
            let w = d.worlds.get_mut(&-2).unwrap();
            w.players.get_mut(&peer).unwrap().pos = target;
            w.updates(id, true)?;
            d.chat(peer, "wts 1 def for 4 spd".into(), received)
        })
        .await
        .unwrap();
    drain(&app, &mut brain, &mut output).await;
    let view = app.world.player_view(id).await.unwrap();
    let approach = decide_before_social(&app, &mut brain, &view)
        .await
        .unwrap()
        .expect("unopened rendezvous must allow approach movement");
    assert!(approach.movement.is_some());
    assert!(brain.mind.market.trade.is_none());
    advance(&app, 400).await;
    nearby(&app, id, peer).await;
    open(&app, id, peer).await;
    drain(&app, &mut brain, &mut output).await;
    injure(&app, id, 400).await;
    let view = app.world.player_view(id).await.unwrap();
    assert!(brain.mind.market.incoming.is_some());
    assert!(
        social::market(&app, &mut brain, &view).await.unwrap(),
        "active incoming window should protect ordinary movement"
    );
    consume_survival(&app, &mut brain).await;
    assert!(brain.mind.market.incoming.is_none());
    drain(&app, &mut brain, &mut output).await;
    advance(&app, 400).await;
    injure(&app, id, 400).await;
    let received = app.world.clock.now();
    app.world
        .directory(move |d| {
            d.chat(peer, "Trader, spare health?".into(), received)?;
            d.chat(peer, "Trader, yes health".into(), received)
        })
        .await
        .unwrap();
    drain(&app, &mut brain, &mut output).await;
    assert!(brain.mind.social.gift_active());
    open(&app, id, peer).await;
    drain(&app, &mut brain, &mut output).await;
    let view = app.world.player_view(id).await.unwrap();
    assert!(
        social::life(&app, &mut brain, &view).await.unwrap(),
        "active generous gift previously preempted reflexes"
    );
    consume_survival(&app, &mut brain).await;
    assert!(!brain.mind.social.gift_active());
    assert!(
        brain.mind.social.episodes.is_empty(),
        "a cancelled gift is not a committed social fact"
    );
    drain(&app, &mut brain, &mut output).await;
    advance(&app, 400).await;
    injure(&app, id, 400).await;
    let view = app.world.player_view(id).await.unwrap();
    let peer_view = app.world.player_view(peer).await.unwrap();
    brain.mind.market.bargain = Some(bargain(
        &view,
        &peer_view,
        [
            Goods {
                kind: 0xa22,
                quantity: 1,
            },
            Goods {
                kind: 0xa20,
                quantity: 1,
            },
        ],
    ));
    open(&app, id, peer).await;
    drain(&app, &mut brain, &mut output).await;
    let view = app.world.player_view(id).await.unwrap();
    assert!(social::market(&app, &mut brain, &view).await.unwrap());
    consume_survival(&app, &mut brain).await;
    let b = brain.mind.market.bargain.as_ref().unwrap();
    assert_eq!(b.confirmed, 0);
    assert!(
        b.resolution.is_none(),
        "emergency interruption cannot fabricate settlement or erase the obligation"
    );
    assert!(
        app.store
            .market_receipts(view.session.account, b.terms.version)
            .await
            .unwrap()
            .is_empty()
    );
    drain(&app, &mut brain, &mut output).await;
    open(&app, id, peer).await;
    drain(&app, &mut brain, &mut output).await;
    let view = app.world.player_view(id).await.unwrap();
    social::market(&app, &mut brain, &view).await.unwrap();
    let mut theirs = [false; 12];
    theirs[4] = true;
    app.world
        .for_player(peer)
        .call(move |w| w.change_trade(peer, theirs))
        .await
        .unwrap();
    drain(&app, &mut brain, &mut output).await;
    advance(&app, 3100).await;
    let delivery = brain
        .mind
        .market
        .bargain
        .as_ref()
        .unwrap()
        .terms
        .delivery(0)
        .unwrap();
    let session = [-2, id, view.session.character];
    let mine = brain.mind.market.trade.as_ref().unwrap().offers[0];
    app.world
        .for_player(id)
        .call(move |w| {
            ensure!(
                w.autonomous_trade(
                    id,
                    session,
                    crate::world::TradeAction::Accept { mine, theirs },
                    delivery
                )?
                .is_none(),
                "human has not accepted yet"
            );
            Ok(())
        })
        .await
        .unwrap();
    injure(&app, id, 400).await;
    let stale = app.world.player_view(id).await.unwrap();
    let plan = app
        .world
        .for_player(peer)
        .call(move |w| {
            w.accept_trade(peer, theirs, mine)?
                .context("mutual native acceptance missing")
        })
        .await
        .unwrap();
    server::persist_trade(&app, plan).await.unwrap();
    assert!(
        decide_before_social(&app, &mut brain, &stale)
            .await
            .unwrap()
            .is_none(),
        "published receipt must not release an item action planned from the old inventory revision"
    );
    assert_eq!(brain.activity, Activity::Survive);
    assert_eq!(brain.mind.market.bargain.as_ref().unwrap().confirmed, 1);
    let current = app.world.player_view(id).await.unwrap();
    assert_eq!(
        current.character.inventory_revision,
        stale.character.inventory_revision + 1
    );
    assert_eq!(
        current.character.hp, stale.character.hp,
        "stale safety consumption must not run"
    );
    lifecycle::finalize(&app, id, CloseReason::Controller)
        .unwrap()
        .wait()
        .await
        .unwrap();
    lifecycle::finalize(&app, peer, CloseReason::Controller)
        .unwrap()
        .wait()
        .await
        .unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn runtime_coordinator_rebind_resumes_paid_installments_only_through_fresh_native_windows() {
    let (dir, app) = server::test_app();
    app.world.clock.set_paused(true);
    let (_owner, id, mut output, mut brain) =
        actor(&app, "runtime-installment-bot", "Trader", [0xa21; 8]).await;
    let (_peer_owner, peer, mut peer_output, _) = actor(
        &app,
        "runtime-installment-peer",
        "Peer",
        [0xa20, 0xa20, 0xa20, 0xa20, -1, -1, -1, -1],
    )
    .await;
    nearby(&app, id, peer).await;
    let view = app.world.player_view(id).await.unwrap();
    let peer_view = app.world.player_view(peer).await.unwrap();
    brain.mind.market.bargain = Some(bargain(
        &view,
        &peer_view,
        [
            Goods {
                kind: 0xa21,
                quantity: 8,
            },
            Goods {
                kind: 0xa20,
                quantity: 4,
            },
        ],
    ));
    let terms = brain.mind.market.bargain.as_ref().unwrap().terms.clone();
    for ordinal in 0..2 {
        if ordinal == 0 {
            open(&app, id, peer).await;
        } else {
            app.world
                .for_player(peer)
                .call(move |w| w.request_trade(peer, "Trader".into()))
                .await
                .unwrap();
        }
        drain(&app, &mut brain, &mut output).await;
        if ordinal == 1 {
            brain.mind.market.trade.as_mut().unwrap().binding[0] = -3;
            brain.mind.market.bargain.as_mut().unwrap().own_consent = true;
            brain.mind.market.bargain.as_mut().unwrap().peer_consent =
                Some([-3, peer, peer_view.session.character]);
            let view = app.world.player_view(id).await.unwrap();
            social::market(&app, &mut brain, &view).await.unwrap();
            assert_closed(&app, id).await;
            assert_eq!(
                app.store
                    .market_receipts(view.session.account, terms.version)
                    .await
                    .unwrap()
                    .len(),
                1,
                "stale window consent cannot settle the balance"
            );
            drain(&app, &mut brain, &mut output).await;
            brain.mind.market.bind();
            brain.mind.market.name = "Trader".into();
            let view = app.world.player_view(id).await.unwrap();
            social::market(&app, &mut brain, &view).await.unwrap();
            app.world
                .for_player(peer)
                .call(move |w| w.request_trade(peer, "Trader".into()))
                .await
                .unwrap();
            drain(&app, &mut brain, &mut output).await;
            assert!(!brain.mind.market.bargain.as_ref().unwrap().own_consent);
            let view = app.world.player_view(id).await.unwrap();
            assert!(
                decide_before_social(&app, &mut brain, &view)
                    .await
                    .unwrap()
                    .is_none(),
                "fresh native window protects movement while offers settle"
            );
        } else {
            let view = app.world.player_view(id).await.unwrap();
            assert!(social::market(&app, &mut brain, &view).await.unwrap());
        }
        let theirs = app
            .world
            .player_view(peer)
            .await
            .unwrap()
            .character
            .items
            .iter()
            .enumerate()
            .fold([false; 12], |mut mask, (i, k)| {
                if i >= 4 && *k == 0xa20 && mask.iter().filter(|b| **b).count() < 4 {
                    mask[i] = true;
                }
                mask
            });
        app.world
            .for_player(peer)
            .call(move |w| w.change_trade(peer, theirs))
            .await
            .unwrap();
        drain(&app, &mut brain, &mut output).await;
        advance(&app, 3100).await;
        if ordinal == 1 {
            injure(&app, id, 50).await;
        }
        let view = app.world.player_view(id).await.unwrap();
        let mine = brain.mind.market.trade.as_ref().unwrap().offers[0];
        let delivery = terms.delivery(ordinal).unwrap();
        let session = [-2, id, view.session.character];
        let plan = app
            .world
            .directory(move |d| {
                let w = d.worlds.get_mut(&-2).unwrap();
                assert!(
                    w.autonomous_trade(
                        id,
                        session,
                        crate::world::TradeAction::Accept { mine, theirs },
                        delivery
                    )?
                    .is_none()
                );
                w.accept_trade(peer, theirs, mine)?
                    .context("fresh mutual native acceptance must produce an atomic plan")
            })
            .await
            .unwrap();
        assert_eq!(
            brain.mind.market.bargain.as_ref().unwrap().confirmed,
            ordinal
        );
        if ordinal == 1 {
            assert!(
                decide_before_social(&app, &mut brain, &view)
                    .await
                    .unwrap()
                    .is_none(),
                "pending atomic commit must fence the safety journey"
            );
            assert_eq!(brain.activity, Activity::Survive);
            assert!(app.world.player_view(id).await.unwrap().lifecycle.busy);
            assert_eq!(brain.mind.market.bargain.as_ref().unwrap().confirmed, 1);
        }
        server::persist_trade(&app, plan).await.unwrap();
        drain(&app, &mut brain, &mut output).await;
        if ordinal == 0 {
            let saved = serde_json::to_string(&brain.mind.market).unwrap();
            brain.mind.market = serde_json::from_str(&saved).unwrap();
            brain.mind.market.bind();
            brain.mind.market.name = "Trader".into();
            assert_eq!(
                brain.mind.market.bargain.as_ref().unwrap().confirmed,
                0,
                "UI success must not mark an installment paid"
            );
            social::recover_market(&app, &mut brain).await.unwrap();
            assert_eq!(brain.mind.market.bargain.as_ref().unwrap().confirmed, 1);
            assert!(!brain.mind.market.bargain.as_ref().unwrap().own_consent);
            assert!(
                brain
                    .mind
                    .market
                    .bargain
                    .as_ref()
                    .unwrap()
                    .peer_consent
                    .is_none()
            );
            brain.mind.market.bargain.as_mut().unwrap().own_consent = true;
            brain.mind.market.bargain.as_mut().unwrap().peer_consent =
                Some([-3, peer, peer_view.session.character]);
            app.world
                .for_player(peer)
                .call(move |w| {
                    w.players.get_mut(&peer).unwrap().character.name = "Unknown".into();
                    w.updates(id, true)
                })
                .await
                .unwrap();
            let view = app.world.player_view(id).await.unwrap();
            assert!(
                !social::market(&app, &mut brain, &view).await.unwrap(),
                "unknown peer cannot renew consent"
            );
            app.world
                .for_player(peer)
                .call(move |w| {
                    w.players.get_mut(&peer).unwrap().character.name = "Peer".into();
                    w.updates(id, true)
                })
                .await
                .unwrap();
            brain.mind.market.bind();
            brain.mind.market.name = "Trader".into();
            advance(&app, 400).await;
            while peer_output.try_recv().is_ok() {}
            let view = app.world.player_view(id).await.unwrap();
            social::market(&app, &mut brain, &view).await.unwrap();
            assert!(
                std::iter::from_fn(|| peer_output.try_recv().ok()).any(|p| p.id == 61),
                "reload must request a fresh native window without restoring old chat consent"
            );
            assert!(!brain.mind.market.bargain.as_ref().unwrap().own_consent);
        }
    }
    advance(&app, 400).await;
    let view = app.world.player_view(id).await.unwrap();
    let decision = decide_before_social(&app, &mut brain, &view)
        .await
        .unwrap()
        .expect("completed receipt must release safety movement");
    assert_eq!(brain.activity, Activity::Survive);
    assert_eq!(decision.goal, Goal::Recover);
    let b = brain.mind.market.bargain.as_ref().unwrap();
    assert_eq!(b.confirmed, 2);
    assert_eq!(
        b.resolution.as_deref(),
        Some("settled_all_installments_transaction_identified")
    );
    assert_eq!(
        app.store
            .market_receipts(view.session.account, terms.version)
            .await
            .unwrap()
            .len(),
        2
    );
    lifecycle::finalize(&app, id, CloseReason::Controller)
        .unwrap()
        .wait()
        .await
        .unwrap();
    lifecycle::finalize(&app, peer, CloseReason::Controller)
        .unwrap()
        .wait()
        .await
        .unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[derive(Clone, Default)]
struct DialogueLogs(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for DialogueLogs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl DialogueLogs {
    fn records(&self) -> Vec<serde_json::Value> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    async fn wait(&self, message: &str, player: i32) -> serde_json::Value {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(row) = self
                    .records()
                    .into_iter()
                    .find(|r| r["fields"]["message"] == message && r["fields"]["player"] == player)
                {
                    return row;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("missing {message}: {:?}", self.records()))
    }
}

#[tokio::test]
async fn society_runtime_cancels_same_pending_request_without_chat_application_or_failure() {
    let logs = DialogueLogs::default();
    let sink = logs.clone();
    let _subscriber = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .json()
            .with_ansi(false)
            .with_writer(move || sink.clone())
            .finish(),
    );
    let (dir, app) = server::test_app();
    app.world.clock.set_paused(true);
    let (peer_owner, peer, mut peer_output, _) =
        actor(&app, "runtime-outing-peer", "Peer", [-1; 8]).await;
    let peer_view = app.world.player_view(peer).await.unwrap();
    app.society
        .lock()
        .unwrap()
        .register(peer_view.session.account, Default::default());
    let account = app
        .store
        .list("runtime-outing-bot".into())
        .await
        .unwrap()
        .id;
    let mut character = app
        .store
        .create(account, app.content.get(crate::content::WIZARD).unwrap())
        .await
        .unwrap();
    character.base[0] = 500;
    character.hp = 500;
    let owner = app.claim(account).unwrap();
    let selected = character.id;
    start_selected(&app, profile(), character, -2, peer_view.position, owner)
        .await
        .unwrap();
    let id = app
        .world
        .directory(move |d| {
            Ok(d.worlds[&-2]
                .players
                .iter()
                .find(|(_, p)| p.character.id == selected)
                .unwrap()
                .0
                .to_owned())
        })
        .await
        .unwrap();
    while peer_output.try_recv().is_ok() {}
    app.world.clock.set_paused(false);
    let scheduled = logs.wait("bot dialogue publication scheduled", id).await;
    let request = scheduled["fields"]["request"].as_str().unwrap();
    {
        let mut state = app.society.lock().unwrap();
        for peer in 100..105 {
            state.apply(
                adventures::Member {
                    account,
                    name: "Trader".into(),
                },
                adventures::Plan::Answer(adventures::Offer {
                    from: adventures::Member {
                        account,
                        name: "Trader".into(),
                    },
                    to: adventures::Member {
                        account: peer,
                        name: format!("Member{peer}"),
                    },
                    purpose: adventures::Purpose::Equipment,
                    until: 60000,
                    text: "want to hunt?".into(),
                    answer_started: true,
                }),
                true,
                "yes".into(),
                0,
            );
        }
        assert_eq!(state.group(account, 0).unwrap().members.len(), 6);
    }
    let canceled = logs
        .wait("bot dialogue canceled; pending intention not published", id)
        .await;
    assert_eq!(canceled["fields"]["request"], request);
    assert_eq!(canceled["level"], "INFO");
    for row in logs
        .records()
        .into_iter()
        .filter(|r| r["fields"]["request"] == request)
    {
        assert_ne!(row["level"], "ERROR");
        assert_ne!(row["fields"]["message"], "bot dialogue intention applied");
        assert_ne!(row["fields"]["message"], "bot dialogue provider succeeded");
    }
    assert!(!std::iter::from_fn(|| peer_output.try_recv().ok()).any(|p| p.id == wire::TEXT));
    assert!(app.society.lock().unwrap().outgoing(account, 0).is_none());
    lifecycle::finalize(&app, peer, CloseReason::Controller)
        .unwrap()
        .wait()
        .await
        .unwrap();
    drop(peer_owner);
    app.shutdown.send_replace(true);
    lifecycle::drain::terminate(&app).await.unwrap();
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn drain_waits_for_accepted_bot_reentry_after_source_save() {
    let (dir, app) = server::test_app();
    app.world.clock.set_paused(true);
    let (mut owner, id, _, brain) = actor(&app, "drain-reentry", "Trader", [-1; 8]).await;
    let account = app.world.player_view(id).await.unwrap().session.account;
    let character = brain.session.2;
    let (_, completion) = owner.control(true).unwrap();
    let controller_entry = lifecycle::drain::entry(&app).unwrap();
    let entry = lifecycle::drain::entry(&app).unwrap();
    let (time, key, reservation) = app
        .world
        .directory(move |d| {
            let world = d.worlds.get_mut(&-2).unwrap();
            world.players.get_mut(&id).unwrap().character.lifetime.tiles += 1;
            let (time, key, _) = d.issue(account, character, 1)?;
            let reservation = d.reserve_transfer(key.clone())?;
            Ok((time, key, reservation))
        })
        .await
        .unwrap();
    lifecycle::finalize_transfer(&app, id, reservation)
        .unwrap()
        .wait()
        .await
        .unwrap();
    let saved = app.store.load(account, character).await.unwrap();
    assert_eq!(lifecycle::state(&app, account), Some((false, None, false)));
    let (ready, held) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let controller_app = app.clone();
    let controller = tokio::spawn(async move {
        let _controller_entry = controller_entry;
        let _entry = entry;
        ready.send(()).unwrap();
        released.await.unwrap();
        let mut owner = Some(owner);
        let mut expected = None;
        let result: Result<()> = async {
            let (id, ..) = reenter(
                &controller_app,
                profile(),
                account,
                &mut owner,
                _entry,
                Reentry::Portal {
                    destination: 1,
                    time,
                    key,
                    session: crate::world::view::Session {
                        world: -2,
                        player: id,
                        account,
                        character,
                    },
                    selected: Some(character),
                },
            )
            .await?;
            expected = Some(
                controller_app
                    .world
                    .for_player(id)
                    .call(move |w| Ok(w.players[&id].character.clone()))
                    .await?,
            );
            Ok(())
        }
        .await;
        let closing = close_controller(
            &controller_app,
            owner.take().unwrap(),
            result.as_ref().err(),
            CloseReason::Shutdown,
        )
        .unwrap();
        if let Some(closing) = closing {
            closing.wait().await.unwrap();
        }
        completion.complete(result);
        expected
    });
    held.await.unwrap();
    app.shutdown.send_replace(true);
    lifecycle::drain::prepare(&app);
    let draining_app = app.clone();
    let draining = tokio::spawn(async move { lifecycle::drain::terminate(&draining_app).await });
    tokio::task::yield_now().await;
    assert!(!draining.is_finished());
    assert!(!controller.is_finished());
    release.send(()).unwrap();
    let expected = tokio::time::timeout(Duration::from_secs(5), controller)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), draining)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(lifecycle::state(&app, account).is_none());
    assert!(!app.world.supervisor.account_fenced(account).unwrap());
    let expected = expected.unwrap();
    assert!(expected.lifetime.tiles >= saved.lifetime.tiles);
    assert_eq!(app.store.load(account, character).await.unwrap(), expected);
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[derive(Clone, Copy)]
enum EntryBoundary {
    Freeze,
    Shutdown,
    Protocol,
    World,
    Io,
    Claim,
}
thread_local! {
    static ENTRY_BOUNDARY: std::cell::Cell<Option<(i32, EntryBoundary)>> = const { std::cell::Cell::new(None) };
}
pub(super) async fn entry_boundary(app: &App, id: i32, admitted: bool) -> Result<()> {
    let hook = ENTRY_BOUNDARY.with(|slot| {
        let (player, mode) = slot.get()?;
        let after = !matches!(mode, EntryBoundary::Freeze | EntryBoundary::Shutdown);
        if player != id || admitted != after {
            return None;
        }
        slot.set(None);
        Some(mode)
    });
    let Some(mode) = hook else {
        return Ok(());
    };
    if matches!(mode, EntryBoundary::Shutdown) {
        app.shutdown.send_replace(true);
    }
    lifecycle::drain::prepare(app);
    match mode {
        EntryBoundary::Freeze | EntryBoundary::Shutdown => Ok(()),
        EntryBoundary::Protocol => {
            app.world
                .for_player(id)
                .call(move |w| w.acknowledge_autonomous_goto(id))
                .await
        }
        EntryBoundary::World => {
            app.world
                .directory::<()>(|_| anyhow::bail!("post-freeze World failure"))
                .await
        }
        EntryBoundary::Io => {
            let path = std::env::temp_dir().join("drain-entry-io-never-created");
            assert!(!path.exists());
            std::fs::File::open(path)?;
            Ok(())
        }
        EntryBoundary::Claim => app.try_claim(999_999).map(drop),
    }
}
async fn drain_entry_bot(app: &App) -> (i32, i32, i32) {
    let account = app.store.list("drain-entry-bot".into()).await.unwrap().id;
    let ch = app
        .store
        .create(account, app.content.get(crate::content::WIZARD).unwrap())
        .await
        .unwrap();
    let character = ch.id;
    let owner = app.claim(account).unwrap();
    start_selected(app, profile(), ch, -2, Pos { x: 77.5, y: 72.5 }, owner)
        .await
        .unwrap();
    let id = app
        .world
        .directory(move |d| {
            Ok(*d.worlds[&-2]
                .players
                .iter()
                .find(|(_, p)| p.character.id == character)
                .unwrap()
                .0)
        })
        .await
        .unwrap();
    (account, character, id)
}
async fn reach_entry_boundary(app: &App, id: i32, mode: EntryBoundary) {
    ENTRY_BOUNDARY.with(|slot| slot.set(Some((id, mode))));
    app.world.clock.set_paused(false);
    tokio::time::timeout(Duration::from_secs(5), async {
        while ENTRY_BOUNDARY.with(|slot| slot.get().is_some()) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
fn assert_entry_stop(logs: &DialogueLogs, id: i32) {
    let rows: Vec<_> = logs
        .records()
        .into_iter()
        .filter(|r| r["fields"]["player"] == id)
        .collect();
    assert!(
        rows.iter().any(|r| r["level"] == "INFO"
            && r["fields"]["message"] == "autonomous controller stopped before admission"),
        "{rows:?}"
    );
    assert!(!rows.iter().any(|r| r["level"] == "ERROR"), "{rows:?}");
}

#[tokio::test]
async fn drain_entry_runtime_frozen_before_launch_releases_unpublished_claim() {
    let (dir, app) = server::test_app();
    app.world.clock.set_paused(true);
    let account = app
        .store
        .list("frozen-before-launch".into())
        .await
        .unwrap()
        .id;
    let ch = app
        .store
        .create(account, app.content.get(crate::content::WIZARD).unwrap())
        .await
        .unwrap();
    let character = ch.id;
    let owner = app.claim(account).unwrap();
    lifecycle::drain::prepare(&app);
    let error = start_selected(&app, profile(), ch, -2, Pos { x: 77.5, y: 72.5 }, owner)
        .await
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("controller entry interrupted by drain"),
        "{error:#}"
    );
    assert!(
        app.world
            .directory(move |d| Ok(d
                .worlds
                .values()
                .all(|w| w.players.values().all(|p| p.character.id != character))))
            .await
            .unwrap()
    );
    assert_eq!(app.cctv.binding(profile().name), None);
    assert!(
        lifecycle::state(&app, account).is_none(),
        "unpublished rejected launch retained a claim"
    );
    tokio::time::timeout(Duration::from_secs(5), lifecycle::drain::terminate(&app))
        .await
        .unwrap()
        .unwrap();
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn drain_entry_runtime_freeze_before_new_entry_is_expected_stop() {
    for mode in [EntryBoundary::Freeze, EntryBoundary::Shutdown] {
        let logs = DialogueLogs::default();
        let sink = logs.clone();
        let _subscriber = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .json()
                .with_writer(move || sink.clone())
                .finish(),
        );
        let (dir, app) = server::test_app();
        app.world.clock.set_paused(true);
        let (account, _, id) = drain_entry_bot(&app).await;
        reach_entry_boundary(&app, id, mode).await;
        tokio::time::timeout(Duration::from_secs(5), lifecycle::drain::terminate(&app))
            .await
            .unwrap()
            .unwrap();
        assert!(lifecycle::state(&app, account).is_none());
        assert!(!app.world.supervisor.account_fenced(account).unwrap());
        assert_entry_stop(&logs, id);
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
async fn drain_entry_runtime_preserves_actual_failures_after_admission_even_when_frozen() {
    for mode in [
        EntryBoundary::Protocol,
        EntryBoundary::World,
        EntryBoundary::Io,
        EntryBoundary::Claim,
    ] {
        let logs = DialogueLogs::default();
        let sink = logs.clone();
        let _subscriber = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .json()
                .with_writer(move || sink.clone())
                .finish(),
        );
        let (dir, app) = server::test_app();
        app.world.clock.set_paused(true);
        let (account, _, id) = drain_entry_bot(&app).await;
        reach_entry_boundary(&app, id, mode).await;
        let stopped = logs.wait("autonomous controller stopped", id).await;
        assert_eq!(stopped["level"], "ERROR");
        let error = stopped["fields"]["error"].as_str().unwrap();
        let expected = match mode {
            EntryBoundary::Protocol => "unsolicited GOTO acknowledgement",
            EntryBoundary::World => "post-freeze World failure",
            EntryBoundary::Io => "No such file or directory",
            EntryBoundary::Claim => "server admission is frozen",
            _ => unreachable!(),
        };
        assert!(error.contains(expected), "{stopped}");
        assert!(!logs.records().iter().any(|r| r["fields"]["player"] == id
            && r["fields"]["message"] == "autonomous controller stopped before admission"));
        let drain = tokio::time::timeout(Duration::from_secs(5), lifecycle::drain::terminate(&app))
            .await
            .unwrap();
        let failure = format!("{:#}", drain.unwrap_err());
        assert!(failure.contains(expected), "{failure}");
        assert!(lifecycle::state(&app, account).is_some_and(|state| state.2));
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
async fn drain_entry_runtime_waits_for_already_accepted_progress_before_release() {
    let logs = DialogueLogs::default();
    let sink = logs.clone();
    let _subscriber = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .json()
            .with_writer(move || sink.clone())
            .finish(),
    );
    let (dir, app) = server::test_app();
    app.world.clock.set_paused(true);
    let (account, character, id) = drain_entry_bot(&app).await;
    let expected_tiles = app
        .world
        .for_player(id)
        .call(move |w| {
            let lifetime = &mut w.players.get_mut(&id).unwrap().character.lifetime;
            lifetime.tiles += 1;
            Ok(lifetime.tiles)
        })
        .await
        .unwrap();
    let (committed, release) = app.store.lifecycle_test_pause_reply();
    let saving_app = app.clone();
    let saving = tokio::spawn(async move { server::persistence_tick(&saving_app).await });
    tokio::time::timeout(Duration::from_secs(5), committed)
        .await
        .unwrap()
        .unwrap();
    reach_entry_boundary(&app, id, EntryBoundary::Shutdown).await;
    let draining_app = app.clone();
    let draining = tokio::spawn(async move { lifecycle::drain::terminate(&draining_app).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!saving.is_finished());
    assert!(!draining.is_finished());
    assert!(lifecycle::state(&app, account).unwrap().0);
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), saving)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), draining)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(lifecycle::state(&app, account).is_none());
    assert!(!app.world.supervisor.account_fenced(account).unwrap());
    assert_eq!(
        app.store
            .load(account, character)
            .await
            .unwrap()
            .lifetime
            .tiles,
        expected_tiles
    );
    assert_eq!(
        app.store.load(account, character).await.unwrap().revision,
        0
    );
    assert_entry_stop(&logs, id);
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

struct DialogueWorldBoundary {
    player: i32,
    ready: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
}
thread_local! {
    static DIALOGUE_WORLD_BOUNDARY: std::cell::RefCell<Option<DialogueWorldBoundary>> = const { std::cell::RefCell::new(None) };
}
pub(in crate::bots) async fn dialogue_world_boundary(player: i32) {
    let hook = DIALOGUE_WORLD_BOUNDARY.with(|slot| {
        let mut hook = slot.borrow_mut();
        if hook.as_ref().is_some_and(|hook| hook.player == player) {
            hook.take()
        } else {
            None
        }
    });
    if let Some(hook) = hook {
        hook.ready.send(()).unwrap();
        hook.release.await.unwrap();
    }
}

#[tokio::test]
async fn dialogue_world_runtime_cancels_after_validated_view_before_authoritative_publication() {
    let logs = DialogueLogs::default();
    let sink = logs.clone();
    let _subscriber = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .json()
            .with_ansi(false)
            .with_writer(move || sink.clone())
            .finish(),
    );
    let (dir, app) = server::test_app();
    app.world.clock.set_paused(true);
    let (peer_owner, peer, mut peer_output, _) =
        actor(&app, "world-dialogue-peer", "Peer", [-1; 8]).await;
    let peer_view = app.world.player_view(peer).await.unwrap();
    app.society
        .lock()
        .unwrap()
        .register(peer_view.session.account, Default::default());
    let account = app
        .store
        .list("world-dialogue-bot".into())
        .await
        .unwrap()
        .id;
    let mut character = app
        .store
        .create(account, app.content.get(crate::content::WIZARD).unwrap())
        .await
        .unwrap();
    character.base[0] = 500;
    character.hp = 500;
    let selected = character.id;
    let owner = app.claim(account).unwrap();
    start_selected(&app, profile(), character, -2, peer_view.position, owner)
        .await
        .unwrap();
    let id = app
        .world
        .directory(move |d| {
            Ok(*d.worlds[&-2]
                .players
                .iter()
                .find(|(_, p)| p.character.id == selected)
                .unwrap()
                .0)
        })
        .await
        .unwrap();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, waiting) = tokio::sync::oneshot::channel();
    DIALOGUE_WORLD_BOUNDARY.with(|slot| {
        *slot.borrow_mut() = Some(DialogueWorldBoundary {
            player: id,
            ready: entered,
            release: waiting,
        })
    });
    while peer_output.try_recv().is_ok() {}
    app.world.clock.set_paused(false);
    let scheduled = logs.wait("bot dialogue publication scheduled", id).await;
    let request = scheduled["fields"]["request"].as_str().unwrap();
    tokio::time::timeout(Duration::from_secs(10), ready)
        .await
        .unwrap()
        .unwrap();
    app.world.clock.set_paused(true);
    app.world
        .directory(move |d| {
            let w = d.worlds.get_mut(&-2).unwrap();
            w.dialogue_participants(id, peer)?;
            assert!(w.players[&id].known.contains(&peer));
            let pos = w.players[&id].pos;
            w.players.get_mut(&peer).unwrap().pos = Pos {
                x: pos.x + 13.,
                y: pos.y,
            };
            Ok(())
        })
        .await
        .unwrap();
    release.send(()).unwrap();
    let row = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(row) = logs.records().into_iter().find(|r| {
                r["fields"]["request"] == request
                    && matches!(
                        r["fields"]["message"].as_str(),
                        Some(
                            "bot dialogue canceled; pending intention not published"
                                | "bot dialogue intention rejected"
                        )
                    )
            }) {
                break row;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(row["level"], "INFO", "{row}");
    assert_eq!(row["fields"]["reason"], "speaker_out_of_chat_range");
    for row in logs
        .records()
        .into_iter()
        .filter(|r| r["fields"]["request"] == request)
    {
        assert_ne!(row["level"], "ERROR");
        assert_ne!(row["fields"]["message"], "bot dialogue intention applied");
        assert_ne!(
            row["fields"]["message"],
            "bot dialogue interaction unavailable"
        );
        assert_ne!(row["fields"]["message"], "bot dialogue provider succeeded");
    }
    assert!(!std::iter::from_fn(|| peer_output.try_recv().ok()).any(|p| p.id == wire::TEXT));
    assert!(app.society.lock().unwrap().outgoing(account, 0).is_none());
    assert!(app.society.lock().unwrap().group(account, 0).is_none());
    lifecycle::finalize(&app, peer, CloseReason::Controller)
        .unwrap()
        .wait()
        .await
        .unwrap();
    drop(peer_owner);
    app.shutdown.send_replace(true);
    lifecycle::drain::terminate(&app).await.unwrap();
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn dialogue_world_publication_keeps_intent_owner_and_chat_errors_fatal() {
    for (drift, malformed_owner) in [
        ("range", None),
        ("known", None),
        ("ignore", None),
        ("peer_left", None),
        ("peer_dead", None),
        ("peer_departing", None),
        ("text", None),
        ("rate", None),
        ("absent_invalid_owner", Some((true, f32::NAN))),
        ("absent_invalid_owner", Some((true, f32::INFINITY))),
        ("absent_invalid_owner", Some((true, f32::NEG_INFINITY))),
        ("absent_invalid_owner", Some((false, f32::NAN))),
        ("absent_invalid_owner", Some((false, f32::INFINITY))),
        ("absent_invalid_owner", Some((false, f32::NEG_INFINITY))),
    ] {
        let (dir, app) = server::test_app();
        app.world.clock.set_paused(true);
        let (_owner, id, mut output, mut brain) =
            actor(&app, "world-guards-bot", "Trader", [-1; 8]).await;
        let (_peer_owner, peer, mut peer_output, _) =
            actor(&app, "world-guards-peer", "Peer", [-1; 8]).await;
        nearby(&app, id, peer).await;
        let view = app.world.player_view(id).await.unwrap();
        let own = view.session.account;
        let other = app.world.player_view(peer).await.unwrap().session.account;
        let mut request = brain
            .adventure_request(
                adventures::Plan::Invite {
                    peer: adventures::Member {
                        account: other,
                        name: "Peer".into(),
                    },
                    purpose: adventures::Purpose::Equipment,
                },
                &view,
                &app.content,
            )
            .unwrap();
        let mut reply = dialogue::Reply {
            intent: dialogue::Intent::Invite,
            reply: "Peer, want to look for better gear together?".into(),
        };
        brain.validate_dialogue(&request, &reply, &view).unwrap();
        if matches!(drift, "peer_left" | "absent_invalid_owner") {
            lifecycle::finalize(&app, peer, CloseReason::Controller)
                .unwrap()
                .wait()
                .await
                .unwrap();
            if let Some((x_axis, invalid)) = malformed_owner {
                app.world
                    .for_player(id)
                    .call(move |w| {
                        let position = &mut w.players.get_mut(&id).unwrap().pos;
                        if x_axis {
                            position.x = invalid;
                        } else {
                            position.y = invalid;
                        }
                        Ok(())
                    })
                    .await
                    .unwrap();
            }
        } else {
            app.world
                .directory(move |d| {
                    let now = d.clock.now();
                    if drift == "rate" {
                        for _ in 0..3 {
                            d.chat(id, "budget control".into(), now)?;
                        }
                    }
                    let w = d.worlds.get_mut(&-2).unwrap();
                    match drift {
                        "range" => {
                            let pos = w.players[&id].pos;
                            w.players.get_mut(&peer).unwrap().pos = Pos {
                                x: pos.x + 13.,
                                y: pos.y,
                            };
                        }
                        "known" => {
                            w.players.get_mut(&id).unwrap().known.remove(&peer);
                        }
                        "ignore" => {
                            w.sync_account_lists(
                                peer,
                                Some([Default::default(), [own].into_iter().collect()]),
                            )?;
                        }
                        "peer_dead" => w.players.get_mut(&peer).unwrap().character.hp = 0,
                        "peer_departing" => w.players.get_mut(&peer).unwrap().departing = true,
                        "text" | "rate" => {}
                        _ => unreachable!(),
                    }
                    Ok(())
                })
                .await
                .unwrap();
        }
        while output.try_recv().is_ok() {}
        while peer_output.try_recv().is_ok() {}
        let invalid = dialogue::Reply {
            intent: dialogue::Intent::Join,
            reply: "yes".into(),
        };
        let error = dialogue::publish(&app, &mut brain, &request, &invalid, &view)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "dialogue intention not requested");
        assert!(!error.is::<dialogue::Obsolete>());
        let authorized = request.plan.take();
        request.plan = Some(adventures::Plan::Answer(adventures::Offer {
            from: adventures::Member {
                account: other,
                name: "Peer".into(),
            },
            to: adventures::Member {
                account: 999_999,
                name: "Another".into(),
            },
            purpose: adventures::Purpose::Equipment,
            until: 60000,
            text: "want gear?".into(),
            answer_started: true,
        }));
        let error = dialogue::publish(&app, &mut brain, &request, &reply, &view)
            .await
            .unwrap_err();
        assert!(!error.is::<dialogue::Obsolete>(), "{drift}: {error:#}");
        request.plan = authorized;
        if drift == "text" {
            reply.reply = "Peer, want \0gear?".into();
        }
        let error = dialogue::publish(&app, &mut brain, &request, &reply, &view)
            .await
            .unwrap_err();
        if matches!(drift, "text" | "rate" | "absent_invalid_owner") {
            assert!(!error.is::<dialogue::Obsolete>(), "{drift}: {error:#}");
            assert!(
                error.to_string().contains(match drift {
                    "text" => "control characters",
                    "rate" => "Chat rate limit",
                    "absent_invalid_owner" => "dialogue controller position is not finite",
                    _ => unreachable!(),
                }),
                "{error:#}"
            );
        } else {
            let reason = match drift {
                "range" => "speaker_out_of_chat_range",
                "known" => "speaker_no_longer_known",
                "ignore" => "speaker_ignores_dialogue",
                "peer_left" => "speaker_left_world",
                "peer_dead" | "peer_departing" => "speaker_unavailable",
                _ => unreachable!(),
            };
            assert_eq!(
                error.downcast_ref::<dialogue::Obsolete>().unwrap().0,
                reason
            );
        }
        assert!(!std::iter::from_fn(|| output.try_recv().ok()).any(|p| p.id == wire::TEXT));
        assert!(!std::iter::from_fn(|| peer_output.try_recv().ok()).any(|p| p.id == wire::TEXT));
        assert!(brain.agreement.is_none());
        assert_eq!(brain.invitation_until, 0);
        assert!(app.society.lock().unwrap().outgoing(own, 0).is_none());
        assert!(app.society.lock().unwrap().group(own, 0).is_none());
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
