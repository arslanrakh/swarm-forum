//! The engine (design D.11.2, D.8.1, D.11.8; WP-E4): the single writer of §24 and everything
//! around it.
//!
//! - **The writer thread** owns the read-write connection and the in-memory `State`. Tool handlers
//!   send it jobs through one queue per class ([`queue`]), served by deficit round-robin. It runs
//!   them in groups: one SQLite transaction for at most [`COMMIT_GROUP_MAX`] jobs, those already
//!   queued and those that arrive within [`COMMIT_LINGER`] of the group's first while the group has
//!   something to commit; then one COMMIT; then every reply of the group (§24: respond only after
//!   commit). Each call runs
//!   D.8.1: the pre-settle, the `write.per_hour` valve, restores, `apply`, the post-settle, the next
//!   calls, rendering, and the `st_ops` row of an `op_id`. A job that appends nothing writes
//!   nothing (CS-14).
//!   A pre-settle commits the group at each batch boundary; a post-settle keeps its batches in the
//!   open transaction until the job's `st_ops` row is written, and commits after it (O1).
//! - **Before queueing**, the handler searches (D.11.12) and answers `limit(Busy)` when the class's
//!   oldest queued call has waited longer than `queue.max_wait`, measured on a monotonic clock, or
//!   when its lane or the queues are full ([`QUEUE_MAX`]) (CS-29); the writer skips a job whose
//!   caller has gone, applying nothing.
//! - **When idle**, the writer waits for a job, the first queued deadline, the health cache's
//!   refresh, or a deferred wake-up, and at a deadline settles to `now` as a group of its own
//!   (CS-30, §17): by T2 the events are those the next call's pre-settle would fire.
//! - **When a session's silence or retirement fires**, in any settle, the shell's per-session maps
//!   drop it at once, so the call that revives it records after the drop (CS-17, D.11.4).
//! - **After each commit** it wakes the `talk wait` handlers the group's events concern (D.11.11),
//!   tells the search feeder how far the history goes, and refreshes the health cache (D.8.22).
//! - **Crash-only**: any storage error after `evolve` ran, a violated database constraint, a
//!   `Stuck` settle, a panic, or (in debug builds) a failed check (`check_fire`, `check_step`,
//!   `check_invariants`, `check_guide`, D.9.1) ends the writer before any reply of its group is
//!   sent; production aborts the process, and a restart loads the state tables (D.11.8). A database
//!   locked by another connection is waited out, not fatal ([`WRITER_BUSY`]).
//!
//! Startup ([`Engine::start`]) is D.11.8: the database's lock (§24's single writer), open (creating
//! Genesis), settle to `last_now`, the startup checks, the configured parameters through
//! `Cmd::Admin(SetParams)`, settle to `now`; then the threads: the writer, the checkpointer, and the
//! search feeder (whose damaged file is moved aside and rebuilt, never fatal).

pub mod queue;
mod writer;

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use forum_core::health::{Health, HealthReason, Level};
use forum_core::{
    ClassName, Cmd, Dur, Eid, NodeId, NodeRef, Params, SessionCtx, SessionId, TalkCmd, Time,
};
use rusqlite::{Connection, OpenFlags};
use tokio::sync::oneshot;

use crate::auth::{SharedTokens, TokenMap};
use crate::clock::{Clock, Mono};
use crate::decode::{EntryLookup, ShellAnswer};
use crate::mcp::server::{Backend, Call, Refusal, Reply};
use crate::render::SearchHits;
use crate::search::{Query, Searcher};
use crate::search_feed::Feeder;
use crate::session_bind::SessionBinding;
use crate::store::load::Checks;
use crate::store::{StoreError, shell_tables};
use crate::wait::{Parked, Waiters};
use queue::{Key, Lane, Queues};

/// The most jobs one transaction commits (D.11.2, CS-13).
pub const COMMIT_GROUP_MAX: usize = 64;
/// A group takes the jobs that arrive within this time of its first (D.11.2, CS-13).
pub const COMMIT_LINGER: Duration = Duration::from_millis(2);
/// The health cache is recomputed at most this often, and at most this late after a commit
/// (D.8.22, CS-21).
pub const HEALTH_REFRESH: Dur = Dur(10_000);
/// An idle writer looks at the clock at least this often.
pub const IDLE_MAX: Duration = Duration::from_secs(1);
/// How long the writer's `BEGIN IMMEDIATE` waits for another connection's write lock before it
/// tries again: longer than any other connection holds it (the checkpointer's
/// [`crate::checkpointer::CHECKPOINT_BUSY`]), so a checkpoint delays the writer and never stops it
/// (review E4-robust-1).
pub const WRITER_BUSY: Duration = Duration::from_secs(5);
/// The most jobs one class's lane holds (D.11.2's bounded queues, review E4-spec-13): about what the
/// writer runs within `queue.max_wait` at its lowest measured rate, 2,200 calls a second for 10 s
/// (D.11.2), so a full lane would be busy before its last job ran anyway. A 07:00 herd of one class
/// still fits.
pub const QUEUE_MAX: usize = 20_000;
/// The most jobs all lanes hold together: four full lanes.
pub const QUEUE_TOTAL_MAX: usize = 4 * QUEUE_MAX;

