//! Tests of 1.12's texts and responses (the lab's seventh round, it6): what a `to_user` answered
//! and left unsettled, its warnings on a node nobody asked for, the wait a release recorded, a
//! paused node's brief and rows, the pause's reason on the status line, and the end texts.

use super::*;

/// The token a hold-issuing reply carries.
fn token_of(c: &Called) -> String {
    match &ok(c).reply {
        Reply::Created { token: Some(t), .. } | Reply::Taken { token: t, .. } => t.render(),
        other => panic!("{other:?}"),
    }
}

/// 1.12 (§9.4, §20.7, it6 l1, l4): a result that answers a request without settling it lists it in
/// `not_settled`, with what settles it, and leaves out an empty `settled`; once a decision listed
/// it, the result settles it, and `settled` names it.
#[test]
fn a_result_says_what_it_answered_and_what_settles_it() {
    let mut k = Kit::new();
    let (a, _) = k.hello(
        "coder",
        json!({"action": "hello", "principal": "mail:lena@shop.example"}),
    );
    ok(&k.call(
        Some(a),
        "node",
        json!({"action": "create", "path": "/flaky", "title": "fix the flaky test"}),
    ));
    ok(&k.call(
        Some(a),
        "talk",
        json!({"action": "from_user", "text": "Fix the flaky test; tell me when there's a PR",
               "client": "ide", "to": "/flaky"}),
    ));
    let request = k.w.s.requests.keys().next().copied().unwrap();
    let c = k.call(
        Some(a),
        "talk",
        json!({"action": "to_user", "text": "PR #12 is up", "kind": "result",
               "re": [request.render()], "verbatim": true}),
    );
    ok(&c);
    let v = &c.response.structured;
    assert!(v.get("settled").is_none(), "{v}");
    assert_eq!(
        v["not_settled"],
        json!([{"request": request.render(),
                "why": "answered; your close of /flaky settles it, or release blocked with \
                        settles if it waits for its merge"}])
    );
    // Another session, which does not hold the node, is told what settles it there.
    let (b, _) = k.hello(
        "coder",
        json!({"action": "hello", "principal": "mail:lena@shop.example"}),
    );
    ok(&k.call(
        Some(b),
        "talk",
        json!({"action": "from_user", "text": "and the other flaky one", "client": "ide",
               "to": "/flaky"}),
    ));
    let second =
        k.w.s
            .requests
            .keys()
            .copied()
            .find(|r| *r != request)
            .unwrap();
    let c = k.call(
        Some(b),
        "talk",
        json!({"action": "to_user", "text": "fixed too", "kind": "result",
               "re": [second.render()]}),
    );
    assert_eq!(
        c.response.structured["not_settled"][0]["why"],
        json!("answered; a close of /flaky, or a decide that lists it in settles, settles it")
    );
    // A decision listed it first: the result settles it.
    let (x, _) = k.hello(
        "coder",
        json!({"action": "hello", "principal": "mail:lena@shop.example"}),
    );
    ok(&k.call(
        Some(x),
        "talk",
        json!({"action": "from_user", "text": "and a third", "client": "ide", "to": "/flaky"}),
    ));
    let third =
        k.w.s
            .requests
            .keys()
            .copied()
            .find(|r| *r != request && *r != second)
            .unwrap();
    ok(&k.call(
        Some(x),
        "talk",
        json!({"action": "decide", "at": "/flaky", "text": "the third is the same race",
               "settles": [third.render()]}),
    ));
    let c = k.call(
        Some(x),
        "talk",
        json!({"action": "to_user", "text": "the third is fixed by the same PR", "kind": "result",
               "re": [third.render()]}),
    );
    let v = &c.response.structured;
    assert_eq!(v["settled"], json!([third.render()]), "{v}");
    assert!(v.get("not_settled").is_none(), "{v}");
}

