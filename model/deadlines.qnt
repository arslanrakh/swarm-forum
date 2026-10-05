// -*- mode: Bluespec; -*-
/**
 * Deadlines (D.4.8, D.8.2, §17): `deadline_of`, the queue computed from the state and its minimum,
 * `fire` with every arm of D.8.2, and the bounded settle with its `saturated` and `stuck` flags
 * (D.10.4, D.8.2).
 *
 * The model has no event log: a transition applies the base change of D.7.4 directly (the derived
 * indices do not exist). The functions named `ev_…` are those base changes; the helpers below them
 * (`close_node`, `create_uq`, `add_digest`, `lapse`, `e1_check`, `refusal_default`, `assume_now`,
 * `record_assumptions`, `try_settle`, `maybe_missed`, `open_next_occurrence`) are the ones a firing
 * shares with the commands of `commands.qnt`. They allocate ids, and append entries, in the order
 * Rust's events do, so that a model state and a projected Rust state compare field by field
 * (D.10.8).
 *
 * The queue is never stored: it is `queue(s)`, the recount of D.4.8, so T4's "queue equals the
 * recount" holds by construction and a firing removes its own key exactly when its transition or
 * marker makes `deadline_of` none.
 */
module deadlines {
  import types.* from "./types"
  import params.* from "./params"
  import basics.* from "./basics"

  // ------------------------------------------------------------------ kinds and owners (D.4.8, D.10.4)

  /// The kind order of §17.
  pure val K_LEASE = 0
  pure val K_WAIT = 1
  pure val K_RECHECK = 2
  pure val K_USER_QUESTION = 3
  pure val K_COMMITMENT = 4
  pure val K_OCCURRENCE_SLOT = 5
  pure val K_OCCURRENCE_WINDOW = 6
  pure val K_REFUSAL_EXPIRY = 7
  pure val K_DUE = 8
  pure val K_PAUSE = 9
  pure val K_SILENCE = 10
  pure val K_RETIRE = 11
  pure val K_STALE = 12                 // 1.8 (§14, §17): a node untouched for `offer.stale`

  /// Owner ranks, in the declaration order of Rust's `Owner` (A-43).
  pure val O_HOLD = 0
  pure val O_NODE = 1
  pure val O_ENTRY = 2
  pure val O_REFUSAL = 3
  pure val O_CLASS = 4
  pure val O_SESSION = 5

  /// An owner is `rank × OWNER_BASE + payload`, so that the integer order is Rust's order: payloads
  /// `node × 100 + fence` (Hold), `id` (Node), `eid` (Entry), `node × 10 + class index` (Refusal),
  /// `class index` (Class), and `session` (Session). Fences stay below 100 and class indices below
  /// 10 within the bounds (MAX_VER, CLASS_NAMES).
  pure val OWNER_BASE = 1000000

  /// A queued deadline; derived order `(at, kind, owner)` (§17).
  type DeadlineKey = { at: int, kind: int, owner: int }

  /// No key: the queue is empty.
  pure val NO_KEY: DeadlineKey = { at: -1, kind: -1, owner: -1 }

  /// A class's position in CLASS_NAMES, or -1.
  pure def class_index(c: str): int = get_or(CLASS_INDEX, c, -1)

  pure def class_name(i: int): str = CLASS_NAMES[i]

  pure def owner_hold(t: (int, int)): int = O_HOLD * OWNER_BASE + t._1 * 100 + t._2
  pure def owner_node(n: int): int = O_NODE * OWNER_BASE + n
  pure def owner_entry(e: int): int = O_ENTRY * OWNER_BASE + e
  pure def owner_refusal(n: int, c: str): int = O_REFUSAL * OWNER_BASE + n * 10 + class_index(c)
  pure def owner_class(c: str): int = O_CLASS * OWNER_BASE + class_index(c)
  pure def owner_session(x: int): int = O_SESSION * OWNER_BASE + x

  pure def owner_rank(o: int): int = o / OWNER_BASE
  pure def owner_payload(o: int): int = o % OWNER_BASE

  /// The derived order of `DeadlineKey`: `(at, kind, owner)`.
  pure def key_lt(a: DeadlineKey, b: DeadlineKey): bool =
    a.at < b.at or (a.at == b.at and (a.kind < b.kind or (a.kind == b.kind and a.owner < b.owner)))

  // ------------------------------------------------------------------ the queue (D.4.8)

  /// The Stale key of node `n` (1.8, §14, D.4.8): `touched + offer.stale` while the node is open,
  /// not reserved, touched, and its marker is not set; -1 for none. Whether it is an offer is the
  /// flag's question, not the queue's.
  pure def stale_key_at(nd: Node): int =
    if (node_is_open(nd) and nd.origin != OReserved and not(nd.stale_fired) and nd.touched >= 0)
      nd.touched + OFFER_STALE
    else -1

  /// The node-owned deadlines of `n`: Wait, OccurrenceSlot, OccurrenceWindow, Due, and (1.8) Stale.
  /// (Pause is owned by its scope node but queued while the pause exists, open node or not.)
  pure def node_keys(s: State, n: int): Set[DeadlineKey] = {
    val nd = s.nodes.get(n)
    val o = owner_node(n)
    if (not(node_is_open(nd))) Set()
    else {
      val wait_keys = match nd.wait {
        | NoWait => Set()
        | HasWait(w) =>
            if (w.fired) Set()
            else match w.on {
              | WTime(t) => Set({ at: t, kind: K_WAIT, owner: o })
              | WExternal(t) => Set({ at: t, kind: K_WAIT, owner: o })
              | _ => Set()
            }
      }
      val occ_keys = match nd.role {
        | Occurrence(x) =>
            if (x.phase == Waiting) Set({ at: x.slot, kind: K_OCCURRENCE_SLOT, owner: o })
            else if (not(x.window_fired)) Set({ at: x.window_end, kind: K_OCCURRENCE_WINDOW, owner: o })
            else Set()
        | _ => Set()
      }
      val due_keys =
        if (nd.due >= 0 and not(nd.due_fired)) Set({ at: nd.due, kind: K_DUE, owner: o }) else Set()
      val stale_keys =
        if (stale_key_at(nd) >= 0) Set({ at: stale_key_at(nd), kind: K_STALE, owner: o }) else Set()
      wait_keys.union(occ_keys).union(due_keys).union(stale_keys)
    }
  }

