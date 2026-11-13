//! The interfaces through which the tool drives a bench: its clock, power
//! control as a command and a separate confirmation, measurement, the
//! board's serial console, and the delivery of payloads to the directory that
//! the board reads over TFTP when it boots from the network.
//!
//! A run reaches every device through these traits, so simulated and real
//! devices are driven and recorded the same way. No method takes or returns
//! an address, a relay channel, a port path or any other value of a bench's
//! instance configuration; those stay inside the driver of each device.

use std::fmt;

use kyto_bench_core::names::{GenerationName, PayloadName};
use kyto_bench_core::write::WriteTrigger;

pub use kyto_bench_core::roles::{Quantity, Role};

use crate::gate::{self, Checked, WritePermit, WriteRefused};

/// A time of the bench clock: nanoseconds since its origin.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Nanos(pub u64);

impl Nanos {
    /// `ms` milliseconds, or the latest time when that does not fit.
    pub const fn from_millis(ms: u64) -> Self {
        Self(ms.saturating_mul(1_000_000))
    }

    /// This time plus `later`, or the latest time when that does not fit.
    pub const fn saturating_add(self, later: Nanos) -> Self {
        Self(self.0.saturating_add(later.0))
    }
}

/// The one time source of a run. Every time the tool records comes from it,
/// and every wait, timeout and deadline uses it.
pub trait Clock {
    /// The current time.
    fn now(&self) -> Nanos;
    /// Returns at `deadline` or later, with the time it returned.
    fn wait_until(&self, deadline: Nanos) -> Nanos;
}

/// The supply state that a power command asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerState {
    On,
    Off,
}

/// Switches the board's supply. Success means only that the command was
/// sent: a switch never reports its contacts or the supply. Whether the
/// supply followed is read by a [`PowerConfirm`].
pub trait PowerSwitch {
    /// Sends `state`; returns the time the command was sent.
    fn command(&mut self, state: PowerState) -> Result<Nanos, DeviceError>;
}

/// One reading, in micro-units of its quantity: microvolts or microamperes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Measurement {
    pub at: Nanos,
    pub micro: i64,
}

/// An instrument that measures one quantity of the board's supply.
pub trait Meter {
    /// The quantity it measures.
    fn quantity(&self) -> Quantity;
    /// Takes one reading.
    fn sample(&mut self) -> Result<Measurement, DeviceError>;
}

/// The confirmation of the supply state: a reading of the supply taken by a
/// device that is not the switch. The thresholds that turn readings into on
/// or off are parameters of the instance configuration, applied by the
/// tool's rules, never by the device.
pub trait PowerConfirm {
    /// The quantity it reads.
    fn quantity(&self) -> Quantity;
    /// Takes one reading.
    fn read(&mut self) -> Result<Measurement, DeviceError>;
}

/// Any meter of the supply serves as the confirmation device.
#[derive(Debug)]
pub struct MeterConfirm<M: Meter>(pub M);

impl<M: Meter> PowerConfirm for MeterConfirm<M> {
    fn quantity(&self) -> Quantity {
        self.0.quantity()
    }

    fn read(&mut self) -> Result<Measurement, DeviceError> {
        self.0.sample()
    }
}

/// The board's bidirectional serial console. Received bytes come in chunks
/// stamped with the bench clock, exactly as they arrived, without line
/// normalisation or decoding; every send returns the time the port accepted
/// its bytes. The chunks and the sends of one cold boot make its timestamped
/// transcript and its send log.
pub trait Console {
    /// Opens the port exclusively for one cold boot; bytes that arrived
    /// before are discarded. Returns the time from which every byte is kept,
    /// or fails at `deadline` when the port is not ready by then.
    fn open(&mut self, deadline: Nanos) -> Result<Nanos, DeviceError>;
    /// The next input, or [`Received::Deadline`] when nothing arrives by
    /// `deadline`.
    fn receive(&mut self, deadline: Nanos) -> Result<Received, DeviceError>;
    /// Hands every byte to the port; returns the time it accepted them.
    fn send(&mut self, bytes: &[u8]) -> Result<Nanos, DeviceError>;
    /// Closes the port; what it still holds is discarded.
    fn close(&mut self) -> Result<(), DeviceError>;
}

/// What [`Console::receive`] returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Received {
    /// Bytes, with the time they were received.
    Bytes { at: Nanos, bytes: Vec<u8> },
    /// Nothing arrived by the deadline.
    Deadline { at: Nanos },
    /// The port closed.
    Closed { at: Nanos },
}

