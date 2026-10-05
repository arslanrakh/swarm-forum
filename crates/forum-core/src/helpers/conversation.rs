//! Conversations, requests, and their answers (design D.8.8, D.8.11, D.8.15). Bodies: WP-D4.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use super::Exec;
use crate::error::{Error, Why};
use crate::event::{EntryBody, EntryText, Ev};
use crate::state::{PqState, Routing, SaidState, State, UqState};
use crate::types::{
    Eid, Key, KeyStr, Mark, NodeId, NormVer, QuestionId, RequestId, Requester, SaidId, SaidKind,
    UserEntryId,
};

/// Where an entry about conversation `k` lies (C1): `node`; `add_key` when it is case 3 of D.8.8,
/// so execution emits `KeyAdded { node, k }` before recording the entry.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct ConvNode {
    /// The conversation node.
    pub node: NodeId,
    /// No node carries `k`: it is added to `node` first.
    pub add_key: bool,
}

/// `n` and its ancestors, nearest first: at most `nodes.len()` steps (D.8.4).
fn up_from(s: &State, n: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    core::iter::once(n).chain(crate::index::ancestors(s, n))
}

/// `conv_key(s, at, where_)` (D.8.8): `where_` if given; otherwise the one conversation key of the
/// nearest node from `at` up to ROOT that carries any (`KeyStr::is_conversation` with
/// `params.conversation_schemes`, CS-36), with its normaliser version. A `repeat:` `where_` is the
/// caller's to refuse (`invalid(ReservedKey)`, A-90).
///
/// # Errors
///
/// `invalid(AmbiguousConversationKey)` when the nearest such node carries several;
/// `invalid(NoConversationKey)` when no node does (A-30).
pub fn conv_key(s: &State, at: NodeId, where_: Option<&Key>) -> Result<Key, Error> {
    if let Some(k) = where_ {
        return Ok(k.clone());
    }
    let schemes = &s.params.conversation_schemes;
    for m in up_from(s, at) {
        let Some(node) = s.nodes.get(&m) else {
            continue;
        };
        let mut convs = node.keys.iter().filter(|(k, _)| k.is_conversation(schemes));
        if let Some((k, &nv)) = convs.next() {
            if convs.next().is_some() {
                return Err(Error::invalid(Why::AmbiguousConversationKey));
            }
            return Ok(Key { key: k.clone(), nv });
        }
    }
    Err(Error::invalid(Why::NoConversationKey))
}

/// `conv_node(s, at, k)` (D.8.8, §9.1): the nearest node from `at` up to ROOT whose keys contain
/// `k` (open or closed); else `ix.keys_open[k]`; else `at`, with `add_key`.
///
/// # Errors
///
/// `invalid(ReservedNode)` when the result is a reserved node (A-29): only case 3 with a reserved
/// `at` can yield one, since no key can be added to a reserved node (K8).
pub fn conv_node(s: &State, at: NodeId, k: &KeyStr) -> Result<ConvNode, Error> {
    let found = up_from(s, at)
        .find(|m| s.nodes.get(m).is_some_and(|x| x.keys.contains_key(k)))
        .map(|node| ConvNode {
            node,
            add_key: false,
        })
        .or_else(|| {
            s.ix.keys_open.get(k).map(|&node| ConvNode {
                node,
                add_key: false,
            })
        })
        .unwrap_or(ConvNode {
            node: at,
            add_key: true,
        });
    if found.node.is_reserved() {
        return Err(Error::invalid(Why::ReservedNode));
    }
    Ok(found)
}

/// `record_request(c, origin, conv, from_user)` (D.8.8): if `ix.origins[origin.key] = r`, the
/// request exists and nothing is emitted (Q1); otherwise `Appended(Request { origin, conv,
/// from_user, requester })` on `c`, with `requester = User(p)` for the principal of the entry
/// `from_user`, and `External` without one (CS-4). `text` is the entry's text (the request, and the
/// requester as `TextField::Requester`). Returns the request and whether it existed (1.4: the
/// commands name a request they recorded in their replies, R3).
///
/// A request whose origin is a user's message is a user request whoever records it (§9.4: "A
/// user's message is `user:<entry>`", and "the requester of a user request is its entry's
/// principal"; §26 C1: "a request from a `user:` entry has no conversation"; `Request::from_user`
/// is "set when the origin is `user:e<entry>`"): with `from_user` not given, an origin `user:e<n>`
/// that names a resident user entry gives `from_user = e<n>`, so `node create {request}` records
/// such a request as the entry's in both branches. An unrouted entry is then routed to `c`, the
/// request's node (§9.4: "an unrouted entry is routed there"; D.8.15 does the same for `talk
/// request`), so that no later routing of the entry names a node its request is not on.
pub fn record_request(
    ex: &mut Exec<'_>,
    c: NodeId,
    origin: Key,
    conv: KeyStr,
    from_user: Option<UserEntryId>,
    text: EntryText,
) -> (RequestId, bool) {
    if let Some(&r) = ex.s().ix.origins.get(&origin.key) {
        return (r, true);
    }
    // A routed entry is unrouted until this call, so it is resident (D.4.12): its principal is
    // known. An entry that is not resident cannot be routed, and the request is then External.
    let s = ex.s();
    let from_user = from_user.or_else(|| {
        origin
            .key
            .user_entry()
            .filter(|e| s.user_entries.contains_key(e))
    });
    let entry = from_user.and_then(|e| s.user_entries.get(&e));
    let requester = entry.map_or(Requester::External, |u| Requester::User(u.principal));
    let unrouted = entry.is_some_and(|u| u.routing == Routing::Unrouted);
    let by = ex.author();
    let r = ex.append(
        c,
        by,
        EntryBody::Request {
            origin,
            conv,
            from_user,
            requester,
        },
        text,
    );
    if let Some(e) = from_user
        && unrouted
    {
        ex.emit(Ev::EntryRouted {
            entry: e,
            routing: Routing::Request {
                node: c,
                request: r,
            },
            reason: None,
        });
    }
    (r, false)
}

