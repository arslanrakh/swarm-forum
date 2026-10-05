//! Unit tests of WP-D5b's people commands (design D.12.1): `talk request`, `said`, its
//! confirmation, `heard`, `ask` to a person, and the Recheck firing (D.8.2, D.8.8, D.8.15, §9).
//! Every call runs through `World::call_ctx`, which checks `check_step` (C1 and Settled among
//! them), the dirty set, `decide == apply`, the fold of the events, the firings, and every state
//! invariant (Q1, W1, M1, and the rest) with R1 after the call.

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

use alloc::vec;
use alloc::vec::Vec;

use super::super::keys::tests::{
    decision, evs, id, inv, key, ks, nf, node, one, request, said, user_entry,
};
use super::super::restructure::tests::merge;
use crate::apply::tests::{class, create_with, created, entry, ok, refused, release, take, text};
use crate::cmd::*;
use crate::error::{What, Why};
use crate::event::{EntryBody, Ev};
use crate::query::{MOVED_MAX, SCAN_MAX};
use crate::reply::{Reply, Total, Warning, WarningCode, WarningDetail};
use crate::state::{Author, CommitmentState, PqState, Routing, SaidState, WaitOn};
use crate::testkit::World;
use crate::types::*;

// ------------------------------------------------------------------------------------------------
// Commands

pub(crate) fn request_cmd(n: NodeId, origin: &str, where_: Option<&str>) -> Cmd {
    Cmd::Talk(TalkCmd::Request {
        at: id(n),
        text: text("why did the summary miss two tickets?"),
        origin: key(origin),
        where_: where_.map(key),
        requester: text("Bob"),
    })
}

pub(crate) fn said_cmd(n: NodeId, where_: Option<&str>, kind: SaidKind) -> Cmd {
    Cmd::Talk(TalkCmd::Said {
        at: id(n),
        where_: where_.map(key),
        text: text("on it"),
        kind,
        corrects: None,
        seen: None,
    })
}

pub(crate) fn decide_cmd(n: NodeId, settles: &[Eid]) -> Cmd {
    Cmd::Talk(TalkCmd::Decide {
        at: id(n),
        text: text("decided"),
        settles: settles.iter().copied().collect(),
        supersedes: None,
        basis: Vec::new(),
        evidence: None,
    })
}

pub(crate) fn ask_cmd(n: NodeId, where_: Option<&str>, blocking: bool, again: bool) -> Cmd {
    Cmd::Talk(TalkCmd::Ask {
        at: id(n),
        text: text("which tickets?"),
        to: AskTo::Person {
            where_: where_.map(key),
            blocking,
        },
        again,
    })
}

pub(crate) fn heard_cmd(n: NodeId, where_: Option<&str>, answers: Option<Eid>) -> Cmd {
    Cmd::Talk(TalkCmd::Heard {
        at: id(n),
        where_: where_.map(key),
        from: text("Bob"),
        text: text("PROJ-12 and PROJ-19"),
        answers,
    })
}

fn confirm_cmd(a: Eid) -> Cmd {
    Cmd::Talk(TalkCmd::SaidConfirm {
        said: a,
        confirmed: text("https://chat.example/m/9"),
    })
}

/// The `said` reply's lists.
fn said_reply(r: &Reply) -> (Eid, NodeId, Vec<Eid>, Vec<Eid>, Vec<Eid>) {
    match r {
        Reply::Said {
            said,
            node,
            answered,
            settled,
            fulfilled,
            ..
        } => (
            *said,
            *node,
            answered.clone(),
            settled.clone(),
            fulfilled.clone(),
        ),
        other => panic!("not a said: {other:?}"),
    }
}

fn request_of(r: &Reply) -> (Eid, NodeId, bool) {
    match r {
        Reply::Request {
            request,
            node,
            existing,
        } => (*request, *node, *existing),
        other => panic!("not a request: {other:?}"),
    }
}

fn asked(r: &Reply) -> (Eid, Option<Eid>, NodeId, bool) {
    match r {
        Reply::Asked {
            question,
            said,
            node,
            existing,
            ..
        } => (*question, *said, *node, *existing),
        other => panic!("not asked: {other:?}"),
    }
}

fn warning(ws: &[Warning], code: WarningCode) -> Option<&Warning> {
    ws.iter().find(|w| w.code == code)
}

// ------------------------------------------------------------------------------------------------
// talk request

#[test]
fn request_refusals_in_order() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let unknown = NodeId(999);
    refused(
        &mut w,
        &c,
        s,
        request_cmd(unknown, "chat:a/1/m1", Some("chat:a/1")),
        nf(What::Node),
    );
    let mut empty = request_cmd(n, "chat:a/1/m1", Some("chat:a/1"));
    if let Cmd::Talk(TalkCmd::Request { text: t, .. }) = &mut empty {
        *t = Text::default();
    }
    refused(
        &mut w,
        &c,
        s,
        empty,
        inv(Why::EmptyText(crate::error::Field::Text)),
    );
    refused(
        &mut w,
        &c,
        s,
        request_cmd(n, "repeat:1:20261001T0000Z", None),
        inv(Why::ReservedKey),
    );
    refused(
        &mut w,
        &c,
        s,
        request_cmd(n, "chat:a/1/m1", Some("repeat:1:20261001T0000Z")),
        inv(Why::ReservedKey),
    );
    // A reserved node as the request's node: case 3 on a reserved `at` (C1).
    refused(
        &mut w,
        &c,
        s,
        request_cmd(USER, "chat:z/9/m1", Some("chat:z/9")),
        inv(Why::ReservedNode),
    );
}

#[test]
fn requests_by_message() {
    // A-61: one request per message; a thread's messages are its requests on its conversation
    // node; a message origin without `where` becomes a key of `at` (case 3); a conversation key as
    // origin without `where` behaves as in 1.1.
    let (mut w, c, s) = one();
    let thread = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let child = node(&mut w, &c, s, "/support/work", &[]);
    let d = ok(w.call(&c, s, request_cmd(child, "chat:a/1/m1", Some("chat:a/1"))));
    let (r1, n1, existing) = request_of(&d.reply);
    assert_eq!((n1, existing), (thread, false));
    assert_eq!(w.s.requests[&r1].conv, ks("chat:a/1"));
    assert_eq!(w.s.requests[&r1].origin.key, ks("chat:a/1/m1"));
    // The same message again is the same request, and nothing is appended (Q1).
    let d = ok(w.call(&c, s, request_cmd(child, "chat:a/1/m1", Some("chat:a/1"))));
    assert_eq!(request_of(&d.reply), (r1, thread, true));
    assert!(w.last_events.is_empty() || evs(&w).iter().all(|e| matches!(e, Ev::Touched { .. })));
    // A second message of the thread is a second request on the same node.
    let d = ok(w.call(&c, s, request_cmd(child, "chat:a/1/m2", Some("chat:a/1"))));
    let (r2, n2, _) = request_of(&d.reply);
    assert_ne!(r1, r2);
    assert_eq!(n2, thread);
    // A message origin without `where`: the message becomes a key of `at`.
    let other = node(&mut w, &c, s, "/other", &[]);
    let d = ok(w.call(&c, s, request_cmd(other, "mail:m1@x", None)));
    let (r3, n3, _) = request_of(&d.reply);
    assert_eq!(n3, other);
    assert!(w.s.nodes[&other].keys.contains_key(&ks("mail:m1@x")));
    assert_eq!(w.s.requests[&r3].conv, ks("mail:m1@x"));
    // A conversation key as origin without `where`: one request per conversation, as in 1.1.
    let d = ok(w.call(&c, s, request_cmd(child, "chat:a/1", None)));
    let (r4, n4, _) = request_of(&d.reply);
    assert_eq!(n4, thread);
    assert_eq!(w.s.requests[&r4].conv, ks("chat:a/1"));
    let d = ok(w.call(&c, s, request_cmd(child, "chat:a/1", None)));
    assert_eq!(request_of(&d.reply), (r4, thread, true));
}

