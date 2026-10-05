//! Unit tests of WP-D5b's agent commands (design D.12.1): `talk say`, `ask` to agents, `answer`,
//! `wait`, `who`, `session status`, and the helpers `recipients`, `inbox`, and
//! `conversation_holder` (D.8.16, D.8.26, §13). Every call runs through `World::call_ctx` with
//! every check of a step and of the state.

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

use super::super::keys::tests::{evs, id, nf, node, one};
use crate::apply::tests::{class, entry, ok, refused, take, text};
use crate::cmd::*;
use crate::error::{What, Why};
use crate::event::{EntryBody, Ev};
use crate::helpers::agents::{conversation_holder, inbox, recipients};
use crate::query;
use crate::reply::{Reply, WarningCode, WarningDetail};
use crate::state::{AqState, Author};
use crate::types::*;

// ------------------------------------------------------------------------------------------------
// Commands

fn say(n: NodeId, to: Option<AddresseeArg>) -> Cmd {
    Cmd::Talk(TalkCmd::Say {
        at: id(n),
        text: text("the parser is fixed"),
        to,
        expects: None,
        reply_to: None,
        until: None,
    })
}

fn ask_agent(n: NodeId, to: AgentAddressee) -> Cmd {
    Cmd::Talk(TalkCmd::Ask {
        at: id(n),
        text: text("did the rerun pass?"),
        to: AskTo::Agent(to),
        again: false,
    })
}

fn answer(q: Eid, node: NodeId, asker: Option<SessionId>) -> Cmd {
    Cmd::Talk(TalkCmd::Answer {
        question: q,
        node,
        asker,
        text: text("yes"),
    })
}

fn wait(for_: Option<NodeId>, since: Option<Seq>) -> Cmd {
    Cmd::Talk(TalkCmd::Wait {
        timeout: Dur(0),
        for_: for_.map(id),
        since,
    })
}

fn hello() -> Cmd {
    Cmd::Session(SessionCmd::Hello {
        intent: None,
        caps: None,
        horizon: None,
        principal: None,
        from_user: None,
    })
}

fn said_entry(d: &crate::reply::Done) -> Eid {
    match d.reply {
        Reply::Say { entry } => entry,
        ref other => panic!("not a say: {other:?}"),
    }
}

fn asked(d: &crate::reply::Done) -> Eid {
    match d.reply {
        Reply::Asked { question, .. } => question,
        ref other => panic!("not asked: {other:?}"),
    }
}

fn delivered(d: &crate::reply::Done) -> Vec<Eid> {
    match &d.reply {
        Reply::Waited { delivered, .. } => delivered.clone(),
        Reply::Hello { inbox, .. } => inbox.clone(),
        other => panic!("{other:?}"),
    }
}

// ------------------------------------------------------------------------------------------------
// session status and talk who

#[test]
fn session_status_records_its_text() {
    let (mut w, c, s) = one();
    let d = ok(w.call(
        &c,
        s,
        Cmd::Session(SessionCmd::Status {
            text: text("rerunning the tests"),
        }),
    ));
    assert_eq!(d.reply, Reply::Status);
    assert!(evs(&w).iter().any(|e| matches!(
        e,
        Ev::SessionStatus { session, .. } if *session == s
    )));
}

#[test]
fn who_reads_and_refuses_an_unknown_node() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/work", &[]);
    refused(
        &mut w,
        &c,
        s,
        Cmd::Talk(TalkCmd::Who {
            at: Some(id(NodeId(999))),
        }),
        nf(What::Node),
    );
    let d = ok(w.call(&c, s, Cmd::Talk(TalkCmd::Who { at: Some(id(n)) })));
    assert_eq!(d.reply, Reply::Who { at: Some(n) });
    assert!(evs(&w).iter().all(|e| matches!(e, Ev::Touched { .. })));
    let d = ok(w.call(&c, s, Cmd::Talk(TalkCmd::Who { at: None })));
    assert_eq!(d.reply, Reply::Who { at: None });
}

// ------------------------------------------------------------------------------------------------
// talk say

