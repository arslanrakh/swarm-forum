// -*- mode: Bluespec; -*-
/**
 * The invariants of D.9.2 (§26) as predicates of the model (D.10.1, WP-A3).
 *
 * A state invariant is a `pure def inv_X(st, now)` (sometimes with the last action) over a state
 * settled at `now`, which is the model's `clock` (D.10.2). A step invariant is a `pure def
 * step_X(c: Step)` over one step of an action: the state before it, the state after it, its `now`,
 * and `last_action` (D.10.2, D.9.2's `check_step` and `check_fire`). `forum.qnt` and the slices
 * apply them to their variables; their names are the Quint names of D.9.2.
 *
 * Every action ends with a settle, as every call does in D.8.1: a command action runs its command
 * and then the post-settle at its `now`, and a time action settles to its clock. `steps_of` splits
 * an action into the steps D.9.1 checks: the command's own step, from `prev` to `mid` (one
 * `check_step`), and the settle, from `mid` to `s` (D.9.2's `check_fire`, checked firing by
 * firing). The settle is replayed from `mid`: the queue's minimum by its definition (`queue_min`,
 * not `settle`'s `first_key`) while it lies at or before the target, keeping the state before and
 * after each firing (`Firing`). `step_T4` checks that the replay ends where the action's own settle
 * ended. A time action runs no command (`mid` is `prev`), and `init` neither command nor firing.
 *
 * Every check is written from the specification and D.9.2, over the base maps, and reuses the model's
 * transitions only where D.9.2 names the model's own definition (the flags of `flags.qnt`, `chain`,
 * `audience`, the queue of `deadlines.qnt`, `classify` of `next`). Where D.9.1 asks for a naive
 * definition independent of the code it checks, it is written here again (`up_path`, `conv_node_naive`,
 * `default_conv_naive`, `reach_naive`, `resolved_naive`, `paused_naive`), so that a wrong helper in
 * `commands.qnt` is caught by its check.
 *
 * Not checked in the model, with D.9.2's reasons: R1 (the model has no indices), Total (no `evolve`),
 * K2 (keys are atoms), and G1 (D.0.3 dropped WP-A5's `guide.qnt`; `check_guide` checks it in Rust).
 * F1 has nothing to compare (no stored flags), so `inv_F` is F2. AR is the definition `archived`.
 *
 * Locals are never named after a variable of `forum.qnt` or of a slice (`s`, `clock`, `prev`,
 * `last_action`): Apalache prefixes local names like variables, and TLC refuses the clash (README).
 */
module invariants {
  import types.* from "./types"
  import params.* from "./params"
  import basics.* from "./basics"
  import flags.* from "./flags"
  import deadlines.* from "./deadlines"
  import commands.* from "./commands"

  // ================================================================== the step (D.9.1)

  /// One firing of a settle: the state before it, its key, and the state after it (`check_fire`).
  type Firing = { pre: State, key: DeadlineKey, post: State }

  /// One step of an action as the step invariants see it, a command's or a settle's:
  /// - `pre`, `post`: the states before and after it;
  /// - `now`: for a command and its post-settle, `max(clock, prev.last_now)` (its `ctx.now`
  ///   clamped, D.8.1); for a time action, the clock it settled to;
  /// - `cmd`: the step of a command (`check_step`); otherwise a settle (`check_fire` per firing);
  /// - `fs`: the firings of a settle, replayed from `pre` (none for a command);
  /// - `end`: the state the replay ends in, and `rest`: the queue's minimum there (a settle's only).
  type Step = {
    pre: State,
    post: State,
    now: int,
    la: Action,
    cmd: bool,
    fs: List[Firing],
    end: State,
    rest: DeadlineKey,
  }

  pure def is_time_action(la: Action): bool = la.name == "act_tick" or la.name == "act_tick_next"

  pure def is_command_action(la: Action): bool = la.name != "init" and not(is_time_action(la))

  pure def outcome_ok(o: CmdOutcome): bool = match o { | OutOk(_) => true | _ => false }

  /// A settle replayed firing by firing: the queue's minimum (by the definition of the queue,
  /// D.4.8), while it lies at or before `max(target, last_now)`, at most `max_firings` times.
  pure def replay_settle(st: State, target: int, max_firings: int): { fs: List[Firing], end: State } = {
    val t = imax(target, st.last_now)
    // Once nothing is due, the state no longer changes: later passes skip the queue (`idle`).
    val r = range(0, max_firings).foldl({ cur: st, fs: [], idle: false }, (acc, i) =>
      if (acc.idle) acc
      else {
        val k = queue_min(acc.cur)
        if (k != NO_KEY and k.at <= t) {
          val nxt = fire(acc.cur, k)
          { cur: nxt, fs: acc.fs.append({ pre: acc.cur, key: k, post: nxt }), idle: false }
        } else { ...acc, idle: true }
      })
    { fs: r.fs, end: r.cur }
  }

  /// The step of a command, from `pre` to the state after it and before its post-settle
  /// (`check_step`), at its clamped `now`.
  pure def call_step(pre: State, post: State, clk: int, la: Action): Step =
    { pre: pre, post: post, now: imax(clk, pre.last_now), la: la, cmd: true, fs: [], end: post, rest: NO_KEY }

  /// The settle that ends an action, from `pre` to `post` at `now`, replayed firing by firing
  /// (`check_fire`).
  pure def settle_step(pre: State, post: State, now: int, la: Action, max_firings: int): Step = {
    val r = replay_settle(pre, now, max_firings)
    { pre: pre, post: post, now: now, la: la, cmd: false, fs: r.fs, end: r.end, rest: queue_min(r.end) }
  }

  /// One step from `pre` to `post`: a command's own step for a command action, or for a time
  /// action (and `init`) its settle to `clk`.
  pure def step_of(pre: State, post: State, clk: int, la: Action, max_firings: int): Step =
    if (is_command_action(la)) call_step(pre, post, clk, la) else settle_step(pre, post, clk, la, max_firings)

  /// The steps of one action that D.9.1 checks (D.8.1): `call`, the command's own step from `pre`
  /// (`prev`) to `cmd_post` (`mid`), checked when `has_call`; and `fire`, the settle that ends the
  /// action, from `cmd_post` to `post` (`s`): a command's post-settle at its `now`, or a time
  /// action's settle to its clock. A time action and `init` run no command: `cmd_post` is `pre`,
  /// and `call` is never checked.
  type Steps = { call: Step, fire: Step, has_call: bool }

  pure def steps_of(pre: State, cmd_post: State, post: State, clk: int, la: Action, max_firings: int): Steps = {
    val has_call = is_command_action(la)
    val now = if (has_call) imax(clk, pre.last_now) else clk
    { call: call_step(pre, cmd_post, clk, la), fire: settle_step(cmd_post, post, now, la, max_firings), has_call: has_call }
  }

  /// `p` holds on every step of the action: the settle, and the command when there is one.
  pure def each_step(cs: Steps, p: Step => bool): bool = p(cs.fire) and (not(cs.has_call) or p(cs.call))

  pure def all_firings(c: Step, p: Firing => bool): bool = c.fs.foldl(true, (ok, f) => ok and p(f))

  // ================================================================== naive helpers (D.9.1)

  pure def new_ids(before: Set[a], after: Set[a]): Set[a] = after.exclude(before)

  pure def node_open(st: State, n: int): bool = st.nodes.keys().contains(n) and st.nodes.get(n).life == Open

  pure def node_closed(st: State, n: int): bool = st.nodes.keys().contains(n) and st.nodes.get(n).life != Open

  /// The outcome of a closed node, if it is closed with `o`.
  pure def closed_with(st: State, n: int, o: Outcome): bool =
    st.nodes.keys().contains(n) and match st.nodes.get(n).life { | Closed(c) => c.outcome == o | Open => false }

  /// The path from `n` up to ROOT, `n` first, by the parents (at most WALK_MAX + 1 nodes).
  pure def up_path(st: State, n: int): List[int] =
    range(0, WALK_MAX + 1).foldl({ cur: n, out: [] }, (acc, i) =>
      if (acc.cur < 0 or not(st.nodes.keys().contains(acc.cur))) acc
      else { cur: st.nodes.get(acc.cur).parent, out: acc.out.append(acc.cur) }).out

  /// The depth of `n` (ROOT is at 0): the length of its path to ROOT, less one.
  pure def depth_naive(st: State, n: int): int = up_path(st, n).length() - 1

  pure def is_conv_key(k: Key): bool = match k { | Conv(_) => true | Msg(_) => true | _ => false }

  pure def conv_keys_of(st: State, n: int): Set[Key] = st.nodes.get(n).keys.filter(k => is_conv_key(k))

  /// The default conversation key of §9.1 at `at`: the only conversation key of the nearest node
  /// from `at` up that carries any; `NoKey` when there is none, or more than one.
  pure def default_conv_naive(st: State, at: int): OptKey = {
    val carriers = up_path(st, at).select(m => conv_keys_of(st, m) != Set())
    if (carriers.length() == 0) NoKey
    else {
      val ks = conv_keys_of(st, carriers[0])
      if (ks.size() == 1) SomeKey(ks.fold(Conv(""), (acc, k) => k)) else NoKey
    }
  }

  /// `conv_node(at, k)` of §9.1: the nearest node from `at` up that carries `k`, open or closed;
  /// else the open node that carries it; else `at`.
  pure def conv_node_naive(st: State, at: int, k: Key): int = {
    val near = up_path(st, at).select(m => st.nodes.get(m).keys.contains(k))
    if (near.length() > 0) near[0]
    else {
      val carriers = st.nodes.keys().filter(m => node_open(st, m) and st.nodes.get(m).keys.contains(k))
      if (carriers != Set()) set_min(carriers, -1) else at
    }
  }

