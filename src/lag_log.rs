use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RETENTION: Duration = Duration::from_secs(7 * 24 * 3600);
const CAP_BYTES: u64 = 500 * 1024 * 1024;
static LOG: OnceLock<Mutex<Log>> = OnceLock::new();

struct Log {
    dir: PathBuf,
    hour: u64,
    file: File,
}

fn hour_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_secs()
        / 3600
}

fn open_hour(dir: &Path, hour: u64) -> std::io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(format!("lag-{hour:010}.jsonl")))
}

fn prune(dir: &Path, current_hour: u64) -> std::io::Result<()> {
    let mut files: Vec<(u64, PathBuf, u64)> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().into_string().unwrap_or_default();
        if let Some(hour) = name
            .strip_prefix("lag-")
            .and_then(|n| n.strip_suffix(".jsonl"))
            .and_then(|n| n.parse().ok())
        {
            files.push((hour, entry.path(), entry.metadata()?.len()));
        }
    }
    files.sort();
    let oldest_kept = current_hour.saturating_sub(RETENTION.as_secs() / 3600);
    let mut total: u64 = files.iter().map(|f| f.2).sum();
    for (hour, path, len) in files {
        if hour != current_hour && (hour < oldest_kept || total > CAP_BYTES) {
            std::fs::remove_file(path)?;
            total -= len;
        }
    }
    Ok(())
}

pub fn open(state: &Path) -> Result<PathBuf> {
    let dir = state.join("lag");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let hour = hour_now();
    prune(&dir, hour)?;
    let file = open_hour(&dir, hour)?;
    LOG.set(Mutex::new(Log {
        dir: dir.clone(),
        hour,
        file,
    }))
    .map_err(|_| anyhow::anyhow!("lag log opened twice"))?;
    Ok(dir)
}

pub struct Writer;
impl Write for Writer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut log = LOG
            .get()
            .expect("lag record before the lag log was opened")
            .lock()
            .expect("lag log poisoned");
        let result = (|| {
            let hour = hour_now();
            if hour != log.hour {
                log.file = open_hour(&log.dir, hour)?;
                log.hour = hour;
                prune(&log.dir, hour)?;
            }
            log.file.write_all(buf)
        })();
        // The fmt layer drops writer errors; journald must still show a failing lag log.
        if let Err(error) = &result {
            eprintln!("lag log write failed: {error}");
        }
        result.map(|()| buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct HostCounters {
    pressure: Option<BTreeMap<&'static str, u64>>,
    processes: BTreeMap<i32, (String, u64)>,
    threads: BTreeMap<String, (u64, u64)>,
}

fn read_host(own: i32) -> Result<HostCounters> {
    let mut host = HostCounters::default();
    // Kernels without PSI report null pressure, not zero.
    if Path::new("/proc/pressure").exists() {
        let mut pressure = BTreeMap::new();
        for kind in ["cpu", "memory", "io"] {
            let text = std::fs::read_to_string(format!("/proc/pressure/{kind}"))?;
            let total = text
                .split("total=")
                .nth(1)
                .and_then(|t| t.split_whitespace().next())
                .context("pressure total")?;
            pressure.insert(kind, total.parse()?);
        }
        host.pressure = Some(pressure);
    }
    for entry in std::fs::read_dir("/proc")? {
        let Ok(pid) = entry?.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        let (Some(open), Some(close)) = (stat.find('('), stat.rfind(')')) else {
            continue;
        };
        let fields: Vec<&str> = stat[close + 2..].split_whitespace().collect();
        let ticks = fields[11].parse::<u64>()? + fields[12].parse::<u64>()?;
        host.processes
            .insert(pid, (stat[open + 1..close].to_owned(), ticks));
    }
    for entry in std::fs::read_dir(format!("/proc/{own}/task"))? {
        let path = entry?.path();
        let (Ok(comm), Ok(sched)) = (
            std::fs::read_to_string(path.join("comm")),
            std::fs::read_to_string(path.join("schedstat")),
        ) else {
            continue;
        };
        let mut values = sched.split_whitespace().map(|v| v.parse::<u64>());
        let (run, wait) = (
            values.next().context("schedstat run")??,
            values.next().context("schedstat wait")??,
        );
        let group = host.threads.entry(comm.trim().to_owned()).or_default();
        group.0 += run;
        group.1 += wait;
    }
    Ok(host)
}

fn snapshot(
    state: &Path,
    own: i32,
    before: &HostCounters,
    after: &HostCounters,
    seconds: f64,
) -> Result<Value> {
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let path = std::ffi::CString::new(state.as_os_str().as_encoded_bytes())?;
    anyhow::ensure!(
        unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } == 0,
        "statvfs {}: {}",
        state.display(),
        std::io::Error::last_os_error()
    );
    let stat = unsafe { stat.assume_init() };
    let mut others: Vec<_> = after
        .processes
        .iter()
        .filter(|(pid, _)| **pid != own)
        .filter_map(|(pid, (name, ticks))| {
            before.processes.get(pid).map(|(_, b)| (ticks - b, name))
        })
        .collect();
    others.sort_by(|a, b| b.0.cmp(&a.0));
    let cores = |ticks: u64| ticks as f64 / hz / seconds;
    let game_cpu = after
        .processes
        .get(&own)
        .zip(before.processes.get(&own))
        .map(|(a, b)| cores(a.1 - b.1));
    let threads: BTreeMap<_, _> = after
        .threads
        .iter()
        .map(|(name, (run, wait))| {
            let (run0, wait0) = before.threads.get(name).copied().unwrap_or((*run, *wait));
            (name, json!({"running_cores": run.saturating_sub(run0) as f64 / 1e9 / seconds, "cpu_wait_cores": wait.saturating_sub(wait0) as f64 / 1e9 / seconds}))
        })
        .collect();
    let pressure = after
        .pressure
        .as_ref()
        .zip(before.pressure.as_ref())
        .map(|(a, b)| {
            a.iter()
                .map(|(k, v)| (*k, (v - b[k]) as f64 / 1e6 / seconds))
                .collect::<BTreeMap<_, _>>()
        });
    Ok(
        json!({"window_seconds": seconds, "free_disk_bytes": stat.f_bavail * stat.f_frsize,
        "pressure_some_share": pressure, "game_cores": game_cpu, "game_threads": threads,
        "other_processes": others.iter().take(3).map(|(t, n)| json!({"name": n, "cores": cores(*t)})).collect::<Vec<_>>(),
        "load_average": std::fs::read_to_string("/proc/loadavg")?.split_whitespace().take(3).collect::<Vec<_>>()}),
    )
}

