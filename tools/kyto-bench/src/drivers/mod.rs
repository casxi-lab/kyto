//! The drivers of real devices, [`relay`] of the kind `lcus-1`, [`ina228`]
//! of the kind `ina228-mcp2221` behind the USB bridge of [`mcp2221`] and
//! [`hid`], [`console`] of the kind `serial`, [`tftp`] of the kind
//! `tftp-directory`, and their [`clock`]; the settings that they read, none
//! of which opens a device; and the guard that keeps the settings out of
//! device errors. A bench's devices are all simulated or all real.

pub mod clock;
pub mod console;
pub mod hid;
pub mod ina228;
pub mod mcp2221;
pub mod relay;
pub mod tftp;

use std::fs;
use std::rc::Rc;

use serde::{Deserialize, de::DeserializeOwned};

use kyto_bench_core::board::{Board, Parity};
use kyto_bench_core::instance::{Instance, MIN_SEARCHED_LEN, SIMULATED};
use kyto_bench_os::clock::SystemClock;
use kyto_bench_os::tty::catch_interrupts;

use crate::device::{Clock, DeviceError, MeterConfirm, Quantity, Role};
use crate::env::{Bench, Interrupt};

/// The kind of the real device of each role, in the order of [`Role::ALL`].
pub const KINDS: [&str; 4] = ["lcus-1", ina228::KIND, "serial", tftp::KIND];

const MIXED: &str = "a bench's devices are all simulated or all real";
const NO_MODEL: &str =
    "this build has no power_confirmation device of the kind ina228-mcp2221 and the model";
const NO_METER_SETTINGS: &str =
    "[local.power_confirmation]: an ina228-mcp2221 meter takes no settings";

/// Whether the devices of `instance` are real, not simulated; an error
/// names a kind that this build has no driver of for its role, or says that
/// the two are mixed.
pub fn real(instance: &Instance) -> Result<bool, String> {
    let mut real = 0;
    for (role, driver) in Role::ALL.into_iter().zip(KINDS) {
        let kind = &instance.devices().device(role).kind;
        if kind != driver && kind != SIMULATED {
            let no = format!("this build has no {role} device of the kind {kind}");
            return Err(no);
        }
        real += usize::from(kind == driver);
    }
    if real % KINDS.len() != 0 {
        return Err(MIXED.into());
    }
    Ok(real > 0)
}

/// The settings of a bench of real devices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub power: PowerSettings,
    pub console: ConsoleSettings,
    /// The quantity that the meter reads, and the shunt of its model in
    /// micro-ohms.
    pub meter: (Quantity, i64),
    pub delivery: tftp::TftpSettings,
}

/// The settings of the real devices of `instance` for `board`, checked for
/// their form alone: no device, port or directory is opened or read.
pub fn settings(board: &Board, instance: &Instance) -> Result<Settings, String> {
    let (power, console) = serial_settings(board, instance)?;
    let model = &instance.devices().power_confirmation.model;
    let shunt = ina228::shunt_of(model).ok_or_else(|| format!("{NO_MODEL} {model}"))?;
    let meter = instance.local(Role::PowerConfirmation);
    if meter.is_some_and(|meter| !meter.is_empty()) {
        return Err(NO_METER_SETTINGS.into());
    }
    let meter = (instance.parameters().confirmation.quantity, shunt);
    let delivery = tftp::settings(instance)?;
    Ok(Settings {
        power,
        console,
        meter,
        delivery,
    })
}

