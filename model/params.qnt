// -*- mode: Bluespec; -*-
/**
 * Parameters of the model: the scaled parameters of D.10.5, the constants of the core that the model
 * reads, and the bounds of D.10.7, all as pure values.
 *
 * One tick is one minute when a trace is replayed against Rust (D.10.5). Every real duration has its
 * own tick value, so the scaling adds no equality and keeps every order of the real values (§28).
 * `refusal.quorum` is not here: it is the `QUORUM` constant of `forum.qnt` (D.10.1).
 */
module params {
  import types.* from "./types"

  // ------------------------------------------------------------------ durations (ticks, D.10.5)

  pure val PRESENCE_RESOLUTION = 1      // 3 min (CS-14)
  pure val LEASE_FIRST = 3              // 15 min
  pure val PRESENCE_SILENCE = 3         // 15 min
  pure val LEASE_DEFAULT = 4            // 20 min
  pure val USER_QUESTION_DUE_ABSENT = 5 // 1 h (CS-2)
  pure val REFUSAL_BACKOFF = 5          // 1 h (CS-7)
  pure val COMMITMENT_FOLLOWUP = 6      // 90 min
  pure val LEASE_MAX = 7                // 2 h
  pure val PAUSE_TTL = 7                // 2 h
  pure val QUESTION_RECHECK = 8         // 4 h
  pure val LEASE_CEILING = 9            // 8 h
  pure val DELIVER_WINDOW = 10          // 24 h
  pure val SESSION_RETAIN = 10          // 24 h (CS-8)
  pure val OP_RETAIN = 10               // 24 h (CS-8)
  pure val USER_QUESTION_DUE = 11       // 72 h
  pure val REFUSAL_TTL = 12             // 7 days
  pure val REFUSAL_WINDOW = 12          // 7 days
  pure val ARCHIVE_AFTER = 12           // 7 days
  pure val CAPACITY_WINDOW = 12         // 7 days
  pure val USER_GAP_WINDOW = 13         // 28 days
  pure val COVERAGE_WINDOW = 13         // 28 days
  pure val OFFER_STALE = 14             // 90 days (1.8, §14): within MAX_TIME, so offers go stale

  /// The hour of `aged` (§10.4): the replay's real hour, not scaled. Every model time lies in the
  /// first hour bucket because MAX_TIME < HOUR, so `aged = priority` (D.10.5).
  pure val HOUR = 60

  // ------------------------------------------------------------------ counts (D.10.5)

  pure val RENEW_MAX = 2                // 3
  pure val HOLD_PER_SESSION = 2         // 2
  pure val QUESTION_MAX_RECHECKS = 2    // 2
  pure val TOO_BIG_ATTEMPTS = 2         // 3
  pure val TREE_MAX_DEPTH = 4           // 16
  pure val TREE_MAX_CHILDREN = 3        // 500

  /// Not scaled: they only drive a warning or a list length, which conformance drops (D.10.5).
  pure val USER_QUESTION_MAX_PENDING = 20
  pure val LEAVE_BETTER_MAX = 3

  /// The lease terms of every hold the model issues (a snapshot in Rust, A-102).
  pure val LEASE_TERMS: LeaseTerms = {
    first: LEASE_FIRST,
    default: LEASE_DEFAULT,
    max: LEASE_MAX,
    ceiling: LEASE_CEILING,
    renew_max: RENEW_MAX,
  }

  // ------------------------------------------------------------------ constants of the core (D.3)

  pure val UQ_REQUESTERS_MAX = 3        // requester stages of a chain (D.8.12)
  pure val UQ_OWNERS_MAX = 3            // owner stages of a chain
  pure val UQ_CHAIN_MAX = 8             // 1 explicit + 3 requesters + 3 owners + `user` (U1)
  pure val HELLO_USER_ITEMS = 5

  /// Real-time constants, never scaled (D.10.5): 7 and 28 days in ticks, far beyond MAX_TIME, so
  /// nothing leaves memory in a model run but notes, agent questions, and `op_id` records.
  pure val FINISHED_RETAIN = 10080
  pure val DIGEST_RETAIN = 40320

  // ------------------------------------------------------------------ the seeds' series (D.10.5)

  /// Period 15 ticks, offset 0, `run_within` 4 ticks; replayed as `*/15 * * * *` in UTC (15 divides
  /// 60, D.10.6). D.10.5 says period 10, but then a series created at 0 that nobody takes opens
  /// occurrences at slots 10, 20, 30, and 40 within MAX_TIME, four, against the bound of §28 and
  /// D.10.7 ("up to 3 occurrences of one recurring node"). With period 15 the slots within 40 are 15
  /// and 30, and the third occurrence (slot 45) opens at 34, so time alone opens at most 3
  /// (`seedOccurrenceBoundTest`). Recorded for a decision (README, "Deviations").
  pure val SEED_REPEAT: RepeatSpec = { period: 15, offset: 0, run_within: 4 }

  // ------------------------------------------------------------------ bounds and guards (D.10.7)

