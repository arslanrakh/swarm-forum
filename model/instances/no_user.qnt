// -*- mode: Bluespec; -*-
/**
 * The forum with no human (D.10.1, D.10.7; CS-2, CS-6, ZU-9): one worker class, no `from_user` and
 * no `to_user` action at all, and `user` as the only principal, so that a question nobody answers
 * is seen to end (re-addressed, defaulted, or lapsed). For simulation; the `user_none` module of
 * the `user` slice (WP-A3) is the same configuration for Apalache, TLC, and liveness.
 * For simulation, a call carries an `op` or a long text in one roll in RARE_ODDS = 8 (README,
 * "Simulation coverage"); `scripts/simulate.sh` reports the runs that reach each witness.
 *
 *   npx quint run instances/no_user.qnt --max-steps 40 --max-samples 20000 --invariant inv_all
 */
module no_user {
  import forum(
    CLASSES = Set("w"),
    USE_FNV = false,
    QUORUM = 1,
    PRINCIPALS = Set("user"),
    HAS_USER = false,
    MAX_FIRINGS = 40,
    SEED_SERIES = true,
    LONG_TEXTS = Set(false, true),
    RARE_ODDS = 8
  ).* from "../forum"
}
