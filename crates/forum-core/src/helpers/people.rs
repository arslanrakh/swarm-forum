//! Questions to people: the Recheck firing (design D.8.2, §9.3, A-24, A-81, A-93, A-104).
//! Body: WP-D5b.

use alloc::vec::Vec;

use super::Exec;
use super::close::assume_now;
use super::user::{NewUq, add_digest, audience, create_uq};
use crate::event::{EntryText, Ev};
use crate::state::{DigestItem, PqState, UqSource, WaitKey, WaitOn};
use crate::types::{Mark, NodeId, QuestionId};

/// The nodes whose wait on people question `q` the recheck that brings it past `rechecks`
/// resolves: those waiting on `Question { q, rechecks_at_set = rechecks }`, ascending (D.8.2).
#[must_use]
pub(crate) fn resolving(s: &crate::state::State, q: QuestionId, rechecks: u32) -> Vec<NodeId> {
    let want = WaitOn::Question {
        q,
        rechecks_at_set: rechecks,
    };
    s.ix.waiters
        .get(&WaitKey::Question(q))
        .into_iter()
        .flatten()
        .copied()
        .filter(|w| {
            s.nodes
                .get(w)
                .and_then(|x| x.wait.as_ref())
                .is_some_and(|wt| wt.on == want)
        })
        .collect()
}

/// The Recheck firing of people question `q` at `ex.now()` (D.8.2's row; §9.3):
/// - `resolving`: the nodes whose wait this recheck resolves; `marked`: those, or the question's
///   conversation node when none and it is open (A-81), else none;
/// - a recheck that is not the last: `PqRechecked { q, k + 1, at + question.recheck }`, then the
///   `question_recheck` mark on each node of `marked`;
/// - the last, for a non-blocking question: `PqExpired`, the `Assumption` digest entries for its
///   audience (read before the expiry), then the `proceed_on_assumption` mark on each node of
///   `marked`, or, with none, `assume_now` (A-104);
/// - the last, for a blocking question: the user question of its escalation (`create_uq`, no
///   default, A-59), `PqEscalated`, and the nodes of `resolving` wait on that user question
///   instead; no mark is set (A-93).
///
/// A question that is not resident or no longer open is left alone: its key is gone then (T4).
pub fn recheck(ex: &mut Exec<'_>, q: QuestionId) {
    let s = ex.s();
    let Some(pq) = s.pquestions.get(&q) else {
        return;
    };
    if pq.state != PqState::Open {
        return;
    }
    let (node, blocking, rechecks) = (pq.node, pq.blocking, pq.rechecks);
    let resolving = resolving(s, q, rechecks);
    let node_open = s.nodes.get(&node).is_some_and(crate::state::Node::is_open);
    let marked: Vec<NodeId> = if !resolving.is_empty() {
        resolving.clone()
    } else if node_open {
        alloc::vec![node]
    } else {
        Vec::new()
    };
    let next = rechecks.saturating_add(1);
    let at = ex.now();
    if next < s.params.question_max_rechecks {
        let recheck_at = at.plus(s.params.question_recheck);
        ex.emit(Ev::PqRechecked {
            q,
            rechecks: next,
            next: recheck_at,
        });
        for m in marked {
            ex.emit(Ev::MarkSet {
                node: m,
                mark: Mark::Recheck,
                question: q,
                on: true,
            });
        }
    } else if !blocking {
        let item = DigestItem::Assumption { question: q };
        let aud = audience(s, &item);
        ex.emit(Ev::PqExpired { q });
        add_digest(ex, item, &aud);
        if marked.is_empty() {
            assume_now(ex, q);
        } else {
            for m in marked {
                ex.emit(Ev::MarkSet {
                    node: m,
                    mark: Mark::Assume,
                    question: q,
                    on: true,
                });
            }
        }
    } else {
        let uq = create_uq(
            ex,
            NewUq {
                source: UqSource::Escalation { question: q },
                about: Some(node),
                has_default: false,
                explicit_to: None,
                explicit_due: None,
                text: EntryText::default(),
            },
        );
        ex.emit(Ev::PqEscalated { q, uq });
        for w in resolving {
            ex.emit(Ev::WaitSet {
                node: w,
                wait: Some(WaitOn::UserQuestion(uq)),
            });
        }
    }
}
