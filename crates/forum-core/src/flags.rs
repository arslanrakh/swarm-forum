//! Flags (design D.8.3, §14, F) and the staleness of a decision's basis (D.8.17, D-4).
//!
//! The predicates are over a settled state at `now`. Every quantifier ranges over an open subset of
//! the node's `ix.items` (CS-20), so a node's flags cost O(open items on the node), whatever its
//! history; each line equals the quantifier over every entity of `spec::flags` (D.9.1), which F
//! compares with the materialised `ix.node[n].flags`. `evolve` recomputes `flags_of(s, n, e.at)` for
//! every node it refreshes (D.7.4).
//!
//! The predicates read times, not markers (`now ≥ T`, `now ≥ c.followup_at`, `now ≥ d`): the markers
//! only keep the deadline queue exact (D.4.8), and T3 checks that each equals its predicate on a
//! settled state.

use alloc::collections::BTreeSet;

use crate::state::{
    BasisRef, BasisTarget, CommitmentState, Decision, DecisionState, Ix, Life, Node, Origin,
    PqState, Role, State, UqState, WaitOn,
};
use crate::types::{Flag, FlagSet, NodeId, Outcome, Time, USER};

/// `CLOSED_TAKE` (D.8.5): the flags that make a closed node a candidate of `next`, whose take
/// reopens it (A-10, D-4); with `objection` among them, it is a review item instead, which nothing
/// reopens (1.9, §10.4). Every flag a closed node that is not merged can carry is in it (D.8.3).
pub const CLOSED_TAKE: FlagSet = FlagSet(
    Flag::UnansweredRequest.bit()
        | Flag::OpenCommitment.bit()
        | Flag::Objection.bit()
        | Flag::OpenIntent.bit()
        | Flag::UnconfirmedSaid.bit()
        | Flag::BasisChanged.bit(),
);

/// `T0` (D.8.5): the flags that put a candidate in tier 0.
pub const T0: FlagSet = FlagSet(
    Flag::Unrouted.bit()
        | Flag::UnansweredRequest.bit()
        | Flag::OpenCommitment.bit()
        | Flag::UnconfirmedSaid.bit()
        | Flag::OpenIntent.bit()
        | Flag::QuestionRecheck.bit()
        | Flag::ProceedOnAssumption.bit()
        | Flag::BasisChanged.bit(),
);

/// The flags that hold on open nodes only (§26 F): `too_big`, `overdue`, `ready_to_close`,
/// `missed`, and `stale` (D.8.3, 1.8).
pub const OPEN_ONLY: FlagSet = FlagSet(
    Flag::TooBig.bit()
        | Flag::Overdue.bit()
        | Flag::ReadyToClose.bit()
        | Flag::Missed.bit()
        | Flag::Stale.bit(),
);

/// `resolved(w, now)` of D.8.3: a people question resolves the wait when it leaves `Open` or is
/// rechecked past the count the wait was set at; a user question when it leaves `Pending`; a node
/// while it is closed; a time or an external wait at its time. A question that is not resident has
/// left memory, which only a finished question does, so it resolves the wait; a node that does not
/// exist resolves it too (neither occurs: a wait holds its question back, D.4.12, and nodes are never
/// removed).
#[must_use]
pub fn resolved(s: &State, w: &WaitOn, now: Time) -> bool {
    match *w {
        WaitOn::Question { q, rechecks_at_set } => s
            .pquestions
            .get(&q)
            .is_none_or(|pq| pq.state != PqState::Open || pq.rechecks > rechecks_at_set),
        WaitOn::UserQuestion(q) => s
            .uquestions
            .get(&q)
            .is_none_or(|uq| uq.state != UqState::Pending),
        WaitOn::Node(t) => s.nodes.get(&t).is_none_or(|t| !t.is_open()),
        WaitOn::Time(t) | WaitOn::External(t) => now >= t,
    }
}

/// `waiting(n, now) := n.wait = Some(w) ∧ ¬resolved(w, now)` (D.8.3).
#[must_use]
pub fn waiting(s: &State, n: &Node, now: Time) -> bool {
    n.wait.as_ref().is_some_and(|w| !resolved(s, &w.on, now))
}

