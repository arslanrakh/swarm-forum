//! The end-to-end harness of WP-F2 (design D.12.2, D.12.6): the real server stack in process (the
//! engine with its writer, the search feeder and the checkpointer, the `/mcp` route behind the
//! bearer layer, served by axum on a local port) over a temporary database, with a manual clock,
//! and rmcp clients that speak MCP streamable HTTP to it, one MCP session per agent and one bearer
//! token per class.
//!
//! Every COMMIT of the writer is followed by the persist-and-reload check (D.12.6): the state
//! tables, loaded into a fresh `State` as startup loads them, equal the writer's in-memory state,
//! field by field, indices included (the engine's `after_commit` test hook). `Forum::verify`
//! folds the history from Genesis and compares it with the state tables, then settles it and runs
//! the full check (`replay::verify_checked`).

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use forum_core::{Dur, Eid, FlagSet, Name, NodeId, Params, ROOT, SessionId, State, Time};
use forum_server::auth::{SharedTokens, TokenMap, hex, sha256};
use forum_server::clock::{Clock, ManualClock};
use forum_server::engine::{Engine, Hooks, OnFatal, Options, StateHook};
use forum_server::mcp::server::{ForumServer, HttpSettings, router};
use forum_server::mcp::tools::INSTRUCTIONS;
use forum_server::session_bind::SessionBinding;
use forum_server::store::load::load_state;
use forum_server::store::replay::{differences, verify_checked};
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult, ClientConfig};
use rmcp::service::RunningService;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rusqlite::Connection;
use serde_json::{Value, json};

/// 2026-10-01T09:00:00Z, §32's start (D.12.2).
pub const T0: Time = Time(1_790_845_200_000);
pub const MINUTE_MS: u64 = 60_000;
pub const HOUR_MS: u64 = 3_600_000;

/// The classes of the tests, each with its own bearer token (test values, never real ones).
pub const CLASSES: [(&str, &str); 3] = [
    (
        "chat-assistant",
        "fsk_e2e-chat-assistant-token-0123456789abcdefghijklm",
    ),
    (
        "chat-worker",
        "fsk_e2e-chat-worker-token-0123456789abcdefghijklmno",
    ),
    (
        "coding-agent",
        "fsk_e2e-coding-agent-token-0123456789abcdefghijklm",
    ),
];

/// The bearer token of `class`.
#[must_use]
pub fn token_of(class: &str) -> &'static str {
    CLASSES
        .iter()
        .find(|(c, _)| *c == class)
        .map(|(_, t)| *t)
        .unwrap_or_else(|| panic!("no token for {class}"))
}

/// The `FORUM_TOKEN_*` variables of [`CLASSES`], as the environment would hold them.
#[must_use]
pub fn token_vars() -> Vec<(String, String)> {
    CLASSES
        .iter()
        .map(|(class, token)| {
            (
                format!("FORUM_TOKEN_{}", class.replace('-', "_").to_uppercase()),
                format!("{};class={class}", hex(&sha256(token.as_bytes()))),
            )
        })
        .collect()
}

/// The token map of [`CLASSES`].
#[must_use]
pub fn token_map() -> TokenMap {
    TokenMap::load(token_vars(), None).unwrap()
}

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

/// A directory of the test's files, removed when dropped.
pub struct Dir(pub PathBuf);

