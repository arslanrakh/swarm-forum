//! JSON → `Cmd` for every action (D.12.1), the forms of a basis reference (CS-22), the form
//! check of `ask` (R27), the raw text of node arguments (R5), masking before the command and the
//! digest (D.11.13, CS-35), and the `text.max` check after masking (A-114).

use std::collections::{BTreeMap, BTreeSet};

use forum_core::*;
use serde_json::{Value, json};

use super::*;

/// 2026-10-01T00:00:00Z.
const NOW: u64 = 1_790_812_800_000;
const DAY: u64 = 86_400_000;

/// The shell's entries: e1 to e999 exist on node #(40 + n), except e404; every question's author
/// is s-3 on #77.
struct Entries;

impl EntryLookup for Entries {
    fn entry_node(&self, eid: Eid) -> Option<NodeId> {
        (eid.0 != 404 && (1..1_000).contains(&eid.0)).then(|| NodeId(40 + eid.0))
    }

    fn question(&self, eid: Eid) -> Option<(NodeId, Option<SessionId>)> {
        (eid.0 != 404).then_some((NodeId(77), Some(SessionId(3))))
    }
}

fn env(text_max: u32) -> DecodeEnv<'static> {
    DecodeEnv {
        now: Time(NOW),
        text_max,
        coverage_window: Dur(28 * DAY),
        entries: &Entries,
    }
}

fn args(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        other => panic!("not an object: {other}"),
    }
}

fn try_dec(tool: &str, v: Value) -> Result<Prepared, ShellAnswer> {
    prepare(tool, args(v), &env(65_536))
}

fn dec(tool: &str, v: Value) -> Prepared {
    match try_dec(tool, v.clone()) {
        Ok(p) => p,
        Err(e) => panic!("{tool} {v}: {}", e.render()),
    }
}

fn cmd(tool: &str, v: Value) -> Cmd {
    dec(tool, v).cmd
}

fn invalid(tool: &str, v: Value) -> DecodeError {
    match try_dec(tool, v.clone()) {
        Err(ShellAnswer::Invalid(e)) => e,
        other => panic!("{tool} {v}: expected invalid, got {other:?}"),
    }
}

/// The first problem of a refusal, as its hint's first line shows it.
fn line(e: &DecodeError) -> String {
    format!("{}: {}", e.arg, e.why)
}

fn t(s: &str) -> Text {
    Text(s.to_string())
}

fn n(s: &str) -> NodeRef {
    values::node(s).unwrap()
}

fn id(i: u64) -> NodeRef {
    NodeRef::Id(NodeId(i))
}

fn pth(s: &str) -> Path {
    values::path(s).unwrap()
}

fn k(s: &str) -> Key {
    values::key(s).unwrap()
}

fn l(s: &str) -> Label {
    Label::new(s).unwrap()
}

fn ls(xs: &[&str]) -> BTreeSet<Label> {
    xs.iter().map(|x| l(x)).collect()
}

fn pk(s: &str) -> PrincipalKey {
    PrincipalKey::new(s).unwrap()
}

fn e(i: u64) -> Eid {
    Eid(i)
}

fn es(xs: &[u64]) -> BTreeSet<Eid> {
    xs.iter().map(|x| Eid(*x)).collect()
}

fn tok(node: u64, fence: u64) -> Token {
    Token {
        node: NodeId(node),
        fence: Fence(fence),
    }
}

fn at(rfc3339: &str) -> When {
    values::when(rfc3339).unwrap()
}

fn tm(pairs: &[(&str, &str)]) -> BTreeMap<Text, Text> {
    pairs.iter().map(|(a, b)| (t(a), t(b))).collect()
}

// ---------------------------------------------------------------- session

#[test]
fn session_hello_and_status() {
    assert_eq!(
        cmd("session", json!({"action": "hello"})),
        Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps: None,
            horizon: None,
            principal: None,
            from_user: None
        })
    );
    assert_eq!(
        cmd(
            "session",
            json!({"action": "hello", "intent": "fix the summary gap", "caps": ["rust", "team:payments"],
                   "horizon": 90, "principal": "mail:Alice@Corp.Example",
                   "from_user": {"text": "summarise tickets", "client": "slack", "principal": "slack:t01/u02"}})
        ),
        Cmd::Session(SessionCmd::Hello {
            intent: Some(t("fix the summary gap")),
            caps: Some(ls(&["rust", "team:payments"])),
            horizon: Some(90),
            principal: Some(pk("mail:alice@corp.example")),
            from_user: Some(FromUserArg {
                text: t("summarise tickets"),
                client: t("slack"),
                principal: Some(pk("slack:t01/u02")),
                answers: None,
                to: None,
                clarifies: None,
                reason: None,
            }),
        })
    );
    // caps: [] adds nothing to the token's caps (1.6).
    assert_eq!(
        cmd("session", json!({"action": "hello", "caps": []})),
        Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps: Some(BTreeSet::new()),
            horizon: None,
            principal: None,
            from_user: None
        })
    );
    assert_eq!(
        cmd(
            "session",
            json!({"action": "status", "text": "reading the brief"})
        ),
        Cmd::Session(SessionCmd::Status {
            text: t("reading the brief")
        })
    );
    // 1.6 (§8.2, l2, l3): hello's from_user takes answers, as talk from_user does.
    let Cmd::Session(SessionCmd::Hello { from_user, .. }) = cmd(
        "session",
        json!({"action": "hello", "from_user": {"text": "yes, go", "client": "ide", "answers": "e9"}}),
    ) else {
        panic!("a hello");
    };
    assert_eq!(from_user.and_then(|f| f.answers), Some(e(9)));
    let err = invalid(
        "session",
        json!({"action": "hello", "from_user": {"text": "hi"}}),
    );
    assert_eq!(err.arg, "from_user.client");
    let err = invalid(
        "session",
        json!({"action": "hello", "principal": "user:e12"}),
    );
    assert_eq!(err.arg, "principal");
}

// ---------------------------------------------------------------- view

#[test]
fn view_actions() {
    assert_eq!(
        cmd("view", json!({"action": "node", "path": "/a/b"})),
        Cmd::View(ViewCmd::Node {
            targets: vec![n("/a/b")],
            view: None,
            page: None
        })
    );
    assert_eq!(
        cmd(
            "view",
            json!({"action": "node", "paths": ["#12", "c/d"], "view": "full", "page": 2})
        ),
        Cmd::View(ViewCmd::Node {
            targets: vec![id(12), n("/c/d")],
            view: Some(ViewMode::Full),
            page: Some(2)
        })
    );
    assert_eq!(
        cmd(
            "view",
            json!({"action": "node", "path": "#3", "view": "review"})
        ),
        Cmd::View(ViewCmd::Node {
            targets: vec![id(3)],
            view: Some(ViewMode::Review),
            page: None
        })
    );
    assert_eq!(
        cmd(
            "view",
            json!({"action": "find", "key": "ISSUE:Platform#88"})
        ),
        Cmd::View(ViewCmd::Find {
            by: FindBy::Key(k("issue:platform#88").key)
        })
    );
    assert_eq!(
        cmd("view", json!({"action": "find", "text": "summary gap"})),
        Cmd::View(ViewCmd::Find {
            by: FindBy::Text(t("summary gap"))
        })
    );
    assert_eq!(
        cmd(
            "view",
            json!({"action": "feed", "at": "/a", "since": 7, "types": ["said", "from_user"], "limit": 20})
        ),
        Cmd::View(ViewCmd::Feed {
            at: n("/a"),
            since: Some(Seq(7)),
            types: [EntryType::Said, EntryType::FromUser].into_iter().collect(),
            limit: Some(20)
        })
    );
    assert_eq!(
        cmd(
            "view",
            json!({"action": "list", "under": "/kb", "kind": "paper", "tags": ["offer"], "status": "draft",
                   "fields": {"venue": "ICML"}, "flags": ["overdue", "missed"], "closed": true, "limit": 500})
        ),
        Cmd::View(ViewCmd::List {
            under: Some(n("/kb")),
            kind: Some(l("paper")),
            tags: ls(&["offer"]),
            status: Some(t("draft")),
            fields: tm(&[("venue", "ICML")]),
            flags: FlagSet::EMPTY.with(Flag::Overdue).with(Flag::Missed),
            closed: Some(true),
            limit: Some(500)
        })
    );
    // 1.6 (§20.4, l2, l4): status open and closed filter the node's state, as closed does.
    for (status, closed) in [("open", false), ("closed", true)] {
        for also in [None, Some(closed)] {
            let mut v = json!({"action": "list", "status": status});
            if let Some(c) = also {
                v["closed"] = json!(c);
            }
            let Cmd::View(ViewCmd::List {
                status: s,
                closed: c,
                ..
            }) = cmd("view", v)
            else {
                panic!("a list");
            };
            assert_eq!((s, c), (None, Some(closed)), "{status}");
        }
        let err = invalid(
            "view",
            json!({"action": "list", "status": status, "closed": !closed}),
        );
        assert_eq!(err.arg, "status");
        assert!(err.why.ends_with("pass one of them"), "{}", err.why);
    }
    assert_eq!(
        cmd("view", json!({"action": "list"})),
        Cmd::View(ViewCmd::List {
            under: None,
            kind: None,
            tags: BTreeSet::new(),
            status: None,
            fields: BTreeMap::new(),
            flags: FlagSet::EMPTY,
            closed: None,
            limit: None
        })
    );
    assert_eq!(
        cmd(
            "view",
            json!({"action": "history", "path": "#9", "before": 100, "limit": 5})
        ),
        Cmd::View(ViewCmd::History {
            at: id(9),
            before: Some(100),
            limit: Some(5)
        })
    );
    for (requester, want) in [
        ("me", RequesterFilter::Mine),
        ("all", RequesterFilter::All),
        (
            "mail:<Bob@X.Example>",
            RequesterFilter::Principal(pk("mail:bob@x.example")),
        ),
    ] {
        assert_eq!(
            cmd(
                "view",
                json!({"action": "requests", "status": "closed_unanswered", "requester": requester})
            ),
            Cmd::View(ViewCmd::Requests {
                status: Some(BoardState::ClosedUnanswered),
                requester: Some(want)
            })
        );
    }
    // 1.6 (§20.4, l6): feed's since also takes an entry id, which the writer turns into the
    // node's seq at that entry; a seq's digits are a seq.
    let p = dec(
        "view",
        json!({"action": "feed", "at": "/a", "since": "e57"}),
    );
    assert_eq!(p.since_entry, Some(e(57)));
    let Cmd::View(ViewCmd::Feed { since, .. }) = p.cmd else {
        panic!("a feed");
    };
    assert_eq!(since, None);
    let p = dec("view", json!({"action": "feed", "at": "/a", "since": "12"}));
    assert_eq!(p.since_entry, None);
    assert!(matches!(
        p.cmd,
        Cmd::View(ViewCmd::Feed {
            since: Some(Seq(12)),
            ..
        })
    ));
    let err = invalid(
        "view",
        json!({"action": "feed", "at": "/a", "since": "yesterday"}),
    );
    assert_eq!(err.arg, "since");
    assert!(err.why.contains("such as e57"), "{}", err.why);
    assert_eq!(
        cmd(
            "view",
            json!({"action": "user_log", "since": "e40", "limit": 50})
        ),
        Cmd::View(ViewCmd::UserLog {
            since: Some(e(40)),
            limit: Some(50)
        })
    );
    assert_eq!(
        cmd("view", json!({"action": "constitution"})),
        Cmd::View(ViewCmd::Constitution)
    );
    assert_eq!(invalid("view", json!({"action": "node"})).arg, "path|paths");
    assert_eq!(
        invalid(
            "view",
            json!({"action": "node", "path": "/a", "paths": ["/b"]})
        )
        .arg,
        "path|paths"
    );
    assert_eq!(
        invalid(
            "view",
            json!({"action": "feed", "at": "/a", "types": ["gossip"]})
        )
        .arg,
        "types[0]"
    );
    // `view agreements` (1.4, D.14.5) takes only `under` and `limit`.
    assert_eq!(
        invalid("view", json!({"action": "agreements", "kind": "task"})).arg,
        "kind"
    );
}

// ---------------------------------------------------------------- node