#[test]
fn say_refusals_in_order() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/work", &[]);
    refused(&mut w, &c, s, say(NodeId(999), None), nf(What::Node));
    // An unknown session in `to` is `not_found(Addressee)`, not the caller's session (R12).
    refused(
        &mut w,
        &c,
        s,
        say(n, Some(AddresseeArg::Session(SessionId(99)))),
        nf(What::Addressee),
    );
    refused(
        &mut w,
        &c,
        s,
        say(n, Some(AddresseeArg::Holder(id(NodeId(999))))),
        nf(What::Node),
    );
    refused(
        &mut w,
        &c,
        s,
        say(n, Some(AddresseeArg::Holders(id(NodeId(999))))),
        nf(What::Node),
    );
    let mut reply = say(n, None);
    if let Cmd::Talk(TalkCmd::Say { reply_to, .. }) = &mut reply {
        *reply_to = Some(Eid(9_999));
    }
    refused(&mut w, &c, s, reply, nf(What::Entry));
    // The addressee before `reply_to`.
    let mut both = say(n, Some(AddresseeArg::Session(SessionId(99))));
    if let Cmd::Talk(TalkCmd::Say { reply_to, .. }) = &mut both {
        *reply_to = Some(Eid(9_999));
    }
    refused(&mut w, &c, s, both, nf(What::Addressee));
}

#[test]
fn say_and_ask_to_a_retired_session_are_not_found_addressee() {
    let (mut w, c, s) = one();
    let gone = w.hello(&c);
    let n = node(&mut w, &c, s, "/work", &[]);
    // `gone` retires after `session.retain` without a call; `s` stays by calling.
    let retain = w.s.params.session_retain;
    let step = w.s.params.presence_silence;
    let mut t = Dur(0);
    while t.0 <= retain.0 {
        w.advance(step);
        t = Dur(t.0 + step.0);
        ok(w.call(&c, s, Cmd::Talk(TalkCmd::Who { at: None })));
    }
    w.advance(Dur(1));
    assert!(!w.s.sessions.contains_key(&gone));
    refused(
        &mut w,
        &c,
        s,
        say(n, Some(AddresseeArg::Session(gone))),
        nf(What::Addressee),
    );
    refused(
        &mut w,
        &c,
        s,
        ask_agent(n, AgentAddressee::Session(gone)),
        nf(What::Addressee),
    );
}

#[test]
fn a_plain_note_and_a_session_note() {
    let (mut w, c, s) = one();
    let s2 = w.hello(&c);
    let n = node(&mut w, &c, s, "/work", &[]);
    // A plain note: no addressee, no recipient, delivered to nobody. 1.10 (§13): that is a note
    // at a node nobody can hold, such as the root, or one only its sender holds.
    let d = ok(w.call(&c, s, say(ROOT, None)));
    let e = said_entry(&d);
    assert!(!w.s.notes.contains_key(&e));
    assert!(inbox(&w.s, s2, w.clock).is_empty());
    // 1.10 (§13): at a node nobody holds that can be held, a note to its next holder.
    let d = ok(w.call(&c, s, say(n, None)));
    let e = said_entry(&d);
    assert_eq!(w.s.notes[&e].to, Addressee::Holder(n));
    assert!(w.s.notes[&e].recipients.is_empty());
    assert!(w.s.notes[&e].pending_holder);
    assert!(inbox(&w.s, s2, w.clock).is_empty());
    // A note to a session: delivered by its next wait, once.
    let d = ok(w.call(&c, s, say(n, Some(AddresseeArg::Session(s2)))));
    let x = said_entry(&d);
    assert_eq!(w.s.notes[&x].recipients, BTreeSet::from([s2]));
    assert_eq!(w.s.notes[&x].until, w.clock.plus(w.s.params.deliver_window));
    assert_eq!(inbox(&w.s, s2, w.clock), vec![x]);
    let d = ok(w.call(&c, s2, wait(None, None)));
    assert_eq!(delivered(&d), vec![x]);
    assert!(inbox(&w.s, s2, w.clock).is_empty());
    // `session:<self>` reaches its own sender (R12 skips only `holder:` and `holders:`).
    let d = ok(w.call(&c, s, say(n, Some(AddresseeArg::Session(s)))));
    let own = said_entry(&d);
    assert_eq!(inbox(&w.s, s, w.clock), vec![own]);
}

