//! Regression tests of the review of WP-E3 (D.13.2): each reproduces a finding and keeps it fixed.
//! The tests of 1.4 renderings whose core types are still to come (WP-D1 to WP-D6) are written
//! against the calls that will produce them, and ignored with the packages they wait for.

use super::*;

use crate::decode::DecodeError;

/// The mark every secret of `hidden_texts_show_in_no_view` carries.
const LEAK: &str = "LEAK";

/// The text and the structured content of a response, for a search.
fn all_of(r: &Response) -> String {
    format!("{}\n{}", r.text, r.structured)
}

/// Every view, the status line, the hints, an empty `next`, `control health`, and `hello` show
/// none of a hidden entry's texts nor a hidden file's value (§16, D.11.7): a request from outside
/// with its requester, a decision with an external basis, a question for the user with its options,
/// a note, a `from_user`, a pause's reason, and a node's file (E3-spec-1, E3-spec-2, E3-bugs-1,
/// E3-spec-18).
#[test]
fn hidden_texts_show_in_no_view() {
    let mut k = Kit::new();
    let (s1, _) = k.hello(
        "coder",
        json!({"action": "hello", "principal": "mail:alice@example.org"}),
    );
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/work", "keys": ["chat:team/1"], "title": "work"}),
    );
    ok(&c);
    let work = k.node("/work");
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/work/sub", "title": "a part", "tags": ["offer"],
               "take": false}),
    );
    ok(&c);
    let alice = k.w.s.sessions[&s1]
        .principals
        .iter()
        .copied()
        .next()
        .unwrap();
    k.at(60_000);
    let mut ids: Vec<Eid> = Vec::new();
    let mut note = Eid(0);
    k.events(|d| {
        ids.push(request_event(
            d,
            work,
            s1,
            "chat:team/1/m1",
            "chat:team/1",
            "LEAK-REQUEST-TEXT",
            "LEAK-REQUESTER",
        ));
        ids.push(append(
            d,
            work,
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
            text("LEAK-DECISION", &[(TextField::Basis, "LEAK-IN-BASIS")]),
        ));
        let due = d.clock.plus(d.s.params.user_question_due);
        ids.push(append(
            d,
            work,
            s1,
            EntryBody::Question(QuestionBody::User {
                source: UqSource::Agent {
                    session: s1,
                    digest: ReqDigest([3; 32]),
                },
                about: Some(work),
                to: vec![alice],
                due,
                has_default: true,
            }),
            text(
                "LEAK-QUESTION",
                &[
                    (TextField::Option, "LEAK-IN-OPTION"),
                    (TextField::Recommendation, "LEAK-RECOMMENDATION"),
                    (TextField::Default, "LEAK-DEFAULT"),
                ],
            ),
        ));
        let until = d.clock.plus(d.s.params.deliver_window);
        note = append(
            d,
            work,
            s1,
            EntryBody::Note {
                to: Some(Addressee::Session(s1)),
                until: Some(until),
                recipients: [s1].into(),
                pending_holder: false,
                expects: None,
                reply_to: None,
            },
            text("LEAK-NOTE", &[]),
        );
        ids.push(note);
        ids.push(append(
            d,
            USER,
            s1,
            EntryBody::FromUser { principal: alice },
            text("LEAK-FROM-USER", &[(TextField::Client, "ide")]),
        ));
        d.push(Ev::NodeUpdated {
            node: work,
            patch: NodePatch {
                files: Some([(t("spec"), t("https://x.example/spec?v=LEAK-IN-FILE"))].into()),
                ..NodePatch::default()
            },
        });
    });
    let c = k.call(
        Some(s1),
        "control",
        json!({"action": "pause", "scope": "/work", "reason": "LEAK-PAUSE-REASON"}),
    );
    ok(&c);
    let pause_entry = StoreSource::new(&k.w.conn).newest(work, 1)[0].clone();
    assert!(matches!(
        pause_entry.body,
        EntryBody::System(SystemNote::Paused { .. })
    ));
    ids.push(pause_entry.eid);
    // Everything is hidden: each entry, and the file.
    k.at(60_000);
    for e in &ids {
        let c = k.call(
            Some(s1),
            "node",
            json!({"action": "hide", "entry": e.render(), "reason": "a secret slipped in"}),
        );
        ok(&c);
    }
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "hide", "file": {"path": "/work", "name": "spec"},
               "reason": "a secret slipped in"}),
    );
    ok(&c);
    // The sources agree that the pause's reason is hidden.
    let events: Vec<Event> = crate::store::history::events_after(&k.w.conn, 0, u32::MAX)
        .unwrap()
        .into_iter()
        .map(|x| x.event)
        .collect();
    let mem = MemorySource::of(&events);
    let store = StoreSource::new(&k.w.conn);
    assert_eq!(
        store.pause_reason(work),
        Some(t("[hidden: a secret slipped in]"))
    );
    assert_eq!(mem.pause_reason(work), store.pause_reason(work));
    // Every view of the node, its history, the logs, and the board.
    let mut shown: Vec<(String, String)> = Vec::new();
    let mut look = |k: &mut Kit, title: &str, sess: SessionId, tool: &str, args: Value| {
        let class_name = k.w.s.sessions[&sess].class.to_string();
        let c = k.call_as(&class_name, Some(sess), tool, args, None);
        shown.push((title.to_string(), all_of(&c.response)));
    };
    look(
        &mut k,
        "brief",
        s1,
        "view",
        json!({"action": "node", "path": "/work"}),
    );
    look(
        &mut k,
        "full",
        s1,
        "view",
        json!({"action": "node", "path": "/work", "view": "full"}),
    );
    look(
        &mut k,
        "review",
        s1,
        "view",
        json!({"action": "node", "path": "/work", "view": "review"}),
    );
    look(
        &mut k,
        "feed",
        s1,
        "view",
        json!({"action": "feed", "at": "/work"}),
    );
    look(
        &mut k,
        "history",
        s1,
        "view",
        json!({"action": "history", "path": "/work", "limit": 200}),
    );
    look(
        &mut k,
        "user_log",
        s1,
        "view",
        json!({"action": "user_log"}),
    );
    look(
        &mut k,
        "requests",
        s1,
        "view",
        json!({"action": "requests", "requester": "all"}),
    );
    look(&mut k, "hello", s1, "session", json!({"action": "hello"}));
    // Another session: a take refused under the pause, and an empty next that the pause caused.
    let (s2, _) = k.hello("reviewer", json!({"action": "hello"}));
    look(
        &mut k,
        "paused hint",
        s2,
        "hold",
        json!({"action": "take", "target": "/work/sub"}),
    );
    look(&mut k, "empty next", s2, "hold", json!({"action": "next"}));
    // control health with the pause as its first reason.
    k.health = Health {
        level: Level::Red,
        reasons: vec![HealthReason::Paused {
            level: Level::Red,
            count: 1,
            top: vec![PauseExclusion {
                pause: PauseView {
                    scope: work,
                    by: s1,
                    class: class("coder"),
                    until: k.w.clock.plus(Dur(3_600_000)),
                },
                excluded: 1,
            }],
        }],
        capacity: forum_core::health::CapacityReport::default(),
    };
    look(
        &mut k,
        "control health",
        s1,
        "control",
        json!({"action": "health"}),
    );
    // The note as an inbox delivers it, in `wait` and in `hello`.
    let view = Cmd::View(ViewCmd::Constitution);
    let r = k.render(
        Some(s1),
        &view,
        &done(Reply::Waited {
            delivered: vec![note],
            news: None,
        }),
    );
    shown.push(("wait".into(), all_of(&r)));
    let src = StoreSource::new(&k.w.conn);
    let h = hello::render_hello(
        &k.w.s,
        s1,
        k.w.clock,
        &k.health,
        &hello::HelloInput {
            delivered: Some(&[note]),
            user_entry: None,
            hits: &k.hits,
            route: &hello::RouteCall::default(),
            first: true,
        },
        &src,
    );
    shown.push(("hello with the note".into(), h.text()));
    for (title, text) in &shown {
        assert!(!text.contains(LEAK), "{title} shows a hidden text:\n{text}");
    }
    let joined: String = shown.iter().map(|(_, x)| x.as_str()).collect();
    for want in [
        // The status line names the pause over the hold, its reason hidden.
        "(paused: /work by coder/s-1 until",
        "[hidden: a secret slipped in]",
        "paused at /work by coder/s-1",
        "\"spec\":\"[hidden: a secret slipped in]\"",
        "\"options\":[\"[hidden: a secret slipped in]\"]",
        "\"basis\":[\"external: [hidden: a secret slipped in]",
        "\"requester\":\"[hidden: a secret slipped in]\"",
    ] {
        assert!(joined.contains(want), "{want} missing:\n{joined}");
    }
}