/// The node a merged node lives on in: follows `merged_into` until a node that was not merged, at
/// most `nodes.len()` steps, so it terminates even on a corrupted cycle (A-88, D.8.4).
#[must_use]
pub fn resolve_merged(s: &State, n: NodeId) -> NodeId {
    let mut cur = n;
    for _ in 0..s.nodes.len() {
        match s.nodes.get(&cur).and_then(Node::merged_into) {
            Some(next) => cur = next,
            None => break,
        }
    }
    cur
}

/// `stale(s, b)` of D.8.17: a node reference is stale when the node, or the node it lives on in, was
/// retracted, or when that node is closed with an outcome other than the one it had when the
/// decision was recorded (`done` if it was open then); an entry reference when the entry was
/// retracted, or is a decision that is no longer active. An edit, a reopen, and a merge make
/// nothing stale; external references are later (§29).
#[must_use]
pub fn stale(s: &State, b: &BasisRef) -> bool {
    match b {
        BasisRef::Node { id, life_at, .. } => {
            let f = resolve_merged(s, *id);
            if s.retracted.contains(&BasisTarget::Node(*id))
                || s.retracted.contains(&BasisTarget::Node(f))
            {
                return true;
            }
            let now = match s.nodes.get(&f).map(|x| &x.life) {
                Some(Life::Closed(c)) => Some(&c.outcome),
                Some(Life::Open) | None => None,
            };
            stale_node(false, now, life_at.as_ref())
        }
        BasisRef::Entry(e) => stale_entry(
            s.retracted.contains(&BasisTarget::Entry(*e)),
            s.decisions.get(e).map(|d| d.state),
        ),
        BasisRef::External { .. } => false,
    }
}

/// The node half of `stale` over its inputs (D.8.17): the node, or the node it lives on in, was
/// retracted; or that node is closed (`now` its outcome, None while open) with another outcome than
/// it had when the decision was recorded (`life_at`, `done` for an open node then). A reopen, which
/// makes the node open, and a merge into an open node never make a reference stale.
#[must_use]
pub fn stale_node(retracted: bool, now: Option<&Outcome>, life_at: Option<&Outcome>) -> bool {
    retracted || now.is_some_and(|o| o != life_at.unwrap_or(&Outcome::Done))
}

/// The entry half of `stale` over its inputs (D.8.17): retracted, or a resident decision that is no
/// longer active.
#[must_use]
pub fn stale_entry(retracted: bool, decision: Option<DecisionState>) -> bool {
    retracted || decision.is_some_and(|d| d != DecisionState::Active)
}

/// `stale_decision(s, d) := d is Active ∧ some reference of its basis is stale` (D.8.17).
#[must_use]
pub fn stale_decision(s: &State, d: &Decision) -> bool {
    d.state == DecisionState::Active && d.basis.iter().any(|b| stale(s, b))
}

/// The targets under which `ix.cited_by` files an active decision (D.4.10, D.8.17): each node
/// reference under `Node(resolve_merged(m))`, so that a merge re-files the citers of `from` under
/// `into`; each entry reference under `Entry(e)`. External references are filed nowhere.
#[must_use]
pub fn basis_targets(s: &State, d: &Decision) -> BTreeSet<BasisTarget> {
    let mut out = BTreeSet::new();
    for b in &d.basis {
        match b {
            BasisRef::Node { id, .. } => {
                out.insert(BasisTarget::Node(resolve_merged(s, *id)));
            }
            BasisRef::Entry(e) => {
                out.insert(BasisTarget::Entry(*e));
            }
            BasisRef::External { .. } => {}
        }
    }
    out
}

/// `flags_of(s, n, now)` of D.8.3: one bit per predicate, read from the node, its `ix.items` open
/// subsets, and its `ix.node` rollup. A node that does not exist has none.
#[must_use]
pub fn flags_of(s: &State, n: NodeId, now: Time) -> FlagSet {
    flags_in(s, &s.ix, n, now)
}

