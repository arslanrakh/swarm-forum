//! 1.8's corrections, intents, and tidying (design D.8.17, D.8.10, D.8.3, D.8.5): an objection
//! upheld on a closed node corrects it, and the node leaves `next`, `take`, and `leave_better`
//! (§5, §11.3); a brief lists the failed and refuted nodes its node rests on (§10.4); any session
//! records what became of an open intent with `hold check {intent_done}` (§10.2); and an offer
//! that nothing touched for `offer.stale` carries `stale` and comes last in its tier (§14, §10.4).
//! 1.9's tidying: an idle worker that `next` offers nothing gets the forum's stale offers and likely
//! duplicates as next calls (§10.3, §16, §20.9 rule 13). Every call runs through `World`, with
//! every check of a step, of a firing, and of the state, and G1 on its next calls.

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
use crate::apply::tests::{
    checkpoint, class, create_with, created, label, ok, refused, release, take, text,
};
use crate::cmd::*;
use crate::error::{Error, Hint, What, Why};
use crate::event::{EntryBody, Ev};
use crate::params::Params;
use crate::query::{NoMeter, RestsVia};
use crate::reply::{Corrected, NextReply, Reply, Total};
use crate::state::{Cand, Life, ObjectionState, Owner};
use crate::types::*;

// ------------------------------------------------------------------------------------------------
// Commands

fn object(n: NodeId) -> Cmd {
    Cmd::Talk(TalkCmd::Object {
        on: id(n),
        note: text("a counterexample at n = 11"),
    })
}

fn resolve(o: Eid, upheld: bool) -> Cmd {
    Cmd::Talk(TalkCmd::Resolve {
        objection: o,
        note: text("the counterexample checks"),
        upheld,
        defer: false,
    })
}

fn say(n: NodeId) -> Cmd {
    Cmd::Talk(TalkCmd::Say {
        at: id(n),
        text: text("still wanted"),
        to: None,
        expects: None,
        reply_to: None,
        until: None,
    })
}

fn check(n: NodeId, intent_done: Option<&str>) -> Cmd {
    Cmd::Hold(HoldCmd::Check {
        target: id(n),
        token: None,
        intent_done: intent_done.map(text),
    })
}

fn objected(d: &crate::reply::Done) -> Eid {
    match d.reply {
        Reply::Objected { objection } => objection,
        ref other => panic!("not objected: {other:?}"),
    }
}

fn token_of(d: &crate::reply::Done) -> Token {
    match d.reply {
        Reply::Taken { token, .. } => token,
        ref other => panic!("not taken: {other:?}"),
    }
}

