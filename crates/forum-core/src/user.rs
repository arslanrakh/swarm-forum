//! Bounds on what the user side shows and stores (design D.3, CS-2, CS-3, CS-5, D-3). They are
//! constants, not parameters, because §22 fixes them.

use crate::types::Dur;

/// Items of `user_decisions` and `user_digest` in `hello`.
pub const HELLO_USER_ITEMS: usize = 5;
/// Requester stages of a chain (D.8.12).
pub const UQ_REQUESTERS_MAX: usize = 3;
/// Owner stages of a chain: the nearest owners, then ROOT's.
pub const UQ_OWNERS_MAX: usize = 3;
/// 1 explicit + 3 requesters + 3 owners + `user` (U1).
pub const UQ_CHAIN_MAX: usize = 8;
/// A digest entry not shown to its principal within this time leaves the pending list, counted in
/// `State::digest_left` (CS-5, D.4.12); it stays in `@user/digest`. Real time, never scaled: 28
/// days.
pub const DIGEST_RETAIN: Dur = Dur(2_419_200_000);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DAY;

    #[test]
    fn the_fixed_values_of_section_22() {
        assert_eq!(DIGEST_RETAIN, Dur(28 * DAY.0));
        assert_eq!(UQ_CHAIN_MAX, 1 + UQ_REQUESTERS_MAX + UQ_OWNERS_MAX + 1);
    }
}