  /// A wait is resolved (§10.3): its people question is no longer open or was rechecked since the
  /// wait was set; its user question is no longer pending; its node is closed; its time has come.
  pure def resolved_naive(st: State, w: Wait, now: int): bool =
    match w.on {
      | WQuestion(x) =>
          not(st.pquestions.keys().contains(x.q))
          or st.pquestions.get(x.q).state != PqOpen
          or st.pquestions.get(x.q).rechecks > x.rechecks_at_set
      | WUserQuestion(q) => not(st.uquestions.keys().contains(q)) or st.uquestions.get(q).state != UqPending
      | WNode(t) => node_closed(st, t)
      | WTime(t) => t <= now
      | WExternal(t) => t <= now
    }

  pure def waiting_naive(st: State, n: int, now: int): bool =
    match st.nodes.get(n).wait { | NoWait => false | HasWait(w) => not(resolved_naive(st, w, now)) }

  /// A pause over `n` or one of its ancestors is active at `now` (D.8.21).
  pure def paused_naive(st: State, n: int, now: int): bool =
    up_path(st, n).foldl(false, (acc, m) => acc or (st.pauses.keys().contains(m) and now < st.pauses.get(m).until))

  /// A hold of the map on `n` is live at `now`.
  pure def live_hold_on(st: State, n: int, now: int): bool =
    st.holds.keys().exists(t => t._1 == n and now < st.holds.get(t).lease_until)

  /// The edges of T5 out of `x`: its `{node}` wait, resolved or not, and its `depends_on`.
  pure def edges_naive(st: State, x: int): Set[int] =
    match st.nodes.get(x).wait {
      | NoWait => st.nodes.get(x).depends_on
      | HasWait(w) => match w.on { | WNode(t) => st.nodes.get(x).depends_on.union(Set(t)) | _ => st.nodes.get(x).depends_on }
    }.filter(y => st.nodes.keys().contains(y))

  /// The nodes reachable from `x` by one edge or more (every model forum has fewer than 24 nodes).
  pure def reach_naive(st: State, x: int): Set[int] =
    range(0, 24).foldl(edges_naive(st, x), (r, i) => r.union(r.map(z => edges_naive(st, z)).flatten()))

  pure def reply_kind(k: SaidKind): bool = k == SReply or k == SResult or k == SCorrection

  pure def user_origin(k: Key): bool = match k { | User(_) => true | _ => false }

  /// The queued deadline of `owner` of kind `kind`, if any lies after `now`.
  pure def queued(q: Set[DeadlineKey], kind: int, owner: int, now: int): bool =
    q.exists(k => k.kind == kind and k.owner == owner and k.at > now)

  /// Some open node carries the assume mark of question `q` (§9.3, §8.4).
  pure def assume_marked(st: State, q: int, reserved_ok: bool): bool =
    st.nodes.keys().exists(m => node_open(st, m) and st.nodes.get(m).assume_marks.contains(q)
                                and (reserved_ok or st.nodes.get(m).origin != OReserved))

  /// The number of open agent-made children of `p` (D.8.4, L1), counted over `nodes`.
  pure def agent_children(st: State, p: int): int =
    st.nodes.keys().filter(c => st.nodes.get(c).parent == p and node_open(st, c)
                                and match st.nodes.get(c).origin { | OAgent(_) => true | OAncestor(_) => true | _ => false }).size()

  /// `archived(n, now)` (§26 AR): closed, and `archive.after` has passed since. A function; no state
  /// field stores it, and no model action reads it.
  pure def archived(st: State, n: int, now: int): bool =
    st.nodes.keys().contains(n) and match st.nodes.get(n).life {
      | Closed(c) => now >= c.at + ARCHIVE_AFTER
      | Open => false
    }

  // ================================================================== slots and tokens

  /// H1: the live holds on a node are within its capacity.
  pure def inv_H1(st: State, now: int): bool =
    st.nodes.keys().forall(n =>
      st.holds.keys().filter(t => t._1 == n and now < st.holds.get(t).lease_until).size() <= st.nodes.get(n).capacity)

  /// H2, state half: a session's live holds are within `hold.per_session`; every hold's taker
  /// exists, and retires after the hold's ceiling (CS-8: a retired session holds nothing).
  pure def inv_H2(st: State, now: int): bool = and {
    st.sessions.keys().forall(x =>
      st.holds.keys().filter(t => st.holds.get(t).session == x and now < st.holds.get(t).lease_until).size()
        <= HOLD_PER_SESSION),
    st.holds.keys().forall(t => {
      val h = st.holds.get(t)
      st.sessions.keys().contains(h.session) and st.sessions.get(h.session).retire_at > h.taken_at + h.terms.ceiling
    }),
    // 1.4 (R27): a silent session holds no live hold.
    st.holds.keys().forall(t => {
      val h = st.holds.get(t)
      now < h.lease_until and st.sessions.keys().contains(h.session) implies not(st.sessions.get(h.session).silent)
    }),
  }

  /// H2, step half: a session retires only holding nothing; a hold is issued only to a session
  /// that the step touched (`Touched`, which sets `last_seen` to now) or opened.
  pure def step_H2(c: Step): bool =
    if (c.cmd) new_ids(c.pre.holds.keys(), c.post.holds.keys()).forall(t => {
      val x = c.post.holds.get(t).session
      x == c.la.sess and c.post.sessions.keys().contains(x) and c.post.sessions.get(x).last_seen == c.now
    })
    else all_firings(c, f =>
      f.pre.sessions.keys().exclude(f.post.sessions.keys()).forall(x =>
        not(f.pre.holds.keys().exists(t => f.pre.holds.get(t).session == x))))

  /// H3, state half: every hold's node exists, and the token's fence is at most the node's.
  pure def inv_H3(st: State): bool =
    st.holds.keys().forall(t => st.nodes.keys().contains(t._1) and t._2 <= st.nodes.get(t._1).fence)

  /// H3, step half: a fence only increases, by one for each hold issued; a new token carries the
  /// node's fence before the step plus one, so no token repeats. No firing issues a hold.
  pure def step_H3(c: Step): bool = {
    def fences_ok(pre: State, post: State): bool = pre.nodes.keys().forall(n =>
      post.nodes.keys().contains(n) and {
        val a = pre.nodes.get(n).fence
        val b = post.nodes.get(n).fence
        a <= b and b <= a + 1
      })
    if (c.cmd) and {
      fences_ok(c.pre, c.post),
      new_ids(c.pre.holds.keys(), c.post.holds.keys()).forall(t => {
        val f0 = if (c.pre.nodes.keys().contains(t._1)) c.pre.nodes.get(t._1).fence else 0
        t._2 == f0 + 1 and c.post.nodes.get(t._1).fence == t._2
      }),
      // a fence that rose has issued its token
      c.post.nodes.keys().forall(n => {
        val f0 = if (c.pre.nodes.keys().contains(n)) c.pre.nodes.get(n).fence else 0
        c.post.nodes.get(n).fence > f0 implies c.post.holds.keys().contains((n, c.post.nodes.get(n).fence))
      }),
    }
    else all_firings(c, f => and {
      f.post.holds.keys().subseteq(f.pre.holds.keys()),
      f.pre.nodes.keys().forall(n => f.post.nodes.keys().contains(n) and f.post.nodes.get(n).fence == f.pre.nodes.get(n).fence),
    })
  }

  /// The token of a write (D.9.2 H4: checkpoint, renew, release, close with a token; the model's
  /// update carries none), and the node it targets.
  pure def write_token(cmd: Cmd): OptToken =
    match cmd {
      | CCheckpoint(a) => SomeToken(a.token)
      | CRenew(t) => SomeToken(t)
      | CRelease(a) => SomeToken(a.token)
      | CClose(a) => a.token
      | _ => NoToken
    }

  pure def write_target(cmd: Cmd): int =
    match cmd {
      | CCheckpoint(a) => a.token._1
      | CRenew(t) => t._1
      | CRelease(a) => a.token._1
      | CClose(a) => a.node
      | _ => -1
    }

  /// H4 (step): a write with a token that is not a live hold on its target saves exactly one `late`
  /// entry and changes nothing else (D.9.2): one more eid and one more seq on the target, the time,
  /// the expiry prune of `op_id` records and notes, and the `op_id` record. The answers before it
  /// save nothing: a long text (L1), a live `op_id` record (O1), an unknown session, an unknown
  /// target.
  pure def step_H4(c: Step): bool =
    not(c.cmd) or match write_token(c.la.cmd) {
      | NoToken => true
      | SomeToken(t) => {
          val n = write_target(c.la.cmd)
          val pre = c.pre
          val live_op = c.la.op != "" and pre.ops.keys().contains((c.la.class, c.la.op))
                        and pre.ops.get((c.la.class, c.la.op)).expires_at > c.now
          val unchanged = c.post == pre
          if (c.la.long) unchanged
          else if (live_op) unchanged
          else if (not(pre.sessions.keys().contains(c.la.sess))) unchanged and c.la.outcome == OutErr(ErrNotFound(WhatSession))
          else if (not(pre.nodes.keys().contains(n))) unchanged and c.la.outcome == OutErr(ErrNotFound(WhatNode))
          else if (t._1 == n and pre.holds.keys().contains(t) and c.now < pre.holds.get(t).lease_until) true
          else {
            val ops_kept = pre.ops.keys().filter(k => pre.ops.get(k).expires_at > c.now).mapBy(k => pre.ops.get(k))
            val expected = {
              ...pre,
              last_now: imax(pre.last_now, c.now),
              next_eid: pre.next_eid + 1,
              nodes: pre.nodes.set(n, { ...pre.nodes.get(n), seq: pre.nodes.get(n).seq + 1 }),
              ops: if (c.la.op == "") ops_kept
                   else ops_kept.put((c.la.class, c.la.op), { digest: c.la.cmd, session: c.la.sess, expires_at: c.now + OP_RETAIN }),
              notes: pre.notes.keys().filter(x => pre.notes.get(x).until > c.now).mapBy(x => pre.notes.get(x)),
              aquestions: pre.aquestions.keys()
                .filter(q => not(pre.notes.keys().contains(q)) or pre.notes.get(q).until > c.now)
                .mapBy(q => pre.aquestions.get(q)),
            }
            c.la.outcome == OutErr(ErrStaleToken) and c.post == expected
          }
        }
    }

