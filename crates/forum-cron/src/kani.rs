//! Kani harnesses for `forum-cron` (design D.12.5).
//!
//! Run with `cargo kani -p forum-cron`. Arithmetic in the harnesses is plain on purpose: Kani checks
//! it for overflow, so a reference that overflowed would fail the harness rather than hide a bug.

use alloc::string::String;
use alloc::vec::Vec;

use crate::civil::{civil_from_days, days_from_civil, weekday};
use crate::cron::{
    CronSpec, DOMS_ALL, DOWS_ALL, HOURS_ALL, MINUTES_ALL, MONTHS_ALL, date_matches, next_slot,
};
use crate::tz::{MAX_OFFSET_S, Transition, TzTable, earliest_utc, offset_at, valid_table};

/// The day numbers the calendar harnesses cover: ±10⁶ days around 1970-01-01.
const DAYS: i64 = 1_000_000;

// The calendar references work in 32 bits, which the years of ±10⁶ days fit.

fn is_leap(y: i32) -> bool {
    y.rem_euclid(4) == 0 && (y.rem_euclid(100) != 0 || y.rem_euclid(400) == 0)
}

fn month_len(y: i32, m: u8) -> u8 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `civil_from_days(days_from_civil(d)) = d` for days within ±10⁶, and the converse: every day
/// number in the range is a valid date that maps back to it.
#[kani::proof]
#[kani::unwind(1)]
fn civil_roundtrip() {
    let y: i32 = kani::any();
    let m: u8 = kani::any();
    let d: u8 = kani::any();
    kani::assume((-1_000..=5_000).contains(&y));
    kani::assume((1..=12).contains(&m) && 1 <= d && d <= month_len(y, m));
    let z = days_from_civil(i64::from(y), m, d);
    kani::assume((-DAYS..=DAYS).contains(&z));
    assert_eq!(civil_from_days(z), (i64::from(y), m, d));

    let z: i64 = kani::any();
    kani::assume((-DAYS..=DAYS).contains(&z));
    let (y, m, d) = civil_from_days(z);
    assert!((1..=12).contains(&m));
    let year = i32::try_from(y).unwrap();
    assert!(1 <= d && d <= month_len(year, m));
    assert_eq!(days_from_civil(y, m, d), z);
}

/// `weekday` matches a reference for a symbolic day: `(days + 4) mod 7` computed with a truncating
/// remainder, for any `i64` (so consecutive days have consecutive weekdays). Day 0, 1970-01-01, was
/// a Thursday; the unit tests check the weekday of every day of 5,000 years against Sakamoto's
/// method on the civil date.
#[kani::proof]
#[kani::unwind(1)]
fn weekday_reference() {
    let days: i64 = kani::any();
    let r = days % 7; // in (−7, 7), with the sign of `days`
    assert_eq!(i64::from(weekday(days)), (r + 7 + 4) % 7);
    assert_eq!(weekday(0), 4);
}

/// A whole-minute offset within ±26 hours.
fn any_offset() -> i32 {
    let minutes: i32 = kani::any();
    kani::assume((-MAX_OFFSET_S / 60..=MAX_OFFSET_S / 60).contains(&minutes));
    minutes * 60
}

/// For tables of at most 2 symbolic transitions: a local time in a gap has no instant
/// (`None`), one in a fold has two and gets the earlier, and any other has exactly one.
#[kani::proof]
#[kani::unwind(4)]
fn tz_earliest() {
    // Instants and local times within ±2⁴⁰ s (about 35,000 years), so that no sum overflows.
    const BOUND: i64 = 1 << 40;
    let count: usize = kani::any();
    kani::assume(count <= 2);
    let first = Transition {
        at: kani::any(),
        offset: any_offset(),
    };
    let second = Transition {
        at: kani::any(),
        offset: any_offset(),
    };
    kani::assume(-BOUND <= first.at && first.at < second.at && second.at <= BOUND);
    let mut transitions = Vec::from([first, second]);
    transitions.truncate(count);
    let tz = TzTable {
        name: String::new(),
        tzdb_version: String::new(),
        initial: any_offset(),
        transitions,
    };
    assert!(valid_table(&tz));
    let local: i64 = kani::any();
    kani::assume((-BOUND..=BOUND).contains(&local));

    // Reference: each span of constant offset [start, end) holds at most one instant whose local
    // time is `local`; count them and keep the earliest.
    let mut spans: Vec<(i64, i64, i32)> = Vec::new();
    let mut start = i64::MIN;
    let mut offset = tz.initial;
    for t in &tz.transitions {
        spans.push((start, t.at, offset));
        start = t.at;
        offset = t.offset;
    }
    spans.push((start, i64::MAX, offset));
    let mut instants = 0_u8;
    let mut earliest: Option<i64> = None;
    for &(start, end, offset) in &spans {
        let u = local - i64::from(offset);
        if start <= u && u < end {
            instants += 1;
            earliest = Some(earliest.map_or(u, |e| e.min(u)));
        }
    }

    let result = earliest_utc(&tz, local);
    if instants == 0 {
        // A gap.
        assert_eq!(result, None);
    } else {
        // The unique instant, or in a fold the earliest of its instants.
        assert!(result.is_some());
        assert_eq!(result, earliest);
    }
    // By the definition: the result has this local time, and no instant before it does.
    if let Some(e) = result {
        assert_eq!(e + i64::from(offset_at(&tz, e)), local);
    }
    let u: i64 = kani::any();
    kani::assume((-2 * BOUND..=2 * BOUND).contains(&u));
    if u + i64::from(offset_at(&tz, u)) == local {
        assert!(result.is_some_and(|e| e <= u));
    }
}

