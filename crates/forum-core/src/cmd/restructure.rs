//! The commands of `node reopen`, `move`, and `merge` (design D.8.20, §16). Owner: WP-D4.
//!
//! Each command is a pair: a validation that reads the state and returns the plan or the error,
//! and an execution of the plan that emits the events and cannot fail. Every validation follows
//! D.8.24's row of its command.
//!
//! The pure decisions the validations take (`move_place` with `within`, `move_height`,
//! `merge_sides`, and `merge_edges` with `redirect`) are in `helpers/restructure.rs`, so that the
//! Kani harnesses `move_keeps_tree` and `merge_redirects` check the very code the commands run
//! (D.12.5).

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;

use crate::apply::{node_of, target};
use crate::cmd::{Cmd, NodeCmd};
use crate::error::{Error, Field, Hint, LimitKind, What, Why};
use crate::event::{EndCause, EntryBody, EntryText, Ev, SystemNote};
use crate::helpers::Exec;
use crate::helpers::close::{Closing, close_node};
use crate::helpers::conversation::try_settle;
use crate::helpers::recurring::maybe_missed;
use crate::helpers::restructure::{
    MergeSides, MovePlace, merge_edges, merge_sides, move_height, move_place,
    reopen as reopen_node, resolve_pair,
};
use crate::helpers::tree::reaches;
use crate::reply::{Reply, Warning, WarningCode, WarningDetail};
use crate::state::{Author, Node, OccPhase, Origin, PqState, Role, State, WaitKey, WaitOn};
use crate::types::{
    KeyStr, Label, Mark, Name, NodeId, OFFER, Outcome, PEOPLE, SaidKind, SessionId, TEAMS, Text,
    Time,
};

use super::Vcx;
use super::keys::reserved;
use super::node_basic::ancestor_init;

/// The sessions other than `me` with a live hold on `n`.
fn other_holders(s: &State, n: NodeId, me: Option<SessionId>, now: Time) -> Vec<SessionId> {
    crate::query::holders(s, n, now)
        .into_iter()
        .map(|(x, _)| x)
        .filter(|x| Some(*x) != me)
        .collect()
}

/// The `held_by_other` warning naming `holders`, if any.
fn held_by_other(ex: &mut Exec<'_>, n: NodeId, holders: Vec<SessionId>) {
    if holders.is_empty() {
        return;
    }
    ex.warn(Warning {
        code: WarningCode::HeldByOther,
        id: Some(n),
        detail: WarningDetail::HeldByOther {
            holders,
            left: Vec::new(),
        },
    });
}

// ------------------------------------------------------------------------------------------------
// reopen

/// The plan of `node reopen` (D.8.20).
#[derive(Debug)]
pub(crate) struct ReopenPlan {
    n: NodeId,
    reason: Text,
}

/// Validates `node reopen {path, reason}` (D.8.20, D.8.24's row): the node → an empty `reason`
/// (`invalid(EmptyText(Reason))`: §20.5 makes `reason` required, and §21 makes "an empty required
/// text" `invalid`, as for `talk route`, `node hide`, and `control pause`) → an open node
/// (`invalid(NotClosed)`, a reserved node included: it is always open) → a closed occurrence of a
/// series that is still open and recurring (`invalid(OccurrenceReopen)`, A-13). A merged node
/// reopens (§16).
pub(crate) fn reopen(cx: &Vcx<'_>, cmd: &Cmd) -> Result<ReopenPlan, Error> {
    let Cmd::Node(NodeCmd::Reopen { reason, .. }) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    let s = cx.s;
    let n = target(s, cmd)?.ok_or(Error::not_found(What::Node))?;
    let Some(node) = s.nodes.get(&n) else {
        return Err(Error::not_found(What::Node));
    };
    if reason.is_empty() {
        return Err(Error::invalid(Why::EmptyText(Field::Reason)));
    }
    if node.is_open() {
        return Err(Error::invalid(Why::NotClosed));
    }
    if s.occ_locked(n) {
        return Err(Error::invalid(Why::OccurrenceReopen));
    }
    Ok(ReopenPlan {
        n,
        reason: reason.clone(),
    })
}

