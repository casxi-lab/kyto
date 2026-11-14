//! The supply meter of the kind `ina228-mcp2221`: a TI INA228 (data sheet
//! SLYS021A) on an Adafruit 5832 board, whose shunt is in the supply's VBUS
//! on the board side of the relay and whose VBUS pin is joined to its VIN+,
//! read over I2C through an MCP2221 or MCP2221A USB bridge.
//!
//! The driver fixes every setting that changes what a reading means: the
//! INA228 at I2C address 0x40, the bus at 100 kHz, `CONFIG` 0 after a reset
//! (a shunt range of ±163.84 mV) and `ADC_CONFIG` `BB68h`, continuous shunt
//! and bus conversions of 1052 µs each without averaging. The supply
//! voltage is the bus voltage, the supply current the shunt voltage over
//! the shunt of the model, so no calibration register is used.
//!
//! The bridge is opened at the first reading, and again at the next reading
//! after its link failed; opening finds exactly one bridge, opens it,
//! exclusively on macOS, and checks that an INA228 answers. A reading waits
//! 5 ms from its start, longer than a conversion cycle takes, so that the
//! conversion behind its value started after the reading was asked for,
//! and it is stamped when the value arrives. After each value `ADC_CONFIG`
//! is read again: a reset of the INA228 sets it back to `FB68h` and zeroes
//! its results, which then never pass for a supply that is off. Every error
//! is one of a closed set of texts.

use std::fmt;
use std::rc::Rc;

use super::hid::{HidLink, HidOpen, LinkError, OpenError};
use super::mcp2221::Bridge;
use crate::device::{Clock, DeviceError, Measurement, Meter, Nanos, Quantity, Role};

/// The kind of the meter.
pub const KIND: &str = "ina228-mcp2221";

/// The models of the kind, each with the resistance of its shunt in
/// micro-ohms: the Adafruit 5832 has a 15 mΩ shunt.
pub const MODELS: [(&str, i64); 2] = [
    (
        "Adafruit 5832 INA228 board (15 mOhm shunt) with Adafruit 4471 MCP2221A bridge",
        15_000,
    ),
    (
        "Adafruit 5832 INA228 board (15 mOhm shunt) with Microchip ADM00559 MCP2221 bridge",
        15_000,
    ),
];

/// The I2C address of the INA228, with A0 and A1 open.
const ADDRESS: u8 = 0x40;
/// The registers used.
const CONFIG: u8 = 0x00;
const ADC_CONFIG: u8 = 0x01;
const VSHUNT: u8 = 0x04;
const VBUS: u8 = 0x05;
const MANUFACTURER_ID: u8 = 0x3E;
const DEVICE_ID: u8 = 0x3F;
/// `CONFIG` with its self-clearing reset bit.
const RESET: u16 = 0x8000;
/// `ADC_CONFIG`: continuous shunt and bus conversions, 1052 µs each, one
/// average.
pub const CONVERSIONS: u16 = 0xBB68;
/// The manufacturer, "TI", and the device, in bits 15 to 4 of `DEVICE_ID`.
const TI: u16 = 0x5449;
const INA228: u16 = 0x228;
/// The lowest shunt code taken: −20 mA across 15 mΩ, 300 times the
/// INA228's shunt offset of at most ±1 µV. A lower one means that the
/// current flows from the board to the relay.
const LOWEST_SHUNT_CODE: i64 = -960;
/// How long a reading waits from its start before it reads the value.
pub const FRESH: Nanos = Nanos::from_millis(5);

/// The shunt of `model`, in micro-ohms, when this driver knows the model.
pub fn shunt_of(model: &str) -> Option<i64> {
    let known = MODELS.iter().find(|(known, _)| *known == model);
    known.map(|(_, shunt)| *shunt)
}