#[test]
fn node_create() {
    assert_eq!(
        cmd(
            "node",
            json!({"action": "create", "path": "/work/summary-gap", "title": "Summary gap"})
        ),
        Cmd::Node(NodeCmd::Create(CreateArgs {
            path: pth("/work/summary-gap"),
            keys: vec![],
            kind: None,
            title: t("Summary gap"),
            body: None,
            digest: None,
            fields: BTreeMap::new(),
            tags: BTreeSet::new(),
            capacity: None,
            needs: None,
            est: None,
            priority: None,
            due: None,
            depends_on: vec![],
            owners: vec![],
            repeat: None,
            agreements: None,
            request: None,
            take: None,
        }))
    );
    let c = cmd(
        "node",
        json!({"action": "create", "path": "digest/daily-tickets", "keys": ["user:e1", "chat:Support/t9"],
               "kind": "series", "title": "Daily tickets", "body": "b", "digest": "d",
               "fields": {"owner": "ops"}, "tags": ["offer"], "capacity": 0, "needs": [], "est": 30,
               "priority": -2, "due": "in 2h", "depends_on": ["#4"], "owners": ["@people/alice"],
               "repeat": {"cron": "0 7 * * MON-FRI", "tz": "Europe/Berlin", "run_within": "2h"},
               "agreements": "be brief",
               "request": {"text": "summarise", "origin": "chat:Support/t9/m1", "requester": "Bob"},
               "take": false}),
    );
    let Cmd::Node(NodeCmd::Create(a)) = c else {
        panic!("a create");
    };
    assert_eq!(a.path, pth("/digest/daily-tickets"));
    assert_eq!(a.keys, vec![k("user:e1"), k("chat:support/t9")]);
    assert_eq!(a.kind, Some(l("series")));
    assert_eq!(a.fields, tm(&[("owner", "ops")]));
    assert_eq!(a.capacity, Some(0));
    assert_eq!(a.needs, Some(BTreeSet::new()), "[] is an explicit none");
    assert_eq!(a.est, Some(30));
    assert_eq!(a.priority, Some(-2));
    assert_eq!(a.due, Some(When::In(Dur(7_200_000))));
    assert_eq!(a.depends_on, vec![id(4)]);
    assert_eq!(a.owners, vec![n("/@people/alice")]);
    let r = a.repeat.unwrap();
    assert_eq!(r.cron_text, t("0 7 * * MON-FRI"));
    assert_eq!(r.cron, forum_cron::parse_cron("0 7 * * 1-5").unwrap());
    assert_eq!(r.tz.name, "Europe/Berlin");
    assert_eq!(
        r.tz,
        crate::tz::table("Europe/Berlin", NOW - 28 * DAY, NOW + crate::tz::AHEAD_MS).unwrap(),
        "frozen from now - coverage.window to now + 50 years"
    );
    assert_eq!(r.run_within, Dur(7_200_000));
    assert_eq!(a.agreements, Some(t("be brief")));
    assert_eq!(
        a.request,
        Some(RequestArg {
            text: t("summarise"),
            origin: Some(k("chat:support/t9/m1")),
            where_: None,
            requester: t("Bob"),
        })
    );
    assert_eq!(a.take, Some(false));
    // Without tz, UTC.
    let Cmd::Node(NodeCmd::Create(a)) = cmd(
        "node",
        json!({"action": "create", "path": "/s", "title": "s", "repeat": {"cron": "*/15 * * * *", "run_within": 600000}}),
    ) else {
        panic!("a create");
    };
    let r = a.repeat.unwrap();
    assert_eq!((r.tz.name.as_str(), r.tz.initial), ("UTC", 0));
    assert_eq!(r.run_within, Dur(600_000));
    // Refusals naming the argument.
    for (v, arg) in [
        (
            json!({"action": "create", "path": "#12", "title": "x"}),
            "path",
        ),
        (
            json!({"action": "create", "path": "/a", "title": "x", "keys": ["repeat:1:20261002T0500Z"]}),
            "keys[0]",
        ),
        (
            json!({"action": "create", "path": "/a", "title": "x", "tags": ["Offer"]}),
            "tags[0]",
        ),
        (
            json!({"action": "create", "path": "/a", "title": "x", "repeat": {"cron": "0 7 * *", "run_within": "1h"}}),
            "repeat.cron",
        ),
        (
            json!({"action": "create", "path": "/a", "title": "x", "repeat": {"cron": "0 7 L * *", "run_within": "1h"}}),
            "repeat.cron",
        ),
        (
            json!({"action": "create", "path": "/a", "title": "x", "repeat": {"cron": "0 7 * * *", "tz": "Mars/Base", "run_within": "1h"}}),
            "repeat.tz",
        ),
        (
            json!({"action": "create", "path": "/a", "title": "x", "repeat": {"cron": "0 7 * * *"}}),
            "repeat.run_within",
        ),
        (json!({"action": "create", "path": "/a"}), "title"),
        (
            json!({"action": "create", "path": "/a", "title": "x", "request": {"text": "r"}}),
            "request.requester",
        ),
        (
            json!({"action": "create", "path": "/a", "title": "x", "capacity": -1}),
            "capacity",
        ),
        (
            json!({"action": "create", "path": "/a", "title": "x", "est": 4_294_967_296_u64}),
            "est",
        ),
    ] {
        assert_eq!(invalid("node", v.clone()).arg, arg, "{v}");
    }
    // 1.7 (§20.2, §20.5): `create` takes no files; given, they are ignored with a warning.
    let p = dec(
        "node",
        json!({"action": "create", "path": "/a", "title": "x", "files": {"a": "b"}}),
    );
    assert_eq!(p.ignored.len(), 1);
    assert_eq!(p.ignored[0].name, "files");
    assert_eq!(p.ignored[0].takers, "node update and hold checkpoint");
    assert_eq!(
        p.cmd,
        cmd(
            "node",
            json!({"action": "create", "path": "/a", "title": "x"})
        )
    );
}

#[test]
fn node_update() {
    let c = cmd(
        "node",
        json!({"action": "update", "path": "#12", "version": 4, "digest_version": 2, "title": "T",
               "body": "B", "digest": "D", "fields": {"a": "1"}, "files": {"spec": "https://x.example/s.pdf"},
               "kind": "paper", "status": "review", "capacity": 3, "priority": 1, "est": 15,
               "due": "2026-10-03T12:00:00Z", "depends_on": [], "owners": ["#6"],
               "repeat": {"cron": "0 9 * * *", "run_within": "1h"}, "agreements": "",
               "tags_add": ["offer"], "tags_remove": ["stale"], "needs_add": ["rust"], "needs_remove": ["go"],
               "token": "12.3"}),
    );
    let Cmd::Node(NodeCmd::Update {
        at: a,
        version,
        digest_version,
        token,
        patch,
    }) = c
    else {
        panic!("an update");
    };
    assert_eq!(a, id(12));
    assert_eq!(version, Version(4));
    assert_eq!(digest_version, Some(Version(2)));
    assert_eq!(token, Some(tok(12, 3)));
    assert_eq!(patch.title, Some(t("T")));
    assert_eq!(
        patch.files,
        Some(tm(&[("spec", "https://x.example/s.pdf")]))
    );
    assert_eq!(patch.kind, Change::Set(l("paper")));
    assert_eq!(patch.est, Change::Set(15));
    assert_eq!(patch.due, Change::Set(at("2026-10-03T12:00:00Z")));
    assert!(matches!(patch.repeat, Change::Set(_)));
    assert_eq!(patch.depends_on, Some(vec![]));
    assert_eq!(patch.owners, Some(vec![id(6)]));
    assert_eq!(patch.agreements, Some(t("")));
    assert_eq!(patch.tags_add, ls(&["offer"]));
    assert_eq!(patch.tags_remove, ls(&["stale"]));
    assert_eq!(patch.needs_add, ls(&["rust"]));
    assert_eq!(patch.needs_remove, ls(&["go"]));
    // null clears kind, est, due, and repeat; absence keeps them.
    let Cmd::Node(NodeCmd::Update { patch, .. }) = cmd(
        "node",
        json!({"action": "update", "path": "/a", "version": 1, "kind": null, "est": null, "due": null, "repeat": null}),
    ) else {
        panic!("an update");
    };
    assert_eq!(
        patch,
        Patch {
            kind: Change::Clear,
            est: Change::Clear,
            due: Change::Clear,
            repeat: Change::Clear,
            ..Patch::default()
        }
    );
    let Cmd::Node(NodeCmd::Update { patch, .. }) = cmd(
        "node",
        json!({"action": "update", "path": "/a", "version": 1}),
    ) else {
        panic!("an update");
    };
    assert_eq!(patch, Patch::default());
    // §20.5: a field set to null is cleared. The fields that Patch clears with an empty value
    // take null too: the empty text, map, or list (review E2-spec-8).
    let Cmd::Node(NodeCmd::Update { patch, .. }) = cmd(
        "node",
        json!({"action": "update", "path": "/a", "version": 1, "body": null, "fields": null,
               "files": null, "status": null, "depends_on": null, "owners": null,
               "agreements": null}),
    ) else {
        panic!("an update");
    };
    assert_eq!(
        patch,
        Patch {
            body: Some(t("")),
            fields: Some(BTreeMap::new()),
            files: Some(BTreeMap::new()),
            status: Some(t("")),
            depends_on: Some(vec![]),
            owners: Some(vec![]),
            agreements: Some(t("")),
            ..Patch::default()
        }
    );
    // A field that cannot be cleared says so, and a required one is missing its value.
    for field in [
        "title", "digest", "capacity", "priority", "token", "tags_add",
    ] {
        let mut v = json!({"action": "update", "path": "/a", "version": 1});
        v[field] = Value::Null;
        let e = invalid("node", v);
        assert_eq!(e.arg, field);
        assert!(
            e.why
                .starts_with(&format!("{field} cannot be cleared with null: give ")),
            "{e}"
        );
    }
    let e = invalid(
        "node",
        json!({"action": "update", "path": "/a", "version": null}),
    );
    assert_eq!(
        line(&e),
        "version: version cannot be cleared with null: give the version of the brief you were \
         shown, or leave it out for the one last shown to you"
    );
    // Outside update, null is the wrong type.
    let e = invalid(
        "node",
        json!({"action": "create", "path": "/a", "title": "T", "body": null}),
    );
    assert_eq!(line(&e), "body: expected a string, found null");
    // 1.7 (§20.5): the versions left out are the writer's to fill, from what the session was
    // shown; the command carries 0, no node's version, until then.
    let p = dec("node", json!({"action": "update", "path": "/a"}));
    assert_eq!(
        p.fill,
        Some(crate::decode::FillVersions {
            version: true,
            digest_version: false
        })
    );
    assert!(matches!(
        p.cmd,
        Cmd::Node(NodeCmd::Update {
            version: Version(0),
            digest_version: None,
            ..
        })
    ));
    let p = dec(
        "node",
        json!({"action": "update", "path": "/a", "version": 3, "digest": "d"}),
    );
    assert_eq!(
        p.fill,
        Some(crate::decode::FillVersions {
            version: false,
            digest_version: true
        })
    );
    let p = dec(
        "node",
        json!({"action": "update", "path": "/a", "version": 3, "title": "t"}),
    );
    assert_eq!(p.fill, None);
    // 1.7 (§20.2): `tags` is `create`'s; `update` adds and removes them, and ignores `tags`.
    let p = dec(
        "node",
        json!({"action": "update", "path": "/a", "version": 1, "tags": ["x"]}),
    );
    assert_eq!(p.ignored[0].name, "tags");
}