/// Whether the hour and minute of the UTC instant `ms` match the masks.
fn matches(cron: &CronSpec, ms: u64) -> bool {
    let minute_of_day = u32::try_from(ms / 60_000 % 1_440).unwrap();
    let (hour, minute) = (minute_of_day / 60, minute_of_day % 60);
    (cron.hours >> hour) & 1 == 1 && (cron.minutes >> minute) & 1 == 1
}

/// Symbolic hour and minute masks, unrestricted days, UTC: the result is `> after`, matches the
/// masks, and no matching minute lies between.
///
/// Narrowed (design D.12.5): `after` is any instant of the first three days. That covers every
/// time of day, so every start minute of the search within a day and the step to the next day;
/// the arithmetic differs for later days only in the size of the day number. The form over all
/// days is the brute-force reference test of `cron::tests`. With `after` unbounded the harness
/// did not finish within 25 minutes: relating the 64-bit divisions of milliseconds, seconds, days,
/// and minutes is hard for the SAT solver.
#[kani::proof]
#[kani::unwind(64)]
fn cron_next_slot_sound() {
    let minutes: u64 = kani::any();
    let hours: u32 = kani::any();
    kani::assume(minutes != 0 && minutes & !MINUTES_ALL == 0);
    kani::assume(hours != 0 && hours & !HOURS_ALL == 0);
    let cron = CronSpec {
        minutes,
        hours,
        doms: DOMS_ALL,
        months: MONTHS_ALL,
        dows: DOWS_ALL,
        dom_any: true,
        dow_any: true,
    };
    let tz = TzTable {
        name: String::from("UTC"),
        tzdb_version: String::new(),
        initial: 0,
        transitions: Vec::new(),
    };
    let after: u64 = kani::any();
    kani::assume(after < 3 * 86_400_000);

    let slot = next_slot(&cron, &tz, after);
    // Every day is allowed and both masks are non-empty, so a slot comes within two days.
    assert!(slot.is_some());
    let s = slot.unwrap();
    assert!(s > after && s - after <= 2 * 86_400_000);
    assert!(s % 60_000 == 0);
    assert!(matches(&cron, s));
    let minute: u64 = kani::any();
    kani::assume(minute < u64::MAX / 60_000);
    let between = minute * 60_000;
    kani::assume(after < between && between < s);
    assert!(!matches(&cron, between));
}

/// The day rule of `date_matches`, for symbolic masks (stray bits included), both day flags, and
/// every month, day of month, and day of week: the month must match, and then both day fields if
/// either flag is set (Vixie cron), or either field if neither is (POSIX). With every value of
/// every day field allowed every date matches, so the shortcut of `day_matches` for that case
/// changes nothing.
///
/// `day_matches` is `date_matches` of `civil_from_days` and `weekday`, which `civil_roundtrip` and
/// `weekday_reference` check; `cron_next_slot_sound` allows every day, so this harness is where the
/// day rule meets the solver. The same harness over a symbolic day within ±10⁶ took 573 s.
#[kani::proof]
#[kani::unwind(1)]
fn cron_day_rule() {
    let cron = CronSpec {
        minutes: kani::any(),
        hours: kani::any(),
        doms: kani::any(),
        months: kani::any(),
        dows: kani::any(),
        dom_any: kani::any(),
        dow_any: kani::any(),
    };
    let (month, dom, dow): (u8, u8, u8) = (kani::any(), kani::any(), kani::any());
    kani::assume((1..=12).contains(&month) && (1..=31).contains(&dom) && dow <= 6);
    let month_ok = (cron.months >> month) & 1 == 1;
    let dom_ok = (cron.doms >> dom) & 1 == 1;
    let dow_ok = (cron.dows >> dow) & 1 == 1;
    let expected = month_ok
        && match (cron.dom_any || cron.dow_any, dom_ok, dow_ok) {
            (_, true, true) => true,
            (true, _, _) => false,
            (false, dom_ok, dow_ok) => dom_ok || dow_ok,
        };
    let got = date_matches(&cron, month, dom, dow);
    assert_eq!(got, expected);
    if cron.months & MONTHS_ALL == MONTHS_ALL
        && cron.doms & DOMS_ALL == DOMS_ALL
        && cron.dows & DOWS_ALL == DOWS_ALL
    {
        assert!(got);
    }
}
