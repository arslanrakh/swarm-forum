// -*- mode: Bluespec; -*-
/**
 * Run tests of the invariants (WP-A3, `invariants.qnt`): every check of D.9.2 on steps built by
 * hand, for the situations that random simulation reaches rarely, and the readings of D.9.2 that
 * the model's checks take where D.9.2's wording and the specification part (README, "Open
 * questions").
 *
 *   cd model && npx quint test tests/invariants_test.qnt
 *
 * The configuration is `two_class`'s with the tie-break by id, as in `forum_test.qnt`.
 */
module invariants_test {
  import types.* from "../types"
  import params.* from "../params"
  import basics.* from "../basics"
  import flags.* from "../flags"
  import deadlines.* from "../deadlines"
  import commands.* from "../commands"
  import invariants.* from "../invariants"
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

  // The seed (D.10.7): sessions 1 (class a) and 2 (class b) at 0; /w1 (#8, key c1) and /w2 (#9,
  // key c2); the series /s (#10) and its first occurrence (#11, slot 15, window 19).
  pure val W1 = 8
  pure val W2 = 9
  pure val SER = 10
  pure val OCC1 = 11

  pure def cls_of(st: State, sess: int): str = if (has_session(st, sess)) st.sessions.get(sess).class else "a"

  /// One call, as `command` makes it, and the step the invariants see.
  pure def call(st: State, now: int, sess: int, cmd: Cmd): Run =
    run_cmd(CFG, st, now, cls_of(st, sess), sess, "", false, cmd)

  /// The state after the command, before its post-settle (`mid`).
  pure def after(st: State, now: int, sess: int, cmd: Cmd): State = call(st, now, sess, cmd).state

  /// D.8.1's post-settle of the call, as `command` runs it.
  pure def post_settle(st: State, now: int, sess: int, cmd: Cmd): Settle = settle(after(st, now, sess, cmd), now, MAX_FIRINGS)

  pure def action_of(st: State, now: int, sess: int, name: str, cmd: Cmd): Action = {
    val p = post_settle(st, now, sess, cmd)
    { name: name, sess: sess, class: cls_of(st, sess), op: "", long: false, cmd: cmd, outcome: call(st, now, sess, cmd).outcome,
      settle: { saturated: p.saturated, stuck: p.stuck } }
  }

  /// The command's own step (`check_step`), from `st` to the state before the post-settle.
  pure def stepped(st: State, now: int, sess: int, name: str, cmd: Cmd): Step =
    step_of(st, after(st, now, sess, cmd), now, action_of(st, now, sess, name, cmd), MAX_FIRINGS)

  /// Every check of `inv_all` on the action of one call: the command, then its post-settle.
  pure def all_after(st: State, now: int, sess: int, name: str, cmd: Cmd): bool =
    all_invariants(st, after(st, now, sess, cmd), post_settle(st, now, sess, cmd).state, now,
                   action_of(st, now, sess, name, cmd), MAX_FIRINGS, T2_K)

  /// Two sessions kept live by hand at `t` (a fixture: no call made it).
  pure def live_at(st: State, t: int): State = touched(touched(st, 1, t), 2, t)

  pure def closed_as(st: State, n: int, o: Outcome): bool =
    match st.nodes.get(n).life { | Closed(c) => c.outcome == o | Open => false }

  run seedInvariantsTest = assert(all_invariants(SEED, SEED, SEED, 0, INIT_ACTION, MAX_FIRINGS, T2_K))

  // ------------------------------------------------------------------ the post-settle of a command (D.8.1)

  /// R27 (1.4, D.4.8): session 2 takes /w1 at 0 (its `silence_at` is 3); session 1 checkpoints
  /// that hold at 1 (a token is the capability, H2), which extends its lease to 5 and keeps session
  /// 2 present past 3; at 3 session 1 releases it with a `{time}` wait at 3. Session 2 then holds
  /// nothing, so its Silence key lies at `max(silence_at, last_now) = 3`, the release's `now`: the
  /// state after the command alone is not settled at 3 (T3, T4), and the post-settle fires the key
  /// in the same call, as the core does. The wait at `now` is resolved at once by its marker (A-68)
  /// and queues nothing. `step_T4` allows the command a key at exactly its `now` only for such a
  /// Silence key (D.9.2), and every check holds on the command and on the firing.
  run postSettleSilenceTest = {
    val taken = after(SEED, 0, 2, CTake({ node: W1, lease: -1 }))
    val extended = after(taken, 1, 1, CCheckpoint({ token: (W1, 1), intent: false, intent_done: false }))
    val ticked = settle(extended, 3, MAX_FIRINGS).state
    val release = CRelease({ token: (W1, 1), reason: Handoff, wait: WaTime(3), settles: Set() })
    val released = after(ticked, 3, 1, release)
    val p = post_settle(ticked, 3, 1, release)
    val c = stepped(ticked, 3, 1, "act_release", release)
    assert(and {
      taken.sessions.get(2).silence_at == 3,
      extended.holds.get((W1, 1)).lease_until == 5,
      extended.holds.get((W1, 1)).session == 2,
      ticked == extended,                                                 // nothing due by 3
      settled(ticked, 3),
      call(ticked, 3, 1, release).outcome == OutOk([]),
      released.holds == Map(),
      released.nodes.get(W1).wait == HasWait({ on: WTime(3), set_at: 3, fired: true }),
      first_key(released) == { at: 3, kind: K_SILENCE, owner: owner_session(2) },
      not(inv_T3(released, 3)),                                           // the defect: not settled
      not(p.saturated) and not(p.stuck),
      p.state.sessions.get(2).silent,
      p.state != released,
      settled(p.state, 3),
      inv_T3(p.state, 3) and inv_T4(p.state, 3, action_of(ticked, 3, 1, "act_release", release)),
      step_T4(c),
      last_hold_ended(ticked, released, 2, 3),
      steps_of(ticked, released, p.state, 3, action_of(ticked, 3, 1, "act_release", release), MAX_FIRINGS).fire.fs.length() == 1,
      all_after(ticked, 3, 1, "act_release", release),
    })
  }

