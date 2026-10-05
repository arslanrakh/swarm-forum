//! The step invariants of WP-D4 (design D.9.2): K7 (a reopen keeps only the keys no other open
//! node carries), N5 (a merge moves and redirects), with Settled's merge clause (a merge credits a
//! moved request exactly with the first answering `said` of its conversation on `into` after it,
//! §9.4), C1 (every entry about a conversation lies on its conversation node, and a reply fulfils
//! exactly its own conversation's commitments), and L1's half for series (a move, or a `repeat`
//! set at create or update, leaves room for the series' occurrences within `tree.max_depth`,
//! D.14.5).
//!
//! Like the other step checks, they compute from the base state and the events with naive
//! definitions written from the specification (`spec`, and the key view below, which replays only
//! the tree and the keys): no code of `apply`, the helpers, or `evolve` is shared (D.9.1).

use alloc::collections::{BTreeMap, BTreeSet};

use super::{Out, text};
use crate::cmd::{Cmd, Ctx, NodeCmd, SessionCtx, TalkCmd};
use crate::error::Error;
use crate::event::{EntryBody, Ev, Event, QuestionBody};
use crate::invariants::{Inv, ViolationDetail};
use crate::reply::{Done, Reply};
use crate::spec;
use crate::state::{
    CommitmentState, Life, ObjectionState, PqState, Role, SaidState, State, WaitOn,
};
use crate::types::{Change, Eid, KeyStr, NodeId, SaidKind, SessionId};

// ------------------------------------------------------------------------------------------------
// K7

/// K7: every node a `NodeReopened` of the step reopens keeps exactly the keys of `before` that no
/// other node carries openly in `before` (a reopen by `node reopen`, or by `take` or `next` of a
/// closed node).
pub(super) fn k7(before: &State, events: &[Event], after: &State, o: &mut Out) {
    for e in events {
        let Ev::NodeReopened { node } = &e.ev else {
            continue;
        };
        let Some(b) = before.nodes.get(node) else {
            o.push(Inv::K7, ViolationDetail::Node(*node));
            continue;
        };
        let want: BTreeSet<&KeyStr> = b
            .keys
            .keys()
            .filter(|k| {
                !before.nodes.iter().any(|(m, x)| {
                    m != node && matches!(x.life, Life::Open) && x.keys.contains_key(*k)
                })
            })
            .collect();
        let have: BTreeSet<&KeyStr> = after
            .nodes
            .get(node)
            .map(|x| x.keys.keys().collect())
            .unwrap_or_default();
        o.check(want == have, Inv::K7, ViolationDetail::Node(*node));
    }
}

// ------------------------------------------------------------------------------------------------
// N5

/// Whether an event is bookkeeping that any refused call may append: the session a first call
/// opens, its principal, and the retry record (§21).
fn bookkeeping(ev: &Ev) -> bool {
    matches!(
        ev,
        Ev::SessionOpened { .. } | Ev::PrincipalSeen { .. } | Ev::OpRecorded { .. }
    )
}

/// The caller of a step, if it is a session that existed before it.
fn caller(ctx: &Ctx) -> Option<SessionId> {
    match ctx.session {
        SessionCtx::Existing(x) => Some(x),
        SessionCtx::New | SessionCtx::System => None,
    }
}