/// `view history` shows each event in the shell's formats: its name in snake case, nodes as paths
/// or `#id`, sessions as `s-N`, tokens as `12.3`, and times as RFC 3339, never the core's numbers
/// and variant names (E3-spec-8).
#[test]
fn history_shows_events_in_the_shells_formats() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/work", "title": "work", "keys": ["chat:team/1"],
               "due": "2026-10-01T00:00:00Z", "tags": ["offer"]}),
    );
    let token = match &ok(&c).reply {
        Reply::Created { token: Some(t), .. } => *t,
        other => panic!("{other:?}"),
    };
    k.at(60_000);
    let c = k.call(
        Some(s1),
        "hold",
        json!({"action": "checkpoint", "token": token.render(), "done": "a start",
               "next_step": "go on", "intent": "post the fix"}),
    );
    ok(&c);
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "update", "path": "/work", "version": 1, "status": "half done",
               "token": token.render()}),
    );
    ok(&c);
    let c = k.call(
        Some(s1),
        "control",
        json!({"action": "pause", "scope": "/work", "reason": "a release"}),
    );
    ok(&c);
    let c = k.call(
        Some(s1),
        "view",
        json!({"action": "history", "path": "/work", "limit": 200}),
    );
    let v = &c.response.structured;
    let events = v["events"].as_array().unwrap();
    assert!(events.len() >= 8, "{v}");
    let names: Vec<&str> = events
        .iter()
        .map(|e| e["event"].as_str().expect("a name"))
        .collect();
    for want in [
        "node_created",
        "hold_issued",
        "intent_opened",
        "hold_extended",
        "node_updated",
        "paused",
        "appended",
    ] {
        assert!(names.contains(&want), "{want} in {names:?}");
    }
    for name in &names {
        assert!(
            name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
            "{name}"
        );
    }
    // No id, session, token, or time is a number or an object of the core's.
    fn walk(v: &Value, f: &mut dyn FnMut(&str, &Value)) {
        match v {
            Value::Object(m) => {
                for (k, x) in m {
                    f(k, x);
                    walk(x, f);
                }
            }
            Value::Array(xs) => xs.iter().for_each(|x| walk(x, f)),
            _ => {}
        }
    }
    let mut seen = 0;
    for e in events {
        walk(e, &mut |key, x| {
            let typed = [
                "node",
                "parent",
                "session",
                "token",
                "lease_until",
                "until",
                "at",
                "due",
                "intent",
                "scope",
                "by",
            ];
            if typed.contains(&key) {
                seen += 1;
                assert!(x.is_string() || x.is_null(), "{key}: {x} in {e}");
            }
        });
    }
    assert!(seen > 20);
    let text = &c.response.text;
    assert!(text.contains(r#""token":"8.1""#), "{text}");
    assert!(text.contains(r#""session":"s-1""#), "{text}");
    assert!(text.contains(r#""by":"checkpoint""#), "{text}");
    assert!(text.contains(r#""due":"2026-10-01T00:00:00Z""#), "{text}");
    assert!(!text.contains("Checkpoint\""), "no variant names: {text}");
    assert!(!text.contains("{\"fence\""), "no tokens as objects: {text}");
}

/// Section 9 counts the entries it draws from: when every entry fits, every section shows as many
/// as it counts; past the entries it scans, its total is a lower bound, `≥k` (E3-spec-3).
#[test]
fn everything_else_counts_what_it_draws_from() {
    let shape = BriefShape {
        budget: 60_000,
        digest: 10,
        decisions: vec![50; 2],
        requests: vec![50; 2],
        saids: vec![50],
        notes: vec![50; 3],
        checkpoint: Some(30),
        predecessors: Vec::new(),
    };
    let (k, s1, n) = brief_world(&shape);
    let src = StoreSource::new(&k.w.conn);
    let (j, _) = brief::render_brief(&k.w.s, n, Some(s1), k.w.clock, &src);
    let v: Value = serde_json::from_str(&j.text()).unwrap();
    // Nothing was cut, so no section carries counts (1.7, §18).
    for x in v["sections"].as_array().unwrap() {
        assert!(x.get("shown").is_none() && x.get("total").is_none(), "{x}");
    }
    assert_eq!(
        section_named(&v, "everything_else")["items"]
            .as_array()
            .unwrap()
            .len(),
        3,
        "the three notes"
    );
    // 80 notes: the scan of 50 sees 49 of them, and the rest is counted, not read.
    let shape = BriefShape {
        notes: vec![20; 80],
        ..shape
    };
    let (k, s1, n) = brief_world(&shape);
    let src = StoreSource::new(&k.w.conn);
    let (j, _) = brief::render_brief(&k.w.s, n, Some(s1), k.w.clock, &src);
    let v: Value = serde_json::from_str(&j.text()).unwrap();
    let nine = section_named(&v, "everything_else");
    assert_eq!(nine["shown"], json!(49), "{nine}");
    assert_eq!(nine["total"], json!("≥80"), "{nine}");
}

/// A refusal by `write.per_hour`, busy, or a text over `text.max` carries no next calls, whatever
/// the guidance holds; another refusal carries them (§20.1, §21, D.11.7; E3-spec-4).
#[test]
fn limit_refusals_of_the_shell_carry_no_next_calls() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let g = guidance(
        vec![item(
            Rule::MoreWork,
            Moment::Now,
            Skeleton::Next,
            Because::MoreWork { at_hello: false },
        )],
        Total::Exact(1),
    );
    let src = StoreSource::new(&k.w.conn);
    let view = Cmd::View(ViewCmd::Constitution);
    let refused = |kind: LimitKind| {
        let out = Err(Error {
            code: ErrorCode::Limit,
            hint: Hint::Limit(kind),
        });
        response(&Input {
            state: &k.w.s,
            now: k.w.clock,
            session: Some(s1),
            opened: None,
            cmd: &view,
            node_args: &[],
            out: &out,
            guidance: Some(&g),
            health: &k.health,
            health_at: k.w.clock,
            hits: &k.hits,
            masked_fields: 0,
            ignored: None,
            src: &src,
            seen: None,
            rebind: None,
        })
    };
    for kind in [
        LimitKind::WritesPerHour,
        LimitKind::Busy,
        LimitKind::TextTooLong,
    ] {
        let r = refused(kind);
        assert!(r.text.starts_with("[swarm] "), "{}", r.text);
        assert!(r.text.contains("error: limit\nhint: "), "{}", r.text);
        assert!(!r.text.contains("next_calls"), "{kind:?}: {}", r.text);
        assert!(r.structured.get("next_calls").is_none(), "{kind:?}");
    }
    let r = refused(LimitKind::Renew);
    assert!(r.text.contains("next_calls (1 of 1):"), "{}", r.text);
}

/// A principal key given in `owners` gets the person's node in its hint, or how to make one; not
/// the hint of a key that a conversation should carry (§21; E3-spec-7).
#[test]
fn a_principal_in_owners_gets_the_persons_node() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let create = json!({"action": "create", "path": "/x", "title": "x",
                        "owners": ["mail:alice@example.org"]});
    let c = k.call(Some(s1), "node", create.clone());
    assert!(c.response.is_error, "{}", c.response.text);
    assert!(
        c.response.text.contains(
            "mail:alice@example.org is a principal, and no person node carries \
             person:mail:alice@example.org: create one under /@people with that key, then pass \
             it in owners"
        ),
        "{}",
        c.response.text
    );
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/@people/alice", "title": "Alice",
               "keys": ["person:mail:alice@example.org"], "take": false}),
    );
    ok(&c);
    let alice = k.node("/@people/alice");
    let c = k.call(Some(s1), "node", create);
    assert!(c.response.is_error, "{}", c.response.text);
    assert!(
        c.response.text.contains(&format!(
            "mail:alice@example.org is a principal: pass their person node /@people/alice ({}) \
             in owners",
            alice.render()
        )),
        "{}",
        c.response.text
    );
    assert!(!c.response.text.contains("a conversation key as where"));
}

/// An `intent_done` with no intent open names the checkpoint that closed the last one (§21), and a
/// checkpoint returns its own entry (§20.6; E3-spec-11, E3-spec-13).
#[test]
fn an_intent_done_with_none_open_names_the_checkpoint_that_closed_it() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/work", "title": "work"}),
    );
    let token = match &ok(&c).reply {
        Reply::Created { token: Some(t), .. } => *t,
        other => panic!("{other:?}"),
    };
    let checkpoint = |k: &mut Kit, extra: Value| {
        let mut args = json!({"action": "checkpoint", "token": token.render(), "done": "a step",
                              "next_step": "the next"});
        for (key, v) in extra.as_object().unwrap() {
            args[key] = v.clone();
        }
        k.call(Some(s1), "hold", args)
    };
    // With no intent ever opened: the plain hint.
    let c = checkpoint(&mut k, json!({"intent_done": "it happened"}));
    assert!(c.response.is_error);
    assert!(
        c.response
            .text
            .contains("no intent is open on the node, so there is nothing for intent_done"),
        "{}",
        c.response.text
    );
    let c = checkpoint(&mut k, json!({"intent": "post the fix"}));
    let opened = ok(&c).clone();
    let entry = c.response.structured["entry"].as_str().unwrap().to_string();
    assert_eq!(
        Some(entry.as_str()),
        c.response.structured["intent"].as_str(),
        "the checkpoint that opens an intent is the intent"
    );
    let _ = opened;
    let c = checkpoint(&mut k, json!({"intent_done": "posted it"}));
    ok(&c);
    let closer = c.response.structured["entry"].as_str().unwrap().to_string();
    assert_ne!(closer, entry);
    let c = checkpoint(&mut k, json!({"intent_done": "posted it again"}));
    assert!(c.response.is_error);
    assert!(
        c.response.text.contains(&format!(
            "no intent is open on the node: {closer} closed the last one, so there is nothing \
             for intent_done to close: leave it out"
        )),
        "{}",
        c.response.text
    );
}

