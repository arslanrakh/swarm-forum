//! The harnesses of WP-D3 (design D.12.5) for the key invariants it owns (D.0.3): H1 to H6 (holds
//! and fencing), O1 (retries), T1 to T5 (time and cycles), and the order of `next`'s keys.
//!
//! **Narrowed** (D.12.5), and recorded for `docs/verification.md`. The end-to-end forms (a command
//! or a settle on a symbolic `State`) do not leave CBMC's symbolic execution within the CI budget:
//! every `BTreeMap` operation is expensive for CBMC, as WP-D2 found for `evolve`. So each harness
//! checks the pure function that enforces its property, which the production code calls, over all
//! its inputs; the end-to-end form is the property test `tests/prop.rs` (every step checked with
//! `check_step` and `check_fire`, the state with `check_invariants`) and the unit tests:
//! - `lease_issue_h5`, `lease_extend_h5`, `renew_limit` (H5): `lease_l`, `first_lease`,
//!   `extended_until`, and `renew_allowed`, which `issue_hold`, `extend`, and `renew`'s validation
//!   call; times below 2⁴⁴ ms (at `u64::MAX`, `first_lease` saturates and H5's strict
//!   `taken_at < lease_until` fails);
//! - `hold_cmds_preserve_h` (H1, H2, H3, H5, H6, P2): a model of two nodes and two sessions under
//!   up to three symbolic take, checkpoint, release, expiry, pause, and close steps, gated by
//!   `take_gate` (the checks of `hold take` that `can_take` shares), fenced by `next_fence` (which
//!   `issue_hold` calls), and leased by the functions above; with two nodes a session can meet
//!   `hold.per_session`;
//! - `stale_token_only_late` (H4): `gate` puts a text over `text.max` before everything, and
//!   `live_on`, H4's predicate, is false for another node or no hold and stays false once the lease
//!   ended; that a stale write yields exactly its late entry is H4's step check, not the harness's;
//! - `op_retry_no_effect`, `op_expiry` (O1): `gate`'s retry decision, and that an expired record
//!   never answers again; the prune of expired records is `evolve`'s, checked end to end by the
//!   unit test `retries_replay_conflict_and_expire` and items 2 and 12 of the property test;
//! - `session_retire` (H2, CS-8): `retire_after`, with `Params::valid`'s order of
//!   `session.retain` and `lease.ceiling`; that a touch calls it is the unit test
//!   `sessions_retire_and_the_retire_arm`, which lowers `session.retain` and touches the session;
//! - `silence_after_leases` (H2, T4, 1.4): `drain` over a queue of up to two Lease keys and the
//!   session's Silence key, in `DeadlineKey`'s order: the session falls silent holding nothing.
//!   The Silence time is D.4.8's formula written in the harness, not `deadline_of`'s (on a `State`
//!   of one session and one hold that did not finish within 10 minutes, and `deadlines.rs`, WP-D2's,
//!   has no pure form of it); `deadline_of`'s agreement with the formula is T4's recount and T3's
//!   silent clause, checked after every step of the property test. So the harness checks
//!   `DeadlineKind`'s order and `drain`;
//! - `close_ends_holds` (H6): `ends_on_close`, the selection of step 2 of `close_node`;
//! - `take_gate_closed_nodes` (H6, §25 Node, 1.8, 1.9): `take_gate` over every input: a closed
//!   node with no flag, merged, marked upheld, or with an open objection is refused with `closed`,
//!   and no take it lets through reopens one;
//! - `settle_idempotent_small`, `settle_path_independent_small`, `fire_once_small`,
//!   `event_stamps_t1` (T1 to T4): `drain`, the settle loop itself, over a model queue of up to
//!   two keys `(at, kind, owner)` where one firing may schedule a later key, so that at most three
//!   keys fire within D.12.5's unwind bound of 4 (the state's queue of `DeadlineKey`s is beyond the
//!   budget: its owners hold class names); and `clamp_now`, the time `apply` stamps a command's
//!   events with (that it stamps every one with it is T1's step check);
//! - `reaches_terminates` (T5): `search` and `path`, which `reaches` runs, with the marks and the
//!   stack in fixed arrays where `reaches` keeps them in a `BTreeMap` and a `Vec`, and the
//!   successors in an array, against a brute-force reachability over at most 4 nodes;
//! - `next_key_total`: `NextKey`'s order.
//!
//! **Unwind bounds.** D.12.5 gives 4 for `stale_token_only_late`, `op_retry_no_effect`, and
//! `op_expiry`, sized for the end-to-end form; the narrowed form compares 32-byte request digests,
//! whose `memcmp` needs 33, so these three run at 33. The others run at D.12.5's bounds or lower,
//! with unwinding assertions on, so a loop that needed more would fail the harness.

