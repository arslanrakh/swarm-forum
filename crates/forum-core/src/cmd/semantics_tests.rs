//! 1.10's semantics (design D.8.10, D.8.16, D.8.17): a second, opposite verdict on a resolved
//! objection opens a new one, and withdraws the correction an upheld verdict made (§11.3); a
//! resolve that keeps a claim with no decision standing warns `no_basis` (§11.3); a checkpoint ends
//! its node's wait, and the `waiting` warning tells the session that set it (§10.1, §10.2); an
//! intent under a pause warns `paused` (§10.2, §19). Every call runs through `World`, with every
//! check of a step, of a firing, and of the state, and G1 on its next calls. (The `said` of a
//! session that holds neither its conversation node nor work inside it, and the note without `to`
//! at a node nobody holds, are tested with their commands.)

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

use super::keys::tests::{close_cmd, id, node, one};
use crate::apply::tests::{checkpoint, create_with, created, label, ok, release, take, text};
use crate::cmd::*;
use crate::error::{Error, Hint};
use crate::event::{EntryBody, Ev};
use crate::reply::{Corrected, Disputed, Reply, Verdict, Warning, WarningCode, WarningDetail};
use crate::state::{Cand, Life, ObjectionState, WaitOn};
use crate::types::*;

// ------------------------------------------------------------------------------------------------
// Commands and readings

fn object(n: NodeId) -> Cmd {
    Cmd::Talk(TalkCmd::Object {
        on: id(n),
        note: text("a counterexample at n = 11"),
    })
}

fn resolve(o: Eid, upheld: bool, note: &str) -> Cmd {
    Cmd::Talk(TalkCmd::Resolve {
        objection: o,
        note: text(note),
        upheld,
        defer: false,
    })
}

fn decide(at: NodeId, basis: Vec<BasisArg>) -> Cmd {
    Cmd::Talk(TalkCmd::Decide {
        at: id(at),
        text: text("decided"),
        settles: BTreeSet::new(),
        supersedes: None,
        basis,
        evidence: None,
    })
}

fn on(n: NodeId) -> BasisArg {
    BasisArg::Node {
        at: id(n),
        version: None,
    }
}

fn objected(d: &crate::reply::Done) -> Eid {
    match d.reply {
        Reply::Objected { objection } => objection,
        ref other => panic!("not objected: {other:?}"),
    }
}

fn decided(d: &crate::reply::Done) -> Eid {
    match d.reply {
        Reply::Decided { decision, .. } => decision,
        ref other => panic!("not decided: {other:?}"),
    }
}

fn token_of(d: &crate::reply::Done) -> Token {
    match d.reply {
        Reply::Taken { token, .. } => token,
        ref other => panic!("not taken: {other:?}"),
    }
}

/// A claim at `p`, made without a hold.
fn claim(w: &mut crate::testkit::World, c: &ClassName, s: SessionId, p: &str) -> NodeId {
    let d = ok(w.call(
        c,
        s,
        create_with(p, |a| {
            a.take = Some(false);
            a.kind = Some(label(CLAIM));
        }),
    ));
    created(&d).0
}

fn closure(w: &crate::testkit::World, n: NodeId) -> crate::state::Closure {
    match &w.s.nodes[&n].life {
        Life::Closed(c) => c.clone(),
        Life::Open => panic!("{n:?} is open"),
    }
}

fn refuted() -> Outcome {
    Outcome::Other(label(REFUTED))
}

fn warned(d: &crate::reply::Done, code: WarningCode) -> Vec<Warning> {
    d.warnings
        .iter()
        .filter(|x| x.code == code)
        .cloned()
        .collect()
}

// ------------------------------------------------------------------------------------------------
// A second verdict (§11.3)

