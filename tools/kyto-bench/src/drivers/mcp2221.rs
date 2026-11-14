//! The reports of the MCP2221 and MCP2221A USB bridges that the meter uses,
//! from section 3.1 of their data sheets, DS20005292C and DS20005565E, which
//! give both chips the same commands, and the exchange of one report for
//! its answer on the bench clock. The meter uses the status command, the
//! I2C speed, I2C writes with and without a stop, reads with a repeated
//! start and the fetch of the data read; never the flash, GPIO or SRAM
//! commands, so the bridge's own settings are never changed. An answer is
//! taken only by the fields that both data sheets document alike.

use super::hid::{HidLink, Report};
use super::ina228::Fault;
use crate::device::{Clock, Nanos};

/// The command codes.
pub const STATUS: u8 = 0x10;
pub const GET_DATA: u8 = 0x40;
pub const WRITE: u8 = 0x90;
pub const READ: u8 = 0x93;
pub const WRITE_NO_STOP: u8 = 0x94;

/// The I2C clock divider of 100 kHz: 12 MHz / 100 kHz − 2.
pub const DIVIDER: u8 = 118;
/// How long the bridge has to answer a report.
pub const ANSWER: Nanos = Nanos::from_millis(200);
/// How often the link is asked for an answer, the wait before each fetch
/// of the data of a read and before a command is sent again.
const POLL: Nanos = Nanos::from_millis(1);
/// How many times the data of a read are fetched, and an I2C command that
/// the busy I2C engine did not take is sent, before it fails.
const TRIES: usize = 3;
/// The status of an I2C command that the busy I2C engine did not take.
const BUSY: u8 = 0x01;
/// The most reports that the bridge can have left from earlier exchanges.
const LEFT: usize = 64;

/// The report of `command` with `fields` from index 1 on, the rest zero.
fn report(command: u8, fields: &[u8]) -> Report {
    let mut report = [0; 64];
    report[0] = command;
    report[1..=fields.len()].copy_from_slice(fields);
    report
}

/// The status command, which also cancels the transfer in progress and
/// frees the bus when `cancel` is set.
pub fn status(cancel: bool) -> Report {
    report(STATUS, &[0, if cancel { 0x10 } else { 0 }])
}

/// The status command that sets the I2C clock to 100 kHz.
pub fn speed() -> Report {
    report(STATUS, &[0, 0, 0x20, DIVIDER])
}

/// An I2C write of `data`, at most 60 bytes, to the 7-bit `address`, with a
/// stop for [`WRITE`] and without one for [`WRITE_NO_STOP`].
pub fn write(command: u8, address: u8, data: &[u8]) -> Report {
    let mut write = report(command, &[data.len() as u8, 0, address << 1]);
    write[4..4 + data.len()].copy_from_slice(data);
    write
}

/// An I2C read of `len` bytes from the 7-bit `address`, after a repeated
/// start.
pub fn read(address: u8, len: u8) -> Report {
    report(READ, &[len, 0, (address << 1) | 1])
}

/// The fetch of the data that the last read gave.
pub fn get_data() -> Report {
    report(GET_DATA, &[])
}

/// The bridge on `link`, whose waits and timeouts run on `clock`.
pub struct Bridge<'a, L: HidLink> {
    pub link: &'a mut L,
    pub clock: &'a dyn Clock,
}

