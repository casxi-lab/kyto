//! The serial console of a bench of real devices: a terminal opened with
//! the copied transport for one cold boot, at its raw 115200 8N1, whose
//! input is stamped by the bench clock when it arrives.
//!
//! The copied reader thread reads what the port receives, at most 4096
//! bytes at a time, and a thread of this module stamps each chunk with a
//! copy of the clock as it arrives, so a stamp does not depend on when the
//! run takes the chunk. The adapter's own buffering delays it. An open waits
//! [`OPEN_SETTLE`] and keeps only what is stamped from then on, which
//! discards the bytes that the host or the adapter held before.
//!
//! An error is `<step>: <text>`, as of the command port, and never names
//! the port.

use std::io::Write;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::Failure;
use crate::clock::{StampClock, session_end};
use crate::command_port::PortError;
use crate::timing::Timing;
use crate::tty::{TtyPort, TtyWriter, os_error};

/// How long an open waits before it keeps what arrives.
pub const OPEN_SETTLE: Duration = Duration::from_millis(50);

/// The time values of the copied transport, of which a port reads two: the
/// longest wait of the reader thread, 100 ms, which is also the longest
/// slice of a receive, and the longest transmission of one request, 2 s.
const TIMING: Timing = Timing::PI5;

/// The bytes of the largest request of the copied writer.
const REQUEST_LIMIT: usize = 128;

/// What [`SerialConsole::receive`] returned: bytes with the time they
/// arrived, a deadline with the time now, or the end of the port's stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Got {
    Bytes { at: u64, bytes: Vec<u8> },
    Deadline { at: u64 },
    Closed { at: u64 },
}

/// A board's serial console on the terminal at a path.
pub struct SerialConsole<C: StampClock> {
    path: String,
    clock: C,
    interrupt: Option<&'static AtomicI32>,
    session: Option<Session>,
    /// Where the stamping thread reports each chunk it stamped, with its
    /// length, for a test to wait on.
    #[cfg(test)]
    stamps: Option<mpsc::Sender<(u64, usize)>>,
    /// Where a receive reports each wait for a chunk that it begins, with
    /// its length, for a test to wait on.
    #[cfg(test)]
    waits: Option<mpsc::Sender<Duration>>,
}

/// An open port with its writer and its stamped chunks.
struct Session {
    port: TtyPort,
    writer: TtyWriter,
    chunks: Receiver<(u64, Vec<u8>)>,
    /// The thread that stamps the chunks, which ends with the reader thread.
    stamper: JoinHandle<()>,
    /// A chunk taken from the channel and kept for a later receive.
    held: Option<(u64, Vec<u8>)>,
    /// The time from which chunks are kept.
    ready_at: u64,
}

fn not_open(step: &'static str) -> PortError {
    PortError::new(step, "the console is not open")
}

impl<C: StampClock> SerialConsole<C> {
    /// The console on `path`, stamping with `clock`; a receive ends early
    /// once `interrupt` holds a signal. Nothing is opened.
    pub fn new(path: String, clock: C, interrupt: Option<&'static AtomicI32>) -> Self {
        Self {
            path,
            clock,
            interrupt,
            session: None,
            #[cfg(test)]
            stamps: None,
            #[cfg(test)]
            waits: None,
        }
    }

