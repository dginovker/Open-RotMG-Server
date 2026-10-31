use super::*;
use axum::extract::Path;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, VecDeque},
    time::Instant,
};

pub struct Diagnostics {
    root: PathBuf,
    pub binary_sha256: String,
    pub(super) source_revision: Option<String>,
    browsers: Mutex<HashMap<String, std::sync::Weak<Mutex<Browser>>>>,
    files: Mutex<Reports>,
    drops: Mutex<HashMap<i32, Value>>,
    anonymous_reports: Mutex<HashMap<IpAddr, Instant>>,
}
// Players report after reconnecting, so a dropped connection must outlive it and any deploy restart: https://github.com/OpenRotMG/Open-RotMG-Server/issues/469
const DROP_WINDOW_MS: u64 = 600_000;
const DROPS: &str = "recent-drops.state";
fn unix_ms() -> u64 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    u64::try_from(now.expect("system clock predates Unix epoch").as_millis())
        .expect("unix time fits u64")
}
fn drop_age(drop: &Value) -> u64 {
    unix_ms().saturating_sub(
        drop["at_unix_ms"]
            .as_u64()
            .expect("validated drop timestamp"),
    )
}
#[derive(Default)]
struct Reports {
    pending: BTreeSet<String>,
}
fn note_progress(report: &Value) -> Result<(usize, usize)> {
    let Some(notes) = report.get("notes") else {
        return Ok((0, 0));
    };
    let total = notes.as_array().context("invalid stored bug notes")?.len();
    let through = usize::try_from(
        report["delivery"]["notes_through"]
            .as_u64()
            .context("invalid bug note progress")?,
    )?;
    ensure!(through <= total && total <= 32, "invalid bug note progress");
    Ok((through, total))
}
#[derive(Default)]
pub struct Browser {
    samples: VecDeque<(Instant, BrowserSample)>,
    pub reports: Option<mpsc::Sender<WebReport>>,
    last_report: Option<Instant>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserSample {
    build: String,
    renderer: Option<String>,
    renderer_status: String,
    width: u32,
    height: u32,
    pixel_ratio: f64,
    hidden: bool,
    hidden_during_interval: bool,
    frames: u32,
    max_frame_gap_ms: f64,
    long_tasks: Option<u32>,
    long_task_ms: Option<f64>,
    elapsed_ms: f64,
}
impl BrowserSample {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.build.len() <= 256
                && self.renderer.as_ref().is_none_or(|s| s.len() <= 512)
                && self.renderer_status.len() <= 80,
            "diagnostic string too long"
        );
        ensure!(
            self.width <= 32768 && self.height <= 32768 && self.frames <= 100_000,
            "invalid display/frame count"
        );
        for n in [self.pixel_ratio, self.max_frame_gap_ms, self.elapsed_ms] {
            ensure!(
                n.is_finite() && (0.0..=86_400_000.0).contains(&n),
                "invalid browser timing"
            );
        }
        ensure!(
            self.long_task_ms
                .is_none_or(|n| n.is_finite() && (0.0..=86_400_000.0).contains(&n)),
            "invalid long task timing"
        );
        Ok(())
    }
}
impl Diagnostics {
    pub fn open(root: PathBuf) -> Result<Self> {
        use sha2::{Digest, Sha256};
        std::fs::create_dir_all(&root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
        }
        let mut pending = Reports::default();
        for entry in std::fs::read_dir(&root)? {
            let path = entry?.path();
            if path.extension().is_some_and(|v| v == "json") {
                let report: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
                let id = report["id"].as_str().context("missing stored bug ID")?;
                ensure!(
                    valid_id(id) && path.file_stem().is_some_and(|s| s == id),
                    "invalid stored bug identity"
                );
                match report["delivery"]["status"].as_str() {
                    Some("pending") => {
                        pending.pending.insert(id.into());
                    }
                    Some("delivered") => {}
                    _ => bail!("invalid stored bug delivery state"),
                }
                note_progress(&report)?;
            }
        }
        let drops: HashMap<i32, Value> = match std::fs::read(root.join(DROPS)) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).context("invalid stored disconnect diagnostics")?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(e.into()),
        };
        ensure!(
            drops.values().all(|d| d["at_unix_ms"].is_u64()),
            "stored disconnect diagnostics lack a timestamp"
        );
        let executable = std::env::current_exe()?;
        let source_revision = match std::fs::read_to_string(executable.with_file_name("REVISION")) {
            Ok(value) => {
                let value = value.trim().to_string();
                ensure!(
                    value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit()),
                    "invalid release REVISION"
                );
                Some(value)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            root,
            source_revision,
            binary_sha256: format!("{:x}", Sha256::digest(std::fs::read(executable)?)),
            browsers: Mutex::default(),
            files: Mutex::new(pending),
            drops: Mutex::new(drops),
            anonymous_reports: Mutex::default(),
        })
    }
    pub(super) fn previous_drop(&self, account: i32) -> Value {
        let drops = self.drops.lock().expect("drop diagnostics poisoned");
        let drop = drops.get(&account).filter(|d| drop_age(d) < DROP_WINDOW_MS);
        drop.map_or(Value::Null, |d| {
            let mut d = d.clone();
            d["age_ms"] = json!(drop_age(&d));
            d
        })
    }
    fn note_drop(&self, account: i32, drop: Value) -> Result<()> {
        let mut drops = self.drops.lock().expect("drop diagnostics poisoned");
        drops.retain(|_, d| drop_age(d) < DROP_WINDOW_MS);
        drops.insert(account, drop);
        self.write_file(DROPS, &json!(*drops))
    }
    pub fn browser(&self, key: &str) -> Result<Arc<Mutex<Browser>>> {
        ensure!(
            key.len() == 32 && key.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid diagnostic session"
        );
        let mut browsers = self.browsers.lock().expect("diagnostics poisoned");
        browsers.retain(|_, v| v.strong_count() > 0);
        if let Some(browser) = browsers.get(key).and_then(|v| v.upgrade()) {
            return Ok(browser);
        }
        ensure!(browsers.len() < 256, "diagnostic session limit reached");
        let browser = Arc::new(Mutex::new(Browser::default()));
        browsers.insert(key.into(), Arc::downgrade(&browser));
        Ok(browser)
    }
    fn save(&self, report: &Value) -> Result<()> {
        let mut pending = self.files.lock().expect("diagnostics poisoned");
        ensure!(
            std::fs::read_dir(&self.root)?.count() < 10_000,
            "Bug report storage full; notify an administrator."
        );
        self.write(report)?;
        pending
            .pending
            .insert(report["id"].as_str().context("missing report ID")?.into());
        Ok(())
    }
    fn next(&self) -> Result<Option<Value>> {
        let mut files = self.files.lock().expect("diagnostics poisoned");
        while let Some(id) = files.pending.first().cloned() {
            let mut report: Value =
                serde_json::from_slice(&std::fs::read(self.root.join(format!("{id}.json")))?)?;
            if report["delivery"]["status"] == "pending" {
                let (through, total) = note_progress(&report)?;
                if through < total && report["delivery"]["note_batch"].is_null() {
                    // A retry must identify the same comment even when more details arrive during publication.
                    report["delivery"]["note_batch"] = json!({"from":through+1,"through":total});
                    self.write(&report)?;
                }
                report
                    .as_object_mut()
                    .expect("report object")
                    .remove("screenshot");
                return Ok(Some(report));
            }
            ensure!(
                report["delivery"]["status"] == "delivered",
                "invalid stored bug delivery state"
            );
            files.pending.remove(&id);
        }
        Ok(None)
    }
    fn acknowledge(&self, id: &str, receipt: &Receipt) -> Result<Option<(i32, i32, String)>> {
        ensure!(valid_id(id), "invalid bug report ID");
        receipt.validate()?;
        let mut files = self.files.lock().expect("diagnostics poisoned");
        let mut report: Value =
            serde_json::from_slice(&std::fs::read(self.root.join(format!("{id}.json")))?)?;
        let notify =
            receipt.target == "issue_number" && report["delivery"]["issue_number"].is_null();
        if receipt.target == "notes" {
            let through = receipt.through.context("missing bug note receipt range")?;
            let (previous, _) = note_progress(&report)?;
            if through == previous {
                ensure!(
                    report["delivery"]["note_comment_id"] == receipt.id,
                    "conflicting bug note receipt"
                );
                return Ok(None);
            }
            let batch = &report["delivery"]["note_batch"];
            ensure!(
                batch["from"] == previous + 1 && batch["through"] == through,
                "conflicting bug note receipt range"
            );
            ensure!(
                through > previous && through <= note_progress(&report)?.1,
                "invalid bug note receipt range"
            );
            report["delivery"]["notes_through"] = json!(through);
            report["delivery"]["note_comment_id"] = json!(receipt.id);
            report["delivery"]["note_batch"] = Value::Null;
        } else {
            ensure!(
                report["delivery"][&receipt.target].is_null()
                    || report["delivery"][&receipt.target] == receipt.id,
                "conflicting delivery receipt"
            );
            report["delivery"][&receipt.target] = json!(receipt.id);
        }
        let (through, total) = note_progress(&report)?;
        let complete = through == total
            && ["public_message_id", "issue_number"]
                .iter()
                .all(|key| report["delivery"][*key].is_string());
        report["delivery"]["status"] = json!(if complete { "delivered" } else { "pending" });
        self.write(&report)?;
        if complete {
            files.pending.remove(id);
        } else {
            files.pending.insert(id.into());
        }
        tracing::info!(report_id=%id, target=%receipt.target, external_id=%receipt.id, notes_through=through, "bug report delivery acknowledged");
        Ok(if notify {
            report_owner(&report)?.map(|(account, player)| (player, account, receipt.id.clone()))
        } else {
            None
        })
    }
    fn write(&self, report: &Value) -> Result<()> {
        let id = report["id"].as_str().context("missing report ID")?;
        ensure!(valid_id(id), "invalid report ID");
        self.write_file(&format!("{id}.json"), report)
    }
    fn write_file(&self, name: &str, value: &Value) -> Result<()> {
        use std::io::Write;
        let path = self.root.join(name);
        let temp = self
            .root
            .join(format!("{name}-{:016x}.tmp", rand::random::<u64>()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(&serde_json::to_vec_pretty(value)?)?;
        file.sync_all()?;
        std::fs::rename(temp, path)?;
        std::fs::File::open(&self.root)?.sync_all()?;
        Ok(())
    }
}
fn valid_id(id: &str) -> bool {
    id.len() == 24 && id.bytes().all(|b| b.is_ascii_hexdigit())
}
struct ConnectionId(u64);
impl Default for ConnectionId {
    fn default() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}
pub struct CommandSample {
    pub packet: u8,
    pub queued: std::time::Duration,
    pub handling: std::time::Duration,
    pub queue_before: usize,
    pub queue_after: usize,
    pub failed: bool,
    pub requests: crate::performance::Requests,
}
#[derive(Default)]
pub struct ConnectionDiagnostics {
    connection_id: ConnectionId,
    pub world: Option<i32>,
    slow_commands: VecDeque<(Instant, Value)>,
    slowest_handler: Option<(Instant, Value)>,
    slowest_queue: Option<(Instant, Value)>,
    pub user_agent: Option<String>,
    pub browser: Option<Arc<Mutex<Browser>>>,
    pending: BTreeMap<i32, Instant>,
    replies: VecDeque<(Instant, f64)>,
    pub packets_in: u64,
    pub packets_out: u64,
    pub max_send_ms: f64,
    pub max_handler_ms: f64,
    pub max_input_queue_ms: f64,
    pub queue_high_water: usize,
    pub expired_pings: u64,
    pub unmatched_pongs: u64,
    started: Option<Instant>,
    last_processed: Option<(Instant, u8)>,
    last_move: Option<Instant>,
    last_update_ack: Option<Instant>,
    pub last_socket_send: Option<Instant>,
    native_ticks_sent: u64,
    last_native_tick_send: Option<Instant>,
    max_native_tick_send_gap: Option<(std::time::Duration, u64)>,
    native_tick_stalls: u64,
}
pub(super) const UPDATE_STALL: std::time::Duration = std::time::Duration::from_millis(250);
impl ConnectionDiagnostics {
    // Fast writes do not rule out long pauses between them (#1250).
    pub fn queued_packet_sent(&mut self, packet: u8, now: Instant) -> Option<std::time::Duration> {
        self.last_socket_send = Some(now);
        if packet != 62 {
            return None;
        }
        self.native_ticks_sent += 1;
        let gap = now.duration_since(self.last_native_tick_send.replace(now)?);
        self.max_native_tick_send_gap = self.max_native_tick_send_gap.max(Some((gap, unix_ms())));
        (gap > UPDATE_STALL).then(|| {
            self.native_tick_stalls += 1;
            gap
        })
    }
    pub fn handled(&mut self, sample: CommandSample) {
        let queue_ms = sample.queued.as_secs_f64() * 1000.;
        let handler_ms = sample.handling.as_secs_f64() * 1000.;
        let max_handler = handler_ms > self.max_handler_ms;
        let max_queue = queue_ms > self.max_input_queue_ms;
        let slow = handler_ms >= 50. || queue_ms >= 50. || sample.failed;
        self.max_handler_ms = self.max_handler_ms.max(handler_ms);
        self.max_input_queue_ms = self.max_input_queue_ms.max(queue_ms);
        if !slow && !max_handler && !max_queue {
            return;
        }
        let at = Instant::now();
        let event = json!({"completed_unix_ms":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("system clock predates Unix epoch").as_millis(),
            "packet":sample.packet,"world_id":self.world,"queue_wait_ms":queue_ms,"handler_wall_ms":handler_ms,
            "input_queue_before":sample.queue_before,"input_queue_after":sample.queue_after,"failed":sample.failed,"world_requests":sample.requests.snapshot()});
        if max_handler {
            self.slowest_handler = Some((at, event.clone()));
        }
        if max_queue {
            self.slowest_queue = Some((at, event.clone()));
        }
        if slow {
            self.slow_commands.push_back((at, event));
            while self.slow_commands.len() > 8 {
                self.slow_commands.pop_front();
            }
        }
    }
    pub fn processed(&mut self, packet: u8) {
        let now = Instant::now();
        self.last_processed = Some((now, packet));
        if packet == wire::MOVE {
            self.last_move = Some(now);
        }
        if packet == wire::UPDATE_ACK {
            self.last_update_ack = Some(now);
        }
    }
    pub fn sent(&mut self, serial: i32) {
        let now = Instant::now();
        self.started.get_or_insert(now);
        let count = self.pending.len();
        self.pending.retain(|_, t| t.elapsed().as_secs() < 120);
        self.expired_pings += (count - self.pending.len()) as u64;
        self.pending.insert(serial, now);
    }
    pub fn pong(&mut self, serial: i32) {
        if let Some(sent) = self.pending.remove(&serial) {
            self.replies
                .push_back((Instant::now(), sent.elapsed().as_secs_f64() * 1000.0));
            while self
                .replies
                .front()
                .is_some_and(|(t, _)| t.elapsed().as_secs() > 120)
            {
                self.replies.pop_front();
            }
        } else {
            self.unmatched_pongs += 1;
        }
    }
    fn snapshot(&self) -> Value {
        let samples: Vec<_> = self
            .replies
            .iter()
            .filter(|(t, _)| t.elapsed().as_secs() <= 120)
            .map(|(t, ms)| json!({"age_ms": t.elapsed().as_millis(), "rtt_ms": ms}))
            .collect();
        let browser = self.browser.as_ref().map(|b| {
            b.lock()
                .expect("browser poisoned")
                .samples
                .iter()
                .filter(|(t, _)| t.elapsed().as_secs() <= 120)
                .map(|(t, sample)| json!({"age_ms": t.elapsed().as_millis(), "sample": sample}))
                .collect::<Vec<_>>()
        });
        let observation = |(at, event): &(Instant, Value)| json!({"completed_age_ms":at.elapsed().as_secs_f64()*1000.,"event":event});
        json!({"connection_id":self.connection_id.0,"process_id":std::process::id(),"world_id":self.world,
            "slow_commands":self.slow_commands.iter().map(observation).collect::<Vec<_>>(),
            "slowest_handler":self.slowest_handler.as_ref().map(observation),"slowest_input_queue":self.slowest_queue.as_ref().map(observation),
            "user_agent": self.user_agent, "browser_samples": browser, "rtt_samples": samples,
            "last_move_received_age_ms":self.last_move.map(|at|at.elapsed().as_secs_f64()*1000.),
            "last_update_ack_received_age_ms":self.last_update_ack.map(|at|at.elapsed().as_secs_f64()*1000.),
            "last_processed_packet":self.last_processed.map(|(_,id)|id),
            "last_processed_age_ms":self.last_processed.map(|(at,_)|at.elapsed().as_secs_f64()*1000.),
            "last_queued_packet_send_age_ms":self.last_socket_send.map(|at|at.elapsed().as_secs_f64()*1000.),
            "last_pong_age_ms":self.replies.back().map(|(at,_)|at.elapsed().as_secs_f64()*1000.),
            "pending_ping_ages_ms": self.pending.values().map(|t| t.elapsed().as_millis()).collect::<Vec<_>>(),
            "expired_pings_session": self.expired_pings, "unmatched_pongs_session": self.unmatched_pongs,
            "game_commands_in_session": self.packets_in, "queued_packets_out_session": self.packets_out,
            "native_ticks_sent_session":self.native_ticks_sent,"native_tick_stalls_session":self.native_tick_stalls,
            "max_native_tick_send_gap_ms_session":self.max_native_tick_send_gap.map(|(gap,_)|gap.as_secs_f64()*1000.),
            "max_native_tick_send_gap_end_unix_ms":self.max_native_tick_send_gap.map(|(_,at)|at),
            "max_socket_send_ms_session": self.max_send_ms, "max_handler_ms_session": self.max_handler_ms, "max_input_queue_ms_session": self.max_input_queue_ms,
            "observed_outgoing_queue_high_water_session": self.queue_high_water,
            "ping_observation_age_ms": self.started.map(|t| t.elapsed().as_millis())})
    }
}
fn summary(values: impl Iterator<Item = f64>) -> Value {
    let mut values: Vec<_> = values.collect();
    values.sort_by(f64::total_cmp);
    let percentile = |p: f64| {
        values
            .get(((values.len() as f64 * p).ceil() as usize).saturating_sub(1))
            .copied()
    };
    json!({"samples": values.len(), "median": percentile(0.5), "p95": percentile(0.95), "max": values.last()})
}
fn command_observation(value: &Value) -> Value {
    if value.is_null() {
        return Value::Null;
    }
    let e = &value["event"];
    let r = &e["world_requests"];
    json!({"completed_age_ms":value["completed_age_ms"].as_f64(),"event":{
        "completed_unix_ms":e["completed_unix_ms"].as_u64(),"packet":e["packet"].as_u64(),"world_id":e["world_id"].as_i64(),
        "queue_wait_ms":e["queue_wait_ms"].as_f64(),"handler_wall_ms":e["handler_wall_ms"].as_f64(),
        "input_queue_before":e["input_queue_before"].as_u64(),"input_queue_after":e["input_queue_after"].as_u64(),"failed":e["failed"].as_bool(),
        "world_requests":{"calls":r["calls"].as_u64(),"completed":r["completed"].as_u64(),"send_wait_sum_ms":r["send_wait_sum_ms"].as_f64(),
            "queue_wait_sum_ms":r["queue_wait_sum_ms"].as_f64(),"work_wall_sum_ms":r["work_wall_sum_ms"].as_f64(),"work_cpu_sum_ms":r["work_cpu_sum_ms"].as_f64(),"round_trip_sum_ms":r["round_trip_sum_ms"].as_f64()}}})
}
fn lag_diagnostics(value: &Value, browser: &[&Value]) -> Value {
    let mut lag = crate::world::native_lag::public_snapshot(value);
    if lag.is_null() {
        return lag;
    }
    let visibility = |v: &Value| {
        let (Some(end), Some(gap)) = (v["max_gap_end_age_ms"].as_f64(), v["max_gap_ms"].as_f64())
        else {
            return "unavailable";
        };
        let intervals: Vec<_> = browser
            .iter()
            .filter_map(|v| {
                Some((
                    v["age_ms"].as_f64()?,
                    v["sample"]["elapsed_ms"].as_f64()?,
                    v["sample"]["hidden_during_interval"].as_bool()?,
                ))
            })
            .collect();
        // Browser sample receipt times are visibility hints, not synchronized client clocks.
        if intervals
            .iter()
            .any(|(age, ms, hidden)| *hidden && *age <= end + gap && age + ms >= end)
        {
            "hidden_or_mixed_sample"
        } else if intervals
            .iter()
            .any(|(age, ms, hidden)| !hidden && *age <= end && age + ms >= end + gap)
        {
            "visible_sample"
        } else {
            "unavailable"
        }
    };
    lag["max_gap_visibility_hint"] = json!(visibility(&lag));
    if let Some(rows) = lag["history"].as_array_mut() {
        for row in rows {
            row["max_gap_visibility_hint"] = json!(visibility(row));
        }
    }
    lag["expected_ack_interval_ms"] = json!(100);
    lag["bucket_seconds"] = json!(5);
    lag
}
fn public_diagnostics(report: &Value) -> Value {
    let connection = &report["connection"];
    let rtt = connection["rtt_samples"].as_array().expect("RTT samples");
    let ticks = report["context"]["recent_ticks"].as_array();
    let browser: Vec<_> = connection["browser_samples"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    let visible: Vec<_> = browser
        .iter()
        .filter(|v| v["sample"]["hidden_during_interval"] == false)
        .collect();
    let versions = browser
        .last()
        .and_then(|v| v["sample"]["build"].as_str())
        .and_then(|s| serde_json::from_str::<Value>(s).ok());
    let revision = |key: &str, length: usize| {
        versions
            .as_ref()
            .and_then(|v| v[key].as_str())
            .filter(|s| s.len() == length && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .map(str::to_owned)
    };
    let mut history = Vec::new();
    for bucket in 0..24 {
        let within = |v: &&Value| v["age_ms"].as_u64().is_some_and(|age| age / 5000 == bucket);
        let tick_values: Vec<_> = ticks.into_iter().flatten().filter(within).collect();
        let ping_values: Vec<_> = rtt.iter().filter(within).collect();
        if !tick_values.is_empty() || !ping_values.is_empty() {
            history.push(json!({"seconds_before_report": [bucket * 5, (bucket + 1) * 5],
                "game_rtt_ms": summary(ping_values.iter().filter_map(|v| v["rtt_ms"].as_f64())),
                "tick_ms": ticks.map(|_|summary(tick_values.iter().filter_map(|v| v["execution_us"].as_f64().map(|n| n / 1000.0)))),
                "scheduling_delay_ms": ticks.map(|_|summary(tick_values.iter().filter_map(|v| v["scheduling_lateness_us"].as_f64().map(|n| n / 1000.0))))}));
        }
    }
    let browser_history: Vec<_> = browser.iter().map(|v| {
        let s = &v["sample"];
        json!({"age_ms": v["age_ms"], "interval_ms": s["elapsed_ms"], "hidden_during_interval": s["hidden_during_interval"],
            "animation_frames": s["frames"], "max_frame_gap_ms": s["max_frame_gap_ms"], "long_tasks": s["long_tasks"], "long_task_ms": s["long_task_ms"],
            "canvas_width": s["width"], "canvas_height": s["height"], "pixel_ratio": s["pixel_ratio"]})
    }).collect();
    let ua = connection["user_agent"].as_str().unwrap_or("");
    let browser_family = if ua.contains("Firefox/") {
        Some("Firefox")
    } else if ua.contains("Edg/") {
        Some("Edge")
    } else if ua.contains("Chrome/") {
        Some("Chrome")
    } else if ua.contains("Safari/") {
        Some("Safari")
    } else {
        None
    };
    let os_family = if ua.contains("Android") {
        Some("Android")
    } else if ua.contains("iPhone") || ua.contains("iPad") {
        Some("iOS")
    } else if ua.contains("Windows") {
        Some("Windows")
    } else if ua.contains("Macintosh") {
        Some("macOS")
    } else if ua.contains("Linux") {
        Some("Linux")
    } else {
        None
    };
    json!({"schema": 2, "reported_at_unix_ms": report["created_at_unix_ms"],
        "versions": {"server": report["server_source_revision"], "server_binary_sha256": report["server_binary_sha256"], "protocol": report["protocol_build"],
            "website": revision("website", 40), "client": revision("client", 40), "runtime": revision("runtime", 16)},
        "environment": {"browser": browser_family, "os": os_family},
        "game_rtt_ms": summary(rtt.iter().filter_map(|v| v["rtt_ms"].as_f64())),
        "world_context_status":if ticks.is_some() {"available"}else if report["context"].is_null() {"unavailable"}else{"invalid"},
        "tick_ms": ticks.map(|t|summary(t.iter().filter_map(|v| v["execution_us"].as_f64().map(|n| n / 1000.0)))),
        "scheduling_delay_ms": ticks.map(|t|summary(t.iter().filter_map(|v| v["scheduling_lateness_us"].as_f64().map(|n| n / 1000.0)))),
        "visible_frame_gap_ms": summary(visible.iter().filter_map(|v| v["sample"]["max_frame_gap_ms"].as_f64())),
        "native_lag":lag_diagnostics(&report["context"]["native_lag"], &browser),
        "native_acknowledgements":{
            "weapon_tick_records":report["context"]["native_acknowledgements"]["weapon_tick_records"],"goto":report["context"]["native_acknowledgements"]["goto"],
            "shoot":report["context"]["native_acknowledgements"]["shoot"],"oldest_goto_ms":report["context"]["native_acknowledgements"]["oldest_goto_ms"],
            "oldest_shoot_pause_excluding_ms":report["context"]["native_acknowledgements"]["oldest_shoot_pause_excluding_ms"],"last_update_ack_received_age_ms":connection["last_update_ack_received_age_ms"]},

        "connection": {"connection_id":connection["connection_id"].as_u64(),"process_id":connection["process_id"].as_u64(),"world_id":connection["world_id"].as_i64(),
            "slow_commands":connection["slow_commands"].as_array().map(|a|a.iter().rev().take(8).rev().map(command_observation).collect::<Vec<_>>()),
            "slowest_handler":command_observation(&connection["slowest_handler"]),"slowest_input_queue":command_observation(&connection["slowest_input_queue"]),
            "last_move_received_age_ms":connection["last_move_received_age_ms"],"last_processed_packet":connection["last_processed_packet"],"last_processed_age_ms":connection["last_processed_age_ms"],
            "last_queued_packet_send_age_ms":connection["last_queued_packet_send_age_ms"],"last_pong_age_ms":connection["last_pong_age_ms"],
            "native_ticks_sent":connection["native_ticks_sent_session"].as_u64(),"native_tick_stalls":connection["native_tick_stalls_session"].as_u64(),
            "max_native_tick_send_gap_ms":connection["max_native_tick_send_gap_ms_session"].as_f64(),
            "max_native_tick_send_gap_end_unix_ms":connection["max_native_tick_send_gap_end_unix_ms"].as_u64(),
            "max_socket_send_ms": connection["max_socket_send_ms_session"], "max_handler_ms": connection["max_handler_ms_session"],
            "max_input_queue_ms": connection["max_input_queue_ms_session"], "outgoing_queue_high_water": connection["observed_outgoing_queue_high_water_session"],
            "expired_pings": connection["expired_pings_session"], "pending_ping_ages_ms": connection["pending_ping_ages_ms"]},
        "capture_window_seconds": 120,
        "population": {"world_players": report["context"]["world_players"], "world_entities": report["context"]["world_entities"], "world_projectiles": report["context"]["world_projectiles"],
            "total_players": report["context"]["total_players"], "total_worlds": report["context"]["total_worlds"]},
        "history": history, "browser_history": browser_history,
        "units": "Timings in milliseconds; history uses five-second buckets. RTT includes game/client/server processing. Animation-frame gaps are not game FPS. Null means unavailable. Native tick send gaps cover successful application socket writes, not client receipt; the first tick has no gap and the maximum covers the connection session. Command handler times include awaits/descheduling; world-request sums may overlap and are not exclusive handler CPU. native_lag covers up to 120 seconds in five-second buckets of the current World residency; transfer resets it. MOVE tick-ACK ingress gaps include server update cadence, network and client processing, not just network RTT. First ACK has no gap; repeated/regressed tick IDs are counted separately. Visibility hints use overlapping browser samples and never suppress gaps. World performance native_lag windows overlap: do not sum them. Other correction reasons remain in the rejection logs."})
}
pub(super) async fn disconnect(
    a: &App,
    player: i32,
    dropped_account: Option<i32>,
    connection: &ConnectionDiagnostics,
    transport: Value,
) {
    // Capture before departure removes the player; never create a public bug report or gold reward.
    let mut report = json!({"created_at_unix_ms":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock before epoch").as_millis(),
        "server_source_revision":a.diagnostics.source_revision,"server_binary_sha256":a.diagnostics.binary_sha256,
        "protocol_build":"123.5.1","connection":connection.snapshot()});
    let started = Instant::now();
    let context = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        a.world.directory(move |d| d.bug_context(player)),
    )
    .await;
    let context_request_ms = started.elapsed().as_secs_f64() * 1000.;
    match context {
        Ok(Ok(context)) => {
            report["context"] = context;
            tracing::info!(player,transport=%transport,context_request_ms,diagnostics=%public_diagnostics(&report),"game disconnect diagnostics");
        }
        Ok(Err(error)) => {
            tracing::warn!(player,transport=%transport,context_request_ms,%error,diagnostics=%public_diagnostics(&report),"game disconnect diagnostics unavailable")
        }
        Err(error) => {
            tracing::warn!(player,transport=%transport,context_request_ms,%error,diagnostics=%public_diagnostics(&report),"game disconnect diagnostics deadline exceeded")
        }
    }
    if let Some(account) = dropped_account {
        let drop = json!({"at_unix_ms": unix_ms(), "transport": transport, "diagnostics": public_diagnostics(&report)});
        let diagnostics = a.diagnostics.clone();
        match tokio::task::spawn_blocking(move || diagnostics.note_drop(account, drop)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::error!(account, %error, "disconnect diagnostics not saved"),
            Err(error) => tracing::error!(account, %error, "disconnect diagnostics writer failed"),
        }
    }
}

pub async fn attachment(State(a): State<App>, Path(id): Path<String>) -> Response {
    if !valid_id(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let result = tokio::task::spawn_blocking(move || -> Result<Option<Value>> {
        let path = a.diagnostics.root.join(format!("{id}.json"));
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let report: Value = serde_json::from_slice(&bytes)?;
        Ok(report.get("diagnostics").cloned())
    })
    .await;
    match result {
        Ok(Ok(Some(value))) => (
            [
                (
                    "content-disposition",
                    "attachment; filename=\"diagnostics.json\"",
                ),
                ("x-content-type-options", "nosniff"),
            ],
            Json(value),
        )
            .into_response(),
        Ok(Ok(None)) => StatusCode::NOT_FOUND.into_response(),
        Ok(Err(e)) => error(e),
        Err(e) => error(e.into()),
    }
}

pub async fn report(
    a: &App,
    account: i32,
    player: i32,
    message: &str,
    connection: &ConnectionDiagnostics,
    screenshot: Option<String>,
) -> Result<String> {
    save_report(
        a,
        Some((account, player)),
        message,
        connection.snapshot(),
        screenshot,
    )
    .await
}
fn report_owner(report: &Value) -> Result<Option<(i32, i32)>> {
    let account: Option<i32> = serde_json::from_value(
        report
            .get("account_id")
            .context("missing report account")?
            .clone(),
    )?;
    match account {
        Some(account) => Ok(Some((
            account,
            serde_json::from_value(report["context"]["player_id"].clone())?,
        ))),
        None => {
            ensure!(
                report.get("context") == Some(&Value::Null),
                "anonymous report has game context"
            );
            Ok(None)
        }
    }
}
async fn save_report(
    a: &App,
    owner: Option<(i32, i32)>,
    message: &str,
    connection: Value,
    screenshot: Option<String>,
) -> Result<String> {
    ensure!(
        a.discord_link_token.is_some(),
        "Discord bug reporting is not configured."
    );
    ensure!(
        !message.trim().is_empty(),
        "Describe the bug before submitting. Reports are public; do not include personal information."
    );
    ensure!(
        message.chars().count() <= 4000,
        "Bug message is too long (maximum 4000 characters)."
    );
    let requested = Instant::now();
    let context = match owner {
        Some((_, player)) => a.world.directory(move |d| d.bug_context(player)).await?,
        None => Value::Null,
    };
    let id = {
        use rand::RngCore;
        let mut bytes = [0u8; 12];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    let reporter_name = if owner.is_some() {
        context["player_name"]
            .as_str()
            .context("bug reporter name missing")?
    } else {
        "Website visitor (no character connected)"
    };
    let mut report = json!({"schema": 3, "reporter_name": reporter_name, "id": id, "created_at_unix_ms": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis(),
        "delivery": {"status": "pending", "notes_through": 0}, "notes": [], "message_untrusted": message, "account_id": owner.map(|(account, _)| account),
        "server_binary_sha256": a.diagnostics.binary_sha256, "server_source_revision": a.diagnostics.source_revision, "protocol_build": "123.5.1", "context": context,
        "server_uptime_ms": a.started.elapsed().as_millis(), "configured_player_cap": a.max_players,
        "context_request_ms": requested.elapsed().as_secs_f64() * 1000.0, "connection": connection,
        "capture_window_seconds": 120});
    report["diagnostics"] = public_diagnostics(&report);
    report["diagnostics"]["previous_drop"] = owner.map_or(Value::Null, |(account, _)| {
        a.diagnostics.previous_drop(account)
    });
    report["diagnostics_url"] = json!(format!(
        "{}/api/bug-reports/{id}/diagnostics.json",
        a.origin
    ));
    if let Some(screenshot) = screenshot {
        report["screenshot"] = json!(screenshot);
        report["screenshot_url"] =
            json!(format!("{}/api/bug-reports/{id}/screenshot.png", a.origin));
    }
    let object = report.as_object_mut().expect("report object");
    object.remove("connection");
    object.insert(
        "context".into(),
        owner.map_or(Value::Null, |(_, player)| json!({"player_id": player})),
    );
    let diagnostics = a.diagnostics.clone();
    tokio::task::spawn_blocking(move || diagnostics.save(&report)).await??;
    tracing::info!(report_id=%id, ?owner, "bug report saved; awaiting Discord delivery");
    Ok(id)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebReportInput {
    message: String,
    screenshot: String,
}
pub struct WebReport {
    input: WebReportInput,
    reply: tokio::sync::oneshot::Sender<Result<String>>,
}
fn screenshot_png(encoded: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    ensure!(encoded.len() <= 1_400_000, "Game screenshot exceeds 1 MiB.");
    let png = base64::engine::general_purpose::STANDARD.decode(encoded)?;
    ensure!(
        png.len() >= 33
            && png.len() <= 1024 * 1024
            && png.starts_with(b"\x89PNG\r\n\x1a\n")
            && &png[12..16] == b"IHDR",
        "Game screenshot must be a PNG of at most 1 MiB."
    );
    for offset in [16, 20] {
        let dimension = u32::from_be_bytes(png[offset..offset + 4].try_into()?);
        ensure!(
            (1..=4096).contains(&dimension),
            "Invalid game screenshot dimensions."
        );
    }
    Ok(png)
}
pub async fn submit(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(key): Path<String>,
    headers: HeaderMap,
    Json(input): Json<WebReportInput>,
) -> Response {
    super::hiscores::json_result(async {
        ensure!(headers.get("origin").and_then(|v|v.to_str().ok()) == Some(a.origin.as_str()), "Bug report origin denied.");
        ensure!(!input.message.trim().is_empty() && input.message.chars().count() <= 4000, "Describe the bug in 1–4000 characters.");
        screenshot_png(&input.screenshot)?;
        ensure!(key.len() == 32 && key.bytes().all(|b| b.is_ascii_hexdigit()), "invalid diagnostic session");
        let browser = a.diagnostics.browsers.lock().expect("diagnostics poisoned").get(&key).and_then(|b|b.upgrade());
        let connected = browser.as_ref().is_some_and(|b| b.lock().expect("browser poisoned").reports.as_ref().is_some_and(|s| !s.is_closed()));
        if !connected {
            // Anonymous input has no game authority; bound it by trusted peer IP: https://cheatsheetseries.owasp.org/cheatsheets/REST_Security_Cheat_Sheet.html#http-return-code
            let ip = super::request_ip(&a, &headers, peer)?;
            {
                let mut recent = a.diagnostics.anonymous_reports.lock().expect("anonymous report rates poisoned");
                recent.retain(|_, at| at.elapsed().as_secs() < 30);
                ensure!(!recent.contains_key(&ip), "Wait 30 seconds between bug reports.");
                ensure!(recent.len() < 4096, "Anonymous report limit reached; try later.");
                recent.insert(ip, Instant::now());
            }
            let connection = json!({"user_agent":headers.get("user-agent").map(|v| v.to_str()).transpose()?,"rtt_samples":[],"browser_samples":[]});
            let id = save_report(&a, None, &input.message, connection, Some(input.screenshot)).await?;
            return Ok(json!({"id":id}));
        }
        let browser = browser.context("connected diagnostic session missing")?;
        let (reply, response) = tokio::sync::oneshot::channel();
        {
            let mut browser = browser.lock().expect("browser poisoned");
            ensure!(browser.last_report.is_none_or(|t|t.elapsed().as_secs() >= 30), "Wait 30 seconds between bug reports.");
            browser.reports.as_ref().context("No game connection for this tab.")?.try_send(WebReport {input,reply}).context("Game connection is busy or closed; report was not queued.")?;
            browser.last_report = Some(Instant::now());
        }
        let id = tokio::time::timeout(std::time::Duration::from_secs(30), response).await.context("Submission outcome unknown; check the bug backlog before retrying.")?.context("Game disconnected before confirming submission; check the backlog before retrying.")??;
        Ok(json!({"id":id}))
    }.await)
}
pub(super) async fn from_session(a: &App, session: &Session, request: WebReport) {
    // Resolve identity on the owning game task, not browser-supplied IDs: https://cheatsheetseries.owasp.org/cheatsheets/Session_Management_Cheat_Sheet.html
    let result = async {
        ensure!(
            !session.reconnecting,
            "World transfer in progress; try again when connected."
        );
        let account = session
            .account
            .as_ref()
            .context("Sign into the game before reporting.")?
            .id;
        let player = session
            .player
            .context("Connect to a character before reporting.")?;
        report(
            a,
            account,
            player,
            &request.input.message,
            &session.diagnostics,
            Some(request.input.screenshot),
        )
        .await
    }
    .await;
    if let Err(error) = &result {
        tracing::warn!(%error, "website bug report failed");
    }
    let _ = request.reply.send(result);
}
pub async fn status(State(a): State<App>, Path(id): Path<String>) -> Response {
    if !valid_id(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let result = tokio::task::spawn_blocking(move || -> Result<Option<Value>> {
        let bytes = match std::fs::read(a.diagnostics.root.join(format!("{id}.json"))) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let report: Value = serde_json::from_slice(&bytes)?;
        ensure!(report["id"] == id, "stored report identity mismatch");
        let url = match report["delivery"].get("issue_number") {
            None | Some(Value::Null) => None,
            Some(Value::String(issue)) => {
                Receipt {
                    target: "issue_number".into(),
                    id: issue.clone(),
                    through: None,
                }
                .validate()?;
                Some(format!(
                    "https://github.com/OpenRotMG/Open-RotMG-Server/issues/{issue}"
                ))
            }
            Some(_) => bail!("invalid stored issue number"),
        };
        Ok(Some(json!({"url":url})))
    })
    .await;
    match result {
        Ok(Ok(Some(value))) => ([("cache-control", "no-store")], Json(value)).into_response(),
        Ok(Ok(None)) => StatusCode::NOT_FOUND.into_response(),
        Ok(Err(error)) => super::error(error),
        Err(error) => super::error(error.into()),
    }
}
pub async fn screenshot(State(a): State<App>, Path(id): Path<String>) -> Response {
    if !valid_id(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let result = tokio::task::spawn_blocking(move || -> Result<Option<Vec<u8>>> {
        let bytes = match std::fs::read(a.diagnostics.root.join(format!("{id}.json"))) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let report: Value = serde_json::from_slice(&bytes)?;
        report
            .get("screenshot")
            .map(|v| screenshot_png(v.as_str().context("Invalid stored screenshot")?))
            .transpose()
    })
    .await;
    match result {
        Ok(Ok(Some(png))) => (
            [
                ("content-type", "image/png"),
                ("x-content-type-options", "nosniff"),
                ("cache-control", "no-store"),
            ],
            png,
        )
            .into_response(),
        Ok(Ok(None)) => StatusCode::NOT_FOUND.into_response(),
        Ok(Err(e)) => error(e),
        Err(e) => error(e.into()),
    }
}

pub async fn browser_sample(
    State(a): State<App>,
    Path(key): Path<String>,
    headers: HeaderMap,
    Json(sample): Json<BrowserSample>,
) -> Response {
    let result: Result<bool> = (|| {
        ensure!(
            headers.get("origin").and_then(|v| v.to_str().ok()) == Some(a.origin.as_str()),
            "unapproved diagnostic origin"
        );
        sample.validate()?;
        let browser = a
            .diagnostics
            .browsers
            .lock()
            .expect("diagnostics poisoned")
            .get(&key)
            .and_then(|v| v.upgrade());
        let Some(browser) = browser else {
            return Ok(false);
        };
        let mut browser = browser.lock().expect("browser poisoned");
        ensure!(
            browser
                .samples
                .back()
                .is_none_or(|(t, _)| t.elapsed().as_secs() >= 3),
            "diagnostic sample rate exceeded"
        );
        while browser.samples.len() >= 24 {
            browser.samples.pop_front();
        }
        browser.samples.push_back((Instant::now(), sample));
        Ok(true)
    })();
    match result {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::CONFLICT,
            "No active game socket for this diagnostic session",
        )
            .into_response(),
        Err(e) => error(e),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    target: String,
    id: String,
    through: Option<usize>,
}
impl Receipt {
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.id.is_empty() && self.id.bytes().all(|b| b.is_ascii_digit()),
            "invalid bug delivery receipt"
        );
        ensure!(
            match self.target.as_str() {
                "private_message_id" | "public_message_id" =>
                    (17..=20).contains(&self.id.len()) && self.through.is_none(),
                "issue_number" =>
                    self.id.parse::<u32>().is_ok_and(|n| n > 0) && self.through.is_none(),
                "notes" =>
                    self.id.parse::<u64>().is_ok_and(|n| n > 0)
                        && self.through.is_some_and(|n| n > 0),
                _ => false,
            },
            "invalid bug delivery target or range"
        );
        Ok(())
    }
}
pub async fn pending(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = super::hiscores::authorize(&a, peer, &headers) {
        return error(e);
    }
    let result = tokio::task::spawn_blocking(move || -> Result<Value> {
        Ok(json!({"report": a.diagnostics.next()?}))
    })
    .await;
    match result {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => error(e),
        Err(e) => error(e.into()),
    }
}
pub async fn receipt(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(receipt): Json<Receipt>,
) -> Response {
    let result: Result<()> = async {
        super::hiscores::authorize(&a, peer, &headers)?;
        ensure!(valid_id(&id), "invalid bug report ID");
        receipt.validate()?;
        if receipt.target == "issue_number" {
            let diagnostics = a.diagnostics.clone();
            let (id, issue) = (id.clone(), receipt.id.clone());
            let owner = tokio::task::spawn_blocking(move || -> Result<Option<(i32, i32)>> {
                let report: Value = serde_json::from_slice(&std::fs::read(diagnostics.root.join(format!("{id}.json")))?)?;
                ensure!(report["id"] == id && matches!(report["delivery"]["status"].as_str(), Some("pending" | "delivered")), "invalid stored bug report");
                ensure!(report["delivery"]["issue_number"].is_null() || report["delivery"]["issue_number"] == issue, "conflicting delivery receipt");
                report_owner(&report)
            }).await??;
            // Keep the report retryable until its once-only reward is durable.
            if let Some((account, _)) = owner {
                a.store.reward_first_bug(account).await?;
                super::gold::sync(&a, account).await?;
            }
        }
        let world = a.world.clone();
        let notification = tokio::task::spawn_blocking(move || a.diagnostics.acknowledge(&id, &receipt)).await??;
        if let Some((player, account, issue)) = notification
            && let Err(error) = world.for_player(player).call(move |w| {
                let p = w.players.get(&player).context("reporting player disconnected")?;
                ensure!(p.character.account == account, "reporting session changed");
                let packet = crate::world::chat_info(&format!("Reported as #{issue}: https://github.com/OpenRotMG/Open-RotMG-Server/issues/{issue}"));
                crate::world::send(p, packet.id, packet.body);
                Ok(())
            }).await { tracing::info!(%error, "bug issue created after reporting player disconnected"); }
        Ok(())
    }.await;
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => error(e),
    }
}

#[cfg(test)]
mod native_lag_tests {
    use super::*;
    #[test]
    fn native_dispatch_gaps_ignore_other_packets_and_keep_the_session_maximum() {
        let mut connection = ConnectionDiagnostics::default();
        let start = Instant::now() - std::time::Duration::from_secs(5);
        assert!(connection.snapshot()["max_native_tick_send_gap_ms_session"].is_null());
        assert_eq!(connection.queued_packet_sent(62, start), None);
        assert!(connection.snapshot()["max_native_tick_send_gap_ms_session"].is_null());
        connection.queued_packet_sent(wire::PING, start + std::time::Duration::from_secs(2));
        let stall = connection.queued_packet_sent(62, start + std::time::Duration::from_secs(3));
        assert_eq!(stall, Some(std::time::Duration::from_secs(3)));
        let ms = std::time::Duration::from_millis;
        let on_time =
            connection.queued_packet_sent(62, start + std::time::Duration::from_secs(3) + ms(100));
        assert_eq!(on_time, None);
        connection.queued_packet_sent(62, start + std::time::Duration::from_secs(4));
        let snapshot = connection.snapshot();
        assert_eq!(snapshot["native_ticks_sent_session"], 4);
        assert_eq!(snapshot["native_tick_stalls_session"], 2);
        assert_eq!(
            snapshot["max_native_tick_send_gap_ms_session"].as_f64(),
            Some(3000.)
        );
        assert!(
            snapshot["max_native_tick_send_gap_end_unix_ms"]
                .as_u64()
                .is_some()
        );
        assert_eq!(
            connection.last_socket_send,
            Some(start + std::time::Duration::from_secs(4))
        );
        let public = public_diagnostics(&json!({"connection":snapshot}));
        assert_eq!(public["connection"]["native_ticks_sent"], 4);
        assert_eq!(public["connection"]["native_tick_stalls"], 2);
        assert_eq!(
            public["connection"]["max_native_tick_send_gap_ms"].as_f64(),
            Some(3000.)
        );
    }
    #[test]
    fn visibility_is_a_hint_and_missing_metrics_are_not_zero() {
        let raw = json!({"max_gap_ms":700.,"max_gap_end_age_ms":100.,"private":"SECRET",
            "history":[{"max_gap_ms":700.,"max_gap_end_age_ms":100.}]});
        let visible =
            json!({"age_ms":0,"sample":{"elapsed_ms":5000.,"hidden_during_interval":false}});
        let hidden =
            json!({"age_ms":0,"sample":{"elapsed_ms":5000.,"hidden_during_interval":true}});
        let partial =
            json!({"age_ms":0,"sample":{"elapsed_ms":500.,"hidden_during_interval":false}});
        let v = lag_diagnostics(&raw, &[&visible]);
        assert_eq!(v["max_gap_visibility_hint"], "visible_sample");
        assert_eq!(v["history"][0]["max_gap_visibility_hint"], "visible_sample");
        assert!(!v.to_string().contains("SECRET"));
        assert_eq!(
            lag_diagnostics(&raw, &[&visible, &hidden])["max_gap_visibility_hint"],
            "hidden_or_mixed_sample"
        );
        assert_eq!(
            lag_diagnostics(&raw, &[&partial])["max_gap_visibility_hint"],
            "unavailable"
        );
        assert_eq!(
            lag_diagnostics(&raw, &[])["max_gap_visibility_hint"],
            "unavailable"
        );
        assert!(lag_diagnostics(&Value::Null, &[]).is_null());
    }
}
