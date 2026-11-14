//! The record of a run as it happens. Every event has the time of the bench
//! clock at which its device reported it, or at which the clock was read,
//! and the number of bytes of the cold boot's console output received
//! before it. The console of a cold boot is read only through a [`Capture`]:
//! it keeps the bytes it lets through exactly as they arrived, with the
//! offset and time of each chunk, logs every send, and feeds the same chunks
//! and sends to the [`Matcher`] that decides when to send and when the cold
//! boot ends, in the order in which a reader of the receipt feeds them.

use std::rc::Rc;

use kyto_bench_core::observe::Matcher;
use kyto_bench_core::receipt::{Base64, EndReason, Event, EventKind, MAX_CHUNKS};

use crate::device::{Clock, Console, DeviceError, Nanos, Received, Role};

/// Adds `event`, at `at` after `received` bytes, to `events`; returns its
/// index.
pub fn add(events: &mut Vec<Event>, at: Nanos, received: u64, event: EventKind) -> usize {
    let at_ns = at.0;
    events.push(Event {
        at_ns,
        received,
        event,
    });
    events.len() - 1
}

/// Adds the failure of a device, at `at` after `received` bytes, to
/// `events`.
pub fn failed(events: &mut Vec<Event>, at: Nanos, received: u64, error: &DeviceError) {
    let (role, detail) = (error.role, error.detail.clone());
    add(
        events,
        at,
        received,
        EventKind::DeviceError { role, detail },
    );
}

/// A failure of the console.
fn fault(detail: &str) -> Result<Got, DeviceError> {
    Err(DeviceError::new(Role::Console, detail))
}

/// What [`Capture::receive`] or [`Capture::receive_held`] got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Got {
    /// Bytes, kept whole, or none.
    Bytes,
    /// Nothing by the deadline.
    Deadline,
    /// A chunk that the capture has no room for, kept in part or not at all.
    Limit(EndReason),
}

/// The console output, the sends and the events of one cold boot.
pub struct Capture<'a> {
    pub events: Vec<Event>,
    /// The bytes received, exactly as they arrived.
    pub bytes: Vec<u8>,
    /// The `[offset, at_ns]` of each chunk of `bytes`.
    pub index: Vec<[u64; 2]>,
    pub matcher: Matcher<'a>,
    clock: Rc<dyn Clock>,
    /// The most bytes that the capture keeps.
    limit: u64,
    open: bool,
}

impl<'a> Capture<'a> {
    /// The capture of a cold boot that `matcher` reads, of at most `limit`
    /// bytes, stamped by `clock`.
    pub fn new(matcher: Matcher<'a>, limit: u64, clock: Rc<dyn Clock>) -> Self {
        let (events, bytes, index, open) = (Vec::new(), Vec::new(), Vec::new(), false);
        Self {
            events,
            bytes,
            index,
            matcher,
            clock,
            limit,
            open,
        }
    }

    /// The number of bytes received.
    pub fn received(&self) -> u64 {
        self.bytes.len() as u64
    }

    /// Adds `event` now.
    pub fn event(&mut self, event: EventKind) {
        let (now, received) = (self.clock.now(), self.received());
        add(&mut self.events, now, received, event);
    }

    /// Adds the failure of a device now.
    pub fn failed(&mut self, error: &DeviceError) {
        let (now, received) = (self.clock.now(), self.received());
        failed(&mut self.events, now, received, error);
    }

    /// Opens `console`, ready by `deadline`; whether it is ready.
    pub fn open(&mut self, console: &mut dyn Console, deadline: Nanos) -> bool {
        match console.open(deadline) {
            Ok(at) => {
                self.open = true;
                let received = self.received();
                add(&mut self.events, at, received, EventKind::ConsoleReady {});
            }
            Err(error) => self.failed(&error),
        }
        self.open
    }

    /// Receives from `console` by `deadline`. A chunk is kept as far as the
    /// capture has room for it, at most [`MAX_CHUNKS`] chunks, and what is
    /// kept is fed to the matcher. A console that closed, and a chunk
    /// stamped before the one before it, are failures of the console.
    pub fn receive(
        &mut self,
        console: &mut dyn Console,
        deadline: Nanos,
    ) -> Result<Got, DeviceError> {
        match console.receive(deadline)? {
            Received::Bytes { at, bytes } => self.keep(at.0, &bytes),
            Received::Deadline { .. } => Ok(Got::Deadline),
            Received::Closed { .. } => fault("the console closed"),
        }
    }

