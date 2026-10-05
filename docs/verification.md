# Verification

This document records how the Swarm Forum is verified and what the checks have found: what each
level checks (design D.0.3), the model checking, the conformance replay of model traces against the
core, the scale gates, the year simulation, the usability rounds with agents, and the Kani harnesses,
with the limits of each. Two blocks are generated from the code by `scripts/gen-verification-table.py`, and
`scripts/verify.sh` fails when they are stale; the rest is written by hand.

**Status: the first release** (protocol 1.5, 2026-10-04). The final run's numbers are below, each
with the commit it ran at; the release's later commits changed no code that an earlier result
depends on, as each section says.

## How to run the checks

`scripts/verify.sh` runs the checks of design D.12.9 and ends with a summary that names each check
as passed, failed, or skipped, with the reason for every skip. It exits non-zero if any check that
ran failed.

```sh
scripts/verify.sh                        # every check that needs neither the reference hardware nor model tokens
scripts/verify.sh --no-model --no-kani   # the quick form, which CI's verify job runs
scripts/verify.sh --bench                # also the scale gates; BENCH_SCALE=smoke for a laptop
scripts/verify.sh --usability -- run --scenario s0-smoke --model claude:claude-sonnet-5-5
python3 scripts/gen-verification-table.py   # regenerates the two generated blocks below
```

| Step | What runs | Skipped when |
|---|---|---|
| 1 | `cargo fmt --check` | never |
| 2 | `cargo clippy -- -D warnings` on the default build (`--workspace --all-targets`), the usability build (`-p forum-server --features usability`), and the scale gates' build (`--profile bench-gates -p forum-server --features bench`, with `--cfg forum_bench`) | never |
| 3 | The guard rails of D.1.3: `forum-core`'s dependency allow-list, no floating-point type in `forum-core` and `forum-cron`, and a `HashMap` probe that clippy must reject | never |
| 4 | `cargo test --workspace` (unit, scenario, property, cost, conformance, and shell tests); `cargo test -p forum-server --features usability`; the bench-gates build's library and binary tests; the generated blocks of this document are current; the usability harness's own tests (`python3 -m unittest discover -s usability/tests`, after building the usability server) | Python 3.10 or later is missing (the last two) |
| 5 | `cargo kani -p forum-cron` and `cargo kani -p forum-core` | `--no-kani`, or `cargo kani` is missing |
| 6 | In `model/`: `npm run typecheck`, `npm test`, `npm run simulate`, and `npm run traces`; TLC on the `holds` and `keys` slices in both class configurations, and the liveness of `holds` (D.0.3); Apalache on the same modules only with `VERIFY_APALACHE=1` | `--no-model`, Quint is missing (`npm ci` in `model/`), or Java is missing (TLC) |
| 7 | The conformance replay: the committed fixtures in step 4, and the sweep (`traces.sh --sweep`, each group replayed) | `--no-model`, or Quint is missing (the sweep) |
| 8 | The scale gates, once each, through `scripts/bench/run-all.sh` | without `--bench` |
| 9 | The usability test with live agents, `usability/run.sh` (`campaign` unless arguments follow `--`) | without `--usability`; it spends model tokens |

A TLC run that its time limit (`MODEL_CHECK_SECONDS`, 1,800 s by default) stops with no violation
is reported as passed and bounded, with the depth it reached; `MODEL_CHECK_STRICT=1` makes it fail.

**CI** (`.github/workflows/ci.yml`, no secrets). On every push to `main` and every pull request: the
`verify` job runs `scripts/verify.sh --no-model --no-kani`, and the `model` job runs `npm ci`,
`npm run typecheck`, and `npm test` in `model/`, then the conformance tests with the model's Quint on
`PATH`. Nightly and on a manual run, the formal checks of the key invariants: `cargo kani` on both
crates, TLC on `holds_two`, `holds_one`, `keys_two`, and `keys_one`, the liveness of `holds_two` and
`holds_one`, and the conformance sweep. The model's simulation, Apalache, the scale gates, and the
usability test are left to `scripts/verify.sh`.

## What is checked at each level

The verification scope was decided on 2026-10-02 (D.0.3). **Every invariant** of §26 has an
executable check, written from the specification over the naive definitions of `forum_core::spec`
and sharing no code with `evolve` or `apply` (D.9.1): `check_invariants` for a state,
`check_step` and `check_fire` for a step, and `check_guide` for the next calls (G1). The checks run
in every unit, scenario, and property test (the property test checks every step of its generated
runs), in debug builds of the server after every command and every firing, in `forum-server check`
and `serve --check`, and at the end of every history replay; a subset linear in the state
(`check_startup`) runs at every startup, release builds included.

**The key invariants**, those that keep data intact, are also checked formally: H1 to H6 (holds and
fencing), O1 and O2 (retries and versions), K1, K3, K4, K5, and T5 (keys, the tree, merges, and waits
without cycles), T1 to T4 (time), and the totality of `evolve` (`Total`). For them:
- the Quint model checks them in its `holds` and `keys` slices, and the time checks T3 and T4 in
  every slice; TLC runs those slices, and Apalache runs only where it reaches (today it reaches no
  step: below);
- Kani harnesses check them, with the primitives they rest on (tokens, time arithmetic, FNV, and the
  order of deadlines);
- Verus proofs are a later, optional step (D.13.4).

The other invariants get no Quint or Kani work from D.0.3. Where WP-A3 had already written a model
check for one before the decision, the check stays in its slice and runs with it.

<!-- registry:begin -->
<!-- Generated by scripts/gen-verification-table.py; do not edit by hand. Run `python3 scripts/gen-verification-table.py` after the registry, the model, or a Kani harness changes. -->

Every executable check runs after every call and firing in every test (`World`), in the property test, in the §32 scenario, in debug builds of the server, and in `forum-server check` (D.9.1). The tests column names the files whose tests exercise the invariant by name (under `crates/forum-core` unless a crate is named): the mutations its check must reject, and the flows that reach it.

**The key invariants (D.0.3).** Each has its executable check and tests, the model's checks in the slices named, TLC on the modules of D.0.3's slices (`npm run tlc:<module>`), Kani harnesses, and a Verus TODO. A name marked *not in the model* or *not written* is one the registry names that does not exist.

| Invariant | Kind | Executable check | Tests | Quint | Slices | TLC modules | Kani | Verus |
|---|---|---|---|---|---|---|---|---|
| H1 | state | `check_invariants` | `src/evolve_tests.rs` | `inv_H1` | holds | `holds_one`, `holds_two` | `hold_cmds_preserve_h` | V-H1, todo |
| H2 | state and step | `check_invariants`, `check_step`, `check_fire` | `src/evolve_tests.rs`, `src/evolve_check_tests.rs`, `src/invariants/step_tests.rs` | `inv_H2`, `step_H2` | holds | `holds_one`, `holds_two` | `hold_cmds_preserve_h`, `session_retire`, `silence_after_leases` | V-H2, todo |
| H3 | state and step | `check_invariants`, `check_step`, `check_fire` | `src/evolve_tests.rs`, `src/invariants/step_tests.rs` | `inv_H3`, `step_H3` | holds | `holds_one`, `holds_two` | `hold_cmds_preserve_h` | V-H3, todo |
| H4 | step | `check_step`, `check_fire` | `src/invariants/step_tests.rs` | `step_H4` | holds | `holds_one`, `holds_two` | `stale_token_only_late` | V-H4, todo |
| H5 | state and step | `check_invariants`, `check_step`, `check_fire` | `src/evolve_tests.rs`, `src/evolve_check_tests.rs`, `src/invariants/step_tests.rs` | `inv_H5` | holds | `holds_one`, `holds_two` | `lease_issue_h5`, `lease_extend_h5`, `renew_limit` | V-H5, todo |
| H6 | state | `check_invariants` | `src/evolve_tests.rs` | `inv_H6` | basis, holds, people | `holds_one`, `holds_two` | `close_ends_holds`, `take_gate_closed_nodes` | V-H6, todo |
| O1 | state and step | `check_invariants`, `check_step`, `check_fire` | `src/evolve_tests.rs`, `src/invariants/step_tests.rs` | `step_O1`, `inv_O1` | holds | `holds_one`, `holds_two` | `op_retry_no_effect`, `op_expiry` | V-O1, todo |
| O2 | state and step | `check_invariants`, `check_step`, `check_fire` | `src/evolve_tests.rs`, `src/evolve_check_tests.rs`, `src/invariants/step_tests.rs` | `inv_O2`, `step_O2` | holds, keys | `holds_one`, `holds_two`, `keys_one`, `keys_two` | `version_bumps_o2` | V-O2, todo |
| K1 | state | `check_invariants` | `src/evolve_tests.rs` | `inv_K1` | keys | `keys_one`, `keys_two` | `keys_open_unique` | V-K1, todo |
| K3 | state | `check_invariants` | `src/evolve_tests.rs`, `src/evolve_check_tests.rs` | `inv_K3` | keys | `keys_one`, `keys_two` | `move_keeps_tree` | V-K3, todo |
| K4 | state | `check_invariants` | `src/evolve_tests.rs` | `inv_K4` | keys | `keys_one`, `keys_two` | `move_keeps_tree` | V-K4, todo |
| K5 | state | `check_invariants` | `src/evolve_tests.rs`, `src/evolve_check_tests.rs` | `inv_K5` | keys | `keys_one`, `keys_two` | `merge_redirects` | V-K5, todo |
| T1 | state and step | `check_invariants`, `check_step`, `check_fire` | `src/evolve_tests.rs`, `src/invariants/step_tests.rs` | `inv_T1`, `step_T1` | holds | `holds_one`, `holds_two` | `event_stamps_t1` | V-T1, todo |
| T2 | property | the T2 item of the property test, at random points (D.12.3) | `tests/prop.rs` | `inv_T2`, `test_T2` | holds, people, recurring, user, `tests/forum_test.qnt` | `holds_one`, `holds_two` | `settle_path_independent_small` | V-T2, todo |
| T3 | state | `check_invariants` | `src/evolve_tests.rs`, `src/evolve_check_tests.rs` | `inv_T3` | basis, holds, keys, people, recurring, user | `holds_one`, `holds_two`, `keys_one`, `keys_two` | `settle_idempotent_small` | V-T3, todo |
| T4 | state and step | `check_invariants`, `check_step`, `check_fire` | `src/evolve_tests.rs`, `src/evolve_check_tests.rs`, `src/invariants/step_tests.rs`, `src/cmd/recurring_tests.rs`, `src/cmd/talk_people_questions_tests.rs` | `inv_T4`, `step_T4` | basis, holds, keys, people, recurring, user | `holds_one`, `holds_two`, `keys_one`, `keys_two` | `deadline_order`, `fire_once_small` | V-T4, todo |
| T5 | state | `check_invariants` | `src/evolve_tests.rs` | `inv_T5` | keys | `keys_one`, `keys_two` | `reaches_terminates`, `merge_redirects` | V-T5, todo |
| Total | state | `check_invariants` | `src/evolve_tests.rs` | none: *the model applies transitions directly and has no evolve preconditions* | none | none | `evolve_counts_anomalies` | V-Total, todo |

