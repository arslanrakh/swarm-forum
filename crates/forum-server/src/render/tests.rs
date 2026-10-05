//! Tests of rendering (design D.12.6, D.13.2 WP-E3): snapshots of each response kind, the budget
//! properties of `hello` and the brief, and the next-call tests.
//!
//! The forum under test is the store's test world (`store::testkit`): every transaction is
//! written to an in-memory SQLite database, so the renderer reads texts and pages through
//! `StoreSource`, as the engine's will. Commands that the core implements so far (WP-D3) run
//! through `apply_tracked`; the entities of later packages (requests, `said`, decisions, user
//! questions) are made from the events their commands will emit.
//!
//! A snapshot lives in `src/render/snapshots/<name>.txt`. A missing one is written and the test
//! fails, so that it is reviewed before it is committed.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test code"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use forum_core::health::{Health, HealthReason, Level, PauseExclusion};
use forum_core::query::PauseView;
use forum_core::query::guide::{
    Because, Guidance, Moment, NextCall, RouteAlt, RouteSk, Rule, Skeleton,
};
use forum_core::*;
use rusqlite::Connection;
use serde_json::{Value, json};

use super::*;
use crate::decode::{DecodeEnv, EntryLookup, prepare};
use crate::store::history::TxKind;
use crate::store::testkit::{World, class, keystr, test_params};

// ------------------------------------------------------------------------------------------------
// The test kit

/// The shell's entry reads on the world's database (D.11.5).
struct Lookup<'c>(&'c Connection);

impl EntryLookup for Lookup<'_> {
    fn entry_node(&self, eid: Eid) -> Option<NodeId> {
        crate::store::shell_tables::entry_node(self.0, eid)
            .ok()
            .flatten()
            .map(|(n, _)| n)
    }

    fn question(&self, eid: Eid) -> Option<(NodeId, Option<SessionId>)> {
        crate::store::shell_tables::entry_node(self.0, eid)
            .ok()
            .flatten()
    }
}

fn green() -> Health {
    Health {
        level: Level::Green,
        reasons: Vec::new(),
        capacity: forum_core::health::CapacityReport::default(),
    }
}

fn t(s: &str) -> Text {
    Text(s.to_string())
}

fn text(main: &str, extra: &[(TextField, &str)]) -> EntryText {
    EntryText {
        main: t(main),
        extra: extra.iter().map(|(f, x)| (*f, t(x))).collect(),
    }
}

/// One call as the engine runs it: decoded, applied, written, and rendered.
struct Called {
    response: Response,
    out: Result<Done, Error>,
}

struct Kit {
    w: World,
    health: Health,
    hits: SearchHits,
    /// A call given no guidance renders the next calls the core computes for it, as the writer
    /// does (`query::guide`), instead of none.
    guided: bool,
    /// What each session was shown, as the writer keeps it (1.7, §20.1).
    seen: SeenBySession,
}

impl Kit {
    fn new() -> Kit {
        let mut w = World::memory(test_params());
        w.at(crate::store::testkit::START);
        Kit {
            w,
            health: green(),
            hits: SearchHits::default(),
            guided: false,
            seen: SeenBySession::new(),
        }
    }

    fn at(&mut self, ms: u64) {
        let t = self.w.clock.plus(Dur(ms));
        self.w.at(t);
    }

    /// Runs `tool` with `args` in `sess` (None: the call opens a session) of class `class`.
    fn call_as(
        &mut self,
        class_name: &str,
        sess: Option<SessionId>,
        tool: &str,
        args: Value,
        guidance: Option<&Guidance>,
    ) -> Called {
        let Value::Object(map) = args else {
            panic!("not an object");
        };
        let prepared = {
            let env = DecodeEnv {
                now: self.w.clock,
                text_max: self.w.s.params.text_max,
                coverage_window: self.w.s.params.coverage_window,
                entries: &Lookup(&self.w.conn),
            };
            match prepare(tool, map, &env) {
                Ok(p) => p,
                Err(e) => panic!("{tool}: {}", e.render()),
            }
        };
        let ctx = Ctx {
            now: self.w.clock,
            class: class(class_name),
            caps: None,
            session: sess.map_or(SessionCtx::New, SessionCtx::Existing),
            op: None,
        };
        let before = self.w.s.clone();
        let mut dirty = Tracked::new();
        let (events, out) = apply_tracked(&mut self.w.s, &ctx, &prepared.cmd, &mut dirty);
        if !events.is_empty() {
            self.w.write_applied(TxKind::Cmd, &before, &events, &dirty);
        }
        let opened = events.iter().find_map(|e| match &e.ev {
            Ev::SessionOpened { session, .. } => Some(*session),
            _ => None,
        });
        let session = match &out {
            Ok(d) => d.opened.or(sess),
            Err(_) => opened.or(sess),
        };
        let empty = Guidance {
            items: Vec::new(),
            total: Total::Exact(0),
            end: None,
        };
        let computed = (guidance.is_none() && self.guided).then(|| {
            forum_core::query::guide(
                &self.w.s,
                session,
                self.w.clock,
                &prepared.cmd,
                &out,
                &mut forum_core::query::NoMeter,
            )
        });
        let src = StoreSource::new(&self.w.conn);
        let ignored = prepared.ignored_detail();
        let ignored_refused = prepared.ignored_detail_refused();
        let response = response(&Input {
            state: &self.w.s,
            now: self.w.clock,
            session,
            opened,
            cmd: &prepared.cmd,
            node_args: &prepared.node_args,
            out: &out,
            guidance: Some(guidance.or(computed.as_ref()).unwrap_or(&empty)),
            health: &self.health,
            health_at: self.w.clock,
            hits: &self.hits,
            masked_fields: prepared.masked_fields,
            ignored: ignored
                .as_deref()
                .zip(ignored_refused.as_deref())
                .map(|(applied, refused)| IgnoredDetail { applied, refused }),
            src: &src,
            seen: session.and_then(|x| self.seen.get(x)),
            rebind: None,
        });
        if let (Some(x), Some(after)) = (session, &response.seen) {
            self.seen.set(x, after.clone());
        }
        carries_the_text(&response);
        Called { response, out }
    }

    fn call(&mut self, sess: Option<SessionId>, tool: &str, args: Value) -> Called {
        self.call_as("coder", sess, tool, args, None)
    }

    /// Opens a session with `hello`.
    fn hello(&mut self, class_name: &str, args: Value) -> (SessionId, Called) {
        let c = self.call_as(class_name, None, "session", args, None);
        let s = match &c.out {
            Ok(d) => d.opened.expect("opened"),
            Err(e) => panic!("{e:?}"),
        };
        (s, c)
    }

    /// Applies events as one transaction at the clock.
    fn events(&mut self, f: impl FnOnce(&mut crate::store::testkit::Draft)) {
        self.w.run(TxKind::Cmd, f);
    }

    /// Renders `reply` as the outcome of `cmd` in `sess`, without applying anything.
    fn render(&self, sess: Option<SessionId>, cmd: &Cmd, out: &Result<Done, Error>) -> Response {
        let src = StoreSource::new(&self.w.conn);
        let empty = Guidance {
            items: Vec::new(),
            total: Total::Exact(0),
            end: None,
        };
        response(&Input {
            state: &self.w.s,
            now: self.w.clock,
            session: sess,
            opened: None,
            cmd,
            node_args: &[],
            out,
            guidance: Some(&empty),
            health: &self.health,
            health_at: self.w.clock,
            hits: &self.hits,
            masked_fields: 0,
            ignored: None,
            src: &src,
            seen: sess.and_then(|x| self.seen.get(x)),
            rebind: None,
        })
    }

    fn node(&self, p: &str) -> NodeId {
        forum_core::query::resolve(&self.w.s, &crate::decode::values::node(p).unwrap()).unwrap()
    }
}

fn done(reply: Reply) -> Result<Done, Error> {
    Ok(Done {
        reply,
        warnings: Vec::new(),
        opened: None,
        stale_session: false,
        delivered: Vec::new(),
    })
}

fn ok(c: &Called) -> &Done {
    match &c.out {
        Ok(d) => d,
        Err(e) => panic!("refused: {e:?}\n{}", c.response.text),
    }
}

/// 1.5 (§20.1, D.11.7): the structured content carries everything the text does, and nothing
/// else: the status line as `status` (absent without one), the `hello` line as `hello`, `STALE
/// SESSION` as `stale_session`, the body's fields (or the body as `body`), `error` and `hint`,
/// `warnings`, and `next_calls` with as many items as the text's block. Every response the render
/// tests make is checked.
fn carries_the_text(r: &Response) {
    let v = r
        .structured
        .as_object()
        .unwrap_or_else(|| panic!("structured content is an object: {}", r.structured));
    let lines: Vec<&str> = r.text.lines().collect();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    // The status line.
    match lines.first() {
        Some(l) if l.starts_with("[swarm] ") => {
            assert_eq!(v.get("status"), Some(&json!(l)), "{}", r.text);
            seen.insert("status".into());
        }
        _ => assert!(v.get("status").is_none(), "no status line: {}", r.text),
    }
    // The hello of a call that opened its session.
    match lines.iter().find_map(|l| l.strip_prefix("hello: ")) {
        Some(h) => {
            let h: Value = serde_json::from_str(h).unwrap();
            assert_eq!(v.get("hello"), Some(&h), "{}", r.text);
            seen.insert("hello".into());
        }
        None => assert!(v.get("hello").is_none(), "{}", r.text),
    }
    if lines.contains(&status::STALE_SESSION) {
        assert_eq!(v.get("stale_session"), Some(&json!(true)));
        seen.insert("stale_session".into());
    } else {
        assert!(v.get("stale_session").is_none());
    }
    // A refusal.
    match (v.get("error"), v.get("hint")) {
        (Some(Value::String(code)), Some(Value::String(hint))) => {
            assert!(
                r.text.contains(&format!("error: {code}\nhint: {hint}")),
                "{}",
                r.text
            );
            assert!(r.is_error);
            seen.insert("error".into());
            seen.insert("hint".into());
        }
        (None, None) => assert!(!r.is_error, "{}", r.text),
        other => panic!("error and hint come together: {other:?}"),
    }
    // The warnings.
    match lines.iter().find_map(|l| l.strip_prefix("warnings: ")) {
        Some(w) => {
            let w: Value = serde_json::from_str(w).unwrap();
            assert_eq!(v.get("warnings"), Some(&w), "{}", r.text);
            seen.insert("warnings".into());
        }
        None => assert!(v.get("warnings").is_none(), "{}", r.text),
    }
    // 1.7 (§13): the notes a write delivered.
    if let Some(n) = lines.iter().find_map(|l| l.strip_prefix("notes: ")) {
        let n: Value = serde_json::from_str(n).unwrap();
        assert_eq!(v.get("notes"), Some(&n), "{}", r.text);
        seen.insert("notes".into());
    } else {
        assert!(v.get("notes").is_none(), "{}", r.text);
    }
    // The next calls: as many items as the text's block shows; `end` with none (1.7).
    let block = lines
        .iter()
        .position(|l| l.starts_with("next_calls: none") || l.starts_with("next_calls ("));
    if let Some(end) = lines
        .iter()
        .find_map(|l| l.strip_prefix("next_calls: none — end: "))
    {
        assert_eq!(v["next_calls"]["end"], json!(end), "{}", r.text);
    } else if let Some(nc) = v.get("next_calls") {
        assert!(nc.get("end").is_none(), "{}", r.text);
    }
    match block {
        Some(i) => {
            let n = v["next_calls"]["items"].as_array().unwrap().len();
            let shown = lines
                .get(i + 1..)
                .unwrap_or_default()
                .iter()
                .take_while(|l| {
                    l.split_once(". ")
                        .is_some_and(|(k, _)| k.parse::<usize>().is_ok())
                })
                .count();
            assert_eq!(n, shown, "{}", r.text);
            seen.insert("next_calls".into());
        }
        None => assert!(v.get("next_calls").is_none(), "{}", r.text),
    }
    // The body: the first JSON object line that is not the hello's.
    if !r.is_error {
        let body = lines
            .iter()
            .skip(usize::from(seen.contains("status")))
            .filter(|l| !l.starts_with("hello: "))
            .find(|l| l.starts_with('{'))
            .unwrap_or_else(|| panic!("a body: {}", r.text));
        let body: Value = serde_json::from_str(body).unwrap();
        match v.get("body") {
            Some(b) => {
                assert_eq!(b, &body, "{}", r.text);
                let fields = body.as_object().unwrap();
                assert!(
                    fields.keys().any(|k| ENVELOPE_KEYS.contains(&k.as_str())),
                    "a body stands apart only when its fields would collide: {}",
                    r.text
                );
                seen.insert("body".into());
            }
            None => {
                for (k, x) in body.as_object().unwrap() {
                    assert_eq!(v.get(k), Some(x), "{k}: {}", r.text);
                    seen.insert(k.clone());
                }
            }
        }
    }
    // `control export` adds its JSON lines.
    if v.contains_key("lines") {
        seen.insert("lines".into());
    }
    let extra: Vec<&String> = v.keys().filter(|k| !seen.contains(*k)).collect();
    assert!(
        extra.is_empty(),
        "keys the text lacks: {extra:?}: {}",
        r.text
    );
    in_section_20_1_order(r);
}

/// The keys of a JSON object, in order.
fn keys_of(v: &Value) -> Vec<String> {
    v.as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// 1.5 (§20.1, §20.3, R3, D.11.7): the structured content's keys come in §20.1's order, `status`,
/// `stale_session`, `error`, `hint`, `warnings`, `next_calls`, `hello`, then the body's fields in
/// the text's order (or `body`), as serialised; and a `hello` starts with `session`.
fn in_section_20_1_order(r: &Response) {
    let keys = keys_of(&r.structured);
    let envelope: Vec<&String> = keys
        .iter()
        .take_while(|k| ENVELOPE_KEYS.contains(&k.as_str()) && k.as_str() != "body")
        .collect();
    let rank = |k: &str| ENVELOPE_KEYS.iter().position(|e| *e == k).unwrap();
    assert!(
        envelope.windows(2).all(|w| rank(w[0]) < rank(w[1])),
        "the envelope in §20.1's order: {keys:?}"
    );
    let rest: Vec<&String> = keys
        .iter()
        .skip(envelope.len())
        .filter(|k| k.as_str() != "lines")
        .collect();
    assert!(
        rest.iter()
            .all(|k| k.as_str() == "body" || !ENVELOPE_KEYS.contains(&k.as_str())),
        "every envelope key before the body: {keys:?}"
    );
    if !r.is_error && !rest.iter().any(|k| k.as_str() == "body") {
        let lines: Vec<&str> = r.text.lines().collect();
        let body = lines
            .iter()
            .skip(usize::from(r.structured.get("status").is_some()))
            .filter(|l| !l.starts_with("hello: "))
            .find(|l| l.starts_with('{'))
            .unwrap();
        let body: Value = serde_json::from_str(body).unwrap();
        let want = keys_of(&body);
        assert_eq!(
            rest,
            want.iter().collect::<Vec<_>>(),
            "the body's fields in the text's order"
        );
    }
    // The serialised form keeps the order: a client reads it from the top.
    let wire = serde_json::to_string(&r.structured).unwrap();
    let mut at = 0;
    for k in &keys {
        let found = wire[at..]
            .find(&format!("{}:", serde_json::to_string(k).unwrap()))
            .map(|i| i + at);
        assert!(found.is_some(), "{k} after byte {at}: {wire}");
        at = found.unwrap();
    }
    // A `hello`, prepended or as the body of `session hello`, starts with the session's id, the
    // one the status line names.
    let hello_fields = if r.structured.get("hello").is_some() {
        keys_of(&r.structured["hello"])
    } else if !r.is_error && rest.iter().any(|k| k.as_str() == "session") {
        rest.iter().map(|k| (*k).clone()).collect()
    } else {
        Vec::new()
    };
    if !hello_fields.is_empty() {
        assert_eq!(
            hello_fields.first().map(String::as_str),
            Some("session"),
            "{keys:?}"
        );
        let h = r.structured.get("hello").unwrap_or(&r.structured);
        let id = h["session"].as_str().unwrap();
        let line = r.structured["status"].as_str().unwrap();
        assert!(
            line.contains(&format!(" session {id} (")),
            "{line} names {id}"
        );
    }
}

/// Compares `actual` with the snapshot `name`, or writes a missing one and fails.
fn snapshot(name: &str, actual: &str) {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("src/render/snapshots");
    p.push(format!("{name}.txt"));
    match std::fs::read_to_string(&p) {
        Ok(want) => assert!(
            want == actual,
            "snapshot {name} differs:\n--- want\n{want}\n--- got\n{actual}"
        ),
        Err(_) => {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, actual).unwrap();
            panic!("snapshot {name} written: review it and run again");
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Snapshots of each response kind

/// A log of rendered responses, one section per call: the text content, and, apart, the
/// structured content (1.5, §20.1).
#[derive(Default)]
struct Log(String, String);

impl Log {
    fn add(&mut self, title: &str, r: &Response) {
        carries_the_text(r);
        self.0
            .push_str(&format!("== {title} (is_error: {})\n", r.is_error));
        self.0.push_str(&r.text);
        self.0.push_str("\n\n");
        self.1.push_str(&format!("== {title}\n"));
        self.1
            .push_str(&serde_json::to_string_pretty(&r.structured).unwrap());
        self.1.push_str("\n\n");
    }
}

/// A request from a person outside the swarm on `node`, as `talk request` records it.
fn request_event(
    d: &mut crate::store::testkit::Draft,
    node: NodeId,
    by: SessionId,
    origin: &str,
    conv: &str,
    words: &str,
    who: &str,
) -> Eid {
    let eid = d.s.counters.next_eid;
    let ev = Ev::Appended {
        eid,
        node,
        seq: Seq(d.s.nodes[&node].seq.0 + 1),
        by: Author::Session(by),
        body: EntryBody::Request {
            origin: Key {
                key: keystr(origin),
                nv: NormVer(1),
            },
            conv: keystr(conv),
            from_user: None,
            requester: Requester::External,
        },
        text: text(words, &[(TextField::Requester, who)]),
    };
    d.push(ev);
    eid
}

/// The work of one session: hello, create with a key, a request from a person, a checkpoint with
/// an intent, the views of the node, a second session refused as full, an empty `next`, a pause,
/// and the close that leaves the request unsettled.
#[test]
fn a_working_session_renders_each_response() {
    let mut k = Kit::new();
    let mut log = Log::default();
    let (s1, c) = k.hello(
        "coder",
        json!({"action": "hello", "intent": "fix the parser", "caps": ["rust"]}),
    );
    log.add("session hello", &c.response);
    k.at(60_000);
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/work", "keys": ["chat:team/1"], "title": "Fix the parser",
               "body": "The parser drops the last ticket of a page.", "est": 30}),
    );
    log.add("node create", &c.response);
    let work = k.node("/work");
    let token = match &ok(&c).reply {
        Reply::Created { token: Some(t), .. } => *t,
        other => panic!("{other:?}"),
    };
    // Bob's request in the conversation, recorded as `talk request` will record it.
    k.at(60_000);
    k.events(|d| {
        request_event(
            d,
            work,
            s1,
            "chat:team/1/m1",
            "chat:team/1",
            "why did yesterday's summary miss two tickets?",
            "Bob",
        );
    });
    k.at(120_000);
    let c = k.call(
        Some(s1),
        "hold",
        json!({"action": "checkpoint", "token": token.render(), "done": "found the off-by-one",
               "next_step": "rerun the summary", "intent": "post the fix to the team thread"}),
    );
    log.add("hold checkpoint with an intent", &c.response);
    k.at(60_000);
    let c = k.call(Some(s1), "view", json!({"action": "node", "path": "/work"}));
    log.add("view node (brief)", &c.response);
    let c = k.call(
        Some(s1),
        "view",
        json!({"action": "node", "path": "/work", "view": "review"}),
    );
    log.add("view node (review)", &c.response);
    let c = k.call(Some(s1), "view", json!({"action": "feed", "at": "/work"}));
    log.add("view feed", &c.response);
    let c = k.call(
        Some(s1),
        "view",
        json!({"action": "history", "path": "/work", "limit": 3}),
    );
    log.add("view history", &c.response);
    let c = k.call(Some(s1), "view", json!({"action": "list", "under": "/"}));
    log.add("view list", &c.response);
    let c = k.call(
        Some(s1),
        "view",
        json!({"action": "find", "key": "chat:team/1"}),
    );
    log.add("view find by key", &c.response);
    let c = k.call(
        Some(s1),
        "view",
        json!({"action": "requests", "requester": "all"}),
    );
    log.add("view requests", &c.response);
    // A second session of another class: hello is prepended to its first call, which is refused.
    k.at(60_000);
    let c = k.call_as(
        "reviewer",
        None,
        "hold",
        json!({"action": "take", "target": "/work"}),
        None,
    );
    log.add("first call refused as full, with hello", &c.response);
    let s2 = SessionId(s1.0 + 1);
    let c = k.call_as(
        "reviewer",
        Some(s2),
        "hold",
        json!({"action": "next"}),
        None,
    );
    log.add("hold next, empty", &c.response);
    let c = k.call_as(
        "reviewer",
        Some(s2),
        "control",
        json!({"action": "pause", "scope": "/work", "reason": "a release is being cut"}),
        None,
    );
    log.add("control pause", &c.response);
    k.at(60_000);
    let c = k.call(
        Some(s1),
        "view",
        json!({"action": "find", "key": "chat:team/2"}),
    );
    log.add("a call while a pause covers the hold", &c.response);
    let c = k.call_as(
        "reviewer",
        Some(s2),
        "control",
        json!({"action": "unpause", "scope": "/work"}),
        None,
    );
    log.add("control unpause", &c.response);
    // An offered node, which next hands to the reviewer as a hold; a release, and a take.
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/later", "title": "Tidy the parser tests",
               "tags": ["offer"], "take": false, "est": 20}),
    );
    log.add("node create without a hold", &c.response);
    let later = k.node("/later");
    let c = k.call_as(
        "reviewer",
        Some(s2),
        "hold",
        json!({"action": "next"}),
        None,
    );
    log.add("hold next, a hold", &c.response);
    let lt = match &ok(&c).reply {
        Reply::Next(NextReply::Hold { token, .. }) => *token,
        other => panic!("{other:?}"),
    };
    let c = k.call_as(
        "reviewer",
        Some(s2),
        "hold",
        json!({"action": "release", "token": lt.render(), "done": "read the tests",
               "next_step": "split the slow ones out", "reason": "handoff"}),
        None,
    );
    log.add("hold release", &c.response);
    let c = k.call_as(
        "reviewer",
        Some(s2),
        "hold",
        json!({"action": "take", "target": "/later"}),
        None,
    );
    log.add("hold take", &c.response);
    k.at(60_000);
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "update", "path": "/work", "version": 1, "status": "fixing",
               "token": token.render()}),
    );
    log.add("node update", &c.response);
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "update", "path": "/work", "version": 1, "status": "rerunning",
               "token": token.render()}),
    );
    log.add("node update with a stale version", &c.response);
    let c = k.call(
        Some(s1),
        "hold",
        json!({"action": "renew", "token": token.render(), "why": "the rerun is slow"}),
    );
    log.add("hold renew", &c.response);
    let c = k.call(
        Some(s1),
        "hold",
        json!({"action": "check", "target": "/work", "token": token.render()}),
    );
    log.add("hold check", &c.response);
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "close", "path": "/work", "outcome": "done", "summary": "fixed the off-by-one",
               "token": token.render()}),
    );
    log.add("node close with an unsettled request", &c.response);
    // 1.9: a word of the query that no document has, corrected (D.11.12).
    k.hits = SearchHits {
        related: Hits {
            nodes: vec![later, work],
            partial: true,
        },
        knowledge: Hits::default(),
        matched: vec!["parser for parsr".into()],
    };
    let c = k.call(Some(s1), "view", json!({"action": "find", "text": "parsr"}));
    log.add("view find by text", &c.response);
    k.hits = SearchHits::default();
    let c = k.call(
        Some(s1),
        "view",
        json!({"action": "node", "path": "chat:team/1"}),
    );
    log.add("a key where a node goes", &c.response);
    let c = k.call(
        Some(SessionId(99)),
        "view",
        json!({"action": "constitution"}),
    );
    log.add("an unknown session", &c.response);
    let c = k.call(Some(s1), "control", json!({"action": "health"}));
    log.add("control health", &c.response);
    let c = k.call(Some(s1), "view", json!({"action": "constitution"}));
    log.add("view constitution", &c.response);
    let c = k.call(Some(s1), "control", json!({"action": "constitution"}));
    log.add("control constitution", &c.response);
    // A series and its first occurrence (WP-D6; 1.5's stub of it is gone); a body longer than the
    // brief's head says how much was cut (1.5, R3); an expired hold's note says what the server
    // knows.
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/daily", "title": "Daily summary",
               "repeat": {"cron": "0 9 * * *", "run_within": "1h"}}),
    );
    log.add("node create with repeat", &c.response);
    let body: String = (1..=50)
        .map(|i| format!("Step {i}: rerun the parser on page {i} and record the count. "))
        .collect();
    ok(&k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/long", "title": "Recount every page", "body": body,
               "take": false}),
    ));
    let (taker, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(taker),
        "hold",
        json!({"action": "take", "target": "/long"}),
    );
    log.add(
        "hold take, a body longer than the brief's head",
        &c.response,
    );
    let lease = k.w.s.params.lease_first.0;
    k.at(lease + 60_000);
    assert!(k.w.catch_up(k.w.clock));
    let (reader, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(reader),
        "view",
        json!({"action": "node", "path": "/long"}),
    );
    log.add(
        "view node after a hold expired without a checkpoint",
        &c.response,
    );
    snapshot("working_session", &log.0);
    snapshot("working_session.structured", &log.1);
}

