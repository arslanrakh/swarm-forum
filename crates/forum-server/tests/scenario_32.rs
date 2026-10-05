//! The §32 example over MCP streamable HTTP (design D.12.2, WP-F2): the eight steps of
//! `forum-core/tests/scenario_32.rs`, made by rmcp clients against the real server stack (the
//! engine, the bearer layer, and the `/mcp` route on a local port) over a temporary database, with
//! a manual clock from 2026-10-01T09:00Z. Each agent is an MCP client of its own, with its class's
//! bearer token; no call passes `session`, so the MCP binding carries it (D.11.4).
//!
//! What it asserts:
//! - every response: the status line first, also in `structuredContent.status`, and the next
//!   calls last, their text and structured forms agreeing item by item;
//! - the next calls of §32 against D.16, item by item, with the ids of this run (D.16's rows, the
//!   1.4 rows, and 1.5's rule 7 on a held node, R28, which adds a `later` decision to the two
//!   creates that record a request on the node they hold, as at core level);
//! - D.12.2's assertions of each step, on the rendered responses, and on the state tables where a
//!   response does not show them;
//! - the debug build's checks in the writer (`check_step`, `check_fire`, `check_invariants`,
//!   `check_guide`) ran on every call and firing, and none failed (a failure stops the writer);
//! - persist and reload after every commit, the history replayed to the state tables at the end,
//!   and a restart that loads them to the same state and renders the same reads (D.12.6).
//!
//! Step 1's clarification is still the open question of `forum-core/tests/scenario_32.rs`: the
//! `to_user` settles `r1` before it, so it is recorded as a new request that follows `r1` (D.14.5),
//! where §32 and D.12.2 say it attaches to `r1`. Like the core test, this one accepts either.
//! Step 8's report names the user's entry, not a request: it answers no request, and, since it
//! replies to an entry of its addressee, carries no `result_answered_nothing` (§21, 1.5).
//! Nobody calls `next`, so the idle share is unknown, and health never reads `overloaded`, though
//! demand exceeds supply from step 4's two-hour wait on (§17, 1.5).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test code"
)]

mod common;

use std::sync::atomic::Ordering;

use common::*;
use forum_core::{
    Cand, CommitmentState, DeadlineKind, Dur, Flag, Label, OccPhase, Owner, Params, PqState, Role,
    Routing, SaidState, Time, USER, USER_PRINCIPAL, WaitOn, spec,
};
use forum_server::clock::Clock;
use forum_server::engine::Hooks;
use serde_json::json;

const THREAD: &str = "chat:team/thread-1";

/// Step 2's create of summary-gap with the person's request, as both agent runs make it.
fn gap_request() -> serde_json::Value {
    json!({
        "action": "create",
        "path": "/summary-gap",
        "keys": [THREAD],
        "title": "Why did yesterday's summary miss two tickets?",
        "request": {
            "text": "why did yesterday's summary miss two tickets?",
            "origin": "chat:team/thread-1/1001",
            "requester": "alex"
        }
    })
}

