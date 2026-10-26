use super::Measurement;
use serde_json::{Value, json};
use std::{
    cell::Cell,
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

const BOUNDS_US: [u64; 23] = [
    0,
    10,
    25,
    50,
    100,
    250,
    500,
    1_000,
    2_000,
    4_000,
    8_000,
    12_000,
    16_000,
    24_000,
    32_000,
    50_000,
    75_000,
    100_000,
    250_000,
    500_000,
    1_000_000,
    5_000_000,
    u64::MAX,
];
const STAGES: [&str; 7] = [
    "send",
    "mailbox",
    "ownership",
    "pool",
    "work",
    "publish",
    "reply",
];
const EXAMPLES: usize = 4;
static NEXT: AtomicU64 = AtomicU64::new(1);
thread_local! { static CURRENT: Cell<(&'static str, u64)> = const { Cell::new(("coordinator_tick", 0)) }; }
pub(crate) struct Context(
    (&'static str, u64),
    std::marker::PhantomData<std::rc::Rc<()>>,
);
impl Context {
    pub fn enter(operation: &'static str, id: u64) -> Self {
        Self(
            CURRENT.with(|c| c.replace((operation, id))),
            std::marker::PhantomData,
        )
    }
    pub fn current() -> (&'static str, u64) {
        CURRENT.with(Cell::get)
    }
}
impl Drop for Context {
    fn drop(&mut self) {
        CURRENT.with(|c| c.set(self.0));
    }
}

pub(crate) fn micros(duration: Duration) -> u64 {
    duration
        .as_micros()
        .try_into()
        .expect("attribution duration exceeds u64")
}

pub(crate) struct Sample {
    pub id: u64,
    pub operation: &'static str,
    pub kind: &'static str,
    pub tick: u64,
    pub stages: [Duration; 7],
    pub cpu: Option<Duration>,
    pub publish_cpu: Option<Duration>,
    pub phases: Vec<(&'static str, Measurement)>,
    pub blocker: Option<(&'static str, u64)>,
}
impl Sample {
    pub fn new(operation: &'static str) -> Box<Self> {
        let id = NEXT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
            .expect("attribution ID exhausted");
        Box::new(Self {
            id,
            operation,
            kind: if matches!(operation, "tick" | "coordinator_tick") {
                "tick"
            } else {
                "job"
            },
            tick: 0,
            stages: [Duration::ZERO; 7],
            cpu: None,
            publish_cpu: None,
            phases: Vec::new(),
            blocker: None,
        })
    }
    fn total(&self) -> Duration {
        self.stages.iter().sum()
    }
    fn snapshot(&self) -> Value {
        let phases: BTreeMap<_, _> = self
            .phases
            .iter()
            .map(|(name, m)| {
                (
                    *name,
                    json!({"wall_us":micros(m.wall), "cpu_us":m.cpu.map(micros)}),
                )
            })
            .collect();
        let stages: BTreeMap<_, _> = STAGES.into_iter().zip(self.stages.map(micros)).collect();
        json!({"id":self.id,"operation":self.operation,"kind":self.kind,"tick":self.tick,"stages_us":stages,"total_us":micros(self.total()),"work_cpu_us":self.cpu.map(micros),"publish_cpu_us":self.publish_cpu.map(micros),"phases":phases,"observed_owner":self.blocker.map(|(operation,id)|json!({"operation":operation,"id":id}))})
    }
}

#[derive(Default)]
struct Histogram {
    counts: [u64; BOUNDS_US.len()],
    sum: u64,
    maximum: u64,
}
impl Histogram {
    fn record(&mut self, value: Duration) {
        let us = micros(value);
        self.counts[BOUNDS_US.partition_point(|bound| *bound < us)] += 1;
        self.sum += us;
        self.maximum = self.maximum.max(us);
    }
    fn snapshot(&self) -> Value {
        let count: u64 = self.counts.iter().sum();
        let quantile = |percent: u64| {
            let mut seen = 0;
            let rank = (count * percent).div_ceil(100);
            self.counts.iter().enumerate().find_map(|(i, n)| {
                seen += n;
                (count > 0 && seen >= rank && i < BOUNDS_US.len() - 1).then_some(BOUNDS_US[i])
            })
        };
        json!({"counts":self.counts,"count":count,"sum_us":self.sum,"max_us":self.maximum,"p95_upper_us":quantile(95),"p99_upper_us":quantile(99)})
    }
}
#[derive(Default)]
struct Operation {
    stages: [Histogram; 7],
    total: Histogram,
    cpu: Histogram,
    publish_cpu: Histogram,
}

#[derive(Default)]
pub(crate) struct Attribution {
    started: Option<Instant>,
    // Operation labels come from compiled call-site types, never request data.
    operations: BTreeMap<(&'static str, &'static str), Operation>,
    examples: [Vec<(Duration, Box<Sample>)>; 3],
}
impl Attribution {
    #[cfg(test)]
    pub(crate) fn slowest(&self) -> impl Iterator<Item = &Sample> {
        self.examples
            .iter()
            .flatten()
            .map(|(_, sample)| sample.as_ref())
    }
    pub fn record(&mut self, sample: Box<Sample>) {
        let total = sample.total();
        self.started.get_or_insert_with(Instant::now);
        let operation = self
            .operations
            .entry((sample.kind, sample.operation))
            .or_default();
        for (histogram, value) in operation.stages.iter_mut().zip(sample.stages) {
            histogram.record(value);
        }
        operation.total.record(total);
        if let Some(cpu) = sample.cpu {
            operation.cpu.record(cpu);
        }
        if let Some(cpu) = sample.publish_cpu {
            operation.publish_cpu.record(cpu);
        }
        let index = match sample.kind {
            "tick" => 0,
            "job" => 1,
            "directory_loan" => 2,
            _ => panic!("unknown attribution kind"),
        };
        let examples = &mut self.examples[index];
        if examples.len() < EXAMPLES {
            examples.push((total, sample));
        } else {
            let (index, smallest) = examples
                .iter()
                .enumerate()
                .min_by_key(|(_, (elapsed, _))| *elapsed)
                .expect("examples empty");
            if total > smallest.0 {
                examples[index] = (total, sample);
            }
        }
    }
    pub fn emit(report: Value, world: Option<i32>) {
        let pid = std::process::id();
        let scope = if world.is_some() {
            "world"
        } else {
            "directory"
        };
        let window_seconds = report["window_seconds"]
            .as_f64()
            .expect("invalid attribution window");
        // One record per compiled operation avoids journal line truncation as callers grow.
        for operation in report["operations"]
            .as_array()
            .expect("invalid attribution operations")
        {
            tracing::info!(pid, scope, world, window_seconds, histogram_upper_bounds_us=%report["histogram_upper_bounds_us"],
                histogram=%operation, quantiles="upper bounds; noncumulative buckets; all completed operations", "latency histogram");
        }
        for kind in ["slowest_ticks", "slowest_jobs", "slowest_directory_loans"] {
            for sample in report[kind]
                .as_array()
                .expect("invalid attribution examples")
            {
                tracing::info!(pid, scope, world, window_seconds, sample=%sample, total_scope=%report["total_scope"],
                    phase_scope="inclusive; do not sum nested phases", owner_scope=%report["owner_scope"], "latency example");
            }
        }
    }
    pub fn report(&mut self) -> Option<Value> {
        let elapsed = self.started?.elapsed();
        if elapsed < Duration::from_secs(60) {
            return None;
        }
        let operations: Vec<_> = std::mem::take(&mut self.operations).into_iter().map(|((kind,name),o)| {
            let stages: BTreeMap<_,_> = STAGES.into_iter().zip(o.stages.iter().map(Histogram::snapshot)).collect();
            json!({"kind":kind,"operation":name,"stages":stages,"total":o.total.snapshot(),"work_cpu":o.cpu.snapshot(),"publish_cpu":o.publish_cpu.snapshot()})
        }).collect();
        let examples = std::mem::take(&mut self.examples).map(|samples| {
            samples
                .iter()
                .map(|(_, sample)| sample.snapshot())
                .collect::<Vec<_>>()
        });
        self.started = Some(Instant::now());
        Some(
            json!({"window_seconds":elapsed.as_secs_f64(),"histogram_upper_bounds_us":&BOUNDS_US[..],"histogram_counts":"noncumulative; every completed operation; last bucket is overflow","operations":operations,"slowest_ticks":examples[0],"slowest_jobs":examples[1],"slowest_directory_loans":examples[2],"example_limit_per_kind":EXAMPLES,"phase_scope":"inclusive; do not sum nested phases","total_scope":"sum of measured stages; excludes instrumentation gaps, caller scheduling/network and post-tick coordination; directory_loan work is ownership hold time, not CPU execution","owner_scope":"first Directory owner observed while acquisition blocked, not all intervening owners"}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn histograms_include_all_samples_and_examples_are_bounded_and_correlated() {
        let mut a = Attribution::default();
        for i in 0..1000 {
            let mut s = Sample::new("tick");
            s.tick = i;
            s.stages[4] = Duration::from_micros(i);
            s.cpu = Some(Duration::from_micros(i / 2));
            s.phases.push((
                "npc",
                Measurement {
                    wall: Duration::from_micros(i),
                    cpu: s.cpu,
                },
            ));
            a.record(s);
        }
        a.started = Some(Instant::now() - Duration::from_secs(60));
        let v = a.report().unwrap();
        assert_eq!(
            v["operations"][0]["total"]["counts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n.as_u64().unwrap())
                .sum::<u64>(),
            1000
        );
        let samples = v["slowest_ticks"].as_array().unwrap();
        assert_eq!(samples.len(), EXAMPLES);
        for s in samples {
            assert!(s["tick"].as_u64().unwrap() >= 996);
            assert_eq!(s["tick"], s["phases"]["npc"]["wall_us"]);
        }
        assert!(a.operations.is_empty());
        assert!(a.examples.iter().all(Vec::is_empty));
        assert!(a.report().is_none());
    }
    #[test]
    fn histogram_bounds_do_not_claim_exact_quantiles_or_drop_overflow() {
        let mut h = Histogram::default();
        for _ in 0..95 {
            h.record(Duration::from_micros(11));
        }
        for _ in 0..4 {
            h.record(Duration::from_micros(26));
        }
        h.record(Duration::from_secs(6));
        let v = h.snapshot();
        assert_eq!(v["count"], 100);
        assert_eq!(v["p95_upper_us"], 25);
        assert_eq!(v["p99_upper_us"], 50);
        assert_eq!(v["counts"][BOUNDS_US.len() - 1], 1);
        assert!(Histogram::default().snapshot()["p99_upper_us"].is_null());
    }
    #[test]
    fn tick_capture_keeps_same_tick_cpu_and_nested_phase_totals() {
        let mut window = crate::performance::Window::default();
        window.begin_capture();
        for _ in 0..2 {
            window.completed(
                "visibility",
                Measurement {
                    wall: Duration::from_micros(5),
                    cpu: Some(Duration::from_micros(3)),
                },
            );
        }
        window.record("scheduling_lateness", Duration::from_micros(7));
        let captured = window.end_capture();
        assert_eq!(captured.len(), 2);
        assert_eq!(captured[0].0, "visibility");
        assert_eq!(captured[0].1.wall, Duration::from_micros(10));
        assert_eq!(captured[0].1.cpu, Some(Duration::from_micros(6)));
        assert_eq!(captured[1].0, "scheduling_lateness");
        window.begin_capture();
        assert!(window.end_capture().is_empty());
    }
    #[test]
    fn context_is_thread_local_and_restored() {
        let outer = Context::current();
        {
            let _c = Context::enter("persistence", 42);
            assert_eq!(Context::current(), ("persistence", 42));
            std::thread::spawn(|| assert_eq!(Context::current(), ("coordinator_tick", 0)))
                .join()
                .unwrap();
        }
        assert_eq!(Context::current(), outer);
    }
}