/// Creates `p` without a hold, with `f` applied to its arguments.
fn node_with(
    w: &mut crate::testkit::World,
    c: &ClassName,
    s: SessionId,
    p: &str,
    f: impl FnOnce(&mut CreateArgs),
) -> NodeId {
    let d = ok(w.call(
        c,
        s,
        create_with(p, |a| {
            a.take = Some(false);
            f(a);
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

// ------------------------------------------------------------------------------------------------
// C1: an upheld objection corrects a closed node

/// A claim closed `done` with an objection that stands: one upheld resolve makes it `refuted`,
/// marked upheld, with no counter changed; it leaves `next` and `leave_better`, a take of it is
/// `closed`, and a decision that cited it as done now has a stale basis (§5, §11.3, §15).
#[test]
fn an_upheld_objection_corrects_a_closed_claim_and_keeps_it_from_next() {
    let (mut w, c, s) = one();
    let s2 = w.hello(&c);
    let lemma = node_with(&mut w, &c, s, "/claims/lemma-04", |a| {
        a.kind = Some(label(CLAIM));
    });
    let thm = node(&mut w, &c, s, "/theorems/thm-1", &[]);
    ok(w.call(&c, s, close_cmd(lemma, Outcome::Done)));
    // thm-1 rests on the lemma as done.
    ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Decide {
            at: id(thm),
            text: text("the bound holds by lemma 4"),
            settles: BTreeSet::new(),
            supersedes: None,
            basis: vec![BasisArg::Node {
                at: id(lemma),
                version: None,
            }],
            evidence: None,
        }),
    ));
    assert!(!w.s.ix.node[&thm].flags.contains(Flag::BasisChanged));
    let o = objected(&ok(w.call(&c, s2, object(lemma))));
    // 1.9 (§10.1, §10.4): a closed node with an objection is a review item, with no hold: `next`
    // gives it as `{review}` and reopens nothing, and a take of it is `closed`.
    assert!(w.s.ix.next.of.contains_key(&Cand::Status(lemma)));
    assert!(!w.s.ix.next.of.contains_key(&Cand::Node(lemma)));
    let s3 = w.hello(&c);
    let d = ok(w.call(
        &c,
        s3,
        Cmd::Hold(HoldCmd::Next {
            query: NextQuery {
                under: Some(id(lemma)),
                ..NextQuery::default()
            },
            lease: None,
        }),
    ));
    assert!(
        matches!(d.reply, Reply::Next(NextReply::Review { node, objection, why })
            if node == lemma && objection == o && why.reason == crate::reply::Reason::Review),
        "{:?}",
        d.reply
    );
    assert!(!w.s.nodes[&lemma].is_open());
    assert!(w.s.holds.keys().all(|t| t.node != lemma));
    assert_eq!(
        w.call(&c, s3, take(lemma)).err(),
        Some(Error::of(Hint::Closed {
            node: lemma,
            merged_into: None,
        }))
    );
    assert!(!w.s.nodes[&lemma].is_open());
    let version = w.s.nodes[&lemma].version;
    let d = ok(w.call(&c, s2, resolve(o, true)));
    let Reply::ObjectionResolved {
        entry: Some(e),
        corrected: Some(x),
        ..
    } = d.reply
    else {
        panic!("{:?}", d.reply)
    };
    assert_eq!(
        x,
        Corrected {
            node: lemma,
            was: Outcome::Done,
            outcome: refuted(),
        }
    );
    assert!(w.last_events.iter().any(|ev| matches!(
        &ev.ev,
        Ev::Appended {
            body: EntryBody::Resolve { upheld: true, .. },
            ..
        }
    )));
    assert_eq!(w.s.objections[&o].state, ObjectionState::Upheld { by: e });
    let cl = closure(&w, lemma);
    assert_eq!((cl.outcome, cl.upheld), (refuted(), Some(e)));
    // O2: no counter moves.
    assert_eq!(w.s.nodes[&lemma].version, version);
    // The flag cleared, the node is no candidate, and nothing suggests it.
    assert!(!w.s.ix.node[&lemma].flags.contains(Flag::Objection));
    assert!(!w.s.ix.next.of.contains_key(&Cand::Node(lemma)));
    assert!(
        crate::index::classify_from(
            &crate::index::ClassInputs {
                reserved: false,
                open: false,
                occ_waiting: false,
                occ_locked: false,
                merged: false,
                upheld: true,
                capacity: 1,
                live: 0,
                offered: false,
                available: false,
            },
            false,
            FlagSet::EMPTY.with(Flag::UnansweredRequest),
        )
        .is_none()
    );
    // §15: the citing decision's basis went stale.
    assert!(w.s.ix.node[&thm].flags.contains(Flag::BasisChanged));
    // §10.1: a take is `closed`, though another flag were on it.
    let r = w.call(&c, s, take(lemma));
    assert_eq!(
        r.err(),
        Some(Error::of(Hint::Closed {
            node: lemma,
            merged_into: None,
        }))
    );
    // A request recorded on it later flags it, and still nothing offers it (§10.4).
    crate::apply::tests::entry(
        &mut w,
        lemma,
        crate::state::Author::Session(s),
        EntryBody::Object,
    );
    assert!(w.s.ix.node[&lemma].flags.contains(Flag::Objection));
    assert!(!w.s.ix.next.of.contains_key(&Cand::Node(lemma)));
    assert!(!w.s.ix.next.of.contains_key(&Cand::Status(lemma)));
    assert!(
        crate::spec::ranked(&w.s, s, w.clock, &NextQuery::default())
            .iter()
            .all(|x| *x != Cand::Node(lemma))
    );
    // `node reopen` reopens it, and the mark goes with the closure.
    ok(w.call(
        &c,
        s,
        Cmd::Node(NodeCmd::Reopen {
            at: id(lemma),
            reason: text("a new proof attempt"),
        }),
    ));
    assert!(w.s.nodes[&lemma].is_open());
}

/// The outcome a correction gives (§5, §11.3): `done` becomes `failed` for a node that is not a
/// claim; another outcome stays, and the node is marked all the same. On an open node, an upheld
/// objection records its verdict only.
#[test]
fn an_upheld_objection_on_other_nodes() {
    let (mut w, c, s) = one();
    let fix = node(&mut w, &c, s, "/web/fix", &[]);
    ok(w.call(&c, s, close_cmd(fix, Outcome::Done)));
    let o = objected(&ok(w.call(&c, s, object(fix))));
    let d = ok(w.call(&c, s, resolve(o, true)));
    assert!(matches!(
        d.reply,
        Reply::ObjectionResolved {
            corrected: Some(Corrected {
                was: Outcome::Done,
                outcome: Outcome::Failed,
                ..
            }),
            ..
        }
    ));
    assert_eq!(closure(&w, fix).outcome, Outcome::Failed);
    // Closed `dropped`: the outcome stays, the mark is set.
    let dropped = node(&mut w, &c, s, "/web/dropped", &[]);
    ok(w.call(&c, s, close_cmd(dropped, Outcome::Dropped)));
    let o = objected(&ok(w.call(&c, s, object(dropped))));
    ok(w.call(&c, s, resolve(o, true)));
    let cl = closure(&w, dropped);
    assert_eq!(cl.outcome, Outcome::Dropped);
    assert!(cl.upheld.is_some());
    // Open: the verdict only.
    let open = node(&mut w, &c, s, "/web/open", &[]);
    let o = objected(&ok(w.call(&c, s, object(open))));
    let d = ok(w.call(&c, s, resolve(o, true)));
    assert!(matches!(
        d.reply,
        Reply::ObjectionResolved {
            entry: Some(_),
            corrected: None,
            ..
        }
    ));
    assert!(w.s.nodes[&open].is_open());
    assert!(matches!(
        w.s.objections[&o].state,
        ObjectionState::Upheld { .. }
    ));
    assert!(
        !w.last_events
            .iter()
            .any(|ev| matches!(ev.ev, Ev::NodeCorrected { .. }))
    );
    // Not upheld: as before, on a closed node too.
    let kept = node(&mut w, &c, s, "/web/kept", &[]);
    ok(w.call(&c, s, close_cmd(kept, Outcome::Done)));
    let o = objected(&ok(w.call(&c, s, object(kept))));
    ok(w.call(&c, s, resolve(o, false)));
    let cl = closure(&w, kept);
    assert_eq!((cl.outcome, cl.upheld), (Outcome::Done, None));
    assert!(matches!(
        w.s.objections[&o].state,
        ObjectionState::Resolved { .. }
    ));
}

/// The brief's `rests_on` (§10.4): the nodes its node depends on, or that its active decisions
/// cite, that are closed `failed` or `refuted`, with how it rests on each.
#[test]
fn a_brief_lists_the_failed_and_refuted_nodes_it_rests_on() {
    let (mut w, c, s) = one();
    let l4 = node_with(&mut w, &c, s, "/claims/lemma-04", |a| {
        a.kind = Some(label(CLAIM));
    });
    let l6 = node(&mut w, &c, s, "/claims/lemma-06", &[]);
    let l7 = node(&mut w, &c, s, "/claims/lemma-07", &[]);
    let run = node(&mut w, &c, s, "/runs/run-1", &[]);
    let thm = node_with(&mut w, &c, s, "/theorems/thm-1", |a| {
        a.depends_on = vec![id(l4), id(l6), id(l7)];
    });
    ok(w.call(&c, s, close_cmd(l4, Outcome::Done)));
    ok(w.call(&c, s, close_cmd(l6, Outcome::Failed)));
    ok(w.call(&c, s, close_cmd(l7, Outcome::Done)));
    ok(w.call(&c, s, close_cmd(run, Outcome::Failed)));
    let d = ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Decide {
            at: id(thm),
            text: text("the base cases hold by the run"),
            settles: BTreeSet::new(),
            supersedes: None,
            basis: vec![BasisArg::Node {
                at: id(run),
                version: None,
            }],
            evidence: None,
        }),
    ));
    let Reply::Decided { decision, .. } = d.reply else {
        panic!()
    };
    let o = objected(&ok(w.call(&c, s, object(l4))));
    ok(w.call(&c, s, resolve(o, true)));
    let b = crate::query::brief(&w.s, thm, Some(s), w.clock, &mut NoMeter);
    let got: Vec<(NodeId, Outcome, RestsVia)> = b
        .rests_on
        .iter()
        .map(|r| (r.node, r.outcome.clone(), r.via))
        .collect();
    assert_eq!(
        got,
        vec![
            (l4, refuted(), RestsVia::DependsOn),
            (l6, Outcome::Failed, RestsVia::DependsOn),
            (run, Outcome::Failed, RestsVia::Basis(decision)),
        ]
    );
    // A node that rests on nothing that failed lists nothing.
    let b = crate::query::brief(&w.s, l7, Some(s), w.clock, &mut NoMeter);
    assert!(b.rests_on.is_empty());
}

// ------------------------------------------------------------------------------------------------
// C2: a check records what became of an open intent