  /// H5: every hold, with the terms it was issued with (the scaled parameters, A-102):
  /// `taken_at < lease_until <= min(taken_at + ceiling, last_extended + max)`; before its first
  /// checkpoint, `lease_until <= taken_at + first`; `renews <= renew_max`.
  pure def inv_H5(st: State): bool =
    st.holds.keys().forall(t => {
      val h = st.holds.get(t)
      and {
        h.terms == LEASE_TERMS,
        h.taken_at < h.lease_until,
        h.lease_until <= h.taken_at + h.terms.ceiling,
        h.lease_until <= h.last_extended + h.terms.max,
        not(h.checkpointed) implies h.lease_until <= h.taken_at + h.terms.first,
        h.renews <= h.terms.renew_max,
        h.l >= 1 and h.l <= h.terms.max,
      }
    })

  /// H6: no hold is on a closed node.
  pure def inv_H6(st: State): bool = st.holds.keys().forall(t => node_open(st, t._1))

  // ================================================================== history and retries

  /// O1, state half: every record expires after `last_now`, at most `op.retain` after it was made.
  pure def inv_O1(st: State): bool =
    st.ops.keys().forall(k => {
      val r = st.ops.get(k)
      r.expires_at > st.last_now and r.expires_at <= st.last_now + OP_RETAIN
    })

  pure def is_hello(cmd: Cmd): bool = match cmd { | CHello(_) => true | _ => false }

  /// O1, step half (CS-8): a call with a live record of its `(class, op)` changes nothing, and is a
  /// replay exactly when it is the same command from the record's session (or a `hello`), a
  /// conflict otherwise; a call without one that changed the state records `{command, session,
  /// now + op.retain}` and keeps the other live records; a time action only drops records, each at
  /// its expiry. A long text comes first (L1).
  pure def step_O1(c: Step): bool =
    if (c.cmd) {
      val pre = c.pre
      val key = (c.la.class, c.la.op)
      val live = c.la.op != "" and pre.ops.keys().contains(key) and pre.ops.get(key).expires_at > c.now
      if (c.la.long) true
      else if (live) {
        val r = pre.ops.get(key)
        val same = r.digest == c.la.cmd and (is_hello(c.la.cmd) or r.session == c.la.sess)
        c.post == pre and c.la.outcome == (if (same) OutReplay else OutErr(ErrOpIdConflict))
      }
      else if (c.post == pre) true
      else and {
        pre.ops.keys().forall(k => (k != key and pre.ops.get(k).expires_at > c.now) implies
          (c.post.ops.keys().contains(k) and c.post.ops.get(k) == pre.ops.get(k))),
        c.post.ops.keys().forall(k => k == key or pre.ops.keys().contains(k)),
        c.la.op != "" implies (c.post.ops.keys().contains(key) and c.post.ops.get(key) == {
          digest: c.la.cmd,
          session: if (is_hello(c.la.cmd)) pre.next_session else c.la.sess,
          expires_at: c.now + OP_RETAIN,
        }),
        c.la.op == "" implies c.post.ops.keys().subseteq(pre.ops.keys()),
      }
    }
    else all_firings(c, f =>
      f.post.ops == f.pre.ops.keys().filter(k => f.pre.ops.get(k).expires_at > f.key.at).mapBy(k => f.pre.ops.get(k)))

  /// O2, state half: per node, every kept entry's seq is at most the node's and no two are equal,
  /// `version >= 1`, `digest_version >= 1`, and `content_version` is a version the node has had.
  pure def inv_O2(st: State): bool =
    st.nodes.keys().forall(n => {
      val nd = st.nodes.get(n)
      val es = st.entries.keys().filter(e => st.entries.get(e).node == n)
      and {
        es.forall(e => st.entries.get(e).seq >= 1 and st.entries.get(e).seq <= nd.seq),
        es.map(e => st.entries.get(e).seq).size() == es.size(),
        nd.version >= 1,
        nd.digest_version >= 1,
        nd.content_version >= 1 and nd.content_version <= nd.version,
      }
    })

  /// The fields whose change moves `version` (§5, O2): title, body, fields, and files (`content`),
  /// status, keys, outgoing links, and open or closed.
  pure def versioned(nd: Node): (int, int, Set[Key], Set[(LinkRel, int)], bool) =
    (nd.content, nd.status, nd.keys, nd.links, nd.life == Open)

  /// The most events that move `version` a transition from `a` to `b` can hold (O2: "once per
  /// event"): a `KeyAdded` or `KeyRemoved` per key that came or went, a `LinkAdded` per link, one
  /// `NodeUpdated` for the content and the status, and one `NodeClosed` or `NodeReopened`.
  pure def version_events(a: Node, b: Node): int =
    a.keys.exclude(b.keys).size() + b.keys.exclude(a.keys).size()
    + a.links.exclude(b.links).size() + b.links.exclude(a.links).size()
    + (if (a.content != b.content or a.status != b.status) 1 else 0)
    + (if ((a.life == Open) != (b.life == Open)) 1 else 0)

  /// O2 for one node across one command or one firing.
  pure def o2_node(a: Node, b: Node, new_seqs: Set[int]): bool = and {
    b.version >= a.version,
    b.digest_version >= a.digest_version,
    b.seq >= a.seq,
    b.fence >= a.fence,
    b.content_version >= a.content_version,
    // version moves exactly when a versioned field changed, by one per event at most
    (b.version != a.version) iff (versioned(b) != versioned(a)),
    b.version <= a.version + version_events(a, b),
    // digest_version by one exactly when the digest changed (a digest change moves nothing else)
    b.digest_version == (if (b.digest != a.digest) a.digest_version + 1 else a.digest_version),
    // content_version is the version of the last content change
    b.content_version == (if (b.content != a.content) b.version else a.content_version),
    // a kept entry appended on the node takes a seq the step added
    new_seqs.forall(q => q > a.seq and q <= b.seq),
  }

  pure def o2_states(pre: State, post: State): bool =
    pre.nodes.keys().forall(n =>
      post.nodes.keys().contains(n) and o2_node(pre.nodes.get(n), post.nodes.get(n),
        new_ids(pre.entries.keys(), post.entries.keys()).filter(e => post.entries.get(e).node == n)
          .map(e => post.entries.get(e).seq)))
    and new_ids(pre.nodes.keys(), post.nodes.keys()).forall(n => {
      val nd = post.nodes.get(n)
      nd.version == 1 and nd.digest_version == 1 and nd.content_version == 1
    })

  /// O2, step half: the counters of every node change as D.9.2 says, per command and per firing; a
  /// `stale_version` edit and an edit with a digest and no `digest_version` change nothing (CS-11);
  /// a digest changes only by an `update` of that node that carries its `digest_version` as it was
  /// before the step (D.9.2).
  pure def step_O2(c: Step): bool =
    if (c.cmd) and {
      o2_states(c.pre, c.post),
      (c.la.outcome == OutErr(ErrStaleVersion) or c.la.outcome == OutErr(ErrInvalid(WhyMissingDigestVersion)))
        implies c.post == c.pre,
      c.pre.nodes.keys().forall(n => c.post.nodes.get(n).digest != c.pre.nodes.get(n).digest implies match c.la.cmd {
        | CUpdate(u) => u.node == n and u.dver == c.pre.nodes.get(n).digest_version
        | _ => false
      }),
    }
    else all_firings(c, f => o2_states(f.pre, f.post))

  /// O3 (step): every entry of `prev` is in `s` unchanged (nothing reaches FINISHED_RETAIN within
  /// MAX_TIME, D.10.5), but the entry of a note, or of an agent question, whose `until` has come,
  /// which leaves with its note (`at_event`, as the core's `may_leave`; 1.12's `finish_two`, the
  /// first conformance group generated with the slice's invariants checked, found a note's entry
  /// leaving at a tick); every new entry has an eid the step allocated, and what is retracted stays
  /// retracted.
  pure def step_O3(c: Step): bool = and {
    c.post.next_eid >= c.pre.next_eid,
    c.pre.entries.keys().forall(e =>
      if (c.post.entries.keys().contains(e)) c.post.entries.get(e) == c.pre.entries.get(e)
      else c.pre.notes.keys().contains(e) and c.pre.notes.get(e).until <= c.post.last_now),
    new_ids(c.pre.entries.keys(), c.post.entries.keys()).forall(e => e >= c.pre.next_eid and e < c.post.next_eid),
    c.pre.retracted.subseteq(c.post.retracted),
  }

  // ================================================================== keys and names

  /// K1: a key is carried by at most one open node.
  pure def inv_K1(st: State): bool =
    st.nodes.keys().forall(n => node_open(st, n) implies st.nodes.get(n).keys.forall(k =>
      st.nodes.keys().filter(m => node_open(st, m) and st.nodes.get(m).keys.contains(k)).size() == 1))

  /// K3: live names and aliases are disjoint; every node but ROOT has its place as a live name, and
  /// every live name points at a node with that place; every alias points at a node.
  pure def inv_K3(st: State): bool = and {
    st.names.keys().intersect(st.aliases.keys()) == Set(),
    st.nodes.keys().forall(n => n == ROOT or {
      val nd = st.nodes.get(n)
      st.names.keys().contains((nd.parent, nd.name)) and st.names.get((nd.parent, nd.name)) == n
    }),
    st.names.keys().forall(p => {
      val n = st.names.get(p)
      st.nodes.keys().contains(n) and n != ROOT and (st.nodes.get(n).parent, st.nodes.get(n).name) == p
    }),
    st.aliases.keys().forall(p => st.nodes.keys().contains(st.aliases.get(p))),
  }

  /// K4: from every node, the parent walk reaches ROOT.
  pure def inv_K4(st: State): bool =
    st.nodes.keys().forall(n => {
      val p = up_path(st, n)
      p.length() >= 1 and p[p.length() - 1] == ROOT and st.nodes.get(ROOT).parent == -1
    })

