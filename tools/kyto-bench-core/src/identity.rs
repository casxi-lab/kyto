//! Identity records: what is known about one board under test.
//!
//! A board description says how each of ten identity fields can be read. An
//! identity record gives the values of one board, each with the way it was
//! read; a value that cannot be read is `unknown`, never an estimate. The
//! record is the `[identity]` table of a bench's instance configuration;
//! `tools/templates/identity.toml` is its blank form. The board's serial
//! number is not part of it.

use std::fmt;

use serde::Deserialize;
use serde::de::{self, DeserializeOwned, Deserializer};

use crate::board::{Board, Method};
use crate::document::{Header, is_line_text, named_enum};
use crate::names::{self, PayloadName};

/// The `format` of an identity record.
pub const FORMAT: &str = "kyto-bench board identity";

/// The `format_version` of an identity record that this tool reads.
pub const FORMAT_VERSION: i64 = 1;

/// The value that stands for a value that could not be read.
pub const UNKNOWN: &str = "unknown";

named_enum! {
    /// An identity field.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum Field {
        /// The board revision code.
        RevisionCode = "revision_code",
        /// The PCB revision, as a number.
        PcbVersion = "pcb_version",
        /// The stepping of the system on chip.
        SocStepping = "soc_stepping",
        /// The memory package: `vendor` and `part`.
        Memory = "memory",
        /// The Ethernet PHY.
        EthernetPhy = "ethernet_phy",
        /// The SPI flash that holds the bootloader: `jedec_id` and `part`.
        SpiFlash = "spi_flash",
        /// The bootloader release in the EEPROM.
        EepromVersion = "eeprom_version",
        /// The board's manufacturing version, as the bootloader reports it.
        ManufacturingVersion = "manufacturing_version",
        /// The same number, as software on the board reads it.
        MinBootVersion = "min_boot_version",
        /// The device tree files the board reads at boot.
        BootFiles = "boot_files",
    }
}

/// The kind of value a field holds: a line of text, such as a part number,
/// a whole number of at least 1, or a list of payload names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Number,
    Files,
}

impl Field {
    /// The names of the field's parts; empty for a field with one value.
    pub fn parts(self) -> &'static [&'static str] {
        match self {
            Field::Memory => &["vendor", "part"],
            Field::SpiFlash => &["jedec_id", "part"],
            _ => &[],
        }
    }

    /// The kind of value the field holds.
    pub fn kind(self) -> Kind {
        match self {
            Field::ManufacturingVersion | Field::MinBootVersion => Kind::Number,
            Field::BootFiles => Kind::Files,
            _ => Kind::Text,
        }
    }

    /// The values of the field: the field itself, or each of its parts.
    pub fn values(self) -> Vec<FieldRef> {
        let part = |part| FieldRef { field: self, part };
        match self.parts() {
            [] => vec![part(None)],
            parts => parts.iter().map(|name| part(Some(*name))).collect(),
        }
    }
}

/// One value of an identity record: a field, or one part of a field that has
/// parts. It is written `field` or `field.part`, such as `memory.part`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldRef {
    pub field: Field,
    /// The part, one of [`Field::parts`]; `None` for a field with one value.
    pub part: Option<&'static str>,
}

impl FieldRef {
    /// The value of `field`, a field with one value.
    pub fn whole(field: Field) -> Self {
        Self { field, part: None }
    }

    /// Reads `field` or `field.part`; the part must be one the field has, and
    /// a field with parts must name one.
    pub fn parse(text: &str) -> Option<FieldRef> {
        let (name, part) = match text.split_once('.') {
            Some((name, part)) => (name, Some(part)),
            None => (text, None),
        };
        let field = Field::ALL.into_iter().find(|field| field.name() == name)?;
        field.values().into_iter().find(|value| value.part == part)
    }

