//! Board descriptions: the facts the bench tool knows about a type of board,
//! as data, with every line pattern's examples and every official document
//! it relies on (`docs/bench/formats.md`, board descriptions). A description
//! is read strictly: an unknown or missing key, a value of the wrong type, a
//! pattern that does not compile, lacks a group or does not match its
//! examples, a reference to nothing and a source that nothing cites are
//! errors.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::Deref;

use serde::Deserialize;

use crate::bootloader::ReleaseDate;
use crate::document::{Header, NOT_A_NAME, bounded, check, is_line_text, is_name};
use crate::identity::{Field, FieldRef, Kind, ReadMethod, UNKNOWN};
use crate::names::{self, PayloadName};
use crate::pattern::LinePattern;
use crate::write::{self, WriteClass, WriteTrigger};

/// The `format` of a board description.
pub const FORMAT: &str = "kyto-bench board description";

/// The `format_version` of a board description that this tool reads.
pub const FORMAT_VERSION: i64 = 1;

/// The description of the Raspberry Pi 5.
pub const RASPBERRY_PI_5: &str = include_str!("../../boards/raspberry-pi-5.toml");

/// The texts of the board descriptions built into the tool.
pub const EMBEDDED: [&str; 1] = [RASPBERRY_PI_5];

const MAX_OFF_INTERVAL_MS: u64 = 3_600_000;

/// An official document that a description cites by `id`, with its `https`
/// address and the commit or release that the description relies on.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub title: String,
    pub url: String,
    pub version: String,
}

/// The parity of the serial console.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Parity {
    None,
    Even,
    Odd,
}

/// The settings of the board's serial console.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleSettings {
    pub baud: u32,
    pub data_bits: u8,
    pub parity: Parity,
    pub stop_bits: u8,
    #[serde(default)]
    pub sources: Vec<String>,
}

/// A step of a cold boot: publish the payload, wait for the off interval
/// from the off confirmation, open the console, or switch the supply on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Publish,
    WaitOffInterval,
    OpenConsole,
    PowerOn,
}

/// The order of a cold boot, each step once and switching the supply on
/// last, and the time in milliseconds that the board stays off, counted from
/// the confirmation that the supply is off.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColdBoot {
    pub order: Vec<Step>,
    pub off_interval_ms: u64,
}

/// A line pattern with the example lines that document the form it reads.
/// Every example matches the pattern.
#[derive(Clone, Debug)]
pub struct LineForm {
    pub pattern: LinePattern,
    pub examples: Vec<String>,
}

/// A marker, which run plans name `board.<name>`, a firmware version line or
/// a sensitive line, whose named groups a public receipt must not show.
#[derive(Clone, Debug)]
pub struct NamedLine {
    pub name: String,
    pub form: LineForm,
    pub sources: Vec<String>,
}

/// The line that names the boot mode the firmware tries, with the groups
/// `mode` and `code`; `network` is the mode of a network boot, and every
/// example of the marker `network_marker` is a boot mode line of that mode.
#[derive(Clone, Debug)]
pub struct BootMode {
    pub form: LineForm,
    pub network: String,
    pub network_marker: String,
    pub sources: Vec<String>,
}

/// The lines of TFTP transfers: lines that name the server in the group
/// `server`, a file read in full, with the groups `path` and `bytes`, and a
/// file that could not be read, with the group `path`.
#[derive(Clone, Debug)]
pub struct Tftp {
    pub servers: Vec<LineForm>,
    pub read: LineForm,
    pub read_failed: LineForm,
}

/// A way of reading an identity value, with what it needs.
#[derive(Clone, Debug)]
pub enum Method {
    /// The group `value` of a line of the firmware log.
    FirmwareLog { form: LineForm },
    /// The presence of a line, which gives `value`; its absence leaves the
    /// value unknown.
    FirmwareLogPresence { form: LineForm, value: String },
    /// A group of a firmware version line.
    FirmwareVersion { name: String, group: String },
    /// Which of `files` the board reads over TFTP.
    TftpReads { files: Vec<PayloadName> },
    /// Looking at the board, as `how` says.
    Visual { how: String },
    /// Software running on the board, as `how` says.
    Software { how: String },
}

impl Method {
    /// The name of the way of reading, as identity records write it.
    pub fn kind(&self) -> ReadMethod {
        match self {
            Method::FirmwareLog { .. } => ReadMethod::FirmwareLog,
            Method::FirmwareLogPresence { .. } => ReadMethod::FirmwareLogPresence,
            Method::FirmwareVersion { .. } => ReadMethod::FirmwareVersion,
            Method::TftpReads { .. } => ReadMethod::TftpReads,
            Method::Visual { .. } => ReadMethod::Visual,
            Method::Software { .. } => ReadMethod::Software,
        }
    }

    /// The value that the presence of a line gives.
    pub fn presence_value(&self) -> Option<&str> {
        match self {
            Method::FirmwareLogPresence { value, .. } => Some(value),
            _ => None,
        }
    }
}

/// One way of reading an identity field.
#[derive(Clone, Debug)]
pub struct Read {
    pub field: Field,
    /// The part it reads; `None` reads every part of the field.
    pub part: Option<&'static str>,
    pub method: Method,
    pub sources: Vec<String>,
}

/// The payload files that a board needs for one value of an identity field.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    pub field: FieldRef,
    pub value: String,
    pub payload: Vec<PayloadName>,
    #[serde(default)]
    pub sources: Vec<String>,
}

/// The earliest bootloader release with a manufacturing version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirstRelease {
    pub version: u32,
    pub date: ReleaseDate,
}

/// A lower bound on the bootloader release: a board whose `field` is one of
/// `any_of`, ignoring ASCII case, needs a release of `at_least` or later.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bound {
    pub field: FieldRef,
    pub any_of: Vec<String>,
    pub at_least: ReleaseDate,
    pub source: String,
}

/// The rules that give the earliest bootloader release a board needs:
/// `releases`, the earliest release of each manufacturing version in
/// increasing order of both, which `sources` support, and `bounds`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MinBootloader {
    pub releases: Vec<FirstRelease>,
    pub sources: Vec<String>,
    pub bounds: Vec<Bound>,
}

/// Payload files whose presence lets the firmware write, with the document
/// that says so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trigger {
    pub trigger: WriteTrigger,
    pub source: String,
}

/// A setting that a payload file must not contain.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefusedProperty {
    pub name: String,
    pub source: String,
}

/// The settings that payload files must not contain, in the files whose
/// payload names `files` matches, ignoring ASCII case.
#[derive(Clone, Debug)]
pub struct RefusedProperties {
    pub files: LineForm,
    pub properties: Vec<RefusedProperty>,
}

/// A board description, read and checked by [`Board::parse`]. It keeps its
/// text, which receipts record verbatim, and dereferences to its parts.
#[derive(Clone, Debug)]
pub struct Board {
    text: String,
    description: Description,
}

