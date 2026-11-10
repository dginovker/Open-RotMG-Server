use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{net::SocketAddr, path::PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(skip)]
    pub desktop: bool,
    pub bind: SocketAddr,
    pub origin: String,
    #[serde(default)]
    pub bot_view_origin: Option<String>,
    pub data: PathBuf,
    pub client: PathBuf,
    pub state: PathBuf,
    #[serde(default, deserialize_with = "supplied_backup")]
    pub pre_migration_backup: Option<PathBuf>,
    #[serde(default, deserialize_with = "supplied_generation")]
    pub client_generation: Option<String>,
    #[serde(default)]
    pub test_state: bool,
    pub max_players: usize,
    pub behind_proxy: bool,
    #[serde(default = "default_shore_bots")]
    pub shore_bots: bool,
    #[serde(default)]
    pub bot_llm: bool,
    #[serde(default = "default_bot_population")]
    pub bot_population: usize,
    #[serde(default = "default_bot_session_seconds")]
    pub bot_session_seconds: [u64; 2],
    pub realm_seed: u64,
    #[serde(default = "default_realm_count")]
    pub realm_count: usize,
}
fn supplied_backup<'de, D: serde::Deserializer<'de>>(
    input: D,
) -> std::result::Result<Option<PathBuf>, D::Error> {
    PathBuf::deserialize(input).map(Some)
}
fn supplied_generation<'de, D: serde::Deserializer<'de>>(
    input: D,
) -> std::result::Result<Option<String>, D::Error> {
    String::deserialize(input).map(Some)
}
fn default_realm_count() -> usize {
    2
}
fn default_bot_session_seconds() -> [u64; 2] {
    [1800, 5400]
}
fn default_bot_population() -> usize {
    20
}
fn default_shore_bots() -> bool {
    true
}

