use super::*;
use crate::server::App;
use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response, sse},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    time::{Duration, Instant, SystemTime},
};

const THOUGHTS: usize = 50;
const VIEWERS: usize = 32;
const VIEWERS_PER_IP: usize = 2;

// Telemetry and native viewers share bot identity, but have independent admission budgets.
#[derive(Default)]
pub(crate) struct Cctv(Mutex<Registry>);
#[derive(Default)]
struct Registry {
    feeds: BTreeMap<&'static str, Feed>,
    ips: HashMap<(IpAddr, bool), usize>,
}
struct Feed {
    updated: Instant,
    frame: Value,
    session: Option<(i32, i32)>,
    thoughts: VecDeque<(u64, u128, String)>,
    reason: String,
    shots: Vec<u8>,
    viewers: usize,
}
impl Registry {
    fn feed(&mut self, name: &'static str) -> &mut Feed {
        self.feeds.entry(name).or_insert_with(|| Feed {
            updated: Instant::now(),
            frame: Value::Null,
            session: None,
            thoughts: VecDeque::new(),
            reason: String::new(),
            shots: Vec::new(),
            viewers: 0,
        })
    }
}
impl Feed {
    fn think(&mut self, text: String) {
        let seq = self.thoughts.back().map_or(1, |t| t.0 + 1);
        let at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system clock before 1970")
            .as_millis();
        self.thoughts.push_back((seq, at, text));
        if self.thoughts.len() > THOUGHTS {
            self.thoughts.pop_front();
        }
    }
}

impl Cctv {
    fn lock(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.0.lock().expect("bot CCTV registry poisoned")
    }
    pub(super) fn entered(&self, name: &'static str, player: i32, account: i32) {
        let mut registry = self.lock();
        let feed = registry.feed(name);
        feed.session = Some((player, account));
        feed.shots.clear();
        feed.frame = Value::Null;
        feed.updated = Instant::now();
    }
    pub(super) fn departed(&self, name: &'static str, player: i32, account: i32) {
        let mut registry = self.lock();
        if let Some(feed) = registry.feeds.get_mut(name)
            && feed.session == Some((player, account))
        {
            feed.session = None;
            feed.frame = Value::Null;
            feed.shots.clear();
            feed.updated = Instant::now();
        }
    }
    fn live_bots(&self) -> Vec<Value> {
        self.lock().feeds.iter().filter(|(_,f)|f.session.is_some()).map(|(name,f)|
            json!({"name":name,"age_ms":f.updated.elapsed().as_millis(),"viewers":f.viewers,"frame":f.frame})
        ).collect()
    }
    pub(crate) fn binding(&self, name: &str) -> Option<(i32, i32)> {
        self.lock().feeds.get(name).and_then(|f| f.session)
    }
    pub(super) fn decided(
        &self,
        name: &'static str,
        v: &PlayerView,
        d: &Decision,
        reason: String,
        content: &Content,
    ) {
        let shots: Vec<u8> = v
            .projectiles
            .iter()
            .filter(|p| p.owner == v.session.player)
            .map(|p| p.id)
            .collect();
        let mut registry = self.lock();
        let feed = registry.feed(name);
        let fired = shots.iter().any(|id| !feed.shots.contains(id));
        if feed.reason != reason {
            feed.think(reason.clone());
        }
        feed.frame = json!({"class":content.objects[&v.character.class].name,"level":v.character.level,
            "hp":v.character.hp,"max_hp":v.character.stats[0],"mp":v.character.mp,"max_mp":v.character.stats[1],
            "world":v.map.name,"x":v.position.x,"y":v.position.y,"aim":d.aim,"fired":fired,
            "character":v.session.character,"target":d.movement.map(|p|[p.x,p.y]),"goal":format!("{:?}",d.goal),"reason":reason});
        feed.session = Some((v.session.player, v.session.account));
        feed.shots = shots;
        feed.reason = reason;
        feed.updated = Instant::now();
    }
    pub(super) fn heard(&self, name: &'static str, own: i32, packet: &Packet) -> Result<()> {
        if packet.id != wire::TEXT {
            return Ok(());
        }
        let mut r = Reader::new(&packet.body);
        r.utf()?;
        let actor = r.i32()?;
        r.i32()?;
        r.u8()?;
        let recipient = r.utf()?;
        let text = r.utf()?;
        // Private tells stay private; only the bot's own public chat is shown.
        if actor == own && recipient.is_empty() {
            self.lock().feed(name).think(format!("said: {text}"));
        }
        Ok(())
    }
    fn update(&self, name: &'static str, seen: u64) -> (String, u64) {
        let registry = self.lock();
        let feed = &registry.feeds[name];
        let thoughts: Vec<_> = feed
            .thoughts
            .iter()
            .filter(|t| t.0 > seen)
            .map(|(seq, at, text)| json!({"seq":seq,"at":at,"text":text}))
            .collect();
        let event = json!({"age_ms":feed.updated.elapsed().as_millis(),"frame":feed.frame,
            "thoughts":thoughts});
        (
            event.to_string(),
            feed.thoughts.back().map_or(seen, |t| t.0),
        )
    }
}