  pure val MAX_TIME = 40                // clock + dt <= MAX_TIME
  pure val MAX_DT = 3                   // act_tick(dt), dt in 1..MAX_DT
  pure val MAX_EID = 16                 // next_eid <= MAX_EID
  pure val MAX_VER = 6                  // every version, fence, and seq <= MAX_VER
  pure val MAX_COUNT = 3                // attempts, checkpoints, and approvals <= MAX_COUNT
  pure val MAX_USER_ENTRIES = 2

  /// The bound of every ancestor walk and every walk along `merged_into` (D.8.4: "at most
  /// tree.max_depth steps, and at most nodes.len() steps"). It covers TREE_MAX_DEPTH with room for a
  /// violation of L1's depth half to stay visible to the invariants.
  pure val WALK_MAX = 6

  /// The order of class names for the deadline queue's owner encoding (D.10.4): the class index of
  /// `Refusal(n, c)` and `Class(c)` is a class's position here. Quint cannot compare strings, so
  /// every class name an instance uses must be in this list, sorted as Rust sorts `ClassName`, and
  /// CLASS_INDEX must invert it.
  pure val CLASS_NAMES = ["a", "b", "w"]
  pure val CLASS_INDEX: str -> int = Map("a" -> 0, "b" -> 1, "w" -> 2)

  /// Every class of a configuration has an index. WP-A2's instances assert it of their `CLASSES`:
  /// for a class outside CLASS_INDEX, `owner_class` would encode an owner of rank 3 (`Refusal`) and
  /// `fire` would read `class_name(-1)`.
  pure def classes_indexed(classes: Set[str]): bool = classes.forall(c => CLASS_INDEX.keys().contains(c))

  /// The bounds of §28 as D.10.7 reads them (A-108). The class configurations themselves are the
  /// `CLASSES` constant of `forum.qnt` (two classes, `a` and `b`, or one, `w`), and the principals
  /// its `PRINCIPALS` constant (D.10.1).
  pure val SESSIONS_PER_CLASS = 2          // session ids 1..4 with two classes, each opened once
  pure val MAX_AGENT_NODES = 4             // besides the eight reserved and the server-made nodes
  pure val MAX_OCCURRENCES = 3             // of the one series (`may_open_occurrence`, basics.qnt)
  pure val MAX_SIGNAL_NODES = 2
  pure val OP_IDS = Set("o1", "o2")        // an action's `op` is one of these, or none ("")
  pure val MAX_REQUESTS = 2                // user requests included
  pure val CAPACITIES = Set(0, 1, 2)
  pure val DEFAULT_PRINCIPALS = Set("user", "p1", "p2")   // PRINCIPALS of the instances that name none
  pure val MAX_DECISIONS = 3
  pure val MAX_BASIS_REFS = 2              // per decision (CS-22)
  /// The origins and `where` keys `act_request` picks from (A-61, D.10.4): a repeat of one message,
  /// a second message of one conversation, and a conversation without message ids.
  pure val REQUEST_ORIGINS: Set[Key] = Set(Msg(("c1", 1)), Msg(("c1", 2)), Conv("c1"), Conv("c2"))
  pure val REQUEST_WHERES: Set[OptKey] = Set(NoKey, SomeKey(Conv("c1")), SomeKey(Conv("c2")))
  /// T2 as a state invariant (D.10.7) settles to `clock + a` and `clock + b` for `a <= b <= K`:
  /// K = 12 in simulation and TLC, 4 in Apalache (2 in the `user` and `people` slices if needed).
  pure val T2_K = 12
  pure val T2_K_APALACHE = 4

  // ------------------------------------------------------------------ checks

  /// The orders `Params::valid` checks (D.3), on the scaled values (D.10.5). Durations are at least
  /// one tick; one tick is a minute, at least the `SECOND` that `Params::valid` asks for.
  pure val PARAMS_VALID: bool = and {
    PRESENCE_RESOLUTION >= 1,
    LEASE_FIRST <= LEASE_DEFAULT,
    LEASE_DEFAULT <= LEASE_MAX,
    LEASE_MAX <= LEASE_CEILING,
    USER_QUESTION_DUE_ABSENT <= USER_QUESTION_DUE,
    REFUSAL_BACKOFF <= REFUSAL_TTL,
    3 * PRESENCE_RESOLUTION <= PRESENCE_SILENCE,
    SESSION_RETAIN > LEASE_CEILING,
    SESSION_RETAIN > PRESENCE_SILENCE,
    TREE_MAX_DEPTH >= 3,
    HOLD_PER_SESSION >= 1,
    QUESTION_MAX_RECHECKS >= 1,
    TOO_BIG_ATTEMPTS >= 1,
    TREE_MAX_CHILDREN >= 1,
    RENEW_MAX >= 0,
  }