impl<L: HidLink> Bridge<'_, L> {
    /// Sends `command`, once the reports left from earlier exchanges are
    /// taken, and returns the answer that echoes it, with the time it
    /// arrived.
    pub fn exchange(&mut self, command: &Report) -> Result<(Report, Nanos), Fault> {
        for _ in 0..LEFT {
            if self.link.receive().map_err(Fault::Link)?.is_none() {
                break;
            }
        }
        self.link.send(command).map_err(Fault::Link)?;
        let deadline = self.clock.now().saturating_add(ANSWER);
        loop {
            if let Some(answer) = self.link.receive().map_err(Fault::Link)? {
                let (got, sent) = (answer[0], command[0]);
                if got != sent {
                    return Err(Fault::Answered { got, sent });
                }
                return Ok((answer, self.clock.now()));
            }
            let now = self.clock.now();
            if now >= deadline {
                return Err(Fault::NoAnswer);
            }
            self.clock
                .wait_until(now.saturating_add(POLL).min(deadline));
        }
    }

    /// Waits `time` on the bench clock.
    fn pause(&self, time: Nanos) {
        let now = self.clock.now();
        self.clock.wait_until(now.saturating_add(time));
    }

    /// [`Bridge::exchange`], whose answer must report success. A command
    /// that the busy I2C engine did not take is sent again 1 ms later, at
    /// most three times in all; any other status refuses it.
    fn accepted(&mut self, command: &Report) -> Result<Report, Fault> {
        let mut sent = 1;
        loop {
            let (answer, _) = self.exchange(command)?;
            match answer[1] {
                0 => return Ok(answer),
                BUSY if sent < TRIES => sent += 1,
                status => {
                    let command = command[0];
                    return Err(Fault::Refused { command, status });
                }
            }
            self.pause(POLL);
        }
    }

    /// Cancels the transfer in progress and frees the bus; the status that
    /// follows, read again 2 ms later when a transfer was cancelled, as the
    /// bus is released some hundreds of microseconds after the answer.
    pub fn cancel(&mut self) -> Result<Report, Fault> {
        let answer = self.accepted(&status(true))?;
        if answer[2] != 0x10 {
            return Ok(answer);
        }
        self.pause(Nanos::from_millis(2));
        self.accepted(&status(false))
    }

    /// Frees the bus, which must then read high on both lines, and sets the
    /// I2C clock to 100 kHz.
    pub fn set_up(&mut self) -> Result<(), Fault> {
        let status = self.cancel()?;
        let (scl, sda) = (status[22], status[23]);
        if scl == 0 || sda == 0 {
            return Err(Fault::BusLow { scl, sda });
        }
        let answer = self.accepted(&speed())?;
        if (answer[3], answer[4]) != (0x20, DIVIDER) {
            return Err(Fault::Speed { status: answer[3] });
        }
        Ok(())
    }

    /// Writes `value` to the 16-bit `register` of the device at `address`.
    pub fn write_register(&mut self, address: u8, register: u8, value: u16) -> Result<(), Fault> {
        let [high, low] = value.to_be_bytes();
        let command = write(WRITE, address, &[register, high, low]);
        self.accepted(&command).map(drop)
    }

    /// The `N` bytes of `register` of the device at `address`, with the time
    /// they arrived. The register pointer is written for every read, never
    /// assumed, and data are taken only with the status of success and
    /// exactly `N` bytes, fetched at most three times 1 ms apart.
    pub fn read_register<const N: usize>(
        &mut self,
        address: u8,
        register: u8,
    ) -> Result<([u8; N], Nanos), Fault> {
        self.accepted(&write(WRITE_NO_STOP, address, &[register]))?;
        self.accepted(&read(address, N as u8))?;
        for _ in 0..TRIES {
            self.pause(POLL);
            let (answer, at) = self.exchange(&get_data())?;
            if (answer[1], usize::from(answer[3])) == (0, N) {
                let mut data = [0; N];
                data.copy_from_slice(&answer[4..4 + N]);
                return Ok((data, at));
            }
        }
        Err(Fault::NoDevice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::hid::LinkError;
    use crate::sim::SimWorld;

    /// A link that records what it is sent and answers each report with the
    /// next of `answers`, if any.
    #[derive(Default)]
    struct Scripted {
        sent: Vec<Report>,
        answers: Vec<Option<Report>>,
        due: Option<Report>,
    }

    impl HidLink for Scripted {
        fn send(&mut self, report: &Report) -> Result<(), LinkError> {
            self.sent.push(*report);
            self.due = self.answers.remove(0);
            Ok(())
        }

        fn receive(&mut self) -> Result<Option<Report>, LinkError> {
            Ok(self.due.take())
        }
    }

    fn answer(fields: &[u8]) -> Option<Report> {
        Some(report(fields[0], &fields[1..]))
    }

    #[test]
    fn the_bridge_reports_have_their_documented_bytes() {
        let bytes = |report: Report, len: usize| report[..len].to_vec();
        assert_eq!(bytes(status(true), 4), [0x10, 0, 0x10, 0]);
        assert_eq!(bytes(status(false), 4), [0x10, 0, 0, 0]);
        assert_eq!(bytes(speed(), 6), [0x10, 0, 0, 0x20, 118, 0]);
        let config = write(WRITE, 0x40, &[0x01, 0xBB, 0x68]);
        assert_eq!(bytes(config, 8), [0x90, 3, 0, 0x80, 0x01, 0xBB, 0x68, 0]);
        let pointer = write(WRITE_NO_STOP, 0x40, &[0x05]);
        assert_eq!(bytes(pointer, 6), [0x94, 1, 0, 0x80, 0x05, 0]);
        assert_eq!(bytes(read(0x40, 3), 5), [0x93, 3, 0, 0x81, 0]);
        assert_eq!(bytes(get_data(), 2), [0x40, 0]);
        for report in [status(true), speed(), config, read(0x40, 3), get_data()] {
            assert!(report[8..].iter().all(|byte| *byte == 0));
        }
    }

    #[test]
    fn answers_are_taken_only_by_their_echo_status_and_length() {
        let world = SimWorld::new();
        let read_answers = |data: Option<Report>| {
            let accepted = [answer(&[0x94, 0]), answer(&[0x93, 0])];
            accepted.into_iter().chain([data; 3]).collect::<Vec<_>>()
        };
        let cases = [
            (
                read_answers(answer(&[0x40, 0, 0, 2, 0xBB, 0x68])),
                Ok([0xBB, 0x68]),
            ),
            (
                vec![answer(&[0x93, 0])],
                Err(Fault::Answered {
                    got: 0x93,
                    sent: 0x94,
                }),
            ),
            // Data of another length, as a fetch that the last transfer
            // left, and the status of a failed read.
            (
                read_answers(answer(&[0x40, 0, 0, 3, 0xBB, 0x68, 0])),
                Err(Fault::NoDevice),
            ),
            (
                read_answers(answer(&[0x40, 0x41, 0, 127])),
                Err(Fault::NoDevice),
            ),
            (vec![None], Err(Fault::NoAnswer)),
        ];
        for (answers, expected) in cases {
            let mut link = Scripted {
                answers,
                ..Scripted::default()
            };
            let mut bridge = Bridge {
                link: &mut link,
                clock: &world,
            };
            let read = bridge.read_register::<2>(0x40, 0x01);
            let commands: Vec<u8> = link.sent.iter().map(|report| report[0]).collect();
            if expected.is_ok() {
                // The pointer, the read and one fetch.
                assert_eq!(commands, [WRITE_NO_STOP, READ, GET_DATA]);
            }
            assert_eq!(read.map(|(data, _)| data), expected);
        }
        // A command that the busy I2C engine did not take is sent again
        // 1 ms later, three times in all; another status refuses it at once.
        let data = || read_answers(answer(&[0x40, 0, 0, 2, 0xBB, 0x68]));
        let refused = |status| {
            Err(Fault::Refused {
                command: 0x94,
                status,
            })
        };
        for (answers, expected, sent, waited) in [
            (
                [vec![answer(&[0x94, 1]); 2], data()].concat(),
                Ok([0xBB, 0x68]),
                5,
                3,
            ),
            (vec![answer(&[0x94, 1]); 3], refused(1), 3, 2),
            (vec![answer(&[0x94, 2])], refused(2), 1, 0),
        ] {
            let start = world.now();
            let mut link = Scripted {
                answers,
                ..Scripted::default()
            };
            let mut bridge = Bridge {
                link: &mut link,
                clock: &world,
            };
            let read = bridge.read_register::<2>(0x40, 0x01);
            assert_eq!(read.map(|(data, _)| data), expected);
            assert_eq!(link.sent.len(), sent);
            let waited = start.saturating_add(Nanos::from_millis(waited));
            assert_eq!(world.now(), waited);
        }
        // No answer fails after 200 ms of the bench clock, polled every 1 ms.
        let start = world.now();
        let mut link = Scripted {
            answers: vec![None],
            ..Scripted::default()
        };
        let mut bridge = Bridge {
            link: &mut link,
            clock: &world,
        };
        assert_eq!(bridge.exchange(&status(false)), Err(Fault::NoAnswer));
        assert_eq!(world.now(), start.saturating_add(ANSWER));
    }
}
