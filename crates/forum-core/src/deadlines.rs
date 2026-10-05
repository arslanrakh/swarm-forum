//! The deadline queue as a function of the base state (design D.4.8, D.0.2 principle 4, T4).
//!
//! Every deadline owner has a pure `deadline_of`, which returns `None` once the deadline has fired:
//! its transition happened, or its owner's one-shot marker is set. A cancelled deadline is one whose
//! owner changed; it is no longer in the recount, so it cannot fire. `evolve` keeps `ix.deadlines`
//! and `ix.deadline_at` equal to [`recount_deadlines`] by re-reading `deadline_of` for the owners an
//! event touched (D.7.4).

use alloc::collections::{BTreeMap, BTreeSet};

use crate::state::{
    CommitmentState, DeadlineKey, DeadlineKind, Life, OccPhase, Owner, PqState, Role, State,
    UqState, WaitOn,
};
use crate::types::{SessionId, Time};

/// The kinds a deadline owner can have (D.4.8).
#[must_use]
pub fn kinds_of(owner: &Owner) -> &'static [DeadlineKind] {
    match owner {
        Owner::Hold(_) => &[DeadlineKind::Lease],
        Owner::Node(_) => &[
            DeadlineKind::Wait,
            DeadlineKind::OccurrenceSlot,
            DeadlineKind::OccurrenceWindow,
            DeadlineKind::Due,
            DeadlineKind::Pause,
            DeadlineKind::Stale,
        ],
        Owner::Entry(_) => &[
            DeadlineKind::Recheck,
            DeadlineKind::UserQuestion,
            DeadlineKind::Commitment,
        ],
        Owner::Refusal(..) | Owner::Class(_) => &[DeadlineKind::RefusalExpiry],
        Owner::Session(_) => &[DeadlineKind::Silence, DeadlineKind::Retire],
    }
}

