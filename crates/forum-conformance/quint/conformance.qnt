// -*- mode: Bluespec; -*-
/**
 * Trace generators for the conformance replay (WP-F1, D.10.8). Not part of the model: the modules
 * below import the model's files unchanged and only choose which of a slice's actions a step picks
 * and how often.
 *
 * `holds_busy_two` and `holds_busy_one` are the `holds` slice of D.10.7 (`model/slices/holds.qnt`)
 * with its twelve command actions as twelve choices of the step beside the two time actions, where
 * the slice's `step_holds` picks among three (all its commands, `act_tick`, `act_tick_next`): its
 * random runs spend two steps in three on time, so that every session has retired before most
 * runs call much. Every step is a step of `step_holds`, so every trace is a trace of the slice.
 *
 * `keys_core_two` and `keys_core_one` are the `keys` slice in its two class configurations,
 * restricted to the commands `forum-core` implements in this checkout: `node create` that makes a
 * node (a create that would join an open carrier is WP-D4's), `node close`, `hold release` with a
 * `{node}` wait, `node update {depends_on}`, `node hide`, and the clock. The other actions of the
 * slice (key add and remove, link, move, merge, reopen, and the `talk` actions of WP-D5b) stay in
 * `keys_two` and `keys_one`, whose traces the replay keeps as ignored tests until the core has them.
 *
 * `said_held_two` (1.9) is the `keys` slice's requests and `said` entries beside takes of their
 * conversation nodes, so that a `said` is recorded while another session holds its conversation:
 * 1.9 withheld it; since 1.10 it answers and fulfils as any `said` does (§9.2).
 *
 * `review_two` (1.10) is the `people` slice's objections, resolves, closes, takes, checkpoints, and
 * notes, with releases that wait on a node: a second, opposite verdict opens a new objection and
 * withdraws the correction an upheld one made (§11.3), a `say` without `to` at a node nobody holds
 * waits for its next holder (§13), and a checkpoint ends its node's wait (§10.2). 1.11: its resolves
 * include deferrals, which back the caller's class off a closed node's review until a verdict
 * clears them (§10.3, §11.3), and its notes without `to` at a closed node reach the session that
 * closed it (§13).
 *
 * `finish_two` (1.12) is the `people` slice's requests, answers, releases that settle them, questions
 * to people and their retracts, notes to `holder:` of nodes that close, and reopens (§9.3, §10.3,
 * §13, §16).
 *
 * `post_settle_two` is `holds_busy_two` whose releases may also carry a `{time}` wait at the clock,
 * as the instances' releases do (`release_wait_groups` in `model/forum.qnt`): the wait is resolved
 * at once by its marker (A-68). `scripts/traces.sh` runs it until a release with such a wait ends
 * the last hold of another session whose `silence_at` has passed (1.4, R27, D.4.8): the release
 * leaves that session's Silence key at its `now`, which D.8.1's post-settle fires in the same call,
 * in the model (`w_release_wait_now`) as in the core.
 *
 * `scripts/traces.sh` runs them; from `model/`, for example:
 *
 *   npx quint run ../crates/forum-conformance/quint/conformance.qnt --main keys_core_two \
 *     --step step_keys_core --mbt --max-steps 30 --out-itf 'traces/k{seq}.itf.json'
 */
module holds_busy_two {
  import types.* from "../../../model/types"
  import params.* from "../../../model/params"
  import common.* from "../../../model/slices/common"
  import holds.* from "../../../model/slices/holds"
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
  ).* from "../../../model/forum"

  action step_holds_busy = any {
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
    act_tick,
    act_tick_next,
  }

  val inv_holds_busy = holds_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, T2_K)
}

/// The same with one worker class of two sessions, `QUORUM = 1`.
module holds_busy_one {
  import types.* from "../../../model/types"
  import params.* from "../../../model/params"
  import common.* from "../../../model/slices/common"
  import holds.* from "../../../model/slices/holds"
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
  ).* from "../../../model/forum"

  action step_holds_busy = any {
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
    act_tick,
    act_tick_next,
  }

  val inv_holds_busy = holds_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, T2_K)
}

