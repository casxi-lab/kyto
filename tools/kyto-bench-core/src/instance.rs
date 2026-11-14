//! Instance configurations: the settings of one bench, kept outside the
//! repository (`docs/bench/formats.md`, instance configurations). The
//! canonical JSON of `[devices]` gives the bench hardware revision, so it
//! holds no address, relay channel or local path: those belong in
//! `[local.<role>]`, whose keys the driver of each kind reads.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::board::Board;
use crate::digest::{Sha256, is_lower_hex};
use crate::document::{Header, NOT_A_NAME, check, is_line_text, is_name};
use crate::identity::{IdentityError, IdentityRecord};
use crate::names::{self, PayloadName};
pub use crate::roles::{Quantity, Role};

/// The `format` of an instance configuration.
pub const FORMAT: &str = "kyto-bench instance";

/// The `format_version` of an instance configuration that this tool reads.
pub const FORMAT_VERSION: i64 = 1;

/// The shortest instance value that is searched for in recorded data; a
/// shorter value would match unrelated bytes.
pub const MIN_SEARCHED_LEN: usize = 4;

/// The number of hexadecimal digits of the salt in `[local]`: 128 bits.
pub const SALT_DIGITS: usize = 32;

const MAX_TIMEOUT_MS: u64 = 3_600_000;

/// Where the board finds its files over TFTP: instance values that a public
/// receipt never shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TftpParameters {
    /// The address of the TFTP server as the board's firmware prints it.
    pub server_address: String,
    /// The prefix of every path the board reads; it can be empty.
    pub path_prefix: String,
}

/// How readings of the supply confirm that it is on or off, in micro-units
/// of `quantity`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Confirmation {
    pub quantity: Quantity,
    /// A reading at or above this is on.
    pub on_at_least_micro: i64,
    /// A reading at or below this is off; below `on_at_least_micro`.
    pub off_at_most_micro: i64,
    /// The number of readings in a row that confirm a state.
    pub consecutive: u32,
    pub poll_ms: u64,
    pub on_timeout_ms: u64,
    pub off_timeout_ms: u64,
}

/// How long the console may take to be ready.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleParameters {
    pub ready_timeout_ms: u64,
}

/// A set of payload digests that the bench pins, which run plans name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pin {
    pub name: String,
    pub files: Vec<PinnedFile>,
}

/// One payload file of a pinned set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinnedFile {
    pub name: PayloadName,
    pub sha256: Sha256,
}

/// The parameters that receipts record, which verdicts and the checks before a run read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Parameters {
    pub tftp: TftpParameters,
    pub confirmation: Confirmation,
    pub console: ConsoleParameters,
    pub identity: IdentityRecord,
    pub pins: Vec<Pin>,
}

/// The device of one role: the kind of driver that runs it and its model.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    pub kind: String,
    pub model: String,
}

/// A connection of the bench: between devices, between a device and the
/// board or its supply, or between two points of the board, such as a
/// jumper.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    pub from: String,
    pub to: String,
}

/// The devices of a bench and their connections.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Devices {
    pub power: Device,
    pub power_confirmation: Device,
    pub console: Device,
    pub delivery: Device,
    pub connections: Vec<Connection>,
}

impl Devices {
    /// The device of `role`.
    pub fn device(&self, role: Role) -> &Device {
        match role {
            Role::Power => &self.power,
            Role::PowerConfirmation => &self.power_confirmation,
            Role::Console => &self.console,
            Role::Delivery => &self.delivery,
        }
    }

    /// Whether any device is of the kind [`SIMULATED`]; a run on such a
    /// bench is never evidence.
    pub fn any_simulated(&self) -> bool {
        Role::ALL
            .iter()
            .any(|role| self.device(*role).kind == SIMULATED)
    }
}

/// The kind of a simulated device.
pub const SIMULATED: &str = "simulated";

/// An instance value that a public receipt must not show, with its rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceValue {
    pub rule: String,
    pub value: String,
}

/// An instance configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    board: String,
    parameters: Parameters,
    devices: Devices,
    /// `[local]`: the salt and a table of each role's settings.
    local: toml::Table,
    hardware_revision: Sha256,
}

/// Why an instance configuration cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstanceError {
    /// The text is not TOML or does not have the form of the format.
    Syntax(String),
    /// The document is not an instance configuration of this version.
    Format(Header),
    /// A value breaks a rule of the format; `at` names the value.
    Invalid { at: String, reason: String },
    /// The identity record cannot be used.
    Identity(IdentityError),
    /// The configuration is for the board `instance`, not that of the
    /// description.
    Board { instance: String, board: String },
}

fn invalid(at: impl Into<String>, reason: impl Into<String>) -> InstanceError {
    InstanceError::Invalid {
        at: at.into(),
        reason: reason.into(),
    }
}

