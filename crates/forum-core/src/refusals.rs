//! The refusal predicates of D.8.14 (§10.3, CS-7), which `next`'s exclusions, the `Class(c)`
//! firing arm, and E1 read. Every time they read is stored (a profile's window end, an entry's
//! `backoff_until` and `expires_at`, a record's `quorum`), so a `ParamsChanged` changes none of
//! them (A-103).
//!
//! They read `refusals` by its key and `classes` by class, never by a scan of every record; the
//! naive forms of `spec` scan both, and the proptests compare the two (D.9.1, D.12.3).

use alloc::collections::BTreeSet;

use crate::state::{Refusal, State};
use crate::types::{ClassName, NodeId, Time};

/// `live(r, now)`: the entries of a record that have not expired.
fn live_entries(r: &Refusal, now: Time) -> usize {
    r.by.values().filter(|x| x.expires_at > now).count()
}

/// `class_refused(n, c, now)`: the record of `(n, c)` exists and has at least its `quorum` live
/// entries (`expires_at > now`).
#[must_use]
pub fn class_refused(s: &State, n: NodeId, c: &ClassName, now: Time) -> bool {
    s.refusals
        .get(&(n, c.clone()))
        .is_some_and(|r| live_entries(r, now) >= usize::try_from(r.quorum).unwrap_or(usize::MAX))
}

/// `backed_off(n, c, now)`: the record of `(n, c)` exists and some entry has `backoff_until > now`.
#[must_use]
pub fn backed_off(s: &State, n: NodeId, c: &ClassName, now: Time) -> bool {
    s.refusals
        .get(&(n, c.clone()))
        .is_some_and(|r| r.by.values().any(|x| x.backoff_until > now))
}

/// `excluded(n, c, now) := class_refused ∨ backed_off`: what `next` skips.
#[must_use]
pub fn excluded(s: &State, n: NodeId, c: &ClassName, now: Time) -> bool {
    class_refused(s, n, c, now) || backed_off(s, n, c, now)
}

/// `active(n, now)`: the classes with a caps profile ⊇ `n.needs` whose window ends after `now`.
#[must_use]
pub fn active(s: &State, n: NodeId, now: Time) -> BTreeSet<ClassName> {
    let Some(node) = s.nodes.get(&n) else {
        return BTreeSet::new();
    };
    s.classes
        .iter()
        .filter(|(_, a)| {
            a.caps_seen
                .iter()
                .any(|(caps, end)| *end > now && node.needs.is_subset(caps))
        })
        .map(|(c, _)| c.clone())
        .collect()
}

/// `refusers(n, now)`: the classes that have refused `n` (through `ix.refused`).
fn refusers(s: &State, n: NodeId, now: Time) -> BTreeSet<ClassName> {
    s.ix.refused
        .get(&n)
        .into_iter()
        .flatten()
        .filter(|c| class_refused(s, n, c, now))
        .cloned()
        .collect()
}

/// `all_refused(n, now) := open(n) ∧ refusers(n, now) ≠ ∅ ∧ active(n, now) ⊆ refusers(n, now)`.
#[must_use]
pub fn all_refused(s: &State, n: NodeId, now: Time) -> bool {
    if !s.nodes.get(&n).is_some_and(crate::state::Node::is_open) {
        return false;
    }
    let r = refusers(s, n, now);
    !r.is_empty() && active(s, n, now).is_subset(&r)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "test code: the deny list guards the crate's own code"
)]
mod tests {
    use super::*;
    use crate::event::Ev;
    use crate::params::Params;
    use crate::spec;
    use crate::testkit::World;
    use crate::types::{Label, SessionId};

    fn class(name: &str) -> ClassName {
        ClassName::new(name).unwrap()
    }

    /// The predicates agree with `spec`'s scans on hand-built records: a backoff alone, a quorum
    /// reached and then lost entry by entry, and the active classes by their caps profiles.
    #[test]
    fn the_predicates_agree_with_spec() {
        let mut w = World::new(
            Params {
                refusal_quorum: 2,
                ..Params::default()
            },
            Time(1_000_000),
        );
        let n = w.plain_node("a");
        let (worker, other) = (class("w"), class("o"));
        let s1 = w.open(&worker);
        let s2 = w.open(&worker);
        let now = w.clock;
        let refuse = |w: &mut World, sess: SessionId, life: u64| {
            let at = w.clock;
            w.emit(Ev::Refused {
                node: n,
                class: worker.clone(),
                session: sess,
                backoff_until: Time(at.0 + 1_000),
                expires_at: Time(at.0 + life),
                quorum: 2,
            });
        };
        refuse(&mut w, s1, 5_000);
        let check = |w: &World, t: Time| {
            for c in [&worker, &other] {
                assert_eq!(
                    class_refused(&w.s, n, c, t),
                    spec::class_refused(&w.s, n, c, t)
                );
                assert_eq!(backed_off(&w.s, n, c, t), spec::backed_off(&w.s, n, c, t));
                assert_eq!(excluded(&w.s, n, c, t), spec::excluded(&w.s, n, c, t));
            }
            assert_eq!(active(&w.s, n, t), spec::active(&w.s, n, t));
            assert_eq!(all_refused(&w.s, n, t), spec::all_refused(&w.s, n, t));
        };
        check(&w, now);
        assert!(backed_off(&w.s, n, &worker, now));
        assert!(!class_refused(&w.s, n, &worker, now));
        refuse(&mut w, s2, 9_000);
        check(&w, now);
        assert!(class_refused(&w.s, n, &worker, now));
        // No class is active: one class reaching its quorum is enough (§10.3).
        assert!(all_refused(&w.s, n, now));
        // The first entry expires: below the quorum again.
        check(&w, Time(now.0 + 6_000));
        assert!(!class_refused(&w.s, n, &worker, Time(now.0 + 6_000)));
        // An active class that has not refused keeps E1 false.
        let s3 = w.open(&other);
        w.emit(Ev::NextCalled {
            session: s3,
            outcome: crate::event::NextOutcome::Nothing,
            open: true,
            class_window_end: Time(w.clock.0 + 100_000),
        });
        check(&w, w.clock);
        assert!(!all_refused(&w.s, n, w.clock));
        assert_eq!(active(&w.s, n, w.clock), [other.clone()].into());
        // A class whose only profile misses the node's needs is not active.
        let mut needs = BTreeSet::new();
        needs.insert(Label::new("x").unwrap());
        w.set_needs(n, needs);
        check(&w, w.clock);
        assert!(active(&w.s, n, w.clock).is_empty());
    }
}
