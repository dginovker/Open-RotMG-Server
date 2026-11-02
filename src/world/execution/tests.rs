use super::*;
use std::{
    sync::atomic::{AtomicBool, Ordering},
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

#[tokio::test]
async fn realm_retirement_rejects_queued_mailbox_and_cached_world_handle() {
    let mut d = directory();
    d.grow_realm_for_test();
    d.clock.set_paused(true);
    let handle = super::super::Handle::spawn(d);
    let cached = handle.in_world(1);
    let ran = Arc::new(AtomicBool::new(false));
    let marked = ran.clone();
    let (entered, ready) = oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let owner = handle.clone();
    let transaction = tokio::spawn(async move {
        owner
            .directory(move |d| {
                d.realm_allocation_step();
                entered.send(()).unwrap();
                wait.recv().unwrap();
                d.tick += 6000;
                d.realm_allocation_step();
                assert!(!d.worlds.contains_key(&1));
                Ok(())
            })
            .await
    });
    ready.await.unwrap();
    let mut queued = Box::pin(cached.call(move |_| {
        marked.store(true, Ordering::Release);
        Ok(())
    }));
    assert!(futures_util::poll!(queued.as_mut()).is_pending());
    release.send(()).unwrap();
    transaction.await.unwrap().unwrap();
    assert!(queued.await.is_err());
    assert!(!ran.load(Ordering::Acquire));
    assert!(cached.call(|w| Ok(w.tick)).await.is_err());
    handle.supervisor.ensure_running().unwrap();
}