#[test]
fn a_note_expires_at_its_until() {
    let (mut w, c, s) = one();
    let s2 = w.hello(&c);
    let n = node(&mut w, &c, s, "/work", &[]);
    let mut cmd = say(n, Some(AddresseeArg::Session(s2)));
    if let Cmd::Talk(TalkCmd::Say { until, .. }) = &mut cmd {
        *until = Some(When::In(Dur(60_000)));
    }
    let x = said_entry(&ok(w.call(&c, s, cmd)));
    assert_eq!(w.s.notes[&x].until, w.clock.plus(Dur(60_000)));
    w.advance(Dur(60_000));
    assert!(inbox(&w.s, s2, w.clock).is_empty());
    // An `until` at or before now is born live and expires at once (T3).
    let mut past = say(n, Some(AddresseeArg::Session(s2)));
    if let Cmd::Talk(TalkCmd::Say { until, .. }) = &mut past {
        *until = Some(When::At(Time(0)));
    }
    let y = said_entry(&ok(w.call(&c, s, past)));
    assert_eq!(w.s.notes[&y].until, w.clock.plus(Dur(1)));
    w.advance(Dur(1));
    assert!(!w.s.notes.contains_key(&y) || inbox(&w.s, s2, w.clock).is_empty());
}

#[test]
fn say_with_expects_warns_that_it_is_ignored() {
    // 1.4, §13, R17.
    let (mut w, c, s) = one();
    let s2 = w.hello(&c);
    let n = node(&mut w, &c, s, "/work", &[]);
    for (e, warned) in [
        (Expects::Ack, true),
        (Expects::Reply, true),
        (Expects::None, false),
    ] {
        let mut cmd = say(n, Some(AddresseeArg::Session(s2)));
        if let Cmd::Talk(TalkCmd::Say { expects, .. }) = &mut cmd {
            *expects = Some(e);
        }
        let d = ok(w.call(&c, s, cmd));
        let x = said_entry(&d);
        assert!(w.s.notes.contains_key(&x), "recorded as given");
        let ws: Vec<_> = d
            .warnings
            .iter()
            .filter(|w| w.code == WarningCode::ExpectsIgnored)
            .collect();
        if warned {
            assert_eq!(ws.len(), 1);
            assert_eq!(ws[0].detail, WarningDetail::ExpectsIgnored { expects: e });
        } else {
            assert!(ws.is_empty());
        }
    }
}

#[test]
fn holder_notes_skip_their_sender() {
    // 1.4, §13, R12: `holder:` goes to the other holders, or waits for the next other taker.
    let (mut w, c, s) = one();
    let s2 = w.hello(&c);
    let s3 = w.hello(&c);
    let n = {
        let d = ok(w.call(
            &c,
            s,
            crate::apply::tests::create_with("/work", |a| {
                a.capacity = Some(2);
                a.take = Some(false);
            }),
        ));
        crate::apply::tests::created(&d).0
    };
    ok(w.call(&c, s, take(n)));
    // The only holder writes to `holder:` of its own node: nobody but it holds, so the note waits.
    let d = ok(w.call(&c, s, say(n, Some(AddresseeArg::Holder(id(n))))));
    let x = said_entry(&d);
    assert!(w.s.notes[&x].recipients.is_empty());
    assert!(w.s.notes[&x].pending_holder);
    assert!(inbox(&w.s, s, w.clock).is_empty());
    // The next other taker gets it, in the response to its take: notes ride on writes (1.7).
    let d = ok(w.call(&c, s2, take(n)));
    assert_eq!(w.s.notes[&x].recipients, BTreeSet::from([s2]));
    assert_eq!(d.delivered, vec![x]);
    assert!(inbox(&w.s, s2, w.clock).is_empty());
    // With another holder, the note goes to it alone.
    let d = ok(w.call(&c, s2, say(n, Some(AddresseeArg::Holder(id(n))))));
    let y = said_entry(&d);
    assert_eq!(w.s.notes[&y].recipients, BTreeSet::from([s]));
    assert!(!w.s.notes[&y].pending_holder);
    // An agent question to `holder:` too.
    let d = ok(w.call(&c, s, ask_agent(n, AgentAddressee::Holder(id(n)))));
    let q = asked(&d);
    assert_eq!(w.s.notes[&q].recipients, BTreeSet::from([s2]));
    assert_eq!(w.s.aquestions[&q].asker, s);
    let _ = s3;
}

