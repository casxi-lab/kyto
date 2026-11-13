//! The simulated world: time, contacts and supply, and the bench clock.

use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

use crate::device::{Clock, Nanos, PowerState};

use super::console::{ConsoleKnobs, ConsoleState};
use super::meter::MeterKnobs;
use super::relay::RelayKnobs;
use super::tftp::{TftpKnobs, TftpState};

/// The supply voltage while the supply is on, in microvolts.
pub const ON_MICRO: i64 = 5_100_000;

/// A handle to one simulated world; clones share it. It is the clock of the
/// devices made on it: [`Clock::wait_until`] moves the simulated time on.
#[derive(Clone, Default)]
pub struct SimWorld(Rc<RefCell<World>>);

#[derive(Default)]
pub(super) struct World {
    pub(super) now: Nanos,
    pub(super) knobs: Knobs,
    /// Whether the relay's contacts are closed.
    contacts_closed: bool,
    /// The commands the relay sent.
    pub(super) commands: Vec<(Nanos, PowerState)>,
    /// When the supply turned or will turn on or off, in order of time. A
    /// change of the contacts drops the changes that have not happened yet.
    supply: Vec<(Nanos, PowerState)>,
    pub(super) console: ConsoleState,
    pub(super) tftp: TftpState,
}

/// The fault knobs of every device of a world.
#[derive(Clone, Debug, Default)]
pub struct Knobs {
    pub relay: RelayKnobs,
    pub supply: SupplyKnobs,
    pub meter: MeterKnobs,
    pub console: ConsoleKnobs,
    pub tftp: TftpKnobs,
}

/// How the supply follows the relay's contacts.
#[derive(Clone, Debug, Default)]
pub struct SupplyKnobs {
    /// How long the supply takes to come on after the contacts close, as
    /// when a USB-C supply negotiates its power.
    pub rise: Nanos,
    /// How long the supply takes to go off after the contacts open.
    pub fall: Nanos,
    /// The voltage left on the supply while it is off, as when a serial
    /// adapter feeds the board through its UART pins, in microvolts.
    pub residual_micro: i64,
}

impl SimWorld {
    /// A world at time 0 with the contacts open, the supply off, no console
    /// output, nothing staged and every knob off.
    pub fn new() -> Self {
        Self::default()
    }

    /// The knobs, to read or change.
    pub fn knobs(&self) -> RefMut<'_, Knobs> {
        RefMut::map(self.0.borrow_mut(), |world| &mut world.knobs)
    }

    /// Every command the relay sent, with the time it was sent, whether or
    /// not the contacts followed.
    pub fn power_commands(&self) -> Vec<(Nanos, PowerState)> {
        self.0.borrow().commands.clone()
    }

    pub(super) fn borrow(&self) -> Ref<'_, World> {
        self.0.borrow()
    }

    pub(super) fn borrow_mut(&self) -> RefMut<'_, World> {
        self.0.borrow_mut()
    }
}

impl Clock for SimWorld {
    fn now(&self) -> Nanos {
        self.0.borrow().now
    }

    fn wait_until(&self, deadline: Nanos) -> Nanos {
        let mut world = self.0.borrow_mut();
        world.now = world.now.max(deadline);
        world.now
    }
}

impl World {
    /// Whether the supply is on now.
    pub(super) fn supply_on(&self) -> bool {
        self.supply
            .iter()
            .rev()
            .find(|(at, _)| *at <= self.now)
            .is_some_and(|(_, state)| *state == PowerState::On)
    }

    /// Closes or opens the contacts now. The supply follows after its rise
    /// or fall time; the board starts its console output when the supply
    /// comes on and stops it when the supply goes off.
    pub(super) fn set_contacts(&mut self, state: PowerState) {
        let closed = state == PowerState::On;
        if self.contacts_closed == closed {
            return;
        }
        self.contacts_closed = closed;
        let now = self.now;
        let delay = match state {
            PowerState::On => self.knobs.supply.rise,
            PowerState::Off => self.knobs.supply.fall,
        };
        let at = now.saturating_add(delay);
        self.supply.retain(|(change, _)| *change <= now);
        self.supply.push((at, state));
        match state {
            PowerState::On => self.console.power_on(at, self.tftp.served()),
            PowerState::Off => self.console.power_off(at),
        }
    }
}
