//! Recurring work: missed occurrences, the next occurrence, and coverage (design D.8.18, §12).
//! Bodies: WP-D6.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use super::Exec;
use super::close::{Closing, close_node};
use crate::event::{EndCause, EntryBody, EntryText, Ev, NodeInit, SystemNote};
use crate::reply::WarningDetail;
use crate::state::{Author, OccPhase, Occurrence, Origin, Repeat, Role, State};
use crate::types::{KeyStr, Name, NodeId, NormVer, Outcome, Text, Time};

/// The text of a missed occurrence's `Closed` entry.
const MISSED_TEXT: &str = "missed: no session took it within its window";
/// The text of the `Closed` entry of a series whose schedule has no further slot (A-51).
const ENDED_TEXT: &str = "the schedule has no further slot";
/// The local hours after the next slot's own that `first_usual` searches: three days (D.8.18).
const FIRST_USUAL_HOURS: i64 = 72;
/// The slots `coverage` reads at most within `coverage.window` (D.8.18).
const COVERAGE_SLOTS_MAX: usize = 50_000;

fn text(s: &str) -> EntryText {
    EntryText {
        main: Text(String::from(s)),
        extra: Vec::new(),
    }
}

/// Whether `o` is an open occurrence that `maybe_missed` closes (D.8.18, R2): its window has
/// passed (`window_fired`), nobody holds it, it has no open intent, and no checkpoint.
#[must_use]
pub fn missed_due(s: &State, o: NodeId) -> bool {
    let Some(node) = s.nodes.get(&o) else {
        return false;
    };
    let Role::Occurrence(x) = &node.role else {
        return false;
    };
    node.is_open()
        && x.window_fired
        && s.ix.node.get(&o).is_none_or(|i| i.live == 0)
        && node.open_intent.is_none()
        && node.checkpoints == 0
}

/// `maybe_missed(o)` (D.8.18, A-80, R2): at the window deadline and after every hold end on an
/// occurrence that leaves it open, close it as `missed` when its window has passed, nobody holds
/// it, it has no open intent, and no checkpoint. An occurrence with a checkpoint never closes as
/// missed: it stays open with `overdue` until someone finishes it.
pub fn maybe_missed(ex: &mut Exec<'_>, o: NodeId) {
    if !missed_due(ex.s(), o) {
        return;
    }
    close_node(
        ex,
        o,
        Closing {
            outcome: Outcome::Missed,
            by: Author::Server,
            merged_into: None,
            cause: EndCause::Close,
            text: text(MISSED_TEXT),
            reslot: None,
        },
    );
}

/// The name of the occurrence of `slot` under `series`: `YYYYMMDDTHHMMZ`, with `-<id>` added when
/// that name is live under the series (D.8.18), and `-<k>` after it in the unlikely case that an
/// agent took that one too; None only if no such name is free, which the number of names bounds.
fn occurrence_name(s: &State, series: NodeId, slot: Time, id: NodeId) -> Option<Name> {
    let base = forum_cron::slot_name(slot.0);
    let free = |n: &Name| !s.names.contains_key(&(series, n.clone()));
    let first = Name::new(&base)?;
    if free(&first) {
        return Some(first);
    }
    let mut with_id = base;
    with_id.push('-');
    with_id.push_str(&alloc::format!("{}", id.0));
    let second = Name::new(&with_id)?;
    if free(&second) {
        return Some(second);
    }
    (1..=s.names.len()).find_map(|k| {
        let n = Name::new(&alloc::format!("{with_id}-{k}"))?;
        free(&n).then_some(n)
    })
}