/// l5 of the lab's fifth round: B resolved lemma-06's objection as not standing, and C's upheld
/// verdict a minute later was dropped with a bare `{already: true}`. The same verdict again records
/// nothing and names the first; the opposite one is a new open objection with its text, which
/// flags the closed claim, so that it is offered as a review again; a third verdict decides it.
#[test]
fn an_opposite_verdict_opens_a_new_objection_that_is_reviewed_again() {
    let (mut w, c, s) = one();
    let b = w.hello(&c);
    let cc = w.hello(&c);
    let lemma = claim(&mut w, &c, s, "/claims/lemma-06");
    ok(w.call(&c, s, close_cmd(lemma, Outcome::Done)));
    let o = objected(&ok(w.call(&c, s, object(lemma))));
    let version = w.s.nodes[&lemma].version;
    // B: the objection does not stand.
    let d = ok(w.call(&c, b, resolve(o, false, "the recount gives 1984/1013")));
    let Reply::ObjectionResolved { entry: Some(e), .. } = d.reply else {
        panic!("{:?}", d.reply)
    };
    assert_eq!(w.s.objections[&o].state, ObjectionState::Resolved { by: e });
    assert!(!w.s.ix.node[&lemma].flags.contains(Flag::Objection));
    // The same verdict again: nothing recorded, and the verdict named.
    let d = ok(w.call(&c, cc, resolve(o, false, "agreed")));
    assert_eq!(
        d.reply,
        Reply::ObjectionResolved {
            entry: None,
            corrected: None,
            already: Some(Verdict {
                verdict: e,
                upheld: false,
            }),
            disputed: None,
            withdrawn: None,
            deferred: None,
        }
    );
    assert!(
        w.last_events
            .iter()
            .all(|x| matches!(x.ev, Ev::Touched { .. }))
    );
    // C: the opposite verdict, recorded as a new open objection by C, with C's text.
    let d = ok(w.call(&c, cc, resolve(o, true, "the recount gives 1987/1204")));
    let Reply::ObjectionResolved {
        entry: Some(o2),
        corrected: None,
        already: None,
        disputed: Some(x),
        withdrawn: None,
        deferred: None,
    } = d.reply
    else {
        panic!("{:?}", d.reply)
    };
    assert_eq!(
        x,
        Disputed {
            objection: o2,
            node: lemma,
            disputes: o,
            earlier: Verdict {
                verdict: e,
                upheld: false,
            },
        }
    );
    assert!(w.last_events.iter().any(|ev| matches!(
        &ev.ev,
        Ev::Appended {
            eid,
            body: EntryBody::Object,
            by: crate::state::Author::Session(by),
            ..
        } if *eid == o2 && *by == cc
    )));
    assert_eq!(w.s.objections[&o2].state, ObjectionState::Open);
    assert_eq!(w.s.objections[&o].state, ObjectionState::Resolved { by: e });
    // The claim stays closed done, with no counter moved, and is offered as the new review.
    assert_eq!(closure(&w, lemma).outcome, Outcome::Done);
    assert_eq!(w.s.nodes[&lemma].version, version);
    assert!(w.s.ix.node[&lemma].flags.contains(Flag::Objection));
    assert!(w.s.ix.next.of.contains_key(&Cand::Status(lemma)));
    assert_eq!(crate::query::review_objection(&w.s, lemma), Some(o2));
    // A third verdict decides it as any resolve does.
    let d = ok(w.call(&c, s, resolve(o2, true, "the third recount agrees with C")));
    assert!(matches!(
        d.reply,
        Reply::ObjectionResolved {
            corrected: Some(Corrected {
                was: Outcome::Done,
                ..
            }),
            disputed: None,
            ..
        }
    ));
    assert_eq!(closure(&w, lemma).outcome, refuted());
    assert_eq!(closure(&w, lemma).corrected_from, Some(Outcome::Done));
}

