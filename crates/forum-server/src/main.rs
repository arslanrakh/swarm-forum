//! The `forum-server` binary (design D.11.2, D.11.3, D.11.6, D.11.8).
//!
//! ```text
//! forum-server serve [--config <file>] [--db <path>] [--search-db <path>] [--listen <addr>] [--check]
//! forum-server check --db <path>
//! forum-server token new --class <class> [--caps <label>,…] [--name <NAME>]
//! forum-server replay --db <path> --into <new path>
//! forum-server rebuild-shell-tables --db <path>
//! forum-server usability-export --db <path>      (feature `usability` only, WP-U)
//! ```
//!
//! `serve` loads the configuration (D.11.9) and the tokens (the environment, then `.env` in the
//! working directory, D.11.3), starts the engine (D.11.8), and serves MCP over streamable HTTP at
//! `/mcp` behind the bearer layer. SIGHUP re-reads the `.env` tokens; SIGTERM and Ctrl-C stop:
//! parked waits answer, every stream of the route ends, the requests in flight drain (for at most
//! [`DRAIN_MARGIN`] past `queue.max_wait` and `talk.wait_max`, or until a second signal), and the
//! writer finishes its current group. Both signals are handled from before the engine starts, so a
//! SIGHUP during startup reloads the tokens once it is up rather than end the process. `check`
//! runs the full invariant check on the database, read-only; `--check` runs it at startup too.
//! `--database` is accepted for `--db`; `--db` without `--search-db` puts the search file beside
//! the database (`<stem>-search.db`).
//!
//! With the feature `bench` (the scale gates only, WP-F4, design D.12.8; never in a release
//! build), `serve` also takes `--clock-scale <k>` and `--clock-start <unix ms>` (a clock that runs
//! k times as fast, and an MCP idle expiry k times as short, never under 30 s), `--bench-stats
//! <file>` (the writer's counters as a JSON line a second; a file `<file>.jump` holding a number of
//! milliseconds moves the clock that far forward at once, and is then removed), and
//! `--bench-shell-bump` (a `shell_version` bump with an empty step, gate G-7); `forum-server
//! checkpoint --db <path>` moves a stopped server's WAL into its file (gate G-8); `forum-server
//! replay-verify --db <path>` folds the history and compares it with the state tables (gate G-10);
//! and `forum-server commit-probe --dir <dir>` times durable commits made as the store makes them
//! (the machine record of every gate).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use forum_core::{ClassName, Label};
use forum_server::auth::{SharedTokens, new_token, token_line};
use forum_server::config::{self, DOTENV};
use forum_server::engine::{Engine, Options, check, lock_database};
use forum_server::mcp::server::{ForumServer, HttpSettings, router_with_stop};
use forum_server::mcp::tools::instructions;
use forum_server::session_bind::SessionBinding;
use forum_server::store::load::Checks;
use tokio::sync::watch;

/// How long past `max(queue.max_wait, talk.wait_max)` the stop waits for the requests in flight
/// before it stops anyway (review E4-spec-2).
const DRAIN_MARGIN: Duration = Duration::from_secs(10);

const USAGE: &str = "usage:
  forum-server serve [--config <file>] [--db <path>] [--search-db <path>] [--listen <addr>] [--check]
  forum-server check --db <path>
  forum-server token new --class <class> [--caps <label>,...] [--name <NAME>]
  forum-server replay --db <path> --into <new path>
  forum-server rebuild-shell-tables --db <path>";

/// The command line: flags with a value, and bare switches.
struct Args {
    values: Vec<(String, String)>,
    switches: Vec<String>,
}

impl Args {
    fn parse(args: &[String], switches: &[&str]) -> Result<Args, String> {
        let mut out = Args {
            values: Vec::new(),
            switches: Vec::new(),
        };
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let Some(name) = a.strip_prefix("--") else {
                return Err(format!("unexpected argument {a:?}"));
            };
            if switches.contains(&name) {
                out.switches.push(name.to_string());
                continue;
            }
            let name = if name == "database" { "db" } else { name };
            let v = it.next().ok_or_else(|| format!("--{name} needs a value"))?;
            out.values.push((name.to_string(), v.clone()));
        }
        Ok(out)
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn need(&self, name: &str) -> Result<&str, String> {
        self.get(name)
            .ok_or_else(|| format!("--{name} is required"))
    }