/// `flags_of` with the indices given apart from the state, for the recount, which evaluates the
/// flags on the indices it is building (D.4.10).
#[must_use]
pub fn flags_in(s: &State, ix: &Ix, n: NodeId, now: Time) -> FlagSet {
    flag_inputs(s, ix, n).map_or(FlagSet::EMPTY, |i| flags_from(&i, now))
}

/// A node's wait as the flags read it: none, unresolved or resolved for a reason time does not
/// change (a question or a node), or resolving at a time (a time or an external wait).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WaitInput {
    /// No wait.
    None,
    /// A question or node wait that has not resolved.
    Unresolved,
    /// A question or node wait that has resolved.
    Resolved,
    /// A time or external wait, resolved from this time on.
    Until(Time),
}

impl WaitInput {
    /// `waiting(n, now)` of D.8.3.
    #[must_use]
    pub fn waiting(self, now: Time) -> bool {
        match self {
            WaitInput::None | WaitInput::Resolved => false,
            WaitInput::Unresolved => true,
            WaitInput::Until(t) => now < t,
        }
    }
}

/// What the flags of one node depend on, read from the state and its indices without the time
/// (D.8.3): `flags_from` combines them at a time. The quantifier over the open commitments is
/// summarised by whether one exists and the earliest follow-up among those whose conversation has
/// no open people question.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "one input per predicate of D.8.3"
)]
pub struct FlagInputs {
    /// The node is `@user` and some user entry is unrouted.
    pub user_unrouted: bool,
    /// The node is open.
    pub open: bool,
    /// The node is closed with `merged_into`.
    pub merged: bool,
    /// A live hold on the node or a descendant.
    pub attended: bool,
    /// A live hold on the node itself.
    pub held_self: bool,
    /// The node's wait.
    pub wait: WaitInput,
    /// Some request on the node is unsettled.
    pub unsettled: bool,
    /// Some commitment on the node is open.
    pub open_commitment: bool,
    /// The earliest follow-up among the open commitments whose conversation has no open people
    /// question on the node.
    pub free_followup: Option<Time>,
    /// Some `said` on the node is unconfirmed.
    pub unconfirmed: bool,
    /// Some objection on the node is open.
    pub objection: bool,
    /// Some active decision on the node has a stale reference.
    pub stale_basis: bool,
    /// The node has an open intent.
    pub open_intent: bool,
    /// The node carries a recheck mark.
    pub recheck: bool,
    /// The node carries an assume mark.
    pub assume: bool,
    /// `attempts ≥ too_big.attempts`.
    pub attempts_over: bool,
    /// The node's due.
    pub due: Option<Time>,
    /// Not reserved, held at least once (its fence is past 0), at least one child, and no open
    /// child (1.6: a node never held is a folder, which its children's closing does not finish).
    pub children_closed: bool,
    /// A series whose last occurrence was missed.
    pub series_missed: bool,
    /// 1.8 (§14): tagged `offer`, capacity above 0, and not a container: the node is a candidate by
    /// its tag.
    pub offered: bool,
    /// 1.8: `touched_at + offer.stale`, None for a node never touched since 1.8.
    pub stale_at: Option<Time>,
}

