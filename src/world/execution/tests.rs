use super::*;
use rand::RngCore;
use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};
use tokio::sync::oneshot;

fn directory() -> Directory {
    let content = Arc::new(crate::content::Content::load(std::path::Path::new("data")).unwrap());
    Directory::test_with_seed(std::path::Path::new("data"), content, 16, 12351).unwrap()
}
#[tokio::test]
async fn directory_loan_outlives_borrows_without_stalling_other_worlds() {
    let handle = super::super::Handle::spawn(directory());
    let (entered, ready) = oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let owner = handle.clone();
    let transaction = tokio::spawn(async move {
        owner
            .directory(move |d| {
                let before = d.worlds[&-1].tick;
                entered.send(before).unwrap();
                wait.recv().unwrap();
                assert_eq!(d.worlds[&-1].tick, before);
                Ok(before)
            })
            .await
    });
    let before = ready.await.unwrap();
    let ran = Arc::new(AtomicBool::new(false));
    let marked = ran.clone();
    let pending = handle.in_world(-1);
    let blocked = tokio::spawn(async move {
        pending
            .call(move |_| {
                marked.store(true, Ordering::Release);
                Ok(())
            })
            .await
    });
    let other = handle.in_world(-2);
    let baseline = other.call(|w| Ok(w.tick)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if other.call(|w| Ok(w.tick)).await.unwrap() >= baseline + 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(!ran.load(Ordering::Acquire));
    assert_eq!(
        handle
            .routing
            .resolve(Target::World(-1))
            .unwrap()
            .latest()
            .tick,
        before
    );
    release.send(()).unwrap();
    assert_eq!(transaction.await.unwrap().unwrap(), before);
    blocked.await.unwrap().unwrap();
    assert!(ran.load(Ordering::Acquire));
    handle.supervisor.ensure_running().unwrap();
}
#[tokio::test]
async fn due_realm_maintenance_releases_healthy_world_before_waiting_for_another() {
    struct Held {
        slot: Arc<Slot>,
        world: Option<Box<World>>,
    }
    impl Drop for Held {
        fn drop(&mut self) {
            if let Some(world) = self.world.take() {
                self.slot.restore(world).unwrap();
            }
        }
    }
    let handle = super::super::Handle::spawn(directory());
    handle.clock.set_paused(true);
    handle.in_world(1).call(|_| Ok(())).await.unwrap();
    handle
        .directory(|d| {
            d.tick = crate::world::endgame::REMAINING_TAUNT_MS / 50;
            Ok(())
        })
        .await
        .unwrap();
    let slot = handle.routing.resolve(Target::World(1)).unwrap();
    let held = Held {
        world: Some(slot.try_work().unwrap().unwrap()),
        slot: slot.clone(),
    };
    let healthy = handle.in_world(-3);
    let before = healthy.call(|w| Ok(w.tick)).await.unwrap();
    handle.clock.set_paused(false);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if slot.state.lock().unwrap().directory_waiting {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if healthy.call(|w| Ok(w.tick)).await.unwrap() >= before + 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    drop(held);
    handle.directory(|_| Ok(())).await.unwrap();
    handle.supervisor.ensure_running().unwrap();
}
#[tokio::test]
async fn cpu_panic_keeps_world_context_and_unblocks_directory() {
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(crate::supervision::install_panic_hook);
    let handle = super::super::Handle::spawn(directory());
    handle.clock.set_paused(true);
    let slot = handle.routing.resolve(Target::World(-1)).unwrap();
    let (entered, ready) = oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let actor = handle.in_world(-1);
    let panicking = tokio::spawn(async move {
        actor
            .call::<()>(move |_| {
                entered.send(()).unwrap();
                wait.recv().unwrap();
                let _npc = crate::supervision::npc(42, 0x615);
                panic!("injected CPU world failure")
            })
            .await
    });
    ready.await.unwrap();
    let directory = handle.clone();
    let borrower =
        tokio::spawn(async move { directory.directory(|d| Ok(d.worlds[&-1].tick)).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if slot.state.lock().unwrap().directory_waiting {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    release.send(()).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), panicking)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(5), borrower)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    let incident = handle.supervisor.failed().await;
    assert_eq!(incident.component, "world");
    assert_eq!(incident.failure, "panicked");
    let context = serde_json::to_value(incident.context).unwrap();
    assert_eq!(context["world"], -1);
    assert_eq!(context["npc"], serde_json::json!([42, 0x615]));
    assert!(incident.location.unwrap().contains("execution/tests.rs"));
}
#[test]
fn player_routing_waits_for_whole_directory_publication() {
    let directory = directory();
    let routing = directory.worlds.routing.clone();
    routing.players.write().unwrap().insert(99, -1);
    let publication = routing.publication.write().unwrap();
    let reader = routing.clone();
    let (entered, ready) = std::sync::mpsc::channel();
    let (reply, result) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        entered.send(()).unwrap();
        reply
            .send(reader.resolve(Target::Player(99)).unwrap().id)
            .unwrap();
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(result.recv_timeout(Duration::from_millis(20)).is_err());
    routing.players.write().unwrap().remove(&99);
    routing.players.write().unwrap().insert(99, -2);
    drop(publication);
    assert_eq!(result.recv_timeout(Duration::from_secs(5)).unwrap(), -2);
    thread.join().unwrap();
}
#[test]
fn coordination_only_consumes_effects_of_completed_worlds() {
    let mut directory = directory();
    directory
        .worlds
        .get_mut(&-1)
        .unwrap()
        .critical_deaths
        .push(42);
    directory.coordinate(&[]);
    assert_eq!(directory.worlds[&-1].critical_deaths, vec![42]);
    directory.coordinate(&[-2]);
    assert_eq!(directory.worlds[&-1].critical_deaths, vec![42]);
    directory.coordinate(&[-1]);
    assert!(directory.worlds[&-1].critical_deaths.is_empty());
    directory.tick = crate::world::endgame::REMAINING_TAUNT_MS / 50;
    directory
        .worlds
        .get_mut(&-3)
        .unwrap()
        .critical_deaths
        .push(-999);
    let fingerprint = |world: &World| {
        let mut rng = world.rng.clone();
        (0..8).map(|_| rng.next_u64()).collect::<Vec<_>>()
    };
    let before = fingerprint(&directory.worlds[&-3]);
    directory.coordinate(&[]);
    assert_eq!(directory.worlds[&-3].critical_deaths, vec![-999]);
    assert_eq!(fingerprint(&directory.worlds[&-3]), before);
    directory.coordinate(&[-3]);
    assert!(directory.worlds[&-3].critical_deaths.is_empty());
}
#[test]
fn pool_bounds_execution_and_cancels_retired_worlds() {
    let mut directory = directory();
    directory.worlds.release();
    let supervisor = crate::supervision::Supervisor::default();
    let pool = Pool::new(supervisor.clone(), 2).unwrap();
    let slots: Vec<_> = directory
        .worlds
        .entries
        .values()
        .take(4)
        .map(|e| e.slot.clone())
        .collect();
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let (started, arrivals) = std::sync::mpsc::channel();
    let (mut gates, mut completions, mut replies) = (BTreeMap::new(), Vec::new(), BTreeMap::new());
    for slot in &slots {
        let id = slot.id;
        let (release, wait) = std::sync::mpsc::channel();
        let (reply, response) = oneshot::channel();
        let (active, maximum, started) = (active.clone(), maximum.clone(), started.clone());
        let world = slot.try_work().unwrap().unwrap();
        let complete = pool
            .submit(
                slot.clone(),
                world,
                Box::new(move |_| {
                    let count = active.fetch_add(1, Ordering::AcqRel) + 1;
                    maximum.fetch_max(count, Ordering::AcqRel);
                    started.send(id).unwrap();
                    wait.recv().unwrap();
                    active.fetch_sub(1, Ordering::AcqRel);
                    Box::new(move || {
                        let _ = reply.send(id);
                    })
                }),
            )
            .unwrap();
        gates.insert(id, release);
        completions.push(complete);
        replies.insert(id, response);
    }
    let first = arrivals.recv_timeout(Duration::from_secs(5)).unwrap();
    let second = arrivals.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(active.load(Ordering::Acquire), 2);
    assert!(arrivals.try_recv().is_err());
    let retired = slots[2].id;
    slots[2].retire();
    gates[&first].send(()).unwrap();
    gates[&second].send(()).unwrap();
    let fourth = arrivals.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(fourth, slots[3].id);
    gates[&fourth].send(()).unwrap();
    for complete in completions {
        complete.blocking_recv().unwrap();
    }
    for (id, response) in replies {
        let result = response.blocking_recv();
        if id == retired {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap(), id);
        }
    }
    assert_eq!(active.load(Ordering::Acquire), 0);
    assert_eq!(maximum.load(Ordering::Acquire), 2);
    assert!(matches!(
        slots[2].state.lock().unwrap().owner,
        Owner::Retired
    ));
    supervisor.ensure_running().unwrap();
}
