//! The scale gates of design D.12.8 that run against the server over MCP: G-1 (latency), G-2
//! (writer utilisation, from the same run), G-4 (wake-ups), G-5 (settle storm), G-6 (memory over
//! simulated days), G-7 (startup after simulated days), G-8 (growth of the database), G-9
//! (search), G-10 (history replay), and G-12 (connections). G-3 (the core's cost test) and G-11
//! (the race test of D.12.7) are cargo tests that `scripts/bench` runs.
//!
//! Each gate runs at a scale: `full` is the gate as D.12.8 specifies it, for the reference
//! hardware; `smoke` is the same scenario scaled down to finish in minutes on a laptop, and its
//! numbers say so. A gate prints its numbers with the limits of D.12.8 and pass or fail, and
//! writes them as JSON with the machine they ran on.
//!
//! Should a command of the mix be a stub of the server (none is since WP-D7 merged), the numbers
//! that depend on the mix (latencies, the writer's load, bytes per call) are **provisional**: a
//! verdict says so, with the share of stubbed calls. A stubbed answer is in no latency histogram
//! and in no served-call count.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::agent::{Agent, ROOT, Shared};
use crate::clock::{Instant, now, unix_millis, unix_seconds};
use crate::hist::Hist;
use crate::machine::{Machine, describe, machine};
use crate::mcp::Outcome;
use crate::mix::{Action, BASE, HEAVY, HIGH, Profile};
use crate::record::{Category, Counts, StubPolicy};
use crate::run::{Burst, Phase, Spec, run, seed};
use crate::server::{Launch, Server, StatsTail, fd_errors_logged, file_bytes, jump_clock, rss_of};
use crate::text;

/// A gate's scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scale {
    /// Scaled down for a laptop.
    Smoke,
    /// As D.12.8 specifies, for the reference hardware.
    Full,
}

impl Scale {
    /// Its name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Scale::Smoke => "smoke",
            Scale::Full => "full",
        }
    }

    fn pick<T>(self, smoke: T, full: T) -> T {
        match self {
            Scale::Smoke => smoke,
            Scale::Full => full,
        }
    }
}

/// How to run a gate.
#[derive(Clone, Debug)]
pub struct Options {
    /// The gate: `g1`, `g2`, `g4` to `g10`, or `g12`.
    pub gate: String,
    /// The scale.
    pub scale: Scale,
    /// The `forum-server` binary built with the `bench` feature.
    pub bin: PathBuf,
    /// Where the gate keeps its databases and logs.
    pub work: PathBuf,
    /// Where the JSON result goes.
    pub out: Option<PathBuf>,
    /// The stub policy.
    pub stubs: StubPolicy,
    /// The seed.
    pub seed: u64,
    /// Overrides the number of simulated agents.
    pub agents: Option<usize>,
    /// Overrides the simulated days (G-6, G-7, G-10).
    pub days: Option<f64>,
    /// Overrides the length of the main phase, in seconds.
    pub seconds: Option<u64>,
    /// Overrides the documents indexed (G-9).
    pub docs: Option<usize>,
    /// Keeps the gate's databases afterwards (its logs and counters are always kept).
    pub keep: bool,
}

/// One check of a gate: a number against its bound, or a number recorded with none.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Check {
    /// What it measures.
    pub name: String,
    /// The number, if it could be measured.
    pub value: Option<f64>,
    /// Its unit.
    pub unit: String,
    /// `<=`, `>=`, or empty for a number recorded with no bound.
    pub op: String,
    /// The bound of D.12.8.
    pub limit: Option<f64>,
    /// Pass or fail; None when not measured yet.
    pub pass: Option<bool>,
    /// Why it is not measured, or what qualifies it.
    pub note: Option<String>,
}

impl Check {
    fn at_most(name: &str, value: f64, limit: f64, unit: &str) -> Check {
        Check {
            name: name.into(),
            value: Some(value),
            unit: unit.into(),
            op: "<=".into(),
            limit: Some(limit),
            pass: Some(value <= limit),
            note: None,
        }
    }

    fn at_least(name: &str, value: f64, limit: f64, unit: &str) -> Check {
        Check {
            name: name.into(),
            value: Some(value),
            unit: unit.into(),
            op: ">=".into(),
            limit: Some(limit),
            pass: Some(value >= limit),
            note: None,
        }
    }

    fn info(name: &str, value: f64, unit: &str) -> Check {
        Check {
            name: name.into(),
            value: Some(value),
            unit: unit.into(),
            op: String::new(),
            limit: None,
            pass: None,
            note: None,
        }
    }

    fn not_yet(name: &str, note: &str) -> Check {
        Check {
            name: name.into(),
            value: None,
            unit: String::new(),
            op: String::new(),
            limit: None,
            pass: None,
            note: Some(note.into()),
        }
    }

    fn note(mut self, note: &str) -> Check {
        self.note = Some(note.into());
        self
    }

    /// A percentile check over `h`, with the sample's size in its note.
    fn p_at_most(name: &str, h: &Hist, q: f64, limit: f64) -> Check {
        Check::at_most(name, ms(h, q), limit, "ms").note(&format!("n = {}", h.len()))
    }

    /// A percentile recorded with no bound, with the sample's size in its note.
    fn p_info(name: &str, h: &Hist, q: f64) -> Check {
        Check::info(name, ms(h, q), "ms").note(&format!("n = {}", h.len()))
    }
}

/// A gate's result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Verdict {
    /// `G-1` and so on.
    pub gate: String,
    /// What it protects.
    pub title: String,
    /// The scale.
    pub scale: Scale,
    /// Pass, fail, or None when nothing could be checked yet.
    pub pass: Option<bool>,
    /// Why the numbers are provisional, if they are: the stubbed share of the mix.
    #[serde(default)]
    pub provisional: Vec<String>,
    /// The checks.
    pub checks: Vec<Check>,
    /// Everything else measured.
    pub numbers: Value,
    /// What qualifies the numbers.
    pub notes: Vec<String>,
    /// The machine.
    pub machine: Machine,
    /// When it started (Unix seconds).
    pub started_unix: u64,
    /// How long it took, in seconds.
    pub seconds: f64,
}

impl Verdict {
    fn new(gate: &str, title: &str, o: &Options, m: &Machine, started: (u64, Instant)) -> Verdict {
        Verdict {
            gate: gate.into(),
            title: title.into(),
            scale: o.scale,
            pass: None,
            provisional: Vec::new(),
            checks: Vec::new(),
            numbers: Value::Null,
            notes: Vec::new(),
            machine: m.clone(),
            started_unix: started.0,
            seconds: now().saturating_duration_since(started.1).as_secs_f64(),
        }
    }

    fn decide(mut self) -> Verdict {
        let any_fail = self.checks.iter().any(|c| c.pass == Some(false));
        let any_pass = self.checks.iter().any(|c| c.pass == Some(true));
        self.pass = if any_fail {
            Some(false)
        } else if any_pass {
            Some(true)
        } else {
            None
        };
        self
    }

    /// Notes the stubbed calls of `c`, and makes the verdict provisional while there are any:
    /// they are part of the mix the numbers stand for, but the server did none of their work.
    fn stubs(&mut self, c: &Counts) {
        if c.stubbed == 0 {
            return;
        }
        self.notes.push(format!(
            "{} of {} calls ({:.1} %) were answered as stubs, {} of them writes (commands this \
             server does not do yet); they are in no latency, served-call, or bytes-per-call \
             number (--stubs count); --stubs require fails them",
            c.stubbed,
            c.calls,
            c.stub_share() * 100.0,
            c.stubbed_writes
        ));
        self.provisional.push(format!(
            "{:.1} % of the mix's calls are stubs: the latencies, the writer's load, and the \
             bytes per call stand for the real mix only once those commands are merged",
            c.stub_share() * 100.0
        ));
    }

    /// The console form: the checks with their limits, and the result.
    #[must_use]
    pub fn render(&self) -> String {
        let mut s = format!(
            "== {} {} ({}) ==\nmachine: {}\n",
            self.gate,
            self.title,
            self.scale.name(),
            describe(&self.machine)
        );
        s.push_str(&format!(
            "  {:<66} {:>14} {:>16}  {}\n",
            "check", "value", "limit", "result"
        ));
        for c in &self.checks {
            let v = c
                .value
                .map_or("-".to_string(), |v| format!("{} {}", fmt(v), c.unit));
            let l = c
                .limit
                .map_or(String::new(), |l| format!("{} {} {}", c.op, fmt(l), c.unit));
            let r = match c.pass {
                Some(true) => "pass",
                Some(false) => "FAIL",
                None if c.value.is_some() => "recorded",
                None => "not yet",
            };
            s.push_str(&format!("  {:<66} {:>14} {:>16}  {}", c.name, v, l, r));
            if let Some(n) = &c.note {
                s.push_str(&format!("  ({n})"));
            }
            s.push('\n');
        }
        for n in &self.notes {
            s.push_str(&format!("  note: {n}\n"));
        }
        for p in &self.provisional {
            s.push_str(&format!("  PROVISIONAL: {p}\n"));
        }
        s.push_str(&format!(
            "{} {}: {}{} in {:.0} s\n",
            self.gate,
            self.scale.name(),
            match self.pass {
                Some(true) => "PASS",
                Some(false) => "FAIL",
                None => "NOT YET",
            },
            if self.provisional.is_empty() {
                ""
            } else {
                " (provisional)"
            },
            self.seconds
        ));
        s
    }
}

fn fmt(v: f64) -> String {
    if v != 0.0 && (v.abs() >= 1e7 || v.abs() < 1e-3) {
        format!("{v:.3e}")
    } else if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.3}")
    }
}

fn ms(h: &Hist, q: f64) -> f64 {
    #[allow(clippy::cast_precision_loss, reason = "microseconds as milliseconds")]
    let v = h.quantile_us(q) as f64 / 1_000.0;
    v
}

#[allow(clippy::cast_precision_loss, reason = "counts as floats for a report")]
fn f(x: u64) -> f64 {
    x as f64
}

#[allow(clippy::cast_precision_loss, reason = "counts as floats for a report")]
fn fu(x: usize) -> f64 {
    x as f64
}

/// `a / b`, or 0 when `b` is 0.
fn share(a: f64, b: f64) -> f64 {
    if b == 0.0 { 0.0 } else { a / b }
}

const GIB: f64 = 1_073_741_824.0;
/// presence.silence (§22): a session's silence falls due this long after its last presence.
const SILENCE_MIN: u64 = 15;
/// presence.resolution (§22): presence is refreshed at most this often, so a silence may fall
/// this much later.
const PRESENCE_RESOLUTION_MIN: u64 = 3;
/// lease.default (§22), the longest lease a gate's holds get.
const LEASE_MIN: u64 = 20;
/// FINISHED_RETAIN (D.4.12): finished entities leave memory this many days after they finished.
const FINISHED_RETAIN_DAYS: f64 = 7.0;
/// DIGEST_RETAIN (D.4.12): a digest entry never shown to its principal leaves memory this many days
/// after it was made. The mixes have no principal's session to show one, so every entry waits it.
const DIGEST_RETAIN_DAYS: f64 = 28.0;
/// COMMIT_LINGER (D.11.2), in milliseconds: a group takes the jobs that arrive within it of its
/// first, so a writing group lasts at least that long, and its commit's sync follows.
const COMMIT_LINGER_MS: f64 = 2.0;

/// The most commits a second the group commit allows on a machine whose commit takes `commit_ms`:
/// one per COMMIT_LINGER and commit, so at most 500 a second (D.11.2, gate G-4).
fn commit_ceiling(commit_ms: f64) -> f64 {
    1_000.0 / (COMMIT_LINGER_MS + commit_ms.max(0.0))
}