/// A node named in `re`, which takes entry ids, gets that node's unsettled requests in the hint
/// (§21; E3-spec-11).
#[test]
fn a_node_named_in_re_lists_its_unsettled_requests() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/work", "keys": ["chat:team/1"], "title": "work"}),
    );
    ok(&c);
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/quiet", "title": "no requests", "take": false}),
    );
    ok(&c);
    let work = k.node("/work");
    let mut rs = Vec::new();
    k.events(|d| {
        for i in 0..7 {
            rs.push(request_event(
                d,
                work,
                s1,
                &format!("chat:team/1/m{i}"),
                "chat:team/1",
                "fix it",
                "Bob",
            ));
        }
    });
    let src = StoreSource::new(&k.w.conn);
    let refusal = |raw: &str| {
        // The decoder's refusal of `re[0]`, with the raw text it keeps (WP-E2).
        let mut e = DecodeError::new("re[0]", format!("{raw:?} is not an entry id such as e57"));
        e.raw_node = Some(raw.into());
        shell_refusal(
            &k.w.s,
            Some(s1),
            k.w.clock,
            &k.health,
            &ShellAnswer::Invalid(e),
            &src,
            None,
        )
    };
    let r = refusal("/work");
    let first: Vec<String> = rs.iter().take(5).map(|e| e.render()).collect();
    assert!(
        r.text.contains(&format!(
            "re[0] names the node /work; its unsettled requests (the first 5 of 7) are {}: pass \
             those in re",
            first.join(", ")
        )),
        "{}",
        r.text
    );
    assert!(r.text.starts_with("[swarm] "));
    let r = refusal("#9");
    assert!(
        r.text
            .contains("re[0] names the node /quiet, which has no unsettled request"),
        "{}",
        r.text
    );
    // Text that names no node gets no such hint.
    let r = refusal("hello");
    assert!(!r.text.contains("names the node"), "{}", r.text);
}

/// The same through the decoder, which keeps the raw text of a node given in `re` as it keeps a
/// node argument's.
#[test]
fn to_user_with_a_node_in_re_lists_its_unsettled_requests() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/work", "keys": ["chat:team/1"], "title": "work"}),
    );
    ok(&c);
    let work = k.node("/work");
    let mut r = Eid(0);
    k.events(|d| {
        r = request_event(
            d,
            work,
            s1,
            "chat:team/1/m1",
            "chat:team/1",
            "fix it",
            "Bob",
        );
    });
    let Value::Object(map) = json!({"action": "to_user", "session": s1.render(), "text": "fixed",
                                    "kind": "result", "verbatim": true, "re": ["/work"]})
    else {
        unreachable!()
    };
    let env = DecodeEnv {
        now: k.w.clock,
        text_max: k.w.s.params.text_max,
        coverage_window: k.w.s.params.coverage_window,
        entries: &Lookup(&k.w.conn),
    };
    let answer = prepare("talk", map, &env).unwrap_err();
    let src = StoreSource::new(&k.w.conn);
    let resp = shell_refusal(&k.w.s, Some(s1), k.w.clock, &k.health, &answer, &src, None);
    assert!(
        resp.text.contains(&format!(
            "its unsettled requests are {}: pass those in re",
            r.render()
        )),
        "{}",
        resp.text
    );
}

/// An optional fill says so: `intent?` in the remedy of `invalid(IntentOpen)`, and `basis?` in a
/// decision (D.8.26; E3-spec-14).
#[test]
fn optional_fills_are_marked() {
    let l = long_forum();
    let s = &l.k.w.s;
    let c = next_calls::call(
        s,
        Some(l.sess),
        &Skeleton::CheckpointSwap {
            token: long_token(),
        },
    );
    assert_eq!(c.fill, vec!["intent_done", "intent?", "done", "next_step"]);
    let text = next_calls::call_text(&c);
    assert!(
        text.ends_with("intent_done: ?, intent?: ?, done: ?, next_step: ?}"),
        "{text}"
    );
    for supersedes in [None, Some(eid())] {
        let c = next_calls::call(
            s,
            Some(l.sess),
            &Skeleton::Decide {
                at: l.node,
                settles: Vec::new(),
                supersedes,
            },
        );
        assert_eq!(c.fill, vec!["text", "basis?"]);
        assert!(next_calls::call_text(&c).ends_with("text: ?, basis?: ?}"));
    }
    assert_eq!(next_calls::fill_arg("intent?"), "intent");
    assert_eq!(next_calls::fill_arg("text"), "text");
    assert!(crate::mcp::tools::SESSION.contains("an <arg>?: ? only if it applies"));
}

/// A node in an open `conflict` signal is disputed in search hits, whichever end of the pair it
/// is; an open objection makes one disputed too (§18, D.11.12; E3-spec-16).
#[test]
fn a_node_in_an_open_conflict_signal_is_disputed() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    for p in ["/a", "/b", "/c", "/signal"] {
        let c = k.call(
            Some(s1),
            "node",
            json!({"action": "create", "path": p, "title": p, "take": false}),
        );
        ok(&c);
    }
    let (a, b, c3, sig) = (k.node("/a"), k.node("/b"), k.node("/c"), k.node("/signal"));
    assert!(!hello::disputed(&k.w.s, a));
    // `b` names `a` as conflicting: the signal's key is (conflict, a, b).
    k.events(|d| {
        d.push(Ev::LinkAdded {
            node: b,
            rel: LinkRel::ConflictsWith,
            to: a,
        });
        d.push(Ev::SignalOpened {
            key: SignalKey {
                kind: SignalKind::Conflict,
                a,
                b,
            },
            node: sig,
        });
    });
    let s = &k.w.s;
    assert!(hello::disputed(s, a), "the smaller node");
    assert!(hello::disputed(s, b), "the larger node");
    assert!(!hello::disputed(s, c3));
    let src = StoreSource::new(&k.w.conn);
    let h = hello::hit(s, b, 200, None, &src);
    assert_eq!(h.get("disputed"), Some(&json::Json::Bool(true)));
    // A duplicate signal disputes nothing.
    k.events(|d| {
        d.push(Ev::SignalResolved {
            key: SignalKey {
                kind: SignalKind::Conflict,
                a,
                b,
            },
        });
        d.push(Ev::SignalOpened {
            key: SignalKey {
                kind: SignalKind::Duplicate,
                a,
                b,
            },
            node: sig,
        });
    });
    assert!(!hello::disputed(&k.w.s, a));
    assert!(!hello::disputed(&k.w.s, b));
}

/// The brief's header carries `status` and `est` in §10.4's order (E3-spec-17), and only when they
/// are set (1.7, §20.1): an empty status, a missing `est`, no holders, and a rollup of zeros are
/// left out.
#[test]
fn the_brief_header_has_status_and_est_when_set() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/work", "title": "work", "take": false}),
    );
    ok(&c);
    let header = |k: &Kit| {
        let src = StoreSource::new(&k.w.conn);
        let (j, _) = brief::render_brief(&k.w.s, k.node("/work"), Some(s1), k.w.clock, &src);
        j.get("node").unwrap().clone()
    };
    let head = header(&k);
    let json::Json::O(fields) = &head else {
        panic!("an object")
    };
    let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        ["id", "path", "title", "state", "version", "digest_version"]
    );
    ok(&k.call(
        Some(s1),
        "node",
        json!({"action": "update", "path": "/work", "version": 1, "status": "drafting",
               "est": 30}),
    ));
    ok(&k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/work/part", "title": "part", "take": false}),
    ));
    let head = header(&k);
    let json::Json::O(fields) = &head else {
        panic!("an object")
    };
    let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys.get(..8).unwrap(),
        [
            "id",
            "path",
            "title",
            "state",
            "status",
            "est",
            "version",
            "digest_version"
        ]
    );
    assert_eq!(head.get("status"), Some(&json::Json::s("drafting")));
    assert_eq!(head.get("est"), Some(&json::Json::U(30)));
    assert_eq!(
        head.get("rollup"),
        Some(&json::Json::s("children 1 open; below: 1 open"))
    );
}

