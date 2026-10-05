// -*- mode: Bluespec; -*-
/**
 * Run tests of the commands and the main module (D.10.1, WP-A2): one test or more per command
 * family of D.10.4, each family with a long text where its command has one (D.10.4, A-114), the
 * retry store (O1), the stale token's `late` entry (H4), presence (CS-14), the guards of D.10.7, and
 * the seed. WP-A4 (D.10.1, D.13.2): `scenario_32`, the example of §32 step by step as D.12.2 scripts
 * it, and `test_T2`, path independence of settling on every state of that script and on fixtures
 * that reach every other deadline kind.
 *
 *   cd model && npx quint test tests/forum_test.qnt
 *
 * Without `--match`, `quint test` runs only the runs whose names end in `Test`. `scenario_32` and
 * `test_T2` keep the names D.10.8 and D.13.2 give them, and run under that command through their
 * aliases `scenario32Test` and `t2Test` (README).
 *
 * The configuration is `two_class`'s with the tie-break by id, so that `next` picks the lowest id
 * among equals and the expected ids can be read off the fixtures.
 */
module forum_test {
  import types.* from "../types"
  import params.* from "../params"
  import basics.* from "../basics"
  import flags.* from "../flags"
  import deadlines.* from "../deadlines"
  import commands.* from "../commands"
  import forum(
    CLASSES = Set("a", "b"),
    USE_FNV = false,
    QUORUM = 2,
    PRINCIPALS = Set("user", "p1", "p2"),
    HAS_USER = true,
    MAX_FIRINGS = 40,
    SEED_SERIES = true,
    LONG_TEXTS = Set(false, true),
    RARE_ODDS = 1
  ).* from "../forum"

  // ================================================================== fixtures

  // The seed (D.10.7): sessions 1 (class a) and 2 (class b) opened at 0; /w1 (#8, key c1) and /w2
  // (#9, key c2), capacity 1, tagged `offer`; the series /s (#10) and its first occurrence (#11,
  // slot 15, window 19). No entries. Sessions fall silent at 3 and retire at 10.
  pure val W1 = 8
  pure val W2 = 9
  pure val SER = 10
  pure val OCC1 = 11

  /// One call by session `sess` (of its own class, or "a" when it does not exist) at `now`, with
  /// no `op` and no long text.
  pure def call(st: State, now: int, sess: int, cmd: Cmd): Run =
    run_cmd(CFG, st, now, if (has_session(st, sess)) st.sessions.get(sess).class else "a", sess, "", false, cmd)

  pure def after(st: State, now: int, sess: int, cmd: Cmd): State = call(st, now, sess, cmd).state
  pure def out(st: State, now: int, sess: int, cmd: Cmd): CmdOutcome = call(st, now, sess, cmd).outcome

  /// The same call with a free text over `text.max` (D.10.4): nothing changes.
  pure def long_rejected(st: State, now: int, sess: int, cmd: Cmd): bool = {
    val r = run_cmd(CFG, st, now, if (has_session(st, sess)) st.sessions.get(sess).class else "a", sess, "", true, cmd)
    r.state == st and r.outcome == OutErr(ErrLimit(LkTextTooLong))
  }

  /// The same call with `op` and a long text: even a live record of that `op` is not read (O1
  /// comes after the text check, A-114).
  pure def long_before_op(st: State, now: int, sess: int, op: str, cmd: Cmd): bool = {
    val r = run_cmd(CFG, st, now, st.sessions.get(sess).class, sess, op, true, cmd)
    r.state == st and r.outcome == OutErr(ErrLimit(LkTextTooLong))
  }

  /// Node `n` is closed, with outcome `o`; an open node never passes.
  pure def closed_as(st: State, n: int, o: Outcome): bool =
    match st.nodes.get(n).life { | Closed(c) => c.outcome == o | Open => false }

  pure def err(e: ErrorCode): CmdOutcome = OutErr(e)
  pure def why(w: Why): CmdOutcome = OutErr(ErrInvalid(w))
  pure def nfo(w: What): CmdOutcome = OutErr(ErrNotFound(w))

  pure def create_cmd(parent: int, name: str, ks: Set[Key], take: bool, capacity: int): Cmd =
    CCreate({ parent: parent, name: Lit(name), keys: ks, take: take, capacity: capacity, owners: Set(),
              repeat: RepKeep, tags: Set() })

  pure def update_none(n: int, ver: int): { node: int, ver: int, dver: int, content: int, status: int, digest: int,
      capacity: int, needs_add: Set[str], needs_remove: Set[str], tags_add: Set[str], tags_remove: Set[str],
      due: DueChange, depends: IdsChange, owners: IdsChange, repeat: RepeatChange } = {
    node: n, ver: ver, dver: -1, content: -1, status: -1, digest: -1, capacity: -1, needs_add: Set(),
    needs_remove: Set(), tags_add: Set(), tags_remove: Set(), due: DueKeep, depends: IdsKeep, owners: IdsKeep,
    repeat: RepKeep,
  }

  pure def take_cmd(n: int): Cmd = CTake({ node: n, lease: -1 })

  pure def close_cmd(n: int, o: Outcome): Cmd = CClose({ node: n, outcome: o, settles: Set(), token: NoToken })

  /// The seed with the two seed sessions touched at `t` (so that they stay live in a test that
  /// moves time by hand) and nothing else.
  pure def live_at(st: State, t: int): State = touched(touched(st, 1, t), 2, t)

  // ================================================================== the seed and the wiring

  run seedTest = assert(and {
    SEED.next_node == 12,
    SEED.next_eid == 1,
    SEED.next_session == 3,
    SEED.sessions.keys() == Set(1, 2),
    SEED.sessions.get(1).class == "a",
    SEED.sessions.get(2).class == "b",
    SEED.nodes.get(W1).keys == Set(Conv("c1")),
    SEED.nodes.get(W1).tags == Set(OFFER),
    SEED.nodes.get(W2).keys == Set(Conv("c2")),
    SEED.nodes.get(W1).origin == OAgent(1),
    is_series(SEED, SER),
    SEED.nodes.get(OCC1).name == Slot(15),
    open_occurrences(SEED, SER) == Set(OCC1),
    SEED.holds == Map(),
    SEED.entries == Map(),
    first_key(SEED) == { at: 3, kind: K_SILENCE, owner: owner_session(1) },
    settled(SEED, 0),
    SEED_CLASSES == ["a", "b"],
  })

  /// `init`, a command action, and a time action, through the actions of `forum.qnt`.
  run wiringTest =
    init
      .then(all { assert(s == SEED and clock == 0 and last_action.name == "init"), command("act_take", 1, "a", "o1", false, take_cmd(W1)) })
      .then(all {
        assert(and {
          s.holds.keys() == Set((W1, 1)),
          last_action.outcome == OutOk([W1, 1]),
          last_action.class == "a",
          last_action.settle == NO_FLAGS,
          prev == SEED,
          mid == s,                                             // the post-settle fired nothing
          s.ops.get(("a", "o1")).session == 1,
        }),
        act_tick_next,
      })
      .then(all {
        assert(and {
          clock == 3,
          last_action.outcome == OutNone,
          last_action.settle == NO_FLAGS,
          mid == prev,
          s.holds == Map(),                                     // the lease ended at 3
          s.nodes.get(W1).attempts == 1,
          s.sessions.get(2).silent,
        }),
        command("act_take", 9, "b", "", false, take_cmd(W1)),
      })
      .expect(last_action.outcome == nfo(WhatSession) and last_action.class == "b" and s == prev)

  /// D.8.1's post-settle through the actions (R27, D.4.8): session 2 takes /w1 at 0, session 1
  /// extends that hold at 1, and at 3, past session 2's `silence_at`, releases it with a `{time}`
  /// wait at 3. The release's command leaves session 2's Silence key at 3 (`mid` is not settled);
  /// `command`'s post-settle fires it, so the state after the action is settled and session 2 is
  /// silent, and `inv_all` holds on the command and the firing.
  run postSettleWiringTest =
    init
      .then(command("act_take", 2, "b", "", false, take_cmd(W1)))
      .then(tick32(1))
      .then(command("act_checkpoint", 1, "a", "", false, CCheckpoint({ token: (W1, 1), intent: false, intent_done: false })))
      .then(tick32(2))
      .then(all {
        assert(clock == 3 and not(s.sessions.get(2).silent) and s.holds.get((W1, 1)).lease_until == 5),
        command("act_release", 1, "a", "", false, CRelease({ token: (W1, 1), reason: Handoff, wait: WaTime(3), settles: Set() })),
      })
      .expect(and {
        last_action.outcome == OutOk([]),
        last_action.settle == NO_FLAGS,
        not(settled(mid, 3)),
        mid.sessions.get(2) != s.sessions.get(2),
        s.sessions.get(2).silent,
        settled(s, clock),
        w_post_settle,
        inv_all,
      })

  /// A command is enabled only on a state settled at `now`, and only within the bounds of D.10.7,
  /// which disable a command that would raise a counter past its bound but not one that leaves it
  /// where a firing put it.
  run guardsTest = {
    val due = { ...SEED, sessions: SEED.sessions.setBy(1, se => { ...se, silence_at: 0 }) }
    val full_eids = { ...SEED, next_eid: MAX_EID }
    val past_eids = { ...SEED, next_eid: MAX_EID + 2 }
    assert(and {
      not(settled(due, 0)),
      settled(SEED, 0),
      // an object appends an entry: at MAX_EID it would raise next_eid past the bound
      not(within_bounds(2, full_eids, after(full_eids, 0, 1, CObject(W1)))),
      within_bounds(2, full_eids, after(full_eids, 0, 1, CView(W1))),
      within_bounds(2, past_eids, after(past_eids, 1, 1, CView(W1))),
      not(within_bounds(2, past_eids, after(past_eids, 1, 1, CObject(W1)))),
      // a hello opens the class of its session id: ids 1 and 3 for a, 2 and 4 for b (D.10.7)
      [1, 2, 3, 4].foldl([], (acc, x) => acc.append(hello_class(x))) == ["a", "b", "a", "b"],
      // a fifth session is past SESSIONS_PER_CLASS × 2
      not(within_bounds(2, { ...SEED, next_session: 5 },
        run_cmd(CFG, { ...SEED, next_session: 5 }, 0, "a", -1, "", false, CHello({ caps: Set(), principal: "" })).state)),
      within_bounds(2, SEED, run_cmd(CFG, SEED, 0, "a", -1, "", false, CHello({ caps: Set(), principal: "" })).state),
      // a fifth agent-made node (the seed has three)
      within_bounds(2, SEED, after(SEED, 1, 1, create_cmd(W1, "a1", Set(), false, 1))),
      not(within_bounds(2, after(SEED, 1, 1, create_cmd(W1, "a1", Set(), false, 1)),
        after(after(SEED, 1, 1, create_cmd(W1, "a1", Set(), false, 1)), 1, 1, create_cmd(W1, "a2", Set(), false, 1)))),
    })
  }

  // ================================================================== session, presence, O1, H4

  pure val HELLO_P1: Cmd = CHello({ caps: Set("x"), principal: "p1" })

  /// `session hello` opens a session (D.8.23): a new principal is interned first (CS-1), and the
  /// `op_id` record names the new session.
  run helloTest = {
    val r = run_cmd(CFG, SEED, 1, "a", -1, "o1", false, HELLO_P1)
    val st = r.state
    assert(and {
      r.outcome == OutOk([3]),
      st.next_session == 4,
      st.sessions.get(3) == {
        class: "a", started_at: 1, last_seen: 1, silence_at: 1 + PRESENCE_SILENCE,
        retire_at: 1 + SESSION_RETAIN, silent: false, caps: Set("x"), horizon: -1, principals: Set(1),
        shown: Map(), branch_seen: Map(), handed_off: NO_HANDOFF,
      },
      st.principal_ids == Map("user" -> 0, "p1" -> 1),
      st.next_principal == 2,
      st.last_now == 1,
      st.ops == Map(("a", "o1") -> { digest: HELLO_P1, session: 3, expires_at: 1 + OP_RETAIN }),
      // the same hello again opens nothing: a new session names no session (O1)
      run_cmd(CFG, st, 2, "a", -1, "o1", false, HELLO_P1) == { state: st, outcome: OutReplay },
      run_cmd(CFG, st, 2, "a", -1, "o1", false, CHello({ caps: Set(), principal: "p1" })) == { state: st, outcome: err(ErrOpIdConflict) },
      // the key is (class, op): another class's o1 is a new call, and the known principal is not
      // interned again
      run_cmd(CFG, st, 2, "b", -1, "o1", false, HELLO_P1).outcome == OutOk([4]),
      run_cmd(CFG, st, 2, "b", -1, "o1", false, HELLO_P1).state.next_principal == 2,
      // a long text comes before the record (A-114)
      run_cmd(CFG, st, 2, "a", -1, "o1", true, HELLO_P1) == { state: st, outcome: err(ErrLimit(LkTextTooLong)) },
      // without a principal the session serves none
      after(SEED, 0, -1, CHello({ caps: Set(), principal: "" })).sessions.get(3).principals == Set(),
    })
  }

  /// `hello` delivers the notes to its principal (D.8.16): `InboxRead` marks them read.
  run helloInboxTest = {
    val noted = after(SEED, 1, 1, CSay({ at: W1, to: SayPrincipal("p1") }))
    val st = run_cmd(CFG, noted, 2, "b", -1, "", false, HELLO_P1).state
    assert(and {
      noted.notes.get(1).to == ToPrincipal(1),
      noted.notes.get(1).recipients == Set(),
      st.notes.get(1).read_by == Set(3),
      // a hello without the principal reads nothing
      run_cmd(CFG, noted, 2, "b", -1, "", false, CHello({ caps: Set(), principal: "" })).state.notes.get(1).read_by == Set(),
    })
  }

  /// Presence (CS-14, D.8.23): `Touched` when presence is due, the session is silent, or a hold is
  /// issued; a call that appends nothing returns the state itself (A-101); an error appends nothing.
  run presenceTest = {
    val viewed = call(SEED, 1, 1, CView(W1))
    val silent = { ...SEED, sessions: SEED.sessions.setBy(1, se => { ...se, silent: true }) }
    assert(and {
      // at 0 the session was seen at 0: a read of an unchanged node appends nothing
      call(SEED, 0, 1, CView(W1)) == { state: SEED, outcome: OutOk([]) },
      // a resolution later, presence is due
      viewed.outcome == OutOk([]),
      viewed.state.sessions.get(1).last_seen == 1,
      viewed.state.sessions.get(1).silence_at == 1 + PRESENCE_SILENCE,
      viewed.state.sessions.get(1).retire_at == 1 + SESSION_RETAIN,
      viewed.state.last_now == 1,
      // a silent session is touched at once, and live again
      not(after(silent, 0, 1, CView(W1)).sessions.get(1).silent),
      // a hold makes Touched due within the resolution
      after(SEED, 0, 1, take_cmd(W1)).sessions.get(1).retire_at == SESSION_RETAIN,
      after(SEED, 0, 1, take_cmd(W1)).holds.keys() == Set((W1, 1)),
      // an error appends nothing, Touched included
      call(SEED, 1, 1, CView(99)) == { state: SEED, outcome: nfo(WhatNode) },
      // retire_at never decreases
      touched({ ...SEED, sessions: SEED.sessions.setBy(1, se => { ...se, retire_at: 30 }) }, 1, 1).sessions.get(1).retire_at == 30,
    })
  }

  /// The retry store (O1, CS-8, A-106): a repeat of the same command from the record's session is a
  /// replay; another command, or the same from another session of the class, a conflict; an error
  /// that appended nothing is not recorded; a record answers until its expiry, and a retired
  /// session's record still answers its repeat.
  run retryTest = {
    val take1 = run_cmd(CFG, SEED, 1, "a", 1, "o1", false, take_cmd(W1))
    val st = take1.state
    val three = run_cmd(CFG, st, 1, "a", -1, "", false, CHello({ caps: Set(), principal: "" })).state
    val retired = { ...st, sessions: map_remove(st.sessions, 1) }
    assert(and {
      take1.outcome == OutOk([W1, 1]),
      st.ops == Map(("a", "o1") -> { digest: take_cmd(W1), session: 1, expires_at: 1 + OP_RETAIN }),
      run_cmd(CFG, st, 2, "a", 1, "o1", false, take_cmd(W1)) == { state: st, outcome: OutReplay },
      run_cmd(CFG, st, 2, "a", 1, "o1", false, take_cmd(W2)) == { state: st, outcome: err(ErrOpIdConflict) },
      run_cmd(CFG, three, 2, "a", 3, "o1", false, take_cmd(W1)) == { state: three, outcome: err(ErrOpIdConflict) },
      // a record of class a does not answer class b
      run_cmd(CFG, st, 2, "b", 2, "o1", false, take_cmd(W2)).outcome == OutOk([W2, 1]),
      // a retired session's repeat is still a replay (O1 before the session, D.8.1)
      run_cmd(CFG, retired, 2, "a", 1, "o1", false, take_cmd(W1)) == { state: retired, outcome: OutReplay },
      run_cmd(CFG, retired, 2, "a", 1, "", false, take_cmd(W1)) == { state: retired, outcome: nfo(WhatSession) },
      // an error that appended nothing is not recorded: the op runs afresh later
      run_cmd(CFG, SEED, 1, "a", 1, "o2", false, CView(99)) == { state: SEED, outcome: nfo(WhatNode) },
      // at its expiry the record no longer answers; the call runs afresh and the record is replaced
      run_cmd(CFG, st, 1 + OP_RETAIN, "a", 1, "o1", false, take_cmd(W2)).outcome == OutOk([W2, 1]),
      run_cmd(CFG, st, 1 + OP_RETAIN, "a", 1, "o1", false, take_cmd(W2)).state.ops.get(("a", "o1")).digest == take_cmd(W2),
      // an event prunes the expired record (CS-8)
      after(st, 1 + OP_RETAIN, 2, CObject(W2)).ops == Map(),
      // a call that appends nothing records nothing, op or not
      run_cmd(CFG, SEED, 0, "a", 1, "o2", false, CView(W1)) == { state: SEED, outcome: OutOk([]) },
    })
  }

  /// H4 (A-105, A-114): a stale token appends one `late` entry on its node (no metadata kept,
  /// CS-17), with the `op_id` record, and nothing else; a long text, a live record, an unknown
  /// session, and an unknown node append none.
  run staleTokenTest = {
    val ck = CCheckpoint({ token: (W1, 1), intent: false, intent_done: false })
    val r = run_cmd(CFG, SEED, 1, "a", 1, "o1", false, ck)
    val late = r.state
    val held = after(after(SEED, 1, 1, take_cmd(W1)), 1, 1, CRelease({ token: (W1, 1), reason: Handoff, wait: WaNone, settles: Set() }))
    assert(and {
      r.outcome == err(ErrStaleToken),
      late.nodes.get(W1).seq == 1,
      late.next_eid == 2,
      late.entries == Map(),
      late.last_now == 1,
      late.sessions == SEED.sessions,                      // no Touched
      late.holds == SEED.holds,
      late.nodes.get(W1) == { ...SEED.nodes.get(W1), seq: 1 },
      late.ops.get(("a", "o1")).session == 1,
      // a repeat is answered by the record, and appends no second late entry
      run_cmd(CFG, late, 2, "a", 1, "o1", false, ck) == { state: late, outcome: OutReplay },
      // each write with a token
      out(SEED, 1, 1, CRenew((W1, 1))) == err(ErrStaleToken),
      after(SEED, 1, 1, CRenew((W1, 1))).nodes.get(W1).seq == 1,
      out(SEED, 1, 1, CRelease({ token: (W1, 1), reason: Handoff, wait: WaNone, settles: Set() })) == err(ErrStaleToken),
      out(SEED, 1, 1, CClose({ node: W1, outcome: Done, settles: Set(), token: SomeToken((W1, 1)) })) == err(ErrStaleToken),
      after(SEED, 1, 1, CClose({ node: W1, outcome: Done, settles: Set(), token: SomeToken((W1, 1)) })).nodes.get(W1).seq == 1,
      // a token of an ended hold is stale too
      out(held, 2, 1, ck) == err(ErrStaleToken),
      // a token for another node is not a live hold on the target
      out(after(SEED, 1, 1, take_cmd(W2)), 1, 1, CClose({ node: W1, outcome: Done, settles: Set(), token: SomeToken((W2, 1)) }))
        == err(ErrStaleToken),
      // no late entry for a long text, an unknown session, or an unknown node
      long_rejected(SEED, 1, 1, ck),
      call(SEED, 1, 7, ck) == { state: SEED, outcome: nfo(WhatSession) },
      call(SEED, 1, 1, CCheckpoint({ token: (99, 1), intent: false, intent_done: false })) == { state: SEED, outcome: nfo(WhatNode) },
    })
  }

  /// A session of another class (A-69): nothing appended.
  run classMismatchTest = assert(
    run_cmd(CFG, SEED, 1, "b", 1, "o1", false, CView(W1)) == { state: SEED, outcome: why(WhyClassMismatch) }
  )


  // ================================================================== node (D.8.9, D.8.20)

  /// `node create` (D.8.9): a new node under /w1 with a hold; its fields; the defaults.
  run createTest = {
    val r = call(SEED, 1, 1, create_cmd(W1, "a1", Set(), true, 1))
    val nd = r.state.nodes.get(12)
    assert(and {
      r.outcome == OutOk([12, 1]),
      nd.parent == W1,
      nd.name == Lit("a1"),
      nd.origin == OAgent(1),
      nd.created_at == 1,
      nd.content == 1,
      nd.capacity == 1,
      nd.needs == Set(),
      nd.links == Set(),
      nd.role == Plain,
      nd.fence == 1,
      r.state.names.get((W1, Lit("a1"))) == 12,
      r.state.holds.get((12, 1)) == {
        session: 1, taken_at: 1, lease_until: 1 + LEASE_FIRST, last_extended: 1, terms: LEASE_TERMS,
        l: LEASE_DEFAULT, renews: 0, checkpointed: false,
      },
      r.state.sessions.get(1).last_seen == 1,
      r.state.next_node == 13,
      long_rejected(SEED, 1, 1, create_cmd(W1, "a1", Set(), true, 1)),
    })
  }

  /// No hold: `take: false` adds no tag; at `hold.per_session` or under a pause the node is offered
  /// (`no_hold`); with capacity 0 it gets no tag (A-65). The parent's `needs` (CS-26).
  run createNoHoldTest = {
    val two = after(after(SEED, 1, 1, take_cmd(W1)), 1, 1, take_cmd(W2))
    val paused_w1 = after(SEED, 1, 1, CPause(W1))
    val needy = after(SEED, 1, 1, CUpdate({ ...update_none(W1, 1), needs_add: Set("x") }))
    assert(and {
      call(SEED, 1, 1, create_cmd(W1, "a1", Set(), false, 1)).outcome == OutOk([12, -1]),
      after(SEED, 1, 1, create_cmd(W1, "a1", Set(), false, 1)).nodes.get(12).tags == Set(),
      call(two, 1, 1, create_cmd(W1, "a1", Set(), true, 1)).outcome == OutOk([12, -1]),
      after(two, 1, 1, create_cmd(W1, "a1", Set(), true, 1)).nodes.get(12).tags == Set(OFFER),
      after(paused_w1, 1, 1, create_cmd(W1, "a1", Set(), true, 1)).nodes.get(12).tags == Set(OFFER),
      after(paused_w1, 1, 1, create_cmd(W1, "a1", Set(), true, 1)).holds == Map(),
      after(SEED, 1, 1, create_cmd(W1, "a1", Set(), true, 0)).nodes.get(12).tags == Set(),
      after(SEED, 1, 1, create_cmd(W1, "a1", Set(), true, 0)).holds == Map(),
      after(needy, 1, 1, create_cmd(W1, "a1", Set(), false, 1)).nodes.get(12).needs == Set("x"),
    })
  }

  /// Join (D.8.9, §7): the open carrier with the smallest id gets the keys no open node carries, a
  /// duplicate signal opens for every other carrier, and no hold is issued; a join of keys the
  /// carrier has already appends nothing.
  run joinTest = {
    val j = call(SEED, 1, 1, create_cmd(W2, "a1", Set(Conv("c1"), Obj("o1")), true, 1))
    val dup = call(SEED, 1, 1, create_cmd(ROOT, "a1", Set(Conv("c1"), Conv("c2")), true, 1))
    assert(and {
      j.outcome == OutOk([W1]),
      j.state.nodes.get(W1).keys == Set(Conv("c1"), Obj("o1")),
      j.state.nodes.get(W1).version == 2,
      j.state.holds == Map(),
      j.state.next_node == 12,
      dup.outcome == OutOk([W1]),
      dup.state.signals == Map((SigDuplicate, W1, W2) -> 12),
      dup.state.nodes.get(12).parent == SIGNALS,
      dup.state.nodes.get(12).name == Signal((SigDuplicate, W1, W2, 12)),
      dup.state.nodes.get(12).origin == OSignal,
      dup.state.nodes.get(12).tags == Set(OFFER),
      dup.state.nodes.get(12).links == Set((Duplicate, W1), (Duplicate, W2)),
      // nothing to add: only Touched, when it is due
      call(SEED, 0, 1, create_cmd(W2, "a1", Set(Conv("c1")), true, 1)) == { state: SEED, outcome: OutOk([W1]) },
      // a second such join meets the open duplicate signal only: it appends nothing, so its op_id
      // is not recorded (D.8.1 step 6, D.8.19, O1)
      run_cmd(CFG, dup.state, 1, "a", 1, "o2", false, create_cmd(ROOT, "a1", Set(Conv("c1"), Conv("c2")), true, 1))
        == { state: dup.state, outcome: OutOk([W1]) },
    })
  }

  /// Create after the carriers closed: `continues` to the newest closed carrier (A-117); `person:`
  /// keys interned in key order; a user key routes its entry (D.8.9).
  run createKeysTest = {
    val closed = after(SEED, 1, 1, close_cmd(W1, Done))
    val again = after(after(closed, 1, 1, create_cmd(W2, "a1", Set(Conv("c1")), false, 1)), 1, 1, close_cmd(12, Done))
    val persons = after(SEED, 1, 1, create_cmd(PEOPLE, "a1", Set(Person("p2"), Person("p1")), false, 1))
    val entry = after(SEED, 1, 1, CFromUser({ principal: "p1", answers: -1, clarifies: -1, node: -1 }))
    val routed = after(entry, 1, 1, create_cmd(W1, "a1", Set(User(1)), false, 1))
    assert(and {
      after(closed, 1, 1, create_cmd(W2, "a1", Set(Conv("c1")), false, 1)).nodes.get(12).links == Set((Continues, W1)),
      after(again, 1, 1, create_cmd(W2, "a2", Set(Conv("c1")), false, 1)).nodes.get(13).links == Set((Continues, 12)),
      persons.principal_ids == Map("user" -> 0, "p1" -> 1, "p2" -> 2),
      aliases_of(persons, 12) == Set(1, 2),
      routed.nodes.get(12).keys == Set(User(1)),
      routed.requests.get(2) == {
        node: 12, origin: User(1), conv: User(1), from_user: 1, requester: ReqUser(1), answered_by: -1, settled: Unsettled,
        moved_from: -1,
      },
      routed.user_entries.get(1).routing == RtRequest({ node: 12, request: 2 }),
    })
  }

  /// Validation of create (D.8.9, D.8.24): `repeat:` keys, user keys naming no entry, the parent,
  /// the name, the owners, and L1 (depth, a series' first occurrence, children; none under PEOPLE).
  run createRefusalsTest = {
    val d2 = after(SEED, 1, 1, create_cmd(W1, "a1", Set(), false, 1))
    val d3 = after(d2, 1, 1, create_cmd(12, "a1", Set(), false, 1))
    val d4 = after(d3, 1, 1, create_cmd(13, "a1", Set(), false, 1))
    assert(and {
      out(SEED, 1, 1, create_cmd(W1, "a1", Set(RepeatSlot((SER, 15))), true, 1)) == why(WhyReservedKey),
      out(SEED, 1, 1, create_cmd(W1, "a1", Set(User(5)), true, 1)) == nfo(WhatUserEntry),
      out(SEED, 1, 1, create_cmd(99, "a1", Set(), true, 1)) == nfo(WhatNode),
      out(SEED, 1, 1, create_cmd(ROOT, "w1", Set(), true, 1)) == why(WhyNameTaken),
      out(SEED, 1, 1, CCreate({ parent: W1, name: Lit("a1"), keys: Set(), take: true, capacity: 1, owners: Set(99),
                                repeat: RepKeep, tags: Set() })) == nfo(WhatNode),
      // ROOT has three open agent-made children (/w1, /w2, /s): tree.max_children
      out(SEED, 1, 1, create_cmd(ROOT, "a1", Set(), true, 1)) == err(ErrLimit(LkChildren)),
      out(SEED, 1, 1, create_cmd(PEOPLE, "a1", Set(), true, 1)) == OutOk([12, 1]),
      // depth: ROOT 0, /w1 1, … d4's node #14 is at depth 4
      depth(d4, 14) == 4,
      out(d4, 1, 1, create_cmd(14, "a1", Set(), true, 1)) == err(ErrLimit(LkDepth)),
      out(d3, 1, 1, CCreate({ parent: 13, name: Lit("a1"), keys: Set(), take: false, capacity: 1, owners: Set(),
                              repeat: RepSet(SEED_REPEAT), tags: Set() })) == err(ErrLimit(LkDepth)),
      out(d2, 1, 1, CCreate({ parent: 12, name: Lit("a1"), keys: Set(), take: false, capacity: 1, owners: Set(),
                              repeat: RepSet(SEED_REPEAT), tags: Set() })) == OutOk([13, -1]),
      // a join validates nothing else (§7: it ignores the other fields)
      out(SEED, 1, 1, create_cmd(99, "w1", Set(Conv("c1")), true, 1)) == OutOk([W1]),
    })
  }

