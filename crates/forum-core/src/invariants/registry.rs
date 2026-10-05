//! The invariant registry (design D.9.4): one row per `Inv`, at the verification level D.0.3 gives
//! it (decided on 2026-10-02). The **key invariants** (H1 to H6, O1, O2, K1, K3, K4,
//! K5, T1 to T5, and the totality of `evolve`, `Total`) are checked formally: each row names its
//! Quint checks (TLC on the `holds` and `keys` slices) and its Kani harnesses, beside its
//! executable check and tests. **Every other invariant** is checked by its executable check
//! (`check_invariants`, `check_step`, `check_fire`, or `check_guide`) and by the tests that
//! exercise it; its row records both, and names a Quint check or a Kani harness only where one
//! exists, which D.0.3 no longer requires. A level with no check gives the reason instead. Each row
//! also names its Verus TODO (D.13.4: after the first release, for the key invariants).
//!
//! `docs/verification.md` holds the tables generated from `INVARIANTS` by
//! `scripts/gen-verification-table.py`, whose `--check` `scripts/verify.sh` and CI run;
//! `tests/registry.rs` checks that every name and path a row gives exists.

use super::Inv;

/// What kind of invariant a row is.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum InvKind {
    /// A state invariant, checked by `check_invariants`.
    State,
    /// A step invariant, checked by `check_step` or `check_fire`.
    Step,
    /// A property of runs or responses (T2, G1), or a definition (AR).
    Property,
}

/// The verification level of a row (D.0.3).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum Level {
    /// A key invariant: its executable check and tests, the Quint model's checks, and Kani
    /// harnesses; a Verus proof later.
    Key,
    /// Every other invariant: its executable check and the tests that exercise it ("tests").
    Tests,
}

/// How far the Verus proof of a row is.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum VerusStatus {
    /// Not started (D.13.4: Verus proofs come after the first release).
    Todo,
    /// Started.
    InProgress,
    /// Proved.
    Proved,
}

/// One row of the registry. Serialize only under the `serde` feature (D.2): it holds `&'static`
/// data.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct InvariantInfo {
    /// The invariant.
    pub id: Inv,
    /// H2, H3, H5, O1, O2, Settled, D1, U1, R2, T1, T4, L1: State and Step.
    pub kinds: &'static [InvKind],
    /// D.0.3's level: `Key` exactly for the key invariants.
    pub level: Level,
    /// The executable check that runs it (D.9.1), in words.
    pub check: &'static str,
    /// The test files that exercise it by name, from the repository's root: the mutations its
    /// check must reject, and the flows that reach it. Every test also runs every check after
    /// every call and firing (`World`), and so do the property test and the §32 scenario.
    pub tests: &'static [&'static str],
    /// Names in `model/*.qnt`.
    pub quint: &'static [&'static str],
    /// Why there is no Quint check, or for which part.
    pub quint_reason: Option<&'static str>,
    /// `#[kani::proof]` functions.
    pub kani: &'static [&'static str],
    /// Why there is no Kani harness.
    pub kani_reason: Option<&'static str>,
    /// The Verus proof's status.
    pub verus: VerusStatus,
    /// The tracked TODO, id `V-<Inv>`.
    pub verus_todo: &'static str,
}

const S: &[InvKind] = &[InvKind::State];
const ST: &[InvKind] = &[InvKind::Step];
const S_ST: &[InvKind] = &[InvKind::State, InvKind::Step];
const P: &[InvKind] = &[InvKind::Property];

/// The key invariants of D.0.3, in the registry's order.
pub const KEY: [Inv; 18] = [
    Inv::H1,
    Inv::H2,
    Inv::H3,
    Inv::H4,
    Inv::H5,
    Inv::H6,
    Inv::O1,
    Inv::O2,
    Inv::K1,
    Inv::K3,
    Inv::K4,
    Inv::K5,
    Inv::T1,
    Inv::T2,
    Inv::T3,
    Inv::T4,
    Inv::T5,
    Inv::Total,
];