/// Appends an entry by `by` on `node` with `body` and `words`, and returns its id.
fn append(
    d: &mut crate::store::testkit::Draft,
    node: NodeId,
    by: SessionId,
    body: EntryBody,
    words: EntryText,
) -> Eid {
    let eid = d.s.counters.next_eid;
    let ev = Ev::Appended {
        eid,
        node,
        seq: Seq(d.s.nodes[&node].seq.0 + 1),
        by: Author::Session(by),
        body,
        text: words,
    };
    d.push(ev);
    eid
}

/// A research forum: a proof with three steps (one done, one failed, one open and held), a lemma
/// in `kb/` it depends on and derives from, a decision on the proof whose basis cites the lemma
/// and an outside survey, and the lemma closed `failed` afterwards, so the decision's basis is
/// stale. Renders the brief with the stale basis and the rollup, `view node {view: full}` of the
/// lemma with what rests on it, and `view list` with the subtree counts (CS-22, CS-32).
#[test]
fn a_research_forum_renders_decisions_backlinks_and_rollups() {
    let mut k = Kit::new();
    let mut log = Log::default();
    let (s1, _) = k.hello(
        "coder",
        json!({"action": "hello", "intent": "prove the theorem"}),
    );
    let mk = |k: &mut Kit, args: Value| {
        let c = k.call(Some(s1), "node", args);
        ok(&c).reply.clone()
    };
    mk(
        &mut k,
        json!({"action": "create", "path": "/kb/lemma-3", "title": "Lemma 3 holds for every n",
               "body": "By induction on n.", "take": false}),
    );
    let lemma = k.node("/kb/lemma-3");
    let proof_token = match mk(
        &mut k,
        json!({"action": "create", "path": "/proof", "title": "The main theorem",
               "digest": "Three steps; step 2 refuted, step 3 open.", "depends_on": ["/kb/lemma-3"]}),
    ) {
        Reply::Created { token: Some(t), .. } => t,
        other => panic!("{other:?}"),
    };
    let proof = k.node("/proof");
    for (name, title) in [
        ("step-1", "Reduce to the bounded case"),
        ("step-2", "Bound the error term"),
        ("step-3", "Close the gap"),
    ] {
        mk(
            &mut k,
            json!({"action": "create", "path": format!("/proof/{name}"), "title": title, "take": false}),
        );
    }
    k.at(60_000);
    mk(
        &mut k,
        json!({"action": "close", "path": "/proof/step-1", "outcome": "done", "summary": "reduced"}),
    );
    mk(
        &mut k,
        json!({"action": "close", "path": "/proof/step-2", "outcome": "failed",
               "summary": "the error term is unbounded for n = 7"}),
    );
    // The proof derives from the lemma; a decision on the proof cites it and an outside survey.
    k.at(60_000);
    let survey_at = k.w.clock;
    let decision = k.w.s.counters.next_eid;
    k.events(|d| {
        d.push(Ev::LinkAdded {
            node: proof,
            rel: LinkRel::DerivedFrom,
            to: lemma,
        });
        append(
            d,
            proof,
            s1,
            EntryBody::Decision {
                settles: BTreeSet::new(),
                supersedes: None,
                assumes: BTreeSet::new(),
                evidence: None,
                basis: vec![
                    BasisRef::Node {
                        id: lemma,
                        version: Some(Version(1)),
                        life_at: None,
                    },
                    BasisRef::External {
                        checked_at: survey_at,
                        recheck: Some(survey_at.plus(Dur(30 * 86_400_000))),
                    },
                ],
            },
            text(
                "go on with step 3 by the lemma",
                &[(TextField::Basis, "the 2025 survey of bounds")],
            ),
        );
    });
    let c = k.call(
        Some(s1),
        "view",
        json!({"action": "node", "path": "/proof"}),
    );
    log.add("the brief, before the lemma failed", &c.response);
    // The lemma is refuted: the decision's basis goes stale (D-4).
    k.at(60_000);
    mk(
        &mut k,
        json!({"action": "close", "path": "/kb/lemma-3", "outcome": "failed",
               "summary": "fails for n = 7"}),
    );
    let c = k.call(
        Some(s1),
        "view",
        json!({"action": "node", "path": "/proof"}),
    );
    log.add("the brief with a stale basis and the rollup", &c.response);
    assert!(
        c.response.text.contains("now closed failed"),
        "{}",
        c.response.text
    );
    let c = k.call(
        Some(s1),
        "view",
        json!({"action": "node", "path": "/kb/lemma-3", "view": "full"}),
    );
    log.add("view node full with what rests on it", &c.response);
    let c = k.call(
        Some(s1),
        "view",
        json!({"action": "list", "under": "/proof"}),
    );
    log.add("view list with the subtree counts", &c.response);
    let c = k.call(
        Some(s1),
        "view",
        json!({"action": "list", "under": "/proof", "closed": true}),
    );
    log.add("view list of closed nodes", &c.response);
    // A later attempt at the lemma continues the refuted one (§7): its brief names the
    // predecessor with its summary.
    mk(
        &mut k,
        json!({"action": "create", "path": "/kb/lemma-3b", "title": "Lemma 3 for n other than 7",
               "take": false}),
    );
    let again = k.node("/kb/lemma-3b");
    k.events(|d| {
        d.push(Ev::LinkAdded {
            node: again,
            rel: LinkRel::Continues,
            to: lemma,
        });
    });
    let c = k.call(
        Some(s1),
        "view",
        json!({"action": "node", "path": "/kb/lemma-3b"}),
    );
    log.add("a brief with a predecessor", &c.response);
    assert!(c.response.text.contains("\"summary\":\"fails for n = 7\""));
    let _ = (proof_token, decision);
    snapshot("research_forum", &log.0);
}

/// The ids of the people side of `people_forum`.
struct People {
    worker: SessionId,
    assistant: SessionId,
    work: NodeId,
    request: Eid,
    said: Eid,
    ack: Eid,
    heard: Eid,
    pq: Eid,
    objection: Eid,
    from_user: Eid,
    uq: Eid,
    digest: Eid,
    note: Eid,
    aq: Eid,
    answer: Eid,
    decision: Eid,
}

/// A forum with people and a human: a worker holding a node with a conversation key, Bob's
/// request, an ack and a reply, a question to Bob and his answer, an objection; an assistant
/// that serves alice, her message, a question for her, a digest entry, a note, and an agent's
/// question and answer.
fn people_forum(k: &mut Kit) -> People {
    let (worker, _) = k.hello("coder", json!({"action": "hello", "intent": "answer Bob"}));
    let (assistant, _) = k.hello(
        "coder",
        json!({"action": "hello", "principal": "mail:alice@example.org"}),
    );
    k.at(60_000);
    let c = k.call(
        Some(worker),
        "node",
        json!({"action": "create", "path": "/support/summary-gap", "keys": ["chat:team/7"],
               "title": "Two tickets missing from the summary"}),
    );
    ok(&c);
    let work = k.node("/support/summary-gap");
    let alice = k.w.s.sessions[&assistant]
        .principals
        .iter()
        .copied()
        .next()
        .expect("the assistant serves alice");
    k.at(60_000);
    let mut ids = BTreeMap::new();
    k.events(|d| {
        let conv = keystr("chat:team/7");
        let r = request_event(
            d,
            work,
            worker,
            "chat:team/7/m1",
            "chat:team/7",
            "why did yesterday's summary miss two tickets?",
            "Bob",
        );
        ids.insert("request", r);
        let ack = append(
            d,
            work,
            worker,
            EntryBody::Said {
                conv: conv.clone(),
                kind: SaidKind::Ack,
                corrects: None,
                voice: false,
            },
            text("on it", &[]),
        );
        ids.insert("ack", ack);
        let followup = d.clock.plus(d.s.params.commitment_followup);
        d.push(Ev::CommitmentOpened {
            c: ack,
            node: work,
            conv: conv.clone(),
            followup_at: followup,
        });
        let said = append(
            d,
            work,
            worker,
            EntryBody::Said {
                conv: conv.clone(),
                kind: SaidKind::Reply,
                corrects: None,
                voice: false,
            },
            text("the parser dropped them; fixed", &[]),
        );
        ids.insert("said", said);
        d.push(Ev::RequestAnswered {
            request: r,
            by: said,
        });
        let pq_said = append(
            d,
            work,
            worker,
            EntryBody::Said {
                conv: conv.clone(),
                kind: SaidKind::Question,
                corrects: None,
                voice: false,
            },
            text("which tickets?", &[]),
        );
        let recheck = d.clock.plus(d.s.params.question_recheck);
        let pq = append(
            d,
            work,
            worker,
            EntryBody::Question(QuestionBody::People {
                conv: conv.clone(),
                blocking: true,
                said: pq_said,
                supersedes: None,
                recheck_at: recheck,
            }),
            text("which tickets?", &[]),
        );
        ids.insert("pq", pq);
        let heard = append(
            d,
            work,
            worker,
            EntryBody::Heard {
                conv: conv.clone(),
                answers: None,
            },
            text("PROJ-12 and PROJ-19", &[(TextField::From, "Bob")]),
        );
        ids.insert("heard", heard);
        let objection = append(
            d,
            work,
            worker,
            EntryBody::Object,
            text("the fix skips the last page", &[]),
        );
        ids.insert("objection", objection);
        let decision = append(
            d,
            work,
            worker,
            EntryBody::Decision {
                settles: [r].into(),
                supersedes: None,
                assumes: BTreeSet::new(),
                evidence: None,
                basis: Vec::new(),
            },
            text("the parser fix answers it", &[]),
        );
        ids.insert("decision", decision);
        // Alice's message, a question for her about the work, and a digest entry for her.
        let fu = append(
            d,
            USER,
            assistant,
            EntryBody::FromUser { principal: alice },
            text(
                "please make the daily summary complete",
                &[(TextField::Client, "ide")],
            ),
        );
        ids.insert("from_user", fu);
        let due = d.clock.plus(d.s.params.user_question_due);
        let uq = append(
            d,
            work,
            worker,
            EntryBody::Question(QuestionBody::User {
                source: UqSource::Agent {
                    session: worker,
                    digest: ReqDigest([7; 32]),
                },
                about: Some(work),
                to: vec![alice, USER_PRINCIPAL],
                due,
                has_default: true,
            }),
            text(
                "rerun the summary for last week too?",
                &[
                    (TextField::Option, "yes"),
                    (TextField::Option, "no"),
                    (TextField::Recommendation, "yes"),
                    (TextField::Default, "no"),
                ],
            ),
        );
        ids.insert("uq", uq);
        let dg = append(
            d,
            USER_DIGEST,
            worker,
            EntryBody::System(SystemNote::Digest {
                item: DigestItem::Assumption { question: pq },
                to: alice,
            }),
            text("", &[]),
        );
        ids.insert("digest", dg);
        // A note from the worker to the assistant, and an agent question to the worker.
        let until = d.clock.plus(d.s.params.deliver_window);
        let note = append(
            d,
            work,
            worker,
            EntryBody::Note {
                to: Some(Addressee::Session(assistant)),
                until: Some(until),
                recipients: [assistant].into(),
                pending_holder: false,
                expects: None,
                reply_to: None,
            },
            text("Bob is answered; tell alice when the rerun is done", &[]),
        );
        ids.insert("note", note);
        let aq = append(
            d,
            work,
            assistant,
            EntryBody::Question(QuestionBody::Agent {
                to: Addressee::Session(worker),
                until,
                recipients: [worker].into(),
                pending_holder: false,
            }),
            text("is the rerun scheduled?", &[]),
        );
        ids.insert("aq", aq);
        let answer = append(
            d,
            work,
            worker,
            EntryBody::Answer {
                question: aq,
                recipient: assistant,
                until,
            },
            text("yes, at 07:00", &[]),
        );
        ids.insert("answer", answer);
    });
    People {
        worker,
        assistant,
        work,
        request: ids["request"],
        said: ids["said"],
        ack: ids["ack"],
        heard: ids["heard"],
        pq: ids["pq"],
        objection: ids["objection"],
        from_user: ids["from_user"],
        uq: ids["uq"],
        digest: ids["digest"],
        note: ids["note"],
        aq: ids["aq"],
        answer: ids["answer"],
        decision: ids["decision"],
    }
}