    /// Receives from `console` a chunk that it holds already, stamped
    /// before `by`, a time that has passed: it asks once, with a deadline
    /// 1 ns before `by`, so that the console does not wait. A chunk is kept
    /// as [`Capture::receive`] keeps one; no bytes, a closed port and a
    /// chunk stamped at `by` or later are [`Got::Deadline`].
    pub fn receive_held(
        &mut self,
        console: &mut dyn Console,
        by: Nanos,
    ) -> Result<Got, DeviceError> {
        match console.receive(Nanos(by.0.saturating_sub(1)))? {
            Received::Bytes { at, bytes } if at < by && !bytes.is_empty() => {
                self.keep(at.0, &bytes)
            }
            _ => Ok(Got::Deadline),
        }
    }

    /// Keeps the chunk `bytes`, stamped `at`, as far as there is room.
    fn keep(&mut self, at: u64, bytes: &[u8]) -> Result<Got, DeviceError> {
        if self.index.last().is_some_and(|&[_, last]| at < last) {
            return fault("the console stamped a chunk before the one before it");
        }
        if self.index.len() >= MAX_CHUNKS && !bytes.is_empty() {
            return Ok(Got::Limit(EndReason::ChunkLimit));
        }
        let room = self.limit.saturating_sub(self.received());
        let kept = &bytes[..bytes.len().min(usize::try_from(room).unwrap_or(usize::MAX))];
        if !kept.is_empty() {
            self.index.push([self.received(), at]);
            self.bytes.extend_from_slice(kept);
            self.matcher.push(at, kept);
        }
        Ok(if kept.len() < bytes.len() {
            Got::Limit(EndReason::ByteLimit)
        } else {
            Got::Bytes
        })
    }

    /// Sends the bytes of `step` to `console`, as an event that the matcher
    /// records.
    pub fn send(
        &mut self,
        console: &mut dyn Console,
        step: usize,
        bytes: &[u8],
    ) -> Result<(), DeviceError> {
        let at = console.send(bytes)?;
        let send = EventKind::Send {
            step,
            bytes_base64: Base64(bytes.to_vec()),
        };
        let received = self.received();
        let event = add(&mut self.events, at, received, send);
        self.matcher.sent(step, event, at.0);
        Ok(())
    }