/// One row per `Inv`, the table of D.9.2 at D.0.3's levels. Every row starts with
/// `VerusStatus::Todo`.
pub const INVARIANTS: &[InvariantInfo] = &[
    InvariantInfo {
        id: Inv::H1,
        kinds: S,
        level: Level::Key,
        check: "`check_invariants`",
        tests: &["crates/forum-core/src/evolve_tests.rs"],
        quint: &["inv_H1"],
        quint_reason: None,
        kani: &["hold_cmds_preserve_h"],
        kani_reason: None,
        verus: VerusStatus::Todo,
        verus_todo: "V-H1: evolve of each event that apply and settle emit preserves H1",
    },
    InvariantInfo {
        id: Inv::H2,
        kinds: S_ST,
        level: Level::Key,
        check: "`check_invariants`, `check_step`, `check_fire`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/evolve_check_tests.rs",
            "crates/forum-core/src/invariants/step_tests.rs",
        ],
        quint: &["inv_H2", "step_H2"],
        quint_reason: None,
        kani: &[
            "hold_cmds_preserve_h",
            "session_retire",
            "silence_after_leases",
        ],
        kani_reason: None,
        verus: VerusStatus::Todo,
        verus_todo: "V-H2: evolve of each event that apply and settle emit preserves H2; every \
                     (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, \
                     satisfies H2's step predicate",
    },
    InvariantInfo {
        id: Inv::H3,
        kinds: S_ST,
        level: Level::Key,
        check: "`check_invariants`, `check_step`, `check_fire`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/invariants/step_tests.rs",
        ],
        quint: &["inv_H3", "step_H3"],
        quint_reason: None,
        kani: &["hold_cmds_preserve_h"],
        kani_reason: None,
        verus: VerusStatus::Todo,
        verus_todo: "V-H3: evolve of each event that apply and settle emit preserves H3; every \
                     (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, \
                     satisfies H3's step predicate",
    },
    InvariantInfo {
        id: Inv::H4,
        kinds: ST,
        level: Level::Key,
        check: "`check_step`, `check_fire`",
        tests: &["crates/forum-core/src/invariants/step_tests.rs"],
        quint: &["step_H4"],
        quint_reason: None,
        kani: &["stale_token_only_late"],
        kani_reason: None,
        verus: VerusStatus::Todo,
        verus_todo: "V-H4: every (s, ctx, cmd) with decide's events, and every (s, key) with \
                     fire's events, satisfies H4's step predicate",
    },
    InvariantInfo {
        id: Inv::H5,
        kinds: S_ST,
        level: Level::Key,
        check: "`check_invariants`, `check_step`, `check_fire`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/evolve_check_tests.rs",
            "crates/forum-core/src/invariants/step_tests.rs",
        ],
        quint: &["inv_H5"],
        quint_reason: None,
        kani: &["lease_issue_h5", "lease_extend_h5", "renew_limit"],
        kani_reason: None,
        verus: VerusStatus::Todo,
        verus_todo: "V-H5: evolve of each event that apply and settle emit preserves H5; every \
                     (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, \
                     satisfies H5's step predicate",
    },
    InvariantInfo {
        id: Inv::H6,
        kinds: S,
        level: Level::Key,
        check: "`check_invariants`",
        tests: &["crates/forum-core/src/evolve_tests.rs"],
        quint: &["inv_H6"],
        quint_reason: None,
        kani: &["close_ends_holds", "take_gate_closed_nodes"],
        kani_reason: None,
        verus: VerusStatus::Todo,
        verus_todo: "V-H6: evolve of each event that apply and settle emit preserves H6",
    },
    InvariantInfo {
        id: Inv::O1,
        kinds: S_ST,
        level: Level::Key,
        check: "`check_invariants`, `check_step`, `check_fire`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/invariants/step_tests.rs",
        ],
        quint: &["step_O1", "inv_O1"],
        quint_reason: None,
        kani: &["op_retry_no_effect", "op_expiry"],
        kani_reason: None,
        verus: VerusStatus::Todo,
        verus_todo: "V-O1: exactly-once op_id; evolve of each event that apply and settle emit \
                     preserves O1; every (s, ctx, cmd) with decide's events, and every (s, key) \
                     with fire's events, satisfies O1's step predicate",
    },
    InvariantInfo {
        id: Inv::O2,
        kinds: S_ST,
        level: Level::Key,
        check: "`check_invariants`, `check_step`, `check_fire`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/evolve_check_tests.rs",
            "crates/forum-core/src/invariants/step_tests.rs",
        ],
        quint: &["inv_O2", "step_O2"],
        quint_reason: None,
        kani: &["version_bumps_o2"],
        kani_reason: None,
        verus: VerusStatus::Todo,
        verus_todo: "V-O2: evolve of each event that apply and settle emit preserves O2; every \
                     (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, \
                     satisfies O2's step predicate",
    },
    InvariantInfo {
        id: Inv::O3,
        kinds: ST,
        level: Level::Tests,
        check: "`check_step`, `check_fire`",
        tests: &["crates/forum-core/src/invariants/step_tests.rs"],
        quint: &["step_O3"],
        quint_reason: None,
        kani: &[],
        kani_reason: Some(
            "structural: the core has no hidden status to read, and the prefix property is the \
             shell's append-only history; the step check runs in every test",
        ),
        verus: VerusStatus::Todo,
        verus_todo: "V-O3: every (s, ctx, cmd) with decide's events, and every (s, key) with \
                     fire's events, satisfies O3's step predicate",
    },
    InvariantInfo {
        id: Inv::R1,
        kinds: S,
        level: Level::Tests,
        check: "`check_invariants`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/evolve_check_tests.rs",
        ],
        quint: &[],
        quint_reason: Some("the model has no indices"),
        kani: &[
            "evolve_no_panic_sessions",
            "evolve_no_panic_nodes",
            "evolve_no_panic_holds",
            "evolve_no_panic_entities",
        ],
        kani_reason: None,
        verus: VerusStatus::Todo,
        verus_todo: "V-R1: recount ∘ evolve = evolve_index ∘ recount; evolve of each event that \
                     apply and settle emit preserves R1",
    },
    InvariantInfo {
        id: Inv::K1,
        kinds: S,
        level: Level::Key,
        check: "`check_invariants`",
        tests: &["crates/forum-core/src/evolve_tests.rs"],
        quint: &["inv_K1"],
        quint_reason: None,
        kani: &["keys_open_unique"],
        kani_reason: None,
        verus: VerusStatus::Todo,
        verus_todo: "V-K1: evolve of each event that apply and settle emit preserves K1",
    },
    InvariantInfo {
        id: Inv::K2,
        kinds: S,
        level: Level::Tests,
        check: "`check_invariants`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/evolve_check_tests.rs",
            "crates/forum-keys/tests/golden.rs",
        ],
        quint: &[],
        quint_reason: Some("keys are model atoms (D.10.6)"),
        kani: &[],
        kani_reason: Some("a shell property: the proptests of forum-keys"),
        verus: VerusStatus::Todo,
        verus_todo: "V-K2: evolve of each event that apply and settle emit preserves K2's core \
                     part (version 0 keys and origins are canonical, no repeat: key of another \
                     version)",
    },
    InvariantInfo {
        id: Inv::K3,
        kinds: S,
        level: Level::Key,
        check: "`check_invariants`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/evolve_check_tests.rs",
        ],
        quint: &["inv_K3"],
        quint_reason: None,
        kani: &["move_keeps_tree"],
        kani_reason: None,
        verus: VerusStatus::Todo,
        verus_todo: "V-K3: evolve of each event that apply and settle emit preserves K3",
    },
    InvariantInfo {
        id: Inv::K4,
        kinds: S,
        level: Level::Key,
        check: "`check_invariants`",
        tests: &["crates/forum-core/src/evolve_tests.rs"],
        quint: &["inv_K4"],
        quint_reason: None,
        kani: &["move_keeps_tree"],
        kani_reason: None,
        verus: VerusStatus::Todo,
        verus_todo: "V-K4: evolve of each event that apply and settle emit preserves K4",
    },
    InvariantInfo {
        id: Inv::K5,
        kinds: S,
        level: Level::Key,
        check: "`check_invariants`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/evolve_check_tests.rs",
        ],
        quint: &["inv_K5"],
        quint_reason: None,
        kani: &["merge_redirects"],
        kani_reason: None,
        verus: VerusStatus::Todo,
        verus_todo: "V-K5: evolve of each event that apply and settle emit preserves K5",
    },
    InvariantInfo {
        id: Inv::K6,
        kinds: S,
        level: Level::Tests,
        check: "`check_invariants`",
        tests: &["crates/forum-core/src/evolve_tests.rs"],
        quint: &["inv_K6"],
        quint_reason: None,
        kani: &[],
        kani_reason: Some("D.0.3: not a key invariant: its executable check and tests"),
        verus: VerusStatus::Todo,
        verus_todo: "V-K6: evolve of each event that apply and settle emit preserves K6",
    },
    InvariantInfo {
        id: Inv::K7,
        kinds: ST,
        level: Level::Tests,
        check: "`check_step`, `check_fire`",
        tests: &["crates/forum-core/src/cmd/restructure_tests.rs"],
        quint: &["step_K7"],
        quint_reason: None,
        kani: &[],
        kani_reason: Some("D.0.3: not a key invariant: its executable check and tests"),
        verus: VerusStatus::Todo,
        verus_todo: "V-K7: every (s, ctx, cmd) with decide's events, and every (s, key) with \
                     fire's events, satisfies K7's step predicate",
    },
    InvariantInfo {
        id: Inv::K8,
        kinds: S,
        level: Level::Tests,
        check: "`check_invariants`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/evolve_check_tests.rs",
        ],
        quint: &["inv_K8"],
        quint_reason: None,
        kani: &[],
        kani_reason: Some("D.0.3: not a key invariant: its executable check and tests"),
        verus: VerusStatus::Todo,
        verus_todo: "V-K8: evolve of each event that apply and settle emit preserves K8",
    },
    InvariantInfo {
        id: Inv::C1,
        kinds: ST,
        level: Level::Tests,
        check: "`check_step`, `check_fire`",
        tests: &["crates/forum-core/src/cmd/restructure_tests.rs"],
        quint: &["step_C1"],
        quint_reason: None,
        kani: &[],
        kani_reason: Some("D.0.3: not a key invariant: its executable check and tests"),
        verus: VerusStatus::Todo,
        verus_todo: "V-C1: every (s, ctx, cmd) with decide's events, and every (s, key) with \
                     fire's events, satisfies C1's step predicate",
    },
    InvariantInfo {
        id: Inv::Q1,
        kinds: S,
        level: Level::Tests,
        check: "`check_invariants`",
        tests: &["crates/forum-core/src/evolve_tests.rs"],
        quint: &["inv_Q1"],
        quint_reason: None,
        kani: &[],
        kani_reason: Some("D.0.3: not a key invariant: its executable check and tests"),
        verus: VerusStatus::Todo,
        verus_todo: "V-Q1: evolve of each event that apply and settle emit preserves Q1",
    },
    InvariantInfo {
        id: Inv::S1,
        kinds: S_ST,
        level: Level::Tests,
        check: "`check_invariants`, `check_step`, `check_fire`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/evolve_check_tests.rs",
            "crates/forum-core/src/invariants/step_tests.rs",
            "crates/forum-core/src/cmd/tidy_tests.rs",
        ],
        quint: &["inv_S1"],
        quint_reason: Some(
            "1.11's step clause, a duplicate signal resolved with its pair's second close: its \
             executable check and tests (D.0.3); the model mirrors the close \
             (`resolve_closed_duplicates`), which the conformance replay compares",
        ),
        kani: &[],
        kani_reason: Some("D.0.3: not a key invariant: its executable check and tests"),
        verus: VerusStatus::Todo,
        verus_todo: "V-S1: evolve of each event that apply and settle emit preserves S1; every \
                     (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, \
                     satisfies S1's step predicate",
    },
    InvariantInfo {
        id: Inv::N1,
        kinds: S_ST,
        level: Level::Tests,
        check: "`check_invariants`, `check_step`; `check_fire` has no clause of it, since no firing \
                logs or routes an entry",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/invariants/step_tests.rs",
            "crates/forum-core/src/cmd/talk_user_tests.rs",
        ],
        quint: &["inv_N1"],
        quint_reason: Some(
            "1.11's step clause, a follow-up routed as a clarification by default: its executable \
             check and tests (D.0.3); the model mirrors the routing (`from_user_ex`), which the \
             conformance replay compares",
        ),
        kani: &[],
        kani_reason: Some("D.0.3: not a key invariant: its executable check and tests"),
        verus: VerusStatus::Todo,
        verus_todo: "V-N1: evolve of each event that apply and settle emit preserves N1; every \
                     (s, ctx, cmd) with decide's events satisfies N1's step predicate",
    },
    InvariantInfo {
        id: Inv::Settled,
        kinds: S_ST,
        level: Level::Tests,
        check: "`check_invariants`, `check_step`, `check_fire`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/evolve_check_tests.rs",
            "crates/forum-core/src/invariants/step_tests.rs",
            "crates/forum-core/src/cmd/restructure_tests.rs",
            "crates/forum-core/src/cmd/finish_tests.rs",
        ],
        quint: &["inv_Settled", "step_Settled"],
        quint_reason: Some(
            "1.12's step clause, a release's decision (`release_decision`): its executable check \
             and tests (D.0.3); the model mirrors the release (`release_ex`, `record_decision`), \
             which the conformance replay compares",
        ),
        kani: &[],
        kani_reason: Some("D.0.3: not a key invariant: its executable check and tests"),
        verus: VerusStatus::Todo,
        verus_todo: "V-Settled: evolve of each event that apply and settle emit preserves \
                     Settled; every (s, ctx, cmd) with decide's events, and every (s, key) with \
                     fire's events, satisfies Settled's step predicate",
    },
    InvariantInfo {
        id: Inv::F,
        kinds: S,
        level: Level::Tests,
        check: "`check_invariants`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/evolve_check_tests.rs",
        ],
        quint: &["inv_F"],
        quint_reason: None,
        kani: &[],
        kani_reason: Some("D.0.3: not a key invariant: its executable check and tests"),
        verus: VerusStatus::Todo,
        verus_todo: "V-F: flag monotonicity between events (the lemma behind F2); evolve of each \
                     event that apply and settle emit preserves F",
    },
    InvariantInfo {
        id: Inv::N5,
        kinds: ST,
        level: Level::Tests,
        check: "`check_step`, `check_fire`",
        tests: &["crates/forum-core/src/cmd/restructure_tests.rs"],
        quint: &["step_N5"],
        quint_reason: None,
        kani: &["merge_redirects"],
        kani_reason: None,
        verus: VerusStatus::Todo,
        verus_todo: "V-N5: every (s, ctx, cmd) with decide's events, and every (s, key) with \
                     fire's events, satisfies N5's step predicate",
    },
    InvariantInfo {
        id: Inv::E1,
        kinds: S_ST,
        level: Level::Tests,
        check: "`check_invariants`, `check_step`, `check_fire`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/evolve_check_tests.rs",
            "crates/forum-core/src/cmd/cross_tests.rs",
            "crates/forum-core/src/invariants/step_tests.rs",
            "crates/forum-core/src/cmd/finish_tests.rs",
        ],
        quint: &["inv_E1"],
        quint_reason: Some(
            "1.12's step clause, a question withdrawn only by its retract: its executable check and \
             tests (D.0.3); the model mirrors the retract (`retract_ex`), which the conformance \
             replay compares",
        ),
        kani: &[],
        kani_reason: Some("D.0.3: not a key invariant: its executable check and tests"),
        verus: VerusStatus::Todo,
        verus_todo: "V-E1: evolve of each event that apply and settle emit preserves E1; every \
                     (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, \
                     satisfies E1's step predicate",
    },
    InvariantInfo {
        id: Inv::W1,
        kinds: S,
        level: Level::Tests,
        check: "`check_invariants`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/evolve_check_tests.rs",
        ],
        quint: &["inv_W1"],
        quint_reason: None,
        kani: &[],
        kani_reason: Some("D.0.3: not a key invariant: its executable check and tests"),
        verus: VerusStatus::Todo,
        verus_todo: "V-W1: evolve of each event that apply and settle emit preserves W1",
    },
    InvariantInfo {
        id: Inv::R2,
        kinds: S_ST,
        level: Level::Tests,
        check: "`check_invariants`, `check_step`, `check_fire`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/invariants/step_tests.rs",
            "crates/forum-core/src/cmd/recurring_tests.rs",
        ],
        quint: &["inv_R2", "step_R2"],
        quint_reason: None,
        kani: &[],
        kani_reason: Some("D.0.3: not a key invariant: its executable check and tests"),
        verus: VerusStatus::Todo,
        verus_todo: "V-R2: evolve of each event that apply and settle emit preserves R2; every \
                     (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, \
                     satisfies R2's step predicate",
    },
    InvariantInfo {
        id: Inv::D1,
        kinds: S_ST,
        level: Level::Tests,
        check: "`check_invariants`, `check_step`, `check_fire`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/evolve_check_tests.rs",
            "crates/forum-core/src/invariants/step_tests.rs",
        ],
        quint: &["inv_D1", "step_D1"],
        quint_reason: None,
        kani: &[],
        kani_reason: Some(
            "needs firing chains across questions, defaults, and digest entries, beyond Kani's \
             bounds on BTreeMap states: the proptests and Quint's user and people slices",
        ),
        verus: VerusStatus::Todo,
        verus_todo: "V-D1: evolve of each event that apply and settle emit preserves D1; every \
                     (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, \
                     satisfies D1's step predicate",
    },
    InvariantInfo {
        id: Inv::M1,
        kinds: S,
        level: Level::Tests,
        check: "`check_invariants`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/evolve_check_tests.rs",
            "crates/forum-core/src/cmd/cross_tests.rs",
        ],
        quint: &["inv_M1"],
        quint_reason: None,
        kani: &[],
        kani_reason: Some(
            "quantifies over every entity kind of a whole state: the proptests and every Quint \
             slice",
        ),
        verus: VerusStatus::Todo,
        verus_todo: "V-M1: evolve of each event that apply and settle emit preserves M1a",
    },
    InvariantInfo {
        id: Inv::U1,
        kinds: S_ST,
        level: Level::Tests,
        check: "`check_invariants`, `check_step`, `check_fire`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/evolve_check_tests.rs",
            "crates/forum-core/src/invariants/step_tests.rs",
        ],
        quint: &["inv_U1", "step_U1", "hello_exact"],
        quint_reason: None,
        kani: &[],
        kani_reason: Some("D.0.3: not a key invariant: its executable check and tests"),
        verus: VerusStatus::Todo,
        verus_todo: "V-U1: evolve of each event that apply and settle emit preserves U1; every \
                     (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, \
                     satisfies U1's step predicate",
    },
    InvariantInfo {
        id: Inv::T1,
        kinds: S_ST,
        level: Level::Key,
        check: "`check_invariants`, `check_step`, `check_fire`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/invariants/step_tests.rs",
        ],
        quint: &["inv_T1", "step_T1"],
        quint_reason: None,
        kani: &["event_stamps_t1"],
        kani_reason: None,
        verus: VerusStatus::Todo,
        verus_todo: "V-T1: evolve of each event that apply and settle emit preserves T1; every \
                     (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, \
                     satisfies T1's step predicate",
    },
    InvariantInfo {
        id: Inv::T2,
        kinds: P,
        level: Level::Key,
        check: "the T2 item of the property test, at random points (D.12.3)",
        tests: &["crates/forum-core/tests/prop.rs"],
        quint: &["inv_T2", "test_T2"],
        quint_reason: None,
        kani: &["settle_path_independent_small"],
        kani_reason: None,
        verus: VerusStatus::Todo,
        verus_todo: "V-T2: settle idempotence and path independence, by induction on the pops \
                     through the lemmas L1 and L2 of D.8.2",
    },
    InvariantInfo {
        id: Inv::T3,
        kinds: S,
        level: Level::Key,
        check: "`check_invariants`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/evolve_check_tests.rs",
        ],
        quint: &["inv_T3"],
        quint_reason: None,
        kani: &["settle_idempotent_small"],
        kani_reason: None,
        verus: VerusStatus::Todo,
        verus_todo: "V-T3: every reply equals the reply on the settled state; evolve of each \
                     event that apply and settle emit preserves T3",
    },
    InvariantInfo {
        id: Inv::T4,
        kinds: S_ST,
        level: Level::Key,
        check: "`check_invariants`, `check_step`, `check_fire`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/evolve_check_tests.rs",
            "crates/forum-core/src/invariants/step_tests.rs",
            "crates/forum-core/src/cmd/recurring_tests.rs",
            "crates/forum-core/src/cmd/talk_people_questions_tests.rs",
        ],
        quint: &["inv_T4", "step_T4"],
        quint_reason: None,
        kani: &["deadline_order", "fire_once_small"],
        kani_reason: None,
        verus: VerusStatus::Todo,
        verus_todo: "V-T4: recount ∘ evolve = evolve_index ∘ recount for the deadline queue; \
                     evolve of each event that apply and settle emit preserves T4; every (s, \
                     ctx, cmd) with decide's events, and every (s, key) with fire's events, \
                     satisfies T4's step predicate",
    },
    InvariantInfo {
        id: Inv::T5,
        kinds: S,
        level: Level::Key,
        check: "`check_invariants`",
        tests: &["crates/forum-core/src/evolve_tests.rs"],
        quint: &["inv_T5"],
        quint_reason: None,
        kani: &["reaches_terminates", "merge_redirects"],
        kani_reason: None,
        verus: VerusStatus::Todo,
        verus_todo: "V-T5: evolve of each event that apply and settle emit preserves T5",
    },
    InvariantInfo {
        id: Inv::L1,
        kinds: S_ST,
        level: Level::Tests,
        check: "`check_invariants`, `check_step`, `check_fire`",
        tests: &[
            "crates/forum-core/src/evolve_tests.rs",
            "crates/forum-core/src/invariants/step_tests.rs",
            "crates/forum-core/src/cmd/restructure_tests.rs",
        ],
        quint: &["inv_L1", "step_L1"],
        quint_reason: None,
        kani: &[],
        kani_reason: Some("D.0.3: not a key invariant: its executable check and tests"),
        verus: VerusStatus::Todo,
        verus_todo: "V-L1: evolve of each event that apply and settle emit preserves L1's depth \
                     half; every (s, ctx, cmd) with decide's events, and every (s, key) with \
                     fire's events, satisfies L1's step predicate (children and texts)",
    },
    InvariantInfo {
        id: Inv::P1,
        kinds: S,
        level: Level::Tests,
        check: "`check_invariants`",
        tests: &["crates/forum-core/src/evolve_tests.rs"],
        quint: &["inv_P1"],
        quint_reason: None,
        kani: &[],
        kani_reason: Some("D.0.3: not a key invariant: its executable check and tests"),
        verus: VerusStatus::Todo,
        verus_todo: "V-P1: evolve of each event that apply and settle emit preserves P1",
    },
    InvariantInfo {
        id: Inv::P2,
        kinds: ST,
        level: Level::Tests,
        check: "`check_step`, `check_fire`",
        tests: &["crates/forum-core/src/invariants/step_tests.rs"],
        quint: &["step_P2"],
        quint_reason: None,
        kani: &["hold_cmds_preserve_h"],
        kani_reason: None,
        verus: VerusStatus::Todo,
        verus_todo: "V-P2: every (s, ctx, cmd) with decide's events, and every (s, key) with \
                     fire's events, satisfies P2's step predicate",
    },
    InvariantInfo {
        id: Inv::AR,
        kinds: P,
        level: Level::Tests,
        check: "a unit test of `query::list` (D.9.2)",
        tests: &["crates/forum-core/src/query/tests.rs"],
        quint: &["archived"],
        quint_reason: None,
        kani: &[],
        kani_reason: Some(
            "a definition with no state to check: the unit test in the Rust check column",
        ),
        verus: VerusStatus::Todo,
        verus_todo: "V-AR: archived is a function of (S, now) that no state field stores",
    },
    InvariantInfo {
        id: Inv::G1,
        kinds: P,
        level: Level::Tests,
        check: "`check_guide`, after every call in every test (D.9.1)",
        tests: &[
            "crates/forum-core/src/query/guide_tests.rs",
            "crates/forum-core/tests/prop.rs",
            "crates/forum-core/tests/scenario_32.rs",
        ],
        quint: &[],
        quint_reason: Some(
            "D.0.3 dropped WP-A5, the next calls in the model: G1 is checked in Rust by \
             `check_guide`, and reads, `session hello`, texts, and rendering have no state effect \
             to model",
        ),
        kani: &[],
        kani_reason: Some("D.0.3: not a key invariant: its executable check and tests"),
        verus: VerusStatus::Todo,
        verus_todo: "V-G1: every item of guide on a settled state satisfies validate's \
                     precondition, and a Close item's plan raises none of the four warnings",
    },
    InvariantInfo {
        id: Inv::Total,
        kinds: S,
        level: Level::Key,
        check: "`check_invariants`",
        tests: &["crates/forum-core/src/evolve_tests.rs"],
        quint: &[],
        quint_reason: Some(
            "the model applies transitions directly and has no evolve preconditions",
        ),
        kani: &["evolve_counts_anomalies"],
        kani_reason: None,
        verus: VerusStatus::Todo,
        verus_todo: "V-Total: every event that apply and settle emit passes evolve's \
                     precondition, so anomalies stays 0; and a termination measure for every \
                     loop of the core (D.9.4)",
    },
];