/// What a fatal error does (crash-only, D.11.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OnFatal {
    /// Logs and aborts the process: production.
    #[default]
    Abort,
    /// Ends the writer thread, dropping its group's replies and the queued jobs, as an abort would
    /// look to the clients; the connection closes, rolling back the open transaction. Tests.
    StopWriter,
}

/// A gate the writer waits at before it takes each job (tests: to queue jobs while it waits).
#[derive(Debug, Default)]
pub struct Gate {
    closed: Mutex<bool>,
    cv: Condvar,
}

impl Gate {
    /// A gate, closed or open.
    #[must_use]
    pub fn new(closed: bool) -> Arc<Gate> {
        Arc::new(Gate {
            closed: Mutex::new(closed),
            cv: Condvar::new(),
        })
    }

    /// Closes or opens it.
    pub fn set(&self, closed: bool) {
        *lock(&self.closed) = closed;
        self.cv.notify_all();
    }

    fn is_closed(&self) -> bool {
        *lock(&self.closed)
    }

    fn pass(&self) {
        let mut g = lock(&self.closed);
        while *g {
            g = match self.cv.wait(g) {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
        }
    }
}

/// A test hook that reads the writer's in-memory state.
pub type StateHook = Arc<dyn Fn(&forum_core::State) + Send + Sync>;

/// Test hooks (D.12.6: "crash behaviour through in-crate tests with the test hooks"). All off by
/// default; production never sets them.
#[derive(Clone, Default)]
pub struct Hooks {
    /// Called with the number of jobs in a group just before its COMMIT.
    pub before_commit: Option<Arc<dyn Fn(usize) + Send + Sync>>,
    /// Called with the writer's in-memory state just after each COMMIT that wrote something: the
    /// persist-and-reload check of the end-to-end tests compares it with the state tables (D.12.6).
    pub after_commit: Option<StateHook>,
    /// Ends the writer as a crash would, once this many jobs of a group have run, before COMMIT.
    pub crash_after_jobs: Option<usize>,
    /// When set, the next group's replies are dropped after its COMMIT, as a crash between the
    /// commit and the replies would drop them.
    pub drop_replies_after_commit: Arc<AtomicBool>,
    /// The writer waits at this gate before it takes each job.
    pub gate: Option<Arc<Gate>>,
    /// The writer connection's busy timeout in place of [`WRITER_BUSY`].
    pub writer_busy: Option<Duration>,
    /// `WriteSurge`'s floor in place of [`crate::limits::WRITE_SURGE_FLOOR`].
    pub write_surge_floor: Option<u32>,
    /// The queues' bounds, per lane and in all, in place of [`QUEUE_MAX`] and
    /// [`QUEUE_TOTAL_MAX`].
    pub queue_max: Option<(usize, usize)>,
}

impl fmt::Debug for Hooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hooks")
            .field("before_commit", &self.before_commit.is_some())
            .field("after_commit", &self.after_commit.is_some())
            .field("crash_after_jobs", &self.crash_after_jobs)
            .field("gate", &self.gate.is_some())
            .field("writer_busy", &self.writer_busy)
            .field("write_surge_floor", &self.write_surge_floor)
            .field("queue_max", &self.queue_max)
            .finish_non_exhaustive()
    }
}

/// How the engine starts.
#[derive(Clone)]
pub struct Options {
    /// The forum's database file.
    pub database: PathBuf,
    /// The search file (D.11.12); None: no search.
    pub search_database: Option<PathBuf>,
    /// The configured parameters (D.11.9): Genesis's for a new database, and applied at startup
    /// when they differ from the stored ones (D.11.8 step 6).
    pub params: Params,
    /// The startup checks (D.11.8 step 4): the linear subset always, the full check in debug
    /// builds or with `--check`.
    pub checks: Checks,
    /// Runs D.9.1's checks of debug builds: `check_fire` after every firing, `check_step` after
    /// every command, `check_invariants` after every completed settle, and `check_guide` on every
    /// response's next calls.
    pub check_every_call: bool,
    /// The clock.
    pub clock: Arc<dyn Clock>,
    /// What a fatal error does.
    pub on_fatal: OnFatal,
    /// The checkpointer's interval (D.11.2).
    pub checkpoint_every: Duration,
    /// The WAL size past which the checkpointer restarts the WAL (D.11.2).
    pub wal_restart: u64,
    /// The search's time budget (D.11.12).
    pub search_budget: Duration,
    /// Test hooks.
    pub hooks: Hooks,
    /// The usability test's arm (feature `usability`, WP-U only).
    #[cfg(feature = "usability")]
    pub arm: crate::render::usability::Arm,
    /// The migration steps startup runs (feature `bench`: gate G-7's `shell_version` bump);
    /// otherwise always the released ones.
    #[cfg(feature = "bench")]
    pub steps: crate::store::migrate::Steps,
}

impl Options {
    /// Production options for the database at `database`: the system clock, the checks of the
    /// build, aborting on a fatal error.
    #[must_use]
    pub fn new(database: PathBuf, search_database: Option<PathBuf>, params: Params) -> Options {
        Options {
            database,
            search_database,
            params,
            checks: if cfg!(debug_assertions) {
                Checks::Full
            } else {
                Checks::Startup
            },
            check_every_call: cfg!(debug_assertions),
            clock: Arc::new(crate::clock::SystemClock),
            on_fatal: OnFatal::Abort,
            checkpoint_every: crate::checkpointer::CHECKPOINT_EVERY,
            wal_restart: crate::checkpointer::WAL_RESTART,
            search_budget: crate::search::SEARCH_BUDGET,
            hooks: Hooks::default(),
            #[cfg(feature = "usability")]
            arm: crate::render::usability::Arm::default(),
            #[cfg(feature = "bench")]
            steps: crate::store::migrate::RELEASED,
        }
    }
}