#[test]
fn a_note_without_to_at_a_node_others_hold_reaches_its_holders() {
    // 1.9 (§13, l6 of the lab's fourth round): five plain notes to the lead's node reached nobody.
    let (mut w, c, s) = one();
    let s2 = w.hello(&c);
    let n = {
        let d = ok(w.call(
            &c,
            s,
            crate::apply::tests::create_with("/work", |a| {
                a.capacity = Some(2);
                a.take = Some(false);
            }),
        ));
        crate::apply::tests::created(&d).0
    };
    // Nobody holds it: 1.10 (§13, l7 of the lab's fifth round), a note to its next holder, which
    // the take binds and delivers in its response.
    let x = said_entry(&ok(w.call(&c, s2, say(n, None))));
    assert_eq!(w.s.notes[&x].to, Addressee::Holder(n));
    assert!(w.s.notes[&x].pending_holder);
    let d = ok(w.call(&c, s, take(n)));
    assert_eq!(w.s.notes[&x].recipients, BTreeSet::from([s]));
    assert_eq!(d.delivered, vec![x]);
    // Another session holds it: `holder:<n>`, delivered to that holder at once.
    let d = ok(w.call(&c, s2, say(n, None)));
    let y = said_entry(&d);
    assert_eq!(w.s.notes[&y].to, Addressee::Holder(n));
    assert_eq!(w.s.notes[&y].recipients, BTreeSet::from([s]));
    assert!(!w.s.notes[&y].pending_holder);
    assert_eq!(inbox(&w.s, s, w.clock), vec![y]);
    // Its only holder writing there: a plain note, as before.
    let z = said_entry(&ok(w.call(&c, s, say(n, None))));
    assert!(!w.s.notes.contains_key(&z));
    // Two holders: each reaches the other.
    ok(w.call(&c, s2, take(n)));
    let v = said_entry(&ok(w.call(&c, s, say(n, None))));
    assert_eq!(w.s.notes[&v].recipients, BTreeSet::from([s2]));
}

#[test]
fn a_holder_note_its_sender_takes_over_stays_pending_for_the_next_other_taker() {
    let (mut w, c, s) = one();
    let s2 = w.hello(&c);
    let n = node(&mut w, &c, s, "/work", &[]);
    let d = ok(w.call(&c, s, say(n, Some(AddresseeArg::Holder(id(n))))));
    let x = said_entry(&d);
    assert!(w.s.notes[&x].pending_holder);
    // The sender takes the node: the note is not bound to it.
    let t = match ok(w.call(&c, s, take(n))).reply {
        Reply::Taken { token, .. } => token,
        other => panic!("{other:?}"),
    };
    assert!(w.s.notes[&x].pending_holder);
    assert!(inbox(&w.s, s, w.clock).is_empty());
    ok(w.call(
        &c,
        s,
        crate::apply::tests::release(t, ReleaseReason::Handoff),
    ));
    ok(w.call(&c, s2, take(n)));
    assert_eq!(w.s.notes[&x].recipients, BTreeSet::from([s2]));
}

#[test]
fn holders_notes_are_read_at_read_time_once_per_session_and_never_by_their_sender() {
    // D-6, R12: recorded with no recipients; delivered to whoever holds in the branch when it
    // reads, a later taker included; one cursor per scope.
    let (mut w, c, s) = one();
    let s2 = w.hello(&c);
    let s3 = w.hello(&c);
    let a = node(&mut w, &c, s, "/a", &[]);
    let a1 = node(&mut w, &c, s, "/a/one", &[]);
    let a2 = node(&mut w, &c, s, "/a/two", &[]);
    let b = node(&mut w, &c, s, "/b", &[]);
    let b1 = node(&mut w, &c, s, "/b/one", &[]);
    ok(w.call(&c, s, take(a1)));
    ok(w.call(&c, s2, take(a2)));
    // `s` writes to `holders:/a`: it holds there, and never receives its own note.
    let d = ok(w.call(&c, s, say(a, Some(AddresseeArg::Holders(id(a))))));
    let x = said_entry(&d);
    assert!(w.s.notes[&x].recipients.is_empty());
    assert!(inbox(&w.s, s, w.clock).is_empty());
    assert_eq!(inbox(&w.s, s2, w.clock), vec![x]);
    // A newer note of another branch, read first, does not hide the older one of `/a` from a
    // session that enters `/a` later (one cursor per scope).
    ok(w.call(&c, s3, take(b1)));
    // `s2`'s write delivers it `x` (1.7, §13), and not its own new note.
    let d = ok(w.call(&c, s2, say(b, Some(AddresseeArg::Holders(id(b))))));
    let y = said_entry(&d);
    assert_eq!(d.delivered, vec![x]);
    assert!(inbox(&w.s, s2, w.clock).is_empty());
    assert_eq!(delivered(&ok(w.call(&c, s3, wait(None, None)))), vec![y]);
    assert!(inbox(&w.s, s3, w.clock).is_empty());
    let a3 = node(&mut w, &c, s, "/a/three", &[]);
    // The take that brings `s3` into `/a` delivers the older note of `/a`.
    let d = ok(w.call(&c, s3, take(a3)));
    assert_eq!(d.delivered, vec![x]);
    assert!(inbox(&w.s, s3, w.clock).is_empty());
    assert!(delivered(&ok(w.call(&c, s3, wait(None, None)))).is_empty());
    // `s2` read it once.
    assert!(delivered(&ok(w.call(&c, s2, hello()))).is_empty());
    assert!(inbox(&w.s, s2, w.clock).is_empty());
    // The sender's inbox skips its own note without moving its cursor: a later note of `/a` by
    // another session still reaches it.
    let d = ok(w.call(&c, s2, say(a, Some(AddresseeArg::Holders(id(a))))));
    let z = said_entry(&d);
    assert_eq!(inbox(&w.s, s, w.clock), vec![z]);
}

