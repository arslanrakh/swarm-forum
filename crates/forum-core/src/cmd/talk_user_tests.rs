//! Unit tests of WP-D5a's commands (design D.8.11, D.8.12, D.12.1's user side and its 1.4 row):
//! `talk from_user`, `hello {from_user}`, `talk route`, `talk to_user`, and `talk ask` to a human.
//! Every call runs through `World::call_ctx`, which checks `check_step` (N1, U1, D1, Settled, C1
//! among them), the dirty set, `decide == apply`, the fold of the events, every firing with
//! `check_fire`, and every state invariant with R1 after the call.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::wildcard_enum_match_arm,
    reason = "test code: the deny list guards the crate's own code"
)]

use alloc::collections::BTreeSet;
use alloc::vec;
use alloc::vec::Vec;

use crate::apply::tests::{class, create_with, created, ok, refused, text};
use crate::cmd::*;
use crate::error::{Error, Field, What, Why};
use crate::event::{EntryBody, Ev, QuestionBody, TextField};
use crate::helpers::user::tests::{
    Ask, Dir, ask_cmd, asked, decision, dir, evs, ext_request, from_user_cmd, mk, open_as,
    people_question, pid, pk, route_cmd, routed, to_user_cmd, token_of, user_request, world_with,
};
use crate::reply::{Reply, RoutedReply, UnroutedWhy, UserEntryReply, WarningCode, WarningDetail};
use crate::state::{DigestItem, DigestState, Routing, UqSource, UqState};
use crate::testkit::World;
use crate::types::*;

fn nf(what: What) -> Error {
    Error::not_found(what)
}

fn inv(why: Why) -> Error {
    Error::invalid(why)
}

/// A world with a worker and an assistant session that serves `mail:alice@x`.
fn assistant() -> (World, ClassName, SessionId, ClassName, SessionId) {
    let mut w = world_with(|_| {});
    let c = class("worker");
    let s = w.hello(&c);
    let ac = class("assistant");
    let a = open_as(&mut w, &ac, Some("mail:alice@x"));
    (w, c, s, ac, a)
}

fn entry_routing(w: &World, e: UserEntryId) -> Routing {
    w.s.user_entries[&e].routing
}

// ------------------------------------------------------------------------------------------------
// from_user (D.8.11, §8.1, §8.2, R3, D.14.5)

/// Each `routed` branch: no target with 0 and 2 holds, a node, the session's only hold, a
/// clarification, a clarification of a settled request (a new request that `follows` it), an
/// answer, and an answer to a question no longer pending; `answers` > `clarifies` > `node`.
#[test]
fn from_user_returns_each_routed_branch() {
    let (mut w, c, s, ac, a) = assistant();
    let alice = pid(&w, "mail:alice@x");
    // No target, no hold: unrouted, flagged on @user (N1), offered for routing.
    let d = ok(w.call(&ac, a, from_user_cmd("hi", None, None, None, None)));
    let (e0, r) = routed(&d);
    assert_eq!(r, RoutedReply::Unrouted(UnroutedWhy::NoTarget { holds: 0 }));
    assert_eq!(entry_routing(&w, e0), Routing::Unrouted);
    assert!(w.s.ix.unrouted.contains(&e0));
    assert!(w.s.ix.node[&USER].flags.contains(Flag::Unrouted));
    assert!(matches!(d.reply, Reply::FromUser { principal, ref key, .. }
        if principal == alice && *key == KeyStr::of_user_entry(e0)));
    // `node`: a new request there, with alice as its requester and the entry's key on the node.
    let n = mk(&mut w, &c, s, "/n", |_| {});
    let (e1, r1) = user_request(&mut w, &ac, a, n, "mail:alice@x");
    assert_eq!(
        entry_routing(&w, e1),
        Routing::Request {
            node: n,
            request: r1
        }
    );
    assert_eq!(w.s.requests[&r1].requester, Requester::User(alice));
    assert_eq!(w.s.requests[&r1].from_user, Some(e1));
    assert!(w.s.nodes[&n].keys.contains_key(&KeyStr::of_user_entry(e1)));
    // The session's only live hold.
    let (h1, _) = created(&ok(w.call(&ac, a, create_with("/h1", |_| {}))));
    let d = ok(w.call(&ac, a, from_user_cmd("for h1", None, None, None, None)));
    let (e2, r2) = routed(&d);
    assert!(matches!(r2, RoutedReply::Request { node, follows: None, .. } if node == h1));
    assert!(matches!(entry_routing(&w, e2), Routing::Request { node, .. } if node == h1));
    // Two holds: no target.
    ok(w.call(&ac, a, create_with("/h2", |_| {})));
    let d = ok(w.call(&ac, a, from_user_cmd("which?", None, None, None, None)));
    assert_eq!(
        routed(&d).1,
        RoutedReply::Unrouted(UnroutedWhy::NoTarget { holds: 2 })
    );
    // A clarification of an unsettled request.
    let d = ok(w.call(&ac, a, from_user_cmd("also", None, None, None, Some(r1))));
    let (e3, r3) = routed(&d);
    assert_eq!(r3, RoutedReply::Clarifies { request: r1 });
    assert!(w.s.ix.clarifications[&r1].contains(&e3));
    // A clarification of a settled request: a new request on its node, which follows it.
    decision(&mut w, s, n, &[r1]);
    ok(w.call(&ac, a, to_user_cmd(ToUserKind::Result, None, &[r1])));
    assert!(w.s.requests[&r1].settled.is_some());
    let d = ok(w.call(
        &ac,
        a,
        from_user_cmd("and one more", None, None, Some(n), Some(r1)),
    ));
    let (e4, r4) = routed(&d);
    let RoutedReply::Request {
        request,
        node,
        follows,
    } = r4
    else {
        panic!("{r4:?}");
    };
    assert_eq!((node, follows), (n, Some(r1)));
    assert_ne!(request, r1);
    assert_eq!(w.s.requests[&request].node, n);
    assert_eq!(w.s.requests[&request].from_user, Some(e4));
    assert!(w.s.requests[&request].settled.is_none());
    assert!(w.s.ix.unsettled_requests.contains(&request));
    // An answer to a pending question, given with `clarifies` and `node` too: the answer wins.
    let q = asked(&ok(w.call(&c, s, ask_cmd(n, &Ask::default())))).0;
    let d = ok(w.call(
        &ac,
        a,
        from_user_cmd("yes", None, Some(q), Some(n), Some(request)),
    ));
    let (e5, r5) = routed(&d);
    assert_eq!(r5, RoutedReply::Answers { question: q });
    assert_eq!(w.s.uquestions[&q].state, UqState::Answered { by: e5 });
    assert!(
        w.s.ix
            .pending_uq_by
            .values()
            .all(|x| x.iter().all(|r| r.q != q))
    );
    // A second answer: the question is no longer pending, so the entry stays unrouted (A-37).
    let d = ok(w.call(&ac, a, from_user_cmd("no, wait", None, Some(q), None, None)));
    let (e6, r6) = routed(&d);
    assert_eq!(
        r6,
        RoutedReply::Unrouted(UnroutedWhy::NotPending { question: q })
    );
    assert_eq!(entry_routing(&w, e6), Routing::Unrouted);
    // `clarifies` before `node`.
    let n2 = mk(&mut w, &c, s, "/n2", |_| {});
    let d = ok(w.call(
        &ac,
        a,
        from_user_cmd("x", None, None, Some(n2), Some(request)),
    ));
    assert_eq!(routed(&d).1, RoutedReply::Clarifies { request });
    assert_eq!(w.replay(), w.s);
}

/// A user request of alice on `n`, decided and answered, so settled.
fn settled_request(
    w: &mut World,
    s: SessionId,
    ac: &ClassName,
    a: SessionId,
    n: NodeId,
) -> RequestId {
    let (_, r) = user_request(w, ac, a, n, "mail:alice@x");
    decision(w, s, n, &[r]);
    ok(w.call(ac, a, to_user_cmd(ToUserKind::Result, None, &[r])));
    assert!(w.s.requests[&r].settled.is_some());
    r
}

/// A clarification of a settled request is a new request on that request's node, whatever `node`
/// names: `clarifies` comes before `node` (A-99, §8.1), and `node` is ignored.
#[test]
fn a_follow_up_goes_to_the_settled_requests_node() {
    let (mut w, c, s, ac, a) = assistant();
    let n = mk(&mut w, &c, s, "/n", |_| {});
    let n2 = mk(&mut w, &c, s, "/n2", |_| {});
    let r = settled_request(&mut w, s, &ac, a, n);
    let d = ok(w.call(
        &ac,
        a,
        from_user_cmd("one more", None, None, Some(n2), Some(r)),
    ));
    let RoutedReply::Request {
        request,
        node,
        follows,
    } = routed(&d).1
    else {
        panic!("{:?}", d.reply);
    };
    assert_eq!((node, follows), (n, Some(r)));
    assert_eq!(w.s.requests[&request].node, n);
    assert!(w.s.requests.values().all(|x| x.node != n2));
}