  /// K5: `merged_into` only on closed `superseded` nodes, naming a node; following it from any node
  /// ends at a node that was not merged.
  pure def inv_K5(st: State): bool = {
    def into(n: int): int = match st.nodes.get(n).life { | Closed(c) => c.merged_into | Open => -1 }
    and {
      st.nodes.keys().forall(n => match st.nodes.get(n).life {
        | Closed(c) => c.merged_into >= 0 implies (c.outcome == Superseded and st.nodes.keys().contains(c.merged_into))
        | Open => true
      }),
      st.nodes.keys().forall(n => {
        val end = range(0, 12).foldl(n, (x, i) => if (into(x) >= 0) into(x) else x)
        into(end) < 0
      }),
    }
  }

  /// K6: an open series has exactly one open occurrence; a closed series, and a node that is no
  /// series, none (a detached occurrence has role `Plain`, A-13).
  pure def inv_K6(st: State): bool =
    st.nodes.keys().forall(x => {
      val occ = st.nodes.keys().filter(o => node_open(st, o) and match st.nodes.get(o).role {
        | Occurrence(i) => i.series == x
        | _ => false
      })
      val series = match st.nodes.get(x).role { | Series(_) => true | _ => false }
      occ.size() == (if (series and node_open(st, x)) 1 else 0)
    })

  /// K7 (step): a node the step reopened (by `reopen`, `take`, or `next`) keeps exactly the keys
  /// that no other node carried openly before the step.
  pure def step_K7(c: Step): bool =
    not(c.cmd) or c.pre.nodes.keys().forall(n =>
      (node_closed(c.pre, n) and node_open(c.post, n)) implies
        c.post.nodes.get(n).keys == c.pre.nodes.get(n).keys.filter(k =>
          not(c.pre.nodes.keys().exists(m => m != n and node_open(c.pre, m) and c.pre.nodes.get(m).keys.contains(k)))))

  /// The place Genesis gives each reserved node (D.7.4).
  pure val RESERVED_PLACES: int -> (int, Name) = Map(
    ROOT -> (-1, Lit("")),
    USER -> (ROOT, Lit("@user")),
    USER_DECISIONS -> (USER, Lit("decisions")),
    USER_DIGEST -> (USER, Lit("digest")),
    SYS -> (ROOT, Lit("sys")),
    SIGNALS -> (SYS, Lit("signals")),
    PEOPLE -> (ROOT, Lit("@people")),
    TEAMS -> (ROOT, Lit("@teams"))
  )

  /// K8: each reserved node exists with origin `Reserved`, open, at its Genesis place, with capacity
  /// 0, role `Plain`, and no key; no other node has origin `Reserved`.
  pure def inv_K8(st: State): bool = and {
    RESERVED_PLACES.keys().forall(n => and {
      st.nodes.keys().contains(n),
      st.nodes.get(n).origin == OReserved,
      st.nodes.get(n).life == Open,
      (st.nodes.get(n).parent, st.nodes.get(n).name) == RESERVED_PLACES.get(n),
      st.nodes.get(n).capacity == 0,
      st.nodes.get(n).role == Plain,
      st.nodes.get(n).keys == Set(),
    }),
    st.nodes.keys().forall(n => st.nodes.get(n).origin == OReserved implies RESERVED_PLACES.keys().contains(n)),
  }

  pure def cmd_at(cmd: Cmd): int =
    match cmd {
      | CRequest(a) => a.at
      | CSaid(a) => a.at
      | CHeard(a) => a.at
      | CAskPerson(a) => a.at
      | _ => -1
    }

  /// C1 (step): every request from a conversation (an origin that is no `user:` entry), people
  /// question, `said`, `heard`, and commitment a step records lies on the conversation node of its
  /// key, which is not reserved, with that key: for a request, its `where` or else its origin; for a
  /// `said`, its `where` or else the default of §9.1; for `heard` and `ask`, the default (A-61, A-12,
  /// A-29). Only `request`, `said`, `heard`, and `ask` to a person record them, and no firing does.
  /// 1.3 (§9.2): a commitment is fulfilled only by a `said` of kind reply, result, or correction
  /// that the step records on its node in its conversation, and such a `said` leaves no commitment
  /// of its conversation on its node open; 1.9: unless it is withheld, which fulfils nothing.
  pure def step_C1(c: Step): bool = {
    val pre = c.pre
    val post = c.post
    val reqs = new_ids(pre.requests.keys(), post.requests.keys()).filter(r => not(user_origin(post.requests.get(r).origin)))
    val pqs = new_ids(pre.pquestions.keys(), post.pquestions.keys())
    val saids = new_ids(pre.saids.keys(), post.saids.keys())
    val heards = new_ids(pre.entries.keys(), post.entries.keys()).filter(e => post.entries.get(e).ty == EHeard)
    val cms = new_ids(pre.commitments.keys(), post.commitments.keys())
    val at = cmd_at(c.la.cmd)
    val talk = c.cmd and outcome_ok(c.la.outcome) and at >= 0
    val dflt = if (talk) default_conv_naive(pre, at) else NoKey
    val key = match c.la.cmd {
      | CRequest(a) => match a.where_key { | SomeKey(x) => SomeKey(x) | NoKey => SomeKey(a.origin) }
      | CSaid(a) => match a.where_key { | SomeKey(x) => SomeKey(x) | NoKey => dflt }
      | _ => dflt
    }
    val home = match key { | SomeKey(k) => if (talk) conv_node_naive(pre, at, k) else -1 | NoKey => -1 }
    val k0 = match key { | SomeKey(k) => k | NoKey => Conv("") }
    def on_home(n: int, k: Key): bool = n == home and k == k0
    val recorded = reqs.union(pqs).union(saids).union(heards).union(cms) != Set()
    val fulfilled = pre.commitments.keys().filter(x =>
      pre.commitments.get(x).state == CmOpen and post.commitments.get(x).state != CmOpen)
    and {
      recorded implies and {
        talk,
        home >= 0,
        pre.nodes.get(home).origin != OReserved,
        reqs.forall(r => on_home(post.requests.get(r).node, post.requests.get(r).conv)),
        pqs.forall(q => on_home(post.pquestions.get(q).node, post.pquestions.get(q).conv)),
        saids.forall(x => on_home(post.saids.get(x).node, post.saids.get(x).conv)),
        heards.forall(e => post.entries.get(e).node == home),
        cms.forall(x => on_home(post.commitments.get(x).node, post.commitments.get(x).conv)),
      },
      fulfilled.forall(x => match post.commitments.get(x).state {
        | CmFulfilled(a) => and {
            saids.contains(a),
            reply_kind(post.saids.get(a).kind),
            post.saids.get(a).state != SdWithheld,
            post.saids.get(a).node == pre.commitments.get(x).node,
            post.saids.get(a).conv == pre.commitments.get(x).conv,
          }
        | CmOpen => false
      }),
      saids.forall(a => (reply_kind(post.saids.get(a).kind) and post.saids.get(a).state != SdWithheld) implies
        pre.commitments.keys().forall(x => {
          val cm = pre.commitments.get(x)
          (cm.state == CmOpen and cm.node == post.saids.get(a).node and cm.conv == post.saids.get(a).conv)
            implies post.commitments.get(x).state != CmOpen
        })),
    }
  }

  /// Q1: request origins, the messages they came in (A-61), are pairwise distinct.
  pure def inv_Q1(st: State): bool =
    st.requests.keys().map(r => st.requests.get(r).origin).size() == st.requests.keys().size()

  /// S1: at most one open signal node per (kind, pair); `signals` maps each key to the open node
  /// with that signal role, and to nothing else (after a reopen too, A-89).
  pure def inv_S1(st: State): bool = {
    val open_signals = st.nodes.keys().filter(n => node_open(st, n) and match st.nodes.get(n).role { | RoleSignal(_) => true | _ => false })
    def key_of(n: int): (SignalKind, int, int) = match st.nodes.get(n).role {
      | RoleSignal(k) => (k.kind, k.a, k.b)
      | _ => (SigDuplicate, -1, -1)
    }
    and {
      open_signals.map(n => key_of(n)).size() == open_signals.size(),
      open_signals.forall(n => st.signals.keys().contains(key_of(n)) and st.signals.get(key_of(n)) == n),
      st.signals.keys().forall(k => open_signals.contains(st.signals.get(k)) and key_of(st.signals.get(k)) == k),
      st.signals.keys().forall(k => k._2 < k._3),
    }
  }

  // ================================================================== nothing is lost

  /// N1: every user entry has one routing state, consistent with what it points at, and its entry
  /// on USER; `unrouted ∈ flags(@user)` exactly when some entry is unrouted.
  pure def inv_N1(st: State, now: int): bool = and {
    flags_of(st, USER, now).contains(FUnrouted) iff st.user_entries.keys().exists(e => st.user_entries.get(e).routing == Unrouted),
    st.user_entries.keys().forall(e => {
      val u = st.user_entries.get(e)
      and {
        st.entries.keys().contains(e),
        st.entries.get(e).ty == EFromUser,
        st.entries.get(e).node == USER,
        st.entries.get(e).principal == u.principal,
        match u.routing {
          | Unrouted => true
          | RtRequest(x) => st.requests.keys().contains(x.request) and st.requests.get(x.request).from_user == e
          | RtClarifies(r) => st.requests.keys().contains(r)
          | RtAnswers(q) => st.uquestions.keys().contains(q) and st.uquestions.get(q).state == UqAnswered(e)
          | RtNoAction => true
        },
      }
    }),
  }

  /// The answer `a` of request `r` is one that may answer it (§26 Settled): newer than `r`; for a
  /// request from people or agents, a `said` of kind reply, result, or correction in its own
  /// conversation (1.3); for a user request, a `to_user` with a principal (A-23, CS-4).
  pure def valid_answer(st: State, r: int, a: int): bool = {
    val req = st.requests.get(r)
    a > r and match req.requester {
      | ReqExternal =>
          st.saids.keys().contains(a) and reply_kind(st.saids.get(a).kind) and st.saids.get(a).conv == req.conv
      | ReqUser(_) =>
          st.entries.keys().contains(a) and st.entries.get(a).ty == EToUser and st.entries.get(a).principal >= 0
    }
  }

