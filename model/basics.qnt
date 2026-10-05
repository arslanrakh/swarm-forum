// -*- mode: Bluespec; -*-
/**
 * Basic helpers of the model (D.10.1): reserved ids, genesis, ancestors, name resolution,
 * `resolve_merged`, holds and `live`, waits and `waiting`, `paused`, conversation keys and
 * `conv_node`, the directory of people (aliases, `gone`, `rep`), the addressee chain of a question
 * for the user with `due_for` and `p90_gap` (D.8.12), the digest audience (D.8.13), the refusal
 * predicates (D.8.14), the staleness of a basis reference (D.8.17), and the FNV-1a-64 tie-break
 * (D.2.6).
 *
 * Everything here is a pure function of the state: no definition changes it. The transitions are in
 * `deadlines.qnt` (those a firing needs) and in `commands.qnt`.
 */
module basics {
  import types.* from "./types"
  import params.* from "./params"

  // ------------------------------------------------------------------ reserved ids (D.2.2)

  pure val ROOT = 0             // "/"
  pure val USER = 1             // "/@user"
  pure val USER_DECISIONS = 2   // "/@user/decisions"
  pure val USER_DIGEST = 3      // "/@user/digest"
  pure val SYS = 4              // "/sys"
  pure val SIGNALS = 5          // "/sys/signals"
  pure val PEOPLE = 6           // "/@people"
  pure val TEAMS = 7            // "/@teams"
  pure val FIRST_FREE_NODE = 8

  /// The reserved principal `user` (CS-1).
  pure val USER_PRINCIPAL = 0
  pure val USER_KEY = "user"

  /// `EntryMeta.by` of an entry the server wrote (`Author::Server`); sessions start at 1.
  pure val SERVER = 0

  pure val OFFER = "offer"
  pure val GONE = "gone"

  pure val NO_INTENT: Intent = { eid: -1, session: -1 }

  // ------------------------------------------------------------------ generic helpers

  // Apalache encodes these. It needs every `range` to have literal bounds, so a loop over a list or
  // a set of unknown length is a fold over that list or set, never `range(0, n)`, `indices()`, or
  // `slice`, and no exponent is a variable.

  pure def imax(a: int, b: int): int = if (a >= b) a else b
  pure def imin(a: int, b: int): int = if (a <= b) a else b

  /// Quint 0.33 has no map removal.
  pure def map_remove(m: a -> b, k: a): a -> b = m.keys().exclude(Set(k)).mapBy(x => m.get(x))

  pure def get_or(m: a -> b, k: a, d: b): b = if (m.keys().contains(k)) m.get(k) else d

  /// Whether the list `l` contains `x`.
  pure def has(l: List[a], x: a): bool = l.foldl(false, (acc, y) => acc or y == x)

  /// Whether some element of the list `l` satisfies `p`.
  pure def any_of(l: List[a], p: a => bool): bool = l.foldl(false, (acc, y) => acc or p(y))

  pure def to_set(l: List[a]): Set[a] = l.foldl(Set(), (acc, x) => acc.union(Set(x)))

  /// The smallest element of `xs`, or `d` when it is empty.
  pure def set_min(xs: Set[int], d: int): int =
    xs.fold(d, (acc, x) => if (acc == d or x < acc) x else acc)

  /// The elements of `xs`, ascending: |xs| times, the smallest of those not taken yet.
  pure def sorted(xs: Set[int]): List[int] =
    xs.fold({ rest: xs, out: [] }, (acc, x) => {
      val m = set_min(acc.rest, -1)
      { rest: acc.rest.exclude(Set(m)), out: acc.out.append(m) }
    }).out

  /// How many elements of `l` are at most `x`.
  pure def count_le(l: List[int], x: int): int = l.foldl(0, (c, y) => if (y <= x) c + 1 else c)

  /// The `k`-th smallest element of `l` (from 0, duplicates counted), or -1 if `l` is shorter.
  pure def kth_smallest(l: List[int], k: int): int =
    l.foldl({ found: false, v: -1 }, (acc, y) =>
      if (count_le(l, y) > k and (not(acc.found) or y < acc.v)) { found: true, v: y } else acc).v

  /// The elements of `l` ascending, duplicates kept.
  pure def sort_list(l: List[int]): List[int] =
    l.foldl({ k: 0, out: [] }, (acc, y) => { k: acc.k + 1, out: acc.out.append(kth_smallest(l, acc.k)) }).out

  /// The first element of `l` that satisfies `p`, or -1. For lists of ids, which are never -1.
  pure def first_where(l: List[int], p: int => bool): int =
    l.foldl(-1, (acc, x) => if (acc == -1 and p(x)) x else acc)

  // ------------------------------------------------------------------ genesis (D.7.4)

