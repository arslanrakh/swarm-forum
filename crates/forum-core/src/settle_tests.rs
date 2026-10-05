//! Unit tests of `settle` and the firing arms of WP-D3 (design D.8.2, D.12.1): each firing runs
//! through `World::settle`, which checks it with `check_fire` and its dirty set.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::panic,
    clippy::wildcard_enum_match_arm,
    clippy::needless_range_loop,
    reason = "test code: the deny list guards the crate's own code"
)]

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use super::*;
use crate::apply::tests::{
    class, close, create, create_with, created, entry, ok, release, text, world,
};
use crate::cmd::{Cmd, ControlCmd, HoldCmd};
use crate::event::{EntryBody, NextOutcome};
use crate::state::{Author, DeadlineKind, Owner};
use crate::testkit::World;
use crate::types::{Dur, KeyStr, NodeRef, Outcome, ReleaseReason, SaidKind};

/// A lease that ends fires `HoldEnded(Expired)`: the `offer` tag, and an attempt for a hold with no
/// checkpoint (D.8.2, A-9).
#[test]
fn a_lease_expires() {
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    let (n, t) = created(&ok(w.call(&c, s, create("/a"))));
    let t = t.unwrap();
    let end = w.s.holds[&t].lease_until;
    w.at(Time(end.0 - 1));
    assert!(w.settle().is_empty());
    w.at(end);
    let evs = w.settle();
    assert!(matches!(
        evs.as_slice(),
        [
            Event {
                ev: Ev::Fired {
                    kind: DeadlineKind::Lease,
                    ..
                },
                ..
            },
            Event {
                ev: Ev::HoldEnded {
                    end: HoldEnd::Expired,
                    ..
                },
                ..
            },
            ..
        ]
    ));
    assert!(w.s.holds.is_empty());
    assert_eq!(w.s.nodes[&n].attempts, 1);
    assert!(w.s.nodes[&n].tags.iter().any(|l| l.as_str() == "offer"));
}

/// A rescheduled deadline fires only at its new time: a checkpoint moves the Lease key, and at the
/// old time nothing fires (T4).
#[test]
fn a_rescheduled_deadline_fires_at_its_new_time() {
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    let (_, t) = created(&ok(w.call(&c, s, create("/a"))));
    let t = t.unwrap();
    let old = w.s.holds[&t].lease_until;
    ok(w.call(&c, s, crate::apply::tests::checkpoint(t)));
    let new = w.s.holds[&t].lease_until;
    assert!(new > old);
    w.at(old);
    let evs = w.settle();
    assert!(
        !evs.iter().any(|e| matches!(e.ev, Ev::HoldEnded { .. })),
        "{evs:?}"
    );
    w.at(new);
    let evs = w.settle();
    assert!(evs.iter().any(|e| matches!(e.ev, Ev::HoldEnded { .. })));
}

/// Deadlines fire in `(at, kind, owner)` order (§17): at one instant a lease before a pause before
/// a session's silence; each event carries the deadline's own time.
#[test]
fn deadlines_fire_in_kind_order() {
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    let (n, t) = created(&ok(w.call(&c, s, create("/a"))));
    let t = t.unwrap();
    // A pause that ends exactly when the lease does.
    let lease = w.s.holds[&t].lease_until;
    let ttl = lease.since(w.clock);
    let mut p = w.s.params.clone();
    p.pause_ttl = ttl;
    let ctx = w.ctx(&c, crate::cmd::SessionCtx::System, None);
    ok(w.call_ctx(&ctx, &Cmd::Admin(crate::cmd::AdminCmd::SetParams(p))));
    ok(w.call(
        &c,
        s,
        Cmd::Control(ControlCmd::Pause {
            scope: NodeRef::Id(n),
            reason: text("r"),
        }),
    ));
    w.at(Time(lease.0 + 10_000_000));
    let evs = w.settle();
    let kinds: Vec<DeadlineKind> = evs
        .iter()
        .filter_map(|e| match &e.ev {
            Ev::Fired { kind, .. } => Some(*kind),
            _ => None,
        })
        .collect();
    let at_lease: Vec<DeadlineKind> = evs
        .iter()
        .filter(|e| e.at == lease)
        .filter_map(|e| match &e.ev {
            Ev::Fired { kind, .. } => Some(*kind),
            _ => None,
        })
        .collect();
    // The session's silence falls at its last lease's end (1.4, R27): last, by kind.
    assert_eq!(
        at_lease,
        vec![
            DeadlineKind::Lease,
            DeadlineKind::Pause,
            DeadlineKind::Silence
        ]
    );
    assert!(kinds.contains(&DeadlineKind::Silence));
    assert!(evs.windows(2).all(|x| x[0].at <= x[1].at));
}

