//! The server under test: a `forum-server` binary started on a database of its own, with tokens
//! made by its own `token new`, the bench switches of its `bench` feature (a compressed clock and
//! the writer's counters as JSON lines), and its memory read from `ps`.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::process::{Child, Command};

use crate::clock::{Instant, now};
use crate::hist::Hist;
use crate::mcp::Mcp;

/// The classes of the simulated agents (D.15.1: several vendors and classes).
pub const CLASSES: [&str; 4] = [
    "coding-agent",
    "review-agent",
    "research-agent",
    "ops-agent",
];

/// The settings variables that `serve` reads (design D.11.9; `forum_server::config::ENV_SETTINGS`,
/// repeated here since this crate links no forum crate), removed from the server's environment so
/// that a developer's own do not configure the server under test.
const SETTINGS: [&str; 5] = [
    "FORUM_CONFIG",
    "FORUM_LISTEN",
    "FORUM_DATABASE",
    "FORUM_SEARCH_DATABASE",
    "FORUM_ALLOWED_HOSTS",
];

/// How to start the server.
#[derive(Clone, Debug)]
pub struct Launch {
    /// The binary, built with the `bench` feature.
    pub bin: PathBuf,
    /// The directory of its database, logs, and stats.
    pub dir: PathBuf,
    /// The port on 127.0.0.1.
    pub port: u16,
    /// How many times as fast as real time its clock runs.
    pub scale: u64,
    /// Where its clock starts (Unix milliseconds), if not now.
    pub clock_start: Option<u64>,
    /// With search (D.11.12).
    pub search: bool,
    /// A `shell_version` bump with an empty step (G-7).
    pub shell_bump: bool,
    /// Extra lines for a configuration file's `[params]` table.
    pub params: Vec<String>,
}

impl Launch {
    /// A launch of `bin` in `dir` on a free port, at real time, with search.
    ///
    /// # Errors
    ///
    /// No free port.
    pub fn new(bin: &Path, dir: &Path) -> Result<Launch, String> {
        // The server runs in `dir`: both paths must be absolute.
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let abs = |p: &Path| std::fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()));
        Ok(Launch {
            bin: abs(bin)?,
            dir: abs(dir)?,
            port: free_port()?,
            scale: 1,
            clock_start: None,
            search: true,
            shell_bump: false,
            params: Vec::new(),
        })
    }

    /// The database file.
    #[must_use]
    pub fn db(&self) -> PathBuf {
        self.dir.join("forum.db")
    }

    /// The stats file.
    #[must_use]
    pub fn stats(&self) -> PathBuf {
        self.dir.join("stats.jsonl")
    }

    /// The MCP endpoint.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port)
    }
}

/// A free port on 127.0.0.1.
///
/// # Errors
///
/// The OS gave none.
pub fn free_port() -> Result<u16, String> {
    let l = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    l.local_addr().map(|a| a.port()).map_err(|e| e.to_string())
}

/// Tokens for the classes: `(token, environment line)` from `forum-server token new`.
///
/// # Errors
///
/// The binary failed or printed something else.
pub async fn tokens(bin: &Path) -> Result<Vec<(String, String, String)>, String> {
    let mut out = Vec::new();
    for (i, class) in CLASSES.iter().enumerate() {
        let name = format!("SIM{i}");
        let o = Command::new(bin)
            .args(["token", "new", "--class", class, "--name", &name])
            .output()
            .await
            .map_err(|e| format!("{}: {e}", bin.display()))?;
        let text = String::from_utf8_lossy(&o.stdout).to_string();
        let token = text
            .lines()
            .find_map(|l| l.strip_prefix("token: "))
            .ok_or_else(|| format!("token new printed no token: {text}"))?
            .to_string();
        let line = text
            .lines()
            .find(|l| l.starts_with("FORUM_TOKEN_"))
            .ok_or("token new printed no FORUM_TOKEN_ line")?;
        let (var, value) = line.split_once('=').ok_or("a malformed token line")?;
        out.push((token, var.to_string(), value.to_string()));
    }
    Ok(out)
}

