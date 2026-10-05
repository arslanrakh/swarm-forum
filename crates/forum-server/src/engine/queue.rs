//! The writer's queues (design D.11.2, CS-29): one bounded queue per class and lane, served by
//! deficit round-robin with a quantum of one job, so one job per class per round, in arrival order
//! within a class. A lane holds at most `lane_max` jobs and all lanes together `total_max`
//! (`engine::QUEUE_MAX`, review E4-spec-13); a job past either is answered `limit(Busy)` before it
//! is queued. Each job is stamped with a monotonic reading, on which its age is measured (review
//! E4-robust-13).
//!
//! Each class has two lanes: its calls, and the shell's refusals of its calls that name an
//! existing session, whose status line only the writer can render (D.11.7). A runaway class that
//! floods either lane therefore delays another class by at most one job per lane per round, and
//! the refusals of one class never wait behind the calls of another. `queue.max_wait` is measured
//! on the calls lane.

use std::collections::{BTreeMap, VecDeque};

use forum_core::ClassName;

use crate::clock::Mono;

/// A class's lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lane {
    /// Tool calls, and the readiness checks of `talk wait`.
    Calls,
    /// The status lines of the shell's refusals (D.11.7).
    Refusals,
}

/// A queue's key.
pub type Key = (ClassName, Lane);

/// The queues, generic over the job so that the order can be tested on its own.
#[derive(Debug)]
pub struct Queues<J> {
    lanes: BTreeMap<Key, VecDeque<(Mono, J)>>,
    /// The keys with jobs, in the order of the round.
    ring: VecDeque<Key>,
    len: usize,
    lane_max: usize,
    total_max: usize,
}

impl<J> Default for Queues<J> {
    fn default() -> Self {
        Queues::with_bounds(usize::MAX, usize::MAX)
    }
}

impl<J> Queues<J> {
    /// No jobs, and no bound.
    #[must_use]
    pub fn new() -> Self {
        Queues::default()
    }

    /// No jobs; at most `lane_max` in a lane and `total_max` in all.
    #[must_use]
    pub fn with_bounds(lane_max: usize, total_max: usize) -> Self {
        Queues {
            lanes: BTreeMap::new(),
            ring: VecDeque::new(),
            len: 0,
            lane_max,
            total_max,
        }
    }

    /// Queues `job` in `key`'s queue, stamped `at`.
    ///
    /// # Errors
    ///
    /// The job, given back, when its lane or the queues as a whole are full.
    pub fn push(&mut self, key: Key, at: Mono, job: J) -> Result<(), J> {
        if self.len >= self.total_max
            || self
                .lanes
                .get(&key)
                .is_some_and(|q| q.len() >= self.lane_max)
        {
            return Err(job);
        }
        let q = self.lanes.entry(key.clone()).or_default();
        if q.is_empty() {
            self.ring.push_back(key);
        }
        q.push_back((at, job));
        self.len += 1;
        Ok(())
    }

    /// The next job of the round: the first job of the next key in the ring, whose key then goes
    /// to the back of the ring if it has more.
    pub fn pop(&mut self) -> Option<(Key, Mono, J)> {
        let key = self.ring.pop_front()?;
        let q = self.lanes.get_mut(&key)?;
        let (at, job) = q.pop_front()?;
        if q.is_empty() {
            self.lanes.remove(&key);
        } else {
            self.ring.push_back(key.clone());
        }
        self.len -= 1;
        Some((key, at, job))
    }

    /// When the oldest job of `key`'s queue was queued.
    #[must_use]
    pub fn oldest(&self, key: &Key) -> Option<Mono> {
        self.lanes.get(key).and_then(|q| q.front()).map(|(t, _)| *t)
    }

    /// How many jobs are queued.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether none is.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Every job, emptied out (a writer that stopped drops them, and their callers hear nothing).
    pub fn drain(&mut self) -> Vec<J> {
        self.ring.clear();
        self.len = 0;
        std::mem::take(&mut self.lanes)
            .into_values()
            .flat_map(|q| q.into_iter().map(|(_, j)| j))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn key(c: &str) -> Key {
        (ClassName::new(c).unwrap(), Lane::Calls)
    }

    /// The reading `ms` after the first one of the tests.
    fn at(ms: u64) -> Mono {
        static ORIGIN: std::sync::OnceLock<Mono> = std::sync::OnceLock::new();
        ORIGIN
            .get_or_init(Mono::now)
            .plus(Duration::from_millis(ms))
    }

    #[test]
    fn one_job_per_class_per_round_in_arrival_order_within_a_class() {
        let mut q = Queues::new();
        // A runaway class queues ten jobs before two others queue one each.
        for i in 0..10 {
            q.push(key("runaway"), at(i), format!("r{i}")).unwrap();
        }
        q.push(key("ide-agent"), at(20), "i0".to_string()).unwrap();
        q.push(key("ci-agent"), at(21), "c0".to_string()).unwrap();
        q.push(key("ide-agent"), at(22), "i1".to_string()).unwrap();
        assert_eq!(q.len(), 13);
        assert_eq!(q.oldest(&key("runaway")), Some(at(0)));
        let order: Vec<String> = std::iter::from_fn(|| q.pop().map(|(_, _, j)| j)).collect();
        assert_eq!(
            order[..6],
            ["r0", "i0", "c0", "r1", "i1", "r2"],
            "the others wait at most one runaway job per round"
        );
        assert_eq!(order.len(), 13);
        assert!(q.is_empty());
        assert_eq!(q.oldest(&key("runaway")), None);
    }

    #[test]
    fn the_refusal_lane_is_a_queue_of_its_own() {
        let mut q = Queues::new();
        let c = ClassName::new("coding-agent").unwrap();
        for i in 0..3 {
            q.push((c.clone(), Lane::Calls), at(i), i).unwrap();
        }
        q.push((c.clone(), Lane::Refusals), at(9), 100).unwrap();
        let order: Vec<u64> = std::iter::from_fn(|| q.pop().map(|(_, _, j)| j)).collect();
        assert_eq!(order, [0, 100, 1, 2]);
        q.push((c.clone(), Lane::Calls), at(1), 7).unwrap();
        assert_eq!(q.drain(), vec![7]);
        assert!(q.is_empty() && q.pop().is_none());
    }

    /// A lane holds at most its bound, and the queues their total; a job past either is given
    /// back (review E4-spec-13).
    #[test]
    fn a_full_lane_or_full_queues_give_the_job_back() {
        let mut q = Queues::with_bounds(2, 3);
        let c = ClassName::new("coding-agent").unwrap();
        let d = ClassName::new("ide-agent").unwrap();
        let calls = (c.clone(), Lane::Calls);
        assert!(q.push(calls.clone(), at(0), 0).is_ok());
        assert!(q.push(calls.clone(), at(1), 1).is_ok());
        assert_eq!(q.push(calls.clone(), at(2), 2), Err(2), "the lane is full");
        assert!(q.push((c, Lane::Refusals), at(3), 3).is_ok());
        assert_eq!(
            q.push((d, Lane::Calls), at(4), 4),
            Err(4),
            "the queues are full"
        );
        assert_eq!(q.pop().map(|(_, _, j)| j), Some(0));
        assert!(q.push(calls, at(5), 5).is_ok(), "room again");
        assert_eq!(q.len(), 3);
    }
}