/// Why a reading failed: each is one text of a closed set, which holds no
/// text of a library, no path and no serial number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Open(OpenError),
    Link(LinkError),
    NoAnswer,
    Answered { got: u8, sent: u8 },
    Refused { command: u8, status: u8 },
    BusLow { scl: u8, sda: u8 },
    Speed { status: u8 },
    NoDevice,
    Manufacturer(u16),
    Device(u16),
    NotKept { config: u16, adc: u16 },
    Reset(u16),
    Malformed([u8; 3]),
    Reversed,
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bridge = "the MCP2221 USB bridge";
        let other = "the device at I2C address 0x40 is not an INA228";
        match *self {
            Self::Open(error) => error.fmt(f),
            Self::Link(error) => error.fmt(f),
            Self::NoAnswer => write!(f, "{bridge} did not answer within 200 ms"),
            Self::Answered { got, sent } => {
                write!(f, "{bridge} answered command {got:02x} to command {sent:02x}")
            }
            Self::Refused { command, status } => {
                write!(f, "{bridge} refused command {command:02x} with status {status:02x}")
            }
            Self::BusLow { scl, sda } => write!(
                f,
                "the I2C bus is held low (SCL {scl}, SDA {sda}); unplug and replug the meter"
            ),
            Self::Speed { status } => {
                write!(f, "{bridge} did not set the I2C speed (status {status:02x})")
            }
            Self::NoDevice => write!(f, "no INA228 answered at I2C address 0x40"),
            Self::Manufacturer(id) => write!(f, "{other} (manufacturer ID {id:04x})"),
            Self::Device(id) => write!(f, "{other} (device ID {id:04x})"),
            Self::NotKept { config, adc } => write!(
                f,
                "the INA228 did not keep its configuration (CONFIG {config:04x}, ADC_CONFIG {adc:04x})"
            ),
            Self::Reset(adc) => {
                write!(f, "the INA228 was reset since it was set up (ADC_CONFIG {adc:04x})")
            }
            Self::Malformed([b0, b1, b2]) => write!(
                f,
                "the INA228 returned a malformed reading ({b0:02x} {b1:02x} {b2:02x})"
            ),
            Self::Reversed => f.write_str(
                "the current flows from the board to the relay; the shunt is wired the wrong way round",
            ),
        }
    }
}

/// The code of a 24-bit result: bits 23 to 4 as a two's-complement number.
/// Bits 3 to 0 are reserved and read 0.
pub fn code(bytes: [u8; 3]) -> Result<i64, Fault> {
    let raw = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], 0]);
    if raw & 0xF00 != 0 {
        return Err(Fault::Malformed(bytes));
    }
    Ok(i64::from(raw as i32 >> 12))
}

/// The supply voltage of a `VBUS` result, in microvolts: 195.3125 µV a
/// step, truncated. The bus voltage is never negative.
pub fn bus_micro(bytes: [u8; 3]) -> Result<i64, Fault> {
    let code = code(bytes)?;
    if code < 0 {
        return Err(Fault::Malformed(bytes));
    }
    Ok(code * 3125 / 16)
}

/// The supply current of a `VSHUNT` result across `shunt` micro-ohms, in
/// microamperes, positive from the relay to the board: 312.5 nV a step,
/// truncated toward zero.
pub fn current_micro(bytes: [u8; 3], shunt: i64) -> Result<i64, Fault> {
    let code = code(bytes)?;
    if code < LOWEST_SHUNT_CODE {
        return Err(Fault::Reversed);
    }
    Ok(code * 312_500 / shunt)
}

/// The meter behind the bridge that `opener` opens, which waits and stamps
/// on `clock` and reads `quantity` across a shunt of `shunt` micro-ohms.
pub struct Ina228Meter<O: HidOpen> {
    opener: O,
    link: Option<O::Link>,
    /// Whether the bridge and the INA228 are set up before the next reading.
    set_up: bool,
    clock: Rc<dyn Clock>,
    quantity: Quantity,
    shunt: i64,
}

impl<O: HidOpen> Ina228Meter<O> {
    /// The meter; nothing is opened before its first reading.
    pub fn new(opener: O, clock: Rc<dyn Clock>, quantity: Quantity, shunt: i64) -> Self {
        let (link, set_up) = (None, true);
        Self {
            opener,
            link,
            set_up,
            clock,
            quantity,
            shunt,
        }
    }

