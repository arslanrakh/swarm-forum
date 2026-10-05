//! Tests of 1.11's texts and responses (the lab's sixth round, it5): what a `said` that answered
//! says, a note at a closed node, the health a session is shown, and rule 4's feed read.

use super::*;

/// The items of a response's next calls.
fn items(c: &Called) -> Vec<Value> {
    c.response.structured["next_calls"]["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// 1.11 (§9.2, §20.7, it5 l1): a `said` that answered requests it did not settle says that they
/// now read as answered, that the caller's close settles them when it holds their node, and that a
/// post that is only "on it" is an ack; one that answered nothing says nothing of it.
#[test]
fn a_said_that_answered_says_what_that_means() {
    let mut k = Kit::new();
    let (a, _) = k.hello("coder", json!({"action": "hello"}));
    ok(&k.call(
        Some(a),
        "node",
        json!({"action": "create", "path": "/deploys", "title": "slow staging deploys",
               "keys": ["chat:ops/7"],
               "request": {"text": "why are staging deploys slow?", "origin": "chat:ops/7/m1",
                           "requester": "Omar"}}),
    ));
    let request = k.w.s.requests.keys().next().copied().unwrap().render();
    // An ack answers nothing, and says nothing of it.
    let c = k.call(
        Some(a),
        "talk",
        json!({"action": "said", "at": "/deploys", "text": "on it", "kind": "ack"}),
    );
    ok(&c);
    assert_eq!(c.response.structured["answered"], json!([]));
    assert!(c.response.structured.get("answered_note").is_none());
    // "I'll compare …" as a reply answers the request at once: the response says so.
    let c = k.call(
        Some(a),
        "talk",
        json!({"action": "said", "at": "/deploys",
               "text": "I'll compare recent staging deploy timings with the earlier runs",
               "kind": "reply"}),
    );
    ok(&c);
    assert_eq!(c.response.structured["answered"], json!([request]));
    assert_eq!(
        c.response.structured["answered_note"],
        json!(format!(
            "{request} now reads as answered: your close settles it; if this was only 'on it', \
             such posts are kind ack"
        ))
    );
    // A session that does not hold the node is not told that its close settles it.
    let (b, _) = k.hello("coder", json!({"action": "hello"}));
    ok(&k.call(
        Some(b),
        "talk",
        json!({"action": "request", "at": "/deploys", "text": "and production?",
               "origin": "chat:ops/7/m2", "where": "chat:ops/7", "requester": "Omar"}),
    ));
    let second =
        k.w.s
            .requests
            .keys()
            .copied()
            .map(|r| r.render())
            .find(|r| *r != request)
            .unwrap();
    let c = k.call(
        Some(b),
        "talk",
        json!({"action": "said", "at": "/deploys", "text": "production is fine", "kind": "result"}),
    );
    ok(&c);
    assert_eq!(
        c.response.structured["answered_note"],
        json!(format!(
            "{second} now reads as answered; if this was only 'on it', such posts are kind ack"
        ))
    );
}

/// 1.11 (§13, §20.7, it5 l5): a note at a closed node, which is never taken, that reached no session
/// says so, and names the session that closed it as its last holder, unless the sender closed it.
/// 1.12 (§13, §21, it6 l6): in the `no_recipient` warning, with the close; `holder:` there reaches
/// the live closer as a note without `to` does; and a note to a silent session warns
/// `addressee_gone`.
#[test]
fn a_note_at_a_closed_node_says_that_it_reaches_nobody() {
    let mut k = Kit::new();
    let (a, _) = k.hello("coder", json!({"action": "hello"}));
    let c = k.call(
        Some(a),
        "node",
        json!({"action": "create", "path": "/tokens", "title": "migrate the tokens"}),
    );
    let token = match &ok(&c).reply {
        Reply::Created { token: Some(t), .. } => t.render(),
        other => panic!("{other:?}"),
    };
    ok(&k.call(
        Some(a),
        "node",
        json!({"action": "close", "path": "/tokens", "token": token, "outcome": "done",
               "summary": "migrated"}),
    ));
    let closed = match &k.w.s.nodes[&k.node("/tokens")].life {
        forum_core::Life::Closed(c) => c.at,
        forum_core::Life::Open => panic!(),
    };
    let e = super::super::source::Source::close_entry(
        &super::super::source::StoreSource::new(&k.w.conn),
        k.node("/tokens"),
    )
    .unwrap()
    .eid
    .render();
    let head = format!(
        "/tokens was closed done by {} at {} ({e}): nobody holds or will take it; this note \
         reaches nobody",
        a.render(),
        super::super::format::time(closed)
    );
    let say = |k: &mut Kit, s: SessionId, to: Option<&str>| {
        let mut args =
            json!({"action": "say", "at": "/tokens", "text": "PR #215 conflicts with #214"});
        if let Some(t) = to {
            args["to"] = json!(t);
        }
        let c = k.call(Some(s), "talk", args);
        ok(&c);
        (
            c.response.structured["delivered_to"].clone(),
            c.response.structured["warnings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|w| w["code"] == "no_recipient")
                .map(|w| w["detail"].as_str().unwrap().to_owned())
                .next(),
        )
    };
    // The closer's own note: nobody else holds or will take it.
    assert_eq!(say(&mut k, a, None), (json!([]), Some(head.clone())));
    assert_eq!(
        say(&mut k, a, Some("holder:")),
        (json!([]), Some(head.clone()))
    );
    // 1.12's review: its own agent question to `holder:` says it is a question.
    let c = k.call(
        Some(a),
        "talk",
        json!({"action": "ask", "at": "/tokens", "to": "holder:/tokens",
               "text": "did PR #215 land?"}),
    );
    ok(&c);
    assert_eq!(
        c.response.structured["warnings"][0]["detail"],
        json!(head.replace("this note", "this question")),
        "{}",
        c.response.text
    );
    // Another session's note, without `to` or to `holder:`, reaches the live closer.
    let (b, _) = k.hello("coder", json!({"action": "hello"}));
    assert_eq!(say(&mut k, b, None), (json!([a.render()]), None));
    assert_eq!(
        say(&mut k, b, Some("holder:/tokens")),
        (json!([a.render()]), None)
    );
    // Its closer falls silent: another session's note reaches nobody, and names the closer.
    let (c, _) = k.hello("coder", json!({"action": "hello"}));
    k.at(k.w.s.params.presence_silence.0 + 60_000);
    assert!(k.w.catch_up(k.w.clock));
    assert!(k.w.s.sessions[&a].silent);
    let named = format!(
        "{head}; to reach its last holder use to: session:{}",
        a.render()
    );
    assert_eq!(say(&mut k, c, None), (json!([]), Some(named.clone())));
    assert_eq!(say(&mut k, c, Some("holder:")), (json!([]), Some(named)));
    // A note to the silent session itself is recorded and delivered, and says it will not be read.
    let last = k.w.s.sessions[&a].last_seen;
    let d = k.call(
        Some(c),
        "talk",
        json!({"action": "say", "at": "/tokens", "to": format!("session:{}", a.render()),
               "text": "why is /payments paused?"}),
    );
    assert_eq!(d.response.structured["delivered_to"], json!([a.render()]));
    assert_eq!(
        warning_detail(&d, "addressee_gone"),
        format!(
            "{} has been silent since {}: it will not read this",
            a.render(),
            super::super::format::time(last)
        )
    );
}

/// 1.11 (§17, §20.1, it5 l5, l7): the health a session is shown counts its own caps and horizon:
/// the only session with a node's caps is not told that no session has them, while `control
/// health` shows the cache.
#[test]
fn the_health_a_session_is_shown_counts_its_caps() {
    let mut k = Kit::new();
    let (lead, _) = k.hello("coder", json!({"action": "hello"}));
    ok(&k.call(
        Some(lead),
        "node",
        json!({"action": "create", "path": "/a11y-focus", "title": "focus rings",
               "needs": ["a11y", "web"], "take": false}),
    ));
    let n = k.node("/a11y-focus");
    k.health = Health {
        level: Level::Yellow,
        reasons: vec![HealthReason::Unreachable {
            level: Level::Yellow,
            count: 1,
            by_needs: 1,
            by_horizon: 0,
            top: vec![(n, forum_core::health::UnreachableBy::Needs)],
        }],
        capacity: forum_core::health::CapacityReport::default(),
    };
    let status = |c: &Called| c.response.text.lines().next().unwrap().to_string();
    let (_, c) = k.hello("coder", json!({"action": "hello", "caps": ["repo"]}));
    assert!(
        status(&c).ends_with(
            " · health yellow: unreachable: /a11y-focus needs caps no session has (a11y, web)"
        ),
        "{}",
        status(&c)
    );
    let (s, c) = k.hello(
        "coder",
        json!({"action": "hello", "caps": ["a11y", "repo", "web"]}),
    );
    assert!(!status(&c).contains("health"), "{}", status(&c));
    let c = k.call(Some(s), "control", json!({"action": "health"}));
    assert!(!status(&c).contains("health"), "{}", status(&c));
    assert!(
        c.response.text.contains("unreachable"),
        "{}",
        c.response.text
    );
}

/// 1.11 (§20.9 rule 4, it5 l6): rule 4's item reads only what is new, `view feed {at, since}` from
/// the last seq the session was shown of that node, and `view node` of a node it was shown none
/// of; the feed read is the last such item.
#[test]
fn rule_4_reads_the_feed_from_what_the_session_was_shown() {
    let mut k = Kit::new();
    k.guided = true;
    let (a, _) = k.hello("coder", json!({"action": "hello"}));
    ok(&k.call(
        Some(a),
        "node",
        json!({"action": "create", "path": "/inc", "title": "checkout errors",
               "keys": ["chat:support/311"]}),
    ));
    ok(&k.call(
        Some(a),
        "node",
        json!({"action": "create", "path": "/inc/fix", "title": "the hotfix", "take": false}),
    ));
    let (b, _) = k.hello("coder", json!({"action": "hello"}));
    // Rule 4 counts what came after the take, so later.
    k.at(60_000);
    ok(&k.call(
        Some(b),
        "talk",
        json!({"action": "request", "at": "/inc", "text": "are EU checkouts failing?",
               "origin": "chat:support/311/m1", "where": "chat:support/311",
               "requester": "Tomás"}),
    ));
    ok(&k.call(Some(a), "view", json!({"action": "node", "path": "/inc"})));
    let seen = k.w.s.sessions[&a].shown[&k.node("/inc")];
    ok(&k.call(
        Some(b),
        "talk",
        json!({"action": "request", "at": "/inc", "text": "is it fixed yet?",
               "origin": "chat:support/311/m9", "where": "chat:support/311",
               "requester": "Tomás"}),
    ));
    let c = k.call(Some(a), "view", json!({"action": "list"}));
    let reread: Vec<Value> = items(&c)
        .into_iter()
        .filter(|x| x["rule"] == json!("reread"))
        .collect();
    assert_eq!(reread.len(), 1, "{}", c.response.text);
    assert_eq!(reread[0]["tool"], json!("view"));
    assert_eq!(
        reread[0]["args"],
        json!({"action": "feed", "session": a.render(), "at": "/inc", "since": seen.0})
    );
    assert!(
        c.response.text.contains(&format!(
            "view feed {{session: {}, at: /inc, since: {}}}",
            a.render(),
            seen.0
        )),
        "{}",
        c.response.text
    );
    // The feed read shows the new entry and marks the node shown: no rule 4 item follows.
    let c = k.call(
        Some(a),
        "view",
        json!({"action": "feed", "at": "/inc", "since": seen.0}),
    );
    ok(&c);
    assert!(c.response.text.contains("is it fixed yet?"));
    assert!(items(&c).iter().all(|x| x["rule"] != json!("reread")));
    // A session that holds a child it took was shown only that child: `view node` of the
    // conversation node.
    let (d, _) = k.hello("coder", json!({"action": "hello"}));
    ok(&k.call(
        Some(d),
        "hold",
        json!({"action": "take", "target": "/inc/fix"}),
    ));
    k.at(60_000);
    ok(&k.call(
        Some(b),
        "talk",
        json!({"action": "request", "at": "/inc", "text": "any news?",
               "origin": "chat:support/311/m10", "where": "chat:support/311",
               "requester": "Tomás"}),
    ));
    let c = k.call(Some(d), "view", json!({"action": "list"}));
    let reread: Vec<Value> = items(&c)
        .into_iter()
        .filter(|x| x["rule"] == json!("reread"))
        .collect();
    assert_eq!(reread.len(), 1, "{}", c.response.text);
    assert_eq!(
        reread[0]["args"],
        json!({"action": "node", "session": d.render(), "path": "/inc"})
    );
}
