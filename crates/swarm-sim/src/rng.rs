//! A small deterministic generator (xoshiro256**, seeded by SplitMix64): every simulated agent has
//! its own, derived from the run's seed and its index, so a run's choices repeat with its seed.

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
    /// A generator for `seed` and `stream` (an agent's index, say).
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

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        // 53 random bits: the mantissa of an f64.
        #[allow(
            clippy::cast_precision_loss,
            reason = "53 bits fit an f64's mantissa exactly"
        )]
        let x = (self.next_u64() >> 11) as f64;
        x / 9_007_199_254_740_992.0
    }

    /// Uniform in `0..n` (0 for n = 0).
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        // Lemire's multiply-shift; the bias is below 2^-32 for the sizes used here.
        let wide = u128::from(self.next_u64()) * u128::from(n);
        u64::try_from(wide >> 64).unwrap_or(0)
    }

    /// Uniform in `0..n` as a usize.
    pub fn index(&mut self, n: usize) -> usize {
        usize::try_from(self.below(u64::try_from(n).unwrap_or(u64::MAX))).unwrap_or(0)
    }

    /// True with probability `p`.
    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    /// Exponentially distributed with the given mean.
    pub fn exp(&mut self, mean: f64) -> f64 {
        let u = 1.0 - self.unit();
        -u.ln() * mean
    }

    /// Uniform in [lo, hi).
    pub fn between(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    /// An index in `0..n` with a log-uniform (about Zipf with exponent 1) distribution: small
    /// indices are much more likely.
    pub fn zipf(&mut self, n: usize) -> usize {
        if n <= 1 {
            return 0;
        }
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "n is far below 2^53, and the result is clamped into 0..n"
        )]
        let i = ((n as f64 + 1.0).powf(self.unit()) - 1.0).floor() as usize;
        i.min(n - 1)
    }

    /// A version 4 UUID, as an `op_id` (§20.1: at least 96 random bits).
    pub fn uuid(&mut self) -> String {
        let a = self.next_u64();
        let b = self.next_u64();
        format!(
            "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
            a >> 32,
            (a >> 16) & 0xffff,
            a & 0x0fff,
            ((b >> 48) & 0x3fff) | 0x8000,
            b & 0xffff_ffff_ffff
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_repeats_with_its_seed_and_differs_by_stream() {
        let mut a = Rng::new(7, 1);
        let mut b = Rng::new(7, 1);
        let mut c = Rng::new(7, 2);
        let xs: Vec<u64> = (0..8).map(|_| a.next_u64()).collect();
        let ys: Vec<u64> = (0..8).map(|_| b.next_u64()).collect();
        let zs: Vec<u64> = (0..8).map(|_| c.next_u64()).collect();
        assert_eq!(xs, ys);
        assert_ne!(xs, zs);
    }

    #[test]
    fn its_distributions_stay_in_range_and_have_their_means() {
        let mut r = Rng::new(1, 0);
        let n = 100_000;
        let mut sum = 0.0;
        for _ in 0..n {
            let u = r.unit();
            assert!((0.0..1.0).contains(&u));
            assert!(r.below(10) < 10);
            assert!(r.zipf(1_000) < 1_000);
            sum += r.exp(40.0);
        }
        let mean = sum / f64::from(n);
        assert!((38.0..42.0).contains(&mean), "{mean}");
        let low = (0..10_000).filter(|_| r.zipf(1_000) < 10).count();
        assert!(low > 2_500, "a log-uniform index favours the head: {low}");
    }

    #[test]
    fn a_uuid_has_the_version_4_form() {
        let u = Rng::new(3, 3).uuid();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
        assert!(matches!(&u[19..20], "8" | "9" | "a" | "b"));
    }
}
