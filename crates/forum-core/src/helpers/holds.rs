//! Holds: issue, extend, bind `holder:` notes, and `can_take` (design D.8.10, D.8.16, D.8.26).
//!
//! The lease arithmetic is in pure functions (`lease_l`, `first_lease`, `extended_until`) that the
//! helpers and the Kani harnesses of H5 share (D.12.5: a harness may be narrowed to the pure
//! helper that enforces its property).

use alloc::vec::Vec;

use super::Exec;
use crate::event::{Ev, Extension};
use crate::state::{Author, Hold, LeaseTerms, Life, OccPhase, Role, State};
use crate::types::{Dur, Fence, NodeId, Outcome, SECOND, SessionId, Time, Token};

/// The lease terms in force under `params`, as a hold keeps them (A-102).
#[must_use]
pub fn terms_of(p: &crate::params::Params) -> LeaseTerms {
    LeaseTerms {
        first: p.lease_first,
        default: p.lease_default,
        max: p.lease_max,
        ceiling: p.lease_ceiling,
        renew_max: p.renew_max,
    }
}

/// `L` of §10.1: the `lease` argument clamped to `[1 s, lease.max]`, `lease.default` without one
/// (A-8): `min(max(lease or default, 1 s), max)`.
#[must_use]
pub fn lease_l(lease: Option<Dur>, t: &LeaseTerms) -> Dur {
    lease.unwrap_or(t.default).max(SECOND).min(t.max)
}

/// The first lease: `now + lease.first`, whatever `lease` the call gives (§10.1).
#[must_use]
pub fn first_lease(now: Time, t: &LeaseTerms) -> Time {
    now.plus(t.first)
}

/// The lease end after a checkpoint or a renew at `now` (§10.1, D.8.10):
/// `max(lease_until, min(now + L, cap))` with `cap = taken_at + ceiling` once the hold is
/// checkpointed, else `taken_at + first`.
#[must_use]
pub fn extended_until(h: &Hold, now: Time) -> Time {
    let cap = if h.checkpointed {
        h.taken_at.plus(h.terms.ceiling)
    } else {
        h.taken_at.plus(h.terms.first)
    };
    h.lease_until.max(now.plus(h.l).min(cap))
}

/// The fence of the next hold on a node whose fence is `f` (H3, D.8.10): `f + 1`, so a node's fence
/// only increases and each token is issued once. Pure, for the Kani harness
/// `hold_cmds_preserve_h`; it saturates at `u64::MAX`, 2⁶⁴ − 1 holds on one node, which no forum
/// reaches.
#[must_use]
pub(crate) const fn next_fence(f: Fence) -> Fence {
    Fence(f.0.saturating_add(1))
}

/// `issue_hold(n, lease, reopened)` (D.8.10): emits `HoldIssued` with the next fence, the lease
/// terms in force as a snapshot, `L = min(max(lease or lease.default, 1 s), lease.max)`, the first
/// lease `now + lease.first`, and `reopened`, the outcome of the closed node the caller's take or
/// `next` has just reopened (1.4, R4), None everywhere else. Returns the token and the lease end.
/// The taker is the executing session; a firing issues no hold.
pub fn issue_hold(
    ex: &mut Exec<'_>,
    n: NodeId,
    lease: Option<Dur>,
    reopened: Option<Outcome>,
) -> (Token, Time) {
    let now = ex.now();
    let terms = terms_of(&ex.s().params);
    let fence = next_fence(ex.s().nodes.get(&n).map_or(Fence(0), |x| x.fence));
    let token = Token { node: n, fence };
    let lease_until = first_lease(now, &terms);
    let Some(session) = ex.sess() else {
        return (token, lease_until);
    };
    ex.emit(Ev::HoldIssued {
        token,
        session,
        lease_until,
        terms,
        l: lease_l(lease, &terms),
        reopened,
    });
    (token, lease_until)
}

/// `extend` (D.8.10): a checkpoint or a renew extends the lease to `max(lease_until, min(now + L,
/// cap))`, with `cap = taken_at + ceiling` once checkpointed, else `taken_at + first`; emits
/// `HoldExtended` and returns the new lease end. A token that is not a hold emits nothing.
pub fn extend(ex: &mut Exec<'_>, token: Token, by: Extension) -> Time {
    let now = ex.now();
    let Some(h) = ex.s().holds.get(&token) else {
        return now;
    };
    let lease_until = extended_until(h, now);
    ex.emit(Ev::HoldExtended {
        token,
        lease_until,
        by,
    });
    lease_until
}

