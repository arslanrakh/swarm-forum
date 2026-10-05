//! The writer thread (design D.11.2, D.8.1, D.11.8): the single writer of §24. It owns the
//! read-write connection and the in-memory `State`, and runs one call at a time, in groups that
//! share a transaction.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use forum_core::health::Health;
use forum_core::query::{self, NoMeter};
use forum_core::{
    AdminCmd, ClassName, Cmd, Ctx, Done, Error, Event, Hint, NextReply, NodeCmd, NodeId, NodeRef,
    OpCtx, Params, Reply as CoreReply, Seq, SessionCtx, SessionId, Settled, State, TalkCmd, Time,
    Tracked, ViewCmd, ViewMode, What, Why, apply_tracked, check_fire, check_guide_with,
    check_invariants, check_step, restore_tracked, settle_tracked,
};
use rusqlite::ErrorCode;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use super::{
    COMMIT_GROUP_MAX, COMMIT_LINGER, Feeder, HealthCache, Hooks, IDLE_MAX, OnFatal, Options,
    Readiness, Shared, StartError, WRITER_BUSY, Work, lock, mono, sessions_leaving, with_surges,
};
use crate::clock::{Clock, until};
use crate::decode::{FillVersions, ShellAnswer, versions_not_shown};
use crate::limits::{WRITE_SURGE_FLOOR, WriteCounts, WriteWindows};
use crate::mask::MASKING_VERSION;
use crate::mcp::server::{Call, Reply};
use crate::render::{self, Input, SearchHits, SeenBySession, StoreSource};
use crate::search::FORUM_ID;
use crate::shown::{Shown, ShownVersions};
#[cfg(not(feature = "bench"))]
use crate::store::load::open;
#[cfg(feature = "bench")]
use crate::store::load::open_connection;
use crate::store::load::{Genesis, check_loaded};
#[cfg(not(feature = "bench"))]
use crate::store::migrate::RELEASED;
use crate::store::schema::{get_meta, set_meta};
use crate::store::state::{put_op, stored_op};
use crate::store::{Store, StoreError, TxKind, TxMeta, restore, write};
use crate::wait::{Coalescer, NewsSeen, concerned};

/// A fatal error: the writer ends (crash-only, D.11.2).
#[derive(Debug)]
pub(crate) struct Fatal(pub(crate) String);

impl From<StoreError> for Fatal {
    fn from(e: StoreError) -> Fatal {
        Fatal(format!("storage: {e}"))
    }
}

impl From<rusqlite::Error> for Fatal {
    fn from(e: rusqlite::Error) -> Fatal {
        Fatal(format!("sqlite: {e}"))
    }
}

/// A rendered response as `st_ops` stores it, returned verbatim to a repeat (O1, D.11.6).
#[derive(Serialize, Deserialize)]
struct Stored {
    text: String,
    structured: Option<serde_json::Value>,
    is_error: bool,
}

/// A reply held until its group commits.
enum Pending {
    Reply(oneshot::Sender<Reply>, Reply),
    Ready(oneshot::Sender<Readiness>, Readiness),
}

/// What a group collected.
#[derive(Default)]
struct Group {
    replies: Vec<Pending>,
    events: Vec<Event>,
    /// Something committed during the group (a settle's batch boundary).
    committed: bool,
}

/// What the writer waits for when it looks for a job.
#[derive(Clone, Copy)]
enum Wait {
    /// Idle: until a job, or the first timer.
    Idle,
    /// Within a group: at most this long.
    Linger(Duration),
    /// Within a group: only a job already queued.
    Now,
}

/// What it found.
enum Next {
    Job(Work),
    /// A timer is due, or a kick asked for a look at the clock.
    Timer,
    /// Nothing within the linger.
    Nothing,
    Stop,
}

/// What a settle does at the boundary of two batches (D.8.1, D.8.2).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Boundary {
    /// Commits the group so far: a pre-settle, an idle settle, and the startup settles.
    Commit,
    /// Keeps every batch in the open transaction: a post-settle, whose job's `st_ops` row is
    /// written after it, so that its events never commit without the stored response (O1;
    /// review E4-spec-1). The job commits after that row when the settle ran past one batch.
    Hold,
}

pub(crate) struct Writer {
    sh: Option<Arc<Shared>>,
    store: Store,
    s: State,
    /// T1's floor in memory: a call that appends nothing still never sees time go back (A-101).
    max_now: Time,
    in_tx: bool,
    clock: Arc<dyn Clock>,
    health: HealthCache,
    windows: WriteWindows,
    counts: WriteCounts,
    news: NewsSeen,
    /// What each session's responses showed it: the health of its last status line and the
    /// agreement texts of its warnings (1.7, §20.1).
    seen: SeenBySession,
    /// The versions each session was shown of each node (1.7, §20.5).
    shown: ShownVersions,
    coalesce: Coalescer,
    feeder: Option<Arc<Feeder>>,
    hooks: Hooks,
    on_fatal: OnFatal,
    check_every_call: bool,
    #[cfg(feature = "usability")]
    arm: crate::render::usability::Arm,
    /// Within the stretch of work being timed, how long `take` waited for more jobs (feature
    /// `bench`: gate G-2 counts the writer busy without these waits).
    #[cfg(feature = "bench")]
    waited: Duration,
    /// When the sizes of the retained collections were last recorded (feature `bench`, gate G-6).
    #[cfg(feature = "bench")]
    sized: crate::clock::Mono,
}

/// The `meta` row of the forum's identity, which its search file copies (D.11.12; review
/// E4-spec-6): a random id, written once, at the first startup of a database.
fn ensure_forum_id(conn: &rusqlite::Connection, path: &std::path::Path) -> Result<(), StoreError> {
    if get_meta(conn, FORUM_ID)?.is_none() {
        set_meta(conn, FORUM_ID, &random_id(path))?;
    }
    Ok(())
}

