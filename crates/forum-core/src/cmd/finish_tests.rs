//! 1.12's finishing, notes, and questions (design D.8.10, D.8.12, D.8.15, D.8.16, D.8.17, D.8.20):
//! a release with `settles` records the decision a `decide` would, after the release (§9.4,
//! §10.3, Settled); a note to `holder:` of a closed node reaches the session that closed it, and
//! one that reaches nobody, or goes to a silent session, says so (§13); a question none of whose
//! stages any session serves warns `no_human_in_reach` (§8.4); a retract of an open question to a
//! person withdraws it (§9.3, §25, E1); a reopen offers its node (§16); and a `said` that answers
//! nothing where no request is warns, while one that gives `at` its conversation key says so
//! (§9.1, §9.2). Every call runs through `World`, with every check of a step, of a firing, and of
//! the state, and G1 on its next calls.

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

use super::keys::tests::{close_cmd, evs, id, ks, nf, node, one};
use super::talk_people::tests::{ask_cmd, heard_cmd, request_cmd, said_cmd};
use crate::apply::tests::{class, create_with, created, ok, release, take, text};
use crate::cmd::*;
use crate::error::What;
use crate::event::{EntryBody, Ev, SystemNote};
use crate::invariants::Inv;
use crate::reply::{NotSettled, NotSettledWhy, Reply, Warning, WarningCode, WarningDetail};
use crate::state::{DeadlineKind, Owner, PqState, WaitOn};
use crate::testkit::World;
use crate::types::*;

// ------------------------------------------------------------------------------------------------
// Commands and readings

fn token_of(d: &crate::reply::Done) -> Token {
    match d.reply {
        Reply::Taken { token, .. } => token,
        ref other => panic!("not taken: {other:?}"),
    }
}

fn request_of(d: &crate::reply::Done) -> Eid {
    match d.reply {
        Reply::Request { request, .. } => request,
        ref other => panic!("not a request: {other:?}"),
    }
}

fn question_of(d: &crate::reply::Done) -> Eid {
    match d.reply {
        Reply::Asked { question, .. } => question,
        ref other => panic!("not asked: {other:?}"),
    }
}

/// `hold release {token, done, next_step, reason: blocked, waits_for?, settles}`.
fn release_settling(t: Token, wait: Option<WaitArg>, settles: &[Eid]) -> Cmd {
    let mut cmd = release(t, ReleaseReason::Blocked);
    if let Cmd::Hold(HoldCmd::Release {
        waits_for,
        settles: s,
        done,
        ..
    }) = &mut cmd
    {
        *waits_for = wait;
        *s = settles.iter().copied().collect();
        *done = text("PR #412 is open; tests pass");
    }
    cmd
}

fn merged_wait() -> Option<WaitArg> {
    Some(WaitArg::External {
        what: text("PR #412 merged"),
        recheck_after: When::In(Dur(3_600_000)),
    })
}

fn say_to(at: NodeId, to: Option<AddresseeArg>) -> Cmd {
    Cmd::Talk(TalkCmd::Say {
        at: id(at),
        text: text("the recovery data: 140 tickets"),
        to,
        expects: None,
        reply_to: None,
        until: None,
    })
}

fn retract(q: Eid, node: NodeId) -> Cmd {
    Cmd::Talk(TalkCmd::Retract {
        target: RetractArg::Entry { eid: q, node },
        text: text("asked the wrong person"),
        reason: text("Omar is not the commit's author"),
    })
}

fn reopen(n: NodeId) -> Cmd {
    Cmd::Node(NodeCmd::Reopen {
        at: id(n),
        reason: text("nothing proves it"),
    })
}

fn warned(d: &crate::reply::Done, code: WarningCode) -> Vec<Warning> {
    d.warnings
        .iter()
        .filter(|x| x.code == code)
        .cloned()
        .collect()
}

/// The decision entries a call appended, with their node and `settles`.
fn decisions(w: &World) -> Vec<(Eid, NodeId, BTreeSet<Eid>)> {
    evs(w)
        .into_iter()
        .filter_map(|e| match e {
            Ev::Appended {
                eid,
                node,
                body: EntryBody::Decision { settles, .. },
                ..
            } => Some((eid, node, settles)),
            _ => None,
        })
        .collect()
}

