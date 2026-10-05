//! The `holds` slice (D.10.7) replayed against the core (D.10.8, WP-F1): H1 to H6, O1, O2, T1 to
//! T4, and the leases, fences, tokens, `op_id` records, versions, and clock they read.
//!
//! Every command of the slice is WP-D3's. One consequence is not: with one class (`holds_one`,
//! `QUORUM = 1`), a `release {cannot}` by an active class makes every active class refuse the node,
//! and E1's `e1_check` asks the user (`create_uq`, D.8.14), which is WP-D5a's. With WP-D5a merged,
//! every fixture replays to its end.

mod common;

use common::{actions, divergences, incomplete, missing_ok, replay_group};
use forum_conformance::{Options, Verdict};

/// The slice's actions (`slices/holds.qnt`), each of which some fixture replays with success.
const HOLDS_ACTIONS: [&str; 14] = [
    "act_hello",
    "act_create",
    "act_take",
    "act_next",
    "act_checkpoint",
    "act_renew",
    "act_release",
    "act_close",
    "act_update",
    "act_pause",
    "act_unpause",
    "act_view",
    "act_tick",
    "act_tick_next",
];

#[test]
fn every_holds_fixture_replays_with_no_divergence() {
    let reports = replay_group("holds", Options::default());
    let bad = divergences(&reports);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    // The only gap the slice may reach is E1's question for the user (WP-D5a).
    for r in &reports {
        if let Verdict::Gap { reason, .. } = &r.verdict {
            assert!(reason.contains("WP-D5a"), "{}", r.describe());
        }
    }
    println!("{:?}", actions(&reports));
    let missing = missing_ok(&reports, &HOLDS_ACTIONS);
    assert!(
        missing.is_empty(),
        "never replayed with success: {missing:?}"
    );
    // A gap stops a trace, so most of the states still have to be replayed.
    let states: usize = reports.iter().map(|r| r.states).sum();
    let replayed: usize = reports.iter().map(|r| r.replayed).sum();
    println!("{replayed} of {states} states replayed");
    assert!(replayed * 4 >= states * 3, "{replayed} of {states}");
}

#[test]
fn every_holds_fixture_replays_to_its_end() {
    let reports = replay_group("holds", Options { gaps_diverge: true });
    let bad = incomplete(&reports);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
