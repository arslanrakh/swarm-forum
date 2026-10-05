//! The life of a people question past its first recheck (design D.8.2, D.8.15, §9.3, §25):
//! `check_fire` holds the Recheck firing to D.8.2's row in each of its branches (D.9.1: "the events
//! are exactly the table's"), and `heard {answers}` answers an expired or an escalated question.
//!
//! The last recheck's branches are built by hand here, as the row states them, so that each can be
//! edited off the row; the firing arm's own tests, with WP-D5a's `add_digest` and `create_uq`, are
//! `the_last_recheck_of_*`.

use alloc::vec;
use alloc::vec::Vec;

use super::super::super::keys::tests::{evs, node, one};
use super::{ask_cmd, asked, created_token, heard_cmd, waiting_release};
use crate::apply::tests::{entry, ok};
use crate::event::{EntryBody, EntryText, Ev, Event, QuestionBody};
use crate::helpers::Exec;
use crate::invariants::{Inv, Violation, ViolationDetail, check_fire};
use crate::reply::Reply;
use crate::state::{
    Author, DeadlineKey, DeadlineKind, DigestItem, Owner, PqState, State, UqSource, UqState, WaitOn,
};
use crate::testkit::World;
use crate::types::*;

// ------------------------------------------------------------------------------------------------
// Helpers

/// Settles every key before people question `q`'s recheck, which is then the queue's first.
fn up_to_recheck(w: &mut World, q: Eid) -> DeadlineKey {
    let at = w.s.pquestions[&q].recheck_at;
    w.at(Time(at.0 - 1));
    w.settle();
    let key = w.s.ix.deadlines.first().cloned().unwrap();
    assert_eq!(
        (key.kind, &key.owner, key.at),
        (DeadlineKind::Recheck, &Owner::Entry(q), at)
    );
    key
}

/// The real firing of the queue's first key: the state before, the key, its events, and after.
fn fire_first(w: &World) -> (State, DeadlineKey, Vec<Event>, State) {
    let before = w.s.clone();
    let key = before.ix.deadlines.first().cloned().unwrap();
    let mut after = before.clone();
    let (events, _) = crate::settle::settle(&mut after, key.at, 1);
    (before, key, events, after)
}