    /// A reading that started at `since`. A failed link is dropped, so the
    /// next reading opens the bridge again; after any other failure the
    /// transfer is cancelled, as far as the bridge lets it, dropping the
    /// link if that fails on it, and the next reading sets the bridge and
    /// the INA228 up again.
    fn reading(&mut self, since: Nanos) -> Result<Measurement, Fault> {
        let link = match &mut self.link {
            Some(link) => link,
            closed => {
                let link = self.opener.open().map_err(Fault::Open)?;
                self.set_up = true;
                closed.insert(link)
            }
        };
        let mut bridge = Bridge {
            link,
            clock: &*self.clock,
        };
        let (quantity, shunt) = (self.quantity, self.shunt);
        let measured = measure(&mut bridge, &mut self.set_up, quantity, shunt, since);
        match measured {
            Err(Fault::Link(_)) => self.link = None,
            Err(_) => {
                if let Err(Fault::Link(_)) = bridge.cancel() {
                    self.link = None;
                }
                self.set_up = true;
            }
            Ok(_) => {}
        }
        measured
    }
}

impl<O: HidOpen> Meter for Ina228Meter<O> {
    fn quantity(&self) -> Quantity {
        self.quantity
    }

    fn sample(&mut self) -> Result<Measurement, DeviceError> {
        let since = self.clock.now();
        let reading = self.reading(since);
        reading.map_err(|fault| DeviceError::new(Role::PowerConfirmation, fault.to_string()))
    }
}

/// The 16-bit `register`.
fn register<L: HidLink>(bridge: &mut Bridge<'_, L>, register: u8) -> Result<u16, Fault> {
    let (bytes, _) = bridge.read_register::<2>(ADDRESS, register)?;
    Ok(u16::from_be_bytes(bytes))
}

/// Sets the bridge up, checks that an INA228 answers and configures it.
fn set_up<L: HidLink>(bridge: &mut Bridge<'_, L>) -> Result<(), Fault> {
    bridge.set_up()?;
    let manufacturer = register(bridge, MANUFACTURER_ID)?;
    if manufacturer != TI {
        return Err(Fault::Manufacturer(manufacturer));
    }
    let device = register(bridge, DEVICE_ID)?;
    if device >> 4 != INA228 {
        return Err(Fault::Device(device));
    }
    bridge.write_register(ADDRESS, CONFIG, RESET)?;
    let reset = bridge.clock.now().saturating_add(Nanos::from_millis(2));
    bridge.clock.wait_until(reset);
    bridge.write_register(ADDRESS, ADC_CONFIG, CONVERSIONS)?;
    let (config, adc) = (register(bridge, CONFIG)?, register(bridge, ADC_CONFIG)?);
    if (config, adc) != (0, CONVERSIONS) {
        return Err(Fault::NotKept { config, adc });
    }
    // The first conversions.
    let converted = bridge.clock.now().saturating_add(Nanos::from_millis(10));
    bridge.clock.wait_until(converted);
    Ok(())
}

/// One reading of `quantity` that started at `since`, after a set-up when
/// `set_up` is set.
fn measure<L: HidLink>(
    bridge: &mut Bridge<'_, L>,
    set_up: &mut bool,
    quantity: Quantity,
    shunt: i64,
    since: Nanos,
) -> Result<Measurement, Fault> {
    if *set_up {
        self::set_up(bridge)?;
        *set_up = false;
    }
    bridge.clock.wait_until(since.saturating_add(FRESH));
    let result = match quantity {
        Quantity::SupplyVoltage => VBUS,
        Quantity::SupplyCurrent => VSHUNT,
    };
    let (value, at) = bridge.read_register::<3>(ADDRESS, result)?;
    let adc = register(bridge, ADC_CONFIG)?;
    if adc != CONVERSIONS {
        return Err(Fault::Reset(adc));
    }
    let micro = match quantity {
        Quantity::SupplyVoltage => bus_micro(value)?,
        Quantity::SupplyCurrent => current_micro(value, shunt)?,
    };
    Ok(Measurement { at, micro })
}

#[cfg(test)]
mod tests {
    use kyto_bench_core::instance::Confirmation;
    use kyto_bench_core::receipt::{Command, EventKind};