/// 1.12 (§20.7): a result for a request whose node is closed, as when a worker closed before it
/// replied, names only the decide, since closing a closed node is `invalid` (§5); that decide then
/// settles it.
#[test]
fn a_result_on_a_closed_node_names_only_the_decide() {
    let mut k = Kit::new();
    let (a, _) = k.hello(
        "coder",
        json!({"action": "hello", "principal": "mail:lena@shop.example"}),
    );
    ok(&k.call(
        Some(a),
        "node",
        json!({"action": "create", "path": "/flaky", "title": "fix the flaky test"}),
    ));
    ok(&k.call(
        Some(a),
        "talk",
        json!({"action": "from_user", "text": "Fix the flaky test", "client": "ide",
               "to": "/flaky"}),
    ));
    let request = k.w.s.requests.keys().next().copied().unwrap();
    ok(&k.call(
        Some(a),
        "node",
        json!({"action": "close", "path": "/flaky", "outcome": "done",
               "summary": "fixed in PR #12"}),
    ));
    let c = k.call(
        Some(a),
        "talk",
        json!({"action": "to_user", "text": "PR #12 fixes it", "kind": "result",
               "re": [request.render()]}),
    );
    assert_eq!(
        c.response.structured["not_settled"],
        json!([{"request": request.render(),
                "why": "answered; /flaky is closed, so a decide that lists it in settles, settles \
                        it"}]),
        "{}",
        c.response.text
    );
    let c = k.call(
        Some(a),
        "talk",
        json!({"action": "decide", "at": "/flaky", "text": "fixed in PR #12",
               "settles": [request.render()]}),
    );
    assert_eq!(
        c.response.structured["settled"],
        json!([request.render()]),
        "{}",
        c.response.text
    );
}