use crate::apply::{Gate, clamp_now, gate, live_on, retire_after};
use crate::cmd::SessionCtx;
use crate::helpers::close::ends_on_close;
use crate::helpers::holds::{
    TakeBlock, TakeInputs, extended_until, first_lease, lease_l, next_fence, renew_allowed,
    take_gate,
};
use crate::settle::{Queue, drain};
use crate::state::{DeadlineKey, DeadlineKind, Hold, LeaseTerms, OpRecord, Owner};
use crate::types::{Dur, Fence, NodeId, ReqDigest, SECOND, SessionId, Time, Token};

/// Times and durations stay below 2⁴⁴ ms (about 557 years): no sum of two saturates.
const BOUND: u64 = 1 << 44;

/// Lease terms that pass `Params::valid` (D.3): every duration at least a second,
/// `first <= default <= max <= ceiling`.
fn terms() -> LeaseTerms {
    let t: LeaseTerms = kani::any();
    kani::assume(t.first >= SECOND && t.first <= t.default);
    kani::assume(t.default <= t.max && t.max <= t.ceiling && t.ceiling.0 <= BOUND);
    t
}

fn time() -> Time {
    let t: Time = kani::any();
    kani::assume(t.0 <= BOUND);
    t
}

/// H5 of §26 for one hold, with its own terms (A-102).
fn h5(h: &Hold) -> bool {
    let t = &h.terms;
    h.taken_at < h.lease_until
        && h.lease_until <= h.taken_at.plus(t.ceiling).min(h.last_extended.plus(t.max))
        && (h.checkpointed || h.lease_until <= h.taken_at.plus(t.first))
        && h.renews <= t.renew_max
}

/// A hold as `issue_hold` makes it at `now` (D.8.10).
fn issued(now: Time, lease: Option<Dur>, t: LeaseTerms) -> Hold {
    Hold {
        session: SessionId(1),
        taken_at: now,
        lease_until: first_lease(now, &t),
        last_extended: now,
        terms: t,
        l: lease_l(lease, &t),
        renews: 0,
        checkpointed: false,
        reopened: None,
    }
}

/// `issue_hold` satisfies H5 for any `now` below 2⁴⁴ ms, any `lease`, and valid terms; `L` lies in
/// `[1 s, max]`.
///
/// Bounded: at `now` within `lease.first` of `u64::MAX` ms (some 584 million years from the
/// epoch), `first_lease` saturates and H5's strict `taken_at < lease_until` fails; the bound of
/// 2⁴⁴ ms (about 557 years) is recorded for `docs/verification.md`.
#[kani::proof]
#[kani::unwind(1)]
fn lease_issue_h5() {
    let t = terms();
    let h = issued(time(), kani::any(), t);
    assert!(h5(&h));
    assert!(h.l >= SECOND && h.l <= t.max);
}

/// An extension (a checkpoint, or a renew that `renew_allowed` admits) of a live hold that
/// satisfies H5 preserves it, and never shortens the lease.
#[kani::proof]
#[kani::unwind(1)]
fn lease_extend_h5() {
    let t = terms();
    let mut h = Hold {
        session: SessionId(1),
        taken_at: time(),
        lease_until: time(),
        last_extended: time(),
        terms: t,
        l: kani::any(),
        renews: kani::any(),
        checkpointed: kani::any(),
        reopened: None,
    };
    kani::assume(h.l >= SECOND && h.l <= t.max);
    kani::assume(h.taken_at <= h.last_extended);
    kani::assume(h5(&h));
    let now = time();
    kani::assume(h.last_extended <= now && now < h.lease_until);
    let before = h.lease_until;
    if kani::any() {
        // A checkpoint: its entry marks the hold checkpointed before the extension (D.8.10).
        h.checkpointed = true;
        h.renews = 0;
    } else {
        kani::assume(renew_allowed(h.renews, t.renew_max));
        h.renews += 1;
    }
    h.lease_until = extended_until(&h, now);
    h.last_extended = now;
    assert!(h.lease_until >= before);
    assert!(h5(&h));
}

/// A renew is admitted only below `renew.max`, so renews never exceed it (H5).
#[kani::proof]
#[kani::unwind(1)]
fn renew_limit() {
    let renews: u32 = kani::any();
    let max: u32 = kani::any();
    kani::assume(renews <= max);
    if renew_allowed(renews, max) {
        assert!(renews + 1 <= max);
    } else {
        assert!(renews == max);
    }
}

/// One node of the model of `hold_cmds_preserve_h`: its capacity, its fence, and whether it is
/// closed.
#[derive(Clone, Copy)]
struct NodeModel {
    capacity: u32,
    fence: Fence,
    closed: bool,
}