  /// A series created with `repeat` opens its first occurrence after `now` (D.8.18).
  run createSeriesTest = {
    val st = after(SEED, 1, 1, CCreate({ parent: W1, name: Lit("a1"), keys: Set(), take: false, capacity: 1,
                                         owners: Set(), repeat: RepSet(SEED_REPEAT), tags: Set() }))
    assert(and {
      st.nodes.get(12).role == Series({ repeat: SEED_REPEAT, last_missed: false }),
      open_occurrences(st, 12) == Set(13),
      st.nodes.get(13).role == Occurrence({ series: 12, slot: 15, window_end: 19, phase: Waiting, window_fired: false }),
      st.nodes.get(13).keys == Set(RepeatSlot((12, 15))),
    })
  }

  /// `node update` (D.8.20, O2, CS-34): content bumps `version` and `content_version`, status only
  /// `version`, the digest only `digest_version` and only with `digest_version` (CS-11); labels by
  /// additions and removals; a patch that changes nothing appends nothing.
  run updateTest = {
    val c2 = after(SEED, 1, 1, CUpdate({ ...update_none(W1, 1), content: 2 }))
    val st2 = after(c2, 1, 1, CUpdate({ ...update_none(W1, 2), status: 1 }))
    val dg = after(st2, 1, 1, CUpdate({ ...update_none(W1, 3), digest: 1, dver: 1 }))
    val tagged = after(SEED, 1, 1, CUpdate({ ...update_none(W1, 1), tags_add: Set(GONE), tags_remove: Set(OFFER), needs_add: Set("x") }))
    assert(and {
      c2.nodes.get(W1).content == 2,
      c2.nodes.get(W1).version == 2,
      c2.nodes.get(W1).content_version == 2,
      st2.nodes.get(W1).status == 1,
      st2.nodes.get(W1).version == 3,
      st2.nodes.get(W1).content_version == 2,
      out(st2, 1, 1, CUpdate({ ...update_none(W1, 3), digest: 1 })) == why(WhyMissingDigestVersion),
      dg.nodes.get(W1).digest == 1,
      dg.nodes.get(W1).digest_version == 2,
      dg.nodes.get(W1).version == 3,
      out(st2, 1, 1, CUpdate({ ...update_none(W1, 3), digest: 1, dver: 1 })) == OutOk([3, 2]),
      // stale versions change nothing (O2)
      call(st2, 1, 1, CUpdate({ ...update_none(W1, 2), content: 1 })) == { state: st2, outcome: err(ErrStaleVersion) },
      out(dg, 1, 1, CUpdate({ ...update_none(W1, 3), digest: 0, dver: 1 })) == err(ErrStaleVersion),
      out(dg, 1, 1, CUpdate({ ...update_none(W1, 3), content: 2, dver: 1 })) == err(ErrStaleVersion),
      tagged.nodes.get(W1).tags == Set(GONE),
      tagged.nodes.get(W1).needs == Set("x"),
      tagged.nodes.get(W1).version == 1,
      // removals first, then additions
      after(SEED, 1, 1, CUpdate({ ...update_none(W1, 1), tags_add: Set(OFFER), tags_remove: Set(OFFER) })).nodes.get(W1).tags == Set(OFFER),
      // nothing differs: no event (Touched is not due at 0)
      call(SEED, 0, 1, CUpdate({ ...update_none(W1, 1), tags_add: Set(OFFER), content: 1 })) == { state: SEED, outcome: OutOk([1, 1]) },
      long_rejected(SEED, 1, 1, CUpdate({ ...update_none(W1, 1), content: 2 })),
    })
  }

  /// `node update`: due, owners, depends, capacity; the refusals of D.8.20.
  run updateRefusalsTest = {
    val held = after(SEED, 1, 1, take_cmd(W1))
    val dep = after(SEED, 1, 1, CUpdate({ ...update_none(W1, 1), depends: IdsSet(Set(W2)) }))
    val due_now = after(SEED, 1, 1, CUpdate({ ...update_none(W1, 1), due: DueSet(1) }))
    assert(and {
      after(SEED, 1, 1, CUpdate({ ...update_none(W1, 1), due: DueSet(5) })).nodes.get(W1).due == 5,
      not(after(SEED, 1, 1, CUpdate({ ...update_none(W1, 1), due: DueSet(5) })).nodes.get(W1).due_fired),
      due_now.nodes.get(W1).due_fired,
      after(due_now, 1, 1, CUpdate({ ...update_none(W1, 1), due: DueClear })).nodes.get(W1).due == -1,
      // owners may be set on ROOT, the forum's owners (D.2.2)
      after(SEED, 1, 1, CUpdate({ ...update_none(ROOT, 1), owners: IdsSet(Set(W1)) })).nodes.get(ROOT).owners == Set(W1),
      out(SEED, 1, 1, CUpdate({ ...update_none(ROOT, 1), capacity: 1 })) == why(WhyReservedNode),
      out(SEED, 1, 1, CUpdate({ ...update_none(ROOT, 1), repeat: RepSet(SEED_REPEAT) })) == why(WhyReservedNode),
      out(SEED, 1, 1, CUpdate({ ...update_none(OCC1, 1), repeat: RepSet(SEED_REPEAT) })) == why(WhyRoleConflict),
      out(SEED, 1, 1, CUpdate({ ...update_none(W1, 1), owners: IdsSet(Set(99)) })) == nfo(WhatNode),
      out(SEED, 1, 1, CUpdate({ ...update_none(W1, 1), depends: IdsSet(Set(W1)) })) == err(ErrCycle),
      out(dep, 1, 1, CUpdate({ ...update_none(W2, 1), depends: IdsSet(Set(W1)) })) == err(ErrCycle),
      out(held, 1, 1, CUpdate({ ...update_none(W1, 1), capacity: 0 })) == err(ErrFull),
      out(held, 1, 1, CUpdate({ ...update_none(W1, 1), capacity: 2 })) == OutOk([1, 1]),
      out(SEED, 1, 1, CUpdate({ ...update_none(99, 1) })) == nfo(WhatNode),
    })
  }

  /// `update {repeat}` (D.8.18, K6): a new schedule drops a waiting occurrence and reslots from
  /// now; clearing it drops a waiting occurrence or detaches an available one; adding it to a
  /// plain node opens the first occurrence; on a closed node only the role changes.
  run updateRepeatTest = {
    val alt = after(SEED, 1, 1, CUpdate({ ...update_none(SER, 1), repeat: RepSet(ALT_REPEAT) }))
    val cleared = after(SEED, 1, 1, CUpdate({ ...update_none(SER, 1), repeat: RepClear }))
    val available = settle(live_at(SEED, 9), 15, 40).state
    val detached = after(available, 15, 1, CUpdate({ ...update_none(SER, 1), repeat: RepClear }))
    val plain = after(SEED, 1, 1, CUpdate({ ...update_none(W1, 1), repeat: RepSet(SEED_REPEAT) }))
    val closed = after(SEED, 1, 1, close_cmd(W1, Done))
    val closed_rep = after(closed, 1, 1, CUpdate({ ...update_none(W1, 2), repeat: RepSet(SEED_REPEAT) }))
    assert(and {
      alt.nodes.get(SER).role == Series({ repeat: ALT_REPEAT, last_missed: false }),
      closed_as(alt, OCC1, Dropped),
      open_occurrences(alt, SER) == Set(12),
      alt.nodes.get(12).role == Occurrence({ series: SER, slot: 20, window_end: 25, phase: Waiting, window_fired: false }),
      alt.nodes.get(SER).version == 1,
      // the same schedule again is no change (A-75)
      call(SEED, 0, 1, CUpdate({ ...update_none(SER, 1), repeat: RepSet(SEED_REPEAT) })) == { state: SEED, outcome: OutOk([1, 1]) },
      cleared.nodes.get(SER).role == Plain,
      closed_as(cleared, OCC1, Dropped),
      cleared.next_node == 12,
      occ_available(available.nodes.get(OCC1)),
      detached.nodes.get(OCC1).role == Plain,
      is_open(detached, OCC1),
      plain.nodes.get(W1).role == Series({ repeat: SEED_REPEAT, last_missed: false }),
      open_occurrences(plain, W1) == Set(12),
      closed_rep.nodes.get(W1).role == Series({ repeat: SEED_REPEAT, last_missed: false }),
      closed_rep.next_node == 12,
    })
  }

  /// `update {needs}` on a refused node runs `e1_check` (CS-7): an added need can leave no active
  /// class but the refusers.
  run updateNeedsE1Test = {
    val refused = {
      ...SEED,
      refusals: Map((W1, "a") -> { quorum: 1, by: Map(1 -> { backoff_until: 6, expires_at: 13 }) }),
      classes: Map("b" -> { caps_seen: Map(Set() -> 13) }),
    }
    val st = after(refused, 1, 1, CUpdate({ ...update_none(W1, 1), needs_add: Set("x") }))
    assert(and {
      not(all_refused(refused, W1, 1)),
      all_refused(st, W1, 1),
      st.uquestions.keys() == Set(1),
      st.uquestions.get(1).source == SrcRefusal(W1),
      st.uquestions.get(1).has_default,
      st.entries.get(1).by == 1,
      // a change of something else runs no check
      after(refused, 1, 1, CUpdate({ ...update_none(W1, 1), content: 2 })).uquestions == Map(),
    })
  }

  /// `node link` (D.8.20, D.8.19).
  run linkTest = {
    val dup = call(SEED, 1, 1, CLink({ node: W1, rel: Duplicate, to: W2 }))
    val sup = after(dup.state, 1, 1, CLink({ node: W2, rel: SupersededBy, to: W1 }))
    val conf = call(SEED, 1, 1, CLink({ node: W1, rel: ConflictsWith, to: W2 }))
    assert(and {
      dup.outcome == OutOk([12]),
      dup.state.nodes.get(W1).links == Set((Duplicate, W2)),
      dup.state.nodes.get(W1).version == 2,
      dup.state.signals == Map((SigDuplicate, W1, W2) -> 12),
      // identical: nothing
      call(dup.state, 1, 1, CLink({ node: W1, rel: Duplicate, to: W2 })) == { state: dup.state, outcome: OutOk([-1]) },
      // superseded_by resolves the pair's signals: the server closes the signal as done
      sup.signals == Map(),
      closed_as(sup, 12, Done),
      conf.outcome == OutOk([12]),
      conf.state.signals == Map((SigConflict, W1, W2) -> 12),
      out(SEED, 1, 1, CLink({ node: W1, rel: Continues, to: W1 })) == why(WhySelfLink),
      out(SEED, 1, 1, CLink({ node: W1, rel: Continues, to: 99 })) == nfo(WhatNode),
      after(SEED, 1, 1, CLink({ node: ROOT, rel: DerivedFrom, to: W1 })).nodes.get(ROOT).version == 2,
    })
  }

  /// `node key` (D.8.9): a key another open node carries opens a signal; a key the node has changes
  /// nothing; a `person:` key is interned; a user key routes; refusals.
  run keyTest = {
    val entry = after(SEED, 1, 1, CFromUser({ principal: "user", answers: -1, clarifies: -1, node: -1 }))
    val routed = after(entry, 1, 1, CKeyAdd({ node: W2, key: User(1) }))
    assert(and {
      call(SEED, 1, 1, CKeyAdd({ node: W1, key: Conv("c2") })).outcome == OutOk([12]),
      after(SEED, 1, 1, CKeyAdd({ node: W1, key: Conv("c2") })).nodes.get(W1).keys == Set(Conv("c1")),
      call(SEED, 0, 1, CKeyAdd({ node: W1, key: Conv("c1") })) == { state: SEED, outcome: OutOk([-1]) },
      after(SEED, 1, 1, CKeyAdd({ node: W1, key: Obj("o1") })).nodes.get(W1).keys == Set(Conv("c1"), Obj("o1")),
      after(SEED, 1, 1, CKeyAdd({ node: W1, key: Obj("o1") })).nodes.get(W1).version == 2,
      after(SEED, 1, 1, CKeyAdd({ node: W2, key: Person("p2") })).principal_ids.get("p2") == 1,
      routed.nodes.get(W2).keys == Set(Conv("c2"), User(1)),
      routed.user_entries.get(1).routing == RtRequest({ node: W2, request: 2 }),
      routed.nodes.get(W2).version == 2,
      out(SEED, 1, 1, CKeyAdd({ node: W1, key: RepeatSlot((SER, 15)) })) == why(WhyReservedKey),
      out(SEED, 1, 1, CKeyAdd({ node: ROOT, key: Obj("o1") })) == why(WhyReservedNode),
      out(SEED, 1, 1, CKeyAdd({ node: W1, key: User(4) })) == nfo(WhatUserEntry),
      out(SEED, 1, 1, CKeyAdd({ node: 99, key: Obj("o1") })) == nfo(WhatNode),
      after(SEED, 1, 1, CKeyRemove({ node: W1, key: Conv("c1") })).nodes.get(W1).keys == Set(),
      after(SEED, 1, 1, CKeyRemove({ node: W1, key: Conv("c1") })).nodes.get(W1).version == 2,
      out(SEED, 1, 1, CKeyRemove({ node: W1, key: Conv("c2") })) == nfo(WhatKey),
      out(SEED, 1, 1, CKeyRemove({ node: OCC1, key: RepeatSlot((SER, 15)) })) == why(WhyReservedKey),
      out(SEED, 1, 1, CKeyRemove({ node: ROOT, key: Obj("o1") })) == why(WhyReservedNode),
    })
  }

  /// `node close` (D.8.20): the hold ends, `offer` leaves, the version rises; a series drops its
  /// occurrence; a waiting occurrence closes only as dropped, and its next one opens.
  run closeTest = {
    val held = after(SEED, 1, 1, take_cmd(W1))
    val closed = call(held, 2, 1, CClose({ node: W1, outcome: Done, settles: Set(), token: SomeToken((W1, 1)) }))
    val ser = after(SEED, 1, 1, close_cmd(SER, Dropped))
    val occ = after(SEED, 1, 1, close_cmd(OCC1, Dropped))
    assert(and {
      closed.outcome == OutOk([-1]),
      closed_as(closed.state, W1, Done),
      closed.state.holds == Map(),
      closed.state.nodes.get(W1).tags == Set(),
      closed.state.nodes.get(W1).version == 2,
      closed.state.nodes.get(W1).attempts == 0,
      closed_as(ser, SER, Dropped),
      closed_as(ser, OCC1, Dropped),
      closed_as(occ, OCC1, Dropped),
      open_occurrences(occ, SER) == Set(12),
      occ.nodes.get(12).role == Occurrence({ series: SER, slot: 30, window_end: 34, phase: Waiting, window_fired: false }),
      out(SEED, 1, 1, close_cmd(OCC1, Done)) == why(WhyOccurrenceNotYetAvailable),
      out(SEED, 1, 1, close_cmd(W1, Missed)) == why(WhyServerOutcome),
      out(SEED, 1, 1, close_cmd(ROOT, Done)) == why(WhyReservedNode),
      out(closed.state, 2, 1, close_cmd(W1, Done)) == why(WhyAlreadyClosed),
      out(SEED, 1, 1, CClose({ node: W1, outcome: Done, settles: Set(5), token: NoToken })) == nfo(WhatRequest),
      out(SEED, 1, 1, close_cmd(99, Done)) == nfo(WhatNode),
      long_rejected(held, 2, 1, close_cmd(W1, Done)),
    })
  }

  /// `node close {settles}` records a decision that settles, once answered (§5, A-15); the
  /// session's pending `said` entries are confirmed (§9.2).
  run closeSettlesTest = {
    val req = after(SEED, 1, 1, CRequest({ at: W1, origin: Msg(("c1", 1)), where_key: SomeKey(Conv("c1")) }))
    val ack = after(req, 1, 1, CSaid({ at: W1, kind: SAck, where_key: NoKey }))
    // 1.11 (§9.2): session 2 holds /w2, so that its reply stays pending; session 1 holds nothing,
    // so its ack was confirmed as it was recorded
    val replied = after(after(ack, 1, 2, take_cmd(W2)), 1, 2, CSaid({ at: W1, kind: SReply, where_key: NoKey }))
    val r = call(replied, 2, 1, CClose({ node: W1, outcome: Done, settles: Set(1), token: NoToken }))
    assert(and {
      replied.requests.get(1).answered_by == 3,
      r.outcome == OutOk([4]),
      r.state.decisions.get(4) == { node: W1, settles: Set(1), state: DActive, basis: [] },
      r.state.requests.get(1).settled == SettledBy({ decision: 4, answer: 3 }),
      // the ack of session 1 is confirmed, the reply of session 2 stays pending
      ack.saids.get(2).state == SdConfirmed,
      r.state.saids.get(2).state == SdConfirmed,
      r.state.saids.get(3).state == SdPending,
    })
  }

  /// `node reopen` (D.8.20, K7, A-89): the keys another open node carries leave and a duplicate
  /// signal opens; a series opens its next occurrence; a locked occurrence is refused.
  run reopenTest = {
    val closed = after(SEED, 1, 1, close_cmd(W1, Done))
    val taken = after(closed, 1, 1, create_cmd(W2, "a1", Set(Conv("c1")), false, 1))
    val r = after(taken, 2, 1, CReopen(W1))
    val ser = after(after(SEED, 1, 1, close_cmd(SER, Dropped)), 2, 1, CReopen(SER))
    val occ = after(SEED, 1, 1, close_cmd(OCC1, Dropped))
    assert(and {
      is_open(r, W1),
      r.nodes.get(W1).keys == Set(),
      r.nodes.get(W1).version == 4,                       // close, KeyRemoved, NodeReopened
      r.signals == Map((SigDuplicate, W1, 12) -> 13),
      r.nodes.get(W1).seq == closed.nodes.get(W1).seq + 2,  // KeysDropped and Reopened
      is_open(ser, SER),
      open_occurrences(ser, SER) == Set(12),
      // the closed occurrence keeps the name Slot(15) (a live name is any name in `names`, D.8.18)
      ser.nodes.get(12).name == SlotDup((15, 12)),
      out(SEED, 1, 1, CReopen(W1)) == why(WhyNotClosed),
      out(occ, 1, 1, CReopen(OCC1)) == why(WhyOccurrenceReopen),
      out(SEED, 1, 1, CReopen(99)) == nfo(WhatNode),
      long_rejected(closed, 1, 1, CReopen(W1)),
    })
  }

  /// Reopening a merged node clears `merged_into` (§16); an occurrence of a closed series is
  /// detached (A-13); a signal node whose pair has another signal is detached (A-89).
  run reopenRolesTest = {
    val mg = after(SEED, 1, 1, CMerge({ from: W2, into: W1 }))
    val ser_closed = after(SEED, 1, 1, close_cmd(SER, Dropped))
    val sig = after(SEED, 1, 1, CLink({ node: W1, rel: Duplicate, to: W2 }))
    val sig_closed = after(sig, 1, 1, close_cmd(12, Done))
    val second = after(sig_closed, 1, 1, CLink({ node: W2, rel: Duplicate, to: W1 }))
    assert(and {
      merged(mg, W2),
      is_open(after(mg, 2, 1, CReopen(W2)), W2),
      not(merged(after(mg, 2, 1, CReopen(W2)), W2)),
      after(ser_closed, 2, 1, CReopen(OCC1)).nodes.get(OCC1).role == Plain,
      // the closed signal reopens as the pair's signal
      after(sig_closed, 2, 1, CReopen(12)).signals == Map((SigDuplicate, W1, W2) -> 12),
      // another signal for the pair opened meanwhile: the reopened one is detached
      second.signals == Map((SigDuplicate, W1, W2) -> 13),
      after(second, 2, 1, CReopen(12)).nodes.get(12).role == Plain,
      after(second, 2, 1, CReopen(12)).signals == Map((SigDuplicate, W1, W2) -> 13),
    })
  }

  /// `node move` (D.8.20, K4, L1, A-6, A-113): the old place becomes an alias; the holds of other
  /// sessions end and the node is offered; a move onto its own place changes nothing.
  run moveTest = {
    val r = after(SEED, 1, 1, CMove({ node: W2, parent: W1, name: Lit("a1") }))
    val held2 = after(SEED, 1, 2, take_cmd(W2))
    val moved = after(held2, 2, 1, CMove({ node: W2, parent: W1, name: Lit("a1") }))
    val own = after(held2, 2, 2, CMove({ node: W2, parent: W1, name: Lit("a1") }))
    assert(and {
      r.nodes.get(W2).parent == W1,
      r.nodes.get(W2).name == Lit("a1"),
      r.names.get((W1, Lit("a1"))) == W2,
      not(r.names.keys().contains((ROOT, Lit("w2")))),
      r.aliases.get((ROOT, Lit("w2"))) == W2,
      resolve(r, ROOT, Lit("w2")) == W2,
      r.nodes.get(W2).version == 1,
      moved.holds == Map(),
      moved.nodes.get(W2).attempts == 1,
      moved.nodes.get(W2).tags == Set(OFFER),
      own.holds.keys() == Set((W2, 1)),
      call(SEED, 0, 1, CMove({ node: W1, parent: ROOT, name: Lit("w1") })) == { state: SEED, outcome: OutOk([]) },
      out(SEED, 1, 1, CMove({ node: W1, parent: W1, name: Lit("a1") })) == err(ErrCycle),
      out(r, 1, 1, CMove({ node: W1, parent: W2, name: Lit("a2") })) == err(ErrCycle),
      out(SEED, 1, 1, CMove({ node: W2, parent: ROOT, name: Lit("w1") })) == why(WhyNameTaken),
      out(SEED, 1, 1, CMove({ node: ROOT, parent: W1, name: Lit("a1") })) == why(WhyReservedNode),
      out(SEED, 1, 1, CMove({ node: W2, parent: 99, name: Lit("a1") })) == nfo(WhatNode),
      // moving a child of /w1 to ROOT would give ROOT a fourth open agent-made child; once /w2 has
      // left ROOT there is room again
      out(after(SEED, 1, 1, create_cmd(W1, "a1", Set(), false, 1)), 1, 1, CMove({ node: 12, parent: ROOT, name: Lit("a2") }))
        == err(ErrLimit(LkChildren)),
      out(r, 1, 1, CMove({ node: W2, parent: ROOT, name: Lit("a2") })) == OutOk([]),
      // a rename in place does not count the node twice
      out(SEED, 1, 1, CMove({ node: W2, parent: ROOT, name: Lit("a2") })) == OutOk([]),
      // under PEOPLE there is no child limit, and the moved node is a person: no offer at a hold end
      after(held2, 2, 1, CMove({ node: W2, parent: PEOPLE, name: Lit("a1") })).nodes.get(W2).tags == Set(OFFER),
      after(after(held2, 1, 2, CUpdate({ ...update_none(W2, 1), tags_remove: Set(OFFER) })), 2, 1,
            CMove({ node: W2, parent: PEOPLE, name: Lit("a1") })).nodes.get(W2).tags == Set(),
    })
  }

  /// Move depth: `depth(new parent) + 1 + height(n)` (L1).
  run moveDepthTest = {
    val d2 = after(SEED, 1, 1, create_cmd(W1, "a1", Set(), false, 1))
    val d3 = after(d2, 1, 1, create_cmd(12, "a1", Set(), false, 1))
    assert(and {
      height(d3, W1) == 2,
      out(d3, 1, 1, CMove({ node: W1, parent: W2, name: Lit("a1") })) == OutOk([]),
      out(d3, 1, 1, CMove({ node: W1, parent: 13, name: Lit("a2") })) == err(ErrCycle),
      out(after(d3, 1, 1, create_cmd(W2, "a1", Set(), false, 1)), 1, 1, CMove({ node: W1, parent: 14, name: Lit("a2") }))
        == err(ErrLimit(LkDepth)),
    })
  }

  /// `node merge` (D.8.20, N5, A-95): the keys move; the open requests, people questions,
  /// commitments, objections, and open `said` entries move with `moved_from`, and so do the marks;
  /// what is finished stays; a moved request is not credited by a reply of another conversation
  /// (§9.4, 1.3); `from` closes as superseded; the pair's signals resolve.
  run mergeTest = {
    val req = after(SEED, 1, 1, CRequest({ at: W2, origin: Msg(("c1", 2)), where_key: SomeKey(Conv("c2")) }))
    val replied = after(req, 1, 1, CSaid({ at: W1, kind: SReply, where_key: NoKey }))
    val sig = after(replied, 1, 1, CLink({ node: W1, rel: Duplicate, to: W2 }))
    val m = after(sig, 2, 2, CMerge({ from: W2, into: W1 }))
    // open items on /w2: an ack (said 1, commitment 1), a question (said 2, question 3), an
    // objection (4), a confirmed ack (said 5, which refreshes commitment 1), a resolved objection
    // (6, resolved by 7), and the question's marks
    // 1.11 (§9.2): session 1 holds /w1, so that its `said` entries on /w2 stay pending and move
    val i1 = after(after(after(after(SEED, 1, 1, take_cmd(W1)), 1, 1, CSaid({ at: W2, kind: SAck, where_key: NoKey })), 1, 1,
                         CAskPerson({ at: W2, blocking: false, again: false })), 1, 1, CObject(W2))
    val i2 = after(after(after(i1, 1, 2, CSaid({ at: W2, kind: SAck, where_key: NoKey })), 1, 2, CConfirm(5)), 1, 1, CObject(W2))
    val i3 = after(i2, 1, 1, CResolve({ objection: 6, upheld: false, defer: false }))
    val items = { ...i3, nodes: i3.nodes.setBy(W2, nd => { ...nd, recheck_marks: Set(3), assume_marks: Set(3) }) }
    val mi = after(items, 2, 2, CMerge({ from: W2, into: W1 }))
    assert(and {
      replied.requests.get(1).answered_by == -1,              // the reply is in another conversation
      m.nodes.get(W1).keys == Set(Conv("c1"), Conv("c2")),
      m.nodes.get(W2).keys == Set(),
      m.requests.get(1).node == W1,
      m.requests.get(1).moved_from == W2,
      // the reply on /w1 is in c1 and the request in c2: no credit (§9.4, 1.3)
      m.requests.get(1).answered_by == -1,
      m.nodes.get(W2).life == Closed({ at: 2, outcome: Superseded, merged_into: W1, upheld: -1, from_done: false, by: 2 }),
      m.signals == Map(),
      closed_as(m, 12, Done),
      // the open items move with `moved_from`, keeping their conversation
      i3.saids.get(5).state == SdConfirmed,
      i3.commitments.keys() == Set(1),
      mi.commitments.get(1) == { ...i3.commitments.get(1), node: W1, moved_from: W2 },
      mi.commitments.get(1).conv == Conv("c2"),
      mi.saids.get(1) == { ...i3.saids.get(1), node: W1, moved_from: W2 },
      mi.saids.get(2) == { ...i3.saids.get(2), node: W1, moved_from: W2 },
      mi.pquestions.get(3) == { ...i3.pquestions.get(3), node: W1, moved_from: W2 },
      mi.objections.get(4) == { node: W1, state: ObOpen, moved_from: W2 },
      // what is finished stays on `from`: a confirmed `said`, a resolved objection
      mi.saids.get(5) == i3.saids.get(5),
      mi.objections.get(6) == i3.objections.get(6),
      mi.nodes.get(W1).recheck_marks == Set(3),
      mi.nodes.get(W1).assume_marks == Set(3),
      mi.nodes.get(W2).recheck_marks == Set(),
      mi.nodes.get(W2).assume_marks == Set(),
      out(SEED, 1, 1, CMerge({ from: W1, into: W1 })) == why(WhySelfMerge),
      out(SEED, 1, 1, CMerge({ from: ROOT, into: W1 })) == why(WhyReservedNode),
      out(SEED, 1, 1, CMerge({ from: W1, into: USER })) == why(WhyReservedNode),
      out(after(SEED, 1, 1, close_cmd(W1, Done)), 1, 1, CMerge({ from: W2, into: W1 })) == err(ErrClosed),
      out(after(SEED, 1, 1, close_cmd(W1, Done)), 1, 1, CMerge({ from: W1, into: W2 })) == why(WhyMergeFromClosed),
      out(SEED, 1, 1, CMerge({ from: OCC1, into: W1 })) == why(WhyOccurrenceNotYetAvailable),
      out(SEED, 1, 1, CMerge({ from: 99, into: W1 })) == nfo(WhatNode),
    })
  }