/// The parts of a board description.
#[derive(Clone, Debug)]
pub struct Description {
    /// The id of the board type, which other documents name.
    pub id: String,
    pub name: String,
    pub sources: Vec<Source>,
    pub console: ConsoleSettings,
    pub cold_boot: ColdBoot,
    pub markers: Vec<NamedLine>,
    pub boot_mode: BootMode,
    pub tftp: Tftp,
    pub firmware_versions: Vec<NamedLine>,
    /// The ways of reading every identity field, in the order of
    /// [`Field::ALL`].
    pub identity: Vec<Read>,
    pub variants: Vec<Variant>,
    pub min_bootloader: MinBootloader,
    pub write_triggers: Vec<Trigger>,
    pub refused_properties: Option<RefusedProperties>,
    pub sensitive: Vec<NamedLine>,
    /// Values never hidden, even where a sensitive line holds them.
    pub never: Vec<String>,
}

/// Why a board description cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoardError {
    /// The text is not TOML, or a key or value does not have the form of the
    /// format.
    Syntax(String),
    /// The document is not a board description of this version.
    Format(Header),
    /// A value breaks a rule of the format; `at` names the value.
    Invalid { at: String, reason: String },
}

/// The result of reading part of a board description.
type Checked<T> = Result<T, BoardError>;

fn invalid(at: impl Into<String>, reason: impl Into<String>) -> BoardError {
    BoardError::Invalid {
        at: at.into(),
        reason: reason.into(),
    }
}

impl Board {
    /// Reads and checks a board description.
    pub fn parse(text: &str) -> Result<Self, BoardError> {
        let syntax = |error: toml::de::Error| BoardError::Syntax(error.to_string());
        let mut table: toml::Table = toml::from_str(text).map_err(syntax)?;
        let header = Header::take(&mut table).map_err(syntax)?;
        if !header.is(FORMAT, FORMAT_VERSION) {
            return Err(BoardError::Format(header));
        }
        for (key, value) in &table {
            check! { key.as_str() => bounded(value), "a list or table has at most 100 items"; }
        }
        let mut raw: RawBoard = table.try_into().map_err(syntax)?;
        let sources = std::mem::take(&mut raw.sources);
        let description = Builder::new(sources)?.build(raw)?;
        let text = text.to_string();
        Ok(Self { text, description })
    }

    /// The text the description was read from.
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl Deref for Board {
    type Target = Description;

    fn deref(&self) -> &Description {
        &self.description
    }
}

impl Description {
    /// The marker called `name`.
    pub fn marker(&self, name: &str) -> Option<&NamedLine> {
        self.markers.iter().find(|marker| marker.name == name)
    }

    /// The firmware version line called `name`.
    pub fn firmware_version(&self, name: &str) -> Option<&NamedLine> {
        self.firmware_versions.iter().find(|line| line.name == name)
    }

    /// The ways of reading `field`: those for its part and those for every
    /// part.
    pub fn reads(&self, field: FieldRef) -> impl Iterator<Item = &Read> {
        self.identity.iter().filter(move |read| {
            read.field == field.field && (read.part.is_none() || read.part == field.part)
        })
    }

    /// The write triggers, without their sources.
    pub fn triggers(&self) -> Vec<WriteTrigger> {
        let triggers = self.write_triggers.iter();
        triggers.map(|listed| listed.trigger.clone()).collect()
    }
}

/// Builds a [`Description`], checking every rule and recording citations.
struct Builder {
    sources: Vec<Source>,
    cited: BTreeSet<String>,
}

impl Builder {
    fn new(sources: Vec<Source>) -> Checked<Self> {
        let mut seen = BTreeSet::new();
        for source in &sources {
            let at = format!("sources.{}", source.id);
            let address = source.url.strip_prefix("https://").unwrap_or_default();
            let url = !address.is_empty() && source.url.bytes().all(|byte| byte.is_ascii_graphic());
            check! {
                "sources" => is_name(&source.id), format!("{:?}: {NOT_A_NAME}", source.id);
                &at => seen.insert(&source.id), "the id is used twice";
                &at => is_line_text(&source.title), "the title is a line of text";
                &at => is_line_text(&source.version), "the version is a line of text";
                &at => url, "the url is an https address without spaces";
            }
        }
        let cited = BTreeSet::new();
        Ok(Self { sources, cited })
    }

    /// Records the citations of `ids`, each of which must be a listed source.
    fn cite(&mut self, at: &str, ids: &[String]) -> Checked<()> {
        for id in ids {
            check! {
                at => self.sources.iter().any(|source| &source.id == id),
                    format!("{id} is not listed in sources");
            }
            self.cited.insert(id.clone());
        }
        Ok(())
    }

