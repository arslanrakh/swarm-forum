// -*- mode: Bluespec; -*-
/**
 * The main module of the model (D.10.1): its constants, the state variables of D.10.2, `init`
 * (genesis and the seed of D.10.7), one named action per command of D.10.4 and the two time
 * actions, `step`, and `inv_all`.
 *
 * Instances and slices instantiate it (D.10.1), for example
 *
 *   module one_class {
 *     import forum(CLASSES = Set("w"), USE_FNV = false, QUORUM = 1, PRINCIPALS = Set("user", "p1", "p2"),
 *                  HAS_USER = true, MAX_FIRINGS = 40, SEED_SERIES = false, LONG_TEXTS = Set(false, true),
 *                  RARE_ODDS = 8).* from "../forum"
 *   }
 *
 * Actions are thin (D.10.4): each picks its parameters with `nondet` from small domains, checks
 * that the state is settled at `now = max(clock, s.last_now)`, calls `run_cmd`, then settles at
 * `now` as D.8.1's post-settle does, and sets `prev`, `mid`, and `last_action`. Every action thus
 * ends with a settle: a command's post-settle at its `now` fires what the command made due at
 * `now` (the Silence key of a session whose last hold it ended, R27, D.4.8), and a time action's
 * settle what time made due. A command action is disabled when its command would take a bounded
 * quantity past its bound of D.10.7 (`within_bounds`); the time actions are bounded by `MAX_TIME`
 * alone, so that time never stops inside the bounds (README).
 */
module forum {
  import types.* from "./types"
  import params.* from "./params"
  import basics.* from "./basics"
  import flags.* from "./flags"
  import deadlines.* from "./deadlines"
  import commands.* from "./commands"
  import invariants.* from "./invariants"

  // ------------------------------------------------------------------ constants (D.10.1)

  /// The class names of the configuration, each in CLASS_NAMES (`params.qnt`).
  const CLASSES: Set[str]
  /// `true`: `next` breaks ties by FNV-1a-64 as Rust does (simulation and traces); `false`: by id
  /// (Apalache, TLC), which the replay matches with `TieBreak::Id` (D.10.6).
  const USE_FNV: bool
  /// `refusal.quorum` (CS-7): 2 with two classes, 1 with one (D.10.5).
  const QUORUM: int
  /// The principal keys that `hello`, `from_user`, `to_user`, `ask {to: user}`, `say`, and the
  /// `person:` keys of `create` pick from; `user` is always in it (D.10.1).
  const PRINCIPALS: Set[str]
  /// `false`: no `from_user` and no `to_user` action (the forum with no human, CS-2, CS-6).
  const HAS_USER: bool
  /// The firings one settle may make (D.10.4): enough for every settle within the bounds, so that
  /// `inv_settle_bound` holds; Apalache needs it bound to a literal.
  const MAX_FIRINGS: int
  /// `true`: the seed also creates the series `/s` (the `recurring` seed, D.10.7). The instances
  /// set it, so that simulation exercises recurring work.
  const SEED_SERIES: bool
  /// The values an action's `long` takes (D.10.4): `Set(false, true)` in the instances and the
  /// `holds` slice, where O1 and H4 meet a long text, and `Set(false)` in the other slices, where a
  /// long text changes nothing whatever the command.
  const LONG_TEXTS: Set[bool]
  /// How rare a call with an `op` or a long text is in simulation (README, "Simulation coverage"):
  /// each call draws a roll in `0..RARE_ODDS - 1`, and only roll 0 may carry an `op` and only the
  /// last roll a long text. 1 in the slices and the tests, where every value of `op` and `long` is
  /// drawn as before (one roll, no extra branching for TLC); 8 in the instances, where a uniform
  /// draw made half the calls `limit(TextTooLong)` and a third of the rest `op_id_conflict`, so that
  /// sessions retired early and few flows of D.10.7 ran to their end (review of WP-A2).
  const RARE_ODDS: int

  assume classes_known = CLASSES != Set() and classes_indexed(CLASSES)
  assume principals_known = PRINCIPALS.contains(USER_KEY) and PRINCIPALS.forall(p => PRINCIPAL_RANK.keys().contains(p))
  assume quorum_positive = QUORUM >= 1
  assume firings_positive = MAX_FIRINGS >= 1
  assume long_texts_known = LONG_TEXTS.contains(false)
  assume rare_odds_positive = RARE_ODDS >= 1

  val CFG: Cfg = { use_fnv: USE_FNV, quorum: QUORUM }

  /// The session ids of the bounds (D.10.7): two per class, each opened at most once.
  val SESSION_IDS: Set[int] = 1.to(SESSIONS_PER_CLASS * CLASSES.size())

  /// The classes in the order of CLASS_NAMES: the seed opens one session for each, in this order.
  val SEED_CLASSES: List[str] = CLASS_NAMES.select(c => CLASSES.contains(c))

  // ------------------------------------------------------------------ state (D.10.2)

  var s: State               // the abstract base state (D.10.3)
  var clock: int             // model-only time in ticks: where the shell's clock has got to
  var prev: State            // the state before the last action (step invariants)
  var mid: State             // the state after the last action's command, before its post-settle
                             // (D.8.1); `prev` for `init` and the time actions (step invariants)
  var last_action: Action    // the last action: name, parameters, op, outcome, and the flags of
                             // its settle (D.10.8)

  // ------------------------------------------------------------------ the seed (D.10.7)

  /// One `node create` of the seed: by session 1 at time 0, under ROOT, with capacity 1 and
  /// `take: false`.
  pure def seed_create(cfg: Cfg, st: State, c: str, name: str, ks: Set[Key], tags: Set[str], rep: RepeatChange): State =
    run_cmd(cfg, st, 0, c, 1, "", false, CCreate({
      parent: ROOT, name: Lit(name), keys: ks, take: false, capacity: 1, owners: Set(), repeat: rep, tags: tags,
    })).state