impl Dir {
    #[must_use]
    pub fn new(tag: &str) -> Dir {
        let n = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("forum-e2e-{}-{tag}-{n}", std::process::id()));
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

/// What the persist-and-reload check found: how many commits it checked, and every difference.
#[derive(Default)]
pub struct Reloads {
    pub checked: AtomicU64,
    pub failures: Mutex<Vec<String>>,
}

/// The persist-and-reload check after each COMMIT (D.12.6): the resident rows, loaded into a fresh
/// state (D.11.8 step 2, which recounts the indices), equal the writer's in-memory state.
fn reload_hook(db: PathBuf, r: Arc<Reloads>) -> StateHook {
    Arc::new(move |s: &State| {
        let found = Connection::open(&db)
            .map_err(|e| e.to_string())
            .and_then(|c| load_state(&c).map_err(|e| e.to_string()));
        match found {
            Ok(loaded) => {
                let d = differences(&loaded, s);
                if !d.is_empty() {
                    r.failures.lock().unwrap().push(format!(
                        "at last_now {:?}, after commit {}: the loaded state differs in {d:?}",
                        s.last_now,
                        r.checked.load(Ordering::SeqCst)
                    ));
                }
            }
            Err(e) => r.failures.lock().unwrap().push(format!("load: {e}")),
        }
        r.checked.fetch_add(1, Ordering::SeqCst);
    })
}

/// The route's task and the engine, stopped when dropped (before the files go).
struct Running {
    task: tokio::task::JoinHandle<()>,
    engine: Arc<Engine>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
        self.engine.shutdown();
    }
}

/// A running forum: the engine and its HTTP route on a local port.
pub struct Forum {
    pub engine: Arc<Engine>,
    running: Running,
    pub clock: Arc<ManualClock>,
    pub tokens: SharedTokens,
    pub addr: SocketAddr,
    pub reloads: Arc<Reloads>,
    pub params: Params,
    pub dir: Dir,
}

impl Forum {
    /// A new forum at [`T0`].
    pub async fn start(tag: &str, params: Params, hooks: Hooks) -> Forum {
        Forum::start_in(
            Dir::new(tag),
            Arc::new(ManualClock::new(T0)),
            params,
            hooks,
            Arc::new(Reloads::default()),
        )
        .await
    }

    async fn start_in(
        dir: Dir,
        clock: Arc<ManualClock>,
        params: Params,
        mut hooks: Hooks,
        reloads: Arc<Reloads>,
    ) -> Forum {
        let db = dir.0.join("forum.db");
        hooks.after_commit = Some(reload_hook(db.clone(), Arc::clone(&reloads)));
        let mut o = Options::new(db, Some(dir.0.join("forum-search.db")), params.clone());
        o.clock = Arc::clone(&clock) as Arc<dyn Clock>;
        o.on_fatal = OnFatal::StopWriter;
        o.hooks = hooks;
        let tokens = SharedTokens::new(token_map());
        let binding = Arc::new(Mutex::new(SessionBinding::new()));
        let engine = Engine::start(o, tokens.clone(), Arc::clone(&binding)).unwrap();
        let server = ForumServer::new(Arc::clone(&engine), INSTRUCTIONS.to_string(), binding);
        let http = HttpSettings {
            allowed_hosts: vec!["localhost".into(), "127.0.0.1".into()],
            mcp_idle: Duration::from_millis(params.presence_silence.0),
        };
        let app = router(server, tokens.clone(), &http);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Forum {
            running: Running {
                task,
                engine: Arc::clone(&engine),
            },
            engine,
            clock,
            tokens,
            addr,
            reloads,
            params,
            dir,
        }
    }

    /// Stops the engine (as SIGTERM does, or after a crash) and the route, and starts both again
    /// over the same files and clock, with `hooks`: startup loads the state tables (D.11.8). The
    /// clients of the old route are gone; agents connect again.
    pub async fn restart(self, hooks: Hooks) -> Forum {
        let Forum {
            dir,
            clock,
            engine,
            running,
            reloads,
            params,
            ..
        } = self;
        drop(running);
        drop(engine);
        Forum::start_in(dir, clock, params, hooks, reloads).await
    }

    #[must_use]
    pub fn db(&self) -> PathBuf {
        self.dir.0.join("forum.db")
    }

    #[must_use]
    pub fn conn(&self) -> Connection {
        Connection::open(self.db()).unwrap()
    }

