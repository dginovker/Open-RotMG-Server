use super::*;
use crate::world::Controller;

async fn count(client: &reqwest::Client, url: &str, expected: &str) {
    let response = client
        .get(format!("{url}/realmdex/stats"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "text/plain; charset=utf-8"
    );
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body = response.text().await.unwrap();
    assert_eq!(body, expected);
    println!(
        "GET /realmdex/stats -> 200 text/plain; charset=utf-8 | Cache-Control: no-store | body: {body}"
    );
}

#[tokio::test]
async fn realmdex_counts_only_live_humans_across_worlds_and_reports_failure() {
    let (dir, app) = test_app();
    app.world.clock.set_paused(true);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let router = routes(&app, dir.join("client"), false, None);
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let client = reqwest::Client::new();
    count(&client, &url, "0").await;
    let mut actors = Vec::new();
    let mut outputs = Vec::new();
    for (i, (world, controller)) in [
        (-1, Controller::Human),
        (-2, Controller::Human),
        (-1, Controller::Autonomous),
        (-2, Controller::Qa),
    ]
    .into_iter()
    .enumerate()
    {
        let account = app
            .store
            .characters(format!("realmdex-fixture-{i}"), String::new())
            .await
            .unwrap()
            .id;
        let character = app
            .store
            .create(account, app.content.get(WIZARD).unwrap())
            .await
            .unwrap();
        let (tx, rx) = mpsc::channel(256);
        outputs.push(rx);
        let actor = app
            .world
            .in_world(world)
            .call(move |w| {
                let id = w.enter(
                    character,
                    Some(crate::world::Output {
                        trace: None,
                        packets: tx,
                        lost: tokio::sync::watch::channel(None).0,
                    }),
                )?;
                w.players
                    .get_mut(&id)
                    .context("test actor missing")?
                    .controller = controller;
                Ok(id)
            })
            .await
            .unwrap();
        actors.push((world, actor, account));
    }
    let offline = app
        .store
        .characters("realmdex-offline".into(), String::new())
        .await
        .unwrap()
        .id;
    app.store
        .create(offline, app.content.get(WIZARD).unwrap())
        .await
        .unwrap();
    let viewer = app
        .world
        .spectate(actors[2].1, actors[2].2)
        .await
        .unwrap()
        .unwrap();
    count(&client, &url, "2").await;
    let health = app
        .world
        .directory(|d| Ok(d.health(std::time::Instant::now())))
        .await
        .unwrap();
    assert_eq!(health["players"], 2);
    assert_eq!(health["bots"], 1);
    assert_eq!(health["qa_actors"], 1);
    let id = actors[0].1;
    let character = app
        .world
        .in_world(-1)
        .call(move |w| w.leave(id))
        .await
        .unwrap();
    count(&client, &url, "1").await;
    let id = app
        .world
        .in_world(-2)
        .call(move |w| w.enter(character, None))
        .await
        .unwrap();
    app.world
        .in_world(-2)
        .call(move |w| {
            w.players
                .get_mut(&id)
                .context("reconnected human missing")?
                .controller = Controller::Human;
            Ok(())
        })
        .await
        .unwrap();
    count(&client, &url, "2").await;
    drop(viewer);
    app.world.supervisor.failure(
        "realmdex-test",
        anyhow::anyhow!("private diagnostic detail"),
    );
    let response = client
        .get(format!("{url}/realmdex/stats"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.text().await.unwrap(), "player count unavailable");
    server.abort();
    drop(outputs);
    drop(app);
}