    /// Opens the port exclusively and returns the time from which every
    /// byte is kept, [`OPEN_SETTLE`] after the open; when that is after
    /// `deadline`, it closes the port again and fails.
    pub fn open(&mut self, deadline: u64) -> Result<u64, PortError> {
        if self.session.is_some() {
            return Err(PortError::new("open", "the console is already open"));
        }
        let port = TtyPort::open(&self.path).map_err(|failure| self.open_error(failure))?;
        let (sender, read) = mpsc::channel::<Vec<u8>>();
        let (forward, chunks) = mpsc::channel();
        let notes = Arc::new(Mutex::new(Vec::new()));
        let writer = port
            .start_reader(sender, &TIMING)
            .and_then(|()| port.writer(session_end(), notes, &TIMING))
            .map_err(|Failure(text)| PortError::new("open", text))?;
        let clock = self.clock;
        #[cfg(test)]
        let stamps = self.stamps.clone();
        // The thread ends with the reader thread, which ends when the port is
        // finished; a chunk that comes after the console was closed is dropped.
        let stamper = std::thread::spawn(move || {
            while let Ok(bytes) = read.recv() {
                let at = clock.now_ns();
                #[cfg(test)]
                let count = bytes.len();
                let _ = forward.send((at, bytes));
                #[cfg(test)]
                if let Some(stamps) = &stamps {
                    let _ = stamps.send((at, count));
                }
            }
        });
        let settle = u64::try_from(OPEN_SETTLE.as_nanos()).unwrap_or(u64::MAX);
        let ready_at = self
            .clock
            .wait_until_ns(self.clock.now_ns().saturating_add(settle));
        self.session = Some(Session {
            port,
            writer,
            chunks,
            stamper,
            held: None,
            ready_at,
        });
        if ready_at > deadline {
            // Closed as `close` closes it, so that the next open receives
            // every byte.
            let late = PortError::new("open", "the port was not ready by the deadline");
            let _ = self.close();
            return Err(late);
        }
        Ok(ready_at)
    }

    /// The step and text of a failed open of the copied transport, without
    /// the name of the port that it writes before the text of a refused
    /// open. A refused lock is the step `lock`.
    fn open_error(&self, Failure(text): Failure) -> PortError {
        match text.strip_prefix(&format!("{:?}: ", self.path)) {
            Some(text) => PortError::new("open", text),
            None if text == os_error(libc::EWOULDBLOCK) => PortError::new("lock", text),
            None => PortError::new("open", text),
        }
    }

    /// The next chunk stamped at or before `deadline`, or the deadline at
    /// the time now. A chunk stamped later is held for the next receive. A
    /// receive whose deadline has passed takes only what has arrived and
    /// never waits, and once the interrupt flag holds a signal, a receive
    /// returns the deadline at once. The end of the port's stream is
    /// [`Got::Closed`], or the error of the read that ended it.
    pub fn receive(&mut self, deadline: u64) -> Result<Got, PortError> {
        let session = self.session.as_mut().ok_or_else(|| not_open("receive"))?;
        loop {
            let next = match session.held.take() {
                Some(chunk) => Ok(chunk),
                None => session.chunks.try_recv(),
            };
            match next {
                Ok((at, _)) if at < session.ready_at => continue,
                Ok((at, bytes)) if at <= deadline => return Ok(Got::Bytes { at, bytes }),
                Ok(chunk) => {
                    session.held = Some(chunk);
                    return Ok(Got::Deadline {
                        at: self.clock.now_ns(),
                    });
                }
                Err(TryRecvError::Disconnected) => {
                    return match session.port.read_error() {
                        Some(text) => Err(PortError::new("read", text)),
                        None => Ok(Got::Closed {
                            at: self.clock.now_ns(),
                        }),
                    };
                }
                Err(TryRecvError::Empty) => {}
            }
            let now = self.clock.now_ns();
            let interrupted = self
                .interrupt
                .is_some_and(|flag| flag.load(Ordering::SeqCst) != 0);
            if now >= deadline || interrupted {
                return Ok(Got::Deadline { at: now });
            }
            let slice = TIMING.reader_poll.min(Duration::from_nanos(deadline - now));
            #[cfg(test)]
            if let Some(waits) = &self.waits {
                let _ = waits.send(slice);
            }
            session.held = session.chunks.recv_timeout(slice).ok();
        }
    }

    /// Hands every byte to the port, in requests of at most 128 bytes, and
    /// returns the time when the last was written.
    pub fn send(&mut self, bytes: &[u8]) -> Result<u64, PortError> {
        let session = self.session.as_mut().ok_or_else(|| not_open("send"))?;
        for request in bytes.chunks(REQUEST_LIMIT) {
            let written = session.writer.write_all(request);
            written.map_err(|error| PortError::new("write", error.to_string()))?;
        }
        Ok(self.clock.now_ns())
    }

