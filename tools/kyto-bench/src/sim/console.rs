//! The simulated serial console and the board output it plays.

use std::collections::{BTreeMap, VecDeque};

use kyto_bench_core::names::PayloadName;

use crate::device::{Console, DeviceError, Nanos, Received, Role};

use super::world::SimWorld;

/// What the board writes to its console after one power-on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Script {
    /// Bytes the board writes, each chunk with its delay after the supply
    /// came on.
    pub chunks: Vec<(Nanos, Vec<u8>)>,
    /// Answers to bytes the board receives.
    pub responders: Vec<Responder>,
}

/// An answer of the board: when the bytes it received since its last answer
/// contain `trigger`, it writes `bytes` `delay` after the send that completed
/// the trigger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Responder {
    pub trigger: Vec<u8>,
    pub delay: Nanos,
    pub bytes: Vec<u8>,
}

/// The files of a generation, by payload name.
pub type Files = BTreeMap<PayloadName, Vec<u8>>;

/// Gives the script of each power-on from its number, counted from 1, and
/// the files of the generation served at that moment (none when no
/// generation is served). It must not call into the world.
pub type ScriptProvider = Box<dyn FnMut(u32, &Files) -> Script>;

/// A provider that replays recorded console output: the `n`-th power-on
/// plays the `n`-th recording, one chunk per line (a line ends after its
/// LF), line `k` (counted from 0) `k + 1` ms after the supply came on. After
/// the last recording the board writes nothing, and it never answers.
pub fn replay(recordings: Vec<Vec<u8>>) -> ScriptProvider {
    Box::new(move |power_on, _| {
        let Some(recording) = power_on
            .checked_sub(1)
            .and_then(|index| recordings.get(index as usize))
        else {
            return Script::default();
        };
        let chunks = recording
            .split_inclusive(|byte| *byte == b'\n')
            .zip(1..)
            .map(|(line, ms)| (Nanos::from_millis(ms), line.to_vec()))
            .collect();
        Script {
            chunks,
            responders: Vec::new(),
        }
    })
}

/// The board's serial console. It plays the script of each power-on by the
/// world's clock, and what the board wrote while the port was closed, or
/// before it was opened, is discarded.
pub struct SimConsole {
    world: SimWorld,
}

/// Faults of the console.
#[derive(Clone, Debug, Default)]
pub struct ConsoleKnobs {
    /// Opening the port fails at once.
    pub open_fails: bool,
    /// How long the port takes to be ready after it is opened.
    pub ready_after: Nanos,
    /// The port reports ready but keeps only bytes that arrive this long
    /// after that.
    pub keeps_after: Nanos,
    /// The port closes at this time, as when its adapter is unplugged.
    pub closes_at: Option<Nanos>,
    /// Sent bytes are accepted but never reach the board.
    pub drops_sends: bool,
}

/// The console's part of the world.
#[derive(Default)]
pub(super) struct ConsoleState {
    provider: Option<ScriptProvider>,
    power_ons: u32,
    /// The bytes the board writes, in order of the time they are due.
    pending: VecDeque<(Nanos, Vec<u8>)>,
    responders: Vec<Responder>,
    /// When the board's last power-on began and, once it has, ended.
    powered: Option<(Nanos, Option<Nanos>)>,
    /// The bytes the board received since its last answer.
    input: Vec<u8>,
    /// While the port is open, the time from which it keeps bytes.
    keeps_from: Option<Nanos>,
}

impl ConsoleState {
    /// The supply came on at `at`: the board starts the next script.
    pub(super) fn power_on(&mut self, at: Nanos, served: Option<&Files>) {
        self.power_ons += 1;
        let empty = Files::new();
        let script = match &mut self.provider {
            Some(provider) => provider(self.power_ons, served.unwrap_or(&empty)),
            None => Script::default(),
        };
        let mut chunks: Vec<_> = script
            .chunks
            .into_iter()
            .map(|(after, bytes)| (at.saturating_add(after), bytes))
            .collect();
        chunks.sort_by_key(|(due, _)| *due);
        self.pending.extend(chunks);
        self.responders = script.responders;
        self.input.clear();
        self.powered = Some((at, None));
    }

    /// The supply goes off at `at`: the board writes nothing from then on.
    pub(super) fn power_off(&mut self, at: Nanos) {
        if let Some((_, end @ None)) = &mut self.powered {
            *end = Some(at);
        }
        self.pending.retain(|(due, _)| *due < at);
    }