#[test]
fn principal_notes_reach_only_that_persons_sessions() {
    // CS-3, trace C: a `principal:` note reaches the sessions that serve that principal.
    let (mut w, c, s) = one();
    let alice = {
        let ctx = w.ctx(&c, SessionCtx::New, None);
        let d = w
            .call_ctx(
                &ctx,
                &Cmd::Session(SessionCmd::Hello {
                    intent: None,
                    caps: None,
                    horizon: None,
                    principal: Some(PrincipalKey::new("test:alice").unwrap()),
                    from_user: None,
                }),
            )
            .unwrap();
        d.opened.unwrap()
    };
    let other = w.hello(&c);
    let n = node(&mut w, &c, s, "/work", &[]);
    let d = ok(w.call(
        &c,
        s,
        say(
            n,
            Some(AddresseeArg::Principal(
                PrincipalKey::new("test:alice").unwrap(),
            )),
        ),
    ));
    let x = said_entry(&d);
    assert!(w.s.notes[&x].recipients.is_empty());
    assert_eq!(inbox(&w.s, alice, w.clock), vec![x]);
    assert!(inbox(&w.s, other, w.clock).is_empty());
    assert_eq!(delivered(&ok(w.call(&c, alice, wait(None, None)))), vec![x]);
    assert!(inbox(&w.s, alice, w.clock).is_empty());
    // A principal key never seen is interned by the `say`.
    let next = w.s.counters.next_principal;
    ok(w.call(
        &c,
        s,
        say(
            n,
            Some(AddresseeArg::Principal(
                PrincipalKey::new("test:carol").unwrap(),
            )),
        ),
    ));
    assert!(evs(&w).iter().any(|e| matches!(
        e,
        Ev::PrincipalSeen { id, .. } if *id == next
    )));
    // A bare `user` is the first stage of `at`'s chain, `user` without requesters or owners.
    let d = ok(w.call(&c, s, say(n, Some(AddresseeArg::User))));
    let u = said_entry(&d);
    assert_eq!(w.s.notes[&u].to, Addressee::Principal(USER_PRINCIPAL));
}

// ------------------------------------------------------------------------------------------------
// talk ask, to agents, and talk answer

#[test]
fn an_agent_question_is_delivered_answered_and_its_answer_delivered() {
    let (mut w, c, s) = one();
    let s2 = w.hello(&c);
    let n = node(&mut w, &c, s, "/work", &[]);
    let d = ok(w.call(&c, s, ask_agent(n, AgentAddressee::Session(s2))));
    let q = asked(&d);
    assert_eq!(
        d.reply,
        Reply::Asked {
            question: q,
            said: None,
            node: n,
            existing: false,
            keyed: None,
        }
    );
    assert_eq!(w.s.aquestions[&q].state, AqState::Open);
    assert_eq!(delivered(&ok(w.call(&c, s2, wait(None, None)))), vec![q]);
    let d = ok(w.call(&c, s2, answer(q, n, Some(s))));
    let Reply::Answered { entry: a } = d.reply else {
        panic!()
    };
    assert_eq!(w.s.aquestions[&q].state, AqState::Answered { by: a });
    assert_eq!(inbox(&w.s, s, w.clock), vec![a]);
    // A second answer is recorded and changes no state.
    let d = ok(w.call(&c, s2, answer(q, n, Some(s))));
    let Reply::Answered { entry: a2 } = d.reply else {
        panic!()
    };
    assert!(evs(&w).iter().all(|e| !matches!(e, Ev::AqAnswered { .. })));
    assert_eq!(inbox(&w.s, s, w.clock), vec![a, a2]);
}