  /// Settled, state half: a settled request names a decision that lists it and its answer, a valid
  /// one; an unsettled request is not both answered and listed by a decision that is not retracted
  /// (a superseded one counts, §9.4); an answer, once there, is valid.
  pure def inv_Settled(st: State): bool =
    st.requests.keys().forall(r => {
      val req = st.requests.get(r)
      and {
        req.answered_by >= 0 implies valid_answer(st, r, req.answered_by),
        match req.settled {
          | SettledBy(x) => and {
              st.decisions.keys().contains(x.decision),
              st.decisions.get(x.decision).settles.contains(r),
              x.answer == req.answered_by,
              x.answer >= 0,
            }
          | Unsettled => not(req.answered_by >= 0 and st.decisions.keys().exists(d =>
              st.decisions.get(d).settles.contains(r) and match st.decisions.get(d).state { | DRetracted(_) => false | _ => true }))
        },
      }
    })

  /// Settled, step half: settling and answering are for good (A-11); a request answered in the step
  /// by a `to_user` is answered by a new `to_user` of kind `result` to the same person as its
  /// requester (A-23, CS-4); one answered in the step by a `said` is answered by a command, with a
  /// `said` of kind reply, result, or correction in the request's own conversation (§9.4, 1.3),
  /// recorded by the step, or one a merge credits to a request it moves (A-95).
  pure def step_Settled(c: Step): bool = and {
    c.pre.requests.keys().forall(r => and {
      c.post.requests.keys().contains(r),
      c.pre.requests.get(r).settled != Unsettled implies c.post.requests.get(r).settled == c.pre.requests.get(r).settled,
      c.pre.requests.get(r).answered_by >= 0 implies c.post.requests.get(r).answered_by == c.pre.requests.get(r).answered_by,
    }),
    c.post.requests.keys().forall(r => {
      val a = c.post.requests.get(r).answered_by
      val was = if (c.pre.requests.keys().contains(r)) c.pre.requests.get(r).answered_by else -1
      (a >= 0 and was < 0 and c.post.saids.keys().contains(a)) implies and {
        c.cmd,
        reply_kind(c.post.saids.get(a).kind),
        c.post.saids.get(a).state != SdWithheld,
        c.post.saids.get(a).conv == c.post.requests.get(r).conv,
        a >= c.pre.next_eid or match c.la.cmd { | CMerge(_) => true | _ => false },
      }
    }),
    c.post.requests.keys().forall(r => {
      val a = c.post.requests.get(r).answered_by
      val was = if (c.pre.requests.keys().contains(r)) c.pre.requests.get(r).answered_by else -1
      (a >= 0 and was < 0 and c.post.entries.keys().contains(a) and c.post.entries.get(a).ty == EToUser) implies and {
        a >= c.pre.next_eid,
        c.cmd,
        match c.la.cmd { | CToUser(x) => x.kind == TuResult | _ => false },
        match c.post.requests.get(r).requester {
          | ReqUser(p) => same_person(c.pre, c.post.entries.get(a).principal, p)
          | ReqExternal => false
        },
      }
    }),
  }

  /// The flags that hold on open nodes only, and what never holds on reserved and merged nodes (§26 F).
  pure val OPEN_ONLY: Set[Flag] = Set(FTooBig, FOverdue, FReadyToClose, FMissed, FStale)

  /// F (F2 of D.9.2): no flag changes by time alone before the next queued deadline, and none when
  /// nothing is queued; `too_big`, `overdue`, `ready_to_close`, `missed`, and `stale` (1.8) only on
  /// open nodes, `ready_to_close` never on a reserved node nor on one never held (1.6), `stale` never
  /// on a container or a held node (1.8), `basis_changed` never on a merged one. The model stores no
  /// flags, so F1 has nothing to compare (D.10.3).
  pure def inv_F(st: State, now: int): bool = {
    val t = first_key(st).at
    val later = if (t < 0) now + 10 * MAX_TIME else t - 1
    st.nodes.keys().forall(n => {
      val fl = flags_of(st, n, now)
      and {
        fl == flags_of(st, n, later),
        node_closed(st, n) implies fl.intersect(OPEN_ONLY) == Set(),
        st.nodes.get(n).origin == OReserved implies not(fl.contains(FReadyToClose)),
        st.nodes.get(n).fence == 0 implies not(fl.contains(FReadyToClose)),
        is_container(st, n) or held_self(st, n, now) implies not(fl.contains(FStale)),
        merged(st, n) implies not(fl.contains(FBasisChanged)),
      }
    })
  }

  /// N5 (step): a merge moves the keys, and keeps with their ids, marked `moved_from`, the unsettled
  /// requests, the open, expired, and escalated people questions, the open commitments and
  /// objections, and the pending and unconfirmed `said` entries of `from`, its marks, and another
  /// session's open intent when `into` has none; it leaves no wait or dependency on `from` and no
  /// self-edge; a merge refused with `cycle` changes nothing.
  pure def step_N5(c: Step): bool =
    not(c.cmd) or match c.la.cmd {
      | CMerge(a) =>
          if (c.la.outcome == OutErr(ErrCycle)) c.post == c.pre
          else if (not(outcome_ok(c.la.outcome))) true
          else {
            val pre = c.pre
            val post = c.post
            val fm = a.from
            val i = a.into
            def moved(n: int, mf: int): bool = n == i and mf == fm
            and {
              post.nodes.get(i).keys == pre.nodes.get(i).keys.union(pre.nodes.get(fm).keys),
              post.nodes.get(fm).keys == Set(),
              pre.requests.keys().forall(r => (pre.requests.get(r).node == fm and pre.requests.get(r).settled == Unsettled)
                implies moved(post.requests.get(r).node, post.requests.get(r).moved_from)),
              pre.pquestions.keys().forall(q => (pre.pquestions.get(q).node == fm and pq_answerable(pre.pquestions.get(q).state))
                implies moved(post.pquestions.get(q).node, post.pquestions.get(q).moved_from)),
              pre.commitments.keys().forall(x => (pre.commitments.get(x).node == fm and pre.commitments.get(x).state == CmOpen)
                implies moved(post.commitments.get(x).node, post.commitments.get(x).moved_from)),
              pre.objections.keys().forall(o => (pre.objections.get(o).node == fm and pre.objections.get(o).state == ObOpen)
                implies (moved(post.objections.get(o).node, post.objections.get(o).moved_from) and post.objections.get(o).state == ObOpen)),
              pre.saids.keys().forall(x => (pre.saids.get(x).node == fm and (pre.saids.get(x).state == SdPending or pre.saids.get(x).state == SdUnconfirmed))
                implies (moved(post.saids.get(x).node, post.saids.get(x).moved_from) and post.saids.get(x).state == pre.saids.get(x).state)),
              post.nodes.get(fm).assume_marks == Set(),
              post.nodes.get(fm).recheck_marks == Set(),
              pre.nodes.get(fm).assume_marks.union(pre.nodes.get(i).assume_marks).subseteq(post.nodes.get(i).assume_marks),
              pre.nodes.get(fm).recheck_marks.union(pre.nodes.get(i).recheck_marks).subseteq(post.nodes.get(i).recheck_marks),
              (pre.nodes.get(fm).open_intent.eid >= 0 and pre.nodes.get(fm).open_intent.session != c.la.sess
               and pre.nodes.get(i).open_intent.eid < 0) implies post.nodes.get(i).open_intent == pre.nodes.get(fm).open_intent,
              post.nodes.keys().forall(n => not(edges_naive(post, n).contains(fm)) and not(edges_naive(post, n).contains(n))),
            }
          }
      | _ => true
    }

  /// E1: refusals and questions always reach a mover (§26 E1, D.8.14, CS-2, CS-7).
  /// (a) an open node that every active class has refused (or, with no active class, one has) has
  /// a pending refusal question; (b) refusal records and caps profiles are live; (c) an expired
  /// people question whose assumption is not recorded marks an open node, and closed nodes carry no
  /// assume mark; (d) an escalated question has its user question; (e) a pending user question has
  /// its deadline queued at its due; (f) a lapsed user question whose assumption is not recorded, and
  /// whose escalated question was not answered after all, marks an open node that is not reserved.
  pure def inv_E1(st: State, now: int): bool = {
    val q = queue(st)
    def refused_by(n: int, cls: str): bool = st.refusals.keys().contains((n, cls)) and {
      val r = st.refusals.get((n, cls))
      r.by.keys().filter(x => r.by.get(x).expires_at > now).size() >= r.quorum
    }
    def active_for(n: int): Set[str] = st.classes.keys().filter(cls =>
      st.classes.get(cls).caps_seen.keys().exists(caps => st.classes.get(cls).caps_seen.get(caps) > now and st.nodes.get(n).needs.subseteq(caps)))
    def refusing(n: int): Set[str] = st.refusals.keys().filter(k => k._1 == n and refused_by(n, k._2)).map(k => k._2)
    and {
      // (a)
      st.nodes.keys().forall(n => (node_open(st, n) and refusing(n) != Set() and active_for(n).subseteq(refusing(n)))
        implies st.uquestions.keys().exists(u => st.uquestions.get(u).state == UqPending and st.uquestions.get(u).source == SrcRefusal(n))),
      // (b)
      st.refusals.keys().forall(k => {
        val r = st.refusals.get(k)
        r.by.keys() != Set() and r.by.keys().forall(x => r.by.get(x).backoff_until <= r.by.get(x).expires_at and r.by.get(x).expires_at > now)
      }),
      st.classes.keys().forall(cls => {
        val ca = st.classes.get(cls)
        ca.caps_seen.keys() != Set() and ca.caps_seen.keys().forall(caps => ca.caps_seen.get(caps) > now)
      }),
      // (c)
      st.pquestions.keys().forall(x => {
        val pq = st.pquestions.get(x)
        (pq.state == PqExpired and pq.assumed_by < 0) implies assume_marked(st, x, true)
      }),
      st.nodes.keys().forall(n => node_closed(st, n) implies st.nodes.get(n).assume_marks == Set()),
      // (d)
      st.pquestions.keys().forall(x => match st.pquestions.get(x).state {
        | PqEscalated(u) => st.uquestions.keys().contains(u) and st.uquestions.get(u).source == SrcEscalation(x)
        | _ => true
      }),
      // (e)
      st.uquestions.keys().forall(u => st.uquestions.get(u).state == UqPending implies
        q.contains({ at: st.uquestions.get(u).due, kind: K_USER_QUESTION, owner: owner_entry(u) })),
      // (f)
      st.uquestions.keys().forall(u => {
        val uq = st.uquestions.get(u)
        val answered_after_all = match uq.source {
          | SrcEscalation(x) => st.pquestions.keys().contains(x) and match st.pquestions.get(x).state { | PqAnswered(_) => true | _ => false }
          | _ => false
        }
        (uq.state == UqLapsed and uq.assumed_by < 0 and not(answered_after_all)) implies assume_marked(st, u, false)
      }),
      // (g) 1.12 (§9.3): a withdrawn question queues no recheck, and no wait on it is unresolved.
      st.pquestions.keys().forall(x => match st.pquestions.get(x).state {
        | PqWithdrawn(_) => and {
            not(q.exists(k => k.kind == K_RECHECK and k.owner == owner_entry(x))),
            st.nodes.keys().forall(n => match st.nodes.get(n).wait {
              | HasWait(w) => match w.on {
                  | WQuestion(y) => y.q == x implies resolved_naive(st, w, now)
                  | _ => true
                }
              | NoWait => true
            }),
          }
        | _ => true
      }),
    }
  }