/// The opposite order: the first verdict upheld the objection and corrected the claim to refuted;
/// the dissent withdraws that correction, so that the claim reads as it was closed, with the
/// dissent as an open objection, until a review decides. References that recorded the corrected
/// outcome go stale, as a correction's do, and no counter moves.
#[test]
fn a_dissent_from_an_upheld_verdict_withdraws_its_correction() {
    let (mut w, c, s) = one();
    let b = w.hello(&c);
    let lemma = claim(&mut w, &c, s, "/claims/lemma-04");
    let thm = node(&mut w, &c, s, "/theorems/thm-1", &[]);
    let note = node(&mut w, &c, s, "/notes/lemma-04-refuted", &[]);
    ok(w.call(&c, s, close_cmd(lemma, Outcome::Done)));
    // thm-1 rests on the lemma as done.
    ok(w.call(&c, s, decide(thm, vec![on(lemma)])));
    let o = objected(&ok(w.call(&c, s, object(lemma))));
    let d = ok(w.call(&c, s, resolve(o, true, "the counterexample checks")));
    let Reply::ObjectionResolved { entry: Some(e), .. } = d.reply else {
        panic!("{:?}", d.reply)
    };
    assert_eq!(closure(&w, lemma).outcome, refuted());
    assert_eq!(closure(&w, lemma).upheld, Some(e));
    assert_eq!(closure(&w, lemma).corrected_from, Some(Outcome::Done));
    assert!(w.s.ix.node[&thm].flags.contains(Flag::BasisChanged));
    // A note rests on the lemma as refuted.
    ok(w.call(&c, s, decide(note, vec![on(lemma)])));
    assert!(!w.s.ix.node[&note].flags.contains(Flag::BasisChanged));
    let version = w.s.nodes[&lemma].version;
    // B dissents: the objection does not stand.
    let d = ok(w.call(
        &c,
        b,
        resolve(o, false, "the counterexample used a buggy checker"),
    ));
    let Reply::ObjectionResolved {
        entry: Some(o2),
        disputed: Some(x),
        withdrawn: Some(back),
        ..
    } = d.reply
    else {
        panic!("{:?}", d.reply)
    };
    assert_eq!(
        x.earlier,
        Verdict {
            verdict: e,
            upheld: true,
        }
    );
    assert_eq!(
        back,
        Corrected {
            node: lemma,
            was: refuted(),
            outcome: Outcome::Done,
        }
    );
    assert!(w.last_events.iter().any(|ev| matches!(
        &ev.ev,
        Ev::CorrectionWithdrawn { node, outcome: Outcome::Done } if *node == lemma
    )));
    let cl = closure(&w, lemma);
    assert_eq!(
        (cl.outcome, cl.upheld, cl.corrected_from),
        (Outcome::Done, None, None)
    );
    assert_eq!(w.s.nodes[&lemma].version, version);
    assert_eq!(w.s.objections[&o].state, ObjectionState::Upheld { by: e });
    assert_eq!(w.s.objections[&o2].state, ObjectionState::Open);
    // The note that recorded `refuted` now has a stale basis; the claim is a review again, and a
    // take of it is still `closed`.
    assert!(w.s.ix.node[&note].flags.contains(Flag::BasisChanged));
    assert!(w.s.ix.next.of.contains_key(&Cand::Status(lemma)));
    assert_eq!(
        w.call(&c, b, take(lemma)).err(),
        Some(Error::of(Hint::Closed {
            node: lemma,
            merged_into: None,
        }))
    );
    // A third verdict that keeps it: done, unmarked, and both objections resolved.
    ok(w.call(&c, s, resolve(o2, false, "the checker was buggy")));
    assert_eq!(closure(&w, lemma).outcome, Outcome::Done);
    assert!(!w.s.ix.node[&lemma].flags.contains(Flag::Objection));
}

/// An upheld verdict on an open node records the verdict only, so a dissent from it withdraws
/// nothing; nor does a dissent from a verdict whose correction a later one replaced as the mark.
#[test]
fn a_dissent_withdraws_only_the_correction_its_verdict_made() {
    let (mut w, c, s) = one();
    let b = w.hello(&c);
    let lemma = claim(&mut w, &c, s, "/claims/lemma-09");
    let o = objected(&ok(w.call(&c, s, object(lemma))));
    ok(w.call(&c, s, resolve(o, true, "it stands")));
    ok(w.call(&c, s, close_cmd(lemma, refuted())));
    let d = ok(w.call(&c, b, resolve(o, false, "it does not")));
    assert!(matches!(
        d.reply,
        Reply::ObjectionResolved {
            disputed: Some(_),
            withdrawn: None,
            ..
        }
    ));
    assert_eq!(closure(&w, lemma).outcome, refuted());
    assert!(
        !w.last_events
            .iter()
            .any(|ev| matches!(ev.ev, Ev::CorrectionWithdrawn { .. }))
    );
    // Two upheld verdicts: the mark is the later one's, and a dissent from the earlier withdraws
    // nothing.
    let other = claim(&mut w, &c, s, "/claims/lemma-10");
    ok(w.call(&c, s, close_cmd(other, Outcome::Done)));
    let o1 = objected(&ok(w.call(&c, s, object(other))));
    let o2 = objected(&ok(w.call(&c, s, object(other))));
    ok(w.call(&c, s, resolve(o1, true, "it stands")));
    ok(w.call(&c, s, resolve(o2, true, "so does this one")));
    ok(w.call(&c, b, resolve(o1, false, "the first does not")));
    assert_eq!(closure(&w, other).outcome, refuted());
    assert!(closure(&w, other).upheld.is_some());
}

// ------------------------------------------------------------------------------------------------
// A claim with no basis (§11.3)

