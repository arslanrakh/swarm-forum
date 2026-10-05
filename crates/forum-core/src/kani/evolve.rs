//! The harnesses of WP-D2 (design D.12.5) for the key invariants it owns (D.0.3): the totality of
//! `evolve` (D.7.5: no panic, no overflow, and an anomaly counted for an event it cannot apply) and
//! O2's counters. The harnesses D.12.5 lists for W1, F, and R1 (`flags_waiting_w1`,
//! `flags_between_events`, `basis_stale_refresh`, `subtree_rollup_move`) are not built: D.0.3
//! leaves those invariants to their executable checks in tests.
//!
//! **Narrowed** (D.12.5), and recorded for `docs/verification.md`. The end-to-end forms, one event
//! of a group applied by `evolve` to a small state, did not leave CBMC's symbolic execution within
//! 20 to 30 CPU-minutes each, against a CI budget of 30 minutes for the whole set: every
//! `BTreeMap` operation is expensive for CBMC, its leaves holding arrays of whole nodes, and even a
//! state of two nodes built by hand stayed there. So each harness checks the pure helper that
//! enforces its property, over all its inputs, at D.12.5's unwind of 4 (with unwinding assertions,
//! so a loop that needed more would fail the harness), and the end-to-end form stays a test that
//! runs `evolve` itself. Whether totality, a key invariant (D.0.3), may keep this narrowed form is
//! an open question (see the package report):
//! - `evolve_no_panic_*`: the arithmetic of the indices (bit lengths, the subtree counts, the held
//!   time of a hold, the mean in the median of completed work) never panics or overflows; the
//!   constant divisions (the day, the minute, and the hour group of a time) are unit-tested at
//!   their edges. Totality is otherwise structural, by the deny list of D.1.3 (no indexing,
//!   `unwrap`, `expect`, `panic!`, or unchecked arithmetic in the crate). End to end, the
//!   randomized test `evolve::random_tests` applies every event variant (60 sequences of 300
//!   events in CI, and 2.5 million events once by hand) with no panic, and checks after every event
//!   that the indices equal the recount;
//! - `evolve_counts_anomalies`: `NodeUpdated`'s precondition (`patch_differs`: every present field
//!   differs) over symbolic patches; end to end, the unit tests
//!   `an_event_naming_a_missing_object_is_an_anomaly_and_changes_nothing_else` and
//!   `a_failed_precondition_counts_an_anomaly_and_changes_nothing` (a hold past its taker's
//!   retirement, a settlement by a decision that does not list the request, a firing before its
//!   time or of a key that is not queued) require the state to be unchanged but for the anomaly
//!   count, and the randomized test compares every anomaly it draws (about a third of its events)
//!   with a pure anomaly at the same time, over every base and history map;
//! - `version_bumps_o2`: the counter rule of an update (`counter_bumps`) over every combination of
//!   present fields; every node event end to end is the unit test
//!   `o2_counters_move_by_their_rule_for_every_node_event`.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;

use crate::event::NodePatch;
use crate::evolve::{counter_bumps, patch_differs};
use crate::index::{bit_len, est_bucket, horizon_bits, mean_floor, sub_apply};
use crate::state::{Life, Node, Origin, Role, Subtree};
use crate::types::{Change, Fence, Seq, Text, Time, Version};

/// A reserved node with every field at its start: capacity 0, priority 0, an empty title.
fn blank() -> Node {
    Node {
        place: None,
        origin: Origin::Reserved,
        created_at: Time(0),
        title: Text(String::new()),
        body: Text(String::new()),
        fields: BTreeMap::new(),
        files: BTreeMap::new(),
        status: Text(String::new()),
        digest: Text(String::new()),
        agreements: Text(String::new()),
        agreements_at: None,
        kind: None,
        tags: BTreeSet::new(),
        capacity: 0,
        priority: 0,
        est: None,
        needs: BTreeSet::new(),
        due: None,
        due_fired: false,
        owners: BTreeSet::new(),
        keys: BTreeMap::new(),
        links: BTreeSet::new(),
        depends_on: BTreeSet::new(),
        wait: None,
        role: Role::Plain,
        life: Life::Open,
        version: Version(1),
        digest_version: Version(1),
        fence: Fence(0),
        seq: Seq(0),
        content_version: Version(1),
        attempts: 0,
        checkpoints: 0,
        last_checkpoint: None,
        expired_hold: None,
        open_intent: None,
        recheck_marks: BTreeSet::new(),
        assume_marks: BTreeSet::new(),
        held_ms: 0,
        people_left: Seq(0),
        left: 0,
        touched_at: None,
        stale_fired: false,
        retracted_decision: None,
        pr_named: false,
    }
}

// ------------------------------------------------------------------------------------------------
// Totality of the index arithmetic (D.7.5)

/// The session group's arithmetic: a horizon's bit length (the profile key, D.4.9) and a silence
/// or retire time `at + d`, for any inputs. The day and the minute of a time are constant
/// divisions (`checked_div` by a non-zero constant, so they cannot fail), checked at their edges by
/// unit tests: a symbolic 64-bit division does not finish within the budget (as WP-B found).
#[kani::proof]
#[kani::unwind(4)]
fn evolve_no_panic_sessions() {
    let h: Option<u32> = kani::any();
    let at: Time = kani::any();
    let d: crate::types::Dur = kani::any();
    let bits = horizon_bits(h);
    assert!(bits <= 33);
    assert!((bits == 33) == h.is_none());
    if let Some(x) = h
        && x > 0
    {
        assert!(x >> (bits - 1) == 1);
    }
    assert!(at.plus(d) >= at);
}

