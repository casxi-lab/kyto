//! The console of the kind `serial`: the board's debug UART through a USB
//! serial adapter, opened for each cold boot at 115200 8N1, whose input the
//! bench clock stamps when it arrives.

use std::sync::atomic::AtomicI32;

use kyto_bench_os::clock::SystemClock;
use kyto_bench_os::command_port::PortError;
use kyto_bench_os::console::{Got, SerialConsole};

use super::{ConsoleSettings, Hidden, device_error};
use crate::device::{Console, DeviceError, Nanos, Received, Role};

/// A serial console on the port of its settings.
pub struct HostConsole {
    console: SerialConsole<SystemClock>,
    hidden: Hidden,
}

impl HostConsole {
    /// The console of `settings`, which stamps with `clock`, ends a receive
    /// early once `interrupt` holds a signal, and whose errors name no form
    /// of `hidden`. Nothing is opened.
    pub fn new(
        settings: &ConsoleSettings,
        clock: SystemClock,
        interrupt: Option<&'static AtomicI32>,
        hidden: Hidden,
    ) -> Self {
        let console = SerialConsole::new(settings.port.clone(), clock, interrupt);
        Self { console, hidden }
    }

    fn error(&self, error: PortError) -> DeviceError {
        device_error(Role::Console, &error.to_string(), &self.hidden)
    }
}

/// `got` as the device interface gives it.
fn received(got: Got) -> Received {
    match got {
        Got::Bytes { at, bytes } => Received::Bytes {
            at: Nanos(at),
            bytes,
        },
        Got::Deadline { at } => Received::Deadline { at: Nanos(at) },
        Got::Closed { at } => Received::Closed { at: Nanos(at) },
    }
}

impl Console for HostConsole {
    fn open(&mut self, deadline: Nanos) -> Result<Nanos, DeviceError> {
        let opened = self.console.open(deadline.0);
        opened.map(Nanos).map_err(|error| self.error(error))
    }

    fn receive(&mut self, deadline: Nanos) -> Result<Received, DeviceError> {
        let got = self.console.receive(deadline.0);
        got.map(received).map_err(|error| self.error(error))
    }

    fn send(&mut self, bytes: &[u8]) -> Result<Nanos, DeviceError> {
        let sent = self.console.send(bytes);
        sent.map(Nanos).map_err(|error| self.error(error))
    }

    fn close(&mut self) -> Result<(), DeviceError> {
        let closed = self.console.close();
        closed.map_err(|error| self.error(error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_port_gives_keeps_its_bytes_and_times() {
        let bytes = Got::Bytes {
            at: 7,
            bytes: b"\x00\r\n".to_vec(),
        };
        let expected = Received::Bytes {
            at: Nanos(7),
            bytes: b"\x00\r\n".to_vec(),
        };
        assert_eq!(received(bytes), expected);
        let deadline = Received::Deadline { at: Nanos(8) };
        assert_eq!(received(Got::Deadline { at: 8 }), deadline);
        assert_eq!(
            received(Got::Closed { at: 9 }),
            Received::Closed { at: Nanos(9) }
        );
    }
}