/// `bind_holder_notes(n)` (D.8.16): after every `HoldIssued` on `n`, `NoteBound` for each pending
/// `holder:` note of `n` that is still live and that the taker did not write (1.4, R12: a note never
/// reaches its sender), ascending.
pub fn bind_holder_notes(ex: &mut Exec<'_>, n: NodeId) {
    let Some(taker) = ex.sess() else {
        return;
    };
    let now = ex.now();
    let s = ex.s();
    let notes: Vec<_> =
        s.ix.pending_holder
            .get(&n)
            .into_iter()
            .flatten()
            .copied()
            .filter(|x| {
                s.notes.get(x).is_some_and(|note| note.until > now)
                    && s.entries.get(x).map(|m| m.by) != Some(Author::Session(taker))
            })
            .collect();
    for note in notes {
        ex.emit(Ev::NoteBound {
            note,
            session: taker,
        });
    }
}

/// Why `hold take` refuses (D.8.10), in its order; `can_take` is its absence.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TakeBlock {
    /// The node does not exist.
    Missing,
    /// The session already holds a live hold on it: `take` returns that hold.
    Existing(Token),
    /// Closed with no flags, merged, marked upheld (1.8), or with an open objection (1.9):
    /// `closed`.
    Closed(Option<NodeId>),
    /// A closed occurrence of an open series (`invalid(OccurrenceReopen)`).
    OccurrenceReopen,
    /// An open occurrence before its slot (`invalid(OccurrenceNotYetAvailable)`).
    NotYetAvailable,
    /// Under a pause (`paused`), with the paused scope and its end.
    Paused(NodeId, Time),
    /// The session holds `hold.per_session` slots (`holds_limit`).
    HoldsLimit,
    /// No free slot (`full`), with the node's capacity and live holds.
    Full(u32, u32),
}

/// The live holds of session `x`, ascending.
#[must_use]
pub(crate) fn live_holds_of(s: &State, x: SessionId, now: Time) -> Vec<Token> {
    s.ix.holds_by_session
        .get(&x)
        .into_iter()
        .flatten()
        .copied()
        .filter(|t| s.holds.get(t).is_some_and(|h| now < h.lease_until))
        .collect()
}

/// The session's live hold on `n`, if any.
#[must_use]
pub(crate) fn own_hold(s: &State, x: SessionId, n: NodeId, now: Time) -> Option<Token> {
    s.ix.holds_by_session
        .get(&x)
        .into_iter()
        .flatten()
        .copied()
        .find(|t| t.node == n && s.holds.get(t).is_some_and(|h| now < h.lease_until))
}

/// The nearest pause over `n` or an ancestor that has not ended, with its end (D.8.21).
#[must_use]
pub(crate) fn paused_by(s: &State, n: NodeId, now: Time) -> Option<(NodeId, Time)> {
    let mut cur = Some(n);
    let mut steps = 0_usize;
    while let Some(m) = cur {
        if let Some(p) = s.pauses.get(&m)
            && p.until > now
        {
            return Some((m, p.until));
        }
        steps = steps.saturating_add(1);
        if steps > s.nodes.len() {
            break;
        }
        cur = s.nodes.get(&m).and_then(crate::state::Node::parent);
    }
    None
}

/// `hold take`'s validation (D.8.10) after the target resolved: the first check that refuses, or
/// None when the take is possible. `can_take` and `take` share it.
pub(crate) fn take_block(s: &State, x: SessionId, n: NodeId, now: Time) -> Option<TakeBlock> {
    let Some(node) = s.nodes.get(&n) else {
        return Some(TakeBlock::Missing);
    };
    let flags =
        s.ix.node
            .get(&n)
            .map_or(crate::types::FlagSet::EMPTY, |i| i.flags);
    let (closed, merged_into, upheld) = match &node.life {
        Life::Closed(c) => (true, c.merged_into, c.upheld.is_some()),
        Life::Open => (false, None, false),
    };
    let i = TakeInputs {
        existing: own_hold(s, x, n, now),
        closed,
        flagged: !flags.is_empty(),
        merged_into,
        upheld,
        objection: flags.contains(crate::types::Flag::Objection),
        occ_locked: s.occ_locked(n),
        occ_waiting: matches!(&node.role, Role::Occurrence(o) if o.phase == OccPhase::Waiting),
        paused: paused_by(s, n, now),
        held: u32::try_from(live_holds_of(s, x, now).len()).unwrap_or(u32::MAX),
        per_session: s.params.hold_per_session,
        live: s.ix.node.get(&n).map_or(0, |i| i.live),
        capacity: node.capacity,
    };
    take_gate(&i)
}