  /// The real durations in minutes, in the order of the table of D.10.5, beside their ticks: the
  /// scaling keeps every strict order and every equality of the real values, and adds none.
  pure val SCALED: List[(int, int)] = [
    (3, PRESENCE_RESOLUTION),
    (15, LEASE_FIRST), (15, PRESENCE_SILENCE),
    (20, LEASE_DEFAULT),
    (60, USER_QUESTION_DUE_ABSENT), (60, REFUSAL_BACKOFF),
    (90, COMMITMENT_FOLLOWUP),
    (120, LEASE_MAX), (120, PAUSE_TTL),
    (240, QUESTION_RECHECK),
    (480, LEASE_CEILING),
    (1440, DELIVER_WINDOW), (1440, SESSION_RETAIN), (1440, OP_RETAIN),
    (4320, USER_QUESTION_DUE),
    (10080, REFUSAL_TTL), (10080, REFUSAL_WINDOW), (10080, ARCHIVE_AFTER), (10080, CAPACITY_WINDOW),
    (40320, USER_GAP_WINDOW), (40320, COVERAGE_WINDOW),
    (129600, OFFER_STALE),
  ]

  pure val SCALING_PRESERVES_ORDER: bool =
    SCALED.indices().forall(i => SCALED.indices().forall(j => and {
      SCALED[i]._1 < SCALED[j]._1 iff SCALED[i]._2 < SCALED[j]._2,
      SCALED[i]._1 == SCALED[j]._1 iff SCALED[i]._2 == SCALED[j]._2,
    }))

  /// The comparisons `Params::valid` makes (D.3) whose sides are not single durations of SCALED, as
  /// `(real lhs, real rhs, scaled lhs, scaled rhs)`, each read as `lhs <= rhs` (minutes, ticks).
  pure val VALID_COMBINATIONS: List[(int, int, int, int)] = [
    (3 * 3, 15, 3 * PRESENCE_RESOLUTION, PRESENCE_SILENCE),   // 3 × presence.resolution <= presence.silence
  ]

  /// Every combination holds scaled, as `Params::valid` needs. D.10.5 says that "the scaling adds no
  /// equality"; that is true of the durations pairwise (SCALING_PRESERVES_ORDER), but not of this
  /// combination: 9 < 15 minutes is 3 = 3 ticks, so the model runs at the boundary `Params::valid`
  /// allows (silence at least two thirds of `presence.silence` after the last call, CS-14, where the
  /// real values give four fifths). No rule of §26 reads the difference. This value pins it, so that a
  /// change of the table shows here; the claim of D.10.5 is recorded for a decision (README).
  pure val VALID_COMBINATIONS_SCALED: bool = and {
    VALID_COMBINATIONS.foldl(true, (ok, c) => ok and c._1 <= c._2 and c._3 <= c._4),
    VALID_COMBINATIONS[0]._1 < VALID_COMBINATIONS[0]._2,
    VALID_COMBINATIONS[0]._3 == VALID_COMBINATIONS[0]._4,
  }

  /// The counts of the table of D.10.5 (the real value in each comment), and the lease terms every
  /// hold gets.
  pure val COUNTS_SCALED: bool = and {
    RENEW_MAX == 2,                 // 3
    HOLD_PER_SESSION == 2,          // 2
    QUESTION_MAX_RECHECKS == 2,     // 2
    TOO_BIG_ATTEMPTS == 2,          // 3
    TREE_MAX_DEPTH == 4,            // 16
    TREE_MAX_CHILDREN == 3,         // 500
    LEASE_TERMS == { first: 3, default: 4, max: 7, ceiling: 9, renew_max: 2 },
  }

  /// D.10.5: within any model run every time lies in the first hour bucket, so `aged = priority`.
  run maxTimeBelowHourTest = assert(MAX_TIME < HOUR)

  run paramsValidTest = assert(PARAMS_VALID)

  run scalingPreservesOrderTest = assert(SCALING_PRESERVES_ORDER)

  run validCombinationsTest = assert(VALID_COMBINATIONS_SCALED)

  run countsScaledTest = assert(COUNTS_SCALED)

  /// The seed's series has a period that divides 60 (D.10.6) and fewer than MAX_OCCURRENCES slots up
  /// to MAX_TIME, so the occurrence a slot's close opens is at most the third (§28).
  run seedRepeatTest = assert(and {
    60 % SEED_REPEAT.period == 0,
    SEED_REPEAT.run_within > 0 and SEED_REPEAT.run_within < SEED_REPEAT.period,
    (MAX_TIME - SEED_REPEAT.offset) / SEED_REPEAT.period < MAX_OCCURRENCES,
  })

  run classesIndexedTest = assert(and {
    classes_indexed(Set("a", "b")),
    classes_indexed(Set("w")),
    not(classes_indexed(Set("a", "x"))),
  })

  /// The walks cover the deepest node L1 allows, and the class order is consistent.
  run walkAndClassesTest = assert(and {
    WALK_MAX >= TREE_MAX_DEPTH + 1,
    CLASS_NAMES.indices().forall(i => CLASS_INDEX.get(CLASS_NAMES[i]) == i),
    CLASS_INDEX.keys().size() == CLASS_NAMES.length(),
  })

  /// A session retires within MAX_TIME of its last touch, holding nothing (CS-8, D.10.5).
  run retireWithinBoundsTest = assert(SESSION_RETAIN < MAX_TIME and LEASE_CEILING < SESSION_RETAIN)
}