/// Two nodes of capacity 0 to 2, two sessions, `hold.per_session` of 1 or 2, and up to three
/// symbolic steps (take, checkpoint, release, a time step with expiries, a pause, a close), each on
/// either node: after each, H1 per node; H2 per session, which a take can meet because a session
/// may hold both nodes; H3, with every new fence `next_fence` of its node's, above it, and no
/// token held twice; H5; and H6; and no take passed a pause (P2). Every take is gated by
/// `take_gate` over the inputs `take_block` reads: the session's live hold on the node, its live
/// holds in all, and the node's live holds.
#[kani::proof]
#[kani::unwind(4)]
fn hold_cmds_preserve_h() {
    let t = terms();
    let per_session: u32 = kani::any();
    kani::assume((1..=2).contains(&per_session));
    let node = || {
        let capacity: u32 = kani::any();
        kani::assume(capacity <= 2);
        NodeModel {
            capacity,
            fence: Fence(0),
            closed: false,
        }
    };
    let mut nodes = [node(), node()];
    let mut holds: [Option<(Token, Hold)>; 3] = [None, None, None];
    let mut now = Time(1_000);
    let mut paused = false;
    for _ in 0..3 {
        let op: u8 = kani::any();
        let sess = SessionId(if kani::any() { 2 } else { 1 });
        let k = usize::from(kani::any::<bool>());
        let n = NodeId(8 + k as u64);
        let live_hold = |x: &&(Token, Hold)| now < x.1.lease_until;
        let mine = holds
            .iter()
            .flatten()
            .filter(live_hold)
            .find(|(tk, h)| tk.node == n && h.session == sess)
            .map(|(tk, _)| *tk);
        let held = holds
            .iter()
            .flatten()
            .filter(live_hold)
            .filter(|(_, h)| h.session == sess)
            .count() as u32;
        let live = holds
            .iter()
            .flatten()
            .filter(live_hold)
            .filter(|(tk, _)| tk.node == n)
            .count() as u32;
        match op % 6 {
            0 => {
                let i = TakeInputs {
                    existing: mine,
                    closed: nodes[k].closed,
                    flagged: false,
                    merged_into: None,
                    upheld: kani::any(),
                    objection: kani::any(),
                    occ_locked: false,
                    occ_waiting: false,
                    paused: paused.then_some((n, Time(u64::MAX))),
                    held,
                    per_session,
                    live,
                    capacity: nodes[k].capacity,
                };
                match take_gate(&i) {
                    None => {
                        assert!(!paused, "P2");
                        assert!(!nodes[k].closed, "H6");
                        let fence = next_fence(nodes[k].fence);
                        assert!(fence > nodes[k].fence, "H3: a node's fence only increases");
                        nodes[k].fence = fence;
                        let mut h = issued(now, kani::any(), t);
                        h.session = sess;
                        let entry = Some((Token { node: n, fence }, h));
                        if holds[0].is_none() {
                            holds[0] = entry;
                        } else if holds[1].is_none() {
                            holds[1] = entry;
                        } else if holds[2].is_none() {
                            holds[2] = entry;
                        } else {
                            // Three live holds, two per session at most and both nodes' slots
                            // used: the gate refuses a fourth.
                            assert!(false, "a take past the limits");
                        }
                    }
                    Some(TakeBlock::Full(c, l)) => assert!(l >= c, "full only without a slot"),
                    Some(TakeBlock::HoldsLimit) => assert!(held >= per_session, "the limit met"),
                    Some(_) => {}
                }
            }
            1 => {
                // A checkpoint of the session's hold on the node.
                for (tk, h) in holds.iter_mut().flatten() {
                    if Some(*tk) == mine {
                        h.checkpointed = true;
                        h.renews = 0;
                        h.lease_until = extended_until(h, now);
                        h.last_extended = now;
                    }
                }
            }
            2 => {
                // A release.
                for slot in holds.iter_mut() {
                    if mine.is_some() && slot.as_ref().map(|(tk, _)| *tk) == mine {
                        *slot = None;
                    }
                }
            }
            3 => {
                // A time step: the leases that end, end.
                let d: u64 = kani::any();
                kani::assume(d <= t.ceiling.0);
                now = Time(now.0 + d);
                for slot in holds.iter_mut() {
                    if slot.as_ref().is_some_and(|(_, h)| now >= h.lease_until) {
                        *slot = None;
                    }
                }
            }
            4 => paused = !paused,
            _ => {
                // A close ends every live hold on the node (close_node, step 2).
                nodes[k].closed = true;
                for slot in holds.iter_mut() {
                    if slot
                        .as_ref()
                        .is_some_and(|(tk, h)| ends_on_close(tk, h, n, now))
                    {
                        *slot = None;
                    }
                }
            }
        }
        // H1 and H6, with H3's fence bound, per node.
        for (j, nd) in nodes.iter().enumerate() {
            let on = NodeId(8 + j as u64);
            let mut count = 0_u32;
            for (tk, _) in holds.iter().flatten() {
                if tk.node == on {
                    count += 1;
                    assert!(tk.fence <= nd.fence, "H3");
                    assert!(!nd.closed, "H6");
                }
            }
            assert!(count <= nd.capacity, "H1");
        }
        // H2 per session; H5 per hold.
        for s in [SessionId(1), SessionId(2)] {
            let k = holds
                .iter()
                .flatten()
                .filter(|(_, h)| h.session == s)
                .count() as u32;
            assert!(k <= per_session, "H2");
        }
        for (_, h) in holds.iter().flatten() {
            assert!(h5(h), "H5");
        }
        // H3: no token is held twice.
        for (a, b) in [(0, 1), (0, 2), (1, 2)] {
            if let (Some((x, _)), Some((y, _))) = (&holds[a], &holds[b]) {
                assert!(x != y, "H3: no token repeats");
            }
        }
    }
}

