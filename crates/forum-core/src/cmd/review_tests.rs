//! Unit tests of WP-D5b's review commands (design D.12.1, the research side: CS-22, CS-32, D-4):
//! `talk decide` with its basis references, `approve`, `object`, `resolve`, and `retract`, and the
//! one-hop `basis_changed` they and closes drive (D.8.17, §11, §15). Every call runs through
//! `World::call_ctx` with every check of a step and of the state.

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

use super::super::keys::tests::{close_cmd, evs, id, inv, link, nf, node, one, request};
use super::super::restructure::tests::merge;
use super::super::talk_people::tests::{decide_cmd, request_cmd, said_cmd};
use crate::apply::tests::{entry, ok, refused, take, text};
use crate::cmd::*;
use crate::error::{What, Why};
use crate::event::{EntryBody, Ev};
use crate::query::{self, NoMeter};
use crate::reply::{NextReply, NotSettled, NotSettledWhy, Reply, WarningCode, WarningDetail};
use crate::state::{Author, BasisRef, DecisionState, ObjectionState, PqState};
use crate::testkit::World;
use crate::types::*;

// ------------------------------------------------------------------------------------------------
// Commands

fn decide_with(n: NodeId, f: impl FnOnce(&mut Vec<BasisArg>, &mut Option<Eid>)) -> Cmd {
    let mut basis = Vec::new();
    let mut supersedes = None;
    f(&mut basis, &mut supersedes);
    Cmd::Talk(TalkCmd::Decide {
        at: id(n),
        text: text("go on by the lemma"),
        settles: BTreeSet::new(),
        supersedes,
        basis,
        evidence: Some(Evidence::Inference),
    })
}

fn node_ref(n: NodeId, version: Option<u64>) -> BasisArg {
    BasisArg::Node {
        at: id(n),
        version: version.map(Version),
    }
}

fn decided(d: &crate::reply::Done) -> (Eid, Vec<Eid>, Vec<NotSettled>) {
    match &d.reply {
        Reply::Decided {
            decision,
            settled,
            not_settled,
        } => (*decision, settled.clone(), not_settled.clone()),
        other => panic!("not decided: {other:?}"),
    }
}

fn retract_entry(e: Eid, node: NodeId) -> Cmd {
    Cmd::Talk(TalkCmd::Retract {
        target: RetractArg::Entry { eid: e, node },
        text: text("the lemma is false"),
        reason: text("a counterexample at n = 7"),
    })
}

fn retract_node(n: NodeId) -> Cmd {
    Cmd::Talk(TalkCmd::Retract {
        target: RetractArg::Node(id(n)),
        text: text("the lemma is false"),
        reason: text("a counterexample at n = 7"),
    })
}

fn flagged(w: &World, n: NodeId) -> bool {
    w.s.ix.node[&n].flags.contains(Flag::BasisChanged)
}

fn update_title(w: &World, n: NodeId) -> Cmd {
    Cmd::Node(NodeCmd::Update {
        at: id(n),
        version: w.s.nodes[&n].version,
        digest_version: None,
        token: None,
        patch: Patch {
            title: Some(text("a new title")),
            ..Patch::default()
        },
    })
}

// ------------------------------------------------------------------------------------------------
// talk decide

#[test]
fn decide_refusals_in_order() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/proof", &[]);
    let lemma = node(&mut w, &c, s, "/lemma", &[]);
    refused(&mut w, &c, s, decide_cmd(NodeId(999), &[]), nf(What::Node));
    refused(
        &mut w,
        &c,
        s,
        decide_cmd(n, &[Eid(9_999)]),
        nf(What::Request),
    );
    refused(
        &mut w,
        &c,
        s,
        decide_with(n, |_, sup| *sup = Some(Eid(9_999))),
        nf(What::Decision),
    );
    refused(
        &mut w,
        &c,
        s,
        decide_with(n, |b, _| b.push(node_ref(NodeId(999), None))),
        nf(What::Node),
    );
    refused(
        &mut w,
        &c,
        s,
        decide_with(n, |b, _| b.push(BasisArg::Entry(Eid(9_999)))),
        nf(What::Entry),
    );
    let (d1, ..) = decided(&ok(w.call(&c, s, decide_cmd(n, &[]))));
    let (d2, ..) = decided(&ok(w.call(&c, s, decide_with(n, |_, sup| *sup = Some(d1)))));
    assert_eq!(
        w.s.decisions[&d1].state,
        DecisionState::Superseded { by: d2 }
    );
    refused(
        &mut w,
        &c,
        s,
        decide_with(n, |_, sup| *sup = Some(d1)),
        inv(Why::DecisionNotActive),
    );
    refused(
        &mut w,
        &c,
        s,
        decide_with(n, |b, _| b.push(node_ref(lemma, Some(9)))),
        inv(Why::VersionAhead),
    );
    // `not_found` before `invalid` (D.14.5): an inactive `supersedes` with an unknown basis node.
    refused(
        &mut w,
        &c,
        s,
        decide_with(n, |b, sup| {
            *sup = Some(d1);
            b.push(node_ref(NodeId(999), None));
        }),
        nf(What::Node),
    );
    // A version ahead on `at` itself is refused before the reference is dropped.
    refused(
        &mut w,
        &c,
        s,
        decide_with(n, |b, _| b.push(node_ref(n, Some(99)))),
        inv(Why::VersionAhead),
    );
}

