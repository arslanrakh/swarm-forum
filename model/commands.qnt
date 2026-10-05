// -*- mode: Bluespec; -*-
/**
 * Commands (D.10.1, D.10.4): one pure function per command of D.10.4, validation then execution, and
 * `run_cmd`, which is `apply` of D.8.1 for one call: the text-size check (L1, A-114), the retry store
 * (O1), the session, `validate` in the order of D.8.24, the `late` entry of a stale token (H4), and
 * on success `Touched` (CS-14), the command's base changes, and the `op_id` record.
 *
 * Where D.8.24's general order and a command's own section of D.8 disagree, the model follows the
 * command's section: `from_user` checks `answers` (NotUserQuestion) before `node` resolves (D.8.11),
 * and `move` checks ReservedNode before the new parent resolves (D.8.20), where D.8.24 puts every
 * `not_found` first (README, "Open questions", item 10; `validationOrderTest`).
 *
 * The model has no event log (D.10.3): an execution applies the base change of each event of D.7.4 in
 * the order Rust emits them (D.8.9 onwards), so that ids (nodes, eids, principals, sessions, fences)
 * are allocated in the same order as Rust's and a model state compares with a projected Rust state
 * field by field (D.10.8). `evolve`'s step 1 (the expiry prune and `last_now`) is `at_event`, run
 * once before the first event of a call that appends any; a call that appends nothing returns the
 * state unchanged (T1, A-101).
 *
 * Every helper here is total on the states it is called with: validation checks every reference
 * before an execution reads it (D.8.1 step 4), as Rust's `Plan` does.
 *
 * Locals are never named after a variable of `forum.qnt` or of a slice (`s`, `clock`, `prev`,
 * `last_action`): Apalache prefixes local names like variables, and TLC refuses the clash (README).
 */
module commands {
  import types.* from "./types"
  import params.* from "./params"
  import basics.* from "./basics"
  import flags.* from "./flags"
  import deadlines.* from "./deadlines"

  // ------------------------------------------------------------------ configuration and results

  /// The constants of `forum.qnt` that a command reads (D.10.1): the tie-break of `next` (D.10.6)
  /// and `refusal.quorum` (CS-7).
  type Cfg = { use_fnv: bool, quorum: int }

  /// A validation verdict: the first failing check of D.8.24's order, or none.
  type Check = Pass | Fail(ErrorCode)

  /// What an execution returns: the state after the command's events (without `Touched` and the
  /// `op_id` record, which `run_cmd` adds), the ids of its reply, whether it appended any event,
  /// and whether it issued a hold (which makes `Touched` due, CS-14).
  type Ex = { st: State, ids: List[int], ev: bool, hold: bool }

  /// `run_cmd`'s result: the state after the call and the outcome conformance compares (D.10.8).
  type Run = { state: State, outcome: CmdOutcome }

  pure def ex(st: State, ids: List[int]): Ex = { st: st, ids: ids, ev: true, hold: false }
  pure def ex_if(st: State, ids: List[int], ev: bool): Ex = { st: st, ids: ids, ev: ev, hold: false }
  pure def ex_hold(st: State, ids: List[int]): Ex = { st: st, ids: ids, ev: true, hold: true }

  pure def nf(w: What): Check = Fail(ErrNotFound(w))
  pure def bad(w: Why): Check = Fail(ErrInvalid(w))

  pure def set_max(xs: Set[int], d: int): int = xs.fold(d, (acc, x) => if (acc == d or x > acc) x else acc)

  // ------------------------------------------------------------------ the order of keys

  // Rust iterates a node's keys, a `BTreeSet<Key>`, in the order of their strings, and the replay
  // driver sends a command's keys in that order too. Quint cannot compare strings, so `key_rank`
  // gives each key of the model's domains an integer that orders like the string the driver sends
  // for it (D.10.8): `chat:t/<c>` for `Conv(c)`, `chat:t/<c>/<n>` for `Msg((c, n))`, `lean:<o>`
  // for `Obj(o)`, `person:test:p1`, `person:test:p2`, and `person:user` for `Person`,
  // `repeat:<series>:<YYYYMMDDTHHMMZ>` for `RepeatSlot`, and `user:e<e>` for `User(e)`. The schemes
  // sort `chat` < `lean` < `person` < `repeat` < `user`, and the decimal ids inside them by their
  // digits, as strings do (`user:e10` < `user:e9`).

  pure val CONV_INDEX: str -> int = Map("c1" -> 1, "c2" -> 2)
  pure val OBJ_INDEX: str -> int = Map("o1" -> 1)
  /// The principal keys of the model, in the order of their replayed strings (`test:p1`,
  /// `test:p2`, `user`); `PRINCIPAL_ORDER` lists them so.
  pure val PRINCIPAL_RANK: str -> int = Map("p1" -> 1, "p2" -> 2, "user" -> 3)
  pure val PRINCIPAL_ORDER: List[str] = ["p1", "p2", "user"]

  /// The position of the decimal string of `x` (0 <= x < 100) among such strings.
  pure def lex_rank(x: int): int = if (x < 10) x * 11 else (x / 10) * 11 + x % 10 + 1

  pure def key_rank(k: Key): int =
    match k {
      | Conv(c) => 1000000 + get_or(CONV_INDEX, c, 0) * 100
      | Msg(m) => 1000000 + get_or(CONV_INDEX, m._1, 0) * 100 + m._2
      | Obj(o) => 2000000 + get_or(OBJ_INDEX, o, 0)
      | Person(p) => 3000000 + get_or(PRINCIPAL_RANK, p, 0)
      | RepeatSlot(r) => 4000000 + lex_rank(r._1) * 1000 + r._2
      | User(e) => 5000000 + lex_rank(e)
    }

  /// The keys of `ks` in the order of their strings.
  pure def sorted_keys(ks: Set[Key]): List[Key] =
    sorted(ks.map(k => key_rank(k))).foldl([], (out, r) =>
      out.append(ks.filter(k => key_rank(k) == r).fold(Conv(""), (acc, k) => k)))

  pure def user_key_entry(k: Key): int = match k { | User(e) => e | _ => -1 }
  pure def is_repeat_key(k: Key): bool = match k { | RepeatSlot(_) => true | _ => false }

  // ------------------------------------------------------------------ nodes and roles

  pure def is_agent_made(nd: Node): bool =
    match nd.origin { | OAgent(_) => true | OAncestor(_) => true | _ => false }

  pure def occ_waiting(nd: Node): bool =
    match nd.role { | Occurrence(o) => o.phase == Waiting | _ => false }

  pure def occ_available(nd: Node): bool =
    match nd.role { | Occurrence(o) => o.phase == Available | _ => false }

  /// An occurrence in phase `Available` whose window has not ended: tier 0 of `next` (§10.4).
  pure def occ_in_window(nd: Node): bool =
    match nd.role { | Occurrence(o) => o.phase == Available and not(o.window_fired) | _ => false }

  pure def role_is_signal(r: Role): bool = match r { | RoleSignal(_) => true | _ => false }

  /// `open_agent_children(p)` (D.8.4): the open children with origin `Agent` or `Ancestor`, other
  /// than `except`.
  pure def open_agent_children(s: State, p: int, except: int): int =
    children(s, p).filter(c => c != except and is_open(s, c) and is_agent_made(s.nodes.get(c))).size()

  /// L1's child half (D.8.4): one more open agent-made child of `p` stays within
  /// `tree.max_children`; PEOPLE and TEAMS have no child limit (A-113).
  pure def child_room(s: State, p: int, except: int): bool =
    p == PEOPLE or p == TEAMS or open_agent_children(s, p, except) + 1 <= TREE_MAX_CHILDREN

  /// `height(n)`: the length of the longest downward path from `n` in its subtree.
  pure def height(s: State, n: int): int = {
    val dn = depth(s, n)
    s.nodes.keys().fold(0, (acc, m) => if (m != n and is_under(s, m, n)) imax(acc, depth(s, m) - dn) else acc)
  }

  /// `KeyAdded` (version + 1); it touches the node (1.8, §5).
  pure def add_key(s: State, n: int, k: Key, now: int): State =
    upd_node(s, n, nd => { ...nd, keys: nd.keys.union(Set(k)), version: nd.version + 1, touched: now, stale_fired: false })

  /// `KeyRemoved` (version + 1); it touches the node (1.8, §5).
  pure def remove_key(s: State, n: int, k: Key, now: int): State =
    upd_node(s, n, nd => { ...nd, keys: nd.keys.exclude(Set(k)), version: nd.version + 1, touched: now, stale_fired: false })

  /// The open occurrence of series `x`, or -1.
  pure def open_occurrence(s: State, x: int): int = set_min(open_occurrences(s, x), -1)

  // ------------------------------------------------------------------ sessions and presence (D.8.23)

  pure def has_session(s: State, x: int): bool = s.sessions.keys().contains(x)

  /// `Touched` is due for a session that is silent or was last seen a resolution ago (CS-14).
  pure def presence_due(s: State, sess: int, now: int): bool = {
    val se = s.sessions.get(sess)
    se.silent or now >= se.last_seen + PRESENCE_RESOLUTION
  }

  /// `Touched { session, silence_at, retire_at }` (D.7.4, CS-8): `retire_at` never decreases.
  pure def touched(s: State, sess: int, now: int): State =
    { ...s, sessions: s.sessions.setBy(sess, se => {
        ...se,
        last_seen: now,
        silence_at: now + PRESENCE_SILENCE,
        retire_at: imax(se.retire_at, now + SESSION_RETAIN),
        silent: false,
      }) }

  /// A brief of `n` raises the session's `shown[n]` (D.8.6, CS-14).
  pure def raises_shown(s: State, sess: int, n: int): bool =
    s.nodes.get(n).seq > get_or(s.sessions.get(sess).shown, n, 0)

  /// `Shown { sess, n, n.seq }` when it raises `shown[n]`, and nothing otherwise.
  pure def shown(s: State, sess: int, n: int): State =
    if (raises_shown(s, sess, n))
      { ...s, sessions: s.sessions.setBy(sess, se => { ...se, shown: se.shown.put(n, s.nodes.get(n).seq) }) }
    else s

  // ------------------------------------------------------------------ principals (D.8.11, CS-1)

  type Interned = { st: State, id: int }

  /// `intern(key)`: `principal_ids[key]`, or a `PrincipalSeen` first when the key is new.
  pure def intern(s: State, k: str): Interned =
    if (s.principal_ids.keys().contains(k)) { st: s, id: s.principal_ids.get(k) }
    else {
      st: { ...s, principal_ids: s.principal_ids.put(k, s.next_principal), next_principal: s.next_principal + 1 },
      id: s.next_principal,
    }

  /// `PrincipalSeen` for each `person:` key of `ks` whose principal is not interned yet, in key
  /// order (D.8.9).
  pure def intern_person_keys(s: State, ks: List[Key]): State =
    ks.foldl(s, (st, k) => match k { | Person(p) => intern(st, p).st | _ => st })

  /// How `resolve_principal(sess, arg)` (D.8.11) resolves, before anything is interned: the
  /// argument's key, a principal the session serves (or USER_PRINCIPAL, for a `to_user` only since
  /// 1.9: a `from_user` there is `invalid(NoPrincipal)` first), or ambiguous.
  type PrincipalChoice = PcKey(str) | PcId(int) | PcAmbiguous

  pure def principal_choice(s: State, sess: int, arg: str): PrincipalChoice =
    if (arg != "") PcKey(arg)
    else {
      val ps = s.sessions.get(sess).principals
      val low = set_min(ps, -1)
      if (ps == Set()) PcId(USER_PRINCIPAL)
      else if (ps.forall(p => same_person(s, p, low))) PcId(low)
      else PcAmbiguous
    }

  pure def take_principal(s: State, c: PrincipalChoice): Interned =
    match c {
      | PcKey(k) => intern(s, k)
      | PcId(p) => { st: s, id: p }
      | PcAmbiguous => { st: s, id: -1 }
    }

  // ------------------------------------------------------------------ holds (D.8.10)

  /// L of §10.1 (A-8): the `lease` argument, clamped to [1, lease.max], default `lease.default`.
  /// Rust's lower clamp is one second; the model's domains never pick a lease below one tick.
  pure def lease_l(lease: int): int = if (lease < 0) LEASE_DEFAULT else imin(imax(lease, 1), LEASE_MAX)

  /// `issue_hold(n, lease)`: `HoldIssued { Token { n, fence + 1 }, sess, now + lease.first, terms, l }`.
  pure def issue_hold(s: State, n: int, sess: int, lease: int, now: int): State = {
    val f = s.nodes.get(n).fence + 1
    {
      ...upd_node(s, n, nd => { ...nd, fence: f }),
      holds: s.holds.put((n, f), {
        session: sess, taken_at: now, lease_until: now + LEASE_TERMS.first, last_extended: now,
        terms: LEASE_TERMS, l: lease_l(lease), renews: 0, checkpointed: false,
      }),
    }
  }

  /// `extend`'s new `lease_until` (D.8.10): up to `taken_at + first` before the first checkpoint,
  /// up to `taken_at + ceiling` after it, and never lower than now.
  pure def extended_until(h: Hold, now: int): int = {
    val cap = if (h.checkpointed) h.taken_at + h.terms.ceiling else h.taken_at + h.terms.first
    imax(h.lease_until, imin(now + h.l, cap))
  }

  /// `bind_holder_notes(n)` (D.8.16): a `holder:` note waiting for a holder of `n` binds to `sess`,
  /// unless `sess` wrote it (1.4, R12: the sender is never a `holder:` recipient; 1.10 makes such
  /// notes common, a note without `to` at a node nobody holds).
  pure def bind_holder_notes(s: State, n: int, sess: int, now: int): State =
    { ...s, notes: s.notes.keys().mapBy(x => {
        val nt = s.notes.get(x)
        if (nt.pending_holder and nt.to == ToHolder(n) and nt.until > now
            and not(s.entries.keys().contains(x) and s.entries.get(x).by == sess))
          { ...nt, recipients: nt.recipients.union(Set(sess)), pending_holder: false }
        else nt
      }) }

  /// The tokens of the holds that session `sess` took (in the map; live on a settled state).
  pure def holds_of(s: State, sess: int): Set[(int, int)] =
    s.holds.keys().filter(t => s.holds.get(t).session == sess)

  /// `sess` holds `n`: a hold of its own on `n` in the map.
  pure def holds_node(s: State, sess: int, n: int): bool =
    holds_on(s, n).exists(t => s.holds.get(t).session == sess)

  /// A token is a live hold on `n` (H4: anything else is `stale_token`).
  pure def token_live(s: State, t: (int, int), n: int, now: int): bool =
    t._1 == n and s.holds.keys().contains(t) and live(s.holds.get(t), now)

  /// A checkpoint entry (`Appended(Checkpoint)`, not kept, CS-17): `checkpoints += 1`, `attempts :=
  /// 0`, `last_checkpoint`, and the token's hold checkpointed (D.7.4).
  pure def checkpoint_entry(s: State, t: (int, int), sess: int, now: int): State = {
    val e = s.next_eid
    val s1 = append_entry(s, t._1, now, sess, ECheckpoint, -1, false)
    val s2 = upd_node(s1, t._1, nd => { ...nd, checkpoints: nd.checkpoints + 1, attempts: 0, last_checkpoint: e })
    if (s2.holds.keys().contains(t)) { ...s2, holds: s2.holds.setBy(t, h => { ...h, checkpointed: true }) } else s2
  }

  /// `SaidConfirmed { x, Release | Close }` for each pending `said` of the session (§9.2; `unsent`
  /// is not modelled).
  pure def confirm_saids(s: State, sess: int): State =
    { ...s, saids: s.saids.keys().mapBy(x => {
        val sd = s.saids.get(x)
        if (sd.session == sess and sd.state == SdPending) { ...sd, state: SdConfirmed } else sd
      }) }

  // ------------------------------------------------------------------ waits and cycles (D.8.4, T5)

