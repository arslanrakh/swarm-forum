//! The shell's safety valves on writes (design D.11.2, D.11.15, §19): each session's
//! `write.per_hour` window, each class's write counts of the trailing hour behind the `WriteSurge` health
//! reason, and the `queue.max_wait` test of `limit(Busy)`.
//!
//! Everything here lives in memory: the windows reset at a restart, and a session's window goes
//! at its silence or retirement (D.11.4, CS-17). None of it is a permission (§3, §19): every class
//! and every session meets the same limits, and there is no per-class write limit (D-7).

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use forum_core::health::HealthReason;
use forum_core::{ClassName, Dur, HOUR, MINUTE, SessionId, Time};

use crate::clock::Mono;

/// `WriteSurge`'s floor: a class's writes in the last hour must exceed it (§17, D.11.2).
pub const WRITE_SURGE_FLOOR: u32 = 1_000;
/// `WriteSurge`'s factor over the mean hour of the 24 before (§17, D.11.2).
pub const WRITE_SURGE_FACTOR: u64 = 3;
/// Hours of class counts kept: the last hour and the 24 before it.
pub const SURGE_HOURS: u64 = 25;

/// Each session's write calls in the last hour (D.11.15).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WriteWindows {
    by_session: BTreeMap<SessionId, VecDeque<Time>>,
}

impl WriteWindows {
    /// No session has written.
    #[must_use]
    pub fn new() -> WriteWindows {
        WriteWindows::default()
    }

    fn prune(q: &mut VecDeque<Time>, now: Time) {
        let from = now.minus(HOUR);
        while q.front().is_some_and(|t| *t <= from) {
            q.pop_front();
        }
    }

    /// The sessions with a write in their window (feature `bench`: gate G-6 checks that the
    /// shell's maps stay bounded).
    #[cfg(feature = "bench")]
    #[must_use]
    pub fn sessions(&self) -> usize {
        self.by_session.len()
    }

    /// Whether `session` has made `limit` or more write calls in the hour before `now`: its next
    /// write is answered `limit(WritesPerHour)` (§19).
    pub fn over(&mut self, session: SessionId, now: Time, limit: u32) -> bool {
        let Some(q) = self.by_session.get_mut(&session) else {
            return limit == 0;
        };
        Self::prune(q, now);
        let n = q.len();
        if n == 0 {
            self.by_session.remove(&session);
        }
        u32::try_from(n).unwrap_or(u32::MAX) >= limit
    }

    /// Counts one write call of `session` at `now`.
    pub fn record(&mut self, session: SessionId, now: Time) {
        let q = self.by_session.entry(session).or_default();
        Self::prune(q, now);
        q.push_back(now);
    }

    /// Forgets `session`: at its silence or retirement (CS-17).
    pub fn drop_session(&mut self, session: SessionId) {
        self.by_session.remove(&session);
    }

    /// How many sessions have a window.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_session.len()
    }

    /// Whether none has.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_session.is_empty()
    }
}

/// Each class's writes in minute buckets over the last [`SURGE_HOURS`] hours (D.11.2): the
/// `WriteSurge` reason compares the trailing hour, the 60 minutes up to `now`, with the mean hour
/// of the 24 hours before it (§17: "a class's writes in the last hour"), so a surge neither
/// vanishes at the top of an hour nor hides by straddling one (review E4-spec-12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteCounts {
    by_class: BTreeMap<ClassName, BTreeMap<u64, u32>>,
    floor: u32,
}

impl Default for WriteCounts {
    fn default() -> WriteCounts {
        WriteCounts::with_floor(WRITE_SURGE_FLOOR)
    }
}

fn minute_of(t: Time) -> u64 {
    t.0 / MINUTE.0
}

/// Minutes in an hour.
const HOUR_MINUTES: u64 = HOUR.0 / MINUTE.0;
/// Minutes of class counts kept.
const SURGE_MINUTES: u64 = SURGE_HOURS * HOUR_MINUTES;

