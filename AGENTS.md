# AGENTS.md

Instructions for every agent working in this repository: Claude Code, Codex, and others. There is no
`CLAUDE.md`; this file is the single source of agent instructions.

## What this is

Swarm Forum is an MCP server through which a swarm of AI agents coordinates. The specification,
[`docs/protocol.md`](docs/protocol.md) (version 1.12), is the source of truth. The implementation is in progress; the README says what exists.

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
  - The server refuses only what would corrupt data, or what a safety valve stops (§21).
  - Everything else it warns about and flags: "the server notices rather than forbids".
  - A pause can be set by anyone; the server makes it visible rather than limiting it.
- **Agents never return, and any session may vanish mid-task.** Every mechanism must survive a session
  disappearing at any moment.
- **Rust.**
  - The core (`forum-core`) is a pure, deterministic function: `settle`, `decide`, `evolve`.
  - The key invariants of the core are checked formally (§24–§28): H1–H6 (holds and fencing), O1 and O2 (retries and versions), K1, K3, K4, K5, and T5 (keys, the tree, merges, and waits without cycles), T1–T4 (time), and the totality of `evolve`. The rest are
    checked by executable invariants and tests.
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

## Conventions

- **Code, docs, and commit messages** are in English: British spelling (-ise), serial comma, sentence
  case in headings.
- **No secrets and no private data** in the repository: no personal names, internal hostnames, or
  company identifiers. The one exception is the copyright line in [`LICENSE`](LICENSE).
- **Determinism in the core.**
  - Time comes in through the command context.
  - Ids come from counters.
  - Ties are broken by fixed functions (FNV-1a-64, then id).
- **Executable invariants.** `check_invariants(&S, now)` and `check_step(S, cmd, S')` mirror §26. They
  run in full in debug builds, in tests (including replay tests), in `forum-server check`, and in CI. A
  linear subset runs on the state loaded at every startup.
- **Verify after yourself.** Run the model checker, the tests, and the harnesses before you call a
  change done, and state which ones ran.
