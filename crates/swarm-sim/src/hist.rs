//! A log-linear latency histogram: values in microseconds, 64 sub-buckets per power of two, so a
//! percentile is within 1.6 % of the true value, for every value, in a fixed 4,096 counters.
//! Histograms merge by adding counters.

use serde::Serialize;

/// Sub-buckets per power of two.
const SUB: u64 = 64;
/// Enough buckets for every u64: the largest index is 57 * 64 + 127.
const BUCKETS: usize = 64 * 64;

/// Counts of values by bucket.
#[derive(Clone, Debug)]
pub struct Hist {
    counts: Vec<u64>,
    n: u64,
    sum: u128,
    max: u64,
}

impl Default for Hist {
    fn default() -> Hist {
        Hist::new()
    }
}

fn bucket(v: u64) -> usize {
    if v < SUB {
        return usize::try_from(v).unwrap_or(0);
    }
    // v >> shift lies in [64, 128): its low six bits are the sub-bucket.
    let shift = 63 - v.leading_zeros() - 6;
    let i = u64::from(shift) * SUB + (v >> shift);
    usize::try_from(i).unwrap_or(BUCKETS - 1).min(BUCKETS - 1)
}

/// The largest value that lands in bucket `i` (what a percentile reports).
fn upper(i: usize) -> u64 {
    let i = u64::try_from(i).unwrap_or(0);
    if i < SUB {
        return i;
    }
    let shift = i / SUB - 1;
    let m = i % SUB + SUB;
    ((m + 1) << shift) - 1
}

/// A summary of a histogram, in milliseconds.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Summary {
    /// Values recorded.
    pub n: u64,
    /// The mean.
    pub mean_ms: f64,
    /// The median.
    pub p50_ms: f64,
    /// The 90th percentile.
    pub p90_ms: f64,
    /// The 99th percentile.
    pub p99_ms: f64,
    /// The 99.9th percentile.
    pub p999_ms: f64,
    /// The largest value.
    pub max_ms: f64,
}

#[allow(
    clippy::cast_precision_loss,
    reason = "microsecond counts far below 2^53 become milliseconds for a report"
)]
fn us_to_ms(us: u64) -> f64 {
    us as f64 / 1_000.0
}

impl Hist {
    /// Empty.
    #[must_use]
    pub fn new() -> Hist {
        Hist {
            counts: vec![0; BUCKETS],
            n: 0,
            sum: 0,
            max: 0,
        }
    }

    /// Records one value in microseconds.
    pub fn record_us(&mut self, us: u64) {
        let i = bucket(us);
        if let Some(c) = self.counts.get_mut(i) {
            *c += 1;
        }
        self.n += 1;
        self.sum += u128::from(us);
        self.max = self.max.max(us);
    }

    /// Records a duration.
    pub fn record(&mut self, d: std::time::Duration) {
        self.record_us(u64::try_from(d.as_micros()).unwrap_or(u64::MAX));
    }

    /// Values recorded.
    #[must_use]
    pub fn len(&self) -> u64 {
        self.n
    }

    /// Whether none was recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// Adds another histogram's counts.
    pub fn merge(&mut self, other: &Hist) {
        for (a, b) in self.counts.iter_mut().zip(&other.counts) {
            *a += b;
        }
        self.n += other.n;
        self.sum += other.sum;
        self.max = self.max.max(other.max);
    }

    /// The value at quantile `q` (0 to 1), in microseconds: the upper bound of its bucket, at most
    /// the largest value recorded.
    #[must_use]
    pub fn quantile_us(&self, q: f64) -> u64 {
        if self.n == 0 {
            return 0;
        }
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a rank between 1 and n"
        )]
        let rank = ((q.clamp(0.0, 1.0) * self.n as f64).ceil() as u64).clamp(1, self.n);
        let mut seen = 0;
        for (i, c) in self.counts.iter().enumerate() {
            seen += c;
            if seen >= rank {
                return upper(i).min(self.max);
            }
        }
        self.max
    }

    /// The summary in milliseconds.
    #[must_use]
    pub fn summary(&self) -> Summary {
        #[allow(
            clippy::cast_precision_loss,
            reason = "a mean of microsecond values for a report"
        )]
        let mean = if self.n == 0 {
            0.0
        } else {
            self.sum as f64 / self.n as f64 / 1_000.0
        };
        Summary {
            n: self.n,
            mean_ms: mean,
            p50_ms: us_to_ms(self.quantile_us(0.50)),
            p90_ms: us_to_ms(self.quantile_us(0.90)),
            p99_ms: us_to_ms(self.quantile_us(0.99)),
            p999_ms: us_to_ms(self.quantile_us(0.999)),
            max_ms: us_to_ms(self.max),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_are_ordered_and_their_bounds_contain_their_values() {
        let mut last = 0;
        for v in (0..200_000_u64).chain([1 << 30, 1 << 40, u64::MAX >> 20]) {
            let i = bucket(v);
            assert!(i >= last || v == 1 << 30, "monotone at {v}");
            last = i;
            assert!(upper(i) >= v, "{v} in bucket {i} with upper {}", upper(i));
            if i > 0 {
                assert!(upper(i - 1) < v, "{v} not in the bucket before");
            }
        }
    }

    #[test]
    fn quantiles_are_within_two_percent() {
        let mut h = Hist::new();
        for v in 1..=100_000_u64 {
            h.record_us(v);
        }
        for (q, want) in [(0.5, 50_000.0), (0.99, 99_000.0), (0.999, 99_900.0)] {
            #[allow(clippy::cast_precision_loss, reason = "test")]
            let got = h.quantile_us(q) as f64;
            assert!((got - want).abs() / want < 0.02, "q{q}: {got} vs {want}");
        }
        assert_eq!(h.quantile_us(1.0), 100_000);
        assert_eq!(h.summary().n, 100_000);
    }

    #[test]
    fn merged_histograms_add_up() {
        let mut a = Hist::new();
        let mut b = Hist::new();
        for v in 0..1_000 {
            a.record_us(v);
            b.record_us(v + 5_000);
        }
        a.merge(&b);
        assert_eq!(a.len(), 2_000);
        assert!(a.quantile_us(0.75) >= 5_000);
        assert_eq!(a.summary().max_ms, 5.999);
        assert!(Hist::new().is_empty());
        assert_eq!(Hist::new().summary(), Summary::default());
    }
}