/// 1.8 (§10.1, §25 Node): over every input, `take_gate` refuses with `closed` a take of a closed
/// node that carries no flag, was merged, or is marked upheld (an objection upheld on it corrected
/// its outcome), or, 1.9, carries an open objection (its review is a `talk resolve`), naming its
/// `merged_into`; and a take it lets through is of an open node, or of a closed one that is
/// flagged, not merged, not upheld, with no open objection, and not a locked occurrence: so no take
/// reopens a node whose correction says its result does not stand, or whose objection is still
/// undecided.
#[kani::proof]
#[kani::unwind(2)]
fn take_gate_closed_nodes() {
    let i = TakeInputs {
        existing: if kani::any() { Some(kani::any()) } else { None },
        closed: kani::any(),
        flagged: kani::any(),
        merged_into: if kani::any() { Some(NodeId(9)) } else { None },
        upheld: kani::any(),
        objection: kani::any(),
        occ_locked: kani::any(),
        occ_waiting: kani::any(),
        paused: if kani::any() {
            Some((NodeId(8), kani::any()))
        } else {
            None
        },
        held: kani::any(),
        per_session: kani::any(),
        live: kani::any(),
        capacity: kani::any(),
    };
    let r = take_gate(&i);
    if i.existing.is_none()
        && i.closed
        && (!i.flagged || i.merged_into.is_some() || i.upheld || i.objection)
    {
        assert!(r == Some(TakeBlock::Closed(i.merged_into)), "closed");
    }
    if r.is_none() {
        assert!(i.existing.is_none());
        assert!(
            !i.closed
                || (i.flagged
                    && i.merged_into.is_none()
                    && !i.upheld
                    && !i.objection
                    && !i.occ_locked)
        );
    }
}

/// H4's precondition order: a text over `text.max` is answered before the retry store, whatever
/// the record, so a stale write with one never reaches the token check and appends no late entry;
/// and `live_on`, H4's predicate (§26 `live(h, now)`), holds only for a token of the node it names
/// whose hold exists, and once false for a time stays false for every later time (a lease that
/// ended does not come back).
///
/// Narrowed (D.12.5): that a stale write then yields exactly the late entry, and nothing else, is
/// H4's step check, run on every step of the unit tests and the property test (`check_step`).
#[kani::proof]
#[kani::unwind(33)]
fn stale_token_only_late() {
    let longest: u64 = kani::any();
    let text_max: u32 = kani::any();
    let record: Option<OpRecord> = if kani::any() {
        Some(OpRecord {
            digest: kani::any(),
            session: kani::any(),
            expires_at: kani::any(),
        })
    } else {
        None
    };
    let g = gate(
        longest,
        text_max,
        record.as_ref(),
        kani::any(),
        SessionCtx::Existing(kani::any()),
        kani::any(),
    );
    assert!((g == Gate::TextTooLong) == (longest > u64::from(text_max)));
    let h: Option<Hold> = if kani::any() {
        Some(Hold {
            session: kani::any(),
            taken_at: kani::any(),
            lease_until: kani::any(),
            last_extended: kani::any(),
            terms: kani::any(),
            l: kani::any(),
            renews: kani::any(),
            checkpointed: kani::any(),
            reopened: None,
        })
    } else {
        None
    };
    let token: Token = kani::any();
    let n: NodeId = kani::any();
    let (t1, t2): (Time, Time) = (kani::any(), kani::any());
    kani::assume(t1 <= t2);
    if live_on(h.as_ref(), token, n, t2) {
        assert!(
            live_on(h.as_ref(), token, n, t1),
            "a lease that ended stays ended"
        );
    }
    if token.node != n || h.is_none() {
        assert!(
            !live_on(h.as_ref(), token, n, t1),
            "stale: another node, or no hold"
        );
    }
}