  /// Every model forum has fewer nodes than this, so a walk of this many steps reaches every node
  /// that is reachable at all (the bound of `reaches`, D.8.4).
  pure val REACH_STEPS = 24

  /// The edges of T5 out of `x`: its `{node}` wait, resolved or not, and its `depends_on`.
  pure def succ(s: State, x: int): Set[int] =
    if (not(has_node(s, x))) Set()
    else {
      val nd = s.nodes.get(x)
      val w = match nd.wait {
        | NoWait => Set()
        | HasWait(wt) => match wt.on { | WNode(t) => Set(t) | _ => Set() }
      }
      w.union(nd.depends_on)
    }

  /// `reaches(s, x, y)`: `y` is reachable from `x` along the edges of T5 (`x` itself included).
  pure def reaches(s: State, x: int, y: int): bool =
    range(0, REACH_STEPS).foldl(Set(x), (r, i) => r.union(r.map(z => succ(s, z)).flatten())).contains(y)

  /// The edges after a merge of `fm` into `into` (D.8.20): every edge to `fm` now goes to `into`,
  /// and self-edges are dropped. `fm` has no incoming edge left, so its own edges, which the merge
  /// keeps, cannot be part of a cycle.
  pure def succ_merged(s: State, x: int, fm: int, into: int): Set[int] =
    succ(s, x).map(t => if (t == fm) into else t).exclude(Set(x))

  /// The merge's edges `E'` have a cycle (N5, T5).
  pure def merge_cycle(s: State, fm: int, into: int): bool =
    s.nodes.keys().exists(x =>
      range(0, REACH_STEPS).foldl(succ_merged(s, x, fm, into), (r, i) =>
        r.union(r.map(z => succ_merged(s, z, fm, into)).flatten())).contains(x))

  // ------------------------------------------------------------------ requests and routing (D.8.8, D.8.11)

  type Recorded = { st: State, id: int }

  /// `ix.origins[origin]` (Q1), or -1.
  pure def request_by_origin(s: State, o: Key): int =
    set_min(s.requests.keys().filter(r => s.requests.get(r).origin == o), -1)

  /// `record_request(c, origin, conv, from_user)`: the request of that origin if one exists (Q1),
  /// otherwise `Appended(Request)` on `c` by `by`, with `conv` the conversation key `c` was found
  /// by (§9.4, 1.3), and requester `User(p)` for the principal of the entry `from_user`, and
  /// `External` without one (CS-4).
  pure def record_request(s: State, c: int, origin: Key, conv: Key, from_user: int, by: int, now: int): Recorded =
    if (request_by_origin(s, origin) >= 0) { st: s, id: request_by_origin(s, origin) }
    else {
      val r = s.next_eid
      val s1 = append_entry(s, c, now, by, ERequest, -1, true)
      val who =
        if (from_user >= 0 and s1.user_entries.keys().contains(from_user)) ReqUser(s1.user_entries.get(from_user).principal)
        else ReqExternal
      {
        st: { ...s1, requests: s1.requests.put(r, {
          node: c, origin: origin, conv: conv, from_user: from_user, requester: who, answered_by: -1,
          settled: Unsettled, moved_from: -1,
        }) },
        id: r,
      }
    }

  /// `route_to(e, n)` (D.8.11): `KeyAdded { n, user:e<e> }` unless an open node carries it, the
  /// request, and `EntryRouted { e, Request { node: n, request } }`. A node that carries the key
  /// already (a closed one, after `node key {add}` of that key) does not get it again: D.8.11's
  /// "if no open node carries" would emit a `KeyAdded` that fails its precondition (README).
  pure def route_to(s: State, e: int, n: int, by: int, now: int): Recorded = {
    val k = User(e)
    val s1 = if (key_open(s, k) < 0 and not(s.nodes.get(n).keys.contains(k))) add_key(s, n, k, now) else s
    val r = record_request(s1, n, k, k, e, by, now)
    {
      st: { ...r.st, user_entries: r.st.user_entries.setBy(e, u => { ...u, routing: RtRequest({ node: n, request: r.id }) }) },
      id: r.id,
    }
  }

  /// `route_to` for each user key of `ks` whose entry is unrouted, in key order (D.8.9).
  pure def route_user_keys(s: State, ks: List[Key], n: int, by: int, now: int): State =
    ks.foldl(s, (st, k) => {
      val e = user_key_entry(k)
      if (e >= 0 and st.user_entries.keys().contains(e) and st.user_entries.get(e).routing == Unrouted)
        route_to(st, e, n, by, now).st
      else st
    })

  /// The user answer of D.8.8: a `to_user` of kind `result` with eid `a` to principal `p` answers
  /// request `r` of the same person, still unanswered and older than `a`.
  pure def answers_user_request(s: State, r: int, p: int, a: int): bool =
    s.requests.keys().contains(r) and {
      val req = s.requests.get(r)
      req.answered_by < 0 and r < a and match req.requester {
        | ReqUser(q) => same_person(s, p, q)
        | ReqExternal => false
      }
    }

  // ------------------------------------------------------------------ signals (D.8.19)

  pure def signal_rel(k: SignalKind): LinkRel = match k { | SigDuplicate => Duplicate | SigConflict => ConflictsWith }

  /// `open_signal(kind, x, y)`: the open signal of the pair, or a new signal node under SIGNALS
  /// tagged `offer`, with `SignalOpened`. -1 for `x = y`.
  pure def open_signal(s: State, kind: SignalKind, x: int, y: int, now: int): Recorded =
    if (x == y) { st: s, id: -1 }
    else {
      val a = imin(x, y)
      val b = imax(x, y)
      if (s.signals.keys().contains((kind, a, b))) { st: s, id: s.signals.get((kind, a, b)) }
      else {
        val id = s.next_node
        val nd: Node = {
          ...reserved_node(SIGNALS, "", now),
          name: Signal((kind, a, b, id)),
          origin: OSignal,
          tags: Set(OFFER),
          capacity: 1,
          links: Set((signal_rel(kind), a), (signal_rel(kind), b)),
          role: RoleSignal({ kind: kind, a: a, b: b }),
        }
        val s1 = ev_node_created(s, nd, now)
        { st: { ...s1, signals: s1.signals.put((kind, a, b), id) }, id: id }
      }
    }

  /// Resolves the open signals of the pair `{x, y}`, both kinds: the server closes each signal node
  /// as `done` (D.8.19).
  pure def resolve_signals(s: State, x: int, y: int, now: int): State =
    [SigDuplicate, SigConflict].foldl(s, (st, kd) => {
      val key = (kd, imin(x, y), imax(x, y))
      if (st.signals.keys().contains(key)) close_node(st, st.signals.get(key), Done, SERVER, -1, CauseClose, -1, now)
      else st
    })

  // ------------------------------------------------------------------ reopen (D.8.20)

  /// `reopen(n, reason)`, shared by `node reopen` and by `take` and `next` of a flagged closed node:
  /// the keys another open node carries leave (K7); an occurrence or a signal whose key is taken is
  /// detached; `NodeReopened`; a free signal key is mapped again (S1); a duplicate signal for each
  /// dropped key; the system entries; a series' next occurrence; and `e1_check` (with `by` the
  /// session as the author of a refusal question).
  pure def reopen(s: State, n: int, by: int, now: int): State = {
    val dropped = sorted_keys(s.nodes.get(n).keys).select(k => key_open(s, k) >= 0 and key_open(s, k) != n)
    val others = dropped.foldl([], (out, k) => out.append(key_open(s, k)))
    val s1 = dropped.foldl(s, (st, k) => remove_key(st, n, k, now))
    val s2 = match s1.nodes.get(n).role {
      | Occurrence(_) => upd_node(s1, n, nd => { ...nd, role: Plain })
      | RoleSignal(key) =>
          if (s1.signals.keys().contains((key.kind, key.a, key.b)) and s1.signals.get((key.kind, key.a, key.b)) != n)
            upd_node(s1, n, nd => { ...nd, role: Plain })
          else s1
      | _ => s1
    }
    val s3 = upd_node(s2, n, nd => {
      ...nd, life: Open, version: nd.version + 1, due_fired: nd.due >= 0 and nd.due <= now,
      touched: now, stale_fired: false,
    })
    val s4 = match s3.nodes.get(n).role {
      | RoleSignal(key) =>
          if (s3.signals.keys().contains((key.kind, key.a, key.b))) s3
          else { ...s3, signals: s3.signals.put((key.kind, key.a, key.b), n) }
      | _ => s3
    }
    val s5 = others.foldl(s4, (st, o) => open_signal(st, SigDuplicate, n, o, now).st)
    val s6 = if (dropped.length() > 0) append_entry(s5, n, now, by, ESystem, -1, false) else s5
    val s7 = append_entry(s6, n, now, by, ESystem, -1, false)
    val s8 = if (is_series(s7, n)) open_next_occurrence(s7, n, now, now) else s7
    e1_check(s8, n, by, now)
  }

  // ------------------------------------------------------------------ people questions (D.8.15)

  /// `answer_pq(q, by)`: `PqAnswered`, the marks the question set leave, and an escalated
  /// question's user question is answered (A-60), or the marks of its lapse leave (CS-2).
  pure def answer_pq(s: State, q: int, by: int): State = {
    val pq = s.pquestions.get(q)
    val s1 = { ...s, pquestions: s.pquestions.set(q, { ...pq, state: PqAnswered(by) }) }
    val s2 = { ...s1, nodes: s1.nodes.keys().mapBy(m => {
      val nd = s1.nodes.get(m)
      { ...nd, recheck_marks: nd.recheck_marks.exclude(Set(q)), assume_marks: nd.assume_marks.exclude(Set(q)) }
    }) }
    match pq.state {
      | PqEscalated(uq) =>
          if (not(s2.uquestions.keys().contains(uq))) s2
          else if (s2.uquestions.get(uq).state == UqPending)
            { ...s2, uquestions: s2.uquestions.setBy(uq, u => { ...u, state: UqAnswered(by) }) }
          else if (s2.uquestions.get(uq).state == UqLapsed)
            { ...s2, nodes: s2.nodes.keys().mapBy(m => {
                val nd = s2.nodes.get(m)
                { ...nd, assume_marks: nd.assume_marks.exclude(Set(uq)) }
              }) }
          else s2
      | _ => s2
    }
  }

  pure def pq_answerable(st: PqState): bool =
    match st { | PqOpen => true | PqExpired => true | PqEscalated(_) => true | _ => false }

  /// `q` is a question entry of any kind (people, agent, or user).
  pure def is_question(s: State, q: int): bool =
    s.pquestions.keys().contains(q) or s.uquestions.keys().contains(q) or s.aquestions.keys().contains(q)
    or (s.entries.keys().contains(q) and s.entries.get(q).ty == EQuestion)

  /// `q` is an agent question: resident, or a question entry that is neither a people nor a user
  /// question (an agent question leaves memory with its note at `until`, D.4.12).
  pure def is_agent_question(s: State, q: int): bool =
    s.aquestions.keys().contains(q)
    or (s.entries.keys().contains(q) and s.entries.get(q).ty == EQuestion
        and not(s.pquestions.keys().contains(q)) and not(s.uquestions.keys().contains(q)))

  // ------------------------------------------------------------------ notes and the inbox (D.8.16)

  /// The principals a session serves, with every alias of each (D.8.11).
  pure def served(s: State, sess: int): Set[int] =
    s.sessions.get(sess).principals.fold(Set(), (acc, p) => acc.union(aliases(s, p)))

  /// The notes delivered to `sess` and not read by it: addressed to it, or to a principal it serves.
  pure def direct_inbox(s: State, sess: int, now: int): Set[int] =
    s.notes.keys().filter(x => {
      val nt = s.notes.get(x)
      nt.until > now and not(nt.read_by.contains(sess)) and (
        nt.recipients.contains(sess) or match nt.to { | ToPrincipal(p) => served(s, sess).contains(p) | _ => false })
    })

  /// The scopes of the session's holds: each node it holds and its ancestors.
  pure def held_scopes(s: State, sess: int): Set[int] =
    holds_of(s, sess).map(t => to_set(ancestors_or_self(s, t._1))).flatten()

  /// The live `holders:` notes of the branches the session holds in, newer than its cursor (D-6).
  pure def branch_inbox(s: State, sess: int, now: int): Set[int] =
    s.notes.keys().filter(x => {
      val nt = s.notes.get(x)
      nt.until > now and match nt.to {
        | ToHolders(a) => held_scopes(s, sess).contains(a) and x > get_or(s.sessions.get(sess).branch_seen, a, 0)
        | _ => false
      }
    })

  type Delivered = { st: State, delivered: Set[int] }

  /// `inbox(s, sess, now)` and its `InboxRead`: the direct notes get `sess` in `read_by`, and a
  /// `holders:` note advances the cursor of its scope.
  pure def read_inbox(s: State, sess: int, now: int): Delivered = {
    val d = direct_inbox(s, sess, now)
    val b = branch_inbox(s, sess, now)
    val s1 = { ...s, notes: s.notes.keys().mapBy(x => {
      val nt = s.notes.get(x)
      if (d.contains(x)) { ...nt, read_by: nt.read_by.union(Set(sess)) } else nt
    }) }
    val s2 = b.fold(s1, (st, x) => match st.notes.get(x).to {
      | ToHolders(a) => { ...st, sessions: st.sessions.setBy(sess, se =>
          { ...se, branch_seen: se.branch_seen.put(a, imax(get_or(se.branch_seen, a, 0), x)) }) }
      | _ => st
    })
    { st: s2, delivered: d.union(b) }
  }

  /// `recipients(to)` (D.8.16): fixed for a session and for the live holders of a node (or
  /// `pending_holder` when nobody holds it); none for `holders:` and principals (read time).
  type Recipients = { to: Addressee, recipients: Set[int], pending_holder: bool }

  /// `holder:<n>`: the holders of `n` other than the sender `sess` (1.4, R12), or, with none, the
  /// next taker other than it (`pending_holder`). 1.10: the sender left out, as the core does.
  /// 1.12 (§13): at a closed node, which is never taken, the session that closed it while it is
  /// live and is not the sender (`closer`), else nobody, and never pending.
  pure def holder_recipients(s: State, n: int, sess: int): Recipients =
    if (node_is_open(s.nodes.get(n))) {
      val hs = holds_on(s, n).map(t => s.holds.get(t).session).exclude(Set(sess))
      { to: ToHolder(n), recipients: hs, pending_holder: hs == Set() }
    } else {
      val cl = closer(s, n, sess)
      { to: ToHolder(n), recipients: if (cl >= 0) Set(cl) else Set(), pending_holder: false }
    }

  pure def agent_recipients(s: State, a: AgentTo, sess: int): Recipients =
    match a {
      | AgSession(x) => { to: ToSession(x), recipients: Set(x), pending_holder: false }
      | AgHolder(n) => holder_recipients(s, n, sess)
      | AgHolders(n) => { to: ToHolders(n), recipients: Set(), pending_holder: false }
    }

  /// Records an addressed note on `n` (`Appended(Note | Question(Agent))`, kept), with eid
  /// `s.next_eid`.
  pure def record_note(s: State, n: int, rc: Recipients, ty: EntryType, by: int, now: int): State = {
    val x = s.next_eid
    val s1 = append_entry(s, n, now, by, ty, -1, true)
    { ...s1, notes: s1.notes.put(x, {
      node: n, to: rc.to, until: now + DELIVER_WINDOW, recipients: rc.recipients,
      pending_holder: rc.pending_holder, read_by: Set(),
    }) }
  }

  // ------------------------------------------------------------------ candidates and `next` (D.8.5)