/// Executes `node reopen`: `reopen(n, reason)` (D.8.20); 1.12 (§16): `OfferSet { on }` unless the
/// node carries `offer` already or is a container (§4), as at the end of a hold (§10.3), so that
/// `next` offers the reopened work; and the `duplicate_key` warning listing the keys it dropped
/// with their open carriers (K7). In the lab's seventh round (it6 l5), four reopened claims
/// carried neither a flag nor the tag, and nothing offered them for the rest of the run.
pub(crate) fn reopen_exec(ex: &mut Exec<'_>, plan: ReopenPlan) -> Reply {
    let n = plan.n;
    let dropped = reopen_node(
        ex,
        n,
        EntryText {
            main: plan.reason,
            extra: Vec::new(),
        },
    );
    let dropped_keys: Vec<KeyStr> = dropped.iter().map(|(k, _)| k.clone()).collect();
    let offered = ex.s().nodes.get(&n).is_some_and(|x| {
        x.is_open() && !x.is_container() && !x.tags.iter().any(|t| t.as_str() == OFFER)
    });
    if offered {
        ex.emit(Ev::OfferSet { node: n, on: true });
    }
    if !dropped.is_empty() {
        ex.warn(Warning {
            code: WarningCode::DuplicateKey,
            id: Some(n),
            detail: WarningDetail::DuplicateKey { keys: dropped },
        });
    }
    Reply::Reopened {
        node: n,
        dropped_keys,
        offered,
    }
}

// ------------------------------------------------------------------------------------------------
// move

/// The plan of `node move` (D.8.20).
#[derive(Debug)]
pub(crate) struct MovePlan {
    n: NodeId,
    /// The move goes nowhere: `n` is at that place already.
    noop: bool,
    sess: SessionId,
    /// The existing node the missing steps hang from.
    parent: NodeId,
    /// The missing ancestors' names, outermost first, and their `needs` (CS-26).
    ancestors: Vec<Name>,
    ancestor_needs: BTreeSet<Label>,
    name: Name,
}

/// The path from `n` down to `p`, both included, for a `cycle` hint: the walk up from `p` to `n`,
/// reversed.
fn path_down(s: &State, n: NodeId, p: NodeId) -> Vec<NodeId> {
    let mut path = alloc::vec![p];
    let mut cur = p;
    while cur != n && path.len() <= s.nodes.len() {
        let Some(up) = s.nodes.get(&cur).and_then(Node::parent) else {
            break;
        };
        path.push(up);
        cur = up;
    }
    path.reverse();
    path
}

/// The open agent-made children of `p` other than `except` (L1, CS-10): origin `Agent` or
/// `Ancestor`, and open.
fn open_agent_children(s: &State, p: NodeId, except: NodeId) -> u32 {
    let count = s.ix.node.get(&p).map_or(0, |x| x.open_agent_children);
    let counted = s.nodes.get(&except).is_some_and(|x| {
        x.parent() == Some(p)
            && x.is_open()
            && matches!(x.origin, Origin::Agent(_) | Origin::Ancestor(_))
    });
    count.saturating_sub(u32::from(counted))
}

