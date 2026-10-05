// -*- mode: Bluespec; -*-
/**
 * The `keys` slice of D.10.7 (WP-A3): create with keys (a join, an object key beside a
 * conversation key, CS-36), key add and remove, link, move, merge, reopen, close, release with a
 * `{node}` wait, take and checkpoint (1.10: a checkpoint ends that wait), update (`depends_on`),
 * request, said, ask person, and heard, on the two conversation keys of the seed, so that C1 and N5
 * see requests, `said` entries, questions, and commitments; hide; and tick.
 *
 * Invariants: K1, K3, K4, K5, K7, S1, N5, T5, C1, O2, L1 (a reopen past `tree.max_children`, and a
 * `create` or `move` refused at it, CS-10; a `create` or `move` under `@people`, which the limit
 * never refuses, A-113), M1, and the six of every slice (`common.qnt`). Move, merge, close, and key
 * name ROOT too (K8).
 *
 *   scripts/verify-file.sh apalache slices/keys.qnt --main keys_two --step step_keys --invariant inv_keys
 *   scripts/verify-file.sh tlc slices/keys.qnt --main keys_two --step step_keys --invariant inv_keys
 *
 * and the same with `keys_one`.
 */
module keys_slice {
  import types.* from "../types"
  import params.* from "../params"
  import basics.* from "../basics"
  import flags.* from "../flags"
  import deadlines.* from "../deadlines"
  import commands.* from "../commands"
  import invariants.* from "../invariants"
  import common.* from "./common"

  /// The firings one settle of the slice may make: leases, silences, retirements, rechecks, and
  /// follow-ups, with room.
  pure val KEYS_FIRINGS = 30

  pure val KEYS_RESERVED: Set[int] = Set(ROOT, PEOPLE)

  pure def k_nodes(st: State): Set[int] = slice_nodes(st, Set(ROOT))

  /// Four places (two names under ROOT, so that the child limit is met; one under `/w1`; one under
  /// `@people`, where it never is), each with no key (held or not), `Conv("c1")` (a join with
  /// `/w1`), `Obj("o1")`, and both (a join that adds the object key).
  pure def k_creates(st: State): Set[Cmd] = {
    val places = Set((ROOT, Lit("a1")), (ROOT, Lit("a2")), (SEED_W1, Lit("a1")), (PEOPLE, Lit("a1")))
    val plain = tuples(places, Set(false, true)).map(((pl, take)) => create_cmd(pl._1, pl._2, Set(), take, 1, Set(), RepKeep))
    val keyed = tuples(places, Set(Set(Conv("c1")), Set(Obj("o1")), Set(Conv("c1"), Obj("o1")))).map(((pl, ks)) =>
      create_cmd(pl._1, pl._2, ks, false, 1, Set(), RepKeep))
    plain.union(keyed)
  }

  pure val KEY_CHOICES: Set[Key] = Set(Conv("c1"), Conv("c2"), Msg(("c1", 1)), Obj("o1"))

  pure def k_key_adds(st: State): Set[Cmd] =
    tuples(k_nodes(st), KEY_CHOICES).map(((n, k)) => CKeyAdd({ node: n, key: k }))

  pure def k_key_removes(st: State): Set[Cmd] =
    k_nodes(st).map(n => (if (st.nodes.keys().contains(n)) st.nodes.get(n).keys else Set()).union(Set(Obj("o1")))
      .map(k => CKeyRemove({ node: n, key: k }))).flatten()

  /// `duplicate` opens a signal (S1), `superseded_by` resolves the pair's.
  pure def k_links(st: State): Set[Cmd] =
    tuples(made_nodes(st), made_nodes(st), Set(Duplicate, SupersededBy)).map(((n, m, r)) => CLink({ node: n, rel: r, to: m }))

  pure def k_moves(st: State): Set[Cmd] =
    tuples(k_nodes(st), Set(ROOT, SEED_W1, PEOPLE), Set(Lit("a1"), Lit("w1"))).map(((n, p, nm)) =>
      CMove({ node: n, parent: p, name: nm }))

  pure def k_merges(st: State): Set[Cmd] =
    tuples(k_nodes(st), k_nodes(st)).map(((f, i)) => CMerge({ from: f, into: i }))

  pure def k_reopens(st: State): Set[Cmd] =
    st.nodes.keys().filter(n => st.nodes.get(n).life != Open).union(Set(ROOT)).map(n => CReopen(n))

  pure def k_closes(st: State): Set[Cmd] =
    k_nodes(st).map(n => CClose({ node: n, outcome: Done, settles: Set(), token: NoToken }))

  /// 1.10 (§10.2): a take of a made node, so that a node a release left waiting is held again, and
  /// its checkpoint ends the wait.
  pure def k_takes(st: State): Set[Cmd] = made_nodes(st).map(n => CTake({ node: n, lease: -1 }))

  /// 1.10 (§10.2): a checkpoint, which ends its node's wait: T5's graph loses the edge.
  pure def k_checkpoints(st: State): Set[Cmd] =
    slice_tokens(st).map(t => CCheckpoint({ token: t, intent: false, intent_done: false }))

  /// A release that waits on a node (T5: a cycle is refused).
  pure def k_releases(st: State): Set[Cmd] =
    tuples(slice_tokens(st), made_nodes(st)).map(((t, m)) => CRelease({ token: t, reason: Blocked, wait: WaNode(m), settles: Set() }))

