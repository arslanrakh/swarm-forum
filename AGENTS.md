# AGENTS.md

Instructions for every agent working in this repository: Claude Code, Codex, and others. This file
is the single source of agent instructions. `CLAUDE.md` is one line, `@AGENTS.md`, which makes
Claude Code read this file; put new instructions here, never in `CLAUDE.md`.

## What this is

Swarm Forum is an MCP server through which a swarm of AI agents coordinates. The specification,
[`docs/protocol.md`](docs/protocol.md) (version 1.12), is the source of truth;
[`docs/design.md`](docs/design.md) is the implementation design, cited as D.x.y in code and comments.
The README says what is built and what is not.

## Decisions already made

Do not reopen these without the user.

- **Pull-only.** The server never initiates contact; agents come to it. While idle, the server may fire
  deadlines that are due; this contacts nobody and yields the same events as firing them in the next call.
- **MCP-only.** Everything the swarm uses to coordinate goes through the server's tools.
  - This includes the directories of people and teams, which agents maintain.
  - Artefacts (code, proofs, data) live outside the forum; the forum holds references to them.
  - Agent tokens are configuration: environment variables or a `.env` file, never forum content.
- **All agents are trusted.**
  - There is no permission model.
  - Everything in a forum is visible to every agent, including colleagues' conversations with their
    agents. This is intended.
  - The server refuses only what would corrupt data, or what a safety valve stops (§19; §21 lists
    every refusal).
  - Everything else it warns about and flags: "the server notices rather than forbids".
  - A pause can be set by anyone; the server makes it visible rather than limiting it.
- **Agents never return, and any session may vanish mid-task.** Every mechanism must survive a session
  disappearing at any moment.
- **Rust.**
  - The core (`forum-core`) is a pure, deterministic function: `settle`, `decide`, `evolve`.
  - The key invariants of the core are checked formally (§24–§28): H1–H6 (holds and fencing), O1 and
    O2 (retries and versions), K1, K3, K4, K5, and T5 (keys, the tree, merges, and waits without
    cycles), T1–T4 (time), and the totality of `evolve`. The rest are checked by executable
    invariants and tests.
  - The core has no IO, no clock reads, no async, no hash-map iteration, and no floating point.
- **Single writer.** One process with SQLite.
  - The state tables are the source of truth. They are updated in the same transaction as the event
    log, and loaded directly at startup.
  - The event log is append-only history. It is not replayed at startup, and there are no state hashes
    or snapshots.
  - Nothing is deleted from the history.
- **Scale.** The design must hold for one user, for many users in a company, for no user at all, and
  for about 10,000 agents on a research programme as hard as the Navier–Stokes problem.
- **Minimal.** No feature without a failure it prevents.

## Layout

| Path | What it is |
|---|---|
| `crates/forum-core` | The pure core: `settle`, `decide`, `evolve`, the executable invariants, Kani harnesses. `no_std` outside tests and Kani |
| `crates/forum-cron` | Calendar, frozen time-zone tables, and 5-field cron for the core; pure, with Kani harnesses |
| `crates/forum-keys` | Versioned key normalisers for the shell; its Unicode crates are pinned to exact versions on purpose |
| `crates/forum-server` | The binary: MCP over streamable HTTP, tokens, configuration, the SQLite store, rendering |
| `crates/forum-conformance` | Test-only: replays Quint traces against the core; `scripts/traces.sh` regenerates its fixtures |
| `crates/forum-sim`, `crates/swarm-sim` | Test-only: the year simulation, and the load driver of the scale gates |
| `model/` | The Quint model of the protocol, in slices; [`model/README.md`](model/README.md) |
| `scripts/verify.sh` | Runs every check and prints which ran, failed, or were skipped |
| `scripts/bench/` | The scale gates G-1 to G-12 |
| `usability/` | The usability test with live agents; [`usability/README.md`](usability/README.md) |
| `docs/verification.md` | What each check found; two blocks are generated |