    use super::*;
    use crate::device::{MeterConfirm, PowerState, PowerSwitch};
    use crate::drivers::hid::Report;
    use crate::sim::ina228::{BridgeKnobs, ON_CURRENT_MICRO, SimBridge};
    use crate::sim::{self, ON_MICRO, SimRelay, SimWorld};
    use crate::{env, run};

    const SHUNT: i64 = 15_000;

    /// A fault set on the knobs of a simulated bridge.
    type Knob = fn(&mut BridgeKnobs);

    /// The texts of every error of the meter, each `{}` a hexadecimal or
    /// decimal number.
    const TEMPLATES: [&str; 17] = [
        "no MCP2221 USB bridge is connected",
        "more than one MCP2221 USB bridge is connected; connect only the meter's bridge",
        "the MCP2221 USB bridge cannot be opened; another program may be using it",
        "the MCP2221 USB bridge did not take a report",
        "the MCP2221 USB bridge is gone",
        "the MCP2221 USB bridge did not answer within 200 ms",
        "the MCP2221 USB bridge answered command {} to command {}",
        "the MCP2221 USB bridge refused command {} with status {}",
        "the I2C bus is held low (SCL {}, SDA {}); unplug and replug the meter",
        "the MCP2221 USB bridge did not set the I2C speed (status {})",
        "no INA228 answered at I2C address 0x40",
        "the device at I2C address 0x40 is not an INA228 (manufacturer ID {})",
        "the device at I2C address 0x40 is not an INA228 (device ID {})",
        "the INA228 did not keep its configuration (CONFIG {}, ADC_CONFIG {})",
        "the INA228 was reset since it was set up (ADC_CONFIG {})",
        "the INA228 returned a malformed reading ({} {} {})",
        "the current flows from the board to the relay; the shunt is wired the wrong way round",
    ];

    /// A world with its supply on, its relay, and a meter of `quantity`
    /// behind the simulated bridge of the world.
    fn on_bench(quantity: Quantity) -> (SimWorld, SimBridge, Ina228Meter<SimBridge>) {
        let world = SimWorld::new();
        SimRelay::new(&world).command(PowerState::On).unwrap();
        let bridge = SimBridge::new(&world);
        let clock = Rc::new(world.clone());
        let meter = Ina228Meter::new(bridge.clone(), clock, quantity, SHUNT);
        (world, bridge, meter)
    }

    fn micro(meter: &mut Ina228Meter<SimBridge>) -> i64 {
        meter.sample().unwrap().micro
    }

    fn detail(meter: &mut Ina228Meter<SimBridge>) -> String {
        let error = meter.sample().unwrap_err();
        assert_eq!(error.role, Role::PowerConfirmation);
        error.detail
    }

    /// Whether `text` is `template` with each `{}` a run of hexadecimal
    /// digits.
    fn fits(text: &str, template: &str) -> bool {
        let mut parts = template.split("{}");
        let Some(mut rest) = text.strip_prefix(parts.next().unwrap_or_default()) else {
            return false;
        };
        for part in parts {
            let digits = rest.trim_start_matches(|c: char| c.is_ascii_hexdigit());
            match digits.strip_prefix(part) {
                Some(after) if digits.len() < rest.len() => rest = after,
                _ => return false,
            }
        }
        rest.is_empty()
    }