/// N5: a merge moves keys, open requests, people questions `Open`, `Expired`, or `Escalated`,
/// open commitments and objections, `Pending` and `Unconfirmed` `said` entries, the marks, and
/// another session's open intent when `into` has none, each keeping its id with `moved_from =
/// from`, and leaves every other intent where it was; afterwards no wait or dependency names
/// `from`, and there is no self-edge. A refused merge changes nothing. With it, Settled's merge
/// clause (`merge_credits`).
pub(super) fn n5(
    before: &State,
    ctx: &Ctx,
    cmd: &Cmd,
    out: &Result<Done, Error>,
    events: &[Event],
    after: &State,
    o: &mut Out,
) {
    let Cmd::Node(NodeCmd::Merge { .. }) = cmd else {
        return;
    };
    let (from, into) = match out {
        Ok(Done {
            reply: Reply::Merged { from, into },
            ..
        }) => (*from, *into),
        Ok(_) => return,
        Err(_) => {
            o.check(
                events.iter().all(|e| bookkeeping(&e.ev)),
                Inv::N5,
                text("a refused merge changes nothing"),
            );
            return;
        }
    };
    let (Some(f), Some(i), Some(f2), Some(i2)) = (
        before.nodes.get(&from),
        before.nodes.get(&into),
        after.nodes.get(&from),
        after.nodes.get(&into),
    ) else {
        o.push(Inv::N5, text("a merge of nodes that do not exist"));
        return;
    };
    // Keys.
    let mut keys = i.keys.clone();
    keys.extend(f.keys.iter().map(|(k, v)| (k.clone(), *v)));
    o.check(i2.keys == keys, Inv::N5, ViolationDetail::Node(into));
    o.check(f2.keys.is_empty(), Inv::N5, ViolationDetail::Node(from));
    // What is still open moves, with its id and `moved_from`.
    let moved = |node: Option<NodeId>, moved_from: Option<NodeId>| {
        node == Some(into) && moved_from == Some(from)
    };
    for (r, x) in &before.requests {
        if x.node == from && x.settled.is_none() {
            let y = after.requests.get(r);
            o.check(
                moved(y.map(|y| y.node), y.and_then(|y| y.moved_from)),
                Inv::N5,
                ViolationDetail::Entry(*r),
            );
        }
    }
    for (q, x) in &before.pquestions {
        let open = matches!(
            x.state,
            PqState::Open | PqState::Expired | PqState::Escalated { .. }
        );
        if x.node == from && open {
            let y = after.pquestions.get(q);
            o.check(
                moved(y.map(|y| y.node), y.and_then(|y| y.moved_from)),
                Inv::N5,
                ViolationDetail::Entry(*q),
            );
        }
    }
    for (c, x) in &before.commitments {
        if x.node == from && x.state == CommitmentState::Open {
            let y = after.commitments.get(c);
            o.check(
                moved(y.map(|y| y.node), y.and_then(|y| y.moved_from)),
                Inv::N5,
                ViolationDetail::Entry(*c),
            );
        }
    }
    for (b, x) in &before.objections {
        if x.node == from && x.state == ObjectionState::Open {
            let y = after.objections.get(b);
            o.check(
                moved(y.map(|y| y.node), y.and_then(|y| y.moved_from)),
                Inv::N5,
                ViolationDetail::Entry(*b),
            );
        }
    }
    for (a, x) in &before.saids {
        if x.node == from && matches!(x.state, SaidState::Pending | SaidState::Unconfirmed) {
            let y = after.saids.get(a);
            o.check(
                moved(y.map(|y| y.node), y.and_then(|y| y.moved_from)),
                Inv::N5,
                ViolationDetail::Entry(*a),
            );
        }
    }
    // The marks.
    o.check(
        f2.recheck_marks.is_empty() && f2.assume_marks.is_empty(),
        Inv::N5,
        ViolationDetail::Node(from),
    );
    o.check(
        f.recheck_marks.is_subset(&i2.recheck_marks) && f.assume_marks.is_subset(&i2.assume_marks),
        Inv::N5,
        ViolationDetail::Node(into),
    );
    // Another session's open intent moves when `into` has none; every other intent stays off
    // `into` (D.8.20 step 2: "an intent that cannot move stays on from"): the merging session's
    // own, which the close of `from` then resolves (§5, D.8.18), and any when `into` has one.
    let mine = f
        .open_intent
        .is_some_and(|x| Some(x.session) == caller(ctx));
    let moves = f.open_intent.is_some() && !mine && i.open_intent.is_none();
    if moves {
        o.check(
            i2.open_intent == f.open_intent && f2.open_intent.is_none(),
            Inv::N5,
            ViolationDetail::Node(into),
        );
    } else {
        o.check(
            f2.open_intent == f.open_intent || (mine && f2.open_intent.is_none()),
            Inv::N5,
            ViolationDetail::Node(from),
        );
        o.check(
            i2.open_intent == i.open_intent,
            Inv::N5,
            ViolationDetail::Node(into),
        );
    }
    merge_credits(before, events, after, from, into, o);
    // Waits and dependencies: none names `from`, and none is a self-edge.
    for (&x, node) in &after.nodes {
        let wait = node.wait.as_ref().map(|w| w.on);
        let ok = !matches!(wait, Some(WaitOn::Node(t)) if t == from || t == x)
            && !node.depends_on.contains(&from)
            && !node.depends_on.contains(&x);
        o.check(ok, Inv::N5, ViolationDetail::Node(x));
    }
}