  /// The classification of `n` (D.8.5): -1 none, 0 `Cand::Node`, 1 `Cand::Status`, an item with
  /// no hold: a waiting node's status item, or, 1.9 (§10.4), the review of a closed node's open
  /// objection, which `next` does not reopen.
  pure def classify(s: State, n: int, fl: Set[Flag], now: int): int = {
    val nd = s.nodes.get(n)
    if (nd.origin == OReserved) -1
    else if (node_is_open(nd) and occ_waiting(nd)) -1
    else if (not(node_is_open(nd)) and occ_locked(s, n)) -1
    else if (merged(s, n)) -1
    else if (upheld(s, n)) -1
    else if (not(node_is_open(nd)) and fl.contains(FObjection)) 1
    else if (not(node_is_open(nd))) { if (nd.capacity > 0 and fl.intersect(CLOSED_TAKE) != Set()) 0 else -1 }
    else if (waiting(s, n, now)) { if (fl.contains(FOpenCommitment)) 1 else -1 }
    else if (holds_on(s, n).size() < nd.capacity
             and ((nd.tags.contains(OFFER) and not(is_container(s, n))) or fl != Set() or occ_available(nd))) 0
    else -1
  }

  /// 1.11 (§10.4, §12): `n` is an open series whose open occurrence waits for its slot.
  pure def series_before_slot(s: State, n: int): bool =
    is_open(s, n) and is_series(s, n) and open_occurrences(s, n).exists(o => occ_waiting(s.nodes.get(o)))

  /// `ix.next.held`: an open dependency, or a pause over `n` (D.8.5); 1.11: a series whose open
  /// occurrence waits for its slot.
  pure def held_back(s: State, n: int, now: int): bool =
    s.nodes.get(n).depends_on.exists(d => is_open(s, d)) or paused(s, n, now) or series_before_slot(s, n)

  /// `tier(c)` of §10.4 for a node candidate.
  pure def tier_of(s: State, n: int, fl: Set[Flag]): int = {
    val nd = s.nodes.get(n)
    if (fl.intersect(T0) != Set() or occ_in_window(nd)) 0
    else if (nd.priority > 0 or fl.exclude(Set(FStale)) != Set()) 1
    else 2
  }

  /// A candidate with its key `(tier, stale, est, recheck, review, −aged, h, id, kind_rank)`;
  /// `aged = priority = 0` within the model's time bound (D.10.5), and the model has no `est` or
  /// horizon, so `est` is 0 for every candidate and the key is `(tier, stale, recheck, review, h,
  /// id, kind)`, with `h` as `(hi, lo)`. `stale` (1.8, §10.4): 1 for a node flagged `stale`, so that
  /// it comes last in its tier, 0 otherwise. `recheck` (1.7): 0 for a node whose `{external}` wait
  /// has resolved at its `recheck_after`, so that it comes first among the rest, 1 otherwise.
  /// `review` (1.10): 0 for a review item, the item with no hold on a closed node, so that the
  /// reviews of a tier come next, by the ring and not by age (a review's `aged` is its priority
  /// alone, 0 here), 1 otherwise. `kind`: 0 node, 1 status, 2 route. 1.11 (§10.4): `request` 0
  /// for a candidate flagged `unanswered_request`, or a routing item, 1 otherwise, after `stale`;
  /// `needs`, after `recheck`, the number of the node's needs, at most 15, negated, so that work
  /// that needs more of the caller's caps comes first; and a review item's tier is its node's
  /// without `objection`. The key is `(tier, stale, request, recheck, needs, review, h, id, kind)`.
  type Cand = { tier: int, stale: int, request: int, recheck: int, needs: int, review: int, hi: int, lo: int, id: int, kind: int }

  pure val NO_CAND: Cand = { tier: -1, stale: 0, request: 1, recheck: 1, needs: 0, review: 1, hi: 0, lo: 0, id: -1, kind: -1 }

  pure def cand_lt(a: Cand, b: Cand): bool =
    a.tier < b.tier or (a.tier == b.tier and (
      a.stale < b.stale or (a.stale == b.stale and (
        a.request < b.request or (a.request == b.request and (
          a.recheck < b.recheck or (a.recheck == b.recheck and (
            a.needs < b.needs or (a.needs == b.needs and (
              a.review < b.review or (a.review == b.review and (
                a.hi < b.hi or (a.hi == b.hi and (
                  a.lo < b.lo or (a.lo == b.lo and (
                    a.id < b.id or (a.id == b.id and a.kind < b.kind)))))))))))))))))

  /// `recheck` of a node candidate (1.7, §10.4): its wait is `{external}` and resolved at `now`.
  pure def recheck_of(s: State, n: int, k: int, now: int): int =
    match s.nodes.get(n).wait {
      | HasWait(w) => match w.on {
          | WExternal(t) => if (k == 0 and t <= now) 0 else 1
          | _ => 1
        }
      | NoWait => 1
    }

  pure def better(c: Cand, best: Cand): Cand = if (best == NO_CAND or cand_lt(c, best)) c else best

  /// 1.12 (§10.4): `n` is the node `sess` last released as `handoff`, no hold of it was issued
  /// since (its fence is still the token's), and the release was less than `lease.first` ago.
  pure def handed_back(s: State, sess: int, n: int, now: int): bool = {
    val h = s.sessions.get(sess).handed_off
    h.node == n and now < h.until and s.nodes.get(n).fence == h.fence
  }

  /// 1.12 (§10.4, §11.3): the review of closed `n`, its first open objection, was raised by `sess`.
  pure def own_review(s: State, sess: int, n: int): bool = {
    val o = set_min(open_objections_on(s, n), -1)
    o >= 0 and s.entries.keys().contains(o) and s.entries.get(o).by == sess
  }

  /// The candidate `next` gives `sess` of class `cls` (an open `next`, D.10.4: no query), or
  /// NO_CAND: the minimum by key of the candidates nothing holds back and no caller exclusion
  /// removes (`refused`, `needs`, `own`; the model has no `est` or horizon). 1.12 (§10.4): nor what
  /// the caller put down, the node it handed off (`handed_back`) and the review of its own
  /// objection (`own_review`).
  pure def best_candidate(cfg: Cfg, s: State, sess: int, cls: str, now: int): Cand = {
    val caps = s.sessions.get(sess).caps
    val by_node = s.nodes.keys().fold(NO_CAND, (best, n) => {
      val fl = flags_of(s, n, now)
      val k = classify(s, n, fl, now)
      if (k < 0 or held_back(s, n, now) or excluded(s, n, cls, now) or not(s.nodes.get(n).needs.subseteq(caps))
          or (k == 0 and holds_node(s, sess, n))
          or (k == 0 and handed_back(s, sess, n, now))
          or (k == 1 and not(node_is_open(s.nodes.get(n))) and own_review(s, sess, n))) best
      else {
        val h = tie_key(cfg.use_fnv, sess, n)
        val review = k == 1 and not(node_is_open(s.nodes.get(n)))
        better({
          tier: tier_of(s, n, if (review) fl.exclude(Set(FObjection)) else fl),
          stale: if (fl.contains(FStale)) 1 else 0,
          request: if (fl.contains(FUnansweredRequest)) 0 else 1,
          recheck: recheck_of(s, n, k, now),
          needs: 0 - imin(s.nodes.get(n).needs.size(), 15),
          review: if (review) 0 else 1,
          hi: h._1, lo: h._2, id: n, kind: k,
        }, best)
      }
    })
    s.user_entries.keys().fold(by_node, (best, e) =>
      if (s.user_entries.get(e).routing != Unrouted) best
      else {
        val h = tie_key(cfg.use_fnv, sess, e)
        better({ tier: 0, stale: 0, request: 0, recheck: 1, needs: 0, review: 1, hi: h._1, lo: h._2, id: e, kind: 2 }, best)
      })
  }

  // ================================================================== validation (D.8.24)

  // session (§20.3) -----------------------------------------------------------------------------
  // `hello` is in `run_cmd`: it opens its session, and in the model nothing it validates can fail.

  // node (§20.5) --------------------------------------------------------------------------------

  /// The open carriers of `ks` (a join when non-empty, D.8.9).
  pure def carriers(s: State, ks: Set[Key]): Set[int] = ks.map(k => key_open(s, k)).exclude(Set(-1))

  pure def is_rep_set(r: RepeatChange): bool = match r { | RepSet(_) => true | _ => false }

  pure def ids_resolve(s: State, c: IdsChange): bool =
    match c { | IdsKeep => true | IdsSet(xs) => xs.forall(x => has_node(s, x)) }

  /// D.8.9: `repeat:` keys, user keys naming no user entry; then a join validates nothing else;
  /// a create checks the parent, the name, the owners, and L1 (depth, the first occurrence of a
  /// series included, then children). The model's titles are never empty, its digests never too
  /// long, and its schedules always fire (D.10.6).
  pure def v_create(s: State, a: { parent: int, name: Name, keys: Set[Key], take: bool, capacity: int,
                                   owners: Set[int], repeat: RepeatChange, tags: Set[str] }): Check =
    if (a.keys.exists(k => is_repeat_key(k))) bad(WhyReservedKey)
    else if (a.keys.exists(k => user_key_entry(k) >= 0 and not(s.user_entries.keys().contains(user_key_entry(k)))))
      nf(WhatUserEntry)
    else if (carriers(s, a.keys) != Set()) Pass
    else if (not(has_node(s, a.parent))) nf(WhatNode)
    else if (s.names.keys().contains((a.parent, a.name))) bad(WhyNameTaken)
    else if (not(a.owners.forall(o => has_node(s, o)))) nf(WhatNode)
    else if (depth(s, a.parent) + 1 > TREE_MAX_DEPTH or (is_rep_set(a.repeat) and depth(s, a.parent) + 2 > TREE_MAX_DEPTH))
      Fail(ErrLimit(LkDepth))
    else if (not(child_room(s, a.parent, -1))) Fail(ErrLimit(LkChildren))
    else Pass

  /// Adding the edges `n → d` for the new `depends_on` closes a cycle (T5).
  pure def depends_cycle(s: State, n: int, c: IdsChange): bool =
    match c {
      | IdsKeep => false
      | IdsSet(xs) => xs.exclude(s.nodes.get(n).depends_on).exists(d => d == n or reaches(s, d, n))
    }

  /// D.8.20: the node; `digest` without `digest_version`; the references; the versions; reserved
  /// nodes and roles; cycles; capacity below the live holds; L1 for a new occurrence. `repeat` on a
  /// reserved node is any change of it; `RoleConflict` is a `repeat` set on an occurrence or a
  /// signal (a clear of one changes nothing there).
  pure def v_update(s: State, a: { node: int, ver: int, dver: int, content: int, status: int, digest: int,
                                   capacity: int, needs_add: Set[str], needs_remove: Set[str],
                                   tags_add: Set[str], tags_remove: Set[str], due: DueChange,
                                   depends: IdsChange, owners: IdsChange, repeat: RepeatChange }): Check =
    if (not(has_node(s, a.node))) nf(WhatNode)
    else if (a.digest >= 0 and a.dver < 0) bad(WhyMissingDigestVersion)
    else if (not(ids_resolve(s, a.depends)) or not(ids_resolve(s, a.owners))) nf(WhatNode)
    else {
      val nd = s.nodes.get(a.node)
      if (a.ver != nd.version or (a.dver >= 0 and a.dver != nd.digest_version)) Fail(ErrStaleVersion)
      else if (is_reserved(s, a.node) and (a.capacity >= 0 or a.repeat != RepKeep)) bad(WhyReservedNode)
      else if (is_rep_set(a.repeat) and (role_is_occurrence(nd.role) or role_is_signal(nd.role))) bad(WhyRoleConflict)
      else if (depends_cycle(s, a.node, a.depends)) Fail(ErrCycle)
      else if (a.capacity >= 0 and a.capacity < holds_on(s, a.node).size()) Fail(ErrFull)
      else if (is_rep_set(a.repeat) and node_is_open(nd) and depth(s, a.node) + 1 > TREE_MAX_DEPTH) Fail(ErrLimit(LkDepth))
      else Pass
    }

  pure def v_link(s: State, a: { node: int, rel: LinkRel, to: int }): Check =
    if (not(has_node(s, a.node)) or not(has_node(s, a.to))) nf(WhatNode)
    else if (a.node == a.to) bad(WhySelfLink)
    else Pass

  /// `node key` (D.8.9, D.8.24): `repeat:` keys and reserved nodes are refused for an addition and
  /// a removal alike; a user key must name a user entry (addition); a removal needs the key.
  pure def v_key(s: State, n: int, k: Key, add: bool): Check =
    if (not(has_node(s, n))) nf(WhatNode)
    else if (is_repeat_key(k)) bad(WhyReservedKey)
    else if (is_reserved(s, n)) bad(WhyReservedNode)
    else if (add and user_key_entry(k) >= 0 and not(s.user_entries.keys().contains(user_key_entry(k)))) nf(WhatUserEntry)
    else if (not(add) and not(s.nodes.get(n).keys.contains(k))) nf(WhatKey)
    else Pass

  pure def v_close(s: State, now: int, a: { node: int, outcome: Outcome, settles: Set[int], token: OptToken }): Check =
    if (not(has_node(s, a.node))) nf(WhatNode)
    else if (match a.token { | NoToken => false | SomeToken(t) => not(token_live(s, t, a.node, now)) }) Fail(ErrStaleToken)
    else if (a.outcome == Missed) bad(WhyServerOutcome)
    else if (not(a.settles.forall(r => s.requests.keys().contains(r)))) nf(WhatRequest)
    else if (is_reserved(s, a.node)) bad(WhyReservedNode)
    else if (is_closed(s, a.node)) bad(WhyAlreadyClosed)
    else if (occ_waiting(s.nodes.get(a.node)) and a.outcome != Dropped) bad(WhyOccurrenceNotYetAvailable)
    else Pass

  pure def v_reopen(s: State, n: int): Check =
    if (not(has_node(s, n))) nf(WhatNode)
    else if (is_open(s, n)) bad(WhyNotClosed)
    else if (occ_locked(s, n)) bad(WhyOccurrenceReopen)
    else Pass

  /// D.8.20: a move onto the node's own place is a no-op success. ReservedNode comes before the new
  /// parent resolves, as D.8.20 lists them (README, "Open questions", item 10).
  pure def v_move(s: State, a: { node: int, parent: int, name: Name }): Check =
    if (not(has_node(s, a.node))) nf(WhatNode)
    else if (is_reserved(s, a.node)) bad(WhyReservedNode)
    else if (not(has_node(s, a.parent))) nf(WhatNode)
    else if (s.nodes.get(a.node).parent == a.parent and s.nodes.get(a.node).name == a.name) Pass
    else if (is_under(s, a.parent, a.node)) Fail(ErrCycle)
    else if (s.names.keys().contains((a.parent, a.name))) bad(WhyNameTaken)
    else if (depth(s, a.parent) + 1 + height(s, a.node) > TREE_MAX_DEPTH) Fail(ErrLimit(LkDepth))
    else if (is_open(s, a.node) and is_agent_made(s.nodes.get(a.node)) and not(child_room(s, a.parent, a.node)))
      Fail(ErrLimit(LkChildren))
    else Pass

  /// D.8.20's order. `into` the open occurrence of `from`, a series, is `invalid` (1.5): closing
  /// `from` would drop that occurrence, and with it the keys and marks the merge moved there (N5).
  pure def v_merge(s: State, a: { from: int, into: int }): Check =
    if (not(has_node(s, a.from)) or not(has_node(s, a.into))) nf(WhatNode)
    else if (a.from == a.into) bad(WhySelfMerge)
    else if (is_reserved(s, a.from) or is_reserved(s, a.into)) bad(WhyReservedNode)
    else if (is_closed(s, a.into)) Fail(ErrClosed)
    else if (is_closed(s, a.from)) bad(WhyMergeFromClosed)
    else if (occ_waiting(s.nodes.get(a.from))) bad(WhyOccurrenceNotYetAvailable)
    else if (is_series(s, a.from) and open_occurrences(s, a.from).contains(a.into))
      bad(WhyMergeIntoOwnOccurrence)
    else if (merge_cycle(s, a.from, a.into)) Fail(ErrCycle)
    else Pass

  /// `node hide {entry}`: the entry must exist. The shell adds its node from its `entries` table
  /// (D.5.2); the model knows the node of a kept entry only, and its actions name only those (or an
  /// eid that does not exist yet).
  pure def v_hide(s: State, e: int): Check =
    if (e >= s.next_eid or not(s.entries.keys().contains(e))) nf(WhatEntry) else Pass