  /// Moving `c2`'s key from /w2 to /w1 lets a reply in `c2` land on /w1 (§9.1, case 1) while a
  /// request of `c2` stays on /w2.
  pure def move_c2(st: State): State =
    after(after(st, 1, 1, CKeyRemove({ node: W2, key: Conv("c2") })), 1, 1, CKeyAdd({ node: W1, key: Conv("c2") }))
  pure def move_c2_back(st: State): State =
    after(after(st, 1, 1, CKeyRemove({ node: W1, key: Conv("c2") })), 1, 1, CKeyAdd({ node: W2, key: Conv("c2") }))
  pure val REPLY_C2: Cmd = CSaid({ at: W1, kind: SReply, where_key: SomeKey(Conv("c2")) })
  pure val MERGE_W2: Cmd = CMerge({ from: W2, into: W1 })

  /// A merge credits a moved request from people or agents with the first reply already on `into`
  /// after it in the request's own conversation (A-95; §9.4, 1.3, D.8.20 step 3), and with nothing
  /// else: not a reply older than the request, and never a user request.
  run mergeCreditTest = {
    // a request in c2 on /w2 (1), then a reply in c2 on /w1 (2)
    val later = after(move_c2(after(SEED, 1, 1, CRequest({ at: W2, origin: Msg(("c2", 1)), where_key: SomeKey(Conv("c2")) }))),
                      1, 1, REPLY_C2)
    // a reply in c2 on /w1 (1), then a request in c2 on /w2 (2)
    val earlier = after(move_c2_back(after(move_c2(SEED), 1, 1, REPLY_C2)), 1, 1,
                        CRequest({ at: W2, origin: Msg(("c2", 1)), where_key: SomeKey(Conv("c2")) }))
    // a user request in c2 on /w2 (entry 1, request 2), then a reply in c2 on /w1 (3)
    val user = after(move_c2(after(after(SEED, 1, 1, CFromUser({ principal: "user", answers: -1, clarifies: -1, node: -1 })), 1, 1,
                                   CRequest({ at: W2, origin: User(1), where_key: SomeKey(Conv("c2")) }))),
                     1, 1, REPLY_C2)
    assert(and {
      later.requests.get(1).node == W2,
      // 1.11 (§9.2): session 1 holds nothing, so its reply is confirmed as it is recorded
      later.saids.get(2) == { node: W1, conv: Conv("c2"), kind: SReply, session: 1, state: SdConfirmed, moved_from: -1 },
      later.requests.get(1).answered_by == -1,
      after(later, 2, 2, MERGE_W2).requests.get(1).answered_by == 2,
      earlier.saids.get(1).node == W1,
      earlier.requests.get(2).node == W2,
      after(earlier, 2, 2, MERGE_W2).requests.get(2).answered_by == -1,
      user.requests.get(2).requester == ReqUser(USER_PRINCIPAL),
      user.requests.get(2).conv == Conv("c2"),
      after(user, 2, 2, MERGE_W2).requests.get(2).node == W1,
      after(user, 2, 2, MERGE_W2).requests.get(2).answered_by == -1,
    })
  }

  /// Merge: a cycle in the redirected edges is refused (N5, T5); waits and dependencies on `from`
  /// move to `into`, and a self-edge is dropped; the marks move; an intent of another session moves.
  run mergeEdgesTest = {
    val a1 = after(SEED, 1, 1, create_cmd(W1, "a1", Set(), false, 1))
    val e1 = after(after(a1, 1, 1, CUpdate({ ...update_none(W1, 1), depends: IdsSet(Set(12)) })), 1, 1,
                   CUpdate({ ...update_none(12, 1), depends: IdsSet(Set(W2)) }))
    val e2 = after(after(a1, 1, 1, CUpdate({ ...update_none(12, 1), depends: IdsSet(Set(W2)) })), 1, 1,
                   CUpdate({ ...update_none(W1, 1), depends: IdsSet(Set(W2)) }))
    val m2 = after(e2, 2, 1, CMerge({ from: W2, into: W1 }))
    val marked = { ...SEED, nodes: SEED.nodes.setBy(W2, nd => { ...nd, assume_marks: Set(5), open_intent: { eid: 4, session: 2 } }) }
    val m3 = after(marked, 2, 1, CMerge({ from: W2, into: W1 }))
    assert(and {
      out(e1, 2, 1, CMerge({ from: W2, into: W1 })) == err(ErrCycle),
      m2.nodes.get(12).depends_on == Set(W1),
      m2.nodes.get(W1).depends_on == Set(),
      m3.nodes.get(W1).assume_marks == Set(5),
      m3.nodes.get(W2).assume_marks == Set(),
      m3.nodes.get(W1).open_intent == { eid: 4, session: 2 },
      m3.nodes.get(W2).open_intent == NO_INTENT,
      // the merger's own intent stays on `from`, and its close ends it (§5)
      after({ ...SEED, nodes: SEED.nodes.setBy(W2, nd => { ...nd, open_intent: { eid: 4, session: 1 } }) }, 2, 1,
            CMerge({ from: W2, into: W1 })).nodes.get(W1).open_intent == NO_INTENT,
    })
  }

  /// A merge of a series into its own open occurrence, waiting or available, is
  /// `invalid(MergeIntoOwnOccurrence)` and changes nothing (1.5, D.8.20, N5): closing the series
  /// would drop the occurrence with what the merge moved there (README, "Open questions", item 20).
  /// It comes before `cycle`. The series still merges into another node, and another node into the
  /// occurrence.
  run mergeIntoOwnOccurrenceTest = {
    val avail = settle(live_at(SEED, 14), 17, MAX_FIRINGS).state                 // #11 available at 15
    val marked = { ...avail, nodes: avail.nodes.setBy(SER, nd => { ...nd, assume_marks: Set(5) }) }
    val own = CMerge({ from: SER, into: OCC1 })
    // OCC1 → W1 → SER: with SER replaced by OCC1, a cycle
    val cyclic = { ...marked, nodes: marked.nodes.setBy(OCC1, nd => { ...nd, depends_on: Set(W1) })
                                                 .setBy(W1, nd => { ...nd, depends_on: Set(SER) }) }
    val other = call(marked, 17, 1, CMerge({ from: SER, into: W1 }))
    val into_occ = call(marked, 17, 1, CMerge({ from: W2, into: OCC1 }))
    assert(and {
      occ_waiting(SEED.nodes.get(OCC1)),
      call(SEED, 1, 1, own) == { state: SEED, outcome: why(WhyMergeIntoOwnOccurrence) },
      occ_available(marked.nodes.get(OCC1)),
      open_occurrences(marked, SER) == Set(OCC1),
      call(marked, 17, 1, own) == { state: marked, outcome: why(WhyMergeIntoOwnOccurrence) },
      merge_cycle(cyclic, SER, OCC1),
      call(cyclic, 17, 1, own) == { state: cyclic, outcome: why(WhyMergeIntoOwnOccurrence) },
      // into another node: the marks move, the series closes, and so does its occurrence (D.8.18)
      other.outcome == OutOk([]),
      other.state.nodes.get(SER).life == Closed({ at: 17, outcome: Superseded, merged_into: W1, upheld: -1, from_done: false, by: 1 }),
      other.state.nodes.get(W1).assume_marks == Set(5),
      other.state.nodes.get(SER).assume_marks == Set(),
      closed_as(other.state, OCC1, Dropped),
      // another node into the occurrence
      into_occ.outcome == OutOk([]),
      into_occ.state.nodes.get(W2).life == Closed({ at: 17, outcome: Superseded, merged_into: OCC1, upheld: -1, from_done: false, by: 1 }),
      into_occ.state.nodes.get(OCC1).keys.contains(Conv("c2")),
      is_open(into_occ.state, OCC1),
    })
  }

  /// `node hide {entry}` (D.8.20): the entry must exist; a system entry on its node; O3 (nothing
  /// else changes).
  run hideTest = {
    val obj = after(SEED, 1, 1, CObject(W1))
    val r = call(obj, 2, 1, CHide(1))
    assert(and {
      r.outcome == OutOk([]),
      r.state.nodes.get(W1).seq == 2,
      r.state.next_eid == 3,
      r.state.entries == obj.entries,
      r.state.objections == obj.objections,
      out(obj, 2, 1, CHide(2)) == nfo(WhatEntry),
      long_rejected(obj, 2, 1, CHide(1)),
    })
  }


  // ================================================================== hold (D.8.5, D.8.10)

  /// `hold take` (D.8.10): the first lease is always `lease.first`; `lease` is L, clamped to
  /// [1, lease.max] (A-8); an existing hold is returned; the refusals in their order.
  run takeTest = {
    val t = call(SEED, 1, 1, CTake({ node: W1, lease: 8 }))
    val two = after(after(SEED, 1, 1, take_cmd(W1)), 1, 1, take_cmd(W2))
    assert(and {
      t.outcome == OutOk([W1, 1]),
      t.state.holds.get((W1, 1)).lease_until == 1 + LEASE_FIRST,
      t.state.holds.get((W1, 1)).l == LEASE_MAX,
      after(SEED, 1, 1, CTake({ node: W1, lease: 1 })).holds.get((W1, 1)).l == 1,
      after(SEED, 1, 1, take_cmd(W1)).holds.get((W1, 1)).l == LEASE_DEFAULT,
      // the session's existing hold: the same token, nothing new
      call(t.state, 1, 1, take_cmd(W1)) == { state: t.state, outcome: OutOk([W1, 1]) },
      out(t.state, 1, 2, take_cmd(W1)) == err(ErrFull),
      out(SEED, 1, 1, take_cmd(ROOT)) == err(ErrFull),
      out(two, 1, 1, take_cmd(SER)) == err(ErrHoldsLimit),
      out(after(SEED, 1, 2, CPause(W1)), 1, 1, take_cmd(W1)) == err(ErrPaused),
      out(after(SEED, 1, 2, CPause(ROOT)), 1, 1, take_cmd(W1)) == err(ErrPaused),
      out(SEED, 1, 1, take_cmd(OCC1)) == why(WhyOccurrenceNotYetAvailable),
      out(SEED, 1, 1, take_cmd(99)) == nfo(WhatNode),
      // a closed node without flags, or a merged one, is closed (§21, A-94, A-96)
      out(after(SEED, 1, 1, close_cmd(W1, Done)), 1, 1, take_cmd(W1)) == err(ErrClosed),
      out(after(SEED, 1, 1, CMerge({ from: W2, into: W1 })), 1, 1, take_cmd(W2)) == err(ErrClosed),
    })
  }

  /// Taking a closed node with a flag reopens it (§10.1); a closed occurrence of an open series is
  /// refused (A-13).
  run takeClosedTest = {
    val req = after(SEED, 1, 1, CRequest({ at: W1, origin: Msg(("c1", 1)), where_key: SomeKey(Conv("c1")) }))
    val closed = after(req, 1, 1, close_cmd(W1, Done))
    val r = call(closed, 2, 2, take_cmd(W1))
    val occ_req = after(SEED, 1, 1, CRequest({ at: OCC1, origin: Conv("c9"), where_key: NoKey }))
    val occ_closed = after(occ_req, 1, 1, close_cmd(OCC1, Dropped))
    assert(and {
      flags_of(closed, W1, 2) == Set(FUnansweredRequest),
      r.outcome == OutOk([W1, 1]),
      is_open(r.state, W1),
      r.state.holds.keys() == Set((W1, 1)),
      r.state.nodes.get(W1).version == 3,
      // the reopen's entry raises the node's seq, and the brief marks it shown
      r.state.sessions.get(2).shown.get(W1) == r.state.nodes.get(W1).seq,
      occ_req.requests.get(1).node == OCC1,
      flags_of(occ_closed, OCC1, 2) == Set(FUnansweredRequest),
      out(occ_closed, 2, 1, take_cmd(OCC1)) == why(WhyOccurrenceReopen),
    })
  }

  /// `hold next` (D.8.5): the open `next` records the caller's caps as a profile of its class (CS-7);
  /// the best candidate by `(tier, h, id)`: with the tie-break by id, /w1 first; a routing item and a
  /// closed flagged node are tier 0; a status item for a waiting node with an open commitment.
  run nextTest = {
    val n1 = call(SEED, 1, 1, CNext)
    val n2 = call(n1.state, 1, 2, CNext)
    val entry = after(SEED, 1, 1, CFromUser({ principal: "user", answers: -1, clarifies: -1, node: -1 }))
    assert(and {
      n1.outcome == OutOk([0, W1, 1]),
      n1.state.holds.keys() == Set((W1, 1)),
      n1.state.classes == Map("a" -> { caps_seen: Map(Set() -> 1 + REFUSAL_WINDOW) }),
      n2.outcome == OutOk([0, W2, 1]),
      n2.state.classes.get("b") == { caps_seen: Map(Set() -> 1 + REFUSAL_WINDOW) },
      // nothing left for session 1: /w1 is its own, /w2 full, /s a container, #11 not yet available
      call(n2.state, 1, 2, CNext).outcome == OutOk([]),
      call(n2.state, 1, 2, CNext).state.classes == n2.state.classes,
      out(entry, 1, 2, CNext) == OutOk([2, 1]),
      // an unsettled request on an unattended node is tier 0, before /w1's offer (tier 2)
      out(after(SEED, 1, 1, CRequest({ at: W2, origin: Conv("c2"), where_key: NoKey })), 1, 1, CNext) == OutOk([0, W2, 1]),
      out(after(after(SEED, 1, 1, CRequest({ at: W2, origin: Conv("c2"), where_key: NoKey })), 1, 1, close_cmd(W2, Done)), 1, 1, CNext)
        == OutOk([0, W2, 1]),
      out(n1.state, 1, 1, CNext) == OutOk([0, W2, 1]),
      out(after(n1.state, 1, 1, take_cmd(W2)), 1, 1, CNext) == err(ErrHoldsLimit),
      long_rejected(SEED, 1, 1, CNext),
    })
  }

  /// `next`'s exclusions (D.8.5): a class's refusal (backoff or quorum), needs beyond the caps, the
  /// caller's own hold (A-115), a pause, an open dependency.
  run nextExclusionsTest = {
    val backoff = { ...SEED, refusals: Map((W1, "a") -> { quorum: 2, by: Map(2 -> { backoff_until: 6, expires_at: 13 }) }) }
    val needy = after(SEED, 1, 1, CUpdate({ ...update_none(W1, 1), needs_add: Set("x") }))
    val cap2 = after(after(SEED, 1, 1, CUpdate({ ...update_none(W1, 1), capacity: 2 })), 1, 1, take_cmd(W1))
    assert(and {
      out(backoff, 1, 1, CNext) == OutOk([0, W2, 1]),
      out(backoff, 1, 2, CNext) == OutOk([0, W1, 1]),
      out(needy, 1, 1, CNext) == OutOk([0, W2, 1]),
      out({ ...needy, sessions: needy.sessions.setBy(1, se => { ...se, caps: Set("x") }) }, 1, 1, CNext) == OutOk([0, W1, 1]),
      out(cap2, 1, 1, CNext) == OutOk([0, W2, 1]),
      out(cap2, 1, 2, CNext) == OutOk([0, W1, 2]),
      out(after(SEED, 1, 2, CPause(W1)), 1, 1, CNext) == OutOk([0, W2, 1]),
      out(after(SEED, 1, 1, CUpdate({ ...update_none(W1, 1), depends: IdsSet(Set(W2)) })), 1, 1, CNext) == OutOk([0, W2, 1]),
    })
  }

  /// A status item (§10.4): a waiting node whose open commitment's follow-up has passed. The FNV
  /// tie-break orders a tier by the ring distance from the session (D.2.6, D.10.6).
  run nextStatusTest = {
    val acked = after(SEED, 1, 1, CSaid({ at: W1, kind: SAck, where_key: NoKey }))
    val wt = after(after(acked, 1, 1, take_cmd(W1)), 2, 1, CRelease({ token: (W1, 1), reason: Blocked, wait: WaTime(30), settles: Set() }))
    val fnv = { use_fnv: true, quorum: 2 }
    val first = if (u64_lt(tie_hash(1, W1), tie_hash(1, W2))) W1 else W2
    assert(and {
      waiting(wt, W1, 8),
      flags_of(wt, W1, 8).contains(FOpenCommitment),
      out(wt, 8, 2, CNext) == OutOk([1, W1]),
      not(flags_of(wt, W1, 6).contains(FOpenCommitment)),
      out(wt, 6, 2, CNext) == OutOk([0, W2, 1]),
      best_candidate(fnv, SEED, 1, "a", 1).id == first,
      run_cmd(fnv, SEED, 1, "a", 1, "", false, CNext).outcome == OutOk([0, first, 1]),
    })
  }

  /// `hold checkpoint` (D.8.10): the checkpoint (attempts reset, A-9), the intent rules (A-67), and
  /// the extension up to the ceiling once checkpointed.
  run checkpointTest = {
    val held = after(SEED, 1, 1, take_cmd(W1))
    val c = call(held, 2, 1, CCheckpoint({ token: (W1, 1), intent: true, intent_done: false }))
    val st = c.state
    val again = after(st, 3, 1, CCheckpoint({ token: (W1, 1), intent: true, intent_done: true }))
    assert(and {
      c.outcome == OutOk([2 + LEASE_DEFAULT]),
      st.nodes.get(W1).checkpoints == 1,
      st.nodes.get(W1).last_checkpoint == 1,
      st.nodes.get(W1).open_intent == { eid: 1, session: 1 },
      st.holds.get((W1, 1)).checkpointed,
      st.holds.get((W1, 1)).lease_until == 2 + LEASE_DEFAULT,
      st.entries == Map(),
      st.next_eid == 2,
      out(st, 3, 1, CCheckpoint({ token: (W1, 1), intent: true, intent_done: false })) == why(WhyIntentOpen),
      out(held, 2, 1, CCheckpoint({ token: (W1, 1), intent: false, intent_done: true })) == why(WhyNoIntent),
      again.nodes.get(W1).open_intent == { eid: 2, session: 1 },
      after(st, 3, 1, CCheckpoint({ token: (W1, 1), intent: false, intent_done: true })).nodes.get(W1).open_intent == NO_INTENT,
      // the ceiling caps a checkpointed hold: taken at 1, ceiling 9 (5 + L = 9, then 8 + L > 10)
      after(after(st, 5, 1, CCheckpoint({ token: (W1, 1), intent: false, intent_done: false })), 8, 1,
            CCheckpoint({ token: (W1, 1), intent: false, intent_done: false })).holds.get((W1, 1)).lease_until == 1 + LEASE_CEILING,
      // a hold whose lease has ended is not live: its token is stale (H4)
      out(st, 7, 1, CCheckpoint({ token: (W1, 1), intent: false, intent_done: false })) == err(ErrStaleToken),
      // attempts reset by a checkpoint
      after({ ...held, nodes: held.nodes.setBy(W1, nd => { ...nd, attempts: 2 }) }, 2, 1,
            CCheckpoint({ token: (W1, 1), intent: false, intent_done: false })).nodes.get(W1).attempts == 0,
      long_rejected(held, 2, 1, CCheckpoint({ token: (W1, 1), intent: false, intent_done: false })),
    })
  }

  /// `hold renew` (D.8.10, A-7): before a checkpoint the lease stays at `taken_at + lease.first`;
  /// `renew.max` renewals; a checkpoint resets the count.
  run renewTest = {
    val held = after(SEED, 1, 1, take_cmd(W1))
    val r1 = call(held, 2, 1, CRenew((W1, 1)))
    val r2 = after(r1.state, 2, 1, CRenew((W1, 1)))
    val ck = after(r2, 3, 1, CCheckpoint({ token: (W1, 1), intent: false, intent_done: false }))
    assert(and {
      r1.outcome == OutOk([1 + LEASE_FIRST, 1]),
      r1.state.holds.get((W1, 1)).renews == 1,
      r1.state.nodes.get(W1).seq == 1,
      out(r2, 2, 1, CRenew((W1, 1))) == err(ErrLimit(LkRenew)),
      ck.holds.get((W1, 1)).renews == 0,
      out(ck, 4, 1, CRenew((W1, 1))) == OutOk([4 + LEASE_DEFAULT, 1]),
      long_rejected(held, 2, 1, CRenew((W1, 1))),
    })
  }

  /// `hold release` (D.8.10): a checkpoint entry, the hold ends with `offer`, the recheck marks
  /// leave, the session's pending `said` entries are confirmed; the waits and their rechecks.
  run releaseTest = {
    val acked = after(after(SEED, 1, 1, CSaid({ at: W1, kind: SAck, where_key: NoKey })), 1, 1, take_cmd(W1))
    val marked = { ...acked, nodes: acked.nodes.setBy(W1, nd => { ...nd, recheck_marks: Set(7) }) }
    val r = call(marked, 2, 1, CRelease({ token: (W1, 1), reason: Handoff, wait: WaNone, settles: Set() }))
    val asked = after(after(SEED, 1, 1, CAskPerson({ at: W1, blocking: false, again: false })), 1, 1, take_cmd(W1))
    val wq = after(asked, 3, 1, CRelease({ token: (W1, 1), reason: Handoff, wait: WaQuestion(2), settles: Set() }))
    val held = after(SEED, 1, 1, take_cmd(W1))
    assert(and {
      r.outcome == OutOk([]),
      r.state.holds == Map(),
      r.state.nodes.get(W1).tags == Set(OFFER),
      r.state.nodes.get(W1).checkpoints == 1,
      r.state.nodes.get(W1).recheck_marks == Set(),
      r.state.nodes.get(W1).wait == NoWait,
      r.state.saids.get(1).state == SdConfirmed,
      // a wait on a people question: the recheck moves to now + question.recheck (A-24)
      wq.nodes.get(W1).wait == HasWait({ on: WQuestion({ q: 2, rechecks_at_set: 0 }), set_at: 3, fired: false }),
      wq.pquestions.get(2).recheck_at == 3 + QUESTION_RECHECK,
      // a time at or before now resolves at once (A-68)
      after(held, 2, 1, CRelease({ token: (W1, 1), reason: Handoff, wait: WaTime(2), settles: Set() })).nodes.get(W1).wait
        == HasWait({ on: WTime(2), set_at: 2, fired: true }),
      after(held, 2, 1, CRelease({ token: (W1, 1), reason: Handoff, wait: WaExternal(5), settles: Set() })).nodes.get(W1).wait
        == HasWait({ on: WExternal(5), set_at: 2, fired: false }),
      after(held, 2, 1, CRelease({ token: (W1, 1), reason: Handoff, wait: WaNode(W2), settles: Set() })).nodes.get(W1).wait
        == HasWait({ on: WNode(W2), set_at: 2, fired: false }),
      out(held, 2, 1, CRelease({ token: (W1, 1), reason: Handoff, wait: WaNode(W1), settles: Set() })) == err(ErrCycle),
      out(held, 2, 1, CRelease({ token: (W1, 1), reason: Handoff, wait: WaQuestion(5), settles: Set() })) == nfo(WhatQuestion),
      out(held, 2, 1, CRelease({ token: (W1, 1), reason: Handoff, wait: WaNode(99), settles: Set() })) == nfo(WhatNode),
      long_rejected(held, 2, 1, CRelease({ token: (W1, 1), reason: Handoff, wait: WaNone, settles: Set() })),
    })
  }

  /// A wait that closes a cycle (T5): /w2 waits on /w1, so /w1 may not wait on /w2.
  run releaseCycleTest = {
    val w2_waits = after(after(SEED, 1, 2, take_cmd(W2)), 2, 2, CRelease({ token: (W2, 1), reason: Blocked, wait: WaNode(W1), settles: Set() }))
    assert(out(after(w2_waits, 2, 1, take_cmd(W1)), 2, 1, CRelease({ token: (W1, 1), reason: Blocked, wait: WaNode(W2), settles: Set() })) == err(ErrCycle))
  }

  /// `release {reason: cannot}` (CS-7, E1): one session's refusal backs the class off; the class
  /// has refused at its quorum, and with no other active class the user is asked, with a default;
  /// `cannot` with a wait records `blocked` and no refusal (A-98).
  run releaseCannotTest = {
    val three = run_cmd(CFG, SEED, 1, "a", -1, "", false, CHello({ caps: Set(), principal: "" })).state
    val c1 = after(after(three, 1, 1, take_cmd(W1)), 2, 1, CRelease({ token: (W1, 1), reason: Cannot, wait: WaNone, settles: Set() }))
    val c2 = after(after(c1, 2, 3, take_cmd(W1)), 3, 3, CRelease({ token: (W1, 2), reason: Cannot, wait: WaNone, settles: Set() }))
    val blocked = after(after(three, 1, 1, take_cmd(W1)), 2, 1, CRelease({ token: (W1, 1), reason: Cannot, wait: WaTime(9), settles: Set() }))
    assert(and {
      c1.refusals == Map((W1, "a") -> { quorum: 2, by: Map(1 -> { backoff_until: 2 + REFUSAL_BACKOFF, expires_at: 2 + REFUSAL_TTL }) }),
      backed_off(c1, W1, "a", 2),
      not(class_refused(c1, W1, "a", 2)),
      c1.uquestions == Map(),
      class_refused(c2, W1, "a", 3),
      all_refused(c2, W1, 3),
      // eids 1 and 2 are the releases' checkpoint entries
      c2.uquestions.keys() == Set(3),
      c2.uquestions.get(3).source == SrcRefusal(W1),
      c2.uquestions.get(3).to == [USER_PRINCIPAL],
      c2.uquestions.get(3).has_default,
      // a refusal question keeps the long due, even for an absent addressee (D.8.12)
      c2.uquestions.get(3).due == 3 + USER_QUESTION_DUE,
      blocked.refusals == Map(),
    })
  }


  // ================================================================== talk: the user (D.8.11, D.8.12)

  pure def from_user(principal: str, answers: int, clarifies: int, n: int): Cmd =
    CFromUser({ principal: principal, answers: answers, clarifies: clarifies, node: n })

  /// `talk from_user` (D.8.11): the entry on USER with its principal (CS-1), the message time, and
  /// the session then serves the principal; routing to a node, or to the session's only hold.
  run fromUserTest = {
    val r = call(SEED, 1, 1, from_user("p1", -1, -1, -1))
    val st = r.state
    val routed = after(SEED, 1, 1, from_user("user", -1, -1, W1))
    val held = after(after(SEED, 1, 1, take_cmd(W2)), 2, 1, from_user("user", -1, -1, -1))
    assert(and {
      r.outcome == OutOk([1, 1]),
      st.entries.get(1) == { node: USER, seq: 1, at: 1, by: 1, ty: EFromUser, principal: 1 },
      st.user_entries.get(1) == { session: 1, principal: 1, at: 1, routing: Unrouted },
      st.user_msgs == Map(1 -> [1]),
      st.sessions.get(1).principals == Set(1),
      // a session that serves one principal speaks for it; one that serves none names one, `user`
      // among them (1.9: none is NoPrincipal)
      after(st, 2, 1, from_user("", -1, -1, -1)).user_entries.get(2).principal == 1,
      out(SEED, 1, 1, from_user("", -1, -1, -1)) == why(WhyNoPrincipal),
      out(SEED, 1, 1, from_user("", -1, -1, W1)) == why(WhyNoPrincipal),
      routed.user_entries.get(1).principal == USER_PRINCIPAL,
      routed.nodes.get(W1).keys == Set(Conv("c1"), User(1)),
      routed.requests.get(2).requester == ReqUser(USER_PRINCIPAL),
      routed.user_entries.get(1).routing == RtRequest({ node: W1, request: 2 }),
      held.user_entries.get(1).routing == RtRequest({ node: W2, request: 2 }),
      // two principals of different persons: ambiguous (CS-1)
      out(after(st, 2, 1, from_user("p2", -1, -1, -1)), 3, 1, from_user("", -1, -1, -1)) == why(WhyAmbiguousPrincipal),
      out(SEED, 1, 1, from_user("", -1, -1, USER)) == why(WhyReservedNode),
      out(SEED, 1, 1, from_user("", -1, -1, 99)) == nfo(WhatNode),
      out(SEED, 1, 1, from_user("", 4, -1, -1)) == nfo(WhatQuestion),
      out(SEED, 1, 1, from_user("", -1, 4, -1)) == nfo(WhatRequest),
      long_rejected(SEED, 1, 1, from_user("", -1, -1, -1)),
    })
  }

