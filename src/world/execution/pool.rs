use super::{Slot, World};
use anyhow::{Context, Result, bail, ensure};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use tokio::sync::oneshot;

pub(super) type Body = crate::world::Job<World>;

pub(crate) struct Pool {
    sender: mpsc::SyncSender<Task>,
    stopping: Arc<AtomicBool>,
}
impl Pool {
    pub(crate) fn new(
        supervisor: crate::supervision::Supervisor,
        workers: usize,
    ) -> Result<Arc<Self>> {
        ensure!(workers > 0, "simulation requires at least one CPU worker");
        let (sender, receiver) = mpsc::sync_channel::<Task>(256);
        let pool = Arc::new(Self {
            sender,
            stopping: Arc::new(AtomicBool::new(false)),
        });
        let receiver = Arc::new(Mutex::new(receiver));
        for _ in 0..workers {
            let (receiver, stopping, monitor) =
                (receiver.clone(), pool.stopping.clone(), supervisor.clone());
            supervisor
                .spawn_finite_thread("world_cpu", move || {
                    loop {
                        let next = receiver
                            .lock()
                            .expect("simulation dispatch lock poisoned")
                            .recv();
                        match next {
                            Ok(task) => {
                                monitor.ensure_running()?;
                                task.run(&monitor)?;
                            }
                            Err(_) if stopping.load(Ordering::Acquire) => return Ok(()),
                            Err(error) => {
                                bail!("simulation dispatch stopped unexpectedly: {error}")
                            }
                        }
                    }
                })
                .context("starting simulation CPU pool")?;
        }
        Ok(pool)
    }
    pub(super) fn submit(
        &self,
        slot: Arc<Slot>,
        world: Box<World>,
        body: Body,
    ) -> Result<oneshot::Receiver<()>> {
        let (finished, result) = oneshot::channel();
        let task = Task {
            slot,
            tick: world.tick,
            world: Some(world),
            body: Some(body),
            finished: Some(finished),
            complete: false,
        };
        self.sender.try_send(task).map_err(|error| {
            anyhow::anyhow!("bounded simulation dispatch refused work: {error}")
        })?;
        Ok(result)
    }
}
impl Drop for Pool {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
    }
}
struct Task {
    slot: Arc<Slot>,
    tick: u64,
    world: Option<Box<World>>,
    body: Option<Body>,
    finished: Option<oneshot::Sender<()>>,
    complete: bool,
}
impl Task {
    fn run(mut self, supervisor: &crate::supervision::Supervisor) -> Result<()> {
        let _context = crate::supervision::world(self.slot.id, self.tick);
        // Publish the original CPU-thread incident before waking any blocked Directory borrower.
        let outcome = supervisor.blocking_work("world", || -> Result<()> {
            if self.slot.retired() {
                self.slot
                    .restore(self.world.take().expect("retired task lost its world"))?;
                let _ = self
                    .finished
                    .take()
                    .expect("retired task completion missing")
                    .send(());
                return Ok(());
            }
            let reply = self.body.take().expect("simulation job consumed twice")(
                self.world
                    .as_mut()
                    .expect("simulation job has no owned world"),
            );
            self.slot
                .restore(self.world.take().expect("simulation world returned twice"))?;
            reply();
            let _ = self
                .finished
                .take()
                .expect("simulation completion consumed twice")
                .send(());
            Ok(())
        });
        match outcome {
            Ok(Ok(())) => {
                self.complete = true;
                Ok(())
            }
            Ok(Err(error)) => {
                let detail = format!("{error:#}");
                supervisor.failure("world", error);
                bail!("simulation task failed: {detail}")
            }
            Err(error) => Err(error),
        }
    }
}
impl Drop for Task {
    fn drop(&mut self) {
        if !self.complete {
            self.slot.poison();
        }
    }
}