  // hold (§20.6) --------------------------------------------------------------------------------

  /// D.8.10: an existing hold of the session succeeds at once (§10.1). A closed node with no flag,
  /// merged, marked upheld (1.8), or with an open objection (1.9: its review is a resolve) is
  /// `closed`.
  pure def v_take(s: State, sess: int, now: int, n: int): Check =
    if (not(has_node(s, n))) nf(WhatNode)
    else if (holds_node(s, sess, n)) Pass
    else {
      val nd = s.nodes.get(n)
      val fl = flags_of(s, n, now)
      if (not(node_is_open(nd)) and (fl == Set() or merged(s, n) or upheld(s, n) or fl.contains(FObjection))) Fail(ErrClosed)
      else if (not(node_is_open(nd)) and occ_locked(s, n)) bad(WhyOccurrenceReopen)
      else if (node_is_open(nd) and occ_waiting(nd)) bad(WhyOccurrenceNotYetAvailable)
      else if (paused(s, n, now)) Fail(ErrPaused)
      else if (live_holds_of(s, sess, now).size() >= HOLD_PER_SESSION) Fail(ErrHoldsLimit)
      else if (holds_on(s, n).size() >= nd.capacity) Fail(ErrFull)
      else Pass
    }

  pure def v_next(s: State, sess: int, now: int): Check =
    if (live_holds_of(s, sess, now).size() >= HOLD_PER_SESSION) Fail(ErrHoldsLimit) else Pass

  /// The token's node exists, then the token is a live hold on it (H4).
  pure def v_token(s: State, t: (int, int), now: int): Check =
    if (not(has_node(s, t._1))) nf(WhatNode)
    else if (not(token_live(s, t, t._1, now))) Fail(ErrStaleToken)
    else Pass

  pure def v_checkpoint(s: State, now: int, a: { token: (int, int), intent: bool, intent_done: bool }): Check =
    match v_token(s, a.token, now) {
      | Fail(e) => Fail(e)
      | Pass =>
          val open_intent = s.nodes.get(a.token._1).open_intent.eid >= 0
          if (a.intent and open_intent and not(a.intent_done)) bad(WhyIntentOpen)
          else if (a.intent_done and not(open_intent)) bad(WhyNoIntent)
          else Pass
    }

  pure def v_renew(s: State, now: int, t: (int, int)): Check =
    match v_token(s, t, now) {
      | Fail(e) => Fail(e)
      | Pass =>
          val h = s.holds.get(t)
          if (h.renews + 1 > h.terms.renew_max) Fail(ErrLimit(LkRenew)) else Pass
    }

  /// 1.12 (§10.3): each request of `settles` exists (`not_found(Request)`), after the token.
  pure def v_release(s: State, now: int, a: { token: (int, int), reason: ReleaseReason, wait: WaitArg, settles: Set[int] }): Check =
    match v_token(s, a.token, now) {
      | Fail(e) => Fail(e)
      | Pass =>
          if (not(a.settles.forall(r => s.requests.keys().contains(r)))) nf(WhatRequest)
          else match a.wait {
            | WaQuestion(q) =>
                if (s.pquestions.keys().contains(q) or s.uquestions.keys().contains(q)) Pass else nf(WhatQuestion)
            | WaNode(t) =>
                if (not(has_node(s, t))) nf(WhatNode)
                else if (t == a.token._1 or reaches(s, t, a.token._1)) Fail(ErrCycle)
                else Pass
            | _ => Pass
          }
    }

  // talk: the user (§20.7, D.8.11, D.8.12) --------------------------------------------------------

  /// D.8.11's order, argument by argument (README, "Open questions", item 10). 1.9 (§8.1): in a
  /// session that serves no principal, a `from_user` names one, `user` among them, or is
  /// `invalid(NoPrincipal)`, where 1.6 to 1.8 filed it for `user`.
  pure def v_from_user(s: State, sess: int, a: { principal: str, answers: int, clarifies: int, node: int }): Check =
    if (a.answers >= 0 and not(is_question(s, a.answers))) nf(WhatQuestion)
    else if (a.answers >= 0 and not(s.uquestions.keys().contains(a.answers))) bad(WhyNotUserQuestion)
    else if (a.clarifies >= 0 and not(s.requests.keys().contains(a.clarifies))) nf(WhatRequest)
    else if (a.node >= 0 and not(has_node(s, a.node))) nf(WhatNode)
    else if (a.node >= 0 and is_reserved(s, a.node)) bad(WhyReservedNode)
    else if (a.principal == "" and s.sessions.get(sess).principals == Set()) bad(WhyNoPrincipal)
    else if (principal_choice(s, sess, a.principal) == PcAmbiguous) bad(WhyAmbiguousPrincipal)
    else Pass

  /// A `re` id must name a request, a `from_user` entry, or a digest entry (§20.7).
  pure def reportable(s: State, x: int): bool =
    s.requests.keys().contains(x) or s.user_entries.keys().contains(x) or s.digest.keys().contains(x)

  pure def v_to_user(s: State, sess: int, a: { principal: str, kind: ToUserKind, re: Set[int] }): Check =
    if (a.re.exists(x => x >= s.next_eid or x < 1)) nf(WhatEntry)
    else if (a.re.exists(x => not(reportable(s, x)))) bad(WhyNotReportable)
    else if (principal_choice(s, sess, a.principal) == PcAmbiguous) bad(WhyAmbiguousPrincipal)
    else Pass

  pure def v_route(s: State, a: { entry: int, to: int }): Check =
    if (a.entry >= s.next_eid or a.entry < 1) nf(WhatUserEntry)
    else if (not(s.user_entries.keys().contains(a.entry))) bad(WhyNotUserEntry)
    else if (a.to >= 0 and not(has_node(s, a.to))) nf(WhatNode)
    else if (a.to >= 0 and is_reserved(s, a.to)) bad(WhyReservedNode)
    else Pass

  pure def v_ask_user(s: State, at: int): Check = if (not(has_node(s, at))) nf(WhatNode) else Pass

  // talk: people (§20.7, D.8.15) ------------------------------------------------------------------

  pure def opt_key_is_repeat(k: OptKey): bool = match k { | SomeKey(x) => is_repeat_key(x) | NoKey => false }

  /// The conversation key of a request: `where`, else the origin (A-61).
  pure def request_conv(a: { at: int, origin: Key, where_key: OptKey }): Key =
    match a.where_key { | SomeKey(k) => k | NoKey => a.origin }

  /// D.8.15: a repeat of one message is answered before the reserved check (Q1).
  pure def v_request(s: State, a: { at: int, origin: Key, where_key: OptKey }): Check =
    if (not(has_node(s, a.at))) nf(WhatNode)
    else if (is_repeat_key(a.origin) or opt_key_is_repeat(a.where_key)) bad(WhyReservedKey)
    else if (request_by_origin(s, a.origin) >= 0) Pass
    else if (is_reserved(s, conv_node(s, a.at, request_conv(a)).node)) bad(WhyReservedNode)
    else Pass

  /// The conversation of `said`, `heard`, and `ask` to a person: the key (`where`, or the default
  /// of §9.1), and the conversation node with its case. D.10.4 gives `said` a `where` and `heard`
  /// and `ask` none, so these two always take the default (README, "Open questions", item 11).
  pure def conv_of(s: State, at: int, where_key: OptKey): { key: Key, node: int, case: int } =
    match conv_key(s, at, where_key) {
      | CkOk(k) => { val cn = conv_node(s, at, k) { key: k, node: cn.node, case: cn.case } }
      | CkErr(_) => { key: Conv(""), node: -1, case: 0 }
    }

  /// `at` resolves; the conversation key exists and is unique; a `repeat:` `where` is refused; the
  /// conversation node is not reserved (D.8.8, D.8.15, D.8.24).
  pure def v_conversation(s: State, at: int, where_key: OptKey): Check =
    if (not(has_node(s, at))) nf(WhatNode)
    else match conv_key(s, at, where_key) {
      | CkErr(w) => bad(w)
      | CkOk(k) =>
          if (is_repeat_key(k)) bad(WhyReservedKey)
          else if (is_reserved(s, conv_node(s, at, k).node)) bad(WhyReservedNode)
          else Pass
    }

  pure def v_said(s: State, a: { at: int, kind: SaidKind, where_key: OptKey }): Check =
    if (not(has_node(s, a.at))) nf(WhatNode)
    else if (a.kind == SQuestion) bad(WhySaidKindQuestion)
    else v_conversation(s, a.at, a.where_key)

  pure def v_heard(s: State, a: { at: int, answers: int }): Check =
    match v_conversation(s, a.at, NoKey) {
      | Fail(e) => Fail(e)
      | Pass =>
          if (a.answers >= 0 and not(is_question(s, a.answers))) nf(WhatQuestion)
          else if (a.answers >= 0 and not(s.pquestions.keys().contains(a.answers))) bad(WhyNotPeopleQuestion)
          else Pass
    }

  // talk: agents (§20.7, D.8.16) ------------------------------------------------------------------

  pure def v_agent_to(s: State, a: AgentTo): Check =
    match a {
      | AgSession(x) => if (has_session(s, x)) Pass else nf(WhatAddressee)
      | AgHolder(n) => if (has_node(s, n)) Pass else nf(WhatNode)
      | AgHolders(n) => if (has_node(s, n)) Pass else nf(WhatNode)
    }

  pure def v_ask_agent(s: State, a: { at: int, to: AgentTo }): Check =
    if (not(has_node(s, a.at))) nf(WhatNode) else v_agent_to(s, a.to)

  /// `talk answer`: the shell adds the question's node and asker, or answers `not_found(Question)`;
  /// any entry that is not an agent question is `invalid(NotAgentQuestion)` (A-36).
  pure def v_answer(s: State, q: int): Check =
    if (q >= s.next_eid or q < 1) nf(WhatQuestion)
    else if (not(is_agent_question(s, q))) bad(WhyNotAgentQuestion)
    else Pass

  pure def v_say(s: State, a: { at: int, to: SayTo }): Check =
    if (not(has_node(s, a.at))) nf(WhatNode)
    else match a.to {
      | SaySession(x) => if (has_session(s, x)) Pass else nf(WhatAddressee)
      | SayHolder(n) => if (has_node(s, n)) Pass else nf(WhatNode)
      | SayHolders(n) => if (has_node(s, n)) Pass else nf(WhatNode)
      | _ => Pass
    }

  pure def v_wait(s: State, n: int): Check = if (n >= 0 and not(has_node(s, n))) nf(WhatNode) else Pass

  // talk: review (§20.7, D.8.17) ------------------------------------------------------------------

  pure def basis_missing(s: State, b: BasisArg): Check =
    match b {
      | BaNode(x) => if (has_node(s, x.id)) Pass else nf(WhatNode)
      | BaEntry(e) => if (e >= 1 and e < s.next_eid) Pass else nf(WhatEntry)
    }

  /// D.8.17: settles, supersedes, the basis references (resolution), then the states: an active
  /// superseded decision, no version ahead, and no reference to `at` itself (as the reference
  /// resolves through `merged_into`, A-88).
  pure def v_decide(s: State, a: { at: int, settles: Set[int], supersedes: int, basis: List[BasisArg] }): Check =
    if (not(has_node(s, a.at))) nf(WhatNode)
    else if (not(a.settles.forall(r => s.requests.keys().contains(r)))) nf(WhatRequest)
    else if (a.supersedes >= 0 and not(s.decisions.keys().contains(a.supersedes))) nf(WhatDecision)
    else {
      val missing = a.basis.foldl(Pass, (acc, b) => if (acc != Pass) acc else basis_missing(s, b))
      if (missing != Pass) missing
      else if (a.supersedes >= 0 and s.decisions.get(a.supersedes).state != DActive) bad(WhyDecisionNotActive)
      else if (any_of(a.basis, b => match b { | BaNode(x) => x.version > s.nodes.get(x.id).version | _ => false }))
        bad(WhyVersionAhead)
      else if (any_of(a.basis, b => match b { | BaNode(x) => resolve_merged(s, x.id) == a.at | _ => false }))
        bad(WhySelfLink)
      else Pass
    }

  /// `talk retract`: a node resolves, or an entry exists with its node (D.8.17); as for `hide`, the
  /// model knows the node of a kept entry only.
  pure def v_retract(s: State, t: BasisTarget): Check =
    match t {
      | TNode(n) => if (has_node(s, n)) Pass else nf(WhatNode)
      | TEntry(e) => if (e < s.next_eid and s.entries.keys().contains(e)) Pass else nf(WhatEntry)
    }

  pure def v_approve(s: State, a: { node: int, ver: int }): Check =
    if (not(has_node(s, a.node))) nf(WhatNode)
    else if (a.ver > s.nodes.get(a.node).version) bad(WhyVersionAhead)
    else Pass

  pure def v_node(s: State, n: int): Check = if (has_node(s, n)) Pass else nf(WhatNode)

  /// 1.11 (§11.3): `defer` with `upheld` is `invalid(BadParams)`, after the objection resolves.
  pure def v_resolve(s: State, a: { objection: int, upheld: bool, defer: bool }): Check =
    if (not(s.objections.keys().contains(a.objection))) nf(WhatObjection)
    else if (a.defer and a.upheld) bad(WhyBadParams)
    else Pass

  /// `hold check {target: n, intent_done}` (1.8, §10.2): the node exists, then it has an open
  /// intent, whoever opened it; no hold is needed.
  pure def v_check_intent(s: State, n: int): Check =
    if (not(has_node(s, n))) nf(WhatNode)
    else if (s.nodes.get(n).open_intent.eid < 0) bad(WhyNoIntent)
    else Pass

  pure def v_confirm(s: State, x: int): Check = if (s.saids.keys().contains(x)) Pass else nf(WhatSaid)

  // control (§20.8) ------------------------------------------------------------------------------

  pure def v_unpause(s: State, n: int): Check =
    if (not(has_node(s, n))) nf(WhatNode)
    else if (not(s.pauses.keys().contains(n))) nf(WhatPause)
    else Pass

  /// `validate(s, sess, now, cmd)` (D.8.1 step 4) for every command but `hello`.
  pure def validate(s: State, sess: int, now: int, cmd: Cmd): Check =
    match cmd {
      | CCreate(a) => v_create(s, a)
      | CUpdate(a) => v_update(s, a)
      | CLink(a) => v_link(s, a)
      | CKeyAdd(a) => v_key(s, a.node, a.key, true)
      | CKeyRemove(a) => v_key(s, a.node, a.key, false)
      | CClose(a) => v_close(s, now, a)
      | CReopen(n) => v_reopen(s, n)
      | CMove(a) => v_move(s, a)
      | CMerge(a) => v_merge(s, a)
      | CTake(a) => v_take(s, sess, now, a.node)
      | CNext => v_next(s, sess, now)
      | CCheckpoint(a) => v_checkpoint(s, now, a)
      | CRenew(t) => v_renew(s, now, t)
      | CRelease(a) => v_release(s, now, a)
      | CFromUser(a) => v_from_user(s, sess, a)
      | CToUser(a) => v_to_user(s, sess, a)
      | CRoute(a) => v_route(s, a)
      | CRequest(a) => v_request(s, a)
      | CSaid(a) => v_said(s, a)
      | CConfirm(x) => v_confirm(s, x)
      | CHeard(a) => v_heard(s, a)
      | CAskPerson(a) => v_conversation(s, a.at, NoKey)
      | CAskUser(a) => v_ask_user(s, a.at)
      | CAskAgent(a) => v_ask_agent(s, a)
      | CAnswer(q) => v_answer(s, q)
      | CDecide(a) => v_decide(s, a)
      | CRetract(t) => v_retract(s, t)
      | CApprove(a) => v_approve(s, a)
      | CObject(n) => v_node(s, n)
      | CResolve(a) => v_resolve(s, a)
      | CCheckIntent(n) => v_check_intent(s, n)
      | CSay(a) => v_say(s, a)
      | CWait(n) => v_wait(s, n)
      | CHide(e) => v_hide(s, e)
      | CPause(n) => v_node(s, n)
      | CUnpause(n) => v_unpause(s, n)
      | CView(n) => v_node(s, n)
      | _ => bad(WhyBadParams)
    }