  /// `step_T4`'s exception is the Silence key alone: the same command step with a pause key at
  /// `now` added to its state after it fails (a command that queued it would break D.9.2's T4).
  run postSettleOnlySilenceTest = {
    val taken = after(SEED, 0, 2, CTake({ node: W1, lease: -1 }))
    val ticked = settle(after(taken, 1, 1, CCheckpoint({ token: (W1, 1), intent: false, intent_done: false })), 3, MAX_FIRINGS).state
    val release = CRelease({ token: (W1, 1), reason: Handoff, wait: WaNone, settles: Set() })
    val c = stepped(ticked, 3, 1, "act_release", release)
    val bad = { ...c.post, pauses: c.post.pauses.put(W2, { until: 3, last_renewed: 3, ttl: PAUSE_TTL, by: 1, class: "a" }) }
    assert(and {
      step_T4(c),
      not(step_T4({ ...c, post: bad })),
    })
  }

  // ------------------------------------------------------------------ C1 on a node of two conversations (1.3)

  /// §9.2, 1.3: on /w1 carrying c1 and c2 (after a merge), with an ack in each, a reply in c1
  /// fulfils c1's commitment and leaves c2's open; `step_C1` holds on each step, and so does every
  /// other check.
  run c1TwoConversationsTest = {
    val r1 = after(SEED, 1, 1, CRequest({ at: W1, origin: Msg(("c1", 1)), where_key: SomeKey(Conv("c1")) }))
    val r2 = after(r1, 1, 1, CRequest({ at: W2, origin: Msg(("c2", 1)), where_key: SomeKey(Conv("c2")) }))
    val acks = after(after(r2, 1, 1, CSaid({ at: W1, kind: SAck, where_key: NoKey })), 1, 1,
                     CSaid({ at: W2, kind: SAck, where_key: NoKey }))                 // commitments 3 (c1), 4 (c2)
    val merge = CMerge({ from: W2, into: W1 })
    val both = after(acks, 2, 2, merge)
    val reply = CSaid({ at: W1, kind: SReply, where_key: SomeKey(Conv("c1")) })
    val c = stepped(both, 3, 1, "act_said", reply)
    assert(and {
      both.nodes.get(W1).keys == Set(Conv("c1"), Conv("c2")),
      both.commitments.get(3).state == CmOpen and both.commitments.get(4).state == CmOpen,
      step_C1(c),
      c.post.commitments.get(3).state != CmOpen,
      c.post.commitments.get(4).state == CmOpen,
      all_after(acks, 2, 2, "act_merge", merge),
      step_N5(stepped(acks, 2, 2, "act_merge", merge)),
      all_after(both, 3, 1, "act_said", reply),
    })
  }

  // ------------------------------------------------------------------ R2 and a move (A-80)

  /// D.9.2's step half of R2, word for word: a `missed` close only on a node with no checkpoint,
  /// no open intent, and no live hold in the state before the step.
  pure def r2_literal(pre: State, post: State, at: int): bool =
    newly_missed(pre, post).forall(n => and {
      pre.nodes.get(n).checkpoints == 0,
      pre.nodes.get(n).open_intent.eid < 0,
      not(pre.holds.keys().exists(t => t._1 == n and at < pre.holds.get(t).lease_until)),
    })

