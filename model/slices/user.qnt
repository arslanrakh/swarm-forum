// -*- mode: Bluespec; -*-
/**
 * The `user` slice of D.10.7 (WP-A3): hello (with a principal), from_user and to_user (each with a
 * principal of PRINCIPALS, to_user of each kind, A-23; neither in `user_none`), route, ask user (to
 * `user` or a principal), ask person (blocking, so that escalations arise), next (open, so that
 * classes become active with their caps, CS-7), release `cannot`, release with a `{question}` wait on
 * a people or a user question, update (needs, CS-7; owners and the `gone` tag, D-3), create under
 * `@people` (person keys as aliases) and `@teams`, with capacity 0 or 1 and with or without a hold
 * (A-113), decide, retract, take, close, tick, and tick_next.
 *
 * Invariants: N1, E1 (with (e) and (f), the quorum, and needs-aware active classes), D1, Settled (a
 * report to `p1` never answers `p2`'s request unless they are aliases of one person, and only a
 * `result` answers, A-23), M1, U1 (with `step_U1` and `hello_exact`, over aliases and owners), T2,
 * and the six of every slice (`common.qnt`). Liveness: `deadline_fires`, `uq_resolves`, and
 * `wait_on_uq_resolves`, in `user_none` too, where nobody ever answers (CS-2, CS-6).
 *
 *   scripts/verify-file.sh apalache slices/user.qnt --main user_two --step step_user --invariant inv_user_apalache
 *   scripts/verify-file.sh tlc slices/user.qnt --main user_two --step step_user --invariant inv_user
 *   scripts/verify-file.sh tlc slices/user.qnt --main user_two --step step_user --temporal liveness_user
 *
 * and the same with `user_one` and `user_none`.
 */
module user {
  import types.* from "../types"
  import params.* from "../params"
  import basics.* from "../basics"
  import flags.* from "../flags"
  import deadlines.* from "../deadlines"
  import commands.* from "../commands"
  import invariants.* from "../invariants"
  import common.* from "./common"

  /// Leases, silences, retirements, rechecks and escalations, every re-addressing and lapse of
  /// each user question, refusal and class expiries, with room.
  pure val USER_FIRINGS = 40

  pure def u_nodes(st: State): Set[int] = slice_nodes(st, Set(ROOT, USER, PEOPLE, TEAMS))

  /// The persons and teams of the directory, and ROOT (D.10.4: owners "persons or teams, ROOT
  /// included").
  pure def u_owners(st: State): Set[int] =
    st.nodes.keys().filter(n => st.nodes.get(n).parent == PEOPLE or st.nodes.get(n).parent == TEAMS).union(Set(ROOT))

  pure def u_from_users(st: State, principals: Set[str]): Set[Cmd] =
    tuples(principals.union(Set("")), Set(-1).union(st.uquestions.keys()), Set(-1, SEED_W1)).map(((p, q, n)) =>
      CFromUser({ principal: p, answers: q, clarifies: -1, node: n }))

  pure def u_to_users(st: State, principals: Set[str]): Set[Cmd] =
    tuples(principals.union(Set("")), Set(TuAck, TuQuestion, TuResult),
           st.requests.keys().union(st.user_entries.keys()).union(st.digest.keys()).map(x => Set(x))).map(((p, k, re)) =>
      CToUser({ principal: p, kind: k, re: re }))

  /// A route to nothing, to each seed node, and to `@user`, a reserved node (K8).
  /// Each entry routed to each target, as a union rather than a product: Quint prints `keys()` as
  /// TLA+'s `DOMAIN`, and `DOMAIN f \X s` does not parse in TLC (a precedence conflict), so
  /// `tuples(st.user_entries.keys(), …)` kept TLC from the `user` slices until 1.9.
  pure def u_routes(st: State): Set[Cmd] =
    st.user_entries.keys().map(e => Set(-1, SEED_W1, SEED_W2, USER).map(n => CRoute({ entry: e, to: n }))).flatten()

  pure def u_ask_users(st: State, principals: Set[str]): Set[Cmd] =
    tuples(Set(SEED_W1, SEED_W2), Set(false, true), principals).map(((at, d, p)) =>
      CAskUser({ at: at, has_default: d, to: p, again: false, digest: 1 }))

  pure def u_ask_persons(st: State): Set[Cmd] =
    conv_nodes(st).map(at => CAskPerson({ at: at, blocking: true, again: false }))

  /// `release {cannot}` without a wait (a refusal, CS-7), and a release that waits on each people
  /// or user question; 1.12 (§10.3): and a blocked one that settles a request.
  pure def u_releases(st: State): Set[Cmd] =
    tuples(slice_tokens(st), Set(WaNone).union(st.pquestions.keys().union(st.uquestions.keys()).map(q => WaQuestion(q))))
      .map(((t, w)) => CRelease({ token: t, reason: if (w == WaNone) Cannot else Blocked, wait: w, settles: Set() }))
      .union(tuples(slice_tokens(st), st.requests.keys()).map(((t, r)) =>
        CRelease({ token: t, reason: Blocked, wait: WaNone, settles: Set(r) })))