  /// The node a stale token's `late` entry goes on (H4): the token's node for `checkpoint`,
  /// `renew`, and `release`, the target of `close` (the model's `update` carries no token).
  pure def late_node(cmd: Cmd): int =
    match cmd {
      | CCheckpoint(a) => a.token._1
      | CRenew(t) => t._1
      | CRelease(a) => a.token._1
      | CClose(a) => a.node
      | _ => -1
    }

  // ================================================================== execution (D.8.9 onwards)

  // session (§20.3, D.8.23) ---------------------------------------------------------------------

  /// `session hello {caps?, principal?}` on a new session (D.5.1, D.8.23): `PrincipalSeen` for a
  /// new principal, `SessionOpened` (its caps are `hello`'s; tokens carry no default caps in the
  /// model), and `InboxRead` of the notes to its principal. Reply ids: `[session]`.
  pure def hello_ex(s: State, cls: str, now: int, a: { caps: Set[str], principal: str }): Ex = {
    val pi = if (a.principal != "") intern(s, a.principal) else { st: s, id: -1 }
    val x = pi.st.next_session
    val s1 = {
      ...pi.st,
      next_session: x + 1,
      sessions: pi.st.sessions.put(x, {
        class: cls, started_at: now, last_seen: now, silence_at: now + PRESENCE_SILENCE,
        retire_at: now + SESSION_RETAIN, silent: false, caps: a.caps, horizon: -1,
        principals: if (pi.id >= 0) Set(pi.id) else Set(), shown: Map(), branch_seen: Map(),
        handed_off: NO_HANDOFF,
      }),
    }
    ex(read_inbox(s1, x, now).st, [x])
  }

  // node (§20.5) --------------------------------------------------------------------------------

  /// `node create` (D.8.9). Join: the keys no open node carries go to the open carrier with the
  /// smallest id, its user keys are routed, and a duplicate signal opens for every other carrier;
  /// no hold. Create: the node (the parent's `needs`, CS-26; `continues` to the newest closed
  /// carrier of each key, A-117), the series' first occurrence, the routing of its user keys, and
  /// the hold, or the `offer` tag when `take` cannot issue one (`no_hold`), then `Shown`. Reply ids:
  /// `[id, fence]` (fence -1 without a hold) for a create, `[existing]` for a join.
  pure def create_ex(s: State, sess: int, now: int, a: { parent: int, name: Name, keys: Set[Key],
                     take: bool, capacity: int, owners: Set[int], repeat: RepeatChange, tags: Set[str] }): Ex = {
    val ks = sorted_keys(a.keys)
    val s0 = intern_person_keys(s, ks)
    val cs = carriers(s0, a.keys)
    if (cs != Set()) {
      val c = set_min(cs, -1)
      val added = ks.select(k => key_open(s0, k) < 0)
      val s1 = added.foldl(s0, (st, k) => add_key(st, c, k, now))
      val s2 = route_user_keys(s1, added, c, sess, now)
      val s3 = sorted(cs.exclude(Set(c))).foldl(s2, (st, o) => open_signal(st, SigDuplicate, c, o, now).st)
      // An event was emitted iff the state changed: a `PrincipalSeen`, a `KeyAdded`, a routing, or
      // a new signal; the open signal of a pair that has one emits nothing (D.8.19), so a join that
      // only meets existing signals appends nothing and records no `op_id` (D.8.1 step 6, O1).
      ex_if(s3, [c], s3 != s)
    } else {
      val id = s0.next_node
      val preds = ks.foldl(Set(), (acc, k) => {
        val carried = s0.nodes.keys().filter(m => s0.nodes.get(m).keys.contains(k))
        if (carried == Set()) acc else acc.union(Set(set_max(carried, -1)))
      })
      val nd: Node = {
        ...reserved_node(a.parent, "", now),
        name: a.name,
        origin: OAgent(sess),
        content: 1,
        tags: a.tags,
        capacity: a.capacity,
        needs: s0.nodes.get(a.parent).needs,
        owners: a.owners,
        keys: a.keys,
        links: preds.map(p => (Continues, p)),
        role: match a.repeat { | RepSet(r) => Series({ repeat: r, last_missed: false }) | _ => Plain },
      }
      val s1 = ev_node_created(s0, nd, now)
      val s2 = if (is_rep_set(a.repeat)) open_next_occurrence(s1, id, now, now) else s1
      val s3 = route_user_keys(s2, ks, id, sess, now)
      val can_hold = a.take and a.capacity >= 1 and not(paused(s3, id, now))
                     and live_holds_of(s3, sess, now).size() < HOLD_PER_SESSION
      val s4 =
        if (can_hold) bind_holder_notes(issue_hold(s3, id, sess, -1, now), id, sess, now)
        else if (a.take and a.capacity >= 1) upd_node(s3, id, x => { ...x, tags: x.tags.union(Set(OFFER)) })
        else s3
      { st: shown(s4, sess, id), ids: [id, if (can_hold) 1 else -1], ev: true, hold: can_hold }
    }
  }

  /// `node update` (D.8.20): `NodeUpdated` with the fields whose value differs (none: no event),
  /// with the counters of O2 (CS-34); on an open node the occurrence consequences of D.8.18 (a
  /// waiting occurrence dropped and reslotted, or dropped or detached when `repeat` is cleared, or
  /// the first occurrence when `repeat` is added); `e1_check` when `needs` changed on a refused
  /// node. Reply ids: `[version, digest_version]`.
  pure def update_ex(s: State, sess: int, now: int, a: { node: int, ver: int, dver: int, content: int,
                     status: int, digest: int, capacity: int, needs_add: Set[str], needs_remove: Set[str],
                     tags_add: Set[str], tags_remove: Set[str], due: DueChange, depends: IdsChange,
                     owners: IdsChange, repeat: RepeatChange }): Ex = {
    val n = a.node
    val nd = s.nodes.get(n)
    val content_ch = a.content >= 0 and a.content != nd.content
    val status_ch = a.status >= 0 and a.status != nd.status
    val digest_ch = a.digest >= 0 and a.digest != nd.digest
    val capacity2 = if (a.capacity >= 0) a.capacity else nd.capacity
    val tags2 = nd.tags.exclude(a.tags_remove).union(a.tags_add)
    val needs2 = nd.needs.exclude(a.needs_remove).union(a.needs_add)
    val due2 = match a.due { | DueKeep => nd.due | DueClear => -1 | DueSet(t) => t }
    val deps2 = match a.depends { | IdsKeep => nd.depends_on | IdsSet(xs) => xs }
    val owners2 = match a.owners { | IdsKeep => nd.owners | IdsSet(xs) => xs }
    val was_series = role_is_series(nd.role)
    val role2 = match a.repeat {
      | RepKeep => nd.role
      | RepClear => if (was_series) Plain else nd.role
      | RepSet(r) => match nd.role {
          | Series(si) => Series({ ...si, repeat: r })
          | Plain => Series({ repeat: r, last_missed: false })
          | _ => nd.role
        }
    }
    val repeat_ch = role2 != nd.role
    val changed = content_ch or status_ch or digest_ch or capacity2 != nd.capacity or tags2 != nd.tags
                  or needs2 != nd.needs or due2 != nd.due or deps2 != nd.depends_on or owners2 != nd.owners
                  or repeat_ch
    val v2 = if (content_ch or status_ch) nd.version + 1 else nd.version
    val s1 =
      if (not(changed)) s
      else upd_node(s, n, x => {
        ...x,
        content: if (content_ch) a.content else x.content,
        status: if (status_ch) a.status else x.status,
        digest: if (digest_ch) a.digest else x.digest,
        capacity: capacity2,
        tags: tags2,
        needs: needs2,
        due: due2,
        due_fired: if (due2 != x.due) due2 >= 0 and due2 <= now else x.due_fired,
        depends_on: deps2,
        owners: owners2,
        role: role2,
        version: v2,
        content_version: if (content_ch) v2 else x.content_version,
        digest_version: if (digest_ch) x.digest_version + 1 else x.digest_version,
        touched: now,
        stale_fired: false,
      })
    val occ = open_occurrence(s1, n)
    val s2 =
      if (not(node_is_open(nd)) or not(repeat_ch)) s1
      else if (was_series and is_series(s1, n)) {
        if (occ >= 0 and occ_waiting(s1.nodes.get(occ))) close_node(s1, occ, Dropped, sess, -1, CauseClose, now, now)
        else s1
      }
      else if (was_series) {
        if (occ < 0) s1
        else if (occ_waiting(s1.nodes.get(occ))) close_node(s1, occ, Dropped, sess, -1, CauseClose, -1, now)
        else upd_node(s1, occ, x => { ...x, role: Plain })
      }
      else open_next_occurrence(s1, n, now, now)
    val s3 = if (needs2 != nd.needs and refused_nodes(s2).contains(n)) e1_check(s2, n, sess, now) else s2
    ex_if(s3, [s3.nodes.get(n).version, s3.nodes.get(n).digest_version], changed)
  }

  /// `node link` (D.8.20): an identical link changes nothing; otherwise `LinkAdded` (version + 1)
  /// and a signal for `duplicate` and `conflicts_with`, or the pair's signals resolved for
  /// `superseded_by`. Reply ids: `[signal]` (-1 for none).
  pure def link_ex(s: State, now: int, a: { node: int, rel: LinkRel, to: int }): Ex =
    if (s.nodes.get(a.node).links.contains((a.rel, a.to))) ex_if(s, [-1], false)
    else {
      val s1 = upd_node(s, a.node, nd => {
        ...nd, links: nd.links.union(Set((a.rel, a.to))), version: nd.version + 1, touched: now, stale_fired: false
      })
      if (a.rel == Duplicate) { val r = open_signal(s1, SigDuplicate, a.node, a.to, now) ex(r.st, [r.id]) }
      else if (a.rel == ConflictsWith) { val r = open_signal(s1, SigConflict, a.node, a.to, now) ex(r.st, [r.id]) }
      else if (a.rel == SupersededBy) ex(resolve_signals(s1, a.node, a.to, now), [-1])
      else ex(s1, [-1])
    }

  /// `node key {add}` (D.8.9): a key another open node carries is not added and opens a duplicate
  /// signal; a key the node carries changes nothing; otherwise `PrincipalSeen` for a new `person:`
  /// key, `KeyAdded`, and the routing of an unrouted user key. Reply ids: `[signal]` (-1 for none).
  pure def key_add_ex(s: State, sess: int, now: int, n: int, k: Key): Ex = {
    val o = key_open(s, k)
    if (o >= 0 and o != n) {
      val r = open_signal(s, SigDuplicate, n, o, now)
      ex_if(r.st, [r.id], r.st.next_node != s.next_node)
    }
    else if (s.nodes.get(n).keys.contains(k)) ex_if(s, [-1], false)
    else {
      val s1 = add_key(intern_person_keys(s, [k]), n, k, now)
      ex(route_user_keys(s1, [k], n, sess, now), [-1])
    }
  }

  /// `node close` (D.8.20): with `settles`, a decision on the node that records its assumptions
  /// and settles; the session's pending `said` entries confirmed; `close_node`. Reply ids:
  /// `[decision]` (-1 for none).
  pure def close_ex(s: State, sess: int, now: int, a: { node: int, outcome: Outcome, settles: Set[int], token: OptToken }): Ex = {
    val d = if (a.settles != Set()) s.next_eid else -1
    val s1 =
      if (a.settles == Set()) s
      else sorted(a.settles).foldl(record_assumed(append_decision(s, a.node, now, sess, a.settles), a.node, d),
                                   (st, r) => try_settle(st, r))
    val s2 = confirm_saids(s1, sess)
    ex(close_node(s2, a.node, a.outcome, sess, -1, CauseClose, -1, now), [d])
  }

  /// `node move` (D.8.20): `NodeMoved` (the old place becomes an alias), then the holds of other
  /// sessions end (`EndedByClose(Move)`, judged at the new place, A-113), the system entry, and the
  /// missed check of an occurrence. Reply ids: `[]`.
  pure def move_ex(s: State, sess: int, now: int, a: { node: int, parent: int, name: Name }): Ex = {
    val nd = s.nodes.get(a.node)
    if (nd.parent == a.parent and nd.name == a.name) ex_if(s, [], false)
    else {
      val old = (nd.parent, nd.name)
      val place = (a.parent, a.name)
      val s1 = {
        ...upd_node(s, a.node, x => { ...x, parent: a.parent, name: a.name }),
        names: map_remove(s.names, old).put(place, a.node),
        aliases: map_remove(s.aliases, place).put(old, a.node),
      }
      val ended = holds_on(s1, a.node).filter(t => s1.holds.get(t).session != sess)
      val s2 = ended.fold(s1, (st, t) => ev_hold_ended(st, t, HeEndedByClose(CauseMove)))
      val s3 = append_entry(s2, a.node, now, sess, ESystem, -1, false)
      ex(if (ended != Set() and is_occurrence(s3, a.node)) maybe_missed(s3, a.node, now) else s3, [])
    }
  }

  /// `node merge` (D.8.20, N5): the keys move; the open requests, people questions, commitments,
  /// objections, and open `said` entries move with `moved_from`, the marks move, and an intent that
  /// is not the merger's moves if `into` has none; a moved request from people or agents still
  /// unanswered is answered by the first qualifying `said` on `into` after it in its own
  /// conversation (A-95; §9.4, 1.3); the system entry; the waits and
  /// dependencies on `from` are redirected; `close_node(from, superseded)`; the pair's signals are
  /// resolved. Reply ids: `[]`.
  pure def merge_ex(s: State, sess: int, now: int, a: { from: int, into: int }): Ex = {
    val fm = a.from
    val i = a.into
    val s1 = sorted_keys(s.nodes.get(fm).keys).foldl(s, (st, k) => add_key(remove_key(st, fm, k, now), i, k, now))
    val moved = s1.requests.keys().filter(r => s1.requests.get(r).node == fm and is_unsettled(s1.requests.get(r)))
    val s2 = {
      ...s1,
      requests: s1.requests.keys().mapBy(r => {
        val x = s1.requests.get(r)
        if (moved.contains(r)) { ...x, node: i, moved_from: fm } else x
      }),
      pquestions: s1.pquestions.keys().mapBy(q => {
        val x = s1.pquestions.get(q)
        if (x.node == fm and pq_answerable(x.state)) { ...x, node: i, moved_from: fm } else x
      }),
      commitments: s1.commitments.keys().mapBy(c => {
        val x = s1.commitments.get(c)
        if (x.node == fm and x.state == CmOpen) { ...x, node: i, moved_from: fm } else x
      }),
      objections: s1.objections.keys().mapBy(o => {
        val x = s1.objections.get(o)
        if (x.node == fm and x.state == ObOpen) { ...x, node: i, moved_from: fm } else x
      }),
      saids: s1.saids.keys().mapBy(d => {
        val x = s1.saids.get(d)
        if (x.node == fm and (x.state == SdPending or x.state == SdUnconfirmed)) { ...x, node: i, moved_from: fm } else x
      }),
    }
    val from_nd = s2.nodes.get(fm)
    val s3 = upd_node(upd_node(s2, i, x => {
        ...x,
        recheck_marks: x.recheck_marks.union(from_nd.recheck_marks),
        assume_marks: x.assume_marks.union(from_nd.assume_marks),
      }), fm, x => { ...x, recheck_marks: Set(), assume_marks: Set() })
    val s4 =
      if (from_nd.open_intent.eid >= 0 and from_nd.open_intent.session != sess and s3.nodes.get(i).open_intent.eid < 0)
        upd_node(upd_node(s3, i, x => { ...x, open_intent: from_nd.open_intent }), fm, x => { ...x, open_intent: NO_INTENT })
      else s3
    val answering = s4.saids.keys().filter(x => {
      val sd = s4.saids.get(x)
      sd.node == i and (sd.kind == SReply or sd.kind == SResult or sd.kind == SCorrection) and sd.state != SdWithheld
    })
    val s5 = sorted(moved).foldl(s4, (st, r) => {
      val req = st.requests.get(r)
      val by = set_min(answering.filter(x => x > r and s4.saids.get(x).conv == req.conv), -1)
      if (req.answered_by < 0 and req.requester == ReqExternal and by >= 0)
        try_settle({ ...st, requests: st.requests.set(r, { ...req, answered_by: by }) }, r)
      else st
    })
    val s6 = append_entry(s5, i, now, sess, ESystem, -1, false)
    val s7 = node_waiters(s6, fm).fold(s6, (st, w) =>
      if (w == i) upd_node(st, w, x => { ...x, wait: NoWait }) else set_wait(st, w, WNode(i), now))
    val s8 = s7.nodes.keys().filter(d => s7.nodes.get(d).depends_on.contains(fm)).fold(s7, (st, d) =>
      upd_node(st, d, x => {
        ...x,
        depends_on: if (d == i) x.depends_on.exclude(Set(fm)) else x.depends_on.exclude(Set(fm)).union(Set(i)),
      }))
    val s9 = close_node(s8, fm, Superseded, sess, i, CauseMerge, -1, now)
    ex(resolve_signals(s9, fm, i, now), [])
  }