impl Config {
    pub fn load(args: &[String]) -> Result<(Self, bool)> {
        let mut file = None;
        let mut port = None;
        let mut realm_seed = None;
        let mut control = false;
        let mut desktop = false;
        let mut shore_bots = None;
        let mut bot_llm = None;
        let mut client_dir = None;
        let mut state_dir = None;
        let mut test_state = false;
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--config" => {
                    ensure!(file.is_none(), "duplicate --config");
                    file = Some(args.next().context("--config requires a JSON file")?);
                }
                "--port" => {
                    ensure!(port.is_none(), "duplicate --port");
                    let value = args.next().context("--port requires a port number")?;
                    let value: u16 = value.parse().context("port must be between 1 and 65535")?;
                    ensure!(value != 0, "port must be between 1 and 65535");
                    port = Some(value);
                }
                "--realm-seed" => {
                    ensure!(realm_seed.is_none(), "duplicate --realm-seed");
                    realm_seed = Some(
                        args.next()
                            .context("--realm-seed requires an unsigned 64-bit integer")?
                            .parse::<u64>()
                            .context("--realm-seed requires an unsigned 64-bit integer")?,
                    );
                }
                "--desktop" => {
                    ensure!(!desktop, "duplicate --desktop");
                    desktop = true;
                }
                "--dev-control" => {
                    ensure!(!control, "duplicate --dev-control");
                    control = true;
                }
                "--shore-bots" | "--no-shore-bots" => {
                    ensure!(shore_bots.is_none(), "duplicate or conflicting bot flags");
                    shore_bots = Some(arg == "--shore-bots");
                }
                "--bot-llm" | "--no-bot-llm" => {
                    ensure!(bot_llm.is_none(), "duplicate or conflicting LLM flags");
                    bot_llm = Some(arg == "--bot-llm");
                }
                "--client" => {
                    ensure!(client_dir.is_none(), "duplicate --client");
                    client_dir = Some(PathBuf::from(
                        args.next().context("--client requires a directory")?,
                    ));
                }
                "--state" | "--test-state" => {
                    ensure!(state_dir.is_none(), "duplicate state directory");
                    if arg == "--test-state" {
                        test_state = true;
                    }
                    state_dir = Some(PathBuf::from(
                        args.next().context("--state requires a directory")?,
                    ));
                }
                _ => anyhow::bail!(
                    "usage: openrotmg [--desktop --config ABS_FILE | --config FILE] [--port PORT] [--dev-control] [--shore-bots | --no-shore-bots] [--bot-llm | --no-bot-llm] [--client DIR] [--state DIR] [--realm-seed SEED]"
                ),
            }
        }
        ensure!(
            !desktop || file.is_some_and(|path| std::path::Path::new(path).is_absolute()),
            "--desktop requires --config with an absolute JSON file path"
        );
        let mut config = if let Some(file) = file {
            serde_json::from_slice(
                &std::fs::read(file).with_context(|| format!("read config {file}"))?,
            )?
        } else {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            Self {
                desktop: false,
                bind: "127.0.0.1:18923".parse()?,
                origin: "http://127.0.0.1:18923".into(),
                bot_view_origin: Some("http://127.0.0.1:18923".into()),
                data: root.join("data"),
                client: root
                    .parent()
                    .context("workspace parent missing")?
                    .join("client"),
                state: root.join(".local"),
                pre_migration_backup: None,
                client_generation: None,
                test_state: false,
                max_players: 85,
                behind_proxy: false,
                shore_bots: true,
                bot_llm: false,
                bot_population: default_bot_population(),
                bot_session_seconds: default_bot_session_seconds(),
                realm_seed: rand::random(),
                realm_count: default_realm_count(),
            }
        };
        if let Some(client) = client_dir {
            config.client = client;
        }
        if let Some(state) = state_dir {
            config.state = state;
        }
        if let Some(port) = port {
            config.bind.set_port(port);
            if file.is_none() {
                config.origin = format!("http://{}", config.bind);
                config.bot_view_origin = Some(config.origin.clone());
            }
        }
        if let Some(seed) = realm_seed {
            config.realm_seed = seed;
        }
        if let Some(enabled) = shore_bots {
            config.shore_bots = enabled;
        }
        if let Some(enabled) = bot_llm {
            config.bot_llm = enabled;
        }
        config.desktop = desktop;
        config.test_state |= test_state;
        control |= test_state;
        ensure!(
            !config.test_state || control,
            "test state requires local development control"
        );
        ensure!(
            !config.test_state || !config.shore_bots,
            "prepared sessions require --no-shore-bots or shore_bots:false; spawn every controller explicitly"
        );
        config.validate(control)?;
        Ok((config, control))
    }
    fn validate(&self, control: bool) -> Result<()> {
        if let Some(generation) = &self.client_generation {
            ensure!(self.desktop, "client_generation requires desktop mode");
            ensure!(
                generation.len() == 64
                    && generation
                        .bytes()
                        .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
                "client_generation requires exactly 64 lowercase hexadecimal characters from the verified paired build identity"
            );
        }
        if let Some(path) = &self.pre_migration_backup {
            ensure!(self.desktop, "pre_migration_backup requires desktop mode");
            ensure!(
                path.is_absolute()
                    && path.file_name().is_some()
                    && path
                        .to_str()
                        .is_some_and(|s| !s.chars().any(char::is_control))
                    && path.components().all(|c| !matches!(
                        c,
                        std::path::Component::ParentDir | std::path::Component::CurDir
                    )),
                "pre_migration_backup requires an absolute UTF-8 file path without traversal or controls"
            );
        }
        if self.desktop {
            ensure!(
                !control && !self.test_state && !self.behind_proxy,
                "--desktop forbids development control, prepared state and proxy hosting"
            );
            ensure!(
                !self.shore_bots && !self.bot_llm,
                "--desktop requires shore_bots:false and bot_llm:false"
            );
            ensure!(
                self.origin == format!("http://{}", self.bind),
                "--desktop origin must match the loopback HTTP bind address"
            );
        }
        ensure!(
            (1..=crate::world::realm_population::MAX_CURRENT).contains(&self.realm_count),
            "initial realm_count must be between 1 and 8"
        );
        ensure!(
            self.bind.ip().is_loopback(),
            "backend bind must be loopback; terminate public TLS at the proxy"
        );
        if let Some(origin) = &self.bot_view_origin {
            let url = reqwest::Url::parse(origin).context("invalid bot_view_origin")?;
            let local = url.host_str().is_some_and(|h| {
                h == "localhost"
                    || h.parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
            });
            ensure!(
                url.origin().ascii_serialization() == *origin
                    && (url.scheme() == "https" || local && url.scheme() == "http"),
                "bot_view_origin requires an exact HTTPS origin, or loopback HTTP"
            );
        }
        let uri: axum::http::Uri = self.origin.parse().context("invalid public origin")?;
        ensure!(
            uri.authority().is_some()
                && uri.path() == "/"
                && uri.query().is_none()
                && !self.origin.ends_with('/'),
            "origin must be scheme://host[:port], without a path"
        );
        let local = uri.host().is_some_and(|h| {
            h == "localhost"
                || h.parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        ensure!(
            uri.scheme_str() == Some("https") || (local && uri.scheme_str() == Some("http")),
            "public origin requires HTTPS"
        );
        ensure!(
            !control || (local && !self.behind_proxy),
            "development control cannot be enabled behind a public proxy"
        );
        ensure!(
            !self.shore_bots || self.max_players > self.bot_population,
            "adventuring bots require capacity for {} bots plus a human",
            self.bot_population
        );
        ensure!(
            (30..=86400).contains(&self.bot_session_seconds[0])
                && self.bot_session_seconds[0] <= self.bot_session_seconds[1]
                && self.bot_session_seconds[1] <= 86400,
            "bot_session_seconds must be an ordered range between 30 and 86400 seconds"
        );
        ensure!(
            self.bot_population <= crate::bots::persona_count(),
            "bot_population exceeds the {} catalogued personas",
            crate::bots::persona_count()
        );
        ensure!(
            (1..=85).contains(&self.max_players),
            "max_players must be between 1 and 85"
        );
        ensure!(
            [&self.data, &self.client, &self.state]
                .iter()
                .all(|p| p.is_absolute()),
            "data, client and state paths must be absolute"
        );
        Ok(())
    }
}

#[cfg(test)]
mod bot_view_tests {
    use super::*;
    #[test]
    fn initial_realm_count_accepts_one_to_eight_without_changing_player_limit() {
        let (mut config, _) = Config::load(&[]).unwrap();
        assert_eq!(config.realm_count, 2);
        config.max_players = 72;
        for count in 1..=8 {
            config.realm_count = count;
            config.validate(false).unwrap();
            assert_eq!(config.max_players, 72);
        }
        for count in [0, 9, usize::MAX] {
            config.realm_count = count;
            assert!(
                config
                    .validate(false)
                    .unwrap_err()
                    .to_string()
                    .contains("initial realm_count")
            );
        }
    }

    #[test]
    fn explicit_realm_seed_preserves_zero_and_rejects_invalid_arguments() {
        for seed in [0, u64::MAX] {
            let (config, _) = Config::load(&["--realm-seed".into(), seed.to_string()]).unwrap();
            assert_eq!(config.realm_seed, seed);
        }
        for args in [
            vec!["--realm-seed"],
            vec!["--realm-seed", "-1"],
            vec!["--realm-seed", "18446744073709551616"],
            vec!["--realm-seed", "1", "--realm-seed", "2"],
        ] {
            assert!(Config::load(&args.into_iter().map(String::from).collect::<Vec<_>>()).is_err());
        }
    }

    #[test]
    fn bot_view_origin_is_explicit_and_local_defaults_track_the_port() {
        let (mut config, _) = Config::load(&["--port".into(), "19023".into()]).unwrap();
        assert_eq!(
            config.bot_view_origin.as_deref(),
            Some(config.origin.as_str())
        );
        for origin in [
            "https://game.example/path",
            "https://user@game.example",
            "http://game.example",
            "https://game.example/",
        ] {
            config.bot_view_origin = Some(origin.into());
            assert!(config.validate(false).is_err(), "{origin}");
        }
        config.bot_view_origin = Some("https://game.example".into());
        config.validate(false).unwrap();
        config.bot_view_origin = None;
        config.validate(false).unwrap();
    }
}

#[cfg(test)]
mod desktop_tests {
    use super::*;

    #[test]
    fn desktop_requires_an_explicit_absolute_config_and_preserves_ordinary_defaults() {
        for args in [
            vec!["--desktop"],
            vec!["--desktop", "--config", "relative.json"],
            vec!["--desktop", "--desktop"],
        ] {
            assert!(Config::load(&args.into_iter().map(String::from).collect::<Vec<_>>()).is_err());
        }
        let (config, control) = Config::load(&[]).unwrap();
        assert!(!config.desktop && !control && config.shore_bots);
        let path = std::env::temp_dir().join(format!("desktop-config-{}.json", std::process::id()));
        let dir = std::env::temp_dir();
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "bind":"127.0.0.1:18923", "origin":"http://127.0.0.1:18923",
                "data":dir, "client":dir, "state":dir, "max_players":85,
                "behind_proxy":false, "shore_bots":false, "bot_llm":false, "realm_seed":0
            }))
            .unwrap(),
        )
        .unwrap();
        let args = vec![
            "--desktop".into(),
            "--config".into(),
            path.to_str().unwrap().into(),
        ];
        let (config, control) = Config::load(&args).unwrap();
        assert!(config.desktop && !control && !config.shore_bots && !config.bot_llm);
        for flag in ["--dev-control", "--shore-bots", "--bot-llm"] {
            let mut invalid = args.clone();
            invalid.push(flag.into());
            assert!(Config::load(&invalid).is_err(), "{flag}");
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn desktop_rejects_proxy_public_origin_and_relative_runtime_paths() {
        let (mut config, _) = Config::load(&["--no-shore-bots".into()]).unwrap();
        config.desktop = true;
        config.validate(false).unwrap();
        config.behind_proxy = true;
        assert!(config.validate(false).is_err());
        config.behind_proxy = false;
        config.origin = "https://example.com".into();
        assert!(config.validate(false).is_err());
        config.origin = format!("http://{}", config.bind);
        config.state = "relative".into();
        assert!(config.validate(false).is_err());
    }
}