/// A running server.
pub struct Server {
    /// How it was started.
    pub launch: Launch,
    child: Child,
    /// The bearer tokens, one per class.
    pub tokens: Vec<String>,
    env: Vec<(String, String)>,
    /// When it was spawned.
    pub spawned: Instant,
    /// How long it took to serve: from the spawn to the first successful `initialize`.
    pub ready_after: Duration,
}

impl Server {
    /// Starts the server and waits until it serves MCP (at most `ready_within`).
    ///
    /// # Errors
    ///
    /// It did not start or did not serve in time.
    pub async fn start(launch: Launch, ready_within: Duration) -> Result<Server, String> {
        std::fs::create_dir_all(&launch.dir).map_err(|e| e.to_string())?;
        let made = tokens(&launch.bin).await?;
        let env: Vec<(String, String)> = made
            .iter()
            .map(|(_, k, v)| (k.clone(), v.clone()))
            .collect();
        let tokens = made.into_iter().map(|(t, _, _)| t).collect();
        Server::spawn(launch, tokens, env, ready_within).await
    }

    async fn spawn(
        launch: Launch,
        tokens: Vec<String>,
        env: Vec<(String, String)>,
        ready_within: Duration,
    ) -> Result<Server, String> {
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(launch.dir.join("server.log"))
            .map_err(|e| e.to_string())?;
        let log2 = log.try_clone().map_err(|e| e.to_string())?;
        let mut cmd = Command::new(&launch.bin);
        for v in SETTINGS {
            cmd.env_remove(v);
        }
        cmd.current_dir(&launch.dir)
            .arg("serve")
            .arg("--db")
            .arg(launch.db())
            .arg("--listen")
            .arg(format!("127.0.0.1:{}", launch.port))
            .arg("--bench-stats")
            .arg(launch.stats())
            .arg("--clock-scale")
            .arg(launch.scale.to_string())
            .arg("--search-db")
            .arg(if launch.search {
                launch.dir.join("forum-search.db").display().to_string()
            } else {
                String::new()
            })
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log2))
            .kill_on_drop(true);
        if let Some(t) = launch.clock_start {
            cmd.arg("--clock-start").arg(t.to_string());
        }
        if launch.shell_bump {
            cmd.arg("--bench-shell-bump");
        }
        if !launch.params.is_empty() {
            let cfg = launch.dir.join("forum.toml");
            let text = format!("[params]\n{}\n", launch.params.join("\n"));
            std::fs::write(&cfg, text).map_err(|e| e.to_string())?;
            cmd.arg("--config").arg(cfg);
        }
        for (k, v) in &env {
            cmd.env(k, v);
        }
        let spawned = now();
        let child = cmd
            .spawn()
            .map_err(|e| format!("{}: {e}", launch.bin.display()))?;
        let mut s = Server {
            launch,
            child,
            tokens,
            env,
            spawned,
            ready_after: Duration::ZERO,
        };
        s.ready_after = s.wait_ready(ready_within).await?;
        Ok(s)
    }

    async fn wait_ready(&mut self, within: Duration) -> Result<Duration, String> {
        let deadline = self.spawned + within;
        let token = self.tokens.first().cloned().unwrap_or_default();
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Err(format!(
                    "the server exited with {status} (see {})",
                    self.launch.dir.join("server.log").display()
                ));
            }
            if let Ok(mut m) = Mcp::new(&self.launch.url(), &token, Duration::from_secs(5))
                && m.initialize().await.is_ok()
            {
                return Ok(now().saturating_duration_since(self.spawned));
            }
            if now() >= deadline {
                return Err(format!("the server did not serve within {within:?}"));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Its process id.
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Its resident memory in bytes, from `ps`.
    pub async fn rss(&self) -> Option<u64> {
        rss_of(self.pid()?).await
    }

    /// Its open file descriptors, from `lsof` (None without it).
    pub async fn open_files(&self) -> Option<u64> {
        let pid = self.pid()?;
        let o = Command::new("lsof")
            .args(["-n", "-P", "-p", &pid.to_string()])
            .output()
            .await
            .ok()?;
        let lines = String::from_utf8_lossy(&o.stdout).lines().count();
        u64::try_from(lines.saturating_sub(1)).ok()
    }

    /// Stops it with SIGTERM (a clean shutdown: the current group finishes, SQLite checkpoints its
    /// WAL as the last connection closes), at most `within`, else kills it.
    ///
    /// # Errors
    ///
    /// It could not be signalled.
    pub async fn stop(mut self, within: Duration) -> Result<Launch, String> {
        if let Some(pid) = self.pid() {
            let _ = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status()
                .await;
            let waited = tokio::time::timeout(within, self.child.wait()).await;
            if waited.is_err() {
                let _ = self.child.kill().await;
            }
        }
        Ok(self.launch)
    }

    /// Restarts it with `launch` (a new clock start, a bump), keeping the tokens.
    ///
    /// # Errors
    ///
    /// As [`Server::start`].
    pub async fn restart(self, launch: Launch, ready_within: Duration) -> Result<Server, String> {
        let tokens = self.tokens.clone();
        let env = self.env.clone();
        let _ = self.stop(Duration::from_secs(60)).await?;
        Server::spawn(launch, tokens, env, ready_within).await
    }

    /// Starts a server on an existing database with known tokens (after a stop).
    ///
    /// # Errors
    ///
    /// As [`Server::start`].
    pub async fn resume(
        launch: Launch,
        tokens: Vec<String>,
        env: Vec<(String, String)>,
        ready_within: Duration,
    ) -> Result<Server, String> {
        Server::spawn(launch, tokens, env, ready_within).await
    }

    /// The token environment, to resume after a stop.
    #[must_use]
    pub fn env(&self) -> Vec<(String, String)> {
        self.env.clone()
    }
}