/// Why the engine did not start: each stops the server (D.11.8).
#[derive(Debug)]
pub enum StartError {
    /// Storage: opening, migrating, loading, a startup check, or a write.
    Store(StoreError),
    /// The configured parameters were refused (D.11.8 step 6): the core's error.
    Params(forum_core::Error),
    /// A thread could not be started.
    Thread(std::io::Error),
    /// Another process holds the database's lock (§24: one writer; review E4-robust-12).
    Locked(PathBuf),
    /// The database's lock file could not be opened or locked.
    Lock(PathBuf, std::io::Error),
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StartError::Store(e) => write!(f, "{e}"),
            StartError::Params(e) => write!(
                f,
                "the configured parameters were refused: {:?} ({:?})",
                e.code, e.hint
            ),
            StartError::Thread(e) => write!(f, "a thread did not start: {e}"),
            StartError::Locked(p) => write!(
                f,
                "another forum-server holds {}: one process writes a database (§24)",
                p.display()
            ),
            StartError::Lock(p, e) => write!(f, "{}: {e}", p.display()),
        }
    }
}

impl std::error::Error for StartError {}

impl From<StoreError> for StartError {
    fn from(e: StoreError) -> StartError {
        StartError::Store(e)
    }
}

/// Counters of what the engine did.
#[derive(Debug, Default)]
pub struct Stats {
    /// Groups run.
    pub groups: AtomicU64,
    /// COMMITs.
    pub commits: AtomicU64,
    /// Jobs run.
    pub jobs: AtomicU64,
    /// Jobs skipped because their caller had gone.
    pub skipped: AtomicU64,
    /// Calls answered `limit(Busy)`.
    pub busy: AtomicU64,
    /// Idle settles (CS-30).
    pub idle_settles: AtomicU64,
    /// Health computations.
    pub health_computed: AtomicU64,
    /// Wake-ups sent to parked `talk wait` handlers.
    pub wakes: AtomicU64,
    /// Firings checked with `check_fire` (debug builds, D.9.1).
    pub fires_checked: AtomicU64,
    /// Completed settles checked with `check_invariants` (debug builds, D.9.1).
    pub settles_checked: AtomicU64,
    /// Responses whose next calls were checked with `check_guide` (debug builds, D.9.1).
    pub guides_checked: AtomicU64,
    /// `BEGIN IMMEDIATE`s that found the database locked and tried again.
    pub busy_begins: AtomicU64,
    /// Nanoseconds the writer spent on groups and idle work, its waits for more jobs within a
    /// group left out (feature `bench`: gate G-2's utilisation).
    #[cfg(feature = "bench")]
    pub busy_ns: AtomicU64,
    /// Nanoseconds the writer spent waking `talk wait` handlers after commits (feature `bench`:
    /// gate G-4).
    #[cfg(feature = "bench")]
    pub wake_ns: AtomicU64,
    /// Parked sessions the wake-ups after commits examined: every session in the parked set that
    /// a commit's wake-up collects (feature `bench`: gate G-4's readiness checks per commit).
    #[cfg(feature = "bench")]
    pub wake_examined: AtomicU64,
    /// Parked waits the commits concerned: the sessions a commit addresses and the waits for the
    /// nodes it appended to (feature `bench`: gate G-4's bound on the checks).
    #[cfg(feature = "bench")]
    pub wake_concerned: AtomicU64,
    /// Commits after which parked waiters were looked at (feature `bench`: gate G-4).
    #[cfg(feature = "bench")]
    pub wake_rounds: AtomicU64,
    /// The history's length after the writer's last group (feature `bench`: gates G-4 and G-10).
    #[cfg(feature = "bench")]
    pub events: AtomicU64,
}

/// What a `talk wait` handler learns when it asks whether to park (D.11.11).
#[derive(Debug)]
pub(crate) enum Readiness {
    /// Run the `wait` command now: the session is ready, or the call will be refused.
    Ready,
    /// Parked: wait for a wake-up or the timeout.
    Park(Parked),
}

/// One job of the writer.
pub(crate) enum Work {
    /// A tool call, with the search hits its handler found.
    Call {
        call: Box<Call>,
        hits: SearchHits,
        reply: oneshot::Sender<Reply>,
    },
    /// The shell's refusal of a call that names an existing session: rendered with its status
    /// line (D.11.7). Boxed, since a refusal that names every problem of a call (1.6) is large.
    Refuse {
        class: ClassName,
        session: SessionId,
        answer: Box<ShellAnswer>,
        reply: oneshot::Sender<Reply>,
    },
    /// The readiness check of a `talk wait` as it parks (D.11.11): read-only.
    Ready {
        class: ClassName,
        session: SessionId,
        for_: Option<NodeRef>,
        reply: oneshot::Sender<Readiness>,
    },
}

impl Work {
    /// Whether its caller has gone (D.11.2: such a job is skipped).
    fn closed(&self) -> bool {
        match self {
            Work::Call { reply, .. } | Work::Refuse { reply, .. } => reply.is_closed(),
            Work::Ready { reply, .. } => reply.is_closed(),
        }
    }
}