/// What `hold take`'s validation reads of a node and the session (D.8.10).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "one input per check of D.8.10"
)]
pub(crate) struct TakeInputs {
    /// The session's live hold on the node.
    pub(crate) existing: Option<Token>,
    /// The node is closed.
    pub(crate) closed: bool,
    /// It carries a flag.
    pub(crate) flagged: bool,
    /// It was merged into this node.
    pub(crate) merged_into: Option<NodeId>,
    /// 1.8 (§10.1): closed and marked upheld.
    pub(crate) upheld: bool,
    /// 1.9 (§10.1): it carries an open objection (the `objection` flag).
    pub(crate) objection: bool,
    /// A closed occurrence of an open series (`occ_locked`).
    pub(crate) occ_locked: bool,
    /// An open occurrence before its slot.
    pub(crate) occ_waiting: bool,
    /// The nearest pause over it that has not ended.
    pub(crate) paused: Option<(NodeId, Time)>,
    /// The session's live holds.
    pub(crate) held: u32,
    /// `hold.per_session`.
    pub(crate) per_session: u32,
    /// The node's live holds.
    pub(crate) live: u32,
    /// Its capacity.
    pub(crate) capacity: u32,
}

/// `hold take`'s checks in D.8.10's order, over their inputs: the existing hold → `closed` (no
/// flag, merged, marked upheld, 1.8, or with an open objection, 1.9) → `OccurrenceReopen` → `OccurrenceNotYetAvailable` → `paused` (P2) →
/// `holds_limit` (H2) → `full` (H1). A pure function: the Kani harness `hold_cmds_preserve_h`
/// checks H1, H2, and P2 over it (D.12.5).
#[must_use]
pub(crate) fn take_gate(i: &TakeInputs) -> Option<TakeBlock> {
    if let Some(t) = i.existing {
        return Some(TakeBlock::Existing(t));
    }
    if i.closed {
        // 1.9 (§10.1): a closed node with an open objection is reviewed with `talk resolve`, not
        // reopened by a take.
        if !i.flagged || i.merged_into.is_some() || i.upheld || i.objection {
            return Some(TakeBlock::Closed(i.merged_into));
        }
        if i.occ_locked {
            return Some(TakeBlock::OccurrenceReopen);
        }
    } else if i.occ_waiting {
        return Some(TakeBlock::NotYetAvailable);
    }
    if let Some((scope, until)) = i.paused {
        return Some(TakeBlock::Paused(scope, until));
    }
    if !under_limit(i.held, i.per_session) {
        return Some(TakeBlock::HoldsLimit);
    }
    if !slot_free(i.live, i.capacity) {
        return Some(TakeBlock::Full(i.capacity, i.live));
    }
    None
}

/// A free slot: fewer live holds than the capacity (§10.1, A-65); capacity 0 is never free (H1).
#[must_use]
pub(crate) const fn slot_free(live: u32, capacity: u32) -> bool {
    live < capacity
}

/// The session holds fewer than `hold.per_session` slots (H2).
#[must_use]
pub(crate) const fn under_limit(held: u32, per_session: u32) -> bool {
    held < per_session
}

/// A renew is allowed while `renews + 1 <= renew_max` (H5): it never takes `renews` past it.
#[must_use]
pub(crate) const fn renew_allowed(renews: u32, renew_max: u32) -> bool {
    match renews.checked_add(1) {
        Some(r) => r <= renew_max,
        None => false,
    }
}

/// `can_take(x, n)` (D.8.26): `hold take`'s validation (D.8.10) as a predicate: `n` exists, it is
/// free, not paused, `x` holds fewer than `hold.per_session` slots and not `n`, and a closed `n`
/// carries a flag, was not merged, and is not marked upheld (1.8) (nor is a locked occurrence, nor
/// an occurrence before its slot). A reserved node has capacity 0, so it is never free. `take`'s own validation calls the
/// same function, and the next calls read it, so the two cannot disagree.
#[must_use]
pub fn can_take(s: &State, x: SessionId, n: NodeId, now: Time) -> bool {
    take_block(s, x, n, now).is_none()
}

