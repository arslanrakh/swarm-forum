//! Frozen time-zone tables: UTC offsets, gaps, and folds.
//!
//! A table maps each UTC instant to the offset in force (`offset_at`); a local time is the UTC
//! instant plus that offset. A local time skipped by a forward shift (a gap) has no instant, and one
//! repeated by a backward shift (a fold) has two; `earliest_utc` returns the earlier.

use alloc::string::String;
use alloc::vec::Vec;

use crate::civil::floor_div_rem;

/// The most transitions a [`TzTable`] may hold.
pub const MAX_TRANSITIONS: usize = 512;

/// The largest UTC offset, east or west, that a valid table may hold: 26 hours, in seconds.
pub(crate) const MAX_OFFSET_S: i32 = 93_600;

/// Seconds in a day.
pub(crate) const DAY_S: i64 = 86_400;

/// A time zone frozen as a list of UTC offset transitions. The shell builds it from its bundled
/// time-zone database (`forum_server::tz::table`); it travels in commands and events, so replay
/// never consults a database.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TzTable {
    /// IANA name, "UTC" by default (provenance and rendering).
    pub name: String,
    /// For example "2026c" (provenance only).
    pub tzdb_version: String,
    /// UTC offset in seconds before the first transition.
    pub initial: i32,
    /// Strictly increasing `at`, at most [`MAX_TRANSITIONS`].
    pub transitions: Vec<Transition>,
}

/// One change of the UTC offset.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Transition {
    /// Unix seconds.
    pub at: i64,
    /// Seconds east of UTC.
    pub offset: i32,
}

/// Why the shell could not build a [`TzTable`] (`forum_server::tz::table`); it answers `invalid`
/// with the `tz` field. No function of this crate returns it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TzError {
    /// The time-zone database has no zone of that name.
    UnknownZone,
    /// The zone changes its offset more than [`MAX_TRANSITIONS`] times within the table's range.
    TooManyTransitions,
}

/// Whether `tz` is ordered, bounded, and has offsets within ±26 hours.
///
/// Ordered: the transitions' `at` strictly increase. Bounded: at most [`MAX_TRANSITIONS`]
/// transitions. Every offset, `initial` included, lies within ±26 hours and is a whole number of
/// minutes, so that every slot is a whole UTC minute (design D.2.1). Every function of this crate
/// is total on any table; on a table that is not valid, `next_slot` finds no slot and the results
/// of the others are unspecified.
pub fn valid_table(tz: &TzTable) -> bool {
    tz.transitions.len() <= MAX_TRANSITIONS
        && valid_offset(tz.initial)
        && tz.transitions.iter().all(|t| valid_offset(t.offset))
        && tz
            .transitions
            .iter()
            .zip(tz.transitions.iter().skip(1))
            .all(|(a, b)| a.at < b.at)
}

/// Within ±26 hours, in whole minutes.
fn valid_offset(offset: i32) -> bool {
    (-MAX_OFFSET_S..=MAX_OFFSET_S).contains(&offset) && offset.wrapping_rem(60) == 0
}

/// The UTC offset in force at `utc_s` (binary search): that of the last transition at or before
/// `utc_s`, or `initial` before the first.
pub fn offset_at(tz: &TzTable, utc_s: i64) -> i32 {
    let after = tz.transitions.partition_point(|t| t.at <= utc_s);
    after
        .checked_sub(1)
        .and_then(|i| tz.transitions.get(i))
        .map_or(tz.initial, |t| t.offset)
}

/// The local time of `utc_s`, in seconds since 1970-01-01T00:00 local; `None` only on overflow.
pub(crate) fn local_time(tz: &TzTable, utc_s: i64) -> Option<i64> {
    utc_s.checked_add(i64::from(offset_at(tz, utc_s)))
}

/// The earliest UTC instant whose local time is `local_s`; `None` in a gap.
///
/// Tests each offset `o` in force at some instant within ±26 hours of `local_s` (every instant
/// whose local time is `local_s` lies there) and keeps those with `offset_at(local_s − o) = o`:
/// none means the local time does not exist; two mean it is ambiguous, and the earlier is returned.
pub fn earliest_utc(tz: &TzTable, local_s: i64) -> Option<i64> {
    let max_offset = i64::from(MAX_OFFSET_S);
    let window_start = local_s.saturating_sub(max_offset);
    let window_end = local_s.saturating_add(max_offset);
    let instant = |offset: i32| {
        local_s
            .checked_sub(i64::from(offset))
            .filter(|&utc| offset_at(tz, utc) == offset)
    };
    // The offset in force at the window's start, then that of each transition within it.
    let first = tz.transitions.partition_point(|t| t.at <= window_start);
    let later = tz
        .transitions
        .get(first..)
        .unwrap_or_default()
        .iter()
        .take_while(|t| t.at <= window_end)
        .filter_map(|t| instant(t.offset));
    instant(offset_at(tz, window_start))
        .into_iter()
        .chain(later)
        .min()
}

