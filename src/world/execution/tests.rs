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
    let directory = directory();
    directory.clock.set_paused(true);
    let handle = super::super::Handle::spawn(directory);
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
    let held_since = std::time::Instant::now();
    let ran = Arc::new(AtomicBool::new(false));
    let marked = ran.clone();
    let pending = handle.in_world(-1);
    let mut blocked = Box::pin(pending.call(move |_| {
        marked.store(true, Ordering::Release);
        Ok(())
    }));
    assert!(futures_util::poll!(blocked.as_mut()).is_pending());
    let slot = handle.routing.resolve(Target::World(-1)).unwrap();
    // A timer must not win the actor's select and take this request's ownership wait.
    tokio::time::timeout(Duration::from_secs(5), async {
        while slot.mailbox.capacity() != slot.mailbox.max_capacity() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let other = handle.in_world(-2);
    let baseline = other.call(|w| Ok(w.tick)).await.unwrap();
    handle.clock.set_paused(false);
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
    tokio::time::sleep(Duration::from_millis(60)).await;
    let guaranteed_hold = held_since.elapsed();
    release.send(()).unwrap();
    assert_eq!(transaction.await.unwrap().unwrap(), before);
    blocked.await.unwrap();
    other.in_world(-1).call(|_| Ok(())).await.unwrap();
    let slot = handle.routing.resolve(Target::World(-1)).unwrap();
    let attribution = slot.attribution.lock().unwrap();
    let blocked = attribution
        .slowest()
        .find(|s| s.kind == "job" && s.blocker.is_some())
        .expect("blocked job attribution missing");
    assert!(blocked.stages[2] >= Duration::from_millis(50));
    assert!(blocked.stages[2] > blocked.stages[4]);
    assert!(
        blocked
            .operation
            .contains("directory_loan_outlives_borrows")
    );
    let loan = attribution
        .slowest()
        .find(|s| s.kind == "directory_loan" && Some((s.operation, s.id)) == blocked.blocker)
        .expect("owner loan correlation missing");
    assert!(loan.stages[4] >= guaranteed_hold);
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
        world: Some(slot.try_work(&mut None).unwrap().unwrap()),
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
#[tokio::test]
async fn attribution_measures_backpressure_before_enqueue() {
    let handle = super::super::Handle::spawn(directory());
    handle.clock.set_paused(true);
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    sender
        .send(crate::world::Job::new(
            "occupied",
            Duration::ZERO,
            Box::new(|_: &mut Directory| Box::new(|| {})),
        ))
        .await
        .unwrap_or_else(|_| panic!("test queue closed"));
    let requester = handle.clone();
    let mut pending = tokio::spawn(async move {
        requester
            .request(
                &sender,
                "directory",
                "backpressure_test",
                |d| &mut d.performance,
                |_| Ok(()),
            )
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut pending)
            .await
            .is_err()
    );
    drop(receiver.recv().await.unwrap());
    let job = receiver.recv().await.unwrap();
    assert!(job.sample.stages[0] >= Duration::from_millis(20));
    handle
        .directory(move |d| {
            crate::world::execute_directory_job(d, job);
            Ok(())
        })
        .await
        .unwrap();
    pending.await.unwrap().unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attribution_separates_publication_from_execution() {
    let handle = super::super::Handle::spawn(directory());
    handle.clock.set_paused(true);
    handle.in_world(-1).call(|_| Ok(())).await.unwrap();
    let slot = handle.routing.resolve(Target::World(-1)).unwrap();
    let (entered, ready) = oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let actor = handle.in_world(-1);
    let mut job = tokio::spawn(async move {
        actor
            .call(move |_| {
                for n in 0..100_000u64 {
                    std::hint::black_box(n.wrapping_mul(n));
                }
                entered.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(())
            })
            .await
    });
    ready.await.unwrap();
    let (held, locked) = std::sync::mpsc::channel();
    let (publish, gate) = std::sync::mpsc::channel();
    let reader_slot = slot.clone();
    let reader = std::thread::spawn(move || {
        let _snapshot = reader_slot.snapshot.read().unwrap();
        held.send(()).unwrap();
        gate.recv_timeout(Duration::from_secs(5)).unwrap();
    });
    locked.recv_timeout(Duration::from_secs(5)).unwrap();
    release.send(()).unwrap();
    let queued_actor = handle.in_world(-1);
    let queued = tokio::spawn(async move { queued_actor.call(|_| Ok(())).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while slot.mailbox.capacity() == slot.mailbox.max_capacity() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut job)
            .await
            .is_err()
    );
    publish.send(()).unwrap();
    reader.join().unwrap();
    job.await.unwrap().unwrap();
    queued.await.unwrap().unwrap();
    handle.in_world(-1).call(|_| Ok(())).await.unwrap();
    let attribution = slot.attribution.lock().unwrap();
    let sample = attribution
        .slowest()
        .filter(|s| s.kind == "job")
        .max_by_key(|s| s.stages[5])
        .unwrap();
    assert!(sample.stages[5] >= Duration::from_millis(20));
    assert_eq!(sample.cpu.is_some(), cfg!(target_os = "linux"));
    assert_eq!(sample.publish_cpu.is_some(), cfg!(target_os = "linux"));
    if let Some(cpu) = sample.cpu {
        assert!(cpu > Duration::ZERO);
        assert!(sample.stages[5] > cpu);
    }
    if let Some(cpu) = sample.publish_cpu {
        assert!(sample.stages[5] > cpu);
    }
    let queued = attribution
        .slowest()
        .filter(|s| s.kind == "job")
        .max_by_key(|s| s.stages[1])
        .unwrap();
    assert!(queued.stages[1] >= Duration::from_millis(20));
    assert!(queued.stages[1] > queued.stages[2]);
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
        let world = slot.try_work(&mut None).unwrap().unwrap();
        let complete = pool
            .submit(
                slot.clone(),
                world,
                crate::world::Job::new(
                    "pool_test",
                    Duration::ZERO,
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
                ),
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
    std::thread::sleep(Duration::from_millis(20));
    gates[&first].send(()).unwrap();
    gates[&second].send(()).unwrap();
    let fourth = arrivals.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(fourth, slots[3].id);
    gates[&fourth].send(()).unwrap();
    for complete in completions {
        complete.blocking_recv().unwrap();
    }
    {
        let attribution = slots[3].attribution.lock().unwrap();
        let queued = attribution
            .slowest()
            .find(|s| s.operation == "pool_test")
            .unwrap();
        assert!(queued.stages[3] >= Duration::from_millis(20));
        assert_eq!(queued.stages[2], Duration::ZERO);
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