#[test]
fn node_link_key_close_reopen_move_merge_hide() {
    assert_eq!(
        cmd(
            "node",
            json!({"action": "link", "from": "/a", "rel": "derived_from", "to": "#3"})
        ),
        Cmd::Node(NodeCmd::Link {
            at: n("/a"),
            rel: LinkRel::DerivedFrom,
            to: id(3)
        })
    );
    for (rel, want) in [
        ("continues", LinkRel::Continues),
        ("conflicts_with", LinkRel::ConflictsWith),
        ("duplicate", LinkRel::Duplicate),
        ("superseded_by", LinkRel::SupersededBy),
    ] {
        assert_eq!(
            cmd(
                "node",
                json!({"action": "link", "from": "/a", "rel": rel, "to": "/b"})
            ),
            Cmd::Node(NodeCmd::Link {
                at: n("/a"),
                rel: want,
                to: n("/b")
            })
        );
    }
    // 1.6 (§20.5, l4, l6): link names the node it goes from `from`, as merge does; 1.7 (§20.2):
    // `path`, or another name of the node a call is about, names it too.
    assert_eq!(
        cmd(
            "node",
            json!({"action": "link", "path": "/a", "rel": "duplicate", "to": "/b"})
        ),
        cmd(
            "node",
            json!({"action": "link", "from": "/a", "rel": "duplicate", "to": "/b"})
        )
    );
    let err = invalid(
        "node",
        json!({"action": "link", "from": "/a", "path": "/c", "rel": "duplicate", "to": "/b"}),
    );
    assert_eq!(err.arg, "from|path");
    assert!(err.why.starts_with("two nodes given"), "{}", err.why);
    assert_eq!(err.signature, "node link takes {from*, rel*, to*}");
    assert!(
        try_dec(
            "node",
            json!({"action": "link", "from": "/a", "path": "/a", "rel": "duplicate", "to": "/b"})
        )
        .is_ok(),
        "the same node twice is one"
    );
    assert_eq!(
        cmd(
            "node",
            json!({"action": "key", "path": "/a", "add": "https://X.Example:443/p?token=t&q=1#frag"})
        ),
        Cmd::Node(NodeCmd::Key {
            at: n("/a"),
            change: KeyChange::Add(k("https://x.example/p?q=1"))
        })
    );
    assert_eq!(
        cmd(
            "node",
            json!({"action": "key", "path": "/a", "remove": "CHAT:C/t"})
        ),
        Cmd::Node(NodeCmd::Key {
            at: n("/a"),
            change: KeyChange::Remove(k("chat:c/t").key)
        })
    );
    assert_eq!(
        invalid("node", json!({"action": "key", "path": "/a"})).arg,
        "add|remove"
    );
    assert_eq!(
        cmd(
            "node",
            json!({"action": "close", "path": "/a", "outcome": "done", "summary": "ok", "verified": "ran it",
                   "settles": ["e5", "e6"], "learnings": "l", "token": "9.1", "unsent": ["e8"]})
        ),
        Cmd::Node(NodeCmd::Close {
            at: n("/a"),
            outcome: Outcome::Done,
            summary: t("ok"),
            verified: Some(t("ran it")),
            settles: Some(es(&[5, 6])),
            learnings: Some(t("l")),
            token: Some(tok(9, 1)),
            unsent: es(&[8]),
        })
    );
    assert_eq!(
        cmd(
            "node",
            json!({"action": "close", "path": "/a", "outcome": "wont-fix", "summary": "s"})
        ),
        Cmd::Node(NodeCmd::Close {
            at: n("/a"),
            outcome: Outcome::Other(l("wont-fix")),
            summary: t("s"),
            verified: None,
            // 1.7 (§5): without `settles`, the default; `settles: []` settles none.
            settles: None,
            learnings: None,
            token: None,
            unsent: BTreeSet::new(),
        })
    );
    assert!(matches!(
        cmd(
            "node",
            json!({"action": "close", "path": "/a", "outcome": "done", "summary": "s", "settles": []})
        ),
        Cmd::Node(NodeCmd::Close { settles: Some(s), .. }) if s.is_empty()
    ));
    // `missed` reaches the core, which refuses it as the server's own outcome (A-66).
    let Cmd::Node(NodeCmd::Close { outcome, .. }) = cmd(
        "node",
        json!({"action": "close", "path": "/a", "outcome": "missed", "summary": "s"}),
    ) else {
        panic!("a close");
    };
    assert_eq!(outcome, Outcome::Missed);
    // 1.9 (§5): the usual words for done reach the core as done.
    for w in ["completed", "implemented", "fixed", "resolved", "merged"] {
        let Cmd::Node(NodeCmd::Close { outcome, .. }) = cmd(
            "node",
            json!({"action": "close", "path": "/a", "outcome": w, "summary": "s"}),
        ) else {
            panic!("a close");
        };
        assert_eq!(outcome, Outcome::Done, "{w}");
    }
    assert_eq!(
        cmd(
            "node",
            json!({"action": "reopen", "path": "/a", "reason": "r"})
        ),
        Cmd::Node(NodeCmd::Reopen {
            at: n("/a"),
            reason: t("r")
        })
    );
    assert_eq!(
        cmd(
            "node",
            json!({"action": "move", "path": "#5", "to": "/b/c"})
        ),
        Cmd::Node(NodeCmd::Move {
            at: id(5),
            to: pth("/b/c")
        })
    );
    assert_eq!(
        invalid("node", json!({"action": "move", "path": "#5", "to": "#6"})).arg,
        "to"
    );
    assert_eq!(
        cmd(
            "node",
            json!({"action": "merge", "from": "/a", "into": "#2"})
        ),
        Cmd::Node(NodeCmd::Merge {
            from: n("/a"),
            into: id(2)
        })
    );
    assert_eq!(
        cmd(
            "node",
            json!({"action": "hide", "entry": "e57", "reason": "a secret"})
        ),
        Cmd::Node(NodeCmd::Hide {
            target: HideArg::Entry {
                eid: e(57),
                node: NodeId(97)
            },
            reason: t("a secret")
        })
    );
    assert_eq!(
        cmd(
            "node",
            json!({"action": "hide", "file": {"path": "/a", "name": "creds.txt"}, "reason": "r"})
        ),
        Cmd::Node(NodeCmd::Hide {
            target: HideArg::File {
                at: n("/a"),
                name: t("creds.txt")
            },
            reason: t("r")
        })
    );
    // An entry the shell's table lacks names no node; the core refuses it in §21's order
    // (review E2-spec-3).
    assert_eq!(
        cmd(
            "node",
            json!({"action": "hide", "entry": "e404", "reason": "r"})
        ),
        Cmd::Node(NodeCmd::Hide {
            target: HideArg::Entry {
                eid: e(404),
                node: UNKNOWN_NODE
            },
            reason: t("r")
        })
    );
}

// ---------------------------------------------------------------- hold

#[test]
fn hold_actions() {
    assert_eq!(
        cmd(
            "hold",
            json!({"action": "take", "target": "/a", "lease": "20m"})
        ),
        Cmd::Hold(HoldCmd::Take {
            target: n("/a"),
            lease: Some(Dur(1_200_000))
        })
    );
    assert_eq!(
        cmd("hold", json!({"action": "next"})),
        Cmd::Hold(HoldCmd::Next {
            query: NextQuery::default(),
            lease: None
        })
    );
    assert_eq!(
        cmd(
            "hold",
            json!({"action": "next", "query": {"under": "/kb", "kind": "paper", "tags": ["offer"]}, "lease": 50000})
        ),
        Cmd::Hold(HoldCmd::Next {
            query: NextQuery {
                under: Some(n("/kb")),
                kind: Some(l("paper")),
                tags: ls(&["offer"])
            },
            lease: Some(Dur(50_000))
        })
    );
    assert_eq!(
        cmd(
            "hold",
            json!({"action": "checkpoint", "token": "12.3", "done": "d", "next_step": "n", "intent": "push",
                   "intent_done": "pushed", "files": ["https://x.example/a.log"]})
        ),
        Cmd::Hold(HoldCmd::Checkpoint {
            token: tok(12, 3),
            done: t("d"),
            next_step: t("n"),
            intent: Some(t("push")),
            intent_done: Some(t("pushed")),
            files: vec![t("https://x.example/a.log")]
        })
    );
    assert_eq!(
        cmd(
            "hold",
            json!({"action": "renew", "token": "12.3", "why": "a long build"})
        ),
        Cmd::Hold(HoldCmd::Renew {
            token: tok(12, 3),
            why: t("a long build")
        })
    );
    let release = |waits_for: Value, reason: &str| {
        cmd(
            "hold",
            json!({"action": "release", "token": "12.3", "done": "d", "next_step": "n", "reason": reason,
                   "waits_for": waits_for, "learnings": "l", "unsent": ["e8"]}),
        )
    };
    let want = |waits_for: WaitArg, reason: ReleaseReason| {
        Cmd::Hold(HoldCmd::Release {
            token: tok(12, 3),
            done: t("d"),
            next_step: t("n"),
            reason,
            waits_for: Some(waits_for),
            intent_done: None,
            learnings: Some(t("l")),
            unsent: es(&[8]),
            settles: BTreeSet::new(),
        })
    };
    assert_eq!(
        release(json!({"question": "e9"}), "blocked"),
        want(WaitArg::Question(e(9)), ReleaseReason::Blocked)
    );
    assert_eq!(
        release(json!({"node": "#4"}), "handoff"),
        want(WaitArg::Node(id(4)), ReleaseReason::Handoff)
    );
    assert_eq!(
        release(json!({"time": "in 1h"}), "cannot"),
        want(
            WaitArg::Time(When::In(Dur(3_600_000))),
            ReleaseReason::Cannot
        )
    );
    assert_eq!(
        release(
            json!({"external": "CI run 812", "recheck_after": "2026-10-01T09:30:00Z"}),
            "blocked"
        ),
        want(
            WaitArg::External {
                what: t("CI run 812"),
                recheck_after: at("2026-10-01T09:30:00Z")
            },
            ReleaseReason::Blocked
        )
    );
    for (w, arg) in [
        (json!({"question": "e9", "node": "#4"}), "waits_for"),
        (json!({"external": "x"}), "waits_for.recheck_after"),
        (json!({}), "waits_for"),
        (json!({"question": "9"}), "waits_for.question"),
    ] {
        let v = json!({"action": "release", "token": "1.1", "done": "d", "next_step": "n",
                       "reason": "blocked", "waits_for": w});
        assert_eq!(invalid("hold", v.clone()).arg, arg, "{v}");
    }
    assert_eq!(
        invalid(
            "hold",
            json!({"action": "release", "token": "1.1", "done": "d", "next_step": "n"})
        )
        .arg,
        "reason",
        "reason is required (A-98)"
    );
    assert_eq!(
        cmd(
            "hold",
            json!({"action": "check", "target": "/a", "token": "3.2"})
        ),
        Cmd::Hold(HoldCmd::Check {
            target: n("/a"),
            token: Some(tok(3, 2)),
            intent_done: None
        })
    );
    assert_eq!(
        invalid(
            "hold",
            json!({"action": "renew", "token": "12", "why": "w"})
        )
        .arg,
        "token"
    );
}

// ---------------------------------------------------------------- talk

#[test]
fn talk_user_actions() {
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "from_user", "text": "and skip spam", "client": "slack",
                   "principal": "slack:T01/U02", "clarifies": "e1"})
        ),
        Cmd::Talk(TalkCmd::FromUser {
            text: t("and skip spam"),
            client: t("slack"),
            principal: Some(pk("slack:T01/U02")),
            answers: None,
            to: None,
            clarifies: Some(e(1)),
            reason: None,
        })
    );
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "from_user", "text": "yes", "client": "ide", "answers": "e9", "node": "/a"})
        ),
        Cmd::Talk(TalkCmd::FromUser {
            text: t("yes"),
            client: t("ide"),
            principal: None,
            answers: Some(e(9)),
            to: Some(forum_core::RouteTo::Node(n("/a"))),
            clarifies: None,
            reason: None,
        })
    );
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "to_user", "text": "done", "kind": "result", "verbatim": true,
                   "principal": "mail:bob@x.example", "re": ["e1", "e2"]})
        ),
        Cmd::Talk(TalkCmd::ToUser {
            text: t("done"),
            kind: ToUserKind::Result,
            verbatim: true,
            principal: Some(pk("mail:bob@x.example")),
            re: es(&[1, 2]),
            at: None,
        })
    );
    // 1.6 (§8.1, it0): verbatim is optional; without it, the entry is a paraphrase.
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "to_user", "text": "x", "kind": "result"})
        ),
        Cmd::Talk(TalkCmd::ToUser {
            text: t("x"),
            kind: ToUserKind::Result,
            verbatim: false,
            principal: None,
            re: BTreeSet::new(),
            at: None,
        })
    );
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "route", "entry": "e3", "to": "/digest/daily", "reason": "a request"})
        ),
        Cmd::Talk(TalkCmd::Route {
            entry: e(3),
            to: Some(forum_core::RouteTo::Node(n("/digest/daily"))),
            answers: None,
            clarifies: None,
            reason: Some(t("a request")),
        })
    );
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "route", "entry": "e3", "to": "none", "reason": "thanks"})
        ),
        Cmd::Talk(TalkCmd::Route {
            entry: e(3),
            to: Some(forum_core::RouteTo::Nowhere),
            answers: None,
            clarifies: None,
            reason: Some(t("thanks")),
        })
    );
    // 1.7 (§8.2): `answers` or `clarifies` in place of `to`, and `reason` optional; one target is
    // needed.
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "route", "entry": "e3", "answers": "e2"})
        ),
        Cmd::Talk(TalkCmd::Route {
            entry: e(3),
            to: None,
            answers: Some(e(2)),
            clarifies: None,
            reason: None,
        })
    );
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "route", "entry": "e3", "clarifies": "e7"})
        ),
        Cmd::Talk(TalkCmd::Route {
            entry: e(3),
            to: None,
            answers: None,
            clarifies: Some(e(7)),
            reason: None,
        })
    );
    assert_eq!(
        invalid("talk", json!({"action": "route", "entry": "e3"})).arg,
        "to|answers|clarifies"
    );
    assert_eq!(
        invalid(
            "talk",
            json!({"action": "route", "entry": "e3", "to": "none", "answers": "e2"})
        )
        .arg,
        "to|answers|clarifies"
    );
    // `from_user {to: none}` logs and routes a thanks in one call; `node` is read as `to`, and the
    // two together are refused.
    assert!(matches!(
        cmd(
            "talk",
            json!({"action": "from_user", "text": "thanks", "client": "ide", "to": "none"})
        ),
        Cmd::Talk(TalkCmd::FromUser {
            to: Some(forum_core::RouteTo::Nowhere),
            reason: None,
            ..
        })
    ));
    assert!(matches!(
        cmd(
            "talk",
            json!({"action": "from_user", "text": "more", "client": "ide", "node": "/a"})
        ),
        Cmd::Talk(TalkCmd::FromUser {
            to: Some(forum_core::RouteTo::Node(_)),
            ..
        })
    ));
    assert_eq!(
        invalid(
            "talk",
            json!({"action": "from_user", "text": "x", "client": "ide", "to": "/a", "node": "/b"})
        )
        .arg,
        "node"
    );
    let Cmd::Session(SessionCmd::Hello { from_user, .. }) = cmd(
        "session",
        json!({"action": "hello", "from_user": {"text": "thanks", "client": "ide", "to": "none",
               "reason": "a thanks"}}),
    ) else {
        panic!("a hello");
    };
    let f = from_user.unwrap();
    assert_eq!(f.to, Some(forum_core::RouteTo::Nowhere));
    assert_eq!(f.reason, Some(t("a thanks")));
}

