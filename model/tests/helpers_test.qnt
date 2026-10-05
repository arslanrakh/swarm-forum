// -*- mode: Bluespec; -*-
/**
 * Run tests of the model's helpers (WP-A1, D.13.2): the FNV-1a-64 tie-break against the vectors of
 * D.2.6, the parameters (MAX_TIME < 60), the reads of `basics.qnt`, the flags of `flags.qnt`, and
 * the queue, every firing arm, and the bounded settle of `deadlines.qnt`.
 *
 *   cd model && npx quint test tests/helpers_test.qnt
 */
module helpers_test {
  import types.* from "../types"
  import params.* from "../params"
  import basics.* from "../basics"
  import flags.* from "../flags"
  import deadlines.* from "../deadlines"

  // ================================================================== fixtures

  /// An agent-made plain node (content 1, capacity 1).
  pure def work_node(parent: int, name: str, ks: Set[Key], tags: Set[str]): Node =
    { ...reserved_node(parent, name, 0), origin: OAgent(1), capacity: 1, content: 1, keys: ks, tags: tags }

  pure def with_node(s: State, nd: Node): State = ev_node_created(s, nd, s.last_now)

  pure def new_session(c: str, at: int): Session = {
    class: c, started_at: at, last_seen: at, silence_at: at + PRESENCE_SILENCE,
    retire_at: at + SESSION_RETAIN, silent: false, caps: Set(), horizon: -1, principals: Set(),
    shown: Map(), branch_seen: Map(), handed_off: NO_HANDOFF,
  }

  pure def with_session(s: State, c: str, at: int): State =
    { ...s, next_session: s.next_session + 1, sessions: s.sessions.put(s.next_session, new_session(c, at)) }

  /// A hold issued at `at` (`HoldIssued`): the next fence, the first lease.
  pure def with_hold(s: State, n: int, sess: int, at: int): State = {
    val f = s.nodes.get(n).fence + 1
    {
      ...upd_node(s, n, nd => { ...nd, fence: f }),
      holds: s.holds.put((n, f), {
        session: sess, taken_at: at, lease_until: at + LEASE_FIRST, last_extended: at,
        terms: LEASE_TERMS, l: LEASE_DEFAULT, renews: 0, checkpointed: false
      }),
    }
  }

  /// A node that was held once and released: its fence is past 0, and it holds nothing now (1.6:
  /// `ready_to_close` reads it).
  pure def held_once(s: State, n: int): State = upd_node(s, n, nd => { ...nd, fence: nd.fence + 1 })

  pure def with_principal(s: State, k: str): State =
    { ...s, next_principal: s.next_principal + 1, principal_ids: s.principal_ids.put(k, s.next_principal) }

  /// The seed of D.10.7 without the commands: sessions 1 (class "a") and 2 (class "b") opened at 0,
  /// and the work nodes /w1 (#8, key c1) and /w2 (#9, key c2), capacity 1, tagged `offer`.
  pure val W0: State =
    with_node(
      with_node(with_session(with_session(genesis(0), "a", 0), "b", 0),
        work_node(ROOT, "w1", Set(Conv("c1")), Set(OFFER))),
      work_node(ROOT, "w2", Set(Conv("c2")), Set(OFFER)))

  /// Removes the sessions, so that a test sees only the deadlines it sets up.
  pure def quiet(s: State): State = { ...s, sessions: Map() }

  pure def pq_rec(n: int, conv: Key, blocking: bool, rechecks: int, recheck_at: int): PQuestion = {
    node: n, conv: conv, blocking: blocking, said: 1, state: PqOpen, rechecks: rechecks,
    recheck_at: recheck_at, lapse: NoLapse, assumed_by: -1, moved_from: -1,
  }

  pure def uq_rec(src: UqSource, about: int, stages: List[int], due: int, dflt: bool): UQuestion = {
    source: src, about: about, to: stages, stage: 0, due: due, has_default: dflt,
    state: UqPending, assumed_by: -1,
  }

  pure def user_request(n: int, p: int, answered_by: int): Request = {
    node: n, origin: User(1), conv: User(1), from_user: 1, requester: ReqUser(p), answered_by: answered_by,
    settled: Unsettled, moved_from: -1,
  }

  pure def run_to(s: State, t: int): State = settle(s, t, 30).state

  /// The queue without its Stale keys (1.8): every node a fixture makes is touched at its
  /// creation, so each open one has a Stale key at `touched + OFFER_STALE`, which `staleKeysTest`
  /// covers and the other tests leave out.
  pure def q(s: State): Set[DeadlineKey] = queue(s).filter(k => k.kind != K_STALE)

  /// The series spec of the helper tests: period 10, so that a run to 40 sees several slots. The
  /// seeds use SEED_REPEAT (period 15, `params.qnt`), tested in `seedOccurrenceBoundTest`.
  pure val R10: RepeatSpec = { period: 10, offset: 0, run_within: 4 }

  pure def closed_with(s: State, n: int): Outcome =
    match s.nodes.get(n).life { | Closed(c) => c.outcome | Open => Superseded }

  // ================================================================== D.2.6: FNV-1a-64 and tie_hash

  // The vectors of D.2.6 at u64::MAX hash the eight bytes 0xff directly here: Quint's default
  // evaluator cannot hold 2^64 − 1 as an integer. `tests/fnv_ts_test.qnt` runs `fnv_id`,
  // `fnv_session`, `tie_hash`, and `le_bytes` themselves at u64::MAX on the TypeScript evaluator.
  pure val U64_MAX_BYTES = [255, 255, 255, 255, 255, 255, 255, 255]

  run fnvEmptyTest = assert(fnv1a64([]) == u64_of(0xcbf29ce4, 0x84222325))
  run fnvATest = assert(fnv1a64([0x61]) == u64_of(0xaf63dc4c, 0x8601ec8c))
  run fnvFoobarTest =
    assert(fnv1a64([0x66, 0x6f, 0x6f, 0x62, 0x61, 0x72]) == u64_of(0x85944171, 0xf73967e8))

  run fnvIdTest = assert(and {
    fnv_id(0) == u64_of(0xe604823a, 0x249029bf),
    fnv_id(6) == u64_of(0x2c23d803, 0xe2f46cf9),
    fnv_id(42) == u64_of(0x8f919d01, 0x15208895),
    fnv1a64([0].concat(U64_MAX_BYTES)) == u64_of(0x7a4969ca, 0x2d631437),
  })

  run fnvSessionTest = assert(and {
    fnv_session(0) == u64_of(0xe604813a, 0x2490280c),
    fnv_session(1) == u64_of(0x529a2ddc, 0x8ff5355f),
    fnv_session(2) == u64_of(0x0cd92bf5, 0x4dc61432),
    fnv_session(17) == u64_of(0x893f83b5, 0xd9a4968f),
    fnv1a64(U64_MAX_BYTES.append(1)) == u64_of(0x280457de, 0x49e07df4),
  })

  run tieHashTest = assert(and {
    tie_hash(0, 0) == u64_of(0x00000100, 0x000001b3),
    tie_hash(1, 6) == u64_of(0xd989aa27, 0x52ff379a),
    tie_hash(1, 42) == u64_of(0x3cf76f24, 0x852b5336),
    tie_hash(2, 42) == u64_of(0x82b8710b, 0xc75a7463),
    tie_hash(17, 5) == u64_of(0xc1df1b57, 0x143f208b),
    u64_sub(fnv1a64([0].concat(U64_MAX_BYTES)), fnv1a64(U64_MAX_BYTES.append(1)))
      == u64_of(0x524511eb, 0xe3829643),
  })

  run tieKeyTest = assert(and {
    tie_key(true, 1, 6) == (0xd989aa27, 0x52ff379a),
    tie_key(false, 1, 6) == (0, 0),
    u64_lt(tie_hash(1, 42), tie_hash(1, 6)),
    not(u64_lt(tie_hash(1, 6), tie_hash(1, 6))),
    // the low half decides when the high halves are equal
    u64_lt(u64_of(1, 2), u64_of(1, 3)) and not(u64_lt(u64_of(1, 3), u64_of(1, 2))),
    u64_lt(u64_of(1, 0xffffffff), u64_of(2, 0)) and not(u64_lt(u64_of(2, 0), u64_of(1, 0xffffffff))),
    le_bytes(258) == [2, 1, 0, 0, 0, 0, 0, 0],
    xor8(0xf0, 0x3c) == 0xcc,
  })

  // ================================================================== D.10.5: parameters

  run maxTimeTest = assert(MAX_TIME < 60 and MAX_TIME < HOUR)
  run paramsTest = assert(PARAMS_VALID and SCALING_PRESERVES_ORDER)

  // ================================================================== basics

  run genesisTest = {
    val g = genesis(0)
    assert(and {
      g.nodes.keys() == 0.to(7),
      g.nodes.keys().forall(n => g.nodes.get(n).origin == OReserved and g.nodes.get(n).capacity == 0),
      resolve(g, ROOT, Lit("@user")) == USER,
      resolve(g, USER, Lit("digest")) == USER_DIGEST,
      resolve(g, SYS, Lit("signals")) == SIGNALS,
      resolve(g, ROOT, Lit("@teams")) == TEAMS,
      g.names.keys().size() == 7,
      g.principal_ids == Map("user" -> 0),
      g.next_node == 8 and g.next_eid == 1 and g.next_session == 1 and g.next_principal == 1,
      q(g) == Set(),
    })
  }

  run treeTest = {
    val s = with_node(W0, work_node(8, "sub", Set(), Set()))
    val moved = { ...s, aliases: s.aliases.put((9, Lit("old")), 10) }
    assert(and {
      ancestors(s, 10) == [8, ROOT],
      ancestors(s, ROOT) == [],
      ancestors_or_self(s, SIGNALS) == [SIGNALS, SYS, ROOT],
      depth(s, 10) == 2 and depth(s, SIGNALS) == 2 and depth(s, ROOT) == 0,
      is_under(s, 10, 8) and is_under(s, 10, ROOT) and not(is_under(s, 8, 10)),
      children(s, 8) == Set(10),
      resolve(s, 8, Lit("sub")) == 10,
      resolve(s, 8, Lit("none")) == -1,
      resolve(moved, 9, Lit("old")) == 10,
      // the live name wins over an alias at the same place (D.8.4)
      resolve({ ...s, aliases: s.aliases.put((ROOT, Lit("w1")), 9) }, ROOT, Lit("w1")) == 8,
    })
  }

  run resolveMergedTest = {
    val s0 = with_node(W0, work_node(ROOT, "w3", Set(), Set()))
    val s1 = ev_node_closed(s0, 8, 0, Superseded, 9)
    val s2 = ev_node_closed(s1, 9, 0, Superseded, 10)
    assert(and {
      resolve_merged(s2, 8) == 10,
      resolve_merged(s2, 10) == 10,
      merged(s2, 8) and merged(s2, 9) and not(merged(s2, 10)),
    })
  }