/// Every reply of D.6.2 that the scenarios above do not produce through the core, rendered from
/// the reply as its command will return it; every warning of §21 in one response; `STALE
/// SESSION`; and the status line of a session that serves a human, with a red health.
#[test]
fn every_reply_and_warning_renders() {
    let mut k = Kit::new();
    let p = people_forum(&mut k);
    let mut log = Log::default();
    let view = Cmd::View(ViewCmd::Constitution);
    let w = p.worker;
    let a = p.assistant;
    let alice = k.w.s.sessions[&a]
        .principals
        .iter()
        .copied()
        .next()
        .unwrap();
    // The assistant's hello: its principal, the question and the digest entry for alice, its
    // inbox, and alice's message logged with it.
    let c = k.call(Some(a), "session", json!({"action": "hello"}));
    log.add("hello of a session that serves a human", &c.response);
    let a_holds = super::hello::live_holds(&k.w.s, a, k.w.clock);
    let user_entry = UserEntryReply {
        entry: p.from_user,
        key: KeyStr::of_user_entry(p.from_user),
        routed: routed_of(&k.w.s, p.from_user, a_holds),
        principal: alice,
    };
    // Two more of alice's messages: one routed to the work as a request, one to none, as `talk
    // route` will route them (WP-D5a).
    let (routed_to_node, routed_to_none) = route_two(&mut k, &p, alice);
    // A node closed as failed, which a take or a next reopens (§10.1, §20.6).
    let c = k.call(
        Some(w),
        "node",
        json!({"action": "create", "path": "/support/old-attempt", "title": "An earlier attempt",
               "take": false}),
    );
    ok(&c);
    let c = k.call(
        Some(w),
        "node",
        json!({"action": "close", "path": "/support/old-attempt", "outcome": "failed",
               "summary": "the parser was not the cause"}),
    );
    ok(&c);
    let old = k.node("/support/old-attempt");
    let old_token = Token {
        node: old,
        fence: Fence(1),
    };
    let old_lease = k.w.clock.plus(Dur(900_000));
    let route = |entry: Eid, to: Option<&str>, reason: &str| {
        Cmd::Talk(TalkCmd::Route {
            entry,
            to: Some(
                (to.map(|x| crate::decode::values::node(x).unwrap()))
                    .map_or(forum_core::RouteTo::Nowhere, forum_core::RouteTo::Node),
            ),
            answers: None,
            clarifies: None,
            reason: Some(t(reason)),
        })
    };
    let replies: Vec<(&str, SessionId, Cmd, Reply)> = vec![
        (
            "session hello {from_user}",
            a,
            view.clone(),
            Reply::Hello {
                session: a,
                user_entry: Some(user_entry.clone()),
                inbox: vec![p.note],
            },
        ),
        ("session status", w, view.clone(), Reply::Status),
        (
            "view node of two nodes",
            w,
            view.clone(),
            Reply::ViewNodes {
                nodes: vec![p.work, ROOT],
                view: ViewMode::Brief,
                page: 0,
            },
        ),
        (
            "view user_log",
            a,
            view.clone(),
            Reply::ViewUserLog {
                since: None,
                limit: 50,
            },
        ),
        (
            "node create, joined",
            w,
            view.clone(),
            Reply::Joined {
                existing: p.work,
                holders: vec![w],
                open_question: Some(p.pq),
                added_keys: vec![keystr("issue:proj#7")],
                request: None,
                user_requests: Vec::new(),
            },
        ),
        (
            "node link",
            w,
            view.clone(),
            Reply::Linked {
                existing: false,
                signal: None,
            },
        ),
        (
            "node key add",
            w,
            view.clone(),
            Reply::KeyAdded {
                already: false,
                user_request: None,
            },
        ),
        (
            "node key add, carried by another node",
            w,
            view.clone(),
            Reply::KeyExisting {
                existing: p.work,
                signal: None,
            },
        ),
        ("node key remove", w, view.clone(), Reply::KeyRemoved),
        (
            "node reopen",
            w,
            view.clone(),
            Reply::Reopened {
                node: p.work,
                dropped_keys: vec![keystr("chat:team/9")],
                // 1.12 (§16): the reopen tagged it offer.
                offered: true,
            },
        ),
        ("node move", w, view.clone(), Reply::Moved { node: p.work }),
        (
            "node merge",
            w,
            view.clone(),
            Reply::Merged {
                from: NodeId(42),
                into: p.work,
            },
        ),
        ("node hide", w, view.clone(), Reply::Hidden),
        (
            "talk from_user",
            a,
            view.clone(),
            Reply::FromUser {
                entry: p.from_user,
                key: KeyStr::of_user_entry(p.from_user),
                routed: routed_of(&k.w.s, p.from_user, a_holds),
                principal: alice,
            },
        ),
        (
            "talk to_user",
            a,
            view.clone(),
            Reply::ToUser {
                entry: Eid(900),
                principal: alice,
                settled: vec![],
                shown_digest: vec![p.digest],
                // 1.8 (§8.1): the requests `at` named.
                re: vec![p.request],
            },
        ),
        (
            "talk route, existing",
            a,
            view.clone(),
            Reply::Routed {
                entry: p.from_user,
                routed: routed_of(&k.w.s, p.from_user, a_holds),
                existing: true,
            },
        ),
        (
            "talk request",
            w,
            view.clone(),
            Reply::Request {
                request: p.request,
                node: p.work,
                existing: false,
            },
        ),
        (
            "talk said",
            w,
            view.clone(),
            Reply::Said {
                said: p.said,
                node: p.work,
                answered: vec![],
                settled: vec![],
                fulfilled: vec![],
                // 1.12 (§9.1): it gave the node its conversation key.
                keyed: Some(keystr("chat:team/9")),
            },
        ),
        (
            "talk said {said_id}",
            w,
            Cmd::Talk(TalkCmd::SaidConfirm {
                said: p.said,
                confirmed: t("https://chat.example/team/7/m3"),
            }),
            Reply::SaidConfirmed { already: false },
        ),
        (
            "talk heard",
            w,
            view.clone(),
            Reply::Heard {
                entry: p.heard,
                node: p.work,
                answered: Some(p.pq),
                keyed: None,
            },
        ),
        (
            "talk ask, existing",
            w,
            view.clone(),
            Reply::Asked {
                question: p.pq,
                said: None,
                node: p.work,
                existing: true,
                keyed: None,
            },
        ),
        (
            "talk ask agents",
            a,
            view.clone(),
            Reply::Asked {
                question: p.aq,
                said: None,
                node: p.work,
                existing: false,
                keyed: None,
            },
        ),
        (
            "talk ask {to: principal:<key>}",
            w,
            view.clone(),
            Reply::Asked {
                question: p.uq,
                said: None,
                node: p.work,
                existing: false,
                keyed: None,
            },
        ),
        (
            "talk answer",
            w,
            view.clone(),
            Reply::Answered { entry: p.answer },
        ),
        ("talk say", w, view.clone(), Reply::Say { entry: p.note }),
        (
            "talk decide",
            w,
            view.clone(),
            Reply::Decided {
                decision: p.decision,
                settled: vec![p.request],
                not_settled: vec![],
            },
        ),
        (
            "talk approve",
            w,
            view.clone(),
            Reply::Approved {
                entry: Eid(901),
                approvals: 2,
            },
        ),
        (
            "talk object",
            w,
            view.clone(),
            Reply::Objected {
                objection: p.objection,
            },
        ),
        (
            "talk resolve",
            w,
            view.clone(),
            Reply::ObjectionResolved {
                entry: Some(Eid(902)),
                corrected: None,
                already: None,
                disputed: None,
                withdrawn: None,
                deferred: None,
            },
        ),
        (
            "talk resolve, upheld on a closed node",
            w,
            view.clone(),
            Reply::ObjectionResolved {
                entry: Some(Eid(902)),
                corrected: Some(forum_core::Corrected {
                    node: p.work,
                    was: forum_core::Outcome::Done,
                    outcome: forum_core::Outcome::Other(forum_core::Label::new("refuted").unwrap()),
                }),
                already: None,
                disputed: None,
                withdrawn: None,
                deferred: None,
            },
        ),
        (
            "talk resolve, already",
            w,
            view.clone(),
            Reply::ObjectionResolved {
                entry: None,
                corrected: None,
                already: None,
                disputed: None,
                withdrawn: None,
                deferred: None,
            },
        ),
        // 1.10 (§11.3): the same verdict again names the one that decided it.
        (
            "talk resolve, already with its verdict",
            w,
            view.clone(),
            Reply::ObjectionResolved {
                entry: None,
                corrected: None,
                already: Some(forum_core::Verdict {
                    verdict: p.objection,
                    upheld: false,
                }),
                disputed: None,
                withdrawn: None,
                deferred: None,
            },
        ),
        // 1.10 (§11.3): an opposite verdict after one that kept the outcome.
        (
            "talk resolve, a second verdict",
            w,
            view.clone(),
            Reply::ObjectionResolved {
                entry: Some(Eid(905)),
                corrected: None,
                already: None,
                disputed: Some(forum_core::Disputed {
                    objection: Eid(905),
                    node: p.work,
                    disputes: p.objection,
                    earlier: forum_core::Verdict {
                        verdict: Eid(902),
                        upheld: false,
                    },
                }),
                withdrawn: None,
                deferred: None,
            },
        ),
        // 1.10 (§11.3): an opposite verdict after an upheld one, whose correction it withdraws.
        (
            "talk resolve, a second verdict that withdraws a correction",
            w,
            view.clone(),
            Reply::ObjectionResolved {
                entry: Some(Eid(905)),
                corrected: None,
                already: None,
                disputed: Some(forum_core::Disputed {
                    objection: Eid(905),
                    node: p.work,
                    disputes: p.objection,
                    earlier: forum_core::Verdict {
                        verdict: Eid(902),
                        upheld: true,
                    },
                }),
                withdrawn: Some(forum_core::Corrected {
                    node: p.work,
                    was: forum_core::Outcome::Other(forum_core::Label::new("refuted").unwrap()),
                    outcome: forum_core::Outcome::Done,
                }),
                deferred: None,
            },
        ),
        // 1.11 (§11.3): a deferral of a closed node's review, and of an open node's objection.
        (
            "talk resolve {defer}, on a closed node",
            w,
            view.clone(),
            Reply::ObjectionResolved {
                entry: Some(Eid(906)),
                corrected: None,
                already: None,
                disputed: None,
                withdrawn: None,
                deferred: Some(forum_core::Deferred {
                    until: Some(Time(1_790_003_720_000)),
                }),
            },
        ),
        (
            "talk resolve {defer}, on an open node",
            w,
            view.clone(),
            Reply::ObjectionResolved {
                entry: Some(Eid(906)),
                corrected: None,
                already: None,
                disputed: None,
                withdrawn: None,
                deferred: Some(forum_core::Deferred { until: None }),
            },
        ),
        (
            "hold check, intent_done",
            w,
            view.clone(),
            Reply::HoldCheck {
                node: p.work,
                token_current: None,
                entry: Some(Eid(904)),
                intent_done: Some(Eid(903)),
            },
        ),
        (
            "talk retract",
            w,
            view.clone(),
            // 1.12 (§9.3): a retract of an open question to a person withdraws it.
            Reply::Retracted {
                entry: Eid(903),
                withdrawn: Some(p.pq),
            },
        ),
        (
            "talk wait, timeout",
            w,
            view.clone(),
            Reply::Waited {
                delivered: vec![],
                news: None,
            },
        ),
        (
            "talk wait, delivered",
            a,
            view.clone(),
            Reply::Waited {
                delivered: vec![p.note, p.answer],
                news: Some((p.work, Seq(9))),
            },
        ),
        ("talk who", w, view.clone(), Reply::Who { at: None }),
        (
            "talk who {at}",
            w,
            view.clone(),
            Reply::Who { at: Some(p.work) },
        ),
        (
            "control export",
            w,
            view.clone(),
            Reply::Export {
                after: Some(10),
                limit: 1_000,
            },
        ),
        (
            "talk route, to a node",
            a,
            route(routed_to_node, Some("/support/summary-gap"), "a new ask"),
            Reply::Routed {
                entry: routed_to_node,
                routed: routed_of(&k.w.s, routed_to_node, a_holds),
                existing: false,
            },
        ),
        (
            "talk route, to none",
            a,
            route(routed_to_none, None, "a thank-you: nothing to do"),
            Reply::Routed {
                entry: routed_to_none,
                routed: RoutedReply::NoAction,
                existing: false,
            },
        ),
        (
            "talk route, to none, existing: the first routing's reason",
            a,
            route(routed_to_none, None, "another reason, which loses"),
            Reply::Routed {
                entry: routed_to_none,
                routed: RoutedReply::NoAction,
                existing: true,
            },
        ),
        (
            "talk from_user {answers}, a question no longer pending",
            a,
            Cmd::Talk(TalkCmd::FromUser {
                text: t("yes"),
                client: t("ide"),
                principal: None,
                answers: Some(p.uq),
                to: None,
                clarifies: None,
                reason: None,
            }),
            Reply::FromUser {
                entry: p.from_user,
                key: KeyStr::of_user_entry(p.from_user),
                routed: RoutedReply::Unrouted(UnroutedWhy::NotPending { question: p.uq }),
                principal: alice,
            },
        ),
        (
            "hold take, reopening a closed node",
            w,
            view.clone(),
            Reply::Taken {
                node: old,
                token: old_token,
                lease_until: old_lease,
                reopened: Some(Outcome::Failed),
                existing: false,
            },
        ),
        (
            "hold next, a hold that reopened a closed node",
            w,
            view.clone(),
            Reply::Next(NextReply::Hold {
                node: old,
                token: old_token,
                lease_until: old_lease,
                why: WhyThis {
                    tier: 0,
                    flags: FlagSet::EMPTY.with(Flag::Objection),
                    aged: 2,
                    reason: Reason::ClosedFlagged,
                    no_estimate: false,
                    longer: false,
                    beyond_horizon: None,
                },
                reopened: Some(Outcome::Failed),
            }),
        ),
        ("admin set params", w, view.clone(), Reply::ParamsSet),
        ("control health, green", w, view.clone(), Reply::Health),
        (
            "hold next, a routing item",
            a,
            view.clone(),
            Reply::Next(NextReply::Route {
                entry: p.from_user,
                why: WhyThis {
                    tier: 0,
                    flags: FlagSet::EMPTY.with(Flag::Unrouted),
                    aged: 1,
                    reason: Reason::Routing,
                    no_estimate: false,
                    longer: false,
                    beyond_horizon: None,
                },
            }),
        ),
        (
            "hold next, a status item",
            a,
            view.clone(),
            Reply::Next(NextReply::Status {
                node: p.work,
                why: WhyThis {
                    tier: 0,
                    flags: FlagSet::EMPTY.with(Flag::OpenCommitment),
                    aged: 3,
                    reason: Reason::Status,
                    no_estimate: false,
                    longer: false,
                    beyond_horizon: None,
                },
            }),
        ),
        // 1.9 (§10.4): a closed node's objection, offered as a review with no hold.
        (
            "hold next, a review item",
            w,
            view.clone(),
            Reply::Next(NextReply::Review {
                node: old,
                objection: Eid(901),
                why: WhyThis {
                    tier: 1,
                    flags: FlagSet::EMPTY.with(Flag::Objection),
                    aged: 2,
                    reason: Reason::Review,
                    no_estimate: false,
                    longer: false,
                    beyond_horizon: None,
                },
            }),
        ),
        // 1.10 (§10.4): a node without `est`, offered under a horizon.
        (
            "hold next, a node without est under a horizon",
            w,
            view.clone(),
            Reply::Next(NextReply::Hold {
                node: old,
                token: old_token,
                lease_until: old_lease,
                why: WhyThis {
                    tier: 2,
                    flags: FlagSet::EMPTY,
                    aged: 5,
                    reason: Reason::Offered,
                    no_estimate: true,
                    longer: false,
                    beyond_horizon: None,
                },
                reopened: None,
            }),
        ),
        // The 1.4 shapes of D.6.2 (R3, R9, R15, D.14.5).
        (
            "view agreements",
            w,
            view.clone(),
            Reply::Agreements {
                items: vec![AgreementItem {
                    node: p.work,
                    approvals: 1,
                    changed: Some(k.w.clock),
                }],
                total: Total::Exact(1),
            },
        ),
        (
            "node create, with the request a user key's routing recorded",
            w,
            view.clone(),
            Reply::Created {
                id: p.work,
                token: None,
                lease_until: None,
                continues: vec![],
                request: None,
                user_requests: vec![p.request],
            },
        ),
        (
            "node key add, routing a user entry",
            w,
            view.clone(),
            Reply::KeyAdded {
                already: false,
                user_request: Some(p.request),
            },
        ),
        (
            "node close of a series",
            w,
            view.clone(),
            Reply::Closed {
                node: p.work,
                decision: None,
                settled: vec![],
                not_settled: vec![],
                dropped: Some(old),
                leave_better: vec![],
            },
        ),
        (
            "hold checkpoint, closing an intent",
            w,
            view.clone(),
            Reply::Checkpointed {
                entry: p.note,
                lease_until: old_lease,
                intent: None,
                intent_done: Some(p.said),
            },
        ),
        (
            "hold release, closing an intent",
            w,
            view.clone(),
            Reply::Released {
                intent_done: Some(p.said),
                leave_better: vec![],
                decision: None,
                settled: vec![],
                not_settled: vec![],
            },
        ),
        (
            // 1.12 (§10.3): a release with `settles` records a decision after it.
            "hold release, with settles",
            w,
            view.clone(),
            Reply::Released {
                intent_done: None,
                leave_better: vec![],
                decision: Some(Eid(905)),
                settled: vec![p.request],
                not_settled: vec![NotSettled {
                    request: Eid(906),
                    why: NotSettledWhy::AwaitsResult { requester: alice },
                }],
            },
        ),
        (
            "talk decide, with requests it did not settle",
            w,
            view.clone(),
            Reply::Decided {
                decision: p.decision,
                settled: vec![],
                not_settled: vec![
                    NotSettled {
                        request: p.request,
                        why: NotSettledWhy::SettledBefore,
                    },
                    NotSettled {
                        request: p.request,
                        why: NotSettledWhy::AwaitsResult { requester: alice },
                    },
                    NotSettled {
                        request: p.request,
                        why: NotSettledWhy::AwaitsSaid {
                            node: p.work,
                            conv: keystr("chat:team/7"),
                        },
                    },
                ],
            },
        ),
    ];
    for (title, sess, cmd, reply) in &replies {
        let r = k.render(Some(*sess), cmd, &done(reply.clone()));
        log.add(title, &r);
    }
    // Every warning of §21 in one response, with the shell's own `secret_masked`.
    let warnings = vec![
        Warning {
            code: WarningCode::Moved,
            id: Some(p.work),
            detail: WarningDetail::Moved {
                entries: vec![p.heard, p.request],
                total: Total::Exact(2),
            },
        },
        Warning {
            code: WarningCode::UnsettledRequests,
            id: Some(p.work),
            detail: WarningDetail::UnsettledRequests {
                requests: vec![p.request],
            },
        },
        Warning {
            code: WarningCode::OpenCommitments,
            id: Some(p.work),
            detail: WarningDetail::OpenCommitments {
                commitments: vec![p.ack],
            },
        },
        Warning {
            code: WarningCode::OpenObjections,
            id: Some(p.work),
            detail: WarningDetail::OpenObjections {
                objections: vec![p.objection],
            },
        },
        Warning {
            code: WarningCode::Agreement,
            id: Some(p.work),
            detail: WarningDetail::Agreement {
                sources: vec![ROOT, p.work],
                approvals: 1,
                content_version: Version(2),
            },
        },
        // 1.10 (§9.2): the `said` counts all the same.
        Warning {
            code: WarningCode::ConversationHeld,
            id: Some(p.work),
            detail: WarningDetail::ConversationHeld { holders: vec![w] },
        },
        Warning {
            code: WarningCode::HeldByOther,
            id: Some(p.work),
            detail: WarningDetail::HeldByOther {
                holders: vec![w],
                left: vec![],
            },
        },
        Warning {
            code: WarningCode::NoHold,
            id: Some(p.work),
            detail: WarningDetail::NoHold {
                why: NoHoldWhy::HoldsLimit,
            },
        },
        Warning {
            code: WarningCode::UncoveredSlot,
            id: Some(p.work),
            detail: WarningDetail::UncoveredSlot {
                covered_days: 3,
                days: 28,
                usual_hours: vec![9, 10, 14],
                first_start: None,
            },
        },
        Warning {
            code: WarningCode::DuplicateKey,
            id: Some(p.work),
            detail: WarningDetail::DuplicateKey {
                keys: vec![(keystr("chat:team/7"), p.work)],
            },
        },
        Warning {
            code: WarningCode::UserSaturated,
            id: None,
            detail: WarningDetail::UserSaturated {
                principal: alice,
                pending: 21,
            },
        },
        Warning {
            code: WarningCode::SessionOpened,
            id: None,
            detail: WarningDetail::SessionOpened { session: w },
        },
        // 1.4 (§12, §13, §15, §21, R2, R14, R15, R17, R18, re-run R33, R34).
        Warning {
            code: WarningCode::UncoveredSlot,
            id: Some(p.work),
            detail: WarningDetail::UncoveredSlot {
                covered_days: 3,
                days: 28,
                usual_hours: vec![9, 10, 14],
                first_start: Some(Time(1_790_845_200_000)),
            },
        },
        Warning {
            code: WarningCode::OpenIntent,
            id: Some(p.work),
            detail: WarningDetail::OpenIntent { intent: p.said },
        },
        Warning {
            code: WarningCode::ExpectsIgnored,
            id: Some(p.work),
            detail: WarningDetail::ExpectsIgnored {
                expects: Expects::Reply,
            },
        },
        Warning {
            code: WarningCode::SeriesStopped,
            id: Some(p.work),
            detail: WarningDetail::SeriesStopped { occurrence: old },
        },
        Warning {
            code: WarningCode::SelfBasisDropped,
            id: Some(p.work),
            detail: WarningDetail::SelfBasisDropped { dropped: 1 },
        },
        Warning {
            code: WarningCode::SupersededBasisDropped,
            id: Some(p.work),
            detail: WarningDetail::SupersededBasisDropped {
                decision: p.decision,
                dropped: 1,
            },
        },
        Warning {
            code: WarningCode::ResultAnsweredNothing,
            id: None,
            detail: WarningDetail::ResultAnsweredNothing { requests: vec![] },
        },
        Warning {
            code: WarningCode::ResultAnsweredNothing,
            id: None,
            detail: WarningDetail::ResultAnsweredNothing {
                requests: vec![p.request],
            },
        },
        Warning {
            code: WarningCode::Waiting,
            id: Some(p.work),
            detail: WarningDetail::Waiting {
                on: WaitOn::External(k.w.clock.plus(Dur(3_600_000))),
                set_at: k.w.clock,
                release: Some(Eid(906)),
            },
        },
        Warning {
            code: WarningCode::Waiting,
            id: Some(p.work),
            detail: WarningDetail::Waiting {
                on: WaitOn::Question {
                    q: p.pq,
                    rechecks_at_set: 0,
                },
                set_at: k.w.clock,
                release: None,
            },
        },
        Warning {
            code: WarningCode::UnnamedUser,
            id: Some(USER),
            detail: WarningDetail::UnnamedUser { entry: p.from_user },
        },
        // 1.8 (§5, §8.1, §8.4, §21).
        Warning {
            code: WarningCode::HeldByOther,
            id: Some(p.work),
            detail: WarningDetail::HeldByOther {
                holders: vec![w],
                left: vec![p.request],
            },
        },
        Warning {
            code: WarningCode::IgnoredArguments,
            id: Some(p.work),
            detail: WarningDetail::AtIgnored { node: p.work },
        },
        Warning {
            code: WarningCode::NoHumanInReach,
            id: None,
            detail: WarningDetail::NoHumanInReach {
                due: None,
                default: false,
            },
        },
        Warning {
            code: WarningCode::NoHumanInReach,
            id: None,
            detail: WarningDetail::NoHumanInReach {
                due: Some(k.w.clock.plus(Dur(3_600_000))),
                default: true,
            },
        },
        // 1.9 (§8.1, §10.1, §20.5, §21).
        Warning {
            code: WarningCode::ClientContext,
            id: Some(USER),
            detail: WarningDetail::ClientContext { entry: p.from_user },
        },
        Warning {
            code: WarningCode::UnmetNeeds,
            id: Some(p.work),
            detail: WarningDetail::UnmetNeeds {
                needs: vec![Label::new("exact-statement").unwrap()],
                take: false,
            },
        },
        Warning {
            code: WarningCode::UnmetNeeds,
            id: Some(p.work),
            detail: WarningDetail::UnmetNeeds {
                needs: vec![Label::new("lean").unwrap(), Label::new("swift").unwrap()],
                take: true,
            },
        },
        // 1.10 (§11.3).
        Warning {
            code: WarningCode::NoBasis,
            id: Some(p.work),
            detail: WarningDetail::NoBasis {
                node: p.work,
                decision: Eid(907),
            },
        },
        // 1.10 (§10.2, §19).
        Warning {
            code: WarningCode::Paused,
            id: Some(p.work),
            detail: WarningDetail::Paused {
                scope: p.work,
                until: k.w.clock.plus(Dur(3_600_000)),
            },
        },
        // 1.11 (§8.2, §21).
        Warning {
            code: WarningCode::FollowUp,
            id: Some(p.work),
            detail: WarningDetail::FollowUp {
                entry: p.from_user,
                request: p.request,
                node: p.work,
                requester: k.w.s.user_entries[&p.from_user].principal,
            },
        },
        // 1.12 (§8.4, §9.2, §13, §21).
        Warning {
            code: WarningCode::NoHumanInReach,
            id: None,
            detail: WarningDetail::NobodyServes {
                question: p.uq,
                chain: vec![alice, forum_core::USER_PRINCIPAL],
            },
        },
        Warning {
            code: WarningCode::NoRecipient,
            id: Some(p.work),
            detail: WarningDetail::NoRecipient {
                node: p.work,
                question: false,
            },
        },
        Warning {
            code: WarningCode::AddresseeGone,
            id: None,
            detail: WarningDetail::AddresseeGone {
                session: w,
                since: k.w.clock,
            },
        },
        Warning {
            code: WarningCode::AnsweredNothing,
            id: Some(p.work),
            detail: WarningDetail::AnsweredNothing {
                node: p.work,
                conv: keystr("chat:team/7"),
            },
        },
    ];
    let out = Ok(Done {
        reply: Reply::Status,
        warnings,
        opened: None,
        stale_session: true,
        delivered: Vec::new(),
    });
    let src = StoreSource::new(&k.w.conn);
    let empty = Guidance {
        items: Vec::new(),
        total: Total::Exact(0),
        end: None,
    };
    let r = response(&Input {
        state: &k.w.s,
        now: k.w.clock,
        session: Some(w),
        opened: None,
        cmd: &view,
        node_args: &[],
        out: &out,
        guidance: Some(&empty),
        health: &k.health,
        health_at: k.w.clock,
        hits: &k.hits,
        masked_fields: 2,
        ignored: None,
        src: &src,
        seen: k.seen.get(w),
        rebind: None,
    });
    log.add("every warning, STALE SESSION, and secret_masked", &r);
    // A red health with its first reason, a pause at the root, and a session that serves alice.
    k.health = Health {
        level: Level::Red,
        reasons: vec![HealthReason::Paused {
            level: Level::Red,
            count: 1,
            top: vec![PauseExclusion {
                pause: PauseView {
                    scope: ROOT,
                    by: w,
                    class: class("coder"),
                    until: k.w.clock.plus(Dur(3_600_000)),
                },
                excluded: 4,
            }],
        }],
        capacity: forum_core::health::CapacityReport::default(),
    };
    let r = k.render(Some(a), &view, &done(Reply::Status));
    log.add("a red health and a session that serves a human", &r);
    // `control health` with a reason of each kind, each with its count and first items (§17).
    k.health = every_reason(&k, &p, w);
    let r = k.render(Some(w), &view, &done(Reply::Health));
    log.add("control health, red, a reason of each kind", &r);
    // What the 1.4 shapes say (§8.2, §20.6, §17), beside the snapshot.
    let all = &log.0;
    assert_eq!(
        all.matches(r#""reopened":{"was":"failed"}"#).count(),
        2,
        "{all}"
    );
    assert_eq!(
        all.matches(r#""routed":{"none":"a thank-you: nothing to do"}"#)
            .count(),
        2,
        "the reason of this call, and of the first routing for an existing one"
    );
    assert!(all.contains(&format!(
        r#""routed":{{"unrouted":"the question {} is no longer pending"}}"#,
        p.uq.render()
    )));
    assert!(all.contains(r#""routed":{"unrouted":"no target, and you hold 0 nodes"}"#));
    let Routing::Request { request, .. } = k.w.s.user_entries[&routed_to_node].routing else {
        panic!("routed to the work");
    };
    assert!(all.contains(&format!(
        r#""routed":{{"request":"{}","node":"/support/summary-gap"}}"#,
        request.render()
    )));
    // 1.6 (the lab's first round): the first reason names its first item and what holds it
    // there, and each unreachable item its needs (here none: only the absence of every session
    // keeps it from work).
    assert!(all.contains(r#""reason":"unreachable: /support/summary-gap: no session seen in 7d, and 2 more","count":3,"by_needs":2,"by_horizon":1,"items":[{"node":"/support/summary-gap","by":"needs","needs":[]}]"#), "{all}");
    assert!(all.contains("health red: paused: / by coder/s-"), "{all}");
    assert!(all.contains(r#""reason":"overloaded: 900 min of work in 7d, 300 min done; oldest waiting /support/summary-gap""#), "{all}");
    snapshot("every_reply", &log.0);
    snapshot("every_reply.structured", &log.1);
}

/// Routes two more of alice's messages, as `talk route` will (WP-D5a): one to `p.work` as a
/// request, one to none with a reason. Returns them.
/// The `RoutedReply` of user entry `e` as its routing in the state stands, a session holding `holds`
/// nodes naming no target for an unrouted one (1.4, R3): what a `from_user` or `route` reply carries.
fn routed_of(s: &State, e: Eid, holds: u32) -> RoutedReply {
    match s.user_entries.get(&e).map(|u| u.routing) {
        Some(Routing::Request { node, request }) => RoutedReply::Request {
            request,
            node,
            follows: None,
        },
        Some(Routing::Answers { question }) => RoutedReply::Answers { question },
        Some(Routing::Clarifies { request }) => RoutedReply::Clarifies { request },
        Some(Routing::NoAction) => RoutedReply::NoAction,
        Some(Routing::Unrouted) | None => RoutedReply::Unrouted(UnroutedWhy::NoTarget { holds }),
    }
}

fn route_two(k: &mut Kit, p: &People, alice: PrincipalId) -> (Eid, Eid) {
    k.at(60_000);
    let mut out = (Eid(0), Eid(0));
    let (a, work) = (p.assistant, p.work);
    k.events(|d| {
        let ask = append(
            d,
            USER,
            a,
            EntryBody::FromUser { principal: alice },
            text("and add the weekly totals", &[(TextField::Client, "ide")]),
        );
        let key = KeyStr::of_user_entry(ask);
        d.push(Ev::KeyAdded {
            node: work,
            key: Key {
                key: key.clone(),
                nv: NormVer(0),
            },
        });
        let request = append(
            d,
            work,
            a,
            EntryBody::Request {
                origin: Key {
                    key: key.clone(),
                    nv: NormVer(0),
                },
                conv: key,
                from_user: Some(ask),
                requester: Requester::User(alice),
            },
            text("and add the weekly totals", &[]),
        );
        d.push(Ev::EntryRouted {
            entry: ask,
            routing: Routing::Request {
                node: work,
                request,
            },
            reason: None,
        });
        let thanks = append(
            d,
            USER,
            a,
            EntryBody::FromUser { principal: alice },
            text("thanks!", &[(TextField::Client, "ide")]),
        );
        d.push(Ev::EntryRouted {
            entry: thanks,
            routing: Routing::NoAction,
            reason: Some(t("a thank-you: nothing to do")),
        });
        out = (ask, thanks);
    });
    out
}

/// A red health with a reason of each kind, their items naming the people forum's nodes.
fn every_reason(k: &Kit, p: &People, w: SessionId) -> Health {
    use forum_core::health::{MoverGap, UnreachableBy};
    Health {
        level: Level::Red,
        reasons: vec![
            HealthReason::Paused {
                level: Level::Red,
                count: 1,
                top: vec![PauseExclusion {
                    pause: PauseView {
                        scope: ROOT,
                        by: w,
                        class: class("coder"),
                        until: k.w.clock.plus(Dur(3_600_000)),
                    },
                    excluded: 4,
                }],
            },
            HealthReason::FlagsPilingUp { start: 40, now: 95 },
            HealthReason::Overloaded {
                level: Level::Red,
                demand_min: 900,
                supply_min: 300,
                idle_permille: 50,
                oldest: Some(Cand::Node(p.work)),
            },
            HealthReason::NoMover {
                level: Level::Yellow,
                count: 2,
                top: vec![
                    MoverGap::NodeWait {
                        node: p.work,
                        target: ROOT,
                    },
                    MoverGap::FlaggedUnreachable {
                        node: p.work,
                        flags: FlagSet::EMPTY.with(Flag::Objection),
                    },
                ],
            },
            HealthReason::Unreachable {
                level: Level::Yellow,
                count: 3,
                by_needs: 2,
                by_horizon: 1,
                top: vec![(p.work, UnreachableBy::Needs)],
            },
            HealthReason::WriteSurge {
                class: class("coder"),
                last_hour: 5_000,
                baseline: 10,
            },
            HealthReason::Underused { idle_permille: 600 },
        ],
        capacity: forum_core::health::CapacityReport::default(),
    }
}

/// §10.4 and §18 (1.12's review): `beyond_horizon`'s count is shown `≥k` when the scan of the bucket
/// the horizon falls in stopped at `SCAN_MAX`, and as a number otherwise.
#[test]
fn beyond_horizon_shows_a_cut_count_as_at_least() {
    let mut k = Kit::new();
    let (w, _) = k.hello("coder", json!({"action": "hello"}));
    ok(&k.call(
        Some(w),
        "node",
        json!({"action": "create", "path": "/lemma", "title": "lemma", "est": 60, "take": false}),
    ));
    let n = k.node("/lemma");
    let why = |capped| WhyThis {
        tier: 0,
        flags: FlagSet::EMPTY,
        aged: 0,
        reason: Reason::Offered,
        no_estimate: false,
        longer: false,
        beyond_horizon: Some(forum_core::BeyondHorizon {
            count: 2_000,
            capped,
            first: n,
            est: 60,
            horizon: 45,
        }),
    };
    let shown = |capped| super::hello::why_this_at(&k.w.s, None, &why(capped)).to_value();
    assert_eq!(shown(true)["beyond_horizon"]["count"], json!("≥2000"));
    assert_eq!(shown(false)["beyond_horizon"]["count"], json!(2000));
    assert_eq!(
        shown(true)["beyond_horizon"]["first"],
        json!("/lemma est 60 > your horizon 45")
    );
}

/// 1.12 (§20.9; it6): the texts of the seventh round's guidance. Rule 13's `why` names the node, its
/// title cut to fit, and its estimate, with the default to take the next; rule 8's names a merge
/// wait's blocked release with the requests it would settle, as many as fit; rule 3's own intent is
/// closed in the release; and rule 9's reopen of a claim nothing proves is `node reopen {path,
/// reason: ?}`.
#[test]
fn the_seventh_round_s_guidance_texts() {
    let mut k = Kit::new();
    let (w, _) = k.hello("coder", json!({"action": "hello"}));
    ok(&k.call(
        Some(w),
        "node",
        json!({"action": "create", "path": "/toast",
               "title": "Replace the toast library with v2 Toast across every page of the portal",
               "est": 30, "take": false}),
    ));
    let n = k.node("/toast");
    let s = &k.w.s;
    let why = next_calls::why(
        s,
        &Because::WorkWaiting {
            first: forum_core::Cand::Node(n),
        },
    );
    assert!(why.len() <= next_calls::WHY_MAX, "{why}");
    assert!(
        why.starts_with(&format!("take it: {} \"Replace the toast", n.render())),
        "{why}"
    );
    assert!(
        why.ends_with("…\" (est 30); a worker takes the next until next offers nothing"),
        "{why}"
    );
    assert_eq!(
        next_calls::why(
            s,
            &Because::MergeWait {
                settles: vec![Eid(2), Eid(5)]
            }
        ),
        "if done; awaiting merge: release blocked, waits_for {external: 'PR #N merged'}, settles: \
         [e2, e5]"
    );
    // Ids that do not all fit end in "…", whole ids before it.
    let many = next_calls::why(
        s,
        &Because::MergeWait {
            settles: vec![Eid(1_234), Eid(5_678), Eid(9_012)],
        },
    );
    assert_eq!(
        many,
        "if done; awaiting merge: release blocked, waits_for {external: 'PR #N merged'}, settles: \
         […]"
    );
    assert_eq!(
        next_calls::why(
            s,
            &Because::MergeWait {
                settles: Vec::new()
            }
        ),
        "if it is merged; waiting for its merge: release blocked, waits_for {external: 'PR #N \
         merged'}"
    );
    assert_eq!(
        next_calls::why(s, &Because::Finished),
        "if done; else release handoff with the rest in next_step"
    );
    assert_eq!(
        next_calls::why(
            s,
            &Because::IntentOpen {
                by: w,
                entry: Eid(17),
                own: true
            }
        ),
        "after the action announced in e17; pass intent_done in the release if you release next"
    );
    assert_eq!(
        next_calls::why(s, &Because::Unproved),
        "if nothing proves it: reopen; it reads unproved, not refuted; a decide with its basis keeps \
         it done"
    );
    // 1.12's review: a due checkpoint beside rule 8's close of the node is for work that goes on.
    let lease = |closing| Because::LeaseDue {
        lease_until: forum_core::Time(1_790_845_200_000),
        at_ceiling: false,
        intent: None,
        closing,
    };
    let at = super::format::time(forum_core::Time(1_790_845_200_000));
    assert_eq!(
        next_calls::why(s, &lease(true)),
        format!("if not done: your lease ends {at}; checkpoint by then to keep it")
    );
    assert_eq!(
        next_calls::why(s, &lease(false)),
        format!("your lease ends {at}: checkpoint now to keep it")
    );
    let c = next_calls::call(s, Some(w), &Skeleton::Reopen { node: n });
    assert_eq!(
        next_calls::call_text(&c),
        format!(
            "node reopen {{session: {}, path: /toast, reason: ?}}",
            w.render()
        )
    );
}

/// 1.11 (§10.4): a review item's `why_this` names its node's needs, the caps its work asked for,
/// and its tier is its node's without the objection.
#[test]
fn a_review_names_its_node_s_needs() {
    let mut k = Kit::new();
    let (w, _) = k.hello("coder", json!({"action": "hello", "caps": ["lean"]}));
    ok(&k.call(
        Some(w),
        "node",
        json!({"action": "create", "path": "/claims/lemma-02", "title": "Lemma 2",
               "needs": ["lean"], "take": false}),
    ));
    let n = k.node("/claims/lemma-02");
    ok(&k.call(
        Some(w),
        "node",
        json!({"action": "close", "path": "/claims/lemma-02", "outcome": "done",
               "summary": "proved"}),
    ));
    // 1.12 (§10.4): another session objects; its own objection is never its review.
    let (o, _) = k.hello("coder", json!({"action": "hello", "caps": ["lean"]}));
    ok(&k.call(
        Some(o),
        "talk",
        json!({"action": "object", "on": "/claims/lemma-02", "text": "a sorry remains"}),
    ));
    let mine = k.call(Some(o), "hold", json!({"action": "next"}));
    let mine = ok(&mine);
    assert!(
        matches!(mine.reply, Reply::Next(NextReply::Empty { .. })),
        "{:?}",
        mine.reply
    );
    let c = k.call(Some(w), "hold", json!({"action": "next"}));
    let d = ok(&c);
    assert!(
        matches!(d.reply, Reply::Next(NextReply::Review { node, .. }) if node == n),
        "{:?}",
        d.reply
    );
    assert!(
        c.response.text.contains(r#""why_this":{"tier":2,"flags":["objection"],"aged":0,"reason":"review","needs":["lean"]}"#),
        "{}",
        c.response.text
    );
}

/// 1.6 (the lab's first round, where `unreachable 9 (horizon)` and `overloaded` named nothing an
/// agent could act on): the status line names health's first reason by its first item and the
/// value that holds it there, with how many more it counts, and `control health` gives each
/// unreachable item its needs or its `est`.
#[test]
fn health_s_first_reason_names_its_item_and_what_holds_it() {
    use forum_core::health::{MoverGap, UnreachableBy};
    let mut k = Kit::new();
    let (w, _) = k.hello("coder", json!({"action": "hello"}));
    k.at(60_000);
    ok(&k.call(
        Some(w),
        "node",
        json!({"action": "create", "path": "/support/tag-cleanup", "title": "Tags",
               "needs": ["support", "sql"], "take": false}),
    ));
    ok(&k.call(
        Some(w),
        "node",
        json!({"action": "create", "path": "/tidepool/app-store-511", "title": "Release",
               "est": 60, "take": false}),
    ));
    let tags = k.node("/support/tag-cleanup");
    let app = k.node("/tidepool/app-store-511");
    let s = &k.w.s;
    let words = |r: &HealthReason| status::reason_words(s, r);
    let unreachable = |count: u32, top: Vec<(NodeId, UnreachableBy)>| HealthReason::Unreachable {
        level: Level::Red,
        count,
        by_needs: 1,
        by_horizon: 1,
        top,
    };
    let by_needs = unreachable(1, vec![(tags, UnreachableBy::Needs)]);
    assert_eq!(
        words(&by_needs),
        "unreachable: /support/tag-cleanup needs caps no session has (sql, support)"
    );
    let health = Health {
        level: Level::Red,
        reasons: vec![by_needs.clone()],
        capacity: forum_core::health::CapacityReport::default(),
    };
    assert_eq!(
        status::health_segment(s, &health),
        "health red: unreachable: /support/tag-cleanup needs caps no session has (sql, support)"
    );
    assert_eq!(
        words(&unreachable(9, vec![(app, UnreachableBy::Horizon)])),
        "unreachable: /tidepool/app-store-511 est 60 min exceeds every session's horizon, and 8 \
         more"
    );
    assert_eq!(
        words(&HealthReason::NoMover {
            level: Level::Red,
            count: 1,
            top: vec![MoverGap::Dependency {
                node: app,
                target: tags,
            }],
        }),
        "no mover: /tidepool/app-store-511 depends on /support/tag-cleanup, which nothing moves"
    );
    assert_eq!(
        words(&HealthReason::NoMover {
            level: Level::Yellow,
            count: 1,
            top: vec![MoverGap::FlaggedUnreachable {
                node: tags,
                flags: FlagSet::EMPTY.with(Flag::Objection),
            }],
        }),
        "no mover: /support/tag-cleanup is flagged objection, and nothing can take it"
    );
    assert_eq!(
        words(&HealthReason::Overloaded {
            level: Level::Yellow,
            demand_min: 640,
            supply_min: 120,
            idle_permille: 0,
            oldest: Some(Cand::Route(Eid(57))),
        }),
        "overloaded: 640 min of work in 7d, 120 min done; oldest waiting user entry e57 to route"
    );
    assert_eq!(
        words(&HealthReason::Overloaded {
            level: Level::Yellow,
            demand_min: 640,
            supply_min: 120,
            idle_permille: 0,
            oldest: None,
        }),
        "overloaded: 640 min of work in 7d, 120 min done"
    );
    // `control health`: each unreachable item with the value that keeps sessions from it.
    let mut both = unreachable(
        2,
        vec![(tags, UnreachableBy::Needs), (app, UnreachableBy::Horizon)],
    );
    if let HealthReason::Unreachable { by_needs, .. } = &mut both {
        *by_needs = 1;
    }
    k.health = Health {
        level: Level::Red,
        reasons: vec![both],
        capacity: forum_core::health::CapacityReport::default(),
    };
    let view = Cmd::Control(ControlCmd::Health);
    let r = k.render(Some(w), &view, &done(Reply::Health));
    assert!(
        r.text.contains(
            r#""items":[{"node":"/support/tag-cleanup","by":"needs","needs":["sql","support"]},{"node":"/tidepool/app-store-511","by":"horizon","est":60}]"#
        ),
        "{}",
        r.text
    );
}

/// The renderer reads the same texts and pages from the store's tables as from the events held
/// in memory.
#[test]
fn memory_and_store_sources_agree() {
    let mut k = Kit::new();
    let p = people_forum(&mut k);
    let events: Vec<Event> = crate::store::history::events_after(&k.w.conn, 0, u32::MAX)
        .unwrap()
        .into_iter()
        .map(|x| x.event)
        .collect();
    let mem = MemorySource::of(&events);
    let store = StoreSource::new(&k.w.conn);
    let s = &k.w.s;
    let all = BTreeSet::new();
    for e in [
        p.request,
        p.said,
        p.heard,
        p.note,
        p.answer,
        p.from_user,
        p.decision,
    ] {
        assert_eq!(mem.entry(e), store.entry(e), "{e:?}");
    }
    assert_eq!(mem.entry(Eid(9_999)), None);
    let seq = s.nodes[&p.work].seq;
    assert_eq!(
        mem.feed(p.work, Seq(1), seq, &all, 5),
        store.feed(p.work, Seq(1), seq, &all, 5)
    );
    let types: BTreeSet<EntryType> = [EntryType::Said, EntryType::Heard].into();
    assert_eq!(
        mem.feed(p.work, Seq(0), seq, &types, 2),
        store.feed(p.work, Seq(0), seq, &types, 2)
    );
    assert_eq!(mem.newest(p.work, 4), store.newest(p.work, 4));
    assert_eq!(mem.history(p.work, None, 6), store.history(p.work, None, 6));
    let (a, _) = brief::render_brief(s, p.work, Some(p.worker), k.w.clock, &mem);
    let (b, _) = brief::render_brief(s, p.work, Some(p.worker), k.w.clock, &store);
    assert_eq!(a, b);
    let _ = p.assistant;
}

// ------------------------------------------------------------------------------------------------
// Next calls (D.12.6)

/// A 10-digit id: "every id at its longest" in the size rules of D.11.7.
const LONG: u64 = 9_999_999_999;

/// A string of `n` bytes for a key, a principal, or a path step.
fn filler(prefix: &str, n: usize) -> String {
    let mut s = prefix.to_string();
    while s.len() < n {
        s.push('x');
    }
    s.truncate(n);
    s
}

/// A conversation key of `n` bytes: `chat:<channel>/<thread>`.
fn chat_key(n: usize) -> KeyStr {
    let half = (n - 6) / 2;
    let k = format!("chat:{}/{}", filler("c", half), filler("t", n - 6 - half));
    assert_eq!(k.len(), n);
    KeyStr::new(&k).unwrap()
}

/// A forum with a session at a 10-digit id's length of keys: a node at a 64-byte path, a principal
/// of a 64-byte key, and a node under a 1 KiB parent path.
struct Long {
    k: Kit,
    sess: SessionId,
    node: NodeId,
    deep: NodeId,
    principal: PrincipalId,
    huge_principal: PrincipalId,
}

fn long_forum() -> Long {
    let mut k = Kit::new();
    let key64 = filler("mail:", 64);
    let (sess, _) = k.hello(
        "coder",
        json!({"action": "hello", "principal": key64.clone()}),
    );
    // A second principal of 256 bytes, the longest key D.11.14 allows for a principal.
    let key256 = filler("mail:", 256);
    let (other, _) = k.hello("coder", json!({"action": "hello", "principal": key256}));
    let principal = k.w.s.sessions[&sess]
        .principals
        .iter()
        .copied()
        .next()
        .unwrap();
    let huge_principal = k.w.s.sessions[&other]
        .principals
        .iter()
        .copied()
        .next()
        .unwrap();
    // `/` and 63 bytes of names: a 64-byte path.
    let p64 = format!("/{}/{}", filler("a", 31), filler("b", 31));
    assert_eq!(p64.len(), 64);
    let c = k.call(
        Some(sess),
        "node",
        json!({"action": "create", "path": p64, "title": "long", "take": false}),
    );
    ok(&c);
    let node = k.node(&p64);
    // A parent path of 1 KiB: ten steps of 101 bytes and the slashes.
    let deep_path: String = (0..10)
        .map(|i| format!("/{}", filler(&format!("d{i}"), 101)))
        .collect::<String>();
    assert!(deep_path.len() >= 1_000);
    let c = k.call(
        Some(sess),
        "node",
        json!({"action": "create", "path": deep_path, "title": "deep", "take": false}),
    );
    ok(&c);
    let deep = k.node(&deep_path);
    Long {
        k,
        sess,
        node,
        deep,
        principal,
        huge_principal,
    }
}

fn eid() -> Eid {
    Eid(LONG)
}

fn long_token() -> Token {
    Token {
        node: NodeId(LONG),
        fence: Fence(LONG),
    }
}

/// Every `Skeleton` variant, with every id at 10 digits, every node at a 64-byte path, every list
/// at `NEXT_IDS_MAX` ids, and every key and principal at 64 bytes.
fn every_skeleton(l: &Long) -> Vec<Skeleton> {
    let n = l.node;
    let ids = vec![eid(); forum_core::query::guide::NEXT_IDS_MAX];
    let key = chat_key(64);
    let all = vec![
        Skeleton::Hello,
        Skeleton::LogFromUser {
            principal: l.principal,
        },
        Skeleton::ViewNode { node: n },
        Skeleton::Checkpoint {
            token: long_token(),
            intent_done: true,
            intent: false,
        },
        Skeleton::Checkpoint {
            token: long_token(),
            intent_done: false,
            intent: false,
        },
        Skeleton::Checkpoint {
            token: long_token(),
            intent_done: false,
            intent: true,
        },
        Skeleton::Checkpoint {
            token: long_token(),
            intent_done: true,
            intent: true,
        },
        Skeleton::CheckpointSwap {
            token: long_token(),
        },
        Skeleton::Release {
            token: long_token(),
            wait: Some(eid()),
        },
        Skeleton::Release {
            token: long_token(),
            wait: None,
        },
        Skeleton::Take { target: n },
        Skeleton::Next,
        Skeleton::CreateFor { entry: eid() },
        Skeleton::Split { parent: n },
        Skeleton::AddOffer {
            node: n,
            version: Version(LONG),
        },
        Skeleton::Close {
            node: n,
            token: long_token(),
            settles: ids.clone(),
            outcome: None,
        },
        Skeleton::Close {
            node: n,
            token: long_token(),
            settles: ids.clone(),
            outcome: Some(Outcome::Superseded),
        },
        Skeleton::SayTo {
            at: n,
            to: SessionId(LONG),
        },
        Skeleton::ToUser {
            principal: l.principal,
            kind: Some(ToUserKind::Result),
            re: ids.clone(),
        },
        Skeleton::ToUser {
            principal: l.principal,
            kind: None,
            re: ids.clone(),
        },
        Skeleton::Said {
            at: n,
            kind: SaidKind::Correction,
            where_: Some(key.clone()),
            where_fill: false,
        },
        Skeleton::Said {
            at: n,
            kind: SaidKind::Reply,
            where_: None,
            where_fill: true,
        },
        Skeleton::Heard {
            at: n,
            answers: eid(),
            where_: key,
        },
        Skeleton::Decide {
            at: n,
            settles: ids.clone(),
            supersedes: Some(eid()),
        },
        Skeleton::Route {
            entry: eid(),
            to: RouteSk::Nowhere,
        },
        Skeleton::Route {
            entry: eid(),
            to: RouteSk::Fill,
        },
        Skeleton::Route {
            entry: eid(),
            to: RouteSk::Node(n),
        },
        Skeleton::Route {
            entry: eid(),
            to: RouteSk::Answers(eid()),
        },
        Skeleton::Route {
            entry: eid(),
            to: RouteSk::Clarifies(eid()),
        },
        Skeleton::AnswerUser {
            question: eid(),
            principal: Some(l.principal),
        },
        Skeleton::AnswerAgent { question: eid() },
        Skeleton::Resolve { objection: eid() },
        Skeleton::Merge { from: n, into: n },
        Skeleton::Wait {
            timeout: Dur(50_000),
        },
        Skeleton::LinkDuplicate { from: n, to: n },
        Skeleton::AskOwners { at: n },
        Skeleton::Reopen { node: n },
    ];
    // Every variant is here: the match fails to compile when one is added.
    let mut seen = BTreeSet::new();
    for s in &all {
        seen.insert(match s {
            Skeleton::Hello => 0,
            Skeleton::LogFromUser { .. } => 1,
            Skeleton::ViewNode { .. } => 2,
            Skeleton::Checkpoint { .. } => 3,
            Skeleton::CheckpointSwap { .. } => 4,
            Skeleton::Release { .. } => 5,
            Skeleton::Take { .. } => 6,
            Skeleton::Next => 7,
            Skeleton::CreateFor { .. } => 8,
            Skeleton::Split { .. } => 9,
            Skeleton::AddOffer { .. } => 10,
            Skeleton::Close { .. } => 11,
            Skeleton::SayTo { .. } => 12,
            Skeleton::ToUser { .. } => 13,
            Skeleton::Said { .. } => 14,
            Skeleton::Heard { .. } => 15,
            Skeleton::Decide { .. } => 16,
            Skeleton::Route { .. } => 17,
            Skeleton::AnswerUser { .. } => 18,
            Skeleton::AnswerAgent { .. } => 19,
            Skeleton::Resolve { .. } => 20,
            Skeleton::Merge { .. } => 21,
            Skeleton::Wait { .. } => 22,
            Skeleton::LinkDuplicate { .. } => 23,
            Skeleton::AskOwners { .. } => 24,
            Skeleton::Reopen { .. } => 25,
        });
    }
    assert_eq!(seen.len(), 26, "every Skeleton variant");
    all
}

/// Every `Because` variant at its longest: ids and counts at 10 digits, times at 20 bytes.
fn every_because(p: PrincipalId) -> Vec<Because> {
    let ids = vec![eid(); forum_core::query::guide::NEXT_IDS_MAX];
    let t = Time(253_402_300_740_000); // 9999-12-31T23:59:00Z
    let s = SessionId(LONG);
    let n = NodeId(LONG);
    let big = u32::MAX;
    let all = vec![
        Because::HoldEnded { late: eid() },
        Because::Reread {
            version: Version(LONG),
            digest_version: Version(LONG),
        },
        Because::MergedInto { node: n },
        Because::CloseOpenIntent {
            by: s,
            entry: eid(),
            own: true,
        },
        Because::CloseOpenIntent {
            by: s,
            entry: eid(),
            own: false,
        },
        Because::RenewLimit,
        Because::AnswerFits { question: eid() },
        Because::NoSession,
        Because::LogFirst { principal: p },
        Because::TellHolder { holder: s },
        Because::LeaseDue {
            lease_until: t,
            at_ceiling: true,
            intent: Some(eid()),
            closing: false,
        },
        Because::LeaseDue {
            lease_until: t,
            at_ceiling: false,
            intent: Some(eid()),
            closing: true,
        },
        Because::LeaseDue {
            lease_until: t,
            at_ceiling: false,
            intent: None,
            closing: false,
        },
        Because::LeaseDue {
            lease_until: t,
            at_ceiling: false,
            intent: None,
            closing: true,
        },
        Because::IntentOpen {
            by: s,
            entry: eid(),
            own: true,
        },
        Because::IntentOpen {
            by: s,
            entry: eid(),
            own: false,
        },
        Because::CheckpointBy { by: t },
        Because::UncoveredSlot,
        Because::News {
            node: n,
            count: Total::AtLeast(big),
        },
        Because::Unrouted {
            entry: eid(),
            related: None,
        },
        Because::Unrouted {
            entry: eid(),
            related: Some(n),
        },
        Because::AnsweredEntry {
            entry: eid(),
            or: None,
        },
        Because::AnsweredEntry {
            entry: eid(),
            or: Some(RouteAlt::Node {
                node: n,
                held: true,
            }),
        },
        Because::AnsweredEntry {
            entry: eid(),
            or: Some(RouteAlt::Create),
        },
        Because::RouteAnswers {
            entry: eid(),
            question: eid(),
            or: None,
        },
        Because::RouteAnswers {
            entry: eid(),
            question: eid(),
            or: Some(RouteAlt::Nowhere),
        },
        Because::RouteAnswers {
            entry: eid(),
            question: eid(),
            or: Some(RouteAlt::Node {
                node: n,
                held: false,
            }),
        },
        Because::RouteAnswers {
            entry: eid(),
            question: eid(),
            or: Some(RouteAlt::Create),
        },
        Because::RouteNode {
            entry: eid(),
            node: n,
            held: true,
            or: Some(RouteAlt::Create),
        },
        Because::RouteNode {
            entry: eid(),
            node: n,
            held: false,
            or: Some(RouteAlt::Create),
        },
        Because::RouteNode {
            entry: eid(),
            node: n,
            held: false,
            or: None,
        },
        Because::RouteClarifies {
            entry: eid(),
            request: eid(),
            or: Some(RouteAlt::Node {
                node: n,
                held: true,
            }),
        },
        Because::Linked { into: n },
        Because::ReplyOwed {
            requests: ids.clone(),
            misaddressed: None,
            recorded: false,
            ready: false,
        },
        Because::ReplyOwed {
            requests: ids.clone(),
            misaddressed: None,
            recorded: true,
            ready: false,
        },
        Because::ReplyOwed {
            requests: ids.clone(),
            misaddressed: None,
            recorded: false,
            ready: true,
        },
        Because::ReplyOwed {
            requests: ids.clone(),
            misaddressed: Some(p),
            recorded: false,
            ready: false,
        },
        Because::DecisionOwed {
            requests: ids.clone(),
            after_close: false,
            ready: true,
            recorded: false,
            held: true,
            after_release: false,
        },
        Because::DecisionOwed {
            requests: ids.clone(),
            after_close: true,
            ready: false,
            recorded: false,
            held: false,
            after_release: false,
        },
        Because::DecisionOwed {
            requests: ids.clone(),
            after_close: false,
            ready: false,
            recorded: false,
            held: false,
            after_release: false,
        },
        Because::DecisionOwed {
            requests: ids.clone(),
            after_close: false,
            ready: false,
            recorded: true,
            held: false,
            after_release: false,
        },
        Because::DecisionOwed {
            requests: ids.clone(),
            after_close: false,
            ready: false,
            recorded: true,
            held: true,
            after_release: false,
        },
        Because::DecisionOwed {
            requests: ids.clone(),
            after_close: false,
            ready: false,
            recorded: false,
            held: false,
            after_release: true,
        },
        Because::Finished,
        Because::MergeWait {
            settles: ids.clone(),
        },
        Because::MergeWait {
            settles: Vec::new(),
        },
        Because::Unproved,
        Because::Reclose {
            was: Outcome::Superseded,
        },
        Because::ContainerStays { series: true },
        Because::ContainerStays { series: false },
        Because::AskWait {
            question: eid(),
            blocking: true,
            existing: false,
        },
        Because::AskWait {
            question: eid(),
            blocking: false,
            existing: false,
        },
        Because::AskWait {
            question: eid(),
            blocking: true,
            existing: true,
        },
        Because::AgentWait { question: eid() },
        Because::JoinAnswer { question: eid() },
        Because::NotOffered,
        Because::AnswersNext {
            question: eid(),
            principal: p,
        },
        Because::StatusItem,
        Because::Review { objection: eid() },
        Because::RouteItem,
        Because::Assumption {
            questions: ids.clone(),
            blocking: true,
        },
        Because::Assumption {
            questions: ids.clone(),
            blocking: false,
        },
        Because::StaleBasis { decision: eid() },
        Because::Recheck { question: eid() },
        Because::TooBig { attempts: big },
        Because::Duplicate { other: n },
        Because::Signal,
        Because::Objection {
            objection: eid(),
            retracted: None,
        },
        Because::Objection {
            objection: eid(),
            retracted: Some(eid()),
        },
        Because::Inbox { count: big },
        Because::AgentAsked {
            by: s,
            until: t,
            question: eid(),
            later: true,
        },
        Because::AgentAsked {
            by: s,
            until: t,
            question: eid(),
            later: false,
        },
        Because::HumanItems {
            principal: p,
            questions: big,
            digest: big,
        },
        Because::Tidy {
            node: n,
            flags: Flag::ALL.into_iter().collect(),
        },
        Because::Joined,
        Because::MoreWork { at_hello: true },
        Because::MoreWork { at_hello: false },
        Because::StaleOffer { node: n, cut: true },
        Because::Alike {
            from: n,
            to: n,
            cut: true,
        },
        Because::AwaitMerge,
        Because::StaleAnswered {
            node: n,
            question: eid(),
            cut: true,
        },
        Because::WorkWaiting {
            first: forum_core::Cand::Status(n),
        },
        Because::WorkWaiting {
            first: forum_core::Cand::Node(n),
        },
        Because::WorkWaiting {
            first: forum_core::Cand::Route(eid()),
        },
    ];
    let mut seen = BTreeSet::new();
    for b in &all {
        seen.insert(match b {
            Because::HoldEnded { .. } => 0,
            Because::Reread { .. } => 1,
            Because::MergedInto { .. } => 2,
            Because::CloseOpenIntent { .. } => 3,
            Because::RenewLimit => 4,
            Because::AnswerFits { .. } => 5,
            Because::NoSession => 6,
            Because::LogFirst { .. } => 7,
            Because::Review { .. } => 8,
            Because::TellHolder { .. } => 9,
            Because::LeaseDue { .. } => 10,
            Because::IntentOpen { .. } => 11,
            Because::CheckpointBy { .. } => 12,
            Because::News { .. } => 13,
            Because::UncoveredSlot => 14,
            Because::Unrouted { .. } => 15,
            Because::AnsweredEntry { .. } => 16,
            Because::ReplyOwed { .. } => 17,
            Because::DecisionOwed { .. } => 18,
            Because::Finished => 19,
            Because::ContainerStays { .. } => 20,
            Because::AskWait { .. } => 21,
            Because::AgentWait { .. } => 22,
            Because::JoinAnswer { .. } => 23,
            Because::NotOffered => 24,
            Because::AnswersNext { .. } => 25,
            Because::StatusItem => 26,
            Because::RouteItem => 27,
            Because::Assumption { .. } => 28,
            Because::StaleBasis { .. } => 29,
            Because::Recheck { .. } => 30,
            Because::TooBig { .. } => 31,
            Because::Duplicate { .. } => 32,
            Because::Signal => 33,
            Because::Objection { .. } => 34,
            Because::Inbox { .. } => 35,
            Because::AgentAsked { .. } => 36,
            Because::HumanItems { .. } => 37,
            Because::Tidy { .. } => 38,
            Because::Joined => 39,
            Because::MoreWork { .. } => 40,
            Because::Reclose { .. } => 41,
            Because::RouteAnswers { .. } => 42,
            Because::RouteNode { .. } => 43,
            Because::StaleOffer { .. } => 44,
            Because::Alike { .. } => 45,
            Because::AwaitMerge => 46,
            Because::StaleAnswered { .. } => 47,
            Because::WorkWaiting { .. } => 48,
            Because::RouteClarifies { .. } => 49,
            Because::Linked { .. } => 50,
            Because::MergeWait { .. } => 51,
            Because::Unproved => 52,
        });
    }
    assert_eq!(seen.len(), 53, "every Because variant");
    all
}

/// 1.12: where a `Because` quotes, last, names it cuts to fit: the requests a blocked release
/// would settle (`MergeWait`), and the title of the node `next` would give (`WorkWaiting`).
fn quotes_names(b: &Because) -> Option<&'static str> {
    match b {
        Because::MergeWait { settles } if !settles.is_empty() => Some("settles: ["),
        Because::WorkWaiting {
            first: forum_core::Cand::Node(_),
        } => Some("\""),
        _ => None,
    }
}

/// Whether a `Because` quotes a principal's key.
fn quotes_principal(b: &Because) -> bool {
    matches!(
        b,
        Because::LogFirst { .. }
            | Because::ReplyOwed {
                misaddressed: Some(_),
                ..
            }
            | Because::AnswersNext { .. }
            | Because::HumanItems { .. }
    )
}

/// The call part of every skeleton is at most 256 bytes with each key and the split's parent path
/// at 64 bytes, and every `why` is at most 100 bytes with its condition never cut (D.12.6).
#[test]
fn every_item_fits_its_size_rules() {
    let l = long_forum();
    let s = &l.k.w.s;
    for sk in every_skeleton(&l) {
        let c = next_calls::call(s, Some(SessionId(LONG)), &sk);
        let text = next_calls::call_text(&c);
        assert!(
            text.len() <= next_calls::CALL_MAX,
            "{} bytes: {text}",
            text.len()
        );
    }
    // A short principal: no template is cut. A 256-byte one: the cut falls on the key only,
    // which every template that quotes one puts last.
    let short = l.principal;
    for b in every_because(short) {
        let w = next_calls::why(s, &b);
        if quotes_principal(&b) {
            // The 64-byte key may be cut; the words before it never are.
            let key = format::principal(s, short);
            let words = w.split(key.get(..8).unwrap()).next().unwrap();
            assert!(w.len() <= next_calls::WHY_MAX, "{w}");
            assert!(!words.contains(format::ELLIPSIS), "{w}");
        } else if let Some(at) = quotes_names(&b) {
            // 1.12: the cut falls on the names quoted last, never on the words before them.
            assert!(w.len() <= next_calls::WHY_MAX, "{} bytes: {w}", w.len());
            let words = w.split(at).next().unwrap();
            assert!(!words.contains(format::ELLIPSIS), "cut: {w}");
        } else {
            assert!(w.len() <= next_calls::WHY_MAX, "{} bytes: {w}", w.len());
            assert!(!w.contains(format::ELLIPSIS), "cut: {w}");
        }
    }
    let huge = format::principal(s, l.huge_principal);
    assert_eq!(huge.len(), 256);
    for b in every_because(l.huge_principal) {
        let w = next_calls::why(s, &b);
        assert!(w.len() <= next_calls::WHY_MAX, "{w}");
        if quotes_principal(&b) {
            let at = w.find("mail:").expect("the key's start is shown");
            assert!(w.ends_with(format::ELLIPSIS), "{w}");
            assert!(at > 0, "the condition comes first: {w}");
        }
    }
}

/// A 256-byte principal key, a 512-byte `where`, and a split under a 1 KiB parent path render
/// whole; the split's `args` decode (D.12.6).
#[test]
fn long_keys_and_a_split_parent_render_whole() {
    let l = long_forum();
    let s = &l.k.w.s;
    let huge = format::principal(s, l.huge_principal);
    let c = next_calls::call(
        s,
        Some(l.sess),
        &Skeleton::ToUser {
            principal: l.huge_principal,
            kind: Some(ToUserKind::Result),
            re: vec![eid()],
        },
    );
    assert!(next_calls::call_text(&c).contains(&huge));
    let w512 = chat_key(512);
    let c = next_calls::call(
        s,
        Some(l.sess),
        &Skeleton::Said {
            at: l.node,
            kind: SaidKind::Result,
            where_: Some(w512.clone()),
            where_fill: false,
        },
    );
    assert!(next_calls::call_text(&c).contains(w512.as_str()));
    let parent = format::path(s, l.deep);
    assert!(parent.len() >= 1_000);
    let c = next_calls::call(s, Some(l.sess), &Skeleton::Split { parent: l.deep });
    let text = next_calls::call_text(&c);
    assert!(text.contains(&format!("path: {parent}/?")), "{text}");
    assert!(
        !text.contains("name: ?"),
        "the name is the path's ?: {text}"
    );
    // The args decode, the agent's name in place of the `?`.
    let mut args = c.args.to_value();
    args["path"] = Value::String(format!("{parent}/child"));
    args["title"] = Value::String("a smaller step".into());
    let cmd = decode_args("node", args, &l.k);
    match cmd {
        Cmd::Node(NodeCmd::Create(a)) => {
            assert_eq!(a.path.0.len(), 11);
            assert_eq!(a.take, Some(false));
            assert!(a.tags.contains(&Label::new("offer").unwrap()));
        }
        other => panic!("{other:?}"),
    }
}

/// Decodes `args` of `tool` through D.11.5, as the next call's agent would send them.
fn decode_args(tool: &str, args: Value, k: &Kit) -> Cmd {
    struct Questions;
    impl EntryLookup for Questions {
        fn entry_node(&self, _: Eid) -> Option<NodeId> {
            Some(NodeId(8))
        }
        fn question(&self, _: Eid) -> Option<(NodeId, Option<SessionId>)> {
            Some((NodeId(8), Some(SessionId(1))))
        }
    }
    let Value::Object(map) = args else {
        panic!("args are an object");
    };
    let env = DecodeEnv {
        now: k.w.clock,
        text_max: k.w.s.params.text_max,
        coverage_window: k.w.s.params.coverage_window,
        entries: &Questions,
    };
    match prepare(tool, map, &env) {
        Ok(p) => p.cmd,
        Err(e) => panic!("{tool}: {}", e.render()),
    }
}

/// A value an agent writes for a `fill` argument.
fn fill_value(name: &str) -> Value {
    match name {
        "outcome" => json!("done"),
        "verbatim" => json!(true),
        // 1.9: the verdict of a review; 1.10: `stands`, `upheld` before.
        "stands" => json!(false),
        "kind" => json!("result"),
        "where" => json!("chat:team/1"),
        "to" => json!("/new-work"),
        "path" => json!("/new-work"),
        "basis" => json!(["e1"]),
        _ => json!("written by the agent"),
    }
}

/// Every skeleton's `args`, with its `fill` written, decode through D.11.5 to the command the
/// skeleton describes (D.12.6).
#[test]
fn every_skeletons_args_decode_to_its_command() {
    let l = long_forum();
    let s = &l.k.w.s;
    let at = NodeRef::Path(crate::decode::values::path(&format::path(s, l.node)).unwrap());
    for sk in every_skeleton(&l) {
        let c = next_calls::call(s, Some(l.sess), &sk);
        let mut args = c.args.to_value();
        // An optional fill (`intent?`) left out decodes too.
        let mut bare = args.clone();
        for f in &c.fill {
            let arg = next_calls::fill_arg(f);
            if *f == "name" {
                let p = args["path"].as_str().unwrap().replace("/?", "/child");
                args["path"] = Value::String(p.clone());
                bare["path"] = Value::String(p);
            } else {
                args[arg] = fill_value(arg);
                if arg == *f {
                    bare[arg] = fill_value(arg);
                }
            }
        }
        if c.fill.iter().any(|f| f.ends_with('?')) {
            let _ = decode_args(c.tool, bare, &l.k);
        }
        if !matches!(sk, Skeleton::Hello) {
            assert_eq!(args["session"], json!(l.sess.render()), "{sk:?}");
        } else {
            assert!(args.get("session").is_none(), "hello opens a new session");
        }
        let cmd = decode_args(c.tool, args, &l.k);
        let fits = match (&sk, &cmd) {
            (Skeleton::Hello, Cmd::Session(SessionCmd::Hello { .. })) => true,
            (
                Skeleton::LogFromUser { .. },
                Cmd::Talk(TalkCmd::FromUser {
                    principal: Some(_),
                    answers: None,
                    ..
                }),
            ) => true,
            (Skeleton::ViewNode { .. }, Cmd::View(ViewCmd::Node { targets, .. })) => {
                targets == &vec![at.clone()]
            }
            (
                Skeleton::Checkpoint {
                    token,
                    intent_done,
                    intent,
                },
                Cmd::Hold(HoldCmd::Checkpoint {
                    token: t,
                    intent_done: d,
                    intent: i,
                    ..
                }),
            ) => t == token && d.is_some() == *intent_done && i.is_some() == *intent,
            (
                Skeleton::CheckpointSwap { token },
                Cmd::Hold(HoldCmd::Checkpoint {
                    token: t,
                    intent: Some(_),
                    intent_done: Some(_),
                    ..
                }),
            ) => t == token,
            (
                Skeleton::Release { token, wait },
                Cmd::Hold(HoldCmd::Release {
                    token: t,
                    reason,
                    waits_for,
                    ..
                }),
            ) => {
                t == token
                    && match wait {
                        Some(q) => {
                            *reason == ReleaseReason::Blocked
                                && *waits_for == Some(WaitArg::Question(*q))
                        }
                        None => *reason == ReleaseReason::Handoff && waits_for.is_none(),
                    }
            }
            (Skeleton::Take { .. }, Cmd::Hold(HoldCmd::Take { target, .. })) => *target == at,
            (Skeleton::Next, Cmd::Hold(HoldCmd::Next { query, .. })) => {
                *query == NextQuery::default()
            }
            (Skeleton::CreateFor { entry }, Cmd::Node(NodeCmd::Create(a))) => a
                .keys
                .iter()
                .any(|k| k.key == KeyStr::of_user_entry(*entry)),
            (Skeleton::Split { .. }, Cmd::Node(NodeCmd::Create(a))) => a.take == Some(false),
            (
                Skeleton::AddOffer { version, .. },
                Cmd::Node(NodeCmd::Update {
                    at: x,
                    version: v,
                    patch,
                    ..
                }),
            ) => *x == at && v == version && patch.tags_add.contains(&Label::new("offer").unwrap()),
            (
                Skeleton::Close { token, settles, .. },
                Cmd::Node(NodeCmd::Close {
                    at: x,
                    token: t,
                    settles: st,
                    ..
                }),
            ) => {
                *x == at
                    && *t == Some(*token)
                    // 1.7 (§5): an item without `settles` decodes to the default.
                    && match st {
                        None => settles.is_empty(),
                        Some(st) => {
                            st.iter().copied().collect::<Vec<_>>()
                                == settles
                                    .iter()
                                    .copied()
                                    .collect::<BTreeSet<_>>()
                                    .into_iter()
                                    .collect::<Vec<_>>()
                        }
                    }
            }
            (
                Skeleton::SayTo { to, .. },
                Cmd::Talk(TalkCmd::Say {
                    at: x,
                    to: Some(AddresseeArg::Session(y)),
                    ..
                }),
            ) => *x == at && y == to,
            (
                Skeleton::ToUser { kind, re, .. },
                Cmd::Talk(TalkCmd::ToUser {
                    principal: Some(_),
                    kind: k2,
                    re: r2,
                    ..
                }),
            ) => {
                kind.is_none_or(|k| k == *k2)
                    && r2.iter().copied().collect::<Vec<_>>()
                        == re
                            .iter()
                            .copied()
                            .collect::<BTreeSet<_>>()
                            .into_iter()
                            .collect::<Vec<_>>()
            }
            (
                Skeleton::Said {
                    kind,
                    where_,
                    where_fill,
                    ..
                },
                Cmd::Talk(TalkCmd::Said {
                    at: x,
                    kind: k2,
                    where_: w2,
                    ..
                }),
            ) => {
                *x == at
                    && k2 == kind
                    && match where_ {
                        Some(w) => w2.as_ref().map(|k| &k.key) == Some(w),
                        None => w2.is_some() == *where_fill,
                    }
            }
            (
                Skeleton::Heard {
                    answers, where_, ..
                },
                Cmd::Talk(TalkCmd::Heard {
                    at: x,
                    where_: Some(w2),
                    answers: Some(q),
                    ..
                }),
            ) => *x == at && q == answers && w2.key == *where_,
            (
                Skeleton::Decide {
                    settles,
                    supersedes,
                    ..
                },
                Cmd::Talk(TalkCmd::Decide {
                    at: x,
                    settles: st,
                    supersedes: sp,
                    ..
                }),
            ) => {
                *x == at
                    && sp == supersedes
                    && st.iter().copied().collect::<Vec<_>>()
                        == settles
                            .iter()
                            .copied()
                            .collect::<BTreeSet<_>>()
                            .into_iter()
                            .collect::<Vec<_>>()
            }
            (
                Skeleton::Route { entry, to },
                Cmd::Talk(TalkCmd::Route {
                    entry: e,
                    to: t,
                    answers,
                    clarifies,
                    ..
                }),
            ) => {
                e == entry
                    && match to {
                        RouteSk::Fill => t.is_some(),
                        RouteSk::Nowhere => matches!(t, Some(forum_core::RouteTo::Nowhere)),
                        RouteSk::Node(_) => matches!(t, Some(forum_core::RouteTo::Node(_))),
                        RouteSk::Answers(q) => *answers == Some(*q),
                        RouteSk::Clarifies(r) => *clarifies == Some(*r),
                    }
            }
            (
                Skeleton::AnswerUser { question, .. },
                Cmd::Talk(TalkCmd::FromUser {
                    answers: Some(q), ..
                }),
            ) => q == question,
            (
                Skeleton::AnswerAgent { question },
                Cmd::Talk(TalkCmd::Answer { question: q, .. }),
            ) => q == question,
            (Skeleton::Resolve { objection }, Cmd::Talk(TalkCmd::Resolve { objection: o, .. })) => {
                o == objection
            }
            (Skeleton::Merge { .. }, Cmd::Node(NodeCmd::Merge { from, into })) => {
                *from == at && *into == at
            }
            (
                Skeleton::LinkDuplicate { .. },
                Cmd::Node(NodeCmd::Link {
                    at: x,
                    rel: forum_core::LinkRel::Duplicate,
                    to,
                }),
            ) => *x == at && *to == at,
            (Skeleton::Wait { timeout }, Cmd::Talk(TalkCmd::Wait { timeout: d, .. })) => {
                d == timeout
            }
            // 1.10: a stale offer's question, in the human form, with its options and default.
            (
                Skeleton::AskOwners { .. },
                Cmd::Talk(TalkCmd::Ask {
                    at: x,
                    to: forum_core::AskTo::User(u),
                    again: false,
                    ..
                }),
            ) => {
                *x == at
                    && u.to.is_none()
                    && u.options
                        .iter()
                        .map(|o| o.0.as_str())
                        .eq(forum_core::query::guide::STALE_OPTIONS)
                    && u.default.as_ref().map(|d| d.0.as_str())
                        == Some(forum_core::query::guide::STALE_DEFAULT)
                    && u.due.is_none()
                    && u.recommendation.is_none()
            }
            // 1.12 (rule 9): the reopen of a claim nothing proves.
            (Skeleton::Reopen { .. }, Cmd::Node(NodeCmd::Reopen { at: x, .. })) => *x == at,
            _ => false,
        };
        assert!(fits, "{sk:?} decoded to {cmd:?}");
    }
}

/// 1.7 (§20.4): a row shows a waiting node's wait instead of `open`.
#[test]
fn a_row_shows_a_waiting_node_s_wait() {
    let mut k = Kit::new();
    let (s, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(s),
        "node",
        json!({"action": "create", "path": "/job", "title": "a long job"}),
    );
    let token = ok(&c);
    let token = match &token.reply {
        Reply::Created { token: Some(t), .. } => t.render(),
        other => panic!("{other:?}"),
    };
    ok(&k.call(
        Some(s),
        "hold",
        json!({"action": "release", "token": token, "done": "started", "next_step": "check it",
               "reason": "blocked",
               "waits_for": {"external": "cluster job 9362", "recheck_after": "2026-09-22T10:00:00Z"}}),
    ));
    let c = k.call(Some(s), "view", json!({"action": "list"}));
    let row = c.response.structured["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["path"] == json!("/job"))
        .cloned()
        .unwrap();
    assert_eq!(
        row["state"],
        json!("waiting (external) until 2026-09-22T10:00:00Z")
    );
}

/// The token a `node create` issued.
fn created_token(c: &Called) -> String {
    match &ok(c).reply {
        Reply::Created { token: Some(t), .. } => t.render(),
        other => panic!("{other:?}"),
    }
}

/// The detail of the first warning of `code` in a response.
fn warning_detail(c: &Called, code: &str) -> String {
    c.response.structured["warnings"]
        .as_array()
        .unwrap_or_else(|| panic!("no warnings: {}", c.response.text))
        .iter()
        .find(|w| w["code"] == json!(code))
        .unwrap_or_else(|| panic!("no {code}: {}", c.response.text))["detail"]
        .as_str()
        .unwrap()
        .to_string()
}

/// 1.10 (§10.1, §21, l1 of the lab's fifth round): the `waiting` warning tells the session whose
/// release set the wait that its next checkpoint ends it, and another taker which release to read
/// and that its first checkpoint ends the wait.
#[test]
fn the_waiting_warning_tells_who_set_the_wait() {
    let mut k = Kit::new();
    let (a, _) = k.hello("coder", json!({"action": "hello"}));
    let (b, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(a),
        "node",
        json!({"action": "create", "path": "/fix", "title": "fix the crash"}),
    );
    let token = created_token(&c);
    let release = |k: &mut Kit, s: SessionId, token: &str| {
        ok(&k.call(
            Some(s),
            "hold",
            json!({"action": "release", "token": token, "done": "asked Omar", "next_step": "apply his choice",
                   "reason": "blocked",
                   "waits_for": {"external": "Omar's choice of fix", "recheck_after": "2026-09-22T10:00:00Z"}}),
        ));
    };
    release(&mut k, a, &token);
    let e = k.w.s.nodes[&k.node("/fix")]
        .last_checkpoint
        .unwrap()
        .render();
    let c = k.call(Some(a), "hold", json!({"action": "take", "target": "/fix"}));
    let detail = warning_detail(&c, "waiting");
    assert!(
        detail.starts_with(&format!("you set this wait in {e}:")),
        "{detail}"
    );
    assert!(detail.contains("your next checkpoint ends it"), "{detail}");
    let token = match &ok(&c).reply {
        Reply::Taken { token, .. } => token.render(),
        other => panic!("{other:?}"),
    };
    release(&mut k, a, &token);
    let e = k.w.s.nodes[&k.node("/fix")]
        .last_checkpoint
        .unwrap()
        .render();
    let c = k.call(Some(b), "hold", json!({"action": "take", "target": "/fix"}));
    let detail = warning_detail(&c, "waiting");
    assert!(
        detail.contains(&format!(
            "read the release that set it, {e}, and ask its author"
        )),
        "{detail}"
    );
    assert!(
        detail.ends_with("your first checkpoint ends the wait"),
        "{detail}"
    );
}

/// 1.10 (§13, §20.7, l7 of the lab's fifth round): a `say` without `to` says whom it reached: the
/// next holder of a node nobody holds, the holders of a held one, and nobody for a plain note. 1.12
/// (it6 l6): `delivered_to` is always a list, `[]` for none, and the next holder is `pending`.
#[test]
fn a_say_names_whom_it_reached() {
    let mut k = Kit::new();
    let (a, _) = k.hello("coder", json!({"action": "hello"}));
    let (b, _) = k.hello("coder", json!({"action": "hello"}));
    ok(&k.call(
        Some(a),
        "node",
        json!({"action": "create", "path": "/tokens", "title": "migrate the tokens", "take": false}),
    ));
    let say = |k: &mut Kit, s: SessionId, at: &str| {
        let c = k.call(
            Some(s),
            "talk",
            json!({"action": "say", "at": at, "text": "PR #215 conflicts with #214"}),
        );
        ok(&c);
        (
            c.response.structured["delivered_to"].clone(),
            c.response.structured["pending"].clone(),
        )
    };
    assert_eq!(
        say(&mut k, b, "/tokens"),
        (json!([]), json!("the next holder of /tokens"))
    );
    ok(&k.call(
        Some(a),
        "hold",
        json!({"action": "take", "target": "/tokens"}),
    ));
    assert_eq!(
        say(&mut k, b, "/tokens"),
        (json!([a.render()]), Value::Null)
    );
    assert_eq!(say(&mut k, a, "/tokens"), (json!([]), Value::Null));
    assert_eq!(say(&mut k, a, "/"), (json!([]), Value::Null));
}

/// 1.10 (§10.4, §11.3, l5 of the lab's fifth round): a resolve that keeps a claim whose deciding
/// decision is retracted warns `no_basis`, and its brief's header says so.
#[test]
fn a_claim_with_its_decision_retracted_reads_so() {
    let mut k = Kit::new();
    let (a, _) = k.hello("coder", json!({"action": "hello"}));
    ok(&k.call(
        Some(a),
        "node",
        json!({"action": "create", "path": "/claims/lemma-06", "title": "g(6) <= 192",
               "kind": "claim", "take": false}),
    ));
    let d = match &ok(&k.call(
        Some(a),
        "talk",
        json!({"action": "decide", "at": "/claims/lemma-06", "text": "the exhaustive check passes"}),
    ))
    .reply
    {
        Reply::Decided { decision, .. } => decision.render(),
        other => panic!("{other:?}"),
    };
    ok(&k.call(
        Some(a),
        "node",
        json!({"action": "close", "path": "/claims/lemma-06", "outcome": "done", "summary": "checked"}),
    ));
    ok(&k.call(
        Some(a),
        "talk",
        json!({"action": "retract", "entry": d, "text": "the check double-counted", "reason": "duplicate rows"}),
    ));
    let o = match &ok(&k.call(
        Some(a),
        "talk",
        json!({"action": "object", "on": "/claims/lemma-06", "text": "the raw masks repeat"}),
    ))
    .reply
    {
        Reply::Objected { objection } => objection.render(),
        other => panic!("{other:?}"),
    };
    let c = k.call(
        Some(a),
        "talk",
        json!({"action": "resolve", "objection": o, "text": "this rejects the objection's evidence only",
               "upheld": false}),
    );
    // 1.12 (§11.3, §21): the alternative to a decide is rule 9's reopen, not a new objection.
    assert_eq!(
        warning_detail(&c, "no_basis"),
        format!(
            "/claims/lemma-06 reads as done, but its decision {d} is retracted and nothing else \
             decides it: if it is proved, talk decide {{at, text, basis}}; if not, reopen it (node \
             reopen): it reads unproved, not refuted"
        ),
        "{}",
        c.response.text
    );
    let c = k.call(
        Some(a),
        "view",
        json!({"action": "node", "path": "/claims/lemma-06"}),
    );
    assert!(
        c.response
            .text
            .contains(&format!(r#""state":"closed done, decision {d} retracted""#)),
        "{}",
        c.response.text
    );
}

/// 1.10 (§11.3, §20.7, l5 of the lab's fifth round): the later, opposite verdict is recorded and
/// named, with who gave the earlier one; the same verdict again names it.
#[test]
fn a_second_verdict_is_recorded_and_named() {
    let mut k = Kit::new();
    let (a, _) = k.hello("coder", json!({"action": "hello"}));
    let (b, _) = k.hello("coder", json!({"action": "hello"}));
    ok(&k.call(
        Some(a),
        "node",
        json!({"action": "create", "path": "/claims/lemma-04", "title": "g(4) <= 48",
               "kind": "claim", "take": false}),
    ));
    ok(&k.call(
        Some(a),
        "node",
        json!({"action": "close", "path": "/claims/lemma-04", "outcome": "done", "summary": "checked"}),
    ));
    let o = match &ok(&k.call(
        Some(a),
        "talk",
        json!({"action": "object", "on": "/claims/lemma-04", "text": "a counterexample at n = 11"}),
    ))
    .reply
    {
        Reply::Objected { objection } => objection.render(),
        other => panic!("{other:?}"),
    };
    let resolve = |k: &mut Kit, s: SessionId, upheld: bool| {
        k.call(
            Some(s),
            "talk",
            json!({"action": "resolve", "objection": o, "text": "the recount", "upheld": upheld}),
        )
    };
    let c = resolve(&mut k, a, true);
    let v = c.response.structured["entry"].as_str().unwrap().to_string();
    let c = resolve(&mut k, b, true);
    assert_eq!(c.response.structured["already"], json!(true));
    assert_eq!(c.response.structured["verdict"], json!(v));
    assert_eq!(c.response.structured["by"], json!(a.render()));
    let c = resolve(&mut k, b, false);
    let disputed = &c.response.structured["disputed"];
    assert_eq!(disputed["disputes"], json!(o));
    assert_eq!(disputed["by"], json!(a.render()));
    assert_eq!(
        c.response.structured["withdrawn"],
        json!({"node": "/claims/lemma-04", "was": "refuted", "outcome": "done"})
    );
    assert!(
        disputed["why"].as_str().unwrap().starts_with(&format!(
            "{o} was upheld by {v} ({}); your opposite verdict is recorded as the objection",
            a.render()
        )),
        "{}",
        c.response.text
    );
}

/// 1.10 (§10.2, §19, l6 of the lab's fifth round): an intent on a node under a pause warns
/// `paused`, with the pause's source and reason.
#[test]
fn an_intent_under_a_pause_names_the_pause() {
    let mut k = Kit::new();
    let (a, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(a),
        "node",
        json!({"action": "create", "path": "/payments/vat-svc", "title": "the VAT service"}),
    );
    let token = created_token(&c);
    ok(&k.call(
        Some(a),
        "control",
        json!({"action": "pause", "scope": "/payments",
               "reason": "Freeze for INC-311: no payments deploys until the incident lead says so"}),
    ));
    let c = k.call(
        Some(a),
        "hold",
        json!({"action": "checkpoint", "token": token, "done": "built the hotfix",
               "next_step": "deploy it", "intent": "deploy vat-svc v2.14.1 to prod"}),
    );
    let detail = warning_detail(&c, "paused");
    assert!(
        detail.starts_with(&format!(
            "recorded; the subtree is paused at /payments by coder/{} until ",
            a.render()
        )),
        "{detail}"
    );
    assert!(
        detail.ends_with(
            ": Freeze for INC-311: no payments deploys until the incident lead says so; make sure the \
             pause allows this"
        ),
        "{detail}"
    );
}

/// 1.9 (§20.4, the lab's fourth round, l1 F3): a row of a node another session holds reads `held
/// by s-N`, in `list` and `find` alike; the caller's own hold leaves it `open`.
#[test]
fn a_row_says_who_else_holds_its_node() {
    let mut k = Kit::new();
    let (a, _) = k.hello("coder", json!({"action": "hello"}));
    let (b, _) = k.hello("coder", json!({"action": "hello"}));
    ok(&k.call(
        Some(a),
        "node",
        json!({"action": "create", "path": "/held", "title": "a held node"}),
    ));
    let state = |k: &mut Kit, s: SessionId| -> Value {
        let c = k.call(Some(s), "view", json!({"action": "list"}));
        c.response.structured["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["path"] == json!("/held"))
            .unwrap()["state"]
            .clone()
    };
    assert_eq!(state(&mut k, a), json!("open"));
    assert_eq!(state(&mut k, b), json!(format!("held by {}", a.render())));
    let s = &k.w.s;
    let src = StoreSource::new(&k.w.conn);
    let n = forum_core::query::resolve(
        s,
        &NodeRef::Path(crate::decode::values::path("/held").unwrap()),
    )
    .unwrap();
    assert_eq!(
        super::hello::hit(s, n, 200, Some(b), &src).to_value()["state"],
        json!(format!("held by {}", a.render()))
    );
    assert_eq!(
        super::hello::hit(s, n, 200, Some(a), &src).to_value()["state"],
        json!("open")
    );
}

/// 1.9 (§9.2): no next call asks to confirm a `said`, so a brief shows an unconfirmed one as
/// possibly sent, with what to do.
#[test]
fn a_brief_shows_an_unconfirmed_said_as_possibly_sent() {
    let mut k = Kit::new();
    k.guided = true;
    let (s, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(s),
        "node",
        json!({"action": "create", "path": "/t", "title": "a thread", "keys": ["chat:x/1"]}),
    );
    let token = match &ok(&c).reply {
        Reply::Created { token: Some(t), .. } => t.render(),
        other => panic!("{other:?}"),
    };
    let c = k.call(
        Some(s),
        "talk",
        json!({"action": "said", "at": "/t", "text": "on it", "kind": "ack"}),
    );
    let said = c.response.structured["said_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        c.response.text.ends_with("next_calls: none"),
        "{}",
        c.response.text
    );
    ok(&k.call(
        Some(s),
        "hold",
        json!({"action": "release", "token": token, "done": "drafted", "next_step": "send it",
               "reason": "handoff", "unsent": [said]}),
    ));
    let c = k.call(Some(s), "view", json!({"action": "node", "path": "/t"}));
    assert!(
        c.response.text.contains(
            "possibly sent — check the conversation; confirm it with its link if it went out, else \
             send it first"
        ),
        "{}",
        c.response.text
    );
}

/// Rule 7's reason for a request the call itself recorded (§20.9, R3, R28, M-9): on a node nobody
/// holds, "nobody holds this node", now. 1.7: on a node the caller holds, no decision, but rule 6's
/// later reply, whose reason says to finish the work first, reply, then close.
#[test]
fn rule_7_s_reason_for_a_recorded_request_says_who_holds_its_node() {
    let mut k = Kit::new();
    k.guided = true;
    let (s, _) = k.hello("coder", json!({"action": "hello"}));
    let decide = |c: &Called| -> String {
        ok(c);
        let t = &c.response.text;
        t.lines()
            .find(|l| l.contains("talk decide {"))
            .unwrap_or_else(|| panic!("no decide: {t}"))
            .to_string()
    };
    // Held: the caller took /m at its create.
    ok(&k.call(
        Some(s),
        "node",
        json!({"action": "create", "path": "/m", "title": "m"}),
    ));
    let c = k.call(
        Some(s),
        "talk",
        json!({"action": "request", "at": "/m", "text": "please", "origin": "chat:m/1/1",
               "requester": "Bob"}),
    );
    ok(&c);
    let t = &c.response.text;
    assert!(!t.contains("talk decide {"), "{t}");
    let line = t
        .lines()
        .find(|l| l.contains("talk said {"))
        .unwrap_or_else(|| panic!("no reply: {t}"));
    assert!(line.contains(". later: talk said {"), "{line}");
    assert!(
        line.ends_with(
            " — once its work is done: reply with the result, then close, which settles it"
        ),
        "{line}"
    );
    assert!(!t.contains("nobody holds"), "{t}");
    // Unheld: /u was created without a take.
    ok(&k.call(
        Some(s),
        "node",
        json!({"action": "create", "path": "/u", "title": "u", "take": false}),
    ));
    let c = k.call(
        Some(s),
        "talk",
        json!({"action": "request", "at": "/u", "text": "thanks", "origin": "chat:u/1/1",
               "requester": "Bob"}),
    );
    let line = decide(&c);
    assert!(!line.contains("later: "), "{line}");
    assert!(
        line.ends_with(
            " — if it needs no further work: decide, then reply; nobody holds this node"
        ),
        "{line}"
    );
}

/// A guidance of the given items.
fn guidance(items: Vec<NextCall>, total: Total) -> Guidance {
    Guidance {
        items,
        total,
        end: None,
    }
}

fn item(rule: Rule, when: Moment, call: Skeleton, because: Because) -> NextCall {
    NextCall {
        rule,
        when,
        once: false,
        call,
        because,
    }
}

/// 1.9 (the lab's fourth round): rule 5's one item names its alternative in its `why`; a review
/// item's `why` says what `upheld` does; and the resolve's `upheld` is for the agent to write. In
/// the round, 1.7's two route items read as two steps, and no call passed `upheld`. 1.10 (the
/// fifth): the verdict is `stands`, and the `why` says what each value means for the claim; a
/// reviewer who found a claim true passed `upheld: true`, which would have refuted it.
#[test]
fn the_why_of_routes_and_reviews() {
    let l = long_forum();
    let s = &l.k.w.s;
    let (e, q, n) = (Eid(42), Eid(36), NodeId(18));
    assert_eq!(
        next_calls::why(
            s,
            &Because::RouteAnswers {
                entry: e,
                question: q,
                or: Some(RouteAlt::Node {
                    node: n,
                    held: false
                })
            }
        ),
        "if e42's words answer e36; else to #18 if it lives there"
    );
    assert_eq!(
        next_calls::why(s, &Because::AnsweredEntry { entry: e, or: None }),
        "if nothing else in e42 needs work: route it to none"
    );
    assert_eq!(
        next_calls::why(
            s,
            &Because::Unrouted {
                entry: e,
                related: None
            }
        ),
        "if e42 asks for work: create its node by key; if it needs none, route it to none"
    );
    // 1.12 (it6 l4 F1): the first related row is named beside the create, never routed to.
    assert_eq!(
        next_calls::why(
            s,
            &Because::Unrouted {
                entry: e,
                related: Some(NodeId(38))
            }
        ),
        "if new work: create by key; if only #38's work: route there; if answered: to_user, to none"
    );
    // 1.12 (it6 l5 F1): false says only that the objection fails; a reviewer that cannot tell
    // defers.
    assert_eq!(
        next_calls::why(s, &Because::Review { objection: Eid(5) }),
        "true: the claim is false (refuted or failed); false: the objection fails; cannot tell yet: \
         defer"
    );
    let c = next_calls::call(s, Some(l.sess), &Skeleton::Resolve { objection: Eid(5) });
    assert_eq!(c.fill, vec!["text", "stands"]);
    assert_eq!(
        next_calls::call_text(&c),
        format!(
            "talk resolve {{session: {}, objection: e5, text: ?, stands: ?}}",
            l.sess.render()
        )
    );
}

/// 1.6 (the lab's first round): rule 3's `why` at issue asks for an intent before an outside write
/// only, and says that reads need none; 1.8 (the third round): nor thread posts, whose said is
/// their record; 1.9 (the fourth round): nor local edits and tests. Rule 10's names the way out of an objection that stands, and rule 8's, after it,
/// the outcome `failed`.
#[test]
fn the_why_of_intents_and_objections() {
    let l = long_forum();
    let s = &l.k.w.s;
    let by = Time(1_790_812_800_000);
    assert_eq!(
        next_calls::why(s, &Because::CheckpointBy { by }),
        format!(
            "by {}; intent before a push, PR, deploy, job, mail; not local edits, tests, posts",
            format::time(by)
        )
    );
    // 1.9: the `why` names `upheld`, and a decision already retracted; 1.10: `stands`.
    assert_eq!(
        next_calls::why(
            s,
            &Because::Objection {
                objection: Eid(5),
                retracted: None
            }
        ),
        "if fixed: stands false; if it stands: decide or retract, stands true, then close refuted or \
         failed"
    );
    assert_eq!(
        next_calls::why(
            s,
            &Because::Objection {
                objection: Eid(5),
                retracted: Some(Eid(3))
            }
        ),
        "e3 is retracted: if the objection stands, stands true, close refuted or failed; else false"
    );
    assert_eq!(
        next_calls::why(s, &Because::Reclose { was: Outcome::Done }),
        "if done still stands, else failed if an objection stood: left open, it is offered as work"
    );
}

/// The text and the structured forms agree item by item; the JSON shape of
/// `structuredContent.next_calls`; `next_calls: none` for an empty list; `≥20` for a capped total
/// (D.12.6).
#[test]
fn the_block_renders_in_text_and_structured_forms() {
    let l = long_forum();
    let s = &l.k.w.s;
    let g = guidance(
        vec![
            item(
                Rule::Review,
                Moment::Now,
                Skeleton::Resolve { objection: Eid(8) },
                Because::Review { objection: Eid(8) },
            ),
            item(
                Rule::AskWait,
                Moment::Later,
                Skeleton::Release {
                    token: Token {
                        node: NodeId(12),
                        fence: Fence(1),
                    },
                    wait: Some(Eid(9)),
                },
                Because::AskWait {
                    question: Eid(9),
                    blocking: false,
                    existing: false,
                },
            ),
            item(
                Rule::HeldFlag,
                Moment::Now,
                Skeleton::Split { parent: l.node },
                Because::TooBig { attempts: 3 },
            ),
        ],
        Total::AtLeast(20),
    );
    let (text, json) = next_calls::block(s, Some(l.sess), &g);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.first(), Some(&"next_calls (3 of ≥20):"));
    let v = json.to_value();
    let obj = v.as_object().unwrap();
    assert_eq!(
        obj.keys().cloned().collect::<BTreeSet<_>>(),
        ["items", "total"].map(String::from).into()
    );
    assert_eq!(v["total"], json!("≥20"));
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    for (i, it) in items.iter().enumerate() {
        let keys: BTreeSet<String> = it.as_object().unwrap().keys().cloned().collect();
        assert_eq!(
            keys,
            ["tool", "args", "fill", "when", "why", "rule"]
                .map(String::from)
                .into()
        );
        let line = lines[i + 1];
        let prefix = format!("{}. ", i + 1);
        let rest = line.strip_prefix(&prefix).unwrap();
        let later = it["when"] == json!("later");
        assert_eq!(rest.starts_with("later: "), later, "{line}");
        let rest = rest.strip_prefix("later: ").unwrap_or(rest);
        let tool = it["tool"].as_str().unwrap();
        let action = it["args"]["action"].as_str().unwrap();
        assert!(rest.starts_with(&format!("{tool} {action} {{")), "{line}");
        for (k, x) in it["args"].as_object().unwrap() {
            if k == "action" {
                continue;
            }
            let shown = match x {
                Value::String(s) => format!("{k}: {s}"),
                Value::Array(xs) => format!(
                    "{k}: [{}]",
                    xs.iter()
                        .map(|y| y.as_str().unwrap().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                Value::Object(m) => format!(
                    "{k}: {{{}}}",
                    m.iter()
                        .map(|(a, b)| format!("{a}: {}", b.as_str().unwrap()))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                other => format!("{k}: {other}"),
            };
            assert!(rest.contains(&shown), "{shown} in {line}");
        }
        for f in it["fill"].as_array().unwrap() {
            let f = f.as_str().unwrap();
            if f == "name" {
                assert!(rest.contains("/?"), "{line}");
            } else {
                assert!(rest.contains(&format!("{f}: ?")), "{f} in {line}");
            }
        }
        assert!(
            rest.ends_with(&format!(" — {}", it["why"].as_str().unwrap())),
            "{line}"
        );
        assert!(it["rule"].is_string());
    }
    // The rule names of §20.9.
    assert_eq!(items[2]["rule"], json!("held_flag"));
    let (text, json) = next_calls::block(s, Some(l.sess), &guidance(Vec::new(), Total::Exact(0)));
    assert_eq!(text, "next_calls: none");
    assert_eq!(json.to_value(), json!({"items": [], "total": 0}));
}

/// The block is last: after the warnings, and after a `hello` that a first call prepends, which
/// it does not count against `hello.budget`; the shell's own answers carry none (D.12.6).
#[test]
fn the_block_follows_the_whole_response_and_the_shells_answers_carry_none() {
    let mut k = Kit::new();
    k.w.s.params.hello_budget = 1_024;
    let g = guidance(
        vec![
            item(
                Rule::Checkpoint,
                Moment::Later,
                Skeleton::Checkpoint {
                    token: Token {
                        node: NodeId(8),
                        fence: Fence(1),
                    },
                    intent_done: false,
                    intent: false,
                },
                Because::CheckpointBy { by: k.w.clock },
            ),
            item(
                Rule::MoreWork,
                Moment::Now,
                Skeleton::Next,
                Because::MoreWork { at_hello: false },
            ),
        ],
        Total::Exact(2),
    );
    let c = k.call_as(
        "coder",
        None,
        "node",
        json!({"action": "create", "path": "/a", "title": "first work"}),
        Some(&g),
    );
    let lines: Vec<&str> = c.response.text.lines().collect();
    let hello = lines
        .iter()
        .find_map(|l| l.strip_prefix("hello: "))
        .expect("a hello is prepended");
    assert!(hello.len() <= 1_024, "{} bytes", hello.len());
    let warnings = lines
        .iter()
        .position(|l| l.starts_with("warnings: "))
        .expect("session_opened");
    let block = lines
        .iter()
        .position(|l| l.starts_with("next_calls ("))
        .expect("the block");
    assert!(warnings < block);
    assert_eq!(lines.len(), block + 3, "the block is last");
    assert!(c.response.structured["next_calls"].is_object());
    assert!(c.response.structured["hello"].is_object());
    // The shell's own answers carry no block, with a session and without.
    let answer = crate::decode::ShellAnswer::TextTooLong {
        max: 64,
        longest: 65,
    };
    let src = StoreSource::new(&k.w.conn);
    let r = shell_refusal(
        &k.w.s,
        Some(SessionId(1)),
        k.w.clock,
        &k.health,
        &answer,
        &src,
        None,
    );
    assert!(r.text.starts_with("[swarm] "), "{}", r.text);
    assert!(!r.text.contains("next_calls"));
    assert!(r.is_error);
    let r = shell_refusal(&k.w.s, None, k.w.clock, &k.health, &answer, &src, None);
    assert!(r.text.starts_with("error: limit\nhint: "), "{}", r.text);
    assert!(!r.text.contains("next_calls"));
    // Without guidance (the usability switch), no block at all.
    let out = done(Reply::Status);
    let none = response(&Input {
        state: &k.w.s,
        now: k.w.clock,
        session: Some(SessionId(1)),
        opened: None,
        cmd: &Cmd::View(ViewCmd::Constitution),
        node_args: &[],
        out: &out,
        guidance: None,
        health: &k.health,
        health_at: k.w.clock,
        hits: &k.hits,
        masked_fields: 0,
        ignored: None,
        src: &src,
        seen: None,
        rebind: None,
    });
    assert!(!none.text.contains("next_calls"));
    assert!(none.structured.get("next_calls").is_none());
}

/// What `fill` and the `?` of a next call mean (1.4, R20) is in the `session` description, since
/// no tool has an output schema (1.7, §20.1).
#[test]
fn the_session_description_says_what_fill_means() {
    for t in crate::mcp::tools::tools() {
        assert!(t.output_schema.is_none(), "{}", t.name);
    }
    assert!(
        crate::mcp::tools::SESSION
            .contains("you write each <arg>: ?, and an <arg>?: ? only if it applies")
    );
}

// ------------------------------------------------------------------------------------------------
// Budgets (D.11.7, §10.4, §20.3)

/// The shape of a node for the brief's budget property.
#[derive(Clone, Debug)]
struct BriefShape {
    budget: u32,
    digest: usize,
    decisions: Vec<usize>,
    requests: Vec<usize>,
    saids: Vec<usize>,
    notes: Vec<usize>,
    checkpoint: Option<usize>,
    predecessors: Vec<usize>,
}

/// A world with one session holding a node of `shape`, and the node.
fn brief_world(shape: &BriefShape) -> (Kit, SessionId, NodeId) {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/n", "keys": ["chat:team/1"], "title": "a node",
               "digest": filler("d", shape.digest.min(2_048)), "body": filler("b", 600)}),
    );
    let token = match &ok(&c).reply {
        Reply::Created { token: Some(t), .. } => *t,
        other => panic!("{other:?}"),
    };
    let n = k.node("/n");
    for (i, len) in shape.predecessors.iter().enumerate() {
        let path = format!("/old-{i}");
        let c = k.call(
            Some(s1),
            "node",
            json!({"action": "create", "path": path, "title": "an earlier attempt", "take": false}),
        );
        ok(&c);
        let c = k.call(
            Some(s1),
            "node",
            json!({"action": "close", "path": path, "outcome": "failed", "summary": filler("s", (*len).max(1))}),
        );
        ok(&c);
        let old = k.node(&path);
        k.events(|d| {
            d.push(Ev::LinkAdded {
                node: n,
                rel: LinkRel::Continues,
                to: old,
            });
        });
    }
    let shape = shape.clone();
    k.events(|d| {
        for len in &shape.requests {
            let origin = format!("chat:team/1/m{}", d.s.counters.next_eid.0);
            request_event(d, n, s1, &origin, "chat:team/1", &filler("r", *len), "Bob");
        }
        for len in &shape.saids {
            append(
                d,
                n,
                s1,
                EntryBody::Said {
                    conv: keystr("chat:team/1"),
                    kind: SaidKind::Reply,
                    corrects: None,
                    voice: false,
                },
                text(&filler("y", *len), &[]),
            );
        }
        for len in &shape.decisions {
            append(
                d,
                n,
                s1,
                EntryBody::Decision {
                    settles: BTreeSet::new(),
                    supersedes: None,
                    assumes: BTreeSet::new(),
                    evidence: None,
                    basis: vec![BasisRef::External {
                        checked_at: d.clock,
                        recheck: None,
                    }],
                },
                text(
                    &filler("x", *len),
                    &[(TextField::Basis, &filler("w", *len / 4))],
                ),
            );
        }
        for len in &shape.notes {
            append(
                d,
                n,
                s1,
                EntryBody::Note {
                    to: None,
                    until: None,
                    recipients: BTreeSet::new(),
                    pending_holder: false,
                    expects: None,
                    reply_to: None,
                },
                text(&filler("z", *len), &[]),
            );
        }
    });
    if let Some(len) = shape.checkpoint {
        let c = k.call(
            Some(s1),
            "hold",
            json!({"action": "checkpoint", "token": token.render(), "done": filler("c", len.max(1)),
                   "next_step": "go on"}),
        );
        ok(&c);
    }
    k.w.s.params.brief_budget = shape.budget;
    (k, s1, n)
}

fn brief_shape() -> impl proptest::strategy::Strategy<Value = BriefShape> {
    use proptest::prelude::*;
    let lens = |n: usize, max: usize| proptest::collection::vec(0..max, 0..n);
    (
        1_024_u32..20_000,
        // An empty digest a tenth of the time: the header then carries `body_head` (1.4, §10.4).
        prop_oneof![1 => Just(0_usize), 9 => 0_usize..2_048],
        lens(12, 3_000),
        lens(8, 3_000),
        lens(8, 2_000),
        lens(12, 2_000),
        proptest::option::of(1_usize..5_000),
        lens(4, 3_000),
    )
        .prop_map(
            |(budget, digest, decisions, requests, saids, notes, checkpoint, predecessors)| {
                BriefShape {
                    budget,
                    digest,
                    decisions,
                    requests,
                    saids,
                    notes,
                    checkpoint,
                    predecessors,
                }
            },
        )
}

/// A rendered total, `12` or `"≥12"`, as its number.
fn total_of(v: &Value) -> u64 {
    match v {
        Value::Number(n) => n.as_u64().unwrap(),
        Value::String(s) => s.strip_prefix('≥').unwrap().parse().unwrap(),
        other => panic!("not a total: {other}"),
    }
}

/// The checks of one brief against its budget: within it unless only the minimum is shown; every
/// section present in §10.4's order with `shown ≤ total`; a non-empty section shows its newest
/// item; and the JSON is valid.
fn check_brief(j: &json::Json, extra: usize, budget: usize) {
    let text = j.text();
    let v: Value = serde_json::from_str(&text).expect("valid JSON");
    assert!(
        j.size() <= budget || extra == 0,
        "{} bytes over a budget of {budget} with {extra} items beyond the minimum",
        j.size()
    );
    // A brief with no section has no `sections` (1.7, §20.1).
    let none = Vec::new();
    let sections = v["sections"].as_array().unwrap_or(&none);
    let names: Vec<&str> = sections
        .iter()
        .map(|x| x["name"].as_str().unwrap())
        .collect();
    // §10.4's order, with the empty sections left out (1.6).
    let order = [
        "flags",
        "unfinished",
        "said_and_commitments",
        "decisions_and_agreements",
        "unsettled_requests",
        "objections",
        "digest",
        "last_checkpoint",
        "everything_else",
    ];
    let mut rest = order.iter();
    for name in &names {
        assert!(rest.any(|x| x == name), "{names:?} in §10.4's order");
    }
    for x in sections {
        let items = x["items"].as_array().unwrap().len() as u64;
        // 1.7 (§18): a section shown whole carries no counts; one cut short carries both.
        assert_eq!(x.get("shown").is_some(), x.get("total").is_some(), "{x}");
        let (shown, total) = match x.get("shown") {
            Some(k) => (k.as_u64().unwrap(), total_of(&x["total"])),
            None => (items, items),
        };
        assert_eq!(shown, items);
        assert!(shown <= total, "{x}");
        if x.get("shown").is_some() {
            assert!(
                shown < total || x["total"].is_string(),
                "only a section cut short carries counts: {x}"
            );
        }
        assert!(total > 0, "an empty section is left out: {x}");
        if x["name"] != "everything_else" {
            assert!(shown >= 1, "the newest item of every section: {x}");
        }
    }
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig {
        cases: 48,
        failure_persistence: None,
        ..proptest::prelude::ProptestConfig::default()
    })]

    /// The brief stays within `brief.budget`, or shows only each section's newest item, cut to
    /// its share; a section cut short shows `shown/total` (§10.4, §18, D.11.7).
    #[test]
    fn the_brief_keeps_its_budget(shape in brief_shape()) {
        let (k, s1, n) = brief_world(&shape);
        let src = StoreSource::new(&k.w.conn);
        let (j, extra) = brief::render_brief(&k.w.s, n, Some(s1), k.w.clock, &src);
        check_brief(&j, extra, usize::try_from(shape.budget).unwrap());
        // With room for everything, every item read is shown whole: nothing but the header's
        // `body_head`, which is the start of the body by definition, whatever the budget (1.4,
        // §10.4), is cut.
        let mut roomy = k.w.s.clone();
        roomy.params.brief_budget = u32::MAX;
        let (all, _) = brief::render_brief(&roomy, n, Some(s1), k.w.clock, &src);
        assert!(!without_body_head(&all).contains(format::ELLIPSIS));
        // The full view holds that whole brief, every section with its shown and total.
        let full = brief::view_node(&k.w.s, n, ViewMode::Full, 0, Some(s1), k.w.clock, &src);
        assert_eq!(full.get("brief"), Some(&j));
    }
}

/// The text of a rendered brief without its header's `body_head`, which is cut at
/// `body_head_max` bytes whatever the budget (1.4, §10.4).
fn without_body_head(j: &json::Json) -> String {
    let mut v: Value = serde_json::from_str(&j.text()).unwrap();
    if let Some(node) = v.get_mut("node").and_then(Value::as_object_mut) {
        node.remove("body_head");
    }
    v.to_string()
}

/// The proptest's flake (`the_brief_keeps_its_budget`): with an empty digest the header carries
/// `body_head`, the start of the body (1.4, §10.4), and, with room for everything, the brief holds
/// an ellipsis only there, if anywhere. The property drew an empty digest once in about 2,000
/// shapes. Its body of 600 bytes is whole since 1.5 (R3): `body_head` keeps up to 2 KiB.
#[test]
fn a_brief_with_an_empty_digest_cuts_only_the_head_of_its_body() {
    let shape = BriefShape {
        budget: 20_000,
        digest: 0,
        decisions: vec![],
        requests: vec![],
        saids: vec![],
        notes: vec![],
        checkpoint: None,
        predecessors: vec![],
    };
    let (k, s1, n) = brief_world(&shape);
    let src = StoreSource::new(&k.w.conn);
    let mut roomy = k.w.s.clone();
    roomy.params.brief_budget = u32::MAX;
    let (all, _) = brief::render_brief(&roomy, n, Some(s1), k.w.clock, &src);
    let v: Value = serde_json::from_str(&all.text()).unwrap();
    let head = v["node"]["body_head"].as_str().unwrap();
    assert_eq!(head, filler("b", 600), "the whole body, within 2 KiB");
    assert!(!without_body_head(&all).contains(format::ELLIPSIS));
}

/// 1.5 (R3, D.14.7): `body_head` keeps up to 2 KiB of the body, at most `digest.max` and a quarter
/// of `brief.budget`, the note included, and says how much it cut: `<shown> of <total> bytes;
/// view node full for the rest`; under a bound too small for the note, it is cut with "…" alone.
/// In round R3, a cut at 256 bytes sent 15 sessions of 16 to read the node again at once.
#[test]
fn the_body_head_keeps_2_kib_and_says_how_much_it_cut() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let long = filler("b", 5_000);
    let wide = "é".repeat(3_000);
    for (name, body) in [("long", long.as_str()), ("wide", wide.as_str())] {
        ok(&k.call(
            Some(s1),
            "node",
            json!({"action": "create", "path": format!("/{name}"), "title": name, "body": body,
                   "take": false}),
        ));
    }
    ok(&k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/short", "title": "short", "body": filler("b", 2_048),
               "take": false}),
    ));
    let head = |k: &Kit, p: &str| -> String {
        let src = StoreSource::new(&k.w.conn);
        let (j, _) = brief::render_brief(&k.w.s, k.node(p), Some(s1), k.w.clock, &src);
        let v: Value = serde_json::from_str(&j.text()).unwrap();
        v["node"]["body_head"].as_str().unwrap().to_string()
    };
    // The defaults: 2 KiB, note included.
    assert_eq!(brief::body_head_max(&k.w.s.params), 2_048);
    let h = head(&k, "/long");
    assert!(h.len() <= 2_048, "{}", h.len());
    let (text, note) = h.rsplit_once(format::ELLIPSIS).unwrap();
    assert!(long.starts_with(text));
    assert_eq!(
        note,
        format!(
            " ({} of 5000 bytes; view node full for the rest)",
            text.len()
        )
    );
    assert!(
        text.len() > 1_900,
        "most of the 2 KiB is the body: {}",
        text.len()
    );
    // A body of 2 KiB is whole.
    assert_eq!(head(&k, "/short"), filler("b", 2_048));
    // A cut never splits a character.
    let h = head(&k, "/wide");
    let (text, note) = h.rsplit_once(format::ELLIPSIS).unwrap();
    assert!(wide.starts_with(text) && text.len() % 2 == 0);
    assert!(
        note.starts_with(&format!(" ({} of 6000 bytes;", text.len())),
        "{note}"
    );
    // At most digest.max, and a quarter of brief.budget.
    k.w.s.params.digest_max = 1_000;
    assert!(head(&k, "/long").len() <= 1_000);
    k.w.s.params.digest_max = 2_048;
    k.w.s.params.brief_budget = 1_024;
    let h = head(&k, "/long");
    assert!(h.len() <= 256, "{}", h.len());
    assert!(
        h.ends_with("of 5000 bytes; view node full for the rest)"),
        "{h}"
    );
    // A digest.max too small for the note, which `Params::valid` allows: the body cut with "…"
    // alone, within the bound, never the note without the body.
    k.w.s.params.digest_max = 16;
    assert_eq!(head(&k, "/long"), format!("{}…", &long[..13]));
    // The unit: whole, or cut with its note within the bound.
    assert_eq!(brief::body_head("abc", 3), "abc");
    let x = "x".repeat(500);
    for max in 4..=120 {
        let h = brief::body_head(&x, max);
        assert!(h.len() <= max, "{max}: {h}");
        assert!(h.starts_with('x'), "{max}: some of the body shows: {h}");
        assert!(h.contains(format::ELLIPSIS), "{max}: {h}");
    }
    assert_eq!(brief::body_head(&"é".repeat(10), 4), format::ELLIPSIS);
    let h = brief::body_head(&"x".repeat(100), 60);
    assert_eq!(
        h,
        format!(
            "{}… (9 of 100 bytes; view node full for the rest)",
            "x".repeat(9)
        )
    );
    assert!(h.len() <= 60);
}

/// 1.5 (R3, D.14.7): the brief's note on the last expired hold says what the server knows: the
/// holder, when its hold expired, the node's last checkpoint and its time, and the open intent if
/// any. In round R3 every taker after such a hold read "check before you repeat it", and three
/// could not tell what to check.
#[test]
fn an_expired_holds_note_says_what_the_server_knows() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let token = |c: &Called| match &ok(c).reply {
        Reply::Created { token: Some(t), .. } | Reply::Taken { token: t, .. } => *t,
        other => panic!("{other:?}"),
    };
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/work", "title": "Fix the parser"}),
    );
    let t1 = token(&c);
    ok(&k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/quiet", "title": "Tidy", "take": false}),
    ));
    // s-1 checkpoints with an intent, and leaves it open when it releases.
    k.at(60_000);
    ok(&k.call(
        Some(s1),
        "hold",
        json!({"action": "checkpoint", "token": t1.render(), "done": "found the off-by-one",
               "next_step": "post the fix", "intent": "post the fix to the team thread"}),
    ));
    let intent = k.w.s.nodes[&k.node("/work")].open_intent.unwrap();
    k.at(60_000);
    ok(&k.call(
        Some(s1),
        "hold",
        json!({"action": "release", "token": t1.render(), "done": "wrote the fix",
               "next_step": "post it", "reason": "handoff"}),
    ));
    // The release's entry is the node's last checkpoint (§10.3).
    let checkpointed = k.w.clock;
    let checkpoint = k.w.s.nodes[&k.node("/work")].last_checkpoint.unwrap();
    assert_ne!(checkpoint, intent.eid);
    // s-2 takes both nodes, records nothing, and its holds expire.
    let (s2, _) = k.hello("coder", json!({"action": "hello"}));
    for p in ["/work", "/quiet"] {
        token(&k.call(Some(s2), "hold", json!({"action": "take", "target": p})));
    }
    let taken = k.w.clock;
    let lease = k.w.s.params.lease_first;
    let expired = taken.plus(lease);
    k.at(lease.0 + 60_000);
    assert!(k.w.catch_up(k.w.clock));
    let (s3, _) = k.hello("coder", json!({"action": "hello"}));
    let item = |k: &mut Kit, p: &str| -> Value {
        let c = k.call(Some(s3), "view", json!({"action": "node", "path": p}));
        let v = c.response.structured.clone();
        let unfinished = v["brief"]["sections"]
            .as_array()
            .or_else(|| v["sections"].as_array())
            .unwrap()
            .iter()
            .find(|x| x["name"] == "unfinished")
            .unwrap()
            .clone();
        unfinished["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x.get("expired_hold").is_some())
            .unwrap_or_else(|| panic!("{unfinished}"))
            .clone()
    };
    let w = item(&mut k, "/work");
    assert_eq!(w["expired_hold"], json!(s2.render()));
    assert_eq!(w["at"], json!(format::time(expired)));
    assert_eq!(w["last_checkpoint"], json!(checkpoint.render()));
    assert_eq!(w["last_checkpoint_at"], json!(format::time(checkpointed)));
    assert_eq!(w["open_intent"], json!(intent.eid.render()));
    assert_eq!(
        w["note"],
        json!(format!(
            "{} held this node until {}, when its hold expired; it recorded no checkpoint and no \
             intent, and left no said entry here unconfirmed; an action it did not record would \
             show only outside the forum; the last checkpoint is {}, at {} by {}; intent {} by {} \
             is still open: check whether its action happened before you repeat it",
            s2.render(),
            format::time(expired),
            checkpoint.render(),
            format::time(checkpointed),
            s1.render(),
            intent.eid.render(),
            s1.render()
        ))
    );
    assert!(
        !w.to_string().contains(&format!("\"{}.", k.node("/work").0)),
        "never a token: {w}"
    );
    let q = item(&mut k, "/quiet");
    assert_eq!(q["expired_hold"], json!(s2.render()));
    assert_eq!(q["last_checkpoint"], Value::Null);
    assert!(q.get("last_checkpoint_at").is_none());
    assert_eq!(q["open_intent"], Value::Null);
    assert!(
        q["note"]
            .as_str()
            .unwrap()
            .ends_with("; the node has no checkpoint; no intent is open"),
        "{q}"
    );
    // The last checkpoint's id is the node's, even where the shell's tables lack its entry: the
    // note then gives no time rather than saying that the node has no checkpoint.
    let (j, _) = brief::render_brief(
        &k.w.s,
        k.node("/work"),
        Some(s3),
        k.w.clock,
        &MemorySource::new(),
    );
    let v: Value = serde_json::from_str(&j.text()).unwrap();
    let w = v["sections"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|x| x["items"].as_array().unwrap().iter())
        .find(|x| x.get("expired_hold").is_some())
        .unwrap()
        .clone();
    assert_eq!(w["last_checkpoint"], json!(checkpoint.render()));
    assert!(w.get("last_checkpoint_at").is_none(), "{w}");
    assert!(
        w["note"].as_str().unwrap().contains(&format!(
            "; the last checkpoint is {}; intent ",
            checkpoint.render()
        )),
        "{w}"
    );
}

/// 1.8 (§20.4, §10.4, §10.2): an open intent whose session is gone is marked on its node's row
/// and in its brief, and any session records what became of it with `hold check {target,
/// intent_done}`, holding nothing; the marks then go. In the lab's sixth task of every round, a
/// vanished session's intent stayed open after its action had happened, since only a checkpoint
/// of a hold on its node could close it.
#[test]
fn an_open_intent_of_a_gone_session_is_marked_until_a_check_records_it() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/work", "title": "Fix the parser"}),
    );
    let t1 = match &ok(&c).reply {
        Reply::Created { token: Some(t), .. } => *t,
        other => panic!("{other:?}"),
    };
    ok(&k.call(
        Some(s1),
        "hold",
        json!({"action": "checkpoint", "token": t1.render(), "done": "found the off-by-one",
               "next_step": "post the fix", "intent": "post the fix to the team thread"}),
    ));
    let intent = k.w.s.nodes[&k.node("/work")].open_intent.unwrap();
    ok(&k.call(
        Some(s1),
        "hold",
        json!({"action": "release", "token": t1.render(), "done": "wrote the fix",
               "next_step": "post it", "reason": "handoff"}),
    ));
    let row = |k: &mut Kit, s: SessionId| -> Value {
        let c = k.call(Some(s), "view", json!({"action": "list"}));
        c.response.structured["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["path"] == json!("/work"))
            .cloned()
            .unwrap()
    };
    // While s-1 is present, its intent is not marked.
    let (s2, _) = k.hello("coder", json!({"action": "hello"}));
    assert_eq!(row(&mut k, s2).get("intent"), None);
    // s-1 falls silent: its intent is marked on the row and in the brief.
    k.at(k.w.s.params.presence_silence.0 + 60_000);
    assert!(k.w.catch_up(k.w.clock));
    let (s3, _) = k.hello("coder", json!({"action": "hello"}));
    assert_eq!(row(&mut k, s3)["intent"], json!(entries::GONE_INTENT));
    let c = k.call(Some(s3), "view", json!({"action": "node", "path": "/work"}));
    let v = c.response.structured.clone();
    let item = v["brief"]["sections"]
        .as_array()
        .or_else(|| v["sections"].as_array())
        .unwrap()
        .iter()
        .flat_map(|x| x["items"].as_array().cloned().unwrap_or_default())
        .find(|x| x.get("intent") == Some(&json!(intent.eid.render())))
        .unwrap_or_else(|| panic!("{v}"));
    assert_eq!(item["gone"], json!(entries::GONE_INTENT_BRIEF));
    // s-3 records what it found without a hold; the intent closes, and the marks go.
    let c = k.call(
        Some(s3),
        "hold",
        json!({"action": "check", "target": "/work", "intent_done": "the fix is in the thread"}),
    );
    ok(&c);
    assert_eq!(
        c.response.structured["intent_done"],
        json!(intent.eid.render())
    );
    assert!(c.response.structured["entry"].is_string());
    assert_eq!(k.w.s.nodes[&k.node("/work")].open_intent, None);
    assert!(k.w.s.holds.values().all(|h| h.session != s3));
    assert_eq!(row(&mut k, s3).get("intent"), None);
}

/// 1.5 (R3, D.14.7): a holder that recorded a `said` and vanished announced an outside action
/// without a checkpoint: the note on its expired hold names the `said` entries it left
/// unconfirmed, never that it announced none, and those of another session are not its.
#[test]
fn an_expired_holds_note_names_the_saids_it_left_unconfirmed() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    ok(&k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/thread", "title": "Answer Bob",
               "keys": ["chat:team/1"], "take": false}),
    ));
    let n = k.node("/thread");
    let (s2, _) = k.hello("coder", json!({"action": "hello"}));
    ok(&k.call(
        Some(s2),
        "hold",
        json!({"action": "take", "target": "/thread"}),
    ));
    // s-1 and s-2 each record a reply they are about to send (talk said is WP-D5b's: the events
    // stand in for it).
    let conv = keystr("chat:team/1");
    let mut saids = Vec::new();
    k.events(|d| {
        for by in [s1, s2, s2] {
            saids.push(append(
                d,
                n,
                by,
                EntryBody::Said {
                    conv: conv.clone(),
                    kind: SaidKind::Reply,
                    corrects: None,
                    voice: false,
                },
                text("the fix is out", &[]),
            ));
        }
    });
    let lease = k.w.s.params.lease_first;
    k.at(lease.0 + 60_000);
    assert!(k.w.catch_up(k.w.clock));
    assert!(k.w.s.nodes[&n].expired_hold.is_some(), "s-2's hold expired");
    let (s3, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(s3),
        "view",
        json!({"action": "node", "path": "/thread"}),
    );
    let v = &c.response.structured;
    let item = v["sections"]
        .as_array()
        .or_else(|| v["brief"]["sections"].as_array())
        .unwrap()
        .iter()
        .flat_map(|x| x["items"].as_array().unwrap().iter())
        .find(|x| x.get("expired_hold").is_some())
        .unwrap_or_else(|| panic!("{v}"))
        .clone();
    let note = item["note"].as_str().unwrap();
    assert!(
        note.contains(&format!(
            "; it recorded no checkpoint and no intent, and left said entries here unconfirmed \
             ({}, {}): check in their conversations whether they went out before you send them \
             again; the node has no checkpoint",
            saids[1].render(),
            saids[2].render()
        )),
        "{note}"
    );
    assert!(
        !note.contains(&saids[0].render()),
        "s-1's said is not s-2's: {note}"
    );
    assert!(!note.contains("announced"), "{note}");
}