/// `try_settle(r)` (D.8.8): if `r` is unsettled, answered, and listed by a decision that is not
/// retracted (`ix.settling[r]`, superseded decisions included), `RequestSettled { r, decision: min
/// ix.settling[r], answer }`. Returns whether it settled. Settling is final (A-11): a settled
/// request is left alone.
pub fn try_settle(ex: &mut Exec<'_>, r: RequestId) -> bool {
    let s = ex.s();
    let Some(req) = s.requests.get(&r) else {
        return false;
    };
    let (None, Some(answer)) = (req.settled, req.answered_by) else {
        return false;
    };
    let Some(&decision) = s.ix.settling.get(&r).and_then(|ds| ds.first()) else {
        return false;
    };
    ex.emit(Ev::RequestSettled {
        request: r,
        decision,
        answer,
    });
    true
}

/// The marks of question `q` on the nodes `ix.marked_by_question[q]` names: `(node, mark)`, the
/// recheck mark before the assume mark of each node, nodes ascending.
fn marks_of(s: &State, q: QuestionId, only: Option<Mark>) -> Vec<(NodeId, Mark)> {
    let mut out = Vec::new();
    for &w in s.ix.marked_by_question.get(&q).into_iter().flatten() {
        let Some(node) = s.nodes.get(&w) else {
            continue;
        };
        if only != Some(Mark::Assume) && node.recheck_marks.contains(&q) {
            out.push((w, Mark::Recheck));
        }
        if only != Some(Mark::Recheck) && node.assume_marks.contains(&q) {
            out.push((w, Mark::Assume));
        }
    }
    out
}

/// 1.12 (§9.3): `withdraw_pq(q, by)`, a `talk retract` of open people question `q` by entry `by`:
/// `PqWithdrawn`, then the recheck marks it set off. The waits on it resolve with its state (§10.3),
/// and its Recheck deadline leaves the queue (T4), so it never marks, expires, or escalates. A
/// question that is not resident or not open is left alone, and the retract changes nothing else.
/// Returns whether it withdrew it. In the lab's seventh round (it6 l1), a blocking question put to
/// the wrong person could not be withdrawn, and would have escalated beside the right one's.
pub fn withdraw_pq(ex: &mut Exec<'_>, q: QuestionId, by: Eid) -> bool {
    if ex
        .s()
        .pquestions
        .get(&q)
        .is_none_or(|pq| pq.state != PqState::Open)
    {
        return false;
    }
    ex.emit(Ev::PqWithdrawn { q, by });
    for (node, mark) in marks_of(ex.s(), q, None) {
        ex.emit(Ev::MarkSet {
            node,
            mark,
            question: q,
            on: false,
        });
    }
    true
}

/// `answer_pq(q, by)` (D.8.15): `PqAnswered`, the question's marks off, and, for an escalated
/// question, its user question answered while it is pending (A-60), or the marks of its lapse off
/// when it lapsed (CS-2). A question that is not resident, or that is answered, superseded, or
/// (1.12) withdrawn already, is left alone: an answer to it is recorded and changes no state (§9.3,
/// §25).
pub fn answer_pq(ex: &mut Exec<'_>, q: QuestionId, by: Eid) {
    let Some(state) = ex.s().pquestions.get(&q).map(|pq| pq.state) else {
        return;
    };
    let uq = match state {
        PqState::Open | PqState::Expired => None,
        PqState::Escalated { uq } => Some(uq),
        PqState::Answered { .. } | PqState::Superseded { .. } | PqState::Withdrawn { .. } => return,
    };
    ex.emit(Ev::PqAnswered { q, by });
    for (node, mark) in marks_of(ex.s(), q, None) {
        ex.emit(Ev::MarkSet {
            node,
            mark,
            question: q,
            on: false,
        });
    }
    let Some(uq) = uq else {
        return;
    };
    match ex.s().uquestions.get(&uq).map(|u| u.state) {
        Some(UqState::Pending) => ex.emit(Ev::UqAnswered { q: uq, by }),
        Some(UqState::Lapsed) => {
            for (node, mark) in marks_of(ex.s(), uq, Some(Mark::Assume)) {
                ex.emit(Ev::MarkSet {
                    node,
                    mark,
                    question: uq,
                    on: false,
                });
            }
        }
        Some(UqState::Answered { .. } | UqState::Defaulted) | None => {}
    }
}