/// `route {existing}` on the entry of a clarification of a settled request returns its request and
/// node as the first routing gave them. `follows` is in the first reply only: neither `Routing` nor
/// the history records the link, so `{existing}` cannot give it back (§8.2 asks for "the first
/// routing"; an open question for the owner of `Routing`, WP-D1). This pins what the
/// core does meanwhile.
#[test]
fn route_existing_on_a_follow_up_gives_its_request_and_node() {
    let (mut w, c, s, ac, a) = assistant();
    let n = mk(&mut w, &c, s, "/n", |_| {});
    let r = settled_request(&mut w, s, &ac, a, n);
    let d = ok(w.call(&ac, a, from_user_cmd("one more", None, None, None, Some(r))));
    let (e, first) = routed(&d);
    let RoutedReply::Request { request, node, .. } = first else {
        panic!("{first:?}");
    };
    assert_eq!(
        first,
        RoutedReply::Request {
            request,
            node: n,
            follows: Some(r)
        }
    );
    let d = ok(w.call(&c, s, route_cmd(e, None, "already routed")));
    assert_eq!(
        d.reply,
        Reply::Routed {
            entry: e,
            routed: RoutedReply::Request {
                request,
                node,
                follows: None
            },
            existing: true
        }
    );
}

/// A clarification of a settled request whose node has since been merged is recorded on that
/// node, as §8.1 reads ("a new request on the same node"): settled requests stay where they were
/// in a merge, and the follow-up is not resolved through `merged_into` (A-88: command targets do
/// not follow it). Nobody can take the merged node, so the request waits on the board; whether it
/// should go to the node it was merged into is an open question. This pins the
/// literal reading meanwhile.
#[test]
fn a_follow_up_on_a_merged_node_stays_on_that_node() {
    let (mut w, c, s, ac, a) = assistant();
    let n = mk(&mut w, &c, s, "/n", |_| {});
    let m = mk(&mut w, &c, s, "/m", |_| {});
    let r = settled_request(&mut w, s, &ac, a, n);
    ok(w.call(
        &c,
        s,
        Cmd::Node(NodeCmd::Merge {
            from: NodeRef::Id(n),
            into: NodeRef::Id(m),
        }),
    ));
    assert_eq!(w.s.requests[&r].node, n);
    let d = ok(w.call(&ac, a, from_user_cmd("one more", None, None, None, Some(r))));
    let RoutedReply::Request {
        request,
        node,
        follows,
    } = routed(&d).1
    else {
        panic!("{:?}", d.reply);
    };
    assert_eq!((node, follows), (n, Some(r)));
    assert_eq!(w.s.requests[&request].node, n);
    assert!(matches!(
        &w.s.nodes[&n].life,
        crate::state::Life::Closed(x) if x.merged_into == Some(m)
    ));
}

/// An answer to an escalated people question's user question also answers the people question
/// (A-60), and its marks go.
#[test]
fn from_user_answers_an_escalation_and_its_people_question() {
    let (mut w, c, s, ac, a) = assistant();
    let n = mk(&mut w, &c, s, "/n", |a| {
        a.keys = vec![crate::helpers::user::tests::key("chat:t/1")];
    });
    let pq = people_question(&mut w, s, n, "chat:t/1", true);
    // The escalation's question for the user and the people question's state, by events alone
    // (the Recheck arm is WP-D5b's).
    let uq = w.s.counters.next_eid;
    let due = w.clock.plus(HOUR);
    crate::apply::tests::entry(
        &mut w,
        USER_DECISIONS,
        crate::state::Author::Server,
        EntryBody::Question(QuestionBody::User {
            source: UqSource::Escalation { question: pq },
            about: Some(n),
            to: vec![USER_PRINCIPAL],
            due,
            has_default: false,
        }),
    );
    w.emit(Ev::PqEscalated { q: pq, uq });
    let d = ok(w.call(
        &ac,
        a,
        from_user_cmd("go ahead", None, Some(uq), None, None),
    ));
    let (e, r) = routed(&d);
    assert_eq!(r, RoutedReply::Answers { question: uq });
    assert_eq!(w.s.uquestions[&uq].state, UqState::Answered { by: e });
    assert_eq!(
        w.s.pquestions[&pq].state,
        crate::state::PqState::Answered { by: e }
    );
}

/// `from_user`'s refusals in D.8.24's order (D.14.5): the target node, then `not_found` of the
/// question and the request, then `invalid` for an answer to another kind of question, a reserved
/// node, and an ambiguous principal; nothing is applied.
#[test]
fn from_user_refusals_in_order() {
    let (mut w, c, s, ac, a) = assistant();
    let n = mk(&mut w, &c, s, "/n", |a| {
        a.keys = vec![crate::helpers::user::tests::key("chat:t/1")];
    });
    let pq = people_question(&mut w, s, n, "chat:t/1", false);
    let r = ext_request(&mut w, s, n, "mail:m1");
    let missing = Eid(9_999);
    let cases: Vec<(Cmd, Error)> = vec![
        // The target first, whatever else is wrong.
        (
            from_user_cmd("x", None, Some(pq), Some(NodeId(999)), Some(missing)),
            nf(What::Node),
        ),
        (
            from_user_cmd("x", None, Some(missing), None, None),
            nf(What::Question),
        ),
        // A request is no question.
        (
            from_user_cmd("x", None, Some(r), None, None),
            nf(What::Question),
        ),
        // `not_found` of the request before `invalid` of the answer.
        (
            from_user_cmd("x", None, Some(pq), None, Some(missing)),
            nf(What::Request),
        ),
        (
            from_user_cmd("x", None, Some(pq), Some(USER), None),
            inv(Why::NotUserQuestion),
        ),
        (
            from_user_cmd("x", None, None, Some(USER), None),
            inv(Why::ReservedNode),
        ),
    ];
    for (cmd, want) in cases {
        refused(&mut w, &ac, a, cmd, want);
    }
    // A session that serves two persons must name the principal.
    ok(w.call(
        &ac,
        a,
        from_user_cmd("hi", Some("mail:bob@x"), None, None, None),
    ));
    refused(
        &mut w,
        &ac,
        a,
        from_user_cmd("hi", None, None, None, None),
        inv(Why::AmbiguousPrincipal),
    );
    // Even then, a reserved node is refused first.
    refused(
        &mut w,
        &ac,
        a,
        from_user_cmd("hi", None, None, Some(USER_DIGEST), None),
        inv(Why::ReservedNode),
    );
}

/// Principal resolution in its cases (D.8.11, CS-1): the argument; the session's only principal,
/// or the lowest of the aliases of one person it serves; `AmbiguousPrincipal` for one that serves
/// several persons. Since 1.9 a `from_user` in a session that serves none names its principal,
/// `user` among them, or is `NoPrincipal`; a `to_user` there is still for `user`.
#[test]
fn from_user_resolves_its_principal() {
    let Dir { mut w, c, s, .. } = dir();
    let principal_of = |d: &crate::reply::Done| match d.reply {
        Reply::FromUser { principal, .. } => principal,
        ref other => panic!("{other:?}"),
    };
    let x = w.hello(&class("bot"));
    refused(
        &mut w,
        &class("bot"),
        x,
        from_user_cmd("hi", None, None, None, None),
        inv(Why::NoPrincipal),
    );
    let d = ok(w.call(
        &class("bot"),
        x,
        from_user_cmd("hi", Some("user"), None, None, None),
    ));
    assert_eq!(principal_of(&d), USER_PRINCIPAL);
    // The session now serves `user`; the argument wins.
    let d = ok(w.call(
        &class("bot"),
        x,
        from_user_cmd("hi", Some("mail:new@x"), None, None, None),
    ));
    assert!(w.s.principal_ids.contains_key(&pk("mail:new@x")));
    assert_eq!(principal_of(&d), pid(&w, "mail:new@x"));
    // Two aliases of bob: one person, the lowest id.
    let b = open_as(&mut w, &class("bot2"), Some("chat:bob"));
    ok(w.call(
        &class("bot2"),
        b,
        from_user_cmd("hi", Some("mail:bob@x"), None, None, None),
    ));
    let d = ok(w.call(
        &class("bot2"),
        b,
        from_user_cmd("hi", None, None, None, None),
    ));
    let lowest = pid(&w, "mail:bob@x").min(pid(&w, "chat:bob"));
    assert_eq!(principal_of(&d), lowest);
    // A second person: ambiguous.
    ok(w.call(
        &class("bot2"),
        b,
        from_user_cmd("hi", Some("mail:carol@x"), None, None, None),
    ));
    refused(
        &mut w,
        &class("bot2"),
        b,
        from_user_cmd("hi", None, None, None, None),
        inv(Why::AmbiguousPrincipal),
    );
    refused(
        &mut w,
        &class("bot2"),
        b,
        to_user_cmd(ToUserKind::Ack, None, &[]),
        inv(Why::AmbiguousPrincipal),
    );
    let _ = (c, s);
}

// ------------------------------------------------------------------------------------------------
// hello {from_user} (D.8.11, D.8.24)

