//! Run plans: the payloads a run boots and what each cold boot must show
//! (`docs/bench/formats.md`, run plans). A plan is read strictly by
//! [`RunPlan::parse`]; [`RunPlan::check`] checks its references to the board
//! description and to the pinned sets of the instance configuration.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::Deserialize;

use crate::board::Board;
use crate::digest::Sha256;
use crate::document::{Header, NOT_A_NAME, bounded, check, is_line_text, is_name};
use crate::instance::{Pin, PinnedFile};
use crate::names::{self, PayloadName};
use crate::pattern::LinePattern;
use crate::write::WriteEnables;

/// The `format` of a run plan.
pub const FORMAT: &str = "kyto-bench run plan";

/// The `format_version` of a run plan that this tool reads.
pub const FORMAT_VERSION: i64 = 1;

/// The prefix of the names by which an entry refers to the markers of the
/// board description, as in `board.network_boot`.
pub const BOARD_MARKER: &str = "board.";

/// The capture limit of a cold boot when an entry gives none, in bytes.
pub const DEFAULT_CAPTURE_BYTES: u64 = 4 << 20;

/// The largest capture limit of a cold boot, in bytes.
pub const MAX_CAPTURE_BYTES: u64 = 16 << 20;

/// The most line patterns of a plan, `line` and `expect` of every entry.
pub const MAX_PATTERNS: usize = 1000;

const MAX_ITEMS: usize = 100;
const MAX_MS: u64 = 3_600_000;
const MAX_REFERENCE: usize = 1024;
const MAX_SEND: usize = 4096;

/// A run plan. It keeps its text, which receipts record verbatim.
#[derive(Clone, Debug)]
pub struct RunPlan {
    text: String,
    /// The id of the board description.
    pub board: String,
    pub reference: Option<String>,
    pub entries: Vec<Entry>,
}

/// One entry of a run plan: a payload, booted `cold_boots` times, and what
/// each cold boot must show.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub name: String,
    pub reference: Option<String>,
    /// The directory of the payload's source files, below the payload root.
    pub payload_dir: PayloadName,
    /// The payload files with their digests, or `None` when `payload_pin`
    /// names a pinned set of the instance instead; see [`Entry::files`].
    pub payload: Option<Vec<PinnedFile>>,
    pub payload_pin: Option<String>,
    pub cold_boots: u32,
    pub boot_timeout_ms: u64,
    pub end: End,
    #[serde(default = "default_capture_bytes")]
    pub max_capture_bytes: u64,
    /// The expected values of groups of firmware version lines, by the name
    /// of the line and of the group.
    #[serde(default)]
    pub firmware: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    pub markers: Vec<Marker>,
    #[serde(default)]
    pub expect: Vec<Expect>,
    #[serde(default)]
    pub order: Vec<Order>,
    #[serde(default)]
    pub forbid: Vec<Forbid>,
    #[serde(default)]
    pub before_first_send: Vec<String>,
    /// `None` when the entry has no steps; see [`Entry::steps`].
    pub steps: Option<Vec<Step>>,
    /// `false` when the entry has no steps.
    pub sends: Option<bool>,
    #[serde(default)]
    pub values: Vec<NamedValue>,
    #[serde(default)]
    pub segments: Vec<Segment>,
    #[serde(default)]
    pub writes: WriteEnables,
}

/// A line of the payload that the entry names.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Marker {
    pub name: String,
    pub line: LinePattern,
}

/// When a cold boot ends: once a marker has appeared and every step is
/// complete, or at the boot timeout.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum End {
    Marker(EndMarker),
    Timeout(Timeout),
}

/// The marker that ends a cold boot.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndMarker {
    pub marker: String,
}

/// The end of a cold boot at its boot timeout, written `"timeout"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Timeout {
    Timeout,
}

/// How often a marker appears: exactly `count` times, or `min` to `max`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    pub marker: String,
    pub count: Option<u32>,
    pub min: Option<u32>,
    pub max: Option<u32>,
}

impl Expect {
    /// The fewest and the most occurrences allowed; `None` allows any number.
    pub fn range(&self) -> (u32, Option<u32>) {
        match self.count {
            Some(count) => (count, Some(count)),
            None => (self.min.unwrap_or(0), self.max),
        }
    }
}

/// Markers whose first occurrences appear in this order.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Order {
    pub markers: Vec<String>,
}

/// A line that must not appear strictly between the first occurrences of
/// `from` and `to`; an open end is the start or the end of the capture.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Forbid {
    pub line: LinePattern,
    pub from: Option<String>,
    pub to: Option<String>,
}