#[test]
fn an_answer_after_the_question_left_memory_is_recorded() {
    let (mut w, c, s) = one();
    let s2 = w.hello(&c);
    let n = node(&mut w, &c, s, "/work", &[]);
    let q = asked(&ok(w.call(
        &c,
        s,
        ask_agent(n, AgentAddressee::Session(s2)),
    )));
    w.advance(w.s.params.deliver_window);
    assert!(!w.s.aquestions.contains_key(&q));
    // The shell adds the node and the asker from its `entries` table (D.4.12); the sessions of
    // that day may have retired, so a new one answers.
    let s3 = w.hello(&c);
    let d = ok(w.call(&c, s3, answer(q, n, Some(s))));
    let Reply::Answered { entry: a } = d.reply else {
        panic!()
    };
    assert!(evs(&w).iter().all(|e| !matches!(e, Ev::AqAnswered { .. })));
    assert_eq!(w.s.notes[&a].recipients, BTreeSet::from([s]));
}

#[test]
fn answer_refusals() {
    // D.14.6: an eid the shell's table lacks reaches the core with `UNKNOWN_NODE`.
    let unknown = NodeId(u64::MAX);
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/work", &["chat:a/1"]);
    refused(
        &mut w,
        &c,
        s,
        answer(Eid(9_999), n, Some(s)),
        nf(What::Question),
    );
    let q = asked(&ok(w.call(&c, s, ask_agent(n, AgentAddressee::Session(s)))));
    refused(
        &mut w,
        &c,
        s,
        answer(q, unknown, Some(s)),
        nf(What::Question),
    );
    // A person's question is not an agent's (A-36).
    let pq = asked(&ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Ask {
            at: id(n),
            text: text("which tickets?"),
            to: AskTo::Person {
                where_: None,
                blocking: false,
            },
            again: false,
        }),
    )));
    refused(
        &mut w,
        &c,
        s,
        answer(pq, n, Some(s)),
        crate::error::Error::invalid(Why::NotAgentQuestion),
    );
    // Nor is an entry of another type.
    let note = entry(&mut w, n, Author::Session(s), EntryBody::Object);
    refused(
        &mut w,
        &c,
        s,
        answer(note, n, Some(s)),
        crate::error::Error::invalid(Why::NotAgentQuestion),
    );
}

#[test]
fn a_first_retract_or_answer_naming_a_missing_entry_opens_its_session() {
    // D.14.6: the refusal comes after the session check, so a first call opens its session.
    let (mut w, c, _) = one();
    let unknown = NodeId(u64::MAX);
    for cmd in [
        answer(Eid(9_999), unknown, None),
        Cmd::Talk(TalkCmd::Retract {
            target: RetractArg::Entry {
                eid: Eid(9_999),
                node: unknown,
            },
            text: text("t"),
            reason: text("r"),
        }),
    ] {
        let ctx = w.ctx(&c, SessionCtx::New, None);
        let next = w.s.counters.next_session;
        let out = w.call_ctx(&ctx, &cmd);
        let e = out.unwrap_err();
        assert_eq!(e.code, crate::error::ErrorCode::NotFound);
        assert!(w.s.sessions.contains_key(&next), "the session opened");
        assert!(
            w.last_events
                .iter()
                .all(|e| matches!(e.ev, Ev::SessionOpened { .. }))
        );
    }
}

// ------------------------------------------------------------------------------------------------
// talk wait