/// Runs a gate and returns its verdicts (G-1 also returns G-2's, from the same run).
///
/// # Errors
///
/// The server did not start, or the gate is unknown.
pub async fn gate(o: &Options) -> Result<Vec<Verdict>, String> {
    let dir = o.work.join(format!("{}-{}", o.gate, o.scale.name()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let m = machine(&dir, Some(&o.bin));
    let started = (unix_seconds(), now());
    let result = match o.gate.as_str() {
        "g1" | "g2" => g1(o, &dir, &m, started).await,
        "g4" => Ok(vec![g4(o, &dir, &m, started).await?]),
        "g5" => Ok(vec![g5(o, &dir, &m, started).await?]),
        "g6" => Ok(vec![g6(o, &dir, &m, started).await?]),
        "g7" => Ok(vec![g7(o, &dir, &m, started).await?]),
        "g8" => Ok(vec![g8(o, &dir, &m, started).await?]),
        "g9" => Ok(vec![g9(o, &dir, &m, started).await?]),
        "g10" => Ok(vec![g10(o, &dir, &m, started).await?]),
        "g12" => Ok(vec![g12(o, &dir, &m, started).await?]),
        other => Err(format!("unknown gate {other:?}")),
    };
    if !o.keep {
        // The databases of a full run are large; the logs and the counters stay.
        remove_databases(&dir);
    }
    result
}

fn remove_databases(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for f in entries.flatten() {
        let name = f.file_name().to_string_lossy().to_string();
        if name.starts_with("forum.db") || name.starts_with("forum-search.db") {
            let _ = std::fs::remove_file(f.path());
        } else if f.path().is_dir() {
            remove_databases(&f.path());
        }
    }
}

async fn start(o: &Options, dir: &Path, scale: u64) -> Result<Server, String> {
    let mut l = Launch::new(&o.bin, dir)?;
    l.scale = scale;
    if scale > 1 {
        // queue.max_wait (10 s, §22) is read on the server's clock: on a compressed clock it
        // stays 10 s of real time, as a writer queue is real.
        l.params.push(format!(
            "queue_max_wait = {}",
            10_000_u64.saturating_mul(scale)
        ));
    }
    Server::start(l, Duration::from_secs(300)).await
}

/// A spec with the gate's common settings.
fn spec(o: &Options, server: &Server, agents: usize, profile: Profile, scale: u64) -> Spec {
    let mut s = Spec::new(
        &server.launch.url(),
        server.tokens.clone(),
        o.agents.unwrap_or(agents),
        profile,
    );
    s.seed = o.seed;
    s.stubs = o.stubs;
    #[allow(clippy::cast_precision_loss, reason = "a clock factor")]
    {
        s.scale = scale as f64;
    }
    s
}

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

fn minutes_ms(m: u64) -> u64 {
    m.saturating_mul(60_000)
}

// ---------------------------------------------------------------------------------------------
// G-1 and G-2

/// G-1: 10,000 agents, the base mix for an hour, then the heavy mix with 3× bursts for 15
/// minutes, with a broadcast every minute and a pause of the root followed by a herd of `control
/// health`. G-2: the writer's utilisation during it.
#[allow(clippy::too_many_lines, reason = "one gate's run and its checks")]
async fn g1(
    o: &Options,
    dir: &Path,
    m: &Machine,
    started: (u64, Instant),
) -> Result<Vec<Verdict>, String> {
    let full = o.scale == Scale::Full;
    // Smoke: a tenth of the agents on a clock ten times as fast make full's call rate, with
    // sessions, checkpoints, and leases at their forum-time pace.
    let clock = o.scale.pick(10, 1);
    let server = start(o, dir, clock).await?;
    let mut s = spec(o, &server, o.scale.pick(1_000, 10_000), BASE, clock);
    s.backlog = o.scale.pick(3_000, 100_000);
    s.seeders = o.scale.pick(32, 64);
    s.ramp = secs(o.scale.pick(20, 300));
    s.warmup = secs(o.scale.pick(20, 300));
    let minute = secs(60 / clock);
    let base = secs(o.seconds.unwrap_or(o.scale.pick(120, 3_600)));
    let heavy_len = secs(o.scale.pick(60, 900));
    s.phases = vec![
        Phase::base("base", base),
        Phase {
            name: "heavy".into(),
            duration: heavy_len,
            mix: HEAVY,
            burst: Some(Burst {
                period: secs(o.scale.pick(20, 300)),
                length: secs(o.scale.pick(7, 60)),
            }),
            broadcast_every: Some(minute),
            pause_at: Some(secs(o.scale.pick(20, 300))),
            pause_for: secs(o.scale.pick(20, 180)),
            herd_window: minute,
        },
    ];
    let tail = StatsTail::new(&server.launch.stats(), now());
    let out = run(&s, Some(tail)).await;
    let rss = server.rss().await;
    let _ = server.stop(secs(60)).await;
    let rec = &out.shared.rec;
    let measured = ["base", "heavy"];
    let cat = |c| rec.category(&measured, c);
    let (write, read, next, hello, health) = (
        cat(Category::Write),
        cat(Category::Read),
        cat(Category::Next),
        cat(Category::Hello),
        rec.kind(&["heavy"], "control.health"),
    );
    let all = rec.all_calls(&measured);
    let reads_without_search = rec.category_without(&measured, Category::Read, "view.find_text");
    let counts = rec.counts(&measured);
    let base_c = rec.counts(&["base"]);
    let mut v1 = Verdict::new("G-1", "latency", o, m, started);
    v1.checks = vec![
        Check::p_at_most("p99 of writes", &write, 0.99, 50.0),
        Check::p_at_most("p99 of reads", &read, 0.99, 20.0),
        Check::p_info(
            "p99 of reads other than view find {text} (a search)",
            &reads_without_search,
            0.99,
        ),
        Check::p_info(
            "p99 of view find {text}",
            &rec.kind(&measured, "view.find_text"),
            0.99,
        ),
        Check::p_at_most("p99 of hold next", &next, 0.99, 30.0),
        Check::p_at_most("p99 of session hello, with search", &hello, 0.99, 100.0),
        Check::p_at_most("p99.9 of every call", &all, 0.999, 250.0),
        Check::at_most("unexpected error codes", f(counts.unexpected), 0.0, "calls"),
    ];
    let stats = out.stats.as_ref();
    if let (Some(st), Some(&(h0, h1))) = (stats, out.herds.first()) {
        // HEALTH_REFRESH is 10 s of the forum's time; the herd's window, one minute of it.
        #[allow(clippy::cast_precision_loss, reason = "a small count")]
        let allowed = (minute.as_secs_f64() * clock as f64 / 10.0).ceil() + 2.0;
        let computed = st.delta("health_computed", h0, h1 + secs(1));
        v1.checks.push(
            Check::at_most(
                "health computations during the herd",
                f(computed),
                allowed,
                "",
            )
            .note("one per HEALTH_REFRESH, whatever the herd"),
        );
    }
    v1.checks.push(Check::p_at_most(
        "p99 of control health in the heavy phase (herd included)",
        &health,
        0.99,
        20.0,
    ));
    // What a call is: a forum tool call, waits and stubbed answers included, MCP handshakes left
    // out (record.rs); D.15.2's 250 a second at base counts so.
    v1.checks.extend([
        Check::info(
            "calls a second, base (waits and stubs included, handshakes left out)",
            base_c.rate(base_c.calls),
            "",
        ),
        Check::info(
            "  of them served (stubs and transport failures left out)",
            base_c.rate(base_c.served()),
            "",
        ),
        Check::info(
            "  of them without waits",
            base_c.rate(base_c.calls.saturating_sub(base_c.waits)),
            "",
        ),
        Check::info(
            "writes the server did a second, base",
            base_c.rate(base_c.writes),
            "",
        ),
        Check::info(
            "writes answered as stubs a second, base (not done)",
            base_c.rate(base_c.stubbed_writes),
            "",
        ),
        Check::info(
            "reads among the calls, waits counted as reads (D.15.2: 55 %)",
            share(f(counts.reads), f(counts.calls)),
            "share",
        ),
        Check::info(
            "reads among the calls, waits left out",
            share(
                f(counts.reads.saturating_sub(counts.waits)),
                f(counts.calls.saturating_sub(counts.waits)),
            ),
            "share",
        ),
    ]);
    // The mix per session against D.15.2's counts (1.3 next and 1.5 view node per review session,
    // so 1.15 view node per next).
    let hellos = f(rec.calls_of(&["base"], "session.hello"));
    let nexts = f(rec.calls_of(&["base"], "hold.next"));
    let views = f(rec.calls_of(&["base"], "view.node"));
    v1.checks.extend([
        Check::info(
            "view node per hold next, base (D.15.2: 1.15)",
            share(views, nexts),
            "",
        ),
        Check::info("hold next per session, base", share(nexts, hellos), ""),
        Check::info("view node per session, base", share(views, hellos), ""),
    ]);
    if let Some(st) = stats
        && let Some((a, b)) = out.window("base").zip(out.window("heavy"))
    {
        let h = st.searches("hello", a.0, b.1, None);
        v1.checks.push(Check::p_info(
            "p99 of the server's search for a hello (in the call above)",
            &h,
            0.99,
        ));
    }
    if let Some(r) = rss {
        v1.checks
            .push(Check::info("server memory at the end", f(r) / GIB, "GiB"));
    }
    v1.notes.push(
        "a pause of / turns health red (Paused at the root, D.8.22); the herd and its cost run \
         regardless"
            .into(),
    );
    v1.notes.push(
        "a call counts in the phase of its intended time; the calls of the last burst still \
         unanswered when the run gave up on them count as late (transport: unfinished)"
            .into(),
    );
    v1.notes.push(
        "every response the core computed carries the next calls of §20.9 (WP-D7): computing and \
         rendering them is in the latencies"
            .into(),
    );
    v1.stubs(&counts);
    if !full {
        v1.notes.push(format!(
            "smoke: {} agents on a clock {clock} times as fast (full's call rate with a tenth of \
             the sessions), 2 min of base and 1 min of heavy with bursts",
            s.agents
        ));
    }
    v1.numbers = json!({"run": out.report(&s), "errors": counts.outcomes});
    let mut v2 = Verdict::new("G-2", "writer utilisation", o, m, started);
    match stats {
        Some(st) => {
            let base_u = out
                .window("base")
                .and_then(|(a, b)| st.utilisation(a, b))
                .unwrap_or(f64::NAN);
            let burst_u = out
                .bursts
                .iter()
                .filter_map(|&(a, b)| st.utilisation(a, b))
                .reduce(f64::max)
                .unwrap_or(f64::NAN);
            let heavy_u = out
                .window("heavy")
                .and_then(|(a, b)| st.utilisation(a, b))
                .unwrap_or(f64::NAN);
            let heavy_c = rec.counts(&["heavy"]);
            v2.checks = vec![
                Check::at_most("writer busy share, base mix (mean)", base_u, 0.30, ""),
                Check::at_most(
                    "writer busy share, heaviest burst (mean)",
                    burst_u,
                    0.60,
                    "",
                ),
                Check::info("writer busy share, heavy phase (mean)", heavy_u, ""),
                Check::info(
                    "jobs per commit, heavy phase",
                    out.window("heavy").map_or(0.0, |(a, b)| {
                        let c = st.delta("commits", a, b).max(1);
                        f(st.delta("jobs", a, b)) / f(c)
                    }),
                    "",
                ),
                Check::info(
                    "writes the server did a second, base",
                    base_c.rate(base_c.writes),
                    "",
                ),
                Check::info(
                    "writes the server did a second, heavy",
                    heavy_c.rate(heavy_c.writes),
                    "",
                ),
                Check::info(
                    "writes answered as stubs a second, heavy (not done)",
                    heavy_c.rate(heavy_c.stubbed_writes),
                    "",
                ),
            ];
        }
        None => v2
            .checks
            .push(Check::not_yet("writer busy share", "no bench stats")),
    }
    v2.numbers = json!({"bursts": out.bursts.iter().map(|(a, b)| [a.as_secs_f64(), b.as_secs_f64()]).collect::<Vec<_>>()});
    v2.notes.push(
        "the same run as G-1: the writer's busy time from the bench feature's counters".into(),
    );
    v2.stubs(&counts);
    Ok(vec![v1.decide(), v2.decide()])
}

// ---------------------------------------------------------------------------------------------
// Scripted roles

/// Opens `n` agents (a new MCP and forum session each), at most `width` at a time.
async fn open_agents(sh: &Arc<Shared>, n: usize, base: usize, width: usize) -> Vec<Agent> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < n {
        let end = (start + width).min(n);
        let mut tasks = Vec::new();
        for i in start..end {
            let sh = Arc::clone(sh);
            tasks.push(tokio::spawn(async move {
                let mut a = Agent::new(sh, base + i, Duration::from_secs(300)).ok()?;
                a.open().await.ok()?;
                Some(a)
            }));
        }
        for t in tasks {
            if let Ok(Some(a)) = t.await {
                out.push(a);
            }
        }
        start = end;
    }
    out
}

/// How scripted waiters wait.
#[derive(Clone)]
struct WaitHow {
    /// The `timeout` of each wait.
    timeout: Duration,
    /// A node to wait `for`. Should `talk wait` answer as a stub (as it did until WP-D5b), the
    /// waiter reads the node before its next wait instead: the read marks the node shown, so that
    /// its next wait parks until the next append, as a wait that reported the news would.
    node: Option<String>,
    /// One call in this many is a read of a node rather than a wait (the mix's parked sessions
    /// read now and then, which keeps their presence; D.15.2).
    read_every: Option<u32>,
    /// The pace of those reads: a read is intended this long after the call before it.
    read_after: Duration,
}

/// The (sent, answered) times of calls, shared by the tasks that make them.
type Spans = Arc<Mutex<Vec<(Instant, Instant)>>>;

/// Waiters: each loops `talk wait` until `until`, or until `stop` is set, logging every wait's
/// sent and done times. A wait answered at once as a stub is followed by a pause of its timeout,
/// as if it had waited, so that a stub does not turn a parked session into a busy loop.
fn spawn_waiters(
    agents: Vec<Agent>,
    until: Instant,
    how: &WaitHow,
    log: &Spans,
    stop: &Arc<AtomicBool>,
) -> Vec<tokio::task::JoinHandle<()>> {
    agents
        .into_iter()
        .map(|mut a| {
            let log = Arc::clone(log);
            let how = how.clone();
            let stop = Arc::clone(stop);
            tokio::spawn(async move {
                let mut read = false;
                let mut k = 0_u32;
                while now() < until && !stop.load(Ordering::Relaxed) {
                    k += 1;
                    if how.read_every.is_some_and(|e| k.is_multiple_of(e)) {
                        let at = now() + how.read_after;
                        tokio::time::sleep_until(at.min(until)).await;
                        if now() >= until {
                            break;
                        }
                        a.act(Action::ViewNode, at).await;
                        continue;
                    }
                    if read && let Some(n) = &how.node {
                        let mut m = Map::new();
                        m.insert("path".into(), n.clone().into());
                        a.call_with(Action::ViewNode, m, now()).await;
                    }
                    let mut m = Map::new();
                    m.insert(
                        "timeout".into(),
                        format!("{}s", how.timeout.as_secs().max(1)).into(),
                    );
                    if let Some(n) = &how.node {
                        m.insert("for".into(), n.clone().into());
                    }
                    let s = now();
                    let r = a.call_with(Action::Wait, m, s).await;
                    let d = now();
                    if let Ok(mut l) = log.lock() {
                        l.push((s, d));
                    }
                    let stub = r.outcome == Outcome::Stubbed;
                    // A stub tells the waiter nothing: it reads the node before its next wait.
                    read = stub;
                    if stub
                        && how.node.is_none()
                        && d.saturating_duration_since(s) < Duration::from_millis(100)
                    {
                        let left = until.saturating_duration_since(now());
                        tokio::time::sleep(how.timeout.min(left)).await;
                    }
                }
            })
        })
        .collect()
}

/// A writer: takes a node and checkpoints it every `every` (Poisson) until `until`, at its
/// intended times, with a new session after 100 writes (`write.per_hour`); logs the intended
/// (sent) time and the answer's time of each checkpoint that succeeded.
fn spawn_writer(
    mut a: Agent,
    target: Option<String>,
    every: Duration,
    until: Instant,
    log: Option<Spans>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut at = now();
        let mut writes = 0;
        let mut holding = false;
        while now() < until {
            if !holding || writes >= 100 {
                if writes >= 100 {
                    a.act(Action::Release, now()).await;
                    if a.open().await.is_err() {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        continue;
                    }
                    writes = 0;
                }
                let r = match &target {
                    Some(t) => {
                        let mut m = Map::new();
                        m.insert("target".into(), t.clone().into());
                        Some(a.call_with(Action::Take, m, now()).await)
                    }
                    None => a.act(Action::Next, now()).await,
                };
                writes += 1;
                holding = r.is_some_and(|r| r.outcome == Outcome::Ok) && !a.held().is_empty();
                if !holding {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
                at = now();
            }
            // Poisson arrivals: writers that start together do not stay in step.
            let gap = a.rng().exp(every.as_secs_f64());
            at += Duration::from_secs_f64(gap);
            tokio::time::sleep_until(at).await;
            let r = a.act(Action::Checkpoint, at).await;
            writes += 1;
            if let (Some(l), Some(r)) = (&log, &r)
                && r.outcome == Outcome::Ok
                && let Ok(mut l) = l.lock()
            {
                l.push((at, now()));
            }
            if a.held().is_empty() {
                holding = false;
            }
        }
    })
}