**The other invariants (D.0.3).** Each is checked by its executable check and its tests (the registry's level *tests*); D.0.3 asks no Quint or Kani work of them. The last column names the Quint checks and Kani harnesses that exist for them all the same, and the reasons where none does.

| Invariant | Kind | Executable check | Tests | Quint and Kani (not required) |
|---|---|---|---|---|
| O3 | step | `check_step`, `check_fire` | `src/invariants/step_tests.rs` | Quint `step_O3`; no Kani: *structural: the core has no hidden status to read, and the prefix property is the shell's append-only history; the step check runs in every test* |
| R1 | state | `check_invariants` | `src/evolve_tests.rs`, `src/evolve_check_tests.rs` | no Quint: *the model has no indices*; Kani `evolve_no_panic_sessions`, `evolve_no_panic_nodes`, `evolve_no_panic_holds`, `evolve_no_panic_entities` |
| K2 | state | `check_invariants` | `src/evolve_tests.rs`, `src/evolve_check_tests.rs`, `forum-keys/tests/golden.rs` | no Quint: *keys are model atoms (D.10.6)*; no Kani: *a shell property: the proptests of forum-keys* |
| K6 | state | `check_invariants` | `src/evolve_tests.rs` | Quint `inv_K6`; no Kani: *D.0.3: not a key invariant: its executable check and tests* |
| K7 | step | `check_step`, `check_fire` | `src/cmd/restructure_tests.rs` | Quint `step_K7`; no Kani: *D.0.3: not a key invariant: its executable check and tests* |
| K8 | state | `check_invariants` | `src/evolve_tests.rs`, `src/evolve_check_tests.rs` | Quint `inv_K8`; no Kani: *D.0.3: not a key invariant: its executable check and tests* |
| C1 | step | `check_step`, `check_fire` | `src/cmd/restructure_tests.rs` | Quint `step_C1`; no Kani: *D.0.3: not a key invariant: its executable check and tests* |
| Q1 | state | `check_invariants` | `src/evolve_tests.rs` | Quint `inv_Q1`; no Kani: *D.0.3: not a key invariant: its executable check and tests* |
| S1 | state and step | `check_invariants`, `check_step`, `check_fire` | `src/evolve_tests.rs`, `src/evolve_check_tests.rs`, `src/invariants/step_tests.rs`, `src/cmd/tidy_tests.rs` | Quint `inv_S1`; no Quint for the rest: *1.11's step clause, a duplicate signal resolved with its pair's second close: its executable check and tests (D.0.3); the model mirrors the close (`resolve_closed_duplicates`), which the conformance replay compares*; no Kani: *D.0.3: not a key invariant: its executable check and tests* |
| N1 | state and step | `check_invariants`, `check_step`; `check_fire` has no clause of it, since no firing logs or routes an entry | `src/evolve_tests.rs`, `src/invariants/step_tests.rs`, `src/cmd/talk_user_tests.rs` | Quint `inv_N1`; no Quint for the rest: *1.11's step clause, a follow-up routed as a clarification by default: its executable check and tests (D.0.3); the model mirrors the routing (`from_user_ex`), which the conformance replay compares*; no Kani: *D.0.3: not a key invariant: its executable check and tests* |
| Settled | state and step | `check_invariants`, `check_step`, `check_fire` | `src/evolve_tests.rs`, `src/evolve_check_tests.rs`, `src/invariants/step_tests.rs`, `src/cmd/restructure_tests.rs`, `src/cmd/finish_tests.rs` | Quint `inv_Settled`, `step_Settled`; no Quint for the rest: *1.12's step clause, a release's decision (`release_decision`): its executable check and tests (D.0.3); the model mirrors the release (`release_ex`, `record_decision`), which the conformance replay compares*; no Kani: *D.0.3: not a key invariant: its executable check and tests* |
| F | state | `check_invariants` | `src/evolve_tests.rs`, `src/evolve_check_tests.rs` | Quint `inv_F`; no Kani: *D.0.3: not a key invariant: its executable check and tests* |
| N5 | step | `check_step`, `check_fire` | `src/cmd/restructure_tests.rs` | Quint `step_N5`; Kani `merge_redirects` |
| E1 | state and step | `check_invariants`, `check_step`, `check_fire` | `src/evolve_tests.rs`, `src/evolve_check_tests.rs`, `src/cmd/cross_tests.rs`, `src/invariants/step_tests.rs`, `src/cmd/finish_tests.rs` | Quint `inv_E1`; no Quint for the rest: *1.12's step clause, a question withdrawn only by its retract: its executable check and tests (D.0.3); the model mirrors the retract (`retract_ex`), which the conformance replay compares*; no Kani: *D.0.3: not a key invariant: its executable check and tests* |
| W1 | state | `check_invariants` | `src/evolve_tests.rs`, `src/evolve_check_tests.rs` | Quint `inv_W1`; no Kani: *D.0.3: not a key invariant: its executable check and tests* |
| R2 | state and step | `check_invariants`, `check_step`, `check_fire` | `src/evolve_tests.rs`, `src/invariants/step_tests.rs`, `src/cmd/recurring_tests.rs` | Quint `inv_R2`, `step_R2`; no Kani: *D.0.3: not a key invariant: its executable check and tests* |
| D1 | state and step | `check_invariants`, `check_step`, `check_fire` | `src/evolve_tests.rs`, `src/evolve_check_tests.rs`, `src/invariants/step_tests.rs` | Quint `inv_D1`, `step_D1`; no Kani: *needs firing chains across questions, defaults, and digest entries, beyond Kani's bounds on BTreeMap states: the proptests and Quint's user and people slices* |
| M1 | state | `check_invariants` | `src/evolve_tests.rs`, `src/evolve_check_tests.rs`, `src/cmd/cross_tests.rs` | Quint `inv_M1`; no Kani: *quantifies over every entity kind of a whole state: the proptests and every Quint slice* |
| U1 | state and step | `check_invariants`, `check_step`, `check_fire` | `src/evolve_tests.rs`, `src/evolve_check_tests.rs`, `src/invariants/step_tests.rs` | Quint `inv_U1`, `step_U1`, `hello_exact`; no Kani: *D.0.3: not a key invariant: its executable check and tests* |
| L1 | state and step | `check_invariants`, `check_step`, `check_fire` | `src/evolve_tests.rs`, `src/invariants/step_tests.rs`, `src/cmd/restructure_tests.rs` | Quint `inv_L1`, `step_L1`; no Kani: *D.0.3: not a key invariant: its executable check and tests* |
| P1 | state | `check_invariants` | `src/evolve_tests.rs` | Quint `inv_P1`; no Kani: *D.0.3: not a key invariant: its executable check and tests* |
| P2 | step | `check_step`, `check_fire` | `src/invariants/step_tests.rs` | Quint `step_P2`; Kani `hold_cmds_preserve_h` |
| AR | property | a unit test of `query::list` (D.9.2) | `src/query/tests.rs` | Quint `archived`; no Kani: *a definition with no state to check: the unit test in the Rust check column* |
| G1 | property | `check_guide`, after every call in every test (D.9.1) | `src/query/guide_tests.rs`, `tests/prop.rs`, `tests/scenario_32.rs` | no Quint: *D.0.3 dropped WP-A5, the next calls in the model: G1 is checked in Rust by `check_guide`, and reads, `session hello`, texts, and rendering have no state effect to model*; no Kani: *D.0.3: not a key invariant: its executable check and tests* |

**Verus TODOs** (D.9.4, D.13.4: after the first release, an optional step for the key invariants, D.0.3). Every row starts at *todo*.

- **V-H1** (todo): evolve of each event that apply and settle emit preserves H1
- **V-H2** (todo): evolve of each event that apply and settle emit preserves H2; every (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, satisfies H2's step predicate
- **V-H3** (todo): evolve of each event that apply and settle emit preserves H3; every (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, satisfies H3's step predicate
- **V-H4** (todo): every (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, satisfies H4's step predicate
- **V-H5** (todo): evolve of each event that apply and settle emit preserves H5; every (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, satisfies H5's step predicate
- **V-H6** (todo): evolve of each event that apply and settle emit preserves H6
- **V-O1** (todo): exactly-once op_id; evolve of each event that apply and settle emit preserves O1; every (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, satisfies O1's step predicate
- **V-O2** (todo): evolve of each event that apply and settle emit preserves O2; every (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, satisfies O2's step predicate
- **V-K1** (todo): evolve of each event that apply and settle emit preserves K1
- **V-K3** (todo): evolve of each event that apply and settle emit preserves K3
- **V-K4** (todo): evolve of each event that apply and settle emit preserves K4
- **V-K5** (todo): evolve of each event that apply and settle emit preserves K5
- **V-T1** (todo): evolve of each event that apply and settle emit preserves T1; every (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, satisfies T1's step predicate
- **V-T2** (todo): settle idempotence and path independence, by induction on the pops through the lemmas L1 and L2 of D.8.2
- **V-T3** (todo): every reply equals the reply on the settled state; evolve of each event that apply and settle emit preserves T3
- **V-T4** (todo): recount ∘ evolve = evolve_index ∘ recount for the deadline queue; evolve of each event that apply and settle emit preserves T4; every (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, satisfies T4's step predicate
- **V-T5** (todo): evolve of each event that apply and settle emit preserves T5
- **V-Total** (todo): every event that apply and settle emit passes evolve's precondition, so anomalies stays 0; and a termination measure for every loop of the core (D.9.4)

The other rows keep their TODOs in the registry, which D.0.3 no longer requires:

- **V-O3** (todo): every (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, satisfies O3's step predicate
- **V-R1** (todo): recount ∘ evolve = evolve_index ∘ recount; evolve of each event that apply and settle emit preserves R1
- **V-K2** (todo): evolve of each event that apply and settle emit preserves K2's core part (version 0 keys and origins are canonical, no repeat: key of another version)
- **V-K6** (todo): evolve of each event that apply and settle emit preserves K6
- **V-K7** (todo): every (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, satisfies K7's step predicate
- **V-K8** (todo): evolve of each event that apply and settle emit preserves K8
- **V-C1** (todo): every (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, satisfies C1's step predicate
- **V-Q1** (todo): evolve of each event that apply and settle emit preserves Q1
- **V-S1** (todo): evolve of each event that apply and settle emit preserves S1; every (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, satisfies S1's step predicate
- **V-N1** (todo): evolve of each event that apply and settle emit preserves N1; every (s, ctx, cmd) with decide's events satisfies N1's step predicate
- **V-Settled** (todo): evolve of each event that apply and settle emit preserves Settled; every (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, satisfies Settled's step predicate
- **V-F** (todo): flag monotonicity between events (the lemma behind F2); evolve of each event that apply and settle emit preserves F
- **V-N5** (todo): every (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, satisfies N5's step predicate
- **V-E1** (todo): evolve of each event that apply and settle emit preserves E1; every (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, satisfies E1's step predicate
- **V-W1** (todo): evolve of each event that apply and settle emit preserves W1
- **V-R2** (todo): evolve of each event that apply and settle emit preserves R2; every (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, satisfies R2's step predicate
- **V-D1** (todo): evolve of each event that apply and settle emit preserves D1; every (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, satisfies D1's step predicate
- **V-M1** (todo): evolve of each event that apply and settle emit preserves M1a
- **V-U1** (todo): evolve of each event that apply and settle emit preserves U1; every (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, satisfies U1's step predicate
- **V-L1** (todo): evolve of each event that apply and settle emit preserves L1's depth half; every (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, satisfies L1's step predicate (children and texts)
- **V-P1** (todo): evolve of each event that apply and settle emit preserves P1
- **V-P2** (todo): every (s, ctx, cmd) with decide's events, and every (s, key) with fire's events, satisfies P2's step predicate
- **V-AR** (todo): archived is a function of (S, now) that no state field stores
- **V-G1** (todo): every item of guide on a settled state satisfies validate's precondition, and a Close item's plan raises none of the four warnings
<!-- registry:end -->

Notes on the tables:
- The registry (`crates/forum-core/src/invariants/registry.rs`) records each row's level of D.0.3
  (WP-D9): the key invariants keep their Quint checks and Kani harnesses, `silence_after_leases`
  among H2's; every other invariant records its executable check and the tests that exercise it,
  its level *tests*, and names a Quint check or a Kani harness only where one exists. D.9.2's
  planned harnesses for the other invariants, which no package built (WP-D2 to WP-D4 left them out,
  citing D.0.3), and G1's `inv_G1` and `inv_G1_close`, which went with WP-A5 when D.0.3 dropped it,
  are gone from it. `crates/forum-core/tests/registry.rs` fails on a Quint name, a Kani harness, or
  a test file that does not exist, and on a key invariant that no key slice checks.
- `Total` has no model check: the model applies its transitions directly and has no `evolve`. Its
  harness, and the four `evolve_no_panic_*` harnesses that the registry files under R1, check the
  arithmetic and the preconditions of `evolve` (below, "Kani harnesses"); `evolve::random_tests`
  applies every event variant end to end.
- D.10.7 also lists O2 among the `recurring` slice's invariants (1.4's copy of a series at its slot);
  that slice checks no O2 today, so the table shows `holds` and `keys`.

## Model checking

The model is `model/` (Quint 0.33.0, design D.10); its README describes the files, the slices, and
the route by which Apalache and TLC read a model of this size. Every model-checking result below is
bounded: TLC and Apalache explore only the states within the guards of D.10.7, and the runs so far
were stopped by a time limit before they finished even that. Each run below was made on a 12-core
laptop with 36 GiB of memory.

**Run tests.** `npm test`: 179 runs passing at commit 647b461 (2026-10-03), and again at the
release's model (ce6744a, 2026-10-04): `fnv_ts_test` 2, `forum_test` 83 (`scenario_32` and `test_T2`
among them), `helpers_test` 78, `invariants_test` 16. `npm run typecheck` passes on every file.
At 1.8's corrections, intents, and tidying (group C): 183 runs, `fnv_ts_test` 2, `forum_test` 86
(`upheldTest`, `checkIntentTest`, and `staleTest` added), `helpers_test` 79 (`staleKeysTest`),
`invariants_test` 16. At 1.9's words and records (group A): the same 183 runs, `fromUserTest` with
`WhyNoPrincipal`, and the tests that meant `user` passing `principal: user`. At 1.9's tidying and
messages (group C): 184 runs, `forum_test` 87 (`withheldTest` added, `sayWaitTest` with a note to a
holder), the others as before; `npm run typecheck` passes on every file. At the merge of 1.9's
groups A, B, and C: the same 184 runs pass, and so does `npm run typecheck`. At 1.10's guidance
(group B): 185 runs, `forum_test` 88 (`reviewOrderTest` added: a review item comes before an open
flagged node of its tier, whatever their ids and ring positions, and once its objection is resolved
the open node comes first again), the others as before; `npm run typecheck` passes on every file. At
1.10's semantics (group C): 186 runs, `fnv_ts_test` 2, `forum_test` 89 (`disputeTest` and
`checkpointWaitTest` added, 1.9's `withheldTest` now `heldSaidTest`, `sayWaitTest` with a note to
the next holder, `upheldTest` with the closure's `from_done`), `helpers_test` 79, `invariants_test`
16; `npm run typecheck` passes on every file. At the merge of 1.10's groups A, B, and C: 187 runs,
`forum_test` 90, the others as before; `npm run typecheck` passes on every file. At 1.11's reviews
and conversations (group C): 190 runs, `fnv_ts_test` 2, `forum_test` 93 (`deferTest`,
`saidAloneTest`, and `closerNoteTest` added; `closeSettlesTest`, `mergeTest`, `mergeCreditTest`,
`heldSaidTest`, `saidTest`, and the fixture `T2_QUESTIONS` adapted, since a `said` by a session that
holds nothing is now confirmed as it is recorded: where a test meant a pending one, its session
holds a node first, and `recorded_pending` of the §32 run allows the confirmed one),
`helpers_test` 79, `invariants_test` 16; `npm run typecheck` passes on every file. At 1.11's
guidance and routing (group B): 192 runs, `forum_test` 95 (`needsOrderTest`, `requestOrderTest`,
`seriesBeforeSlotTest`, `duplicateBothClosedTest`, and `followUpTest` added, and `reviewOrderTest`
with a review in tier 2, before a plain offer and after an open node's objection), the others as
before; `npm run typecheck` passes on every file. At the merge of 1.11's groups A, B, and C: 195
runs, `fnv_ts_test` 2, `forum_test` 98 (group C's 3 and group B's 5 added), `helpers_test` 79,
`invariants_test` 16; `npm run typecheck` passes on every file. At 1.12's guidance (group B): 196
runs, `forum_test` 99 (`handoffTest` added; `reviewOrderTest` and `deferTest` with the objection's
review never given to the session that raised it), the others as before; `npm run typecheck` passes
on every file. At 1.12's finishing, notes, and questions (group C): 199 runs, `fnv_ts_test` 2,
`forum_test` 102 (`releaseSettlesTest`, `withdrawTest`, `holderClosedTest`, and `reopenOfferTest`
added; the 30 releases of the others carry `settles: Set()`), `helpers_test` 79, `invariants_test`
16; `npm run typecheck` passes on every file, `conformance.qnt` included. At the merge of 1.12's
groups A, B, and C: 200 runs, `fnv_ts_test` 2, `forum_test` 103 (group B's 1 and group C's 4 added;
`handoffTest`'s releases carry `settles: Set()`), `helpers_test` 79, `invariants_test` 16;
`npm run typecheck` passes on every file, `conformance.qnt` included. After the merge, 201 runs,
`forum_test` 104 (`handoffSettlesTest` added: a `handoff` release with `settles` records its
decision and keeps its node from its releaser), the others as before.

**Simulation** (`scripts/simulate.sh`, `inv_all`, 40 steps a run, every witness counted):

| Commit | Instance | Runs | Result |
|---|---|---|---|
| d761685 | `one_class`, `no_user` | 20,000 each | no violation |
| d761685 | `two_class` | about 19,216 | a violation of `step_N5` (seed 0x34e51994d0f1ba43): a merge of a series into its own open occurrence lost the series' marks; 1.5 refuses that merge (§8, D.14.7) |
| 647b461 | `two_class`, `one_class`, `no_user` | 2,000 each | no violation; every required witness reached; the seed above passes |
| 8f8098b | `two_class` | 20,000 | no violation (seed 0xa09e112a8409a669); every required witness reached; 6,814 s on 4 threads |
| ce6744a (the release's model) | `two_class`, `one_class`, `no_user` | 2,000 each | no violation (seeds 0x8eaa9155d044f522, 0x91f40a0abc616a03, 0x90f404ace0045a0); every required witness reached; 710 s on 6 threads |
| 1.8, group C | `two_class` | 300 | no violation (seed 0x1c470f); every required witness reached; `w_stale_offer` in 172 runs, `w_upheld` and `w_ok_check_intent` in none (both rare at 40 steps, and not required; the run tests reach them); 832 s on 3 threads |

The model changed once between those two rows: ce6744a caps a user question's first stage at
`capacity.window` in `due_for` (1.5, O-1). The 20,000-run simulation of every instance planned for
the release was cut to 2,000 runs each at the release's model, for time.

**TLC** (`npm run tlc:<module>`, 4 workers), on the slices of the key invariants:

| Date | Model at | Module | Time limit | Depth reached | Distinct states | Generated | Result |
|---|---|---|---|---|---|---|---|
| 2026-10-02 | WP-A3 | a key slice (the record names none) | | 5 | about 85,000 | | no violation; stopped by the limit |
| 2026-10-03 | cc1681c | `holds_two` | 40 min | 5 | 173,743 | 416,637 | no violation; stopped, 172,862 states left on the queue |
| 2026-10-03 | cc1681c | `keys_two` | 40 min | 6 | 1,984,371 | 7,956,079 | no violation; stopped, 1,964,985 left |
| 2026-10-03 | 52cf305 (1.4's Silence key) | `holds_two` | 25 min | 5 | 132,096 | 288,410 | no violation; stopped, 131,496 left |
| 2026-10-03 | d761685 (the post-settle) | `holds_two` | 20 min | 5 | 97,318 | 211,836 | no violation; stopped, 96,877 left |
| 2026-10-03 | d761685 | `keys_two` | 20 min | 6 | 624,835 | 2,521,687 | no violation; stopped, 618,432 left |
| 2026-10-04 | 8f8098b | `holds_two` | 20 min | 7 | 86,742 | 267,167 | no violation; stopped, 86,260 left |
| 2026-10-04 | 8f8098b | `holds_one` | 20 min | 8 | 131,003 | 426,103 | no violation; stopped, 129,594 left |
| 2026-10-04 | 8f8098b | `keys_two` | 20 min | 6 | 654,094 | 2,690,564 | no violation; stopped, 647,376 left |
| 2026-10-04 | 8f8098b | `keys_one` | 20 min | 7 | 951,195 | 2,448,701 | no violation; stopped, 939,374 left |
| 2026-10-04 | 1.8, group C | `holds_two` | 20 min | 6 | 85,735 | 234,650 | no violation; stopped, 85,300 left |
| 2026-10-04 | 1.8, group C | `keys_two` | 20 min | 7 | 686,887 | 2,788,676 | no violation; stopped, 680,032 left |
| 2026-10-04 | 1.8, group C | `holds_one` | 20 min | 6 | 124,900 | 441,495 | no violation; stopped, 123,540 left |
| 2026-10-04 | 1.8, group C | `basis_two` | 20 min | 7 | 1,684,966 | 3,397,087 | no violation; stopped, 1,665,403 left |
| 2026-10-04 | 1.8, group C | `keys_one` | 20 min | 7 | 1,091,713 | 2,887,963 | no violation; stopped, 1,077,419 left |
| 2026-10-04 | 1.8, group C | `people_two` | 20 min | 7 | 125,772 | 416,749 | no violation; stopped, 124,317 left |
| 2026-10-04 | 1.9, group B | `holds_two` | 20 min | 6 | 29,107 | 67,827 | no violation; stopped, 28,986 left |
| 2026-10-04 | 1.9, group B | `people_two` | 20 min | 6 | 42,930 | 142,194 | no violation; stopped, 42,438 left |
| 2026-10-04 | 1.9, group B | `holds_one` | 20 min | 5 | 43,133 | 109,489 | no violation; stopped, 42,784 left |
| 2026-10-04 | 1.9, group B | `basis_two` | 20 min | 6 | 513,518 | 1,165,860 | no violation; stopped, 505,390 left |
| 2026-10-04 | 1.9, group A | `user_two` | 20 min | 6 | 31,249 | 35,751 | no violation; stopped, 31,119 left |
| 2026-10-04 | 1.9, group A | `user_one` | 20 min | 7 | 37,984 | 63,713 | no violation; stopped, 37,589 left |
| 2026-10-04 | 1.9, group C | `people_two` | 20 min | 7 | 69,113 | 229,911 | no violation; stopped, 68,307 left |
| 2026-10-04 | 1.9, group C | `keys_two` | 20 min | 6 | 414,545 | 1,658,871 | no violation; stopped, 410,141 left |
| 2026-10-04 | 1.9, group C | `people_one` | 20 min | 9 | 93,841 | 282,849 | no violation; stopped, 91,848 left |
| 2026-10-04 | 1.9, group C | `keys_one` | 20 min | 8 | 655,827 | 1,637,559 | no violation; stopped, 647,952 left |
| 2026-10-05 | 1.10, group B | `holds_two` | 20 min | 9 | 50,944 | 141,162 | no violation; stopped, 50,718 left |
| 2026-10-05 | 1.10, group B | `people_two` | 20 min | 8 | 76,718 | 288,704 | no violation; stopped, 75,699 left |
| 2026-10-05 | 1.10, group B | `holds_one` | 20 min | 6 | 86,890 | 266,097 | no violation; stopped, 86,089 left |
| 2026-10-05 | 1.10, group B | `basis_two` | 20 min | 7 | 1,087,707 | 2,401,922 | no violation; stopped, 1,072,251 left |
| 2026-10-05 | 1.10, group C | `people_two` | 20 min | 6 | 60,591 | 166,559 | no violation; stopped, 60,028 left |
| 2026-10-05 | 1.10, group C | `people_one` | 20 min | 7 | 61,590 | 165,284 | no violation; stopped, 60,465 left |
| 2026-10-05 | 1.10, group C | `basis_two` | 20 min | 6 | 443,314 | 1,004,266 | no violation; stopped, 436,224 left |
| 2026-10-05 | 1.10, group C | `keys_two` | 20 min | 6 | 532,363 | 2,086,662 | no violation; stopped, 527,172 left |
| 2026-10-05 | 1.10, group C | `keys_one` | 20 min | 6 | 746,626 | 1,893,905 | no violation; stopped, 737,512 left |
| 2026-10-05 | 1.10, groups A, B, and C merged | `people_two` | 10 min | 6 | 57,843 | 228,475 | no violation; stopped, 57,060 left |
| 2026-10-05 | 1.10, groups A, B, and C merged | `basis_two` | 10 min | 7 | 788,338 | 1,771,607 | no violation; stopped, 776,289 left |
| 2026-10-05 | 1.11, group C | `people_two` | 20 min | 6 | 93,339 | 298,494 | no violation; stopped, 92,329 left |
| 2026-10-05 | 1.11, group C | `people_one` | 20 min | 9 | 117,707 | 340,843 | no violation; stopped, 115,383 left |
| 2026-10-05 | 1.11, group C | `keys_two` | 20 min | 7 | 688,016 | 2,502,045 | no violation; stopped, 681,906 left |
| 2026-10-05 | 1.11, group C | `basis_two` | 20 min | 7 | 1,074,835 | 2,221,804 | no violation; stopped, 1,059,604 left |
| 2026-10-05 | 1.11, group C | `holds_two` | 20 min | 8 | 56,546 | 163,868 | no violation; stopped, 56,287 left |
| 2026-10-05 | 1.11, group C | `keys_one` | 20 min | 7 | 1,019,941 | 2,609,999 | no violation; stopped, 1,007,497 left |
| 2026-10-05 | 1.11, group C | `holds_one` | 20 min | 7 | 122,608 | 420,614 | no violation; stopped, 121,338 left |
| 2026-10-05 | 1.11, group B | `user_two` | 20 min | 4 | 27,063 | 31,305 | no violation; stopped, 26,936 left |
| 2026-10-05 | 1.11, group B | `keys_two` | 20 min | 6 | 209,543 | 802,483 | no violation; stopped, 207,297 left |
| 2026-10-05 | 1.11, group B | `recurring_two` | 20 min | 6 | 21,606 | 80,880 | no violation; stopped, 21,014 left |
| 2026-10-05 | 1.11, group B | `holds_two` | 20 min | 5 | 30,317 | 63,608 | no violation; stopped, 30,194 left |
| 2026-10-05 | 1.11, group B | `people_two` | 20 min | 5 | 40,519 | 126,589 | no violation; stopped, 40,090 left |
| 2026-10-05 | 1.11, group B | `basis_two` | 20 min | 6 | 415,890 | 925,000 | no violation; stopped, 409,374 left |
| 2026-10-05 | 1.11, group B | `user_one` | 20 min | 5 | 37,058 | 60,691 | no violation; stopped, 36,655 left |
| 2026-10-05 | 1.11, group B | `keys_one` | 20 min | 6 | 265,043 | 650,550 | no violation; stopped, 261,857 left |
| 2026-10-05 | 1.11, group B | `holds_one` | 20 min | 4 | 30,758 | 74,883 | no violation; stopped, 30,499 left |
| 2026-10-05 | 1.12, group B | `holds_two` | 20 min | 5 | 46,194 | 95,088 | no violation; stopped, 46,013 left |
| 2026-10-05 | 1.12, group B | `holds_one` | 20 min | 5 | 59,800 | 150,294 | no violation; stopped, 59,317 left |
| 2026-10-05 | 1.12, group B | `people_two` | 20 min | 5 | 42,098 | 152,878 | no violation; stopped, 41,580 left |
| 2026-10-05 | 1.12, group B | `people_one` | 20 min | 6 | 56,871 | 157,079 | no violation; stopped, 55,835 left |
| 2026-10-05 | 1.12, group B | `recurring_two` | 20 min | 6 | 29,557 | 83,863 | no violation; stopped, 28,940 left |
| 2026-10-05 | 1.12, group B | `recurring_one` | 20 min | 6 | 35,948 | 94,080 | no violation; stopped, 34,615 left |
| 2026-10-05 | 1.12, group C | `people_one` | 20 min | 10 | 97,081 | 299,092 | no violation; stopped, 95,070 left |
| 2026-10-05 | 1.12, group C | `people_two` | 20 min | 7 | 77,770 | 249,488 | no violation; stopped, 76,935 left |
| 2026-10-05 | 1.12, group C | `holds_two` | 20 min | 8 | 49,894 | 147,674 | no violation; stopped, 49,648 left |
| 2026-10-05 | 1.12, group C | `keys_two` | 20 min | 7 | 517,956 | 1,907,103 | no violation; stopped, 513,127 left |
| 2026-10-05 | 1.12, group C | `holds_one` | 20 min | 7 | 79,686 | 224,268 | no violation; stopped, 79,019 left |
| 2026-10-05 | 1.12, group C | `user_two` | 20 min | 8 | 96,319 | 129,771 | no violation; stopped, 95,914 left |
| 2026-10-05 | 1.12, group C | `user_one` | 20 min | 7 | 127,645 | 251,437 | no violation; stopped, 126,319 left |
| 2026-10-05 | 1.12, group C | `keys_one` | 20 min | 7 | 1,927,751 | 5,240,029 | no violation; stopped, 1,902,528 left |
| 2026-10-05 | 1.12, final | `holds_two` | 20 min | 6 | 146,013 | 382,836 | no violation; stopped, 145,330 left |
| 2026-10-05 | 1.12, final | `holds_one` | 20 min | 7 | 190,465 | 730,219 | no violation; stopped, 188,311 left |
| 2026-10-05 | 1.12, final | `keys_two` | 20 min | 5 | 747,992 | 2,640,480 | no violation; stopped, 741,271 left |
| 2026-10-05 | 1.12, final | `keys_one` | 20 min | 6 | 1,014,396 | 2,640,088 | no violation; stopped, 1,001,709 left |
| 2026-10-05 | 1.12, final | `basis_two` | 20 min | 6 | 1,607,896 | 2,976,526 | no violation; stopped, 1,590,059 left |
| 2026-10-05 | 1.12, final | `basis_one` | 20 min | 7 | 1,845,074 | 3,075,580 | no violation; stopped, 1,814,268 left |
| 2026-10-05 | 1.12, final | `people_two` | 10 min | 5 | 58,614 | 196,554 | no violation; stopped, 57,959 left |
| 2026-10-05 | 1.12, final | `people_one` | 10 min | 7 | 72,330 | 210,897 | no violation; stopped, 70,947 left |
| 2026-10-05 | 1.12, final | `user_two` | 10 min | 5 | 48,524 | 57,544 | no violation; stopped, 48,311 left |
| 2026-10-05 | 1.12, final | `user_one` | 10 min | 5 | 59,620 | 99,373 | no violation; stopped, 59,020 left |
| 2026-10-05 | 1.12, final | `recurring_two` | 10 min | 6 | 46,629 | 144,183 | no violation; stopped, 45,561 left |
| 2026-10-05 | 1.12, final | `recurring_one` | 10 min | 7 | 57,753 | 173,347 | no violation; stopped, 55,275 left |

The release's runs were made two at a time beside a simulation, with a limit of 20 minutes each
(`MODEL_CHECK_SECONDS=1200`). They carry over to the release's model: its one later change, O-1's cap
in `due_for`, cannot bind in the `holds` and `keys` slices, which have no `from_user`, so the p90 gap
there is always 0 and the due is `user_question.due`, below `capacity.window`.

1.8's group C ran the four slices of the release and the two whose actions it changed besides
(`basis_two` and `people_two`, which resolve objections with and without `upheld`), two at a time
beside Kani or a simulation, and the liveness of the `holds` slices (20 minutes each, two at a time):
`holds_two` no violation, stopped at depth 5 (15,593 distinct states, 36,568 generated, 15,532
left); `holds_one` no violation, stopped at depth 6 (26,567 distinct, 79,488 generated, 26,326
left).

1.9's group B changed the take gate and `next` for a closed node with an open objection (a review
item, kind 1, with no hold), and added H6 to the `people` and `basis` slices, the two where such
nodes arise; it ran the `holds` slices, whose takes and `next` it changed, and those two, two at a
time beside two other groups' model checks on the same machine, so each reached less depth than
1.8's runs.

1.9's group A changed one action of the model, `from_user`'s refusal of a session that names no
principal, which only the `user` slices take, and ran them, beside the other groups' runs. Until
then TLC could not parse the `user` slices: Quint printed `u_routes`'s product of the entries and the
targets as `DOMAIN f \X s`, a precedence conflict in TLA+; the slice now builds the same set as a
union. The 20 minutes include the translation to TLA+, so TLC itself ran about 11 of them.

1.9's group C ran the two slices whose actions it changed, `people` (a `say` without `to` at a held
node, and a `said` by a session that holds neither its conversation node nor work inside it, now
withheld) and `keys` (its `said` entries), in both class configurations, two at a time, beside the
other groups' model checks; no statement of §28 changed, and the `holds` slices, which have no
`say` or `said`, were not run again.

1.10's group B changed the order of `next` in the model (`best_candidate`: a review item comes
before the other candidates of its tier, after the recheck-due ones), which every slice's `next`
reads, and no statement of §28. It ran the `holds` slices, whose `next` it reorders, and the two
where reviews arise, `people` and `basis`, as 1.9's group B did, two at a time beside the other
groups' model checks; the 20 minutes exclude the translation to TLA+.

1.10's group C ran the slices whose actions it changed, three at a time, on a laptop that the
other groups' model checks and builds loaded to an average of about 50, so each reached less depth
than earlier rounds: `people` (a `said` by a session that holds neither its conversation node nor
work inside it, now counted; a `say` without `to`, which reaches a node's holders or its next
holder; a second verdict on a resolved objection; and checkpoints, which end the `{question}` waits
of its releases, against W1 and M1), in both class configurations; `basis` (a second verdict, and a
withdrawn correction against the decisions that cite the node), in `basis_two`; and `keys` (its
`said` entries, and a checkpoint that ends a `{node}` wait, against T5, M1, and K1 to K5), in both
class configurations, after the slice gained takes: before, nothing held a node again once its
release had set a wait, so its checkpoints ended none (a run of `keys_two` before that, 20 minutes,
reached depth 6 with no violation, 346,327 distinct states). No statement of §28 changed. Not
run: `basis_one`, whose actions are `basis_two`'s in one class; `holds`, whose releases set no
wait, so its checkpoints end none, and which has no `said`, `say`, or `resolve`; `user`, which
has none of the changed commands, and `recurring`, whose releases set no wait either; the liveness
checks, which run on those three slices; and Apalache (D.14.5).

At the merge of 1.10's groups A, B, and C, the two slices where group B's order of `next` meets
group C's second verdicts, `people_two` and `basis_two`, ran again, two at a time, 10 minutes each;
no statement of §28 changed.

1.11's group C changed four actions of the model and the closure every close writes: `resolve`
defers (a refusal record on a closed node, which `next` reads, and a verdict that clears it),
`said` and `ask` to a person confirm the `said` of a session that holds nothing, `say` without `to`
at a closed node reaches its live closer, and every closure records who closed it (`by`, -1 for the
server), which `release`'s `cannot` now records through the helper the deferral shares (`refuse`).
It ran the slices that take those actions: `people` in both class configurations (deferrals,
against E1 and M1 over refusals of closed nodes; `said` and `ask` of sessions that hold nothing;
notes at closed nodes), `keys` in both (its `said` entries and closes, against K1 to K5 and T5),
`holds` in both (its closes and `cannot` releases, against H1 to H6 and O1, O2), and `basis_two`
(its resolves, which now clear refusals, against the basis references), two or three at a time,
beside the conformance sweep, the full checks, and Kani on the same laptop; the 20 minutes exclude
the translation to TLA+. No statement of §28 changed. Not run: `basis_one`, whose actions are
`basis_two`'s in one class and do not defer; `user` and `recurring`, whose only change is the
closure's `by`, which no invariant reads, and `release`'s refactored `refuse`, which the run tests
and the `holds` slices exercise; the liveness checks of the `holds`, `recurring`, and `user`
slices, whose deadlines and leases the group did not change (a deferral queues its refusal's
expiry as a `cannot` release does, in the `people` slice only); and Apalache (D.14.5).

1.11's group B changed four things the model holds: the routing default of `from_user`
(`from_user_ex`), which only the `user` slices take; the order of `next` (`best_candidate`'s
`request` and `needs`, and a review item's tier without `objection`), which every slice's `next`
reads; a series held back while its occurrence waits for its slot (`held_back`), which the
`recurring` slice reaches; and a duplicate signal resolved by the close of its pair's second node
(`close_node`'s step 9), which the `keys` slice reaches through its links and closes. It ran the
`user` and `keys` slices in both class configurations, `recurring_two`, and the `holds`, `people`,
and `basis` slices whose `next` the order changes, as 1.10's group B did, three at a time beside the
other groups' model checks and builds on the same laptop, at a load average of 20 to 43, so each
reached less depth than earlier rounds; the 20 minutes exclude the translation to TLA+, which took
10 to 30 minutes a module under that load. No statement of §28 changed: N1's and S1's step clauses
are not key invariants, and the model checks their state forms only (D.0.3). Not run: the liveness
checks, `recurring_one`, `people_one`, and `basis_one`, whose actions are their two-class
modules' in one class, `user_none`, which has no `from_user`, and Apalache (D.14.5).

1.12's group B changed what `next` reads in the model: `best_candidate` leaves out, for its
caller, the review of an objection the caller raised (`own_review`) and the node it last released as
`handoff` (`handed_back`), which a new session field, `handed_off`, holds from `release_ex` until
another hold of the node or `lease.first`. Every slice's `next` reads the exclusions; the `holds`
and `recurring` slices release as `handoff`, and the `people` slice has objections. It ran those
three slices in both class configurations, two at a time beside the other groups' model checks and
builds, 20 minutes each, the translation to TLA+ excluded, and the liveness of the `holds` slices
(20 minutes each, two at a time): `holds_two` no violation, stopped at depth 5 (11,401 distinct
states, 16,521 generated, 11,370 left); `holds_one` no violation, stopped at depth 5 (9,636
distinct, 24,308 generated, 9,562 left). No statement of §28 changed. Not run: `keys` and `user`,
which make no `handoff` release and no objection, so the exclusions never bind there; `basis`,
whose objections reach the review exclusion as the `people` slice's do, for lack of time in the
round; and Apalache (D.14.5).

1.12's group C changed five things the model holds: a release with `settles` records the decision
`decide` would, after itself (`release_ex`, `record_decision`); a retract of an open question to a
person withdraws it (`PqWithdrawn`), which resolves the waits on it and drops its recheck; `holder:`
of a closed node reaches its live closer; a reopen tags its node `offer`; and `inv_E1` gains (g).
It also corrected `step_O3`, which counted the entry of an expired note leaving as a lost entry
(the core's `may_leave` lets it go): the conformance group `finish_two`, generated with the slice's
invariants checked, found it at a tick of a run with a note, which no TLC run had reached. It ran
the slices that take those actions, three at a time beside the other groups' model checks and
builds on the same laptop, at a load average of 15 to 50: `people` in both class configurations
(releases that settle, retracts that withdraw questions, against W1, M1, and E1; `say` to
`holder:` of a closed node), `holds` in both (the release, against H1 to H6, O1, and O2), `keys_two`
(its releases, reopens, and closes, against K1 to K5 and T5), and `user_two` (releases that settle
user requests); the 20 minutes exclude the translation to TLA+, 18 to 30 minutes a module under
that load. No statement of §28 changed: Settled and E1 are not key invariants, and their 1.12
step clauses, a release's decision and a question withdrawn only by its retract, are executable
checks only (D.0.3), while `step_Settled` keeps Settled's older step clauses and `inv_Settled` and
`inv_E1` their state forms; a wait on a question is no edge of T5's graph,
so a withdrawn question changes none, as the `people` slices' W1 and M1 check. A restart of the
session stopped `keys_one`, `user_one`, and `basis_two` during their translation; `keys_one` and
`user_one` ran again afterwards, two at a time. Not run: `basis`, whose reopens and
retracts of decisions are its only changed actions (the reopen's tag is checked by `keys_two` and
the run test `reopenOfferTest`), `recurring`, and `user_none`, whose changed actions are a release
without `settles`; the liveness checks, whose deadlines and leases the group did not change; and
Apalache (D.14.5).

**1.12, final** (2026-10-05). After the three groups merged and the fixes of 1.12's review (a
review of the whole diff by seven reviewers, each finding checked by three skeptics) were merged,
the model holds what group C's merge left, plus comments. `scripts/verify.sh` ran at d205b62
(`MODEL_CHECK_SECONDS=1200`, beside the review's agents): `npm run typecheck`, `npm test` (201
runs), `npm run simulate` (no violation, 59 s), `npm run traces`, and TLC on `holds_two` and
`holds_one`; the runner's two-hour limit then stopped it during the liveness of `holds_two`. The
remaining checks ran at 9cb1e41, the final code, two at a time with five TLC workers each: the
liveness of `holds_two` (no violation, stopped at depth 5: 17,465 distinct states, 32,366
generated, 17,405 left) and of `holds_one` (no violation, stopped at depth 5: 25,489 distinct,
67,583 generated, 25,269 left), 20 minutes each; TLC on `keys_two`, `keys_one`, `basis_two`, and
`basis_one`, 20 minutes each; and, at 10 minutes each, `people`, `user`, and `recurring` in both
class configurations, so that every slice ran on the merged model. Every run stopped at its time
limit with no violation (the table above). Not run: `user_none` and Apalache (D.14.5).

**Liveness** (`npm run liveness:<module>`, TLC under weak fairness, §27), at 8f8098b, 20 minutes
each: `holds_two` no violation, stopped at depth 5 (15,020 distinct states, 36,636 generated, 14,947
left on the queue); `holds_one` no violation, stopped at depth 5 (31,025 distinct, 88,621 generated,
30,707 left).

**Apalache** has not reached a step on any slice. On the `holds_two` slice at length 1 it type
checks (19 minutes) and then runs out of its 8 GB heap while inlining, at d761685 as before it; on
WP-A1's smoke spec, Z3 ran out of memory on the first settle. It was decided (D.14.5) that Apalache
runs at a smaller depth with only the deadline kinds a slice needs, and that TLC is the exhaustive
check per slice; `scripts/verify.sh` therefore skips Apalache unless `VERIFY_APALACHE=1`.

**Mutation campaigns.** On 2026-10-02, 153 single-point mutants of `basics.qnt`, `flags.qnt`,
`deadlines.qnt`, and `params.qnt` ran against `tests/helpers_test.qnt`; 152 failed it, and the
survivor is equivalent within the model's bounds (`model/README.md`). The non-vacuity campaign of the
invariant checks (each check shown to fail on a mutated model) was not run for the first release:
its tables in `model/README.md` are still placeholders (Known limitations).

## Conformance replay

`crates/forum-conformance` (WP-F1, D.10.8, narrowed by D.0.3 to the key parts) replays traces of the
Quint model against `forum-core`. A hand-written ITF driver reads each trace with `serde_json` and
dispatches on the model's own `last_action` variable, since Quint 0.33 writes no action names into
counterexamples. It replays genesis and the seed of D.10.7 on the scaled parameters, then each step
through the shell's pipeline (`settle`, `apply_with` with the tie-break by id, `settle`), and compares
the outcome, the clock, and the projected state after every step, running `check_step`,
`check_fire`, and `check_invariants` as it goes. A trace that reaches a command of a package not
built yet stops there as a gap, not a divergence.

What it replays:
- the committed fixtures (`tests/traces/`, regenerated with fixed seeds by `scripts/traces.sh`),
  which run in `cargo test --workspace`: 8 traces of the `holds` slice and of a variant that calls
  more often than time passes, 4 of the `keys` slice restricted to the commands the core had when
  they were made, 3 of the whole `keys` slice, and 2 findings (below);
- a counterexample of the `holds` slice for a deliberately false invariant (no fence above 1), which
  the core must reproduce;
- fixtures tampered in six ways (a lease, a fence, an outcome, the clock, an `op_id` record, a key),
  which the replay must catch;
- the sweep: `traces.sh --sweep` writes 200 traces of each module with fixed seeds, and each group is
  replayed (`tests/sweep.rs`, opt-in; `scripts/verify.sh` step 7 and CI's nightly `sweep` job);
  since 1.9 it includes `said_held_two`, whose `said` entries are recorded while another session may
  hold their conversation node (1.9 withheld them; since 1.10 they count), and since 1.10
  `review_two`, the `people` slice's objections, resolves of each verdict, closes, takes,
  checkpoints, and notes, so that second verdicts, withdrawn corrections, and notes for a node's next
  holder are replayed; the driver maps `object`, `resolve`, and `say` for it. Since 1.11 its
  resolves defer too, which the driver passes as `defer`, and the projection compares each
  closure's closer (`by`). Since 1.12 it includes `finish_two`, whose releases settle requests,
  retracts withdraw questions to people, notes go to `holder:` of closed nodes, and reopens tag
  their node `offer`; the driver passes a release's `settles` and maps `retract`.

Two tests replay a group to its end: the whole `keys` slice, and the `holds` fixtures past E1's
question for the user. They waited for WP-D4, WP-D5a, and WP-D5b, and run unignored since c5e91fe.

The replay found two divergences between the model and the core, both in the model, and both
resolved in the model:

| Found | Divergence | Resolution |
|---|---|---|
| WP-F1 (cc1681c), 24 of 800 sweep runs of the `holds` modules | The model queued a session's Silence deadline at `silence_at` (1.3), while 1.4 (R27, §26 H2, D.4.8) and the core queue it at the latest of `silence_at`, the end of the session's last live lease, and `last_now`, so that a holder is never silent | 52cf305: the model got 1.4's Silence key, `inv_H2` 1.4's clause, and `inv_T3` its session marker; `tests/findings.rs` keeps the witness as a regression test |
| WP-F1's replay and the model's simulation (seeds 0x93f4da67ca4de794 and 0x94a12e7bdfc1e83a) | The model's commands did not settle after the command, as D.8.1's post-settle does: a release that ends another session's last hold after its `silence_at` leaves its Silence key at `now`, which the core fires in the same call and the model fired a tick later | d761685: every command action ends with the post-settle, a fifth variable `mid` separates the command's step from its firings (D.14.7), and a second finding fixture pins it |

The model's simulation found a third defect, in the specification rather than between the model and
the core: a merge of a series into its own open occurrence (above). 1.5 refuses it (647b461 in the
model); the core refuses it since WP-D6 (`invalid(MergeIntoOwnOccurrence)`).

Sweep results:

| Model at | Group | Traces | States replayed | Divergences |
|---|---|---|---|---|
| 52cf305 | `holds` | 800 | 28,221 of 28,800 | 0 (24 before the fix) |
| 52cf305 | `keys` (core commands) | 400 | 12,400 of 12,400 | 0 |
| 52cf305 | `keys_full` | 400 | 3,390 of 8,400 (the rest past a stubbed command) | 0 |
| d761685 | `holds`, `keys`, `keys_full`, `cex`, `post_settle` | 800, 400, 400, 1, 200 | 25 `holds` traces stopped at WP-D5a's gap, 311 `keys_full` at WP-D5b's | 0 |
| 8f8098b (the release's key slices) | `cex`, `holds`, `keys`, `keys_full`, `post_settle` | 1, 800, 400, 400, 200 | 28 of 28, 28,800 of 28,800, 12,400 of 12,400, 8,400 of 8,400, 8,200 of 8,200: every trace to its end | 0 |
| 1.8, group C | `cex`, `holds`, `keys`, `keys_full`, `post_settle` | 1, 800, 400, 400, 200 | 25 of 25, 28,800 of 28,800, 12,400 of 12,400, 8,400 of 8,400, 8,200 of 8,200: every trace to its end; 997 traces fire a Stale key, and one has an intent check that records (2,062 checks in all, the rest refused with no intent open) | 0 |
| 1.9, group C | `cex`, `holds`, `keys`, `keys_full`, `post_settle` | 1, 800, 400, 400, 200 | 25 of 25, 28,800 of 28,800, 12,400 of 12,400, 8,400 of 8,400, 8,200 of 8,200: every trace to its end; none of them records a withheld `said` or a note to a holder, so the new group below was added | 0 |
| 1.9, group C | `said_held` (new: `said_held_two`, the `keys` slice's requests and `said` entries beside takes of their conversation nodes) | 200 | 6,200 of 6,200: every trace to its end; 119 traces record a withheld `said`. With the core's withholding turned off by hand, the same traces diverge in 64 of them (5,071 states replayed) | 0 |
| 1.10, group B | `cex`, `holds`, `keys`, `keys_full`, `post_settle`, `said_held` | 1, 800, 400, 400, 200, 200 | 25 of 25, 28,800 of 28,800, 12,400 of 12,400, 8,400 of 8,400, 8,200 of 8,200, 6,200 of 6,200: every trace to its end. No trace of these groups records an objection, so none reaches a review item, whose new place in `next`'s order (1.10) the model's run test `reviewOrderTest` and the core's `ranked_from_the_index_equals_spec` check instead; the driver replays no `object` or `resolve` | 0 |
| 1.10, group C | `cex`, `holds`, `keys`, `keys_full`, `post_settle`, `said_held` | 1, 800, 400, 400, 200, 200 | 25 of 25, 28,800 of 28,800, 12,400 of 12,400, 8,400 of 8,400, 8,200 of 8,200, 6,200 of 6,200: every trace to its end; `said_held` records 56 answers by a session that held neither its conversation node nor work inside it, in 41 traces (1.9 withheld them); the committed fixtures were regenerated, since closures now carry `from_done` and the `keys` slice takes and checkpoints | 0 |
| 1.10, group C | `review` (new: `review_two`, the `people` slice's objections, resolves, closes, takes, checkpoints, and notes, with releases that wait on a node) | 200 | 6,200 of 6,200: every trace to its end; 25 second verdicts in 24 traces, 4 of them withdrawing a correction, 144 notes for a node's next holder in 107 traces, and 3 checkpoints that end a wait. Building the group found two older differences of the model from the core, both fixed in the model: the expiry prune kept a note's entry metadata, which the core drops (10 of 20 first traces diverged), and a `session:` addressee that does not exist was `not_found(Session)` where the core says `not_found(Addressee)` (1 of 40); the model's `holder:` recipients also left the sender in, which the projection does not compare, and now leave it out | 0 |
| 1.10, groups A, B, and C merged | `review` (`review_two`) | 200 | 6,200 of 6,200: every trace to its end; 213 of the 742 `next` calls, in 93 traces, return a review item, in the order of 1.10's group B (`Cand.review`), which the core's order matches | 0 |
| 1.11, group C | `cex`, `holds`, `keys`, `keys_full`, `post_settle`, `said_held` | 1, 800, 400, 400, 200, 200 | 25 of 25, 28,800 of 28,800, 12,400 of 12,400, 8,400 of 8,400, 8,200 of 8,200, 6,200 of 6,200: every trace to its end; `said_held` records 698 `said` entries confirmed as they are recorded, their sessions holding nothing, and `keys_full` 241 (§9.2, 1.11); every close carries its closer, which the projection compares (`closure.by`). The committed fixtures were regenerated: 9 of them change, their closures now carrying `by` | 0 |
| 1.11, group C | `review` (`review_two`, whose resolves now defer too) | 200 | 6,200 of 6,200: every trace to its end; 75 deferrals in 61 traces, 21 of them of a closed node's review, which record the caller's refusal of the node that `next` then reads, and 1 verdict that clears such refusals; 35 notes without `to` at a closed node, in 33 traces, reach the session that closed it | 0 |
| 1.11, group B | `cex`, `holds`, `keys`, `keys_full`, `post_settle`, `review`, `said_held` | 1, 800, 400, 400, 200, 200, 200 | 25 of 25, 28,800 of 28,800, 12,400 of 12,400, 8,400 of 8,400, 8,200 of 8,200, 6,200 of 6,200, 6,200 of 6,200: every trace to its end. In `review`, 221 of the 811 `next` calls, in 100 traces, return a review item, now in its node's tier without the objection (`Cand.request`, `Cand.needs` beside `Cand.review`), which the core's order matches; in `keys_full`, one trace closes the second node of a duplicate pair, whose signal both resolve (`resolve_closed_duplicates`). No group's nodes carry needs, and none has the seed series, which the driver does not replay (WP-D6), so `needs` and a series held back before its slot are checked by `spec::ranked`, the property tests, and the model's run tests instead; and the driver maps no `from_user`, so 1.11's routing default is checked by the core's tests (N1's step clause) and the model's `followUpTest` | 0 |
| 1.12, group B | `cex`, `holds`, `keys`, `keys_full`, `post_settle`, `review`, `said_held` | 1, 800, 400, 400, 200, 200, 200 | 25 of 25, 28,800 of 28,800, 12,400 of 12,400, 8,400 of 8,400, 8,200 of 8,200, 6,200 of 6,200, 6,200 of 6,200: every trace to its end. Sessions now carry `handed_off`, which the projection compares; the committed fixtures were regenerated, all 18 changing in that field alone. In `holds`, 14 of the 1,094 `next` calls come while their session's `handoff` release keeps a node from it (64 such releases); in `review`, 147 of the 760 come while a closed node's open objection the caller raised awaits review. With the core's exclusion of what a session put down turned off by hand (in `spec::ranked`, which the replay's tie-break by id reads), 9 `holds` traces and 60 `review` traces diverge | 0 |
| 1.12, group C | `cex`, `holds`, `keys`, `keys_full`, `post_settle`, `review`, `said_held` | 1, 800, 400, 400, 200, 200, 200 | 25 of 25, 28,800 of 28,800, 12,400 of 12,400, 8,400 of 8,400, 8,200 of 8,200, 6,200 of 6,200, 6,200 of 6,200: every trace to its end. The driver now passes a release's `settles` (none in a trace of a model before 1.12) and maps `retract`, and a release that settles names its decision in the reply's ids. The committed fixtures were regenerated: 12 of them change, since the model's releases carry `settles` and its runs draw other choices | 0 |
| 1.12, group C | `finish` (new: `finish_two`, the `people` slice's requests, answers, releases that settle them, questions to people and their retracts, notes to `holder:` of nodes that close, and reopens) | 200 | 6,200 of 6,200: every trace to its end; 34 releases with `settles` in 32 traces, which settle 11 requests; 90 questions withdrawn in 80 traces; 120 notes to `holder:` of a closed node in 77 traces, 53 of which reach its live closer; 81 reopens that tag their node `offer`, in 69 traces | 0 |
| 1.12, groups A, B, and C merged | `cex`, `holds`, `keys`, `keys_full`, `post_settle`, `review`, `said_held`, `finish` | 1, 800, 400, 400, 200, 200, 200, 200 | 25 of 25, 28,800 of 28,800, 12,400 of 12,400, 8,400 of 8,400, 8,200 of 8,200, 6,200 of 6,200, 6,200 of 6,200, 6,200 of 6,200: every trace to its end. The committed fixtures were regenerated from the merged model: the 12 that both groups changed are group C's with group B's `handed_off`, and the other 6 are group B's, unchanged. In `finish`, 34 releases with `settles`, in 32 traces, as at group C; in `holds`, 10 of the 1,099 `next` calls that succeed come while their session's `handoff` release keeps a node from it (57 such releases), and in `post_settle` 2 of 508 (12). No trace makes a `handoff` release with `settles`, where the two groups' releases meet: the core's `a_handoff_release_that_settles_keeps_its_node_from_the_releaser` and the model's `handoffSettlesTest` check it instead | 0 |
| 1.12, final (9cb1e41) | `cex`, `holds`, `keys`, `keys_full`, `post_settle`, `review`, `said_held`, `finish` | 1, 800, 400, 400, 200, 200, 200, 200 | 25 of 25, 28,800 of 28,800, 12,400 of 12,400, 8,400 of 8,400, 8,200 of 8,200, 6,200 of 6,200, 6,200 of 6,200, 6,200 of 6,200: every trace to its end, on the final code with the review's fixes (`traces.sh --sweep`, then each group replayed) | 0 |

1.8 changes which runs the model draws from a seed, so run 49 of `holds_two` (seed 0x5eed01) is no
longer the Silence key's witness, and none of 200 runs of 1.8's `holds_two` for seven seeds has a
session whose hold another session extends past its `silence_at`. The finding's fixture is now run
50 of `holds_silence_two` (`quint/conformance.qnt`, the same seed), `holds_two` without 1.8's intent
check, in which session 1 checkpoints session 2's hold at state 7 and session 2 stays present at
state 8, past its `silence_at`.

## Scale gates

The gates of D.12.8 (WP-F4) drive the server over MCP HTTP with `swarm-sim`, simulated agents with
the call mixes and profiles of D.15.2, against a build with the `bench` feature (profile
`bench-gates`). They are specified for **reference hardware**: 16 vCPU, 64 GB, and NVMe with a
durable fsync of at most 1 ms.

**No run so far used the reference hardware.** Every number below is a smoke run
(`scripts/bench/run-all.sh --smoke`, each gate scaled down to finish in minutes) on a 12-core laptop
(Apple M3 Pro, 36 GiB, macOS, APFS) whose durable 4 KiB fsync took 3.0 to 3.9 ms at p50 and 4.1 to
7.0 ms at p99, with the driver and the server on the same machine and other agents' builds and tests
running beside them (a load average of 5 to 12). Latencies vary between runs for that reason. The
commands of core packages that were not merged yet answered as stubs; the mixes count those answers
apart, not as errors.

| Gate | Pass | Smoke, 2026-10-03, at aa97774 (WP-F4) | Smoke, 2026-10-03, at 0090ef7 (search and wake-up fixes) |
|---|---|---|---|
| G-1 latency | p99 writes ≤ 50 ms, reads ≤ 20 ms, `next` ≤ 30 ms, `hello` ≤ 100 ms; p99.9 ≤ 250 ms | **fail**: `control health` 28.9 ms in the heavy phase; writes 16.9, reads 15.9, `next` 14.0, `hello` 34.3, p99.9 37.9 ms | **fail**: reads 37.9 ms, with other agents' tests loading the machine; alone on a calmer machine, reads 9.7 ms and every bound met |
| G-2 writer | ≤ 30 % busy at base, ≤ 60 % in a burst | pass: 3.9 % and 17.5 % | pass: 5.6 % and 37.2 % |
| G-3 cost test | visits bounded; time ratio ≤ 1.5 | pass, at the test's own sizes, without `guide` and `health` | not yet |
| G-4 wake-ups | readiness checks ≤ the sessions a commit concerns; wake work ≤ 5 % | pass: 0 checks, 4.4 % with 1,000 parked sessions | pass: 0 checks, 0.1 %; the hot node's part recorded only while `talk wait` is a stub |
| G-5 settle storm | no call waits > 1 s; p99 ≤ 250 ms | pass: 58 ms longest, p99 55 ms (2,000 silences and 1,000 leases) | pass: 60 ms, p99 25.1 ms |
| G-6 memory | resident state ≤ 10 GB, flat after 7 days | pass: 0.27 GiB peak over 8 forum days | **fail**: resident memory kept growing after day 7, at 0.57 times its rate before (limit 0.2); nodes never leave memory (WP-D8, below) |
| G-7 startup | serving within 5 min | pass: 0.16 s after 2 forum days of the high profile, with and without the `shell_version` bump | pass: 0.16 s |
| G-8 file growth | recorded; an append on a big node ≤ 1.5 × a fresh node's | pass: 555 B a call, so 96, 48, and 24 months to 17.59 TB at the low, base, and high profiles; ratio 0.85 | pass: ratio 1.02 |
| G-9 search | `hello` p99 ≤ 50 ms with the search; writer unchanged | **fail**: 94 ms; the smoke file held 12,661 documents, not 50,000 (a driver defect) | pass: 25.1 ms over about 50,000 documents |
| G-10 replay | equal to the state tables; a day within 15 min | pass: 122,000 events in 0.63 s, 257 s projected for 5 × 10⁷ | pass: 328 s projected |
| G-11 group commit | identical bytes on retry; one holder per slot | not yet: the race test (WP-F2) does not exist | not yet |
| G-12 connections | no descriptor errors; shell ≤ 2 GB | pass: 2,000 sessions, 1,000 parked, 0.72 GiB | pass: 0.50 GiB |

What the smoke runs changed: the worst-case search queued on SQLite's shared page cache and gave
each scope's fallback a budget of its own, so 0090ef7 bounds one search by one deadline and ranks
the scopes in one query (D.11.12); the wake-up check now examines only the sessions a commit
concerns; and the drivers of G-9 and G-4 were corrected.

**G-6 fails at scale until WP-D8.** Closed nodes never leave memory in the first release: WP-D8
(D.4.13) was deferred to after it. The smoke run made about 2,000 nodes a day at 30 simulated
agents, every one of them resident; at 10,000 agents that is tens to hundreds of GB a year, so G-6's
bound of 10 GB, flat after FINISHED_RETAIN, cannot hold there. A personal or company forum (10⁴ to
10⁵ nodes) is not affected.

**Full scale.** `--full` runs each gate as D.12.8 specifies. At the throughput measured so far, some
take long over HTTP: G-6 about 90 hours, G-7 about 91 days (365 forum days of the high profile), G-9
hours to days to index 10⁸ documents, and G-10 about 3 hours; `--days` and `--docs` shorten them, and
each prints its estimate when it starts.

**The final run** (2026-10-04, `scripts/bench/run-all.sh --smoke` at 46fb439, with every core
package merged, so every response carries its next calls and no command is a stub). The same
laptop: durable fsync p50 2.9 to 3.0 ms, p99 4.1 to 5.3 ms; a usability round and a model
simulation ran beside it.

| Gate | Result | Numbers |
|---|---|---|
| G-1 latency | pass (re-run at 8f9e7d9) | p99 writes 9.1 ms, reads 14.1 ms, `next` 6.4 ms, `hello` 14.5 ms, `control health` in the herd 5.5 ms, p99.9 18.4 ms; 0 unexpected errors. The first run failed only on 394 `talk heard` calls refused `invalid(NoConversationKey)`: the driver named no conversation key, which 8f9e7d9 adds, as its `said` already did |
| G-2 writer | pass | 6.4 % busy at base, 27.7 % in the heaviest burst |
| G-3 cost test | not yet | the core's cost tests pass at their own sizes, `guide` and `health` included (time ratios 1.0 to 1.55), not at D.12.8's 10⁴ and 10⁶ nodes |
| G-4 wake-ups | pass | 204 commits a second; 0 parked sessions examined; wake work 0.2 to 0.3 % of the writer; addressed-wait latency p99 1,016 ms against 1,100 ms |
| G-5 settle storm | pass | longest wait 63 ms, p99 37 ms over 5,621 events |
| G-6 memory | **fail** | peak 0.35 GiB; resident memory grew after day 7 at 0.49 times its rate before (limit 0.2): nodes never leave memory (WP-D8), and digest entries nobody is shown stay 28 days (DIGEST_RETAIN), which 9a3d1ac makes the gate wait for; sessions, op records, notes, agent questions, and the shell's maps are flat |
| G-7 startup | pass | serving 0.26 s after a restart, with and without the `shell_version` bump |
| G-8 file growth | pass | an append on a big node writes 1.05 times a fresh node's bytes |
| G-9 search | pass | `hello` search p99 25.1 ms; writer unchanged |
| G-10 replay | pass | the history equals the state tables; 332 s projected for 5 × 10⁷ events |
| G-11 group commit | not yet | the race test (D.12.7) is not written: WP-F2 was light; its crash tests cover retries with stored bytes |
| G-12 connections | pass | 2,000 sessions, 0 descriptor or transport errors, 0.54 GiB |

**G-9 and 1.9's search** (2026-10-05, `scripts/bench/g09-search.sh --smoke`, the same laptop, the
databases kept to measure the search file). 1.9's first draft (72aa2e0) kept the 6 rarest terms of
a query whatever their lists; 1.9 keeps 4, as 1.8 did, and more only while their lists hold fewer
than 500 documents (D.11.12); 6895554 has 1.8's search. Runs of the two 1.9 servers alternated, as
other agents' builds, property tests, and model checks began to share the machine: the load is its
1-minute load average when the run started and when it ended. The check passes when the writer's
busy share with worst-case searches exceeds the one with cheap searches by at most a tenth of the
latter, or 0.01.

| Run | Server | Load | Worst `hello` p99 | Cheap `hello` p99 | Busy, cheap | Busy, worst | Worst − cheap | Bound | Check | Search file, WAL |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | first draft | 3.6 → 2.7 | 16.9 ms | 45.1 ms | 0.136 | 0.155 | +0.019 | 0.014 | fail | 15.8 MB, 5.1 MB |
| 2 | first draft | 2.8 → 2.9 | 16.6 ms | 48.1 ms | 0.137 | 0.156 | +0.018 | 0.014 | fail | 15.3 MB, 5.2 MB |
| 3 | 1.8's search | 2.9 → 7.0 | 17.3 ms | 41.0 ms | 0.128 | 0.117 | −0.011 | 0.013 | pass | 13.3 MB, 5.4 MB |
| 4 | first draft | 2.9 → 6.5 | 40.4 ms | 44.0 ms | 0.078 | 0.098 | +0.020 | 0.010 | fail | 15.3 MB, 5.5 MB |
| 5 | 1.8's search | 6.5 → 10.2 | 18.9 ms | 35.3 ms | 0.123 | 0.134 | +0.011 | 0.012 | pass | 13.7 MB, 8.3 MB |
| 6 | 1.9 | 4.1 → 5.2 | 12.4 ms | 31.5 ms | 0.068 | 0.057 | −0.010 | 0.010 | pass | 15.3 MB, 5.4 MB |
| 7 | first draft | 5.2 → 5.4 | 13.4 ms | 40.4 ms | 0.064 | 0.066 | +0.001 | 0.010 | pass | 15.2 MB, 5.4 MB |
| 8 | 1.9 | 5.4 → 14.5 | 32.8 ms | 40.4 ms | 0.143 | 0.133 | −0.009 | 0.014 | pass | 15.7 MB, 5.2 MB |
| 9 | first draft | 14.5 → 25.2 | 28.2 ms | 42.5 ms | 0.219 | 0.204 | −0.015 | 0.022 | pass | 15.4 MB, 5.2 MB |
| 10 | 1.9 | 25.2 → 21.0 | 19.7 ms | 49.2 ms | 0.226 | 0.173 | −0.053 | 0.023 | pass | 15.2 MB, 5.2 MB |
| 11 | first draft | 20.3 → 38.3 | 17.9 ms | 38.4 ms | 0.161 | 0.172 | +0.011 | 0.016 | pass | 15.4 MB, 5.0 MB |

What they show. G-9's worst case, the eight most common words, falls back to one page of the
rarest list with any floor of terms, since its four rarest lists already hold 137,000 documents,
more than RANKED_MAX: it costs what 1.8's did. Counted in SQLite's page cache
(`sqlite3_db_status`, one search alone, a scratch probe over run 2's search file), a worst-case
`hello` fetches 1,170 pages and a `view find` 185, with either floor, where 1.8's fetched 1,104 to
1,155 and 122 to 170 in its own files. The cheap search, eight words of about 100 documents each,
is the costlier: 3,110 pages with the floor of 6 (6 terms, 553 documents), 2,653 with 4 (5 terms,
446), and 1,932 to 2,514 for 1.8 (4 terms, 349). The writer check followed the machine's load more
than the server: on the quieter machine the first draft failed it three times, by +0.018 to +0.020,
as it did before (+0.016 to +0.025, and +0.022 on an idle machine), and 1.8's search passed twice
with −0.011 and +0.011; under heavier load every run passed. With the search turned off altogether
(a scratch build whose searches return nothing at once), the writer's busy share went from 0.041 to
0.133 across one run's four phases, and in run 3's last phase it fell to 4 % a second while another
agent's property test used eight cores. On this laptop, with other jobs beside it, the check cannot
tell the two floors apart: the reference hardware, or at least an idle machine, is to settle it.


**At 1.10 (73f15b7), on an idle machine** (2026-10-05, load average 2.5 to 3.6, the same laptop): two smoke runs,
back to back. Worst-case `hello` search p99 30.0 ms and 15.7 ms (pass, bound 50 ms); cheap `hello` p99 43.4 and
42.5 ms; writer busy share with cheap searches 0.181 and 0.183, with worst-case searches 0.202 and 0.201, so the
difference was 0.021 (fail, bound 0.018) and 0.018 (pass, bound 0.018). The writer check sits at its bound on this
laptop: heavy searches add about two points of busy share, with four fifths of the writer still free. It stays
open until a run on the reference hardware (durable fsync at most 1 ms, a third of this laptop's) settles it.

## Year simulation

`crates/forum-sim` (WP-Y, light) runs the core in process over 30 measured days, after 28 days of
warm-up and before 28 of cool-down, with simulated principals, workers that vanish mid-task, and
scheduled jobs (design D.15.4), checking every invariant at intervals. It found two defects, both
fixed in 1.5: a question's first stage waited weeks for a principal who writes about monthly (now
capped at `capacity.window`), and a lapsed escalation answered late never finished. Its guided mode,
in which a worker makes the first `now` next call of a response with probability `--p-follow`, ran
for the first time once the next calls existed, and its own bookkeeping was wrong (47390d6).

Final runs, at 47390d6, 30 days, seed 1 (`cargo run --release -p forum-sim -- --profile <p> --days
30 --seed 1 [--p-follow 0.9]`):

| Profile | Worker sessions | Calls | Holds issued | Invariant checks | `health` red | D.15.4 |
|---|---|---|---|---|---|---|
| `company`, 200 principals | 10,581 | 309,502 | 30,325 | 87, no violation | 0.0 % of hourly samples | every row met or reported: 89.5 % of questions answered by their first addressee, 0 lapses, nothing misdirected |
| `company`, guided (0.9) | 10,903 | 322,468 | 31,613 | 87, no violation | 0.0 % | the same: 88.3 % by the first addressee, 5 lapses, nothing misdirected |
| `no-user` | 1,390 and 629 scheduled | 13,027 | 1,701 | 31, no violation | 0.3 % | every row met or reported: nothing waited past its chain, no node parked, every lapse recorded its assumption |
| `no-user`, guided (0.9) | 1,531 and 637 scheduled | 14,290 | 1,871 | 31, no violation | 0.1 % | the same; 170 of 170 lapses got an assumption |

Guided agents work like unguided ones: within 1.25 times their sessions and holds, and no followed
call refused (`tests/profiles.rs`). The light form leaves out the profiles of 10,000 agents and the
year-long run of D.15.4.

## Usability rounds

Agents that know only the instructions (§22) and the tool descriptions (§20.2) use the forum through
scripted scenarios; what they do wrong becomes changes to the protocol's texts, rules, and server.
Two rounds ran against a server that a model simulated; the rest, the final round and the lab's
seven, against the real server.

**Round 1, Wizard-of-Oz, on 1.3** (before any code). Agents worked through seven scenarios against a
server simulated from the specification: 19 runs, most with next calls and some without. The core of
the protocol held where the agent did the work itself (no duplicate pull request, no non-idempotent
step repeated), but the most expensive failures were silent: a series closed so that its reminder
never ran, a reply recorded and never sent, a human told nothing. Agents confused their own human, a
person in a thread, and recording with sending; responses hid what a call had done; the next calls
were the part agents praised most (14 of 15 valid runs with guidance); and the agents' own ratings did
not track their results. Its 27 recommendations, R1 to R27, were decided on 2026-10-02 and became 1.4
(D.14.4).

**Round 2, Wizard-of-Oz re-run, on 1.3.** Harness defects had broken 8 of round 1's 19 runs; they
were re-run on the same frozen 1.3. In all five re-runs with a human, the human heard nothing: the
agent recorded a `to_user` or a `said` and took the record for delivery. With guidance the weaker
model met more required criteria in all three pairs (5 of 6 against 3 of 6, 5 of 8 against 1 of 8,
and 4 of 6 against 1 of 6), but only where the server can see the slip. Its findings, R28 to R40,
joined 1.4 (D.14.5).

**Smoke run, live, on 1.4** (2026-10-03). The harness of WP-U ran the `s0-smoke` scenario (a worker
vanishes mid-task, the next takes over) through the vendors' own clients: three runs, a Claude model
in both arms and a Codex model with guidance, with no slip. It found three client problems that kept
Claude agents from the forum: a client on MCP 2026-07-28 dropped every tool whose listing lacked that
version's fields; the Anthropic API refuses a tool input schema with a top-level `oneOf`, which 1.4
had given every tool; and Claude Code shows its model only a result's structured content, which then
lacked the status line. 1.5 and WP-E5 fixed all three in the server (D.14.7).

**Round R3, live, on 1.5** (2026-10-03). Eight runs of `s0-smoke`: four models (`claude-sonnet-5-5`,
`claude-opus-5-5`, `gpt-6.1-sol`, and `gpt-6-astra`), each in both arms, one run each, and an
interview of all 16 agent sessions. Only the four Claude runs counted: the Codex runs used an
unisolated Codex home, whose global instructions and tools reached the agents, so they serve as
observations. All eight ended with the work done and no slip; the core had only WP-D1 to WP-D4, so
the commands of the user, people, and decisions answered as stubs, and `next_calls` was always
empty. This round measured the tools on a short path, not the protocol. What it found:
- the stubs' refusal read like a real parameter error: 21 refusals in 6 of 8 runs, agents rewriting
  `talk decide` two to five times;
- the texts made `talk decide` a step of every close, so 6 of 8 workers tried to decide on a node
  with no request;
- the brief cut the node's body at 256 bytes, so 15 of 16 sessions read the whole node next, and no
  first worker checkpointed before the scenario ended it;
- the Codex sessions met a required argument (`version`) only by its refusal, in 4 of 4 runs; the
  Claude sessions, in none;
- both Claude Sonnet runs held a node in another session than their `hello`, since a client on MCP
  2026-07-28 keeps no MCP session and the session id came last in the structured content;
- a failed first call did not say what it had applied; a scheduler's prompt was logged as a human's
  words (3 of 8 sessions without guidance, all Codex, none with it); and an unisolated run invented
  a principal from what had leaked into its context;
- the interview misled the Claude sessions ended after their third call: their client had kept only
  one or two of those calls, and each blamed itself for a stop it had not made.

The protocol's part became 1.5 (D.14.7); the server's became WP-E6; the harness now tells an ended
session which calls it made, checks what a client adds from its login, and refuses an
unisolated Codex home unless told otherwise. The Claude runs cost 0.37 USD and their interviews
0.32 USD.

**The final round** (2026-10-04, at 8f8098b, every core package merged; the report is
[`usability/report.md`](../usability/report.md)). The campaign was cut to one round: 9 scenarios
× 4 models (Claude Opus 5.5 and Sonnet 5.5, GPT-6.1 Sol and GPT-6 Astra) × 2 arms × 1 run, with
every one of the 154 sessions interviewed; Claude cost 25.58 USD. The harness counted no run, since
every run had an isolation problem (the client's login details reached the model; Codex ran in an
unisolated home), so the figures are observations recomputed by hand: 72 slips with the next calls
and 82 without (62 and 67 without the harness's artefacts), with clear gains in routing and
replying and none overall. Four findings were serious for agents and were fixed before the release,
specification first: health that read red at once in a small forum (46fb439, 2c36609), `said`
entries listed as `unsent` that the next calls still asked to confirm, texts that kept a worker from
reporting to another principal's requester (2c36609), and Codex's code mode, which hid the forum's
tools in 35 of 76 Codex sessions (a README note; the harness's change is open). The lab's rounds,
below, ran on code with these fixes, from the first (it0, on 1.5).

**The lab, seven rounds, on 1.5 to 1.11** (2026-10-04 to 2026-10-05; `usability/run.sh lab`, the
README's "The lab"). Each round seeded forums at different stages of a product's life (day one, a
solo month, a company quarter, a mature year, a research programme, an incident, and, from the fifth
round, six workers arriving at once at one epic), put three agents on each (rotating GPT-6.1 Sol, GPT-6
Astra, and Claude Sonnet 5.5; Codex given the server instructions as its starting prompt and run in
an unisolated Codex home, so its global instructions and skills reached its agents, which the
analyses flag as harness effects), let a simulated world and simulated people answer their actions,
interviewed every session, and had one analyst per forum compare the run with the seed's intended
flow. Each round's findings became the next version: "What changed in 1.6" to "What changed in 1.12"
in the protocol. On the six forums every round shared (18 sessions):

| Round | Protocol | Calls | Refused | Wasted | Done / partial / not done | Tool list |
|---|---|---|---|---|---|---|
| it0 | 1.5 | 314 | 13.1 % | 25.5 % | 12 / 4 / 2 | 57 KB |
| it1 | 1.6 | 319 | 11.3 % | 21.3 % | 9 / 8 / 1 | 57 KB |
| it2 | 1.7 | 237 | 5.1 % | 19.8 % | 10 / 7 / 1 | 24 KB |
| it3 | 1.8 | 239 | 5.0 % | 18.8 % | 13 / 4 / 1 | 25 KB |
| it4 | 1.9 | 213 | 6.1 % | 17.4 % | 11 / 7 / 0 | 25 KB |
| it5 | 1.10 | 236 | 5.5 % | 16.9 % | 14 / 4 / 0 | 25 KB |
| it6 | 1.11 | 206 | 2.9 % | 14.6 % | 13 / 5 / 0 | 25 KB |

"Wasted" counts calls the analyst judged unneeded for the task; "refused" includes refusals the
protocol intends (from_user without a principal). The harness improved between rounds too (one
simulated world per run, conditional facts, message ids, the whole turn as the reply), so the early
gains are partly the harness's; it4 to it6 ran on nearly the same harness. In the parallel forum, five
of six workers finished a task on 1.9, four on 1.10, and five on 1.11, and no two ever took one task.
Each version fixed the previous round's high findings, but two of 1.11's broad text changes regressed
elsewhere (a "decide now" in every close item, and a route to the first related row); 1.12 undoes both
and fixes it6's other findings. 1.12 has had no lab round of its own: the next round should re-run the
lab on it before changing more, and change less per version.

## Kani harnesses

`cargo kani -p forum-cron` and `cargo kani -p forum-core` run every harness (`scripts/verify.sh`
step 5, CI's nightly `kani` job). The harnesses are behind `#[cfg(kani)]`, in
`crates/forum-core/src/kani/` and `crates/forum-cron/src/kani.rs`, each with an explicit unwind
bound and unwinding assertions on.

<!-- kani:begin -->
<!-- Generated by scripts/gen-verification-table.py; do not edit by hand. Run `python3 scripts/gen-verification-table.py` after the registry, the model, or a Kani harness changes. -->

37 harnesses are written. *Scope* is *key* when a key invariant's registry row names the harness, *other* when only other rows do, and *not in the registry* otherwise (the primitives D.12.5 lists under the key invariants, and harnesses that D.9.2 names but the registry does not).

| Harness | Crate | File | Unwind | Registry rows | Scope |
|---|---|---|---|---|---|
| `evolve_counts_anomalies` | forum-core | `crates/forum-core/src/kani/evolve.rs` | 4 | Total | key |
| `evolve_no_panic_entities` | forum-core | `crates/forum-core/src/kani/evolve.rs` | 4 | R1 | other |
| `evolve_no_panic_holds` | forum-core | `crates/forum-core/src/kani/evolve.rs` | 4 | R1 | other |
| `evolve_no_panic_nodes` | forum-core | `crates/forum-core/src/kani/evolve.rs` | 4 | R1 | other |
| `evolve_no_panic_sessions` | forum-core | `crates/forum-core/src/kani/evolve.rs` | 4 | R1 | other |
| `version_bumps_o2` | forum-core | `crates/forum-core/src/kani/evolve.rs` | 4 | O2 | key |
| `close_ends_holds` | forum-core | `crates/forum-core/src/kani/holds.rs` | 1 | H6 | key |
| `event_stamps_t1` | forum-core | `crates/forum-core/src/kani/holds.rs` | 4 | T1 | key |
| `fire_once_small` | forum-core | `crates/forum-core/src/kani/holds.rs` | 4 | T4 | key |
| `hold_cmds_preserve_h` | forum-core | `crates/forum-core/src/kani/holds.rs` | 4 | H1, H2, H3, P2 | key |
| `lease_extend_h5` | forum-core | `crates/forum-core/src/kani/holds.rs` | 1 | H5 | key |
| `lease_issue_h5` | forum-core | `crates/forum-core/src/kani/holds.rs` | 1 | H5 | key |
| `next_key_total` | forum-core | `crates/forum-core/src/kani/holds.rs` | 1 | - | not in the registry |
| `op_expiry` | forum-core | `crates/forum-core/src/kani/holds.rs` | 33 | O1 | key |
| `op_retry_no_effect` | forum-core | `crates/forum-core/src/kani/holds.rs` | 33 | O1 | key |
| `reaches_terminates` | forum-core | `crates/forum-core/src/kani/holds.rs` | 5 | T5 | key |
| `renew_limit` | forum-core | `crates/forum-core/src/kani/holds.rs` | 1 | H5 | key |
| `session_retire` | forum-core | `crates/forum-core/src/kani/holds.rs` | 1 | H2 | key |
| `settle_idempotent_small` | forum-core | `crates/forum-core/src/kani/holds.rs` | 4 | T3 | key |
| `settle_path_independent_small` | forum-core | `crates/forum-core/src/kani/holds.rs` | 4 | T2 | key |
| `silence_after_leases` | forum-core | `crates/forum-core/src/kani/holds.rs` | 4 | H2 | key |
| `stale_token_only_late` | forum-core | `crates/forum-core/src/kani/holds.rs` | 33 | H4 | key |
| `take_gate_closed_nodes` | forum-core | `crates/forum-core/src/kani/holds.rs` | 2 | H6 | key |
| `deadline_order` | forum-core | `crates/forum-core/src/kani/primitives.rs` | 3 | T4 | key |
| `flagset_ops` | forum-core | `crates/forum-core/src/kani/primitives.rs` | 15 | - | not in the registry |
| `fnv_reference` | forum-core | `crates/forum-core/src/kani/primitives.rs` | 20 | - | not in the registry |
| `fnv_total` | forum-core | `crates/forum-core/src/kani/primitives.rs` | 20 | - | not in the registry |
| `time_arith_total` | forum-core | `crates/forum-core/src/kani/primitives.rs` | 1 | - | not in the registry |
| `token_roundtrip` | forum-core | `crates/forum-core/src/kani/primitives.rs` | 42 | - | not in the registry |
| `keys_open_unique` | forum-core | `crates/forum-core/src/kani/restructure.rs` | 4 | K1 | key |
| `merge_redirects` | forum-core | `crates/forum-core/src/kani/restructure.rs` | 5 | K5, N5, T5 | key |
| `move_keeps_tree` | forum-core | `crates/forum-core/src/kani/restructure.rs` | 4 | K3, K4 | key |
| `civil_roundtrip` | forum-cron | `crates/forum-cron/src/kani.rs` | 1 | - | not in the registry |
| `cron_day_rule` | forum-cron | `crates/forum-cron/src/kani.rs` | 1 | - | not in the registry |
| `cron_next_slot_sound` | forum-cron | `crates/forum-cron/src/kani.rs` | 64 | - | not in the registry |
| `tz_earliest` | forum-cron | `crates/forum-cron/src/kani.rs` | 4 | - | not in the registry |
| `weekday_reference` | forum-cron | `crates/forum-cron/src/kani.rs` | 1 | - | not in the registry |
<!-- kani:end -->

**Narrowed harnesses** (D.12.5 asks that this document record them). A whole command or settle on a
symbolic `State` does not leave CBMC's symbolic execution within the CI budget (30 minutes for the
whole set): every `BTreeMap` operation is expensive for it. So most core harnesses check the pure
function that enforces their property, which the production code calls, over all its inputs, and
the end-to-end form is a test:
- WP-D2's `evolve_no_panic_*`, `evolve_counts_anomalies`, and `version_bumps_o2`: the index
  arithmetic, `NodeUpdated`'s precondition (`patch_differs`), and the counter rule
  (`counter_bumps`); end to end, `evolve::random_tests` applies every event variant (60 sequences of
  300 events in CI) and checks the indices against the recount after each, and unit tests check
  that an anomaly changes nothing but its count. Whether totality, a key invariant, may keep this
  narrowed form is still to be decided;
- WP-D3's holds and time harnesses: `lease_l`, `first_lease`, `extended_until`, `renew_allowed`,
  `take_gate`, `next_fence`, `gate`, `live_on`, `retire_after`, `ends_on_close`, `drain` (the settle
  loop, over a model queue of up to two keys), `clamp_now`, and `reaches`'s `search` and `path`; end
  to end, the holds universe of `tests/prop.rs`. `stale_token_only_late`, `op_retry_no_effect`, and
  `op_expiry` compare 32-byte request digests and run at an unwind of 33, not D.12.5's 4.
  `silence_after_leases` writes D.4.8's Silence time in the harness and checks `DeadlineKind`'s order
  and `drain`;
- WP-D4's `keys_open_unique`, `move_keeps_tree`, and `merge_redirects`: the decisions
  `join_adds`, `key_add`, `reopen_keeps`, `move_place`, and `redirect` on a model of the facts they
  change; end to end, the keys universe of `tests/prop.rs`;
- `token_roundtrip`: values below 10⁴, not any pair of `u64`; the full range is in the unit tests;
- `cron_next_slot_sound`: `after` within the first three days, which covers every time of day;
  `cron_day_rule` takes a date's fields, which `civil_roundtrip` and `weekday_reference` cover.

**Results of the final run** (2026-10-04, Kani on the 12-core laptop, beside other work):
- `cargo kani -p forum-cron` at 8f8098b: 5 of 5 verified in 1,455 s: `cron_day_rule` 0.1 s,
  `weekday_reference` 18 s, `tz_earliest` 149 s, `cron_next_slot_sound` 369 s, `civil_roundtrip`
  915 s.
- `cargo kani -p forum-core` at 8f8098b: 31 of 31 verified in 3,553 s. The longest:
  `token_roundtrip` 1,163 s, `hold_cmds_preserve_h` 650 s, `version_bumps_o2` 484 s, `fnv_total`
  433 s, `merge_redirects` 248 s, `reaches_terminates` 171 s, `silence_after_leases` 169 s; the other
  24 under 70 s each.

The release's later core changes (O-1's `due_for`, rule 7's reason on a held node, rule 10's merge
direction, the idle share and the red-by-age rule of `health`, the reply to a user entry, a `said`
listed in `unsent` at a release or close, and the registry) are outside every harness's reach: the
harnesses drive `evolve`, the hold, close, and restructure helpers, `settle`'s drain, and the
primitives directly, never a command's execution, `due_for`, `guide`, or `health`. So the results
carry over.

**1.11, group C** (2026-10-05, `cargo kani -p forum-core --harness <name>`, one at a time, beside
TLC, the conformance sweep, and the full checks on the same laptop): `Node` gained
`retracted_decision`, `Closure` `by`, `Objection` `deferred`, `NodeClosed` and `DecisionRetracted`
a field each, `EntryBody::Said` `voice`, `EntryBody::Resolve` `deferred`, and `ConfirmVia` a
variant, and `evolve`'s arms of `NodeClosed`, `DecisionRetracted`, and `Appended` read them, so the
six harnesses of `crates/forum-core/src/kani/evolve.rs` ran, whose `blank()` node builds the new
field, with `take_gate_closed_nodes` (closed nodes, which a deferral's refusal now concerns): 7 of 7
verified, `evolve_no_panic_sessions` 10 s, `evolve_no_panic_nodes` 9 s, `evolve_no_panic_holds`
9 s, `evolve_no_panic_entities` 11 s, `evolve_counts_anomalies` 15 s, `version_bumps_o2` 211 s, and
`take_gate_closed_nodes` 9 s (the times include the build). No harness was added or changed but
for `blank()`'s new field: the group's other changes (a deferral's validation and refusal, the
voice by its cap, a `said` confirmed as it is recorded, a note's closer, `leave_better`'s deferred
reviews, and the texts) are commands, queries, next calls, and the shell, outside every harness's
reach; `hold_cmds_preserve_h` and `close_ends_holds` check gates and `ends_on_close`, which did not
change, and were not run.

**1.12, group C** (2026-10-05, `cargo kani -p forum-core --harness <name>`, one at a time, beside
TLC and the other groups' checks on the same laptop, at a load average of about 15 to 50):
`PqState` gained a variant (`Withdrawn`, with its derived `Arbitrary`) and `evolve` an arm
(`PqWithdrawn`), so the five harnesses of `evolve`'s totality and preconditions ran; a release
records its decision after its hold has ended, which `hold_cmds_preserve_h` (H1, H2, H3, P2 over
take, checkpoint, release, time, pause, and close), `stale_token_only_late` (H4: a release's
`settles` are read after the token), and `close_ends_holds` (H6: a release's decision on a signal
node closes it) cover, and a reopen now tags its node, beside `take_gate_closed_nodes`: 9 of 9
verified, `evolve_no_panic_sessions` 8 s, `evolve_no_panic_nodes` 7 s, `evolve_no_panic_holds`
6 s, `evolve_no_panic_entities` 9 s, `evolve_counts_anomalies` 11 s, `close_ends_holds` 8 s,
`stale_token_only_late` 8 s, `take_gate_closed_nodes` 8 s, and `hold_cmds_preserve_h` 1,266 s (the
times include the build). No harness was added or changed: the group's other changes (the
decision of a release, the withdrawal's validation, a note's recipients at a closed node, the
warnings, and the replies) are commands and the shell, outside every harness's reach.

**1.10, group C** (2026-10-05, `cargo kani -p forum-core --harness <name>`, one at a time, beside
TLC, the conformance sweep, and the other groups' checks on the same laptop, at a load average of
about 50): `evolve` gained an arm (`CorrectionWithdrawn`), `NodeCorrected` the field `was`, and
`Closure` the field `corrected_from`, so the five harnesses of `evolve`'s totality and
preconditions ran, with `version_bumps_o2` (a correction and its withdrawal move no counter),
`take_gate_closed_nodes` (a closed node whose correction a second verdict withdrew is unmarked and
carries an open objection, which the gate refuses), `reaches_terminates` (T5's search, over a graph
from which a checkpoint now removes a wait's edge), and `hold_cmds_preserve_h` (the hold commands,
whose checkpoint now ends a wait): 9 of 9 verified, the five of `evolve` 4 to 7 s each,
`version_bumps_o2` 166 s, `take_gate_closed_nodes` 10 s, `reaches_terminates` 122 s, and
`hold_cmds_preserve_h` 704 s. The seven whose code the final commit changed (`NodeCorrected`'s
`was`) ran again on it: 7 of 7 verified, the five of `evolve` 23 to 33 s, `version_bumps_o2`
586 s, `take_gate_closed_nodes` 14 s (the times include the build). No harness was added or
changed: the group's other changes (a second verdict's validation, a `say`'s addressee, the
checkpoint's wait and pause, the warnings, and the texts) are commands, next calls, and the shell,
outside every harness's reach.

**1.9, group C** (2026-10-04, `cargo kani -p forum-core --harness <name>`, beside the other groups'
checks): `evolve` gained an arm (`SaidWithheld`) and `SaidState` a variant, so the five harnesses of
`evolve`'s totality and preconditions ran, 5 of 5 verified: `evolve_no_panic_sessions` 8 s,
`evolve_no_panic_nodes` 7 s, `evolve_no_panic_holds` 6 s, `evolve_no_panic_entities` 8 s, and
`evolve_counts_anomalies` 11 s. The group's other changes (a `said` withheld at recording, a
replaced `unsent` entry withheld at a release or close, a `say` without `to` addressed to its node's
holders, the forum's tidy items, and the stop words of search) are commands, next calls, and the
shell, outside every harness's reach.

**1.9, the merge of groups A, B, and C** (2026-10-04, `cargo kani -p forum-core --harness
<name>`): `evolve` now carries both group B's classification of a closed node with an open
objection and group C's `SaidWithheld` arm, so the five harnesses of its totality and preconditions
ran again, with `take_gate_closed_nodes`, 6 of 6 verified, 4 to 7 s each.

**1.8, group C** (2026-10-04, `cargo kani -p forum-core --harness <name>`, beside TLC): the
harnesses whose code the group changed, 10 of 10 verified. `take_gate_closed_nodes` (new: a take of
a closed node marked upheld is refused) 5 s, `flagset_ops` (fourteen flags) 7 s, `deadline_order`
(the Stale kind) 20 s, `hold_cmds_preserve_h` (with the upheld input) 359 s, `version_bumps_o2`
340 s, `evolve_counts_anomalies` 21 s, and the four `evolve_no_panic_*` 14 s each. The group's other
core changes (the touch of a node, the correction of a closed node, the intent check, and the
stale key's group shift, which saturates) are outside every harness's reach, as above.

**1.9, group B** (2026-10-04, `cargo kani -p forum-core --harness <name>`, beside TLC): the
harnesses whose code the group changed, 7 of 7 verified. `take_gate_closed_nodes` (with the
objection input: a take of a closed node with an open objection is refused) 5 s,
`hold_cmds_preserve_h` (with the objection input) 620 s, and, since `evolve`'s index refresh runs
the candidate classification that now makes such a node a review item, `evolve_counts_anomalies`
11 s and the four `evolve_no_panic_*` 7 to 8 s each. The group's other core changes (the review
reply of `next`, `guide`'s rules, and `to_user`'s execution) are outside every harness's reach.

**1.10, group B** (2026-10-05, `cargo kani -p forum-core --harness <name>`, beside TLC): the
order of `next` changed, by a review item's hour group (`index::cand_at_with`, which `evolve`'s
index refresh runs) and by the merge's `est` (`query::next`), and `NextKey` did not, so the harness
of its order and those of `evolve`'s totality and preconditions ran, 6 of 6 verified:
`next_key_total` 0.5 s, `evolve_counts_anomalies` 14 s, and the four `evolve_no_panic_*` 8 to 12 s
each. The group's other changes (the next calls of rules 7, 8, and 13, the forum's tidy items,
`why_this`'s `est`, and `health`'s read of the review band) are outside every harness's reach.

**1.11, group B** (2026-10-05, `cargo kani -p forum-core --harness <name>`, one at a time, beside
TLC, the conformance sweep, and the other groups' checks, at a load average of about 20 to 40): the
order of `next` changed again in the hour group (`index::band`: a request band, a needs band, wider
shifts, and a review item's tier without `objection`), `NextKey` did not, `Held` gained
`before_slot`, `evolve`'s refresh now refreshes the series of a refreshed occurrence, and
`close_node` gained step 9, which resolves a duplicate signal whose pair's second node it closes.
So the harness of the order, those of `evolve`'s totality and preconditions, and the harness of
`close_node`'s step 2 ran, 7 of 7 verified: `next_key_total` 12 s, `evolve_no_panic_sessions` 11 s,
`evolve_no_panic_nodes` 8 s, `evolve_no_panic_holds` 11 s, `evolve_no_panic_entities` 11 s,
`evolve_counts_anomalies` 15 s, and `close_ends_holds` 19 s (the times include the build). No
harness was added or changed: the group's other changes (the routing default of `from_user`, the
next calls of rules 5, 8, and 10, the related row, and `why_this.needs`) are commands, next calls,
and the shell, outside every harness's reach.

**1.12, group B** (2026-10-05, `cargo kani -p forum-core --harness <name>`, one at a time, beside
TLC and the other groups' checks): `Node` gained `pr_named` and `Session` `handed_off`, which
`evolve`'s `Appended` arm sets from the `intent_done` text of a checkpoint entry or a `hold check`
(`IntentChecked`) and from a `handoff` release's hold, so the six harnesses of
`crates/forum-core/src/kani/evolve.rs` ran, whose `blank()` node builds the new field, with
`next_key_total` (the order of `next`, whose index keys did not change, though `query::next` now
reads a second merge after the fitting candidates): 7 of 7 verified, `evolve_no_panic_sessions`,
`evolve_no_panic_nodes`, `evolve_no_panic_holds`, and `evolve_no_panic_entities` under 1 s of
verification each, `evolve_counts_anomalies` 3 s, `version_bumps_o2` 238 s, and `next_key_total`
1 s. No harness was added or changed but for `blank()`'s new field: the group's other changes (the
next calls of rules 3, 5, 8, 9, and 13, `next`'s exclusions and its work longer than every horizon,
`leave_better`, and the unreachable reason shown to a session) are queries, next calls, and the
shell, outside every harness's reach.

**1.12, the fixes of its review** (2026-10-05, `cargo kani -p forum-core --harness <name>`, one at
a time, beside the workspace tests on the same laptop): `evolve`'s `Appended` arm now sets
`Node::pr_named` and `Session::handed_off` from fields the entries carry (`EntryBody::Checkpoint`'s
`pr_named` and `handed_off_until`, `IntentChecked`'s `pr_named`), not from their texts, so that a
history written before 1.12 replays to the tables it wrote (D.7.6); `Intent` gained `pr_named`;
and `NodeReopened` clears `pr_named`. So the six harnesses of `crates/forum-core/src/kani/evolve.rs`
ran, with `hold_cmds_preserve_h`, whose checkpoint and release now compute those fields: 7 of 7
verified, `evolve_no_panic_sessions` 9 s, `evolve_no_panic_nodes` 8 s, `evolve_no_panic_holds` 8 s,
`evolve_no_panic_entities` 8 s, `evolve_counts_anomalies` 11 s, `version_bumps_o2` 222 s, and
`hold_cmds_preserve_h` 695 s (the times include the build). No harness was added or changed:
the other fixes (rule 8's merge wait by the open intent, rule 3's due checkpoint `later` beside
rule 8's close, a release's `settles` in reach and in the shell's restore list, G1 (d)'s new
clause, `beyond_horizon`'s `≥k` and its first node, and `no_recipient` for an agent question) are
queries, next calls, warnings, and the shell, outside every harness's reach.

**1.12, final** (2026-10-05): every harness on the final code, 9cb1e41, with `cargo kani -p
forum-cron` and `cargo kani -p forum-core`, beside TLC on the same laptop: 5 of 5 and 32 of 32
verified, 37 of 37 in all. `scripts/verify.sh` had run them once before at d205b62, before the
review's fixes: 37 of 37 verified.

## Traceability

The traceability of the specification to the design, design Appendices A, B, and C, is the coverage
check that nothing in the specification is lost. The design's appendices remain the record: the
first release did not re-check them against 1.5's last rows, which the design's own sections
(D.14.7, D.8.x) describe. Each invariant's row above names its executable check and the tests that
exercise it, which `crates/forum-core/tests/registry.rs` checks exist.

## Known limitations

- **Closed nodes never leave memory.** WP-D8 (D.4.13) is deferred until after the first release, so
  gate G-6 fails at the scale of 10,000 agents; a personal or company forum is not affected.
- **No gate has run on the reference hardware.** Every scale number above comes from a 12-core
  laptop shared with other work, at a smoke scale; G-11's race test is not written (WP-F2 was light),
  and G-3 runs the core's cost tests at their own sizes, not D.12.8's.
- **The model checks are bounded and incomplete.** TLC has been stopped by its time limit every
  time, at depth 6 to 8 at the release; the liveness runs at depth 5; Apalache reaches no step; the
  non-vacuity campaign of the invariant checks has not run.
- **Most core Kani harnesses are narrowed** to the pure helpers they rest on; the end-to-end forms
  are property and unit tests.
- **Usability numbers are one run per cell**: the lab's seven rounds ran on 1.5 to 1.11, and 1.12
  has had no round of its own yet.
- **Usability runs are not all isolated.** Codex runs so far used an unisolated Codex home, whose
  global instructions, skills, and tools reached the agents, and are left out of every count; Claude
  runs made with a claude.ai login carry the login's email into the model's context, which the
  harness now reports as an isolation problem, so they count only once the client signs in with an
  API key.
- **No Verus proof exists**: the TODOs above are tracked, for after the first release.