/// Bytes sent after a marker, and the line that must answer them within
/// `timeout_ms`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub after: String,
    pub send: String,
    pub expect: LinePattern,
    pub timeout_ms: u64,
}

/// A value of the payload recorded in each cold boot: the group `value` of
/// `line`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedValue {
    pub field: String,
    pub line: LinePattern,
}

/// The digest of the bytes after the line of `from` up to the line of `to`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Segment {
    pub name: String,
    pub from: String,
    pub to: String,
    pub sha256: Sha256,
}

/// Why a run plan cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanError {
    /// The text is not TOML or does not have the form of the format.
    Syntax(String),
    /// The document is not a run plan of this version.
    Format(Header),
    /// A value breaks a rule of the format; `at` names the value.
    Invalid { at: String, reason: String },
    /// The plan is for the board `plan`, not that of the description.
    Board { plan: String, board: String },
}

fn invalid(at: impl Into<String>, reason: impl Into<String>) -> PlanError {
    PlanError::Invalid {
        at: at.into(),
        reason: reason.into(),
    }
}

fn default_capture_bytes() -> u64 {
    DEFAULT_CAPTURE_BYTES
}

/// Whether `text` can be a reference: at most 1024 bytes, without C0 control
/// characters or DEL.
fn is_reference(text: &str) -> bool {
    text.len() <= MAX_REFERENCE && !text.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
}

const REFERENCE: &str = "a reference is at most 1024 bytes without control characters";
const ENTRIES: &str = "a plan has 1 to 100 entries";
const TIMEOUT: &str = "the timeout is 1 ms to 1 h";

impl RunPlan {
    /// Reads and checks a run plan on its own.
    pub fn parse(text: &str) -> Result<Self, PlanError> {
        let syntax = |error: toml::de::Error| PlanError::Syntax(error.to_string());
        let mut table: toml::Table = toml::from_str(text).map_err(syntax)?;
        let header = Header::take(&mut table).map_err(syntax)?;
        if !header.is(FORMAT, FORMAT_VERSION) {
            return Err(PlanError::Format(header));
        }
        check_sizes(&table)?;
        let raw: RawPlan = table.try_into().map_err(syntax)?;
        let entries = &raw.entries;
        check! {
            "board" => is_name(&raw.board), NOT_A_NAME;
            "reference" => raw.reference.as_deref().is_none_or(is_reference), REFERENCE;
            "entries" => (1..=MAX_ITEMS).contains(&entries.len()), ENTRIES;
        }
        let mut seen = BTreeSet::new();
        for entry in entries {
            let at = format!("entries.{}", entry.name);
            check! {
                &at => is_name(&entry.name), NOT_A_NAME;
                &at => seen.insert(&entry.name), "the name is used twice";
            }
            entry.check(&at)?;
        }
        let (text, board, reference) = (text.to_string(), raw.board, raw.reference);
        let entries = raw.entries;
        Ok(Self {
            text,
            board,
            reference,
            entries,
        })
    }

    /// The text the plan was read from.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Checks the plan against the board description it names and the
    /// pinned sets of the instance configuration: the plan is for the board,
    /// every `board.<name>` marker and every firmware version line and group
    /// exists in the description, and every `payload_pin` names a set.
    pub fn check(&self, board: &Board, pins: &[Pin]) -> Result<(), PlanError> {
        if self.board != board.id {
            let (plan, board) = (self.board.clone(), board.id.clone());
            return Err(PlanError::Board { plan, board });
        }
        for entry in &self.entries {
            let at = |key: &str| format!("entries.{}.{key}", entry.name);
            for (key, name) in entry.marker_refs() {
                let found = name
                    .strip_prefix(BOARD_MARKER)
                    .is_none_or(|marker| board.marker(marker).is_some());
                check! { at(key) => found, format!("the board has no marker {name}"); }
            }
            for (name, groups) in &entry.firmware {
                let line = board.firmware_version(name);
                let missing = groups
                    .keys()
                    .find(|group| line.is_none_or(|line| !line.form.pattern.has_group(group)));
                check! {
                    at("firmware") => line.is_some(), format!("the board has no firmware version {name}");
                    at("firmware") => missing.is_none(),
                        format!("{name} has no group {}", missing.cloned().unwrap_or_default());
                }
            }
            check! {
                at("payload_pin") => entry.files(pins).is_some(),
                    "the instance configuration pins no set of this name";
            }
        }
        Ok(())
    }
}

