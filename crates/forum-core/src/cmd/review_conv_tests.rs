//! 1.11's reviews and conversations (design D.8.7, D.8.15, D.8.16, D.8.17): a review that cannot be
//! decided yet is deferred, which backs the caller's class off it as a `cannot` release backs a
//! class off a node, until the backoff ends or a verdict comes (§10.3, §11.3); a node keeps its
//! newest retracted decision, so `no_basis` names it after it leaves memory (§11.3, §24); the
//! conversation's voice is a session with the cap its scheme names (§9.1, §9.2); a `said` by a
//! session that holds nothing is taken as sent at once (§9.2); and a note without `to` at a closed
//! node reaches the session that closed it, while it is live (§13). Every call runs through
//! `World`, with every check of a step, of a firing, and of the state, and G1 on its next calls.

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

use super::keys::tests::{close_cmd, evs, id, key, node, one};
use crate::apply::tests::{create_with, created, label, next, ok, release, take, text};
use crate::cmd::*;
use crate::error::{Hint, Why};
use crate::event::{ConfirmVia, EntryBody, Ev};
use crate::query::guide::{Rule, Skeleton};
use crate::reply::{Deferred, NextReply, Reply, Verdict, Warning, WarningCode, WarningDetail};
use crate::state::{Life, ObjectionState, SaidState};
use crate::testkit::World;
use crate::types::*;

// ------------------------------------------------------------------------------------------------
// Commands and readings

fn object(n: NodeId) -> Cmd {
    Cmd::Talk(TalkCmd::Object {
        on: id(n),
        note: text("nothing certifies IsExtremal"),
    })
}

fn resolve(o: Eid, upheld: bool) -> Cmd {
    Cmd::Talk(TalkCmd::Resolve {
        objection: o,
        note: text("checked the run"),
        upheld,
        defer: false,
    })
}

fn defer(o: Eid) -> Cmd {
    Cmd::Talk(TalkCmd::Resolve {
        objection: o,
        note: text("sunflower-free, but nothing certifies extremality yet"),
        upheld: false,
        defer: true,
    })
}

fn objected(d: &crate::reply::Done) -> Eid {
    match d.reply {
        Reply::Objected { objection } => objection,
        ref other => panic!("not objected: {other:?}"),
    }
}

fn class_of(name: &str) -> ClassName {
    ClassName::new(name).unwrap()
}

/// A claim at `p`, made without a hold.
fn claim(w: &mut World, c: &ClassName, s: SessionId, p: &str) -> NodeId {
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

/// Opens a session of `class` with `caps`.
fn hello_caps(w: &mut World, c: &ClassName, caps: &[&str]) -> SessionId {
    let ctx = w.ctx(c, SessionCtx::New, None);
    let d = w.call_ctx(
        &ctx,
        &Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps: Some(caps.iter().map(|x| label(x)).collect()),
            horizon: None,
            principal: None,
            from_user: None,
        }),
    );
    ok(d).opened.unwrap()
}

fn warned(d: &crate::reply::Done, code: WarningCode) -> Vec<Warning> {
    d.warnings
        .iter()
        .filter(|x| x.code == code)
        .cloned()
        .collect()
}

/// The review `next` gives, if it gives one.
fn review_of(d: &crate::reply::Done) -> Option<(NodeId, Eid)> {
    match d.reply {
        Reply::Next(NextReply::Review {
            node, objection, ..
        }) => Some((node, objection)),
        _ => None,
    }
}

fn calls(w: &World) -> Vec<Skeleton> {
    w.last_guidance()
        .items
        .iter()
        .map(|i| i.call.clone())
        .collect()
}

fn rules(w: &World) -> Vec<Rule> {
    w.last_guidance().items.iter().map(|i| i.rule).collect()
}

fn said_of(d: &crate::reply::Done) -> Eid {
    match d.reply {
        Reply::Said { said, .. } => said,
        ref other => panic!("not said: {other:?}"),
    }
}