/// `holds_busy_two` whose releases may also wait until the clock (`{time}` at `now`).
module post_settle_two {
  import types.* from "../../../model/types"
  import params.* from "../../../model/params"
  import common.* from "../../../model/slices/common"
  import holds.* from "../../../model/slices/holds"
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
  ).* from "../../../model/forum"

  /// The slice's releases, and a handoff of each token with a `{time}` wait at `clock`.
  def releases(st: State): Set[Cmd] =
    h_releases(st).union(slice_tokens(st).map(t => CRelease({ token: t, reason: Handoff, wait: WaTime(clock), settles: Set() })))

  action step_post_settle = any {
    slice_hello(CAPS_CHOICES, Set("")),
    slice_call("act_create", h_creates(s), TEXT_OPS),
    slice_call("act_take", h_takes(s), TAKE_OPS),
    slice_call("act_next", Set(CNext), PLAIN),
    slice_call("act_checkpoint", h_checkpoints(s), WRITE_OPS),
    slice_call("act_check_intent", h_intent_checks(s), WRITE_OPS),
    slice_call("act_renew", h_renews(s), WRITE_OPS),
    slice_call("act_release", releases(s), WRITE_OPS),
    slice_call("act_close", h_closes(s), WRITE_OPS),
    slice_call("act_update", h_updates(s), TEXT_OPS),
    slice_call("act_pause", h_pauses(s), TEXT_OPS),
    slice_call("act_unpause", h_unpauses(s), PLAIN),
    slice_call("act_view", h_views(s), PLAIN),
    act_tick,
    act_tick_next,
  }

  /// The last action was a release with a `{time}` wait at its `now`, and its post-settle fired.
  val w_release_wait_now = w_post_settle and match last_action.cmd {
    | CRelease(a) => a.wait == WaTime(clock)
    | _ => false
  }

  val inv_post_settle = holds_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, T2_K)
}

module keys_core_two {
  import types.* from "../../../model/types"
  import params.* from "../../../model/params"
  import commands.* from "../../../model/commands"
  import common.* from "../../../model/slices/common"
  import keys_slice.* from "../../../model/slices/keys"
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
  ).* from "../../../model/forum"

  /// The creates of the slice that make a node: none of their keys is carried by an open node.
  pure def making(st: State, cmds: Set[Cmd]): Set[Cmd] =
    cmds.filter(c => match c { | CCreate(a) => carriers(st, a.keys) == Set() | _ => false })

  /// The commands as five choices beside `act_tick`, as `holds_busy_two` does.
  action step_keys_core = any {
    slice_call("act_create", making(s, k_creates(s)), PLAIN),
    slice_call("act_close", k_closes(s), PLAIN),
    slice_call("act_release", k_releases(s), PLAIN),
    slice_call("act_update", k_updates(s), PLAIN),
    slice_call("act_hide", k_hides(s), PLAIN),
    act_tick,
  }

  val inv_keys_core = keys_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS)
}

/// The same with one worker class of two sessions, `QUORUM = 1`.
module keys_core_one {
  import types.* from "../../../model/types"
  import params.* from "../../../model/params"
  import commands.* from "../../../model/commands"
  import common.* from "../../../model/slices/common"
  import keys_slice.* from "../../../model/slices/keys"
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
  ).* from "../../../model/forum"

  pure def making(st: State, cmds: Set[Cmd]): Set[Cmd] =
    cmds.filter(c => match c { | CCreate(a) => carriers(st, a.keys) == Set() | _ => false })

  /// The commands as five choices beside `act_tick`, as `holds_busy_two` does.
  action step_keys_core = any {
    slice_call("act_create", making(s, k_creates(s)), PLAIN),
    slice_call("act_close", k_closes(s), PLAIN),
    slice_call("act_release", k_releases(s), PLAIN),
    slice_call("act_update", k_updates(s), PLAIN),
    slice_call("act_hide", k_hides(s), PLAIN),
    act_tick,
  }

  val inv_keys_core = keys_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS)
}