impl Instance {
    /// Reads and checks an instance configuration.
    pub fn parse(text: &str) -> Result<Self, InstanceError> {
        let syntax = |error: toml::de::Error| InstanceError::Syntax(error.to_string());
        let mut table: toml::Table = toml::from_str(text).map_err(syntax)?;
        let header = Header::take(&mut table).map_err(syntax)?;
        if !header.is(FORMAT, FORMAT_VERSION) {
            return Err(InstanceError::Format(header));
        }
        let raw: RawInstance = table.try_into().map_err(syntax)?;
        let devices: Devices = raw.devices.clone().try_into().map_err(syntax)?;
        raw.check(&devices)?;
        let identity = IdentityRecord::from_table(raw.identity).map_err(InstanceError::Identity)?;
        if identity.board() != raw.board {
            let reason = format!("the record is for {}, not {}", identity.board(), raw.board);
            return Err(invalid("identity.board", reason));
        }
        // The revision covers what `[devices]` holds, as written.
        let json = canonical_json(&raw.devices)
            .map_err(|error| invalid(format!("devices{}", error.at), error.to_string()))?;
        let parameters = Parameters {
            tftp: raw.tftp,
            confirmation: raw.confirmation,
            console: raw.console,
            identity,
            pins: raw.pins,
        };
        parameters.check_values()?;
        Ok(Self {
            board: raw.board,
            parameters,
            devices,
            local: raw.local,
            hardware_revision: Sha256::of(json.as_bytes()),
        })
    }

    /// The id of the board description the configuration is for.
    pub fn board(&self) -> &str {
        &self.board
    }

    /// The parts that verdicts and the checks before a run read.
    pub fn parameters(&self) -> &Parameters {
        &self.parameters
    }

    pub fn devices(&self) -> &Devices {
        &self.devices
    }

    /// This machine's settings of the device of `role`, which the driver of
    /// its kind reads.
    pub fn local(&self, role: Role) -> Option<&toml::Table> {
        self.local.get(role.name()).and_then(toml::Value::as_table)
    }

    /// The pinned set called `name`.
    pub fn pin(&self, name: &str) -> Option<&Pin> {
        self.parameters.pins.iter().find(|pin| pin.name == name)
    }

    /// The bench hardware revision: the SHA-256 of the canonical JSON of the
    /// `[devices]` table ([`canonical_json`]).
    pub fn hardware_revision(&self) -> &Sha256 {
        &self.hardware_revision
    }

    /// The instance values that a public receipt must not show, with their
    /// rules: the TFTP server address (`instance.tftp.server_address`), the
    /// path prefix unless empty (`instance.tftp.path_prefix`), the salt
    /// (`instance.local.salt`), and every string of at least
    /// [`MIN_SEARCHED_LEN`] bytes in the tables of `[local]`, nested ones too
    /// (`instance.local.<role>.<key>`, nested keys joined by `.`), in the
    /// order of the roles and of the keys.
    pub fn deidentify_values(&self) -> Vec<InstanceValue> {
        let tftp = &self.parameters.tftp;
        let mut values = Vec::new();
        for (key, value) in [
            ("server_address", &tftp.server_address),
            ("path_prefix", &tftp.path_prefix),
        ] {
            let value = toml::Value::from(value.as_str());
            collect(&format!("instance.tftp.{key}"), &value, &mut values);
        }
        if let Some(salt) = self.local.get("salt") {
            collect("instance.local.salt", salt, &mut values);
        }
        for role in Role::ALL {
            for (key, value) in self.local(role).into_iter().flatten() {
                collect(&format!("instance.local.{role}.{key}"), value, &mut values);
            }
        }
        values
    }

    /// Checks the configuration against the description of its board, which
    /// must accept the identity record and hold no value to hide.
    pub fn check_board(&self, board: &Board) -> Result<(), InstanceError> {
        if self.board != board.id {
            let (instance, board) = (self.board.clone(), board.id.clone());
            return Err(InstanceError::Board { instance, board });
        }
        self.check_text(board.text(), IN_BOARD)?;
        let identity = &self.parameters.identity;
        identity.check(board).map_err(InstanceError::Identity)
    }

    /// Checks that no value to hide occurs in `text`, ignoring ASCII case;
    /// the error at the value's key gives `reason`.
    pub fn check_text(&self, text: &str, reason: &str) -> Result<(), InstanceError> {
        let text = text.to_ascii_lowercase();
        for InstanceValue { rule, value } in self.deidentify_values() {
            let at = rule.trim_start_matches("instance.");
            check! { at => !text.contains(&value.to_ascii_lowercase()), reason; }
        }
        Ok(())
    }
}

/// Adds every string of `value` that can be searched for, under `rule`.
fn collect(rule: &str, value: &toml::Value, values: &mut Vec<InstanceValue>) {
    match value {
        toml::Value::String(text) if text.len() >= MIN_SEARCHED_LEN => {
            let (rule, value) = (rule.to_string(), text.clone());
            values.push(InstanceValue { rule, value });
        }
        toml::Value::Array(items) => items.iter().for_each(|item| collect(rule, item, values)),
        toml::Value::Table(table) => {
            for (key, item) in table {
                collect(&format!("{rule}.{key}"), item, values);
            }
        }
        _ => {}
    }
}

/// Whether `text` is 4 to 255 printable ASCII characters without spaces, so
/// that it can be searched for.
fn is_searchable(text: &str) -> bool {
    (MIN_SEARCHED_LEN..=255).contains(&text.len())
        && text.bytes().all(|byte| byte.is_ascii_graphic())
}