/// The release's own checkpoint entry in the last call.
fn release_entry(w: &World) -> Eid {
    evs(w)
        .into_iter()
        .find_map(|e| match e {
            Ev::Appended {
                eid,
                body:
                    EntryBody::Checkpoint {
                        release: Some(_), ..
                    },
                ..
            } => Some(eid),
            _ => None,
        })
        .unwrap()
}

// ------------------------------------------------------------------------------------------------
// C1: a release settles (§9.4, §10.3, Settled)

/// it6 l7, l1, l4: finished work that waited for its merge took a `decide` beside the release. A
/// release with `settles` records, after the release, the decision `talk decide {at, text: done,
/// settles}` would: it settles each listed request that has its answer, and names each other one
/// with what it awaits; the decision's later answer settles that one too. Settled's step clause
/// holds on every call (`World`).
#[test]
fn a_release_with_settles_records_the_decision_after_the_release() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/shop/fix-flaky", &["chat:eng/T1"]);
    let t = token_of(&ok(w.call(&c, s, take(n))));
    let r1 = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(n, "chat:eng/T1/M1", Some("chat:eng/T1")),
    )));
    ok(w.call(&c, s, said_cmd(n, Some("chat:eng/T1"), SaidKind::Result)));
    let r2 = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(n, "chat:eng/T1/M2", Some("chat:eng/T1")),
    )));
    let d = ok(w.call(&c, s, release_settling(t, merged_wait(), &[r1, r2])));
    let cp = release_entry(&w);
    let [(dec, at, listed)] = decisions(&w).try_into().unwrap();
    assert!(
        dec > cp,
        "the decision comes after the release's checkpoint"
    );
    assert_eq!(at, n);
    assert_eq!(listed, BTreeSet::from([r1, r2]));
    assert_eq!(w.s.entries[&dec].by, crate::state::Author::Session(s));
    let Reply::Released {
        decision,
        settled,
        not_settled,
        ..
    } = d.reply
    else {
        panic!("{:?}", d.reply)
    };
    assert_eq!(decision, Some(dec));
    assert_eq!(settled, vec![r1]);
    assert_eq!(
        not_settled,
        vec![NotSettled {
            request: r2,
            why: NotSettledWhy::AwaitsSaid {
                node: n,
                conv: ks("chat:eng/T1"),
            },
        }]
    );
    assert!(w.s.requests[&r1].settled.is_some());
    assert!(w.s.requests[&r2].settled.is_none());
    // The release itself: the hold ended, the node waits for its merge.
    assert!(w.s.holds.is_empty());
    assert!(matches!(
        w.s.nodes[&n].wait.as_ref().map(|x| x.on),
        Some(WaitOn::External(_))
    ));
    // r2's answer, later, settles it with the release's decision.
    let s2 = w.hello(&c);
    ok(w.call(&c, s2, said_cmd(n, Some("chat:eng/T1"), SaidKind::Reply)));
    assert!(w.s.requests[&r2].settled.is_some());
}

/// Without `settles`, or with `settles: []`, a release records no decision and settles nothing
/// (1.10's partial reports stand); a listed request that does not exist is `not_found`, after the
/// token's check (H4).
#[test]
fn a_release_without_settles_settles_nothing() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/shop/fix-flaky", &["chat:eng/T1"]);
    let t = token_of(&ok(w.call(&c, s, take(n))));
    let r = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(n, "chat:eng/T1/M1", Some("chat:eng/T1")),
    )));
    ok(w.call(&c, s, said_cmd(n, Some("chat:eng/T1"), SaidKind::Result)));
    // A request that does not exist.
    let e = w
        .call(&c, s, release_settling(t, merged_wait(), &[Eid(9_999)]))
        .unwrap_err();
    assert_eq!(e, nf(What::Request));
    let d = ok(w.call(&c, s, release_settling(t, merged_wait(), &[])));
    assert!(decisions(&w).is_empty());
    assert!(matches!(
        d.reply,
        Reply::Released {
            decision: None,
            ref settled,
            ref not_settled,
            ..
        } if settled.is_empty() && not_settled.is_empty()
    ));
    assert!(w.s.requests[&r].settled.is_none());
    // A stale token is `stale_token` before the requests are read.
    let e = w
        .call(&c, s, release_settling(t, merged_wait(), &[Eid(9_999)]))
        .unwrap_err();
    assert_eq!(e.code, crate::error::ErrorCode::StaleToken);
}

