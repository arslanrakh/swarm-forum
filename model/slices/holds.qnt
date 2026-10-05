// -*- mode: Bluespec; -*-
/**
 * The `holds` slice of D.10.7 (WP-A3): hello, create (no keys), take, next, checkpoint, the check
 * that records what became of an intent (1.8), renew,
 * release (no wait), close, update (capacity; a digest with and without `digest_version`, CS-11),
 * pause, unpause, view, tick, and tick_next, so that sessions retire and `op_id` records expire
 * (CS-8). The writes carry an `op` or a long text (D.10.4): O1 meets a replay, a conflict, and a
 * long repeat of a recorded `op`, and H4 a long stale write, which saves no `late` entry; `create`,
 * `update`, and `pause`, whose commands carry a text too, pick a long one.
 *
 * Invariants: H1 to H6, O1, O2, P1, P2, T1, T2, L1, M1, and the six of every slice (`common.qnt`).
 * Liveness: `deadline_fires`, `lease_ends`, `pause_ends`. `close` and `update` name ROOT and `@user`
 * too, so that K8's refusals are exercised.
 *
 *   scripts/verify-file.sh apalache slices/holds.qnt --main holds_two --step step_holds --invariant inv_holds_apalache
 *   scripts/verify-file.sh tlc slices/holds.qnt --main holds_two --step step_holds --invariant inv_holds
 *   scripts/verify-file.sh tlc slices/holds.qnt --main holds_two --step step_holds --temporal liveness_holds
 *
 * and the same with `holds_one`.
 */
module holds {
  import types.* from "../types"
  import params.* from "../params"
  import basics.* from "../basics"
  import flags.* from "../flags"
  import deadlines.* from "../deadlines"
  import commands.* from "../commands"
  import invariants.* from "../invariants"
  import common.* from "./common"

  /// The firings one settle of the slice may make (D.10.4): up to eight leases (four sessions of
  /// two holds), four silences and four retirements, and a pause on each node, with room.
  pure val HOLDS_FIRINGS = 24

  /// The reserved nodes the slice names: ROOT and `@user` (K8).
  pure val HOLDS_RESERVED: Set[int] = Set(ROOT, USER)

  /// A write's `op` and long text: none; `o1`; and `o1` with a long text, a long repeat of a
  /// recorded `op` (O1) and a long stale write (H4, L1).
  pure val WRITE_OPS: Set[(str, bool)] = Set(("", false), ("o1", false), ("o1", true))

  /// A take carries no text: none or `o1`.
  pure val TAKE_OPS: Set[(str, bool)] = Set(("", false), ("o1", false))

  /// An update, a create (its title), and a pause (its reason) without and with a long text (L1's
  /// text half: every action of the slice whose command carries a text picks `long`, D.10.4).
  pure val TEXT_OPS: Set[(str, bool)] = Set(("", false), ("", true))

  pure def h_nodes(st: State): Set[int] = slice_nodes(st, HOLDS_RESERVED)

  /// `node create`, no keys: under ROOT or `/w1`, held or not, of each capacity.
  pure def h_creates(st: State): Set[Cmd] =
    tuples(Set(ROOT, SEED_W1), Set(false, true), CAPACITIES).map(((p, take, cap)) =>
      create_cmd(p, Lit("a1"), Set(), take, cap, Set(), RepKeep))

  /// `hold take` with the default lease and with one beyond `lease.max` (clamped, A-8).
  pure def h_takes(st: State): Set[Cmd] =
    tuples(h_nodes(st), Set(-1, 8)).map(((n, l)) => CTake({ node: n, lease: l }))

  pure def h_checkpoints(st: State): Set[Cmd] =
    tuples(slice_tokens(st), Set(false, true), Set(false, true)).map(((t, i, d)) =>
      CCheckpoint({ token: t, intent: i, intent_done: d }))

  pure def h_renews(st: State): Set[Cmd] = slice_tokens(st).map(t => CRenew(t))

  /// 1.8 (§10.2): `hold check {target, intent_done}` on each node, without a hold.
  pure def h_intent_checks(st: State): Set[Cmd] = h_nodes(st).map(n => CCheckIntent(n))

  /// `hold release` without a wait, handed off or refused (`cannot`, CS-7).
  pure def h_releases(st: State): Set[Cmd] =
    tuples(slice_tokens(st), Set(Handoff, Cannot)).map(((t, r)) => CRelease({ token: t, reason: r, wait: WaNone, settles: Set() }))

  /// `node close {done}`, without a token and with each token (H4: a stale token saves a `late`
  /// entry).
  pure def h_closes(st: State): Set[Cmd] =
    tuples(h_nodes(st), Set(NoToken).union(slice_tokens(st).map(t => SomeToken(t)))).map(((n, t)) =>
      CClose({ node: n, outcome: Done, settles: Set(), token: t }))

  /// `node update`: capacity 0 or 2 (`full` below the live holds), or a digest with and without
  /// `digest_version` (`invalid(MissingDigestVersion)`, CS-11).
  pure def h_updates(st: State): Set[Cmd] =
    h_nodes(st).map(n => {
      val dv = if (st.nodes.keys().contains(n)) st.nodes.get(n).digest_version else 1
      Set(
        update_cmd(st, n, { ...NO_UPDATE, capacity: 0 }),
        update_cmd(st, n, { ...NO_UPDATE, capacity: 2 }),
        update_cmd(st, n, { ...NO_UPDATE, digest: 1, dver: dv }),
        update_cmd(st, n, { ...NO_UPDATE, digest: 1 }),
      )
    }).flatten()