/// Over the budget, each section's newest item is cut to an equal share of what remains, and the
/// brief fits; with a little more room, further items come in section order, newest first.
#[test]
fn the_brief_cuts_its_newest_items_to_equal_shares() {
    let shape = BriefShape {
        budget: 6_000,
        digest: 2_000,
        decisions: vec![3_000; 4],
        requests: vec![3_000; 4],
        saids: vec![2_000; 3],
        notes: vec![2_000; 5],
        checkpoint: Some(4_000),
        predecessors: vec![3_000],
    };
    let (k, s1, n) = brief_world(&shape);
    let src = StoreSource::new(&k.w.conn);
    let (j, extra) = brief::render_brief(&k.w.s, n, Some(s1), k.w.clock, &src);
    check_brief(&j, extra, 6_000);
    assert_eq!(extra, 0, "only the newest item of each section");
    assert!(j.size() <= 6_000, "{} bytes: {}", j.size(), j.text());
    let v: Value = serde_json::from_str(&j.text()).unwrap();
    for x in v["sections"].as_array().unwrap() {
        if let Some(first) = x["items"].as_array().unwrap().first() {
            let t = first.to_string();
            assert!(t.contains(format::ELLIPSIS), "cut to its share: {t}");
        }
    }
    // One predecessor, shown whole: no counts (1.7, §18).
    assert_eq!(v["continues"]["items"].as_array().unwrap().len(), 1);
    assert!(v["continues"].get("shown").is_none());
    // A budget with room for the newest items whole, and a few more.
    let mut more = k.w.s.clone();
    more.params.brief_budget = 40_000;
    let (j, extra) = brief::render_brief(&more, n, Some(s1), k.w.clock, &src);
    check_brief(&j, extra, 40_000);
    assert!(extra > 0);
    let v: Value = serde_json::from_str(&j.text()).unwrap();
    let decisions = &section_named(&v, "decisions_and_agreements")["items"];
    let first = decisions[0]["decision"].as_str().unwrap();
    let second = decisions[1]["decision"].as_str().unwrap();
    assert!(
        Eid::parse(first).unwrap() > Eid::parse(second).unwrap(),
        "newest first"
    );
}