/// Probes: each calls at `every` intervals (reads, with a create one time in fifty) until
/// `until`, and not before `from`.
fn spawn_probes(
    agents: Vec<Agent>,
    from: Instant,
    every: Duration,
    until: Instant,
) -> Vec<tokio::task::JoinHandle<()>> {
    agents
        .into_iter()
        .enumerate()
        .map(|(i, mut a)| {
            tokio::spawn(async move {
                // Spread over the first interval, so that the probes do not call in step.
                #[allow(clippy::cast_precision_loss, reason = "a fraction of an interval")]
                let offset = every.mul_f64((i % 97) as f64 / 97.0);
                let mut at = from + offset;
                let mut k = 0_u32;
                while at < until {
                    tokio::time::sleep_until(at).await;
                    k += 1;
                    // One write in 50 calls keeps a probe below write.per_hour on a fast clock.
                    let act = match k % 50 {
                        0 => Action::Create,
                        x if x % 3 == 0 => Action::FindText,
                        x if x % 3 == 1 => Action::List,
                        _ => Action::ViewNode,
                    };
                    a.act(act, at).await;
                    // Probes never hold: a create's hold is handed back at once.
                    for _ in 0..a.held().len() {
                        a.act(Action::Release, now()).await;
                    }
                    at += every;
                }
            })
        })
        .collect()
}

/// Waits for `tasks`, all of them within `within`; those still running then are aborted.
async fn join(tasks: Vec<tokio::task::JoinHandle<()>>, within: Duration) {
    let deadline = now() + within;
    for t in tasks {
        let abort = t.abort_handle();
        if tokio::time::timeout(deadline.saturating_duration_since(now()), t)
            .await
            .is_err()
        {
            abort.abort();
        }
    }
}

/// Polls `tail` every 250 ms until `until`.
async fn poll_until(tail: &mut StatsTail, until: Instant) {
    while now() < until {
        tokio::time::sleep(Duration::from_millis(250)).await;
        tail.poll();
    }
}

// ---------------------------------------------------------------------------------------------
// G-4

/// The latency of each wait an append woke, as an upper bound, and how many waits ran their whole
/// timeout or ended with no append to explain them. `waits` and `appends` are (sent, answered)
/// pairs; a wait that lasted `timeout` less half a second or more ran out. A wait's latency runs
/// from the intended time of the first append that may have committed after the wait was sent
/// (the earliest its commit can have been), or, when every append before its answer was answered
/// before the wait was sent, from the wait's own sending (the news was there when it parked).
fn wake_latencies(
    waits: &[(Instant, Instant)],
    appends: &[(Instant, Instant)],
    timeout: Duration,
) -> (Hist, u64, u64) {
    let mut sorted = appends.to_vec();
    sorted.sort();
    let mut lat = Hist::new();
    let (mut timed_out, mut unexplained) = (0_u64, 0_u64);
    for (sent, done) in waits {
        if done.saturating_duration_since(*sent) + Duration::from_millis(500) >= timeout {
            timed_out += 1;
            continue;
        }
        // The first append sent before the wait's answer whose commit may have come after the
        // wait was sent: its answer came after the wait's sending.
        let after = sorted
            .iter()
            .filter(|(a_sent, a_done)| a_done > sent && a_sent < done)
            .map(|(a_sent, _)| *a_sent)
            .min();
        match after {
            Some(a) => lat.record(done.saturating_duration_since(a.max(*sent))),
            None if sorted.first().is_some_and(|(a_sent, _)| a_sent < sent) => {
                lat.record(done.saturating_duration_since(*sent));
            }
            None => unexplained += 1,
        }
    }
    (lat, timed_out, unexplained)
}