    /// Restores and closes the port and discards what it still holds. It
    /// returns once the reader thread has stopped, within one wait of that
    /// thread, so that the next open receives every byte. Closing a console
    /// that is not open does nothing.
    pub fn close(&mut self) -> Result<(), PortError> {
        let Some(Session {
            mut port,
            writer,
            chunks,
            stamper,
            ..
        }) = self.session.take()
        else {
            return Ok(());
        };
        drop((writer, chunks));
        let finished = port.finish();
        let _ = stamper.join();
        finished.map_err(|Failure(text)| PortError::new("close", text))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::TEST_WAIT;
    use crate::clock::stepped::TestClock;
    use crate::pty::{Pty, exclusive_open_refused, lock_refused};
    use std::collections::BTreeMap;

    const MS: u64 = 1_000_000;
    const SETTLE: u64 = 50 * MS;
    const SECOND: u64 = 1_000 * MS;

    impl<C: StampClock> SerialConsole<C> {
        /// Reports each chunk that the stamping thread stamps, with its
        /// length, from the next open on.
        fn report_stamps(&mut self) -> Receiver<(u64, usize)> {
            let (sender, stamps) = mpsc::channel();
            self.stamps = Some(sender);
            stamps
        }

        /// Reports each wait for a chunk that a receive begins, with its
        /// length.
        fn report_waits(&mut self) -> Receiver<Duration> {
            let (sender, waits) = mpsc::channel();
            self.waits = Some(sender);
            waits
        }
    }

    /// A receive by `deadline` on a thread of its own, for a deadline that
    /// the clock has not reached, and what waits for it to return the
    /// console and its result. A test fails when the receive has not
    /// returned within [`TEST_WAIT`].
    fn receive_apart(
        mut console: SerialConsole<TestClock>,
        deadline: u64,
    ) -> impl FnOnce() -> (SerialConsole<TestClock>, Result<Got, PortError>) {
        let (sender, returned) = mpsc::channel();
        std::thread::spawn(move || {
            let got = console.receive(deadline);
            let _ = sender.send((console, got));
        });
        move || {
            returned
                .recv_timeout(TEST_WAIT)
                .expect("the receive returned")
        }
    }

    /// The stamps of chunks of `count` bytes in all. A test fails when
    /// they have not been stamped within [`TEST_WAIT`].
    #[track_caller]
    fn stamped(stamps: &Receiver<(u64, usize)>, count: usize) -> Vec<u64> {
        let (mut seen, mut at) = (0, Vec::new());
        while seen < count {
            let (stamp, length) = stamps.recv_timeout(TEST_WAIT).expect("stamped bytes");
            seen += length;
            at.push(stamp);
        }
        at
    }

    /// A console on `pty` whose stamps are reported.
    fn console(pty: &Pty, clock: TestClock) -> (SerialConsole<TestClock>, Receiver<(u64, usize)>) {
        let mut console = SerialConsole::new(pty.path().to_string(), clock, None);
        let stamps = console.report_stamps();
        (console, stamps)
    }

    /// Opens `console` on another thread, runs `during` while the open
    /// waits for the settle time, and moves `clock` over it; what the open
    /// returned.
    fn open_settled(
        console: &mut SerialConsole<TestClock>,
        clock: TestClock,
        during: impl FnOnce(),
    ) -> Result<u64, PortError> {
        let start = clock.now_ns();
        std::thread::scope(|scope| {
            let open = scope.spawn(|| console.open(start + SECOND));
            clock.await_wait(start + SETTLE);
            during();
            clock.advance_to(start + SETTLE);
            open.join().unwrap()
        })
    }

    /// A console open on `pty`, ready at the settle time after the clock's
    /// time now.
    pub(crate) fn opened(pty: &Pty, clock: TestClock) -> SerialConsole<TestClock> {
        let start = clock.now_ns();
        let (mut console, _) = console(pty, clock);
        assert_eq!(open_settled(&mut console, clock, || {}), Ok(start + SETTLE));
        console
    }

    /// What `console` received by `deadline`, by stamp, up to the deadline.
    fn received(console: &mut SerialConsole<TestClock>, deadline: u64) -> BTreeMap<u64, Vec<u8>> {
        let mut received = BTreeMap::<u64, Vec<u8>>::new();
        loop {
            match console.receive(deadline) {
                Ok(Got::Bytes { at, bytes }) => received.entry(at).or_default().extend(bytes),
                Ok(Got::Deadline { .. }) => return received,
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn the_console_keeps_raw_bytes_in_order() {
        let pty = Pty::open();
        let clock = TestClock::at_zero();
        let (mut console, stamps) = console(&pty, clock);
        assert_eq!(open_settled(&mut console, clock, || {}), Ok(SETTLE));
        let parts: [&[u8]; 3] = [b"\x00\xff\r\n", b"a\x7f\x03\x1a\x11", b"\r\r\n\n"];
        let mut expected = BTreeMap::new();
        for (part, at) in parts
            .into_iter()
            .zip([SETTLE, SETTLE + MS, SETTLE + 2 * MS])
        {
            clock.advance_to(at);
            pty.write(part);
            assert!(
                stamped(&stamps, part.len())
                    .iter()
                    .all(|stamp| *stamp == at)
            );
            expected.insert(at, part.to_vec());
        }
        clock.advance_to(SETTLE + 3 * MS);
        assert_eq!(received(&mut console, SETTLE + 3 * MS), expected);
        console.close().unwrap();
    }

    #[test]
    fn a_chunk_that_came_before_a_past_deadline_is_returned() {
        let pty = Pty::open();
        let clock = TestClock::at_zero();
        let (mut console, stamps) = console(&pty, clock);
        assert_eq!(open_settled(&mut console, clock, || {}), Ok(SETTLE));
        for (part, at) in [(b"before", SETTLE + 10 * MS), (b"after!", SETTLE + 30 * MS)] {
            clock.advance_to(at);
            pty.write(part);
            stamped(&stamps, part.len());
        }
        // The deadline has passed: the chunk stamped before it comes without
        // a wait, and the one stamped after it is held for the next receive.
        let before = BTreeMap::from([(SETTLE + 10 * MS, b"before".to_vec())]);
        assert_eq!(received(&mut console, SETTLE + 20 * MS), before);
        let after = BTreeMap::from([(SETTLE + 30 * MS, b"after!".to_vec())]);
        assert_eq!(received(&mut console, SETTLE + 30 * MS), after);
        console.close().unwrap();
    }

    #[test]
    fn a_send_of_300_bytes_arrives_whole() {
        let pty = Pty::open();
        let clock = TestClock::at_zero();
        let mut console = opened(&pty, clock);
        let bytes: Vec<u8> = (0..300u32).map(|byte| (byte % 251) as u8).collect();
        let board = pty.board(|master| master.read(300));
        clock.advance_to(SETTLE + MS);
        assert_eq!(console.send(&bytes), Ok(SETTLE + MS));
        assert_eq!(board.join(), bytes);
        console.close().unwrap();
    }

    #[test]
    fn the_console_opens_again_after_it_closed() {
        let pty = Pty::open();
        let clock = TestClock::at_zero();
        let (mut console, stamps) = console(&pty, clock);
        for start in [0, SECOND] {
            clock.advance_to(start);
            assert_eq!(open_settled(&mut console, clock, || {}), Ok(start + SETTLE));
            let again = console.open(start + SECOND);
            assert_eq!(
                again,
                Err(PortError::new("open", "the console is already open"))
            );
            pty.write(b"x");
            stamped(&stamps, 1);
            let x = BTreeMap::from([(start + SETTLE, b"x".to_vec())]);
            assert_eq!(received(&mut console, start + SETTLE), x);
            console.close().unwrap();
            assert_eq!(console.receive(start), Err(not_open("receive")));
        }
    }

    #[test]
    fn bytes_before_the_console_was_ready_are_discarded() {
        let pty = Pty::open();
        let clock = TestClock::at_zero();
        let (mut console, stamps) = console(&pty, clock);
        let ready = open_settled(&mut console, clock, || {
            pty.write(b"stale");
            assert!(stamped(&stamps, 5).iter().all(|stamp| *stamp == 0));
        });
        assert_eq!(ready, Ok(SETTLE));
        pty.write(b"fresh");
        stamped(&stamps, 5);
        let fresh = BTreeMap::from([(SETTLE, b"fresh".to_vec())]);
        assert_eq!(received(&mut console, SETTLE), fresh);
        console.close().unwrap();
    }

    #[test]
    fn an_open_not_ready_by_its_deadline_releases_the_port() {
        let pty = Pty::open();
        let clock = TestClock::at_zero();
        let (mut console, stamps) = console(&pty, clock);
        let late = std::thread::scope(|scope| {
            let open = scope.spawn(|| console.open(SETTLE - 1));
            clock.await_wait(SETTLE);
            clock.advance_to(SETTLE);
            open.join().unwrap()
        });
        let refused = PortError::new("open", "the port was not ready by the deadline");
        assert_eq!(late, Err(refused));
        // The stamping thread has ended, and so has the reader, whose end
        // ends it: nothing else holds a sender of the stamps.
        console.stamps = None;
        assert_eq!(stamps.try_recv(), Err(TryRecvError::Disconnected));
        // The port was restored, released and unlocked: it opens again at
        // once, and the new reader receives every byte.
        let stamps = console.report_stamps();
        assert_eq!(open_settled(&mut console, clock, || {}), Ok(2 * SETTLE));
        pty.write(b"x");
        assert_eq!(stamped(&stamps, 1), [2 * SETTLE]);
        let x = BTreeMap::from([(2 * SETTLE, b"x".to_vec())]);
        assert_eq!(received(&mut console, 2 * SETTLE), x);
        console.close().unwrap();
    }

    #[test]
    fn an_interrupt_ends_a_receive_early() {
        let pty = Pty::open();
        let clock = TestClock::at_zero();
        let flag: &'static AtomicI32 = Box::leak(Box::new(AtomicI32::new(0)));
        let mut console = SerialConsole::new(pty.path().to_string(), clock, Some(flag));
        let waits = console.report_waits();
        assert_eq!(open_settled(&mut console, clock, || {}), Ok(SETTLE));
        // The clock never reaches the deadline. The signal comes while the
        // receive waits for a chunk, and the receive returns at the end of
        // that wait of the reader, or of the next at the latest.
        let returned = receive_apart(console, SECOND);
        let wait = waits.recv_timeout(TEST_WAIT).expect("a wait for a chunk");
        assert_eq!(wait, TIMING.reader_poll);
        flag.store(libc::SIGINT, Ordering::SeqCst);
        let (mut console, got) = returned();
        assert_eq!(got, Ok(Got::Deadline { at: SETTLE }));
        assert!(waits.try_iter().count() <= 1, "one more wait at most");
        console.close().unwrap();
    }

    #[test]
    fn a_hang_up_names_no_port() {
        let mut pty = Pty::open();
        let clock = TestClock::at_zero();
        let console = opened(&pty, clock);
        pty.close_master();
        // A Darwin pseudo-terminal ends its stream, and a Linux one fails
        // the read with `EIO`, though the clock never reaches the deadline.
        let (mut console, got) = receive_apart(console, SECOND)();
        match got {
            Ok(Got::Closed { at }) => assert_eq!(at, SETTLE),
            Err(error) => {
                assert_eq!(
                    error,
                    PortError::new("read", "Input/output error (os error 5)")
                );
            }
            other => panic!("{other:?}"),
        }
        // Restoring a hung-up terminal can fail; the error names no port.
        if let Err(error) = console.close() {
            assert!(!error.to_string().contains(pty.path()), "{error}");
        }
    }

    #[test]
    fn a_second_open_is_refused() {
        let pty = Pty::open();
        let clock = TestClock::at_zero();
        let mut first = opened(&pty, clock);
        let held = pty.attributes();
        let mut second = SerialConsole::new(pty.path().to_string(), clock, None);
        let expected = if exclusive_open_refused() {
            PortError::new("open", "Device or resource busy (os error 16)")
        } else {
            PortError::new("lock", lock_refused().0)
        };
        assert_eq!(second.open(SECOND), Err(expected));
        assert_eq!(
            pty.attributes(),
            held,
            "the first console's port is unchanged"
        );
        assert_eq!(first.send(b"?"), Ok(SETTLE));
        assert_eq!(pty.read(1), b"?");
        first.close().unwrap();
    }
}
