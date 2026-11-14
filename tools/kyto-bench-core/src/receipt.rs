//! Receipts: the record of a run that the tool writes, one JSON file in the
//! pretty form of [`Receipt::render`] (`docs/bench/formats.md`, receipts).
//! It holds the inputs of judgement verbatim, every event with the time of
//! the bench clock, the raw bytes of every cold boot, and the observations,
//! checks and verdicts. [`Receipt::parse`] reads one strictly: it must be
//! byte for byte the rendering of what it holds, so any edit is refused.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::de::DeserializeOwned;
use serde::ser::{self, SerializeMap as _};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::Value as Json;

use crate::board::Board;
use crate::digest::{Sha256, is_lower_hex};
use crate::document::{Header, check};
use crate::identity::{self, IdentityError, IdentityRecord, ReadMethod, UNKNOWN, Value};
use crate::instance::{Parameters, Quantity, Role};
use crate::lines;
use crate::names::{GenerationName, PayloadName};
use crate::observe::Observations;
use crate::plan::RunPlan;

/// The `format` of a receipt.
pub const FORMAT: &str = "kyto-bench receipt";

/// The `format_version` of a receipt that this tool reads and writes.
pub const FORMAT_VERSION: i64 = 1;

/// The most chunks that the `index` of a cold boot's console output lists.
pub const MAX_CHUNKS: usize = 65_536;

/// The rules that make a receipt not evidence, in the order that
/// `evidence.reasons` lists them: the tool's commit is unknown, its working
/// tree had changes, the bench's devices are simulated.
pub const EVIDENCE_RULES: [&str; 3] = [
    "evidence.commit_unknown",
    "evidence.tree_changed",
    "evidence.simulated",
];

/// The result of a rule evaluated on one subject.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Pass,
    Fail,
    Fault,
}

/// A rule evaluated on one subject, such as a payload file or a marker,
/// with a detail when it did not pass.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub rule: String,
    pub subject: String,
    pub result: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Check {
    /// A check of `rule` on `subject` that passed when `detail` is `None`
    /// and has `result` and `detail` otherwise.
    pub fn new(rule: &str, subject: &str, result: Outcome, detail: Option<String>) -> Self {
        let result = detail.as_ref().map_or(Outcome::Pass, |_| result);
        let (rule, subject) = (rule.into(), subject.into());
        Self {
            rule,
            subject,
            result,
            detail,
        }
    }
}

/// Why a run was refused before any power command: the rule, the index of
/// the entry it concerns, if any, the payload file it found, and what it
/// found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Refusal {
    pub rule: String,
    pub entry: Option<usize>,
    pub subject: String,
    pub detail: String,
}

impl Refusal {
    pub(crate) fn new(rule: &str, entry: usize, subject: &PayloadName, detail: String) -> Self {
        let (rule, entry, subject) = (rule.to_string(), Some(entry), subject.to_string());
        Self {
            rule,
            entry,
            subject,
            detail,
        }
    }
}

/// The verdict of a run, an entry or a cold boot; only an entry is `NOT
/// RUN`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    #[serde(rename = "PASS")]
    Pass,
    #[serde(rename = "FAIL")]
    Fail,
    #[serde(rename = "BENCH FAULT")]
    BenchFault,
    #[serde(rename = "NOT RUN")]
    NotRun,
}

/// The record of a run, with its fields in the order the file has them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub format: String,
    pub format_version: i64,
    /// 128 bits from the operating system, as 32 lowercase hexadecimal
    /// digits.
    pub nonce: String,
    pub tool: Tool,
    pub evidence: Evidence,
    pub bench: Bench,
    pub inputs: Inputs,
    pub reference: Option<String>,
    pub refusal: Option<Refusal>,
    pub start: Start,
    pub entries: Vec<EntryRecord>,
    /// `None` exactly when the run was refused.
    pub verdict: Option<Verdict>,
}

/// The tool's commit and whether its working tree was clean, each `None`
/// when unknown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    pub commit: Option<String>,
    pub clean: Option<bool>,
}

/// Whether the receipt can be evidence, and the reasons it cannot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub eligible: bool,
    pub reasons: Vec<Reason>,
}

/// A rule of [`EVIDENCE_RULES`] that applies, and what it found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reason {
    pub rule: String,
    pub detail: String,
}

/// The bench: its hardware revision and whether its devices are simulated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bench {
    pub hardware_revision: HardwareRevision,
    pub simulated: bool,
}

/// The bench hardware revision: the SHA-256 of the canonical JSON of the
/// devices table of the instance configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HardwareRevision {
    pub kind: RevisionKind,
    pub value: Sha256,
}

/// How the hardware revision is computed; one way so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RevisionKind {
    #[serde(rename = "devices-sha256")]
    DevicesSha256,
}

/// The inputs of judgement: the run plan and the board description
/// verbatim, and the instance parameters with the digest of the file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inputs {
    pub run_plan: Text,
    pub board_description: BoardText,
    pub instance: InstanceRecord,
}

/// A document's text and its SHA-256.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Text {
    pub sha256: Sha256,
    pub text: String,
}

/// A board description's id, SHA-256 and text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardText {
    pub id: String,
    pub sha256: Sha256,
    pub text: String,
}

/// The SHA-256 of the instance configuration file and the parameters it
/// gives; its devices enter only as the hardware revision, its local
/// settings not at all.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceRecord {
    pub sha256: Sha256,
    pub parameters: Parameters,
}

/// The events before the first entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Start {
    pub events: Vec<Event>,
}

/// The record of a run plan entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryRecord {
    pub index: usize,
    pub name: String,
    pub reference: Option<String>,
    pub payload: PayloadRecord,
    pub events: Vec<Event>,
    pub checks: Vec<Check>,
    pub cold_boots: Vec<ColdBootRecord>,
    pub verdict: Verdict,
}

/// The payload of an entry: the pinned set it came from, if any, the
/// generation it was staged as, and its files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayloadRecord {
    pub pin: Option<String>,
    pub generation: Option<GenerationName>,
    pub files: Vec<PayloadFile>,
}

/// A payload file: its size and the SHA-256 of its source, staged and served
/// copies, each `None` when it was not read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayloadFile {
    pub name: PayloadName,
    pub bytes: Option<u64>,
    pub source_sha256: Option<Sha256>,
    pub staged_sha256: Option<Sha256>,
    pub served_sha256: Option<Sha256>,
}

/// The record of one cold boot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColdBootRecord {
    pub index: usize,
    pub events: Vec<Event>,
    pub serial: Serial,
    pub observations: Observations,
    pub checks: Vec<Check>,
    pub verdict: Verdict,
}

/// The bytes received from the console in one cold boot, exactly as read,
/// and the `[offset, at_ns]` of each chunk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Serial {
    pub length: u64,
    pub bytes_base64: Base64,
    pub index: Vec<[u64; 2]>,
}

/// Bytes written in the standard base64 alphabet with padding; only the
/// canonical encoding is read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Base64(pub Vec<u8>);

impl Serialize for Base64 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Base64 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let refused = |error| de::Error::custom(format!("not canonical base64: {error}"));
        STANDARD.decode(text).map(Self).map_err(refused)
    }
}

/// Something that happened at `at_ns` of the bench clock, after `received`
/// bytes of the cold boot's console output. It is written as one object:
/// `at_ns`, `received`, then the fields of [`EventKind`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub at_ns: u64,
    pub received: u64,
    pub event: EventKind,
}