  /// W1: a waiting node carries neither `unanswered_request` nor `open_intent`, and `next` sees it
  /// only as a status item, exactly while it carries `open_commitment` (D.8.5's `classify`).
  pure def inv_W1(st: State, now: int): bool =
    st.nodes.keys().forall(n => waiting_naive(st, n, now) implies {
      val fl = flags_of(st, n, now)
      and {
        not(fl.contains(FUnansweredRequest)),
        not(fl.contains(FOpenIntent)),
        classify(st, n, fl, now) == (if (fl.contains(FOpenCommitment)) 1 else -1),
      }
    })

  /// R2, state half: no node closed `missed` has a checkpoint.
  pure def inv_R2(st: State): bool =
    st.nodes.keys().forall(n => closed_with(st, n, Missed) implies st.nodes.get(n).checkpoints == 0)

  /// The nodes a transition closed as `missed`.
  pure def newly_missed(pre: State, post: State): Set[int] =
    pre.nodes.keys().filter(n => node_open(pre, n) and closed_with(post, n, Missed))

  /// R2 for one transition: a `missed` close only of an occurrence with no checkpoint, no open
  /// intent, and no hold live at the close in the state before (D.9.2), other than the holds of
  /// other sessions that the transition's own move of the occurrence ended (`moved`, the session
  /// `mover`): D.8.20 runs the missed check after such a move, and A-80 has it run "after every
  /// hold end that leaves the occurrence open (lease expiry, release, move)". D.9.2's "no live hold
  /// in `before`" does not except them (README, "Open questions"; `r2MoveTest`).
  pure def r2_ok(pre: State, post: State, at: int, moved: int, mover: int): bool =
    newly_missed(pre, post).forall(n => {
      val nd = pre.nodes.get(n)
      and {
        match nd.role { | Occurrence(_) => true | _ => false },
        nd.checkpoints == 0,
        nd.open_intent.eid < 0,
        not(pre.holds.keys().exists(t => t._1 == n and at < pre.holds.get(t).lease_until
                                         and not(n == moved and pre.holds.get(t).session != mover))),
      }
    })

  /// R2, step half: only the server closes a node as `missed` (§12), as above.
  pure def step_R2(c: Step): bool =
    if (c.cmd) and {
      r2_ok(c.pre, c.post, c.now, match c.la.cmd { | CMove(a) => a.node | _ => -1 }, c.la.sess),
      match c.la.cmd { | CClose(a) => a.outcome == Missed implies not(outcome_ok(c.la.outcome)) | _ => true },
    }
    else all_firings(c, f => r2_ok(f.pre, f.post, f.key.at, -1, -1))

  /// D1, state half (CS-5): every digest entry is addressed to an interned principal, and one shown
  /// was shown by a `to_user`.
  pure def inv_D1(st: State): bool =
    st.digest.keys().forall(d => {
      val de = st.digest.get(d)
      and {
        st.principal_ids.keys().exists(k => st.principal_ids.get(k) == de.to),
        st.entries.keys().contains(d) and st.entries.get(d).node == USER_DIGEST,
        match de.state {
          | DgShown(by) => st.entries.keys().contains(by) and st.entries.get(by).ty == EToUser
          | DgPending => true
        },
      }
    })

  /// The digest items one firing must emit: an expired people question's assumption, a defaulted
  /// user question's default (or the drop of its refused node), and a lapse.
  pure def due_items(f: Firing): Set[DigestItem] = {
    val pre = f.pre
    val post = f.post
    val expired = pre.pquestions.keys().filter(x => pre.pquestions.get(x).state == PqOpen and post.pquestions.get(x).state == PqExpired)
    val lapsed = pre.uquestions.keys().filter(u => pre.uquestions.get(u).state == UqPending and post.uquestions.get(u).state == UqLapsed)
    val defaulted = pre.uquestions.keys().filter(u => pre.uquestions.get(u).state == UqPending and post.uquestions.get(u).state == UqDefaulted)
    expired.map(x => DiAssumption(x))
      .union(lapsed.map(u => DiLapsed(u)))
      .union(defaulted.map(u => match pre.uquestions.get(u).source {
        | SrcRefusal(n) =>
            if (node_open(pre, n) and closed_with(post, n, Dropped)) DiRefusalDrop({ node: n, question: u })
            else DiDefaultApplied(u)
        | _ => DiDefaultApplied(u)
      }))
  }

  /// D1 for one firing: for each item it must emit, one pending digest entry for each principal of
  /// the item's audience in the state before the firing, and no other new digest entry.
  pure def d1_firing(f: Firing): bool = {
    val fresh = new_ids(f.pre.digest.keys(), f.post.digest.keys())
    val items = due_items(f)
    and {
      fresh.forall(d => items.contains(f.post.digest.get(d).item) and f.post.digest.get(d).state == DgPending),
      items.forall(it => {
        val ds = fresh.filter(d => f.post.digest.get(d).item == it)
        val tos = ds.map(d => f.post.digest.get(d).to)
        tos.size() == ds.size() and tos == audience(f.pre, it)
      }),
    }
  }

  /// D1, step half: per firing as above; a command emits no digest entry; a digest entry is shown
  /// only by a `to_user` of the step to its principal or an alias of it.
  pure def step_D1(c: Step): bool = and {
    if (c.cmd) c.post.digest.keys() == c.pre.digest.keys() else all_firings(c, f => d1_firing(f)),
    c.pre.digest.keys().forall(d => (c.pre.digest.get(d).state == DgPending and c.post.digest.get(d).state != DgPending) implies
      match c.post.digest.get(d).state {
        | DgShown(by) => and {
            c.cmd,
            by >= c.pre.next_eid,
            c.post.entries.keys().contains(by),
            c.post.entries.get(by).ty == EToUser,
            same_person(c.pre, c.post.entries.get(by).principal, c.pre.digest.get(d).to),
          }
        | DgPending => true
      }),
    c.pre.digest.keys().forall(d => c.pre.digest.get(d).state != DgPending implies c.post.digest.get(d) == c.pre.digest.get(d)),
  }

  /// M1a (D.9.3): every waiting object has its server-guaranteed mover, a queued deadline or a flag
  /// (questions for the user and the digest are not movers, CS-6).
  pure def inv_M1(st: State, now: int): bool = {
    val q = queue(st)
    def fl(n: int): Set[Flag] = flags_of(st, n, now)
    def attended_n(n: int): bool = st.holds.keys().exists(t => now < st.holds.get(t).lease_until and has(up_path(st, t._1), n))
    def uq_mover(u: int): bool = st.uquestions.keys().contains(u) and {
      val uq = st.uquestions.get(u)
      if (uq.state == UqPending) queued(q, K_USER_QUESTION, owner_entry(u), now)
      else if (uq.state == UqLapsed) uq.assumed_by >= 0 or assume_marked(st, u, true)
      else true
    }
    and {
      // waits
      st.nodes.keys().forall(n => node_open(st, n) implies match st.nodes.get(n).wait {
        | NoWait => true
        | HasWait(w) => resolved_naive(st, w, now) or match w.on {
            | WTime(_) => queued(q, K_WAIT, owner_node(n), now)
            | WExternal(_) => queued(q, K_WAIT, owner_node(n), now)
            | WQuestion(x) => st.pquestions.get(x.q).state == PqOpen and queued(q, K_RECHECK, owner_entry(x.q), now)
            | WUserQuestion(u) => st.uquestions.get(u).state == UqPending and queued(q, K_USER_QUESTION, owner_entry(u), now)
            | WNode(t) => node_open(st, t)
          }
      }),
      // people questions
      st.pquestions.keys().forall(x => {
        val pq = st.pquestions.get(x)
        match pq.state {
          | PqOpen => queued(q, K_RECHECK, owner_entry(x), now)
          | PqExpired => pq.assumed_by >= 0 or assume_marked(st, x, true)
          | PqEscalated(u) => uq_mover(u)
          | _ => true
        }
      }),
      // commitments
      st.commitments.keys().forall(x => {
        val cm = st.commitments.get(x)
        cm.state == CmOpen implies or {
          queued(q, K_COMMITMENT, owner_entry(x), now),
          fl(cm.node).contains(FOpenCommitment),
          attended_n(cm.node),
          st.pquestions.keys().exists(y => st.pquestions.get(y).node == cm.node and st.pquestions.get(y).conv == cm.conv
                                            and st.pquestions.get(y).state == PqOpen),
        }
      }),
      // `said` entries
      st.saids.keys().forall(x => {
        val sd = st.saids.get(x)
        if (sd.state == SdPending) queued(q, K_SILENCE, owner_session(sd.session), now)
        else if (sd.state == SdUnconfirmed) fl(sd.node).contains(FUnconfirmedSaid) or attended_n(sd.node)
        else true
      }),
      // open intents
      st.nodes.keys().forall(n => st.nodes.get(n).open_intent.eid >= 0 implies
        (fl(n).contains(FOpenIntent) or live_hold_on(st, n, now) or waiting_naive(st, n, now))),
      // open occurrences
      st.nodes.keys().forall(n => (node_open(st, n) and match st.nodes.get(n).role { | Occurrence(_) => true | _ => false }) implies or {
        queued(q, K_OCCURRENCE_SLOT, owner_node(n), now),
        queued(q, K_OCCURRENCE_WINDOW, owner_node(n), now),
        live_hold_on(st, n, now),
        fl(n).contains(FOverdue),
        fl(n).contains(FOpenIntent),
      }),
      // questions for the user
      st.uquestions.keys().forall(u => {
        val uq = st.uquestions.get(u)
        val answered_after_all = match uq.source {
          | SrcEscalation(x) => st.pquestions.keys().contains(x) and match st.pquestions.get(x).state { | PqAnswered(_) => true | _ => false }
          | _ => false
        }
        if (uq.state == UqPending) queued(q, K_USER_QUESTION, owner_entry(u), now)
        else if (uq.state == UqLapsed and uq.assumed_by < 0 and not(answered_after_all)) assume_marked(st, u, true)
        else true
      }),
      // refusals and pauses
      st.refusals.keys().forall(k => queued(q, K_REFUSAL_EXPIRY, owner_refusal(k._1, k._2), now)),
      st.pauses.keys().forall(p => queued(q, K_PAUSE, owner_node(p), now)),
    }
  }