  /// A reserved node as Genesis makes it: origin `Reserved`, open, capacity 0, title = name
  /// (content 0), no status, versions 1, no owners.
  pure def reserved_node(parent: int, name: str, at: int): Node = {
    parent: parent,
    name: Lit(name),
    origin: OReserved,
    created_at: at,
    content: 0,
    status: 0,
    digest: 0,
    agreements: false,
    kind: "",
    tags: Set(),
    capacity: 0,
    priority: 0,
    est: -1,
    needs: Set(),
    due: -1,
    due_fired: false,
    owners: Set(),
    keys: Set(),
    links: Set(),
    depends_on: Set(),
    wait: NoWait,
    role: Plain,
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

  /// The state after the `Genesis` event at `at`: the eight reserved nodes, their names, and the
  /// principal `user` (CS-1).
  pure def genesis(at: int): State = {
    val reserved = Map(
      ROOT -> reserved_node(-1, "", at),
      USER -> reserved_node(ROOT, "@user", at),
      USER_DECISIONS -> reserved_node(USER, "decisions", at),
      USER_DIGEST -> reserved_node(USER, "digest", at),
      SYS -> reserved_node(ROOT, "sys", at),
      SIGNALS -> reserved_node(SYS, "signals", at),
      PEOPLE -> reserved_node(ROOT, "@people", at),
      TEAMS -> reserved_node(ROOT, "@teams", at)
    )
    {
      last_now: at,
      next_node: FIRST_FREE_NODE,
      next_eid: 1,
      next_session: 1,
      next_principal: 1,
      anomalies: 0,
      principal_ids: Map(USER_KEY -> USER_PRINCIPAL),
      user_msgs: Map(),
      nodes: reserved,
      names: reserved.keys().exclude(Set(ROOT))
        .fold(Map(), (m, n) => m.put((reserved.get(n).parent, reserved.get(n).name), n)),
      aliases: Map(),
      entries: Map(),
      holds: Map(),
      requests: Map(),
      pquestions: Map(),
      aquestions: Map(),
      uquestions: Map(),
      commitments: Map(),
      saids: Map(),
      decisions: Map(),
      retracted: Set(),
      objections: Map(),
      user_entries: Map(),
      digest: Map(),
      notes: Map(),
      approvals: Map(),
      refusals: Map(),
      classes: Map(),
      signals: Map(),
      pauses: Map(),
      sessions: Map(),
      ops: Map(),
    }
  }

  // ------------------------------------------------------------------ nodes (D.4.3)

  pure def has_node(s: State, n: int): bool = s.nodes.keys().contains(n)

  pure def node_is_open(nd: Node): bool = nd.life == Open

  pure def is_open(s: State, n: int): bool = has_node(s, n) and node_is_open(s.nodes.get(n))

  pure def is_closed(s: State, n: int): bool = has_node(s, n) and not(node_is_open(s.nodes.get(n)))

  pure def is_reserved(s: State, n: int): bool = has_node(s, n) and s.nodes.get(n).origin == OReserved

  /// `merged(n)` of D.8.3: closed with `merged_into`; never a candidate (A-94).
  pure def merged(s: State, n: int): bool =
    has_node(s, n) and match s.nodes.get(n).life {
      | Open => false
      | Closed(c) => c.merged_into >= 0
    }

  /// 1.8 (§5, §11.3): closed and marked upheld; never a candidate, and a take of it is `closed`.
  pure def upheld(s: State, n: int): bool =
    has_node(s, n) and match s.nodes.get(n).life {
      | Open => false
      | Closed(c) => c.upheld >= 0
    }

  /// The node's outcome as a basis reference records it (`life_at`, D.8.17).
  pure def life_at(s: State, n: int): LifeAt =
    match s.nodes.get(n).life {
      | Open => AtOpen
      | Closed(c) => AtClosed(c.outcome)
    }

  pure def role_is_series(r: Role): bool = match r { | Series(_) => true | _ => false }
  pure def role_is_occurrence(r: Role): bool = match r { | Occurrence(_) => true | _ => false }

  pure def is_series(s: State, n: int): bool = has_node(s, n) and role_is_series(s.nodes.get(n).role)
  pure def is_occurrence(s: State, n: int): bool =
    has_node(s, n) and role_is_occurrence(s.nodes.get(n).role)

  /// The series of an occurrence, or -1.
  pure def series_of(s: State, n: int): int =
    if (not(has_node(s, n))) -1
    else match s.nodes.get(n).role { | Occurrence(o) => o.series | _ => -1 }

  /// The open occurrences of series `x` (K6: exactly one while `x` is open and a series).
  pure def open_occurrences(s: State, x: int): Set[int] =
    s.nodes.keys().filter(o => is_open(s, o) and series_of(s, o) == x)

  /// The occurrences the server has made, open, closed, or detached (origin `Occurrence`). The
  /// model has one series, so this is what the bound "at most 3 occurrences of the one series"
  /// counts (§28, D.10.7, MAX_OCCURRENCES).
  pure def occurrences_made(s: State): Set[int] =
    s.nodes.keys().filter(o => s.nodes.get(o).origin == OOccurrence)

  /// Whether one more occurrence stays within MAX_OCCURRENCES. The time actions open occurrences
  /// only as slots pass, which the seed's series keeps within the bound by its period (`params.qnt`,
  /// `seedOccurrenceBoundTest`); a command that opens one at `now` (`update {repeat}` dropping a
  /// waiting occurrence, adding `repeat` to a plain node, a reopen of a closed series) is guarded by
  /// this in WP-A2, as D.10.7's guards are.
  pure def may_open_occurrence(s: State): bool = occurrences_made(s).size() < MAX_OCCURRENCES

  /// `is_directory(n)`: a person or a team of the directory, a child of PEOPLE or TEAMS (A-113).
  pure def is_directory(s: State, n: int): bool =
    has_node(s, n) and (s.nodes.get(n).parent == PEOPLE or s.nodes.get(n).parent == TEAMS)

  /// `is_container(n)`: reserved, a series, or a person or team (A-29, A-113). Containers get no
  /// `offer` tag when a hold ends.
  pure def is_container(s: State, n: int): bool =
    is_reserved(s, n) or is_series(s, n) or is_directory(s, n)

  /// `occ_locked(o)`: an occurrence of a series that is open and still a series (§12, A-13).
  pure def occ_locked(s: State, n: int): bool = {
    val x = series_of(s, n)
    x >= 0 and is_open(s, x) and is_series(s, x)
  }

  // ------------------------------------------------------------------ the tree (D.8.4)

  /// The ancestors of `n`, from its parent to ROOT, at most WALK_MAX steps.
  pure def ancestors(s: State, n: int): List[int] = {
    val walk = range(0, WALK_MAX).foldl({ cur: n, out: [] }, (acc, i) =>
      if (acc.cur < 0 or not(has_node(s, acc.cur))) acc
      else {
        val p = s.nodes.get(acc.cur).parent
        if (p < 0) { cur: -1, out: acc.out } else { cur: p, out: acc.out.append(p) }
      })
    walk.out
  }

  /// `n` followed by its ancestors, nearest first.
  pure def ancestors_or_self(s: State, n: int): List[int] = [n].concat(ancestors(s, n))

  /// ROOT is at depth 0.
  pure def depth(s: State, n: int): int = ancestors(s, n).length()

  /// Whether `n` is `a` or a descendant of `a`.
  pure def is_under(s: State, n: int, a: int): bool = n == a or has(ancestors(s, n), a)

  pure def children(s: State, n: int): Set[int] =
    s.nodes.keys().filter(c => s.nodes.get(c).parent == n)

  pure def open_children(s: State, n: int): Set[int] = children(s, n).filter(c => is_open(s, c))

  /// Resolution of one path step (D.8.4): the live name, else the alias, else -1.
  pure def resolve(s: State, parent: int, name: Name): int =
    if (s.names.keys().contains((parent, name))) s.names.get((parent, name))
    else if (s.aliases.keys().contains((parent, name))) s.aliases.get((parent, name))
    else -1

  /// Follows `merged_into` (links and basis references resolve so, A-88), at most WALK_MAX steps.
  pure def resolve_merged(s: State, m: int): int =
    range(0, WALK_MAX).foldl(m, (x, i) =>
      if (not(has_node(s, x))) x
      else match s.nodes.get(x).life {
        | Open => x
        | Closed(c) => if (c.merged_into >= 0) c.merged_into else x
      })

  // ------------------------------------------------------------------ holds (D.4.4)

  /// `live(h, now)` of §26.
  pure def live(h: Hold, now: int): bool = now < h.lease_until

  /// The tokens of the holds on `n` that are in the map (what Rust's `ix.node[n].live` counts:
  /// holds leave the map when they end).
  pure def holds_on(s: State, n: int): Set[(int, int)] = s.holds.keys().filter(t => t._1 == n)

  /// The tokens of the live holds on `n` at `now`.
  pure def live_holds_on(s: State, n: int, now: int): Set[(int, int)] =
    holds_on(s, n).filter(t => live(s.holds.get(t), now))

  /// The tokens of the live holds that session `sess` took.
  pure def live_holds_of(s: State, sess: int, now: int): Set[(int, int)] =
    s.holds.keys().filter(t => s.holds.get(t).session == sess and live(s.holds.get(t), now))

  /// `attended(n)`: a live hold on `n` or a descendant (§14 "unattended" negated).
  pure def attended(s: State, n: int, now: int): bool =
    s.holds.keys().exists(t => live(s.holds.get(t), now) and is_under(s, t._1, n))

  /// `held_self(n)`: a live hold on `n` itself.
  pure def held_self(s: State, n: int, now: int): bool = live_holds_on(s, n, now) != Set()

  // ------------------------------------------------------------------ waits (D.8.3)

  /// `resolved(w, now)`. A question that is not in the state counts as resolved.
  pure def resolved(s: State, w: Wait, now: int): bool =
    match w.on {
      | WQuestion(x) =>
          if (not(s.pquestions.keys().contains(x.q))) true
          else {
            val pq = s.pquestions.get(x.q)
            pq.state != PqOpen or pq.rechecks > x.rechecks_at_set
          }
      | WUserQuestion(q) =>
          if (not(s.uquestions.keys().contains(q))) true
          else s.uquestions.get(q).state != UqPending
      | WNode(t) => is_closed(s, t)
      | WTime(t) => now >= t
      | WExternal(t) => now >= t
    }

  /// `waiting(n, now)`: the node has an unresolved wait.
  pure def waiting(s: State, n: int, now: int): bool =
    has_node(s, n) and match s.nodes.get(n).wait {
      | NoWait => false
      | HasWait(w) => not(resolved(s, w, now))
    }

  /// `ix.waiters[Question(q)]`: the nodes whose wait names question `q` (people or user).
  pure def question_waiters(s: State, q: int): Set[int] =
    s.nodes.keys().filter(n => match s.nodes.get(n).wait {
      | NoWait => false
      | HasWait(w) => match w.on {
          | WQuestion(x) => x.q == q
          | WUserQuestion(u) => u == q
          | _ => false
        }
    })

  /// `ix.waiters[Node(t)]`: the nodes whose wait is a `{node}` wait on `t`.
  pure def node_waiters(s: State, t: int): Set[int] =
    s.nodes.keys().filter(n => match s.nodes.get(n).wait {
      | NoWait => false
      | HasWait(w) => w.on == WNode(t)
    })

  // ------------------------------------------------------------------ pause (D.8.21)

  /// `paused(n, now)`: an active pause on `n` or an ancestor.
  pure def paused(s: State, n: int, now: int): bool =
    ancestors_or_self(s, n).foldl(false, (acc, a) =>
      acc or (s.pauses.keys().contains(a) and s.pauses.get(a).until > now))

  // ------------------------------------------------------------------ keys and conversations (D.8.8)

  /// A conversation key's scheme is a conversation scheme (CS-36): `Conv` keys, and `Msg` keys,
  /// which are message ids in a conversation's scheme (D.2.4). Object keys (`Obj`), and `user:`,
  /// `repeat:`, and `person:` keys, are not: they name objects, and `conv_key` skips them.
  pure def is_conversation(k: Key): bool = match k { | Conv(_) => true | Msg(_) => true | _ => false }

  /// The open nodes carrying `k` (K1: at most one).
  pure def open_carriers(s: State, k: Key): Set[int] =
    s.nodes.keys().filter(n => is_open(s, n) and s.nodes.get(n).keys.contains(k))

  /// `ix.keys_open[k]`, or -1.
  pure def key_open(s: State, k: Key): int = set_min(open_carriers(s, k), -1)

  /// The result of `conv_key`: the key, or the reason it has none.
  type ConvKey = CkOk(Key) | CkErr(Why)

  /// `conv_key(s, at, where)`: `where` if given; else the only conversation key of the nearest node
  /// from `at` up that carries any (A-30).
  pure def conv_key(s: State, at: int, where_key: OptKey): ConvKey =
    match where_key {
      | SomeKey(k) => CkOk(k)
      | NoKey => {
          val m = first_where(ancestors_or_self(s, at), n =>
            s.nodes.get(n).keys.exists(k => is_conversation(k)))
          if (m < 0) CkErr(WhyNoConversationKey)
          else {
            val ks = s.nodes.get(m).keys.filter(k => is_conversation(k))
            if (ks.size() == 1) CkOk(ks.fold(Conv(""), (acc, k) => k)) else CkErr(WhyAmbiguousConversationKey)
          }
        }
    }

  /// `conv_node(s, at, k)` of §9.1: (1) the nearest node from `at` up whose keys contain `k`, open
  /// or closed; (2) else the open carrier of `k`; (3) else `at`, where execution adds `k` first.
  /// `case` says which.
  pure def conv_node(s: State, at: int, k: Key): { node: int, case: int } = {
    val near = first_where(ancestors_or_self(s, at), n => s.nodes.get(n).keys.contains(k))
    if (near >= 0) { node: near, case: 1 }
    else if (key_open(s, k) >= 0) { node: key_open(s, k), case: 2 }
    else { node: at, case: 3 }
  }

  // ------------------------------------------------------------------ principals and the directory (D.8.11)

  /// The key of principal `p` (one or none).
  pure def keys_of_principal(s: State, p: int): Set[str] =
    s.principal_ids.keys().filter(k => s.principal_ids.get(k) == p)

  /// A person: an open child of PEOPLE (D.2.2).
  pure def is_person_node(s: State, m: int): bool = is_open(s, m) and s.nodes.get(m).parent == PEOPLE

  /// A team: an open child of TEAMS (D.2.2).
  pure def is_team_node(s: State, m: int): bool = is_open(s, m) and s.nodes.get(m).parent == TEAMS

  /// `person_of[p]`: the person node carrying `person:<key of p>`, or -1 (K1: at most one).
  pure def person_of(s: State, p: int): int = {
    val ks = keys_of_principal(s, p)
    set_min(
      s.nodes.keys().filter(m =>
        is_person_node(s, m) and ks.exists(k => s.nodes.get(m).keys.contains(Person(k)))),
      -1)
  }

  /// `aliases_of[m]`: the interned principals whose `person:` key person node `m` carries.
  pure def aliases_of(s: State, m: int): Set[int] =
    if (not(is_person_node(s, m))) Set()
    else s.principal_ids.keys()
      .filter(k => s.nodes.get(m).keys.contains(Person(k)))
      .map(k => s.principal_ids.get(k))

  /// `aliases(p)`: the aliases of `p`'s person, or `{p}`.
  pure def aliases(s: State, p: int): Set[int] = {
    val m = person_of(s, p)
    if (m >= 0) aliases_of(s, m) else Set(p)
  }

  pure def same_person(s: State, p: int, q: int): bool =
    p == q or { val m = person_of(s, p) m >= 0 and m == person_of(s, q) }

  /// `gone(p)`: `p` has a person tagged `gone`.
  pure def gone(s: State, p: int): bool = {
    val m = person_of(s, p)
    m >= 0 and s.nodes.get(m).tags.contains(GONE)
  }

  /// `rep(m)`: a person's stage in a chain, its lowest alias id; -1 for a person without aliases.
  pure def rep(s: State, m: int): int = set_min(aliases_of(s, m), -1)

  /// Appends `p` unless `out` has `p` or an alias of it.
  pure def push_person(s: State, out: List[int], p: int): List[int] =
    if (p < 0 or any_of(out, q => same_person(s, q, p))) out else out.append(p)

  // ------------------------------------------------------------------ requests (D.8.8, D.8.12)

  pure def is_unsettled(r: Request): bool = r.settled == Unsettled

  /// `items(n).unsettled`.
  pure def unsettled_on(s: State, n: int): Set[int] =
    s.requests.keys().filter(r => s.requests.get(r).node == n and is_unsettled(s.requests.get(r)))

  pure def requester_principal(r: Request): int = match r.requester { | ReqUser(p) => p | ReqExternal => -1 }

  /// The unsettled user requests on `n`, oldest first.
  pure def user_requests_on(s: State, n: int): List[int] =
    sorted(unsettled_on(s, n).filter(r => requester_principal(s.requests.get(r)) >= 0))

  /// `requesters_on(m, skip_gone)`: the requesters of the unsettled user requests on `m`, oldest
  /// request first, without a second alias of one person, skipping gone persons if `skip_gone`,
  /// at most UQ_REQUESTERS_MAX.
  pure def requesters_on(s: State, m: int, skip_gone: bool): List[int] =
    user_requests_on(s, m).foldl([], (out, r) => {
      val p = requester_principal(s.requests.get(r))
      if (out.length() >= UQ_REQUESTERS_MAX or (skip_gone and gone(s, p))) out
      else push_person(s, out, p)
    })

  /// `nearest_requested(n)`: the nearest node among `n` and its ancestors with an unsettled user
  /// request, or -1 (also for `n = -1`).
  pure def nearest_requested(s: State, n: int): int =
    if (n < 0 or not(has_node(s, n))) -1
    else first_where(ancestors_or_self(s, n), m => user_requests_on(s, m).length() > 0)

  /// `requesters(n)`: stage 2 of a chain (§8.4).
  pure def requesters(s: State, n: int): List[int] = {
    val m = nearest_requested(s, n)
    if (m >= 0) requesters_on(s, m, true) else []
  }

  /// `ix.settling[r]`: the decisions that list `r` and are not retracted (§9.4: a superseded one
  /// still counts).
  pure def settling(s: State, r: int): Set[int] =
    s.decisions.keys().filter(d => {
      val dd = s.decisions.get(d)
      dd.settles.contains(r) and match dd.state { | DRetracted(_) => false | _ => true }
    })

  // ------------------------------------------------------------------ owners and the chain (D.8.12)

  /// The rep of a person node that is not gone, as a list of at most one.
  pure def person_stage(s: State, m: int): List[int] =
    if (is_person_node(s, m) and not(s.nodes.get(m).tags.contains(GONE)) and rep(s, m) >= 0)
      [rep(s, m)]
    else []

  /// `owners(n)`: the directory's owners of `n` (D.2.2, D-3): the owners of the nearest node among
  /// `n` (ROOT for -1) and its ancestors with non-empty `owners`, then ROOT's unless that node is
  /// ROOT; a person stands for itself, a team for the persons of its own `owners` (A-116); gone
  /// persons skipped; no second alias of one person; at most UQ_OWNERS_MAX.
  pure def owners(s: State, n: int): List[int] = {
    val start = if (n < 0 or not(has_node(s, n))) ROOT else n
    val owned = first_where(ancestors_or_self(s, start), m => s.nodes.get(m).owners != Set())
    val o = if (owned >= 0) sorted(s.nodes.get(owned).owners) else []
    val r = if (owned != ROOT) sorted(s.nodes.get(ROOT).owners) else []
    val stages = o.concat(r).foldl([], (acc, x) =>
      if (is_person_node(s, x)) acc.concat(person_stage(s, x))
      else if (is_team_node(s, x))
        sorted(s.nodes.get(x).owners).foldl(acc, (acc2, m) => acc2.concat(person_stage(s, m)))
      else acc)
    stages.foldl([], (out, p) => if (out.length() >= UQ_OWNERS_MAX) out else push_person(s, out, p))
  }

  /// Appends `p` to a chain unless the chain has `p` or an alias of it, or `p` is `user` or an alias
  /// of `user`, which is always the last stage.
  pure def push_stage(s: State, out: List[int], p: int): List[int] =
    if (same_person(s, p, USER_PRINCIPAL)) out else push_person(s, out, p)

  /// `chain(about, explicit, refusal)` (D.8.12): the explicit addressee (-1 for none), even when
  /// gone; the requesters (of `about` itself for a refusal question); the owners; then `user`.
  pure def chain(s: State, about: int, explicit: int, refusal: bool): List[int] = {
    val c0 = if (explicit >= 0) push_stage(s, [], explicit) else []
    val reqs = if (refusal) requesters_on(s, about, true) else requesters(s, about)
    val c1 = reqs.foldl(c0, (out, p) => push_stage(s, out, p))
    val c2 = owners(s, about).foldl(c1, (out, p) => push_stage(s, out, p))
    c2.append(USER_PRINCIPAL)
  }

  // ------------------------------------------------------------------ due of a stage (D.8.12)

  /// The message times of every alias of `p` at or after `now − user_gap.window`, merged, ascending.
  pure def recent_msgs(s: State, p: int, now: int): List[int] =
    sort_list(aliases(s, p).fold([], (acc, a) =>
      acc.concat(get_or(s.user_msgs, a, []).select(t => t >= now - USER_GAP_WINDOW))))

  /// `absent(p, now)`: gone, or no alias wrote within `user_gap.window`.
  pure def absent(s: State, p: int, now: int): bool =
    gone(s, p) or recent_msgs(s, p, now).length() == 0

  /// `p90_gap(s, p, now)`: the nearest-rank 90th percentile of the gaps between the person's recent
  /// messages (A-46), 0 without gaps.
  pure def p90_gap(s: State, p: int, now: int): int = {
    val msgs = recent_msgs(s, p, now)
    if (msgs.length() < 2) 0
    else {
      val gaps = msgs.foldl({ prev: -1, out: [] }, (acc, t) =>
        { prev: t, out: if (acc.prev < 0) acc.out else acc.out.append(t - acc.prev) }).out
      val rank = (9 * gaps.length() + 9) / 10
      kth_smallest(gaps, rank - 1)
    }
  }

  pure def is_refusal_source(src: UqSource): bool = match src { | SrcRefusal(_) => true | _ => false }

  /// `due_for(p, stage, source, now)`: `due_absent` for an absent addressee (not for a refusal
  /// question); the long due, raised to the p90 gap at stage 0 and capped at `capacity.window` (1.5,
  /// O-1); `user_question.due` later.
  pure def due_for(s: State, p: int, stage: int, source: UqSource, now: int): int =
    if (absent(s, p, now) and not(is_refusal_source(source))) now + USER_QUESTION_DUE_ABSENT
    else if (stage == 0) now + imin(imax(USER_QUESTION_DUE, p90_gap(s, p, now)), CAPACITY_WINDOW)
    else now + USER_QUESTION_DUE

  // ------------------------------------------------------------------ the digest audience (D.8.13)

  /// The requesters of the nearest node with user requests, gone persons included.
  pure def nearest_requesters(s: State, n: int): List[int] = {
    val m = nearest_requested(s, n)
    if (m >= 0) requesters_on(s, m, false) else []
  }

  /// The principals of the unsettled user requests on `n`, one per person, oldest request first.
  pure def drop_requesters(s: State, n: int): List[int] =
    user_requests_on(s, n).foldl([], (out, r) => push_person(s, out, requester_principal(s.requests.get(r))))

  /// The persons a user question was put to so far, other than `user`, one principal each.
  pure def asked_persons(s: State, uq: int): Set[int] = {
    val u = s.uquestions.get(uq)
    to_set(u.to.foldl({ i: 0, out: [] }, (acc, p) =>
      { i: acc.i + 1, out: if (acc.i <= u.stage) push_stage(s, acc.out, p) else acc.out }).out)
  }

  /// `audience(item)`: never empty (§8.5). The requesters whose work the item concerns; else the
  /// persons the question was put to; else `user`.
  pure def audience(s: State, it: DigestItem): Set[int] = {
    val req = match it {
      | DiRefusalDrop(x) => drop_requesters(s, x.node)
      | DiDefaultApplied(uq) => nearest_requesters(s, s.uquestions.get(uq).about)
      | DiLapsed(uq) => nearest_requesters(s, s.uquestions.get(uq).about)
      | DiAssumption(q) => nearest_requesters(s, s.pquestions.get(q).node)
    }
    if (req.length() > 0) to_set(req)
    else {
      val asked = match it {
        | DiRefusalDrop(x) => asked_persons(s, x.question)
        | DiDefaultApplied(uq) => asked_persons(s, uq)
        | DiLapsed(uq) => asked_persons(s, uq)
        | DiAssumption(q) => {
            val c = chain(s, s.pquestions.get(q).node, -1, false)
            if (c.length() > 1) Set(c[0]) else Set()
          }
      }
      if (asked != Set()) asked else Set(USER_PRINCIPAL)
    }
  }

  // ------------------------------------------------------------------ refusals and E1 (D.8.14)

  /// The refusal record of class `c` on `n` exists.
  pure def has_refusal(s: State, n: int, c: str): bool = s.refusals.keys().contains((n, c))

  /// `live(r, now)`: the entries that have not expired.
  pure def refusal_live(r: Refusal, now: int): Set[int] =
    r.by.keys().filter(x => r.by.get(x).expires_at > now)

  /// `class_refused(n, c, now)`: at least the record's quorum of live entries (CS-7).
  pure def class_refused(s: State, n: int, c: str, now: int): bool =
    has_refusal(s, n, c) and {
      val r = s.refusals.get((n, c))
      refusal_live(r, now).size() >= r.quorum
    }

  /// `backed_off(n, c, now)`: some session of the class refused `n` less than a backoff ago.
  pure def backed_off(s: State, n: int, c: str, now: int): bool =
    has_refusal(s, n, c) and {
      val r = s.refusals.get((n, c))
      r.by.keys().exists(x => r.by.get(x).backoff_until > now)
    }

  /// 1.12 (§10.4): no hold released as `handoff`.
  pure val NO_HANDOFF: HandedOff = { node: -1, fence: -1, until: -1 }

  /// `excluded(n, c, now)`: what `next` skips for a session of class `c`.
  pure def excluded(s: State, n: int, c: str, now: int): bool =
    class_refused(s, n, c, now) or backed_off(s, n, c, now)

  /// `active(n, now)`: the classes with a caps profile ⊇ `n.needs` whose window end lies after `now`.
  pure def active(s: State, n: int, now: int): Set[str] =
    s.classes.keys().filter(c => {
      val ca = s.classes.get(c)
      ca.caps_seen.keys().exists(caps =>
        ca.caps_seen.get(caps) > now and s.nodes.get(n).needs.subseteq(caps))
    })

  /// `refusers(n, now)`.
  pure def refusers(s: State, n: int, now: int): Set[str] =
    s.refusals.keys().filter(k => k._1 == n).map(k => k._2).filter(c => class_refused(s, n, c, now))

  /// `all_refused(n, now)`: open, refused by some class, and by every active class.
  pure def all_refused(s: State, n: int, now: int): bool =
    is_open(s, n) and {
      val rs = refusers(s, n, now)
      rs != Set() and active(s, n, now).subseteq(rs)
    }

  /// `covered(n)`: a pending `Refusal { n }` question (`ix.refusal_question[n]`).
  pure def covered(s: State, n: int): bool =
    s.uquestions.keys().exists(q =>
      s.uquestions.get(q).state == UqPending and s.uquestions.get(q).source == SrcRefusal(n))

  /// `ix.refused`: the nodes with a refusal record.
  pure def refused_nodes(s: State): Set[int] = s.refusals.keys().map(k => k._1)

  // ------------------------------------------------------------------ basis (D.8.17, D-4)

  pure def outcome_or_done(l: LifeAt): Outcome = match l { | AtOpen => Done | AtClosed(o) => o }

  /// `stale(s, b)`: a reference whose target was struck out, a cited decision that is no longer
  /// active, or a cited node (as it lives on after merges) closed with another outcome than the one
  /// recorded (`done` for a node that was open then).
  pure def stale(s: State, b: BasisRef): bool =
    match b {
      | BNode(x) => {
          val f = resolve_merged(s, x.id)
          or {
            s.retracted.contains(TNode(x.id)),
            s.retracted.contains(TNode(f)),
            has_node(s, f) and match s.nodes.get(f).life {
              | Open => false
              | Closed(c) => c.outcome != outcome_or_done(x.life_at)
            },
          }
        }
      | BEntry(e) =>
          s.retracted.contains(TEntry(e))
          or (s.decisions.keys().contains(e) and s.decisions.get(e).state != DActive)
    }

  /// `stale_decision(s, d)`: an active decision with a stale reference.
  pure def stale_decision(s: State, d: int): bool = {
    val dd = s.decisions.get(d)
    dd.state == DActive and any_of(dd.basis, b => stale(s, b))
  }

  // ------------------------------------------------------------------ FNV-1a-64 and tie_hash (D.2.6)

  // Quint 0.33's default (Rust) evaluator computes in i64 and fails on overflow, so a 64-bit value
  // is a list of four 16-bit limbs, least significant first, and every product stays below 2^26.

  pure val LIMB = 65536

  /// The 64-bit value with high half `hi` and low half `lo` (each below 2^32).
  pure def u64_of(hi: int, lo: int): List[int] = [lo % LIMB, lo / LIMB, hi % LIMB, hi / LIMB]

  pure def u64_hi(x: List[int]): int = x[3] * LIMB + x[2]
  pure def u64_lo(x: List[int]): int = x[1] * LIMB + x[0]

  /// `a < b` as unsigned 64-bit values: the order of Rust's `h` (D.2.6), which `tie_key` gives as a
  /// pair for the lexicographic key of `next` (WP-A2).
  pure def u64_lt(a: List[int], b: List[int]): bool =
    u64_hi(a) < u64_hi(b) or (u64_hi(a) == u64_hi(b) and u64_lo(a) < u64_lo(b))

  /// `(x × m) mod 2^64` for `0 <= m < 2^16`.
  pure def u64_mul_small(x: List[int], m: int): List[int] =
    range(0, 4).foldl({ out: [], carry: 0 }, (acc, i) => {
      val t = x[i] * m + acc.carry
      { out: acc.out.append(t % LIMB), carry: t / LIMB }
    }).out

  /// `(a + b) mod 2^64`.
  pure def u64_add(a: List[int], b: List[int]): List[int] =
    range(0, 4).foldl({ out: [], carry: 0 }, (acc, i) => {
      val t = a[i] + b[i] + acc.carry
      { out: acc.out.append(t % LIMB), carry: t / LIMB }
    }).out

  /// `(a − b) mod 2^64` (Rust's `wrapping_sub`).
  pure def u64_sub(a: List[int], b: List[int]): List[int] =
    range(0, 4).foldl({ out: [], borrow: 0 }, (acc, i) => {
      val t = a[i] - b[i] - acc.borrow
      if (t < 0) { out: acc.out.append(t + LIMB), borrow: 1 } else { out: acc.out.append(t), borrow: 0 }
    }).out

  /// The XOR of two bytes, by an 8-step bit fold.
  pure def xor8(a: int, b: int): int =
    range(0, 8).foldl({ x: a, y: b, p: 1, out: 0 }, (acc, i) => {
      x: acc.x / 2,
      y: acc.y / 2,
      p: acc.p * 2,
      out: if (acc.x % 2 != acc.y % 2) acc.out + acc.p else acc.out,
    }).out

  /// The offset basis 0xcbf2_9ce4_8422_2325, as its four limbs: TLC's integers are 32-bit signed,
  /// so no literal of the model may reach 2^31 (TLC stops with "can't handle a number this big").
  pure val FNV_OFFSET: List[int] = [0x2325, 0x8422, 0x9ce4, 0xcbf2]

  /// `h × 0x0000_0100_0000_01b3 mod 2^64 = h × 0x1b3 + h × 2^40`; `h × 2^40` is `h × 2^8` moved up
  /// two limbs.
  pure def fnv_mul_prime(h: List[int]): List[int] = {
    val y = u64_mul_small(h, 256)
    u64_add(u64_mul_small(h, 0x1b3), [0, 0, y[0], y[1]])
  }

  /// One step: `h = (h ^ b).wrapping_mul(prime)`. XOR with a byte changes the low byte only.
  pure def fnv_step(h: List[int], b: int): List[int] = {
    val lo = h[0] % 256
    fnv_mul_prime([h[0] - lo + xor8(lo, b), h[1], h[2], h[3]])
  }

  /// The step is passed as a lambda: Apalache refuses a fold over an operator passed by name once
  /// the module is instantiated (`Expected F to be a binary operator passed by name`).
  pure def fnv1a64(bytes: List[int]): List[int] = bytes.foldl(FNV_OFFSET, (h, b) => fnv_step(h, b))

  /// The 8 little-endian bytes of `x` (`0 <= x < 2^64`). Quint's default (Rust) evaluator holds
  /// only `x < 2^63`; the vectors at `u64::MAX` run on the TypeScript evaluator
  /// (`tests/fnv_ts_test.qnt`). Model ids stay far below either bound.
  pure def le_bytes(x: int): List[int] =
    range(0, 8).foldl({ rest: x, out: [] }, (acc, i) =>
      { rest: acc.rest / 256, out: acc.out.append(acc.rest % 256) }).out

  /// `fnv_id(id) = FNV-1a-64(0x00 ‖ id.to_le_bytes())`.
  pure def fnv_id(id: int): List[int] = fnv1a64([0].concat(le_bytes(id)))

  /// `fnv_session(s) = FNV-1a-64(s.to_le_bytes() ‖ 0x01)`.
  pure def fnv_session(sess: int): List[int] = fnv1a64(le_bytes(sess).append(1))

  /// `h = tie_hash(s, id) = fnv_id(id).wrapping_sub(fnv_session(s))`: the ring distance from the
  /// session's point to the candidate's (CS-12, D-1).
  pure def tie_hash(sess: int, id: int): List[int] = u64_sub(fnv_id(id), fnv_session(sess))

  /// The tie-break component of `next`'s key as a pair that orders like `h`: `(hi, lo)` of
  /// `tie_hash` with `use_fnv`, and `(0, 0)` without, so that the id that follows in the key breaks
  /// the tie (`TieBreak::Id`, D.10.6).
  pure def tie_key(use_fnv: bool, sess: int, id: int): (int, int) =
    if (use_fnv) { val h = tie_hash(sess, id) (u64_hi(h), u64_lo(h)) } else (0, 0)
}
