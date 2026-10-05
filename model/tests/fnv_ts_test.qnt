// -*- mode: Bluespec; -*-
/**
 * The vectors of D.2.6 at u64::MAX, through `le_bytes`, `fnv_id`, `fnv_session`, and `tie_hash`
 * themselves. Quint's default (Rust) evaluator computes in 64-bit signed integers and refuses the
 * literal 2^64 − 1 (QNT600), so this file runs on the TypeScript evaluator, which `npm test` picks
 * for every file named `*_ts_test.qnt`:
 *
 *   cd model && npx quint test --backend typescript tests/fnv_ts_test.qnt
 */
module fnv_ts_test {
  import basics.* from "../basics"

  pure val U64MAX = 18446744073709551615

  run u64MaxBytesTest = assert(and {
    le_bytes(U64MAX) == [255, 255, 255, 255, 255, 255, 255, 255],
    le_bytes(U64MAX - 1) == [254, 255, 255, 255, 255, 255, 255, 255],
    le_bytes(9223372036854775808) == [0, 0, 0, 0, 0, 0, 0, 128],     // 2^63
  })

  run u64MaxFnvTest = assert(and {
    fnv_id(U64MAX) == u64_of(0x7a4969ca, 0x2d631437),
    fnv_session(U64MAX) == u64_of(0x280457de, 0x49e07df4),
    tie_hash(U64MAX, U64MAX) == u64_of(0x524511eb, 0xe3829643),
  })
}