#[test]
fn talk_people_actions() {
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "request", "at": "/support/t9", "text": "please fix", "origin": "chat:Support/t9/m4",
                   "where": "chat:Support/t9", "requester": "Bob"})
        ),
        Cmd::Talk(TalkCmd::Request {
            at: n("/support/t9"),
            text: t("please fix"),
            origin: k("chat:support/t9/m4"),
            where_: Some(k("chat:support/t9")),
            requester: t("Bob")
        })
    );
    assert_eq!(
        invalid(
            "talk",
            json!({"action": "request", "at": "/a", "text": "x", "requester": "Bob"})
        )
        .arg,
        "origin"
    );
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "said", "at": "/a", "where": "mail:<m1@X.Example>", "text": "fixed",
                   "kind": "result", "corrects": "e7", "seen": 12})
        ),
        Cmd::Talk(TalkCmd::Said {
            at: n("/a"),
            where_: Some(k("mail:m1@x.example")),
            text: t("fixed"),
            kind: SaidKind::Result,
            corrects: Some(e(7)),
            seen: Some(Seq(12))
        })
    );
    // A said of kind question reaches the core, whose refusal names talk ask (M1a).
    let Cmd::Talk(TalkCmd::Said { kind, .. }) = cmd(
        "talk",
        json!({"action": "said", "at": "/a", "text": "?", "kind": "question"}),
    ) else {
        panic!("a said");
    };
    assert_eq!(kind, SaidKind::Question);
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "said", "said_id": "e8", "confirmed": "https://chat.example/m/1"})
        ),
        Cmd::Talk(TalkCmd::SaidConfirm {
            said: e(8),
            confirmed: t("https://chat.example/m/1")
        })
    );
    // 1.7 (§20.2): the confirmation takes no node, and ignores one, with a warning.
    let p = dec(
        "talk",
        json!({"action": "said", "said_id": "e8", "confirmed": "x", "at": "/a"}),
    );
    assert_eq!(p.ignored[0].name, "at");
    assert!(
        p.ignored_detail()
            .unwrap()
            .starts_with("talk said, confirming a said entry as sent does not take at"),
        "{:?}",
        p.ignored_detail()
    );
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "heard", "at": "/a", "from": "Bob", "text": "thanks", "answers": "e9"})
        ),
        Cmd::Talk(TalkCmd::Heard {
            at: n("/a"),
            where_: None,
            from: t("Bob"),
            text: t("thanks"),
            answers: Some(e(9))
        })
    );
}

#[test]
fn talk_ask_has_three_forms() {
    // A person: no to.
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "ask", "at": "/a", "text": "which?", "where": "chat:C/t", "again": true, "blocking": true})
        ),
        Cmd::Talk(TalkCmd::Ask {
            at: n("/a"),
            text: t("which?"),
            to: AskTo::Person {
                where_: Some(k("chat:c/t")),
                blocking: true
            },
            again: true
        })
    );
    // Agents.
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "ask", "at": "/a", "text": "status?", "to": "holders:/a/**", "blocking": true})
        ),
        Cmd::Talk(TalkCmd::Ask {
            at: n("/a"),
            text: t("status?"),
            to: AskTo::Agent(AgentAddressee::Holders(n("/a"))),
            again: false
        })
    );
    // 1.9 (§13): `holder:` with no path is the holder of `at`.
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "ask", "at": "/a", "text": "status?", "to": "holder:"})
        ),
        Cmd::Talk(TalkCmd::Ask {
            at: n("/a"),
            text: t("status?"),
            to: AskTo::Agent(AgentAddressee::Holder(n("/a"))),
            again: false
        })
    );
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "ask", "at": "/a", "text": "?", "to": "session:s-4"})
        ),
        Cmd::Talk(TalkCmd::Ask {
            at: n("/a"),
            text: t("?"),
            to: AskTo::Agent(AgentAddressee::Session(SessionId(4))),
            again: false
        })
    );
    // A human, with the question digest of the masked text and options.
    let c = cmd(
        "talk",
        json!({"action": "ask", "at": "/a", "text": "Ship it?", "to": "principal:mail:Alice@Corp.Example",
               "options": ["yes", "no"], "recommendation": "yes", "default": "no", "due": "in 1h", "again": true}),
    );
    assert_eq!(
        c,
        Cmd::Talk(TalkCmd::Ask {
            at: n("/a"),
            text: t("Ship it?"),
            to: AskTo::User(UserAsk {
                to: Some(pk("mail:alice@corp.example")),
                options: vec![t("yes"), t("no")],
                recommendation: Some(t("yes")),
                default: Some(t("no")),
                due: Some(When::In(Dur(3_600_000))),
                digest: crate::digest::question_digest("Ship it?", &["yes".into(), "no".into()]),
            }),
            again: true
        })
    );
    // An open question: options may be empty, but are required.
    let Cmd::Talk(TalkCmd::Ask { to, .. }) = cmd(
        "talk",
        json!({"action": "ask", "at": "/a", "text": "What next?", "to": "user", "options": []}),
    ) else {
        panic!("an ask");
    };
    assert!(matches!(to, AskTo::User(UserAsk { to: None, ref options, .. }) if options.is_empty()));
    let err = invalid(
        "talk",
        json!({"action": "ask", "at": "/a", "text": "?", "to": "user"}),
    );
    assert_eq!(err.arg, "options");
    // R27: an argument of another form names the argument and the form; 1.7 (§20.2, §20.7)
    // ignored it, with a warning; 1.8 (it2 l1, l6: a question for a person in a thread went to the
    // human) refuses it again, as 1.6 did, and the hint ends with every form's signature, the
    // call's own first.
    for (v, arg, form) in [
        (
            json!({"action": "ask", "at": "/a", "text": "?", "to": "user", "options": [], "blocking": true}),
            "blocking",
            "for a human",
        ),
        (
            json!({"action": "ask", "at": "/a", "text": "?", "to": "principal:mail:a@b", "options": [], "where": "chat:c/t"}),
            "where",
            "for a human",
        ),
        (
            json!({"action": "ask", "at": "/a", "text": "?", "options": ["a"]}),
            "options",
            "for a person",
        ),
        (
            json!({"action": "ask", "at": "/a", "text": "?", "due": "in 1h"}),
            "due",
            "for a person",
        ),
        (
            json!({"action": "ask", "at": "/a", "text": "?", "to": "session:s-1", "recommendation": "x"}),
            "recommendation",
            "for agents",
        ),
        (
            json!({"action": "ask", "at": "/a", "text": "?", "to": "session:s-1", "where": "chat:c/t"}),
            "where",
            "for agents",
        ),
    ] {
        let e = invalid("talk", v.clone());
        assert_eq!(e.arg, arg, "{v}");
        assert!(e.why.contains(form), "{v}: {}", e.why);
        assert!(
            e.why.contains("it marks another form of ask"),
            "{v}: {}",
            e.why
        );
        let sigs: Vec<&str> = e.signature.lines().collect();
        assert_eq!(sigs.len(), 3, "{v}: {}", e.signature);
        assert!(
            sigs[0].starts_with(&format!("talk ask {form}")),
            "{v}: {sigs:?}"
        );
        for f in ["for a human", "for a person", "for agents"] {
            assert!(
                sigs.iter().any(|l| l.starts_with(&format!("talk ask {f}"))),
                "{sigs:?}"
            );
        }
    }
    // With `to: user`, `where` and `blocking` say how a person in a thread is asked.
    let e = invalid(
        "talk",
        json!({"action": "ask", "at": "/a", "text": "?", "to": "user", "options": [], "blocking": true}),
    );
    assert!(
        e.why.ends_with(
            "it marks another form of ask: to ask a person in a thread, leave out to, and ask \
             records the said for you; each form's arguments follow"
        ),
        "{}",
        e.why
    );
    // Another action's argument is still ignored on any form (1.7).
    let p = dec(
        "talk",
        json!({"action": "ask", "at": "/a", "text": "?", "to": "user", "options": [], "evidence": "reported"}),
    );
    assert_eq!(p.ignored[0].name, "evidence");
    // The later-release addressees (A-83).
    let err = invalid(
        "talk",
        json!({"action": "ask", "at": "/a", "text": "?", "to": "class:coding-agent"}),
    );
    assert_eq!(err.arg, "to");
    assert!(err.why.contains("later release"));
}

#[test]
fn talk_agent_and_review_actions() {
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "answer", "question": "e9", "text": "yes"})
        ),
        Cmd::Talk(TalkCmd::Answer {
            question: e(9),
            node: NodeId(77),
            asker: Some(SessionId(3)),
            text: t("yes")
        })
    );
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "answer", "question": "e404", "text": "yes"})
        ),
        Cmd::Talk(TalkCmd::Answer {
            question: e(404),
            node: UNKNOWN_NODE,
            asker: None,
            text: t("yes")
        })
    );
    for (to, want) in [
        (
            json!("session:s-4"),
            Some(AddresseeArg::Session(SessionId(4))),
        ),
        (json!("holder:/a"), Some(AddresseeArg::Holder(n("/a")))),
        // 1.9 (§13): `holder:` with no path is the holder of `at`.
        (json!("holder:"), Some(AddresseeArg::Holder(n("/a")))),
        (json!("holders:/a/**"), Some(AddresseeArg::Holders(n("/a")))),
        (json!("user"), Some(AddresseeArg::User)),
        (
            json!("principal:mail:bob@x.example"),
            Some(AddresseeArg::Principal(pk("mail:bob@x.example"))),
        ),
    ] {
        assert_eq!(
            cmd(
                "talk",
                json!({"action": "say", "at": "/a", "text": "hi", "to": to, "expects": "reply",
                       "reply_to": "e5", "until": "in 1d"})
            ),
            Cmd::Talk(TalkCmd::Say {
                at: n("/a"),
                text: t("hi"),
                to: want,
                expects: Some(Expects::Reply),
                reply_to: Some(e(5)),
                until: Some(When::In(Dur(DAY)))
            })
        );
    }
    for later in ["class:x", "cap:rust", "everyone"] {
        let err = invalid(
            "talk",
            json!({"action": "say", "at": "/a", "text": "hi", "to": later}),
        );
        assert!(err.why.contains("later release"), "{later}");
    }
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "decide", "at": "/a", "text": "ship", "settles": ["e1"], "supersedes": "e2",
                   "evidence": "observation"})
        ),
        Cmd::Talk(TalkCmd::Decide {
            at: n("/a"),
            text: t("ship"),
            settles: es(&[1]),
            supersedes: Some(e(2)),
            basis: vec![],
            evidence: Some(Evidence::Observation)
        })
    );
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "approve", "on": "#12@4", "checked": "ran the tests"})
        ),
        Cmd::Talk(TalkCmd::Approve {
            on: id(12),
            version: Version(4),
            checked: t("ran the tests")
        })
    );
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "approve", "on": "/a/b@7", "checked": "c"})
        ),
        Cmd::Talk(TalkCmd::Approve {
            on: n("/a/b"),
            version: Version(7),
            checked: t("c")
        })
    );
    assert_eq!(
        invalid(
            "talk",
            json!({"action": "approve", "on": "#12", "checked": "c"})
        )
        .arg,
        "on"
    );
    // 1.6 (§20.7, l4, l6): object and resolve take their words as `text`.
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "object", "on": "/a", "text": "unsafe"})
        ),
        Cmd::Talk(TalkCmd::Object {
            on: n("/a"),
            note: t("unsafe")
        })
    );
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "resolve", "objection": "e12", "text": "fixed"})
        ),
        Cmd::Talk(TalkCmd::Resolve {
            objection: e(12),
            note: t("fixed"),
            upheld: false,
            defer: false,
        })
    );
    let err = invalid(
        "talk",
        json!({"action": "object", "on": "/a", "note": "unsafe"}),
    );
    assert_eq!(
        err.problems().map(|p| p.arg.as_str()).collect::<Vec<_>>(),
        ["note", "text"]
    );
    assert_eq!(err.signature, "talk object takes {on*, text*}");
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "retract", "entry": "e30", "text": "wrong", "reason": "misread"})
        ),
        Cmd::Talk(TalkCmd::Retract {
            target: RetractArg::Entry {
                eid: e(30),
                node: NodeId(70)
            },
            text: t("wrong"),
            reason: t("misread")
        })
    );
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "retract", "path": "/kb/claim", "text": "t", "reason": "r"})
        ),
        Cmd::Talk(TalkCmd::Retract {
            target: RetractArg::Node(n("/kb/claim")),
            text: t("t"),
            reason: t("r")
        })
    );
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "wait", "timeout": "50s", "for": "/a"})
        ),
        Cmd::Talk(TalkCmd::Wait {
            timeout: Dur(50_000),
            for_: Some(n("/a")),
            since: None
        })
    );
    assert_eq!(
        cmd("talk", json!({"action": "who"})),
        Cmd::Talk(TalkCmd::Who { at: None })
    );
    assert_eq!(
        cmd("talk", json!({"action": "who", "at": "/kb"})),
        Cmd::Talk(TalkCmd::Who { at: Some(n("/kb")) })
    );
}