  run containerTest = {
    val s0 = with_node(W0, { ...work_node(PEOPLE, "bob", Set(), Set()) })
    val s1 = with_node(s0, { ...work_node(ROOT, "s", Set(), Set()), role: Series({ repeat: R10, last_missed: false }) })
    val team = with_node(s1, work_node(TEAMS, "t", Set(), Set()))
    assert(and {
      is_container(s1, ROOT) and is_container(s1, USER),
      is_container(s1, 10) and is_directory(s1, 10),
      is_container(s1, 11) and not(is_directory(s1, 11)),
      not(is_container(s1, 8)),
      is_container(team, 12) and is_directory(team, 12),            // a team is a container (A-113)
    })
  }

  run occLockedTest = {
    val s0 = with_node(quiet(W0), { ...work_node(ROOT, "s", Set(), Set()), role: Series({ repeat: R10, last_missed: false }) })
    val s = open_next_occurrence(s0, 10, 0, 0)
    val closed_series = ev_node_closed(s, 10, 0, Done, -1)
    val plain = upd_node(s, 10, nd => { ...nd, role: Plain })
    assert(and {
      occ_locked(s, 11),                          // its series is open and still a series
      not(occ_locked(closed_series, 11)),
      not(occ_locked(plain, 11)),                 // the series' repeat was removed
      not(occ_locked(s, 10)) and not(occ_locked(s, 8)),
    })
  }

  run holdsTest = {
    val s0 = with_node(W0, work_node(8, "sub", Set(), Set()))
    val s = with_hold(s0, 10, 1, 0)
    assert(and {
      s.holds.keys() == Set((10, 1)),
      live(s.holds.get((10, 1)), 2) and not(live(s.holds.get((10, 1)), 3)),
      attended(s, 10, 2) and attended(s, 8, 2) and attended(s, ROOT, 2),
      not(attended(s, 9, 2)) and not(attended(s, 8, 3)),
      held_self(s, 10, 2) and not(held_self(s, 8, 2)),
      live_holds_of(s, 1, 2) == Set((10, 1)) and live_holds_of(s, 2, 2) == Set(),
      holds_on(s, 10) == Set((10, 1)) and live_holds_on(s, 10, 5) == Set(),
    })
  }

  run waitingTest = {
    val s0 = { ...W0, pquestions: W0.pquestions.put(2, pq_rec(8, Conv("c1"), false, 0, 8)) }
    val tw = set_wait(s0, 8, WTime(7), 0)
    val nw = set_wait(s0, 8, WNode(9), 0)
    val qw = set_wait(s0, 9, WQuestion({ q: 2, rechecks_at_set: 0 }), 0)
    val past = set_wait(s0, 8, WExternal(0), 3)
    assert(and {
      waiting(tw, 8, 6) and not(waiting(tw, 8, 7)),
      not(waiting(s0, 8, 0)),
      waiting(nw, 8, 0) and not(waiting(ev_node_closed(nw, 9, 0, Done, -1), 8, 0)),
      waiting(qw, 9, 0),
      not(waiting({ ...qw, pquestions: qw.pquestions.setBy(2, x => { ...x, rechecks: 1 }) }, 9, 0)),
      not(waiting({ ...qw, pquestions: qw.pquestions.setBy(2, x => { ...x, state: PqAnswered(5) }) }, 9, 0)),
      question_waiters(qw, 2) == Set(9) and node_waiters(nw, 9) == Set(8),
      match past.nodes.get(8).wait { | HasWait(w) => w.fired | NoWait => false },
      // `fired := T <= at` at equality too (A-68)
      match set_wait(s0, 8, WTime(3), 3).nodes.get(8).wait { | HasWait(w) => w.fired | NoWait => false },
      match tw.nodes.get(8).wait { | HasWait(w) => not(w.fired) | NoWait => false },
    })
  }

  run pausedTest = {
    val s0 = with_node(W0, work_node(8, "sub", Set(), Set()))
    val s = { ...s0, pauses: s0.pauses.put(8, { until: 5, last_renewed: 0, ttl: PAUSE_TTL, by: 1, class: "a" }) }
    assert(and {
      paused(s, 10, 4) and paused(s, 8, 4),
      not(paused(s, 10, 5)) and not(paused(s, 9, 4)) and not(paused(s, ROOT, 4)),
    })
  }

  run convTest = {
    val s0 = with_node(W0, work_node(8, "sub", Set(), Set()))
    val two = upd_node(s0, 9, nd => { ...nd, keys: Set(Conv("c2"), Msg(("c2", 1))) })
    val obj = upd_node(s0, 10, nd => { ...nd, keys: Set(User(3), RepeatSlot((1, 2)), Person("p1")) })
    // an object key does not name a conversation (CS-36): #10 carries only one, #9 carries one
    // beside its chat key, the RS-8 case that 1.1 failed with AmbiguousConversationKey
    val lean = upd_node(upd_node(s0, 10, nd => { ...nd, keys: Set(Obj("l1")) }), 9, nd => { ...nd, keys: Set(Conv("c2"), Obj("l2")) })
    assert(and {
      not(is_conversation(Obj("l1"))) and is_conversation(Msg(("c1", 1))) and is_conversation(Conv("c1")),
      conv_key(lean, 10, NoKey) == CkOk(Conv("c1")),
      conv_key(lean, 9, NoKey) == CkOk(Conv("c2")),
      conv_key(upd_node(lean, 8, nd => { ...nd, keys: Set(Obj("l3")) }), 10, NoKey) == CkErr(WhyNoConversationKey),
      conv_node(lean, 10, Obj("l2")) == { node: 9, case: 2 },
      // case 1 takes a closed carrier too (§9.1: "open or closed")
      conv_node(ev_node_closed(s0, 8, 0, Done, -1), 10, Conv("c1")) == { node: 8, case: 1 },
      conv_node(s0, 10, Conv("c1")) == { node: 8, case: 1 },
      conv_node(s0, 10, Conv("c2")) == { node: 9, case: 2 },
      conv_node(s0, 10, Conv("c3")) == { node: 10, case: 3 },
      conv_node(ev_node_closed(s0, 9, 0, Done, -1), 10, Conv("c2")) == { node: 10, case: 3 },
      conv_key(s0, 10, NoKey) == CkOk(Conv("c1")),
      conv_key(s0, 10, SomeKey(Conv("c9"))) == CkOk(Conv("c9")),
      conv_key(s0, ROOT, NoKey) == CkErr(WhyNoConversationKey),
      conv_key(two, 9, NoKey) == CkErr(WhyAmbiguousConversationKey),
      conv_key(obj, 10, NoKey) == CkOk(Conv("c1")),
      key_open(s0, Conv("c2")) == 9 and key_open(s0, Conv("c3")) == -1,
    })
  }

  // ------------------------------------------------------------------ the directory, chain, due, audience

  /// Principals user (0), p1 (1), p2 (2), p3 (3); person #8 under @people with the aliases p1 and
  /// p2; team #9 under @teams with member #8; work node #10 owned by the team; work node #11 under
  /// #10.
  pure val D0: State = {
    val s0 = with_principal(with_principal(with_principal(quiet(genesis(0)), "p1"), "p2"), "p3")
    val s1 = with_node(s0, work_node(PEOPLE, "dave", Set(Person("p1"), Person("p2")), Set()))
    val s2 = with_node(s1, { ...work_node(TEAMS, "payments", Set(), Set()), owners: Set(8) })
    val s3 = with_node(s2, { ...work_node(ROOT, "work", Set(Conv("c1")), Set()), owners: Set(9) })
    with_node(s3, work_node(10, "task", Set(), Set()))
  }

  /// D0 plus the principals p4 (4) and p5 (5) and three more persons: #12 (p3), #13 (p4), #14 (p5).
  pure val D1: State = {
    val s0 = with_principal(with_principal(D0, "p4"), "p5")
    val s1 = with_node(s0, work_node(PEOPLE, "erin", Set(Person("p3")), Set()))
    val s2 = with_node(s1, work_node(PEOPLE, "finn", Set(Person("p4")), Set()))
    with_node(s2, work_node(PEOPLE, "gus", Set(Person("p5")), Set()))
  }

  run directoryTest = {
    val g = upd_node(D0, 8, nd => { ...nd, tags: Set(GONE) })
    assert(and {
      person_of(D0, 1) == 8 and person_of(D0, 2) == 8 and person_of(D0, 3) == -1,
      person_of(D0, USER_PRINCIPAL) == -1,
      aliases_of(D0, 8) == Set(1, 2) and rep(D0, 8) == 1,
      aliases(D0, 2) == Set(1, 2) and aliases(D0, 3) == Set(3),
      same_person(D0, 1, 2) and same_person(D0, 3, 3) and not(same_person(D0, 1, 3)),
      not(gone(D0, 1)) and gone(g, 1) and gone(g, 2) and not(gone(g, 3)),
      is_person_node(D0, 8) and is_team_node(D0, 9) and not(is_person_node(D0, 9)),
      // a closed person node is no person: its aliases are separate principals again (A-113)
      person_of(ev_node_closed(D0, 8, 0, Done, -1), 1) == -1,
    })
  }

  run ownersChainTest = {
    val g = upd_node(D0, 8, nd => { ...nd, tags: Set(GONE) })
    val root_same = upd_node(D0, ROOT, nd => { ...nd, owners: Set(8) })
    val direct = upd_node(D0, 11, nd => { ...nd, owners: Set(8, 10) })
    assert(and {
      owners(D0, 11) == [1],                  // inherited from #10: the team's member #8
      owners(D0, -1) == [],                   // ROOT owns nothing
      owners(g, 11) == [],                    // gone persons are skipped
      owners(direct, 11) == [1],              // #10 is neither a person nor a team
      owners(root_same, 11) == [1],           // ROOT's owner is #8 again: no second stage
      // ROOT's owners follow those of the nearest owned node, when they are another person
      owners(upd_node(D1, ROOT, nd => { ...nd, owners: Set(12) }), 11) == [1, 3],
      owners(upd_node(D1, ROOT, nd => { ...nd, owners: Set(12) }), -1) == [3],
      // at most three stages
      owners(upd_node(D1, 10, nd => { ...nd, owners: Set(8, 12, 13, 14) }), 11) == [1, 3, 4],
      // the node's own owners come before its ancestors'
      owners(upd_node(D1, 11, nd => { ...nd, owners: Set(12) }), 11) == [3],
      chain(D0, 11, -1, false) == [1, USER_PRINCIPAL],
      chain(D0, 11, 2, false) == [2, USER_PRINCIPAL],   // p1 is an alias of the explicit p2
      chain(D0, 11, 3, false) == [3, 1, USER_PRINCIPAL],
      chain(g, 11, 2, false) == [2, USER_PRINCIPAL],    // explicit even when gone
      chain(D0, 11, USER_PRINCIPAL, false) == [1, USER_PRINCIPAL],
      chain(genesis(0), -1, -1, false) == [USER_PRINCIPAL],
    })
  }