/// The resident memory of process `pid` in bytes, from `ps`.
pub async fn rss_of(pid: u32) -> Option<u64> {
    let o = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .await
        .ok()?;
    String::from_utf8_lossy(&o.stdout)
        .trim()
        .parse::<u64>()
        .ok()
        .map(|kb| kb * 1_024)
}

/// The bytes of a stopped server's database file once its WAL is checkpointed into it (the bench
/// subcommand `forum-server checkpoint`): the server reuses up to 1 GiB of WAL while it serves, so
/// the WAL's size is not the history's. The file and its WAL, if the checkpoint fails.
pub async fn file_bytes(bin: &Path, db: &Path) -> u64 {
    let o = Command::new(bin)
        .arg("checkpoint")
        .arg("--db")
        .arg(db)
        .output()
        .await;
    o.ok()
        .filter(|o| o.status.success())
        .and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok())
        .and_then(|v| v.get("bytes").and_then(Value::as_u64))
        .unwrap_or_else(|| db_bytes(db))
}

/// The size of a database: the file and its WAL, in bytes.
#[must_use]
pub fn db_bytes(db: &Path) -> u64 {
    let size = |p: PathBuf| std::fs::metadata(p).map_or(0, |m| m.len());
    let mut wal = db.as_os_str().to_owned();
    wal.push("-wal");
    size(db.to_path_buf()) + size(PathBuf::from(wal))
}

/// The writer's counters, as the server writes them once a second.
#[derive(Clone, Debug, Default)]
pub struct StatsLine {
    /// Driver time when the line was read, from the run's start.
    pub at: Duration,
    /// The line.
    pub v: Value,
}

impl StatsLine {
    /// A counter.
    #[must_use]
    pub fn n(&self, key: &str) -> u64 {
        self.v.get(key).and_then(Value::as_u64).unwrap_or(0)
    }
}

/// Follows the stats file: each poll reads the lines written since the last.
pub struct StatsTail {
    path: PathBuf,
    offset: u64,
    origin: Instant,
    /// Every line read, with the driver time it was read at.
    pub lines: Vec<StatsLine>,
}

impl StatsTail {
    /// Follows `path` from its current end, timing lines from `origin`.
    #[must_use]
    pub fn new(path: &Path, origin: Instant) -> StatsTail {
        let offset = std::fs::metadata(path).map_or(0, |m| m.len());
        StatsTail {
            path: path.to_path_buf(),
            offset,
            origin,
            lines: Vec::new(),
        }
    }

