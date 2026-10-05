//! Crashes end to end (design D.12.6, WP-F2), over MCP streamable HTTP:
//! - a crash between a group's COMMIT and its replies (the engine's test hook drops the replies):
//!   every client of the group retries with the same `op_id` and gets the stored bytes, next calls
//!   included, byte for byte, again and again, from its own session or with none; the history does
//!   not grow; the same `op_id` with other arguments, or from another session, is
//!   `op_id_conflict` (O1);
//! - a crash inside a group (the hook stops the writer after some of its jobs ran `apply`, before
//!   COMMIT): no reply of the group is sent, a restart loads the state before the group, and each
//!   retry executes its command once (CS-13);
//! - with `settle.batch` 1, a crash after a job whose post-settle ran past one batch: its events
//!   and its `st_ops` row are both committed or neither, and the retry gets the stored response or
//!   runs once;
//! - the real binary killed with SIGKILL while calls are in flight, then started again on its
//!   database: a call answered before the kill is answered again with the same bytes, the one in
//!   flight runs once or returns what it stored, and `forum-server check` passes.
//!
//! After each, the history replays to the state tables (with the full check of the replayed
//! state), and persist and reload held at every commit of the in-process engines (D.12.6).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test code"
)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use common::*;
use forum_core::{Dur, Params};
use forum_server::engine::{Gate, Hooks};
use forum_server::store::state::stored_op;
use serde_json::{Value, json};

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

/// Opens a session for each of `n` agents of `class`.
async fn agents(f: &Forum, class: &str, n: usize) -> Vec<Agent> {
    let mut out = Vec::new();
    for _ in 0..n {
        let mut a = f.agent(class).await;
        a.ok("session", json!({"action": "hello"})).await;
        out.push(a);
    }
    out
}

fn create(path: &str, op: &str) -> Value {
    json!({"action": "create", "path": path, "title": format!("work at {path}"), "op_id": op})
}

/// Each agent's call `args(i)`, all at once; the agents come back with their responses.
async fn all_at_once(
    agents: Vec<Agent>,
    tool: &'static str,
    args: impl Fn(usize) -> Value,
) -> Vec<(Agent, Resp)> {
    let tasks: Vec<_> = agents
        .into_iter()
        .enumerate()
        .map(|(i, mut a)| {
            let v = args(i);
            tokio::spawn(async move {
                let r = a.raw(tool, v).await;
                (a, r)
            })
        })
        .collect();
    let mut out = Vec::new();
    for t in tasks {
        out.push(t.await.unwrap());
    }
    out
}

/// The answer to a call whose writer stopped before replying (crash-only, D.11.2).
fn unavailable(r: &Resp) -> bool {
    r.is_error && r.text.starts_with("error: unavailable")
}