    /// The state the tables hold, loaded as startup loads it.
    #[must_use]
    pub fn state(&self) -> State {
        load_state(&self.conn()).unwrap()
    }

    /// The number of events in the history.
    #[must_use]
    pub fn events(&self) -> u64 {
        forum_server::store::history::last_seq(&self.conn()).unwrap()
    }

    /// Time passes: the clock moves `ms` forward, the idle writer is told to look, and the test
    /// waits until it has: it fires the deadlines due as a group of its own (CS-30) and refreshes
    /// the health cache, so the next call sees the same state and status line on every run.
    /// Something must have committed since the cache was last computed.
    pub async fn advance(&self, ms: u64) {
        let computed = || self.engine.stats().health_computed.load(Ordering::SeqCst);
        let before = computed();
        self.clock.advance(Dur(ms));
        self.engine.kick();
        for _ in 0..2_000 {
            if computed() > before {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the idle writer did not look at the clock");
    }

    /// Waits until the search feeder has fed the whole history, so that the search hits of the
    /// next call do not depend on its pace.
    pub fn fed(&self) {
        let last = self.events();
        assert!(
            self.engine.search_caught_up(last, Duration::from_secs(30)),
            "the search feeder caught up"
        );
    }

    /// An agent of `class` with its own MCP session.
    pub async fn agent(&self, class: &str) -> Agent {
        Agent::connect(self.addr, class).await
    }

    /// The history replays to the state tables, and the replayed state, settled to its
    /// `last_now`, passes the full check (D.12.6).
    pub fn verify(&self) -> State {
        verify_checked(&self.conn()).unwrap_or_else(|e| panic!("history replay: {e}"))
    }

    /// Persist and reload held at every commit so far (D.12.6), and at least `min` were checked.
    pub fn assert_reloads(&self, min: u64) {
        let failures = self.reloads.failures.lock().unwrap().clone();
        assert!(failures.is_empty(), "persist and reload: {failures:#?}");
        let n = self.reloads.checked.load(Ordering::SeqCst);
        assert!(n >= min, "only {n} commits were checked");
    }
}

/// One tool result: its text, its structured content, and whether it is an error.
#[derive(Clone, Debug)]
pub struct Resp {
    pub text: String,
    pub v: Value,
    pub is_error: bool,
}

/// One next call: when, the tool, the arguments without `session`, and what the agent fills.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nc {
    pub when: String,
    pub tool: String,
    pub args: Value,
    pub fill: Vec<String>,
}

/// An expected next call.
#[must_use]
pub fn nc(when: &str, tool: &str, args: Value, fill: &[&str]) -> Nc {
    Nc {
        when: when.to_string(),
        tool: tool.to_string(),
        args,
        fill: fill.iter().map(|f| (*f).to_string()).collect(),
    }
}

impl Resp {
    fn of(r: &CallToolResult) -> Resp {
        Resp {
            text: r
                .content
                .iter()
                .filter_map(|c| c.as_text().map(|t| t.text.clone()))
                .collect::<Vec<_>>()
                .join("\n"),
            v: r.structured_content.clone().unwrap_or(Value::Null),
            is_error: r.is_error == Some(true),
        }
    }

    /// The status line: the text's first line.
    #[must_use]
    pub fn status(&self) -> &str {
        self.text.lines().next().unwrap_or("")
    }