/// A session opens an intent and vanishes; another records what became of it with `hold check
/// {intent_done}`, without a hold, on an open node and on a closed one (§10.2): a `checkpoint`
/// entry by the checker that closes the intent, extends no lease, and counts for no counter.
#[test]
fn a_check_records_what_became_of_a_gone_sessions_intent() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/payments/vat-hotfix", &[]);
    let t = token_of(&ok(w.call(&c, s, take(n))));
    let mut cp = checkpoint(t);
    if let Cmd::Hold(HoldCmd::Checkpoint { intent, .. }) = &mut cp {
        *intent = Some(text("deploy vat-svc v2.14.1"));
    }
    let d = ok(w.call(&c, s, cp));
    let Reply::Checkpointed {
        intent: Some(intent),
        ..
    } = d.reply
    else {
        panic!("{:?}", d.reply)
    };
    // While its session is live, its intent is not a gone session's.
    assert_eq!(crate::query::gone_intent(&w.s, n), None);
    // The session vanishes: its lease ends, then it falls silent.
    w.advance(Dur(2 * HOUR.0));
    assert!(w.s.sessions[&s].silent);
    assert_eq!(
        crate::query::gone_intent(&w.s, n).map(|i| i.eid),
        Some(intent)
    );
    assert!(w.s.ix.node[&n].flags.contains(Flag::OpenIntent));
    let s2 = w.hello(&c);
    // Without an intent open it is invalid; without `intent_done` it reads.
    let other = node(&mut w, &c, s2, "/payments/other", &[]);
    refused(
        &mut w,
        &c,
        s2,
        check(other, Some("nothing to check")),
        Error::invalid(Why::NoIntent),
    );
    refused(
        &mut w,
        &c,
        s2,
        check(NodeId(999), Some("x")),
        Error::not_found(What::Node),
    );
    assert!(check(n, None).is_read());
    assert!(!check(n, Some("x")).is_read());
    let checkpoints = w.s.nodes[&n].checkpoints;
    let d = ok(w.call(
        &c,
        s2,
        check(n, Some("v2.14.1 was deployed from #38; nothing to redo")),
    ));
    let Reply::HoldCheck {
        entry: Some(e),
        intent_done: Some(closed),
        ..
    } = d.reply
    else {
        panic!("{:?}", d.reply)
    };
    assert_eq!(closed, intent);
    assert_eq!(w.s.nodes[&n].open_intent, None);
    assert!(!w.s.ix.node[&n].flags.contains(Flag::OpenIntent));
    assert_eq!(w.s.nodes[&n].checkpoints, checkpoints);
    assert!(w.last_events.iter().any(|ev| matches!(
        &ev.ev,
        Ev::Appended {
            eid,
            by: crate::state::Author::Session(x),
            body: EntryBody::IntentChecked { intent: i, .. },
            ..
        } if *eid == e && *x == s2 && *i == intent
    )));
    // On a closed node: a close by another session leaves the intent open (§5); a check closes
    // it without a reopen.
    let t = token_of(&ok(w.call(&c, s2, take(other))));
    let mut cp = checkpoint(t);
    if let Cmd::Hold(HoldCmd::Checkpoint { intent, .. }) = &mut cp {
        *intent = Some(text("push the branch"));
    }
    ok(w.call(&c, s2, cp));
    ok(w.call(&c, s2, release(t, ReleaseReason::Handoff)));
    let s3 = w.hello(&c);
    ok(w.call(&c, s3, close_cmd(other, Outcome::Done)));
    assert!(w.s.nodes[&other].open_intent.is_some());
    assert!(w.s.ix.node[&other].flags.contains(Flag::OpenIntent));
    ok(w.call(&c, s3, check(other, Some("the branch was pushed"))));
    assert!(!w.s.nodes[&other].is_open());
    assert_eq!(w.s.nodes[&other].open_intent, None);
    assert!(w.s.ix.node[&other].flags.is_empty());
}

// ------------------------------------------------------------------------------------------------
// C3: stale offers

/// An offer that nothing touches for `offer.stale` carries `stale`, stays in its tier, and comes
/// after the rest of it in `next`; `leave_better` lists it; any write to it clears the flag (§14,
/// §10.3, §10.4). A held, a waiting, and a container node never go stale.
#[test]
fn an_untouched_offer_goes_stale_and_comes_last_in_its_tier() {
    let mut w = crate::apply::tests::world();
    let c = class("worker");
    let s = w.hello(&c);
    let offer = |a: &mut CreateArgs| {
        a.tags = [label(OFFER)].into_iter().collect();
    };
    let old = node_with(&mut w, &c, s, "/web/legacy-ie11", offer);
    let series_like = node_with(&mut w, &c, s, "/@people/kenji", |a| {
        offer(a);
        a.capacity = Some(0);
    });
    let stale = w.s.params.offer_stale.unwrap();
    // 60 days later, a second offer; then 31 more: the first is 91 days untouched.
    w.advance(Dur(stale.0 / 90 * 60));
    let s = w.hello(&c);
    let young = node_with(&mut w, &c, s, "/web/tablet-layout", offer);
    let waits = node_with(&mut w, &c, s, "/web/waits", offer);
    let t = token_of(&ok(w.call(&c, s, take(waits))));
    let mut rel = release(t, ReleaseReason::Blocked);
    if let Cmd::Hold(HoldCmd::Release { waits_for, .. }) = &mut rel {
        *waits_for = Some(WaitArg::Node(id(young)));
    }
    ok(w.call(&c, s, rel));
    w.advance(Dur(stale.0 / 90 * 31));
    assert!(w.s.ix.node[&old].flags.contains(Flag::Stale));
    assert!(!w.s.ix.node[&young].flags.contains(Flag::Stale));
    assert!(!w.s.ix.node[&series_like].flags.contains(Flag::Stale));
    // Waiting since 31 days, untouched: not stale while it waits.
    assert!(!w.s.ix.node[&waits].flags.contains(Flag::Stale));
    // Both tier 2; the stale one after the other.
    let s = w.hello(&c);
    let order = crate::spec::ranked(&w.s, s, w.clock, &NextQuery::default());
    let pos = |n: NodeId| order.iter().position(|x| *x == Cand::Node(n)).unwrap();
    assert!(pos(young) < pos(old), "{order:?}");
    assert_eq!(
        w.s.ix.next.of[&Cand::Node(old)].key.tier,
        w.s.ix.next.of[&Cand::Node(young)].key.tier
    );
    let d = ok(w.call(&c, s, crate::apply::tests::next()));
    let Reply::Next(NextReply::Hold { node, token, .. }) = d.reply else {
        panic!("{:?}", d.reply)
    };
    assert_eq!(node, young);
    // A close under /web lists the stale offer in leave_better.
    let d = ok(w.call(
        &c,
        s,
        Cmd::Node(NodeCmd::Close {
            at: id(young),
            outcome: Outcome::Done,
            summary: text("done"),
            verified: None,
            settles: Some(BTreeSet::new()),
            learnings: None,
            token: Some(token),
            unsent: BTreeSet::new(),
        }),
    ));
    let Reply::Closed { leave_better, .. } = d.reply else {
        panic!("{:?}", d.reply)
    };
    assert!(leave_better.contains(&old), "{leave_better:?}");
    // A held offer is not stale; a note clears the flag, and the clock starts again.
    let t = token_of(&ok(w.call(&c, s, take(old))));
    assert!(!w.s.ix.node[&old].flags.contains(Flag::Stale));
    ok(w.call(&c, s, release(t, ReleaseReason::Handoff)));
    assert!(!w.s.ix.node[&old].flags.contains(Flag::Stale));
    w.advance(Dur(stale.0 - 1));
    let s = w.hello(&c);
    assert!(!w.s.ix.node[&old].flags.contains(Flag::Stale));
    ok(w.call(&c, s, say(old)));
    w.advance(Dur(stale.0 - 1));
    let s = w.hello(&c);
    assert!(!w.s.ix.node[&old].flags.contains(Flag::Stale));
    w.advance(Dur(1));
    assert!(w.s.ix.node[&old].flags.contains(Flag::Stale));
    assert!(w.log.iter().any(|e| matches!(
        e.ev,
        Ev::Fired {
            kind: crate::state::DeadlineKind::Stale,
            owner: Owner::Node(x),
        } if x == old
    )));
    // An update of its fields touches it too.
    ok(w.call(
        &c,
        s,
        Cmd::Node(NodeCmd::Update {
            at: id(old),
            version: w.s.nodes[&old].version,
            digest_version: None,
            token: None,
            patch: Patch {
                title: Some(text("IE 11 layout, still wanted")),
                ..Patch::default()
            },
        }),
    ));
    assert!(!w.s.ix.node[&old].flags.contains(Flag::Stale));
}

