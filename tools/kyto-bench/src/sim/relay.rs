//! The simulated relay: the power command.

use crate::device::{DeviceError, Nanos, PowerState, PowerSwitch, Role};

use super::world::SimWorld;

/// The relay that switches the board's supply. Its contacts follow each
/// command unless a knob holds them, and a command reports only the time it
/// was sent, as a relay that sends a frame and reads no reply.
pub struct SimRelay {
    world: SimWorld,
}

/// Faults of the relay.
#[derive(Clone, Debug, Default)]
pub struct RelayKnobs {
    /// The contacts ignore every command, as if stuck.
    pub stuck: bool,
    /// Every command fails before it is sent.
    pub fails: bool,
}

impl SimRelay {
    /// The relay of `world`.
    pub fn new(world: &SimWorld) -> Self {
        Self {
            world: world.clone(),
        }
    }
}

impl PowerSwitch for SimRelay {
    fn command(&mut self, state: PowerState) -> Result<Nanos, DeviceError> {
        let mut world = self.world.borrow_mut();
        if world.knobs.relay.fails {
            return Err(DeviceError::new(
                Role::Power,
                "the simulated relay could not send the command",
            ));
        }
        let at = world.now;
        world.commands.push((at, state));
        if !world.knobs.relay.stuck {
            world.set_contacts(state);
        }
        Ok(at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Clock, MeterConfirm, PowerConfirm};
    use crate::sim::{ON_MICRO, SimSupplyMeter};

    #[test]
    fn sim_relay_reports_only_that_it_sent() {
        let world = SimWorld::new();
        let mut relay = SimRelay::new(&world);
        let mut confirm = MeterConfirm(SimSupplyMeter::new(&world));
        // The same answer whether the contacts follow or not: the time the
        // command was sent, nothing about the contacts or the supply.
        world.wait_until(Nanos(10));
        assert_eq!(relay.command(PowerState::On), Ok(Nanos(10)));
        assert_eq!(confirm.read().unwrap().micro, ON_MICRO);
        world.knobs().relay.stuck = true;
        world.wait_until(Nanos(20));
        assert_eq!(relay.command(PowerState::Off), Ok(Nanos(20)));
        assert_eq!(confirm.read().unwrap().micro, ON_MICRO);
        world.knobs().relay.fails = true;
        let error = relay.command(PowerState::Off).unwrap_err();
        assert_eq!(error.role, Role::Power);
        assert_eq!(
            world.power_commands(),
            [(Nanos(10), PowerState::On), (Nanos(20), PowerState::Off)]
        );
        // Sending moves no time on.
        assert_eq!(world.now(), Nanos(20));
    }
}
