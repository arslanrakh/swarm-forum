//! The machine a gate ran on, recorded with its numbers (D.12.8: the reference hardware is 16
//! vCPU, 64 GB, and NVMe with a durable fsync of at most 1 ms, measured with each run).
//!
//! Two syncs are measured, because they differ on macOS by about a hundred times:
//! - the **durable fsync** of D.12.8's reference criterion: `File::sync_all`, which is `fsync` on
//!   Linux and `F_FULLFSYNC` on macOS (a plain `fsync` there leaves the data in the drive's cache);
//! - the **commit** the server really pays: a SQLite commit made as the store makes it (WAL,
//!   `synchronous = FULL`), timed by the server binary's `commit-probe`. SQLite calls plain
//!   `fsync` on macOS unless `PRAGMA fullfsync` is on, and the store leaves it off, so there the
//!   commit is not durable against power loss and costs far less than the durable fsync.
//!
//! A probe that fails records nothing (`null`), and the machine is then not the reference.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::clock::now;
use crate::hist::Hist;

/// What the machine is.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Machine {
    /// The OS and architecture.
    pub os: String,
    /// The CPU model, as the OS names it.
    pub cpu: String,
    /// Logical cores.
    pub cores: usize,
    /// Memory, in bytes.
    pub memory_bytes: u64,
    /// Free space where the gate writes, in bytes.
    pub free_disk_bytes: u64,
    /// A durable fsync of a 4 KiB write in the gate's directory (`File::sync_all`, which is
    /// `F_FULLFSYNC` on macOS): median, in milliseconds; `null` if the probe failed.
    pub fsync_p50_ms: Option<f64>,
    /// Its 99th percentile.
    pub fsync_p99_ms: Option<f64>,
    /// A SQLite commit of a 4 KiB row made as the server's store makes it (the server binary's
    /// `commit-probe`): median, in milliseconds; `null` if not measured.
    #[serde(default)]
    pub commit_p50_ms: Option<f64>,
    /// Its 99th percentile.
    #[serde(default)]
    pub commit_p99_ms: Option<f64>,
    /// SQLite's `fullfsync` setting in that probe (0 on the store's connections).
    #[serde(default)]
    pub sqlite_fullfsync: Option<i64>,
    /// Whether it meets the reference hardware: 16 cores, 64 GB, and a durable fsync of at most
    /// 1 ms, measured.
    pub reference: bool,
}

fn run(cmd: &str, args: &[&str]) -> String {
    std::process::Command::new(cmd)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn memory() -> u64 {
    if cfg!(target_os = "macos") {
        return run("sysctl", &["-n", "hw.memsize"]).parse().unwrap_or(0);
    }
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|t| {
            t.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<u64>().ok())
        })
        .map_or(0, |kb| kb * 1_024)
}

fn cpu() -> String {
    if cfg!(target_os = "macos") {
        return run("sysctl", &["-n", "machdep.cpu.brand_string"]);
    }
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|t| {
            t.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split_once(':'))
                .map(|(_, v)| v.trim().to_string())
        })
        .unwrap_or_default()
}

/// Free space in `dir`, from `df -k`.
#[must_use]
pub fn free_disk(dir: &Path) -> u64 {
    let out = run("df", &["-k", &dir.display().to_string()]);
    out.lines()
        .nth(1)
        .and_then(|l| l.split_whitespace().nth(3))
        .and_then(|kb| kb.parse::<u64>().ok())
        .map_or(0, |kb| kb * 1_024)
}