/// Settled's merge clause (§9.4 and §16 "Answers", §26 Settled: "or one that a merge credits to
/// r"; D.8.20 step 3): a merge credits each moved request that is unanswered and not a user
/// request with the first `said` on `into` after it, of kind reply, result, or correction, in its
/// own conversation, and not withheld (1.9), when there is one, and credits nothing else. As D.8.20 orders the steps, the
/// `said` entries on `into` are those after the merge's moves (step 3 follows step 2: a pending
/// `said` that moves with `from` counts), read in memory (D.4.12).
fn merge_credits(
    before: &State,
    events: &[Event],
    after: &State,
    from: NodeId,
    into: NodeId,
    o: &mut Out,
) {
    let mut credited: BTreeMap<Eid, Eid> = BTreeMap::new();
    for e in events {
        if let Ev::RequestAnswered { request, by } = &e.ev {
            o.check(
                credited.insert(*request, *by).is_none(),
                Inv::Settled,
                ViolationDetail::Entry(*request),
            );
        }
    }
    for (r, x) in &before.requests {
        let moved = x.node == from
            && x.settled.is_none()
            && x.answered_by.is_none()
            && x.from_user.is_none();
        let want = if moved {
            after
                .saids
                .iter()
                .filter(|(a, y)| {
                    y.node == into
                        && *a > r
                        && answering(y.kind)
                        && y.conv == x.conv
                        && y.state != SaidState::Withheld
                })
                .map(|(a, _)| *a)
                .min()
        } else {
            None
        };
        o.check(
            credited.remove(r) == want,
            Inv::Settled,
            ViolationDetail::Entry(*r),
        );
    }
    o.check(
        credited.is_empty(),
        Inv::Settled,
        text("a merge credits only the requests it moves"),
    );
}

// ------------------------------------------------------------------------------------------------
// C1

/// The tree and the keys of a state, replayed event by event through a step: what `conv_node`
/// reads (§9.1), so that each entry is checked against the state just before it (A-12).
struct KeyView {
    /// Each node: its parent, whether it is open, and its keys.
    nodes: BTreeMap<NodeId, (Option<NodeId>, bool, BTreeSet<KeyStr>)>,
}

impl KeyView {
    fn of(s: &State) -> KeyView {
        KeyView {
            nodes: s
                .nodes
                .iter()
                .map(|(&n, x)| {
                    (
                        n,
                        (
                            x.parent(),
                            matches!(x.life, Life::Open),
                            x.keys.keys().cloned().collect(),
                        ),
                    )
                })
                .collect(),
        }
    }

    /// Applies the tree and key part of one event; every other event changes neither.
    fn apply(&mut self, ev: &Ev) {
        if let Ev::NodeCreated {
            id, parent, init, ..
        } = ev
        {
            self.nodes.insert(
                *id,
                (Some(*parent), true, init.keys.keys().cloned().collect()),
            );
        } else if let Ev::KeyAdded { node, key } = ev
            && let Some(x) = self.nodes.get_mut(node)
        {
            x.2.insert(key.key.clone());
        } else if let Ev::KeyRemoved { node, key } = ev
            && let Some(x) = self.nodes.get_mut(node)
        {
            x.2.remove(key);
        } else if let Ev::NodeMoved { node, parent, .. } = ev
            && let Some(x) = self.nodes.get_mut(node)
        {
            x.0 = Some(*parent);
        } else if let Ev::NodeClosed { node, .. } = ev
            && let Some(x) = self.nodes.get_mut(node)
        {
            x.1 = false;
        } else if let Ev::NodeReopened { node } = ev
            && let Some(x) = self.nodes.get_mut(node)
        {
            x.1 = true;
        }
    }