/// 1.9 (§9.2): the `keys` slice's requests and `said` entries beside takes and releases of the
/// conversation nodes, so that a session records a `said` while another holds its conversation
/// node, which the slice alone never reaches: 1.9 withheld such a `said`; since 1.10 it answers,
/// fulfils, and opens as any `said`, in the model as in the core.
module said_held_two {
  import types.* from "../../../model/types"
  import params.* from "../../../model/params"
  import commands.* from "../../../model/commands"
  import common.* from "../../../model/slices/common"
  import keys_slice.* from "../../../model/slices/keys"
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
  ).* from "../../../model/forum"

  /// A take of a conversation node: the seed's `/w1` and `/w2`, or one the run made.
  pure def takes(st: State): Set[Cmd] =
    conv_nodes(st).filter(n => n >= SEED_W1).map(n => CTake({ node: n, lease: -1 }))

  action step_said_held = any {
    slice_call("act_take", takes(s), PLAIN),
    slice_call("act_release", k_releases(s), PLAIN),
    slice_call("act_request", k_requests(s), PLAIN),
    slice_call("act_said", k_saids(s), PLAIN),
    act_tick,
  }

  /// A run reached a `said` recorded while another session held its conversation node (1.10: it
  /// counts), which answered a request.
  val w_held_answer = s.requests.keys().exists(r => s.requests.get(r).answered_by >= 0)

  val inv_said_held = keys_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS)
}

/// 1.10 (§10.2, §11.3, §13): the `people` slice's review and notes: objections on closed and open
/// nodes, resolves with each verdict, of open and resolved objections (a second verdict opens a new
/// objection, and withdraws the correction an upheld one made), closes, takes, releases that wait on
/// a node, checkpoints (which end such a wait), and notes without `to` or to a session, which `next`
/// and the clock interleave.
module review_two {
  import types.* from "../../../model/types"
  import params.* from "../../../model/params"
  import commands.* from "../../../model/commands"
  import common.* from "../../../model/slices/common"
  import people.* from "../../../model/slices/people"
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
  ).* from "../../../model/forum"

  /// The notes the core's driver replays: without `to`, and to a session.
  pure def says(st: State): Set[Cmd] =
    tuples(made_nodes(st), Set(SayNone, SaySession(1), SaySession(2))).map(((at, a)) => CSay({ at: at, to: a }))

  /// A release that waits on a node, so that a later take's checkpoint ends the wait (§10.2).
  pure def releases(st: State): Set[Cmd] =
    tuples(slice_tokens(st), made_nodes(st)).map(((t, m)) => CRelease({ token: t, reason: Blocked, wait: WaNode(m), settles: Set() }))

  action step_review = any {
    slice_call("act_take", p_takes(s), PLAIN),
    slice_call("act_checkpoint", p_checkpoints(s), PLAIN),
    slice_call("act_release", releases(s), PLAIN),
    slice_call("act_close", p_closes(s), PLAIN),
    slice_call("act_object", p_objects(s), PLAIN),
    slice_call("act_resolve", p_resolves(s), PLAIN),
    slice_call("act_say", says(s), PLAIN),
    slice_call("act_next", Set(CNext), PLAIN),
    act_tick,
  }

  /// 1.11: a run deferred a closed node's review: a refusal record on a closed node.
  val w_deferred = s.refusals.keys().exists(k => s.nodes.get(k._1).life != Open)

  /// A run withdrew a correction: a closed node that was once marked upheld is not marked, and an
  /// objection on it was upheld.
  val w_withdrawn = s.objections.keys().exists(o => {
    val ob = s.objections.get(o)
    match ob.state {
      | ObUpheld(v) => match s.nodes.get(ob.node).life { | Closed(c) => c.upheld != v | Open => false }
      | _ => false
    }
  })

  val inv_review = people_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, T2_K)
}