impl Parameters {
    /// Checks the rules of every value, as reading a configuration does, and
    /// the identity record against the description of the board.
    pub fn check(&self, board: &Board) -> Result<(), InstanceError> {
        self.check_values()?;
        self.identity.check(board).map_err(InstanceError::Identity)
    }

    /// Checks the rules of every value but those of the identity record.
    fn check_values(&self) -> Result<(), InstanceError> {
        let (tftp, conf, console) = (&self.tftp, &self.confirmation, &self.console);
        let prefix = &tftp.path_prefix;
        let timeout = |ms: u64| (1..=MAX_TIMEOUT_MS).contains(&ms);
        check! {
            "tftp.server_address" => is_searchable(&tftp.server_address), SEARCHABLE;
            "tftp.path_prefix" => prefix.is_empty() || is_searchable(prefix), SEARCHABLE;
            "confirmation.off_at_most_micro" => conf.off_at_most_micro < conf.on_at_least_micro,
                "the off threshold is below the on threshold";
            "confirmation.consecutive" => (1..=1000).contains(&conf.consecutive),
                "a confirmation takes 1 to 1000 readings";
            "confirmation.poll_ms" => (1..=60_000).contains(&conf.poll_ms),
                "readings are 1 ms to 1 min apart";
            "confirmation.on_timeout_ms" => timeout(conf.on_timeout_ms), TIMEOUT;
            "confirmation.off_timeout_ms" => timeout(conf.off_timeout_ms), TIMEOUT;
            "console.ready_timeout_ms" => timeout(console.ready_timeout_ms), TIMEOUT;
        }
        let mut seen = BTreeSet::new();
        for pin in &self.pins {
            let at = format!("pins.{}", pin.name);
            check! {
                &at => is_name(&pin.name), NOT_A_NAME;
                &at => seen.insert(&pin.name), "the name is used twice";
                &at => !pin.files.is_empty(), "a pinned set has at least one file";
            }
            names::check_distinct(pin.files.iter().map(|file| &file.name))
                .map_err(|error| invalid(&at, error.to_string()))?;
        }
        Ok(())
    }
}

impl RawInstance {
    /// Checks the rules of `[local]` and `[devices]`.
    fn check(&self, devices: &Devices) -> Result<(), InstanceError> {
        for (key, settings) in self.local.iter().filter(|(key, _)| *key != "salt") {
            if !Role::ALL.iter().any(|role| role.name() == key) || !settings.is_table() {
                let message = format!("[local.{key}] is not the settings table of a role");
                return Err(InstanceError::Syntax(message));
            }
        }
        for role in Role::ALL {
            let device = devices.device(role);
            check! {
                format!("devices.{role}.kind") => is_name(&device.kind), NOT_A_NAME;
                format!("devices.{role}.model") => is_line_text(&device.model), "a line of text";
            }
        }
        for (index, connection) in devices.connections.iter().enumerate() {
            check! {
                format!("devices.connections[{index}]") =>
                    is_line_text(&connection.from) && is_line_text(&connection.to),
                    "both ends of a connection are lines of text";
            }
        }
        let salt = self.local.get("salt").and_then(toml::Value::as_str);
        let real = Role::ALL.map(|role| devices.device(role).kind != SIMULATED);
        check! {
            "local.salt" => salt.is_some_and(|salt| is_lower_hex(salt, SALT_DIGITS)),
                "the salt is 32 lowercase hexadecimal digits, 128 random bits";
            "local.salt" => !real.contains(&true) || salt.is_none_or(|salt| !PUBLISHED_SALTS.contains(&salt)),
                "kyto publishes this salt; generate one with openssl rand -hex 16";
        }
        Ok(())
    }
}

/// The salts that kyto publishes, in its documentation, tests and example
/// configuration, which only simulated devices may use.
const PUBLISHED_SALTS: [&str; 3] = [
    "9c1f5e0a7b3d48e2a6c0f4b8d2e71a35",
    "6b2d9f41c07e8a35d4e19b7c2f60a853",
    "d5bedecc5ee387d76097a2fa96272358",
];

const SEARCHABLE: &str = "the value is 4 to 255 printable ASCII characters without spaces";
const TIMEOUT: &str = "the timeout is 1 ms to 1 h";
const IN_BOARD: &str = "the board description holds this value to hide; choose another";

/// A value that canonical JSON does not hold: `found`, a float or a date or
/// time, `at` a path of `.key` and `[index]` steps from the top.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalError {
    pub at: String,
    pub found: &'static str,
}

impl fmt::Display for CanonicalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let found = self.found;
        write!(f, "{found} has no canonical form; use a string or integer")
    }
}

impl std::error::Error for CanonicalError {}

/// The canonical JSON of a TOML value (`docs/bench/formats.md`, devices):
/// compact UTF-8, the keys of every object in increasing order of their
/// UTF-8 bytes, and strings escaped as `serde_json` escapes them. Floats,
/// dates and times are refused.
pub fn canonical_json(value: &toml::Value) -> Result<String, CanonicalError> {
    Ok(to_json(value, "")?.to_string())
}

