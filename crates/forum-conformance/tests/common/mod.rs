//! What the fixture tests share: replaying a group of `tests/traces/` and summing up.

#![allow(
    dead_code,
    clippy::unwrap_used,
    reason = "test helpers: each test file uses some of them"
)]

use std::collections::{BTreeMap, BTreeSet};

use forum_conformance::{Options, Report, Verdict, fixtures, replay_dir};

/// Replays every fixture of `group`, printing each report (`--nocapture` shows them).
pub fn replay_group(group: &str, opts: Options) -> Vec<Report> {
    let reports = replay_dir(&fixtures(group), opts).unwrap();
    assert!(!reports.is_empty(), "no fixtures in tests/traces/{group}");
    for r in &reports {
        println!("{}", r.describe());
    }
    reports
}

/// The reports that diverged, rendered.
pub fn divergences(reports: &[Report]) -> Vec<String> {
    reports
        .iter()
        .filter(|r| matches!(r.verdict, Verdict::Divergence { .. }))
        .map(Report::describe)
        .collect()
}

/// The reports that did not replay to their last state, rendered.
pub fn incomplete(reports: &[Report]) -> Vec<String> {
    reports
        .iter()
        .filter(|r| r.verdict != Verdict::Pass)
        .map(Report::describe)
        .collect()
}

/// Every replayed action with its result (`act_take ok`), over all reports, with counts.
pub fn actions(reports: &[Report]) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for r in reports {
        for (k, n) in &r.actions {
            *out.entry(k.clone()).or_default() += n;
        }
    }
    out
}

/// The names of `want` that no report replayed with result `ok`.
pub fn missing_ok(reports: &[Report], want: &[&str]) -> BTreeSet<String> {
    let seen = actions(reports);
    want.iter()
        .filter(|a| !seen.contains_key(&format!("{a} ok")) && !seen.contains_key(**a))
        .map(|a| (*a).to_owned())
        .collect()
}
