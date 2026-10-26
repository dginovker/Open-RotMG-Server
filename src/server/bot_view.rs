use super::*;
use crate::{
    bots::cctv::Viewer,
    world::spectator::{Attachment, Projection},
};
use axum::extract::{Path, ws::CloseFrame};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

pub(super) async fn socket(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(name): Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(origin) = &a.bot_view_origin else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "bot_view_origin must be configured for native bot viewing",
        )
            .into_response();
    };
    if headers.get("origin").and_then(|v| v.to_str().ok()) != Some(origin.as_str()) {
        return (
            StatusCode::FORBIDDEN,
            "native bot view origin is not approved",
        )
            .into_response();
    }
    let mut forwarded = headers.clone();
    // The native iframe has its own origin; preserve the existing player's origin policy on every other route.
    forwarded.remove("origin");
    let admitted = request_ip(&a, &forwarded, peer)
        .map_err(|e| (StatusCode::FORBIDDEN, e.to_string()))
        .and_then(|ip| Viewer::join(&a.cctv, &name, ip, true));
    let viewer = match admitted {
        Ok(v) => v,
        Err((status, error)) => {
            return (status, Json(serde_json::json!({"error":error}))).into_response();
        }
    };
    ws.max_message_size(65_536)
        .max_frame_size(65_536)
        .on_upgrade(move |socket| async move {
            let _viewer = viewer;
            let mut stream = Stream::new(socket, a.shutdown.subscribe());
            let result = stream.watch(&a, &name).await;
            if let Err(error) = result {
                tracing::warn!(bot=name, error=%error, "native bot view ended with error");
                let reason = error.to_string().chars().take(110).collect::<String>();
                let _ = tokio::time::timeout(
                    Duration::from_secs(1),
                    stream.socket.send(Message::Close(Some(CloseFrame {
                        code: 1011,
                        reason: reason.into(),
                    }))),
                )
                .await;
            }
            tracing::info!(
                bot = name,
                ignored_inputs = stream.ignored,
                "native bot viewer left"
            );
        })
}

struct Stream {
    socket: WebSocket,
    codec: Codec,
    pending: VecDeque<Packet>,
    shutdown: tokio::sync::watch::Receiver<bool>,
    input_window: Instant,
    inputs: usize,
    ignored: u64,
}
impl Stream {
    fn new(socket: WebSocket, shutdown: tokio::sync::watch::Receiver<bool>) -> Self {
        Self {
            socket,
            codec: Codec::server(),
            pending: VecDeque::new(),
            shutdown,
            input_window: Instant::now(),
            inputs: 0,
            ignored: 0,
        }
    }
    async fn output(&mut self, id: u8, body: Vec<u8>) -> Result<()> {
        let frame = self.codec.frame(id, body);
        tokio::select! {
            _ = async { let _ = self.shutdown.wait_for(|stop| *stop).await; } => bail!("server is stopping"),
            sent = tokio::time::timeout(Duration::from_secs(2), self.socket.send(Message::Binary(frame.into()))) => sent.context("native bot viewer is too slow")??,
        }
        Ok(())
    }
    async fn input(&mut self) -> Result<Option<Packet>> {
        loop {
            if let Some(packet) = self.pending.pop_front() {
                if self.input_window.elapsed() >= Duration::from_secs(1) {
                    self.input_window = Instant::now();
                    self.inputs = 0;
                }
                self.inputs += 1;
                ensure!(self.inputs <= 512, "native bot viewer input rate exceeded");
                return Ok(Some(packet));
            }
            match self.socket.recv().await {
                Some(Ok(Message::Binary(bytes))) => self.pending.extend(self.codec.feed(&bytes)?),
                Some(Ok(Message::Close(_))) | None => return Ok(None),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                Some(Ok(Message::Text(_))) => bail!("native bot viewer requires binary packets"),
                Some(Err(error)) => return Err(error.into()),
            }
        }
    }
    async fn attach(a: &App, name: &str) -> Result<Attachment> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some((player, account)) = a.cctv.binding(name)
                    && let Some(attached) = a.world.spectate(player, account).await?
                {
                    return Ok::<_, anyhow::Error>(attached);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .context("bot has no living session; it may be replacing a dead character or offline")?
    }
    async fn watch(&mut self, a: &App, name: &str) -> Result<()> {
        let hello = tokio::time::timeout(Duration::from_secs(10), self.input())
            .await??
            .context("viewer closed before HELLO")?;
        ensure!(
            hello.id == wire::HELLO && Reader::new(&hello.body).utf()? == "123.5.1",
            "native bot viewer requires a 123.5.1 HELLO"
        );
        let mut attached = tokio::select! {
            _ = async { let _ = self.shutdown.wait_for(|stop| *stop).await; } => bail!("server is stopping"),
            result = Self::attach(a,name) => result?,
        };
        self.output(wire::MAP_INFO, attached.map).await?;
        let entry = tokio::time::timeout(Duration::from_secs(10), self.input())
            .await??
            .context("viewer closed before LOAD")?;
        ensure!(
            matches!(entry.id, wire::LOAD | wire::CREATE),
            "native bot viewer requires LOAD after MAP_INFO"
        );
        let mut own = Writer::new();
        own.i32(attached.player).i32(attached.character);
        self.output(wire::CREATE_SUCCESS, own.0).await?;
        let mut projection = Projection::new(attached.player);
        for packet in attached.snapshot {
            let packet = projection
                .packet(packet, &a.content)?
                .context("snapshot packet was not visual")?;
            self.output(packet.id, packet.body).await?;
        }
        tracing::info!(
            bot = name,
            player = attached.player,
            character = attached.character,
            "native bot viewer attached"
        );
        let mut stop = self.shutdown.clone();
        loop {
            tokio::select! {
                _ = async { let _ = stop.wait_for(|stop| *stop).await; } => break,
                packet = self.input() => {
                    let Some(packet)=packet? else { break; };
                    // The recovered engine still reports collision/ACK packets; none belong to a gameplay session here.
                    if !matches!(packet.id, wire::MOVE | wire::UPDATE_ACK | wire::PONG | 14 | 24) {
                        self.ignored+=1;
                        if self.ignored==1 { tracing::info!(bot=name, packet=packet.id, "read-only bot viewer input ignored"); }
                    }
                },
                packet = attached.receiver.recv() => match packet {
                    Ok(packet) => if let Some(packet)=projection.packet(packet,&a.content)? { self.output(packet.id,packet.body).await?; },
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        let mut reconnect=Writer::new();reconnect.utf("Bot CCTV").utf("").i32(2050).i32(-2).i32(0).i16(0);
                        self.output(39,reconnect.0).await?;
                        break;
                    },
                    Err(error) => return Err(error).context("native bot view overflowed; reconnect for a fresh snapshot"),
                }
            }
        }
        Ok(())
    }
}