    /// The name of the part as receipts write it: the part, or `value` for a
    /// field with one value.
    pub fn part_name(self) -> &'static str {
        self.part.unwrap_or("value")
    }
}

impl<'de> Deserialize<'de> for FieldRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let unknown = || de::Error::custom(format!("{text:?} is not an identity field or part"));
        FieldRef::parse(&text).ok_or_else(unknown)
    }
}

impl fmt::Display for FieldRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.part {
            Some(part) => write!(f, "{}.{part}", self.field),
            None => write!(f, "{}", self.field),
        }
    }
}

named_enum! {
    /// A way of reading an identity value.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ReadMethod {
        /// A group of a line of the firmware log.
        FirmwareLog = "firmware_log",
        /// The presence of a line of the firmware log, which stands for one
        /// value; its absence leaves the value unknown.
        FirmwareLogPresence = "firmware_log_presence",
        /// A group of a firmware version line.
        FirmwareVersion = "firmware_version",
        /// The files the board reads over TFTP.
        TftpReads = "tftp_reads",
        /// Looking at the board.
        Visual = "visual",
        /// Software running on the board.
        Software = "software",
    }
}

impl ReadMethod {
    /// Whether this way of reading can give a value of `kind`.
    pub fn reads(self, kind: Kind) -> bool {
        match self {
            ReadMethod::FirmwareLogPresence => kind == Kind::Text,
            ReadMethod::TftpReads => kind == Kind::Files,
            _ => kind != Kind::Files,
        }
    }
}

/// A value of an identity record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// The value could not be read.
    Unknown,
    /// A line of text.
    Text(String),
    /// A whole number of at least 1.
    Number(u32),
    /// At least one payload name; one directory can hold them as files.
    Files(Vec<PayloadName>),
}

/// One value of an identity record with the way it was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub field: FieldRef,
    pub value: Value,
    pub read: ReadMethod,
}

/// The identity record of one board.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityRecord {
    board: String,
    entries: Vec<Entry>,
}

/// Why an identity record cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdentityError {
    /// The text is not TOML, or a key or value does not have the form of the
    /// format.
    Syntax(String),
    /// The document is not an identity record of this version.
    Format(Header),
    /// A value has a form its field does not take.
    Value { field: FieldRef, reason: String },
    /// The record is for the board `record`, not that of the description.
    Board { record: String, board: String },
    /// A value is read in a way that the board description does not list
    /// for it.
    Read { field: FieldRef, read: ReadMethod },
    /// A value read from the presence of a line is not the value that the
    /// line stands for.
    Presence { field: FieldRef, value: String },
    /// A value read from the TFTP reads names a file that no such way of
    /// reading lists for it.
    TftpReads { field: FieldRef, file: PayloadName },
    /// `manufacturing_version` and `min_boot_version` are both known and
    /// differ, though both are the board's manufacturing version.
    ManufacturingVersions {
        manufacturing_version: u32,
        min_boot_version: u32,
    },
}

fn syntax(message: impl fmt::Display) -> IdentityError {
    IdentityError::Syntax(message.to_string())
}

impl IdentityRecord {
    /// Reads an identity record from the text of its own file.
    pub fn parse(text: &str) -> Result<Self, IdentityError> {
        Self::from_table(toml::from_str(text).map_err(syntax)?)
    }

    /// Reads an identity record from a table, such as the `[identity]` table
    /// of an instance configuration.
    pub fn from_table(mut table: toml::Table) -> Result<Self, IdentityError> {
        let header = Header::take(&mut table).map_err(syntax)?;
        if !header.is(FORMAT, FORMAT_VERSION) {
            return Err(IdentityError::Format(header));
        }
        let board = take(&mut table, "board")?;
        let mut entries = Vec::new();
        for field in Field::ALL {
            if field.parts().is_empty() {
                let reading = take(&mut table, field.name())?;
                entries.push(entry(FieldRef::whole(field), reading)?);
                continue;
            }
            let mut parts: toml::Table = take(&mut table, field.name())?;
            for value in field.values() {
                entries.push(entry(value, take(&mut parts, value.part_name())?)?);
            }
            no_other_key(&parts)?;
        }
        no_other_key(&table)?;
        Ok(Self { board, entries })
    }

