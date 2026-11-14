//! The power switch of the kind `lcus-1`: an LCUS-1 USB relay module, whose
//! CH340 takes commands of four bytes at 9600 8N1 and gives no reply. Its
//! one relay switches the supply's VBUS, so a command is only sent; whether
//! the supply followed is read by the confirmation device.

use std::time::Duration;

use kyto_bench_os::clock::SystemClock;
use kyto_bench_os::command_port::{CommandTiming, send_frame};

use super::{Hidden, PowerSettings, device_error};
use crate::device::{DeviceError, Nanos, PowerState, PowerSwitch, Role};

/// The waits of a command: 250 ms on the bench clock after the port is
/// flushed and before the frame is written, and 150 ms after the frame left
/// the port and before the port is closed; the port must take the frame
/// within 2 s. They are constants of the driver, fixed by the commit of the
/// tool, not settings of a bench.
pub const TIMING: CommandTiming = CommandTiming {
    settle: Duration::from_millis(250),
    hold: Duration::from_millis(150),
    write: Duration::from_secs(2),
};

/// The frame that sets the relay `channel` to `state`: `A0`, the channel,
/// 1 for on or 0 for off, and the sum of the three bytes modulo 256.
pub fn frame(channel: u8, state: PowerState) -> [u8; 4] {
    let on = u8::from(state == PowerState::On);
    [
        0xA0,
        channel,
        on,
        0xA0u8.wrapping_add(channel).wrapping_add(on),
    ]
}

/// An LCUS-1 on the port of its settings, which each command opens,
/// writes once and closes again, so that a command still reaches a module
/// whose USB connection was renewed since the last one.
pub struct Lcus1Relay {
    port: String,
    clock: SystemClock,
    hidden: Hidden,
}

impl Lcus1Relay {
    /// The relay of `settings`, which stamps with `clock` and whose errors
    /// name no form of `hidden`. Nothing is opened.
    pub fn new(settings: &PowerSettings, clock: SystemClock, hidden: Hidden) -> Self {
        Self {
            port: settings.port.clone(),
            clock,
            hidden,
        }
    }
}

impl PowerSwitch for Lcus1Relay {
    /// Sends the frame of the relay `1` and returns the time at which it had
    /// left the port. The module's reply is not read, and a command that
    /// fails is not repeated.
    fn command(&mut self, state: PowerState) -> Result<Nanos, DeviceError> {
        let sent = send_frame(&self.port, &frame(1, state), &TIMING, &self.clock);
        sent.map(Nanos)
            .map_err(|error| device_error(Role::Power, &error.to_string(), &self.hidden))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lcus_1_frames_are_the_documented_bytes() {
        assert_eq!(frame(1, PowerState::On), [0xA0, 0x01, 0x01, 0xA2]);
        assert_eq!(frame(1, PowerState::Off), [0xA0, 0x01, 0x00, 0xA1]);
    }
}