/// A release's decision records the node's assumptions, and on an open signal node closes it as
/// `done`, as any decision on it does (§14, A-34).
#[test]
fn a_release_s_decision_on_a_signal_node_closes_it() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &["chat:x/1"]);
    let b = node(&mut w, &c, s, "/b", &[]);
    let d = ok(w.call(
        &c,
        s,
        Cmd::Node(NodeCmd::Link {
            at: id(b),
            rel: LinkRel::Duplicate,
            to: id(a),
        }),
    ));
    let Reply::Linked {
        signal: Some(sig), ..
    } = d.reply
    else {
        panic!("{:?}", d.reply)
    };
    let r = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(a, "chat:x/1/M1", Some("chat:x/1")),
    )));
    let t = token_of(&ok(w.call(&c, s, take(sig))));
    ok(w.call(&c, s, release_settling(t, None, &[r])));
    assert!(!w.s.nodes[&sig].is_open());
    assert!(evs(&w).iter().any(|e| matches!(
        e,
        Ev::Appended {
            node,
            body: EntryBody::System(SystemNote::Closed { .. }),
            ..
        } if *node == sig
    )));
}

/// 1.12, where groups B and C meet: a `handoff` release with `settles` does both. It records its
/// decision after its checkpoint, which settles the answered request (C1), and it keeps its node
/// from its releaser (`handed_off`, §10.4) while another session's `next` offers it (B5): the
/// decision that follows the release's checkpoint changes neither.
#[test]
fn a_handoff_release_that_settles_keeps_its_node_from_the_releaser() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/fix-flaky", &["chat:eng/T1"]);
    let t = token_of(&ok(w.call(&c, s, take(n))));
    let r = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(n, "chat:eng/T1/M1", Some("chat:eng/T1")),
    )));
    ok(w.call(&c, s, said_cmd(n, Some("chat:eng/T1"), SaidKind::Result)));
    let mut cmd = release_settling(t, None, &[r]);
    if let Cmd::Hold(HoldCmd::Release { reason, .. }) = &mut cmd {
        *reason = ReleaseReason::Handoff;
    }
    let d = ok(w.call(&c, s, cmd));
    let cp = release_entry(&w);
    let [(dec, at, _)] = decisions(&w).try_into().unwrap();
    assert!(dec > cp);
    assert_eq!(at, n);
    assert!(
        matches!(&d.reply, Reply::Released { decision: Some(x), settled, .. }
            if *x == dec && settled == &vec![r]),
        "{:?}",
        d.reply
    );
    assert!(w.s.requests[&r].settled.is_some());
    assert_eq!(w.s.sessions[&s].handed_off.map(|h| h.token), Some(t));
    assert!(crate::state::handed_back(&w.s, s, n, w.clock));
    let first = |w: &World, x: SessionId| {
        crate::query::ranked(
            &w.s,
            x,
            w.clock,
            &NextQuery::default(),
            1,
            &mut crate::query::NoMeter,
        )
        .0
        .first()
        .map(|r| r.cand)
    };
    assert_ne!(first(&w, s), Some(crate::state::Cand::Node(n)));
    let s2 = w.hello(&c);
    assert_eq!(first(&w, s2), Some(crate::state::Cand::Node(n)));
}

// ------------------------------------------------------------------------------------------------
// C2: notes at a closed node (§13)