/// Requests to the writer that are not jobs: applied at a job boundary.
#[derive(Debug, Default)]
struct Control {
    /// A token map to swap in (SIGHUP, D.11.3).
    reload: Option<TokenMap>,
    /// Finish the current group and stop.
    shutdown: bool,
    /// Look at the clock now (a test that moved a manual clock).
    kick: bool,
}

struct Inbox {
    queues: Queues<Work>,
    control: Control,
}

/// The parameters the handlers read; fixed after startup (D.11.9).
#[derive(Clone, Copy, Debug)]
struct Knobs {
    text_max: u32,
    coverage_window: Dur,
    queue_max_wait: Dur,
    talk_wait_max: Dur,
}

/// What the handlers and the writer share.
pub(crate) struct Shared {
    clock: Arc<dyn Clock>,
    knobs: Knobs,
    inbox: Mutex<Inbox>,
    cv: Condvar,
    entries: EntryReader,
    searcher: Option<Searcher>,
    waiters: Mutex<Waiters>,
    binding: Arc<Mutex<SessionBinding>>,
    tokens: SharedTokens,
    stats: Stats,
    /// The writer is running.
    alive: AtomicBool,
    /// The server is stopping: a `talk wait` no longer parks (D.11.8, review E4-robust-10).
    draining: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

impl Shared {
    fn inbox(&self) -> MutexGuard<'_, Inbox> {
        lock(&self.inbox)
    }

    pub(crate) fn waiters(&self) -> MutexGuard<'_, Waiters> {
        lock(&self.waiters)
    }

    /// Queues `work` in `key`'s queue, or answers busy: the class's oldest queued call has waited
    /// longer than `queue.max_wait` (CS-29), or the lane or the queues are full (D.11.2, review
    /// E4-spec-13). A refusal is never busy by age, only when it does not fit; its caller then
    /// answers without the status line.
    fn submit(&self, key: Key, work: Work) -> Result<(), Work> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err(work);
        }
        let now = self.clock.mono();
        let mut inbox = self.inbox();
        if key.1 == Lane::Calls {
            let oldest = inbox.queues.oldest(&key);
            if crate::limits::busy(oldest, now, self.knobs.queue_max_wait) {
                self.stats.busy.fetch_add(1, Ordering::Relaxed);
                return Err(work);
            }
        }
        if let Err(work) = inbox.queues.push(key, now, work) {
            self.stats.busy.fetch_add(1, Ordering::Relaxed);
            return Err(work);
        }
        drop(inbox);
        self.cv.notify_one();
        Ok(())
    }
}

/// The shell's reads of entries for the decoder (D.11.5): a read-only connection to the forum's
/// database, opened at first use.
pub struct EntryReader {
    path: PathBuf,
    conn: Mutex<Option<Connection>>,
}

impl EntryReader {
    fn new(path: PathBuf) -> EntryReader {
        EntryReader {
            path,
            conn: Mutex::new(None),
        }
    }

    fn read<T>(&self, f: impl FnOnce(&Connection) -> Option<T>) -> Option<T> {
        let mut g = lock(&self.conn);
        if g.is_none() {
            *g = Connection::open_with_flags(
                &self.path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .ok();
        }
        g.as_ref().and_then(f)
    }
}

impl EntryLookup for EntryReader {
    fn entry_node(&self, eid: Eid) -> Option<NodeId> {
        self.read(|c| shell_tables::entry_node(c, eid).ok().flatten())
            .map(|(n, _)| n)
    }

    fn question(&self, eid: Eid) -> Option<(NodeId, Option<SessionId>)> {
        self.read(|c| shell_tables::entry_node(c, eid).ok().flatten())
    }
}

/// The health cache (D.8.22, CS-21): the whole `Health` of `query::health` with the shell's
/// `WriteSurge` reasons, when it was computed, and whether anything committed since.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthCache {
    /// The cached value.
    pub health: Health,
    /// When it was computed.
    pub at: Time,
    /// Something committed since.
    pub committed: bool,
}

impl HealthCache {
    /// A cache computed at `at`.
    #[must_use]
    pub fn new(health: Health, at: Time) -> HealthCache {
        HealthCache {
            health,
            at,
            committed: false,
        }
    }

    /// When the cache is due for a refresh: [`HEALTH_REFRESH`] after it was computed, if
    /// something committed since.
    #[must_use]
    pub fn due(&self) -> Option<Time> {
        self.committed.then(|| self.at.plus(HEALTH_REFRESH))
    }

    /// Recomputes with `compute` when due at `now`; whether it did.
    pub fn refresh(&mut self, now: Time, compute: impl FnOnce() -> Health) -> bool {
        if self.due().is_some_and(|t| t <= now) {
            self.health = compute();
            self.at = now;
            self.committed = false;
            true
        } else {
            false
        }
    }
}