/// How many open or closed nodes carry `title`.
fn nodes_titled(f: &Forum, title: &str) -> usize {
    f.state()
        .nodes
        .values()
        .filter(|n| n.title.0 == title)
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crash_between_a_group_s_commit_and_its_replies_is_answered_by_the_stored_bytes() {
    let gate = Gate::new(false);
    let drop_replies = Arc::new(AtomicBool::new(false));
    let hooks = Hooks {
        gate: Some(Arc::clone(&gate)),
        drop_replies_after_commit: Arc::clone(&drop_replies),
        ..Hooks::default()
    };
    let f = Forum::start("crash-reply", Params::default(), hooks).await;
    let workers = agents(&f, "coding-agent", 3).await;
    let sessions: Vec<String> = workers.iter().map(|a| a.sid().to_string()).collect();
    let mut other = f.agent("coding-agent").await;
    other.ok("session", json!({"action": "hello"})).await;
    // Three writes queue behind the gate and run as one group, whose replies are lost after its
    // COMMIT.
    gate.set(true);
    let args = |i: usize| create(&format!("/r/n{i}"), &format!("op-r{i}"));
    let pending = tokio::spawn(all_at_once(workers, "node", args));
    until("three queued jobs", || f.engine.queued() == 3).await;
    let groups = f.engine.stats().groups.load(Ordering::SeqCst);
    drop_replies.store(true, Ordering::SeqCst);
    gate.set(false);
    let lost = pending.await.unwrap();
    assert_eq!(
        f.engine.stats().groups.load(Ordering::SeqCst),
        groups + 1,
        "one group"
    );
    for (_, r) in &lost {
        assert!(unavailable(r), "no reply of the group was sent: {}", r.text);
    }
    assert!(f.engine.alive(), "the writer runs on");
    let events = f.events();
    for i in 0..3 {
        assert_eq!(
            nodes_titled(&f, &format!("work at /r/n{i}")),
            1,
            "the group committed"
        );
    }
    // Each client retries with the same op_id: the stored bytes, next calls included, every time.
    for (i, (mut a, _)) in lost.into_iter().enumerate() {
        let first = a.ok("node", args(i)).await;
        assert_eq!(a.sid(), sessions[i]);
        assert!(first.text.contains("\nnext_calls"), "the stored next calls");
        assert!(
            first.v.get("next_calls").is_some(),
            "and in structuredContent"
        );
        let second = a.raw("node", args(i)).await;
        assert_eq!(second.text, first.text, "byte-identical");
        assert_eq!(second.v, first.v);
        // A fresh MCP session that names the session gets them too; one that names none is
        // refused, and opens and appends nothing (1.6, §20.1).
        let mut fresh = f.agent("coding-agent").await;
        let mut named = args(i);
        named["session"] = json!(sessions[i]);
        let third = fresh.raw("node", named).await;
        assert_eq!(
            third.text, first.text,
            "byte-identical, from another MCP session too"
        );
        assert_eq!(third.v, first.v);
        let mut bare = f.agent("coding-agent").await;
        let r = bare.raw("node", args(i)).await;
        assert!(
            r.is_error && r.text.starts_with("error: invalid\nhint: session: missing"),
            "{}",
            r.text
        );
        // The stored row is what the retries returned.
        let stored = stored_op(&f.conn(), "coding-agent", &format!("op-r{i}"))
            .unwrap()
            .unwrap();
        let v: Value = serde_json::from_slice(&stored.response).unwrap();
        assert_eq!(v["text"].as_str(), Some(first.text.as_str()));
        // Other arguments, or another session, conflict.
        let r = a
            .raw("node", create("/r/elsewhere", &format!("op-r{i}")))
            .await;
        assert!(r.text.contains("error: op_id_conflict"), "{}", r.text);
        let r = other.raw("node", args(i)).await;
        assert!(r.text.contains("error: op_id_conflict"), "{}", r.text);
    }
    assert_eq!(f.events(), events, "the repeats appended nothing");
    for i in 0..3 {
        assert_eq!(nodes_titled(&f, &format!("work at /r/n{i}")), 1);
    }
    f.assert_reloads(5);
    f.verify();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crash_inside_a_group_leaves_the_state_before_it_and_each_retry_runs_once() {
    let gate = Gate::new(false);
    let hooks = Hooks {
        gate: Some(Arc::clone(&gate)),
        crash_after_jobs: Some(2),
        ..Hooks::default()
    };
    let f = Forum::start("crash-group", Params::default(), hooks).await;
    let workers = agents(&f, "coding-agent", 3).await;
    let sessions: Vec<String> = workers.iter().map(|a| a.sid().to_string()).collect();
    let before = f.state();
    let events = f.events();
    gate.set(true);
    let args = |i: usize| create(&format!("/c/n{i}"), &format!("op-c{i}"));
    let pending = tokio::spawn(all_at_once(workers, "node", args));
    until("three queued jobs", || f.engine.queued() == 3).await;
    gate.set(false);
    for (_, r) in pending.await.unwrap() {
        assert!(
            unavailable(&r),
            "no reply of the group was sent: {}",
            r.text
        );
    }
    assert!(
        !f.engine.alive(),
        "the writer ended, as a crash would end it"
    );
    // Restart: the state tables hold the state before the group.
    let f = f.restart(Hooks::default()).await;
    assert_eq!(f.events(), events, "nothing of the group was committed");
    assert!(f.state() == before, "the state before the group");
    // Each retry, from its own session (named, since the MCP sessions went with the old route),
    // executes once; a second retry is answered by the stored response.
    for (i, x) in sessions.iter().enumerate() {
        let mut a = f.agent("coding-agent").await;
        let mut v = args(i);
        v["session"] = json!(x);
        let r = a.ok("node", v.clone()).await;
        assert_eq!(a.sid(), x);
        assert!(r.v.get("token").is_some(), "it ran: {}", r.text);
        let again = a.raw("node", v).await;
        assert_eq!(again.text, r.text);
        assert_eq!(again.v, r.v);
    }
    for i in 0..3 {
        assert_eq!(
            nodes_titled(&f, &format!("work at /c/n{i}")),
            1,
            "each node once"
        );
    }
    f.assert_reloads(5);
    f.verify();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crash_after_a_post_settle_past_one_batch_keeps_events_and_stored_response_together() {
    let params = Params {
        settle_batch: 1,
        presence_silence: Dur(9 * MINUTE_MS),
        ..Params::default()
    };
    let f = Forum::start("crash-post-settle", params, Hooks::default()).await;
    let mut a = f.agent("coding-agent").await;
    let mut b = f.agent("coding-agent").await;
    let mut c = f.agent("coding-agent").await;
    for x in [&mut a, &mut b, &mut c] {
        x.ok("session", json!({"action": "hello"})).await;
    }
    a.ok(
        "node",
        json!({"action": "create", "path": "/n", "title": "t", "capacity": 2}),
    )
    .await;
    b.ok("hold", json!({"action": "take", "target": "/n"}))
        .await;
    let cs = c.sid().to_string();
    drop((a, b, c));
    // At 12 minutes the close ends both holds, so A's and B's Silence keys (9 minutes) fall due
    // at `now`: the close's post-settle fires them in two batches, and the writer stops after it.
    let f = f
        .restart(Hooks {
            crash_after_jobs: Some(1),
            ..Hooks::default()
        })
        .await;
    f.clock.set(T0.plus(Dur(12 * MINUTE_MS)));
    let close = json!({"action": "close", "path": "/n", "outcome": "done", "summary": "s",
                       "op_id": "op-close", "session": cs});
    let mut c = f.agent("coding-agent").await;
    let r = c.raw("node", close.clone()).await;
    assert!(unavailable(&r), "{}", r.text);
    let f = f.restart(Hooks::default()).await;
    let committed: i64 = f
        .conn()
        .query_row(
            "SELECT count(*) FROM txs WHERE op_id = 'op-close'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let stored = stored_op(&f.conn(), "coding-agent", "op-close").unwrap();
    assert_eq!(
        committed > 0,
        stored.is_some(),
        "the job's events and its st_ops row are committed together"
    );
    let events = f.events();
    let mut c = f.agent("coding-agent").await;
    let r = c.ok("node", close.clone()).await;
    assert_eq!(r.v["closed"], json!("/n"), "the retry: {}", r.text);
    let again = c.raw("node", close).await;
    assert_eq!(again.text, r.text, "a second retry is the stored response");
    if stored.is_some() {
        assert_eq!(f.events(), events, "a repeat appends nothing");
    }
    f.assert_reloads(5);
    f.verify();
}

/// The real binary, killed with SIGKILL while calls are in flight (crash-only, D.11.2), then
/// started again on its database.
#[cfg(unix)]
mod binary {
    use std::io::{BufRead, BufReader};
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;

    use super::*;

    /// A running `forum-server serve` on `dir/forum.db`, and the address it listens on.
    struct Server {
        child: Child,
        addr: std::net::SocketAddr,
    }

    impl Server {
        fn start(dir: &Path) -> Server {
            let mut child = Command::new(env!("CARGO_BIN_EXE_forum-server"))
                .args(["serve", "--db", "forum.db", "--listen", "127.0.0.1:0"])
                .current_dir(dir)
                .envs(token_vars())
                .env("NO_COLOR", "1")
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let stderr = child.stderr.take().unwrap();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                for l in BufReader::new(stderr).lines() {
                    let Ok(l) = l else { return };
                    let _ = tx.send(l);
                }
            });
            let mut seen = Vec::new();
            let addr = loop {
                let l = rx
                    .recv_timeout(Duration::from_secs(120))
                    .unwrap_or_else(|e| panic!("it serves: {e} {seen:?}"));
                if l.contains("serving MCP at /mcp")
                    && let Some(at) = l.split("listen=").nth(1)
                {
                    break at.split_whitespace().next().unwrap().parse().unwrap();
                }
                seen.push(l);
            };
            // Keep draining its log, so that it never blocks on a full pipe.
            std::thread::spawn(move || while rx.recv().is_ok() {});
            Server { child, addr }
        }

        async fn agent(&self, class: &str) -> Agent {
            Agent::connect(self.addr, class).await
        }

        /// SIGTERM, and waits for a clean stop.
        fn stop(mut self) {
            let ok = Command::new("kill")
                .args(["-TERM", &self.child.id().to_string()])
                .status()
                .unwrap()
                .success();
            assert!(ok);
            let status = self.child.wait().unwrap();
            assert!(status.success(), "{status:?}");
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    const CLIENTS: usize = 4;
    const CALLS: usize = 30;

    fn op(client: usize, k: usize) -> Value {
        create(&format!("/k/c{client}/n{k}"), &format!("op-k{client}-{k}"))
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sigkill_with_calls_in_flight_then_restart_answers_each_retry_once() {
        let dir = Dir::new("crash-sigkill");
        let srv = Server::start(&dir.0);
        let mut sessions = Vec::new();
        let mut clients = Vec::new();
        for _ in 0..CLIENTS {
            let mut a = srv.agent("coding-agent").await;
            a.ok("session", json!({"action": "hello"})).await;
            sessions.push(a.sid().to_string());
            clients.push(a);
        }
        // Each client writes its calls one after another, keeping the answers it got, until the
        // server goes away; the server is killed once a third of all calls are answered.
        let answered = Arc::new(AtomicUsize::new(0));
        let tasks: Vec<_> = clients
            .into_iter()
            .enumerate()
            .map(|(c, a)| {
                let answered = Arc::clone(&answered);
                tokio::spawn(async move {
                    let mut got: Vec<Resp> = Vec::new();
                    let mut a = a;
                    for k in 0..CALLS {
                        match a.try_raw("node", op(c, k)).await {
                            Some(r) if !r.is_error => {
                                got.push(r);
                                answered.fetch_add(1, Ordering::SeqCst);
                            }
                            _ => break,
                        }
                    }
                    got
                })
            })
            .collect();
        until("a third of the calls answered", || {
            answered.load(Ordering::SeqCst) >= CLIENTS * CALLS / 3
        })
        .await;
        let mut srv = srv;
        srv.child.kill().unwrap();
        srv.child.wait().unwrap();
        let mut answers = Vec::new();
        for t in tasks {
            answers.push(t.await.unwrap());
        }
        assert!(
            answers.iter().map(Vec::len).sum::<usize>() < CLIENTS * CALLS,
            "the kill came while calls were in flight"
        );
        drop(srv);
        // Restart on the same database: every call is retried from its session with its op_id.
        let srv = Server::start(&dir.0);
        for (c, x) in sessions.iter().enumerate() {
            let mut a = srv.agent("coding-agent").await;
            for k in 0..CALLS {
                let mut v = op(c, k);
                v["session"] = json!(x);
                let r = a.ok("node", v.clone()).await;
                if let Some(before) = answers[c].get(k) {
                    assert_eq!(
                        r.text, before.text,
                        "an answered call is answered again with the stored bytes"
                    );
                    assert_eq!(r.v, before.v);
                }
                let again = a.raw("node", v).await;
                assert_eq!(again.text, r.text, "and again");
            }
        }
        srv.stop();
        // Each node exists once; `forum-server check` passes; the history replays to the tables.
        let st = forum_server::store::load::load_state(
            &rusqlite::Connection::open(dir.0.join("forum.db")).unwrap(),
        )
        .unwrap();
        for c in 0..CLIENTS {
            for k in 0..CALLS {
                let title = format!("work at /k/c{c}/n{k}");
                assert_eq!(
                    st.nodes.values().filter(|n| n.title.0 == title).count(),
                    1,
                    "{title} once"
                );
            }
        }
        let out = Command::new(env!("CARGO_BIN_EXE_forum-server"))
            .args(["check", "--db", "forum.db"])
            .current_dir(&dir.0)
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "check: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        forum_server::store::replay::verify_checked(
            &rusqlite::Connection::open(dir.0.join("forum.db")).unwrap(),
        )
        .unwrap();
    }
}