// Unnamed objects show their type name, as the native client does.
fn label<'a>(o: &'a crate::world::view::VisibleObject, content: &'a Content) -> &'a str {
    if o.name.is_empty() {
        &content.objects[&o.kind].name
    } else {
        &o.name
    }
}
pub(super) fn reason(
    activity: Activity,
    d: &Decision,
    v: &PlayerView,
    content: &Content,
) -> String {
    let named = |id: &i32| {
        v.objects.iter().find(|o| o.id == *id).map_or_else(
            || format!("out-of-sight object {id}"),
            |o| label(o, content).into(),
        )
    };
    let doing = match (&d.journey, &d.goal) {
        (Some(Journey::Portal(id)), _) => format!("entering {}", named(id)),
        (Some(Journey::Nexus), _) => "returning to the Nexus".into(),
        (Some(Journey::Regroup(id)), _) => format!("teleporting to {}", named(id)),
        (None, Goal::Recover) => "low health, backing off to recover".into(),
        (None, Goal::Remedy(id)) => format!("under threat, heading for {}", named(id)),
        (None, Goal::Dodge) => "under threat, dodging".into(),
        (None, Goal::Fight(id)) => format!("fighting {}", named(id)),
        (None, Goal::Hunt(id)) => format!("hunting quest {}", named(id)),
        (None, Goal::Venture) => "heading inland for tougher enemies".into(),
        (None, Goal::Bank(_)) => "storing loot in the vault".into(),
        (None, Goal::Break(id)) => format!("breaking {}", named(id)),
        (None, Goal::Loot(id)) => format!("looting {}", named(id)),
        (None, Goal::Follow(id)) => format!("following {}", named(id)),
        (None, Goal::Gather(_)) => "regrouping with nearby players".into(),
        (None, Goal::Portal(id)) => format!("walking to {}", named(id)),
        (None, Goal::Revisit(..)) => "returning to remembered loot".into(),
        (None, Goal::Survey(..)) => "heading to a realm event".into(),
        (None, Goal::Explore(..)) => "exploring".into(),
        (None, Goal::Linger) => "keeping a personal Nexus spot".into(),
        (None, Goal::NexusNoRoom) => "waiting: no clear reachable Nexus spot".into(),
        (None, Goal::Wait) => "waiting".into(),
    };
    format!("{activity}: {doing}")
}

pub(crate) struct Viewer {
    cctv: Arc<Cctv>,
    name: &'static str,
    ip: IpAddr,
    native: bool,
}
impl Viewer {
    pub(crate) fn join(
        cctv: &Arc<Cctv>,
        name: &str,
        ip: IpAddr,
        native: bool,
    ) -> Result<Self, (StatusCode, String)> {
        let mut registry = cctv.lock();
        let Some((&name, _)) = registry
            .feeds
            .get_key_value(name)
            .filter(|(_, f)| f.session.is_some())
        else {
            return Err((StatusCode::NOT_FOUND, format!("no live bot named {name}")));
        };
        if registry
            .ips
            .iter()
            .filter(|((_, kind), _)| *kind == native)
            .map(|(_, count)| count)
            .sum::<usize>()
            >= VIEWERS
        {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                format!("all {VIEWERS} bot viewer slots are in use; retry later"),
            ));
        }
        let mine = registry.ips.entry((ip, native)).or_default();
        if *mine >= VIEWERS_PER_IP {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                format!("at most {VIEWERS_PER_IP} bot streams per address; close another tab"),
            ));
        }
        *mine += 1;
        registry.feed(name).viewers += 1;
        Ok(Self {
            cctv: cctv.clone(),
            name,
            ip,
            native,
        })
    }
}
impl Drop for Viewer {
    fn drop(&mut self) {
        let mut registry = self.cctv.lock();
        registry.feed(self.name).viewers -= 1;
        let mine = registry
            .ips
            .get_mut(&(self.ip, self.native))
            .expect("bot viewer address missing");
        *mine -= 1;
        if *mine == 0 {
            registry.ips.remove(&(self.ip, self.native));
        }
        tracing::info!(bot = self.name, "bot CCTV viewer left");
    }
}

pub(crate) async fn list(State(a): State<App>) -> Response {
    let bots = a.cctv.live_bots();
    let llm = dialogue::health(a.dialogue.as_deref());
    (
        [("cache-control", "no-store")],
        Json(json!({"bots":bots,"llm":llm})),
    )
        .into_response()
}