/// The bench of real devices of `settings`, whose clock starts now. From
/// now on SIGINT, SIGTERM and SIGHUP set the flag of the bench's interrupt
/// instead of ending the process. Tests never call it.
pub fn open(settings: Settings) -> Result<Bench, String> {
    let flag = catch_interrupts().map_err(|error| format!("cannot catch interrupts: {error}"))?;
    let clock = SystemClock::start();
    let shared: Rc<dyn Clock> = Rc::new(clock);
    let (power, (quantity, shunt)) = (&settings.power, settings.meter);
    let hidden = Hidden::of(&[&power.port, &settings.console.port]);
    let meter = ina228::Ina228Meter::new(hid::HidapiOpener, Rc::clone(&shared), quantity, shunt);
    let serial = console::HostConsole::new(&settings.console, clock, Some(flag), hidden.clone());
    Ok(Bench {
        clock: shared,
        power: Box::new(relay::Lcus1Relay::new(power, clock, hidden)),
        confirm: Box::new(MeterConfirm(meter)),
        console: Box::new(serial),
        relay: Some(power.relay.clone()),
        delivery: Box::new(tftp::TftpDirectory::new(settings.delivery)),
        interrupt: Interrupt::of(flag),
    })
}

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
    use std::path::Path;

    use kyto_bench_core::board::RASPBERRY_PI_5;
    use kyto_bench_core::instance::InstanceError;

    use super::console::HostConsole;
    use super::relay::Lcus1Relay;
    use super::tftp::TftpSettings;
    use super::*;
    use crate::device::{Console, Nanos, PowerState, PowerSwitch};
    use crate::env::{Env, HostEnv};

    const POWER: &str = "[local.power]\nport = \"/dev/cu.usbserial-relay\"\nrelay = \"1\"\n";
    const CONSOLE: &str = "[local.console]\nport = \"/dev/cu.usbserial-console\"\n";
    const DELIVERY: &str = "[local.delivery]\ndir = \"/srv/tftp/board\"\n";
    const REAL: [&str; 4] = ["lcus-1", ina228::KIND, "serial", tftp::KIND];

    /// The example instance configuration with devices of `kinds`, for the
    /// roles in their order, the meter of `model`, the path prefix
    /// `prefix`, a salt that kyto does not publish, and `local` as the
    /// settings of its roles.
    pub(super) fn instance(kinds: [&str; 4], model: &str, prefix: &str, local: &str) -> Instance {
        let text = include_str!("../../../examples/instance.toml");
        let mut text = text[..text.find("\n[local]").unwrap()].to_string();
        for (role, kind) in Role::ALL.iter().zip(kinds) {
            let model = if *role == Role::PowerConfirmation {
                model
            } else {
                "simulated"
            };
            let device = format!("{role} = {{ kind = \"{kind}\", model = \"{model}\" }}");
            let simulated = format!("{role} = {{ kind = \"simulated\", model = \"simulated\" }}");
            text = text.replacen(&simulated, &device, 1);
        }
        let text = text.replacen(
            "path_prefix = \"\"",
            &format!("path_prefix = \"{prefix}\""),
            1,
        );
        let salt = "0".repeat(32);
        Instance::parse(&format!("{text}\n[local]\nsalt = \"{salt}\"\n{local}")).unwrap()
    }

    /// An instance configuration of real devices with `local` as the
    /// settings of its roles.
    fn of_real(local: &str) -> Instance {
        instance(REAL, ina228::MODELS[0].0, "current/", local)
    }

    fn board(text: &str) -> Board {
        Board::parse(text).unwrap()
    }

    #[test]
    fn the_settings_of_real_devices_are_checked_before_any_is_used() {
        let pi5 = board(RASPBERRY_PI_5);
        // Ports and a directory that do not exist pass: only their form is
        // checked.
        let all = format!("{POWER}{CONSOLE}{DELIVERY}");
        let power = PowerSettings {
            port: "/dev/cu.usbserial-relay".into(),
            relay: "1".into(),
        };
        let console = ConsoleSettings {
            port: "/dev/cu.usbserial-console".into(),
        };
        let delivery = TftpSettings {
            dir: "/srv/tftp/board".into(),
            link: "current".into(),
        };
        let expected = Settings {
            power: power.clone(),
            console: console.clone(),
            meter: (Quantity::SupplyVoltage, 15_000),
            delivery,
        };
        assert_eq!(settings(&pi5, &of_real(&all)), Ok(expected.clone()));
        assert_eq!(serial_settings(&pi5, &of_real(&all)), Ok((power, console)));
        // Either model of the meter, and an empty table of its settings.
        let adm00559 = instance(REAL, ina228::MODELS[1].0, "current/", &all);
        assert_eq!(settings(&pi5, &adm00559), Ok(expected.clone()));
        let empty = of_real(&format!("{all}[local.power_confirmation]\n"));
        assert_eq!(settings(&pi5, &empty), Ok(expected));
        let refused = [
            (CONSOLE.to_string(), "[local.power]: missing field `port`"),
            (POWER.to_string(), "[local.console]: missing field `port`"),
            (
                format!("{}extra = \"x\"\n{CONSOLE}", POWER),
                "[local.power]: unknown field `extra`, expected `port` or `relay`",
            ),
            (
                format!("{POWER}{CONSOLE}extra = \"x\"\n"),
                "[local.console]: unknown field `extra`, expected `port`",
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
            (
                format!("{POWER}{CONSOLE}[local.power_confirmation]\nbridge = \"x\"\n"),
                "[local.power_confirmation]: an ina228-mcp2221 meter takes no settings",
            ),
            (
                format!("{POWER}{CONSOLE}"),
                "[local.delivery]: missing field `dir`",
            ),
        ];
        for (local, error) in refused {
            let local = if local.contains("[local.delivery]") || error.contains("delivery") {
                local
            } else {
                format!("{local}{DELIVERY}")
            };
            let checked = settings(&pi5, &of_real(&local));
            assert_eq!(checked.unwrap_err(), error, "{local}");
        }
        let model = "Adafruit 5832 INA228 board";
        let unknown = instance(REAL, model, "current/", &all);
        let error = format!(
            "this build has no power_confirmation device of the kind {} and the model {model}",
            ina228::KIND
        );
        assert_eq!(settings(&pi5, &unknown), Err(error));
        let prefix = instance(REAL, ina228::MODELS[0].0, "", &all);
        assert_eq!(
            settings(&pi5, &prefix).unwrap_err(),
            tftp::settings(&prefix).unwrap_err()
        );
        // Every setting of the board's console line other than 115200 8N1
        // is refused.
        for (line, other) in [
            ("parity = \"none\"", "parity = \"even\""),
            ("baud = 115200", "baud = 9600"),
            ("data_bits = 8", "data_bits = 7"),
            ("stop_bits = 1", "stop_bits = 2"),
        ] {
            let other = board(&RASPBERRY_PI_5.replacen(line, other, 1));
            let checked = settings(&other, &of_real(&all));
            assert_eq!(checked.unwrap_err(), ONLY_8N1, "{line}");
        }
    }

    #[test]
    fn the_instance_template_and_the_bench_example_open_with_a_fresh_salt() {
        let pi5 = board(RASPBERRY_PI_5);
        let identity = include_str!("../../../templates/identity.toml");
        let identity = toml::Value::Table(toml::from_str(identity).unwrap());
        let files = [
            include_str!("../../../templates/instance.toml"),
            include_str!("../../../examples/bench-instance.toml"),
        ];
        let mut tables = Vec::new();
        for text in files {
            // As written, each is refused for its salt.
            let refused = Instance::parse(text).unwrap_err();
            let salt = matches!(&refused, InstanceError::Invalid { at, .. } if at == "local.salt");
            assert!(salt, "{refused}");
            let placeholder = "replace with the output of openssl rand -hex 16";
            let text = text.replacen(placeholder, &crate::env::nonce().unwrap(), 1);
            let instance = Instance::parse(&text).unwrap();
            instance.check_board(&pi5).unwrap();
            // Real devices of the kinds of the drivers, whose settings are
            // key names alone.
            let devices = instance.devices();
            assert_eq!(
                Role::ALL.map(|role| devices.device(role).kind.as_str()),
                KINDS
            );
            assert!(!devices.any_simulated());
            for role in Role::ALL {
                let values = instance
                    .local(role)
                    .into_iter()
                    .flat_map(|local| local.values());
                assert!(
                    values.into_iter().all(|value| value.as_str() == Some("")),
                    "{role}"
                );
            }
            // Filled in, the settings are those that the drivers read.
            let filled = text
                .replacen("port = \"\"", "port = \"/dev/cu.kyto-test-relay\"", 1)
                .replacen("relay = \"\"", "relay = \"1\"", 1)
                .replacen("port = \"\"", "port = \"/dev/cu.kyto-test-console\"", 1)
                .replacen("dir = \"\"", "dir = \"/kyto-test/tftp\"", 1);
            let read = settings(&pi5, &Instance::parse(&filled).unwrap()).unwrap();
            assert_eq!(
                (read.power.relay.as_str(), read.delivery.link.as_str()),
                ("1", "example")
            );
            assert_eq!(read.meter, (Quantity::SupplyVoltage, 15_000));
            let extra = filled.replacen(
                "[local.console]\n",
                "[local.console]\nbaud = \"115200\"\n",
                1,
            );
            let refused = settings(&pi5, &Instance::parse(&extra).unwrap()).unwrap_err();
            assert!(
                refused.starts_with("[local.console]: unknown field `baud`"),
                "{refused}"
            );
            // The identity record is the form of the identity template.
            let table: toml::Table = toml::from_str(&text).unwrap();
            assert_eq!(table["identity"], identity);
            tables.push(table);
        }
        // The example is the template with a GPIO loopback, which the
        // template shows as a comment.
        let kinds = |table: &toml::Table| {
            let devices = table["devices"].as_table().unwrap();
            Role::ALL.map(|role| devices[role.name()]["kind"].clone())
        };
        assert_eq!(kinds(&tables[0]), kinds(&tables[1]));
        let loopback = toml::Value::Table(toml::from_str(
            "from = \"board header pin 11, GPIO17\"\nto = \"board header pin 13, GPIO27, through 1 kOhm\"",
        ).unwrap());
        let connections =
            |table: &toml::Table| table["devices"]["connections"].as_array().unwrap().clone();
        let mut expected = connections(&tables[0]);
        expected.push(loopback);
        assert_eq!(connections(&tables[1]), expected);
        assert!(files[0].contains("\n# from = \"board header pin 11, GPIO17\"\n"));
    }

    #[test]
    fn a_bench_mixes_no_simulated_and_real_devices() {
        let simulated = [SIMULATED; 4];
        let all = format!("{POWER}{CONSOLE}{DELIVERY}");
        let model = ina228::MODELS[0].0;
        assert_eq!(real(&of_real(&all)), Ok(true));
        assert_eq!(real(&instance(simulated, "simulated", "", "")), Ok(false));
        for role in 0..4 {
            let (mut one, mut but_one) = (simulated, REAL);
            (one[role], but_one[role]) = (REAL[role], SIMULATED);
            for kinds in [one, but_one] {
                let mixed = instance(kinds, model, "current/", &all);
                assert_eq!(real(&mixed), Err(MIXED.into()), "{kinds:?}");
                let pi5 = board(RASPBERRY_PI_5);
                let opened = HostEnv.open_bench(&pi5, &mixed, Path::new("/"));
                assert_eq!(opened.err(), Some(MIXED.into()));
            }
        }
        // A kind of real device in another role is no kind of that role.
        let mut kinds = REAL;
        kinds.swap(0, 2);
        let error = "this build has no power device of the kind serial";
        let other = instance(kinds, model, "current/", &all);
        assert_eq!(real(&other), Err(error.into()));
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
        // A port that is a link also hides the path that it resolves to,
        // with the file name of that path and its twin.
        let name = format!("hidden-forms-{}", std::process::id());
        let dir = std::env::current_exe().unwrap().with_file_name(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        let target = dir.join("cu.usbserial-EF56GH");
        fs::write(&target, b"").unwrap();
        let link = dir.join("console-alias");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let hidden = Hidden::of(&[link.to_str().unwrap()]);
        let resolved = fs::canonicalize(&target).unwrap();
        for detail in [
            format!("open: {}: busy", resolved.display()),
            "read: CU.USBSERIAL-EF56GH went away".to_string(),
            "read: the adapter tty.usbserial-ef56gh went away".to_string(),
        ] {
            let error = device_error(Role::Console, &detail, &hidden);
            assert_eq!(error.detail, format!("{}: {withheld}", &detail[..4]));
        }
        fs::remove_dir_all(&dir).unwrap();
        // A form shorter than 4 bytes is not hidden, as the public version
        // of a receipt does not search for it.
        assert_eq!(Hidden::of(&["/a"]), Hidden::default());
    }
}
