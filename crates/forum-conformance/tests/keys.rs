//! The `keys` slice (D.10.7) replayed against the core (D.10.8, WP-F1): K1, K3, K4, K5, and T5 read
//! the keys, the tree, `merged_into`, and the waits.
//!
//! The slice's commands were built in layers: WP-D3's `node create` that makes a node, `node
//! close`, `hold release` with a `{node}` wait, `node update {depends_on}`, `node hide`, and the
//! clock; WP-D4's `node key`, `node link`, `node move`, `node merge`, `node reopen`, and a create
//! that joins an open carrier; and WP-D5b's `talk request`, `said`, `ask` to a person, and `heard`.
//! So:
//! - `tests/traces/keys/` holds traces of the slice restricted to WP-D3's commands
//!   (`quint/conformance.qnt`, `keys_core_two` and `keys_core_one`);
//! - `tests/traces/keys_full/` holds traces of the whole slice (`keys_two`, `keys_one`);
//!
//! and with every layer merged both replay to their end.

mod common;

use common::{actions, divergences, incomplete, missing_ok, replay_group};
use forum_conformance::{Options, Verdict};

/// The implemented actions of the slice, each of which some `keys/` fixture replays with success.
/// `act_hide` is not among them: it hides a kept entry (`v_hide`), and of the slice's commands only
/// WP-D5b's `talk` commands keep one, so in `keys_core_*` it meets only `not_found(Entry)`.
const CORE_ACTIONS: [&str; 5] = [
    "act_create",
    "act_close",
    "act_release",
    "act_update",
    "act_tick",
];

#[test]
fn every_keys_fixture_of_the_implemented_commands_replays_to_its_end() {
    let reports = replay_group("keys", Options::default());
    let bad = incomplete(&reports);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    println!("{:?}", actions(&reports));
    let missing = missing_ok(&reports, &CORE_ACTIONS);
    assert!(
        missing.is_empty(),
        "never replayed with success: {missing:?}"
    );
}

/// The whole slice, up to the first command the core does not implement yet: no divergence before
/// it, and every stop is such a command.
#[test]
fn the_whole_keys_slice_replays_up_to_the_later_packages() {
    let reports = replay_group("keys_full", Options::default());
    let bad = divergences(&reports);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    for r in &reports {
        if let Verdict::Gap { reason, .. } = &r.verdict {
            assert!(reason.contains("BadParams"), "{}", r.describe());
        }
    }
}

#[test]
fn the_whole_keys_slice_replays_to_its_end() {
    let reports = replay_group("keys_full", Options { gaps_diverge: true });
    let bad = incomplete(&reports);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
