use super::*;
use crate::world::DungeonReturn;
use std::time::Duration;

pub(super) struct NativeSocket {
    io: reqwest::Upgraded,
    codec: Codec,
    pending: std::collections::VecDeque<Packet>,
    forbid_reconnect: bool,
}

impl NativeSocket {
    pub(super) async fn connect(port: u16, origin: &str) -> Self {
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://127.0.0.1:{port}/game"))
            .header("origin", origin)
            .header("connection", "Upgrade")
            .header("upgrade", "websocket")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        Self {
            io: response.upgrade().await.unwrap(),
            codec: Codec::client(),
            pending: Default::default(),
            forbid_reconnect: false,
        }
    }
    pub(super) async fn send(&mut self, packet: Packet) {
        use tokio::io::AsyncWriteExt;
        let bytes = self.codec.frame(packet.id, packet.body);
        let mut frame = vec![0x82];
        if bytes.len() < 126 {
            frame.push(0x80 | bytes.len() as u8);
        } else {
            frame.push(0x80 | 126);
            frame.extend((bytes.len() as u16).to_be_bytes());
        }
        frame.extend([0; 4]);
        frame.extend(bytes);
        self.io.write_all(&frame).await.unwrap();
    }
    pub(super) async fn receive(&mut self, expected: u8) -> Packet {
        use tokio::io::AsyncReadExt;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                while let Some(packet) = self.pending.pop_front() {
                    if self.forbid_reconnect {
                        assert_ne!(packet.id, 39, "failed transfer published RECONNECT");
                    }
                    if packet.id == expected {
                        return packet;
                    }
                }
                let mut header = [0; 2];
                self.io.read_exact(&mut header).await.unwrap();
                assert_eq!(header[0] & 15, 2, "expected a binary WebSocket frame");
                assert_eq!(header[1] & 128, 0);
                let n = match header[1] & 127 {
                    126 => self.io.read_u16().await.unwrap() as usize,
                    127 => self.io.read_u64().await.unwrap() as usize,
                    n => n as usize,
                };
                assert!(n <= wire::MAX_FRAME);
                let mut body = vec![0; n];
                self.io.read_exact(&mut body).await.unwrap();
                self.pending.extend(self.codec.feed(&body).unwrap());
            }
        })
        .await
        .expect("native socket did not produce required packet")
    }
}

async fn avatar(a: &App, name: &str) -> (Owner, i32, crate::store::Character) {
    let account = a.store.list(name.into()).await.unwrap().id;
    let ch = a
        .store
        .create(account, a.content.get(WIZARD).unwrap())
        .await
        .unwrap();
    let saved = ch.clone();
    let owner = a.claim_session(account).unwrap();
    let bind = lifecycle::binding(a, account).unwrap();
    let id = a
        .world
        .call(move |w| {
            let id = w.enter(ch, None)?;
            bind(id)?;
            Ok(id)
        })
        .await
        .unwrap();
    (owner, id, saved)
}

pub(super) fn socket_hello(
    dir: &std::path::Path,
    guid: &str,
    game: i32,
    transfer: Option<(i32, Vec<u8>)>,
) -> Packet {
    socket_hello_password(dir, guid, "", game, transfer)
}

pub(super) fn socket_hello_password(
    dir: &std::path::Path,
    guid: &str,
    password: &str,
    game: i32,
    transfer: Option<(i32, Vec<u8>)>,
) -> Packet {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use rsa::{Pkcs1v15Encrypt, RsaPrivateKey, pkcs8::DecodePrivateKey};
    let key =
        RsaPrivateKey::from_pkcs8_pem(&std::fs::read_to_string(dir.join("server.pem")).unwrap())
            .unwrap();
    let guid = STANDARD.encode(
        key.to_public_key()
            .encrypt(&mut rand::rngs::OsRng, Pkcs1v15Encrypt, guid.as_bytes())
            .unwrap(),
    );
    let password = STANDARD.encode(
        key.to_public_key()
            .encrypt(&mut rand::rngs::OsRng, Pkcs1v15Encrypt, password.as_bytes())
            .unwrap(),
    );
    let (time, key) = transfer.unwrap_or((0, vec![]));
    let mut body = Writer::new();
    body.utf("123.5.1")
        .i32(game)
        .utf(&guid)
        .utf(&password)
        .utf("")
        .i32(time)
        .i16(key.len() as i16);
    body.0.extend(key);
    body.i32(0);
    for _ in 0..4 {
        body.utf("");
    }
    Packet {
        id: wire::HELLO,
        body: body.0,
    }
}

pub(super) async fn socket_load(
    socket: &mut NativeSocket,
    dir: &std::path::Path,
    guid: &str,
    ch: i32,
) -> i32 {
    socket.send(socket_hello(dir, guid, -2, None)).await;
    socket.receive(wire::MAP_INFO).await;
    socket
        .send(Packet {
            id: wire::LOAD,
            body: ch.to_be_bytes().to_vec(),
        })
        .await;
    let success = socket.receive(wire::CREATE_SUCCESS).await;
    Reader::new(&success.body).i32().unwrap()
}

pub(super) async fn socket_server(a: &App) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    println!(
        "owned native socket fixture pid={} port={port}",
        std::process::id()
    );
    let router = Router::new()
        .route("/game", get(websocket))
        .with_state(a.clone());
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (port, task)
}