/// A lower `offer.stale` moves every Stale key and marker at once (T3, T4): `World` checks the
/// queue against its recount after the event.
#[test]
fn a_lower_offer_stale_applies_at_once() {
    let mut w = crate::apply::tests::world();
    let c = class("worker");
    let s = w.hello(&c);
    let n = node_with(&mut w, &c, s, "/web/x", |a| {
        a.tags = [label(OFFER)].into_iter().collect();
    });
    w.advance(Dur(10 * DAY.0));
    assert!(!w.s.ix.node[&n].flags.contains(Flag::Stale));
    let mut p = w.s.params.clone();
    p.offer_stale = Some(Dur(5 * DAY.0));
    w.emit(Ev::ParamsChanged { params: p });
    assert!(w.s.ix.node[&n].flags.contains(Flag::Stale));
    assert!(w.s.nodes[&n].stale_fired);
    w.check().unwrap();
    let mut p = w.s.params.clone();
    p.offer_stale = Some(Dur(20 * DAY.0));
    w.emit(Ev::ParamsChanged { params: p });
    assert!(!w.s.ix.node[&n].flags.contains(Flag::Stale));
    assert!(!w.s.nodes[&n].stale_fired);
    w.check().unwrap();
}

/// A history written before 1.8, whose parameters have no `offer.stale`, tracks no touch, so it
/// replays to the state it wrote (D.7.6): no node is touched, no Stale key is queued, and no node
/// goes stale, however old its offer.
#[test]
fn without_offer_stale_nothing_is_tracked() {
    let p = Params {
        offer_stale: None,
        ..Params::default()
    };
    let mut w = crate::testkit::World::new(p, T0);
    let c = class("worker");
    let s = w.hello(&c);
    let n = node_with(&mut w, &c, s, "/web/x", |a| {
        a.tags = [label(OFFER)].into_iter().collect();
    });
    w.advance(Dur(400 * DAY.0));
    assert_eq!(w.s.nodes[&n].touched_at, None);
    assert!(
        w.s.ix
            .deadlines
            .iter()
            .all(|k| k.kind != crate::state::DeadlineKind::Stale)
    );
    assert!(!w.s.ix.node[&n].flags.contains(Flag::Stale));
}

// ------------------------------------------------------------------------------------------------
// 1.9: the forum's tidy items for an idle worker

/// An offer only a `web` worker is offered, titled `title`.
fn web_offer(title: &str) -> impl FnOnce(&mut CreateArgs) + '_ {
    move |a: &mut CreateArgs| {
        a.tags = [label(OFFER)].into_iter().collect();
        a.needs = Some([label("web")].into_iter().collect());
        a.title = text(title);
    }
}

/// The calls of the last response's next calls.
fn calls(w: &crate::testkit::World) -> Vec<crate::query::guide::Skeleton> {
    w.last_guidance()
        .items
        .iter()
        .map(|i| i.call.clone())
        .collect()
}

/// An offer that needs `need`, titled `title`, with `body`.
fn offer_of<'a>(need: &'a str, title: &'a str, body: &'a str) -> impl FnOnce(&mut CreateArgs) + 'a {
    move |a: &mut CreateArgs| {
        a.tags = [label(OFFER)].into_iter().collect();
        a.needs = Some([label(need)].into_iter().collect());
        a.title = text(title);
        a.body = Some(text(body));
    }
}