  /// Needs added and removed (CS-7), owners set to one owner or none, the `gone` tag added and
  /// removed (D-3).
  pure def u_updates(st: State): Set[Cmd] =
    made_nodes(st).union(Set(ROOT)).map(n => Set(
      update_cmd(st, n, { ...NO_UPDATE, needs_add: Set("x") }),
      update_cmd(st, n, { ...NO_UPDATE, needs_remove: Set("x") }),
      update_cmd(st, n, { ...NO_UPDATE, tags_add: Set(GONE) }),
      update_cmd(st, n, { ...NO_UPDATE, tags_remove: Set(GONE) }),
      update_cmd(st, n, { ...NO_UPDATE, owners: IdsSet(Set()) }),
    ).union(u_owners(st).map(o => update_cmd(st, n, { ...NO_UPDATE, owners: IdsSet(Set(o)) })))).flatten()

  /// A person under `@people` with one alias or two (D-3), and a team under `@teams` owned by each
  /// person, capacity 0 or 1, taken or not (A-113).
  pure def u_creates(st: State, principals: Set[str]): Set[Cmd] = {
    val aliases = principals.exclude(Set(USER_KEY)).map(p => Set(Person(p)))
      .union(Set(principals.exclude(Set(USER_KEY)).map(p => Person(p)))).exclude(Set(Set()))
      .union(Set(Set(Person(USER_KEY))))
    val persons = tuples(aliases, Set(0, 1), Set(false, true)).map(((ks, cap, take)) =>
      create_cmd(PEOPLE, Lit("a1"), ks, take, cap, Set(), RepKeep))
    val teams = tuples(st.nodes.keys().filter(n => st.nodes.get(n).parent == PEOPLE), Set(0, 1), Set(false, true)).map(((m, cap, take)) =>
      create_cmd(TEAMS, Lit("a2"), Set(), take, cap, Set(m), RepKeep))
    persons.union(teams)
  }

  pure def u_decides(st: State): Set[Cmd] =
    tuples(made_nodes(st), Set(Set()).union(st.requests.keys().map(r => Set(r)))).map(((at, rs)) =>
      CDecide({ at: at, settles: rs, supersedes: -1, basis: [] }))

  pure def u_retracts(st: State): Set[Cmd] = st.decisions.keys().map(d => CRetract(TEntry(d)))

  pure def u_takes(st: State): Set[Cmd] = u_nodes(st).map(n => CTake({ node: n, lease: -1 }))

  pure def u_closes(st: State): Set[Cmd] =
    u_nodes(st).map(n => CClose({ node: n, outcome: Done, settles: Set(), token: NoToken }))

  /// The invariants of the slice (D.10.7), on the action from `pre` through `cmd_post` (after its
  /// command, before its settle) to `post`.
  pure def user_invariants(pre: State, cmd_post: State, post: State, clk: int, la: Action, max_firings: int, t2_k: int): bool = {
    val cs = steps_of(pre, cmd_post, post, clk, la, max_firings)
    and {
      common_invariants(cs, post, clk, la),
      inv_N1(post, clk), inv_E1(post, clk), inv_D1(post), inv_Settled(post),
      inv_M1(post, clk), inv_U1(post), hello_exact(post),
      inv_T2(post, clk, t2_k, max_firings),
      each_step(cs, c => and { step_D1(c), step_Settled(c), step_U1(c) }),
    }
  }
}

/// Two classes, `QUORUM = 2`, the principals `user`, `p1`, and `p2`.
module user_two {
  import types.* from "../types"
  import params.* from "../params"
  import common.* from "./common"
  import user.*
  import forum(
    CLASSES = Set("a", "b"),
    USE_FNV = false,
    QUORUM = 2,
    PRINCIPALS = Set("user", "p1", "p2"),
    HAS_USER = true,
    MAX_FIRINGS = 40,
    SEED_SERIES = false,
    LONG_TEXTS = Set(false),
    RARE_ODDS = 1
  ).* from "../forum"

  action user_calls = any {
    slice_hello(CAPS_CHOICES, PRINCIPALS.union(Set(""))),
    slice_call("act_from_user", u_from_users(s, PRINCIPALS), PLAIN),
    slice_call("act_to_user", u_to_users(s, PRINCIPALS), PLAIN),
    slice_call("act_route", u_routes(s), PLAIN),
    slice_call("act_ask_user", u_ask_users(s, PRINCIPALS), PLAIN),
    slice_call("act_ask_person", u_ask_persons(s), PLAIN),
    slice_call("act_next", Set(CNext), PLAIN),
    slice_call("act_release", u_releases(s), PLAIN),
    slice_call("act_update", u_updates(s), PLAIN),
    slice_call("act_create", u_creates(s, PRINCIPALS), PLAIN),
    slice_call("act_decide", u_decides(s), PLAIN),
    slice_call("act_retract", u_retracts(s), PLAIN),
    slice_call("act_take", u_takes(s), PLAIN),
    slice_call("act_close", u_closes(s), PLAIN),
  }

