//! The simulated bridge and meter: an MCP2221A with an INA228 at I2C
//! address 0x40 behind it, on a simulated world, for the tests of the real
//! meter's driver. The bridge answers each report after 1 ms of simulated
//! time; the INA228 converts the shunt and the bus voltage in turns of
//! 1052 µs from the time it is configured, and reads the supply that the
//! relay's contacts feed, [`ON_MICRO`] and [`ON_CURRENT_MICRO`] while it is
//! on and the residual voltage of the world while it is off, across a
//! 15 mΩ shunt. After a reset its results read 0 until it is configured
//! again, the worst case of a reset. A cancel that stops a transfer holds
//! SDA low for 1 ms, until the bridge releases the bus.

use std::cell::{Ref, RefCell, RefMut};
use std::collections::VecDeque;
use std::rc::Rc;

use super::world::{ON_MICRO, SimWorld, World};
use crate::device::{Clock, Nanos};
use crate::drivers::hid::{HidLink, HidOpen, LinkError, OpenError, Report};
use crate::drivers::ina228::CONVERSIONS;

/// The supply current while the supply is on, in microamperes.
pub const ON_CURRENT_MICRO: i64 = 600_000;
/// How long the bridge takes to answer a report.
const LATENCY: Nanos = Nanos::from_millis(1);
/// The time of one conversion, in nanoseconds.
const CONVERSION: u64 = 1_052_000;
/// The register value of `ADC_CONFIG` after a reset.
const ADC_RESET: u16 = 0xFB68;
/// How long the bridge takes to release the bus after a cancel.
const RELEASE: Nanos = Nanos::from_millis(1);

/// A bridge with its INA228, on a world; clones share it. It is its own
/// opener and the link that opening gives.
#[derive(Clone)]
pub struct SimBridge {
    world: SimWorld,
    state: Rc<RefCell<State>>,
}

/// Faults of the bridge and the INA228.
#[derive(Clone, Debug, Default)]
pub struct BridgeKnobs {
    /// What opening finds in place of one bridge that it can open.
    pub open: Option<OpenError>,
    /// Every send and receive on a link fails with this.
    pub link: Option<LinkError>,
    /// How late the bridge answers, in place of 1 ms.
    pub late: Option<Nanos>,
    /// Answers echo this command code.
    pub echo: Option<u8>,
    /// The I2C engine is busy for this many more I2C commands, which it
    /// refuses; a cancel meanwhile stops a transfer.
    pub busy: u32,
    /// SCL reads low.
    pub scl_low: bool,
    /// SDA reads low, as when a device holds it after an aborted transfer.
    pub sda_low: bool,
    /// The bridge does not set the I2C speed.
    pub no_speed: bool,
    /// No device answers at the INA228's address.
    pub absent: bool,
    /// The manufacturer and device IDs read these.
    pub ids: Option<(u16, u16)>,
    /// The INA228 keeps no register that is written.
    pub ignores_writes: bool,
    /// The INA228 is reset at this time.
    pub reset_at: Option<Nanos>,
    /// The next this many fetches give the data of the transfer before.
    pub stale: u32,
    /// The reserved bits of the results read 1.
    pub malformed: bool,
    /// The shunt is wired the wrong way round.
    pub reversed: bool,
}

#[derive(Default)]
struct State {
    knobs: BridgeKnobs,
    opens: u32,
    answers: VecDeque<(Nanos, Report)>,
    pointer: u8,
    config: u16,
    adc: u16,
    /// When the conversions began, unless the INA228 was reset since.
    since: Option<Nanos>,
    /// The data of the last read and of the one before; `None` when no
    /// device answered.
    data: Option<Vec<u8>>,
    before: Option<Vec<u8>>,
    /// The start and end of the conversion behind the last result read.
    conversion: Option<(Nanos, Nanos)>,
    /// When the bus was released after the last transfer that was stopped.
    released: Nanos,
}

impl SimBridge {
    /// The bridge and INA228 of `world`, which no one has configured.
    pub fn new(world: &SimWorld) -> Self {
        let mut state = State::default();
        state.reset();
        let (world, state) = (world.clone(), Rc::new(RefCell::new(state)));
        Self { world, state }
    }

    /// The knobs, to read or change.
    pub fn knobs(&self) -> RefMut<'_, BridgeKnobs> {
        RefMut::map(self.state.borrow_mut(), |state| &mut state.knobs)
    }

    /// How many times the bridge was opened or tried.
    pub fn opens(&self) -> u32 {
        self.state.borrow().opens
    }

    /// `CONFIG` and `ADC_CONFIG`.
    pub fn configuration(&self) -> (u16, u16) {
        let state = self.state.borrow();
        (state.config, state.adc)
    }

    /// The start and end of the conversion behind the last result read.
    pub fn conversion(&self) -> Option<(Nanos, Nanos)> {
        self.state.borrow().conversion
    }
}

impl State {
    fn reset(&mut self) {
        (self.pointer, self.config, self.adc, self.since) = (0, 0, ADC_RESET, None);
    }

