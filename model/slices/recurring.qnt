// -*- mode: Bluespec; -*-
/**
 * The `recurring` slice of D.10.7 (WP-A3): create a series, take, next, checkpoint (with an
 * intent), release, close, update `repeat` (on open and closed series), reopen, move, tick, and
 * tick_next. Its seed has the series `/s` of SEED_REPEAT (D.10.5) and the first occurrence.
 *
 * Invariants: K6, R2, F (`overdue`, `missed`), T2, M1, and the six of every slice (`common.qnt`).
 * Liveness: `deadline_fires`, `occurrence_resolves`. Close, update, and move name ROOT too (K8).
 *
 *   scripts/verify-file.sh apalache slices/recurring.qnt --main recurring_two --step step_recurring --invariant inv_recurring_apalache
 *   scripts/verify-file.sh tlc slices/recurring.qnt --main recurring_two --step step_recurring --invariant inv_recurring
 *   scripts/verify-file.sh tlc slices/recurring.qnt --main recurring_two --step step_recurring --temporal liveness_recurring
 *
 * and the same with `recurring_one`.
 */
module recurring {
  import types.* from "../types"
  import params.* from "../params"
  import basics.* from "../basics"
  import flags.* from "../flags"
  import deadlines.* from "../deadlines"
  import commands.* from "../commands"
  import invariants.* from "../invariants"
  import common.* from "./common"

  /// Leases, slots, windows, dues, silences, and retirements, with room.
  pure val RECURRING_FIRINGS = 30

  /// A second schedule (period 20 divides 60, D.10.6), as `forum.qnt`'s ALT_REPEAT.
  pure val OTHER_REPEAT: RepeatSpec = { period: 20, offset: 0, run_within: 5 }

  pure def r_nodes(st: State): Set[int] = slice_nodes(st, Set(ROOT))

  /// A series under ROOT, and a plain node under ROOT or under the seed's series (`/s/a1`).
  pure def r_creates(st: State): Set[Cmd] = Set(
    create_cmd(ROOT, Lit("a1"), Set(), false, 1, Set(), RepSet(SEED_REPEAT)),
    create_cmd(ROOT, Lit("a2"), Set(), true, 1, Set(), RepKeep),
    create_cmd(SEED_SERIES_NODE, Lit("a1"), Set(), false, 1, Set(), RepKeep),
  )

  pure def r_takes(st: State): Set[Cmd] = r_nodes(st).map(n => CTake({ node: n, lease: -1 }))

  pure def r_checkpoints(st: State): Set[Cmd] =
    tuples(slice_tokens(st), Set((false, false), (true, false), (false, true))).map(((t, x)) =>
      CCheckpoint({ token: t, intent: x._1, intent_done: x._2 }))

  pure def r_releases(st: State): Set[Cmd] = slice_tokens(st).map(t => CRelease({ token: t, reason: Handoff, wait: WaNone, settles: Set() }))

  /// `done` and `dropped` (a waiting occurrence closes only as `dropped`), and `missed`, which only
  /// the server gives (`invalid(ServerOutcome)`, R2).
  pure def r_closes(st: State): Set[Cmd] =
    tuples(r_nodes(st), Set(Done, Dropped, Missed)).map(((n, o)) => CClose({ node: n, outcome: o, settles: Set(), token: NoToken }))

  /// `repeat` changed, cleared, or added, on open and closed nodes.
  pure def r_updates(st: State): Set[Cmd] =
    tuples(made_nodes(st).union(Set(ROOT)), Set(RepClear, RepSet(OTHER_REPEAT), RepSet(SEED_REPEAT))).map(((n, r)) =>
      update_cmd(st, n, { ...NO_UPDATE, repeat: r }))

  pure def r_reopens(st: State): Set[Cmd] =
    st.nodes.keys().filter(n => st.nodes.get(n).life != Open).union(Set(ROOT)).map(n => CReopen(n))

  /// A move under ROOT or `/w1` (an occurrence moved by another session ends its hold, A-80).
  pure def r_moves(st: State): Set[Cmd] =
    tuples(r_nodes(st), Set(ROOT, SEED_W1)).map(((n, p)) => CMove({ node: n, parent: p, name: Lit("a1") }))