  /// Genesis at 0, then one `session hello` per class (sessions 1, 2, … in the order of
  /// `classes`), then `/w1` (key `Conv("c1")`) and `/w2` (key `Conv("c2")`), tagged `offer`, and
  /// with `series` the series `/s` of SEED_REPEAT (D.10.5), all by session 1. The replay driver
  /// sends the same commands (D.10.8).
  pure def seed_state(cfg: Cfg, classes: List[str], series: bool): State = {
    val s0 = classes.foldl(genesis(0), (st, c) =>
      run_cmd(cfg, st, 0, c, -1, "", false, CHello({ caps: Set(), principal: "" })).state)
    val c0 = classes[0]
    val s1 = seed_create(cfg, s0, c0, "w1", Set(Conv("c1")), Set(OFFER), RepKeep)
    val s2 = seed_create(cfg, s1, c0, "w2", Set(Conv("c2")), Set(OFFER), RepKeep)
    if (series) seed_create(cfg, s2, c0, "s", Set(), Set(), RepSet(SEED_REPEAT)) else s2
  }

  val SEED: State = seed_state(CFG, SEED_CLASSES, SEED_SERIES)

  pure val NO_FLAGS: SettleFlags = { saturated: false, stuck: false }

  pure val INIT_ACTION: Action =
    { name: "init", sess: -1, class: "", op: "", long: false, cmd: CInit, outcome: OutNone, settle: NO_FLAGS }

  action init = all {
    s' = SEED,
    clock' = 0,
    prev' = SEED,
    mid' = SEED,
    last_action' = INIT_ACTION,
  }

  // ------------------------------------------------------------------ bounds (D.10.7)

  /// The agent-made nodes (origin `Agent` or `Ancestor`): at most MAX_AGENT_NODES, the seed's
  /// included.
  pure def agent_nodes(st: State): int = st.nodes.keys().filter(n => is_agent_made(st.nodes.get(n))).size()

  pure def signal_nodes(st: State): int = st.nodes.keys().filter(n => st.nodes.get(n).origin == OSignal).size()

  /// `post` stays within `bound`, or does not grow past what `pre` had already: a firing may have
  /// taken a counter past its bound (a digest entry on USER_DIGEST, say), and the commands that do
  /// not raise it further stay enabled. Written as one comparison, not as `post <= bound or post <=
  /// pre`: TLC expands the definitions an action uses and explores each true disjunct of an `or`
  /// as a branch of its own, so each guard with both disjuncts true doubled every successor of
  /// `command` (each state's successors came out hundreds of times over, README, "TLC and the
  /// guards of an action").
  pure def le_bound(pre: int, post: int, bound: int): bool = post <= imax(bound, pre)

  /// D.10.7's guards on a command's result: `next_eid <= MAX_EID`; every `version`, `fence`, and
  /// `seq <= MAX_VER`; `attempts`, `checkpoints`, and approvals `<= MAX_COUNT`; the user entries,
  /// agent-made nodes, occurrences, signal nodes, requests, and decisions within their bounds; and
  /// the session ids within `1..SESSIONS_PER_CLASS × nclasses`. A `hello` opens the class its
  /// session id belongs to (`hello_class`), so the bound on ids is the bound per class: at most
  /// SESSIONS_PER_CLASS sessions of each class are opened in all (D.10.7).
  pure def within_bounds(nclasses: int, pre: State, post: State): bool = and {
    le_bound(pre.next_eid, post.next_eid, MAX_EID),
    post.nodes.keys().forall(n => {
      val a = post.nodes.get(n)
      val b = if (pre.nodes.keys().contains(n)) pre.nodes.get(n) else { ...a, version: 0, fence: 0, seq: 0, attempts: 0, checkpoints: 0 }
      and {
        le_bound(b.version, a.version, MAX_VER),
        le_bound(b.fence, a.fence, MAX_VER),
        le_bound(b.seq, a.seq, MAX_VER),
        le_bound(b.attempts, a.attempts, MAX_COUNT),
        le_bound(b.checkpoints, a.checkpoints, MAX_COUNT),
      }
    }),
    post.approvals.keys().forall(k => le_bound(get_or(pre.approvals, k, 0), post.approvals.get(k), MAX_COUNT)),
    le_bound(pre.user_entries.keys().size(), post.user_entries.keys().size(), MAX_USER_ENTRIES),
    le_bound(agent_nodes(pre), agent_nodes(post), MAX_AGENT_NODES),
    le_bound(occurrences_made(pre).size(), occurrences_made(post).size(), MAX_OCCURRENCES),
    le_bound(signal_nodes(pre), signal_nodes(post), MAX_SIGNAL_NODES),
    le_bound(pre.requests.keys().size(), post.requests.keys().size(), MAX_REQUESTS),
    le_bound(pre.decisions.keys().size(), post.decisions.keys().size(), MAX_DECISIONS),
    post.next_session - 1 <= SESSIONS_PER_CLASS * nclasses,
  }

  // ------------------------------------------------------------------ domains of the picks (D.10.4, D.10.7)

  pure val OPS: Set[str] = OP_IDS.union(Set(""))

  /// The rarity roll of a call (RARE_ODDS): roll 0 may carry an `op`, the last roll a long text;
  /// with RARE_ODDS = 1 the one roll may carry both.
  val ROLLS: Set[int] = 0.to(RARE_ODDS - 1)
  pure def op_picks(roll: int): Set[str] = if (roll == 0) OPS else Set("")
  def long_picks(roll: int): Set[bool] = if (roll == RARE_ODDS - 1) LONG_TEXTS else Set(false)

  /// The class of session id `x`: the ids alternate between the classes in the order of
  /// CLASS_NAMES (1, 3 the first; 2, 4 the second), as the seed opens them, so that each class
  /// opens at most SESSIONS_PER_CLASS sessions within the ids of the bounds (D.10.7: "2 classes ×
  /// 2 sessions each, session ids 1..4, each opened at most once"). A retired session leaves the
  /// state with its class, so a count per class could not be read from the state.
  def hello_class(x: int): str = SEED_CLASSES[(x - 1) % SEED_CLASSES.length()]

