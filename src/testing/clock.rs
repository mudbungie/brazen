//! The `Clock` double (arch §9.4): time set explicitly, so fresh/stale branches
//! and device-flow deadlines run with no real time. Atomic, not `Cell`, so the
//! same double serves the `--serve` accept loop's `Sync`-bounded seams.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::store::Clock;

/// A `Clock` whose time is set explicitly — drives fresh/stale branches and
/// device-flow deadlines with no real time (arch §9.4).
pub struct FakeClock {
    now: AtomicU64,
}

impl FakeClock {
    /// A clock reading `now` unix-seconds.
    pub fn new(now: u64) -> Self {
        FakeClock {
            now: AtomicU64::new(now),
        }
    }

    /// Jump the clock to `now`.
    pub fn set(&self, now: u64) {
        self.now.store(now, Ordering::Relaxed);
    }

    /// Advance the clock by `secs` seconds.
    pub fn advance(&self, secs: u64) {
        self.now.fetch_add(secs, Ordering::Relaxed);
    }
}

impl Clock for FakeClock {
    fn now(&self) -> u64 {
        self.now.load(Ordering::Relaxed)
    }
}
