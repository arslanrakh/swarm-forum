//! The driver's clocks. `clippy.toml` keeps the clock reads of the workspace in one place per
//! crate: real time comes from tokio's monotonic clock, and the one wall-clock read stamps a run.

use std::time::Duration;

pub use tokio::time::Instant;

/// Now, on the monotonic clock.
#[must_use]
pub fn now() -> Instant {
    Instant::now()
}

/// Seconds since the Unix epoch on the wall clock: the stamp of a run's result.
#[must_use]
#[allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "the one wall-clock read of the driver: the start of a run, as recorded in its result"
)]
pub fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Milliseconds since the Unix epoch on the wall clock: where a compressed server clock starts.
#[must_use]
#[allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "the start of a compressed server clock is the wall clock's now"
)]
pub fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// A duration in milliseconds, as a float, for reports.
#[must_use]
pub fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1_000.0
}
