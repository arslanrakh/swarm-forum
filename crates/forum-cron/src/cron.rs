//! Five-field cron expressions and their slots.
//!
//! A cron expression has five fields: minute (0–59), hour (0–23), day of month (1–31), month
//! (1–12), and day of week (0–7, where both 0 and 7 are Sunday). Each field is a comma-separated
//! list of items, and each item is `*`, a value `a`, a range `a-b`, or a step `*/n` or `a-b/n`
//! (steps are a common extension of POSIX, A-45). There is no `?`, `L`, `W`, or `#`.
//!
//! A value is a decimal number or, in the month and day-of-week fields, a three-letter English name
//! in any case, as in cronie: `JAN` to `DEC` are 1 to 12, and `SUN` to `SAT` are 0 to 6. A name
//! stands for its number wherever a number may, so `MON-FRI/2` is `1-5/2`; a step is always a
//! number. A day-of-week range that ends on Sunday after a later start runs to 7, as in cronie:
//! `FRI-SUN` and `5-0` are `5-7`, and `MON-SUN` is every day. This departs from cronie twice. The
//! rule holds for a stepped range too, so `FRI-SUN/2` is Friday and Sunday, where cronie reads it
//! as no day at all. And any other backwards range is refused, `X-0` in the other fields included;
//! cronie accepts a backwards range silently, as no value, except that it reads an unstepped `X-0`
//! as `X-7` in every field.
//!
//! Days follow Vixie cron (and cronie). If either day field starts with `*` (`*`, `*/2`, `*,5`), a
//! day must match both the day of month and the day of week; otherwise a day matches if either
//! does (POSIX). A field that starts with `*` still allows only its own values, so `0 9 */2 * 1-5`
//! fires on odd-dated weekdays, while `0 0 13 * 5` fires on every 13th and every Friday.

use alloc::string::String;

use crate::civil::{civil_from_days, floor_div_rem, weekday};
use crate::tz::{DAY_S, MAX_OFFSET_S, TzTable, earliest_utc, local_time, offset_at, valid_table};

/// The most local days [`next_slot`] searches.
pub const MAX_SEARCH_DAYS: u32 = 3_000;

/// Minutes 0..=59.
pub(crate) const MINUTES_ALL: u64 = 0x0FFF_FFFF_FFFF_FFFF;
/// Hours 0..=23.
pub(crate) const HOURS_ALL: u32 = 0x00FF_FFFF;
/// Days of the month 1..=31.
pub(crate) const DOMS_ALL: u32 = 0xFFFF_FFFE;
/// Months 1..=12.
pub(crate) const MONTHS_ALL: u16 = 0x1FFE;
/// Days of the week 0..=6.
pub(crate) const DOWS_ALL: u8 = 0x7F;

/// A parsed 5-field cron expression, one bit per allowed value.
///
/// A local day matches if its month does and, when `dom_any` or `dow_any` is set, both its day of
/// month and its day of week do; when neither is set, either does (POSIX, and Vixie cron).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CronSpec {
    /// Bits 0..=59.
    pub minutes: u64,
    /// Bits 0..=23.
    pub hours: u32,
    /// Bits 1..=31.
    pub doms: u32,
    /// Bits 1..=12.
    pub months: u16,
    /// Bits 0..=6, Sunday = 0 (7 is accepted as Sunday when parsing).
    pub dows: u8,
    /// The day-of-month field starts with `*` (`*`, `*/n`, or a list whose first item is one of
    /// these), so a day must match both day fields (Vixie cron). The field's values, in `doms`,
    /// still apply: for `*/2` they are the odd days.
    pub dom_any: bool,
    /// The day-of-week field starts with `*`, with the same effect.
    pub dow_any: bool,
}

/// Why a cron expression could not be parsed; `index` is the 0-based field.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CronError {
    /// The expression does not have exactly five whitespace-separated fields.
    FieldCount,
    /// A field is not a list of `*`, values (numbers, or the field's names), ranges, and steps.
    Field { index: u8 },
    /// A value lies outside its field's range, or a range runs backwards.
    Range { index: u8 },
    /// A step is not a positive number, or follows something other than `*` or a range.
    Step { index: u8 },
}

/// One field of a cron expression: its place, its range, and its names.
#[derive(Clone, Copy)]
struct FieldSpec {
    /// The 0-based place of the field, as [`CronError`] reports it.
    index: u8,
    min: u32,
    max: u32,
    /// The names of `min`, `min + 1`, and so on, matched in any case.
    names: &'static [&'static str],
    /// A range that ends on 0 (Sunday) after a later start runs to 7 instead (cronie), with or
    /// without a step (cronie: without one only).
    sunday_ends_at_seven: bool,
}

const MINUTE: FieldSpec = FieldSpec {
    index: 0,
    min: 0,
    max: 59,
    names: &[],
    sunday_ends_at_seven: false,
};
const HOUR: FieldSpec = FieldSpec {
    index: 1,
    min: 0,
    max: 23,
    names: &[],
    sunday_ends_at_seven: false,
};
const DAY_OF_MONTH: FieldSpec = FieldSpec {
    index: 2,
    min: 1,
    max: 31,
    names: &[],
    sunday_ends_at_seven: false,
};
const MONTH: FieldSpec = FieldSpec {
    index: 3,
    min: 1,
    max: 12,
    names: &[
        "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
    ],
    sunday_ends_at_seven: false,
};
const DAY_OF_WEEK: FieldSpec = FieldSpec {
    index: 4,
    min: 0,
    max: 7,
    names: &["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"],
    sunday_ends_at_seven: true,
};

/// Parses a 5-field cron expression.
///
/// Fields are separated by ASCII whitespace; leading and trailing whitespace is ignored. Fields are
/// checked in order, and the error names the first field that fails.
pub fn parse_cron(text: &str) -> Result<CronSpec, CronError> {
    let mut fields = text.split_ascii_whitespace();
    let (Some(minute), Some(hour), Some(dom), Some(month), Some(dow), None) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return Err(CronError::FieldCount);
    };
    let minutes = parse_field(minute, MINUTE)?;
    let hours = parse_field(hour, HOUR)?;
    let doms = parse_field(dom, DAY_OF_MONTH)?;
    let months = parse_field(month, MONTH)?;
    let dows = parse_field(dow, DAY_OF_WEEK)?;
    // 7 is Sunday, like 0.
    let sunday = dows.checked_shr(7).unwrap_or(0) & 1;
    Ok(CronSpec {
        minutes,
        hours: narrow(hours, HOUR)?,
        doms: narrow(doms, DAY_OF_MONTH)?,
        months: narrow(months, MONTH)?,
        dows: narrow((dows | sunday) & u64::from(DOWS_ALL), DAY_OF_WEEK)?,
        // Vixie cron looks only at the first character of a day field.
        dom_any: dom.starts_with('*'),
        dow_any: dow.starts_with('*'),
    })
}

/// The bits of one field: the union of its comma-separated items.
fn parse_field(text: &str, field: FieldSpec) -> Result<u64, CronError> {
    text.split(',')
        .try_fold(0_u64, |bits, item| Ok(bits | parse_item(item, field)?))
}

/// The bits of one item: `*`, `a`, `a-b`, `*/n`, or `a-b/n`.
fn parse_item(item: &str, field: FieldSpec) -> Result<u64, CronError> {
    let index = field.index;
    let (range, step) = match item.split_once('/') {
        Some((range, step)) => (range, Some(step)),
        None => (item, None),
    };
    let (first, last) = if range == "*" {
        (field.min, field.max)
    } else if let Some((a, b)) = range.split_once('-') {
        let (first, last) = (value(a, field)?, value(b, field)?);
        if field.sunday_ends_at_seven && last == 0 && first > last {
            (first, 7)
        } else {
            (first, last)
        }
    } else {
        let v = value(range, field)?;
        if step.is_some() {
            // A step needs `*` or a range to step through.
            return Err(CronError::Step { index });
        }
        (v, v)
    };
    if first < field.min || last > field.max || first > last {
        return Err(CronError::Range { index });
    }
    let step = match step {
        None => 1,
        Some(text) => number(text)
            .filter(|&n| n > 0)
            .ok_or(CronError::Step { index })?,
    };
    let mut bits = 0_u64;
    let mut v = first;
    while v <= last {
        bits |= 1_u64.checked_shl(v).unwrap_or(0);
        match v.checked_add(step) {
            Some(next) => v = next,
            None => break,
        }
    }
    Ok(bits)
}

/// A value in a field: a decimal number, or one of the field's names in any case. `Field` if
/// `text` is neither, `Range` if the number does not fit in a `u32`.
fn value(text: &str, field: FieldSpec) -> Result<u32, CronError> {
    let index = field.index;
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        return number(text).ok_or(CronError::Range { index });
    }
    field
        .names
        .iter()
        .position(|name| name.eq_ignore_ascii_case(text))
        .and_then(|i| u32::try_from(i).ok())
        .and_then(|i| field.min.checked_add(i))
        .ok_or(CronError::Field { index })
}