/// O1 (CS-8, A-114): with a live record, the same digest from no session or the record's is
/// `Replay`, anything else `op_id_conflict`; with none, or an expired one, the call proceeds; a
/// text over `text.max` is `TextTooLong` whatever the record.
#[kani::proof]
#[kani::unwind(33)]
fn op_retry_no_effect() {
    let rec = OpRecord {
        digest: kani::any(),
        session: kani::any(),
        expires_at: kani::any(),
    };
    let digest: ReqDigest = kani::any();
    let now: Time = kani::any();
    let sc = match kani::any::<u8>() % 3 {
        0 => SessionCtx::New,
        1 => SessionCtx::Existing(kani::any()),
        _ => SessionCtx::Existing(rec.session),
    };
    let longest: u64 = kani::any();
    let text_max: u32 = kani::any();
    let g = gate(longest, text_max, Some(&rec), Some(digest), sc, now);
    if longest > u64::from(text_max) {
        assert!(g == Gate::TextTooLong);
    } else if rec.expires_at <= now {
        assert!(g == Gate::Proceed);
    } else {
        let same = matches!(sc, SessionCtx::New)
            || matches!(sc, SessionCtx::Existing(x) if x == rec.session);
        assert!((g == Gate::Replay) == (digest == rec.digest && same));
        assert!((g == Gate::Conflict) == !(digest == rec.digest && same));
    }
}

/// O1's expiry (CS-8): the record answers repeats exactly while `expires_at > now`, and once it
/// stops answering it never answers again at a later time, so pruning it at the first event at or
/// after `expires_at` changes no answer (T2).
///
/// Narrowed (D.12.5): the prune itself is `evolve`'s; that the first event at or after
/// `expires_at` drops the record, in either order of two events, is checked end to end by the unit
/// test `retries_replay_conflict_and_expire`, O1's state check (every stored record has
/// `expires_at > last_now`), and items 2 and 12 of the property test.
#[kani::proof]
#[kani::unwind(33)]
fn op_expiry() {
    let rec = OpRecord {
        digest: kani::any(),
        session: kani::any(),
        expires_at: kani::any(),
    };
    let now: Time = kani::any();
    let g = gate(0, 0, Some(&rec), Some(rec.digest), SessionCtx::New, now);
    assert!((g == Gate::Replay) == (rec.expires_at > now));
    let later: Time = kani::any();
    kani::assume(later >= now);
    if g == Gate::Proceed {
        let again = gate(0, 0, Some(&rec), Some(rec.digest), SessionCtx::New, later);
        assert!(
            again == Gate::Proceed,
            "an expired record never answers again"
        );
    }
}

/// CS-8, H2: a touch never lowers `retire_at`, and, with `session.retain > lease.ceiling`
/// (`Params::valid`), a hold issued after it ends before the session retires: `evolve`'s
/// precondition `retire_at > at + ceiling` holds, and so does H2's `retire_at > taken_at +
/// ceiling`.
#[kani::proof]
#[kani::unwind(1)]
fn session_retire() {
    let t = terms();
    let retain: Dur = kani::any();
    kani::assume(retain > t.ceiling && retain.0 <= BOUND);
    let old = time();
    let now = time();
    let r = retire_after(old, now, retain);
    assert!(r >= old);
    assert!(r > now.plus(t.ceiling));
    let h = issued(now, kani::any(), t);
    assert!(h.lease_until <= h.taken_at.plus(t.ceiling));
    assert!(r > h.taken_at.plus(t.ceiling));
}

/// The queue of one session with up to two holds (1.4, D.4.8): their Lease keys, and the session's
/// Silence key at `max(silence_at, the largest lease_until, last_now)`. It records whether a Lease
/// key was still queued, that is a hold still live, when the Silence key fired.
#[derive(Clone)]
struct SilenceQueue {
    keys: [Option<DeadlineKey>; 3],
    silence_fired: bool,
    held_at_silence: bool,
}

