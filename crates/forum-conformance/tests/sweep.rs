//! An opt-in sweep (D.10.8): replays every ITF trace of the directory named by
//! `FORUM_CONFORMANCE_TRACES`, for example the hundreds that `scripts/traces.sh --sweep` writes
//! into `model/traces/conformance/` (git-ignored), so that a wide run needs no committed fixtures.
//!
//!   FORUM_CONFORMANCE_TRACES=model/traces/conformance/holds \
//!     cargo test -p forum-conformance --test sweep -- --ignored --nocapture

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code: a failure is the report"
)]

use std::path::PathBuf;

use forum_conformance::{Options, Verdict, replay_dir};

#[test]
#[ignore = "opt-in: set FORUM_CONFORMANCE_TRACES to a directory of ITF traces"]
#[allow(
    clippy::disallowed_methods,
    reason = "a test's input directory, not configuration of the server"
)]
fn sweep() {
    let dir = PathBuf::from(
        std::env::var("FORUM_CONFORMANCE_TRACES").expect("FORUM_CONFORMANCE_TRACES is not set"),
    );
    let gaps_diverge = std::env::var("FORUM_CONFORMANCE_GAPS_DIVERGE").is_ok();
    let reports = replay_dir(&dir, Options { gaps_diverge }).unwrap();
    let mut divergences = 0;
    let (mut states, mut replayed) = (0, 0);
    for r in &reports {
        println!("{}", r.describe());
        states += r.states;
        replayed += r.replayed;
        if matches!(r.verdict, Verdict::Divergence { .. }) {
            divergences += 1;
        }
    }
    println!(
        "{} traces, {replayed} of {states} states replayed, {divergences} with a divergence",
        reports.len()
    );
    assert_eq!(divergences, 0);
}