/// `open_next_occurrence(series, after)` (D.8.18, A-51): the occurrence of the first slot after
/// `after`, a child of the series named after its slot, with the series' title, body, needs, and
/// est, capacity 1, `due` at the end of its window, the key `repeat:<series>:<slot>` (omitted if an
/// open node carries it, which agents cannot cause, A-90), and phase `Waiting`; or, with no slot
/// within `MAX_SEARCH_DAYS`, `System(ScheduleEnded)` on the series and the series closed as
/// failed. A node that is not an open series, or that has an open occurrence already (K6), is
/// left alone.
pub fn open_next_occurrence(ex: &mut Exec<'_>, series: NodeId, after: Time) {
    let s = ex.s();
    let Some(node) = s.nodes.get(&series) else {
        return;
    };
    let Role::Series(x) = &node.role else {
        return;
    };
    if !node.is_open() || s.ix.series_open.get(&series).is_some_and(|o| !o.is_empty()) {
        return;
    }
    let repeat = x.repeat.clone();
    let Some(slot) = forum_cron::next_slot(&repeat.cron, &repeat.tz, after.0).map(Time) else {
        ex.append(
            series,
            Author::Server,
            EntryBody::System(SystemNote::ScheduleEnded),
            EntryText::default(),
        );
        close_node(
            ex,
            series,
            Closing {
                outcome: Outcome::Failed,
                by: Author::Server,
                merged_into: None,
                cause: EndCause::Close,
                text: text(ENDED_TEXT),
                reslot: None,
            },
        );
        return;
    };
    let id = s.counters.next_node;
    let Some(name) = occurrence_name(s, series, slot, id) else {
        return;
    };
    let window_end = slot.plus(repeat.run_within);
    let key = KeyStr::of_occurrence(series, slot);
    let mut keys = BTreeMap::new();
    if !s.ix.keys_open.contains_key(&key) {
        keys.insert(key, NormVer(0));
    }
    let init = NodeInit {
        title: node.title.clone(),
        body: node.body.clone(),
        fields: BTreeMap::new(),
        status: Text::default(),
        digest: Text::default(),
        agreements: Text::default(),
        kind: None,
        tags: alloc::collections::BTreeSet::new(),
        capacity: 1,
        priority: 0,
        est: node.est,
        needs: node.needs.clone(),
        due: Some(window_end),
        depends_on: alloc::collections::BTreeSet::new(),
        owners: alloc::collections::BTreeSet::new(),
        keys,
        links: alloc::collections::BTreeSet::new(),
        role: Role::Occurrence(Occurrence {
            series,
            slot,
            window_end,
            phase: OccPhase::Waiting,
            window_fired: false,
        }),
    };
    ex.emit(Ev::NodeCreated {
        id,
        parent: series,
        name,
        origin: Origin::Occurrence,
        init,
    });
}

/// The OccurrenceSlot firing (D.8.2, D.8.18, 1.4, R10): the occurrence copies its series' title,
/// body, needs, and est as they are now, with one `NodeUpdated` carrying the fields that differ
/// (none: no event), then becomes available (`OccurrenceAvailable`).
pub fn occurrence_slot(ex: &mut Exec<'_>, o: NodeId) {
    if let Some(patch) = slot_copy(ex.s(), o) {
        ex.emit(Ev::NodeUpdated { node: o, patch });
    }
    ex.emit(Ev::OccurrenceAvailable { node: o });
}

/// The patch of the copy at the slot (R10): the series' title, body, needs, and est where they
/// differ from the occurrence's; None when none differs, or when `o` is not an occurrence of an
/// existing node.
#[must_use]
pub fn slot_copy(s: &State, o: NodeId) -> Option<crate::event::NodePatch> {
    let node = s.nodes.get(&o)?;
    let Role::Occurrence(x) = &node.role else {
        return None;
    };
    let series = s.nodes.get(&x.series)?;
    let patch = crate::event::NodePatch {
        title: (series.title != node.title).then(|| series.title.clone()),
        body: (series.body != node.body).then(|| series.body.clone()),
        needs: (series.needs != node.needs).then(|| series.needs.clone()),
        est: if series.est == node.est {
            crate::types::Change::Keep
        } else {
            series
                .est
                .map_or(crate::types::Change::Clear, crate::types::Change::Set)
        },
        ..crate::event::NodePatch::default()
    };
    (patch != crate::event::NodePatch::default()).then_some(patch)
}

/// `coverage(s, repeat, now)` (D.8.18, A-47): `Some(WarningDetail::UncoveredSlot { .. })` when on
/// most of the schedule's days within `coverage.window` no session started within a slot's window.
/// The next slot is the schedule's first after `now`, the slot of a series' first occurrence;
/// `coverage_at` takes another.
#[must_use]
pub fn coverage(s: &State, repeat: &Repeat, now: Time) -> Option<WarningDetail> {
    let next = forum_cron::next_slot(&repeat.cron, &repeat.tz, now.0).map(Time)?;
    coverage_at(s, repeat, now, next)
}