    /// Closes `console` if this capture opened it.
    pub fn close(&mut self, console: &mut dyn Console) {
        if std::mem::take(&mut self.open) {
            match console.close() {
                Ok(()) => self.event(EventKind::ConsoleClosed {}),
                Err(error) => self.failed(&error),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use kyto_bench_core::board::{Board, RASPBERRY_PI_5};
    use kyto_bench_core::instance::TftpParameters;
    use kyto_bench_core::plan::RunPlan;

    use super::*;
    use crate::device::{PowerState, PowerSwitch};
    use crate::sim::{Responder, Script, SimConsole, SimRelay, SimWorld};

    const MS: u64 = 1_000_000;

    fn context() -> (Board, TftpParameters) {
        let (server_address, path_prefix) = ("192.0.2.10".into(), String::new());
        let tftp = TftpParameters {
            server_address,
            path_prefix,
        };
        (Board::parse(RASPBERRY_PI_5).unwrap(), tftp)
    }

    fn plan(steps: &str) -> RunPlan {
        let text = format!(
            "format = \"kyto-bench run plan\"\nformat_version = 1\nboard = \"raspberry-pi-5\"\n\
             [[entries]]\nname = \"e\"\npayload_dir = \"e\"\n\
             payload = [{{ name = \"a\", sha256 = \"{}\" }}]\ncold_boots = 1\n\
             boot_timeout_ms = 1000\nend = \"timeout\"\nmax_capture_bytes = 9\n{steps}\n",
            "0".repeat(64)
        );
        RunPlan::parse(&text).unwrap()
    }

    #[test]
    fn sends_are_logged_with_the_bytes_received_before_them() {
        let (board, tftp) = context();
        let plan = plan(
            "steps = [{ after = \"go\", send = \"?\", expect = '^ok$', timeout_ms = 5 }]\nmarkers = [{ name = \"go\", line = '^go$' }]",
        );
        let entry = &plan.entries[0];
        let world = SimWorld::new();
        let answer = Responder {
            trigger: b"?".to_vec(),
            delay: Nanos(MS),
            bytes: b"ok\n".to_vec(),
        };
        let chunks = vec![(Nanos(MS), b"go\n".to_vec())];
        let script = Script {
            chunks,
            responders: vec![answer],
        };
        let mut console = SimConsole::new(&world, Box::new(move |_, _| script.clone()));
        let matcher = Matcher::new(&board, entry, &tftp, entry.files(&[]).unwrap());
        let mut capture = Capture::new(matcher, 9, Rc::new(world.clone()));
        assert!(capture.open(&mut console, Nanos(0)));
        SimRelay::new(&world).command(PowerState::On).unwrap();
        assert_eq!(capture.receive(&mut console, Nanos(9 * MS)), Ok(Got::Bytes));
        assert_eq!(capture.matcher.armed(), Some(0));
        assert_eq!(capture.send(&mut console, 0, b"?"), Ok(()));
        assert_eq!(capture.receive(&mut console, Nanos(9 * MS)), Ok(Got::Bytes));
        assert!(capture.matcher.deadline().is_none(), "the response came");
        assert_eq!(capture.bytes, b"go\nok\n");
        assert_eq!(capture.index, [[0, MS], [3, 2 * MS]]);
        let send = &capture.events[1];
        assert_eq!((send.at_ns, send.received), (MS, 3));
        assert_eq!(
            capture.receive(&mut console, Nanos(9 * MS)),
            Ok(Got::Deadline)
        );
        capture.close(&mut console);
        let kinds: Vec<_> = capture
            .events
            .iter()
            .map(|e| (e.at_ns, e.received))
            .collect();
        assert_eq!(kinds, [(0, 0), (MS, 3), (9 * MS, 6)]);
    }

    /// A console that gives what it holds, whatever the deadline, and then
    /// the deadline, and keeps the deadlines that it was given.
    struct Given(Option<Received>, Vec<Nanos>);

    impl Console for Given {
        fn open(&mut self, _: Nanos) -> Result<Nanos, DeviceError> {
            Ok(Nanos(0))
        }

        fn receive(&mut self, deadline: Nanos) -> Result<Received, DeviceError> {
            self.1.push(deadline);
            Ok(self.0.take().unwrap_or(Received::Deadline { at: deadline }))
        }

        fn send(&mut self, _: &[u8]) -> Result<Nanos, DeviceError> {
            Ok(Nanos(0))
        }

        fn close(&mut self) -> Result<(), DeviceError> {
            Ok(())
        }
    }

    #[test]
    fn a_held_chunk_is_taken_only_when_it_was_stamped_before_its_time() {
        let (board, tftp) = context();
        let plan = plan("sends = false");
        let entry = &plan.entries[0];
        let world = SimWorld::new();
        let matcher = Matcher::new(&board, entry, &tftp, entry.files(&[]).unwrap());
        let mut capture = Capture::new(matcher, 9, Rc::new(world.clone()));
        let by = Nanos(5 * MS);
        let chunk = |at, bytes: &[u8]| Received::Bytes {
            at: Nanos(at),
            bytes: bytes.to_vec(),
        };
        // A chunk stamped at the time or later, no bytes and a closed port
        // are the deadline, and nothing is kept.
        for given in [
            Some(chunk(5 * MS, b"ab")),
            Some(chunk(6 * MS, b"ab")),
            Some(chunk(MS, b"")),
            Some(Received::Closed { at: Nanos(MS) }),
            None,
        ] {
            let mut console = Given(given, Vec::new());
            assert_eq!(capture.receive_held(&mut console, by), Ok(Got::Deadline));
            assert_eq!(console.1, [Nanos(5 * MS - 1)], "the console need not wait");
        }
        assert!(capture.bytes.is_empty() && capture.index.is_empty());
        let mut console = Given(Some(chunk(5 * MS - 1, b"ab")), Vec::new());
        assert_eq!(capture.receive_held(&mut console, by), Ok(Got::Bytes));
        assert_eq!(capture.index, [[0, 5 * MS - 1]]);
    }

    #[test]
    fn the_capture_keeps_at_most_its_limit_and_refuses_a_closed_console() {
        let (board, tftp) = context();
        let plan = plan("sends = false");
        let entry = &plan.entries[0];
        let world = SimWorld::new();
        let lines = vec![
            (Nanos(MS), b"12345\n".to_vec()),
            (Nanos(2 * MS), b"6789\n".to_vec()),
        ];
        let script = Script {
            chunks: lines,
            responders: Vec::new(),
        };
        let mut console = SimConsole::new(&world, Box::new(move |_, _| script.clone()));
        let matcher = Matcher::new(&board, entry, &tftp, entry.files(&[]).unwrap());
        let mut capture = Capture::new(matcher, entry.max_capture_bytes, Rc::new(world.clone()));
        assert!(capture.open(&mut console, Nanos(0)));
        SimRelay::new(&world).command(PowerState::On).unwrap();
        assert_eq!(capture.receive(&mut console, Nanos(9 * MS)), Ok(Got::Bytes));
        let limit = Ok(Got::Limit(EndReason::ByteLimit));
        assert_eq!(capture.receive(&mut console, Nanos(9 * MS)), limit);
        assert_eq!(
            (capture.bytes.as_slice(), capture.matcher.received()),
            (&b"12345\n678"[..], 9)
        );
        world.knobs().console.closes_at = Some(Nanos(5 * MS));
        let closed = capture.receive(&mut console, Nanos(9 * MS)).unwrap_err();
        assert_eq!(closed.role, Role::Console);
    }
}