    fn only(&self, allowed: &[&str]) -> Result<(), String> {
        match self
            .values
            .iter()
            .find(|(k, _)| !allowed.contains(&k.as_str()))
        {
            Some((k, _)) => Err(format!("unknown option --{k}")),
            None => Ok(()),
        }
    }

    fn on(&self, switch: &str) -> bool {
        self.switches.iter().any(|s| s == switch)
    }
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("serve") => serve(&args[1..]),
        Some("check") => check_cmd(&args[1..]),
        Some("token") if args.get(1).map(String::as_str) == Some("new") => token(&args[2..]),
        Some("replay") => replay(&args[1..]),
        Some("rebuild-shell-tables") => rebuild(&args[1..]),
        #[cfg(feature = "usability")]
        Some("usability-export") => usability_export(&args[1..]),
        #[cfg(feature = "bench")]
        Some("replay-verify") => bench::replay_verify(&args[1..]),
        #[cfg(feature = "bench")]
        Some("checkpoint") => bench::checkpoint(&args[1..]),
        #[cfg(feature = "bench")]
        Some("commit-probe") => bench::commit_probe(&args[1..]),
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            Ok(())
        }
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("forum-server: {e}");
            ExitCode::FAILURE
        }
    }
}

fn serve(args: &[String]) -> Result<(), String> {
    #[cfg(not(feature = "bench"))]
    let a = Args::parse(args, &["check"])?;
    #[cfg(not(feature = "bench"))]
    a.only(&["config", "db", "search-db", "listen"])?;
    #[cfg(feature = "bench")]
    let a = Args::parse(args, &["check", "bench-shell-bump"])?;
    #[cfg(feature = "bench")]
    a.only(&[
        "config",
        "db",
        "search-db",
        "listen",
        "clock-scale",
        "clock-start",
        "bench-stats",
    ])?;
    #[cfg(feature = "bench")]
    let mut bench = bench::Settings::from_args(&a)?;
    // The runtime and its signals first: until they are handled, a SIGHUP ends the process, and
    // the startup can take a minute at scale (D.11.8; review E4-robust-11).
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let signals = {
        let _in = rt.enter();
        Signals::install()
    };
    let mut cfg = config::load(a.get("config").map(Path::new)).map_err(|e| e.to_string())?;
    if let Some(db) = a.get("db") {
        cfg.set_database(PathBuf::from(db));
    }
    if let Some(s) = a.get("search-db") {
        cfg.search_database = (!s.is_empty()).then(|| PathBuf::from(s));
    }
    if let Some(l) = a.get("listen") {
        cfg.listen = l.to_string();
    }
    let dotenv = PathBuf::from(DOTENV);
    let tokens =
        config::load_tokens(config::token_environment(), &dotenv).map_err(|e| e.to_string())?;
    if tokens.is_empty() {
        tracing::warn!("no FORUM_TOKEN_* is configured: every request will get HTTP 401");
    }
    for (name, source) in tokens.names() {
        tracing::info!(token = %name, from = ?source, "token");
    }
    let text = instructions(&cfg.instructions, cfg.params.instructions_max)
        .map_err(|e| format!("instructions: {e:?}"))?;
    #[cfg(feature = "usability")]
    let text = cfg.usability_arm.instructions(&text).to_string();
    let mut opts = Options::new(
        cfg.database.clone(),
        cfg.search_database.clone(),
        cfg.params.clone(),
    );
    if a.on("check") {
        opts.checks = Checks::Full;
    }
    #[cfg(feature = "usability")]
    {
        opts.arm = cfg.usability_arm;
        if let Some((lag, file)) = cfg.usability_clock.clone() {
            opts.clock = lab_clock(lag.0, file)?;
        }
    }
    #[cfg(feature = "bench")]
    bench.apply(&mut opts);
    let shared = SharedTokens::new(tokens);
    let binding = Arc::new(Mutex::new(SessionBinding::new()));
    let http = HttpSettings {
        allowed_hosts: cfg.allowed_hosts.clone(),
        mcp_idle: Duration::from_millis(cfg.params.presence_silence.0),
    };
    // On a compressed clock an idle MCP session expires as much sooner, but never within 30 s
    // of real time: a request in flight (a parked wait) takes real time.
    #[cfg(feature = "bench")]
    let http = HttpSettings {
        mcp_idle: bench
            .real(http.mcp_idle)
            .max(std::time::Duration::from_secs(30)),
        ..http
    };
    let result = rt.block_on(async move {
        tracing::info!(database = %cfg.database.display(), "starting the engine");
        let engine = {
            let (shared, binding) = (shared.clone(), Arc::clone(&binding));
            tokio::task::spawn_blocking(move || Engine::start(opts, shared, binding))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?
        };
        #[cfg(feature = "bench")]
        bench.report(Arc::clone(&engine), Arc::clone(&binding))?;
        let served =
            serve_engine(&engine, &cfg, text, &binding, shared, dotenv, http, signals).await;
        Ok::<_, String>((engine, served))
    });
    // A clean stop finishes the current group (D.11.8); a failed start has no engine to stop.
    let result = result.and_then(|(engine, served)| {
        engine.shutdown();
        served
    });
    rt.shutdown_timeout(Duration::from_secs(5));
    if result.is_ok() {
        tracing::info!("stopped");
    }
    result
}