/// The fourteen flags of D.8.3 from their inputs at `now`.
#[must_use]
pub fn flags_from(i: &FlagInputs, now: Time) -> FlagSet {
    let wait = i.wait.waiting(now);
    let mut f = FlagSet::EMPTY;
    if i.user_unrouted {
        f.insert(Flag::Unrouted);
    }
    if i.unsettled && (!i.open || (!i.attended && !wait)) {
        f.insert(Flag::UnansweredRequest);
    }
    let due_followup = i.free_followup.is_some_and(|t| now >= t);
    if (!i.open && i.open_commitment) || (i.open && !i.attended && due_followup) {
        f.insert(Flag::OpenCommitment);
    }
    if i.unconfirmed && !i.attended {
        f.insert(Flag::UnconfirmedSaid);
    }
    if i.open_intent && !i.held_self && !wait {
        f.insert(Flag::OpenIntent);
    }
    if i.recheck {
        f.insert(Flag::QuestionRecheck);
    }
    if i.assume {
        f.insert(Flag::ProceedOnAssumption);
    }
    if i.objection {
        f.insert(Flag::Objection);
    }
    if i.stale_basis && !i.merged {
        f.insert(Flag::BasisChanged);
    }
    if i.open {
        if i.attempts_over {
            f.insert(Flag::TooBig);
        }
        if i.due.is_some_and(|d| now >= d) {
            f.insert(Flag::Overdue);
        }
        if i.children_closed {
            f.insert(Flag::ReadyToClose);
        }
        if i.series_missed {
            f.insert(Flag::Missed);
        }
        if i.offered && !i.held_self && !wait && i.stale_at.is_some_and(|t| now >= t) {
            f.insert(Flag::Stale);
        }
    }
    f
}

/// The inputs of `flags_from` for node `n`, from the node, its `ix.items` open subsets, and its
/// `ix.node` rollup; None for a node that does not exist.
#[must_use]
pub fn flag_inputs(s: &State, ix: &Ix, n: NodeId) -> Option<FlagInputs> {
    let node = s.nodes.get(&n)?;
    let nix = ix.node.get(&n).copied().unwrap_or_default();
    let items = ix.items.get(&n);
    let wait = match &node.wait {
        None => WaitInput::None,
        Some(w) => match w.on {
            WaitOn::Time(t) | WaitOn::External(t) => WaitInput::Until(t),
            WaitOn::Question { .. } | WaitOn::UserQuestion(_) | WaitOn::Node(_) => {
                if resolved(s, &w.on, Time::ZERO) {
                    WaitInput::Resolved
                } else {
                    WaitInput::Unresolved
                }
            }
        },
    };
    let mut open_commitment = false;
    let mut free_followup: Option<Time> = None;
    if let Some(items) = items {
        for c in items
            .open_commitments
            .iter()
            .filter_map(|c| s.commitments.get(c))
        {
            if c.state != CommitmentState::Open {
                continue;
            }
            open_commitment = true;
            if items.open_pq.get(&c.conv).is_none_or(BTreeSet::is_empty) {
                free_followup = Some(free_followup.map_or(c.followup_at, |t| t.min(c.followup_at)));
            }
        }
    }
    let any = |f: fn(&crate::state::NodeItems) -> bool| items.is_some_and(f);
    Some(FlagInputs {
        user_unrouted: n == USER && !ix.unrouted.is_empty(),
        open: node.is_open(),
        merged: node.merged_into().is_some(),
        attended: nix.subtree_live > 0,
        held_self: nix.live > 0,
        wait,
        unsettled: any(|it| !it.unsettled.is_empty()),
        open_commitment,
        free_followup,
        unconfirmed: any(|it| !it.unconfirmed.is_empty()),
        objection: any(|it| !it.open_objections.is_empty()),
        stale_basis: any(|it| !it.stale_basis.is_empty()),
        open_intent: node.open_intent.is_some(),
        recheck: !node.recheck_marks.is_empty(),
        assume: !node.assume_marks.is_empty(),
        attempts_over: node.attempts >= s.params.too_big_attempts,
        due: node.due,
        children_closed: node.origin != Origin::Reserved
            && node.fence.0 > 0
            && nix.children >= 1
            && nix.open_children == 0,
        series_missed: matches!(&node.role, Role::Series(x) if x.last_missed),
        offered: stale_candidate(node),
        stale_at: s.params.stale_at(node.touched_at),
    })
}

/// Whether node `n` is an offer that can go stale (1.8, §14): it carries `offer`, has capacity
/// above 0, and is not a container (reserved, a series, or a person or team of the directory).
#[must_use]
pub fn stale_candidate(node: &Node) -> bool {
    node.capacity > 0
        && !node.is_container()
        && node.tags.iter().any(|t| t.as_str() == crate::types::OFFER)
}