/// l5 of the lab's fifth round: each deciding decision had been retracted by the objector, and the
/// reviewer's `upheld: false` left three lemmas reading as done with nothing that decided them.
#[test]
fn a_resolve_that_keeps_a_claim_with_no_decision_standing_warns_no_basis() {
    let (mut w, c, s) = one();
    let lemma = claim(&mut w, &c, s, "/claims/lemma-11");
    let d1 = decided(&ok(w.call(&c, s, decide(lemma, Vec::new()))));
    ok(w.call(&c, s, close_cmd(lemma, Outcome::Done)));
    ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Retract {
            target: RetractArg::Entry {
                eid: d1,
                node: lemma,
            },
            text: text("the exhaustive check double-counted"),
            reason: text("duplicate rows"),
        }),
    ));
    assert_eq!(crate::query::no_basis(&w.s, lemma), Some(d1));
    let o = objected(&ok(w.call(&c, s, object(lemma))));
    let d = ok(w.call(
        &c,
        s,
        resolve(o, false, "this rejects the objection's evidence only"),
    ));
    assert_eq!(
        warned(&d, WarningCode::NoBasis)
            .into_iter()
            .map(|x| x.detail)
            .collect::<Vec<_>>(),
        vec![WarningDetail::NoBasis {
            node: lemma,
            decision: d1,
        }]
    );
    // Upheld, it is corrected instead: no warning.
    let o = objected(&ok(w.call(&c, s, object(lemma))));
    let d = ok(w.call(&c, s, resolve(o, true, "it stands")));
    assert!(warned(&d, WarningCode::NoBasis).is_empty());
    assert_eq!(crate::query::no_basis(&w.s, lemma), None);
    // A claim with an active decision, or a node that is not a claim, warns nothing.
    let held = claim(&mut w, &c, s, "/claims/lemma-12");
    let gone = decided(&ok(w.call(&c, s, decide(held, Vec::new()))));
    ok(w.call(&c, s, decide(held, Vec::new())));
    ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Retract {
            target: RetractArg::Entry {
                eid: gone,
                node: held,
            },
            text: text("withdrawn"),
            reason: text("superseded by the second"),
        }),
    ));
    ok(w.call(&c, s, close_cmd(held, Outcome::Done)));
    let o = objected(&ok(w.call(&c, s, object(held))));
    let d = ok(w.call(&c, s, resolve(o, false, "fine")));
    assert!(warned(&d, WarningCode::NoBasis).is_empty());
    let task = node(&mut w, &c, s, "/tasks/fix", &[]);
    let t1 = decided(&ok(w.call(&c, s, decide(task, Vec::new()))));
    ok(w.call(&c, s, close_cmd(task, Outcome::Done)));
    ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Retract {
            target: RetractArg::Entry {
                eid: t1,
                node: task,
            },
            text: text("withdrawn"),
            reason: text("wrong"),
        }),
    ));
    let o = objected(&ok(w.call(&c, s, object(task))));
    let d = ok(w.call(&c, s, resolve(o, false, "fine")));
    assert!(warned(&d, WarningCode::NoBasis).is_empty());
}

// ------------------------------------------------------------------------------------------------
// A checkpoint ends its node's wait (§10.1, §10.2, §10.3)

fn release_waiting(t: Token, wait: WaitArg) -> Cmd {
    let mut cmd = release(t, ReleaseReason::Blocked);
    if let Cmd::Hold(HoldCmd::Release { waits_for, .. }) = &mut cmd {
        *waits_for = Some(wait);
    }
    cmd
}