/// 1.5 (§13, §20.3): the inbox is never cut. The notes a `hello` delivers are marked read, so a
/// note cut from it would never be delivered (the shell's review, WP-E3); the core delivers at
/// most 10, oldest first. Against any budget, every delivered note is shown, in order, each
/// item's text cut to its share, with no counts when nothing else waits (1.7, §18); the droppable
/// sections go first. This replaces 1.4's cut, newest kept (E3-bugs-6, E3-bugs-7).
#[test]
fn the_inbox_is_never_cut() {
    let shape = HelloShape {
        budget: 4_096,
        notes: (0..10).map(|i| 400 + 13 * i).collect(),
        offers: Vec::new(),
        others: 0,
        map: 0,
        questions: vec![20; 2],
        digest: Vec::new(),
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
    for budget in [200_usize, 1_000, 2_500, 4_096, 30_000] {
        let mut s = k.w.s.clone();
        s.params.hello_budget = u32::try_from(budget).unwrap();
        let h = hello::render_hello(&s, a, k.w.clock, &k.health, &input, &src);
        let v: Value = serde_json::from_str(&h.text()).unwrap();
        let items = v["inbox"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 10, "budget {budget}");
        assert!(v["inbox"].get("shown").is_none(), "budget {budget}");
        assert!(v["inbox"].get("total").is_none(), "budget {budget}");
        for (item, e) in items.iter().zip(&notes) {
            assert_eq!(item["e"], json!(e.render()), "oldest first");
        }
        if h.size() > budget {
            // Only the sections that are never dropped are left.
            for gone in [
                "constitution",
                "knowledge",
                "related",
                "present",
                "orientation",
            ] {
                assert!(v.get(gone).is_none(), "budget {budget}: {gone}");
            }
        }
    }
}

/// Whatever the core delivers is shown whole, even a long inbox over a small budget: the
/// renderer never chooses which notes an agent reads.
#[test]
fn a_long_delivered_inbox_is_shown_whole() {
    let shape = HelloShape {
        budget: 4_096,
        notes: vec![300; 40],
        offers: Vec::new(),
        others: 0,
        map: 0,
        questions: Vec::new(),
        digest: Vec::new(),
        options: 1,
    };
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
    let items = v["inbox"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 40);
    assert_eq!(v["inbox"]["shown"], v["inbox"]["total"]);
    assert_eq!(
        items.last().unwrap()["e"],
        json!(notes.last().unwrap().render())
    );
}

/// What rests on a node pages to its end: a `≥k` list says "for more" only on a full page, and a
/// page past `u32::MAX`'s neighbour neither overflows nor offers more (E3-bugs-8, E3-bugs-9).
#[test]
fn what_rests_on_a_node_pages_to_its_end() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/lemma", "title": "a lemma", "take": false}),
    );
    ok(&c);
    let lemma = k.node("/lemma");
    // More decisions on the node than the scan of its citations visits: `cited_by` is `≥0`.
    k.events(|d| {
        for _ in 0..=forum_core::query::SCAN_MAX {
            append(
                d,
                lemma,
                s1,
                EntryBody::Decision {
                    settles: BTreeSet::new(),
                    supersedes: None,
                    assumes: BTreeSet::new(),
                    evidence: None,
                    basis: Vec::new(),
                },
                text("a step", &[]),
            );
        }
    });
    // 25 nodes that link to it: the exact total pages to its end.
    for i in 0..25 {
        let c = k.call(
            Some(s1),
            "node",
            json!({"action": "create", "path": format!("/use-{i}"), "title": "a use",
                   "take": false}),
        );
        ok(&c);
        let n = k.node(&format!("/use-{i}"));
        k.events(|d| {
            d.push(Ev::LinkAdded {
                node: n,
                rel: LinkRel::DerivedFrom,
                to: lemma,
            });
        });
    }
    let src = StoreSource::new(&k.w.conn);
    let full = |page: u32| {
        let j = brief::view_node(
            &k.w.s,
            lemma,
            ViewMode::Full,
            page,
            Some(s1),
            k.w.clock,
            &src,
        );
        serde_json::from_str::<Value>(&j.get("rests_on").unwrap().text()).unwrap()
    };
    let p0 = full(0);
    assert_eq!(p0["cited_by"]["shown"], json!("0/≥0"), "{p0}");
    assert!(p0["cited_by"].get("more").is_none(), "an empty page: {p0}");
    assert_eq!(p0["links"]["shown"], json!("20/25"));
    assert_eq!(p0["links"]["more"], json!("page: 1 for more"));
    let p1 = full(1);
    assert_eq!(p1["links"]["shown"], json!("5/25"));
    assert!(p1["links"].get("more").is_none(), "{p1}");
    let last = full(u32::MAX);
    assert!(last["cited_by"].get("more").is_none(), "{last}");
    assert!(last["links"].get("more").is_none(), "{last}");
}

/// A question for the user with many long options keeps its `user_decisions` item bounded: at most
/// `USER_OPTIONS_MAX` options, each cut to its share, and their total (A-73; E3-bugs-10).
#[test]
fn a_question_with_many_options_stays_bounded() {
    let shape = HelloShape {
        budget: 4_096,
        notes: Vec::new(),
        offers: Vec::new(),
        others: 0,
        map: 0,
        questions: vec![2_000],
        digest: Vec::new(),
        options: 50,
    };
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
    let item = &v["user_decisions"]["items"][0];
    let options = item["options"].as_array().unwrap();
    assert_eq!(options.len(), hello::USER_OPTIONS_MAX);
    assert_eq!(item["options_total"], json!(50));
    let cap = 4_096 / 16;
    for o in options {
        assert!(o.as_str().unwrap().len() <= (cap / hello::USER_OPTIONS_MAX).max(16));
    }
    assert!(
        item.to_string().len() <= 3 * cap,
        "{} bytes",
        item.to_string().len()
    );
}

/// `view find {key}` lists a page of the closed carriers, newest first, with `shown/total` and how
/// to reach the older ones (§18; E3-bugs-11).
#[test]
fn find_by_key_pages_the_closed_carriers() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let key = keystr("issue:proj#7");
    let mut carriers = Vec::new();
    for i in 0..25 {
        let path = format!("/attempt-{i}");
        let c = k.call(
            Some(s1),
            "node",
            json!({"action": "create", "path": path, "title": "an attempt", "take": false}),
        );
        ok(&c);
        let n = k.node(&path);
        let key = key.clone();
        k.events(|d| {
            d.push(Ev::KeyAdded {
                node: n,
                key: Key {
                    key,
                    nv: NormVer(1),
                },
            });
        });
        let c = k.call(
            Some(s1),
            "node",
            json!({"action": "close", "path": path, "outcome": "failed", "summary": "no"}),
        );
        ok(&c);
        carriers.push(n);
    }
    let c = k.call(
        Some(s1),
        "view",
        json!({"action": "find", "key": "issue:proj#7"}),
    );
    let v = &c.response.structured;
    let closed = v["closed"].as_array().unwrap();
    assert_eq!(closed.len(), lists::FIND_MAX);
    assert_eq!(v["shown"], json!("20/25"));
    assert_eq!(
        closed[0]["id"],
        json!(carriers[24].render()),
        "newest first"
    );
    assert!(v["more"].as_str().unwrap().contains("continues"));
}

/// The close of a node is found however many system entries followed it, by both sources, and
/// shows in the review view (E3-bugs-12).
#[test]
fn a_close_is_found_after_many_system_entries() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/old", "title": "old", "take": false}),
    );
    ok(&c);
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "close", "path": "/old", "outcome": "failed",
               "summary": "the approach did not hold"}),
    );
    ok(&c);
    let old = k.node("/old");
    k.events(|d| {
        for _ in 0..70 {
            append(
                d,
                old,
                s1,
                EntryBody::System(SystemNote::Unpaused),
                text("", &[]),
            );
        }
    });
    let store = StoreSource::new(&k.w.conn);
    let events: Vec<Event> = crate::store::history::events_after(&k.w.conn, 0, u32::MAX)
        .unwrap()
        .into_iter()
        .map(|x| x.event)
        .collect();
    let mem = MemorySource::of(&events);
    let found = store.close_entry(old).expect("the close");
    assert_eq!(found.text.main, t("the approach did not hold"));
    assert_eq!(mem.close_entry(old), Some(found));
    let c = k.call(
        Some(s1),
        "view",
        json!({"action": "node", "path": "/old", "view": "review"}),
    );
    assert!(
        c.response
            .text
            .contains(r#""summary":"the approach did not hold""#),
        "{}",
        c.response.text
    );
}

/// Each row of the request board carries the detail of its state (§8.3, D.8.8; E3-spec-10).
#[test]
fn the_request_board_shows_each_states_detail() {
    let mut k = Kit::new();
    let (s1, _) = k.hello(
        "coder",
        json!({"action": "hello", "principal": "mail:alice@example.org"}),
    );
    let alice = k.w.s.sessions[&s1]
        .principals
        .iter()
        .copied()
        .next()
        .unwrap();
    let mk = |k: &mut Kit, path: &str, take: bool| {
        let c = k.call(
            Some(s1),
            "node",
            json!({"action": "create", "path": path, "title": path, "take": take,
                   "keys": [format!("chat:{}/1", path.trim_start_matches('/'))]}),
        );
        match &ok(&c).reply {
            Reply::Created { token, .. } => (k.node(path), *token),
            other => panic!("{other:?}"),
        }
    };
    let (held, held_token) = mk(&mut k, "/held", true);
    let (waits, waits_token) = mk(&mut k, "/waits", true);
    let (decided, _) = mk(&mut k, "/decided", false);
    let (closed, _) = mk(&mut k, "/closed", false);
    let (answered, _) = mk(&mut k, "/answered", false);
    let mut r = BTreeMap::new();
    k.events(|d| {
        for (name, n) in [
            ("held", held),
            ("waits", waits),
            ("decided", decided),
            ("closed", closed),
            ("answered", answered),
        ] {
            let conv = format!("chat:{name}/1");
            let id = request_event(
                d,
                n,
                s1,
                &format!("{conv}/m1"),
                &conv,
                &format!("please look at {name}"),
                "Bob",
            );
            r.insert(name, id);
        }
        append(
            d,
            decided,
            s1,
            EntryBody::Decision {
                settles: [r["decided"]].into(),
                supersedes: None,
                assumes: BTreeSet::new(),
                evidence: None,
                basis: Vec::new(),
            },
            text("ship the fix on Friday", &[]),
        );
        let said = append(
            d,
            answered,
            s1,
            EntryBody::Said {
                conv: keystr("chat:answered/1"),
                kind: SaidKind::Reply,
                corrects: None,
                voice: false,
            },
            text("done, see the thread", &[]),
        );
        d.push(Ev::RequestAnswered {
            request: r["answered"],
            by: said,
        });
        r.insert("said", said);
        r.insert(
            "unrouted",
            append(
                d,
                USER,
                s1,
                EntryBody::FromUser { principal: alice },
                text("and the weekly totals?", &[(TextField::Client, "ide")]),
            ),
        );
    });
    k.at(120_000);
    let c = k.call(
        Some(s1),
        "hold",
        json!({"action": "checkpoint", "token": held_token.unwrap().render(),
               "done": "found it", "next_step": "fix it"}),
    );
    ok(&c);
    let last = c.response.structured["entry"].as_str().unwrap().to_string();
    let c = k.call(
        Some(s1),
        "hold",
        json!({"action": "release", "token": waits_token.unwrap().render(), "done": "the fix",
               "next_step": "wait for the merge", "reason": "blocked",
               "waits_for": {"external": "Bob's merge of the fix",
                             "recheck_after": "2026-09-30T00:00:00Z"}}),
    );
    ok(&c);
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "close", "path": "/closed", "outcome": "dropped",
               "summary": "not ours"}),
    );
    ok(&c);
    k.at(300_000);
    let c = k.call(
        Some(s1),
        "view",
        json!({"action": "requests", "requester": "all"}),
    );
    let rows = c.response.structured["rows"].as_array().unwrap().clone();
    let row = |state: &str| {
        rows.iter()
            .find(|x| x["state"] == json!(state))
            .unwrap_or_else(|| panic!("no {state} row: {rows:?}"))
            .clone()
    };
    let unrouted = row("unrouted");
    assert_eq!(unrouted["entry"], json!(r["unrouted"].render()));
    assert_eq!(unrouted["waited"], json!("7m"), "{unrouted}");
    let x = row("answered");
    assert_eq!(x["answered"]["entry"], json!(r["said"].render()));
    assert_eq!(x["answered"]["by"], json!(s1.render()));
    assert!(x["answered"]["at"].is_string());
    assert_eq!(
        row("closed_unanswered")["flag"],
        json!("unanswered_request")
    );
    let x = row("decided");
    assert!(x["decision"].is_string(), "{x}");
    assert_eq!(x["outcome"], json!("ship the fix on Friday"));
    let x = row("waiting");
    assert_eq!(
        x["waits_for"],
        json!({"recheck_after": "2026-09-30T00:00:00Z", "external": "Bob's merge of the fix"}),
        "{x}"
    );
    let x = row("in_progress");
    assert_eq!(x["holders"][0]["session"], json!(s1.render()), "{x}");
    assert_eq!(x["last_checkpoint"]["entry"], json!(last));
}