    /// The board receives `bytes` at `at` and answers what they trigger.
    fn board_receives(&mut self, at: Nanos, bytes: &[u8]) {
        let on = self
            .powered
            .is_some_and(|(start, end)| start <= at && end.is_none_or(|end| at < end));
        if !on {
            return;
        }
        self.input.extend_from_slice(bytes);
        for responder in &self.responders {
            let Some(end) = find_end(&self.input, &responder.trigger) else {
                continue;
            };
            self.input.drain(..end);
            let due = at.saturating_add(responder.delay);
            let index = self.pending.partition_point(|(other, _)| *other <= due);
            self.pending.insert(index, (due, responder.bytes.clone()));
        }
    }
}

/// The end of the first occurrence of `needle` in `haystack`.
fn find_end(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|start| start + needle.len())
}

fn console_error(detail: &str) -> DeviceError {
    DeviceError::new(Role::Console, detail)
}

impl SimConsole {
    /// The console of `world`, whose board writes what `provider` gives for
    /// each power-on.
    pub fn new(world: &SimWorld, provider: ScriptProvider) -> Self {
        world.borrow_mut().console.provider = Some(provider);
        Self {
            world: world.clone(),
        }
    }
}

impl Console for SimConsole {
    fn open(&mut self, deadline: Nanos) -> Result<Nanos, DeviceError> {
        let mut world = self.world.borrow_mut();
        let knobs = world.knobs.console.clone();
        if knobs.open_fails {
            return Err(console_error("the simulated console could not be opened"));
        }
        if world.console.keeps_from.is_some() {
            return Err(console_error("the simulated console is already open"));
        }
        let ready = world.now.saturating_add(knobs.ready_after);
        if ready > deadline {
            world.now = world.now.max(deadline);
            return Err(console_error(
                "the simulated console was not ready by the deadline",
            ));
        }
        world.now = ready;
        world.console.keeps_from = Some(ready.saturating_add(knobs.keeps_after));
        Ok(ready)
    }

    fn receive(&mut self, deadline: Nanos) -> Result<Received, DeviceError> {
        let mut guard = self.world.borrow_mut();
        let world = &mut *guard;
        let now = world.now;
        let closes_at = world.knobs.console.closes_at;
        let console = &mut world.console;
        let keeps_from = console
            .keeps_from
            .ok_or_else(|| console_error("the simulated console is not open"))?;
        while console
            .pending
            .front()
            .is_some_and(|(due, _)| *due < keeps_from)
        {
            console.pending.pop_front();
        }
        // Bytes that the board wrote before the port closed are delivered
        // first, even when the bench clock has passed both times: the port
        // stamps each chunk when it is read, no earlier than now.
        let due = console.pending.front().map(|(due, _)| *due);
        let chunk_first = match (due, closes_at) {
            (Some(due), Some(closed)) => due < closed,
            (Some(_), None) => true,
            (None, _) => false,
        };
        if chunk_first {
            let at = due.unwrap_or(now).max(now);
            if at <= deadline {
                let bytes = console.pending.pop_front().map(|(_, bytes)| bytes);
                world.now = at;
                return Ok(Received::Bytes {
                    at,
                    bytes: bytes.unwrap_or_default(),
                });
            }
        } else if let Some(closed) = closes_at.map(|closed| closed.max(now))
            && closed <= deadline
        {
            world.now = closed;
            return Ok(Received::Closed { at: closed });
        }
        world.now = now.max(deadline);
        Ok(Received::Deadline { at: world.now })
    }

    fn send(&mut self, bytes: &[u8]) -> Result<Nanos, DeviceError> {
        let mut world = self.world.borrow_mut();
        if world.console.keeps_from.is_none() {
            return Err(console_error("the simulated console is not open"));
        }
        let at = world.now;
        if !world.knobs.console.drops_sends {
            world.console.board_receives(at, bytes);
        }
        Ok(at)
    }

    fn close(&mut self) -> Result<(), DeviceError> {
        self.world.borrow_mut().console.keeps_from = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Clock, Delivery, PowerState, PowerSwitch};
    use crate::sim::{SimRelay, SimTftp};

    fn ms(count: u64) -> Nanos {
        Nanos::from_millis(count)
    }

    fn bytes(at: Nanos, text: &[u8]) -> Received {
        Received::Bytes {
            at,
            bytes: text.to_vec(),
        }
    }

    fn script(chunks: &[(u64, &[u8])], responders: Vec<Responder>) -> Script {
        Script {
            chunks: chunks
                .iter()
                .map(|(after, text)| (ms(*after), text.to_vec()))
                .collect(),
            responders,
        }
    }