fn to_json(value: &toml::Value, at: &str) -> Result<serde_json::Value, CanonicalError> {
    let refused = |found| CanonicalError {
        at: at.to_string(),
        found,
    };
    Ok(match value {
        toml::Value::String(text) => text.as_str().into(),
        toml::Value::Integer(number) => (*number).into(),
        toml::Value::Boolean(flag) => (*flag).into(),
        toml::Value::Float(_) => return Err(refused("a float")),
        toml::Value::Datetime(_) => return Err(refused("a date or time")),
        toml::Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(index, item)| to_json(item, &format!("{at}[{index}]")))
            .collect::<Result<_, _>>()?,
        toml::Value::Table(table) => {
            let mut keys: Vec<&String> = table.keys().collect();
            keys.sort();
            let object = keys.into_iter().map(|key| {
                let value = to_json(&table[key], &format!("{at}.{key}"))?;
                Ok((key.clone(), value))
            });
            serde_json::Value::Object(object.collect::<Result<_, _>>()?)
        }
    })
}

impl fmt::Display for InstanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(message) => write!(f, "cannot read the instance configuration: {message}"),
            Self::Format(found) => write!(f, "{found} is not \"{FORMAT}\" version 1"),
            Self::Invalid { at, reason } => write!(f, "instance configuration at {at}: {reason}"),
            Self::Identity(error) => write!(f, "in [identity]: {error}"),
            Self::Board { instance, board } => write!(f, "configured for {instance}, not {board}"),
        }
    }
}

impl std::error::Error for InstanceError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInstance {
    board: String,
    tftp: TftpParameters,
    confirmation: Confirmation,
    console: ConsoleParameters,
    identity: toml::Table,
    #[serde(default)]
    pins: Vec<Pin>,
    devices: toml::Value,
    local: toml::Table,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::RASPBERRY_PI_5;