  // hold (§20.6) --------------------------------------------------------------------------------

  /// `hold take` (D.8.10): an existing hold is returned (with `Shown`); otherwise a closed node is
  /// reopened, the hold issued, its `holder:` notes bound, and `Shown`. Reply ids: `[node, fence]`.
  pure def take_ex(s: State, sess: int, now: int, a: { node: int, lease: int }): Ex =
    if (holds_node(s, sess, a.node)) {
      val f = holds_on(s, a.node).filter(t => s.holds.get(t).session == sess).fold(-1, (acc, t) => t._2)
      ex_if(shown(s, sess, a.node), [a.node, f], raises_shown(s, sess, a.node))
    } else {
      val s1 = if (is_closed(s, a.node)) reopen(s, a.node, sess, now) else s
      val f = s1.nodes.get(a.node).fence + 1
      val s2 = bind_holder_notes(issue_hold(s1, a.node, sess, a.lease, now), a.node, sess, now)
      ex_hold(shown(s2, sess, a.node), [a.node, f])
    }

  /// `hold next` without a query (D.8.5): `NextCalled`, which records the session's caps as a
  /// profile of its class until `now + refusal.window` (CS-7), then the best candidate: a routing
  /// item, an item with no hold (`Shown`: a status item, or, 1.9, a review item, whose closed node
  /// is not reopened), or a node, reopened if closed, held, and shown. Reply ids: `[]` for empty,
  /// `[2, entry]`, `[1, node]`, or `[0, node, fence]`.
  pure def next_ex(cfg: Cfg, s: State, sess: int, cls: str, now: int): Ex = {
    val c = best_candidate(cfg, s, sess, cls, now)
    val caps = s.sessions.get(sess).caps
    val seen = if (s.classes.keys().contains(cls)) s.classes.get(cls).caps_seen else Map()
    val s1 = { ...s, classes: s.classes.put(cls, { caps_seen: seen.put(caps, now + REFUSAL_WINDOW) }) }
    if (c == NO_CAND) ex(s1, [])
    else if (c.kind == 2) ex(s1, [2, c.id])
    else if (c.kind == 1) ex(shown(s1, sess, c.id), [1, c.id])
    else {
      val s2 = if (is_closed(s1, c.id)) reopen(s1, c.id, sess, now) else s1
      val f = s2.nodes.get(c.id).fence + 1
      val s3 = bind_holder_notes(issue_hold(s2, c.id, sess, -1, now), c.id, sess, now)
      ex_hold(shown(s3, sess, c.id), [0, c.id, f])
    }
  }

  /// `hold checkpoint` (D.8.10): the checkpoint entry, `IntentClosed` with `intent_done`,
  /// `IntentOpened` with `intent` (its eid is the checkpoint's), and `HoldExtended`. Reply ids:
  /// `[lease_until]`.
  pure def checkpoint_ex(s: State, sess: int, now: int, a: { token: (int, int), intent: bool, intent_done: bool }): Ex = {
    val n = a.token._1
    val e = s.next_eid
    val s1 = checkpoint_entry(s, a.token, sess, now)
    val s2 = if (a.intent_done) upd_node(s1, n, x => { ...x, open_intent: NO_INTENT }) else s1
    val s3 = if (a.intent) upd_node(s2, n, x => { ...x, open_intent: { eid: e, session: sess } }) else s2
    // 1.10 (§10.2): a checkpoint of a live hold ends the node's wait (`WaitSet { None }`).
    val s4 = if (s3.nodes.get(n).wait != NoWait) upd_node(s3, n, x => { ...x, wait: NoWait }) else s3
    val lu = extended_until(s4.holds.get(a.token), now)
    ex({ ...s4, holds: s4.holds.setBy(a.token, h => { ...h, lease_until: lu, last_extended: now, renews: 0 }) }, [lu])
  }

  /// `hold renew` (D.8.10): the system entry and `HoldExtended`. Reply ids: `[lease_until,
  /// renews_left]`.
  pure def renew_ex(s: State, sess: int, now: int, t: (int, int)): Ex = {
    val s1 = append_entry(s, t._1, now, sess, ESystem, -1, false)
    val h = s1.holds.get(t)
    val lu = extended_until(h, now)
    ex({ ...s1, holds: s1.holds.set(t, { ...h, lease_until: lu, last_extended: now, renews: h.renews + 1 }) },
       [lu, h.terms.renew_max - (h.renews + 1)])
  }

  /// The wait a release sets (D.8.10): none without `waits_for`.
  pure def wait_on(s: State, w: WaitArg): OptWait =
    match w {
      | WaNone => NoWait
      | WaQuestion(q) =>
          if (s.pquestions.keys().contains(q))
            HasWait({ on: WQuestion({ q: q, rechecks_at_set: s.pquestions.get(q).rechecks }), set_at: -1, fired: false })
          else HasWait({ on: WUserQuestion(q), set_at: -1, fired: false })
      | WaNode(t) => HasWait({ on: WNode(t), set_at: -1, fired: false })
      | WaTime(t) => HasWait({ on: WTime(t), set_at: -1, fired: false })
      | WaExternal(t) => HasWait({ on: WExternal(t), set_at: -1, fired: false })
    }

  pure def same_wait(a: OptWait, b: OptWait): bool =
    match a {
      | NoWait => b == NoWait
      | HasWait(x) => match b { | NoWait => false | HasWait(y) => x.on == y.on }
    }

  /// `Refused { n, cls, sess, now + refusal.backoff, now + refusal.ttl, quorum }` (CS-7): the
  /// session's entry in its class's record on `n`, which a `cannot` release and (1.11) a deferral
  /// of a closed node's review record.
  pure def refuse(cfg: Cfg, s: State, n: int, sess: int, cls: str, now: int): State = {
    val entry = { backoff_until: now + REFUSAL_BACKOFF, expires_at: now + REFUSAL_TTL }
    val rec =
      if (s.refusals.keys().contains((n, cls))) { ...s.refusals.get((n, cls)), by: s.refusals.get((n, cls)).by.put(sess, entry) }
      else { quorum: cfg.quorum, by: Map(sess -> entry) }
    { ...s, refusals: s.refusals.put((n, cls), rec) }
  }

  /// `hold release` (D.8.10): the checkpoint entry (A-9); `WaitSet` when the wait differs; a people
  /// question waited on gets its recheck rescheduled (A-24); the recheck marks leave; `HoldEnded
  /// { Released }` and the missed check; with `cannot` and no wait, the refusal (CS-7) and
  /// `e1_check`; the session's pending `said` entries confirmed; 1.12 (§10.3): with `settles`, the
  /// decision `talk decide {at, text: done, settles}` records (`record_decision`). Reply ids: `[]`,
  /// or `[decision]` with `settles`.
  pure def release_ex(cfg: Cfg, s: State, sess: int, cls: str, now: int,
                      a: { token: (int, int), reason: ReleaseReason, wait: WaitArg, settles: Set[int] }): Ex = {
    val n = a.token._1
    val reason = if (a.wait != WaNone) Blocked else a.reason
    // 1.12 (§10.4): a `handoff` release keeps the node from its releaser for the hold's
    // `lease.first`, or until the node's next hold (its fence moves past the token's).
    val s0 =
      if (reason == Handoff and s.holds.keys().contains(a.token) and s.sessions.keys().contains(sess))
        { ...s, sessions: s.sessions.setBy(sess, se => { ...se, handed_off: {
            node: n, fence: a.token._2, until: now + s.holds.get(a.token).terms.first } }) }
      else s
    val s1 = checkpoint_entry(s0, a.token, sess, now)
    val w2 = wait_on(s1, a.wait)
    val s2 =
      if (same_wait(s1.nodes.get(n).wait, w2)) s1
      else match w2 {
        | NoWait => upd_node(s1, n, x => { ...x, wait: NoWait })
        | HasWait(w) => set_wait(s1, n, w.on, now)
      }
    val s3 = match a.wait {
      | WaQuestion(q) =>
          if (s2.pquestions.keys().contains(q) and s2.pquestions.get(q).state == PqOpen)
            { ...s2, pquestions: s2.pquestions.setBy(q, x => { ...x, recheck_at: now + QUESTION_RECHECK }) }
          else s2
      | _ => s2
    }
    val s4 = upd_node(s3, n, x => { ...x, recheck_marks: Set() })
    val s5 = ev_hold_ended(s4, a.token, HeReleased)
    val s6 = if (is_occurrence(s5, n)) maybe_missed(s5, n, now) else s5
    val s7 = if (reason != Cannot) s6 else e1_check(refuse(cfg, s6, n, sess, cls, now), n, sess, now)
    val s8 = confirm_saids(s7, sess)
    if (a.settles == Set()) ex(s8, [])
    else ex(record_decision(s8, sess, now, n, a.settles, -1, []), [s8.next_eid])
  }

  // talk: the user (§20.7) ----------------------------------------------------------------------

  /// `talk from_user` (D.8.11): the entry on USER (its principal's message time, and the session
  /// serves the principal), then the routing: an answer to a pending user question (and its
  /// source's consequence), the entry left unrouted for a question no longer pending, a
  /// clarification, a node, the session's only hold, or nothing. 1.11 (§8.2): on the session's
  /// only hold, the default is a clarification of the newest unsettled request there of the
  /// entry's person, when the principal was known before the call and there is one (`follow_up`).
  /// Reply ids: `[entry, principal]`.
  pure def from_user_ex(s: State, sess: int, now: int, a: { principal: str, answers: int, clarifies: int, node: int }): Ex = {
    val choice = principal_choice(s, sess, a.principal)
    val known = match choice {
      | PcKey(k) => s.principal_ids.keys().contains(k)
      | PcId(_) => true
      | PcAmbiguous => false
    }
    val pi = take_principal(s, choice)
    val p = pi.id
    val e = pi.st.next_eid
    val s1 = append_entry(pi.st, USER, now, sess, EFromUser, p, true)
    val s2 = {
      ...s1,
      user_entries: s1.user_entries.put(e, { session: sess, principal: p, at: now, routing: Unrouted }),
      user_msgs: s1.user_msgs.put(p, get_or(s1.user_msgs, p, []).select(t => t >= now - USER_GAP_WINDOW).append(now)),
      sessions: s1.sessions.setBy(sess, se => { ...se, principals: se.principals.union(Set(p)) }),
    }
    val mine = live_holds_of(s2, sess, now)
    val s3 =
      if (a.answers >= 0) {
        val uq = a.answers
        val u = s2.uquestions.get(uq)
        if (u.state != UqPending) s2
        else {
          val st1 = {
            ...s2,
            user_entries: s2.user_entries.setBy(e, x => { ...x, routing: RtAnswers(uq) }),
            uquestions: s2.uquestions.set(uq, { ...u, state: UqAnswered(e) }),
          }
          match u.source {
            | SrcEscalation(q) => if (st1.pquestions.keys().contains(q)) answer_pq(st1, q, e) else st1
            | SrcRefusal(m) => clear_refusals(st1, m)
            | SrcAgent(_) => st1
          }
        }
      }
      else if (a.clarifies >= 0) { ...s2, user_entries: s2.user_entries.setBy(e, x => { ...x, routing: RtClarifies(a.clarifies) }) }
      else if (a.node >= 0) route_to(s2, e, a.node, sess, now).st
      else if (mine.size() == 1) {
        val n = mine.fold(-1, (acc, t) => t._1)
        val follow =
          if (known) set_max(unsettled_on(s2, n).filter(r => match s2.requests.get(r).requester {
            | ReqUser(q) => same_person(s2, q, p)
            | ReqExternal => false
          }), -1)
          else -1
        if (follow >= 0) { ...s2, user_entries: s2.user_entries.setBy(e, x => { ...x, routing: RtClarifies(follow) }) }
        else route_to(s2, e, n, sess, now).st
      }
      else s2
    ex(s3, [e, p])
  }

  /// `talk to_user` (D.8.11): the entry on USER; `DigestShown` for each pending digest entry in `re`
  /// for the same person (CS-5); for kind `result` only, the user answers of the same person's
  /// requests named in `re`, directly or through their user entry, and `try_settle` (A-23, CS-4).
  /// Reply ids: `[entry, principal]`.
  pure def to_user_ex(s: State, sess: int, now: int, a: { principal: str, kind: ToUserKind, re: Set[int] }): Ex = {
    val pi = take_principal(s, principal_choice(s, sess, a.principal))
    val p = pi.id
    val e = pi.st.next_eid
    val s1 = append_entry(pi.st, USER, now, sess, EToUser, p, true)
    val s2 = {
      ...s1,
      digest: s1.digest.keys().mapBy(d => {
        val de = s1.digest.get(d)
        if (a.re.contains(d) and de.state == DgPending and same_person(s1, de.to, p)) { ...de, state: DgShown(e) } else de
      }),
    }
    val named = a.re.map(x =>
      if (s2.requests.keys().contains(x)) x
      else if (s2.user_entries.keys().contains(x))
        match s2.user_entries.get(x).routing { | RtRequest(rr) => rr.request | _ => -1 }
      else -1).exclude(Set(-1))
    val s3 =
      if (a.kind != TuResult) s2
      else sorted(named).foldl(s2, (st, r) =>
        if (answers_user_request(st, r, p, e)) try_settle({ ...st, requests: st.requests.setBy(r, x => { ...x, answered_by: e }) }, r)
        else st)
    ex(s3, [e, p])
  }

  /// `talk route` (D.8.11): an entry already routed stays as it is ("the first routing wins").
  /// Reply ids: `[request]` (-1 for none).
  pure def route_ex(s: State, sess: int, now: int, a: { entry: int, to: int }): Ex =
    if (s.user_entries.get(a.entry).routing != Unrouted) ex_if(s, [-1], false)
    else if (a.to >= 0) { val r = route_to(s, a.entry, a.to, sess, now) ex(r.st, [r.id]) }
    else ex({ ...s, user_entries: s.user_entries.setBy(a.entry, x => { ...x, routing: RtNoAction }) }, [-1])