/// CS-22: `#12@4`, a path with `@4`, `#12`, a bare path (1.5, §15, R3), `e57`, `entry:e57`, and
/// `{external}`; any other string is something outside the forum (1.6); a string written as a
/// reference that is not one is `invalid` naming the item.
#[test]
fn the_forms_of_a_basis_reference() {
    let call = json!({"action": "decide", "at": "/a", "text": "t", "basis": [
        "#12@4", "/kb/result@3", "kb/other@1", "#12", "e57", "entry:e58",
        {"external": "build 812 green", "checked_at": "2026-10-01T06:00:00Z", "recheck": "in 7d"},
        {"external": "dataset v2", "checked_at": "in 0s"},
        "/kb/result", "kb/other"
    ]});
    let c = cmd("talk", call.clone());
    let Cmd::Talk(TalkCmd::Decide { basis, .. }) = c else {
        panic!("a decide");
    };
    let (basis, bare) = basis.split_at(8);
    assert_eq!(
        bare.to_vec(),
        vec![
            BasisArg::Node {
                at: n("/kb/result"),
                version: None
            },
            BasisArg::Node {
                at: n("/kb/other"),
                version: None
            },
        ]
    );
    // The text of each node reference is kept for R5's hint, as a node argument's is.
    let p = dec("talk", call);
    let kept: Vec<(&str, &str)> = p
        .node_args
        .iter()
        .map(|a| (a.arg.as_str(), a.raw.as_str()))
        .collect();
    assert_eq!(
        kept,
        [
            ("at", "/a"),
            ("basis[0]", "#12@4"),
            ("basis[1]", "/kb/result@3"),
            ("basis[2]", "kb/other@1"),
            ("basis[3]", "#12"),
            ("basis[8]", "/kb/result"),
            ("basis[9]", "kb/other"),
        ]
    );
    let basis = basis.to_vec();
    assert_eq!(
        basis,
        vec![
            BasisArg::Node {
                at: id(12),
                version: Some(Version(4))
            },
            BasisArg::Node {
                at: n("/kb/result"),
                version: Some(Version(3))
            },
            BasisArg::Node {
                at: n("/kb/other"),
                version: Some(Version(1))
            },
            BasisArg::Node {
                at: id(12),
                version: None
            },
            BasisArg::Entry(e(57)),
            BasisArg::Entry(e(58)),
            BasisArg::External {
                what: t("build 812 green"),
                checked_at: at("2026-10-01T06:00:00Z"),
                recheck: Some(When::In(Dur(7 * DAY)))
            },
            BasisArg::External {
                what: t("dataset v2"),
                checked_at: When::In(Dur(0)),
                recheck: None
            },
        ]
    );
    for (item, arg) in [
        (json!("#result"), "basis[0]"),
        (json!("kb//result"), "basis[0]"),
        (json!(12), "basis[0]"),
        (
            json!({"external": "x", "checked_at": "yesterday"}),
            "basis[0].checked_at",
        ),
        (json!({"what": "x", "checked_at": "in 0s"}), "basis[0].what"),
    ] {
        let v = json!({"action": "decide", "at": "/a", "text": "t", "basis": [item]});
        assert_eq!(invalid("talk", v.clone()).arg, arg, "{v}");
    }
    // Every item that is not a reference, at once (1.6).
    let e = invalid(
        "talk",
        json!({"action": "decide", "at": "/a", "text": "t", "basis": ["#x", "#1", "entry:nope"]}),
    );
    assert_eq!(
        e.problems().map(|p| p.arg.as_str()).collect::<Vec<_>>(),
        ["basis[0]", "basis[2]"]
    );
    // 1.6 (§15, l1, l2, l6): a string not written as a reference is something outside the
    // forum, checked now; checked_at is optional, and takes now and a date.
    let Cmd::Talk(TalkCmd::Decide { basis, .. }) = cmd(
        "talk",
        json!({"action": "decide", "at": "/a", "text": "t", "basis": [
            "Platform run timings 9412 and run 9437 dependency logs",
            "https://remark42.com/docs/",
            "arxiv:2101.00001",
            {"external": "the survey"},
            {"external": "the docs", "checked_at": "now"},
            {"external": "the dataset", "checked_at": "2026-09-30"},
        ]}),
    ) else {
        panic!("a decide");
    };
    let checked_now = |what: &str| BasisArg::External {
        what: t(what),
        checked_at: When::In(Dur(0)),
        recheck: None,
    };
    assert_eq!(
        basis,
        vec![
            checked_now("Platform run timings 9412 and run 9437 dependency logs"),
            checked_now("https://remark42.com/docs/"),
            checked_now("arxiv:2101.00001"),
            checked_now("the survey"),
            checked_now("the docs"),
            BasisArg::External {
                what: t("the dataset"),
                checked_at: When::At(Time(NOW - DAY)),
                recheck: None
            },
        ]
    );
    // A string outside the forum is free text: masked, and not kept as a node argument.
    let p = dec(
        "talk",
        json!({"action": "decide", "at": "/a", "text": "t",
               "basis": ["the run with token=ghp_abcdefghijklmnopqrstuvwxyz0123456789 passed"]}),
    );
    assert_eq!(p.masked_fields, 1);
    assert_eq!(p.node_args.len(), 1, "only at");
    let Cmd::Talk(TalkCmd::Decide { basis, .. }) = p.cmd else {
        panic!("a decide");
    };
    let [BasisArg::External { what, .. }] = basis.as_slice() else {
        panic!("{basis:?}");
    };
    assert!(!what.0.contains("ghp_"), "{what:?}");
}

// ---------------------------------------------------------------- control

#[test]
fn control_actions() {
    assert_eq!(
        cmd(
            "control",
            json!({"action": "pause", "scope": "/", "reason": "incident"})
        ),
        Cmd::Control(ControlCmd::Pause {
            scope: n("/"),
            reason: t("incident")
        })
    );
    assert_eq!(
        cmd("control", json!({"action": "unpause", "scope": "#0"})),
        Cmd::Control(ControlCmd::Unpause { scope: id(0) })
    );
    assert_eq!(
        cmd("control", json!({"action": "health"})),
        Cmd::Control(ControlCmd::Health)
    );
    assert_eq!(
        cmd("control", json!({"action": "constitution"})),
        Cmd::Control(ControlCmd::Constitution)
    );
    assert_eq!(
        cmd(
            "control",
            json!({"action": "export", "after": 1000, "limit": 10})
        ),
        Cmd::Control(ControlCmd::Export {
            after: Some(1000),
            limit: Some(10)
        })
    );
    assert_eq!(
        invalid("control", json!({"action": "health", "verbose": true})).arg,
        "verbose"
    );
}

// ---------------------------------------------------------------- the call itself

#[test]
fn session_op_id_tool_and_action() {
    let p = dec(
        "hold",
        json!({"action": "next", "session": "s-12", "op_id": "0b9e2c5e-1d7a-4b45-9a8c-3f8d2e0c6a11"}),
    );
    assert_eq!(p.tool, Tool::Hold);
    assert_eq!(p.action, "next");
    assert_eq!(p.session, Some(SessionId(12)));
    assert_eq!(p.op_id, OpId::new("0b9e2c5e-1d7a-4b45-9a8c-3f8d2e0c6a11"));
    assert!(p.digest.is_some());
    assert_eq!(dec("hold", json!({"action": "next"})).digest, None);
    for (tool, v, arg) in [
        (
            "hold",
            json!({"action": "next", "session": "12"}),
            "session",
        ),
        ("hold", json!({"action": "next", "session": 12}), "session"),
        ("hold", json!({"action": "next", "op_id": ""}), "op_id"),
        ("hold", json!({"action": "next", "op_id": "é"}), "op_id"),
        ("hold", json!({}), "action"),
        ("hold", json!({"action": "grab"}), "action"),
        ("hold", json!({"action": 1}), "action"),
        ("holds", json!({"action": "next"}), "tool"),
        ("hold", json!({"action": "next", "lease": 1.5}), "lease"),
        (
            "node",
            json!({"action": "create", "path": "/a", "title": "t", "fields": {"x": "y"}, "est": 2.0}),
            "est",
        ),
        (
            "hold",
            json!({"action": "next", "query": {"tags": [1.5]}}),
            "query.tags[0]",
        ),
        ("hold", json!({"action": "take"}), "target"),
        ("hold", json!({"action": "take", "target": 12}), "target"),
        (
            "hold",
            json!({"action": "take", "target": "/a", "lease": "15 minutes"}),
            "lease",
        ),
    ] {
        assert_eq!(invalid(tool, v.clone()).arg, arg, "{tool} {v}");
    }
    // 1.7 (§20.2, §21): an argument of another action that records nothing of its own is
    // ignored, with a warning that names the actions that take it.
    let p = dec(
        "hold",
        json!({"action": "take", "target": "/a", "until": "in 1h"}),
    );
    assert_eq!(
        p.ignored_detail().as_deref(),
        Some(
            "hold take does not take until (talk say takes it), so the call was applied without it"
        )
    );
    // 1.8 (§21, it2 l1): on a call the core refuses, the warning says that it would be ignored,
    // since nothing was applied.
    assert_eq!(
        p.ignored_detail_refused().as_deref(),
        Some(
            "hold take does not take until (talk say takes it), so it would be ignored: leave it \
             out when you retry"
        )
    );
    let two = dec(
        "hold",
        json!({"action": "take", "target": "/a", "until": "in 1h", "evidence": "reported"}),
    );
    assert!(
        two.ignored_detail_refused()
            .unwrap()
            .contains("so they would be ignored: leave them out when you retry"),
        "{:?}",
        two.ignored_detail_refused()
    );
    // Refused for another problem, the call names it too, as one that would be ignored (1.8),
    // and ends with the action's signature (1.6, §20.2).
    let err = invalid(
        "hold",
        json!({"action": "take", "target": "/a", "until": "in 1h", "lease": "soon"}),
    );
    assert_eq!(
        ShellAnswer::Invalid(err).render(),
        "error: invalid\nhint: until: until is not an argument of hold take, and would be ignored; \
         until is an argument of talk say\nlease: \"soon\" is not a duration: write digits and \
         one unit, such as 50s, 15m, 2h, or 7d, or an integer of milliseconds\nhold take takes \
         {target*, lease}"
    );
    // 1.5 (§20.2): the hint names the actions that take an argument, those of the call's own
    // tool first, as its input schema names them; or that none does.
    let err = invalid(
        "hold",
        json!({"action": "take", "target": "/a", "token": "1.1"}),
    );
    assert_eq!(
        err.why,
        "token is not an argument of hold take; token is an argument of hold checkpoint, renew, \
         release, check and node update, close"
    );
    let err = invalid("hold", json!({"action": "next", "colour": "red"}));
    assert_eq!(
        err.why,
        "colour is not an argument of hold next; no action takes colour"
    );
    assert_eq!(err.signature, "hold next takes {query, lease}");
    assert_eq!(
        invalid("view", json!({"action": "constitution", "x": 1})).signature,
        "view constitution takes no arguments"
    );
}

/// R5: the decoder keeps the raw text of every node argument, and of a node argument it refuses.
#[test]
fn the_raw_text_of_node_arguments() {
    let p = dec(
        "talk",
        json!({"action": "said", "at": "chat:C1/t2/m3", "text": "x", "kind": "ack"}),
    );
    assert_eq!(
        p.node_args,
        vec![NodeArgText {
            arg: "at".into(),
            raw: "chat:C1/t2/m3".into()
        }]
    );
    // A multi-step key decodes as a relative path; the core then answers not_found(Node).
    let Cmd::Talk(TalkCmd::Said { at: a, .. }) = p.cmd else {
        panic!("a said");
    };
    assert_eq!(a, n("/chat:C1/t2/m3"));
    // A URL key does not decode as a path: invalid, with its raw text kept for the hint.
    let err = invalid(
        "talk",
        json!({"action": "decide", "at": "https://x.example/issues/88", "text": "t"}),
    );
    assert_eq!(err.arg, "at");
    assert_eq!(err.raw_node.as_deref(), Some("https://x.example/issues/88"));
    let p = dec(
        "node",
        json!({"action": "create", "path": "/a/b", "title": "t", "depends_on": ["#1", "e400"],
               "owners": ["@teams/core"]}),
    );
    assert_eq!(
        p.node_args
            .iter()
            .map(|a| (a.arg.as_str(), a.raw.as_str()))
            .collect::<Vec<_>>(),
        [
            ("path", "/a/b"),
            ("depends_on[0]", "#1"),
            ("depends_on[1]", "e400"),
            ("owners[0]", "@teams/core")
        ]
    );
    let p = dec(
        "hold",
        json!({"action": "release", "token": "1.1", "done": "d", "next_step": "n", "reason": "blocked",
               "waits_for": {"node": "issue:platform#88"}}),
    );
    assert_eq!(
        p.node_args,
        vec![NodeArgText {
            arg: "waits_for.node".into(),
            raw: "issue:platform#88".into()
        }]
    );
    let p = dec(
        "talk",
        json!({"action": "approve", "on": "e400@2", "checked": "c"}),
    );
    assert_eq!(p.node_args[0].raw, "e400@2");
    let err = invalid("node", json!({"action": "move", "path": "/a", "to": "#3"}));
    assert_eq!(err.raw_node.as_deref(), Some("#3"));
}

/// R8: a time the status line shows (RFC 3339 UTC with seconds) decodes to the same instant.
#[test]
fn a_shown_time_passed_back_decodes_to_the_same_instant() {
    for ms in [NOW, NOW + 7 * 3_600_000, NOW + 5 * 60_000 + 3 * DAY] {
        let ts = jiff::Timestamp::from_millisecond(i64::try_from(ms).unwrap()).unwrap();
        let shown = ts.strftime("%Y-%m-%dT%H:%M:%SZ").to_string();
        assert_eq!(values::when(&shown), Ok(When::At(Time(ms))), "{shown}");
    }
    assert_eq!(
        values::when("2026-10-01T07:00:00Z"),
        Ok(When::At(Time(NOW + 7 * 3_600_000)))
    );
}

// ---------------------------------------------------------------- masking, digest, text.max

