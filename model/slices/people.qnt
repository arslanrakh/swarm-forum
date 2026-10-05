// -*- mode: Bluespec; -*-
/**
 * The `people` slice of D.10.7 (WP-A3): request (with message origins and `where`, A-61), said (of
 * each kind, with `where`), confirm, ask person (with `again`), heard, release with a `{question}`
 * wait, decide (with `supersedes`), retract, approve, object, resolve, merge, key remove, ask agent,
 * answer, say, wait, take, checkpoint (1.10: it ends its node's wait), next, close, tick, and
 * tick_next. 1.10: `say` without `to`, which reaches its node's holders or its next holder, and
 * `resolve` of a resolved objection, which records a second verdict. 1.11: `resolve` with `defer`,
 * which backs the caller's class off a closed node's review (a refusal record on a closed node,
 * which E1 and M1 read), and `say` without `to` at a closed node, which reaches its live closer.
 * 1.12: `release` with `settles`, which records a decision after itself; `retract` of a question,
 * which withdraws it while it is open (E1 (g), W1, M1); and `say` to `holder:` of a node that may
 * be closed, which reaches its live closer.
 *
 * 1.3 (§9.2, §9.4): `/w2` merged into `/w1`, or `/w2` closed and a request of `c2` made at `/w1`
 * (case 3 of §9.1), makes a node that carries both conversation keys; a request and an ack in each,
 * then `said` of each kind with each `where`, show that a reply answers and fulfils only its own
 * conversation's (`inv_Settled`, `step_Settled`, `step_C1`).
 *
 * Invariants: Q1, C1, Settled, F, W1, E1, D1, M1, N5, T2, and the six of every slice
 * (`common.qnt`).
 *
 *   scripts/verify-file.sh apalache slices/people.qnt --main people_two --step step_people --invariant inv_people_apalache
 *   scripts/verify-file.sh tlc slices/people.qnt --main people_two --step step_people --invariant inv_people
 *
 * and the same with `people_one`.
 */
module people {
  import types.* from "../types"
  import params.* from "../params"
  import basics.* from "../basics"
  import flags.* from "../flags"
  import deadlines.* from "../deadlines"
  import commands.* from "../commands"
  import invariants.* from "../invariants"
  import common.* from "./common"

  /// Leases, silences, retirements, rechecks (with expiry and escalation), follow-ups, the user
  /// questions of escalations and their re-addressing and lapse, with room.
  pure val PEOPLE_FIRINGS = 30

  pure def p_nodes(st: State): Set[int] = slice_nodes(st, Set(ROOT))

  pure def p_requests(st: State): Set[Cmd] =
    tuples(conv_nodes(st), REQUEST_ORIGINS, REQUEST_WHERES).map(((at, o, w)) => CRequest({ at: at, origin: o, where_key: w }))

  pure def p_saids(st: State): Set[Cmd] =
    tuples(conv_nodes(st), Set(SAck, SReply, SResult, SCorrection, SQuestion), REQUEST_WHERES).map(((at, k, w)) =>
      CSaid({ at: at, kind: k, where_key: w }))

  pure def p_confirms(st: State): Set[Cmd] = st.saids.keys().union(Set(st.next_eid)).map(x => CConfirm(x))

  pure def p_asks(st: State): Set[Cmd] =
    tuples(conv_nodes(st), Set(false, true), Set(false, true)).map(((at, b, ag)) =>
      CAskPerson({ at: at, blocking: b, again: ag }))

  pure def p_heards(st: State): Set[Cmd] =
    tuples(conv_nodes(st), Set(-1).union(st.pquestions.keys())).map(((at, q)) => CHeard({ at: at, answers: q }))