  /// `recount_deadlines(s)`: every owner's deadline (D.4.8). In the model this is the queue.
  pure def queue(s: State): Set[DeadlineKey] = Set(
    s.holds.keys().map(t => { at: s.holds.get(t).lease_until, kind: K_LEASE, owner: owner_hold(t) }),
    s.nodes.keys().map(n => node_keys(s, n)).flatten(),
    s.pquestions.keys()
      .filter(q => s.pquestions.get(q).state == PqOpen)
      .map(q => { at: s.pquestions.get(q).recheck_at, kind: K_RECHECK, owner: owner_entry(q) }),
    s.uquestions.keys()
      .filter(q => s.uquestions.get(q).state == UqPending)
      .map(q => { at: s.uquestions.get(q).due, kind: K_USER_QUESTION, owner: owner_entry(q) }),
    s.commitments.keys()
      .filter(c => s.commitments.get(c).state == CmOpen and not(s.commitments.get(c).followup_fired))
      .map(c => { at: s.commitments.get(c).followup_at, kind: K_COMMITMENT, owner: owner_entry(c) }),
    s.refusals.keys()
      .filter(k => s.refusals.get(k).by.keys() != Set())
      .map(k => {
        val r = s.refusals.get(k)
        { at: set_min(r.by.keys().map(x => r.by.get(x).expires_at), -1),
          kind: K_REFUSAL_EXPIRY,
          owner: owner_refusal(k._1, k._2) }
      }),
    s.classes.keys()
      .filter(c => s.classes.get(c).caps_seen.keys() != Set())
      .map(c => {
        val ca = s.classes.get(c)
        { at: set_min(ca.caps_seen.keys().map(caps => ca.caps_seen.get(caps)), -1),
          kind: K_REFUSAL_EXPIRY,
          owner: owner_class(c) }
      }),
    s.pauses.keys().map(p => { at: s.pauses.get(p).until, kind: K_PAUSE, owner: owner_node(p) }),
    s.sessions.keys()
      .filter(x => not(s.sessions.get(x).silent))
      .map(x => { at: silence_key_at(s, x), kind: K_SILENCE, owner: owner_session(x) }),
    s.sessions.keys().map(x => { at: s.sessions.get(x).retire_at, kind: K_RETIRE, owner: owner_session(x) }),
  ).flatten()

  /// The time of session `x`'s Silence key (1.4, R27, D.4.8): the later of `silence_at` and the end
  /// of the session's last lease, so that a session is never silent while a hold it took is live,
  /// floored at `last_now` (the floor binds only after an event at `last_now` ended the session's
  /// last hold past its `silence_at`).
  pure def silence_key_at(s: State, x: int): int = {
    val last_lease = s.holds.keys().fold(-1, (acc, t) => {
      val h = s.holds.get(t)
      if (h.session == x and h.lease_until > acc) h.lease_until else acc
    })
    imax(imax(s.sessions.get(x).silence_at, last_lease), s.last_now)
  }

  /// `deadline_of(s, kind, owner)`: the time of the owner's deadline of that kind, or -1.
  pure def deadline_of(s: State, kind: int, owner: int): int =
    set_min(queue(s).filter(k => k.kind == kind and k.owner == owner).map(k => k.at), -1)

  /// The minimum of `queue(s)` by its definition, or NO_KEY: the reference that `first_key` is
  /// tested against (`firstKeyTest`).
  pure def queue_min(s: State): DeadlineKey =
    queue(s).fold(NO_KEY, (best, k) => if (best == NO_KEY or key_lt(k, best)) k else best)

  /// The smaller of two keys in the derived order; NO_KEY is above every key.
  pure def min_key(a: DeadlineKey, b: DeadlineKey): DeadlineKey =
    if (a == NO_KEY) b else if (b == NO_KEY) a else if (key_lt(b, a)) b else a

  /// `best` lowered by the node-owned keys of `n` (`node_keys`, without building the set).
  pure def node_min(s: State, n: int, best: DeadlineKey): DeadlineKey = {
    val nd = s.nodes.get(n)
    val o = owner_node(n)
    if (not(node_is_open(nd))) best
    else {
      val b1 = match nd.wait {
        | NoWait => best
        | HasWait(w) =>
            if (w.fired) best
            else match w.on {
              | WTime(t) => min_key(best, { at: t, kind: K_WAIT, owner: o })
              | WExternal(t) => min_key(best, { at: t, kind: K_WAIT, owner: o })
              | _ => best
            }
      }
      val b2 = match nd.role {
        | Occurrence(x) =>
            if (x.phase == Waiting) min_key(b1, { at: x.slot, kind: K_OCCURRENCE_SLOT, owner: o })
            else if (not(x.window_fired)) min_key(b1, { at: x.window_end, kind: K_OCCURRENCE_WINDOW, owner: o })
            else b1
        | _ => b1
      }
      val b3 = if (nd.due >= 0 and not(nd.due_fired)) min_key(b2, { at: nd.due, kind: K_DUE, owner: o }) else b2
      if (stale_key_at(nd) >= 0) min_key(b3, { at: stale_key_at(nd), kind: K_STALE, owner: o }) else b3
    }
  }

  /// The smallest of the integers `m` maps to, or -1 when it is empty.
  pure def min_value(m: a -> int): int =
    m.keys().fold(-1, (acc, x) => if (acc < 0 or m.get(x) < acc) m.get(x) else acc)