/// G-4: `talk wait` with parked sessions under a commit load (part A), and parked sessions all
/// waiting for one node that gets 10 appends a second (part B).
#[allow(
    clippy::too_many_lines,
    reason = "one gate's two parts and their checks"
)]
async fn g4(
    o: &Options,
    dir: &Path,
    m: &Machine,
    started: (u64, Instant),
) -> Result<Verdict, String> {
    let server = start(o, dir, 1).await?;
    let mut s = spec(o, &server, 0, BASE, 1);
    s.parked_share = 0.0;
    let sh = s.shared("seed");
    let parked = o.agents.unwrap_or(o.scale.pick(1_000, 5_000));
    // D.12.8 asks for 500 commits a second, which is the group commit's ceiling (one commit per
    // COMMIT_LINGER) before any sync: no machine reaches it. Full drives the writes until the
    // commits reach 90 % of what the group commit allows with this machine's measured commit (500
    // at most); smoke, until 200 a second.
    let ceiling = commit_ceiling(m.commit_p50_ms.unwrap_or(1.0)).min(500.0);
    let target = o.scale.pick(200.0, 0.9 * ceiling);
    // The ramp stops a tenth above the target (or halfway to the ceiling, if that is nearer): the
    // measured window comes after the ramp's own, and a ramp that stopped at the target itself
    // left it to the window's noise (a smoke run ramped to 208 a second, then measured 196).
    let ramp_to = (target * 1.1).min(f64::midpoint(target, ceiling.max(target)));
    let first_writers = o.scale.pick(100, 250);
    let most_writers = o.scale.pick(1_000, 5_000);
    let per_writer = Duration::from_millis(500); // two writes a second each
    let part = secs(o.seconds.unwrap_or(o.scale.pick(30, 300)));
    let part_b = secs(o.seconds.unwrap_or(o.scale.pick(30, 120)));
    seed(&sh, most_writers + 50, 32, secs(300)).await;
    let mut tail = StatsTail::new(&server.launch.stats(), sh.start);
    let at = |t: Instant| t.saturating_duration_since(sh.start);

    // Part A: parked sessions that nothing addresses, and writers added until the commits reach
    // `ramp_to`.
    let a_waiters = open_agents(&sh, parked, 10_000, 256).await;
    let mut a_writers = open_agents(&sh, first_writers, 20_000, 128).await;
    let a0 = now();
    sh.rec.phase("a", a0);
    let waits = Arc::new(Mutex::new(Vec::new()));
    // The waiters park until the end of part A, whatever the ramp takes.
    let how_a = WaitHow {
        timeout: secs(50),
        node: None,
        read_every: None,
        read_after: Duration::ZERO,
    };
    let stop_a = a0 + secs(3_600);
    let end_a = Arc::new(AtomicBool::new(false));
    let waiter_tasks = spawn_waiters(a_waiters, stop_a, &how_a, &waits, &end_a);
    let mut writer_count = 0;
    let mut ramp_steps = Vec::new();
    let mut commits_rate = 0.0;
    let mut writer_tasks = Vec::new();
    for step in 0..10 {
        for w in a_writers.drain(..) {
            writer_tasks.push(spawn_writer(w, None, per_writer, stop_a, None));
            writer_count += 1;
        }
        let t0 = now();
        poll_until(&mut tail, t0 + secs(6)).await;
        let (w0, w1) = (at(t0) + secs(1), at(now()));
        commits_rate =
            f(tail.delta("commits", w0, w1)) / (w1.saturating_sub(w0)).as_secs_f64().max(1e-9);
        ramp_steps.push(
            json!({"step": step, "writers": writer_count, "commits_per_second": commits_rate}),
        );
        if commits_rate >= ramp_to || writer_count >= most_writers {
            break;
        }
        let more = (writer_count / 2).max(1).min(most_writers - writer_count);
        a_writers = open_agents(&sh, more, 20_000 + writer_count, 128).await;
    }
    let m0 = now();
    let a1 = m0 + part;
    poll_until(&mut tail, a1).await;
    let (wa0, wa1) = (at(m0) + secs(1), at(a1));
    sh.rec.phase("between", now());
    // Part A ends: the writers stop now, and the waiters after their wait in flight (at most
    // talk.wait_max), so that none is still parked in part B.
    for t in writer_tasks.drain(..) {
        t.abort();
    }
    end_a.store(true, Ordering::Relaxed);
    join(waiter_tasks, secs(90)).await;
    tail.poll();

    // Part B: one hot node, its appenders, and the parked sessions waiting for it.
    let hot = format!("{ROOT}/hot-{}", unix_millis());
    let appenders = 10;
    let mut creator = open_agents(&sh, 1, 30_000, 1).await;
    if let Some(c) = creator.first_mut() {
        let mut args = Map::new();
        args.insert("path".into(), hot.clone().into());
        args.insert("title".into(), "the hot node of gate G-4".into());
        args.insert("capacity".into(), (appenders + 2).into());
        c.call_with(Action::Create, args, now()).await;
        for _ in 0..c.held().len() {
            c.act(Action::Release, now()).await;
        }
    }
    let appends = Arc::new(Mutex::new(Vec::new()));
    // A baseline of the appenders alone: the events one append writes.
    let app_agents = open_agents(&sh, appenders, 40_000, appenders).await;
    let b_pre = now();
    sh.rec.phase("b_baseline", b_pre);
    let b_end = b_pre + secs(10) + part_b;
    let mut tasks: Vec<_> = app_agents
        .into_iter()
        .map(|a| {
            spawn_writer(
                a,
                Some(hot.clone()),
                secs(1),
                b_end,
                Some(Arc::clone(&appends)),
            )
        })
        .collect();
    poll_until(&mut tail, b_pre + secs(10)).await;
    let base_appends = appends.lock().map_or(0, |l| l.len());
    let base_events = tail.delta("events", at(b_pre) + secs(2), at(now()));
    let b_waiters = open_agents(&sh, parked, 50_000, 256).await;
    let b0 = now();
    sh.rec.phase("b", b0);
    let bwaits = Arc::new(Mutex::new(Vec::new()));
    let appends_before = appends.lock().map_or(0, |l| l.len());
    let wait_timeout = secs(50);
    let how_b = WaitHow {
        timeout: wait_timeout,
        node: Some(hot.clone()),
        read_every: None,
        read_after: Duration::ZERO,
    };
    let never = Arc::new(AtomicBool::new(false));
    tasks.extend(spawn_waiters(b_waiters, b_end, &how_b, &bwaits, &never));
    poll_until(&mut tail, b_end + secs(2)).await;
    let (wb0, wb1) = (at(b0) + secs(2), at(b_end));
    sh.rec.phase("drain", now());
    sh.stop.store(true, Ordering::SeqCst);
    join(tasks, secs(120)).await;
    sh.rec.finish(now());
    tail.poll();
    let rss = server.rss().await;
    let _ = server.stop(secs(60)).await;

    // Part A's numbers.
    let busy_a = tail.delta("busy_ns", wa0, wa1);
    let wake_a = tail.delta("wake_ns", wa0, wa1);
    let commits_a = tail.delta("commits", wa0, wa1).max(1);
    let jobs_a = tail.delta("jobs", wa0, wa1);
    let examined_a = tail.delta("wake_examined", wa0, wa1);
    let concerned_a = tail.delta("wake_concerned", wa0, wa1);
    let dt_a = (wa1.saturating_sub(wa0)).as_secs_f64().max(1e-9);
    // Part B's.
    let busy_b = tail.delta("busy_ns", wb0, wb1);
    let wake_b = tail.delta("wake_ns", wb0, wb1);
    let events_b = tail.delta("events", wb0, wb1);
    let wakes_b = tail.delta("wakes", wb0, wb1);
    let commits_b = tail.delta("commits", wb0, wb1).max(1);
    let examined_b = tail.delta("wake_examined", wb0, wb1);
    let concerned_b = tail.delta("wake_concerned", wb0, wb1);
    let appended: Vec<(Instant, Instant)> =
        appends.lock().map_or_else(|_| Vec::new(), |l| l.clone());
    let appends_b = appended.len().saturating_sub(appends_before);
    let per_append = if base_appends == 0 {
        0.0
    } else {
        f(base_events) / fu(base_appends)
    };
    let waited = bwaits.lock().map_or_else(|_| Vec::new(), |l| l.clone());
    let (lat, timed_out, unexplained) = wake_latencies(&waited, &appended, wait_timeout);
    let mut v = Verdict::new("G-4", "talk wait wake-ups", o, m, started);
    v.checks = vec![
        Check::info(
            "A: parked sessions (most at once)",
            f(tail.max("parked", wa0, wa1)),
            "",
        ),
        Check::at_least("A: commits a second", f(commits_a) / dt_a, target, "").note(&format!(
            "{}; {writer_count} writers at two writes a second",
            if o.scale == Scale::Full {
                format!(
                    "90 % of the group commit's ceiling here, 1/(COMMIT_LINGER + the commit) = \
                     {ceiling:.0} a second; D.12.8 asks for 500, the ceiling with no sync"
                )
            } else {
                "smoke's own target".to_string()
            }
        )),
        Check::info("A: jobs a second", f(jobs_a) / dt_a, ""),
        Check::at_most(
            "A: parked sessions the wake-ups examined, per commit",
            f(examined_a) / f(commits_a),
            f(concerned_a) / f(commits_a),
            "",
        )
        .note("at most the waits the commits concern (here none): readiness work per commit"),
        Check::at_most(
            "A: wake work, share of the writer",
            share(f(wake_a), f(busy_a)),
            0.05,
            "",
        ),
        Check::info("A: writer busy share", f(busy_a) / (dt_a * 1e9), ""),
        Check::not_yet(
            "A: wake latency p99 of addressed waits <= 100 ms",
            "notes and agent questions that address a session are WP-D5b's",
        ),
        Check::info(
            "B: parked sessions on the hot node (most at once)",
            f(tail.max("parked", wb0, wb1)),
            "",
        ),
        Check::info("B: appends on the hot node", fu(appends_b), ""),
        Check::info("B: woken waits", f(wakes_b), ""),
    ];
    // Should talk wait answer as a stub (as it did until WP-D5b), it records no `since`: a
    // waiter cannot be told the news, most of its waits find news at once instead of parking,
    // and its reads (which mark the node shown) add events. Part B's numbers are then recorded,
    // not checked.
    let b_stub = sh
        .rec
        .counts(&["b"])
        .outcomes
        .get("talk.wait: stubbed")
        .copied()
        .unwrap_or(0)
        > 0;
    let gated = |c: Check| {
        if b_stub {
            let mut c = Check::info(&c.name, c.value.unwrap_or(f64::NAN), &c.unit);
            c.note = Some("not checked: talk wait answered as a stub".into());
            c
        } else {
            c
        }
    };
    v.checks.extend([
        gated(
            Check::at_most(
                "B: parked sessions the wake-ups examined, per commit",
                f(examined_b) / f(commits_b),
                f(concerned_b) / f(commits_b),
                "",
            )
            .note("at most the waits the commits concern (the hot node's)"),
        ),
        gated(Check::p_at_most(
            "B: wake latency p99, within WAIT_COALESCE (1 s) + 100 ms",
            &lat,
            0.99,
            1_100.0,
        )),
        gated(Check::at_most(
            "B: events per woken wait (they append none)",
            if wakes_b == 0 {
                0.0
            } else {
                (f(events_b) - per_append * fu(appends_b)).max(0.0) / f(wakes_b)
            },
            0.01,
            "",
        )),
        gated(Check::at_most(
            "B: wake work, share of the writer",
            share(f(wake_b), f(busy_b)),
            0.05,
            "",
        )),
        gated(Check::at_most(
            "B: writer busy share (G-2's burst bound)",
            f(busy_b) / ((wb1.saturating_sub(wb0)).as_secs_f64().max(1e-9) * 1e9),
            0.60,
            "",
        )),
    ]);
    if let Some(r) = rss {
        v.checks
            .push(Check::info("server memory at the end", f(r) / GIB, "GiB"));
    }
    let counts = sh.rec.counts(&["a", "b"]);
    v.stubs(&counts);
    v.notes.push(format!(
        "part A added writers until the commits reached {ramp_to:.0} a second, a margin over \
         the target ({} steps); a waiter of part \
         B reads the hot node before each wait (the read marks it shown); of part B's waits, \
         {timed_out} ran their whole timeout and {unexplained} ended with no append to explain \
         them; a wake's latency runs from the intended time of the first append that may have \
         committed after the wait was sent (an upper bound)",
        ramp_steps.len()
    ));
    v.numbers = json!({
        "recorded": sh.rec.report(),
        "ramp": ramp_steps,
        "commits_per_second_at_ramp_end": commits_rate,
        "events_per_append": per_append,
        "wake_latency": lat.summary(),
        "errors": counts.outcomes,
    });
    Ok(v.decide())
}

// ---------------------------------------------------------------------------------------------
// G-5

/// G-5: a storm of Silence and Lease deadlines at one instant, while probes keep calling. The
/// server's clock runs at real time while the sessions open and take their holds; then it jumps
/// past every deadline at once (the bench feature's jump), so that every one falls due together.
async fn g5(
    o: &Options,
    dir: &Path,
    m: &Machine,
    started: (u64, Instant),
) -> Result<Verdict, String> {
    let server = start(o, dir, 1).await?;
    let sessions = o.agents.unwrap_or(o.scale.pick(2_000, 10_000));
    let holders = sessions / 2;
    let s = spec(o, &server, 0, BASE, 1);
    let sh = s.shared("seed");
    seed(&sh, holders + 200, 32, secs(300)).await;
    let probes = open_agents(&sh, 20, 90_000, 20).await;
    let mut tail = StatsTail::new(&server.launch.stats(), sh.start);
    sh.rec.phase("open", now());
    let storm_len = secs(o.seconds.unwrap_or(o.scale.pick(30, 60)));
    // Open the sessions as fast as the server takes them; half take a hold each.
    let o0 = now();
    let mut opened = Vec::new();
    let mut i = 0;
    while i < sessions {
        let end = (i + 256).min(sessions);
        let mut batch = Vec::new();
        for k in i..end {
            let sh2 = Arc::clone(&sh);
            batch.push(tokio::spawn(async move {
                let mut a = Agent::new(sh2, 100_000 + k, secs(120)).ok()?;
                a.open().await.ok()?;
                if k < holders {
                    a.act(Action::Next, now()).await;
                }
                Some(a.held().len())
            }));
        }
        for b in batch {
            if let Ok(Some(h)) = b.await {
                opened.push(h);
            }
        }
        i = end;
    }
    let o1 = now();
    let held: usize = opened.iter().sum();
    // The probes call from now on; after a few seconds of calm, the clock jumps past the last
    // deadline: the longest lease, the silence with presence's resolution, the time the opening
    // took, and a minute.
    let calm = secs(5);
    let jump_at = now() + calm;
    let probe_until = jump_at + storm_len + secs(10);
    let tasks = spawn_probes(probes, now(), Duration::from_millis(100), probe_until);
    tokio::time::sleep_until(jump_at).await;
    let opening_ms = u64::try_from(o1.saturating_duration_since(o0).as_millis()).unwrap_or(0);
    let jump_ms = minutes_ms(LEASE_MIN.max(SILENCE_MIN + PRESENCE_RESOLUTION_MIN) + 1)
        + opening_ms
        + u64::try_from(calm.as_millis()).unwrap_or(0);
    tail.poll();
    let events_before = tail.last().map_or(0, |x| x.n("events"));
    sh.rec.phase("storm", now());
    jump_clock(&server.launch.stats(), jump_ms)?;
    let storm1 = now() + storm_len;
    poll_until(&mut tail, storm1).await;
    sh.rec.phase("after", now());
    let jumped = tail.last().map_or(0, |x| x.n("jumped_ms"));
    // Events from the line read just before the jump to the last: the firings and the probes'.
    let fired = tail
        .last()
        .map_or(0, |x| x.n("events"))
        .saturating_sub(events_before);
    // The server's first second with the jump applied: the firings came at once if most are there.
    let first_second = tail
        .lines
        .iter()
        .position(|l| l.n("jumped_ms") > 0)
        .and_then(|i| {
            Some(
                tail.lines[i]
                    .n("events")
                    .saturating_sub(tail.lines.get(i.checked_sub(1)?)?.n("events")),
            )
        })
        .unwrap_or(0);
    sh.stop.store(true, Ordering::SeqCst);
    join(tasks, secs(60)).await;
    sh.rec.finish(now());
    tail.poll();
    let _ = server.stop(secs(60)).await;
    let h = sh.rec.all_calls(&["storm"]);
    let mut v = Verdict::new("G-5", "settle storm", o, m, started);
    v.checks = vec![
        Check::info(
            "sessions that fall silent at the jump",
            fu(opened.len()),
            "",
        ),
        Check::info("holds whose leases end at the jump", fu(held), ""),
        Check::info(
            "seconds the sessions took to open (before the jump)",
            o1.saturating_duration_since(o0).as_secs_f64(),
            "s",
        ),
        Check::at_least(
            "the clock jumped past every deadline",
            f(jumped),
            f(jump_ms),
            "ms",
        ),
        Check::at_least(
            "events during the storm, one at least per deadline",
            f(fired),
            fu(opened.len() + held),
            "",
        )
        .note("each firing appends its Fired"),
        Check::info(
            "events in the server's first second after the jump",
            f(first_second),
            "",
        ),
        Check::p_at_most("longest wait of a call during the storm", &h, 1.0, 1_000.0),
        Check::p_at_most("p99 of calls during the storm", &h, 0.99, 250.0),
    ];
    if o.scale == Scale::Smoke {
        v.notes.push(format!(
            "smoke: {sessions} silences and {holders} leases rather than 10,000 and 5,000, and a \
             storm window of {} s",
            storm_len.as_secs()
        ));
    }
    v.notes.push(format!(
        "the clock ran at real time while the sessions opened, then jumped {jump_ms} ms forward \
         at once, so every Silence and Lease deadline fell due at the same instant; 20 probes \
         called every 100 ms throughout"
    ));
    let counts = sh.rec.counts(&["storm"]);
    v.stubs(&counts);
    v.numbers = json!({"recorded": sh.rec.report(), "errors": counts.outcomes});
    Ok(v.decide())
}