#[test]
fn a_request_from_a_user_message_routes_its_entry() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/work", &[]);
    let e = user_entry(&mut w, s, USER_PRINCIPAL);
    let origin = alloc::format!("user:{}", e.render());
    let cmd = Cmd::Talk(TalkCmd::Request {
        at: id(n),
        text: text("the user's message"),
        origin: Key {
            key: ks(&origin),
            nv: NormVer(0),
        },
        where_: None,
        requester: text("the user"),
    });
    let d = ok(w.call(&c, s, cmd));
    let (r, node_, _) = request_of(&d.reply);
    assert_eq!(node_, n);
    assert_eq!(w.s.requests[&r].from_user, Some(e));
    assert_eq!(w.s.requests[&r].requester, Requester::User(USER_PRINCIPAL));
    assert_eq!(
        w.s.user_entries[&e].routing,
        Routing::Request {
            node: n,
            request: r
        }
    );
}

// ------------------------------------------------------------------------------------------------
// talk said

#[test]
fn said_refusals_in_order() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let plain = node(&mut w, &c, s, "/plain", &[]);
    let two = node(&mut w, &c, s, "/two", &["chat:b/1", "mail:m@x"]);
    refused(
        &mut w,
        &c,
        s,
        said_cmd(NodeId(999), None, SaidKind::Reply),
        nf(What::Node),
    );
    refused(
        &mut w,
        &c,
        s,
        said_cmd(n, None, SaidKind::Question),
        inv(Why::SaidKindQuestion),
    );
    let mut corrects = said_cmd(n, None, SaidKind::Correction);
    if let Cmd::Talk(TalkCmd::Said { corrects: x, .. }) = &mut corrects {
        *x = Some(Eid(999));
    }
    refused(&mut w, &c, s, corrects, nf(What::Said));
    refused(
        &mut w,
        &c,
        s,
        said_cmd(n, Some("repeat:1:20261001T0000Z"), SaidKind::Reply),
        inv(Why::ReservedKey),
    );
    refused(
        &mut w,
        &c,
        s,
        said_cmd(plain, None, SaidKind::Reply),
        inv(Why::NoConversationKey),
    );
    refused(
        &mut w,
        &c,
        s,
        said_cmd(two, None, SaidKind::Reply),
        inv(Why::AmbiguousConversationKey),
    );
    refused(
        &mut w,
        &c,
        s,
        said_cmd(USER, Some("chat:z/1"), SaidKind::Reply),
        inv(Why::ReservedNode),
    );
    // `not_found` comes before `invalid` (D.14.5): an unknown `corrects` on a node with no
    // conversation key is `not_found(Said)`.
    let mut both = said_cmd(plain, None, SaidKind::Correction);
    if let Cmd::Talk(TalkCmd::Said { corrects: x, .. }) = &mut both {
        *x = Some(Eid(999));
    }
    refused(&mut w, &c, s, both, nf(What::Said));
}

#[test]
fn an_ack_opens_and_refreshes_a_commitment_and_a_reply_fulfils_answers_and_settles() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    busy(&mut w, &c, s);
    let d = ok(w.call(&c, s, request_cmd(n, "chat:a/1/m1", Some("chat:a/1"))));
    let (r, _, _) = request_of(&d.reply);
    // The ack: a commitment, and empty lists (R9).
    let d = ok(w.call(&c, s, said_cmd(n, None, SaidKind::Ack)));
    let (ack, node_, answered, settled, fulfilled) = said_reply(&d.reply);
    assert_eq!(node_, n);
    assert!(answered.is_empty() && settled.is_empty() && fulfilled.is_empty());
    assert_eq!(w.s.commitments[&ack].state, CommitmentState::Open);
    assert_eq!(w.s.saids[&ack].state, SaidState::Pending);
    // A second ack refreshes it (A-26): no second commitment.
    w.advance(Dur(60_000));
    let d = ok(w.call(&c, s, said_cmd(n, None, SaidKind::Ack)));
    let (ack2, ..) = said_reply(&d.reply);
    assert!(!w.s.commitments.contains_key(&ack2));
    assert_eq!(w.s.commitments[&ack].last_ack, w.clock);
    assert!(
        evs(&w)
            .iter()
            .any(|e| matches!(e, Ev::CommitmentRefreshed { c, .. } if *c == ack))
    );
    // A decision first, then the reply: answered, settled, fulfilled.
    ok(w.call(&c, s, decide_cmd(n, &[r])));
    assert!(w.s.requests[&r].settled.is_none());
    let d = ok(w.call(&c, s, said_cmd(n, None, SaidKind::Result)));
    let (a, _, answered, settled, fulfilled) = said_reply(&d.reply);
    assert_eq!(answered, vec![r]);
    assert_eq!(settled, vec![r]);
    assert_eq!(fulfilled, vec![ack]);
    assert_eq!(w.s.requests[&r].answered_by, Some(a));
    assert_eq!(
        w.s.commitments[&ack].state,
        CommitmentState::Fulfilled { by: a }
    );
    // A later reply answers nothing more.
    let d = ok(w.call(&c, s, said_cmd(n, None, SaidKind::Reply)));
    let (_, _, answered, settled, fulfilled) = said_reply(&d.reply);
    assert!(answered.is_empty() && settled.is_empty() && fulfilled.is_empty());
}

#[test]
fn settled_with_the_decision_before_or_after_the_answer_and_after_a_supersession() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    // The answer first, then the decision.
    let (r1, ..) =
        request_of(&ok(w.call(&c, s, request_cmd(n, "chat:a/1/m1", Some("chat:a/1")))).reply);
    let d = ok(w.call(&c, s, said_cmd(n, None, SaidKind::Reply)));
    let (a1, _, answered, settled, _) = said_reply(&d.reply);
    assert_eq!((answered, settled), (vec![r1], Vec::new()));
    let d = ok(w.call(&c, s, decide_cmd(n, &[r1])));
    let Reply::Decided {
        decision: d1,
        settled,
        not_settled,
    } = &d.reply
    else {
        panic!("{:?}", d.reply)
    };
    assert_eq!(settled, &vec![r1]);
    assert!(not_settled.is_empty());
    assert_eq!(w.s.requests[&r1].settled.map(|x| x.answer), Some(a1));
    assert_eq!(w.s.requests[&r1].settled.map(|x| x.decision), Some(*d1));
    // Supersede then answer: `d1 {settles: [r]}`, `d2 {supersedes: d1}`, then the answer settles
    // `r` by `d1` (A-11).
    let (r2, ..) =
        request_of(&ok(w.call(&c, s, request_cmd(n, "chat:a/1/m2", Some("chat:a/1")))).reply);
    let d = ok(w.call(&c, s, decide_cmd(n, &[r2])));
    let Reply::Decided { decision: d2, .. } = d.reply else {
        panic!()
    };
    let sup = Cmd::Talk(TalkCmd::Decide {
        at: id(n),
        text: text("revised"),
        settles: Default::default(),
        supersedes: Some(d2),
        basis: Vec::new(),
        evidence: None,
    });
    ok(w.call(&c, s, sup));
    let d = ok(w.call(&c, s, said_cmd(n, None, SaidKind::Reply)));
    let (_, _, _, settled, _) = said_reply(&d.reply);
    assert_eq!(settled, vec![r2]);
    assert_eq!(w.s.requests[&r2].settled.map(|x| x.decision), Some(d2));
}

