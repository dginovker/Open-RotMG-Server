mod content;
#[allow(dead_code)]
#[path = "../wire.rs"]
mod wire;
mod world;
use anyhow::{Context, Result, ensure};
use axum::{
    Form, Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use content::Content;
use rusqlite::{Connection, OptionalExtension};
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tower_http::services::ServeDir;
use wire::{Codec, Packet, Reader, Writer};
use world::{Character, Room};

struct App {
    content: Content,
    db: Mutex<Connection>,
    active: Mutex<HashSet<String>>,
    origin: String,
}
struct Lease {
    app: Arc<App>,
    guid: String,
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.app
            .active
            .lock()
            .expect("session mutex poisoned")
            .remove(&self.guid);
    }
}
impl App {
    fn character(&self, guid: &str) -> Result<Option<Character>> {
        let json: Option<String> = self
            .db
            .lock()
            .expect("database mutex poisoned")
            .query_row(
                "SELECT state FROM build100_characters WHERE guid=?",
                [guid],
                |r| r.get(0),
            )
            .optional()?;
        json.map(|s| Ok(serde_json::from_str(&s)?)).transpose()
    }
    fn save(&self, guid: &str, p: &Character) -> Result<()> {
        let state = serde_json::to_string(p)?;
        self.db.lock().expect("database mutex poisoned").execute("INSERT INTO build100_characters(guid,state) VALUES(?,?) ON CONFLICT(guid) DO UPDATE SET state=excluded.state",[guid,&state])?;
        Ok(())
    }
}
fn database(path: &std::path::Path) -> Result<Connection> {
    let db = Connection::open(path)?;
    db.pragma_update(None, "journal_mode", "WAL")?;
    db.pragma_update(None, "synchronous", "FULL")?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS build100_characters(guid TEXT PRIMARY KEY,state TEXT NOT NULL)",
    )?;
    Ok(db)
}
fn guid(value: &str) -> Result<()> {
    ensure!(
        value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit()),
        "expected build100 anonymous GUID"
    );
    Ok(())
}
async fn chars(State(a): State<Arc<App>>, Form(f): Form<HashMap<String, String>>) -> Response {
    let result = (|| -> Result<String> {
        let id = f.get("guid").context("missing GUID")?;
        guid(id)?;
        Ok(match a.character(id)?.filter(|p| p.alive) {
            Some(p) => format!(
                "<Chars><Char><CharId>0</CharId><Name>{}</Name><ObjectType>{}</ObjectType><Level>{}</Level></Char></Chars>",
                p.name, p.kind, p.stats[&7]
            ),
            None => "<Chars/>".into(),
        })
    })();
    match result {
        Ok(xml) => ([("Content-Type", "text/xml")], xml).into_response(),
        Err(e) => {
            tracing::warn!(error=%e,"build100 character list rejected");
            (StatusCode::BAD_REQUEST, e.to_string()).into_response()
        }
    }
}
async fn upgrade(State(a): State<Arc<App>>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    if headers.get("origin").and_then(|v| v.to_str().ok()) != Some(&a.origin) {
        return (StatusCode::FORBIDDEN, "unapproved build100 origin").into_response();
    }
    ws.on_upgrade(move |socket| async move {
        if let Err(e) = connection(socket, a).await {
            tracing::warn!(error=%format!("{e:#}"),"build100 session ended");
        }
    })
}
async fn send(socket: &mut WebSocket, codec: &mut Codec, packets: Vec<Packet>) -> Result<()> {
    for p in packets {
        socket
            .send(Message::Binary(codec.frame(p.id, p.body).into()))
            .await?;
    }
    Ok(())
}
async fn connection(mut socket: WebSocket, a: Arc<App>) -> Result<()> {
    let mut codec = Codec::server_with_keys(
        &[
            0x7a, 0x43, 0x56, 0x32, 0x74, 0x73, 0x59, 0x30, 0x5d, 0x73, 0x3b, 0x7c, 0x5d,
        ],
        &[
            0x68, 0x50, 0x76, 0x4a, 0x28, 0x52, 0x7d, 0x4d, 0x70, 0x24, 0x2d, 0x63, 0x67,
        ],
    );
    let mut lease: Option<Lease> = None;
    let mut room: Option<Room> = None;
    let mut timer = tokio::time::interval(Duration::from_millis(50));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            message=socket.recv()=>{
                let Some(message)=message else {break};
                let bytes=match message? {Message::Binary(b)=>b,Message::Close(_)=>break,Message::Ping(b)=>{socket.send(Message::Pong(b)).await?;continue},_=>anyhow::bail!("expected binary build100 stream")};
                for packet in codec.feed(&bytes)? {
                    let result=(||->Result<Vec<Packet>>{
                        if packet.id==17 {
                            ensure!(lease.is_none(),"duplicate HELLO");let mut r=Reader::new(&packet.body);
                            ensure!(r.i32()?==100,"client must be build100");let game=r.i32()?;ensure!(game==0||game== -1,"only arena entry is implemented");
                            let id=r.utf()?;r.done()?;guid(&id)?;
                            ensure!(a.active.lock().expect("session mutex poisoned").insert(id.clone()),"this character is already connected");
                            lease=Some(Lease{app:a.clone(),guid:id});
                            let mut w=Writer::new();w.i32(32).i32(32).utf("Build 100 Compatibility Arena").i32(0);
                            tracing::info!("build100 HELLO accepted");return Ok(vec![Packet{id:24,body:w.0}])
                        }
                        let id=&lease.as_ref().context("HELLO required")?.guid;
                        if packet.id==3||packet.id==25 {
                            ensure!(room.is_none(),"character already loaded");let mut r=Reader::new(&packet.body);let previous=a.character(id)?;
                            let p=if packet.id==3 {
                                ensure!(!previous.as_ref().is_some_and(|p|p.alive),"load the living character before creating another");
                                let kind=r.i16()?;let name=r.utf()?;r.done()?;
                                Character::new(&a.content,kind,name,previous.map_or(1,|p|p.generation+1))?
                            }else{
                                ensure!(r.i32()?==0,"unknown saved character");let _name=r.utf()?;r.done()?;
                                let mut p=previous.context("saved character missing")?;ensure!(p.alive,"character is permanently dead");
                                p.x=16.5;p.y=16.5;p
                            };
                            a.save(id,&p)?;tracing::info!(kind=p.kind,generation=p.generation,"build100 character entered");
                            let mut new=Room::new(p,rand::random());let out=new.enter(&a.content)?;room=Some(new);return Ok(out)
                        }
                        let room=room.as_mut().context("character selection required")?;
                        room.handle(packet,&a.content)
                    })();
                    match result {
                        Ok(out)=>{
                            if let (Some(room),Some(lease))=(&room,&lease) { a.save(&lease.guid,&room.character)?; }
                            send(&mut socket,&mut codec,out).await?;
                        },
                        Err(e)=>{
                            tracing::warn!(error=%e,"build100 request rejected");
                            if room.as_ref().is_some_and(|r|r.character.alive) {send(&mut socket,&mut codec,vec![Packet{id:20,body:vec![]},world::message(&e.to_string())]).await?;}
                            else {let mut w=Writer::new();w.i32(0).utf(&e.to_string());send(&mut socket,&mut codec,vec![Packet{id:0,body:w.0}]).await?;return Err(e)}
                        }
                    }
                }
            }
            _=timer.tick(),if room.as_ref().is_some_and(|r|r.character.alive)=>{
                let room=room.as_mut().expect("timer guarded by room");let before=serde_json::to_string(&room.character)?;
                let out=room.advance(0.05,&a.content)?;
                // Rewards, HP and permanent death commit before the corresponding client packets leave the server.
                if serde_json::to_string(&room.character)?!=before {a.save(&lease.as_ref().expect("authenticated room").guid,&room.character)?;}
                send(&mut socket,&mut codec,out).await?;
            }
        }
    }
    Ok(())
}
#[tokio::main]
async fn main() -> Result<()> {
    let filter = match std::env::var("RUST_LOG") {
        Ok(value) => tracing_subscriber::EnvFilter::try_new(value)?,
        Err(std::env::VarError::NotPresent) => tracing_subscriber::EnvFilter::new("info"),
        Err(error) => return Err(error.into()),
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        args.len() == 3,
        "usage: openrotmg-build100 LISTEN CLIENT_DIRECTORY STATE_DIRECTORY (experimental solo arena; loopback only)"
    );
    let listen: SocketAddr = args[0].parse()?;
    ensure!(
        listen.ip().is_loopback(),
        "build100 experimental anonymous protocol is restricted to loopback"
    );
    let client = PathBuf::from(&args[1]).canonicalize()?;
    let state = PathBuf::from(&args[2]);
    std::fs::create_dir_all(&state)?;
    for f in ["client.swf", "index.html", "vendor/ruffle.js"] {
        ensure!(
            client.join(f).is_file(),
            "missing prepared build100 asset {f}; run scripts/prepare-build100.py"
        );
    }
    let content = Content::load(&client)?;
    let db = database(&state.join("build100.sqlite"))?;
    let origin = format!("http://{listen}");
    let app = Arc::new(App {
        content,
        db: Mutex::new(db),
        active: Mutex::new(HashSet::new()),
        origin: origin.clone(),
    });
    let router = Router::new()
        .route("/game", get(upgrade))
        .route("/legacy/char/list", post(chars))
        .route(
            "/legacy/servers.txt",
            get(|| async { "Compatibility Arena,127.0.0.1\n" }),
        )
        .fallback_service(ServeDir::new(client))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(url=%origin,state=%state.display(),"build100 compatibility arena ready; reconstructed solo loop, not production or complete historical gameplay");
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_preserves_rewards_inventory_and_permanent_death() {
        let path = std::env::temp_dir().join(format!(
            "openrotmg-build100-{}.sqlite",
            rand::random::<u64>()
        ));
        let open = || App {
            content: Content {
                objects: Default::default(),
            },
            db: Mutex::new(database(&path).unwrap()),
            active: Mutex::new(HashSet::new()),
            origin: "http://127.0.0.1:8190".into(),
        };
        let id = "0123456789012345678901234567890123456789";
        let mut p = Character {
            generation: 1,
            kind: 0x30e,
            name: "Test".into(),
            x: 16.5,
            y: 16.5,
            stats: std::collections::BTreeMap::from([(1, 70), (6, 15), (12, 0xa22)]),
            alive: true,
        };
        {
            let app = open();
            app.save(id, &p).unwrap();
        }
        {
            let app = open();
            let got = app.character(id).unwrap().unwrap();
            assert_eq!(got.stats, p.stats);
            assert!(got.alive);
            p.alive = false;
            p.stats.insert(1, 0);
            app.save(id, &p).unwrap();
        }
        {
            let app = open();
            assert!(!app.character(id).unwrap().unwrap().alive);
            p.generation = 2;
            p.alive = true;
            p.stats.insert(1, 100);
            app.save(id, &p).unwrap();
        }
        {
            let app = open();
            assert_eq!(app.character(id).unwrap().unwrap().generation, 2);
        }
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn anonymous_id_validation_rejects_markup_and_password_shapes() {
        assert!(guid("0123456789012345678901234567890123456789").is_ok());
        for id in ["", "a@b.com", "<Chars>", "../state"] {
            assert!(guid(id).is_err());
        }
    }
}
