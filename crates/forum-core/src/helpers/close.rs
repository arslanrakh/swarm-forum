//! Closing and recording assumptions (design D.8.17, D.8.18).

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;

use super::Exec;
use super::recurring::open_next_occurrence;
use crate::event::{EndCause, EntryBody, EntryText, Ev, HoldEnd, IntentEnd, SystemNote};
use crate::state::{Author, Life, PqState, Role, UqState};
use crate::types::{
    DecisionId, LinkRel, Mark, NodeId, Outcome, QuestionId, SignalKind, Text, Time, USER_DECISIONS,
};

/// How `close_node` closes a node: the arguments of D.8.18's `close_node(n, outcome, by,
/// merged_into, cause, text)`, and the `reslot` of its step 7.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Closing {
    /// The outcome.
    pub outcome: Outcome,
    /// Who closes it: a session, or the server.
    pub by: Author,
    /// The node it is merged into (K5: only with `Superseded`).
    pub merged_into: Option<NodeId>,
    /// What ends its holds.
    pub cause: EndCause,
    /// The text of its `System(Closed)` entry: summary, verified, learnings.
    pub text: EntryText,
    /// `Some(now)` only when `update {repeat}` drops a waiting occurrence, so the next occurrence
    /// follows the new schedule from now (D.8.18 step 7).
    pub reslot: Option<Time>,
}

/// `close_node` (D.8.18), the one helper every close goes through (agent close, merge, missed,
/// refusal drop, series drop, signal resolution, a schedule that ended):
/// 0. `record_assumptions`;
/// 1. `Appended(System(Closed))` on `n`;
/// 2. `HoldEnded { EndedByClose(cause) }` for each live hold on `n`;
/// 3. `IntentClosed { ResolvedByClose }` if the open intent is the closing session's;
/// 4. the recheck marks off;
/// 5. `NodeClosed`;
/// 6. a series closes its open occurrence as `dropped` (A-14);
/// 7. an occurrence of an open series records `SeriesOutcome` and opens the next occurrence;
/// 8. a signal node resolves its signal;
/// 9. (1.11) a close other than a merge resolves the `duplicate` signals whose other node is closed
///    and was not merged.
///
/// Returns the open occurrence that step 6 dropped, which `node close` and `node merge` report with
/// the `series_stopped` warning (1.4, §12, R2, D.8.20); None for any other node. A node that does
/// not exist or is closed already is left alone.
pub fn close_node(ex: &mut Exec<'_>, n: NodeId, how: Closing) -> Option<NodeId> {
    if !ex
        .s()
        .nodes
        .get(&n)
        .is_some_and(crate::state::Node::is_open)
    {
        return None;
    }
    // 0.
    record_assumptions(ex, n, how.by, &how.text);
    // 1.
    ex.append(
        n,
        how.by,
        EntryBody::System(SystemNote::Closed {
            outcome: how.outcome.clone(),
        }),
        how.text.clone(),
    );
    // 2.
    let now = ex.now();
    let holds: Vec<_> = ex
        .s()
        .holds
        .iter()
        .filter(|(t, h)| ends_on_close(t, h, n, now))
        .map(|(t, _)| *t)
        .collect();
    for token in holds {
        ex.emit(Ev::HoldEnded {
            token,
            end: HoldEnd::EndedByClose(how.cause),
        });
    }
    // 3.
    if let (Some(intent), Author::Session(x)) = (
        ex.s().nodes.get(&n).and_then(|node| node.open_intent),
        how.by,
    ) && intent.session == x
    {
        ex.emit(Ev::IntentClosed {
            node: n,
            end: IntentEnd::ResolvedByClose,
        });
    }
    // 4.
    let marks: Vec<QuestionId> = ex
        .s()
        .nodes
        .get(&n)
        .map(|node| node.recheck_marks.iter().copied().collect())
        .unwrap_or_default();
    for q in marks {
        ex.emit(Ev::MarkSet {
            node: n,
            mark: Mark::Recheck,
            question: q,
            on: false,
        });
    }
    // 5.
    let role = ex.s().nodes.get(&n).map(|node| node.role.clone());
    ex.emit(Ev::NodeClosed {
        node: n,
        outcome: how.outcome.clone(),
        merged_into: how.merged_into,
        // 1.11 (§13): who closed it, which a note without `to` at the closed node reaches.
        by: match how.by {
            Author::Session(x) => Some(x),
            Author::Server => None,
        },
    });
    let mut dropped = None;
    match role {
        // 6.
        Some(Role::Series(_)) => {
            let open: Vec<NodeId> = ex
                .s()
                .ix
                .series_open
                .get(&n)
                .into_iter()
                .flatten()
                .copied()
                .collect();
            for o in open {
                close_node(
                    ex,
                    o,
                    Closing {
                        outcome: Outcome::Dropped,
                        by: how.by,
                        merged_into: None,
                        cause: EndCause::Close,
                        text: how.text.clone(),
                        reslot: None,
                    },
                );
                // K6: a series has at most one open occurrence; the first is the one reported.
                dropped = dropped.or(Some(o));
            }
        }
        // 7.
        Some(Role::Occurrence(o)) => {
            let series_open =
                ex.s().nodes.get(&o.series).is_some_and(|x| {
                    matches!(x.life, Life::Open) && matches!(x.role, Role::Series(_))
                });
            if series_open {
                ex.emit(Ev::SeriesOutcome {
                    series: o.series,
                    missed: how.outcome == Outcome::Missed,
                });
                let after = how.reslot.unwrap_or_else(|| o.slot.max(now));
                open_next_occurrence(ex, o.series, after);
            }
        }
        // 8.
        Some(Role::Signal(key)) => {
            if ex.s().signals.get(&key) == Some(&n) {
                ex.emit(Ev::SignalResolved { key });
            }
        }
        Some(Role::Plain) | None => {}
    }
    // 9. (1.11, §14)
    if how.merged_into.is_none() {
        resolve_closed_duplicates(ex, n);
    }
    dropped
}