// ---------------------------------------------------------------------------------------------
// G-6, G-7, and G-10: days of forum time on a compressed clock

struct Long {
    server: Server,
    spec: Spec,
    out: crate::run::Outcome,
    /// (forum days, resident bytes), sampled through the run.
    rss: Vec<(f64, u64)>,
}

/// Runs `profile` for `days` of forum time on a clock `clock` times as fast.
async fn long_run(
    o: &Options,
    dir: &Path,
    profile: Profile,
    agents: usize,
    clock: u64,
    days: f64,
    backlog: usize,
) -> Result<Long, String> {
    let server = start(o, dir, clock).await?;
    let mut s = spec(o, &server, agents, profile, clock);
    s.backlog = backlog;
    s.seeders = 16;
    s.ramp = secs(2);
    s.warmup = Duration::ZERO;
    #[allow(clippy::cast_precision_loss, reason = "a clock factor")]
    let real = Duration::from_secs_f64(days * 86_400.0 / clock as f64);
    s.phases = vec![Phase::base("load", real)];
    let pid = server.pid().unwrap_or(0);
    let stop = Arc::new(AtomicBool::new(false));
    let samples = Arc::new(Mutex::new(Vec::new()));
    let sampler = {
        let stop = Arc::clone(&stop);
        let samples = Arc::clone(&samples);
        let t0 = now();
        // One sample per hour of forum time.
        #[allow(clippy::cast_precision_loss, reason = "a clock factor")]
        let every = Duration::from_secs_f64((3_600.0 / clock as f64).max(0.5));
        tokio::spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                if let Some(r) = rss_of(pid).await {
                    #[allow(clippy::cast_precision_loss, reason = "a clock factor")]
                    let day =
                        now().saturating_duration_since(t0).as_secs_f64() * clock as f64 / 86_400.0;
                    if let Ok(mut v) = samples.lock() {
                        v.push((day, r));
                    }
                }
                tokio::time::sleep(every).await;
            }
        })
    };
    let tail = StatsTail::new(&server.launch.stats(), now());
    let out = run(&s, Some(tail)).await;
    stop.store(true, Ordering::Relaxed);
    let _ = sampler.await;
    let rss = samples.lock().map_or_else(|_| Vec::new(), |v| v.clone());
    Ok(Long {
        server,
        spec: s,
        out,
        rss,
    })
}

#[allow(clippy::cast_precision_loss, reason = "durations for a report")]
fn estimate(days: f64, clock: u64) -> String {
    let h = days * 86_400.0 / clock as f64 / 3_600.0;
    format!("{days} days of forum time at {clock} times real time take {h:.1} h of real time")
}

/// The least-squares slope, per day, of the points whose day lies in `[from, to)`.
fn slope(points: &[(f64, f64)], from: f64, to: f64) -> Option<f64> {
    let w: Vec<&(f64, f64)> = points.iter().filter(|p| p.0 >= from && p.0 < to).collect();
    if w.len() < 3 {
        return None;
    }
    let n = fu(w.len());
    let mx = w.iter().map(|p| p.0).sum::<f64>() / n;
    let my = w.iter().map(|p| p.1).sum::<f64>() / n;
    let sxx = w.iter().map(|p| (p.0 - mx).powi(2)).sum::<f64>();
    let sxy = w.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum::<f64>();
    (sxx > 0.0).then(|| sxy / sxx)
}

/// Whether a series stops growing after `retain` days: its growth a day over `[retain + 0.5,
/// end]` against its growth a day over `[0.5, retain)`, at most a fifth of it; a series that
/// hardly grew before (by under 1 % of its level a day) passes when it grows by at most 1 % of
/// its level a day after. `None` when the run is shorter than twice `retain`, the tail as long as
/// the retention.
fn flat_after(name: &str, points: &[(f64, f64)], retain: f64, unit: &str) -> Check {
    let end = points.last().map_or(0.0, |p| p.0);
    if end < 2.0 * retain {
        return Check::not_yet(
            name,
            &format!(
                "the run's {end:.1} forum days leave no tail as long as the retention ({retain} \
                 days) after it"
            ),
        );
    }
    let level = points
        .iter()
        .filter(|p| p.0 >= retain)
        .map(|p| p.1)
        .fold(0.0, f64::max)
        .max(1.0);
    let (Some(before), Some(after)) = (
        slope(points, 0.5, retain),
        slope(points, retain + 0.5, end + 1.0),
    ) else {
        return Check::not_yet(name, "too few samples");
    };
    if before <= 0.01 * level {
        return Check::at_most(
            &format!("{name}: growth a day after day {retain}, share of its level"),
            after / level,
            0.01,
            "",
        )
        .note(&format!(
            "it hardly grew before either ({before:.3} {unit} a day); level {level:.0} {unit}"
        ));
    }
    Check::at_most(
        &format!("{name}: growth a day after day {retain}, share of the growth before"),
        after / before,
        0.2,
        "",
    )
    .note(&format!(
        "{after:.3} against {before:.3} {unit} a day; flat once what finished leaves memory"
    ))
}

/// G-6: memory over 30 days of forum time at the base profile.
#[allow(clippy::too_many_lines, reason = "one gate's run and its checks")]
async fn g6(
    o: &Options,
    dir: &Path,
    m: &Machine,
    started: (u64, Instant),
) -> Result<Verdict, String> {
    // Smoke runs twice FINISHED_RETAIN and a day, so that a tail as long as the retention follows
    // it.
    let days = o.days.unwrap_or(o.scale.pick(15.0, 30.0));
    let clock = o.scale.pick(1_500, 8);
    eprintln!("G-6: {}", estimate(days, clock));
    let l = long_run(
        o,
        dir,
        BASE,
        o.scale.pick(30, 10_000),
        clock,
        days,
        o.scale.pick(1_000, 100_000),
    )
    .await?;
    // After a clean shutdown the WAL is checkpointed into the file (D.11.2 keeps up to 1 GiB of
    // it while serving).
    let launch = l.server.stop(secs(120)).await?;
    let db = file_bytes(&launch.bin, &launch.db()).await;
    let peak = l.rss.iter().map(|x| x.1).max().unwrap_or(0);
    let rss_mib: Vec<(f64, f64)> = l
        .rss
        .iter()
        .map(|(d, b)| (*d, f(*b) / 1_048_576.0))
        .collect();
    let mut v = Verdict::new("G-6", "memory over forum days", o, m, started);
    v.checks = vec![
        Check::at_most(
            "peak resident memory of the server",
            f(peak) / GIB,
            10.0,
            "GiB",
        ),
        flat_after("resident memory", &rss_mib, FINISHED_RETAIN_DAYS, "MiB"),
    ];
    // The collections a retention bounds (D.12.8): sessions and op records (24 h), notes and agent
    // questions (until their `until`, or FINISHED_RETAIN), digest entries (DIGEST_RETAIN unshown),
    // and the shell's maps (presence.silence); each must stop growing. The others are recorded.
    let stats = l.out.stats.as_ref();
    let mut sizes = Map::new();
    if let Some(st) = stats {
        for (key, name) in [
            ("sessions", "sessions"),
            ("ops", "op records"),
            ("notes", "notes"),
            ("aquestions", "agent questions"),
            ("digest", "digest entries"),
            ("shell_bindings", "the shell's session bindings"),
            ("shell_news", "the shell's news map"),
            ("shell_shown", "the shell's versions shown (1.7)"),
            ("shell_windows", "the shell's write windows"),
            ("entries", "entry metadata"),
        ] {
            let series = st.by_day(&["sizes", key]);
            let last = series.last().map_or(0.0, |p| p.1);
            sizes.insert(
                key.into(),
                json!(
                    series
                        .iter()
                        .step_by(24)
                        .map(|p| [p.0, p.1])
                        .collect::<Vec<_>>()
                ),
            );
            if series.iter().all(|p| p.1 == 0.0) {
                v.checks.push(
                    Check::info(&format!("{name}: largest count"), 0.0, "")
                        .note("none were made by this run's mix"),
                );
            } else {
                let retain = if key == "digest" {
                    DIGEST_RETAIN_DAYS
                } else {
                    FINISHED_RETAIN_DAYS
                };
                v.checks.push(flat_after(name, &series, retain, ""));
                v.checks
                    .push(Check::info(&format!("{name}: count at the end"), last, ""));
            }
        }
        for key in ["nodes", "holds", "requests", "leaving"] {
            let series = st.by_day(&["sizes", key]);
            v.checks.push(Check::info(
                &format!("{key} in memory at the end"),
                series.last().map_or(0.0, |p| p.1),
                "",
            ));
        }
        // Nodes are not among the entities D.4.12 lets leave memory: a closed node stays, so the
        // nodes the mix creates grow memory with the forum's age (D.4.11 sizes it at 10^5 to 10^6
        // nodes).
        let nodes = st.by_day(&["sizes", "nodes"]);
        let end = nodes.last().map_or(0.0, |p| p.0);
        if let Some(g) = slope(&nodes, FINISHED_RETAIN_DAYS + 0.5, end + 1.0) {
            v.checks.push(
                Check::info("nodes in memory: growth a day after day 7", g, "")
                    .note("closed nodes stay in memory (D.4.12 lists no node)"),
            );
        }
    } else {
        v.checks
            .push(Check::not_yet("the retained collections", "no bench stats"));
    }
    v.checks.extend([
        Check::info("forum days run", days, "days"),
        Check::info("database size at the end", f(db) / GIB, "GiB"),
        Check::info(
            "sessions started",
            f(l.out.shared.sessions.load(Ordering::Relaxed)),
            "",
        ),
    ]);
    let counts = l.out.shared.rec.counts(&["load"]);
    v.checks.push(Check::info("calls", f(counts.calls), ""));
    v.stubs(&counts);
    v.notes.push(estimate(days, clock));
    v.notes.push(
        "resident memory is the process's RSS (ps): the core's state with the shell's maps and \
         the allocator's slack; a series is flat when its growth a day after FINISHED_RETAIN is \
         at most a fifth of its growth before"
            .into(),
    );
    v.numbers = json!({
        "rss_by_day": l.rss.iter().map(|(d, b)| [*d, f(*b)]).collect::<Vec<_>>(),
        "sizes_by_day": sizes,
        "run": l.out.report(&l.spec),
        "errors": counts.outcomes,
    });
    Ok(v.decide())
}

/// What startup read of the history (the bench feature's trace of the writer's statements),
/// from the first stats line of a server just started: (VM steps on the history tables, their
/// statements, all of startup's steps, the history statements' text).
async fn startup_reads(stats: &Path) -> Option<(f64, f64, f64, Value)> {
    let mut tail = StatsTail::new(stats, now());
    let deadline = now() + secs(5);
    while now() < deadline {
        tokio::time::sleep(Duration::from_millis(250)).await;
        tail.poll();
        if let Some(sr) = tail.last_value(&["startup"]).filter(|x| !x.is_null()) {
            let n = |k: &str| sr.get(k).and_then(Value::as_f64).unwrap_or(f64::NAN);
            return Some((
                n("history_steps"),
                n("history_statements"),
                n("steps"),
                sr.get("history").cloned().unwrap_or(Value::Null),
            ));
        }
    }
    None
}