#[test]
fn a_said_answers_only_its_own_conversation() {
    // §9.4 (1.3): a node carrying `chat:a/1` and `mail:m@x`.
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/two", &["chat:a/1", "mail:m@x"]);
    let chat = request(&mut w, s, n, "chat:a/1/m1", "chat:a/1");
    let mail = request(&mut w, s, n, "mail:m1@x", "mail:m@x");
    // Without `where` the node's two keys are ambiguous.
    refused(
        &mut w,
        &c,
        s,
        said_cmd(n, None, SaidKind::Result),
        inv(Why::AmbiguousConversationKey),
    );
    let d = ok(w.call(&c, s, said_cmd(n, Some("chat:a/1"), SaidKind::Result)));
    let (a, _, answered, ..) = said_reply(&d.reply);
    assert_eq!(answered, vec![chat]);
    assert_eq!(w.s.requests[&chat].answered_by, Some(a));
    assert_eq!(w.s.requests[&mail].answered_by, None);
    // A later request of `chat:a/1` is not answered by the earlier `said`.
    let later = request(&mut w, s, n, "chat:a/1/m2", "chat:a/1");
    assert_eq!(w.s.requests[&later].answered_by, None);
    // The mail conversation's own reply answers it.
    let d = ok(w.call(&c, s, said_cmd(n, Some("mail:m@x"), SaidKind::Reply)));
    let (_, _, answered, ..) = said_reply(&d.reply);
    assert_eq!(answered, vec![mail]);
    assert_eq!(w.s.requests[&later].answered_by, None);
}

#[test]
fn a_said_whose_where_is_a_user_key_answers_no_user_request() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/work", &["chat:a/1"]);
    let e = user_entry(&mut w, s, USER_PRINCIPAL);
    let k = alloc::format!("user:{}", e.render());
    ok(w.call(
        &c,
        s,
        Cmd::Node(NodeCmd::Key {
            at: id(n),
            change: KeyChange::Add(Key {
                key: ks(&k),
                nv: NormVer(0),
            }),
        }),
    ));
    let r = match w.s.user_entries[&e].routing {
        Routing::Request { request, .. } => request,
        other => panic!("{other:?}"),
    };
    let d = ok(w.call(&c, s, said_cmd(n, Some(&k), SaidKind::Result)));
    let (_, _, answered, ..) = said_reply(&d.reply);
    assert!(answered.is_empty());
    assert_eq!(w.s.requests[&r].answered_by, None);
}

#[test]
fn a_said_fulfils_only_its_own_conversations_commitments() {
    // §9.2 (1.3): an open commitment in each of two conversations on one node.
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/two", &["chat:a/1", "mail:m@x"]);
    let d = ok(w.call(&c, s, said_cmd(n, Some("chat:a/1"), SaidKind::Ack)));
    let (chat_ack, ..) = said_reply(&d.reply);
    let d = ok(w.call(&c, s, said_cmd(n, Some("mail:m@x"), SaidKind::Ack)));
    let (mail_ack, ..) = said_reply(&d.reply);
    assert_eq!(w.s.commitments[&mail_ack].conv, ks("mail:m@x"));
    // An ack in the chat refreshes only the chat's commitment.
    w.advance(Dur(60_000));
    let before = w.s.commitments[&mail_ack].clone();
    ok(w.call(&c, s, said_cmd(n, Some("chat:a/1"), SaidKind::Ack)));
    assert_eq!(w.s.commitments[&chat_ack].last_ack, w.clock);
    assert_eq!(w.s.commitments[&mail_ack], before);
    // A reply in the chat fulfils the chat's commitment and leaves the mail's open.
    let d = ok(w.call(&c, s, said_cmd(n, Some("chat:a/1"), SaidKind::Correction)));
    let (_, _, _, _, fulfilled) = said_reply(&d.reply);
    assert_eq!(fulfilled, vec![chat_ack]);
    assert_eq!(w.s.commitments[&mail_ack].state, CommitmentState::Open);
    // Once its follow-up passes on the unattended node, the mail's commitment raises the flag.
    w.advance(w.s.params.commitment_followup);
    assert!(w.s.ix.node[&n].flags.contains(Flag::OpenCommitment));
}

#[test]
fn a_merge_brings_a_commitment_that_the_next_reply_of_its_conversation_fulfils() {
    let (mut w, c, s) = one();
    let into = node(&mut w, &c, s, "/into", &["chat:a/1", "mail:m@x"]);
    let from = node(&mut w, &c, s, "/from", &["issue:p#1"]);
    let d = ok(w.call(&c, s, said_cmd(from, None, SaidKind::Ack)));
    let (third, ..) = said_reply(&d.reply);
    ok(w.call(&c, s, merge(from, into)));
    assert_eq!(w.s.commitments[&third].node, into);
    assert_eq!(w.s.commitments[&third].state, CommitmentState::Open);
    // A reply in another conversation on `into` does not fulfil it.
    let d = ok(w.call(&c, s, said_cmd(into, Some("chat:a/1"), SaidKind::Reply)));
    assert!(said_reply(&d.reply).4.is_empty());
    // Its own conversation's reply does.
    let d = ok(w.call(&c, s, said_cmd(into, Some("issue:p#1"), SaidKind::Reply)));
    assert_eq!(said_reply(&d.reply).4, vec![third]);
}

#[test]
fn said_confirm_and_its_refusal() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    busy(&mut w, &c, s);
    refused(&mut w, &c, s, confirm_cmd(Eid(999)), nf(What::Said));
    let d = ok(w.call(&c, s, said_cmd(n, None, SaidKind::Reply)));
    let (a, ..) = said_reply(&d.reply);
    let d = ok(w.call(&c, s, confirm_cmd(a)));
    assert_eq!(d.reply, Reply::SaidConfirmed { already: false });
    assert_eq!(w.s.saids[&a].state, SaidState::Confirmed);
    assert!(evs(&w).iter().any(|e| matches!(
        e,
        Ev::SaidConfirmed {
            said,
            via: crate::event::ConfirmVia::Explicit,
            link: Some(_)
        } if *said == a
    )));
    let d = ok(w.call(&c, s, confirm_cmd(a)));
    assert_eq!(d.reply, Reply::SaidConfirmed { already: true });
    assert!(
        evs(&w)
            .iter()
            .all(|e| !matches!(e, Ev::SaidConfirmed { .. }))
    );
}

#[test]
fn an_unconfirmed_said_is_confirmed_by_its_link() {
    // A `said` whose session fell silent is unconfirmed; the explicit link confirms it (§25). 1.11:
    // its session held a node when it recorded it, whose lease then ended.
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    busy(&mut w, &c, s);
    let d = ok(w.call(&c, s, said_cmd(n, None, SaidKind::Reply)));
    let (a, ..) = said_reply(&d.reply);
    w.advance(w.s.params.presence_silence);
    w.advance(Dur(1));
    assert_eq!(w.s.saids[&a].state, SaidState::Unconfirmed);
    let s2 = w.hello(&c);
    let d = ok(w.call(&c, s2, confirm_cmd(a)));
    assert_eq!(d.reply, Reply::SaidConfirmed { already: false });
    assert_eq!(w.s.saids[&a].state, SaidState::Confirmed);
}