impl Queue for SilenceQueue {
    type Key = DeadlineKey;
    fn first(&self) -> Option<DeadlineKey> {
        // The least of three, without an iterator.
        let min = |a: Option<DeadlineKey>, b: Option<DeadlineKey>| match (a, b) {
            (Some(a), Some(b)) => Some(if a < b { a } else { b }),
            (Some(a), None) => Some(a),
            (None, b) => b,
        };
        min(
            min(self.keys[0].clone(), self.keys[1].clone()),
            self.keys[2].clone(),
        )
    }
    fn at(k: &DeadlineKey) -> Time {
        k.at
    }
    fn fire(&mut self, k: &DeadlineKey) {
        if k.kind == DeadlineKind::Silence {
            self.silence_fired = true;
            self.held_at_silence = self.keys[0].is_some() || self.keys[1].is_some();
        }
        if self.keys[0].as_ref() == Some(k) {
            self.keys[0] = None;
        }
        if self.keys[1].as_ref() == Some(k) {
            self.keys[1] = None;
        }
        if self.keys[2].as_ref() == Some(k) {
            self.keys[2] = None;
        }
    }
    fn queued(&self, k: &DeadlineKey) -> bool {
        self.keys[0].as_ref() == Some(k)
            || self.keys[1].as_ref() == Some(k)
            || self.keys[2].as_ref() == Some(k)
    }
}

/// 1.4 (§3, R27, H2, T4): a session with up to two live holds, symbolic lease ends, `silence_at`,
/// and `last_now`. Its Silence key at D.4.8's time lies at or after every lease's end and after
/// `last_now`, each Lease key precedes it in `DeadlineKey`'s order, and `drain`, the settle loop,
/// settling to any time never fires the Silence key while a Lease key is queued: the session falls
/// silent holding nothing.
///
/// Narrowed (D.12.5): `deadline_of` on a `State` of one session and one hold did not finish within
/// 10 minutes (every `BTreeMap` operation is expensive for CBMC), so the Silence time is D.4.8's
/// formula here, and `deadline_of`'s agreement with it is checked end to end by T4 (the queue
/// equals `recount_deadlines`) and T3's silent clause after every step of the property test.
#[kani::proof]
#[kani::unwind(4)]
fn silence_after_leases() {
    let x = SessionId(1);
    let silence_at = time();
    let last_now = time();
    let lease = |node: u64| {
        if kani::any() {
            let at = time();
            // A settled state queues no key at or before `last_now` (T4).
            kani::assume(at > last_now);
            Some(DeadlineKey {
                at,
                kind: DeadlineKind::Lease,
                owner: Owner::Hold(Token {
                    node: NodeId(node),
                    fence: crate::types::Fence(1),
                }),
            })
        } else {
            None
        }
    };
    let a = lease(8);
    let b = lease(9);
    let mut at = silence_at.max(last_now);
    if let Some(k) = &a {
        at = at.max(k.at);
    }
    if let Some(k) = &b {
        at = at.max(k.at);
    }
    let silence = DeadlineKey {
        at,
        kind: DeadlineKind::Silence,
        owner: Owner::Session(x),
    };
    assert!(silence.at >= last_now);
    if let Some(k) = &a {
        assert!(*k < silence, "the lease fires first");
    }
    if let Some(k) = &b {
        assert!(*k < silence, "the lease fires first");
    }
    let mut q = SilenceQueue {
        keys: [a, b, Some(silence)],
        silence_fired: false,
        held_at_silence: false,
    };
    let now = time();
    let st = drain(&mut q, now, u32::MAX);
    assert!(st == crate::settle::Drained::Done);
    if q.silence_fired {
        assert!(
            !q.held_at_silence,
            "a session falls silent holding nothing (H2)"
        );
    }
}

/// H6: on a settled state every hold in the map is live (T3), and a close ends every live hold
/// on its node, so none is left.
#[kani::proof]
#[kani::unwind(1)]
fn close_ends_holds() {
    let tk: Token = kani::any();
    let h = Hold {
        session: kani::any(),
        taken_at: kani::any(),
        lease_until: kani::any(),
        last_extended: kani::any(),
        terms: kani::any(),
        l: kani::any(),
        renews: kani::any(),
        checkpointed: kani::any(),
        reopened: None,
    };
    let now: Time = kani::any();
    kani::assume(h.lease_until > now);
    assert!(ends_on_close(&tk, &h, tk.node, now));
    let other: NodeId = kani::any();
    kani::assume(other != tk.node);
    assert!(!ends_on_close(&tk, &h, other, now));
}

// ------------------------------------------------------------------------------------------------
// The settle loop over a model queue (T1 to T4)

/// A model key: `(at, kind, owner)`, ordered as `DeadlineKey` is, field by field.
type Key = (u8, u8, u8);

/// A queue of at most two keys, owned by 0 and 1 (D.12.5: "a state with at most 2 deadlines").
/// Firing a key removes it and, by a fixed table (a function of the key alone, as L1 of T2
/// requires), may schedule its owner again a symbolic delay of at least one later (D.8.2: a firing
/// creates deadlines only strictly after itself); one owner at most does so, once, so at most three
/// keys fire, which D.12.5's unwind bound of 4 covers. The fired keys are recorded in order.
#[derive(Clone, PartialEq, Eq)]
struct Model {
    keys: [Option<Key>; 2],
    again: [Option<u8>; 2],
    fired: [Option<Key>; 3],
    count: usize,
}