  /// A release that waits on a people question or a question for the user (and one that names no
  /// question).
  /// 1.12 (§10.3): and one that settles a request, which records its decision after itself.
  pure def p_releases(st: State): Set[Cmd] =
    tuples(slice_tokens(st), st.pquestions.keys().union(st.uquestions.keys()).union(Set(st.next_eid))).map(((t, q)) =>
      CRelease({ token: t, reason: Blocked, wait: WaQuestion(q), settles: Set() }))
    .union(tuples(slice_tokens(st), st.requests.keys().union(Set(st.next_eid))).map(((t, r)) =>
      CRelease({ token: t, reason: Blocked, wait: WaNone, settles: Set(r) })))

  pure def p_decides(st: State): Set[Cmd] =
    tuples(p_nodes(st), Set(Set()).union(st.requests.keys().map(r => Set(r))), Set(-1).union(st.decisions.keys())).map(((at, rs, sup)) =>
      CDecide({ at: at, settles: rs, supersedes: sup, basis: [] }))

  /// A decision's retract, and (1.12, §9.3) a question's, which withdraws it while it is open.
  pure def p_retracts(st: State): Set[Cmd] =
    st.decisions.keys().union(st.pquestions.keys()).union(Set(st.next_eid)).map(d => CRetract(TEntry(d)))

  pure def p_approves(st: State): Set[Cmd] =
    made_nodes(st).map(n => CApprove({ node: n, ver: st.nodes.get(n).version }))

  pure def p_objects(st: State): Set[Cmd] = made_nodes(st).map(n => CObject(n))

  /// A verdict of each kind, and (1.11, §11.3) a deferral, which backs the caller's class off a
  /// closed node's review (E1, M1).
  pure def p_resolves(st: State): Set[Cmd] =
    st.objections.keys().union(Set(st.next_eid)).map(o =>
      Set((false, false), (true, false), (false, true)).map(v => CResolve({ objection: o, upheld: v._1, defer: v._2 }))).flatten()

  pure def p_merges(st: State): Set[Cmd] =
    tuples(made_nodes(st), made_nodes(st).union(Set(ROOT))).map(((f, i)) => CMerge({ from: f, into: i }))

  /// A key each made node carries, and ROOT's (`invalid(ReservedNode)`, K8).
  pure def p_key_removes(st: State): Set[Cmd] =
    made_nodes(st).map(n => st.nodes.get(n).keys.map(k => CKeyRemove({ node: n, key: k }))).flatten()
      .union(Set(CKeyRemove({ node: ROOT, key: Conv("c1") })))

  pure def p_ask_agents(st: State): Set[Cmd] =
    tuples(made_nodes(st), Set(AgSession(1), AgHolder(SEED_W1), AgHolders(ROOT))).map(((at, a)) =>
      CAskAgent({ at: at, to: a }))

  pure def p_answers(st: State): Set[Cmd] =
    st.entries.keys().filter(e => st.entries.get(e).ty == EQuestion).union(Set(st.next_eid)).map(q => CAnswer(q))

  /// 1.10 (§13): with no `to` too, which reaches the node's holders, or its next holder.
  pure def p_says(st: State): Set[Cmd] =
    tuples(made_nodes(st), Set(SayNone, SaySession(1), SaySession(2), SayHolders(ROOT), SayHolder(SEED_W1))).map(((at, a)) =>
      CSay({ at: at, to: a }))

  /// 1.10 (§10.2): a checkpoint, which ends its node's wait (W1, M1).
  pure def p_checkpoints(st: State): Set[Cmd] =
    slice_tokens(st).map(t => CCheckpoint({ token: t, intent: false, intent_done: false }))

  pure def p_takes(st: State): Set[Cmd] = p_nodes(st).map(n => CTake({ node: n, lease: -1 }))

  pure def p_closes(st: State): Set[Cmd] =
    p_nodes(st).map(n => CClose({ node: n, outcome: Done, settles: Set(), token: NoToken }))