`forum-server`'s features `usability` and `bench` are for the usability test and the scale gates
only: `usability` compiles only in a debug build, and `bench` only under `--cfg forum_bench`, which
`scripts/bench` sets for its own profile, `bench-gates`. Neither goes into a release binary.

## Building and checking

```sh
cargo build --release -p forum-server       # the server, at target/release/forum-server
cargo test --workspace                      # every Rust test
scripts/verify.sh --no-model --no-kani      # what CI's verify job runs
scripts/verify.sh                           # also Kani, the Quint model, TLC, and the conformance sweep
python3 scripts/gen-verification-table.py   # regenerate the generated blocks of docs/verification.md
```

After any change, run at least `scripts/verify.sh --no-model --no-kani`. After a change to
`forum-core`, `forum-cron`, or `model/`, run the full `scripts/verify.sh`: CI does not run Kani,
TLC, or the conformance sweep. Both forms need Python 3.10 or later; the full run also needs
Node.js with `npm ci` in `model/`, a JDK 17 or later, and `cargo kani`. What is missing is skipped
and named in the summary.
`scripts/verify.sh --help` lists the flags and environment variables.

`scripts/verify.sh` fails when the generated blocks of `docs/verification.md` are stale: regenerate
them after the invariant registry, the model, or a Kani harness changes. After a model change,
`crates/forum-conformance/scripts/traces.sh` regenerates the committed conformance traces. Its
runs are seeded, so a rerun writes the same files.

`clippy.toml` forbids `HashMap`, `HashSet`, clock reads, and `std::env::var` across the workspace,
with the reason for each, and `forum-core` and `forum-cron` deny them outright. Never get around one
with an `allow`. `scripts/verify.sh` also checks `forum-core`'s dependency allow-list and that no
floating-point type appears in `forum-core` or `forum-cron`.

`rust-toolchain.toml` pins Rust. CI reads it, and the Dockerfile's `RUST_VERSION` must equal it (the
image build stops otherwise), so change the two together.

Ask the user before running the scale gates (`--bench`), which need the reference hardware, or the
usability test (`--usability`, `usability/run.sh`), which spends model tokens.

## How to change the protocol

1. **Specification first, code second.** A change in behaviour starts in `docs/protocol.md`. Bump the
   version, and add a row to "What changed".
2. **Change everything that describes the behaviour, together:**
   - the section that describes it;
   - the tool signature (§20);
   - the error and warning table (§21);
   - the entity states (§25);
   - the invariants (§26).

   Every refusal maps to an invariant.
3. **Every invariant has an executable check** (`check_invariants` or `check_step`) that tests
   exercise. **The key invariants** (§28) are also checked in the Quint model and by Kani harnesses;
   a Verus proof of them is a later, optional step. Other invariants get no Quint or Kani work.
4. **Prefer evidence over opinion.** A behavioural change should be backed by a simulation run, a
   model-checker counterexample, or a real incident.
5. **Update the README** when what is built, a command, a setting, or a known limitation changes.

## Conventions

- **Code, docs, and commit messages** are in English: British spelling (-ise), serial comma, sentence
  case in headings.
- **No secrets and no private data** in the repository: no personal names, internal hostnames, or
  company identifiers. The one exception is the copyright line in [`LICENSE`](LICENSE). Examples use
  `forum.example.com`; tokens and `.env` files never enter the repository or an image.
- **Determinism in the core.**
  - Time comes in through the command context.
  - Ids come from counters.
  - Ties are broken by fixed functions (FNV-1a-64, then id).
- **Executable invariants.** `check_invariants(&S, now)` and `check_step(S, cmd, S')` mirror §26. They
  run in full in debug builds, in tests (including replay tests), in `forum-server check`, and in CI. A
  linear subset runs on the state loaded at every startup.
- **Verify after yourself.** Run the checks above before you call a change done, and state which ones
  ran and which the summary skipped.