/// The section of a rendered brief by its name; a brief leaves an empty one out (1.6, §10.4).
fn section_named<'v>(v: &'v Value, name: &str) -> &'v Value {
    v["sections"]
        .as_array()
        .and_then(|xs| xs.iter().find(|x| x["name"] == json!(name)))
        .unwrap_or_else(|| panic!("no section {name} in {v}"))
}

/// The shape of a session's `hello` for its budget property.
#[derive(Clone, Debug)]
struct HelloShape {
    budget: u32,
    notes: Vec<usize>,
    offers: Vec<usize>,
    others: usize,
    map: usize,
    questions: Vec<usize>,
    digest: Vec<usize>,
    /// The options of each question (E3-bugs-10).
    options: usize,
}

fn hello_shape() -> impl proptest::strategy::Strategy<Value = HelloShape> {
    use proptest::prelude::*;
    let lens = |n: usize, max: usize| proptest::collection::vec(0..max, 0..n);
    (
        512_u32..8_192,
        lens(10, 2_000),
        lens(5, 500),
        0_usize..12,
        0_usize..2_048,
        lens(8, 3_000),
        lens(8, 3_000),
        0_usize..40,
    )
        .prop_map(
            |(budget, notes, offers, others, map, questions, digest, options)| HelloShape {
                budget,
                notes,
                offers,
                others,
                map,
                questions,
                digest,
                options,
            },
        )
}