/// 1.9 (§10.3, §16, §20.9 rule 13; l4 of the lab's fourth round, as in the three before): a worker
/// that `next` offers nothing gets, at `hello` and after an empty `next`, the forum's stale offers
/// and possible duplicates, wherever they are and whatever its caps: 1.10's question for the owners
/// of each stale offer, and a duplicate link from the newer of two open nodes under one parent that
/// share rare words of their titles and bodies to the older. The seed's pair, "Review warehouse
/// costs" and "Why is Snowflake spend up?", which share no word of their titles and which two
/// sessions created, weeks apart, is found; unrelated siblings are not; and a pair once linked is
/// not suggested again.
#[test]
fn an_idle_worker_gets_the_forum_s_stale_offers_and_possible_duplicates() {
    use crate::query::guide::{Because, Rule, Skeleton};
    let mut w = crate::apply::tests::world();
    let c = class("worker");
    let s = w.hello(&c);
    let old = node_with(
        &mut w,
        &c,
        s,
        "/web/legacy-ie11",
        offer_of(
            "web",
            "Keep the checkout working on IE 11",
            "Corporate customers on IE 11 report a blank checkout. Investigate a polyfill bundle.",
        ),
    );
    let stale = w.s.params.offer_stale.unwrap();
    w.advance(Dur(stale.0 + DAY.0));
    let s = w.hello(&c);
    node_with(
        &mut w,
        &c,
        s,
        "/web/tablet-layout",
        offer_of(
            "web",
            "Tablet layout for the product screen",
            "Use the width on tablets.",
        ),
    );
    let costs = node_with(
        &mut w,
        &c,
        s,
        "/data/warehouse-cost-review",
        offer_of(
            "sql",
            "Review warehouse costs",
            "Snowflake costs grew 40 % this quarter; find the biggest warehouses and queries.",
        ),
    );
    // 1.10: another session, as in the seed (week 44, after week 40's).
    let s2 = w.hello(&c);
    let spend = node_with(
        &mut w,
        &c,
        s2,
        "/data/review-warehouse-spend",
        offer_of(
            "sql",
            "Why is Snowflake spend up?",
            "Finance asks why Snowflake spend is up; look at credit usage by warehouse.",
        ),
    );
    node_with(
        &mut w,
        &c,
        s,
        "/data/airflow-2-10",
        offer_of(
            "python",
            "Upgrade Airflow to 2.10",
            "Upgrade the scheduler and workers from 2.7 to 2.10; the nightly_core dag must keep \
             running.",
        ),
    );
    node_with(
        &mut w,
        &c,
        s,
        "/data/nightly-sla-alerts",
        offer_of(
            "python",
            "Alert when nightly_core publishes late",
            "Page the data on-call when the BI schemas are not published by 05:00 UTC.",
        ),
    );
    assert!(w.s.ix.node[&old].flags.contains(Flag::Stale));
    // A worker with no caps: nothing is offered to it.
    let x = w.hello(&c);
    let want = vec![
        Skeleton::AskOwners { at: old },
        Skeleton::LinkDuplicate {
            from: spend,
            to: costs,
        },
    ];
    assert_eq!(calls(&w), want);
    let g = w.last_guidance().clone();
    assert_eq!(g.total, Total::Exact(2));
    assert_eq!(g.end, None);
    assert!(g.items.iter().all(|i| i.rule == Rule::Tidy && i.once));
    assert_eq!(
        g.items[0].because,
        Because::StaleOffer {
            node: old,
            cut: false
        }
    );
    assert_eq!(
        g.items[1].because,
        Because::Alike {
            from: spend,
            to: costs,
            cut: false
        }
    );
    // An empty `next` gives the same items, as `spec::guide` does.
    let cmd = crate::apply::tests::next();
    let out = w.call(&c, x, cmd.clone());
    assert!(matches!(
        out,
        Ok(crate::reply::Done {
            reply: Reply::Next(NextReply::Empty { .. }),
            ..
        })
    ));
    assert_eq!(calls(&w), want);
    assert_eq!(
        *w.last_guidance(),
        crate::spec::guide(&w.s, Some(x), w.clock, &cmd, &out)
    );
    // The link opens a duplicate signal; the pair is not suggested again, and `next` now offers
    // the signal, so the worker is sent there instead.
    ok(w.call(
        &c,
        x,
        super::keys::tests::link(spend, LinkRel::Duplicate, costs),
    ));
    let d = ok(w.call(&c, x, crate::apply::tests::next()));
    assert!(
        matches!(d.reply, Reply::Next(NextReply::Hold { .. })),
        "{:?}",
        d.reply
    );
    // A worker with the caps is offered the work: no tidy items for it.
    let y = w.s.counters.next_session;
    let ctx = w.ctx(&c, SessionCtx::New, None);
    let d = w.call_ctx(
        &ctx,
        &Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps: Some([label("web")].into_iter().collect()),
            horizon: None,
            principal: None,
            from_user: None,
        }),
    );
    assert_eq!(ok(d).opened, Some(y));
    assert!(
        calls(&w)
            .iter()
            .all(|k| !matches!(k, Skeleton::LinkDuplicate { .. }))
    );
}

/// 1.9 (§16; l6 of the lab's fourth round): the incident lead's node and the support thread's node
/// about the same incident, "INC-311: EU checkout returns 500s" and "Checkout errors for EU
/// customers", share `checkout` and `eu`, which no other node there carries, and each title holds
/// them: a possible duplicate, linked from the newer, unheld thread node to the lead's. An
/// unrelated sibling is not paired with either.
#[test]
fn the_incident_and_its_support_thread_are_possible_duplicates() {
    use crate::query::guide::Skeleton;
    let mut w = crate::apply::tests::world();
    let c = class("worker");
    let lead = w.hello(&c);
    let inc = {
        let d = ok(w.call(
            &c,
            lead,
            create_with("/incidents/inc-311-eu-checkout", |a| {
                a.title = text("INC-311: EU checkout returns 500s");
                a.body = Some(text(
                    "EU checkout requests fail with HTTP 500 since 16:05 UTC (about 18 % of EU \
                     checkouts).",
                ));
            }),
        ));
        crate::apply::tests::created(&d).0
    };
    let s = w.hello(&c);
    let thread = node_with(
        &mut w,
        &c,
        s,
        "/incidents/checkout-errors-eu",
        offer_of("chat", "Checkout errors for EU customers", ""),
    );
    node_with(
        &mut w,
        &c,
        s,
        "/incidents/postmortem-template",
        offer_of(
            "chat",
            "Postmortem template for incidents",
            "Agree on the sections every postmortem has.",
        ),
    );
    let _ = w.hello(&c);
    assert_eq!(
        calls(&w),
        vec![Skeleton::LinkDuplicate {
            from: thread,
            to: inc
        }]
    );
}

/// 1.9 (§16): the merge's direction picks the link's: a newer node that another session holds
/// is the one that remains; a pair with any link between them is not suggested.
#[test]
fn a_possible_duplicate_link_keeps_a_held_node_and_skips_linked_pairs() {
    use crate::query::guide::Skeleton;
    let mut w = crate::apply::tests::world();
    let c = class("worker");
    let s = w.hello(&c);
    let a = node_with(
        &mut w,
        &c,
        s,
        "/web/a",
        web_offer("Migrate the build to Bazel"),
    );
    // 1.10: another session's; one session's two nodes are its plan.
    let s2 = w.hello(&c);
    let b = node_with(
        &mut w,
        &c,
        s2,
        "/web/b",
        web_offer("Migrate build to Bazel"),
    );
    let h = w.hello(&c);
    ok(w.call(&c, h, take(b)));
    let _ = w.hello(&c);
    assert_eq!(calls(&w), vec![Skeleton::LinkDuplicate { from: a, to: b }]);
    ok(w.call(&c, s, super::keys::tests::link(a, LinkRel::Continues, b)));
    let _ = w.hello(&c);
    assert!(calls(&w).is_empty());
    assert!(w.last_guidance().end.is_some());
}

/// 1.9 (§18): at most three tidy items; with more found, or a scan its bound stopped, `total` is
/// `≥k`. Words that four nodes carry are not rare, so four offers of one title are no pairs.
#[test]
fn the_tidy_items_are_three_at_most_and_say_when_more_were_left() {
    use crate::query::guide::Skeleton;
    use crate::query::leave_better::{TidyItem, tidy_within};
    let mut w = crate::apply::tests::world();
    let c = class("worker");
    let s = w.hello(&c);
    let o: Vec<NodeId> = (0..4)
        .map(|i| {
            node_with(
                &mut w,
                &c,
                s,
                &alloc::format!("/web/o{i}"),
                web_offer("An old offer"),
            )
        })
        .collect();
    let stale = w.s.params.offer_stale.unwrap();
    w.advance(Dur(stale.0 + DAY.0));
    let x = w.hello(&c);
    let g = w.last_guidance().clone();
    assert_eq!(
        calls(&w),
        vec![
            Skeleton::AskOwners { at: o[0] },
            Skeleton::AskOwners { at: o[1] },
            Skeleton::AskOwners { at: o[2] },
        ]
    );
    assert_eq!(g.total, Total::AtLeast(3));
    // A bound of a few visits stops the scan, and says so.
    let t = tidy_within(&w.s, x, w.clock, 3, &mut NoMeter);
    assert!(t.cut);
    assert!(t.items.iter().all(|i| matches!(i, TidyItem::Stale(..))));
}

