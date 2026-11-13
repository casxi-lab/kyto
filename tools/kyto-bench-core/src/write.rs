//! Writes that a payload can make the board's firmware perform, and the
//! rules that keep them off.
//!
//! A board description lists write triggers: payload files whose presence
//! lets the firmware write firmware, EEPROM contents or storage media. A run
//! plan entry enables classes of writes; every class is off unless the entry
//! enables it. An entry's enable is needed for a write, but it does not
//! authorize one: whoever runs a plan that enables a write decides that
//! outside the tool. A board description also lists refused properties:
//! settings that a payload file must not contain, whatever the plan enables.

use std::fmt;

use serde::Deserialize;

use crate::document::named_enum;
use crate::names::{self, PayloadName};
use crate::pattern::LinePattern;

named_enum! {
    /// A class of writes. There are exactly three.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum WriteClass {
        /// Firmware files kept on the board.
        Firmware = "firmware",
        /// The contents of the boot EEPROM.
        Eeprom = "eeprom",
        /// Storage media such as SD cards.
        Storage = "storage",
    }
}

/// The classes of writes a run plan entry enables. A class left out is not
/// enabled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WriteEnables {
    pub firmware: bool,
    pub eeprom: bool,
    pub storage: bool,
}

impl WriteEnables {
    /// Whether `class` is enabled.
    pub fn enabled(&self, class: WriteClass) -> bool {
        match class {
            WriteClass::Firmware => self.firmware,
            WriteClass::Eeprom => self.eeprom,
            WriteClass::Storage => self.storage,
        }
    }
}

/// Payload files whose presence lets the firmware write `class`. A payload
/// file is a trigger when its file name, the last segment of its payload
/// name, equals one of `files` ignoring ASCII case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteTrigger {
    pub class: WriteClass,
    files: Vec<String>,
}

/// Why a write trigger cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TriggerError {
    /// The trigger lists no file.
    NoFiles,
    /// A listed file is not one segment of a payload name.
    NotAFileName { file: String },
}

impl WriteTrigger {
    /// A trigger of `class` for the file names `files`, each one segment of
    /// a payload name.
    pub fn new(class: WriteClass, files: Vec<String>) -> Result<Self, TriggerError> {
        if files.is_empty() {
            return Err(TriggerError::NoFiles);
        }
        if let Some(file) = files.iter().find(|file| !names::is_segment(file)) {
            return Err(TriggerError::NotAFileName { file: file.clone() });
        }
        Ok(Self { class, files })
    }

    /// The file names of the trigger.
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// Whether `name` is one of the trigger's files.
    pub fn matches(&self, name: &PayloadName) -> bool {
        let file = name.file_name();
        self.files
            .iter()
            .any(|trigger| trigger.eq_ignore_ascii_case(file))
    }
}

/// A payload file that triggers a write of a class that is not permitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unpermitted {
    pub file: PayloadName,
    pub class: WriteClass,
}

/// Checks every file of a payload against `triggers`: a file that triggers a
/// class not in `permitted` is refused. All refused files are returned, in
/// the order of `files`, each once for every class it triggers.
pub fn check_triggers<'a>(
    files: impl IntoIterator<Item = &'a PayloadName>,
    triggers: &[WriteTrigger],
    permitted: &[WriteClass],
) -> Result<(), Vec<Unpermitted>> {
    let mut refused = Vec::new();
    for file in files {
        for class in WriteClass::ALL {
            let triggered = triggers
                .iter()
                .any(|trigger| trigger.class == class && trigger.matches(file));
            if triggered && !permitted.contains(&class) {
                refused.push(Unpermitted {
                    file: file.clone(),
                    class,
                });
            }
        }
    }
    if refused.is_empty() {
        Ok(())
    } else {
        Err(refused)
    }
}

/// Whether `text` can be the name of a refused property: ASCII letters,
/// digits and `_`.
pub fn is_property_name(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// A place in a payload file that sets a refused property.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertyHit {
    /// The payload file.
    pub file: PayloadName,
    /// The offset in the file of the line that sets the property: the start
    /// of the file, or the byte after a CR or LF.
    pub offset: u64,
    /// The refused property, as the board description writes it.
    pub property: String,
}

/// Finds the places that set a refused property in every file whose payload
/// name matches `scanned`. A line starts at the start of a file and after
/// every CR and every LF, so that the check holds whichever of them a reader
/// of the file takes for a line end. A line sets a property when it is, in
/// order, optional spaces, tabs, vertical tabs and form feeds, the
/// property's name, optional ASCII white space, CR and LF included, and `=`.
/// Names are compared ignoring ASCII case. Hits are returned in the order
/// of `files` and of their offsets, at most one for each line.
pub fn find_refused_properties<'a>(
    files: impl IntoIterator<Item = (&'a PayloadName, &'a [u8])>,
    scanned: &LinePattern,
    properties: &[String],
) -> Vec<PropertyHit> {
    let mut hits = Vec::new();
    for (file, bytes) in files
        .into_iter()
        .filter(|(file, _)| scanned.is_match(file.as_str().as_bytes()))
    {
        let line_start = |start: &usize| *start == 0 || matches!(bytes[start - 1], b'\n' | b'\r');
        for start in (0..=bytes.len()).filter(line_start) {
            let set = properties
                .iter()
                .find(|property| sets_property(&bytes[start..], property));
            if let Some(property) = set {
                hits.push(PropertyHit {
                    file: file.clone(),
                    offset: start as u64,
                    property: property.clone(),
                });
            }
        }
    }
    hits
}

