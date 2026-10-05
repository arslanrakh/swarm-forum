//! The shell's clocks (design D.8.1: `clock::now_ms`). The core never reads a clock: time enters it
//! through `Ctx::now` (§24, D.1.3). These are the only reads of the system's clocks in the server,
//! which `clippy.toml` enforces for every other file.
//!
//! - [`now_ms`]: the wall clock in milliseconds since the Unix epoch, the `now` of every call before
//!   the core clamps it (T1);
//! - [`Clock`]: what the engine reads it through, so that a test or a simulation can move time by
//!   hand ([`ManualClock`]);
//! - [`Mono`]: a monotonic reading for the writer's own short waits (the commit linger, D.11.2)
//!   and for the age of a queued job (`queue.max_wait`, CS-29), which must not jump when the wall
//!   clock is set (review E4-robust-13).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use forum_core::{Dur, Time};

/// The wall clock, in milliseconds since the Unix epoch; 0 before the epoch.
#[must_use]
#[allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "the one read of the wall clock (clippy.toml: only clock::now_ms reads it)"
)]
pub fn now_ms() -> Time {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(Time(0), |d| {
            Time(u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        })
}

/// Where the engine reads the time from.
pub trait Clock: Send + Sync + 'static {
    /// The current time.
    fn now(&self) -> Time;

    /// A monotonic reading: what the age of a queued job is measured on (CS-29), so that a step of
    /// the wall clock never makes a class busy or keeps it from being busy.
    fn mono(&self) -> Mono {
        Mono::now()
    }
}

/// The system's wall clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Time {
        now_ms()
    }
}

/// A clock that moves only when told: tests and simulations (D.12.6: a fake clock that sometimes
/// goes backwards, which T1 clamps). [`ManualClock::advance`] is time passing, which moves its
/// monotonic reading too; [`ManualClock::set`] is a step of the wall clock, which does not.
#[derive(Debug, Default)]
pub struct ManualClock {
    wall: AtomicU64,
    /// Nanoseconds `advance` added to the monotonic reading.
    elapsed: AtomicU64,
}

impl ManualClock {
    /// A clock showing `t`.
    #[must_use]
    pub fn new(t: Time) -> ManualClock {
        ManualClock {
            wall: AtomicU64::new(t.0),
            elapsed: AtomicU64::new(0),
        }
    }

    /// Shows `t` from now on, earlier or later: a step of the wall clock.
    pub fn set(&self, t: Time) {
        self.wall.store(t.0, Ordering::SeqCst);
    }

    /// Moves forward by `d`: time passing.
    pub fn advance(&self, d: Dur) {
        self.wall.fetch_add(d.0, Ordering::SeqCst);
        self.elapsed
            .fetch_add(d.0.saturating_mul(1_000_000), Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Time {
        Time(self.wall.load(Ordering::SeqCst))
    }

    fn mono(&self) -> Mono {
        Mono::now().plus(Duration::from_nanos(self.elapsed.load(Ordering::SeqCst)))
    }
}

// The feature `bench` is for the scale gates' own build only (scripts/bench sets `forum_bench`).
#[cfg(all(feature = "bench", not(forum_bench)))]
compile_error!(
    "the bench feature is for the scale gates only (scripts/bench/lib/common.sh builds it with \
     --cfg forum_bench under the bench-gates profile): never in a release build"
);

/// A clock that runs `factor` times as fast as real time, from `start` at the moment it is made,
/// and that a gate can move forward at once ([`ScaledClock::jump`]): the scale gates' simulated
/// days, and deadlines that fall due together (feature `bench`, WP-F4, design D.12.8: G-5, G-6,
/// G-7, G-10, and G-12 run on it). It reads the monotonic clock, so a step of the wall clock does
/// not move it.
#[cfg(feature = "bench")]
#[derive(Debug)]
pub struct ScaledClock {
    start: Time,
    origin: Mono,
    factor: u64,
    /// Milliseconds the jumps have added.
    jumped: AtomicU64,
}

#[cfg(feature = "bench")]
impl ScaledClock {
    /// A clock showing `start` now and running `factor` (at least 1) times as fast as real time.
    #[must_use]
    pub fn new(start: Time, factor: u64) -> ScaledClock {
        ScaledClock {
            start,
            origin: Mono::now(),
            factor: factor.max(1),
            jumped: AtomicU64::new(0),
        }
    }

    /// How many times as fast as real time it runs.
    #[must_use]
    pub fn factor(&self) -> u64 {
        self.factor
    }

    /// Moves the clock `ms` forward at once (never back: T1), so that every deadline within `ms`
    /// falls due at the same instant (gates G-5 and G-12).
    pub fn jump(&self, ms: u64) {
        self.jumped.fetch_add(ms, Ordering::SeqCst);
    }

    /// Milliseconds the jumps have added so far.
    #[must_use]
    pub fn jumped(&self) -> u64 {
        self.jumped.load(Ordering::SeqCst)
    }
}

#[cfg(feature = "bench")]
impl Clock for ScaledClock {
    fn now(&self) -> Time {
        let real = Mono::now().since(self.origin).as_nanos();
        let ms = real
            .saturating_mul(u128::from(self.factor))
            .checked_div(1_000_000)
            .unwrap_or(0);
        Time(
            self.start
                .0
                .saturating_add(u64::try_from(ms).unwrap_or(u64::MAX))
                .saturating_add(self.jumped()),
        )
    }
}

/// A wall clock that starts `lag` behind the system's and catches up only when told
/// ([`LaggedClock::catch_up`]), never passing it: the usability lab's seeds (feature `usability`,
/// test only), whose history spans weeks of the forum's time before the agents arrive at the real
/// time. Catching up is time passing, so the clock never goes backwards.
#[cfg(feature = "usability")]
#[derive(Debug)]
pub struct LaggedClock {
    /// Milliseconds still behind the system's clock.
    lag: AtomicU64,
}

#[cfg(feature = "usability")]
impl LaggedClock {
    /// A clock `lag_ms` behind the system's.
    #[must_use]
    pub fn new(lag_ms: u64) -> LaggedClock {
        LaggedClock {
            lag: AtomicU64::new(lag_ms),
        }
    }

    /// Moves forward by `ms`, at most to the system's clock; returns the lag left.
    pub fn catch_up(&self, ms: u64) -> u64 {
        let mut left = 0;
        let _ = self
            .lag
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |l| {
                left = l.saturating_sub(ms);
                Some(left)
            });
        left
    }

    /// Milliseconds still behind the system's clock.
    #[must_use]
    pub fn lag(&self) -> u64 {
        self.lag.load(Ordering::SeqCst)
    }
}

#[cfg(feature = "usability")]
impl Clock for LaggedClock {
    fn now(&self) -> Time {
        Time(now_ms().0.saturating_sub(self.lag()))
    }
}

/// A monotonic reading: nanoseconds since the first reading in this process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Mono(u64);

impl Mono {
    /// Now.
    #[must_use]
    #[allow(
        clippy::disallowed_types,
        clippy::disallowed_methods,
        reason = "the writer's own short waits (the commit linger) need a clock that never jumps"
    )]
    pub fn now() -> Mono {
        static ORIGIN: OnceLock<std::time::Instant> = OnceLock::new();
        let origin = ORIGIN.get_or_init(std::time::Instant::now);
        Mono(u64::try_from(origin.elapsed().as_nanos()).unwrap_or(u64::MAX))
    }

    /// The time from `earlier` to this reading, zero if `earlier` is later.
    #[must_use]
    pub fn since(self, earlier: Mono) -> Duration {
        Duration::from_nanos(self.0.saturating_sub(earlier.0))
    }

    /// The reading `d` after this one.
    #[must_use]
    pub fn plus(self, d: Duration) -> Mono {
        Mono(
            self.0
                .saturating_add(u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)),
        )
    }
}