    fn build(mut self, raw: RawBoard) -> Checked<Description> {
        let (console, cold_boot) = (raw.console, raw.cold_boot);
        let (order, off_interval) = (&cold_boot.order, cold_boot.off_interval_ms);
        // With four steps, four distinct steps are each step once.
        let distinct = |(index, step): (usize, &Step)| !order[..index].contains(step);
        check! {
            "id" => is_name(&raw.id), NOT_A_NAME;
            "name" => is_line_text(&raw.name), "the name is a line of text";
            "console.baud" => console.baud > 0, "the baud rate is at least 1";
            "console.data_bits" => (5..=8).contains(&console.data_bits), "5 to 8 data bits";
            "console.stop_bits" => (1..=2).contains(&console.stop_bits), "1 or 2 stop bits";
            "cold_boot.order" => order.len() == 4, "the order lists each of the four steps once";
            "cold_boot.order" => order.iter().enumerate().all(distinct), "a step is listed twice";
            "cold_boot.order" => order.last() == Some(&Step::PowerOn), "power_on is the last step";
            "cold_boot.off_interval_ms" => (1..=MAX_OFF_INTERVAL_MS).contains(&off_interval),
                "the off interval is 1 ms to 1 h";
        }
        self.cite("console.sources", &console.sources)?;
        let markers = self.named("markers", raw.markers)?;
        let boot_mode = self.boot_mode(raw.boot_mode, &markers)?;
        let tftp = raw.tftp;
        check! { "tftp.servers" => !tftp.servers.is_empty(), "at least one server line is listed"; }
        let mut servers = Vec::with_capacity(tftp.servers.len());
        for (index, server) in tftp.servers.into_iter().enumerate() {
            let at = format!("tftp.servers[{index}]");
            servers.push(line_form(&at, server.line, server.example, &["server"])?);
        }
        let (read, failed) = (tftp.read, tftp.read_failed);
        let tftp = Tftp {
            servers,
            read: line_form("tftp.read", read.line, read.example, &["path", "bytes"])?,
            read_failed: line_form("tftp.read_failed", failed.line, failed.example, &["path"])?,
        };
        let firmware_versions = self.named("firmware_versions", raw.firmware_versions)?;
        let identity = self.identity(raw.identity, &firmware_versions)?;
        for (index, variant) in raw.variants.iter().enumerate() {
            let at = format!("variants[{index}]");
            let same =
                |other: &Variant| other.field == variant.field && other.value == variant.value;
            check! {
                &at => variant.field.field.kind() == Kind::Text, TEXT_FIELD;
                &at => is_known(&variant.value), KNOWN;
                &at => !raw.variants[..index].iter().any(same),
                    "the field and value are listed twice";
            }
            check_payload(&at, &variant.payload)?;
            self.cite(&at, &variant.sources)?;
        }
        let min_bootloader = self.min_bootloader(raw.min_bootloader)?;
        let mut write_triggers = Vec::with_capacity(raw.write_triggers.len());
        for (index, listed) in raw.write_triggers.into_iter().enumerate() {
            let at = format!("write_triggers[{index}]");
            self.cite(&at, std::slice::from_ref(&listed.source))?;
            let trigger = WriteTrigger::new(listed.class, listed.files)
                .map_err(|error| invalid(&at, error.to_string()))?;
            let source = listed.source;
            write_triggers.push(Trigger { trigger, source });
        }
        let refused_properties = raw
            .refused_properties
            .map(|refused| self.refused_properties(refused))
            .transpose()?;
        let sensitive = self.named("sensitive", raw.sensitive)?;
        let never = raw.deidentify.map(|raw| raw.never).unwrap_or_default();
        let mut seen = BTreeSet::new();
        check! {
            "deidentify.never" => never.iter().all(|value| !value.is_empty() && seen.insert(value)),
                "the values are distinct and not empty";
        }
        for source in &self.sources {
            let cited = self.cited.contains(&source.id);
            check! { format!("sources.{}", source.id) => cited, "the source is not cited"; }
        }
        Ok(Description {
            id: raw.id,
            name: raw.name,
            sources: self.sources,
            console,
            cold_boot,
            markers,
            boot_mode,
            tftp,
            firmware_versions,
            identity,
            variants: raw.variants,
            min_bootloader,
            write_triggers,
            refused_properties,
            sensitive,
            never,
        })
    }

    /// Reads the named lines of `list`, whose names differ; markers need no
    /// named group, and only firmware version lines cite sources.
    fn named(&mut self, list: &str, raw: Vec<RawNamed>) -> Checked<Vec<NamedLine>> {
        let mut lines: Vec<NamedLine> = Vec::with_capacity(raw.len());
        for line in raw {
            let at = format!("{list}.{}", line.name);
            let form = line_form(&at, line.line, line.example, &[])?;
            let source = form.pattern.as_str();
            check! {
                &at => is_name(&line.name), NOT_A_NAME;
                &at => !lines.iter().any(|known| known.name == line.name), "the name is used twice";
                &at => list == "markers" || !form.pattern.group_names().is_empty(),
                    "the pattern has at least one named group";
                &at => list == "firmware_versions" || line.sources.is_none(),
                    "only firmware version lines cite sources";
                &at => list != "sensitive" || !(source.starts_with('^') || source.ends_with('$')),
                    "a sensitive line is not anchored with ^ or $";
            }
            let sources = line.sources.unwrap_or_default();
            self.cite(&at, &sources)?;
            let name = line.name;
            lines.push(NamedLine {
                name,
                form,
                sources,
            });
        }
        Ok(lines)
    }

    fn boot_mode(&mut self, raw: RawBootMode, markers: &[NamedLine]) -> Checked<BootMode> {
        let form = line_form("boot_mode", raw.line, raw.example, &["mode", "code"])?;
        let network = |marker: &NamedLine| {
            let of_network = |line: &String| {
                form.pattern.group(line.as_bytes(), "mode") == Some(raw.network.as_bytes())
            };
            marker.name == raw.network_marker && marker.form.examples.iter().all(of_network)
        };
        check! {
            "boot_mode.network" => is_line_text(&raw.network), "the mode is a line of text";
            "boot_mode.network_marker" => markers.iter().any(network),
                "the marker's examples are boot mode lines of the network mode";
        }
        self.cite("boot_mode.sources", &raw.sources)?;
        Ok(BootMode {
            form,
            network: raw.network,
            network_marker: raw.network_marker,
            sources: raw.sources,
        })
    }

    fn identity(&mut self, mut raw: RawIdentity, versions: &[NamedLine]) -> Checked<Vec<Read>> {
        let mut reads: Vec<Read> = Vec::new();
        for field in Field::ALL {
            let at = format!("identity.{field}");
            let Some(listed) = raw.remove(field.name()) else {
                return Err(invalid(at, "the field is missing"));
            };
            let first = reads.len();
            for (index, raw) in listed.read.into_iter().enumerate() {
                let at = format!("{at}.read[{index}]");
                let read = check_read(&at, field, raw, versions)?;
                self.cite(&at, &read.sources)?;
                reads.push(read);
            }
            for value in field.values() {
                let of_value = |read: &Read| read.part.is_none() || read.part == value.part;
                check! {
                    &at => reads[first..].iter().any(of_value),
                        format!("no way of reading {value} is listed");
                }
            }
        }
        match raw.into_keys().next() {
            Some(key) => Err(invalid(format!("identity.{key}"), "there is no such field")),
            None => Ok(reads),
        }
    }

    fn min_bootloader(&mut self, rules: MinBootloader) -> Checked<MinBootloader> {
        let releases = &rules.releases;
        let increasing = |pair: &[FirstRelease]| {
            pair[0].version < pair[1].version && pair[0].date < pair[1].date
        };
        check! {
            "min_bootloader.releases" => releases.windows(2).all(increasing),
                "the versions and the dates increase";
            "min_bootloader.releases" => releases.first().is_none_or(|first| first.version > 0),
                "the versions start from 1";
        }
        self.cite("min_bootloader.sources", &rules.sources)?;
        for (index, bound) in rules.bounds.iter().enumerate() {
            let at = format!("min_bootloader.bounds[{index}]");
            let mut seen = BTreeSet::new();
            check! {
                &at => bound.field.field.kind() == Kind::Text, TEXT_FIELD;
                &at => !bound.any_of.is_empty(), "the bound lists at least one value";
                &at => bound.any_of.iter().all(|value| is_known(value)), KNOWN;
                &at => bound.any_of.iter().all(|value| seen.insert(value.to_ascii_lowercase())),
                    "the values differ ignoring ASCII case";
            }
            self.cite(&at, std::slice::from_ref(&bound.source))?;
        }
        Ok(rules)
    }