const SECRET: &str = "Zx7Lq2Wn9Bv4Kc6Mt1Rp8Hs3Jd5Gf0Ya";

#[test]
fn free_texts_are_masked_before_the_command_is_built() {
    let p = dec(
        "node",
        json!({"action": "create", "path": "/a", "title": format!("token={SECRET}"),
               "fields": {format!("password={SECRET}"): format!("api_key: {SECRET}")},
               "keys": ["chat:c/t"],
               "request": {"text": format!("secret = '{SECRET}'"), "requester": "Bob"}}),
    );
    let Cmd::Node(NodeCmd::Create(a)) = &p.cmd else {
        panic!("a create");
    };
    assert_eq!(a.title, t("token=[masked:token]"));
    assert_eq!(
        a.fields,
        tm(&[("password=[masked:password]", "api_key: [masked:api_key]")])
    );
    assert_eq!(
        a.request.as_ref().unwrap().text,
        t("secret = '[masked:secret]'")
    );
    assert_eq!(p.masked_fields, 4);
    // Keys, paths, and ids are never masked; a URL key loses its credentials by normalisation.
    let p = dec(
        "node",
        json!({"action": "key", "path": "/token=abcdefghijklmnop0123", "add": format!("https://x.example/a?token={SECRET}")}),
    );
    assert_eq!(p.masked_fields, 0);
    assert_eq!(
        p.cmd,
        Cmd::Node(NodeCmd::Key {
            at: n("/token=abcdefghijklmnop0123"),
            change: KeyChange::Add(k("https://x.example/a"))
        })
    );
    // Every kind of free text: lists, options, a basis's external, a wait's external, from_user.
    let p = dec(
        "talk",
        json!({"action": "ask", "at": "/a", "text": format!("token={SECRET}"), "to": "user",
               "options": [format!("token={SECRET}"), "no"]}),
    );
    let Cmd::Talk(TalkCmd::Ask {
        to: AskTo::User(u),
        text,
        ..
    }) = &p.cmd
    else {
        panic!("an ask");
    };
    assert_eq!(text, &t("token=[masked:token]"));
    assert_eq!(u.options, vec![t("token=[masked:token]"), t("no")]);
    assert_eq!(
        u.digest,
        crate::digest::question_digest(
            "token=[masked:token]",
            &["token=[masked:token]".into(), "no".into()]
        ),
        "the question digest is of the masked text and options"
    );
    let p = dec(
        "talk",
        json!({"action": "decide", "at": "/a", "text": "t",
               "basis": [{"external": format!("token={SECRET}"), "checked_at": "in 0s"}]}),
    );
    assert_eq!(p.masked_fields, 1);
    let p = dec(
        "hold",
        json!({"action": "release", "token": "1.1", "done": "d", "next_step": "n", "reason": "blocked",
               "waits_for": {"external": format!("Authorization: Bearer {SECRET}"), "recheck_after": "in 1h"}}),
    );
    let Cmd::Hold(HoldCmd::Release {
        waits_for: Some(WaitArg::External { what, .. }),
        ..
    }) = &p.cmd
    else {
        panic!("a release");
    };
    assert_eq!(what, &t("Authorization: Bearer [masked:bearer_token]"));
    let p = dec(
        "session",
        json!({"action": "hello", "from_user": {"text": format!("my token={SECRET}"), "client": "slack"}}),
    );
    assert_eq!(p.masked_fields, 1);
    let p = dec(
        "hold",
        json!({"action": "checkpoint", "token": "1.1", "done": "d", "next_step": "n",
               "files": [format!("https://u:{SECRET}@x.example/f")]}),
    );
    let Cmd::Hold(HoldCmd::Checkpoint { files, .. }) = &p.cmd else {
        panic!("a checkpoint");
    };
    assert_eq!(
        files,
        &vec![t("https://u:[masked:url_password]@x.example/f")]
    );
}

/// CS-35: the digest is computed after masking, so two requests that differ only in a masked
/// secret share it; `session` and `op_id` do not enter it; and a retry digests with its record's
/// masking version.
#[test]
fn the_digest_is_computed_after_masking() {
    let call = |secret: &str, session: &str| {
        dec(
            "hold",
            json!({"action": "checkpoint", "token": "1.1", "done": format!("token={secret}"),
                   "next_step": "n", "session": session, "op_id": "op-1"}),
        )
    };
    let a = call(SECRET, "s-1");
    let b = call("Qy8Mn3Vb7Xc2Lk9Hj4Gf6Ds1Ap5Zw0Te", "s-2");
    assert!(a.digest.is_some());
    assert_eq!(a.digest, b.digest);
    assert_eq!(
        a.digest,
        crate::digest::request_digest(
            "hold",
            &args(
                json!({"action": "checkpoint", "token": "1.1", "done": "token=[masked:token]",
                         "next_step": "n"})
            )
        )
    );
    let c = call("short", "s-1");
    assert_ne!(a.digest, c.digest);
    assert_eq!(a.digest_with(crate::mask::MASKING_VERSION), a.digest);
    // The arguments as sent are kept for a retry's digest, and never shown.
    assert!(!format!("{a:?}").contains(SECRET));
    assert_eq!(a.digest_with(0), None);
    assert_eq!(a.digest_with(99), None);
    assert_eq!(dec("hold", json!({"action": "next"})).digest_with(1), None);
}

fn with_text_max(tool: &str, v: Value, text_max: u32) -> Result<Prepared, ShellAnswer> {
    prepare(tool, args(v), &env(text_max))
}

/// A-114: a free-text argument of `text.max` bytes after masking is accepted, and one byte more,
/// in any argument of any action, is `limit(TextTooLong)`.
#[test]
fn the_text_max_check_runs_after_masking() {
    const MAX: u32 = 256;
    let at_max = "a".repeat(256);
    let over = "a".repeat(257);
    let calls = |s: &str| {
        vec![
            (
                "node",
                json!({"action": "create", "path": "/a", "title": s}),
            ),
            (
                "node",
                json!({"action": "create", "path": "/a", "title": "t", "fields": {"f": s}}),
            ),
            (
                "node",
                json!({"action": "create", "path": "/a", "title": "t", "fields": {s: "v"}}),
            ),
            (
                "node",
                json!({"action": "update", "path": "/a", "version": 1, "files": {"f": s}}),
            ),
            (
                "talk",
                json!({"action": "ask", "at": "/a", "text": "q", "to": "user", "options": [s]}),
            ),
            (
                "hold",
                json!({"action": "checkpoint", "token": "1.1", "done": "d", "next_step": s}),
            ),
            (
                "talk",
                json!({"action": "from_user", "text": s, "client": "c"}),
            ),
            (
                "session",
                json!({"action": "hello", "from_user": {"text": "x", "client": s}}),
            ),
            (
                "talk",
                json!({"action": "decide", "at": "/a", "text": "t", "basis": [{"external": s, "checked_at": "in 0s"}]}),
            ),
            (
                "control",
                json!({"action": "pause", "scope": "/", "reason": s}),
            ),
            ("view", json!({"action": "find", "text": s})),
        ]
    };
    for (tool, v) in calls(&at_max) {
        assert!(with_text_max(tool, v.clone(), MAX).is_ok(), "{tool} {v}");
    }
    for (tool, v) in calls(&over) {
        assert_eq!(
            with_text_max(tool, v.clone(), MAX).unwrap_err(),
            ShellAnswer::TextTooLong {
                max: MAX,
                longest: 257
            },
            "{tool} {v}"
        );
    }
    // Measured after masking: a text over the limit as sent, within it once masked, passes.
    let raw = format!("{} token={SECRET}", "a".repeat(220));
    assert!(raw.len() > 256);
    let p = with_text_max(
        "talk",
        json!({"action": "from_user", "text": raw, "client": "c"}),
        MAX,
    )
    .unwrap();
    let Cmd::Talk(TalkCmd::FromUser { text, .. }) = p.cmd else {
        panic!("a from_user");
    };
    assert!(text.byte_len() <= 256);
    let answer = with_text_max(
        "node",
        json!({"action": "create", "path": "/a", "title": over}),
        MAX,
    )
    .unwrap_err();
    assert_eq!(answer.code(), ErrorCode::Limit);
    assert!(answer.render().starts_with(
        "error: limit\nhint: a free-text argument has 257 bytes, over text.max (256 bytes)"
    ));
}

#[test]
fn shell_answers_render_without_a_status_line() {
    let answer = try_dec("hold", json!({"action": "take"})).unwrap_err();
    let text = answer.render();
    assert!(
        text.starts_with("error: invalid\nhint: target: missing"),
        "{text}"
    );
    assert!(!text.contains("[swarm]"));
    assert!(!text.contains("next_calls"));
}

/// 1.5 (R3, D.14.7): a missing argument's refusal names every required argument the call lacks,
/// and where to find each, so that one retry supplies them all. In R3, every Codex run was refused
/// twice on its first `node update`: for `version`, then for `digest_version`; since 1.7 (§20.5)
/// the writer fills both from what the session was shown, and refuses itself when it was shown
/// none, naming the current values and the same update with them filled.
#[test]
fn a_missing_argument_names_every_one_and_where_to_find_it() {
    let p = dec(
        "node",
        json!({"action": "update", "path": "/a", "digest": "d"}),
    );
    assert_eq!(
        p.fill,
        Some(crate::decode::FillVersions {
            version: true,
            digest_version: true
        })
    );
    let e = crate::decode::versions_not_shown("#12", 5, 2, p.fill.unwrap());
    assert_eq!(e.arg, "version, digest_version");
    assert_eq!(
        e.to_string(),
        "version, digest_version: missing, and this session was not shown #12 since the server \
         started or the session fell silent: it is at version 5 and digest_version 2 now; if \
         your edit still fits it, retry with them: node update {path: #12, version: 5, \
         digest_version: 2, …}\nnode update takes {path*, version, digest_version, title, body, \
         digest, fields, files, kind, status, capacity, priority, est, due, depends_on, owners, \
         repeat, agreements, tags_add, tags_remove, needs_add, needs_remove, token}"
    );
    // Each with its own source, those that share one together.
    let e = invalid("hold", json!({"action": "checkpoint"}));
    assert_eq!(e.arg, "token, done, next_step");
    assert_eq!(
        e.why,
        "missing: hold checkpoint requires token, done, and next_step; token: your hold's token, \
         on the status line and in hello's holds; done: what you did since the last checkpoint; \
         next_step: what comes next, so that a stranger could take over"
    );
    assert_eq!(
        invalid("hold", json!({"action": "take"})).to_string(),
        "target: missing: hold take requires it; a path or #<id>, as hello's offers, view find, \
         and view list show them\nhold take takes {target*, lease}"
    );
    let e = invalid("node", json!({"action": "close", "path": "/a"}));
    assert_eq!(e.arg, "outcome, summary");
    assert_eq!(
        e.why,
        "missing: node close requires outcome and summary; outcome: one label: done, failed, \
         dropped, superseded, refuted, or another; summary: what came of it, for whoever comes \
         next"
    );
    let e = invalid("talk", json!({"action": "said", "at": "/a"}));
    assert_eq!(e.arg, "text, kind");
    // 1.9 (§9.2, §21): what each kind means; 1.11: ack first, with the words of such a post.
    assert!(
        e.why.ends_with(
            "kind: ack: 'I'll look into it', on it, or progress so far: answers nothing; reply: the \
             answer to what they asked; result: the outcome of the work; correction: replaces an \
             earlier said"
        ),
        "{}",
        e.why
    );
    let e = invalid(
        "talk",
        json!({"action": "said", "at": "/a", "text": "t", "kind": "progress"}),
    );
    assert_eq!(
        e.why,
        "\"progress\" is not a kind of said: ack: 'I'll look into it', on it, or progress so far: \
         answers nothing; reply: the answer to what they asked; result: the outcome of the work; \
         correction: replaces an earlier said"
    );
    // 1.11 (§20.7, §21, it5 l2, l4): a to_user with a said's kind, or none, is refused with
    // to_user's kinds and what each means.
    let e = invalid(
        "talk",
        json!({"action": "to_user", "text": "t", "kind": "reply"}),
    );
    assert_eq!(e.arg, "kind");
    assert_eq!(
        e.why,
        "\"reply\" is a kind of said, not of to_user: result: the outcome, which answers their \
         request; question: you ask them; ack: a promise, or anything else you want on record"
    );
    let e = invalid(
        "talk",
        json!({"action": "to_user", "text": "t", "kind": "correction"}),
    );
    assert!(
        e.why
            .starts_with("\"correction\" is a kind of said, not of to_user: result:"),
        "{}",
        e.why
    );
    let e = invalid("talk", json!({"action": "to_user", "text": "t"}));
    assert_eq!(e.arg, "kind");
    assert_eq!(
        e.why,
        "missing: talk to_user requires it; result: the outcome, which answers their request; \
         question: you ask them; ack: a promise, or anything else you want on record"
    );
    // 1.9 (§5): a sentence as an outcome; and the usual words for done are done.
    let e = invalid(
        "node",
        json!({"action": "close", "path": "/a", "outcome": "Implemented and PR opened",
               "summary": "s"}),
    );
    assert!(
        e.why.ends_with(
            "is not an outcome: one label, usually done, failed, dropped, superseded, or refuted \
             ([a-z0-9][a-z0-9_.:-]{0,63}); write the sentence in summary"
        ),
        "{}",
        e.why
    );
    // Inside an object, the path names it.
    let e = invalid(
        "talk",
        json!({"action": "decide", "at": "/a", "text": "t", "basis": [{"checked_at": "now"}]}),
    );
    assert_eq!(
        line(&e),
        "basis[0].external: missing: basis[0] requires it; something outside the forum, such as \
         an artefact"
    );
    // A `one_of` group the call gives none of is missing too, with its arguments' sources, so
    // that the retry is not refused again for it.
    let e = invalid("talk", json!({"action": "retract"}));
    assert_eq!(e.arg, "text, reason, entry|path");
    assert_eq!(
        e.why,
        "missing: talk retract requires text, reason, and one of entry and path; text: the \
         statement retracted; reason: why, in a sentence; entry: an eN, from your inbox, hello, \
         or the brief; path: a path or #<id>, as hello's offers, view find, and view list show \
         them"
    );
    assert_eq!(
        line(&invalid("node", json!({"action": "hide", "entry": "e5"}))),
        "reason: missing: node hide requires it; why, in a sentence"
    );
    assert_eq!(
        line(&invalid(
            "node",
            json!({"action": "hide", "reason": "a leaked key"})
        )),
        "entry|file: missing: node hide requires one of entry and file; entry: an eN, from your \
         inbox, hello, or the brief; file: a file of a node: {path, name}"
    );
    assert_eq!(
        line(&invalid("view", json!({"action": "node"}))),
        "path|paths: missing: view node requires one of path and paths; path: a path or #<id>, as \
         hello's offers, view find, and view list show them; paths: paths or #<id>s, as hello's \
         offers, view find, and view list show them"
    );
    let e = invalid("node", json!({"action": "key"}));
    assert_eq!(e.arg, "path, add|remove");
    assert!(
        e.why
            .starts_with("missing: node key requires path and one of add and remove; "),
        "{}",
        e.why
    );
    // Both of a group: refused as before, after the check for what is missing.
    assert_eq!(
        line(&invalid(
            "view",
            json!({"action": "node", "path": "/a", "paths": ["/b"]})
        )),
        "path|paths: view node takes exactly one of path and paths"
    );
}