    /// §9.1: the nearest node from `at` up to the root that carries `k`, open or closed; else the
    /// open node that carries it; else `at`.
    fn conv_node(&self, at: NodeId, k: &KeyStr) -> NodeId {
        let mut cur = Some(at);
        let mut steps = 0_usize;
        while let Some(m) = cur {
            if steps > self.nodes.len() {
                break;
            }
            steps = steps.saturating_add(1);
            let Some(x) = self.nodes.get(&m) else {
                break;
            };
            if x.2.contains(k) {
                return m;
            }
            cur = x.0;
        }
        self.nodes
            .iter()
            .find(|(_, x)| x.1 && x.2.contains(k))
            .map_or(at, |(&m, _)| m)
    }
}

/// The node a command's conversation entries are found from (§9.1's `at`): the created or joined
/// node of a `create` with a request, and the `at` of `talk request`, `said`, `heard`, and `ask`.
fn conv_at(before: &State, cmd: &Cmd, out: &Result<Done, Error>) -> Option<NodeId> {
    if let Cmd::Node(NodeCmd::Create(_)) = cmd {
        return match out.as_ref().map(|d| &d.reply) {
            Ok(Reply::Created { id, .. }) => Some(*id),
            Ok(Reply::Joined { existing, .. }) => Some(*existing),
            Ok(_) | Err(_) => None,
        };
    }
    if let Cmd::Talk(
        TalkCmd::Request { at, .. }
        | TalkCmd::Said { at, .. }
        | TalkCmd::Heard { at, .. }
        | TalkCmd::Ask { at, .. },
    ) = cmd
    {
        return spec::resolve(before, at);
    }
    None
}

/// The conversation key of an entry C1 places (§26 C1): a request from a conversation (an origin
/// other than a `user:` entry), a people question, a `said`, or a `heard`; or of a
/// `CommitmentOpened`. With the node it lies on.
fn conv_entry(ev: &Ev) -> Option<(NodeId, &KeyStr, Eid)> {
    if let Ev::CommitmentOpened { c, node, conv, .. } = ev {
        return Some((*node, conv, *c));
    }
    let Ev::Appended {
        eid, node, body, ..
    } = ev
    else {
        return None;
    };
    if let EntryBody::Request {
        origin,
        conv,
        from_user: None,
        ..
    } = body
        && origin.key.user_entry().is_none()
    {
        return Some((*node, conv, *eid));
    }
    if let EntryBody::Question(QuestionBody::People { conv, .. })
    | EntryBody::Said { conv, .. }
    | EntryBody::Heard { conv, .. } = body
    {
        return Some((*node, conv, *eid));
    }
    None
}

/// Whether a `said` of this kind answers and fulfils (§9.2, §9.4).
fn answering(kind: SaidKind) -> bool {
    matches!(
        kind,
        SaidKind::Reply | SaidKind::Result | SaidKind::Correction
    )
}