  /// 1.11 (§8.2): a `from_user` with no target, from a session whose only hold carries an
  /// unsettled request of the entry's person, clarifies the newest such request; with none there,
  /// it is a new request; another person's words are a new request too.
  run followUpTest = {
    val held = after(SEED, 1, 1, take_cmd(W2))
    val first = after(held, 2, 1, from_user("user", -1, -1, -1))
    val second = after(first, 3, 1, from_user("user", -1, -1, -1))
    val other = after(second, 3, 1, from_user("p1", -1, -1, -1))
    assert(and {
      first.user_entries.get(1).routing == RtRequest({ node: W2, request: 2 }),
      second.user_entries.get(3).routing == RtClarifies(2),
      other.user_entries.get(4).routing == RtRequest({ node: W2, request: 5 }),
      // the explicit node is new work
      after(first, 3, 1, from_user("user", -1, -1, W2)).user_entries.get(3).routing == RtRequest({ node: W2, request: 4 }),
    })
  }

  /// `from_user {answers}` (D.8.11, CS-2, A-37, A-56, A-60): a pending question is answered by the
  /// entry; an escalated people question is answered with it; a refusal question clears the
  /// refusals; a question no longer pending leaves the entry unrouted. Precedence answers >
  /// clarifies > node (A-99).
  run fromUserAnswersTest = {
    val asked = after(SEED, 1, 1, CAskUser({ at: W1, has_default: false, to: "user", again: false, digest: 1 }))
    val ans = after(asked, 2, 1, from_user("user", 1, -1, W2))
    val req = after(SEED, 1, 1, CRequest({ at: W1, origin: Msg(("c1", 1)), where_key: SomeKey(Conv("c1")) }))
    val clar = after(req, 2, 1, from_user("user", -1, 1, W2))
    val esc = {
      ...SEED,
      next_eid: 4,
      pquestions: Map(2 -> { node: W1, conv: Conv("c1"), blocking: true, said: 1, state: PqEscalated(3), rechecks: 2,
                             recheck_at: 16, lapse: LapseEscalated, assumed_by: -1, moved_from: -1 }),
      uquestions: Map(3 -> { source: SrcEscalation(2), about: W1, to: [0], stage: 0, due: 30, has_default: false,
                             state: UqPending, assumed_by: -1 }),
      entries: Map(2 -> { node: W1, seq: 1, at: 0, by: 1, ty: EQuestion, principal: -1 },
                   3 -> { node: USER_DECISIONS, seq: 1, at: 0, by: 0, ty: EQuestion, principal: -1 }),
    }
    val esc_ans = after(esc, 2, 1, from_user("user", 3, -1, -1))
    val refq = {
      ...SEED,
      next_eid: 2,
      refusals: Map((W1, "a") -> { quorum: 1, by: Map(1 -> { backoff_until: 6, expires_at: 13 }) }),
      uquestions: Map(1 -> { source: SrcRefusal(W1), about: W1, to: [0], stage: 0, due: 30, has_default: true,
                             state: UqPending, assumed_by: -1 }),
      entries: Map(1 -> { node: USER_DECISIONS, seq: 1, at: 0, by: 1, ty: EQuestion, principal: -1 }),
    }
    assert(and {
      ans.uquestions.get(1).state == UqAnswered(2),
      ans.user_entries.get(2).routing == RtAnswers(1),
      ans.requests == Map(),
      // answered again: the second entry stays unrouted (A-37)
      after(ans, 3, 1, from_user("", 1, -1, W2)).user_entries.get(3).routing == Unrouted,
      clar.user_entries.get(2).routing == RtClarifies(1),
      esc_ans.uquestions.get(3).state == UqAnswered(4),
      esc_ans.pquestions.get(2).state == PqAnswered(4),
      after(refq, 2, 1, from_user("user", 1, -1, -1)).refusals == Map(),
      // a people question is not a user question
      out(esc, 2, 1, from_user("", 2, -1, -1)) == why(WhyNotUserQuestion),
    })
  }

  /// `talk to_user` (D.8.11, CS-4, CS-5, A-23): only a `result` to the requester (or an alias)
  /// answers a user request; a digest entry is shown only to its own person.
  run toUserTest = {
    val req = after(SEED, 1, 1, from_user("p1", -1, -1, W1))                 // entry 1, request 2, requester p1
    val dec = after(req, 1, 1, CDecide({ at: W1, settles: Set(2), supersedes: -1, basis: [] }))   // decision 3
    val res = call(dec, 2, 2, CToUser({ principal: "p1", kind: TuResult, re: Set(2) }))
    val via_entry = after(dec, 2, 2, CToUser({ principal: "p1", kind: TuResult, re: Set(1) }))
    val alias = after(dec, 1, 1, create_cmd(PEOPLE, "a1", Set(Person("p1"), Person("p2")), false, 1))
    val dg = { ...SEED, next_eid: 2, principal_ids: Map("user" -> 0, "p1" -> 1, "p2" -> 2), next_principal: 3,
               digest: Map(1 -> { item: DiLapsed(9), to: 1, state: DgPending }) }
    assert(and {
      res.outcome == OutOk([4, 1]),
      res.state.entries.get(4) == { node: USER, seq: 2, at: 2, by: 2, ty: EToUser, principal: 1 },
      res.state.requests.get(2).answered_by == 4,
      res.state.requests.get(2).settled == SettledBy({ decision: 3, answer: 4 }),
      via_entry.requests.get(2).settled == SettledBy({ decision: 3, answer: 4 }),
      after(dec, 2, 2, CToUser({ principal: "p1", kind: TuAck, re: Set(2) })).requests.get(2).answered_by == -1,
      after(dec, 2, 2, CToUser({ principal: "p1", kind: TuQuestion, re: Set(2) })).requests.get(2).answered_by == -1,
      after(dec, 2, 2, CToUser({ principal: "p2", kind: TuResult, re: Set(2) })).requests.get(2).answered_by == -1,
      // p2 is an alias of p1's person now: a report to p2 answers p1's request (D-3)
      after(alias, 2, 2, CToUser({ principal: "p2", kind: TuResult, re: Set(2) })).requests.get(2).answered_by == 4,
      // a session that serves no principal reports to `user` without one
      call(dec, 2, 2, CToUser({ principal: "", kind: TuAck, re: Set() })).outcome == OutOk([4, 0]),
      after(dg, 1, 1, CToUser({ principal: "p1", kind: TuAck, re: Set(1) })).digest.get(1).state == DgShown(2),
      after(dg, 1, 1, CToUser({ principal: "p2", kind: TuAck, re: Set(1) })).digest.get(1).state == DgPending,
      out(dec, 2, 2, CToUser({ principal: "p1", kind: TuResult, re: Set(9) })) == nfo(WhatEntry),
      out(dec, 2, 2, CToUser({ principal: "p1", kind: TuResult, re: Set(3) })) == why(WhyNotReportable),
      long_rejected(dec, 2, 2, CToUser({ principal: "p1", kind: TuResult, re: Set(2) })),
    })
  }

  /// `talk route` (D.8.11, §8.2): the first routing wins.
  run routeTest = {
    val e = after(SEED, 1, 1, from_user("user", -1, -1, -1))
    val r = call(e, 2, 2, CRoute({ entry: 1, to: W1 }))
    assert(and {
      r.outcome == OutOk([2]),
      r.state.user_entries.get(1).routing == RtRequest({ node: W1, request: 2 }),
      call(r.state, 2, 2, CRoute({ entry: 1, to: W2 })) == { state: r.state, outcome: OutOk([-1]) },
      after(e, 2, 2, CRoute({ entry: 1, to: -1 })).user_entries.get(1).routing == RtNoAction,
      out(e, 2, 2, CRoute({ entry: 1, to: USER })) == why(WhyReservedNode),
      out(e, 2, 2, CRoute({ entry: 1, to: 99 })) == nfo(WhatNode),
      out(e, 2, 2, CRoute({ entry: 2, to: W1 })) == nfo(WhatUserEntry),
      out(after(e, 2, 2, CObject(W1)), 2, 2, CRoute({ entry: 2, to: W1 })) == why(WhyNotUserEntry),
      long_rejected(e, 2, 2, CRoute({ entry: 1, to: W1 })),
    })
  }

  /// `talk ask {to: user | principal}` (D.8.12, CS-2): the chain fixed now; asking once unless
  /// `again`; an explicit principal is interned and comes first.
  run askUserTest = {
    val r = call(SEED, 1, 1, CAskUser({ at: W1, has_default: true, to: "user", again: false, digest: 1 }))
    val st = r.state
    val p1 = after(SEED, 1, 1, CAskUser({ at: W1, has_default: false, to: "p1", again: false, digest: 1 }))
    assert(and {
      r.outcome == OutOk([1]),
      st.uquestions.get(1) == {
        source: SrcAgent({ session: 1, digest: 1 }), about: W1, to: [USER_PRINCIPAL], stage: 0,
        due: 1 + USER_QUESTION_DUE_ABSENT, has_default: true, state: UqPending, assumed_by: -1,
      },
      st.entries.get(1).node == USER_DECISIONS,
      // the same question for the same first addressee: returned, nothing appended
      call(st, 1, 1, CAskUser({ at: W1, has_default: true, to: "user", again: false, digest: 1 })) == { state: st, outcome: OutOk([1]) },
      out(st, 1, 2, CAskUser({ at: W1, has_default: true, to: "user", again: true, digest: 1 })) == OutOk([2]),
      out(st, 1, 2, CAskUser({ at: W1, has_default: true, to: "user", again: false, digest: 2 })) == OutOk([2]),
      out(st, 1, 2, CAskUser({ at: W2, has_default: true, to: "user", again: false, digest: 1 })) == OutOk([2]),
      p1.principal_ids.get("p1") == 1,
      p1.uquestions.get(1).to == [1, USER_PRINCIPAL],
      out(SEED, 1, 1, CAskUser({ at: 99, has_default: true, to: "user", again: false, digest: 1 })) == nfo(WhatNode),
      long_rejected(SEED, 1, 1, CAskUser({ at: W1, has_default: true, to: "user", again: false, digest: 1 })),
    })
  }

  // ================================================================== talk: people (D.8.15)

  /// `talk request` (D.8.15, Q1, A-61): one request per message; `where` names the conversation,
  /// else the origin; case 3 adds the key; a reserved conversation node is refused (C1, A-29).
  run requestTest = {
    val r = call(SEED, 1, 1, CRequest({ at: W1, origin: Msg(("c1", 1)), where_key: SomeKey(Conv("c1")) }))
    val st = r.state
    val second = after(st, 1, 1, CRequest({ at: W1, origin: Msg(("c1", 2)), where_key: SomeKey(Conv("c1")) }))
    val case2 = after(SEED, 1, 1, CRequest({ at: W1, origin: Conv("c2"), where_key: NoKey }))
    val case3 = after(SEED, 1, 1, CRequest({ at: W2, origin: Msg(("c1", 1)), where_key: NoKey }))
    assert(and {
      r.outcome == OutOk([1, W1]),
      st.requests.get(1) == { node: W1, origin: Msg(("c1", 1)), conv: Conv("c1"), from_user: -1, requester: ReqExternal,
                              answered_by: -1, settled: Unsettled, moved_from: -1 },
      st.entries.get(1) == { node: W1, seq: 1, at: 1, by: 1, ty: ERequest, principal: -1 },
      // a repeat of the message is the same request, from any `at`
      call(st, 1, 1, CRequest({ at: W2, origin: Msg(("c1", 1)), where_key: NoKey })) == { state: st, outcome: OutOk([1, W1]) },
      second.requests.keys() == Set(1, 2),
      case2.requests.get(1).node == W2,
      case3.requests.get(1).node == W2,
      case3.nodes.get(W2).keys == Set(Conv("c2"), Msg(("c1", 1))),
      out(SEED, 1, 1, CRequest({ at: ROOT, origin: Conv("c9"), where_key: NoKey })) == why(WhyReservedNode),
      out(SEED, 1, 1, CRequest({ at: W1, origin: RepeatSlot((SER, 15)), where_key: NoKey })) == why(WhyReservedKey),
      out(SEED, 1, 1, CRequest({ at: 99, origin: Conv("c1"), where_key: NoKey })) == nfo(WhatNode),
      long_rejected(SEED, 1, 1, CRequest({ at: W1, origin: Conv("c1"), where_key: NoKey })),
    })
  }

  /// 1.10 (§9.2): a `said` by a session that holds neither the conversation node nor work inside it,
  /// while another session does (`conv_held`, the warning), counts as any `said` does: it answers
  /// and fulfils; 1.9 withheld it.
  run heldSaidTest = {
    val req = after(SEED, 1, 1, CRequest({ at: W1, origin: Msg(("c1", 1)), where_key: SomeKey(Conv("c1")) }))   // request 1
    val ack = after(req, 1, 1, CSaid({ at: W1, kind: SAck, where_key: NoKey }))                                  // said 2
    val held = after(ack, 2, 2, take_cmd(W1))
    val w = call(held, 3, 1, CSaid({ at: W1, kind: SReply, where_key: NoKey }))                                 // said 3
    assert(and {
      // 1.11 (§9.2): session 1 holds nothing, so its `said` entries are confirmed as recorded
      ack.saids.get(2).state == SdConfirmed,
      ack.commitments.get(2).state == CmOpen,
      w.outcome == OutOk([3, W1]),
      w.state.saids.get(3).state == SdConfirmed,
      w.state.requests.get(1).answered_by == 3,
      w.state.commitments.get(2).state == CmFulfilled(3),
      after(held, 3, 1, CSaid({ at: W1, kind: SAck, where_key: NoKey })).commitments.get(2).followup_at == 3 + COMMITMENT_FOLLOWUP,
      conv_held(held, W1, 1, 3),
      not(conv_held(held, W1, 2, 3)),
      not(conv_held(ack, W1, 1, 3)),
    })
  }

  /// `talk said` (D.8.15, §9.2): an ack opens a commitment, a second refreshes it; a reply fulfils
  /// the open commitments and answers the earlier requests from people (A-84), which settle once
  /// decided.
  run saidTest = {
    val req = after(SEED, 1, 1, CRequest({ at: W1, origin: Msg(("c1", 1)), where_key: SomeKey(Conv("c1")) }))
    val dec = after(req, 1, 1, CDecide({ at: W1, settles: Set(1), supersedes: -1, basis: [] }))     // decision 2
    val ack = call(dec, 1, 1, CSaid({ at: W1, kind: SAck, where_key: NoKey }))                                           // said 3
    val ack2 = after(ack.state, 2, 1, CSaid({ at: W1, kind: SAck, where_key: NoKey }))                                  // said 4
    val rep = after(ack2, 3, 2, CSaid({ at: W1, kind: SReply, where_key: NoKey }))                                      // said 5
    assert(and {
      ack.outcome == OutOk([3, W1]),
      // 1.11 (§9.2): session 1 holds nothing: confirmed as recorded
      ack.state.saids.get(3) == { node: W1, conv: Conv("c1"), kind: SAck, session: 1, state: SdConfirmed, moved_from: -1 },
      ack.state.commitments.get(3) == { node: W1, conv: Conv("c1"), last_ack: 1, followup_at: 1 + COMMITMENT_FOLLOWUP,
                                        followup_fired: false, state: CmOpen, moved_from: -1 },
      ack.state.requests.get(1).answered_by == -1,
      ack2.commitments.keys() == Set(3),
      ack2.commitments.get(3).followup_at == 2 + COMMITMENT_FOLLOWUP,
      rep.commitments.get(3).state == CmFulfilled(5),
      rep.requests.get(1).answered_by == 5,
      rep.requests.get(1).settled == SettledBy({ decision: 2, answer: 5 }),
      // the reply's ids: the said, its node, and the requests it settled (D.8.15, 1.3)
      out(ack2, 3, 2, CSaid({ at: W1, kind: SReply, where_key: NoKey })) == OutOk([5, W1, 1]),
      out(after(req, 1, 1, CSaid({ at: W1, kind: SAck, where_key: NoKey })), 2, 1, CSaid({ at: W1, kind: SResult, where_key: NoKey }))
        == OutOk([3, W1]),
      // from a child of /w1 the conversation is still /w1's (§9.1)
      after(after(SEED, 1, 1, create_cmd(W1, "a1", Set(), false, 1)), 1, 1, CSaid({ at: 12, kind: SAck, where_key: NoKey })).saids.get(1).node == W1,
      out(SEED, 1, 1, CSaid({ at: W1, kind: SQuestion, where_key: NoKey })) == why(WhySaidKindQuestion),
      out(SEED, 1, 1, CSaid({ at: SER, kind: SAck, where_key: NoKey })) == why(WhyNoConversationKey),
      out(after(SEED, 1, 1, CKeyAdd({ node: W1, key: Msg(("c1", 2)) })), 1, 1, CSaid({ at: W1, kind: SAck, where_key: NoKey }))
        == why(WhyAmbiguousConversationKey),
      out(after(SEED, 1, 1, CKeyAdd({ node: W1, key: Obj("o1") })), 1, 1, CSaid({ at: W1, kind: SAck, where_key: NoKey })) == OutOk([1, W1]),
      out(SEED, 1, 1, CSaid({ at: 99, kind: SAck, where_key: NoKey })) == nfo(WhatNode),
      long_rejected(SEED, 1, 1, CSaid({ at: W1, kind: SAck, where_key: NoKey })),
    })
  }

  /// 1.3 (§9.2, §9.4, D.8.8, D.8.15; D.13.2, WP-A2): on a node that carries two conversation keys,
  /// with a request and an open commitment in each, a reply in one conversation answers only that
  /// conversation's requests and fulfils only its commitments; the other conversation's request
  /// stays unanswered and its commitment open, until a reply in it. An ack refreshes only its own
  /// conversation's commitment. Without `where`, a `said` there is ambiguous.
  run saidTwoConversationsTest = {
    val r1 = after(SEED, 1, 1, CRequest({ at: W1, origin: Msg(("c1", 1)), where_key: SomeKey(Conv("c1")) }))   // request 1, c1
    val r2 = after(r1, 1, 1, CRequest({ at: W2, origin: Msg(("c2", 1)), where_key: SomeKey(Conv("c2")) }))     // request 2, c2
    val acks = after(after(r2, 1, 1, CSaid({ at: W1, kind: SAck, where_key: NoKey })), 1, 1,                  // commitment 3, c1
                     CSaid({ at: W2, kind: SAck, where_key: NoKey }))                                         // commitment 4, c2
    val dec = after(acks, 1, 1, CDecide({ at: W1, settles: Set(1), supersedes: -1, basis: [] }))               // decision 5
    val both = after(dec, 2, 2, CMerge({ from: W2, into: W1 }))
    val refreshed = after(both, 3, 1, CSaid({ at: W1, kind: SAck, where_key: SomeKey(Conv("c1")) }))
    val x1 = refreshed.next_eid
    val c1 = call(refreshed, 4, 1, CSaid({ at: W1, kind: SReply, where_key: SomeKey(Conv("c1")) }))           // said x1
    val x2 = c1.state.next_eid
    val c2 = after(c1.state, 5, 1, CSaid({ at: W1, kind: SReply, where_key: SomeKey(Conv("c2")) }))           // said x2
    assert(and {
      both.nodes.get(W1).keys == Set(Conv("c1"), Conv("c2")),
      both.requests.get(1).conv == Conv("c1"),
      both.requests.get(2).conv == Conv("c2"),
      both.requests.get(2).node == W1,
      both.commitments.get(4).node == W1,
      out(both, 3, 1, CSaid({ at: W1, kind: SReply, where_key: NoKey })) == why(WhyAmbiguousConversationKey),
      // an ack in c1 refreshes c1's commitment and opens none
      refreshed.commitments.keys() == Set(3, 4),
      refreshed.commitments.get(3).followup_at == 3 + COMMITMENT_FOLLOWUP,
      refreshed.commitments.get(4) == both.commitments.get(4),
      // a reply in c1 answers and settles c1's request and fulfils c1's commitment
      c1.outcome == OutOk([x1, W1, 1]),
      c1.state.saids.get(x1).conv == Conv("c1"),
      c1.state.requests.get(1).answered_by == x1,
      c1.state.requests.get(1).settled == SettledBy({ decision: 5, answer: x1 }),
      c1.state.commitments.get(3).state == CmFulfilled(x1),
      // ... and leaves c2's request unanswered and c2's commitment open
      c1.state.requests.get(2).answered_by == -1,
      c1.state.commitments.get(4).state == CmOpen,
      // a reply in c2 answers and fulfils c2's
      c2.requests.get(2).answered_by == x2,
      c2.commitments.get(4).state == CmFulfilled(x2),
      c2.requests.get(1).answered_by == x1,
    })
  }

  /// A reply fulfils only the commitments on its conversation node: a commitment of the same
  /// conversation left on a node that no longer carries the key stays open (§9.2, C1).
  run saidCommitmentNodeTest = {
    val acked = after(SEED, 1, 1, CSaid({ at: W1, kind: SAck, where_key: NoKey }))                         // commitment 1 on /w1
    val moved = after(after(acked, 1, 1, CKeyRemove({ node: W1, key: Conv("c1") })), 1, 1, CKeyAdd({ node: W2, key: Conv("c1") }))
    val rep = after(moved, 2, 1, CSaid({ at: W2, kind: SReply, where_key: SomeKey(Conv("c1")) }))          // said 2 on /w2
    assert(and {
      rep.saids.get(2).node == W2,
      rep.commitments.get(1).node == W1,
      rep.commitments.get(1).state == CmOpen,
    })
  }

  /// A request recorded with a message id and no `where` has the message id as its conversation
  /// (case 3 adds it to `at`); after `node key {remove}` repairs the node, a reply in the node's
  /// own conversation does not answer it, and a reply whose `where` is that message id does, adding
  /// the key again (§9.4, D.8.15, 1.3).
  run saidMessageConversationTest = {
    val r = after(SEED, 1, 1, CRequest({ at: W1, origin: Msg(("c1", 1)), where_key: NoKey }))
    val repaired = after(r, 1, 1, CKeyRemove({ node: W1, key: Msg(("c1", 1)) }))
    val other = after(repaired, 2, 1, CSaid({ at: W1, kind: SReply, where_key: NoKey }))                     // said 2, c1
    val own = after(other, 3, 1, CSaid({ at: W1, kind: SReply, where_key: SomeKey(Msg(("c1", 1))) }))       // said 3
    assert(and {
      r.requests.get(1).conv == Msg(("c1", 1)),
      r.nodes.get(W1).keys == Set(Conv("c1"), Msg(("c1", 1))),
      out(r, 1, 1, CSaid({ at: W1, kind: SReply, where_key: NoKey })) == why(WhyAmbiguousConversationKey),
      other.saids.get(2).conv == Conv("c1"),
      other.requests.get(1).answered_by == -1,
      own.nodes.get(W1).keys == Set(Conv("c1"), Msg(("c1", 1))),
      own.saids.get(3).conv == Msg(("c1", 1)),
      own.requests.get(1).answered_by == 3,
      out(SEED, 1, 1, CSaid({ at: W1, kind: SReply, where_key: SomeKey(RepeatSlot((SER, 15))) })) == why(WhyReservedKey),
      out(SEED, 1, 1, CSaid({ at: ROOT, kind: SReply, where_key: SomeKey(Conv("c9")) })) == why(WhyReservedNode),
    })
  }

  /// `talk said {said_id, confirmed}` (D.8.15).
  run confirmTest = {
    val ack = after(SEED, 1, 1, CSaid({ at: W1, kind: SAck, where_key: NoKey }))
    val c = after(ack, 2, 1, CConfirm(1))
    assert(and {
      c.saids.get(1).state == SdConfirmed,
      call(c, 2, 1, CConfirm(1)) == { state: c, outcome: OutOk([1]) },
      out(ack, 2, 1, CConfirm(2)) == nfo(WhatSaid),
      long_rejected(ack, 2, 1, CConfirm(1)),
    })
  }

  /// `talk ask` to a person (D.8.15, §9.3): the `said` of kind question and the question; asking
  /// once; `again` supersedes and moves the unresolved waits (A-109).
  run askPersonTest = {
    val r = call(SEED, 1, 1, CAskPerson({ at: W1, blocking: true, again: false }))
    val st = r.state
    val waits = after(after(st, 1, 2, take_cmd(W2)), 2, 2, CRelease({ token: (W2, 1), reason: Blocked, wait: WaQuestion(2), settles: Set() }))
    val again = after(waits, 3, 1, CAskPerson({ at: W1, blocking: false, again: true }))
    assert(and {
      r.outcome == OutOk([2, 1, W1]),
      st.saids.get(1).kind == SQuestion,
      st.pquestions.get(2) == { node: W1, conv: Conv("c1"), blocking: true, said: 1, state: PqOpen, rechecks: 0,
                                recheck_at: 1 + QUESTION_RECHECK, lapse: NoLapse, assumed_by: -1, moved_from: -1 },
      call(st, 2, 2, CAskPerson({ at: W1, blocking: false, again: false })).outcome == OutOk([2, -1, W1]),
      call(st, 2, 2, CAskPerson({ at: W1, blocking: false, again: false })).state.pquestions == st.pquestions,
      again.pquestions.get(2).state == PqSuperseded(5),
      again.pquestions.get(5).said == 4,
      again.nodes.get(W2).wait == HasWait({ on: WQuestion({ q: 5, rechecks_at_set: 0 }), set_at: 3, fired: false }),
      out(SEED, 1, 1, CAskPerson({ at: SER, blocking: false, again: false })) == why(WhyNoConversationKey),
      long_rejected(SEED, 1, 1, CAskPerson({ at: W1, blocking: false, again: false })),
    })
  }

  /// `talk heard` (D.8.15): with `answers`, an open, expired, or escalated people question is
  /// answered; an answered one is not changed. An answer to an escalated question whose user
  /// question lapsed clears the lapse's assume marks (CS-2).
  run heardTest = {
    val asked = after(SEED, 1, 1, CAskPerson({ at: W1, blocking: false, again: false }))   // said 1, question 2
    val h = call(asked, 2, 2, CHeard({ at: W1, answers: 2 }))
    val uq = after(SEED, 1, 1, CAskUser({ at: W1, has_default: false, to: "user", again: false, digest: 1 }))
    val expired = { ...asked, pquestions: asked.pquestions.setBy(2, q => { ...q, state: PqExpired, rechecks: 2 }) }
    val lapsed = {
      ...asked,
      next_eid: 4,
      pquestions: asked.pquestions.setBy(2, q => { ...q, blocking: true, state: PqEscalated(3), rechecks: 2 }),
      uquestions: Map(3 -> { source: SrcEscalation(2), about: W1, to: [0], stage: 0, due: 30, has_default: false,
                             state: UqLapsed, assumed_by: -1 }),
      entries: asked.entries.put(3, { node: USER_DECISIONS, seq: 1, at: 1, by: 0, ty: EQuestion, principal: -1 }),
      nodes: asked.nodes.setBy(W2, nd => { ...nd, assume_marks: Set(3, 7) }),
    }
    val hl = after(lapsed, 2, 2, CHeard({ at: W1, answers: 2 }))
    assert(and {
      after(expired, 2, 2, CHeard({ at: W1, answers: 2 })).pquestions.get(2).state == PqAnswered(3),
      hl.pquestions.get(2).state == PqAnswered(4),
      hl.uquestions.get(3).state == UqLapsed,
      hl.nodes.get(W2).assume_marks == Set(7),
      h.outcome == OutOk([3, W1, 2]),
      h.state.pquestions.get(2).state == PqAnswered(3),
      h.state.entries.get(3).ty == EHeard,
      call(h.state, 3, 2, CHeard({ at: W1, answers: 2 })).outcome == OutOk([4, W1, -1]),
      call(h.state, 3, 2, CHeard({ at: W1, answers: 2 })).state.pquestions == h.state.pquestions,
      out(asked, 2, 2, CHeard({ at: W1, answers: -1 })) == OutOk([3, W1, -1]),
      out(uq, 2, 2, CHeard({ at: W1, answers: 1 })) == why(WhyNotPeopleQuestion),
      out(asked, 2, 2, CHeard({ at: W1, answers: 7 })) == nfo(WhatQuestion),
      long_rejected(asked, 2, 2, CHeard({ at: W1, answers: 2 })),
    })
  }