  /// The minimum of the queue, or NO_KEY: `queue_min(s)`, computed by one fold per owner kind
  /// without building the queue, because every command's guard (`settled`) and every firing of a
  /// settle reads it, and Apalache encodes it each time (README, "Cost of settling in Apalache").
  pure def first_key(s: State): DeadlineKey = {
    val k1 = s.holds.keys().fold(NO_KEY, (b, t) =>
      min_key(b, { at: s.holds.get(t).lease_until, kind: K_LEASE, owner: owner_hold(t) }))
    val k2 = s.nodes.keys().fold(k1, (b, n) => node_min(s, n, b))
    val k3 = s.pquestions.keys().fold(k2, (b, q) =>
      if (s.pquestions.get(q).state == PqOpen)
        min_key(b, { at: s.pquestions.get(q).recheck_at, kind: K_RECHECK, owner: owner_entry(q) })
      else b)
    val k4 = s.uquestions.keys().fold(k3, (b, q) =>
      if (s.uquestions.get(q).state == UqPending)
        min_key(b, { at: s.uquestions.get(q).due, kind: K_USER_QUESTION, owner: owner_entry(q) })
      else b)
    val k5 = s.commitments.keys().fold(k4, (b, c) => {
      val cm = s.commitments.get(c)
      if (cm.state == CmOpen and not(cm.followup_fired))
        min_key(b, { at: cm.followup_at, kind: K_COMMITMENT, owner: owner_entry(c) })
      else b
    })
    val k6 = s.refusals.keys().fold(k5, (b, k) => {
      val r = s.refusals.get(k)
      if (r.by.keys() == Set()) b
      else min_key(b, {
        at: r.by.keys().fold(-1, (acc, x) =>
          if (acc < 0 or r.by.get(x).expires_at < acc) r.by.get(x).expires_at else acc),
        kind: K_REFUSAL_EXPIRY,
        owner: owner_refusal(k._1, k._2),
      })
    })
    val k7 = s.classes.keys().fold(k6, (b, c) => {
      val ca = s.classes.get(c)
      if (ca.caps_seen.keys() == Set()) b
      else min_key(b, { at: min_value(ca.caps_seen), kind: K_REFUSAL_EXPIRY, owner: owner_class(c) })
    })
    val k8 = s.pauses.keys().fold(k7, (b, p) =>
      min_key(b, { at: s.pauses.get(p).until, kind: K_PAUSE, owner: owner_node(p) }))
    s.sessions.keys().fold(k8, (b, x) => {
      val se = s.sessions.get(x)
      val b1 = if (se.silent) b else min_key(b, { at: silence_key_at(s, x), kind: K_SILENCE, owner: owner_session(x) })
      min_key(b1, { at: se.retire_at, kind: K_RETIRE, owner: owner_session(x) })
    })
  }

  /// The time of the first queued key, or -1: where `act_tick_next` jumps to.
  pure def next_deadline(s: State): int = first_key(s).at

  /// The state is settled at `now`: no key at or before `now` (T3, T4).
  pure def settled(s: State, now: int): bool = {
    val k = first_key(s)
    k.at < 0 or k.at > now
  }

  // ------------------------------------------------------------------ base changes of events (D.7.4)

  pure def upd_node(s: State, n: int, f: Node => Node): State = { ...s, nodes: s.nodes.setBy(n, f) }

  /// Step 1 of `evolve` for an event at `at`: `last_now` moves, and the `op_id` records with
  /// `expires_at <= at` and the notes with `until <= at` leave, an agent question with its note,
  /// and so does their entry's metadata, which bears nothing else (CS-8, CS-17; the core's
  /// `meta_after`; 1.10's conformance group `review_two`, the first to replay notes, found the
  /// model keeping it). Every event of one command or one firing has the same `at`, so the prune
  /// runs once, before the first; a step without events must not call it (T1).
  pure def at_event(s: State, at: int): State = {
    val expired_notes = s.notes.keys().filter(x => s.notes.get(x).until <= at)
    {
      ...s,
      last_now: imax(s.last_now, at),
      ops: s.ops.keys().filter(k => s.ops.get(k).expires_at > at).mapBy(k => s.ops.get(k)),
      notes: s.notes.keys().exclude(expired_notes).mapBy(x => s.notes.get(x)),
      aquestions: s.aquestions.keys().exclude(expired_notes).mapBy(q => s.aquestions.get(q)),
      entries: s.entries.keys().exclude(expired_notes).mapBy(e => s.entries.get(e)),
    }
  }

  /// `Appended` without its entity (`append` is a Quint built-in, hence the name): the next eid, the
  /// node's next seq, and the entry's metadata if `keep` (`keeps_meta`: every body but a checkpoint,
  /// a plain note, a system entry other than a digest entry, and a late write, CS-17). The new eid is
  /// `s.next_eid` before the call.
  pure def append_entry(s: State, n: int, at: int, by: int, ty: EntryType, principal: int, keep: bool): State = {
    val eid = s.next_eid
    val seq = s.nodes.get(n).seq + 1
    // 1.8 (§5): an entry touches its node, except a write made with a stale token (H4).
    val s0 = upd_node(s, n, nd => { ...nd, seq: seq })
    val s1 = { ...(if (ty == ELate) s0 else touch_node(s0, n, at)), next_eid: eid + 1 }
    if (keep)
      { ...s1, entries: s1.entries.put(eid, { node: n, seq: seq, at: at, by: by, ty: ty, principal: principal }) }
    else s1
  }

  /// 1.8 (§5): node `n` is touched at `at`, and the Stale marker clears.
  pure def touch_node(s: State, n: int, at: int): State =
    upd_node(s, n, nd => { ...nd, touched: at, stale_fired: false })

  /// `NodeCreated` of a node built by the caller: the id is `next_node`, `due_fired := due <= at`,
  /// the name enters `names`, and an alias at that place leaves; it is touched (1.8).
  pure def ev_node_created(s: State, nd: Node, at: int): State = {
    val id = s.next_node
    val place = (nd.parent, nd.name)
    {
      ...s,
      next_node: id + 1,
      nodes: s.nodes.put(id, {
        ...nd, created_at: at, due_fired: nd.due >= 0 and nd.due <= at, touched: at, stale_fired: false
      }),
      names: s.names.put(place, id),
      aliases: map_remove(s.aliases, place),
    }
  }