/// C1: every entry about conversation `k` lies on `conv_node(at, k)` of the state just before it,
/// a node that is not reserved; every `CommitmentFulfilled { x, by: a }` follows, in the step, the
/// `said` `a` of kind reply, result, or correction on `x`'s node, in `x`'s conversation, and not
/// withheld in the step (1.9); and such a `said` of conversation `k` on node `c` leaves no
/// commitment of `k` on `c` that was open before it still open (§9.2, 1.3).
pub(super) fn c1(
    before: &State,
    cmd: Option<(&Cmd, &Result<Done, Error>)>,
    events: &[Event],
    after: &State,
    o: &mut Out,
) {
    let at = cmd.and_then(|(c, out)| conv_at(before, c, out));
    let mut view = KeyView::of(before);
    let mut saids: BTreeMap<Eid, (NodeId, KeyStr, SaidKind)> = BTreeMap::new();
    for e in events {
        if let Some((node, k, eid)) = conv_entry(&e.ev) {
            o.check(!node.is_reserved(), Inv::C1, ViolationDetail::Entry(eid));
            if let Some(at) = at {
                o.check(
                    view.conv_node(at, k) == node,
                    Inv::C1,
                    ViolationDetail::Entry(eid),
                );
            }
        }
        if let Ev::Appended {
            eid,
            node,
            body: EntryBody::Said { conv, kind, .. },
            ..
        } = &e.ev
        {
            saids.insert(*eid, (*node, conv.clone(), *kind));
        }
        // 1.9 (§9.2): a withheld `said` takes no effect of its kind.
        if let Ev::SaidWithheld { said } = &e.ev {
            saids.remove(said);
        }
        if let Ev::CommitmentFulfilled { c, by } = &e.ev {
            let ok = before.commitments.get(c).is_some_and(|x| {
                saids
                    .get(by)
                    .is_some_and(|(n, k, kind)| answering(*kind) && *n == x.node && *k == x.conv)
            });
            o.check(ok, Inv::C1, ViolationDetail::Entry(*c));
        }
        view.apply(&e.ev);
    }
    for (node, k, kind) in saids.values() {
        if !answering(*kind) {
            continue;
        }
        for (c, x) in &before.commitments {
            if x.node == *node && x.conv == *k && x.state == CommitmentState::Open {
                let still = after
                    .commitments
                    .get(c)
                    .is_some_and(|y| y.state == CommitmentState::Open);
                o.check(!still, Inv::C1, ViolationDetail::Entry(*c));
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------
// L1, series (D.14.5)

/// L1's half for series (§19, D.14.5): after a step that moves a subtree, creates a series, or
/// sets a `repeat`, every series among them lies at least one level above `tree.max_depth`, so
/// that its occurrences fit; a move counts each series of the moved subtree.
pub(super) fn l1_series(events: &[Event], after: &State, o: &mut Out) {
    let mut roots: BTreeSet<NodeId> = BTreeSet::new();
    for e in events {
        if let Ev::NodeMoved { node, .. } = &e.ev {
            roots.insert(*node);
        } else if let Ev::NodeCreated { id, init, .. } = &e.ev
            && matches!(init.role, Role::Series(_))
        {
            roots.insert(*id);
        } else if let Ev::NodeUpdated { node, patch } = &e.ev
            && matches!(patch.repeat, Change::Set(_))
        {
            roots.insert(*node);
        }
    }
    if roots.is_empty() {
        return;
    }
    let max = usize::try_from(after.params.tree_max_depth).unwrap_or(usize::MAX);
    for (&m, node) in &after.nodes {
        if !matches!(node.role, Role::Series(_)) {
            continue;
        }
        let up = spec::ancestors_or_self(after, m);
        if up.iter().any(|a| roots.contains(a)) {
            let depth = up.len().saturating_sub(1);
            o.check(
                depth.saturating_add(1) <= max,
                Inv::L1,
                ViolationDetail::Node(m),
            );
        }
    }
}

// ------------------------------------------------------------------------------------------------
// S1's step clause (1.11)

/// S1 [step] (1.11, §14): a step that closes a node other than by a merge leaves no open
/// `duplicate` signal whose pair holds that node and whose other node is closed and not merged:
/// the close of the pair's second node resolves the signal in the same step. Read from the step's
/// `NodeClosed` events and `after`'s base maps.
pub(super) fn s1_step(_before: &State, events: &[Event], after: &State, o: &mut Out) {
    let closed: BTreeSet<NodeId> = events
        .iter()
        .filter_map(|e| {
            if let Ev::NodeClosed {
                node,
                merged_into: None,
                ..
            } = &e.ev
            {
                Some(*node)
            } else {
                None
            }
        })
        .collect();
    if closed.is_empty() {
        return;
    }
    let done = |n: NodeId| {
        after
            .nodes
            .get(&n)
            .is_some_and(|x| !x.is_open() && x.merged_into().is_none())
    };
    for (key, &sig) in &after.signals {
        if key.kind != crate::types::SignalKind::Duplicate {
            continue;
        }
        let open = after.nodes.get(&sig).is_some_and(|x| x.is_open());
        let stale =
            (closed.contains(&key.a) || closed.contains(&key.b)) && done(key.a) && done(key.b);
        o.check(
            !(open && stale),
            Inv::S1,
            text("a duplicate signal whose two nodes are closed, neither merged, must resolve"),
        );
    }
}