  pure val CAPS_CHOICES: Set[Set[str]] = Set(Set(), Set("x"))
  pure val LABEL_CHOICES: Set[Set[str]] = Set(Set(), Set("x"))
  pure val TAG_CHOICES: Set[Set[str]] = Set(Set(), Set(OFFER), Set(GONE))
  pure val NAMES: Set[Name] = Set(Lit("a1"), Lit("a2"), Lit("w1"))
  pure val LEASES: Set[int] = Set(-1, 1, 8)
  /// A second schedule for `update {repeat}` (period 20 divides 60, D.10.6).
  pure val ALT_REPEAT: RepeatSpec = { period: 20, offset: 0, run_within: 5 }
  pure val ALL_OUTCOMES: Set[Outcome] = Set(Done, Failed, Dropped, Superseded, Missed)
  pure val ALL_RELS: Set[LinkRel] = Set(Continues, DerivedFrom, ConflictsWith, Duplicate, SupersededBy)
  pure val SAID_KINDS: Set[SaidKind] = Set(SAck, SReply, SResult, SCorrection, SQuestion)
  pure val TO_USER_KINDS: Set[ToUserKind] = Set(TuAck, TuQuestion, TuResult)
  pure val REASONS: Set[ReleaseReason] = Set(Handoff, Blocked, Cannot)

  val PRINCIPAL_PICKS: Set[str] = PRINCIPALS.union(Set(""))

  /// The reserved nodes the actions name (D.10.7: moves, merges, closes, keys, updates, and routes
  /// aim at reserved nodes too, so that K8's refusals are exercised): ROOT, `@user` and its
  /// `decisions`, and the two directory parents. `sys`, `sys/signals`, and `@user/digest` add
  /// nothing these do not, and picking all eight spent half the calls on reserved nodes.
  pure val PICKED_RESERVED: Set[int] = Set(ROOT, USER, USER_DECISIONS, PEOPLE, TEAMS)

  /// Every node made by an agent or the server, the reserved nodes above, and the next id, which
  /// names no node (`not_found`).
  pure def node_picks(st: State): Set[int] =
    st.nodes.keys().filter(n => n >= FIRST_FREE_NODE).union(PICKED_RESERVED).union(Set(st.next_node))

  /// Person and team nodes, and ROOT (D.10.4: owners "persons or teams, ROOT included").
  pure def owner_picks(st: State): Set[int] =
    st.nodes.keys().filter(n => st.nodes.get(n).parent == PEOPLE or st.nodes.get(n).parent == TEAMS).union(Set(ROOT))

  /// The key sets of `act_create` (D.10.4): none, each conversation key, an object key beside a
  /// conversation key (CS-36), each principal's `person:` key and both aliases at once (D-3), the
  /// key of each user entry and of one that does not exist, and a `repeat:` key (A-90).
  pure def create_key_picks(st: State, principals: Set[str]): Set[Set[Key]] =
    Set(Set(), Set(Conv("c1")), Set(Conv("c2")), Set(Obj("o1")), Set(Conv("c1"), Obj("o1")), Set(RepeatSlot((FIRST_FREE_NODE, 0))))
      .union(principals.map(p => Set(Person(p))))
      .union(Set(principals.exclude(Set(USER_KEY)).map(p => Person(p))).exclude(Set(Set())))
      .union(st.user_entries.keys().union(Set(st.next_eid)).map(e => Set(User(e))))

  /// The keys of `node key`: the conversation keys, a message key, the object key, each
  /// principal's `person:` key, each user entry's key, and every key some node carries.
  pure def key_picks(st: State, principals: Set[str]): Set[Key] =
    Set(Conv("c1"), Conv("c2"), Msg(("c1", 1)), Obj("o1"))
      .union(principals.map(p => Person(p)))
      .union(st.user_entries.keys().map(e => User(e)))
      .union(st.nodes.keys().map(n => st.nodes.get(n).keys).flatten())

  /// Tokens: every hold's, and one stale token (H4): the latest fence of the lowest node whose
  /// latest hold has ended, or a token of ROOT that was never issued. Offering every spent fence
  /// beside the live ones made almost every `release` in simulation a stale write.
  pure def token_picks(st: State): Set[(int, int)] = {
    val spent = st.nodes.keys().filter(n => st.nodes.get(n).fence > 0 and not(st.holds.keys().contains((n, st.nodes.get(n).fence))))
    val m = set_min(spent, -1)
    st.holds.keys().union(Set(if (m >= 0) (m, st.nodes.get(m).fence) else (ROOT, 1)))
  }

  pure def question_picks(st: State): Set[int] = st.pquestions.keys().union(st.uquestions.keys())

  /// The basis references of `act_decide` (D.10.4): a reference to each agent- or server-made node
  /// (closed and merged ones included, and `at` itself, for `SelfLink`), at no version and one
  /// version ahead (`VersionAhead`, A-71), each decision, and an eid that does not exist.
  pure def basis_ref_picks(st: State): Set[BasisArg] =
    st.nodes.keys().filter(n => n >= FIRST_FREE_NODE)
      .map(n => Set(BaNode({ id: n, version: -1 }), BaNode({ id: n, version: st.nodes.get(n).version + 1 }))).flatten()
      .union(st.decisions.keys().map(d => BaEntry(d)))
      .union(Set(BaEntry(st.next_eid)))

  /// At most MAX_BASIS_REFS references (CS-22, D.10.4), grouped by size so that each size is drawn
  /// alike: none; one; or two, of which two distinct nodes (a decision resting on two lemmas, one of
  /// which a later close or retraction makes stale, CS-22, D-4), two distinct decisions, or a node
  /// and a decision.
  pure def basis_groups(st: State): Set[Set[List[BasisArg]]] = {
    val ns = st.nodes.keys().filter(n => n >= FIRST_FREE_NODE)
    val ds = st.decisions.keys()
    val pairs =
      tuples(ns, ns).filter(p => p._1 < p._2).map(p => [BaNode({ id: p._1, version: -1 }), BaNode({ id: p._2, version: -1 })])
        .union(tuples(ds, ds).filter(p => p._1 < p._2).map(p => [BaEntry(p._1), BaEntry(p._2)]))
        .union(tuples(ns, ds).map(p => [BaNode({ id: p._1, version: -1 }), BaEntry(p._2)]))
    Set(Set([]), basis_ref_picks(st).map(b => [b]), pairs).exclude(Set(Set()))
  }