  /// `HoldEnded`: the hold leaves; an expiry or a move without a checkpoint counts an attempt (A-9);
  /// a release, an expiry, or a move tags an open node `offer` unless it is a container (A-6,
  /// A-113).
  pure def ev_hold_ended(s: State, t: (int, int), end: HoldEnd): State = {
    val h = s.holds.get(t)
    val n = t._1
    val attempt = not(h.checkpointed) and (end == HeExpired or end == HeEndedByClose(CauseMove))
    val offers = end == HeReleased or end == HeExpired or end == HeEndedByClose(CauseMove)
    val s1 = upd_node({ ...s, holds: map_remove(s.holds, t) }, n, nd =>
      { ...nd, attempts: if (attempt) nd.attempts + 1 else nd.attempts })
    if (offers and is_open(s1, n) and not(is_container(s1, n)))
      upd_node(s1, n, nd => { ...nd, tags: nd.tags.union(Set(OFFER)) })
    else s1
  }

  /// `NodeClosed`: closed with the outcome, version + 1, the wait cleared, `offer` removed; 1.11:
  /// `by` the closing session, -1 for the server (§13).
  pure def ev_node_closed_by(s: State, n: int, at: int, outcome: Outcome, merged_into: int, by: int): State =
    upd_node(s, n, nd => {
      ...nd,
      life: Closed({ at: at, outcome: outcome, merged_into: merged_into, upheld: -1, from_done: false, by: by }),
      version: nd.version + 1,
      wait: NoWait,
      tags: nd.tags.exclude(Set(OFFER)),
    })

  /// `NodeClosed` by the server.
  pure def ev_node_closed(s: State, n: int, at: int, outcome: Outcome, merged_into: int): State =
    ev_node_closed_by(s, n, at, outcome, merged_into, -1)

  /// `MarkSet { n, mark, q, on }`.
  pure def set_mark(s: State, n: int, mark: Mark, q: int, on: bool): State =
    upd_node(s, n, nd =>
      if (mark == MRecheck)
        { ...nd, recheck_marks: if (on) nd.recheck_marks.union(Set(q)) else nd.recheck_marks.exclude(Set(q)) }
      else
        { ...nd, assume_marks: if (on) nd.assume_marks.union(Set(q)) else nd.assume_marks.exclude(Set(q)) })

  /// `WaitSet { n, Some(on) }` at `at`: `fired` is `T <= at` for a time or external wait (A-68).
  pure def set_wait(s: State, n: int, on: WaitOn, at: int): State = {
    val fired = match on { | WTime(t) => t <= at | WExternal(t) => t <= at | _ => false }
    upd_node(s, n, nd => { ...nd, wait: HasWait({ on: on, set_at: at, fired: fired }) })
  }

  /// `RefusalsCleared { n }`: every refusal record on `n` leaves (A-56).
  pure def clear_refusals(s: State, n: int): State =
    { ...s, refusals: s.refusals.keys().filter(k => k._1 != n).mapBy(k => s.refusals.get(k)) }

  // ------------------------------------------------------------------ decisions and assumptions (D.8.17)

  /// `Appended(Decision { settles, basis: [] })` by `by` on `n`, and its `Decision` (`Active`).
  pure def append_decision(s: State, n: int, at: int, by: int, settles: Set[int]): State = {
    val d = s.next_eid
    val s1 = append_entry(s, n, at, by, EDecision, -1, true)
    { ...s1, decisions: s1.decisions.put(d, { node: n, settles: settles, state: DActive, basis: [] }) }
  }

  /// `PqAssumed { q, d }` for a people question still `Expired`, or `UqAssumed { q, d }` for a user
  /// question still `Lapsed`, each only while `assumed_by` is none (their preconditions, D.7.4).
  /// D.8.17's step 3 and `record_assumptions` name `assumed_by = None` for the user question only;
  /// one expiry can mark two waiting nodes (`fire_recheck`), and a decision on the second would then
  /// emit a `PqAssumed` that fails its precondition, an anomaly in Rust. The model skips it, as it
  /// skips the `UqAssumed`; the reading is recorded for a decision (README, "Open questions"). The
  /// model has no event layer, so `anomalies` stays 0, and the replay compares Rust's with it.
  pure def assume_one(s: State, q: int, d: int): State =
    if (s.pquestions.keys().contains(q)
        and s.pquestions.get(q).state == PqExpired and s.pquestions.get(q).assumed_by < 0)
      { ...s, pquestions: s.pquestions.setBy(q, x => { ...x, assumed_by: d }) }
    else if (s.uquestions.keys().contains(q)
        and s.uquestions.get(q).state == UqLapsed and s.uquestions.get(q).assumed_by < 0)
      { ...s, uquestions: s.uquestions.setBy(q, x => { ...x, assumed_by: d }) }
    else s

  /// Step 3 of `talk decide` for decision `d` on `n`: record each assumption of `n.assume_marks`,
  /// and clear the marks (A-15).
  pure def record_assumed(s: State, n: int, d: int): State = {
    val marks = s.nodes.get(n).assume_marks
    val s1 = marks.fold(s, (st, q) => assume_one(st, q, d))
    upd_node(s1, n, nd => { ...nd, assume_marks: nd.assume_marks.exclude(marks) })
  }

  /// `record_assumptions(n, by, text)` (A-104): before a close of a node with assume marks, a
  /// decision that records them.
  pure def record_assumptions(s: State, n: int, by: int, at: int): State =
    if (s.nodes.get(n).assume_marks == Set()) s
    else {
      val d = s.next_eid
      record_assumed(append_decision(s, n, at, by, Set()), n, d)
    }

  /// `assume_now(q)` (A-104, CS-2): a question expires or lapses with no open node to mark; the
  /// server records the assumption on its home node.
  pure def assume_now(s: State, q: int, at: int): State = {
    val home =
      if (s.pquestions.keys().contains(q)) s.pquestions.get(q).node
      else {
        val a = s.uquestions.get(q).about
        if (a >= 0 and has_node(s, a) and not(is_reserved(s, a))) a else USER_DECISIONS
      }
    val d = s.next_eid
    assume_one(append_decision(s, home, at, SERVER, Set()), q, d)
  }