fn said_cmd(n: NodeId, kind: SaidKind) -> Cmd {
    Cmd::Talk(TalkCmd::Said {
        at: id(n),
        where_: None,
        text: text("we're on it"),
        kind,
        corrects: None,
        seen: None,
    })
}

fn say(n: NodeId) -> Cmd {
    Cmd::Talk(TalkCmd::Say {
        at: id(n),
        text: text("please share any overlapping evidence"),
        to: None,
        expects: None,
        reply_to: None,
        until: None,
    })
}

fn said_voice(w: &World, y: Eid) -> Option<bool> {
    evs(w).iter().find_map(|e| match e {
        Ev::Appended {
            eid,
            body: EntryBody::Said { voice, .. },
            ..
        } if *eid == y => Some(*voice),
        _ => None,
    })
}

// ------------------------------------------------------------------------------------------------
// C1: a review that cannot be decided yet (§10.3, §11.3)

/// l5 of the lab's sixth round: every reviewer found that the family is sunflower-free and that
/// nothing certifies extremality, and `next` kept giving the review back, to the reviewer that had
/// just declined it too. A deferral records why, leaves the objection open and flagged, and backs
/// the caller's class off the review: `next` gives it to that class no more, and to another class
/// still, until the backoff ends.
#[test]
fn a_deferral_backs_the_class_off_the_review_until_its_backoff_ends() {
    let (mut w, c, s) = one();
    let lemma = claim(&mut w, &c, s, "/claims/lemma-04");
    ok(w.call(&c, s, close_cmd(lemma, Outcome::Done)));
    let o = objected(&ok(w.call(&c, s, object(lemma))));
    let a = w.hello(&c);
    assert_eq!(review_of(&ok(w.call(&c, a, next()))), Some((lemma, o)));
    // A defers: an entry that decides nothing, and the class's refusal of the node.
    let now = w.clock;
    let d = ok(w.call(&c, a, defer(o)));
    let until = now.plus(w.s.params.refusal_backoff);
    let Reply::ObjectionResolved {
        entry: Some(e),
        corrected: None,
        already: None,
        disputed: None,
        withdrawn: None,
        deferred: Some(Deferred { until: u }),
    } = d.reply
    else {
        panic!("{:?}", d.reply)
    };
    assert_eq!(u, Some(until));
    assert_eq!(w.s.objections[&o].state, ObjectionState::Open);
    assert_eq!(w.s.objections[&o].deferred, Some(e));
    assert!(w.s.ix.node[&lemma].flags.contains(Flag::Objection));
    assert_eq!(w.s.entries[&e].ty, EntryType::Resolve);
    assert!(evs(&w).iter().any(|x| matches!(
        x,
        Ev::Refused { node, session, backoff_until, .. }
            if *node == lemma && *session == a && *backoff_until == until
    )));
    // Neither A nor another session of its class is given the review now.
    let d = ok(w.call(&c, a, next()));
    assert_eq!(review_of(&d), None, "{:?}", d.reply);
    let b = w.hello(&c);
    assert_eq!(review_of(&ok(w.call(&c, b, next()))), None);
    // Another class is.
    let other = class_of("prover");
    let p = w.hello(&other);
    assert_eq!(review_of(&ok(w.call(&other, p, next()))), Some((lemma, o)));
    // Once the backoff has ended, A's class is given it again.
    w.advance(w.s.params.refusal_backoff);
    let b = w.hello(&c);
    assert_eq!(review_of(&ok(w.call(&c, b, next()))), Some((lemma, o)));
}