fn hello_cmd(principal: Option<&str>, from: Option<(&str, Option<&str>)>) -> Cmd {
    Cmd::Session(SessionCmd::Hello {
        intent: None,
        caps: None,
        horizon: None,
        principal: principal.map(pk),
        from_user: from.map(|(t, p)| FromUserArg {
            text: text(t),
            client: text("ide"),
            principal: p.map(pk),
            answers: None,
            to: None,
            clarifies: None,
            reason: None,
        }),
    })
}

fn user_entry(d: &crate::reply::Done) -> UserEntryReply {
    match &d.reply {
        Reply::Hello {
            user_entry: Some(u),
            ..
        } => u.clone(),
        other => panic!("no user entry: {other:?}"),
    }
}

/// `hello {principal, from_user}` logs the message first in the call that opens the session, with
/// `hello`'s principal unless `from_user` names its own; the entry is unrouted with no hold. On an
/// existing session that holds one node, the message routes to it; the events come after
/// `SessionHello` and before `InboxRead`.
#[test]
fn hello_logs_and_routes_its_from_user() {
    let mut w = world_with(|_| {});
    let ac = class("assistant");
    let ctx = w.ctx(&ac, SessionCtx::New, None);
    let d = ok(w.call_ctx(
        &ctx,
        &hello_cmd(Some("mail:alice@x"), Some(("fix it", None))),
    ));
    let a = d.opened.unwrap();
    let u = user_entry(&d);
    let alice = pid(&w, "mail:alice@x");
    assert_eq!(u.principal, alice);
    assert_eq!(
        u.routed,
        RoutedReply::Unrouted(UnroutedWhy::NoTarget { holds: 0 })
    );
    assert_eq!(u.key, KeyStr::of_user_entry(u.entry));
    assert_eq!(w.s.user_entries[&u.entry].session, a);
    let kinds: Vec<&'static str> = evs(&w)
        .iter()
        .map(|e| match e {
            Ev::PrincipalSeen { .. } => "seen",
            Ev::SessionOpened { .. } => "opened",
            Ev::Appended {
                body: EntryBody::FromUser { .. },
                ..
            } => "from_user",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, vec!["seen", "opened", "from_user"]);
    // `from_user.principal` wins over `hello`'s.
    let ctx = w.ctx(&ac, SessionCtx::New, None);
    let d = ok(w.call_ctx(
        &ctx,
        &hello_cmd(Some("mail:alice@x"), Some(("for bob", Some("mail:bob@x")))),
    ));
    assert_eq!(user_entry(&d).principal, pid(&w, "mail:bob@x"));
    // An existing session with one hold: the message routes there, after `SessionHello`.
    let (n, _) = created(&ok(w.call(&ac, a, create_with("/n", |_| {}))));
    let d = ok(w.call(&ac, a, hello_cmd(None, Some(("and this", None)))));
    let u = user_entry(&d);
    assert!(matches!(u.routed, RoutedReply::Request { node, .. } if node == n));
    assert_eq!(u.principal, alice);
    let order: Vec<usize> = evs(&w)
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            Ev::SessionHello { .. }
            | Ev::Appended {
                body: EntryBody::FromUser { .. },
                ..
            } => Some(i),
            _ => None,
        })
        .collect();
    assert_eq!(order.len(), 2);
    assert!(matches!(evs(&w)[order[0]], Ev::SessionHello { .. }));
    // A session that serves two persons must name the principal.
    ok(w.call(
        &ac,
        a,
        from_user_cmd("hi", Some("mail:carol@x"), None, None, None),
    ));
    refused(
        &mut w,
        &ac,
        a,
        hello_cmd(None, Some(("who?", None))),
        inv(Why::AmbiguousPrincipal),
    );
}

/// On a session that serves alice, `hello {principal: bob, from_user}` without its own principal
/// logs bob's words: `from_user.principal`, else `hello`'s own `principal`, else the session's
/// (D.8.11, rule 1).
#[test]
fn hello_principal_names_its_from_user() {
    let (mut w, _, _, ac, a) = assistant();
    let d = ok(w.call(
        &ac,
        a,
        hello_cmd(Some("mail:bob@x"), Some(("from bob", None))),
    ));
    let u = user_entry(&d);
    let bob = pid(&w, "mail:bob@x");
    assert_eq!(u.principal, bob);
    assert_eq!(w.s.user_entries[&u.entry].principal, bob);
    assert_eq!(w.s.entries[&u.entry].principal, Some(bob));
}

/// 1.9 (§8.1, §21): a `from_user` with no principal, through `talk from_user` and `hello
/// {from_user}`, in a session that serves none is `invalid(NoPrincipal)`, and appends nothing but
/// the session a first `hello` opens (1.5). In the lab, every human message logged at `hello` came
/// with a principal, and every scheduler's prompt logged as one came without.
#[test]
fn a_from_user_with_no_principal_in_a_session_that_serves_none_is_refused() {
    let mut w = world_with(|_| {});
    let bot = class("bot");
    let x = w.hello(&bot);
    refused(
        &mut w,
        &bot,
        x,
        from_user_cmd("The scheduler started you.", None, None, None, None),
        inv(Why::NoPrincipal),
    );
    assert!(w.s.user_entries.is_empty());
    assert!(w.s.sessions[&x].principals.is_empty());
    // A first `hello` with such a `from_user` still opens its session, and logs nothing.
    let before = w.s.sessions.len();
    let ctx = w.ctx(&bot, SessionCtx::New, None);
    assert_eq!(
        w.call_ctx(&ctx, &hello_cmd(None, Some(("run the job", None))))
            .unwrap_err(),
        inv(Why::NoPrincipal)
    );
    assert_eq!(w.s.sessions.len(), before + 1, "the failed hello opened it");
    assert!(w.s.user_entries.is_empty());
    assert_eq!(w.replay(), w.s);
    // A session that serves a principal speaks for it without naming it (§8.1, case 3).
    let (mut w, _, _, ac, a) = assistant();
    let d = ok(w.call(&ac, a, from_user_cmd("hi", None, None, None, None)));
    let (e, _) = routed(&d);
    assert_eq!(w.s.user_entries[&e].principal, pid(&w, "mail:alice@x"));
}

/// 1.6, 1.9 (§8.1, §21): `principal: user` files the words for `user`, the person the client does
/// not name, and warns `unnamed_user` in a session that did not serve `user` before the call,
/// through `talk from_user` and `hello`; the session then serves `user`, so its next `from_user`
/// needs no principal and does not warn, and a named principal never does.
#[test]
fn principal_user_warns_unnamed_user_once() {
    let unnamed = |d: &crate::reply::Done| {
        d.warnings
            .iter()
            .filter(|x| x.code == WarningCode::UnnamedUser)
            .map(|x| x.detail.clone())
            .collect::<Vec<_>>()
    };
    let mut w = world_with(|_| {});
    let ide = class("ide");
    let x = w.hello(&ide);
    let d = ok(w.call(&ide, x, from_user_cmd("hi", Some("user"), None, None, None)));
    let (e, _) = routed(&d);
    assert_eq!(unnamed(&d), vec![WarningDetail::UnnamedUser { entry: e }]);
    assert_eq!(w.s.user_entries[&e].principal, USER_PRINCIPAL);
    assert!(w.s.sessions[&x].principals.contains(&USER_PRINCIPAL));
    // The session now serves `user`: no principal needed, and no warning, named or not.
    let d = ok(w.call(&ide, x, from_user_cmd("more", None, None, None, None)));
    assert!(unnamed(&d).is_empty());
    let d = ok(w.call(
        &ide,
        x,
        from_user_cmd("again", Some("user"), None, None, None),
    ));
    assert!(unnamed(&d).is_empty());
    // A named principal never warns.
    let y = w.hello(&ide);
    let d = ok(w.call(
        &ide,
        y,
        from_user_cmd("hi", Some("mail:a@x"), None, None, None),
    ));
    assert!(unnamed(&d).is_empty());
    // `hello {principal: user, from_user}` in a new session warns; with a named principal, not.
    let ctx = w.ctx(&ide, SessionCtx::New, None);
    let d = ok(w.call_ctx(&ctx, &hello_cmd(Some("user"), Some(("run the job", None)))));
    let u = user_entry(&d);
    assert_eq!(
        unnamed(&d),
        vec![WarningDetail::UnnamedUser { entry: u.entry }]
    );
    assert_eq!(u.principal, USER_PRINCIPAL);
    let ctx = w.ctx(&ide, SessionCtx::New, None);
    let d = ok(w.call_ctx(&ctx, &hello_cmd(Some("mail:a@x"), Some(("fix it", None)))));
    assert!(unnamed(&d).is_empty());
}

