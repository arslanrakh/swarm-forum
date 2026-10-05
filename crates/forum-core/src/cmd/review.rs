//! The commands of `talk decide`, `approve`, `object`, `resolve`, and `retract` (design D.8.17,
//! §11, §15, CS-22, D-4). Owner: WP-D5b.
//!
//! Each command is a pair: a validation that reads the state and returns the plan or the error,
//! and an execution of the plan that emits the events and cannot fail. Every validation follows
//! D.8.24's row of its command, with `not_found` before `invalid` (D.14.5).

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::apply::{node_of, target};
use crate::cmd::{BasisArg, Cmd, RetractArg, TalkCmd};
use crate::error::{Error, What, Why};
use crate::event::{EndCause, EntryBody, EntryText, Ev, TextField};
use crate::helpers::Exec;
use crate::helpers::close::{Closing, assume_marks_off, close_node};
use crate::helpers::conversation::try_settle;
use crate::query::resolve_merged;
use crate::reply::{
    Corrected, Deferred, Disputed, NotSettled, NotSettledWhy, Reply, Verdict, Warning, WarningCode,
    WarningDetail,
};
use crate::state::{BasisRef, BasisTarget, DecisionState, Life, ObjectionState, Role, State};
use crate::types::{
    DecisionId, Eid, Evidence, NodeId, ObjectionId, Outcome, RequestId, Requester, Text, Version,
};

use super::Vcx;

/// A text as an entry's main text.
fn main(t: &Text) -> EntryText {
    EntryText {
        main: t.clone(),
        extra: Vec::new(),
    }
}

/// The target `at` or `on`: it resolved (`target` returned `Some`).
fn at_of(cx: &Vcx<'_>, cmd: &Cmd) -> Result<NodeId, Error> {
    target(cx.s, cmd)?.ok_or(Error::not_found(What::Node))
}

/// The outcome of `n` while it is closed, None while it is open.
fn outcome_of(s: &State, n: NodeId) -> Option<Outcome> {
    match &s.nodes.get(&n)?.life {
        Life::Closed(c) => Some(c.outcome.clone()),
        Life::Open => None,
    }
}

// ------------------------------------------------------------------------------------------------
// decide

/// The plan of `talk decide` (D.8.17).
#[derive(Debug)]
pub(crate) struct DecidePlan {
    at: NodeId,
    settles: BTreeSet<RequestId>,
    supersedes: Option<DecisionId>,
    evidence: Option<Evidence>,
    /// The references kept, resolved (CS-22, A-88).
    basis: Vec<BasisRef>,
    /// The decision's text, then the `what` of each external reference kept, in order.
    text: EntryText,
    /// The references to `at`, directly or through a node merged into it, dropped (1.4, R14).
    self_dropped: u32,
    /// The references to the decision `supersedes` names, dropped (1.4, re-run R33).
    superseded_dropped: u32,
    /// The requests of `settles` that were settled before this call (R9).
    settled_before: BTreeSet<RequestId>,
}

impl DecidePlan {
    /// 1.12 (§10.3): the decision of a `hold release {settles}` on `at`, as `talk decide {at,
    /// text: done, settles}` would plan it: no supersession, evidence, or basis.
    pub(super) fn of_release(
        s: &State,
        at: NodeId,
        settles: BTreeSet<RequestId>,
        done: &Text,
    ) -> DecidePlan {
        let settled_before = settles
            .iter()
            .copied()
            .filter(|r| s.requests.get(r).is_some_and(|x| x.settled.is_some()))
            .collect();
        DecidePlan {
            at,
            settles,
            supersedes: None,
            evidence: None,
            basis: Vec::new(),
            text: main(done),
            self_dropped: 0,
            superseded_dropped: 0,
            settled_before,
        }
    }
}

/// One basis reference, resolved: what the decision stores, or a reference that is dropped.
enum Resolved {
    /// Kept, with the `what` of an external reference.
    Keep(BasisRef, Option<Text>),
    /// It names `at`, directly or through `merged_into` (R14).
    SelfRef,
    /// It names the decision this one supersedes (R33).
    Superseded,
}