/// 128 random bits in hexadecimal; from the clocks, the process, and the path when the system has
/// no randomness to give.
fn random_id(path: &std::path::Path) -> String {
    let mut bytes = [0_u8; 16];
    let random = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut bytes))
        .is_ok();
    if !random {
        let seed = format!(
            "{}:{:?}:{}:{}",
            crate::clock::now_ms().0,
            mono(),
            std::process::id(),
            path.display()
        );
        let h = crate::auth::sha256(seed.as_bytes());
        bytes.copy_from_slice(&h[..16]);
    }
    crate::auth::hex(&bytes)
}

/// Whether SQLite answered that another connection holds the lock.
fn busy(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _)
        if f.code == ErrorCode::DatabaseBusy || f.code == ErrorCode::DatabaseLocked)
}

/// The message of a panic's payload.
fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic without a message".into())
}

impl Writer {
    /// D.11.8 steps 1 to 4, 6, and 7: open (creating Genesis), settle to `last_now`, check, apply
    /// the configured parameters, settle to now. Returns the writer and the parameters in force.
    pub(crate) fn open(opts: &Options) -> Result<(Writer, Params), StartError> {
        let g = Genesis {
            params: opts.params.clone(),
            now: opts.clock.now(),
            masking_version: MASKING_VERSION,
        };
        #[cfg(not(feature = "bench"))]
        let o = open(&opts.database, &g, &RELEASED)?;
        // The bench build traces the statements startup runs (gate G-7: no history read) and counts
        // the WAL frames the commits append (gate G-8).
        #[cfg(feature = "bench")]
        let o = {
            let conn = rusqlite::Connection::open(&opts.database).map_err(StoreError::from)?;
            crate::bench::trace_startup(&conn);
            let o = open_connection(conn, &g, &opts.steps)?;
            crate::bench::count_wal(o.store.conn()).map_err(StoreError::from)?;
            o
        };
        o.store
            .conn()
            .pragma_update(
                None,
                "journal_size_limit",
                i64::try_from(opts.wal_restart).unwrap_or(i64::MAX),
            )
            .map_err(StoreError::from)?;
        // Longer than any other connection holds the write lock (review E4-robust-1).
        o.store
            .conn()
            .busy_timeout(opts.hooks.writer_busy.unwrap_or(WRITER_BUSY))
            .map_err(StoreError::from)?;
        ensure_forum_id(o.store.conn(), &opts.database)?;
        if o.created {
            tracing::info!(path = %opts.database.display(), "created the forum");
        }
        let at = o.state.last_now;
        let mut w = Writer {
            sh: None,
            store: o.store,
            s: o.state,
            max_now: at,
            in_tx: false,
            clock: Arc::clone(&opts.clock),
            health: HealthCache::new(
                Health {
                    level: forum_core::health::Level::Green,
                    reasons: Vec::new(),
                    capacity: forum_core::health::CapacityReport::default(),
                },
                at,
            ),
            windows: WriteWindows::new(),
            counts: WriteCounts::with_floor(
                opts.hooks.write_surge_floor.unwrap_or(WRITE_SURGE_FLOOR),
            ),
            news: NewsSeen::new(),
            seen: SeenBySession::new(),
            shown: ShownVersions::new(),
            coalesce: Coalescer::new(),
            feeder: None,
            hooks: opts.hooks.clone(),
            on_fatal: opts.on_fatal,
            check_every_call: opts.check_every_call,
            #[cfg(feature = "usability")]
            arm: opts.arm,
            #[cfg(feature = "bench")]
            waited: Duration::ZERO,
            #[cfg(feature = "bench")]
            sized: crate::clock::Mono::default(),
        };
        let fatal = |f: Fatal| StartError::Store(StoreError::Invalid(f.0));
        // Step 3: a crash between two settle batches may have left due deadlines.
        w.settle_to(at, &mut Vec::new(), Boundary::Commit)
            .map_err(fatal)?;
        w.commit().map_err(fatal)?;
        // Step 4.
        check_loaded(&w.s, opts.checks)?;
        // Step 6.
        if w.s.params != opts.params {
            w.set_params(opts.params.clone())?;
        }
        // Step 7.
        let now = w.now();
        w.settle_to(now, &mut Vec::new(), Boundary::Commit)
            .map_err(fatal)?;
        w.commit().map_err(fatal)?;
        w.health = HealthCache::new(w.compute_health(now), now);
        let params = w.s.params.clone();
        #[cfg(feature = "bench")]
        crate::bench::end_startup(w.store.conn());
        Ok((w, params))
    }

    /// `Cmd::Admin(SetParams(p))` under `SessionCtx::System` as its own transaction, at the
    /// state's `last_now` (D.11.8 step 6).
    fn set_params(&mut self, p: Params) -> Result<(), StartError> {
        let now = self.s.last_now;
        let ctx = Ctx {
            now,
            class: ClassName::new("server")
                .ok_or_else(|| StartError::Store(StoreError::Invalid("the class name".into())))?,
            caps: None,
            session: SessionCtx::System,
            op: None,
        };
        let mut dirty = Tracked::new();
        let (evs, out) = apply_tracked(
            &mut self.s,
            &ctx,
            &Cmd::Admin(AdminCmd::SetParams(p)),
            &mut dirty,
        );
        if let Err(e) = out {
            return Err(StartError::Params(e));
        }
        let fatal = |f: Fatal| StartError::Store(StoreError::Invalid(f.0));
        self.persist(&TxMeta::new(TxKind::Admin, now), &evs, &dirty)
            .map_err(fatal)?;
        self.commit().map_err(fatal)?;
        tracing::info!("applied the configured parameters");
        Ok(())
    }

    /// The writer with the handlers' shared state, ready to run.
    pub(crate) fn attach(mut self, sh: Arc<Shared>, feeder: Option<Arc<Feeder>>) -> Writer {
        self.sh = Some(sh);
        self.feeder = feeder;
        if let Some(f) = &self.feeder {
            f.committed(self.s.counters.events);
        }
        self
    }

