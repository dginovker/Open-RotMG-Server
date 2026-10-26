use super::{Execution, Slot, World};
use crate::world::Job;
use anyhow::{Context, Result};
use std::sync::{Arc, Weak};
use tokio::sync::{oneshot, watch};

pub(super) fn start(slot: &Arc<Slot>, execution: Execution) -> Result<()> {
    let mut receiver = slot
        .receiver
        .lock()
        .expect("world mailbox poisoned")
        .take()
        .context("world actor started twice")?;
    let mut retired = slot.retired.subscribe();
    let weak = Arc::downgrade(slot);
    let runtime = execution.runtime.clone();
    execution.supervisor.clone().spawn_finite_on(&runtime,"world",async move {
        let mut timer=tokio::time::interval(std::time::Duration::from_millis(50));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                changed=retired.changed()=>{
                    if changed.is_err() || *retired.borrow() { return Ok(()); }
                }
                scheduled=timer.tick()=>{
                    if execution.clock.paused() { continue; }
                    let Some(slot)=weak.upgrade() else { return Ok(()) };
                    let id=slot.id;
                    drop(slot);
                    let (finished,stepped)=oneshot::channel();
                    let body=Job::new("tick",std::time::Duration::ZERO,Box::new(move|world:&mut World| {
                        let run=!world.clock.paused();
                        if run {
                            let _context=crate::supervision::world(id,world.tick+1);
                            world.performance.borrow_mut().record("scheduling_lateness",scheduled.elapsed());
                            let started=std::time::Instant::now();
                            world.step();
                            let elapsed_us=started.elapsed().as_micros() as u64;
                            if elapsed_us>25_000 { tracing::warn!(world=id,tick=world.tick,elapsed_us,"slow world simulation tick"); }
                            world.report_performance(id);
                        }
                        Box::new(move||{let _=finished.send(run);}) as crate::world::Reply
                    }));
                    if !run(&weak,&execution,&mut retired,body).await? { return Ok(()); }
                    if stepped.await.context("world tick completion missing")? {
                        let Some(slot)=weak.upgrade() else { return Ok(()) };
                        let needs=slot.latest().needs_coordinator();drop(slot);
                        if needs && !coordinate(&execution,&mut retired,id).await? { return Ok(()); }
                    }
                    for _ in 0..16 {
                        // Serial native readers need the reply before they can enqueue their next ACK.
                        tokio::task::yield_now().await;
                        let Ok(job)=receiver.try_recv() else { break };
                        if !run(&weak,&execution,&mut retired,job).await? { return Ok(()); }
                    }
                }
                job=receiver.recv()=>{
                    let Some(job)=job else { return Ok(()) };
                    if !run(&weak,&execution,&mut retired,job).await? { return Ok(()); }
                }
            }
        }
    });
    Ok(())
}
async fn run(
    weak: &Weak<Slot>,
    execution: &Execution,
    retired: &mut watch::Receiver<bool>,
    mut body: Job<World>,
) -> Result<bool> {
    body.sample.stages[1] = body.enqueued.elapsed();
    let ownership = std::time::Instant::now();
    execution.supervisor.ensure_running()?;
    let Some(slot) = weak.upgrade() else {
        return Ok(false);
    };
    let world = loop {
        let available = slot.available.notified();
        tokio::pin!(available);
        available.as_mut().enable();
        if *retired.borrow() {
            return Ok(false);
        }
        if let Some(world) = slot.try_work(&mut body.sample.blocker)? {
            break world;
        }
        tokio::select! {
            _=available=>{},
            changed=retired.changed()=>{if changed.is_err() || *retired.borrow() { return Ok(false); }},
        }
        execution.supervisor.ensure_running()?;
    };
    body.sample.stages[2] = ownership.elapsed();
    let finished = execution.pool.submit(slot.clone(), world, body)?;
    finished
        .await
        .context("world CPU worker did not complete")?;
    Ok(!slot.retired())
}
async fn coordinate(
    execution: &Execution,
    retired: &mut watch::Receiver<bool>,
    world: i32,
) -> Result<bool> {
    let Some(coordinator) = execution.coordinator.upgrade() else {
        return Ok(false);
    };
    let (finished, result) = oneshot::channel();
    let send_started = std::time::Instant::now();
    let permit = coordinator
        .reserve()
        .await
        .context("world coordinator stopped")?;
    permit.send(Job::new(
        "coordinate",
        send_started.elapsed(),
        Box::new(move |directory| {
            let started = crate::performance::Measure::start();
            directory.coordinate(&[world]);
            directory.performance.work("coordination", started);
            Box::new(move || {
                let _ = finished.send(());
            })
        }),
    ));
    tokio::select! {
        done=result=>{done.context("world coordination did not complete")?;Ok(true)},
        changed=retired.changed()=>{
            if changed.is_err() || *retired.borrow() { Ok(false) }
            else { anyhow::bail!("world retirement notification changed without retirement") }
        },
    }
}