/// Serves the started engine until a stop signal and the drain after it (D.11.8).
#[allow(
    clippy::too_many_arguments,
    reason = "the pieces `serve` assembled, handed over once"
)]
async fn serve_engine(
    engine: &Arc<Engine>,
    cfg: &config::Config,
    text: String,
    binding: &Arc<Mutex<SessionBinding>>,
    shared: SharedTokens,
    dotenv: PathBuf,
    http: HttpSettings,
    signals: Signals,
) -> Result<(), String> {
    let server = ForumServer::new(Arc::clone(engine), text, Arc::clone(binding));
    #[cfg(feature = "usability")]
    let server = server.with_tools(cfg.usability_arm.tools());
    let (app, streams) = router_with_stop(server, shared.clone(), &http);
    let listener = tokio::net::TcpListener::bind(&cfg.listen)
        .await
        .map_err(|err| format!("{}: {err}", cfg.listen))?;
    let at = listener
        .local_addr()
        .map_or_else(|_| cfg.listen.clone(), |a| a.to_string());
    tracing::info!(listen = %at, database = %cfg.database.display(), "serving MCP at /mcp");
    let Signals { hangup, stops } = signals;
    tokio::spawn(reload_on_hangup(Arc::clone(engine), shared, dotenv, hangup));
    let drain_max =
        Duration::from_millis(cfg.params.queue_max_wait.0.max(cfg.params.talk_wait_max.0))
            + DRAIN_MARGIN;
    let e = Arc::clone(engine);
    let mut first = stops.clone();
    let graceful = axum::serve(listener, app).with_graceful_shutdown(async move {
        let _ = first.wait_for(|n| *n >= 1).await;
        tracing::info!("stopping: finishing the current group");
        // Parked waits answer now and no wait parks again; every stream of the route ends, so the
        // drain waits only for the requests in flight (review E4-spec-2, E4-robust-10).
        e.wake_all();
        streams.stop();
    });
    let mut later = stops;
    let backstop = async move {
        let _ = later.wait_for(|n| *n >= 1).await;
        tokio::select! {
            () = tokio::time::sleep(drain_max) => {
                tracing::warn!(after = ?drain_max, "the requests in flight did not drain: stopping anyway");
            }
            r = later.wait_for(|n| *n >= 2) => {
                if r.is_ok() {
                    tracing::warn!("a second stop signal: stopping without waiting for the drain");
                }
            }
        }
    };
    tokio::select! {
        r = graceful => r.map_err(|err| err.to_string()),
        () = backstop => Ok(()),
    }
}

/// The signals `serve` handles, installed before the engine starts: SIGHUP's stream, and the
/// count of stop signals (SIGTERM and Ctrl-C) seen so far.
struct Signals {
    #[cfg(unix)]
    hangup: Option<tokio::signal::unix::Signal>,
    #[cfg(not(unix))]
    hangup: Option<()>,
    stops: watch::Receiver<u32>,
}