/// 1.10 (§9.2, l6 of the lab's fifth round): a `said` by a session that holds neither the
/// conversation node nor work inside it, while another session does, counts as any `said` does:
/// it answers, fulfils, and opens, with the `conversation_held` warning, and its author's release
/// confirms it. 1.9 withheld it, and the chat worker's answer was withheld while the holder could
/// not post in the chat.
#[test]
fn a_non_holder_s_said_counts_with_conversation_held() {
    let (mut w, c, s) = one();
    let s2 = w.hello(&c);
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let (r, ..) =
        request_of(&ok(w.call(&c, s, request_cmd(n, "chat:a/1/7", Some("chat:a/1")))).reply);
    ok(w.call(&c, s, said_cmd(n, None, SaidKind::Ack)));
    let open: Vec<Eid> = w.s.commitments.keys().copied().collect();
    assert_eq!(open.len(), 1);
    ok(w.call(&c, s, take(n)));
    // 1.11 (§9.2): s2 holds other work, so its `said` entries stay pending until its release.
    let other = node(&mut w, &c, s2, "/other", &[]);
    let t = match ok(w.call(&c, s2, take(other))).reply {
        Reply::Taken { token, .. } => token,
        x => panic!("{x:?}"),
    };
    // The non-holder's reply answers the request and fulfils the commitment.
    let d = ok(w.call(&c, s2, said_cmd(n, None, SaidKind::Reply)));
    let (a, _, answered, _, fulfilled) = said_reply(&d.reply);
    assert_eq!(answered, vec![r]);
    assert_eq!(fulfilled, open);
    assert_eq!(w.s.saids[&a].state, SaidState::Pending);
    assert_eq!(w.s.requests[&r].answered_by, Some(a));
    assert!(matches!(
        w.s.commitments[&open[0]].state,
        CommitmentState::Fulfilled { .. }
    ));
    assert!(!evs(&w).iter().any(|e| matches!(e, Ev::SaidWithheld { .. })));
    assert_eq!(
        warning(&d.warnings, WarningCode::ConversationHeld).map(|x| &x.detail),
        Some(&WarningDetail::ConversationHeld { holders: vec![s] })
    );
    // Its ack opens a commitment.
    let d = ok(w.call(&c, s2, said_cmd(n, None, SaidKind::Ack)));
    let (ack, ..) = said_reply(&d.reply);
    assert_eq!(w.s.saids[&ack].state, SaidState::Pending);
    assert_eq!(w.s.commitments.len(), 2);
    // Its author's release of other work confirms both.
    ok(w.call(&c, s2, release(t, ReleaseReason::Handoff)));
    assert_eq!(w.s.saids[&a].state, SaidState::Confirmed);
    assert_eq!(w.s.saids[&ack].state, SaidState::Confirmed);
}

/// 1.9 (§9.2, l6 of the lab's fourth round): an entry listed in `unsent` that a later `said` of its
/// conversation replaced is withheld, not unconfirmed, so the closed node is no candidate; one
/// that nothing replaced is unconfirmed as before. 1.10: a non-holder's later reply replaces it.
#[test]
fn an_unsent_said_a_later_one_replaced_is_withheld() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let t = created_token(&mut w, &c, s, n);
    let (draft, ..) = said_reply(&ok(w.call(&c, s, said_cmd(n, None, SaidKind::Reply))).reply);
    let (fix, ..) = said_reply(&ok(w.call(&c, s, said_cmd(n, None, SaidKind::Correction))).reply);
    let mut cmd = super::super::keys::tests::close_cmd(n, Outcome::Done);
    if let Cmd::Node(NodeCmd::Close { unsent, token, .. }) = &mut cmd {
        *unsent = [draft].into();
        *token = Some(t);
    }
    ok(w.call(&c, s, cmd));
    assert_eq!(w.s.saids[&draft].state, SaidState::Withheld);
    assert_eq!(w.s.saids[&fix].state, SaidState::Confirmed);
    assert!(!w.s.ix.node[&n].flags.contains(Flag::UnconfirmedSaid));
    assert!(!w.s.ix.next.of.contains_key(&crate::state::Cand::Node(n)));
    // Both unsent: the earlier is replaced, the last is for whoever checks the conversation.
    let m = node(&mut w, &c, s, "/billing", &["chat:b/1"]);
    let t = created_token(&mut w, &c, s, m);
    let (first, ..) = said_reply(&ok(w.call(&c, s, said_cmd(m, None, SaidKind::Reply))).reply);
    let (last, ..) = said_reply(&ok(w.call(&c, s, said_cmd(m, None, SaidKind::Reply))).reply);
    let mut cmd = release(t, ReleaseReason::Handoff);
    if let Cmd::Hold(HoldCmd::Release { unsent, .. }) = &mut cmd {
        *unsent = [first, last].into();
    }
    ok(w.call(&c, s, cmd));
    assert_eq!(w.s.saids[&first].state, SaidState::Withheld);
    assert_eq!(w.s.saids[&last].state, SaidState::Unconfirmed);
    assert!(w.s.ix.node[&m].flags.contains(Flag::UnconfirmedSaid));
    // An ack after it, or another conversation's reply, replaces nothing.
    let k = node(&mut w, &c, s, "/desk", &["chat:d/1"]);
    let t = created_token(&mut w, &c, s, k);
    let (reply, ..) = said_reply(&ok(w.call(&c, s, said_cmd(k, None, SaidKind::Reply))).reply);
    ok(w.call(&c, s, said_cmd(k, None, SaidKind::Ack)));
    let mut cmd = release(t, ReleaseReason::Handoff);
    if let Cmd::Hold(HoldCmd::Release { unsent, .. }) = &mut cmd {
        *unsent = [reply].into();
    }
    ok(w.call(&c, s, cmd));
    assert_eq!(w.s.saids[&reply].state, SaidState::Unconfirmed);
    // 1.10 (§9.2): a non-holder's reply after it counts, so it replaces it.
    let s2 = w.hello(&c);
    let j = node(&mut w, &c, s, "/ops", &["chat:o/1"]);
    let t = created_token(&mut w, &c, s, j);
    let (own, ..) = said_reply(&ok(w.call(&c, s, said_cmd(j, None, SaidKind::Reply))).reply);
    let (theirs, ..) = said_reply(&ok(w.call(&c, s2, said_cmd(j, None, SaidKind::Reply))).reply);
    // 1.11 (§9.2): s2 holds nothing, so its reply is taken as sent at once, with its reply.
    assert_eq!(w.s.saids[&theirs].state, SaidState::Confirmed);
    assert!(w.s.saids[&theirs].with_reply);
    assert!(super::super::hold::replaced(&w.s, own));
    let mut cmd = release(t, ReleaseReason::Handoff);
    if let Cmd::Hold(HoldCmd::Release { unsent, .. }) = &mut cmd {
        *unsent = [own].into();
    }
    ok(w.call(&c, s, cmd));
    assert_eq!(w.s.saids[&own].state, SaidState::Withheld);
}

// ------------------------------------------------------------------------------------------------
// The warnings of `said` and `ask`