/// 1.9 (§8.1, §21): a `from_user` whose text starts with `# AGENTS.md instructions`, or carries
/// `<environment_context>` or `<INSTRUCTIONS>`, is recorded as given and warns `client_context`;
/// other words do not. In the lab, a Codex agent logged its client's instruction file and
/// environment block as its human's words.
#[test]
fn a_client_s_injected_text_warns_client_context() {
    let context = |d: &crate::reply::Done| {
        d.warnings
            .iter()
            .filter(|x| x.code == WarningCode::ClientContext)
            .map(|x| x.detail.clone())
            .collect::<Vec<_>>()
    };
    let (mut w, _, _, ac, a) = assistant();
    for t in [
        "# AGENTS.md instructions\n\n<INSTRUCTIONS>\nBe brief.\n</INSTRUCTIONS>",
        "  # AGENTS.md instructions for /repo",
        "fix the build <environment_context><cwd>/tmp</cwd></environment_context>",
        "<INSTRUCTIONS>use tabs</INSTRUCTIONS>",
    ] {
        let d = ok(w.call(&ac, a, from_user_cmd(t, None, None, None, None)));
        let (e, _) = routed(&d);
        assert_eq!(
            context(&d),
            vec![WarningDetail::ClientContext { entry: e }],
            "{t}"
        );
        assert_eq!(w.s.user_entries[&e].principal, pid(&w, "mail:alice@x"));
    }
    for t in [
        "please read AGENTS.md first",
        "the # AGENTS.md instructions heading is odd",
        "fix the build",
    ] {
        let d = ok(w.call(&ac, a, from_user_cmd(t, None, None, None, None)));
        assert!(context(&d).is_empty(), "{t}");
    }
    // Through `hello` too.
    let ctx = w.ctx(&ac, SessionCtx::New, None);
    let d = ok(w.call_ctx(
        &ctx,
        &hello_cmd(
            Some("mail:alice@x"),
            Some(("# AGENTS.md instructions\n<environment_context/>", None)),
        ),
    ));
    let u = user_entry(&d);
    assert_eq!(
        context(&d),
        vec![WarningDetail::ClientContext { entry: u.entry }]
    );
}

/// 1.6 (§8.2, l2, l3: agents logged the same words twice): `hello {from_user: {answers}}` routes
/// the entry as the answer to that question for the human, as `talk from_user {answers}` does; a
/// question that does not exist is `not_found`, and the first `hello` still opens its session.
#[test]
fn hello_from_user_answers_a_question_for_the_human() {
    let (mut w, c, s, ac, _) = assistant();
    let n = mk(&mut w, &c, s, "/n", |_| {});
    let q = asked(&ok(w.call(&c, s, ask_cmd(n, &Ask::default())))).0;
    let answering = |answers: Eid| {
        let mut cmd = hello_cmd(Some("mail:alice@x"), Some(("yes, go ahead", None)));
        if let Cmd::Session(SessionCmd::Hello {
            from_user: Some(f), ..
        }) = &mut cmd
        {
            f.answers = Some(answers);
        }
        cmd
    };
    let ctx = w.ctx(&ac, SessionCtx::New, None);
    let d = ok(w.call_ctx(&ctx, &answering(q)));
    let u = user_entry(&d);
    assert_eq!(u.routed, RoutedReply::Answers { question: q });
    assert_eq!(w.s.uquestions[&q].state, UqState::Answered { by: u.entry });
    assert_eq!(entry_routing(&w, u.entry), Routing::Answers { question: q });
    let sessions = w.s.sessions.len();
    let ctx = w.ctx(&ac, SessionCtx::New, None);
    assert_eq!(
        w.call_ctx(&ctx, &answering(Eid(9_999))).unwrap_err(),
        nf(What::Question)
    );
    assert_eq!(
        w.s.sessions.len(),
        sessions + 1,
        "the failed hello opened it"
    );
    assert_eq!(w.replay(), w.s);
}

// ------------------------------------------------------------------------------------------------
// route (D.8.11)

/// `route`'s refusals in order, then each routing: to a node (a request there), to none (no action,
/// with its reason), and an entry already routed (`{existing}` with its first routing).
#[test]
fn route_routes_once_and_returns_the_first_routing() {
    let (mut w, c, s, ac, a) = assistant();
    let n = mk(&mut w, &c, s, "/n", |_| {});
    let e = routed(&ok(w.call(
        &ac,
        a,
        from_user_cmd("hi", None, None, None, None),
    )))
    .0;
    let e2 = routed(&ok(w.call(
        &ac,
        a,
        from_user_cmd("thanks", None, None, None, None),
    )))
    .0;
    let r = ext_request(&mut w, s, n, "mail:m1");
    let cases: Vec<(Cmd, Error)> = vec![
        (
            route_cmd(Eid(9_999), Some(NodeId(999)), "r"),
            nf(What::Node),
        ),
        (route_cmd(Eid(9_999), Some(n), "r"), nf(What::UserEntry)),
        (route_cmd(Eid(0), None, "r"), nf(What::UserEntry)),
        (route_cmd(r, Some(n), ""), inv(Why::NotUserEntry)),
        (
            route_cmd(e, Some(n), ""),
            inv(Why::EmptyText(Field::Reason)),
        ),
        // An empty reason before a reserved target (D.8.24's row: `EmptyText`, then
        // `ReservedNode`).
        (
            route_cmd(e, Some(USER), ""),
            inv(Why::EmptyText(Field::Reason)),
        ),
        (route_cmd(e, Some(USER), "r"), inv(Why::ReservedNode)),
    ];
    for (cmd, want) in cases {
        refused(&mut w, &c, s, cmd, want);
    }
    let d = ok(w.call(&c, s, route_cmd(e, Some(n), "a bug report")));
    let Reply::Routed {
        entry,
        routed: first,
        existing,
    } = d.reply
    else {
        panic!("{:?}", d.reply);
    };
    assert_eq!((entry, existing), (e, false));
    let RoutedReply::Request { request, node, .. } = first else {
        panic!("{first:?}");
    };
    assert_eq!(node, n);
    assert_eq!(
        w.s.requests[&request].requester,
        Requester::User(pid(&w, "mail:alice@x"))
    );
    assert!(!w.s.ix.unrouted.contains(&e));
    // Again: the first routing wins, and nothing is appended.
    let d = ok(w.call(&c, s, route_cmd(e, None, "never mind")));
    assert_eq!(
        d.reply,
        Reply::Routed {
            entry: e,
            routed: first,
            existing: true
        }
    );
    assert!(
        !evs(&w)
            .iter()
            .any(|x| matches!(x, Ev::EntryRouted { .. } | Ev::Appended { .. }))
    );
    // To none: no action, with the reason.
    let d = ok(w.call(&c, s, route_cmd(e2, None, "a thank-you")));
    assert!(matches!(
        d.reply,
        Reply::Routed {
            routed: RoutedReply::NoAction,
            existing: false,
            ..
        }
    ));
    assert!(evs(&w).iter().any(|x| matches!(x, Ev::EntryRouted {
        routing: Routing::NoAction,
        reason: Some(t),
        ..
    } if t.0 == "a thank-you")));
    let d = ok(w.call(&c, s, route_cmd(e2, Some(n), "after all")));
    assert!(matches!(
        d.reply,
        Reply::Routed {
            routed: RoutedReply::NoAction,
            existing: true,
            ..
        }
    ));
    assert!(w.s.ix.unrouted.is_empty());
    assert!(!w.s.ix.node[&USER].flags.contains(Flag::Unrouted));
}

// ------------------------------------------------------------------------------------------------
// to_user (D.8.11, D.8.8, CS-4, CS-5, A-23, R34)

/// `to_user`'s refusals: an id at or above `next_eid` (or 0) is `not_found(Entry)`, before an entry
/// of another kind (a user question, a `said`, a decision), which is `invalid(NotReportable)`.
#[test]
fn to_user_refusals_in_order() {
    let (mut w, c, s, ac, a) = assistant();
    let n = mk(&mut w, &c, s, "/n", |a| {
        a.keys = vec![crate::helpers::user::tests::key("chat:t/1")];
    });
    let q = asked(&ok(w.call(&c, s, ask_cmd(n, &Ask::default())))).0;
    let pq = people_question(&mut w, s, n, "chat:t/1", false);
    let dec = decision(&mut w, s, n, &[]);
    for (re, want) in [
        (vec![Eid(9_999)], nf(What::Entry)),
        (vec![Eid(0)], nf(What::Entry)),
        (vec![q, Eid(9_999)], nf(What::Entry)),
        (vec![q], inv(Why::NotReportable)),
        (vec![pq], inv(Why::NotReportable)),
        (vec![dec], inv(Why::NotReportable)),
    ] {
        refused(
            &mut w,
            &ac,
            a,
            to_user_cmd(ToUserKind::Result, None, &re),
            want,
        );
    }
}