/// A verdict ends the backoff (§10.3, §11.3): a resolve on the closed node clears its refusals, so
/// a second verdict's new objection reaches the class that deferred the first.
#[test]
fn a_verdict_clears_the_deferrals_so_a_new_objection_reaches_every_class() {
    let (mut w, c, s) = one();
    let lemma = claim(&mut w, &c, s, "/claims/lemma-25");
    ok(w.call(&c, s, close_cmd(lemma, Outcome::Done)));
    let o = objected(&ok(w.call(&c, s, object(lemma))));
    let a = w.hello(&c);
    ok(w.call(&c, a, defer(o)));
    assert!(w.s.refusals.contains_key(&(lemma, c.clone())));
    // A prover resolves it: the objection does not stand, and the refusals go.
    let other = class_of("prover");
    let p = w.hello(&other);
    let d = ok(w.call(&other, p, resolve(o, false)));
    let Reply::ObjectionResolved { entry: Some(v), .. } = d.reply else {
        panic!("{:?}", d.reply)
    };
    assert!(evs(&w).iter().any(|x| matches!(
        x,
        Ev::RefusalsCleared { node, by } if *node == lemma && *by == v
    )));
    assert!(!w.s.refusals.keys().any(|(n, _)| *n == lemma));
    // B disputes it: a new objection, which A's class is given at once.
    let b = w.hello(&c);
    let d = ok(w.call(&c, b, resolve(o, true)));
    let Reply::ObjectionResolved {
        disputed: Some(x), ..
    } = d.reply
    else {
        panic!("{:?}", d.reply)
    };
    assert_eq!(
        review_of(&ok(w.call(&c, a, next()))),
        Some((lemma, x.objection))
    );
}

/// A deferral's other cases: of an objection already resolved it records nothing and names the
/// verdict; with `upheld` it is `invalid`; on an open node it records the deferral and backs
/// nobody off, since the node's holder decides the objection with its work (rule 10).
#[test]
fn a_deferral_of_a_resolved_or_an_open_node_s_objection() {
    let (mut w, c, s) = one();
    let lemma = claim(&mut w, &c, s, "/claims/lemma-23");
    ok(w.call(&c, s, close_cmd(lemma, Outcome::Done)));
    let o = objected(&ok(w.call(&c, s, object(lemma))));
    let d = ok(w.call(&c, s, resolve(o, false)));
    let Reply::ObjectionResolved { entry: Some(v), .. } = d.reply else {
        panic!("{:?}", d.reply)
    };
    let a = w.hello(&c);
    let before = w.s.counters.next_eid;
    let d = ok(w.call(&c, a, defer(o)));
    assert_eq!(
        d.reply,
        Reply::ObjectionResolved {
            entry: None,
            corrected: None,
            already: Some(Verdict {
                verdict: v,
                upheld: false,
            }),
            disputed: None,
            withdrawn: None,
            deferred: None,
        }
    );
    assert_eq!(w.s.counters.next_eid, before);
    // `defer` with `upheld` would decide nothing and something at once.
    let mut both = defer(o);
    if let Cmd::Talk(TalkCmd::Resolve { upheld, .. }) = &mut both {
        *upheld = true;
    }
    let e = w.call(&c, a, both).unwrap_err();
    assert_eq!(e.hint, Hint::Invalid(Why::BadParams));
    // On an open node: recorded, the objection marked, and no refusal.
    let open = node(&mut w, &c, s, "/claims/lemma-31", &[]);
    let o2 = objected(&ok(w.call(&c, s, object(open))));
    let d = ok(w.call(&c, a, defer(o2)));
    let Reply::ObjectionResolved {
        entry: Some(e2),
        deferred: Some(Deferred { until: None }),
        ..
    } = d.reply
    else {
        panic!("{:?}", d.reply)
    };
    assert_eq!(w.s.objections[&o2].deferred, Some(e2));
    assert_eq!(w.s.objections[&o2].state, ObjectionState::Open);
    assert!(!w.s.refusals.contains_key(&(open, c.clone())));
}