    /// The id of the board description the record is for.
    pub fn board(&self) -> &str {
        &self.board
    }

    /// Every value of the record, one per field or part, in the order of
    /// [`Field::ALL`] and of each field's parts.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The value of `field`; [`Value::Unknown`] for a field or part the
    /// record does not have, which a parsed record never lacks.
    pub fn value(&self, field: FieldRef) -> &Value {
        let entry = self.entries.iter().find(|entry| entry.field == field);
        entry.map_or(&Value::Unknown, |entry| &entry.value)
    }

    /// Checks the record against the board description it is for: it names
    /// the board, and every value is read in a way that the description lists
    /// for it and is a value that way can give (`docs/bench/formats.md`).
    pub fn check(&self, board: &Board) -> Result<(), IdentityError> {
        if self.board != board.id {
            let (record, board) = (self.board.clone(), board.id.clone());
            return Err(IdentityError::Board { record, board });
        }
        for entry in &self.entries {
            let (field, read) = (entry.field, entry.read);
            let ways: Vec<&Method> = board
                .reads(field)
                .map(|read| &read.method)
                .filter(|method| method.kind() == read)
                .collect();
            match &entry.value {
                _ if ways.is_empty() => return Err(IdentityError::Read { field, read }),
                Value::Text(value) if read == ReadMethod::FirmwareLogPresence => {
                    if !ways.iter().any(|way| way.presence_value() == Some(value)) {
                        let value = value.clone();
                        return Err(IdentityError::Presence { field, value });
                    }
                }
                Value::Files(files) if read == ReadMethod::TftpReads => {
                    let listed = |file: &&PayloadName| {
                        ways.iter().any(|way| {
                            matches!(way, Method::TftpReads { files }
                                if files.iter().any(|listed| listed.eq_ignore_ascii_case(file)))
                        })
                    };
                    if let Some(file) = files.iter().find(|file| !listed(file)) {
                        let file = file.clone();
                        return Err(IdentityError::TftpReads { field, file });
                    }
                }
                _ => {}
            }
        }
        let value = |field| self.value(FieldRef::whole(field));
        match (
            value(Field::ManufacturingVersion),
            value(Field::MinBootVersion),
        ) {
            (Value::Number(device), Value::Number(software)) if device != software => {
                Err(IdentityError::ManufacturingVersions {
                    manufacturing_version: *device,
                    min_boot_version: *software,
                })
            }
            _ => Ok(()),
        }
    }
}

/// Removes `key` from `table` and reads it as `T`.
fn take<T: DeserializeOwned>(table: &mut toml::Table, key: &str) -> Result<T, IdentityError> {
    let value = table
        .remove(key)
        .ok_or_else(|| syntax(format!("missing field `{key}`")))?;
    value
        .try_into()
        .map_err(|error| syntax(format!("{key}: {error}")))
}

/// Refuses a key that is left in `table`.
fn no_other_key(table: &toml::Table) -> Result<(), IdentityError> {
    match table.keys().next() {
        Some(key) => Err(syntax(format!("unknown field `{key}`"))),
        None => Ok(()),
    }
}