/// A firing of the queue's first key built by hand: `Fired`, then what `f` emits.
fn fire_by_hand(w: &World, f: impl FnOnce(&mut Exec<'_>)) -> (State, DeadlineKey, Vec<Event>) {
    let before = w.s.clone();
    let key = before.ix.deadlines.first().cloned().unwrap();
    let mut after = before.clone();
    let mut ex = Exec::new(&mut after, key.at, None);
    ex.emit(Ev::Fired {
        kind: key.kind,
        owner: key.owner.clone(),
    });
    f(&mut ex);
    let (events, _) = ex.finish();
    (before, key, events)
}

/// The events of a firing, `f` applied to their list, each at the key's time.
fn edited(key: &DeadlineKey, events: &[Event], f: impl FnOnce(&mut Vec<Ev>)) -> Vec<Event> {
    let mut evs: Vec<Ev> = events.iter().map(|e| e.ev.clone()).collect();
    f(&mut evs);
    evs.into_iter().map(|ev| Event { at: key.at, ev }).collect()
}

/// `check_fire` of `events` against `before` and the fold of `events` onto it.
fn fire_check(before: &State, key: &DeadlineKey, events: &[Event]) -> Result<(), Vec<Violation>> {
    let mut after = before.clone();
    for e in events {
        crate::evolve::evolve_mut(&mut after, e);
    }
    check_fire(before, key, events, &after)
}

/// `check_fire` refuses `events` for not being the row's (T4, on the key).
fn off_the_row(before: &State, key: &DeadlineKey, events: &[Event]) -> bool {
    fire_check(before, key, events).is_err_and(|v| {
        v.contains(&Violation {
            inv: Inv::T4,
            detail: ViolationDetail::Deadline(key.clone()),
        })
    })
}

fn mark(node: NodeId, mark: Mark, q: Eid, on: bool) -> Ev {
    Ev::MarkSet {
        node,
        mark,
        question: q,
        on,
    }
}

/// The digest entries of `q`'s expiry, as `add_digest` writes them (D.8.13), for the audience on
/// the state before the firing.
fn digest(ex: &mut Exec<'_>, before: &State, q: Eid) {
    let item = DigestItem::Assumption { question: q };
    for to in crate::spec::audience(before, &item) {
        ex.append(
            USER_DIGEST,
            Author::Server,
            EntryBody::System(crate::event::SystemNote::Digest { item, to }),
            EntryText::default(),
        );
    }
}

/// A people question on `/support` (key `chat:a/1`), and the nodes `/support/a` and `/support/b`.
fn asked_on_support(blocking: bool) -> (World, ClassName, SessionId, NodeId, NodeId, NodeId, Eid) {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let a = node(&mut w, &c, s, "/support/a", &[]);
    let b = node(&mut w, &c, s, "/support/b", &[]);
    let d = ok(w.call(&c, s, ask_cmd(n, None, blocking, false)));
    let (q, ..) = asked(&d.reply);
    (w, c, s, n, a, b, q)
}

/// `x` waits on `q` until its next recheck: a take and a release with `waits_for: {question}`,
/// which moves the recheck to `now + question.recheck`.
fn wait_on(w: &mut World, c: &ClassName, s: SessionId, x: NodeId, q: Eid) {
    let t = created_token(w, c, s, x);
    ok(w.call(c, s, waiting_release(t, q)));
}

// ------------------------------------------------------------------------------------------------
// check_fire: a recheck that is not the last

#[test]
fn check_fire_holds_a_recheck_that_is_not_the_last_to_its_row() {
    // No waiter: the open conversation node is marked (A-81).
    let (mut w, c, s, n, a, _, q) = asked_on_support(false);
    let key = up_to_recheck(&mut w, q);
    let (before, key2, events, after) = fire_first(&w);
    assert_eq!(key, key2);
    assert!(check_fire(&before, &key, &events, &after).is_ok());
    assert!(
        events
            .iter()
            .any(|e| e.ev == mark(n, Mark::Recheck, q, true))
    );
    // The mark that §9.3 and A-81 require, dropped.
    let dropped = edited(&key, &events, |v| {
        v.retain(|e| !matches!(e, Ev::MarkSet { .. }))
    });
    assert!(off_the_row(&before, &key, &dropped));
    // Another node marked instead.
    let other = edited(&key, &events, |v| {
        for e in v.iter_mut() {
            if let Ev::MarkSet { node, .. } = e {
                *node = a;
            }
        }
    });
    assert!(off_the_row(&before, &key, &other));
    // The next recheck at another time, and the count not raised.
    let late = edited(&key, &events, |v| {
        for e in v.iter_mut() {
            if let Ev::PqRechecked { next, .. } = e {
                *next = next.plus(Dur(1));
            }
        }
    });
    assert!(off_the_row(&before, &key, &late));
    let same_count = edited(&key, &events, |v| {
        for e in v.iter_mut() {
            if let Ev::PqRechecked { rechecks, .. } = e {
                *rechecks = 0;
            }
        }
    });
    assert!(off_the_row(&before, &key, &same_count));
    // An assumption instead of a recheck.
    let assumed = edited(&key, &events, |v| {
        for e in v.iter_mut() {
            if let Ev::MarkSet { mark: m, .. } = e {
                *m = Mark::Assume;
            }
        }
    });
    assert!(off_the_row(&before, &key, &assumed));
    let _ = (c, s);
}

#[test]
fn check_fire_marks_only_the_waiters_of_a_recheck() {
    // A waiter: the waits this recheck resolves are marked, and the conversation node is not.
    let (mut w, c, s, n, a, b, q) = asked_on_support(false);
    wait_on(&mut w, &c, s, a, q);
    wait_on(&mut w, &c, s, b, q);
    let key = up_to_recheck(&mut w, q);
    let (before, _, events, after) = fire_first(&w);
    assert!(check_fire(&before, &key, &events, &after).is_ok());
    let marked: Vec<NodeId> = events
        .iter()
        .filter_map(|e| match e.ev {
            Ev::MarkSet { node, on: true, .. } => Some(node),
            _ => None,
        })
        .collect();
    assert_eq!(marked, vec![a, b]);
    // The conversation node marked too.
    let more = edited(&key, &events, |v| v.push(mark(n, Mark::Recheck, q, true)));
    assert!(off_the_row(&before, &key, &more));
    // One waiter left out, or the two out of order.
    let less = edited(&key, &events, |v| {
        v.retain(|e| *e != mark(b, Mark::Recheck, q, true));
    });
    assert!(off_the_row(&before, &key, &less));
    let swapped = edited(&key, &events, |v| {
        let i = v.len() - 2;
        v.swap(i, i + 1);
    });
    assert!(off_the_row(&before, &key, &swapped));
}

#[test]
fn check_fire_marks_nothing_for_a_recheck_on_a_closed_conversation_node() {
    let (mut w, c, s, n, _, _, q) = asked_on_support(false);
    ok(w.call(&c, s, crate::apply::tests::close(n, Outcome::Done)));
    let key = up_to_recheck(&mut w, q);
    let (before, _, events, after) = fire_first(&w);
    assert!(check_fire(&before, &key, &events, &after).is_ok());
    assert!(!events.iter().any(|e| matches!(e.ev, Ev::MarkSet { .. })));
    let marked = edited(&key, &events, |v| v.push(mark(n, Mark::Recheck, q, true)));
    assert!(off_the_row(&before, &key, &marked));
}

// ------------------------------------------------------------------------------------------------
// check_fire: the last recheck, by hand

/// The last recheck of `q` is next: the first fires for real, then `waiters` wait on `q` again.
fn up_to_last_recheck(
    w: &mut World,
    c: &ClassName,
    s: SessionId,
    q: Eid,
    waiters: &[NodeId],
) -> DeadlineKey {
    assert_eq!(w.s.params.question_max_rechecks, 2);
    let at = w.s.pquestions[&q].recheck_at;
    w.at(at);
    w.settle();
    assert_eq!(w.s.pquestions[&q].rechecks, 1);
    // A fresh session takes and releases the waiters (the asker may have retired by now).
    let s2 = w.hello(c);
    for &x in waiters {
        wait_on(w, c, s2, x, q);
    }
    let _ = s;
    up_to_recheck(w, q)
}

#[test]
fn check_fire_holds_an_expiry_to_its_row() {
    // With waiters: `PqExpired`, the digest entries, and the assume marks on the waiters.
    let (mut w, c, s, n, a, b, q) = asked_on_support(false);
    let key = up_to_last_recheck(&mut w, &c, s, q, &[a, b]);
    let before = w.s.clone();
    let (_, key2, events) = fire_by_hand(&w, |ex| {
        ex.emit(Ev::PqExpired { q });
        digest(ex, &before, q);
        ex.emit(mark(a, Mark::Assume, q, true));
        ex.emit(mark(b, Mark::Assume, q, true));
    });
    assert_eq!(key, key2);
    assert_eq!(fire_check(&before, &key, &events), Ok(()));
    // Recheck marks, as a recheck that is not the last sets.
    let rechecked = edited(&key, &events, |v| {
        for e in v.iter_mut() {
            if let Ev::MarkSet { mark: m, .. } = e {
                *m = Mark::Recheck;
            }
        }
    });
    assert!(off_the_row(&before, &key, &rechecked));
    // The conversation node marked instead of the waiters.
    let conv = edited(&key, &events, |v| {
        v.retain(|e| !matches!(e, Ev::MarkSet { .. }));
        v.push(mark(n, Mark::Assume, q, true));
    });
    assert!(off_the_row(&before, &key, &conv));
    // No digest entry.
    let silent = edited(&key, &events, |v| {
        v.retain(|e| !matches!(e, Ev::Appended { .. }));
    });
    assert!(off_the_row(&before, &key, &silent));
    // A further recheck instead of the expiry.
    let again = edited(&key, &events, |v| {
        v[1] = Ev::PqRechecked {
            q,
            rechecks: 2,
            next: key.at.plus(before.params.question_recheck),
        };
    });
    assert!(off_the_row(&before, &key, &again));
}

#[test]
fn check_fire_holds_an_expiry_with_nothing_to_mark_to_assume_now() {
    // No waiter and a closed conversation node: the server records the assumption at once
    // (A-104), on the conversation node.
    let (mut w, c, s, n, _, _, q) = asked_on_support(false);
    let key = up_to_last_recheck(&mut w, &c, s, q, &[]);
    let s2 = w.hello(&c);
    ok(w.call(&c, s2, crate::apply::tests::close(n, Outcome::Done)));
    assert_eq!(w.s.ix.deadlines.first(), Some(&key));
    let before = w.s.clone();
    let (_, _, events) = fire_by_hand(&w, |ex| {
        ex.emit(Ev::PqExpired { q });
        digest(ex, &before, q);
        let d = ex.append(
            n,
            Author::Server,
            EntryBody::Decision {
                settles: alloc::collections::BTreeSet::new(),
                supersedes: None,
                assumes: [q].into_iter().collect(),
                evidence: None,
                basis: Vec::new(),
            },
            EntryText::default(),
        );
        ex.emit(Ev::PqAssumed { q, decision: d });
    });
    assert_eq!(fire_check(&before, &key, &events), Ok(()));
    // The assumption left unrecorded.
    let unrecorded = edited(&key, &events, |v| {
        v.truncate(v.len() - 2);
    });
    assert!(off_the_row(&before, &key, &unrecorded));
    // A mark on the closed node instead.
    let marked = edited(&key, &events, |v| {
        v.truncate(v.len() - 2);
        v.push(mark(n, Mark::Assume, q, true));
    });
    assert!(off_the_row(&before, &key, &marked));
}

#[test]
fn check_fire_holds_an_escalation_to_its_row() {
    // A blocking question: the user question, `PqEscalated`, and the waiters moved to it; no mark
    // (A-93).
    let (mut w, c, s, n, a, _, q) = asked_on_support(true);
    let key = up_to_last_recheck(&mut w, &c, s, q, &[a]);
    let before = w.s.clone();
    let (_, _, events) = fire_by_hand(&w, |ex| {
        let uq = ex.append(
            USER_DECISIONS,
            Author::Server,
            EntryBody::Question(QuestionBody::User {
                source: UqSource::Escalation { question: q },
                about: Some(n),
                to: crate::spec::chain(&before, Some(n), None, false),
                due: key.at.plus(before.params.user_question_due),
                has_default: false,
            }),
            EntryText::default(),
        );
        ex.emit(Ev::PqEscalated { q, uq });
        ex.emit(Ev::WaitSet {
            node: a,
            wait: Some(WaitOn::UserQuestion(uq)),
        });
    });
    assert_eq!(fire_check(&before, &key, &events), Ok(()));
    // A recheck mark on the waiter, which A-93 rules out.
    let marked = edited(&key, &events, |v| v.push(mark(a, Mark::Recheck, q, true)));
    assert!(off_the_row(&before, &key, &marked));
    // The waiter left waiting on the people question.
    let stuck = edited(&key, &events, |v| {
        v.retain(|e| !matches!(e, Ev::WaitSet { .. }));
    });
    assert!(off_the_row(&before, &key, &stuck));
    // A user question about another node, or with a default.
    let elsewhere = edited(&key, &events, |v| {
        if let Ev::Appended {
            body: EntryBody::Question(QuestionBody::User { about, .. }),
            ..
        } = &mut v[1]
        {
            *about = Some(a);
        }
    });
    assert!(off_the_row(&before, &key, &elsewhere));
    let defaulted = edited(&key, &events, |v| {
        if let Ev::Appended {
            body: EntryBody::Question(QuestionBody::User { has_default, .. }),
            ..
        } = &mut v[1]
        {
            *has_default = true;
        }
    });
    assert!(off_the_row(&before, &key, &defaulted));
    // An expiry instead of the escalation.
    let expired = edited(&key, &events, |v| {
        v.truncate(1);
        v.push(Ev::PqExpired { q });
    });
    assert!(off_the_row(&before, &key, &expired));
}

// ------------------------------------------------------------------------------------------------
// heard {answers} on an expired or escalated question (§9.3, §25)

#[test]
fn heard_answers_an_expired_question_and_clears_its_assume_marks() {
    let (mut w, c, s, n, a, _, q) = asked_on_support(false);
    // q expires by its events alone, with the assume mark on the waiter `a`.
    w.emit(Ev::PqExpired { q });
    w.emit(mark(a, Mark::Assume, q, true));
    let d = ok(w.call(&c, s, heard_cmd(n, None, Some(q))));
    let Reply::Heard {
        entry: h, answered, ..
    } = d.reply
    else {
        panic!()
    };
    assert_eq!(answered, Some(q));
    assert_eq!(w.s.pquestions[&q].state, PqState::Answered { by: h });
    let e = evs(&w);
    assert!(e.contains(&Ev::PqAnswered { q, by: h }));
    assert!(e.contains(&mark(a, Mark::Assume, q, false)));
    assert!(!w.s.nodes[&a].assume_marks.contains(&q));
}

/// q escalated by its events alone: a user question about `n` on `@user/decisions` and
/// `PqEscalated`; returns the user question.
fn escalate(w: &mut World, n: NodeId, q: Eid) -> Eid {
    let due = w.clock.plus(w.s.params.user_question_due);
    let uq = entry(
        w,
        USER_DECISIONS,
        Author::Server,
        EntryBody::Question(QuestionBody::User {
            source: UqSource::Escalation { question: q },
            about: Some(n),
            to: vec![USER_PRINCIPAL],
            due,
            has_default: false,
        }),
    );
    w.emit(Ev::PqEscalated { q, uq });
    uq
}

#[test]
fn heard_answers_an_escalated_question_and_its_pending_user_question() {
    // A-60.
    let (mut w, c, s, n, _, _, q) = asked_on_support(true);
    let uq = escalate(&mut w, n, q);
    assert_eq!(w.s.uquestions[&uq].state, UqState::Pending);
    let d = ok(w.call(&c, s, heard_cmd(n, None, Some(q))));
    let Reply::Heard {
        entry: h, answered, ..
    } = d.reply
    else {
        panic!()
    };
    assert_eq!(answered, Some(q));
    assert_eq!(w.s.pquestions[&q].state, PqState::Answered { by: h });
    let e = evs(&w);
    assert!(e.contains(&Ev::PqAnswered { q, by: h }));
    assert!(e.contains(&Ev::UqAnswered { q: uq, by: h }));
    assert_eq!(w.s.uquestions[&uq].state, UqState::Answered { by: h });
}

#[test]
fn heard_answers_an_escalated_question_whose_user_question_lapsed() {
    // CS-2: the late answer clears the lapse's assume marks; the user question stays lapsed.
    let (mut w, c, s, n, a, _, q) = asked_on_support(true);
    let uq = escalate(&mut w, n, q);
    w.emit(Ev::UqLapsed { q: uq });
    w.emit(mark(a, Mark::Assume, uq, true));
    let d = ok(w.call(&c, s, heard_cmd(n, None, Some(q))));
    let Reply::Heard {
        entry: h, answered, ..
    } = d.reply
    else {
        panic!()
    };
    assert_eq!(answered, Some(q));
    assert_eq!(w.s.pquestions[&q].state, PqState::Answered { by: h });
    let e = evs(&w);
    assert!(e.contains(&Ev::PqAnswered { q, by: h }));
    assert!(e.contains(&mark(a, Mark::Assume, uq, false)));
    assert!(!e.iter().any(|x| matches!(x, Ev::UqAnswered { .. })));
    assert!(!w.s.nodes[&a].assume_marks.contains(&uq));
    assert_eq!(w.s.uquestions[&uq].state, UqState::Lapsed);
}