  // ================================================================== talk: agents (D.8.16)

  /// `talk ask {to: agent}` and `talk answer` (D.8.16, A-36): the question with its note, the
  /// answer as a note to the asker; a `holder:` question binds to the next holder.
  run askAgentTest = {
    val q = call(SEED, 1, 1, CAskAgent({ at: W1, to: AgSession(2) }))
    val a = call(q.state, 2, 2, CAnswer(1))
    val hold_q = after(SEED, 1, 1, CAskAgent({ at: W1, to: AgHolder(W1) }))
    assert(and {
      q.outcome == OutOk([1]),
      q.state.aquestions.get(1) == { node: W1, asker: 1, state: AqOpen },
      q.state.notes.get(1) == { node: W1, to: ToSession(2), until: 1 + DELIVER_WINDOW, recipients: Set(2),
                                pending_holder: false, read_by: Set() },
      a.outcome == OutOk([2]),
      a.state.aquestions.get(1).state == AqAnswered(2),
      a.state.notes.get(2) == { node: W1, to: ToSession(1), until: 2 + DELIVER_WINDOW, recipients: Set(1),
                                pending_holder: false, read_by: Set() },
      hold_q.notes.get(1).pending_holder,
      after(hold_q, 2, 2, take_cmd(W1)).notes.get(1).recipients == Set(2),
      not(after(hold_q, 2, 2, take_cmd(W1)).notes.get(1).pending_holder),
      // an agent question that left memory with its note is still answered, without a state change
      after({ ...q.state, aquestions: Map(), notes: Map() }, 2, 2, CAnswer(1)).aquestions == Map(),
      out(SEED, 1, 1, CAskAgent({ at: W1, to: AgSession(7) })) == nfo(WhatAddressee),
      out(SEED, 1, 1, CAskAgent({ at: W1, to: AgHolders(99) })) == nfo(WhatNode),
      out(q.state, 2, 2, CAnswer(2)) == nfo(WhatQuestion),
      out(after(SEED, 1, 1, CAskPerson({ at: W1, blocking: false, again: false })), 2, 2, CAnswer(2)) == why(WhyNotAgentQuestion),
      long_rejected(SEED, 1, 1, CAskAgent({ at: W1, to: AgSession(2) })),
      long_rejected(q.state, 2, 2, CAnswer(1)),
    })
  }

  /// `talk say` and `talk wait` (D.8.16, D-6, CS-3): a plain note has no addressee and no metadata
  /// (CS-17), unless another session holds its node (1.9: then it is a `holder:` note to that
  /// holder) or nobody does (1.10: a `holder:` note for its next holder); `wait` delivers once; a `holders:` note reaches the holders of the branch through
  /// their cursor; a bare `user` is the head of the chain.
  run sayWaitTest = {
    val plain = call(SEED, 1, 1, CSay({ at: ROOT, to: SayNone }))
    // 1.10 (§13): at a node nobody holds that can be held, a `holder:` note for its next holder,
    // which a take by another session binds, and the sender's own take does not.
    val next_holder = after(SEED, 1, 1, CSay({ at: W1, to: SayNone }))
    val to2 = after(SEED, 1, 1, CSay({ at: W1, to: SaySession(2) }))
    val w = call(to2, 2, 2, CWait(-1))
    val branch = after(after(SEED, 1, 2, take_cmd(W2)), 2, 1, CSay({ at: ROOT, to: SayHolders(ROOT) }))
    val bw = after(branch, 3, 2, CWait(-1))
    // 1.9 (§13): a plain note at a node another session holds reaches that holder.
    val held = after(SEED, 1, 2, take_cmd(W1))
    val to_holder = after(held, 2, 1, CSay({ at: W1, to: SayNone }))
    assert(and {
      to_holder.notes.get(1).to == ToHolder(W1),
      to_holder.notes.get(1).recipients == Set(2),
      not(to_holder.notes.get(1).pending_holder),
      call(to_holder, 3, 2, CWait(-1)).outcome == OutOk([1]),
      after(held, 2, 2, CSay({ at: W1, to: SayNone })).notes == Map(),
      plain.outcome == OutOk([1]),
      plain.state.notes == Map(),
      plain.state.entries == Map(),
      plain.state.nodes.get(ROOT).seq == SEED.nodes.get(ROOT).seq + 1,
      next_holder.notes.get(1).to == ToHolder(W1),
      next_holder.notes.get(1).recipients == Set(),
      next_holder.notes.get(1).pending_holder,
      after(next_holder, 2, 2, take_cmd(W1)).notes.get(1).recipients == Set(2),
      after(next_holder, 2, 1, take_cmd(W1)).notes.get(1).pending_holder,
      to2.notes.get(1).recipients == Set(2),
      w.outcome == OutOk([1]),
      w.state.notes.get(1).read_by == Set(2),
      call(w.state, 2, 2, CWait(-1)) == { state: w.state, outcome: OutOk([]) },
      bw.sessions.get(2).branch_seen == Map(ROOT -> 1),
      call(bw, 3, 2, CWait(-1)).outcome == OutOk([]),
      call(branch, 3, 1, CWait(-1)).outcome == OutOk([]),
      after(SEED, 1, 1, CSay({ at: W1, to: SayUser })).notes.get(1).to == ToPrincipal(USER_PRINCIPAL),
      after(SEED, 1, 1, CSay({ at: W1, to: SayPrincipal("p2") })).notes.get(1).to == ToPrincipal(1),
      after(SEED, 1, 1, CSay({ at: W1, to: SayHolder(W1) })).notes.get(1).pending_holder,
      out(SEED, 1, 1, CSay({ at: W1, to: SaySession(7) })) == nfo(WhatAddressee),
      out(SEED, 1, 1, CWait(99)) == nfo(WhatNode),
      // an expired note is pruned at the next event (CS-17)
      after(to2, 1 + DELIVER_WINDOW, 1, CObject(W2)).notes == Map(),
      long_rejected(SEED, 1, 1, CSay({ at: W1, to: SaySession(2) })),
    })
  }

  // ================================================================== talk: review (D.8.17)

  /// `talk decide` (D.8.17, CS-22, A-71, A-88): the basis as references (a node with its outcome
  /// now, through `merged_into`), supersession, and the refusals in their order.
  run decideTest = {
    val d = call(SEED, 1, 1, CDecide({ at: W1, settles: Set(), supersedes: -1, basis: [BaNode({ id: W2, version: -1 }), BaEntry(5)] }))
    val st = after(SEED, 1, 1, CDecide({ at: W1, settles: Set(), supersedes: -1, basis: [BaNode({ id: W2, version: 1 })] }))
    val sup = after(st, 2, 1, CDecide({ at: W1, settles: Set(), supersedes: 1, basis: [] }))
    val mg = after(SEED, 1, 1, CMerge({ from: W2, into: W1 }))
    assert(and {
      out(SEED, 1, 1, CDecide({ at: W1, settles: Set(), supersedes: -1, basis: [BaEntry(5)] })) == nfo(WhatEntry),
      d.outcome == nfo(WhatEntry),
      st.decisions.get(1) == { node: W1, settles: Set(), state: DActive, basis: [BNode({ id: W2, life_at: AtOpen })] },
      sup.decisions.get(1).state == DSuperseded(2),
      sup.decisions.get(2).state == DActive,
      out(sup, 2, 1, CDecide({ at: W1, settles: Set(), supersedes: 1, basis: [] })) == why(WhyDecisionNotActive),
      out(SEED, 1, 1, CDecide({ at: W1, settles: Set(), supersedes: -1, basis: [BaNode({ id: W2, version: 2 })] })) == why(WhyVersionAhead),
      out(SEED, 1, 1, CDecide({ at: W1, settles: Set(), supersedes: -1, basis: [BaNode({ id: W1, version: -1 })] })) == why(WhySelfLink),
      // a reference to a node merged into `at` is a reference to `at` (A-88)
      out(mg, 2, 1, CDecide({ at: W1, settles: Set(), supersedes: -1, basis: [BaNode({ id: W2, version: -1 })] })) == why(WhySelfLink),
      // eids 1 and 2 are the merge's system entries
      after(mg, 2, 1, CDecide({ at: SER, settles: Set(), supersedes: -1, basis: [BaNode({ id: W2, version: -1 })] })).decisions.get(3).basis
        == [BNode({ id: W1, life_at: AtOpen })],
      after(after(SEED, 1, 1, close_cmd(W2, Failed)), 2, 1, CDecide({ at: W1, settles: Set(), supersedes: -1,
            basis: [BaNode({ id: W2, version: -1 })] })).decisions.get(2).basis == [BNode({ id: W2, life_at: AtClosed(Failed) })],
      out(SEED, 1, 1, CDecide({ at: W1, settles: Set(3), supersedes: -1, basis: [] })) == nfo(WhatRequest),
      out(SEED, 1, 1, CDecide({ at: W1, settles: Set(), supersedes: 3, basis: [] })) == nfo(WhatDecision),
      out(SEED, 1, 1, CDecide({ at: W1, settles: Set(), supersedes: -1, basis: [BaNode({ id: 99, version: -1 })] })) == nfo(WhatNode),
      long_rejected(SEED, 1, 1, CDecide({ at: W1, settles: Set(), supersedes: -1, basis: [] })),
    })
  }

  /// A decision records the node's assumptions (A-15) and closes an open signal node (A-34).
  run decideAssumesTest = {
    val pq = {
      ...SEED,
      next_eid: 3,
      pquestions: Map(2 -> { node: W1, conv: Conv("c1"), blocking: false, said: 1, state: PqExpired, rechecks: 2,
                             recheck_at: 16, lapse: LapseExpired, assumed_by: -1, moved_from: -1 }),
      nodes: SEED.nodes.setBy(W1, nd => { ...nd, assume_marks: Set(2) }),
    }
    val d = after(pq, 2, 1, CDecide({ at: W1, settles: Set(), supersedes: -1, basis: [] }))
    val sig = after(SEED, 1, 1, CLink({ node: W1, rel: Duplicate, to: W2 }))
    assert(and {
      d.pquestions.get(2).assumed_by == 3,
      d.nodes.get(W1).assume_marks == Set(),
      not(f_proceed_on_assumption(d, W1, 2)),
      closed_as(after(sig, 2, 1, CDecide({ at: 12, settles: Set(), supersedes: -1, basis: [] })), 12, Done),
      after(sig, 2, 1, CDecide({ at: 12, settles: Set(), supersedes: -1, basis: [] })).signals == Map(),
    })
  }

  /// `talk retract` (D.8.17, D-4): the target is struck out; an active decision is retracted, and a
  /// decision that cites it gets `basis_changed`.
  run retractTest = {
    val d1 = after(SEED, 1, 1, CDecide({ at: W1, settles: Set(), supersedes: -1, basis: [] }))
    val d2 = after(d1, 1, 1, CDecide({ at: W2, settles: Set(), supersedes: -1, basis: [BaEntry(1)] }))
    val r = call(d2, 2, 1, CRetract(TEntry(1)))
    assert(and {
      r.outcome == OutOk([3]),
      r.state.retracted == Set(TEntry(1)),
      r.state.decisions.get(1).state == DRetracted(3),
      f_basis_changed(r.state, W2, 2),
      not(f_basis_changed(d2, W2, 2)),
      after(SEED, 1, 1, CRetract(TNode(W2))).retracted == Set(TNode(W2)),
      out(SEED, 1, 1, CRetract(TNode(99))) == nfo(WhatNode),
      out(SEED, 1, 1, CRetract(TEntry(1))) == nfo(WhatEntry),
      long_rejected(d2, 2, 1, CRetract(TEntry(1))),
    })
  }

  /// `talk approve`, `object`, and `resolve` (D.8.17).
  run reviewTest = {
    val ap = call(SEED, 1, 1, CApprove({ node: W1, ver: 1 }))
    val ob = after(SEED, 1, 1, CObject(W1))
    val rs = call(ob, 2, 2, CResolve({ objection: 1, upheld: false, defer: false }))
    assert(and {
      ap.outcome == OutOk([1]),
      ap.state.approvals == Map((W1, 1) -> 1),
      after(ap.state, 1, 1, CApprove({ node: W1, ver: 1 })).approvals == Map((W1, 1) -> 2),
      out(SEED, 1, 1, CApprove({ node: W1, ver: 2 })) == why(WhyVersionAhead),
      ob.objections == Map(1 -> { node: W1, state: ObOpen, moved_from: -1 }),
      f_objection(ob, W1, 1),
      rs.outcome == OutOk([2]),
      rs.state.objections.get(1).state == ObResolved(2),
      call(rs.state, 2, 2, CResolve({ objection: 1, upheld: false, defer: false })) == { state: rs.state, outcome: OutOk([-1]) },
      out(SEED, 1, 1, CResolve({ objection: 1, upheld: false, defer: false })) == nfo(WhatObjection),
      long_rejected(SEED, 1, 1, CApprove({ node: W1, ver: 1 })),
      long_rejected(SEED, 1, 1, CObject(W1)),
      long_rejected(ob, 2, 2, CResolve({ objection: 1, upheld: false, defer: false })),
    })
  }

  /// 1.8 (§11.3, §5, §10.1, §10.4): an upheld resolve on a closed node corrects it: `done` becomes
  /// `failed` (no kind in the model), the closure is marked upheld, no counter moves, `next` never
  /// offers it, and a take of it is `closed`. On an open node it records the verdict only. 1.9
  /// (§10.1, §10.4): before the resolve, the closed node with an open objection is a review item
  /// (kind 1, no hold), `next` gives it without reopening it, and a take of it is `closed`.
  run upheldTest = {
    val closed = after(SEED, 1, 1, close_cmd(W1, Done))
    val ob = after(closed, 1, 1, CObject(W1))
    val rs = call(ob, 2, 2, CResolve({ objection: 2, upheld: true, defer: false }))
    val open_ob = after(SEED, 1, 1, CObject(W2))
    val open_rs = after(open_ob, 2, 2, CResolve({ objection: 1, upheld: true, defer: false }))
    assert(and {
      // 1.9: a closed node with an objection is a review item, not a node to take
      classify(ob, W1, flags_of(ob, W1, 1), 1) == 1,
      out(ob, 1, 1, take_cmd(W1)) == err(ErrClosed),
      val rv = call(ob, 2, 2, CNext)
      and {
        rv.outcome == OutOk([1, W1]),
        not(node_is_open(rv.state.nodes.get(W1))),
        holds_on(rv.state, W1) == Set(),
      },
      rs.outcome == OutOk([3]),
      rs.state.objections.get(2).state == ObUpheld(3),
      rs.state.nodes.get(W1).life == Closed({ at: 1, outcome: Failed, merged_into: -1, upheld: 3, from_done: true, by: 1 }),
      rs.state.nodes.get(W1).version == ob.nodes.get(W1).version,
      upheld(rs.state, W1),
      classify(rs.state, W1, flags_of(rs.state, W1, 2), 2) == -1,
      // a later objection flags it again, and still nothing offers or takes it
      val again = after(rs.state, 2, 1, CObject(W1))
      and {
        f_objection(again, W1, 2),
        classify(again, W1, flags_of(again, W1, 2), 2) == -1,
        out(again, 2, 1, take_cmd(W1)) == err(ErrClosed),
        best_candidate(CFG, again, 1, "a", 2).id != W1,
        // `node reopen` reopens it, without the mark
        not(upheld(after(again, 2, 1, CReopen(W1)), W1)),
      },
      // open: the verdict only
      open_rs.objections.get(1).state == ObUpheld(2),
      open_rs.nodes.get(W2).life == Open,
    })
  }

  /// 1.10 (§10.4): a review item ranks by the ring before age: within its tier, after the
  /// recheck-due nodes, the reviews come before the other candidates, whatever their ids and ring
  /// positions, so that two sessions given reviews start at different ones. 1.11: a review item's
  /// tier is its node's without the objection, so the review of a closed node with no other flag is
  /// tier 2, before the plain offers there, and after an open node whose objection is tier 1.
  run reviewOrderTest = {
    val closed = after(SEED, 1, 1, close_cmd(W2, Done))
    val ob = after(closed, 1, 2, CObject(W2))
    val both = after(ob, 1, 1, CObject(W1))
    val fnv = { ...CFG, use_fnv: true }
    assert(and {
      W1 < W2,
      classify(ob, W1, flags_of(ob, W1, 1), 1) == 0,
      classify(ob, W2, flags_of(ob, W2, 1), 1) == 1,
      tier_of(ob, W1, flags_of(ob, W1, 1)) == 2,
      tier_of(ob, W2, flags_of(ob, W2, 1).exclude(Set(FObjection))) == 2,
      best_candidate(CFG, ob, 1, "a", 1).id == W2,
      best_candidate(CFG, ob, 1, "a", 1).review == 0,
      best_candidate(CFG, ob, 1, "a", 1).tier == 2,
      best_candidate(fnv, ob, 1, "a", 1).id == W2,
      // 1.12 (§10.4): never to the session that raised the objection
      own_review(ob, 2, W2),
      best_candidate(fnv, ob, 2, "a", 1).id == W1,
      // an open objection is tier 1: before the review, which its objection no longer lifts
      tier_of(both, W1, flags_of(both, W1, 1)) == 1,
      best_candidate(CFG, both, 1, "a", 1).id == W1,
      // with no review, the open node comes first again
      best_candidate(CFG, after(both, 2, 2, CResolve({ objection: 2, upheld: false, defer: false })), 1, "a", 2).id == W1,
    })
  }

  /// 1.12 (§10.4): a `handoff` release keeps the node from its releaser, for `lease.first` of the
  /// hold's terms or until another hold of it is issued; other sessions get it at once. A blocked
  /// release keeps nothing.
  run handoffTest = {
    val held = after(SEED, 1, 1, take_cmd(W1))
    val left = after(held, 1, 1, CRelease({ token: (W1, 1), reason: Handoff, wait: WaNone, settles: Set() }))
    val taken = after(left, 2, 2, take_cmd(W1))
    val again = after(taken, 2, 2, CRelease({ token: (W1, 2), reason: Handoff, wait: WaNone, settles: Set() }))
    val blocked = after(held, 1, 1, CRelease({ token: (W1, 1), reason: Blocked, wait: WaTime(1), settles: Set() }))
    assert(and {
      left.sessions.get(1).handed_off == { node: W1, fence: 1, until: 1 + LEASE_FIRST },
      handed_back(left, 1, W1, 1),
      best_candidate(CFG, left, 1, "a", 1).id == W2,
      best_candidate(CFG, left, 2, "b", 1).id == W1,
      // `lease.first` later, it is offered to its releaser again
      not(handed_back(left, 1, W1, 1 + LEASE_FIRST)),
      best_candidate(CFG, left, 1, "a", 1 + LEASE_FIRST).id == W1,
      // another hold of it ends the keeping: session 2 handed it off in turn
      not(handed_back(again, 1, W1, 2)),
      handed_back(again, 2, W1, 2),
      best_candidate(CFG, again, 1, "a", 2).id == W1,
      blocked.sessions.get(1).handed_off == NO_HANDOFF,
    })
  }

  /// 1.11 (§10.4): within a tier, a candidate whose node needs more labels, all among the caller's
  /// caps, comes first: a session with caps {x} gets W2, which needs x, before W1, which needs
  /// nothing; a session without x is not offered W2 at all.
  run needsOrderTest = {
    val opened = after(SEED, 1, -1, CHello({ caps: Set("x"), principal: "" }))
    val needy = after(opened, 1, 1, CUpdate({ ...update_none(W2, 1), needs_add: Set("x") }))
    assert(and {
      best_candidate(CFG, opened, 3, "a", 1).id == W1,
      best_candidate(CFG, needy, 3, "a", 1).id == W2,
      best_candidate(CFG, needy, 3, "a", 1).needs == -1,
      best_candidate(CFG, needy, 1, "a", 1).id == W1,
    })
  }

  /// 1.11 (§10.4): within a tier, a candidate flagged `unanswered_request` comes before one that is
  /// not: W2 carries a request nobody is at work on and W1 an open intent of a session that let it
  /// go, both tier 0; W2 comes first whatever their ids.
  run requestOrderTest = {
    val held = after(SEED, 1, 1, take_cmd(W1))
    val intent = after(held, 1, 1, CCheckpoint({ token: (W1, 1), intent: true, intent_done: false }))
    val left = after(intent, 1, 1, CRelease({ token: (W1, 1), reason: Handoff, wait: WaNone, settles: Set() }))
    val asked = after(left, 1, 1, CRequest({ at: W2, origin: Msg(("c2", 1)), where_key: SomeKey(Conv("c2")) }))
    assert(and {
      flags_of(left, W1, 1).contains(FOpenIntent),
      tier_of(left, W1, flags_of(left, W1, 1)) == 0,
      flags_of(asked, W2, 1).contains(FUnansweredRequest),
      best_candidate(CFG, left, 2, "b", 1).id == W1,
      best_candidate(CFG, asked, 2, "b", 1).id == W2,
      best_candidate(CFG, asked, 2, "b", 1).request == 0,
    })
  }

  /// 1.11 (§10.4, §12): a series whose open occurrence waits for its slot is held back from
  /// `next`, for every caller; once the occurrence is available, it is not.
  run seriesBeforeSlotTest = assert(and {
    occ_waiting(SEED.nodes.get(OCC1)),
    series_before_slot(SEED, SER),
    held_back(SEED, SER, 1),
    not(series_before_slot(SEED, W1)),
    not(series_before_slot({ ...SEED, nodes: SEED.nodes.setBy(OCC1, nd => match nd.role {
      | Occurrence(o) => { ...nd, role: Occurrence({ ...o, phase: Available }) }
      | _ => nd
    }) }, SER)),
  })

  /// 1.11 (§14): a duplicate signal resolves, and its node closes, when a close leaves both nodes
  /// of its pair closed, neither merged; a conflict signal does not.
  run duplicateBothClosedTest = {
    val dup = after(SEED, 1, 1, CLink({ node: W2, rel: Duplicate, to: W1 }))
    val sig = dup.signals.get((SigDuplicate, W1, W2))
    val one = after(dup, 1, 1, close_cmd(W1, Done))
    val both = after(one, 1, 1, close_cmd(W2, Dropped))
    val con = after(SEED, 1, 1, CLink({ node: W2, rel: ConflictsWith, to: W1 }))
    val con_both = after(after(con, 1, 1, close_cmd(W1, Done)), 1, 1, close_cmd(W2, Done))
    assert(and {
      dup.signals.keys().contains((SigDuplicate, W1, W2)),
      one.signals.keys().contains((SigDuplicate, W1, W2)),
      is_open(one, sig),
      not(both.signals.keys().contains((SigDuplicate, W1, W2))),
      closed_as(both, sig, Done),
      con_both.signals.keys().contains((SigConflict, W1, W2)),
    })
  }

  /// 1.10 (§11.3): a second, opposite verdict on a resolved objection records a new open objection
  /// on its node (reply ids `[objection]`), which flags the closed node as a review item again; the
  /// same verdict records nothing. After an upheld verdict that still marks the node, the dissent
  /// withdraws the correction: `done` again, unmarked; no counter moves.
  run disputeTest = {
    val closed = after(SEED, 1, 1, close_cmd(W1, Done))
    val ob = after(closed, 1, 1, CObject(W1))                                    // objection 2
    val kept = after(ob, 2, 2, CResolve({ objection: 2, upheld: false, defer: false }))       // resolve 3
    val d1 = call(kept, 3, 1, CResolve({ objection: 2, upheld: true, defer: false }))         // objection 4
    val up = after(ob, 2, 2, CResolve({ objection: 2, upheld: true, defer: false }))          // resolve 3, corrected
    val d2 = call(up, 3, 1, CResolve({ objection: 2, upheld: false, defer: false }))          // objection 4, withdrawn
    val third = after(d2.state, 4, 2, CResolve({ objection: 4, upheld: true, defer: false }))  // resolve 5
    assert(and {
      out(kept, 3, 1, CResolve({ objection: 2, upheld: false, defer: false })) == OutOk([-1]),
      d1.outcome == OutOk([4]),
      d1.state.objections.get(2).state == ObResolved(3),
      d1.state.objections.get(4) == { node: W1, state: ObOpen, moved_from: -1 },
      f_objection(d1.state, W1, 3),
      classify(d1.state, W1, flags_of(d1.state, W1, 3), 3) == 1,
      d1.state.nodes.get(W1).life == closed.nodes.get(W1).life,
      up.nodes.get(W1).life == Closed({ at: 1, outcome: Failed, merged_into: -1, upheld: 3, from_done: true, by: 1 }),
      d2.outcome == OutOk([4]),
      d2.state.nodes.get(W1).life == Closed({ at: 1, outcome: Done, merged_into: -1, upheld: -1, from_done: false, by: 1 }),
      d2.state.nodes.get(W1).version == up.nodes.get(W1).version,
      not(upheld(d2.state, W1)),
      classify(d2.state, W1, flags_of(d2.state, W1, 3), 3) == 1,
      out(d2.state, 3, 1, take_cmd(W1)) == err(ErrClosed),
      third.nodes.get(W1).life == Closed({ at: 1, outcome: Failed, merged_into: -1, upheld: 5, from_done: true, by: 1 }),
      // a dissent from a verdict on an open node withdraws nothing
      val open_up = after(after(SEED, 1, 1, CObject(W2)), 2, 2, CResolve({ objection: 1, upheld: true, defer: false }))
      after(open_up, 3, 1, CResolve({ objection: 1, upheld: false, defer: false })).nodes.get(W2).life == Open,
    })
  }

  /// 1.10 (§10.2, §10.3): a checkpoint of a live hold ends its node's wait, of any kind, resolved or
  /// not (T5's graph loses the edge); a renew does not.
  run checkpointWaitTest = {
    val waits = after(after(SEED, 1, 1, take_cmd(W1)), 2, 1, CRelease({ token: (W1, 1), reason: Blocked, wait: WaNode(W2), settles: Set() }))
    val back = after(waits, 3, 1, take_cmd(W1))
    val cp = after(back, 4, 1, CCheckpoint({ token: (W1, 2), intent: false, intent_done: false }))
    assert(and {
      back.nodes.get(W1).wait == HasWait({ on: WNode(W2), set_at: 2, fired: false }),
      after(back, 4, 1, CRenew((W1, 2))).nodes.get(W1).wait == back.nodes.get(W1).wait,
      cp.nodes.get(W1).wait == NoWait,
      succ(cp, W1) == Set(),
      succ(back, W1) == Set(W2),
    })
  }

  /// 1.11 (§10.3, §11.3): a deferral of a closed node's review is a `resolve` entry that decides
  /// nothing, and the caller's refusal of the node, as a `cannot` release records one: `next` gives
  /// the review to the caller's class no more, and to the other class still, until the backoff
  /// ends. A verdict on the closed node clears its refusals. A deferral of a resolved objection
  /// records nothing; one with `upheld` is `invalid`; on an open node it records the entry only.
  run deferTest = {
    val closed = after(SEED, 1, 1, close_cmd(W1, Done))
    val ob = after(closed, 1, 1, CObject(W1))                                                     // objection 2
    val df = call(ob, 2, 2, CResolve({ objection: 2, upheld: false, defer: true }))               // resolve 3
    val kept = after(df.state, 2, 1, CResolve({ objection: 2, upheld: false, defer: false }))      // resolve 4
    val open_ob = after(SEED, 1, 1, CObject(W2))                                                  // objection 1
    val third = after(df.state, 2, -1, CHello({ caps: Set(), principal: "" }))                    // session 3, a
    assert(and {
      df.outcome == OutOk([3]),
      df.state.objections.get(2).state == ObOpen,
      f_objection(df.state, W1, 2),
      df.state.refusals.get((W1, "b")) == { quorum: QUORUM, by: Map(2 -> { backoff_until: 2 + REFUSAL_BACKOFF, expires_at: 2 + REFUSAL_TTL }) },
      classify(df.state, W1, flags_of(df.state, W1, 2), 2) == 1,
      excluded(df.state, W1, "b", 2),
      not(excluded(df.state, W1, "a", 2)),
      best_candidate(CFG, df.state, 2, "b", 2).id != W1,
      best_candidate(CFG, third, 3, "a", 2).id == W1,
      // 1.12 (§10.4): never to the session that raised the objection
      best_candidate(CFG, df.state, 1, "a", 2).id != W1,
      not(excluded(df.state, W1, "b", 2 + REFUSAL_BACKOFF)),
      // a verdict on the closed node clears its refusals
      kept.objections.get(2).state == ObResolved(4),
      kept.refusals == Map(),
      // a deferral of a resolved objection records nothing
      out(kept, 3, 2, CResolve({ objection: 2, upheld: false, defer: true })) == OutOk([-1]),
      after(kept, 3, 2, CResolve({ objection: 2, upheld: false, defer: true })).next_eid == kept.next_eid,
      out(ob, 2, 2, CResolve({ objection: 2, upheld: true, defer: true })) == why(WhyBadParams),
      out(SEED, 2, 2, CResolve({ objection: 9, upheld: true, defer: true })) == nfo(WhatObjection),
      // on an open node: the entry, and no refusal
      after(open_ob, 2, 2, CResolve({ objection: 1, upheld: false, defer: true })).refusals == Map(),
      after(open_ob, 2, 2, CResolve({ objection: 1, upheld: false, defer: true })).objections.get(1).state == ObOpen,
    })
  }