/// What happened, named by the field `event`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum EventKind {
    /// A power command as the switch reported it sent, with the relay of
    /// the instance's power settings, `None` for simulated devices.
    Power {
        command: Command,
        relay: Option<String>,
    },
    /// A reading of the device that confirms the supply state.
    Reading { quantity: Quantity, micro: i64 },
    /// The console is open and keeps every byte from now on.
    ConsoleReady {},
    /// The console is closed.
    ConsoleClosed {},
    /// The bytes of a step, sent to the console.
    Send { step: usize, bytes_base64: Base64 },
    /// The cold boot ended.
    End { reason: EndReason },
    /// The board is served the payload of `generation`.
    Switch { generation: GenerationName },
    /// A device failed.
    DeviceError { role: Role, detail: String },
}

/// A power command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Command {
    On,
    Off,
}

/// Why a cold boot ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    EndCondition,
    BootTimeout,
    StepTimeout,
    ByteLimit,
    ChunkLimit,
    DeviceError,
}

impl Serialize for Event {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let kind = serde_json::to_value(&self.event).map_err(ser::Error::custom)?;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("at_ns", &self.at_ns)?;
        map.serialize_entry("received", &self.received)?;
        for (key, value) in kind.as_object().into_iter().flatten() {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Event {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut map = serde_json::Map::deserialize(deserializer)?;
        let mut take = |key: &'static str| {
            let value = map
                .shift_remove(key)
                .ok_or_else(|| de::Error::missing_field(key))?;
            u64::deserialize(value).map_err(de::Error::custom)
        };
        let (at_ns, received) = (take("at_ns")?, take("received")?);
        let event = EventKind::deserialize(serde_json::Value::Object(map));
        let event = event.map_err(de::Error::custom)?;
        Ok(Self {
            at_ns,
            received,
            event,
        })
    }
}

/// An identity record as receipts write it: its header and board, and each
/// value in the order of [`IdentityRecord::entries`], with `part` `value`
/// for a field with one value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedIdentity {
    pub format: String,
    pub format_version: i64,
    pub board: String,
    pub fields: Vec<RecordedValue>,
}

/// One value of a recorded identity record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedValue {
    pub field: String,
    pub part: String,
    pub value: Written,
    pub read: ReadMethod,
}

/// A value as written: a text, `unknown` included, a number or file names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Written {
    Text(String),
    Number(u32),
    Files(Vec<PayloadName>),
}

impl From<IdentityRecord> for RecordedIdentity {
    fn from(record: IdentityRecord) -> Self {
        let value = |entry: &identity::Entry| RecordedValue {
            field: entry.field.field.name().into(),
            part: entry.field.part_name().into(),
            value: match &entry.value {
                Value::Unknown => Written::Text(UNKNOWN.into()),
                Value::Text(text) => Written::Text(text.clone()),
                Value::Number(number) => Written::Number(*number),
                Value::Files(files) => Written::Files(files.clone()),
            },
            read: entry.read,
        };
        Self {
            format: identity::FORMAT.into(),
            format_version: identity::FORMAT_VERSION,
            board: record.board().into(),
            fields: record.entries().iter().map(value).collect(),
        }
    }
}

impl TryFrom<RecordedIdentity> for IdentityRecord {
    type Error = IdentityError;

    /// Reads the record as an identity record is read; a recorded form that
    /// is not the record's own, such as one with a value twice or out of
    /// order, is refused.
    fn try_from(recorded: RecordedIdentity) -> Result<Self, IdentityError> {
        let header = [
            ("format", recorded.format.clone().into()),
            ("format_version", recorded.format_version.into()),
            ("board", recorded.board.clone().into()),
        ];
        let mut table = toml::Table::from_iter(header.map(|(key, value)| (key.into(), value)));
        for value in &recorded.fields {
            let written = match &value.value {
                Written::Text(text) => toml::Value::from(text.as_str()),
                Written::Number(number) => toml::Value::from(*number),
                Written::Files(files) => files
                    .iter()
                    .map(PayloadName::as_str)
                    .collect::<Vec<_>>()
                    .into(),
            };
            let reading = [("value", written), ("read", value.read.name().into())];
            let reading = toml::Table::from_iter(reading.map(|(key, value)| (key.into(), value)));
            if value.part == "value" {
                table.insert(value.field.clone(), reading.into());
            } else if let toml::Value::Table(parts) = table
                .entry(value.field.clone())
                .or_insert_with(|| toml::Table::new().into())
            {
                parts.insert(value.part.clone(), reading.into());
            }
        }
        let record = IdentityRecord::from_table(table)?;
        if RecordedIdentity::from(record.clone()) != recorded {
            let message = "the fields are not each value of the record once, in order";
            return Err(IdentityError::Syntax(message.into()));
        }
        Ok(record)
    }
}

/// Why a receipt cannot be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReceiptError {
    /// The text is not JSON in UTF-8, or does not have the form of the
    /// format.
    Syntax(String),
    /// The document is not of the format and version read.
    Format(Header),
    /// The text is not the rendering of what it holds: it was edited.
    NotRendered,
    /// A value breaks a rule of the format; `at` is its JSON pointer.
    Invalid { at: String, reason: String },
}

pub(crate) fn invalid(at: impl Into<String>, reason: impl Into<String>) -> ReceiptError {
    ReceiptError::Invalid {
        at: at.into(),
        reason: reason.into(),
    }
}

/// Makes an error of the value at `at` the receipt's.
fn fails<E: fmt::Display>(at: &'static str) -> impl Fn(E) -> ReceiptError {
    move |error| invalid(at, error.to_string())
}

impl Receipt {
    /// The receipt as the tool writes it: the pretty JSON of its fields in
    /// order, with two spaces of indentation and a final newline.
    pub fn render(&self) -> String {
        render_json(self)
    }

    /// Reads a receipt strictly, as [`read_rendered`] reads a document, and
    /// checks the rules of [`Receipt::check`].
    pub fn parse(text: &[u8]) -> Result<Self, ReceiptError> {
        let receipt: Self = read_rendered(text, FORMAT, FORMAT_VERSION)?;
        receipt.check()?;
        Ok(receipt)
    }