/// The delivery of payloads to the directory that the board's TFTP server
/// reads. Each payload is staged as a new generation, read back, and then
/// made the served one in one step.
///
/// A device implements [`Delivery::write_generation`], which writes only
/// files that the write gate has checked, and keeps the provided
/// [`Delivery::stage`], which checks them.
pub trait Delivery {
    /// The board's write triggers, which [`Delivery::stage`] checks every
    /// payload file against.
    fn write_triggers(&self) -> &[WriteTrigger];
    /// Stages `files` as a complete new generation that the board is not
    /// served yet and returns its name. Before anything is written, it
    /// checks that every file that lets the firmware write has a permit of
    /// its class ([`crate::gate::check_staging`]); then the device writes
    /// the generation ([`Delivery::write_generation`]).
    fn stage(
        &mut self,
        files: &[(PayloadName, Vec<u8>)],
        permits: &[WritePermit<'_>],
    ) -> Result<GenerationName, StageError> {
        let checked = gate::check_staging(files, self.write_triggers(), permits);
        self.write_generation(checked.map_err(StageError::Refused)?)
            .map_err(StageError::Device)
    }
    /// Writes `files`, which the write gate has checked, as a complete new
    /// generation and returns its name, the next free one; never touches an
    /// existing generation.
    fn write_generation(&mut self, files: Checked<'_>) -> Result<GenerationName, DeviceError>;
    /// A staged file, read back from where the TFTP server reads files.
    fn read_staged(
        &mut self,
        generation: &GenerationName,
        name: &PayloadName,
    ) -> Result<Vec<u8>, DeviceError>;
    /// Makes `generation` the one the board is served, in one step; returns
    /// the one served before.
    fn switch_to(
        &mut self,
        generation: &GenerationName,
    ) -> Result<Option<GenerationName>, DeviceError>;
    /// The bytes the board is served for `name` now, read through the
    /// pointer to the served generation.
    fn read_served(&mut self, name: &PayloadName) -> Result<Vec<u8>, DeviceError>;
}

/// Why [`Delivery::stage`] wrote nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StageError {
    /// The write gate refused a file of the payload.
    Refused(WriteRefused),
    /// The device failed.
    Device(DeviceError),
}

/// A device that failed. The detail names no value of the instance
/// configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceError {
    pub role: Role,
    pub detail: String,
}

impl DeviceError {
    /// A failure of the device of `role`.
    pub fn new(role: Role, detail: impl Into<String>) -> Self {
        Self {
            role,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the {} device failed: {}", self.role, self.detail)
    }
}

impl std::error::Error for DeviceError {}

impl fmt::Display for StageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StageError::Refused(refused) => refused.fmt(f),
            StageError::Device(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for StageError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A meter of the supply current with a fixed reading.
    struct CurrentMeter {
        micro: i64,
        fails: bool,
    }

    impl Meter for CurrentMeter {
        fn quantity(&self) -> Quantity {
            Quantity::SupplyCurrent
        }

        fn sample(&mut self) -> Result<Measurement, DeviceError> {
            if self.fails {
                return Err(DeviceError::new(Role::PowerConfirmation, "no reading"));
            }
            Ok(Measurement {
                at: Nanos(7),
                micro: self.micro,
            })
        }
    }

    #[test]
    fn meter_confirm_serves_any_supply_meter() {
        let mut confirm = MeterConfirm(CurrentMeter {
            micro: 250_000,
            fails: false,
        });
        assert_eq!(confirm.quantity(), Quantity::SupplyCurrent);
        assert_eq!(
            confirm.read(),
            Ok(Measurement {
                at: Nanos(7),
                micro: 250_000
            })
        );
        confirm.0.fails = true;
        let error = confirm.read().unwrap_err();
        assert_eq!(error.role, Role::PowerConfirmation);
        assert_eq!(
            error.to_string(),
            "the power_confirmation device failed: no reading"
        );
    }

    #[test]
    fn bench_times_saturate() {
        assert_eq!(Nanos::from_millis(5), Nanos(5_000_000));
        assert_eq!(Nanos::from_millis(u64::MAX), Nanos(u64::MAX));
        assert_eq!(
            Nanos(u64::MAX - 1).saturating_add(Nanos(2)),
            Nanos(u64::MAX)
        );
    }
}