  pure def h_pauses(st: State): Set[Cmd] = h_nodes(st).map(n => CPause(n))

  pure def h_unpauses(st: State): Set[Cmd] = st.pauses.keys().union(Set(ROOT)).map(n => CUnpause(n))

  pure def h_views(st: State): Set[Cmd] = h_nodes(st).map(n => CView(n))

  /// The invariants of the slice (D.10.7), with T2 over pairs of times up to `t2_k` ticks ahead,
  /// on the action from `pre` through `cmd_post` (after its command, before its settle) to `post`.
  pure def holds_invariants(pre: State, cmd_post: State, post: State, clk: int, la: Action, max_firings: int, t2_k: int): bool = {
    val cs = steps_of(pre, cmd_post, post, clk, la, max_firings)
    and {
      common_invariants(cs, post, clk, la),
      inv_H1(post, clk), inv_H2(post, clk), inv_H3(post), inv_H5(post), inv_H6(post),
      inv_O1(post), inv_O2(post),
      inv_P1(post, clk),
      inv_T1(post, clk),
      inv_T2(post, clk, t2_k, max_firings),
      inv_L1(post),
      inv_M1(post, clk),
      each_step(cs, c => and {
        step_H2(c), step_H3(c), step_H4(c), step_O1(c), step_O2(c), step_P2(c), step_T1(c), step_L1(c),
      }),
    }
  }
}

/// Two classes of two sessions, `QUORUM = 2` (D.10.1).
module holds_two {
  import types.* from "../types"
  import params.* from "../params"
  import common.* from "./common"
  import holds.*
  import forum(
    CLASSES = Set("a", "b"),
    USE_FNV = false,
    QUORUM = 2,
    PRINCIPALS = Set("user", "p1", "p2"),
    HAS_USER = true,
    MAX_FIRINGS = 24,
    SEED_SERIES = false,
    LONG_TEXTS = Set(false, true),
    RARE_ODDS = 1
  ).* from "../forum"

  action holds_calls = any {
    slice_hello(CAPS_CHOICES, Set("")),
    slice_call("act_create", h_creates(s), TEXT_OPS),
    slice_call("act_take", h_takes(s), TAKE_OPS),
    slice_call("act_next", Set(CNext), PLAIN),
    slice_call("act_checkpoint", h_checkpoints(s), WRITE_OPS),
    slice_call("act_check_intent", h_intent_checks(s), WRITE_OPS),
    slice_call("act_renew", h_renews(s), WRITE_OPS),
    slice_call("act_release", h_releases(s), WRITE_OPS),
    slice_call("act_close", h_closes(s), WRITE_OPS),
    slice_call("act_update", h_updates(s), TEXT_OPS),
    slice_call("act_pause", h_pauses(s), TEXT_OPS),
    slice_call("act_unpause", h_unpauses(s), PLAIN),
    slice_call("act_view", h_views(s), PLAIN),
  }

  action step_holds = any { holds_calls, act_tick, act_tick_next }

  val inv_holds = holds_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, T2_K)
  val inv_holds_apalache = holds_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, T2_K_APALACHE)

  temporal liveness_holds = and {
    weakFair(act_tick, (s, clock, prev, mid, last_action)),
    weakFair(act_tick_next, (s, clock, prev, mid, last_action)),
    weakFair(holds_calls, (s, clock, prev, mid, last_action)),
  } implies and { deadline_fires, lease_ends, pause_ends }
}

/// One worker class of two sessions, `QUORUM = 1` (§28).
module holds_one {
  import types.* from "../types"
  import params.* from "../params"
  import common.* from "./common"
  import holds.*
  import forum(
    CLASSES = Set("w"),
    USE_FNV = false,
    QUORUM = 1,
    PRINCIPALS = Set("user", "p1", "p2"),
    HAS_USER = true,
    MAX_FIRINGS = 24,
    SEED_SERIES = false,
    LONG_TEXTS = Set(false, true),
    RARE_ODDS = 1
  ).* from "../forum"

  action holds_calls = any {
    slice_hello(CAPS_CHOICES, Set("")),
    slice_call("act_create", h_creates(s), TEXT_OPS),
    slice_call("act_take", h_takes(s), TAKE_OPS),
    slice_call("act_next", Set(CNext), PLAIN),
    slice_call("act_checkpoint", h_checkpoints(s), WRITE_OPS),
    slice_call("act_check_intent", h_intent_checks(s), WRITE_OPS),
    slice_call("act_renew", h_renews(s), WRITE_OPS),
    slice_call("act_release", h_releases(s), WRITE_OPS),
    slice_call("act_close", h_closes(s), WRITE_OPS),
    slice_call("act_update", h_updates(s), TEXT_OPS),
    slice_call("act_pause", h_pauses(s), TEXT_OPS),
    slice_call("act_unpause", h_unpauses(s), PLAIN),
    slice_call("act_view", h_views(s), PLAIN),
  }

  action step_holds = any { holds_calls, act_tick, act_tick_next }

  val inv_holds = holds_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, T2_K)
  val inv_holds_apalache = holds_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, T2_K_APALACHE)

  temporal liveness_holds = and {
    weakFair(act_tick, (s, clock, prev, mid, last_action)),
    weakFair(act_tick_next, (s, clock, prev, mid, last_action)),
    weakFair(holds_calls, (s, clock, prev, mid, last_action)),
  } implies and { deadline_fires, lease_ends, pause_ends }
}