#[test]
fn decide_stores_its_basis_references() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/proof", &[]);
    let open = node(&mut w, &c, s, "/kb/open", &[]);
    let done = node(&mut w, &c, s, "/kb/done", &[]);
    ok(w.call(&c, s, close_cmd(done, Outcome::Done)));
    let dup = node(&mut w, &c, s, "/kb/dup", &[]);
    ok(w.call(&c, s, merge(dup, open)));
    let (cited, ..) = decided(&ok(w.call(&c, s, decide_cmd(open, &[]))));
    let at = w.clock;
    let d = ok(w.call(
        &c,
        s,
        decide_with(n, |b, _| {
            b.push(node_ref(open, Some(1)));
            b.push(BasisArg::Node {
                at: NodeRef::Path(crate::apply::tests::path("/kb/done")),
                version: None,
            });
            b.push(node_ref(dup, None));
            b.push(BasisArg::Entry(cited));
            b.push(BasisArg::External {
                what: text("the 2025 survey"),
                checked_at: When::At(Time(at.0 - 1_000)),
                recheck: Some(When::In(Dur(86_400_000))),
            });
        }),
    ));
    let (dec, ..) = decided(&d);
    assert!(d.warnings.is_empty());
    assert_eq!(
        w.s.decisions[&dec].basis,
        vec![
            BasisRef::Node {
                id: open,
                version: Some(Version(1)),
                life_at: None
            },
            BasisRef::Node {
                id: done,
                version: None,
                life_at: Some(Outcome::Done)
            },
            // A merged node is stored as the node it lives on in (A-88).
            BasisRef::Node {
                id: open,
                version: None,
                life_at: None
            },
            BasisRef::Entry(cited),
            BasisRef::External {
                checked_at: Time(at.0 - 1_000),
                recheck: Some(at.plus(Dur(86_400_000)))
            },
        ]
    );
    // The external reference's `what` is the decision's `Basis` text.
    let text = w
        .log
        .iter()
        .rev()
        .find_map(|e| match &e.ev {
            Ev::Appended { eid, text, .. } if *eid == dec => Some(text.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        text.extra,
        vec![(
            crate::event::TextField::Basis,
            crate::apply::tests::text("the 2025 survey")
        )]
    );
    // What rests on `open`: the decision cites it, and the decision on it.
    let bl = query::backlinks(&w.s, open, 0, &mut NoMeter);
    assert!(bl.cited_by.contains(&dec));
}

/// 1.6 (§15, l6): an external reference checked after now is read as checked now, where 1.4
/// refused it: the agent's clock and the server's differ.
#[test]
fn an_external_reference_checked_after_now_is_read_as_now() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/proof", &[]);
    let at = w.clock;
    let d = ok(w.call(
        &c,
        s,
        decide_with(n, |b, _| {
            b.push(BasisArg::External {
                what: text("the build"),
                checked_at: When::In(Dur(60_000)),
                recheck: None,
            });
            b.push(BasisArg::External {
                what: text("the run"),
                checked_at: When::At(Time(at.0 + 3_600_000)),
                recheck: None,
            });
        }),
    ));
    let (dec, ..) = decided(&d);
    assert_eq!(
        w.s.decisions[&dec].basis,
        vec![
            BasisRef::External {
                checked_at: at,
                recheck: None
            },
            BasisRef::External {
                checked_at: at,
                recheck: None
            },
        ]
    );
}