  /// `try_settle(r)` (D.8.8): settle an answered request that a non-retracted decision lists, by the
  /// smallest such decision. Settling is final (A-11).
  pure def try_settle(s: State, r: int): State = {
    val req = s.requests.get(r)
    val ds = settling(s, r)
    if (req.settled == Unsettled and req.answered_by >= 0 and ds != Set())
      { ...s, requests: s.requests.set(r, { ...req, settled: SettledBy({ decision: set_min(ds, -1), answer: req.answered_by }) }) }
    else s
  }

  // ------------------------------------------------------------------ recurring work (D.8.18)

  /// The first slot strictly after `after`: the smallest `offset + k × period > after` (D.10.6).
  /// It always exists, so the model never reaches A-51's "the schedule has no further slot".
  pure def next_slot(r: RepeatSpec, after: int): int =
    if (after < r.offset) r.offset else r.offset + ((after - r.offset) / r.period + 1) * r.period

  pure def repeat_of(s: State, x: int): RepeatSpec =
    match s.nodes.get(x).role { | Series(si) => si.repeat | _ => SEED_REPEAT }

  /// `open_next_occurrence(series, after)`: a new occurrence at the first slot after `after`, named
  /// by its slot (with its own id when that name is live), with the series' content (title and
  /// body, D.8.18; no status), needs, and est, capacity 1, `due` = window end (A-57), and the key
  /// `repeat:<series>:<slot>` unless an open node carries it already (A-90).
  ///
  /// It checks no limit, as D.8.18 has it: L1's depth half is checked by its callers, `create` and
  /// `update` adding `repeat` to an open node (D.8.20). A closed series at `tree.max_depth` that is
  /// reopened therefore gets an occurrence one level deeper than L1 allows (D.8.20's reopen step 7
  /// checks nothing); this is an open question of the specification, recorded for a decision
  /// (README, "Open questions").
  pure def open_next_occurrence(s: State, x: int, after: int, at: int): State = {
    val r = repeat_of(s, x)
    val slot = next_slot(r, after)
    val id = s.next_node
    val key = RepeatSlot((x, slot))
    val series = s.nodes.get(x)
    val nd: Node = {
      parent: x,
      name: if (s.names.keys().contains((x, Slot(slot)))) SlotDup((slot, id)) else Slot(slot),
      origin: OOccurrence,
      created_at: at,
      content: series.content,
      status: 0,
      digest: 0,
      agreements: false,
      kind: "",
      tags: Set(),
      capacity: 1,
      priority: 0,
      est: series.est,
      needs: series.needs,
      due: slot + r.run_within,
      due_fired: false,
      owners: Set(),
      keys: if (key_open(s, key) >= 0) Set() else Set(key),
      links: Set(),
      depends_on: Set(),
      wait: NoWait,
      role: Occurrence({
        series: x, slot: slot, window_end: slot + r.run_within, phase: Waiting, window_fired: false
      }),
      life: Open,
      version: 1,
      digest_version: 1,
      fence: 0,
      seq: 0,
      content_version: 1,
      attempts: 0,
      checkpoints: 0,
      last_checkpoint: -1,
      open_intent: NO_INTENT,
      recheck_marks: Set(),
      assume_marks: Set(),
      touched: at,
      stale_fired: false,
    }
    ev_node_created(s, nd, at)
  }

  /// `SeriesOutcome { series, missed }`.
  pure def series_outcome(s: State, x: int, missed: bool): State =
    upd_node(s, x, nd => match nd.role {
      | Series(si) => { ...nd, role: Series({ ...si, last_missed: missed }) }
      | _ => nd
    })

  // ------------------------------------------------------------------ closing (D.8.18)

  /// Steps 0 to 5 of `close_node` for one node: the assumptions, the `Closed` entry, the end of its
  /// holds, the closing session's intent, the recheck marks, and `NodeClosed`. `by` is a session, or
  /// SERVER.
  pure def close_steps(s: State, n: int, outcome: Outcome, by: int, merged_into: int,
                       cause: EndCause, at: int): State = {
    val s0 = record_assumptions(s, n, by, at)
    val s1 = append_entry(s0, n, at, by, ESystem, -1, false)
    val s2 = holds_on(s1, n).fold(s1, (st, t) => ev_hold_ended(st, t, HeEndedByClose(cause)))
    val s3 = upd_node(s2, n, nd =>
      if (by != SERVER and nd.open_intent.eid >= 0 and nd.open_intent.session == by)
        { ...nd, open_intent: NO_INTENT }
      else nd)
    val s4 = upd_node(s3, n, nd => { ...nd, recheck_marks: Set() })
    ev_node_closed_by(s4, n, at, outcome, merged_into, if (by == SERVER) -1 else by)
  }

  /// `close_node(n, outcome, by, merged_into, cause, text)`, the one helper every close goes
  /// through. `reslot` is `now` when `update {repeat}` drops a waiting occurrence, and -1 otherwise.
  /// Step 6 closes a series' open occurrence (its own steps 7 and 8 do nothing: its series is closed
  /// and it is no signal); step 7 opens an occurrence's next one; step 8 resolves a signal; 1.11's
  /// step 9 (§14) resolves, after a close other than a merge, the duplicate signals of `n` whose
  /// other node is closed and was not merged (`resolve_closed_duplicates`).
  pure def close_node(s: State, n: int, outcome: Outcome, by: int, merged_into: int,
                      cause: EndCause, reslot: int, at: int): State = {
    val s8 = close_role(s, n, outcome, by, merged_into, cause, reslot, at)
    if (merged_into >= 0) s8 else resolve_closed_duplicates(s8, n, at)
  }

  /// 1.11 (§14): the server closes as `done` each open duplicate signal node whose pair holds `n`
  /// and whose other node is closed and was not merged, and the signal resolves (step 8 of that
  /// close). A signal node is no member of another signal's pair, so these closes resolve nothing
  /// further.
  pure def resolve_closed_duplicates(s: State, n: int, at: int): State =
    s.signals.keys()
      .filter(k => k._1 == SigDuplicate and (k._2 == n or k._3 == n))
      .filter(k => { val o = if (k._2 == n) k._3 else k._2 is_closed(s, o) and not(merged(s, o)) })
      .fold(s, (st, k) =>
        if (st.signals.keys().contains(k)) {
          val sig = st.signals.get(k)
          val st1 = close_steps(st, sig, Done, SERVER, -1, CauseClose, at)
          { ...st1, signals: map_remove(st1.signals, k) }
        } else st)