/// A non-empty decimal number of ASCII digits that fits in a `u32`.
fn number(text: &str) -> Option<u32> {
    if text.is_empty() {
        return None;
    }
    text.bytes().try_fold(0_u32, |n, b| {
        let digit = b
            .is_ascii_digit()
            .then(|| u32::from(b.wrapping_sub(b'0')))?;
        n.checked_mul(10)?.checked_add(digit)
    })
}

/// A field's bits in the width of its [`CronSpec`] field.
fn narrow<T: TryFrom<u64>>(bits: u64, field: FieldSpec) -> Result<T, CronError> {
    T::try_from(bits).map_err(|_| CronError::Range { index: field.index })
}

/// The first slot strictly after `after_ms`: the smallest UTC instant s > after_ms such that
/// s = earliest(local(d, h, m)) for a local date d and time h:m matching the cron, where
/// earliest() maps a local time to its first UTC instant, and a local time that does not exist
/// (a DST gap) has no instant. A local time that occurs twice (a fold) fires once, at its
/// earlier instant. Searches at most MAX_SEARCH_DAYS local days; None if there is no slot.
///
/// The local days searched are the local day of `after_ms` and the `MAX_SEARCH_DAYS − 1` days
/// after it. The search walks the table's spans of constant offset in UTC order, and the local
/// minutes of each span in ascending order, so the first match is the smallest instant even where
/// a fold reaches back past an earlier gap. A table that fails [`valid_table`] has no slot.
pub fn next_slot(cron: &CronSpec, tz: &TzTable, after_ms: u64) -> Option<u64> {
    if !valid_table(tz) {
        return None;
    }
    // A slot is a whole second (a whole minute in fact), so s > after_ms ⟺ s ≥ first second.
    let after_s = i64::try_from(after_ms / 1_000).ok()?;
    let first_s = after_s.checked_add(1)?;
    // Local times from `limit` on lie beyond the search; no instant from `stop` on has a local
    // time before `limit`.
    let limit = floor_div_rem(local_time(tz, after_s)?, DAY_S)
        .0
        .checked_add(i64::from(MAX_SEARCH_DAYS))?
        .checked_mul(DAY_S)?;
    let stop = limit.checked_add(i64::from(MAX_OFFSET_S))?;
    let transitions = &tz.transitions;
    // The span of constant `offset` being searched ends at the transition `next`, if any. Within
    // it the search stands at local day `day`, from its minute `minute` on. Each step looks at one
    // day or one candidate minute, and moves forward.
    let mut next = transitions.partition_point(|t| t.at <= first_s);
    let mut offset = i64::from(offset_at(tz, first_s));
    let (mut day, mut minute) = day_and_minute(first_s.checked_add(offset)?);
    loop {
        // The span's local times end at `to` (exclusive), or at the search's limit.
        let end = transitions.get(next).map(|t| t.at);
        let to = end
            .and_then(|e| e.checked_add(offset))
            .map_or(limit, |e| e.min(limit));
        let day_start = day.checked_mul(DAY_S)?;
        if day_start < to {
            let found = if day_matches(cron, day) {
                first_minute(cron, minute)
            } else {
                None
            };
            if let Some(m) = found {
                let local = day_start.checked_add(i64::from(m).checked_mul(60)?)?;
                if local < to {
                    let utc = local.checked_sub(offset)?;
                    // A local time that an earlier span already had (a fold) fired there, or lay
                    // before `after_ms`: it does not fire again.
                    if earliest_utc(tz, local) == Some(utc) {
                        return u64::try_from(utc).ok()?.checked_mul(1_000);
                    }
                    minute = m.checked_add(1)?;
                    continue;
                }
            }
            // Nothing more on this day.
            day = day.checked_add(1)?;
            minute = 0;
            continue;
        }
        // The span is searched; the next one starts at the transition `next`.
        let end = end?;
        if end >= stop {
            return None;
        }
        offset = i64::from(transitions.get(next)?.offset);
        (day, minute) = day_and_minute(end.checked_add(offset)?);
        next = next.checked_add(1)?;
    }
}

/// The local day of `local` (local seconds), and the first whole minute of that day at or after
/// `local` (1440: none is left that day).
fn day_and_minute(local: i64) -> (i64, u32) {
    let (day, second) = floor_div_rem(local, DAY_S);
    let second = u32::try_from(second).unwrap_or(0); // [0, 86_399]
    (day, second.saturating_add(59) / 60)
}

/// The first minute of the day at or after `from` (0..=1440) whose hour and minute match.
fn first_minute(cron: &CronSpec, from: u32) -> Option<u32> {
    let minutes = cron.minutes & MINUTES_ALL;
    let hours = cron.hours & HOURS_ALL;
    let hour = from / 60;
    // The rest of the current hour.
    if has(u64::from(hours), hour) {
        let rest = minutes.checked_shr(from % 60).unwrap_or(0);
        if rest != 0 {
            return from.checked_add(rest.trailing_zeros());
        }
    }
    // The first minute of a later hour.
    let next_hour = hour.checked_add(1)?;
    let later = hours.checked_shr(next_hour).unwrap_or(0);
    if later == 0 || minutes == 0 {
        return None;
    }
    next_hour
        .checked_add(later.trailing_zeros())?
        .checked_mul(60)?
        .checked_add(minutes.trailing_zeros())
}

/// Whether the cron matches the local day `day` (days since 1970-01-01): its month, and its day
/// of month and day of week by the rule of [`CronSpec`].
fn day_matches(cron: &CronSpec, day: i64) -> bool {
    // Every value of every day field allowed: every date matches (the harness `cron_day_rule`).
    if cron.months & MONTHS_ALL == MONTHS_ALL
        && cron.doms & DOMS_ALL == DOMS_ALL
        && cron.dows & DOWS_ALL == DOWS_ALL
    {
        return true;
    }
    let (_, month, dom) = civil_from_days(day);
    date_matches(cron, month, dom, weekday(day))
}

/// Whether the cron matches a date by its month (1–12), day of month (1–31), and day of week (0–6,
/// Sunday = 0): its month must match, and then both day fields if `dom_any` or `dow_any` is set
/// (Vixie cron), or either if neither is (POSIX).
pub(crate) fn date_matches(cron: &CronSpec, month: u8, dom: u8, dow: u8) -> bool {
    let month_ok = has(u64::from(cron.months & MONTHS_ALL), u32::from(month));
    let dom_ok = has(u64::from(cron.doms & DOMS_ALL), u32::from(dom));
    let dow_ok = has(u64::from(cron.dows & DOWS_ALL), u32::from(dow));
    month_ok
        && if cron.dom_any || cron.dow_any {
            dom_ok && dow_ok
        } else {
            dom_ok || dow_ok
        }
}

/// Whether bit `i` of `mask` is set.
fn has(mask: u64, i: u32) -> bool {
    mask.checked_shr(i).is_some_and(|m| m & 1 == 1)
}