/// The entry of `field`, with its value converted to the kind the field
/// holds.
fn entry(field: FieldRef, reading: RawReading) -> Result<Entry, IdentityError> {
    let invalid = |reason: String| IdentityError::Value { field, reason };
    let value = match (field.field.kind(), reading.value) {
        // Read as a value, "Unknown" would be a known value that matches
        // nothing, and a rule that needs the value would be decided on it.
        (_, RawValue::Text(text)) if text == UNKNOWN => Value::Unknown,
        (_, RawValue::Text(text)) if text.eq_ignore_ascii_case(UNKNOWN) => {
            return Err(invalid("write unknown in lower case".into()));
        }
        (Kind::Text, RawValue::Text(text)) if is_line_text(&text) => Value::Text(text),
        (Kind::Number, RawValue::Integer(number @ 1..)) if let Ok(number) = number.try_into() => {
            Value::Number(number)
        }
        (Kind::Files, RawValue::List(list)) if !list.is_empty() => {
            let files = list
                .iter()
                .map(|name| PayloadName::parse(name).map_err(|error| format!("{name:?}: {error}")))
                .collect::<Result<Vec<_>, _>>()
                .map_err(invalid)?;
            names::check_distinct(&files).map_err(|error| invalid(error.to_string()))?;
            Value::Files(files)
        }
        (Kind::Text, _) => return Err(invalid("a line of text, or unknown".into())),
        (Kind::Number, _) => {
            return Err(invalid(format!("1 to {}; write unknown for 0", u32::MAX)));
        }
        (Kind::Files, _) => return Err(invalid("a list of payload names, or unknown".into())),
    };
    let read = reading.read;
    Ok(Entry { field, value, read })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawReading {
    value: RawValue,
    read: ReadMethod,
}

/// A value as written: a string, an integer or a list of strings.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawValue {
    Text(String),
    Integer(i64),
    List(Vec<String>),
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(message) => write!(f, "cannot read the identity record: {message}"),
            Self::Format(found) => write!(f, "{found} is not \"{FORMAT}\" version 1"),
            Self::Value { field, reason } => write!(f, "the identity value {field}: {reason}"),
            Self::Board { record, board } => write!(f, "the record is for {record}, not {board}"),
            Self::Read { field, read } => write!(f, "{field} is not listed as read by {read}"),
            Self::Presence { field, value } => write!(f, "no line of {field} gives {value}"),
            Self::TftpReads { field, file } => write!(f, "no tftp_reads of {field} names {file}"),
            Self::ManufacturingVersions {
                manufacturing_version: device,
                min_boot_version: software,
            } => write!(
                f,
                "manufacturing_version {device} and min_boot_version {software} differ"
            ),
        }
    }
}