  run requestersTest = {
    val s0 = { ...D0, requests: D0.requests.put(20, user_request(10, 3, -1)).put(21, user_request(10, 2, -1)).put(22, user_request(10, 1, -1)) }
    val ext = { ...D0, requests: D0.requests.put(20, { ...user_request(11, 3, -1), requester: ReqExternal }) }
    val done = { ...s0, requests: s0.requests.setBy(20, r => { ...r, settled: SettledBy({ decision: 30, answer: 31 }) }) }
    val four = { ...D1, requests: Map(20 -> user_request(10, 3, -1), 21 -> user_request(10, 4, -1),
                                      22 -> user_request(10, 5, -1), 23 -> user_request(10, 1, -1)) }
    val gone2 = upd_node({ ...D0, requests: Map(20 -> user_request(10, 2, -1), 21 -> user_request(10, 3, -1)) },
                         8, nd => { ...nd, tags: Set(GONE) })
    val own = { ...D0, requests: Map(20 -> user_request(10, 3, -1), 21 -> user_request(11, 2, -1)) }
    assert(and {
      requesters_on(four, 10, false) == [3, 4, 5],     // at most three
      requesters_on(gone2, 10, true) == [3] and requesters_on(gone2, 10, false) == [2, 3],
      nearest_requested(own, 11) == 11 and requesters(own, 11) == [2],   // the node itself first
      user_requests_on(s0, 10) == [20, 21, 22],
      requesters_on(s0, 10, false) == [3, 2],          // p1 is an alias of p2, whose request is older
      nearest_requested(s0, 11) == 10,
      nearest_requested(ext, 11) == -1,                 // external requests are no user requests
      requesters(s0, 11) == [3, 2],
      requesters_on(done, 10, false) == [2],
      chain(s0, 11, -1, false) == [3, 2, USER_PRINCIPAL],
      chain(s0, 11, -1, true) == [1, USER_PRINCIPAL],   // a refusal: #11's own requests only
      drop_requesters(s0, 10) == [3, 2],
    })
  }

  run dueTest = {
    val s = { ...D0, user_msgs: Map(1 -> [1, 3, 4, 8], 2 -> [9]) }
    val one = { ...D0, user_msgs: Map(3 -> [1, 3, 4, 8]) }
    assert(and {
      recent_msgs(s, 1, 10) == [1, 3, 4, 8, 9],
      p90_gap(one, 3, 10) == 4,                         // gaps 2, 1, 4: rank 3 of 3
      p90_gap(s, 1, 10) == 4,                           // gaps 1, 1, 2, 4: rank 4 of 4
      p90_gap(s, USER_PRINCIPAL, 10) == 0,
      recent_msgs(s, 1, 22) == [9],                     // the window is 13 ticks
      not(absent(s, 1, 10)) and absent(s, USER_PRINCIPAL, 10) and absent(s, 1, 23),
      absent(upd_node(s, 8, nd => { ...nd, tags: Set(GONE) }), 1, 10),
      due_for(s, 1, 0, SrcAgent({ session: 1, digest: 0 }), 10) == 10 + USER_QUESTION_DUE,
      due_for(s, USER_PRINCIPAL, 0, SrcEscalation(5), 10) == 10 + USER_QUESTION_DUE_ABSENT,
      due_for(s, USER_PRINCIPAL, 0, SrcRefusal(10), 10) == 10 + USER_QUESTION_DUE,
      due_for(s, 1, 1, SrcRefusal(10), 10) == 10 + USER_QUESTION_DUE,
      // a p90 gap above user_question.due raises stage 0, and only stage 0
      p90_gap({ ...D1, user_msgs: Map(3 -> [0, 12]) }, 3, 13) == 12,
      due_for({ ...D1, user_msgs: Map(3 -> [0, 12]) }, 3, 0, SrcAgent({ session: 1, digest: 0 }), 13) == 13 + 12,
      due_for({ ...D1, user_msgs: Map(3 -> [0, 12]) }, 3, 1, SrcAgent({ session: 1, digest: 0 }), 13) == 13 + USER_QUESTION_DUE,
      // a p90 gap above capacity.window waits capacity.window only (1.5, O-1)
      p90_gap({ ...D1, user_msgs: Map(3 -> [0, 13]) }, 3, 13) == 13,
      due_for({ ...D1, user_msgs: Map(3 -> [0, 13]) }, 3, 0, SrcAgent({ session: 1, digest: 0 }), 13) == 13 + CAPACITY_WINDOW,
    })
  }

  run p90LongTest = {
    // ten gaps within the window, eight of 1, then 2 and 3: nearest rank ⌈0.9 × 10⌉ = 9 (A-46)
    val s = { ...D0, user_msgs: Map(3 -> [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 13]) }
    val t = { ...D0, user_msgs: Map(3 -> [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 13]) }
    assert(and {
      p90_gap(s, 3, 13) == 2,
      p90_gap(t, 3, 13) == 1,
      // at 14 the message at 0 has left the window: nine gaps, rank ⌈8.1⌉ = 9
      p90_gap(s, 3, 14) == 3,
    })
  }

  run audienceTest = {
    val reqs = { ...D0, requests: D0.requests.put(20, user_request(10, 3, -1)) }
    val pq = { ...reqs, pquestions: reqs.pquestions.put(30, pq_rec(11, Conv("c1"), false, 2, 8)) }
    val bare = { ...D0, pquestions: D0.pquestions.put(30, pq_rec(11, Conv("c1"), false, 2, 8)) }
    val uq = { ...D0, uquestions: D0.uquestions
      .put(31, { ...uq_rec(SrcAgent({ session: 1, digest: 0 }), 11, [2, USER_PRINCIPAL], 5, false), stage: 1 })
      .put(32, uq_rec(SrcAgent({ session: 1, digest: 0 }), -1, [USER_PRINCIPAL], 5, false)) }
    assert(and {
      audience(pq, DiAssumption(30)) == Set(3),                 // the requesters in reach
      audience(bare, DiAssumption(30)) == Set(1),               // the chain's first stage
      audience(uq, DiLapsed(31)) == Set(2),                     // the persons asked, not `user`
      audience(uq, DiLapsed(32)) == Set(USER_PRINCIPAL),        // nobody else
      audience({ ...uq, requests: reqs.requests }, DiRefusalDrop({ node: 10, question: 31 })) == Set(3),
      audience(uq, DiRefusalDrop({ node: 10, question: 31 })) == Set(2),
      // the persons asked so far include the current stage
      asked_persons({ ...D1, uquestions: Map(31 -> { ...uq_rec(SrcAgent({ session: 1, digest: 0 }), 11, [1, 3, USER_PRINCIPAL], 5, false), stage: 1 }) }, 31) == Set(1, 3),
      asked_persons({ ...D1, uquestions: Map(31 -> uq_rec(SrcAgent({ session: 1, digest: 0 }), 11, [1, 3, USER_PRINCIPAL], 5, false)) }, 31) == Set(1),
    })
  }

  run refusalsTest = {
    val r2: Refusal = { quorum: 2, by: Map(1 -> { backoff_until: 5, expires_at: 12 }, 2 -> { backoff_until: 6, expires_at: 9 }) }
    val s = { ...W0,
      refusals: Map((8, "a") -> r2, (9, "a") -> { quorum: 2, by: Map(1 -> { backoff_until: 5, expires_at: 12 }) }),
      classes: Map("a" -> { caps_seen: Map(Set() -> 10) }, "b" -> { caps_seen: Map(Set("gpu") -> 2) }) }
    val gpu = upd_node(s, 8, nd => { ...nd, needs: Set("gpu") })
    assert(and {
      class_refused(s, 8, "a", 3) and not(class_refused(s, 8, "a", 9)),
      not(class_refused(s, 9, "a", 3)) and backed_off(s, 9, "a", 3) and not(backed_off(s, 9, "a", 5)),
      excluded(s, 9, "a", 4) and not(excluded(s, 9, "a", 5)),
      active(s, 8, 1) == Set("a", "b"),                // caps {gpu} cover a node without needs
      active(s, 8, 3) == Set("a") and active(s, 8, 10) == Set(),
      active(gpu, 8, 1) == Set("b") and active(gpu, 8, 3) == Set(),
      refusers(s, 8, 3) == Set("a"),
      all_refused(s, 8, 3) and not(all_refused(s, 8, 1)),
      not(all_refused(gpu, 8, 1)) and all_refused(gpu, 8, 3),
      not(all_refused(s, 9, 3)),
      not(all_refused(s, 8, 10)),                      // one live entry at 10, below the quorum
      not(all_refused(ev_node_closed(s, 8, 0, Done, -1), 8, 3)),
      refused_nodes(s) == Set(8, 9),
      not(covered(s, 8)),
      covered({ ...s, uquestions: Map(40 -> uq_rec(SrcRefusal(8), 8, [0], 20, true)) }, 8),
      // a finished refusal question covers nothing
      not(covered({ ...s, uquestions: Map(40 -> { ...uq_rec(SrcRefusal(8), 8, [0], 20, true), state: UqDefaulted }) }, 8)),
      not(covered({ ...s, uquestions: Map(40 -> uq_rec(SrcRefusal(9), 9, [0], 20, true)) }, 8)),
    })
  }