/// A world with an assistant that serves alice, the session, and the notes addressed to it,
/// which its `session hello` delivers (the core's `inbox` query is WP-D5b's).
fn hello_world(shape: &HelloShape) -> (Kit, SessionId, Vec<Eid>) {
    let mut k = Kit::new();
    let (a, _) = k.hello(
        "coder",
        json!({"action": "hello", "principal": "mail:alice@example.org"}),
    );
    let (w, _) = k.hello("worker", json!({"action": "hello"}));
    for _ in 0..shape.others {
        k.hello("worker", json!({"action": "hello"}));
    }
    for (i, len) in shape.offers.iter().enumerate() {
        let c = k.call_as(
            "worker",
            Some(w),
            "node",
            json!({"action": "create", "path": format!("/offer-{i}"), "title": filler("o", (*len).max(1)),
                   "tags": ["offer"], "take": false}),
            None,
        );
        ok(&c);
    }
    let alice = k.w.s.sessions[&a]
        .principals
        .iter()
        .copied()
        .next()
        .unwrap();
    let shape2 = shape.clone();
    let mut notes = Vec::new();
    k.events(|d| {
        if shape2.map > 0 {
            d.push(Ev::NodeUpdated {
                node: ROOT,
                patch: forum_core::NodePatch {
                    digest: Some(t(&filler("m", shape2.map))),
                    agreements: (shape2.map > 1).then(|| t(&filler("h", shape2.map / 2))),
                    ..forum_core::NodePatch::default()
                },
            });
        }
        let until = d.clock.plus(d.s.params.deliver_window);
        for len in &shape2.notes {
            let note = append(
                d,
                ROOT,
                w,
                EntryBody::Note {
                    to: Some(Addressee::Session(a)),
                    until: Some(until),
                    recipients: [a].into(),
                    pending_holder: false,
                    expects: None,
                    reply_to: None,
                },
                text(&filler("n", *len), &[]),
            );
            notes.push(note);
        }
        let due = d.clock.plus(d.s.params.user_question_due);
        let mut qs = Vec::new();
        for len in &shape2.questions {
            let q = append(
                d,
                ROOT,
                w,
                EntryBody::Question(QuestionBody::User {
                    source: UqSource::Agent {
                        session: w,
                        digest: ReqDigest([u8::try_from(qs.len()).unwrap(); 32]),
                    },
                    about: None,
                    to: vec![alice, USER_PRINCIPAL],
                    due,
                    has_default: false,
                }),
                text(
                    &filler("q", *len),
                    &vec![(TextField::Option, filler("o", *len / 2)); shape2.options.max(1)]
                        .iter()
                        .map(|(f, x)| (*f, x.as_str()))
                        .collect::<Vec<_>>(),
                ),
            );
            qs.push(q);
        }
        for (i, _) in shape2.digest.iter().enumerate() {
            let Some(q) = qs.get(i % qs.len().max(1)).copied() else {
                break;
            };
            append(
                d,
                USER_DIGEST,
                w,
                EntryBody::System(SystemNote::Digest {
                    item: DigestItem::Lapsed { question: q },
                    to: alice,
                }),
                text("", &[]),
            );
        }
    });
    k.w.s.params.hello_budget = shape.budget;
    (k, a, notes)
}