/// A session's silence makes its pending `said` entries unconfirmed (§9.2), in ascending order.
#[test]
fn silence_unconfirms_pending_saids() {
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    let (n, _) = created(&ok(w.call(
        &c,
        s,
        create_with("/a", |a| a.take = Some(false)),
    )));
    let conv = KeyStr::new("chat:x/1").unwrap();
    let said = |w: &mut World| {
        entry(
            w,
            n,
            Author::Session(s),
            EntryBody::Said {
                conv: conv.clone(),
                kind: SaidKind::Reply,
                corrects: None,
                voice: false,
            },
        )
    };
    let a = said(&mut w);
    let confirmed = said(&mut w);
    w.emit(Ev::SaidConfirmed {
        said: confirmed,
        via: crate::event::ConfirmVia::Explicit,
        link: None,
    });
    let b = said(&mut w);
    let silence = w.s.sessions[&s].silence_at;
    w.at(silence);
    let evs = w.settle();
    let un: Vec<_> = evs
        .iter()
        .filter_map(|e| match e.ev {
            Ev::SaidUnconfirmed { said } => Some(said),
            _ => None,
        })
        .collect();
    assert_eq!(un, vec![a, b]);
    assert!(w.s.sessions[&s].silent);
    // Live again, a new `said`, and silent again: only the new one, not those already unconfirmed.
    ok(w.call(&c, s, Cmd::View(crate::cmd::ViewCmd::Constitution)));
    let d = said(&mut w);
    let silence = w.s.sessions[&s].silence_at;
    w.at(silence);
    let evs = w.settle();
    let un: Vec<_> = evs
        .iter()
        .filter_map(|e| match e.ev {
            Ev::SaidUnconfirmed { said } => Some(said),
            _ => None,
        })
        .collect();
    assert_eq!(un, vec![d]);
}

/// Refusal entries expire one by one at their own times (CS-7); a class's caps profile leaves at
/// its window's end, and the class with its last profile.
#[test]
fn refusals_and_class_profiles_expire() {
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    let (n, t) = created(&ok(w.call(&c, s, create("/a"))));
    ok(w.call(&c, s, release(t.unwrap(), ReleaseReason::Cannot)));
    ok(w.call(
        &c,
        s,
        Cmd::Hold(HoldCmd::Next {
            query: crate::cmd::NextQuery::default(),
            lease: None,
        }),
    ));
    assert!(w.s.classes.contains_key(&c));
    assert!(w.last_events.iter().any(|e| matches!(
        e.ev,
        Ev::NextCalled {
            outcome: NextOutcome::NothingForYou,
            open: true,
            ..
        }
    )));
    // Both end 7 days on: the refusal entry (refusal.ttl) and the profile (refusal.window).
    let ttl = w.s.params.refusal_ttl;
    let evs = w.advance(ttl);
    assert!(
        evs.iter()
            .any(|e| matches!(e.ev, Ev::RefusalExpired { node, .. } if node == n))
    );
    assert!(w.s.refusals.is_empty());
    assert!(evs.iter().any(|e| matches!(
        e.ev,
        Ev::Fired {
            kind: DeadlineKind::RefusalExpiry,
            owner: Owner::Class(_)
        }
    )));
    assert!(w.s.classes.is_empty());
}

/// Settling again changes nothing (idempotence), and settling to `t1` then `t2` equals settling to
/// `t2` (T2, path independence), over a world with leases, pauses, silences, and retirements.
#[test]
fn settling_is_idempotent_and_path_independent() {
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    let s2 = w.hello(&c);
    let (n, _) = created(&ok(w.call(&c, s, create("/a"))));
    created(&ok(w.call(&c, s2, create("/b"))));
    ok(w.call(
        &c,
        s2,
        Cmd::Control(ControlCmd::Pause {
            scope: NodeRef::Id(n),
            reason: text("r"),
        }),
    ));
    let base = w.s.clone();
    let horizon = w.clock.plus(Dur(2 * 86_400_000));
    let mut direct = base.clone();
    let (all, _) = settle(&mut direct, horizon, u32::MAX);
    let mut again = direct.clone();
    let (none, st) = settle(&mut again, horizon, u32::MAX);
    assert!(none.is_empty());
    assert_eq!(st, crate::reply::Settled::Done);
    assert_eq!(again, direct);
    for k in [1_u64, 7, 60, 900, 3_600, 7_200, 86_400] {
        let t1 = w.clock.plus(Dur(k * 1_000));
        let mut stepped = base.clone();
        let (a, _) = settle(&mut stepped, t1, u32::MAX);
        let (b, _) = settle(&mut stepped, horizon, u32::MAX);
        assert_eq!(stepped, direct, "t1 = {t1:?}");
        let mut evs = a;
        evs.extend(b);
        assert_eq!(evs, all);
    }
    // Batches split the firings only.
    let mut batched = base.clone();
    let mut evs = Vec::new();
    loop {
        let (e, st) = settle(&mut batched, horizon, 2);
        evs.extend(e);
        if st == crate::reply::Settled::Done {
            break;
        }
    }
    assert_eq!(batched, direct);
    assert_eq!(evs, all);
    assert_eq!(settle_events(&base, horizon), all);
    let _ = BTreeSet::<u8>::new();
    let _ = close(n, Outcome::Done);
}
