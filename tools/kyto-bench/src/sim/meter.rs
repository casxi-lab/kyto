//! The simulated supply meter: measurement and the power confirmation.

use crate::device::{DeviceError, Measurement, Meter, Quantity, Role};

use super::world::{ON_MICRO, SimWorld};

/// A meter of the supply voltage, a device of its own beside the relay. It
/// reads the supply that the relay's contacts feed, never the command:
/// [`ON_MICRO`] while the supply is on, and the residual voltage of
/// [`super::SupplyKnobs`] (0 V unless set) while it is off. Wrapped in
/// [`crate::device::MeterConfirm`], it is the power confirmation.
pub struct SimSupplyMeter {
    world: SimWorld,
}

/// Faults of the meter.
#[derive(Clone, Debug, Default)]
pub struct MeterKnobs {
    /// Every sample fails.
    pub fails: bool,
    /// Every sample reads this value, in microvolts, whatever the supply.
    pub stuck_micro: Option<i64>,
}

impl SimSupplyMeter {
    /// The supply meter of `world`.
    pub fn new(world: &SimWorld) -> Self {
        Self {
            world: world.clone(),
        }
    }
}

impl Meter for SimSupplyMeter {
    fn quantity(&self) -> Quantity {
        Quantity::SupplyVoltage
    }

    fn sample(&mut self) -> Result<Measurement, DeviceError> {
        let world = self.world.borrow();
        let knobs = &world.knobs;
        if knobs.meter.fails {
            return Err(DeviceError::new(
                Role::PowerConfirmation,
                "the simulated meter could not take a sample",
            ));
        }
        let supply = if world.supply_on() {
            ON_MICRO
        } else {
            knobs.supply.residual_micro
        };
        Ok(Measurement {
            at: world.now,
            micro: knobs.meter.stuck_micro.unwrap_or(supply),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Clock, MeterConfirm, Nanos, PowerConfirm, PowerState, PowerSwitch};
    use crate::sim::SimRelay;

    fn bench() -> (SimWorld, SimRelay, MeterConfirm<SimSupplyMeter>) {
        let world = SimWorld::new();
        let relay = SimRelay::new(&world);
        let confirm = MeterConfirm(SimSupplyMeter::new(&world));
        (world, relay, confirm)
    }

    fn reading(confirm: &mut impl PowerConfirm) -> i64 {
        confirm.read().unwrap().micro
    }

    #[test]
    fn confirmation_follows_the_supply_not_the_command() {
        let (world, mut relay, mut confirm) = bench();
        assert_eq!(confirm.quantity(), Quantity::SupplyVoltage);
        assert_eq!(reading(&mut confirm), 0);
        relay.command(PowerState::On).unwrap();
        assert_eq!(reading(&mut confirm), ON_MICRO);
        // Contacts that ignore the off command keep the supply on: the
        // command was sent, and the confirmation still reads on.
        world.knobs().relay.stuck = true;
        relay.command(PowerState::Off).unwrap();
        assert_eq!(reading(&mut confirm), ON_MICRO);
        world.knobs().relay.stuck = false;
        relay.command(PowerState::Off).unwrap();
        assert_eq!(reading(&mut confirm), 0);
        // And contacts that ignore the on command leave it off.
        world.knobs().relay.stuck = true;
        relay.command(PowerState::On).unwrap();
        assert_eq!(reading(&mut confirm), 0);
        // A relay that cannot send changes nothing either.
        world.knobs().relay = Default::default();
        world.knobs().relay.fails = true;
        assert!(relay.command(PowerState::On).is_err());
        assert_eq!(reading(&mut confirm), 0);
        assert_eq!(
            world.power_commands().len(),
            4,
            "the failed command was not sent"
        );
    }

    #[test]
    fn confirmation_is_read_from_its_own_device() {
        let (world, mut relay, mut confirm) = bench();
        relay.command(PowerState::On).unwrap();
        world.knobs().meter.fails = true;
        let error = confirm.read().unwrap_err();
        assert_eq!(error.role, Role::PowerConfirmation);
        // The relay still works, and its command is no reading.
        assert!(relay.command(PowerState::Off).is_ok());
        world.knobs().meter.fails = false;
        world.knobs().meter.stuck_micro = Some(4_000_000);
        assert_eq!(reading(&mut confirm), 4_000_000);
        world.knobs().meter.stuck_micro = None;
        world.wait_until(Nanos(30));
        assert_eq!(
            confirm.read(),
            Ok(Measurement {
                at: Nanos(30),
                micro: 0
            })
        );
    }

    #[test]
    fn the_supply_rises_falls_and_keeps_its_residual_voltage() {
        let (world, mut relay, mut confirm) = bench();
        {
            let mut knobs = world.knobs();
            knobs.supply.rise = Nanos(100);
            knobs.supply.fall = Nanos(50);
            knobs.supply.residual_micro = 600_000;
        }
        assert_eq!(reading(&mut confirm), 600_000);
        relay.command(PowerState::On).unwrap();
        world.wait_until(Nanos(99));
        assert_eq!(reading(&mut confirm), 600_000);
        world.wait_until(Nanos(100));
        assert_eq!(reading(&mut confirm), ON_MICRO);
        relay.command(PowerState::Off).unwrap();
        world.wait_until(Nanos(149));
        assert_eq!(reading(&mut confirm), ON_MICRO);
        world.wait_until(Nanos(150));
        assert_eq!(reading(&mut confirm), 600_000);
        // Contacts that open before the supply has risen keep it off.
        relay.command(PowerState::On).unwrap();
        world.wait_until(Nanos(200));
        relay.command(PowerState::Off).unwrap();
        world.wait_until(Nanos(400));
        assert_eq!(reading(&mut confirm), 600_000);
    }
}