/// White space within a line.
fn is_blank(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | 0x0b | 0x0c)
}

/// ASCII white space, line ends included.
fn is_space(byte: u8) -> bool {
    is_blank(byte) || matches!(byte, b'\n' | b'\r')
}

/// Whether the bytes from the start of a line set `property`.
fn sets_property(from: &[u8], property: &str) -> bool {
    let rest = &from[from.iter().take_while(|byte| is_blank(**byte)).count()..];
    let Some(name) = rest.get(..property.len()) else {
        return false;
    };
    let mut after = rest[property.len()..]
        .iter()
        .filter(|byte| !is_space(**byte));
    name.eq_ignore_ascii_case(property.as_bytes()) && after.next() == Some(&b'=')
}

impl fmt::Display for TriggerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoFiles => write!(f, "a write trigger lists at least one file"),
            Self::NotAFileName { file } => write!(f, "the trigger {file:?} is not a file name"),
        }
    }
}

impl std::error::Error for TriggerError {}

impl fmt::Display for Unpermitted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the payload file {} lets the firmware write {}, which is not enabled",
            self.file, self.class
        )
    }
}

impl fmt::Display for PropertyHit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the payload file {} sets the refused property {} in the line at offset {}",
            self.file, self.property, self.offset
        )
    }
}

#[cfg(test)]
mod tests {
    use serde::de::IntoDeserializer;
    use serde::de::value::{Error as ValueError, MapDeserializer, StrDeserializer};

    use super::*;

    fn name(text: &str) -> PayloadName {
        PayloadName::parse(text).unwrap()
    }

    fn eeprom_trigger() -> WriteTrigger {
        let files = [
            "pieeprom.upd",
            "pieeprom.sig",
            "pieeprom.bin",
            "recovery.bin",
        ];
        WriteTrigger::new(WriteClass::Eeprom, files.map(String::from).to_vec()).unwrap()
    }

