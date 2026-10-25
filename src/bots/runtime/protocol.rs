use super::*;
use std::{collections::VecDeque, future::Future};

pub(super) struct ProtocolOutput {
    packets: mpsc::Receiver<Packet>,
    deferred: VecDeque<Packet>,
}
impl ProtocolOutput {
    pub(super) fn new(packets: mpsc::Receiver<Packet>) -> Self {
        Self {
            packets,
            deferred: VecDeque::new(),
        }
    }
    pub(super) fn try_recv(&mut self) -> std::result::Result<Packet, mpsc::error::TryRecvError> {
        self.deferred
            .pop_front()
            .map(Ok)
            .unwrap_or_else(|| self.packets.try_recv())
    }
    // Transfers/finalization end the ACK context, so only session-stable work belongs here.
    // Publication may reject work nonterminally; transport failures must still stop the controller.
    pub(super) async fn waiting<T>(
        &mut self,
        world: &crate::world::Handle,
        id: i32,
        work: impl Future<Output = Result<T>>,
    ) -> Result<Result<T>> {
        tokio::pin!(work);
        loop {
            let received: Result<()> = tokio::select! {
                result = &mut work => return Ok(result),
                packet = self.packets.recv() => {
                    match packet {
                        Some(packet) if packet.id == wire::GOTO => {
                            world.for_player(id).call(move |w| w.acknowledge_autonomous_goto(id)).await
                        }
                        Some(packet) if self.deferred.len() < OUTPUT_CAPACITY => {
                            self.deferred.push_back(packet);
                            Ok(())
                        }
                        Some(_) => Err(anyhow::anyhow!("autonomous deferred output queue full")),
                        None => Err(anyhow::anyhow!("autonomous output channel closed")),
                    }
                }
            };
            if let Err(error) = received {
                tracing::error!(player=id,error=%format_args!("{error:#}"),"autonomous protocol failed while awaiting controller work");
                // Accepted inventory/Store work must settle even when transport has failed.
                if let Err(work_error) = work.await {
                    return Err(
                        error.context(format!("controller work also failed: {work_error:#}"))
                    );
                }
                return Err(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deferred_packets_keep_fifo_and_transport_failure_waits_for_work() {
        let (dir, app) = server::test_app();
        app.world.clock.set_paused(true);
        for count in [0, 3, OUTPUT_CAPACITY + 1] {
            let (tx, rx) = mpsc::channel(OUTPUT_CAPACITY + 2);
            for n in 0..count {
                tx.send(Packet {
                    id: wire::TEXT,
                    body: (n as u32).to_be_bytes().to_vec(),
                })
                .await
                .unwrap();
            }
            if count == 0 {
                drop(tx);
            }
            let mut output = ProtocolOutput::new(rx);
            let (release, wait) = tokio::sync::oneshot::channel();
            let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let settled = finished.clone();
            let work = async move {
                wait.await.unwrap();
                settled.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            };
            let (result, ()) = tokio::join!(output.waiting(&app.world, 0, work), async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                assert!(!finished.load(std::sync::atomic::Ordering::SeqCst));
                release.send(()).unwrap();
            });
            assert!(finished.load(std::sync::atomic::Ordering::SeqCst));
            if count == 0 {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("output channel closed")
                );
            } else if count > OUTPUT_CAPACITY {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("deferred output queue full")
                );
            } else {
                result.unwrap().unwrap();
            }
            for n in 0..count.min(OUTPUT_CAPACITY) {
                assert_eq!(output.try_recv().unwrap().body, (n as u32).to_be_bytes());
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
