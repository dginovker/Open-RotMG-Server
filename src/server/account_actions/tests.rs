use super::*;

pub(crate) fn fixture() -> (PathBuf, App) {
    fixture_with_realm_seed(None)
}
fn fixture_with_realm_seed(production_seed: Option<u64>) -> (PathBuf, App) {
    let dir = std::env::temp_dir().join(format!("session-actions-{}", rand::random::<u64>()));
    std::fs::create_dir(&dir).unwrap();
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let app = App {
        diagnostics: Arc::new(
            super::super::diagnostics::Diagnostics::open(dir.join("bug-reports")).unwrap(),
        ),
        started: std::time::Instant::now(),
        testing: None,
        society: Arc::default(),
        cctv: Arc::default(),
        store: Store::open(&dir.join("game.db"), &content).unwrap(),
        world: Handle::spawn(
            match production_seed {
                Some(seed) => crate::world::Directory::with_seed(
                    std::path::Path::new("data"),
                    content.clone(),
                    16,
                    seed,
                    2,
                ),
                None => crate::world::Directory::test_with_seed(
                    std::path::Path::new("data"),
                    content.clone(),
                    16,
                    12351,
                ),
            }
            .unwrap(),
        ),
        keys: Arc::new(Keys::load(&dir).unwrap()),
        content,
        swf: Arc::default(),
        origin: "http://127.0.0.1:1".into(),
        sfx: "data/sfx".into(),
        max_players: 16,
        behind_proxy: false,
        shore_bots: false,
        dialogue: None,
        requests: Arc::default(),
        picture_reads: Arc::default(),
        connections: Arc::new(tokio::sync::Semaphore::new(32)),
        control: None,
        discord_link_token: Some("a".repeat(64)),
        browser_links: Arc::default(),
        owners: Arc::default(),
        admission_started: Arc::default(),
        admission: Arc::default(),
        authentication: Arc::default(),
        guild_actions: Arc::default(),
        gold_updates: Arc::default(),
        bots: Arc::default(),
        quarantined: Arc::default(),
        shutdown: tokio::sync::watch::channel(false).0,
    };
    (dir, app)
}
