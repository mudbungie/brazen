//! The `Clock` double (arch §9.4): time set explicitly, so fresh/stale branches
//! and device-flow deadlines run with no real time.

use std::cell::Cell;

use crate::store::Clock;

/// A `Clock` whose time is set explicitly — drives fresh/stale branches and
/// device-flow deadlines with no real time (arch §9.4).
pub struct FakeClock {
    now: Cell<u64>,
}

impl FakeClock {
    /// A clock reading `now` unix-seconds.
    pub fn new(now: u64) -> Self {
        FakeClock {
            now: Cell::new(now),
        }
    }

    /// Jump the clock to `now`.
    pub fn set(&self, now: u64) {
        self.now.set(now);
    }

    /// Advance the clock by `secs` seconds.
    pub fn advance(&self, secs: u64) {
        self.now.set(self.now.get() + secs);
    }
}

impl Clock for FakeClock {
    fn now(&self) -> u64 {
        self.now.get()
    }
}