impl WriteCounts {
    /// No class has written.
    #[must_use]
    pub fn new() -> WriteCounts {
        WriteCounts::default()
    }

    /// No class has written, and a surge needs more than `floor` writes in the trailing hour (a
    /// test lowers [`WRITE_SURGE_FLOOR`]).
    #[must_use]
    pub fn with_floor(floor: u32) -> WriteCounts {
        WriteCounts {
            by_class: BTreeMap::new(),
            floor,
        }
    }

    /// Counts one write call of `class` at `now`.
    pub fn record(&mut self, class: &ClassName, now: Time) {
        let m = minute_of(now);
        let buckets = self.by_class.entry(class.clone()).or_default();
        let slot = buckets.entry(m).or_insert(0);
        *slot = slot.saturating_add(1);
        let oldest = m.saturating_sub(SURGE_MINUTES - 1);
        while buckets.first_key_value().is_some_and(|(k, _)| *k < oldest) {
            buckets.pop_first();
        }
    }

    /// The `WriteSurge` reasons at `now`, by class: the class's writes in the trailing hour exceed
    /// `max(floor, WRITE_SURGE_FACTOR × mean)`, the mean taken over the 24 hours before that hour,
    /// a minute without writes counting as 0 (§17, D.8.22).
    #[must_use]
    pub fn surges(&self, now: Time) -> Vec<HealthReason> {
        let m = minute_of(now);
        let hour_from = m.saturating_sub(HOUR_MINUTES - 1);
        let first = m.saturating_sub(SURGE_MINUTES - 1);
        let hours = SURGE_HOURS - 1;
        let mut out = Vec::new();
        for (class, buckets) in &self.by_class {
            let sum = |r: std::ops::RangeInclusive<u64>| -> u64 {
                buckets.range(r).map(|(_, n)| u64::from(*n)).sum()
            };
            let last = sum(hour_from..=m);
            let before = if hour_from > first {
                sum(first..=hour_from - 1)
            } else {
                0
            };
            // last > FACTOR × (before / hours), compared exactly in integers.
            if last > u64::from(self.floor)
                && last.saturating_mul(hours) > WRITE_SURGE_FACTOR.saturating_mul(before)
            {
                out.push(HealthReason::WriteSurge {
                    class: class.clone(),
                    last_hour: u32::try_from(last).unwrap_or(u32::MAX),
                    baseline: u32::try_from(before / hours).unwrap_or(u32::MAX),
                });
            }
        }
        out
    }
}

