// -*- mode: Bluespec; -*-
/**
 * The `basis` slice of D.10.7 (D-4, CS-22; WP-A3): create, take, next, close (with the outcomes
 * `done`, `failed`, and `dropped`), reopen, merge, decide (with `supersedes`, and a basis of node and
 * entry references), retract (a decision, another entry, a node), object, resolve, and tick.
 *
 * Invariants: F (`basis_changed`: one hop, set by a close with another outcome, a retraction, or a
 * supersession, never by a reopen or a merge; cleared by a supersession or a retraction of the
 * citing decision), W1, M1, O3 (`retracted` only grows), and the six of every slice
 * (`common.qnt`). Close and merge name ROOT too (K8).
 *
 *   scripts/verify-file.sh apalache slices/basis.qnt --main basis_two --step step_basis --invariant inv_basis
 *   scripts/verify-file.sh tlc slices/basis.qnt --main basis_two --step step_basis --invariant inv_basis
 *
 * and the same with `basis_one`.
 */
module basis {
  import types.* from "../types"
  import params.* from "../params"
  import basics.* from "../basics"
  import flags.* from "../flags"
  import deadlines.* from "../deadlines"
  import commands.* from "../commands"
  import invariants.* from "../invariants"
  import common.* from "./common"

  /// Leases, silences, and retirements, with room.
  pure val BASIS_FIRINGS = 24

  pure def b_nodes(st: State): Set[int] = slice_nodes(st, Set(ROOT))

  pure def b_creates(st: State): Set[Cmd] =
    tuples(Set(Lit("a1"), Lit("a2")), Set(false, true)).map(((nm, take)) => create_cmd(ROOT, nm, Set(), take, 1, Set(), RepKeep))

  pure def b_takes(st: State): Set[Cmd] = b_nodes(st).map(n => CTake({ node: n, lease: -1 }))

  pure def b_closes(st: State): Set[Cmd] =
    tuples(b_nodes(st), Set(Done, Failed, Dropped)).map(((n, o)) => CClose({ node: n, outcome: o, settles: Set(), token: NoToken }))

  pure def b_reopens(st: State): Set[Cmd] =
    st.nodes.keys().filter(n => st.nodes.get(n).life != Open).union(Set(ROOT)).map(n => CReopen(n))

  pure def b_merges(st: State): Set[Cmd] =
    tuples(made_nodes(st), made_nodes(st).union(Set(ROOT))).map(((f, i)) => CMerge({ from: f, into: i }))

  /// The bases of a decision (CS-22, at most two references): none, a node, a decision, two
  /// nodes, or a node and a decision.
  pure def b_bases(st: State): Set[List[BasisArg]] = {
    val ns = made_nodes(st)
    val ds = st.decisions.keys()
    Set([]).union(ns.map(n => [BaNode({ id: n, version: -1 })]))
      .union(ds.map(d => [BaEntry(d)]))
      .union(tuples(ns, ns).filter(p => p._1 < p._2).map(p => [BaNode({ id: p._1, version: -1 }), BaNode({ id: p._2, version: -1 })]))
      .union(tuples(ns, ds).map(p => [BaNode({ id: p._1, version: -1 }), BaEntry(p._2)]))
  }

  pure def b_decides(st: State): Set[Cmd] =
    tuples(made_nodes(st), Set(-1).union(st.decisions.keys()), b_bases(st)).map(((at, sup, b)) =>
      CDecide({ at: at, settles: Set(), supersedes: sup, basis: b }))

  /// A decision, another kept entry, or a node (D-4).
  pure def b_retracts(st: State): Set[Cmd] =
    st.entries.keys().map(e => CRetract(TEntry(e))).union(made_nodes(st).map(n => CRetract(TNode(n))))

  pure def b_objects(st: State): Set[Cmd] = made_nodes(st).map(n => CObject(n))

  pure def b_resolves(st: State): Set[Cmd] = st.objections.keys().map(o => Set(false, true).map(u => CResolve({ objection: o, upheld: u, defer: false }))).flatten()

  /// The invariants of the slice (D.10.7), on the action from `pre` through `cmd_post` (after its
  /// command, before its settle) to `post`.
  pure def basis_invariants(pre: State, cmd_post: State, post: State, clk: int, la: Action, max_firings: int): bool = {
    val cs = steps_of(pre, cmd_post, post, clk, la, max_firings)
    and {
      common_invariants(cs, post, clk, la),
      // 1.9 (§10.1, §10.4): H6 where closed nodes carry objections (as in `people`).
      inv_H6(post),
      inv_F(post, clk), inv_W1(post, clk), inv_M1(post, clk),
    }
  }
}

module basis_two {
  import types.* from "../types"
  import params.* from "../params"
  import common.* from "./common"
  import basis.*
  import forum(
    CLASSES = Set("a", "b"),
    USE_FNV = false,
    QUORUM = 2,
    PRINCIPALS = Set("user", "p1", "p2"),
    HAS_USER = true,
    MAX_FIRINGS = 24,
    SEED_SERIES = false,
    LONG_TEXTS = Set(false),
    RARE_ODDS = 1
  ).* from "../forum"

  action basis_calls = any {
    slice_call("act_create", b_creates(s), PLAIN),
    slice_call("act_take", b_takes(s), PLAIN),
    slice_call("act_next", Set(CNext), PLAIN),
    slice_call("act_close", b_closes(s), PLAIN),
    slice_call("act_reopen", b_reopens(s), PLAIN),
    slice_call("act_merge", b_merges(s), PLAIN),
    slice_call("act_decide", b_decides(s), PLAIN),
    slice_call("act_retract", b_retracts(s), PLAIN),
    slice_call("act_object", b_objects(s), PLAIN),
    slice_call("act_resolve", b_resolves(s), PLAIN),
  }

  action step_basis = any { basis_calls, act_tick }

  val inv_basis = basis_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS)
}

module basis_one {
  import types.* from "../types"
  import params.* from "../params"
  import common.* from "./common"
  import basis.*
  import forum(
    CLASSES = Set("w"),
    USE_FNV = false,
    QUORUM = 1,
    PRINCIPALS = Set("user", "p1", "p2"),
    HAS_USER = true,
    MAX_FIRINGS = 24,
    SEED_SERIES = false,
    LONG_TEXTS = Set(false),
    RARE_ODDS = 1
  ).* from "../forum"

  action basis_calls = any {
    slice_call("act_create", b_creates(s), PLAIN),
    slice_call("act_take", b_takes(s), PLAIN),
    slice_call("act_next", Set(CNext), PLAIN),
    slice_call("act_close", b_closes(s), PLAIN),
    slice_call("act_reopen", b_reopens(s), PLAIN),
    slice_call("act_merge", b_merges(s), PLAIN),
    slice_call("act_decide", b_decides(s), PLAIN),
    slice_call("act_retract", b_retracts(s), PLAIN),
    slice_call("act_object", b_objects(s), PLAIN),
    slice_call("act_resolve", b_resolves(s), PLAIN),
  }

  action step_basis = any { basis_calls, act_tick }

  val inv_basis = basis_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS)
}