#[test]
fn a_reference_to_its_own_node_or_to_the_decision_it_supersedes_is_dropped() {
    // 1.4, §15, R14 and re-run R33.
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/proof", &[]);
    let side = node(&mut w, &c, s, "/side", &[]);
    let lemma = node(&mut w, &c, s, "/lemma", &[]);
    ok(w.call(&c, s, merge(side, n)));
    let d = ok(w.call(
        &c,
        s,
        decide_with(n, |b, _| {
            b.push(node_ref(n, Some(1)));
            b.push(node_ref(lemma, None));
            b.push(node_ref(side, None));
        }),
    ));
    let (d1, ..) = decided(&d);
    assert_eq!(
        w.s.decisions[&d1].basis,
        vec![BasisRef::Node {
            id: lemma,
            version: None,
            life_at: None
        }]
    );
    let warn = d
        .warnings
        .iter()
        .find(|x| x.code == WarningCode::SelfBasisDropped)
        .expect("self_basis_dropped");
    assert_eq!(warn.detail, WarningDetail::SelfBasisDropped { dropped: 2 });
    assert_eq!(warn.id, Some(n));
    // A decision that supersedes d1 and cites it drops that reference.
    let d = ok(w.call(
        &c,
        s,
        decide_with(n, |b, sup| {
            *sup = Some(d1);
            b.push(BasisArg::Entry(d1));
            b.push(node_ref(lemma, None));
        }),
    ));
    let (d2, ..) = decided(&d);
    assert_eq!(w.s.decisions[&d2].basis.len(), 1);
    let warn = d
        .warnings
        .iter()
        .find(|x| x.code == WarningCode::SupersededBasisDropped)
        .expect("superseded_basis_dropped");
    assert_eq!(
        warn.detail,
        WarningDetail::SupersededBasisDropped {
            decision: d1,
            dropped: 1
        }
    );
    assert!(!flagged(&w, n), "not stale at once");
}

#[test]
fn decided_names_why_a_listed_request_is_not_settled() {
    // 1.4, §20.7, R9.
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let r_settled = request(&mut w, s, n, "chat:a/1/m1", "chat:a/1");
    ok(w.call(&c, s, said_cmd(n, None, SaidKind::Reply)));
    let (_, settled, _) = decided(&ok(w.call(&c, s, decide_cmd(n, &[r_settled]))));
    assert_eq!(settled, vec![r_settled]);
    let r_open = request(&mut w, s, n, "chat:a/1/m2", "chat:a/1");
    // A user request, routed by a `user:` key.
    let e = super::super::keys::tests::user_entry(&mut w, s, USER_PRINCIPAL);
    let k = alloc::format!("user:{}", e.render());
    ok(w.call(
        &c,
        s,
        Cmd::Node(NodeCmd::Key {
            at: id(n),
            change: KeyChange::Add(Key {
                key: KeyStr::new(&k).unwrap(),
                nv: NormVer(0),
            }),
        }),
    ));
    let r_user = match w.s.user_entries[&e].routing {
        crate::state::Routing::Request { request, .. } => request,
        other => panic!("{other:?}"),
    };
    let (_, settled, not_settled) = decided(&ok(w.call(
        &c,
        s,
        decide_cmd(n, &[r_settled, r_open, r_user]),
    )));
    assert!(settled.is_empty());
    assert_eq!(
        not_settled,
        vec![
            NotSettled {
                request: r_settled,
                why: NotSettledWhy::SettledBefore
            },
            NotSettled {
                request: r_open,
                why: NotSettledWhy::AwaitsSaid {
                    node: n,
                    conv: KeyStr::new("chat:a/1").unwrap()
                }
            },
            NotSettled {
                request: r_user,
                why: NotSettledWhy::AwaitsResult {
                    requester: USER_PRINCIPAL
                }
            },
        ]
    );
    // The reply answers r_open, which the decision above lists: it settles at once.
    let d = ok(w.call(&c, s, said_cmd(n, None, SaidKind::Reply)));
    assert!(matches!(&d.reply, Reply::Said { settled, .. } if *settled == vec![r_open]));
    // An answered request that no decision lists yet always settles when one does.
    let r_answered = request(&mut w, s, n, "chat:a/1/m3", "chat:a/1");
    ok(w.call(&c, s, said_cmd(n, None, SaidKind::Reply)));
    assert!(w.s.requests[&r_answered].answered_by.is_some());
    let (_, settled, not_settled) = decided(&ok(w.call(&c, s, decide_cmd(n, &[r_answered]))));
    assert_eq!(settled, vec![r_answered]);
    assert!(not_settled.is_empty());
}