/// `query::health` with the shell's `WriteSurge` reasons: every reason sorted red first, then in
/// the declaration order of `HealthReason`, and the level the worst of them (§17, D.8.22).
#[must_use]
pub fn with_surges(mut h: Health, surges: Vec<HealthReason>) -> Health {
    fn order(r: &HealthReason) -> u8 {
        match r {
            HealthReason::Paused { .. } => 0,
            HealthReason::NoMover { .. } => 1,
            HealthReason::Unreachable { .. } => 2,
            HealthReason::FlagsPilingUp { .. } => 3,
            HealthReason::Overloaded { .. } => 4,
            HealthReason::WriteSurge { .. } => 5,
            HealthReason::Underused { .. } => 6,
        }
    }
    h.reasons.extend(surges);
    h.reasons
        .sort_by_key(|r| (std::cmp::Reverse(r.level() == Level::Red), order(r)));
    h.level = h
        .reasons
        .iter()
        .map(HealthReason::level)
        .max()
        .unwrap_or(Level::Green);
    h
}

/// The running engine: the [`Backend`] of the MCP handler.
pub struct Engine {
    sh: Arc<Shared>,
    writer: Mutex<Option<JoinHandle<()>>>,
    checkpointer: Option<crate::checkpointer::Checkpointer>,
    feeder: Option<Arc<Feeder>>,
    gate: Option<Arc<Gate>>,
    /// The database's lock, held until shutdown.
    lock: Mutex<Option<DbLock>>,
}

/// The lock that keeps the writer single (§24, review E4-robust-12): an exclusive lock on
/// `<db>.lock`, held by `serve` for as long as its engine runs and by `rebuild-shell-tables` while
/// it writes. The operating system releases it when the process ends, however it ends.
#[derive(Debug)]
pub struct DbLock {
    _file: std::fs::File,
}

/// The lock file of the database at `db`: `<db>.lock`.
#[must_use]
pub fn lock_path(db: &std::path::Path) -> PathBuf {
    let mut p = db.as_os_str().to_owned();
    p.push(".lock");
    PathBuf::from(p)
}

/// Takes the exclusive lock of the database at `db`, without waiting.
///
/// # Errors
///
/// `Locked` when another process holds it, and `Lock` when the file cannot be opened or locked.
pub fn lock_database(db: &std::path::Path) -> Result<DbLock, StartError> {
    let path = lock_path(db);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| StartError::Lock(path.clone(), e))?;
    match file.try_lock() {
        Ok(()) => Ok(DbLock { _file: file }),
        Err(std::fs::TryLockError::WouldBlock) => Err(StartError::Locked(path)),
        Err(std::fs::TryLockError::Error(e)) => Err(StartError::Lock(path, e)),
    }
}

/// Whether `a` and `b` name one file (the search file must not be the forum's database).
fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// Starts the search feeder on the search file at `index` (D.11.8 step 5). Search is advisory
/// (§18): a file the feeder cannot open is moved aside to `<file>.damaged` and rebuilt from the
/// history, and if that fails too the server runs without search; neither stops it (review
/// E4-robust-9).
fn start_feeder(db: &std::path::Path, index: &std::path::Path) -> Option<Feeder> {
    if same_file(db, index) {
        tracing::warn!(path = %index.display(), "the search file is the forum's database: search is off");
        return None;
    }
    let first = match Feeder::start(db, index) {
        Ok(f) => return Some(f),
        Err(e) => e,
    };
    tracing::warn!(path = %index.display(), error = %first, "the search file cannot be used: moving it aside and rebuilding it");
    for suffix in ["", "-wal", "-shm"] {
        let mut from = index.as_os_str().to_owned();
        from.push(suffix);
        let mut to = index.as_os_str().to_owned();
        to.push(".damaged");
        to.push(suffix);
        if let Err(e) = std::fs::rename(&from, &to)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %PathBuf::from(from).display(), error = %e, "could not move it aside");
        }
    }
    match Feeder::start(db, index) {
        Ok(f) => Some(f),
        Err(e) => {
            tracing::warn!(path = %index.display(), error = %e, "the search file cannot be rebuilt: search is off");
            None
        }
    }
}

impl Engine {
    /// Starts the engine (D.11.8): opens the database, loads and checks the state, applies the
    /// configured parameters, settles to now, and starts the writer, the checkpointer, and the
    /// search feeder. `tokens` and `binding` are shared with the HTTP layer and the handler.
    ///
    /// # Errors
    ///
    /// Whatever stops the server at startup.
    pub fn start(
        opts: Options,
        tokens: SharedTokens,
        binding: Arc<Mutex<SessionBinding>>,
    ) -> Result<Arc<Engine>, StartError> {
        // §24's single writer: a second process on the same database stops here (review
        // E4-robust-12).
        let lock = lock_database(&opts.database)?;
        // The writer opens the database first: it creates it, with Genesis, when it is new.
        let (writer, params) = writer::Writer::open(&opts)?;
        let feeder = opts
            .search_database
            .as_deref()
            .and_then(|p| start_feeder(&opts.database, p))
            .map(Arc::new);
        let searcher = opts
            .search_database
            .as_deref()
            .filter(|_| feeder.is_some())
            .map(|p| Searcher::with_budget(p, opts.search_budget));
        let (lane_max, total_max) = opts.hooks.queue_max.unwrap_or((QUEUE_MAX, QUEUE_TOTAL_MAX));
        let sh = Arc::new(Shared {
            clock: Arc::clone(&opts.clock),
            knobs: Knobs {
                text_max: params.text_max,
                coverage_window: params.coverage_window,
                queue_max_wait: params.queue_max_wait,
                talk_wait_max: params.talk_wait_max,
            },
            inbox: Mutex::new(Inbox {
                queues: Queues::with_bounds(lane_max, total_max),
                control: Control::default(),
            }),
            cv: Condvar::new(),
            entries: EntryReader::new(opts.database.clone()),
            searcher,
            waiters: Mutex::new(Waiters::new()),
            binding,
            tokens,
            stats: Stats::default(),
            alive: AtomicBool::new(true),
            draining: AtomicBool::new(false),
        });
        let checkpointer = crate::checkpointer::Checkpointer::start(
            &opts.database,
            opts.checkpoint_every,
            opts.wal_restart,
        )?;
        let w = writer.attach(Arc::clone(&sh), feeder.clone());
        let thread = std::thread::Builder::new()
            .name("writer".into())
            .spawn(move || w.run())
            .map_err(StartError::Thread)?;
        Ok(Arc::new(Engine {
            sh,
            writer: Mutex::new(Some(thread)),
            checkpointer: Some(checkpointer),
            feeder,
            gate: opts.hooks.gate.clone(),
            lock: Mutex::new(Some(lock)),
        }))
    }