/// Validates `node move {path, to}` (D.8.20, D.8.24's row): the node → a reserved node
/// (`invalid(ReservedNode)`) → the new place: its existing prefix resolves through names and
/// aliases, and its missing steps become ancestors; the node's own place is a no-op success →
/// the new parent is the node or lies below it (`cycle`, K4) → a live name there
/// (`invalid(NameTaken)`, K3) → L1: `depth(new parent) + 1 + height(n) <= tree.max_depth`, a
/// series counting one level more (D.14.5), then the child limit, which counts open agent-made
/// children and never at `@people` or `@teams` (`limit(Depth)`, `limit(Children)`, A-113).
pub(crate) fn move_node(cx: &Vcx<'_>, cmd: &Cmd) -> Result<MovePlan, Error> {
    let Cmd::Node(NodeCmd::Move { to, .. }) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    let s = cx.s;
    let n = target(s, cmd)?.ok_or(Error::not_found(What::Node))?;
    let sess = cx.sess.ok_or(Error::invalid(Why::BadParams))?;
    if reserved(s, n) {
        return Err(Error::invalid(Why::ReservedNode));
    }
    let Some(node) = s.nodes.get(&n) else {
        return Err(Error::not_found(What::Node));
    };
    // The new place: the root has none, and no node can move there.
    let Some((name, steps)) = to.0.split_last() else {
        return Err(Error::invalid(Why::NameTaken));
    };
    let mut parent = crate::types::ROOT;
    let mut ancestors: Vec<Name> = Vec::new();
    for step in steps {
        if ancestors.is_empty() {
            let key = (parent, step.clone());
            if let Some(&m) = s.names.get(&key).or_else(|| s.aliases.get(&key)) {
                parent = m;
                continue;
            }
        }
        ancestors.push(step.clone());
    }
    let parent_needs = s
        .nodes
        .get(&parent)
        .map(|x| x.needs.clone())
        .unwrap_or_default();
    let plan = |noop: bool, ancestors: Vec<Name>| MovePlan {
        n,
        noop,
        sess,
        parent,
        ancestors,
        ancestor_needs: parent_needs.clone(),
        name: name.clone(),
    };
    let here = node
        .place
        .as_ref()
        .is_some_and(|pl| pl.parent == parent && pl.name == *name);
    // The own place, then state: the tree (K4), then the name (K3).
    let parent_of = |x: NodeId| s.nodes.get(&x).and_then(Node::parent);
    match move_place(
        n,
        parent,
        !ancestors.is_empty(),
        here,
        s.names.contains_key(&(parent, name.clone())),
        parent_of,
        s.nodes.len(),
    ) {
        MovePlace::Noop => return Ok(plan(true, ancestors)),
        MovePlace::Cycle => {
            return Err(Error::of(Hint::Cycle {
                path: path_down(s, n, parent),
            }));
        }
        MovePlace::NameTaken => return Err(Error::invalid(Why::NameTaken)),
        MovePlace::Move => {}
    }
    // Safety valves (L1): the depth of the deepest node of the moved subtree, a series' future
    // occurrences included, and the child limit where the move adds an open agent-made child.
    let depth_of = |x: NodeId| s.ix.node.get(&x).map_or(0, |i| i.depth);
    let base = depth_of(n);
    let height = move_height(crate::index::subtree(s, n).into_iter().map(|m| {
        let series = s
            .nodes
            .get(&m)
            .is_some_and(|x| matches!(x.role, Role::Series(_)));
        (depth_of(m).saturating_sub(base), series)
    }));
    let missing = u32::try_from(ancestors.len()).unwrap_or(u32::MAX);
    let deepest = depth_of(parent)
        .saturating_add(missing)
        .saturating_add(1)
        .saturating_add(height);
    if deepest > s.params.tree_max_depth {
        return Err(Error::limit(LimitKind::Depth));
    }
    let agent_child =
        node.is_open() && matches!(node.origin, Origin::Agent(_) | Origin::Ancestor(_));
    let adds_child = !ancestors.is_empty() || agent_child;
    if adds_child
        && parent != PEOPLE
        && parent != TEAMS
        && open_agent_children(s, parent, n).saturating_add(1) > s.params.tree_max_children
    {
        return Err(Error::limit(LimitKind::Children));
    }
    Ok(plan(false, ancestors))
}