/// Step 9 of `close_node` (1.11, §14): a close of `n` other than a merge resolves each open
/// `duplicate` signal whose pair holds `n` and whose other node is closed and was not merged: the
/// server closes the signal node as `done`, and step 8 of that close resolves the signal. The
/// signals are found through `n`'s incoming `duplicate` links, which every signal node of its pairs
/// carries (§14), so the cost is those links. A merge into a third node leaves the signal open, as
/// before, and so does a later close of the other node.
fn resolve_closed_duplicates(ex: &mut Exec<'_>, n: NodeId) {
    let s = ex.s();
    let range = (LinkRel::Duplicate, NodeId(0))..=(LinkRel::Duplicate, NodeId(u64::MAX));
    let signals: Vec<NodeId> =
        s.ix.backlinks
            .get(&n)
            .into_iter()
            .flat_map(|xs| xs.range(range.clone()))
            .map(|&(_, y)| y)
            .filter(|y| {
                let Some(node) = s.nodes.get(y) else {
                    return false;
                };
                let Role::Signal(key) = &node.role else {
                    return false;
                };
                let other = if key.a == n { key.b } else { key.a };
                key.kind == SignalKind::Duplicate
                    && (key.a == n || key.b == n)
                    && node.is_open()
                    && s.signals.get(key) == Some(y)
                    && s.nodes
                        .get(&other)
                        .is_some_and(|o| !o.is_open() && o.merged_into().is_none())
            })
            .collect();
    for y in signals {
        close_node(
            ex,
            y,
            Closing {
                outcome: Outcome::Done,
                by: Author::Server,
                merged_into: None,
                cause: EndCause::Close,
                text: EntryText {
                    main: Text(String::from(BOTH_CLOSED)),
                    extra: Vec::new(),
                },
                reslot: None,
            },
        );
    }
}

/// The text of the `System(Closed)` entry of a duplicate signal that step 9 resolves (1.11).
pub const BOTH_CLOSED: &str = "both nodes of the pair are closed";

/// Step 2 of `close_node`: the holds a close of `n` at `now` ends, its live holds (H6: on a
/// settled state every hold in the map is live, T3, so a close leaves none). Pure, for the Kani
/// harness `close_ends_holds`.
#[must_use]
pub(crate) fn ends_on_close(
    t: &crate::types::Token,
    h: &crate::state::Hold,
    n: NodeId,
    now: Time,
) -> bool {
    t.node == n && now < h.lease_until
}