#[test]
fn moved_lists_what_others_recorded_after_seen() {
    let (mut w, c, s) = one();
    let s2 = w.hello(&c);
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    // Nothing recorded by others: no warning.
    let d = ok(w.call(&c, s, said_cmd(n, None, SaidKind::Ack)));
    assert!(warning(&d.warnings, WarningCode::Moved).is_none());
    // Another session records three people entries; the first session's own are not listed.
    let r = request(&mut w, s2, n, "chat:a/1/m1", "chat:a/1");
    let a = said(&mut w, s2, n, "chat:a/1", SaidKind::Reply);
    let h = entry(
        &mut w,
        n,
        Author::Session(s2),
        EntryBody::Heard {
            conv: ks("chat:a/1"),
            answers: None,
        },
    );
    let d = ok(w.call(&c, s, said_cmd(n, None, SaidKind::Reply)));
    let m = warning(&d.warnings, WarningCode::Moved).expect("moved");
    assert_eq!(m.id, Some(n));
    assert_eq!(
        m.detail,
        WarningDetail::Moved {
            entries: vec![h, a, r],
            total: Total::Exact(3),
        }
    );
    // With `seen` past them, nothing.
    let seen = w.s.nodes[&n].seq;
    let mut cmd = said_cmd(n, None, SaidKind::Reply);
    if let Cmd::Talk(TalkCmd::Said { seen: x, .. }) = &mut cmd {
        *x = Some(seen);
    }
    let d = ok(w.call(&c, s, cmd));
    assert!(warning(&d.warnings, WarningCode::Moved).is_none());
    // A brief marks the node shown, which is the default `seen`.
    ok(w.call(
        &c,
        s,
        Cmd::View(ViewCmd::Node {
            targets: vec![id(n)],
            view: None,
            page: None,
        }),
    ));
    let d = ok(w.call(&c, s, said_cmd(n, None, SaidKind::Reply)));
    assert!(warning(&d.warnings, WarningCode::Moved).is_none());
}

#[test]
fn moved_is_cut_at_moved_max_and_counted_up_to_scan_max() {
    let (mut w, c, s) = one();
    let s2 = w.hello(&c);
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let many = MOVED_MAX + 5;
    let mut ids = Vec::new();
    for _ in 0..many {
        ids.push(said(&mut w, s2, n, "chat:a/1", SaidKind::Ack));
    }
    let d = ok(w.call(&c, s, said_cmd(n, None, SaidKind::Ack)));
    let WarningDetail::Moved { entries, total } = &warning(&d.warnings, WarningCode::Moved)
        .expect("moved")
        .detail
    else {
        panic!()
    };
    let newest: Vec<Eid> = ids.iter().rev().take(MOVED_MAX).copied().collect();
    assert_eq!(entries, &newest);
    assert_eq!(*total, Total::Exact(u32::try_from(many).unwrap()));
    // Past SCAN_MAX entries after `seen`, the total is `≥k`.
    let mut w2 = World::new(crate::params::Params::default(), w.clock);
    let c2 = class("worker");
    let a = w2.hello(&c2);
    let b = w2.hello(&c2);
    let n2 = node(&mut w2, &c2, a, "/big", &["chat:a/1"]);
    for _ in 0..=SCAN_MAX {
        entry(
            &mut w2,
            n2,
            Author::Session(b),
            EntryBody::Heard {
                conv: ks("chat:a/1"),
                answers: None,
            },
        );
    }
    let d = ok(w2.call(&c2, a, said_cmd(n2, None, SaidKind::Ack)));
    let WarningDetail::Moved { entries, total } = &warning(&d.warnings, WarningCode::Moved)
        .expect("moved")
        .detail
    else {
        panic!()
    };
    assert_eq!(entries.len(), MOVED_MAX);
    assert_eq!(*total, Total::AtLeast(SCAN_MAX));
}

#[test]
fn moved_counts_at_least_once_a_people_entry_after_seen_left_memory() {
    // D.8.15, D.4.12: the total is exact only when `seen >= c.people_left`; a `heard` past `seen`
    // whose metadata left memory makes it `≥k`, though the range holds far fewer than SCAN_MAX.
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let heard = |w: &mut World, by: SessionId| {
        entry(
            w,
            n,
            Author::Session(by),
            EntryBody::Heard {
                conv: ks("chat:a/1"),
                answers: None,
            },
        )
    };
    let old = heard(&mut w, s);
    let old_seq = w.s.entries[&old].seq;
    // Past FINISHED_RETAIN the old entry's metadata leaves memory, and `people_left` rises to it.
    w.advance(Dur(crate::state::FINISHED_RETAIN.0 + 1));
    let a = w.hello(&c);
    let b = w.hello(&c);
    assert!(!w.s.entries.contains_key(&old));
    assert_eq!(w.s.nodes[&n].people_left, old_seq);
    let new = heard(&mut w, b);
    // `a` was shown nothing, so `seen` is 0, below `people_left`.
    let d = ok(w.call(&c, a, said_cmd(n, None, SaidKind::Ack)));
    let m = warning(&d.warnings, WarningCode::Moved).expect("moved");
    assert_eq!(
        m.detail,
        WarningDetail::Moved {
            entries: vec![new],
            total: Total::AtLeast(1),
        }
    );
    // With `seen` at `people_left`, nothing that left lies past it: the total is exact.
    let mut cmd = said_cmd(n, None, SaidKind::Ack);
    if let Cmd::Talk(TalkCmd::Said { seen: x, .. }) = &mut cmd {
        *x = Some(old_seq);
    }
    let d = ok(w.call(&c, a, cmd));
    let m = warning(&d.warnings, WarningCode::Moved).expect("moved");
    assert_eq!(
        m.detail,
        WarningDetail::Moved {
            entries: vec![new],
            total: Total::Exact(1),
        }
    );
}

#[test]
fn conversation_held_names_the_holder_of_the_conversation_or_work_inside_it() {
    let (mut w, c, s) = one();
    let s2 = w.hello(&c);
    let conv = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let work = node(&mut w, &c, s, "/support/work", &[]);
    // Nobody holds anything: no warning.
    let d = ok(w.call(&c, s, said_cmd(conv, None, SaidKind::Ack)));
    assert!(warning(&d.warnings, WarningCode::ConversationHeld).is_none());
    // Another session holds work inside the conversation.
    ok(w.call(&c, s2, take(work)));
    assert_eq!(
        crate::helpers::agents::conversation_holder(
            &w.s,
            conv,
            s,
            &crate::types::KeyStr::new("chat:a/1").unwrap()
        ),
        Some(s2)
    );
    let d = ok(w.call(&c, s, said_cmd(conv, None, SaidKind::Ack)));
    let held = warning(&d.warnings, WarningCode::ConversationHeld).expect("held");
    assert_eq!(
        held.detail,
        WarningDetail::ConversationHeld { holders: vec![s2] }
    );
    // The holder itself gets none.
    let d = ok(w.call(&c, s2, said_cmd(work, None, SaidKind::Ack)));
    assert!(warning(&d.warnings, WarningCode::ConversationHeld).is_none());
    // Nor does a caller that holds work inside it too.
    let other = node(&mut w, &c, s, "/support/other", &[]);
    ok(w.call(&c, s, take(other)));
    let d = ok(w.call(&c, s, said_cmd(conv, None, SaidKind::Ack)));
    assert!(warning(&d.warnings, WarningCode::ConversationHeld).is_none());
    assert_eq!(
        crate::helpers::agents::conversation_holder(
            &w.s,
            conv,
            s,
            &crate::types::KeyStr::new("chat:a/1").unwrap()
        ),
        None
    );
}