/// `route_to(e, n)` (D.8.11): `KeyAdded { n, user:e<e> }` if no open node carries the key and `n`
/// does not carry it already (D.14.5: a closed `n` that got the key by `node key {add}`, or the
/// carrier a join just gave it); the request of `user:e<e>` on `n`, with the entry's principal as
/// its requester (CS-4); and `EntryRouted { e, Request { node: n, request } }`, which
/// `record_request` emits for the unrouted entry. Returns the request it recorded (1.4, R3: the
/// commands name it in their replies, `user_requests` and `user_request`), or None when it recorded
/// none. The request's entry carries no text: the core never holds feed texts (D.0.2, principle 6),
/// and the shell renders a routed request from its user entry. An entry that is not unrouted (or not
/// resident) is left alone, and None is returned (callers route unrouted entries only).
///
/// When a request of the message `user:e<e>` exists already (Q1: one recorded while the entry was
/// not yet resident), the routing names that request on its own node, never `n`, so that a routing
/// never names a node its request is not on; nothing is recorded, and None is returned (D.8.26: "a
/// known origin records nothing").
pub fn route_to(ex: &mut Exec<'_>, e: UserEntryId, n: NodeId) -> Option<RequestId> {
    match ex.s().user_entries.get(&e).map(|u| u.routing) {
        Some(Routing::Unrouted) => {}
        Some(
            Routing::Request { .. }
            | Routing::Clarifies { .. }
            | Routing::Answers { .. }
            | Routing::NoAction,
        )
        | None => return None,
    }
    let key = Key {
        key: KeyStr::of_user_entry(e),
        nv: NormVer(0),
    };
    let s = ex.s();
    let carried = s.ix.keys_open.contains_key(&key.key)
        || s.nodes
            .get(&n)
            .is_some_and(|x| x.keys.contains_key(&key.key));
    if !carried {
        ex.emit(Ev::KeyAdded {
            node: n,
            key: key.clone(),
        });
    }
    let conv = key.key.clone();
    let (request, existed) = record_request(ex, n, key, conv, Some(e), EntryText::default());
    if existed {
        let node = ex.s().requests.get(&request).map_or(n, |r| r.node);
        ex.emit(Ev::EntryRouted {
            entry: e,
            routing: Routing::Request { node, request },
            reason: None,
        });
        return None;
    }
    Some(request)
}

/// Whether the unsettled request `r` has a *result answer* (1.7, §5): for a user request, its
/// answer, which only a `to_user` of kind `result` to its requester gives (§9.4); for a request
/// from a person or an agent, a `said` of kind `result` or `correction` recorded after it in its
/// conversation, on its node, that is not unconfirmed, not withheld (1.9), and not listed in
/// `unsent`.
#[must_use]
pub fn result_answered(s: &State, r: RequestId, unsent: &BTreeSet<SaidId>) -> bool {
    let Some(q) = s.requests.get(&r) else {
        return false;
    };
    if q.settled.is_some() || q.answered_by.is_none() {
        return false;
    }
    match q.requester {
        Requester::User(_) => true,
        Requester::External => s.ix.items.get(&q.node).is_some_and(|it| {
            it.saids.range(Eid(r.0.saturating_add(1))..).any(|y| {
                s.saids.get(y).is_some_and(|x| {
                    x.conv == q.conv
                        && matches!(x.kind, SaidKind::Result | SaidKind::Correction)
                        && !matches!(x.state, SaidState::Unconfirmed | SaidState::Withheld)
                        && !unsent.contains(y)
                })
            })
        }),
    }
}

/// The default `settles` of a close of `n` (1.7, §5): every unsettled request on `n` with a result
/// answer.
#[must_use]
pub fn default_settles(s: &State, n: NodeId, unsent: &BTreeSet<SaidId>) -> BTreeSet<RequestId> {
    s.ix.items
        .get(&n)
        .map(|it| {
            it.unsettled
                .iter()
                .copied()
                .filter(|r| result_answered(s, *r, unsent))
                .collect()
        })
        .unwrap_or_default()
}