    #[allow(
        clippy::expect_used,
        reason = "the writer runs only once attached; a bug otherwise"
    )]
    fn sh(&self) -> &Arc<Shared> {
        self.sh.as_ref().expect("attached")
    }

    /// The thread's body: serve until shutdown or a fatal error. A panic is a fatal error too: the
    /// writer is marked stopped and its queue drained, so no caller waits on a writer that is gone,
    /// and production aborts (crash-only; review E4-robust-3).
    pub(crate) fn run(mut self) {
        let sh = Arc::clone(self.sh());
        let result = match std::panic::catch_unwind(AssertUnwindSafe(|| self.serve())) {
            Ok(r) => r,
            Err(p) => Err(Fatal(format!(
                "the writer panicked: {}",
                panic_message(p.as_ref())
            ))),
        };
        sh.alive.store(false, Ordering::SeqCst);
        if let Err(Fatal(msg)) = result {
            tracing::error!(error = %msg, "the writer stopped (crash-only)");
            if self.on_fatal == OnFatal::Abort {
                std::process::abort();
            }
        }
        // Whatever is still queued is dropped: its callers hear that the server stopped.
        let left = sh.inbox().queues.drain();
        drop(left);
        // Dropping the connection rolls back an open transaction.
        drop(self);
    }

    fn serve(&mut self) -> Result<(), Fatal> {
        loop {
            match self.take(Wait::Idle) {
                Next::Stop => return Ok(()),
                Next::Timer | Next::Nothing => {
                    #[cfg(feature = "bench")]
                    let from = self.busy_from();
                    self.idle()?;
                    #[cfg(feature = "bench")]
                    self.busy_until(from);
                }
                Next::Job(w) => {
                    #[cfg(feature = "bench")]
                    let from = self.busy_from();
                    self.group(w)?;
                    #[cfg(feature = "bench")]
                    self.busy_until(from);
                }
            }
        }
    }

    /// Starts timing a stretch of the writer's work (feature `bench`, gate G-2).
    #[cfg(feature = "bench")]
    fn busy_from(&mut self) -> crate::clock::Mono {
        self.waited = Duration::ZERO;
        mono()
    }

    /// Adds the stretch since `from` to the writer's busy time, its waits for more jobs left out,
    /// and records the history's length (feature `bench`, gates G-2, G-4, and G-10).
    #[cfg(feature = "bench")]
    fn busy_until(&mut self, from: crate::clock::Mono) {
        let d = mono().since(from).saturating_sub(self.waited);
        let sh = Arc::clone(self.sh());
        sh.stats.busy_ns.fetch_add(
            u64::try_from(d.as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        sh.stats
            .events
            .store(self.s.counters.events, Ordering::Relaxed);
        self.record_sizes(&sh);
    }

    /// Records, at most once a second, the sizes of the collections a retention bounds (feature
    /// `bench`, gate G-6): the state's resident entities and the shell's maps.
    #[cfg(feature = "bench")]
    fn record_sizes(&mut self, sh: &Shared) {
        let t = mono();
        if self.sized != crate::clock::Mono::default()
            && t.since(self.sized) < Duration::from_secs(1)
        {
            return;
        }
        self.sized = t;
        let s = &self.s;
        crate::bench::set_sizes(&[
            ("sessions", s.sessions.len()),
            ("ops", s.ops.len()),
            ("notes", s.notes.len()),
            ("aquestions", s.aquestions.len()),
            ("pquestions", s.pquestions.len()),
            ("uquestions", s.uquestions.len()),
            ("digest", s.digest.len()),
            ("entries", s.entries.len()),
            ("requests", s.requests.len()),
            ("commitments", s.commitments.len()),
            ("saids", s.saids.len()),
            ("decisions", s.decisions.len()),
            ("objections", s.objections.len()),
            ("user_entries", s.user_entries.len()),
            ("holds", s.holds.len()),
            ("nodes", s.nodes.len()),
            ("leaving", s.leaving.len()),
            ("shell_bindings", lock(&sh.binding).len()),
            ("shell_news", self.news.len()),
            ("shell_seen", self.seen.len()),
            ("shell_shown", self.shown.len()),
            ("shell_windows", self.windows.sessions()),
        ]);
    }

    /// The idle wait: until the first deadline, the health cache's refresh, or a deferred
    /// wake-up, and at most [`IDLE_MAX`] (D.11.2).
    fn idle_timeout(&self) -> Duration {
        let mut at = Vec::new();
        if let Some(k) = self.s.ix.deadlines.first() {
            at.push(k.at);
        }
        if let Some(t) = self.health.due() {
            at.push(t);
        }
        if let Some(t) = self.coalesce.next_due() {
            at.push(t);
        }
        at.into_iter()
            .map(|t| until(self.clock.as_ref(), t, IDLE_MAX))
            .min()
            .unwrap_or(IDLE_MAX)
    }

    /// Takes the next job by the queues' round, applying the control requests first, at a job
    /// boundary (D.11.2: the token map is swapped there). Skips jobs whose caller has gone.
    fn take(&mut self, wait: Wait) -> Next {
        if let Some(g) = &self.hooks.gate {
            g.pass();
        }
        let budget = match wait {
            Wait::Idle => self.idle_timeout(),
            Wait::Linger(d) => d,
            Wait::Now => Duration::ZERO,
        };
        let start = mono();
        let sh = Arc::clone(self.sh());
        let mut inbox = sh.inbox();
        loop {
            if let Some(map) = inbox.control.reload.take() {
                sh.tokens.swap(map);
                tracing::info!("swapped the token map");
            }
            if inbox.control.shutdown {
                return Next::Stop;
            }
            if let Some(g) = self.hooks.gate.as_ref().filter(|g| g.is_closed()) {
                drop(inbox);
                g.pass();
                inbox = sh.inbox();
                continue;
            }
            while let Some((_, _, work)) = inbox.queues.pop() {
                if work.closed() {
                    sh.stats.skipped.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                return Next::Job(work);
            }
            if inbox.control.kick {
                inbox.control.kick = false;
                if matches!(wait, Wait::Idle) {
                    return Next::Timer;
                }
            }
            let left = budget.saturating_sub(mono().since(start));
            if left.is_zero() {
                return match wait {
                    Wait::Idle => Next::Timer,
                    Wait::Linger(_) | Wait::Now => Next::Nothing,
                };
            }
            #[cfg(feature = "bench")]
            let waiting = mono();
            inbox = match sh.cv.wait_timeout(inbox, left) {
                Ok((g, _)) => g,
                Err(p) => p.into_inner().0,
            };
            #[cfg(feature = "bench")]
            if !matches!(wait, Wait::Idle) {
                self.waited = self.waited.saturating_add(mono().since(waiting));
            }
        }
    }

    /// `max(clock, last_now, max_now)`, recorded as the new floor (T1, D.8.1).
    fn now(&mut self) -> Time {
        let now = self.clock.now().max(self.s.last_now).max(self.max_now);
        self.max_now = now;
        now
    }

    /// Whether the engine asked the writer to stop.
    fn stopping(&self) -> bool {
        self.sh
            .as_ref()
            .is_some_and(|sh| sh.inbox().control.shutdown)
    }

    /// `BEGIN IMMEDIATE`. Only another connection's checkpoint holds the write lock (D.11.2), and
    /// it lets go within the writer's busy timeout; should it not, the writer tries again rather
    /// than end: the state in memory is ahead of the database, but no reply of the group has been
    /// sent, so waiting loses nothing (review E4-robust-1).
    fn begin(&mut self) -> Result<(), Fatal> {
        if self.in_tx {
            return Ok(());
        }
        loop {
            match self.store.conn().execute_batch("BEGIN IMMEDIATE") {
                Ok(()) => break,
                Err(e) if busy(&e) => {
                    if let Some(sh) = &self.sh {
                        sh.stats.busy_begins.fetch_add(1, Ordering::Relaxed);
                    }
                    if self.stopping() {
                        return Err(Fatal(format!("stopped while the database was locked: {e}")));
                    }
                    tracing::warn!(error = %e, "the database is locked: the writer tries again");
                }
                Err(e) => return Err(e.into()),
            }
        }
        self.in_tx = true;
        Ok(())
    }

    fn commit(&mut self) -> Result<(), Fatal> {
        if self.in_tx {
            self.store.conn().execute_batch("COMMIT")?;
            self.in_tx = false;
            if let Some(sh) = &self.sh {
                sh.stats.commits.fetch_add(1, Ordering::Relaxed);
            }
            if let Some(f) = &self.hooks.after_commit {
                f(&self.s);
            }
        }
        Ok(())
    }

    /// Writes one transaction of events just applied, with their dirty facts (D.11.6); nothing
    /// for no events (CS-14).
    fn persist(&mut self, meta: &TxMeta, events: &[Event], dirty: &Tracked) -> Result<(), Fatal> {
        if events.is_empty() {
            return Ok(());
        }
        self.begin()?;
        write(self.store.conn(), meta, events, &self.s, dirty)?;
        Ok(())
    }

    /// One batch of `settle.batch` firings. With the checks of every call (debug builds), the keys
    /// fire one at a time, each checked with `check_fire` (D.9.1, review E4-spec-4); the batch and
    /// its events are the same.
    fn settle_batch(
        &mut self,
        now: Time,
        dirty: &mut Tracked,
    ) -> Result<(Vec<Event>, Settled), Fatal> {
        let batch = self.s.params.settle_batch;
        if !self.check_every_call {
            return Ok(settle_tracked(&mut self.s, now, batch, dirty));
        }
        let mut out = Vec::new();
        for _ in 0..batch.max(1) {
            let before = self.s.clone();
            let (evs, st) = settle_tracked(&mut self.s, now, 1, dirty);
            if let Settled::Stuck(k) = st {
                return Ok((out, Settled::Stuck(k)));
            }
            if !evs.is_empty()
                && let Some(key) = before.ix.deadlines.first()
            {
                if let Err(v) = check_fire(&before, key, &evs, &self.s) {
                    return Err(Fatal(format!("check_fire at {key:?}: {v:?}")));
                }
                if let Some(sh) = &self.sh {
                    sh.stats.fires_checked.fetch_add(1, Ordering::Relaxed);
                }
            }
            out.extend(evs);
            if st == Settled::Done {
                return Ok((out, Settled::Done));
            }
        }
        Ok((out, Settled::More))
    }

    /// Settles to `now` in batches of `settle.batch`, each batch a settle transaction; at a batch
    /// boundary the group commits or holds, by `boundary` (D.8.1, D.8.2). The shell's maps drop a
    /// session as soon as its silence or retirement fires, so the call that revives it records
    /// after the drop (CS-17, D.11.4; review E4-spec-3). A completed settle is checked with
    /// `check_invariants` in debug builds (D.9.1). Returns whether it ran past one batch.
    fn settle_to(
        &mut self,
        now: Time,
        events: &mut Vec<Event>,
        boundary: Boundary,
    ) -> Result<bool, Fatal> {
        let mut more = false;
        loop {
            let mut dirty = Tracked::new();
            let (evs, st) = self.settle_batch(now, &mut dirty)?;
            self.persist(&TxMeta::new(TxKind::Settle, now), &evs, &dirty)?;
            self.leaving(&evs);
            events.extend(evs);
            match st {
                Settled::Done => {
                    if self.check_every_call {
                        if let Err(v) = check_invariants(&self.s, now) {
                            return Err(Fatal(format!("check_invariants after a settle: {v:?}")));
                        }
                        if let Some(sh) = &self.sh {
                            sh.stats.settles_checked.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    return Ok(more);
                }
                Settled::More => {
                    more = true;
                    if boundary == Boundary::Commit {
                        self.commit()?;
                    }
                }
                Settled::Stuck(k) => {
                    return Err(Fatal(format!(
                        "settle is stuck at {k:?}: a core bug (D.8.2)"
                    )));
                }
            }
        }
    }

    /// The shell's maps forget the sessions whose silence or retirement `evs` fired (CS-17).
    fn leaving(&mut self, evs: &[Event]) {
        for x in sessions_leaving(evs) {
            if let Some(sh) = &self.sh {
                lock(&sh.binding).drop_session(x);
            }
            self.windows.drop_session(x);
            self.news.drop_session(x);
            self.seen.drop_session(x);
            self.shown.drop_session(x);
        }
    }

    /// One group: its jobs in one transaction, one COMMIT, then every reply (D.11.2, CS-13).
    fn group(&mut self, first: Work) -> Result<(), Fatal> {
        let start = mono();
        let mut g = Group::default();
        self.job(first, &mut g)?;
        let mut n = 1_usize;
        loop {
            if self.hooks.crash_after_jobs == Some(n) {
                return Err(Fatal(
                    "a test hook crashed the writer inside a group".into(),
                ));
            }
            if n >= COMMIT_GROUP_MAX {
                break;
            }
            // Jobs already queued join the group whatever the time; the writer waits for new ones
            // only within the linger, and only when the group has something to commit.
            let left = COMMIT_LINGER.saturating_sub(mono().since(start));
            let wait = if self.in_tx && !left.is_zero() {
                Wait::Linger(left)
            } else {
                Wait::Now
            };
            match self.take(wait) {
                Next::Job(w) => {
                    self.job(w, &mut g)?;
                    n += 1;
                }
                Next::Timer | Next::Nothing | Next::Stop => break,
            }
        }
        let sh = Arc::clone(self.sh());
        sh.stats.groups.fetch_add(1, Ordering::Relaxed);
        sh.stats
            .jobs
            .fetch_add(u64::try_from(n).unwrap_or(u64::MAX), Ordering::Relaxed);
        if let Some(f) = &self.hooks.before_commit {
            f(n);
        }
        let wrote = self.in_tx || g.committed;
        self.commit()?;
        let now = self.max_now;
        if self
            .hooks
            .drop_replies_after_commit
            .swap(false, Ordering::SeqCst)
        {
            drop(g.replies);
        } else {
            for p in g.replies {
                match p {
                    Pending::Reply(tx, r) => {
                        let _ = tx.send(r);
                    }
                    Pending::Ready(tx, r) => {
                        if let Err(Readiness::Park(p)) = tx.send(r) {
                            sh.waiters().unregister(p.id);
                        }
                    }
                }
            }
        }
        self.after_commit(&g.events, wrote, now);
        Ok(())
    }

    fn job(&mut self, work: Work, g: &mut Group) -> Result<(), Fatal> {
        let commits = self
            .sh
            .as_ref()
            .map_or(0, |s| s.stats.commits.load(Ordering::Relaxed));
        match work {
            Work::Call { call, hits, reply } => {
                let r = self.call(*call, &hits, &mut g.events)?;
                g.replies.push(Pending::Reply(reply, r));
            }
            Work::Refuse {
                class,
                session,
                answer,
                reply,
            } => {
                let now = self.now();
                let r = self.refusal(&class, Some(session), now, &answer);
                g.replies.push(Pending::Reply(reply, r));
            }
            Work::Ready {
                class,
                session,
                for_,
                reply,
            } => {
                let r = self.ready(&class, session, for_.as_ref());
                g.replies.push(Pending::Ready(reply, r));
            }
        }
        if self
            .sh
            .as_ref()
            .is_some_and(|s| s.stats.commits.load(Ordering::Relaxed) != commits)
        {
            g.committed = true;
        }
        Ok(())
    }

    /// The shell's refusal of a call that names `session`: with its status line when the session
    /// exists and is of the call's class, rendered from the state as it stands; it appends nothing
    /// (§20.1, D.11.7).
    fn refusal(
        &mut self,
        class: &ClassName,
        session: Option<SessionId>,
        now: Time,
        answer: &ShellAnswer,
    ) -> Reply {
        let sess = session.filter(|x| self.s.sessions.get(x).is_some_and(|s| &s.class == class));
        let src = StoreSource::new(self.store.conn());
        let seen = sess.and_then(|x| self.seen.get(x));
        let r = render::shell_refusal(&self.s, sess, now, &self.health.health, answer, &src, seen);
        if let (Some(x), Some(after)) = (sess, r.seen) {
            self.seen.set(x, after);
        }
        Reply {
            text: r.text,
            structured: Some(r.structured),
            is_error: true,
            session: None,
        }
    }

    /// The readiness check of a `talk wait` as it parks (D.11.11): a read-only query on the state
    /// as it stands, answered without a transaction or events (D.11.2). The waiter is registered
    /// first, so a commit after it still wakes it; a session that is ready, a call the core will
    /// refuse, or any wait once the server is stopping runs its `wait` command at once.
    fn ready(
        &mut self,
        class: &ClassName,
        session: SessionId,
        for_: Option<&NodeRef>,
    ) -> Readiness {
        let sh = Arc::clone(self.sh());
        if sh.draining.load(Ordering::SeqCst) {
            return Readiness::Ready;
        }
        let now = self.now();
        let probe = Cmd::Talk(TalkCmd::Wait {
            timeout: forum_core::Dur(0),
            for_: for_.cloned(),
            since: None,
        });
        let ctx = Ctx {
            now,
            class: class.clone(),
            caps: None,
            session: SessionCtx::Existing(session),
            op: None,
        };
        if query::precheck(&self.s, &ctx, &probe).is_some() {
            return Readiness::Ready;
        }
        let node = for_.and_then(|r| query::resolve(&self.s, r));
        let since = node.and_then(|n| self.news.get(session, n));
        let p = sh.waiters().register(session, node);
        if query::inbox_ready(&self.s, session, node, since, now) {
            sh.waiters().unregister(p.id);
            Readiness::Ready
        } else {
            Readiness::Park(p)
        }
    }

    /// One call, end to end (D.8.1).
    fn call(
        &mut self,
        c: Call,
        hits: &SearchHits,
        events: &mut Vec<Event>,
    ) -> Result<Reply, Fatal> {
        let now = self.now();
        // Pre-settle.
        self.settle_to(now, events, Boundary::Commit)?;
        let class = c.caller.class.clone();
        let mut cmd = c.prepared.cmd.clone();
        // `wait`'s `since`: the last seq reported to this session for that node (D.11.11).
        if let (Cmd::Talk(TalkCmd::Wait { for_, since, .. }), SessionCtx::Existing(x)) =
            (&mut cmd, c.session)
            && let Some(n) = for_.as_ref().and_then(|r| query::resolve(&self.s, r))
        {
            *since = self.news.get(x, n);
        }
        // `feed`'s `since` given as an entry: the node's last seq at that entry (1.6, §20.4).
        if let (Cmd::View(ViewCmd::Feed { at, since, .. }), Some(e)) =
            (&mut cmd, c.prepared.since_entry)
            && let Some(n) = query::resolve(&self.s, at)
        {
            *since = Some(Seq(crate::store::shell_tables::seq_through(
                self.store.conn(),
                n,
                e,
            )?));
        }
        // The retry identity, digested with the masking version of a live record (CS-35).
        let live = c
            .prepared
            .op_id
            .as_ref()
            .is_some_and(|id| query::op_lookup(&self.s, &class, id, now).is_some());
        let op = match &c.prepared.op_id {
            None => None,
            Some(id) => {
                let digest = if live {
                    stored_op(self.store.conn(), class.as_str(), id.as_str())?
                        .and_then(|r| c.prepared.digest_with(r.masking_version))
                        .or(c.prepared.digest)
                } else {
                    c.prepared.digest
                };
                digest.map(|digest| OpCtx {
                    id: id.clone(),
                    digest,
                })
            }
        };
        // 1.7 (§20.5): the versions a `node update` left out, from those last shown to the
        // session; with none shown, a refusal of the shell that names the node's current ones. A
        // live op_id record answers the call whatever its versions (O1), so it is not refused here.
        if let Some(fill) = c.prepared.fill
            && !live
            && let Some(answer) = self.fill_versions(&mut cmd, fill, &class, c.session)
        {
            let named = match c.session {
                SessionCtx::Existing(x) => Some(x),
                SessionCtx::New | SessionCtx::System => None,
            };
            return Ok(self.refusal(&class, named, now, &answer));
        }
        let ctx = Ctx {
            now,
            class: class.clone(),
            caps: c.caller.caps.clone(),
            session: c.session,
            op,
        };
        // write.per_hour, after O1, the session, and the target (§21's precedence, D.11.15).
        if !cmd.is_read()
            && !live
            && let SessionCtx::Existing(x) = c.session
            && self.windows.over(x, now, self.s.params.write_per_hour)
            && query::precheck(&self.s, &ctx, &cmd).is_none()
        {
            let answer = ShellAnswer::WritesPerHour {
                max: self.s.params.write_per_hour,
            };
            return Ok(self.refusal(&class, Some(x), now, &answer));
        }
        // Restores (D.4.12), unless a live record answers: an entity that left memory, or whose
        // leaving time has come with no event since (the command's first event would prune it).
        if !live {
            let named = restore::named(&self.s, &cmd);
            let items = restore::items_at(self.store.conn(), &self.s, &named, now)?;
            if !items.is_empty() {
                let mut dirty = Tracked::new();
                let evs = restore_tracked(&mut self.s, now, items, &mut dirty);
                self.persist(&TxMeta::new(TxKind::Restore, now), &evs, &dirty)?;
                events.extend(evs);
            }
        }
        // Apply.
        let before = self.check_every_call.then(|| self.s.clone());
        let mut dirty = Tracked::new();
        // 1.7 (§20.5): the session's marks that are current, which its own write may move.
        let writer_session = match c.session {
            SessionCtx::Existing(x) if !cmd.is_read() => Some(x),
            _ => None,
        };
        let marks = writer_session.map(|x| self.shown.current(&self.s, x));
        let (evs, out) = apply_tracked(&mut self.s, &ctx, &cmd, &mut dirty);
        if let (Some(x), Some(marks)) = (writer_session, marks) {
            self.follow_own_write(x, &marks, &cmd, &out);
        }
        if let Some(before) = &before
            && let Err(v) = check_step(before, &ctx, &cmd, &out, &evs, &self.s)
        {
            return Err(Fatal(format!("check_step after {cmd:?}: {v:?}")));
        }
        let opened = out.as_ref().ok().and_then(|d| d.opened).or_else(|| {
            evs.iter().find_map(|e| match &e.ev {
                forum_core::Ev::SessionOpened { session, .. } => Some(*session),
                _ => None,
            })
        });
        let session_failed = matches!(
            &out,
            Err(Error {
                hint: Hint::NotFound(What::Session) | Hint::Invalid(Why::ClassMismatch),
                ..
            })
        );
        let sess = match c.session {
            SessionCtx::Existing(x) => Some(x),
            SessionCtx::New => opened,
            SessionCtx::System => None,
        };
        let meta = TxMeta {
            kind: TxKind::Cmd,
            at: now,
            class: Some(class.clone()),
            session: sess,
            op_id: c.prepared.op_id.clone(),
            detail: None,
        };
        self.persist(&meta, &evs, &dirty)?;
        let recorded = evs
            .iter()
            .any(|e| matches!(e.ev, forum_core::Ev::OpRecorded { .. }));
        events.extend(evs);
        // Post-settle: fires a deadline the command created at exactly `now`. Its batches stay in
        // the open transaction until the job's `st_ops` row is written (review E4-spec-1); its
        // completed settle is checked with `check_invariants` in debug builds (D.9.1).
        let more = self.settle_to(now, events, Boundary::Hold)?;
        // A repeat gets the stored response, next calls included (O1).
        if let Ok(Done {
            reply: CoreReply::Replay,
            ..
        }) = &out
        {
            let id = c.prepared.op_id.as_ref().map(|o| o.as_str()).unwrap_or("");
            let stored = stored_op(self.store.conn(), class.as_str(), id)?.ok_or_else(|| {
                Fatal(format!(
                    "a live op_id record ({class}, {id}) has no stored response"
                ))
            })?;
            let r: Stored = serde_json::from_slice(&stored.response)
                .map_err(|e| Fatal(format!("a stored response does not decode: {e}")))?;
            let session = c
                .prepared
                .op_id
                .as_ref()
                .and_then(|o| query::op_lookup(&self.s, &class, o, now))
                .map(|r| r.session);
            if more {
                self.commit()?;
            }
            return Ok(Reply {
                text: r.text,
                structured: r.structured,
                is_error: r.is_error,
                session,
            });
        }
        // The next calls, on the settled state (T3, §20.9), checked by G1 in debug builds (D.9.1).
        let guide_session = if session_failed { None } else { sess };
        // 1.11 (§20.9 "Inputs"): the first row of the call's `related`, which rule 5 reads for the
        // entry a `hello` or a `talk from_user` logged.
        let related = hits.related.nodes.first().copied();
        let guidance = query::guide::guide_with(
            &self.s,
            guide_session,
            now,
            &cmd,
            &out,
            related,
            &mut NoMeter,
        );
        if self.check_every_call {
            if let Err(v) =
                check_guide_with(&self.s, guide_session, now, &cmd, &out, related, &guidance)
            {
                return Err(Fatal(format!("check_guide after {cmd:?}: {v:?}")));
            }
            if let Some(sh) = &self.sh {
                sh.stats.guides_checked.fetch_add(1, Ordering::Relaxed);
            }
        }
        #[cfg(feature = "usability")]
        let shown = self.arm.guidance(&guidance);
        #[cfg(not(feature = "usability"))]
        let shown = Some(&guidance);
        // 1.7 (§20.5): the versions of each brief shown to the session, as rendered.
        if let (Some(x), Ok(done)) = (sess, &out) {
            for n in briefs_shown(&done.reply) {
                self.shown.show(&self.s, x, n);
            }
        }
        // 1.12 (§20.1, §21): a call refused for the session it named, from an MCP session bound
        // to another live session of its class, is answered in terms of that session.
        let rebind = match (&out, c.session, &self.sh) {
            (
                Err(Error {
                    hint: Hint::NotFound(What::Session),
                    ..
                }),
                SessionCtx::Existing(x),
                Some(sh),
            ) => match lock(&sh.binding).resolve(&class, c.mcp_session.as_deref(), None) {
                SessionCtx::Existing(y) if y != x && self.s.sessions.contains_key(&y) => {
                    Some(render::Rebind {
                        session: y,
                        tool: c.prepared.tool.name(),
                        action: c.prepared.action,
                        given: c.prepared.given(),
                    })
                }
                _ => None,
            },
            _ => None,
        };
        let src = StoreSource::new(self.store.conn());
        let ignored = c.prepared.ignored_detail();
        let ignored_refused = c.prepared.ignored_detail_refused();
        let resp = render::response(&Input {
            state: &self.s,
            now,
            session: sess,
            opened,
            cmd: &cmd,
            node_args: &c.prepared.node_args,
            out: &out,
            guidance: shown,
            health: &self.health.health,
            health_at: self.health.at,
            hits,
            masked_fields: c.prepared.masked_fields,
            ignored: ignored
                .as_deref()
                .zip(ignored_refused.as_deref())
                .map(|(applied, refused)| render::IgnoredDetail { applied, refused }),
            src: &src,
            seen: sess.and_then(|x| self.seen.get(x)),
            rebind,
        });
        // What the response showed its session (1.7, §20.1); a repeat answered by a stored
        // response returned above, and changes none of it.
        if let (Some(x), Some(after)) = (sess, resp.seen) {
            self.seen.set(x, after);
        }
        let reply = Reply {
            text: resp.text,
            structured: Some(resp.structured),
            is_error: resp.is_error,
            session: sess.filter(|x| self.s.sessions.contains_key(x)),
        };
        // 1.12 (§20.1): the MCP session is bound to the session the call ran in or opened before
        // the next call of the class is served, as the handler binds it after the reply: a call
        // sent beside `hello` that the writer serves next finds the binding.
        if let (Some(sh), Some(m), Some(x)) = (&self.sh, c.mcp_session.as_deref(), reply.session) {
            lock(&sh.binding).bind(&class, m, x);
        }
        // The stored response of an op_id (D.11.6 step 4).
        if recorded && let Some(id) = &c.prepared.op_id {
            let bytes = serde_json::to_vec(&Stored {
                text: reply.text.clone(),
                structured: reply.structured.clone(),
                is_error: reply.is_error,
            })
            .map_err(|e| Fatal(format!("a response does not serialise: {e}")))?;
            put_op(
                self.store.conn(),
                &self.s,
                &class,
                id,
                MASKING_VERSION,
                &bytes,
            )?;
        }
        // The post-settle's batch boundary, now that the job's rows are all written.
        if more {
            self.commit()?;
        }
        // The shell's write counts: the session's hour and the class's buckets, skipping a call
        // whose op_id a live record answered (D.11.15).
        if !cmd.is_read() && !session_failed && !live {
            if let Some(x) = sess {
                self.windows.record(x, now);
            }
            self.counts.record(&class, now);
        }
        // What `wait` reported, for its next `since`.
        if let (
            Ok(Done {
                reply:
                    CoreReply::Waited {
                        news: Some((n, seq)),
                        ..
                    },
                ..
            }),
            Some(x),
        ) = (&out, sess)
        {
            self.news.set(x, *n, *seq);
        }
        Ok(reply)
    }

    /// Fills the versions a `node update` left out with those last shown to `session` (1.7,
    /// §20.5); the refusal when none were shown. Nothing to fill when the session or the node
    /// does not exist: the core refuses the call first (§21's order).
    fn fill_versions(
        &self,
        cmd: &mut Cmd,
        fill: FillVersions,
        class: &ClassName,
        session: SessionCtx,
    ) -> Option<ShellAnswer> {
        let Cmd::Node(NodeCmd::Update {
            at,
            version,
            digest_version,
            ..
        }) = cmd
        else {
            return None;
        };
        let SessionCtx::Existing(x) = session else {
            return None;
        };
        if self.s.sessions.get(&x).is_none_or(|s| &s.class != class) {
            return None;
        }
        let n = query::resolve(&self.s, at)?;
        let now = Shown::of(&self.s, n)?;
        let Some(m) = self.shown.get(x, n) else {
            return Some(ShellAnswer::Invalid(versions_not_shown(
                &n.render(),
                now.version.0,
                now.digest_version.0,
                fill,
            )));
        };
        if fill.version {
            *version = m.version;
        }
        if fill.digest_version {
            *digest_version = Some(m.digest_version);
        }
        None
    }

    /// After a write of `x`: its marks that were current before the write follow what the write
    /// changed, and an update whose versions were both checked, given or filled, marks what it
    /// left (1.7, §20.5).
    fn follow_own_write(
        &mut self,
        x: SessionId,
        marks: &[(NodeId, Shown)],
        cmd: &Cmd,
        out: &Result<Done, Error>,
    ) {
        self.shown.follow(&self.s, x, marks);
        if let (
            Cmd::Node(NodeCmd::Update {
                at,
                digest_version: Some(_),
                ..
            }),
            Ok(Done {
                reply: CoreReply::Updated { .. },
                ..
            }),
        ) = (cmd, out)
            && let Some(n) = query::resolve(&self.s, at)
        {
            self.shown.show(&self.s, x, n);
        }
    }

    /// The idle writer: deferred wake-ups, deadlines due (settled to now as a group of its own,
    /// CS-30), and the health cache.
    fn idle(&mut self) -> Result<(), Fatal> {
        let now = self.now();
        self.fire_due_wakes(now);
        if self.s.ix.deadlines.first().is_some_and(|k| k.at <= now) {
            let mut events = Vec::new();
            self.settle_to(now, &mut events, Boundary::Commit)?;
            let wrote = self.in_tx || !events.is_empty();
            self.commit()?;
            self.sh().stats.idle_settles.fetch_add(1, Ordering::Relaxed);
            self.after_commit(&events, wrote, now);
        } else {
            self.refresh_health(now);
        }
        Ok(())
    }

    /// After a commit: the waiters it concerns wake, the feeder learns the history's end, and the
    /// health cache refreshes when due. (The shell's maps already followed the sessions, when
    /// their silence or retirement fired.)
    fn after_commit(&mut self, events: &[Event], wrote: bool, now: Time) {
        let sh = Arc::clone(self.sh());
        #[cfg(feature = "bench")]
        let waking = mono();
        {
            let w = sh.waiters();
            if !w.is_empty() && !events.is_empty() {
                let c = concerned(&self.s, now, events, &w);
                let mut n = 0;
                for x in &c.sessions {
                    n += w.wake_session(*x);
                }
                let watched: Vec<_> = c.nodes.into_iter().filter(|x| w.watched(*x)).collect();
                // What the wake-up examined: the parked sessions it looked at (`Concerned::examined`); and
                // what the commit concerns: the sessions it addresses and the waits for the nodes
                // it appended to (gate G-4: the first must not exceed the second).
                #[cfg(feature = "bench")]
                {
                    let n = |x: usize| u64::try_from(x).unwrap_or(u64::MAX);
                    let concerned = c
                        .sessions
                        .len()
                        .saturating_add(watched.iter().map(|x| w.parked_for(*x)).sum::<usize>());
                    sh.stats
                        .wake_examined
                        .fetch_add(n(c.examined), Ordering::Relaxed);
                    sh.stats
                        .wake_concerned
                        .fetch_add(n(concerned), Ordering::Relaxed);
                    sh.stats.wake_rounds.fetch_add(1, Ordering::Relaxed);
                }
                for x in self.coalesce.appended(watched, now) {
                    n += w.wake_node(x);
                }
                sh.stats
                    .wakes
                    .fetch_add(u64::try_from(n).unwrap_or(0), Ordering::Relaxed);
            }
        }
        self.fire_due_wakes(now);
        #[cfg(feature = "bench")]
        sh.stats.wake_ns.fetch_add(
            u64::try_from(mono().since(waking).as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        if wrote {
            if let Some(f) = &self.feeder {
                f.committed(self.s.counters.events);
            }
            self.health.committed = true;
        }
        self.refresh_health(now);
    }

    fn fire_due_wakes(&mut self, now: Time) {
        let due = self.coalesce.due(now);
        if due.is_empty() {
            return;
        }
        let sh = Arc::clone(self.sh());
        let w = sh.waiters();
        let mut n = 0;
        for x in due {
            n += w.wake_node(x);
        }
        sh.stats
            .wakes
            .fetch_add(u64::try_from(n).unwrap_or(0), Ordering::Relaxed);
    }

    fn compute_health(&self, now: Time) -> Health {
        with_surges(query::health(&self.s, now), self.counts.surges(now))
    }

    fn refresh_health(&mut self, now: Time) {
        let computed = {
            let s = &self.s;
            let counts = &self.counts;
            self.health.refresh(now, || {
                with_surges(query::health(s, now), counts.surges(now))
            })
        };
        if computed && let Some(sh) = &self.sh {
            sh.stats.health_computed.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The nodes whose brief a reply shows, with their versions (1.7, §20.5): `take`, `next` with a
/// node, `create` of a new node, and `view node` as a brief or in full; a review shows no
/// `digest_version`.
fn briefs_shown(reply: &CoreReply) -> Vec<NodeId> {
    match reply {
        CoreReply::Created { id, .. } => vec![*id],
        CoreReply::Taken { node, .. }
        | CoreReply::Next(
            NextReply::Hold { node, .. }
            | NextReply::Status { node, .. }
            | NextReply::Review { node, .. },
        ) => {
            vec![*node]
        }
        CoreReply::ViewNodes {
            nodes,
            view: ViewMode::Brief | ViewMode::Full,
            ..
        } => nodes.clone(),
        _ => Vec::new(),
    }
}