/// it6 l6: two notes to `holder:` of an incident closed seconds before waited for a next holder
/// while its closer was live. `holder:<path>` and `holder:` at a closed node reach what a note
/// without `to` reaches: the live closer, not the sender; else nobody, with `no_recipient`.
#[test]
fn holder_of_a_closed_node_reaches_its_live_closer() {
    let (mut w, c, s) = one();
    let n = created(&ok(w.call(
        &c,
        s,
        create_with("/incidents/inc-311", |_| {}),
    )))
    .0;
    ok(w.call(&c, s, close_cmd(n, Outcome::Done)));
    let b = w.hello(&c);
    for to in [
        None,
        Some(AddresseeArg::Holder(id(n))),
        Some(AddresseeArg::Holder(id(n))),
    ] {
        let d = ok(w.call(&c, b, say_to(n, to)));
        let Reply::Say { entry } = d.reply else {
            panic!("{:?}", d.reply)
        };
        assert_eq!(w.s.notes[&entry].recipients, BTreeSet::from([s]));
        assert!(!w.s.notes[&entry].pending_holder);
        assert!(warned(&d, WarningCode::NoRecipient).is_empty());
    }
    // The closer's own note reaches nobody, and says so.
    let d = ok(w.call(&c, s, say_to(n, Some(AddresseeArg::Holder(id(n))))));
    let Reply::Say { entry } = d.reply else {
        panic!("{:?}", d.reply)
    };
    assert!(w.s.notes[&entry].recipients.is_empty());
    assert!(!w.s.notes[&entry].pending_holder);
    assert_eq!(
        warned(&d, WarningCode::NoRecipient),
        vec![Warning {
            code: WarningCode::NoRecipient,
            id: Some(n),
            detail: WarningDetail::NoRecipient {
                node: n,
                question: false,
            },
        }]
    );
    // 1.12's review: an agent question to `holder:` of it reaches nobody too, and says it is one.
    let d = ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Ask {
            at: id(n),
            text: text("is the incident over?"),
            to: AskTo::Agent(AgentAddressee::Holder(id(n))),
            again: false,
        }),
    ));
    assert_eq!(
        warned(&d, WarningCode::NoRecipient)
            .iter()
            .map(|x| &x.detail)
            .collect::<Vec<_>>(),
        vec![&WarningDetail::NoRecipient {
            node: n,
            question: true,
        }]
    );
    // Once the closer is silent, another session's note reaches nobody too; to the closer by
    // name, it is delivered, and warns `addressee_gone`.
    w.advance(Dur(w.s.params.presence_silence.0 + 60_000));
    assert!(w.s.sessions[&s].silent);
    let last = w.s.sessions[&s].last_seen;
    let x = w.hello(&c);
    let d = ok(w.call(&c, x, say_to(n, None)));
    assert_eq!(warned(&d, WarningCode::NoRecipient).len(), 1);
    let d = ok(w.call(&c, x, say_to(n, Some(AddresseeArg::Session(s)))));
    let Reply::Say { entry } = d.reply else {
        panic!("{:?}", d.reply)
    };
    assert_eq!(w.s.notes[&entry].recipients, BTreeSet::from([s]));
    assert_eq!(
        warned(&d, WarningCode::AddresseeGone),
        vec![Warning {
            code: WarningCode::AddresseeGone,
            id: None,
            detail: WarningDetail::AddresseeGone {
                session: s,
                since: last,
            },
        }]
    );
    // An agent question to the silent session warns the same.
    let d = ok(w.call(
        &c,
        x,
        Cmd::Talk(TalkCmd::Ask {
            at: id(n),
            text: text("why is /payments paused?"),
            to: AskTo::Agent(AgentAddressee::Session(s)),
            again: false,
        }),
    ));
    assert_eq!(warned(&d, WarningCode::AddresseeGone).len(), 1);
    // At an open node nobody holds, `holder:` still waits for the next holder, with no warning.
    let open = node(&mut w, &c, x, "/incidents/inc-312", &[]);
    let d = ok(w.call(&c, x, say_to(open, Some(AddresseeArg::Holder(id(open))))));
    let Reply::Say { entry } = d.reply else {
        panic!("{:?}", d.reply)
    };
    assert!(w.s.notes[&entry].pending_holder);
    assert!(d.warnings.is_empty());
}

// ------------------------------------------------------------------------------------------------
// C3: a person nobody serves (§8.4)