/// 1.12 (§8.1, §21, it6 l2): a worker's report on a node nobody asked for is told that nobody asked
/// for the work and where its outcome goes; `no_human_in_reach` then names only `principal`, the
/// result does not ask for a request that does not exist, and `to` names `principal`.
#[test]
fn a_report_on_work_nobody_asked_for_says_so() {
    let mut k = Kit::new();
    let (w, _) = k.hello("coder", json!({"action": "hello"}));
    ok(&k.call(
        Some(w),
        "node",
        json!({"action": "create", "path": "/tidepool/rss-crash", "title": "fix the RSS crash"}),
    ));
    let c = k.call(
        Some(w),
        "talk",
        json!({"action": "to_user", "at": "/tidepool/rss-crash", "text": "PR #49 fixes it",
               "kind": "result", "to": "principal:mail:sam@rivera.example"}),
    );
    ok(&c);
    let ignored: Vec<String> = c.response.structured["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["code"] == json!("ignored_arguments"))
        .map(|w| w["detail"].as_str().unwrap().to_string())
        .collect();
    assert!(
        ignored.contains(
            &"/tidepool/rss-crash has no unsettled request, so at was ignored: nobody asked for \
              this work; your outcome goes in the release's done or the close's summary; to tell \
              a person anyway, pass principal"
                .to_string()
        ),
        "{ignored:?}"
    );
    assert!(
        ignored
            .iter()
            .any(|d| d.ends_with("; to_user names its addressee with principal: <key>, not to")),
        "{ignored:?}"
    );
    assert_eq!(
        warning_detail(&c, "no_human_in_reach"),
        "nobody will read this: it is filed for user, whom no live session serves, and your \
         session serves no human; pass principal"
    );
    assert_eq!(
        warning_detail(&c, "result_answered_nothing"),
        "this result answered no request: user has none unsettled, so it stays on record as a \
         report"
    );
}

/// 1.12 (§10.3, §20.6, it6 l7): a release with `waits_for` says what it recorded and when the node
/// is offered again, a duration as the time it means.
#[test]
fn a_blocked_release_says_what_it_waits_for() {
    let mut k = Kit::new();
    let (a, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(a),
        "node",
        json!({"action": "create", "path": "/icons", "title": "replace the icon set"}),
    );
    let token = token_of(&c);
    let c = k.call(
        Some(a),
        "hold",
        json!({"action": "release", "token": token, "done": "PR #413 is up",
               "next_step": "close it once merged", "reason": "blocked",
               "waits_for": {"external": "PR #413 merged", "recheck_after": "2h"}}),
    );
    ok(&c);
    let at = super::super::format::time(k.w.clock.plus(Dur(2 * 3_600_000)));
    assert_eq!(
        c.response.structured["waiting"],
        json!(format!(
            "for 'PR #413 merged', outside the forum: offered again at its recheck, {at}, to \
             whoever takes it then"
        ))
    );
    // A handoff records no wait, and says none.
    let c = k.call(
        Some(a),
        "node",
        json!({"action": "create", "path": "/modal", "title": "the modal"}),
    );
    let token = token_of(&c);
    let c = k.call(
        Some(a),
        "hold",
        json!({"action": "release", "token": token, "done": "half", "next_step": "the rest",
               "reason": "handoff"}),
    );
    assert!(c.response.structured.get("waiting").is_none());
}

/// 1.12 (§10.4, §19, §20.4, it6 l6): an open node under a pause below `/` says so, with the pause's
/// reason, in its brief and in its rows, and a closed one does not; the status line's `Paused`
/// reason quotes it.
#[test]
fn a_paused_node_says_so_where_it_is_read() {
    let mut k = Kit::new();
    let (a, _) = k.hello("coder", json!({"action": "hello"}));
    let (b, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(a),
        "node",
        json!({"action": "create", "path": "/payments/vat-hotfix", "title": "the VAT hotfix",
               "take": false}),
    );
    ok(&c);
    ok(&k.call(
        Some(a),
        "node",
        json!({"action": "create", "path": "/payments/old-fix", "title": "an older fix"}),
    ));
    ok(&k.call(
        Some(a),
        "node",
        json!({"action": "close", "path": "/payments/old-fix", "outcome": "done",
               "summary": "shipped"}),
    ));
    let reason = "Freeze for INC-311: no payments deploys until the incident lead says so";
    ok(&k.call(
        Some(a),
        "control",
        json!({"action": "pause", "scope": "/payments", "reason": reason}),
    ));
    let until = k.w.s.pauses.values().next().unwrap().until;
    let want = format!(
        "paused at /payments by {} until {}: {reason}",
        a.render(),
        super::super::format::hhmm(until)
    );
    let c = k.call(
        Some(b),
        "view",
        json!({"action": "node", "path": "/payments/vat-hotfix"}),
    );
    assert_eq!(c.response.structured["node"]["paused"], json!(want));
    let c = k.call(Some(b), "view", json!({"action": "list"}));
    let row = |path: &str| {
        c.response.structured["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["path"] == json!(path))
            .cloned()
            .unwrap()
    };
    assert_eq!(row("/payments/vat-hotfix")["paused"], json!(want));
    assert_eq!(row("/payments")["paused"], json!(want));
    assert!(row("/payments/old-fix").get("paused").is_none());
    // The status line's `Paused` reason quotes the pause's reason, cut to 80 bytes.
    let r = HealthReason::Paused {
        level: Level::Red,
        count: 1,
        top: vec![PauseExclusion {
            pause: PauseView {
                scope: k.node("/payments"),
                by: a,
                class: class("coder"),
                until,
            },
            excluded: 2,
        }],
    };
    let src = StoreSource::new(&k.w.conn);
    assert_eq!(
        status::reason_words_shown(&k.w.s, &r, &src),
        format!(
            "paused: /payments by coder/{} until {} (\"{}\"), excludes 2",
            a.render(),
            super::super::format::hhmm(until),
            super::super::format::cut(reason, status::PAUSED_ITEM_REASON_MAX)
        )
    );
}

/// 1.12 (§20.9, it6 l1, l3): `end` says that the session needs no closing, and, after a `hello`
/// of a session that serves no principal, how a thread's message is recorded.
#[test]
fn the_end_says_no_closing_and_how_a_thread_s_message_is_recorded() {
    let mut k = Kit::new();
    let (worker, _) = k.hello("chat", json!({"action": "hello"}));
    let (ide, _) = k.hello(
        "coder",
        json!({"action": "hello", "principal": "mail:lena@shop.example"}),
    );
    let at_hello = |x: SessionId| {
        let g = Guidance {
            items: Vec::new(),
            total: Total::Exact(0),
            end: Some(forum_core::query::guide::End::AtHello),
        };
        next_calls::block(&k.w.s, Some(x), &g).0
    };
    assert_eq!(
        at_hello(worker),
        format!(
            "next_calls: none — end: nothing is offered to you; unless you came for a message or \
             an event, end your run; your session needs no closing{}",
            next_calls::END_THREAD
        )
    );
    assert!(
        at_hello(ide).ends_with("end your run; your session needs no closing"),
        "{}",
        at_hello(ide)
    );
    assert_eq!(
        next_calls::end_text(forum_core::query::guide::End::AfterNext),
        "nothing is offered to you: end your run; your session needs no closing, and waiting \
         nodes come back by themselves"
    );
}

/// 1.12's review: the flow rule 3 asks for, an intent before the pull request and its
/// `intent_done` in the release, with no checkpoint between. At the result, the node's open intent
/// names a pull request, so rule 8's close already names the blocked release that settles the
/// request, beside rule 3's `later` item for the `intent_done`; then one release closes the intent,
/// settles the request, and records the wait. Before, the close said "else release handoff".
#[test]
fn the_merge_wait_finish_after_an_intent_is_a_result_then_one_release() {
    let mut k = Kit::new();
    k.guided = true;
    let (a, _) = k.hello(
        "coder",
        json!({"action": "hello", "principal": "mail:lena@shop.example"}),
    );
    let c = k.call(
        Some(a),
        "node",
        json!({"action": "create", "path": "/flaky", "title": "fix the flaky test"}),
    );
    let token = token_of(&c);
    ok(&k.call(
        Some(a),
        "talk",
        json!({"action": "from_user", "text": "Fix the flaky test; tell me when there's a PR",
               "client": "ide", "to": "/flaky"}),
    ));
    let request = k.w.s.requests.keys().next().copied().unwrap();
    ok(&k.call(
        Some(a),
        "hold",
        json!({"action": "checkpoint", "token": token, "done": "fixed it locally",
               "next_step": "open the PR", "intent": "push the fix and open a PR"}),
    ));
    // The push and the pull request happen outside the forum.
    let c = k.call(
        Some(a),
        "talk",
        json!({"action": "to_user", "text": "PR #12 is up", "kind": "result",
               "re": [request.render()], "verbatim": true}),
    );
    ok(&c);
    let items: Vec<Value> = c.response.structured["next_calls"]["items"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let close: Vec<&Value> = items
        .iter()
        .filter(|x| x["tool"] == json!("node") && x["args"]["action"] == json!("close"))
        .collect();
    assert_eq!(close.len(), 1, "{}", c.response.text);
    assert_eq!(
        close[0]["why"],
        json!(format!(
            "if done; awaiting merge: release blocked, waits_for {{external: 'PR #N merged'}}, \
             settles: [{}]",
            request.render()
        )),
        "{}",
        c.response.text
    );
    let intent_entry = k.w.s.nodes[&k.node("/flaky")].open_intent.unwrap().eid;
    assert!(
        items.iter().any(|x| x["why"]
            == json!(format!(
                "after the action announced in {}; pass intent_done in the release if you \
                 release next",
                intent_entry.render()
            ))),
        "{}",
        c.response.text
    );
    // One release: the intent's `intent_done`, the request settled, the wait recorded.
    let c = k.call(
        Some(a),
        "hold",
        json!({"action": "release", "token": token, "done": "PR #12 is up",
               "next_step": "close it once PR #12 merges", "reason": "blocked",
               "intent_done": "pushed and opened PR #12",
               "settles": [request.render()],
               "waits_for": {"external": "PR #12 merged", "recheck_after": "2h"}}),
    );
    let d = ok(&c);
    let Reply::Released {
        intent_done: Some(_),
        settled,
        not_settled,
        ..
    } = &d.reply
    else {
        panic!("{:?}", d.reply);
    };
    assert_eq!(settled, &vec![request]);
    assert!(not_settled.is_empty());
    let s = &k.w.s;
    let n = k.node("/flaky");
    assert!(s.requests[&request].settled.is_some());
    assert!(s.nodes[&n].is_open() && s.nodes[&n].open_intent.is_none() && s.nodes[&n].pr_named);
    assert!(matches!(
        s.nodes[&n].wait.as_ref().map(|w| w.on),
        Some(forum_core::WaitOn::External(_))
    ));
}

/// 1.12 (§9.4, §10.3, §20.6, §20.9 rule 8; it6 l7, l1): the merge-wait finish, where the three
/// groups of 1.12 meet. After a result on a node whose `intent_done` named a pull request, the
/// `to_user` says what settles the request (group A), rule 8's close names the blocked release with
/// that request in `settles` (group B), and that one release settles it, as a close would (group
/// C), and says what wait it recorded (group A): the finish is the result, then one release.
#[test]
fn the_merge_wait_finish_is_a_result_then_one_release() {
    let mut k = Kit::new();
    k.guided = true;
    let (a, _) = k.hello(
        "coder",
        json!({"action": "hello", "principal": "mail:lena@shop.example"}),
    );
    let c = k.call(
        Some(a),
        "node",
        json!({"action": "create", "path": "/flaky", "title": "fix the flaky test"}),
    );
    let token = token_of(&c);
    ok(&k.call(
        Some(a),
        "talk",
        json!({"action": "from_user", "text": "Fix the flaky test; tell me when there's a PR",
               "client": "ide", "to": "/flaky"}),
    ));
    let request = k.w.s.requests.keys().next().copied().unwrap();
    ok(&k.call(
        Some(a),
        "hold",
        json!({"action": "checkpoint", "token": token, "done": "fixed it locally",
               "next_step": "open the PR", "intent": "push the fix and open a PR"}),
    ));
    ok(&k.call(
        Some(a),
        "hold",
        json!({"action": "checkpoint", "token": token, "done": "opened PR #12",
               "next_step": "wait for its merge", "intent_done": "pushed and opened PR #12"}),
    ));
    let c = k.call(
        Some(a),
        "talk",
        json!({"action": "to_user", "text": "PR #12 is up", "kind": "result",
               "re": [request.render()], "verbatim": true}),
    );
    ok(&c);
    assert_eq!(
        c.response.structured["not_settled"][0]["why"],
        json!(
            "answered; your close of /flaky settles it, or release blocked with settles if it \
             waits for its merge"
        )
    );
    let close: Vec<Value> = c.response.structured["next_calls"]["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|x| x["tool"] == json!("node") && x["args"]["action"] == json!("close"))
        .collect();
    assert_eq!(close.len(), 1, "{}", c.response.text);
    assert_eq!(
        close[0]["why"],
        json!(format!(
            "if done; awaiting merge: release blocked, waits_for {{external: 'PR #N merged'}}, \
             settles: [{}]",
            request.render()
        )),
        "{}",
        c.response.text
    );
    // The release that rule 8's `why` names settles the request and records the wait.
    let c = k.call(
        Some(a),
        "hold",
        json!({"action": "release", "token": token, "done": "PR #12 is up",
               "next_step": "close it once PR #12 merges", "reason": "blocked",
               "settles": [request.render()],
               "waits_for": {"external": "PR #12 merged", "recheck_after": "2h"}}),
    );
    let d = ok(&c);
    let Reply::Released {
        decision: Some(decision),
        settled,
        not_settled,
        ..
    } = &d.reply
    else {
        panic!("{:?}", d.reply);
    };
    assert_eq!(settled, &vec![request]);
    assert!(not_settled.is_empty());
    let v = &c.response.structured;
    assert_eq!(v["decision"], json!(decision.render()));
    assert_eq!(v["settled"], json!([request.render()]));
    assert!(v.get("not_settled").is_none(), "{v}");
    let at = super::super::format::time(k.w.clock.plus(Dur(2 * 3_600_000)));
    assert_eq!(
        v["waiting"],
        json!(format!(
            "for 'PR #12 merged', outside the forum: offered again at its recheck, {at}, to \
             whoever takes it then"
        ))
    );
    let s = &k.w.s;
    assert!(s.requests[&request].settled.is_some());
    let n = k.node("/flaky");
    assert!(s.nodes[&n].is_open());
    assert!(matches!(
        s.nodes[&n].wait.as_ref().map(|w| w.on),
        Some(forum_core::WaitOn::External(_))
    ));
}