/// An entry left unrouted says why: the call named no target, and the session holds two nodes
/// (§8.2; E3-spec-6).
#[test]
fn an_unrouted_entry_says_why() {
    let mut k = Kit::new();
    let (s1, _) = k.hello(
        "coder",
        json!({"action": "hello", "principal": "mail:alice@example.org"}),
    );
    for p in ["/a", "/b"] {
        let c = k.call(
            Some(s1),
            "node",
            json!({"action": "create", "path": p, "title": p}),
        );
        ok(&c);
    }
    let alice = k.w.s.sessions[&s1]
        .principals
        .iter()
        .copied()
        .next()
        .unwrap();
    let mut e = Eid(0);
    k.events(|d| {
        e = append(
            d,
            USER,
            s1,
            EntryBody::FromUser { principal: alice },
            text("also the totals", &[(TextField::Client, "ide")]),
        );
    });
    let cmd = Cmd::Talk(TalkCmd::FromUser {
        text: t("also the totals"),
        client: t("ide"),
        principal: None,
        answers: None,
        to: None,
        clarifies: None,
        reason: None,
    });
    let r = k.render(
        Some(s1),
        &cmd,
        &done(Reply::FromUser {
            entry: e,
            key: KeyStr::of_user_entry(e),
            routed: RoutedReply::Unrouted(UnroutedWhy::NoTarget { holds: 2 }),
            principal: alice,
        }),
    );
    assert!(
        r.text
            .contains(r#""routed":{"unrouted":"no target, and you hold 2 nodes"}"#),
        "{}",
        r.text
    );
}

// ------------------------------------------------------------------------------------------------
// 1.4 renderings that wait for the core (E3-spec-12, E3-spec-13)

/// An empty `next` counts the nodes that wait, with D.11.7's sentence, and `hello`'s empty offers
/// say the same (§10.4, R27).
#[test]
fn an_empty_next_counts_the_nodes_that_wait() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/work", "title": "work", "tags": ["offer"]}),
    );
    let token = match &ok(&c).reply {
        Reply::Created { token: Some(t), .. } => *t,
        other => panic!("{other:?}"),
    };
    let c = k.call(
        Some(s1),
        "hold",
        json!({"action": "release", "token": token.render(), "done": "asked", "next_step": "wait",
               "reason": "blocked", "waits_for": {"time": "2026-12-01T00:00:00Z"}}),
    );
    ok(&c);
    let sentence = "1 nodes wait (for a question, a node, a time, or something outside) and come \
                    back when their waits resolve";
    let (s2, c) = k.hello("reviewer", json!({"action": "hello"}));
    assert!(c.response.text.contains(sentence), "{}", c.response.text);
    let c = k.call_as(
        "reviewer",
        Some(s2),
        "hold",
        json!({"action": "next"}),
        None,
    );
    assert!(
        c.response.text.contains(r#""waiting":1"#),
        "{}",
        c.response.text
    );
    assert!(c.response.text.contains(sentence), "{}", c.response.text);
}

/// A `said` says which requests it answered and which commitments it fulfilled (§20.7, R9).
#[test]
fn a_said_says_what_it_answered_and_fulfilled() {
    let mut k = Kit::new();
    let p = people_forum(&mut k);
    let c = k.call(
        Some(p.worker),
        "talk",
        json!({"action": "said", "at": "/support/summary-gap", "where": "chat:team/7",
               "kind": "result", "text": "the summary is complete again"}),
    );
    let v = &c.response.structured;
    assert!(v["answered"].is_array(), "{v}");
    assert!(v["fulfilled"].is_array(), "{v}");
}

/// 1.10 (§9.2, §20.7): a recorded `said` says when it counts as sent, "post it in this reply; your
/// release or close marks it sent", and so does one by a session that holds neither the
/// conversation node nor work inside it, which 1.10 records as any `said` (with
/// `conversation_held`), where 1.9 withheld it as a draft for the holder. In the lab's fifth round
/// (it4 l1), an agent listed in `unsent` a post it sent in the reply after its release, which made
/// the post unconfirmed.
#[test]
fn a_said_says_when_it_counts_as_sent() {
    let mut k = Kit::new();
    let p = people_forum(&mut k);
    let c = k.call(
        Some(p.worker),
        "talk",
        json!({"action": "said", "at": "/support/summary-gap", "where": "chat:team/7",
               "kind": "result", "text": "the summary is complete again"}),
    );
    assert_eq!(
        c.response.structured["send"],
        json!("post it in this reply; your release or close marks it sent"),
        "{}",
        c.response.text
    );
    let c = k.call(
        Some(p.assistant),
        "talk",
        json!({"action": "said", "at": "/support/summary-gap", "where": "chat:team/7",
               "kind": "reply", "text": "I can look too"}),
    );
    assert!(
        c.response.text.contains("conversation_held"),
        "{}",
        c.response.text
    );
    // 1.11 (§9.2): the assistant holds nothing, so no release or close of its will come: its
    // `said` counts as sent at once, and the response says so.
    assert_eq!(
        c.response.structured["send"],
        json!("post it in this reply; you hold nothing, so it counts as sent now"),
        "{}",
        c.response.text
    );
}

/// 1.10 (§20.5, §21): `unmet_needs` names the caps in use, those of the sessions seen within
/// `capacity.window`, ascending. In the lab's fifth round (it4 l2), "none has git" left a mail
/// assistant unsure which label a worker would match, while the one present called it `repo`.
#[test]
fn unmet_needs_names_the_caps_in_use() {
    let mut k = Kit::new();
    let (s1, _) = k.hello(
        "coder",
        json!({"action": "hello", "caps": ["swift", "repo"]}),
    );
    let (_, _) = k.hello("coder", json!({"action": "hello", "caps": ["shell"]}));
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/rss", "title": "empty dates", "needs": ["git"],
               "tags": ["offer"], "take": false}),
    );
    let text = all_of(&c.response);
    assert!(
        text.contains(
            "no session can take /rss now: none has git; the caps in use: repo, shell, swift; \
             needs name what a taker must be able to do"
        ),
        "{text}"
    );
}

/// A checkpoint names the intent it closed, whichever session opened it (§20.6, R9).
#[test]
fn a_checkpoint_names_the_intent_it_closed() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/work", "title": "work"}),
    );
    let token = match &ok(&c).reply {
        Reply::Created { token: Some(t), .. } => *t,
        other => panic!("{other:?}"),
    };
    let c = k.call(
        Some(s1),
        "hold",
        json!({"action": "checkpoint", "token": token.render(), "done": "a", "next_step": "b",
               "intent": "post it"}),
    );
    let intent = c.response.structured["intent"].clone();
    let c = k.call(
        Some(s1),
        "hold",
        json!({"action": "checkpoint", "token": token.render(), "done": "posted",
               "next_step": "c", "intent_done": "posted it"}),
    );
    assert_eq!(c.response.structured["intent_done"], intent);
}

/// A release closes an intent with `intent_done`, and one that leaves its own intent open warns
/// `open_intent` (§10.3, §20.6, §21, R15).
#[test]
fn a_release_closes_an_intent_or_warns_that_it_is_open() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/work", "title": "work"}),
    );
    let token = match &ok(&c).reply {
        Reply::Created { token: Some(t), .. } => *t,
        other => panic!("{other:?}"),
    };
    let c = k.call(
        Some(s1),
        "hold",
        json!({"action": "checkpoint", "token": token.render(), "done": "a", "next_step": "b",
               "intent": "post it"}),
    );
    let intent = c.response.structured["intent"]
        .as_str()
        .unwrap()
        .to_string();
    let c = k.call(
        Some(s1),
        "hold",
        json!({"action": "release", "token": token.render(), "done": "a", "next_step": "b",
               "reason": "handoff"}),
    );
    assert!(
        c.response.text.contains(&format!(
            "your intent {intent} is still open on /work: if the action happened, say so with \
             intent_done; otherwise the next holder must check whether it did"
        )),
        "{}",
        c.response.text
    );
}