  /// U1, state half: every question for the user has a chain of addressees that is non-empty, has
  /// no repeated stage and at most UQ_CHAIN_MAX stages, ends with `user`, and its stage lies within
  /// it.
  pure def inv_U1(st: State): bool =
    st.uquestions.keys().forall(u => {
      val uq = st.uquestions.get(u)
      val n = uq.to.length()
      and {
        n >= 1,
        n <= UQ_CHAIN_MAX,
        uq.to[n - 1] == USER_PRINCIPAL,
        to_set(uq.to).size() == n,
        uq.stage >= 0 and uq.stage < n,
      }
    })

  /// U1 at creation, for the questions one transition created: the chain of D.8.12, fixed at
  /// creation (§26 U1 [step]); stage 0, pending; no two stages name one person, and no stage
  /// before the last is an alias of `user` (D-3).
  ///
  /// The chain is read on the state at creation, which is `post`: nothing a command or a firing
  /// does after it creates a question changes what `chain` reads (requests, the directory, owners,
  /// principals). D.9.2 reads `spec::chain(before, …)`, the state before the step; an `update` that
  /// sets the owners or the `gone` tag and the needs of a refused node in one call creates the
  /// refusal question after `NodeUpdated`, with the new owners, which `before` does not have
  /// (README, "Open questions"; `u1ChainAtCreationTest`).
  pure def u1_created(pre: State, post: State, la: Action, is_cmd: bool): bool =
    new_ids(pre.uquestions.keys(), post.uquestions.keys()).forall(u => {
      val uq = post.uquestions.get(u)
      val explicit = match la.cmd {
        | CAskUser(a) => if (is_cmd and a.to != USER_KEY and post.principal_ids.keys().contains(a.to)) post.principal_ids.get(a.to) else -1
        | _ => -1
      }
      val src_ok = match uq.source {
        | SrcAgent(x) => is_cmd and x.session == la.sess and match la.cmd { | CAskUser(_) => true | _ => false }
        | _ => true
      }
      val n = uq.to.length()
      and {
        src_ok,
        uq.stage == 0,
        uq.state == UqPending,
        uq.to == chain(post, uq.about, match uq.source { | SrcAgent(_) => explicit | _ => -1 }, is_refusal_source(uq.source)),
        uq.to.foldl({ i: 0, ok: true }, (acc, p) =>
          { i: acc.i + 1, ok: acc.ok and (acc.i == n - 1 or not(same_person(post, p, USER_PRINCIPAL))) }).ok,
        uq.to.foldl({ seen: [], ok: true }, (acc, p) =>
          { seen: acc.seen.append(p), ok: acc.ok and not(any_of(acc.seen, x => same_person(post, x, p))) }).ok,
      }
    })

  /// U1, step half: at creation, per command and per firing.
  pure def step_U1(c: Step): bool =
    if (c.cmd) u1_created(c.pre, c.post, c.la, true)
    else all_firings(c, f => u1_created(f.pre, f.post, c.la, false))

  /// The pending questions and digest entries addressed to the persons of session `x`'s
  /// principals (§20.3, U1's query property), by a scan of every pending item, and by the
  /// principals the session serves (`served`, with every alias), as `query::hello` reads its
  /// per-principal indices. `hello_exact` says that the two agree.
  pure def hello_scan(st: State, x: int): (Set[int], Set[int]) = {
    val ps = st.sessions.get(x).principals
    (st.uquestions.keys().filter(u => st.uquestions.get(u).state == UqPending
       and ps.exists(p => same_person(st, p, st.uquestions.get(u).to[st.uquestions.get(u).stage]))),
     st.digest.keys().filter(d => st.digest.get(d).state == DgPending and ps.exists(p => same_person(st, p, st.digest.get(d).to))))
  }

  pure def hello_indexed(st: State, x: int): (Set[int], Set[int]) = {
    val sv = served(st, x)
    (sv.map(p => st.uquestions.keys().filter(u => st.uquestions.get(u).state == UqPending
                   and st.uquestions.get(u).to[st.uquestions.get(u).stage] == p)).flatten(),
     sv.map(p => st.digest.keys().filter(d => st.digest.get(d).state == DgPending and st.digest.get(d).to == p)).flatten())
  }

  pure def hello_exact(st: State): bool =
    st.sessions.keys().forall(x => hello_scan(st, x) == hello_indexed(st, x))

  // ================================================================== time and limits

  /// T1, state half: every kept entry's time is at most `last_now`, which is at most the clock.
  pure def inv_T1(st: State, now: int): bool =
    st.last_now <= now and st.entries.keys().forall(e => st.entries.get(e).at <= st.last_now)

  pure def presence(se: Session): (int, int, int, bool) = (se.last_seen, se.silence_at, se.retire_at, se.silent)

  /// T1, step half: a command's events are stamped `max(ctx.now, last_now)`, and `last_now` moves
  /// to it; a call that appends nothing (an error other than `stale_token`, a replay, a call with
  /// no event) changes nothing, not the time and no session's presence; only the caller's presence
  /// moves, to now. A firing's events are stamped with its key's time, no earlier than `last_now`,
  /// and `last_now` moves to it.
  pure def step_T1(c: Step): bool =
    if (c.cmd) and {
      match c.la.outcome {
        | OutErr(e) => e != ErrStaleToken implies c.post == c.pre
        | OutReplay => c.post == c.pre
        | _ => true
      },
      c.post == c.pre or c.post.last_now == imax(c.pre.last_now, c.now),
      new_ids(c.pre.entries.keys(), c.post.entries.keys()).forall(e => c.post.entries.get(e).at == c.now),
      c.pre.sessions.keys().forall(x => (x != c.la.sess and c.post.sessions.keys().contains(x)) implies
        presence(c.post.sessions.get(x)) == presence(c.pre.sessions.get(x))),
      (c.pre.sessions.keys().contains(c.la.sess) and c.post.sessions.keys().contains(c.la.sess)
       and presence(c.post.sessions.get(c.la.sess)) != presence(c.pre.sessions.get(c.la.sess)))
        implies c.post.sessions.get(c.la.sess).last_seen == c.now,
    }
    else and {
      all_firings(c, f => and {
        f.key.at >= f.pre.last_now,
        f.post.last_now == imax(f.pre.last_now, f.key.at),
        new_ids(f.pre.entries.keys(), f.post.entries.keys()).forall(e => f.post.entries.get(e).at == f.key.at),
      }),
      c.fs.length() == 0 implies c.post == c.pre,
    }

  /// T2 as a state invariant (D.10.7): `run(run(S, t1), t2) == run(S, t2)` for every pair of times
  /// `now + a <= now + b`, `b <= k`, with `run` the bounded settle and none of these settles
  /// saturated or stuck.
  pure def inv_T2(st: State, now: int, k: int, max_firings: int): bool = {
    val rs = range(0, k + 1).foldl([], (acc, b) => acc.append(settle(st, now + b, max_firings)))
    range(0, k + 1).foldl(true, (ok, b) =>
      ok and not(rs[b].saturated) and not(rs[b].stuck) and
      range(0, k + 1).foldl(true, (ok2, a) => ok2 and (a > b or {
        val r2 = settle(rs[a].state, now + b, max_firings)
        not(r2.saturated) and not(r2.stuck) and r2.state == rs[b].state
      })))
  }