/// Step 3 of `talk decide` (D.8.17) for the questions `qs` and decision `d` on node `n`:
/// `PqAssumed` for a people question still `Expired` with `assumed_by = None`, or `UqAssumed` for a
/// user question still `Lapsed` with `assumed_by = None` (CS-2, D.14.5), and the assume mark off.
pub(crate) fn assume_marks_off(
    ex: &mut Exec<'_>,
    n: NodeId,
    d: DecisionId,
    qs: &BTreeSet<QuestionId>,
) {
    for &q in qs {
        assumed(ex, q, d);
        ex.emit(Ev::MarkSet {
            node: n,
            mark: Mark::Assume,
            question: q,
            on: false,
        });
    }
}

/// `PqAssumed` or `UqAssumed` for `q` and `d`, when the question still awaits its assumption.
fn assumed(ex: &mut Exec<'_>, q: QuestionId, d: DecisionId) {
    let s = ex.s();
    if s.pquestions
        .get(&q)
        .is_some_and(|pq| pq.state == PqState::Expired && pq.assumed_by.is_none())
    {
        ex.emit(Ev::PqAssumed { q, decision: d });
    } else if s
        .uquestions
        .get(&q)
        .is_some_and(|uq| uq.state == UqState::Lapsed && uq.assumed_by.is_none())
    {
        ex.emit(Ev::UqAssumed { q, decision: d });
    }
}

/// A decision entry with no settles, no supersession, and no basis, recording `assumes`.
fn assumption_decision(assumes: BTreeSet<QuestionId>) -> EntryBody {
    EntryBody::Decision {
        settles: BTreeSet::new(),
        supersedes: None,
        assumes,
        evidence: None,
        basis: Vec::new(),
    }
}

/// `record_assumptions(n, by, text)` (D.8.17, A-104): before a close of a node with assume marks,
/// a decision that records them, with `PqAssumed` or `UqAssumed` for each question still `Expired`
/// or `Lapsed` and the marks off. Returns the decision, if it recorded one.
pub fn record_assumptions(
    ex: &mut Exec<'_>,
    n: NodeId,
    by: Author,
    text: &EntryText,
) -> Option<DecisionId> {
    let marks = ex
        .s()
        .nodes
        .get(&n)
        .map(|node| node.assume_marks.clone())
        .unwrap_or_default();
    if marks.is_empty() {
        return None;
    }
    let d = ex.append(n, by, assumption_decision(marks.clone()), text.clone());
    assume_marks_off(ex, n, d, &marks);
    Some(d)
}

/// `assume_now(q)` (D.8.17, CS-2): a question that expires or lapses with no open node to mark has
/// its assumption recorded at once, by the server, on its home node: a people question's node; a
/// user question's `about` if it is a node that is not reserved, else `@user/decisions`. A question
/// that is not resident, or no longer awaits its assumption, records nothing.
pub fn assume_now(ex: &mut Exec<'_>, q: QuestionId) -> Option<DecisionId> {
    let s = ex.s();
    let (home, text) = if let Some(pq) = s.pquestions.get(&q) {
        if pq.state != PqState::Expired || pq.assumed_by.is_some() {
            return None;
        }
        (
            pq.node,
            "the node was closed before the question expired; the assumption stands",
        )
    } else if let Some(uq) = s.uquestions.get(&q) {
        if uq.state != UqState::Lapsed || uq.assumed_by.is_some() {
            return None;
        }
        let home = uq
            .about
            .filter(|a| !a.is_reserved() && s.nodes.contains_key(a))
            .unwrap_or(USER_DECISIONS);
        (
            home,
            "nobody answered by the last due; the assumption stands",
        )
    } else {
        return None;
    };
    let d = ex.append(
        home,
        Author::Server,
        assumption_decision([q].into_iter().collect()),
        EntryText {
            main: Text(String::from(text)),
            extra: Vec::new(),
        },
    );
    assumed(ex, q, d);
    Some(d)
}