    fn refused_properties(&mut self, raw: RawRefused) -> Checked<RefusedProperties> {
        let at = "refused_properties";
        // The files are matched ignoring the case of ASCII letters.
        let files = line_form(at, format!("(?i){}", raw.files), raw.example, &[])?;
        for example in &files.examples {
            PayloadName::parse(example)
                .map_err(|error| invalid(at, format!("{example:?}: {error}")))?;
        }
        check! { at => !raw.properties.is_empty(), "at least one property is listed"; }
        for (index, property) in raw.properties.iter().enumerate() {
            let at = format!("refused_properties.{}", property.name);
            let same = |other: &RefusedProperty| other.name.eq_ignore_ascii_case(&property.name);
            check! {
                &at => write::is_property_name(&property.name),
                    "a property name is ASCII letters, digits and _";
                &at => !raw.properties[..index].iter().any(same), "the property is listed twice";
            }
            self.cite(&at, std::slice::from_ref(&property.source))?;
        }
        let properties = raw.properties;
        Ok(RefusedProperties { files, properties })
    }
}

/// Checks one way of reading `field`, whose citations the caller records.
fn check_read(at: &str, field: Field, read: RawRead, versions: &[NamedLine]) -> Checked<Read> {
    let digits = |value: &[u8]| !value.is_empty() && value.iter().all(u8::is_ascii_digit);
    let (part, method, sources) = match read {
        RawRead::FirmwareLog(raw) => {
            let form = line_form(at, raw.line, raw.example, &["value"])?;
            let number = |line: &String| {
                form.pattern
                    .group(line.as_bytes(), "value")
                    .is_some_and(digits)
            };
            check! {
                at => raw.value.is_none(), "a firmware_log read has no value";
                at => field.kind() != Kind::Number || form.examples.iter().all(number),
                    "the value of every example is a number";
            }
            (raw.part, Method::FirmwareLog { form }, raw.sources)
        }
        RawRead::FirmwareLogPresence(raw) => {
            let form = line_form(at, raw.line, raw.example, &[])?;
            let value = raw.value.unwrap_or_default();
            check! { at => is_known(&value), KNOWN; }
            let method = Method::FirmwareLogPresence { form, value };
            (raw.part, method, raw.sources)
        }
        RawRead::FirmwareVersion(raw) => {
            let (name, group) = (raw.name, raw.group);
            let version = versions.iter().find(|version| version.name == name);
            check! {
                at => version.is_some_and(|version| version.form.pattern.has_group(&group)),
                    format!("no firmware version {name} has the group {group}");
            }
            let method = Method::FirmwareVersion { name, group };
            (raw.part, method, raw.sources)
        }
        RawRead::TftpReads(raw) => {
            check_payload(at, &raw.files)?;
            (None, Method::TftpReads { files: raw.files }, raw.sources)
        }
        RawRead::Visual(raw) => (raw.part, Method::Visual { how: raw.how }, raw.sources),
        RawRead::Software(raw) => (raw.part, Method::Software { how: raw.how }, raw.sources),
    };
    let how = match &method {
        Method::Visual { how } | Method::Software { how } => is_line_text(how),
        _ => true,
    };
    let parts = field.parts();
    let known = part.as_deref().is_none_or(|part| parts.contains(&part));
    check! {
        at => how, "how is a line of text";
        at => method.kind().reads(field.kind()),
            format!("{} cannot read a value of {field}", method.kind());
        at => known, format!("{field} has no part {}", part.unwrap_or_default());
    }
    let part = part.and_then(|part| parts.iter().copied().find(|known| *known == part));
    Ok(Read {
        field,
        part,
        method,
        sources,
    })
}

/// Compiles a line pattern and checks that it has `groups` and matches each
/// of its examples, of which there is at least one.
fn line_form(at: &str, line: String, examples: Examples, groups: &[&str]) -> Checked<LineForm> {
    let examples = match examples {
        Examples::One(example) => vec![example],
        Examples::Many(examples) => examples,
    };
    check! { at => !examples.is_empty(), "the pattern has at least one example"; }
    let pattern = LinePattern::with_examples(&line, &examples, groups)
        .map_err(|error| invalid(at, error.to_string()))?;
    Ok(LineForm { pattern, examples })
}

const TEXT_FIELD: &str = "the field holds a text";
const KNOWN: &str = "a value is a line of text other than unknown in any ASCII case";

/// Whether `value` is a line of text other than `unknown` in any ASCII case.
fn is_known(value: &str) -> bool {
    is_line_text(value) && !value.eq_ignore_ascii_case(UNKNOWN)
}

/// Checks that `payload` lists files that one directory can hold.
fn check_payload(at: &str, payload: &[PayloadName]) -> Checked<()> {
    check! { at => !payload.is_empty(), "at least one payload file is listed"; }
    names::check_distinct(payload).map_err(|error| invalid(at, error.to_string()))
}

impl fmt::Display for BoardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(message) => write!(f, "cannot read the board description: {message}"),
            Self::Format(found) => write!(f, "{found} is not \"{FORMAT}\" version 1"),
            Self::Invalid { at, reason } => write!(f, "the board description at {at}: {reason}"),
        }
    }
}

impl std::error::Error for BoardError {}