  /// `talk ask {to: user | principal:<p>}` (D.8.12): unless `again`, a pending agent question about
  /// `at` with the same digest for the same first addressee is returned; otherwise `create_uq` with
  /// the chain fixed now. Reply ids: `[question]`.
  pure def ask_user_ex(s: State, sess: int, now: int, a: { at: int, has_default: bool, to: str, again: bool, digest: int }): Ex = {
    val pi = if (a.to == USER_KEY) { st: s, id: -1 } else intern(s, a.to)
    val s0 = pi.st
    val first_to = chain(s0, a.at, pi.id, false)[0]
    val existing = s0.uquestions.keys().filter(q => {
      val u = s0.uquestions.get(q)
      u.about == a.at and u.state == UqPending and same_person(s0, u.to[0], first_to) and match u.source {
        | SrcAgent(x) => x.digest == a.digest
        | _ => false
      }
    })
    if (not(a.again) and existing != Set()) ex_if(s0, [set_min(existing, -1)], s0 != s)
    else {
      val q = s0.next_eid
      ex(create_uq(s0, SrcAgent({ session: sess, digest: a.digest }), a.at, a.has_default, pi.id, -1, sess, now), [q])
    }
  }

  // talk: people (§20.7, D.8.15) ------------------------------------------------------------------

  /// `talk request` (D.8.15): a repeat of one message is the same request (Q1); otherwise the
  /// conversation node of `where` (else the origin), its key first in case 3, the request, and the
  /// routing of an unrouted user entry it comes from. Reply ids: `[request, node]`.
  pure def request_ex(s: State, sess: int, now: int, a: { at: int, origin: Key, where_key: OptKey }): Ex = {
    val r0 = request_by_origin(s, a.origin)
    if (r0 >= 0) ex_if(s, [r0, s.requests.get(r0).node], false)
    else {
      val k = request_conv(a)
      val cn = conv_node(s, a.at, k)
      val c = cn.node
      val s1 = if (cn.case == 3) add_key(s, c, k, now) else s
      val fu = user_key_entry(a.origin)
      val rec = record_request(s1, c, a.origin, k, fu, sess, now)
      val s2 =
        if (fu >= 0 and rec.st.user_entries.keys().contains(fu) and rec.st.user_entries.get(fu).routing == Unrouted)
          { ...rec.st, user_entries: rec.st.user_entries.setBy(fu, x => { ...x, routing: RtRequest({ node: c, request: rec.id }) }) }
        else rec.st
      ex(s2, [rec.id, c])
    }
  }

  /// The `said` entry and its entity on the conversation node `c` (eid `s.next_eid`), in state `st`.
  pure def said_entry(s: State, c: int, k: Key, kind: SaidKind, sess: int, now: int, st: SaidState): State = {
    val x = s.next_eid
    val s1 = append_entry(s, c, now, sess, ESaid, -1, true)
    { ...s1, saids: s1.saids.put(x, { node: c, conv: k, kind: kind, session: sess, state: st, moved_from: -1 }) }
  }

  /// 1.11 (§9.2): `sess` holds no live hold, so a `said` it records, by `talk said` or `talk ask`,
  /// is taken as sent at once, with its reply (`SaidConfirmed { via: recorded }`): no release or
  /// close of its will come.
  pure def holds_nothing(s: State, sess: int, now: int): bool =
    holds_of(s, sess).forall(t => not(live(s.holds.get(t), now)))

  /// The state a `said` that `sess` records starts in (1.11, §9.2).
  pure def said_start(s: State, sess: int, now: int): SaidState =
    if (holds_nothing(s, sess, now)) SdConfirmed else SdPending

  /// `conversation_held` (§9.2, D.8.26's `conversation_holder`): a session other than `sess` holds
  /// `c` or a node below it live, and `sess` holds neither.
  pure def conv_held(s: State, c: int, sess: int, now: int): bool = {
    val hs = s.holds.keys().filter(t => live(s.holds.get(t), now) and is_under(s, t._1, c))
    and {
      hs.exists(t => s.holds.get(t).session != sess),
      not(hs.exists(t => s.holds.get(t).session == sess)),
    }
  }

  /// `talk said` (D.8.15): the conversation key `k` (`where`, or the default of §9.1), the key on
  /// `at` in case 3, the `said`; an `ack` refreshes the open commitments of `k` on `c`, or opens
  /// one; a reply, result, or correction fulfils the open commitments of `k` on `c` and answers the
  /// requests of `k` from people and agents recorded there before it (A-84), which then settle if
  /// decided. A reply in another conversation that shares `c` fulfils and answers none of that
  /// conversation's (§9.2, §9.4, 1.3). Reply ids: `[said, node]` followed by the requests the step
  /// settled, ascending (`Said.settled`, D.8.15, 1.3). 1.10 (§9.2): a `said` by a session that holds
  /// neither `c` nor a node below it, while another session does (`conv_held`, the warning), does
  /// all of that too; 1.9 recorded it withheld.
  pure def said_ex(s: State, sess: int, now: int, a: { at: int, kind: SaidKind, where_key: OptKey }): Ex = {
    val cv = conv_of(s, a.at, a.where_key)
    val c = cv.node
    val k = cv.key
    val s1 = if (cv.case == 3) add_key(s, c, k, now) else s
    val x = s1.next_eid
    val s2 = said_entry(s1, c, k, a.kind, sess, now, said_start(s, sess, now))
    val mine = open_commitments_on(s2, c).filter(m => s2.commitments.get(m).conv == k)
    if (a.kind == SAck) {
      val s3 =
        if (mine != Set())
          { ...s2, commitments: s2.commitments.keys().mapBy(m => {
              val cm = s2.commitments.get(m)
              if (mine.contains(m)) { ...cm, last_ack: now, followup_at: now + COMMITMENT_FOLLOWUP, followup_fired: false } else cm
            }) }
        else
          { ...s2, commitments: s2.commitments.put(x, {
              node: c, conv: k, last_ack: now, followup_at: now + COMMITMENT_FOLLOWUP,
              followup_fired: false, state: CmOpen, moved_from: -1,
            }) }
      ex(s3, [x, c])
    } else {
      val fulfilled = {
        ...s2,
        commitments: s2.commitments.keys().mapBy(m => {
          val cm = s2.commitments.get(m)
          if (mine.contains(m)) { ...cm, state: CmFulfilled(x) } else cm
        }),
      }
      val answered = sorted(fulfilled.requests.keys().filter(r => {
        val req = fulfilled.requests.get(r)
        req.node == c and req.conv == k and is_unsettled(req) and req.answered_by < 0
        and req.requester == ReqExternal and r < x
      }))
      val s3 = answered.foldl(fulfilled, (st, r) =>
        try_settle({ ...st, requests: st.requests.setBy(r, q => { ...q, answered_by: x }) }, r))
      ex(s3, [x, c].concat(answered.select(r => s3.requests.get(r).settled != Unsettled)))
    }
  }

  /// `talk said {said_id, confirmed}` (D.8.15). Reply ids: `[said]`.
  pure def confirm_ex(s: State, x: int): Ex =
    if (s.saids.get(x).state == SdConfirmed) ex_if(s, [x], false)
    else ex({ ...s, saids: s.saids.setBy(x, sd => { ...sd, state: SdConfirmed }) }, [x])

  /// `talk heard` (D.8.15): the key in case 3, the entry, and with `answers` a question that is
  /// open, expired, or escalated is answered. Reply ids: `[entry, node, answered]` (-1 for none).
  pure def heard_ex(s: State, sess: int, now: int, a: { at: int, answers: int }): Ex = {
    val cv = conv_of(s, a.at, NoKey)
    val c = cv.node
    val s1 = if (cv.case == 3) add_key(s, c, cv.key, now) else s
    val h = s1.next_eid
    val s2 = append_entry(s1, c, now, sess, EHeard, -1, true)
    val answered = a.answers >= 0 and pq_answerable(s2.pquestions.get(a.answers).state)
    ex(if (answered) answer_pq(s2, a.answers, h) else s2, [h, c, if (answered) a.answers else -1])
  }

  /// `talk ask` to a person (D.8.15): an open question on the same conversation key is returned
  /// unless `again`; otherwise the `said` of kind question, the question (first recheck at
  /// `now + question.recheck`, A-24), and with `again` the old question superseded and its
  /// unresolved waits moved (A-109). Reply ids: `[question, said, node]` (`said` -1 for an
  /// existing one).
  pure def ask_person_ex(s: State, sess: int, now: int, a: { at: int, blocking: bool, again: bool }): Ex = {
    val cv = conv_of(s, a.at, NoKey)
    val c = cv.node
    val old = set_min(s.pquestions.keys().filter(q =>
      s.pquestions.get(q).node == c and s.pquestions.get(q).conv == cv.key and s.pquestions.get(q).state == PqOpen), -1)
    if (old >= 0 and not(a.again)) ex_if(s, [old, -1, c], false)
    else {
      val s1 = if (cv.case == 3) add_key(s, c, cv.key, now) else s
      val x = s1.next_eid
      val s2 = said_entry(s1, c, cv.key, SQuestion, sess, now, said_start(s, sess, now))
      val q = s2.next_eid
      val s3 = append_entry(s2, c, now, sess, EQuestion, -1, true)
      val s4 = { ...s3, pquestions: s3.pquestions.put(q, {
        node: c, conv: cv.key, blocking: a.blocking, said: x, state: PqOpen, rechecks: 0,
        recheck_at: now + QUESTION_RECHECK, lapse: NoLapse, assumed_by: -1, moved_from: -1,
      }) }
      val s5 =
        if (old < 0) s4
        else {
          val opq = s4.pquestions.get(old)
          val waiters = s4.nodes.keys().filter(w => match s4.nodes.get(w).wait {
            | NoWait => false
            | HasWait(wt) => wt.on == WQuestion({ q: old, rechecks_at_set: opq.rechecks })
          })
          val st1 = { ...s4, pquestions: s4.pquestions.set(old, { ...opq, state: PqSuperseded(q) }) }
          waiters.fold(st1, (st, w) => set_wait(st, w, WQuestion({ q: q, rechecks_at_set: 0 }), now))
        }
      ex(s5, [q, x, c])
    }
  }

  // talk: agents (§20.7, D.8.16) ------------------------------------------------------------------

  /// `talk ask {to: agent}`: the question with its note to the addressees. Reply ids: `[question]`.
  pure def ask_agent_ex(s: State, sess: int, now: int, a: { at: int, to: AgentTo }): Ex = {
    val q = s.next_eid
    val s1 = record_note(s, a.at, agent_recipients(s, a.to, sess), EQuestion, sess, now)
    ex({ ...s1, aquestions: s1.aquestions.put(q, { node: a.at, asker: sess, state: AqOpen }) }, [q])
  }

  /// `talk answer` (D.8.16): the answer entry, a note to the asker, on the question's node, and
  /// `AqAnswered` if the question is resident and still open. Reply ids: `[entry]`.
  pure def answer_ex(s: State, sess: int, now: int, q: int): Ex = {
    val m = s.entries.get(q)
    val x = s.next_eid
    val s1 = record_note(s, m.node, { to: ToSession(m.by), recipients: Set(m.by), pending_holder: false }, EAnswer, sess, now)
    val s2 =
      if (s1.aquestions.keys().contains(q) and s1.aquestions.get(q).state == AqOpen)
        { ...s1, aquestions: s1.aquestions.setBy(q, aq => { ...aq, state: AqAnswered(x) }) }
      else s1
    ex(s2, [x])
  }

  /// The sessions other than `sess` with a live hold on `n`.
  pure def other_holders(s: State, n: int, sess: int, now: int): Set[int] =
    live_holds_on(s, n, now).map(t => s.holds.get(t).session).exclude(Set(sess))

  /// 1.11 (§13): the session that closed `n`, when `n` is closed and that session is live (not
  /// silent, not retired) and is not `sess`; -1 otherwise.
  pure def closer(s: State, n: int, sess: int): int =
    match s.nodes.get(n).life {
      | Closed(c) =>
          if (c.by >= 0 and c.by != sess and has_session(s, c.by) and not(s.sessions.get(c.by).silent)) c.by else -1
      | Open => -1
    }

  /// `talk say` (D.8.16): a plain note without `to` (no `EntryMeta`, CS-17), unless another session
  /// holds `at` (1.9, §13): then a note to `holder:<at>`, delivered to those holders; or nobody
  /// holds `at` and it can be held (1.10, §13: open (1.11), not reserved, capacity above 0): then a
  /// note to `holder:<at>` waiting for its next holder; or (1.11) `at` is closed and the session
  /// that closed it is live: then a note to it; otherwise a note to the addressees; a bare `user`
  /// is the head of `at`'s chain (CS-3), a principal is interned. Reply ids: `[entry]`.
  pure def say_ex(s: State, sess: int, now: int, a: { at: int, to: SayTo }): Ex =
    match a.to {
      | SayNone => {
          val x = s.next_eid
          val hs = other_holders(s, a.at, sess, now)
          val nd = s.nodes.get(a.at)
          val cl = closer(s, a.at, sess)
          if (hs != Set())
            ex(record_note(s, a.at, { to: ToHolder(a.at), recipients: hs, pending_holder: false }, ENote, sess, now), [x])
          else if (node_is_open(nd) and live_holds_on(s, a.at, now) == Set() and nd.origin != OReserved and nd.capacity > 0)
            ex(record_note(s, a.at, { to: ToHolder(a.at), recipients: Set(), pending_holder: true }, ENote, sess, now), [x])
          else if (cl >= 0)
            ex(record_note(s, a.at, { to: ToSession(cl), recipients: Set(cl), pending_holder: false }, ENote, sess, now), [x])
          else
            ex(append_entry(s, a.at, now, sess, ENote, -1, false), [x])
        }
      | SayPrincipal(k) => {
          val pi = intern(s, k)
          val x = pi.st.next_eid
          ex(record_note(pi.st, a.at, { to: ToPrincipal(pi.id), recipients: Set(), pending_holder: false }, ENote, sess, now), [x])
        }
      | _ => {
          val rc = match a.to {
            | SaySession(x) => { to: ToSession(x), recipients: Set(x), pending_holder: false }
            | SayHolder(n) => holder_recipients(s, n, sess)
            | SayHolders(n) => { to: ToHolders(n), recipients: Set(), pending_holder: false }
            | _ => { to: ToPrincipal(chain(s, a.at, -1, false)[0]), recipients: Set(), pending_holder: false }
          }
          val x = s.next_eid
          ex(record_note(s, a.at, rc, ENote, sess, now), [x])
        }
    }

  /// `talk wait` (D.8.16): `InboxRead` of what it delivers; news moves no `seen`. Reply ids: the
  /// delivered notes, ascending.
  pure def wait_ex(s: State, sess: int, now: int): Ex = {
    val r = read_inbox(s, sess, now)
    ex_if(r.st, sorted(r.delivered), r.delivered != Set())
  }

  // talk: review (§20.7, D.8.17) ------------------------------------------------------------------

  /// `talk decide` (D.8.17): the decision with its basis (each node reference through
  /// `merged_into`, with its outcome now, A-88), the supersession, the assumptions of `at`, the
  /// settling, and the close of an open signal node (A-34). Reply ids: `[decision]`.
  pure def decide_ex(s: State, sess: int, now: int, a: { at: int, settles: Set[int], supersedes: int, basis: List[BasisArg] }): Ex = {
    val d = s.next_eid
    val refs = a.basis.foldl([], (out, b) => out.append(match b {
      | BaNode(x) => { val f = resolve_merged(s, x.id) BNode({ id: f, life_at: life_at(s, f) }) }
      | BaEntry(e) => BEntry(e)
    }))
    ex(record_decision(s, sess, now, a.at, a.settles, a.supersedes, refs), [d])
  }

  /// The events of `talk decide` (D.8.17), which (1.12, §10.3) a `hold release {settles}` records
  /// after the release: the decision on `at` (eid `s.next_eid`), the supersession, the assumptions
  /// of `at`, the settling, and the close of an open signal node (A-34).
  pure def record_decision(s: State, sess: int, now: int, at: int, settles: Set[int], supersedes: int,
                           refs: List[BasisRef]): State = {
    val d = s.next_eid
    val s1 = append_entry(s, at, now, sess, EDecision, -1, true)
    val s2 = { ...s1, decisions: s1.decisions.put(d, { node: at, settles: settles, state: DActive, basis: refs }) }
    val s3 = if (supersedes >= 0) { ...s2, decisions: s2.decisions.setBy(supersedes, x => { ...x, state: DSuperseded(d) }) } else s2
    val s4 = sorted(settles).foldl(record_assumed(s3, at, d), (st, r) => try_settle(st, r))
    if (is_open(s4, at) and role_is_signal(s4.nodes.get(at).role)) close_node(s4, at, Done, sess, -1, CauseClose, -1, now)
    else s4
  }

