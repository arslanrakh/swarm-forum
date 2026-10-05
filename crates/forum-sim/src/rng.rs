//! The simulation's own deterministic generator: xoshiro256**, seeded by SplitMix64.
//!
//! Everything the core sees is an integer, so every draw that becomes a time, a duration, or a
//! choice is integer arithmetic: `below` is Lemire's unbiased multiply-shift, `chance` compares
//! with a probability in parts per million, and `exp_ms` computes `−ln U` in Q32 fixed point with
//! an integer `log2` (the squaring method), so a run repeats bit for bit on any machine. Floats
//! appear only in the report's statistics.

/// Parts per million: the unit of every probability.
pub const PPM: u32 = 1_000_000;

/// `ln 2` in Q32: `round(0.693147180559945 × 2³²)`.
const LN2_Q32: u128 = 2_977_044_472;

/// A xoshiro256** generator.
#[derive(Clone, Debug)]
pub struct Rng {
    s: [u64; 4],
}

fn splitmix(x: &mut u64) -> u64 {
    *x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl Rng {
    /// A generator for `seed` and `stream`: different streams of one seed are independent.
    #[must_use]
    pub fn new(seed: u64, stream: u64) -> Rng {
        let mut x = seed ^ stream.wrapping_mul(0xD1B5_4A32_D192_ED03);
        Rng {
            s: [
                splitmix(&mut x),
                splitmix(&mut x),
                splitmix(&mut x),
                splitmix(&mut x),
            ],
        }
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Uniform in `0..n`, without bias (Lemire's method with rejection); 0 for `n = 0`.
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        let mut m = u128::from(self.next_u64()) * u128::from(n);
        let mut low = m as u64;
        if low < n {
            let threshold = n.wrapping_neg() % n;
            while low < threshold {
                m = u128::from(self.next_u64()) * u128::from(n);
                low = m as u64;
            }
        }
        (m >> 64) as u64
    }

    /// Uniform in `lo..=hi` (`lo` when `hi < lo`).
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            return lo;
        }
        lo + self.below(hi - lo + 1)
    }

    /// Uniform index into a slice of length `n`.
    pub fn index(&mut self, n: usize) -> usize {
        self.below(n as u64) as usize
    }

    /// True with probability `ppm` parts per million.
    pub fn chance(&mut self, ppm: u32) -> bool {
        self.below(u64::from(PPM)) < u64::from(ppm)
    }

    /// `−ln U` for `U` uniform in `(0, 1]`, in Q32 fixed point: integer arithmetic only.
    pub fn neg_ln_q32(&mut self) -> u64 {
        // U = v / 2^53 with v uniform in 1..=2^53, so −ln U = (53 − log2 v) × ln 2.
        let v = (self.next_u64() >> 11) + 1;
        let log2v = log2_q32(v);
        let neg_log2 = (53_u128 << 32).saturating_sub(u128::from(log2v));
        ((neg_log2 * LN2_Q32) >> 32) as u64
    }

    /// An exponentially distributed duration with mean `mean_ms`, in milliseconds.
    pub fn exp_ms(&mut self, mean_ms: u64) -> u64 {
        let x = u128::from(self.neg_ln_q32());
        ((u128::from(mean_ms) * x) >> 32) as u64
    }

    /// A Fisher–Yates shuffle.
    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        let mut i = v.len();
        while i > 1 {
            let j = self.index(i);
            i -= 1;
            v.swap(i, j);
        }
    }
}

/// `log2 v` in Q32 for `v ≥ 1`: the integer part from the leading zeros, then 32 fractional bits by
/// repeated squaring of the mantissa in Q63.
#[must_use]
pub fn log2_q32(v: u64) -> u64 {
    if v == 0 {
        return 0;
    }
    let int = 63 - u64::from(v.leading_zeros());
    // The mantissa in [1, 2) as Q63.
    let mut m: u128 = u128::from(v) << (63 - int);
    let two: u128 = 1 << 64;
    let mut frac: u64 = 0;
    for bit in (0..32).rev() {
        m = (m * m) >> 63;
        if m >= two {
            frac |= 1 << bit;
            m >>= 1;
        }
    }
    (int << 32) | frac
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_repeats_and_streams_differ() {
        let mut a = Rng::new(1, 0);
        let mut b = Rng::new(1, 0);
        let mut c = Rng::new(1, 1);
        let xa: Vec<u64> = (0..16).map(|_| a.next_u64()).collect();
        let xb: Vec<u64> = (0..16).map(|_| b.next_u64()).collect();
        let xc: Vec<u64> = (0..16).map(|_| c.next_u64()).collect();
        assert_eq!(xa, xb);
        assert_ne!(xa, xc);
    }

    #[test]
    fn below_stays_in_range_and_covers_it() {
        let mut r = Rng::new(7, 0);
        let mut seen = [0_u32; 7];
        for _ in 0..7_000 {
            let x = r.below(7);
            assert!(x < 7);
            seen[x as usize] += 1;
        }
        assert!(seen.iter().all(|&n| (850..1_150).contains(&n)), "{seen:?}");
        assert_eq!(r.below(0), 0);
        assert_eq!(r.range(5, 5), 5);
        assert_eq!(r.range(9, 3), 9);
    }

    #[test]
    fn chance_matches_its_probability() {
        let mut r = Rng::new(3, 0);
        let hits = (0..100_000).filter(|_| r.chance(250_000)).count();
        assert!((24_000..26_000).contains(&hits), "{hits}");
        assert!(!(0..1_000).any(|_| r.chance(0)));
        assert!((0..1_000).all(|_| r.chance(PPM)));
    }

    #[test]
    fn log2_is_exact_on_powers_and_close_elsewhere() {
        assert_eq!(log2_q32(1), 0);
        assert_eq!(log2_q32(2), 1 << 32);
        assert_eq!(log2_q32(1 << 40), 40 << 32);
        // log2 3 = 1.584962500721156
        let got = log2_q32(3) as f64 / 4_294_967_296.0;
        assert!((got - 1.584_962_500_721_156).abs() < 1e-8, "{got}");
        let got = log2_q32(1_000_003) as f64 / 4_294_967_296.0;
        assert!((got - 1_000_003_f64.log2()).abs() < 1e-8, "{got}");
    }

    #[test]
    fn exp_has_its_mean_and_its_tail() {
        let mut r = Rng::new(11, 0);
        let n = 200_000_u64;
        let mean = 3_600_000_u64;
        let xs: Vec<u64> = (0..n).map(|_| r.exp_ms(mean)).collect();
        let avg = xs.iter().sum::<u64>() as f64 / n as f64;
        assert!((avg / mean as f64 - 1.0).abs() < 0.01, "{avg}");
        // P(X > 2 × mean) = e^-2 = 0.1353
        let tail = xs.iter().filter(|&&x| x > 2 * mean).count() as f64 / n as f64;
        assert!((tail - 0.1353).abs() < 0.005, "{tail}");
        assert_eq!(r.exp_ms(0), 0);
    }

    #[test]
    fn shuffle_permutes() {
        let mut r = Rng::new(5, 0);
        let mut v: Vec<u32> = (0..50).collect();
        r.shuffle(&mut v);
        let mut sorted = v.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..50).collect::<Vec<u32>>());
        assert_ne!(v, sorted);
    }
}
