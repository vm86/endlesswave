//! The time the radio keeps, behind a trait so that tests can set it. Task 3.0.3 of docs/rust-port-radio.md.
//!
//! The Python radio reads `time.monotonic()` (how long a track has been heard, not counting the pauses) and
//! `time.time()` (when a blend was made). Here `monotonic` runs on tokio's clock, which a test stops and moves with
//! `tokio::time::pause` and `advance`; for tests with no runtime there is [`ManualClock`].

use std::sync::Mutex;

pub trait Clock: Send + Sync {
    /// Seconds from some point that never goes back; only the differences count.
    fn monotonic(&self) -> f64;
    /// Seconds since the Unix epoch.
    fn wall(&self) -> f64;
}

/// The real time (or tokio's paused time, in a test that paused it).
pub struct SystemClock {
    start: tokio::time::Instant,
}

impl SystemClock {
    /// Must be made inside a tokio runtime when the time is to be paused (an `Instant` needs the clock of one).
    pub fn new() -> Self {
        Self {
            start: tokio::time::Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn monotonic(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn wall(&self) -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64())
    }
}

/// A clock that is set by hand.
#[derive(Default)]
pub struct ManualClock {
    now: Mutex<f64>,
}

impl ManualClock {
    pub fn new(start: f64) -> Self {
        Self {
            now: Mutex::new(start),
        }
    }

    pub fn advance(&self, seconds: f64) {
        *self.now.lock().expect("not poisoned") += seconds;
    }

    pub fn set(&self, seconds: f64) {
        *self.now.lock().expect("not poisoned") = seconds;
    }
}

impl Clock for ManualClock {
    fn monotonic(&self) -> f64 {
        *self.now.lock().expect("not poisoned")
    }

    /// The same number: a test that cares about the wall clock reads it as the epoch plus what it set.
    fn wall(&self) -> f64 {
        *self.now.lock().expect("not poisoned")
    }
}