    /// Checks the rules that tie the fields of a receipt together: the forms
    /// of the nonce and the commit; `evidence` lists exactly the reasons that
    /// apply and is eligible exactly when it lists none; the recorded texts
    /// have their digests; the recorded plan, description and parameters are
    /// read and checked as a run checks them; the reference, the board and
    /// every entry are those of the run plan, with the payload files of the
    /// plan or of its pinned set in order; a refused run has no run verdict,
    /// no event before the first entry, entries that were not run and
    /// checked nothing, and a refusal that names its entry; every entry not
    /// run has no cold boot, and a cold boot's console output, at most the
    /// capture limit of its entry, is described by its index; a check has a
    /// detail exactly when it did not pass.
    pub fn check(&self) -> Result<(), ReceiptError> {
        let (tool, evidence, inputs) = (&self.tool, &self.evidence, &self.inputs);
        let (plan_text, board) = (&inputs.run_plan.text, &inputs.board_description);
        let (plan_at, parameters) = ("/inputs/run_plan/text", &inputs.instance.parameters);
        let plan = RunPlan::parse(plan_text).map_err(fails(plan_at))?;
        let read = Board::parse(&board.text).map_err(fails("/inputs/board_description/text"))?;
        parameters
            .check(&read)
            .map_err(fails("/inputs/instance/parameters"))?;
        let pins = &parameters.pins;
        plan.check(&read, pins).map_err(fails(plan_at))?;
        let applies = [
            tool.commit.is_none(),
            tool.clean != Some(true),
            self.bench.simulated,
        ];
        let due = EVIDENCE_RULES
            .iter()
            .zip(applies)
            .filter(|(_, applies)| *applies);
        let reasons = evidence.reasons.iter().map(|reason| &reason.rule);
        let refused = self.refusal.is_some();
        check! {
            "/nonce" => is_lower_hex(&self.nonce, 32), "the nonce is 32 lowercase hexadecimal digits";
            "/tool/commit" => tool.commit.as_deref().is_none_or(|commit| is_lower_hex(commit, 40)),
                "a commit is 40 lowercase hexadecimal digits";
            "/evidence/eligible" => evidence.eligible == evidence.reasons.is_empty(),
                "a receipt is eligible exactly when it lists no reason";
            "/evidence/reasons" => reasons.eq(due.map(|(rule, _)| rule)),
                "the reasons are the rules that apply, once each, in order";
            "/inputs/run_plan/sha256" => inputs.run_plan.sha256.matches(plan_text.as_bytes()), DIGEST;
            "/inputs/board_description/sha256" => board.sha256.matches(board.text.as_bytes()), DIGEST;
            "/inputs/board_description/id" => board.id == read.id, "the id of the recorded description";
            "/reference" => self.reference == plan.reference, "the reference of the run plan";
            "/entries" => self.entries.len() == plan.entries.len(), "one entry for each of the plan";
            "/verdict" => refused != self.verdict.is_some(), "exactly one of refusal and verdict";
            "/verdict" => self.verdict != Some(Verdict::NotRun), NOT_RUN;
            "/refusal/entry" => self.refusal.as_ref().is_none_or(|refusal| refusal.entry
                .is_some_and(|entry| entry < self.entries.len())), "the index of an entry";
            "/start/events" => !refused || self.start.events.is_empty(),
                "a run is refused before any power command";
        }
        for (index, (entry, planned)) in self.entries.iter().zip(&plan.entries).enumerate() {
            let at = |key: &str| format!("/entries/{index}/{key}");
            let files = planned
                .files(pins)
                .unwrap_or_default()
                .iter()
                .map(|file| &file.name);
            let not_run = entry.verdict == Verdict::NotRun;
            check! {
                at("index") => entry.index == index, "the index of the entry";
                at("name") => entry.name == planned.name, "the name of the plan's entry";
                at("reference") => entry.reference == planned.reference, "the reference of the plan's entry";
                at("payload/pin") => entry.payload.pin == planned.payload_pin, "the payload_pin of the plan's entry";
                at("payload/files") => files.eq(entry.payload.files.iter().map(|file| &file.name)),
                    "the files of the entry's payload, in order";
                at("verdict") => !refused || (not_run && entry.events.is_empty()), "a refused run runs no entry";
                at("checks") => !refused || entry.checks.is_empty(), "a refused run checks no entry";
                at("cold_boots") => !not_run || entry.cold_boots.is_empty(), "an entry not run has no cold boot";
                at("cold_boots") => entry.cold_boots.len() <= planned.cold_boots as usize,
                    "at most the cold boots of the plan's entry";
            }
            check_results(&at("checks"), &entry.checks)?;
            for (number, cold_boot) in entry.cold_boots.iter().enumerate() {
                let at = |key: &str| format!("/entries/{index}/cold_boots/{number}/{key}");
                let serial = &cold_boot.serial;
                let chunks = lines::chunks(&serial.bytes_base64.0, &serial.index);
                check! {
                    at("index") => cold_boot.index == number, "the index of the cold boot";
                    at("serial/length") => serial.length == serial.bytes_base64.0.len() as u64,
                        "the number of bytes";
                    at("serial/length") => serial.length <= planned.max_capture_bytes,
                        "at most the max_capture_bytes of the plan's entry";
                    at("serial/index") => serial.index.len() <= MAX_CHUNKS, "at most 65536 chunks";
                    at("serial/index") => chunks.is_ok(), chunks.err().map(|error| error.to_string()).unwrap_or_default();
                    at("verdict") => cold_boot.verdict != Verdict::NotRun, NOT_RUN;
                }
                check_results(&at("checks"), &cold_boot.checks)?;
            }
        }
        Ok(())
    }
}

/// Checks that each check of the list at `at` has a detail exactly when it
/// did not pass.
fn check_results(at: &str, checks: &[Check]) -> Result<(), ReceiptError> {
    for (index, check) in checks.iter().enumerate() {
        check! {
            format!("{at}/{index}/detail") => check.detail.is_some() == (check.result != Outcome::Pass),
                "a check has a detail exactly when it did not pass";
        }
    }
    Ok(())
}

/// Reads a JSON document of `format` and `version` in the form of receipts:
/// its `format` and `format_version` are read first; then every field, none
/// unknown or missing; and the text must be the rendering of what it holds.
pub(crate) fn read_rendered<T: Serialize + DeserializeOwned>(
    text: &[u8],
    format: &str,
    version: i64,
) -> Result<T, ReceiptError> {
    let syntax = |error: serde_json::Error| ReceiptError::Syntax(error.to_string());
    let header: Header = serde_json::from_slice(text).map_err(syntax)?;
    if !header.is(format, version) {
        return Err(ReceiptError::Format(header));
    }
    let document: T = serde_json::from_slice(text).map_err(syntax)?;
    if render_json(&document).as_bytes() != text {
        return Err(ReceiptError::NotRendered);
    }
    Ok(document)
}

/// The pretty JSON of `value`, the form of receipts: two spaces of
/// indentation, object keys in the order of their fields, a final newline.
pub(crate) fn render_json(value: &impl Serialize) -> String {
    let json = serde_json::to_string_pretty(value).expect("a document is JSON without map keys");
    json + "\n"
}

/// Calls `visit` with the JSON pointer and the text of every string of
/// `value`, a receipt or a part of one, in the order of the document.
pub(crate) fn walk(value: &mut Json, at: &str, visit: &mut dyn FnMut(&str, &mut String)) {
    match value {
        Json::String(text) => visit(at, text),
        Json::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                walk(item, &format!("{at}/{index}"), visit);
            }
        }
        Json::Object(map) => {
            for (key, item) in map.iter_mut() {
                walk(item, &format!("{at}/{key}"), visit);
            }
        }
        _ => {}
    }
}

/// The last key of the JSON pointer `at`.
pub(crate) fn key(at: &str) -> &str {
    at.rsplit('/').next().unwrap_or_default()
}

/// Whether the field of a receipt at `at` is a digest.
pub(crate) fn is_digest(at: &str) -> bool {
    let key = key(at);
    key == "sha256" || key.ends_with("_sha256") || at == "/bench/hardware_revision/value"
}

/// The bytes of the field of a receipt at `at` when it is a recorded stream:
/// the console output of a cold boot or the bytes of a send.
pub(crate) fn decoded(at: &str, text: &str) -> Option<Vec<u8>> {
    let stream = key(at) == "bytes_base64";
    stream.then(|| STANDARD.decode(text).ok()).flatten()
}

const DIGEST: &str = "the SHA-256 of the recorded text";
const NOT_RUN: &str = "only an entry is NOT RUN";

impl fmt::Display for ReceiptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(message) => write!(f, "cannot read the receipt: {message}"),
            Self::Format(found) => write!(f, "{found} is not a format that this reader reads"),
            Self::NotRendered => write!(f, "the document is not as the tool wrote it"),
            Self::Invalid { at, reason } => write!(f, "the receipt at {at}: {reason}"),
        }
    }
}

impl std::error::Error for ReceiptError {}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::board::{Board, RASPBERRY_PI_5};
    use crate::instance::Instance;
    use crate::observe::{self, ColdBootInputs, Sent};

    /// A run plan of one entry: the cold boot of the worked example of the
    /// formats page.
    pub(crate) const PLAN: &str = r#"format = "kyto-bench run plan"