  /// 1.11 (§9.2): a `said` by a session that holds nothing, by `talk said` or `talk ask`, is
  /// confirmed as it is recorded; one by a session that holds a node stays pending until its
  /// release or close.
  run saidAloneTest = {
    val alone = call(SEED, 1, 1, CSaid({ at: W1, kind: SReply, where_key: NoKey }))              // said 1
    val asked = call(SEED, 1, 1, CAskPerson({ at: W1, blocking: false, again: false }))           // said 1, question 2
    val held = after(SEED, 1, 1, take_cmd(W2))
    val busy = call(held, 1, 1, CSaid({ at: W1, kind: SReply, where_key: NoKey }))               // said 1
    assert(and {
      alone.outcome == OutOk([1, W1]),
      alone.state.saids.get(1).state == SdConfirmed,
      asked.state.saids.get(1).state == SdConfirmed,
      busy.state.saids.get(1).state == SdPending,
      holds_nothing(SEED, 1, 1),
      not(holds_nothing(held, 1, 1)),
    })
  }

  /// 1.11 (§13): a note without `to` at a closed node reaches the session that closed it while
  /// it is live, as `session:`; the closer's own note there, and one after it falls silent, are
  /// plain notes. An open node nobody holds keeps 1.10's next holder.
  run closerNoteTest = {
    val closed = after(after(SEED, 1, 1, take_cmd(W1)), 1, 1, close_cmd(W1, Done))
    val e = closed.next_eid
    val to_closer = after(closed, 2, 2, CSay({ at: W1, to: SayNone }))
    assert(and {
      match closed.nodes.get(W1).life { | Closed(c) => c.by == 1 | Open => false },
      closer(closed, W1, 2) == 1,
      closer(closed, W1, 1) == -1,
      to_closer.notes.get(e).to == ToSession(1),
      to_closer.notes.get(e).recipients == Set(1),
      not(to_closer.notes.get(e).pending_holder),
      after(closed, 2, 1, CSay({ at: W1, to: SayNone })).notes == Map(),
      // session 1 falls silent at 4 (PRESENCE_SILENCE after its last call at 1)
      val later = settle(closed, 5, MAX_FIRINGS).state
      and {
        later.sessions.get(1).silent,
        closer(later, W1, 2) == -1,
        after(later, 5, 2, CSay({ at: W1, to: SayNone })).notes == Map(),
      },
      // a node the server closed has no closer
      match after(SEED, 20, 1, CView(W1)).nodes.get(OCC1).life { | Closed(c) => c.by == -1 | Open => true },
    })
  }

  /// 1.12 (§10.3): a release with `settles` records, after itself (its checkpoint is entry 3), the
  /// decision `talk decide {at, text: done, settles}` would: it settles a listed request that has its
  /// answer, keeps one that has none listed, and names its decision; without `settles` it records
  /// none; a request that does not exist is `not_found`, after the token.
  run releaseSettlesTest = {
    val req = after(SEED, 1, 1, CRequest({ at: W1, origin: Msg(("c1", 1)), where_key: SomeKey(Conv("c1")) }))
    val held = after(req, 1, 1, take_cmd(W1))
    val answered = after(held, 1, 1, CSaid({ at: W1, kind: SResult, where_key: NoKey }))    // said 2
    val r = call(answered, 2, 1, CRelease({ token: (W1, 1), reason: Blocked, wait: WaExternal(9), settles: Set(1) }))
    val unanswered = call(held, 2, 1, CRelease({ token: (W1, 1), reason: Blocked, wait: WaNone, settles: Set(1) }))
    val bare = call(answered, 2, 1, CRelease({ token: (W1, 1), reason: Blocked, wait: WaNone, settles: Set() }))
    assert(and {
      // the checkpoint entry 3, then the decision 4
      r.outcome == OutOk([4]),
      r.state.entries.get(4).ty == EDecision,
      r.state.entries.get(4).by == 1,
      r.state.decisions.get(4) == { node: W1, settles: Set(1), state: DActive, basis: [] },
      r.state.requests.get(1).settled == SettledBy({ decision: 4, answer: 2 }),
      r.state.holds == Map(),
      r.state.nodes.get(W1).wait != NoWait,
      // with no answer yet, the decision lists it, and its answer settles it later
      unanswered.outcome == OutOk([3]),
      unanswered.state.requests.get(1).settled == Unsettled,
      after(unanswered.state, 3, 2, CSaid({ at: W1, kind: SReply, where_key: NoKey })).requests.get(1).settled
        == SettledBy({ decision: 3, answer: 4 }),
      // without `settles`, no decision
      bare.outcome == OutOk([]),
      bare.state.decisions == answered.decisions,
      bare.state.requests.get(1).settled == Unsettled,
      out(answered, 2, 1, CRelease({ token: (W1, 1), reason: Blocked, wait: WaNone, settles: Set(99) })) == nfo(WhatRequest),
      out(answered, 2, 1, CRelease({ token: (W1, 7), reason: Blocked, wait: WaNone, settles: Set(99) })) == err(ErrStaleToken),
    })
  }

  /// 1.12, where groups B and C meet: a `handoff` release with `settles` records its decision after
  /// itself, which settles the answered request, and keeps its node from its releaser
  /// (`handed_off`) while another session gets it: the decision after the release changes neither.
  run handoffSettlesTest = {
    val req = after(SEED, 1, 1, CRequest({ at: W1, origin: Msg(("c1", 1)), where_key: SomeKey(Conv("c1")) }))
    val held = after(req, 1, 1, take_cmd(W1))
    val answered = after(held, 1, 1, CSaid({ at: W1, kind: SResult, where_key: NoKey }))    // said 2
    val r = call(answered, 2, 1, CRelease({ token: (W1, 1), reason: Handoff, wait: WaNone, settles: Set(1) }))
    assert(and {
      // the checkpoint entry 3, then the decision 4
      r.outcome == OutOk([4]),
      r.state.requests.get(1).settled == SettledBy({ decision: 4, answer: 2 }),
      r.state.sessions.get(1).handed_off == { node: W1, fence: 1, until: 2 + LEASE_FIRST },
      handed_back(r.state, 1, W1, 2),
      best_candidate(CFG, r.state, 1, "a", 2).id != W1,
      best_candidate(CFG, r.state, 2, "b", 2).id == W1,
    })
  }

  /// 1.12 (§9.3): a retract of an open question to a person withdraws it: its waits resolve, its
  /// recheck leaves the queue, and the recheck marks it set leave; a retract of another entry, or of
  /// an answered question, withdraws nothing.
  run withdrawTest = {
    val asked = after(SEED, 1, 1, CAskPerson({ at: W1, blocking: true, again: false }))     // said 1, question 2
    val waits = after(after(asked, 1, 2, take_cmd(W2)), 2, 2, CRelease({ token: (W2, 1), reason: Blocked, wait: WaQuestion(2), settles: Set() }))
    val marked = { ...waits, nodes: waits.nodes.setBy(W1, nd => { ...nd, recheck_marks: Set(2) }) }
    val r = call(marked, 3, 1, CRetract(TEntry(2)))
    val st = r.state
    assert(and {
      // the release's checkpoint is entry 3, the retract 4
      r.outcome == OutOk([4]),
      st.pquestions.get(2).state == PqWithdrawn(4),
      st.nodes.get(W1).recheck_marks == Set(),
      not(queue(st).exists(k => k.kind == K_RECHECK and k.owner == owner_entry(2))),
      match st.nodes.get(W2).wait { | HasWait(w) => resolved(st, w, 3) | NoWait => false },
      // past every recheck, nothing escalates
      settle(st, 40, MAX_FIRINGS).state.uquestions == st.uquestions,
      // a late answer is recorded and changes nothing; the retract of its said withdraws nothing
      after(st, 4, 1, CHeard({ at: W1, answers: 2 })).pquestions.get(2).state == PqWithdrawn(4),
      after(asked, 2, 1, CRetract(TEntry(1))).pquestions.get(2).state == PqOpen,
      after(after(asked, 2, 2, CHeard({ at: W1, answers: 2 })), 3, 1, CRetract(TEntry(2))).pquestions.get(2).state == PqAnswered(3),
    })
  }

  /// 1.12 (§13): `holder:` of a closed node reaches the session that closed it while it is live and
  /// is not the sender, and nobody otherwise, never pending.
  run holderClosedTest = {
    val closed = after(after(SEED, 1, 1, take_cmd(W1)), 1, 1, close_cmd(W1, Done))
    val e = closed.next_eid
    val to_closer = after(closed, 2, 2, CSay({ at: W1, to: SayHolder(W1) }))
    val own = after(closed, 2, 1, CSay({ at: W1, to: SayHolder(W1) }))
    val later = settle(closed, 5, MAX_FIRINGS).state
    val silent = after(later, 5, 2, CSay({ at: W1, to: SayHolder(W1) }))
    assert(and {
      to_closer.notes.get(e) == { node: W1, to: ToHolder(W1), until: 2 + DELIVER_WINDOW, recipients: Set(1),
                                  pending_holder: false, read_by: Set() },
      own.notes.get(e).recipients == Set(),
      not(own.notes.get(e).pending_holder),
      later.sessions.get(1).silent,
      silent.notes.get(silent.next_eid - 1).recipients == Set(),
      not(silent.notes.get(silent.next_eid - 1).pending_holder),
      // at an open node nobody holds, still its next holder
      after(SEED, 1, 2, CSay({ at: W2, to: SayHolder(W2) })).notes.get(1).pending_holder,
    })
  }

  /// 1.12 (§16): `node reopen` tags its node `offer` unless it is a container; a take that reopens
  /// a node adds none.
  run reopenOfferTest = {
    val closed = after(SEED, 1, 1, close_cmd(W1, Done))
    val r = after(closed, 2, 1, CReopen(W1))
    val ser = after(after(SEED, 1, 1, close_cmd(SER, Dropped)), 2, 1, CReopen(SER))
    assert(and {
      not(closed.nodes.get(W1).tags.contains(OFFER)),
      r.nodes.get(W1).tags.contains(OFFER),
      not(ser.nodes.get(SER).tags.contains(OFFER)),
    })
  }

  /// 1.8 (§10.2): `hold check {target, intent_done}` by another session, without a hold, closes the
  /// node's open intent with a `checkpoint` entry and counts for no counter; with none open it is
  /// `invalid(NoIntent)`.
  run checkIntentTest = {
    val held = after(SEED, 1, 1, take_cmd(W1))
    val intent = after(held, 1, 1, CCheckpoint({ token: (W1, 1), intent: true, intent_done: false }))
    val checked = call(intent, 2, 2, CCheckIntent(W1))
    assert(and {
      intent.nodes.get(W1).open_intent == { eid: 1, session: 1 },
      checked.outcome == OutOk([2, 1]),
      checked.state.nodes.get(W1).open_intent == NO_INTENT,
      checked.state.nodes.get(W1).seq == intent.nodes.get(W1).seq + 1,
      checked.state.nodes.get(W1).checkpoints == intent.nodes.get(W1).checkpoints,
      checked.state.holds == intent.holds,
      out(SEED, 1, 1, CCheckIntent(W1)) == why(WhyNoIntent),
      out(SEED, 1, 1, CCheckIntent(99)) == nfo(WhatNode),
      long_rejected(intent, 2, 2, CCheckIntent(W1)),
    })
  }

  /// 1.8 (§14, §10.4, §17): an offer untouched for `offer.stale` carries `stale` from its Stale
  /// deadline on, and comes after the rest of its tier; a note touches it again.
  run staleTest = {
    val noted = after(SEED, 5, 1, CSay({ at: W2, to: SayNone }))
    val live = live_at(noted, 13)
    val st = settle(live, 14, 40).state
    assert(and {
      SEED.nodes.get(W1).touched == 0,
      noted.nodes.get(W2).touched == 5,
      queue(live).contains({ at: 14, kind: K_STALE, owner: owner_node(W1) }),
      not(f_stale(live, W1, 13)),
      f_stale(st, W1, 14),
      st.nodes.get(W1).stale_fired,
      not(f_stale(st, W2, 14)),
      tier_of(st, W1, flags_of(st, W1, 14)) == tier_of(st, W2, flags_of(st, W2, 14)),
      // both offers in one tier: the stale one comes after the other, whatever their ids
      best_candidate(CFG, st, 1, "a", 14).id == W2,
      // a note touches it, and the flag clears
      not(f_stale(after(st, 14, 1, CSay({ at: W1, to: SayNone })), W1, 14)),
    })
  }

  // ================================================================== control and view (D.8.21)

  /// `control pause` and `unpause` (D.8.21, P1): pausing again renews and changes the source; anyone
  /// may lift it.
  run pauseTest = {
    val p = call(SEED, 1, 1, CPause(W1))
    val renewed = after(p.state, 3, 2, CPause(W1))
    assert(and {
      p.outcome == OutOk([1 + PAUSE_TTL]),
      p.state.pauses == Map(W1 -> { until: 1 + PAUSE_TTL, last_renewed: 1, ttl: PAUSE_TTL, by: 1, class: "a" }),
      p.state.nodes.get(W1).seq == 1,
      renewed.pauses.get(W1) == { until: 3 + PAUSE_TTL, last_renewed: 3, ttl: PAUSE_TTL, by: 2, class: "b" },
      after(p.state, 2, 2, CUnpause(W1)).pauses == Map(),
      out(SEED, 1, 1, CUnpause(W1)) == nfo(WhatPause),
      out(SEED, 1, 1, CPause(99)) == nfo(WhatNode),
      long_rejected(SEED, 1, 1, CPause(W1)),
    })
  }

  /// `view node` marks the node shown up to its `seq` when that raises it (CS-14, A-85).
  run viewTest = {
    val ob = after(SEED, 1, 2, CObject(W1))
    val v = after(ob, 1, 1, CView(W1))
    assert(and {
      v.sessions.get(1).shown == Map(W1 -> 1),
      call(v, 1, 1, CView(W1)) == { state: v, outcome: OutOk([]) },
      out(SEED, 1, 1, CView(99)) == nfo(WhatNode),
    })
  }


  // ================================================================== cases a mutation check asked for

  /// `take` checks `paused` before `holds_limit` (D.8.10, D.8.24: resources in that order).
  run takeOrderTest = {
    val st = after(after(after(SEED, 1, 1, create_cmd(W1, "a1", Set(), false, 1)), 1, 2, CPause(12)), 1, 1, take_cmd(W1))
    assert(and {
      out(after(st, 1, 1, take_cmd(W2)), 1, 1, take_cmd(12)) == err(ErrPaused),
      out(after(st, 1, 1, take_cmd(W2)), 1, 1, take_cmd(SER)) == err(ErrHoldsLimit),
    })
  }

  /// A closed node with a flag but capacity 0 is no candidate (D.8.5: `capacity > 0`).
  run nextClosedCapacityTest = {
    val req = after(SEED, 1, 1, CRequest({ at: W1, origin: Conv("c1"), where_key: NoKey }))
    val zero = after(after(req, 1, 1, CUpdate({ ...update_none(W1, 1), capacity: 0 })), 1, 1, close_cmd(W1, Done))
    assert(and {
      flags_of(zero, W1, 1).contains(FUnansweredRequest),
      out(zero, 1, 1, CNext) == OutOk([0, W2, 1]),
      out(after(req, 1, 1, close_cmd(W1, Done)), 1, 1, CNext) == OutOk([0, W1, 1]),
    })
  }

  /// A `said` reply answers the requests from people and agents only: a user request on the same
  /// node waits for a `to_user` of kind `result` (§9.4, A-84, CS-4).
  run saidUserRequestTest = {
    val user_req = after(SEED, 1, 1, CFromUser({ principal: "user", answers: -1, clarifies: -1, node: W1 }))   // request 2
    val people_req = after(user_req, 1, 1, CRequest({ at: W1, origin: Msg(("c1", 1)), where_key: SomeKey(Conv("c1")) }))   // request 3
    val rep = after(people_req, 2, 1, CSaid({ at: W1, kind: SReply, where_key: NoKey }))
    assert(and {
      rep.requests.get(2).answered_by == -1,
      rep.requests.get(3).answered_by == 4,
    })
  }

  /// An answer clears the marks its question set (D.8.15 `answer_pq`).
  run answerClearsMarksTest = {
    val asked = after(SEED, 1, 1, CAskPerson({ at: W1, blocking: false, again: false }))   // said 1, question 2
    val marked = { ...asked, nodes: asked.nodes.setBy(W1, nd => { ...nd, recheck_marks: Set(2, 7), assume_marks: Set(2) }) }
    val h = after(marked, 2, 2, CHeard({ at: W1, answers: 2 }))
    assert(and {
      h.nodes.get(W1).recheck_marks == Set(7),
      h.nodes.get(W1).assume_marks == Set(),
    })
  }

  // ================================================================== readings recorded for a decision (README)

  /// D.8.9 and D.8.11: `node key {add}` of an unrouted user key on a closed node emits `KeyAdded`,
  /// then `route_to`, whose "if no open node carries `user:e<e>`, `KeyAdded`" would add the key a
  /// second time, a `KeyAdded` that fails its precondition (an anomaly in Rust). The model adds it
  /// once: the node's version rises by one, and the request is recorded there.
  run routeToClosedNodeTest = {
    val entry = after(after(SEED, 1, 1, CFromUser({ principal: "user", answers: -1, clarifies: -1, node: -1 })), 1, 1, close_cmd(W1, Done))
    val st = after(entry, 2, 1, CKeyAdd({ node: W1, key: User(1) }))
    assert(and {
      is_closed(entry, W1),
      st.nodes.get(W1).version == entry.nodes.get(W1).version + 1,
      st.nodes.get(W1).keys == Set(Conv("c1"), User(1)),
      // eid 2 is the close's system entry
      st.requests.get(3).node == W1,
      st.user_entries.get(1).routing == RtRequest({ node: W1, request: 3 }),
    })
  }

  /// D.8.24 and the command sections disagree on the order of two checks (README, "Open questions",
  /// item 10): D.8.24's general order puts the other references (`not_found`, step 5) before the
  /// `invalid` checks by state (step 7), while D.8.11 checks `answers` (NotUserQuestion) before
  /// `node` resolves, and D.8.20 checks ReservedNode before the new parent. The model follows the
  /// command sections; conformance compares the codes, so which reading holds is still open.
  run validationOrderTest = {
    val asked = after(SEED, 1, 1, CAskPerson({ at: W1, blocking: false, again: false }))   // question 2
    assert(and {
      out(asked, 1, 1, CFromUser({ principal: "", answers: 2, clarifies: -1, node: 99 })) == why(WhyNotUserQuestion),
      out(SEED, 1, 1, CMove({ node: ROOT, parent: 99, name: Lit("a1") })) == why(WhyReservedNode),
    })
  }

  /// §28 and D.10.7 bound the model at three occurrences of the one series, and `within_bounds`
  /// disables a command that would make a fourth. Two `update {repeat}` calls that each drop a
  /// waiting occurrence and reslot from now stay within it (two occurrences made, then three), and
  /// time alone then opens two more by `missed` (slots 15 and 30 pass): five in all by MAX_TIME. The
  /// guard cannot see the occurrences time will open (README, "Open questions").
  run occurrenceBoundTest = {
    val u1 = after(SEED, 1, 1, CUpdate({ ...update_none(SER, 1), repeat: RepSet(ALT_REPEAT) }))
    val u2 = after(u1, 2, 1, CUpdate({ ...update_none(SER, 1), repeat: RepSet(SEED_REPEAT) }))
    val late = settle(u2, MAX_TIME, 40)
    assert(and {
      occurrences_made(u1).size() == 2,
      within_bounds(2, SEED, u1),
      occurrences_made(u2).size() == 3,
      within_bounds(2, u1, u2),
      not(late.saturated),
      occurrences_made(late.state).size() == 5,
    })
  }

  // ================================================================== §32, step by step (WP-A4)

  // The example of §32, step by step, as D.12.2 scripts it for the core: the calls of each step,
  // the reply each call must get, and D.12.2's assertions after it. The script is data
  // (`Step32`), grouped in named parts (PARTS_32). `scenario_32` plays it through actions, as a
  // replay does (D.10.8), with each of D.12.2's advances played one tick at a time, and checks at
  // the end of every part that it is where the pure fold of the script (`WORLDS_32`, with the
  // advances as single jumps) is; `test_T2` checks T2 on every state of that fold.
  //
  // How the example maps onto the model:
  // - Sessions are new sessions, opened by `hello` in the order of the example: the assistant is
  //   #3, s-1 to s-7 are #4 to #10, and the coding agent is #11. Classes: `a` for chat-assistant,
  //   `b` for chat-worker, and `w` for coding-agent (CLASS_NAMES, `params.qnt`).
  // - The seed of D.10.7 comes first. Seed session 1 closes /w1 and /w2 as `dropped`, so that
  //   §32's two top-level nodes fit under ROOT beside the seed's series /s within the scaled
  //   `tree.max_children` (3, D.10.5). /s stays open and quiet: its first slot (15) lies after
  //   the example's last call (at 11).
  // - `hello {from_user}` is `hello`, then `talk from_user` (the model's `hello` carries no
  //   message, D.10.4). `node create {path: digest/daily-tickets}` is two creates, because the
  //   model's `create` names an existing parent: `digest` stands for the missing ancestor.
  //   `node create {request}` is `create`, then `talk request` with `where` the thread's key,
  //   the default of a create's request (§20.5, A-61, D.8.9). The model's `create` has no
  //   `request`, so the script passes that default instead of the server computing it: step 2's
  //   `conv: THREAD` holds by construction, and the default itself is left to WP-D6's core
  //   scenario. Without `where`, `talk request` would take case 3 and make the message a key of
  //   summary-gap (§9.4), which `bare_request_case_3` shows, so the split needs the explicit
  //   `where`.
  // - The thread `chat:team/thread-1` is `Conv("t1")`, and the person's message
  //   `chat:team/thread-1/1001` is `Msg(("t1", 1))`.
  // - The schedule `0 7 * * *` in Europe/Berlin with `run_within: 2h` is DAILY_REPEAT: every 30
  //   ticks in UTC, `run_within` 7 ticks (2 h, D.10.5), because the model's schedules are periods
  //   that divide 60 (D.10.6). Its first slot, 30, lies after the example's end, so its
  //   occurrence waits throughout, as D.12.2's `20261002T0500Z` does.
  // - Time: steps 1 to 4 are at tick 0, D.12.2's 09:00Z (in a replay, T0 of D.10.5: only
  //   differences of time matter here). Step 4's "advance 2 h" is 7 ticks, the 2 h of D.10.5's
  //   table: past `commitment.followup` (90 min, 6) and before `question.recheck` (4 h, 8). Step
  //   6's "advance 21 min" is 4 ticks: the checkpoint extended the lease to `lease.default` (20
  //   min, 4 ticks) after it, and a hold expires when `now >= lease_until`. 21 minutes lies
  //   between 20 min (4 ticks) and 1 h (5 ticks), and no tick lies between those two.
  // - The scaling keeps the order of each single duration, not of their sums (D.10.5): the two
  //   advances, 2 h 21 min in all, are 7 + 4 = 11 ticks, past `session.retain` (24 h, 10 ticks).
  //   So the six sessions last touched at tick 0 (the seed's two, the assistant, s-1, s-2, and
  //   s-3) retire at tick 10, within the second advance, and only s-4 and s-5 are left when s-6
  //   arrives (`after_6_vanish`). D.12.2's run ends at 11:21 and retires none: from step 6 on, its
  //   nine sessions are all present, silent. No assertion of D.12.2 reads `sessions`, and a
  //   comparison with WP-D6's run on real params must leave `sessions` out from step 6 on (README,
  //   "Open questions", item 15).
  // - Not modelled, and so asserted only through the state they report: warnings (the four
  //   warnings of a close by their conditions, `close_clean`; `moved` and `conversation_held` of
  //   s-3's ask by `moved_of` and `conversation_held_of` on the state before it, D.8.15), the
  //   join's `open_question` (the open question on the node), `view requests` (the requests and
  //   entries the board reads), and `leave_better` (`leave_better_of`, D.8.7, a query the model
  //   does not compute). Texts, briefs, `verified`, and `uncovered_slot` have no state.
  // - `aged`: within MAX_TIME the model's `aged` is 0 (D.10.5), so step 6's ranking checks only
  //   that summary-gap and rerun-test are both tier-0 candidates and that the tie-break puts
  //   summary-gap first (by id, USE_FNV = false, and by FNV, as `two_class` breaks ties). D.12.2's
  //   reason, that summary-gap is older (Rust's `aged` is 2 against 0 at 11:21), is left to
  //   WP-D6's core run on the real clock.
  // - The checks after every call and every tick are `inv_all` (`forum.qnt`), every check of
  //   D.9.2 that the model makes (WP-A3, `invariants.qnt`), T2 and the step checks included, and
  //   `recorded_pending`. The states between two firings due at the same tick are not settled, so
  //   no state invariant applies to them; D.12.2's check after every firing is Rust's
  //   `check_fire`, which `inv_all`'s step checks replay firing by firing after every tick.

  /// One call of the script: the action's name as D.10.4 names it, the session, the class of a
  /// `hello` (any other call takes its session's class), the command, and the outcome it must get.
  type Call32 = { name: str, sess: int, cls: str, cmd: Cmd, out: CmdOutcome }

  /// A step of the script: a call, or the clock moving `dt` ticks with everything due firing.
  type Step32 = Do32(Call32) | Advance32(int)

  /// Where a script has got to: the state and the clock (D.10.2).
  type World = { st: State, clock: int }

  // Sessions, in the order the example opens them (the seed's are 1 and 2).
  pure val ASSISTANT = 3
  pure val S_1 = 4
  pure val S_2 = 5
  pure val S_3 = 6
  pure val S_4 = 7
  pure val S_5 = 8
  pure val S_6 = 9
  pure val S_7 = 10
  pure val CODER = 11
  pure val CHAT_ASSISTANT = "a"
  pure val CHAT_WORKER = "b"
  pure val CODING_AGENT = "w"

  // Nodes.
  pure val DIGEST_DIR = 12   // digest
  pure val DAILY = 13        // digest/daily-tickets
  pure val MORNING = 14      // its waiting occurrence
  pure val GAP = 15          // summary-gap
  pure val AUDIT = 16        // summary-gap/audit
  pure val RERUN = 17        // summary-gap/rerun-test

  // Entries. Eids 1 and 2 are the system entries of the setup's closes; 10 and 13 are the
  // checkpoints of s-1's and s-3's releases, 16 is s-6's checkpoint, and 17, 19, and 22 are the
  // system entries of the three closes. None of these keeps metadata (CS-17).
  pure val E_1 = 3           // the user's first message
  pure val R_1 = 4           // its request on the series
  pure val DEC_1 = 5         // the assistant's decision, settling r1
  pure val REPORT_1 = 6      // the assistant's `to_user` result
  pure val CLARIFY = 7       // "and skip tickets tagged spam"
  pure val R_2 = 8           // the person's request
  pure val ACK = 9           // s-1's ack ("looking"), and the commitment it opens
  pure val Q_SAID = 11       // the `said` of s-3's question
  pure val Q_1 = 12          // s-3's question to the person
  pure val HEARD = 14        // the person's answer
  pure val INTENT = 15       // s-5's checkpoint with the intent
  pure val DEC_AUDIT = 18    // the decision that audit closes with
  pure val DEC_2 = 20        // s-7's decision, settling r2
  pure val RESULT = 21       // s-7's result
  pure val E_8 = 23          // "what about my tasks?"
  pure val REPORT_8 = 24     // the coding agent's report

  pure val THREAD: Key = Conv("t1")
  pure val MESSAGE: Key = Msg(("t1", 1))

