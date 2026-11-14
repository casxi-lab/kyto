//! The clock of a bench of real devices, and the clock with which the
//! command port and the serial console stamp and wait.

use std::time::{Duration, Instant};

/// A clock that stamps and waits in nanoseconds since its origin. Its
/// copies share the origin, so that a thread of a driver stamps with a copy
/// of the bench's clock.
pub trait StampClock: Copy + Send + Sync + 'static {
    /// Nanoseconds since the origin.
    fn now_ns(&self) -> u64;
    /// Returns at `deadline` or later, with the time it returned.
    fn wait_until_ns(&self, deadline: u64) -> u64;
}

/// The clock of a bench of real devices: nanoseconds since it was started,
/// read from the monotonic clock of the operating system, which does not
/// run while the host sleeps. It is the one source of the bench's times:
/// the copied writer reads the system clock only to bound its requests and
/// to note their times, which kyto discards.
#[derive(Clone, Copy, Debug)]
pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    /// A clock whose origin is now.
    pub fn start() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl StampClock for SystemClock {
    fn now_ns(&self) -> u64 {
        u64::try_from(self.origin.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }

    /// Sleeps until `deadline`. A sleep that a signal interrupts goes on, so
    /// the wait ends at its deadline only.
    fn wait_until_ns(&self, deadline: u64) -> u64 {
        loop {
            let now = self.now_ns();
            if now >= deadline {
                return now;
            }
            std::thread::sleep(Duration::from_nanos(deadline - now));
        }
    }
}

/// An instant of the system clock a day from now, past the end of any cold
/// boot: the deadline of a session of the copied writer, which bounds each
/// request by the system clock itself.
pub(crate) fn session_end() -> Instant {
    let now = Instant::now();
    now.checked_add(Duration::from_secs(86_400)).unwrap_or(now)
}

#[cfg(test)]
pub(crate) mod stepped {
    //! A clock that only a test moves, for the tests of the command port
    //! and the console: a wait returns once the test has moved the clock to
    //! its deadline, so the test decides when a port is ready, when a chunk
    //! is stamped and when a deadline passes. Real time only bounds a test
    //! that fails.

    use super::StampClock;
    use crate::TEST_WAIT;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    /// No wait in progress.
    const NONE: u64 = u64::MAX;

    /// The clock and the deadline of the wait in progress, shared by copies.
    #[derive(Clone, Copy, Debug)]
    pub struct TestClock {
        now: &'static AtomicU64,
        waiting: &'static AtomicU64,
    }

    impl TestClock {
        /// A clock at 0.
        pub fn at_zero() -> Self {
            Self {
                now: Box::leak(Box::new(AtomicU64::new(0))),
                waiting: Box::leak(Box::new(AtomicU64::new(NONE))),
            }
        }

        /// Moves the clock to `time`, unless it is already later.
        pub fn advance_to(&self, time: u64) {
            self.now.fetch_max(time, Ordering::SeqCst);
        }

        /// Waits until the code under test waits for `deadline`. A test
        /// fails when it has not within [`TEST_WAIT`].
        #[track_caller]
        pub fn await_wait(&self, deadline: u64) {
            let end = Instant::now() + TEST_WAIT;
            while self.waiting.load(Ordering::SeqCst) != deadline {
                assert!(Instant::now() < end, "no wait for {deadline} ns");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    impl StampClock for TestClock {
        fn now_ns(&self) -> u64 {
            self.now.load(Ordering::SeqCst)
        }

        fn wait_until_ns(&self, deadline: u64) -> u64 {
            self.waiting.store(deadline, Ordering::SeqCst);
            let end = Instant::now() + TEST_WAIT;
            loop {
                let now = self.now_ns();
                if now >= deadline {
                    self.waiting.store(NONE, Ordering::SeqCst);
                    return now;
                }
                assert!(
                    Instant::now() < end,
                    "the test clock stayed before {deadline} ns"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_clock_never_goes_back_and_waits_until_its_deadline() {
        let clock = SystemClock::start();
        let copy = clock;
        let mut last = 0;
        for _ in 0..1000 {
            let (first, second) = (clock.now_ns(), copy.now_ns());
            assert!(last <= first && first <= second, "a copy shares the origin");
            last = second;
        }
        let deadline = clock.now_ns() + 1_000_000;
        let woke = clock.wait_until_ns(deadline);
        assert!(woke >= deadline && clock.now_ns() >= woke);
        // A deadline that has passed returns the time now.
        let now = clock.wait_until_ns(0);
        assert!(now >= woke && clock.now_ns() >= now);
    }
}