format_version = 1
board = "raspberry-pi-5"
reference = "example"

[[entries]]
name = "echo"
payload_dir = "echo"
payload = [{ name = "config.txt", sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" }]
cold_boots = 1
boot_timeout_ms = 60000
end = { marker = "ready" }
markers = [{ name = "ready", line = '^READY$' }]
steps = [{ after = "ready", send = "?\r", expect = '^STATS seq=1', timeout_ms = 1000 }]
"#;

    /// The console output of the worked example, and its chunks.
    pub(crate) const SERIAL: &[u8] =
        b"  4.94 Boot mode: NETWORK (02) order f1\r\nREADY\r\nSTATS seq=1\r\n";
    const INDEX: [[u64; 2]; 3] = [[0, 6_100_000_000], [41, 7_600_000_000], [48, 7_700_000_000]];

    /// The example instance configuration of the formats page.
    pub(crate) fn instance_text() -> String {
        let page = include_str!("../../../docs/bench/formats.md");
        let start = page
            .find("```toml\nformat = \"kyto-bench instance\"")
            .unwrap()
            + 8;
        page[start..start + page[start..].find("```").unwrap()].to_string()
    }

    fn event(at_ns: u64, received: u64, event: EventKind) -> Event {
        Event {
            at_ns,
            received,
            event,
        }
    }

    fn power(command: Command) -> EventKind {
        EventKind::Power {
            command,
            relay: None,
        }
    }

    fn reading(micro: i64) -> EventKind {
        let quantity = Quantity::SupplyVoltage;
        EventKind::Reading { quantity, micro }
    }

    fn pass(rule: &str, subject: &str) -> Check {
        let (rule, subject) = (rule.into(), subject.into());
        Check {
            rule,
            subject,
            result: Outcome::Pass,
            detail: None,
        }
    }

    /// The receipt of a run of [`PLAN`] on simulated devices whose one cold
    /// boot is the worked example.
    pub(crate) fn example() -> Receipt {
        let (board, plan) = (
            Board::parse(RASPBERRY_PI_5).unwrap(),
            RunPlan::parse(PLAN).unwrap(),
        );
        let text = instance_text();
        let instance = Instance::parse(&text).unwrap();
        let parameters = instance.parameters().clone();
        let entry = &plan.entries[0];
        let sends = [Sent {
            step: 0,
            event: 3,
            at_ns: 7_600_000_000,
            received: 48,
        }];
        let observations = observe::observe(&ColdBootInputs {
            board: &board,
            entry,
            tftp: &parameters.tftp,
            payload: entry.files(&[]).unwrap(),
            transcript: SERIAL,
            index: &INDEX,
            sends: &sends,
        });
        let empty = Some(Sha256::of(b""));
        let generation = GenerationName::new(1).unwrap();
        let cold_boot = ColdBootRecord {
            index: 0,
            events: vec![
                event(5_100_000_000, 0, EventKind::ConsoleReady {}),
                event(5_100_000_000, 0, power(Command::On)),
                event(5_200_000_000, 0, reading(5_100_000)),
                event(
                    7_600_000_000,
                    48,
                    EventKind::Send {
                        step: 0,
                        bytes_base64: Base64(b"?\r".to_vec()),
                    },
                ),
                event(
                    7_700_000_000,
                    61,
                    EventKind::End {
                        reason: EndReason::EndCondition,
                    },
                ),
                event(7_700_000_000, 61, power(Command::Off)),
                event(7_800_000_000, 61, reading(0)),
                event(7_800_000_000, 61, EventKind::ConsoleClosed {}),
            ],
            serial: Serial {
                length: SERIAL.len() as u64,
                bytes_base64: Base64(SERIAL.to_vec()),
                index: INDEX.to_vec(),
            },
            observations: observations.unwrap(),
            checks: vec![
                pass("boot.network", "board.network_boot"),
                pass("step.response", "0"),
            ],
            verdict: Verdict::Pass,
        };
        let record = |text: &str| Text {
            sha256: Sha256::of(text.as_bytes()),
            text: text.into(),
        };
        let board_text = record(RASPBERRY_PI_5);
        Receipt {
            format: FORMAT.into(),
            format_version: FORMAT_VERSION,
            nonce: "f0e1d2c3b4a5968778695a4b3c2d1e0f".into(),
            tool: Tool {
                commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
                clean: Some(true),
            },
            evidence: Evidence {
                eligible: false,
                reasons: vec![Reason {
                    rule: "evidence.simulated".into(),
                    detail: "the run used simulated devices".into(),
                }],
            },
            bench: Bench {
                hardware_revision: HardwareRevision {
                    kind: RevisionKind::DevicesSha256,
                    value: instance.hardware_revision().clone(),
                },
                simulated: true,
            },
            inputs: Inputs {
                run_plan: record(PLAN),
                board_description: BoardText {
                    id: board.id.clone(),
                    sha256: board_text.sha256,
                    text: board_text.text,
                },
                instance: InstanceRecord {
                    sha256: Sha256::of(text.as_bytes()),
                    parameters,
                },
            },
            reference: Some("example".into()),
            refusal: None,
            start: Start {
                events: vec![
                    event(0, 0, power(Command::Off)),
                    event(100_000_000, 0, reading(0)),
                ],
            },
            entries: vec![EntryRecord {
                index: 0,
                name: "echo".into(),
                reference: None,
                payload: PayloadRecord {
                    pin: None,
                    generation: Some(generation),
                    files: vec![PayloadFile {
                        name: PayloadName::parse("config.txt").unwrap(),
                        bytes: Some(0),
                        source_sha256: empty.clone(),
                        staged_sha256: empty.clone(),
                        served_sha256: empty,
                    }],
                },
                events: vec![event(100_000_000, 0, EventKind::Switch { generation })],
                checks: vec![pass("payload.served_digest", "config.txt")],
                cold_boots: vec![cold_boot],
                verdict: Verdict::Pass,
            }],
            verdict: Some(Verdict::Pass),
        }
    }

    /// The error of parsing the rendering of `receipt`.
    fn parse_error(receipt: &Receipt) -> ReceiptError {
        Receipt::parse(receipt.render().as_bytes()).unwrap_err()
    }

    /// The JSON pointer of the rule that `receipt` breaks.
    fn invalid_at(receipt: &Receipt) -> String {
        match parse_error(receipt) {
            ReceiptError::Invalid { at, .. } => at,
            other => panic!("{other:?}"),
        }
    }

    /// The rendering of the example with `from`, which occurs exactly once,
    /// replaced by `to`.
    fn edited(from: &str, to: &str) -> String {
        let text = example().render();
        assert_eq!(text.matches(from).count(), 1, "{from:?}");
        text.replacen(from, to, 1)
    }

    #[test]
    fn receipt_renders_in_field_order_with_two_spaces_and_a_final_newline() {
        let text = example().render();
        assert!(text.starts_with(
            "{\n  \"format\": \"kyto-bench receipt\",\n  \"format_version\": 1,\n  \"nonce\": "
        ));
        assert!(text.ends_with("\n  \"verdict\": \"PASS\"\n}\n"), "{text}");
        let keys: Vec<&str> = text
            .lines()
            .filter_map(|line| line.strip_prefix("  \"")?.split('"').next())
            .collect();
        let fields = [
            "format",
            "format_version",
            "nonce",
            "tool",
            "evidence",
            "bench",
            "inputs",
            "reference",
            "refusal",
            "start",
            "entries",
            "verdict",
        ];
        assert_eq!(keys, fields);
        let send = concat!(
            "{\n              \"at_ns\": 7600000000,\n              \"received\": 48,\n",
            "              \"event\": \"send\",\n              \"step\": 0,\n",
            "              \"bytes_base64\": \"Pw0=\"\n            }"
        );
        assert!(text.contains(send), "{text}");
        let power =
            "\"event\": \"power\",\n        \"command\": \"off\",\n        \"relay\": null\n";
        assert!(text.contains(power), "{text}");
        let serial = "\"bytes_base64\": \"ICA0Ljk0IEJvb3QgbW9kZTogTkVUV09SSyAoMDIpIG9yZGVyIGYxDQpSRUFEWQ0KU1RBVFMgc2VxPTENCg==\"";
        assert!(text.contains(serial), "{text}");
    }

    #[test]
    fn receipt_round_trips() {
        let receipt = example();
        let text = receipt.render();
        assert_eq!(Receipt::parse(text.as_bytes()), Ok(receipt.clone()));
        let cold_boot = &receipt.entries[0].cold_boots[0];
        assert_eq!(cold_boot.observations.steps[0].response_line, Some(48));
        // The recorded identity record is the instance's.
        let identity = &receipt.inputs.instance.parameters.identity;
        let recorded = RecordedIdentity::from(identity.clone());
        assert_eq!(recorded.fields.len(), 12);
        assert_eq!(IdentityRecord::try_from(recorded).as_ref(), Ok(identity));
        assert!(text.contains("\"part\": \"jedec_id\",\n"), "{text}");
    }

    #[test]
    fn a_receipt_with_an_unknown_field_is_rejected() {
        let places = [
            ("\"nonce\": ", "\"extra\": 1,\n  \"nonce\": "),
            ("\"clean\": true", "\"clean\": true,\n    \"dirty\": false"),
            (
                "\"event\": \"console_ready\"",
                "\"event\": \"console_ready\",\n \"x\": 1",
            ),
            (
                "\"event\": \"switch\",",
                "\"event\": \"switch\",\n \"relay\": null,",
            ),
            (
                "\"step\": 0,\n                \"trigger_line\"",
                "\"step\": 0,\n \"line\": 1,\n                \"trigger_line\"",
            ),
        ];
        for (from, to) in places {
            let error = Receipt::parse(edited(from, to).as_bytes()).unwrap_err();
            assert!(
                matches!(&error, ReceiptError::Syntax(message) if message.contains("unknown field")),
                "{to}: {error}"
            );
        }
        // An unknown kind of event is refused as well.
        let text = edited(
            "\"event\": \"console_closed\"",
            "\"event\": \"console_lost\"",
        );
        assert!(matches!(
            Receipt::parse(text.as_bytes()),
            Err(ReceiptError::Syntax(_))
        ));
    }

    #[test]
    fn a_receipt_with_hand_edited_spacing_or_a_missing_field_is_rejected() {
        let edits = [
            ("\"nonce\": ", "\"nonce\":  "),
            ("\n}\n", "\n}"),
            ("\n}\n", "\n}\n\n"),
            (
                "  \"reference\": \"example\",\n",
                "  \"reference\": \"example\" ,\n",
            ),
            ("  \"refusal\": null,\n", ""),
            ("  \"refusal\": null,\n  \"start\"", "  \"start\""),
        ];
        for (from, to) in edits {
            let text = edited(from, to);
            assert_eq!(
                Receipt::parse(text.as_bytes()),
                Err(ReceiptError::NotRendered),
                "{to:?}"
            );
        }
        let text = edited(
            "\"format_version\": 1,\n  \"nonce\"",
            "\"format_version\": 2,\n  \"nonce\"",
        );
        let error = Receipt::parse(text.as_bytes()).unwrap_err();
        assert!(matches!(
            error,
            ReceiptError::Format(Header {
                format_version: 2,
                ..
            })
        ));
        let mut bytes = example().render().into_bytes();
        bytes.extend_from_slice(b"\xff");
        assert!(matches!(
            Receipt::parse(&bytes),
            Err(ReceiptError::Syntax(_))
        ));
    }

    #[test]
    fn a_receipt_whose_recorded_text_does_not_match_its_digest_is_rejected() {
        let mut receipt = example();
        receipt.inputs.run_plan.text.push('\n');
        assert_eq!(invalid_at(&receipt), "/inputs/run_plan/sha256");
        let mut receipt = example();
        receipt.inputs.board_description.text.push(' ');
        assert_eq!(invalid_at(&receipt), "/inputs/board_description/sha256");
        // A run plan that is not one is refused.
        let mut receipt = example();
        receipt.inputs.run_plan = Text {
            sha256: Sha256::of(b"x = 1"),
            text: "x = 1".into(),
        };
        assert_eq!(invalid_at(&receipt), "/inputs/run_plan/text");
    }

    #[test]
    fn a_receipt_with_non_canonical_base64_is_rejected() {
        let canonical = "\"bytes_base64\": \"Pw0=\"";
        for other in ["Pw1=", "Pw0", "Pw0==", "P w0=", "Pw0=\\n"] {
            let text = edited(canonical, &format!("\"bytes_base64\": \"{other}\""));
            let error = Receipt::parse(text.as_bytes()).unwrap_err();
            assert!(
                matches!(&error, ReceiptError::Syntax(message) if message.contains("base64")),
                "{other}: {error}"
            );
        }
    }

    #[test]
    fn the_serial_index_and_length_describe_the_bytes() {
        let at = "/entries/0/cold_boots/0/serial/";
        let mut receipt = example();
        receipt.entries[0].cold_boots[0].serial.length += 1;
        assert_eq!(invalid_at(&receipt), format!("{at}length"));
        for index in [
            vec![[0, 1], [61, 2]],
            vec![[1, 1]],
            vec![[0, 2], [41, 1]],
            vec![[0, 1], [41, 1], [41, 1]],
            vec![],
        ] {
            let mut receipt = example();
            receipt.entries[0].cold_boots[0].serial.index = index;
            assert_eq!(invalid_at(&receipt), format!("{at}index"));
        }
        // At most 65536 chunks, one byte each at most here.
        let bytes = vec![b'x'; MAX_CHUNKS + 1];
        let index: Vec<[u64; 2]> = (0..bytes.len() as u64)
            .map(|offset| [offset, offset])
            .collect();
        let mut receipt = example();
        receipt.entries[0].cold_boots[0].serial = Serial {
            length: bytes.len() as u64,
            bytes_base64: Base64(bytes),
            index,
        };
        assert_eq!(invalid_at(&receipt), format!("{at}index"));
        receipt.entries[0].cold_boots[0]
            .serial
            .index
            .truncate(MAX_CHUNKS);
        assert!(Receipt::parse(receipt.render().as_bytes()).is_ok());
        // At most the capture limit of the plan's entry.
        let limit = |bytes: u64| {
            let plan = PLAN.replace(
                "cold_boots = 1\n",
                &format!("cold_boots = 1\nmax_capture_bytes = {bytes}\n"),
            );
            let mut receipt = example();
            receipt.inputs.run_plan = Text {
                sha256: Sha256::of(plan.as_bytes()),
                text: plan,
            };
            receipt
        };
        assert!(Receipt::parse(limit(61).render().as_bytes()).is_ok());
        assert_eq!(invalid_at(&limit(60)), format!("{at}length"));
    }

    #[test]
    fn evidence_lists_exactly_the_reasons_that_apply() {
        let mut receipt = example();
        receipt.evidence.eligible = true;
        assert_eq!(invalid_at(&receipt), "/evidence/eligible");
        let mut receipt = example();
        receipt.evidence.reasons.clear();
        receipt.evidence.eligible = true;
        assert_eq!(invalid_at(&receipt), "/evidence/reasons");
        // An unknown commit and a changed tree are reasons, in order.
        let mut receipt = example();
        (receipt.tool.commit, receipt.tool.clean) = (None, Some(false));
        assert_eq!(invalid_at(&receipt), "/evidence/reasons");
        let reason = |rule: &str| Reason {
            rule: rule.into(),
            detail: "found".into(),
        };
        let reasons: Vec<Reason> = EVIDENCE_RULES.iter().map(|rule| reason(rule)).collect();
        receipt.evidence.reasons = reasons.clone();
        receipt.render();
        assert_eq!(
            Receipt::parse(receipt.render().as_bytes()),
            Ok(receipt.clone())
        );
        assert!(receipt.render().contains("\"commit\": null,\n"));
        receipt.evidence.reasons = [&reasons[1], &reasons[0], &reasons[2]]
            .map(Clone::clone)
            .to_vec();
        assert_eq!(invalid_at(&receipt), "/evidence/reasons");
        receipt.evidence.reasons = vec![reason("evidence.other")];
        assert_eq!(invalid_at(&receipt), "/evidence/reasons");
        // A receipt of real devices, a known commit and a clean tree.
        let mut receipt = example();
        receipt.bench.simulated = false;
        receipt.evidence = Evidence {
            eligible: true,
            reasons: Vec::new(),
        };
        assert_eq!(
            Receipt::parse(receipt.render().as_bytes()),
            Ok(receipt.clone())
        );
        // A clean flag that the build could not tell is not a clean tree.
        let mut unknown = receipt.clone();
        unknown.tool.clean = None;
        assert_eq!(invalid_at(&unknown), "/evidence/reasons");
        unknown.evidence = Evidence {
            eligible: false,
            reasons: vec![reason("evidence.tree_changed")],
        };
        assert_eq!(
            Receipt::parse(unknown.render().as_bytes()),
            Ok(unknown.clone())
        );
        assert!(unknown.render().contains("\"clean\": null\n"));
        unknown.evidence.reasons.clear();
        assert_eq!(invalid_at(&unknown), "/evidence/eligible");
        receipt.tool.commit = Some("0123456789ABCDEF0123456789abcdef01234567".into());
        assert_eq!(invalid_at(&receipt), "/tool/commit");
        let mut receipt = example();
        receipt.nonce.pop();
        assert_eq!(invalid_at(&receipt), "/nonce");
    }

    /// The example as the receipt of a run refused before its first power
    /// command.
    fn refused() -> Receipt {
        let mut receipt = example();
        receipt.refusal = Some(Refusal::new(
            "payload.staged_digest",
            0,
            &PayloadName::parse("config.txt").unwrap(),
            "the staged copy of config.txt was not read".into(),
        ));
        receipt.verdict = None;
        receipt.start.events.clear();
        let entry = &mut receipt.entries[0];
        (entry.events, entry.checks, entry.cold_boots) = (Vec::new(), Vec::new(), Vec::new());
        entry.verdict = Verdict::NotRun;
        receipt
    }

    #[test]
    fn a_receipt_has_either_a_refusal_or_a_verdict() {
        let receipt = refused();
        assert_eq!(
            Receipt::parse(receipt.render().as_bytes()),
            Ok(receipt.clone())
        );
        assert!(receipt.render().contains("\"verdict\": null\n}\n"));
        let mut both = refused();
        both.verdict = Some(Verdict::Fail);
        assert_eq!(invalid_at(&both), "/verdict");
        let mut neither = example();
        neither.verdict = None;
        assert_eq!(invalid_at(&neither), "/verdict");
        let mut not_run = example();
        not_run.verdict = Some(Verdict::NotRun);
        assert_eq!(invalid_at(&not_run), "/verdict");
        let mut powered = refused();
        powered.start = example().start;
        assert_eq!(invalid_at(&powered), "/start/events");
        let mut run = refused();
        run.entries[0].verdict = Verdict::Pass;
        assert_eq!(invalid_at(&run), "/entries/0/verdict");
        let mut booted = refused();
        booted.entries[0].cold_boots = example().entries[0].cold_boots.clone();
        assert_eq!(invalid_at(&booted), "/entries/0/cold_boots");
        let mut outside = refused();
        outside.refusal.as_mut().unwrap().entry = Some(1);
        assert_eq!(invalid_at(&outside), "/refusal/entry");
        // Every rule that refuses a run names its entry, and served copies
        // are checked only after the switch.
        let mut unnamed = refused();
        unnamed.refusal.as_mut().unwrap().entry = None;
        assert_eq!(invalid_at(&unnamed), "/refusal/entry");
        let mut checked = refused();
        checked.entries[0].checks = example().entries[0].checks.clone();
        assert_eq!(invalid_at(&checked), "/entries/0/checks");
    }

    #[test]
    fn entries_are_those_of_the_run_plan() {
        type Edit = fn(&mut Receipt);
        let cases: [(&str, Edit); 8] = [
            ("/entries", |r| r.entries.push(r.entries[0].clone())),
            ("/reference", |r| r.reference = None),
            ("/entries/0/index", |r| r.entries[0].index = 1),
            ("/entries/0/name", |r| r.entries[0].name = "other".into()),
            ("/entries/0/reference", |r| {
                r.entries[0].reference = Some("x".into())
            }),
            ("/entries/0/payload/pin", |r| {
                r.entries[0].payload.pin = Some("self-test".into())
            }),
            ("/entries/0/payload/files", |r| {
                r.entries[0].payload.files.clear()
            }),
            ("/entries/0/cold_boots", |r| {
                let again = r.entries[0].cold_boots[0].clone();
                r.entries[0]
                    .cold_boots
                    .push(ColdBootRecord { index: 1, ..again });
            }),
        ];
        for (at, edit) in cases {
            let mut receipt = example();
            edit(&mut receipt);
            assert_eq!(invalid_at(&receipt), at);
        }
        let mut receipt = example();
        receipt.entries[0].cold_boots[0].verdict = Verdict::NotRun;
        assert_eq!(invalid_at(&receipt), "/entries/0/cold_boots/0/verdict");
        receipt.entries[0].cold_boots[0].index = 1;
        assert_eq!(invalid_at(&receipt), "/entries/0/cold_boots/0/index");
        let mut receipt = example();
        receipt.entries[0].checks[0].detail = Some("found".into());
        assert_eq!(invalid_at(&receipt), "/entries/0/checks/0/detail");
        receipt.entries[0].checks[0].result = Outcome::Fault;
        assert_eq!(
            Receipt::parse(receipt.render().as_bytes()),
            Ok(receipt.clone())
        );
        receipt.entries[0].cold_boots[0].checks[1].result = Outcome::Fail;
        assert_eq!(
            invalid_at(&receipt),
            "/entries/0/cold_boots/0/checks/1/detail"
        );
    }

    #[test]
    fn an_entry_of_a_pinned_set_records_the_pin_and_its_files() {
        let pinned = PLAN.replace(
            "payload = [{ name = \"config.txt\", sha256 = \"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\" }]",
            "payload_pin = \"empty-config\"",
        );
        let mut receipt = example();
        receipt.inputs.run_plan = Text {
            sha256: Sha256::of(pinned.as_bytes()),
            text: pinned,
        };
        assert_eq!(invalid_at(&receipt), "/entries/0/payload/pin");
        receipt.entries[0].payload.pin = Some("empty-config".into());
        assert_eq!(
            Receipt::parse(receipt.render().as_bytes()),
            Ok(receipt.clone())
        );
        assert!(receipt.render().contains("\"pin\": \"empty-config\",\n"));
        // The files are those of the pinned set that the receipt records.
        let pins = &mut receipt.inputs.instance.parameters.pins;
        pins[0].files[0].name = PayloadName::parse("cmdline.txt").unwrap();
        assert_eq!(invalid_at(&receipt), "/entries/0/payload/files");
        // A pin that names no recorded set is refused, whatever the files.
        receipt.inputs.instance.parameters.pins.clear();
        assert_eq!(invalid_at(&receipt), "/inputs/run_plan/text");
        receipt.entries[0].payload.files.clear();
        assert_eq!(invalid_at(&receipt), "/inputs/run_plan/text");
    }

    #[test]
    fn recorded_inputs_are_read_and_checked_as_a_run_reads_them() {
        let record = |text: &str| Text {
            sha256: Sha256::of(text.as_bytes()),
            text: text.into(),
        };
        // A board description that is not one, or not of the recorded id.
        let mut receipt = example();
        let text = record("anything at all");
        (
            receipt.inputs.board_description.sha256,
            receipt.inputs.board_description.text,
        ) = (text.sha256, text.text);
        assert_eq!(invalid_at(&receipt), "/inputs/board_description/text");
        let mut receipt = example();
        receipt.inputs.board_description.id = "other".into();
        assert_eq!(invalid_at(&receipt), "/inputs/board_description/id");
        // A plan that names a firmware version line the board does not have.
        let mut receipt = example();
        let plan = PLAN.replace(
            "cold_boots = 1\n",
            "cold_boots = 1\nfirmware.bl32 = { hash = \"1accd665\" }\n",
        );
        receipt.inputs.run_plan = record(&plan);
        assert_eq!(invalid_at(&receipt), "/inputs/run_plan/text");
        // Parameters that an instance configuration could not hold.
        type Edit = fn(&mut Parameters);
        let edits: [Edit; 4] = [
            |p| p.tftp.path_prefix = "ab".into(),
            |p| p.confirmation.consecutive = 0,
            |p| p.confirmation.off_at_most_micro = 9_000_000,
            |p| p.pins[0].files.clear(),
        ];
        for edit in edits {
            let mut receipt = example();
            edit(&mut receipt.inputs.instance.parameters);
            assert_eq!(invalid_at(&receipt), "/inputs/instance/parameters");
        }
    }

    #[test]
    fn a_recorded_identity_record_is_read_as_a_record() {
        let receipt = example();
        let identity = receipt.inputs.instance.parameters.identity.clone();
        let recorded = RecordedIdentity::from(identity);
        let mut swapped = recorded.clone();
        swapped.fields.swap(0, 1);
        let mut twice = recorded.clone();
        twice.fields.push(twice.fields[0].clone());
        let mut number = recorded.clone();
        number.fields[0].value = Written::Number(3);
        let mut part = recorded.clone();
        part.fields[0].part = "vendor".into();
        let mut unknown = recorded.clone();
        unknown.fields[0].value = Written::Text("Unknown".into());
        for refused in [swapped, twice, number, part, unknown] {
            assert!(IdentityRecord::try_from(refused).is_err());
        }
        let text = edited("\"read\": \"software\"", "\"read\": \"by eye\"");
        assert!(matches!(
            Receipt::parse(text.as_bytes()),
            Err(ReceiptError::Syntax(_))
        ));
    }

    #[test]
    fn the_serial_length_is_not_less_than_the_bytes() {
        let mut receipt = example();
        receipt.entries[0].cold_boots[0].serial.length -= 1;
        assert_eq!(
            invalid_at(&receipt),
            "/entries/0/cold_boots/0/serial/length"
        );
    }

    #[test]
    fn a_receipt_with_no_reason_is_eligible() {
        let mut receipt = example();
        receipt.bench.simulated = false;
        receipt.evidence.reasons.clear();
        assert!(!receipt.evidence.eligible);
        assert_eq!(invalid_at(&receipt), "/evidence/eligible");
    }

    #[test]
    fn a_refused_run_records_no_event_of_an_entry() {
        let mut receipt = refused();
        receipt.entries[0].events = example().entries[0].events.clone();
        assert_eq!(invalid_at(&receipt), "/entries/0/verdict");
    }

    #[test]
    fn a_receipt_records_every_entry_of_the_plan() {
        let mut receipt = example();
        receipt.entries.clear();
        assert_eq!(invalid_at(&receipt), "/entries");
    }

    #[test]
    fn an_entry_records_the_pin_of_the_plan_by_name() {
        let pinned = PLAN.replace(
            "payload = [{ name = \"config.txt\", sha256 = \"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\" }]",
            "payload_pin = \"empty-config\"",
        );
        let mut receipt = example();
        receipt.inputs.run_plan = Text {
            sha256: Sha256::of(pinned.as_bytes()),
            text: pinned,
        };
        receipt.entries[0].payload.pin = Some("other-config".into());
        assert_eq!(invalid_at(&receipt), "/entries/0/payload/pin");
    }

    /// The first line of the worked example, its boot mode line, with its
    /// line end.
    const BOOT_MODE: &[u8] = b"  4.94 Boot mode: NETWORK (02) order f1\r\n";

    /// The example with `chunks` as the console output of its cold boot,
    /// received 0.1 s apart from 6.1 s up to the send, which comes after
    /// the chunk of the number `send`, and at 7.7 s from then on, with
    /// observations made from them.
    fn with_chunks(chunks: &[&[u8]], send: usize) -> Receipt {
        let mut receipt = example();
        let (mut bytes, mut index, mut sent) = (Vec::new(), Vec::new(), 0);
        for (number, chunk) in chunks.iter().enumerate() {
            let at = if number <= send {
                6_100_000_000 + number as u64 * 100_000_000
            } else {
                7_700_000_000
            };
            index.push([bytes.len() as u64, at]);
            bytes.extend_from_slice(chunk);
            if number == send {
                sent = bytes.len() as u64;
            }
        }
        let length = bytes.len() as u64;
        let cold_boot = &mut receipt.entries[0].cold_boots[0];
        for event in &mut cold_boot.events {
            event.received = match event.event {
                EventKind::Send { .. } => sent,
                _ if event.received > 0 => length,
                _ => 0,
            };
        }
        cold_boot.serial = Serial {
            length,
            bytes_base64: Base64(bytes),
            index,
        };
        let seen = observed(&receipt);
        receipt.entries[0].cold_boots[0].observations = seen;
        receipt
    }

    /// The observations that the record of the cold boot of `receipt`
    /// gives.
    fn observed(receipt: &Receipt) -> Observations {
        let board = Board::parse(&receipt.inputs.board_description.text).unwrap();
        let plan = RunPlan::parse(&receipt.inputs.run_plan.text).unwrap();
        let entry = &plan.entries[0];
        let cold_boot = &receipt.entries[0].cold_boots[0];
        let serial = &cold_boot.serial;
        let sends = crate::verdict::sends(&cold_boot.events);
        observe::observe(&ColdBootInputs {
            board: &board,
            entry,
            tftp: &receipt.inputs.instance.parameters.tftp,
            payload: entry.files(&[]).unwrap(),
            transcript: &serial.bytes_base64.0,
            index: &serial.index,
            sends: &sends,
        })
        .unwrap()
    }

    /// `receipt` with `bytes` as the console output of its cold boot, its
    /// length theirs, and each offset of its index and the bytes received
    /// before each event moved by `moved`.
    fn replaced(receipt: &Receipt, bytes: Vec<u8>, moved: impl Fn(u64) -> u64) -> Receipt {
        let mut receipt = receipt.clone();
        let cold_boot = &mut receipt.entries[0].cold_boots[0];
        for chunk in &mut cold_boot.serial.index {
            chunk[0] = moved(chunk[0]);
        }
        for event in &mut cold_boot.events {
            event.received = moved(event.received);
        }
        cold_boot.serial.length = bytes.len() as u64;
        cold_boot.serial.bytes_base64 = Base64(bytes);
        receipt
    }

    /// `receipt` with each chunk of the console output of its cold boot
    /// changed by `change`, and its length, its index and the bytes received
    /// before each event made to agree with the changed bytes.
    fn rewritten(receipt: &Receipt, change: impl Fn(&[u8]) -> Vec<u8>) -> Receipt {
        let serial = &receipt.entries[0].cold_boots[0].serial;
        let (mut bytes, mut ends) = (Vec::new(), vec![(0, 0)]);
        for (_, chunk) in lines::chunks(&serial.bytes_base64.0, &serial.index).unwrap() {
            bytes.extend(change(chunk));
            let end = ends.last().map_or(0, |(end, _)| *end) + chunk.len() as u64;
            ends.push((end, bytes.len() as u64));
        }
        let moved = |offset| ends.iter().find(|(end, _)| *end == offset).unwrap().1;
        replaced(receipt, bytes, moved)
    }

    /// `bytes` with every line end made an LF alone: each CR that comes
    /// before an LF, or before such a CR, is left out. With them, for each
    /// offset of `bytes` up to its length, the offset in what is left of
    /// the byte there.
    fn line_ends_lf(bytes: &[u8]) -> (Vec<u8>, Vec<u64>) {
        let mut dropped = vec![false; bytes.len()];
        let mut ends_line = false;
        for (at, byte) in bytes.iter().enumerate().rev() {
            match byte {
                b'\n' => ends_line = true,
                b'\r' => dropped[at] = ends_line,
                _ => ends_line = false,
            }
        }
        let (mut kept, mut offsets) = (Vec::new(), Vec::new());
        for (byte, left_out) in bytes.iter().zip(dropped) {
            offsets.push(kept.len() as u64);
            if !left_out {
                kept.push(*byte);
            }
        }
        offsets.push(kept.len() as u64);
        (kept, offsets)
    }

    /// The offset of the first `line` in `bytes`.
    fn offset(bytes: &[u8], line: &[u8]) -> u64 {
        bytes.windows(line.len()).position(|w| w == line).unwrap() as u64
    }

    #[test]
    fn a_receipt_keeps_bytes_that_are_not_utf8() {
        // A line of every byte value but LF, and one of sequences that are
        // not UTF-8, before READY.
        let every: Vec<u8> = (0..=u8::MAX).filter(|byte| *byte != b'\n').collect();
        let every = [&every[..], b"\r\n"].concat();
        let broken = b"\xc3( \xe2\x82 \xf0\x9f\x92 \xed\xa0\x80 \xc0\xaf \xff\r\n";
        let chunks: [&[u8]; 5] = [BOOT_MODE, &every, broken, b"READY\r\n", b"STATS seq=1\r\n"];
        let bytes = chunks.concat();
        assert!(std::str::from_utf8(&bytes).is_err());
        let receipt = with_chunks(&chunks, 3);
        let recorded = &receipt.entries[0].cold_boots[0].observations;
        assert_eq!(
            recorded.steps[0].response_line,
            Some(offset(&bytes, b"STATS"))
        );
        // Read back from its rendering, the receipt holds the bytes as they
        // arrived, and they give the recorded observations again.
        let read = Receipt::parse(receipt.render().as_bytes()).unwrap();
        assert_eq!(read, receipt);
        assert_eq!(read.entries[0].cold_boots[0].serial.bytes_base64.0, bytes);
        assert_eq!(observed(&read), *recorded);
        // Bytes decoded as UTF-8, with each sequence that is not UTF-8
        // written as U+FFFD, are others: in place of the bytes as read, they
        // break the recorded length.
        let lossy = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned().into_bytes();
        let mut decoded = receipt.clone();
        decoded.entries[0].cold_boots[0].serial.bytes_base64 = Base64(lossy(&bytes));
        assert_ne!(lossy(&bytes), bytes);
        let at = "/entries/0/cold_boots/0/serial/length";
        assert_eq!(invalid_at(&decoded), at);
        // With the length, the index and the events made to agree, the
        // receipt is read, but its lines have moved: the observations that
        // its bytes give are not those it records.
        let decoded = Receipt::parse(rewritten(&receipt, lossy).render().as_bytes()).unwrap();
        let moved = observed(&decoded);
        assert_ne!(moved, *recorded);
        let bytes = &decoded.entries[0].cold_boots[0].serial.bytes_base64.0;
        assert_eq!(moved.steps[0].response_line, Some(offset(bytes, b"STATS")));
    }

    #[test]
    fn a_receipt_keeps_cr_and_lf_as_read() {
        // Lines ended by CR LF, by CR CR LF and by LF alone, an empty line,
        // lone CR bytes inside a line, and a CR and its LF that arrived in
        // two chunks.
        let chunks: [&[u8]; 6] = [
            BOOT_MODE,
            b"progress 1\rprogress 2\r\r\n",
            b"\n",
            b"READY\r",
            b"\n",
            b"STATS seq=1\n",
        ];
        let bytes = chunks.concat();
        let receipt = with_chunks(&chunks, 4);
        let recorded = &receipt.entries[0].cold_boots[0].observations;
        let ready = recorded.markers.iter().find(|m| m.name == "ready");
        assert_eq!(ready.map(|m| m.line), Some(offset(&bytes, b"READY")));
        assert_eq!(
            recorded.steps[0].response_line,
            Some(offset(&bytes, b"STATS"))
        );
        // Read back from its rendering, the receipt holds every CR and LF
        // where it arrived, and they give the recorded observations again.
        let read = Receipt::parse(receipt.render().as_bytes()).unwrap();
        assert_eq!(read, receipt);
        assert_eq!(read.entries[0].cold_boots[0].serial.bytes_base64.0, bytes);
        assert_eq!(observed(&read), *recorded);
        // Line ends normalised to LF over the whole output, the CR and LF
        // that arrived apart among them, in place of the bytes as read,
        // break the recorded length.
        let (normalised, offsets) = line_ends_lf(&bytes);
        assert!(!normalised.windows(2).any(|pair| pair == b"\r\n"));
        assert_eq!(normalised.iter().filter(|byte| **byte == b'\r').count(), 1);
        let mut changed = receipt.clone();
        let serial = &mut changed.entries[0].cold_boots[0].serial;
        serial.bytes_base64 = Base64(normalised.clone());
        assert_eq!(
            invalid_at(&changed),
            "/entries/0/cold_boots/0/serial/length"
        );
        // With the length, the index and the events made to agree, the
        // receipt is read, but its lines have moved: the observations that
        // its bytes give are not those it records.
        let moved = |offset: u64| offsets[offset as usize];
        let changed = replaced(&receipt, normalised.clone(), moved);
        let changed = Receipt::parse(changed.render().as_bytes()).unwrap();
        assert_eq!(
            changed.entries[0].cold_boots[0].serial.bytes_base64.0,
            normalised
        );
        let moved = observed(&changed);
        assert_ne!(moved, *recorded);
        let ready = moved.markers.iter().find(|m| m.name == "ready");
        assert_eq!(ready.map(|m| m.line), Some(offset(&normalised, b"READY")));
    }
}