    fn bench(provider: ScriptProvider) -> (SimWorld, SimRelay, SimConsole) {
        let world = SimWorld::new();
        let relay = SimRelay::new(&world);
        let console = SimConsole::new(&world, provider);
        (world, relay, console)
    }

    #[test]
    fn sim_console_plays_its_script_by_the_bench_clock() {
        let (world, mut relay, mut console) = bench(Box::new(|power_on, _| {
            let first: &[(u64, &[u8])] = &[(20, b"boot\r\n"), (10, b"hello\n"), (500, b"late\n")];
            match power_on {
                1 => script(first, Vec::new()),
                _ => script(&[(5, b"again\n")], Vec::new()),
            }
        }));
        world.wait_until(ms(100));
        assert_eq!(console.open(ms(102)), Ok(ms(100)));
        // Nothing before power-on: receiving waits out the deadline.
        assert_eq!(
            console.receive(ms(150)),
            Ok(Received::Deadline { at: ms(150) })
        );
        assert_eq!(world.now(), ms(150));
        relay.command(PowerState::On).unwrap();
        // Chunks come in order of their time after power-on, each at its
        // time on the bench clock, which the console moves on.
        assert_eq!(console.receive(ms(1000)), Ok(bytes(ms(160), b"hello\n")));
        assert_eq!(world.now(), ms(160));
        world.wait_until(ms(175));
        assert_eq!(console.receive(ms(1000)), Ok(bytes(ms(175), b"boot\r\n")));
        assert_eq!(
            console.receive(ms(200)),
            Ok(Received::Deadline { at: ms(200) })
        );
        // Power-off discards what the board has not written yet.
        relay.command(PowerState::Off).unwrap();
        assert_eq!(
            console.receive(ms(2000)),
            Ok(Received::Deadline { at: ms(2000) })
        );
        console.close().unwrap();
        // The next power-on plays its own script.
        assert_eq!(console.open(ms(2000)), Ok(ms(2000)));
        relay.command(PowerState::On).unwrap();
        assert_eq!(console.receive(ms(3000)), Ok(bytes(ms(2005), b"again\n")));
        assert_eq!(
            console.receive(Nanos(ms(2005).0 + 1)),
            Ok(Received::Deadline {
                at: Nanos(ms(2005).0 + 1)
            })
        );
    }

    #[test]
    fn console_ready_times_out_on_the_bench_clock() {
        let (world, _, mut console) = bench(replay(Vec::new()));
        world.knobs().console.ready_after = ms(300);
        assert!(console.open(ms(200)).is_err());
        assert_eq!(world.now(), ms(200));
        assert_eq!(console.open(ms(500)), Ok(ms(500)));
        assert_eq!(world.now(), ms(500));
        assert!(console.open(ms(900)).is_err(), "already open");
    }

    #[test]
    fn the_board_answers_sends_while_it_is_on() {
        let answer = Responder {
            trigger: b"?\r".to_vec(),
            delay: ms(2),
            bytes: b"STATS seq=1\r\n".to_vec(),
        };
        let (world, mut relay, mut console) = bench(Box::new(move |_, _| {
            script(&[(1, b"READY\r\n"), (50, b"done\n")], vec![answer.clone()])
        }));
        console.open(Nanos(0)).unwrap();
        // The board is off, so nothing answers.
        assert_eq!(console.send(b"?\r"), Ok(Nanos(0)));
        relay.command(PowerState::On).unwrap();
        assert_eq!(console.receive(ms(10)), Ok(bytes(ms(1), b"READY\r\n")));
        // A trigger split over two sends still counts.
        assert_eq!(console.send(b"?"), Ok(ms(1)));
        world.wait_until(ms(3));
        assert_eq!(console.send(b"\r"), Ok(ms(3)));
        assert_eq!(
            console.receive(ms(100)),
            Ok(bytes(ms(5), b"STATS seq=1\r\n"))
        );
        assert_eq!(console.receive(ms(100)), Ok(bytes(ms(50), b"done\n")));
        // Dropped sends never reach the board.
        world.knobs().console.drops_sends = true;
        assert_eq!(console.send(b"?\r"), Ok(ms(50)));
        assert_eq!(
            console.receive(ms(100)),
            Ok(Received::Deadline { at: ms(100) })
        );
    }

