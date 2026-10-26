use super::*;

async fn request(app: &App, id: &str, command: Value) -> Value {
    let mut headers = HeaderMap::new();
    headers.insert("authorization", "Bearer guild-test".parse().unwrap());
    let response = super::command(
        State(app.clone()),
        headers,
        Json(json!({"id":id,"command":command})),
    )
    .await;
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1_048_576)
            .await
            .unwrap(),
    )
    .unwrap()
}

fn emitted_tick_stats(
    receiver: &mut tokio::sync::mpsc::Receiver<crate::wire::Packet>,
) -> BTreeMap<(i32, u8), Value> {
    let packets = std::iter::from_fn(|| receiver.try_recv().ok())
        .filter(|packet| packet.id == crate::wire::NEW_TICK)
        .collect::<Vec<_>>();
    assert_eq!(packets.len(), 1);
    let mut wire = crate::wire::Reader::new(&packets[0].body);
    wire.i32().unwrap();
    wire.i32().unwrap();
    let count = wire.i16().unwrap();
    let mut stats = BTreeMap::new();
    for _ in 0..count {
        let player = wire.i32().unwrap();
        wire.f32().unwrap();
        wire.f32().unwrap();
        for _ in 0..wire.i16().unwrap() {
            let key = wire.u8().unwrap();
            let value = if matches!(key, 31 | 62) {
                json!(wire.utf().unwrap())
            } else {
                json!(wire.i32().unwrap())
            };
            assert!(stats.insert((player, key), value).is_none());
        }
    }
    wire.done().unwrap();
    stats
}