/// Only a `result` to the requester, or to another alias of the same person, answers a user request
/// (A-23, CS-4): a report to alice naming bob's request answers nothing (trace A) and warns; an `ack`
/// or a `question` to bob answers nothing, without the warning, but shows the digest entries it
/// names; a `result` to bob's other alias answers, and settles with the decision recorded before. A
/// later `result` to bob about the answered request warns only in §21's three cases (R34).
#[test]
fn only_a_result_to_the_requester_answers_a_user_request() {
    let Dir { mut w, c, s, .. } = dir();
    let ac = class("assistant");
    let a = open_as(&mut w, &ac, Some("mail:alice@x"));
    let b = open_as(&mut w, &class("bob-bot"), Some("chat:bob"));
    let n = mk(&mut w, &c, s, "/n", |_| {});
    let (_, r) = user_request(&mut w, &class("bob-bot"), b, n, "mail:bob@x");
    decision(&mut w, s, n, &[r]);
    // To alice: recorded, answers nothing, warns with the request.
    let d = ok(w.call(&ac, a, to_user_cmd(ToUserKind::Result, None, &[r])));
    assert!(matches!(&d.reply, Reply::ToUser { settled, .. } if settled.is_empty()));
    assert_eq!(w.s.requests[&r].answered_by, None);
    let warn = d
        .warnings
        .iter()
        .find(|x| x.code == WarningCode::ResultAnsweredNothing)
        .unwrap();
    assert_eq!(
        warn.detail,
        WarningDetail::ResultAnsweredNothing { requests: vec![r] }
    );
    // An ack and a question to bob answer nothing and do not warn.
    for kind in [ToUserKind::Ack, ToUserKind::Question] {
        let d = ok(w.call(&ac, a, to_user_cmd(kind, Some("mail:bob@x"), &[r])));
        assert!(d.warnings.is_empty(), "{:?}", d.warnings);
        assert_eq!(w.s.requests[&r].answered_by, None);
    }
    // A result to bob's other alias answers and settles it.
    let d = ok(w.call(
        &ac,
        a,
        to_user_cmd(ToUserKind::Result, Some("chat:bob"), &[r]),
    ));
    let Reply::ToUser { entry, settled, .. } = &d.reply else {
        panic!("{:?}", d.reply);
    };
    assert_eq!(settled, &vec![r]);
    assert_eq!(w.s.requests[&r].answered_by, Some(*entry));
    assert!(d.warnings.is_empty());
    // The session told alice, then bob's aliases (rule 12, R13).
    assert!(w.s.sessions[&a].told.contains(&pid(&w, "mail:alice@x")));
    assert!(w.s.sessions[&a].told.contains(&pid(&w, "chat:bob")));
    // A later result to bob naming his answered request records a report and answers nothing new,
    // but it is not one of §21's three cases (none named, another principal's request, a requester
    // outside the swarm): it does not warn, and the first answer stands.
    let d = ok(w.call(
        &ac,
        a,
        to_user_cmd(ToUserKind::Result, Some("mail:bob@x"), &[r]),
    ));
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    assert!(matches!(&d.reply, Reply::ToUser { settled, .. } if settled.is_empty()));
    assert_eq!(w.s.requests[&r].answered_by, Some(*entry));
    assert!(
        !evs(&w)
            .iter()
            .any(|x| matches!(x, Ev::RequestAnswered { .. }))
    );
    // Named beside bob's answered request, alice's own does not change that; alone, a request of
    // another person warns, with the requests named.
    let (_, ra) = user_request(&mut w, &ac, a, n, "mail:alice@x");
    let d = ok(w.call(
        &ac,
        a,
        to_user_cmd(ToUserKind::Result, Some("chat:bob"), &[r, ra]),
    ));
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    assert_eq!(w.s.requests[&ra].answered_by, None);
    let d = ok(w.call(
        &ac,
        a,
        to_user_cmd(ToUserKind::Result, Some("chat:bob"), &[ra]),
    ));
    assert_eq!(
        d.warnings.first().map(|x| x.detail.clone()),
        Some(WarningDetail::ResultAnsweredNothing { requests: vec![ra] })
    );
}

/// A `result` naming the user entry that was routed as a request answers that request; one that
/// names no request, or a request from outside the swarm, warns `result_answered_nothing`.
#[test]
fn a_result_answers_through_its_entry_and_warns_when_it_answered_nothing() {
    let (mut w, c, s, ac, a) = assistant();
    let n = mk(&mut w, &c, s, "/n", |_| {});
    let (e, r) = user_request(&mut w, &ac, a, n, "mail:alice@x");
    let d = ok(w.call(&ac, a, to_user_cmd(ToUserKind::Result, None, &[e])));
    assert!(d.warnings.is_empty());
    let a_eid = match d.reply {
        Reply::ToUser { entry, .. } => entry,
        ref other => panic!("{other:?}"),
    };
    assert_eq!(w.s.requests[&r].answered_by, Some(a_eid));
    // Nothing named.
    let d = ok(w.call(&ac, a, to_user_cmd(ToUserKind::Result, None, &[])));
    assert_eq!(
        d.warnings.first().map(|x| x.detail.clone()),
        Some(WarningDetail::ResultAnsweredNothing { requests: vec![] })
    );
    // A request from outside the swarm is answered by a `said` in its conversation.
    let ext = ext_request(&mut w, s, n, "mail:m1");
    let d = ok(w.call(&ac, a, to_user_cmd(ToUserKind::Result, None, &[ext])));
    assert_eq!(
        d.warnings.first().map(|x| x.detail.clone()),
        Some(WarningDetail::ResultAnsweredNothing {
            requests: vec![ext]
        })
    );
    assert_eq!(w.s.requests[&ext].answered_by, None);
    // An ack never warns.
    let d = ok(w.call(&ac, a, to_user_cmd(ToUserKind::Ack, None, &[])));
    assert!(d.warnings.is_empty());
}

/// Rule 5's reply to an entry that needs no request (1.5, §21, §32 step 8): a `result` to the
/// entry's person, under either alias, that names the entry in `re` replies to it and does not warn,
/// before and after the entry is routed to none; the same report to another person still warns.
#[test]
fn a_result_that_replies_to_its_persons_entry_does_not_warn() {
    let Dir { mut w, c, s, .. } = dir();
    let bc = class("bob-bot");
    let b = open_as(&mut w, &bc, Some("mail:bob@x"));
    let e = routed(&ok(w.call(
        &bc,
        b,
        from_user_cmd("what about my tasks?", None, None, None, None),
    )))
    .0;
    assert_eq!(entry_routing(&w, e), Routing::Unrouted);
    for to in [None, Some("chat:bob")] {
        let d = ok(w.call(&bc, b, to_user_cmd(ToUserKind::Result, to, &[e])));
        assert!(d.warnings.is_empty(), "{to:?}: {:?}", d.warnings);
        assert!(matches!(&d.reply, Reply::ToUser { settled, .. } if settled.is_empty()));
    }
    // To alice, bob's entry is not hers: the report answered nothing, and named no request.
    let d = ok(w.call(
        &bc,
        b,
        to_user_cmd(ToUserKind::Result, Some("mail:alice@x"), &[e]),
    ));
    assert_eq!(
        d.warnings.first().map(|x| x.detail.clone()),
        Some(WarningDetail::ResultAnsweredNothing { requests: vec![] })
    );
    // Routed to none, the entry is still bob's.
    ok(w.call(&c, s, route_cmd(e, None, "answered from the board")));
    assert_eq!(entry_routing(&w, e), Routing::NoAction);
    let d = ok(w.call(
        &bc,
        b,
        to_user_cmd(ToUserKind::Result, Some("chat:bob"), &[e]),
    ));
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
}

/// A `to_user` of any kind shows the pending digest entries it names that are addressed to its
/// principal or an alias of it; an entry for carol named in a report to alice stays pending
/// (trace B).
#[test]
fn a_report_shows_only_its_principals_digest_entries() {
    let Dir { mut w, c, s, .. } = dir();
    let p = w.s.params.clone();
    let n = mk(&mut w, &c, s, "/n", |_| {});
    let q_bob = asked(&ok(w.call(
        &c,
        s,
        ask_cmd(
            n,
            &Ask {
                to: Some("chat:bob"),
                default: Some("no"),
                ..Ask::default()
            },
        ),
    )))
    .0;
    let q_carol = asked(&ok(w.call(
        &c,
        s,
        ask_cmd(
            n,
            &Ask {
                to: Some("mail:carol@x"),
                default: Some("no"),
                ..Ask::default()
            },
        ),
    )))
    .0;
    // Both absent: their questions default after `due_absent`, each with a digest entry.
    w.advance(p.user_question_due_absent);
    let d_bob =
        *w.s.digest
            .iter()
            .find(|(_, x)| x.item == DigestItem::DefaultApplied { question: q_bob })
            .unwrap()
            .0;
    let d_carol =
        *w.s.digest
            .iter()
            .find(|(_, x)| x.item == DigestItem::DefaultApplied { question: q_carol })
            .unwrap()
            .0;
    let ac = class("assistant");
    let a = open_as(&mut w, &ac, Some("mail:bob@x"));
    let d = ok(w.call(
        &ac,
        a,
        to_user_cmd(ToUserKind::Ack, None, &[d_bob, d_carol]),
    ));
    assert!(matches!(&d.reply, Reply::ToUser { shown_digest, .. } if *shown_digest == vec![d_bob]));
    assert!(matches!(
        w.s.digest.get(&d_bob).map(|x| x.state),
        Some(DigestState::Shown { .. }) | None
    ));
    assert_eq!(w.s.digest[&d_carol].state, DigestState::Pending);
    assert!(w.s.ix.pending_digest_by[&pid(&w, "mail:carol@x")].contains(&d_carol));
}