/// 1.12 (§9.3, §10.3, §13, §16): the `people` slice's finishing, notes, and questions: requests and
/// their answers, releases that settle them (the decision recorded after the release), questions to
/// people and their retracts (an open one withdrawn, its wait resolved and its recheck gone), notes
/// to `holder:` of nodes that close (reaching the live closer), and reopens (the `offer` tag), which
/// takes, closes, `next`, and the clock interleave.
module finish_two {
  import types.* from "../../../model/types"
  import params.* from "../../../model/params"
  import commands.* from "../../../model/commands"
  import common.* from "../../../model/slices/common"
  import people.* from "../../../model/slices/people"
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
  ).* from "../../../model/forum"

  /// Notes without `to`, to a session, and to `holder:` of each made node, open or closed.
  pure def says(st: State): Set[Cmd] =
    tuples(made_nodes(st), Set(SayNone, SaySession(1), SaySession(2))).map(((at, a)) => CSay({ at: at, to: a }))
      .union(made_nodes(st).map(n => CSay({ at: n, to: SayHolder(n) })))

  /// The retract of each question to a person and of each decision.
  pure def retracts(st: State): Set[Cmd] =
    st.pquestions.keys().union(st.decisions.keys()).map(e => CRetract(TEntry(e)))

  /// The reopen of each closed made node.
  pure def reopens(st: State): Set[Cmd] =
    made_nodes(st).filter(n => st.nodes.get(n).life != Open).map(n => CReopen(n))

  /// A release that settles an answered request, with no wait or waiting for a time: drawn beside
  /// the slice's releases, so that runs settle by a release's decision more often than by chance.
  pure def settling(st: State): Set[Cmd] =
    tuples(slice_tokens(st), st.requests.keys().filter(r => st.requests.get(r).answered_by >= 0)).map(((t, r)) =>
      CRelease({ token: t, reason: Blocked, wait: WaNone, settles: Set(r) }))

  /// A note to `holder:` of a closed made node, drawn beside the others for the same reason.
  pure def closed_holder_says(st: State): Set[Cmd] =
    made_nodes(st).filter(n => st.nodes.get(n).life != Open).map(n => CSay({ at: n, to: SayHolder(n) }))

  action step_finish = any {
    slice_call("act_take", p_takes(s), PLAIN),
    slice_call("act_request", p_requests(s), PLAIN),
    slice_call("act_said", p_saids(s), PLAIN),
    slice_call("act_ask_person", p_asks(s), PLAIN),
    slice_call("act_release", p_releases(s), PLAIN),
    slice_call("act_release", settling(s), PLAIN),
    slice_call("act_retract", retracts(s), PLAIN),
    slice_call("act_close", p_closes(s), PLAIN),
    slice_call("act_reopen", reopens(s), PLAIN),
    slice_call("act_say", says(s), PLAIN),
    slice_call("act_say", closed_holder_says(s), PLAIN),
    slice_call("act_next", Set(CNext), PLAIN),
    act_tick,
  }

  /// A run settled a request, by a release's decision or a close's.
  val w_settled = s.requests.keys().exists(r => match s.requests.get(r).settled {
    | SettledBy(x) => s.decisions.keys().contains(x.decision)
    | Unsettled => false
  })

  /// A run withdrew a question.
  val w_withdrawn_question = s.pquestions.keys().exists(q => match s.pquestions.get(q).state {
    | PqWithdrawn(_) => true
    | _ => false
  })

  val inv_finish = people_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, T2_K)
}

/// The `holds` slice's `holds_two` as 1.7 had it, without 1.8's intent check (`act_check_intent`),
/// for the Silence key's witness (`tests/findings.rs`), which `scripts/traces.sh` writes from its
/// run 50 of 200 (seed `0x5eed01`). 1.8's model draws other runs from the same seed, and a session
/// whose hold another session extends past its `silence_at` is rare: none of 200 runs of 1.8's
/// `holds_two` for each of seven seeds has one.
module holds_silence_two {
  import types.* from "../../../model/types"
  import params.* from "../../../model/params"
  import common.* from "../../../model/slices/common"
  import holds.* from "../../../model/slices/holds"
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
  ).* from "../../../model/forum"

  action holds_calls = any {
    slice_hello(CAPS_CHOICES, Set("")),
    slice_call("act_create", h_creates(s), TEXT_OPS),
    slice_call("act_take", h_takes(s), TAKE_OPS),
    slice_call("act_next", Set(CNext), PLAIN),
    slice_call("act_checkpoint", h_checkpoints(s), WRITE_OPS),
    slice_call("act_renew", h_renews(s), WRITE_OPS),
    slice_call("act_release", h_releases(s), WRITE_OPS),
    slice_call("act_close", h_closes(s), WRITE_OPS),
    slice_call("act_update", h_updates(s), TEXT_OPS),
    slice_call("act_pause", h_pauses(s), TEXT_OPS),
    slice_call("act_unpause", h_unpauses(s), PLAIN),
    slice_call("act_view", h_views(s), PLAIN),
  }

  action step_holds_silence = any { holds_calls, act_tick, act_tick_next }

  val inv_holds_silence = holds_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, T2_K)
}