    #[test]
    fn register_codes_convert_to_micro_units() {
        let bus = [
            ([0x06, 0x40, 0x00], Ok(5_000_000)),
            ([0x06, 0x60, 0x00], Ok(5_100_000)),
            ([0x00, 0x00, 0x10], Ok(195)),
            ([0x7F, 0xFF, 0xF0], Ok(102_399_804)),
            (
                [0x80, 0x00, 0x00],
                Err(Fault::Malformed([0x80, 0x00, 0x00])),
            ),
            (
                [0x00, 0x00, 0x01],
                Err(Fault::Malformed([0x00, 0x00, 0x01])),
            ),
        ];
        for (bytes, expected) in bus {
            assert_eq!(bus_micro(bytes), expected, "{bytes:02x?}");
        }
        let shunt = [
            ([0x00, 0x96, 0x00], Ok(50_000)),
            ([0xFF, 0xFF, 0xF0], Ok(-20)),
            ([0xFF, 0xC4, 0x00], Ok(-20_000)),
            ([0xFF, 0xC3, 0xF0], Err(Fault::Reversed)),
            ([0xFF, 0x6A, 0x00], Err(Fault::Reversed)),
            ([0x80, 0x00, 0x00], Err(Fault::Reversed)),
            (
                [0x00, 0x00, 0x08],
                Err(Fault::Malformed([0x00, 0x00, 0x08])),
            ),
        ];
        for (bytes, expected) in shunt {
            assert_eq!(current_micro(bytes, SHUNT), expected, "{bytes:02x?}");
        }
        assert_eq!(MODELS.map(|(model, _)| shunt_of(model)), [Some(SHUNT); 2]);
        assert_eq!(shunt_of("Adafruit 5832 INA228 board"), None);
    }

    #[test]
    fn the_meter_opens_only_an_ina228_behind_one_bridge() {
        let (_, bridge, mut meter) = on_bench(Quantity::SupplyVoltage);
        assert_eq!(bridge.opens(), 0, "nothing is opened before a reading");
        assert_eq!(micro(&mut meter), ON_MICRO);
        assert_eq!(micro(&mut meter), ON_MICRO);
        assert_eq!(
            (bridge.opens(), bridge.configuration()),
            (1, (0, CONVERSIONS))
        );
        // A link that fails is dropped, and the next reading opens the
        // bridge again, sets it up and checks the INA228's identity again.
        let other = "the device at I2C address 0x40 is not an INA228 (manufacturer ID 5448)";
        for error in [LinkError::Gone, LinkError::NotTaken] {
            bridge.knobs().link = Some(error);
            assert_eq!(detail(&mut meter), error.to_string());
            bridge.knobs().link = None;
            bridge.knobs().ids = Some((0x5448, 0x2281));
            assert_eq!(detail(&mut meter), other);
            bridge.knobs().ids = None;
            assert_eq!(micro(&mut meter), ON_MICRO);
        }
        assert_eq!(bridge.opens(), 3);
        // No bridge, two, or one that another program holds: the reading
        // fails, and the next tries again.
        let (_, bridge, mut meter) = on_bench(Quantity::SupplyVoltage);
        let errors = [
            OpenError::NoBridge,
            OpenError::SeveralBridges,
            OpenError::NotOpened,
        ];
        for (tried, error) in (1..).zip(errors) {
            bridge.knobs().open = Some(error);
            assert_eq!(detail(&mut meter), error.to_string());
            assert_eq!(bridge.opens(), tried);
        }
        bridge.knobs().open = None;
        assert_eq!(micro(&mut meter), ON_MICRO);
        // A device that is not an INA228, one that does not keep its
        // configuration, and a bus or a bridge that is not ready are
        // refused, and the next reading sets them up again.
        let refused: [(Knob, &str); 8] = [
            (
                |knobs| knobs.ids = Some((0x5448, 0x2281)),
                "the device at I2C address 0x40 is not an INA228 (manufacturer ID 5448)",
            ),
            (
                |knobs| knobs.ids = Some((0x5449, 0x2271)),
                "the device at I2C address 0x40 is not an INA228 (device ID 2271)",
            ),
            (
                |knobs| knobs.absent = true,
                "no INA228 answered at I2C address 0x40",
            ),
            (
                |knobs| knobs.ignores_writes = true,
                "the INA228 did not keep its configuration (CONFIG 0000, ADC_CONFIG fb68)",
            ),
            (
                |knobs| (knobs.scl_low, knobs.sda_low) = (true, true),
                "the I2C bus is held low (SCL 0, SDA 0); unplug and replug the meter",
            ),
            (
                |knobs| knobs.sda_low = true,
                "the I2C bus is held low (SCL 1, SDA 0); unplug and replug the meter",
            ),
            (
                |knobs| knobs.scl_low = true,
                "the I2C bus is held low (SCL 0, SDA 1); unplug and replug the meter",
            ),
            (
                |knobs| knobs.no_speed = true,
                "the MCP2221 USB bridge did not set the I2C speed (status 21)",
            ),
        ];
        for (fault, text) in refused {
            let (_, bridge, mut meter) = on_bench(Quantity::SupplyVoltage);
            fault(&mut bridge.knobs());
            assert_eq!(detail(&mut meter), text);
            *bridge.knobs() = BridgeKnobs::default();
            assert_eq!(micro(&mut meter), ON_MICRO, "{text}");
        }
        // Another revision of the INA228 is an INA228.
        let (_, bridge, mut meter) = on_bench(Quantity::SupplyVoltage);
        bridge.knobs().ids = Some((0x5449, 0x2282));
        assert_eq!(micro(&mut meter), ON_MICRO);
    }