/// `coverage` with the slot of the series' next occurrence, `next` (D.8.18, 1.4, R18):
/// - the slots from `now − coverage.window` up to `now`, at most `COVERAGE_SLOTS_MAX`;
/// - a slot is covered when some minute `m` of `stats.starts` with `slot ≤ m × 60,000 < slot +
///   run_within` has a count above 0;
/// - the days are the distinct local days (in the series' zone) of the slots, and the covered
///   days those with a covered slot; with days and `2 × covered < days`, the warning, with the
///   three most frequent local hours of the session starts in `[now − coverage.window, now)` (by
///   count descending, then hour ascending) and `first_start`, the first of those hours inside
///   the next slot's window (`first_usual`).
#[must_use]
pub fn coverage_at(s: &State, repeat: &Repeat, now: Time, next: Time) -> Option<WarningDetail> {
    let from = now.minus(s.params.coverage_window);
    let mut slots: Vec<Time> = Vec::new();
    let mut cursor = from;
    while slots.len() < COVERAGE_SLOTS_MAX {
        let Some(t) = forum_cron::next_slot(&repeat.cron, &repeat.tz, cursor.0).map(Time) else {
            break;
        };
        if t >= now {
            break;
        }
        slots.push(t);
        cursor = t;
    }
    let covered = |slot: Time| {
        let first = slot.0.div_ceil(60_000);
        let end = slot.plus(repeat.run_within);
        s.stats
            .starts
            .range(first..)
            .take_while(|(m, _)| m.saturating_mul(60_000) < end.0)
            .any(|(_, c)| *c > 0)
    };
    let mut days: BTreeMap<i64, bool> = BTreeMap::new();
    for &slot in &slots {
        let (day, _) = forum_cron::local_day_hour(&repeat.tz, slot.0);
        let c = covered(slot);
        let e = days.entry(day).or_insert(false);
        *e = *e || c;
    }
    let n_days = u32::try_from(days.len()).unwrap_or(u32::MAX);
    let n_covered = u32::try_from(days.values().filter(|c| **c).count()).unwrap_or(u32::MAX);
    if n_days == 0 || n_covered.saturating_mul(2) >= n_days {
        return None;
    }
    let usual_hours = usual_hours(s, &repeat.tz, from, now);
    let first_start = first_usual(&repeat.tz, next, next.plus(repeat.run_within), &usual_hours);
    Some(WarningDetail::UncoveredSlot {
        covered_days: n_covered,
        days: n_days,
        usual_hours,
        first_start,
    })
}

/// The three most frequent local hours (in `tz`) of the session starts in `[from, now)`, summed
/// over the minutes of `stats.starts`, by count descending, then hour ascending (D.8.18).
fn usual_hours(s: &State, tz: &forum_cron::TzTable, from: Time, now: Time) -> Vec<u8> {
    let first = from.0.div_ceil(60_000);
    let mut by_hour: BTreeMap<u8, u64> = BTreeMap::new();
    for (&m, &c) in s.stats.starts.range(first..) {
        let at = m.saturating_mul(60_000);
        if at >= now.0 {
            break;
        }
        if c == 0 {
            continue;
        }
        let (_, h) = forum_cron::local_day_hour(tz, at);
        let e = by_hour.entry(h).or_insert(0);
        *e = e.saturating_add(u64::from(c));
    }
    let mut hours: Vec<(u64, u8)> = by_hour.into_iter().map(|(h, c)| (c, h)).collect();
    hours.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    hours.into_iter().take(3).map(|(_, h)| h).collect()
}

/// `first_usual(tz, next, end, hours)` (D.8.18, 1.4, R18): the first usual start inside the next
/// slot's window `[next, end)`: `next` itself when its own local hour is usual; else the start of
/// the first of the 72 local hours after it that is usual, if that local time exists (a gap skips
/// its hour) and lies before `end`; None when no usual hour falls inside the window, or none is
/// known.
#[must_use]
pub fn first_usual(tz: &forum_cron::TzTable, next: Time, end: Time, hours: &[u8]) -> Option<Time> {
    if hours.is_empty() {
        return None;
    }
    let (day, hour) = forum_cron::local_day_hour(tz, next.0);
    if hours.contains(&hour) {
        return Some(next);
    }
    let start = day
        .checked_mul(24)
        .and_then(|h| h.checked_add(i64::from(hour)))?;
    for i in 1..=FIRST_USUAL_HOURS {
        let local_hour = start.checked_add(i)?;
        let h = u8::try_from(local_hour.rem_euclid(24)).ok()?;
        if !hours.contains(&h) {
            continue;
        }
        let Some(utc_s) = forum_cron::earliest_utc(tz, local_hour.checked_mul(3_600)?) else {
            continue;
        };
        let t = u64::try_from(utc_s).ok()?.checked_mul(1_000).map(Time)?;
        return (t < end).then_some(t);
    }
    None
}