/// At the smallest budget, every droppable section goes, and the sections that are never dropped
/// stay, the inbox whole among them, the minimum winning over the budget (§20.3, A-73; 1.5: the
/// inbox is never cut).
#[test]
fn hello_drops_sections_in_order_and_never_cuts_the_inbox() {
    let shape = HelloShape {
        budget: 512,
        notes: vec![300; 6],
        offers: vec![200; 3],
        others: 4,
        map: 1_500,
        questions: vec![20; 7],
        digest: vec![10; 7],
        options: 1,
    };
    let (k, a, notes) = hello_world(&shape);
    let src = StoreSource::new(&k.w.conn);
    let input = hello::HelloInput {
        delivered: Some(&notes),
        user_entry: None,
        hits: &k.hits,
        route: &hello::RouteCall::default(),
        first: true,
    };
    let h = hello::render_hello(&k.w.s, a, k.w.clock, &k.health, &input, &src);
    let v: Value = serde_json::from_str(&h.text()).unwrap();
    // The sections that remain, in §20.3's order, the session first (1.5, R3).
    let text = h.text();
    assert!(
        text.starts_with(&format!("{{\"session\":\"{}\",", a.render())),
        "{text}"
    );
    // The session holds nothing and has no caps, so `holds` and `caps` are left out (1.7), and
    // `cut` names every droppable section, in the order the budget dropped them.
    let order = [
        "\"session\"",
        "\"principals\"",
        "\"inbox\"",
        "\"user_decisions\"",
        "\"user_digest\"",
        "\"cut\"",
    ];
    let at: Vec<usize> = order.iter().map(|k| text.find(k).unwrap()).collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "{text}");
    assert_eq!(v.as_object().unwrap().len(), order.len(), "{text}");
    let inbox: Vec<&str> = v["inbox"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["e"].as_str().unwrap())
        .collect();
    let all: Vec<String> = notes.iter().map(|e| e.render()).collect();
    assert_eq!(inbox, all, "every delivered note, oldest first");
    assert!(
        v["inbox"].get("shown").is_none(),
        "shown whole, with no counts"
    );
    // No search ran, so `related` and `knowledge` were empty, left out, and not cut.
    assert_eq!(
        v["cut"],
        json!(["constitution", "present", "orientation", "offers"])
    );
    assert!(h.size() > 512, "the minimum wins over the budget");
    assert_eq!(v["user_decisions"]["count"], json!(7));
    assert_eq!(v["user_decisions"]["items"].as_array().unwrap().len(), 5);
    assert_eq!(v["user_digest"]["count"], json!(7));
}