/// An entry is shown once: a later report to the same person that names it again records the
/// report, shows nothing, and leaves the entry shown by the first (§8.5, D.8.11: each pending entry
/// in `re`).
#[test]
fn a_shown_digest_entry_is_not_shown_again() {
    let Dir { mut w, c, s, .. } = dir();
    let n = mk(&mut w, &c, s, "/n", |_| {});
    let q = asked(&ok(w.call(
        &c,
        s,
        ask_cmd(
            n,
            &Ask {
                to: Some("chat:bob"),
                default: Some("no"),
                ..Ask::default()
            },
        ),
    )))
    .0;
    let due = w.s.params.user_question_due_absent;
    w.advance(due);
    let d =
        *w.s.digest
            .iter()
            .find(|(_, x)| x.item == DigestItem::DefaultApplied { question: q })
            .unwrap()
            .0;
    let ac = class("assistant");
    let a = open_as(&mut w, &ac, Some("mail:bob@x"));
    let first = ok(w.call(&ac, a, to_user_cmd(ToUserKind::Ack, None, &[d])));
    assert!(matches!(&first.reply, Reply::ToUser { shown_digest, .. } if *shown_digest == vec![d]));
    let shown = w.s.digest[&d].state;
    assert!(matches!(shown, DigestState::Shown { .. }));
    let again = ok(w.call(&ac, a, to_user_cmd(ToUserKind::Result, None, &[d])));
    assert!(matches!(&again.reply, Reply::ToUser { shown_digest, .. } if shown_digest.is_empty()));
    assert!(!evs(&w).iter().any(|e| matches!(e, Ev::DigestShown { .. })));
    assert_eq!(w.s.digest[&d].state, shown);
}

// ------------------------------------------------------------------------------------------------
// ask to a human (D.8.12, A-16, CS-2, CS-33, R27)

/// The text of question `q`'s entry.
fn question_text(w: &World, q: QuestionId) -> crate::event::EntryText {
    w.log
        .iter()
        .find_map(|e| match &e.ev {
            Ev::Appended { eid, text, .. } if *eid == q => Some(text.clone()),
            _ => None,
        })
        .unwrap()
}

/// A question for a human is recorded on `@user/decisions` with its chain, options,
/// recommendation, and default; `options: []` is an open question (R27); the same question pending
/// for the same first addressee is returned as `{existing}` with nothing appended, unless `again`,
/// another digest, or another first addressee.
#[test]
fn ask_records_a_question_once_per_first_addressee() {
    let Dir { mut w, c, s, .. } = dir();
    let n = mk(&mut w, &c, s, "/n", |_| {});
    let d = ok(w.call(
        &c,
        s,
        ask_cmd(
            n,
            &Ask {
                default: Some("no"),
                ..Ask::default()
            },
        ),
    ));
    let (q, existing) = asked(&d);
    assert!(!existing);
    assert!(matches!(
        d.reply,
        Reply::Asked {
            said: None,
            node: USER_DECISIONS,
            ..
        }
    ));
    let u = &w.s.uquestions[&q];
    assert_eq!(u.to, vec![USER_PRINCIPAL]);
    assert_eq!(u.about, Some(n));
    assert!(u.has_default);
    assert!(matches!(u.source, UqSource::Agent { session, .. } if session == s));
    let t = question_text(&w, q);
    assert_eq!(t.main.0, "which?");
    let fields: Vec<(TextField, &str)> = t.extra.iter().map(|(f, x)| (*f, x.0.as_str())).collect();
    assert_eq!(
        fields,
        vec![
            (TextField::Option, "yes"),
            (TextField::Option, "no"),
            (TextField::Recommendation, "yes"),
            (TextField::Default, "no"),
        ]
    );
    // The same question again: `{existing}`, nothing appended.
    let d = ok(w.call(
        &c,
        s,
        ask_cmd(
            n,
            &Ask {
                default: Some("no"),
                ..Ask::default()
            },
        ),
    ));
    assert_eq!(asked(&d), (q, true));
    assert!(!evs(&w).iter().any(|e| matches!(e, Ev::Appended { .. })));
    // `again`, another digest, another first addressee: new questions.
    let again = asked(&ok(w.call(
        &c,
        s,
        ask_cmd(
            n,
            &Ask {
                again: true,
                ..Ask::default()
            },
        ),
    )));
    assert!(!again.1 && again.0 != q);
    let other = asked(&ok(w.call(
        &c,
        s,
        ask_cmd(
            n,
            &Ask {
                digest: 7,
                ..Ask::default()
            },
        ),
    )));
    assert!(!other.1);
    let to_bob = asked(&ok(w.call(
        &c,
        s,
        ask_cmd(
            n,
            &Ask {
                to: Some("mail:bob@x"),
                ..Ask::default()
            },
        ),
    )));
    assert!(!to_bob.1);
    assert_eq!(
        w.s.uquestions[&to_bob.0].to,
        vec![pid(&w, "mail:bob@x"), USER_PRINCIPAL]
    );
    // Bob's other alias is the same first addressee: `{existing}`.
    let d = ok(w.call(
        &c,
        s,
        ask_cmd(
            n,
            &Ask {
                to: Some("chat:bob"),
                ..Ask::default()
            },
        ),
    ));
    assert_eq!(asked(&d), (to_bob.0, true));
    // An open question: no options, no recommendation.
    let open = asked(&ok(w.call(
        &c,
        s,
        ask_cmd(
            n,
            &Ask {
                options: vec![],
                digest: 8,
                ..Ask::default()
            },
        ),
    )));
    assert!(question_text(&w, open.0).extra.is_empty());
    assert_eq!(w.s.uquestions[&open.0].state, UqState::Pending);
    // An unknown node.
    refused(
        &mut w,
        &c,
        s,
        ask_cmd(NodeId(999), &Ask::default()),
        nf(What::Node),
    );
}

/// `ask`'s `{existing}` stands only for a pending question an agent asked (D.8.12: source `Agent`
/// with the same digest): a pending refusal question or escalation question about the node, for
/// the same first addressee, leaves a new ask to record its own question.
#[test]
fn ask_existing_matches_only_an_agents_question() {
    let mut w = world_with(|p| p.refusal_quorum = 1);
    let c = class("worker");
    let s = w.hello(&c);
    let (n, t) = created(&ok(w.call(
        &c,
        s,
        create_with("/n", |a| {
            a.keys = vec![crate::helpers::user::tests::key("chat:t/1")];
        }),
    )));
    ok(w.call(
        &c,
        s,
        crate::apply::tests::release(t.unwrap(), ReleaseReason::Cannot),
    ));
    let refusal = w.s.ix.refusal_question[&n];
    assert_eq!(w.s.uquestions[&refusal].to, vec![USER_PRINCIPAL]);
    // An escalation's question about the same node, by its events (the Recheck arm is WP-D5b's).
    let pq = people_question(&mut w, s, n, "chat:t/1", true);
    let esc = w.s.counters.next_eid;
    let due = w.clock.plus(HOUR);
    crate::apply::tests::entry(
        &mut w,
        USER_DECISIONS,
        crate::state::Author::Server,
        EntryBody::Question(QuestionBody::User {
            source: UqSource::Escalation { question: pq },
            about: Some(n),
            to: vec![USER_PRINCIPAL],
            due,
            has_default: false,
        }),
    );
    w.emit(Ev::PqEscalated { q: pq, uq: esc });
    assert_eq!(w.s.uquestions[&esc].state, UqState::Pending);
    let d = ok(w.call(&c, s, ask_cmd(n, &Ask::default())));
    let (q, existing) = asked(&d);
    assert!(!existing);
    assert!(q != refusal && q != esc);
    assert!(matches!(w.s.uquestions[&q].source, UqSource::Agent { .. }));
}