  /// Steps 0 to 8 of `close_node`.
  pure def close_role(s: State, n: int, outcome: Outcome, by: int, merged_into: int,
                      cause: EndCause, reslot: int, at: int): State = {
    val s5 = close_steps(s, n, outcome, by, merged_into, cause, at)
    match s5.nodes.get(n).role {
      | Series(_) =>
          sorted(open_occurrences(s5, n)).foldl(s5, (st, o) =>
            resolve_closed_duplicates(close_steps(st, o, Dropped, by, -1, CauseClose, at), o, at))
      | Occurrence(o) =>
          if (is_open(s5, o.series) and is_series(s5, o.series))
            open_next_occurrence(
              series_outcome(s5, o.series, outcome == Missed),
              o.series,
              if (reslot >= 0) reslot else imax(o.slot, at),
              at)
          else s5
      | RoleSignal(k) =>
          if (s5.signals.keys().contains((k.kind, k.a, k.b)))
            { ...s5, signals: map_remove(s5.signals, (k.kind, k.a, k.b)) }
          else s5
      | Plain => s5
    }
  }

  /// `maybe_missed(o)` (A-80): an open occurrence whose window has passed, with no hold, no open
  /// intent, and no checkpoint, closes as `missed` (R2).
  pure def maybe_missed(s: State, o: int, at: int): State = {
    val nd = s.nodes.get(o)
    val window_fired = match nd.role { | Occurrence(x) => x.window_fired | _ => false }
    if (is_open(s, o) and window_fired and holds_on(s, o) == Set()
        and nd.open_intent.eid < 0 and nd.checkpoints == 0)
      close_node(s, o, Missed, SERVER, -1, CauseClose, -1, at)
    else s
  }

  // ------------------------------------------------------------------ the user side (D.8.12, D.8.13, D.8.14)

  /// `add_digest(item, aud)`: one `System(Digest)` entry on USER_DIGEST per audience principal,
  /// ascending, by the server.
  pure def add_digest(s: State, it: DigestItem, aud: Set[int], at: int): State =
    sorted(aud).foldl(s, (st, a) => {
      val e = st.next_eid
      val st1 = append_entry(st, USER_DIGEST, at, SERVER, ESystem, -1, true)
      { ...st1, digest: st1.digest.put(e, { item: it, to: a, state: DgPending }) }
    })

  /// `create_uq(source, about, has_default, explicit_to, explicit_due)`: a question for the user on
  /// USER_DECISIONS, with the chain fixed now (U1) and its first stage's due. `explicit` is an
  /// interned principal or -1; `explicit_due` a time or -1 (clamped to `at`, A-68); `by` the author
  /// (SERVER inside a firing). The new question's id is `s.next_eid` before the call.
  pure def create_uq(s: State, source: UqSource, about: int, has_default: bool, explicit: int,
                     explicit_due: int, by: int, at: int): State = {
    val stages = chain(s, about, explicit, is_refusal_source(source))
    val due = if (explicit_due >= 0) imax(explicit_due, at) else due_for(s, stages[0], 0, source, at)
    val q = s.next_eid
    val s1 = append_entry(s, USER_DECISIONS, at, by, EQuestion, -1, true)
    {
      ...s1,
      uquestions: s1.uquestions.put(q, {
        source: source, about: about, to: stages, stage: 0, due: due, has_default: has_default,
        state: UqPending, assumed_by: -1
      }),
    }
  }

  /// `e1_check(n)` (D.8.14): when every active class has refused `n` and no refusal question is
  /// pending, ask the user, with the default "drop the request and tell the requester".
  pure def e1_check(s: State, n: int, by: int, at: int): State =
    if (all_refused(s, n, at) and not(covered(s, n))) create_uq(s, SrcRefusal(n), n, true, -1, -1, by, at)
    else s

  /// `refusal_default(n, uq)` (D.8.14): clear the refusals; drop an open, unheld node with a
  /// decision that settles its unsettled requests once they are answered; tell the audience.
  pure def refusal_default(s: State, n: int, uq: int, at: int): State = {
    val s1 = clear_refusals(s, n)
    if (is_open(s1, n) and holds_on(s1, n) == Set()) {
      val rs = unsettled_on(s1, n)
      val drop = DiRefusalDrop({ node: n, question: uq })
      val aud = audience(s1, drop)
      val d = s1.next_eid
      val s2 = record_assumed(append_decision(s1, n, at, SERVER, rs), n, d)
      val s3 = sorted(rs).foldl(s2, (st, r) => try_settle(st, r))
      val s4 = close_node(s3, n, Dropped, SERVER, -1, CauseClose, -1, at)
      add_digest(s4, drop, aud, at)
    } else add_digest(s1, DiDefaultApplied(uq), audience(s1, DiDefaultApplied(uq)), at)
  }

  /// `lapse(uq)` (CS-2): the last stage's due passed without a default. The waits on it resolve by
  /// predicate; the nodes that waited, or else its open `about` node, get `proceed_on_assumption`;
  /// with none, the assumption is recorded at once.
  pure def lapse(s: State, uq: int, at: int): State = {
    val marked = question_waiters(s, uq)
    val aud = audience(s, DiLapsed(uq))
    val s1 = { ...s, uquestions: s.uquestions.setBy(uq, u => { ...u, state: UqLapsed }) }
    val s2 = add_digest(s1, DiLapsed(uq), aud, at)
    val about = s.uquestions.get(uq).about
    val marks =
      if (marked == Set() and about >= 0 and is_open(s2, about) and not(is_reserved(s2, about))) Set(about)
      else marked
    if (marks != Set()) marks.fold(s2, (st, m) => set_mark(st, m, MAssume, uq, true))
    else assume_now(s2, uq, at)
  }

