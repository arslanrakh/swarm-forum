//! Settling and deadline firing (design D.8.2, §17, T2, T4).
//!
//! `settle(s, now, max)` fires the queued deadlines with `at <= now` in `(at, kind, owner)` order,
//! at most `max` of them, each with `now :=` its own time: `Fired`, then the effects of its kind
//! (the table of D.8.2). A firing always removes its own key, because its transition or marker
//! makes `deadline_of` return `None`; a key still queued after its own firing is a core bug, and
//! `settle` returns `Stuck(k)` rather than popping it for ever.
//!
//! The arms are WP-D3's for Lease, Wait, Commitment, Due, Pause, Silence, Retire, and both forms of
//! RefusalExpiry (D.13.1). The arm of `Class(c)` calls `e1_check`, WP-D5a's; the UserQuestion arm
//! is WP-D5a's (`helpers::user::fire_question`), Recheck WP-D5b's (`helpers::people::recheck`), and
//! OccurrenceSlot and OccurrenceWindow WP-D6's (`helpers::recurring`).

use alloc::vec::Vec;

use crate::event::{EntryBody, EntryText, Ev, Event, HoldEnd, SystemNote};
use crate::facts::Tracked;
use crate::helpers::Exec;
use crate::helpers::recurring::{maybe_missed, occurrence_slot};
use crate::helpers::user::{e1_check, fire_question};
use crate::reply::Settled;
use crate::state::{Author, DeadlineKey, DeadlineKind, Owner, Role, SaidState, State};
use crate::types::{NodeId, Time};

/// Fires the deadlines with `at <= now` in `(at, kind, owner)` order, at most `max` of them
/// (`settle.batch`), each with `now :=` its deadline; `now` is first clamped to `s.last_now` (T1).
/// Returns the events, and whether the state is settled at `now` (`Done`), has due deadlines left
/// for the next batch (`More`), or kept a key after its own firing (`Stuck`, D.8.2).
pub fn settle(s: &mut State, now: Time, max: u32) -> (Vec<Event>, Settled) {
    run(s, now, max, None)
}

/// `settle`, adding to `dirty` every fact its events change (D.6.1, FQ-2).
pub fn settle_tracked(
    s: &mut State,
    now: Time,
    max: u32,
    dirty: &mut Tracked,
) -> (Vec<Event>, Settled) {
    run(s, now, max, Some(dirty))
}

/// §24's pure form: every deadline event `settle` would emit at `now`, with no batch limit, on a
/// copy of `s`. A `Stuck` key ends the list (D.8.2).
#[must_use]
pub fn settle_events(s: &State, now: Time) -> Vec<Event> {
    let mut t = s.clone();
    let mut out = Vec::new();
    loop {
        let (evs, st) = settle(&mut t, now, u32::MAX);
        out.extend(evs);
        match st {
            Settled::More => {}
            Settled::Done | Settled::Stuck(_) => return out,
        }
    }
}

/// What the settle loop reads and does (D.8.2): the queue's minimum, its time, firing a key, and
/// whether a key is still queued. `drain` is the loop; the state's queue is one `Queue`, and the
/// Kani harnesses of T1 to T4 run the same loop over a small model queue (D.12.5).
pub(crate) trait Queue {
    /// A key of the queue, ordered as the queue fires them.
    type Key: Clone + Ord;
    /// The queue's minimum.
    fn first(&self) -> Option<Self::Key>;
    /// The time a key is due.
    fn at(k: &Self::Key) -> Time;
    /// Fires `k` at its time: its `Fired` and its kind's effects.
    fn fire(&mut self, k: &Self::Key);
    /// Whether `k` is queued.
    fn queued(&self, k: &Self::Key) -> bool;
}

/// What `drain` left: settled, more due keys past the batch, or a key still queued after its own
/// firing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum Drained<K> {
    /// No key is due.
    Done,
    /// Due keys remain for the next batch.
    More,
    /// This key is still queued after its own firing.
    Stuck(K),
}

/// The loop of D.8.2: fires the minimum while it is due, at most `max` keys; `Stuck(k)` when a key
/// is still queued after its own firing.
pub(crate) fn drain<Q: Queue>(q: &mut Q, now: Time, max: u32) -> Drained<Q::Key> {
    let mut fired: u32 = 0;
    while fired < max {
        let Some(k) = q.first() else {
            return Drained::Done;
        };
        if Q::at(&k) > now {
            return Drained::Done;
        }
        q.fire(&k);
        if q.queued(&k) {
            return Drained::Stuck(k);
        }
        fired = fired.saturating_add(1);
    }
    if q.first().is_none_or(|k| Q::at(&k) > now) {
        Drained::Done
    } else {
        Drained::More
    }
}

/// The state's queue: each firing through an `Exec` at the key's own time, its events collected.
struct StateQueue<'a> {
    s: &'a mut State,
    dirty: Option<&'a mut Tracked>,
    events: Vec<Event>,
}