impl Entry {
    /// The payload files of the entry: its own, or those of the pinned set
    /// that `payload_pin` names among `pins`.
    pub fn files<'a>(&'a self, pins: &'a [Pin]) -> Option<&'a [PinnedFile]> {
        match &self.payload_pin {
            None => self.payload.as_deref(),
            Some(name) => pins
                .iter()
                .find(|pin| &pin.name == name)
                .map(|pin| &pin.files[..]),
        }
    }

    /// The steps of the entry, in order.
    pub fn steps(&self) -> &[Step] {
        self.steps.as_deref().unwrap_or_default()
    }

    /// Every marker the entry refers to, with the key that refers to it.
    fn marker_refs(&self) -> Vec<(&'static str, &String)> {
        let mut refs = Vec::new();
        if let End::Marker(end) = &self.end {
            refs.push(("end", &end.marker));
        }
        refs.extend(self.expect.iter().map(|expect| ("expect", &expect.marker)));
        let ordered = self.order.iter().flat_map(|order| &order.markers);
        refs.extend(ordered.map(|marker| ("order", marker)));
        let bounds = self
            .forbid
            .iter()
            .flat_map(|forbid| forbid.from.iter().chain(&forbid.to));
        refs.extend(bounds.map(|marker| ("forbid", marker)));
        let sends = self.before_first_send.iter();
        refs.extend(sends.map(|marker| ("before_first_send", marker)));
        refs.extend(self.steps().iter().map(|step| ("steps", &step.after)));
        let ends = self
            .segments
            .iter()
            .flat_map(|segment| [&segment.from, &segment.to]);
        refs.extend(ends.map(|marker| ("segments", marker)));
        refs
    }

    /// Checks the rules that the entry keeps on its own.
    fn check(&self, entry: &str) -> Result<(), PlanError> {
        let at = |key: &str| format!("{entry}.{key}");
        let ms = |ms: u64| (1..=MAX_MS).contains(&ms);
        let mut expected = self.firmware.values().flat_map(BTreeMap::values);
        check! {
            at("reference") => self.reference.as_deref().is_none_or(is_reference), REFERENCE;
            at("payload") => matches!((self.payload.as_deref(), &self.payload_pin), (Some([_, ..]), None) | (None, Some(_))),
                "an entry has either payload files or payload_pin";
            at("payload_pin") => self.payload_pin.as_deref().is_none_or(is_name), NOT_A_NAME;
            at("cold_boots") => (1..=100).contains(&self.cold_boots), "1 to 100 cold boots";
            at("boot_timeout_ms") => ms(self.boot_timeout_ms), TIMEOUT;
            at("max_capture_bytes") => (1..=MAX_CAPTURE_BYTES).contains(&self.max_capture_bytes),
                "the capture limit is 1 byte to 16 MiB";
            at("steps") => matches!((self.steps.as_deref(), self.sends), (Some([_, ..]), None) | (None, Some(false))),
                "an entry has either steps or sends = false";
            at("before_first_send") => self.steps.is_some() || self.before_first_send.is_empty(),
                "an entry without steps sends nothing";
            at("firmware") => expected.all(|value| is_line_text(value)),
                "an expected value is a line of text";
        }
        names::check_distinct(self.payload.iter().flatten().map(|file| &file.name))
            .map_err(|error| invalid(at("payload"), error.to_string()))?;
        let mut markers = BTreeSet::new();
        for marker in &self.markers {
            check! {
                at("markers") => is_name(&marker.name) && markers.insert(&marker.name),
                    format!("{:?}: marker names are distinct names", marker.name);
            }
        }
        for (key, name) in self.marker_refs() {
            let known = match name.strip_prefix(BOARD_MARKER) {
                Some(marker) => is_name(marker),
                None => markers.contains(name),
            };
            check! { at(key) => known, format!("{name} is not a marker of the entry or board.<name>"); }
        }
        for expect in &self.expect {
            let valid = match (expect.count, expect.min, expect.max) {
                (Some(_), None, None) => true,
                (None, Some(min), Some(max)) => min <= max,
                (None, min, max) => min.is_some() || max.is_some(),
                _ => false,
            };
            check! { at("expect") => valid, "an expectation has count, or min and max with min <= max"; }
        }
        for order in &self.order {
            let markers = &order.markers;
            let distinct = |(index, marker)| !markers[..index].contains(marker);
            check! {
                at("order") => markers.len() >= 2 && markers.iter().enumerate().all(distinct),
                    "an order lists at least two distinct markers";
            }
        }
        for step in self.steps() {
            check! {
                at("steps") => (1..=MAX_SEND).contains(&step.send.len()), "a step sends 1 to 4096 bytes";
                at("steps") => ms(step.timeout_ms), TIMEOUT;
            }
        }
        let mut fields = BTreeSet::new();
        for value in &self.values {
            check! {
                at("values") => is_name(&value.field) && fields.insert(&value.field),
                    "the fields of values are distinct names";
                at("values") => value.line.has_group("value"), "the pattern has the group value";
            }
        }
        let mut segments = BTreeSet::new();
        for segment in &self.segments {
            check! {
                at("segments") => is_name(&segment.name) && segments.insert(&segment.name),
                    "the names of segments are distinct names";
            }
        }
        Ok(())
    }
}

