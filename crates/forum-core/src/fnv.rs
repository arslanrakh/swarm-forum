//! FNV-1a-64 and the tie-break ring of `next` (design D.2.6, §10.4, CS-12, D-1).
//!
//! The candidates of one aged group stand on a circle at `fnv_id(id)`, and each session starts at
//! its own point `fnv_session(session)` and goes round, so `tie_hash` is the distance from the
//! session's point to the candidate's:
//!
//! ```text
//! fnv_id(id)          = FNV-1a-64(0x00 ‖ id.to_le_bytes())          // 9 bytes
//! fnv_session(s)      = FNV-1a-64(s.0.to_le_bytes() ‖ 0x01)         // 9 bytes
//! h = tie_hash(s, id) = fnv_id(id).wrapping_sub(fnv_session(s))     // (… − …) mod 2^64
//! ```

use crate::types::SessionId;

/// The FNV-1a-64 offset basis.
pub const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
/// The FNV-1a-64 prime.
pub const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// One FNV-1a step: `h = (h ^ b) * prime`, wrapping.
fn step(h: u64, b: u8) -> u64 {
    (h ^ u64::from(b)).wrapping_mul(FNV_PRIME)
}

/// Continues a hash over `bytes`.
fn extend(h: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(h, |h, &b| step(h, b))
}

/// FNV-1a-64 of `bytes`.
#[must_use]
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    extend(FNV_OFFSET_BASIS, bytes)
}

/// A candidate's point on the ring: FNV-1a-64 of `0x00` and the id's 8 little-endian bytes.
#[must_use]
pub fn fnv_id(id: u64) -> u64 {
    extend(step(FNV_OFFSET_BASIS, 0x00), &id.to_le_bytes())
}

/// A session's starting point on the ring: FNV-1a-64 of the id's 8 little-endian bytes and `0x01`.
#[must_use]
pub fn fnv_session(session: SessionId) -> u64 {
    step(extend(FNV_OFFSET_BASIS, &session.0.to_le_bytes()), 0x01)
}

/// The distance on the ring from the session's point to the candidate's:
/// `fnv_id(id).wrapping_sub(fnv_session(session))`.
#[must_use]
pub fn tie_hash(session: SessionId, id: u64) -> u64 {
    fnv_id(id).wrapping_sub(fnv_session(session))
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    reason = "test code: the deny list guards the crate's own code"
)]
mod tests {
    use super::*;

    /// The pinned vectors of design D.2.6, which `kani::primitives::fnv_reference` checks too.
    #[test]
    fn pinned_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
        assert_eq!(fnv_id(0), 0xe604_823a_2490_29bf);
        assert_eq!(fnv_id(6), 0x2c23_d803_e2f4_6cf9);
        assert_eq!(fnv_id(42), 0x8f91_9d01_1520_8895);
        assert_eq!(fnv_id(u64::MAX), 0x7a49_69ca_2d63_1437);
        assert_eq!(fnv_session(SessionId(0)), 0xe604_813a_2490_280c);
        assert_eq!(fnv_session(SessionId(1)), 0x529a_2ddc_8ff5_355f);
        assert_eq!(fnv_session(SessionId(2)), 0x0cd9_2bf5_4dc6_1432);
        assert_eq!(fnv_session(SessionId(17)), 0x893f_83b5_d9a4_968f);
        assert_eq!(fnv_session(SessionId(u64::MAX)), 0x2804_57de_49e0_7df4);
        assert_eq!(tie_hash(SessionId(0), 0), 0x0000_0100_0000_01b3);
        assert_eq!(tie_hash(SessionId(1), 6), 0xd989_aa27_52ff_379a);
        assert_eq!(tie_hash(SessionId(1), 42), 0x3cf7_6f24_852b_5336);
        assert_eq!(tie_hash(SessionId(2), 42), 0x82b8_710b_c75a_7463);
        assert_eq!(tie_hash(SessionId(17), 5), 0xc1df_1b57_143f_208b);
        assert_eq!(
            tie_hash(SessionId(u64::MAX), u64::MAX),
            0x5245_11eb_e382_9643
        );
    }

    #[test]
    fn the_nine_byte_forms() {
        let mut id_bytes = [0_u8; 9];
        id_bytes[1..].copy_from_slice(&42_u64.to_le_bytes());
        assert_eq!(fnv_id(42), fnv1a64(&id_bytes));
        let mut session_bytes = [1_u8; 9];
        session_bytes[..8].copy_from_slice(&17_u64.to_le_bytes());
        assert_eq!(fnv_session(SessionId(17)), fnv1a64(&session_bytes));
    }
}