/// it6 l1: a question for a commit's author, whom no session served, was filed with no warning.
/// No stage of its chain is served: `no_human_in_reach`, naming the chain; the question is still
/// recorded. A session that serves the person, or the person's recent message, keeps it quiet.
#[test]
fn a_question_nobody_serves_warns_no_human_in_reach() {
    use crate::helpers::user::tests::{Ask, ask_cmd as ask_user, from_user_cmd, open_as};
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/shop/fix-flaky", &[]);
    let d = ok(w.call(
        &c,
        s,
        ask_user(
            n,
            &Ask {
                to: Some("mail:author@x"),
                ..Ask::default()
            },
        ),
    ));
    let q = question_of(&d);
    assert!(w.s.uquestions.contains_key(&q));
    let chain = w.s.uquestions[&q].to.clone();
    assert_eq!(chain.len(), 2);
    assert_eq!(
        warned(&d, WarningCode::NoHumanInReach),
        vec![Warning {
            code: WarningCode::NoHumanInReach,
            id: None,
            detail: WarningDetail::NobodyServes { question: q, chain },
        }]
    );
    // A session that serves `user` makes the chain's last stage served.
    let ide = open_as(&mut w, &class("ide"), Some("user"));
    let d = ok(w.call(
        &c,
        s,
        ask_user(
            n,
            &Ask {
                to: Some("mail:author@x"),
                digest: 2,
                ..Ask::default()
            },
        ),
    ));
    assert!(warned(&d, WarningCode::NoHumanInReach).is_empty());
    // Once that session has retired, the person's own message within user_gap.window counts.
    w.advance(Dur(w.s.params.presence_silence.0
        + w.s.params.session_retain.0
        + 60_000));
    assert!(!w.s.sessions.contains_key(&ide));
    let s = w.hello(&c);
    let d = ok(w.call(
        &c,
        s,
        ask_user(
            n,
            &Ask {
                to: Some("mail:author@x"),
                digest: 3,
                ..Ask::default()
            },
        ),
    ));
    assert_eq!(warned(&d, WarningCode::NoHumanInReach).len(), 1);
    let mail = open_as(&mut w, &class("mail"), Some("mail:author@x"));
    ok(w.call(
        &class("mail"),
        mail,
        from_user_cmd("it is mine", Some("mail:author@x"), None, None, None),
    ));
    w.advance(Dur(w.s.params.presence_silence.0
        + w.s.params.session_retain.0
        + 60_000));
    assert!(!w.s.sessions.contains_key(&mail));
    let s = w.hello(&c);
    let d = ok(w.call(
        &c,
        s,
        ask_user(
            n,
            &Ask {
                to: Some("mail:author@x"),
                digest: 4,
                ..Ask::default()
            },
        ),
    ));
    assert!(warned(&d, WarningCode::NoHumanInReach).is_empty());
}

// ------------------------------------------------------------------------------------------------
// C4: withdrawing a question (§9.3, E1)

/// it6 l1: a blocking question to the wrong person could not be withdrawn, and would have
/// escalated. A retract of it, open, withdraws it: its waits resolve, no recheck is queued, and its
/// rechecks never come; E1's step clause and (g) hold on every call and firing (`World`).
#[test]
fn a_retract_withdraws_an_open_question_to_a_person() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/shop/fix-flaky", &["chat:eng/T2001"]);
    let t = token_of(&ok(w.call(&c, s, take(n))));
    let q = question_of(&ok(w.call(&c, s, ask_cmd(n, None, true, false))));
    let mut wait = release(t, ReleaseReason::Blocked);
    if let Cmd::Hold(HoldCmd::Release { waits_for, .. }) = &mut wait {
        *waits_for = Some(WaitArg::Question(q));
    }
    ok(w.call(&c, s, wait));
    assert!(crate::spec::waiting(&w.s, n, w.clock));
    let d = ok(w.call(&c, s, retract(q, n)));
    let Reply::Retracted { entry, withdrawn } = d.reply else {
        panic!("{:?}", d.reply)
    };
    assert_eq!(withdrawn, Some(q));
    assert_eq!(w.s.pquestions[&q].state, PqState::Withdrawn { by: entry });
    assert!(evs(&w).contains(&Ev::PqWithdrawn { q, by: entry }));
    assert!(!crate::spec::waiting(&w.s, n, w.clock));
    assert!(
        !w.s.ix
            .deadline_at
            .contains_key(&(DeadlineKind::Recheck, Owner::Entry(q)))
    );
    // Past every recheck: nothing marks, expires, or escalates it.
    let uqs = w.s.uquestions.len();
    for _ in 0..=w.s.params.question_max_rechecks {
        w.advance(w.s.params.question_recheck);
    }
    assert_eq!(w.s.pquestions[&q].state, PqState::Withdrawn { by: entry });
    assert_eq!(w.s.uquestions.len(), uqs);
    assert!(w.s.nodes[&n].recheck_marks.is_empty());
    // A late answer is recorded and changes nothing; a second retract withdraws nothing.
    let x = w.hello(&c);
    ok(w.call(&c, x, heard_cmd(n, None, Some(q))));
    assert_eq!(w.s.pquestions[&q].state, PqState::Withdrawn { by: entry });
    let d = ok(w.call(&c, x, retract(q, n)));
    assert!(matches!(
        d.reply,
        Reply::Retracted {
            withdrawn: None,
            ..
        }
    ));
}

