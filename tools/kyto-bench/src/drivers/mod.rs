//! The drivers of real devices, [`relay`] of the kind `lcus-1`, [`console`]
//! of the kind `serial` and their [`clock`], with the settings that they
//! read and the guard that keeps the settings out of device errors. No run
//! uses them yet: a run opens a bench of simulated devices only.

pub mod clock;
pub mod console;
pub mod relay;

use std::fs;

use serde::{Deserialize, de::DeserializeOwned};

use kyto_bench_core::board::{Board, Parity};
use kyto_bench_core::instance::{Instance, MIN_SEARCHED_LEN};

use crate::device::{DeviceError, Role};

/// `[local.power]` of an `lcus-1`: the serial port of its CH340 and its
/// relay, `"1"`, the one relay that it has.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerSettings {
    pub port: String,
    pub relay: String,
}

/// `[local.console]` of a `serial` console: the serial port of its adapter.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleSettings {
    pub port: String,
}

const ABSOLUTE: &str = "port: an absolute path without a NUL byte";
const ONE_RELAY: &str = "relay: an LCUS-1 has the one relay \"1\"";
const ONE_DEVICE: &str = "port: the same device as the power switch's port";
const ONLY_8N1: &str = "this build's serial console runs at 115200 8N1 only, unlike the board's";

/// The settings of the `lcus-1` power switch and the `serial` console of
/// `instance` for `board`, checked for their form alone: nothing is opened.
pub fn serial_settings(
    board: &Board,
    instance: &Instance,
) -> Result<(PowerSettings, ConsoleSettings), String> {
    let power: PowerSettings = local(instance, Role::Power)?;
    let console: ConsoleSettings = local(instance, Role::Console)?;
    for (role, port) in [(Role::Power, &power.port), (Role::Console, &console.port)] {
        if !port.starts_with('/') || port.contains('\0') {
            return Err(format!("[local.{role}] {ABSOLUTE}"));
        }
    }
    if power.relay != "1" {
        return Err(format!("[local.power] {ONE_RELAY}"));
    }
    let callout = |port: &str| port.replacen("/dev/tty.", "/dev/cu.", 1);
    if callout(&power.port) == callout(&console.port) {
        return Err(format!("[local.console] {ONE_DEVICE}"));
    }
    let line = &board.console;
    if (line.baud, line.data_bits, line.parity, line.stop_bits) != (115_200, 8, Parity::None, 1) {
        return Err(ONLY_8N1.into());
    }
    Ok((power, console))
}

/// The settings of `role` in `instance`, none read as an empty table.
fn local<T: DeserializeOwned>(instance: &Instance, role: Role) -> Result<T, String> {
    let table = instance.local(role).cloned().unwrap_or_default();
    let checked = table.try_into().map_err(|error| error.to_string());
    checked.map_err(|error| format!("[local.{role}]: {}", error.trim_end()))
}

/// The forms of the ports of a bench that no device error may name, in
/// lowercase and of at least [`MIN_SEARCHED_LEN`] bytes: each port and its
/// canonical path, with the file name of each and its `tty.` or `cu.` twin.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hidden(Vec<String>);

impl Hidden {
    /// The forms of `ports`. A canonical path resolves links and opens no
    /// device; a port that does not exist has none.
    pub fn of(ports: &[&str]) -> Self {
        let mut forms = Vec::new();
        for port in ports {
            let canonical = fs::canonicalize(port).map(|path| path.display().to_string());
            for path in [Ok(port.to_string()), canonical].into_iter().flatten() {
                let path = path.to_ascii_lowercase();
                let name = path.rsplit('/').next().unwrap_or_default().to_string();
                let twin = (name.strip_prefix("tty.").map(|rest| format!("cu.{rest}")))
                    .or_else(|| name.strip_prefix("cu.").map(|rest| format!("tty.{rest}")));
                forms.extend([path, name].into_iter().chain(twin));
            }
        }
        forms.retain(|form| form.len() >= MIN_SEARCHED_LEN);
        forms.sort();
        forms.dedup();
        Self(forms)
    }

    /// Whether `text` names a form, ignoring ASCII case.
    fn named(&self, text: &str) -> bool {
        let text = text.to_ascii_lowercase();
        self.0.iter().any(|form| text.contains(form.as_str()))
    }
}

/// A failure of the device of `role` with `detail`, `<step>: <text>`. A
/// detail that names a form of `hidden` keeps only its step.
pub fn device_error(role: Role, detail: &str, hidden: &Hidden) -> DeviceError {
    if !hidden.named(detail) {
        return DeviceError::new(role, detail);
    }
    let step = detail.split_once(": ").map(|(step, _)| step);
    let step = step.filter(|step| !hidden.named(step)).unwrap_or("error");
    let detail = format!("{step}: withheld, as the text named a local setting");
    DeviceError::new(role, detail)
}

#[cfg(test)]
mod tests {
    use kyto_bench_core::board::RASPBERRY_PI_5;
    use kyto_bench_os::clock::SystemClock;

    use super::console::HostConsole;
    use super::relay::Lcus1Relay;
    use super::*;
    use crate::device::{Console, Nanos, PowerState, PowerSwitch};

