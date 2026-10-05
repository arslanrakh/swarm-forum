//! The engine's tests (design D.12.1, D.12.6; WP-E4): the queues, group commit, crashes inside a
//! group and between a commit and its replies, wake-ups with coalescing, the health cache, the
//! token reload, search, export, idle firing (T2), the write limit, the configured parameters at
//! startup, `check`, and calls end to end over MCP streamable HTTP.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use forum_core::{Dur, Params, SECOND, SessionCtx, SessionId, Time};
use serde_json::{Value, json};
use tokio::task::JoinHandle as Task;

use super::*;
use crate::auth::{Caller, SharedTokens, TokenMap, hex, sha256};
use crate::clock::ManualClock;
use crate::decode::{DecodeEnv, prepare};
use crate::mcp::server::{Backend, Call, Reply};
use crate::session_bind::SessionBinding;

/// 2026-10-01T09:00:00Z.
const T0: Time = Time(1_790_845_200_000);

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

/// A directory of the test's files, removed when dropped.
struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Dir {
        let n = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("forum-engine-{}-{tag}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A running engine over files in a temporary directory, with a manual clock.
struct H {
    dir: Dir,
    engine: Arc<Engine>,
    clock: Arc<ManualClock>,
    tokens: SharedTokens,
    binding: Arc<Mutex<SessionBinding>>,
}

fn opts(dir: &Dir, clock: &Arc<ManualClock>, params: Params, hooks: Hooks) -> Options {
    let mut o = Options::new(
        dir.0.join("forum.db"),
        Some(dir.0.join("forum-search.db")),
        params,
    );
    o.clock = Arc::clone(clock) as Arc<dyn crate::clock::Clock>;
    o.on_fatal = OnFatal::StopWriter;
    o.hooks = hooks;
    o.checkpoint_every = Duration::from_millis(50);
    o
}

fn class(c: &str) -> forum_core::ClassName {
    forum_core::ClassName::new(c).unwrap()
}

impl H {
    fn start(tag: &str, params: Params, hooks: Hooks) -> H {
        let dir = Dir::new(tag);
        let clock = Arc::new(ManualClock::new(T0));
        H::start_in(dir, clock, params, hooks)
    }

    fn start_in(dir: Dir, clock: Arc<ManualClock>, params: Params, hooks: Hooks) -> H {
        let tokens = SharedTokens::new(TokenMap::default());
        let binding = Arc::new(Mutex::new(SessionBinding::new()));
        let engine = Engine::start(
            opts(&dir, &clock, params, hooks),
            tokens.clone(),
            Arc::clone(&binding),
        )
        .unwrap();
        H {
            dir,
            engine,
            clock,
            tokens,
            binding,
        }
    }

    /// Stops the engine and starts another over the same files, with the same clock.
    fn restart(self, params: Params, hooks: Hooks) -> H {
        let H {
            dir, engine, clock, ..
        } = self;
        engine.shutdown();
        drop(engine);
        H::start_in(dir, clock, params, hooks)
    }

    fn db(&self) -> PathBuf {
        self.dir.0.join("forum.db")
    }

    fn call_of(&self, class_name: &str, session: Option<u64>, tool: &str, args: Value) -> Call {
        call_on(
            &self.engine,
            self.clock.now(),
            class_name,
            session,
            tool,
            args,
        )
    }

    async fn call(&self, session: Option<u64>, tool: &str, args: Value) -> Reply {
        self.call_as("coding-agent", session, tool, args).await
    }

    async fn call_as(&self, c: &str, session: Option<u64>, tool: &str, args: Value) -> Reply {
        let call = self.call_of(c, session, tool, args);
        Backend::call(self.engine.as_ref(), call).await
    }

    /// The call in a task of its own.
    fn spawn(&self, c: &str, session: Option<u64>, tool: &str, args: Value) -> Task<Reply> {
        let call = self.call_of(c, session, tool, args);
        let e = Arc::clone(&self.engine);
        tokio::spawn(async move { Backend::call(e.as_ref(), call).await })
    }

    /// Opens a session of `c` with `session hello`; returns its id.
    async fn hello_as(&self, c: &str) -> u64 {
        let r = self
            .call_as(c, None, "session", json!({"action": "hello"}))
            .await;
        assert!(!r.is_error, "{}", r.text);
        r.session.unwrap().0
    }

    async fn hello(&self) -> u64 {
        self.hello_as("coding-agent").await
    }

    fn conn(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.db()).unwrap()
    }

    fn events(&self) -> u64 {
        crate::store::history::last_seq(&self.conn()).unwrap()
    }

    fn txs(&self) -> i64 {
        self.conn()
            .query_row("SELECT count(*) FROM txs", [], |r| r.get(0))
            .unwrap()
    }

    fn state(&self) -> forum_core::State {
        crate::store::load::load_state(&self.conn()).unwrap()
    }

    fn has(&self, path: &str) -> bool {
        let p = crate::decode::values::path(path).unwrap();
        forum_core::query::resolve(&self.state(), &forum_core::NodeRef::Path(p)).is_some()
    }
}

/// A call of `class_name` on `engine` at `now`, decoded as the handler decodes it.
fn call_on(
    engine: &Engine,
    now: Time,
    class_name: &str,
    session: Option<u64>,
    tool: &str,
    args: Value,
) -> Call {
    let Value::Object(args) = args else {
        panic!("an object")
    };
    let env = DecodeEnv {
        now,
        text_max: engine.text_max(),
        coverage_window: engine.coverage_window(),
        entries: engine.entries(),
    };
    let prepared = prepare(tool, args.clone(), &env)
        .unwrap_or_else(|e| panic!("{tool} {args:?}: {}", e.render()));
    Call {
        caller: Caller {
            class: class(class_name),
            caps: None,
        },
        mcp_session: None,
        session: session.map_or(SessionCtx::New, |s| SessionCtx::Existing(SessionId(s))),
        prepared,
    }
}

fn body(r: &Reply) -> &Value {
    r.structured.as_ref().unwrap()
}

/// Waits until `f` holds, at most 10 s.
async fn until(what: &str, f: impl Fn() -> bool) {
    for _ in 0..2_000 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {what}");
}

fn create(path: &str) -> Value {
    json!({"action": "create", "path": path, "title": format!("work at {path}")})
}

// ------------------------------------------------------------------------------------------------
// Group commit (D.11.2, CS-13, CS-14)

#[tokio::test(flavor = "multi_thread")]
async fn a_group_commits_once_and_replies_only_after_its_commit() {
    let gate = Gate::new(false);
    let groups = Arc::new(Mutex::new(Vec::new()));
    let replied = Arc::new(AtomicUsize::new(0));
    let (g2, r2) = (Arc::clone(&groups), Arc::clone(&replied));
    let hooks = Hooks {
        gate: Some(Arc::clone(&gate)),
        before_commit: Some(Arc::new(move |n| {
            g2.lock().unwrap().push((n, r2.load(Ordering::SeqCst)));
        })),
        ..Hooks::default()
    };
    let h = H::start("group", Params::default(), hooks);
    let s = h.hello().await;
    groups.lock().unwrap().clear();
    // Five writes queue while the writer waits; they run as one group.
    gate.set(true);
    let tasks: Vec<_> = (0..5)
        .map(|i| h.spawn("coding-agent", Some(s), "node", create(&format!("/g/n{i}"))))
        .collect();
    until("five queued jobs", || h.engine.queued() == 5).await;
    let commits = h.engine.stats().commits.load(Ordering::SeqCst);
    let txs = h.txs();
    gate.set(false);
    for t in tasks {
        let r = t.await.unwrap();
        assert!(!r.is_error, "{}", r.text);
        replied.fetch_add(1, Ordering::SeqCst);
    }
    assert_eq!(
        *groups.lock().unwrap(),
        vec![(5, 0)],
        "one group of five, and no reply before its COMMIT"
    );
    assert_eq!(h.engine.stats().commits.load(Ordering::SeqCst), commits + 1);
    assert!(h.txs() > txs);
    // Three reads queue: their group writes nothing and commits nothing, and no job writes a
    // `txs` row (CS-14).
    groups.lock().unwrap().clear();
    let commits = h.engine.stats().commits.load(Ordering::SeqCst);
    let txs = h.txs();
    let events = h.events();
    gate.set(true);
    let tasks: Vec<_> = (0..3)
        .map(|_| {
            h.spawn(
                "coding-agent",
                Some(s),
                "view",
                json!({"action": "constitution"}),
            )
        })
        .collect();
    until("three queued reads", || h.engine.queued() == 3).await;
    gate.set(false);
    for t in tasks {
        assert!(!t.await.unwrap().is_error);
    }
    assert_eq!(groups.lock().unwrap().len(), 1);
    assert_eq!(h.engine.stats().commits.load(Ordering::SeqCst), commits);
    assert_eq!(h.txs(), txs);
    assert_eq!(h.events(), events);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crash_inside_a_group_leaves_the_state_before_the_group_and_retries_run_once() {
    let gate = Gate::new(false);
    let hooks = Hooks {
        gate: Some(Arc::clone(&gate)),
        crash_after_jobs: Some(2),
        ..Hooks::default()
    };
    let h = H::start("crash-group", Params::default(), hooks);
    let s = h.hello().await;
    let before = h.state();
    let events = h.events();
    gate.set(true);
    let args = |i: usize| json!({"action": "create", "path": format!("/c/n{i}"), "title": "t", "op_id": format!("op-{i}")});
    let tasks: Vec<_> = (0..3)
        .map(|i| h.spawn("coding-agent", Some(s), "node", args(i)))
        .collect();
    until("three queued jobs", || h.engine.queued() == 3).await;
    gate.set(false);
    for t in tasks {
        let r = t.await.unwrap();
        assert!(
            r.text.starts_with("error: unavailable"),
            "no reply of the group was sent: {}",
            r.text
        );
    }
    assert!(!h.engine.alive(), "the writer ended");
    // Restart: the state tables hold the state before the group.
    let h = h.restart(Params::default(), Hooks::default());
    assert_eq!(h.events(), events);
    assert_eq!(h.state(), before);
    // Each retry executes once; a second retry is answered by the stored response.
    for i in 0..3 {
        let r = h.call(Some(s), "node", args(i)).await;
        assert!(!r.is_error, "{}", r.text);
        let again = h.call(Some(s), "node", args(i)).await;
        assert_eq!(again.text, r.text);
        assert_eq!(again.structured, r.structured);
    }
    let st = h.state();
    let n = st.nodes.values().filter(|x| x.title.0 == "t").count();
    assert_eq!(n, 3, "each node once");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crash_between_commit_and_reply_is_answered_by_the_stored_bytes() {
    let drop_replies = Arc::new(AtomicBool::new(false));
    let hooks = Hooks {
        drop_replies_after_commit: Arc::clone(&drop_replies),
        ..Hooks::default()
    };
    let h = H::start("crash-reply", Params::default(), hooks);
    let s = h.hello().await;
    let other = h.hello().await;
    let args = json!({"action": "create", "path": "/r/a", "title": "t", "op_id": "op-r"});
    drop_replies.store(true, Ordering::SeqCst);
    let lost = h.call(Some(s), "node", args.clone()).await;
    assert!(lost.text.starts_with("error: unavailable"));
    assert!(h.has("/r/a"), "the group committed");
    let events = h.events();
    let first = h.call(Some(s), "node", args.clone()).await;
    let second = h.call(None, "node", args.clone()).await;
    assert!(!first.is_error, "{}", first.text);
    assert!(first.text.starts_with("[swarm]"));
    assert!(first.text.contains("next_calls"), "the stored next calls");
    assert_eq!(
        first.text, second.text,
        "byte-identical, with no session too"
    );
    assert_eq!(first.structured, second.structured);
    assert_eq!(first.session, Some(SessionId(s)));
    assert_eq!(h.events(), events, "a repeat appends nothing");
    // The stored row is what the retries returned.
    let stored = crate::store::state::stored_op(&h.conn(), "coding-agent", "op-r")
        .unwrap()
        .unwrap();
    let v: Value = serde_json::from_slice(&stored.response).unwrap();
    assert_eq!(v["text"].as_str(), Some(first.text.as_str()));
    // Other arguments, or another session, conflict.
    let r = h
        .call(
            Some(s),
            "node",
            json!({"action": "create", "path": "/r/b", "title": "t", "op_id": "op-r"}),
        )
        .await;
    assert!(r.text.contains("error: op_id_conflict"), "{}", r.text);
    let r = h.call(Some(other), "node", args).await;
    assert!(r.text.contains("error: op_id_conflict"), "{}", r.text);
}

/// A post-settle that runs past one batch keeps every batch in the job's transaction until its
/// `st_ops` row is written: a crash after it leaves both the events and the stored response, or
/// neither, and the retry gets the stored response (O1, D.8.1; review E4-spec-1).
#[tokio::test(flavor = "multi_thread")]
async fn a_post_settle_commits_the_job_s_events_only_with_its_stored_response() {
    let params = Params {
        settle_batch: 1,
        presence_silence: Dur(9 * 60_000),
        ..Params::default()
    };
    let h = H::start("post-settle", params.clone(), Hooks::default());
    let a = h.hello().await;
    let b = h.hello().await;
    let c = h.hello().await;
    let r = h
        .call(
            Some(a),
            "node",
            json!({"action": "create", "path": "/n", "title": "t", "capacity": 2}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    let r = h
        .call(Some(b), "hold", json!({"action": "take", "target": "/n"}))
        .await;
    assert!(!r.is_error, "{}", r.text);
    // At 12 minutes the close ends both holds, so A's and B's Silence keys (9 minutes) fall due
    // at `now`: the post-settle fires them in two batches.
    let h = h.restart(
        params.clone(),
        Hooks {
            crash_after_jobs: Some(1),
            ..Hooks::default()
        },
    );
    h.clock.set(T0.plus(Dur(12 * 60_000)));
    let close = json!({"action": "close", "path": "/n", "outcome": "done", "summary": "s",
                       "op_id": "op-close"});
    let r = h.call(Some(c), "node", close.clone()).await;
    assert!(r.text.starts_with("error: unavailable"), "{}", r.text);
    let h = h.restart(params, Hooks::default());
    let txs: i64 = h
        .conn()
        .query_row(
            "SELECT count(*) FROM txs WHERE op_id = 'op-close'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let stored = crate::store::state::stored_op(&h.conn(), "coding-agent", "op-close").unwrap();
    assert_eq!(
        txs > 0,
        stored.is_some(),
        "the job's events and its st_ops row are committed together"
    );
    let events = h.events();
    let r = h.call(Some(c), "node", close.clone()).await;
    assert!(
        !r.is_error,
        "the retry gets the stored response or runs once: {}",
        r.text
    );
    let again = h.call(Some(c), "node", close).await;
    assert_eq!(again.text, r.text, "a second retry is the stored response");
    if stored.is_some() {
        assert_eq!(h.events(), events, "a repeat appends nothing");
    }
}

/// A session whose own call revives it from silence keeps that call's entry in its write window,
/// however the call's pre-settle fired its Silence (D.11.4, D.11.15; review E4-spec-3).
#[tokio::test(flavor = "multi_thread")]
async fn a_silent_session_that_writes_again_keeps_its_new_window_entries() {
    let params = Params {
        write_per_hour: 2,
        ..Params::default()
    };
    let h = H::start("revive", params, Hooks::default());
    let s = h.hello().await;
    let free = |p: &str| json!({"action": "create", "path": p, "title": "t", "take": false});
    assert!(!h.call(Some(s), "node", free("/v/1")).await.is_error);
    // presence.silence is 15 minutes: #2's pre-settle fires the Silence, which drops the window
    // of #1; #2 and #3 then fill the hour.
    h.clock.set(T0.plus(Dur(16 * 60_000)));
    let r = h.call(Some(s), "node", free("/v/2")).await;
    assert!(!r.is_error, "{}", r.text);
    let r = h.call(Some(s), "node", free("/v/3")).await;
    assert!(!r.is_error, "{}", r.text);
    let r = h.call(Some(s), "node", free("/v/4")).await;
    assert!(
        r.text.contains("error: limit\nhint: write.per_hour (2)"),
        "the hour holds #2 and #3: {}",
        r.text
    );
}

/// A panic on the writer thread ends it as a fatal error does: it is marked stopped, its queue is
/// drained, and later calls hear that the server stopped (crash-only; review E4-robust-3).
#[tokio::test(flavor = "multi_thread")]
async fn a_panic_on_the_writer_stops_it_and_later_calls_hear_so() {
    let armed = Arc::new(AtomicBool::new(false));
    let a2 = Arc::clone(&armed);
    let hooks = Hooks {
        before_commit: Some(Arc::new(move |_| {
            assert!(
                !a2.swap(false, Ordering::SeqCst),
                "a test hook panicked inside a group"
            );
        })),
        ..Hooks::default()
    };
    let h = H::start("panic", Params::default(), hooks);
    let s = h.hello().await;
    armed.store(true, Ordering::SeqCst);
    let r = h.call(Some(s), "node", create("/p/a")).await;
    assert!(r.text.starts_with("error: unavailable"), "{}", r.text);
    until("the writer marked stopped", || !h.engine.alive()).await;
    let r = tokio::time::timeout(
        Duration::from_secs(3),
        h.call(Some(s), "node", create("/p/b")),
    )
    .await
    .expect("answered at once");
    assert!(r.text.starts_with("error: unavailable"), "{}", r.text);
    assert_eq!(h.engine.queued(), 0);
}

/// `talk wait`'s deadline is set when its handler starts: time spent queued counts against
/// `min(timeout, talk.wait_max)` (D.11.11; review E4-spec-10).
#[tokio::test(flavor = "multi_thread")]
async fn a_wait_s_deadline_counts_from_its_handler_s_start() {
    let gate = Gate::new(false);
    let h = H::start(
        "wait-deadline",
        Params::default(),
        Hooks {
            gate: Some(Arc::clone(&gate)),
            ..Hooks::default()
        },
    );
    let s1 = h.hello().await;
    let s2 = h.hello().await;
    assert!(!h.call(Some(s1), "node", create("/d")).await.is_error);
    assert!(
        !h.call(Some(s2), "view", json!({"action": "node", "path": "/d"}))
            .await
            .is_error
    );
    gate.set(true);
    let start = tokio::time::Instant::now();
    let w = h.spawn(
        "coding-agent",
        Some(s2),
        "talk",
        json!({"action": "wait", "timeout": "1s", "for": "/d"}),
    );
    until("the readiness check queued", || h.engine.queued() == 1).await;
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    gate.set(false);
    // Only the wait's timing counts here, not what it reports.
    tokio::time::timeout(Duration::from_secs(10), w)
        .await
        .unwrap()
        .unwrap();
    let took = start.elapsed();
    assert!(
        took < Duration::from_millis(2_300),
        "its second was spent in the queue: {took:?}"
    );
}

/// A `talk wait` whose readiness check runs after the stop began does not park: the drain does
/// not wait out `talk.wait_max` (D.11.8; review E4-robust-10).
#[tokio::test(flavor = "multi_thread")]
async fn a_wait_queued_when_the_stop_begins_answers_at_once() {
    let gate = Gate::new(false);
    let h = H::start(
        "wait-drain",
        Params::default(),
        Hooks {
            gate: Some(Arc::clone(&gate)),
            ..Hooks::default()
        },
    );
    let s1 = h.hello().await;
    let s2 = h.hello().await;
    assert!(!h.call(Some(s1), "node", create("/dr")).await.is_error);
    assert!(
        !h.call(Some(s2), "view", json!({"action": "node", "path": "/dr"}))
            .await
            .is_error
    );
    gate.set(true);
    let w = h.spawn("coding-agent", Some(s2), "talk", wait_for("/dr"));
    until("the readiness check queued", || h.engine.queued() == 1).await;
    h.engine.wake_all();
    gate.set(false);
    tokio::time::timeout(Duration::from_secs(5), w)
        .await
        .expect("answered well before talk.wait_max")
        .unwrap();
}

// ------------------------------------------------------------------------------------------------
// Queues (D.11.2, CS-29)

#[tokio::test(flavor = "multi_thread")]
async fn a_gone_caller_is_skipped_and_a_class_past_queue_max_wait_is_busy_alone() {
    let gate = Gate::new(false);
    let hooks = Hooks {
        gate: Some(Arc::clone(&gate)),
        ..Hooks::default()
    };
    let h = H::start("queues", Params::default(), hooks);
    let s = h.hello().await;
    let ide = h.hello_as("ide-agent").await;
    gate.set(true);
    // A caller that goes away while its job is queued.
    let gone = h.spawn("coding-agent", Some(s), "node", create("/q/gone"));
    until("one queued job", || h.engine.queued() == 1).await;
    gone.abort();
    let _ = gone.await;
    let kept = h.spawn("coding-agent", Some(s), "node", create("/q/kept"));
    until("two queued jobs", || h.engine.queued() == 2).await;
    // The class's oldest job has waited past queue.max_wait (10 s): its next call is busy, with
    // its status line, rendered in its refusals lane.
    h.clock.advance(Dur(11 * SECOND.0));
    let busy = h.spawn("coding-agent", Some(s), "node", create("/q/busy"));
    until("the refusal queued", || h.engine.queued() == 3).await;
    // Another class is not busy.
    let other = h.spawn("ide-agent", Some(ide), "node", create("/q/other"));
    until("four queued jobs", || h.engine.queued() == 4).await;
    // A call that names no session gets no status line.
    let fresh = h.call(None, "node", create("/q/fresh")).await;
    assert!(
        fresh
            .text
            .starts_with("error: limit\nhint: the server is busy"),
        "{}",
        fresh.text
    );
    gate.set(false);
    let busy = busy.await.unwrap();
    assert!(busy.is_error);
    assert!(busy.text.starts_with("[swarm] running"), "{}", busy.text);
    assert!(
        busy.text
            .contains("error: limit\nhint: the server is busy; nothing was applied")
    );
    assert!(
        !busy.text.contains("next_calls"),
        "the shell's own answers carry none"
    );
    assert!(!kept.await.unwrap().is_error);
    assert!(!other.await.unwrap().is_error);
    assert_eq!(h.engine.stats().skipped.load(Ordering::SeqCst), 1);
    assert!(h.engine.stats().busy.load(Ordering::SeqCst) >= 2);
    assert!(h.has("/q/kept") && h.has("/q/other"));
    assert!(!h.has("/q/gone"), "a skipped job applies nothing");
    assert!(!h.has("/q/busy") && !h.has("/q/fresh"));
}

// ------------------------------------------------------------------------------------------------
// The write limit (D.11.15, §21's precedence, A-105)

#[tokio::test(flavor = "multi_thread")]
async fn write_per_hour_comes_after_o1_the_session_and_the_target() {
    let params = Params {
        write_per_hour: 2,
        ..Params::default()
    };
    let h = H::start("per-hour", params, Hooks::default());
    let s = h.hello().await;
    let first = h
        .call(
            Some(s),
            "node",
            json!({"action": "create", "path": "/w/a", "title": "t", "op_id": "op-a"}),
        )
        .await;
    assert!(!first.is_error, "{}", first.text);
    assert!(!h.call(Some(s), "node", create("/w/b")).await.is_error);
    let events = h.events();
    let r = h.call(Some(s), "node", create("/w/c")).await;
    assert!(r.text.starts_with("[swarm] running"), "{}", r.text);
    assert!(
        r.text.contains("error: limit\nhint: write.per_hour (2)"),
        "{}",
        r.text
    );
    assert!(!r.text.contains("next_calls"));
    assert_eq!(h.events(), events, "nothing logged");
    // An op_id repeat gets its stored answer.
    let again = h
        .call(
            Some(s),
            "node",
            json!({"action": "create", "path": "/w/a", "title": "t", "op_id": "op-a"}),
        )
        .await;
    assert_eq!(again.text, first.text);
    // A missing target is not_found, and a retired or unknown session too.
    let r = h
        .call(
            Some(s),
            "node",
            json!({"action": "close", "path": "/nope", "outcome": "done", "summary": "s"}),
        )
        .await;
    assert!(r.text.contains("error: not_found"), "{}", r.text);
    let r = h.call(Some(99), "node", create("/w/d")).await;
    assert!(r.text.starts_with("error: not_found"), "{}", r.text);
    // Reads are not limited; an hour later writes go through again.
    assert!(
        !h.call(Some(s), "view", json!({"action": "constitution"}))
            .await
            .is_error
    );
    h.clock.advance(Dur(3_600_000));
    assert!(!h.call(Some(s), "node", create("/w/e")).await.is_error);
}

/// §21's precedence for the two cases D.12.1's row (A-105) names: over its limit, a write with a
/// stale token gets `limit(WritesPerHour)` and no `late` entry (the rate limit before the stale
/// token), and a retired session gets `not_found` (the session first; review E4-spec-15).
#[tokio::test(flavor = "multi_thread")]
async fn write_per_hour_comes_before_a_stale_token_and_after_a_retired_session() {
    let minutes = |m: u64| Dur(m * 60_000);
    let params = Params {
        write_per_hour: 2,
        lease_first: minutes(5),
        lease_default: minutes(5),
        lease_max: minutes(10),
        lease_ceiling: minutes(20),
        presence_silence: minutes(10),
        presence_resolution: minutes(3),
        session_retain: minutes(30),
        ..Params::default()
    };
    let h = H::start("per-hour-stale", params, Hooks::default());
    let s = h.hello().await;
    let r = h.call(Some(s), "node", create("/w/a")).await;
    assert!(!r.is_error, "{}", r.text);
    let token = body(&r)["token"].as_str().unwrap().to_string();
    let r = h
        .call(
            Some(s),
            "hold",
            json!({"action": "release", "token": token, "done": "d", "next_step": "n",
                   "reason": "handoff"}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    // Over the limit, with the token of the hold just released.
    let events = h.events();
    let r = h
        .call(
            Some(s),
            "hold",
            json!({"action": "checkpoint", "token": token, "done": "d", "next_step": "n"}),
        )
        .await;
    assert!(
        r.text.contains("error: limit\nhint: write.per_hour (2)"),
        "{}",
        r.text
    );
    assert_eq!(h.events(), events, "no late entry, nothing logged");
    // Past session.retain the session is retired: not_found, not the limit.
    h.clock.set(T0.plus(minutes(31)));
    let r = h.call(Some(s), "node", create("/w/b")).await;
    assert!(r.text.starts_with("error: not_found"), "{}", r.text);
    assert!(!r.text.contains("write.per_hour"));
}

/// 1.5 (§20.1, §21, R3): a failed first call opens its session, and its response says so with
/// `session_opened`, and that nothing else was applied; its structured content keeps §20.1's order,
/// `hello` (the session first) after the next calls, and so does the stored answer of its `op_id`,
/// which a repeat gets back byte for byte.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_first_call_says_so_and_its_stored_answer_keeps_the_order() {
    let h = H::start("failed-first", Params::default(), Hooks::default());
    let args = json!({"action": "close", "path": "/nope", "outcome": "done", "summary": "s",
                      "op_id": "op-first"});
    let r = h.call(None, "node", args.clone()).await;
    assert!(r.is_error, "{}", r.text);
    let s = r.session.expect("the call opened its session");
    let v = body(&r);
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        ["status", "error", "hint", "warnings", "next_calls", "hello"],
        "{v}"
    );
    assert_eq!(v["error"], json!("not_found"));
    assert_eq!(
        v["warnings"][0]["detail"],
        json!(format!(
            "this call failed, but it opened session {0}; nothing else in the call was applied; \
             pass session: {0} on every call",
            s.render()
        ))
    );
    let hello: Vec<&str> = v["hello"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(hello.first(), Some(&"session"));
    assert_eq!(v["hello"]["session"], json!(s.render()));
    assert!(
        r.text
            .lines()
            .any(|l| l.starts_with("warnings: [{\"code\":\"session_opened\"")),
        "{}",
        r.text
    );
    // The repeat: the stored answer, in the same order.
    let again = h.call(None, "node", args).await;
    assert_eq!(again.text, r.text);
    assert_eq!(
        serde_json::to_string(body(&again)).unwrap(),
        serde_json::to_string(v).unwrap()
    );
}

/// 1.10 (§20.1, §20.9 rule E, §21): a worker's first `hello` refused for a `from_user` that names
/// no principal opens its session with the call's intent, caps, and horizon, says so, and that only
/// the `from_user` was refused, and gets a `hello`'s next calls: `end` while nothing is offered,
/// and `hold next` once an offer waits. In the lab's fifth round (it4), all six such refusals, of
/// workers that logged their scheduler's prompt, said that nothing else in the call was applied,
/// and carried `next_calls: none`.
#[tokio::test(flavor = "multi_thread")]
async fn a_hello_refused_for_its_from_user_says_what_held_and_what_comes_next() {
    let h = H::start("refused-hello", Params::default(), Hooks::default());
    let scheduler = json!({"action": "hello", "intent": "pick up work", "caps": ["python"],
                           "horizon": 30,
                           "from_user": {"text": "The scheduler started you. Pick up work on the forum.",
                                         "client": "ide"}});
    let r = h.call(None, "session", scheduler.clone()).await;
    assert!(r.is_error, "{}", r.text);
    let s = r.session.expect("the call opened its session");
    let v = body(&r);
    assert_eq!(v["error"], json!("invalid"));
    assert_eq!(
        v["warnings"][0]["detail"],
        json!(format!(
            "this call opened session {0}, with its intent, caps, and horizon; only its from_user \
             was refused, and it was not logged: pass session: {0} on every call",
            s.render()
        ))
    );
    assert!(
        v["hello"]["caps"]
            .as_array()
            .unwrap()
            .contains(&json!("python")),
        "{v}"
    );
    assert!(
        r.text.ends_with(&format!(
            "next_calls: none — end: nothing is offered to you; unless you came for a message or \
             an event, end your run; your session needs no closing{}",
            // 1.12 (§20.9): a session that serves no principal is told how a thread's message is
            // recorded.
            crate::render::next_calls::END_THREAD
        )),
        "{}",
        r.text
    );
    // An offer that fits the horizon waits: the refused hello's next call is `hold next`.
    let lead = h.hello().await;
    let r = h
        .call(
            Some(lead),
            "node",
            json!({"action": "create", "path": "/w/task", "title": "a task", "tags": ["offer"],
                   "take": false, "est": 10}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    let r = h.call(None, "session", scheduler).await;
    assert!(r.is_error, "{}", r.text);
    let s = r.session.expect("the call opened its session");
    let items = body(&r)["next_calls"]["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 1, "{}", r.text);
    assert_eq!(items[0]["tool"], json!("hold"), "{}", r.text);
    assert_eq!(items[0]["args"]["action"], json!("next"), "{}", r.text);
    assert_eq!(items[0]["args"]["session"], json!(s.render()), "{}", r.text);
}

// ------------------------------------------------------------------------------------------------
// talk wait (D.11.11, CS-15)

fn wait_for(node: &str) -> Value {
    json!({"action": "wait", "timeout": "30s", "for": node})
}

#[tokio::test(flavor = "multi_thread")]
async fn a_parked_wait_wakes_only_on_a_commit_that_concerns_it() {
    let h = H::start("wake", Params::default(), Hooks::default());
    let s1 = h.hello().await;
    let s2 = h.hello().await;
    let r = h.call(Some(s1), "node", create("/hub")).await;
    let token = body(&r)["token"].as_str().unwrap().to_string();
    let w = h.spawn("coding-agent", Some(s2), "talk", wait_for("/hub"));
    until("the wait parked", || h.engine.parked() == 1).await;
    // A commit that does not concern it does not wake it.
    assert!(
        !h.call(Some(s1), "node", create("/elsewhere"))
            .await
            .is_error
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.engine.parked(), 1);
    assert!(!w.is_finished());
    // An entry on its node does.
    let r = h
        .call(
            Some(s1),
            "hold",
            json!({"action": "checkpoint", "token": token, "done": "d", "next_step": "n"}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    let done = tokio::time::timeout(Duration::from_secs(5), w).await;
    assert!(done.is_ok(), "woken well before its 30 s timeout");
    assert_eq!(h.engine.parked(), 0);
    // A session that is ready does not park: it has news it never saw.
    let s3 = h.hello().await;
    let ready = tokio::time::timeout(
        Duration::from_secs(5),
        h.call(Some(s3), "talk", wait_for("/hub")),
    )
    .await;
    assert!(ready.is_ok());
    // A timeout ends a wait: talk.wait_max bounds it.
    let params = h.engine.sh.knobs.talk_wait_max;
    assert_eq!(params, Dur(50_000));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_waiters_of_a_hot_node_are_woken_at_most_once_a_second() {
    let h = H::start("coalesce", Params::default(), Hooks::default());
    let s1 = h.hello().await;
    let s2 = h.hello().await;
    let r = h.call(Some(s1), "node", create("/hot")).await;
    let token = body(&r)["token"].as_str().unwrap().to_string();
    let checkpoint = json!({"action": "checkpoint", "token": token, "done": "d", "next_step": "n"});
    let view = json!({"action": "node", "path": "/hot"});
    // First wake-up: at once.
    assert!(!h.call(Some(s2), "view", view.clone()).await.is_error);
    let w = h.spawn("coding-agent", Some(s2), "talk", wait_for("/hot"));
    until("parked", || h.engine.parked() == 1).await;
    assert!(!h.call(Some(s1), "hold", checkpoint.clone()).await.is_error);
    tokio::time::timeout(Duration::from_secs(5), w)
        .await
        .expect("woken at once")
        .unwrap();
    // Within the same second, the next append's wake-up waits for the end of the second.
    assert!(!h.call(Some(s2), "view", view).await.is_error);
    let w = h.spawn("coding-agent", Some(s2), "talk", wait_for("/hot"));
    until("parked again", || h.engine.parked() == 1).await;
    assert!(!h.call(Some(s1), "hold", checkpoint).await.is_error);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(h.engine.parked(), 1, "coalesced: not woken yet");
    h.clock.advance(Dur(SECOND.0));
    h.engine.kick();
    tokio::time::timeout(Duration::from_secs(5), w)
        .await
        .expect("woken at the end of the second")
        .unwrap();
    assert_eq!(h.engine.parked(), 0);
}

// ------------------------------------------------------------------------------------------------
// The health cache (D.8.22, CS-9, CS-21)

#[tokio::test(flavor = "multi_thread")]
async fn the_health_cache_refreshes_after_a_commit_within_health_refresh_even_with_only_reads() {
    let h = H::start("health", Params::default(), Hooks::default());
    let computed = || h.engine.stats().health_computed.load(Ordering::SeqCst);
    let at = |r: &Reply| body(r)["computed_at"].as_str().unwrap().to_string();
    assert_eq!(computed(), 0, "computed at startup");
    h.clock.set(T0.plus(Dur(60_000)));
    let s = h.hello().await;
    // A commit on a cache older than HEALTH_REFRESH recomputes it, right after the replies.
    until("the refresh after the commit", || computed() == 1).await;
    // A write 5 s later marks the cache, which is fresh: no computation.
    h.clock.set(T0.plus(Dur(65_000)));
    assert!(!h.call(Some(s), "node", create("/h/a")).await.is_error);
    assert_eq!(computed(), 1);
    let r = h
        .call(Some(s), "control", json!({"action": "health"}))
        .await;
    assert_eq!(at(&r), "2026-10-01T09:01:00Z");
    // 1.7 (§20.1): green health is left out of the status line.
    assert!(
        r.text
            .starts_with("[swarm] running · now 2026-10-01T09:01:00Z · session ")
    );
    assert!(!r.text.lines().next().unwrap().contains("health"));
    // Only reads follow; the idle writer refreshes it at its time.
    h.clock.set(T0.plus(Dur(150_000)));
    h.engine.kick();
    until("the refresh", || computed() == 2).await;
    let r = h
        .call(Some(s), "control", json!({"action": "health"}))
        .await;
    assert_eq!(at(&r), "2026-10-01T09:02:00Z");
    // A herd of health calls costs no computation.
    for _ in 0..10 {
        assert!(
            !h.call(Some(s), "control", json!({"action": "health"}))
                .await
                .is_error
        );
    }
    assert_eq!(computed(), 2);
}

/// 1.7 (§17, §20.1): the writer remembers the health each session's last status line showed, so
/// a standing reason comes once per session and level, and again when the level changes; a
/// session's first response always shows it.
#[tokio::test(flavor = "multi_thread")]
async fn the_status_line_shows_a_standing_reason_once_per_session() {
    let h = H::start("health-once", Params::default(), Hooks::default());
    let computed = || h.engine.stats().health_computed.load(Ordering::SeqCst);
    let first = |r: &Reply| r.text.lines().next().unwrap().to_string();
    h.clock.set(T0.plus(Dur(60_000)));
    let s = h.hello().await;
    until("the refresh after the hello", || computed() == 1).await;
    // A pause of / is red health (§17); the idle writer refreshes the cache at its time.
    h.clock.set(T0.plus(Dur(61_000)));
    let pause = json!({"action": "pause", "scope": "/", "reason": "a release"});
    assert!(!h.call(Some(s), "control", pause.clone()).await.is_error);
    h.clock.set(T0.plus(Dur(75_000)));
    h.engine.kick();
    until("the refresh after the pause", || computed() == 2).await;
    let look = || h.call(Some(s), "view", json!({"action": "list"}));
    let a = first(&look().await);
    assert!(
        a.contains(" · health red: paused: / by coding-agent/s-"),
        "{a}"
    );
    let b = first(&look().await);
    assert!(b.ends_with(" · health red"), "{b}");
    // Another session is shown the reason at its first response.
    let t = h
        .call_as("coding-agent", None, "session", json!({"action": "hello"}))
        .await;
    assert!(first(&t).contains(" · health red: paused: /"), "{}", t.text);
    // Lifted, health is green and left out; set again, the same reason is shown again.
    let lift = json!({"action": "unpause", "scope": "/"});
    assert!(!h.call(Some(s), "control", lift).await.is_error);
    h.clock.set(T0.plus(Dur(90_000)));
    h.engine.kick();
    until("the refresh after the unpause", || computed() == 3).await;
    let c = first(&look().await);
    assert!(!c.contains("health"), "{c}");
    assert!(!h.call(Some(s), "control", pause).await.is_error);
    h.clock.set(T0.plus(Dur(105_000)));
    h.engine.kick();
    until("the refresh after the second pause", || computed() == 4).await;
    let d = first(&look().await);
    assert!(d.contains(" · health red: paused: /"), "{d}");
}

#[test]
fn the_shell_s_surges_join_the_reasons_in_order_and_set_the_level() {
    use forum_core::health::{CapacityReport, HealthReason, Level};
    let h = Health {
        level: Level::Red,
        reasons: vec![
            HealthReason::Underused { idle_permille: 600 },
            HealthReason::Overloaded {
                level: Level::Red,
                demand_min: 10,
                supply_min: 1,
                idle_permille: 0,
                oldest: None,
            },
        ],
        capacity: CapacityReport::default(),
    };
    let surge = HealthReason::WriteSurge {
        class: class("coding-agent"),
        last_hour: 5_000,
        baseline: 10,
    };
    let out = with_surges(h, vec![surge.clone()]);
    assert_eq!(out.level, Level::Red);
    assert!(
        matches!(out.reasons[0], HealthReason::Overloaded { .. }),
        "red first"
    );
    assert_eq!(out.reasons[1], surge);
    assert!(matches!(out.reasons[2], HealthReason::Underused { .. }));
    let green = Health {
        level: Level::Green,
        reasons: Vec::new(),
        capacity: CapacityReport::default(),
    };
    assert_eq!(with_surges(green, vec![surge]).level, Level::Yellow);
    let mut c = HealthCache::new(out, T0);
    assert_eq!(c.due(), None, "nothing committed");
    c.committed = true;
    assert_eq!(c.due(), Some(T0.plus(HEALTH_REFRESH)));
    assert!(!c.refresh(T0.plus(Dur(9_999)), || panic!("not due")));
}

// ------------------------------------------------------------------------------------------------
// Tokens (D.11.3, FQ-5)

#[tokio::test(flavor = "multi_thread")]
async fn a_token_reload_swaps_the_map_at_a_job_boundary() {
    // The commit hook holds the writer inside a group while the reload arrives.
    let hold = Arc::new(AtomicBool::new(false));
    let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let (entered_tx, release_rx) = (Mutex::new(entered_tx), Mutex::new(release_rx));
    let held = Arc::clone(&hold);
    let hooks = Hooks {
        before_commit: Some(Arc::new(move |_| {
            if held.swap(false, Ordering::SeqCst) {
                entered_tx.lock().unwrap().send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
            }
        })),
        ..Hooks::default()
    };
    let h = H::start("tokens", Params::default(), hooks);
    let line = |t: &str| {
        (
            format!("FORUM_TOKEN_{}", t.to_uppercase()),
            format!("{};class=coding-agent", hex(&sha256(t.as_bytes()))),
        )
    };
    h.tokens
        .swap(TokenMap::load(vec![line("fsk_a"), line("fsk_b")], None).unwrap());
    let s = h.hello().await;
    hold.store(true, Ordering::SeqCst);
    let t = h.spawn("coding-agent", Some(s), "node", create("/t/a"));
    tokio::task::spawn_blocking(move || entered_rx.recv().unwrap())
        .await
        .unwrap();
    h.engine
        .reload_tokens(TokenMap::load(vec![line("fsk_a")], None).unwrap());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        h.tokens.current().lookup("fsk_b").is_some(),
        "not swapped while the writer is inside a job"
    );
    release_tx.send(()).unwrap();
    assert!(!t.await.unwrap().is_error);
    until("the swap", || h.tokens.current().lookup("fsk_b").is_none()).await;
    assert!(h.tokens.current().lookup("fsk_a").is_some());
}

// ------------------------------------------------------------------------------------------------
// Search (D.11.12, CS-19, CS-37)

#[tokio::test(flavor = "multi_thread")]
async fn search_finds_earlier_work_for_hello_create_and_find() {
    let h = H::start("search", Params::default(), Hooks::default());
    let s = h.hello().await;
    let r = h
        .call(
            Some(s),
            "node",
            json!({"action": "create", "path": "/work/compression", "title": "zstd compression layer",
                   "body": "level 3 is the default"}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    let r = h
        .call(
            Some(s),
            "node",
            json!({"action": "create", "path": "/kb/zstd", "title": "zstd is fast at level 3"}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    assert!(
        h.engine
            .search_caught_up(h.events(), Duration::from_secs(10))
    );
    let paths = |v: &Value| -> Vec<String> {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["path"].as_str().unwrap().to_string())
            .collect()
    };
    // hello: related outside kb/, knowledge inside it.
    let r = h
        .call(
            None,
            "session",
            json!({"action": "hello", "intent": "tune the zstd level"}),
        )
        .await;
    assert_eq!(paths(&body(&r)["related"]), ["/work/compression"]);
    assert_eq!(paths(&body(&r)["knowledge"]), ["/kb/zstd"]);
    // create: the earlier attempt is related.
    let r = h
        .call(
            Some(s),
            "node",
            json!({"action": "create", "path": "/work/again", "title": "zstd compression again"}),
        )
        .await;
    assert_eq!(paths(&body(&r)["related"]), ["/work/compression"]);
    // view find {text}: anywhere.
    assert!(
        h.engine
            .search_caught_up(h.events(), Duration::from_secs(10))
    );
    let r = h
        .call(Some(s), "view", json!({"action": "find", "text": "zstd"}))
        .await;
    let found = paths(&body(&r)["matches"]);
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(found.contains(&"/kb/zstd".to_string()));
}

/// 1.11 (§20.9 rule 5; it5 l6 F3): the writer gives the next calls the first row of the call's
/// `related`; 1.12 (it6 l4 F1, l2 F1): a `hello` whose human's message is about an open work node
/// gets the create by key, whose `why` names that node beside the route to none, where 1.11 routed
/// the entry there; G1 checks it with that row (debug builds).
#[tokio::test(flavor = "multi_thread")]
async fn a_hello_names_the_first_related_work_node_beside_the_create() {
    let h = H::start("related", Params::default(), Hooks::default());
    let s = h.hello().await;
    let r = h
        .call(
            Some(s),
            "node",
            json!({"action": "create", "path": "/incidents/inc-311",
                   "title": "INC-311: EU checkout returns 500s", "take": false}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    assert!(
        h.engine
            .search_caught_up(h.events(), Duration::from_secs(10))
    );
    let r = h
        .call(
            None,
            "session",
            json!({"action": "hello", "principal": "mail:noor@example.org",
                   "from_user": {"text": "I just got paged: EU checkout is throwing 500s",
                                 "client": "ide"}}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    let v = r.structured.clone().unwrap();
    let related = &v["related"]["items"];
    assert_eq!(related[0]["path"], "/incidents/inc-311", "{v}");
    let item = &v["next_calls"]["items"][0];
    assert_eq!(item["args"]["action"], "create", "{v}");
    assert_eq!(item["args"]["keys"][0], "user:e1", "{v}");
    let why = item["why"].as_str().unwrap();
    assert!(
        why.starts_with("if new work: create by key; if only #"),
        "{v}"
    );
    assert!(why.contains("'s work: route there"), "{v}");
}

/// 1.9 (D.11.12): a node is found by the words of its path, its keys, and its last close's
/// learnings, through the feeder; a move and a key added index it again; and a word no document
/// has is corrected, the response saying so.
#[tokio::test(flavor = "multi_thread")]
async fn search_reads_paths_keys_and_learnings_and_corrects_a_typo() {
    let h = H::start("search-columns", Params::default(), Hooks::default());
    let s = h.hello().await;
    let ok = |r: &Reply| assert!(!r.is_error, "{}", r.text);
    let r = h
        .call(
            Some(s),
            "node",
            json!({"action": "create", "path": "/tidepool/rss-crash",
                   "title": "Fix the crash on an empty NOAA feed item",
                   "keys": ["issue:tidepool-app#41"]}),
        )
        .await;
    ok(&r);
    let token = body(&r)["token"].as_str().unwrap().to_string();
    let r = h
        .call(
            Some(s),
            "node",
            json!({"action": "close", "path": "/tidepool/rss-crash", "outcome": "done",
                   "summary": "the parser skips such items", "token": token,
                   "learnings": "a missing pubDate and an empty one are different cases"}),
        )
        .await;
    ok(&r);
    ok(&h
        .call(
            Some(s),
            "node",
            json!({"action": "create", "path": "/web/photos", "title": "Resize station photos"}),
        )
        .await);
    ok(&h
        .call(
            Some(s),
            "node",
            json!({"action": "move", "path": "/web/photos", "to": "/tidepool/image-resizing"}),
        )
        .await);
    ok(&h
        .call(
            Some(s),
            "node",
            json!({"action": "key", "path": "/tidepool/image-resizing",
                   "add": "issue:tidepool-api#53"}),
        )
        .await);
    assert!(
        h.engine
            .search_caught_up(h.events(), Duration::from_secs(10))
    );
    let find = |text: &'static str| {
        let h = &h;
        async move {
            let r = h
                .call(Some(s), "view", json!({"action": "find", "text": text}))
                .await;
            body(&r)["matches"].clone()
        }
    };
    let paths = |v: &Value| -> Vec<String> {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["path"].as_str().unwrap().to_string())
            .collect()
    };
    // The path's words, the key's, and the learnings'.
    assert_eq!(paths(&find("RSS").await), ["/tidepool/rss-crash"]);
    assert_eq!(paths(&find("app#41").await), ["/tidepool/rss-crash"]);
    assert_eq!(paths(&find("pubdate").await), ["/tidepool/rss-crash"]);
    // The moved node by its new path's words, no longer by its old; and by the key added.
    assert_eq!(paths(&find("image").await), ["/tidepool/image-resizing"]);
    assert!(
        !paths(&find("web").await).contains(&"/tidepool/image-resizing".to_string()),
        "the old path's words are gone"
    );
    assert_eq!(paths(&find("api#53").await), ["/tidepool/image-resizing"]);
    // A typo, corrected to a word of the vocabulary, and said.
    let m = find("resizng").await;
    assert_eq!(paths(&m), ["/tidepool/image-resizing"]);
    assert_eq!(m["fuzzy"], "matched resizing for resizng");
    // Only view find corrects (lab, it4 l6): a hello's intent is prose, so its related never says
    // fuzzy; its words found as written still find the node, and a typo alone finds nothing.
    let hello = |intent: &'static str| {
        let h = &h;
        async move {
            let r = h
                .call(
                    None,
                    "session",
                    json!({"action": "hello", "intent": intent}),
                )
                .await;
            body(&r)["related"].clone()
        }
    };
    let related = hello("check the station photos resizng").await;
    assert_eq!(paths(&related), ["/tidepool/image-resizing"]);
    assert_eq!(related["fuzzy"], Value::Null, "{related}");
    assert_eq!(hello("resizng").await, Value::Null);
}

// ------------------------------------------------------------------------------------------------
// Export (D.11.15, A-78)

#[tokio::test(flavor = "multi_thread")]
async fn control_export_pages_the_history_as_json_lines() {
    let h = H::start("export", Params::default(), Hooks::default());
    let s = h.hello().await;
    assert!(!h.call(Some(s), "node", create("/e/a")).await.is_error);
    let total = h.events();
    let mut after: Option<u64> = None;
    let mut seen = 0_u64;
    loop {
        let mut args = json!({"action": "export", "limit": 2});
        if let Some(a) = after {
            args["after"] = json!(a);
        }
        let r = h.call(Some(s), "control", args).await;
        assert!(!r.is_error, "{}", r.text);
        let b = body(&r);
        let n = b["events"].as_u64().unwrap();
        let lines = b["lines"].as_array().unwrap();
        let text_lines: Vec<&str> = r.text.lines().filter(|l| l.starts_with("{\"")).collect();
        assert_eq!(
            text_lines.len(),
            lines.len() + 1,
            "the header and one line each"
        );
        if n == 0 {
            break;
        }
        assert!(
            lines[0].get("tx").is_some(),
            "each page names its transaction"
        );
        seen += n;
        after = b["next_after"].as_u64();
    }
    assert!(seen >= total, "{seen} {total}");
}

/// The entry ids of a `view feed` page of `/f`, from `since`.
async fn feed_since(h: &H, s: u64, since: Value) -> Vec<String> {
    let r = h
        .call(
            Some(s),
            "view",
            json!({"action": "feed", "at": "/f", "since": since}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    body(&r)["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["e"].as_str().unwrap().to_string())
        .collect()
}

/// 1.6 (§20.4, l6: three refusals in a row for `since: "e57"`): `view feed {since: eN}` lists the
/// node's entries recorded after entry eN, whichever node eN is on; the writer turns it into the
/// node's seq at that entry, as it does for `wait`'s `since`.
#[tokio::test(flavor = "multi_thread")]
async fn a_feed_since_an_entry_lists_the_entries_after_it() {
    let h = H::start("feed-since", Params::default(), Hooks::default());
    let s = h.hello().await;
    for at in ["/f", "/g"] {
        assert!(!h.call(Some(s), "node", create(at)).await.is_error);
    }
    let mut ids = Vec::new();
    for (at, text) in [
        ("/f", "one"),
        ("/g", "elsewhere"),
        ("/f", "two"),
        ("/f", "three"),
    ] {
        let r = h
            .call(
                Some(s),
                "talk",
                json!({"action": "say", "at": at, "text": text}),
            )
            .await;
        assert!(!r.is_error, "{}", r.text);
        ids.push(body(&r)["entry"].as_str().unwrap().to_string());
    }
    let all = feed_since(&h, s, json!(0)).await;
    assert!(all.ends_with(&[ids[0].clone(), ids[2].clone(), ids[3].clone()]));
    assert_eq!(
        feed_since(&h, s, json!(ids[0])).await,
        [ids[2].clone(), ids[3].clone()]
    );
    // An entry of another node: the entries recorded after it.
    assert_eq!(
        feed_since(&h, s, json!(ids[1])).await,
        [ids[2].clone(), ids[3].clone()]
    );
    assert!(feed_since(&h, s, json!(ids[3])).await.is_empty());
    // The same page as the seq that the entry has.
    let seq = all.iter().position(|e| *e == ids[2]).unwrap() + 1;
    assert_eq!(
        feed_since(&h, s, json!(seq)).await,
        feed_since(&h, s, json!(ids[2])).await
    );
}

// ------------------------------------------------------------------------------------------------
// Idle firing (CS-30, §17, T2)

#[tokio::test(flavor = "multi_thread")]
async fn idle_firing_runs_at_the_first_deadline_with_the_events_of_the_next_call() {
    let h = H::start("idle", Params::default(), Hooks::default());
    let s = h.hello().await;
    assert!(!h.call(Some(s), "node", create("/i/a")).await.is_error);
    let before = h.state();
    let last = h.events();
    let at = T0.plus(Dur(16 * 60_000));
    let expected = forum_core::settle_events(&before, at);
    assert!(!expected.is_empty(), "the lease ends at 15 minutes");
    h.clock.set(at);
    h.engine.kick();
    until("the idle settle", || {
        h.engine.stats().idle_settles.load(Ordering::SeqCst) > 0
    })
    .await;
    let fired: Vec<forum_core::Event> = crate::store::history::events_after(&h.conn(), last, 1_000)
        .unwrap()
        .into_iter()
        .map(|e| e.event)
        .collect();
    assert_eq!(
        fired, expected,
        "T2: the events the next call's pre-settle would fire"
    );
    // It contacted nobody and logged settle transactions only.
    let c = h.conn();
    let kinds: Vec<i64> = c
        .prepare(
            "SELECT DISTINCT kind FROM txs WHERE id > (SELECT max(tx) FROM events WHERE seq <= ?1)",
        )
        .unwrap()
        .query_map([i64::try_from(last).unwrap()], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(kinds, [crate::store::TxKind::Settle.code()]);
}

// ------------------------------------------------------------------------------------------------
// Startup (D.11.8)

#[tokio::test(flavor = "multi_thread")]
async fn startup_applies_changed_parameters_and_refuses_ones_that_break_usage() {
    let h = H::start("params", Params::default(), Hooks::default());
    let s = h.hello().await;
    assert!(!h.call(Some(s), "node", create("/p/a")).await.is_error);
    assert!(!h.call(Some(s), "node", create("/p/b")).await.is_error);
    let more = Params {
        write_per_hour: 50,
        ..Params::default()
    };
    let h = h.restart(more.clone(), Hooks::default());
    assert_eq!(h.state().params, more);
    let admin: i64 = h
        .conn()
        .query_row(
            "SELECT count(*) FROM txs WHERE kind = ?1",
            [crate::store::TxKind::Admin.code()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(admin, 1, "one ParamsChanged transaction");
    // A session holds two nodes: one hold per session would break H2's usage.
    let H {
        dir, engine, clock, ..
    } = h;
    engine.shutdown();
    drop(engine);
    let fewer = Params {
        hold_per_session: 1,
        ..more
    };
    let err = Engine::start(
        opts(&dir, &clock, fewer, Hooks::default()),
        SharedTokens::default(),
        Arc::new(Mutex::new(SessionBinding::new())),
    );
    assert!(matches!(err, Err(StartError::Params(_))));
}

/// The files of a directory, by name.
fn files(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// `forum-server check` reads the database read-only, in one snapshot, and copies nothing
/// (D.9.1; review E4-spec-14).
#[tokio::test(flavor = "multi_thread")]
async fn check_runs_the_full_check_read_only() {
    let h = H::start("check", Params::default(), Hooks::default());
    let s = h.hello().await;
    assert!(!h.call(Some(s), "node", create("/k/a")).await.is_error);
    let events = h.events();
    let before = files(&h.dir.0);
    let r = check(&h.db()).unwrap();
    assert_eq!(r.events, events);
    assert_eq!(r.sessions, 1);
    assert_eq!(files(&h.dir.0), before, "no copy, no other file");
    assert_eq!(h.events(), events, "the database is not written");
    let empty = h.dir.0.join("empty.db");
    let _ = rusqlite::Connection::open(&empty).unwrap();
    assert!(check(&empty).is_err());
}

/// A check finishes on a database that is written all the while: it reads one snapshot, where the
/// online backup of a copy started over at every write (review E4-robust-6).
#[tokio::test(flavor = "multi_thread")]
async fn check_finishes_on_a_database_being_written() {
    let h = H::start("check-busy", Params::default(), Hooks::default());
    let s = h.hello().await;
    for i in 0..40 {
        assert!(
            !h.call(Some(s), "node", create(&format!("/k/n{i}")))
                .await
                .is_error
        );
    }
    let db = h.db();
    let stop = Arc::new(AtomicBool::new(false));
    let (stop2, db2) = (Arc::clone(&stop), db.clone());
    let writes = std::thread::spawn(move || {
        let c = rusqlite::Connection::open(&db2).unwrap();
        c.busy_timeout(Duration::from_secs(5)).unwrap();
        let mut n = 0_u64;
        while !stop2.load(Ordering::SeqCst) {
            c.execute(
                "INSERT INTO meta (key, value) VALUES ('probe', ?1)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                [n.to_string()],
            )
            .unwrap();
            n += 1;
            std::thread::sleep(Duration::from_millis(2));
        }
        n
    });
    std::thread::sleep(Duration::from_millis(50));
    let checked = tokio::task::spawn_blocking(move || check(&db));
    let r = tokio::time::timeout(Duration::from_secs(20), checked)
        .await
        .expect("the check finishes while the database is written")
        .unwrap();
    stop.store(true, Ordering::SeqCst);
    let n = writes.join().unwrap();
    assert!(n > 0);
    assert_eq!(r.unwrap().sessions, 1);
}

/// `forum-server check` runs K2's shell part over the tables that outlive memory: the `origins`
/// table, and the `from_user` rows that version-0 `user:e<n>` keys and origins need (D.9.2;
/// review E4-spec-5).
#[test]
fn check_runs_k2_over_the_origins_and_entries_tables() {
    use crate::store::testkit::{TempDb, World, fresh_file, scenario, test_params};
    let db = TempDb::new("check-k2");
    let mut w = World::from_opened(fresh_file(&db, test_params()));
    scenario(&mut w);
    let r = check(&db.path);
    assert!(r.is_ok(), "{r:?}");
    let k2 = |r: Result<CheckReport, StartError>| match r {
        Err(StartError::Store(StoreError::Check(v))) => v.iter().any(|x| {
            x.inv == forum_core::Inv::K2 && matches!(x.detail, forum_core::ViolationDetail::Key(_))
        }),
        _ => false,
    };
    // An origin that left memory, of version 1, that its normaliser would not produce.
    w.conn
        .execute(
            "INSERT INTO origins (origin, nv, request) VALUES ('mail:Bob@Example.COM', 1, 999)",
            [],
        )
        .unwrap();
    assert!(k2(check(&db.path)), "a stored origin that is not normal");
    w.conn
        .execute("DELETE FROM origins WHERE request = 999", [])
        .unwrap();
    assert!(check(&db.path).is_ok());
    // A version-0 `user:e<n>` origin whose entry is not a `from_user`.
    let request: i64 = w
        .conn
        .query_row(
            "SELECT eid FROM entries WHERE type = 'request' ORDER BY eid LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    w.conn
        .execute(
            "INSERT INTO origins (origin, nv, request) VALUES (?1, 0, 998)",
            [format!("user:e{request}")],
        )
        .unwrap();
    assert!(k2(check(&db.path)), "no from_user row for the entry");
    w.conn
        .execute(
            "UPDATE entries SET type = 'from_user' WHERE eid = ?1",
            [request],
        )
        .unwrap();
    assert!(check(&db.path).is_ok(), "now it has one");
    // A version-0 origin past `next_eid`, and a stored version-0 key without its entry.
    w.conn
        .execute(
            "INSERT INTO origins (origin, nv, request) VALUES ('user:e999999', 0, 997)",
            [],
        )
        .unwrap();
    assert!(k2(check(&db.path)));
    w.conn
        .execute("DELETE FROM origins WHERE request = 997", [])
        .unwrap();
    let node: i64 = w
        .conn
        .query_row("SELECT node FROM st_keys LIMIT 1", [], |r| r.get(0))
        .unwrap();
    let other: i64 = w
        .conn
        .query_row(
            "SELECT max(eid) FROM entries WHERE type <> 'from_user'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    w.conn
        .execute(
            "INSERT INTO st_keys (key, node, nv, open) VALUES (?1, ?2, 0, 1)",
            rusqlite::params![format!("user:e{other}"), node],
        )
        .unwrap();
    let r = check(&db.path);
    assert!(
        matches!(&r, Err(StartError::Store(StoreError::Check(v)))
            if v.iter().any(|x| x.inv == forum_core::Inv::K2)),
        "a stored user key without its from_user row: {r:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn shell_maps_drop_a_session_at_its_silence() {
    let h = H::start("silence", Params::default(), Hooks::default());
    let s = h.hello().await;
    h.binding
        .lock()
        .unwrap()
        .bind(&class("coding-agent"), "mcp-1", SessionId(s));
    assert_eq!(h.binding.lock().unwrap().len(), 1);
    // presence.silence is 15 minutes: the idle writer fires the Silence deadline.
    h.clock.set(T0.plus(Dur(16 * 60_000)));
    h.engine.kick();
    until("the binding dropped", || {
        h.binding.lock().unwrap().is_empty()
    })
    .await;
    // A silent session that calls again is bound again by that call, however its Silence fired
    // (D.11.4): here by the call's own pre-settle, which drops the binding before the reply, so
    // the handler's bind after it stays (review E4-spec-3).
    let server = crate::mcp::server::ForumServer::new(
        Arc::clone(&h.engine),
        String::new(),
        Arc::clone(&h.binding),
    );
    let caller = Caller {
        class: class("coding-agent"),
        caps: None,
    };
    let args = |p: &str| {
        let Value::Object(m) = json!({"action": "create", "path": p, "title": "t", "take": false,
                                      "session": format!("s-{s}")})
        else {
            panic!("an object")
        };
        m
    };
    let r = server
        .handle(caller.clone(), Some("mcp-1".into()), "node", args("/s/a"))
        .await;
    assert_eq!(r.is_error, Some(false));
    assert_eq!(h.binding.lock().unwrap().len(), 1, "bound again");
    // Silent again; the reviving call's pre-settle fires the Silence this time.
    h.clock.set(T0.plus(Dur(40 * 60_000)));
    let r = server
        .handle(caller, Some("mcp-1".into()), "node", args("/s/b"))
        .await;
    assert_eq!(r.is_error, Some(false));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        h.binding.lock().unwrap().len(),
        1,
        "the reviving call's binding stays"
    );
}

/// Debug builds check every firing with `check_fire`, every completed settle with
/// `check_invariants`, and every response's next calls with `check_guide` (D.9.1; review
/// E4-spec-4). Release builds, the scale gates' among them, skip these checks.
#[cfg(debug_assertions)]
#[tokio::test(flavor = "multi_thread")]
async fn debug_builds_check_every_firing_settle_and_guide() {
    let h = H::start("checks", Params::default(), Hooks::default());
    let st = h.engine.stats();
    let s = h.hello().await;
    assert!(!h.call(Some(s), "node", create("/c/a")).await.is_error);
    assert!(st.guides_checked.load(Ordering::SeqCst) >= 2);
    assert!(
        st.settles_checked.load(Ordering::SeqCst) >= 4,
        "pre- and post-settles"
    );
    assert_eq!(
        st.fires_checked.load(Ordering::SeqCst),
        0,
        "nothing fired yet"
    );
    // The lease ends at 15 minutes: the idle settle fires it, each firing checked.
    h.clock.set(T0.plus(Dur(16 * 60_000)));
    h.engine.kick();
    until("the idle settle", || {
        st.idle_settles.load(Ordering::SeqCst) > 0
    })
    .await;
    assert!(
        st.fires_checked.load(Ordering::SeqCst) >= 2,
        "the lease and the silence"
    );
}

/// A wait whose readiness check and an append on its node run in one group is woken by that
/// group's commit (D.11.11; review E4-spec-16).
#[tokio::test(flavor = "multi_thread")]
async fn a_wait_parked_in_the_group_of_an_append_on_its_node_is_woken() {
    let gate = Gate::new(false);
    let h = H::start(
        "wake-group",
        Params::default(),
        Hooks {
            gate: Some(Arc::clone(&gate)),
            ..Hooks::default()
        },
    );
    let s1 = h.hello().await;
    let s2 = h.hello().await;
    let r = h.call(Some(s1), "node", create("/g")).await;
    let token = body(&r)["token"].as_str().unwrap().to_string();
    assert!(
        !h.call(Some(s2), "view", json!({"action": "node", "path": "/g"}))
            .await
            .is_error
    );
    gate.set(true);
    let w = h.spawn("coding-agent", Some(s2), "talk", wait_for("/g"));
    until("the readiness check queued", || h.engine.queued() == 1).await;
    let c = h.spawn(
        "coding-agent",
        Some(s1),
        "hold",
        json!({"action": "checkpoint", "token": token, "done": "d", "next_step": "n"}),
    );
    until("the append queued", || h.engine.queued() == 2).await;
    let groups = h.engine.stats().groups.load(Ordering::SeqCst);
    gate.set(false);
    assert!(!c.await.unwrap().is_error);
    tokio::time::timeout(Duration::from_secs(5), w)
        .await
        .expect("woken well before its 30 s timeout")
        .unwrap();
    assert!(h.engine.stats().wakes.load(Ordering::SeqCst) >= 1);
    assert!(h.engine.stats().groups.load(Ordering::SeqCst) > groups);
}

/// The writer feeds the `WriteSurge` counts with every write call but `op_id` repeats and calls
/// whose session check failed, and the reason reaches `control health` within `HEALTH_REFRESH`
/// (§17, D.11.2; review E4-spec-17).
#[tokio::test(flavor = "multi_thread")]
async fn the_writes_of_a_class_feed_write_surge_into_control_health() {
    let h = H::start(
        "surge",
        Params::default(),
        Hooks {
            write_surge_floor: Some(3),
            ..Hooks::default()
        },
    );
    let s = h.hello().await;
    let made = json!({"action": "create", "path": "/u/b", "title": "t", "op_id": "op-b"});
    assert!(!h.call(Some(s), "node", create("/u/a")).await.is_error);
    assert!(!h.call(Some(s), "node", made.clone()).await.is_error);
    // Neither a repeat nor a call whose session check failed counts.
    assert!(!h.call(Some(s), "node", made).await.is_error);
    assert!(h.call(Some(99), "node", create("/u/x")).await.is_error);
    // Nor do reads.
    assert!(
        !h.call(Some(s), "view", json!({"action": "constitution"}))
            .await
            .is_error
    );
    assert!(!h.call(Some(s), "node", create("/u/c")).await.is_error);
    // The class's write surges in `control health`, once the cache is refreshed.
    async fn surges(h: &H, s: u64) -> Vec<u64> {
        let computed = h.engine.stats().health_computed.load(Ordering::SeqCst);
        h.clock.advance(HEALTH_REFRESH);
        h.engine.kick();
        until("a refresh", || {
            h.engine.stats().health_computed.load(Ordering::SeqCst) > computed
        })
        .await;
        let r = h
            .call(Some(s), "control", json!({"action": "health"}))
            .await;
        body(&r)["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|x| x["class"].as_str() == Some("coding-agent"))
            .map(|x| x["count"].as_u64().unwrap())
            .collect()
    }
    assert!(
        surges(&h, s).await.is_empty(),
        "three writes are not over the floor"
    );
    assert!(!h.call(Some(s), "node", create("/u/d")).await.is_error);
    assert_eq!(surges(&h, s).await, [4]);
}

/// The search file names its forum: another forum's file, or one fed past the end of the
/// history, is rebuilt rather than reused (D.11.8 step 5; review E4-spec-6).
#[tokio::test(flavor = "multi_thread")]
async fn a_search_file_of_another_forum_is_rebuilt() {
    let dir = Dir::new("search-other");
    let clock = Arc::new(ManualClock::new(T0));
    // Forum B, with more history than A will have, and no search file yet.
    let start_b = |dir: &Dir, search: bool| {
        let mut o = opts(dir, &clock, Params::default(), Hooks::default());
        o.database = dir.0.join("other.db");
        if !search {
            o.search_database = None;
        }
        Engine::start(
            o,
            SharedTokens::default(),
            Arc::new(Mutex::new(SessionBinding::new())),
        )
        .unwrap()
    };
    let call = |e: &Arc<Engine>, session: Option<u64>, tool: &str, args: Value| {
        let c = call_on(e, clock.now(), "coding-agent", session, tool, args);
        let e = Arc::clone(e);
        async move { Backend::call(e.as_ref(), c).await }
    };
    let b = start_b(&dir, false);
    let t = call(&b, None, "session", json!({"action": "hello"}))
        .await
        .session
        .unwrap()
        .0;
    for i in 0..8 {
        let r = call(
            &b,
            Some(t),
            "node",
            json!({"action": "create", "path": format!("/b/m{i}"), "title": "unrelated work"}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
    }
    b.shutdown();
    drop(b);
    // Forum A, in the same directory, indexes four nodes about zstd in forum-search.db.
    let a = H::start_in(dir, Arc::clone(&clock), Params::default(), Hooks::default());
    let s = a.hello().await;
    for i in 0..4 {
        let r = a
            .call(
                Some(s),
                "node",
                json!({"action": "create", "path": format!("/a/z{i}"), "title": "zstd compression"}),
            )
            .await;
        assert!(!r.is_error, "{}", r.text);
    }
    assert!(
        a.engine
            .search_caught_up(a.events(), Duration::from_secs(10))
    );
    let H { dir, engine, .. } = a;
    engine.shutdown();
    drop(engine);
    // B again, now with A's search file: A's documents must not answer B's search.
    let b = start_b(&dir, true);
    let end = crate::store::history::last_seq(
        &rusqlite::Connection::open(dir.0.join("other.db")).unwrap(),
    )
    .unwrap();
    assert!(b.search_caught_up(end, Duration::from_secs(10)));
    let r = call(
        &b,
        Some(t),
        "view",
        json!({"action": "find", "text": "zstd"}),
    )
    .await;
    assert_eq!(
        body(&r)["matches"]["items"].as_array().unwrap().len(),
        0,
        "A's documents are not B's: {}",
        r.text
    );
    let r = call(
        &b,
        Some(t),
        "view",
        json!({"action": "find", "text": "unrelated"}),
    )
    .await;
    assert_eq!(body(&r)["matches"]["items"].as_array().unwrap().len(), 8);
    b.shutdown();
}

/// A search file that is not a database does not stop the server: it is moved aside and rebuilt
/// (§18, D.11.8 step 5; review E4-robust-9).
#[tokio::test(flavor = "multi_thread")]
async fn a_damaged_search_file_is_moved_aside_and_rebuilt() {
    let dir = Dir::new("search-damaged");
    let index = dir.0.join("forum-search.db");
    std::fs::write(&index, vec![0x5a_u8; 8 * 1024]).unwrap();
    let h = H::start_in(
        dir,
        Arc::new(ManualClock::new(T0)),
        Params::default(),
        Hooks::default(),
    );
    assert!(
        h.dir.0.join("forum-search.db.damaged").exists(),
        "kept aside"
    );
    let s = h.hello().await;
    let r = h
        .call(
            Some(s),
            "node",
            json!({"action": "create", "path": "/d/q", "title": "quaternion"}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    assert!(
        h.engine
            .search_caught_up(h.events(), Duration::from_secs(10))
    );
    let r = h
        .call(
            Some(s),
            "view",
            json!({"action": "find", "text": "quaternion"}),
        )
        .await;
    assert_eq!(body(&r)["matches"]["items"].as_array().unwrap().len(), 1);
}

/// One process writes a database (§24): a second engine on it does not start while the first
/// runs, and does once it stopped (review E4-robust-12).
#[tokio::test(flavor = "multi_thread")]
async fn a_second_engine_on_a_database_does_not_start() {
    let h = H::start("lock", Params::default(), Hooks::default());
    let second = Engine::start(
        opts(&h.dir, &h.clock, Params::default(), Hooks::default()),
        SharedTokens::default(),
        Arc::new(Mutex::new(SessionBinding::new())),
    );
    match second {
        Err(e @ StartError::Locked(_)) => {
            assert!(e.to_string().contains("forum.db.lock"), "{e}");
        }
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("a second writer started"),
    }
    let _ = h.restart(Params::default(), Hooks::default());
}

/// The age of a queued job is measured on a monotonic clock: a step of the wall clock neither
/// makes a class busy nor keeps it from being busy (CS-29; review E4-robust-13).
#[tokio::test(flavor = "multi_thread")]
async fn a_step_of_the_wall_clock_does_not_change_busy() {
    let gate = Gate::new(false);
    let h = H::start(
        "mono",
        Params::default(),
        Hooks {
            gate: Some(Arc::clone(&gate)),
            ..Hooks::default()
        },
    );
    let s = h.hello().await;
    gate.set(true);
    let first = h.spawn("coding-agent", Some(s), "node", create("/m/a"));
    until("one queued job", || h.engine.queued() == 1).await;
    // An hour's step forward is not an hour of waiting.
    h.clock.set(T0.plus(Dur(3_600_000)));
    let second = h.spawn("coding-agent", Some(s), "node", create("/m/b"));
    until("two queued jobs", || h.engine.queued() == 2).await;
    assert_eq!(h.engine.stats().busy.load(Ordering::SeqCst), 0);
    // A step back, then eleven seconds of waiting: busy.
    h.clock.set(T0);
    h.clock.advance(Dur(11 * SECOND.0));
    let r = h.call(None, "node", create("/m/c")).await;
    assert!(
        r.text.starts_with("error: limit\nhint: the server is busy"),
        "{}",
        r.text
    );
    gate.set(false);
    assert!(!first.await.unwrap().is_error);
    assert!(!second.await.unwrap().is_error);
}

/// A class's lanes and the queues as a whole are bounded: a call past the bound is busy before it
/// is queued, and a refusal that does not fit is answered without its status line (D.11.2;
/// review E4-spec-13).
#[tokio::test(flavor = "multi_thread")]
async fn the_queues_are_bounded() {
    let gate = Gate::new(false);
    let h = H::start(
        "bounded",
        Params::default(),
        Hooks {
            gate: Some(Arc::clone(&gate)),
            queue_max: Some((2, 3)),
            ..Hooks::default()
        },
    );
    let s = h.hello().await;
    let ide = h.hello_as("ide-agent").await;
    gate.set(true);
    let a = h.spawn("coding-agent", Some(s), "node", create("/b/a"));
    let b = h.spawn("coding-agent", Some(s), "node", create("/b/b"));
    until("two queued jobs", || h.engine.queued() == 2).await;
    // The class's calls lane is full: busy, with the status line from its refusals lane.
    let full = h.spawn("coding-agent", Some(s), "node", create("/b/c"));
    until("the refusal queued", || h.engine.queued() == 3).await;
    // The queues are full: another class's call is busy too, without a status line.
    let r = h
        .call_as("ide-agent", Some(ide), "node", create("/b/d"))
        .await;
    assert!(
        r.text.starts_with("error: limit\nhint: the server is busy"),
        "{}",
        r.text
    );
    gate.set(false);
    assert!(!a.await.unwrap().is_error);
    assert!(!b.await.unwrap().is_error);
    let full = full.await.unwrap();
    assert!(full.text.starts_with("[swarm] running"), "{}", full.text);
    assert!(full.text.contains("error: limit\nhint: the server is busy"));
    assert!(!h.has("/b/c") && !h.has("/b/d"));
    assert!(h.engine.stats().busy.load(Ordering::SeqCst) >= 2);
}

/// The writer waits out a write lock another connection holds past its busy timeout, and goes
/// on, where it used to end (review E4-robust-1).
#[tokio::test(flavor = "multi_thread")]
async fn the_writer_waits_out_a_locked_database() {
    let h = H::start(
        "locked",
        Params::default(),
        Hooks {
            writer_busy: Some(Duration::from_millis(100)),
            ..Hooks::default()
        },
    );
    let s = h.hello().await;
    let other = h.conn();
    other.execute_batch("BEGIN IMMEDIATE").unwrap();
    let t = h.spawn("coding-agent", Some(s), "node", create("/l/a"));
    until("the writer found the lock", || {
        h.engine.stats().busy_begins.load(Ordering::SeqCst) >= 2
    })
    .await;
    other.execute_batch("COMMIT").unwrap();
    let r = tokio::time::timeout(Duration::from_secs(10), t)
        .await
        .unwrap()
        .unwrap();
    assert!(!r.is_error, "{}", r.text);
    assert!(h.engine.alive());
    assert!(h.has("/l/a"));
}

// ------------------------------------------------------------------------------------------------
// End to end over MCP streamable HTTP

mod http {
    use super::*;
    use crate::mcp::server::{ForumServer, HttpSettings, router, test_http};
    use crate::mcp::tools::INSTRUCTIONS;
    use rmcp::ServiceExt;
    use rmcp::model::{CallToolRequestParams, CallToolResult, ClientConfig};
    use rmcp::service::RunningService;
    use rmcp::transport::StreamableHttpClientTransport;
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

    const TOKEN: &str = "fsk_end-to-end-test-token-0123456789abcdefghijk";

    type Client = RunningService<rmcp::RoleClient, ClientConfig>;

    async fn serve(h: &H) -> Client {
        h.tokens.swap(
            TokenMap::load(
                vec![(
                    "FORUM_TOKEN_E2E".into(),
                    format!("{};class=coding-agent", hex(&sha256(TOKEN.as_bytes()))),
                )],
                None,
            )
            .unwrap(),
        );
        let server = ForumServer::new(
            Arc::clone(&h.engine),
            INSTRUCTIONS.to_string(),
            Arc::clone(&h.binding),
        );
        let http = HttpSettings {
            allowed_hosts: vec!["localhost".into(), "127.0.0.1".into()],
            mcp_idle: Duration::from_secs(900),
        };
        let addr = test_http::serve(router(server, h.tokens.clone(), &http)).await;
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp"))
                .auth_header(TOKEN),
        );
        ClientConfig::default().serve(transport).await.unwrap()
    }

    async fn call(c: &Client, tool: &str, args: Value) -> CallToolResult {
        let Value::Object(args) = args else {
            panic!("an object")
        };
        c.call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(args))
            .await
            .unwrap()
    }

    fn text(r: &CallToolResult) -> String {
        r.content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.clone()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Every response the core computed: the status line first, `next_calls` last, in the text
    /// and in `structuredContent`.
    fn computed(r: &CallToolResult, session: &str) -> Value {
        let t = text(r);
        let first = t.lines().next().unwrap();
        assert!(
            first.starts_with("[swarm] running · now 2026-10-01T09:00:00Z · session ")
                && first.contains(&format!(" session {session} (coding-agent)"))
                && !first.contains("health"),
            "{t}"
        );
        // The block is last: its header, then only its numbered items (WP-D7's next calls).
        let header = t
            .lines()
            .position(|l| l.starts_with("next_calls"))
            .unwrap_or_else(|| panic!("no next_calls block: {t}"));
        assert!(
            t.lines().skip(header + 1).all(|l| l
                .split_once(". ")
                .is_some_and(|(n, _)| n.parse::<u32>().is_ok())),
            "{t}"
        );
        let v = r.structured_content.clone().unwrap();
        assert!(v.get("next_calls").is_some());
        // 1.5 (§20.1): the structured content carries the status line too.
        assert_eq!(v.get("status"), Some(&json!(first)), "{v}");
        v
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_client_works_a_node_over_streamable_http() {
        let h = H::start("e2e", Params::default(), Hooks::default());
        let c = serve(&h).await;
        let info = c.peer_info().unwrap();
        assert_eq!(info.instructions.as_deref(), Some(INSTRUCTIONS));
        // A call that does not decode and names no session: no status line.
        let r = call(&c, "hold", json!({"action": "take"})).await;
        assert!(text(&r).starts_with("error: invalid"), "{}", text(&r));
        // session hello opens s-1 and binds the MCP session to it.
        let r = call(
            &c,
            "session",
            json!({"action": "hello", "intent": "fix the summary gap"}),
        )
        .await;
        assert_eq!(r.is_error, Some(false));
        let v = computed(&r, "s-1");
        assert!(v.get("constitution").is_some() && v.get("offers").is_some());
        // node create takes a hold by default.
        let r = call(
            &c,
            "node",
            json!({"action": "create", "session": "s-1", "path": "/work/summary-gap",
                   "title": "Summary gap", "op_id": "e2e-create"}),
        )
        .await;
        assert_eq!(r.is_error, Some(false), "{}", text(&r));
        let v = computed(&r, "s-1");
        let token = v["token"].as_str().unwrap().to_string();
        assert!(text(&r).contains(
            "holds 1 (/work/summary-gap, 15:00 left, checkpoint by 2026-10-01T09:05:00Z)"
        ));
        // A refusal of the decoder for a bound session keeps the status line.
        let r = call(&c, "hold", json!({"action": "take"})).await;
        assert!(text(&r).starts_with("[swarm] running"), "{}", text(&r));
        assert!(text(&r).contains("error: invalid"));
        // hold checkpoint.
        let r = call(
            &c,
            "hold",
            json!({"action": "checkpoint", "session": "s-1", "token": token,
                   "done": "found the gap", "next_step": "close it"}),
        )
        .await;
        assert_eq!(r.is_error, Some(false), "{}", text(&r));
        computed(&r, "s-1");
        // node close.
        let r = call(
            &c,
            "node",
            json!({"action": "close", "session": "s-1", "path": "/work/summary-gap",
                   "outcome": "done", "summary": "closed the gap", "token": token}),
        )
        .await;
        assert_eq!(r.is_error, Some(false), "{}", text(&r));
        let v = computed(&r, "s-1");
        assert_eq!(v["closed"].as_str(), Some("/work/summary-gap"));
        assert!(text(&r).contains("holds 0"));
        // The retry of the create is the stored response.
        let r = call(
            &c,
            "node",
            json!({"action": "create", "session": "s-1", "path": "/work/summary-gap",
                   "title": "Summary gap", "op_id": "e2e-create"}),
        )
        .await;
        assert!(
            text(&r).contains("checkpoint by 2026-10-01T09:05:00Z"),
            "the stored bytes"
        );
        c.cancel().await.unwrap();
    }

    /// A call whose client cancels it while it is queued is skipped by the writer, and nothing is
    /// applied: rmcp turns `notifications/cancelled` into the request's token, the handler drops
    /// the call, and with it the job's reply channel (§19, D.11.2; review E4-robust-5).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_queued_call_its_client_cancels_is_skipped() {
        use rmcp::model::{CallToolRequest, ClientRequest};
        use rmcp::service::{PeerRequestOptions, ServiceError};
        let gate = Gate::new(false);
        let h = H::start(
            "e2e-cancel",
            Params::default(),
            Hooks {
                gate: Some(Arc::clone(&gate)),
                ..Hooks::default()
            },
        );
        let c = serve(&h).await;
        let r = call(&c, "session", json!({"action": "hello"})).await;
        assert_eq!(r.is_error, Some(false));
        gate.set(true);
        let Value::Object(args) = create("/x/cancelled") else {
            panic!("an object")
        };
        let request = ClientRequest::CallToolRequest(CallToolRequest::new(
            CallToolRequestParams::new("node").with_arguments(args),
        ));
        let handle = c
            .peer()
            .send_request_with_option(
                request,
                PeerRequestOptions::with_timeout(Duration::from_millis(300)),
            )
            .await
            .unwrap();
        // On its timeout the client sends notifications/cancelled.
        let r = handle.await_response().await;
        assert!(matches!(r, Err(ServiceError::Timeout { .. })), "{r:?}");
        assert_eq!(h.engine.queued(), 1, "still queued behind the gate");
        tokio::time::sleep(Duration::from_millis(300)).await;
        gate.set(false);
        until("the job skipped", || {
            h.engine.stats().skipped.load(Ordering::SeqCst) == 1
        })
        .await;
        assert!(!h.has("/x/cancelled"), "nothing applied");
        // The next call runs as usual.
        let r = call(&c, "node", create("/x/kept")).await;
        assert_eq!(r.is_error, Some(false), "{}", text(&r));
        c.cancel().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_client_decides_reports_and_closes_over_streamable_http() {
        let h = H::start("e2e-full", Params::default(), Hooks::default());
        let c = serve(&h).await;
        let r = call(
            &c,
            "session",
            json!({"action": "hello", "principal": "slack:alice", "intent": "ship the fix"}),
        )
        .await;
        computed(&r, "s-1");
        let r = call(
            &c,
            "node",
            json!({"action": "create", "session": "s-1", "path": "/work/fix", "title": "Fix"}),
        )
        .await;
        let token = computed(&r, "s-1")["token"].as_str().unwrap().to_string();
        let r = call(
            &c,
            "hold",
            json!({"action": "checkpoint", "session": "s-1", "token": token,
                   "done": "fixed", "next_step": "report"}),
        )
        .await;
        computed(&r, "s-1");
        let r = call(
            &c,
            "talk",
            json!({"action": "decide", "session": "s-1", "at": "/work/fix", "text": "ship it"}),
        )
        .await;
        assert_eq!(r.is_error, Some(false), "{}", text(&r));
        assert!(computed(&r, "s-1").get("decision").is_some());
        let r = call(
            &c,
            "talk",
            json!({"action": "to_user", "session": "s-1", "text": "shipped the fix",
                   "kind": "result", "verbatim": true}),
        )
        .await;
        assert_eq!(r.is_error, Some(false), "{}", text(&r));
        computed(&r, "s-1");
        let r = call(
            &c,
            "node",
            json!({"action": "close", "session": "s-1", "path": "/work/fix",
                   "outcome": "done", "summary": "shipped", "token": token}),
        )
        .await;
        assert_eq!(r.is_error, Some(false), "{}", text(&r));
        computed(&r, "s-1");
        c.cancel().await.unwrap();
    }

    /// 1.8 (§20.1, §20.3; the lab's third round, l2 F3 and l4 F1): a later turn of the same run,
    /// in a client that starts a new MCP session per turn, passes its session to `hello`, which
    /// resumes it: no new session, no constitution, and its human's words logged in it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_later_turn_resumes_its_session_with_hello() {
        let h = H::start("e2e-resume", Params::default(), Hooks::default());
        let c = serve(&h).await;
        let r = call(
            &c,
            "session",
            json!({"action": "hello", "principal": "mail:sam@x.example"}),
        )
        .await;
        assert!(computed(&r, "s-1").get("constitution").is_some());
        c.cancel().await.unwrap();
        let sessions = h.state().sessions.len();
        // The next turn: another MCP session, bound to no forum session.
        let c = serve(&h).await;
        let r = call(
            &c,
            "session",
            json!({"action": "hello", "session": "s-1",
                   "from_user": {"text": "cool, I'll look tonight", "client": "ide", "to": "none"}}),
        )
        .await;
        assert_eq!(r.is_error, Some(false), "{}", text(&r));
        let v = computed(&r, "s-1");
        assert_eq!(v["session"], json!("s-1"));
        assert!(v.get("constitution").is_none(), "{v}");
        assert!(v.get("user_entry").is_some(), "{v}");
        assert_eq!(h.state().sessions.len(), sessions);
        // The MCP session is now bound to s-1: a call that leaves `session` out runs in it.
        let r = call(&c, "view", json!({"action": "list"})).await;
        computed(&r, "s-1");
        // An unknown session is not_found, as before.
        let r = call(&c, "session", json!({"action": "hello", "session": "s-99"})).await;
        assert_eq!(r.is_error, Some(true));
        assert!(text(&r).starts_with("error: not_found"), "{}", text(&r));
        c.cancel().await.unwrap();
    }
}

// ------------------------------------------------------------------------------------------------
// 1.7: the versions a session was shown, and the arguments the decoder ignores (§20.2, §20.5)

/// 1.7 (§20.5): `node update` without its versions takes those last shown to the session: by the
/// brief of a `create`, a `take`, or a `view node`, and after the session's own update. Another
/// session's change makes them stale, so the edit is `stale_version` as one that names its
/// versions is (O2). With none shown, for a session that never read the node or after a restart,
/// the shell refuses, naming the current versions and the same update with them filled. A retry of
/// the same call gets the stored response, whatever the session was shown since.
#[tokio::test(flavor = "multi_thread")]
async fn node_update_takes_the_versions_last_shown_to_its_session() {
    let h = H::start("shown-versions", Params::default(), Hooks::default());
    let a = h.hello().await;
    let r = h.call(Some(a), "node", create("/n")).await;
    assert!(!r.is_error, "{}", r.text);
    // The brief of the create was shown: both versions are filled.
    let r = h
        .call(
            Some(a),
            "node",
            json!({"action": "update", "path": "/n", "digest": "first digest"}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    // The session's own update moved them.
    let r = h
        .call(
            Some(a),
            "node",
            json!({"action": "update", "path": "/n", "title": "second title", "op_id": "u-2"}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    let stored = r.text.clone();
    // Another session, never shown the node, is refused, with the values to retry with.
    let b = h.hello().await;
    let r = h
        .call(
            Some(b),
            "node",
            json!({"action": "update", "path": "/n", "body": "b's body"}),
        )
        .await;
    assert!(r.is_error);
    let hint = body(&r)["hint"].as_str().unwrap();
    assert!(
        hint.starts_with("version: missing, and this session was not shown #"),
        "{hint}"
    );
    assert!(
        hint.contains("retry with them: node update {path: #"),
        "{hint}"
    );
    assert!(
        !r.text.contains("next_calls"),
        "a shell answer carries none: {}",
        r.text
    );
    // Once shown, it updates; the first session's versions are then stale.
    let r = h
        .call(Some(b), "view", json!({"action": "node", "path": "/n"}))
        .await;
    assert!(!r.is_error, "{}", r.text);
    let r = h
        .call(
            Some(b),
            "node",
            json!({"action": "update", "path": "/n", "body": "b's body"}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    let r = h
        .call(
            Some(a),
            "node",
            json!({"action": "update", "path": "/n", "title": "a's late title"}),
        )
        .await;
    assert!(r.is_error);
    assert_eq!(body(&r)["error"], json!("stale_version"), "{}", r.text);
    // A retry of the same call gets its stored response, though the session's versions moved.
    let r = h
        .call(
            Some(a),
            "node",
            json!({"action": "update", "path": "/n", "title": "second title", "op_id": "u-2"}),
        )
        .await;
    assert_eq!(r.text, stored);
    // After a restart nothing is remembered.
    let h = h.restart(Params::default(), Hooks::default());
    let r = h
        .call(Some(b), "view", json!({"action": "find", "text": "work"}))
        .await;
    assert!(!r.is_error, "{}", r.text);
    let r = h
        .call(
            Some(b),
            "node",
            json!({"action": "update", "path": "/n", "body": "again"}),
        )
        .await;
    assert!(r.is_error);
    assert!(
        body(&r)["hint"]
            .as_str()
            .unwrap()
            .starts_with("version: missing, and this session was not shown"),
        "{}",
        r.text
    );
}

/// 1.7 (§20.2, §21): an argument of another action is ignored, and the response, applied or
/// refused, warns `ignored_arguments`; the node a call is about may be named `path`.
/// 1.8 (§20.9 "Folding", l6 F8): a standing item shown on each of the session's 3 previous
/// responses is folded into one line, and carried as `folded`.
#[tokio::test(flavor = "multi_thread")]
async fn a_standing_next_call_shown_three_times_is_folded() {
    let h = H::start("fold", Params::default(), Hooks::default());
    let s = h.hello().await;
    // 1.9: no confirm is suggested any more (§9.2); an unrouted entry of the session's is a
    // standing item, rule 5's create by key.
    let r = h
        .call(
            Some(s),
            "talk",
            json!({"action": "from_user", "text": "please look at the parser", "client": "ide",
                   "principal": "mail:alice@example.org"}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    // The create, standing and `now`, is shown on the from_user and on two reads.
    assert!(r.text.contains("node create {session: "), "{}", r.text);
    let look = || h.call(Some(s), "view", json!({"action": "list"}));
    for _ in 0..2 {
        let r = look().await;
        assert!(r.text.contains("node create {session: "), "{}", r.text);
        assert!(body(&r)["next_calls"].get("folded").is_none());
    }
    // The fourth response folds it, and so does the fifth.
    for _ in 0..2 {
        let r = look().await;
        assert!(!r.text.contains("node create {"), "{}", r.text);
        assert!(
            r.text
                .contains("next_calls (0 of 1):\n+ 1 standing item, shown before"),
            "{}",
            r.text
        );
        assert_eq!(body(&r)["next_calls"]["folded"], json!(1));
        assert_eq!(body(&r)["next_calls"]["items"], json!([]));
    }
}

/// 1.8 (§10.4, l5 F5, l3 F8): a waiting node's brief header shows its wait, and, once an outside
/// wait's recheck has come, that it is due.
#[tokio::test(flavor = "multi_thread")]
async fn a_waiting_node_s_brief_shows_its_wait() {
    let h = H::start("brief-wait", Params::default(), Hooks::default());
    let s = h.hello().await;
    let r = h.call(Some(s), "node", create("/w")).await;
    let token = body(&r)["token"].as_str().unwrap().to_string();
    let r = h
        .call(
            Some(s),
            "hold",
            json!({"action": "release", "token": token, "done": "started the job",
                   "next_step": "record its values", "reason": "blocked",
                   "waits_for": {"external": "cluster job 9125", "recheck_after": "in 1h"}}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    let view = || h.call(Some(s), "view", json!({"action": "node", "path": "/w"}));
    let r = view().await;
    let wait = &body(&r)["node"]["wait"];
    assert_eq!(wait["for"], json!("cluster job 9125"), "{}", r.text);
    assert!(wait.get("since").is_some() && wait.get("recheck").is_some());
    assert!(wait.get("due").is_none(), "{}", r.text);
    h.clock.set(T0.plus(Dur(2 * 3_600_000)));
    let r = view().await;
    let due = body(&r)["node"]["wait"]["due"].as_str().unwrap_or_default();
    assert!(
        due.starts_with("recheck due since ")
            && due.ends_with(": check it, then decide or release"),
        "{}",
        r.text
    );
}

/// 1.8 (§13, §20.9 rule 11, l4 F8): an agent question delivered on a write says that its answer
/// may wait, and its answer is a `later` next call.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_question_delivered_on_a_write_may_wait() {
    let h = H::start("notes-wait", Params::default(), Hooks::default());
    let a = h.hello().await;
    let b = h.hello().await;
    assert!(!h.call(Some(a), "node", create("/q")).await.is_error);
    let r = h
        .call(
            Some(a),
            "talk",
            json!({"action": "ask", "at": "/q", "text": "where is the repo?",
                   "to": format!("session:s-{b}")}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    let r = h.call(Some(b), "node", create("/b")).await;
    assert!(!r.is_error, "{}", r.text);
    let note = &body(&r)["notes"][0];
    assert!(
        note["answer"]
            .as_str()
            .is_some_and(|x| x.starts_with("it may wait until you know it: open until ")),
        "{}",
        r.text
    );
    assert!(r.text.contains("later: talk answer {"), "{}", r.text);
    assert!(
        r.text.contains(&format!(
            "once you know the answer, by {}: s-{a} asked session:s-{b}",
            crate::render::format::time(T0.plus(Dur(24 * 3_600_000)))
        )),
        "{}",
        r.text
    );
}

/// 1.8 (§8.1, l3 F1): `to_user {at}` reports to the requester of the node's requests.
#[tokio::test(flavor = "multi_thread")]
async fn a_report_about_a_node_goes_to_its_requester() {
    let h = H::start("to-user-at", Params::default(), Hooks::default());
    let w = h.hello().await;
    assert!(!h.call(Some(w), "node", create("/report")).await.is_error);
    let r = h
        .call_as(
            "assistant",
            None,
            "session",
            json!({"action": "hello", "principal": "mail:gita@x.example",
                   "from_user": {"text": "the weekly report, please", "client": "mail",
                                 "to": "/report"}}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    let request = body(&r)["user_entry"]["routed"]["request"]
        .as_str()
        .unwrap()
        .to_string();
    let r = h
        .call(
            Some(w),
            "talk",
            json!({"action": "to_user", "at": "/report", "text": "the table", "kind": "result"}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    assert_eq!(
        body(&r)["principal"],
        json!("mail:gita@x.example"),
        "{}",
        r.text
    );
    assert_eq!(body(&r)["re"], json!([request]), "{}", r.text);
    assert!(!r.text.contains("ignored_arguments"), "{}", r.text);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_argument_of_another_action_is_ignored_with_a_warning() {
    let h = H::start("ignored-arguments", Params::default(), Hooks::default());
    let a = h.hello().await;
    let r = h.call(Some(a), "node", create("/n")).await;
    assert!(!r.is_error, "{}", r.text);
    let r = h
        .call(
            Some(a),
            "talk",
            json!({"action": "say", "path": "/n", "text": "measured", "evidence": "observation"}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    let warnings = body(&r)["warnings"].as_array().unwrap();
    let w = warnings
        .iter()
        .find(|w| w["code"] == json!("ignored_arguments"))
        .unwrap_or_else(|| panic!("{}", r.text));
    assert_eq!(
        w["detail"],
        json!(
            "talk say does not take evidence (talk decide takes it), so the call was applied \
             without it"
        )
    );
    assert!(
        r.text.contains("\"code\":\"ignored_arguments\""),
        "{}",
        r.text
    );
    // On a refusal of the core too, where it would be ignored, since nothing was applied (1.8,
    // §21, it2 l1: "was applied without it" on a refusal left the agent unsure).
    let r = h
        .call(
            Some(a),
            "talk",
            json!({"action": "say", "at": "/nowhere", "text": "x", "why": "because"}),
        )
        .await;
    assert!(r.is_error);
    let w = body(&r)["warnings"]
        .as_array()
        .and_then(|ws| ws.iter().find(|w| w["code"] == json!("ignored_arguments")))
        .cloned()
        .unwrap_or_else(|| panic!("{}", r.text));
    assert_eq!(
        w["detail"],
        json!(
            "talk say does not take why (hold renew takes it), so it would be ignored: leave it \
             out when you retry"
        )
    );
    assert!(!r.text.contains("applied without"), "{}", r.text);
}

/// 1.8 (§21, it2 l6: "pick another name" sent the incident's lead to a duplicate node): a create
/// on the path of a live node names that node, its state and holders, and the calls that take it
/// and give it the create's key; a move there names the node, and a merge into it.
#[tokio::test(flavor = "multi_thread")]
async fn a_create_on_a_live_path_names_the_node_there() {
    let h = H::start("name-taken", Params::default(), Hooks::default());
    let a = h.hello().await;
    let r = h
        .call(
            Some(a),
            "node",
            json!({"action": "create", "path": "/inc/eu-500s", "title": "INC-311: EU checkout returns 500s"}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    let id = body(&r)["id"].as_str().unwrap().to_string();
    let b = h.hello().await;
    let r = h
        .call(
            Some(b),
            "node",
            json!({"action": "create", "path": "/inc/eu-500s", "title": "EU checkout 500s",
                   "keys": ["incident:inc-311"]}),
        )
        .await;
    assert!(r.is_error, "{}", r.text);
    let hint = body(&r)["hint"].as_str().unwrap();
    assert_eq!(
        hint,
        format!(
            "/inc/eu-500s is already {id} \"INC-311: EU checkout returns 500s\", open, held by \
             s-{a}: if it is your work, take it: hold take {{target: {id}}}, and give it your key: \
             node key {{path: {id}, add: incident:inc-311}}"
        )
    );
    assert!(!hint.contains("pick another name"), "{hint}");
    // With several keys, each of them.
    let r = h
        .call(
            Some(b),
            "node",
            json!({"action": "create", "path": "/inc/eu-500s", "title": "EU checkout 500s",
                   "keys": ["incident:inc-311", "chat:inc/t7"]}),
        )
        .await;
    let hint = body(&r)["hint"].as_str().unwrap();
    assert!(
        hint.ends_with(&format!(
            "and give it your keys, one node key {{path: {id}, add: <key>}} each: \
             incident:inc-311, chat:inc/t7"
        )),
        "{hint}"
    );
    // A move there: the node, and a merge into it.
    let r = h.call(Some(b), "node", create("/inc/other")).await;
    assert!(!r.is_error, "{}", r.text);
    let r = h
        .call(
            Some(b),
            "node",
            json!({"action": "move", "path": "/inc/other", "to": "/inc/eu-500s"}),
        )
        .await;
    assert!(r.is_error, "{}", r.text);
    let hint = body(&r)["hint"].as_str().unwrap();
    assert!(
        hint.starts_with(&format!("/inc/eu-500s is already {id} ")),
        "{hint}"
    );
    assert!(
        hint.ends_with(&format!(
            "move to another path, or, if it is the same work, node merge {{from, into: {id}}}"
        )),
        "{hint}"
    );
    // A closed node there: its reopen.
    let r = h
        .call(
            Some(a),
            "node",
            json!({"action": "close", "path": "/inc/eu-500s", "outcome": "done", "summary": "s"}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    let r = h.call(Some(b), "node", create("/inc/eu-500s")).await;
    assert!(r.is_error, "{}", r.text);
    let hint = body(&r)["hint"].as_str().unwrap();
    assert_eq!(
        hint,
        format!(
            "/inc/eu-500s is already {id} \"INC-311: EU checkout returns 500s\", closed done: to \
             continue it, node reopen {{path: {id}, reason}}; for other work, a path of its own"
        )
    );
}

/// 1.8 (§21, it2 l1): a `said` of kind `question` is refused with the form of `ask` for a person
/// in a thread, which records the `said` itself.
#[tokio::test(flavor = "multi_thread")]
async fn a_said_question_names_the_person_form_of_ask() {
    let h = H::start("said-question", Params::default(), Hooks::default());
    let a = h.hello().await;
    let r = h.call(Some(a), "node", create("/n")).await;
    assert!(!r.is_error, "{}", r.text);
    let r = h
        .call(
            Some(a),
            "talk",
            json!({"action": "said", "at": "/n", "where": "chat:eng/t1", "text": "may I?",
                   "kind": "question"}),
        )
        .await;
    assert!(r.is_error, "{}", r.text);
    assert_eq!(
        body(&r)["hint"],
        json!(
            "a question to a person goes through talk ask with no to: talk ask {at, where?, text, \
             blocking?} records the said for you"
        )
    );
}

/// 1.12 (§20.1, §21, it6 l3): the writer binds an MCP session to the session its call opened before
/// it answers; a later call of that MCP session that names another session, unknown or retired, is
/// answered in terms of the connection's session: its hint names it, and its next call is the same
/// call in it, not `session hello`; a `hello` that resumes such a session is told to resume the
/// connection's, with no word against `session hello`. An MCP session bound to none is answered as
/// before.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_naming_another_session_is_answered_in_its_connection_s() {
    let h = H::start("rebind", Params::default(), Hooks::default());
    let mut hello = h.call_of("coding-agent", None, "session", json!({"action": "hello"}));
    hello.mcp_session = Some("m1".into());
    let r = Backend::call(h.engine.as_ref(), hello).await;
    let s = r.session.expect("opened");
    assert_eq!(
        h.binding
            .lock()
            .unwrap()
            .resolve(&class("coding-agent"), Some("m1"), None),
        SessionCtx::Existing(s),
        "bound by the writer"
    );
    let mut find = h.call_of(
        "coding-agent",
        Some(99),
        "view",
        json!({"action": "find", "text": "Postgres 16"}),
    );
    find.mcp_session = Some("m1".into());
    let r = Backend::call(h.engine.as_ref(), find).await;
    assert!(r.is_error, "{}", r.text);
    assert!(
        r.text.contains(&format!(
            "hint: s-99 is unknown or retired; this connection's session is {0}: make the same \
             call with session: {0}, not session hello",
            s.render()
        )),
        "{}",
        r.text
    );
    assert!(
        r.text.ends_with(&format!(
            "next_calls (1 of 1):\n1. view find {{session: {0}, text: ?}} — this connection's \
             session is {0}: the same call in it",
            s.render()
        )),
        "{}",
        r.text
    );
    let items = &body(&r)["next_calls"]["items"];
    assert_eq!(
        items[0]["args"],
        json!({"action": "find", "session": s.render()})
    );
    assert_eq!(items[0]["fill"], json!(["text"]));
    // A `hello` that resumes s-99: the same call resumes the connection's session.
    let mut resume = h.call_of(
        "coding-agent",
        Some(99),
        "session",
        json!({"action": "hello"}),
    );
    resume.mcp_session = Some("m1".into());
    let r = Backend::call(h.engine.as_ref(), resume).await;
    assert!(r.is_error, "{}", r.text);
    assert!(
        r.text.contains(&format!(
            "hint: s-99 is unknown or retired; this connection's session is {0}: make the same \
             call with session: {0}\n",
            s.render()
        )),
        "{}",
        r.text
    );
    assert!(!r.text.contains("not session hello"), "{}", r.text);
    assert!(
        r.text.ends_with(&format!(
            "next_calls (1 of 1):\n1. session hello {{session: {0}}} — this connection's session \
             is {0}: the same call in it",
            s.render()
        )),
        "{}",
        r.text
    );
    // Through an MCP session bound to none: `session hello`, as before.
    let mut find = h.call_of(
        "coding-agent",
        Some(99),
        "view",
        json!({"action": "find", "text": "Postgres 16"}),
    );
    find.mcp_session = Some("m2".into());
    let r = Backend::call(h.engine.as_ref(), find).await;
    assert!(
        r.text.contains("open a new one with session hello"),
        "{}",
        r.text
    );
    assert!(r.text.contains("1. session hello {}"), "{}", r.text);
}

/// 1.12 (§10.3, D.4.12): a release's `settles` names requests, as a close's and a decide's do, so
/// one that left memory, settled more than `FINISHED_RETAIN` ago, is restored first and answered
/// `settled before`, as a close would answer it; before, the release was refused `not_found`.
#[tokio::test(flavor = "multi_thread")]
async fn a_release_restores_a_request_it_settles_that_left_memory() {
    let h = H::start("release-restores", Params::default(), Hooks::default());
    let w = h.hello().await;
    assert!(!h.call(Some(w), "node", create("/merge")).await.is_error);
    let r = h
        .call_as(
            "assistant",
            None,
            "session",
            json!({"action": "hello", "principal": "mail:gita@x.example",
                   "from_user": {"text": "fix the merge, please", "client": "mail",
                                 "to": "/merge"}}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    let request = body(&r)["user_entry"]["routed"]["request"]
        .as_str()
        .unwrap()
        .to_string();
    for args in [
        json!({"action": "to_user", "at": "/merge", "text": "fixed", "kind": "result"}),
        json!({"action": "decide", "at": "/merge", "text": "fixed", "settles": [request]}),
    ] {
        let r = h.call(Some(w), "talk", args).await;
        assert!(!r.is_error, "{}", r.text);
    }
    // Eight days on, a new session's first call prunes the settled request.
    h.clock
        .advance(forum_core::Dur(forum_core::FINISHED_RETAIN.0 + 86_400_000));
    let w2 = h.hello().await;
    let id = forum_core::Eid::parse(&request).unwrap();
    assert!(!h.state().requests.contains_key(&id));
    let r = h
        .call(
            Some(w2),
            "hold",
            json!({"action": "take", "path": "/merge"}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    let token = body(&r)["token"].as_str().unwrap().to_string();
    let r = h
        .call(
            Some(w2),
            "hold",
            json!({"action": "release", "token": token, "done": "waits for the merge",
                   "next_step": "close it once merged", "reason": "blocked",
                   "settles": [request],
                   "waits_for": {"external": "PR #3 merged", "recheck_after": "2h"}}),
        )
        .await;
    assert!(!r.is_error, "{}", r.text);
    assert_eq!(
        body(&r)["not_settled"],
        json!([{"request": request, "why": "settled before"}]),
        "{}",
        r.text
    );
    assert!(h.state().requests.contains_key(&id));
}