impl Signals {
    fn install() -> Signals {
        let (tx, stops) = watch::channel(0_u32);
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let hangup = signal(SignalKind::hangup())
                .map_err(|e| tracing::warn!(error = %e, "SIGHUP cannot be handled: tokens reload only at a restart"))
                .ok();
            let mut term = signal(SignalKind::terminate()).ok();
            let mut int = signal(SignalKind::interrupt()).ok();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        () = next(&mut term) => {}
                        () = next(&mut int) => {}
                    }
                    tx.send_modify(|n| *n = n.saturating_add(1));
                }
            });
            Signals { hangup, stops }
        }
        #[cfg(not(unix))]
        {
            tokio::spawn(async move {
                while tokio::signal::ctrl_c().await.is_ok() {
                    tx.send_modify(|n| *n = n.saturating_add(1));
                }
            });
            Signals {
                hangup: None,
                stops,
            }
        }
    }
}

/// The next delivery of a signal; never, for one that cannot be handled or no longer comes.
#[cfg(unix)]
async fn next(s: &mut Option<tokio::signal::unix::Signal>) {
    if let Some(sig) = s.as_mut()
        && sig.recv().await.is_some()
    {
        return;
    }
    *s = None;
    std::future::pending::<()>().await;
}

/// SIGHUP re-reads the `.env` tokens and swaps them at the writer's next job boundary; a
/// malformed file keeps the old map (D.11.3). A SIGHUP received during startup is delivered here
/// once the engine runs.
#[cfg(unix)]
async fn reload_on_hangup(
    engine: Arc<Engine>,
    tokens: SharedTokens,
    dotenv: PathBuf,
    hangup: Option<tokio::signal::unix::Signal>,
) {
    let Some(mut hup) = hangup else {
        return;
    };
    while hup.recv().await.is_some() {
        match config::reload_tokens(&tokens.current(), &dotenv) {
            Ok(map) => {
                tracing::info!(tokens = map.len(), "reloading the tokens");
                engine.reload_tokens(map);
            }
            Err(e) => tracing::warn!(error = %e, "the tokens were not reloaded"),
        }
    }
}

/// No SIGHUP off Unix: tokens reload only at a restart.
#[cfg(not(unix))]
async fn reload_on_hangup(
    _engine: Arc<Engine>,
    _tokens: SharedTokens,
    _dotenv: PathBuf,
    _hangup: Option<()>,
) {
}

fn check_cmd(args: &[String]) -> Result<(), String> {
    let a = Args::parse(args, &[])?;
    a.only(&["db"])?;
    let db = PathBuf::from(a.need("db")?);
    let r = check(&db).map_err(|e| e.to_string())?;
    println!(
        "check passed: {} events, {} nodes and {} sessions in memory",
        r.events, r.nodes, r.sessions
    );
    Ok(())
}

fn token(args: &[String]) -> Result<(), String> {
    let a = Args::parse(args, &[])?;
    a.only(&["class", "caps", "name"])?;
    let class = ClassName::new(a.need("class")?)
        .ok_or("--class is not a class: [a-z0-9][a-z0-9-]{0,31}")?;
    let caps = a
        .get("caps")
        .map(|c| {
            c.split(',')
                .map(|x| Label::new(x.trim()).ok_or_else(|| format!("{x:?} is not a label")))
                .collect::<Result<std::collections::BTreeSet<_>, _>>()
        })
        .transpose()?;
    let name = a.get("name").map_or_else(
        || class.as_str().to_uppercase().replace('-', "_"),
        str::to_string,
    );
    let mut random = [0_u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut random))
        .map_err(|e| format!("no randomness: {e}"))?;
    let t = new_token(&random);
    println!("token: {t}");
    println!("{}", token_line(&name, &t, &class, caps.as_ref()));
    Ok(())
}

fn replay(args: &[String]) -> Result<(), String> {
    let a = Args::parse(args, &[])?;
    a.only(&["db", "into"])?;
    let src = rusqlite::Connection::open_with_flags(
        a.need("db")?,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| e.to_string())?;
    let into = PathBuf::from(a.need("into")?);
    if into.exists() {
        return Err(format!(
            "{} exists: replay writes a new database",
            into.display()
        ));
    }
    let mut dst = rusqlite::Connection::open(&into).map_err(|e| e.to_string())?;
    let tx = dst.transaction().map_err(|e| e.to_string())?;
    let s =
        forum_server::store::replay::replay_into_checked(&src, &tx).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    println!(
        "replayed {} events into {}",
        s.counters.events,
        into.display()
    );
    Ok(())
}