    /// The codes of the warnings.
    #[must_use]
    pub fn warnings(&self) -> Vec<String> {
        self.v
            .get("warnings")
            .and_then(Value::as_array)
            .map(|ws| {
                ws.iter()
                    .filter_map(|w| w.get("code").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The warning with `code`.
    #[must_use]
    pub fn warning(&self, code: &str) -> Option<&Value> {
        self.v
            .get("warnings")
            .and_then(Value::as_array)?
            .iter()
            .find(|w| w.get("code").and_then(Value::as_str) == Some(code))
    }

    /// The next calls of `structuredContent`, with `args` without `session`; the `session` of
    /// every item but `hello`'s must be `session`.
    #[must_use]
    pub fn next_calls(&self, session: &str) -> Vec<Nc> {
        let items = self.v["next_calls"]["items"]
            .as_array()
            .unwrap_or_else(|| panic!("no next_calls: {}", self.text));
        items
            .iter()
            .map(|i| {
                let mut args = i["args"].clone();
                let obj = args.as_object_mut().unwrap();
                if obj.get("action").and_then(Value::as_str) != Some("hello") {
                    assert_eq!(
                        obj.get("session").and_then(Value::as_str),
                        Some(session),
                        "the item names the caller's session: {i}"
                    );
                }
                obj.remove("session");
                Nc {
                    when: i["when"].as_str().unwrap().to_string(),
                    tool: i["tool"].as_str().unwrap().to_string(),
                    args,
                    fill: i["fill"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|f| f.as_str().unwrap().to_string())
                        .collect(),
                }
            })
            .collect()
    }

    /// The `why` of the next call at `k`.
    #[must_use]
    pub fn why(&self, k: usize) -> String {
        s(&self.v["next_calls"]["items"][k], "why")
    }

    /// The text and structured forms of the next calls agree item by item (D.12.6): the block is
    /// the text's last part, each line names its item's tool and action, `later: ` marks a later
    /// item, and each `why` is at most 100 bytes and ends its line.
    pub fn check_next_calls_block(&self) {
        let items = self.v["next_calls"]["items"].as_array().unwrap();
        let lines: Vec<&str> = self.text.lines().collect();
        let head = lines
            .iter()
            .rposition(|l| l.starts_with("next_calls"))
            .unwrap_or_else(|| panic!("no next_calls block: {}", self.text));
        if items.is_empty() {
            // 1.7 (§20.9): `end` follows `none` when nothing is owed or offered.
            match self.v["next_calls"].get("end").and_then(Value::as_str) {
                Some(end) => assert_eq!(
                    lines[head],
                    format!("next_calls: none — end: {end}"),
                    "{}",
                    self.text
                ),
                None => assert_eq!(lines[head], "next_calls: none", "{}", self.text),
            }
            assert_eq!(head + 1, lines.len(), "the block is last: {}", self.text);
            return;
        }
        assert_eq!(
            lines[head],
            format!(
                "next_calls ({} of {}):",
                items.len(),
                self.v["next_calls"]["total"]
            ),
            "{}",
            self.text
        );
        let rest = &lines[head + 1..];
        assert_eq!(rest.len(), items.len(), "{}", self.text);
        for (k, (line, item)) in rest.iter().zip(items).enumerate() {
            let body = line
                .strip_prefix(&format!("{}. ", k + 1))
                .unwrap_or_else(|| panic!("item {k}: {line}"));
            let later = item["when"] == "later";
            assert_eq!(body.starts_with("later: "), later, "{line}");
            let body = body.strip_prefix("later: ").unwrap_or(body);
            let action = item["args"]["action"].as_str().unwrap();
            assert!(
                body.starts_with(&format!("{} {action} {{", item["tool"].as_str().unwrap())),
                "{line} vs {item}"
            );
            let why = item["why"].as_str().unwrap();
            assert!(body.ends_with(&format!(" — {why}")), "{line} vs {item}");
            assert!(why.len() <= 100, "why within 100 bytes: {why}");
        }
    }
}

/// An agent: an MCP client with its own MCP session, and the forum session it has opened.
pub struct Agent {
    client: RunningService<rmcp::RoleClient, ClientConfig>,
    pub class: String,
    pub session: Option<String>,
}

impl Agent {
    /// Connects to the route at `addr` with the token of `class`.
    pub async fn connect(addr: SocketAddr, class: &str) -> Agent {
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp"))
                .auth_header(token_of(class)),
        );
        let client = ClientConfig::default().serve(transport).await.unwrap();
        Agent {
            client,
            class: class.to_string(),
            session: None,
        }
    }

    /// The instructions the server sent at initialisation.
    #[must_use]
    pub fn instructions(&self) -> Option<String> {
        self.client.peer_info().and_then(|i| i.instructions.clone())
    }

    /// One tool call, as is: no session added (the MCP session is bound to the forum session its
    /// first call opened, D.11.4). Learns the session from the status line, and checks that it
    /// stays the same.
    pub async fn raw(&mut self, tool: &str, args: Value) -> Resp {
        let Value::Object(args) = args else {
            panic!("an object")
        };
        let r = self
            .client
            .call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(args))
            .await
            .unwrap_or_else(|e| panic!("{tool}: {e}"));
        let r = Resp::of(&r);
        if let Some(s) = session_of(r.status(), &self.class) {
            match &self.session {
                None => self.session = Some(s),
                Some(old) => assert_eq!(*old, s, "one forum session per agent: {}", r.text),
            }
        }
        r
    }

    /// One tool call, as is; `None` when the transport fails or no answer comes within 10 s (the
    /// server went away: the client may keep trying to resume a stream that is gone).
    pub async fn try_raw(&mut self, tool: &str, args: Value) -> Option<Resp> {
        let Value::Object(args) = args else {
            panic!("an object")
        };
        let call = self
            .client
            .call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(args));
        let r = tokio::time::timeout(Duration::from_secs(10), call)
            .await
            .ok()?
            .ok()?;
        Some(Resp::of(&r))
    }

    /// A call that must succeed, with the status line first and the next calls last.
    pub async fn ok(&mut self, tool: &str, args: Value) -> Resp {
        let r = self.raw(tool, args.clone()).await;
        assert!(!r.is_error, "{tool} {args} refused:\n{}", r.text);
        assert!(r.text.starts_with("[swarm] running"), "{}", r.text);
        assert_eq!(r.v["status"], json!(r.status()), "{}", r.text);
        r.check_next_calls_block();
        r
    }

    /// The agent's session, `s-<n>`.
    #[must_use]
    pub fn sid(&self) -> &str {
        self.session.as_deref().expect("a session")
    }

    /// The next calls of `r` for this agent.
    #[must_use]
    pub fn next(&self, r: &Resp) -> Vec<Nc> {
        r.next_calls(self.sid())
    }

    /// Ends the MCP session.
    pub async fn close(self) {
        let _ = self.client.cancel().await;
    }
}

/// The session a status line names for `class`: `session s-<n> (<class>)` (1.7, §20.1).
#[must_use]
pub fn session_of(status: &str, class: &str) -> Option<String> {
    let rest = status.split(" · session ").nth(1)?;
    let (id, after) = rest.split_once(' ')?;
    after
        .starts_with(&format!("({class})"))
        .then(|| id.to_string())
}

/// A JSON object's string field.
#[must_use]
pub fn s(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no string {k} in {v}"))
        .to_string()
}

/// The node at `path` in `st`.
#[must_use]
pub fn node(st: &State, path: &str) -> NodeId {
    let mut n = ROOT;
    for step in path.split('/').filter(|x| !x.is_empty()) {
        n = *st
            .names
            .get(&(n, Name::new(step).unwrap()))
            .unwrap_or_else(|| panic!("no node {path}"));
    }
    n
}

/// The entry id of `e<n>`.
#[must_use]
pub fn eid(e: &str) -> Eid {
    Eid(e.strip_prefix('e').unwrap().parse().unwrap())
}

/// The session id of `s-<n>`.
#[must_use]
pub fn sid(x: &str) -> SessionId {
    SessionId(x.strip_prefix("s-").unwrap().parse().unwrap())
}

/// The flags of `n` in `st`.
#[must_use]
pub fn flags(st: &State, n: NodeId) -> FlagSet {
    st.ix.node.get(&n).map_or(FlagSet::EMPTY, |x| x.flags)
}
