// -*- mode: Bluespec; -*-
/**
 * A smoke spec for the model checkers over the WP-A1 modules (README, "Apalache and TLC on a model
 * this size"): a seed state with a deadline of almost every kind, settled by the two time actions
 * of D.10.4 and nothing else. It is no slice of D.10.7 (those come with WP-A3); it shows that
 * `scripts/verify-file.sh` takes the model through Apalache and TLC, and what settling costs them.
 *
 *   scripts/verify-file.sh tlc tests/settle_smoke.qnt --main smoke_tlc --invariant inv_tlc
 *   scripts/verify-file.sh apalache tests/settle_smoke.qnt --main smoke_apalache --invariant inv_smoke --length 1
 *
 * `npm test` does not run it (it has no runs, and is not named `*_test.qnt`); `npm run typecheck`
 * checks it.
 */
module settle_smoke {
  import types.* from "../types"
  import params.* from "../params"
  import basics.* from "../basics"
  import flags.* from "../flags"
  import deadlines.* from "../deadlines"

  /// The firings one settle may make (D.10.4's MAX_FIRINGS).
  const MAX_FIRINGS: int

  pure def smoke_node(parent: int, name: str, ks: Set[Key], tags: Set[str]): Node =
    { ...reserved_node(parent, name, 0), origin: OAgent(1), capacity: 1, content: 1, keys: ks, tags: tags }

  pure def smoke_session(c: str, at: int): Session = {
    class: c, started_at: at, last_seen: at, silence_at: at + PRESENCE_SILENCE,
    retire_at: at + SESSION_RETAIN, silent: false, caps: Set(), horizon: -1, principals: Set(),
    shown: Map(), branch_seen: Map(), handed_off: NO_HANDOFF,
  }

  /// Sessions 1 and 2 (classes a and b), /w1 (#8, held by 1) and /w2 (#9, held by 2, waiting on the
  /// blocking people question #2 about /w1), the series /s (#10) with its first occurrence (#11), a
  /// refusal of /w2 by class a, and a caps profile of class a.
  pure val SEED: State = {
    val g = genesis(0)
    val s0 = { ...g, next_session: 3, sessions: Map(1 -> smoke_session("a", 0), 2 -> smoke_session("b", 0)) }
    val s1 = ev_node_created(ev_node_created(s0, smoke_node(ROOT, "w1", Set(Conv("c1")), Set(OFFER)), 0),
                             smoke_node(ROOT, "w2", Set(Conv("c2")), Set(OFFER)), 0)
    val s2 = ev_node_created(s1, { ...smoke_node(ROOT, "s", Set(), Set()), role: Series({ repeat: SEED_REPEAT, last_missed: false }) }, 0)
    val s3 = open_next_occurrence(s2, 10, 0, 0)
    val s4 = set_wait(s3, 9, WQuestion({ q: 2, rechecks_at_set: 1 }), 0)
    val held = (8, 1)
    val held2 = (9, 1)
    val terms = { session: 1, taken_at: 0, lease_until: LEASE_FIRST, last_extended: 0, terms: LEASE_TERMS,
                  l: LEASE_DEFAULT, renews: 0, checkpointed: false }
    {
      ...s4,
      nodes: s4.nodes.setBy(8, nd => { ...nd, fence: 1 }).setBy(9, nd => { ...nd, fence: 1 }),
      holds: Map(held -> terms, held2 -> { ...terms, session: 2 }),
      next_eid: 3,
      pquestions: Map(2 -> { node: 8, conv: Conv("c1"), blocking: true, said: 1, state: PqOpen, rechecks: 0,
                             recheck_at: QUESTION_RECHECK, lapse: NoLapse, assumed_by: -1, moved_from: -1 }),
      classes: Map("a" -> { caps_seen: Map(Set() -> 12) }),
      refusals: Map((9, "a") -> { quorum: 1, by: Map(1 -> { backoff_until: 6, expires_at: 13 }) }),
    }
  }

  var s: State
  var clock: int
  var saturated: bool
  var stuck: bool

  action init = all { s' = SEED, clock' = 0, saturated' = false, stuck' = false }

  /// D.10.4's `act_tick(dt)`.
  action tick = nondet dt = 1.to(MAX_DT).oneOf() all {
    clock + dt <= MAX_TIME,
    val r = settle(s, clock + dt, MAX_FIRINGS)
    all { s' = r.state, clock' = clock + dt, saturated' = r.saturated, stuck' = r.stuck },
  }

  /// D.10.4's `act_tick_next`.
  action tick_next = {
    val t = next_deadline(s)
    all {
      t >= 0, t <= MAX_TIME, t > clock,
      val r = settle(s, t, MAX_FIRINGS)
      all { s' = r.state, clock' = t, saturated' = r.saturated, stuck' = r.stuck },
    }
  }

  action step = any { tick, tick_next }

  /// What the Apalache measurement checks (README): no hold on a closed node, and every flag
  /// predicate evaluates on every node.
  val inv_no_hold_on_closed = s.holds.keys().forall(t => is_open(s, t._1))
  val inv_flags_total = s.nodes.keys().forall(n => flags_of(s, n, clock).size() <= 13)
  val inv_smoke = inv_no_hold_on_closed and inv_flags_total

  /// What TLC checks besides: no settle was stuck or saturated, the state is settled at `clock`
  /// (T3, T4), `first_key` is the queue's minimum, and the occurrences stay within §28's bound.
  val inv_tlc = and {
    inv_smoke,
    not(stuck),
    not(saturated),
    settled(s, clock),
    first_key(s) == queue_min(s),
    occurrences_made(s).size() <= MAX_OCCURRENCES,
  }
}

/// TLC: enough firings for every settle of the seed (`inv_tlc` fails on saturation).
module smoke_tlc {
  import settle_smoke(MAX_FIRINGS = 20).*
}

/// Apalache: one firing per settle, as measured in the README.
module smoke_apalache {
  import settle_smoke(MAX_FIRINGS = 1).*
}