    /// Swaps the token map at the writer's next job boundary (SIGHUP, D.11.3).
    pub fn reload_tokens(&self, map: TokenMap) {
        self.sh.inbox().control.reload = Some(map);
        self.sh.cv.notify_one();
    }

    /// Wakes every parked `talk wait`, and from now on none parks (at shutdown: each then runs its
    /// `wait` command and answers, so the HTTP drain ends at once; review E4-robust-10).
    pub fn wake_all(&self) {
        self.sh.draining.store(true, Ordering::SeqCst);
        self.sh.waiters().wake_all();
    }

    /// Wakes the writer to look at the clock (a manual clock moved).
    pub fn kick(&self) {
        self.sh.inbox().control.kick = true;
        self.sh.cv.notify_one();
    }

    /// Finishes the current group and stops the writer and the other threads (SIGTERM, D.11.8).
    pub fn shutdown(&self) {
        self.sh.inbox().control.shutdown = true;
        self.sh.cv.notify_all();
        if let Some(g) = &self.gate {
            g.set(false);
        }
        let t = lock(&self.writer).take();
        if let Some(t) = t {
            let _ = t.join();
        }
        if let Some(c) = &self.checkpointer {
            c.stop();
        }
        if let Some(f) = &self.feeder {
            f.stop();
        }
        // The writer is gone: another process may take the database.
        drop(lock(&self.lock).take());
    }

    /// Whether the writer is running.
    #[must_use]
    pub fn alive(&self) -> bool {
        self.sh.alive.load(Ordering::SeqCst)
    }

    /// What the engine did.
    #[must_use]
    pub fn stats(&self) -> &Stats {
        &self.sh.stats
    }

    /// Waits until the search feeder has fed the whole history the writer committed, at most
    /// `timeout`; whether it has (tests and tools; production never waits for it).
    #[must_use]
    pub fn search_caught_up(&self, committed: u64, timeout: Duration) -> bool {
        self.feeder
            .as_ref()
            .is_none_or(|f| f.wait_fed(committed, timeout))
    }

    /// How many `talk wait` handlers are parked.
    #[must_use]
    pub fn parked(&self) -> usize {
        self.sh.waiters().len()
    }

    /// How many jobs are queued.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.sh.inbox().queues.len()
    }

    fn now(&self) -> Time {
        self.sh.clock.now()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The answer to a call the writer never answered: it stopped (crash-only, or shutdown).
fn gone() -> Reply {
    Reply {
        text: "error: unavailable\nhint: the server stopped before answering; nothing was \
               confirmed: retry with the same op_id"
            .into(),
        structured: None,
        is_error: true,
        session: None,
    }
}

/// Removes a parked wait when its handler ends, however it ends.
struct Unpark<'a> {
    sh: &'a Shared,
    id: u64,
}

impl Drop for Unpark<'_> {
    fn drop(&mut self) {
        self.sh.waiters().unregister(self.id);
    }
}

/// The shell's refusal of a call by `class` that names `session` (D.11.7): with the status line,
/// rendered by the writer in the class's refusals lane, when the session is an existing one;
/// without, when the call names none or that lane is itself busy.
async fn refuse(sh: &Shared, class: ClassName, session: SessionCtx, answer: ShellAnswer) -> Reply {
    let plain = |answer: &ShellAnswer| Reply {
        text: answer.render(),
        structured: Some(serde_json::json!({
            "error": answer.code_name(),
            "hint": answer.hint(),
        })),
        is_error: true,
        session: None,
    };
    let SessionCtx::Existing(session) = session else {
        return plain(&answer);
    };
    let key = (class.clone(), Lane::Refusals);
    let busy = crate::limits::busy(
        sh.inbox().queues.oldest(&key),
        sh.clock.mono(),
        sh.knobs.queue_max_wait,
    );
    if busy {
        return plain(&answer);
    }
    let (tx, rx) = oneshot::channel();
    let work = Work::Refuse {
        class,
        session,
        answer: Box::new(answer.clone()),
        reply: tx,
    };
    match sh.submit(key, work) {
        Ok(()) => rx.await.unwrap_or_else(|_| gone()),
        Err(_) => plain(&answer),
    }
}