  /// T3: the state is settled at `now`, and every marker equals its predicate (D.9.2).
  pure def inv_T3(st: State, now: int): bool = and {
    queue(st).forall(k => k.at > now),
    st.holds.keys().forall(t => st.holds.get(t).lease_until > now),
    st.nodes.keys().forall(n => node_open(st, n) implies {
      val nd = st.nodes.get(n)
      and {
        match nd.wait {
          | HasWait(w) => match w.on {
              | WTime(t) => w.fired iff t <= now
              | WExternal(t) => w.fired iff t <= now
              | _ => true
            }
          | NoWait => true
        },
        nd.due >= 0 implies (nd.due_fired iff nd.due <= now),
        // 1.8: the Stale marker of a node that is not reserved.
        (nd.origin != OReserved and nd.touched >= 0) implies (nd.stale_fired iff nd.touched + OFFER_STALE <= now),
        match nd.role {
          | Occurrence(o) => ((o.phase == Available) iff o.slot <= now) and (o.window_fired iff o.window_end <= now)
          | _ => true
        },
      }
    }),
    st.nodes.keys().forall(n => node_closed(st, n) implies st.nodes.get(n).wait == NoWait),
    st.commitments.keys().forall(x => st.commitments.get(x).state == CmOpen implies
      (st.commitments.get(x).followup_fired iff st.commitments.get(x).followup_at <= now)),
    st.classes.keys().forall(cls => st.classes.get(cls).caps_seen.keys().forall(caps => st.classes.get(cls).caps_seen.get(caps) > now)),
    st.sessions.keys().forall(x => {
      val se = st.sessions.get(x)
      // 1.4 (R27): silent once silence_at has passed and the session holds no hold.
      (se.silent iff (se.silence_at <= now and st.holds.keys().forall(t => st.holds.get(t).session != x)))
        and se.retire_at > now
    }),
    st.pauses.keys().forall(p => st.pauses.get(p).until > now),
    st.refusals.keys().forall(k => st.refusals.get(k).by.keys().forall(x => st.refusals.get(k).by.get(x).expires_at > now)),
    st.notes.keys().forall(x => st.notes.get(x).until > st.last_now),
    st.aquestions.keys().subseteq(st.notes.keys()),
  }

  /// T4, state half: nothing is queued at or before `now`; the minimum `settle` reads (`first_key`)
  /// is the queue's minimum by its definition; no settle was stuck (D.8.2's `Stuck`).
  pure def inv_T4(st: State, now: int, la: Action): bool = and {
    queue(st).forall(k => k.at > now),
    first_key(st) == queue_min(st),
    not(la.settle.stuck),
  }

  /// Session `x` held a hold before the step and none after it, is not silent, and its
  /// `silence_at` has passed: its Silence key lies at `now` (D.4.8's floor at `last_now`, 1.4).
  pure def last_hold_ended(pre: State, post: State, x: int, now: int): bool = and {
    post.sessions.keys().contains(x),
    not(post.sessions.get(x).silent),
    post.sessions.get(x).silence_at <= now,
    pre.holds.keys().exists(t => pre.holds.get(t).session == x),
    not(post.holds.keys().exists(t => post.holds.get(t).session == x)),
  }

  /// T4, step half: a command queues no key before its now, and one at exactly its now only for
  /// the Silence key of a session whose last hold it ended after that session's `silence_at` (1.4,
  /// D.4.8), which its post-settle fires (D.9.2; the model's `ask` to the user has no explicit due,
  /// so A-68's clamp never applies); each firing fires the queue's minimum, which is gone
  /// afterwards, and queues no new key at or before its time (`check_fire`); the replay ends where
  /// the action's settle ended, with nothing left due.
  pure def step_T4(c: Step): bool =
    if (c.cmd) queue(c.post).exclude(queue(c.pre)).forall(k => k.at > c.now or and {
      k.at == c.now,
      k.kind == K_SILENCE,
      last_hold_ended(c.pre, c.post, owner_payload(k.owner), c.now),
    })
    else and {
      all_firings(c, f => and {
        f.key == queue_min(f.pre),
        not(queue(f.post).contains(f.key)),
        queue(f.post).exclude(queue(f.pre)).forall(k => k.at > f.key.at),
      }),
      c.end == c.post,
      c.rest == NO_KEY or c.rest.at > c.now,
    }

  /// T5: the graph of `{node}` waits and `depends_on` edges is acyclic.
  pure def inv_T5(st: State): bool = st.nodes.keys().forall(n => not(reach_naive(st, n).contains(n)))

  /// L1, state half: every node within `tree.max_depth`.
  pure def inv_L1(st: State): bool = st.nodes.keys().forall(n => depth_naive(st, n) <= TREE_MAX_DEPTH)

  /// L1, step half: a `create` or `move` that raises the open agent-made children of a node other
  /// than PEOPLE and TEAMS leaves them within `tree.max_children` (a reopen may pass it, A-28; the
  /// directory never counts, A-113); a command with a long text changes nothing and is
  /// `limit(TextTooLong)` (A-114).
  pure def step_L1(c: Step): bool = and {
    (c.cmd and c.la.long) implies (c.post == c.pre and c.la.outcome == OutErr(ErrLimit(LkTextTooLong))),
    (c.cmd and match c.la.cmd { | CCreate(_) => true | CMove(_) => true | _ => false }) implies
      c.post.nodes.keys().forall(p => (p != PEOPLE and p != TEAMS and agent_children(c.post, p) > agent_children(c.pre, p))
        implies agent_children(c.post, p) <= TREE_MAX_CHILDREN),
  }

  /// P1: every pause has `now < until <= last_renewed + ttl`, with the scaled `pause.ttl`.
  pure def inv_P1(st: State, now: int): bool =
    st.pauses.keys().forall(p => {
      val pz = st.pauses.get(p)
      now < pz.until and pz.until <= pz.last_renewed + pz.ttl and pz.ttl == PAUSE_TTL
    })

  /// P2 (step): no hold is issued on a node paused, itself or through an ancestor, before the step.
  pure def step_P2(c: Step): bool =
    not(c.cmd) or new_ids(c.pre.holds.keys(), c.post.holds.keys()).forall(t =>
      not(c.pre.nodes.keys().contains(t._1)) or not(paused_naive(c.pre, t._1, c.now)))

  /// `inv_settle_bound` (D.10.4): the last settle, a time action's or a command's post-settle, was
  /// not truncated by MAX_FIRINGS.
  pure def inv_settle_bound(la: Action): bool = not(la.settle.saturated)

  // ------------------------------------------------------------------ liveness (§27, D.10.7)

  // Each liveness property says that the objects of its kind whose deadlines lie within MAX_TIME
  // are eventually gone for good: no other can appear once the clock has passed their deadlines,
  // because no command queues a deadline at or before its `now` (T4). Objects whose deadlines lie
  // beyond MAX_TIME are outside the model's bounds (§28). `forum.qnt` states them as `eventually
  // (always (…))` over the predicates below, and the slices assume weak fairness of time and of
  // their calls (D.10.7).

  /// `deadline_fires`: no queued key lies at or before MAX_TIME.
  pure def no_key_within(st: State): bool = queue(st).forall(k => k.at > MAX_TIME)

  /// `lease_ends`: no hold has a lease that ends within MAX_TIME.
  pure def no_lease_within(st: State): bool = st.holds.keys().forall(t => st.holds.get(t).lease_until > MAX_TIME)

  /// `pause_ends`: no pause ends within MAX_TIME.
  pure def no_pause_within(st: State): bool = st.pauses.keys().forall(p => st.pauses.get(p).until > MAX_TIME)

  /// `occurrence_resolves`: every open occurrence whose window ends within MAX_TIME is flagged
  /// (`overdue`, its due being its window end): it closed (as missed or otherwise) or is flagged.
  pure def occurrences_resolved(st: State, now: int): bool =
    st.nodes.keys().forall(o => (st.nodes.get(o).life == Open and match st.nodes.get(o).role {
      | Occurrence(x) => x.window_end <= MAX_TIME
      | _ => false
    }) implies flagged(st, o, now))

  /// `uq_resolves`: no question for the user is pending with a due within MAX_TIME: each was
  /// answered, defaulted, or lapsed, with or without a default (CS-2).
  pure def uq_resolved(st: State): bool =
    st.uquestions.keys().forall(u => st.uquestions.get(u).state == UqPending implies st.uquestions.get(u).due > MAX_TIME)

  /// `wait_on_uq_resolves`: every wait on a question for the user whose due lies within MAX_TIME
  /// has resolved.
  pure def uq_waits_resolved(st: State, now: int): bool =
    st.nodes.keys().forall(n => match st.nodes.get(n).wait {
      | HasWait(w) => match w.on {
          | WUserQuestion(u) => not(waiting(st, n, now)) or (st.uquestions.keys().contains(u) and st.uquestions.get(u).due > MAX_TIME)
          | _ => true
        }
      | NoWait => true
    })

  // ================================================================== every check (D.10.7's `inv_all`)

  /// Every check of D.9.2 that the model makes, on the action from `pre` through `cmd_post` (after
  /// its command, before its settle) to `post` at clock `clk`: the state invariants on `post` at
  /// `clk`, the step invariants on each step of the action (`steps_of`), and T2 over pairs of times
  /// up to `t2_k` ticks ahead (none when `t2_k < 0`). `forum.qnt`'s `inv_all`; each slice checks
  /// its own part of it (`slices/*.qnt`).
  pure def all_invariants(pre: State, cmd_post: State, post: State, clk: int, la: Action, max_firings: int, t2_k: int): bool = {
    val cs = steps_of(pre, cmd_post, post, clk, la, max_firings)
    and {
      inv_settle_bound(la),
      inv_H1(post, clk), inv_H2(post, clk), inv_H3(post), inv_H5(post), inv_H6(post),
      inv_O1(post), inv_O2(post),
      inv_K1(post), inv_K3(post), inv_K4(post), inv_K5(post), inv_K6(post), inv_K8(post),
      inv_Q1(post), inv_S1(post),
      inv_N1(post, clk), inv_Settled(post), inv_F(post, clk),
      inv_E1(post, clk), inv_W1(post, clk), inv_R2(post), inv_D1(post),
      inv_M1(post, clk), inv_U1(post), hello_exact(post),
      inv_T1(post, clk), inv_T3(post, clk), inv_T4(post, clk, la), inv_T5(post),
      inv_L1(post), inv_P1(post, clk),
      each_step(cs, c => and {
        step_H2(c), step_H3(c), step_H4(c), step_O1(c), step_O2(c), step_O3(c), step_K7(c), step_C1(c),
        step_Settled(c), step_N5(c), step_R2(c), step_D1(c), step_U1(c), step_T1(c), step_T4(c),
        step_L1(c), step_P2(c),
      }),
      t2_k < 0 or inv_T2(post, clk, t2_k, max_firings),
    }
  }
}