/// 1.9: a stale offer and a possible duplicate in turn, at most three, saying when some were left.
#[test]
fn tidy_items_come_in_turn() {
    use crate::query::leave_better::{StaleStep, TidyItem, interleave};
    let n = NodeId;
    let ask = StaleStep::Ask;
    let (items, more) = interleave(
        &[(n(1), ask), (n(2), ask)],
        &[(3, n(9), n(8)), (2, n(7), n(6))],
    );
    assert_eq!(
        items,
        vec![
            TidyItem::Stale(n(1), ask),
            TidyItem::Alike {
                from: n(9),
                to: n(8),
                shared: 3
            },
            TidyItem::Stale(n(2), ask),
        ]
    );
    assert!(more);
    assert_eq!(interleave(&[], &[]), (vec![], false));
}

/// 1.9 (§20.9 rule 13, groups B and C together): a worker's own review comes before the forum's
/// tidy items. After a release whose `leave_better` names a closed node with an open objection,
/// the first item is the resolve of that objection, never a take of the node; then, since `next`
/// offers the worker nothing, the forum's stale offers follow; `spec::guide` gives the same.
#[test]
fn a_review_comes_before_the_forum_s_tidy_items() {
    use crate::query::guide::{Because, Rule, Skeleton};
    let mut w = crate::apply::tests::world();
    let c = class("worker");
    let s = w.hello(&c);
    let old = node_with(
        &mut w,
        &c,
        s,
        "/web/legacy-ie11",
        web_offer("Keep the checkout working on IE 11"),
    );
    let stale = w.s.params.offer_stale.unwrap();
    w.advance(Dur(stale.0 + DAY.0));
    let s = w.hello(&c);
    // A claim that needs `web`, closed as done, with an open objection.
    let claim = node_with(
        &mut w,
        &c,
        s,
        "/p/claim",
        web_offer("The parser handles every input"),
    );
    ok(w.call(&c, s, close_cmd(claim, Outcome::Done)));
    let o = objected(&ok(w.call(&c, s, object(claim))));
    // A worker with no caps holds a sibling that needs `web` too, and releases it.
    let x = w.hello(&c);
    let d = ok(w.call(
        &c,
        x,
        create_with("/p/fuzz", |a| {
            a.needs = Some([label("web")].into_iter().collect());
            a.title = text("Fuzz the parser");
        }),
    ));
    let cmd = release(created(&d).1.unwrap(), ReleaseReason::Handoff);
    let out = w.call(&c, x, cmd.clone());
    assert!(
        matches!(&out, Ok(crate::reply::Done { reply: Reply::Released { leave_better, .. }, .. })
            if leave_better.first() == Some(&claim)),
        "{out:?}"
    );
    assert_eq!(
        calls(&w),
        vec![
            Skeleton::Resolve { objection: o },
            Skeleton::AskOwners { at: old }
        ]
    );
    let g = w.last_guidance().clone();
    assert!(g.items.iter().all(|i| i.rule == Rule::Tidy));
    assert_eq!(g.items[0].because, Because::Review { objection: o });
    assert!(matches!(g.items[1].because, Because::StaleOffer { node, .. } if node == old));
    assert_eq!(g.end, None);
    assert_eq!(
        *w.last_guidance(),
        crate::spec::guide(&w.s, Some(x), w.clock, &cmd, &out)
    );
}

// ------------------------------------------------------------------------------------------------
// 1.10: the lab's fifth round

/// The question a stale offer's item asks, as the shell decodes it.
fn ask_owners(n: NodeId) -> Cmd {
    Cmd::Talk(TalkCmd::Ask {
        at: id(n),
        text: text("No one has touched this offer since January: is it still wanted?"),
        to: AskTo::User(crate::query::guide::stale_ask()),
        again: false,
    })
}

/// 1.10 (§10.4; it4 l5 F1): review items rank by the ring before age, so concurrent sessions start
/// at different reviews: each session's first review is the one nearest its point on the ring,
/// whatever the reviews' ages, and the sessions of the lab's three do not all start at the oldest.
#[test]
fn concurrent_sessions_start_at_different_reviews() {
    let mut w = crate::apply::tests::world();
    let c = class("worker");
    let s = w.hello(&c);
    let mut reviews = Vec::new();
    for i in 0..3 {
        let s = w.hello(&c);
        let n = node_with(&mut w, &c, s, &alloc::format!("/claims/lemma-{i}"), |a| {
            a.kind = Some(label(CLAIM));
        });
        ok(w.call(&c, s, close_cmd(n, Outcome::Done)));
        ok(w.call(&c, s, object(n)));
        reviews.push(n);
        w.advance(Dur(3 * HOUR.0));
    }
    let _ = s;
    let mut firsts = BTreeSet::new();
    for _ in 0..6 {
        let x = w.hello(&c);
        let (got, _) =
            crate::query::ranked(&w.s, x, w.clock, &NextQuery::default(), 3, &mut NoMeter);
        let got: Vec<Cand> = got.into_iter().map(|r| r.cand).collect();
        let mut want = reviews.clone();
        want.sort_by_key(|n| crate::fnv::tie_hash(x, n.0));
        assert_eq!(
            got,
            want.iter().map(|n| Cand::Status(*n)).collect::<Vec<_>>(),
            "the ring's order for {x:?}"
        );
        firsts.insert(got[0]);
    }
    assert!(firsts.len() > 1, "{firsts:?}");
}