/// The deadline of `owner` of kind `kind`, if one is queued (the table of D.4.8). A kind that the
/// owner cannot have gives `None`.
///
/// The Silence key of a session is `max(silence_at, the largest lease_until of the holds it took,
/// last_now)` (1.4, §3, R27): a session never falls silent while a hold it took is live, and the
/// key never lies before the event that ended its last hold (D.4.8). The holds are found through
/// `ix.holds_by_session`, which R1 recounts over `holds`.
#[must_use]
pub fn deadline_of(s: &State, kind: DeadlineKind, owner: &Owner) -> Option<Time> {
    match (kind, owner) {
        (DeadlineKind::Lease, Owner::Hold(t)) => s.holds.get(t).map(|h| h.lease_until),
        (DeadlineKind::Wait, Owner::Node(n)) => {
            let node = s.nodes.get(n)?;
            if !node.is_open() {
                return None;
            }
            let w = node.wait.as_ref()?;
            match w.on {
                WaitOn::Time(t) | WaitOn::External(t) if !w.fired => Some(t),
                WaitOn::Time(_)
                | WaitOn::External(_)
                | WaitOn::Question { .. }
                | WaitOn::UserQuestion(_)
                | WaitOn::Node(_) => None,
            }
        }
        (DeadlineKind::Recheck, Owner::Entry(q)) => s
            .pquestions
            .get(q)
            .filter(|pq| pq.state == PqState::Open)
            .map(|pq| pq.recheck_at),
        (DeadlineKind::UserQuestion, Owner::Entry(q)) => s
            .uquestions
            .get(q)
            .filter(|uq| uq.state == UqState::Pending)
            .map(|uq| uq.due),
        (DeadlineKind::Commitment, Owner::Entry(c)) => s
            .commitments
            .get(c)
            .filter(|c| c.state == CommitmentState::Open && !c.followup_fired)
            .map(|c| c.followup_at),
        (DeadlineKind::OccurrenceSlot, Owner::Node(n)) => {
            let node = s.nodes.get(n)?;
            match (&node.life, &node.role) {
                (Life::Open, Role::Occurrence(o)) if o.phase == OccPhase::Waiting => Some(o.slot),
                _ => None,
            }
        }
        (DeadlineKind::OccurrenceWindow, Owner::Node(n)) => {
            let node = s.nodes.get(n)?;
            match (&node.life, &node.role) {
                (Life::Open, Role::Occurrence(o))
                    if o.phase == OccPhase::Available && !o.window_fired =>
                {
                    Some(o.window_end)
                }
                _ => None,
            }
        }
        (DeadlineKind::RefusalExpiry, Owner::Refusal(n, c)) => s
            .refusals
            .get(&(*n, c.clone()))
            .and_then(|r| r.by.values().map(|x| x.expires_at).min()),
        (DeadlineKind::RefusalExpiry, Owner::Class(c)) => s
            .classes
            .get(c)
            .and_then(|a| a.caps_seen.values().copied().min()),
        (DeadlineKind::Due, Owner::Node(n)) => {
            let node = s.nodes.get(n)?;
            if node.is_open() && !node.due_fired {
                node.due
            } else {
                None
            }
        }
        (DeadlineKind::Pause, Owner::Node(n)) => s.pauses.get(n).map(|p| p.until),
        (DeadlineKind::Stale, Owner::Node(n)) => s.nodes.get(n).and_then(|x| stale_at(s, x)),
        (DeadlineKind::Silence, Owner::Session(x)) => {
            let session = s.sessions.get(x)?;
            if session.silent {
                return None;
            }
            let leases =
                s.ix.holds_by_session
                    .get(x)
                    .into_iter()
                    .flatten()
                    .filter_map(|t| s.holds.get(t))
                    .map(|h| h.lease_until)
                    .max()
                    .unwrap_or(Time::ZERO);
            Some(session.silence_at.max(leases).max(s.last_now))
        }
        (DeadlineKind::Retire, Owner::Session(x)) => s.sessions.get(x).map(|x| x.retire_at),
        (
            DeadlineKind::Lease
            | DeadlineKind::Wait
            | DeadlineKind::Recheck
            | DeadlineKind::UserQuestion
            | DeadlineKind::Commitment
            | DeadlineKind::OccurrenceSlot
            | DeadlineKind::OccurrenceWindow
            | DeadlineKind::RefusalExpiry
            | DeadlineKind::Due
            | DeadlineKind::Pause
            | DeadlineKind::Silence
            | DeadlineKind::Retire
            | DeadlineKind::Stale,
            _,
        ) => None,
    }
}

/// The Stale key of a node (1.8, §14, D.4.8): `touched_at + offer.stale` while the node is open,
/// is not reserved, and its marker is not set. Whether the node is an offer at all is the flag's
/// question, not the queue's: a tag or a role that changes without a touch then never finds its
/// key in the past (T4).
#[must_use]
pub fn stale_at(s: &State, node: &crate::state::Node) -> Option<Time> {
    if !node.is_open() || node.origin == crate::state::Origin::Reserved || node.stale_fired {
        return None;
    }
    s.params.stale_at(node.touched_at)
}