/// Checks the bounds of the lists of a plan, and of its line patterns,
/// before any pattern is compiled: at most 100 entries, 100 items in every
/// list and table of an entry, and [`MAX_PATTERNS`] patterns.
fn check_sizes(table: &toml::Table) -> Result<(), PlanError> {
    let entries = table.get("entries").and_then(toml::Value::as_array);
    check! { "entries" => entries.is_none_or(|entries| entries.len() <= MAX_ITEMS), ENTRIES; }
    let mut patterns = 0;
    for entry in entries
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_table)
    {
        let name = entry.get("name").and_then(toml::Value::as_str);
        let list = |key| {
            entry
                .get(key)
                .and_then(toml::Value::as_array)
                .map_or(0, Vec::len)
        };
        patterns += ["markers", "forbid", "steps", "values"]
            .map(list)
            .iter()
            .sum::<usize>();
        check! {
            format!("entries.{}", name.unwrap_or_default()) => entry.values().all(bounded),
                "a list has at most 100 items";
        }
    }
    check! { "entries" => patterns <= MAX_PATTERNS, "a plan has at most 1000 line patterns"; }
    Ok(())
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(message) => write!(f, "cannot read the run plan: {message}"),
            Self::Format(found) => write!(f, "{found} is not \"{FORMAT}\" version 1"),
            Self::Invalid { at, reason } => write!(f, "the run plan at {at}: {reason}"),
            Self::Board { plan, board } => write!(f, "the run plan is for {plan}, not {board}"),
        }
    }
}

impl std::error::Error for PlanError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlan {
    board: String,
    reference: Option<String>,
    entries: Vec<Entry>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::RASPBERRY_PI_5;
    use crate::instance::{Instance, InstanceError};

    /// The example run plan, whose digests are those of the example payload.
    const EXAMPLE: &str = include_str!("../../examples/run-plan.toml");

    fn example() -> RunPlan {
        RunPlan::parse(EXAMPLE).unwrap()
    }