  /// 2 h in ticks: `lease.max` and `pause.ttl` in D.10.5's table.
  pure val TWO_HOURS = LEASE_MAX

  /// "Every morning" (see above): period 30 in UTC, `run_within` 2 h.
  pure val DAILY_REPEAT: RepeatSpec = { period: 30, offset: 0, run_within: TWO_HOURS }

  /// The times of the calls: steps 1 to 4 at 0, then 2 h later, then 21 minutes later, when the
  /// lease that s-5's checkpoint extended by `lease.default` has run out (see above).
  pure val T_ASKED = 0
  pure val T_ANSWERED = T_ASKED + TWO_HOURS                // "advance 2 h"
  pure val T_TAKEN_OVER = T_ANSWERED + LEASE_DEFAULT       // "advance 21 min"

  // ------------------------------------------------------------------ playing the script

  /// `command` of `forum.qnt` without D.10.7's bounds, with the reply checked, and with the session
  /// live: the example opens nine sessions (the bounds allow four), records three user entries
  /// (two), and with its setup makes 24 entries (MAX_EID is 16). The bounds keep model checking
  /// finite; a fixed script needs none. Every call comes from a session that has never fallen
  /// silent, which is §32's "no session came back". `last_action` is what `command` records, so a
  /// replay reads the call from it (D.10.8): `quint test --out-itf` writes the state variables
  /// only, with no `mbt::actionTaken`. Like `command`, it ends with D.8.1's post-settle at `now`.
  action call32(c: Call32): bool = {
    val now = imax(clock, s.last_now)
    val cls = if (has_session(s, c.sess)) s.sessions.get(c.sess).class else c.cls
    val r = run_cmd(CFG, s, now, cls, c.sess, "", false, c.cmd)
    val p = settle(r.state, now, MAX_FIRINGS)
    all {
      assert(settled(s, now)),
      assert(c.name == "act_hello" or (has_session(s, c.sess) and not(s.sessions.get(c.sess).silent))),
      assert(r.outcome == c.out),
      assert(not(p.saturated) and not(p.stuck)),
      s' = p.state,
      clock' = clock,
      prev' = s,
      mid' = r.state,
      last_action' = { name: c.name, sess: c.sess, class: cls, op: "", long: false, cmd: c.cmd, outcome: r.outcome,
                       settle: { saturated: p.saturated, stuck: p.stuck } },
    }
  }

  /// `act_tick` with a given `dt` (D.10.4): the clock moves, and everything due fires. The run
  /// plays D.12.2's advances of 7 and 4 ticks one tick at a time (`in_ticks`), within `act_tick`'s
  /// picks (1 to 3), so that its checks see the settled state at every tick, each firing time
  /// among them; the pure fold takes each advance as one jump, and by T2 both end in the same
  /// state, which `part32` checks and `test_T2` checks on every state of the fold.
  action tick32(dt: int): bool = {
    val r = settle(s, clock + dt, MAX_FIRINGS)
    all {
      assert(dt >= 1 and clock + dt <= MAX_TIME),
      assert(not(r.saturated) and not(r.stuck)),
      s' = r.state,
      clock' = clock + dt,
      prev' = s,
      mid' = s,
      last_action' = { name: "act_tick", sess: -1, class: "", op: "", long: false, cmd: CTick(dt),
                       outcome: OutNone, settle: { saturated: r.saturated, stuck: r.stuck } },
    }
  }

  action play32(x: Step32): bool = match x { | Do32(c) => call32(c) | Advance32(dt) => tick32(dt) }

  /// The script as the run plays it: every advance of `dt` ticks as `dt` advances of one.
  pure def in_ticks(xs: List[Step32]): List[Step32] =
    xs.foldl([], (acc, x) => match x {
      | Advance32(dt) => acc.concat(range(0, dt).foldl([], (ts, i) => ts.append(Advance32(1))))
      | Do32(_) => acc.append(x)
    })

  /// `play32` as a pure function on a world: for a call, the same expressions as `call32` on the
  /// same arguments, so that the run and the fold differ only in how they take an advance.
  pure def play_pure(w: World, x: Step32): World =
    match x {
      | Do32(c) => {
          val cls = if (has_session(w.st, c.sess)) w.st.sessions.get(c.sess).class else c.cls
          val now = imax(w.clock, w.st.last_now)
          { st: settle(run_cmd(CFG, w.st, now, cls, c.sess, "", false, c.cmd).state, now, MAX_FIRINGS).state, clock: w.clock }
        }
      | Advance32(dt) => { st: settle(w.st, w.clock + dt, MAX_FIRINGS).state, clock: w.clock + dt }
    }

  /// Every world a script passes through, from `w` (`w` first).
  pure def worlds_of(w: World, xs: List[Step32]): List[World] =
    xs.foldl([w], (acc, x) => acc.append(play_pure(acc[acc.length() - 1], x)))

  // ------------------------------------------------------------------ the calls

  pure def by32(sess: int, name: str, cmd: Cmd, ids: List[int]): Step32 =
    Do32({ name: name, sess: sess, cls: "", cmd: cmd, out: OutOk(ids) })

  pure def hello32(cls: str, sess: int): Step32 =
    Do32({ name: "act_hello", sess: -1, cls: cls, cmd: CHello({ caps: Set(), principal: "" }), out: OutOk([sess]) })

  pure def create32(parent: int, name: str, ks: Set[Key], take: bool, tags: Set[str], rep: RepeatChange): Cmd =
    CCreate({ parent: parent, name: Lit(name), keys: ks, take: take, capacity: 1, owners: Set(), repeat: rep, tags: tags })

  /// A message from the user, routed by nothing in the call; the client names nobody, so the call
  /// passes `principal: user` (§32, 1.9: a session that serves none names its principal).
  pure val LOG_32: Cmd = CFromUser({ principal: "user", answers: -1, clarifies: -1, node: -1 })

  /// `node create {path: summary-gap, keys: [chat:team/thread-1]}` (take by default), and its
  /// request: one message, one origin (A-61).
  pure val CREATE_GAP: Cmd = create32(ROOT, "summary-gap", Set(THREAD), true, Set(), RepKeep)
  pure val REQUEST_GAP: Cmd = CRequest({ at: GAP, origin: MESSAGE, where_key: SomeKey(THREAD) })

  pure def close32(n: int): Cmd = CClose({ node: n, outcome: Done, settles: Set(), token: NoToken })

  pure def decide32(n: int, rs: Set[int]): Cmd = CDecide({ at: n, settles: rs, supersedes: -1, basis: [] })

  /// Setup: the seed's /w1 and /w2 are closed, so that ROOT has room for §32's two top-level nodes.
  pure val SETUP_32: List[Step32] = [
    by32(1, "act_close", CClose({ node: W1, outcome: Dropped, settles: Set(), token: NoToken }), [-1]),
    by32(1, "act_close", CClose({ node: W2, outcome: Dropped, settles: Set(), token: NoToken }), [-1]),
  ]

  /// Step 1. The assistant logs the user's message first (§8.1, constitution line 2).
  pure val STEP_1_LOG: List[Step32] = [
    hello32(CHAT_ASSISTANT, ASSISTANT),
    by32(ASSISTANT, "act_from_user", LOG_32, [E_1, USER_PRINCIPAL]),
  ]

  /// Step 1. It creates `digest/daily-tickets` with the key `user:e1` (and the default hold),
  /// which routes `e1` as the request r1.
  pure val STEP_1_CREATE: List[Step32] = [
    by32(ASSISTANT, "act_create", create32(ROOT, "digest", Set(), false, Set(), RepKeep), [DIGEST_DIR, -1]),
    by32(ASSISTANT, "act_create", create32(DIGEST_DIR, "daily-tickets", Set(User(E_1)), true, Set(), RepSet(DAILY_REPEAT)), [DAILY, 1]),
  ]

  /// Step 1. `decide {settles: [r1]}`, and the report `to_user {kind: result, re: [e1]}`.
  pure val STEP_1_REPORT: List[Step32] = [
    by32(ASSISTANT, "act_decide", decide32(DAILY, Set(R_1)), [DEC_1]),
    by32(ASSISTANT, "act_to_user", CToUser({ principal: "", kind: TuResult, re: Set(E_1) }), [REPORT_1, USER_PRINCIPAL]),
  ]

  /// Step 1. "And skip tickets tagged spam": `from_user {clarifies: r1}`.
  pure val STEP_1_CLARIFY: List[Step32] = [
    by32(ASSISTANT, "act_from_user", CFromUser({ principal: "", answers: -1, clarifies: R_1, node: -1 }), [CLARIFY, USER_PRINCIPAL]),
  ]

  /// Step 2. Two agent runs start on the person's message; the first creates summary-gap with the
  /// thread's key and records the request, with the message as its origin.
  pure val STEP_2_FIRST: List[Step32] = [
    hello32(CHAT_WORKER, S_1),
    hello32(CHAT_WORKER, S_2),
    by32(S_1, "act_create", CREATE_GAP, [GAP, 1]),
    by32(S_1, "act_request", REQUEST_GAP, [R_2, GAP]),
  ]

  /// Step 2. The second gets `{existing}` and records the same message, so the same request (Q1),
  /// and leaves. Neither call appends anything.
  pure val STEP_2_SECOND: List[Step32] = [
    by32(S_2, "act_create", CREATE_GAP, [GAP]),
    by32(S_2, "act_request", REQUEST_GAP, [R_2, GAP]),
  ]

  /// Step 3. s-1 records `said ack` ("looking"), confirms it, splits the work into `audit` and
  /// `rerun-test` (`take: false`, tagged `offer`), and releases summary-gap.
  pure val STEP_3: List[Step32] = [
    by32(S_1, "act_said", CSaid({ at: GAP, kind: SAck, where_key: NoKey }), [ACK, GAP]),
    by32(S_1, "act_confirm", CConfirm(ACK), [ACK]),
    by32(S_1, "act_create", create32(GAP, "audit", Set(), false, Set(OFFER), RepKeep), [AUDIT, -1]),
    by32(S_1, "act_create", create32(GAP, "rerun-test", Set(), false, Set(OFFER), RepKeep), [RERUN, -1]),
    by32(S_1, "act_release", CRelease({ token: (GAP, 1), reason: Handoff, wait: WaNone, settles: Set() }), []),
  ]

  /// Step 4. s-3 takes `audit` with `hold take`.
  pure val STEP_4_TAKE: List[Step32] = [
    hello32(CHAT_WORKER, S_3),
    by32(S_3, "act_take", CTake({ node: AUDIT, lease: -1 }), [AUDIT, 1]),
  ]

  /// Step 4. s-3 asks in the thread, and releases with `waits_for: {question}`.
  pure val STEP_4_ASK: List[Step32] = [
    by32(S_3, "act_ask_person", CAskPerson({ at: AUDIT, blocking: false, again: false }), [Q_1, Q_SAID, GAP]),
    by32(S_3, "act_release", CRelease({ token: (AUDIT, 1), reason: Blocked, wait: WaQuestion(Q_1), settles: Set() }), []),
  ]

  /// Step 4. Two hours pass.
  pure val STEP_4_LATER: List[Step32] = [Advance32(T_ANSWERED - T_ASKED)]

  /// Step 5. The person answers; s-4 runs on that event and gets `{existing}`.
  pure val STEP_5_JOIN: List[Step32] = [
    hello32(CHAT_WORKER, S_4),
    by32(S_4, "act_create", CREATE_GAP, [GAP]),
  ]

  /// Step 5. s-4 records `heard {answers}`.
  pure val STEP_5_HEARD: List[Step32] = [
    by32(S_4, "act_heard", CHeard({ at: GAP, answers: Q_1 }), [HEARD, GAP, Q_1]),
  ]

  /// Step 6. s-5 takes `rerun-test` with `hold take`, checkpoints an intent, and disappears; 21
  /// minutes pass.
  pure val STEP_6_VANISH: List[Step32] = [
    hello32(CHAT_WORKER, S_5),
    by32(S_5, "act_take", CTake({ node: RERUN, lease: -1 }), [RERUN, 1]),
    by32(S_5, "act_checkpoint", CCheckpoint({ token: (RERUN, 1), intent: true, intent_done: false }), [T_ANSWERED + LEASE_DEFAULT]),
    Advance32(T_TAKEN_OVER - T_ANSWERED),
  ]

  /// Step 6. s-6 arrives (and looks at the ranking, which the run asserts).
  pure val STEP_6_ARRIVE: List[Step32] = [hello32(CHAT_WORKER, S_6)]

  /// Step 6. s-6 takes `rerun-test` with `hold take`, sees `open_intent`, checks, records
  /// `intent_done`, and closes it.
  pure val STEP_6_FINISH: List[Step32] = [
    by32(S_6, "act_take", CTake({ node: RERUN, lease: -1 }), [RERUN, 2]),
    by32(S_6, "act_checkpoint", CCheckpoint({ token: (RERUN, 2), intent: false, intent_done: true }), [T_TAKEN_OVER + LEASE_DEFAULT]),
    by32(S_6, "act_close", close32(RERUN), [-1]),
  ]

  /// Step 7. `audit` closes with a decision (D.12.2: by s-6).
  pure val STEP_7_AUDIT: List[Step32] = [
    by32(S_6, "act_take", CTake({ node: AUDIT, lease: -1 }), [AUDIT, 2]),
    by32(S_6, "act_decide", decide32(AUDIT, Set()), [DEC_AUDIT]),
    by32(S_6, "act_close", close32(AUDIT), [-1]),
  ]

  /// Step 7. s-7 records `decide {settles}`, and `said result`, which it sends and confirms.
  pure val STEP_7_ANSWER: List[Step32] = [
    hello32(CHAT_WORKER, S_7),
    by32(S_7, "act_take", CTake({ node: GAP, lease: -1 }), [GAP, 2]),
    by32(S_7, "act_decide", decide32(GAP, Set(R_2)), [DEC_2]),
    by32(S_7, "act_said", CSaid({ at: GAP, kind: SResult, where_key: NoKey }), [RESULT, GAP, R_2]),
    by32(S_7, "act_confirm", CConfirm(RESULT), [RESULT]),
  ]

  /// Step 7. s-7 closes with `close done`.
  pure val STEP_7_CLOSE: List[Step32] = [by32(S_7, "act_close", close32(GAP), [-1])]

  /// Step 8. The user asks a coding agent "what about my tasks?"; it logs the message first.
  pure val STEP_8_LOG: List[Step32] = [
    hello32(CODING_AGENT, CODER),
    by32(CODER, "act_from_user", LOG_32, [E_8, USER_PRINCIPAL]),
  ]

  /// Step 8. It replies from the board, and routes the entry to none.
  pure val STEP_8_REPLY: List[Step32] = [
    by32(CODER, "act_to_user", CToUser({ principal: "", kind: TuResult, re: Set(E_8) }), [REPORT_8, USER_PRINCIPAL]),
    by32(CODER, "act_route", CRoute({ entry: E_8, to: -1 }), [-1]),
  ]

  /// The parts of the script in order, each named after its step of D.12.2.
  pure val PARTS_32: List[(str, List[Step32])] = [
    ("setup", SETUP_32), ("1 log", STEP_1_LOG), ("1 create", STEP_1_CREATE), ("1 report", STEP_1_REPORT),
    ("1 clarify", STEP_1_CLARIFY), ("2 first", STEP_2_FIRST), ("2 second", STEP_2_SECOND), ("3", STEP_3),
    ("4 take", STEP_4_TAKE), ("4 ask", STEP_4_ASK), ("4 later", STEP_4_LATER), ("5 join", STEP_5_JOIN),
    ("5 heard", STEP_5_HEARD), ("6 vanish", STEP_6_VANISH), ("6 arrive", STEP_6_ARRIVE),
    ("6 finish", STEP_6_FINISH), ("7 audit", STEP_7_AUDIT), ("7 answer", STEP_7_ANSWER),
    ("7 close", STEP_7_CLOSE), ("8 log", STEP_8_LOG), ("8 reply", STEP_8_REPLY),
  ]

  pure val SCENARIO_32: List[Step32] = PARTS_32.foldl([], (acc, p) => acc.concat(p._2))

  /// Every world of the script, from the seed at 0, with each advance as one jump.
  pure val WORLDS_32: List[World] = worlds_of({ st: SEED, clock: 0 }, SCENARIO_32)

  /// The position of the part named `name` in PARTS_32 (-1 for none, which no lookup survives).
  pure def part_index(name: str): int =
    PARTS_32.indices().fold(-1, (acc, i) => if (PARTS_32[i]._1 == name) i else acc)

  /// The world of the pure fold at the end of the part named `name`.
  pure def fold_after(name: str): World =
    WORLDS_32[PARTS_32.slice(0, part_index(name) + 1).foldl(0, (n, p) => n + p._2.length())]

  // ------------------------------------------------------------------ what the steps assert

  /// The four warnings a close can raise (§21, D.9.2 G1 (d)) would not be raised by `sess`
  /// closing `n` in `st`: no unsettled request, no open commitment, no open objection, no hold of
  /// another session (`held_by_other`); and no open child, no other session's open intent, and no
  /// agreements (whose reminder is the `agreement` warning, §11.2).
  pure def close_clean(st: State, n: int, sess: int): bool = and {
    unsettled_on(st, n) == Set(),
    open_commitments_on(st, n) == Set(),
    open_objections_on(st, n) == Set(),
    holds_on(st, n).forall(t => st.holds.get(t).session == sess),
    open_children(st, n) == Set(),
    st.nodes.get(n).open_intent.eid < 0 or st.nodes.get(n).open_intent.session == sess,
    not(st.nodes.get(n).agreements),
  }

  /// A node `leave_better` may suggest (D.8.7): flagged, no hold in the map, a node or status
  /// candidate of `next`, and nothing holding it back.
  pure def suggestible(st: State, m: int, now: int): bool = {
    val fl = flags_of(st, m, now)
    fl != Set() and holds_on(st, m) == Set() and classify(st, m, fl, now) >= 0 and not(held_back(st, m, now))
  }

  /// `leave_better(anchor n)` of D.8.7 (§10.3 as amended at G0, A-63): the anchor's siblings, then
  /// the parent and the parent's siblings, and so on up; within a level the ancestor first, then its
  /// siblings by id; never the anchor; at most `leave_better.max`.
  pure def leave_better_of(st: State, n: int, now: int): List[int] = {
    val up = ancestors_or_self(st, n)
    val out = range(0, up.length() - 1).foldl([], (acc, i) => {
      if (acc.length() >= LEAVE_BETTER_MAX) acc
      else {
        val cur = up[i]
        val p = up[i + 1]
        val own = if (i > 0 and suggestible(st, cur, now)) [cur] else []
        val sibs = sorted(children(st, p).exclude(Set(cur)).filter(m => suggestible(st, m, now)))
        acc.concat(own).concat(sibs)
      }
    })
    if (out.length() > LEAVE_BETTER_MAX) out.slice(0, LEAVE_BETTER_MAX) else out
  }

  /// The entry types the `moved` warning lists (§9.2, D.8.15).
  pure val PEOPLE_TYPES: Set[EntryType] = Set(ERequest, ESaid, EHeard, EQuestion, EAnswer)

  /// The `moved` warning of a `said` or an `ask` by `sess` whose conversation node is `c`, on the
  /// state before the call (D.8.15, with no `seen` argument): the kept entries on `c` past the
  /// session's `shown[c]`, of the people types, by other sessions. A set: the newest-first order
  /// and the cut at MOVED_MAX are not modelled.
  pure def moved_of(st: State, sess: int, c: int): Set[int] = {
    val seen = get_or(st.sessions.get(sess).shown, c, 0)
    st.entries.keys().filter(e => {
      val x = st.entries.get(e)
      x.node == c and x.seq > seen and x.by != sess and PEOPLE_TYPES.contains(x.ty)
    })
  }

  /// The `conversation_held` warning of a `said` or an `ask` by `sess` whose conversation node is
  /// `c` (D.8.15): another session holds `c` or a descendant of it, and `sess` holds neither.
  pure def conversation_held_of(st: State, sess: int, c: int): bool = {
    val inside = st.holds.keys().filter(t => to_set(ancestors_or_self(st, t._1)).contains(c))
    inside.exists(t => st.holds.get(t).session != sess) and not(inside.exists(t => st.holds.get(t).session == sess))
  }

  /// The candidate `next` would give session `sess` now (D.8.5): its id, or -1.
  def ranked_first(sess: int): int = best_candidate(CFG, s, sess, s.sessions.get(sess).class, clock).id

  /// The same with the FNV tie-break, as `two_class` and the server break ties (CS-12, D.10.6).
  def ranked_first_fnv(sess: int): int =
    best_candidate({ ...CFG, use_fnv: true }, s, sess, s.sessions.get(sess).class, clock).id

  def flags_now(n: int): Set[Flag] = flags_of(s, n, clock)

  /// The node is a candidate of `next` as a node to take, in tier `tier` (§10.4).
  def candidate_in_tier(n: int, tier: int): bool =
    classify(s, n, flags_now(n), clock) == 0 and tier_of(s, n, flags_now(n)) == tier

  /// §9.2, "record before sending": a call that records a `said` (`said`, or `ask` to a person)
  /// leaves it pending (1.9: or withheld, a `said` while another session holds its conversation);
  /// a confirm, or a release or close by its session, confirms it later (steps 3, 4, and 7), and
  /// `recorded_pending` after every call makes each of those a checked change.
  /// 1.11 (§9.2): or confirmed, when its session holds nothing.
  pure def recorded_state(st: State, x: int, now: int): bool =
    st.saids.get(x).state == SdPending or st.saids.get(x).state == SdWithheld
    or (st.saids.get(x).state == SdConfirmed and holds_nothing(st, st.saids.get(x).session, now))

  val recorded_pending: bool = match last_action.outcome {
    | OutOk(ids) => match last_action.cmd {
        | CSaid(_) => recorded_state(s, ids[0], clock)
        | CAskPerson(_) => ids[1] < 0 or recorded_state(s, ids[1], clock)
        | _ => true
      }
    | _ => true
  }

  /// What the run checks after every call and every tick: `inv_all`, every check of D.9.2 the model
  /// makes (WP-A3), and `recorded_pending`.
  val checked32: bool = inv_all and recorded_pending

  /// Part `name` of the script as the run plays it: each step through its action, an advance one
  /// tick at a time (`in_ticks`), with `checked32` after every step; at its end the run is where
  /// the pure fold of the script is. After calls, that check only ties the run to `WORLDS_32`,
  /// which `test_T2` reads (`play_pure` evaluates what `call32` does); after an advance it is an
  /// instance of T2: the ticks end where the single jump does.
  run part32(name: str): bool = {
    val xs = in_ticks(PARTS_32[part_index(name)]._2)
    val w = fold_after(name)
    xs.length().reps(i => play32(xs[i]).expect(checked32)).expect(s == w.st and clock == w.clock)
  }

  val after_setup: bool = and {
    closed_as(s, W1, Dropped),
    closed_as(s, W2, Dropped),
    open_agent_children(s, ROOT, -1) == 1,                  // /s alone
    s.entries == Map(),
  }

  /// Step 1: `e1` is the user's (no principal named, §3), unrouted, and `@user` carries `unrouted`;
  /// the assistant now serves `user` (§8.1).
  val after_1_log: bool = and {
    s.user_entries.get(E_1) == { session: ASSISTANT, principal: USER_PRINCIPAL, at: T_ASKED, routing: Unrouted },
    s.entries.get(E_1) == { node: USER, seq: 1, at: T_ASKED, by: ASSISTANT, ty: EFromUser, principal: USER_PRINCIPAL },
    flags_now(USER) == Set(FUnrouted),
    s.sessions.get(ASSISTANT).principals == Set(USER_PRINCIPAL),
    s.sessions.get(ASSISTANT).class == CHAT_ASSISTANT,
  }

  /// Step 1: `e1` is routed to the series as the request r1, whose requester is `user`; the
  /// series has its one waiting occurrence, named after its slot (K6, §12); `@user` no longer
  /// carries `unrouted`; the assistant holds the series.
  val after_1_create: bool = and {
    s.user_entries.get(E_1).routing == RtRequest({ node: DAILY, request: R_1 }),
    s.requests.get(R_1) == {
      node: DAILY, origin: User(E_1), conv: User(E_1), from_user: E_1, requester: ReqUser(USER_PRINCIPAL),
      answered_by: -1, settled: Unsettled, moved_from: -1,
    },
    s.nodes.get(DAILY).parent == DIGEST_DIR,
    s.nodes.get(DAILY).keys == Set(User(E_1)),
    s.nodes.get(DAILY).role == Series({ repeat: DAILY_REPEAT, last_missed: false }),
    open_occurrences(s, DAILY) == Set(MORNING),
    s.nodes.get(MORNING).name == Slot(30),
    occ_waiting(s.nodes.get(MORNING)),
    s.nodes.get(MORNING).keys == Set(RepeatSlot((DAILY, 30))),
    flags_now(USER) == Set(),
    s.holds.keys() == Set((DAILY, 1)),
    s.holds.get((DAILY, 1)).session == ASSISTANT,
  }

  /// Step 1: a `to_user` of kind `result` to the requester, naming the entry, answers r1, and the
  /// decision settles it (§9.4: the order of the two does not matter).
  val after_1_report: bool = and {
    s.decisions.get(DEC_1) == { node: DAILY, settles: Set(R_1), state: DActive, basis: [] },
    s.requests.get(R_1).answered_by == REPORT_1,
    s.requests.get(R_1).settled == SettledBy({ decision: DEC_1, answer: REPORT_1 }),
    s.entries.get(REPORT_1).ty == EToUser,
  }

  /// Step 1: the clarification attaches to r1, and no new request appears. r1 is settled already
  /// (D.12.2's order), and settling is final, so nothing in the state asks for the clarification
  /// to be applied (README, "Open questions", item 14).
  val after_1_clarify: bool = and {
    s.user_entries.get(CLARIFY).routing == RtClarifies(R_1),
    s.requests.keys() == Set(R_1),
    flags_now(USER) == Set(),
  }

  /// Step 2: a bare `talk request` from s-1, with the message as origin and no `where`, on the
  /// state before the script's request, would take case 3 (§9.4): the message becomes a key of
  /// summary-gap and the request's conversation. So the script passes the create's default `where`
  /// (A-61, D.8.9) explicitly, and `conv: THREAD` below holds by construction.
  val bare_request_case_3: bool = {
    val r = run_cmd(CFG, prev, clock, CHAT_WORKER, S_1, "", false, CRequest({ at: GAP, origin: MESSAGE, where_key: NoKey }))
    and {
      r.outcome == OutOk([R_2, GAP]),
      r.state.nodes.get(GAP).keys == Set(THREAD, MESSAGE),
      r.state.requests.get(R_2).conv == MESSAGE,
    }
  }

  /// Step 2, the first run: summary-gap is created with a hold and the thread's key; r2 is
  /// recorded on it with the message as its origin and the thread as its conversation (passed as
  /// `where`, see above), and the message id does not become a key (A-61).
  val after_2_first: bool = and {
    s.nodes.get(GAP).parent == ROOT,
    s.nodes.get(GAP).name == Lit("summary-gap"),
    s.nodes.get(GAP).keys == Set(THREAD),
    s.holds.get((GAP, 1)).session == S_1,
    s.requests.get(R_2) == {
      node: GAP, origin: MESSAGE, conv: THREAD, from_user: -1, requester: ReqExternal,
      answered_by: -1, settled: Unsettled, moved_from: -1,
    },
    bare_request_case_3,
  }

  /// Step 2, the second run: `{existing: summary-gap}` and the same request r2 (Q1); no hold, no
  /// new node, no new entry. Neither of its two calls changes anything: the world is the one after
  /// the first run, `shown`, tags, and keys included.
  val after_2_second: bool = and {
    s.next_node == GAP + 1,
    s.next_eid == R_2 + 1,
    holds_of(s, S_2) == Set(),
    s.requests.keys() == Set(R_1, R_2),
    s == fold_after("2 first").st,
  }

  /// Step 3: a commitment is open on summary-gap; both children are candidates; summary-gap is
  /// offered and carries `unanswered_request` (r2 is unsettled, the node unattended and not
  /// waiting: the literal rule, A-62), and no `open_commitment` yet (the follow-up lies ahead).
  val after_3: bool = and {
    s.saids.get(ACK) == { node: GAP, conv: THREAD, kind: SAck, session: S_1, state: SdConfirmed, moved_from: -1 },
    s.commitments.get(ACK) == {
      node: GAP, conv: THREAD, last_ack: T_ASKED, followup_at: T_ASKED + COMMITMENT_FOLLOWUP,
      followup_fired: false, state: CmOpen, moved_from: -1,
    },
    children(s, GAP) == Set(AUDIT, RERUN),
    candidate_in_tier(AUDIT, 2),
    candidate_in_tier(RERUN, 2),
    holds_on(s, GAP) == Set(),
    s.nodes.get(GAP).tags.contains(OFFER),
    flags_now(GAP) == Set(FUnansweredRequest),
    candidate_in_tier(GAP, 0),
  }

  /// Step 4, before the ask: the ask's conversation node is summary-gap, which s-3 has not seen
  /// (its take marks audit shown, which has no entries), so the ask's `moved` lists r2 and the ack,
  /// both by s-1 (§9.2). s-3 holds audit, a descendant, and no other session holds summary-gap or
  /// anything in it, so the ask gets no `conversation_held`, which s-2, holding nothing, would get.
  /// A read of summary-gap first, as in D.12.2's guided run, moves `shown`, and the ask would then
  /// carry no `moved`.
  val after_4_take: bool = and {
    holds_of(s, S_3) == Set((AUDIT, 1)),
    conv_of(s, AUDIT, NoKey).node == GAP,
    get_or(s.sessions.get(S_3).shown, GAP, 0) == 0,
    moved_of(s, S_3, GAP) == Set(R_2, ACK),
    not(conversation_held_of(s, S_3, GAP)),
    conversation_held_of(s, S_2, GAP),
    moved_of(view_ex(s, S_3, GAP).st, S_3, GAP) == Set(),
  }

  /// Step 4: the question and its `said` lie on summary-gap, the conversation node (C1); the
  /// release confirms the `said`, which the ask recorded pending (`recorded_pending`); audit waits
  /// on the question, whose recheck lies 4 h ahead.
  val after_4_ask: bool = and {
    s.pquestions.get(Q_1) == {
      node: GAP, conv: THREAD, blocking: false, said: Q_SAID, state: PqOpen, rechecks: 0,
      recheck_at: T_ASKED + QUESTION_RECHECK, lapse: NoLapse, assumed_by: -1, moved_from: -1,
    },
    s.saids.get(Q_SAID) == { node: GAP, conv: THREAD, kind: SQuestion, session: S_3, state: SdConfirmed, moved_from: -1 },
    s.entries.get(Q_SAID).node == GAP,
    s.entries.get(Q_1).node == GAP,
    s.nodes.get(AUDIT).wait == HasWait({ on: WQuestion({ q: Q_1, rechecks_at_set: 0 }), set_at: T_ASKED, fired: false }),
    waiting(s, AUDIT, clock),
    holds_of(s, S_3) == Set(),
    classify(s, AUDIT, flags_now(AUDIT), clock) == -1,
  }

  /// Step 4, 2 h later: the follow-up of the ack has passed and summary-gap is unattended, but the
  /// open question to the person suppresses `open_commitment` (§14). The assistant's hold on the
  /// series has expired unchecked (one attempt, and no `offer` on a series, §10.3).
  val after_4_later: bool = and {
    clock == T_ANSWERED,
    s.commitments.get(ACK).followup_fired,
    clock >= s.commitments.get(ACK).followup_at,
    not(attended(s, GAP, clock)),
    open_pq(s, GAP, THREAD),
    flags_now(GAP) == Set(FUnansweredRequest),
    s.holds == Map(),
    s.nodes.get(DAILY).attempts == 1,
    not(s.nodes.get(DAILY).tags.contains(OFFER)),
    flags_now(DAILY) == Set(),
    occ_waiting(s.nodes.get(MORNING)),
  }

  /// Step 5: `{existing: summary-gap, open_question: q1}`: the join appends nothing and finds the
  /// question open.
  val after_5_join: bool = and {
    last_action.outcome == OutOk([GAP]),
    open_pq(s, GAP, THREAD),
    holds_of(s, S_4) == Set(),
    s.next_node == RERUN + 1,
  }

  /// Step 5: q1 is answered; audit's wait resolves and audit is a candidate again; summary-gap now
  /// also carries `open_commitment`.
  val after_5_heard: bool = and {
    s.pquestions.get(Q_1).state == PqAnswered(HEARD),
    // summary-gap's sixth entry: r2, the ack, s-1's release checkpoint, the question's `said`,
    // the question, and the answer
    s.entries.get(HEARD) == { node: GAP, seq: 6, at: T_ANSWERED, by: S_4, ty: EHeard, principal: -1 },
    not(waiting(s, AUDIT, clock)),
    candidate_in_tier(AUDIT, 2),
    flags_now(GAP) == Set(FUnansweredRequest, FOpenCommitment),
  }

  /// Step 6, 21 minutes after s-5's checkpoint: its hold has expired (a checkpointed hold counts
  /// no attempt); rerun-test carries `open_intent` (tier 0) and `offer`; s-5 is silent. The scaled
  /// clock has passed `session.retain` since tick 0 (7 + 4 > 10 ticks, see above), so the six
  /// sessions last touched then have retired, which D.12.2's run on the real clock never does.
  val after_6_vanish: bool = and {
    clock == T_TAKEN_OVER,
    s.holds == Map(),
    s.nodes.get(RERUN).open_intent == { eid: INTENT, session: S_5 },
    s.nodes.get(RERUN).attempts == 0,
    s.nodes.get(RERUN).tags.contains(OFFER),
    flags_now(RERUN) == Set(FOpenIntent),
    candidate_in_tier(RERUN, 0),
    s.sessions.get(S_5).silent,
    s.sessions.keys() == Set(S_4, S_5),
  }

  /// Step 6: summary-gap is a tier-0 candidate too, unattended with its request unsettled (A-62),
  /// and `next` would offer it first, so s-6 takes rerun-test with `hold take`. In the model that
  /// order is the tie-break's alone, by id and by FNV alike: its `aged` is 0 (see above), and
  /// D.12.2's "older" is left to WP-D6's run on the real clock.
  val after_6_arrive: bool = and {
    candidate_in_tier(GAP, 0),
    candidate_in_tier(RERUN, 0),
    ranked_first(S_6) == GAP,
    ranked_first_fnv(S_6) == GAP,
  }

  /// Step 6: rerun-test is closed `done`, its intent closed by another session's `intent_done`
  /// (§10.2), with none of the warnings of a close. audit is still open, so summary-gap does not
  /// carry `ready_to_close` yet: the flag appears in step 7, when its last open child closes.
  val after_6_finish: bool = and {
    closed_as(s, RERUN, Done),
    s.nodes.get(RERUN).open_intent == NO_INTENT,
    close_clean(prev, RERUN, S_6),
    holds_of(s, S_6) == Set(),
    flags_now(RERUN) == Set(),
    is_open(s, AUDIT),
    not(flags_now(GAP).contains(FReadyToClose)),
  }

  /// Step 7: audit closes with a decision; summary-gap carries `ready_to_close`, and the close's
  /// `leave_better` suggests it, its parent at its level (A-63).
  val after_7_audit: bool = and {
    closed_as(s, AUDIT, Done),
    s.decisions.get(DEC_AUDIT) == { node: AUDIT, settles: Set(), state: DActive, basis: [] },
    close_clean(prev, AUDIT, S_6),
    flags_now(GAP).contains(FReadyToClose),
    leave_better_of(s, AUDIT, clock) == [GAP],
  }

  /// Step 7: the result fulfils the commitment and settles r2 (the decision came first, §9.4), and
  /// the `said` reports r2 among the requests it settled (1.3); nothing would make the close warn.
  val after_7_answer: bool = and {
    s.commitments.get(ACK).state == CmFulfilled(RESULT),
    s.requests.get(R_2).answered_by == RESULT,
    s.requests.get(R_2).settled == SettledBy({ decision: DEC_2, answer: RESULT }),
    s.saids.get(RESULT) == { node: GAP, conv: THREAD, kind: SResult, session: S_7, state: SdConfirmed, moved_from: -1 },
    close_clean(s, GAP, S_7),
  }

  /// Step 7: summary-gap is closed `done` with no warnings, and carries no flag.
  val after_7_close: bool = and {
    closed_as(s, GAP, Done),
    close_clean(prev, GAP, S_7),
    flags_now(GAP) == Set(),
    s.holds == Map(),
  }

  /// Step 8: the entry is the user's (no principal named), unrouted, and the session now serves
  /// `user`. `view requests` defaults to `all`, because `user` is the only principal the forum has
  /// seen (§8.3): its board shows the entry (Unrouted), and r1 and r2 Answered, an answer recorded
  /// after each, r2's from a person outside the swarm.
  val after_8_log: bool = and {
    s.user_entries.get(E_8) == { session: CODER, principal: USER_PRINCIPAL, at: T_TAKEN_OVER, routing: Unrouted },
    s.sessions.get(CODER).principals == Set(USER_PRINCIPAL),
    s.sessions.get(CODER).class == CODING_AGENT,
    flags_now(USER) == Set(FUnrouted),
    s.principal_ids.keys() == Set(USER_KEY),
    s.requests.keys() == Set(R_1, R_2),
    s.requests.keys().forall(r => s.requests.get(r).answered_by > r),
    s.requests.get(R_2).requester == ReqExternal,
  }

  /// Step 8: the entry is routed to none, and `@user` no longer carries `unrouted`; the report
  /// named an unrouted entry, so it answered no request.
  val after_8_reply: bool = and {
    s.user_entries.get(E_8).routing == RtNoAction,
    s.entries.get(REPORT_8).ty == EToUser,
    flags_now(USER) == Set(),
    s.requests.get(R_1).answered_by == REPORT_1,
    s.requests.get(R_2).answered_by == RESULT,
  }

  /// The end of §32: no word of the user is unrouted, every request is settled, every `said` is
  /// confirmed, every commitment fulfilled, every question answered, and no node carries a flag.
  /// Every node that was offered has been closed, and a close is the only thing that removes the
  /// `offer` tag (§5, §10.1), so no node carries it any more.
  val after_32: bool = and {
    s.user_entries.keys().forall(e => s.user_entries.get(e).routing != Unrouted),
    s.requests.keys().forall(r => s.requests.get(r).settled != Unsettled),
    s.saids.keys().forall(x => s.saids.get(x).state == SdConfirmed),
    s.commitments.keys().forall(c => s.commitments.get(c).state != CmOpen),
    s.pquestions.keys().forall(q => s.pquestions.get(q).state == PqAnswered(HEARD)),
    s.nodes.keys().forall(n => flags_now(n) == Set()),
    s.nodes.keys().forall(n => not(s.nodes.get(n).tags.contains(OFFER))),
    s.anomalies == 0,
    clock <= MAX_TIME,
  }

  /// The §32 example, step by step (D.12.2, D.13.2 WP-A4): every call gets the reply the script
  /// gives, by a session that has never fallen silent; `checked32` holds after every call and
  /// every tick; D.12.2's assertions hold after each part; and each part ends where the pure fold
  /// of the script does (`part32`), so the last state is the fold's last world.
  run scenario_32 =
    init
      .then(part32("setup")).expect(after_setup)
      .then(part32("1 log")).expect(after_1_log)
      .then(part32("1 create")).expect(after_1_create)
      .then(part32("1 report")).expect(after_1_report)
      .then(part32("1 clarify")).expect(after_1_clarify)
      .then(part32("2 first")).expect(after_2_first)
      .then(part32("2 second")).expect(after_2_second)
      .then(part32("3")).expect(after_3)
      .then(part32("4 take")).expect(after_4_take)
      .then(part32("4 ask")).expect(after_4_ask)
      .then(part32("4 later")).expect(after_4_later)
      .then(part32("5 join")).expect(after_5_join)
      .then(part32("5 heard")).expect(after_5_heard)
      .then(part32("6 vanish")).expect(after_6_vanish)
      .then(part32("6 arrive")).expect(after_6_arrive)
      .then(part32("6 finish")).expect(after_6_finish)
      .then(part32("7 audit")).expect(after_7_audit)
      .then(part32("7 answer")).expect(after_7_answer)
      .then(part32("7 close")).expect(after_7_close)
      .then(part32("8 log")).expect(after_8_log)
      .then(part32("8 reply")).expect(after_8_reply)
      .expect(after_32)
      .expect(s == WORLDS_32[WORLDS_32.length() - 1].st and clock == WORLDS_32[WORLDS_32.length() - 1].clock)

  /// `scenario_32` under a name that `quint test` runs without `--match` (D.10.7's command), while
  /// D.10.8 and D.13.2 name the run itself.
  run scenario32Test = scenario_32

  // ================================================================== T2: settling is path independent (WP-A4)

  /// Enough single firings for every fixture below to reach MAX_TIME.
  pure val T2_FIRINGS = 2 * MAX_FIRINGS

  /// Settling `st` up to `horizon` one firing at a time: each step is a `settle` that may fire one
  /// key, the queue's minimum, at its own time, as a writer that fires each deadline as soon as it
  /// falls due while idle would (§17, CS-30). Returns the state, the keys in the order they fired,
  /// and whether a firing was stuck.
  pure def fire_one_by_one(st: State, horizon: int): { st: State, keys: List[DeadlineKey], stuck: bool } =
    range(0, T2_FIRINGS).foldl({ st: st, keys: [], stuck: false }, (acc, i) => {
      val k = first_key(acc.st)
      if (acc.stuck or k.at < 0 or k.at > horizon) acc
      else {
        val r = settle(acc.st, k.at, 1)
        { st: r.state, keys: acc.keys.append(k), stuck: r.stuck }
      }
    })

  /// T2 (§26, D.8.2, D.9.2) on world `w` up to `horizon`, with `run(S, t)` the settle of D.10.4:
  /// - for every pair `t1 <= t2` of the sample times, `run(run(S, t1), t2) == run(S, t2)` on the
  ///   whole state, `last_now` included;
  /// - firing one key at a time up to `horizon` ends in `run(S, horizon)`, with nothing due left;
  /// - no settle is saturated or stuck (so truncation can neither make T2 pass nor fail, D.10.7).
  /// The sample times are `w.clock`, `horizon`, and the time of every key that fires on the way.
  /// While a firing depends only on the state and its own key, never on the target it settles to
  /// (lemma L1 of D.8.2), `run(S, t)` is the same for every `t` from one firing time up to the
  /// next, and the sample stands for every time between `w.clock` and `horizon`. `t2_ticks`
  /// checks every tick near the clock without that assumption: a prune at the target, which
  /// breaks L1, passes the sample and fails there (README, "The §32 scenario and T2").
  pure def t2_holds(w: World, horizon: int): bool = {
    val one = fire_one_by_one(w.st, horizon)
    val ts = sorted(one.keys.foldl(Set(w.clock, horizon), (acc, k) => acc.union(Set(k.at))))
    val rs = ts.foldl([], (acc, t) => acc.append(settle(w.st, t, MAX_FIRINGS)))
    and {
      not(one.stuck),
      first_key(one.st).at < 0 or first_key(one.st).at > horizon,
      rs.foldl(true, (ok, r) => ok and not(r.saturated) and not(r.stuck)),
      one.st == rs[rs.length() - 1].state,
      ts.indices().forall(i => ts.indices().forall(j => i > j or {
        val r = settle(rs[i].state, ts[j], MAX_FIRINGS)
        not(r.saturated) and not(r.stuck) and r.state == rs[j].state
      })),
    }
  }

  /// D.10.7's `inv_T2` on world `w`, literally: for every `a <= b` in `0..k`,
  /// `run(run(S, clock + a), clock + b) == run(S, clock + b)`, with no settle saturated or stuck.
  pure def t2_ticks(w: World, k: int): bool = {
    val rs = range(0, k + 1).foldl([], (acc, b) => acc.append(settle(w.st, w.clock + b, MAX_FIRINGS)))
    and {
      rs.foldl(true, (ok, r) => ok and not(r.saturated) and not(r.stuck)),
      0.to(k).forall(a => a.to(k).forall(b => {
        val r = settle(rs[a].state, w.clock + b, MAX_FIRINGS)
        not(r.saturated) and not(r.stuck) and r.state == rs[b].state
      })),
    }
  }

  /// The kinds of the keys that fire from `w` up to `horizon`, each as `(kind, owner rank)`.
  pure def fired_kinds(w: World, horizon: int): Set[(int, int)] =
    fire_one_by_one(w.st, horizon).keys.foldl(Set(), (acc, k) => acc.union(Set((k.kind, owner_rank(k.owner)))))

  /// Questions, waits, and timers, built by commands from the seed at 1. Up to MAX_TIME:
  /// a time wait (3) and an external wait (5) resolve; the seed sessions fall silent (4), which
  /// makes session 1's unconfirmed ack unconfirmed, and retire (11); the user question to `p1`
  /// (absent) is re-addressed to `user` (6), who is absent too, so that its due is
  /// `user_question.due_absent` again (§8.4), and lapses (11, with the seed sessions' retirement),
  /// marking the node that waits on it; the user question with a default, to `user`, defaults
  /// (6); the ack's follow-up (7), a due (7), and the pause (8) fire; the blocking question to the
  /// person is rechecked (9, marking the node whose wait the recheck resolved) and escalates (17),
  /// and the escalation's user question lapses (22), marking the open node it is about; /s's
  /// occurrences open and are missed twice (#11: 15 and 19; #14: 30 and 34), and #15 (slot 45)
  /// waits.
  pure val T2_QUESTIONS: World = {
    val a1 = after(SEED, 1, 1, take_cmd(W1))                                                           // hold (8, 1)
    val a2 = after(a1, 1, 1, CAskPerson({ at: W1, blocking: true, again: false }))                     // said 1, question 2
    val a3 = after(a2, 1, 1, CRelease({ token: (W1, 1), reason: Blocked, wait: WaQuestion(2), settles: Set() }))       // checkpoint 3
    val a4 = after(a3, 1, 2, CAskUser({ at: W2, has_default: false, to: "p1", again: false, digest: 1 }))   // question 4
    val a5 = after(a4, 1, 2, take_cmd(W2))                                                           // hold (9, 1)
    val a6 = after(a5, 1, 2, CRelease({ token: (W2, 1), reason: Blocked, wait: WaQuestion(4), settles: Set() }))       // checkpoint 5
    val a7 = after(a6, 1, 2, CAskUser({ at: SER, has_default: true, to: USER_KEY, again: false, digest: 2 }))   // question 6
    // 1.11 (§9.2): session 1 holds /w2 (to 4, when it falls silent), so its ack stays pending
    val a7b = after(a7, 1, 1, take_cmd(W2))                                                          // hold (9, 2)
    val a8 = after(a7b, 1, 1, CSaid({ at: W2, kind: SAck, where_key: NoKey }))                         // said 7, unconfirmed
    val a9 = after(a8, 1, 1, CPause(W2))                                                             // system entry 8
    val a10 = after(a9, 1, 2, create_cmd(W1, "a1", Set(), true, 1))                                    // node 12, hold (12, 1)
    val a11 = after(a10, 1, 2, CRelease({ token: (12, 1), reason: Handoff, wait: WaTime(3), settles: Set() }))         // checkpoint 9
    val a12 = after(a11, 1, 2, create_cmd(W1, "a2", Set(), true, 1))                                   // node 13, hold (13, 1)
    val a13 = after(a12, 1, 2, CRelease({ token: (13, 1), reason: Handoff, wait: WaExternal(5), settles: Set() }))     // checkpoint 10
    val a14 = after(a13, 1, 1, CUpdate({ ...update_none(12, a13.nodes.get(12).version), due: DueSet(7) }))
    { st: a14, clock: 1 }
  }

  /// Refusals, built by commands from the seed at 1: class `a` becomes active (`next`, its profile
  /// ends at 13); two of its sessions refuse /w1, the quorum of 2, so the refusal question is asked
  /// (E1) and its default drops /w1 (12); a refusal of /s by class `b` expires (13), and so does
  /// class `a`'s profile (13); a non-blocking question about /w2, which is closed before its last
  /// recheck, expires (17) with no open node to mark, so the server records the assumption at once.
  pure val T2_REFUSALS: World = {
    val b1 = after(SEED, 1, 1, CNext)                                                                // takes /w1: hold (8, 1)
    val b2 = after(b1, 1, 1, CRelease({ token: (W1, 1), reason: Cannot, wait: WaNone, settles: Set() }))             // checkpoint 1
    val b3 = run_cmd(CFG, b2, 1, "a", -1, "", false, CHello({ caps: Set(), principal: "" })).state   // session 3
    val b4 = after(b3, 1, 3, take_cmd(W1))                                                           // hold (8, 2)
    val b5 = after(b4, 1, 3, CRelease({ token: (W1, 2), reason: Cannot, wait: WaNone, settles: Set() }))             // checkpoint 2, question 3
    val b6 = after(b5, 1, 2, CAskPerson({ at: W2, blocking: false, again: false }))                    // said 4, question 5
    val b7 = after(b6, 1, 2, close_cmd(W2, Done))
    val b8 = after(b7, 1, 2, take_cmd(SER))                                                          // hold (10, 1)
    val b9 = after(b8, 1, 2, CRelease({ token: (SER, 1), reason: Cannot, wait: WaNone, settles: Set() }))
    { st: b9, clock: 1 }
  }

  /// The expiry prune (CS-8, CS-17): an `op_id` record, a note, and an agent question with its note,
  /// all made at 1 and gone at 11, a time at which nothing fires, because both seed sessions are
  /// touched again at 3 (they fall silent at 6 and retire at 13). An event at or after 11 prunes
  /// them; D.8.2 says the prune is monotone in `at`, so it commutes with T2.
  pure val T2_PRUNE: World = {
    val c1 = run_cmd(CFG, SEED, 1, "a", 1, "o1", false, CSay({ at: W1, to: SaySession(2) })).state   // note 1
    val c2 = after(c1, 1, 1, CAskAgent({ at: W1, to: AgSession(2) }))                                   // question 2
    val c3 = settle(c2, 3, MAX_FIRINGS).state                                                         // session 2 falls silent
    val c4 = after(c3, 3, 1, CView(W1))
    val c5 = after(c4, 3, 2, CView(W2))
    { st: c5, clock: 3 }
  }

  /// The fixtures reach what they are built for: the waits, questions, pause, and due of
  /// T2_QUESTIONS, and the refusal question, the two refusal records, the class profile, and the
  /// question about a closed node of T2_REFUSALS; and what each comes to by MAX_TIME.
  run t2FixturesTest = {
    val q = T2_QUESTIONS.st
    val r = T2_REFUSALS.st
    val q40 = settle(q, MAX_TIME, MAX_FIRINGS).state
    val r40 = settle(r, MAX_TIME, MAX_FIRINGS).state
    assert(and {
      settled(q, 1),
      q.nodes.get(W1).wait == HasWait({ on: WQuestion({ q: 2, rechecks_at_set: 0 }), set_at: 1, fired: false }),
      q.nodes.get(W2).wait == HasWait({ on: WUserQuestion(4), set_at: 1, fired: false }),
      q.nodes.get(12).wait == HasWait({ on: WTime(3), set_at: 1, fired: false }),
      q.nodes.get(13).wait == HasWait({ on: WExternal(5), set_at: 1, fired: false }),
      q.pquestions.get(2).blocking,
      q.uquestions.get(4).to == [1, USER_PRINCIPAL],
      not(q.uquestions.get(4).has_default),
      q.uquestions.get(6).has_default,
      q.saids.get(7).state == SdPending,
      q.pauses.keys() == Set(W2),
      q.nodes.get(12).due == 7,
      // by MAX_TIME: re-addressed, then lapsed with a mark on the node that waited; defaulted;
      // escalated, and the escalation lapsed with a mark on the node it is about; the ack
      // unconfirmed; /s's first two occurrences missed, and its third waiting
      q40.uquestions.get(4).stage == 1,
      q40.uquestions.get(4).state == UqLapsed,
      q40.nodes.get(W2).assume_marks == Set(4),
      q40.uquestions.get(6).state == UqDefaulted,
      match q40.pquestions.get(2).state {
        | PqEscalated(u) => q40.uquestions.get(u).state == UqLapsed and q40.nodes.get(W1).assume_marks == Set(u)
        | _ => false
      },
      q40.saids.get(7).state == SdUnconfirmed,
      closed_as(q40, OCC1, Missed),
      closed_as(q40, 14, Missed),
      open_occurrences(q40, SER) == Set(15),
      q40.nodes.get(12).due_fired,
      q40.pauses == Map(),
      q40.sessions == Map(),
      settled(r, 1),
      r.refusals.keys() == Set((W1, "a"), (SER, "b")),
      r.refusals.get((W1, "a")).by.keys() == Set(1, 3),
      r.classes.keys() == Set("a"),
      r.uquestions.get(3).source == SrcRefusal(W1),
      r.uquestions.get(3).has_default,
      is_closed(r, W2),
      r.pquestions.get(5).node == W2,
      // by MAX_TIME: the refusal default drops /w1, the other refusal and the profile expire, and
      // the question about the closed /w2 expires with its assumption recorded at once
      r40.uquestions.get(3).state == UqDefaulted,
      closed_as(r40, W1, Dropped),
      r40.refusals == Map(),
      r40.classes == Map(),
      r40.pquestions.get(5).state == PqExpired,
      r40.pquestions.get(5).assumed_by >= 0,
      r40.decisions.get(r40.pquestions.get(5).assumed_by).node == W2,
      // the prune fixture: three things expire at 11, between the firings at 6 and 13
      T2_PRUNE.st.ops.keys() == Set(("a", "o1")),
      T2_PRUNE.st.ops.get(("a", "o1")).expires_at == 1 + OP_RETAIN,
      T2_PRUNE.st.notes.keys() == Set(1, 2),
      T2_PRUNE.st.notes.keys().forall(x => T2_PRUNE.st.notes.get(x).until == 1 + DELIVER_WINDOW),
      T2_PRUNE.st.aquestions.keys() == Set(2),
      settled(T2_PRUNE.st, 3),
      settle(T2_PRUNE.st, 12, MAX_FIRINGS).state.ops.keys() == Set(("a", "o1")),
      next_deadline(settle(T2_PRUNE.st, 12, MAX_FIRINGS).state) == 13,
      settle(T2_PRUNE.st, 13, MAX_FIRINGS).state.ops == Map(),
      settle(T2_PRUNE.st, 13, MAX_FIRINGS).state.notes == Map(),
      settle(T2_PRUNE.st, 13, MAX_FIRINGS).state.aquestions == Map(),
    })
  }

  /// The world of the script after step 1's creates: the assistant's hold on the series is live.
  pure val WORLD_32_HELD: World = fold_after("1 create")

  /// Every deadline kind of §17 with its owner, as `(kind, owner rank)`: a refusal expiry is owned
  /// by a refusal record or by a class (D.8.2).
  pure val ALL_KINDS: Set[(int, int)] = Set(
    (K_LEASE, O_HOLD), (K_WAIT, O_NODE), (K_RECHECK, O_ENTRY), (K_USER_QUESTION, O_ENTRY),
    (K_COMMITMENT, O_ENTRY), (K_OCCURRENCE_SLOT, O_NODE), (K_OCCURRENCE_WINDOW, O_NODE),
    (K_REFUSAL_EXPIRY, O_REFUSAL), (K_REFUSAL_EXPIRY, O_CLASS), (K_DUE, O_NODE), (K_PAUSE, O_NODE),
    (K_SILENCE, O_SESSION), (K_RETIRE, O_SESSION), (K_STALE, O_NODE),
  )

  /// T2 (§26, D.8.2, D.9.2; D.13.2 WP-A4): settling is path independent on every state of the pure
  /// fold of the §32 script (`WORLDS_32`) and on the three fixtures above: up to MAX_TIME over the sample of
  /// `t2_holds`, and over every pair of ticks within T2_K of the clock, as D.10.7's `inv_T2` states
  /// it. Between them the firings cover every deadline kind of §17, a refusal record's expiry and a
  /// class profile's both. With the whole state compared, this includes T2's corollary for idle
  /// firing (CS-30): firing each deadline as soon as it falls due gives the state that firing them
  /// all at the next call does.
  run test_T2 = {
    val worlds = WORLDS_32.concat([T2_QUESTIONS, T2_REFUSALS, T2_PRUNE])
    assert(and {
      worlds.foldl(true, (ok, w) => ok and t2_holds(w, MAX_TIME)),
      worlds.foldl(true, (ok, w) => ok and t2_ticks(w, T2_K)),
      fired_kinds(WORLD_32_HELD, MAX_TIME).union(fired_kinds(T2_QUESTIONS, MAX_TIME)).union(fired_kinds(T2_REFUSALS, MAX_TIME))
        == ALL_KINDS,
    })
  }

  /// `test_T2` under a name that `quint test` runs without `--match` (D.10.7's command), while
  /// D.13.2 names the run itself.
  run t2Test = test_T2

}