/// A decision says, for each request it lists and did not settle, why (§20.7, R9).
#[test]
fn a_decision_says_why_a_listed_request_is_not_settled() {
    let mut k = Kit::new();
    let p = people_forum(&mut k);
    // The fixture's request is answered and listed by a decision built from events alone, which
    // do not settle it; the first decision that lists it settles it, and the next one reports it
    // settled before.
    let decide = json!({"action": "decide", "at": "/support/summary-gap", "text": "done",
                        "settles": [p.request.render()]});
    let c = k.call(Some(p.worker), "talk", decide.clone());
    assert_eq!(
        c.response.structured["settled"],
        json!([p.request.render()]),
        "{}",
        c.response.text
    );
    let c = k.call(Some(p.worker), "talk", decide);
    let v = &c.response.structured;
    assert_eq!(v["not_settled"][0]["request"], json!(p.request.render()));
    assert_eq!(v["not_settled"][0]["why"], json!("settled before"));
}

/// Closing a series names the occurrence it dropped and warns `series_stopped` (§12, §20.5, §21,
/// R2).
#[test]
fn closing_a_series_names_the_dropped_occurrence() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/daily", "title": "the daily summary",
               "repeat": {"cron": "0 7 * * *", "tz": "UTC", "run_within": "2h"}, "take": false}),
    );
    ok(&c);
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "close", "path": "/daily", "outcome": "done", "summary": "set up"}),
    );
    assert!(
        c.response.structured["dropped"].is_string(),
        "{}",
        c.response.text
    );
    assert!(
        c.response
            .text
            .contains("closing the series dropped its occurrence"),
        "{}",
        c.response.text
    );
    assert!(
        c.response
            .text
            .contains("no further occurrence opens until the series is reopened"),
        "{}",
        c.response.text
    );
}

/// `say {expects}` warns `expects_ignored` (§13, §21, R17).
#[test]
fn say_with_expects_warns_that_it_is_ignored() {
    let mut k = Kit::new();
    let p = people_forum(&mut k);
    let c = k.call(
        Some(p.worker),
        "talk",
        json!({"action": "say", "at": "/support/summary-gap",
               "to": format!("session:{}", p.assistant.render()), "text": "done?",
               "expects": "reply"}),
    );
    assert!(
        c.response.text.contains(
            "expects is a later release: nothing waits for the answer; to get one from an agent, \
             use talk ask {to}"
        ),
        "{}",
        c.response.text
    );
}

/// A basis naming the decision's own node, or a decision it supersedes, is dropped with a warning
/// (§15, §21, R14, R33).
#[test]
fn a_basis_naming_its_own_node_is_dropped_with_a_warning() {
    let mut k = Kit::new();
    let p = people_forum(&mut k);
    let c = k.call(
        Some(p.worker),
        "talk",
        json!({"action": "decide", "at": "/support/summary-gap", "text": "go on",
               "basis": [format!("{}@1", p.work.render())]}),
    );
    assert!(
        c.response.text.contains(
            "1 basis reference(s) named this decision's own node and were dropped; the decision \
             was recorded with the others"
        ),
        "{}",
        c.response.text
    );
    let c = k.call(
        Some(p.worker),
        "talk",
        json!({"action": "decide", "at": "/support/summary-gap", "text": "go on, revised",
               "supersedes": p.decision.render(), "basis": [p.decision.render()]}),
    );
    assert!(
        c.response.text.contains("superseded_basis_dropped"),
        "{}",
        c.response.text
    );
}

/// A `to_user` of kind `result` that answered no request warns `result_answered_nothing` (§21).
#[test]
fn a_result_that_answered_nothing_warns() {
    let mut k = Kit::new();
    let p = people_forum(&mut k);
    let c = k.call(
        Some(p.assistant),
        "talk",
        json!({"action": "to_user", "text": "it is done", "kind": "result", "verbatim": true}),
    );
    assert!(
        c.response.text.contains("result_answered_nothing"),
        "{}",
        c.response.text
    );
}

/// `not_found` for an addressee keeps the status line, names `to`, and says the caller's own
/// session is unaffected, with no remedy (§21, R12, D.11.7).
#[test]
fn not_found_for_an_addressee_keeps_the_status_line_and_names_to() {
    let mut k = Kit::new();
    let p = people_forum(&mut k);
    let c = k.call(
        Some(p.worker),
        "talk",
        json!({"action": "say", "at": "/support/summary-gap", "to": "session:s-99",
               "text": "are you there?"}),
    );
    assert!(c.response.is_error);
    assert!(
        c.response.text.starts_with("[swarm] "),
        "{}",
        c.response.text
    );
    assert!(
        c.response.text.contains(
            "to names an unknown or retired session (s-99); your own session is unaffected"
        ),
        "{}",
        c.response.text
    );
    assert!(c.response.text.contains("next_calls: none"));
}

/// `uncovered_slot` names the first likely start inside the next slot's window, or that none is
/// (§12, §21, R18, D.11.7).
#[test]
fn uncovered_slot_names_the_first_likely_start() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/nightly", "title": "the nightly run",
               "repeat": {"cron": "0 3 * * *", "tz": "UTC", "run_within": "1h"}, "take": false}),
    );
    assert!(
        c.response.text.contains("first likely start")
            || c.response.text.contains(
                "no usual start falls within the window: without an outside trigger, the run \
                 will likely be missed"
            ),
        "{}",
        c.response.text
    );
}

/// `view agreements` lists the agreements of a subtree with their source paths, approvals, and
/// last change (§20.4, D.14.5).
#[test]
fn view_agreements_lists_a_subtrees_agreements() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/playbooks", "title": "playbooks", "take": false,
               "agreements": "review every merge"}),
    );
    ok(&c);
    let c = k.call(Some(s1), "view", json!({"action": "agreements"}));
    let v = &c.response.structured;
    assert_eq!(v["items"][0]["path"], json!("/playbooks"), "{v}");
    assert_eq!(v["items"][0]["text"], json!("review every merge"));
    // 1.7 (§20.1): no approvals is left out, and a whole list carries no counts.
    assert!(v["items"][0].get("approvals").is_none(), "{v}");
    assert!(v["items"][0]["changed"].is_string());
    assert!(v.get("total").is_none(), "{v}");
}

// ------------------------------------------------------------------------------------------------
// 1.6: the lab's first round (the core's changes)

/// 1.6 (§8.2): a request routed from a user's message carries the message's words, in the brief
/// and on the request board, and hides them with the message (§16). In the lab's first round,
/// every such request showed `text: ""`, so a stranger saw who asked but not what.
#[test]
fn a_routed_request_shows_its_messages_words() {
    let mut k = Kit::new();
    let (s1, _) = k.hello(
        "coder",
        json!({"action": "hello", "principal": "mail:alice@example.org"}),
    );
    ok(&k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/work", "title": "work", "take": false}),
    ));
    let c = k.call(
        Some(s1),
        "talk",
        json!({"action": "from_user", "text": "add the weekly totals", "client": "ide",
               "node": "/work"}),
    );
    let (e, r) = match &ok(&c).reply {
        Reply::FromUser {
            entry,
            routed: RoutedReply::Request { request, .. },
            ..
        } => (*entry, *request),
        other => panic!("{other:?}"),
    };
    let request_text = |k: &Kit| {
        let src = StoreSource::new(&k.w.conn);
        let (j, _) = brief::render_brief(&k.w.s, k.node("/work"), Some(s1), k.w.clock, &src);
        let v: Value = serde_json::from_str(&j.text()).unwrap();
        let item = section_named(&v, "unsettled_requests")["items"][0].clone();
        assert_eq!(item["e"], json!(r.render()));
        assert_eq!(item["origin"], json!(format!("user:{}", e.render())));
        item["text"].clone()
    };
    assert_eq!(request_text(&k), json!("add the weekly totals"));
    let c = k.call(
        Some(s1),
        "view",
        json!({"action": "requests", "requester": "all"}),
    );
    let rows = c.response.structured["rows"].as_array().unwrap().clone();
    let row = rows
        .iter()
        .find(|x| x["request"] == json!(r.render()))
        .unwrap();
    assert_eq!(row["text"], json!("add the weekly totals"), "{row}");
    ok(&k.call(
        Some(s1),
        "node",
        json!({"action": "hide", "entry": e.render(), "reason": "private"}),
    ));
    assert_eq!(request_text(&k), json!("[hidden: private]"));
}