#[test]
fn the_agreement_warning_of_said_lists_both_chains() {
    let (mut w, c, s) = one();
    let thread = {
        let d = ok(w.call(
            &c,
            s,
            create_with("/threads", |a| {
                a.agreements = Some(text("reply within a day"));
                a.keys = vec![key("chat:a/1")];
                a.take = Some(false);
            }),
        ));
        created(&d).0
    };
    let work = {
        let d = ok(w.call(
            &c,
            s,
            create_with("/work", |a| {
                a.agreements = Some(text("ask before sending"));
                a.take = Some(false);
            }),
        ));
        created(&d).0
    };
    let d = ok(w.call(&c, s, said_cmd(work, Some("chat:a/1"), SaidKind::Ack)));
    let a = warning(&d.warnings, WarningCode::Agreement).expect("agreement");
    assert_eq!(a.id, Some(work));
    let WarningDetail::Agreement {
        sources, approvals, ..
    } = &a.detail
    else {
        panic!()
    };
    assert_eq!(sources, &vec![work, thread]);
    assert_eq!(*approvals, 0);
}

// ------------------------------------------------------------------------------------------------
// talk heard

#[test]
fn heard_refusals_in_order() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let plain = node(&mut w, &c, s, "/plain", &[]);
    refused(
        &mut w,
        &c,
        s,
        heard_cmd(NodeId(999), None, None),
        nf(What::Node),
    );
    refused(
        &mut w,
        &c,
        s,
        heard_cmd(n, None, Some(Eid(9_999))),
        nf(What::Question),
    );
    // An entry that is not a question at all.
    let r = request(&mut w, s, n, "chat:a/1/m1", "chat:a/1");
    refused(
        &mut w,
        &c,
        s,
        heard_cmd(n, None, Some(r)),
        nf(What::Question),
    );
    // `not_found` before `invalid` (D.14.5): an unknown question on a node without a key.
    refused(
        &mut w,
        &c,
        s,
        heard_cmd(plain, None, Some(Eid(9_999))),
        nf(What::Question),
    );
    refused(
        &mut w,
        &c,
        s,
        heard_cmd(plain, None, None),
        inv(Why::NoConversationKey),
    );
    refused(
        &mut w,
        &c,
        s,
        heard_cmd(n, Some("repeat:1:20261001T0000Z"), None),
        inv(Why::ReservedKey),
    );
    // An agent question is not a person's.
    let aq = ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Ask {
            at: id(n),
            text: text("anyone?"),
            to: AskTo::Agent(AgentAddressee::Session(s)),
            again: false,
        }),
    ));
    let (q, ..) = asked(&aq.reply);
    refused(
        &mut w,
        &c,
        s,
        heard_cmd(n, None, Some(q)),
        inv(Why::NotPeopleQuestion),
    );
}

#[test]
fn heard_answers_an_open_question_and_records_a_late_one() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let d = ok(w.call(&c, s, ask_cmd(n, None, false, false)));
    let (q1, ..) = asked(&d.reply);
    // `again` supersedes q1 with q2.
    let d = ok(w.call(&c, s, ask_cmd(n, None, false, true)));
    let (q2, ..) = asked(&d.reply);
    assert_eq!(w.s.pquestions[&q1].state, PqState::Superseded { by: q2 });
    // An answer naming the superseded question is recorded and changes no state.
    let d = ok(w.call(&c, s, heard_cmd(n, None, Some(q1))));
    let Reply::Heard { answered, node, .. } = d.reply else {
        panic!()
    };
    assert_eq!((answered, node), (None, n));
    assert_eq!(w.s.pquestions[&q1].state, PqState::Superseded { by: q2 });
    assert_eq!(w.s.pquestions[&q2].state, PqState::Open);
    // The open one is answered.
    let d = ok(w.call(&c, s, heard_cmd(n, None, Some(q2))));
    let Reply::Heard {
        entry, answered, ..
    } = d.reply
    else {
        panic!()
    };
    assert_eq!(answered, Some(q2));
    assert_eq!(w.s.pquestions[&q2].state, PqState::Answered { by: entry });
}

// ------------------------------------------------------------------------------------------------
// talk ask, to a person

#[test]
fn ask_records_a_said_and_a_question_with_its_first_recheck() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let work = node(&mut w, &c, s, "/support/work", &[]);
    busy(&mut w, &c, s);
    let d = ok(w.call(&c, s, ask_cmd(work, None, true, false)));
    let (q, a, node_, existing) = asked(&d.reply);
    assert_eq!((node_, existing), (n, false));
    let a = a.expect("the said of kind question");
    assert_eq!(w.s.saids[&a].kind, SaidKind::Question);
    assert_eq!(w.s.saids[&a].state, SaidState::Pending);
    let pq = &w.s.pquestions[&q];
    assert_eq!((pq.node, pq.blocking, pq.said), (n, true, a));
    assert_eq!(pq.recheck_at, w.clock.plus(w.s.params.question_recheck));
    // Asking again without `again` returns it, and records nothing (§9.3, A-25).
    let d = ok(w.call(&c, s, ask_cmd(n, None, false, false)));
    assert_eq!(asked(&d.reply), (q, None, n, true));
    assert!(evs(&w).iter().all(|e| matches!(e, Ev::Touched { .. })));
}

#[test]
fn ask_again_moves_only_the_waits_that_are_still_unresolved() {
    // A-109.
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let a = node(&mut w, &c, s, "/support/a", &[]);
    let b = node(&mut w, &c, s, "/support/b", &[]);
    let d = ok(w.call(&c, s, ask_cmd(n, None, false, false)));
    let (q1, ..) = asked(&d.reply);
    for x in [a, b] {
        let t = created_token(&mut w, &c, s, x);
        ok(w.call(&c, s, waiting_release(t, q1)));
    }
    let wait_a = WaitOn::Question {
        q: q1,
        rechecks_at_set: 0,
    };
    assert_eq!(w.s.nodes[&a].wait.as_ref().map(|x| x.on), Some(wait_a));
    // A recheck resolves both waits; then `b` waits again, unresolved.
    let at = w.s.pquestions[&q1].recheck_at;
    w.at(at);
    w.settle();
    assert_eq!(w.s.pquestions[&q1].rechecks, 1);
    assert!(w.s.nodes[&a].recheck_marks.contains(&q1));
    let t = created_token(&mut w, &c, s, b);
    ok(w.call(&c, s, waiting_release(t, q1)));
    let d = ok(w.call(&c, s, ask_cmd(n, None, false, true)));
    let (q2, ..) = asked(&d.reply);
    assert_eq!(w.s.pquestions[&q1].state, PqState::Superseded { by: q2 });
    assert_eq!(
        w.s.nodes[&b].wait.as_ref().map(|x| x.on),
        Some(WaitOn::Question {
            q: q2,
            rechecks_at_set: 0
        })
    );
    // `a`'s wait resolved at the recheck: it stays on q1.
    assert_eq!(w.s.nodes[&a].wait.as_ref().map(|x| x.on), Some(wait_a));
}

/// Takes `n` and returns the token.
/// 1.11 (§9.2): a hold of `s` on a node of its own, `/busy-<s>`, so that the `said` entries it
/// records stay pending until its release or close, as a holder's do; one recorded by a session
/// that holds nothing is confirmed at once.
fn busy(w: &mut World, c: &ClassName, s: SessionId) -> Token {
    let n = node(w, c, s, &alloc::format!("/busy-{}", s.0), &[]);
    created_token(w, c, s, n)
}

fn created_token(w: &mut World, c: &ClassName, s: SessionId, n: NodeId) -> Token {
    let d = ok(w.call(c, s, take(n)));
    match d.reply {
        Reply::Taken { token, .. } => token,
        other => panic!("{other:?}"),
    }
}

/// A release that waits for people question `q`.
fn waiting_release(t: Token, q: Eid) -> Cmd {
    let mut cmd = release(t, ReleaseReason::Blocked);
    if let Cmd::Hold(HoldCmd::Release { waits_for, .. }) = &mut cmd {
        *waits_for = Some(WaitArg::Question(q));
    }
    cmd
}