/// The long poll of `talk wait` (D.11.11): asks the writer whether the session is ready,
/// registering it as parked if not, then sleeps until a wake-up or the deadline, which is
/// `min(timeout, talk.wait_max)` after the handler started: time spent queued counts against it
/// (review E4-spec-10). Once the server is stopping it does not sleep (review E4-robust-10). The
/// `wait` command itself runs afterwards as an ordinary job.
async fn park(
    sh: &Shared,
    class: &ClassName,
    session: SessionId,
    for_: Option<NodeRef>,
    timeout: Dur,
) -> Result<(), ()> {
    let deadline = tokio::time::Instant::now()
        + Duration::from_millis(timeout.0.min(sh.knobs.talk_wait_max.0));
    let (tx, rx) = oneshot::channel();
    let work = Work::Ready {
        class: class.clone(),
        session,
        for_,
        reply: tx,
    };
    sh.submit((class.clone(), Lane::Calls), work)
        .map_err(|_| ())?;
    match rx.await {
        Ok(Readiness::Ready) => Ok(()),
        Ok(Readiness::Park(p)) => {
            let _unpark = Unpark { sh, id: p.id };
            if sh.draining.load(Ordering::SeqCst) {
                return Ok(());
            }
            tokio::select! {
                () = p.notify.notified() => {}
                () = tokio::time::sleep_until(deadline) => {}
            }
            Ok(())
        }
        Err(_) => Err(()),
    }
}

/// A tool call, as its handler runs it (D.8.1, D.11.11).
async fn call(sh: Arc<Shared>, call: Call) -> Reply {
    let class = call.caller.class.clone();
    if let (Cmd::Talk(TalkCmd::Wait { timeout, for_, .. }), SessionCtx::Existing(session)) =
        (&call.prepared.cmd, call.session)
        && park(&sh, &class, session, for_.clone(), *timeout)
            .await
            .is_err()
    {
        return busy_or_gone(&sh, class, call.session).await;
    }
    // Search, for `hello`, `from_user`, `create`, and `view find {text}` (D.11.12).
    let hits = match (Query::of(&call.prepared.cmd), &sh.searcher) {
        (Some(q), Some(_)) => {
            let s = Arc::clone(&sh);
            tokio::task::spawn_blocking(move || {
                #[cfg(feature = "bench")]
                let t = mono();
                let hits = s
                    .searcher
                    .as_ref()
                    .map(|x| x.search(&q))
                    .unwrap_or_default();
                // The search alone, apart from the call around it (gate G-9).
                #[cfg(feature = "bench")]
                crate::bench::searched(&q, mono().since(t));
                hits
            })
            .await
            .unwrap_or_default()
        }
        _ => SearchHits::default(),
    };
    let session = call.session;
    let (tx, rx) = oneshot::channel();
    let work = Work::Call {
        call: Box::new(call),
        hits,
        reply: tx,
    };
    if sh.submit((class.clone(), Lane::Calls), work).is_err() {
        return busy_or_gone(&sh, class, session).await;
    }
    rx.await.unwrap_or_else(|_| gone())
}

async fn busy_or_gone(sh: &Shared, class: ClassName, session: SessionCtx) -> Reply {
    if sh.alive.load(Ordering::SeqCst) {
        refuse(sh, class, session, ShellAnswer::Busy).await
    } else {
        gone()
    }
}

impl Backend for Engine {
    fn now(&self) -> Time {
        Engine::now(self)
    }

    fn text_max(&self) -> u32 {
        self.sh.knobs.text_max
    }

    fn coverage_window(&self) -> Dur {
        self.sh.knobs.coverage_window
    }

    fn entries(&self) -> &(dyn EntryLookup + Sync) {
        &self.sh.entries
    }

    fn call(&self, c: Call) -> impl Future<Output = Reply> + Send {
        call(Arc::clone(&self.sh), c)
    }

    fn refuse(&self, r: Refusal) -> impl Future<Output = Reply> + Send {
        let sh = Arc::clone(&self.sh);
        async move { refuse(&sh, r.caller.class, r.session, r.answer).await }
    }
}

/// The sessions an event retires from the shell's maps: at its `Fired { Silence }` or its
/// `SessionRetired` (CS-17, D.11.4).
#[must_use]
pub fn sessions_leaving(events: &[forum_core::Event]) -> BTreeSet<SessionId> {
    use forum_core::{DeadlineKind, Ev, Owner};
    events
        .iter()
        .filter_map(|e| match &e.ev {
            Ev::Fired {
                kind: DeadlineKind::Silence,
                owner: Owner::Session(x),
            }
            | Ev::SessionRetired { session: x } => Some(*x),
            _ => None,
        })
        .collect()
}

/// Mono, re-exported for the writer's linger.
pub(crate) fn mono() -> Mono {
    Mono::now()
}

/// What `forum-server check` found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckReport {
    /// The events of the history.
    pub events: u64,
    /// The nodes in memory.
    pub nodes: usize,
    /// The sessions in memory.
    pub sessions: usize,
}

