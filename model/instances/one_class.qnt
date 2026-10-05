// -*- mode: Bluespec; -*-
/**
 * The single-worker-class configuration of §28 (D.10.7): one class of two sessions, `QUORUM = 1`,
 * which keeps 1.1's single refusal reachable (CS-7), and the tie-break by id (D.10.6). The
 * principals are `user`, `p1`, and `p2`.
 * For simulation, a call carries an `op` or a long text in one roll in RARE_ODDS = 8 (README,
 * "Simulation coverage"); `scripts/simulate.sh` reports the runs that reach each witness.
 *
 *   npx quint run instances/one_class.qnt --max-steps 40 --max-samples 20000 --invariant inv_all
 */
module one_class {
  import forum(
    CLASSES = Set("w"),
    USE_FNV = false,
    QUORUM = 1,
    PRINCIPALS = Set("user", "p1", "p2"),
    HAS_USER = true,
    MAX_FIRINGS = 40,
    SEED_SERIES = false,
    LONG_TEXTS = Set(false, true),
    RARE_ODDS = 8
  ).* from "../forum"
}