// Backend stepping exercises wire delivery, not native timing, input or rendering.
async fn step_guild_world(app: &App) {
    let clock = app.world.clock.clone();
    app.world
        .in_world(-2)
        .call(move |w| {
            clock.set_paused(false);
            w.step();
            w.step();
            clock.set_paused(true);
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn prepared_guild_commits_real_relation_without_wallet_or_star_changes() {
    let (dir, mut app) = crate::server::account_actions::tests::fixture();
    write_json(
        &dir.join("testing.json"),
        &json!({"schema":1,"provenance":"prepared"}),
    )
    .unwrap();
    app.testing = Testing::open(&dir).unwrap();
    app.control = Some("guild-test".into());
    app.world.clock.set_paused(false);
    assert_eq!(request(&app,"running",json!({"op":"cheat","actor":999,"change":{"kind":"guild","name":"Prepared","members":[]}})).await["status"],"failed");
    app.world.clock.set_paused(true);
    let pos = app
        .world
        .in_world(-2)
        .call(|w| {
            let (x, y) = w.map.spawns[0];
            Ok(Pos { x, y })
        })
        .await
        .unwrap();
    let mut actors = Vec::new();
    let mut states = Vec::new();
    let mut presentations = Vec::new();
    for i in 0..3 {
        let result=request(&app,&format!("spawn-{i}"),json!({"op":"spawn","spec":{"guid":format!("qa-guild-{i}"),"class":WIZARD,"level":8,"seed":12351,"equipment":[-1,-1,-1,-1],"backpack":[-1,-1,-1,-1,-1,-1,-1,-1],"world":-2,"position":pos,"controller":"direct"}})).await;
        assert_eq!(result["status"], "succeeded", "{result}");
        let actor = result["result"]["actor"].as_i64().unwrap();
        actors.push(actor);
        let state = request(
            &app,
            &format!("before-{i}"),
            json!({"op":"observe","actor":actor}),
        )
        .await;
        presentations.push(
            app.store
                .presentation(state["session"]["account"].as_i64().unwrap() as i32)
                .await
                .unwrap(),
        );
        states.push(state);
    }
    let command = json!({"op":"cheat","actor":actors[0],"change":{"kind":"guild","name":"Prepared","members":[actors[1]]}});
    let testing = app.testing.take();
    assert_eq!(
        request(&app, "ordinary", command.clone()).await["status"],
        "failed"
    );
    app.testing = testing;
    for (i, change) in [
        json!({"kind":"guild","name":"!","members":[actors[1]]}),
        json!({"kind":"guild","name":"Prepared","members":[actors[0]]}),
        json!({"kind":"guild","name":"Prepared","members":[99999]}),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            request(
                &app,
                &format!("invalid-{i}"),
                json!({"op":"cheat","actor":actors[0],"change":change})
            )
            .await["status"],
            "failed"
        );
        assert_eq!(
            request(
                &app,
                &format!("invalid-unchanged-{i}"),
                json!({"op":"observe","actor":actors[0]})
            )
            .await,
            states[0]
        );
    }
    let players = states
        .iter()
        .map(|s| s["session"]["player"].as_i64().unwrap() as i32)
        .collect::<Vec<_>>();
    let selected = players.clone();
    let mut receivers = app
        .world
        .in_world(-2)
        .call(move |w| {
            let mut receivers = Vec::new();
            for player in &selected {
                let (packets, receiver) = tokio::sync::mpsc::channel(128);
                w.players.get_mut(player).unwrap().tx = Some(crate::world::Output {
                    packets,
                    lost: tokio::sync::watch::channel(None).0,
                });
                receivers.push(receiver);
            }
            Ok(receivers)
        })
        .await
        .unwrap();
    step_guild_world(&app).await;
    for (recipient, receiver) in receivers.iter_mut().enumerate() {
        let stats = emitted_tick_stats(receiver);
        for (i, player) in players.iter().enumerate() {
            assert_eq!(stats[&(*player, 62)], "");
            assert_eq!(stats[&(*player, 63)], 0);
            assert_eq!(stats[&(*player, 30)], presentations[i].stars());
        }
        assert_eq!(
            stats[&(players[recipient], 35)],
            presentations[recipient].credits
        );
        assert_eq!(
            stats[&(players[recipient], 39)],
            presentations[recipient].fame
        );
    }
    let selected = players.clone();
    app.world
        .in_world(-2)
        .call(move |w| {
            for player in &selected {
                assert!(
                    selected
                        .iter()
                        .all(|id| w.players[player].known.contains(id))
                );
            }
            Ok(())
        })
        .await
        .unwrap();
    step_guild_world(&app).await;
    for receiver in &mut receivers {
        let stats = emitted_tick_stats(receiver);
        assert!(!stats.keys().any(|(_, key)| matches!(key, 62 | 63)));
    }
    for (i, state) in states.iter_mut().enumerate() {
        *state = request(
            &app,
            &format!("primed-{i}"),
            json!({"op":"observe","actor":actors[i]}),
        )
        .await;
    }
    let result = request(&app, "create", command.clone()).await;
    assert_eq!(result["status"], "succeeded", "{result}");
    assert_eq!(result["result"]["prepared_guild"]["unearned"], true);
    assert_eq!(
        result["result"]["prepared_guild"]["client_publication"],
        "next_ordinary_unpaused_tick"
    );
    assert!(app.world.clock.paused());
    for receiver in &mut receivers {
        assert!(matches!(
            receiver.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }
    assert_eq!(request(&app, "create", command).await, result);
    let guild = result["result"]["prepared_guild"]["guild"]["id"]
        .as_i64()
        .unwrap() as i32;
    for i in 0..3 {
        let account = states[i]["session"]["account"].as_i64().unwrap() as i32;
        let actual = app.store.presentation(account).await.unwrap();
        let mut expected = presentations[i].clone();
        if i < 2 {
            assert_eq!(actual.guild.as_ref().unwrap().id, guild);
            assert_eq!(
                actual.guild.as_ref().unwrap().rank,
                if i == 0 { 40 } else { 0 }
            );
            expected.guild = actual.guild.clone();
        }
        assert_eq!(actual, expected);
        let after = request(
            &app,
            &format!("after-{i}"),
            json!({"op":"observe","actor":actors[i]}),
        )
        .await;
        assert_eq!(after["session"], states[i]["session"]);
        assert_eq!(after["character"], states[i]["character"]);
    }
    let rejected=request(&app,"taken",json!({"op":"cheat","actor":actors[2],"change":{"kind":"guild","name":"prepared","members":[]}})).await;
    assert_eq!(rejected["status"], "failed");
    assert_eq!(
        request(
            &app,
            "third-unchanged",
            json!({"op":"observe","actor":actors[2]})
        )
        .await,
        states[2]
    );
    let account = states[2]["session"]["account"].as_i64().unwrap() as i32;
    assert!(
        app.store
            .change_guild(
                account,
                crate::store::GuildAction::Create("Ordinary".into())
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("Choose a name")
    );
    let founder = states[0]["session"]["account"].as_i64().unwrap() as i32;
    let info = app.store.guild(founder).await.unwrap();
    assert_eq!((info.fame, info.earned), (0, 0));
    step_guild_world(&app).await;
    for receiver in &mut receivers {
        let stats = emitted_tick_stats(receiver);
        assert_eq!(stats[&(players[0], 62)], "Prepared");
        assert_eq!(stats[&(players[0], 63)], 40);
        assert_eq!(stats[&(players[1], 62)], "Prepared");
        // Member rank remains zero, so the primed delta cache must elide it.
        assert!(!stats.contains_key(&(players[1], 63)));
        assert!(!stats.contains_key(&(players[2], 62)));
        assert!(!stats.contains_key(&(players[2], 63)));
        assert!(!stats.keys().any(|(_, key)| matches!(key, 30 | 35 | 39)));
    }
    app.shutdown.send_replace(true);
    fs::remove_dir_all(dir).unwrap();
}

// Synthetic backend fixture: ordinary policy and wire deltas, not native rendering proof.
#[tokio::test]
async fn prepared_pause_uses_ordinary_policy_and_preserves_known_guild() {
    async fn persisted_guild_state(
        app: &App,
        members: &[(i32, i32)],
        path: &Path,
    ) -> (Value, Vec<crate::store::AccountPresentation>) {
        let mut characters = Vec::new();
        let mut presentations = Vec::new();
        for &(actor, account) in members {
            characters.push(app.store.load(account, actor).await.unwrap());
            presentations.push(app.store.presentation(account).await.unwrap());
        }
        let guild = app.store.guild(members[0].1).await.unwrap();
        let db =
            rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let rows = db.prepare("SELECT account,guild,rank,fame FROM guild_members WHERE guild=?1 ORDER BY account LIMIT 51").unwrap()
            .query_map([guild.id], |r| Ok((r.get::<_,i32>(0)?,r.get::<_,i32>(1)?,r.get::<_,i32>(2)?,r.get::<_,i32>(3)?))).unwrap()
            .collect::<rusqlite::Result<Vec<_>>>().unwrap();
        assert_eq!(rows.len(), members.len());
        assert_eq!(
            rows.iter().map(|r| r.0).collect::<BTreeSet<_>>(),
            members.iter().map(|m| m.1).collect()
        );
        (
            json!({"characters":characters,"guild":guild,"memberships":rows}),
            presentations,
        )
    }

    let (dir, mut app) = crate::server::account_actions::tests::fixture();
    write_json(
        &dir.join("testing.json"),
        &json!({"schema":1,"provenance":"prepared"}),
    )
    .unwrap();
    app.testing = Testing::open(&dir).unwrap();
    app.control = Some("guild-test".into());
    app.world.clock.set_paused(true);
    let pos = app
        .world
        .in_world(-2)
        .call(|w| {
            let (x, y) = w.map.spawns[0];
            Ok(Pos { x, y })
        })
        .await
        .unwrap();
    let mut actors = Vec::new();
    for i in 0..2 {
        let result=request(&app,&format!("pause-spawn-{i}"),json!({"op":"spawn","spec":{"guid":format!("qa-pause-{i}"),"class":WIZARD,"level":8,"seed":12351,"equipment":[-1,-1,-1,-1],"backpack":[-1,-1,-1,-1,-1,-1,-1,-1],"world":-2,"position":pos,"controller":"direct"}})).await;
        assert_eq!(result["status"], "succeeded", "{result}");
        actors.push(result["result"]["actor"].as_i64().unwrap() as i32);
    }
    let actor = actors[0];
    let player = resolve(&app, actor).await.unwrap();
    let viewer = resolve(&app, actors[1]).await.unwrap();
    let guild = request(&app,"pause-guild",json!({"op":"cheat","actor":actor,"change":{"kind":"guild","name":"PauseGuild","members":[actors[1]]}})).await;
    assert_eq!(guild["status"], "succeeded", "{guild}");
    let mut receiver = app
        .world
        .in_world(-2)
        .call(move |w| {
            let (packets, receiver) = tokio::sync::mpsc::channel(128);
            w.players.get_mut(&viewer).unwrap().tx = Some(crate::world::Output {
                packets,
                lost: tokio::sync::watch::channel(None).0,
            });
            Ok(receiver)
        })
        .await
        .unwrap();
    step_guild_world(&app).await;
    let stats = emitted_tick_stats(&mut receiver);
    assert_eq!(stats[&(player, 62)], "PauseGuild");
    assert_eq!(stats[&(player, 63)], 40);
    assert_eq!(stats[&(player, 29)].as_i64().unwrap() & (1 << 20), 0);
    let command =
        |paused| json!({"op":"cheat","actor":actor,"change":{"kind":"pause","paused":paused}});
    let before = snapshot(&app, actor, false).await.unwrap();
    let mut members = Vec::new();
    for &actor in &actors {
        let state = snapshot(&app, actor, false).await.unwrap();
        members.push((actor, state["session"]["account"].as_i64().unwrap() as i32));
    }
    let db_path = dir.join("game.db");
    let persisted = persisted_guild_state(&app, &members, &db_path).await;
    let testing = app.testing.take();
    assert_eq!(
        request(&app, "pause-unprepared", command(true)).await["status"],
        "failed"
    );
    app.testing = testing;
    app.world.clock.set_paused(false);
    // Rejection precedes actor lookup and never takes a lifecycle reservation.
    assert!(
        prepare_pause(&app, app.testing.as_ref().unwrap(), actor, true)
            .await
            .unwrap_err()
            .to_string()
            .contains("explicitly paused")
    );
    app.world.clock.set_paused(true);
    for (id, cmd) in [
        (
            "pause-foreign",
            json!({"op":"cheat","actor":99999,"change":{"kind":"pause","paused":true}}),
        ),
        ("pause-noop", command(false)),
    ] {
        assert_eq!(request(&app, id, cmd).await["status"], "failed");
    }
    app.world
        .in_world(-2)
        .call(move |w| {
            w.players.get_mut(&player).unwrap().controller = crate::world::Controller::Human;
            Ok(())
        })
        .await
        .unwrap();
    let rejected = request(&app, "pause-human", command(true)).await;
    assert_eq!(rejected["status"], "failed");
    assert!(rejected.to_string().contains("direct actor"), "{rejected}");
    app.world
        .in_world(-2)
        .call(move |w| {
            w.players.get_mut(&player).unwrap().controller = crate::world::Controller::Qa;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(snapshot(&app, actor, false).await.unwrap(), before);
    assert_eq!(
        persisted_guild_state(&app, &members, &db_path).await,
        persisted
    );
    assert!(app.quarantined.lock().unwrap().is_empty());
    for (index, paused) in [true, false].into_iter().enumerate() {
        let initial = snapshot(&app, actor, false).await.unwrap();
        let result = request(&app, &format!("pause-toggle-{index}"), command(paused)).await;
        assert_eq!(result["status"], "succeeded", "{result}");
        assert_eq!(
            result["result"]["prepared_pause"],
            json!({"paused":paused,"client_publication":"next_ordinary_unpaused_tick"})
        );
        let mut after = snapshot(&app, actor, false).await.unwrap();
        assert_eq!(
            after["character"]["conditions"].as_i64().unwrap() & (1 << 20),
            if paused { 1 << 20 } else { 0 }
        );
        after["character"]["conditions"] = initial["character"]["conditions"].clone();
        assert_eq!(after, initial);
        assert_eq!(
            persisted_guild_state(&app, &members, &db_path).await,
            persisted
        );
        assert!(receiver.try_recv().is_err());
        let rejected = request(&app, &format!("pause-repeat-{index}"), command(paused)).await;
        assert_eq!(rejected["status"], "failed");
        assert!(rejected.to_string().contains("already has"), "{rejected}");
        assert_eq!(
            persisted_guild_state(&app, &members, &db_path).await,
            persisted
        );
        app.world
            .in_world(-2)
            .call(move |w| {
                assert!(w.players[&viewer].known.contains(&player));
                Ok(())
            })
            .await
            .unwrap();
        step_guild_world(&app).await;
        let stats = emitted_tick_stats(&mut receiver);
        assert_eq!(
            stats[&(player, 29)].as_i64().unwrap() & (1 << 20),
            if paused { 1 << 20 } else { 0 }
        );
        assert!(!stats.keys().any(|(_, key)| matches!(key, 62 | 63)));
        let before_hold = snapshot(&app, actor, false).await.unwrap();
        let clock = app.world.clock.clone();
        app.world
            .in_world(-2)
            .call(move |w| {
                if paused {
                    let start = w.now;
                    clock.set_paused(false);
                    let result = w.test_paused_script_heartbeats(player);
                    clock.set_paused(true);
                    result?;
                    assert_eq!(w.now - start, 2000);
                } else {
                    let p = &w.players[&player];
                    w.move_to(player, p.pos, w.now as i32)?;
                    assert_eq!(w.players[&player].native_time(), w.now as i32);
                }
                Ok(())
            })
            .await
            .unwrap();
        let after_hold = snapshot(&app, actor, false).await.unwrap();
        assert_eq!(after_hold["lifecycle"], before_hold["lifecycle"]);
        assert_eq!(
            after_hold["game_ms"].as_u64().unwrap() - before_hold["game_ms"].as_u64().unwrap(),
            if paused { 2000 } else { 0 }
        );
        while receiver.try_recv().is_ok() {}
        assert_eq!(
            persisted_guild_state(&app, &members, &db_path).await,
            persisted
        );
    }
    let before_enemy = snapshot(&app, actor, false).await.unwrap();
    app.world
        .in_world(-2)
        .call(move |w| {
            w.players.get_mut(&player).unwrap().controller = crate::world::Controller::Qa;
            let kind = w
                .content
                .objects
                .iter()
                .find(|(_, o)| o.enemy && o.hp > 0)
                .map(|(k, _)| *k)
                .unwrap();
            w.spawn_entity(kind, pos, Vec::new())
        })
        .await
        .unwrap();
    let rejected = request(&app, "pause-unsafe", command(true)).await;
    assert_eq!(rejected["status"], "failed");
    assert!(
        rejected.to_string().contains("Not safe enough"),
        "{rejected}"
    );
    let after_rejection = snapshot(&app, actor, false).await.unwrap();
    assert_eq!(after_rejection["lifecycle"], before_enemy["lifecycle"]);
    assert_eq!(after_rejection["character"], before_enemy["character"]);
    assert_eq!(
        persisted_guild_state(&app, &members, &db_path).await,
        persisted
    );
    assert!(app.quarantined.lock().unwrap().is_empty());
    app.shutdown.send_replace(true);
    fs::remove_dir_all(dir).unwrap();
}