  run staleTest = {
    val s0 = with_node(W0, work_node(ROOT, "w3", Set(), Set()))
    val s = { ...s0, decisions: Map(
      20 -> { node: 10, settles: Set(), state: DActive, basis: [BNode({ id: 9, life_at: AtOpen })] },
      21 -> { node: 10, settles: Set(), state: DSuperseded(22), basis: [BNode({ id: 9, life_at: AtOpen })] },
      22 -> { node: 10, settles: Set(), state: DActive, basis: [BEntry(21)] },
      23 -> { node: 10, settles: Set(), state: DActive, basis: [BNode({ id: 9, life_at: AtClosed(Failed) })] }) }
    val done9 = ev_node_closed(s, 9, 0, Done, -1)
    val failed9 = ev_node_closed(s, 9, 0, Failed, -1)
    val merged9 = ev_node_closed(s, 9, 0, Superseded, 8)
    assert(and {
      not(stale_decision(s, 20)),
      not(stale_decision(done9, 20)) and stale_decision(failed9, 20),
      stale_decision(done9, 23) and not(stale_decision(failed9, 23)),
      not(stale_decision(merged9, 20)),                                      // a merge is not stale
      stale_decision(ev_node_closed(merged9, 8, 0, Failed, -1), 20),         // judged where it lives on
      stale_decision(s, 22) and not(stale_decision(s, 21)),
      stale_decision({ ...s, retracted: Set(TNode(9)) }, 20),
      stale_decision({ ...s, retracted: Set(TEntry(21)) }, 22),
      // a decision that is not active is never stale, even with a stale reference
      stale(failed9, BNode({ id: 9, life_at: AtOpen })) and not(stale_decision(failed9, 21)),
      // #9 merged into #8: a retraction of either the cited node or the node it lives on in
      not(stale_decision(merged9, 20)),
      stale_decision({ ...merged9, retracted: Set(TNode(9)) }, 20),
      stale_decision({ ...merged9, retracted: Set(TNode(8)) }, 20),
      not(stale_decision({ ...merged9, retracted: Set(TNode(10)) }, 20)),
    })
  }

  // ================================================================== flags (D.8.3)

  run flagsRequestTest = {
    val s0 = with_node(W0, work_node(8, "sub", Set(), Set()))
    val s = { ...s0, requests: s0.requests.put(20, user_request(8, 0, -1)) }
    assert(and {
      flags_of(s, 8, 0) == Set(FUnansweredRequest),
      not(f_unanswered_request(with_hold(s, 10, 1, 0), 8, 0)),     // attended through a child
      f_unanswered_request(with_hold(s, 10, 1, 0), 8, 3),          // the lease ended
      not(f_unanswered_request(set_wait(s, 8, WTime(5), 0), 8, 4)),
      f_unanswered_request(set_wait(s, 8, WTime(5), 0), 8, 5),
      f_unanswered_request(ev_node_closed(with_hold(s, 10, 1, 0), 8, 0, Done, -1), 8, 0),
      not(f_unanswered_request({ ...s, requests: s.requests.setBy(20, r => { ...r, settled: SettledBy({ decision: 1, answer: 2 }) }) }, 8, 0)),
    })
  }

  run flagsCommitmentTest = {
    val cm: Commitment = { node: 8, conv: Conv("c1"), last_ack: 0, followup_at: 6, followup_fired: false, state: CmOpen, moved_from: -1 }
    val s = { ...W0, commitments: Map(20 -> cm) }
    val asked = { ...s, pquestions: Map(21 -> pq_rec(8, Conv("c1"), false, 0, 8)) }
    val other = { ...s, pquestions: Map(21 -> pq_rec(8, Conv("c9"), false, 0, 8)) }
    val expired = { ...s, pquestions: Map(21 -> { ...pq_rec(8, Conv("c1"), false, 2, 8), state: PqExpired }) }
    val elsewhere = { ...s, pquestions: Map(21 -> pq_rec(9, Conv("c1"), false, 0, 8)) }
    assert(and {
      not(f_open_commitment(s, 8, 5)) and f_open_commitment(s, 8, 6),
      not(f_open_commitment(asked, 8, 6)) and f_open_commitment(other, 8, 6),
      f_open_commitment(expired, 8, 6),                       // only an open question holds it back
      f_open_commitment(elsewhere, 8, 6),                     // a question on another node does not
      not(f_open_commitment(s, 9, 6)),                        // the commitment is on #8 only
      not(f_open_commitment({ ...s, commitments: Map(20 -> { ...cm, state: CmFulfilled(30) }) }, 8, 6)),
      not(f_open_commitment(with_hold(s, 8, 1, 4), 8, 6)),
      f_open_commitment(ev_node_closed(s, 8, 0, Done, -1), 8, 0),
    })
  }

  run flagsMiscTest = {
    val s0 = with_node(W0, work_node(8, "sub", Set(), Set()))
    val said: Said = { node: 9, conv: Conv("c2"), kind: SAck, session: 1, state: SdUnconfirmed, moved_from: -1 }
    val s = { ...s0,
      saids: Map(20 -> said),
      objections: Map(21 -> { node: 9, state: ObOpen, moved_from: -1 }),
      user_entries: Map(22 -> { session: 1, principal: 0, at: 0, routing: Unrouted }) }
    val intent = upd_node(s, 8, nd => { ...nd, open_intent: { eid: 23, session: 1 } })
    val series = upd_node(s, 9, nd => { ...nd, role: Series({ repeat: R10, last_missed: true }) })
    val marks = upd_node(s, 9, nd => { ...nd, recheck_marks: Set(30), assume_marks: Set(31), attempts: TOO_BIG_ATTEMPTS, due: 4 })
    assert(and {
      flags_of(s, USER, 0) == Set(FUnrouted),
      flags_of(s, 9, 0) == Set(FUnconfirmedSaid, FObjection),
      not(f_unconfirmed_said(with_hold(s, 9, 1, 0), 9, 0)),
      f_ready_to_close(ev_node_closed(held_once(s, 8), 10, 0, Done, -1), 8, 0) and not(f_ready_to_close(held_once(s, 8), 8, 0)),
      // a node never held is a folder: its only child's closing does not make it ready (1.6)
      not(f_ready_to_close(ev_node_closed(s, 10, 0, Done, -1), 8, 0)),
      // a reserved node whose only child is closed is never ready to close
      not(f_ready_to_close(ev_node_closed(held_once(with_node(W0, work_node(PEOPLE, "x", Set(), Set())), PEOPLE), 10, 0, Done, -1), PEOPLE, 0)),
      f_ready_to_close(ev_node_closed(held_once(with_node(W0, work_node(9, "x", Set(), Set())), 9), 10, 0, Done, -1), 9, 0),
      f_open_intent(intent, 8, 0) and not(f_open_intent(with_hold(intent, 8, 2, 0), 8, 0)),
      f_open_intent(with_hold(intent, 10, 2, 0), 8, 0),        // a hold on a child is not a hold on #8
      not(f_open_intent(set_wait(intent, 8, WNode(9), 0), 8, 0)),
      f_missed(series, 9, 0) and not(f_missed(ev_node_closed(series, 9, 0, Done, -1), 9, 0)),
      flags_of(marks, 9, 3) == Set(FUnconfirmedSaid, FObjection, FQuestionRecheck, FProceedOnAssumption, FTooBig),
      flags_of(marks, 9, 4).contains(FOverdue),
      not(f_too_big(ev_node_closed(marks, 9, 0, Done, -1), 9, 0)),
      CLOSED_TAKE == Set(FUnansweredRequest, FOpenCommitment, FObjection, FOpenIntent, FUnconfirmedSaid, FBasisChanged),
      T0 == Set(FUnrouted, FUnansweredRequest, FOpenCommitment, FUnconfirmedSaid, FOpenIntent,
                FQuestionRecheck, FProceedOnAssumption, FBasisChanged),
      // finished items raise no flag
      not(f_objection({ ...s, objections: Map(21 -> { node: 9, state: ObResolved(22), moved_from: -1 }) }, 9, 0)),
      not(f_unconfirmed_said({ ...s, saids: Map(20 -> { ...said, state: SdPending }) }, 9, 0)),
      not(f_unconfirmed_said({ ...s, saids: Map(20 -> { ...said, state: SdConfirmed }) }, 9, 0)),
      not(f_unrouted({ ...s, user_entries: Map(22 -> { session: 1, principal: 0, at: 0, routing: RtNoAction }) }, USER, 0)),
    })
  }

  run flagsBasisTest = {
    val s0 = with_node(W0, work_node(ROOT, "w3", Set(), Set()))
    val s = { ...s0, decisions: Map(20 -> { node: 10, settles: Set(), state: DActive, basis: [BNode({ id: 9, life_at: AtOpen })] }) }
    val failed = ev_node_closed(s, 9, 0, Failed, -1)
    assert(and {
      not(f_basis_changed(s, 10, 0)),
      f_basis_changed(failed, 10, 0),
      f_basis_changed(ev_node_closed(failed, 10, 0, Done, -1), 10, 0),          // closed nodes carry it
      not(f_basis_changed(ev_node_closed(failed, 10, 0, Superseded, 8), 10, 0)), // never a merged node
      not(f_basis_changed(failed, 8, 0)) and not(f_basis_changed(failed, 9, 0)),    // only #10's decision
    })
  }

  // ================================================================== the queue (D.4.8)

