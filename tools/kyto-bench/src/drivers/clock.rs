//! The clock of a bench of real devices: nanoseconds of the monotonic clock
//! of the host since the bench was opened, read by `kyto-bench-os`.

use kyto_bench_os::clock::{StampClock, SystemClock};

use crate::device::{Clock, Nanos};

impl Clock for SystemClock {
    fn now(&self) -> Nanos {
        Nanos(self.now_ns())
    }

    fn wait_until(&self, deadline: Nanos) -> Nanos {
        Nanos(self.wait_until_ns(deadline.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bench_clock_of_real_devices_waits_until_its_deadline() {
        let clock = SystemClock::start();
        let deadline = clock.now().saturating_add(Nanos(1));
        let woke = clock.wait_until(deadline);
        assert!(woke >= deadline && Clock::now(&clock) >= woke);
    }
}