impl Queue for Model {
    type Key = Key;
    fn first(&self) -> Option<Key> {
        // The smaller of two, without an iterator: CBMC unwinds adapters expensively.
        match (self.keys[0], self.keys[1]) {
            (Some(a), Some(b)) => Some(if a < b { a } else { b }),
            (Some(a), None) => Some(a),
            (None, b) => b,
        }
    }
    fn at(k: &Key) -> Time {
        Time(u64::from(k.0))
    }
    fn fire(&mut self, k: &Key) {
        let i = usize::from(k.2 & 1);
        if self.count < self.fired.len() {
            self.fired[self.count] = Some(*k);
        }
        self.count += 1;
        self.keys[i] = match self.again[i] {
            Some(d) if k.0 < 200 && d < 50 => Some((k.0 + 1 + d, k.1, k.2)),
            _ => None,
        };
        // Once per owner; the model lets one owner at most reschedule.
        self.again[i] = None;
    }
    fn queued(&self, k: &Key) -> bool {
        self.keys[0] == Some(*k) || self.keys[1] == Some(*k)
    }
}

fn model() -> Model {
    let key = |owner: u8| {
        let at: u8 = kani::any();
        let kind: u8 = kani::any();
        kani::assume(at < 200 && kind < 12);
        (at, kind, owner)
    };
    let a = if kani::any() { Some(key(0)) } else { None };
    let b = if kani::any() { Some(key(1)) } else { None };
    let again: [Option<u8>; 2] = [kani::any(), kani::any()];
    kani::assume(again[0].is_none() || again[1].is_none());
    Model {
        keys: [a, b],
        again,
        fired: [None; 3],
        count: 0,
    }
}

/// T4: every due key fires once, in `(at, kind, owner)` order, and is gone afterwards; no key at
/// or before `now` stays queued; a firing never leaves its own key (no `Stuck`).
#[kani::proof]
#[kani::unwind(4)]
fn fire_once_small() {
    let mut m = model();
    let now = Time(u64::from(kani::any::<u8>()));
    let st = drain(&mut m, now, u32::MAX);
    assert!(st == crate::settle::Drained::Done);
    if let Some(k) = m.keys[0] {
        assert!(Time(u64::from(k.0)) > now);
    }
    if let Some(k) = m.keys[1] {
        assert!(Time(u64::from(k.0)) > now);
    }
    for i in 1..m.count.min(3) {
        if let (Some(a), Some(b)) = (m.fired[i - 1], m.fired[i]) {
            assert!(a < b, "in order, each once");
            assert!(Time(u64::from(b.0)) <= now);
        }
    }
}

/// T3's idempotence: settling again at the same time fires nothing.
#[kani::proof]
#[kani::unwind(4)]
fn settle_idempotent_small() {
    let mut m = model();
    let now = Time(u64::from(kani::any::<u8>()));
    drain(&mut m, now, u32::MAX);
    let before = m.clone();
    let st = drain(&mut m, now, u32::MAX);
    assert!(st == crate::settle::Drained::Done);
    assert!(m == before);
}

/// T2: settling to `t1` and then to `t2` fires the same keys, in the same order, and leaves the
/// same queue, as settling to `t2`; batches of one change nothing either.
#[kani::proof]
#[kani::unwind(4)]
fn settle_path_independent_small() {
    let base = model();
    let t1 = Time(u64::from(kani::any::<u8>()));
    let t2 = Time(u64::from(kani::any::<u8>()));
    kani::assume(t1 <= t2);
    let mut direct = base.clone();
    drain(&mut direct, t2, u32::MAX);
    let mut stepped = base.clone();
    drain(&mut stepped, t1, u32::MAX);
    drain(&mut stepped, t2, u32::MAX);
    assert!(direct == stepped);
    let mut batched = base;
    let mut rounds = 0;
    while drain(&mut batched, t2, 1) == crate::settle::Drained::More {
        rounds += 1;
        kani::assume(rounds < 5);
    }
    assert!(direct == batched);
}