/// l1 of the lab's fifth round: B took its own waiting node back, was told to ask the release's
/// author, itself, and worked on it while the wait stood. The take names the release and tells its
/// author that its next checkpoint ends the wait; the checkpoint does, and the node is offered
/// again once released. `renew` and `hold check {intent_done}` end no wait.
#[test]
fn a_checkpoint_ends_the_wait_and_the_take_tells_who_set_it() {
    let (mut w, c, s) = one();
    let s2 = w.hello(&c);
    let (n, t) = created(&ok(w.call(&c, s, crate::apply::tests::create("/fix"))));
    let until = w.clock.plus(Dur(4 * 3_600_000));
    ok(w.call(
        &c,
        s,
        release_waiting(
            t.unwrap(),
            WaitArg::External {
                what: text("Omar's choice of fix"),
                recheck_after: When::At(until),
            },
        ),
    ));
    let e = w.s.nodes[&n].last_checkpoint.unwrap();
    assert_eq!(
        w.s.nodes[&n].wait.as_ref().map(|x| x.on),
        Some(WaitOn::External(until))
    );
    // Its author takes it back: the warning says so.
    let d = ok(w.call(&c, s, take(n)));
    let t = token_of(&d);
    assert_eq!(
        warned(&d, WarningCode::Waiting)
            .into_iter()
            .map(|x| x.detail)
            .collect::<Vec<_>>(),
        vec![WarningDetail::Waiting {
            on: WaitOn::External(until),
            set_at: w.s.nodes[&n].wait.as_ref().unwrap().set_at,
            release: Some(e),
        }]
    );
    assert!(w.s.nodes[&n].wait.is_some(), "a take does not end it");
    // A renew does not end it.
    ok(w.call(
        &c,
        s,
        Cmd::Hold(HoldCmd::Renew {
            token: t,
            why: text("reading the thread"),
        }),
    ));
    assert!(w.s.nodes[&n].wait.is_some());
    // The checkpoint does.
    ok(w.call(&c, s, checkpoint(t)));
    assert!(w.s.nodes[&n].wait.is_none());
    assert!(w.last_events.iter().any(|ev| matches!(
        ev.ev,
        Ev::WaitSet { node, wait: None } if node == n
    )));
    // Released as a handoff, it is offered at once.
    ok(w.call(&c, s, release(t, ReleaseReason::Handoff)));
    assert!(w.s.ix.next.of.contains_key(&Cand::Node(n)));
    // Another session's take is told whose release set a wait, and that its first checkpoint
    // ends it.
    let t = token_of(&ok(w.call(&c, s, take(n))));
    let other = node(&mut w, &c, s, "/other", &[]);
    ok(w.call(&c, s, release_waiting(t, WaitArg::Node(id(other)))));
    let e = w.s.nodes[&n].last_checkpoint.unwrap();
    let d = ok(w.call(&c, s2, take(n)));
    assert!(matches!(
        warned(&d, WarningCode::Waiting)[0].detail,
        WarningDetail::Waiting {
            on: WaitOn::Node(m),
            release: Some(r),
            ..
        } if m == other && r == e
    ));
    // `hold check {intent_done}` writes a checkpoint entry but is no checkpoint of a hold.
    let t2 = token_of(&d);
    ok(w.call(
        &c,
        s2,
        Cmd::Hold(HoldCmd::Checkpoint {
            token: t2,
            done: text("did"),
            next_step: text("next"),
            intent: Some(text("push the fix")),
            intent_done: None,
            files: Vec::new(),
        }),
    ));
    assert!(
        w.s.nodes[&n].wait.is_none(),
        "s2's checkpoint ended the {{node}} wait"
    );
    ok(w.call(&c, s2, release_waiting(t2, WaitArg::Node(id(other)))));
    ok(w.call(
        &c,
        s,
        Cmd::Hold(HoldCmd::Check {
            target: id(n),
            token: None,
            intent_done: Some(text("it was pushed")),
        }),
    ));
    assert!(w.s.nodes[&n].wait.is_some());
}

// ------------------------------------------------------------------------------------------------
// An intent under a pause (§10.2, §19)

/// l6 of the lab's fifth round: the lead deployed under a freeze that showed nowhere it worked.
/// An intent recorded on a node under a pause warns `paused`, with its scope and end; one on a
/// node outside it, or a checkpoint without an intent, warns nothing.
#[test]
fn an_intent_under_a_pause_warns_paused() {
    let (mut w, c, s) = one();
    let (n, t) = created(&ok(w.call(
        &c,
        s,
        crate::apply::tests::create("/payments/vat-svc"),
    )));
    let (m, u) = created(&ok(w.call(
        &c,
        s,
        crate::apply::tests::create("/incidents/inc-311"),
    )));
    let (t, u) = (t.unwrap(), u.unwrap());
    let scope = w.s.nodes[&n].parent().unwrap();
    ok(w.call(
        &c,
        s,
        Cmd::Control(ControlCmd::Pause {
            scope: id(scope),
            reason: text("no payments deploys until the incident lead says so"),
        }),
    ));
    let until = w.s.pauses[&scope].until;
    let intent = |t: Token, i: Option<&str>| {
        Cmd::Hold(HoldCmd::Checkpoint {
            token: t,
            done: text("built the hotfix"),
            next_step: text("deploy"),
            intent: i.map(text),
            intent_done: None,
            files: Vec::new(),
        })
    };
    let d = ok(w.call(&c, s, intent(t, Some("deploy vat-svc v2.14.1 to prod"))));
    assert_eq!(
        warned(&d, WarningCode::Paused)
            .into_iter()
            .map(|x| (x.id, x.detail))
            .collect::<Vec<_>>(),
        vec![(Some(n), WarningDetail::Paused { scope, until })]
    );
    assert!(w.s.nodes[&n].open_intent.is_some(), "recorded");
    let d = ok(w.call(&c, s, intent(t, None)));
    assert!(warned(&d, WarningCode::Paused).is_empty());
    let d = ok(w.call(&c, s, intent(u, Some("sync the payments app"))));
    assert!(warned(&d, WarningCode::Paused).is_empty());
    assert!(w.s.nodes[&m].open_intent.is_some());
}