#[test]
fn wait_delivers_and_reports_news_without_marking_it_shown() {
    let (mut w, c, s) = one();
    let s2 = w.hello(&c);
    let n = node(&mut w, &c, s, "/work", &[]);
    refused(
        &mut w,
        &c,
        s2,
        wait(Some(NodeId(999)), None),
        nf(What::Node),
    );
    // No news, nothing delivered: `{timeout}`, and nothing appended but presence.
    let d = ok(w.call(&c, s2, wait(Some(n), Some(w.s.nodes[&n].seq))));
    assert_eq!(
        d.reply,
        Reply::Waited {
            delivered: Vec::new(),
            news: None
        }
    );
    // New entries on the node are news; no `Shown` is appended (A-85, CS-14).
    ok(w.call(&c, s, say(n, None)));
    let seq = w.s.nodes[&n].seq;
    let d = ok(w.call(&c, s2, wait(Some(n), None)));
    assert_eq!(
        d.reply,
        Reply::Waited {
            delivered: Vec::new(),
            news: Some((n, seq))
        }
    );
    assert!(evs(&w).iter().all(|e| !matches!(e, Ev::Shown { .. })));
    // A second wait after the news, with the seq the shell reported, returns only newer news.
    let d = ok(w.call(&c, s2, wait(Some(n), Some(seq))));
    assert_eq!(
        d.reply,
        Reply::Waited {
            delivered: Vec::new(),
            news: None
        }
    );
    ok(w.call(&c, s, say(n, None)));
    let d = ok(w.call(&c, s2, wait(Some(n), Some(seq))));
    assert_eq!(
        d.reply,
        Reply::Waited {
            delivered: Vec::new(),
            news: Some((n, w.s.nodes[&n].seq))
        }
    );
    assert!(query::inbox_ready(&w.s, s2, Some(n), Some(seq), w.clock));
}

// ------------------------------------------------------------------------------------------------
// The helpers

#[test]
fn recipients_by_addressee() {
    let (mut w, c, s) = one();
    let s2 = w.hello(&c);
    let n = node(&mut w, &c, s, "/work", &[]);
    let now = w.clock;
    assert_eq!(
        recipients(&w.s, &Addressee::Session(s), Some(s), now),
        (BTreeSet::from([s]), false)
    );
    assert_eq!(
        recipients(&w.s, &Addressee::Holder(n), Some(s), now),
        (BTreeSet::new(), true)
    );
    ok(w.call(&c, s2, take(n)));
    let now = w.clock;
    assert_eq!(
        recipients(&w.s, &Addressee::Holder(n), Some(s), now),
        (BTreeSet::from([s2]), false)
    );
    assert_eq!(
        recipients(&w.s, &Addressee::Holder(n), Some(s2), now),
        (BTreeSet::new(), true)
    );
    assert_eq!(
        recipients(&w.s, &Addressee::Holder(n), None, now),
        (BTreeSet::from([s2]), false)
    );
    assert_eq!(
        recipients(&w.s, &Addressee::Holders(ROOT), Some(s), now),
        (BTreeSet::new(), false)
    );
    assert_eq!(
        recipients(&w.s, &Addressee::Principal(USER_PRINCIPAL), Some(s), now),
        (BTreeSet::new(), false)
    );
}

#[test]
fn conversation_holder_takes_the_first_held_node_in_preorder() {
    let (mut w, c, s) = one();
    let s2 = w.hello(&c);
    let s3 = w.hello(&c);
    let conv = node(&mut w, &c, s, "/c", &["chat:a/1"]);
    let x = node(&mut w, &c, s, "/c/x", &[]);
    let k = crate::types::KeyStr::new("chat:a/1").unwrap();
    let x1 = node(&mut w, &c, s, "/c/x/one", &[]);
    let y = node(&mut w, &c, s, "/c/y", &[]);
    assert_eq!(conversation_holder(&w.s, conv, s, &k), None);
    ok(w.call(&c, s3, take(y)));
    ok(w.call(&c, s2, take(x1)));
    // Preorder, children by id: `/c/x` (and below it `/c/x/one`) comes before `/c/y`.
    assert!(x < y);
    assert_eq!(conversation_holder(&w.s, conv, s, &k), Some(s2));
    // A hold on the conversation node itself comes first.
    ok(w.call(&c, s3, take(conv)));
    assert_eq!(conversation_holder(&w.s, conv, s, &k), Some(s3));
    // A holder of the node or a descendant gets None.
    assert_eq!(conversation_holder(&w.s, conv, s2, &k), None);
    assert_eq!(conversation_holder(&w.s, x, s2, &k), None);
    let _ = (class("x"), vec![0]);
}