/// Executes `node move` (D.8.20): `NodeCreated` for the missing ancestors; `NodeMoved` (the old
/// name becomes an alias; a path is not content, so `version` is unchanged, A-3); then
/// `HoldEnded { EndedByClose(Move) }` for every live hold of another session on the node, after the
/// move, so that its `offer` tag is judged at the new place (A-6, A-113); `System(Moved)` on the
/// node; and, if holds ended on an occurrence, `maybe_missed` (A-80). The mover's own hold stays.
pub(crate) fn move_node_exec(ex: &mut Exec<'_>, plan: MovePlan) -> Reply {
    let n = plan.n;
    if plan.noop {
        return Reply::Moved { node: n };
    }
    let sess = plan.sess;
    let Some(from) = ex.s().nodes.get(&n).and_then(|x| x.place.clone()) else {
        return Reply::Moved { node: n };
    };
    let mut parent = plan.parent;
    for name in &plan.ancestors {
        let id = ex.s().counters.next_node;
        ex.emit(Ev::NodeCreated {
            id,
            parent,
            name: name.clone(),
            origin: Origin::Ancestor(sess),
            init: ancestor_init(name, &plan.ancestor_needs),
        });
        parent = id;
    }
    ex.emit(Ev::NodeMoved {
        node: n,
        parent,
        name: plan.name,
    });
    let now = ex.now();
    let ended: Vec<(crate::types::Token, SessionId)> = ex
        .s()
        .holds
        .iter()
        .filter(|(t, h)| t.node == n && now < h.lease_until && h.session != sess)
        .map(|(t, h)| (*t, h.session))
        .collect();
    for &(token, _) in &ended {
        ex.emit(Ev::HoldEnded {
            token,
            end: crate::event::HoldEnd::EndedByClose(EndCause::Move),
        });
    }
    ex.append(
        n,
        Author::Session(sess),
        EntryBody::System(SystemNote::Moved { from }),
        EntryText::default(),
    );
    let occurrence = ex
        .s()
        .nodes
        .get(&n)
        .is_some_and(|x| matches!(x.role, Role::Occurrence(_)));
    if !ended.is_empty() && occurrence {
        maybe_missed(ex, n);
    }
    let mut holders: Vec<SessionId> = ended.into_iter().map(|(_, x)| x).collect();
    holders.sort_unstable();
    holders.dedup();
    held_by_other(ex, n, holders);
    Reply::Moved { node: n }
}

// ------------------------------------------------------------------------------------------------
// merge

/// The nodes with a T5 edge to `n`: its waiters (`{node}` waits, resolved or not, A-55) and its
/// dependents, ascending.
fn preds_of(s: &State, n: NodeId) -> Vec<NodeId> {
    let mut out: BTreeSet<NodeId> =
        s.ix.waiters
            .get(&WaitKey::Node(n))
            .cloned()
            .unwrap_or_default();
    out.extend(s.ix.dependents.get(&n).into_iter().flatten().copied());
    out.into_iter().collect()
}

/// The cycle a merge of `from` into `into` would close (N5, T5), as a path for the hint, or None.
/// The edges before the merge are acyclic (T5), and every edge the merge adds ends at `into`, so a
/// cycle afterwards runs through `into` and one of those edges: `into` reaches some `w ≠ into`
/// with an edge to `from`, over the edges after the merge.
fn merge_cycle(s: &State, from: NodeId, into: NodeId) -> Option<Vec<NodeId>> {
    let preds = preds_of(s, from);
    let (extra, removed) = merge_edges(&preds, from, into);
    preds.iter().filter(|&&w| w != into).find_map(|&w| {
        let mut path = reaches(s, into, w, &extra, &removed)?;
        path.push(into);
        Some(path)
    })
}

/// The plan of `node merge` (D.8.20).
#[derive(Debug)]
pub(crate) struct MergePlan {
    from: NodeId,
    into: NodeId,
    sess: SessionId,
}

/// Validates `node merge {from, into}` (D.8.20, D.8.24's row): both nodes (`not_found(Node)`) →
/// `invalid(SelfMerge)` → either one reserved (`invalid(ReservedNode)`) → `into` closed, a merged
/// one included (`closed`, naming its `merged_into`, K5) → `from` closed
/// (`invalid(MergeFromClosed)`, A-33) → `from` an occurrence before its slot
/// (`invalid(OccurrenceNotYetAvailable)`, A-91) → `into` the open occurrence of the series `from`
/// (`invalid(MergeIntoOwnOccurrence)`, 1.5, WP-D6) → the redirected waits and dependencies close a
/// cycle (`cycle`, N5, T5).
pub(crate) fn merge(cx: &Vcx<'_>, cmd: &Cmd) -> Result<MergePlan, Error> {
    let Cmd::Node(NodeCmd::Merge { into, .. }) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    let s = cx.s;
    let from = target(s, cmd)?.ok_or(Error::not_found(What::Node))?;
    let into = node_of(s, into)?;
    let sess = cx.sess.ok_or(Error::invalid(Why::BadParams))?;
    let (Some(f), Some(i)) = (s.nodes.get(&from), s.nodes.get(&into)) else {
        return Err(Error::not_found(What::Node));
    };
    match merge_sides(
        from,
        into,
        reserved(s, from) || reserved(s, into),
        f.is_open(),
        i.is_open(),
    ) {
        Some(MergeSides::SelfMerge) => return Err(Error::invalid(Why::SelfMerge)),
        Some(MergeSides::Reserved) => return Err(Error::invalid(Why::ReservedNode)),
        Some(MergeSides::IntoClosed) => {
            return Err(Error::of(Hint::Closed {
                node: into,
                merged_into: i.merged_into(),
            }));
        }
        Some(MergeSides::FromClosed) => return Err(Error::invalid(Why::MergeFromClosed)),
        None => {}
    }
    if matches!(&f.role, Role::Occurrence(o) if o.phase == OccPhase::Waiting) {
        return Err(Error::invalid(Why::OccurrenceNotYetAvailable));
    }
    if crate::recurring::into_own_occurrence(s, from, into) {
        return Err(Error::invalid(Why::MergeIntoOwnOccurrence));
    }
    if let Some(path) = merge_cycle(s, from, into) {
        return Err(Error::of(Hint::Cycle { path }));
    }
    Ok(MergePlan { from, into, sess })
}