async fn released(a: &App, account: i32) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while a.owners.lock().unwrap().contains_key(&account) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("native account admission never released");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lifecycle_finalize_real_socket_manual_and_resurrection_transfer_retire_before_reconnect() {
    let (dir, a) = account_actions::tests::fixture();
    a.world.clock.set_paused(true);
    let (port, server) = socket_server(&a).await;
    for forced in [false, true] {
        let guid = format!("finalize-transfer-{forced}");
        let account = a.store.list(guid.clone()).await.unwrap().id;
        let mut ch = a
            .store
            .create(account, a.content.get(WIZARD).unwrap())
            .await
            .unwrap();
        if forced {
            ch.items[3] = 0xb3e;
            ch.revision = a
                .store
                .exchange(
                    vec![InventoryWrite {
                        id: ch.id,
                        account,
                        revision: ch.revision,
                        items: ch.items.clone(),
                    }],
                    vec![],
                )
                .await
                .unwrap()[0];
            let mut mismatch = ch.clone();
            mismatch.alive = false;
            mismatch.hp = 0;
            mismatch.tombs += 1;
            assert!(
                a.store
                    .resurrect(mismatch, 3, a.content.clone())
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("resurrection reservation changed")
            );
            assert_eq!(a.store.load(account, ch.id).await.unwrap().tombs, ch.tombs);
        }
        let mut socket = NativeSocket::connect(port, &a.origin).await;
        let id = socket_load(&mut socket, &dir, &guid, ch.id).await;
        lifecycle::account(
            &a,
            vec![account],
            "socket_transfer_load_settled",
            |_| async { Ok(()) },
        )
        .await
        .unwrap();
        if forced {
            a.world
                .for_player(id)
                .call(move |w| {
                    let p = w.players.get_mut(&id).unwrap();
                    p.character.hp = 0;
                    p.death_pending = true;
                    Ok(())
                })
                .await
                .unwrap();
            persistence_tick(&a).await.unwrap();
        } else {
            let (ready, release) = a.store.lifecycle_test_pause_reply();
            let mut text = Writer::new();
            text.utf("/tutorial");
            socket
                .send(Packet {
                    id: wire::PLAYER_TEXT,
                    body: text.0,
                })
                .await;
            tokio::time::timeout(Duration::from_secs(5), ready)
                .await
                .unwrap()
                .unwrap();
            assert!(a.claim(account).is_err());
            assert!(
                a.world
                    .for_player(id)
                    .directory(move |d| Ok(d.worlds.values().any(|w| w.players.contains_key(&id))))
                    .await
                    .unwrap()
            );
            // Admission is still closed while the old save has committed but not published.
            assert_eq!(a.server_usage().unwrap(), 1. / a.max_players as f64);
            release.send(()).unwrap();
        }
        let packet = socket.receive(39).await;
        let mut r = Reader::new(&packet.body);
        r.utf().unwrap();
        r.utf().unwrap();
        r.i32().unwrap();
        let destination = r.i32().unwrap();
        let time = r.i32().unwrap();
        let n = r.i16().unwrap() as usize;
        let key = r.take(n).unwrap().to_vec();
        r.done().unwrap();
        assert_eq!(destination, if forced { -2 } else { -1 });
        assert!(
            !a.owners.lock().unwrap().contains_key(&account),
            "RECONNECT exposed before native admission release"
        );
        assert!(
            !a.world
                .directory(move |d| Ok(d.worlds.values().any(|w| w.players.contains_key(&id))))
                .await
                .unwrap()
        );
        let mut replacement = NativeSocket::connect(port, &a.origin).await;
        replacement
            .send(socket_hello(&dir, &guid, destination, Some((time, key))))
            .await;
        replacement.receive(wire::MAP_INFO).await;
        replacement
            .send(Packet {
                id: wire::LOAD,
                body: ch.id.to_be_bytes().to_vec(),
            })
            .await;
        let created = replacement.receive(wire::CREATE_SUCCESS).await;
        assert_ne!(Reader::new(&created.body).i32().unwrap(), id);
        assert!(a.claim(account).is_err());
        let saved = a.store.load(account, ch.id).await.unwrap();
        assert_eq!(saved.revision, ch.revision + if forced { 1 } else { 0 });
        assert!(saved.alive);
        if forced {
            assert_eq!(saved.items[3], -1);
        }
        drop(socket);
        drop(replacement);
        released(&a, account).await;
        assert_eq!(
            rusqlite::Connection::open(dir.join("game.db"))
                .unwrap()
                .query_row(
                    "SELECT count(*) FROM deaths WHERE character=?1",
                    [ch.id],
                    |r| r.get::<_, i32>(0)
                )
                .unwrap(),
            0
        );
    }
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_death_fame_phase4_pending_giver_finalization() {
    for dying in [false, true] {
        let (_dir, a) = account_actions::tests::fixture();
        a.world.clock.set_paused(true);
        let (_giver_owner, giver, g) = avatar(&a, "phase4-giver").await;
        let (_recipient_owner, recipient, mut ch) = avatar(&a, "phase4-recipient").await;
        // Prepared threshold/source isolates the real cross-account executor, not native-earned XP.
        ch.exp = 49;
        a.store.progress(ch.clone()).await.unwrap();
        a.world
            .call(move |w| {
                w.players.get_mut(&recipient).unwrap().character.exp = 49;
                w.players
                    .get_mut(&recipient)
                    .unwrap()
                    .queue_experience(1, Some(g.id), true)?;
                if dying {
                    let p = w.players.get_mut(&giver).unwrap();
                    p.character.hp = 0;
                    p.death_pending = true;
                }
                Ok(())
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), depart(&a, giver))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            a.store.assist_count(g.id).await.unwrap(),
            1,
            "giver finalized before causal recipient progress"
        );
        assert_eq!(a.store.load(ch.account, ch.id).await.unwrap().level, 2);
        if !dying {
            assert_eq!(
                a.store
                    .load(g.account, g.id)
                    .await
                    .unwrap()
                    .lifetime
                    .level_assists,
                1
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_death_fame_phase4_causal_replay_shutdown() {
    let (_dir, a) = account_actions::tests::fixture();
    a.world.clock.set_paused(true);
    let (_ao, aa, ac) = avatar(&a, "phase4-a").await;
    let (_bo, ba, bc) = avatar(&a, "phase4-b").await;
    let (_ro, recipient, mut rc) = avatar(&a, "phase4-coalesced").await;
    rc.exp = 49;
    a.store.progress(rc.clone()).await.unwrap();
    a.world
        .call(move |w| {
            let p = w.players.get_mut(&recipient).unwrap();
            p.character.exp = 49;
            p.queue_experience(1, Some(ac.id), true)?;
            p.queue_experience(149, None, false)?;
            p.queue_experience(1, Some(bc.id), true)?;
            Ok(())
        })
        .await
        .unwrap();
    let fa = lifecycle::finalize(&a, aa, CloseReason::Shutdown).unwrap();
    let fb = lifecycle::finalize(&a, ba, CloseReason::Shutdown).unwrap();
    let fr = lifecycle::finalize(&a, recipient, CloseReason::Shutdown).unwrap();
    let (ar, br, rr) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(fa.wait(), fb.wait(), fr.wait())
    })
    .await
    .unwrap();
    ar.unwrap();
    br.unwrap();
    rr.unwrap();
    assert_eq!(a.store.assist_count(ac.id).await.unwrap(), 1);
    assert_eq!(a.store.assist_count(bc.id).await.unwrap(), 1);
    let saved = a.store.load(rc.account, rc.id).await.unwrap();
    assert_eq!(saved.level, 3);
    a.store
        .progress_with_assists(
            saved,
            vec![
                crate::store::LevelAssist {
                    reached_level: 2,
                    giver: ac.id,
                },
                crate::store::LevelAssist {
                    reached_level: 3,
                    giver: bc.id,
                },
            ],
        )
        .await
        .unwrap();
    assert_eq!(a.store.assist_count(ac.id).await.unwrap(), 1);
    assert_eq!(a.store.assist_count(bc.id).await.unwrap(), 1);
    assert_eq!(
        a.store
            .load(ac.account, ac.id)
            .await
            .unwrap()
            .lifetime
            .level_assists,
        1
    );
    assert_eq!(
        a.store
            .load(bc.account, bc.id)
            .await
            .unwrap()
            .lifetime
            .level_assists,
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_death_fame_phase4_earned_realm_exit() {
    for arrive in [false, true] {
        let (_dir, a) = account_actions::tests::fixture();
        a.world.clock.set_paused(true);
        let guid = "phase4-native-snake-exit";
        let account = a.store.list(guid.into()).await.unwrap().id;
        let ch = a
            .store
            .create(account, a.content.get(WIZARD).unwrap())
            .await
            .unwrap();
        let character = ch.id;
        let (scope, time, key) = a
            .world
            .directory(move |d| {
                let scope = d.instance("snake-pit", None, Some(DungeonReturn::Realm(1)))?;
                let (time, key, _) = d.issue(account, character, scope)?;
                Ok((scope, time, key))
            })
            .await
            .unwrap();
        let (port, server) = socket_server(&a).await;
        let mut socket = NativeSocket::connect(port, &a.origin).await;
        socket
            .send(socket_hello(&_dir, guid, scope, Some((time, key))))
            .await;
        socket.receive(wire::MAP_INFO).await;
        socket
            .send(Packet {
                id: wire::LOAD,
                body: character.to_be_bytes().to_vec(),
            })
            .await;
        let packet = socket.receive(wire::CREATE_SUCCESS).await;
        let id = Reader::new(&packet.body).i32().unwrap();
        lifecycle::account(
            &a,
            vec![account],
            "phase4-native-entry-settled",
            |_| async { Ok(()) },
        )
        .await
        .unwrap();
        // Prepared already-cleared selected Snake Pit exit, not a claim of native boss progression.
        let portal = a
            .world
            .for_player(id)
            .call(move |w| {
                let pos = w.players[&id].pos;
                w.spawn_entity(0x703, pos, vec![])
            })
            .await
            .unwrap();
        assert!(
            a.store
                .load(account, character)
                .await
                .unwrap()
                .lifetime
                .realm_exits
                .is_empty()
        );
        a.world
            .directory(|d| {
                let mut eligible = 0;
                for w in d.worlds.values_mut().filter(|w| w.name == "Realm") {
                    w.max_players = w.occupancy() + 1;
                    eligible += 1;
                }
                assert!(
                    eligible >= 2,
                    "regression requires multiple eligible Realms"
                );
                Ok(())
            })
            .await
            .unwrap();
        socket
            .send(Packet {
                id: 3,
                body: portal.to_be_bytes().to_vec(),
            })
            .await;
        let packet = socket.receive(39).await;
        let mut r = Reader::new(&packet.body);
        r.utf().unwrap();
        r.utf().unwrap();
        r.i32().unwrap();
        let destination = r.i32().unwrap();
        assert_eq!(
            destination, 1,
            "RECONNECT must retain the dungeon's source Realm"
        );
        let keytime = r.i32().unwrap();
        let n = usize::try_from(r.i16().unwrap()).unwrap();
        let exit_key = r.take(n).unwrap().to_vec();
        r.done().unwrap();
        assert!(
            a.world
                .in_world(destination)
                .call(|w| Ok(w.name == "Realm"))
                .await
                .unwrap()
        );
        a.world
            .directory(move |d| {
                let full: Vec<_> = d
                    .worlds
                    .iter()
                    .filter(|(_, w)| w.name == "Realm" && w.occupancy() == w.max_players)
                    .map(|(id, _)| *id)
                    .collect();
                assert_eq!(
                    full,
                    vec![destination],
                    "selected grant must own the last free slot without rerouting"
                );
                Ok(())
            })
            .await
            .unwrap();
        let saved = a.store.load(account, character).await.unwrap();
        assert_eq!(saved.lifetime.realm_exits.len(), 1);
        assert!(
            saved
                .lifetime
                .realm_exits
                .contains(&crate::dungeon::HistoricalDungeon::SnakePit),
            "successful source departure lost earned exit"
        );
        assert!(!a.owners.lock().unwrap().contains_key(&account));
        drop(socket);
        if arrive {
            let mut arrival = NativeSocket::connect(port, &a.origin).await;
            arrival
                .send(socket_hello(
                    &_dir,
                    guid,
                    destination,
                    Some((keytime, exit_key.clone())),
                ))
                .await;
            arrival.receive(wire::MAP_INFO).await;
            arrival
                .send(Packet {
                    id: wire::LOAD,
                    body: character.to_be_bytes().to_vec(),
                })
                .await;
            let success = arrival.receive(wire::CREATE_SUCCESS).await;
            let replacement = Reader::new(&success.body).i32().unwrap();
            assert_ne!(replacement, id);
            lifecycle::account(&a, vec![account], "phase4_arrival_settled", |_| async {
                Ok(())
            })
            .await
            .unwrap();
            assert!(
                a.store
                    .load(account, character)
                    .await
                    .unwrap()
                    .lifetime
                    .realm_exits
                    .contains(&crate::dungeon::HistoricalDungeon::SnakePit)
            );
            drop(arrival);
            released(&a, account).await;
            let mut replay = NativeSocket::connect(port, &a.origin).await;
            replay.forbid_reconnect = true;
            replay
                .send(socket_hello(
                    &_dir,
                    guid,
                    destination,
                    Some((keytime, exit_key)),
                ))
                .await;
            replay.receive(0).await;
            drop(replay);
            released(&a, account).await;
            assert_eq!(
                a.store
                    .load(account, character)
                    .await
                    .unwrap()
                    .lifetime
                    .realm_exits
                    .len(),
                1
            );
            assert!(
                a.world
                    .directory(move |d| Ok(d
                        .worlds
                        .values()
                        .all(|w| w.players.values().all(|p| p.character.account != account))))
                    .await
                    .unwrap()
            );
        }
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
    }
}

async fn earned_source(
    a: &App,
    name: &str,
) -> (
    Owner,
    i32,
    crate::store::Character,
    i32,
    i32,
    Vec<u8>,
    crate::world::TransferReservation,
) {
    let account = a.store.list(name.into()).await.unwrap().id;
    let ch = a
        .store
        .create(account, a.content.get(WIZARD).unwrap())
        .await
        .unwrap();
    let copy = ch.clone();
    let owner = a.claim_session(account).unwrap();
    let bind = lifecycle::binding(a, account).unwrap();
    let (id, destination, time, key, reservation) = a
        .world
        .directory(move |d| {
            let source = d.instance("snake-pit", None, Some(DungeonReturn::Realm(1)))?;
            let w = d.worlds.get_mut(&source).unwrap();
            let id = w.enter(ch, None)?;
            bind(id)?;
            let portal = w.spawn_entity(0x703, w.players[&id].pos, vec![])?;
            let (account, character, destination) = d.destination(id, Some(portal))?;
            let (time, key, _) = d.issue(account, character, destination)?;
            d.bind_exit(id, Some(portal), &key)?;
            let reservation = d.reserve_transfer(key.clone())?;
            Ok((id, destination, time, key, reservation))
        })
        .await
        .unwrap();
    (owner, id, copy, destination, time, key, reservation)
}
async fn drained_credit_operation(a: &App, account: i32) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while lifecycle::state(a, account).is_some_and(|s| s.0) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
struct HeldReply(Option<std::sync::mpsc::Sender<()>>);
impl Drop for HeldReply {
    fn drop(&mut self) {
        if let Some(reply) = self.0.take() {
            let _ = reply.send(());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_death_fame_phase4_transfer_cancellation_cutoff() {
    let (_dir, a) = account_actions::tests::fixture();
    a.world.clock.set_paused(true);
    let (_owner, id, ch, destination, time, key, reservation) =
        earned_source(&a, "phase4-prehandoff").await;
    drop(reservation);
    assert!(
        a.world
            .directory(move |d| d.consume(ch.account, destination, time, key))
            .await
            .is_err()
    );
    depart(&a, id).await.unwrap();
    assert!(
        a.store
            .load(ch.account, ch.id)
            .await
            .unwrap()
            .lifetime
            .realm_exits
            .is_empty()
    );
    for accepted in [false, true] {
        let (dir, a) = account_actions::tests::fixture();
        a.world.clock.set_paused(true);
        let (_owner, id, ch, destination, time, key, reservation) = earned_source(
            &a,
            if accepted {
                "phase4-accepted-timeout"
            } else {
                "phase4-beforewrite-timeout"
            },
        )
        .await;
        let (ready, release) = if accepted {
            let (entered, ready) = tokio::sync::oneshot::channel();
            let (release, wait) = std::sync::mpsc::channel();
            a.store
                .lifecycle_test_job(move |db| {
                    let mut entered = Some(entered);
                    db.update_hook(Some(
                        move |action: rusqlite::hooks::Action, _: &str, table: &str, row: i64| {
                            if action == rusqlite::hooks::Action::SQLITE_UPDATE
                                && table == "characters"
                                && row == i64::from(ch.id)
                                && let Some(entered) = entered.take()
                            {
                                entered.send(()).unwrap();
                                wait.recv().unwrap();
                            }
                        },
                    ));
                    Ok(())
                })
                .await
                .unwrap();
            (ready, release)
        } else {
            // Canonical departure reads its origin before reaching source snapshot reservation.
            a.world.lifecycle_test_pause_job(9, true)
        };
        let release = HeldReply(Some(release));
        let finalization = lifecycle::finalize_transfer(&a, id, reservation).unwrap();
        tokio::time::timeout(Duration::from_secs(5), ready)
            .await
            .unwrap()
            .unwrap();
        let db = rusqlite::Connection::open(dir.join("game.db")).unwrap();
        let before: String = db
            .query_row("SELECT stats FROM characters WHERE id=?", [ch.id], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(
            serde_json::from_str::<crate::store::Character>(&before)
                .unwrap()
                .lifetime
                .realm_exits
                .is_empty()
        );
        let error = tokio::time::timeout(Duration::from_secs(17), finalization.wait())
            .await
            .expect("canonical deadline waited for held actor/store")
            .unwrap_err();
        assert!(format!("{error:#}").contains("deadline"));
        assert!(lifecycle::state(&a, ch.account).unwrap().2);
        drop(release);
        drained_credit_operation(&a, ch.account).await;
        assert!(
            a.world
                .directory(move |d| d.consume(ch.account, destination, time, key))
                .await
                .is_err(),
            "revoked late closure revived grant"
        );
        let raw: String = db
            .query_row("SELECT stats FROM characters WHERE id=?", [ch.id], |r| {
                r.get(0)
            })
            .unwrap();
        let saved: crate::store::Character = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            saved
                .lifetime
                .realm_exits
                .contains(&crate::dungeon::HistoricalDungeon::SnakePit),
            accepted,
            "accepted write durability must not be called undoable cancellation"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_death_fame_phase4_transfer_duplicate_waiter_and_arrival() {
    let (_dir, a) = account_actions::tests::fixture();
    a.world.clock.set_paused(true);
    let (_owner, id, ch, destination, time, key, reservation) =
        earned_source(&a, "phase4-duplicate-transfer").await;
    let first = lifecycle::finalize_transfer(&a, id, reservation).unwrap();
    let second = lifecycle::finalize(&a, id, CloseReason::Controller).unwrap();
    let (first, second) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(first.wait(), second.wait())
    })
    .await
    .unwrap();
    first.unwrap();
    second.unwrap();
    assert!(
        a.store
            .load(ch.account, ch.id)
            .await
            .unwrap()
            .lifetime
            .realm_exits
            .contains(&crate::dungeon::HistoricalDungeon::SnakePit)
    );
    a.world
        .directory(move |d| {
            let (character, arrival) = d.consume(ch.account, destination, time, key.clone())?;
            assert_eq!(character, ch.id);
            assert!(d.consume(ch.account, destination, time, key).is_err());
            d.admit_transfer(arrival, ch.account, ch.id, destination)
        })
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_death_fame_phase4_reciprocal_credits_and_rollback() {
    for fail in [false, true] {
        let (dir, a) = account_actions::tests::fixture();
        a.world.clock.set_paused(true);
        let (_ao, aa, mut ac) = avatar(&a, "phase4-reciprocal-a").await;
        let (_bo, ba, mut bc) = avatar(&a, "phase4-reciprocal-b").await;
        ac.exp = 49;
        bc.exp = 49;
        a.store.progress(ac.clone()).await.unwrap();
        a.store.progress(bc.clone()).await.unwrap();
        a.world
            .call(move |w| {
                let p = w.players.get_mut(&aa).unwrap();
                p.character.exp = 49;
                p.queue_experience(1, Some(bc.id), true)?;
                let p = w.players.get_mut(&ba).unwrap();
                p.character.exp = 49;
                p.queue_experience(1, Some(ac.id), true)?;
                Ok(())
            })
            .await
            .unwrap();
        let db = rusqlite::Connection::open(dir.join("game.db")).unwrap();
        if fail {
            db.execute_batch("CREATE TRIGGER fail_causal_credit BEFORE INSERT ON level_credits BEGIN SELECT RAISE(ABORT,'injected causal credit rollback'); END;").unwrap();
        }
        let (entered, release) = a.store.lifecycle_test_pause_reply();
        let release = HeldReply(Some(release));
        let fa = lifecycle::finalize(&a, aa, CloseReason::Shutdown).unwrap();
        let fb = lifecycle::finalize(&a, ba, CloseReason::Shutdown).unwrap();
        tokio::time::timeout(Duration::from_secs(5), entered)
            .await
            .unwrap()
            .unwrap();
        assert!(lifecycle::state(&a, ac.account).unwrap().1.is_some());
        assert!(lifecycle::state(&a, bc.account).unwrap().1.is_some());
        let prepared = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let prepared = a
                    .world
                    .call(move |w| {
                        let mut prepared = vec![];
                        for id in [aa, ba] {
                            let (snapshot, departed) = w.credit_reservation_for_test(id);
                            assert!(!departed);
                            if let Some(snapshot) = snapshot {
                                let assists = w.progress_assists(id, &snapshot)?;
                                prepared.push((snapshot, assists));
                            }
                        }
                        Ok(prepared)
                    })
                    .await
                    .unwrap();
                if prepared.len() == 2 {
                    break prepared;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        drop(release);
        let (ar, br) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(fa.wait(), fb.wait())
        })
        .await
        .unwrap();
        if fail {
            assert!(ar.is_err() && br.is_err());
            assert_eq!(
                db.query_row("SELECT count(*) FROM level_credits", [], |r| r
                    .get::<_, i32>(0))
                    .unwrap(),
                0
            );
            assert_eq!(a.store.load(ac.account, ac.id).await.unwrap().exp, 49);
            assert_eq!(a.store.load(bc.account, bc.id).await.unwrap().exp, 49);
            assert!(
                lifecycle::state(&a, ac.account).unwrap().2
                    && lifecycle::state(&a, bc.account).unwrap().2
            );
            db.execute_batch("DROP TRIGGER fail_causal_credit").unwrap();
            let reopened = Store::open(&dir.join("game.db"), &a.content).unwrap();
            for (snapshot, assists) in prepared {
                reopened
                    .progress_with_assists(snapshot.clone(), assists.clone())
                    .await
                    .unwrap();
                reopened
                    .progress_with_assists(snapshot, assists)
                    .await
                    .unwrap();
            }
            assert_eq!(reopened.assist_count(ac.id).await.unwrap(), 1);
            assert_eq!(reopened.assist_count(bc.id).await.unwrap(), 1);
        } else {
            ar.unwrap();
            br.unwrap();
            assert_eq!(
                a.store
                    .load(ac.account, ac.id)
                    .await
                    .unwrap()
                    .lifetime
                    .level_assists,
                1
            );
            assert_eq!(
                a.store
                    .load(bc.account, bc.id)
                    .await
                    .unwrap()
                    .lifetime
                    .level_assists,
                1
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_death_fame_phase4_native_shutdown_held_credit() {
    let (dir, a) = account_actions::tests::fixture();
    a.world.clock.set_paused(true);
    let (port, server) = socket_server(&a).await;
    let mut sockets = vec![];
    let mut actors = vec![];
    for (index, guid) in [
        "phase4-shutdown-a",
        "phase4-shutdown-b",
        "phase4-shutdown-recipient",
    ]
    .into_iter()
    .enumerate()
    {
        let account = a.store.list(guid.into()).await.unwrap().id;
        let mut ch = a
            .store
            .create(account, a.content.get(WIZARD).unwrap())
            .await
            .unwrap();
        if index == 2 {
            ch.exp = 49;
            a.store.progress(ch.clone()).await.unwrap();
        }
        let mut socket = NativeSocket::connect(port, &a.origin).await;
        let id = socket_load(&mut socket, &dir, guid, ch.id).await;
        lifecycle::account(&a, vec![account], "phase4_shutdown_entry", |_| async {
            Ok(())
        })
        .await
        .unwrap();
        sockets.push(socket);
        actors.push((id, ch));
    }
    persistence_tick(&a).await.unwrap();
    let recipient = actors[2].0;
    let ac = actors[0].1.id;
    let bc = actors[1].1.id;
    a.world
        .for_player(recipient)
        .call(move |w| {
            let p = w.players.get_mut(&recipient).unwrap();
            p.queue_experience(1, Some(ac), true)?;
            p.queue_experience(149, None, false)?;
            p.queue_experience(1, Some(bc), true)?;
            Ok(())
        })
        .await
        .unwrap();
    let (entered, release) = a.store.lifecycle_test_pause_reply();
    let release = HeldReply(Some(release));
    let app = a.clone();
    let collector = tokio::spawn(async move { persistence_tick(&app).await });
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .unwrap()
        .unwrap();
    lifecycle::drain::prepare(&a);
    a.shutdown.send_replace(true);
    let app = a.clone();
    let shutdown = tokio::spawn(async move { lifecycle::drain::terminate(&app).await });
    a.world
        .for_player(recipient)
        .call(move |w| {
            let p = &w.players[&recipient];
            assert!(p.progress_started && p.pending);
            assert_eq!(p.level_assists_prepared.len(), 2);
            Ok(())
        })
        .await
        .unwrap();
    drop(release);
    tokio::time::timeout(Duration::from_secs(5), collector)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), shutdown)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    for (_, ch) in actors.iter().take(2) {
        assert_eq!(a.store.assist_count(ch.id).await.unwrap(), 1);
        assert_eq!(
            a.store
                .load(ch.account, ch.id)
                .await
                .unwrap()
                .lifetime
                .level_assists,
            1
        );
    }
    assert_eq!(
        a.store
            .load(actors[2].1.account, actors[2].1.id)
            .await
            .unwrap()
            .level,
        3
    );
    assert!(a.owners.lock().unwrap().is_empty());
    assert_eq!(a.store.lifecycle_test_jobs(), 0);
    drop(sockets);
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_death_fame_phase4_coalesced_partial_failure_retry() {
    let (dir, a) = account_actions::tests::fixture();
    a.world.clock.set_paused(true);
    let (_ao, aa, ac) = avatar(&a, "phase4-partial-a").await;
    let (_bo, ba, bc) = avatar(&a, "phase4-partial-b").await;
    let (_ro, recipient, mut rc) = avatar(&a, "phase4-partial-recipient").await;
    rc.exp = 49;
    a.store.progress(rc.clone()).await.unwrap();
    a.world
        .call(move |w| {
            let p = w.players.get_mut(&recipient).unwrap();
            p.character.exp = 49;
            p.queue_experience(1, Some(ac.id), true)?;
            p.queue_experience(149, None, false)?;
            p.queue_experience(1, Some(bc.id), true)?;
            Ok(())
        })
        .await
        .unwrap();
    let db = rusqlite::Connection::open(dir.join("game.db")).unwrap();
    db.execute_batch("CREATE TRIGGER reject_second_level BEFORE INSERT ON level_credits WHEN NEW.reached_level=3 BEGIN SELECT RAISE(ABORT,'injected second coalesced credit failure'); END;").unwrap();
    let fa = lifecycle::finalize(&a, aa, CloseReason::Shutdown).unwrap();
    let fb = lifecycle::finalize(&a, ba, CloseReason::Shutdown).unwrap();
    let fr = lifecycle::finalize(&a, recipient, CloseReason::Shutdown).unwrap();
    let (ar, br, rr) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(fa.wait(), fb.wait(), fr.wait())
    })
    .await
    .unwrap();
    assert!(ar.is_err() && br.is_err() && rr.is_err());
    assert_eq!(
        db.query_row("SELECT count(*) FROM level_credits", [], |r| r
            .get::<_, i32>(0))
            .unwrap(),
        0
    );
    assert_eq!(a.store.load(rc.account, rc.id).await.unwrap().exp, 49);
    let (snapshot, assists) = a
        .world
        .call(move |w| {
            let (snapshot, departed) = w.credit_reservation_for_test(recipient);
            assert!(!departed);
            let snapshot = snapshot.context("failed causal reservation must remain retryable")?;
            let assists = w.progress_assists(recipient, &snapshot)?;
            assert_eq!(assists.len(), 2);
            Ok((snapshot, assists))
        })
        .await
        .unwrap();
    for account in [ac.account, bc.account, rc.account] {
        assert!(lifecycle::state(&a, account).unwrap().2);
    }
    db.execute_batch("DROP TRIGGER reject_second_level")
        .unwrap();
    let reopened = Store::open(&dir.join("game.db"), &a.content).unwrap();
    for _ in 0..2 {
        reopened
            .progress_with_assists(snapshot.clone(), assists.clone())
            .await
            .unwrap();
    }
    assert_eq!(reopened.assist_count(ac.id).await.unwrap(), 1);
    assert_eq!(reopened.assist_count(bc.id).await.unwrap(), 1);
    assert_eq!(reopened.load(rc.account, rc.id).await.unwrap().level, 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_death_fame_phase4_capacity_acceptance_boundaries() {
    for boundary in 0..3 {
        let (dir, a) = account_actions::tests::fixture();
        a.world.clock.set_paused(true);
        let (_owner, id, ch, destination, time, key, reservation) =
            earned_source(&a, "phase4-capacity-boundary").await;
        let gate = if boundary == 1 {
            Some(a.world.lifecycle_test_pause_job(9, false))
        } else if boundary == 2 {
            let (entered, ready) = tokio::sync::oneshot::channel();
            let (release, wait) = std::sync::mpsc::channel();
            a.store
                .lifecycle_test_job(move |db| {
                    let mut entered = Some(entered);
                    db.update_hook(Some(
                        move |action: rusqlite::hooks::Action, _: &str, table: &str, row: i64| {
                            if action == rusqlite::hooks::Action::SQLITE_UPDATE
                                && table == "characters"
                                && row == i64::from(ch.id)
                                && let Some(entered) = entered.take()
                            {
                                entered.send(()).unwrap();
                                wait.recv().unwrap();
                            }
                        },
                    ));
                    Ok(())
                })
                .await
                .unwrap();
            Some((ready, release))
        } else {
            None
        };
        if boundary == 0 {
            a.world
                .in_world(destination)
                .call(|w| {
                    w.max_players = w.players.len();
                    Ok(())
                })
                .await
                .unwrap();
        }
        let finalization = lifecycle::finalize_transfer(&a, id, reservation).unwrap();
        if let Some((entered, release)) = gate {
            let release = HeldReply(Some(release));
            tokio::time::timeout(Duration::from_secs(5), entered)
                .await
                .unwrap()
                .unwrap();
            // The SQL gate leaves directory jobs runnable; completion can beat the first poll.
            let mut fill = Box::pin(a.world.directory(move |d| {
                let source = d.location(id)?;
                assert!(
                    d.worlds[&source].credit_reservation_for_test(id).1,
                    "capacity loss must follow actual snapshot preparation"
                );
                assert!(
                    d.worlds[&source].players[&id]
                        .character
                        .lifetime
                        .realm_exits
                        .is_empty()
                );
                let w = d.worlds.get_mut(&destination).unwrap();
                w.max_players = w.players.len();
                Ok(())
            }));
            let first = futures_util::poll!(fill.as_mut());
            if boundary == 1 {
                assert!(
                    first.is_pending(),
                    "held Directory gate did not block its reply"
                );
            }
            drop(release);
            match first {
                std::task::Poll::Pending => fill.await.unwrap(),
                std::task::Poll::Ready(result) => result.unwrap(),
            }
        }
        let error = tokio::time::timeout(Duration::from_secs(5), finalization.wait())
            .await
            .unwrap()
            .unwrap_err();
        assert!(format!("{error:#}").contains("full"));
        assert!(lifecycle::state(&a, ch.account).unwrap().2);
        let db = rusqlite::Connection::open(dir.join("game.db")).unwrap();
        let raw: String = db
            .query_row("SELECT stats FROM characters WHERE id=?", [ch.id], |r| {
                r.get(0)
            })
            .unwrap();
        let saved: crate::store::Character = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            saved
                .lifetime
                .realm_exits
                .contains(&crate::dungeon::HistoricalDungeon::SnakePit),
            boundary == 2
        );
        a.world
            .for_player(id)
            .call(move |w| {
                assert!(
                    w.players[&id].character.lifetime.realm_exits.is_empty(),
                    "draft exit published into live character before departure"
                );
                Ok(())
            })
            .await
            .unwrap();
        assert!(
            a.world
                .directory(move |d| d.consume(ch.account, destination, time, key))
                .await
                .is_err()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_death_fame_phase4_native_rejected_capacity_and_save() {
    for failure in 0..3 {
        let (dir, a) = account_actions::tests::fixture();
        a.world.clock.set_paused(true);
        let guid = "phase4-native-rejected";
        let account = a.store.list(guid.into()).await.unwrap().id;
        let ch = a
            .store
            .create(account, a.content.get(WIZARD).unwrap())
            .await
            .unwrap();
        let character = ch.id;
        let (scope, time, key) = a
            .world
            .directory(move |d| {
                let scope = if failure == 0 {
                    -2
                } else {
                    d.instance("snake-pit", None, Some(DungeonReturn::Realm(1)))?
                };
                let (time, key, _) = d.issue(account, character, scope)?;
                Ok((scope, time, key))
            })
            .await
            .unwrap();
        let (port, server) = socket_server(&a).await;
        let mut socket = NativeSocket::connect(port, &a.origin).await;
        socket
            .send(socket_hello(&dir, guid, scope, Some((time, key))))
            .await;
        socket.receive(wire::MAP_INFO).await;
        socket
            .send(Packet {
                id: wire::LOAD,
                body: character.to_be_bytes().to_vec(),
            })
            .await;
        let packet = socket.receive(wire::CREATE_SUCCESS).await;
        let id = Reader::new(&packet.body).i32().unwrap();
        lifecycle::account(
            &a,
            vec![account],
            "phase4_rejected_entry_settled",
            |_| async { Ok(()) },
        )
        .await
        .unwrap();
        let (portal, bound_destination) = a
            .world
            .directory(move |d| {
                let w = d.worlds.get_mut(&scope).unwrap();
                if failure == 0 {
                    assert_eq!(w.name, "Nexus");
                    let portal = w
                        .entities
                        .values()
                        .find(|e| e.kind == 0x712)
                        .context("published named Realm portal missing")?
                        .clone();
                    w.players.get_mut(&id).unwrap().pos = portal.pos;
                    // Routing validates publication, range and the named Realm binding.
                    let binding = d.destination(id, Some(portal.id))?;
                    assert_eq!((binding.0, binding.1), (account, character));
                    Ok((portal.id, Some(binding.2)))
                } else {
                    Ok((w.spawn_entity(0x704, w.players[&id].pos, vec![])?, None))
                }
            })
            .await
            .unwrap();
        let db = rusqlite::Connection::open(dir.join("game.db")).unwrap();
        if failure == 1 {
            db.execute_batch("CREATE TRIGGER reject_exit_save BEFORE UPDATE ON characters BEGIN SELECT RAISE(ABORT,'injected exit save rollback'); END;").unwrap();
        } else if failure == 0 {
            a.world
                .directory(move |d| {
                    let w = d.worlds.get_mut(&bound_destination.unwrap()).unwrap();
                    w.max_players = w.players.len();
                    assert_eq!(w.occupancy(), w.max_players);
                    Ok(())
                })
                .await
                .unwrap();
        }
        let saturated = if failure == 2 {
            let mut identities = vec![(account, character)];
            for n in 0..63 {
                let other = a
                    .store
                    .list(format!("phase4-reserved-{n}"))
                    .await
                    .unwrap()
                    .id;
                let ch = a
                    .store
                    .create(other, a.content.get(WIZARD).unwrap())
                    .await
                    .unwrap();
                identities.push((other, ch.id));
            }
            Some(
                a.world
                    .directory(move |d| {
                        assert!(d.forced_transfer_candidates().is_empty());
                        // Leave the second Realm available so refusal reaches the global grant limit.
                        let destinations = [-2, -1, -4, -3];
                        let mut identities = identities.into_iter();
                        let mut reservations = Vec::new();
                        for destination in destinations {
                            assert_eq!(d.worlds[&destination].occupancy(), 0);
                            for _ in 0..d.worlds[&destination].max_players {
                                let (account, character) = identities.next().context(
                                    "saturation fixture needs one distinct identity per slot",
                                )?;
                                let (_, key, _) = d.issue(account, character, destination)?;
                                reservations.push((account, key));
                            }
                        }
                        assert!(identities.next().is_none());
                        assert_eq!(reservations.len(), 64);
                        assert_eq!(
                            d.worlds.values().map(|w| w.reserved_slots()).sum::<usize>(),
                            64
                        );
                        let (_, _, routed) = d.destination(id, Some(portal))?;
                        assert_eq!(d.worlds[&routed].name, "Realm");
                        assert!(d.worlds[&routed].occupancy() < d.worlds[&routed].max_players);
                        let error = d.issue(account, character, -2).unwrap_err();
                        assert!(
                            error.to_string().contains("transfer grant limit reached"),
                            "{error:#}"
                        );
                        Ok(reservations)
                    })
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        socket.forbid_reconnect = true;
        socket
            .send(Packet {
                id: 3,
                body: portal.to_be_bytes().to_vec(),
            })
            .await;
        if failure == 0 {
            let packet = socket.receive(wire::TEXT).await;
            let mut r = Reader::new(&packet.body);
            r.utf().unwrap();
            r.i32().unwrap();
            r.i32().unwrap();
            r.u8().unwrap();
            r.utf().unwrap();
            assert_eq!(r.utf().unwrap(), "Destination is full.");
            assert_eq!(
                a.world
                    .for_player(id)
                    .call(|w| Ok(w.name.clone()))
                    .await
                    .unwrap(),
                "Nexus",
                "full named-Realm refusal must retain the original Nexus source"
            );
            let state = lifecycle::state(&a, account).unwrap();
            assert!(
                !state.0 && !state.2 && state.1 == Some(id),
                "ordinary full-world refusal must preserve healthy source controller"
            );
        } else {
            socket.receive(0).await;
        }
        let raw: String = db
            .query_row(
                "SELECT stats FROM characters WHERE id=?",
                [character],
                |r| r.get(0),
            )
            .unwrap();
        let saved: crate::store::Character = serde_json::from_str(&raw).unwrap();
        assert!(saved.lifetime.realm_exits.is_empty());
        if failure == 2 {
            released(&a, account).await;
        } else if failure == 1 {
            assert!(lifecycle::state(&a, account).unwrap().2);
        }
        drop(socket);
        if failure == 0 {
            released(&a, account).await;
        }
        if let Some(reservations) = saturated {
            a.world
                .directory(move |d| {
                    for (account, key) in reservations {
                        d.cancel_admission(account, key)?;
                    }
                    assert_eq!(
                        d.worlds.values().map(|w| w.reserved_slots()).sum::<usize>(),
                        0
                    );
                    Ok(())
                })
                .await
                .unwrap();
        }
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_death_fame_phase4_multiple_recipients_terminal_transfer() {
    let (dir, a) = account_actions::tests::fixture();
    a.world.clock.set_paused(true);
    let (_go, giver, g, destination, time, key, reservation) =
        earned_source(&a, "phase4-multiple-giver").await;
    let mut local = a.clone();
    local.world = local.world.for_player(giver);
    let (_ao, aa, mut ac) = avatar(&local, "phase4-multiple-dying-recipient").await;
    let (_bo, ba, mut bc) = avatar(&local, "phase4-multiple-disconnect-recipient").await;
    for ch in [&mut ac, &mut bc] {
        ch.exp = 49;
        a.store.progress(ch.clone()).await.unwrap();
    }
    local
        .world
        .call(move |w| {
            for id in [aa, ba] {
                let p = w.players.get_mut(&id).unwrap();
                p.character.exp = 49;
                p.queue_experience(1, Some(g.id), true)?;
            }
            let p = w.players.get_mut(&aa).unwrap();
            p.character.hp = 0;
            p.death_pending = true;
            Ok(())
        })
        .await
        .unwrap();
    let (entered, release) = a.store.lifecycle_test_pause_reply();
    let release = HeldReply(Some(release));
    let fg = lifecycle::finalize_transfer(&a, giver, reservation).unwrap();
    let fa = lifecycle::finalize(&a, aa, CloseReason::Controller).unwrap();
    let fb = lifecycle::finalize(&a, ba, CloseReason::Transport).unwrap();
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .unwrap()
        .unwrap();
    a.world
        .for_player(giver)
        .call(move |w| {
            assert!(
                !w.credit_reservation_for_test(giver).1,
                "giver snapshot preceded a held recipient publication"
            );
            Ok(())
        })
        .await
        .unwrap();
    drop(release);
    let (gr, ar, br) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(fg.wait(), fa.wait(), fb.wait())
    })
    .await
    .unwrap();
    gr.unwrap();
    ar.unwrap();
    br.unwrap();
    let saved = a.store.load(g.account, g.id).await.unwrap();
    assert_eq!(saved.lifetime.level_assists, 2);
    assert!(
        saved
            .lifetime
            .realm_exits
            .contains(&crate::dungeon::HistoricalDungeon::SnakePit)
    );
    assert_eq!(a.store.load(bc.account, bc.id).await.unwrap().level, 2);
    let db = rusqlite::Connection::open(dir.join("game.db")).unwrap();
    let raw: String = db
        .query_row(
            "SELECT snapshot FROM deaths WHERE character=?",
            [ac.id],
            |r| r.get(0),
        )
        .unwrap();
    let dead: crate::store::Character = serde_json::from_str(&raw).unwrap();
    assert_eq!(dead.level, 2);
    assert!(dead.lifetime.realm_exits.is_empty());
    assert!(
        !a.store
            .source_character_alive(ac.account, ac.id)
            .await
            .unwrap()
    );
    let reopened = Store::open(&dir.join("game.db"), &a.content).unwrap();
    assert_eq!(reopened.assist_count(g.id).await.unwrap(), 2);
    assert_eq!(
        reopened
            .load(g.account, g.id)
            .await
            .unwrap()
            .lifetime
            .level_assists,
        2
    );
    a.world
        .directory(move |d| {
            let (character, arrival) = d.consume(g.account, destination, time, key.clone())?;
            assert_eq!(character, g.id);
            d.admit_transfer(arrival, g.account, g.id, destination)?;
            assert!(d.consume(g.account, destination, time, key).is_err());
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_death_fame_phase4_owned_arrival_takeover_and_drain() {
    for draining in [false, true] {
        let (dir, a) = account_actions::tests::fixture();
        a.world.clock.set_paused(true);
        let guid = "phase4-owned-arrival";
        let (owner, source, ch, destination, time, key, reservation) =
            earned_source(&a, guid).await;
        let stale_pending = lifecycle::binding(&a, ch.account).unwrap();
        let stale_next = lifecycle::binding(&a, ch.account).unwrap();
        lifecycle::finalize_transfer(&a, source, reservation)
            .unwrap()
            .wait()
            .await
            .unwrap();
        owner.release();
        released(&a, ch.account).await;
        let (port, server) = socket_server(&a).await;
        let mut socket = NativeSocket::connect(port, &a.origin).await;
        socket
            .send(socket_hello(
                &dir,
                guid,
                destination,
                Some((time, key.clone())),
            ))
            .await;
        socket.receive(wire::MAP_INFO).await;
        lifecycle::account(
            &a,
            vec![ch.account],
            "phase4_owned_hello_settled",
            |_| async { Ok(()) },
        )
        .await
        .unwrap();
        let (entered, release) = a.store.lifecycle_test_pause_reply();
        let release = HeldReply(Some(release));
        socket
            .send(Packet {
                id: wire::LOAD,
                body: ch.id.to_be_bytes().to_vec(),
            })
            .await;
        tokio::time::timeout(Duration::from_secs(5), entered)
            .await
            .unwrap()
            .unwrap();
        let state = lifecycle::state(&a, ch.account).unwrap();
        assert!(state.0 && state.1.is_none() && !state.2);
        assert!(a.claim(ch.account).is_err());
        assert!(stale_pending(source).is_err());
        a.world
            .directory(move |d| {
                assert!(
                    d.worlds.values().all(|w| w
                        .players
                        .values()
                        .all(|p| p.character.account != ch.account)),
                    "LOAD published before owned Store reply"
                );
                Ok(())
            })
            .await
            .unwrap();
        if draining {
            lifecycle::drain::prepare(&a);
            assert!(!lifecycle::drain::open(&a));
            assert!(a.claim(ch.account).is_err());
        }
        drop(release);
        if draining {
            tokio::time::timeout(Duration::from_secs(5), lifecycle::drain::terminate(&a))
                .await
                .unwrap()
                .unwrap();
            released(&a, ch.account).await;
            assert!(stale_next(source).is_err());
            drop(socket);
        } else {
            let packet = socket.receive(wire::CREATE_SUCCESS).await;
            let admitted = Reader::new(&packet.body).i32().unwrap();
            assert_ne!(admitted, source);
            lifecycle::account(
                &a,
                vec![ch.account],
                "phase4_owned_load_settled",
                |_| async { Ok(()) },
            )
            .await
            .unwrap();
            a.world
                .directory(move |d| {
                    let ids: Vec<_> = d
                        .worlds
                        .values()
                        .flat_map(|w| w.players.iter())
                        .filter(|(_, p)| p.character.account == ch.account)
                        .map(|(id, _)| *id)
                        .collect();
                    assert_eq!(ids, vec![admitted]);
                    Ok(())
                })
                .await
                .unwrap();
            assert!(a.claim(ch.account).is_err());
            drop(socket);
            released(&a, ch.account).await;
            let next = a.claim(ch.account).unwrap();
            assert!(stale_next(source).is_err());
            assert!(!lifecycle::state(&a, ch.account).unwrap().2);
            assert!(
                a.world
                    .directory(move |d| d.consume(ch.account, destination, time, key))
                    .await
                    .is_err()
            );
            assert!(!lifecycle::state(&a, ch.account).unwrap().2);
            drop(next);
        }
        a.world
            .directory(move |d| {
                assert!(
                    d.worlds.values().all(|w| w
                        .players
                        .values()
                        .all(|p| p.character.account != ch.account)),
                    "retired LOAD left a late/ghost actor"
                );
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(a.store.lifecycle_test_jobs(), 0);
        assert!(
            a.store
                .load(ch.account, ch.id)
                .await
                .unwrap()
                .lifetime
                .realm_exits
                .contains(&crate::dungeon::HistoricalDungeon::SnakePit)
        );
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_input_backlog_survives_overrunning_world() {
    let (dir, a) = account_actions::tests::fixture();
    a.world.clock.set_paused(true);
    let (port, server) = socket_server(&a).await;
    let account = a.store.list("input-backlog".into()).await.unwrap().id;
    let ch = a
        .store
        .create(account, a.content.get(WIZARD).unwrap())
        .await
        .unwrap();
    let mut socket = NativeSocket::connect(port, &a.origin).await;
    let id = socket_load(&mut socket, &dir, "input-backlog", ch.id).await;
    // Every world job waits behind a 60ms overrunning tick, as during the #486 Lord of the Lost Lands fight.
    let world = a.world.for_player(id);
    let overrun = tokio::spawn(async move {
        loop {
            world
                .call(|_| {
                    std::thread::sleep(Duration::from_millis(60));
                    Ok(())
                })
                .await
                .unwrap();
        }
    });
    for _ in 0..150 {
        let mut hit = Writer::new();
        hit.u8(0).i32(id);
        socket
            .send(Packet {
                id: 24,
                body: hit.0,
            })
            .await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    overrun.abort();
    socket
        .send(Packet {
            id: wire::TELEPORT,
            body: id.to_be_bytes().to_vec(),
        })
        .await;
    let reply = socket.receive(wire::TEXT).await;
    assert!(
        String::from_utf8_lossy(&reply.body).contains("already at yourself"),
        "{:?}",
        String::from_utf8_lossy(&reply.body)
    );
    drop(socket);
    server.abort();
}