  /// The waits of `release`, grouped by kind so that each kind is drawn alike: none; time and
  /// external; a question (each people or user question, and an id that names none); a node.
  pure def release_wait_groups(st: State, now: int): Set[Set[WaitArg]] =
    Set(
      Set(WaNone),
      Set(WaTime(now), WaTime(now + 2), WaExternal(now + 3)),
      question_picks(st).map(q => WaQuestion(q)).union(Set(WaQuestion(st.next_eid))),
      node_picks(st).map(t => WaNode(t)),
    )

  /// The `at` of the people actions (request, said, heard, ask person), grouped so that half the
  /// draws name a node that carries a conversation key, where these calls succeed, and half any
  /// node of `node_picks` (reserved nodes, nodes without a conversation key, and an id that names
  /// none, for the refusals).
  pure def conv_at_groups(st: State): Set[Set[int]] =
    Set(st.nodes.keys().filter(n => st.nodes.get(n).keys.exists(k => is_conversation(k))), node_picks(st))
      .exclude(Set(Set()))

  pure def singletons(xs: Set[int]): Set[Set[int]] = Set(Set()).union(xs.map(x => Set(x)))

  /// The sessions a command names: every live or silent session, or, when every session has
  /// retired, the lowest id of the bounds (`not_found(Session)`, and the repeat of a retired
  /// session's `op_id`, O1). Picking among the ids of the bounds alike spent about two calls in
  /// three on sessions that did not exist, since every session retires `session.retain` after its
  /// last touch (CS-8).
  pure def session_picks(st: State, ids: Set[int]): Set[int] =
    if (st.sessions.keys() != Set()) st.sessions.keys() else Set(set_min(ids, -1))

  // ------------------------------------------------------------------ actions (D.10.4)