/// `leave_better` and rule 13 (§10.3, §20.9): after a deferral, the release of a sibling does not
/// give its class the deferred review back, though `leave_better` lists flagged nodes whatever the
/// caller's refusals; another class's release still lists it, with the review as its next call.
#[test]
fn leave_better_leaves_out_a_review_the_class_deferred() {
    let (mut w, c, s) = one();
    let lemma = claim(&mut w, &c, s, "/claims/lemma-04");
    ok(w.call(&c, s, close_cmd(lemma, Outcome::Done)));
    let o = objected(&ok(w.call(&c, s, object(lemma))));
    let a = w.hello(&c);
    ok(w.call(&c, a, defer(o)));
    let d = ok(w.call(
        &c,
        a,
        create_with("/claims/lemma-07", |x| x.title = text("Replace the sorry")),
    ));
    let t = created(&d).1.unwrap();
    let d = ok(w.call(&c, a, release(t, ReleaseReason::Handoff)));
    let Reply::Released { leave_better, .. } = d.reply else {
        panic!("{:?}", d.reply)
    };
    assert!(!leave_better.contains(&lemma), "{leave_better:?}");
    assert!(
        !calls(&w).contains(&Skeleton::Resolve { objection: o }),
        "{:?}",
        calls(&w)
    );
    // Another class's release lists it, and its next calls give the review.
    let other = class_of("prover");
    let p = w.hello(&other);
    let d = ok(w.call(
        &other,
        p,
        create_with("/claims/lemma-08", |x| x.title = text("Another lemma")),
    ));
    let t = created(&d).1.unwrap();
    let d = ok(w.call(&other, p, release(t, ReleaseReason::Handoff)));
    let Reply::Released { leave_better, .. } = d.reply else {
        panic!("{:?}", d.reply)
    };
    assert!(leave_better.contains(&lemma), "{leave_better:?}");
    assert!(calls(&w).contains(&Skeleton::Resolve { objection: o }));
}

// ------------------------------------------------------------------------------------------------
// C2: a retracted decision remembered (§11.3, §24)

/// l5 of the lab's sixth round: 1.10's `no_basis` never fired, because the claims' retracted
/// decisions, 9 to 31 days old, had left memory. The node keeps its newest retracted decision.
#[test]
fn no_basis_names_a_retracted_decision_that_left_memory() {
    let (mut w, c, s) = one();
    let lemma = claim(&mut w, &c, s, "/claims/lemma-25");
    let d = ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Decide {
            at: id(lemma),
            text: text("the exhaustive check holds"),
            settles: BTreeSet::new(),
            supersedes: None,
            basis: Vec::new(),
            evidence: None,
        }),
    ));
    let Reply::Decided { decision, .. } = d.reply else {
        panic!("{:?}", d.reply)
    };
    ok(w.call(&c, s, close_cmd(lemma, Outcome::Done)));
    ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Retract {
            target: RetractArg::Entry {
                eid: decision,
                node: lemma,
            },
            text: text("the check was over a restricted link"),
            reason: text("refuted by the full run"),
        }),
    ));
    assert_eq!(w.s.nodes[&lemma].retracted_decision, Some(decision));
    assert!(evs(&w).iter().any(|e| matches!(
        e,
        Ev::DecisionRetracted { d, node: Some(n), .. } if *d == decision && *n == lemma
    )));
    // Weeks pass: the retracted decision leaves memory at the first event after its time.
    w.advance(Dur(crate::state::FINISHED_RETAIN.0 * 3));
    let s = w.hello(&c);
    assert!(!w.s.decisions.contains_key(&decision));
    assert_eq!(crate::query::no_basis(&w.s, lemma), Some(decision));
    // A resolve that keeps the claim warns.
    let o = objected(&ok(w.call(&c, s, object(lemma))));
    let d = ok(w.call(&c, s, resolve(o, false)));
    assert_eq!(
        warned(&d, WarningCode::NoBasis).first().map(|x| &x.detail),
        Some(&WarningDetail::NoBasis {
            node: lemma,
            decision
        })
    );
}

