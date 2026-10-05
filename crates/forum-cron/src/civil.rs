//! The proleptic Gregorian calendar over days since 1970-01-01 (Hinnant's algorithms).
//!
//! Both directions use a year that starts on 1 March, so that the leap day is the last day of the
//! year, and 400-year eras of 146,097 days. Every operation is total: `civil_from_days` is exact for
//! every `i64`, and `days_from_civil` is exact for every valid date whose day number fits in an
//! `i64` (years within about ±2.5 × 10¹⁶) and saturates beyond.

/// Days in a 400-year era of the Gregorian calendar.
const DAYS_PER_ERA: i64 = 146_097;

/// Days from 0000-03-01 (day 0 of era 0) to 1970-01-01.
const EPOCH_FROM_ERA_START: i64 = 719_468;

/// Five eras less the days from the start of era 0 to 1970-01-01: 5 × 146,097 − 719,468.
const FIVE_ERAS_PAST_EPOCH: i64 = 11_017;

// Within an era every quantity is below 2²⁰, so it is computed in 32 bits; only the split into
// eras uses 64-bit arithmetic. This keeps the code cheap for Kani as well as exact.

/// The number of days from 1970-01-01 to the civil date `y`-`m`-`d` (Hinnant).
///
/// `m` is 1 to 12 and `d` is 1 to the length of the month; for any other `m` or `d` the result is
/// unspecified, but the call never panics.
pub fn days_from_civil(y: i64, m: u8, d: u8) -> i64 {
    // January and February belong to the previous March-based year.
    let (y, mp) = if m > 2 {
        (y, i32::from(m).saturating_sub(3))
    } else {
        (y.saturating_sub(1), i32::from(m).saturating_add(9))
    };
    let (era, yoe) = floor_div_rem(y, 400);
    let yoe = i32::try_from(yoe).unwrap_or(0); // [0, 399]
    // [0, 365] for a valid date.
    let doy = mp
        .saturating_mul(153)
        .saturating_add(2)
        .wrapping_div(5)
        .saturating_add(i32::from(d))
        .saturating_sub(1);
    // [0, 146_096] for a valid date.
    let doe = i64::from(
        yoe.saturating_mul(365)
            .saturating_add(yoe.wrapping_div(4))
            .saturating_sub(yoe.wrapping_div(100))
            .saturating_add(doy),
    );
    // era × DAYS_PER_ERA + doe − EPOCH_FROM_ERA_START, ordered so that no intermediate overflows
    // when the result fits: for era ≥ 0, five eras move into the remainder (5 × 146,097 >
    // 719,468), so the product stays below the result; for era < 0 the product stays above it.
    if era >= 0 {
        era.saturating_sub(5)
            .saturating_mul(DAYS_PER_ERA)
            .saturating_add(doe.saturating_add(FIVE_ERAS_PAST_EPOCH))
    } else {
        era.saturating_mul(DAYS_PER_ERA)
            .saturating_add(doe.saturating_sub(EPOCH_FROM_ERA_START))
    }
}

/// The civil date `(year, month, day)` of `z` days since 1970-01-01; the inverse of
/// [`days_from_civil`].
pub fn civil_from_days(z: i64) -> (i64, u8, u8) {
    // z + EPOCH_FROM_ERA_START = era × DAYS_PER_ERA + doe, computed without overflowing: z is split
    // into whole eras and a remainder first, and only the remainder is shifted.
    let (whole_eras, rest) = floor_div_rem(z, DAYS_PER_ERA);
    let shifted = u32::try_from(rest).unwrap_or(0).saturating_add(719_468); // [719_468, 865_564]
    let era = whole_eras.saturating_add(i64::from(shifted / 146_097));
    let doe = shifted % 146_097; // [0, 146_096]
    let yoe = doe
        .saturating_sub(doe / 1_460)
        .saturating_add(doe / 36_524)
        .saturating_sub(doe / 146_096)
        / 365; // [0, 399]
    let doy = doe.saturating_sub(
        yoe.saturating_mul(365)
            .saturating_add(yoe / 4)
            .saturating_sub(yoe / 100),
    ); // [0, 365]
    let mp = doy.saturating_mul(5).saturating_add(2) / 153; // [0, 11]
    let d = doy
        .saturating_sub(mp.saturating_mul(153).saturating_add(2) / 5)
        .saturating_add(1); // [1, 31]
    let m = if mp < 10 {
        mp.saturating_add(3)
    } else {
        mp.saturating_sub(9)
    }; // [1, 12]
    // January and February belong to the next civil year.
    let y = era
        .saturating_mul(400)
        .saturating_add(i64::from(yoe))
        .saturating_add(i64::from(m <= 2));
    (y, small(m), small(d))
}

/// The weekday of `days` since 1970-01-01: `(days + 4) mod 7`, Sunday = 0.
pub fn weekday(days: i64) -> u8 {
    // 1970-01-01 was a Thursday.
    let (_, r) = floor_div_rem(days, 7);
    small(u32::try_from(r).unwrap_or(0).saturating_add(4) % 7)
}

/// The floor quotient of `z` by a positive `divisor`, and the remainder in `[0, divisor)`, with a
/// single division. Every step is exact: `|q × divisor| ≤ |z|`, and `q − 1` stays above `i64::MIN`.
pub(crate) fn floor_div_rem(z: i64, divisor: i64) -> (i64, i64) {
    let q = z.checked_div(divisor).unwrap_or(0);
    let r = z.wrapping_sub(q.wrapping_mul(divisor));
    if r < 0 {
        (q.wrapping_sub(1), r.wrapping_add(divisor))
    } else {
        (q, r)
    }
}

