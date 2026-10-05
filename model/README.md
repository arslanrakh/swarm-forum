# The Quint model

This directory holds the Quint model of the Swarm Forum protocol, [`docs/protocol.md`](../docs/protocol.md)
version 1.3. Of 1.3's changes the model has §9.2's and §9.4's conversation rules: a `said` answers
only the earlier requests, and fulfils only the open commitments, of its own conversation, and a
merge credits a moved request only with a `said` of its conversation (`Request.conv`, `said`'s
`where`; D.8.8, D.8.15, D.8.20). The next calls of §20.9, which change no state, are not
modelled: D.0.3 dropped WP-A5 (`guide.qnt`), and `check_guide` checks G1 in Rust. Of 1.7's
changes the model has §10.4's `recheck` in the order of `next` (`best_candidate`, `recheck_of`); the
close's default `settles`, a result's routing of the entry it answers, `route {answers |
clarifies}`, `from_user {to: none}`, the `with_reply` mark, and notes on writes touch no key
invariant and are not modelled: every close of the model names its `settles`, which the replay
passes as given. Of 1.8's changes the model has the corrections, intents, and tidying of
"What changed in 1.8": `talk resolve {upheld}` on a closed node, which marks its closure `upheld`
(never offered, taken by `next`, or reopened by `take`) and turns a done into a failure (the model
has no node kinds, so no `refuted`); `hold check {target, intent_done}` (`CCheckIntent`) by any
session, holding nothing; and the `stale` flag of an offer nobody has touched for `OFFER_STALE`,
with its key `K_STALE`, last in its tier in `next` (`Node.touched`, `Node.stale_fired`). Of 1.8's flows and guidance, none is modelled, and none touches a key invariant:
the close's default that settles nothing on a node another session holds (every close of the model
names its `settles`), `to_user {at}` and `no_human_in_reach` (the model's `to_user` names its
principal, and the replay drives no `to_user`), and a `hello` that resumes its session, the
brief's `wait`, the folding of standing next calls, and the next calls' rules 6, 7, 10, 11, and 13,
which change no state. Of 1.9's changes the model has the review of a closed node with an open
objection (§10.1, §10.4): `classify` gives it kind 1, an item with no hold, which `next` gives
without reopening the node, and `v_take` refuses it with `closed`; the `people` and `basis` slices,
where such nodes arise, check H6 too. 1.9's other changes of guidance touch no key invariant and
are not modelled: no next call confirms a `said`, rule 5's one item, rows that name another holder,
a result that routes no user entry, and a result with neither `re` nor `at` that names the requests
on its session's holds (the model's `to_user` routes nothing, and the replay drives no `to_user`).
Of 1.9's words and records the model has one refusal, and it touches no key invariant: a
`from_user` with no principal in a session that serves none is `invalid(NoPrincipal)`
(`v_from_user`, `WhyNoPrincipal`), where 1.6 to 1.8 filed it for `user`; the model's tests pass
`principal: user` where they meant that. The rest changes no state the model holds: warnings
(`client_context`, `unmet_needs`), a close's outcome read by the decoder (the usual words for done),
and texts. Of 1.9's tidying and messages, the model has the two that change state: a `say` without
`to` at a node another session holds is a `holder:` note to those holders (`say_ex`,
`other_holders`), and a `said` by a session that holds neither its conversation node nor work inside
it, while another session does, is recorded `SdWithheld` (`conv_held`), answering, fulfilling, and
opening nothing, which `step_C1`, `step_Settled`, and the merge's credit skip. The forum's tidy
items for idle workers are next calls, which change no state, and `unsent`, whose replaced entries
1.9 withholds, is not modelled. Of 1.10's guidance the model has one change, the order of `next`:
a review item ranks by the ring before age, after the recheck-due nodes of its tier and before the
rest (`Cand.review` in `best_candidate`); the model has no `est` or horizon, so 1.10's `est`, which
puts an unestimated node after the estimated ones under a horizon, is 0 throughout, and its other
guidance (tidy items, rules 7, 8, and 13) changes no state. Of 1.10's semantics the model has the
four that change state: a `said` by a session that holds neither its conversation node nor work
inside it is recorded pending and counts (`said_ex`; `conv_held` is the warning's condition, which
is not rendered); a `say` without `to` at a node no session holds, which can be held, is a `holder:`
note waiting for its next holder, and `bind_holder_notes` and `holder_recipients` leave the sender
out, as the core does since 1.4; a checkpoint ends its node's wait (`checkpoint_ex`), which the
`keys` slice (T5, M1; it now takes made nodes, so that a node its release left waiting is held
again) and the `people` slice (W1, M1) now reach; and a resolve of a resolved objection with the
opposite verdict records a new open objection, withdrawing the correction an upheld verdict made
(`dispute_ex`, the closure's `from_done`). The warnings `no_basis` and `paused`, and the text of
`waiting`, are not modelled. Of 1.11's reviews and conversations the model has the three that change
state it holds: a deferral of a closed node's review records the caller's refusal of the node
(`resolve_ex` with `defer`, `refuse`, shared with a `cannot` release), which `best_candidate`'s
`excluded` reads, and a verdict on a closed node clears its refusals (`verdict_clears`); a `said`
recorded by a session that holds nothing is confirmed at once (`said_start`, `holds_nothing`); and a
`say` without `to` at a closed node reaches the session that closed it while it is live (`closer`,
the closure's `by`). The voice by its cap is the warning's and the next calls' condition, and the
retracted decision a node keeps feeds `no_basis` and the brief: not modelled. Of 1.11's guidance and
routing the model has the four changes that reach state or `next`: a `from_user` with no target,
from a session whose only hold carries an unsettled request of the entry's person, known before the
call, is routed as a clarification of the newest such request (`from_user_ex`; the warning
`follow_up` is not rendered); `next`'s key gains `request` and `needs` (`Cand.request`, `Cand.needs`
in `best_candidate`), and a review item's tier is its node's without `objection`; a series whose
open occurrence waits for its slot is held back (`series_before_slot` in `held_back`); and a close
other than a merge resolves the duplicate signals whose other node is closed and not merged
(`resolve_closed_duplicates`, step 9 of `close_node`, also for a dropped occurrence). Rules 5, 8,
and 10, the related row of rule 5, and `why_this.needs` are next calls and rendering, which change
no state. Of 1.12's guidance the model has the one change that reaches `next`: it keeps from a
session what that session put down, the review of an objection it raised (`own_review`, from the
objection's entry) and the node it last released as `handoff` (`handed_back`, from the session's new
`handed_off`, which `release_ex` sets to the token and `now + lease.first`), until another hold of
the node moves its fence or that time passes. 1.12's work longer than every horizon needs an `est`
and a horizon, which the model has not, and its node field `pr_named` (with an open intent's own
flag, since 1.12's review), its next calls, and the unreachable reason computed for a session
change nothing the model holds. Of 1.12's finishing,
notes, and questions the model has the four that change state it holds: a release with `settles`
records, after itself, the decision `decide` would (`release_ex`, `record_decision`, shared with
`decide_ex`); a retract of an open question to a person withdraws it (`PqWithdrawn`, `retract_ex`),
which resolves the waits on it and drops its recheck, as `inv_E1` (g) checks; `holder:` of a closed
node reaches its live closer (`holder_recipients`); and a reopen tags its node `offer` unless it is
a container (`reopen_ex`). The warnings `no_human_in_reach`, `no_recipient`, `addressee_gone`, and
`answered_nothing`, and the replies' `keyed`, are not modelled. `step_O3` lets the entry of a note
whose `until` has come leave, as `at_event` and the core do. WP-A3 adds the invariants of D.9.2
(`invariants.qnt`), the slices of D.10.7 (`slices/`), and the route that takes them to Apalache and
TLC (`scripts/compile-pruned.js`, `scripts/check.sh`).
[`docs/design.md`](../docs/design.md) D.10 is its plan: the files, the abstract state, the
actions, the time scaling, the instances and slices, and the checks of §28. The model is the second
oracle that the Rust core is compared with: traces of the model are replayed against `forum-core`
(D.10.8).

Run every command below from this directory (`model/`).

## What exists

| File | Package | Contents |
|---|---|---|
| `types.qnt` | WP-A1 (1.3's `Request.conv` and `CSaid.where_key` with WP-A2, D.13.1) | Ids, sum types (keys and names included), the records of the abstract state (D.10.3), the abstract command `Cmd`, outcomes, and `Action` |
| `params.qnt` | WP-A1 | The scaled parameters (D.10.5), the core's constants, and the bounds of D.10.7, as pure values; asserts `MAX_TIME < 60` |
| `basics.qnt` | WP-A1 | Genesis, ancestors, `resolve`, `resolve_merged`, `live`, `waiting`, `paused`, `conv_key`, `conv_node`, the directory of people, the addressee chain with `due_for` and `p90_gap`, the digest audience, the refusal predicates, `stale`, and the FNV-1a-64 `tie_hash` |
| `flags.qnt` | WP-A1 | The flag predicates of D.8.3, one to one, `flags_of`, `CLOSED_TAKE`, and `T0` |
| `deadlines.qnt` | WP-A1 | `deadline_of`, the computed queue and its minimum, every arm of `fire` (D.8.2) with the transitions it needs, and the bounded `settle` with its `saturated` and `stuck` flags |
| `tests/helpers_test.qnt` | WP-A1 | Run tests of the helpers above, the vectors of D.2.6 included |
| `tests/fnv_ts_test.qnt` | WP-A1 | The vectors of D.2.6 at `u64::MAX` through `fnv_id`, `fnv_session`, `tie_hash`, and `le_bytes`, on the TypeScript evaluator |
| `tests/settle_smoke.qnt` | WP-A1 | A smoke spec for Apalache and TLC: a seed with almost every kind of deadline, settled by the two time actions (below) |
| `scripts/verify-file.sh` | WP-A1 | Apalache and TLC on a compiled file, for models too large for `quint verify` (below) |
| `commands.qnt` | WP-A2 | One pure function per command of D.10.4, validation in the order of D.8.24 (where a command's section of D.8 orders two checks otherwise, in that order: item 10 of "Open questions") and then execution with the events of D.8.9 onwards, and `run_cmd`, which is `apply` of D.8.1 for one call (text size, O1, the session, the `late` entry of a stale token, `Touched`, the `op_id` record) |
| `forum.qnt` | WP-A2; WP-A3 | The main module: its constants, the variables of D.10.2, `init` (genesis and the seed of D.10.7), one named action per command of D.10.4 with its picks, the two time actions, the guards of D.10.7, `step`, the reachability witnesses `w_…` of simulation (WP-A2); `inv_all`, every check of D.9.2, the liveness properties of D.10.7, `slice_call` and `slice_hello`, the calls of the slices, and the guards written so that TLC does not branch on them (WP-A3, "TLC and the guards of an action") |
| `instances/two_class.qnt`, `one_class.qnt`, `no_user.qnt` | WP-A2 | The configurations of D.10.1 and D.10.7: two classes with the FNV tie-break, one class (§28), and the forum with no human (CS-2, CS-6) |
| `scripts/simulate.sh` | WP-A2; WP-A3 | Simulation of the instances with `inv_all`, reporting how many runs reach each witness and failing when a required one is never reached; `THREADS` sets the evaluator's threads (WP-A3) |
| `tests/forum_test.qnt` | WP-A2; WP-A4 | Run tests of every command family of D.10.4, each with a long text where the command has one, of O1, H4, presence, the guards, and the seed (WP-A2); `scenario_32`, the example of §32 step by step, and `test_T2`, path independence of settling (WP-A4, below) |
| `invariants.qnt` | WP-A3 | Every item of D.9.2 the model checks, as `inv_X(st, now)` and `step_X(c)` (below, "The invariants"), the replay of a settle firing by firing for the step checks of a time action, `all_invariants` (`inv_all`), the definition `archived` (AR), and the state predicates of the liveness properties |
| `slices/*.qnt` | WP-A3 | The six slices of D.10.7, each with a module per class configuration (`holds_two`, `holds_one`, …, `user_none`), and `slices/common.qnt` (below, "The slices") |
| `tests/invariants_test.qnt` | WP-A3 | Run tests of the checks on steps built by hand: the seed, two conversations on one node (C1, 1.3), a move that leaves an occurrence missed (R2), a refusal question created by an update (U1), and `hello` over aliases; the last three pin the readings of "Open questions", items 17 and 18 |
| `scripts/compile-pruned.js` | WP-A3 | Quint's compiler stages with a pruned, streamed JSON output, which Apalache reads; it replaces `quint compile --target json`, which cannot write this model |
| `scripts/check.sh` | WP-A3 | One check of D.10.7 on a slice module (TLC, Apalache, or liveness) with a time limit, reporting the depth reached; the `tlc:*`, `verify:*`, and `liveness:*` scripts of `package.json` |

The modules import each other in one direction: `types` ← `params` ← `basics` ← `flags`, `deadlines` ←
`commands` ← `invariants` ← `forum` ← `instances/*`, `slices/*` (and `slices/common`).

## Requirements

- Node.js with npm. `npm ci` installs Quint 0.33.0, pinned by `package-lock.json`.
- A JDK 17 or newer for Apalache and TLC (tested with OpenJDK 21). The first `npx quint verify` downloads
  Apalache 0.62.1 into `~/.quint/apalache-dist-0.62.1`; TLC runs from the same JAR.

## The checks that exist now

| Check | Command | Result |
|---|---|---|
| Types, every file | `npm run typecheck` | `quint typecheck` of every `.qnt` file here, one by one (14 files) |
| Every run test | `npm test` | `quint test` of every `tests/*_test.qnt`; a file named `*_ts_test.qnt` runs on the TypeScript evaluator. By default `quint test` runs only the runs whose names end in `Test` and exits 0 having skipped the others (`--match '.*'` runs every definition, helpers included, as a test), so WP-A4's `scenario_32` and `test_T2`, whose names D.10.8 and D.13.2 fix, also run under the aliases `scenario32Test` and `t2Test` |
| The helper tests | `npm run test:helpers`, or `npx quint test tests/helpers_test.qnt` | 79 runs (9 of them from `params.qnt`), about 15 s |
| The `u64::MAX` vectors | `npm run test:fnv`, or `npx quint test --backend typescript tests/fnv_ts_test.qnt` | 2 runs |
| The parameters alone | `npx quint test params.qnt` | 9 runs: `MAX_TIME < 60`, `Params::valid` on the scaled values, the scaling of D.10.5 (durations, counts, and the combination `Params::valid` compares), the seed's series, and the class index |
| The command tests | `npm run test:forum`, or `npx quint test tests/forum_test.qnt` (D.10.7) | 90 runs (9 of them from `params.qnt`), `scenario32Test` and `t2Test` among them, about 75 s; `scenario_32` checks `inv_all`, every check of D.9.2 the model makes, after every call and every tick (WP-A3) |
| The invariant tests | `npm run test:invariants`, or `npx quint test tests/invariants_test.qnt` | 16 runs (9 of them from `params.qnt`), about 5 s after the parse |
| WP-A4's runs by their own names | `npx quint test --match '^(scenario_32\|test_T2\|t2FixturesTest)$' tests/forum_test.qnt` | 3 runs, about 10 s after about 15 s of parsing |
| One test | `npx quint test tests/helpers_test.qnt --match tieHashTest` | |
| Simulation, WP-A2's acceptance | `npm run simulate:1000`, which is `scripts/simulate.sh 1000`: each instance, 40 steps, 1,000 samples, `inv_all`, and the witnesses of `forum.qnt` | no violation in any of the three, and every required witness reached (below, "Simulation coverage", for the counts) |
| Simulation, D.10.7 | `npm run simulate`, `simulate:one_class`, `simulate:no_user`: `scripts/simulate.sh` with 20,000 samples each | WP-A3's acceptance, with its `inv_all` |
| Traces for conformance | `npm run traces`: 300 `--mbt` traces of `two_class`, `traces/t0.itf.json` to `traces/t299.itf.json` | each ITF state carries `mbt::actionTaken` (the leaf action, through the grouped `step`) and `mbt::nondetPicks`; Quint 0.33's placeholder in `--out-itf` is `{seq}` (D.10.7's `t{}` writes `t{}0.itf.json`, …), and the state variables are qualified by module (`two_class::forum::s`, `two_class::forum::clock`; item 12 of "Open questions") |
| TLC on the smoke spec | `scripts/verify-file.sh tlc tests/settle_smoke.qnt --main smoke_tlc --invariant inv_tlc` | 41 distinct states, depth 17, no violation, in under a second after about 50 s of compiling and parsing |

`scripts/verify.sh` runs `npm run typecheck` and `npm run test`; WP-A3 adds the `verify:*`, `tlc:*`,
and `liveness:*` scripts, one per slice module (below, "The checks of D.10.7"), for it to run.

What the helper tests cover:
- FNV-1a-64, `fnv_id`, `fnv_session`, and `tie_hash` against every vector of D.2.6. The three at
  `u64::MAX` hash the eight bytes `0xff` directly in `helpers_test.qnt`, because the default
  evaluator cannot hold `2^64 − 1`; `fnv_ts_test.qnt` runs the functions themselves there;
  `u64_lt` on both halves;
- `MAX_TIME < 60`, `Params::valid`'s orders on the scaled values, every duration and count of the
  table of D.10.5 and the lease terms, and the series of the seeds (a period dividing 60, within the
  bound of 3 occurrences);
- genesis, the tree, `resolve` (a live name before an alias) and aliases, `resolve_merged`,
  containers (persons and teams), `occ_locked`, holds and attendance, waits (`fired` at equality),
  pauses, `conv_key` (object keys skipped, CS-36) and the three cases of `conv_node` (a closed
  carrier in case 1);
- the directory (aliases, `gone`, `rep`, a closed person), owners (the node's own, inherited, through
  teams, ROOT's after them, at most three), the chain with and without an explicit addressee,
  requesters (at most three, `gone` skipped, the node itself first), `p90_gap` (nearest rank, the
  window, merged aliases), `absent`, `due_for` (the p90 raising stage 0 only, at most to `capacity.window`), and every branch of
  the audience;
- the refusal predicates (quorum, backoff, needs-aware active classes, `all_refused`, `covered`, which
  a finished question no longer gives) and `stale` (a closed cited node by its outcome against the
  recorded one, a superseded or retracted decision, a retraction of a merged node or of the node it
  lives on; a merge is not stale; a decision that is not active is never stale);
- the fourteen flags (1.8's `stale` in `staleKeysTest`), each with the node filter of its items and their finished states, and
  `CLOSED_TAKE` and `T0` by equality with D.8.5 and §10.4;
- the kind order of §17 and the owner encoding, the queue, `first_key` against the queue's minimum
  on every fixture and on every state a settle passes through, and every firing arm: lease
  (attempts, the `offer` tag, containers), wait, recheck (next, expiry with marks, which waits a
  recheck resolves, digest, and `assume_now`, escalation), user question (default, re-addressing,
  lapse with marks or `assume_now`, about a reserved or a closed node), commitment, occurrence slot
  and window (missed, a checkpoint, a hold, an open intent, the next slot after `max(slot, close
  time)`, a reslot), refusal expiry of a record and of a class (with `e1_check`, and a class key at
  its earliest window end), due, pause, silence, and retire; two keys at one instant whose kind
  order decides the ids; `NodeCreated` (`due_fired`, the alias at a reused place), `HoldEnded` by
  every end, `assume_one` (never overwritten), `create_uq` (a past due clamped); `close_node`
  (assumptions first, holds, the intent, marks, a series and its occurrence, a signal),
  `refusal_default` (the audience read before the requests settle, only unsettled requests),
  `try_settle` (final), the expiry prune of notes, agent questions, and `op_id` records, and
  `settle` (saturation, the target `max(now, last_now)`, `stuck` with a broken arm, idempotence, and
  T2 on a sample of times).

A mutation campaign checked the suite on 2026-10-02: 153 single-point mutants of `basics.qnt`,
`flags.qnt`, `deadlines.qnt`, and `params.qnt` (the 134 of the review of WP-A1, two of them
re-anchored to the new `settle`, and 19 of the code the review's fixes added), each run against
`tests/helpers_test.qnt`. 152 fail it. The survivor is equivalent within the model: a `{question}`
wait on a people question that is not in the state counts as unresolved instead of resolved, but
people questions never leave memory within MAX_TIME (FINISHED_RETAIN), and a wait can only name one
that exists.

What the command tests cover (`tests/forum_test.qnt`), on the seed of `two_class` with the
tie-break by id:
- the seed, and the wiring of `forum.qnt`: `init`, a command action and a time action through the
  actions themselves, and the guards (settled at `now`, and each bound of D.10.7);
- `hello` (a principal interned first, the `op_id` record naming the new session, a replay, a
  conflict, another class, the notes of its principal delivered); presence (`Touched` only when due,
  for a silent session, or with a hold; an error appends nothing; `retire_at` never decreases); the
  retry store (replay, conflict by command and by session, a retired session's replay, no record of
  an error or of a call that appended nothing, expiry and replacement, the prune); H4 (one `late`
  entry, without metadata, for each write with a token, and none for a long text, a replay, an
  unknown session, or an unknown node); `ClassMismatch`;
- `node`: create (fields, defaults, the hold, `no_hold` with and without the `offer` tag, the
  parent's `needs`), join (added keys, duplicate signals, nothing to add, a join that meets only an
  existing signal and records no `op_id`), `continues` to the newest
  closed carrier, `person:` keys interned in key order, a user key routed, every refusal of D.8.9
  with L1's depth (a series' first occurrence included) and child limits (none under `@people`);
  update (the counters of O2 for content, status, and digest, `MissingDigestVersion`,
  `stale_version`, labels, due, owners on ROOT, every refusal, a patch that changes nothing, the
  occurrence consequences of a changed, cleared, or added `repeat`, `e1_check` on a needs change);
  link (signals, identical, `superseded_by`); key (signal, already carried, `person:`, user,
  refusals); close (the hold, the tag, the version, a series, a waiting occurrence, `settles` with
  its decision, `said` confirmations, refusals); reopen (K7's dropped keys and their signal, a
  series' next occurrence, a merged node, detached occurrences and signals, refusals); move
  (aliases, other sessions' holds ended and offered, a rename in place, cycles, the child and depth
  limits, the directory); merge (keys, requests, people questions, commitments, objections, open
  `said` entries, and marks moved with `moved_from`, finished items left; a moved request credited
  only by a later reply of its own conversation, never a user request, 1.3; signals, cycles in the
  redirected edges, waits and dependencies, intents, refusals); hide;
- `hold`: take (the lease L, an existing hold, a closed node reopened by its flag, every refusal),
  next (caps profiles, the order, routing and status items, a closed flagged node, `own`, refusals,
  needs, pauses, dependencies, the FNV tie-break), checkpoint (intents, the ceiling, attempts), renew
  (A-7, `renew.max`), release (waits of each kind, rechecks, cycles, `cannot` with the quorum and
  E1);
- `talk`: from_user (principals, routing, answers with their sources, precedence, refusals), to_user
  (only a `result` to the same person answers, aliases, digest entries per person), route, ask user
  (the chain, asking once), request (Q1, `where`, the conversation key it keeps, the three cases of
  `conv_node`), said (commitments, answers, settling, the reply's settled requests, the
  conversation key and `where`; 1.3: on a node with two conversations a reply answers and fulfils
  only its own conversation's, an ack refreshes only its own, a request whose conversation is a
  message id is answered only with that `where`, and a reply fulfils no commitment of another
  node), confirm, ask person (asking once, `again`), heard (an open, expired, or escalated question
  answered, a lapse's marks cleared), ask agent and answer, say and wait (each addressee, delivery once, `holders:`
  cursors), decide (the basis, supersession, assumptions, a signal closed), retract, approve,
  object, resolve;
- `control` and `view`: pause, renewal, unpause; `Shown` only when it raises `seen`;
- 1.8: an upheld objection on a closed node (`upheldTest`: the outcome corrected to `failed`, the
  closure marked, `take` refused with `closed` and `next` passing it by, a reopen clearing the
  mark); `hold check {target, intent_done}` without a hold (`checkIntentTest`); and the `stale`
  flag of an untouched offer, set by its key, last in its tier in `next`, and cleared by a note
  (`staleTest`);
- 1.10: a review item before an open flagged node of its tier, whatever their ids and ring
  positions, until its objection is resolved (`reviewOrderTest`); a `said` recorded while another
  session holds the conversation counts (`heldSaidTest`, 1.9's `withheldTest`); a second, opposite verdict opens a new objection, the node a review again, and
  withdraws an upheld verdict's correction, `done` again and unmarked (`disputeTest`); a checkpoint
  ends its node's wait, and T5's edge with it, while a renew does not (`checkpointWaitTest`); and a
  `say` without `to` at a node nobody holds waits for its next holder other than the sender
  (`sayWaitTest`);
- 1.11: a deferral backs its class off a closed node's review, which another class is still
  offered, until the backoff ends or a verdict clears it, records nothing for a resolved objection,
  and is refused with `upheld` (`deferTest`); a `said` by a session that holds nothing is confirmed
  as it is recorded, one by a holder stays pending (`saidAloneTest`); a note at a closed node
  reaches its live closer, nobody once it is silent, and never the closer itself
  (`closerNoteTest`); `reviewOrderTest` with a review in tier 2, before a plain offer and after an
  open node's objection; work that needs the caller's caps before work that needs nothing
  (`needsOrderTest`); a request before an open intent in tier 0 (`requestOrderTest`); a series held
  back while its occurrence waits (`seriesBeforeSlotTest`); a duplicate signal resolved when both
  its nodes close, a conflict signal not (`duplicateBothClosedTest`); and a follow-up clarifying its
  person's request on the only hold (`followUpTest`);
- 1.12: a `handoff` release keeps its node from the releaser for `lease.first` or until another hold
  of it, while another session gets it at once, and a blocked release keeps nothing (`handoffTest`);
  `reviewOrderTest` and `deferTest` with the review of an objection never given to the session that
  raised it; a release with `settles` records its decision after its checkpoint, settles a listed
  request with its answer, keeps one without listed until its answer comes, records none without
  `settles`, and is `not_found` for a request that does not exist, after the token
  (`releaseSettlesTest`); a retract of an open question to a person withdraws it, resolves the wait
  on it, drops its recheck and marks, and nothing escalates it, while a retract of its `said`, or of
  an answered question, withdraws nothing (`withdrawTest`); `holder:` of a closed node reaches its
  live closer, nobody once it is silent, and never the closer itself (`holderClosedTest`); and a
  reopen tags a plain node `offer`, a series none (`reopenOfferTest`); after the merge of 1.12's
  groups, a `handoff` release with `settles` both records its decision and keeps its node from its
  releaser (`handoffSettlesTest`);
- three readings recorded for a decision (below, "Open questions", items 7, 8, and 10).

A mutation check on 2026-10-02: 30 single-point mutants of `commands.qnt` (presence, the retry
store, the `late` entry, the lease and its extension, the order of `take`'s checks, `next`'s
classification and exclusions, create's needs and predecessors, update's counters, close, release,
`from_user`, `to_user`, `said`, merge, reopen, move, ask, decide, the key order, `answer_pq`, the
inbox), each run against `tests/forum_test.qnt`: all 30 fail it, four of them after the tests
`takeOrderTest`, `nextClosedCapacityTest`, `saidUserRequestTest`, and `answerClearsMarksTest` were
added for them. Beyond `inv_all`, a scratch module (not kept) ran K3, K5, H5, T3's markers, T5, W1,
E1 (a), Settled, O1's and the errors' step halves, every entity's references, no assume mark on a
closed node, and `next`'s hold in simulation: 4,000 samples of 40 steps with two seeds in each of
the three configurations, no violation.

A second mutation check on 2026-10-02, after the review of WP-A2: 24 single-point mutants of
`commands.qnt` and `forum.qnt`, 12 of them the review's (a reply fulfilling commitments on any node
or of any conversation; a merge crediting a user request, an older reply, or a reply of another
conversation, or not moving commitments, `said` entries, questions, objections, or marks; `heard`
not answering an expired question; a lapse's marks kept; an ack refreshing other conversations'
commitments; a reply not settling) and 12 of the 1.3 code and the fixes (the request's
conversation key, a routed user request's, `where` ignored or a `repeat:` `where` accepted, the
reply's `settled` ids, the join's event check, the class of a `hello`). Each fails
`tests/forum_test.qnt`; before the fixes, 11 of the review's 12 passed it.

## The §32 scenario and T2 (WP-A4)

`scenario_32` plays the example of §32 as D.12.2 scripts it for the core, step by step, on the seed
of `two_class` (tie-break by id). The script is data, a list of `Step32` (a call, with the reply it
must get, or a clock advance), in named parts (`PARTS_32`) that follow D.12.2's steps, so that the
run and `test_T2` share it. The run plays each part through actions (`part32`) and then checks
D.12.2's assertions for that part. After every call and every tick it checks `checked32`: `inv_all`,
every check of D.9.2 that the model makes (WP-A3), and `recorded_pending` (a call that records a
`said` leaves it pending, §9.2, so that each later confirm is a checked change). Every call comes
from a session that has never fallen silent ("no session came back"), and at the end of every part
the run is where the pure fold of the script (`WORLDS_32`) is. The comment above `Call32` in
`tests/forum_test.qnt` gives the mapping in full; in short:

- The example's nine sessions are new sessions #3 to #11, in the order of the example (the
  assistant, s-1 to s-7, the coding agent), of the classes `a` (chat-assistant), `b`
  (chat-worker), and `w` (coding-agent).
- Seed session 1 first closes the seed's /w1 and /w2, so that §32's two top-level nodes fit under
  ROOT beside /s within the scaled `tree.max_children` (3). /s stays open, and quiet: its first slot
  lies after the example's last call.
- Three calls of §32 are two calls each in the model, whose commands lack a field for them:
  `hello {from_user}` is `hello` and `talk from_user`; `node create {path: digest/daily-tickets}`
  creates `digest` first (the missing ancestor); and `node create {request}` is `create` and
  `talk request`, with `where` the thread's key, the default of §20.5 and D.8.9 (A-61). The
  model's `create` has no `request`, so the script passes that default instead of the model
  computing it: step 2's `conv: THREAD` holds by construction, and the default is left to WP-D6's
  core scenario. `bare_request_case_3` shows that the split needs the explicit `where`: without
  it, `talk request` takes case 3 and makes the message a key of summary-gap (§9.4).
- The thread and the person's message are `Conv("t1")` and `Msg(("t1", 1))`. The daily schedule
  is a period of 30 ticks in UTC with `run_within` 2 h (7 ticks), whose first slot lies after the
  example's end, as D.12.2's occurrence `20261002T0500Z` waits throughout.
- Steps 1 to 4 run at tick 0. "Advance 2 h" is 7 ticks (the 2 h of D.10.5: after
  `commitment.followup`, before `question.recheck`), and "advance 21 min" is 4 ticks: the lease
  ends at `lease.default` (4 ticks) after the checkpoint, and no tick lies between 20 min and 1 h.
  Each of the two keeps the order of its own duration, as D.10.5 does, but their sum does not keep
  its order: 7 + 4 = 11 ticks is past `session.retain` (10 ticks, 24 h), where D.12.2's run on the
  real clock, 09:00 to 11:21, is far below it. So the six sessions last touched at tick 0 (the
  seed's two, the assistant, s-1, s-2, and s-3) retire at tick 10, within the second advance, and
  only s-4 and s-5 are left when s-6 arrives, where in D.12.2's run all nine sessions are present,
  silent, from step 6 on. No assertion of D.12.2 reads `sessions`; `after_6_vanish` asserts the
  retirement, so that a change in it is seen. A comparison of this run with WP-D6's run on real
  params must leave `sessions` out from step 6 on; D.10.8's replay, on the scaled params, retires
  them as the model does ("Open questions", item 15).
- The run plays each advance one tick at a time (`in_ticks`), so that `checked32` sees the settled
  state at every tick: in the first advance the series' lease ends and six sessions fall silent at
  3, and the ack's follow-up fires at 6; in the second, two sessions fall silent and six retire at
  10, and s-5's lease ends at 11. The fold takes each advance as one jump, so `part32`'s check at
  the end of those two parts, that the run is where the fold is, is an instance of T2. The states
  between two firings due at the same tick are not settled, so no state invariant applies to
  them; D.12.2's check "after every firing" is Rust's `check_fire`, a step check.
- Warnings, the join's `open_question`, `view requests`, and `leave_better` are not in the model's
  replies, so their assertions read the state they report: `close_clean` checks the conditions of
  the four warnings of a close (§21, G1 (d)); `moved_of` and `conversation_held_of` compute the
  `moved` and `conversation_held` warnings (D.8.15) on the state before s-3's ask, whose `moved`
  lists r2 and the ack and which gets no `conversation_held` (s-2, holding nothing, would get it
  there), and a read of summary-gap first, as in D.12.2's guided run, moves `shown` and leaves
  `moved` empty; and `leave_better_of` computes D.8.7's anchored `leave_better` from the state, so
  that step 7 asserts `[summary-gap]` (A-63).
- Step 6's ranking. Within MAX_TIME the model's `aged` is 0 (D.10.5), so `after_6_arrive` checks
  only that summary-gap and rerun-test are both tier-0 candidates and that the tie-break puts
  summary-gap first, by id and by FNV alike. D.12.2's reason, that summary-gap is older (Rust's
  `aged` is 2 against 0 at 11:21), is left to WP-D6's run on the real clock.
- Step 7's `ready_to_close` is checked as a change: absent after rerun-test closes while audit is
  open (`after_6_finish`), present after audit closes (`after_7_audit`).
- The next calls of 1.3 (D.12.2, "Next calls") are not asserted: the model has no next calls
  (D.0.3 dropped WP-A5); the core's `tests/scenario_32.rs` checks them with `check_guide`.

The calls go through `call32` and `tick32`, which are `command` and `act_tick` of `forum.qnt`
without D.10.7's bounds, with the reply checked: the example opens nine sessions (the bounds allow
four), records three user entries (two), and with its setup makes 24 entries (MAX_EID is 16). The
bounds keep model checking finite, and a fixed script needs none. The run's ticks are of one tick,
within `act_tick`'s picks (1 to 3). `last_action` records each call as `command` does, with the
names of D.10.4 ("Open questions", item 13).

What the run checks and does not check:
- Since WP-A3, `inv_all` is every check of D.9.2 that the model makes ("The invariants"): the
  state checks, the step checks (after a tick, replayed firing by firing, D.9.2's `check_fire`),
  and T2 over pairs of ticks up to 12 ahead. `scenario_32` passed with it, so D.12.2's
  "`check_invariants` holds after every step" holds here for every item of §26 but R1, Total, K2,
  and G1, which the model does not check (D.9.2), and M1b, which is not an invariant ("Open
  questions", item 16).
- The check at the end of each part, that the run is where the fold is, ties `WORLDS_32`, which
  `test_T2` reads, to the run. For a call it cannot fail, because `play_pure` evaluates what
  `call32` does on the same arguments; only for an advance does it compare two paths (the ticks
  and the jump). It is not D.12.2's replay of the event log from genesis through `evolve`, which
  the model has no counterpart for; that belongs to WP-D6, WP-E1, and WP-F2.

Result: `scenario_32` passed on its first run. Every call got the reply that the specification
gives it, and every assertion of D.12.2 held, so the scenario exposed no defect in `commands.qnt`,
which is unchanged. The checks the review of WP-A4 added (above) pass as well.

`test_T2` checks T2 (§26, D.8.2, D.9.2): `run(run(S, t1), t2) == run(S, t2)` on the whole state,
`last_now` included, with `run` the bounded settle of D.10.4. It does so on the 50 states of the
pure fold of the §32 script (`WORLDS_32`, clocks 0, 7, and 11, each advance one jump) and on three
fixtures built by commands from the seed:
`T2_QUESTIONS` (time and external waits, a user question re-addressed and lapsed, a default, a
commitment's follow-up, a due, a pause, a blocking question rechecked and escalated, the
escalation's lapse, an unconfirmed `said`, and two missed occurrences), `T2_REFUSALS` (two refusals at
the quorum, the refusal question's default, a refusal record and a class profile expiring, and a
non-blocking question about a closed node expiring into `assume_now`), and `T2_PRUNE` (an `op_id`
record, a note, and an agent question that expire at a time when nothing fires). On each state, it
checks:
- every pair `t1 <= t2` of the times at which a key fires, from the clock to MAX_TIME. While a
  firing depends only on the state and its own key (lemma L1 of D.8.2), `run(S, t)` is the same
  from one firing time to the next, and these times stand for every time in between;
- every pair of ticks within T2_K (12) of the clock, D.10.7's `inv_T2` as written, which does not
  rest on L1;
- firing one key at a time up to MAX_TIME (the idle writer of CS-30) ends in the state one settle
  reaches;
- no settle saturated or stuck.

Between them the firings cover every deadline kind of §17, and a refusal expiry of both owners (a
record and a class). `t2FixturesTest` checks that the fixtures reach what they are built for; it
reads an outcome with `closed_as`, which an open node never passes (its predecessor read an open
node as `missed`). The run takes about 8 s.

Mutation checks on 2026-10-02, each on a scratch copy of the model:
- T2: a firing stamped with the settle's target instead of its own time, `op_id` records pruned
  at the target, notes pruned at the target, and a key due exactly at the target left unfired: each
  fails `test_T2`. The two prunes are the reason for `T2_PRUNE`: in the other states, a record or
  a note expires exactly when its session retires, which hides a prune at the wrong time, and the
  mutant that prunes records at the target passed `test_T2` without `T2_PRUNE`. It also passes
  the sample of firing times, because a prune at the target breaks lemma L1, on which the sample
  rests; the pairs of ticks catch it. A recheck rescheduled from a value of the state other than
  its own time passed, correctly: it is a function of the state and the key, so T2 still holds.
- `scenario_32`: a release that does not confirm the session's `said` entries, an `intent_done`
  that closes only the caller's own intent, a question recorded on `at` instead of the
  conversation node, a report that does not answer a request through its entry, a question that
  does not suppress `open_commitment`, a join that issues a hold, and a close that keeps the
  `offer` tag: each fails `scenario_32` (the last since the final assertion that no node carries
  `offer` was added for it). An expiry that does not tag the node `offer` passes: the only
  expiries in §32 are on rerun-test, which was created with the tag, and on the series, which never
  gets it.
- After the review of WP-A4, each of these passed the earlier `scenario_32` or `t2FixturesTest`
  and fails the present one: `ready_to_close` set once any child is closed (`after_6_finish`); a
  `said` recorded confirmed (`recorded_pending`, at step 3's `said`); a join that marks the joiner's
  `shown` (`after_2_second`, which compares the world with the one after the first run); a `shown`
  that never moves (`after_4_take`, through the guided read); `session.retain` at 12 ticks
  (`after_6_vanish`); and an occurrence that is never closed as missed (`t2FixturesTest`, through
  `closed_as`).

## The checks of D.10.7 (WP-A3)

These are the checks D.10.7 specifies, with the commands that run them on this model. `<module>` is
a slice in one class configuration: `holds_two`, `holds_one`, `keys_two`, `keys_one`, `people_two`,
`people_one`, `user_two`, `user_one`, `user_none`, `recurring_two`, `recurring_one`, `basis_two`, and
`basis_one` ("The slices", below).

| Check | Command |
|---|---|
| Unit and scenario runs | `npm test`; `npx quint test tests/forum_test.qnt` (`npm run test:forum`) runs `scenario_32` and `test_T2` under their aliases `scenario32Test` and `t2Test`, and `scenario_32` checks `inv_all` after every call and every tick |
| Simulation | `npm run simulate` (`two_class`), `npm run simulate:no_user`, `npm run simulate:one_class`: `scripts/simulate.sh 20000 <instance>`, D.10.7's `npx quint run instances/<instance>.qnt --max-steps 40 --max-samples 20000 --invariant inv_all` with the witnesses |
| Traces for conformance | `npm run traces`: `npx quint run instances/two_class.qnt --mbt --max-steps 40 --n-traces 300 --out-itf 'traces/t{seq}.itf.json'` (D.10.7 writes `t{}`; "Deviations") |
| Bounded model checking (Apalache) | `npm run verify:<module>`: `scripts/check.sh apalache <slice> <module>`, depth 12, with `inv_<x>_apalache` where the slice defines it (T2 over fewer ticks, D.10.7) and `inv_<x>` otherwise |
| Exhaustive (TLC) | `npm run tlc:<module>`: `scripts/check.sh tlc <slice> <module>`, with `inv_<x>` |
| Liveness (TLC) | `npm run liveness:<module>` for the seven modules of `holds`, `recurring`, and `user` (`user_none` included): `scripts/check.sh liveness <slice> <module>`, with `liveness_<x>` |

`scripts/check.sh` compiles a module once into `traces/<module>/` (about 10 minutes, below) and runs
the check under a time limit, `MODEL_CHECK_SECONDS` (1,800 by default). A check that the limit stops
reports `INCOMPLETE` with the depth it reached and its state count, and exits 0 unless
`MODEL_CHECK_STRICT=1`; the log is `traces/<module>/<check>.log`. Runs may overlap: a module's
compilation and each of its checks take a lock (`traces/<module>/<compile|check>.lock`, taken over
when its owner has died), and each TLC run writes a `.cfg` of its own. `traces/` is git-ignored.

@@CHECK_RESULTS@@

### Apalache and TLC on a model this size

`quint verify` cannot check this model with Quint 0.33.0. It hands the whole compiled model to the
Apalache server as one JSON string, and the server refuses strings over 20 MB (`Invalid strict JSON:
String value length … exceeds the maximum allowed (20000000…)`); `quint verify --backend=tlc` takes
the same route, because Quint asks the Apalache server for the TLA+ that TLC checks. Nor can
`quint compile --target json` write a slice module: its output passes Node's largest string (below,
"The size of the compiled main module"). `scripts/verify-file.sh` therefore compiles the module to a
file with `scripts/compile-pruned.js` and runs the tools on the file, and `scripts/check.sh` runs
one check of D.10.7 on a slice module through it (the `verify:*`, `tlc:*`, and `liveness:*` scripts):

```sh
# Apalache, bounded (D.10.7: depth 12); TLC, exhaustive within the guards; TLC, liveness (§27)
scripts/check.sh apalache <slice> <module>
scripts/check.sh tlc <slice> <module>
scripts/check.sh liveness <slice> <module>

# the same by hand, for any module and invariant
scripts/verify-file.sh apalache slices/<x>.qnt --main <module> --step step_<x> --invariant inv_<x> --length 12
scripts/verify-file.sh tlc slices/<x>.qnt --main <module> --step step_<x> --invariant inv_<x>
scripts/verify-file.sh tlc slices/<x>.qnt --main <module> --step step_<x> --temporal liveness_<x>
```

What `verify-file.sh` does, step by step:
1. `compile-pruned.js` runs Quint's own stages (load, parse, typecheck, compile with `q::init` and
   `q::step`) and writes `traces/<module>/<module>.qnt.json` piece by piece, with only what
   Apalache reads: the flattened main module, the types of the expressions that occur in it, and
   the lookup entries of the names that occur in it (kind, id, and name). One compilation holds
   every definition of the main module, so it serves every invariant and property of a slice.
2. Apalache: `apalache-mc check --init=q::init --next=q::step --inv=<names> --length=<n>
   --no-deadlock` reads the `.qnt.json` file directly. A counterexample is
   `traces/<module>/apalache/…/violation1.itf.json`.
3. TLC: `apalache-mc parse --output=<module>.tla` writes the TLA+; the script rewrites the
   assignments `v' := e` of the init action (the operator `q_init` names, which carries a prefix in
   a module that instantiates another, as the slices and instances of D.10.1 do) into equalities
   `v = e` (what `quint verify --backend=tlc` does before it asks for TLA+), so the init action must
   contain its assignments itself; it writes a `.cfg` with the invariants or properties by their
   names in the TLA+, and runs `java -cp apalache.jar tlc2.TLC -deadlock -workers <n> -config
   <module>.cfg` from the JAR `quint verify` uses, without deadlock checking (D.10.7: a run that
   reaches the bounds stutters there), with its states in a directory of its own.

`--reuse` takes the JSON and the TLA+ of an earlier run with the same `--out`, `--main`, `--init`,
and `--step`; `check.sh` compiles a module once and reuses it until a file the slices read changes
(the modules in this directory and `slices/`). The exit status is 0 when the property holds within
the bounds, TLC's or Apalache's own code otherwise (12 for a violated invariant, 13 for a violated
temporal property), and 128 + the signal's number when a signal stopped it (below).
`APALACHE_HOME`, `JVM_ARGS`, `TLC_JVM_ARGS`, and `TLC_WORKERS` override the Apalache directory, the
JVM options (8 GB heap by default), and TLC's workers (one per core by default).

Names in an instantiating module. Apalache gives every definition of an instantiated module the same
prefix, `let` definitions inside operators included, so a local `val` with the name of a state
variable (`val stuck = …` in a helper, and `var stuck` in the spec) is a "multiply-defined symbol"
for TLC. Helpers must not name their locals after the variables of `forum.qnt` and the slices (`s`,
`clock`, `prev`, `mid`, `last_action`, and any a slice adds). TLC's integers are 32-bit, so no
literal of the model may reach 2^31 (`FNV_OFFSET` is written as its four 16-bit limbs).

The route was checked on 2026-10-02 with `tests/settle_smoke.qnt`, a seed state with holds,
sessions, a blocking people question with a waiting node, a series and its occurrence, a refusal,
and a class profile, settled by `tick` and `tick_next`: TLC explored all 41 distinct states (depth
17) against `inv_tlc` (no settle stuck or saturated, settled at `clock`, `first_key` equal to the
queue's minimum, at most 3 occurrences, no hold on a closed node, and `flags_of` on every node)
without a violation. On small specs the script returned 0 for a true invariant, 12 for a false one
from TLC and from Apalache (with `violation1.itf.json` written), and 142 when the alarm below
stopped TLC or Apalache, with no JVM left afterwards.

### TLC and the guards of an action

TLC computes the successors of a state by walking the action's formula, and it expands every
definition the action uses as it goes: a disjunction reached that way is not evaluated to a boolean
but explored, each true disjunct as a branch of its own that continues with the rest of the
conjunction. Two true disjuncts give the same successor twice. `forum.qnt`'s `command` guards each
call with `within_bounds`, whose bounds were each `le_bound(pre, post, bound) = post <= bound or
post <= pre` (seven over the whole state, five per node), and for most calls both disjuncts of most
of them are true. Measured on 2026-10-02 with the `holds` slice: TLC generated 4.1 million successor
states in 6 minutes and found 3 distinct ones (`holds_two`; `holds_one` 2.1 million and 2), never
finishing the successors of the seed state, which has about 300; a probe whose next-state relation
is a single `hello`, which has two successors, generated 10,181 states in a minute. `le_bound` is
now `post <= imax(bound, pre)`, the same predicate as one comparison: the `hello` probe ends with 3
states generated and 3 distinct, and the whole `holds_one` step explores about 1,850 distinct states
a minute on two workers, `inv_holds` (with T2 over 12 ticks) checked on each. A guard that an action
reaches must not be an `or` or an `exists` whose branches can both hold: TLC branches on them where
simulation and Apalache do not. `settled`'s `k.at < 0 or k.at > now` is the one disjunction left on
the path, and its disjuncts exclude each other.

### The size of the compiled main module

`quint compile --target json` of `instances/one_class.qnt` fails after about 14 minutes with
`RangeError: Invalid string length` (Node's largest string, about 512 MB, while `JSON.stringify`
writes the whole output at once), and so does a slice that imports `forum`. Quint keeps every
imported definition, reached or not, every expression carries its type (an expression of type
`State` carries the whole 32-field record type, up to 244 KB), and the lookup table repeats each
definition it points to. `compile-pruned.js` writes what Apalache reads and nothing else, one entry
at a time: a slice module compiles to about 530 MB (`holds_two`: 1,093 declarations, 33,088 of
102,934 types, 12,413 of 38,290 lookup entries, 534 MB) in about 10 minutes with three compilations
side by side, and its TLA+ is about 12.7 MB. This settles "Open questions", item 9, without changing
D.10.1's layout.

### Cost of settling in Apalache

Measured on 2026-10-02 (Apalache 0.62.1, 12 cores, 36 GB). The first two rows used a scratch spec
of the first version of WP-A1 (the seed of `tests/settle_smoke.qnt` with no holds, a series of
period 10, and `first_key` as the minimum of the whole queue); the last row is `smoke_apalache` of
`tests/settle_smoke.qnt`, with `first_key` as one fold per owner kind. Each step is `tick` or
`tick_next` settling the seed state; the invariant is `inv_smoke`.

| Spec | `max_firings` per settle | Heap | Preprocessing | Step 0 | Result |
|---|---|---|---|---|---|
| scratch | 6 | 8 GB | about 2 min | not encoded in 28 min | out of heap memory at 29 min 41 s |
| scratch | 1 | 10 GB | about 9 min (CPU shared with a simulation) | the first transition (`tick`) took 17.5 min to encode and decide | the invariants held in state 1; the second transition (`tick_next`) was still being decided after 47 min |
| `smoke_apalache` | 1 | 10 GB | about 2.5 min (464 MB of JSON) | the first transition (`tick`) was encoded and handed to Z3, which ran out of its memory 20.5 min later (CPU shared with a 6-worker mutation campaign) | the invariants held in state 0; no verdict for length 1 (exit 255 after 23 min) |

The second row was not stopped at its 40-minute limit, as WP-A1's first report said: the earlier
`verify-file.sh` ran Apalache in the foreground, so the alarm ended the shell and left the JVM
running. It was still running 1 h 12 min after its start, with no verdict; the script now passes
the signal on (below).

So one firing of `fire` over the full state costs Apalache minutes, and a settle of several firings
does not fit in 8 GB: Apalache encodes every arm of `fire` for every firing, each building a whole
state. TLC has no such cost (41 states in under a second), and simulation neither. Computing
`first_key` without building the queue did not change that: the seed of `smoke_apalache` has two
holds more than the scratch spec's, so the rows do not compare exactly, but one firing still did not
fit.

Apalache at depth 12 per slice (D.10.7) will need slices whose step settles with few firings, a
smaller state, or a `fire` restricted to the deadline kinds the slice can create; if none of these
is enough, it is decided whether Apalache runs at a smaller depth, with TLC as the exhaustive
check (below, "Open questions").

Since every command action ends with D.8.1's post-settle, as the core's calls do, a slice's step
encodes one settle fold per action (twelve commands and two time actions in `holds`), where it
encoded one per time action before. TLC and simulation pay only for the firings that happen: a
post-settle that finds nothing due reads the queue's minimum twice and stops.

### Long runs

Run model checkers in the background with a time limit, and record what completed, the depth
reached, and any counterexample. macOS has no `timeout`; `perl` gives the same, and a run stopped
by it exits with 142. `verify-file.sh` runs each long step (the compiler, Apalache, TLC) in the
background and waits for it, and passes a SIGALRM, SIGTERM, SIGINT, or SIGHUP on to it as SIGTERM,
so the JVM ends with the script; `pgrep -f apalache.jar` afterwards shows that nothing is left:

```sh
mkdir -p traces
perl -e 'alarm 1800; exec @ARGV' scripts/verify-file.sh apalache slices/holds.qnt \
  --main holds_two --invariant inv_holds --length 12 > traces/holds_two.log 2>&1 &
```

Apalache logs each step it reaches (`Step <k>: picking a transition`, `State <k>: state invariant
<i> holds`), which gives the depth a stopped run reached.

## The invariants (WP-A3)

`invariants.qnt` writes every item of D.9.2 that the model can check as a predicate, from §26 and
D.9.2, over the base maps:

- a state invariant is `inv_X(st, now)`, over a state settled at `now`, the model's `clock`;
- a step invariant is `step_X(c)`, over one step `c` of an action: the state before and after it,
  its `now` (`max(clock, prev.last_now)` for a command and its post-settle, the clock it settled to
  for a time action), and the action. Every action ends with a settle, as every call of D.8.1 does:
  a command action runs its command and then the post-settle at its `now`, and a time action
  settles to its clock. `steps_of(prev, mid, s, clock, last_action, MAX_FIRINGS)` splits the action
  into the command's own step, from `prev` to `mid` (one `check_step`), and the settle, from `mid`
  to `s`, which it replays firing by firing (the queue's minimum by the queue's definition,
  `queue_min`, not `settle`'s `first_key`), so that the `check_fire` clauses of D.9.2 apply to each
  firing with its own before and after; `step_T4` checks that the replay ends where the action's
  settle ended. `each_step` applies a step invariant to both steps; a time action has no command
  step (`mid` is `prev`);
- `all_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, T2_K)` is every check, and
  `forum.qnt`'s `inv_all` (`inv_all_but_T2` leaves T2 out). The slices check their parts.

Where D.9.1 asks for a naive definition apart from the code it checks, `invariants.qnt` writes it
again (`up_path`, `conv_node_naive`, `default_conv_naive`, `resolved_naive`, `paused_naive`,
`reach_naive`), so that a wrong helper of `basics.qnt` or `commands.qnt` is caught; it reuses the
model's definitions where D.9.2 names them (the flags of `flags.qnt`, `chain`, `audience`, the queue,
`classify`).

| §26 | Quint | What the model checks | Slices |
|---|---|---|---|
| H1 | `inv_H1` | live holds per node within capacity | holds |
| H2 | `inv_H2`, `step_H2` | live holds per session within `hold.per_session`; every taker exists and retires after the hold's ceiling; a new hold's session was touched (`last_seen = now`); a firing retires only a session that holds nothing | holds |
| H3 | `inv_H3`, `step_H3` | every hold's node exists and its token's fence is at most the node's; a fence rises by one per hold issued, the new token carries it, and no firing issues a hold | holds |
| H4 | `step_H4` | a write with a token that is not a live hold on its target: `stale_token`, and the state is the one before plus one `late` entry (one eid, one seq), the time, the expiry prune, and the `op_id` record; a long text, a live record, an unknown session, or an unknown target changes nothing | holds |
| H5 | `inv_H5` | the three clauses of D.9.2 with each hold's terms, and the terms are the scaled ones (the step half) | holds |
| H6 | `inv_H6` | no hold on a closed node | holds, people, basis (1.9: where closed nodes carry objections) |
| O1 | `inv_O1`, `step_O1` | every record expires after `last_now` and within `op.retain` of it; a live record: a replay exactly for the same command from its session (or a `hello`), a conflict otherwise, nothing changed; a call that changed the state records `{command, session, now + op.retain}` and keeps the other live records; a firing only drops records at their expiry | holds |
| O2 | `inv_O2`, `step_O2` | per node: kept entries' seqs within the node's, distinct; versions at least 1; per command and per firing: counters never fall, `version` moves exactly when content, status, keys, links, or open/closed changed, and by at most one per event that can move it (`version_events`: a key or a link that came or went, one update of content and status, one close or reopen), `digest_version` by one exactly when the digest changed, `content_version` is the version of the last content change, new kept entries take seqs the step added; a new node starts at version 1; `stale_version` and `MissingDigestVersion` change nothing, and a digest changes only by an `update` of its node that carries the node's `digest_version` from before the step | holds, keys |
| O3 | `step_O3` | every entry kept unchanged, new entries' eids allocated by the step, `retracted` only grows | every slice |
| R1, Total | none | the model has no indices and no `evolve` (D.9.2) | |
| K1 | `inv_K1` | an open node's keys are carried by no other open node | keys |
| K2 | none | keys are atoms (D.10.6) | |
| K3 | `inv_K3` | names and aliases disjoint; every node's place is its live name, and every live name names a node at that place; aliases name nodes | keys |
| K4 | `inv_K4` | every parent walk reaches ROOT | keys |
| K5 | `inv_K5` | `merged_into` only on closed `superseded` nodes, naming a node; every chain ends at a node not merged | keys |
| K6 | `inv_K6` | an open series has exactly one open occurrence, a closed series and a node that is no series none | recurring |
| K7 | `step_K7` | a reopened node keeps exactly the keys no other node carried openly before | keys |
| K8 | `inv_K8` | the reserved nodes as Genesis made them; no other reserved origin | every slice |
| C1 | `step_C1` | every request from a conversation, people question, `said`, `heard`, and commitment a step records lies on the conversation node of its key (naive §9.1), not reserved, with that key; only `request`, `said`, `heard`, and `ask` record them; 1.3: a commitment is fulfilled only by a new reply-kind `said` on its node in its conversation, and such a `said` leaves none of its conversation open there | keys, people |
| Q1 | `inv_Q1` | request origins distinct | people |
| S1 | `inv_S1` | open signal nodes and `signals` agree both ways, one per key | keys |
| N1 | `inv_N1` | `unrouted` on `@user` exactly when an entry is unrouted; every user entry has its `from_user` entry on `@user` with its principal, and its routing names what it points at (a request of that entry, a request, a question it answered) | user |
| Settled | `inv_Settled`, `step_Settled` | a settled request names a decision that lists it and its answer; an unsettled one is not both answered and listed by a decision that is not retracted; an answer, settled or not, is newer and of its kind (a reply-kind `said` in the request's conversation, or a `to_user` with a principal); settling and answering are for good; a `to_user` answers only as a new `result` to the same person; a `said` answers only in a command, of a reply kind and in the request's own conversation (1.3), recorded by the step or credited by a merge | people, user |
| F | `inv_F` | F2: no flag changes by time alone before the next queued key (or ever, with none); the open-only flags never on closed nodes, `ready_to_close` never on reserved ones, `basis_changed` never on merged ones. F1 has nothing to compare: the model stores no flags | people, recurring, basis |
| N5 | `step_N5` | D.9.2's N5 for a merge, keys, open items with `moved_from`, marks, another session's intent, no edge to `from`, no self-edge; a `cycle` changes nothing | keys, people |
| E1 | `inv_E1` | (a) to (f) of D.9.2, and closed nodes carry no assume mark | people, user |
| W1 | `inv_W1` | a waiting node has neither `unanswered_request` nor `open_intent`, and `classify` gives it a status item exactly while it has `open_commitment` | people, basis |
| R2 | `inv_R2`, `step_R2` | no `missed` node with a checkpoint; a `missed` close only of an occurrence with no checkpoint, no intent, and no hold live at the close, apart from holds of other sessions the step's own move ended (below); a session's `close {missed}` is refused | recurring |
| D1 | `inv_D1`, `step_D1` | every digest entry to an interned principal, on `@user/digest`, shown only by a `to_user`; per firing, each expiry, default, drop, and lapse emits one pending entry per principal of its audience on the state before the firing, and nothing else; a command emits none; a `DigestShown` comes from a new `to_user` to the same person | people, user |
| M1 | `inv_M1` | M1a's table of D.9.3, row by row | every slice |
| U1 | `inv_U1`, `step_U1`, `hello_exact` | chains non-empty, without repeats, at most 8 stages, ending with `user`, stage within; at creation the chain of D.8.12 (below), stage 0, pending, no stage before the last an alias of `user`, no two aliases of one person; `hello` lists the same pending questions and digest entries by a scan and by the principals the session serves | user |
| T1 | `inv_T1`, `step_T1` | entries at or before `last_now`, which is at most the clock; a command stamps its entries with its `now` and moves `last_now` to it, and a call that appends nothing changes nothing; only the caller's presence moves; a firing stamps its entries with its key's time, not before `last_now` | holds |
| T2 | `inv_T2` | `run(run(s, now + a), now + b) == run(s, now + b)` for `0 <= a <= b <= K`, with no settle saturated or stuck: K = T2_K (12) in simulation and TLC, 4 (`holds`, `recurring`) or 2 (`people`, `user`) in Apalache (D.10.7) | holds, people, user, recurring |
| T3 | `inv_T3` | settled at `now`, every marker equal to its predicate (D.9.2's list), closed nodes have no wait, agent questions have their notes | every slice |
| T4 | `inv_T4`, `step_T4` | nothing queued at or before `now`, `first_key` is the queue's minimum, no settle stuck; a command queues no key at or before `now`; each firing fires the minimum, which is gone after it, and queues no key at or before its time; the replay ends where the settle did, with nothing due | every slice |
| T5 | `inv_T5` | the graph of `{node}` waits and `depends_on` is acyclic (naive closure) | keys |
| L1 | `inv_L1`, `step_L1` | every node within `tree.max_depth`; a `create` or `move` that raises a parent's open agent-made children leaves them within `tree.max_children` (not `@people` or `@teams`); a long text changes nothing and is `limit(TextTooLong)` | holds, keys |
| P1 | `inv_P1` | `now < until <= last_renewed + ttl`, with the scaled ttl | holds |
| P2 | `step_P2` | no hold issued on a node paused, itself or above, before the step | holds |
| AR | `archived` | the definition; no state field and no model action reads it | |
| G1 | none here | not modelled: D.0.3 dropped WP-A5 (`guide.qnt`); `check_guide` in Rust | |
| (D.10.4) | `inv_settle_bound` | no settle saturated | every slice |

Readings that differ from D.9.2's wording, each with a run test in `tests/invariants_test.qnt` and an
item of "Open questions":

- **R2's step half and a move** (`r2MoveTest`). D.9.2 has a `missed` close happen "only on a node
  with no checkpoint, no open intent, and no live hold in `before`". A move by another session ends
  the holder's hold, and D.8.20 then runs the missed check, which A-80 has run "after every hold end
  that leaves the occurrence open (lease expiry, release, move)": an occurrence held in its window
  and moved by another session closes as `missed` in a step whose `before` has a live hold. §26 R2
  says nothing of holds. `step_R2` excepts the holds of other sessions that the step's own move
  ended, and nothing else.
- **U1's chain "at creation"** (`u1ChainAtCreationTest`). D.9.2 compares a new question's chain with
  `spec::chain(before, …)`. An `update` that sets the owners (or the `gone` tag) and the needs of a
  refused node in one call makes every class inactive for it, and `e1_check` creates the refusal
  question after `NodeUpdated`, with the new owners (D.8.20, D.8.14); §26 U1 fixes the chain "at
  creation". `step_U1` reads the chain on the state after the step, which equals the state at
  creation for every model command and firing (nothing after a creation changes what `chain` reads).

Beyond D.9.2's text, the checks also assert, as consequences of §26 that the model's states can
break: an answer is valid even before its request settles (Settled), the hold terms are the scaled
parameters (H5's step half), a node that is no series has no open occurrence (K6), closed nodes carry
no assume mark (E1 (c), A-104), a command emits no digest entry (D1: in the model only firings
expire, default, drop, and lapse), closed nodes have no wait and agent questions have their notes
(T3).

### Non-vacuity

A check that no reachable state can break proves nothing, so each was shown to fail on a mutated
model. For each check, a scratch copy of the model (never `model/`) got one mutation of a
transition (`commands.qnt`, `deadlines.qnt`, `flags.qnt`, or, for H2, `params.qnt`; for
`inv_settle_bound`, the slice's MAX_FIRINGS), and the slice module whose actions reach it was
simulated with that check alone as the invariant (`quint run slices/<x>.qnt --main <module>
--invariant m_<check>`, 30 steps, up to 4,000 runs, or 40,000 where 4,000 did not reach it; the
`one_class` instance, 40 steps, for L1, whose depth no slice reaches). The control is the same
harness on the unmutated model: each module simulated with every check its mutants use, all at
once (`--invariants`), finds no violation. The driver is a scratch script, not kept; the mutations
are listed here so that they can be repeated. Results, on the model as committed (@@CAMPAIGN_DATE@@):

| Check | Mutation (file) | Module | Result |
|---|---|---|---|
@@MUTATION_TABLE@@

@@CAMPAIGN_SUMMARY@@

## The slices (WP-A3)

The six slices of D.10.7 restrict `step` to the actions their invariants need. Each file
`slices/<x>.qnt` has a module of pure definitions `<x>` (`keys_slice` for `keys`, whose name is a
Quint operator's): the commands its actions pick from, and `<x>_invariants`, the conjunction of its
checks. Then one module per class configuration, `<x>_two` (classes `a` and `b`, `QUORUM = 2`) and
`<x>_one` (class `w`, `QUORUM = 1`), and `user_none` (one class, `PRINCIPALS = Set("user")`,
`HAS_USER = false`), each of which instantiates `forum.qnt` and defines `step_<x>`, `inv_<x>`, and for
`holds`, `recurring`, and `user` `liveness_<x>`; `holds`, `people`, `user`, and `recurring` also define
`inv_<x>_apalache`, with T2 over fewer ticks (D.10.7: 4, or 2 in `people` and `user`). Quint 0.33 does
not pass a module's constants on to an instance it makes (a probe of a module `sl(M)` that imports
`base(N = M)` stopped with "Uninitialized const N"), so a configuration cannot be a constant of `<x>`,
and each configuration module repeats its slice's short list of actions. `slices/common.qnt` holds
what the slices share: the picks, the six checks of every slice (`common_invariants`), and the seed's
node ids.

A slice's command action is `slice_call(name, cmds, op_long)` of `forum.qnt`: a live session (as
`session_picks`), a whole command from a small set that the slice computes from the state, and an
`op` with a long text or not, sent through `command` under the action name of D.10.4, which
`last_action` records with the command, as a replay of a counterexample needs (D.10.8). `hello` is
`slice_hello(caps, principals)`. The sets are small so that TLC meets few branches: each names the
nodes an agent or the server made, the next id (which names none), the reserved nodes K8 needs, and
for each command the values the slice's invariants are about (for example, in `holds`, a capacity of
0 or 2 and a digest with and without `digest_version`; in `people`, every origin and `where` of a
request and every kind of `said`; in `user`, a person of one alias and of two, and a team owned by
each person). Only `holds` gives a call an `op` or a long text (`WRITE_OPS`: none, `o1`, `o1` with a
long text, on the writes; `take` an `op`; `TEXT_OPS`, a long text, on `create`, `update`, and `pause`,
whose commands carry a text too): D.10.4 fixes `long` at `false` in the
other slices, and O1, the only check that reads `op`, is checked in `holds`.

| Slice | MAX_FIRINGS | Seed | Time actions |
|---|---|---|---|
| `holds` | 24 | `/w1`, `/w2` | tick, tick_next |
| `keys` | 30 | `/w1`, `/w2` | tick |
| `people` | 30 | `/w1`, `/w2` | tick, tick_next |
| `user` | 40 | `/w1`, `/w2` | tick, tick_next |
| `recurring` | 30 | `/w1`, `/w2`, the series `/s` and its first occurrence | tick, tick_next |
| `basis` | 24 | `/w1`, `/w2` | tick |

In 1,000 random runs of 30 steps (`--invariant true`, `--witnesses` of `forum.qnt`), the slices reach
what they are for: `holds_two` a replay and a conflict (6, 80 runs), a stale token (487), and a
refusal; `keys_two` a node of two conversation keys (223), a signal (243), a merge (103), a moved
request (13), a fulfilled commitment (23); `people_two` an expired and an escalated people question
(36, 31), two conversation keys (37); `user_two` and `user_none` user questions answered (22, none
in `user_none`), defaulted (120, 140), lapsed (211, 252), and re-addressed (90), a refusal's digest
entry (17 in `user_none`), a person beside a team (21, 37); `recurring_two` a missed occurrence (966);
`basis_two` a stale basis (149), a decision superseded and retracted (83, 74), a basis of two nodes
(84). Slices without `hello` lose their sessions at `session.retain` (10 ticks), so most of their
successes lie in the first ticks, where TLC looks (below).

### Liveness

Each property of D.10.7 is `eventually(always(P))` (`forum.qnt`), with P "no object of its kind has a
deadline at or before MAX_TIME left" (`invariants.qnt`): `deadline_fires` (no queued key),
`lease_ends` (no hold), `pause_ends` (no pause), `occurrence_resolves` (every open occurrence whose
window ends by MAX_TIME is flagged), `uq_resolves` (no pending question for the user), and
`wait_on_uq_resolves` (no unresolved wait on one). §28's bounds make "eventually fires" a statement
about MAX_TIME: an object whose deadline lies beyond it is outside them. `liveness_<x>` assumes weak
fairness of `act_tick`, `act_tick_next`, and the slice's calls, as D.10.7 has it, and is checked
with TLC (`npm run liveness:<module>`, below).

Under weak fairness of `act_tick`, which is enabled whenever `clock < MAX_TIME`, every behaviour's
clock reaches MAX_TIME and stays there. So `always(clock == MAX_TIME implies P)` implies
`eventually(always(P))`: from the first state at MAX_TIME on, every state satisfies P. The safety
probe can be checked by simulation over whole runs, where TLC's exhaustive search reaches only the
first steps (below). The probes
(`m_live_<property>` of the non-vacuity harness) hold on the unmutated model in all seven modules
that have a liveness property, and each fails on a mutated model:

| Property | Mutation (file) | Module | Result |
|---|---|---|---|
@@LIVENESS_TABLE@@

## How the model is built

- **Time** is in ticks; one tick is one minute when a trace is replayed (D.10.5). `params.qnt` lists
  every scaled duration beside its real value; every order and equality of the real durations holds
  between the ticks, and no new one appears between two durations. One comparison of `Params::valid`
  becomes an equality: `3 × presence.resolution <= presence.silence` is 9 < 15 minutes and 3 = 3
  ticks (`VALID_COMBINATIONS` in `params.qnt`, and "Open questions", item 3). `MAX_TIME = 40 < 60`, so every model time lies in the first
  hour bucket and `aged = priority`.
- **No event log.** A transition applies the base change of D.7.4 directly. The functions named
  `ev_…` in `deadlines.qnt` are these base changes; `append_entry` allocates the next eid and seq
  and keeps the entry's metadata where Rust keeps it (`keeps_meta`). Ids are allocated in the same
  order as Rust's events, so states compare field by field.
- **The queue is computed, never stored.** `queue(s)` is the recount of D.4.8, `first_key(s)` its
  minimum by `(at, kind, owner)`, computed by one fold per owner kind without building the set (it
  equals `queue_min(s)`, the minimum of `queue(s)`, on every state the tests reach), and a firing
  removes its key because its transition or marker makes the clause of `deadline_of` empty. Owners
  are integers, `rank × 10^6 + payload`, in Rust's order (D.10.4). Quint cannot compare strings, so
  a class's index is its position in `CLASS_NAMES` (`params.qnt`), which must list every class name
  an instance uses, sorted (`classes_indexed`).
- **Settling** is `range(0, max_firings).foldl(…)` with two flags. `stuck` is Rust's `Stuck(k)`
  (D.8.2): a firing left its own key in the queue, or queued a key before it, and the fold stops
  there; it is a model or core bug, and WP-A3's `inv_T4` fails on it. `saturated` is a key still due
  after `max_firings` firings of a fold that was not stuck. Each slice sets `max_firings` to the
  firings its bounds allow (D.10.4), and `inv_settle_bound` fails on saturation, so a broken arm is
  never reported as a bound to raise.
- **Absent values.** `-1` stands for an absent id, time, or count (Rust's `None`), and `""` for an
  absent label or principal key; where absence changes what an entity is, a sum type is used
  (`OptWait`, `Settlement`, `LifeAt`). A command's optional fields follow the same rules, listed at
  `Cmd` in `types.qnt`: `-1` is an absent `dver`, `content`, `status`, `digest`, or `capacity` of
  `CUpdate`, so that WP-A2 and the replay driver treat presence as Rust's validation does
  (`MissingDigestVersion`, `ReservedNode`, D.8.20).
- **Names.** Constructors share one namespace in Quint, so the variants of one domain carry a prefix
  (`Pq…`, `Uq…`, `Sd…`, `Why…`). The names D.10.3 gives the conformance projection are kept: `Conv`,
  `Msg`, `User`, `Person`, `Lit`, `Slot`, `SlotDup`, `Signal`, `BNode`, `BEntry`; D.10.3's `Repeat` is
  `RepeatSlot`, because the TLA+ that Apalache and TLC check extends Apalache's standard module, which
  defines an operator `Repeat` (no identifier may repeat an operator of the modules generated TLA+
  extends: Integers, Sequences, FiniteSets, TLC, Apalache, Variants).
- **64-bit values.** Quint 0.33.0's default evaluator computes in 64-bit signed integers and stops on
  an overflow, so FNV-1a-64 works on four 16-bit limbs, least significant first. `tie_hash` returns
  the limbs, `u64_of(hi, lo)` builds them from two 32-bit halves, and `tie_key(use_fnv, sess, id)`
  gives the pair `(hi, lo)` that orders like Rust's `h`, or `(0, 0)` without FNV, so that the id
  breaks the tie (D.10.6).
- **Holds in the map, and live holds.** Rust's `ix.node[n].live` counts the holds in the map, and a
  firing reads it (`maybe_missed`, `refusal_default`, `close_node` step 2); the model's `holds_on`
  does the same. Flags and the invariants read the lease predicate `live(h, now)` (`live_holds_on`,
  `attended`, `held_self`). They differ only between two firings at one instant (D.8.2).
- **One call** is `run_cmd(cfg, s, now, class, sess, op, long, cmd)` (`commands.qnt`), D.8.1's
  `apply`: `long` is `limit(TextTooLong)` before anything else; a live record of `(class, op)` is a
  replay or a conflict; the session must exist and be of the class; `validate(s, sess, now, cmd)`
  checks in D.8.24's order on the state before the call, every reference before it is read, as
  Rust's `Plan` does; `stale_token` appends one `late` entry. On success `execute` applies the
  command's base changes on `at_event(s, now)` and returns the reply's ids, whether it appended
  anything, and whether it issued a hold; `Touched` (which only sets the session's presence fields,
  so it commutes with the command's changes) and the `op_id` record follow. A call that appends
  nothing returns `s` itself. `hello` always opens a new session (D.10.4's `act_hello(c, caps,
  principal)` names no session).
- **The order of keys.** Rust iterates a node's keys in the order of their strings, and the replay
  driver sends a command's keys in that order (D.8.9's `PrincipalSeen` and routing, D.8.20's
  dropped keys and their signals, a merge's keys). `key_rank` gives each key of the model's domains
  an integer that orders like the string the driver sends for it: `chat:t/<c>` (`Conv`),
  `chat:t/<c>/<n>` (`Msg`), `lean:<o>` (`Obj`), `person:test:p1`, `person:test:p2`, `person:user`
  (`Person`), `repeat:<series>:<slot>` (`RepeatSlot`), and `user:e<e>` (`User`), with the decimal ids
  in string order (`user:e10` before `user:e9`, `lex_rank`). D.10.8's driver must render keys so.
- **The reply's ids** (`OutOk`), as conformance compares them (D.10.8), are listed at each
  command's execution in `commands.qnt`: for example `[id, fence]` for a create (`fence` -1 without
  a hold), `[existing]` for a join, `[0, node, fence]`, `[1, node]`, `[2, entry]`, or `[]` for
  `next`, `[said, node]` followed by the requests it settled for a `said` (1.3's `settled`,
  D.8.15), and `[entry, principal]` for `from_user` and `to_user`.
- **Actions** (`forum.qnt`) pick every parameter with a named `nondet` (quint-connect reads them,
  D.10.8): the session among the live and silent ones (or, when every session has retired, the
  lowest id of the bounds), the class of a call to a session that no longer exists, nodes among the
  agent- and server-made ones, five reserved nodes, and an id that names none, keys, tokens,
  questions, entries, and so on from the state. `command(name, sess, cls, op, long, cmd)` checks
  that the state is settled at `now = max(clock, s.last_now)`, runs `run_cmd`, then D.8.1's
  post-settle at `now` (the time actions' `settle`, MAX_FIRINGS), and is disabled when the
  command's result would take a bounded quantity past its bound (`within_bounds`: a quantity a
  firing has already taken past its bound may stay there, so that the commands that do not raise it
  stay enabled). The post-settle fires the Silence key of a session whose last hold the command
  ended after its `silence_at` (1.4, R27, D.4.8), the one key a modelled command queues at its
  `now`; without it the state after such a command was not settled (T3, T4), and every command was
  disabled until a tick fired the key later than the core does. `mid` keeps the state between the
  command and the post-settle for the step checks, and `last_action.settle` the post-settle's flags.
  The time actions are bounded by `MAX_TIME` only. `step` picks among five groups alike:
  time; the `session`, `node`, and `control` actions and `view`; the `hold` actions; the `talk`
  actions of a request's flow (`request`, `said`, `heard`, `ask` to a person, `decide`, and
  `to_user`); and the other `talk` actions. A `hello` opens the class its session id belongs to
  (`hello_class`: ids 1 and 3 the first class, 2 and 4 the second). The picks carry D.10.4's
  parameter names, except where Quint reserves the name or a constructor field differs: `key_set`
  (`keys`), `from_node` (`from`), `for_node` (`for`), `target` (`to`), `where_key` (`where`),
  `tok`, `done` (`intent_done`), and `cls`, the class of a call to a session that does not exist.
  D.10.8's `switch!` arms read these names. Some picks are drawn in two steps, each step named too
  (the replay ignores the first): `roll`, the rarity roll of RARE_ODDS, before `op` and `long`;
  `wait_group` (none, time or external, a question, a node) before `release`'s `wait`;
  `basis_group` (no reference, one, two) before `decide`'s `basis`; and `at_group` (the nodes that
  carry a conversation key, or every node of the picks) before the `at` of `request`, `said`,
  `heard`, and `ask` to a person.
- **Simulation coverage.** A check over states that simulation never reaches passes vacuously, so
  a simulation's result is reported with the number of runs that reached each witness `w_…` of
  `forum.qnt` (`scripts/simulate.sh`). The review of WP-A2 found the uniform picks of the first
  version wasteful: `long` was drawn for half the calls with a text and an `op` for two calls in
  three, so 32 % of the command steps were `limit(TextTooLong)` and 14 % `op_id_conflict`; a failed
  call touches no session, so sessions retired early (every session had retired in 71 of 100
  traces, at a median step of 12); 16 % of the command steps changed the state; and in 15,000 runs
  no request settled, no commitment was fulfilled, and no node waited on a question. The picks are
  now weighted (the rarity roll, the grouped picks, and the five step groups, above). Measured on
  2026-10-02 with `npm run simulate:1000` (random seeds), the runs of 1,000 that reached each
  witness, for `two_class`, `one_class`, and `no_user`:

  | Witness | two_class | one_class | no_user |
  |---|---|---|---|
  | a request answered | 79 | 57 | 76 |
  | a request settled | 17 | 8 | 22 |
  | a user request answered (`to_user`) | 3 | 1 | — |
  | a commitment fulfilled | 19 | 16 | 23 |
  | a node with two conversation keys | 110 | 96 | 133 |
  | one conversation's request answered beside another's unanswered | 5 | 2 | 1 |
  | a people question answered, expired, escalated | 54, 29, 39 | 69, 49, 36 | 65, 41, 37 |
  | a user question answered, defaulted, lapsed, re-addressed | 19, 123, 96, 80 | 7, 139, 96, 84 | 1, 157, 155, 0 |
  | a node waiting on a question | 12 | 6 | 12 |
  | a refusal; a class at its quorum; a refusal's digest entry | 12; 0; 0 | 12; 12; 5 | 13; 13; 2 |
  | a decision superseded, retracted; a basis of two nodes; a stale basis | 80, 11; 171; 68 | 74, 9; 159; 51 | 103, 9; 191; 83 |
  | a replay (O1); an `op_id` conflict | 1; 295 | 4; 344 | 6; 362 |

  Every command succeeded in at least 14 runs of each instance where it exists (`merge` in
  `one_class`; most in over 100), except `reopen`
  (4 to 10) and `route` (3 to 5), each of which needs an earlier success of another command (the
  command tests reach both). In 100 `--mbt` traces of `two_class`, 45 % of the command steps
  changed the state and 37 % succeeded; `not_found(Session)` is 14 %, `stale_token` 10 %, and
  `limit(TextTooLong)` 9 %; every session had retired at some step in 31 traces (at a median step
  of 11, when the clock passes `session.retain` before the sessions call), and 1.07 sessions were
  live at step 20 and 0.84 at step 40 on average. With `npm run simulate`, `simulate:one_class`,
  and `simulate:no_user` (20,000 runs each, no violation), the rare witnesses were reached in this
  many runs (`two_class`, `one_class`, `no_user`): a request settled 278, 250, 406; a commitment
  fulfilled 326, 325, 453; a node waiting on a question 194, 148, 177; one conversation's request
  answered beside another's unanswered 72, 87, 51; a replay 66, 106, 125; a person beside a team 13,
  24, 14; a class at its refusal quorum 0, 201, 228, and a refusal's digest entry 0, 82, 82. So in
  `two_class`, whose quorum is 2, no run reached a refusal quorum: it needs two sessions of one class
  to refuse one node within `refusal.ttl`, and a `release` with `cannot` is one draw in twelve of a
  release; `releaseCannotTest` reaches it, and WP-A3 needs a flow, a slice, or TLC for E1 (a) at a
  quorum of 2 (item 9 of "Open questions"). Picks aim at plausible targets and keep one target for each
  refusal: tokens are the live holds' and one spent token, `node key {remove}` names a key of the
  node or the object key, `node reopen` a closed node, ROOT, or no node, and `control unpause` a
  paused node, ROOT, or no node.

## Deviations from D.10

Each is recorded for a decision on whether D.10 changes to match.

| Where | D.10 says | The model | Why |
|---|---|---|---|
| `params.qnt` `SEED_REPEAT` | period 10 ticks (D.10.5, D.10.6) | period 15 (`*/15`, which divides 60) | With period 10, a series created at 0 that nobody takes opens 4 occurrences within 40 ticks (slots 10, 20, 30, 40), against "up to 3 occurrences of one recurring node" (§28, D.10.7). With 15 it opens 3 (`seedOccurrenceBoundTest`) |
| `types.qnt` `Node.status`, `CUpdate.status` | status is part of the one integer `content` (D.10.3) | a separate integer | Since CS-34 a status change bumps `version` but not `content_version` (§5, O2), so approvals survive it (§11.2). With status in `content` no command could change one without the other, and conformance could not catch a core that moves `content_version` on a status change. The replay maps status `v > 0` to `s<v>` |
| `types.qnt` `Key.Obj` | `Key` has no object scheme (D.10.3) | `Obj(o)`, an agent's object key (`lean:<o>` in a replay) | CS-36: only `conversation.schemes` name conversations. Without it `conv_key` never meets a node with an object key beside its chat key (RS-8), nor `create {request}`'s "first conversation key among keys" (D.8.9). WP-A2's `act_create` and `act_key_add` should pick it, at least in WP-A3's `keys` and `people` slices |
| `types.qnt` `Cmd` | D.10.4's parameter lists | `CFromUser.clarifies`, `CAskUser.again` and `.digest`, `BaNode.version` | `from_user {clarifies}` (§8.1, the precedence `answers` > `clarifies` > `node` of §8.2, A-99, and §32 step 1, which `scenario_32` replays), "Asking only once" (§8.4: the same text and options for the same first addressee is `{existing}` unless `again`), and `invalid(VersionAhead)` of `decide` (A-71) are unreachable otherwise |
| `deadlines.qnt` `Settle.stuck` | `settle` returns the state and `saturated` (D.10.4) | also `stuck` | D.8.2's `Stuck(k)`: without it a firing arm that left its key would fire it again until `max_firings` and be reported as saturation |
| `basics.qnt` `occurrences_made`, `may_open_occurrence`; `params.qnt` `classes_indexed` | — | guards for WP-A2 | A command that opens an occurrence at `now` (`update {repeat}` dropping a waiting one, `repeat` added to a plain node, a reopen of a closed series) must be disabled at MAX_OCCURRENCES, as D.10.7's other guards are; every instance asserts `classes_indexed(CLASSES)`, because a class outside `CLASS_INDEX` would encode as a `Refusal` owner. WP-A2's `within_bounds` applies the bound to every command, which subsumes `may_open_occurrence` |
| `forum.qnt` constants | `CLASSES`, `USE_FNV`, `QUORUM`, `PRINCIPALS`, `HAS_USER` (D.10.1) | also `MAX_FIRINGS`, `SEED_SERIES`, `LONG_TEXTS`, `RARE_ODDS` | D.10.4 sets `MAX_FIRINGS` per slice, and Apalache needs it bound to a literal; D.10.7 gives only the `recurring` seed the series `/s`, and the instances take it too (`two_class` and `no_user`; `one_class` not, so that it has room for creates under ROOT, whose child limit the seed's three children reach); D.10.4 picks `long` in `holds` and in simulation and fixes it at `false` in the other slices; `RARE_ODDS` (8 in the instances, 1 in the tests and for the slices) makes a call with an `op` or a long text rare in simulation and adds no branching where it is 1 ("Simulation coverage") |
| `types.qnt` `CCreate.tags`, `Action.settle` | — | Rust's `CreateArgs::tags`; the flags of the settle that ended the last action (a command's post-settle, a time action's settle) | The seed's nodes are "created with `take: false` and tagged `offer`" (D.10.7), and `act_create` leaves `tags` empty; `inv_settle_bound` and `inv_T4` need the flags of the last settle, and a command's outcome is taken by its reply (the time actions' outcome is `OutNone`) |
| `forum.qnt` `command`, `mid`; `invariants.qnt` `steps_of`, `step_T4` | "Commands do not settle in the model", and the post-settle "never has work" (D.10.4); four variables (D.10.2); T4's command half "a command queues no key at or before its now" | a command action ends with D.8.1's post-settle at its `now` (`settle`, MAX_FIRINGS); a fifth variable `mid`, the state after the command and before the post-settle (`prev` for `init` and the time actions); the step checks run on the command (`prev` to `mid`, `check_step`) and on each firing of the settle (`mid` to `s`, `check_fire`); `step_T4` lets a command queue a key at exactly its `now` only for the Silence key of a session whose last hold it ended after its `silence_at`, as D.9.2's T4 says | 1.4's Silence key (R27, D.4.8) lies at `max(silence_at, the largest lease_until of its holds, last_now)`, so a command that ends another session's last hold past its `silence_at` queues the key at its `now`. Without the post-settle the state after it was not settled: simulation of `two_class` failed `inv_T3`, `inv_T4`, and `step_T4` (seeds `0x93f4da67ca4de794` and `0x94a12e7bdfc1e83a`, 40 steps), every command was then disabled, and a later tick fired the key later than the core, whose post-settle fires it in the same call |
| `forum.qnt` `act_hello` | `act_hello(c, caps, principal)` with `long` (D.10.4: every action whose command carries a text) | no `long` | The model's `hello` sends no `intent` and no `from_user`, so the replayed call carries no text |
| `forum.qnt` the class of a call | D.10.4's actions name a session but no class | the session's own class; a picked class for a session that does not exist | Every agent calls with its own token. `invalid(ClassMismatch)` is therefore reachable only in `run_cmd` (`classMismatchTest`) |
| `forum.qnt` sessions, `act_hello`'s class | 2 classes × 2 sessions each, session ids 1..4 (D.10.7); `act_hello(c, …)` picks its class (D.10.4) | the session ids within `1..SESSIONS_PER_CLASS × CLASSES.size()`, and `hello` opens the class of its id (`hello_class`: they alternate in the order of CLASS_NAMES, as the seed opens them) | A retired session leaves the state with its class, so a count per class cannot be read from the state; with a free class, one class could open three sessions in `two_class` (review of WP-A2), against the bound and the reason for `QUORUM = 2` (D.10.1) |
| `forum.qnt` `step`, the picks | a choice among the actions (D.10.4) | time and four groups of commands picked alike; sessions picked among the live ones; some picks drawn in two steps (a group, then a value; the rarity roll before `op` and `long`) | A flat choice moved the clock about twice in 40 steps; with time and the commands alike every session had retired within the first half of a run (240 runs of 40 steps: 1.2 sessions live at step 10, 0.2 at step 30); 4 % of the commands succeeded with sessions picked among 1..4. With uniform picks, 16 % of the command steps changed the state and no run of 15,000 settled a request or fulfilled a commitment ("Simulation coverage") |
| `package.json` `test` | `test` is `quint test tests/forum_test.qnt` (D.10.1) | `quint test` of every `tests/*_test.qnt`, on the TypeScript evaluator for `*_ts_test.qnt` | The helper tests and the `u64::MAX` vectors are run tests too. By default `quint test` runs only the runs named `…Test`, so `scenario_32` and `test_T2`, whose names D.10.8 and D.13.2 fix, run under the aliases `scenario32Test` and `t2Test`; `--match '.*'` would run every definition as a test (872 of them, helpers included, all failing) |
| `package.json` `simulate`, `simulate:1000` | `quint run instances/two_class.qnt --max-steps 40 --max-samples 20000 --invariant inv_all` (D.10.7) | the same through `scripts/simulate.sh`, with `--witnesses` | A simulation's "no violation" says nothing of the states it never reached; the script reports each witness's count and fails when a required one is never reached (review of WP-A2) |
| `package.json` `traces` | `--out-itf traces/t{}.itf.json` (D.10.7) | `--out-itf 'traces/t{seq}.itf.json'` | Quint 0.33's placeholder is `{seq}`; with `{}` the files are named `t{}0.itf.json`, `t{}1.itf.json`, … |
| `commands.qnt` `lease_l` | `l := min(max(lease, SECOND), lease.max)` (D.8.10) | the lower clamp is one tick | One second is below a tick; the actions pick no lease below one tick, so a replay never meets the difference |
| `commands.qnt` `v_hide`, `v_retract` | the shell adds an entry's node from its `entries` table (D.5.2) | only a kept entry's node is known | The model keeps no metadata of checkpoints, plain notes, system entries, or late writes (CS-17); its actions name only kept entries, or an eid that does not exist yet |
| `tests/forum_test.qnt` `scenario_32` | a run of the named actions within D.10.7's guards (D.10.4, D.10.8); D.12.2's advances of 2 h and 21 min, after which every session of §32 is still present | `call32` and `tick32`: `command` and `act_tick` without D.10.7's bounds, with each reply checked; each advance played one tick at a time; the seed's /w1 and /w2 closed first; three calls of §32 made as two each; six sessions retired at tick 10, within the second advance | §32 opens nine sessions, records three user entries, and with the setup makes 24 entries, past the bounds (4, 2, and MAX_EID 16); the scaled `tree.max_children` (3) leaves no room under ROOT for §32's two top-level nodes beside the seed's three; the model's `hello`, `create`, and `create`'s join carry no `from_user`, no path of missing ancestors, and no `request`; and the two advances, 7 + 4 ticks, pass `session.retain` (10 ticks), which their real sum, 2 h 21 min, is far below: D.10.5 keeps the order of each duration, not of sums ("The §32 scenario and T2"; "Open questions", item 15) |
| `commands.qnt` readings of D.8 | — | as listed | `e1_check` in a command (a refusal, a reopen, a needs change) records the refusal question with the session as its author; a move's child limit does not count the moved node when it stays under the same parent (a rename); `decide`'s `SelfLink` is judged after `merged_into` (a reference to a node merged into `at`); `RoleConflict` is a `repeat` set on an occurrence or a signal, and a clear of it changes nothing there; `node key {remove}` is refused for a `repeat:` key and a reserved node like an addition; a merge answers only the moved requests from people and agents, as D.8.8's answers do |
| `package.json` `verify:<slice>`, `tlc:<slice>`, `liveness:<slice>` (D.10.1, D.10.7) | `npx quint verify slices/<x>.qnt …` and `--backend=tlc` | `verify:<module>`, `tlc:<module>`, `liveness:<module>`, one per slice module, through `scripts/check.sh`, `scripts/verify-file.sh`, and `scripts/compile-pruned.js`, each under a time limit | `quint verify` cannot take this model ("Apalache and TLC on a model this size"), and every slice is checked in both class configurations, `user` also as `user_none` (D.10.7) |
| `slices/*.qnt` | one module per class configuration (D.10.1) | the same, and a module of pure definitions per slice (`holds`, `keys_slice`, …) that the configuration modules import; `slices/common.qnt`; each configuration module repeats its slice's list of actions | Quint 0.33 does not pass a module's constants on to an instance it makes, so a configuration cannot be a constant of one slice module ("The slices") |
| `invariants.qnt` `step_R2`, `step_U1` | D.9.2's "no live hold in `before`" (R2) and `spec::chain(before, …)` (U1) | holds that the step's own move ended are excepted (R2); the chain is read at creation, on the state after the step (U1) | §26 and A-80 (R2) and §26 U1's "at creation" (U1); "Open questions", items 17 and 18 |
| `slices/people.qnt`, `slices/user.qnt` `inv_<x>_apalache` | T2 with `K = 4` in Apalache, `K = 2` in `user` and `people` "if Apalache needs it" (D.10.7) | `K = 2` in `people` and `user` from the start | Apalache does not reach one step of any slice with either ("Open questions", item 19), so 4 was not tried first |
| `tests/invariants_test.qnt` | — (D.10.1 lists `tests/forum_test.qnt`) | run tests of the checks on steps built by hand | the readings of items 17 and 18 are pinned by tests, and the seed and two conversations on one node are checked by every check at once |

## Open questions

Found by the review of WP-A1 and reproduced; the specification and the design were not changed.

1. **L1 and a reopened series at `tree.max_depth`** (§26 L1, D.8.4, D.8.20, D.8.18, D.10.7).
   `open_next_occurrence` checks no limit, as D.8.18 has it, and its callers check L1 only on
   `create` and on an `update` that adds `repeat` to an open node (D.8.20: "L1 for a new occurrence,
   when `n` is open"). Trace, with `tree.max_depth = 4`: create `/a/b/c/d` (depth 4); close `d`;
   `node update d {repeat}` (a closed node: only `NodeUpdated`, D.8.18); `node reopen d`: step 7
   opens the occurrence at depth 5. The same follows from D.8.4's reading ("on update that adds
   repeat") by moving a closed depth-3 series under a depth-3 parent: `depth(new parent) + 1 +
   height(n) = 4` passes, because a closed series has no children. A `take` or `next` of a flagged
   closed series reopens it the same way. In the model: a series at depth 4 and
   `open_next_occurrence` give an occurrence at depth 5. Options: (a) check L1 for a series' future
   occurrence on every update that adds `repeat`, open or closed, and count a series as height ≥ 1
   in `move`; or (b) at a reopen of a series at `tree.max_depth`, refuse with `limit(Depth)` or
   open no occurrence (which breaks K6). Either way, D.10.7's `recurring` slice should check `inv_L1`
   (it is now only in `holds` and `keys`, which have no `update {repeat}`), with a `create` that can
   reach depth `tree.max_depth`.
2. **Which `PqAssumed` a decision emits** (D.8.17 step 3 and `record_assumptions`, D.7.4). D.8.17
   names `assumed_by = None` only for the user question ("`PqAssumed` if `q` is a people question still
   `Expired`, or `UqAssumed` if `q` is a user question still `Lapsed` with `assumed_by = None`"), while
   `PqAssumed`'s precondition in D.7.4 is "`Expired`, `assumed_by = None`". One expiry can mark two
   waiting nodes (D.8.2: `marked = resolving`), and a decision on the second then emits a `PqAssumed`
   that `evolve` counts as an anomaly. Trace (`assumeOnceTest`): question #2 non-blocking, nodes #9
   and #10 waiting on `{q: 2, rechecks_at_set: 1}`; at 8 it expires and marks both; a decision on #9
   records the assumption; a decision on #10 would emit the second `PqAssumed`. The model reads both
   clauses with `assumed_by = None` and emits nothing for the second; if Rust follows the literal
   text, conformance diverges on `anomalies`. Suggested wording: "`PqAssumed` if `q` is a people
   question still `Expired` with `assumed_by = None`". The model has no event layer, so its
   `anomalies` stays 0 by construction, and the replay compares Rust's with 0.
3. **"The scaling adds no equality"** (D.10.5). True of the durations pairwise, not of the
   comparison `3 × presence.resolution <= presence.silence` of `Params::valid`: 9 < 15 minutes is
   3 = 3 ticks, so the model runs at the boundary `Params::valid` allows (silence at least two thirds
   of `presence.silence` after the last call, where the real values give four fifths). No rule of §26
   reads the difference. Making it strict needs `presence.silence = 4` ticks, which breaks its
   equality with `lease.first` (both 15 minutes) or moves the whole table. `params.qnt` pins the
   current state (`validCombinationsTest`); D.10.5's sentence needs a correction or a new table.
4. **D.10.3 and D.10.4** need the amendments of "Deviations" above: status apart from content, the
   object key, and the command fields. WP-A2 builds on the model's types; D.10.8's replay maps
   status and `Obj` as `types.qnt` says.
5. **The bound on occurrences** (§28, D.10.5, D.10.7): either D.10.5 takes period 15, or §28 states
   4 occurrences. WP-A2 guards the commands that open an occurrence at `now` with
   `may_open_occurrence` either way.
6. **Apalache at depth 12** (D.10.7, WP-A3): see "Cost of settling in Apalache". If WP-A3 cannot
   bring a slice's settle within reach, it is decided whether Apalache runs at a smaller depth or
   with fewer firings, TLC being the exhaustive check. WP-A3 ran it on every slice module: item 19.

Found while writing WP-A2:

7. **`route_to` after `node key {add}` on a closed node** (D.8.9, D.8.11, D.7.4). `node key {add}`
   emits `KeyAdded { n, user:e<e> }` and then `route_to(e, n)` for an unrouted entry, and `route_to`
   begins "if no open node carries `user:e<e>`, `KeyAdded { n, … }`". When `n` is closed, no open node
   carries the key, so the second `KeyAdded` fails its precondition ("does not carry the key"), an
   anomaly. Trace (`routeToClosedNodeTest`): `talk from_user` (entry 1, unrouted); `node close
   /w1`; `node key /w1 {add: user:e1}`. The model emits the `KeyAdded` once and records the request
   on `/w1`; if Rust follows the literal text, conformance diverges on `anomalies`. Suggested wording
   for `route_to`: "if `n` does not carry `user:e<e>` and no open node does, `KeyAdded`".
8. **The occurrence bound after a reslot** (§28, D.10.7, item 5). The guard disables a command that
   would make a fourth occurrence, but it cannot see the occurrences time will open after the
   command. Trace (`occurrenceBoundTest`): at 1 `update /s {repeat: */20}` drops the waiting
   occurrence #11 and opens #12 (slot 20); at 2 `update /s {repeat: */15}` drops #12 and opens #13
   (slot 15): three made, each command within the bound. Then time alone misses #13 at 19, which
   opens #14 (slot 30), and misses #14 at 34, which opens #15 (slot 45): five by MAX_TIME. The model
   stays finite (each further occurrence needs a slot that passes within MAX_TIME), so TLC
   terminates; only the stated bound is exceeded. Options: state the bound as "3 occurrences opened
   by commands, and those time opens after them", or disable a reslot (and every command that opens
   an occurrence at `now`) once one has been made, at the cost of `update {repeat}` coverage in the
   `recurring` slice.
9. **Apalache and TLC need a smaller compiled model** (D.10.1, D.10.7, WP-A3): see "The size of the
   compiled main module". Slices that instantiate `forum.qnt` do not compile with Quint 0.33.0,
   whatever actions they step. Options for WP-A3: split `commands.qnt` by command family, with a
   `forum` core (constants, variables, `init`, the `command` wrapper) that a slice imports with only
   its families; or a smaller abstract state per slice; or a Quint whose compiler streams its JSON.
   Each changes D.10.1's layout, so it needs a decision; simulation and `quint test` are unaffected.
   Settled by WP-A3 without a change of layout: `scripts/compile-pruned.js` writes only what
   Apalache reads, about 530 MB per slice module ("The size of the compiled main module").

Found by the review of WP-A2 and reproduced; the specification and the design were not changed:

10. **The order of two validation checks** (D.8.24, D.8.11, D.8.20). D.8.24's general order puts
    the other references (`not_found`, step 5) before the `invalid` checks by state (step 7), and
    its table lists one leading `nf` for `talk from_user` ("nf, invalid(NotUserQuestion,
    ReservedNode, AmbiguousPrincipal)") and for `node move` ("nf, invalid(ReservedNode), cycle, …").
    The command sections order them otherwise: D.8.11 checks `answers` (`invalid(NotUserQuestion)`)
    before `node` resolves, and D.8.20 checks `invalid(ReservedNode)` before the new parent. Both
    are reachable from the actions. Trace (`validationOrderTest`): `talk ask` to a person on /w1
    (people question 2); `talk from_user {answers: 2, node: 99}` gives `invalid(NotUserQuestion)` in
    the model, where D.8.24's order gives `not_found(Node)`; `node move {node: ROOT, parent: 99}`
    gives `invalid(ReservedNode)`, where D.8.24's gives `not_found(Node)`. The model follows the
    command sections (`commands.qnt`, `v_from_user`, `v_move`). Conformance compares the codes, so
    one reading has to be chosen, and D.8.24's table, or D.8.11 and D.8.20, corrected to match; the
    replay driver follows the same reading.
11. **`heard` and `ask` without `where`** (D.10.4, 1.3). D.10.4 gives `act_said` a `where` and
    `act_heard` and `act_ask_person` none, so on a node that carries two conversation keys (after a
    merge, say) every `heard` and every `ask` to a person is `invalid(AmbiguousConversationKey)` in
    the model, while Rust's calls take `where` (§9.3). A people question on such a node exists only
    if it was asked before the keys met. If the `people` slice of WP-A3 should ask and hear on a
    node with two conversations, D.10.4 needs `where` for both actions too.
12. **The ITF of `--mbt` traces** (D.10.7, D.10.8, WP-F1). Quint 0.33's `--out-itf` placeholder is
    `{seq}`, not `{}` ("Deviations"). The traces' state variables are qualified by module:
    `two_class::forum::s`, `two_class::forum::clock`, `two_class::forum::prev`,
    `two_class::forum::last_action`, beside `mbt::actionTaken` and `mbt::nondetPicks`, while
    D.10.8's projection reads `state: &["s", "clock"]`. Whether quint-connect strips the prefixes is
    not checked; WP-F1's driver must map the qualified names, or D.10.8's `Config` must name them.
    The picks drawn in two steps (`roll`, `wait_group`, `basis_group`, `at_group`) appear in
    `mbt::nondetPicks` beside the named parameters, and the driver ignores them.

Found while writing WP-A4:

13. **Replaying `scenario_32`** (D.10.8, WP-F1). D.10.8 replays `scenario_32` with
    `#[quint_test(spec = "../../model/tests/forum_test.qnt", test = "scenario_32")]` and a driver
    that dispatches on action names (`switch!`). Quint 0.33's `quint test --out-itf` writes the
    state variables only, qualified by module (checked with a probe on 2026-10-02): no
    `mbt::actionTaken` and no `mbt::nondetPicks`, and `quint test` has no `--mbt`. Whether
    quint-connect's `quint_test` gets the action names some other way is not checked. Everything a
    replay needs is in the state variable `last_action`: the name of D.10.4 (`act_take`, `act_tick`,
    and so on), the session, the class, the `op`, `long`, the command with its parameters, and the
    outcome, which `call32` and `tick32` record as `command` and `act_tick` do. So WP-F1's driver
    can dispatch on `last_action`; if it must use `mbt::actionTaken`, the scenario needs another
    route. Separately, `scenario_32` runs outside D.10.7's bounds (nine sessions and more entries
    than MAX_EID; "Deviations"), which the driver must accept, as Rust does.
14. **A clarification after its request has settled** (§8.1, §32 step 1, D.12.2, §26 M1a).
    D.12.2 scripts step 1 as `decide {settles: [r1]}`, `to_user {kind: result, re: [e1]}`, then
    `from_user {clarifies: r1}`; §30's flow puts clarifications before the decision and the
    report. In D.12.2's order the clarification ("and skip tickets tagged spam") attaches to a
    request that is already settled, and settling is final, so nothing in the state asks for it
    to be applied: no request is unsettled, M1a lists no clarification among the waiting objects,
    and no node carries a flag. Trace (`scenario_32`, after `STEP_1_CLARIFY`, tick 0): r1 is
    `SettledBy({ decision: 5, answer: 6 })`; entry 7 is routed `RtClarifies(4)`, and `@user`
    carries no flag (`after_1_clarify`); the assistant's own hold on the series (13, 1) expires
    unchecked at 3, after which the series carries no flag either (`after_4_later`), and at the
    end of §32 no node does (`after_32`). If the assistant vanished after recording the
    clarification, nothing would bring an agent to apply it to the series, whose content the
    script never changes. §8.1 promises only that "the work and every clarification remain".
    Options: (a) §32 and D.12.2 put the clarification before the decision and the report, as §30
    does; or (b) a clarification of a settled request is routed as a new request, or left
    unrouted like a late answer (§8.4), so that it gets a mover. The model follows D.12.2's
    script and the current rules.

Found by the review of WP-A4 and reproduced; the specification and the design were not changed:

15. **The scaled §32 passes `session.retain`** (D.10.5, D.12.2, D.10.8; WP-D6, WP-F1). D.10.5
    keeps the order of each duration, not that of their sums. D.12.2's "advance 2 h" (7 ticks) and
    "advance 21 min" (4 ticks) add up to 11 ticks, past `session.retain` (24 h, 10 ticks), while
    D.12.2's run on the real clock goes from 09:00 to 11:21. No mapping avoids it: 2 h lies
    strictly between `commitment.followup` (6) and `question.recheck` (8), so it is 7 ticks, and
    s-5's lease, extended by `lease.default` (4) at 7, ends at 11. Trace (`scenario_32`, part "6
    vanish", one tick at a time from 7): at 10, sessions 7 and 8 (s-4 and s-5) fall silent and
    sessions 1 to 6 (the seed's two, the assistant, s-1, s-2, and s-3) retire; at 11, s-5's lease
    ends; `after_6_vanish` asserts `sessions.keys() == Set(7, 8)`. In D.12.2's run all nine
    sessions are present, silent, from step 6 on. No assertion of D.12.2 reads `sessions`, and the
    retired sessions hold nothing and have no pending `said`, so nothing D.12.2 asserts changes.
    D.10.8's replay of `scenario_32` uses the scaled params, so Rust retires them too and the
    states compare equal; WP-D6's `scenario_32.rs` on real params differs from the model's run in
    `sessions` from step 6 on, so a field-by-field comparison of the two must leave `sessions` out
    there. Options: (a) D.10.5 or D.12.2 says so; or (b) the model's scenario is compared with Rust
    only through D.10.8's replay on the scaled params, never with WP-D6's run on real params.
16. **WP-A3's acceptance and `scenario_32`** (D.13.2, D.12.2). D.12.2 asks for
    `check_invariants`, all of §26, after every step. `scenario_32` checks `inv_all` after every
    call and every tick, and `inv_all` is still WP-A2's basic set: Settled, C1, N1, M1a, F, W1, D1,
    and E1 are not in it. WP-A3 replaces `inv_all` with the conjunction of D.9.2, and `scenario_32`
    names `inv_all`, so the run checks §26 once WP-A3 lands, but D.13.2 lists `scenario_32` only
    in WP-A4's acceptance. Suggested: WP-A3's acceptance includes `npx quint test
    tests/forum_test.qnt` (which runs `scenario32Test` and `t2Test`) with its `inv_all`. Done in
    WP-A3: `scenario_32` passes with the full `inv_all`, and it is in WP-A3's report.

Item 9 above was confirmed by the review of WP-A2, and again after its fixes: a three-action slice
(`act_tick`, `act_take`, `act_release`) importing `forum.qnt` with the one-class constants,
`MAX_FIRINGS = 4`, and `RARE_ODDS = 1` failed `quint compile --target json` with the same
`RangeError: Invalid string length` (Node's largest string), after about 5.5 minutes (with tests and
simulations running beside it). WP-A3 compiles the slices with `scripts/compile-pruned.js` instead,
so item 9 needs no decision.

Found while writing WP-A3; the specification and the design were not changed:

17. **R2's step half and a move** (§26 R2, D.9.2, D.8.20, A-80). D.9.2 has a `missed` close happen
    "only on a node with no checkpoint, no open intent, and no live hold in `before`". A move by
    another session ends the holder's hold (`EndedByClose(Move)`), and D.8.20 then runs the missed
    check, which A-80 runs "after every hold end that leaves the occurrence open (lease expiry,
    release, move)". So an occurrence held in its window and moved by another session closes as
    `missed` in a step whose `before` has a live hold. Trace (`r2MoveTest`): the seed with the series
    `/s`, sessions 1 and 2 kept live by the fixture (touched at 14); at 17 occurrence #11 is
    available and session 1 takes it (lease until 20); at 19 its window
    ends (it stays open, held); at 19 session 2 moves #11 under ROOT: the hold ends and #11 closes as
    `missed`, with `before` holding `(11, 1)`, live until 20. D.9.2's literal clause fails on this
    step (`r2_literal` in the test); §26 R2 says only "an occurrence with a checkpoint never closes as
    `missed`, and only the server closes a node as `missed`", which holds. `step_R2` excepts the holds
    of other sessions that the step's own move ended, and nothing else. Options: (a) D.9.2's clause
    reads "no live hold in `before` other than those the step's move ended"; or (b) D.8.20 runs no
    missed check after a move, and an occurrence moved in its window stays open until its next
    check (which changes A-80).
18. **U1's chain "at creation"** (§26 U1, D.9.2, D.8.20, D.8.14). D.9.2 compares a new question's
    chain with `spec::chain(before, about, …)`, on the state before the step. An `update` that adds a
    need to a refused node and sets its owners in one call makes every class inactive for it, and
    `e1_check` creates the refusal question after `NodeUpdated` (D.8.20), with the chain of the
    updated node: its new owner first. Trace (`u1ChainAtCreationTest`): session 2 (class b) calls
    `next` (class b active for no needs); session 3 (class a) opens; a person #12 with key `p1`;
    sessions 1 and 3 take and release `/w2` with `cannot` (class a refuses it at QUORUM 2); then
    `update /w2 {needs_add: x, owners: [12]}` by session 1: class b's caps profile has no `x`, so no
    class is active for `/w2` and class a has refused it, which E1 (a) counts as every class
    (`e1_check` asks); the refusal question's chain is `[p1, user]`, where `chain(before, /w2, …)` is
    `[user]`. §26 U1 says "at creation, the chain follows §8.4", which the model reads on the state
    at creation, the state after the step (nothing a command or a firing does after it creates a
    question changes what `chain` reads). Option: D.9.2 reads the chain on the state at creation
    (`check_step` has it: the state after the step), or `e1_check` in `update` runs before
    `NodeUpdated` (which changes D.8.20).

Found while making the commands post-settle (D.8.1), on the model as it was before that change too:

20. **A merge of a series into its own occurrence** (§16, D.8.20, D.8.18, §26 N5). Simulation of
    `two_class` (seed `0x34e51994d0f1ba43`, its first run, the same at e33156f) ends with session 1's
    `node merge {from: /s, into: #11}`, #11 being `/s`'s open, available occurrence, while `/s`
    carries an assume mark. The merge closes `/s` as `superseded` with `merged_into: #11`, and
    closing a series drops its open occurrence (D.8.18), so `into` itself ends closed `dropped`,
    without the marks it received, and whatever else the merge moves onto it lands on a closed
    node. `step_N5` fails on "the marks of both nodes are on `into` after the merge". N5
    is outside D.0.3's key set. Options: refuse a merge into an open occurrence of `from` (as a
    `cycle` or an `invalid`), or keep the occurrence open when its series is merged into it, or
    except the case in N5; the core's behaviour on this call should be checked either way.
    Resolved by 1.5 (§8, D.8.20, D.14.7): `node merge` refuses `into` = the open occurrence of
    `from`, a series, with `invalid(MergeIntoOwnOccurrence)`, after `OccurrenceNotYetAvailable`
    and before `cycle`. `v_merge` checks it (`mergeIntoOwnOccurrenceTest`), and the seed above
    runs 300 samples of 40 steps with no violation. The core refuses it with WP-D6; until then
    the replay driver (`crates/forum-conformance`) reports the model's refusal against a core that
    merged as a gap naming WP-D6, not as a divergence.