    #[test]
    fn a_reset_ina228_never_reads_as_off() {
        let (world, bridge, mut meter) = on_bench(Quantity::SupplyVoltage);
        assert_eq!(micro(&mut meter), ON_MICRO);
        // After a reset the result reads 0, which would be off; the reading
        // fails instead, and the next one sets the INA228 up again.
        bridge.knobs().reset_at = Some(world.now());
        let reset = "the INA228 was reset since it was set up (ADC_CONFIG fb68)";
        assert_eq!(detail(&mut meter), reset);
        assert_eq!(bridge.conversion(), None, "the value read was a zero");
        assert_eq!(micro(&mut meter), ON_MICRO);
        assert_eq!(bridge.configuration(), (0, CONVERSIONS));
        // A reset at any time from the start of a reading to past its end,
        // every 0.5 ms: each reading until the INA228 is set up again fails
        // or reads the supply, never 0, and `ADC_CONFIG` read after the
        // value is what makes it so. A reset between the pointer and the
        // read of `ADC_CONFIG` points the read at `CONFIG`, which reads 0.
        let resets = [
            reset,
            "the INA228 was reset since it was set up (ADC_CONFIG 0000)",
        ];
        for offset in (0..=40).map(|half| Nanos(half * 500_000)) {
            let (world, bridge, mut meter) = on_bench(Quantity::SupplyVoltage);
            assert_eq!(micro(&mut meter), ON_MICRO);
            bridge.knobs().reset_at = Some(world.now().saturating_add(offset));
            for tried in 1.. {
                assert!(tried <= 4, "{offset:?}: the meter did not recover");
                let pending = bridge.knobs().reset_at.is_some();
                match meter.sample() {
                    Ok(reading) => {
                        assert_eq!(reading.micro, ON_MICRO, "{offset:?}");
                        if !pending {
                            break;
                        }
                    }
                    Err(error) => {
                        let detail = error.detail.as_str();
                        assert!(resets.contains(&detail), "{offset:?}: {detail}");
                    }
                }
            }
            assert_eq!(bridge.configuration(), (0, CONVERSIONS), "{offset:?}");
        }
    }

    /// The simulated bridge, whose link is gone from the first transfer
    /// that a failed reading cancels: a reading of a malformed result.
    #[derive(Clone)]
    struct Unplugged(SimBridge);

    impl HidOpen for Unplugged {
        type Link = Unplugged;

        fn open(&mut self) -> Result<Unplugged, OpenError> {
            self.0.open().map(Unplugged)
        }
    }

    impl HidLink for Unplugged {
        fn send(&mut self, report: &Report) -> Result<(), LinkError> {
            if report[..3] == [0x10, 0, 0x10] && self.0.knobs().malformed {
                self.0.knobs().link = Some(LinkError::Gone);
            }
            self.0.send(report)
        }

        fn receive(&mut self) -> Result<Option<Report>, LinkError> {
            self.0.receive()
        }
    }