/// Whether a session other than `me` holds `n` live at `now`, and `me` holds it not (1.8, §5,
/// §20.9): the node is that holder's, and so are the replies and decisions its requests are owed.
/// A close by `me` settles nothing by default, and no next call closes it, merges it away, or
/// replies on or decides its requests.
#[must_use]
pub fn held_by_another(s: &State, n: NodeId, me: Option<SessionId>, now: Time) -> bool {
    let holders = crate::query::holders(s, n, now);
    holders.iter().any(|(x, _)| Some(*x) != me) && !holders.iter().any(|(x, _)| Some(*x) == me)
}

/// 1.9 (§20.5, §21's `unmet_needs`): the labels of `needs` that no session can meet now, ascending:
/// those that no caps profile seen within `capacity.window` carries (`stats.profiles`, which every
/// session's opening and `hello` refresh with its caps, its token's default caps among them, and
/// which `Unreachable` reads too, §17), nor the caps of session `x`, nor `token`, the caller's
/// token's default caps. The core knows a token's default caps only through the sessions it
/// opens. Bounded by the profiles kept, as `Unreachable` is.
#[must_use]
pub fn unmet_needs(
    s: &State,
    now: Time,
    x: Option<SessionId>,
    token: Option<&alloc::collections::BTreeSet<crate::types::Label>>,
    needs: &alloc::collections::BTreeSet<crate::types::Label>,
) -> Vec<crate::types::Label> {
    let since = now.minus(s.params.capacity_window);
    let mine = x.and_then(|x| s.sessions.get(&x)).map(|y| &y.caps);
    needs
        .iter()
        .filter(|l| {
            !mine.is_some_and(|c| c.contains(*l))
                && !token.is_some_and(|c| c.contains(*l))
                && !s
                    .stats
                    .profiles
                    .iter()
                    .any(|((caps, _), p)| p.last_seen >= since && caps.contains(*l))
        })
        .cloned()
        .collect()
}

/// 1.9 (§10.1, §21's `unmet_needs`): the needs of node `n` that the caps of session `x` lack,
/// ascending: a take holds the node all the same, since caps are never a permission.
#[must_use]
pub fn needs_lacked(s: &State, x: SessionId, n: NodeId) -> Vec<crate::types::Label> {
    let caps = s.sessions.get(&x).map(|y| &y.caps);
    s.nodes
        .get(&n)
        .map(|node| {
            node.needs
                .iter()
                .filter(|l| !caps.is_some_and(|c| c.contains(*l)))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    reason = "test code: the deny list guards the crate's own code"
)]
mod tests {
    use super::*;
    use crate::types::MINUTE;

    fn terms() -> LeaseTerms {
        terms_of(&crate::params::Params::default())
    }

    #[test]
    fn l_is_clamped_to_a_second_and_the_max() {
        let t = terms();
        assert_eq!(lease_l(None, &t), t.default);
        assert_eq!(lease_l(Some(Dur(0)), &t), SECOND);
        assert_eq!(lease_l(Some(Dur(u64::MAX)), &t), t.max);
        assert_eq!(lease_l(Some(Dur(5 * MINUTE.0)), &t), Dur(5 * MINUTE.0));
    }

    #[test]
    fn an_extension_before_the_first_checkpoint_stops_at_the_first_lease() {
        let t = terms();
        let h = Hold {
            session: SessionId(1),
            taken_at: Time(0),
            lease_until: first_lease(Time(0), &t),
            last_extended: Time(0),
            terms: t,
            l: t.default,
            renews: 0,
            checkpointed: false,
            reopened: None,
        };
        // A renew before a checkpoint succeeds without extending (A-7).
        assert_eq!(extended_until(&h, Time(MINUTE.0)), h.lease_until);
        let c = Hold {
            checkpointed: true,
            ..h.clone()
        };
        assert_eq!(
            extended_until(&c, Time(MINUTE.0)),
            Time(MINUTE.0 + t.default.0)
        );
        // Never past the ceiling.
        let late = Time(t.ceiling.0 - MINUTE.0);
        assert_eq!(extended_until(&c, late), Time(t.ceiling.0));
    }
}