/// Times `n` durable 4 KiB writes in `dir`, on a file of this probe's own (two gates may probe
/// the same directory at once).
///
/// # Errors
///
/// The file could not be written.
pub fn fsync(dir: &Path, n: u32) -> std::io::Result<Hist> {
    std::fs::create_dir_all(dir)?;
    static PROBES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = dir.join(format!(
        ".fsync-probe-{}-{}-{}",
        std::process::id(),
        crate::clock::unix_millis(),
        PROBES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mut f = std::fs::File::create(&path)?;
    let block = [0x5a_u8; 4_096];
    let mut h = Hist::new();
    for _ in 0..n {
        f.write_all(&block)?;
        let t = now();
        f.sync_all()?;
        h.record(now().saturating_duration_since(t));
    }
    drop(f);
    std::fs::remove_file(&path)?;
    Ok(h)
}

/// The commit probe of the server binary `bin` (`forum-server commit-probe`), in `dir`: the
/// median and 99th percentile in milliseconds, and SQLite's `fullfsync`.
#[must_use]
pub fn commit(bin: &Path, dir: &Path) -> Option<(f64, f64, i64)> {
    let o = std::process::Command::new(bin)
        .args(["commit-probe", "--n", "200", "--dir"])
        .arg(dir)
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).ok()?;
    Some((
        v.get("p50_ms")?.as_f64()?,
        v.get("p99_ms")?.as_f64()?,
        v.get("fullfsync")?.as_i64()?,
    ))
}

/// The machine, with a durable fsync measured in `dir`, and the server's commit when its binary
/// `bin` (built with the `bench` feature) is given.
#[must_use]
pub fn machine(dir: &Path, bin: Option<&Path>) -> Machine {
    let cores = std::thread::available_parallelism().map_or(0, std::num::NonZero::get);
    let memory_bytes = memory();
    #[allow(clippy::cast_precision_loss, reason = "microseconds as milliseconds")]
    let ms = |us: u64| us as f64 / 1_000.0;
    let h = fsync(dir, 200).ok().filter(|h| !h.is_empty());
    let fsync_p50_ms = h.as_ref().map(|h| ms(h.quantile_us(0.5)));
    let fsync_p99_ms = h.as_ref().map(|h| ms(h.quantile_us(0.99)));
    let c = bin.and_then(|b| commit(b, dir));
    Machine {
        os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        cpu: cpu(),
        cores,
        memory_bytes,
        free_disk_bytes: free_disk(dir),
        fsync_p50_ms,
        fsync_p99_ms,
        commit_p50_ms: c.map(|x| x.0),
        commit_p99_ms: c.map(|x| x.1),
        sqlite_fullfsync: c.map(|x| x.2),
        reference: cores >= 16
            && memory_bytes >= 60 * (1 << 30)
            && fsync_p50_ms.is_some_and(|f| f <= 1.0),
    }
}

/// A short line for the console.
#[must_use]
pub fn describe(m: &Machine) -> String {
    #[allow(clippy::cast_precision_loss, reason = "GiB for the console")]
    let gib = |b: u64| b as f64 / f64::from(1_u32 << 30);
    let ms = |x: Option<f64>| x.map_or_else(|| "failed".to_string(), |v| format!("{v:.2} ms"));
    let commit = match (m.commit_p50_ms, m.commit_p99_ms) {
        (Some(a), Some(b)) => format!(
            ", the server's SQLite commit p50 {a:.2} ms p99 {b:.2} ms (fullfsync {})",
            m.sqlite_fullfsync.unwrap_or(0)
        ),
        _ => String::new(),
    };
    format!(
        "{} cores, {:.0} GiB memory, {:.0} GiB free disk, durable fsync p50 {} p99 {}{} ({}; {}){}",
        m.cores,
        gib(m.memory_bytes),
        gib(m.free_disk_bytes),
        ms(m.fsync_p50_ms),
        ms(m.fsync_p99_ms),
        commit,
        m.cpu,
        m.os,
        if m.reference {
            ", reference hardware"
        } else {
            ", not the reference hardware (16 vCPU, 64 GB, durable fsync <= 1 ms)"
        }
    )
}

/// How long `f` took.
pub fn timed<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let t = now();
    let x = f();
    (x, now().saturating_duration_since(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_machine_is_described() {
        let dir = std::env::temp_dir().join(format!("swarm-sim-machine-{}", std::process::id()));
        let h = fsync(&dir, 3).unwrap();
        assert_eq!(h.len(), 3);
        let m = machine(&dir, None);
        assert!(m.cores >= 1);
        assert!(m.fsync_p50_ms.is_some());
        assert!(
            m.commit_p50_ms.is_none(),
            "no server binary, no commit probe"
        );
        let failed = machine(Path::new("/dev/null/no-such-dir"), None);
        assert_eq!(failed.fsync_p50_ms, None, "a failed probe records nothing");
        assert!(!failed.reference);
        assert!(describe(&failed).contains("failed"));
        assert!(m.memory_bytes > 0);
        assert!(describe(&m).contains("cores"));
        let (x, d) = timed(|| 2 + 2);
        assert_eq!(x, 4);
        assert!(d < Duration::from_secs(1));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