/// 1.6 (§10.1, §21): a take of a node that waits renders the `waiting` warning with what it waits
/// for; an empty `next` renders the first node of each exclusion (§10.4); a `from_user` for no
/// named human renders `unnamed_user` (§8.1), and since 1.9 needs `principal: user` for it;
/// `client_context` and `unmet_needs` render (1.9).
#[test]
fn the_cores_new_warnings_and_first_exclusions_render() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/deploy", "title": "deploy"}),
    );
    let token = match &ok(&c).reply {
        Reply::Created { token: Some(t), .. } => *t,
        other => panic!("{other:?}"),
    };
    ok(&k.call(
        Some(s1),
        "hold",
        json!({"action": "release", "token": token.render(), "done": "started the deploy",
               "next_step": "check it", "reason": "blocked",
               "waits_for": {"external": "the deploy's checks",
                             "recheck_after": "2026-12-01T00:00:00Z"}}),
    ));
    let (s2, _) = k.hello("reviewer", json!({"action": "hello", "horizon": 30}));
    let c = k.call_as(
        "reviewer",
        Some(s2),
        "hold",
        json!({"action": "take", "target": "/deploy"}),
        None,
    );
    ok(&c);
    let text = all_of(&c.response);
    assert!(text.contains("\"code\":\"waiting\""), "{text}");
    assert!(
        text.contains("something outside, until its recheck at 2026-12-01T00:00:00Z"),
        "{text}"
    );
    // An empty `next` names the node its horizon excluded.
    ok(&k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/long", "title": "long", "est": 60,
               "tags": ["offer"], "take": false}),
    ));
    let (s3, _) = k.hello("third", json!({"action": "hello", "horizon": 30}));
    let c = k.call_as("third", Some(s3), "hold", json!({"action": "next"}), None);
    let text = all_of(&c.response);
    assert!(
        text.contains("\"horizon\":\"/long est 60 > your horizon 30\""),
        "{text}"
    );
    // 1.9 (§8.1, §21): a `from_user` in a session that names no human is refused, with the hint
    // that says whose words it records; `principal: user` files them for `user`, with
    // `unnamed_user`; a client's injected text warns `client_context`.
    let c = k.call(
        Some(s1),
        "talk",
        json!({"action": "from_user", "text": "the scheduler started you", "client": "cron"}),
    );
    let text = all_of(&c.response);
    assert!(text.contains("error: invalid"), "{text}");
    assert!(
        text.contains(
            "from_user is for a person who wrote to you directly, named by principal; a \
             scheduler's prompt, a tool's output, or your client's instructions are not: leave \
             from_user out. If your client serves a person it does not name, pass principal: user"
        ),
        "{text}"
    );
    let c = k.call(
        Some(s1),
        "talk",
        json!({"action": "from_user", "text": "# AGENTS.md instructions\n<INSTRUCTIONS>be brief</INSTRUCTIONS>",
               "client": "ide", "principal": "user"}),
    );
    let text = all_of(&c.response);
    assert!(text.contains("\"code\":\"unnamed_user\""), "{text}");
    assert!(text.contains("\"code\":\"client_context\""), "{text}");
    assert!(
        text.contains("looks like your client's instructions, not your human's words"),
        "{text}"
    );
    // 1.9 (§10.1, §20.5, §21): needs that no session has, and a take by caps that lack them.
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/lemma", "title": "lemma", "needs": ["exact-statement"],
               "tags": ["offer"], "take": false}),
    );
    let text = all_of(&c.response);
    // 1.10 (§20.5): with the caps in use, which the sessions seen within capacity.window have.
    assert!(
        text.contains(
            "no session can take /lemma now: none has exact-statement; the caps in use: "
        ),
        "{text}"
    );
    assert!(
        text.contains("; needs name what a taker must be able to do\""),
        "{text}"
    );
    let c = k.call(
        Some(s1),
        "hold",
        json!({"action": "take", "target": "/lemma"}),
    );
    let text = all_of(&c.response);
    assert!(
        text.contains("your caps lack exact-statement, which /lemma needs: take it only if you can do the work"),
        "{text}"
    );
}

/// 1.6 (§10.4, §20.4): a brief leaves out its empty sections, and the full view's feed page leaves
/// out the entries the brief shows, naming them in `in_brief`. In the lab, full views of 13 to 24
/// KB were about half empty sections and repeated entries.
#[test]
fn a_full_view_repeats_nothing_its_brief_shows() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/work", "title": "work"}),
    );
    let token = match &ok(&c).reply {
        Reply::Created { token: Some(t), .. } => *t,
        other => panic!("{other:?}"),
    };
    let c = k.call(
        Some(s1),
        "hold",
        json!({"action": "checkpoint", "token": token.render(), "done": "read the logs",
               "next_step": "fix it"}),
    );
    let last = ok(&c);
    let Reply::Checkpointed { entry, .. } = last.reply else {
        panic!("{:?}", last.reply)
    };
    let c = k.call(
        Some(s1),
        "view",
        json!({"action": "node", "path": "/work", "view": "full"}),
    );
    let v = &c.response.structured;
    let brief = if v["brief"].is_object() {
        &v["brief"]
    } else {
        &v["nodes"][0]["brief"]
    };
    let names: Vec<&str> = brief["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"last_checkpoint"), "{names:?}");
    assert!(!names.contains(&"objections"), "{names:?}");
    let feed = if v["feed"].is_object() {
        &v["feed"]
    } else {
        &v["nodes"][0]["feed"]
    };
    assert!(
        feed["in_brief"]
            .as_array()
            .unwrap()
            .contains(&json!(entry.render())),
        "{feed}"
    );
    assert!(
        !feed["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|x| x["e"] == json!(entry.render())),
        "{feed}"
    );
}

// ------------------------------------------------------------------------------------------------
// 1.7: the lab's second round (responses)

/// The first line of a response.
fn status_of(c: &Called) -> String {
    c.response.text.lines().next().unwrap().to_string()
}

/// 1.7 (§17, §20.1): the status line names the session as the `session` argument takes it, leaves
/// a green health out, and shows a reason only when the session has not been shown it at that
/// level: at its first response, when the level changes, and when the reason's kind or first item
/// does; a count that moves does not. In the lab's second round, one standing red reason came on
/// all 61 responses of a run.
#[test]
fn the_status_line_shows_a_health_reason_when_the_session_has_not_seen_it() {
    use forum_core::health::UnreachableBy;
    let mut k = Kit::new();
    let (s1, first) = k.hello("coder", json!({"action": "hello"}));
    assert_eq!(
        status_of(&first),
        "[swarm] running · now 2026-09-21T14:13:00Z · session s-1 (coder) · holds 0 · inbox 0"
    );
    ok(&k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/a", "title": "a", "needs": ["sql"], "take": false}),
    ));
    let a = k.node("/a");
    let unreachable = |level: Level, count: u32| Health {
        level,
        reasons: vec![HealthReason::Unreachable {
            level,
            count,
            by_needs: count,
            by_horizon: 0,
            top: vec![(a, UnreachableBy::Needs)],
        }],
        capacity: forum_core::health::CapacityReport::default(),
    };
    let piling = Health {
        level: Level::Red,
        reasons: vec![HealthReason::FlagsPilingUp { start: 40, now: 95 }],
        capacity: forum_core::health::CapacityReport::default(),
    };
    let look =
        |k: &mut Kit, s: SessionId| status_of(&k.call(Some(s), "view", json!({"action": "list"})));
    k.health = unreachable(Level::Yellow, 1);
    assert!(
        look(&mut k, s1).ends_with(
            " · inbox 0 · health yellow: unreachable: /a needs caps no session has (sql)"
        )
    );
    assert!(look(&mut k, s1).ends_with(" · inbox 0 · health yellow"));
    // A count that moves is the same reason.
    k.health = unreachable(Level::Yellow, 3);
    assert!(look(&mut k, s1).ends_with(" · health yellow"));
    // The level rises: the reason again, with its count.
    k.health = unreachable(Level::Red, 3);
    assert!(
        look(&mut k, s1).ends_with(
            " · health red: unreachable: /a needs caps no session has (sql), and 2 more"
        )
    );
    assert!(look(&mut k, s1).ends_with(" · health red"));
    // Another reason first: shown, and to another session at its first response.
    k.health = piling.clone();
    assert!(look(&mut k, s1).contains(" · health red: flags piling up: 95 flagged nodes"));
    assert!(look(&mut k, s1).ends_with(" · health red"));
    let (_, hello2) = k.hello("coder", json!({"action": "hello"}));
    assert!(status_of(&hello2).contains(" · session s-2 (coder) · "));
    assert!(status_of(&hello2).contains(" · health red: flags piling up: 95 flagged nodes"));
    // Green is left out; the same reason after it is shown again, since the level changed.
    k.health = green();
    assert!(!look(&mut k, s1).contains("health"));
    k.health = piling;
    assert!(look(&mut k, s1).contains(" · health red: flags piling up: 95 flagged nodes"));
    // Forgotten (the session's silence, a restart): shown again.
    k.seen.drop_session(s1);
    assert!(look(&mut k, s1).contains(" · health red: flags piling up: 95 flagged nodes"));
}

/// The `agreement` warning of a response, if it has one.
fn agreement_warning(c: &Called) -> Option<String> {
    c.response.structured["warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|w| w["code"] == json!("agreement"))
        .map(|w| w["detail"].as_str().unwrap().to_string())
}

/// 1.7 (§11.2, §21): the `agreement` warning names only the agreements set below `/`, carries each
/// text the first time a warning shows it to the session, and names the source alone after that,
/// until the text changes. In the lab's second round it fired 14 times, without its text, and on
/// the house rules that `hello` already shows.
#[test]
fn the_agreement_warning_carries_each_text_once_and_leaves_the_house_rules_to_hello() {
    let mut k = Kit::new();
    k.events(|d| {
        d.push(Ev::NodeUpdated {
            node: ROOT,
            patch: forum_core::NodePatch {
                agreements: Some(t("ask the user before sending")),
                ..forum_core::NodePatch::default()
            },
        });
    });
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    ok(&k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/team", "title": "team", "take": false,
               "agreements": "close after review"}),
    ));
    for p in ["/team/a", "/team/b", "/team/c", "/solo"] {
        ok(&k.call(
            Some(s1),
            "node",
            json!({"action": "create", "path": p, "title": "work", "take": false}),
        ));
    }
    let close = |k: &mut Kit, p: &str| {
        let c = k.call(
            Some(s1),
            "node",
            json!({"action": "close", "path": p, "outcome": "done", "summary": "done"}),
        );
        ok(&c);
        c
    };
    let w = agreement_warning(&close(&mut k, "/team/a")).unwrap();
    assert!(
        w.starts_with("agreements in force: /team: \"close after review\"; "),
        "{w}"
    );
    assert!(!w.contains("ask the user"), "{w}");
    let w = agreement_warning(&close(&mut k, "/team/b")).unwrap();
    assert!(
        w.starts_with("agreements in force: /team (shown before); "),
        "{w}"
    );
    // Under the house rules alone, no warning.
    assert_eq!(agreement_warning(&close(&mut k, "/solo")), None);
    // A changed text is shown again.
    let version = k.w.s.nodes[&k.node("/team")].version.0;
    ok(&k.call(
        Some(s1),
        "node",
        json!({"action": "update", "path": "/team", "version": version,
               "agreements": "close after review by another class"}),
    ));
    let w = agreement_warning(&close(&mut k, "/team/c")).unwrap();
    assert!(
        w.starts_with("agreements in force: /team: \"close after review by another class\"; "),
        "{w}"
    );
}