// ------------------------------------------------------------------------------------------------
// C3: the conversation's voice by its cap (§9.1, §9.2)

/// l6 of the lab's sixth round: the incident's holder had no `chat`, and the chat worker's replies
/// drew `conversation_held` and a `say` to the holder, who could not post. A caller with the cap
/// that the scheme names, while no holder has it, is the conversation's voice: no warning, no
/// `say`, and its entry is marked; a holder with the cap is the voice, which the warning names.
#[test]
fn the_voice_is_a_session_with_the_cap_of_the_conversation_s_scheme() {
    let c = class_of("worker");
    let mut w = crate::apply::tests::world();
    let lead = hello_caps(&mut w, &c, &["k8s", "repo"]);
    let chat = hello_caps(&mut w, &c, &["chat"]);
    let d = ok(w.call(
        &c,
        lead,
        create_with("/incidents/inc-311", |a| {
            a.keys = vec![key("chat:incidents/T7001")];
        }),
    ));
    let inc = created(&d).0;
    let d = ok(w.call(&c, chat, said_cmd(inc, SaidKind::Reply)));
    let y = said_of(&d);
    assert!(
        warned(&d, WarningCode::ConversationHeld).is_empty(),
        "{d:?}"
    );
    assert!(!rules(&w).contains(&Rule::TellHolder), "{:?}", calls(&w));
    assert_eq!(said_voice(&w, y), Some(true));
    assert_eq!(
        crate::query::voice(&w.s, inc, chat, &w.s.saids[&y].conv),
        crate::query::Voice::Caller
    );
    // A session without the cap is told about the holder, as before, and its entry is not marked.
    let ide = hello_caps(&mut w, &c, &["swift"]);
    let d = ok(w.call(&c, ide, said_cmd(inc, SaidKind::Ack)));
    let z = said_of(&d);
    assert_eq!(
        warned(&d, WarningCode::ConversationHeld)
            .first()
            .map(|x| &x.detail),
        Some(&WarningDetail::ConversationHeld {
            holders: vec![lead]
        })
    );
    assert!(calls(&w).contains(&Skeleton::SayTo { at: inc, to: lead }));
    assert_eq!(said_voice(&w, z), Some(false));
    // A holder of work inside it with the cap speaks, though the lead's hold comes first.
    let mate = hello_caps(&mut w, &c, &["chat"]);
    let d = ok(w.call(
        &c,
        lead,
        create_with("/incidents/inc-311/comms", |a| a.take = Some(false)),
    ));
    let comms = created(&d).0;
    ok(w.call(&c, mate, take(comms)));
    for x in [ide, chat] {
        let d = ok(w.call(&c, x, said_cmd(inc, SaidKind::Ack)));
        assert_eq!(
            warned(&d, WarningCode::ConversationHeld)
                .first()
                .map(|w| &w.detail),
            Some(&WarningDetail::ConversationHeld {
                holders: vec![mate]
            })
        );
        assert!(calls(&w).contains(&Skeleton::SayTo { at: inc, to: mate }));
    }
}

// ------------------------------------------------------------------------------------------------
// C4: a said by a session that holds nothing (§9.2)