/// The local day (days since 1970-01-01) and hour of `utc_ms`.
pub fn local_day_hour(tz: &TzTable, utc_ms: u64) -> (i64, u8) {
    let utc_s = i64::try_from(utc_ms / 1_000).unwrap_or(i64::MAX);
    let local = local_time(tz, utc_s).unwrap_or(utc_s);
    let (day, second) = floor_div_rem(local, DAY_S);
    (day, u8::try_from(second / 3_600).unwrap_or(0))
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    reason = "test code: the deny list guards the crate's own code"
)]
mod tests {
    use super::*;
    use crate::civil::days_from_civil;

    const H: i64 = 3_600;

    fn table(initial: i32, transitions: &[(i64, i32)]) -> TzTable {
        TzTable {
            name: String::from("Test/Zone"),
            tzdb_version: String::from("test"),
            initial,
            transitions: transitions
                .iter()
                .map(|&(at, offset)| Transition { at, offset })
                .collect(),
        }
    }

    fn utc() -> TzTable {
        table(0, &[])
    }

    /// Unix seconds of a UTC civil date and time.
    fn unix(y: i64, mo: u8, d: u8, h: i64, mi: i64) -> i64 {
        days_from_civil(y, mo, d) * 86_400 + h * H + mi * 60
    }

    /// Europe/Berlin in 2026 (tzdb): CET, CEST from 29 March, CET from 25 October.
    fn berlin() -> TzTable {
        table(3_600, &[(1_774_746_000, 7_200), (1_792_890_000, 3_600)])
    }

    #[test]
    fn validity() {
        assert!(valid_table(&utc()));
        assert!(valid_table(&berlin()));
        assert!(valid_table(&table(93_600, &[(0, -93_600)])));
        assert!(valid_table(&table(20_700, &[]))); // +05:45
        assert!(!valid_table(&table(93_660, &[]))); // beyond +26 h
        assert!(!valid_table(&table(0, &[(5, -93_660)]))); // beyond −26 h
        assert!(!valid_table(&table(0, &[(5, 3_600), (5, 0)]))); // not strictly increasing
        assert!(!valid_table(&table(0, &[(6, 3_600), (5, 0)]))); // decreasing
        assert!(!valid_table(&table(-2_670, &[]))); // −00:44:30 is not whole minutes
        let full: Vec<(i64, i32)> = (0..512).map(|i| (i * 1_000, 0)).collect();
        assert!(valid_table(&table(0, &full)));
        let over: Vec<(i64, i32)> = (0..513).map(|i| (i * 1_000, 0)).collect();
        assert!(!valid_table(&table(0, &over)));
    }

    #[test]
    fn offsets() {
        let tz = berlin();
        assert_eq!(offset_at(&tz, i64::MIN), 3_600);
        assert_eq!(offset_at(&tz, 1_774_746_000 - 1), 3_600);
        assert_eq!(offset_at(&tz, 1_774_746_000), 7_200);
        assert_eq!(offset_at(&tz, 1_792_890_000 - 1), 7_200);
        assert_eq!(offset_at(&tz, 1_792_890_000), 3_600);
        assert_eq!(offset_at(&tz, i64::MAX), 3_600);
        assert_eq!(offset_at(&utc(), 0), 0);
        // Many transitions: the binary search finds the last one at or before the instant.
        let many: Vec<(i64, i32)> = (0..500)
            .map(|i| (i * 100, i32::try_from(i).unwrap() * 60))
            .collect();
        let tz = table(-60, &many);
        assert_eq!(offset_at(&tz, -1), -60);
        for i in 0..500_i64 {
            let expected = i32::try_from(i).unwrap() * 60;
            assert_eq!(offset_at(&tz, i * 100), expected);
            assert_eq!(offset_at(&tz, i * 100 + 99), expected);
        }
    }