/// The queue recount: every owner's deadline, from the base state alone (the holds of a session by
/// a pass over `holds`, not through `ix.holds_by_session`). T4 compares it with `ix.deadlines`.
#[must_use]
pub fn recount_deadlines(s: &State) -> BTreeSet<DeadlineKey> {
    let mut out = BTreeSet::new();
    let mut put = |at: Option<Time>, kind: DeadlineKind, owner: Owner| {
        if let Some(at) = at {
            out.insert(DeadlineKey { at, kind, owner });
        }
    };
    for (t, h) in &s.holds {
        put(Some(h.lease_until), DeadlineKind::Lease, Owner::Hold(*t));
    }
    for (id, node) in &s.nodes {
        if let (Life::Open, Some(w)) = (&node.life, &node.wait)
            && let WaitOn::Time(t) | WaitOn::External(t) = w.on
            && !w.fired
        {
            put(Some(t), DeadlineKind::Wait, Owner::Node(*id));
        }
        if let (Life::Open, Role::Occurrence(o)) = (&node.life, &node.role) {
            match o.phase {
                OccPhase::Waiting => {
                    put(Some(o.slot), DeadlineKind::OccurrenceSlot, Owner::Node(*id))
                }
                OccPhase::Available if !o.window_fired => put(
                    Some(o.window_end),
                    DeadlineKind::OccurrenceWindow,
                    Owner::Node(*id),
                ),
                OccPhase::Available => {}
            }
        }
        if node.is_open() && !node.due_fired {
            put(node.due, DeadlineKind::Due, Owner::Node(*id));
        }
        put(stale_at(s, node), DeadlineKind::Stale, Owner::Node(*id));
    }
    for (q, pq) in &s.pquestions {
        if pq.state == PqState::Open {
            put(Some(pq.recheck_at), DeadlineKind::Recheck, Owner::Entry(*q));
        }
    }
    for (q, uq) in &s.uquestions {
        if uq.state == UqState::Pending {
            put(Some(uq.due), DeadlineKind::UserQuestion, Owner::Entry(*q));
        }
    }
    for (c, x) in &s.commitments {
        if x.state == CommitmentState::Open && !x.followup_fired {
            put(
                Some(x.followup_at),
                DeadlineKind::Commitment,
                Owner::Entry(*c),
            );
        }
    }
    for ((n, c), r) in &s.refusals {
        put(
            r.by.values().map(|x| x.expires_at).min(),
            DeadlineKind::RefusalExpiry,
            Owner::Refusal(*n, c.clone()),
        );
    }
    for (c, a) in &s.classes {
        put(
            a.caps_seen.values().copied().min(),
            DeadlineKind::RefusalExpiry,
            Owner::Class(c.clone()),
        );
    }
    for (p, x) in &s.pauses {
        put(Some(x.until), DeadlineKind::Pause, Owner::Node(*p));
    }
    let mut leases: BTreeMap<SessionId, Time> = BTreeMap::new();
    for h in s.holds.values() {
        let e = leases.entry(h.session).or_insert(Time::ZERO);
        *e = (*e).max(h.lease_until);
    }
    for (id, x) in &s.sessions {
        if !x.silent {
            let lease = leases.get(id).copied().unwrap_or(Time::ZERO);
            put(
                Some(x.silence_at.max(lease).max(s.last_now)),
                DeadlineKind::Silence,
                Owner::Session(*id),
            );
        }
        put(Some(x.retire_at), DeadlineKind::Retire, Owner::Session(*id));
    }
    out
}

/// `ix.deadline_at` as the queue determines it: `(kind, owner) -> at` for every queued key.
#[must_use]
pub fn deadline_at_of(queue: &BTreeSet<DeadlineKey>) -> BTreeMap<(DeadlineKind, Owner), Time> {
    queue
        .iter()
        .map(|k| ((k.kind, k.owner.clone()), k.at))
        .collect()
}

/// Re-reads `deadline_of(kind, owner)` and moves, inserts, or removes its key in `ix.deadlines` and
/// `ix.deadline_at` (the deadline sync of D.7.4).
pub(crate) fn sync(s: &mut State, kind: DeadlineKind, owner: &Owner) {
    let new = deadline_of(s, kind, owner);
    let slot = (kind, owner.clone());
    let old = s.ix.deadline_at.get(&slot).copied();
    if old == new {
        return;
    }
    if let Some(at) = old {
        s.ix.deadlines.remove(&DeadlineKey {
            at,
            kind,
            owner: owner.clone(),
        });
        s.ix.deadline_at.remove(&slot);
    }
    if let Some(at) = new {
        s.ix.deadlines.insert(DeadlineKey {
            at,
            kind,
            owner: owner.clone(),
        });
        s.ix.deadline_at.insert(slot, at);
    }
}

/// The deadline sync for every kind of `owner`.
pub(crate) fn sync_owner(s: &mut State, owner: &Owner) {
    for &kind in kinds_of(owner) {
        sync(s, kind, owner);
    }
}