/// Whether a class whose oldest queued job was queued at `oldest` is busy at `now`: that job has
/// waited longer than `queue.max_wait`, and the next call of the class is answered `limit(Busy)`
/// before it is queued, with nothing applied (§19, CS-29). Both are monotonic readings, so a step
/// of the wall clock changes nothing (review E4-robust-13).
#[must_use]
pub fn busy(oldest: Option<Mono>, now: Mono, max_wait: Dur) -> bool {
    oldest.is_some_and(|t| now.since(t) > Duration::from_millis(max_wait.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(s: &str) -> ClassName {
        ClassName::new(s).unwrap()
    }

    #[test]
    fn a_session_is_over_its_limit_for_the_hour_after_its_writes() {
        let mut w = WriteWindows::new();
        let s = SessionId(4);
        let t0 = Time(10 * HOUR.0);
        assert!(!w.over(s, t0, 3));
        for i in 0..3 {
            w.record(s, t0.plus(Dur(i * MINUTE.0)));
        }
        assert!(w.over(s, t0.plus(Dur(3 * MINUTE.0)), 3));
        assert!(!w.over(s, t0.plus(Dur(3 * MINUTE.0)), 4));
        // The first write leaves the window an hour after it.
        assert!(w.over(s, t0.plus(HOUR).minus(Dur(1)), 3));
        assert!(!w.over(s, t0.plus(HOUR), 3));
        assert!(!w.over(s, t0.plus(Dur(2 * HOUR.0)), 1));
        assert!(w.is_empty(), "an empty window is forgotten");
        w.record(s, t0);
        w.drop_session(s);
        assert!(w.is_empty());
    }

    #[test]
    fn a_surge_is_the_last_hour_over_the_floor_and_three_times_the_mean() {
        let mut c = WriteCounts::new();
        let a = class("coding-agent");
        let b = class("ide-agent");
        let t0 = Time(100 * HOUR.0);
        // 24 hours of 500 writes each for `a`: the mean is 500, so the bar is 1,500.
        for h in 0..24 {
            for _ in 0..500 {
                c.record(&a, Time((76 + h) * HOUR.0));
            }
        }
        for _ in 0..1_500 {
            c.record(&a, t0);
        }
        assert!(
            c.surges(t0).is_empty(),
            "exactly three times the mean is no surge"
        );
        c.record(&a, t0);
        assert_eq!(
            c.surges(t0),
            vec![HealthReason::WriteSurge {
                class: a.clone(),
                last_hour: 1_501,
                baseline: 500
            }]
        );
        // A quiet class needs more than the floor.
        for _ in 0..1_000 {
            c.record(&b, t0);
        }
        assert_eq!(c.surges(t0).len(), 1, "1,000 writes is not over the floor");
        c.record(&b, t0);
        assert_eq!(c.surges(t0).len(), 2);
        // Old buckets leave: 25 hours later nothing is counted.
        let later = Time(t0.0 + 25 * HOUR.0);
        c.record(&a, later);
        assert!(c.surges(later).is_empty());
        assert_eq!(c.by_class[&a].len(), 1, "only the last 25 hours are kept");
    }

    /// The last hour is the trailing 60 minutes, not the calendar hour: a surge just before the top
    /// of an hour is still reported just after it, and one that straddles it is reported at all
    /// (review E4-spec-12).
    #[test]
    fn a_surge_that_straddles_the_top_of_an_hour_is_reported() {
        let a = class("coding-agent");
        let ten = Time(10 * HOUR.0);
        let minutes = |m: u64| Dur(m * MINUTE.0);
        // 1,600 writes at 09:58: still the last hour at 10:01.
        let mut c = WriteCounts::new();
        for _ in 0..1_600 {
            c.record(&a, ten.minus(minutes(2)));
        }
        assert_eq!(c.surges(ten.minus(minutes(1))).len(), 1);
        assert_eq!(
            c.surges(ten.plus(minutes(1))).len(),
            1,
            "it does not vanish at 10:00"
        );
        assert!(c.surges(ten.plus(minutes(58))).is_empty(), "an hour later");
        // 800 writes at 09:55 and 801 at 10:05: 1,601 in the hour to 10:06.
        let mut c = WriteCounts::new();
        for _ in 0..800 {
            c.record(&a, ten.minus(minutes(5)));
        }
        for _ in 0..801 {
            c.record(&a, ten.plus(minutes(5)));
        }
        assert_eq!(
            c.surges(ten.plus(minutes(6))),
            vec![HealthReason::WriteSurge {
                class: a.clone(),
                last_hour: 1_601,
                baseline: 0
            }]
        );
        // A lowered floor (tests) counts the same way.
        let mut c = WriteCounts::with_floor(3);
        for _ in 0..4 {
            c.record(&a, ten);
        }
        assert_eq!(c.surges(ten).len(), 1);
    }

    #[test]
    fn busy_after_the_oldest_job_waited_longer_than_queue_max_wait() {
        let max = Dur(10_000);
        let t0 = Mono::now();
        let at = |ms: u64| t0.plus(Duration::from_millis(ms));
        assert!(!busy(None, at(50_000), max));
        assert!(!busy(Some(at(40_000)), at(50_000), max));
        assert!(busy(Some(at(39_999)), at(50_000), max));
        assert!(
            !busy(Some(at(60_000)), at(50_000), max),
            "a stamp after the reading has not waited"
        );
    }
}