impl Queue for StateQueue<'_> {
    type Key = DeadlineKey;

    fn first(&self) -> Option<DeadlineKey> {
        self.s.ix.deadlines.first().cloned()
    }

    fn at(k: &DeadlineKey) -> Time {
        k.at
    }

    fn fire(&mut self, k: &DeadlineKey) {
        let mut ex = Exec::tracked(self.s, k.at, None, self.dirty.as_deref_mut());
        ex.emit(Ev::Fired {
            kind: k.kind,
            owner: k.owner.clone(),
        });
        fire(&mut ex, k);
        let (evs, _) = ex.finish();
        self.events.extend(evs);
    }

    fn queued(&self, k: &DeadlineKey) -> bool {
        self.s.ix.deadlines.contains(k)
    }
}

fn run(s: &mut State, now: Time, max: u32, dirty: Option<&mut Tracked>) -> (Vec<Event>, Settled) {
    let now = now.max(s.last_now);
    let mut q = StateQueue {
        s,
        dirty,
        events: Vec::new(),
    };
    let st = match drain(&mut q, now, max) {
        Drained::Done => Settled::Done,
        Drained::More => Settled::More,
        Drained::Stuck(k) => Settled::Stuck(k),
    };
    (q.events, st)
}

/// The effects of a firing after its `Fired`, all at `k.at` (the table of D.8.2).
pub(crate) fn fire(ex: &mut Exec<'_>, k: &DeadlineKey) {
    match (k.kind, &k.owner) {
        (DeadlineKind::Lease, Owner::Hold(t)) => {
            ex.emit(Ev::HoldEnded {
                token: *t,
                end: HoldEnd::Expired,
            });
            if is_occurrence(ex.s(), t.node) {
                maybe_missed(ex, t.node);
            }
        }
        // Fired sets the marker; the wait, the commitment's follow-up, and the due resolve by
        // predicate.
        (DeadlineKind::Wait | DeadlineKind::Due | DeadlineKind::Stale, Owner::Node(_))
        | (DeadlineKind::Commitment, Owner::Entry(_)) => {}
        // WP-D6's arms (D.13.1): the copy at the slot and availability (1.4, R10), and the missed
        // check at the window's end (`Fired` has set `window_fired`).
        (DeadlineKind::OccurrenceSlot, Owner::Node(o)) => occurrence_slot(ex, *o),
        (DeadlineKind::OccurrenceWindow, Owner::Node(o)) => maybe_missed(ex, *o),
        // A default, the next stage, or the lapse (WP-D5a, CS-2).
        (DeadlineKind::UserQuestion, Owner::Entry(uq)) => fire_question(ex, *uq),
        (DeadlineKind::RefusalExpiry, Owner::Refusal(n, c)) => ex.emit(Ev::RefusalExpired {
            node: *n,
            class: c.clone(),
        }),
        (DeadlineKind::RefusalExpiry, Owner::Class(_)) => {
            // Fired has removed the class's caps profiles whose window ended (CS-7).
            let refused: Vec<NodeId> = ex.s().ix.refused.keys().copied().collect();
            for n in refused {
                e1_check(ex, n);
            }
        }
        (DeadlineKind::Pause, Owner::Node(p)) => {
            ex.emit(Ev::PauseExpired { scope: *p });
            ex.append(
                *p,
                Author::Server,
                EntryBody::System(SystemNote::PauseExpired),
                EntryText::default(),
            );
        }
        (DeadlineKind::Silence, Owner::Session(x)) => {
            let pending: Vec<_> = ex
                .s()
                .ix
                .open_saids_by
                .get(x)
                .into_iter()
                .flatten()
                .copied()
                .filter(|a| {
                    ex.s()
                        .saids
                        .get(a)
                        .is_some_and(|y| y.state == SaidState::Pending)
                })
                .collect();
            for said in pending {
                ex.emit(Ev::SaidUnconfirmed { said });
            }
        }
        (DeadlineKind::Retire, Owner::Session(x)) => {
            ex.emit(Ev::SessionRetired { session: *x });
        }
        // WP-D5b's arm (D.13.1): a question to a person is rechecked, expires, or escalates.
        (DeadlineKind::Recheck, Owner::Entry(q)) => crate::helpers::people::recheck(ex, *q),
        // Pairs of a kind and an owner that no deadline has.
        (
            DeadlineKind::Recheck
            | DeadlineKind::UserQuestion
            | DeadlineKind::OccurrenceSlot
            | DeadlineKind::Lease
            | DeadlineKind::Wait
            | DeadlineKind::Commitment
            | DeadlineKind::OccurrenceWindow
            | DeadlineKind::RefusalExpiry
            | DeadlineKind::Due
            | DeadlineKind::Pause
            | DeadlineKind::Silence
            | DeadlineKind::Retire
            | DeadlineKind::Stale,
            _,
        ) => {}
    }
}

fn is_occurrence(s: &State, n: NodeId) -> bool {
    s.nodes
        .get(&n)
        .is_some_and(|x| matches!(x.role, Role::Occurrence(_)))
}

#[cfg(test)]
#[path = "settle_tests.rs"]
mod tests;