/// `forum-server check` (D.9.1, D.11.8 step 4): the full invariant check of the database at `db`,
/// which it opens read-only and reads in one transaction, so it checks one snapshot of a database
/// the server may be writing, writes nothing, and copies nothing (review E4-spec-14,
/// E4-robust-6). The state is loaded as startup loads it, settled in memory to its `last_now`,
/// and checked with `check_keys` and `check_invariants`; then K2's shell part runs over the
/// tables that outlive memory ([`check_k2_tables`]).
///
/// # Errors
///
/// A database that cannot be opened or loaded, a `Stuck` settle, or the violations found.
pub fn check(db: &std::path::Path) -> Result<CheckReport, StartError> {
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(StoreError::from)?;
    conn.execute_batch("BEGIN").map_err(StoreError::from)?;
    let result = (|| {
        if !crate::store::schema::has_schema(&conn)? {
            return Err(StartError::Store(StoreError::Invalid(
                "the database has no forum in it".into(),
            )));
        }
        let mut s = crate::store::load::load_state(&conn)?;
        let at = s.last_now;
        loop {
            match forum_core::settle(&mut s, at, u32::MAX).1 {
                forum_core::Settled::Done => break,
                forum_core::Settled::More => {}
                forum_core::Settled::Stuck(k) => {
                    return Err(StartError::Store(StoreError::Invalid(format!(
                        "settle is stuck at {k:?}"
                    ))));
                }
            }
        }
        crate::store::load::check_loaded(&s, Checks::Full)?;
        check_k2_tables(&conn, &s)?;
        Ok(CheckReport {
            events: s.counters.events,
            nodes: s.nodes.len(),
            sessions: s.sessions.len(),
        })
    })();
    let _ = conn.execute_batch("ROLLBACK");
    result
}

/// The entry of a canonical `user:e<n>` key, the only form of version 0 besides `repeat:`.
fn user_entry(k: &forum_core::KeyStr) -> Option<Eid> {
    k.user_entry()
        .filter(|e| forum_core::KeyStr::of_user_entry(*e) == *k)
}

/// Whether `k` has the form of an occurrence's key, `repeat:<series id>:<YYYYMMDDTHHMMZ>` (the
/// core checks the resident ones in full, K2).
fn occurrence_shaped(k: &forum_core::KeyStr) -> bool {
    k.as_str()
        .strip_prefix("repeat:")
        .and_then(|rest| rest.split_once(':'))
        .is_some_and(|(id, slot)| {
            !id.is_empty()
                && id.bytes().all(|b| b.is_ascii_digit())
                && slot.len() == 14
                && slot.as_bytes().get(8) == Some(&b'T')
                && slot.ends_with('Z')
        })
}

/// K2's shell part over the tables that outlive memory (D.9.2, review E4-spec-5): every row of the
/// `origins` table (origins whose request left memory) is normal under its version, or, at version
/// 0, a canonical `user:e<n>` with `n` below `next_eid` or an occurrence's key; and every version-0
/// `user:e<n>` key of a node (resident, or a row of `st_keys`) or origin (resident, or a row of
/// `origins`) has a `from_user` row for `e<n>` in the `entries` table.
///
/// # Errors
///
/// A failed query, or `Check` with every violation found.
pub fn check_k2_tables(conn: &Connection, s: &forum_core::State) -> Result<(), StoreError> {
    use forum_core::{Inv, KeyStr, NormVer, Violation, ViolationDetail};
    let mut bad: Vec<Violation> = Vec::new();
    let key = |k: &KeyStr| Violation {
        inv: Inv::K2,
        detail: ViolationDetail::Key(k.clone()),
    };
    let mut users: BTreeSet<(KeyStr, Eid)> = BTreeSet::new();
    let mut origins = conn.prepare("SELECT origin, nv FROM origins ORDER BY origin")?;
    let rows = origins.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    for row in rows {
        let (origin, nv) = row?;
        let Some(k) = KeyStr::new(&origin) else {
            bad.push(Violation {
                inv: Inv::K2,
                detail: ViolationDetail::Text("an origin that is not a key"),
            });
            continue;
        };
        let nv = u16::try_from(nv).unwrap_or(u16::MAX);
        if nv == 0 {
            match user_entry(&k) {
                Some(e) if e < s.counters.next_eid => {
                    users.insert((k, e));
                }
                None if occurrence_shaped(&k) => {}
                Some(_) | None => bad.push(key(&k)),
            }
        } else if !forum_keys::is_normal(k.as_str(), nv) {
            bad.push(key(&k));
        }
    }
    drop(origins);
    let mut keys = conn.prepare("SELECT DISTINCT key FROM st_keys WHERE nv = 0 ORDER BY key")?;
    for k in keys.query_map([], |r| r.get::<_, String>(0))? {
        if let Some(k) = KeyStr::new(&k?)
            && let Some(e) = user_entry(&k)
        {
            users.insert((k, e));
        }
    }
    drop(keys);
    for node in s.nodes.values() {
        for (k, nv) in &node.keys {
            if *nv == NormVer(0)
                && let Some(e) = user_entry(k)
            {
                users.insert((k.clone(), e));
            }
        }
    }
    for r in s.requests.values() {
        if r.origin.nv == NormVer(0)
            && let Some(e) = user_entry(&r.origin.key)
        {
            users.insert((r.origin.key.clone(), e));
        }
    }
    let mut from_user = conn
        .prepare("SELECT EXISTS (SELECT 1 FROM entries WHERE eid = ?1 AND type = 'from_user')")?;
    for (k, e) in &users {
        let eid = i64::try_from(e.0).unwrap_or(i64::MAX);
        let found: bool = from_user.query_row([eid], |r| r.get(0))?;
        if !found {
            bad.push(key(k));
        }
    }
    if bad.is_empty() {
        Ok(())
    } else {
        Err(StoreError::Check(bad))
    }
}