/// 1.6 (§20.2, §21, it0: Codex agents' first `to_user` was refused for `at`, then again for a
/// missing `kind` and `verbatim`): a refusal names every problem of the call at once, foreign
/// arguments first, then missing ones, then values that do not parse, one per line, and ends with
/// the action's signature.
#[test]
fn a_refusal_names_every_problem_of_the_call_at_once() {
    // 1.8: `to_user` takes `at` (§8.1); `where`, an argument of other actions, is the foreign one.
    let e = invalid(
        "talk",
        json!({"action": "to_user", "where": "chat:c/t", "text": "done", "re": ["e1", "x"],
               "kind": "final", "principal": "user:sam"}),
    );
    assert_eq!(
        e.problems().map(|p| p.arg.as_str()).collect::<Vec<_>>(),
        ["where", "kind", "principal", "re[1]"]
    );
    assert_eq!(
        e.signature,
        "talk to_user takes {text*, kind*, verbatim, principal, re, at}"
    );
    let hint = ShellAnswer::Invalid(e).hint();
    let lines: Vec<&str> = hint.lines().collect();
    assert_eq!(lines.len(), 5, "{hint}");
    assert!(
        lines[0]
            .starts_with("where: where is not an argument of talk to_user, and would be ignored"),
        "{hint}"
    );
    // 1.11 (§21): what each of to_user's kinds means.
    assert_eq!(
        lines[1],
        "kind: \"final\" is not a kind of to_user: result: the outcome, which answers their \
         request; question: you ask them; ack: a promise, or anything else you want on record"
    );
    assert!(
        lines[2].starts_with("principal: \"user:sam\" is not a principal"),
        "{hint}"
    );
    assert_eq!(lines[3], "re[1]: \"x\" is not an entry id such as e57");
    assert_eq!(
        lines[4],
        "talk to_user takes {text*, kind*, verbatim, principal, re, at}"
    );
    // Missing arguments beside foreign ones and a value of the wrong type.
    let e = invalid(
        "hold",
        json!({"action": "release", "token": 7, "note": "n", "waits_for": {"node": "/a", "x": 1}}),
    );
    assert_eq!(
        e.problems().map(|p| p.arg.as_str()).collect::<Vec<_>>(),
        ["note", "waits_for.x", "done, next_step, reason", "token"]
    );
    // A value in a list, and in a nested object, each named at its place.
    let e = invalid(
        "node",
        json!({"action": "create", "path": "/a", "title": "t", "depends_on": ["#1", "a//b", "#x"],
               "repeat": {"cron": "every day", "run_within": "2 hours"}}),
    );
    assert_eq!(
        e.problems().map(|p| p.arg.as_str()).collect::<Vec<_>>(),
        [
            "depends_on[1]",
            "depends_on[2]",
            "repeat.cron",
            "repeat.run_within"
        ]
    );
    assert_eq!(e.problems().filter(|p| p.raw_node.is_some()).count(), 2);
    // A call with one problem names one, as before.
    let e = invalid(
        "hold",
        json!({"action": "take", "target": "/a", "lease": "soon"}),
    );
    assert!(e.more.is_empty());
    assert_eq!(e.arg, "lease");
}

/// 1.5 (R3, D.14.7): where to find each required argument, and each argument of a `one_of` group,
/// says more than its kind's noun ("a string") or an echo of its name ("why" for `reason`, "the
/// title" for `title`), which taught agents nothing in round R3.
#[test]
fn every_required_argument_says_where_to_find_it() {
    let mut checked = 0;
    let mut check = |at: String, a: &Arg| {
        let src = source_of(a);
        let words = src.split_whitespace().count();
        assert_ne!(src, kind_noun(a.kind), "{at}: only the kind's noun");
        assert!(words > 1, "{at}: {src:?} is one word");
        assert_ne!(src, format!("the {}", a.name), "{at}: an echo of the name");
        checked += 1;
    };
    for t in Tool::ALL {
        for action in t.spec().actions {
            for form in action.forms {
                let grouped: Vec<&str> =
                    form.one_of.iter().flat_map(|g| g.iter().copied()).collect();
                for a in form.args {
                    let at = format!("{} {} ({}) {}", t.name(), action.name, form.name, a.name);
                    if a.required || grouped.contains(&a.name) {
                        check(at.clone(), a);
                    }
                    if let Kind::Object(inner) = a.kind {
                        for i in inner.iter().filter(|x| x.required) {
                            check(format!("{at}.{}", i.name), i);
                        }
                    }
                }
            }
        }
    }
    assert!(checked >= 90, "the walk saw every form: {checked}");
    // The sources R3's review found wanting.
    let arg = |t: Tool, action: &str, name: &str| -> &'static Arg {
        t.spec()
            .action(action)
            .and_then(|a| a.forms.iter().find_map(|f| f.arg(name)))
            .unwrap()
    };
    assert_eq!(
        source_of(arg(Tool::Node, "reopen", "reason")),
        "why, in a sentence"
    );
    assert_eq!(
        source_of(arg(Tool::Control, "pause", "reason")),
        "why, in a sentence: every agent reads it"
    );
    assert_eq!(
        source_of(arg(Tool::Talk, "decide", "text")),
        "the choice made, or what settles the requests, in a sentence or two"
    );
    assert_eq!(
        source_of(arg(Tool::Node, "create", "title")),
        "a short line that names the work"
    );
    assert!(source_of(arg(Tool::Talk, "route", "to")).ends_with(", or none"));
    let cron = super::spec::REPEAT_ARGS
        .iter()
        .find(|a| a.name == "cron")
        .unwrap();
    assert!(source_of(cron).starts_with("a 5-field cron expression"));
}

/// The decoder checks arguments in name order, so the argument a refusal names does not depend
/// on the order the client wrote them in (`serde_json` keeps that order, `preserve_order`, for the
/// structured content, D.11.7).
#[test]
fn a_refusal_does_not_depend_on_the_order_of_the_arguments() {
    let mut a = Map::new();
    a.insert("action".into(), json!("take"));
    a.insert("zzz".into(), json!(1));
    a.insert("lease".into(), json!(true));
    a.insert("target".into(), json!("/a"));
    let mut b = Map::new();
    for k in ["target", "lease", "zzz", "action"] {
        b.insert(k.into(), a[k].clone());
    }
    let first = |m: Map<String, Value>| match prepare("hold", m, &env(65_536)) {
        Err(ShellAnswer::Invalid(e)) => e,
        other => panic!("{other:?}"),
    };
    let (x, y) = (first(a), first(b));
    assert_eq!(x, y);
    // Foreign arguments come first (1.6).
    assert_eq!(
        x.problems().map(|p| p.arg.as_str()).collect::<Vec<_>>(),
        ["zzz", "lease"]
    );
}

/// §20.1 and §21: a missing entry is not an answer that reaches nothing else. The decoder does not
/// refuse it, so that `limit(TextTooLong)` comes first, and the core answers it after a stored
/// `op_id` answer and the session check (review E2-spec-3; the engine's side is in
/// `mcp::server`'s tests).
#[test]
fn text_max_beats_a_missing_entry() {
    let long = "a".repeat(300);
    for (tool, v) in [
        (
            "talk",
            json!({"action": "retract", "entry": "e404", "text": long, "reason": "r"}),
        ),
        (
            "talk",
            json!({"action": "answer", "question": "e404", "text": long}),
        ),
        (
            "node",
            json!({"action": "hide", "entry": "e404", "reason": long}),
        ),
    ] {
        assert_eq!(
            with_text_max(tool, v.clone(), 256).unwrap_err(),
            ShellAnswer::TextTooLong {
                max: 256,
                longest: 300
            },
            "{tool} {v}"
        );
    }
    assert_eq!(
        cmd(
            "talk",
            json!({"action": "retract", "entry": "e404", "text": "t", "reason": "r"})
        ),
        Cmd::Talk(TalkCmd::Retract {
            target: RetractArg::Entry {
                eid: e(404),
                node: UNKNOWN_NODE
            },
            text: t("t"),
            reason: t("r")
        })
    );
}

/// Masking a map's names could make two of them one, losing an entry silently: refused, naming
/// the argument (review E2-bugs-5).
#[test]
fn names_that_mask_to_the_same_text_are_refused() {
    let a = "Zx7Lq2Wn9Bv4Kc6Mt1Rp8Hs3Jd5Gf0Ya";
    let b = "Qy8Mn3Vb7Xc2Lk9Hj4Gf6Ds1Ap5Zw0Te";
    let e = invalid(
        "node",
        json!({"action": "update", "path": "/a", "version": 1,
               "files": {format!("token={a}"): "https://x.example/one",
                         format!("token={b}"): "https://x.example/two"}}),
    );
    assert_eq!(e.arg, "files");
    assert!(
        e.why
            .starts_with("two names are the same once their secrets are masked")
    );
    // Names that stay apart once masked are kept, each masked.
    let p = dec(
        "node",
        json!({"action": "update", "path": "/a", "version": 1,
               "files": {format!("token={a}"): "https://x.example/one",
                         format!("password={b}"): "https://x.example/two"}}),
    );
    let Cmd::Node(NodeCmd::Update { patch, .. }) = &p.cmd else {
        panic!("an update");
    };
    assert_eq!(
        patch.files,
        Some(tm(&[
            ("password=[masked:password]", "https://x.example/two"),
            ("token=[masked:token]", "https://x.example/one"),
        ]))
    );
    assert_eq!(p.masked_fields, 2);
}

/// D.12.1: a retry across a change of the masking version is digested with the record's version
/// (CS-35). With one released version, a test masker stands for an older one, which masked
/// nothing, so that both branches of `digest_with` run (review E2-spec-12).
#[test]
fn a_retry_is_digested_with_its_records_masking_version() {
    fn two_versions(version: u32, s: &str) -> Option<String> {
        match version {
            0 => Some(s.to_string()),
            1 => crate::mask::mask_v(1, s),
            _ => None,
        }
    }
    let raw = json!({"action": "checkpoint", "token": "1.1", "done": format!("token={SECRET}"),
                     "next_step": "n"});
    let mut sent = raw.clone();
    sent["op_id"] = json!("op-1");
    sent["session"] = json!("s-1");
    let p = dec("hold", sent);
    // A record of version 0 is digested from the arguments as sent, masked with version 0.
    let v0 = crate::digest::request_digest("hold", &args(raw));
    assert!(v0.is_some());
    assert_ne!(v0, p.digest);
    assert_eq!(p.digest_under(0, 1, two_versions), v0);
    // Masking again with version 1 gives the digest that `prepare` computed with it.
    assert_eq!(p.digest_under(1, 0, two_versions), p.digest);
    assert_eq!(p.digest_under(1, 1, two_versions), p.digest);
    assert_eq!(p.digest_under(2, 1, two_versions), None);
}