pub(crate) async fn stream(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    let joined = crate::server::request_ip(&a, &headers, peer)
        .map_err(|e| (StatusCode::FORBIDDEN, e.to_string()))
        .and_then(|ip| Viewer::join(&a.cctv, &name, ip, false));
    let viewer = match joined {
        Ok(viewer) => viewer,
        Err((status, error)) => {
            tracing::warn!(bot = name, %status, error, "bot CCTV viewer rejected");
            return (status, Json(json!({"error":error}))).into_response();
        }
    };
    tracing::info!(bot = viewer.name, "bot CCTV viewer joined");
    let ticks = tokio::time::interval(Duration::from_millis(200));
    let state = (viewer, 0, ticks, a.shutdown.subscribe());
    let events = futures_util::stream::unfold(
        state,
        |(viewer, seen, mut ticks, mut shutdown)| async move {
            // Open streams would otherwise hold graceful HTTP shutdown past its deadline.
            tokio::select! {
                _ = ticks.tick() => {},
                _ = shutdown.wait_for(|stop| *stop) => return None,
            }
            viewer.cctv.binding(viewer.name)?;
            let (event, seen) = viewer.cctv.update(viewer.name, seen);
            let event = sse::Event::default().data(event);
            Some((
                Ok::<_, std::convert::Infallible>(event),
                (viewer, seen, ticks, shutdown),
            ))
        },
    );
    ([("cache-control", "no-store")], sse::Sse::new(events)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewers_are_bounded_and_released() {
        let cctv = Arc::new(Cctv::default());
        cctv.entered("Triceps", 7, 1);
        let refused = |name, ip| Viewer::join(&cctv, name, ip, false).err().unwrap().0;
        let ip = IpAddr::from([192, 0, 2, 1]);
        assert_eq!(refused("Nobody", ip), StatusCode::NOT_FOUND);
        let pair = [0, 1].map(|_| Viewer::join(&cctv, "Triceps", ip, false).unwrap());
        assert_eq!(refused("Triceps", ip), StatusCode::TOO_MANY_REQUESTS);
        drop(pair);
        assert!(cctv.lock().ips.is_empty() && cctv.lock().feeds["Triceps"].viewers == 0);
        let full: Vec<_> = (0..VIEWERS as u8)
            .map(|i| Viewer::join(&cctv, "Triceps", IpAddr::from([10, 0, 0, i]), false).unwrap())
            .collect();
        assert_eq!(refused("Triceps", ip), StatusCode::SERVICE_UNAVAILABLE);
        let native = Viewer::join(&cctv, "Triceps", ip, true).unwrap();
        drop(native);
        drop(full);
        assert!(cctv.lock().ips.is_empty());
    }

    #[test]
    fn departures_hide_only_matching_sessions_and_preserve_viewer_leases() {
        let cctv = Arc::new(Cctv::default());
        let ip = IpAddr::from([192, 0, 2, 1]);
        cctv.entered("Triceps", 7, 1);
        let viewer = Viewer::join(&cctv, "Triceps", ip, false).unwrap();
        cctv.departed("Triceps", 8, 1);
        assert_eq!(cctv.binding("Triceps"), Some((7, 1)));
        assert_eq!(cctv.live_bots().len(), 1);
        cctv.departed("Triceps", 7, 1);
        assert!(cctv.binding("Triceps").is_none() && cctv.live_bots().is_empty());
        assert_eq!(
            Viewer::join(&cctv, "Triceps", ip, true).err().unwrap().0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(cctv.lock().feeds["Triceps"].viewers, 1);
        cctv.entered("Triceps", 9, 2);
        cctv.departed("Triceps", 7, 1);
        assert_eq!(cctv.binding("Triceps"), Some((9, 2)));
        assert_eq!(cctv.live_bots()[0]["name"], "Triceps");
        drop(viewer);
        assert!(cctv.lock().ips.is_empty());
        assert_eq!(cctv.lock().feeds["Triceps"].viewers, 0);
    }

    #[test]
    fn only_own_public_chat_becomes_a_thought() {
        let cctv = Cctv::default();
        let text = |actor, recipient: &str, text: &str| {
            let mut w = wire::Writer::new();
            w.utf("Triceps")
                .i32(actor)
                .i32(-1)
                .u8(0)
                .utf(recipient)
                .utf(text)
                .utf("");
            Packet {
                id: wire::TEXT,
                body: w.0,
            }
        };
        for packet in [
            text(7, "", "tp gods"),
            text(7, "Alice", "secret"),
            text(8, "", "hi"),
        ] {
            cctv.heard("Triceps", 7, &packet).unwrap();
        }
        let (event, seen) = cctv.update("Triceps", 0);
        assert_eq!(seen, 1);
        let event: Value = serde_json::from_str(&event).unwrap();
        assert_eq!(event["thoughts"][0]["text"], "said: tp gods");
        assert_eq!(event["thoughts"].as_array().unwrap().len(), 1);
    }
}