  action step_user = any { user_calls, act_tick, act_tick_next }

  val inv_user = user_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, T2_K)
  val inv_user_apalache = user_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, 2)

  temporal liveness_user = and {
    weakFair(act_tick, (s, clock, prev, mid, last_action)),
    weakFair(act_tick_next, (s, clock, prev, mid, last_action)),
    weakFair(user_calls, (s, clock, prev, mid, last_action)),
  } implies and { deadline_fires, uq_resolves, wait_on_uq_resolves }
}

/// One worker class, `QUORUM = 1`.
module user_one {
  import types.* from "../types"
  import params.* from "../params"
  import common.* from "./common"
  import user.*
  import forum(
    CLASSES = Set("w"),
    USE_FNV = false,
    QUORUM = 1,
    PRINCIPALS = Set("user", "p1", "p2"),
    HAS_USER = true,
    MAX_FIRINGS = 40,
    SEED_SERIES = false,
    LONG_TEXTS = Set(false),
    RARE_ODDS = 1
  ).* from "../forum"

  action user_calls = any {
    slice_hello(CAPS_CHOICES, PRINCIPALS.union(Set(""))),
    slice_call("act_from_user", u_from_users(s, PRINCIPALS), PLAIN),
    slice_call("act_to_user", u_to_users(s, PRINCIPALS), PLAIN),
    slice_call("act_route", u_routes(s), PLAIN),
    slice_call("act_ask_user", u_ask_users(s, PRINCIPALS), PLAIN),
    slice_call("act_ask_person", u_ask_persons(s), PLAIN),
    slice_call("act_next", Set(CNext), PLAIN),
    slice_call("act_release", u_releases(s), PLAIN),
    slice_call("act_update", u_updates(s), PLAIN),
    slice_call("act_create", u_creates(s, PRINCIPALS), PLAIN),
    slice_call("act_decide", u_decides(s), PLAIN),
    slice_call("act_retract", u_retracts(s), PLAIN),
    slice_call("act_take", u_takes(s), PLAIN),
    slice_call("act_close", u_closes(s), PLAIN),
  }

  action step_user = any { user_calls, act_tick, act_tick_next }

  val inv_user = user_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, T2_K)
  val inv_user_apalache = user_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, 2)

  temporal liveness_user = and {
    weakFair(act_tick, (s, clock, prev, mid, last_action)),
    weakFair(act_tick_next, (s, clock, prev, mid, last_action)),
    weakFair(user_calls, (s, clock, prev, mid, last_action)),
  } implies and { deadline_fires, uq_resolves, wait_on_uq_resolves }
}

/// The forum with no human (CS-2, CS-6): one worker class, no `from_user` and no `to_user`, and
/// `user` as the only principal, so that a question nobody answers is seen to end.
module user_none {
  import types.* from "../types"
  import params.* from "../params"
  import common.* from "./common"
  import user.*
  import forum(
    CLASSES = Set("w"),
    USE_FNV = false,
    QUORUM = 1,
    PRINCIPALS = Set("user"),
    HAS_USER = false,
    MAX_FIRINGS = 40,
    SEED_SERIES = false,
    LONG_TEXTS = Set(false),
    RARE_ODDS = 1
  ).* from "../forum"

  action user_calls = any {
    slice_hello(CAPS_CHOICES, PRINCIPALS.union(Set(""))),
    slice_call("act_route", u_routes(s), PLAIN),
    slice_call("act_ask_user", u_ask_users(s, PRINCIPALS), PLAIN),
    slice_call("act_ask_person", u_ask_persons(s), PLAIN),
    slice_call("act_next", Set(CNext), PLAIN),
    slice_call("act_release", u_releases(s), PLAIN),
    slice_call("act_update", u_updates(s), PLAIN),
    slice_call("act_create", u_creates(s, PRINCIPALS), PLAIN),
    slice_call("act_decide", u_decides(s), PLAIN),
    slice_call("act_retract", u_retracts(s), PLAIN),
    slice_call("act_take", u_takes(s), PLAIN),
    slice_call("act_close", u_closes(s), PLAIN),
  }

  action step_user = any { user_calls, act_tick, act_tick_next }

  val inv_user = user_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, T2_K)
  val inv_user_apalache = user_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, 2)

  temporal liveness_user = and {
    weakFair(act_tick, (s, clock, prev, mid, last_action)),
    weakFair(act_tick_next, (s, clock, prev, mid, last_action)),
    weakFair(user_calls, (s, clock, prev, mid, last_action)),
  } implies and { deadline_fires, uq_resolves, wait_on_uq_resolves }
}
