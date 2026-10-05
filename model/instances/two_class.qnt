// -*- mode: Bluespec; -*-
/**
 * The forum with two worker classes of two sessions each (§28, D.10.7): `QUORUM = 2`, so that one
 * class can reach a refusal quorum (CS-7), and the FNV-1a-64 tie-break of `next`, so that a trace
 * replays against Rust with exact choices (D.10.6, D.10.8). The principals are `user`, `p1`, and
 * `p2` (CS-1), and the seed has the series `/s`, so that simulation covers recurring work.
 * For simulation, a call carries an `op` or a long text in one roll in RARE_ODDS = 8 (README,
 * "Simulation coverage"); `scripts/simulate.sh` reports the runs that reach each witness.
 *
 *   npx quint run instances/two_class.qnt --max-steps 40 --max-samples 20000 --invariant inv_all
 */
module two_class {
  import forum(
    CLASSES = Set("a", "b"),
    USE_FNV = true,
    QUORUM = 2,
    PRINCIPALS = Set("user", "p1", "p2"),
    HAS_USER = true,
    MAX_FIRINGS = 40,
    SEED_SERIES = true,
    LONG_TEXTS = Set(false, true),
    RARE_ODDS = 8
  ).* from "../forum"
}