/// 1.5 (R3, D.14.7): `invalid(BadParams)`'s hint no longer cites "a rule of the constitution". It
/// named the call and the core package of a command the core still answered with a later
/// package's placeholder; WP-D6 was the last such package, so no stub is left: `node create` with
/// `repeat` and `node update` of `repeat` succeed, also as a first call, `stub_of` names none, and
/// any `invalid(BadParams)` says only that the arguments do not fit the call.
#[test]
fn no_stub_is_left_and_bad_params_says_the_arguments_do_not_fit() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    ok(&k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/plain", "title": "Tidy", "take": false}),
    ));
    let repeat = json!({"cron": "0 9 * * *", "run_within": "1h"});
    let calls = [
        (
            "node",
            json!({"action": "create", "path": "/daily", "title": "d", "repeat": repeat,
                   "take": false}),
        ),
        (
            "node",
            json!({"action": "update", "path": "/plain", "version": 1, "repeat": repeat}),
        ),
    ];
    for (tool, args) in calls {
        let cmd = decode_args(tool, args.clone(), &k);
        assert_eq!(errors::stub_of(&cmd), None, "{args}");
        let c = k.call(Some(s1), tool, args.clone());
        assert!(c.out.is_ok(), "{args}: {:?}", c.out);
    }
    // A first call that creates a series opens its session and succeeds.
    let c = k.call(
        None,
        "node",
        json!({"action": "create", "path": "/weekly", "title": "w", "repeat": repeat,
               "take": false}),
    );
    assert!(c.out.is_ok(), "{:?}", c.out);
    assert!(errors::STUB_PACKAGES.is_empty());
    // Any `invalid(BadParams)` says that the arguments do not fit, without the constitution.
    let take = decode_args("hold", json!({"action": "take", "target": "/plain"}), &k);
    let e = Error::invalid(Why::BadParams);
    let src = StoreSource::new(&k.w.conn);
    let hint = errors::hint(&k.w.s, &e, &take, &[], &src, false);
    assert_eq!(
        hint,
        "these arguments do not fit this call: check them against the tool's description"
    );
    assert!(!hint.contains("constitution"));
    assert_eq!(errors::invalid_text(&k.w.s, Why::BadParams), hint);
}

/// 1.5 (§20.1, §21, R3): a failed first call that opens its session returns the `hello`, the
/// error, and the `session_opened` warning, which says that nothing else in the call was applied;
/// the structured content puts them in §20.1's order, `hello` after the next calls. A refusal of a
/// call that names its session, and the shell's own refusals, which open none, carry no such
/// warning.
#[test]
fn a_failed_first_call_says_it_opened_its_session_and_nothing_else() {
    let mut k = Kit::new();
    let c = k.call(
        None,
        "hold",
        json!({"action": "take", "target": "/nowhere"}),
    );
    let r = &c.response;
    assert!(r.is_error);
    let x = SessionId(1);
    assert!(k.w.s.sessions.contains_key(&x), "the call opened s-1");
    let v = &r.structured;
    assert_eq!(
        keys_of(v),
        ["status", "error", "hint", "warnings", "next_calls", "hello"]
    );
    assert_eq!(v["error"], json!("not_found"));
    assert_eq!(
        v["warnings"],
        json!([{"code": "session_opened", "detail": "this call failed, but it opened session \
            s-1; nothing else in the call was applied; pass session: s-1 on every call"}])
    );
    assert_eq!(v["hello"]["session"], json!("s-1"));
    assert_eq!(
        keys_of(&v["hello"]).first().map(String::as_str),
        Some("session")
    );
    // The text keeps its order: status, hello, error and hint, warnings, next calls.
    let lines: Vec<&str> = r.text.lines().collect();
    assert!(lines[0].starts_with("[swarm] "));
    assert!(lines[1].starts_with("hello: {\"session\":\"s-1\","));
    assert!(lines[2].starts_with("error: not_found"));
    assert!(lines[3].starts_with("hint: "));
    assert!(lines[4].starts_with("warnings: [{\"code\":\"session_opened\""));
    assert!(lines[5].starts_with("next_calls"));
    // The same refusal in an existing session: no warning.
    let c = k.call(
        Some(x),
        "hold",
        json!({"action": "take", "target": "/nowhere"}),
    );
    assert!(c.response.structured.get("warnings").is_none());
    assert!(c.response.structured.get("hello").is_none());
    // A first call that succeeds keeps the core's warning, after the next calls' place.
    let c = k.call(None, "view", json!({"action": "list"}));
    let v = &c.response.structured;
    assert_eq!(
        keys_of(v)[..4],
        ["status", "warnings", "next_calls", "hello"]
    );
    assert_eq!(
        v["warnings"][0]["detail"],
        json!("this call opened session s-2; pass session: s-2 on every call")
    );
}

/// 1.5 (§15, R3, D.14.7): a bare path is a node reference in a basis, and one that names no node
/// meets the core's `not_found(Node)`. Its hint names the item and teaches the forms of a basis,
/// words for something outside the forum among them; an item that names a node adds nothing.
/// Since 1.6, prose and a key there are something outside the forum, and meet no refusal. The
/// hint is rendered from the core's refusal directly.
#[test]
fn a_basis_item_that_names_no_node_teaches_the_forms() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    ok(&k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/work", "title": "Fix the parser"}),
    ));
    let hint_of = |k: &Kit, basis: Value| -> String {
        let env = DecodeEnv {
            now: k.w.clock,
            text_max: k.w.s.params.text_max,
            coverage_window: k.w.s.params.coverage_window,
            entries: &Lookup(&k.w.conn),
        };
        let args = json!({"action": "decide", "at": "/work", "text": "ship it", "basis": basis});
        let Value::Object(map) = args else {
            panic!("an object")
        };
        let p = prepare("talk", map, &env).unwrap_or_else(|e| panic!("{}", e.render()));
        let src = StoreSource::new(&k.w.conn);
        let e = Error::not_found(What::Node);
        errors::hint(&k.w.s, &e, &p.cmd, &p.node_args, &src, false)
    };
    let forms = "a basis item is #12@4 or a path with @4 (a node at a version), #12 or a path (a \
                 node), or e57 (an entry); for something outside the forum, such as a build, a \
                 run, or a document, write it in words, or as {external, checked_at?}";
    assert_eq!(
        hint_of(&k, json!(["/work@1", "kb/build-812"])),
        format!("no such node; basis[1] \"kb/build-812\" names no node: {forms}")
    );
    assert_eq!(
        hint_of(&k, json!(["/nowhere@2"])),
        format!("no such node; basis[0] \"/nowhere@2\" names no node: {forms}")
    );
    // Items that name nodes, and words outside the forum (1.6), add nothing: the missing node
    // is another argument.
    assert_eq!(hint_of(&k, json!(["/work", "#1"])), "no such node");
    assert_eq!(
        hint_of(&k, json!(["build 812 is green", "arxiv:2101.00001"])),
        "no such node"
    );
}

/// A key or an entry id given where a node goes (1.4, R5): the refusal is unchanged, and its
/// hint says what was given and where it belongs, for the core's `not_found(Node)` and for the
/// decoder's `invalid` of a URL key.
#[test]
fn a_key_or_an_entry_where_a_node_goes_gets_its_hint() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/work", "keys": ["chat:team/1", "https://example.org/a"],
               "title": "work"}),
    );
    ok(&c);
    let work = k.node("/work");
    let mut request = Eid(0);
    k.events(|d| {
        request = request_event(
            d,
            work,
            s1,
            "chat:team/1/m1",
            "chat:team/1",
            "fix it",
            "Bob",
        );
    });
    let c = k.call(
        Some(s1),
        "hold",
        json!({"action": "take", "target": "chat:team/1"}),
    );
    assert!(c.response.is_error);
    assert!(
        c.response.text.contains(
            "chat:team/1 is a key: it sits on /work (#8); pass a node as target, and a \
             conversation key as where"
        ),
        "{}",
        c.response.text
    );
    let c = k.call(
        Some(s1),
        "view",
        json!({"action": "node", "path": request.render()}),
    );
    assert!(
        c.response.text.contains(&format!(
            "{0} is a request: pass it in re of to_user",
            request.render()
        )),
        "{}",
        c.response.text
    );
    // A key that is no node: no node carries it.
    let c = k.call(
        Some(s1),
        "view",
        json!({"action": "node", "path": "issue:proj#404"}),
    );
    assert!(
        c.response
            .text
            .contains("issue:proj#404 is a key: no node carries it")
    );
    // The decoder refuses a URL key as a node (its // makes an empty step): the shell's answer
    // reads the carrier too, and keeps the status line of the session the call names.
    let Value::Object(map) =
        json!({"action": "take", "target": "https://example.org/a", "session": "s-1"})
    else {
        unreachable!()
    };
    let env = DecodeEnv {
        now: k.w.clock,
        text_max: k.w.s.params.text_max,
        coverage_window: k.w.s.params.coverage_window,
        entries: &Lookup(&k.w.conn),
    };
    let answer = prepare("hold", map, &env).unwrap_err();
    let src = StoreSource::new(&k.w.conn);
    let r = shell_refusal(&k.w.s, Some(s1), k.w.clock, &k.health, &answer, &src, None);
    assert!(r.text.starts_with("[swarm] "), "{}", r.text);
    assert!(
        r.text.contains("is a key: it sits on /work (#8)"),
        "{}",
        r.text
    );
}

const DROPPABLE: [&str; 6] = [
    "constitution",
    "knowledge",
    "related",
    "present",
    "orientation",
    "offers",
];

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig {
        cases: 48,
        failure_persistence: None,
        ..proptest::prelude::ProptestConfig::default()
    })]

    /// `hello` stays within `hello.budget`, dropping sections in §20.3's order; the sections that
    /// are never dropped are always there, the inbox whole (§20.3, A-73; 1.5).
    #[test]
    fn hello_keeps_its_budget(shape in hello_shape()) {
        let (k, a, notes) = hello_world(&shape);
        let src = StoreSource::new(&k.w.conn);
        let h = hello::render_hello(
            &k.w.s,
            a,
            k.w.clock,
            &k.health,
            &hello::HelloInput {
                delivered: Some(&notes),
                user_entry: None,
                hits: &k.hits,
                route: &hello::RouteCall::default(),
                first: true,
            },
            &src,
        );
        let v: Value = serde_json::from_str(&h.text()).unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        // The sections that are never dropped are there whenever they hold something (1.7,
        // §20.1: a section with nothing in it is left out).
        assert!(keys.contains(&"principals"), "principals is never dropped");
        assert_eq!(keys.contains(&"inbox"), !notes.is_empty(), "{keys:?}");
        assert_eq!(keys.contains(&"user_decisions"), !shape.questions.is_empty(), "{keys:?}");
        // A digest entry names a question, so the world makes none without questions.
        let digest = !shape.digest.is_empty() && !shape.questions.is_empty();
        assert_eq!(keys.contains(&"user_digest"), digest, "{keys:?}");
        let budget = usize::try_from(shape.budget).unwrap();
        if h.size() > budget {
            for d in DROPPABLE {
                assert!(!keys.contains(&d), "{d} is dropped before the minimum goes over");
            }
        }
        // Dropping follows §20.3's order, and `cut` names what it dropped, in that order (1.7):
        // no section is dropped while an earlier one stays.
        let cut: Vec<&str> = v
            .get("cut")
            .map(|c| c.as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect())
            .unwrap_or_default();
        let place = |d: &str| DROPPABLE.iter().position(|x| *x == d).unwrap();
        assert!(cut.windows(2).all(|w| place(w[0]) < place(w[1])), "{cut:?}");
        for d in DROPPABLE {
            assert!(!(keys.contains(&d) && cut.contains(&d)), "{d} is kept and cut");
            if keys.contains(&d) {
                assert!(cut.iter().all(|c| place(c) < place(d)), "{d} stayed after {cut:?}");
            }
        }
        if !cut.is_empty() {
            assert_eq!(keys.last(), Some(&"cut"), "{keys:?}");
        }
        // The inbox is never cut (1.5): every delivered note, oldest first.
        let shown: Vec<&str> = v["inbox"]["items"]
            .as_array()
            .map(|xs| xs.iter().map(|x| x["e"].as_str().unwrap()).collect())
            .unwrap_or_default();
        let all: Vec<String> = notes.iter().map(|e| e.render()).collect();
        assert_eq!(shown, all);
        // Shown whole, the inbox carries no counts (1.7, §18).
        assert!(v["inbox"].get("shown").is_none(), "{}", v["inbox"]);
        // The user items are bounded: at most 5 each, every text cut to its share, and at most
        // `USER_OPTIONS_MAX` options, each cut to its share of the item (E3-bugs-10).
        let cap = usize::try_from(shape.budget / 16).unwrap().max(16);
        for it in v["user_decisions"]["items"].as_array().into_iter().flatten() {
            assert!(it["text"].as_str().unwrap().len() <= cap);
            let options = it["options"].as_array().unwrap();
            assert!(options.len() <= hello::USER_OPTIONS_MAX);
            for o in options {
                assert!(o.as_str().unwrap().len() <= (cap / hello::USER_OPTIONS_MAX).max(16));
            }
            assert!(it.to_string().len() <= 3 * cap.max(160), "{it}");
        }
        assert!(v["user_decisions"]["items"].as_array().map_or(0, Vec::len) <= 5);
        assert!(v["user_digest"]["items"].as_array().map_or(0, Vec::len) <= 5);
    }
}

mod review;
// 1.11 (the lab's sixth round): texts and responses.
mod texts;
// 1.12 (the lab's seventh round): texts and responses.
mod texts_1_12;