#[test]
fn a_decision_records_the_expired_questions_assumption_only_once() {
    // D.17.4, D.14.5: `PqAssumed` only for a question still expired with no assumption.
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let q = super::super::keys::tests::people_question(&mut w, s, n, "chat:a/1", false);
    let q2 = super::super::keys::tests::people_question(&mut w, s, n, "chat:a/1", false);
    // q expires by its events alone (the expiry's digest entries are WP-D5a's).
    w.emit(Ev::PqExpired { q });
    w.emit(Ev::MarkSet {
        node: n,
        mark: Mark::Assume,
        question: q,
        on: true,
    });
    // q2 is marked too, but still open (a hand-built mark): no assumption is recorded for it.
    w.emit(Ev::MarkSet {
        node: n,
        mark: Mark::Assume,
        question: q2,
        on: true,
    });
    let (d, ..) = decided(&ok(w.call(&c, s, decide_cmd(n, &[]))));
    assert_eq!(w.s.pquestions[&q].assumed_by, Some(d));
    assert_eq!(w.s.pquestions[&q2].assumed_by, None);
    assert_eq!(w.s.pquestions[&q2].state, PqState::Open);
    assert!(w.s.nodes[&n].assume_marks.is_empty());
    let assumed: Vec<_> = evs(&w)
        .into_iter()
        .filter(|e| matches!(e, Ev::PqAssumed { .. }))
        .collect();
    assert_eq!(assumed, vec![Ev::PqAssumed { q, decision: d }]);
    // The decision's body records both as its assumptions.
    let body = w
        .log
        .iter()
        .find_map(|e| match &e.ev {
            Ev::Appended {
                eid,
                body: EntryBody::Decision { assumes, .. },
                ..
            } if *eid == d => Some(assumes.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(body, BTreeSet::from([q, q2]));
}

#[test]
fn a_second_decision_on_another_marked_node_records_no_second_assumption() {
    // D.14.5: an expiry that marked two nodes; the first decision records the assumption, and a
    // decision on the other node clears its mark and lists the question, with no second
    // `PqAssumed` (`evolve` would count it as an anomaly).
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let a = node(&mut w, &c, s, "/support/a", &[]);
    let b = node(&mut w, &c, s, "/support/b", &[]);
    let q = super::super::keys::tests::people_question(&mut w, s, n, "chat:a/1", false);
    w.emit(Ev::PqExpired { q });
    for x in [a, b] {
        w.emit(Ev::MarkSet {
            node: x,
            mark: Mark::Assume,
            question: q,
            on: true,
        });
    }
    let (d1, ..) = decided(&ok(w.call(&c, s, decide_cmd(a, &[]))));
    assert!(evs(&w).contains(&Ev::PqAssumed { q, decision: d1 }));
    assert_eq!(w.s.pquestions[&q].assumed_by, Some(d1));
    assert!(w.s.nodes[&b].assume_marks.contains(&q));
    let (d2, ..) = decided(&ok(w.call(&c, s, decide_cmd(b, &[]))));
    let e = evs(&w);
    assert!(!e.iter().any(|x| matches!(x, Ev::PqAssumed { .. })));
    assert!(e.contains(&Ev::MarkSet {
        node: b,
        mark: Mark::Assume,
        question: q,
        on: false,
    }));
    assert_eq!(w.s.pquestions[&q].assumed_by, Some(d1));
    assert_eq!(w.s.pquestions[&q].state, PqState::Expired);
    assert!(w.s.nodes[&b].assume_marks.is_empty());
    let assumes = w
        .log
        .iter()
        .find_map(|e| match &e.ev {
            Ev::Appended {
                eid,
                body: EntryBody::Decision { assumes, .. },
                ..
            } if *eid == d2 => Some(assumes.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(assumes, BTreeSet::from([q]));
}

#[test]
fn a_decision_on_an_open_signal_node_closes_it() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &[]);
    let b = node(&mut w, &c, s, "/b", &[]);
    ok(w.call(&c, s, link(a, LinkRel::Duplicate, b)));
    let sig = super::super::keys::tests::signal(&w, SignalKind::Duplicate, a, b).expect("a signal");
    ok(w.call(&c, s, decide_cmd(sig, &[])));
    assert!(!w.s.nodes[&sig].is_open());
    assert!(super::super::keys::tests::signal(&w, SignalKind::Duplicate, a, b).is_none());
}

// ------------------------------------------------------------------------------------------------
// basis_changed, one hop (D-4)

#[test]
fn basis_changed_follows_the_cited_node_and_decision() {
    let (mut w, c, s) = one();
    let thm = node(&mut w, &c, s, "/thm", &[]);
    let lemma = node(&mut w, &c, s, "/lemma", &[]);
    let open = node(&mut w, &c, s, "/open", &[]);
    ok(w.call(&c, s, close_cmd(lemma, Outcome::Done)));
    let (cite, ..) = decided(&ok(w.call(
        &c,
        s,
        decide_with(thm, |b, _| {
            b.push(node_ref(lemma, None));
            b.push(node_ref(open, None));
        }),
    )));
    assert!(!flagged(&w, thm));
    // An edit, a reopen, or a merge of a cited node does not make it stale.
    ok(w.call(&c, s, update_title(&w, open)));
    assert!(!flagged(&w, thm));
    ok(w.call(
        &c,
        s,
        Cmd::Node(NodeCmd::Reopen {
            at: id(lemma),
            reason: text("re-examine"),
        }),
    ));
    assert!(!flagged(&w, thm));
    // Closing it again `failed` does.
    ok(w.call(&c, s, close_cmd(lemma, Outcome::Failed)));
    assert!(flagged(&w, thm));
    // A decision that supersedes the citing one with a basis as it stands now clears it.
    let (d2, ..) = decided(&ok(w.call(
        &c,
        s,
        decide_with(thm, |b, sup| {
            *sup = Some(cite);
            b.push(node_ref(lemma, None));
        }),
    )));
    assert!(!flagged(&w, thm));
    // An open cited node that closes `dropped` (not `done`) flags it.
    let (d3, ..) = decided(&ok(w.call(
        &c,
        s,
        decide_with(thm, |b, _| b.push(node_ref(open, None))),
    )));
    ok(w.call(&c, s, close_cmd(open, Outcome::Dropped)));
    assert!(flagged(&w, thm));
    // The retraction of the citing decision clears it.
    ok(w.call(&c, s, retract_entry(d3, thm)));
    assert_eq!(
        w.s.decisions[&d3].state,
        DecisionState::Retracted {
            by: w.s.counters.next_eid.prev()
        }
    );
    assert!(!flagged(&w, thm));
    let _ = d2;
}

/// The eid before this one.
trait Prev {
    fn prev(self) -> Eid;
}

impl Prev for Eid {
    fn prev(self) -> Eid {
        Eid(self.0 - 1)
    }
}

#[test]
fn a_cited_decision_superseded_or_retracted_and_a_retracted_entry_or_node_flag_the_citer() {
    let (mut w, c, s) = one();
    let base = node(&mut w, &c, s, "/base", &[]);
    let thm = node(&mut w, &c, s, "/thm", &[]);
    let thm2 = node(&mut w, &c, s, "/thm2", &[]);
    let thm3 = node(&mut w, &c, s, "/thm3", &[]);
    let lemma = node(&mut w, &c, s, "/lemma", &[]);
    let (b1, ..) = decided(&ok(w.call(&c, s, decide_cmd(base, &[]))));
    let (b2, ..) = decided(&ok(w.call(&c, s, decide_cmd(base, &[]))));
    ok(w.call(&c, s, decide_with(thm, |b, _| b.push(BasisArg::Entry(b1)))));
    ok(w.call(&c, s, decide_with(thm2, |b, _| b.push(BasisArg::Entry(b2)))));
    ok(w.call(
        &c,
        s,
        decide_with(thm3, |b, _| b.push(node_ref(lemma, None))),
    ));
    // A cited decision superseded.
    ok(w.call(&c, s, decide_with(base, |_, sup| *sup = Some(b1))));
    assert!(flagged(&w, thm));
    // A cited decision retracted.
    ok(w.call(&c, s, retract_entry(b2, base)));
    assert!(flagged(&w, thm2));
    assert!(matches!(
        w.s.decisions[&b2].state,
        DecisionState::Retracted { .. }
    ));
    // A cited node retracted.
    assert!(!flagged(&w, thm3));
    ok(w.call(&c, s, retract_node(lemma)));
    assert!(flagged(&w, thm3));
    assert!(
        w.s.retracted
            .contains(&crate::state::BasisTarget::Node(lemma))
    );
}

#[test]
fn retracting_a_superseded_decision_changes_no_state_and_a_settled_request_stays_settled() {
    // A-11, §25.
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let d = ok(w.call(&c, s, request_cmd(n, "chat:a/1/m1", Some("chat:a/1"))));
    let Reply::Request { request: r, .. } = d.reply else {
        panic!()
    };
    let (d1, ..) = decided(&ok(w.call(&c, s, decide_cmd(n, &[r]))));
    ok(w.call(&c, s, said_cmd(n, None, SaidKind::Reply)));
    assert!(w.s.requests[&r].settled.is_some());
    let (d2, ..) = decided(&ok(w.call(&c, s, decide_with(n, |_, sup| *sup = Some(d1)))));
    let d = ok(w.call(&c, s, retract_entry(d1, n)));
    let Reply::Retracted { entry, .. } = d.reply else {
        panic!()
    };
    assert_eq!(
        w.s.decisions[&d1].state,
        DecisionState::Superseded { by: d2 }
    );
    assert!(
        evs(&w)
            .iter()
            .all(|e| !matches!(e, Ev::DecisionRetracted { .. }))
    );
    assert!(
        w.s.retracted
            .contains(&crate::state::BasisTarget::Entry(d1))
    );
    assert!(w.s.requests.get(&r).is_none_or(|x| x.settled.is_some()));
    let _ = entry;
}

#[test]
fn the_rs9_trace_flags_what_rests_on_a_refuted_lemma_and_on_its_merged_duplicate() {
    // #100 refuted, #200 cites it: #200 is flagged; and the variant where #200 cites a duplicate
    // of #100 that is then merged into #100.
    for variant in [false, true] {
        let (mut w, c, s) = one();
        let lemma = node(&mut w, &c, s, "/kb/lemma", &[]);
        let dup = node(&mut w, &c, s, "/kb/dup", &[]);
        let concl = node(&mut w, &c, s, "/concl", &[]);
        ok(w.call(&c, s, close_cmd(lemma, Outcome::Done)));
        let cited = if variant { dup } else { lemma };
        ok(w.call(
            &c,
            s,
            decide_with(concl, |b, _| b.push(node_ref(cited, None))),
        ));
        ok(w.call(&c, s, close_cmd(concl, Outcome::Done)));
        if variant {
            ok(w.call(
                &c,
                s,
                Cmd::Node(NodeCmd::Reopen {
                    at: id(lemma),
                    reason: text("merge the duplicate"),
                }),
            ));
            ok(w.call(&c, s, merge(dup, lemma)));
            assert!(!flagged(&w, concl), "a merge flags nothing");
        } else {
            ok(w.call(
                &c,
                s,
                Cmd::Node(NodeCmd::Reopen {
                    at: id(lemma),
                    reason: text("re-examine"),
                }),
            ));
        }
        ok(w.call(&c, s, close_cmd(lemma, Outcome::Failed)));
        assert!(flagged(&w, concl), "variant {variant}");
        // A closed `done` conclusion with the flag is offered by `next` at tier 0, ahead of the
        // open nodes, and reopened by taking (D.12.1): by `next` itself, from a fresh session,
        // and by `take` in a copy of the world.
        let mut by_take = w.clone();
        let fresh = w.hello(&c);
        let d = ok(w.call(&c, fresh, crate::apply::tests::next()));
        let Reply::Next(NextReply::Hold {
            node: offered,
            why,
            reopened,
            ..
        }) = d.reply
        else {
            panic!("variant {variant}: {:?}", d.reply)
        };
        assert_eq!(offered, concl, "variant {variant}");
        assert_eq!(why.tier, 0);
        assert!(why.flags.contains(Flag::BasisChanged));
        assert_eq!(reopened, Some(Outcome::Done));
        assert!(w.s.nodes[&concl].is_open());
        let t = ok(by_take.call(&c, s, take(concl)));
        assert!(matches!(
            t.reply,
            Reply::Taken {
                reopened: Some(Outcome::Done),
                ..
            }
        ));
    }
}

// ------------------------------------------------------------------------------------------------
// approve, object, resolve, retract

#[test]
fn approve_counts_the_approvals_of_the_current_content() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/doc", &[]);
    let approve = |v: u64| {
        Cmd::Talk(TalkCmd::Approve {
            on: id(n),
            version: Version(v),
            checked: text("ran the tests"),
        })
    };
    refused(
        &mut w,
        &c,
        s,
        Cmd::Talk(TalkCmd::Approve {
            on: id(NodeId(999)),
            version: Version(1),
            checked: text("c"),
        }),
        nf(What::Node),
    );
    refused(&mut w, &c, s, approve(9), inv(Why::VersionAhead));
    let d = ok(w.call(&c, s, approve(1)));
    let Reply::Approved { approvals, .. } = d.reply else {
        panic!()
    };
    assert_eq!(approvals, 1);
    // A title change: the old approval lapses; one of the old version is recorded at its version.
    ok(w.call(&c, s, update_title(&w, n)));
    let d = ok(w.call(&c, s, approve(1)));
    let Reply::Approved { approvals, entry } = d.reply else {
        panic!()
    };
    assert_eq!(approvals, 0);
    assert_eq!(w.s.approvals[&(n, Version(1))], 2);
    let v = w.s.nodes[&n].version.0;
    let d = ok(w.call(&c, s, approve(v)));
    let Reply::Approved { approvals, .. } = d.reply else {
        panic!()
    };
    assert_eq!(approvals, 1);
    let _ = entry;
}

#[test]
fn object_and_resolve() {
    let (mut w, c, s) = one();
    let s2 = w.hello(&c);
    let n = node(&mut w, &c, s, "/doc", &[]);
    refused(
        &mut w,
        &c,
        s,
        Cmd::Talk(TalkCmd::Object {
            on: id(NodeId(999)),
            note: text("no"),
        }),
        nf(What::Node),
    );
    let d = ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Object {
            on: id(n),
            note: text("the fix skips the last page"),
        }),
    ));
    let Reply::Objected { objection: o } = d.reply else {
        panic!()
    };
    assert!(w.s.ix.node[&n].flags.contains(Flag::Objection));
    let resolve = |o: Eid| {
        Cmd::Talk(TalkCmd::Resolve {
            objection: o,
            note: text("fixed in the second pass"),
            upheld: false,
            defer: false,
        })
    };
    refused(&mut w, &c, s, resolve(Eid(9_999)), nf(What::Objection));
    // Anyone may resolve.
    let d = ok(w.call(&c, s2, resolve(o)));
    let Reply::ObjectionResolved {
        entry: Some(e),
        corrected: None,
        ..
    } = d.reply
    else {
        panic!("{:?}", d.reply)
    };
    assert_eq!(w.s.objections[&o].state, ObjectionState::Resolved { by: e });
    assert!(!w.s.ix.node[&n].flags.contains(Flag::Objection));
    // 1.10: the same verdict again records nothing, and names the verdict.
    let d = ok(w.call(&c, s, resolve(o)));
    assert_eq!(
        d.reply,
        Reply::ObjectionResolved {
            entry: None,
            corrected: None,
            already: Some(crate::reply::Verdict {
                verdict: e,
                upheld: false
            }),
            disputed: None,
            withdrawn: None,
            deferred: None,
        }
    );
    assert!(evs(&w).iter().all(|e| matches!(e, Ev::Touched { .. })));
}

#[test]
fn retract_refusals() {
    // D.14.6: an eid the shell's table lacks reaches the core with `UNKNOWN_NODE`.
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/doc", &[]);
    refused(&mut w, &c, s, retract_node(NodeId(999)), nf(What::Node));
    refused(&mut w, &c, s, retract_entry(Eid(9_999), n), nf(What::Entry));
    let e = entry(&mut w, n, Author::Session(s), EntryBody::Object);
    refused(
        &mut w,
        &c,
        s,
        retract_entry(e, NodeId(u64::MAX)),
        nf(What::Entry),
    );
    // An entry that is not a decision is retracted as a statement: no state but `retracted`.
    let d = ok(w.call(&c, s, retract_entry(e, n)));
    assert!(matches!(d.reply, Reply::Retracted { .. }));
    assert!(w.s.retracted.contains(&crate::state::BasisTarget::Entry(e)));
}