/// The real-time duration until `at` on `clock`, zero if it has passed, at most `cap`.
#[must_use]
pub fn until(clock: &dyn Clock, at: Time, cap: Duration) -> Duration {
    let ms = at.0.saturating_sub(clock.now().0);
    Duration::from_millis(ms).min(cap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manual_clock_moves_only_when_told() {
        let c = ManualClock::new(Time(1_000));
        assert_eq!(c.now(), Time(1_000));
        c.advance(Dur(500));
        assert_eq!(c.now(), Time(1_500));
        c.set(Time(10));
        assert_eq!(c.now(), Time(10), "a fake clock may go backwards (D.12.6)");
        assert_eq!(
            until(&c, Time(2_010), Duration::from_secs(60)),
            Duration::from_secs(2)
        );
        assert_eq!(until(&c, Time(5), Duration::from_secs(60)), Duration::ZERO);
        assert_eq!(
            until(&c, Time(1_000_000), Duration::from_secs(1)),
            Duration::from_secs(1)
        );
    }

    #[cfg(feature = "bench")]
    #[test]
    fn the_scaled_clock_runs_faster_from_its_start() {
        let c = ScaledClock::new(Time(5_000), 1_000);
        let a = c.now();
        assert!(a >= Time(5_000));
        std::thread::sleep(Duration::from_millis(20));
        let b = c.now();
        assert!(
            b.0 >= a.0 + 20_000,
            "20 ms of real time is at least 20 s of its time"
        );
        assert_eq!(ScaledClock::new(Time(0), 0).factor(), 1);
        c.jump(3_600_000);
        assert!(
            c.now().0 >= b.0 + 3_600_000,
            "a jump moves it forward at once"
        );
        assert_eq!(c.jumped(), 3_600_000);
    }

    #[test]
    fn a_manual_clock_s_steps_leave_its_monotonic_reading_alone() {
        let c = ManualClock::new(Time(1_000));
        let m = c.mono();
        c.set(Time(5_000_000));
        assert!(
            c.mono().since(m) < Duration::from_secs(1),
            "a step is not time passing"
        );
        c.set(Time(10));
        c.advance(Dur(15_000));
        assert_eq!(c.now(), Time(15_010));
        assert!(c.mono().since(m) >= Duration::from_secs(15));
        assert_eq!(
            m.plus(Duration::from_secs(2)).since(m),
            Duration::from_secs(2)
        );
    }

    #[cfg(feature = "usability")]
    #[test]
    fn the_lagged_clock_catches_up_to_the_system_clock_and_never_passes_it() {
        let day = 86_400_000;
        let c = LaggedClock::new(3 * day);
        let behind = now_ms().0.saturating_sub(c.now().0);
        assert!((3 * day - 1_000..=3 * day).contains(&behind), "{behind}");
        assert_eq!(c.catch_up(day), 2 * day);
        assert_eq!(c.catch_up(5 * day), 0, "never past the system clock");
        assert!(now_ms().0.saturating_sub(c.now().0) < 1_000);
    }

    #[test]
    fn the_wall_clock_is_after_2026_and_the_monotonic_one_never_goes_back() {
        assert!(now_ms() > Time(1_767_225_600_000));
        let a = Mono::now();
        let b = Mono::now();
        assert!(b >= a);
        assert_eq!(a.since(b), Duration::ZERO);
    }
}
