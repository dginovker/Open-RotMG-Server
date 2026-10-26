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