  /// A command action: settled at `now`, then `run_cmd` within the bounds, then D.8.1's
  /// post-settle at `now`, the same `settle` the time actions run. The post-settle fires what the
  /// command made due at `now`: the Silence key of a session whose last hold the command ended
  /// after its `silence_at` (1.4, R27, D.4.8: the key lies at `max(silence_at, the largest
  /// lease_until of its holds, last_now)`), which D.9.2's T4 allows a command to queue at `now`
  /// (`step_T4`). The state after every action is therefore settled at its time (T3, T4), as the
  /// core's is after every call. `mid` keeps the state between the command and the post-settle, so
  /// that the step checks see the command alone (`check_step`) and each firing of the post-settle
  /// alone (`check_fire`, `invariants.qnt`'s `steps_of`). The bounds of D.10.7 guard the command,
  /// not the firings, as for the time actions. The class of the call is the session's own, and
  /// `cls` for a session that does not exist (never opened, or retired: the state no longer
  /// records its class), and for `hello`.
  action command(name: str, sess: int, cls: str, op: str, long: bool, cmd: Cmd): bool = {
    val now = imax(clock, s.last_now)
    val c = if (has_session(s, sess)) s.sessions.get(sess).class else cls
    val r = run_cmd(CFG, s, now, c, sess, op, long, cmd)
    val p = settle(r.state, now, MAX_FIRINGS)
    all {
      settled(s, now),
      within_bounds(CLASSES.size(), s, r.state),
      s' = p.state,
      clock' = clock,
      prev' = s,
      mid' = r.state,
      last_action' = { name: name, sess: sess, class: c, op: op, long: long, cmd: cmd, outcome: r.outcome,
                       settle: { saturated: p.saturated, stuck: p.stuck } },
    }
  }

  // session -------------------------------------------------------------------------------------

  /// `session hello {caps?, principal?}`, by the class of the session id it opens (`hello_class`,
  /// D.10.7). The model sends no `intent` and no `from_user`, so the call carries no text and has
  /// no `long`.
  action act_hello = {
    nondet cls = Set(hello_class(s.next_session)).oneOf()
    nondet caps = CAPS_CHOICES.oneOf()
    nondet principal = PRINCIPAL_PICKS.oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    command("act_hello", -1, cls, op, false, CHello({ caps: caps, principal: principal }))
  }

  // node ----------------------------------------------------------------------------------------

  action act_create = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet parent = node_picks(s).oneOf()
    nondet name = NAMES.oneOf()
    nondet key_set = create_key_picks(s, PRINCIPALS).oneOf()
    nondet take = Set(false, true).oneOf()
    nondet capacity = CAPACITIES.oneOf()
    nondet owners = Set(Set()).union(owner_picks(s).map(o => Set(o))).oneOf()
    nondet repeat = Set(RepKeep, RepSet(SEED_REPEAT)).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_create", sess, cls, op, long, CCreate({
      parent: parent, name: name, keys: key_set, take: take, capacity: capacity, owners: owners,
      repeat: repeat, tags: Set(),
    }))
  }

  action act_update = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet n = node_picks(s).oneOf()
    val v = if (has_node(s, n)) s.nodes.get(n).version else 1
    val dv = if (has_node(s, n)) s.nodes.get(n).digest_version else 1
    nondet ver = Set(v, v - 1).oneOf()
    nondet dver = Set(-1, dv, dv - 1).oneOf()
    nondet content = Set(-1, 1, 2).oneOf()
    nondet status = Set(-1, 0, 1).oneOf()
    nondet digest = Set(-1, 1).oneOf()
    nondet capacity = Set(-1).union(CAPACITIES).oneOf()
    nondet needs_add = LABEL_CHOICES.oneOf()
    nondet needs_remove = LABEL_CHOICES.oneOf()
    nondet tags_add = TAG_CHOICES.oneOf()
    nondet tags_remove = TAG_CHOICES.oneOf()
    nondet due = Set(DueKeep, DueClear, DueSet(clock + 2)).oneOf()
    nondet depends = Set(IdsKeep, IdsSet(Set())).union(node_picks(s).map(m => IdsSet(Set(m)))).oneOf()
    nondet owners = Set(IdsKeep, IdsSet(Set())).union(owner_picks(s).map(o => IdsSet(Set(o)))).oneOf()
    nondet repeat = Set(RepKeep, RepClear, RepSet(SEED_REPEAT), RepSet(ALT_REPEAT)).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_update", sess, cls, op, long, CUpdate({
      node: n, ver: ver, dver: dver, content: content, status: status, digest: digest, capacity: capacity,
      needs_add: needs_add, needs_remove: needs_remove, tags_add: tags_add, tags_remove: tags_remove,
      due: due, depends: depends, owners: owners, repeat: repeat,
    }))
  }

  action act_link = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet n = node_picks(s).oneOf()
    nondet rel = ALL_RELS.oneOf()
    nondet m = node_picks(s).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    command("act_link", sess, cls, op, false, CLink({ node: n, rel: rel, to: m }))
  }

  action act_key_add = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet n = node_picks(s).oneOf()
    nondet k = key_picks(s, PRINCIPALS).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    command("act_key_add", sess, cls, op, false, CKeyAdd({ node: n, key: k }))
  }

  /// The key is one the node carries, or the object key, which it may not carry (`not_found(Key)`).
  action act_key_remove = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet n = node_picks(s).oneOf()
    nondet k = (if (has_node(s, n)) s.nodes.get(n).keys else Set()).union(Set(Obj("o1"))).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    command("act_key_remove", sess, cls, op, false, CKeyRemove({ node: n, key: k }))
  }

  action act_close = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet n = node_picks(s).oneOf()
    nondet outcome = ALL_OUTCOMES.oneOf()
    nondet settles = singletons(s.requests.keys()).oneOf()
    nondet tok = Set(NoToken).union(token_picks(s).map(t => SomeToken(t))).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_close", sess, cls, op, long, CClose({ node: n, outcome: outcome, settles: settles, token: tok }))
  }

  /// The node is a closed one, ROOT (open: `NotClosed`), or an id that names none.
  action act_reopen = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet n = s.nodes.keys().filter(m => is_closed(s, m)).union(Set(ROOT, s.next_node)).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_reopen", sess, cls, op, long, CReopen(n))
  }

  action act_move = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet n = node_picks(s).oneOf()
    nondet parent = node_picks(s).oneOf()
    nondet name = NAMES.oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    command("act_move", sess, cls, op, false, CMove({ node: n, parent: parent, name: name }))
  }

  action act_merge = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet from_node = node_picks(s).oneOf()
    nondet into = node_picks(s).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    command("act_merge", sess, cls, op, false, CMerge({ from: from_node, into: into }))
  }

  action act_hide = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet entry = s.entries.keys().union(Set(s.next_eid)).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_hide", sess, cls, op, long, CHide(entry))
  }

  // hold ----------------------------------------------------------------------------------------

  action act_take = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet n = node_picks(s).oneOf()
    nondet lease = LEASES.oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    command("act_take", sess, cls, op, false, CTake({ node: n, lease: lease }))
  }

  action act_next = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    command("act_next", sess, cls, op, false, CNext)
  }

  action act_checkpoint = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet tok = token_picks(s).oneOf()
    nondet intent = Set(false, true).oneOf()
    nondet done = Set(false, true).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_checkpoint", sess, cls, op, long, CCheckpoint({ token: tok, intent: intent, intent_done: done }))
  }

  /// 1.8 (§10.2): `hold check {target, intent_done}`, by any session, without a hold.
  action act_check_intent = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet n = node_picks(s).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_check_intent", sess, cls, op, long, CCheckIntent(n))
  }

  action act_renew = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet tok = token_picks(s).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_renew", sess, cls, op, long, CRenew(tok))
  }

  action act_release = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet tok = token_picks(s).oneOf()
    nondet reason = REASONS.oneOf()
    nondet wait_group = release_wait_groups(s, clock).oneOf()
    nondet wait = wait_group.oneOf()
    // 1.12 (§10.3): a release may settle a request, as a close does.
    nondet settles = singletons(s.requests.keys()).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_release", sess, cls, op, long, CRelease({ token: tok, reason: reason, wait: wait, settles: settles }))
  }

  // talk: the user ------------------------------------------------------------------------------

  action act_from_user = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet principal = PRINCIPAL_PICKS.oneOf()
    nondet answers = Set(-1).union(question_picks(s)).oneOf()
    nondet clarifies = Set(-1).union(s.requests.keys()).oneOf()
    nondet n = Set(-1).union(node_picks(s)).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    all {
      HAS_USER,
      command("act_from_user", sess, cls, op, long, CFromUser({ principal: principal, answers: answers, clarifies: clarifies, node: n })),
    }
  }

  action act_to_user = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet principal = PRINCIPAL_PICKS.oneOf()
    nondet kind = TO_USER_KINDS.oneOf()
    nondet re = singletons(s.requests.keys().union(s.user_entries.keys()).union(s.digest.keys())
                             .union(s.pquestions.keys()).union(Set(s.next_eid))).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    all {
      HAS_USER,
      command("act_to_user", sess, cls, op, long, CToUser({ principal: principal, kind: kind, re: re })),
    }
  }

  action act_route = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet e = s.entries.keys().union(Set(s.next_eid)).oneOf()
    nondet target = Set(-1).union(node_picks(s)).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_route", sess, cls, op, long, CRoute({ entry: e, to: target }))
  }

  action act_ask_user = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet at = node_picks(s).oneOf()
    nondet has_default = Set(false, true).oneOf()
    nondet target = PRINCIPALS.oneOf()
    nondet again = Set(false, true).oneOf()
    nondet digest = Set(1, 2).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_ask_user", sess, cls, op, long,
      CAskUser({ at: at, has_default: has_default, to: target, again: again, digest: digest }))
  }

  // talk: people --------------------------------------------------------------------------------

  action act_request = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet at_group = conv_at_groups(s).oneOf()
    nondet at = at_group.oneOf()
    nondet origin = REQUEST_ORIGINS.oneOf()
    nondet where_key = REQUEST_WHERES.oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_request", sess, cls, op, long, CRequest({ at: at, origin: origin, where_key: where_key }))
  }

  /// `where` among none, `Conv("c1")`, and `Conv("c2")` (D.10.4, 1.3): on a node that carries both
  /// conversation keys, a reply in one of them leaves the other's requests and commitments alone.
  action act_said = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet at_group = conv_at_groups(s).oneOf()
    nondet at = at_group.oneOf()
    nondet kind = SAID_KINDS.oneOf()
    nondet where_key = REQUEST_WHERES.oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_said", sess, cls, op, long, CSaid({ at: at, kind: kind, where_key: where_key }))
  }

  action act_confirm = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet said = s.saids.keys().union(Set(s.next_eid)).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_confirm", sess, cls, op, long, CConfirm(said))
  }

  action act_heard = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet at_group = conv_at_groups(s).oneOf()
    nondet at = at_group.oneOf()
    nondet answers = Set(-1).union(question_picks(s)).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_heard", sess, cls, op, long, CHeard({ at: at, answers: answers }))
  }

  action act_ask_person = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet at_group = conv_at_groups(s).oneOf()
    nondet at = at_group.oneOf()
    nondet blocking = Set(false, true).oneOf()
    nondet again = Set(false, true).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_ask_person", sess, cls, op, long, CAskPerson({ at: at, blocking: blocking, again: again }))
  }

  // talk: agents --------------------------------------------------------------------------------

  action act_ask_agent = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet at = node_picks(s).oneOf()
    nondet target = SESSION_IDS.map(x => AgSession(x))
                      .union(node_picks(s).map(n => AgHolder(n)))
                      .union(node_picks(s).map(n => AgHolders(n))).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_ask_agent", sess, cls, op, long, CAskAgent({ at: at, to: target }))
  }

  action act_answer = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet q = s.entries.keys().filter(e => s.entries.get(e).ty == EQuestion).union(Set(s.next_eid)).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_answer", sess, cls, op, long, CAnswer(q))
  }

  action act_say = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet at = node_picks(s).oneOf()
    nondet target = Set(SayNone, SayUser)
                      .union(SESSION_IDS.map(x => SaySession(x)))
                      .union(node_picks(s).map(n => SayHolder(n)))
                      .union(node_picks(s).map(n => SayHolders(n)))
                      .union(PRINCIPALS.map(p => SayPrincipal(p))).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_say", sess, cls, op, long, CSay({ at: at, to: target }))
  }

  action act_wait = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet for_node = Set(-1).union(node_picks(s)).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    command("act_wait", sess, cls, op, false, CWait(for_node))
  }

  // talk: review --------------------------------------------------------------------------------

  action act_decide = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet at = node_picks(s).oneOf()
    nondet settles = singletons(s.requests.keys()).oneOf()
    nondet supersedes = Set(-1).union(s.decisions.keys()).oneOf()
    nondet basis_group = basis_groups(s).oneOf()
    nondet basis = basis_group.oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_decide", sess, cls, op, long, CDecide({ at: at, settles: settles, supersedes: supersedes, basis: basis }))
  }

  action act_retract = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet target = node_picks(s).map(n => TNode(n))
                      .union(s.entries.keys().map(e => TEntry(e)))
                      .union(Set(TEntry(s.next_eid))).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_retract", sess, cls, op, long, CRetract(target))
  }

  action act_approve = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet n = node_picks(s).oneOf()
    val v = if (has_node(s, n)) s.nodes.get(n).version else 1
    nondet ver = Set(v, v + 1).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_approve", sess, cls, op, long, CApprove({ node: n, ver: ver }))
  }

  action act_object = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet n = node_picks(s).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_object", sess, cls, op, long, CObject(n))
  }

  action act_resolve = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet o = s.objections.keys().union(Set(s.next_eid)).oneOf()
    // 1.11: a verdict, or a deferral, which takes none (§11.3).
    nondet v = Set((false, false), (true, false), (false, true)).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_resolve", sess, cls, op, long, CResolve({ objection: o, upheld: v._1, defer: v._2 }))
  }

  // control and view ----------------------------------------------------------------------------

  action act_pause = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet n = node_picks(s).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    nondet long = long_picks(roll).oneOf()
    command("act_pause", sess, cls, op, long, CPause(n))
  }

  /// The scope is a paused node, ROOT (when not paused: `not_found(Pause)`), or an id that names none.
  action act_unpause = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet n = s.pauses.keys().union(Set(ROOT, s.next_node)).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    command("act_unpause", sess, cls, op, false, CUnpause(n))
  }

  action act_view = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = CLASSES.oneOf()
    nondet n = node_picks(s).oneOf()
    nondet roll = ROLLS.oneOf()
    nondet op = op_picks(roll).oneOf()
    command("act_view", sess, cls, op, false, CView(n))
  }

  // calls of the slices (D.10.7, WP-A3) ---------------------------------------------------------

  /// A command action of a slice (`slices/*.qnt`): a session among the live ones
  /// (`session_picks`), a command among `cmds`, and an `op` with a long text or not among
  /// `op_long`, sent as `command` sends every call, under its action name of D.10.4. A slice picks
  /// each command whole from a small set of the values its invariants need (D.10.7), so that TLC
  /// and Apalache meet few branches; its counterexamples replay through `last_action`, which
  /// records the command (D.10.8). The class is the session's own, or any for a session that does
  /// not exist.
  action slice_call(name: str, cmds: Set[Cmd], op_long: Set[(str, bool)]): bool = {
    nondet sess = session_picks(s, SESSION_IDS).oneOf()
    nondet cls = (if (has_session(s, sess)) Set(s.sessions.get(sess).class) else CLASSES).oneOf()
    nondet cmd = cmds.oneOf()
    nondet ol = op_long.oneOf()
    all { cmds != Set(), command(name, sess, cls, ol._1, ol._2, cmd) }
  }

  /// `session hello` for a slice: the caps and the principal among the given ones.
  action slice_hello(caps: Set[Set[str]], principals: Set[str]): bool = {
    nondet c = caps.oneOf()
    nondet p = principals.oneOf()
    command("act_hello", -1, hello_class(s.next_session), "", false, CHello({ caps: c, principal: p }))
  }

  // time (D.10.4) -------------------------------------------------------------------------------

  /// `act_tick(dt)`: the clock moves by `dt` (1..MAX_DT), then everything due fires.
  action act_tick = {
    nondet dt = 1.to(MAX_DT).oneOf()
    all {
      clock + dt <= MAX_TIME,
      val r = settle(s, clock + dt, MAX_FIRINGS)
      all {
        s' = r.state,
        clock' = clock + dt,
        prev' = s,
        mid' = s,
        last_action' = { name: "act_tick", sess: -1, class: "", op: "", long: false, cmd: CTick(dt),
                         outcome: OutNone, settle: { saturated: r.saturated, stuck: r.stuck } },
      },
    }
  }

  /// `act_tick_next`: the clock jumps to the first queued key, at or before MAX_TIME, which fires.
  action act_tick_next = {
    val t = next_deadline(s)
    all {
      t > clock,
      t <= MAX_TIME,
      val r = settle(s, t, MAX_FIRINGS)
      all {
        s' = r.state,
        clock' = t,
        prev' = s,
        mid' = s,
        last_action' = { name: "act_tick_next", sess: -1, class: "", op: "", long: false, cmd: CTickNext,
                         outcome: OutNone, settle: { saturated: r.saturated, stuck: r.stuck } },
      },
    }
  }

  // step ----------------------------------------------------------------------------------------

  /// The `session`, `node`, and `control` actions of D.10.4, and `view`.
  action node_step = any {
    act_hello, act_create, act_update, act_link, act_key_add, act_key_remove, act_close, act_reopen,
    act_move, act_merge, act_hide, act_pause, act_unpause, act_view,
  }

  /// The `hold` actions: a group of their own, so that a hold is taken and released (with a wait,
  /// or `cannot`) while its lease is live.
  action hold_step = any {
    act_take, act_next, act_checkpoint, act_renew, act_release, act_check_intent,
  }

  /// The `talk` actions of a request's flow (§9.4, §9.2, §8.1): recording it, the `said` that
  /// answers it and fulfils commitments, the decision that settles it, the people questions and
  /// `heard`, and the `to_user` that answers a user request.
  action people_step = any {
    act_request, act_said, act_heard, act_ask_person, act_decide, act_to_user,
  }

  /// The other `talk` actions.
  action talk_step = any {
    act_from_user, act_route, act_ask_user, act_confirm, act_ask_agent, act_answer, act_say,
    act_wait, act_retract, act_approve, act_object, act_resolve,
  }

  action time_step = any { act_tick, act_tick_next }

  /// Time and the four groups of commands are picked alike. A random run of 40 steps then moves
  /// the clock about eight times, enough to reach the deadlines of D.10.5, while its sessions call
  /// often enough to stay live for part of it: a flat choice among the 39 actions moved the clock
  /// about twice, and a choice between time and the commands retired every session within the
  /// first half of the run (CS-8). The flow of a request has a group of its own, so that a request
  /// is answered, decided, and settled, and a commitment fulfilled, in a fair share of the runs
  /// (README, "Simulation coverage").
  action step = any { time_step, node_step, hold_step, people_step, talk_step }

  // ------------------------------------------------------------------ reachability witnesses (simulation)

  // `quint run … --witnesses w_…` counts the runs in which each holds in some state
  // (`scripts/simulate.sh` passes them all). A check over states that simulation never reaches
  // passes vacuously, so the simulation's result is reported with these counts (README,
  // "Simulation coverage"; review of WP-A2): each command's success, and the states the invariants
  // of D.9.2 are about.

  /// The last action was the command `name`, and it succeeded (not a replay).
  def ok_call(name: str): bool =
    last_action.name == name and match last_action.outcome { | OutOk(_) => true | _ => false }

  val w_ok_hello = ok_call("act_hello")
  val w_ok_create = ok_call("act_create")
  val w_ok_update = ok_call("act_update")
  val w_ok_link = ok_call("act_link")
  val w_ok_key_add = ok_call("act_key_add")
  val w_ok_key_remove = ok_call("act_key_remove")
  val w_ok_close = ok_call("act_close")
  val w_ok_reopen = ok_call("act_reopen")
  val w_ok_move = ok_call("act_move")
  val w_ok_merge = ok_call("act_merge")
  val w_ok_hide = ok_call("act_hide")
  val w_ok_take = ok_call("act_take")
  val w_ok_next = ok_call("act_next")
  val w_ok_checkpoint = ok_call("act_checkpoint")
  val w_ok_renew = ok_call("act_renew")
  val w_ok_release = ok_call("act_release")
  val w_ok_from_user = ok_call("act_from_user")
  val w_ok_to_user = ok_call("act_to_user")
  val w_ok_route = ok_call("act_route")
  val w_ok_ask_user = ok_call("act_ask_user")
  val w_ok_request = ok_call("act_request")
  val w_ok_said = ok_call("act_said")
  val w_ok_confirm = ok_call("act_confirm")
  val w_ok_heard = ok_call("act_heard")
  val w_ok_ask_person = ok_call("act_ask_person")
  val w_ok_ask_agent = ok_call("act_ask_agent")
  val w_ok_answer = ok_call("act_answer")
  val w_ok_say = ok_call("act_say")
  val w_ok_wait = ok_call("act_wait")
  val w_ok_decide = ok_call("act_decide")
  val w_ok_retract = ok_call("act_retract")
  val w_ok_approve = ok_call("act_approve")
  val w_ok_object = ok_call("act_object")
  val w_ok_resolve = ok_call("act_resolve")
  val w_ok_check_intent = ok_call("act_check_intent")
  val w_ok_pause = ok_call("act_pause")
  val w_ok_unpause = ok_call("act_unpause")
  val w_ok_view = ok_call("act_view")

  val w_replay = last_action.outcome == OutReplay
  val w_op_conflict = last_action.outcome == OutErr(ErrOpIdConflict)
  val w_stale_token = last_action.outcome == OutErr(ErrStaleToken)
  /// The last action was a command whose post-settle fired (D.8.1; R27's Silence key at `now`).
  val w_post_settle = is_command_action(last_action) and mid != s
  val w_request_answered = s.requests.keys().exists(r => s.requests.get(r).answered_by >= 0)
  val w_request_settled = s.requests.keys().exists(r => s.requests.get(r).settled != Unsettled)
  val w_user_request_answered = s.requests.keys().exists(r => {
    val q = s.requests.get(r)
    q.answered_by >= 0 and q.requester != ReqExternal
  })
  val w_moved_request = s.requests.keys().exists(r => s.requests.get(r).moved_from >= 0)
  val w_commitment_fulfilled = s.commitments.keys().exists(c => s.commitments.get(c).state != CmOpen)
  /// A node that carries two conversation keys (§9.4, 1.3).
  val w_two_conversations = s.nodes.keys().exists(n => s.nodes.get(n).keys.filter(k => is_conversation(k)).size() >= 2)
  /// A node with an answered request in one conversation and an unanswered one in another.
  val w_conversations_apart = s.requests.keys().exists(r1 => s.requests.keys().exists(r2 => {
    val q1 = s.requests.get(r1)
    val q2 = s.requests.get(r2)
    q1.node == q2.node and q1.conv != q2.conv and q1.answered_by >= 0 and q2.answered_by < 0
  }))
  val w_pq_answered = s.pquestions.keys().exists(q => match s.pquestions.get(q).state { | PqAnswered(_) => true | _ => false })
  val w_pq_expired = s.pquestions.keys().exists(q => s.pquestions.get(q).state == PqExpired)
  val w_pq_escalated = s.pquestions.keys().exists(q => match s.pquestions.get(q).state { | PqEscalated(_) => true | _ => false })
  val w_uq_answered = s.uquestions.keys().exists(q => match s.uquestions.get(q).state { | UqAnswered(_) => true | _ => false })
  val w_uq_defaulted = s.uquestions.keys().exists(q => s.uquestions.get(q).state == UqDefaulted)
  val w_uq_lapsed = s.uquestions.keys().exists(q => s.uquestions.get(q).state == UqLapsed)
  val w_uq_readdressed = s.uquestions.keys().exists(q => s.uquestions.get(q).stage > 0)
  /// A node waits on a people question or a question for the user (W1, `wait_on_uq_resolves`).
  val w_wait_on_question = s.nodes.keys().exists(n => match s.nodes.get(n).wait {
    | HasWait(w) => match w.on { | WQuestion(_) => true | WUserQuestion(_) => true | _ => false }
    | NoWait => false
  })
  val w_refusal = s.refusals.keys() != Set()
  /// A class reached its refusal quorum on a node (E1, CS-7).
  val w_class_refused = s.refusals.keys().exists(k => class_refused(s, k._1, k._2, clock))
  val w_refusal_drop = s.digest.keys().exists(d => match s.digest.get(d).item { | DiRefusalDrop(_) => true | _ => false })
  val w_digest_shown = s.digest.keys().exists(d => s.digest.get(d).state != DgPending)
  val w_decision_superseded = s.decisions.keys().exists(d => match s.decisions.get(d).state { | DSuperseded(_) => true | _ => false })
  val w_decision_retracted = s.decisions.keys().exists(d => match s.decisions.get(d).state { | DRetracted(_) => true | _ => false })
  /// A decision resting on two node references (CS-22, D-4).
  val w_basis_two_nodes = s.decisions.keys().exists(d => {
    val b = s.decisions.get(d).basis
    b.length() == 2 and b.foldl(true, (acc, x) => acc and match x { | BNode(_) => true | _ => false })
  })
  val w_stale_basis = s.decisions.keys().exists(d => stale_decision(s, d))
  val w_merged = s.nodes.keys().exists(n => merged(s, n))
  val w_signal = s.signals.keys() != Set()
  val w_missed = s.nodes.keys().exists(n => match s.nodes.get(n).life { | Closed(c) => c.outcome == Missed | _ => false })
  val w_assume_mark = s.nodes.keys().exists(n => s.nodes.get(n).assume_marks != Set())
  val w_open_intent = s.nodes.keys().exists(n => s.nodes.get(n).open_intent.eid >= 0)
  /// 1.8: a closed node an upheld objection corrected (§11.3), and an offer that went stale (§14).
  val w_upheld = s.nodes.keys().exists(n => upheld(s, n))
  val w_stale_offer = s.nodes.keys().exists(n => f_stale(s, n, clock))
  val w_person_and_team = s.nodes.keys().exists(n => s.nodes.get(n).parent == PEOPLE)
                          and s.nodes.keys().exists(n => s.nodes.get(n).parent == TEAMS)
  /// A session still calls in the second half of the clock's range.
  val w_live_session_late = clock >= MAX_TIME / 2 and s.sessions.keys().exists(x => not(s.sessions.get(x).silent))

  // ------------------------------------------------------------------ invariants (D.9.2, WP-A3)

  /// Every check of D.9.2 that the model makes (`invariants.qnt`, `all_invariants`): the state
  /// invariants on `s` at `clock`, the step invariants on the last action (its command from `prev`
  /// to `mid`, and the firings of its settle from `mid` to `s`, replayed), and T2 over pairs of
  /// times up to T2_K ticks ahead (D.10.7). The slices check their parts of it (`slices/*.qnt`).
  val inv_all = all_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, T2_K)

  /// The same without T2, whose pairs of settles cost most of a simulation step (README).
  val inv_all_but_T2 = all_invariants(prev, mid, s, clock, last_action, MAX_FIRINGS, -1)

  // ------------------------------------------------------------------ liveness (§27, D.10.7, WP-A3)

  // The server liveness properties of §27, within the model's bounds (`invariants.qnt`): the
  // objects of each kind whose deadlines lie within MAX_TIME are eventually gone for good. The
  // slices check them with TLC under weak fairness of `act_tick`, `act_tick_next`, and their calls
  // (`liveness_holds`, `liveness_recurring`, `liveness_user`).

  /// Every queued key with `at <= MAX_TIME` eventually fires.
  temporal deadline_fires = eventually(always(no_key_within(s)))

  /// Every hold ends (its lease within MAX_TIME).
  temporal lease_ends = eventually(always(no_lease_within(s)))

  /// Every pause ends (within MAX_TIME).
  temporal pause_ends = eventually(always(no_pause_within(s)))

  /// Every occurrence whose window ends within MAX_TIME closes or is flagged.
  temporal occurrence_resolves = eventually(always(occurrences_resolved(s, clock)))

  /// Every question for the user leaves `Pending`: answered, defaulted, or lapsed (CS-2).
  temporal uq_resolves = eventually(always(uq_resolved(s)))

  /// Every wait on a question for the user resolves.
  temporal wait_on_uq_resolves = eventually(always(uq_waits_resolved(s, clock)))
}