    const POWER: &str = "[local.power]\nport = \"/dev/cu.usbserial-relay\"\nrelay = \"1\"\n";
    const CONSOLE: &str = "[local.console]\nport = \"/dev/cu.usbserial-console\"\n";

    /// The example instance configuration with an `lcus-1` and a `serial`
    /// console, a salt that kyto does not publish, and `local` as the
    /// settings of its roles.
    fn instance(local: &str) -> Instance {
        let text = include_str!("../../../examples/instance.toml");
        let text = &text[..text.find("\n[local]").unwrap()];
        let text = text
            .replacen(
                "power = { kind = \"simulated\"",
                "power = { kind = \"lcus-1\"",
                1,
            )
            .replacen(
                "console = { kind = \"simulated\"",
                "console = { kind = \"serial\"",
                1,
            );
        let salt = "0".repeat(32);
        Instance::parse(&format!("{text}\n[local]\nsalt = \"{salt}\"\n{local}")).unwrap()
    }

    fn board(text: &str) -> Board {
        Board::parse(text).unwrap()
    }

    #[test]
    fn the_settings_of_real_devices_are_checked_before_any_is_used() {
        let pi5 = board(RASPBERRY_PI_5);
        // Ports that do not exist pass: only their form is checked.
        let settings = serial_settings(&pi5, &instance(&format!("{POWER}{CONSOLE}")));
        let power = PowerSettings {
            port: "/dev/cu.usbserial-relay".into(),
            relay: "1".into(),
        };
        let console = ConsoleSettings {
            port: "/dev/cu.usbserial-console".into(),
        };
        assert_eq!(settings, Ok((power, console)));
        let refused = [
            (CONSOLE.to_string(), "[local.power]: missing field `port`"),
            (POWER.to_string(), "[local.console]: missing field `port`"),
            (
                format!("{}extra = \"x\"\n{CONSOLE}", POWER),
                "[local.power]: unknown field `extra`, expected `port` or `relay`",
            ),
            (
                format!("{POWER}{}", CONSOLE.replace("/dev/", "dev/")),
                "[local.console] port: an absolute path without a NUL byte",
            ),
            (
                format!(
                    "{}{CONSOLE}",
                    POWER.replace("relay\"\n", "re\\u0000lay\"\n")
                ),
                "[local.power] port: an absolute path without a NUL byte",
            ),
            (
                format!("{}{CONSOLE}", POWER.replace("\"1\"", "\"2\"")),
                "[local.power] relay: an LCUS-1 has the one relay \"1\"",
            ),
            (
                format!(
                    "{POWER}{}",
                    CONSOLE.replace("cu.usbserial-console", "tty.usbserial-relay")
                ),
                "[local.console] port: the same device as the power switch's port",
            ),
        ];
        for (local, error) in refused {
            let checked = serial_settings(&pi5, &instance(&local));
            assert_eq!(checked.unwrap_err(), error, "{local}");
        }
        let even = board(&RASPBERRY_PI_5.replacen("parity = \"none\"", "parity = \"even\"", 1));
        let checked = serial_settings(&even, &instance(&format!("{POWER}{CONSOLE}")));
        assert_eq!(checked.unwrap_err(), ONLY_8N1);
    }

    #[test]
    fn device_errors_name_no_local_value() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/no-such-directory");
        let port = format!("{dir}/cu.usbserial-AB12CD");
        let hidden = Hidden::of(&[&port]);
        let power = PowerSettings {
            port: port.clone(),
            relay: "1".into(),
        };
        let mut relay = Lcus1Relay::new(&power, SystemClock::start(), hidden.clone());
        let missing = "open: No such file or directory (os error 2)";
        let error = relay.command(PowerState::Off).unwrap_err();
        assert_eq!(error, DeviceError::new(Role::Power, missing));
        let settings = ConsoleSettings { port: port.clone() };
        let mut console = HostConsole::new(&settings, SystemClock::start(), None, hidden.clone());
        let error = console.open(Nanos(u64::MAX)).unwrap_err();
        assert_eq!(error, DeviceError::new(Role::Console, missing));
        // A detail that names the port, its file name or its sibling, in any
        // case, keeps only its step.
        let withheld = "withheld, as the text named a local setting";
        for detail in [
            format!("open: {port:?}: busy"),
            "read: CU.USBSERIAL-AB12CD went away".to_string(),
            "read: the adapter tty.usbserial-ab12cd went away".to_string(),
            format!("{port}: busy"),
        ] {
            let error = device_error(Role::Console, &detail, &hidden);
            let step = if detail.starts_with('/') {
                "error"
            } else {
                &detail[..4]
            };
            assert_eq!(error.detail, format!("{step}: {withheld}"), "{detail}");
        }
        let kept = device_error(Role::Console, missing, &hidden);
        assert_eq!(kept.detail, missing);
        // A form shorter than 4 bytes is not hidden, as the public version
        // of a receipt does not search for it.
        assert_eq!(Hidden::of(&["/a"]), Hidden::default());
    }
}