/// The usability lab's lagged clock (feature `usability`, test only): the forum's clock starts
/// `lag_ms` behind the system's, and catches up by the milliseconds a harness writes to `file`
/// (read within 20 ms, then removed), so that a seed's history spans weeks before the agents
/// arrive at the real time.
#[cfg(feature = "usability")]
fn lab_clock(lag_ms: u64, file: PathBuf) -> Result<Arc<dyn forum_server::clock::Clock>, String> {
    let clock = Arc::new(forum_server::clock::LaggedClock::new(lag_ms));
    tracing::warn!(
        lag_ms,
        "usability: the forum's clock starts behind the system's"
    );
    let c = Arc::clone(&clock);
    std::thread::Builder::new()
        .name("usability-clock".into())
        .spawn(move || {
            loop {
                if let Ok(text) = std::fs::read_to_string(&file) {
                    match text.trim().parse::<u64>() {
                        Ok(ms) => {
                            let left = c.catch_up(ms);
                            tracing::warn!(ms, left, "usability: the clock caught up");
                        }
                        Err(_) => tracing::warn!(text = %text.trim(), "usability: not a number"),
                    }
                    let _ = std::fs::remove_file(&file);
                }
                if c.lag() == 0 {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(clock)
}

/// `forum-server usability-export --db <path>` (feature `usability`, WP-U only, design D.13.2):
/// the whole history as `control export`'s JSON lines (D.11.15), one per line on stdout, read from
/// the database opened read-only. The usability test's slip counter reads a finished run this way,
/// without opening a forum session that would show in the run's own history.
#[cfg(feature = "usability")]
fn usability_export(args: &[String]) -> Result<(), String> {
    use std::io::Write;
    /// The events per page: `control export`'s default (§22).
    const PAGE: u32 = 1_000;
    let a = Args::parse(args, &[])?;
    a.only(&["db"])?;
    let conn = rusqlite::Connection::open_with_flags(
        a.need("db")?,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| e.to_string())?;
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    let mut after = None;
    loop {
        let p = forum_server::export::page(&conn, after, PAGE).map_err(|e| e.to_string())?;
        if p.events == 0 {
            break;
        }
        for line in &p.lines {
            let text = forum_server::export::line_text(line);
            writeln!(out, "{text}").map_err(|e| e.to_string())?;
        }
        after = Some(p.next_after);
    }
    out.flush().map_err(|e| e.to_string())
}

fn rebuild(args: &[String]) -> Result<(), String> {
    let a = Args::parse(args, &[])?;
    a.only(&["db"])?;
    let db = PathBuf::from(a.need("db")?);
    // It writes: not while a server runs on the database (§24, review E4-robust-12).
    let _lock = lock_database(&db).map_err(|e| e.to_string())?;
    let mut conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let n =
        forum_server::store::shell_tables::rebuild_shell_tables(&tx).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    println!("rebuilt the shell's tables from {n} events");
    Ok(())
}

/// The scale gates' switches (feature `bench`, WP-F4, design D.12.8). Never in a release build:
/// the crate compiles with the feature only under `--cfg forum_bench` (see `clock`).
#[cfg(feature = "bench")]
mod bench {
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use forum_core::Time;
    use forum_server::clock::{Clock, Mono, ScaledClock, now_ms};
    use forum_server::engine::{Engine, Options};
    use forum_server::mcp::server::Backend;
    use forum_server::session_bind::SessionBinding;
    use forum_server::store::migrate::{RELEASED, Steps};
    use forum_server::store::schema::SHELL_VERSION;

    use super::Args;

    /// What the bench switches of `serve` asked for.
    pub(crate) struct Settings {
        factor: u64,
        start: Option<Time>,
        stats: Option<PathBuf>,
        shell_bump: bool,
        /// The engine's clock, which the jump file moves forward.
        clock: Option<Arc<ScaledClock>>,
    }

    impl Settings {
        pub(crate) fn from_args(a: &Args) -> Result<Settings, String> {
            let number = |name: &str| {
                a.get(name)
                    .map(|v| {
                        v.parse::<u64>()
                            .map_err(|_| format!("--{name} {v:?} is not a whole number"))
                    })
                    .transpose()
            };
            Ok(Settings {
                factor: number("clock-scale")?.unwrap_or(1).max(1),
                start: number("clock-start")?.map(Time),
                stats: a.get("bench-stats").map(PathBuf::from),
                shell_bump: a.on("bench-shell-bump"),
                clock: None,
            })
        }

        /// The clock (scaled, and moved by the jump file, even at real time's pace), and the
        /// migration steps of a `shell_version` bump.
        pub(crate) fn apply(&mut self, opts: &mut Options) {
            tracing::warn!(
                clock_scale = self.factor,
                "built with the bench feature: for the scale gates only, never a release build"
            );
            let clock = Arc::new(ScaledClock::new(
                self.start.unwrap_or_else(now_ms),
                self.factor,
            ));
            opts.clock = Arc::clone(&clock) as Arc<dyn Clock>;
            self.clock = Some(clock);
            if self.shell_bump {
                opts.steps = Steps {
                    shell: &[(SHELL_VERSION, empty_shell_step)],
                    shell_version: SHELL_VERSION.saturating_add(1),
                    ..RELEASED
                };
            }
        }

        /// A duration of the forum's time as real time on the scaled clock.
        pub(crate) fn real(&self, d: Duration) -> Duration {
            d.checked_div(u32::try_from(self.factor).unwrap_or(u32::MAX))
                .unwrap_or(d)
        }

        /// Appends the engine's counters to the stats file once a second, as JSON lines, and
        /// applies a jump of the clock when a gate writes one (`<stats file>.jump`: milliseconds
        /// to move forward, read within 100 ms and then removed).
        pub(crate) fn report(
            &self,
            engine: Arc<Engine>,
            binding: Arc<Mutex<SessionBinding>>,
        ) -> Result<(), String> {
            let Some(path) = self.stats.clone() else {
                return Ok(());
            };
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            let jump_file = {
                let mut j = path.as_os_str().to_owned();
                j.push(".jump");
                PathBuf::from(j)
            };
            let clock = self.clock.clone();
            let origin = Mono::now();
            // A weak reference: the reporter must not keep the engine, and with it the database's
            // connections, alive past the server's shutdown.
            let weak = Arc::downgrade(&engine);
            drop(engine);
            std::thread::Builder::new()
                .name("bench-stats".into())
                .spawn(move || {
                    let n = |x: &AtomicU64| x.load(Ordering::Relaxed);
                    let mut last: Option<Mono> = None;
                    loop {
                        if let Some(c) = &clock {
                            jump(c, &jump_file);
                        }
                        if last.is_some_and(|l| Mono::now().since(l) < Duration::from_secs(1)) {
                            std::thread::sleep(Duration::from_millis(100));
                            continue;
                        }
                        last = Some(Mono::now());
                        let Some(engine) = weak.upgrade() else {
                            return;
                        };
                        let s = engine.stats();
                        let bound = binding.lock().map_or(0, |b| b.len());
                        let t_ms =
                            u64::try_from(Mono::now().since(origin).as_millis()).unwrap_or(0);
                        let (searches, searches_dropped) = forum_server::bench::take_searches();
                        let line = serde_json::json!({
                            "t_ms": t_ms,
                            "sim_ms": engine.now().0,
                            "jumped_ms": clock.as_ref().map_or(0, |c| c.jumped()),
                            "groups": n(&s.groups),
                            "commits": n(&s.commits),
                            "jobs": n(&s.jobs),
                            "skipped": n(&s.skipped),
                            "busy": n(&s.busy),
                            "idle_settles": n(&s.idle_settles),
                            "health_computed": n(&s.health_computed),
                            "wakes": n(&s.wakes),
                            "busy_ns": n(&s.busy_ns),
                            "wake_ns": n(&s.wake_ns),
                            "wake_examined": n(&s.wake_examined),
                            "wake_concerned": n(&s.wake_concerned),
                            "wake_rounds": n(&s.wake_rounds),
                            "events": n(&s.events),
                            "wal_bytes": forum_server::bench::wal_bytes(),
                            "parked": engine.parked(),
                            "queued": engine.queued(),
                            "bound": bound,
                            "alive": engine.alive(),
                            "sizes": forum_server::bench::sizes(),
                            "search_us": searches,
                            "searches_dropped": searches_dropped,
                            "startup": forum_server::bench::startup(),
                        });
                        let alive = engine.alive();
                        drop(engine);
                        if writeln!(file, "{line}")
                            .and_then(|()| file.flush())
                            .is_err()
                            || !alive
                        {
                            return;
                        }
                    }
                })
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
    }

    /// Applies the jump a gate wrote to `file`, if any, and removes the file.
    fn jump(clock: &ScaledClock, file: &Path) {
        let Ok(text) = std::fs::read_to_string(file) else {
            return;
        };
        let _ = std::fs::remove_file(file);
        match text.trim().parse::<u64>() {
            Ok(ms) => {
                clock.jump(ms);
                tracing::warn!(ms, "bench: the clock jumped forward");
            }
            Err(_) => {
                tracing::warn!(text = %text.trim(), "bench: a jump file that is not a number")
            }
        }
    }

    /// The step of the bench's `shell_version` bump: it rewrites nothing, so that gate G-7 times
    /// the migration path itself (the version check, the transaction, and its `migrate` record).
    fn empty_shell_step(_: &rusqlite::Connection) -> forum_server::store::Result<()> {
        Ok(())
    }

    /// `forum-server checkpoint --db <path>`, on a stopped server's database: moves the whole WAL
    /// into the file and truncates it, then prints the file's bytes and free pages as JSON (gate
    /// G-8 measures the file, not the WAL that the server reuses up to WAL_RESTART).
    pub(crate) fn checkpoint(args: &[String]) -> Result<(), String> {
        let a = Args::parse(args, &[])?;
        a.only(&["db"])?;
        let db = a.need("db")?;
        let conn = rusqlite::Connection::open(db).map_err(|e| e.to_string())?;
        let busy: i64 = conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let page = |p: &str| -> Result<i64, String> {
            conn.query_row(&format!("PRAGMA {p}"), [], |r| r.get(0))
                .map_err(|e| e.to_string())
        };
        let (size, count, free) = (
            page("page_size")?,
            page("page_count")?,
            page("freelist_count")?,
        );
        drop(conn);
        let bytes = std::fs::metadata(db).map_err(|e| e.to_string())?.len();
        println!(
            "{}",
            serde_json::json!({"bytes": bytes, "page_size": size, "pages": count,
                "free_pages": free, "checkpoint_busy": busy})
        );
        Ok(())
    }

    /// `forum-server commit-probe --dir <dir> [--n <commits>]`: times durable commits of a 4 KiB
    /// row on a database of its own in `dir`, with the store's PRAGMAs, and prints their median and
    /// 99th percentile in milliseconds as JSON, with SQLite's `fullfsync` setting (the machine
    /// record of every gate: the sync the server's commits really pay).
    pub(crate) fn commit_probe(args: &[String]) -> Result<(), String> {
        let a = Args::parse(args, &[])?;
        a.only(&["dir", "n"])?;
        let n = a
            .get("n")
            .map(|v| {
                v.parse::<u32>()
                    .map_err(|_| format!("--n {v:?} is not a number"))
            })
            .transpose()?
            .unwrap_or(200)
            .max(1);
        let (mut us, full) = forum_server::bench::commit_probe(Path::new(a.need("dir")?), n)?;
        us.sort_unstable();
        let at = |q: f64| {
            #[allow(
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a rank within the sample, and microseconds as milliseconds"
            )]
            let ms = {
                let i = ((q * us.len() as f64).ceil() as usize).clamp(1, us.len()) - 1;
                us.get(i).copied().unwrap_or(0) as f64 / 1_000.0
            };
            ms
        };
        println!(
            "{}",
            serde_json::json!({"commits": us.len(), "p50_ms": at(0.5), "p99_ms": at(0.99),
                "synchronous": "FULL", "journal_mode": "WAL", "fullfsync": full})
        );
        Ok(())
    }

    /// `forum-server replay-verify --db <path>`: folds the history through `evolve` from Genesis,
    /// single-threaded, and compares the result with the state tables (D.12.6, gate G-10); prints
    /// the events and the milliseconds it took as JSON.
    pub(crate) fn replay_verify(args: &[String]) -> Result<(), String> {
        let a = Args::parse(args, &[])?;
        a.only(&["db"])?;
        let conn = rusqlite::Connection::open_with_flags(
            a.need("db")?,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| e.to_string())?;
        let start = Mono::now();
        let s = forum_server::store::replay::verify(&conn).map_err(|e| e.to_string())?;
        let ms = u64::try_from(Mono::now().since(start).as_millis()).unwrap_or(u64::MAX);
        println!(
            "{}",
            serde_json::json!({"events": s.counters.events, "ms": ms, "equal": true})
        );
        Ok(())
    }
}