    /// An instance configuration of a bench with simulated devices, in
    /// documentation values: the TFTP server address 192.0.2.10, an empty
    /// path prefix, a salt, and console captures named by relative paths.
    const EXAMPLE: &str = r#"format = "kyto-bench instance"
format_version = 1
board = "raspberry-pi-5"

[tftp]
server_address = "192.0.2.10"
path_prefix = ""

[confirmation]
quantity = "supply_voltage"
on_at_least_micro = 4750000
off_at_most_micro = 500000
consecutive = 1
poll_ms = 100
on_timeout_ms = 3000
off_timeout_ms = 3000

[console]
ready_timeout_ms = 2000

[identity]
format = "kyto-bench board identity"
format_version = 1
board = "raspberry-pi-5"
revision_code = { value = "d04171", read = "firmware_log" }
pcb_version = { value = "4", read = "visual" }
soc_stepping = { value = "D0", read = "firmware_log_presence" }
memory.vendor = { value = "unknown", read = "visual" }
memory.part = { value = "unknown", read = "visual" }
ethernet_phy = { value = "unknown", read = "visual" }
spi_flash.jedec_id = { value = "ef4015", read = "firmware_log" }
spi_flash.part = { value = "unknown", read = "visual" }
eeprom_version = { value = "2026/09/25", read = "firmware_version" }
manufacturing_version = { value = "unknown", read = "firmware_log" }
min_boot_version = { value = "unknown", read = "software" }
boot_files = { value = ["bcm2712-rpi-5-b.dtb", "overlays/bcm2712d0.dtbo"], read = "tftp_reads" }

[[pins]]
name = "self-test"
files = [{ name = "config.txt", sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" }]

[devices]
power = { kind = "simulated", model = "simulated" }
power_confirmation = { kind = "simulated", model = "simulated" }
console = { kind = "simulated", model = "simulated" }
delivery = { kind = "simulated", model = "simulated" }

[[devices.connections]]
from = "console"
to = "board debug UART"

[local]
salt = "6b2d9f41c07e8a35d4e19b7c2f60a853"

[local.console]
replay = ["captures/firmware-only.bin", "captures/echo.bin"]
"#;

    /// The documentation values that console fixtures carry in place of a
    /// board's serial number, MAC addresses and IP addresses.
    const FIXTURE_VALUES: [&str; 7] = [
        "1234abcd",
        "00:00:5e:00:53:01",
        "00:00:5e:00:53:02",
        "00:00:5e:00:53:03",
        "192.0.2.10",
        "192.0.2.20",
        "192.0.2.1",
    ];

    fn example() -> Instance {
        Instance::parse(EXAMPLE).unwrap()
    }

    /// The example with `from`, which occurs exactly once, replaced by `to`.
    fn edited(from: &str, to: &str) -> String {
        assert_eq!(
            EXAMPLE.matches(from).count(),
            1,
            "the example has {from:?} once"
        );
        EXAMPLE.replacen(from, to, 1)
    }

    fn at_of(text: &str) -> String {
        match Instance::parse(text) {
            Err(InstanceError::Invalid { at, .. }) => at,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_example_instance_parses() {
        let instance = example();
        assert_eq!(instance.board(), "raspberry-pi-5");
        let parameters = instance.parameters();
        assert_eq!(parameters.tftp.server_address, "192.0.2.10");
        assert_eq!(parameters.tftp.path_prefix, "");
        assert_eq!(parameters.confirmation.quantity, Quantity::SupplyVoltage);
        assert_eq!(parameters.console.ready_timeout_ms, 2000);
        assert_eq!(parameters.identity.board(), "raspberry-pi-5");
        assert_eq!(instance.pin("self-test").unwrap().files.len(), 1);
        assert_eq!(instance.pin("other"), None);
        for role in Role::ALL {
            assert_eq!(instance.devices().device(role).kind, "simulated");
        }
        assert!(instance.local(Role::Console).is_some());
        assert!(instance.local(Role::Power).is_none());
        let board = Board::parse(RASPBERRY_PI_5).unwrap();
        instance.check_board(&board).unwrap();
    }

    #[test]
    fn the_hardware_revision_is_the_digest_of_the_canonical_devices_table() {
        let canonical = concat!(
            r#"{"connections":[{"from":"console","to":"board debug UART"}],"#,
            r#""console":{"kind":"simulated","model":"simulated"},"#,
            r#""delivery":{"kind":"simulated","model":"simulated"},"#,
            r#""power":{"kind":"simulated","model":"simulated"},"#,
            r#""power_confirmation":{"kind":"simulated","model":"simulated"}}"#
        );
        assert_eq!(
            example().hardware_revision(),
            &Sha256::of(canonical.as_bytes())
        );
    }

    #[test]
    fn hardware_revision_ignores_comments_key_order_and_local_settings() {
        let revision = example().hardware_revision().clone();
        let same = [
            edited(
                "[devices]\n",
                "# The devices of this bench.\n[devices] # simulated\n",
            ),
            edited(
                "power = { kind = \"simulated\", model = \"simulated\" }",
                "power = { model = \"simulated\", kind = \"simulated\" }",
            ),
            edited(
                "replay = [\"captures/firmware-only.bin\", \"captures/echo.bin\"]",
                "replay = [\"captures/other.bin\"]\nport = \"/dev/example\"",
            ),
            edited(
                "pcb_version = { value = \"4\", read = \"visual\" }",
                "pcb_version = { value = \"7\", read = \"visual\" }",
            ),
            edited(
                "server_address = \"192.0.2.10\"",
                "server_address = \"192.0.2.20\"",
            ),
        ];
        for text in same {
            assert_eq!(
                Instance::parse(&text).unwrap().hardware_revision(),
                &revision,
                "{text}"
            );
        }
        let different = [
            edited(
                "delivery = { kind = \"simulated\", model = \"simulated\" }",
                "delivery = { kind = \"simulated\", model = \"another model\" }",
            ),
            edited("to = \"board debug UART\"", "to = \"board GPIO header\""),
            edited(
                "to = \"board debug UART\"\n",
                "to = \"board debug UART\"\n\n[[devices.connections]]\nfrom = \"power\"\nto = \"board supply\"\n",
            ),
        ];
        for text in different {
            assert_ne!(
                Instance::parse(&text).unwrap().hardware_revision(),
                &revision,
                "{text}"
            );
        }
    }

    #[test]
    fn canonical_json_sorts_keys_and_escapes_strings() {
        let value: toml::Value = toml::from_str(
            r#"b = 1
a = [true, -2, "x"]
B = { z = "", y = "q\"b\\c\n\t\u0001é/" }
"#,
        )
        .map(toml::Value::Table)
        .unwrap();
        assert_eq!(
            canonical_json(&value).unwrap(),
            r#"{"B":{"y":"q\"b\\c\n\t\u0001é/","z":""},"a":[true,-2,"x"],"b":1}"#
        );
        let float: toml::Value = toml::from_str("x = [1, { y = 1.5 }]")
            .map(toml::Value::Table)
            .unwrap();
        assert_eq!(
            canonical_json(&float),
            Err(CanonicalError {
                at: ".x[1].y".to_string(),
                found: "a float"
            })
        );
        let date: toml::Value = toml::from_str("d = 2026-11-12")
            .map(toml::Value::Table)
            .unwrap();
        assert_eq!(canonical_json(&date).unwrap_err().found, "a date or time");
    }

    #[test]
    fn instance_values_are_the_tftp_parameters_and_local_strings() {
        let values = example().deidentify_values();
        let pairs: Vec<(&str, &str)> = values
            .iter()
            .map(|value| (value.rule.as_str(), value.value.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("instance.tftp.server_address", "192.0.2.10"),
                ("instance.local.salt", "6b2d9f41c07e8a35d4e19b7c2f60a853"),
                (
                    "instance.local.console.replay",
                    "captures/firmware-only.bin"
                ),
                ("instance.local.console.replay", "captures/echo.bin"),
            ]
        );
        let text = edited("path_prefix = \"\"", "path_prefix = \"bench/\"");
        let text = text.replace(
            "[local.console]\n",
            "[local.power]\nport = \"/dev/example\"\nrelay = 3\nunit = \"abc\"\nbus = { name = \"bus-a\", id = 7 }\n\n[local.console]\n",
        );
        let values = Instance::parse(&text).unwrap().deidentify_values();
        let pairs: Vec<(&str, &str)> = values
            .iter()
            .map(|value| (value.rule.as_str(), value.value.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("instance.tftp.server_address", "192.0.2.10"),
                ("instance.tftp.path_prefix", "bench/"),
                ("instance.local.salt", "6b2d9f41c07e8a35d4e19b7c2f60a853"),
                ("instance.local.power.bus.name", "bus-a"),
                ("instance.local.power.port", "/dev/example"),
                (
                    "instance.local.console.replay",
                    "captures/firmware-only.bin"
                ),
                ("instance.local.console.replay", "captures/echo.bin"),
            ]
        );
    }

    #[test]
    fn the_raspberry_pi_5_description_holds_no_value_of_the_example_instance() {
        // Receipts record the description verbatim, and a public receipt
        // must not show an instance value anywhere.
        for value in example().deidentify_values() {
            assert!(
                !RASPBERRY_PI_5.contains(&value.value),
                "{}: {}",
                value.rule,
                value.value
            );
        }
        for value in FIXTURE_VALUES {
            assert!(!RASPBERRY_PI_5.contains(value), "{value}");
        }
    }

    /// The page that documents the formats, whose example instance
    /// configuration a bench may copy.
    const FORMATS_PAGE: &str = include_str!("../../../docs/bench/formats.md");

    /// The example instance configuration of the formats page.
    fn documented_example() -> &'static str {
        let opening = "```toml\nformat = \"kyto-bench instance\"\n";
        let start = FORMATS_PAGE
            .find(opening)
            .expect("the formats page has an example instance")
            + "```toml\n".len();
        let rest = &FORMATS_PAGE[start..];
        &rest[..rest.find("```").expect("the example ends")]
    }

    #[test]
    fn the_documented_example_holds_no_value_of_a_built_in_description() {
        let instance = Instance::parse(documented_example()).unwrap();
        let board = Board::parse(RASPBERRY_PI_5).unwrap();
        instance.check_board(&board).unwrap();
        // The page states the bench hardware revision of its example.
        let revision = instance.hardware_revision().to_string();
        assert!(
            FORMATS_PAGE.contains(&format!("`{revision}`")),
            "{revision}"
        );
        // A bench that copies the example has none of its values in a
        // description that receipts record.
        let values = instance.deidentify_values();
        assert!(!values.is_empty());
        for value in values {
            for description in crate::board::EMBEDDED {
                assert!(
                    !description.contains(&value.value),
                    "{}: {}",
                    value.rule,
                    value.value
                );
            }
        }
    }

    #[test]
    fn tftp_parameters_can_be_searched_for() {
        for prefix in ["bench/", "a/b/", ""] {
            let text = edited("path_prefix = \"\"", &format!("path_prefix = \"{prefix}\""));
            assert!(Instance::parse(&text).is_ok(), "{prefix:?}");
        }
        for prefix in ["ab/", "a b/", "bench\\t/", "caf\u{e9}/"] {
            let text = edited("path_prefix = \"\"", &format!("path_prefix = \"{prefix}\""));
            assert_eq!(at_of(&text), "tftp.path_prefix", "{prefix:?}");
        }
        for address in ["", "1.2", "192.0.2.10 "] {
            let text = edited(
                "server_address = \"192.0.2.10\"",
                &format!("server_address = \"{address}\""),
            );
            assert_eq!(at_of(&text), "tftp.server_address", "{address:?}");
        }
    }

    #[test]
    fn thresholds_and_timeouts_are_bounded() {
        let cases = [
            (
                "off_at_most_micro = 500000",
                "off_at_most_micro = 4750000",
                "confirmation.off_at_most_micro",
            ),
            (
                "consecutive = 1",
                "consecutive = 0",
                "confirmation.consecutive",
            ),
            (
                "consecutive = 1",
                "consecutive = 1001",
                "confirmation.consecutive",
            ),
            ("poll_ms = 100", "poll_ms = 0", "confirmation.poll_ms"),
            (
                "on_timeout_ms = 3000",
                "on_timeout_ms = 3600001",
                "confirmation.on_timeout_ms",
            ),
            (
                "off_timeout_ms = 3000",
                "off_timeout_ms = 0",
                "confirmation.off_timeout_ms",
            ),
            (
                "ready_timeout_ms = 2000",
                "ready_timeout_ms = 0",
                "console.ready_timeout_ms",
            ),
        ];
        for (from, to, at) in cases {
            assert_eq!(at_of(&edited(from, to)), at, "{to}");
        }
        let negative = edited("off_at_most_micro = 500000", "off_at_most_micro = -20000");
        assert!(Instance::parse(&negative).is_ok());
    }

    #[test]
    fn an_instance_without_a_128_bit_salt_is_rejected() {
        let salt = "salt = \"6b2d9f41c07e8a35d4e19b7c2f60a853\"";
        let refused = [
            edited(salt, ""),
            edited(salt, "salt = \"6b2d9f41c07e8a35d4e19b7c2f60a8\""),
            edited(salt, "salt = \"6b2d9f41c07e8a35d4e19b7c2f60a8530\""),
            edited(salt, "salt = \"6B2D9F41C07E8A35D4E19B7C2F60A853\""),
            edited(salt, "salt = \"6b2d9f41c07e8a35d4e19b7c2f60a85g\""),
            edited(salt, "salt = 128"),
            edited(
                salt,
                "salt = { value = \"6b2d9f41c07e8a35d4e19b7c2f60a853\" }",
            ),
        ];
        for text in refused {
            assert_eq!(at_of(&text), "local.salt", "{text}");
        }
        // [local] made only of the settings of the console, and no [local].
        let start = EXAMPLE.find("[local]\n").unwrap();
        let end = EXAMPLE.find("[local.console]\n").unwrap();
        let without = format!("{}{}", &EXAMPLE[..start], &EXAMPLE[end..]);
        assert_eq!(at_of(&without), "local.salt");
        let no_local = &EXAMPLE[..start];
        let error = Instance::parse(no_local).unwrap_err().to_string();
        assert!(error.contains("missing field `local`"), "{error}");
    }

    #[test]
    fn a_salt_that_kyto_publishes_is_only_for_simulated_devices() {
        let power = "power = { kind = \"simulated\", model = \"simulated\" }";
        let real = edited(power, "power = { kind = \"relay-board\", model = \"R4\" }");
        assert_eq!(at_of(&real), "local.salt");
        let documented = real.replace(
            "6b2d9f41c07e8a35d4e19b7c2f60a853",
            "9c1f5e0a7b3d48e2a6c0f4b8d2e71a35",
        );
        assert_eq!(at_of(&documented), "local.salt");
        // A salt of the bench's own, made here so that no text holds it.
        let salt = Sha256::of(b"a bench").to_string();
        let own = real.replace("6b2d9f41c07e8a35d4e19b7c2f60a853", &salt[..32]);
        assert_eq!(
            Instance::parse(&own).unwrap().devices().power.kind,
            "relay-board"
        );
        // The documented example, of simulated devices, keeps its salt.
        assert!(Instance::parse(documented_example()).is_ok());
    }

    #[test]
    fn an_instance_that_breaks_a_rule_is_refused() {
        let syntax = [
            edited("[local.console]", "[local.relay]"),
            edited(
                "board = \"raspberry-pi-5\"\n\n[tftp]",
                "board = \"raspberry-pi-5\"\nsalt = 1\n\n[tftp]",
            ),
            edited("to = \"board debug UART\"\n", ""),
            edited(
                "delivery = { kind = \"simulated\", model = \"simulated\" }\n",
                "",
            ),
            edited(
                "power = { kind = \"simulated\", model = \"simulated\" }",
                "power = { kind = \"simulated\" }",
            ),
            edited(
                "quantity = \"supply_voltage\"",
                "quantity = \"temperature\"",
            ),
            edited("sha256 = \"e3b0", "sha256 = \"E3B0"),
            edited("name = \"config.txt\"", "name = \"../config.txt\""),
        ];
        for text in syntax {
            assert!(
                matches!(Instance::parse(&text), Err(InstanceError::Syntax(_))),
                "{text}"
            );
        }
        let cases = [
            (
                "files = [{ name = \"config.txt\", sha256 = \"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\" }]",
                "files = []",
                "pins.self-test",
            ),
            (
                "name = \"self-test\"",
                "name = \"Self Test\"",
                "pins.Self Test",
            ),
            (
                "console = { kind = \"simulated\", model = \"simulated\" }",
                "console = { kind = \"Simulated\", model = \"simulated\" }",
                "devices.console.kind",
            ),
            (
                "console = { kind = \"simulated\", model = \"simulated\" }",
                "console = { kind = \"simulated\", model = \"\" }",
                "devices.console.model",
            ),
            (
                "from = \"console\"",
                "from = \" console\"",
                "devices.connections[0]",
            ),
        ];
        for (from, to, at) in cases {
            assert_eq!(at_of(&edited(from, to)), at, "{to}");
        }
        let twice = edited(
            "\n[devices]\n",
            "\n[[pins]]\nname = \"self-test\"\nfiles = [{ name = \"a.txt\", sha256 = \"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\" }]\n\n[devices]\n",
        );
        assert_eq!(at_of(&twice), "pins.self-test");
    }

    #[test]
    fn the_identity_record_belongs_to_the_board_of_the_instance() {
        let other = edited(
            "board = \"raspberry-pi-5\"\nrevision_code",
            "board = \"another-board\"\nrevision_code",
        );
        assert_eq!(at_of(&other), "identity.board");
        let field = edited(
            "ethernet_phy = { value = \"unknown\", read = \"visual\" }",
            "ethernet_phy = { value = 3, read = \"visual\" }",
        );
        assert!(matches!(
            Instance::parse(&field),
            Err(InstanceError::Identity(IdentityError::Value { .. }))
        ));
        let read = edited(
            "ethernet_phy = { value = \"unknown\", read = \"visual\" }",
            "ethernet_phy = { value = \"unknown\", read = \"software\" }",
        );
        let board = Board::parse(RASPBERRY_PI_5).unwrap();
        assert!(matches!(
            Instance::parse(&read).unwrap().check_board(&board),
            Err(InstanceError::Identity(IdentityError::Read { .. }))
        ));
    }

    #[test]
    fn a_value_to_hide_that_the_board_description_holds_is_refused() {
        // Receipts record the description verbatim, so such a value would
        // leave no public version of any run of the bench.
        let board = Board::parse(RASPBERRY_PI_5).unwrap();
        example().check_board(&board).unwrap();
        let prefix = "path_prefix = \"\"";
        let console = "[local.console]\n";
        for (from, to, at) in [
            (prefix, "path_prefix = \"bench/\"", "tftp.path_prefix"),
            (prefix, "path_prefix = \"BENCH/\"", "tftp.path_prefix"),
            (
                console,
                "[local.console]\nbaud = \"115200\"\n",
                "local.console.baud",
            ),
        ] {
            let instance = Instance::parse(&edited(from, to)).unwrap();
            assert_eq!(instance.check_board(&board), Err(invalid(at, IN_BOARD)));
        }
    }

    /// The example instance configuration, for simulated devices.
    const EXAMPLE_INSTANCE: &str = include_str!("../../examples/instance.toml");

    /// Every salt that kyto writes, in its documentation, its tests and its
    /// example configuration, is one that only simulated devices may use,
    /// and none is in what a run reads or in console output, where a public
    /// version would have to hide it.
    #[test]
    fn every_salt_that_kyto_writes_is_published_and_found_in_no_example() {
        let tests = [
            include_str!("instance.rs"),
            include_str!("receipt.rs"),
            include_str!("public.rs"),
            include_str!("observe.rs"),
            include_str!("plan.rs"),
            include_str!("payload.rs"),
        ];
        let mut written = BTreeSet::new();
        for text in tests.iter().chain([&FORMATS_PAGE, &EXAMPLE_INSTANCE]) {
            for prefix in ["salt = \"", "salt = \\\""] {
                for (at, _) in text.match_indices(prefix) {
                    let start = at + prefix.len();
                    if let Some(salt) = text.get(start..start + SALT_DIGITS)
                        && is_lower_hex(salt, SALT_DIGITS)
                    {
                        written.insert(salt);
                    }
                }
            }
        }
        assert_eq!(written, BTreeSet::from(PUBLISHED_SALTS));
        let read = [
            include_str!("../../examples/run-plan.toml"),
            include_str!("../../examples/payload/firmware-only/config.txt"),
            include_str!("../../examples/payload/firmware-only/bcm2712-rpi-5-b.dtb"),
            include_str!("../../examples/payload/firmware-only/overlays/bcm2712d0.dtbo"),
            include_str!("../../examples/payload/firmware-only/kernel.img"),
            include_str!("../../examples/payload/echo/config.txt"),
            include_str!("../../examples/payload/echo/bcm2712-rpi-5-b.dtb"),
            include_str!("../../examples/payload/echo/overlays/bcm2712d0.dtbo"),
            include_str!("../../examples/payload/echo/kernel.img"),
            RASPBERRY_PI_5,
        ];
        // The documented instance values, the salts among them.
        let documented = [example(), Instance::parse(documented_example()).unwrap()];
        let example_instance = Instance::parse(EXAMPLE_INSTANCE).unwrap();
        example_instance
            .check_board(&Board::parse(RASPBERRY_PI_5).unwrap())
            .unwrap();
        let documented = [&documented[..], &[example_instance]].concat();
        let values: Vec<String> = documented
            .iter()
            .flat_map(Instance::deidentify_values)
            .map(|value| value.value.to_ascii_lowercase())
            .collect();
        for text in read.map(str::to_ascii_lowercase) {
            for value in &values {
                assert!(!text.contains(value.as_str()), "{value}");
            }
        }
        // The console output that the tests build and the example captures
        // hold the documented TFTP server on purpose, but no salt, and the
        // captures no other instance value.
        for text in tests[1..].iter().map(|text| text.to_ascii_lowercase()) {
            for salt in PUBLISHED_SALTS {
                assert!(!text.contains(salt), "{salt}");
            }
        }
        let captures = [
            &include_bytes!("../../examples/captures/firmware-only.bin")[..],
            include_bytes!("../../examples/captures/echo.bin"),
        ];
        let address = "instance.tftp.server_address";
        let hidden = documented.iter().flat_map(Instance::deidentify_values);
        let hidden: Vec<String> = hidden
            .filter(|value| value.rule != address)
            .map(|value| value.value.to_ascii_lowercase())
            .chain(PUBLISHED_SALTS.map(String::from))
            .collect();
        for capture in captures.map(<[u8]>::to_ascii_lowercase) {
            for value in &hidden {
                let found = capture
                    .windows(value.len())
                    .any(|bytes| bytes == value.as_bytes());
                assert!(!found, "{value}");
            }
        }
    }

    #[test]
    fn an_instance_for_another_board_is_refused() {
        let text = EXAMPLE.replace("board = \"raspberry-pi-5\"", "board = \"synthetic-board\"");
        let instance = Instance::parse(&text).unwrap();
        let board = Board::parse(RASPBERRY_PI_5).unwrap();
        assert_eq!(
            instance.check_board(&board),
            Err(InstanceError::Board {
                instance: "synthetic-board".to_string(),
                board: "raspberry-pi-5".to_string()
            })
        );
        let version = edited(
            "format = \"kyto-bench instance\"\nformat_version = 1",
            "format = \"kyto-bench instance\"\nformat_version = 2",
        );
        assert_eq!(
            Instance::parse(&version),
            Err(InstanceError::Format(Header {
                format: FORMAT.to_string(),
                format_version: 2
            }))
        );
    }
}