/// The registry row of an invariant.
#[must_use]
pub fn info(inv: Inv) -> Option<&'static InvariantInfo> {
    INVARIANTS.iter().find(|row| row.id == inv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_inv_has_exactly_one_row() {
        for inv in Inv::ALL {
            assert_eq!(
                INVARIANTS.iter().filter(|row| row.id == inv).count(),
                1,
                "{inv:?}"
            );
        }
        assert_eq!(INVARIANTS.len(), Inv::ALL.len());
    }

    #[test]
    fn every_row_has_a_check_or_a_reason_at_every_level() {
        for row in INVARIANTS {
            assert!(!row.check.is_empty(), "{:?}: executable check", row.id);
            assert!(!row.tests.is_empty(), "{:?}: tests", row.id);
            assert!(
                !row.quint.is_empty() || row.quint_reason.is_some(),
                "{:?}: Quint",
                row.id
            );
            assert!(
                !row.kani.is_empty() || row.kani_reason.is_some(),
                "{:?}: Kani",
                row.id
            );
            assert!(
                row.verus_todo.starts_with("V-") && row.verus_todo.len() > 3,
                "{:?}: Verus",
                row.id
            );
            assert_eq!(row.verus, VerusStatus::Todo, "{:?}", row.id);
            assert!(!row.kinds.is_empty(), "{:?}: kinds", row.id);
            for name in row.quint.iter().chain(row.kani) {
                assert!(
                    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                    "{:?}: {name:?}",
                    row.id
                );
            }
            for path in row.tests {
                assert!(
                    path.starts_with("crates/") && path.ends_with(".rs"),
                    "{:?}: {path}",
                    row.id
                );
            }
        }
    }

    #[test]
    fn the_verus_todo_ids_name_their_row() {
        for row in INVARIANTS {
            let id = alloc::format!("V-{:?}:", row.id);
            assert!(row.verus_todo.starts_with(&id), "{:?}", row.id);
        }
    }

    #[test]
    fn kinds_follow_d_9_2() {
        let both = [
            Inv::H2,
            Inv::H3,
            Inv::H5,
            Inv::O1,
            Inv::O2,
            Inv::Settled,
            Inv::D1,
            Inv::U1,
            Inv::R2,
            Inv::T1,
            Inv::T4,
            Inv::L1,
            // 1.11: the step clauses of the routing default and of a duplicate pair's closes.
            Inv::N1,
            Inv::S1,
            // 1.12: the step clause of a withdrawn question.
            Inv::E1,
        ];
        let step = [Inv::H4, Inv::O3, Inv::K7, Inv::C1, Inv::N5, Inv::P2];
        let property = [Inv::T2, Inv::AR, Inv::G1];
        for row in INVARIANTS {
            let expected: &[InvKind] = if both.contains(&row.id) {
                S_ST
            } else if step.contains(&row.id) {
                ST
            } else if property.contains(&row.id) {
                P
            } else {
                S
            };
            assert_eq!(row.kinds, expected, "{:?}", row.id);
        }
        assert_eq!(info(Inv::G1).map(|r| r.kinds), Some(P));
    }

    /// D.0.3: the key invariants, and only they, are `Level::Key`, each with a Quint check and a
    /// Kani harness; a state invariant's check is `check_invariants`, and a step invariant's
    /// `check_step` and `check_fire`.
    #[test]
    fn levels_follow_d_0_3() {
        for row in INVARIANTS {
            let key = KEY.contains(&row.id);
            assert_eq!(row.level == Level::Key, key, "{:?}: level", row.id);
            if key && row.id != Inv::Total {
                assert!(
                    !row.quint.is_empty(),
                    "{:?}: a key invariant's Quint",
                    row.id
                );
            }
            if key {
                assert!(!row.kani.is_empty(), "{:?}: a key invariant's Kani", row.id);
            }
            if row.kinds.contains(&InvKind::State) {
                assert!(row.check.contains("`check_invariants`"), "{:?}", row.id);
            }
            if row.kinds.contains(&InvKind::Step) {
                assert!(
                    row.check.contains("`check_step`") && row.check.contains("`check_fire`"),
                    "{:?}",
                    row.id
                );
            }
        }
        assert_eq!(
            info(Inv::G1).map(|r| r.check.contains("`check_guide`")),
            Some(true)
        );
    }

    /// The Quint and Kani columns at D.0.3's levels, row by row: D.9.2's names for the key
    /// invariants, with `silence_after_leases` for H2 (1.4); for the others only the checks that
    /// exist (`tests/registry.rs` checks that each name is in `model/` or under
    /// `#[kani::proof]`). A row that loses or renames a check fails here.
    #[allow(clippy::type_complexity, reason = "a table")]
    const LEVELS: [(Inv, &[&str], &[&str]); 42] = [
        (Inv::H1, &["inv_H1"], &["hold_cmds_preserve_h"]),
        (
            Inv::H2,
            &["inv_H2", "step_H2"],
            &[
                "hold_cmds_preserve_h",
                "session_retire",
                "silence_after_leases",
            ],
        ),
        (Inv::H3, &["inv_H3", "step_H3"], &["hold_cmds_preserve_h"]),
        (Inv::H4, &["step_H4"], &["stale_token_only_late"]),
        (
            Inv::H5,
            &["inv_H5"],
            &["lease_issue_h5", "lease_extend_h5", "renew_limit"],
        ),
        (
            Inv::H6,
            &["inv_H6"],
            &["close_ends_holds", "take_gate_closed_nodes"],
        ),
        (
            Inv::O1,
            &["step_O1", "inv_O1"],
            &["op_retry_no_effect", "op_expiry"],
        ),
        (Inv::O2, &["inv_O2", "step_O2"], &["version_bumps_o2"]),
        (Inv::O3, &["step_O3"], &[]),
        (
            Inv::R1,
            &[],
            &[
                "evolve_no_panic_sessions",
                "evolve_no_panic_nodes",
                "evolve_no_panic_holds",
                "evolve_no_panic_entities",
            ],
        ),
        (Inv::K1, &["inv_K1"], &["keys_open_unique"]),
        (Inv::K2, &[], &[]),
        (Inv::K3, &["inv_K3"], &["move_keeps_tree"]),
        (Inv::K4, &["inv_K4"], &["move_keeps_tree"]),
        (Inv::K5, &["inv_K5"], &["merge_redirects"]),
        (Inv::K6, &["inv_K6"], &[]),
        (Inv::K7, &["step_K7"], &[]),
        (Inv::K8, &["inv_K8"], &[]),
        (Inv::C1, &["step_C1"], &[]),
        (Inv::Q1, &["inv_Q1"], &[]),
        (Inv::S1, &["inv_S1"], &[]),
        (Inv::N1, &["inv_N1"], &[]),
        (Inv::Settled, &["inv_Settled", "step_Settled"], &[]),
        (Inv::F, &["inv_F"], &[]),
        (Inv::N5, &["step_N5"], &["merge_redirects"]),
        (Inv::E1, &["inv_E1"], &[]),
        (Inv::W1, &["inv_W1"], &[]),
        (Inv::R2, &["inv_R2", "step_R2"], &[]),
        (Inv::D1, &["inv_D1", "step_D1"], &[]),
        (Inv::M1, &["inv_M1"], &[]),
        (Inv::U1, &["inv_U1", "step_U1", "hello_exact"], &[]),
        (Inv::T1, &["inv_T1", "step_T1"], &["event_stamps_t1"]),
        (
            Inv::T2,
            &["inv_T2", "test_T2"],
            &["settle_path_independent_small"],
        ),
        (Inv::T3, &["inv_T3"], &["settle_idempotent_small"]),
        (
            Inv::T4,
            &["inv_T4", "step_T4"],
            &["deadline_order", "fire_once_small"],
        ),
        (
            Inv::T5,
            &["inv_T5"],
            &["reaches_terminates", "merge_redirects"],
        ),
        (Inv::L1, &["inv_L1", "step_L1"], &[]),
        (Inv::P1, &["inv_P1"], &[]),
        (Inv::P2, &["step_P2"], &["hold_cmds_preserve_h"]),
        (Inv::AR, &["archived"], &[]),
        (Inv::G1, &[], &[]),
        (Inv::Total, &[], &["evolve_counts_anomalies"]),
    ];

    #[test]
    fn checks_follow_the_levels() {
        assert_eq!(LEVELS.len(), INVARIANTS.len());
        for (id, quint, kani) in LEVELS {
            assert_eq!(info(id).map(|r| r.quint), Some(quint), "{id:?}: Quint");
            assert_eq!(info(id).map(|r| r.kani), Some(kani), "{id:?}: Kani");
        }
        // Where a row gives a reason instead of, or beside, a check.
        // 1.11: N1's and S1's step clauses are not in the model's invariants (D.0.3); 1.12: nor is
        // E1's, nor Settled's release clause, beside the step clauses `step_Settled` has.
        let quint_reasons = [
            Inv::R1,
            Inv::K2,
            Inv::G1,
            Inv::Total,
            Inv::N1,
            Inv::S1,
            Inv::E1,
            Inv::Settled,
        ];
        for row in INVARIANTS {
            assert_eq!(
                row.quint_reason.is_some(),
                quint_reasons.contains(&row.id),
                "{:?}: quint_reason",
                row.id
            );
            assert_eq!(
                row.kani_reason.is_some(),
                row.kani.is_empty(),
                "{:?}: kani_reason",
                row.id
            );
        }
    }

    #[test]
    fn every_verus_todo_of_d_9_4_is_tracked() {
        let todo = |inv: Inv| info(inv).map_or("", |r| r.verus_todo);
        // Every state invariant, and every step invariant and step half. Total's state form is
        // its own wording: every emitted event passes evolve's precondition.
        for row in INVARIANTS {
            if row.kinds.contains(&InvKind::State) && row.id != Inv::Total {
                assert!(
                    row.verus_todo
                        .contains("evolve of each event that apply and settle emit"),
                    "{:?}: the state form",
                    row.id
                );
            }
            if row.kinds.contains(&InvKind::Step) {
                assert!(
                    row.verus_todo.contains("satisfies")
                        && row.verus_todo.contains("step predicate"),
                    "{:?}: the step form",
                    row.id
                );
            }
        }
        // The named obligations.
        let named: [(Inv, &str); 9] = [
            (Inv::T2, "lemmas L1 and L2"),
            (Inv::T3, "every reply equals the reply on the settled state"),
            (Inv::O1, "exactly-once op_id"),
            (Inv::F, "flag monotonicity between events"),
            (Inv::R1, "recount ∘ evolve = evolve_index ∘ recount"),
            (Inv::T4, "recount ∘ evolve = evolve_index ∘ recount"),
            (Inv::G1, "every item of guide on a settled state"),
            (Inv::K8, "evolve of each event"),
            (Inv::Total, "a termination measure for every loop"),
        ];
        for (inv, phrase) in named {
            assert!(todo(inv).contains(phrase), "{inv:?}: {phrase}");
        }
    }
}