  /// `talk retract` (D.8.17, D-4): the entry on the target's node puts the target into
  /// `retracted`; an active decision is retracted; 1.12 (§9.3): an open question to a person is
  /// withdrawn (`PqWithdrawn`), and the marks it set leave, so its waits resolve and its recheck
  /// leaves the queue. Reply ids: `[entry]`.
  pure def retract_ex(s: State, sess: int, now: int, t: BasisTarget): Ex = {
    val n = match t { | TNode(m) => m | TEntry(e) => s.entries.get(e).node }
    val x = s.next_eid
    val s1 = append_entry(s, n, now, sess, ERetract, -1, true)
    val s2 = { ...s1, retracted: s1.retracted.union(Set(t)) }
    val s3 = match t {
      | TEntry(e) =>
          if (s2.decisions.keys().contains(e) and s2.decisions.get(e).state == DActive)
            { ...s2, decisions: s2.decisions.setBy(e, dd => { ...dd, state: DRetracted(x) }) }
          else if (s2.pquestions.keys().contains(e) and s2.pquestions.get(e).state == PqOpen) {
            val s3 = { ...s2, pquestions: s2.pquestions.setBy(e, pq => { ...pq, state: PqWithdrawn(x) }) }
            { ...s3, nodes: s3.nodes.keys().mapBy(m => {
              val nd = s3.nodes.get(m)
              { ...nd, recheck_marks: nd.recheck_marks.exclude(Set(e)), assume_marks: nd.assume_marks.exclude(Set(e)) }
            }) }
          }
          else s2
      | TNode(_) => s2
    }
    ex(s3, [x])
  }

  /// `talk approve` (D.8.17): counted at its version. Reply ids: `[entry]`.
  pure def approve_ex(s: State, sess: int, now: int, a: { node: int, ver: int }): Ex = {
    val x = s.next_eid
    val s1 = append_entry(s, a.node, now, sess, EApprove, -1, true)
    ex({ ...s1, approvals: s1.approvals.put((a.node, a.ver), get_or(s1.approvals, (a.node, a.ver), 0) + 1) }, [x])
  }

  /// `talk object`: an open objection. Reply ids: `[objection]`.
  pure def object_ex(s: State, sess: int, now: int, n: int): Ex = {
    val o = s.next_eid
    val s1 = append_entry(s, n, now, sess, EObject, -1, true)
    ex({ ...s1, objections: s1.objections.put(o, { node: n, state: ObOpen, moved_from: -1 }) }, [o])
  }

  /// `talk resolve`: anyone may resolve. Reply ids: `[entry]` (-1 for none). 1.8 (§11.3): an
  /// upheld resolve marks the objection upheld and, on a closed node, corrects it
  /// (`NodeCorrected`): `done` becomes `failed` (the model's nodes have no kind, so never
  /// `refuted`), another outcome stays, and the closure is marked upheld, `from_done` when it was
  /// `done` before the first correction that marks it; no counter moves. 1.10 (§11.3): a resolved
  /// objection stays as it is; with the same verdict nothing is recorded, and with the opposite one
  /// the caller's text is a new open objection on the objection's node, followed through
  /// `merged_into` (reply ids `[objection]`), and, when the earlier verdict upheld it and still marks
  /// the closed node, the correction is withdrawn (`CorrectionWithdrawn`): `done` again if it was,
  /// unmarked.
  ///
  /// 1.11 (§10.3, §11.3): a deferral of an open objection is a `resolve` entry that decides
  /// nothing, and, on a closed node, the caller's refusal of it (`refuse`), as a `cannot` release
  /// records one; of a resolved objection it records nothing. A verdict recorded on a closed node,
  /// a resolve or a second verdict's objection, clears the node's refusals (`clear_refusals`).
  pure def resolve_ex(cfg: Cfg, s: State, sess: int, cls: str, now: int, a: { objection: int, upheld: bool, defer: bool }): Ex = {
    val o = a.objection
    val ob = s.objections.get(o)
    match ob.state {
      | ObOpen =>
          if (a.defer) {
            val x = s.next_eid
            val s1 = append_entry(s, ob.node, now, sess, EResolve, -1, true)
            ex(if (is_open(s1, ob.node)) s1 else refuse(cfg, s1, ob.node, sess, cls, now), [x])
          }
          else {
          val x = s.next_eid
          val s1 = append_entry(s, ob.node, now, sess, EResolve, -1, true)
          val s2 = { ...s1, objections: s1.objections.set(o, { ...ob, state: if (a.upheld) ObUpheld(x) else ObResolved(x) }) }
          val s3 = match s2.nodes.get(ob.node).life {
            | Closed(c) =>
                if (a.upheld)
                  upd_node(s2, ob.node, nd => {
                    ...nd, life: Closed({
                      ...c,
                      outcome: if (c.outcome == Done) Failed else c.outcome,
                      upheld: x,
                      from_done: if (c.upheld < 0) c.outcome == Done else c.from_done,
                    })
                  })
                else s2
            | Open => s2
          }
          ex(verdict_clears(s3, ob.node), [x])
          }
      | ObResolved(v) => if (a.defer) ex_if(s, [-1], false) else dispute_ex(s, sess, now, o, v, false, a.upheld)
      | ObUpheld(v) => if (a.defer) ex_if(s, [-1], false) else dispute_ex(s, sess, now, o, v, true, a.upheld)
    }
  }

  /// 1.11 (§10.3, §11.3): a verdict recorded on closed node `n` clears its refusals, the
  /// deferrals of its review among them (`RefusalsCleared`).
  pure def verdict_clears(s: State, n: int): State =
    if (is_open(s, n) or not(refused_nodes(s).contains(n))) s else clear_refusals(s, n)

  /// 1.10 (§11.3): a resolve of objection `o`, resolved by entry `v` (`was_upheld`), with verdict
  /// `upheld`: the same verdict records nothing; the opposite one records a new open objection by
  /// `sess` on `o`'s node through `merged_into`, and withdraws the correction `v` made if it still
  /// marks `o`'s closed node.
  pure def dispute_ex(s: State, sess: int, now: int, o: int, v: int, was_upheld: bool, upheld: bool): Ex =
    if (was_upheld == upheld) ex_if(s, [-1], false)
    else {
      val n = s.objections.get(o).node
      val at = resolve_merged(s, n)
      val x = s.next_eid
      val s1 = append_entry(s, at, now, sess, EObject, -1, true)
      val s2 = { ...s1, objections: s1.objections.put(x, { node: at, state: ObOpen, moved_from: -1 }) }
      val s3 = match s2.nodes.get(n).life {
        | Closed(c) =>
            if (was_upheld and c.upheld == v)
              upd_node(s2, n, nd => {
                ...nd, life: Closed({ ...c, outcome: if (c.from_done) Done else c.outcome, upheld: -1, from_done: false })
              })
            else s2
        | Open => s2
      }
      ex(verdict_clears(s3, at), [x])
    }

  /// `hold check {target: n, intent_done}` (1.8, §10.2): the `checkpoint` entry of the checking
  /// session, kept by no index (`keeps_meta` is false for it), and `IntentClosed`; no lease, no
  /// counter. Reply ids: `[entry, intent]`.
  pure def check_intent_ex(s: State, sess: int, now: int, n: int): Ex = {
    val x = s.next_eid
    val i = s.nodes.get(n).open_intent.eid
    val s1 = append_entry(s, n, now, sess, ECheckpoint, -1, false)
    ex(upd_node(s1, n, nd => { ...nd, open_intent: NO_INTENT }), [x, i])
  }

  // control (§20.8, D.8.21) and view -------------------------------------------------------------

  /// `control pause` (D.8.21): pausing again renews, and the renewer becomes the source. Reply ids:
  /// `[until]`.
  pure def pause_ex(s: State, sess: int, cls: str, now: int, n: int): Ex = {
    val s1 = { ...s, pauses: s.pauses.put(n, { until: now + PAUSE_TTL, last_renewed: now, ttl: PAUSE_TTL, by: sess, class: cls }) }
    ex(append_entry(s1, n, now, sess, ESystem, -1, false), [now + PAUSE_TTL])
  }

  pure def unpause_ex(s: State, sess: int, now: int, n: int): Ex =
    ex(append_entry({ ...s, pauses: map_remove(s.pauses, n) }, n, now, sess, ESystem, -1, false), [])

  /// `view node`: `Shown` when it raises `seen` (CS-14). Reply ids: `[]`.
  pure def view_ex(s: State, sess: int, n: int): Ex = ex_if(shown(s, sess, n), [], raises_shown(s, sess, n))

  /// `node hide {entry}` (D.8.20): `Hidden` (no base change, O3) and the system entry on the entry's
  /// node. Reply ids: `[]`.
  pure def hide_ex(s: State, sess: int, now: int, e: int): Ex =
    ex(append_entry(s, s.entries.get(e).node, now, sess, ESystem, -1, false), [])

  /// `node key {remove}`: `KeyRemoved` (version + 1). Reply ids: `[]`.
  pure def key_remove_ex(s: State, n: int, k: Key, now: int): Ex = ex(remove_key(s, n, k, now), [])

  /// `node reopen`; 1.12 (§16): the `offer` tag unless the node carries it or is a container, as
  /// at the end of a hold, so that `next` offers the reopened work. Reply ids: `[]`.
  pure def reopen_ex(s: State, sess: int, now: int, n: int): Ex = {
    val s1 = reopen(s, n, sess, now)
    if (is_open(s1, n) and not(is_container(s1, n))) ex(upd_node(s1, n, nd => { ...nd, tags: nd.tags.union(Set(OFFER)) }), [])
    else ex(s1, [])
  }

  /// The execution of a validated command (D.8.1 step 5), on the state after `at_event`.
  pure def execute(cfg: Cfg, s: State, sess: int, cls: str, now: int, cmd: Cmd): Ex =
    match cmd {
      | CCreate(a) => create_ex(s, sess, now, a)
      | CUpdate(a) => update_ex(s, sess, now, a)
      | CLink(a) => link_ex(s, now, a)
      | CKeyAdd(a) => key_add_ex(s, sess, now, a.node, a.key)
      | CKeyRemove(a) => key_remove_ex(s, a.node, a.key, now)
      | CClose(a) => close_ex(s, sess, now, a)
      | CReopen(n) => reopen_ex(s, sess, now, n)
      | CMove(a) => move_ex(s, sess, now, a)
      | CMerge(a) => merge_ex(s, sess, now, a)
      | CTake(a) => take_ex(s, sess, now, a)
      | CNext => next_ex(cfg, s, sess, cls, now)
      | CCheckpoint(a) => checkpoint_ex(s, sess, now, a)
      | CRenew(t) => renew_ex(s, sess, now, t)
      | CRelease(a) => release_ex(cfg, s, sess, cls, now, a)
      | CFromUser(a) => from_user_ex(s, sess, now, a)
      | CToUser(a) => to_user_ex(s, sess, now, a)
      | CRoute(a) => route_ex(s, sess, now, a)
      | CRequest(a) => request_ex(s, sess, now, a)
      | CSaid(a) => said_ex(s, sess, now, a)
      | CConfirm(x) => confirm_ex(s, x)
      | CHeard(a) => heard_ex(s, sess, now, a)
      | CAskPerson(a) => ask_person_ex(s, sess, now, a)
      | CAskUser(a) => ask_user_ex(s, sess, now, a)
      | CAskAgent(a) => ask_agent_ex(s, sess, now, a)
      | CAnswer(q) => answer_ex(s, sess, now, q)
      | CDecide(a) => decide_ex(s, sess, now, a)
      | CRetract(t) => retract_ex(s, sess, now, t)
      | CApprove(a) => approve_ex(s, sess, now, a)
      | CObject(n) => object_ex(s, sess, now, n)
      | CResolve(a) => resolve_ex(cfg, s, sess, cls, now, a)
      | CCheckIntent(n) => check_intent_ex(s, sess, now, n)
      | CSay(a) => say_ex(s, sess, now, a)
      | CWait(_) => wait_ex(s, sess, now)
      | CHide(e) => hide_ex(s, sess, now, e)
      | CPause(n) => pause_ex(s, sess, cls, now, n)
      | CUnpause(n) => unpause_ex(s, sess, now, n)
      | CView(n) => view_ex(s, sess, n)
      | _ => ex_if(s, [], false)
    }

  // ================================================================== one call (D.8.1)

  /// The `op_id` record (O1, CS-8), the last event of a call that appended any.
  pure def record_op(s: State, cls: str, op: str, cmd: Cmd, sess: int, now: int): State =
    if (op == "") s
    else { ...s, ops: s.ops.put((cls, op), { digest: cmd, session: sess, expires_at: now + OP_RETAIN }) }

  /// `apply(s, ctx, cmd)` of D.8.1 for one call at `now` (`max(clock, last_now)`, which the caller
  /// gives) by class `cls`, in session `sess` (ignored by `hello`, which opens a new session), with
  /// `op` ("" for none) and `long` (a free text over `text.max`, D.10.4):
  /// 1. `long`: `limit(TextTooLong)`, nothing applied, before anything else (L1, A-114);
  /// 2. a live `op_id` record of `(cls, op)`: the same command from a new session or from the
  ///    record's session is a `Replay`, anything else `op_id_conflict`; nothing appended (O1);
  /// 3. the session exists (`not_found(Session)`, which a retired session gets too) and is of the
  ///    class (`invalid(ClassMismatch)`);
  /// 4. `validate`; `stale_token` appends one `late` entry on its node (H4), any other error
  ///    nothing;
  /// 5. on success, the command's events, preceded by `Touched` when presence is due or the
  ///    command issues a hold (CS-14; a `hello` opens its session instead);
  /// 6. the `op_id` record when the call appended events.
  /// A call that appends nothing returns `s` itself: no `last_now`, no prune (T1, A-101).
  pure def run_cmd(cfg: Cfg, s: State, now: int, cls: str, sess: int, op: str, long: bool, cmd: Cmd): Run = {
    val is_hello = match cmd { | CHello(_) => true | _ => false }
    if (long) { state: s, outcome: OutErr(ErrLimit(LkTextTooLong)) }
    else if (op != "" and s.ops.keys().contains((cls, op)) and s.ops.get((cls, op)).expires_at > now) {
      val r = s.ops.get((cls, op))
      if (r.digest == cmd and (is_hello or r.session == sess)) { state: s, outcome: OutReplay }
      else { state: s, outcome: OutErr(ErrOpIdConflict) }
    }
    else match cmd {
      | CHello(a) => {
          val x = hello_ex(at_event(s, now), cls, now, a)
          { state: record_op(x.st, cls, op, cmd, x.ids[0], now), outcome: OutOk(x.ids) }
        }
      | _ =>
          if (not(has_session(s, sess))) { state: s, outcome: OutErr(ErrNotFound(WhatSession)) }
          else if (s.sessions.get(sess).class != cls) { state: s, outcome: OutErr(ErrInvalid(WhyClassMismatch)) }
          else match validate(s, sess, now, cmd) {
            | Fail(e) =>
                if (e == ErrStaleToken) {
                  val s1 = append_entry(at_event(s, now), late_node(cmd), now, sess, ELate, -1, false)
                  { state: record_op(s1, cls, op, cmd, sess, now), outcome: OutErr(e) }
                }
                else { state: s, outcome: OutErr(e) }
            | Pass => {
                val x = execute(cfg, at_event(s, now), sess, cls, now, cmd)
                val touch = presence_due(s, sess, now) or x.hold
                if (touch or x.ev)
                  { state: record_op(if (touch) touched(x.st, sess, now) else x.st, cls, op, cmd, sess, now), outcome: OutOk(x.ids) }
                else { state: s, outcome: OutOk(x.ids) }
              }
          }
    }
  }
}