/// A retract of an answered question, or of the `said` that asked it, withdraws nothing.
#[test]
fn a_retract_of_an_answered_question_or_its_said_withdraws_nothing() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let d = ok(w.call(&c, s, ask_cmd(n, None, false, false)));
    let (q, said) = match d.reply {
        Reply::Asked { question, said, .. } => (question, said.unwrap()),
        ref other => panic!("{other:?}"),
    };
    let d = ok(w.call(&c, s, retract(said, n)));
    assert!(matches!(
        d.reply,
        Reply::Retracted {
            withdrawn: None,
            ..
        }
    ));
    assert_eq!(w.s.pquestions[&q].state, PqState::Open);
    ok(w.call(&c, s, heard_cmd(n, None, Some(q))));
    let d = ok(w.call(&c, s, retract(q, n)));
    assert!(matches!(
        d.reply,
        Reply::Retracted {
            withdrawn: None,
            ..
        }
    ));
    assert!(matches!(w.s.pquestions[&q].state, PqState::Answered { .. }));
}

/// E1 (g): a withdrawn question with a recheck still queued is a violation of the state check.
#[test]
fn e1_rejects_a_withdrawn_question_with_its_recheck_queued() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let q = question_of(&ok(w.call(&c, s, ask_cmd(n, None, false, false))));
    let mut st = w.s.clone();
    if let Some(pq) = st.pquestions.get_mut(&q) {
        pq.state = PqState::Withdrawn { by: Eid(1) };
    }
    let v = crate::invariants::check_invariants(&st, w.clock).unwrap_err();
    assert!(v.iter().any(|x| x.inv == Inv::E1), "{v:?}");
}

// ------------------------------------------------------------------------------------------------
// C5: a reopened node is offered (§16)

/// it6 l5: four reopened claims were offered to nobody. A reopen tags its node offer, so `next`
/// offers it; a node that carries the tag, or a container, gets none.
#[test]
fn a_reopen_offers_its_node() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/claims/lemma-04", &[]);
    ok(w.call(&c, s, close_cmd(n, Outcome::Done)));
    assert!(!w.s.nodes[&n].tags.iter().any(|t| t.as_str() == OFFER));
    let d = ok(w.call(&c, s, reopen(n)));
    assert!(matches!(d.reply, Reply::Reopened { offered: true, .. }));
    assert!(evs(&w).contains(&Ev::OfferSet { node: n, on: true }));
    let x = w.hello(&c);
    let d = ok(w.call(&c, x, crate::apply::tests::next()));
    assert!(
        matches!(d.reply, Reply::Next(crate::reply::NextReply::Hold { node, .. }) if node == n),
        "{:?}",
        d.reply
    );
    // A close strips the tag; a node tagged since, while closed, gets no second one.
    let m = node(&mut w, &c, s, "/claims/lemma-06", &[]);
    ok(w.call(&c, s, close_cmd(m, Outcome::Done)));
    crate::helpers::user::tests::update(&mut w, &c, s, m, |p| {
        p.tags_add = BTreeSet::from([Label::new(OFFER).unwrap()]);
    });
    assert!(w.s.nodes[&m].tags.iter().any(|t| t.as_str() == OFFER));
    let d = ok(w.call(&c, s, reopen(m)));
    assert!(matches!(d.reply, Reply::Reopened { offered: false, .. }));
    assert!(!evs(&w).iter().any(|e| matches!(e, Ev::OfferSet { .. })));
    // A person, a container, gets none.
    let p = crate::helpers::user::tests::person(&mut w, &c, s, "omar", &["mail:omar@x"]);
    ok(w.call(&c, s, close_cmd(p, Outcome::Done)));
    let d = ok(w.call(&c, s, reopen(p)));
    assert!(matches!(d.reply, Reply::Reopened { offered: false, .. }));
    assert!(!w.s.nodes[&p].tags.iter().any(|t| t.as_str() == OFFER));
}