/// 1.10 (§16, §20.9 rule 13; it4 l4 F1): a stale offer's tidy item asks its owners whether it is
/// still wanted, by a question for the user whose chain reaches them (§8.4), never a take to
/// refresh it; `leave_better`'s first node gets the same item. While the question is pending, no
/// item comes for the node; once its owner answers, or its default applies, the item is a take,
/// with that answer as the news it acts on.
#[test]
fn a_stale_offer_s_owners_are_asked_and_their_answer_is_acted_on() {
    use crate::helpers::user::tests::{from_user_cmd, open_as, person, pid, set_owners};
    use crate::query::guide::{Because, End, Rule, Skeleton};
    let mut w = crate::apply::tests::world();
    let c = class("worker");
    let s = w.hello(&c);
    let maria = person(&mut w, &c, s, "maria", &["mail:maria@x"]);
    let web = node_with(&mut w, &c, s, "/web", |_| {});
    set_owners(&mut w, &c, s, web, &[maria]);
    let old = node_with(
        &mut w,
        &c,
        s,
        "/web/legacy-ie11",
        web_offer("Keep the checkout working on IE 11"),
    );
    let other = node_with(
        &mut w,
        &c,
        s,
        "/web/tablet-layout",
        web_offer("Tablet layout for the product screen"),
    );
    let stale = w.s.params.offer_stale.unwrap();
    w.advance(Dur(stale.0 + DAY.0));
    assert!(w.s.ix.node[&old].flags.contains(Flag::Stale));
    // A worker that closes a node under /web: leave_better names the stale offer first, and the
    // item asks its owners.
    let x = w.hello(&c);
    let d = ok(w.call(
        &c,
        x,
        create_with("/web/fix", |a| a.title = text("Fix the footer")),
    ));
    let t = created(&d).1.unwrap();
    let d = ok(w.call(
        &c,
        x,
        Cmd::Node(NodeCmd::Close {
            at: id(created(&d).0),
            outcome: Outcome::Done,
            summary: text("done"),
            verified: None,
            settles: None,
            learnings: None,
            token: Some(t),
            unsent: BTreeSet::new(),
        }),
    ));
    assert!(
        matches!(&d.reply, Reply::Closed { leave_better, .. } if leave_better.first() == Some(&old))
    );
    let g = w.last_guidance().clone();
    assert_eq!(g.items[0].call, Skeleton::AskOwners { at: old });
    assert_eq!(g.items[0].rule, Rule::Tidy);
    assert_eq!(
        g.items[0].because,
        Because::StaleOffer {
            node: old,
            cut: false
        }
    );
    // An idle worker at `hello`: both stale offers, asked of their owners.
    let y = w.hello(&c);
    assert_eq!(
        calls(&w),
        vec![
            Skeleton::AskOwners { at: old },
            Skeleton::AskOwners { at: other }
        ]
    );
    // It asks about the first: the question goes to Maria first, the owner of /web.
    let d = ok(w.call(&c, y, ask_owners(old)));
    let Reply::Asked { question: q, .. } = d.reply else {
        panic!("{:?}", d.reply)
    };
    assert_eq!(
        w.s.uquestions[&q].to.first().copied(),
        Some(pid(&w, "mail:maria@x"))
    );
    // Pending: no item for it, the other one only.
    let _ = w.hello(&c);
    assert_eq!(calls(&w), vec![Skeleton::AskOwners { at: other }]);
    // Maria answers through her assistant: the item becomes a take, with the answer as its news.
    let asst = class("assistant");
    let h = open_as(&mut w, &asst, Some("mail:maria@x"));
    ok(w.call(
        &asst,
        h,
        from_user_cmd("drop it", Some("mail:maria@x"), Some(q), None, None),
    ));
    let z = w.hello(&c);
    let g = w.last_guidance().clone();
    assert_eq!(g.items[0].call, Skeleton::Take { target: old });
    assert_eq!(
        g.items[0].because,
        Because::StaleAnswered {
            node: old,
            question: q,
            cut: false
        }
    );
    // The taker closes it as dropped.
    let t = token_of(&ok(w.call(&c, z, take(old))));
    ok(w.call(
        &c,
        z,
        Cmd::Node(NodeCmd::Close {
            at: id(old),
            outcome: Outcome::Dropped,
            summary: text("Maria: no longer needed"),
            verified: None,
            settles: None,
            learnings: None,
            token: Some(t),
            unsent: BTreeSet::new(),
        }),
    ));
    // The other one: nobody answers, and the default applies at its due; then the take.
    let d = ok(w.call(&c, z, ask_owners(other)));
    let Reply::Asked { question: q2, .. } = d.reply else {
        panic!("{:?}", d.reply)
    };
    let _ = w.hello(&c);
    assert!(calls(&w).is_empty());
    assert_eq!(w.last_guidance().end, Some(End::AtHello));
    w.advance(Dur(w.s.uquestions[&q2].due.0 - w.clock.0 + 1));
    assert_eq!(w.s.uquestions[&q2].state, crate::state::UqState::Defaulted);
    let _ = w.hello(&c);
    assert_eq!(calls(&w), vec![Skeleton::Take { target: other }]);
}

/// 1.10 (§16; it4 l7 F2): two tasks one session created, a lead's split of an epic that share
/// `token` and `theme`, are its plan, not possible duplicates; the same words in a node another
/// session created are a possible duplicate.
#[test]
fn one_session_s_planned_tasks_are_not_possible_duplicates() {
    use crate::query::guide::Skeleton;
    let mut w = crate::apply::tests::world();
    let c = class("worker");
    let lead = w.hello(&c);
    node_with(&mut w, &c, lead, "/portal/v2-migration", |a| {
        a.capacity = Some(0);
    });
    let tokens = node_with(
        &mut w,
        &c,
        lead,
        "/portal/v2-migration/tokens",
        web_offer("Adopt the v2 colour and spacing tokens in theme.ts"),
    );
    let dark = node_with(
        &mut w,
        &c,
        lead,
        "/portal/v2-migration/dark",
        web_offer("Dark-mode variables for the v2 tokens in theme.ts"),
    );
    assert!(crate::query::leave_better::same_creator(&w.s, tokens, dark));
    let _ = w.hello(&c);
    assert!(
        calls(&w)
            .iter()
            .all(|k| !matches!(k, Skeleton::LinkDuplicate { .. })),
        "{:?}",
        w.last_guidance()
    );
    // Another session's node with the same words is paired with both.
    let other = w.hello(&c);
    let again = node_with(
        &mut w,
        &c,
        other,
        "/portal/v2-migration/dark-mode",
        web_offer("Dark-mode variables for the v2 tokens in theme.ts"),
    );
    let _ = w.hello(&c);
    assert!(
        calls(&w)
            .iter()
            .any(|k| matches!(k, Skeleton::LinkDuplicate { from, .. } if *from == again)),
        "{:?}",
        w.last_guidance()
    );
}

/// A session of class `c` with `caps`, opened by `hello`.
fn with_caps(w: &mut crate::testkit::World, c: &ClassName, caps: &[&str]) -> SessionId {
    let ctx = w.ctx(c, SessionCtx::New, None);
    ok(w.call_ctx(
        &ctx,
        &Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps: Some(caps.iter().map(|x| label(x)).collect()),
            horizon: None,
            principal: None,
            from_user: None,
        }),
    ))
    .opened
    .unwrap()
}

/// The first `k` candidates `next` gives `x`, from the index, which equal the oracle's.
fn firsts(w: &crate::testkit::World, x: SessionId, k: usize) -> Vec<Cand> {
    let (got, _) = crate::query::ranked(&w.s, x, w.clock, &NextQuery::default(), k, &mut NoMeter);
    let got: Vec<Cand> = got.into_iter().map(|r| r.cand).collect();
    let want: Vec<Cand> = crate::spec::ranked(&w.s, x, w.clock, &NextQuery::default())
        .into_iter()
        .take(k)
        .collect();
    assert_eq!(got, want);
    got
}

