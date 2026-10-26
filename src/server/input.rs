use crate::wire::{Codec, MAX_FRAME, Packet};
use axum::{body::Bytes, extract::ws::Message};
use futures_util::{Stream, StreamExt};
use std::{
    fmt::Display,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch},
    task::JoinHandle,
};

pub(super) enum Incoming {
    Command(Packet, Instant, OwnedSemaphorePermit),
    Ping(Bytes),
}
pub(super) struct InputTask {
    task: JoinHandle<()>,
    last_frame: Arc<Mutex<Option<Instant>>>,
}
impl InputTask {
    pub fn abort(&self) {
        self.task.abort();
    }
    pub fn last_frame_age_ms(&self) -> Option<f64> {
        self.last_frame
            .lock()
            .expect("input diagnostics poisoned")
            .map(|at| at.elapsed().as_secs_f64() * 1000.)
    }
}
impl Drop for InputTask {
    fn drop(&mut self) {
        self.abort();
    }
}
pub(super) fn receive<S, E>(
    mut stream: S,
    lost: watch::Sender<Option<&'static str>>,
    clock: crate::clock::Clock,
) -> (mpsc::Receiver<Incoming>, InputTask)
where
    S: Stream<Item = Result<Message, E>> + Send + Unpin + 'static,
    E: Display + Send + 'static,
{
    // Input keeps its socket receipt time while queued; MAX_FRAME bytes and 512 packets/s bound it, so a stalled world need not drop players.
    let (tx, rx) = mpsc::channel(512);
    let last_frame = Arc::new(Mutex::new(None));
    let received_frame = last_frame.clone();
    let task = tokio::spawn(async move {
        let mut codec = Codec::server();
        let bytes = Arc::new(Semaphore::new(MAX_FRAME));
        let mut window = Instant::now();
        let mut count = 0usize;
        let mut dispatched = 0;
        while let Some(message) = stream.next().await {
            *received_frame.lock().expect("input diagnostics poisoned") = Some(Instant::now());
            let received = clock.now();
            let inputs = match message {
                Ok(Message::Binary(frame)) => {
                    let packets = match codec.feed(&frame) {
                        Ok(packets) => packets,
                        Err(error) => {
                            tracing::error!(%error, "invalid packet stream");
                            crate::world::lose(&lost, "invalid_packet_stream");
                            return;
                        }
                    };
                    if window.elapsed().as_secs() >= 1 {
                        window = Instant::now();
                        count = 0;
                    }
                    count = count.saturating_add(packets.len());
                    if count > 512 {
                        tracing::warn!("input packet rate exceeded");
                        crate::world::lose(&lost, "input_rate_exceeded");
                        return;
                    }
                    let mut inputs = Vec::with_capacity(packets.len());
                    for packet in packets {
                        let permit = match bytes
                            .clone()
                            .try_acquire_many_owned((packet.body.len() + 5) as u32)
                        {
                            Ok(permit) => permit,
                            Err(error) => {
                                tracing::warn!(%error, "native input byte backlog exceeded");
                                crate::world::lose(&lost, "input_byte_backlog");
                                return;
                            }
                        };
                        inputs.push(Incoming::Command(packet, received, permit));
                    }
                    inputs
                }
                Ok(Message::Ping(bytes)) => vec![Incoming::Ping(bytes)],
                Ok(Message::Close(_)) => return,
                Ok(_) => {
                    tracing::warn!("non-binary game message rejected");
                    crate::world::lose(&lost, "non_binary_message");
                    return;
                }
                Err(error) => {
                    tracing::warn!(%error, "WebSocket failed");
                    crate::world::lose(&lost, "websocket_failed");
                    return;
                }
            };
            let batch = inputs.len();
            for (index, input) in inputs.into_iter().enumerate() {
                if let Err(error) = tx.try_send(input) {
                    tracing::warn!(%error, batch, index, "native input queue rejected before receipt timing diverges");
                    crate::world::lose(&lost, "input_queue_full");
                    return;
                }
                // Buffered separate frames must also let the consumer run before the bounded queue fills.
                dispatched = (dispatched + 1) % 16;
                if dispatched == 0 {
                    tokio::task::yield_now().await;
                }
            }
        }
    });
    (rx, InputTask { task, last_frame })
}