/// Validates `talk decide {at, text, settles?, supersedes?, basis?, evidence?}` (D.8.17, D.8.24's
/// row): `at` → the requests of `settles` (`not_found(Request)`) → `supersedes` is a decision
/// (`not_found(Decision)`) → each basis reference resolves: a node (`not_found(Node)`), an entry
/// below `next_eid` (`not_found(Entry)`, D.4.5) → `supersedes` is active
/// (`invalid(DecisionNotActive)`) → each node reference's version is at most the node's
/// (`invalid(VersionAhead)`, A-71); an external reference's `checked_at` after `now` is read as
/// `now` (1.6, §15: the agent's clock and the server's differ, l6), where 1.4 refused it with
/// `invalid(CheckedAtAhead)`. Then the drops, which refuse nothing: a node reference
/// that names `at`, directly or through `merged_into` (`self_basis_dropped`, R14), and an entry
/// reference to the decision `supersedes` names (`superseded_basis_dropped`, R33). Each node
/// reference kept is stored as the node it lives on in, with that node's outcome now (A-88, D-4).
pub(crate) fn decide(cx: &Vcx<'_>, cmd: &Cmd) -> Result<DecidePlan, Error> {
    let Cmd::Talk(TalkCmd::Decide {
        text,
        settles,
        supersedes,
        basis,
        evidence,
        ..
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    let s = cx.s;
    let now = cx.now;
    let at = at_of(cx, cmd)?;
    if settles.iter().any(|r| !s.requests.contains_key(r)) {
        return Err(Error::not_found(What::Request));
    }
    let old = match supersedes {
        Some(d) => Some(s.decisions.get(d).ok_or(Error::not_found(What::Decision))?),
        None => None,
    };
    // `not_found` for every reference first, then the `invalid`s (D.14.5).
    let mut nodes: Vec<Option<NodeId>> = Vec::with_capacity(basis.len());
    for b in basis {
        match b {
            BasisArg::Node { at: r, .. } => nodes.push(Some(node_of(s, r)?)),
            BasisArg::Entry(e) => {
                if *e >= s.counters.next_eid {
                    return Err(Error::not_found(What::Entry));
                }
                nodes.push(None);
            }
            BasisArg::External { .. } => nodes.push(None),
        }
    }
    if old.is_some_and(|d| d.state != DecisionState::Active) {
        return Err(Error::invalid(Why::DecisionNotActive));
    }
    let mut resolved = Vec::with_capacity(basis.len());
    for (b, n) in basis.iter().zip(nodes) {
        let r = match (b, n) {
            (BasisArg::Node { version, .. }, Some(n)) => {
                let current = s.nodes.get(&n).map_or(Version(0), |x| x.version);
                if version.is_some_and(|v| v > current) {
                    return Err(Error::invalid(Why::VersionAhead));
                }
                let id = resolve_merged(s, n);
                if n == at || id == at {
                    Resolved::SelfRef
                } else {
                    Resolved::Keep(
                        BasisRef::Node {
                            id,
                            version: *version,
                            life_at: outcome_of(s, id),
                        },
                        None,
                    )
                }
            }
            (BasisArg::Entry(e), _) => {
                if Some(*e) == *supersedes {
                    Resolved::Superseded
                } else {
                    Resolved::Keep(BasisRef::Entry(*e), None)
                }
            }
            (
                BasisArg::External {
                    what,
                    checked_at,
                    recheck,
                },
                _,
            ) => {
                let checked_at = checked_at.resolve(now).min(now);
                Resolved::Keep(
                    BasisRef::External {
                        checked_at,
                        recheck: recheck.map(|w| w.resolve(now)),
                    },
                    Some(what.clone()),
                )
            }
            // A node reference always resolved to a node above.
            (BasisArg::Node { .. }, None) => return Err(Error::not_found(What::Node)),
        };
        resolved.push(r);
    }
    let mut kept = Vec::new();
    let mut whats = Vec::new();
    let (mut self_dropped, mut superseded_dropped) = (0_u32, 0_u32);
    for r in resolved {
        match r {
            Resolved::Keep(b, what) => {
                kept.push(b);
                if let Some(w) = what {
                    whats.push((TextField::Basis, w));
                }
            }
            Resolved::SelfRef => self_dropped = self_dropped.saturating_add(1),
            Resolved::Superseded => superseded_dropped = superseded_dropped.saturating_add(1),
        }
    }
    let settled_before = settles
        .iter()
        .copied()
        .filter(|r| s.requests.get(r).is_some_and(|x| x.settled.is_some()))
        .collect();
    Ok(DecidePlan {
        at,
        settles: settles.clone(),
        supersedes: *supersedes,
        evidence: *evidence,
        basis: kept,
        text: EntryText {
            main: text.clone(),
            extra: whats,
        },
        self_dropped,
        superseded_dropped,
        settled_before,
    })
}

/// Why a request of `settles` that this decision did not settle is not settled (1.4, §20.7, R9):
/// settled before; else unanswered, awaiting a `to_user` of kind `result` to its requester, or a
/// `said` in its conversation on its node (§9.4).
pub(super) fn not_settled_why(
    s: &State,
    r: RequestId,
    before: &BTreeSet<RequestId>,
) -> NotSettledWhy {
    if before.contains(&r) {
        return NotSettledWhy::SettledBefore;
    }
    match s.requests.get(&r) {
        Some(x) => match x.requester {
            Requester::User(p) => NotSettledWhy::AwaitsResult { requester: p },
            Requester::External => NotSettledWhy::AwaitsSaid {
                node: x.node,
                conv: x.conv.clone(),
            },
        },
        // A request a decision lists stays resident until it settles (D.4.12).
        None => NotSettledWhy::SettledBefore,
    }
}

/// Executes `talk decide` (D.8.17): the decision on `at`, recording `at`'s assumptions; the
/// supersession; the assumptions recorded (`PqAssumed` only for a people question still expired
/// with no assumption, D.14.5) and their marks off; `try_settle` for each request it lists; and,
/// on an open signal node, the close as `done` that a decision covering it makes (§14, A-34). The
/// reply names the requests it settled and, for each other one it lists, why not (R9); the
/// warnings name the references dropped (R14, R33).
pub(crate) fn decide_exec(ex: &mut Exec<'_>, plan: DecidePlan) -> Reply {
    let at = plan.at;
    let (d, settled, not_settled) = record_decision(ex, &plan);
    if plan.self_dropped > 0 {
        ex.warn(Warning {
            code: WarningCode::SelfBasisDropped,
            id: Some(at),
            detail: WarningDetail::SelfBasisDropped {
                dropped: plan.self_dropped,
            },
        });
    }
    if let Some(old) = plan.supersedes
        && plan.superseded_dropped > 0
    {
        ex.warn(Warning {
            code: WarningCode::SupersededBasisDropped,
            id: Some(at),
            detail: WarningDetail::SupersededBasisDropped {
                decision: old,
                dropped: plan.superseded_dropped,
            },
        });
    }
    Reply::Decided {
        decision: d,
        settled,
        not_settled,
    }
}

/// The events of `talk decide` (D.8.17), which a `hold release {settles}` records too (1.12,
/// §10.3): the decision on `at`, recording `at`'s assumptions; the supersession; the assumptions'
/// marks off; `try_settle` for each request it lists; and, on an open signal node, the close as
/// `done` (§14, A-34). Returns the decision, the requests it settled, and each other one it lists
/// with why it did not settle (R9).
pub(super) fn record_decision(
    ex: &mut Exec<'_>,
    plan: &DecidePlan,
) -> (DecisionId, Vec<RequestId>, Vec<NotSettled>) {
    let at = plan.at;
    let by = ex.author();
    let assumes = ex
        .s()
        .nodes
        .get(&at)
        .map(|x| x.assume_marks.clone())
        .unwrap_or_default();
    let main_text = plan.text.main.clone();
    let d = ex.append(
        at,
        by,
        EntryBody::Decision {
            settles: plan.settles.clone(),
            supersedes: plan.supersedes,
            assumes: assumes.clone(),
            evidence: plan.evidence,
            basis: plan.basis.clone(),
        },
        plan.text.clone(),
    );
    if let Some(old) = plan.supersedes {
        ex.emit(Ev::DecisionSuperseded { d: old, by: d });
    }
    assume_marks_off(ex, at, d, &assumes);
    let mut settled = Vec::new();
    for &r in &plan.settles {
        if try_settle(ex, r) {
            settled.push(r);
        }
    }
    let not_settled = plan
        .settles
        .iter()
        .copied()
        .filter(|r| !settled.contains(r))
        .map(|r| NotSettled {
            request: r,
            why: not_settled_why(ex.s(), r, &plan.settled_before),
        })
        .collect();
    let signal = ex
        .s()
        .nodes
        .get(&at)
        .is_some_and(|x| x.is_open() && matches!(x.role, Role::Signal(_)));
    if signal {
        close_node(
            ex,
            at,
            Closing {
                outcome: Outcome::Done,
                by,
                merged_into: None,
                cause: EndCause::Close,
                text: main(&main_text),
                reslot: None,
            },
        );
    }
    (d, settled, not_settled)
}

// ------------------------------------------------------------------------------------------------
// approve

/// The plan of `talk approve` (D.8.17).
#[derive(Debug)]
pub(crate) struct ApprovePlan {
    n: NodeId,
    version: Version,
    checked: Text,
}

/// Validates `talk approve {on: id@version, checked}` (§11.3, D.8.24's row): `on` →
/// `invalid(VersionAhead)` for a version newer than the node's (A-71). An older version is
/// recorded at its own version.
pub(crate) fn approve(cx: &Vcx<'_>, cmd: &Cmd) -> Result<ApprovePlan, Error> {
    let Cmd::Talk(TalkCmd::Approve {
        version, checked, ..
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    let n = at_of(cx, cmd)?;
    let current = cx.s.nodes.get(&n).map_or(Version(0), |x| x.version);
    if *version > current {
        return Err(Error::invalid(Why::VersionAhead));
    }
    Ok(ApprovePlan {
        n,
        version: *version,
        checked: checked.clone(),
    })
}

/// Executes `talk approve`: the approval at its version, which counts towards the approvals of the
/// current content while that version is at or after `content_version` (CS-34).
pub(crate) fn approve_exec(ex: &mut Exec<'_>, plan: ApprovePlan) -> Reply {
    let by = ex.author();
    let entry = ex.append(
        plan.n,
        by,
        EntryBody::Approve {
            version: plan.version,
        },
        EntryText {
            main: Text::default(),
            extra: alloc::vec![(TextField::Checked, plan.checked)],
        },
    );
    Reply::Approved {
        entry,
        approvals: crate::query::reads::current_approvals(ex.s(), plan.n),
    }
}

// ------------------------------------------------------------------------------------------------
// object

/// The plan of `talk object` (D.8.17).
#[derive(Debug)]
pub(crate) struct ObjectPlan {
    n: NodeId,
    note: Text,
}

/// Validates `talk object {on, note}` (§11.3): `on` resolves.
pub(crate) fn object(cx: &Vcx<'_>, cmd: &Cmd) -> Result<ObjectPlan, Error> {
    let Cmd::Talk(TalkCmd::Object { note, .. }) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    Ok(ObjectPlan {
        n: at_of(cx, cmd)?,
        note: note.clone(),
    })
}

/// Executes `talk object`: an open objection, which sets the `objection` flag.
pub(crate) fn object_exec(ex: &mut Exec<'_>, plan: ObjectPlan) -> Reply {
    let by = ex.author();
    let objection = ex.append(plan.n, by, EntryBody::Object, main(&plan.note));
    Reply::Objected { objection }
}

// ------------------------------------------------------------------------------------------------
// resolve

/// The plan of `talk resolve` (D.8.17).
#[derive(Debug)]
pub(crate) struct ResolvePlan {
    objection: ObjectionId,
    note: Text,
    /// 1.8 (§11.3): the objection stands.
    upheld: bool,
    what: ResolveWhat,
}

/// What a resolve does (§11.3).
#[derive(Debug)]
enum ResolveWhat {
    /// The objection is open, on this node; 1.10: a resolve that keeps a claim closed `done` with
    /// no basis warns `no_basis`, naming this retracted decision.
    Resolve {
        node: NodeId,
        no_basis: Option<DecisionId>,
    },
    /// 1.10: resolved already, with the same verdict: nothing is recorded.
    Already(Verdict),
    /// 1.10: resolved already, with the opposite verdict: the caller's text becomes a new open
    /// objection on `at`, the objection's node followed through `merged_into`, and the correction
    /// the earlier verdict made is withdrawn when it still marks the node.
    Dispute {
        at: NodeId,
        earlier: Verdict,
        withdraw: Option<Corrected>,
    },
    /// 1.11: the objection is open, on this node, and the caller cannot decide it yet: a deferral,
    /// which decides nothing; on a closed node, its review, the caller's class is backed off the
    /// node as a `cannot` release backs it off (§10.3).
    Defer { node: NodeId, closed: bool },
}

/// Validates `talk resolve {objection, note, upheld? | defer?}` (§11.3): the objection exists
/// (`not_found(Objection)`); 1.11: `defer` with `upheld` is `invalid(BadParams)`, a deferral
/// deciding nothing (the shell refuses it first, naming both). Anyone may resolve it. Then, which
/// refuses nothing: for an open objection, `no_basis` (1.10), or, deferred, whether its node is
/// closed (1.11); for a resolved one, whether the verdict is the same as the one that resolved it
/// (`{already}`, as for a deferral of it) or the opposite (1.10: a dispute, which withdraws the
/// earlier verdict's correction if it still marks the node).
pub(crate) fn resolve(cx: &Vcx<'_>, cmd: &Cmd) -> Result<ResolvePlan, Error> {
    let Cmd::Talk(TalkCmd::Resolve {
        objection,
        note,
        upheld,
        defer,
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    let s = cx.s;
    let o = s
        .objections
        .get(objection)
        .ok_or(Error::not_found(What::Objection))?;
    if *defer && *upheld {
        return Err(Error::invalid(Why::BadParams));
    }
    let earlier = match o.state {
        ObjectionState::Open => None,
        ObjectionState::Resolved { by } => Some(Verdict {
            verdict: by,
            upheld: false,
        }),
        ObjectionState::Upheld { by } => Some(Verdict {
            verdict: by,
            upheld: true,
        }),
    };
    let what = match earlier {
        None if *defer => ResolveWhat::Defer {
            node: o.node,
            closed: s.nodes.get(&o.node).is_some_and(|x| !x.is_open()),
        },
        Some(v) if *defer => ResolveWhat::Already(v),
        None => ResolveWhat::Resolve {
            node: o.node,
            no_basis: if *upheld {
                None
            } else {
                crate::query::no_basis(s, o.node)
            },
        },
        Some(v) if v.upheld == *upheld => ResolveWhat::Already(v),
        Some(v) => ResolveWhat::Dispute {
            at: resolve_merged(s, o.node),
            earlier: v,
            withdraw: withdrawal(s, o.node, v),
        },
    };
    Ok(ResolvePlan {
        objection: *objection,
        note: note.clone(),
        upheld: *upheld,
        what,
    })
}

/// 1.10 (§5, §11.3): the correction that the upheld verdict `v` made of node `n`, withdrawn by a
/// second, opposite verdict: when `n` is closed and still marked by `v`, `{node, was: the corrected
/// outcome, outcome: the one it was closed with}` (for a correction recorded before 1.10, with no
/// `corrected_from`, the outcome stays and only the mark goes).
fn withdrawal(s: &State, n: NodeId, v: Verdict) -> Option<Corrected> {
    if !v.upheld {
        return None;
    }
    match &s.nodes.get(&n)?.life {
        Life::Closed(c) if c.upheld == Some(v.verdict) => Some(Corrected {
            node: n,
            was: c.outcome.clone(),
            outcome: c
                .corrected_from
                .clone()
                .unwrap_or_else(|| c.outcome.clone()),
        }),
        Life::Closed(_) | Life::Open => None,
    }
}

/// The outcome an upheld objection gives a closed node that had `was` (1.8, §5, §11.3): `done`
/// becomes `refuted` for a node of kind `claim` and `failed` otherwise; any other outcome stays.
#[must_use]
pub(crate) fn corrected_outcome(kind: Option<&crate::types::Label>, was: &Outcome) -> Outcome {
    match was {
        Outcome::Done => {
            if kind.is_some_and(|k| k.as_str() == crate::types::CLAIM) {
                crate::types::Label::new(crate::types::REFUTED)
                    .map_or(Outcome::Failed, Outcome::Other)
            } else {
                Outcome::Failed
            }
        }
        Outcome::Failed
        | Outcome::Dropped
        | Outcome::Superseded
        | Outcome::Missed
        | Outcome::Other(_) => was.clone(),
    }
}

/// Executes `talk resolve`: the `resolve` entry on the objection's node and `ObjectionResolved`.
/// 1.8 (§11.3): an upheld resolve on a closed node then corrects it (`NodeCorrected`), and the
/// reply names the correction. 1.10 (§11.3): a resolve that keeps a claim closed `done` with no
/// basis warns `no_basis`; a resolved objection with the same verdict changes nothing, and the
/// reply names that verdict; with the opposite verdict, the caller's text is a new open objection
/// (an `object` entry by the caller), then `CorrectionWithdrawn` when the earlier verdict's
/// correction still marks the node. In the lab's fifth round (l5), two reviewers given one review
/// reached opposite verdicts, and the later one was dropped with a bare `{already: true}`.
///
/// 1.11 (§10.3, §11.3): a deferral is a `resolve` entry that decides nothing (`deferred`), which
/// marks the open objection; on a closed node, the review, it records the caller's refusal of the
/// node (`Refused`, as a `cannot` release records one), so that `next`, `hello`'s offers, rule 13,
/// and `leave_better` keep its class off the review until the backoff ends. A verdict on a closed
/// node, a resolve or a second verdict's objection, clears the node's refusals
/// (`RefusalsCleared`): the review of a new objection, or the node's other flags, reach every class
/// again. In the lab's sixth round (l5), every reviewer found a review that neither held nor fell,
/// and `next` and rule 13 gave it back, to its own author too: 11.5 KB of the run's 54.5 KB.
pub(crate) fn resolve_exec(ex: &mut Exec<'_>, plan: ResolvePlan) -> Reply {
    let by = ex.author();
    match plan.what {
        ResolveWhat::Already(v) => Reply::ObjectionResolved {
            entry: None,
            corrected: None,
            already: Some(v),
            disputed: None,
            withdrawn: None,
            deferred: None,
        },
        ResolveWhat::Defer { node: n, closed } => {
            let entry = ex.append(
                n,
                by,
                EntryBody::Resolve {
                    objection: plan.objection,
                    upheld: false,
                    deferred: true,
                },
                main(&plan.note),
            );
            let mut until = None;
            if closed
                && let Some(sess) = ex.sess()
                && let Some(class) = ex.s().sessions.get(&sess).map(|x| x.class.clone())
            {
                let now = ex.now();
                let p = &ex.s().params;
                let (backoff_until, expires_at, quorum) = (
                    now.plus(p.refusal_backoff),
                    now.plus(p.refusal_ttl),
                    p.refusal_quorum,
                );
                ex.emit(Ev::Refused {
                    node: n,
                    class,
                    session: sess,
                    backoff_until,
                    expires_at,
                    quorum,
                });
                until = Some(backoff_until);
            }
            Reply::ObjectionResolved {
                entry: Some(entry),
                corrected: None,
                already: None,
                disputed: None,
                withdrawn: None,
                deferred: Some(Deferred { until }),
            }
        }
        ResolveWhat::Dispute {
            at,
            earlier,
            withdraw,
        } => {
            let objection = ex.append(at, by, EntryBody::Object, main(&plan.note));
            if let Some(w) = &withdraw {
                ex.emit(Ev::CorrectionWithdrawn {
                    node: w.node,
                    outcome: w.outcome.clone(),
                });
            }
            clear_refusals(ex, at, objection);
            Reply::ObjectionResolved {
                entry: Some(objection),
                corrected: None,
                already: None,
                disputed: Some(Disputed {
                    objection,
                    node: at,
                    disputes: plan.objection,
                    earlier,
                }),
                withdrawn: withdraw,
                deferred: None,
            }
        }
        ResolveWhat::Resolve { node: n, no_basis } => {
            let entry = ex.append(
                n,
                by,
                EntryBody::Resolve {
                    objection: plan.objection,
                    upheld: plan.upheld,
                    deferred: false,
                },
                main(&plan.note),
            );
            ex.emit(Ev::ObjectionResolved {
                o: plan.objection,
                by: entry,
                upheld: plan.upheld,
            });
            let mut corrected = None;
            if plan.upheld
                && let Some(node) = ex.s().nodes.get(&n)
                && let Life::Closed(c) = &node.life
            {
                let was = c.outcome.clone();
                let outcome = corrected_outcome(node.kind.as_ref(), &was);
                // 1.10: what a withdrawal restores, the outcome it was closed with, which an
                // earlier correction that still marks it kept.
                let restore = if c.upheld.is_some() {
                    c.corrected_from.clone()
                } else {
                    Some(was.clone())
                };
                ex.emit(Ev::NodeCorrected {
                    node: n,
                    outcome: outcome.clone(),
                    by: entry,
                    was: restore,
                });
                corrected = Some(Corrected {
                    node: n,
                    was,
                    outcome,
                });
            }
            clear_refusals(ex, n, entry);
            if let Some(decision) = no_basis {
                ex.warn(Warning {
                    code: WarningCode::NoBasis,
                    id: Some(n),
                    detail: WarningDetail::NoBasis { node: n, decision },
                });
            }
            Reply::ObjectionResolved {
                entry: Some(entry),
                corrected,
                already: None,
                disputed: None,
                withdrawn: None,
                deferred: None,
            }
        }
    }
}

/// 1.11 (§10.3, §11.3): a verdict recorded on closed node `n` by entry `by` ends the backoffs that
/// deferrals of its review set: `RefusalsCleared` when `n` is closed and has refusal records.
fn clear_refusals(ex: &mut Exec<'_>, n: NodeId, by: Eid) {
    let closed = ex.s().nodes.get(&n).is_some_and(|x| !x.is_open());
    if closed && ex.s().ix.refused.contains_key(&n) {
        ex.emit(Ev::RefusalsCleared { node: n, by });
    }
}

// ------------------------------------------------------------------------------------------------
// retract

/// The plan of `talk retract` (D.8.17).
#[derive(Debug)]
pub(crate) struct RetractPlan {
    target: BasisTarget,
    node: NodeId,
    text: EntryText,
}

/// Validates `talk retract {entry | path, text, reason}` (D.8.17, D.8.24's row, D.14.6): a path
/// resolves (`not_found(Node)`); an entry lies below `next_eid` and the node the shell added from
/// its `entries` table exists (`not_found(Entry)`: an entry the table lacks reaches the core with
/// the shell's `UNKNOWN_NODE` and is refused here, after O1 and the session check).
pub(crate) fn retract(cx: &Vcx<'_>, cmd: &Cmd) -> Result<RetractPlan, Error> {
    let Cmd::Talk(TalkCmd::Retract {
        target: arg,
        text,
        reason,
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    let s = cx.s;
    let path = target(s, cmd)?;
    let (t, node) = match arg {
        RetractArg::Entry { eid, node } => {
            if *eid >= s.counters.next_eid || !s.nodes.contains_key(node) {
                return Err(Error::not_found(What::Entry));
            }
            (BasisTarget::Entry(*eid), *node)
        }
        RetractArg::Node(_) => {
            let n = path.ok_or(Error::not_found(What::Node))?;
            (BasisTarget::Node(n), n)
        }
    };
    Ok(RetractPlan {
        target: t,
        node,
        text: EntryText {
            main: text.clone(),
            extra: alloc::vec![(TextField::Reason, reason.clone())],
        },
    })
}

/// Executes `talk retract` (D.8.17, D-4): the `retract` entry on the target's node, which puts the
/// target into `State::retracted`, so every decision that cites it directly gets `basis_changed`;
/// an active decision retracted is `DecisionRetracted`. A superseded decision keeps its state
/// (§25: no superseded → retracted), and a settled request stays settled (A-11). 1.12 (§9.3): an
/// open question to a person is withdrawn (`withdraw_pq`).
pub(crate) fn retract_exec(ex: &mut Exec<'_>, plan: RetractPlan) -> Reply {
    let by = ex.author();
    let entry = ex.append(
        plan.node,
        by,
        EntryBody::Retract {
            target: plan.target,
        },
        plan.text,
    );
    if let BasisTarget::Entry(d) = plan.target
        && ex
            .s()
            .decisions
            .get(&d)
            .is_some_and(|x| x.state == DecisionState::Active)
    {
        // 1.11 (§11.3): the decision's node keeps it as its newest retracted decision.
        let node = ex.s().decisions.get(&d).map(|x| x.node);
        ex.emit(Ev::DecisionRetracted { d, by: entry, node });
    }
    let withdrawn = match plan.target {
        BasisTarget::Entry(q) => {
            crate::helpers::conversation::withdraw_pq(ex, q, entry).then_some(q)
        }
        BasisTarget::Node(_) => None,
    };
    Reply::Retracted { entry, withdrawn }
}

#[cfg(test)]
#[path = "review_tests.rs"]
mod tests;