/// Narrows a value the arithmetic above keeps within `u8`.
fn small(v: u32) -> u8 {
    u8::try_from(v).unwrap_or(0)
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test code: the deny list guards the crate's own code"
)]
mod tests {
    use super::*;

    fn is_leap(y: i64) -> bool {
        (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
    }

    /// Sakamoto's method: the weekday of a Gregorian date, Sunday = 0.
    fn sakamoto(y: i64, m: u8, d: u8) -> u8 {
        const T: [i64; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
        let y = if m < 3 { y - 1 } else { y };
        let t = T[usize::from(m) - 1];
        let w = (y + y.div_euclid(4) - y.div_euclid(100) + y.div_euclid(400) + t + i64::from(d))
            .rem_euclid(7);
        u8::try_from(w).unwrap()
    }

    fn month_len(y: i64, m: u8) -> u8 {
        match m {
            2 if is_leap(y) => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        }
    }

    #[test]
    fn known_dates() {
        let cases: [(i64, u8, u8, i64); 10] = [
            (1970, 1, 1, 0),
            (1969, 12, 31, -1),
            (1970, 1, 2, 1),
            (2000, 1, 1, 10_957),
            (2000, 2, 29, 11_016),
            (2000, 3, 1, 11_017),
            (2026, 10, 2, 20_728),
            (1900, 3, 1, -25_508),
            (0, 3, 1, -719_468),
            (2100, 2, 28, 47_540),
        ];
        for (y, m, d, z) in cases {
            assert_eq!(days_from_civil(y, m, d), z, "{y}-{m}-{d}");
            assert_eq!(civil_from_days(z), (y, m, d), "{z}");
        }
        assert_eq!(
            FIVE_ERAS_PAST_EPOCH,
            5 * DAYS_PER_ERA - EPOCH_FROM_ERA_START
        );
        // 2100 is not a leap year; 2000 and 2400 are.
        assert_eq!(civil_from_days(47_541), (2100, 3, 1));
        assert_eq!(
            days_from_civil(2400, 3, 1) - days_from_civil(2400, 2, 28),
            2
        );
    }

    #[test]
    fn every_day_of_five_thousand_years() {
        // From 0700-01-01 to 5700-01-01, and back, with its weekday: every day, in order.
        let first = days_from_civil(700, 1, 1);
        let (mut y, mut m, mut d) = (700_i64, 1_u8, 1_u8);
        for z in first..days_from_civil(5_700, 1, 1) {
            assert_eq!(civil_from_days(z), (y, m, d), "{z}");
            assert_eq!(days_from_civil(y, m, d), z, "{y}-{m}-{d}");
            assert_eq!(weekday(z), sakamoto(y, m, d), "{y}-{m}-{d}");
            // The next civil date, by the calendar's rules.
            if d < month_len(y, m) {
                d += 1;
            } else if m < 12 {
                (m, d) = (m + 1, 1);
            } else {
                (y, m, d) = (y + 1, 1, 1);
            }
        }
    }

    #[test]
    fn negative_years_round_trip() {
        for z in [-1_000_000_000_i64, -800_000, -719_469, -719_468, -719_467] {
            let (y, m, d) = civil_from_days(z);
            assert!((1..=12).contains(&m) && (1..=month_len(y, m)).contains(&d));
            assert_eq!(days_from_civil(y, m, d), z);
        }
        assert_eq!(civil_from_days(-719_469), (0, 2, 29)); // year 0 is a leap year
        assert_eq!(days_from_civil(-1, 12, 31), -719_529);
    }

    #[test]
    fn extremes_do_not_panic() {
        for z in [i64::MIN, i64::MIN + 1, i64::MAX - 1, i64::MAX] {
            let (y, m, d) = civil_from_days(z);
            assert!((1..=12).contains(&m) && (1..=31).contains(&d));
            assert_eq!(days_from_civil(y, m, d), z);
            let _ = weekday(z);
        }
        for y in [i64::MIN, i64::MAX] {
            for (m, d) in [(0, 0), (1, 1), (2, 29), (12, 31), (255, 255)] {
                let _ = days_from_civil(y, m, d);
            }
        }
    }

    #[test]
    fn floor_division() {
        assert_eq!(floor_div_rem(0, 7), (0, 0));
        assert_eq!(floor_div_rem(13, 7), (1, 6));
        assert_eq!(floor_div_rem(-1, 7), (-1, 6));
        assert_eq!(floor_div_rem(-7, 7), (-1, 0));
        assert_eq!(floor_div_rem(-8, 7), (-2, 6));
        for z in [i64::MIN, i64::MIN + 1, -1, 0, 1, i64::MAX - 1, i64::MAX] {
            for divisor in [1, 7, 400, 86_400, 146_097] {
                let (q, r) = floor_div_rem(z, divisor);
                assert_eq!(
                    (q, r),
                    (z.div_euclid(divisor), z.rem_euclid(divisor)),
                    "{z} {divisor}"
                );
            }
        }
    }

    #[test]
    fn weekdays() {
        assert_eq!(weekday(0), 4); // 1970-01-01, Thursday
        assert_eq!(weekday(-1), 3); // 1969-12-31, Wednesday
        assert_eq!(weekday(-4), 0); // 1969-12-28, Sunday
        assert_eq!(weekday(days_from_civil(2026, 10, 2)), 5); // Friday
        assert_eq!(weekday(days_from_civil(2000, 1, 1)), 6); // Saturday
        assert_eq!(weekday(days_from_civil(2100, 2, 28)), 0); // Sunday
        for z in -10_000..10_000 {
            assert_eq!(weekday(z + 1), (weekday(z) + 1) % 7);
            assert!(weekday(z) < 7);
        }
        assert_eq!(weekday(i64::MIN), 3);
        assert_eq!(weekday(i64::MAX), 4);
    }
}