    fn enables(pairs: &[(&'static str, bool)]) -> Result<WriteEnables, ValueError> {
        let map: MapDeserializer<'_, _, ValueError> = MapDeserializer::new(pairs.iter().copied());
        WriteEnables::deserialize(map)
    }

    #[test]
    fn write_classes_are_firmware_eeprom_and_storage_only() {
        assert_eq!(
            WriteClass::ALL.map(WriteClass::name),
            ["firmware", "eeprom", "storage"]
        );
        let read = |text: &str| {
            let de: StrDeserializer<'_, ValueError> = text.into_deserializer();
            WriteClass::deserialize(de)
        };
        for class in WriteClass::ALL {
            assert_eq!(read(class.name()).unwrap(), class);
        }
        for text in ["otp", "secure_boot", "rpiboot", "Eeprom", ""] {
            assert!(read(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn every_class_is_off_unless_enabled() {
        let none = enables(&[]).unwrap();
        assert_eq!(none, WriteEnables::default());
        let classes = |enables: &WriteEnables| -> Vec<WriteClass> {
            WriteClass::ALL
                .into_iter()
                .filter(|class| enables.enabled(*class))
                .collect()
        };
        assert!(classes(&none).is_empty());
        let eeprom = enables(&[("eeprom", true)]).unwrap();
        assert_eq!(classes(&eeprom), [WriteClass::Eeprom]);
        assert!(!eeprom.enabled(WriteClass::Firmware));
        assert!(!eeprom.enabled(WriteClass::Storage));
        let all = enables(&[("firmware", true), ("eeprom", true), ("storage", true)]).unwrap();
        assert_eq!(classes(&all), WriteClass::ALL);
        assert!(enables(&[("otp", true)]).is_err());
    }

    #[test]
    fn trigger_names_match_ignoring_ascii_case() {
        let trigger = eeprom_trigger();
        assert!(trigger.matches(&name("pieeprom.upd")));
        assert!(trigger.matches(&name("PIEEPROM.UPD")));
        assert!(trigger.matches(&name("Recovery.Bin")));
        // The file name decides, whatever directory holds the file.
        assert!(trigger.matches(&name("sub/pieeprom.sig")));
        assert!(!trigger.matches(&name("pieeprom.upd.txt")));
        assert!(!trigger.matches(&name("pieeprom.upd/readme")));
        assert!(!trigger.matches(&name("config.txt")));
    }

    #[test]
    fn a_trigger_without_its_class_permitted_is_refused() {
        let triggers = [eeprom_trigger()];
        let payload = [
            name("config.txt"),
            name("PIEEPROM.UPD"),
            name("recovery.bin"),
        ];
        assert_eq!(
            check_triggers(&payload, &triggers, &[]),
            Err(vec![
                Unpermitted {
                    file: name("PIEEPROM.UPD"),
                    class: WriteClass::Eeprom
                },
                Unpermitted {
                    file: name("recovery.bin"),
                    class: WriteClass::Eeprom
                },
            ])
        );
        assert!(check_triggers(&payload, &triggers, &[WriteClass::Firmware]).is_err());
        assert_eq!(
            check_triggers(&payload, &triggers, &[WriteClass::Eeprom]),
            Ok(())
        );
        assert_eq!(check_triggers(&payload[..1], &triggers, &[]), Ok(()));
    }

    #[test]
    fn trigger_files_are_file_names() {
        assert_eq!(
            WriteTrigger::new(WriteClass::Eeprom, Vec::new()),
            Err(TriggerError::NoFiles)
        );
        for file in ["dir/pieeprom.upd", "", "..", "a b"] {
            assert_eq!(
                WriteTrigger::new(WriteClass::Eeprom, vec![file.to_string()]),
                Err(TriggerError::NotAFileName {
                    file: file.to_string()
                }),
                "{file:?}"
            );
        }
        assert_eq!(eeprom_trigger().files().len(), 4);
    }

    #[test]
    fn refused_properties_are_found_in_scanned_text_files() {
        let scanned = LinePattern::new(r"(?i)\.txt$").unwrap();
        let properties = ["program_pubkey", "revoke_devkey"].map(String::from);
        let config = b"# program_pubkey=1 is a comment\r\n[all]\r\n  Program_PubKey\t = 1\r\n";
        let extra = b"revoke_devkey=1";
        let other = b"program_pubkey=1\n";
        let files = [
            (name("config.txt"), &config[..]),
            (name("EXTRA.TXT"), &extra[..]),
            (name("kernel.img"), &other[..]),
        ];
        let hits = find_refused_properties(
            files.iter().map(|(file, bytes)| (file, *bytes)),
            &scanned,
            &properties,
        );
        assert_eq!(
            hits,
            [
                PropertyHit {
                    file: name("config.txt"),
                    offset: 40,
                    property: "program_pubkey".to_string()
                },
                PropertyHit {
                    file: name("EXTRA.TXT"),
                    offset: 0,
                    property: "revoke_devkey".to_string()
                },
            ]
        );
    }

    #[test]
    fn lines_end_at_a_cr_as_well_as_at_an_lf() {
        let scanned = LinePattern::new("(?i).").unwrap();
        let properties = ["program_pubkey", "revoke_devkey"].map(String::from);
        let find = |bytes: &[u8]| {
            let file = name("config.txt");
            find_refused_properties([(&file, bytes)], &scanned, &properties)
                .into_iter()
                .map(|hit| (hit.offset, hit.property))
                .collect::<Vec<_>>()
        };
        // A file with CR line ends only.
        assert_eq!(
            find(b"[all]\rarm_64bit=1\r  revoke_devkey=1\rprogram_pubkey = 1\r"),
            [
                (18, "revoke_devkey".to_string()),
                (36, "program_pubkey".to_string())
            ]
        );
        // A CR inside a line ended by LF: a reader that takes only LF for a
        // line end reads `program_pubkey\r=1` as one setting, and one that
        // also takes CR reads the line after the CR.
        assert_eq!(
            find(b"x=1\nprogram_pubkey\r=1\n"),
            [(4, "program_pubkey".to_string())]
        );
        assert_eq!(
            find(b"# comment \rrevoke_devkey=1\n"),
            [(11, "revoke_devkey".to_string())]
        );
        assert_eq!(find(b"# program_pubkey=1\r\n#\trevoke_devkey=1"), []);
    }

    #[test]
    fn a_property_is_set_only_by_its_whole_name_before_an_equals_sign() {
        for line in [
            &b"program_pubkey=1"[..],
            b"program_pubkey = 1",
            b" \t\x0b\x0cprogram_pubkey\x0b=",
            b"PROGRAM_PUBKEY=0",
            b"program_pubkey\r\n=1",
        ] {
            assert!(sets_property(line, "program_pubkey"), "{line:?}");
        }
        for line in [
            &b"program_pubkey_x=1"[..],
            b"xprogram_pubkey=1",
            b"#program_pubkey=1",
            b"program_pubkey",
            b"program_pubkey 1",
            b"program_pub",
            b"",
            b"[program_pubkey=1]",
            b"\nprogram_pubkey=1",
        ] {
            assert!(!sets_property(line, "program_pubkey"), "{line:?}");
        }
    }

    #[test]
    fn property_names_are_letters_digits_and_underscores() {
        assert!(is_property_name("program_pubkey"));
        assert!(is_property_name("eeprom_write_protect"));
        assert!(!is_property_name(""));
        assert!(!is_property_name("a=b"));
        assert!(!is_property_name("a b"));
    }
}