  /// `depends_on` set to one node or to none (T5).
  pure def k_updates(st: State): Set[Cmd] =
    tuples(made_nodes(st), made_nodes(st).map(m => IdsSet(Set(m))).union(Set(IdsSet(Set())))).map(((n, d)) =>
      update_cmd(st, n, { ...NO_UPDATE, depends: d }))

  /// A request of a message of `c1` or of the conversation `c2`, with no `where` or `where: c2`.
  pure def k_requests(st: State): Set[Cmd] =
    tuples(conv_nodes(st), Set(Msg(("c1", 1)), Conv("c2")), Set(NoKey, SomeKey(Conv("c2")))).map(((at, o, w)) =>
      CRequest({ at: at, origin: o, where_key: w }))

  /// An ack (a commitment) and a reply (which fulfils it), with each `where`.
  pure def k_saids(st: State): Set[Cmd] =
    tuples(conv_nodes(st), Set(SAck, SReply), REQUEST_WHERES).map(((at, k, w)) => CSaid({ at: at, kind: k, where_key: w }))

  pure def k_asks(st: State): Set[Cmd] =
    conv_nodes(st).map(at => CAskPerson({ at: at, blocking: false, again: false }))

  pure def k_heards(st: State): Set[Cmd] =
    tuples(conv_nodes(st), Set(-1).union(st.pquestions.keys())).map(((at, q)) => CHeard({ at: at, answers: q }))

  pure def k_hides(st: State): Set[Cmd] = st.entries.keys().union(Set(st.next_eid)).map(e => CHide(e))

  /// The invariants of the slice (D.10.7), on the action from `pre` through `cmd_post` (after its
  /// command, before its settle) to `post`.
  pure def keys_invariants(pre: State, cmd_post: State, post: State, clk: int, la: Action, max_firings: int): bool = {
    val cs = steps_of(pre, cmd_post, post, clk, la, max_firings)
    and {
      common_invariants(cs, post, clk, la),
      inv_K1(post), inv_K3(post), inv_K4(post), inv_K5(post), inv_S1(post),
      inv_T5(post), inv_O2(post), inv_L1(post), inv_M1(post, clk),
      each_step(cs, c => and { step_K7(c), step_N5(c), step_C1(c), step_O2(c), step_L1(c) }),
    }
  }
}

module keys_two {
  import types.* from "../types"
  import params.* from "../params"
  import common.* from "./common"
  import keys_slice.*
  import forum(
    CLASSES = Set("a", "b"),
    USE_FNV = false,
    QUORUM = 2,
    PRINCIPALS = Set("user", "p1", "p2"),
    HAS_USER = true,
    MAX_FIRINGS = 30,
    SEED_SERIES = false,
    LONG_TEXTS = Set(false),
    RARE_ODDS = 1
  ).* from "../forum"

  action keys_calls = any {
    slice_call("act_create", k_creates(s), PLAIN),
    slice_call("act_key_add", k_key_adds(s), PLAIN),
    slice_call("act_key_remove", k_key_removes(s), PLAIN),
    slice_call("act_link", k_links(s), PLAIN),
    slice_call("act_move", k_moves(s), PLAIN),
    slice_call("act_merge", k_merges(s), PLAIN),
    slice_call("act_reopen", k_reopens(s), PLAIN),
    slice_call("act_close", k_closes(s), PLAIN),
    slice_call("act_release", k_releases(s), PLAIN),
    slice_call("act_take", k_takes(s), PLAIN),
    slice_call("act_checkpoint", k_checkpoints(s), PLAIN),
    slice_call("act_update", k_updates(s), PLAIN),
    slice_call("act_request", k_requests(s), PLAIN),
    slice_call("act_said", k_saids(s), PLAIN),
    slice_call("act_ask_person", k_asks(s), PLAIN),
    slice_call("act_heard", k_heards(s), PLAIN),
    slice_call("act_hide", k_hides(s), PLAIN),
  }

  action step_keys = any { keys_calls, act_tick }

  val inv_keys = keys_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS)
}

module keys_one {
  import types.* from "../types"
  import params.* from "../params"
  import common.* from "./common"
  import keys_slice.*
  import forum(
    CLASSES = Set("w"),
    USE_FNV = false,
    QUORUM = 1,
    PRINCIPALS = Set("user", "p1", "p2"),
    HAS_USER = true,
    MAX_FIRINGS = 30,
    SEED_SERIES = false,
    LONG_TEXTS = Set(false),
    RARE_ODDS = 1
  ).* from "../forum"

  action keys_calls = any {
    slice_call("act_create", k_creates(s), PLAIN),
    slice_call("act_key_add", k_key_adds(s), PLAIN),
    slice_call("act_key_remove", k_key_removes(s), PLAIN),
    slice_call("act_link", k_links(s), PLAIN),
    slice_call("act_move", k_moves(s), PLAIN),
    slice_call("act_merge", k_merges(s), PLAIN),
    slice_call("act_reopen", k_reopens(s), PLAIN),
    slice_call("act_close", k_closes(s), PLAIN),
    slice_call("act_release", k_releases(s), PLAIN),
    slice_call("act_take", k_takes(s), PLAIN),
    slice_call("act_checkpoint", k_checkpoints(s), PLAIN),
    slice_call("act_update", k_updates(s), PLAIN),
    slice_call("act_request", k_requests(s), PLAIN),
    slice_call("act_said", k_saids(s), PLAIN),
    slice_call("act_ask_person", k_asks(s), PLAIN),
    slice_call("act_heard", k_heards(s), PLAIN),
    slice_call("act_hide", k_hides(s), PLAIN),
  }

  action step_keys = any { keys_calls, act_tick }

  val inv_keys = keys_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS)
}