  // ------------------------------------------------------------------ firing (D.8.2)

  /// Lease: `HoldEnded { t, Expired }`, then the missed check of an occurrence.
  pure def fire_lease(s: State, t: (int, int), at: int): State = {
    val s1 = ev_hold_ended(s, t, HeExpired)
    if (is_occurrence(s1, t._1)) maybe_missed(s1, t._1, at) else s1
  }

  /// Wait: `Fired` sets the marker; the wait now resolves by predicate, and its node is touched
  /// (1.8, §5).
  pure def fire_wait(s: State, n: int, at: int): State =
    touch_node(upd_node(s, n, nd => match nd.wait {
      | NoWait => nd
      | HasWait(w) => { ...nd, wait: HasWait({ ...w, fired: true }) }
    }), n, at)

  /// Stale (1.8, §14): `Fired` sets the marker; `stale` turns on where the flag's other conditions
  /// hold.
  pure def fire_stale(s: State, n: int): State = upd_node(s, n, nd => { ...nd, stale_fired: true })

  /// Recheck of people question `q`: the next recheck, the expiry (non-blocking), or the escalation
  /// to the user (blocking).
  pure def fire_recheck(s: State, q: int, at: int): State = {
    val pq = s.pquestions.get(q)
    val resolving = s.nodes.keys().filter(m => match s.nodes.get(m).wait {
      | NoWait => false
      | HasWait(w) => w.on == WQuestion({ q: q, rechecks_at_set: pq.rechecks })
    })
    val marked =
      if (resolving != Set()) resolving else if (is_open(s, pq.node)) Set(pq.node) else Set()
    val k = pq.rechecks + 1
    if (k < QUESTION_MAX_RECHECKS) {
      val s1 = { ...s, pquestions: s.pquestions.set(q, { ...pq, rechecks: k, recheck_at: at + QUESTION_RECHECK }) }
      marked.fold(s1, (st, m) => set_mark(st, m, MRecheck, q, true))
    } else if (not(pq.blocking)) {
      val aud = audience(s, DiAssumption(q))
      val s1 = { ...s, pquestions: s.pquestions.set(q, { ...pq, state: PqExpired, rechecks: k, lapse: LapseExpired }) }
      val s2 = add_digest(s1, DiAssumption(q), aud, at)
      if (marked != Set()) marked.fold(s2, (st, m) => set_mark(st, m, MAssume, q, true))
      else assume_now(s2, q, at)
    } else {
      val uq = s.next_eid
      val s1 = create_uq(s, SrcEscalation(q), pq.node, false, -1, -1, SERVER, at)
      val s2 = { ...s1, pquestions: s1.pquestions.setBy(q, x =>
        { ...x, state: PqEscalated(uq), rechecks: k, lapse: LapseEscalated }) }
      resolving.fold(s2, (st, w) => set_wait(st, w, WUserQuestion(uq), at))
    }
  }

  /// UserQuestion: with a default, the default applies (`Defaulted` entry, `UqDefaulted`, then the
  /// digest, or the refusal default); without, the next stage (`UqReaddressed`) or the lapse (CS-2).
  pure def fire_user_question(s: State, uq: int, at: int): State = {
    val u = s.uquestions.get(uq)
    if (u.has_default) {
      val s1 = append_entry(s, USER_DECISIONS, at, SERVER, ESystem, -1, false)
      val s2 = { ...s1, uquestions: s1.uquestions.set(uq, { ...u, state: UqDefaulted }) }
      match u.source {
        | SrcAgent(_) => add_digest(s2, DiDefaultApplied(uq), audience(s2, DiDefaultApplied(uq)), at)
        | SrcRefusal(n) => refusal_default(s2, n, uq, at)
        | SrcEscalation(_) => s2
      }
    } else if (u.stage + 1 < u.to.length()) {
      val k = u.stage + 1
      { ...s, uquestions: s.uquestions.set(uq, { ...u, stage: k, due: due_for(s, u.to[k], k, u.source, at) }) }
    } else lapse(s, uq, at)
  }

  /// Commitment: `Fired` sets `followup_fired`.
  pure def fire_commitment(s: State, c: int): State =
    { ...s, commitments: s.commitments.setBy(c, x => { ...x, followup_fired: true }) }

  /// OccurrenceSlot: `OccurrenceAvailable` (waiting → available).
  pure def fire_slot(s: State, o: int): State =
    upd_node(s, o, nd => match nd.role {
      | Occurrence(x) => { ...nd, role: Occurrence({ ...x, phase: Available }) }
      | _ => nd
    })

  /// OccurrenceWindow: `Fired` sets `window_fired`; then the missed check.
  pure def fire_window(s: State, o: int, at: int): State = {
    val s1 = upd_node(s, o, nd => match nd.role {
      | Occurrence(x) => { ...nd, role: Occurrence({ ...x, window_fired: true }) }
      | _ => nd
    })
    maybe_missed(s1, o, at)
  }

  /// RefusalExpiry of a record: the entries that expired at `at` go, and the record with its last.
  pure def fire_refusal(s: State, n: int, c: str, at: int): State = {
    val r = s.refusals.get((n, c))
    val kept = r.by.keys().filter(x => r.by.get(x).expires_at > at).mapBy(x => r.by.get(x))
    if (kept.keys() == Set()) { ...s, refusals: map_remove(s.refusals, (n, c)) }
    else { ...s, refusals: s.refusals.set((n, c), { ...r, by: kept }) }
  }

  /// RefusalExpiry of a class: `Fired` removes the caps profiles whose window ended, and the class
  /// with its last (CS-7); then `e1_check` for each refused node, ascending.
  pure def fire_class(s: State, c: str, at: int): State = {
    val ca = s.classes.get(c)
    val kept = ca.caps_seen.keys().filter(k => ca.caps_seen.get(k) > at).mapBy(k => ca.caps_seen.get(k))
    val s1 =
      if (kept.keys() == Set()) { ...s, classes: map_remove(s.classes, c) }
      else { ...s, classes: s.classes.set(c, { caps_seen: kept }) }
    sorted(refused_nodes(s1)).foldl(s1, (st, n) => e1_check(st, n, SERVER, at))
  }