    /// Reads the new lines.
    pub fn poll(&mut self) {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut f) = std::fs::File::open(&self.path) else {
            return;
        };
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut buf = String::new();
        if f.read_to_string(&mut buf).is_err() {
            return;
        }
        // Keep a partial last line for the next poll.
        let complete = buf.rfind('\n').map_or(0, |i| i + 1);
        self.offset += u64::try_from(complete).unwrap_or(0);
        let at = now().saturating_duration_since(self.origin);
        for l in buf[..complete].lines() {
            if let Ok(v) = serde_json::from_str::<Value>(l) {
                self.lines.push(StatsLine { at, v });
            }
        }
    }

    /// The last line read at or before `t` (driver time), else the first.
    #[must_use]
    pub fn at(&self, t: Duration) -> Option<&StatsLine> {
        self.lines
            .iter()
            .rev()
            .find(|l| l.at <= t)
            .or_else(|| self.lines.first())
    }

    /// The writer's busy share between two driver times: busy ns over the server's own elapsed
    /// time between the lines read then.
    #[must_use]
    pub fn utilisation(&self, from: Duration, to: Duration) -> Option<f64> {
        let a = self.at(from)?;
        let b = self.at(to)?;
        let dt = b.n("t_ms").saturating_sub(a.n("t_ms"));
        if dt == 0 {
            return None;
        }
        let busy = b.n("busy_ns").saturating_sub(a.n("busy_ns"));
        #[allow(clippy::cast_precision_loss, reason = "a share for a report")]
        Some(busy as f64 / (dt as f64 * 1e6))
    }

    /// The largest busy share over consecutive one-second lines between two driver times.
    #[must_use]
    pub fn peak_utilisation(&self, from: Duration, to: Duration) -> Option<f64> {
        let w: Vec<&StatsLine> = self
            .lines
            .iter()
            .filter(|l| l.at >= from && l.at <= to)
            .collect();
        w.windows(2)
            .filter_map(|p| {
                let dt = p[1].n("t_ms").saturating_sub(p[0].n("t_ms"));
                let busy = p[1].n("busy_ns").saturating_sub(p[0].n("busy_ns"));
                #[allow(clippy::cast_precision_loss, reason = "a share for a report")]
                (dt > 0).then(|| busy as f64 / (dt as f64 * 1e6))
            })
            .reduce(f64::max)
    }

    /// The change of a counter between two driver times.
    #[must_use]
    pub fn delta(&self, key: &str, from: Duration, to: Duration) -> u64 {
        match (self.at(from), self.at(to)) {
            (Some(a), Some(b)) => b.n(key).saturating_sub(a.n(key)),
            _ => 0,
        }
    }

    /// The largest value of a gauge between two driver times.
    #[must_use]
    pub fn max(&self, key: &str, from: Duration, to: Duration) -> u64 {
        self.lines
            .iter()
            .filter(|l| l.at >= from && l.at <= to)
            .map(|l| l.n(key))
            .max()
            .unwrap_or(0)
    }

    /// The last line.
    #[must_use]
    pub fn last(&self) -> Option<&StatsLine> {
        self.lines.last()
    }

    /// The server's search times of `kind` (`hello`, `find`, `create`, or `from_user`) reported by
    /// the lines read after `from` and at or before `to` (the bench feature's `search_us`), only
    /// those of the text `text` when given.
    #[must_use]
    pub fn searches(&self, kind: &str, from: Duration, to: Duration, text: Option<&str>) -> Hist {
        let want = text.map(text_tag);
        let mut h = Hist::new();
        for l in self.lines.iter().filter(|l| l.at > from && l.at <= to) {
            for s in
                l.v.get("search_us")
                    .and_then(|x| x.get(kind))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
            {
                let us = s.get(0).and_then(Value::as_u64).unwrap_or(0);
                let tag = s.get(1).and_then(Value::as_u64);
                if want.is_none() || tag == want {
                    h.record_us(us);
                }
            }
        }
        h
    }

    /// A number of every line, by the forum day of the line (its `sim_ms` from the first line's),
    /// for a key of `sizes` (`["sizes", "sessions"]`) or of the line itself (`["parked"]`).
    #[must_use]
    pub fn by_day(&self, key: &[&str]) -> Vec<(f64, f64)> {
        let first = self.lines.first().map_or(0, |l| l.n("sim_ms"));
        self.lines
            .iter()
            .filter_map(|l| {
                let v = key.iter().try_fold(&l.v, |v, k| v.get(*k))?.as_u64()?;
                #[allow(clippy::cast_precision_loss, reason = "days and counts for a report")]
                let day = l.n("sim_ms").saturating_sub(first) as f64 / 86_400_000.0;
                #[allow(clippy::cast_precision_loss, reason = "days and counts for a report")]
                let v = v as f64;
                Some((day, v))
            })
            .collect()
    }

    /// A value of the last line read, at a path of keys.
    #[must_use]
    pub fn last_value(&self, key: &[&str]) -> Option<Value> {
        let l = self.lines.last()?;
        key.iter().try_fold(&l.v, |v, k| v.get(*k)).cloned()
    }
}