/// The node group's arithmetic: an `est` bucket, and the subtree counts a node adds to or takes
/// from an ancestor, saturating, for any inputs. The hour group is a constant division, checked by
/// unit tests (see `evolve_no_panic_sessions`).
#[kani::proof]
#[kani::unwind(4)]
fn evolve_no_panic_nodes() {
    let est: Option<u32> = kani::any();
    let b = est_bucket(est);
    assert!(b <= 32);
    assert!((b == 0) == est.is_none_or(|e| e == 0));
    if let Some(e) = est
        && e > 0
    {
        // A bucket below the bit length of a horizon fits it entirely (D.4.10).
        assert!(e >> (b - 1) == 1);
    }
    let a: Subtree = kani::any();
    let u: Subtree = kani::any();
    let add = sub_apply(a, u, true);
    let take = sub_apply(a, u, false);
    assert!(add.open >= a.open && take.open <= a.open);
    assert!(add.flagged >= a.flagged && take.flagged <= a.flagged);
}

/// The hold group's arithmetic: a bit length, and the time a hold was held, `at − taken_at`,
/// saturating, for any inputs.
#[kani::proof]
#[kani::unwind(4)]
fn evolve_no_panic_holds() {
    let x: u32 = kani::any();
    let b = bit_len(x);
    assert!(b <= 32);
    if x > 0 {
        assert!(x >> (b - 1) == 1);
    } else {
        assert!(b == 0);
    }
    let at: Time = kani::any();
    let taken: Time = kani::any();
    let held = at.since(taken).0;
    assert!(held <= at.0);
}

/// The entity group's arithmetic: the mean of the two middle values of completed work (the median
/// of an even count, §17, A-41) lies between them and never overflows, for any inputs. The sort
/// around it is the standard library's, unit-tested with the median.
#[kani::proof]
#[kani::unwind(4)]
fn evolve_no_panic_entities() {
    let a: u64 = kani::any();
    let b: u64 = kani::any();
    let m = mean_floor(a, b);
    assert!(a.min(b) <= m && m <= a.max(b));
    // ⌊(a + b) / 2⌋, computed in 128 bits.
    assert!(u128::from(m) == (u128::from(a) + u128::from(b)) / 2);
}

// ------------------------------------------------------------------------------------------------
// Anomalies (D.7.5) and O2 (A-3, CS-34)

/// A one-letter text, or the empty one.
fn text(on: bool) -> Text {
    Text(if on { String::from("t") } else { String::new() })
}

/// A patch with each field present or not, symbolically, each value one of two.
fn patch() -> NodePatch {
    let some = |on: bool| on.then(|| text(kani::any()));
    NodePatch {
        title: some(kani::any()),
        body: some(kani::any()),
        fields: kani::any::<bool>().then(BTreeMap::new),
        files: kani::any::<bool>().then(BTreeMap::new),
        status: some(kani::any()),
        digest: some(kani::any()),
        agreements: some(kani::any()),
        kind: if kani::any() {
            Change::Clear
        } else {
            Change::Keep
        },
        tags: kani::any::<bool>().then(BTreeSet::new),
        capacity: kani::any(),
        priority: kani::any(),
        est: if kani::any() {
            Change::Set(kani::any())
        } else {
            Change::Keep
        },
        needs: kani::any::<bool>().then(BTreeSet::new),
        due: if kani::any() {
            Change::Set(kani::any())
        } else {
            Change::Keep
        },
        depends_on: kani::any::<bool>().then(BTreeSet::new),
        owners: kani::any::<bool>().then(BTreeSet::new),
        repeat: Change::Keep,
    }
}

/// O2 for an update: `version` moves exactly with title, body, fields, files, or status present;
/// `content_version` follows it exactly with title, body, fields, or files; `digest_version` moves
/// exactly with the digest; nothing else moves a counter (A-3).
#[kani::proof]
#[kani::unwind(4)]
fn version_bumps_o2() {
    let p = patch();
    let b = counter_bumps(&p);
    let content = p.title.is_some() || p.body.is_some() || p.fields.is_some() || p.files.is_some();
    assert!(b.content_version == content);
    assert!(b.version == (content || p.status.is_some()));
    assert!(b.digest_version == p.digest.is_some());
}

/// `NodeUpdated`'s precondition (D.7.4): an update is applied only when every present field
/// differs from the node's; an update whose present fields include one equal to the node's value
/// is an anomaly. Checked on a node whose title is empty, against a patch of symbolic titles,
/// capacities, and priorities.
#[kani::proof]
#[kani::unwind(4)]
fn evolve_counts_anomalies() {
    let node = blank();
    let title: Option<bool> = kani::any();
    let capacity: Option<u32> = kani::any();
    let priority: Option<i32> = kani::any();
    let p = NodePatch {
        title: title.map(text),
        capacity,
        priority,
        ..NodePatch::default()
    };
    let differs = title.is_none_or(|t| t)
        && capacity.is_none_or(|c| c != node.capacity)
        && priority.is_none_or(|x| x != node.priority);
    assert!(patch_differs(&node, &p) == differs);
}