  run ownerOrderTest = assert(and {
    // the kind order of §17, one constant per kind
    [K_LEASE, K_WAIT, K_RECHECK, K_USER_QUESTION, K_COMMITMENT, K_OCCURRENCE_SLOT, K_OCCURRENCE_WINDOW,
     K_REFUSAL_EXPIRY, K_DUE, K_PAUSE, K_SILENCE, K_RETIRE] == [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
    [O_HOLD, O_NODE, O_ENTRY, O_REFUSAL, O_CLASS, O_SESSION] == [0, 1, 2, 3, 4, 5],
    class_index("a") == 0 and class_index("b") == 1 and class_index("w") == 2 and class_index("x") == -1,
    owner_refusal(10, "b") == 3000101,
    owner_hold((12, 3)) < owner_node(0),
    owner_node(99) < owner_entry(1),
    owner_entry(999) < owner_refusal(0, "a"),
    owner_refusal(10, "b") < owner_refusal(11, "a"),
    owner_refusal(99, "w") < owner_class("a"),
    owner_class("w") < owner_session(1),
    key_lt({ at: 1, kind: 11, owner: 0 }, { at: 2, kind: 0, owner: 0 }),
    key_lt({ at: 2, kind: 0, owner: 9 }, { at: 2, kind: 1, owner: 0 }),
    key_lt({ at: 2, kind: 1, owner: 0 }, { at: 2, kind: 1, owner: 1 }),
  })

  run queueTest = {
    val s = with_hold(W0, 8, 1, 0)
    assert(and {
      q(s) == Set(
        { at: 3, kind: K_LEASE, owner: owner_hold((8, 1)) },
        { at: 3, kind: K_SILENCE, owner: owner_session(1) },
        { at: 3, kind: K_SILENCE, owner: owner_session(2) },
        { at: 10, kind: K_RETIRE, owner: owner_session(1) },
        { at: 10, kind: K_RETIRE, owner: owner_session(2) }),
      first_key(s) == { at: 3, kind: K_LEASE, owner: owner_hold((8, 1)) },
      next_deadline(s) == 3,
      deadline_of(s, K_RETIRE, owner_session(2)) == 10,
      deadline_of(s, K_RETIRE, owner_session(3)) == -1,
      settled(s, 2) and not(settled(s, 3)),
      first_key(quiet(genesis(0))) == NO_KEY and settled(genesis(0), 100),
      // a class's key is its earliest window end
      first_key({ ...quiet(W0), classes: Map("a" -> { caps_seen: Map(Set() -> 6, Set("gpu") -> 12) }) })
        == { at: 6, kind: K_REFUSAL_EXPIRY, owner: owner_class("a") },
    })
  }

  // ================================================================== base changes (D.7.4)

  run nodeCreatedTest = {
    val due_past = ev_node_created({ ...W0, last_now: 5 }, { ...work_node(ROOT, "d", Set(), Set()), due: 3 }, 5)
    val due_now = ev_node_created(W0, { ...work_node(ROOT, "d", Set(), Set()), due: 5 }, 5)
    val due_later = ev_node_created(W0, { ...work_node(ROOT, "d", Set(), Set()), due: 6 }, 5)
    val aliased = { ...W0, aliases: W0.aliases.put((ROOT, Lit("d")), 9).put((ROOT, Lit("e")), 9) }
    val c = ev_node_created(aliased, work_node(ROOT, "d", Set(), Set()), 0)
    assert(and {
      due_past.nodes.get(10).due_fired and due_now.nodes.get(10).due_fired,   // `due_fired := due <= at`
      not(due_later.nodes.get(10).due_fired),
      c.names.get((ROOT, Lit("d"))) == 10,
      not(c.aliases.keys().contains((ROOT, Lit("d")))),                         // the reused place's alias leaves
      c.aliases.get((ROOT, Lit("e"))) == 9,
      c.nodes.get(10).created_at == 0 and c.next_node == 11,
    })
  }

  run holdEndedTest = {
    val s = with_hold(upd_node(quiet(W0), 9, nd => { ...nd, tags: Set() }), 9, 1, 0)
    val cp = { ...s, holds: s.holds.setBy((9, 1), h => { ...h, checkpointed: true }) }
    val moved = ev_hold_ended(s, (9, 1), HeEndedByClose(CauseMove))
    val released = ev_hold_ended(s, (9, 1), HeReleased)
    val closed = ev_hold_ended(s, (9, 1), HeEndedByClose(CauseClose))
    val merged = ev_hold_ended(s, (9, 1), HeEndedByClose(CauseMerge))
    assert(and {
      // a move without a checkpoint is an attempt and offers the node (A-6, A-9)
      moved.nodes.get(9).attempts == 1 and moved.nodes.get(9).tags == Set(OFFER),
      ev_hold_ended(cp, (9, 1), HeEndedByClose(CauseMove)).nodes.get(9).attempts == 0,
      // a release offers the node and is no attempt
      released.nodes.get(9).attempts == 0 and released.nodes.get(9).tags == Set(OFFER),
      // a close or a merge is neither
      closed.nodes.get(9).attempts == 0 and closed.nodes.get(9).tags == Set(),
      merged.nodes.get(9).attempts == 0 and merged.nodes.get(9).tags == Set(),
      released.holds == Map(),
    })
  }

  run assumeOnceTest = {
    val s = { ...W0, pquestions: Map(2 -> { ...pq_rec(8, Conv("c1"), false, 2, 8), state: PqExpired, assumed_by: 30 }) }
    val u = { ...W0, uquestions: Map(4 -> { ...uq_rec(SrcAgent({ session: 1, digest: 0 }), 9, [0], 5, false), state: UqLapsed, assumed_by: 31 }) }
    // one expiry marks two waiting nodes; a decision on each records the assumption once: the
    // second `PqAssumed` would fail its precondition (D.7.4), so it is skipped (README, "Open
    // questions": D.8.17 names `assumed_by = None` for the user question only)
    val w = with_node({ ...PQ, pquestions: PQ.pquestions.setBy(2, x => { ...x, rechecks: 1 }) }, work_node(ROOT, "w3", Set(), Set()))
    val b = run_to(set_wait(set_wait(w, 9, WQuestion({ q: 2, rechecks_at_set: 1 }), 0), 10, WQuestion({ q: 2, rechecks_at_set: 1 }), 0), 8)
    val d1 = b.next_eid
    val c1 = record_assumed(append_decision(b, 9, 8, 1, Set()), 9, d1)
    val c2 = record_assumed(append_decision(c1, 10, 8, 1, Set()), 10, c1.next_eid)
    assert(and {
      assume_one(s, 2, 40).pquestions.get(2).assumed_by == 30,
      assume_one(u, 4, 40).uquestions.get(4).assumed_by == 31,
      assume_one({ ...s, pquestions: s.pquestions.setBy(2, x => { ...x, assumed_by: -1 }) }, 2, 40).pquestions.get(2).assumed_by == 40,
      // a question that is not expired is not assumed
      assume_one({ ...s, pquestions: s.pquestions.setBy(2, x => { ...x, state: PqOpen, assumed_by: -1 }) }, 2, 40).pquestions.get(2).assumed_by == -1,
      b.nodes.get(9).assume_marks == Set(2) and b.nodes.get(10).assume_marks == Set(2),
      c2.pquestions.get(2).assumed_by == d1,
      c2.nodes.get(9).assume_marks == Set() and c2.nodes.get(10).assume_marks == Set(),
      c2.anomalies == 0,
    })
  }

  run createUqTest = {
    val s = quiet(W0)
    val src = SrcAgent({ session: 1, digest: 0 })
    assert(and {
      create_uq(s, src, 8, false, -1, 2, 1, 5).uquestions.get(1).due == 5,      // a past due is clamped (A-68)
      create_uq(s, src, 8, false, -1, 9, 1, 5).uquestions.get(1).due == 9,
      create_uq(s, src, 8, true, -1, -1, 1, 5).uquestions.get(1) == {
        source: src, about: 8, to: [USER_PRINCIPAL], stage: 0, due: 5 + USER_QUESTION_DUE_ABSENT,
        has_default: true, state: UqPending, assumed_by: -1 },
      create_uq(s, src, 8, true, -1, -1, 1, 5).entries.get(1) == { node: USER_DECISIONS, seq: 1, at: 5, by: 1, ty: EQuestion, principal: -1 },
    })
  }

  // ================================================================== firing (D.8.2)

  run leaseTest = {
    val untagged = upd_node(W0, 9, nd => { ...nd, tags: Set() })
    val s = with_hold(untagged, 9, 1, 0)
    val r = settle(s, 3, 10)
    val cp = { ...s, holds: s.holds.setBy((9, 1), h => { ...h, checkpointed: true }) }
    assert(and {
      not(r.saturated) and not(r.stuck),
      r.state.holds == Map(),
      r.state.nodes.get(9).attempts == 1,
      r.state.nodes.get(9).tags == Set(OFFER),                 // the server's offer tag (§10.3)
      r.state.last_now == 3,
      r.state.sessions.keys().forall(x => r.state.sessions.get(x).silent),
      run_to(cp, 3).nodes.get(9).attempts == 0,                // a checkpointed hold is no attempt
      settled(r.state, 3) and not(settled(r.state, 10)),
      run_to(r.state, 10).sessions == Map(),                   // Retire (CS-8)
    })
  }

  run leaseContainerTest = {
    val s0 = with_node(quiet(W0), { ...work_node(ROOT, "s", Set(), Set()), role: Series({ repeat: R10, last_missed: false }) })
    val s1 = with_node(s0, work_node(PEOPLE, "bob", Set(), Set()))
    val s = with_hold(with_hold(s1, 10, 1, 0), 11, 2, 0)
    val r = run_to(s, 3)
    assert(and {
      r.holds == Map(),
      r.nodes.get(10).tags == Set() and r.nodes.get(11).tags == Set(),   // containers (A-113)
      r.nodes.get(10).attempts == 1,
    })
  }

  /// A series #10 (period 10, run_within 4) and its first occurrence #11 at slot 10.
  pure val OCC: State = {
    val s0 = with_node(quiet(W0), { ...work_node(ROOT, "s", Set(), Set()), content: 7, status: 3, est: 30, needs: Set("ops"), role: Series({ repeat: R10, last_missed: false }) })
    open_next_occurrence(s0, 10, 0, 0)
  }

  run occurrenceOpenTest = {
    val o = OCC.nodes.get(11)
    assert(and {
      o.parent == 10 and o.name == Slot(10) and o.origin == OOccurrence,
      o.content == 7 and o.est == 30 and o.needs == Set("ops") and o.capacity == 1,
      o.status == 0,                                           // the series' status is not copied
      o.keys == Set(RepeatSlot((10, 10))),
      o.due == 14 and not(o.due_fired),
      o.role == Occurrence({ series: 10, slot: 10, window_end: 14, phase: Waiting, window_fired: false }),
      OCC.names.get((10, Slot(10))) == 11,
      q(OCC) == Set({ at: 10, kind: K_OCCURRENCE_SLOT, owner: owner_node(11) }, { at: 14, kind: K_DUE, owner: owner_node(11) }),
      next_slot(R10, 0) == 10 and next_slot(R10, 10) == 20 and next_slot(R10, 19) == 20,
      next_slot({ period: 10, offset: 5, run_within: 4 }, 0) == 5,
      // a live slot name takes the new node's id (D.8.18)
      open_next_occurrence(OCC, 10, 0, 0).nodes.get(12).name == SlotDup((10, 12)),
      open_next_occurrence(OCC, 10, 0, 0).nodes.get(12).keys == Set(),
    })
  }

  run occurrenceMissedTest = {
    val a = run_to(OCC, 10)
    val m = run_to(a, 14)
    assert(and {
      match a.nodes.get(11).role { | Occurrence(x) => x.phase == Available | _ => false },
      q(a) == Set({ at: 14, kind: K_OCCURRENCE_WINDOW, owner: owner_node(11) }, { at: 14, kind: K_DUE, owner: owner_node(11) }),
      closed_with(m, 11) == Missed,
      f_missed(m, 10, 14),
      m.nodes.get(12).role == Occurrence({ series: 10, slot: 20, window_end: 24, phase: Waiting, window_fired: false }),
      m.nodes.get(12).name == Slot(20),
      m.next_eid == 2 and m.nodes.get(11).seq == 1,           // the `Closed` entry, not kept
      m.entries == Map(),
      q(m) == Set({ at: 20, kind: K_OCCURRENCE_SLOT, owner: owner_node(12) }, { at: 24, kind: K_DUE, owner: owner_node(12) }),
    })
  }

  run occurrenceCheckpointTest = {
    val a = run_to(OCC, 10)
    val cp = upd_node(a, 11, nd => { ...nd, checkpoints: 1 })
    val held = with_hold(a, 11, 1, 12)
    val intent = upd_node(a, 11, nd => { ...nd, open_intent: { eid: 30, session: 1 } })
    val m = run_to(cp, 14)
    val h = run_to(held, 14)
    val after = run_to(held, 15)
    assert(and {
      is_open(run_to(intent, 14), 11),                         // an open intent keeps it open (D.8.18)
      is_open(m, 11) and f_overdue(m, 11, 14),                 // R2: never missed after a checkpoint
      is_open(h, 11),                                          // held at its window end
      closed_with(after, 11) == Missed,                        // its lease ended without a checkpoint
      after.nodes.get(11).attempts == 1,
      is_open(after, 12),
    })
  }

  run occurrenceNextTest = {
    // the next slot is the first after max(slot, close time): no catch-up (D.8.18)
    val a = run_to(OCC, 10)
    val early = close_node(a, 11, Done, 1, -1, CauseClose, -1, 12)
    val late = close_node(a, 11, Done, 1, -1, CauseClose, -1, 25)
    val reslot = close_node(OCC, 11, Dropped, 1, -1, CauseClose, 3, 3)
    assert(and {
      early.nodes.get(12).role == Occurrence({ series: 10, slot: 20, window_end: 24, phase: Waiting, window_fired: false }),
      match late.nodes.get(12).role { | Occurrence(x) => x.slot == 30 | _ => false },
      match reslot.nodes.get(12).role { | Occurrence(x) => x.slot == 10 | _ => false },   // reslot = now = 3
      early.nodes.get(10).role == Series({ repeat: R10, last_missed: false }),
    })
  }

  run seedOccurrenceBoundTest = {
    // the seeds' series, created at 0 and taken by nobody, stays within MAX_OCCURRENCES (§28)
    val s0 = with_node(quiet(W0), { ...work_node(ROOT, "s", Set(), Set()), role: Series({ repeat: SEED_REPEAT, last_missed: false }) })
    val s1 = open_next_occurrence(s0, 10, 0, 0)
    val r = run_to(s1, MAX_TIME)
    assert(and {
      may_open_occurrence(s1),
      occurrences_made(r) == Set(11, 12, 13),
      occurrences_made(r).size() <= MAX_OCCURRENCES,
      not(may_open_occurrence(r)),
      closed_with(r, 11) == Missed and closed_with(r, 12) == Missed and is_open(r, 13),
      match r.nodes.get(13).role { | Occurrence(x) => x.slot == 45 and x.phase == Waiting | _ => false },
      not(settle(s1, MAX_TIME, 30).saturated) and not(settle(s1, MAX_TIME, 30).stuck),
    })
  }

  run seriesCloseTest = {
    val s = close_node(OCC, 10, Done, 1, -1, CauseClose, -1, 3)
    assert(and {
      closed_with(s, 10) == Done and closed_with(s, 11) == Dropped,
      s.nodes.keys() == OCC.nodes.keys(),                     // no next occurrence (A-14)
      q(s) == Set(),
      s.next_eid == 3,
    })
  }

  /// A people question #2 on /w1 (#8), recorded with `said` #1.
  pure val PQ: State = { ...quiet(W0), next_eid: 3, pquestions: Map(2 -> pq_rec(8, Conv("c1"), false, 0, 8)) }

  run recheckTest = {
    val a = run_to(PQ, 8)
    val b = run_to(a, 16)
    assert(and {
      a.pquestions.get(2).rechecks == 1 and a.pquestions.get(2).recheck_at == 16,
      a.pquestions.get(2).state == PqOpen,
      a.nodes.get(8).recheck_marks == Set(2),
      f_question_recheck(a, 8, 8),
      b.pquestions.get(2).state == PqExpired and b.pquestions.get(2).lapse == LapseExpired,
      b.pquestions.get(2).rechecks == 2,
      b.digest == Map(3 -> { item: DiAssumption(2), to: USER_PRINCIPAL, state: DgPending }),
      b.entries.get(3) == { node: USER_DIGEST, seq: 1, at: 16, by: SERVER, ty: ESystem, principal: -1 },
      b.nodes.get(8).assume_marks == Set(2),
      q(b) == Set(),
    })
  }

  run recheckWaitersTest = {
    // /w2 (#9) waits on the question since its first recheck; the recheck resolves that wait and
    // marks #9, not the conversation node
    val s = set_wait({ ...PQ, pquestions: PQ.pquestions.setBy(2, x => { ...x, rechecks: 1 }) }, 9,
                     WQuestion({ q: 2, rechecks_at_set: 1 }), 0)
    val b = run_to(s, 8)
    // a wait set before the first recheck was resolved by it, so this recheck does not count it:
    // the conversation node is marked instead (D.8.2)
    val old = set_wait({ ...PQ, pquestions: PQ.pquestions.setBy(2, x => { ...x, rechecks: 1 }) }, 9,
                       WQuestion({ q: 2, rechecks_at_set: 0 }), 0)
    val o = run_to(old, 8)
    assert(and {
      b.pquestions.get(2).state == PqExpired,
      b.nodes.get(9).assume_marks == Set(2) and b.nodes.get(8).assume_marks == Set(),
      not(waiting(b, 9, 8)),
      o.nodes.get(8).assume_marks == Set(2) and o.nodes.get(9).assume_marks == Set(),
    })
  }

  run expiryClosedNodeTest = {
    // nobody waits and the conversation node is closed: the assumption is recorded at once (A-104)
    val s = ev_node_closed({ ...PQ, pquestions: PQ.pquestions.setBy(2, x => { ...x, rechecks: 1 }) }, 8, 0, Done, -1)
    val b = run_to(s, 8)
    assert(and {
      b.pquestions.get(2).state == PqExpired,
      b.digest.keys() == Set(3),
      b.decisions == Map(4 -> { node: 8, settles: Set(), state: DActive, basis: [] }),
      b.pquestions.get(2).assumed_by == 4,
      b.entries.get(4).ty == EDecision and b.entries.get(4).node == 8,
    })
  }

  run escalationTest = {
    val s = set_wait({ ...PQ, pquestions: PQ.pquestions.setBy(2, x => { ...x, rechecks: 1, blocking: true }) }, 9,
                     WQuestion({ q: 2, rechecks_at_set: 1 }), 0)
    val e = run_to(s, 8)
    val l = run_to(e, 13)
    assert(and {
      e.pquestions.get(2).state == PqEscalated(3) and e.pquestions.get(2).lapse == LapseEscalated,
      e.uquestions.get(3) == { source: SrcEscalation(2), about: 8, to: [USER_PRINCIPAL], stage: 0,
                               due: 8 + USER_QUESTION_DUE_ABSENT, has_default: false,
                               state: UqPending, assumed_by: -1 },
      match e.nodes.get(9).wait { | HasWait(w) => w.on == WUserQuestion(3) | NoWait => false },
      waiting(e, 9, 8) and e.nodes.get(9).assume_marks == Set(),            // no mark (A-93)
      e.entries.get(3) == { node: USER_DECISIONS, seq: 1, at: 8, by: SERVER, ty: EQuestion, principal: -1 },
      // `user` is absent, so the only stage lapses after due_absent (CS-2)
      l.uquestions.get(3).state == UqLapsed,
      l.digest == Map(4 -> { item: DiLapsed(3), to: USER_PRINCIPAL, state: DgPending }),
      l.nodes.get(9).assume_marks == Set(3),
      not(waiting(l, 9, 13)),
    })
  }

  run lapseNoNodeTest = {
    val s = { ...quiet(W0), next_eid: 5, uquestions: Map(4 -> uq_rec(SrcAgent({ session: 1, digest: 0 }), -1, [USER_PRINCIPAL], 5, false)) }
    val r = run_to(s, 5)
    val about = run_to({ ...s, uquestions: s.uquestions.setBy(4, u => { ...u, about: 9 }) }, 5)
    val reserved = run_to({ ...s, uquestions: s.uquestions.setBy(4, u => { ...u, about: USER }) }, 5)
    val closed = run_to(ev_node_closed({ ...s, uquestions: s.uquestions.setBy(4, u => { ...u, about: 9 }) }, 9, 0, Done, -1), 5)
    assert(and {
      r.uquestions.get(4).state == UqLapsed,
      r.digest.keys() == Set(5),
      r.decisions == Map(6 -> { node: USER_DECISIONS, settles: Set(), state: DActive, basis: [] }),
      r.uquestions.get(4).assumed_by == 6,
      about.nodes.get(9).assume_marks == Set(4) and about.decisions == Map(),
      // about a reserved node: no mark there, and the assumption is recorded on USER_DECISIONS
      reserved.nodes.get(USER).assume_marks == Set(),
      reserved.decisions == Map(6 -> { node: USER_DECISIONS, settles: Set(), state: DActive, basis: [] }),
      reserved.uquestions.get(4).assumed_by == 6,
      // about a closed node: no mark, and the assumption is recorded on that node (`assume_now`'s
      // home is `about` whenever it is a node that is not reserved, D.8.17)
      closed.nodes.get(9).assume_marks == Set(),
      closed.decisions.keys().map(d => closed.decisions.get(d).node) == Set(9),
    })
  }

  run readdressTest = {
    val s0 = with_principal(quiet(W0), "p1")
    val s = { ...s0, next_eid: 5, user_msgs: Map(1 -> [0, 2]),
              uquestions: Map(4 -> uq_rec(SrcAgent({ session: 1, digest: 0 }), 9, [1, USER_PRINCIPAL], 5, false)) }
    val r = run_to(s, 5)
    assert(and {
      r.uquestions.get(4).stage == 1,
      r.uquestions.get(4).due == 5 + USER_QUESTION_DUE_ABSENT,     // `user` is absent
      r.uquestions.get(4).state == UqPending,
      r.next_eid == 5 and r.digest == Map(),
      run_to(r, 10).uquestions.get(4).state == UqLapsed,
      run_to(r, 10).digest.get(5).to == 1,                          // the person asked (D.8.13)
    })
  }

  run defaultTest = {
    val s = { ...quiet(W0), next_eid: 5, uquestions: Map(4 -> uq_rec(SrcAgent({ session: 1, digest: 0 }), 9, [USER_PRINCIPAL], 5, true)) }
    val r = run_to(s, 5)
    assert(and {
      r.uquestions.get(4).state == UqDefaulted,
      r.nodes.get(USER_DECISIONS).seq == 1 and not(r.entries.keys().contains(5)),   // `Defaulted`, not kept
      r.digest == Map(6 -> { item: DiDefaultApplied(4), to: USER_PRINCIPAL, state: DgPending }),
      q(r) == Set(),
    })
  }

  /// A refused work node #10 with an answered user request #5 of p1; refusal question #7.
  pure val RQ: State = {
    val s0 = with_node(with_principal(quiet(W0), "p1"), work_node(ROOT, "r", Set(), Set()))
    {
      ...s0,
      next_eid: 8,
      requests: Map(5 -> user_request(10, 1, 6)),
      uquestions: Map(7 -> uq_rec(SrcRefusal(10), 10, [1, USER_PRINCIPAL], 5, true)),
      refusals: Map((10, "a") -> { quorum: 1, by: Map(1 -> { backoff_until: 4, expires_at: 12 }) }),
    }
  }

  run refusalDefaultTest = {
    val r = run_to(RQ, 5)
    val held = run_to(with_hold(RQ, 10, 1, 4), 5)
    val to_user = run_to({ ...RQ, uquestions: RQ.uquestions.setBy(7, u => { ...u, to: [USER_PRINCIPAL] }) }, 5)
    val settled4 = run_to({ ...RQ, requests: RQ.requests.put(4, { ...user_request(10, 1, 6), settled: SettledBy({ decision: 3, answer: 6 }) }) }, 5)
    assert(and {
      r.uquestions.get(7).state == UqDefaulted,
      r.refusals == Map(),
      r.decisions == Map(9 -> { node: 10, settles: Set(5), state: DActive, basis: [] }),
      r.requests.get(5).settled == SettledBy({ decision: 9, answer: 6 }),
      closed_with(r, 10) == Dropped,
      r.digest == Map(11 -> { item: DiRefusalDrop({ node: 10, question: 7 }), to: 1, state: DgPending }),
      r.next_eid == 12,
      // held: nothing to drop; the default applies, and the refusals are cleared all the same
      is_open(held, 10) and held.refusals == Map(),
      held.digest == Map(9 -> { item: DiDefaultApplied(7), to: 1, state: DgPending }),
      // the drop's audience is read before the requests settle (D.8.13): with the question put to
      // `user` alone, the requester p1 is still told
      to_user.digest.keys().map(e => to_user.digest.get(e).to) == Set(1),
      // the drop decision settles only the unsettled requests
      settled4.decisions.get(9).settles == Set(5),
    })
  }

  run classExpiryTest = {
    val s = { ...RQ, uquestions: Map(), classes: Map("a" -> { caps_seen: Map(Set() -> 6) }),
              refusals: Map((10, "a") -> { quorum: 1, by: Map(1 -> { backoff_until: 4, expires_at: 12 }) }) }
    val r = run_to(s, 6)
    val r2 = run_to(r, 12)
    assert(and {
      queue(s).contains({ at: 6, kind: K_REFUSAL_EXPIRY, owner: owner_class("a") }),
      queue(s).contains({ at: 12, kind: K_REFUSAL_EXPIRY, owner: owner_refusal(10, "a") }),
      r.classes == Map(),
      r.uquestions == Map(8 -> { source: SrcRefusal(10), about: 10, to: [1, USER_PRINCIPAL], stage: 0,
                                 due: 6 + USER_QUESTION_DUE, has_default: true,
                                 state: UqPending, assumed_by: -1 }),
      covered(r, 10),
      // the refusal expires; the question stays until its due
      r2.refusals == Map() and r2.uquestions.get(8).state == UqPending,
    })
  }

  run refusalExpiryTest = {
    val s = { ...quiet(W0), refusals: Map((8, "b") -> { quorum: 2, by: Map(1 -> { backoff_until: 5, expires_at: 7 }, 2 -> { backoff_until: 5, expires_at: 9 }) }) }
    val a = run_to(s, 7)
    val b = run_to(a, 9)
    assert(and {
      a.refusals.get((8, "b")).by.keys() == Set(2),
      b.refusals == Map(),
    })
  }

  /// 1.8 (§14, §17, D.4.8): an open node that is not reserved has a Stale key at `touched +
  /// OFFER_STALE` until it fires; `stale` holds from then on an unheld, unwaiting offer that is not
  /// a container; a touch, such as an entry or a time wait resolving, moves the key.
  pure def stale_keys(st: State): Set[DeadlineKey] = queue(st).filter(k => k.kind == K_STALE)

  run staleKeysTest = {
    val s = quiet(W0)
    val r = run_to(s, OFFER_STALE)
    val waited = set_wait(s, 9, WTime(5), 0)
    assert(and {
      stale_keys(s) == Set(
        { at: OFFER_STALE, kind: K_STALE, owner: owner_node(8) },
        { at: OFFER_STALE, kind: K_STALE, owner: owner_node(9) }),
      not(f_stale(s, 8, OFFER_STALE - 1)),
      f_stale(s, 8, OFFER_STALE),
      r.nodes.get(8).stale_fired and r.nodes.get(9).stale_fired,
      stale_keys(r) == Set(),
      f_stale(r, 8, OFFER_STALE),
      // an entry touches the node: its key moves, and the flag clears
      val t = append_entry(r, 8, OFFER_STALE + 1, 1, ENote, -1, false)
      and {
        t.nodes.get(8).touched == OFFER_STALE + 1 and not(t.nodes.get(8).stale_fired),
        stale_keys(t) == Set({ at: 2 * OFFER_STALE + 1, kind: K_STALE, owner: owner_node(8) }),
        not(f_stale(t, 8, OFFER_STALE + 1)),
      },
      // a `late` entry does not (H4)
      append_entry(r, 8, OFFER_STALE + 1, 1, ELate, -1, false).nodes.get(8).touched == 0,
      // a held node, one without `offer`, and a closed one are never stale
      not(f_stale(with_hold(s, 8, 1, OFFER_STALE - 1), 8, OFFER_STALE)),
      not(f_stale(upd_node(s, 9, nd => { ...nd, tags: Set() }), 9, OFFER_STALE)),
      not(f_stale(close_node(s, 9, Done, 1, -1, CauseClose, -1, 1), 9, OFFER_STALE)),
      // a time wait that resolves at its time touches its node
      run_to(waited, 5).nodes.get(9).touched == 5,
      not(f_stale(run_to(waited, OFFER_STALE), 9, OFFER_STALE)),
    })
  }

  run markerKindsTest = {
    val cm: Commitment = { node: 8, conv: Conv("c1"), last_ack: 0, followup_at: 6, followup_fired: false, state: CmOpen, moved_from: -1 }
    val s0 = set_wait(upd_node(quiet(W0), 9, nd => { ...nd, due: 4 }), 8, WExternal(5), 0)
    val s = { ...s0, commitments: Map(20 -> cm) }
    val r = run_to(s, 6)
    assert(and {
      q(s) == Set(
        { at: 4, kind: K_DUE, owner: owner_node(9) },
        { at: 5, kind: K_WAIT, owner: owner_node(8) },
        { at: 6, kind: K_COMMITMENT, owner: owner_entry(20) }),
      r.nodes.get(9).due_fired,
      match r.nodes.get(8).wait { | HasWait(w) => w.fired | NoWait => false },
      r.commitments.get(20).followup_fired,
      q(r) == Set(),
      r.last_now == 6,
      f_overdue(r, 9, 6) and f_open_commitment(r, 8, 6),
    })
  }

  run pauseExpiryTest = {
    val s = { ...quiet(W0), pauses: Map(8 -> { until: 7, last_renewed: 0, ttl: PAUSE_TTL, by: 1, class: "a" }) }
    val r = run_to(s, 7)
    assert(and {
      paused(s, 8, 6),
      r.pauses == Map(),
      r.nodes.get(8).seq == 1 and r.next_eid == 1 + 1,
      not(paused(r, 8, 7)),
    })
  }

  /// 1.4 (R27, D.4.8): a holder is never silent while a hold it took is live. Its Silence key waits
  /// for the end of its last lease; the Lease key fires first at that time, then the Silence key.
  run holderSilenceTest = {
    val s0 = with_hold(W0, 8, 1, 0)
    val s = { ...s0, holds: s0.holds.keys().mapBy(t => { ...s0.holds.get(t), lease_until: 6 }) }
    val r3 = run_to(s, 3)
    val r6 = run_to(s, 6)
    assert(and {
      silence_key_at(s, 1) == 6,
      silence_key_at(s, 2) == s.sessions.get(2).silence_at,
      not(r3.sessions.get(1).silent),
      r3.sessions.get(2).silent,
      r3.holds.keys().size() == 1,
      r6.sessions.get(1).silent,
      r6.holds == Map(),
    })
  }

  run silenceTest = {
    val s = { ...W0, saids: Map(
      20 -> { node: 8, conv: Conv("c1"), kind: SAck, session: 1, state: SdPending, moved_from: -1 },
      21 -> { node: 8, conv: Conv("c1"), kind: SReply, session: 2, state: SdConfirmed, moved_from: -1 },
      22 -> { node: 9, conv: Conv("c2"), kind: SAck, session: 2, state: SdPending, moved_from: -1 }) }
    val r = run_to(s, 3)
    assert(and {
      r.sessions.get(1).silent and r.sessions.get(2).silent,
      r.saids.get(20).state == SdUnconfirmed,
      r.saids.get(21).state == SdConfirmed,
      r.saids.get(22).state == SdUnconfirmed,
      f_unconfirmed_said(r, 8, 3),
      run_to(r, 10).sessions == Map(),
    })
  }

  run expiryPruneTest = {
    val note: Note = { node: 8, to: ToSession(1), until: 4, recipients: Set(1), pending_holder: false, read_by: Set() }
    val s = { ...quiet(W0),
      notes: Map(20 -> note, 21 -> { ...note, until: 6 }),
      aquestions: Map(20 -> { node: 8, asker: 1, state: AqOpen }),
      ops: Map(("a", "o1") -> { digest: CNext, session: 1, expires_at: 4 }, ("a", "o2") -> { digest: CNext, session: 1, expires_at: 9 }),
      pauses: Map(9 -> { until: 4, last_renewed: 0, ttl: PAUSE_TTL, by: 1, class: "a" }) }
    val r = run_to(s, 4)
    assert(and {
      r.notes.keys() == Set(21) and r.aquestions == Map(),
      r.ops.keys() == Set(("a", "o2")),
      // settling without a firing changes nothing: notes and records leave only at events (T1)
      run_to({ ...s, pauses: Map() }, 5) == { ...s, pauses: Map() },
    })
  }

  run closeNodeTest = {
    // a close by session 1 ends every hold, closes its own intent, clears recheck marks, records
    // the assumptions first (A-104), and resolves a signal
    val s0 = upd_node(with_hold(with_hold(quiet(W0), 8, 1, 0), 8, 2, 1), 8, nd =>
      { ...nd, capacity: 2, open_intent: { eid: 30, session: 1 }, recheck_marks: Set(31), assume_marks: Set(32), wait: HasWait({ on: WTime(9), set_at: 0, fired: false }) })
    val s = { ...s0, next_eid: 40, pquestions: Map(32 -> { ...pq_rec(8, Conv("c1"), false, 2, 0), state: PqExpired }) }
    val c = close_node(s, 8, Done, 1, -1, CauseClose, -1, 2)
    val other = close_node(s, 8, Done, 2, -1, CauseClose, -1, 2)
    val sig = upd_node(quiet(W0), 9, nd => { ...nd, role: RoleSignal({ kind: SigDuplicate, a: 8, b: 9 }) })
    val sc = close_node({ ...sig, signals: Map((SigDuplicate, 8, 9) -> 9) }, 9, Done, SERVER, -1, CauseClose, -1, 0)
    assert(and {
      c.holds == Map(),
      c.nodes.get(8).attempts == 0,                       // a close is no attempt (A-9)
      c.nodes.get(8).open_intent == NO_INTENT,
      other.nodes.get(8).open_intent == { eid: 30, session: 1 },
      c.nodes.get(8).recheck_marks == Set() and c.nodes.get(8).assume_marks == Set(),
      c.decisions == Map(40 -> { node: 8, settles: Set(), state: DActive, basis: [] }),
      c.pquestions.get(32).assumed_by == 40,
      c.nodes.get(8).wait == NoWait and c.nodes.get(8).tags == Set(),
      c.nodes.get(8).version == 2 and c.nodes.get(8).seq == 2 and c.next_eid == 42,
      closed_with(c, 8) == Done,
      sc.signals == Map() and closed_with(sc, 9) == Done,
    })
  }

  run e1CheckTest = {
    val s = { ...RQ, uquestions: Map(), classes: Map("a" -> { caps_seen: Map(Set() -> 20) }, "b" -> { caps_seen: Map(Set() -> 20) }) }
    val both = { ...s, refusals: s.refusals.put((10, "b"), { quorum: 1, by: Map(2 -> { backoff_until: 4, expires_at: 12 }) }) }
    assert(and {
      e1_check(s, 10, SERVER, 3) == s,                    // class b is active and has not refused
      e1_check(both, 10, SERVER, 3).uquestions.keys() == Set(8),
      e1_check(e1_check(both, 10, SERVER, 3), 10, SERVER, 3) == e1_check(both, 10, SERVER, 3),   // covered
    })
  }

  run trySettleTest = {
    val s = { ...RQ, decisions: Map(
      30 -> { node: 10, settles: Set(5), state: DRetracted(31), basis: [] },
      32 -> { node: 10, settles: Set(5), state: DSuperseded(33), basis: [] },
      33 -> { node: 10, settles: Set(), state: DActive, basis: [] }) }
    assert(and {
      settling(s, 5) == Set(32),                          // a superseded decision still settles
      try_settle(s, 5).requests.get(5).settled == SettledBy({ decision: 32, answer: 6 }),
      try_settle({ ...s, requests: s.requests.setBy(5, r => { ...r, answered_by: -1 }) }, 5).requests.get(5).settled == Unsettled,
      // settling is final (A-11): a settled request keeps its decision
      try_settle({ ...s, requests: s.requests.setBy(5, r => { ...r, settled: SettledBy({ decision: 33, answer: 6 }) }) }, 5)
        .requests.get(5).settled == SettledBy({ decision: 33, answer: 6 }),
    })
  }

  // ================================================================== settle (D.8.2, D.10.4)

  run saturationTest = {
    val s = { ...quiet(W0), pauses: Map(
      8 -> { until: 2, last_renewed: 0, ttl: PAUSE_TTL, by: 1, class: "a" },
      9 -> { until: 2, last_renewed: 0, ttl: PAUSE_TTL, by: 1, class: "a" }) }
    assert(and {
      settle(s, 2, 1).saturated,
      not(settle(s, 2, 2).saturated),
      settle(s, 2, 1).state.pauses.keys() == Set(9),      // the queue's minimum fired first
      settle(s, 1, 0) == { state: s, saturated: false, stuck: false },
      settle(s, 2, 0).saturated,
      // the target is max(now, last_now): a `now` behind `last_now` still fires what is due by it
      settle({ ...s, last_now: 5, pauses: Map(8 -> { ...s.pauses.get(8), until: 4 }) }, 3, 5).state.pauses == Map(),
      settle({ ...s, last_now: 3, pauses: Map(8 -> { ...s.pauses.get(8), until: 4 }) }, 3, 5).state.pauses.keys() == Set(8),
      not(settle(s, 2, 2).stuck),
    })
  }

  /// A broken firing arm: it leaves its key in the queue, and counts its calls in `anomalies`.
  pure def leave_key(st: State, k: DeadlineKey): State = { ...at_event(st, k.at), anomalies: st.anomalies + 1 }

  run settleStuckTest = {
    val s = { ...quiet(W0), pauses: Map(8 -> { until: 2, last_renewed: 0, ttl: PAUSE_TTL, by: 1, class: "a" }) }
    // an arm that leaves its own key (and counts its calls in `anomalies`): Rust's `Stuck(k)`
    // (D.8.2), never saturation; the fold stops after the first firing, and a stuck last firing of
    // the fold is seen too
    val keeps = settle_by(s, 5, 10, leave_key)
    val keeps_last = settle_by(s, 5, 1, leave_key)
    // an arm that queues a key before its own
    val earlier = settle_by(s, 5, 10, (st, k) => {
      val st1 = fire(st, k)
      { ...st1, pauses: st1.pauses.put(9, { until: k.at - 1, last_renewed: 0, ttl: PAUSE_TTL, by: 1, class: "a" }) }
    })
    // an arm that queues a key at its own time but after it in the order: not seen (see `Settle`)
    val same = settle_by(s, 5, 10, (st, k) => {
      val st1 = fire(st, k)
      if (k.owner == owner_node(8))
        { ...st1, sessions: st1.sessions.put(1, { ...new_session("a", 0), silent: true, retire_at: k.at }) }
      else st1
    })
    assert(and {
      keeps.stuck and not(keeps.saturated) and keeps.state.last_now == 2,
      keeps.state.anomalies == 1 and keeps.state.pauses.keys() == Set(8),   // one firing only
      keeps_last.stuck and not(keeps_last.saturated) and keeps_last.state.anomalies == 1,
      earlier.stuck and not(earlier.saturated),
      not(same.stuck) and same.state.sessions == Map(),
      settle_by(s, 5, 10, (st, k) => fire(st, k)) == settle(s, 5, 10),
      not(settle(s, 5, 10).stuck) and not(settle(s, 5, 10).saturated),
    })
  }

  /// A state with deadlines of most kinds: holds, sessions, an occurrence, a people question, a
  /// refusal question, and a class profile.
  pure val RICH: State = {
    val s0 = with_hold(with_hold({ ...W0, pquestions: Map(2 -> pq_rec(8, Conv("c1"), true, 0, 8)) }, 8, 1, 0), 9, 2, 1)
    val s1 = with_node(s0, { ...work_node(ROOT, "s", Set(), Set()), role: Series({ repeat: R10, last_missed: false }) })
    val s2 = open_next_occurrence(s1, 10, 0, 0)
    val s3 = set_wait(s2, 9, WQuestion({ q: 2, rechecks_at_set: 1 }), 0)
    {
      ...s3,
      next_eid: 3,
      classes: Map("a" -> { caps_seen: Map(Set() -> 12) }),
      refusals: Map((9, "a") -> { quorum: 1, by: Map(1 -> { backoff_until: 6, expires_at: 13 }) }),
    }
  }

  run sameInstantTest = {
    // at 14, the occurrence window (kind 6) fires before the class expiry (kind 7): the missed
    // close appends its `Closed` entry (eid 1) before e1_check asks the refusal question (eid 2)
    val a = { ...run_to(OCC, 10),
      classes: Map("a" -> { caps_seen: Map(Set() -> 14) }),
      refusals: Map((8, "a") -> { quorum: 1, by: Map(1 -> { backoff_until: 9, expires_at: 20 }) }) }
    val r = run_to(a, 14)
    assert(and {
      first_key(a) == { at: 14, kind: K_OCCURRENCE_WINDOW, owner: owner_node(11) },
      closed_with(r, 11) == Missed and r.nodes.get(11).seq == 1,
      r.next_eid == 3,
      r.uquestions.keys() == Set(2),
      r.uquestions.get(2).source == SrcRefusal(8),
      r.entries.get(2).node == USER_DECISIONS,
      r.classes == Map(),
    })
  }

  run settleOrderTest = {
    val r = settle(RICH, 40, 40)
    assert(and {
      not(r.saturated) and not(r.stuck),
      settled(r.state, 40),
      r.state.last_now <= 40,
      r.state.sessions == Map(),
      r.state.holds == Map(),
      r.state.pquestions.get(2).state != PqOpen,
      settle(r.state, 40, 40).state == r.state,           // idempotence (§17)
    })
  }

  /// T2 on a sample: settling to `a` and then to `b` equals settling to `b`.
  run settlePathIndependenceTest = {
    val ts = [0, 1, 3, 8, 10, 12, 13, 16, 24, 31, 40]
    assert(ts.indices().forall(i => ts.indices().forall(j =>
      i > j or run_to(run_to(RICH, ts[i]), ts[j]) == run_to(RICH, ts[j]))))
  }

  /// `first_key` is the minimum of `queue`, on every state the tests reach, and on every state a
  /// settle of RICH passes through.
  run firstKeyTest = {
    val fixtures = [genesis(0), W0, with_hold(W0, 8, 1, 0), OCC, run_to(OCC, 10), run_to(OCC, 14), run_to(OCC, 40),
                    PQ, run_to(PQ, 8), run_to(PQ, 16), RQ, run_to(RQ, 5), D0, D1, RICH,
                    { ...quiet(W0), classes: Map("a" -> { caps_seen: Map(Set() -> 6, Set("gpu") -> 12) }) },
                    { ...quiet(W0), refusals: Map((8, "b") -> { quorum: 2, by: Map(1 -> { backoff_until: 5, expires_at: 7 }, 2 -> { backoff_until: 5, expires_at: 9 }) }) }]
    assert(and {
      fixtures.foldl(true, (ok, x) => ok and first_key(x) == queue_min(x)),
      0.to(21).forall(i => { val x = settle(RICH, 40, i).state first_key(x) == queue_min(x) }),
      0.to(21).forall(i => not(settle(RICH, 40, i).stuck)),
      queue_min(RICH) != NO_KEY,
    })
  }

  run settleOccurrenceRunTest = {
    // the series opens a new occurrence after each one closes; with nobody taking them each is
    // missed, and every occurrence is closed or open with its own deadline (K6)
    val r = run_to(OCC, 40)
    assert(and {
      closed_with(r, 11) == Missed and closed_with(r, 12) == Missed and closed_with(r, 13) == Missed,
      open_occurrences(r, 10) == Set(14),
      f_missed(r, 10, 40),
      settled(r, 40),
    })
  }
}