    fn pi5() -> Board {
        Board::parse(RASPBERRY_PI_5).unwrap()
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

    /// Where a plan breaks a rule, read on its own or against the Pi 5 and
    /// `pins`.
    fn invalid_at(text: &str, pins: &[Pin]) -> String {
        let checked = RunPlan::parse(text).and_then(|plan| plan.check(&pi5(), pins));
        match checked {
            Err(PlanError::Invalid { at, .. }) => at,
            other => panic!("{other:?}"),
        }
    }

    fn syntax(text: &str) -> String {
        match RunPlan::parse(text) {
            Err(PlanError::Syntax(message)) => message,
            other => panic!("{other:?}"),
        }
    }

    fn pin(name: &str) -> Pin {
        let file = PinnedFile {
            name: PayloadName::parse("config.txt").unwrap(),
            sha256: Sha256::of(b""),
        };
        Pin {
            name: name.into(),
            files: vec![file],
        }
    }

    #[test]
    fn example_run_plan_parses() {
        let plan = example();
        plan.check(&pi5(), &[]).unwrap();
        assert_eq!(plan.text(), EXAMPLE);
        assert_eq!(
            (plan.board.as_str(), plan.reference.as_deref()),
            ("raspberry-pi-5", Some("example"))
        );
        let [firmware_only, echo] = &plan.entries[..] else {
            panic!("two entries");
        };
        assert_eq!((firmware_only.cold_boots, echo.cold_boots), (3, 1));
        assert_eq!(firmware_only.sends, Some(false));
        assert!(firmware_only.steps().is_empty() && echo.sends.is_none());
        assert_eq!(echo.steps()[0].send, "?\r");
        assert_eq!(echo.max_capture_bytes, DEFAULT_CAPTURE_BYTES);
        assert_eq!(echo.writes, WriteEnables::default());
        assert_eq!(firmware_only.firmware["bootloader"]["date"], "2026/09/25");
        assert_eq!(firmware_only.expect[1].range(), (0, Some(0)));
        let marker = |end: &End| match end {
            End::Marker(end) => end.marker.clone(),
            End::Timeout(_) => String::new(),
        };
        assert_eq!(marker(&echo.end), "ready");
        // The digests are those of the example payload files.
        let files = [
            (
                "firmware-only/config.txt",
                &include_bytes!("../../examples/payload/firmware-only/config.txt")[..],
            ),
            (
                "firmware-only/bcm2712-rpi-5-b.dtb",
                include_bytes!("../../examples/payload/firmware-only/bcm2712-rpi-5-b.dtb"),
            ),
            (
                "firmware-only/overlays/bcm2712d0.dtbo",
                include_bytes!("../../examples/payload/firmware-only/overlays/bcm2712d0.dtbo"),
            ),
            (
                "firmware-only/kernel.img",
                include_bytes!("../../examples/payload/firmware-only/kernel.img"),
            ),
            (
                "echo/config.txt",
                include_bytes!("../../examples/payload/echo/config.txt"),
            ),
            (
                "echo/bcm2712-rpi-5-b.dtb",
                include_bytes!("../../examples/payload/echo/bcm2712-rpi-5-b.dtb"),
            ),
            (
                "echo/overlays/bcm2712d0.dtbo",
                include_bytes!("../../examples/payload/echo/overlays/bcm2712d0.dtbo"),
            ),
            (
                "echo/kernel.img",
                include_bytes!("../../examples/payload/echo/kernel.img"),
            ),
        ];
        let mut checked = 0;
        for entry in &plan.entries {
            for file in entry.files(&[]).unwrap() {
                let path = format!("{}/{}", entry.payload_dir, file.name);
                let (_, bytes) = files.iter().find(|(name, _)| *name == path).unwrap();
                assert!(file.sha256.matches(bytes), "{path}");
                checked += 1;
            }
        }
        assert_eq!(checked, files.len());
        let kernel = |entry: &Entry| entry.files(&[]).unwrap()[3].sha256.clone();
        assert_ne!(kernel(firmware_only), kernel(echo));
    }

    #[test]
    fn run_plan_rejects_unknown_fields() {
        let cases = [
            (
                "reference = \"example\"",
                "reference = \"example\"\ncomment = \"x\"",
            ),
            ("cold_boots = 3", "cold_boots = 3\nretries = 1"),
            (
                "end = { marker = \"started\" }",
                "end = { marker = \"started\", at = 1 }",
            ),
            ("timeout_ms = 1000 }", "timeout_ms = 1000, repeat = 2 }"),
            (
                "{ name = \"ready\", line",
                "{ name = \"ready\", example = \"READY\", line",
            ),
            (
                "3eec655188\" }]",
                "3eec655188\" }]\n[entries.writes]\nflash = true",
            ),
        ];
        for (from, to) in cases {
            let message = syntax(&edited(from, to));
            let refused = message.contains("unknown field") || message.contains("enum End");
            assert!(refused, "{to}: {message}");
        }
        assert!(syntax(&edited("end = { marker = \"ready\" }", "end = \"never\"")).contains("End"));
        let timeout =
            RunPlan::parse(&edited("end = { marker = \"ready\" }", "end = \"timeout\"")).unwrap();
        assert_eq!(timeout.entries[1].end, End::Timeout(Timeout::Timeout));
        let other = edited("format_version = 1", "format_version = 2");
        assert!(
            matches!(RunPlan::parse(&other), Err(PlanError::Format(header)) if header.format_version == 2)
        );
    }

    #[test]
    fn run_plan_rejects_unresolved_markers_groups_and_bad_digests() {
        // A marker the entry does not have, and one the board does not have.
        let at = invalid_at(&edited("after = \"ready\"", "after = \"done\""), &[]);
        assert_eq!(at, "entries.echo.steps");
        let at = invalid_at(
            &edited(
                "\"board.os_start\", \"started\"",
                "\"board.kernel\", \"started\"",
            ),
            &[],
        );
        assert_eq!(at, "entries.firmware-only.order");
        let at = invalid_at(
            &edited("from = \"board.os_start\"", "from = \"started\""),
            &[],
        );
        assert_eq!(at, "entries.echo.segments");
        // A firmware version line or a group the board does not have.
        let at = invalid_at(
            &edited("[entries.firmware.bl31]", "[entries.firmware.bl32]"),
            &[],
        );
        assert_eq!(at, "entries.firmware-only.firmware");
        let at = invalid_at(&edited("hash = \"1accd665\"", "build = \"1accd665\""), &[]);
        assert_eq!(at, "entries.firmware-only.firmware");
        // A named value without the group `value`, a pattern that does not
        // compile and a digest that is not 64 lowercase hex digits.
        let at = invalid_at(&edited("(?P<value>[0-9a-f]{40})", "([0-9a-f]{40})"), &[]);
        assert_eq!(at, "entries.echo.values");
        assert!(syntax(&edited("'^READY$'", "'^READY($'")).contains("cannot be used"));
        assert!(syntax(&edited("498b39e6", "498B39E6")).contains("SHA-256"));
    }

    #[test]
    fn firmware_expectations_name_only_lines_of_the_board() {
        // Every firmware version line of the board, with its first group.
        let board = pi5();
        let tables = board.firmware_versions.iter().map(|version| {
            let group = version.form.pattern.group_names()[0];
            format!("[entries.firmware.{}]\n{group} = \"x\"\n", version.name)
        });
        let start = EXAMPLE.find("[entries.firmware.bootloader]").unwrap();
        let end = EXAMPLE.find("\n# One cold boot").unwrap();
        let text = format!(
            "{}{}{}",
            &EXAMPLE[..start],
            tables.collect::<String>(),
            &EXAMPLE[end..]
        );
        let plan = RunPlan::parse(&text).unwrap();
        plan.check(&board, &[]).unwrap();
        assert_eq!(
            plan.entries[0].firmware.len(),
            board.firmware_versions.len()
        );
        let at = invalid_at(
            &edited("[entries.firmware.bl31]", "[entries.firmware.kernel]"),
            &[],
        );
        assert_eq!(at, "entries.firmware-only.firmware");
        let at = invalid_at(
            &edited("date = \"2026/09/25\"", "date = \" 2026/09/25\""),
            &[],
        );
        assert_eq!(at, "entries.firmware-only.firmware");
    }

    #[test]
    fn payload_names_must_differ_ignoring_case() {
        let at = invalid_at(
            &edited(
                "{ name = \"kernel.img\", sha256 = \"7a4b",
                "{ name = \"CONFIG.TXT\", sha256 = \"7a4b",
            ),
            &[],
        );
        assert_eq!(at, "entries.firmware-only.payload");
        let at = invalid_at(
            &edited(
                "{ name = \"kernel.img\", sha256 = \"7a4b",
                "{ name = \"overlays\", sha256 = \"7a4b",
            ),
            &[],
        );
        assert_eq!(at, "entries.firmware-only.payload");
        assert!(
            syntax(&edited(
                "payload_dir = \"echo\"",
                "payload_dir = \"../echo\""
            ))
            .contains(". or ..")
        );
    }

    #[test]
    fn payload_pin_resolves_from_the_instance() {
        let start = EXAMPLE
            .find("payload = [\n  { name = \"config.txt\"")
            .unwrap();
        let end = start + EXAMPLE[start..].find("]\ncold_boots = 3").unwrap() + 2;
        let text = format!(
            "{}payload_pin = \"self-test\"\n{}",
            &EXAMPLE[..start],
            &EXAMPLE[end..]
        );
        let plan = RunPlan::parse(&text).unwrap();
        let pins = [pin("other"), pin("self-test")];
        plan.check(&pi5(), &pins).unwrap();
        let entry = &plan.entries[0];
        assert_eq!(entry.files(&pins), Some(&pins[1].files[..]));
        assert_eq!(entry.files(&pins[..1]), None);
        assert_eq!(
            invalid_at(&text, &pins[..1]),
            "entries.firmware-only.payload_pin"
        );
        // Payload files and a pin together, or neither.
        let both = edited(
            "cold_boots = 3",
            "payload_pin = \"self-test\"\ncold_boots = 3",
        );
        assert_eq!(invalid_at(&both, &pins), "entries.firmware-only.payload");
        let neither = text.replace("payload_pin = \"self-test\"\n", "");
        assert_eq!(invalid_at(&neither, &pins), "entries.firmware-only.payload");
        // An empty list of payload files is written, not left out.
        let empty = text.replace("payload_pin = \"self-test\"\n", "payload = []\n");
        assert_eq!(invalid_at(&empty, &pins), "entries.firmware-only.payload");
        let empty_and_pin = text.replace(
            "payload_pin = \"self-test\"\n",
            "payload = []\npayload_pin = \"self-test\"\n",
        );
        assert_eq!(
            invalid_at(&empty_and_pin, &pins),
            "entries.firmware-only.payload"
        );
    }

    #[test]
    fn references_are_opaque_but_bounded() {
        let reference = |value: &str| {
            edited(
                "reference = \"example\"",
                &format!("reference = \"{value}\""),
            )
        };
        let longest = "r".repeat(1024);
        for value in [longest.as_str(), "", "record 7 \u{e9} #3 'quoted'"] {
            let plan = RunPlan::parse(&reference(value)).unwrap();
            assert_eq!(
                plan.reference.as_deref(),
                Some(value.replace("\\u00e9", "\u{e9}").as_str())
            );
        }
        for value in [
            "r".repeat(1025),
            "a\\tb".into(),
            "a\\nb".into(),
            "a\\u007fb".into(),
            "a\\u0000b".into(),
        ] {
            assert_eq!(invalid_at(&reference(&value), &[]), "reference");
        }
        let entry = edited(
            "name = \"echo\"",
            "name = \"echo\"\nreference = \"a\\u001bb\"",
        );
        assert_eq!(invalid_at(&entry, &[]), "entries.echo.reference");
    }

    #[test]
    fn an_entry_has_either_steps_or_sends_false() {
        let at = invalid_at(&edited("sends = false\n", ""), &[]);
        assert_eq!(at, "entries.firmware-only.steps");
        let at = invalid_at(
            &edited(
                "end = { marker = \"ready\" }",
                "end = { marker = \"ready\" }\nsends = false",
            ),
            &[],
        );
        assert_eq!(at, "entries.echo.steps");
        let at = invalid_at(&edited("sends = false", "sends = true"), &[]);
        assert_eq!(at, "entries.firmware-only.steps");
        // `sends` is written only as false, even beside steps.
        let at = invalid_at(
            &edited("name = \"echo\"", "name = \"echo\"\nsends = true"),
            &[],
        );
        assert_eq!(at, "entries.echo.steps");
        let at = invalid_at(
            &edited(
                "sends = false",
                "sends = false\nbefore_first_send = [\"started\"]",
            ),
            &[],
        );
        assert_eq!(at, "entries.firmware-only.before_first_send");
        // An empty list of steps is written, not left out, even beside
        // sends = false.
        let at = invalid_at(&edited("sends = false", "steps = []\nsends = false"), &[]);
        assert_eq!(at, "entries.firmware-only.steps");
        let at = invalid_at(&edited("sends = false\n", "steps = []\n"), &[]);
        assert_eq!(at, "entries.firmware-only.steps");
    }

    #[test]
    fn counts_sizes_and_timeouts_are_bounded() {
        let cases = [
            (
                "cold_boots = 3",
                "cold_boots = 0",
                "entries.firmware-only.cold_boots",
            ),
            (
                "cold_boots = 3",
                "cold_boots = 101",
                "entries.firmware-only.cold_boots",
            ),
            (
                "cold_boots = 1",
                "cold_boots = 1\nmax_capture_bytes = 16777217",
                "entries.echo.max_capture_bytes",
            ),
            ("timeout_ms = 1000", "timeout_ms = 0", "entries.echo.steps"),
            ("send = \"?\\r\"", "send = \"\"", "entries.echo.steps"),
            (
                "{ marker = \"ready\", count = 1 }",
                "{ marker = \"ready\", count = 1, max = 2 }",
                "entries.echo.expect",
            ),
            (
                "{ marker = \"ready\", count = 1 }",
                "{ marker = \"ready\", min = 2, max = 1 }",
                "entries.echo.expect",
            ),
            (
                "{ marker = \"ready\", count = 1 }",
                "{ marker = \"ready\" }",
                "entries.echo.expect",
            ),
            (
                "[\"board.bootloader\", \"board.network_boot\", \"board.os_start\", \"started\"]",
                "[\"started\"]",
                "entries.firmware-only.order",
            ),
            (
                "[\"board.bootloader\", \"board.network_boot\", \"board.os_start\", \"started\"]",
                "[\"started\", \"started\"]",
                "entries.firmware-only.order",
            ),
            (
                "name = \"echo\"",
                "name = \"firmware-only\"",
                "entries.firmware-only",
            ),
        ];
        for (from, to, at) in cases {
            assert_eq!(invalid_at(&edited(from, to), &[]), at, "{to}");
        }
        let at = invalid_at(
            &edited(
                "boot_timeout_ms = 60000\nend = { marker = \"ready\" }",
                "boot_timeout_ms = 3600001\nend = { marker = \"ready\" }",
            ),
            &[],
        );
        assert_eq!(at, "entries.echo.boot_timeout_ms");
        let open = RunPlan::parse(&edited(
            "{ marker = \"ready\", count = 1 }",
            "{ marker = \"ready\", min = 1 }",
        ))
        .unwrap();
        assert_eq!(open.entries[1].expect[0].range(), (1, None));
        let markers = (0..101)
            .map(|n| format!("{{ name = \"m{n}\", line = 'x' }}"))
            .collect::<Vec<_>>();
        let many = edited(
            "markers = [{ name = \"ready\", line = '^READY$' }]",
            &format!(
                "markers = [{{ name = \"ready\", line = '^READY$' }}, {}]",
                markers.join(", ")
            ),
        );
        assert_eq!(invalid_at(&many, &[]), "entries.echo");
        let order = (0..101).map(|n| format!("\"m{n}\"")).collect::<Vec<_>>();
        let long = edited(
            "[\"board.bootloader\", \"board.network_boot\", \"board.os_start\", \"started\"]",
            &format!("[{}]", order.join(", ")),
        );
        assert_eq!(invalid_at(&long, &[]), "entries.firmware-only");
    }

    #[test]
    fn list_and_pattern_bounds_are_checked_before_any_pattern_is_compiled() {
        // Too many markers, none of which compiles: the bound is named.
        let markers = vec!["{ name = \"m\", line = '(' }"; 101].join(", ");
        let many = edited(
            "markers = [{ name = \"ready\", line = '^READY$' }]",
            &format!("markers = [{markers}]"),
        );
        assert_eq!(invalid_at(&many, &[]), "entries.echo");
        // At most 1000 patterns in a plan, counted before the entries are read.
        let entry = |n: usize| {
            let markers = vec!["{ name = \"m\", line = '(' }"; 91].join(", ");
            format!("[[entries]]\nname = \"e{n}\"\nmarkers = [{markers}]\n")
        };
        let head =
            "format = \"kyto-bench run plan\"\nformat_version = 1\nboard = \"raspberry-pi-5\"\n";
        let text = format!("{head}{}", (0..11).map(entry).collect::<String>());
        match RunPlan::parse(&text) {
            Err(PlanError::Invalid { at, reason }) => {
                assert_eq!(
                    (at.as_str(), reason.as_str()),
                    ("entries", "a plan has at most 1000 line patterns")
                );
            }
            other => panic!("{other:?}"),
        }
        let fewer = format!("{head}{}", (0..10).map(entry).collect::<String>());
        assert!(syntax(&fewer).contains("cannot be used"));
        // A short pattern that compiles to more than 64 KiB is refused.
        let large = edited("line = '^READY$'", "line = 'a{25000}'");
        assert!(syntax(&large).contains("size limit"));
    }

    #[test]
    fn board_mismatch_between_plan_and_instance_is_an_input_error() {
        // The documented instance configuration, made one of the synthetic
        // board, beside a plan for the Raspberry Pi 5.
        let page = include_str!("../../../docs/bench/formats.md");
        let start = page
            .find("```toml\nformat = \"kyto-bench instance\"")
            .unwrap()
            + 8;
        let text = &page[start..start + page[start..].find("```").unwrap()];
        let text = text.replace("\"raspberry-pi-5\"", "\"synthetic-board\"");
        let instance = Instance::parse(&text).unwrap();
        let synthetic = include_str!("../../kyto-bench/tests/data/synthetic-board.toml");
        let (pi5, synthetic) = (pi5(), Board::parse(synthetic).unwrap());
        // The plan names the Raspberry Pi 5, for which the instance is not;
        // the description of the instance's board is not the plan's.
        let error = instance.check_board(&pi5).unwrap_err();
        assert!(matches!(error, InstanceError::Board { .. }), "{error}");
        let pins = &instance.parameters().pins;
        let error = example().check(&synthetic, pins).unwrap_err();
        let (plan, board) = ("raspberry-pi-5".into(), "synthetic-board".into());
        assert_eq!(error, PlanError::Board { plan, board });
        let message = "the run plan is for raspberry-pi-5, not synthetic-board";
        assert_eq!(error.to_string(), message);
    }
}