/// The name of the slot at `utc_ms`: "YYYYMMDDTHHMMZ".
pub fn slot_name(utc_ms: u64) -> String {
    let minutes = utc_ms / 60_000;
    let day = i64::try_from(minutes / 1_440).unwrap_or(i64::MAX);
    let minute_of_day = minutes % 1_440;
    let (y, m, d) = civil_from_days(day);
    alloc::format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}Z",
        minute_of_day / 60,
        minute_of_day % 60
    )
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
    use crate::tz::Transition;
    use alloc::format;
    use alloc::string::ToString;
    use alloc::vec::Vec;

    fn table(name: &str, initial: i32, transitions: &[(i64, i32)]) -> TzTable {
        TzTable {
            name: String::from(name),
            tzdb_version: String::from("2026c"),
            initial,
            transitions: transitions
                .iter()
                .map(|&(at, offset)| Transition { at, offset })
                .collect(),
        }
    }

    fn utc() -> TzTable {
        table("UTC", 0, &[])
    }

    // The tables below are copied from tzdb for the years the tests use.

    fn berlin() -> TzTable {
        table(
            "Europe/Berlin",
            3_600,
            &[(1_774_746_000, 7_200), (1_792_890_000, 3_600)],
        )
    }

    fn new_york() -> TzTable {
        table(
            "America/New_York",
            -18_000,
            &[(1_772_953_200, -14_400), (1_793_512_800, -18_000)],
        )
    }

    /// DST is +11:00, standard time +10:30: the clocks move by 30 minutes.
    fn lord_howe() -> TzTable {
        table(
            "Australia/Lord_Howe",
            39_600,
            &[(1_775_314_800, 37_800), (1_791_041_400, 39_600)],
        )
    }

    /// +05:30 until 1986, then +05:45.
    fn kathmandu() -> TzTable {
        table("Asia/Kathmandu", 19_800, &[(504_901_800, 20_700)])
    }

    /// Samoa moved across the date line: 2011-12-30 did not happen there.
    fn apia() -> TzTable {
        table(
            "Pacific/Apia",
            -36_000,
            &[
                (1_301_752_800, -39_600),
                (1_316_872_800, -36_000),
                (1_325_239_200, 50_400),
                (1_333_202_400, 46_800),
                (1_348_927_200, 50_400),
            ],
        )
    }

    /// Milliseconds of a UTC civil date and time.
    fn at(y: i64, mo: u8, d: u8, h: u64, mi: u64) -> u64 {
        let days = u64::try_from(days_from_civil(y, mo, d)).unwrap();
        (days * 1_440 + h * 60 + mi) * 60_000
    }

    fn cron(text: &str) -> CronSpec {
        parse_cron(text).unwrap()
    }

    fn next(text: &str, tz: &TzTable, after: u64) -> Option<u64> {
        next_slot(&cron(text), tz, after)
    }

    // --- Parsing ---------------------------------------------------------------------------------

    fn bits(values: &[u32]) -> u64 {
        values.iter().fold(0, |b, &v| b | 1 << v)
    }

    #[test]
    fn parses_every_form() {
        assert_eq!(
            cron("* * * * *"),
            CronSpec {
                minutes: MINUTES_ALL,
                hours: HOURS_ALL,
                doms: DOMS_ALL,
                months: MONTHS_ALL,
                dows: DOWS_ALL,
                dom_any: true,
                dow_any: true,
            }
        );
        let c = cron("*/15 9-17 * * 1-5");
        assert_eq!(c.minutes, bits(&[0, 15, 30, 45]));
        assert_eq!(
            u64::from(c.hours),
            bits(&[9, 10, 11, 12, 13, 14, 15, 16, 17])
        );
        assert_eq!(u64::from(c.dows), bits(&[1, 2, 3, 4, 5]));
        assert!(c.dom_any && !c.dow_any);
        let c = cron("0,30 8,20 1,15 1-12/3 *");
        assert_eq!(c.minutes, bits(&[0, 30]));
        assert_eq!(u64::from(c.hours), bits(&[8, 20]));
        assert_eq!(u64::from(c.doms), bits(&[1, 15]));
        assert_eq!(u64::from(c.months), bits(&[1, 4, 7, 10]));
        assert!(!c.dom_any && c.dow_any);
        assert_eq!(cron("5-59/20 * * * *").minutes, bits(&[5, 25, 45]));
        assert_eq!(
            cron("1-3,10-12/2,*/30 * * * *").minutes,
            bits(&[0, 1, 2, 3, 10, 12, 30])
        );
        assert_eq!(cron("*/100 * * * *").minutes, bits(&[0]));
        assert_eq!(cron("00 01 01 01 00"), cron("0 1 1 1 0"));
        assert_eq!(cron("\t 0  0\n* *   *  "), cron("0 0 * * *"));
        assert_eq!(cron("59 23 31 12 6").minutes, bits(&[59]));
    }

    #[test]
    fn seven_is_sunday() {
        let sunday = u8::try_from(bits(&[0])).unwrap();
        assert_eq!(cron("0 0 * * 7").dows, sunday);
        assert_eq!(cron("0 0 * * 0,7").dows, sunday);
        assert_eq!(cron("0 0 * * 0").dows, sunday);
        assert_eq!(u64::from(cron("0 0 * * 5-7").dows), bits(&[0, 5, 6]));
        assert_eq!(cron("0 0 * * 0-7").dows, DOWS_ALL);
        assert_eq!(u64::from(cron("0 0 * * */2").dows), bits(&[0, 2, 4, 6]));
        assert!(!cron("0 0 * * 0-7").dow_any);
        // A range that ends on Sunday after a later start runs to 7 (cronie), in either spelling.
        assert_eq!(u64::from(cron("0 0 * * 5-0").dows), bits(&[0, 5, 6]));
        assert_eq!(cron("0 0 * * 1-0").dows, DOWS_ALL);
        // With a step too, where cronie reads the range as no day.
        assert_eq!(cron("0 0 * * 6-0/1"), cron("0 0 * * 6-7"));
        assert_eq!(cron("0 0 * * 1-0/2"), cron("0 0 * * 1,3,5,7"));
        assert_eq!(u64::from(cron("0 0 * * FRI-SUN/2").dows), bits(&[0, 5]));
        assert_eq!(cron("0 0 * * 0-0"), cron("0 0 * * 0"));
        assert_eq!(cron("0 0 * * 7-0"), cron("0 0 * * 0"));
        // 2026-10-04 is a Sunday.
        assert_eq!(
            next("0 0 * * 7", &utc(), at(2026, 10, 2, 0, 0)),
            Some(at(2026, 10, 4, 0, 0))
        );
    }

    #[test]
    fn rejects_malformed_expressions() {
        let cases: &[(&str, CronError)] = &[
            ("", CronError::FieldCount),
            ("   ", CronError::FieldCount),
            ("* * * *", CronError::FieldCount),
            ("* * * * * *", CronError::FieldCount),
            ("x * * * *", CronError::Field { index: 0 }),
            // Names: only months in the month field and days in the day-of-week field, only the
            // three-letter English abbreviations, and never as a step.
            ("JAN * * * *", CronError::Field { index: 0 }),
            ("* MON * * *", CronError::Field { index: 1 }),
            ("* * JAN * *", CronError::Field { index: 2 }),
            ("* * * MON *", CronError::Field { index: 3 }),
            ("* * * * JAN", CronError::Field { index: 4 }),
            ("* * * JANUARY *", CronError::Field { index: 3 }),
            ("* * * * MONDAY", CronError::Field { index: 4 }),
            ("* * * * MO", CronError::Field { index: 4 }),
            ("* * * * M0N", CronError::Field { index: 4 }),
            ("* * * * MÖN", CronError::Field { index: 4 }),
            ("* * * * MON-", CronError::Field { index: 4 }),
            ("* * * * *-FRI", CronError::Field { index: 4 }),
            ("* * * * MON-TUE-WED", CronError::Field { index: 4 }),
            ("* * * DEC-JAN *", CronError::Range { index: 3 }),
            ("* * * * FRI-MON", CronError::Range { index: 4 }),
            ("* * * * SAT-0-SUN", CronError::Field { index: 4 }),
            ("* * * * */MON", CronError::Step { index: 4 }),
            ("* * * JAN/2 *", CronError::Step { index: 3 }),
            ("* * * * MON/2", CronError::Step { index: 4 }),
            ("* * * JAN-DEC/FEB *", CronError::Step { index: 3 }),
            // The Sunday rule of ranges is the day of week's alone (cronie reads `X-0` as `X-7` in
            // any field).
            ("30-0 * * * *", CronError::Range { index: 0 }),
            ("* 5-0 * * *", CronError::Range { index: 1 }),
            ("* * 5-0 * *", CronError::Range { index: 2 }),
            ("* * * 5-0 *", CronError::Range { index: 3 }),
            ("* * * * 8-0", CronError::Range { index: 4 }),
            // No `?`, `L`, `W`, or `#`, with numbers or names.
            ("* * ? * *", CronError::Field { index: 2 }),
            ("* * * * ?", CronError::Field { index: 4 }),
            ("* * 15W * *", CronError::Field { index: 2 }),
            ("* * LW * *", CronError::Field { index: 2 }),
            ("* * * * 5L", CronError::Field { index: 4 }),
            ("* * * * FRIL", CronError::Field { index: 4 }),
            ("* * * * MON#1", CronError::Field { index: 4 }),
            ("1,,2 * * * *", CronError::Field { index: 0 }),
            ("1, * * * *", CronError::Field { index: 0 }),
            ("1- * * * *", CronError::Field { index: 0 }),
            ("-1 * * * *", CronError::Field { index: 0 }),
            ("1-2-3 * * * *", CronError::Field { index: 0 }),
            ("+1 * * * *", CronError::Field { index: 0 }),
            ("* ? * * *", CronError::Field { index: 1 }),
            ("* * L * *", CronError::Field { index: 2 }),
            ("* * * * 5#2", CronError::Field { index: 4 }),
            ("*-5 * * * *", CronError::Field { index: 0 }),
            ("x/2 * * * *", CronError::Field { index: 0 }),
            ("60 * * * *", CronError::Range { index: 0 }),
            ("* 24 * * *", CronError::Range { index: 1 }),
            ("* * 0 * *", CronError::Range { index: 2 }),
            ("* * 32 * *", CronError::Range { index: 2 }),
            ("* * * 0 *", CronError::Range { index: 3 }),
            ("* * * 13 *", CronError::Range { index: 3 }),
            ("* * * * 8", CronError::Range { index: 4 }),
            ("5-3 * * * *", CronError::Range { index: 0 }),
            ("99999999999 * * * *", CronError::Range { index: 0 }),
            ("*/0 * * * *", CronError::Step { index: 0 }),
            ("*/ * * * *", CronError::Step { index: 0 }),
            ("*/x * * * *", CronError::Step { index: 0 }),
            ("*/2/3 * * * *", CronError::Step { index: 0 }),
            ("1-5/0 * * * *", CronError::Step { index: 0 }),
            ("5/10 * * * *", CronError::Step { index: 0 }),
            ("*/99999999999 * * * *", CronError::Step { index: 0 }),
            // The first field that fails is reported.
            ("60 24 * * *", CronError::Range { index: 0 }),
            ("0 0 * 13 x", CronError::Range { index: 3 }),
        ];
        for (text, error) in cases {
            assert_eq!(parse_cron(text), Err(error.clone()), "{text:?}");
        }
    }

    // --- Slots in UTC ----------------------------------------------------------------------------

    #[test]
    fn slots_in_utc() {
        let tz = utc();
        let after = at(2026, 10, 2, 10, 7) + 30_000;
        assert_eq!(
            next("*/15 * * * *", &tz, after),
            Some(at(2026, 10, 2, 10, 15))
        );
        assert_eq!(next("* * * * *", &tz, after), Some(at(2026, 10, 2, 10, 8)));
        // Weekdays at 09:00: Friday 2026-10-02, then Monday.
        assert_eq!(
            next("0 9 * * 1-5", &tz, at(2026, 10, 2, 9, 0)),
            Some(at(2026, 10, 5, 9, 0))
        );
        assert_eq!(
            next("30 2 1 1 *", &tz, at(2026, 10, 2, 0, 0)),
            Some(at(2027, 1, 1, 2, 30))
        );
        assert_eq!(
            next("0 0 1 * *", &tz, at(2026, 12, 15, 0, 0)),
            Some(at(2027, 1, 1, 0, 0))
        );
        assert_eq!(
            next("59 23 31 12 *", &tz, 0),
            Some(at(1970, 12, 31, 23, 59))
        );
        assert_eq!(next("0 0 * * *", &tz, 0), Some(at(1970, 1, 2, 0, 0)));
    }

    #[test]
    fn the_first_slot_strictly_after() {
        let tz = utc();
        let slot = at(2026, 10, 2, 12, 0);
        assert_eq!(next("0 12 * * *", &tz, slot - 1), Some(slot));
        assert_eq!(next("0 12 * * *", &tz, slot), Some(at(2026, 10, 3, 12, 0)));
        assert_eq!(
            next("0 12 * * *", &tz, slot + 1),
            Some(at(2026, 10, 3, 12, 0))
        );
        assert_eq!(next("* * * * *", &tz, slot), Some(slot + 60_000));
        assert_eq!(next("* * * * *", &tz, slot + 59_999), Some(slot + 60_000));
    }

    #[test]
    fn day_of_month_or_day_of_week() {
        let tz = utc();
        // Neither field starts with `*`: the 13th or a Friday (POSIX). 2026-10-02 and 2026-10-09
        // are Fridays.
        assert_eq!(
            next("0 0 13 * 5", &tz, at(2026, 10, 1, 0, 0)),
            Some(at(2026, 10, 2, 0, 0))
        );
        assert_eq!(
            next("0 0 13 * 5", &tz, at(2026, 10, 2, 0, 0)),
            Some(at(2026, 10, 9, 0, 0))
        );
        assert_eq!(
            next("0 0 13 * 5", &tz, at(2026, 10, 9, 0, 0)),
            Some(at(2026, 10, 13, 0, 0))
        );
        // One field is `*`: the other decides.
        assert_eq!(
            next("0 0 13 * *", &tz, at(2026, 10, 1, 0, 0)),
            Some(at(2026, 10, 13, 0, 0))
        );
        assert_eq!(
            next("0 0 * * 5", &tz, at(2026, 10, 2, 0, 0)),
            Some(at(2026, 10, 9, 0, 0))
        );
        // A field that starts with `*`, steps included, makes both fields apply (Vixie cron): `*/1`
        // allows every day of the month, so only Fridays match.
        assert_eq!(
            next("0 0 */1 * 5", &tz, at(2026, 10, 2, 0, 0)),
            Some(at(2026, 10, 9, 0, 0))
        );
        // Likewise on the day of week: the 13th, if it falls on Sunday, Tuesday, Thursday, or
        // Saturday. 2026-10-13 is a Tuesday; by the POSIX "either" rule Saturday 2026-10-03 would
        // come first.
        assert_eq!(
            next("0 0 13 * */2", &tz, at(2026, 10, 1, 0, 0)),
            Some(at(2026, 10, 13, 0, 0))
        );
        // Only the first character counts: with `*,5` both fields apply, and with `5,*` either
        // does (and `5,*` allows every day, here).
        let c = cron("0 0 *,5 * 5");
        assert!(c.dom_any && !c.dow_any);
        assert_eq!(
            next_slot(&c, &tz, at(2026, 10, 2, 0, 0)),
            Some(at(2026, 10, 9, 0, 0))
        );
        let c = cron("0 0 5,* * 5");
        assert!(!c.dom_any && !c.dow_any);
        assert_eq!(
            next_slot(&c, &tz, at(2026, 10, 2, 0, 0)),
            Some(at(2026, 10, 3, 0, 0))
        );
        // A range over every day does not start with `*`, so either field does, as in Vixie cron.
        let c = cron("0 0 1-31 * 5");
        assert!(!c.dom_any && !c.dow_any);
        assert_eq!(
            next_slot(&c, &tz, at(2026, 10, 2, 0, 0)),
            Some(at(2026, 10, 3, 0, 0))
        );
        // The month always applies: Fridays and 13ths in February only.
        assert_eq!(
            next("0 0 13 2 5", &tz, at(2026, 10, 1, 0, 0)),
            Some(at(2027, 2, 5, 0, 0))
        );
    }

    /// The days of October 2026 on which `text`, a cron for midnight, fires in UTC.
    fn days_in_october_2026(text: &str) -> Vec<u64> {
        let (c, tz) = (cron(text), utc());
        let (first, end) = (at(2026, 10, 1, 0, 0), at(2026, 11, 1, 0, 0));
        let mut days = Vec::new();
        let mut after = first - 1;
        while let Some(slot) = next_slot(&c, &tz, after).filter(|&s| s < end) {
            days.push((slot - first) / 86_400_000 + 1);
            after = slot;
        }
        days
    }

    #[test]
    fn the_day_rule_by_hand() {
        // The expected days are read off the calendar of October 2026 by hand, not computed:
        //
        //   Mo Tu We Th Fr Sa Su
        //             1  2  3  4
        //    5  6  7  8  9 10 11
        //   12 13 14 15 16 17 18
        //   19 20 21 22 23 24 25
        //   26 27 28 29 30 31
        let every: Vec<u64> = (1..=31).collect();
        let cases: &[(&str, &[u64])] = &[
            // Neither day field starts with `*`: a day matches if either field does (POSIX).
            ("0 0 13 * 2", &[6, 13, 20, 27]),
            ("0 0 1-10/3 * 0", &[1, 4, 7, 10, 11, 18, 25]),
            (
                "0 0 2-30/2 * 1,3,5",
                &[
                    2, 4, 5, 6, 7, 8, 9, 10, 12, 14, 16, 18, 19, 20, 21, 22, 23, 24, 26, 28, 30,
                ],
            ),
            ("0 0 15,* * 0", every.as_slice()),
            ("0 0 1-31 * 0", every.as_slice()),
            ("0 0 1-31 * 0-6", every.as_slice()),
            // One or both start with `*`: a day must match both fields, each with its own values
            // (Vixie cron).
            ("0 0 * * *", every.as_slice()),
            ("0 0 */10 * *", &[1, 11, 21, 31]),
            ("0 0 */10 * 0", &[11]),
            ("0 0 *,15 * 0", &[4, 11, 18, 25]),
            ("0 0 */15,2 * 0", &[]),
            ("0 0 1-31 * */7", &[4, 11, 18, 25]),
            ("0 0 13 * */3", &[]),
            ("0 0 13 * *,0", &[13]),
            ("0 0 */30 * 4-6", &[1, 31]),
            ("0 0 */31 * THU", &[1]),
            ("0 0 */45 * 5", &[]),
            ("0 0 */30 * SAT-SUN", &[31]),
            ("0 0 */2 * */2", &[1, 3, 11, 13, 15, 17, 25, 27, 29, 31]),
        ];
        for &(text, days) in cases {
            assert_eq!(days_in_october_2026(text), days, "{text}");
        }
    }

    #[test]
    fn a_leap_day_on_a_sunday() {
        // `*/7` is Sunday alone, and it starts with `*`, so 29 February must also be a Sunday: not
        // 2024's (a Thursday) or 2028's (a Tuesday), but 2032's, 2,981 days after 2024-01-01.
        let c = cron("0 0 29 2 */7");
        assert_eq!(u64::from(c.dows), bits(&[0]));
        assert_eq!(
            next_slot(&c, &utc(), at(2024, 1, 1, 0, 0)),
            Some(at(2032, 2, 29, 0, 0))
        );
    }

    #[test]
    fn every_other_day_on_weekdays_is_odd_dated_weekdays() {
        // `*/2` starts with `*`, so the day of month and the day of week must both match: odd days
        // of the month that are Monday to Friday. Friday 2026-10-02 is even; 10-05, 10-07, 10-09,
        // and 10-13 are odd weekdays, and 10-11 is an odd Sunday.
        let tz = utc();
        for text in ["0 9 */2 * 1-5", "0 9 */2 * MON-FRI", "0 9 */2 * mon-fri"] {
            let c = cron(text);
            assert!(c.dom_any && !c.dow_any, "{text}");
            let mut after = at(2026, 10, 2, 10, 0);
            for (month, day) in [(10, 5), (10, 7), (10, 9), (10, 13)] {
                let slot = next_slot(&c, &tz, after);
                assert_eq!(slot, Some(at(2026, month, day, 9, 0)), "{text}");
                assert_eq!(slot, reference(text, &tz, after, 31 * 1_440), "{text}");
                after = slot.unwrap();
            }
        }
    }

    /// The names of months 1 to 12 and of days of the week 0 to 6, written out here rather than
    /// taken from the parser.
    const MONTH_NAMES: [&str; 12] = [
        "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
    ];
    const DAY_NAMES: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

    #[test]
    fn names_stand_for_their_numbers() {
        // Upper, lower, and mixed case.
        let cases = |name: &str| {
            let mixed: String = name
                .chars()
                .enumerate()
                .map(|(i, c)| {
                    if i % 2 == 0 {
                        c.to_ascii_lowercase()
                    } else {
                        c
                    }
                })
                .collect();
            [String::from(name), name.to_ascii_lowercase(), mixed]
        };
        // Every value, range, and stepped range of both fields, each end a name or a number.
        for (a, first) in (1_u32..).zip(MONTH_NAMES) {
            for spelling in cases(first) {
                assert_eq!(
                    cron(&format!("0 0 1 {spelling} *")),
                    cron(&format!("0 0 1 {a} *"))
                );
            }
            for (b, last) in (1_u32..)
                .zip(MONTH_NAMES)
                .skip(usize::try_from(a - 1).unwrap())
            {
                let numbers = cron(&format!("0 0 1 {a}-{b} *"));
                for range in [
                    format!("{first}-{last}"),
                    format!("{a}-{last}"),
                    format!("{}-{b}", first.to_ascii_lowercase()),
                ] {
                    assert_eq!(cron(&format!("0 0 1 {range} *")), numbers, "{range}");
                    assert_eq!(
                        cron(&format!("0 0 1 {range}/2 *")),
                        cron(&format!("0 0 1 {a}-{b}/2 *")),
                        "{range}/2"
                    );
                }
            }
        }
        for (a, first) in (0_u32..).zip(DAY_NAMES) {
            for spelling in cases(first) {
                assert_eq!(
                    cron(&format!("0 0 * * {spelling}")),
                    cron(&format!("0 0 * * {a}"))
                );
            }
            for (b, last) in (0_u32..).zip(DAY_NAMES) {
                // A range that ends on Sunday after a later start runs to 7.
                let end = if b == 0 && a > 0 { 7 } else { b };
                if a > end {
                    assert_eq!(
                        parse_cron(&format!("0 0 * * {first}-{last}")),
                        Err(CronError::Range { index: 4 })
                    );
                    continue;
                }
                for step in [1, 2, 3] {
                    assert_eq!(
                        cron(&format!("0 0 * * {first}-{last}/{step}")),
                        cron(&format!("0 0 * * {a}-{end}/{step}")),
                        "{first}-{last}/{step}"
                    );
                }
            }
        }
        // `SUN` is 0 and may end a range after any day; 7 stays a number.
        assert_eq!(cron("0 0 * * MON-SUN").dows, DOWS_ALL);
        assert_eq!(cron("0 0 * * Sat-Sun"), cron("0 0 * * 0,6"));
        assert_eq!(cron("0 0 * * FRI-7"), cron("0 0 * * 5-7"));
        assert_eq!(cron("0 0 * * SUN-SAT").dows, DOWS_ALL);
        // Names mix with numbers, `*`, and steps in lists.
        assert_eq!(
            cron("0 9 * jan-mar,6,Sep-DEC/2 Mon-Fri,sun"),
            cron("0 9 * 1-3,6,9-12/2 1-5,0")
        );
        assert_eq!(cron("0 0 * */3,DEC *"), cron("0 0 * 1,4,7,10,12 *"));
        // A name restricts its field like a number does.
        let c = cron("0 0 13 * FRI");
        assert!(!c.dom_any && !c.dow_any);
        assert_eq!(c, cron("0 0 13 * 5"));
    }

    #[test]
    fn leap_days_eight_years_apart_across_2100() {
        let tz = utc();
        let leap = "0 0 29 2 *";
        assert_eq!(
            next(leap, &tz, at(2095, 3, 1, 0, 0)),
            Some(at(2096, 2, 29, 0, 0))
        );
        // 2100 is not a leap year: 2,922 days to the next 29 February, within MAX_SEARCH_DAYS.
        assert_eq!(
            next(leap, &tz, at(2096, 2, 29, 0, 0)),
            Some(at(2104, 2, 29, 0, 0))
        );
        assert_eq!(
            next(leap, &tz, at(2100, 2, 28, 23, 59)),
            Some(at(2104, 2, 29, 0, 0))
        );
        // The same in a zone east of UTC: local midnight is 14:00 the day before in UTC.
        let east = table("Etc/GMT-10", 36_000, &[]);
        assert_eq!(
            next(leap, &east, at(2096, 2, 28, 14, 0)),
            Some(at(2104, 2, 28, 14, 0))
        );
    }

    #[test]
    fn impossible_dates_never_fire() {
        for text in [
            "0 0 30 2 *",
            "0 0 31 4 *",
            "0 0 31 2,4,6,9,11 *",
            "0 0 30,31 2 *",
        ] {
            assert_eq!(next(text, &utc(), at(2026, 1, 1, 0, 0)), None, "{text}");
            assert_eq!(next(text, &berlin(), 0), None, "{text}");
        }
        // A hand-built spec with an empty field has no slot either.
        let mut c = cron("* * * * *");
        c.minutes = 0;
        assert_eq!(next_slot(&c, &utc(), 0), None);
        let mut c = cron("0 0 * * *");
        c.months = 1; // bit 0 is not a month
        assert_eq!(next_slot(&c, &utc(), 0), None);
    }

    /// Unix seconds of a UTC civil date and time.
    fn unix(y: i64, mo: u8, d: u8, h: u64, mi: u64) -> i64 {
        i64::try_from(at(y, mo, d, h, mi) / 1_000).unwrap()
    }

    /// Milliseconds of the start of day `day` (days since 1970-01-01) in UTC.
    fn day_ms(day: i64) -> u64 {
        u64::try_from(day).unwrap() * 86_400_000
    }

    /// UTC, except for three changes.
    /// - 2096-02-29 never happens: at its midnight the clocks jump a day ahead, as Apia's did in
    ///   2011, and two days later they fall back a day, so 2 March happens twice. The 29 February
    ///   before 2104's is then 2092's, 4,382 days earlier, so a search that reaches 2104-02-29
    ///   finds no earlier one.
    /// - At 22:00Z on 2104-02-29 the clocks jump from 22:00 to 01:00 (+03:00), and at 00:30Z on
    ///   1 March they fall back from 03:30 to 21:30 (−03:00): local 22:00 to 01:00, skipped by the
    ///   jump, first happens after the fall, from 01:00Z to 04:00Z.
    /// - UTC again from 2110.
    fn no_leap_day_in_2096() -> TzTable {
        table(
            "Test/NoLeapDay",
            0,
            &[
                (unix(2096, 2, 29, 0, 0), 86_400),
                (unix(2096, 3, 2, 0, 0), 0),
                (unix(2104, 2, 29, 22, 0), 10_800),
                (unix(2104, 3, 1, 0, 30), -10_800),
                (unix(2110, 1, 1, 0, 0), 0),
            ],
        )
    }

    #[test]
    fn the_search_covers_max_search_days_local_days() {
        let tz = no_leap_day_in_2096();
        let leap = "0 0 29 2 *";
        let slot = Some(at(2104, 2, 29, 0, 0));
        // The days searched are the local day of `after` and the MAX_SEARCH_DAYS − 1 days after
        // it (local time is UTC on the days of `after` below). From day `first`, 2104-02-29 is
        // the last day searched, whatever the time of day of `after`; from the day before, it is
        // not searched.
        let first = days_from_civil(2104, 2, 29) - i64::from(MAX_SEARCH_DAYS) + 1;
        assert_eq!(next(leap, &tz, day_ms(first)), slot);
        assert_eq!(next(leap, &tz, day_ms(first) + 11 * 3_600_000), slot);
        assert_eq!(next(leap, &tz, day_ms(first + 1) - 1), slot);
        assert_eq!(next(leap, &tz, day_ms(first - 1)), None);
        assert_eq!(next(leap, &tz, day_ms(first) - 1), None);
        // In the last two cases the limit, local midnight on 2104-02-29, falls inside the span
        // from 2096-03-02 to 2104-02-29T22:00Z, which holds the slot: the search stops at the
        // limit, not at the end of the span.
    }

    #[test]
    fn a_span_that_starts_after_the_limit_can_reach_back_before_it() {
        let tz = no_leap_day_in_2096();
        // The span that starts at 00:30Z on 1 March starts after the limit (local midnight that
        // night, compared as if it were UTC), yet its local times start at 21:30 on 29 February:
        // local 23:00 on 2104-02-29, the last day searched, first happens in it, at 02:00Z.
        let late = "0 23 29 2 *";
        let first = days_from_civil(2104, 2, 29) - i64::from(MAX_SEARCH_DAYS) + 1;
        assert_eq!(next(late, &tz, day_ms(first)), Some(at(2104, 3, 1, 2, 0)));
        assert_eq!(next(late, &tz, day_ms(first) - 1), None);
        // Local 21:45 happens at 21:45Z, before the jump, and again at 00:45Z in that span: it
        // fires at the first instant only, and next on 29 February 2108 (−03:00).
        let fold = "45 21 29 2 *";
        assert_eq!(
            next(fold, &tz, day_ms(first)),
            Some(at(2104, 2, 29, 21, 45))
        );
        assert_eq!(
            next(fold, &tz, at(2104, 2, 29, 21, 45)),
            Some(at(2108, 3, 1, 0, 45))
        );
    }

    #[test]
    fn a_transition_that_keeps_the_offset() {
        // A tzdb table may repeat an offset where only the abbreviation or the DST flag changes.
        // The span from 10:30Z starts at local 10:30, not at the start of its day: local 10:00
        // that day happened before `after` and does not fire again.
        let tz = table("Test/Redundant", 0, &[(unix(2026, 10, 2, 10, 30), 0)]);
        assert!(valid_table(&tz));
        let after = at(2026, 10, 2, 10, 0);
        assert_eq!(next("0 10 * * *", &tz, after), Some(at(2026, 10, 3, 10, 0)));
        assert_eq!(
            next("0,45 10 * * *", &tz, after),
            Some(at(2026, 10, 2, 10, 45))
        );
        assert_eq!(
            next("15,30 10 * * *", &tz, after),
            Some(at(2026, 10, 2, 10, 15))
        );
        assert_eq!(
            next("30 10 * * *", &tz, after + 15 * 60_000),
            Some(at(2026, 10, 2, 10, 30))
        );
    }

    #[test]
    fn totality_at_the_edges() {
        let every = cron("* * * * *");
        assert_eq!(next_slot(&every, &utc(), u64::MAX), None);
        assert_eq!(next_slot(&every, &utc(), 0), Some(60_000));
        let far = u64::MAX / 2;
        let _ = next_slot(&every, &berlin(), far);
        let _ = slot_name(u64::MAX);
        // An invalid table has no slot.
        let unordered = table("X", 0, &[(10, 0), (5, 3_600)]);
        assert_eq!(next_slot(&every, &unordered, 0), None);
        let too_far = table("X", 93_660, &[]);
        assert_eq!(next_slot(&every, &too_far, 0), None);
        let seconds = table("X", -2_670, &[]);
        assert_eq!(next_slot(&every, &seconds, 0), None);
    }

    // --- Gaps and folds --------------------------------------------------------------------------

    #[test]
    fn berlin_skips_the_gap_and_fires_the_fold_once() {
        let tz = berlin();
        let daily = "30 2 * * *";
        // 2026-03-28 02:30 CET is 01:30Z; 2026-03-29 02:30 does not exist; 2026-03-30 02:30 CEST
        // is 00:30Z.
        assert_eq!(
            next(daily, &tz, at(2026, 3, 28, 0, 0)),
            Some(1_774_661_400_000)
        );
        assert_eq!(
            next(daily, &tz, at(2026, 3, 28, 1, 30)),
            Some(at(2026, 3, 30, 0, 30))
        );
        // 2026-10-25 02:30 happens at 00:30Z (CEST) and at 01:30Z (CET): it fires at the first.
        assert_eq!(
            next(daily, &tz, at(2026, 10, 24, 0, 30)),
            Some(at(2026, 10, 25, 0, 30))
        );
        assert_eq!(
            next(daily, &tz, at(2026, 10, 25, 0, 30)),
            Some(at(2026, 10, 26, 1, 30))
        );
        // Every half hour: 02:00 and 02:30 CET repeat, so after 02:30 CEST comes 03:00 CET.
        let half = "*/30 * * * *";
        assert_eq!(
            next(half, &tz, at(2026, 10, 25, 0, 0)),
            Some(at(2026, 10, 25, 0, 30))
        );
        assert_eq!(
            next(half, &tz, at(2026, 10, 25, 0, 30)),
            Some(at(2026, 10, 25, 2, 0))
        );
        // Every minute: after 02:00 CET (its second instant, 01:00Z), every minute up to 02:59
        // CET repeats, and the next to fire is 03:00 CET.
        let every = "* * * * *";
        assert_eq!(
            next(every, &tz, at(2026, 10, 25, 0, 59)),
            Some(at(2026, 10, 25, 2, 0))
        );
        assert_eq!(
            next(every, &tz, at(2026, 10, 25, 1, 0) + 30_000),
            Some(at(2026, 10, 25, 2, 0))
        );
        assert_eq!(
            next(every, &tz, at(2026, 10, 25, 0, 58)),
            Some(at(2026, 10, 25, 0, 59))
        );
        // In the spring, 01:30 CET is followed by 03:00 CEST.
        assert_eq!(
            next(half, &tz, at(2026, 3, 29, 0, 30)),
            Some(at(2026, 3, 29, 1, 0))
        );
    }

    #[test]
    fn new_york_skips_and_folds() {
        let tz = new_york();
        // 2026-03-08 02:30 does not exist.
        assert_eq!(
            next("30 2 * * *", &tz, at(2026, 3, 7, 8, 0)),
            Some(at(2026, 3, 9, 6, 30))
        );
        // 2026-11-01 01:30 happens twice, at 05:30Z (EDT) and 06:30Z (EST).
        let fold = "30 1 * * *";
        assert_eq!(
            next(fold, &tz, at(2026, 10, 31, 6, 0)),
            Some(at(2026, 11, 1, 5, 30))
        );
        assert_eq!(
            next(fold, &tz, at(2026, 11, 1, 5, 30)),
            Some(at(2026, 11, 2, 6, 30))
        );
    }

    #[test]
    fn lord_howe_shifts_by_thirty_minutes() {
        let tz = lord_howe();
        // 2026-04-05: 02:00 (+11:00) becomes 01:30 (+10:30), so 01:45 happens twice.
        let fold = "45 1 * * *";
        assert_eq!(
            next(fold, &tz, at(2026, 4, 3, 15, 0)),
            Some(at(2026, 4, 4, 14, 45))
        );
        assert_eq!(
            next(fold, &tz, at(2026, 4, 4, 14, 45)),
            Some(at(2026, 4, 5, 15, 15))
        );
        // 2026-10-04: 02:00 (+10:30) becomes 02:30 (+11:00), so 02:15 does not exist that day.
        assert_eq!(
            next("15 2 * * *", &tz, at(2026, 10, 3, 0, 0)),
            Some(at(2026, 10, 4, 15, 15))
        );
        assert_eq!(
            next("45 2 * * *", &tz, at(2026, 10, 3, 0, 0)),
            Some(at(2026, 10, 3, 15, 45))
        );
    }

    #[test]
    fn kathmandu_is_five_forty_five_ahead() {
        let tz = kathmandu();
        assert_eq!(
            next("0 9 * * *", &tz, at(2026, 10, 2, 0, 0)),
            Some(at(2026, 10, 2, 3, 15))
        );
        // On 1986-01-01 the clocks went from 00:00 (+05:30) to 00:15 (+05:45).
        assert_eq!(
            next("5 0 * * *", &tz, at(1985, 12, 31, 0, 0)),
            Some(504_987_600_000)
        );
        assert_eq!(
            next("15 0 * * *", &tz, at(1985, 12, 31, 0, 0)),
            Some(at(1985, 12, 31, 18, 30))
        );
    }

    #[test]
    fn apia_skipped_a_whole_day() {
        let tz = apia();
        // 2011-12-29 12:00 (−10:00) was 22:00Z; 2011-12-30 did not exist; 2011-12-31 12:00
        // (+14:00) was 2011-12-30 22:00Z.
        let noon = "0 12 * * *";
        assert_eq!(
            next(noon, &tz, at(2011, 12, 29, 21, 59)),
            Some(at(2011, 12, 29, 22, 0))
        );
        assert_eq!(
            next(noon, &tz, at(2011, 12, 29, 22, 0)),
            Some(at(2011, 12, 30, 22, 0))
        );
        // A schedule for 30 December only skips to the next year.
        assert_eq!(
            next("0 12 30 12 *", &tz, at(2011, 12, 1, 0, 0)),
            Some(1_356_818_400_000)
        );
    }

    #[test]
    fn a_fold_that_reaches_back_past_a_gap() {
        // From 01:40Z the offset is +00:50 (local 01:40 jumps to 02:30), and from 03:20Z it is
        // −01:40 (local 04:10 falls back to 01:40). Local 02:40 first happens at 01:50Z; local
        // 02:00 happens only at 03:40Z. Local time order is not UTC order here: 02:40 fires first.
        let tz = table("Test/Pathological", 0, &[(6_000, 3_000), (12_000, -6_000)]);
        assert!(valid_table(&tz));
        let c = "0,40 2 * * *";
        assert_eq!(next(c, &tz, 0), Some(6_600_000));
        assert_eq!(next(c, &tz, 6_600_000), Some(13_200_000));
        // Local 02:40 at 04:20Z is its second instant: it does not fire again.
        assert_eq!(next(c, &tz, 13_200_000), Some(99_600_000));
    }

    #[test]
    fn slot_names() {
        assert_eq!(slot_name(0), "19700101T0000Z");
        assert_eq!(slot_name(at(2026, 3, 29, 1, 30)), "20260329T0130Z");
        assert_eq!(
            slot_name(at(2026, 10, 2, 23, 59) + 59_999),
            "20261002T2359Z"
        );
        assert_eq!(slot_name(at(2104, 2, 29, 0, 0)), "21040229T0000Z");
        assert_eq!(slot_name(at(10_000, 1, 1, 0, 0)), "100000101T0000Z");
        assert_eq!(slot_name(at(2011, 12, 30, 22, 0)), "20111230T2200Z");
    }

    // --- Brute-force reference -------------------------------------------------------------------

    /// SplitMix64: a small deterministic generator, so that every run tests the same cases.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }

        /// A number in `lo..=hi`.
        fn between(&mut self, lo: i64, hi: i64) -> i64 {
            let span = u64::try_from(hi - lo + 1).unwrap();
            lo + i64::try_from(self.below(span)).unwrap()
        }

        fn chance(&mut self, one_in: u64) -> bool {
            self.below(one_in) == 0
        }
    }

    fn random_item(rng: &mut Rng, lo: i64, hi: i64) -> String {
        let a = rng.between(lo, hi);
        let b = rng.between(a, hi);
        let step = rng.between(1, 12);
        match rng.below(4) {
            0 => a.to_string(),
            1 => format!("{a}-{b}"),
            2 => format!("*/{step}"),
            _ => format!("{a}-{b}/{step}"),
        }
    }

    /// `*` one time in `star`, otherwise a list of one to three items.
    fn random_field(rng: &mut Rng, lo: i64, hi: i64, star: u64) -> String {
        if rng.chance(star) {
            return String::from("*");
        }
        let items: Vec<String> = (0..rng.between(1, 3))
            .map(|_| random_item(rng, lo, hi))
            .collect();
        items.join(",")
    }

    /// A list whose first item is `*` or `*/n`, followed by one or two other items.
    fn starred_list(rng: &mut Rng, lo: i64, hi: i64) -> String {
        let mut items = Vec::from([if rng.chance(2) {
            String::from("*")
        } else {
            format!("*/{}", rng.between(1, 12))
        }]);
        for _ in 0..rng.between(1, 2) {
            items.push(random_item(rng, lo, hi));
        }
        items.join(",")
    }

    /// A day-of-week range that ends on Sunday (0) after a later start, sometimes with a step,
    /// and sometimes after another item.
    fn sunday_end(rng: &mut Rng) -> String {
        let start = rng.between(1, 7);
        let range = if rng.chance(2) {
            format!("{start}-0")
        } else {
            format!("{start}-0/{}", rng.between(1, 4))
        };
        if rng.chance(3) {
            format!("{},{range}", random_item(rng, 0, 7))
        } else {
            range
        }
    }

    fn random_cron(rng: &mut Rng) -> String {
        if rng.chance(10) {
            // Rare or impossible days: leap days, and 30 and 31 in short months.
            const DOMS: [&str; 4] = ["29", "30", "31", "29-31"];
            const MONTHS: [&str; 3] = ["2", "4,6", "2,4,6,9,11"];
            let dom = DOMS.get(usize::try_from(rng.below(4)).unwrap()).unwrap();
            let month = MONTHS.get(usize::try_from(rng.below(3)).unwrap()).unwrap();
            let minute = random_field(rng, 0, 59, 8);
            let hour = random_field(rng, 0, 23, 4);
            return format!("{minute} {hour} {dom} {month} *");
        }
        let minute = random_field(rng, 0, 59, 8);
        let hour = random_field(rng, 0, 23, 4);
        let dom = match rng.below(8) {
            // `*/30` and up: the 1st alone, or the 1st and the 31st.
            0 => format!("*/{}", rng.between(30, 40)),
            1 => starred_list(rng, 1, 31),
            _ => random_field(rng, 1, 31, 2),
        };
        let mut month = random_field(rng, 1, 12, 2);
        let mut dow = match rng.below(8) {
            0 | 1 => sunday_end(rng),
            2 => starred_list(rng, 0, 7),
            _ => random_field(rng, 0, 7, 2),
        };
        if rng.chance(3) {
            month = spell_names(rng, &month, &MONTH_NAMES, 1);
        }
        if rng.chance(3) {
            dow = spell_names(rng, &dow, &DAY_NAMES, 0);
        }
        format!("{minute} {hour} {dom} {month} {dow}")
    }

    /// `field` with each value that has a name (not a step) written as the name, in upper, lower,
    /// or title case; `names` are those of `min`, `min + 1`, and so on.
    fn spell_names(rng: &mut Rng, field: &str, names: &[&str], min: u32) -> String {
        let mut spell = |value: &str| {
            let name = value
                .parse::<u32>()
                .ok()
                .and_then(|v| v.checked_sub(min))
                .and_then(|i| names.get(usize::try_from(i).unwrap()));
            match (name, rng.below(3)) {
                (None, _) => String::from(value),
                (Some(name), 0) => String::from(*name),
                (Some(name), 1) => name.to_ascii_lowercase(),
                (Some(name), _) => {
                    let lower = name.to_ascii_lowercase();
                    format!("{}{}", name.get(..1).unwrap(), lower.get(1..).unwrap())
                }
            }
        };
        let items: Vec<String> = field
            .split(',')
            .map(|item| {
                let (range, step) = match item.split_once('/') {
                    Some((range, step)) => (range, format!("/{step}")),
                    None => (item, String::new()),
                };
                let ends: Vec<String> = range.split('-').map(&mut spell).collect();
                format!("{}{step}", ends.join("-"))
            })
            .collect();
        items.join(",")
    }

    /// A whole-minute offset: usually a real-looking one, sometimes anything within ±26 hours.
    fn random_offset(rng: &mut Rng) -> i32 {
        let seconds = if rng.chance(2) {
            rng.between(-48, 56) * 900
        } else {
            rng.between(-1_560, 1_560) * 60
        };
        i32::try_from(seconds).unwrap()
    }

    /// At most four transitions: the first within two days of `after_s`, where the first slot
    /// usually lies, and each next one minutes to days later, or anywhere in the two years. One
    /// transition in four keeps the offset before it, as tzdb transitions do where only the
    /// abbreviation or the DST flag changes.
    fn random_table(rng: &mut Rng, after_s: i64) -> TzTable {
        let initial = random_offset(rng);
        let mut transitions: Vec<(i64, i32)> = Vec::new();
        for _ in 0..rng.below(5) {
            let (at, previous_offset) = match transitions.last() {
                None => (after_s + rng.between(-2 * 86_400, 2 * 86_400), initial),
                Some(&(previous, offset)) if rng.chance(4) => {
                    (previous + rng.between(1, 731 * 86_400), offset)
                }
                Some(&(previous, offset)) => (previous + rng.between(1, 72 * 3_600), offset),
            };
            let offset = if rng.chance(4) {
                previous_offset
            } else {
                random_offset(rng)
            };
            transitions.push((at, offset));
        }
        table("Test/Random", initial, &transitions)
    }

    /// Whether the field `text` allows `v`, read item by item from the text rather than through the
    /// parser. An item is `*`, a value, a range, or a step, and a value is a number or one of
    /// `names` (those of `min`, `min + 1`, and so on). In the day of week (`dow`), 7 is Sunday as
    /// well as 0, and a range that ends on Sunday after a later start runs to 7.
    fn allows(text: &str, v: u32, (min, max): (u32, u32), names: &[&str], dow: bool) -> bool {
        let value = |s: &str| {
            s.parse::<u32>().unwrap_or_else(|_| {
                let i = names
                    .iter()
                    .position(|n| n.eq_ignore_ascii_case(s))
                    .unwrap();
                min + u32::try_from(i).unwrap()
            })
        };
        text.split(',').any(|item| {
            let (range, step) = match item.split_once('/') {
                Some((range, step)) => (range, step.parse::<u32>().unwrap()),
                None => (item, 1),
            };
            let (first, last) = match range.split_once('-') {
                _ if range == "*" => (min, max),
                Some((a, b)) => {
                    let (a, b) = (value(a), value(b));
                    (a, if dow && b == 0 && a > 0 { 7 } else { b })
                }
                None => (value(range), value(range)),
            };
            core::iter::once(v)
                .chain((dow && v == 0).then_some(7))
                .any(|v| first <= v && v <= last && (v - first) % step == 0)
        })
    }

    /// A cron expression as the reference reads it, straight from its text: the values each field
    /// allows, one bit per value as in [`CronSpec`], and whether each day field starts with `*`.
    struct Expected {
        minutes: u64,
        hours: u64,
        doms: u64,
        months: u64,
        dows: u64,
        dom_star: bool,
        dow_star: bool,
    }

    fn expected(text: &str) -> Expected {
        let fields: Vec<&str> = text.split_ascii_whitespace().collect();
        let field = |i: usize| *fields.get(i).unwrap();
        let mask = |i: usize,
                    values: core::ops::RangeInclusive<u32>,
                    range: (u32, u32),
                    names: &[&str],
                    dow: bool| {
            values
                .filter(|&v| allows(field(i), v, range, names, dow))
                .fold(0_u64, |bits, v| bits | 1 << v)
        };
        Expected {
            minutes: mask(0, 0..=59, (0, 59), &[], false),
            hours: mask(1, 0..=23, (0, 23), &[], false),
            doms: mask(2, 1..=31, (1, 31), &[], false),
            months: mask(3, 1..=12, (1, 12), &MONTH_NAMES, false),
            dows: mask(4, 0..=6, (0, 7), &DAY_NAMES, true),
            dom_star: field(2).starts_with('*'),
            dow_star: field(4).starts_with('*'),
        }
    }

    /// Whether the cron matches the local minute starting at `local` (local seconds). The day
    /// rule: if either day field starts with `*`, a day must match both; otherwise either.
    fn matches_local(e: &Expected, local: i64) -> bool {
        let day = local.div_euclid(86_400);
        let second = local.rem_euclid(86_400);
        let (hour, minute) = (second / 3_600, second % 3_600 / 60);
        if (e.minutes >> minute) & 1 == 0 || (e.hours >> hour) & 1 == 0 {
            return false;
        }
        let (_, month, dom) = crate::civil::civil_from_days(day);
        let dow = crate::civil::weekday(day);
        let month_ok = (e.months >> month) & 1 == 1;
        let dom_ok = (e.doms >> dom) & 1 == 1;
        let dow_ok = (e.dows >> dow) & 1 == 1;
        month_ok
            && match (e.dom_star || e.dow_star, dom_ok, dow_ok) {
                (_, true, true) => true,
                (true, _, _) => false,
                (false, dom_ok, dow_ok) => dom_ok || dow_ok,
            }
    }

    /// The first slot of the cron `text` after `after_ms` within `minutes` minutes, by trying
    /// every UTC minute: a minute fires if the cron matches its local time and no earlier instant
    /// has that local time (each offset of the table is tried).
    fn reference(text: &str, tz: &TzTable, after_ms: u64, minutes: u64) -> Option<u64> {
        let e = expected(text);
        let offsets: Vec<i64> = core::iter::once(tz.initial)
            .chain(tz.transitions.iter().map(|t| t.offset))
            .map(i64::from)
            .collect();
        let first = after_ms / 60_000 + 1;
        (first..first + minutes).map(|m| m * 60_000).find(|&ms| {
            let utc = i64::try_from(ms / 1_000).unwrap();
            let local = utc + i64::from(offset_at(tz, utc));
            matches_local(&e, local)
                && !offsets.iter().any(|&o| {
                    let earlier = local - o;
                    earlier < utc && i64::from(offset_at(tz, earlier)) == o
                })
        })
    }

    #[test]
    fn brute_force_reference() {
        const TWO_YEARS_MINUTES: u64 = 731 * 1_440;
        let mut rng = Rng(0x5EED_C0DE_2026_1002);
        let (mut found, mut none) = (0, 0);
        let (mut named, mut stepped_day_fields, mut starred_lists) = (0_usize, 0_usize, 0_usize);
        let (mut single_dom, mut sunday_ends) = (0_usize, 0_usize);
        for case in 0..500 {
            let text = random_cron(&mut rng);
            let c = cron(&text);
            // The parser's values against the reference's, read from the text. Its day flags are
            // checked by the slots below, where the reference applies the day rule to the text.
            let e = expected(&text);
            assert_eq!(
                (
                    c.minutes,
                    u64::from(c.hours),
                    u64::from(c.doms),
                    u64::from(c.months),
                    u64::from(c.dows)
                ),
                (e.minutes, e.hours, e.doms, e.months, e.dows),
                "{text}"
            );
            let fields: Vec<&str> = text.split_ascii_whitespace().collect();
            let (dom, dow) = (*fields.get(2).unwrap(), *fields.get(4).unwrap());
            named += usize::from(text.bytes().any(|b| b.is_ascii_alphabetic()));
            stepped_day_fields += usize::from(dom.starts_with("*/") || dow.starts_with("*/"));
            starred_lists += usize::from(dom.starts_with("*,") || dow.starts_with("*,"));
            single_dom += usize::from(
                dom.strip_prefix("*/")
                    .and_then(|n| n.parse::<u32>().ok())
                    .is_some_and(|n| n >= 30),
            );
            sunday_ends += usize::from(dow.split(',').any(|item| {
                let range = item.split('/').next().unwrap();
                range.split_once('-').is_some_and(|(a, b)| {
                    let sunday = |s: &str| s == "0" || s.eq_ignore_ascii_case("SUN");
                    sunday(b) && !sunday(a)
                })
            }));
            let minute = rng.between(
                days_from_civil(2000, 1, 1) * 1_440,
                days_from_civil(2090, 1, 1) * 1_440,
            );
            let after = u64::try_from(minute).unwrap() * 60_000
                + if rng.chance(3) { 0 } else { rng.below(60_000) };
            let tz = random_table(&mut rng, i64::try_from(after / 1_000).unwrap());
            assert!(valid_table(&tz));
            let expected = reference(&text, &tz, after, TWO_YEARS_MINUTES);
            let got = next_slot(&c, &tz, after);
            match expected {
                Some(_) => {
                    found += 1;
                    assert_eq!(
                        got, expected,
                        "case {case}: {text:?} after {after} in {tz:?}"
                    );
                }
                None => {
                    none += 1;
                    let horizon = (after / 60_000 + 1 + TWO_YEARS_MINUTES) * 60_000;
                    assert!(
                        got.is_none_or(|s| s >= horizon),
                        "case {case}: {text:?} after {after} in {tz:?}: {got:?}"
                    );
                }
            }
            if let Some(s) = got {
                assert!(s > after && s % 60_000 == 0);
            }
        }
        // The generator covers both outcomes, names, day fields that start with `*/` or are lists
        // that start with a bare `*`, `*/n` of the 1st alone or the 1st and 31st, and day-of-week
        // ranges that end on Sunday after a later start.
        assert!(found > 400 && none > 10, "{found} found, {none} none");
        assert!(
            named > 100 && stepped_day_fields > 100,
            "{named} with names, {stepped_day_fields} with `*/n` day fields"
        );
        assert!(
            starred_lists > 25 && single_dom > 25 && sunday_ends > 50,
            "{starred_lists} `*,` lists, {single_dom} single-day `*/n`, {sunday_ends} Sunday ends"
        );
    }
}