impl std::error::Error for IdentityError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board;

    /// The blank form of an identity record.
    const TEMPLATE: &str = include_str!("../../templates/identity.toml");

    fn pi5() -> Board {
        Board::parse(board::RASPBERRY_PI_5).unwrap()
    }

    fn field(text: &str) -> FieldRef {
        FieldRef::parse(text).unwrap()
    }

    /// `text` with the line of `key` replaced by `line`.
    fn replace_line(text: &str, key: &str, line: &str) -> String {
        let mut found = false;
        let mut replaced = String::new();
        for existing in text.lines() {
            if existing.split_whitespace().next() == Some(key) {
                found = true;
                replaced.push_str(line);
            } else {
                replaced.push_str(existing);
            }
            replaced.push('\n');
        }
        assert!(found, "the text has a line for {key}");
        replaced
    }

    /// The template with the line of `key` replaced by `line`.
    fn with_line(key: &str, line: &str) -> String {
        replace_line(TEMPLATE, key, line)
    }

    #[test]
    fn identity_template_parses_with_ten_fields() {
        let record = IdentityRecord::parse(TEMPLATE).unwrap();
        assert_eq!(record.board(), "raspberry-pi-5");
        let fields: Vec<Field> = record
            .entries()
            .iter()
            .map(|entry| entry.field.field)
            .collect();
        let mut distinct = fields.clone();
        distinct.dedup();
        assert_eq!(distinct, Field::ALL);
        assert_eq!(
            record.entries().len(),
            12,
            "ten fields, two of them in two parts"
        );
        assert!(
            record
                .entries()
                .iter()
                .all(|entry| entry.value == Value::Unknown)
        );
        record.check(&pi5()).unwrap();
    }

    #[test]
    fn field_references_name_a_field_or_one_of_its_parts() {
        assert_eq!(
            field("memory.part"),
            FieldRef {
                field: Field::Memory,
                part: Some("part")
            }
        );
        assert_eq!(field("pcb_version").to_string(), "pcb_version");
        assert_eq!(
            field("spi_flash.jedec_id").to_string(),
            "spi_flash.jedec_id"
        );
        assert_eq!(field("pcb_version").part_name(), "value");
        for text in [
            "memory",
            "pcb_version.part",
            "memory.size",
            "serial",
            "",
            ".",
        ] {
            assert_eq!(FieldRef::parse(text), None, "{text:?}");
        }
        let all: Vec<FieldRef> = Field::ALL.into_iter().flat_map(Field::values).collect();
        assert_eq!(all.len(), 12);
        for value in all {
            assert_eq!(FieldRef::parse(&value.to_string()), Some(value));
        }
    }

    #[test]
    fn values_take_the_form_of_their_field() {
        let text = with_line(
            "pcb_version",
            r#"pcb_version = { value = "7", read = "visual" }"#,
        );
        let text = replace_line(
            &text,
            "min_boot_version",
            r#"min_boot_version = { value = 2, read = "software" }"#,
        );
        let text = replace_line(
            &text,
            "boot_files",
            r#"boot_files = { value = ["bcm2712-rpi-5-b.dtb", "overlays/bcm2712d0.dtbo"], read = "tftp_reads" }"#,
        );
        let record = IdentityRecord::parse(&text).unwrap();
        assert_eq!(
            record.value(field("pcb_version")),
            &Value::Text("7".to_string())
        );
        assert_eq!(record.value(field("min_boot_version")), &Value::Number(2));
        assert_eq!(
            record.value(field("boot_files")),
            &Value::Files(vec![
                PayloadName::parse("bcm2712-rpi-5-b.dtb").unwrap(),
                PayloadName::parse("overlays/bcm2712d0.dtbo").unwrap(),
            ])
        );
        record.check(&pi5()).unwrap();
    }

    #[test]
    fn a_value_of_the_wrong_form_is_refused() {
        let cases = [
            (
                "pcb_version",
                r#"pcb_version = { value = 7, read = "visual" }"#,
            ),
            (
                "pcb_version",
                r#"pcb_version = { value = "", read = "visual" }"#,
            ),
            (
                "pcb_version",
                r#"pcb_version = { value = " 7", read = "visual" }"#,
            ),
            (
                "min_boot_version",
                r#"min_boot_version = { value = 0, read = "software" }"#,
            ),
            (
                "min_boot_version",
                r#"min_boot_version = { value = -1, read = "software" }"#,
            ),
            (
                "min_boot_version",
                r#"min_boot_version = { value = 4294967296, read = "software" }"#,
            ),
            (
                "min_boot_version",
                r#"min_boot_version = { value = "1", read = "software" }"#,
            ),
            (
                "boot_files",
                r#"boot_files = { value = [], read = "tftp_reads" }"#,
            ),
            (
                "boot_files",
                r#"boot_files = { value = ["../x.dtb"], read = "tftp_reads" }"#,
            ),
            (
                "boot_files",
                r#"boot_files = { value = ["a.dtb", "A.DTB"], read = "tftp_reads" }"#,
            ),
            (
                "boot_files",
                r#"boot_files = { value = "a.dtb", read = "tftp_reads" }"#,
            ),
            (
                "memory.part",
                r#"memory.part = { value = "Unknown", read = "visual" }"#,
            ),
            (
                "min_boot_version",
                r#"min_boot_version = { value = "UNKNOWN", read = "software" }"#,
            ),
            (
                "boot_files",
                r#"boot_files = { value = "Unknown", read = "tftp_reads" }"#,
            ),
        ];
        for (key, line) in cases {
            let error = IdentityRecord::parse(&with_line(key, line)).unwrap_err();
            assert!(
                matches!(&error, IdentityError::Value { field, .. } if field.to_string() == key),
                "{line}: {error}"
            );
        }
        // Read as a value, "Unknown" would match no bound and decide the
        // minimum bootloader without it.
        let error = IdentityRecord::parse(&with_line(
            "memory.part",
            r#"memory.part = { value = "Unknown", read = "visual" }"#,
        ))
        .unwrap_err();
        assert!(
            error.to_string().contains("write unknown in lower case"),
            "{error}"
        );
        let error = IdentityRecord::parse(&with_line(
            "min_boot_version",
            r#"min_boot_version = { value = 0, read = "software" }"#,
        ))
        .unwrap_err();
        assert!(error.to_string().contains("write unknown"), "{error}");
    }

    #[test]
    fn unknown_keys_and_missing_fields_are_refused() {
        let extra = format!("{TEMPLATE}serial = {{ value = \"unknown\", read = \"visual\" }}\n");
        assert!(matches!(
            IdentityRecord::parse(&extra),
            Err(IdentityError::Syntax(_))
        ));
        let missing: String = TEMPLATE
            .lines()
            .filter(|line| !line.starts_with("ethernet_phy"))
            .map(|line| format!("{line}\n"))
            .collect();
        let error = IdentityRecord::parse(&missing).unwrap_err();
        assert!(error.to_string().contains("ethernet_phy"), "{error}");
        let comment = with_line(
            "pcb_version",
            r#"pcb_version = { value = "4", read = "visual", note = "x" }"#,
        );
        assert!(matches!(
            IdentityRecord::parse(&comment),
            Err(IdentityError::Syntax(_))
        ));
        let method = with_line(
            "pcb_version",
            r#"pcb_version = { value = "4", read = "guess" }"#,
        );
        assert!(matches!(
            IdentityRecord::parse(&method),
            Err(IdentityError::Syntax(_))
        ));
    }

    #[test]
    fn a_record_of_another_format_or_version_is_named_as_such() {
        let version = TEMPLATE.replace("format_version = 1", "format_version = 2");
        assert_eq!(
            IdentityRecord::parse(&version),
            Err(IdentityError::Format(Header {
                format: FORMAT.to_string(),
                format_version: 2
            }))
        );
        let format = TEMPLATE.replace(FORMAT, "kyto-bench instance");
        assert!(matches!(
            IdentityRecord::parse(&format),
            Err(IdentityError::Format(_))
        ));
    }

    #[test]
    fn every_value_is_read_in_a_way_the_board_lists() {
        let board = pi5();
        let listed = with_line(
            "pcb_version",
            r#"pcb_version = { value = "4", read = "software" }"#,
        );
        IdentityRecord::parse(&listed)
            .unwrap()
            .check(&board)
            .unwrap();
        let unlisted = with_line(
            "ethernet_phy",
            r#"ethernet_phy = { value = "unknown", read = "firmware_log" }"#,
        );
        assert_eq!(
            IdentityRecord::parse(&unlisted).unwrap().check(&board),
            Err(IdentityError::Read {
                field: field("ethernet_phy"),
                read: ReadMethod::FirmwareLog
            })
        );
        // The JEDEC identifier is read from the log; the part number is not.
        let part = with_line(
            "spi_flash.part",
            r#"spi_flash.part = { value = "unknown", read = "firmware_log" }"#,
        );
        let error = IdentityRecord::parse(&part)
            .unwrap()
            .check(&board)
            .unwrap_err();
        assert!(
            matches!(error, IdentityError::Read { field, .. } if field.part == Some("part")),
            "{error}"
        );
    }

    #[test]
    fn a_value_read_from_a_present_line_is_the_value_the_line_stands_for() {
        let board = pi5();
        let d0 = with_line(
            "soc_stepping",
            r#"soc_stepping = { value = "D0", read = "firmware_log_presence" }"#,
        );
        IdentityRecord::parse(&d0).unwrap().check(&board).unwrap();
        let c1 = with_line(
            "soc_stepping",
            r#"soc_stepping = { value = "C1", read = "firmware_log_presence" }"#,
        );
        assert_eq!(
            IdentityRecord::parse(&c1).unwrap().check(&board),
            Err(IdentityError::Presence {
                field: field("soc_stepping"),
                value: "C1".to_string()
            })
        );
        let visual = with_line(
            "soc_stepping",
            r#"soc_stepping = { value = "C1", read = "visual" }"#,
        );
        IdentityRecord::parse(&visual)
            .unwrap()
            .check(&board)
            .unwrap();
    }

    #[test]
    fn a_value_read_from_the_tftp_reads_names_only_files_the_way_lists() {
        let board = pi5();
        let with_files = |files: &str| {
            let text = with_line(
                "boot_files",
                &format!(r#"boot_files = {{ value = {files}, read = "tftp_reads" }}"#),
            );
            IdentityRecord::parse(&text).unwrap()
        };
        for files in [
            r#"["bcm2712-rpi-5-b.dtb"]"#,
            r#"["BCM2712-RPI-5-B.DTB", "overlays/bcm2712d0.dtbo"]"#,
        ] {
            with_files(files).check(&board).unwrap();
        }
        let error = with_files(r#"["bcm2712-rpi-5-b.dtb", "foo.dtb"]"#)
            .check(&board)
            .unwrap_err();
        assert_eq!(
            error,
            IdentityError::TftpReads {
                field: field("boot_files"),
                file: PayloadName::parse("foo.dtb").unwrap()
            }
        );
        assert!(error.to_string().contains("foo.dtb"), "{error}");
    }

    #[test]
    fn the_two_manufacturing_versions_agree_when_both_are_known() {
        let board = pi5();
        let with = |manufacturing: &str, min_boot: &str| {
            let text = with_line(
                "manufacturing_version",
                &format!(
                    r#"manufacturing_version = {{ value = {manufacturing}, read = "firmware_log" }}"#
                ),
            );
            let text = replace_line(
                &text,
                "min_boot_version",
                &format!(r#"min_boot_version = {{ value = {min_boot}, read = "software" }}"#),
            );
            IdentityRecord::parse(&text).unwrap()
        };
        for (manufacturing, min_boot) in [("2", "2"), ("1", "\"unknown\""), ("\"unknown\"", "3")] {
            with(manufacturing, min_boot).check(&board).unwrap();
        }
        let error = with("1", "2").check(&board).unwrap_err();
        assert_eq!(
            error,
            IdentityError::ManufacturingVersions {
                manufacturing_version: 1,
                min_boot_version: 2
            }
        );
        assert!(error.to_string().contains("differ"), "{error}");
    }

    #[test]
    fn a_record_for_another_board_is_refused() {
        let other = TEMPLATE.replace("board = \"raspberry-pi-5\"", "board = \"another-board\"");
        let record = IdentityRecord::parse(&other).unwrap();
        assert_eq!(
            record.check(&pi5()),
            Err(IdentityError::Board {
                record: "another-board".to_string(),
                board: "raspberry-pi-5".to_string()
            })
        );
    }

    #[test]
    fn a_record_is_read_from_a_table() {
        let table: toml::Table = toml::from_str(TEMPLATE).unwrap();
        assert_eq!(
            IdentityRecord::from_table(table).unwrap(),
            IdentityRecord::parse(TEMPLATE).unwrap()
        );
        let mut table: toml::Table = toml::from_str(TEMPLATE).unwrap();
        table.insert("extra".to_string(), toml::Value::Boolean(true));
        assert!(matches!(
            IdentityRecord::from_table(table),
            Err(IdentityError::Syntax(_))
        ));
    }
}