    #[test]
    fn console_knobs_fail_keep_later_bytes_and_close() {
        let lines: &[(u64, &[u8])] = &[(1, b"one\n"), (2, b"two\n"), (3, b"three\n")];
        let (world, mut relay, mut console) =
            bench(Box::new(move |_, _| script(lines, Vec::new())));
        world.knobs().console.open_fails = true;
        assert_eq!(console.open(ms(10)).unwrap_err().role, Role::Console);
        assert!(console.receive(ms(10)).is_err(), "not open");
        assert!(console.send(b"x").is_err(), "not open");
        // Bytes written before the port was open are discarded.
        relay.command(PowerState::On).unwrap();
        world.wait_until(ms(1));
        {
            let mut knobs = world.knobs();
            knobs.console.open_fails = false;
            knobs.console.keeps_after = ms(1);
            knobs.console.closes_at = Some(Nanos(ms(3).0 + 1));
        }
        // Ready at 1 ms, but the port keeps only bytes from 2 ms.
        assert_eq!(console.open(ms(10)), Ok(ms(1)));
        assert_eq!(console.receive(ms(10)), Ok(bytes(ms(2), b"two\n")));
        assert_eq!(console.receive(ms(10)), Ok(bytes(ms(3), b"three\n")));
        let closed = Received::Closed {
            at: Nanos(ms(3).0 + 1),
        };
        assert_eq!(console.receive(ms(10)), Ok(closed.clone()));
        assert_eq!(console.receive(ms(10)), Ok(closed));
    }

    #[test]
    fn bytes_written_before_the_close_are_delivered_after_the_clock_passed_it() {
        let lines: &[(u64, &[u8])] = &[(5, b"early\n"), (15, b"late\n")];
        let (world, mut relay, mut console) =
            bench(Box::new(move |_, _| script(lines, Vec::new())));
        world.knobs().console.closes_at = Some(ms(10));
        console.open(Nanos(0)).unwrap();
        relay.command(PowerState::On).unwrap();
        // The bench clock passes both the chunk and the close before the
        // flow reads the console, as while it waits for a confirmation.
        world.wait_until(ms(20));
        assert_eq!(console.receive(ms(30)), Ok(bytes(ms(20), b"early\n")));
        assert_eq!(console.receive(ms(30)), Ok(Received::Closed { at: ms(20) }));
        assert_eq!(console.receive(ms(30)), Ok(Received::Closed { at: ms(20) }));
        // A chunk due at the same time as the close is not delivered.
        let (world, mut relay, mut console) =
            bench(Box::new(move |_, _| script(&[(10, b"same\n")], Vec::new())));
        world.knobs().console.closes_at = Some(ms(10));
        console.open(Nanos(0)).unwrap();
        relay.command(PowerState::On).unwrap();
        assert_eq!(console.receive(ms(30)), Ok(Received::Closed { at: ms(10) }));
    }

    #[test]
    fn replay_plays_one_recording_per_power_on_one_line_per_millisecond() {
        let recordings = vec![b"a\r\nb\n\xffc".to_vec(), b"d\n".to_vec()];
        let mut provider = replay(recordings);
        let files = Files::new();
        assert_eq!(
            provider(1, &files),
            script(&[(1, b"a\r\n"), (2, b"b\n"), (3, b"\xffc")], Vec::new())
        );
        assert_eq!(provider(2, &files), script(&[(1, b"d\n")], Vec::new()));
        assert_eq!(provider(3, &files), Script::default());
        assert_eq!(provider(0, &files), Script::default());
    }

    #[test]
    fn the_script_sees_the_served_files() {
        let (world, mut relay, mut console) = bench(Box::new(|power_on, files| {
            let names: Vec<&str> = files.keys().map(PayloadName::as_str).collect();
            let text = format!("{power_on}:{}\n", names.join(","));
            Script {
                chunks: vec![(Nanos(0), text.into_bytes())],
                responders: Vec::new(),
            }
        }));
        console.open(Nanos(0)).unwrap();
        relay.command(PowerState::On).unwrap();
        assert_eq!(console.receive(ms(1)), Ok(bytes(Nanos(0), b"1:\n")));
        relay.command(PowerState::Off).unwrap();
        let mut tftp = SimTftp::new(&world, Vec::new());
        let file = (PayloadName::parse("config.txt").unwrap(), b"x".to_vec());
        let generation = tftp.stage(&[file], &[]).unwrap();
        tftp.switch_to(&generation).unwrap();
        relay.command(PowerState::On).unwrap();
        assert_eq!(
            console.receive(ms(1)),
            Ok(bytes(Nanos(0), b"2:config.txt\n"))
        );
    }
}
