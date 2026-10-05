//! The Recheck row of D.8.2's table for `check_fire` (D.9.1: "the events are exactly the table's"):
//! WP-D5b's firing arm, recomputed naively from the state before the firing (§9.3, A-24, A-81,
//! A-93, A-104). No code of `helpers::people` is shared: the resolving waits are found by a scan of
//! `nodes`, not through `ix.waiters`.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::event::{EntryBody, Ev, QuestionBody, SystemNote};
use crate::spec;
use crate::state::{Author, DigestItem, Life, PqState, State, UqSource, WaitOn};
use crate::types::{Mark, NodeId, QuestionId, Time, USER_DECISIONS, USER_DIGEST};

/// `rest` (the events after `Fired`) are exactly those of the Recheck row for people question `q`
/// at `at`:
/// - `resolving`: the nodes waiting on `Question { q, rechecks_at_set = q.rechecks }`; `marked`:
///   those, else `{q.node}` if it is open, else none; `k' = q.rechecks + 1`;
/// - not the last (`k' < question.max_rechecks`): `PqRechecked { q, k', at + question.recheck }`,
///   then `MarkSet { m, Recheck, q, on }` for each `m` of `marked`, ascending;
/// - the last, non-blocking: `PqExpired { q }`, the `Assumption` digest entries on `USER_DIGEST`
///   for `audience` read before the firing, ascending, then `MarkSet { m, Assume, q, on }` for
///   each `m` of `marked`, or, with none, `assume_now`'s server decision on `q.node` assuming `q`
///   and its `PqAssumed`;
/// - the last, blocking: `create_uq`'s `Appended(Question(User { Escalation { q }, about: q.node,
///   has_default: false, .. }))` on `USER_DECISIONS`, `PqEscalated { q, uq }`, then `WaitSet { w,
///   UserQuestion(uq) }` for each `w` of `resolving`, ascending, and no mark (A-93).
///
/// A question that is not resident or not open has no Recheck key (T4), and its firing emits
/// nothing.
pub(super) fn recheck(before: &State, q: QuestionId, at: Time, rest: &[&Ev]) -> bool {
    let Some(pq) = before.pquestions.get(&q) else {
        return rest.is_empty();
    };
    if pq.state != PqState::Open {
        return rest.is_empty();
    }
    let wait = WaitOn::Question {
        q,
        rechecks_at_set: pq.rechecks,
    };
    let resolving: Vec<NodeId> = before
        .nodes
        .iter()
        .filter(|(_, x)| x.wait.as_ref().is_some_and(|w| w.on == wait))
        .map(|(id, _)| *id)
        .collect();
    let open = before
        .nodes
        .get(&pq.node)
        .is_some_and(|x| matches!(x.life, Life::Open));
    let marked: Vec<NodeId> = if !resolving.is_empty() {
        resolving.clone()
    } else if open {
        alloc::vec![pq.node]
    } else {
        Vec::new()
    };
    let p = &before.params;
    let next = pq.rechecks.saturating_add(1);
    let marks = |mark: Mark| -> Vec<Ev> {
        marked
            .iter()
            .map(|m| Ev::MarkSet {
                node: *m,
                mark,
                question: q,
                on: true,
            })
            .collect()
    };
    if next < p.question_max_rechecks {
        let mut want = alloc::vec![Ev::PqRechecked {
            q,
            rechecks: next,
            next: at.plus(p.question_recheck),
        }];
        want.extend(marks(Mark::Recheck));
        return same(rest, &want);
    }
    if !pq.blocking {
        let Some((first, tail)) = rest.split_first() else {
            return false;
        };
        if **first != (Ev::PqExpired { q }) {
            return false;
        }
        let item = DigestItem::Assumption { question: q };
        let audience: Vec<_> = spec::audience(before, &item).into_iter().collect();
        let digest = tail.len().min(audience.len());
        let (entries, tail) = tail.split_at(digest);
        let digest_ok = digest == audience.len()
            && entries.iter().zip(&audience).all(|(e, a)| {
                matches!(e, Ev::Appended {
                    node,
                    body: EntryBody::System(SystemNote::Digest { item: i, to }),
                    ..
                } if *node == USER_DIGEST && *i == item && to == a)
            });
        if !digest_ok {
            return false;
        }
        if !marked.is_empty() {
            return same(tail, &marks(Mark::Assume));
        }
        // `assume_now(q)`: nothing open to mark (A-104).
        return match tail {
            [
                Ev::Appended {
                    eid,
                    node,
                    by: Author::Server,
                    body:
                        EntryBody::Decision {
                            settles,
                            supersedes: None,
                            assumes,
                            evidence: None,
                            basis,
                        },
                    ..
                },
                Ev::PqAssumed { q: x, decision },
            ] => {
                *node == pq.node
                    && settles.is_empty()
                    && *assumes == BTreeSet::from([q])
                    && basis.is_empty()
                    && *x == q
                    && decision == eid
            }
            _ => false,
        };
    }
    // Blocking: the escalation (A-59, A-93).
    let [
        Ev::Appended {
            eid: uq,
            node,
            body:
                EntryBody::Question(QuestionBody::User {
                    source: UqSource::Escalation { question },
                    about,
                    has_default: false,
                    ..
                }),
            ..
        },
        Ev::PqEscalated { q: x, uq: y },
        waits @ ..,
    ] = rest
    else {
        return false;
    };
    let want: Vec<Ev> = resolving
        .iter()
        .map(|w| Ev::WaitSet {
            node: *w,
            wait: Some(WaitOn::UserQuestion(*uq)),
        })
        .collect();
    *node == USER_DECISIONS
        && *question == q
        && *about == Some(pq.node)
        && *x == q
        && y == uq
        && same(waits, &want)
}

/// `got` equals `want`, event by event.
fn same(got: &[&Ev], want: &[Ev]) -> bool {
    got.len() == want.len() && got.iter().zip(want).all(|(g, w)| *g == w)
}