  /// The invariants of the slice (D.10.7), on the action from `pre` through `cmd_post` (after its
  /// command, before its settle) to `post`.
  pure def people_invariants(pre: State, cmd_post: State, post: State, clk: int, la: Action, max_firings: int, t2_k: int): bool = {
    val cs = steps_of(pre, cmd_post, post, clk, la, max_firings)
    and {
      common_invariants(cs, post, clk, la),
      // 1.9 (§10.1, §10.4): H6 where closed nodes carry objections, which a take refuses and
      // `next` offers as a review with no hold.
      inv_H6(post),
      inv_Q1(post), inv_Settled(post), inv_F(post, clk), inv_W1(post, clk),
      inv_E1(post, clk), inv_D1(post), inv_M1(post, clk),
      inv_T2(post, clk, t2_k, max_firings),
      each_step(cs, c => and { step_C1(c), step_Settled(c), step_D1(c), step_N5(c) }),
    }
  }
}

module people_two {
  import types.* from "../types"
  import params.* from "../params"
  import common.* from "./common"
  import people.*
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

  action people_calls = any {
    slice_call("act_request", p_requests(s), PLAIN),
    slice_call("act_said", p_saids(s), PLAIN),
    slice_call("act_confirm", p_confirms(s), PLAIN),
    slice_call("act_ask_person", p_asks(s), PLAIN),
    slice_call("act_heard", p_heards(s), PLAIN),
    slice_call("act_release", p_releases(s), PLAIN),
    slice_call("act_decide", p_decides(s), PLAIN),
    slice_call("act_retract", p_retracts(s), PLAIN),
    slice_call("act_approve", p_approves(s), PLAIN),
    slice_call("act_object", p_objects(s), PLAIN),
    slice_call("act_resolve", p_resolves(s), PLAIN),
    slice_call("act_merge", p_merges(s), PLAIN),
    slice_call("act_key_remove", p_key_removes(s), PLAIN),
    slice_call("act_ask_agent", p_ask_agents(s), PLAIN),
    slice_call("act_answer", p_answers(s), PLAIN),
    slice_call("act_say", p_says(s), PLAIN),
    slice_call("act_wait", Set(CWait(-1)), PLAIN),
    slice_call("act_take", p_takes(s), PLAIN),
    slice_call("act_checkpoint", p_checkpoints(s), PLAIN),
    slice_call("act_next", Set(CNext), PLAIN),
    slice_call("act_close", p_closes(s), PLAIN),
  }

  action step_people = any { people_calls, act_tick, act_tick_next }

  val inv_people = people_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, T2_K)
  val inv_people_apalache = people_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, 2)
}

module people_one {
  import types.* from "../types"
  import params.* from "../params"
  import common.* from "./common"
  import people.*
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

  action people_calls = any {
    slice_call("act_request", p_requests(s), PLAIN),
    slice_call("act_said", p_saids(s), PLAIN),
    slice_call("act_confirm", p_confirms(s), PLAIN),
    slice_call("act_ask_person", p_asks(s), PLAIN),
    slice_call("act_heard", p_heards(s), PLAIN),
    slice_call("act_release", p_releases(s), PLAIN),
    slice_call("act_decide", p_decides(s), PLAIN),
    slice_call("act_retract", p_retracts(s), PLAIN),
    slice_call("act_approve", p_approves(s), PLAIN),
    slice_call("act_object", p_objects(s), PLAIN),
    slice_call("act_resolve", p_resolves(s), PLAIN),
    slice_call("act_merge", p_merges(s), PLAIN),
    slice_call("act_key_remove", p_key_removes(s), PLAIN),
    slice_call("act_ask_agent", p_ask_agents(s), PLAIN),
    slice_call("act_answer", p_answers(s), PLAIN),
    slice_call("act_say", p_says(s), PLAIN),
    slice_call("act_wait", Set(CWait(-1)), PLAIN),
    slice_call("act_take", p_takes(s), PLAIN),
    slice_call("act_checkpoint", p_checkpoints(s), PLAIN),
    slice_call("act_next", Set(CNext), PLAIN),
    slice_call("act_close", p_closes(s), PLAIN),
  }

  action step_people = any { people_calls, act_tick, act_tick_next }

  val inv_people = people_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, T2_K)
  val inv_people_apalache = people_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, 2)
}