/// Whether a `said` of this kind answers a request and fulfils a commitment (§9.2, §9.4).
fn answering(kind: SaidKind) -> bool {
    matches!(
        kind,
        SaidKind::Reply | SaidKind::Result | SaidKind::Correction
    )
}

/// Executes `node merge` (D.8.20, N5):
/// 1. each key of `from`: `KeyRemoved` from `from`, then `KeyAdded` to `into`;
/// 2. what is still open moves to `into` with its id and `moved_from`: unsettled requests, people
///    questions `Open`, `Expired`, or `Escalated`, open commitments, open objections, `Pending` or
///    `Unconfirmed` `said` entries, the recheck and assume marks, and an open intent of another
///    session when `into` has none (A-94);
/// 3. each moved request from people or agents still unanswered is answered by the first `said` on
///    `into` after it of kind reply, result, or correction in its own conversation (A-95; §9.4,
///    1.3), then `try_settle`;
/// 4. `System(MergedFrom)` on `into`, with `from`'s digest as its text;
/// 5. the waits on `from` and the dependencies on it redirected to `into`, self-edges dropped;
/// 6. `close_node(from, superseded, merged_into: into)`: its live holds end (`held_by_other` for
///    other sessions', and for those on a series' open occurrence that the close drops, A-14); a
///    series `from` drops its open occurrence, with the `series_stopped` warning (1.4, R2);
/// 7. the open signals of the pair resolved (§14).
#[allow(
    clippy::too_many_lines,
    reason = "the seven steps of D.8.20's merge, in order"
)]
pub(crate) fn merge_exec(ex: &mut Exec<'_>, plan: MergePlan) -> Reply {
    let (from, into, sess) = (plan.from, plan.into, plan.sess);
    let now = ex.now();
    // The warning, on the state before the close: other sessions' holds on `from`, and on the
    // open occurrence a series `from` drops.
    let mut holders = other_holders(ex.s(), from, Some(sess), now);
    if ex
        .s()
        .nodes
        .get(&from)
        .is_some_and(|x| matches!(x.role, Role::Series(_)))
    {
        let occ: Vec<NodeId> = ex
            .s()
            .ix
            .series_open
            .get(&from)
            .into_iter()
            .flatten()
            .copied()
            .collect();
        for o in occ {
            for h in other_holders(ex.s(), o, Some(sess), now) {
                if !holders.contains(&h) {
                    holders.push(h);
                }
            }
        }
    }
    let Some(f) = ex.s().nodes.get(&from).cloned() else {
        return Reply::Merged { from, into };
    };
    // 1.
    for (k, &nv) in &f.keys {
        ex.emit(Ev::KeyRemoved {
            node: from,
            key: k.clone(),
        });
        ex.emit(Ev::KeyAdded {
            node: into,
            key: crate::types::Key { key: k.clone(), nv },
        });
    }
    // 2.
    let items = ex.s().ix.items.get(&from).cloned().unwrap_or_default();
    let moved: Vec<crate::types::RequestId> = items.unsettled.iter().copied().collect();
    for &r in &moved {
        ex.emit(Ev::RequestMoved {
            request: r,
            to: into,
        });
    }
    let pqs: Vec<crate::types::QuestionId> = items
        .pquestions
        .iter()
        .copied()
        .filter(|q| {
            ex.s().pquestions.get(q).is_some_and(|pq| {
                matches!(
                    pq.state,
                    PqState::Open | PqState::Expired | PqState::Escalated { .. }
                )
            })
        })
        .collect();
    for q in pqs {
        ex.emit(Ev::PqMoved { q, to: into });
    }
    for &c in &items.open_commitments {
        ex.emit(Ev::CommitmentMoved { c, to: into });
    }
    for &o in &items.open_objections {
        ex.emit(Ev::ObjectionMoved { o, to: into });
    }
    for &x in &items.open_saids {
        ex.emit(Ev::SaidMoved { said: x, to: into });
    }
    let marks: Vec<(Mark, crate::types::QuestionId)> = f
        .recheck_marks
        .iter()
        .map(|&q| (Mark::Recheck, q))
        .chain(f.assume_marks.iter().map(|&q| (Mark::Assume, q)))
        .collect();
    for (mark, q) in marks {
        ex.emit(Ev::MarkSet {
            node: from,
            mark,
            question: q,
            on: false,
        });
        ex.emit(Ev::MarkSet {
            node: into,
            mark,
            question: q,
            on: true,
        });
    }
    if let Some(intent) = f.open_intent
        && intent.session != sess
        && ex
            .s()
            .nodes
            .get(&into)
            .is_some_and(|x| x.open_intent.is_none())
    {
        ex.emit(Ev::IntentMoved { from, to: into });
    }
    // 3.
    for &r in &moved {
        let s = ex.s();
        let Some(req) = s.requests.get(&r) else {
            continue;
        };
        if req.answered_by.is_some() || req.from_user.is_some() {
            continue;
        }
        let conv = req.conv.clone();
        let by = s.ix.items.get(&into).and_then(|it| {
            it.saids.iter().copied().find(|&a| {
                a > r
                    && s.saids.get(&a).is_some_and(|x| {
                        answering(x.kind)
                            && x.conv == conv
                            && x.state != crate::state::SaidState::Withheld
                    })
            })
        });
        if let Some(a) = by {
            ex.emit(Ev::RequestAnswered { request: r, by: a });
            try_settle(ex, r);
        }
    }
    // 4.
    ex.append(
        into,
        Author::Session(sess),
        EntryBody::System(SystemNote::MergedFrom {
            from,
            last_checkpoint: f.last_checkpoint,
        }),
        EntryText {
            main: f.digest.clone(),
            extra: Vec::new(),
        },
    );
    // 5.
    let waiters: Vec<NodeId> = ex
        .s()
        .ix
        .waiters
        .get(&WaitKey::Node(from))
        .into_iter()
        .flatten()
        .copied()
        .collect();
    for w in waiters {
        let wait = (w != into).then_some(WaitOn::Node(into));
        ex.emit(Ev::WaitSet { node: w, wait });
    }
    let dependents: Vec<NodeId> = ex
        .s()
        .ix
        .dependents
        .get(&from)
        .into_iter()
        .flatten()
        .copied()
        .collect();
    for d in dependents {
        let to = (d != into).then_some(into);
        ex.emit(Ev::DependsRedirected { node: d, from, to });
    }
    // 6.
    held_by_other(ex, from, holders);
    let dropped = close_node(
        ex,
        from,
        Closing {
            outcome: Outcome::Superseded,
            by: Author::Session(sess),
            merged_into: Some(into),
            cause: EndCause::Merge,
            text: EntryText {
                main: Text(alloc::format!("merged into #{}", into.0)),
                extra: Vec::new(),
            },
            reslot: None,
        },
    );
    // A series `from` dropped its open occurrence: `series_stopped`, as a close does (1.4, R2).
    if let Some(o) = dropped {
        ex.warn(super::node_basic::series_stopped(from, o));
    }
    // 7.
    let why: String = alloc::format!("resolved: #{} merged into #{}", from.0, into.0);
    resolve_pair(ex, from, into, &why);
    Reply::Merged { from, into }
}

#[cfg(test)]
#[path = "restructure_tests.rs"]
pub(super) mod tests;