  /// Due: `Fired` sets `due_fired`; `overdue` turns on.
  pure def fire_due(s: State, n: int): State = upd_node(s, n, nd => { ...nd, due_fired: true })

  /// Pause: `PauseExpired`, then `Appended(System(PauseExpired))` on the scope.
  pure def fire_pause(s: State, p: int, at: int): State =
    append_entry({ ...s, pauses: map_remove(s.pauses, p) }, p, at, SERVER, ESystem, -1, false)

  /// Silence: `Fired` sets `silent`; then each pending `said` of the session becomes unconfirmed.
  pure def fire_silence(s: State, x: int): State = {
    val s1 = { ...s, sessions: s.sessions.setBy(x, se => { ...se, silent: true }) }
    {
      ...s1,
      saids: s1.saids.keys().mapBy(d => {
        val sd = s1.saids.get(d)
        if (sd.session == x and sd.state == SdPending) { ...sd, state: SdUnconfirmed } else sd
      }),
    }
  }

  /// Retire: `SessionRetired` removes the session (CS-8).
  pure def fire_retire(s: State, x: int): State = { ...s, sessions: map_remove(s.sessions, x) }

  /// `fire(s, k)`: the arm of `k`'s kind, with every event at `k.at` (§17).
  pure def fire(s: State, k: DeadlineKey): State = {
    val at = k.at
    val s0 = at_event(s, at)
    val p = owner_payload(k.owner)
    if (k.kind == K_LEASE) fire_lease(s0, (p / 100, p % 100), at)
    else if (k.kind == K_WAIT) fire_wait(s0, p, at)
    else if (k.kind == K_RECHECK) fire_recheck(s0, p, at)
    else if (k.kind == K_USER_QUESTION) fire_user_question(s0, p, at)
    else if (k.kind == K_COMMITMENT) fire_commitment(s0, p)
    else if (k.kind == K_OCCURRENCE_SLOT) fire_slot(s0, p)
    else if (k.kind == K_OCCURRENCE_WINDOW) fire_window(s0, p, at)
    else if (k.kind == K_REFUSAL_EXPIRY and owner_rank(k.owner) == O_REFUSAL)
      fire_refusal(s0, p / 10, class_name(p % 10), at)
    else if (k.kind == K_REFUSAL_EXPIRY) fire_class(s0, class_name(p), at)
    else if (k.kind == K_DUE) fire_due(s0, p)
    else if (k.kind == K_PAUSE) fire_pause(s0, p, at)
    else if (k.kind == K_SILENCE) fire_silence(s0, p)
    else if (k.kind == K_RETIRE) fire_retire(s0, p)
    else if (k.kind == K_STALE) fire_stale(s0, p)
    else s0
  }

  // ------------------------------------------------------------------ settling (D.8.2, D.10.4)

  /// The result of a settle:
  /// - `stuck`: D.8.2's `Stuck(k)`. A firing of `k` left `k` in the queue, or queued a key that
  ///   precedes `k` in the queue order (one at an earlier time, or at `k.at` before `k`), which
  ///   `check_fire` forbids ("no new key at or before `key.at`"). The fold stops after that firing,
  ///   as Rust's `settle` returns there. A firing arm that fails to remove its own key is a model
  ///   or core bug, never a bound to raise, so `inv_T4` fails on `stuck` (WP-A3);
  /// - `saturated`: no firing was stuck, and a key at or before the target remained after
  ///   `max_firings` firings. `inv_settle_bound` fails on it, so a truncated fold is never mistaken
  ///   for a protocol violation (D.10.4).
  ///
  /// A queued key at `k.at` that sorts after `k` is not seen by `stuck`: telling it from a key that
  /// was queued before the firing would need the whole queue twice per firing. `settleStuckTest`
  /// shows what is seen.
  type Settle = { state: State, saturated: bool, stuck: bool }

  /// `settle` with the firing function `f` in place of `fire`, so that the tests can check the
  /// `stuck` detection with a broken arm. Each pass reads `first_key` once: the key to fire, and the
  /// check of the previous firing (its key must now lie strictly below the queue's minimum).
  pure def settle_by(s: State, now: int, max_firings: int, f: (State, DeadlineKey) => State): Settle = {
    val t = imax(now, s.last_now)
    // Once a firing is stuck, no later pass fires: the state no longer changes, so each pass finds
    // the same key and the same check. Once a pass finds nothing due (`idle`), no later pass reads
    // the queue again: the state no longer changes, so each would find the same key. This saves the
    // evaluator a `first_key` per pass (simulation, tests, TLC); Apalache encodes every pass anyway.
    val out = range(0, max_firings).foldl({ st: s, last: NO_KEY, stuck: false, idle: false }, (acc, i) =>
      if (acc.idle or acc.stuck) acc
      else {
        val k = first_key(acc.st)
        if (acc.last != NO_KEY and k != NO_KEY and not(key_lt(acc.last, k))) { ...acc, stuck: true }
        else if (k.at >= 0 and k.at <= t) { st: f(acc.st, k), last: k, stuck: false, idle: false }
        else { ...acc, idle: true }
      })
    val rest = first_key(out.st)
    val is_stuck = out.stuck or (out.last != NO_KEY and rest != NO_KEY and not(key_lt(out.last, rest)))
    { state: out.st, saturated: not(is_stuck) and rest.at >= 0 and rest.at <= t, stuck: is_stuck }
  }

  /// `settle(s, now)`: a bounded deterministic fold of at most `max_firings` firings, each of the
  /// queue's minimum while it lies at or before `max(now, last_now)`, with the `stuck` and
  /// `saturated` flags above. Settling does not record the `now` it was called with: only firings
  /// move `last_now` (A-101). Apalache needs `max_firings` to be a literal, or a constant or pure
  /// value bound to one, at every call.
  pure def settle(s: State, now: int, max_firings: int): Settle =
    settle_by(s, now, max_firings, (st, k) => fire(st, k))
}
