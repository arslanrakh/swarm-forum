//! Civil calendar, frozen time-zone tables, 5-field cron, and `next_slot` (design D.11.10).
//!
//! Pure integer arithmetic over a time-zone table that the shell supplies as data: the crate never
//! reads a time-zone database, a clock, or the environment. It is inside the verified boundary
//! (design D.1.4): every function is total, and the Kani harnesses of design D.12.5 are in
//! `kani.rs`.

#![cfg_attr(not(any(test, kani)), no_std)]
#![forbid(unsafe_code)]
#![deny(
    clippy::float_arithmetic,
    clippy::float_cmp,
    clippy::arithmetic_side_effects,   // only checked_*, saturating_*, wrapping_* (FNV)
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::as_conversions,            // casts go through From/TryFrom
    clippy::iter_over_hash_type,
    clippy::disallowed_types,
    clippy::disallowed_methods,
    clippy::wildcard_enum_match_arm,   // every match names every variant
)]

extern crate alloc;

mod civil;
mod cron;
#[cfg(kani)]
mod kani;
mod tz;

pub use civil::{civil_from_days, days_from_civil, weekday};
pub use cron::{CronError, CronSpec, MAX_SEARCH_DAYS, next_slot, parse_cron, slot_name};
pub use tz::{
    MAX_TRANSITIONS, Transition, TzError, TzTable, earliest_utc, local_day_hour, offset_at,
    valid_table,
};

#[cfg(test)]
mod tests {
    use crate::{
        CronError, CronSpec, MAX_SEARCH_DAYS, MAX_TRANSITIONS, Transition, TzError, TzTable,
    };
    use alloc::string::String;
    use alloc::vec::Vec;

    /// Every item of the public API of design D.11.10, named through the crate root with its
    /// type, so that a missing export, a changed signature, or a changed set of variants or
    /// fields fails to compile. Other packages build against this API (design D.13.1).
    #[test]
    fn the_api_of_design_d_11_10() {
        let _: fn(&str) -> Result<CronSpec, CronError> = crate::parse_cron;
        let _: fn(&CronSpec, &TzTable, u64) -> Option<u64> = crate::next_slot;
        let _: fn(u64) -> String = crate::slot_name;
        let _: fn(&TzTable) -> bool = crate::valid_table;
        let _: fn(&TzTable, i64) -> i32 = crate::offset_at;
        let _: fn(&TzTable, i64) -> Option<i64> = crate::earliest_utc;
        let _: fn(&TzTable, u64) -> (i64, u8) = crate::local_day_hour;
        let _: fn(i64, u8, u8) -> i64 = crate::days_from_civil;
        let _: fn(i64) -> (i64, u8, u8) = crate::civil_from_days;
        let _: fn(i64) -> u8 = crate::weekday;
        assert_eq!(MAX_SEARCH_DAYS, 3_000_u32);
        assert_eq!(MAX_TRANSITIONS, 512_usize);

        let spec = CronSpec {
            minutes: 0_u64,
            hours: 0_u32,
            doms: 0_u32,
            months: 0_u16,
            dows: 0_u8,
            dom_any: false,
            dow_any: false,
        };
        let table = TzTable {
            name: String::from("UTC"),
            tzdb_version: String::new(),
            initial: 0_i32,
            transitions: Vec::from([Transition {
                at: 0_i64,
                offset: 0_i32,
            }]),
        };
        assert_eq!(crate::next_slot(&spec, &table, 0), None);

        // The matches are exhaustive and name each variant, so they pin the set of variants.
        let cron_errors = [
            CronError::FieldCount,
            CronError::Field { index: 0_u8 },
            CronError::Range { index: 1_u8 },
            CronError::Step { index: 2_u8 },
        ];
        for error in &cron_errors {
            match error {
                CronError::FieldCount
                | CronError::Field { .. }
                | CronError::Range { .. }
                | CronError::Step { .. } => {}
            }
        }
        let tz_errors = [TzError::UnknownZone, TzError::TooManyTransitions];
        for error in &tz_errors {
            match error {
                TzError::UnknownZone | TzError::TooManyTransitions => {}
            }
        }
        assert!(TzError::UnknownZone < TzError::TooManyTransitions);
    }
}