#[test]
fn a_release_confirms_the_asks_said() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let t = created_token(&mut w, &c, s, n);
    let d = ok(w.call(&c, s, ask_cmd(n, None, false, false)));
    let (q, a, ..) = asked(&d.reply);
    ok(w.call(&c, s, waiting_release(t, q)));
    assert_eq!(w.s.saids[&a.unwrap()].state, SaidState::Confirmed);
    assert_eq!(
        w.s.pquestions[&q].recheck_at,
        w.clock.plus(w.s.params.question_recheck)
    );
}

/// 1.5 (§9.2, §25; the final usability round): a release or a close confirms the session's pending
/// `said` entries, and those it lists in `unsent` become unconfirmed at once, in ascending order;
/// the node, once unattended, carries `unconfirmed_said`, and an entry already confirmed stays so.
#[test]
fn a_release_or_a_close_leaves_the_unsent_unconfirmed() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let t = created_token(&mut w, &c, s, n);
    let (sent, ..) = said_reply(&ok(w.call(&c, s, said_cmd(n, None, SaidKind::Ack))).reply);
    let (kept, ..) = said_reply(&ok(w.call(&c, s, said_cmd(n, None, SaidKind::Reply))).reply);
    let mut cmd = release(t, ReleaseReason::Handoff);
    if let Cmd::Hold(HoldCmd::Release { unsent, .. }) = &mut cmd {
        *unsent = [kept].into();
    }
    ok(w.call(&c, s, cmd));
    let said_evs: Vec<Ev> = evs(&w)
        .into_iter()
        .filter(|e| matches!(e, Ev::SaidConfirmed { .. } | Ev::SaidUnconfirmed { .. }))
        .collect();
    assert_eq!(
        said_evs,
        vec![
            Ev::SaidConfirmed {
                said: sent,
                via: crate::event::ConfirmVia::Release,
                link: None,
            },
            Ev::SaidUnconfirmed { said: kept },
        ]
    );
    assert_eq!(w.s.saids[&sent].state, SaidState::Confirmed);
    assert_eq!(w.s.saids[&kept].state, SaidState::Unconfirmed);
    assert!(w.s.ix.node[&n].flags.contains(Flag::UnconfirmedSaid));
    // The same by a close, which also names an entry it confirmed before: that one stays
    // confirmed.
    let m = node(&mut w, &c, s, "/billing", &["chat:b/1"]);
    let t = created_token(&mut w, &c, s, m);
    let (late, ..) = said_reply(&ok(w.call(&c, s, said_cmd(m, None, SaidKind::Result))).reply);
    let mut cmd = super::super::keys::tests::close_cmd(m, Outcome::Done);
    if let Cmd::Node(NodeCmd::Close { unsent, token, .. }) = &mut cmd {
        *unsent = [late, sent].into();
        *token = Some(t);
    }
    ok(w.call(&c, s, cmd));
    assert_eq!(w.s.saids[&late].state, SaidState::Unconfirmed);
    assert_eq!(w.s.saids[&sent].state, SaidState::Confirmed);
    assert!(w.s.ix.node[&m].flags.contains(Flag::UnconfirmedSaid));
    // Whoever checks the conversation confirms it, and the flag clears.
    let s2 = w.hello(&c);
    ok(w.call(&c, s2, confirm_cmd(kept)));
    assert!(!w.s.ix.node[&n].flags.contains(Flag::UnconfirmedSaid));
}

// ------------------------------------------------------------------------------------------------
// The Recheck firing (D.8.2)

#[test]
fn a_recheck_that_is_not_the_last_marks_the_waiters_or_the_conversation_node() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let d = ok(w.call(&c, s, ask_cmd(n, None, false, false)));
    let (q, ..) = asked(&d.reply);
    // No waiter: the open conversation node is marked (A-81).
    let at = w.s.pquestions[&q].recheck_at;
    w.at(at);
    let fired = w.settle();
    assert!(fired.iter().any(|e| matches!(
        e.ev,
        Ev::PqRechecked { q: x, rechecks: 1, .. } if x == q
    )));
    assert!(w.s.nodes[&n].recheck_marks.contains(&q));
    assert!(w.s.ix.node[&n].flags.contains(Flag::QuestionRecheck));
    assert_eq!(
        w.s.pquestions[&q].recheck_at,
        at.plus(w.s.params.question_recheck)
    );
    // An answer clears the mark.
    ok(w.call(&c, s, heard_cmd(n, None, Some(q))));
    assert!(!w.s.nodes[&n].recheck_marks.contains(&q));
    assert!(!w.s.ix.node[&n].flags.contains(Flag::QuestionRecheck));
    // A waiter: the recheck marks the node whose wait it resolved, and not the conversation node
    // (§9.3, A-81).
    let d = ok(w.call(&c, s, ask_cmd(n, None, false, true)));
    let (q2, ..) = asked(&d.reply);
    let a = node(&mut w, &c, s, "/support/a", &[]);
    let t = created_token(&mut w, &c, s, a);
    ok(w.call(&c, s, waiting_release(t, q2)));
    let at = w.s.pquestions[&q2].recheck_at;
    w.at(at);
    let fired = w.settle();
    let marked: Vec<NodeId> = fired
        .iter()
        .filter_map(|e| match e.ev {
            Ev::MarkSet {
                node,
                question,
                on: true,
                ..
            } if question == q2 => Some(node),
            _ => None,
        })
        .collect();
    assert_eq!(marked, vec![a]);
    assert!(w.s.nodes[&a].recheck_marks.contains(&q2));
    assert!(w.s.ix.node[&a].flags.contains(Flag::QuestionRecheck));
    assert!(!w.s.nodes[&n].recheck_marks.contains(&q2));
    assert!(!w.s.ix.node[&n].flags.contains(Flag::QuestionRecheck));
}

#[test]
fn a_recheck_with_a_closed_conversation_node_marks_nothing() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let d = ok(w.call(&c, s, ask_cmd(n, None, false, false)));
    let (q, ..) = asked(&d.reply);
    ok(w.call(&c, s, crate::apply::tests::close(n, Outcome::Done)));
    let at = w.s.pquestions[&q].recheck_at;
    w.at(at);
    let fired = w.settle();
    assert!(fired.iter().all(|e| !matches!(e.ev, Ev::MarkSet { .. })));
    assert_eq!(w.s.pquestions[&q].rechecks, 1);
}

#[test]
fn the_last_recheck_of_a_non_blocking_question_expires_it() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let d = ok(w.call(&c, s, ask_cmd(n, None, false, false)));
    let (q, ..) = asked(&d.reply);
    for _ in 0..w.s.params.question_max_rechecks {
        let at = w.s.pquestions[&q].recheck_at;
        w.at(at);
        w.settle();
    }
    assert_eq!(w.s.pquestions[&q].state, PqState::Expired);
    assert!(w.s.nodes[&n].assume_marks.contains(&q));
    // A decision records the assumption.
    let d = ok(w.call(&c, s, decide_cmd(n, &[])));
    let Reply::Decided { decision, .. } = d.reply else {
        panic!()
    };
    assert_eq!(w.s.pquestions[&q].assumed_by, Some(decision));
    assert!(!w.s.nodes[&n].assume_marks.contains(&q));
}