/// l6 of the lab's sixth round: the chat worker held nothing, so no release or close of its came,
/// and its two posted replies stayed pending in the holder's brief. Such a `said` is taken as sent
/// at once, with its reply; silence leaves it so; the holder's brief shows it as said, not as
/// unfinished. An `ask`'s `said` too.
#[test]
fn a_said_by_a_session_that_holds_nothing_is_sent_at_once() {
    let (mut w, c, s) = one();
    let inc = node(
        &mut w,
        &c,
        s,
        "/incidents/inc-311",
        &["chat:incidents/T7001"],
    );
    let lead = w.hello(&c);
    ok(w.call(&c, lead, take(inc)));
    let b = w.hello(&c);
    let d = ok(w.call(&c, b, said_cmd(inc, SaidKind::Reply)));
    let y = said_of(&d);
    assert_eq!(w.s.saids[&y].state, SaidState::Confirmed);
    assert!(w.s.saids[&y].with_reply);
    assert!(evs(&w).iter().any(|e| matches!(
        e,
        Ev::SaidConfirmed { said, via: ConfirmVia::Recorded, link: None } if *said == y
    )));
    // Its author falls silent: it stays sent.
    w.advance(Dur(w.s.params.presence_silence.0 + MINUTE.0));
    assert_eq!(w.s.saids[&y].state, SaidState::Confirmed);
    // The holder's brief does not list it as unfinished.
    let brief = crate::query::brief(&w.s, inc, None, w.clock, &mut crate::query::NoMeter);
    assert!(brief.sections.iter().all(|x| {
        x.name != crate::query::SectionName::Unfinished
            || !x.items.contains(&crate::query::BriefItem::Said(y))
    }));
    // An `ask` to the person by a session that holds nothing: its said is sent at once too.
    let q = w.hello(&c);
    let d = ok(w.call(
        &c,
        q,
        Cmd::Talk(TalkCmd::Ask {
            at: id(inc),
            text: text("which region?"),
            to: AskTo::Person {
                where_: None,
                blocking: false,
            },
            again: false,
        }),
    ));
    let Reply::Asked { said: Some(x), .. } = d.reply else {
        panic!("{:?}", d.reply)
    };
    assert_eq!(w.s.saids[&x].state, SaidState::Confirmed);
    assert!(w.s.saids[&x].with_reply);
}

// ------------------------------------------------------------------------------------------------
// C5: a note at a closed node (§13)

/// l5 of the lab's sixth round: 10 of 12 notes were at closed nodes and reported "the next holder",
/// which never comes; one, meant for the reviewer that had closed the node 4.5 s before, reached
/// nobody. A note without `to` at a closed node reaches the session that closed it while it is
/// live, and nobody once it is silent, nor the closer itself; the closure keeps who closed it.
#[test]
fn a_note_at_a_closed_node_reaches_its_closer_while_it_is_live() {
    let (mut w, c, s) = one();
    let a = w.hello(&c);
    let d = ok(w.call(&c, a, create_with("/reviews/lemma-04", |_| {})));
    let r = created(&d).0;
    let t = created(&d).1.unwrap();
    let mut close = close_cmd(r, Outcome::Done);
    if let Cmd::Node(NodeCmd::Close { token, .. }) = &mut close {
        *token = Some(t);
    }
    ok(w.call(&c, a, close));
    match &w.s.nodes[&r].life {
        Life::Closed(x) => assert_eq!(x.by, Some(a)),
        Life::Open => panic!("open"),
    }
    let d = ok(w.call(&c, s, say(r)));
    let Reply::Say { entry } = d.reply else {
        panic!("{:?}", d.reply)
    };
    let note = &w.s.notes[&entry];
    assert_eq!(note.to, Addressee::Session(a));
    assert_eq!(note.recipients, BTreeSet::from([a]));
    assert!(!note.pending_holder);
    // The closer's own note there reaches nobody.
    let d = ok(w.call(&c, a, say(r)));
    let Reply::Say { entry } = d.reply else {
        panic!("{:?}", d.reply)
    };
    assert!(!w.s.notes.contains_key(&entry));
    // Once the closer is silent, a note reaches nobody.
    w.advance(Dur(w.s.params.presence_silence.0 + MINUTE.0));
    assert!(w.s.sessions[&a].silent);
    let s = w.hello(&c);
    let d = ok(w.call(&c, s, say(r)));
    let Reply::Say { entry } = d.reply else {
        panic!("{:?}", d.reply)
    };
    assert!(!w.s.notes.contains_key(&entry));
}