/// G-7: startup after a year of the high profile, and after a `shell_version` bump.
#[allow(clippy::too_many_lines, reason = "one gate's run and its checks")]
async fn g7(
    o: &Options,
    dir: &Path,
    m: &Machine,
    started: (u64, Instant),
) -> Result<Verdict, String> {
    let days = o.days.unwrap_or(o.scale.pick(2.0, 365.0));
    let clock = o.scale.pick(600, 4);
    eprintln!("G-7: {}", estimate(days, clock));
    let l = long_run(
        o,
        dir,
        HIGH,
        o.scale.pick(30, 10_000),
        clock,
        days,
        o.scale.pick(1_000, 100_000),
    )
    .await?;
    let sim_end = l
        .out
        .stats
        .as_ref()
        .and_then(|s| s.last())
        .map_or_else(unix_millis, |x| x.n("sim_ms"));
    let events = l
        .out
        .stats
        .as_ref()
        .and_then(|s| s.last())
        .map_or(0, |x| x.n("events"));
    let tokens = l.server.tokens.clone();
    let env = l.server.env();
    let t = now();
    let launch = l.server.stop(secs(600)).await?;
    let stopped_in = now().saturating_duration_since(t);
    let db = file_bytes(&launch.bin, &launch.db()).await;
    // The clock resumes where the forum's time stopped (T1 would hold it there otherwise).
    let mut again = launch.clone();
    again.clock_start = Some(sim_end + 60_000);
    let s1 = Server::resume(again.clone(), tokens.clone(), env.clone(), secs(1_800)).await?;
    let first = s1.ready_after;
    let reads1 = startup_reads(&launch.stats()).await;
    let rss1 = s1.rss().await;
    let launch = s1.stop(secs(600)).await?;
    let mut bump = launch.clone();
    bump.shell_bump = true;
    bump.clock_start = Some(sim_end + 120_000);
    let s2 = Server::resume(bump, tokens, env, secs(1_800)).await?;
    let second = s2.ready_after;
    let reads2 = startup_reads(&launch.stats()).await;
    let _ = s2.stop(secs(600)).await;
    let mut v = Verdict::new("G-7", "startup after forum days", o, m, started);
    v.checks = vec![
        Check::at_most("serving after a restart", first.as_secs_f64(), 300.0, "s"),
        Check::at_most(
            "serving after a restart with a shell_version bump",
            second.as_secs_f64(),
            300.0,
            "s",
        ),
    ];
    // Reading no history: the statements startup ran on the history tables (events, txs,
    // event_nodes, entries) took at most a bounded lookup's steps, whatever the history's length.
    for (name, r) in [("a restart", &reads1), ("a restart with the bump", &reads2)] {
        match r {
            Some((steps, stmts, all, sql)) => {
                v.checks.push(
                    Check::at_most(
                        &format!(
                            "history read at {name}: VM steps of its statements on the history"
                        ),
                        *steps,
                        1_000.0,
                        "",
                    )
                    .note(&format!(
                        "{stmts} statements on them, {all} steps in all at startup; at most a \
                         few bounded lookups, whatever the {events} events of the history"
                    )),
                );
                v.notes
                    .push(format!("statements on the history at {name}: {sql}"));
            }
            None => v.checks.push(Check::not_yet(
                &format!("history read at {name}"),
                "the server reported no startup trace",
            )),
        }
    }
    v.checks.extend([
        Check::info("forum days run (high profile)", days, "days"),
        Check::info("events in the history", f(events), ""),
        Check::info("database size", f(db) / GIB, "GiB"),
        Check::info("clean shutdown took", stopped_in.as_secs_f64(), "s"),
    ]);
    if let Some(r) = rss1 {
        v.checks.push(Check::info(
            "server memory after the restart",
            f(r) / GIB,
            "GiB",
        ));
    }
    v.notes.push(estimate(days, clock));
    v.notes.push(
        "the bump's shell step rewrites nothing: it times the migration path (a release's own \
         step is that release's to time); the history read at startup is counted by the bench \
         feature's trace of the writer's statements (src/bench.rs)"
            .into(),
    );
    let counts = l.out.shared.rec.counts(&["load"]);
    v.stubs(&counts);
    v.numbers = json!({"run": l.out.report(&l.spec), "errors": counts.outcomes});
    Ok(v.decide())
}

/// G-10: replay of a day of history at the base profile.
async fn g10(
    o: &Options,
    dir: &Path,
    m: &Machine,
    started: (u64, Instant),
) -> Result<Verdict, String> {
    let days = o.days.unwrap_or(o.scale.pick(0.25, 1.0));
    let clock = o.scale.pick(120, 8);
    eprintln!("G-10: {}", estimate(days, clock));
    let l = long_run(
        o,
        dir,
        BASE,
        o.scale.pick(200, 10_000),
        clock,
        days,
        o.scale.pick(1_000, 100_000),
    )
    .await?;
    let launch = l.server.stop(secs(600)).await?;
    let t = now();
    let r = tokio::process::Command::new(&launch.bin)
        .args(["replay-verify", "--db"])
        .arg(launch.db())
        .output()
        .await
        .map_err(|e| e.to_string())?;
    let took = now().saturating_duration_since(t).as_secs_f64();
    let printed: Value =
        serde_json::from_slice(&r.stdout).unwrap_or_else(|_| json!({"equal": false}));
    let equal = r.status.success() && printed["equal"] == true;
    let events = printed["events"].as_u64().unwrap_or(0);
    // D.12.8: 5 × 10⁷ events in at most 15 minutes, single-threaded.
    let projected = if events == 0 {
        f64::INFINITY
    } else {
        took * 5e7 / f(events)
    };
    let mut v = Verdict::new("G-10", "history replay", o, m, started);
    v.checks = vec![
        Check::at_least(
            "the replayed history equals the state tables",
            if equal { 1.0 } else { 0.0 },
            1.0,
            "",
        )
        .note(if equal {
            "store::replay::verify"
        } else {
            "see the error below"
        }),
        Check::info("events replayed", f(events), ""),
        Check::info("replay took", took, "s"),
        Check::at_most(
            "replay time projected to 5 × 10⁷ events (a day at base)",
            projected,
            900.0,
            "s",
        ),
    ];
    if !equal {
        v.notes
            .push(String::from_utf8_lossy(&r.stderr).trim().to_string());
    }
    v.notes.push(estimate(days, clock));
    if events < 50_000_000 {
        v.notes.push(format!(
            "{} agents for {days} forum days wrote {events} events; the replay time is projected \
             to 5 × 10⁷",
            l.spec.agents
        ));
    }
    let counts = l.out.shared.rec.counts(&["load"]);
    v.stubs(&counts);
    v.numbers = json!({"run": l.out.report(&l.spec), "errors": counts.outcomes});
    Ok(v.decide())
}

// ---------------------------------------------------------------------------------------------
// G-8