/// T1: `clamp_now`, the time `apply` stamps every event of a command with (D.8.1 step 1), is
/// `max(ctx.now, last_now)`, never before `last_now`; a firing's events carry the key's own time,
/// and the times of successive firings never decrease.
///
/// Narrowed (D.12.5): that `apply` stamps every event with `clamp_now`'s time and that `last_now`
/// ends at their maximum is T1's step check, run on every step of the unit tests and the property
/// test, and the unit test `a_clock_behind_the_log_is_clamped` sends a clock behind the log.
#[kani::proof]
#[kani::unwind(4)]
fn event_stamps_t1() {
    let ctx_now: Time = kani::any();
    let last_now: Time = kani::any();
    let now = clamp_now(ctx_now, last_now);
    assert!(now >= last_now && now >= ctx_now);
    assert!(now == ctx_now || now == last_now);
    let mut m = model();
    let horizon = Time(u64::from(kani::any::<u8>()));
    drain(&mut m, horizon, u32::MAX);
    for i in 1..m.count.min(3) {
        if let (Some(a), Some(b)) = (m.fired[i - 1], m.fired[i]) {
            assert!(a.0 <= b.0);
        }
    }
}

// ------------------------------------------------------------------------------------------------
// T5 and `next`'s order

/// The marks of a search over nodes 0 to 3, in a fixed array.
struct ArrayMarks([Option<Option<NodeId>>; 4]);

impl crate::helpers::tree::Marks for ArrayMarks {
    fn visit(&mut self, x: NodeId, parent: Option<NodeId>) -> bool {
        let i = (x.0 & 3) as usize;
        if self.0[i].is_some() {
            return false;
        }
        self.0[i] = Some(parent);
        true
    }
    fn parent(&self, x: NodeId) -> Option<NodeId> {
        self.0[(x.0 & 3) as usize].flatten()
    }
}

/// The stack of a search over nodes 0 to 3, in a fixed array: each node is pushed once at most.
struct ArrayStack {
    buf: [NodeId; 4],
    len: usize,
}

impl crate::helpers::tree::Stack for ArrayStack {
    fn push(&mut self, x: NodeId) {
        assert!(self.len < 4, "each node is pushed once at most");
        self.buf[self.len] = x;
        self.len += 1;
    }
    fn pop(&mut self) -> Option<NodeId> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(self.buf[self.len])
    }
}

/// T5: `search`, the search `reaches` runs, over at most 4 nodes with arbitrary edges terminates
/// within its bound and agrees with a brute-force reachability (the transitive closure); and
/// `path` then gives a path from `from` to `to` along the edges.
#[kani::proof]
#[kani::unwind(5)]
fn reaches_terminates() {
    let edges: [[bool; 4]; 4] = kani::any();
    let from: u8 = kani::any();
    let to: u8 = kani::any();
    kani::assume(from < 4 && to < 4);
    // The successors in a fixed array, not a `Vec`: a heap object per call multiplies the objects
    // every pointer of the search may point to.
    let succ = |x: NodeId| {
        let row = edges[(x.0 & 3) as usize];
        [
            row[0].then_some(NodeId(0)),
            row[1].then_some(NodeId(1)),
            row[2].then_some(NodeId(2)),
            row[3].then_some(NodeId(3)),
        ]
        .into_iter()
        .flatten()
    };
    let mut marks = ArrayMarks([None; 4]);
    let mut stack = ArrayStack {
        buf: [NodeId(0); 4],
        len: 0,
    };
    let (a, b) = (NodeId(u64::from(from)), NodeId(u64::from(to)));
    let found = crate::helpers::tree::search(a, b, succ, &mut marks, &mut stack, 4);
    // Brute force: the nodes reachable from `from`, as a bit set grown four times by the rows of
    // the nodes in it.
    let mut reach: u8 = 1 << from;
    for _ in 0..4 {
        let mut next = reach;
        for (i, row) in edges.iter().enumerate() {
            if reach & (1 << i) != 0 {
                for (j, &e) in row.iter().enumerate() {
                    if e {
                        next |= 1 << j;
                    }
                }
            }
        }
        reach = next;
    }
    assert!(found == (reach & (1 << to) != 0));
    if found {
        let path = crate::helpers::tree::path(&marks, b, 4);
        assert!(path.first() == Some(&a));
        assert!(path.last() == Some(&b));
        for w in path.windows(2) {
            assert!(
                edges[(w[0].0 & 3) as usize][(w[1].0 & 3) as usize],
                "the path follows edges"
            );
        }
    }
}

/// `next`'s key order is a total order (D.8.5): exactly one of `<`, `==`, `>`, and transitive.
#[kani::proof]
#[kani::unwind(1)]
fn next_key_total() {
    let a: crate::state::NextKey = kani::any();
    let b: crate::state::NextKey = kani::any();
    let c: crate::state::NextKey = kani::any();
    let lt = a < b;
    let eq = a == b;
    let gt = a > b;
    assert!(u8::from(lt) + u8::from(eq) + u8::from(gt) == 1);
    if a <= b && b <= c {
        assert!(a <= c);
    }
}