    #[test]
    fn gaps_and_folds_in_berlin() {
        let tz = berlin();
        // 2026-03-29 02:30 does not exist: the clocks go from 02:00 to 03:00.
        let gap = unix(2026, 3, 29, 2, 30);
        assert_eq!(earliest_utc(&tz, gap), None);
        assert_eq!(
            earliest_utc(&tz, unix(2026, 3, 29, 1, 59)),
            Some(unix(2026, 3, 29, 0, 59))
        );
        assert_eq!(
            earliest_utc(&tz, unix(2026, 3, 29, 3, 0)),
            Some(unix(2026, 3, 29, 1, 0))
        );
        // 2026-10-25 02:30 happens twice, at 00:30Z (CEST) and 01:30Z (CET): the earlier wins.
        let fold = unix(2026, 10, 25, 2, 30);
        assert_eq!(earliest_utc(&tz, fold), Some(unix(2026, 10, 25, 0, 30)));
        assert_eq!(
            earliest_utc(&tz, unix(2026, 10, 25, 1, 59)),
            Some(unix(2026, 10, 24, 23, 59))
        );
        assert_eq!(
            earliest_utc(&tz, unix(2026, 10, 25, 3, 0)),
            Some(unix(2026, 10, 25, 2, 0))
        );
        // An ordinary time has exactly one instant.
        assert_eq!(
            earliest_utc(&tz, unix(2026, 7, 1, 12, 0)),
            Some(unix(2026, 7, 1, 10, 0))
        );
        assert_eq!(earliest_utc(&utc(), 1_000), Some(1_000));
    }

    #[test]
    fn earliest_is_the_smallest_instant_with_that_local_time() {
        // Transitions close together, a gap followed by a larger fold, and the ±26 h extremes.
        let tables = [
            berlin(),
            table(0, &[(100 * 60, 50 * 60), (200 * 60, -100 * 60)]),
            table(93_600, &[(0, -93_600), (60, 93_600), (7_200, 0)]),
            table(-36_000, &[(1_325_239_200, 50_400)]),
        ];
        for tz in &tables {
            let base = tz.transitions.first().map_or(0, |t| t.at);
            for local in (base - 30 * H..base + 30 * H).step_by(300) {
                // Brute force over every whole minute that could carry this local time.
                let brute = (local - 27 * H..=local + 27 * H)
                    .step_by(60)
                    .find(|&u| u + i64::from(offset_at(tz, u)) == local);
                assert_eq!(earliest_utc(tz, local), brute, "{tz:?} {local}");
            }
        }
    }

    #[test]
    fn total_on_extreme_input() {
        let tz = table(i32::MAX, &[(i64::MIN, i32::MIN), (i64::MAX, i32::MAX)]);
        for x in [i64::MIN, -1, 0, 1, i64::MAX] {
            let _ = offset_at(&tz, x);
            let _ = earliest_utc(&tz, x);
            let _ = local_time(&tz, x);
        }
        for ms in [0, 1, u64::MAX] {
            let _ = local_day_hour(&tz, ms);
            let _ = local_day_hour(&utc(), ms);
        }
        let _ = valid_table(&table(i32::MIN, &[(i64::MAX, i32::MIN), (i64::MIN, 0)]));
    }

    #[test]
    fn local_days_and_hours() {
        assert_eq!(local_day_hour(&utc(), 0), (0, 0));
        assert_eq!(local_day_hour(&utc(), 86_399_999), (0, 23));
        assert_eq!(local_day_hour(&utc(), 86_400_000), (1, 0));
        let tz = berlin();
        let day = days_from_civil(2026, 3, 29);
        // 00:59:59.999Z is 01:59 CET; 01:00Z is 03:00 CEST.
        let ms = |s: i64| u64::try_from(s).unwrap() * 1_000;
        assert_eq!(
            local_day_hour(&tz, ms(unix(2026, 3, 29, 1, 0)) - 1),
            (day, 1)
        );
        assert_eq!(local_day_hour(&tz, ms(unix(2026, 3, 29, 1, 0))), (day, 3));
        // 23:30Z on 28 March is 00:30 CET on 29 March.
        assert_eq!(local_day_hour(&tz, ms(unix(2026, 3, 28, 23, 30))), (day, 0));
        // West of UTC: 03:00Z on 1 January is 22:00 the day before in New York.
        let ny = table(-18_000, &[]);
        let jan1 = days_from_civil(2026, 1, 1);
        assert_eq!(
            local_day_hour(&ny, ms(unix(2026, 1, 1, 3, 0))),
            (jan1 - 1, 22)
        );
        assert_eq!(local_day_hour(&ny, 0), (-1, 19));
    }
}
