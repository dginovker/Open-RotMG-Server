use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{net::SocketAddr, path::PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub bind: SocketAddr,
    pub origin: String,
    pub data: PathBuf,
    pub client: PathBuf,
    pub state: PathBuf,
    #[serde(default)]
    pub test_state: bool,
    pub max_players: usize,
    pub behind_proxy: bool,
    #[serde(default = "default_shore_bots")]
    pub shore_bots: bool,
    #[serde(default)]
    pub bot_llm: bool,
    pub realm_seed: u64,
    #[serde(default = "default_realm_count")]
    pub realm_count: usize,
}
fn default_realm_count() -> usize {
    2
}
fn default_shore_bots() -> bool {
    true
}
impl Config {
    pub fn load(args: &[String]) -> Result<(Self, bool)> {
        let mut file = None;
        let mut port = None;
        let mut control = false;
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
                    "usage: openrotmg [--config FILE] [--port PORT] [--dev-control] [--shore-bots | --no-shore-bots] [--bot-llm | --no-bot-llm] [--client DIR] [--state DIR]"
                ),
            }
        }
        let mut config = if let Some(file) = file {
            serde_json::from_slice(
                &std::fs::read(file).with_context(|| format!("read config {file}"))?,
            )?
        } else {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            Self {
                bind: "127.0.0.1:18923".parse()?,
                origin: "http://127.0.0.1:18923".into(),
                data: root.join("data"),
                client: root
                    .parent()
                    .context("workspace parent missing")?
                    .join("client"),
                state: root.join(".local"),
                test_state: false,
                max_players: 85,
                behind_proxy: false,
                shore_bots: true,
                bot_llm: false,
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
            }
        }
        if let Some(enabled) = shore_bots {
            config.shore_bots = enabled;
        }
        if let Some(enabled) = bot_llm {
            config.bot_llm = enabled;
        }
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
        ensure!(
            (2..=3).contains(&self.realm_count),
            "realm_count must be 2 or 3"
        );
        ensure!(
            self.bind.ip().is_loopback(),
            "backend bind must be loopback; terminate public TLS at the proxy"
        );
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
            !self.shore_bots || self.max_players > crate::bots::POPULATION,
            "adventuring bots require capacity for {} bots plus a human",
            crate::bots::POPULATION
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