/// A new explicit principal is interned by the ask and is its first stage; an explicit `due` sets
/// the first stage's due, and one in the past is clamped to `now`, so the question defaults or
/// moves on in the same call (A-68).
#[test]
fn ask_interns_its_addressee_and_clamps_its_due() {
    let mut w = world_with(|_| {});
    let c = class("worker");
    let s = w.hello(&c);
    let n = mk(&mut w, &c, s, "/n", |_| {});
    let d = ok(w.call(
        &c,
        s,
        ask_cmd(
            n,
            &Ask {
                to: Some("mail:zed@x"),
                due: Some(When::In(Dur(5 * HOUR.0))),
                ..Ask::default()
            },
        ),
    ));
    let q = asked(&d).0;
    let zed = pid(&w, "mail:zed@x");
    assert!(
        evs(&w)
            .iter()
            .any(|e| matches!(e, Ev::PrincipalSeen { id, .. } if *id == zed))
    );
    assert_eq!(w.s.uquestions[&q].to, vec![zed, USER_PRINCIPAL]);
    assert_eq!(w.s.uquestions[&q].due, w.clock.plus(Dur(5 * HOUR.0)));
    // A due in the past: clamped to now, and fired by the call's own settle.
    let past = When::At(Time(T0_MINUS));
    let q = asked(&ok(w.call(
        &c,
        s,
        ask_cmd(
            n,
            &Ask {
                default: Some("no"),
                due: Some(past),
                digest: 3,
                ..Ask::default()
            },
        ),
    )))
    .0;
    assert_eq!(w.s.uquestions[&q].state, UqState::Defaulted);
    let q = asked(&ok(w.call(
        &c,
        s,
        ask_cmd(
            n,
            &Ask {
                due: Some(past),
                digest: 4,
                ..Ask::default()
            },
        ),
    )))
    .0;
    // `[user]`: past its only stage, it lapses at once onto its open `about` node.
    assert_eq!(w.s.uquestions[&q].state, UqState::Lapsed);
    assert!(w.s.nodes[&n].assume_marks.contains(&q));
}

const T0_MINUS: u64 = crate::apply::tests::T0.0 - 1_000;

/// `user_saturated`: no warning with exactly `user_question.max_pending` questions already pending
/// for the first addressee, under any alias; the warning, with the count before this question, from
/// one more; the question is recorded either way (CS-33).
#[test]
fn user_saturated_warns_past_max_pending() {
    let mut w = world_with(|p| p.user_question_max_pending = 2);
    let c = class("worker");
    let s = w.hello(&c);
    let bob =
        crate::helpers::user::tests::person(&mut w, &c, s, "bob", &["mail:bob@x", "chat:bob"]);
    let n = mk(&mut w, &c, s, "/n", |_| {});
    let mut warned = Vec::new();
    for i in 0..4_u8 {
        let to = if i % 2 == 0 { "mail:bob@x" } else { "chat:bob" };
        let d = ok(w.call(
            &c,
            s,
            ask_cmd(
                n,
                &Ask {
                    to: Some(to),
                    digest: 10 + i,
                    ..Ask::default()
                },
            ),
        ));
        assert!(!asked(&d).1);
        // 1.12 (§8.4): no session serves bob, so each question also warns `no_human_in_reach`.
        assert!(
            d.warnings
                .iter()
                .any(|x| x.code == WarningCode::NoHumanInReach)
        );
        warned.push(
            d.warnings
                .iter()
                .filter(|x| x.code != WarningCode::NoHumanInReach)
                .cloned()
                .collect::<Vec<_>>(),
        );
    }
    assert!(warned[0].is_empty() && warned[1].is_empty() && warned[2].is_empty());
    assert_eq!(warned[3].len(), 1);
    let w3 = &warned[3][0];
    assert_eq!(w3.code, WarningCode::UserSaturated);
    assert_eq!(
        w3.detail,
        WarningDetail::UserSaturated {
            principal: pid(&w, "chat:bob"),
            pending: 3
        }
    );
    let _ = bob;
}

/// A worker's token, for the tests that need a hold.
#[allow(dead_code, reason = "kept beside the fixtures it pairs with")]
fn hold_of(w: &mut World, c: &ClassName, s: SessionId, p: &str) -> (NodeId, Token) {
    let d = ok(w.call(c, s, create_with(p, |_| {})));
    (created(&d).0, token_of(&d))
}

/// Silences an unused import in builds that skip some tests.
#[allow(dead_code, reason = "a type the tests name only in patterns")]
fn _types(_: BTreeSet<Eid>) {}

// ------------------------------------------------------------------------------------------------
// 1.8: `to_user {at}` and `no_human_in_reach` (§8.1, §8.4, §21; the lab's third round, l3 F1)

fn to_user_at(kind: ToUserKind, principal: Option<&str>, re: &[Eid], at: NodeId) -> Cmd {
    let mut c = to_user_cmd(kind, principal, re);
    if let Cmd::Talk(TalkCmd::ToUser { at: a, .. }) = &mut c {
        *a = Some(NodeRef::Id(at));
    }
    c
}

/// Without `principal`, a report about a node is to the requester of its oldest unsettled user
/// request, and without `re` it names that person's requests there, which a result answers.
#[test]
fn to_user_at_reports_to_the_node_s_requester() {
    let (mut w, c, s, ac, a) = assistant();
    let n = mk(&mut w, &c, s, "/report", |_| {});
    let (_, r) = user_request(&mut w, &ac, a, n, "mail:alice@x");
    let d = ok(w.call(&c, s, to_user_at(ToUserKind::Result, None, &[], n)));
    let Reply::ToUser {
        entry,
        principal,
        re,
        ..
    } = &d.reply
    else {
        panic!("{:?}", d.reply)
    };
    assert_eq!(*principal, pid(&w, "mail:alice@x"));
    assert_eq!(*re, vec![r]);
    assert_eq!(w.s.requests[&r].answered_by, Some(*entry));
    assert!(d.warnings.iter().all(|x| !matches!(
        x.code,
        WarningCode::IgnoredArguments
            | WarningCode::NoHumanInReach
            | WarningCode::ResultAnsweredNothing
    )));
    // The entry names the request in its `re`, as any report does.
    assert!(evs(&w).iter().any(|e| matches!(e,
        Ev::Appended { body: EntryBody::ToUser { re, .. }, .. } if re.contains(&r))));
    // A named principal of another person: `at` names none of their requests, and is ignored.
    let d = ok(w.call(
        &c,
        s,
        to_user_at(ToUserKind::Ack, Some("mail:bob@x"), &[], n),
    ));
    assert!(matches!(&d.reply, Reply::ToUser { re, .. } if re.is_empty()));
    assert!(
        d.warnings
            .iter()
            .any(|x| x.code == WarningCode::IgnoredArguments
                && x.detail == WarningDetail::AtIgnored { node: n })
    );
}

/// A worker's report about a node with no user request is filed for `user`, `at` ignored; with no
/// live session serving `user`, it warns `no_human_in_reach`, and not once one does.
#[test]
fn a_report_nobody_will_read_warns() {
    let (mut w, c, s, _, _) = assistant();
    let n = mk(&mut w, &c, s, "/report", |_| {});
    let d = ok(w.call(&c, s, to_user_at(ToUserKind::Result, None, &[], n)));
    assert!(matches!(&d.reply, Reply::ToUser { principal, .. } if *principal == USER_PRINCIPAL));
    assert!(
        d.warnings
            .iter()
            .any(|x| x.detail == WarningDetail::AtIgnored { node: n })
    );
    assert!(
        d.warnings
            .iter()
            .any(|x| x.code == WarningCode::NoHumanInReach
                && x.detail
                    == WarningDetail::NoHumanInReach {
                        due: None,
                        default: false
                    })
    );
    // A question for the user alone, asked by the worker, warns too, with its due.
    let d = ok(w.call(&c, s, ask_cmd(n, &Ask::default())));
    let (q, _) = asked(&d);
    let due = w.s.uquestions[&q].due;
    assert!(d.warnings.iter().any(|x| x.detail
        == WarningDetail::NoHumanInReach {
            due: Some(due),
            default: false
        }));
    // A client that names nobody serves `user` (1.9: with `principal: user`): then someone reads
    // it.
    let ide = class("ide");
    let x = w.hello(&ide);
    ok(w.call(&ide, x, from_user_cmd("hi", Some("user"), None, None, None)));
    let d = ok(w.call(&c, s, to_user_cmd(ToUserKind::Result, None, &[])));
    assert!(
        d.warnings
            .iter()
            .all(|x| x.code != WarningCode::NoHumanInReach)
    );
    // A session that serves a principal never gets it.
    let (mut w, c, s, ac, a) = assistant();
    let n = mk(&mut w, &c, s, "/report", |_| {});
    let d = ok(w.call(&ac, a, to_user_at(ToUserKind::Ack, Some("user"), &[], n)));
    assert!(
        d.warnings
            .iter()
            .all(|x| x.code != WarningCode::NoHumanInReach)
    );
}

// ------------------------------------------------------------------------------------------------
// 1.9: a result that routes nothing (§8.2, the lab's fourth round, l2 F1), and one with neither
// `re` nor `at` (§8.1, l6 F4)

/// A `result` that names an unrouted entry of its person replies to it and leaves it unrouted:
/// only the agent knows whether the report answered all of the words (1.7 routed it to none).
#[test]
fn a_result_leaves_the_entry_it_names_unrouted() {
    let Dir { mut w, .. } = dir();
    let bc = class("bob-bot");
    let b = open_as(&mut w, &bc, Some("mail:bob@x"));
    let e = routed(&ok(w.call(
        &bc,
        b,
        from_user_cmd(
            "what did we do about the crash? it crashes again",
            None,
            None,
            None,
            None,
        ),
    )))
    .0;
    let d = ok(w.call(&bc, b, to_user_cmd(ToUserKind::Result, None, &[e])));
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    assert_eq!(entry_routing(&w, e), Routing::Unrouted);
    assert!(!evs(&w).iter().any(|x| matches!(x, Ev::EntryRouted { .. })));
    assert!(w.s.ix.unrouted.contains(&e));
}