/// One example line, or a list of them.
#[derive(Deserialize)]
#[serde(untagged)]
enum Examples {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBoard {
    id: String,
    name: String,
    #[serde(default)]
    sources: Vec<Source>,
    console: ConsoleSettings,
    cold_boot: ColdBoot,
    #[serde(default)]
    markers: Vec<RawNamed>,
    boot_mode: RawBootMode,
    tftp: RawTftp,
    #[serde(default)]
    firmware_versions: Vec<RawNamed>,
    identity: RawIdentity,
    #[serde(default)]
    variants: Vec<Variant>,
    min_bootloader: MinBootloader,
    #[serde(default)]
    write_triggers: Vec<RawTrigger>,
    refused_properties: Option<RawRefused>,
    #[serde(default)]
    sensitive: Vec<RawNamed>,
    deidentify: Option<RawDeidentify>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNamed {
    name: String,
    line: String,
    example: Examples,
    sources: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBootMode {
    line: String,
    example: Examples,
    network: String,
    network_marker: String,
    #[serde(default)]
    sources: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLine {
    line: String,
    example: Examples,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTftp {
    servers: Vec<RawLine>,
    read: RawLine,
    read_failed: RawLine,
}

/// The ways of reading each identity field, by the name of the field.
type RawIdentity = BTreeMap<String, RawFieldReads>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFieldReads {
    read: Vec<RawRead>,
}

/// A way of reading, named by its `from` key, with the keys it takes.
#[derive(Deserialize)]
#[serde(tag = "from", rename_all = "snake_case")]
enum RawRead {
    FirmwareLog(RawLogRead),
    FirmwareLogPresence(RawLogRead),
    FirmwareVersion(RawVersionRead),
    TftpReads(RawFilesRead),
    Visual(RawHowRead),
    Software(RawHowRead),
}

/// A read of the firmware log; only a read of the presence of a line has a
/// value.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLogRead {
    part: Option<String>,
    line: String,
    value: Option<String>,
    example: Examples,
    #[serde(default)]
    sources: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawVersionRead {
    part: Option<String>,
    name: String,
    group: String,
    #[serde(default)]
    sources: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFilesRead {
    files: Vec<PayloadName>,
    #[serde(default)]
    sources: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHowRead {
    part: Option<String>,
    how: String,
    #[serde(default)]
    sources: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTrigger {
    class: WriteClass,
    files: Vec<String>,
    source: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRefused {
    files: String,
    example: Examples,
    properties: Vec<RefusedProperty>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDeidentify {
    never: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test-only board of the bench crate, with other line forms.
    const SYNTHETIC: &str = include_str!("../../kyto-bench/tests/data/synthetic-board.toml");

    fn pi5() -> Board {
        Board::parse(RASPBERRY_PI_5).unwrap()
    }

    fn date(text: &str) -> ReleaseDate {
        ReleaseDate::parse(text).unwrap()
    }

    fn names<'a>(items: impl IntoIterator<Item = &'a String>) -> Vec<&'a str> {
        items.into_iter().map(String::as_str).collect()
    }

    /// The Pi 5 description with `from`, which occurs exactly once, replaced
    /// by `to`.
    fn edited(from: &str, to: &str) -> String {
        assert_eq!(
            RASPBERRY_PI_5.matches(from).count(),
            1,
            "the description has {from:?} once"
        );
        RASPBERRY_PI_5.replacen(from, to, 1)
    }

    /// Every line pattern of a description with a name for where it is.
    fn line_forms(board: &Board) -> Vec<(String, &LineForm)> {
        let mut forms = Vec::new();
        for (list, lines) in [
            ("markers", &board.markers),
            ("firmware_versions", &board.firmware_versions),
            ("sensitive", &board.sensitive),
        ] {
            for line in lines {
                forms.push((format!("{list}.{}", line.name), &line.form));
            }
        }
        forms.push(("boot_mode".to_string(), &board.boot_mode.form));
        for (index, form) in board.tftp.servers.iter().enumerate() {
            forms.push((format!("tftp.servers[{index}]"), form));
        }
        forms.push(("tftp.read".to_string(), &board.tftp.read));
        forms.push(("tftp.read_failed".to_string(), &board.tftp.read_failed));
        for read in &board.identity {
            if let Method::FirmwareLog { form } | Method::FirmwareLogPresence { form, .. } =
                &read.method
            {
                forms.push((format!("identity.{}", read.field), form));
            }
        }
        forms
    }

    #[test]
    fn raspberry_pi_5_description_parses_and_validates() {
        let board = pi5();
        assert_eq!(board.id, "raspberry-pi-5");
        assert_eq!(board.name, "Raspberry Pi 5");
        assert_eq!(board.text(), RASPBERRY_PI_5);
        assert_eq!(
            board.console,
            ConsoleSettings {
                baud: 115_200,
                data_bits: 8,
                parity: Parity::None,
                stop_bits: 1,
                sources: vec!["bootloader".to_string(), "config-boot".to_string()],
            }
        );
        assert_eq!(
            board.cold_boot,
            ColdBoot {
                order: vec![
                    Step::Publish,
                    Step::WaitOffInterval,
                    Step::OpenConsole,
                    Step::PowerOn
                ],
                off_interval_ms: 5000,
            }
        );
        assert_eq!(
            names(board.markers.iter().map(|marker| &marker.name)),
            [
                "bootsys",
                "bootloader",
                "recovery",
                "network_boot",
                "sd_boot",
                "tftp_timeout",
                "os_start",
                "eeprom_write",
                "eeprom_updated",
            ]
        );
        let mode = &board.boot_mode;
        assert_eq!(mode.network, "NETWORK");
        assert_eq!(mode.network_marker, "network_boot");
        assert_eq!(mode.sources, ["bootloader"]);
        assert_eq!(board.tftp.servers.len(), 3);
        assert_eq!(
            names(board.firmware_versions.iter().map(|version| &version.name)),
            [
                "bootloader",
                "bootsys",
                "bootloader_image_mfg",
                "bl31",
                "bl31_built"
            ]
        );
        // Every field has ways of reading it, in the order of the fields.
        let mut fields: Vec<Field> = board.identity.iter().map(|read| read.field).collect();
        fields.dedup();
        assert_eq!(fields, Field::ALL);
        assert_eq!(
            board
                .write_triggers
                .iter()
                .map(|trigger| (trigger.trigger.class, trigger.trigger.files().to_vec()))
                .collect::<Vec<_>>(),
            [(
                WriteClass::Eeprom,
                [
                    "pieeprom.upd",
                    "pieeprom.sig",
                    "pieeprom.bin",
                    "recovery.bin"
                ]
                .map(String::from)
                .to_vec()
            )]
        );
        assert_eq!(
            names(board.sensitive.iter().map(|line| &line.name)),
            [
                "board-serial",
                "network-link",
                "network-addresses",
                "arp",
                "tftp-request",
                "sd-card-id",
            ]
        );
        assert_eq!(board.never, ["0.0.0.0"]);
        assert_eq!(EMBEDDED, [RASPBERRY_PI_5]);
    }

    #[test]
    fn every_pattern_matches_its_example() {
        for text in [RASPBERRY_PI_5, SYNTHETIC] {
            let board = Board::parse(text).unwrap();
            let forms = line_forms(&board);
            assert!(forms.len() > 10, "{}", board.id);
            for (at, form) in forms {
                assert!(!form.examples.is_empty(), "{at} has an example");
                for example in &form.examples {
                    assert!(
                        form.pattern.is_match(example.as_bytes()),
                        "{at}: {example:?}"
                    );
                }
            }
            if let Some(refused) = &board.refused_properties {
                for example in &refused.files.examples {
                    assert!(
                        refused.files.pattern.is_match(example.as_bytes()),
                        "{example}"
                    );
                }
            }
        }
        // An example that does not match is refused.
        let error = Board::parse(&edited(
            "example = ' 46.63 TFTP: disconnect: timeouts 14'",
            "example = ' 46.63 TFTP: disconnect: timeout 14'",
        ))
        .unwrap_err();
        assert!(
            matches!(&error, BoardError::Invalid { at, .. } if at == "markers.tftp_timeout"),
            "{error}"
        );
    }

    #[test]
    fn every_source_is_official_with_url_and_version() {
        let board = pi5();
        let ids: Vec<&str> = board
            .sources
            .iter()
            .map(|source| source.id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "revision-codes",
                "bootloader",
                "boot-eeprom",
                "config-boot",
                "boot-usb",
                "release-notes",
                "versions",
                "eeprom-update",
                "overlays-readme",
                "pcn-22",
                "pcn-46",
                "pcn-53",
                "pcn-65",
            ]
        );
        for source in &board.sources {
            if let Some(path) = source.url.strip_prefix("https://github.com/raspberrypi/") {
                let commit = &source.version;
                assert!(
                    commit.len() == 40
                        && commit
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                    "{}: a repository is cited at a full commit",
                    source.id
                );
                assert!(
                    path.contains(&format!("/blob/{commit}/")),
                    "{}: the url names the commit",
                    source.id
                );
            } else {
                let path = source
                    .url
                    .strip_prefix("https://pip-assets.raspberrypi.com/categories/1129-pcn/")
                    .unwrap_or_else(|| panic!("{} is an official document", source.id));
                assert!(path.ends_with(".pdf"), "{}", source.id);
                assert!(
                    source.version.starts_with("RP-") && source.version.contains(" release "),
                    "{}: a change notice is cited with its document number and release",
                    source.id
                );
            }
        }
        let releases: Vec<(&str, &str)> = board
            .sources
            .iter()
            .filter(|source| source.id.starts_with("pcn-"))
            .map(|source| (source.id.as_str(), source.version.as_str()))
            .collect();
        assert_eq!(
            releases,
            [
                ("pcn-22", "RP-006041 release 1.1, 2025-04-24"),
                ("pcn-46", "RP-009750 release 5, 2026-10-26"),
                ("pcn-53", "RP-010140 release 1, 2026-06-05"),
                ("pcn-65", "RP-010685 release 1, 2026-10-02"),
            ]
        );
    }

    #[test]
    fn every_identity_field_has_a_read_method() {
        let board = pi5();
        for field in Field::ALL {
            for value in field.values() {
                let methods: Vec<ReadMethod> =
                    board.reads(value).map(|read| read.method.kind()).collect();
                assert!(!methods.is_empty(), "{value}");
            }
        }
        let methods = |text: &str| -> Vec<ReadMethod> {
            board
                .reads(FieldRef::parse(text).unwrap())
                .map(|read| read.method.kind())
                .collect()
        };
        use ReadMethod as M;
        assert_eq!(methods("revision_code"), [M::FirmwareLog, M::Software]);
        assert_eq!(methods("pcb_version"), [M::Visual, M::Software]);
        assert_eq!(methods("soc_stepping"), [M::FirmwareLogPresence, M::Visual]);
        assert_eq!(methods("memory.vendor"), [M::Visual]);
        assert_eq!(methods("memory.part"), [M::Visual]);
        assert_eq!(methods("ethernet_phy"), [M::Visual]);
        assert_eq!(methods("spi_flash.jedec_id"), [M::FirmwareLog]);
        assert_eq!(methods("spi_flash.part"), [M::Visual]);
        assert_eq!(methods("eeprom_version"), [M::FirmwareVersion]);
        assert_eq!(methods("manufacturing_version"), [M::FirmwareLog]);
        assert_eq!(methods("min_boot_version"), [M::Software]);
        assert_eq!(methods("boot_files"), [M::TftpReads]);

        // The JEDEC identifier is read from the log without an official
        // source; the part number cites the change notice.
        let spi: Vec<&Read> = board
            .reads(FieldRef::parse("spi_flash.part").unwrap())
            .collect();
        assert_eq!(spi[0].sources, ["pcn-53"]);
        let jedec: Vec<&Read> = board
            .reads(FieldRef::parse("spi_flash.jedec_id").unwrap())
            .collect();
        assert!(jedec[0].sources.is_empty());
    }

    #[test]
    fn stepping_is_d0_only_when_the_overlay_line_appears() {
        let board = pi5();
        let reads: Vec<&Read> = board
            .reads(FieldRef::parse("soc_stepping").unwrap())
            .collect();
        let presence: Vec<&Read> = reads
            .iter()
            .copied()
            .filter(|read| read.method.kind() == ReadMethod::FirmwareLogPresence)
            .collect();
        assert_eq!(presence.len(), 1, "one line gives the stepping");
        assert_eq!(presence[0].method.presence_value(), Some("D0"));
        let Method::FirmwareLogPresence { form, .. } = &presence[0].method else {
            panic!("a presence read")
        };
        assert!(
            form.pattern
                .is_match(b" 18.33 MESS:00:00:18.331117:0: Loaded overlay 'bcm2712d0'")
        );
        for line in [
            &b" 79.45 MESS:00:01:19.453670:0: Loaded overlay 'vc4-kms-v3d-pi5'"[..],
            b" 79.10 Read /overlays/bcm2712d0.dtbo bytes     1363 hnd 0x239a7",
            b" 18.33 MESS:00:00:18.331117:0: Loaded overlay 'bcm2712d0x'",
        ] {
            assert!(!form.pattern.is_match(line), "{line:?}");
        }
        // No line of the log gives another stepping.
        assert!(reads.iter().all(|read| {
            read.method
                .presence_value()
                .is_none_or(|value| value == "D0")
        }));
    }

    #[test]
    fn mfg_version_is_the_device_value() {
        let board = pi5();
        let reads: Vec<&Read> = board
            .reads(FieldRef::parse("manufacturing_version").unwrap())
            .collect();
        let Method::FirmwareLog { form } = &reads[0].method else {
            panic!("a firmware log read")
        };
        assert_eq!(
            form.pattern
                .group(b"  2.92 MFG ver device 1 image 2", "value"),
            Some(&b"1"[..])
        );
        assert_eq!(
            form.pattern
                .group(b" 77.23 MFG ver device 1 image 2 ", "value"),
            Some(&b"1"[..])
        );
        // The line that every boot prints is the version of the bootloader
        // image, a firmware version and not the board's.
        assert!(!form.pattern.is_match(b"  4.37 MFG_VER: 2"));
        let image = board.firmware_version("bootloader_image_mfg").unwrap();
        assert!(image.form.pattern.is_match(b"  4.37 MFG_VER: 2"));
    }

    #[test]
    fn every_bound_lists_parts() {
        let board = pi5();
        let rules = &board.min_bootloader;
        assert_eq!(
            rules.releases,
            [
                FirstRelease {
                    version: 1,
                    date: date("2026/05/11")
                },
                FirstRelease {
                    version: 2,
                    date: date("2026/09/23")
                },
                FirstRelease {
                    version: 3,
                    date: date("2026/10/23")
                },
            ]
        );
        let bounds: Vec<(String, usize, String, &str)> = rules
            .bounds
            .iter()
            .map(|bound| {
                (
                    bound.field.to_string(),
                    bound.any_of.len(),
                    bound.at_least.to_string(),
                    bound.source.as_str(),
                )
            })
            .collect();
        assert_eq!(
            bounds,
            [
                (
                    "pcb_version".to_string(),
                    1,
                    "2024/03/01".to_string(),
                    "pcn-22"
                ),
                (
                    "pcb_version".to_string(),
                    1,
                    "2026/09/25".to_string(),
                    "pcn-65"
                ),
                (
                    "memory.part".to_string(),
                    15,
                    "2026/09/25".to_string(),
                    "pcn-46"
                ),
                (
                    "spi_flash.part".to_string(),
                    3,
                    "2026/05/05".to_string(),
                    "pcn-53"
                ),
            ]
        );
        assert_eq!(rules.bounds[0].any_of, ["4"]);
        assert_eq!(rules.bounds[1].any_of, ["7"]);
        assert_eq!(
            rules.bounds[3].any_of,
            ["W25Q16RVXHJQ", "GD25Q16EEIGR", "PY25Q16HB-UXH-IR"]
        );
        for any_of in [
            "[]",
            r#"["4", "4"]"#,
            r#"["W25Q", "w25q"]"#,
            r#"["UNKNOWN"]"#,
        ] {
            let text = edited(r#"any_of = ["4"]"#, &format!("any_of = {any_of}"));
            assert!(
                matches!(
                    Board::parse(&text),
                    Err(BoardError::Invalid { at, .. }) if at == "min_bootloader.bounds[0]"
                ),
                "{any_of}"
            );
        }
    }

    #[test]
    fn variants_name_the_device_tree_files_of_each_stepping() {
        let board = pi5();
        let variants: Vec<(String, &str, Vec<&str>)> = board
            .variants
            .iter()
            .map(|variant| {
                (
                    variant.field.to_string(),
                    variant.value.as_str(),
                    variant.payload.iter().map(PayloadName::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            variants,
            [
                (
                    "soc_stepping".to_string(),
                    "C1",
                    vec!["bcm2712-rpi-5-b.dtb"]
                ),
                (
                    "soc_stepping".to_string(),
                    "D0",
                    vec!["bcm2712-rpi-5-b.dtb", "overlays/bcm2712d0.dtbo"]
                ),
            ]
        );
    }

    #[test]
    fn refused_properties_each_cite_their_source() {
        let board = pi5();
        let refused = board.refused_properties.as_ref().unwrap();
        let properties: Vec<(&str, &str)> = refused
            .properties
            .iter()
            .map(|property| (property.name.as_str(), property.source.as_str()))
            .collect();
        assert_eq!(
            properties,
            [
                ("program_pubkey", "config-boot"),
                ("revoke_devkey", "config-boot"),
                ("program_rpiboot_gpio", "config-boot"),
                ("program_jtag_lock", "config-boot"),
                ("eeprom_write_protect", "config-boot"),
                ("program_arb", "release-notes"),
                ("bootloader_allow_mfg_downgrade", "release-notes"),
                ("disable_sd_emmc_boot", "release-notes"),
                ("program_usb_boot_mode", "boot-usb"),
                ("program_usb_boot_timeout", "boot-usb"),
            ]
        );
        // A config.txt can include a file of any name, so every payload
        // file is read.
        for name in [
            "config.txt",
            "CONFIG.TXT",
            "sub/extra.Txt",
            "extra.cfg",
            "config.txt.bak",
            "kernel.img",
            "a",
        ] {
            assert!(refused.files.pattern.is_match(name.as_bytes()), "{name}");
        }
        let names: Vec<String> = refused
            .properties
            .iter()
            .map(|property| property.name.clone())
            .collect();
        let config = PayloadName::parse("config.txt").unwrap();
        let extra = PayloadName::parse("extra.cfg").unwrap();
        let payload = [
            (&config, &b"include extra.cfg\n"[..]),
            (&extra, &b"program_pubkey=1\n"[..]),
        ];
        let hits = write::find_refused_properties(payload, &refused.files.pattern, &names);
        assert_eq!(
            hits,
            [write::PropertyHit {
                file: extra.clone(),
                offset: 0,
                property: "program_pubkey".to_string()
            }]
        );
    }

    #[test]
    fn the_network_marker_is_a_network_boot_mode_line() {
        let board = pi5();
        let mode = &board.boot_mode;
        let marker = board.marker(&mode.network_marker).unwrap();
        for example in &marker.form.examples {
            assert_eq!(
                mode.form.pattern.group(example.as_bytes(), "mode"),
                Some(&b"NETWORK"[..])
            );
            assert_eq!(
                mode.form.pattern.group(example.as_bytes(), "code"),
                Some(&b"02"[..])
            );
        }
        let sd = board.marker("sd_boot").unwrap();
        assert!(!marker.form.pattern.is_match(sd.form.examples[0].as_bytes()));
        // A network marker whose example is another mode is refused.
        let wrong = edited(
            "network_marker = \"network_boot\"",
            "network_marker = \"sd_boot\"",
        );
        assert!(matches!(
            Board::parse(&wrong),
            Err(BoardError::Invalid { at, .. }) if at == "boot_mode.network_marker"
        ));
    }

    #[test]
    fn a_description_that_breaks_a_rule_is_refused() {
        let cases: [(&str, &str, &str); 22] = [
            (
                "line = '^ *[0-9]+\\.[0-9]+ RPi: BOOTSYS release '",
                "line = '^ *[0-9]+\\.[0-9]+ RPi: BOOTSYS release '\nsources = []",
                "markers.bootsys",
            ),
            (
                "how = \"The manufacturer and the part number printed on the memory package\",",
                "how = \"The manufacturer and the part number printed on the memory package\", note = \"x\",",
                "",
            ),
            (
                "{ from = \"visual\", how = \"The manufacturer and the part number printed on the memory package\",",
                "{ how = \"The manufacturer and the part number printed on the memory package\",",
                "",
            ),
            (
                "{ from = \"visual\", how = \"The part number on the Ethernet PHY package;",
                "{ from = \"visual\", part = \"part\", how = \"The part number on the Ethernet PHY package;",
                "identity.ethernet_phy.read[0]",
            ),
            (
                "name = \"Raspberry Pi 5\"",
                "name = \"Raspberry Pi 5\"\ncolour = \"green\"",
                "",
            ),
            (
                "source = \"pcn-53\"",
                "source = \"pcn-99\"",
                "min_bootloader.bounds[3]",
            ),
            (
                "order = [\"publish\", \"wait_off_interval\", \"open_console\", \"power_on\"]",
                "order = [\"publish\", \"wait_off_interval\", \"power_on\", \"open_console\"]",
                "cold_boot.order",
            ),
            (
                "order = [\"publish\", \"wait_off_interval\", \"open_console\", \"power_on\"]",
                "order = [\"publish\", \"publish\", \"open_console\", \"power_on\"]",
                "cold_boot.order",
            ),
            (
                "off_interval_ms = 5000",
                "off_interval_ms = 0",
                "cold_boot.off_interval_ms",
            ),
            (
                "name = \"recovery\"",
                "name = \"bootloader\"",
                "markers.bootloader",
            ),
            (
                r"\((?P<code>[0-9a-f]{2})\)",
                r"\(([0-9a-f]{2})\)",
                "boot_mode",
            ),
            (
                "name = \"bootloader\", group = \"date\"",
                "name = \"bootloader\", group = \"day\"",
                "identity.eeprom_version.read[0]",
            ),
            (
                "{ from = \"tftp_reads\", files = [\"bcm2712-rpi-5-b.dtb\", \"overlays/bcm2712d0.dtbo\"] }",
                "{ from = \"visual\", how = \"Look at the files\" }",
                "identity.boot_files.read[0]",
            ),
            (
                "{ name = \"revoke_devkey\", source = \"config-boot\" }",
                "{ name = \"revoke-devkey\", source = \"config-boot\" }",
                "refused_properties.revoke-devkey",
            ),
            (
                "files = [\"pieeprom.upd\", \"pieeprom.sig\", \"pieeprom.bin\", \"recovery.bin\"]",
                "files = [\"pieeprom.upd\", \"sub/pieeprom.sig\"]",
                "write_triggers[0]",
            ),
            (
                "payload = [\"bcm2712-rpi-5-b.dtb\"]",
                "payload = [\"bcm2712-rpi-5-b.dtb\", \"BCM2712-RPI-5-B.DTB\"]",
                "variants[0]",
            ),
            (
                "{ version = 2, date = \"2026/09/23\" }",
                "{ version = 2, date = \"2026/05/01\" }",
                "min_bootloader.releases",
            ),
            (
                "line = '^ *[0-9]+\\.[0-9]+ Writing EEPROM$'",
                "line = '^ *\\pN+\\.[0-9]+ Writing EEPROM$'",
                "markers.eeprom_write",
            ),
            (
                "field = \"memory.part\"",
                "field = \"min_boot_version\"",
                "min_bootloader.bounds[2]",
            ),
            ("value = \"C1\"", "value = \"Unknown\"", "variants[0]"),
            (
                "value = \"D0\", example",
                "value = \"UNKNOWN\", example",
                "identity.soc_stepping.read[0]",
            ),
            (
                "{ from = \"visual\", how = \"The manufacturer and the part number printed on the memory package\", sources = [\"pcn-46\"] }",
                "{ from = \"visual\", part = \"vendor\", how = \"The manufacturer printed on the memory package\", sources = [\"pcn-46\"] }",
                "identity.memory",
            ),
        ];
        for (from, to, at) in cases {
            let error = Board::parse(&edited(from, to)).unwrap_err();
            match &error {
                BoardError::Invalid { at: found, .. } => assert_eq!(found, at, "{to}: {error}"),
                BoardError::Syntax(_) => assert_eq!(at, "", "{to}: {error}"),
                BoardError::Format(_) => panic!("{to}: {error}"),
            }
        }
    }

    #[test]
    fn lists_are_bounded_before_any_pattern_is_compiled() {
        let never = vec!["\"0.0.0.0\""; 101].join(", ");
        let long = edited("never = [\"0.0.0.0\"]", &format!("never = [{never}]"));
        let at = "deidentify";
        assert_eq!(
            Board::parse(&long).unwrap_err(),
            invalid(at, "a list or table has at most 100 items")
        );
        // 101 sensitive lines, none of which compiles, are refused by their number.
        let line = "[[sensitive]]\nname = \"s\"\nline = '('\nexample = 'x'\n\n".repeat(101);
        let many = edited(
            "[[sensitive]]\nname = \"board-serial\"",
            &format!("{line}[[sensitive]]\nname = \"board-serial\""),
        );
        assert_eq!(
            Board::parse(&many).unwrap_err(),
            invalid("sensitive", "a list or table has at most 100 items")
        );
    }

    #[test]
    fn a_sensitive_line_is_not_anchored() {
        let line = "line = 'CID: (?P<cid>[0-9a-f]{32})'";
        for anchored in [
            "'^CID: (?P<cid>[0-9a-f]{32})'",
            "'CID: (?P<cid>[0-9a-f]{32})$'",
        ] {
            let text = edited(line, &format!("line = {anchored}"));
            assert_eq!(
                Board::parse(&text).unwrap_err(),
                invalid(
                    "sensitive.sd-card-id",
                    "a sensitive line is not anchored with ^ or $"
                )
            );
        }
        // Other lines may be anchored.
        let board = pi5();
        assert!(board.sensitive.iter().all(|line| {
            let source = line.form.pattern.as_str();
            !source.starts_with('^') && !source.ends_with('$')
        }));
        assert!(
            board
                .markers
                .iter()
                .any(|marker| marker.form.pattern.as_str().starts_with('^'))
        );
    }

    #[test]
    fn every_source_is_cited_and_every_citation_is_a_source() {
        let unused = edited(
            "[[sources]]\nid = \"revision-codes\"",
            "[[sources]]\nid = \"unused\"\ntitle = \"Not cited\"\nurl = \"https://example.com/\"\nversion = \"1\"\n\n[[sources]]\nid = \"revision-codes\"",
        );
        assert_eq!(
            Board::parse(&unused).unwrap_err(),
            invalid("sources.unused", "the source is not cited")
        );
        let missing = edited("sources = [\"pcn-46\"]", "sources = [\"pcn-47\"]");
        assert!(matches!(
            Board::parse(&missing),
            Err(BoardError::Invalid { at, reason })
                if at == "identity.memory.read[0]" && reason.contains("pcn-47")
        ));
    }

    #[test]
    fn a_document_of_another_format_or_version_is_named_as_such() {
        let version = edited("format_version = 1", "format_version = 2");
        assert_eq!(
            Board::parse(&version).unwrap_err(),
            BoardError::Format(Header {
                format: FORMAT.to_string(),
                format_version: 2
            })
        );
        assert!(matches!(
            Board::parse("not toml ="),
            Err(BoardError::Syntax(_))
        ));
    }

    #[test]
    fn the_synthetic_board_is_another_board_of_the_same_format() {
        let board = Board::parse(SYNTHETIC).unwrap();
        assert_ne!(board.id, "raspberry-pi-5");
        assert_eq!(
            board.cold_boot,
            ColdBoot {
                order: vec![
                    Step::WaitOffInterval,
                    Step::Publish,
                    Step::OpenConsole,
                    Step::PowerOn
                ],
                off_interval_ms: 2000,
            }
        );
        let fields: BTreeSet<FieldRef> =
            board.variants.iter().map(|variant| variant.field).collect();
        assert_eq!(fields.len(), 1, "one variant field");
        assert!(board.write_triggers.is_empty());
        assert!(board.sources.is_empty());
        // Its lines are not those of the Raspberry Pi 5.
        let pi5 = pi5();
        let network = pi5.marker("network_boot").unwrap();
        let other = board.marker(&board.boot_mode.network_marker).unwrap();
        for example in &other.form.examples {
            assert!(
                !network.form.pattern.is_match(example.as_bytes()),
                "{example}"
            );
        }
    }
}