    #[test]
    fn a_link_that_fails_while_a_transfer_is_cancelled_is_dropped() {
        let (world, bridge, _) = on_bench(Quantity::SupplyVoltage);
        let clock = Rc::new(world.clone());
        let opener = Unplugged(bridge.clone());
        let mut meter = Ina228Meter::new(opener, clock, Quantity::SupplyVoltage, SHUNT);
        assert_eq!(meter.sample().unwrap().micro, ON_MICRO);
        bridge.knobs().malformed = true;
        let malformed = "the INA228 returned a malformed reading (06 60 01)";
        assert_eq!(meter.sample().unwrap_err().detail, malformed);
        // The bridge is back: the next reading opens it again.
        *bridge.knobs() = BridgeKnobs::default();
        assert_eq!(meter.sample().unwrap().micro, ON_MICRO);
        assert_eq!(bridge.opens(), 2);
    }

    #[test]
    fn stale_bridge_data_is_refused() {
        let (world, bridge, mut meter) = on_bench(Quantity::SupplyVoltage);
        assert_eq!(micro(&mut meter), ON_MICRO);
        // Fetches that give the data of the transfer before, the two bytes
        // of ADC_CONFIG for a result of three, are fetched again, and
        // after three the reading fails rather than take them.
        bridge.knobs().stale = 2;
        assert_eq!(micro(&mut meter), ON_MICRO);
        bridge.knobs().stale = 3;
        assert_eq!(detail(&mut meter), "no INA228 answered at I2C address 0x40");
        assert_eq!(micro(&mut meter), ON_MICRO);
        // Neither is an answer that comes after 200 ms of the bench clock,
        // nor one that echoes another command, nor a refusal.
        let refused: [(Knob, &str); 3] = [
            (
                |knobs| knobs.late = Some(Nanos::from_millis(250)),
                "the MCP2221 USB bridge did not answer within 200 ms",
            ),
            (
                |knobs| knobs.echo = Some(0x40),
                "the MCP2221 USB bridge answered command 40 to command 94",
            ),
            (
                |knobs| knobs.busy = true,
                "the MCP2221 USB bridge refused command 94 with status 01",
            ),
        ];
        for (fault, text) in refused {
            fault(&mut bridge.knobs());
            assert_eq!(detail(&mut meter), text);
            *bridge.knobs() = BridgeKnobs::default();
            // What came late is taken before the next exchange.
            world.wait_until(world.now().saturating_add(Nanos::from_millis(1000)));
            assert_eq!(micro(&mut meter), ON_MICRO, "{text}");
        }
    }

    #[test]
    fn a_reading_comes_from_a_conversion_that_started_after_it_was_asked() {
        for (quantity, on) in [
            (Quantity::SupplyVoltage, ON_MICRO),
            (Quantity::SupplyCurrent, ON_CURRENT_MICRO),
        ] {
            let (world, bridge, mut meter) = on_bench(quantity);
            assert_eq!(meter.quantity(), quantity);
            for gap in [0, 1, 2, 3, 100, 0] {
                world.wait_until(world.now().saturating_add(Nanos::from_millis(gap)));
                let asked = world.now();
                let reading = meter.sample().unwrap();
                let (start, end) = bridge.conversion().unwrap();
                assert!(start >= asked && reading.at >= end, "{asked:?} {start:?}");
                assert_eq!(reading.micro, on);
            }
        }
    }

