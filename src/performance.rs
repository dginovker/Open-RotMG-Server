use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    time::{Duration, Instant},
};

const RETAINED: usize = 256;

pub struct Measure {
    wall: Instant,
    cpu: Option<Duration>,
    // A thread-CPU interval must not migrate through an async await.
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}
#[derive(Clone, Copy)]
pub struct Measurement {
    pub wall: Duration,
    pub cpu: Option<Duration>,
}
impl Measure {
    pub fn start() -> Self {
        Self {
            wall: Instant::now(),
            cpu: thread_cpu(),
            _thread: std::marker::PhantomData,
        }
    }
    pub fn finish(self) -> Measurement {
        let cpu = self.cpu.map(|start| {
            thread_cpu()
                .expect("thread CPU clock support changed")
                .checked_sub(start)
                .expect("thread CPU clock regressed")
        });
        Measurement {
            wall: self.wall.elapsed(),
            cpu,
        }
    }
}

tokio::task_local! {pub static REQUEST: std::sync::Arc<std::sync::Mutex<Requests>>;}
#[derive(Default, Clone)]
pub struct Requests {
    pub calls: u64,
    pub completed: u64,
    pub send_wait: Duration,
    pub queued: Duration,
    pub work: Duration,
    pub cpu: Option<Duration>,
    pub round_trip: Duration,
    pub native_control: Option<std::sync::Arc<NativeControlTrace>>,
}
pub struct NativeControlTrace {
    pub request: String,
    pub operation: String,
    pub started: Instant,
}
impl NativeControlTrace {
    pub fn record(&self, call: u64, stage: &str, receiver_closed: bool) {
        tracing::info!(request=%self.request,operation=%self.operation,call,stage,receiver_closed,
            elapsed_ms=self.started.elapsed().as_secs_f64()*1000.,"native control world request");
    }
}
impl Requests {
    pub fn snapshot(&self) -> Value {
        json!({"calls":self.calls,"completed":self.completed,"send_wait_sum_ms":self.send_wait.as_secs_f64()*1000.,"queue_wait_sum_ms":self.queued.as_secs_f64()*1000.,"work_wall_sum_ms":self.work.as_secs_f64()*1000.,"work_cpu_sum_ms":self.cpu.map(|d|d.as_secs_f64()*1000.),"round_trip_sum_ms":self.round_trip.as_secs_f64()*1000.})
    }
}
#[cfg(target_os = "linux")]
fn thread_cpu() -> Option<Duration> {
    let mut ts = std::mem::MaybeUninit::<libc::timespec>::uninit();
    let result = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, ts.as_mut_ptr()) };
    assert_eq!(
        result,
        0,
        "thread CPU clock failed: {}",
        std::io::Error::last_os_error()
    );
    let ts = unsafe { ts.assume_init() };
    Some(Duration::new(
        ts.tv_sec.try_into().expect("negative thread CPU time"),
        ts.tv_nsec.try_into().expect("invalid CPU nanoseconds"),
    ))
}
#[cfg(not(target_os = "linux"))]
fn thread_cpu() -> Option<Duration> {
    None
}

#[derive(Default)]
struct Samples {
    recent: VecDeque<u64>,
    count: u64,
    total: u64,
    maximum: u64,
    over_budget: u64,
}
impl Samples {
    fn record(&mut self, us: u64) {
        self.count += 1;
        self.total += us;
        self.maximum = self.maximum.max(us);
        self.over_budget += u64::from(us > 50_000);
        if self.recent.len() == RETAINED {
            self.recent.pop_front();
        }
        self.recent.push_back(us);
    }
    fn snapshot(&self) -> Value {
        let mut sorted: Vec<_> = self.recent.iter().copied().collect();
        sorted.sort_unstable();
        let percentile =
            |p: f64| sorted[((sorted.len() as f64 * p).ceil() as usize).saturating_sub(1)];
        json!({"count":self.count,"retained_samples":sorted.len(),"mean_us":self.total/self.count,
            "p50_us":percentile(0.5),"p95_us":percentile(0.95),"p99_us":percentile(0.99),
            "max_us":self.maximum,"over_50ms":self.over_budget})
    }
}

#[derive(Default)]
pub struct Window {
    started: Option<Instant>,
    samples: BTreeMap<&'static str, Samples>,
    cpu: BTreeMap<&'static str, Samples>,
    counters: BTreeMap<&'static str, u64>,
}
impl Window {
    pub fn work(&mut self, name: &'static str, measure: Measure) {
        self.completed(name, measure.finish());
    }
    pub fn completed(&mut self, name: &'static str, measurement: Measurement) {
        if let Some(cpu) = measurement.cpu {
            self.cpu
                .entry(name)
                .or_default()
                .record(cpu.as_micros().try_into().expect("CPU timing exceeds u64"));
        }
        self.record(name, measurement.wall);
    }
    pub fn count(&mut self, name: &'static str, amount: u64) {
        *self.counters.entry(name).or_default() += amount;
    }
    pub fn record(&mut self, name: &'static str, elapsed: Duration) {
        self.started.get_or_insert_with(|| Instant::now() - elapsed);
        self.samples
            .entry(name)
            .or_default()
            .record(elapsed.as_micros().try_into().expect("timing exceeds u64"));
    }
    pub fn report(&mut self) -> Option<Value> {
        let elapsed = self.started?.elapsed();
        if elapsed < Duration::from_secs(5) {
            return None;
        }
        let achieved_step_hz = self
            .samples
            .get("step")
            .map(|s| s.count as f64 / elapsed.as_secs_f64());
        let missed_tick_deadlines = self
            .samples
            .get("scheduling_lateness")
            .map(|s| s.over_budget);
        let timings: BTreeMap<_, _> = std::mem::take(&mut self.samples)
            .into_iter()
            .map(|(name, s)| (name, s.snapshot()))
            .collect();
        let cpu: BTreeMap<_, _> = std::mem::take(&mut self.cpu)
            .into_iter()
            .map(|(name, s)| (name, s.snapshot()))
            .collect();
        let counters = std::mem::take(&mut self.counters);
        self.started = Some(Instant::now());
        Some(
            json!({"pid":std::process::id(),"window_seconds":elapsed.as_secs_f64(),"achieved_step_hz":achieved_step_hz,"missed_tick_deadlines":missed_tick_deadlines,
            "percentiles":"latest 256 samples; count/max cover full window","timings":timings,"thread_cpu":cpu,"phase_scope":"inclusive; overlapping phases must not be summed","cpu_clock":if cfg!(target_os="linux") {"CLOCK_THREAD_CPUTIME_ID"}else{"unsupported"},"counters":counters}),
        )
    }
}

pub fn memory() -> Value {
    fn read() -> anyhow::Result<Value> {
        let status = std::fs::read_to_string("/proc/self/status")?;
        let field = |name: &str| -> anyhow::Result<u64> {
            let line = status
                .lines()
                .find(|l| l.starts_with(name))
                .ok_or_else(|| anyhow::anyhow!("missing {name}"))?;
            Ok(line
                .split_whitespace()
                .nth(1)
                .ok_or_else(|| anyhow::anyhow!("invalid {name}"))?
                .parse()?)
        };
        Ok(
            json!({"resident_kib":field("VmRSS:")?,"high_water_kib":field("VmHWM:")?,"anonymous_kib":field("RssAnon:")?,"swap_kib":field("VmSwap:")?,"threads":field("Threads:")?}),
        )
    }
    match read() {
        Ok(value) => value,
        Err(error) => json!({"unavailable":error.to_string()}),
    }
}
