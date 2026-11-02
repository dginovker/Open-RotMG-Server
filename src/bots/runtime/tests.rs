use super::*;
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
    actor_in(app, guid, name, backpack, -2).await
}
pub(in crate::bots) async fn actor_in(
    app: &App,
    guid: &str,
    name: &str,
    backpack: [i32; 8],
    world: i32,
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
    let position = if world == -3 {
        let (x, y) = app
            .world
            .in_world(world)
            .call(|w| Ok(w.map.spawns[0]))
            .await
            .unwrap();
        Some(Pos { x, y })
    } else {
        None
    };
    let (id, output, _, brain) = enter(app, profile(), ch, world, position).await.unwrap();
    (owner, id, output, brain)
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
            let (time, key, _) = d.issue(account, character, -3)?;
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
                    destination: -3,
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
    let expected = expected.unwrap();
    assert!(expected.lifetime.tiles >= saved.lifetime.tiles);
    assert_eq!(app.store.load(account, character).await.unwrap(), expected);
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