    /// The answer to `command` at the time of `world`, if any.
    fn answer(&mut self, world: &Ref<'_, World>, command: &Report) -> Option<Report> {
        let now = world.now;
        if self.knobs.reset_at.is_some_and(|at| at <= now) {
            self.knobs.reset_at = None;
            self.reset();
        }
        let knobs = self.knobs.clone();
        let mut answer = [0; 64];
        answer[0] = knobs.echo.unwrap_or(command[0]);
        let (len, data) = (usize::from(command[1]), &command[4..]);
        let i2c = matches!(command[0], 0x90 | 0x93 | 0x94);
        if i2c && knobs.busy > 0 {
            (self.knobs.busy, answer[1]) = (knobs.busy - 1, 1);
            return Some(answer);
        }
        match command[0] {
            0x10 => {
                if command[2] == 0x10 && knobs.busy > 0 {
                    (answer[2], self.released) = (0x10, now.saturating_add(RELEASE));
                } else if command[2] == 0x10 {
                    answer[2] = 0x11;
                }
                if command[3] == 0x20 {
                    answer[3] = if knobs.no_speed { 0x21 } else { 0x20 };
                    answer[4] = if knobs.no_speed { 0 } else { command[4] };
                }
                answer[22] = u8::from(!knobs.scl_low);
                answer[23] = u8::from(!knobs.sda_low && now >= self.released);
            }
            0x90 if !knobs.absent && !knobs.ignores_writes && command[3] == 0x80 => {
                let value = u16::from_be_bytes([data[1], data[2]]);
                match data[0] {
                    0x00 if value & 0x8000 != 0 => self.reset(),
                    0x00 => self.config = value,
                    0x01 => (self.adc, self.since) = (value, Some(now)),
                    _ => {}
                }
            }
            0x94 if !knobs.absent && command[3] == 0x80 => self.pointer = data[0],
            0x93 => {
                let read = (!knobs.absent && command[3] == 0x81).then(|| self.read(world, len));
                self.before = std::mem::replace(&mut self.data, read);
            }
            0x40 => {
                // The data of the transfer before while `stale` lasts.
                let fetched = [&self.data, &self.before][usize::from(knobs.stale > 0)];
                self.knobs.stale = knobs.stale.saturating_sub(1);
                match fetched {
                    Some(bytes) => {
                        answer[3] = bytes.len() as u8;
                        answer[4..4 + bytes.len()].copy_from_slice(bytes);
                    }
                    None => (answer[1], answer[3]) = (0x41, 127),
                }
            }
            0x90 | 0x94 => {}
            _ => return None,
        }
        Some(answer)
    }

    /// `len` bytes of the register at the pointer.
    fn read(&mut self, world: &Ref<'_, World>, len: usize) -> Vec<u8> {
        let (manufacturer, device) = self.knobs.ids.unwrap_or((0x5449, 0x2281));
        let mut bytes = match self.pointer {
            0x00 => self.config.to_be_bytes().to_vec(),
            0x01 => self.adc.to_be_bytes().to_vec(),
            0x04 | 0x05 => self.result(world),
            0x3E => manufacturer.to_be_bytes().to_vec(),
            0x3F => device.to_be_bytes().to_vec(),
            _ => vec![0, 0],
        };
        bytes.resize(len, 0);
        bytes
    }

    /// The last result of the conversion at the pointer: the shunt voltage
    /// at offset 0 of each turn, the bus voltage after it.
    fn result(&mut self, world: &Ref<'_, World>) -> Vec<u8> {
        let bus = self.pointer == 0x05;
        let first = (self.since.filter(|_| self.adc == CONVERSIONS))
            .map(|since| since.0 + if bus { CONVERSION } else { 0 });
        let now = world.now.0;
        self.conversion = first
            .filter(|first| first + CONVERSION <= now)
            .map(|first| {
                let start = first + (now - first - CONVERSION) / (2 * CONVERSION) * 2 * CONVERSION;
                (Nanos(start), Nanos(start + CONVERSION))
            });
        let (on, sign) = (world.supply_on(), if self.knobs.reversed { -1 } else { 1 });
        let code = match (self.conversion, bus) {
            (None, _) => 0,
            (_, true) if on => ON_MICRO * 16 / 3125,
            (_, true) => world.knobs.supply.residual_micro * 16 / 3125,
            (_, false) if on => sign * ON_CURRENT_MICRO * 15_000 / 312_500,
            (_, false) => 0,
        };
        let raw = (((code as u32) << 4) & 0xFF_FFFF) | u32::from(self.knobs.malformed);
        raw.to_be_bytes()[1..].to_vec()
    }
}

impl HidOpen for SimBridge {
    type Link = SimBridge;

    fn open(&mut self) -> Result<SimBridge, OpenError> {
        let mut state = self.state.borrow_mut();
        state.opens += 1;
        if let Some(error) = state.knobs.open {
            return Err(error);
        }
        state.answers.clear();
        Ok(self.clone())
    }
}

impl HidLink for SimBridge {
    fn send(&mut self, report: &Report) -> Result<(), LinkError> {
        let mut state = self.state.borrow_mut();
        if let Some(error) = state.knobs.link {
            return Err(error);
        }
        let world = self.world.borrow();
        let late = state.knobs.late.unwrap_or(LATENCY);
        let due = world.now.saturating_add(late);
        if let Some(answer) = state.answer(&world, report) {
            state.answers.push_back((due, answer));
        }
        Ok(())
    }

    fn receive(&mut self) -> Result<Option<Report>, LinkError> {
        let mut state = self.state.borrow_mut();
        if let Some(error) = state.knobs.link {
            return Err(error);
        }
        let now = self.world.now();
        let arrived = state.answers.pop_front_if(|(due, _)| *due <= now);
        Ok(arrived.map(|(_, report)| report))
    }
}