/// The tag the server gives a search text (the bench feature's `text_tag`): FNV-1a, 32 bits.
#[must_use]
pub fn text_tag(text: &str) -> u64 {
    let mut h: u32 = 0x811c_9dc5;
    for b in text.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    u64::from(h)
}

/// Moves the running server's clock `ms` forward at once (the bench feature's jump file, beside
/// its stats file); the server applies it within about 100 ms.
///
/// # Errors
///
/// The file could not be written.
pub fn jump_clock(stats: &Path, ms: u64) -> Result<(), String> {
    let mut f = stats.as_os_str().to_owned();
    f.push(".jump");
    let file = PathBuf::from(f);
    let mut tmp = file.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, ms.to_string()).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &file).map_err(|e| format!("{}: {e}", file.display()))
}

/// The lines of the server's log that name a file-descriptor or accept error (gate G-12).
#[must_use]
pub fn fd_errors_logged(dir: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(dir.join("server.log")).unwrap_or_default();
    text.lines()
        .filter(|l| {
            let l = l.to_ascii_lowercase();
            l.contains("too many open files")
                || l.contains("emfile")
                || l.contains("accept error")
                || l.contains("failed to accept")
        })
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tail_reads_whole_lines_and_measures_utilisation() {
        let dir = std::env::temp_dir().join(format!("swarm-sim-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("stats.jsonl");
        std::fs::write(&p, "{\"t_ms\":0,\"busy_ns\":0}\n").unwrap();
        let mut t = StatsTail::new(&p, now());
        assert!(t.lines.is_empty(), "it starts at the end");
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        use std::io::Write;
        write!(
            f,
            "{{\"t_ms\":1000,\"busy_ns\":100000000}}\n{{\"t_ms\":2000,\"busy_ns\":400000000}}\n{{\"t_ms\""
        )
        .unwrap();
        t.poll();
        assert_eq!(t.lines.len(), 2, "the partial line waits");
        let end = t.lines[1].at;
        let u = t.utilisation(Duration::ZERO, end).unwrap();
        assert!((u - 0.3).abs() < 1e-9, "{u}");
        let peak = t.peak_utilisation(Duration::ZERO, end).unwrap_or(0.0);
        assert!(peak == 0.0 || (peak - 0.3).abs() < 1e-9);
        assert_eq!(t.delta("busy_ns", Duration::ZERO, end), 300_000_000);
        writeln!(f, ":3000,\"busy_ns\":500000000}}").unwrap();
        t.poll();
        assert_eq!(t.lines.len(), 3);
        assert_eq!(t.last().unwrap().n("t_ms"), 3000);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(db_bytes(&dir.join("none.db")), 0);
        // The server's tag of a text (forum-server's bench::text_tag).
        assert_eq!(text_tag(""), 0x811c_9dc5);
        assert_eq!(text_tag("a"), 0xe40c_292c);
    }
}