/// The bytes the commits wrote to the WAL per append on `path`, and the file's growth per append
/// after a checkpoint: `writers` sessions append until `appends` succeeded, on a server resumed
/// on `launch`.
async fn appends_on(
    launch: &Launch,
    tokens: &[String],
    env: &[(String, String)],
    sh: &Arc<Shared>,
    path: &str,
    appends: usize,
) -> Result<(f64, f64, usize), String> {
    let before = file_bytes(&launch.bin, &launch.db()).await;
    let server = Server::resume(launch.clone(), tokens.to_vec(), env.to_vec(), secs(300)).await?;
    let mut tail = StatsTail::new(&launch.stats(), now());
    let writers = open_agents(sh, 4, 71_000, 4).await;
    // Every writer takes the node before the count starts.
    poll_until(&mut tail, now() + Duration::from_millis(1_500)).await;
    let log = Arc::new(Mutex::new(Vec::new()));
    let until = now() + Duration::from_secs(3_600);
    let tasks: Vec<_> = writers
        .into_iter()
        .map(|w| {
            spawn_writer(
                w,
                Some(path.to_string()),
                Duration::from_millis(20),
                until,
                Some(Arc::clone(&log)),
            )
        })
        .collect();
    poll_until(&mut tail, now() + Duration::from_millis(1_500)).await;
    let w0 = tail.last().map_or(0, |x| x.n("wal_bytes"));
    let n0 = log.lock().map_or(0, |l| l.len());
    while log.lock().map_or(0, |l| l.len()) < n0 + appends {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let n1 = log.lock().map_or(0, |l| l.len());
    poll_until(&mut tail, now() + Duration::from_millis(1_500)).await;
    let w1 = tail.last().map_or(0, |x| x.n("wal_bytes"));
    for t in &tasks {
        t.abort();
    }
    let n = n1.saturating_sub(n0).max(1);
    let launch = server.stop(secs(120)).await?;
    let after = file_bytes(&launch.bin, &launch.db()).await;
    Ok((
        f(w1.saturating_sub(w0)) / fu(n),
        f(after.saturating_sub(before)) / fu(n),
        n,
    ))
}

/// G-8: bytes of database per call on the base mix, the months to SQLite's limit, and the bytes
/// written per append on a node with a 64 KiB body and 10⁴ keys against a fresh node.
#[allow(clippy::too_many_lines, reason = "one gate's runs and its checks")]
async fn g8(
    o: &Options,
    dir: &Path,
    m: &Machine,
    started: (u64, Instant),
) -> Result<Verdict, String> {
    let clock = o.scale.pick(10, 1);
    let server = start(o, dir, clock).await?;
    let mut s = spec(o, &server, o.scale.pick(300, 10_000), BASE, clock);
    s.backlog = o.scale.pick(1_000, 100_000);
    s.ramp = secs(o.scale.pick(10, 300));
    s.warmup = Duration::ZERO;
    s.phases = vec![Phase::base(
        "base",
        secs(o.seconds.unwrap_or(o.scale.pick(90, 1_800))),
    )];
    let sh = s.shared("seed");
    seed(&sh, s.backlog, s.seeders, secs(120)).await;
    // The run after the restart goes on from the seed's paths, keys, and fresh numbers: its
    // creates make new work, and its joins join the seed's nodes.
    s.known = sh.known_paths();
    s.known_keys = sh.known_key_list();
    s.first_fresh = sh.next_fresh();
    s.backlog = 0;
    let tokens = server.tokens.clone();
    let env = server.env();
    let launch = server.stop(secs(120)).await?;
    let b0 = file_bytes(&launch.bin, &launch.db()).await;
    let server = Server::resume(launch.clone(), tokens.clone(), env.clone(), secs(300)).await?;
    let out = run(&s, None).await;
    let launch = server.stop(secs(120)).await?;
    let b1 = file_bytes(&launch.bin, &launch.db()).await;
    let counts = out.shared.rec.counts(&["ramp", "base"]);
    // Bytes per served call, waits included (D.15.2's rates count them); stubs and transport
    // failures left out.
    let served = counts.served().max(1);
    let served_no_waits = counts.served().saturating_sub(counts.waits_served).max(1);
    let growth = f(b1.saturating_sub(b0));
    let per_call = growth / f(served);
    // 17.59 TB at 4 KiB pages; 2.63 × 10⁶ seconds a month.
    let months = |rate: f64| 17.59e12 / (per_call * rate * 2.63e6);

    // Appends on a node with a 64 KiB body and 10⁴ keys, and on a fresh node, in a database of
    // their own on a clock at real time: nothing else writes while they are counted.
    let adir = dir.join("appends");
    let aserver = start(o, &adir, 1).await?;
    let atokens = aserver.tokens.clone();
    let aenv = aserver.env();
    let ash = spec(o, &aserver, 0, BASE, 1).shared("appends");
    let stamp = unix_millis();
    let mut paths = Vec::new();
    {
        let mut c = open_agents(&ash, 1, 70_000, 1).await;
        let c = c.first_mut().ok_or("the appends' creator did not open")?;
        for (name, big) in [("fresh", false), ("big", true)] {
            let path = format!("{ROOT}/g8-{name}-{stamp}");
            let mut args = Map::new();
            args.insert("path".into(), path.clone().into());
            args.insert("title".into(), format!("gate G-8, a {name} node").into());
            args.insert("capacity".into(), 4.into());
            if big {
                args.insert("body".into(), text::body(c.rng(), 65_000).into());
                let keys: Vec<String> = (0..10_000)
                    .map(|i| format!("doi:10.5555/g8.{stamp}.{i}"))
                    .collect();
                args.insert("keys".into(), json!(keys));
            }
            let r = c.call_with(Action::Create, args, now()).await;
            if r.outcome != Outcome::Ok {
                return Err(format!(
                    "gate G-8 could not create its {name} node: {:?}",
                    r.outcome
                ));
            }
            for _ in 0..c.held().len() {
                c.act(Action::Release, now()).await;
            }
            paths.push((name, path));
        }
    }
    let alaunch = aserver.stop(secs(120)).await?;
    let appends = o.scale.pick(300, 2_000);
    let mut per_append = Vec::new();
    for (name, path) in &paths {
        let (written, grown, n) =
            appends_on(&alaunch, &atokens, &aenv, &ash, path, appends).await?;
        per_append.push((*name, written, grown, n));
    }
    let fresh = per_append.first().map_or(0.0, |x| x.1);
    let bigb = per_append.get(1).map_or(0.0, |x| x.1);
    let mut v = Verdict::new("G-8", "growth of the database", o, m, started);
    v.checks = vec![
        Check::info(
            "bytes of database per served call, base mix (waits included)",
            per_call,
            "B",
        ),
        Check::info(
            "  per served call without waits",
            growth / f(served_no_waits),
            "B",
        ),
        Check::info(
            "months to 17.59 TB, low (125 calls/s)",
            months(125.0),
            "months",
        ),
        Check::info(
            "months to 17.59 TB, base (250 calls/s)",
            months(250.0),
            "months",
        ),
        Check::info(
            "months to 17.59 TB, high (500 calls/s)",
            months(500.0),
            "months",
        ),
        Check::info("bytes written per append, fresh node (WAL)", fresh, "B"),
        Check::info(
            "bytes written per append, 64 KiB body and 10⁴ keys (WAL)",
            bigb,
            "B",
        ),
        Check::at_most(
            "bytes written per append, big node over fresh node",
            if fresh > 0.0 {
                bigb / fresh
            } else {
                f64::INFINITY
            },
            1.5,
            "",
        )
        .note("D.4.3: an append must not rewrite the node's content row"),
        Check::info(
            "file growth per append, fresh node",
            per_append.first().map_or(0.0, |x| x.2),
            "B",
        ),
        Check::info(
            "file growth per append, big node",
            per_append.get(1).map_or(0.0, |x| x.2),
            "B",
        ),
        Check::info("calls served", f(served), ""),
        Check::info("calls, stubs included", f(counts.calls), ""),
    ];
    v.notes.push(
        "bytes per call: the whole file (state tables, history, the shell's tables, st_ops) after \
         a clean shutdown and a checkpoint of its WAL, before and after, over the calls the \
         server served, waits included as D.15.2's rates count them; the search file is apart; \
         a release that raises the bytes per call by more than 10 % says why"
            .into(),
    );
    v.notes.push(
        "bytes per append: the WAL frames the commits wrote (the bench feature's WAL hook), so \
         that a row rewritten in place counts in full; the file's growth is recorded beside it"
            .into(),
    );
    v.notes.push(
        "the stored responses carry the next calls of §20.9 (WP-D7, at most about 2.3 KB in each \
         st_ops row), which the bytes per call include"
            .into(),
    );
    v.stubs(&counts);
    v.numbers = json!({
        "bytes_before": b0, "bytes_after": b1,
        "appends": per_append.iter().map(|(n, w, g, k)| json!({"node": n, "wal_bytes_per_append": w, "file_growth_per_append": g, "appends": k})).collect::<Vec<_>>(),
        "run": out.report(&s),
        "errors": counts.outcomes,
    });
    Ok(v.decide())
}

// ---------------------------------------------------------------------------------------------
// G-9

/// G-9: `hello` search over many indexed documents, and the writer's busy time with and without
/// concurrent worst-case searches.
#[allow(clippy::too_many_lines, reason = "one gate's phases and its checks")]
async fn g9(
    o: &Options,
    dir: &Path,
    m: &Machine,
    started: (u64, Instant),
) -> Result<Verdict, String> {
    let server = start(o, dir, 1).await?;
    let docs = o.docs.unwrap_or(o.scale.pick(50_000, 100_000_000));
    let s = spec(o, &server, 0, BASE, 1);
    let sh = s.shared("fill");
    // Fill: each create indexes a node's document. A checkpoint indexes nothing (D.11.12: it is
    // no people-facing entry), and the entries that are indexed (a decision, `said`, `heard`, …)
    // are stubs at this commit; counting checkpoints as documents had a smoke run report 50,031
    // documents over a search file of 12,661.
    let fillers = o.scale.pick(32, 128);
    let fill_agents = open_agents(&sh, fillers, 80_000, 64).await;
    let made = Arc::new(AtomicUsize::new(0));
    let t0 = now();
    let mut tasks = Vec::new();
    for mut a in fill_agents {
        let made = Arc::clone(&made);
        let sh2 = Arc::clone(&sh);
        tasks.push(tokio::spawn(async move {
            // A node and its release: one document, and the node stays offered for the writers
            // of the measured phases. Only a create that succeeded counts as a document.
            let mut writes = 0;
            let mut failed = 0;
            while made.load(Ordering::Relaxed) < docs {
                if writes >= 100 {
                    for _ in 0..a.held().len() {
                        a.act(Action::Release, now()).await;
                    }
                    if a.open().await.is_err() {
                        failed += 1;
                        if failed > 100 {
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        continue;
                    }
                    writes = 0;
                }
                let indexed = if a.held().is_empty() {
                    let p = sh2.new_path(a.rng());
                    let mut m = Map::new();
                    m.insert("path".into(), p.into());
                    m.insert("title".into(), text::words(a.rng(), 6).into());
                    m.insert("body".into(), text::words(a.rng(), 60).into());
                    a.call_with(Action::Create, m, now()).await.outcome == Outcome::Ok
                } else {
                    a.act(Action::Release, now()).await;
                    false
                };
                writes += 1;
                if indexed {
                    made.fetch_add(1, Ordering::Relaxed);
                }
            }
            for _ in 0..a.held().len() {
                a.act(Action::Release, now()).await;
            }
        }));
    }
    join(tasks, Duration::from_secs(86_400 * 7)).await;
    let fill_s = now().saturating_duration_since(t0).as_secs_f64();
    // Let the feeder catch up with the history, and the server settle, before measuring.
    tokio::time::sleep(secs(o.scale.pick(15, 120))).await;
    let mut tail = StatsTail::new(&server.launch.stats(), sh.start);
    let at = |t: Instant| t.saturating_duration_since(sh.start);
    let phase = secs(o.seconds.unwrap_or(o.scale.pick(30, 300)));
    // Searchers, each making two calls a second, alternately a `view find {text}` and a new
    // agent's `hello` with that text as its intent: 20 and 200 searches a second.
    let searchers = o.scale.pick(10, 100);
    // The same calls in every phase; only the text differs: the vocabulary's rarest words (short
    // posting lists, a page of hits all the same) or its most common ones (the search's worst
    // case). The phases alternate, so that neither the fill's after-effects nor the order of the
    // phases favours one.
    let (cheap, worst) = (text::rare_words(8), text::common_words(8));
    let plan = [
        ("control-1", &cheap),
        ("search-1", &worst),
        ("control-2", &cheap),
        ("search-2", &worst),
    ];
    let mut util = Vec::new();
    let mut windows = Vec::new();
    let mut bytes = Vec::new();
    for (name, words) in plan {
        let writers = open_agents(&sh, 20, 85_000, 20).await;
        let readers = open_agents(&sh, searchers, 86_000, 64).await;
        let p0 = now();
        sh.rec.phase(name, p0);
        let p1 = p0 + phase;
        let mut tasks: Vec<_> = writers
            .into_iter()
            .map(|w| spawn_writer(w, None, Duration::from_millis(100), p1, None))
            .collect();
        let found = Arc::new(Mutex::new((0_usize, 0_usize)));
        for mut r in readers {
            let words = words.clone();
            let found = Arc::clone(&found);
            tasks.push(tokio::spawn(async move {
                let mut at = now();
                let mut k = 0_u32;
                while now() < p1 {
                    at += Duration::from_millis(500);
                    tokio::time::sleep_until(at).await;
                    k += 1;
                    if k.is_multiple_of(2) && r.reconnect().await.is_ok() {
                        // A new agent's hello, its intent the phase's text.
                        let mut h = Map::new();
                        h.insert("intent".into(), words.clone().into());
                        r.call_with(Action::HelloSearch, h, at).await;
                    } else {
                        let mut m = Map::new();
                        m.insert("text".into(), words.clone().into());
                        let c = r.call_with(Action::FindText, m, at).await;
                        if let Ok(mut fnd) = found.lock() {
                            fnd.0 += c.text.len();
                            fnd.1 += 1;
                        }
                    }
                }
            }));
        }
        poll_until(&mut tail, p1 + secs(1)).await;
        join(tasks, secs(120)).await;
        let (w0, w1) = (at(p0) + secs(2), at(p1));
        util.push((name, tail.utilisation(w0, w1).unwrap_or(f64::NAN)));
        windows.push((name, w0, w1));
        let (b, n) = found.lock().map_or((0, 0), |x| *x);
        bytes.push((name, share(fu(b), fu(n))));
    }
    sh.rec.finish(now());
    let _ = server.stop(secs(120)).await;
    let searches = ["search-1", "search-2"];
    let hello = sh.rec.kind(&searches, "session.hello_search");
    let find = sh.rec.kind(&searches, "view.find_text");
    let mut server_hello = Hist::new();
    let mut server_find = Hist::new();
    let mut control_hello = Hist::new();
    // The searches of the phases' own texts: the writers' new sessions say hello too, with other
    // intents.
    for (name, w0, w1) in &windows {
        if name.starts_with("search") {
            server_hello.merge(&tail.searches("hello", *w0, *w1, Some(&worst)));
            server_find.merge(&tail.searches("find", *w0, *w1, Some(&worst)));
        } else {
            control_hello.merge(&tail.searches("hello", *w0, *w1, Some(&cheap)));
        }
    }
    let mean = |prefix: &str| {
        let v: Vec<f64> = util
            .iter()
            .filter(|(n, u)| n.starts_with(prefix) && u.is_finite())
            .map(|(_, u)| *u)
            .collect();
        if v.is_empty() {
            f64::NAN
        } else {
            v.iter().sum::<f64>() / fu(v.len())
        }
    };
    let (uc, us) = (mean("control"), mean("search"));
    let mut v = Verdict::new("G-9", "search", o, m, started);
    v.checks = vec![
        Check::info(
            "documents written (nodes created)",
            fu(made.load(Ordering::Relaxed)),
            "",
        ),
        Check::info("fill took", fill_s, "s"),
        Check::p_at_most(
            "p99 of the server's search for a worst-case hello intent",
            &server_hello,
            0.99,
            50.0,
        )
        .note(&format!(
            "the search alone, both its lists, measured on the server; n = {}",
            server_hello.len()
        )),
        Check::p_info(
            "p99 of the server's search for a cheap hello intent",
            &control_hello,
            0.99,
        ),
        Check::p_info(
            "p99 of the server's search for a worst-case view find",
            &server_find,
            0.99,
        ),
        Check::p_info(
            "p99 of hello with a worst-case intent, the whole call (G-1 bounds it by 100 ms)",
            &hello,
            0.99,
        ),
        Check::p_info("p99 of view find with the most common words", &find, 0.99),
        Check::info("writer busy share, cheap searches (mean of two)", uc, ""),
        Check::info(
            "writer busy share, worst-case searches (mean of two)",
            us,
            "",
        ),
        Check::at_most(
            "writer busy share: worst-case searches minus cheap ones",
            us - uc,
            (uc * 0.10).max(0.01),
            "",
        )
        .note("unchanged: within 10 % of it, or a point of busy share"),
    ];
    for (name, b) in &bytes {
        v.checks.push(Check::info(
            &format!("mean bytes of a view find's answer, {name}"),
            *b,
            "B",
        ));
    }
    if o.scale == Scale::Smoke {
        v.notes
            .push(format!("smoke: {docs} documents rather than 10⁸"));
    }
    v.notes.push(format!(
        "four alternating phases of {} s, cheap, worst, cheap, worst: 20 writers checkpointing \
         ten times a second, and {searchers} searchers making two calls a second, alternately a \
         view find {{text}} and a new agent's hello with that text as its intent; the cheap \
         text is the vocabulary's eight rarest words, which find a page of hits over short \
         lists, and the worst is its eight most common",
        phase.as_secs()
    ));
    let counts = sh
        .rec
        .counts(&["control-1", "search-1", "control-2", "search-2"]);
    v.stubs(&counts);
    v.numbers = json!({"recorded": sh.rec.report(), "errors": counts.outcomes});
    Ok(v.decide())
}

// ---------------------------------------------------------------------------------------------
// G-12

/// G-12: many MCP sessions at once, half of them parked in `talk wait`, at the base rate per
/// agent on a clock at real time; then the clock jumps past presence.silence, after which the
/// shell's maps shrink.
#[allow(clippy::too_many_lines, reason = "one gate's run and its checks")]
async fn g12(
    o: &Options,
    dir: &Path,
    m: &Machine,
    started: (u64, Instant),
) -> Result<Verdict, String> {
    let server = start(o, dir, 1).await?;
    let n = o.agents.unwrap_or(o.scale.pick(2_000, 10_000));
    let s = spec(o, &server, 0, BASE, 1);
    let sh = s.shared("seed");
    seed(&sh, 500, 16, secs(120)).await;
    let mut tail = StatsTail::new(&server.launch.stats(), sh.start);
    let at = |t: Instant| t.saturating_duration_since(sh.start);
    sh.rec.phase("open", now());
    let agents = open_agents(&sh, n, 200_000, 512).await;
    let opened = agents.len();
    let live = secs(o.seconds.unwrap_or(o.scale.pick(60, 600)));
    let l0 = now();
    sh.rec.phase("live", l0);
    let l1 = l0 + live;
    let waits = Arc::new(Mutex::new(Vec::new()));
    let mut waiters = Vec::new();
    let mut readers = Vec::new();
    for (i, a) in agents.into_iter().enumerate() {
        if i % 2 == 0 {
            waiters.push(a);
        } else {
            readers.push(a);
        }
    }
    // The base rate per agent: 1.5 calls a minute; a waiter parks for talk.wait_max (50 s) and
    // reads one call in five at that pace.
    let every = Duration::from_secs_f64(60.0 / BASE.calls_per_min);
    let how = WaitHow {
        timeout: secs(50),
        node: None,
        read_every: Some(crate::mix::WAITER_READ_EVERY),
        read_after: every,
    };
    let never = Arc::new(AtomicBool::new(false));
    let mut tasks = spawn_waiters(waiters, l1, &how, &waits, &never);
    tasks.extend(spawn_probes(readers, now(), every, l1));
    let mut peak_rss = 0;
    let mut peak_files = 0;
    while now() < l1 {
        tokio::time::sleep(secs(2)).await;
        tail.poll();
        peak_rss = peak_rss.max(server.rss().await.unwrap_or(0));
        peak_files = peak_files.max(server.open_files().await.unwrap_or(0));
    }
    let bound_live = tail.max("bound", at(l0), at(l1));
    let parked_live = tail.max("parked", at(l0), at(l1));
    sh.rec.phase("silence", now());
    sh.stop.store(true, Ordering::SeqCst);
    join(tasks, secs(120)).await;
    // presence.silence and its resolution pass at once: the clock jumps, and a last call settles
    // the writer to the new now (the silences fire).
    jump_clock(
        &server.launch.stats(),
        minutes_ms(SILENCE_MIN + PRESENCE_RESOLUTION_MIN + 1),
    )?;
    poll_until(&mut tail, now() + secs(3)).await;
    let mut late = open_agents(&sh, 1, 300_000, 1).await;
    if let Some(a) = late.first_mut() {
        a.act(Action::ViewNode, now()).await;
    }
    poll_until(&mut tail, now() + secs(3)).await;
    sh.rec.finish(now());
    let bound_after = tail.last().map_or(0, |x| x.n("bound"));
    let rss_after = server.rss().await.unwrap_or(0);
    let _ = server.stop(secs(60)).await;
    let counts = sh.rec.counts(&["open", "live"]);
    let client_fd: u64 = counts
        .outcomes
        .iter()
        .filter(|(k, _)| k.contains("emfile") || k.contains("os_24") || k.contains("os_23"))
        .map(|(_, n)| n)
        .sum();
    let logged = fd_errors_logged(dir);
    let mut v = Verdict::new("G-12", "connections", o, m, started);
    v.checks = vec![
        Check::at_least("MCP sessions opened", fu(opened), fu(n), ""),
        Check::info("parked in talk wait, most at once", f(parked_live), ""),
        Check::at_most(
            "file-descriptor errors, the driver's and the server's",
            f(client_fd) + fu(logged.len()),
            0.0,
            "",
        )
        .note("EMFILE and ENFILE at the driver; accept and open-file errors in the server's log"),
        Check::at_most("transport errors of any kind", f(counts.transport), 0.0, "")
            .note("a stalled accept shows as a timeout"),
        Check::info("open files of the server, most at once", f(peak_files), ""),
        Check::at_most("server memory, most at once", f(peak_rss) / GIB, 2.0, "GiB"),
        Check::info(
            "MCP-to-forum bindings while live, most at once",
            f(bound_live),
            "",
        ),
        Check::at_most(
            "bindings left after presence.silence (the maps shrink)",
            f(bound_after),
            (f(bound_live) * 0.01).max(2.0),
            "",
        ),
        Check::info("server memory after the silence", f(rss_after) / GIB, "GiB"),
        Check::info("calls a second while live", counts.rate(counts.calls), ""),
    ];
    for l in logged.iter().take(5) {
        v.notes.push(format!("server log: {l}"));
    }
    v.stubs(&counts);
    v.notes.push(
        "one connection per simulated agent, at the base rate per agent on a clock at real time; \
         the clock then jumps past presence.silence and its resolution"
            .into(),
    );
    v.numbers = json!({"recorded": sh.rec.report(), "errors": counts.outcomes});
    Ok(v.decide())
}

/// Writes a verdict's JSON.
///
/// # Errors
///
/// The file could not be written.
pub fn write(v: &Verdict, path: &Path) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The per-tool lines of a cost test's output: `<tool>: small <x>, large <y>`, with `x` and `y`
/// times (`ns`, `µs`, `ms`, or `s`) or visit counts (`<n> visits`).
fn cost_lines(log: &str) -> Vec<Check> {
    let time = |t: &str| -> Option<(f64, &'static str)> {
        let t = t.trim();
        if let Some(n) = t.strip_suffix(" visits") {
            return n.trim().parse::<f64>().ok().map(|x| (x, "visits"));
        }
        for (suffix, scale) in [
            ("ns", 1e-6),
            ("µs", 1e-3),
            ("us", 1e-3),
            ("ms", 1.0),
            ("s", 1e3),
        ] {
            if let Some(n) = t.strip_suffix(suffix)
                && let Ok(x) = n.trim().parse::<f64>()
            {
                return Some((x * scale, "ms"));
            }
        }
        None
    };
    let mut out = Vec::new();
    for l in log.lines() {
        let Some((tool, rest)) = l.split_once(": small ") else {
            continue;
        };
        let Some((small, large)) = rest.split_once(", large ") else {
            continue;
        };
        if let (Some((s, u)), Some((g, _))) = (time(small), time(large)) {
            out.push(Check::info(&format!("{}: small", tool.trim()), s, u));
            out.push(Check::info(&format!("{}: large", tool.trim()), g, u));
            if u == "ms" && s > 0.0 {
                out.push(Check::info(
                    &format!("{}: time ratio large/small (D.12.8: <= 1.5)", tool.trim()),
                    g / s,
                    "",
                ));
            }
        }
    }
    out
}

/// What a script checked itself (G-3, G-11).
pub struct Scripted<'a> {
    /// `G-3` or `G-11`.
    pub gate: &'a str,
    /// What it protects.
    pub title: &'a str,
    /// The scale.
    pub scale: Scale,
    /// `pass`, `fail`, or `not-yet`.
    pub status: &'a str,
    /// What qualifies it.
    pub note: &'a str,
    /// Where the machine's probes write.
    pub dir: &'a Path,
    /// The server binary, for the machine's commit probe.
    pub bin: Option<&'a Path>,
    /// The test's output, whose per-tool lines are recorded (G-3's cost test).
    pub log: Option<&'a str>,
}

/// A verdict for a gate that a script checked itself: its status and a note, the machine (with
/// the server's commit when its binary is given), and the per-tool numbers of the log, if any.
#[must_use]
pub fn scripted(x: &Scripted<'_>) -> Verdict {
    let (gate, title, status, note) = (x.gate, x.title, x.status, x.note);
    let m = machine(x.dir, x.bin);
    let mut v = Verdict {
        gate: gate.into(),
        title: title.into(),
        scale: x.scale,
        pass: None,
        provisional: Vec::new(),
        checks: Vec::new(),
        numbers: Value::Null,
        notes: Vec::new(),
        machine: m,
        started_unix: unix_seconds(),
        seconds: 0.0,
    };
    let check = match status {
        "pass" => Check::at_least(title, 1.0, 1.0, ""),
        "fail" => Check::at_least(title, 0.0, 1.0, ""),
        _ => Check::not_yet(title, note),
    };
    v.checks.push(if note.is_empty() {
        check
    } else {
        check.note(note)
    });
    if let Some(l) = x.log {
        v.checks.extend(cost_lines(l));
    }
    v.decide()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verdict_fails_on_any_failed_check_and_is_not_yet_with_none_checked() {
        let dir = std::env::temp_dir().join(format!("swarm-sim-gates-{}", std::process::id()));
        let x = |status, note, log| Scripted {
            gate: "G-3",
            title: "cost",
            scale: Scale::Smoke,
            status,
            note,
            dir: &dir,
            bin: None,
            log,
        };
        let pass = scripted(&x("pass", "", None));
        assert_eq!(pass.pass, Some(true));
        assert!(pass.render().contains("G-3 smoke: PASS"));
        let fail = scripted(&x("fail", "a note", None));
        assert_eq!(fail.pass, Some(false));
        assert!(fail.render().contains("(a note)"));
        let not_yet = scripted(&x(
            "not-yet",
            "WP-F2",
            Some(
                "apply take: small 2µs, large 2.583µs\nranked k=1: small 2 visits, large 2 visits",
            ),
        ));
        assert_eq!(not_yet.pass, None, "recorded numbers do not decide");
        assert!(not_yet.render().contains("NOT YET"));
        assert_eq!(not_yet.checks.len(), 6);
        assert!((not_yet.checks[2].value.unwrap() - 0.002_583).abs() < 1e-12);
        let mut v = pass.clone();
        v.checks.push(Check::info("recorded only", 3.0, "s"));
        v.checks.push(Check::at_most("bounded", 2.0, 1.0, "ms"));
        let mut v = v.decide();
        assert_eq!(v.pass, Some(false));
        assert!(v.render().contains("FAIL"));
        assert!(v.render().contains("recorded"));
        v.stubs(&Counts {
            calls: 10,
            stubbed: 3,
            ..Counts::default()
        });
        assert!(v.render().contains("FAIL (provisional)"));
        // A verdict round-trips through its JSON (`--reuse`).
        let text = serde_json::to_string(&v).unwrap();
        let back: Verdict = serde_json::from_str(&text).unwrap();
        assert_eq!(back.pass, Some(false));
        assert_eq!(back.checks.len(), 3);
        assert_eq!(back.provisional.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_group_commit_allows_at_most_500_commits_a_second() {
        assert!((commit_ceiling(0.0) - 500.0).abs() < 1e-9);
        // The reference hardware's durable fsync of 1 ms.
        assert!((commit_ceiling(1.0) - 333.333).abs() < 0.001);
    }

    #[test]
    fn numbers_are_formatted_for_the_console() {
        assert_eq!(fmt(3.0), "3");
        assert_eq!(fmt(0.25), "0.250");
        assert_eq!(fmt(1.5e9), "1.500e9");
        assert_eq!(fmt(0.0), "0");
        assert!(estimate(30.0, 8).contains("90.0 h"));
    }

    #[test]
    fn memory_that_keeps_growing_after_the_retention_is_not_flat() {
        // The builder's smoke run: about 8 MiB a day before and after day 7.
        let growing: Vec<(f64, f64)> = (0..=360)
            .map(|h| {
                let d = f64::from(h) / 24.0;
                (d, 180.0 + 8.0 * d)
            })
            .collect();
        let c = flat_after("rss", &growing, 7.0, "MiB");
        assert_eq!(c.pass, Some(false), "{c:?}");
        // Memory that levels off once finished entities leave after 7 days.
        let level: Vec<(f64, f64)> = (0..=360)
            .map(|h| {
                let d = f64::from(h) / 24.0;
                (d, 180.0 + 10.0 * d.min(7.0))
            })
            .collect();
        assert_eq!(flat_after("rss", &level, 7.0, "MiB").pass, Some(true));
        // A run with no tail as long as the retention checks nothing.
        let short: Vec<(f64, f64)> = growing.iter().copied().filter(|p| p.0 < 8.0).collect();
        assert_eq!(flat_after("rss", &short, 7.0, "MiB").pass, None);
        // A series that never grew is flat.
        let flat: Vec<(f64, f64)> = growing.iter().map(|p| (p.0, 50.0)).collect();
        assert_eq!(flat_after("n", &flat, 7.0, "").pass, Some(true));
    }

    #[test]
    fn a_wake_is_timed_from_the_append_that_could_have_woken_it() {
        let t0 = crate::clock::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        // The append is answered after the wait it woke: still counted.
        let appends = [(at(1_000), at(1_200)), (at(5_000), at(5_100))];
        let waits = [
            (at(900), at(1_100)),    // woken by the first append, before its answer
            (at(2_000), at(52_000)), // ran its whole timeout
            (at(3_000), at(3_500)),  // the news was there when it parked: from its sending
            (at(100), at(400)),      // no append before its answer explains it
        ];
        let (lat, timed_out, unexplained) = wake_latencies(&waits, &appends, secs(50));
        assert_eq!((lat.len(), timed_out, unexplained), (2, 1, 1));
        assert!(lat.quantile_us(0.5) >= 100_000 && lat.quantile_us(0.5) < 101_600);
        assert!(lat.quantile_us(1.0) >= 500_000 && lat.quantile_us(1.0) < 508_000);
    }

    #[test]
    fn the_cost_tests_lines_become_numbers() {
        let c = cost_lines(
            "brief: small 10 visits, large 100 visits\nhello: small 1.334µs, large 1.625µs\nnoise",
        );
        assert_eq!(c.len(), 5);
        assert_eq!(c[1].value, Some(100.0));
        assert!((c[2].value.unwrap() - 0.001_334).abs() < 1e-12);
        assert_eq!(c[3].unit, "ms");
        assert!((c[4].value.unwrap() - 1.625 / 1.334).abs() < 1e-9);
    }
}