pub fn observe_host(state: PathBuf) -> Result<()> {
    let own = std::process::id() as i32;
    let mut before = read_host(own)?;
    let mut at = std::time::Instant::now();
    std::thread::Builder::new()
        .name("host-observer".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(5));
                let after = read_host(own).expect("host counters unavailable");
                let host = snapshot(&state, own, &before, &after, at.elapsed().as_secs_f64())
                    .expect("host snapshot failed");
                tracing::info!(target: "lag", host = %host, "host snapshot");
                (before, at) = (after, std::time::Instant::now());
            }
        })?;
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn retention_drops_old_hours_then_oldest_over_cap_but_never_the_current_hour() {
        let dir = std::env::temp_dir().join(format!("lag-log-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let now = 1_000_000;
        let write = |hour: u64, bytes: u64| {
            std::fs::write(
                dir.join(format!("lag-{hour:010}.jsonl")),
                vec![b'x'; bytes as usize],
            )
            .unwrap()
        };
        write(now - 200, 10);
        write(now - 100, CAP_BYTES / 2);
        write(now - 2, CAP_BYTES / 2);
        write(now - 1, 10);
        write(now, CAP_BYTES);
        prune(&dir, now).unwrap();
        let mut left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(left, [format!("lag-{now:010}.jsonl")]);
    }

    #[test]
    fn host_snapshot_reports_disk_pressure_and_thread_waits() {
        let own = std::process::id() as i32;
        let before = read_host(own).unwrap();
        let spin = std::time::Instant::now();
        while spin.elapsed() < Duration::from_millis(50) {
            std::hint::black_box(0u64);
        }
        let after = read_host(own).unwrap();
        let host = snapshot(&std::env::temp_dir(), own, &before, &after, 0.05).unwrap();
        println!("{host}");
        assert!(host["free_disk_bytes"].as_u64().unwrap() > 0);
        assert!(
            host["pressure_some_share"].is_null() || host["pressure_some_share"]["cpu"].is_f64()
        );
        assert!(host["game_cores"].as_f64().unwrap() > 0.);
        assert!(!host["game_threads"].as_object().unwrap().is_empty());
    }
}