    #[test]
    fn every_meter_error_is_one_of_its_templates() {
        let faults = [
            Fault::Open(OpenError::NoBridge),
            Fault::Open(OpenError::SeveralBridges),
            Fault::Open(OpenError::NotOpened),
            Fault::Link(LinkError::NotTaken),
            Fault::Link(LinkError::Gone),
            Fault::NoAnswer,
            Fault::Answered { got: 0, sent: 0xFF },
            Fault::Refused {
                command: 0x90,
                status: 1,
            },
            Fault::BusLow { scl: 0, sda: 1 },
            Fault::Speed { status: 0x21 },
            Fault::NoDevice,
            Fault::Manufacturer(0xFFFF),
            Fault::Device(0),
            Fault::NotKept {
                config: 0x8000,
                adc: 0xFB68,
            },
            Fault::Reset(0xFB68),
            Fault::Malformed([0xAB, 0xCD, 0xEF]),
            Fault::Reversed,
        ];
        for (fault, template) in faults.iter().zip(TEMPLATES) {
            let fitting = TEMPLATES.iter().filter(|t| fits(&fault.to_string(), t));
            assert_eq!(fitting.collect::<Vec<_>>(), [&template], "{fault:?}");
        }
        // Every fault of the simulated bridge and INA228, each in the first
        // reading of a meter, and a reset after one.
        let knobs: [Knob; 17] = [
            |knobs| knobs.open = Some(OpenError::NoBridge),
            |knobs| knobs.open = Some(OpenError::SeveralBridges),
            |knobs| knobs.open = Some(OpenError::NotOpened),
            |knobs| knobs.link = Some(LinkError::NotTaken),
            |knobs| knobs.link = Some(LinkError::Gone),
            |knobs| knobs.late = Some(Nanos(u64::MAX)),
            |knobs| knobs.echo = Some(0x93),
            |knobs| knobs.busy = true,
            |knobs| knobs.sda_low = true,
            |knobs| knobs.no_speed = true,
            |knobs| knobs.absent = true,
            |knobs| knobs.ids = Some((0, 0x2281)),
            |knobs| knobs.ids = Some((0x5449, 0)),
            |knobs| knobs.ignores_writes = true,
            |knobs| knobs.stale = 3,
            |knobs| knobs.malformed = true,
            |knobs| knobs.reversed = true,
        ];
        let mut details = Vec::new();
        for knob in knobs {
            let (_, bridge, mut meter) = on_bench(Quantity::SupplyCurrent);
            knob(&mut bridge.knobs());
            details.push(detail(&mut meter));
        }
        let (world, bridge, mut meter) = on_bench(Quantity::SupplyCurrent);
        assert_eq!(micro(&mut meter), ON_CURRENT_MICRO);
        bridge.knobs().reset_at = Some(world.now());
        details.push(detail(&mut meter));
        let mut found = Vec::new();
        for detail in details {
            let template = TEMPLATES.iter().position(|t| fits(&detail, t));
            assert!(template.is_some() && !detail.contains('/'), "{detail}");
            found.extend(template);
        }
        found.sort();
        found.dedup();
        assert_eq!(found, (0..TEMPLATES.len()).collect::<Vec<_>>());
    }

    #[test]
    fn readings_of_the_ina228_confirm_the_supply_not_the_command() {
        let world = SimWorld::new();
        let mut bench = env::simulated(&world, sim::replay(Vec::new()));
        let clock = Rc::new(world.clone());
        let meter = Ina228Meter::new(
            SimBridge::new(&world),
            clock,
            Quantity::SupplyVoltage,
            SHUNT,
        );
        bench.confirm = Box::new(MeterConfirm(meter));
        let confirmation = Confirmation {
            quantity: Quantity::SupplyVoltage,
            on_at_least_micro: 4_500_000,
            off_at_most_micro: 1_000_000,
            consecutive: 3,
            poll_ms: 100,
            on_timeout_ms: 3000,
            off_timeout_ms: 3000,
        };
        // Whether readings confirmed `command`, and what they read.
        let mut switch = |command| {
            let mut events = Vec::new();
            let sent = run::switch_confirmed(&mut bench, &confirmation, command, &mut events, 0);
            let readings = events.iter().filter_map(|event| match event.event {
                EventKind::Reading { micro, .. } => Some(micro),
                _ => None,
            });
            (sent.is_some(), readings.collect::<Vec<_>>())
        };
        assert_eq!(switch(Command::On), (true, vec![ON_MICRO; 3]));
        assert_eq!(switch(Command::Off), (true, vec![0; 3]));
        // Contacts that ignore a command: it was sent, and the readings
        // follow the supply, which stays off.
        world.knobs().relay.stuck = true;
        let (confirmed, readings) = switch(Command::On);
        assert!(!confirmed && readings.len() > 3 && readings.iter().all(|micro| *micro == 0));
        world.knobs().relay.stuck = false;
        assert!(switch(Command::On).0);
        // A residual voltage above the off threshold is not off.
        world.knobs().supply.residual_micro = 1_200_000;
        let (confirmed, readings) = switch(Command::Off);
        let residual = readings.iter().all(|micro| *micro == 1_200_000);
        assert!(!confirmed && readings.len() > 3 && residual, "{readings:?}");
    }
}