/// 1.7 (§10.4, §20.3): a brief leaves the house rules to `hello`, except the brief of `/`, and a
/// read of several nodes prints each agreement once, naming it by its source after that. In the
/// lab's second round, a read of four nodes printed two agreements four times each, and the house
/// rules came 27 times in briefs.
#[test]
fn briefs_print_each_agreement_once_and_leave_the_house_rules_to_hello() {
    let mut k = Kit::new();
    k.events(|d| {
        d.push(Ev::NodeUpdated {
            node: ROOT,
            patch: forum_core::NodePatch {
                agreements: Some(t("ask the user before sending")),
                ..forum_core::NodePatch::default()
            },
        });
    });
    let (s1, hello) = k.hello("coder", json!({"action": "hello"}));
    assert_eq!(
        hello.response.structured["orientation"]["house_rules"],
        json!("ask the user before sending")
    );
    ok(&k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/team", "title": "team", "take": false,
               "agreements": "close after review"}),
    ));
    for p in ["/team/a", "/team/b"] {
        ok(&k.call(
            Some(s1),
            "node",
            json!({"action": "create", "path": p, "title": "work", "take": false}),
        ));
    }
    let agreements = |b: &Value| section_named(b, "decisions_and_agreements")["items"].clone();
    for view in ["brief", "full"] {
        let c = k.call(
            Some(s1),
            "view",
            json!({"action": "node", "paths": ["/team/a", "/team/b"], "view": view}),
        );
        let v = &c.response.structured;
        let brief = |i: usize| {
            if view == "brief" {
                v["nodes"][i].clone()
            } else {
                v["nodes"][i]["brief"].clone()
            }
        };
        assert_eq!(
            agreements(&brief(0)),
            json!([{"agreements": "close after review", "source": "/team"}]),
            "{v}"
        );
        assert_eq!(
            agreements(&brief(1)),
            json!([{"agreements": "as above", "source": "/team"}]),
            "{v}"
        );
        assert!(!v.to_string().contains("ask the user"), "{v}");
    }
    // Read alone, each node's brief prints the text; the brief of / shows its own agreements.
    let c = k.call(
        Some(s1),
        "view",
        json!({"action": "node", "path": "/team/b"}),
    );
    assert_eq!(
        agreements(&c.response.structured),
        json!([{"agreements": "close after review", "source": "/team"}])
    );
    let c = k.call(Some(s1), "view", json!({"action": "node", "path": "/"}));
    assert_eq!(
        agreements(&c.response.structured),
        json!([{"agreements": "ask the user before sending"}])
    );
}

/// 1.7 (§20.3): `present` counts the live sessions by class and lists the other sessions of the
/// caller's class, newest first, each with what it holds; in the lab's second round it was 6.0 KB
/// of 62.9 KB of `hello`s.
#[test]
fn present_counts_classes_and_lists_the_callers_own() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    ok(&k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/work", "title": "work"}),
    ));
    k.hello("coder", json!({"action": "hello"}));
    k.hello("reviewer", json!({"action": "hello"}));
    let (_, c) = k.hello("coder", json!({"action": "hello"}));
    let present = &c.response.structured["present"];
    assert_eq!(present["total"], json!(4), "{present}");
    assert_eq!(present["by_class"], json!({"coder": 3, "reviewer": 1}));
    let sessions = present["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2, "{present}");
    assert_eq!(sessions[0]["session"], json!("s-2"));
    assert!(
        sessions[0].get("holds").is_none(),
        "nothing held: {present}"
    );
    assert_eq!(sessions[1]["session"], json!("s-1"));
    assert_eq!(sessions[1]["holds"], json!(["/work"]));
    assert!(sessions.iter().all(|x| x.get("class").is_none()));
    assert!(present.get("shown").is_none(), "shown whole: {present}");
}

/// 1.7 (§20.4): a row of `view list` carries its node's needs and holders when it has them, and
/// its rollup only when a count is not 0; the subtree's counts are those that are not 0. In the
/// lab's second round, a list of 9.5 KB was a third rollups of zeros.
#[test]
fn a_list_row_carries_needs_and_holders_and_no_zeros() {
    let mut k = Kit::new();
    let (s1, _) = k.hello("coder", json!({"action": "hello"}));
    ok(&k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/x", "title": "x", "needs": ["sql"]}),
    ));
    ok(&k.call(
        Some(s1),
        "node",
        json!({"action": "create", "path": "/x/y", "title": "y", "take": false}),
    ));
    let c = k.call(Some(s1), "view", json!({"action": "list", "under": "/x"}));
    let v = &c.response.structured;
    let row = |p: &str| {
        v["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["path"] == json!(p))
            .unwrap()
            .clone()
    };
    let x = row("/x");
    assert_eq!(x["needs"], json!(["sql"]), "{v}");
    assert_eq!(x["holders"][0]["session"], json!("s-1"), "{v}");
    assert_eq!(x["rollup"], json!("children 1 open; below: 1 open"), "{v}");
    // A child takes its parent's needs when it is made (§5).
    let y = row("/x/y");
    assert_eq!(y["needs"], json!(["sql"]), "{v}");
    assert!(
        y.get("holders").is_none() && y.get("rollup").is_none(),
        "{v}"
    );
    assert!(v.get("shown").is_none(), "shown whole: {v}");
    assert_eq!(v["under"]["below"], json!("1 open"), "{v}");
}

/// 1.11 (§9.1, §9.2, §11.3, §13, §20.7), the shell's side of the lab's sixth round: a deferral is
/// decoded, refused with a verdict, answered with its backoff, and shown with its objection in the
/// brief; the chat worker's `said` in a thread whose holder has no `chat` draws no
/// `conversation_held`, is shown as the conversation's voice, and counts as sent now, since it
/// holds nothing; a note at a closed node reaches the session that closed it.
#[test]
fn deferrals_the_voice_and_notes_at_closed_nodes_render() {
    let mut k = Kit::new();
    let (lead, _) = k.hello("coder", json!({"action": "hello", "caps": ["k8s", "repo"]}));
    let (chat, _) = k.hello("coder", json!({"action": "hello", "caps": ["chat"]}));
    // A claim, closed as done, with an objection.
    let c = k.call(
        Some(lead),
        "node",
        json!({"action": "create", "path": "/claims/lemma-04", "title": "lemma 4",
               "kind": "claim"}),
    );
    let t = ok(&c).reply.clone();
    let Reply::Created { token: Some(t), .. } = t else {
        panic!("{t:?}")
    };
    let c = k.call(
        Some(lead),
        "node",
        json!({"action": "close", "path": "/claims/lemma-04", "outcome": "done",
               "summary": "checked", "token": t.render()}),
    );
    ok(&c);
    let c = k.call(
        Some(lead),
        "talk",
        json!({"action": "object", "on": "/claims/lemma-04", "text": "nothing certifies it"}),
    );
    let Reply::Objected { objection } = ok(&c).reply else {
        panic!()
    };
    // A deferral is answered with its backoff (the decoder's tests refuse one with a verdict).
    let c = k.call(
        Some(chat),
        "talk",
        json!({"action": "resolve", "objection": objection.render(),
               "text": "sunflower-free, but nothing certifies extremality yet", "defer": true}),
    );
    ok(&c);
    let v = &c.response.structured;
    assert!(v["deferred"]["until"].is_string(), "{v}");
    assert!(
        v["deferred"]["why"]
            .as_str()
            .unwrap()
            .contains("is not offered to your class until"),
        "{v}"
    );
    let c = k.call(
        Some(lead),
        "view",
        json!({"action": "node", "path": "/claims/lemma-04"}),
    );
    let text = all_of(&c.response);
    assert!(
        text.contains(&format!("deferred by {} in ", chat.render()))
            && text.contains("nothing certifies extremality yet"),
        "{text}"
    );
    // The thread's holder has no chat: the chat worker is its voice.
    let c = k.call(
        Some(lead),
        "node",
        json!({"action": "create", "path": "/incidents/inc-311", "title": "EU checkout 500s",
               "keys": ["chat:incidents/T7001"]}),
    );
    ok(&c);
    let c = k.call(
        Some(chat),
        "talk",
        json!({"action": "said", "at": "/incidents/inc-311", "kind": "ack",
               "text": "We're on it; next update within the hour"}),
    );
    ok(&c);
    let text = all_of(&c.response);
    assert!(!text.contains("conversation_held"), "{text}");
    assert_eq!(
        c.response.structured["send"],
        json!("post it in this reply; you hold nothing, so it counts as sent now"),
        "{text}"
    );
    let c = k.call(
        Some(lead),
        "view",
        json!({"action": "node", "path": "/incidents/inc-311"}),
    );
    let text = all_of(&c.response);
    assert!(
        text.contains(
            "the conversation's voice: its author has chat, and no holder of the conversation has"
        ),
        "{text}"
    );
    assert!(
        text.contains("sent with its author's last reply, unverified"),
        "{text}"
    );
    // A note at the closed claim reaches the session that closed it.
    let c = k.call(
        Some(chat),
        "talk",
        json!({"action": "say", "at": "/claims/lemma-04", "text": "any certificate yet?"}),
    );
    ok(&c);
    assert_eq!(
        c.response.structured["delivered_to"],
        json!([lead.render()]),
        "{}",
        c.response.text
    );
}