/// The status line of `who` (`<session> (<class>)`) at `now`, with its holds, and its health
/// segment when the level is not green (1.7, §20.1).
fn status(now: &str, health: &str, who: &str, holds: &str, user: bool) -> String {
    format!(
        "[swarm] running · now {now} · session {who} · holds {holds} · inbox 0{}{}",
        if user { " · user_decisions 0" } else { "" },
        if health == "green" {
            String::new()
        } else {
            format!(" · health {health}")
        }
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn the_example_of_section_32_over_http() {
    let f = Forum::start("scenario-32", Params::default(), Hooks::default()).await;

    // 1. The user tells their chat assistant: "summarise new support tickets every morning".
    let mut asst = f.agent("chat-assistant").await;
    assert!(
        asst.instructions()
            .is_some_and(|i| i.starts_with("Swarm forum")),
        "the instructions at initialisation"
    );
    f.fed();
    let r = asst
        .ok(
            "session",
            json!({"action": "hello", "principal": "user", "from_user": {"text": "summarise new support tickets every morning", "client": "chat"}}),
        )
        .await;
    let a = asst.sid().to_string();
    assert_eq!(
        r.status(),
        status(
            "2026-10-01T09:00:00Z",
            "green",
            &format!("{a} (chat-assistant)"),
            "0",
            true
        )
    );
    let e1 = s(&r.v["user_entry"], "entry");
    assert!(
        r.v["user_entry"]["routed"].get("unrouted").is_some(),
        "{}",
        r.v
    );
    assert!(flags(&f.state(), USER).contains(Flag::Unrouted));
    assert_eq!(
        asst.next(&r),
        vec![nc(
            "now",
            "node",
            json!({"action": "create", "keys": [format!("user:{e1}")]}),
            &["path", "title"]
        )]
    );
    f.fed();
    let r = asst
        .ok(
            "node",
            json!({"action": "create", "path": "/digest/daily-tickets", "keys": [format!("user:{e1}")],
                   "title": "Daily digest of new support tickets",
                   "repeat": {"cron": "0 7 * * *", "tz": "Europe/Berlin", "run_within": "2h"}}),
        )
        .await;
    assert!(r.status().contains(
        "holds 1 (/digest/daily-tickets, 15:00 left, checkpoint by 2026-10-01T09:05:00Z)"
    ));
    let t_series = s(&r.v, "token");
    assert_eq!(r.v["user_requests"].as_array().unwrap().len(), 1);
    let r1 = r.v["user_requests"][0].as_str().unwrap().to_string();
    // No session history: no usual hour, and the run will likely be missed without an outside
    // trigger (R18).
    let w = r.warning("uncovered_slot").expect("uncovered_slot");
    let detail = s(w, "detail");
    assert!(detail.contains("(0/28 days)"), "{detail}");
    assert!(detail.contains("usual start hours none known"), "{detail}");
    assert!(
        detail.contains("without an outside trigger, the run will likely be missed"),
        "{detail}"
    );
    assert_eq!(
        asst.next(&r),
        vec![
            nc(
                "now",
                "hold",
                json!({"action": "checkpoint", "token": t_series}),
                &["intent", "done", "next_step"]
            ),
            // 1.7: the report, then the decision, both later: a series stays open, so no close
            // records the decision (§5); the series' release is cut, and comes back once r1
            // settles.
            nc(
                "later",
                "talk",
                json!({"action": "to_user", "principal": "user", "kind": "result", "re": [r1]}),
                &["text", "verbatim?"]
            ),
            nc(
                "later",
                "talk",
                json!({"action": "decide", "at": "/digest/daily-tickets", "settles": [r1]}),
                &["text", "basis?"]
            ),
        ]
    );
    let st = f.state();
    assert!(!flags(&st, USER).contains(Flag::Unrouted), "e1 is routed");
    let series = node(&st, "/digest/daily-tickets");
    // One waiting occurrence, named after its UTC slot (07:00 CEST is 05:00Z).
    let occ = node(&st, "/digest/daily-tickets/20261002T0500Z");
    assert_eq!(
        st.ix.series_open[&series]
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![occ]
    );
    assert!(matches!(&st.nodes[&occ].role, Role::Occurrence(x) if x.phase == OccPhase::Waiting));
    f.fed();
    let r = asst
        .ok(
            "talk",
            json!({"action": "to_user", "text": "The digest runs every morning at 07:00 Berlin time.",
                   "kind": "result", "verbatim": true, "re": [e1]}),
        )
        .await;
    // 1.12 (§20.7): no empty `settled`; the request it answered, not yet decided, is named in
    // `not_settled` with what settles it.
    assert!(r.v.get("settled").is_none(), "r1 answered, not yet decided");
    let open = r.v["not_settled"].as_array().expect("not_settled");
    assert_eq!(open.len(), 1, "{}", r.v);
    assert!(
        open[0]["why"].as_str().unwrap().starts_with("answered; "),
        "{}",
        r.v
    );
    assert_eq!(
        asst.next(&r),
        vec![nc(
            "now",
            "talk",
            json!({"action": "decide", "at": "/digest/daily-tickets", "settles": [r1]}),
            &["text", "basis?"]
        )]
    );
    f.fed();
    let r = asst
        .ok(
            "talk",
            json!({"action": "decide", "at": "/digest/daily-tickets", "text": "set up the daily digest", "settles": [r1]}),
        )
        .await;
    assert_eq!(r.v["settled"], json!([r1]), "r1 settled by the decision");
    assert!(f.state().requests[&eid(&r1)].settled.is_some());
    // D.16: the series' release, made only in the guided run; §32's assistant lets it expire.
    assert_eq!(
        asst.next(&r),
        vec![nc(
            "now",
            "hold",
            json!({"action": "release", "token": t_series, "reason": "handoff"}),
            &["done", "next_step"]
        )]
    );
    f.fed();
    let r = asst
        .ok(
            "talk",
            json!({"action": "from_user", "text": "and skip tickets tagged spam", "client": "chat", "clarifies": r1}),
        )
        .await;
    // Either reading of step 1 (see the module's note).
    let routed = &r.v["routed"];
    if let Some(x) = routed.get("clarifies") {
        assert_eq!(*x, json!(r1));
    } else {
        assert_eq!(routed["follows"], json!(r1), "{routed}");
        assert_eq!(routed["node"], json!("/digest/daily-tickets"), "{routed}");
        let follow_up = s(routed, "request");
        // 1.7: its report and its decision, both later, on the series the session holds.
        assert_eq!(
            asst.next(&r),
            vec![
                nc(
                    "later",
                    "talk",
                    json!({"action": "to_user", "principal": "user", "kind": "result", "re": [follow_up]}),
                    &["text", "verbatim?"]
                ),
                nc(
                    "later",
                    "talk",
                    json!({"action": "decide", "at": "/digest/daily-tickets", "settles": [follow_up]}),
                    &["text", "basis?"]
                ),
            ]
        );
    }

    // 2. A person writes in a team chat; two agent runs start on the same message, each with
    // `session hello`, since a call that names no session opens none (1.6, §20.1).
    let mut s1 = f.agent("chat-worker").await;
    s1.ok("session", json!({"action": "hello"})).await;
    f.fed();
    let r = s1.ok("node", gap_request()).await;
    let w1 = s1.sid().to_string();
    assert!(r.warnings().is_empty(), "{}", r.text);
    let t11 = s(&r.v, "token");
    let r2 = s(&r.v, "request");
    let st = f.state();
    let gap = node(&st, "/summary-gap");
    let req = &st.requests[&eid(&r2)];
    assert_eq!(req.origin.key.as_str(), "chat:team/thread-1/1001");
    assert_eq!(
        req.conv.as_str(),
        THREAD,
        "where defaults to the thread (A-61)"
    );
    assert_eq!(req.node, gap);
    assert_eq!(
        s1.next(&r),
        vec![
            nc(
                "later",
                "hold",
                json!({"action": "checkpoint", "token": t11}),
                &["done", "next_step"]
            ),
            // 1.7: the reply, later, on the node s-1 holds (1.6 had the decision there).
            nc(
                "later",
                "talk",
                json!({"action": "said", "at": "/summary-gap", "kind": "result"}),
                &["text"]
            ),
        ]
    );
    let mut s2 = f.agent("chat-worker").await;
    s2.ok("session", json!({"action": "hello"})).await;
    f.fed();
    let r = s2.ok("node", gap_request()).await;
    assert_eq!(r.v["existing"], json!("/summary-gap"));
    assert_eq!(r.v["holder"], json!([w1]));
    assert!(
        r.v.get("request").is_none(),
        "the same message: the same request (Q1)"
    );
    assert!(r.v.get("token").is_none(), "no hold");
    assert!(s2.next(&r).is_empty());
    assert_eq!(
        f.state()
            .requests
            .values()
            .filter(|q| q.node == gap)
            .count(),
        1
    );

    // 3. s-1 acknowledges, splits the work, and releases summary-gap.
    f.fed();
    let r = s1
        .ok(
            "talk",
            json!({"action": "said", "at": "/summary-gap", "kind": "ack", "text": "looking"}),
        )
        .await;
    let ack = s(&r.v, "said_id");
    // 1.9 (§9.2): no next call asks for the confirm; s-1 confirms with the link all the same.
    assert_eq!(s1.next(&r), vec![]);
    f.fed();
    let r = s1
        .ok(
            "talk",
            json!({"action": "said", "said_id": ack, "confirmed": "https://chat.example/team/thread-1/2001"}),
        )
        .await;
    assert_eq!(r.v["confirmed"], json!(true));
    assert!(s1.next(&r).is_empty());
    assert!(
        f.state()
            .commitments
            .values()
            .any(|c| c.node == gap && c.state == CommitmentState::Open)
    );
    for (p, title) in [
        ("/summary-gap/audit", "Audit the cutoff"),
        (
            "/summary-gap/rerun-test",
            "Re-run the digest in a test environment",
        ),
    ] {
        f.fed();
        let r = s1
            .ok(
                "node",
                json!({"action": "create", "path": p, "title": title, "take": false, "tags": ["offer"]}),
            )
            .await;
        assert!(r.v.get("token").is_none());
        assert!(s1.next(&r).is_empty(), "both are offered");
    }
    f.fed();
    let r = s1
        .ok(
            "hold",
            json!({"action": "release", "token": t11, "done": "acknowledged and split the work",
                   "next_step": "audit and re-run", "reason": "handoff"}),
        )
        .await;
    assert_eq!(r.v["leave_better"], json!([]));
    assert_eq!(
        s1.next(&r),
        vec![nc("now", "hold", json!({"action": "next"}), &[])]
    );
    let st = f.state();
    let audit = node(&st, "/summary-gap/audit");
    let rerun = node(&st, "/summary-gap/rerun-test");
    for n in [audit, rerun] {
        assert!(st.ix.next.of.contains_key(&Cand::Node(n)), "{n:?} offered");
    }
    // r2 is unsettled, and the node is unattended and not waiting: the literal rule (A-62).
    assert!(flags(&st, gap).contains(Flag::UnansweredRequest));
    assert!(st.ix.next.of.contains_key(&Cand::Node(gap)));

    // 4. s-3 takes audit, asks in the thread, and releases waiting for the answer.
    let mut s3 = f.agent("chat-worker").await;
    s3.ok("session", json!({"action": "hello"})).await;
    f.fed();
    let r = s3
        .ok(
            "hold",
            json!({"action": "take", "target": "/summary-gap/audit"}),
        )
        .await;
    let t12 = s(&r.v, "token");
    assert_eq!(
        s3.next(&r),
        vec![
            nc(
                "now",
                "view",
                json!({"action": "node", "path": "/summary-gap"}),
                &[]
            ),
            nc(
                "later",
                "hold",
                json!({"action": "checkpoint", "token": t12}),
                &["done", "next_step"]
            ),
        ]
    );
    assert!(r.why(0).starts_with("2 entries by others"), "{}", r.why(0));
    f.fed();
    let r = s3
        .ok(
            "talk",
            json!({"action": "ask", "at": "/summary-gap/audit", "text": "were the two tickets created after the cutoff?"}),
        )
        .await;
    let q1 = s(&r.v, "question");
    let asked = s(&r.v, "said_id");
    assert_eq!(
        r.v["node"],
        json!("/summary-gap"),
        "the conversation node (C1)"
    );
    assert!(
        r.warning("conversation_held").is_none(),
        "s-3 holds a descendant"
    );
    let moved = r.warning("moved").expect("moved");
    let entries = moved["entries"].as_array().unwrap();
    assert!(
        entries.contains(&json!(r2)) && entries.contains(&json!(ack)),
        "{moved}"
    );
    assert_eq!(
        s3.next(&r),
        vec![
            nc(
                "now",
                "view",
                json!({"action": "node", "path": "/summary-gap"}),
                &[]
            ),
            nc(
                "later",
                "hold",
                json!({"action": "release", "token": t12, "reason": "blocked", "waits_for": {"question": q1}}),
                &["done", "next_step"]
            ),
        ]
    );
    let st = f.state();
    assert_eq!(st.pquestions[&eid(&q1)].node, gap);
    assert_eq!(st.saids[&eid(&asked)].node, gap);
    f.fed();
    let r = s3
        .ok(
            "hold",
            json!({"action": "release", "token": t12, "done": "asked in the thread",
                   "next_step": "wait for the answer", "reason": "blocked", "waits_for": {"question": q1}}),
        )
        .await;
    assert_eq!(r.v["leave_better"], json!(["/summary-gap"]));
    // 1.8 (§20.9 rule 13): after a release, `hold next` too, while the worker holds nothing.
    assert_eq!(
        s3.next(&r),
        vec![
            nc(
                "now",
                "hold",
                json!({"action": "take", "target": "/summary-gap"}),
                &[]
            ),
            nc("now", "hold", json!({"action": "next"}), &[]),
        ]
    );
    let st = f.state();
    assert_eq!(
        st.saids[&eid(&asked)].state,
        SaidState::Confirmed,
        "the release confirms it"
    );
    assert!(matches!(
        st.nodes[&audit].wait.as_ref().map(|x| x.on),
        Some(WaitOn::Question { q, .. }) if q == eid(&q1)
    ));
    assert!(
        st.ix
            .deadlines
            .iter()
            .any(|k| k.kind == DeadlineKind::Recheck
                && k.owner == Owner::Entry(eid(&q1))
                && k.at == T0.plus(Dur(4 * HOUR_MS)))
    );
    f.advance(2 * HOUR_MS).await;
    assert!(
        !flags(&f.state(), gap).contains(Flag::OpenCommitment),
        "the open question suppresses it"
    );

    // 5. The person answers; s-4 runs on that event.
    let mut s4 = f.agent("chat-worker").await;
    s4.ok("session", json!({"action": "hello"})).await;
    f.fed();
    let r = s4
        .ok(
            "node",
            json!({"action": "create", "path": "/summary-gap", "keys": [THREAD],
                   "title": "Why did yesterday's summary miss two tickets?"}),
        )
        .await;
    assert!(
        r.status()
            .starts_with("[swarm] running · now 2026-10-01T11:00:00Z · session ")
    );
    assert_eq!(r.v["existing"], json!("/summary-gap"));
    assert_eq!(r.v["open_question"], json!(q1));
    assert_eq!(
        s4.next(&r),
        vec![
            nc(
                "now",
                "talk",
                json!({"action": "heard", "at": "/summary-gap", "where": THREAD, "answers": q1}),
                &["from", "text"]
            ),
            nc(
                "now",
                "hold",
                json!({"action": "take", "target": "/summary-gap"}),
                &[]
            ),
        ]
    );
    f.fed();
    let r = s4
        .ok(
            "talk",
            json!({"action": "heard", "at": "/summary-gap", "from": "alex",
                   "text": "yes, both after the cutoff", "answers": q1}),
        )
        .await;
    assert_eq!(r.v["answered"], json!(q1));
    // 1.12 (§20.9 rule 13): a worker that holds nothing gets `hold next` after any write while
    // `next` would offer it an item: the audit, whose wait the answer resolved.
    assert_eq!(
        s4.next(&r),
        vec![nc("now", "hold", json!({"action": "next"}), &[])]
    );
    let st = f.state();
    assert!(matches!(
        st.pquestions[&eid(&q1)].state,
        PqState::Answered { .. }
    ));
    assert!(
        !spec::waiting(&st, audit, st.last_now),
        "audit's wait resolved"
    );
    assert!(st.ix.next.of.contains_key(&Cand::Node(audit)));
    assert!(flags(&st, gap).contains(Flag::OpenCommitment));

    // 6. s-5 takes rerun-test, records an intent, and disappears; s-6 takes it over.
    let mut s5 = f.agent("chat-worker").await;
    s5.ok("session", json!({"action": "hello"})).await;
    f.fed();
    let r = s5
        .ok(
            "hold",
            json!({"action": "take", "target": "/summary-gap/rerun-test"}),
        )
        .await;
    let w5 = s5.sid().to_string();
    let t13 = s(&r.v, "token");
    assert_eq!(
        s5.next(&r),
        vec![
            nc(
                "now",
                "view",
                json!({"action": "node", "path": "/summary-gap"}),
                &[]
            ),
            nc(
                "later",
                "hold",
                json!({"action": "checkpoint", "token": t13}),
                &["done", "next_step"]
            ),
        ]
    );
    f.fed();
    let r = s5
        .ok(
            "hold",
            json!({"action": "checkpoint", "token": t13, "done": "prepared the test environment",
                   "next_step": "re-run the digest", "intent": "re-run yesterday's digest in a test environment"}),
        )
        .await;
    let e10 = s(&r.v, "intent");
    assert!(
        r.status().contains(
            "holds 1 (/summary-gap/rerun-test, 20:00 left, checkpoint by 2026-10-01T11:05:00Z)"
        ),
        "the checkpoint extended the lease to 20 minutes: {}",
        r.status()
    );
    assert_eq!(
        s5.next(&r),
        vec![
            nc(
                "now",
                "view",
                json!({"action": "node", "path": "/summary-gap"}),
                &[]
            ),
            nc(
                "later",
                "hold",
                json!({"action": "checkpoint", "token": t13}),
                &["intent_done", "done", "next_step"]
            ),
        ]
    );
    assert_eq!(
        r.why(1),
        format!(
            "after the action announced in {e10}; pass intent_done in the release if you release \
             next"
        )
    );
    // s-5 vanishes. It is not silent at 15 minutes while its lease runs (R27).
    f.advance(16 * MINUTE_MS).await;
    assert!(
        !f.state().sessions[&sid(&w5)].silent,
        "a holder is never silent"
    );
    f.advance(5 * MINUTE_MS).await;
    let st = f.state();
    assert!(
        !st.holds.keys().any(|t| t.node == rerun),
        "the hold expired"
    );
    assert!(st.sessions[&sid(&w5)].silent, "silent at its lease's end");
    assert!(flags(&st, rerun).contains(Flag::OpenIntent));
    assert!(
        st.nodes[&rerun]
            .tags
            .contains(&Label::new("offer").unwrap())
    );
    let mut s6 = f.agent("chat-worker").await;
    f.fed();
    let r = s6.ok("session", json!({"action": "hello"})).await;
    // `next` would offer summary-gap first: tier 0, and older (A-62); rerun-test shows its
    // open_intent.
    let offers = r.v["offers"].as_array().unwrap();
    assert_eq!(offers[0]["path"], json!("/summary-gap"), "{offers:?}");
    let rt = offers
        .iter()
        .find(|o| o["path"] == json!("/summary-gap/rerun-test"))
        .expect("rerun-test offered");
    assert_eq!(rt["why_this"]["flags"], json!(["open_intent"]));
    assert_eq!(
        s6.next(&r),
        vec![nc("now", "hold", json!({"action": "next"}), &[])]
    );
    f.fed();
    let r = s6
        .ok(
            "hold",
            json!({"action": "take", "target": "/summary-gap/rerun-test"}),
        )
        .await;
    let t132 = s(&r.v, "token");
    let unfinished = r.v["brief"]["sections"]
        .as_array()
        .and_then(|xs| xs.iter().find(|x| x["name"] == json!("unfinished")))
        .unwrap();
    assert_eq!(unfinished["items"][0]["intent"], json!(e10), "{unfinished}");
    assert_eq!(
        s6.next(&r),
        vec![
            nc(
                "now",
                "hold",
                json!({"action": "checkpoint", "token": t132}),
                &["intent_done", "done", "next_step"]
            ),
            nc(
                "now",
                "view",
                json!({"action": "node", "path": "/summary-gap"}),
                &[]
            ),
        ]
    );
    // R6: the check first.
    assert_eq!(
        r.why(0),
        format!(
            "once you checked if {w5}'s {e10} happened: record what you found; do not repeat it"
        )
    );
    f.fed();
    let r = s6
        .ok(
            "hold",
            json!({"action": "checkpoint", "token": t132, "done": "checked: the re-run had not happened; re-ran it",
                   "next_step": "close", "intent_done": "re-ran it myself"}),
        )
        .await;
    assert_eq!(r.v["intent_done"], json!(e10));
    assert_eq!(
        s6.next(&r),
        vec![
            nc(
                "now",
                "view",
                json!({"action": "node", "path": "/summary-gap"}),
                &[]
            ),
            nc(
                "later",
                "node",
                json!({"action": "close", "path": "/summary-gap/rerun-test", "token": t132}),
                &["outcome", "summary"]
            ),
        ]
    );
    // R11: the alternative is named; 1.12: by the node's state, here an intent_done that names
    // no pull request, so the rest is handed off.
    assert_eq!(
        r.why(1),
        "if done; else release handoff with the rest in next_step"
    );
    f.fed();
    let r = s6
        .ok(
            "node",
            json!({"action": "close", "path": "/summary-gap/rerun-test", "outcome": "done",
                   "summary": "re-ran the digest", "verified": "the re-run digest lists both tickets", "token": t132}),
        )
        .await;
    assert_eq!(r.v["leave_better"], json!(["/summary-gap"]));
    assert_eq!(
        s6.next(&r),
        vec![nc(
            "now",
            "hold",
            json!({"action": "take", "target": "/summary-gap"}),
            &[]
        )]
    );

    // 7. audit closes with a decision; s-7 settles the request and closes summary-gap.
    f.fed();
    let r = s6
        .ok(
            "hold",
            json!({"action": "take", "target": "/summary-gap/audit"}),
        )
        .await;
    let t122 = s(&r.v, "token");
    f.fed();
    let r = s6
        .ok(
            "talk",
            json!({"action": "decide", "at": "/summary-gap/audit", "text": "the cutoff excluded them"}),
        )
        .await;
    assert_eq!(
        s6.next(&r),
        vec![
            nc(
                "now",
                "view",
                json!({"action": "node", "path": "/summary-gap"}),
                &[]
            ),
            nc(
                "now",
                "node",
                json!({"action": "close", "path": "/summary-gap/audit", "token": t122}),
                &["outcome", "summary"]
            ),
        ]
    );
    f.fed();
    let r = s6
        .ok(
            "node",
            json!({"action": "close", "path": "/summary-gap/audit", "outcome": "done",
                   "summary": "the cutoff excluded them", "token": t122}),
        )
        .await;
    assert_eq!(
        r.v["leave_better"],
        json!(["/summary-gap"]),
        "its parent at its level (A-63)"
    );
    assert_eq!(
        s6.next(&r),
        vec![nc(
            "now",
            "hold",
            json!({"action": "take", "target": "/summary-gap"}),
            &[]
        )]
    );
    assert!(flags(&f.state(), gap).contains(Flag::ReadyToClose));
    let mut s7 = f.agent("chat-worker").await;
    s7.ok("session", json!({"action": "hello"})).await;
    f.fed();
    let r = s7
        .ok("hold", json!({"action": "take", "target": "/summary-gap"}))
        .await;
    let t112 = s(&r.v, "token");
    assert_eq!(
        r.v["brief"]["sections"][0]["items"],
        json!(["ready_to_close"])
    );
    // 1.7: the reply first, then the close, which records the decision and settles r2 (§5).
    assert_eq!(
        s7.next(&r),
        vec![
            nc(
                "now",
                "talk",
                json!({"action": "said", "at": "/summary-gap", "kind": "result"}),
                &["text"]
            ),
            nc(
                "later",
                "hold",
                json!({"action": "checkpoint", "token": t112}),
                &["done", "next_step"]
            ),
        ]
    );
    assert_eq!(
        r.why(0),
        "the parts are closed: reply with the result, then close, which settles it"
    );
    f.fed();
    let r = s7
        .ok(
            "talk",
            json!({"action": "said", "at": "/summary-gap", "kind": "result", "text": "they were created after the cutoff; fixed"}),
        )
        .await;
    let result = s(&r.v, "said_id");
    assert_eq!(r.v["settled"], json!([]), "answered, not yet decided");
    assert_eq!(
        r.v["fulfilled"],
        json!([ack]),
        "the result fulfils the commitment"
    );
    assert_eq!(
        s7.next(&r),
        vec![nc(
            "now",
            "node",
            json!({"action": "close", "path": "/summary-gap", "token": t112}),
            &["outcome", "summary"],
        ),]
    );
    assert!(
        f.state()
            .commitments
            .values()
            .all(|c| c.node != gap || c.state != CommitmentState::Open)
    );
    f.fed();
    let r = s7
        .ok(
            "node",
            json!({"action": "close", "path": "/summary-gap", "outcome": "done",
                   "summary": "the cutoff moved to midnight", "token": t112}),
        )
        .await;
    assert_eq!(
        r.v["settled"],
        json!([r2]),
        "the close settles r2 by default"
    );
    let st = f.state();
    assert!(st.requests[&eid(&r2)].settled.is_some());
    assert!(
        st.saids[&eid(&result)].with_reply,
        "the close confirms the result with the reply"
    );
    assert!(r.v.get("warnings").is_none(), "no warnings: {}", r.text);
    assert!(!r.text.contains("warnings:"), "{}", r.text);
    assert_eq!(r.v["leave_better"], json!([]));
    // `hold next` while anything is offered; otherwise none (D.16). 1.11 (§10.4, §12): the series,
    // whose occurrence waits for its slot, is held back, and no longer offered.
    let st = f.state();
    let offered = !spec::ranked(
        &st,
        sid(s7.sid()),
        st.last_now,
        &forum_core::NextQuery::default(),
    )
    .is_empty();
    assert_eq!(
        s7.next(&r),
        if offered {
            vec![nc("now", "hold", json!({"action": "next"}), &[])]
        } else {
            vec![]
        }
    );

    // 8. The user asks a coding agent: "what about my tasks?".
    let mut coder = f.agent("coding-agent").await;
    f.fed();
    let r = coder
        .ok(
            "session",
            json!({"action": "hello", "principal": "user", "from_user": {"text": "what about my tasks?", "client": "ide"}}),
        )
        .await;
    let wc = coder.sid().to_string();
    let e = s(&r.v["user_entry"], "entry");
    assert!(r.v["user_entry"]["routed"].get("unrouted").is_some());
    assert_eq!(
        r.v["principals"],
        json!(["user"]),
        "the session serves user"
    );
    assert!(
        f.state().sessions[&sid(&wc)]
            .principals
            .contains(&USER_PRINCIPAL)
    );
    let create_for = nc(
        "now",
        "node",
        json!({"action": "create", "keys": [format!("user:{e}")]}),
        &["path", "title"],
    );
    assert_eq!(coder.next(&r), vec![create_for.clone()]);
    f.fed();
    let r = coder.ok("view", json!({"action": "requests"})).await;
    // `user` is the only principal seen, so the board is `all` (§8.3): it lists `e` itself, `r1`,
    // and `r2`, the latter from a person outside the swarm, both answered.
    let rows = r.v["rows"].as_array().unwrap();
    let row = |k: &str, id: &str| rows.iter().find(|x| x[k] == json!(id)).cloned();
    assert_eq!(row("entry", &e).unwrap()["state"], json!("unrouted"));
    assert_eq!(row("request", &r1).unwrap()["state"], json!("answered"));
    let rr2 = row("request", &r2).unwrap();
    assert_eq!(rr2["state"], json!("answered"));
    assert_eq!(rr2["requester"], json!("alex"));
    assert_eq!(coder.next(&r), vec![create_for]);
    f.fed();
    let r = coder
        .ok(
            "talk",
            json!({"action": "to_user", "text": "The daily digest is set up, and the team chat has its answer.",
                   "kind": "result", "verbatim": true, "re": [e]}),
        )
        .await;
    assert!(
        r.warnings().is_empty(),
        "a reply to the user's entry: {}",
        r.text
    );
    assert!(
        !r.status().contains("health"),
        "green is left out: {}",
        r.status()
    );
    // 1.9 (§8.2): the result leaves the entry unrouted, and rule 5 suggests its route to none, if
    // nothing else in it needs work; then nothing is left to do.
    assert!(r.v.get("routed").is_none(), "{}", r.text);
    assert_eq!(
        coder.next(&r),
        vec![nc(
            "now",
            "talk",
            json!({"action": "route", "entry": e, "to": "none"}),
            &["reason?"]
        )]
    );
    f.fed();
    let r = coder
        .ok(
            "talk",
            json!({"action": "route", "entry": e, "to": "none", "reason": "answered from the board"}),
        )
        .await;
    assert!(coder.next(&r).is_empty());
    f.fed();
    // Demand exceeds supply, and nobody called `next`: the idle share is unknown, so `control
    // health` leaves it out and nothing reads `overloaded` (§17, 1.5); health stayed green.
    let r = coder.ok("control", json!({"action": "health"})).await;
    assert!(
        !r.status().contains("health"),
        "green is left out: {}",
        r.status()
    );
    assert_eq!(r.v["level"], json!("green"), "{}", r.text);
    let cap = &r.v["capacity"];
    assert_eq!(cap["next_calls"], json!(0), "{}", r.text);
    assert!(cap.get("idle_permille").is_none(), "{}", r.text);
    assert!(
        cap["demand_min"].as_u64().unwrap() > cap["supply_min"].as_u64().unwrap(),
        "{}",
        r.text
    );
    f.fed();
    let board = coder.ok("view", json!({"action": "requests"})).await.text;

    // Throughout, the writer's checks held (a failure would have stopped it); at the end no word
    // of the user is unrouted, persist and reload held at every commit, and the history replays
    // to the state tables.
    let stats = f.engine.stats();
    assert!(f.engine.alive());
    assert!(stats.guides_checked.load(Ordering::SeqCst) >= 36);
    assert!(stats.fires_checked.load(Ordering::SeqCst) > 0);
    assert!(stats.settles_checked.load(Ordering::SeqCst) > 0);
    let st = f.state();
    assert!(
        st.user_entries
            .values()
            .all(|u| u.routing != Routing::Unrouted)
    );
    assert!(!flags(&st, USER).contains(Flag::Unrouted));
    f.assert_reloads(30);
    assert!(f.verify() == st, "the history folds to the state tables");

    // The state tables reload to the same state, and the same session's read renders the same
    // bytes from it (FQ-2, D.11.8).
    for x in [asst, s1, s2, s3, s4, s5, s6, s7, coder] {
        x.close().await;
    }
    let events = f.events();
    let f = f.restart(Hooks::default()).await;
    assert_eq!(
        f.events(),
        events,
        "a restart at the same time appends nothing"
    );
    assert!(f.state() == st, "the tables load to the same state");
    let mut again = f.agent("coding-agent").await;
    f.fed();
    let r = again
        .ok("view", json!({"action": "requests", "session": wc}))
        .await;
    assert_eq!(r.text, board, "the reloaded state renders the board alike");

    // The series' first run comes at its slot: available, and a candidate (2026-10-02T05:00Z).
    let slot = Time(1_790_917_200_000);
    f.advance(slot.0 - f.clock.now().0).await;
    let st = f.state();
    assert!(matches!(&st.nodes[&occ].role, Role::Occurrence(x) if x.phase == OccPhase::Available));
    assert!(st.ix.next.of.contains_key(&Cand::Node(occ)));
    f.assert_reloads(30);
    f.verify();
}