/// A `result` with neither `re` nor `at`, from a session that serves its principal, names that
/// person's unsettled user requests on the nodes the session holds, oldest first and at most 5,
/// and answers them: the response lists them in `re`, and nothing warns. Another person's request
/// there, and a request of the same person on a node the session does not hold, are not named. An
/// `ack` names nothing, and a worker's result, which serves nobody, still answers nothing.
#[test]
fn a_result_without_re_names_its_persons_requests_on_held_nodes() {
    let (mut w, c, s, ac, a) = assistant();
    let n = created(&ok(w.call(&ac, a, create_with("/incident", |_| {}))));
    let n = n.0;
    assert!(w.s.holds.iter().any(|(t, h)| t.node == n && h.session == a));
    let mut mine: Vec<RequestId> = Vec::new();
    for _ in 0..6 {
        mine.push(user_request(&mut w, &ac, a, n, "mail:alice@x").1);
    }
    let bc = class("bob-bot");
    let b = open_as(&mut w, &bc, Some("mail:bob@x"));
    let (_, rb) = user_request(&mut w, &bc, b, n, "mail:bob@x");
    let m = mk(&mut w, &c, s, "/elsewhere", |_| {});
    let (_, rm) = user_request(&mut w, &ac, a, m, "mail:alice@x");
    // An ack names nothing.
    let d = ok(w.call(&ac, a, to_user_cmd(ToUserKind::Ack, None, &[])));
    assert!(matches!(&d.reply, Reply::ToUser { re, .. } if re.is_empty()));
    // The result names the 5 oldest of alice's requests on the held node, and answers them.
    let d = ok(w.call(&ac, a, to_user_cmd(ToUserKind::Result, None, &[])));
    let Reply::ToUser { entry, re, .. } = &d.reply else {
        panic!("{:?}", d.reply)
    };
    let five: Vec<RequestId> = mine.iter().copied().take(5).collect();
    assert_eq!(*re, five);
    for r in &five {
        assert_eq!(w.s.requests[r].answered_by, Some(*entry));
    }
    assert_eq!(w.s.requests[&mine[5]].answered_by, None);
    assert_eq!(w.s.requests[&rb].answered_by, None);
    assert_eq!(w.s.requests[&rm].answered_by, None);
    assert!(
        d.warnings
            .iter()
            .all(|x| x.code != WarningCode::ResultAnsweredNothing),
        "{:?}",
        d.warnings
    );
    assert!(evs(&w).iter().any(|e| matches!(e,
        Ev::Appended { body: EntryBody::ToUser { re, .. }, .. } if *re == five.iter().copied().collect())));
    // A worker serves nobody: its result names nothing, and warns as before.
    let d = ok(w.call(&c, s, to_user_cmd(ToUserKind::Result, None, &[])));
    assert!(matches!(&d.reply, Reply::ToUser { re, .. } if re.is_empty()));
    assert!(
        d.warnings
            .iter()
            .any(|x| x.code == WarningCode::ResultAnsweredNothing)
    );
}

// ------------------------------------------------------------------------------------------------
// 1.11 (§8.2): a follow-up clarifies by default

/// A `from_user` with no target, from a session whose only hold carries an unsettled request of
/// the message's person, under any alias, clarifies the newest such request, with `follow_up`;
/// an explicit `to` still makes a new request; another person's words, or a principal the call
/// interns, make a new request; and rule 5 suggests the clarification for an entry left unrouted
/// before the hold was taken. In the lab's sixth round (it5 l3 F2), a human's "Yep, that matches
/// what I meant" became a second request beside her first.
#[test]
fn a_follow_up_with_no_target_clarifies_the_persons_newest_request() {
    let (mut w, c, s, ac, a) = assistant();
    let alice = pid(&w, "mail:alice@x");
    // A person with two aliases, so that the chat alias is the same person.
    crate::helpers::user::tests::person(&mut w, &c, s, "alice", &["mail:alice@x", "chat:alice"]);
    // Unrouted while the assistant holds nothing.
    let d = ok(w.call(
        &ac,
        a,
        from_user_cmd("an early word", None, None, None, None),
    ));
    let (early, _) = routed(&d);
    assert_eq!(entry_routing(&w, early), Routing::Unrouted);
    // Its only hold, with no request of alice: the default is a new request there.
    let (h, _) = created(&ok(w.call(&ac, a, create_with("/refund-retries", |_| {}))));
    let d = ok(w.call(
        &ac,
        a,
        from_user_cmd("make refunds retry", None, None, None, None),
    ));
    let (e1, r) = routed(&d);
    let RoutedReply::Request { request: r1, .. } = r else {
        panic!("a new request: {r:?}");
    };
    assert!(!d.warnings.iter().any(|x| x.code == WarningCode::FollowUp));
    // Rule 5 for the early entry: the clarification of r1, with the route to the hold as the
    // alternative.
    let item = w
        .last_guidance()
        .items
        .iter()
        .find(|i| i.rule == crate::query::guide::Rule::Route)
        .cloned();
    assert!(item.is_none_or(
        |i| matches!(i.call, crate::query::guide::Skeleton::Route { entry, .. } if entry == early)
    ),);
    // The follow-up: a clarification of r1, with the warning.
    let d = ok(w.call(
        &ac,
        a,
        from_user_cmd(
            "Yep, 3 retries after the first failure",
            None,
            None,
            None,
            None,
        ),
    ));
    let (e2, r) = routed(&d);
    assert_eq!(r, RoutedReply::Clarifies { request: r1 });
    assert_eq!(entry_routing(&w, e2), Routing::Clarifies { request: r1 });
    let warn = d
        .warnings
        .iter()
        .find(|x| x.code == WarningCode::FollowUp)
        .expect("follow_up");
    assert_eq!(
        warn.detail,
        WarningDetail::FollowUp {
            entry: e2,
            request: r1,
            node: h,
            requester: alice,
        }
    );
    assert_ne!(e1, e2);
    // An explicit `to` is new work: a second request.
    let d = ok(w.call(
        &ac,
        a,
        from_user_cmd("and also email me", None, None, Some(h), None),
    ));
    let RoutedReply::Request { request: r2, .. } = routed(&d).1 else {
        panic!("a new request");
    };
    assert!(r2 > r1);
    // The next follow-up, under the other alias, clarifies the newest of them.
    let d = ok(w.call(
        &ac,
        a,
        from_user_cmd("thanks", Some("chat:alice"), None, None, None),
    ));
    assert_eq!(routed(&d).1, RoutedReply::Clarifies { request: r2 });
    // Another person, interned by this call: a new request.
    let d = ok(w.call(
        &ac,
        a,
        from_user_cmd("bob here", Some("mail:bob@x"), None, None, None),
    ));
    assert!(matches!(routed(&d).1, RoutedReply::Request { node, .. } if node == h));
    // The early entry is routed by rule 5's clarification.
    let d = ok(w.call(
        &ac,
        a,
        Cmd::Talk(TalkCmd::Route {
            entry: early,
            to: None,
            answers: None,
            clarifies: Some(r2),
            reason: None,
        }),
    ));
    assert!(matches!(d.reply, Reply::Routed { .. }));
    assert_eq!(entry_routing(&w, early), Routing::Clarifies { request: r2 });
}

/// Rule 5 (1.11): an entry unrouted when the session took a node that carries the person's
/// request is suggested as that request's clarification, with the route to the hold as the
/// alternative; `hello {from_user}` with no target routes as `talk from_user` does.
#[test]
fn rule_5_suggests_the_clarification_of_the_persons_request_on_the_only_hold() {
    let (mut w, c, s, ac, a) = assistant();
    let n = mk(&mut w, &c, s, "/ticket", |_| {});
    let (_, r1) = user_request(&mut w, &ac, a, n, "mail:alice@x");
    // Unrouted: the assistant holds nothing.
    let d = ok(w.call(
        &ac,
        a,
        from_user_cmd("one more thing", None, None, None, None),
    ));
    let (e, _) = routed(&d);
    // The take: rule 5's item is the clarification of r1.
    ok(w.call(
        &ac,
        a,
        Cmd::Hold(HoldCmd::Take {
            target: NodeRef::Id(n),
            lease: None,
        }),
    ));
    let route = w
        .last_guidance()
        .items
        .iter()
        .find(|i| i.rule == crate::query::guide::Rule::Route)
        .cloned()
        .expect("rule 5");
    assert_eq!(
        route.call,
        crate::query::guide::Skeleton::Route {
            entry: e,
            to: crate::query::guide::RouteSk::Clarifies(r1),
        }
    );
    assert_eq!(
        route.because,
        crate::query::guide::Because::RouteClarifies {
            entry: e,
            request: r1,
            or: Some(crate::query::guide::RouteAlt::Node {
                node: n,
                held: true
            }),
        }
    );
}