/// 1.11 (§10.4; it5 l5 F4, l7 F1): a review item's tier is its node's without `objection`, and
/// within a tier the work that needs more of the caller's caps comes first: the prover gets its
/// Lean lemma, the numerics worker its lemma whose job's recheck is due, and the reviewer, whose
/// caps no node needs, the review; a node that needs a11y and web comes before one that needs web
/// alone for a session with both; and the review of a node that needs lean comes first for the
/// prover, before its Lean lemma, and names its needs.
#[test]
fn work_that_fits_the_caps_comes_before_reviews_that_need_nothing() {
    let mut w = crate::apply::tests::world();
    let c = class("worker");
    let s = w.hello(&c);
    let review = node_with(&mut w, &c, s, "/claims/lemma-04", |a| {
        a.kind = Some(label(CLAIM));
    });
    ok(w.call(&c, s, close_cmd(review, Outcome::Done)));
    ok(w.call(&c, s, object(review)));
    let lean = node_with(&mut w, &c, s, "/claims/lemma-01", |a| {
        a.tags = [label(OFFER)].into();
        a.needs = Some([label("lean")].into());
    });
    let numerics = node_with(&mut w, &c, s, "/claims/lemma-05", |a| {
        a.tags = [label(OFFER)].into();
        a.needs = Some([label("numerics")].into());
    });
    let plain = node_with(&mut w, &c, s, "/claims/lemma-07", |a| {
        a.tags = [label(OFFER)].into();
    });
    let prover = with_caps(&mut w, &c, &["lean"]);
    let numeric = with_caps(&mut w, &c, &["numerics", "python"]);
    let reviewer = with_caps(&mut w, &c, &["review"]);
    // The review is tier 2, beside the offers.
    let (got, _) = crate::query::ranked(
        &w.s,
        reviewer,
        w.clock,
        &NextQuery::default(),
        3,
        &mut NoMeter,
    );
    assert_eq!(got[0].cand, Cand::Status(review));
    assert_eq!(got[0].why.tier, 2);
    assert_eq!(got[0].why.reason, crate::reply::Reason::Review);
    assert_eq!(
        firsts(&w, reviewer, 2),
        vec![Cand::Status(review), Cand::Node(plain)]
    );
    assert_eq!(
        firsts(&w, prover, 2),
        vec![Cand::Node(lean), Cand::Status(review)]
    );
    assert_eq!(
        firsts(&w, numeric, 2),
        vec![Cand::Node(numerics), Cand::Status(review)]
    );
    // A review of a node that needs lean comes first for the prover, and none for the others.
    let lean_review = node_with(&mut w, &c, s, "/claims/lemma-02", |a| {
        a.kind = Some(label(CLAIM));
        a.needs = Some([label("lean")].into());
    });
    ok(w.call(&c, s, close_cmd(lean_review, Outcome::Done)));
    ok(w.call(&c, s, object(lean_review)));
    assert_eq!(
        firsts(&w, prover, 3),
        vec![
            Cand::Status(lean_review),
            Cand::Node(lean),
            Cand::Status(review)
        ]
    );
    assert!(!firsts(&w, reviewer, 5).contains(&Cand::Status(lean_review)));
    // a11y and web before web alone, for a session with both.
    let web = node_with(&mut w, &c, s, "/portal/table", |a| {
        a.tags = [label(OFFER)].into();
        a.needs = Some([label("web")].into());
    });
    let a11y = node_with(&mut w, &c, s, "/portal/a11y-focus", |a| {
        a.tags = [label(OFFER)].into();
        a.needs = Some([label("a11y"), label("web")].into());
    });
    let both = with_caps(&mut w, &c, &["a11y", "web"]);
    assert_eq!(firsts(&w, both, 2), vec![Cand::Node(a11y), Cand::Node(web)]);
}

/// 1.11 (§10.4; it5 l2 F4): within a tier, a candidate flagged `unanswered_request` comes before
/// one without: an open intent left by a release and a request nobody is at work on are both tier
/// 0, and the request comes first whatever the ring and the ages.
#[test]
fn a_request_comes_first_within_its_tier() {
    let mut w = crate::apply::tests::world();
    let c = class("worker");
    let s = w.hello(&c);
    let left = node_with(&mut w, &c, s, "/left", |a| {
        a.tags = [label(OFFER)].into();
    });
    let t = token_of(&ok(w.call(&c, s, take(left))));
    ok(w.call(
        &c,
        s,
        Cmd::Hold(HoldCmd::Checkpoint {
            token: t,
            done: text("built it"),
            next_step: text("deploy"),
            intent: Some(text("deploy v2")),
            intent_done: None,
            files: Vec::new(),
        }),
    ));
    let _ = w.call(&c, s, release(t, ReleaseReason::Handoff));
    w.advance(Dur(3 * HOUR.0));
    let asked = node(&mut w, &c, s, "/asked", &["chat:x/1"]);
    ok(w.call(
        &c,
        s,
        super::talk_people::tests::request_cmd(asked, "chat:x/1/1", Some("chat:x/1")),
    ));
    let x = w.hello(&c);
    assert!(w.s.ix.node[&left].flags.contains(Flag::OpenIntent));
    assert!(w.s.ix.node[&asked].flags.contains(Flag::UnansweredRequest));
    let got = firsts(&w, x, 2);
    assert_eq!(got, vec![Cand::Node(asked), Cand::Node(left)]);
}

/// 1.11 (§14, S1; it5 l4 F2): a duplicate signal resolves, and its node closes as done, when a close
/// leaves both nodes of its pair closed, neither merged; a conflict signal stays open.
#[test]
fn a_duplicate_signal_resolves_when_both_its_nodes_close() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &[]);
    let b = node(&mut w, &c, s, "/b", &[]);
    let d = ok(w.call(&c, s, super::keys::tests::link(b, LinkRel::Duplicate, a)));
    let Reply::Linked {
        signal: Some(sig), ..
    } = d.reply
    else {
        panic!("a signal: {:?}", d.reply)
    };
    let x = node(&mut w, &c, s, "/x", &[]);
    let y = node(&mut w, &c, s, "/y", &[]);
    let d = ok(w.call(
        &c,
        s,
        super::keys::tests::link(y, LinkRel::ConflictsWith, x),
    ));
    let Reply::Linked {
        signal: Some(con), ..
    } = d.reply
    else {
        panic!("a signal: {:?}", d.reply)
    };
    ok(w.call(&c, s, close_cmd(a, Outcome::Done)));
    assert!(w.s.nodes[&sig].is_open());
    ok(w.call(&c, s, close_cmd(b, Outcome::Dropped)));
    assert!(!w.s.nodes[&sig].is_open());
    assert!(!w.s.signals.values().any(|n| *n == sig));
    ok(w.call(&c, s, close_cmd(x, Outcome::Done)));
    ok(w.call(&c, s, close_cmd(y, Outcome::Done)));
    assert!(w.s.nodes[&con].is_open());
}

const T0: Time = Time(1_767_225_600_000);