#[cfg(test)]
mod startup_backup_tests {
    use super::*;
    #[test]
    fn optional_startup_backup_is_strict_and_desktop_only() {
        let (mut config, _) = Config::load(&["--no-shore-bots".into()]).unwrap();
        assert!(config.pre_migration_backup.is_none());
        let path = std::env::temp_dir().join("startup-backup.db");
        config.pre_migration_backup = Some(path.clone());
        assert!(config.validate(false).is_err());
        config.desktop = true;
        config.validate(false).unwrap();
        for path in [
            PathBuf::from("relative.db"),
            path.join("../other.db"),
            PathBuf::from("/"),
            PathBuf::from("/bad\npath.db"),
        ] {
            config.pre_migration_backup = Some(path);
            assert!(config.validate(false).is_err());
        }
        #[derive(Deserialize)]
        struct Supplied {
            #[serde(default, deserialize_with = "supplied_backup")]
            pre_migration_backup: Option<PathBuf>,
        }
        assert!(
            serde_json::from_str::<Supplied>("{}")
                .unwrap()
                .pre_migration_backup
                .is_none()
        );
        for value in ["null", "false", "42", "{}"] {
            assert!(
                serde_json::from_str::<Supplied>(&format!("{{\"pre_migration_backup\":{value}}}"))
                    .is_err()
            );
        }
    }
}

#[cfg(test)]
mod client_generation_tests {
    use super::*;
    #[test]
    fn client_generation_is_optional_non_null_exact_lower_hex_and_desktop_only() {
        let (mut config, _) = Config::load(&["--no-shore-bots".into()]).unwrap();
        assert!(config.client_generation.is_none());
        config.client_generation = Some("a1".repeat(32));
        assert!(config.validate(false).is_err());
        config.desktop = true;
        config.validate(false).unwrap();
        for invalid in [
            "".into(),
            "a".repeat(63),
            "a".repeat(65),
            "A".repeat(64),
            "g".repeat(64),
            format!("{}/", "a".repeat(64)),
        ] {
            config.client_generation = Some(invalid);
            assert!(config.validate(false).is_err());
        }
        #[derive(Deserialize)]
        struct Supplied {
            #[serde(default, deserialize_with = "supplied_generation")]
            client_generation: Option<String>,
        }
        assert!(
            serde_json::from_str::<Supplied>("{}")
                .unwrap()
                .client_generation
                .is_none()
        );
        for value in ["null", "false", "42", "{}", "[]"] {
            assert!(
                serde_json::from_str::<Supplied>(&format!("{{\"client_generation\":{value}}}"))
                    .is_err()
            );
        }
    }
}