#[test]
fn the_last_recheck_of_a_blocking_question_escalates_it() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let work = node(&mut w, &c, s, "/support/work", &[]);
    let d = ok(w.call(&c, s, ask_cmd(n, None, true, false)));
    let (q, ..) = asked(&d.reply);
    for _ in 1..w.s.params.question_max_rechecks {
        let at = w.s.pquestions[&q].recheck_at;
        w.at(at);
        w.settle();
    }
    let t = created_token(&mut w, &c, s, work);
    ok(w.call(&c, s, waiting_release(t, q)));
    let at = w.s.pquestions[&q].recheck_at;
    w.at(at);
    w.settle();
    let PqState::Escalated { uq } = w.s.pquestions[&q].state else {
        panic!("{:?}", w.s.pquestions[&q].state)
    };
    assert!(w.s.uquestions.contains_key(&uq));
    assert_eq!(
        w.s.nodes[&work].wait.as_ref().map(|x| x.on),
        Some(WaitOn::UserQuestion(uq))
    );
    assert!(!w.s.nodes[&work].recheck_marks.contains(&q));
}

// ------------------------------------------------------------------------------------------------
// C1 through the commands: every entry about a conversation on its conversation node

#[test]
fn every_people_entry_lies_on_the_conversation_node() {
    // The step check of C1 runs after each call; here the entries of every command land on the
    // node that carries the key, from a child two levels down, and a decision's helper builds one
    // by events alone for comparison.
    let (mut w, c, s) = one();
    let conv = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let deep = node(&mut w, &c, s, "/support/a/b", &[]);
    let (r, rn, _) =
        request_of(&ok(w.call(&c, s, request_cmd(deep, "chat:a/1/m1", Some("chat:a/1")))).reply);
    assert_eq!(rn, conv);
    let (a, an, ..) = said_reply(&ok(w.call(&c, s, said_cmd(deep, None, SaidKind::Ack))).reply);
    assert_eq!(an, conv);
    let (q, _, qn, _) = asked(&ok(w.call(&c, s, ask_cmd(deep, None, false, false))).reply);
    assert_eq!(qn, conv);
    let Reply::Heard { node: hn, .. } = ok(w.call(&c, s, heard_cmd(deep, None, None))).reply else {
        panic!()
    };
    assert_eq!(hn, conv);
    for e in [r, a, q] {
        assert_eq!(w.s.entries[&e].node, conv);
    }
    // A decision by its event alone lists the request; the entries stay where they are.
    let d = decision(&mut w, s, conv, &[r]);
    assert_eq!(w.s.decisions[&d].node, conv);
}

#[test]
fn a_research_node_records_a_said_on_its_conversation_key() {
    // CS-36: `lean:` names an object, not a conversation, so the node's one conversation key is
    // `chat:a/1`.
    let (mut w, c, s) = one();
    let n = node(
        &mut w,
        &c,
        s,
        "/lemma",
        &["lean:Mathlib.Foo.bar", "chat:a/1"],
    );
    let d = ok(w.call(&c, s, said_cmd(n, None, SaidKind::Ack)));
    let (a, node_, ..) = said_reply(&d.reply);
    assert_eq!(node_, n);
    assert_eq!(w.s.saids[&a].conv, ks("chat:a/1"));
}

#[test]
fn ask_refusals_in_order() {
    let (mut w, c, s) = one();
    let plain = node(&mut w, &c, s, "/plain", &[]);
    let two = node(&mut w, &c, s, "/two", &["chat:b/1", "mail:m@x"]);
    refused(
        &mut w,
        &c,
        s,
        ask_cmd(NodeId(999), None, false, false),
        nf(What::Node),
    );
    refused(
        &mut w,
        &c,
        s,
        ask_cmd(plain, Some("repeat:1:20261001T0000Z"), false, false),
        inv(Why::ReservedKey),
    );
    refused(
        &mut w,
        &c,
        s,
        ask_cmd(plain, None, false, false),
        inv(Why::NoConversationKey),
    );
    refused(
        &mut w,
        &c,
        s,
        ask_cmd(two, None, false, false),
        inv(Why::AmbiguousConversationKey),
    );
    refused(
        &mut w,
        &c,
        s,
        ask_cmd(USER, Some("chat:z/1"), true, false),
        inv(Why::ReservedNode),
    );
    refused(
        &mut w,
        &c,
        s,
        heard_cmd(two, None, None),
        inv(Why::AmbiguousConversationKey),
    );
    refused(
        &mut w,
        &c,
        s,
        heard_cmd(USER, Some("chat:z/1"), None),
        inv(Why::ReservedNode),
    );
}

#[test]
fn precheck_agrees_with_apply_for_the_people_and_agent_commands() {
    // D.8.25: `query::precheck` answers the session and target errors as `apply` does.
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let gone = NodeId(999);
    let cmds = vec![
        request_cmd(gone, "chat:a/1/m1", None),
        request_cmd(n, "chat:a/1/m1", None),
        said_cmd(gone, None, SaidKind::Ack),
        said_cmd(n, None, SaidKind::Ack),
        heard_cmd(gone, None, None),
        ask_cmd(gone, None, false, false),
        ask_cmd(n, None, false, false),
        decide_cmd(gone, &[]),
        decide_cmd(n, &[]),
        confirm_cmd(Eid(9_999)),
        Cmd::Talk(TalkCmd::Say {
            at: id(gone),
            text: text("x"),
            to: None,
            expects: None,
            reply_to: None,
            until: None,
        }),
        Cmd::Talk(TalkCmd::Ask {
            at: id(gone),
            text: text("x"),
            to: AskTo::Agent(AgentAddressee::Session(s)),
            again: false,
        }),
        Cmd::Talk(TalkCmd::Answer {
            question: Eid(9_999),
            node: NodeId(u64::MAX),
            asker: None,
            text: text("x"),
        }),
        Cmd::Talk(TalkCmd::Approve {
            on: id(gone),
            version: Version(1),
            checked: text("x"),
        }),
        Cmd::Talk(TalkCmd::Object {
            on: id(gone),
            note: text("x"),
        }),
        Cmd::Talk(TalkCmd::Resolve {
            objection: Eid(9_999),
            note: text("x"),
            upheld: false,
            defer: false,
        }),
        Cmd::Talk(TalkCmd::Retract {
            target: RetractArg::Node(id(gone)),
            text: text("x"),
            reason: text("y"),
        }),
        Cmd::Talk(TalkCmd::Retract {
            target: RetractArg::Entry {
                eid: Eid(9_999),
                node: NodeId(u64::MAX),
            },
            text: text("x"),
            reason: text("y"),
        }),
        Cmd::Talk(TalkCmd::Wait {
            timeout: Dur(0),
            for_: Some(id(gone)),
            since: None,
        }),
        Cmd::Talk(TalkCmd::Who { at: Some(id(gone)) }),
        Cmd::Session(SessionCmd::Status { text: text("x") }),
    ];
    for cmd in cmds {
        for (cls, sc) in [
            (c.clone(), SessionCtx::Existing(s)),
            (c.clone(), SessionCtx::Existing(SessionId(77))),
            (class("other"), SessionCtx::Existing(s)),
        ] {
            let ctx = w.ctx(&cls, sc, None);
            let pre = crate::query::precheck(&w.s, &ctx, &cmd);
            let (_, out) = crate::apply::decide(&w.s, &ctx, &cmd);
            match &pre {
                Some(e) => assert_eq!(out.as_ref().err(), Some(e), "{cmd:?}"),
                None => assert!(
                    !matches!(
                        &out,
                        Err(crate::error::Error {
                            hint: crate::error::Hint::NotFound(What::Session | What::Node),
                            ..
                        })
                    ),
                    "{cmd:?}: {out:?}"
                ),
            }
        }
    }
}

#[path = "talk_people_questions_tests.rs"]
mod questions;