// ------------------------------------------------------------------------------------------------
// C6: answers with nothing asked (§9.1, §9.2)

/// it6 l3: a chat worker answered a thread's questions with no request recorded, and its first
/// `said` silently made a node the thread's conversation node. A `reply` or `result` that answers
/// nothing where no request of its conversation is warns `answered_nothing`, and the call that gave
/// `at` its key says so (`keyed`).
#[test]
fn an_answer_with_nothing_asked_warns_and_a_new_key_is_named() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/platform/pg16-upgrade", &[]);
    let d = ok(w.call(
        &c,
        s,
        said_cmd(n, Some("chat:platform/T3301"), SaidKind::Result),
    ));
    let Reply::Said {
        node: at,
        ref answered,
        ref keyed,
        ..
    } = d.reply
    else {
        panic!("{:?}", d.reply)
    };
    assert_eq!(at, n);
    assert!(answered.is_empty());
    assert_eq!(keyed.as_ref(), Some(&ks("chat:platform/T3301")));
    assert_eq!(
        warned(&d, WarningCode::AnsweredNothing),
        vec![Warning {
            code: WarningCode::AnsweredNothing,
            id: Some(n),
            detail: WarningDetail::AnsweredNothing {
                node: n,
                conv: ks("chat:platform/T3301"),
            },
        }]
    );
    // Now the node carries the key: the next `said` gives it nothing; an ack warns nothing.
    let d = ok(w.call(&c, s, said_cmd(n, None, SaidKind::Ack)));
    assert!(matches!(d.reply, Reply::Said { keyed: None, .. }));
    assert!(warned(&d, WarningCode::AnsweredNothing).is_empty());
    // With a request recorded, its answer warns nothing, nor does a later reply.
    ok(w.call(
        &c,
        s,
        request_cmd(n, "chat:platform/T3301/M1", Some("chat:platform/T3301")),
    ));
    let d = ok(w.call(&c, s, said_cmd(n, None, SaidKind::Reply)));
    assert!(matches!(d.reply, Reply::Said { ref answered, .. } if answered.len() == 1));
    assert!(warned(&d, WarningCode::AnsweredNothing).is_empty());
    let d = ok(w.call(&c, s, said_cmd(n, None, SaidKind::Reply)));
    assert!(warned(&d, WarningCode::AnsweredNothing).is_empty());
    // `heard` and `ask` name a key they give.
    let m = node(&mut w, &c, s, "/platform/pg17", &[]);
    let d = ok(w.call(&c, s, heard_cmd(m, Some("chat:platform/T3302"), None)));
    assert!(
        matches!(d.reply, Reply::Heard { keyed: Some(ref k), .. } if *k == ks("chat:platform/T3302"))
    );
    let o = node(&mut w, &c, s, "/platform/pg18", &[]);
    let d = ok(w.call(&c, s, ask_cmd(o, Some("chat:platform/T3303"), false, false)));
    assert!(
        matches!(d.reply, Reply::Asked { keyed: Some(ref k), .. } if *k == ks("chat:platform/T3303"))
    );
}

/// Settled's step clause, by name: every release in these tests passed `check_step`, which rejects
/// a release step whose decision lists other requests than its `settles` (step_tests.rs mutates
/// one); here, a decision of the release's step names its requests.
#[test]
fn settled_holds_over_a_release_with_settles() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/shop/fix", &["chat:eng/T1"]);
    let t = token_of(&ok(w.call(&c, s, take(n))));
    let r = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(n, "chat:eng/T1/M1", Some("chat:eng/T1")),
    )));
    ok(w.call(&c, s, said_cmd(n, Some("chat:eng/T1"), SaidKind::Result)));
    ok(w.call(&c, s, release_settling(t, merged_wait(), &[r])));
    let v = crate::invariants::check_invariants(&w.s, w.clock);
    assert!(
        v.as_ref()
            .err()
            .is_none_or(|v| !v.iter().any(|x| x.inv == Inv::Settled)),
        "{v:?}"
    );
    assert!(w.s.requests[&r].settled.is_some());
}