  /// A move by another session ends the holder's hold, and the missed check that follows closes
  /// the occurrence as `missed` (D.8.20, A-80: "the missed check runs after every hold end that
  /// leaves the occurrence open (lease expiry, release, move)"). The occurrence had a live hold
  /// in the state before the move, so D.9.2's step clause "no live hold in `before`" fails on a
  /// step that §26 R2 and A-80 allow; `step_R2` excludes the holds the step's own move ended
  /// (README, "Open questions").
  run r2MoveTest = {
    val avail = settle(live_at(SEED, 14), 17, MAX_FIRINGS).state                 // #11 available at 15
    val held = after(avail, 17, 1, CTake({ node: OCC1, lease: -1 }))             // lease until 20
    val windowed = settle(held, 19, MAX_FIRINGS).state                           // window ends at 19
    val move = CMove({ node: OCC1, parent: ROOT, name: Lit("a1") })
    val moved = after(windowed, 19, 2, move)
    assert(and {
      held.holds.keys() == Set((OCC1, 1)),
      is_open(windowed, OCC1),
      windowed.holds.keys() == Set((OCC1, 1)),
      closed_as(moved, OCC1, Missed),
      moved.holds == Map(),
      step_R2(stepped(windowed, 19, 2, "act_move", move)),
      not(r2_literal(windowed, moved, 19)),
      all_after(windowed, 19, 2, "act_move", move),
    })
  }

  // ------------------------------------------------------------------ U1's chain at creation

  /// An update that adds a need and sets the owners of a refused node makes every active class
  /// inactive for it, so `e1_check` asks the user (D.8.20, D.8.14) after `NodeUpdated`, with the
  /// chain of the updated node: its new owner first. U1 [step] fixes the chain "at creation"
  /// (§26), and `step_U1` reads it on the state at creation; D.9.2's `spec::chain(before, …)`
  /// would read the owners before the update (README, "Open questions").
  run u1ChainAtCreationTest = {
    val nxt = after(SEED, 0, 2, CNext)                                          // class b active; 2 holds /w1
    val s3 = after(nxt, 0, -1, CHello({ caps: Set(), principal: "" }))         // session 3, class a
    val person = after(s3, 0, 1, CCreate({ parent: PEOPLE, name: Lit("a1"), keys: Set(Person("p1")), take: false,
                                           capacity: 0, owners: Set(), repeat: RepKeep, tags: Set() }))   // #12, p1 = 1
    val r1 = after(after(person, 0, 1, CTake({ node: W2, lease: -1 })), 0, 1,
                   CRelease({ token: (W2, 1), reason: Cannot, wait: WaNone, settles: Set() }))
    val r2 = after(after(r1, 0, 3, CTake({ node: W2, lease: -1 })), 0, 3,
                   CRelease({ token: (W2, 2), reason: Cannot, wait: WaNone, settles: Set() }))    // class a refused /w2
    val upd = CUpdate({ node: W2, ver: r2.nodes.get(W2).version, dver: -1, content: -1, status: -1, digest: -1,
                        capacity: -1, needs_add: Set("x"), needs_remove: Set(), tags_add: Set(), tags_remove: Set(),
                        due: DueKeep, depends: IdsKeep, owners: IdsSet(Set(12)), repeat: RepKeep })
    val c = stepped(r2, 0, 1, "act_update", upd)
    val q = r2.next_eid
    assert(and {
      class_refused(r2, W2, "a", 0),
      not(all_refused(r2, W2, 0)),                                                 // class b is active
      r2.uquestions == Map(),
      c.post.uquestions.keys() == Set(q),
      c.post.uquestions.get(q).source == SrcRefusal(W2),
      c.post.uquestions.get(q).to == [1, USER_PRINCIPAL],                         // the new owner, then user
      chain(r2, W2, -1, true) == [USER_PRINCIPAL],                                 // the owners before the update
      step_U1(c),
      all_after(r2, 0, 1, "act_update", upd),
    })
  }

  // ------------------------------------------------------------------ hello_exact over aliases

  /// A person with two aliases (`p1`, `p2`), a session that serves `p1`, and a question for the
  /// user addressed to `p2`: the session's `hello` lists it, by the scan and by the principals it
  /// serves alike (U1's query property).
  run helloExactAliasesTest = {
    val person = after(SEED, 0, 1, CCreate({ parent: PEOPLE, name: Lit("a1"), keys: Set(Person("p1"), Person("p2")),
                                             take: false, capacity: 0, owners: Set(), repeat: RepKeep, tags: Set() }))
    val said = after(person, 0, 1, CFromUser({ principal: "p1", answers: -1, clarifies: -1, node: -1 }))  // 1 serves p1
    val ask = CAskUser({ at: W1, has_default: false, to: "p2", again: false, digest: 1 })
    val asked = after(said, 0, 2, ask)
    val q = said.next_eid
    assert(and {
      said.sessions.get(1).principals == Set(said.principal_ids.get("p1")),
      asked.uquestions.get(q).to[0] == said.principal_ids.get("p2"),
      hello_scan(asked, 1)._1 == Set(q),
      hello_indexed(asked, 1)._1 == Set(q),
      hello_exact(asked),
      all_after(said, 0, 2, "act_ask_user", ask),
    })
  }
}