  /// The invariants of the slice (D.10.7), on the action from `pre` through `cmd_post` (after its
  /// command, before its settle) to `post`.
  pure def recurring_invariants(pre: State, cmd_post: State, post: State, clk: int, la: Action, max_firings: int, t2_k: int): bool = {
    val cs = steps_of(pre, cmd_post, post, clk, la, max_firings)
    and {
      common_invariants(cs, post, clk, la),
      inv_K6(post), inv_R2(post), inv_F(post, clk), inv_M1(post, clk),
      inv_T2(post, clk, t2_k, max_firings),
      each_step(cs, c => step_R2(c)),
    }
  }
}

module recurring_two {
  import types.* from "../types"
  import params.* from "../params"
  import common.* from "./common"
  import recurring.*
  import forum(
    CLASSES = Set("a", "b"),
    USE_FNV = false,
    QUORUM = 2,
    PRINCIPALS = Set("user", "p1", "p2"),
    HAS_USER = true,
    MAX_FIRINGS = 30,
    SEED_SERIES = true,
    LONG_TEXTS = Set(false),
    RARE_ODDS = 1
  ).* from "../forum"

  action recurring_calls = any {
    slice_call("act_create", r_creates(s), PLAIN),
    slice_call("act_take", r_takes(s), PLAIN),
    slice_call("act_next", Set(CNext), PLAIN),
    slice_call("act_checkpoint", r_checkpoints(s), PLAIN),
    slice_call("act_release", r_releases(s), PLAIN),
    slice_call("act_close", r_closes(s), PLAIN),
    slice_call("act_update", r_updates(s), PLAIN),
    slice_call("act_reopen", r_reopens(s), PLAIN),
    slice_call("act_move", r_moves(s), PLAIN),
  }

  action step_recurring = any { recurring_calls, act_tick, act_tick_next }

  val inv_recurring = recurring_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, T2_K)
  val inv_recurring_apalache = recurring_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, T2_K_APALACHE)

  temporal liveness_recurring = and {
    weakFair(act_tick, (s, clock, prev, mid, last_action)),
    weakFair(act_tick_next, (s, clock, prev, mid, last_action)),
    weakFair(recurring_calls, (s, clock, prev, mid, last_action)),
  } implies and { deadline_fires, occurrence_resolves }
}

module recurring_one {
  import types.* from "../types"
  import params.* from "../params"
  import common.* from "./common"
  import recurring.*
  import forum(
    CLASSES = Set("w"),
    USE_FNV = false,
    QUORUM = 1,
    PRINCIPALS = Set("user", "p1", "p2"),
    HAS_USER = true,
    MAX_FIRINGS = 30,
    SEED_SERIES = true,
    LONG_TEXTS = Set(false),
    RARE_ODDS = 1
  ).* from "../forum"

  action recurring_calls = any {
    slice_call("act_create", r_creates(s), PLAIN),
    slice_call("act_take", r_takes(s), PLAIN),
    slice_call("act_next", Set(CNext), PLAIN),
    slice_call("act_checkpoint", r_checkpoints(s), PLAIN),
    slice_call("act_release", r_releases(s), PLAIN),
    slice_call("act_close", r_closes(s), PLAIN),
    slice_call("act_update", r_updates(s), PLAIN),
    slice_call("act_reopen", r_reopens(s), PLAIN),
    slice_call("act_move", r_moves(s), PLAIN),
  }

  action step_recurring = any { recurring_calls, act_tick, act_tick_next }

  val inv_recurring = recurring_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, T2_K)
  val inv_recurring_apalache = recurring_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, T2_K_APALACHE)

  temporal liveness_recurring = and {
    weakFair(act_tick, (s, clock, prev, mid, last_action)),
    weakFair(act_tick_next, (s, clock, prev, mid, last_action)),
    weakFair(recurring_calls, (s, clock, prev, mid, last_action)),
  } implies and { deadline_fires, occurrence_resolves }
}