/// §21 (1.4): a refusal's hint names what to do instead; for an action of another tool, that
/// tool (review E2-spec-6).
#[test]
fn an_action_of_another_tool_names_its_tool() {
    for (tool, action, want) in [
        (
            "session",
            "take",
            "; take is an action of another tool: call hold {action: take, …}",
        ),
        (
            "view",
            "take",
            "; take is an action of another tool: call hold {action: take, …}",
        ),
        (
            "node",
            "decide",
            "; decide is an action of another tool: call talk {action: decide, …}",
        ),
        (
            "hold",
            "decide",
            "; decide is an action of another tool: call talk {action: decide, …}",
        ),
        (
            "talk",
            "create",
            "; create is an action of another tool: call node {action: create, …}",
        ),
        (
            "control",
            "hello",
            "; hello is an action of another tool: call session {action: hello, …}",
        ),
        (
            "session",
            "constitution",
            "; constitution is an action of another tool: call view {action: constitution} or \
             control {action: constitution}",
        ),
    ] {
        let e = invalid(tool, json!({ "action": action }));
        assert_eq!(e.arg, "action");
        assert!(
            e.why
                .starts_with(&format!("{action:?} is not an action of {tool}: ")),
            "{e}"
        );
        assert!(e.why.ends_with(want), "{e}");
    }
    let e = invalid("hold", json!({"action": "fly"}));
    assert_eq!(
        e.why,
        "\"fly\" is not an action of hold: take, next, checkpoint, renew, release, check"
    );
}

/// §21 (1.4): the `eN` of a question asked with `talk ask` for text in `waits_for.question`; and a
/// node named in `re`, kept for the renderer to name its unsettled requests (review E2-spec-7).
#[test]
fn hints_name_what_to_do_instead() {
    // 1.8 (§8.1): `to_user` takes `at`, the node whose work it reports, under any of the names of
    // the node a call is about; 1.4's hint for it, `talk said`, is gone.
    for name in ["at", "path", "node"] {
        let p = dec(
            "talk",
            json!({"action": "to_user", "text": "t", "kind": "result", "verbatim": true,
                   name: "/a"}),
        );
        assert_eq!(p.ignored_detail(), None);
        assert!(
            matches!(&p.cmd, Cmd::Talk(TalkCmd::ToUser { at: Some(a), .. }) if *a == n("/a")),
            "{:?}",
            p.cmd
        );
    }
    let e = invalid(
        "hold",
        json!({"action": "release", "token": "1.1", "done": "d", "next_step": "n",
               "reason": "blocked", "waits_for": {"question": "the question about the schema"}}),
    );
    assert_eq!(e.arg, "waits_for.question");
    assert!(
        e.why
            .ends_with("pass the eN of a question you asked with talk ask, such as e9"),
        "{e}"
    );
    for (raw, node) in [("/a/b", n("/a/b")), ("#12", id(12)), ("a/b", n("a/b"))] {
        let e = invalid(
            "talk",
            json!({"action": "to_user", "text": "t", "kind": "result", "verbatim": true,
                   "re": ["e5", raw]}),
        );
        assert_eq!(e.arg, "re[1]");
        assert_eq!(e.raw_node.as_deref(), Some(raw));
        assert_eq!(crate::decode::values::node(raw), Ok(node));
        assert!(e.why.contains("is a node, not an entry id"), "{e}");
    }
    // A text that is not plainly a node, a bare word or a key, is only not an entry id.
    for raw in ["the request", "chat:c1/t1"] {
        let e = invalid(
            "talk",
            json!({"action": "to_user", "text": "t", "kind": "result", "verbatim": true,
                   "re": [raw]}),
        );
        assert_eq!(e.raw_node, None);
        assert_eq!(e.why, format!("{raw:?} is not an entry id such as e57"));
    }
}

/// 1.4 (R15): `hold release {intent_done}`, decoded like a checkpoint's (review E2-spec-4).
#[test]
fn release_takes_intent_done() {
    let p = dec(
        "hold",
        json!({"action": "release", "token": "1.1", "done": "d", "next_step": "n",
               "reason": "handoff", "intent_done": format!("pushed with token={SECRET}")}),
    );
    let shown = format!("{:?}", p.cmd);
    assert!(shown.contains("intent_done"), "{shown}");
    assert!(
        shown.contains("pushed with token=[masked:token]"),
        "{shown}"
    );
    assert_eq!(p.masked_fields, 1);
}

/// 1.4 (D.14.5): `view agreements {under?, limit?}`, under `/` by default, 50 by default and at
/// most 500 (review E2-spec-5).
#[test]
fn view_agreements_decodes() {
    let shown = format!(
        "{:?}",
        cmd(
            "view",
            json!({"action": "agreements", "under": "/kb", "limit": 20})
        )
    );
    assert!(shown.starts_with("View(Agreements"), "{shown}");
    let shown = format!("{:?}", cmd("view", json!({"action": "agreements"})));
    assert!(shown.starts_with("View(Agreements"), "{shown}");
}

/// 1.10 (§11.3, §20.2, §20.7): the verdict of `resolve` is `stands`, and `upheld`, its name until
/// 1.10, is read as it: under either name the command and the request digest are the same. Two
/// values that differ are `invalid`, naming both; the schema lists `stands` alone. In the lab's
/// fifth round (it4 l5), a reviewer who found a claim true passed `upheld: true`.
#[test]
fn resolve_takes_stands_and_reads_upheld_as_it() {
    let want = Cmd::Talk(TalkCmd::Resolve {
        objection: e(12),
        note: t("the counterexample holds"),
        upheld: true,
        defer: false,
    });
    let base = json!({"action": "resolve", "objection": "e12", "text": "the counterexample holds",
                      "op_id": "op-1"});
    let mut stands = base.clone();
    stands["stands"] = json!(true);
    let mut upheld = base.clone();
    upheld["upheld"] = json!(true);
    let mut both = base.clone();
    both["stands"] = json!(true);
    both["upheld"] = json!(true);
    let (a, b, c) = (dec("talk", stands), dec("talk", upheld), dec("talk", both));
    assert_eq!(a.cmd, want);
    assert_eq!(b.cmd, want);
    assert_eq!(c.cmd, want);
    assert!(
        a.ignored.is_empty() && b.ignored.is_empty(),
        "{:?}",
        b.ignored
    );
    assert_eq!(
        a.digest, b.digest,
        "a retry under either name is the same request"
    );
    let mut differ = base.clone();
    differ["stands"] = json!(true);
    differ["upheld"] = json!(false);
    let err = invalid("talk", differ);
    assert_eq!(err.arg, "stands|upheld");
    assert!(
        err.why
            .starts_with("two values given: stands true and upheld false"),
        "{}",
        err.why
    );
    // Elsewhere the older name is an argument of resolve, as `stands` is: ignored, not a typo.
    let p = dec(
        "talk",
        json!({"action": "decide", "at": "/a", "text": "x", "upheld": true}),
    );
    assert_eq!(p.ignored.len(), 1);
    assert_eq!(p.ignored[0].takers, "talk resolve");
    let schema = crate::mcp::schema::input_schema(crate::decode::spec::tool("talk").unwrap());
    assert!(schema["properties"].get("stands").is_some());
    assert!(schema["properties"].get("upheld").is_none());
}

/// 1.11 (§11.3, §20.7): `resolve {defer: true}` defers the objection, a third answer beside the
/// verdicts; with `stands` (or `upheld`, its older name) it is `invalid`, naming both, since a
/// deferral decides nothing. `defer` is in the schema, without a description (the tool list's
/// bound), and `stands` names it. In the lab's sixth round (it5 l5), every reviewer found a review
/// that neither held nor fell.
#[test]
fn resolve_takes_defer_without_a_verdict() {
    let p = dec(
        "talk",
        json!({"action": "resolve", "objection": "e12", "text": "nothing certifies it yet",
               "defer": true}),
    );
    assert_eq!(
        p.cmd,
        Cmd::Talk(TalkCmd::Resolve {
            objection: e(12),
            note: t("nothing certifies it yet"),
            upheld: false,
            defer: true,
        })
    );
    for verdict in ["stands", "upheld"] {
        let mut both = json!({"action": "resolve", "objection": "e12", "text": "x",
                              "defer": true});
        both[verdict] = json!(false);
        let err = invalid("talk", both);
        assert_eq!(err.arg, "defer|stands", "{verdict}");
        assert!(err.why.contains("give one of them"), "{}", err.why);
    }
    let schema = crate::mcp::schema::input_schema(crate::decode::spec::tool("talk").unwrap());
    assert!(schema["properties"].get("defer").is_some());
    // 1.12 (it6 l5): `stands` says what each answer means of the objection, and `defer` what it is.
    assert!(
        schema["properties"]["stands"]["description"]
            .as_str()
            .unwrap()
            .ends_with("false: the objection fails; cannot tell yet: defer")
    );
    assert_eq!(
        schema["properties"]["defer"]["description"],
        json!("Cannot decide yet: say in text what evidence would decide it")
    );
}

/// 1.10 (§20.2, §20.7): `kind` on `talk say` is ignored, not refused: a note to agents has no kind,
/// and the warning says where the outcome of the session's own work goes. In the lab's fifth round
/// (it4 l7), four workers with no human sent their result as `say {kind: result}`, 4 of the run's 7
/// refusals; `kind` stays directing on the other actions that do not take it.
#[test]
fn kind_on_say_is_ignored_with_where_the_outcome_goes() {
    let p = dec(
        "talk",
        json!({"action": "say", "at": "/a", "text": "Result: merged", "kind": "result"}),
    );
    assert_eq!(
        p.cmd,
        cmd(
            "talk",
            json!({"action": "say", "at": "/a", "text": "Result: merged"})
        )
    );
    let detail = p.ignored_detail().unwrap();
    assert!(
        detail.starts_with("talk say does not take kind ("),
        "{detail}"
    );
    assert!(
        detail.ends_with(
            "so the call was applied without it; a note to other agents has no kind: the outcome \
             of your own work goes in close's summary, or in a release's done while it waits for \
             a merge"
        ),
        "{detail}"
    );
    let err = invalid(
        "talk",
        json!({"action": "decide", "at": "/a", "text": "x", "kind": "result"}),
    );
    assert_eq!(err.arg, "kind");
}

/// 1.12 (§20.7, §21): `to` on `talk to_user` is ignored, and the warning says that its addressee is
/// named by `principal`. In the lab's seventh round (it6 l2), a worker passed `to:
/// principal:<key>`, as `say` takes it, and the warning named only the actions that take `to`.
#[test]
fn to_on_to_user_names_principal() {
    let p = dec(
        "talk",
        json!({"action": "to_user", "text": "PR #49 fixes it", "kind": "result",
               "to": "principal:mail:sam@rivera.example"}),
    );
    let detail = p.ignored_detail().unwrap();
    assert!(
        detail.starts_with("talk to_user does not take to ("),
        "{detail}"
    );
    assert!(
        detail.ends_with(
            "so the call was applied without it; to_user names its addressee with principal: \
             <key>, not to"
        ),
        "{detail}"
    );
}

/// 1.10 (§20.2): a time also takes a bare duration, read as from now, as `in` reads it; in the
/// lab's fifth round, 2 of 13 `recheck_after` were bare durations, both refused (it4 l6, l7).
/// `checked_at`, a time in the past, still refuses one, which there could mean as long ago.
#[test]
fn a_bare_duration_is_a_time_from_now() {
    let release = |recheck: &str| {
        cmd(
            "hold",
            json!({"action": "release", "token": "1.1", "done": "d", "next_step": "n",
                   "reason": "blocked",
                   "waits_for": {"external": "PR #215 merged", "recheck_after": recheck}}),
        )
    };
    assert_eq!(release("2h"), release("in 2h"));
    assert_eq!(release("2d"), release("in 48h"));
    assert_eq!(values::when("15m"), Ok(When::In(Dur(15 * 60_000))));
    assert_eq!(values::when("in 15m"), Ok(When::In(Dur(15 * 60_000))));
    assert!(values::when("2026-10-01T12:00:00Z").is_ok());
    let why = values::when("soon").unwrap_err();
    assert!(
        why.contains("a duration from now such as 10m or in 10m"),
        "{why}"
    );
    assert!(values::checked_at("2h").is_err());
    assert_eq!(values::checked_at("in 0s"), Ok(When::In(Dur(0))));
}

/// 1.10 (§20.5, §21): a needs item that is not a label is refused with what needs name and where a
/// blocker goes. In the lab's fifth round (it4 l4 C), a sentence about missing access, refused with
/// the pattern alone, took three calls to reach the release's wait.
#[test]
fn a_needs_item_that_is_no_label_names_the_place_for_a_blocker() {
    let sentence = "Access to the Airflow production runbook.";
    for (action, arg) in [
        ("create", "needs"),
        ("update", "needs_add"),
        ("update", "needs_remove"),
    ] {
        let mut call = if action == "create" {
            json!({"action": action, "path": "/a/b", "title": "t"})
        } else {
            json!({"action": action, "path": "/a/b", "version": 1})
        };
        call[arg] = json!([sentence]);
        let err = invalid("node", call);
        assert_eq!(err.arg, format!("{arg}[0]"));
        assert_eq!(
            err.why,
            format!(
                "{sentence:?} is not a label ([a-z0-9][a-z0-9_.:-]{{0,63}}): needs name caps, such \
                 as python or sql; a blocker goes in a checkpoint's next_step or a release's \
                 waits_for"
            )
        );
    }
    // Other labels keep their hint.
    let err = invalid(
        "node",
        json!({"action": "create", "path": "/a/b", "title": "t", "tags": ["Not a tag"]}),
    );
    assert_eq!(
        err.why,
        "\"Not a tag\" is not a label: [a-z0-9][a-z0-9_.:-]{0,63}"
    );
}
