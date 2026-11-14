//! Public versions of receipts (`docs/bench/formats.md`, public versions):
//! [`public_version`] replaces every value to hide by `#` of equal length,
//! lists what it replaced, and checks the result with [`verify`].

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ops::Range;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::board::{Board, EMBEDDED};
use crate::digest::Sha256;
use crate::document::check;
use crate::instance::{InstanceValue, MIN_SEARCHED_LEN};
use crate::observe::sensitive_values;
use crate::receipt::{self, Receipt, ReceiptError, decoded, invalid, is_digest, key, walk};

/// The `format` of a public version.
pub const FORMAT: &str = "kyto-bench public receipt";

/// The `format_version` of a public version that this tool reads and writes.
pub const FORMAT_VERSION: i64 = 1;

/// The byte that replaces every byte of a value.
pub const PLACEHOLDER: u8 = b'#';

pub const RELAY_RULE: &str = "instance.local.power.relay";

pub const NONCE_RULE: &str = "nonce";

/// The rule of the unterminated last line of a cold boot's console output.
pub const UNTERMINATED_RULE: &str = "console.unterminated_line";

/// The rule of a segment whose digest covers replaced bytes.
pub const SEGMENT_RULE: &str = "segment.covers_hidden";

/// The rule of a receipt with more values to hide than a public version searches for.
pub const LIMIT_RULE: &str = "public.values_limit";

/// The rule of a public version that [`PublicReceipt::check`] refuses, or
/// of a receipt whose board description cannot be read.
pub const FORM_RULE: &str = "public.form";

/// The most values to hide that a public version searches for.
pub const MAX_VALUES: usize = 1024;

/// The most bytes of the values to hide that a public version searches for.
pub const MAX_VALUE_BYTES: usize = 64 << 10;

/// The fields that a public version always withholds, by their JSON pointers
/// in the receipt, with their rules, the nonce first.
pub const WITHHELD: [(&str, &str); 3] = [
    ("/nonce", NONCE_RULE),
    (
        "/inputs/instance/parameters/tftp/server_address",
        "instance.tftp.server_address",
    ),
    (
        "/inputs/instance/parameters/tftp/path_prefix",
        "instance.tftp.path_prefix",
    ),
];

/// The recorded texts, which a public version never rewrites.
const TEXTS: [&str; 2] = ["/inputs/run_plan/text", BOARD_TEXT];
const BOARD_TEXT: &str = "/inputs/board_description/text";

type Spans = Vec<(Range<usize>, BTreeSet<String>)>;

/// A value that a public version must not show, with the rule that hides it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Secret {
    pub rule: String,
    pub value: Vec<u8>,
}

impl Secret {
    fn new(rule: &str, value: &[u8]) -> Self {
        let (rule, value) = (rule.into(), value.into());
        Self { rule, value }
    }
}

/// A field that a public version always withholds, by its JSON pointer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Withheld {
    pub field: String,
    pub length: usize,
    pub rule: String,
}

/// Bytes replaced in a field, by its JSON pointer in the receipt: at
/// `offset` of the decoded bytes of a stream or of the UTF-8 bytes of a
/// string, with every rule that found them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Replacement {
    pub field: String,
    pub offset: usize,
    pub length: usize,
    pub rules: Vec<String>,
}

/// The public version of a receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicReceipt {
    pub format: String,
    pub format_version: i64,
    /// The SHA-256 of the receipt's file.
    pub receipt_sha256: Sha256,
    pub placeholder: String,
    pub withheld: Vec<Withheld>,
    pub replacements: Vec<Replacement>,
    pub receipt: Value,
}

/// A value found in a public version: the rule that hides it and the JSON
/// pointer of the field, in the receipt, that holds it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Leak {
    pub rule: String,
    pub field: String,
}

impl Leak {
    fn new(rule: &str, field: &str) -> Self {
        let (rule, field) = (rule.into(), field.into());
        Self { rule, field }
    }
}

impl PublicReceipt {
    /// The public version as the tool writes it, in the form of receipts.
    pub fn render(&self) -> String {
        receipt::render_json(self)
    }

    /// Reads a public version strictly, as a receipt is read, and checks
    /// what [`PublicReceipt::check`] checks.
    pub fn parse(text: &[u8]) -> Result<Self, ReceiptError> {
        let public: Self = receipt::read_rendered(text, FORMAT, FORMAT_VERSION)?;
        public.check()?;
        Ok(public)
    }

    /// Checks the rules that tie the parts of a public version together, as
    /// `docs/bench/formats.md` describes the reading of a public version; its
    /// `receipt` is read with `0` for each replaced byte of a string.
    pub fn check(&self) -> Result<(), ReceiptError> {
        let (mut body, held) = (self.receipt.clone(), withheld(&self.receipt));
        check! {
            "/placeholder" => self.placeholder == "#", "the placeholder is #";
            "/withheld" => self.withheld == held && held.first().is_some_and(|w| w.field == "/nonce")
                && held.iter().all(|w| body.pointer(&w.field).and_then(Value::as_str).is_some_and(|text| is_placeholders(text.as_bytes()))),
                "the fields always withheld that are not empty, as # only";
        }
        let (mut records, mut wrong) = (self.replacements.iter().enumerate().peekable(), None);
        walk(&mut body, "", &mut |at, text| {
            if records.peek().is_none_or(|(_, r)| r.field != at) {
                return;
            }
            let bytes = decoded(at, text).unwrap_or_else(|| text.as_bytes().into());
            let mut after = 0;
            while let Some((index, r)) = records.next_if(|(_, r)| r.field == at) {
                let end = r.offset.checked_add(r.length);
                let span = end.and_then(|end| bytes.get(r.offset..end));
                let hidden = span.is_some_and(|span| !span.is_empty() && is_placeholders(span));
                let sorted = !r.rules.is_empty() && r.rules.is_sorted_by(|a, b| a < b);
                if !rewritten(at) || r.offset < after || !hidden || !sorted {
                    wrong = wrong.or(Some(index));
                } else if key(at) != "bytes_base64" {
                    text.replace_range(r.offset..r.offset + r.length, &"0".repeat(r.length));
                }
                after = r.offset.saturating_add(r.length);
            }
        });
        if let Some(index) = wrong.or(records.next().map(|(index, _)| index)) {
            let reason = "a span of # in a field rewritten, in order, with sorted rules";
            return Err(invalid(format!("/replacements/{index}"), reason));
        }
        let parsed = Receipt::deserialize(&body).map_err(|e| invalid("/receipt", e.to_string()))?;
        check! {
            "/receipt" => receipt::render_json(&parsed) == receipt::render_json(&body),
                "a receipt in the form of the format, its keys in order";
            "/receipt" => parsed.format == receipt::FORMAT && parsed.format_version == receipt::FORMAT_VERSION,
                "a receipt of this version";
            "/receipt/inputs/board_description/text" => Board::parse(&parsed.inputs.board_description.text).is_ok(),
                "a board description that this tool reads";
        }
        Ok(())
    }
}

/// The values of at least 4 bytes that the public version of `receipt`
/// must not show, as `docs/bench/formats.md` lists them, with `instance`,
/// sorted by rule and value, then the nonce. Beyond [`MAX_VALUES`] or
/// [`MAX_VALUE_BYTES`], the error is a leak of [`LIMIT_RULE`] at the empty pointer.
pub fn secrets(receipt: &Receipt, instance: &[InstanceValue]) -> Result<Vec<Secret>, Vec<Leak>> {
    let boards = boards_of(&receipt.inputs.board_description.text)?;
    let mut found = BTreeSet::new();
    let mut add = |rule: &str, value: &[u8]| {
        if value.len() >= MIN_SEARCHED_LEN && found.len() <= MAX_VALUES {
            found.insert(Secret::new(rule, value));
        }
    };
    let mut value = serde_json::to_value(receipt).expect("a receipt is JSON");
    walk(&mut value, "", &mut |at, text| {
        let tftp = WITHHELD[1..].iter().filter(|(field, _)| *field == at);
        let relay = (key(at) == "relay").then_some(RELAY_RULE);
        for rule in tftp.map(|(_, rule)| *rule).chain(relay) {
            add(rule, text.as_bytes());
        }
        for (_, rule, value) in sensitive_values(&boards, &decoded(at, text).unwrap_or_default()) {
            add(&rule, &value);
        }
    });
    instance
        .iter()
        .for_each(|value| add(&value.rule, value.value.as_bytes()));
    let bytes: usize = found.iter().map(|secret| secret.value.len()).sum();
    if found.len() > MAX_VALUES || bytes > MAX_VALUE_BYTES {
        return Err(vec![Leak::new(LIMIT_RULE, "")]);
    }
    let nonce = Secret::new(NONCE_RULE, receipt.nonce.as_bytes());
    Ok(found.into_iter().chain([nonce]).collect())
}

/// Makes the public version of `receipt`, whose instance values are
/// `instance`, as `docs/bench/formats.md` describes, and checks it with
/// [`verify`]; the leaks are the error.
pub fn public_version(
    receipt: &Receipt,
    instance: &[InstanceValue],
) -> Result<PublicReceipt, Vec<Leak>> {
    let boards = boards_of(&receipt.inputs.board_description.text)?;
    let secrets = secrets(receipt, instance)?;
    let finder = Finder::new(secrets.iter().filter(|secret| secret.rule != NONCE_RULE));
    let mut value = serde_json::to_value(receipt).expect("a receipt is JSON");
    let withheld = withheld(&value);
    for w in &withheld {
        *value.pointer_mut(&w.field).expect("a withheld field") = placeholders(w.length).into();
    }
    let mut replacements = Vec::new();
    walk(&mut value, "", &mut |at, text| {
        if !rewritten(at) {
            return;
        }
        let stream = decoded(at, text);
        let mut bytes = stream.clone().unwrap_or_else(|| text.as_bytes().to_vec());
        let mut spans = finder.find(&bytes);
        spans.extend((key(at) == "relay").then(|| (0..bytes.len(), rule(RELAY_RULE))));
        let hits = sensitive_values(&boards, stream.as_deref().unwrap_or_default());
        spans.extend(hits.into_iter().map(|(span, name, _)| (span, rule(&name))));
        let open = unterminated(at, &bytes);
        spans.extend(open.map(|span| (span, rule(UNTERMINATED_RULE))));
        if stream.is_none() {
            // Whole characters are replaced, so the string stays UTF-8.
            for (span, _) in &mut spans {
                *span = text.floor_char_boundary(span.start)..text.ceil_char_boundary(span.end);
            }
        }
        for (span, rules) in merge(spans) {
            bytes[span.clone()].fill(PLACEHOLDER);
            replacements.push(Replacement {
                field: at.into(),
                offset: span.start,
                length: span.len(),
                rules: rules.into_iter().collect(),
            });
        }
        *text = match stream {
            Some(_) => STANDARD.encode(&bytes),
            None => String::from_utf8(bytes).expect("characters replaced whole"),
        };
    });
    let public = PublicReceipt {
        format: FORMAT.into(),
        format_version: FORMAT_VERSION,
        receipt_sha256: Sha256::of(receipt.render().as_bytes()),
        placeholder: char::from(PLACEHOLDER).into(),
        withheld,
        replacements,
        receipt: value,
    };
    verify(&public, &secrets)?;
    Ok(public)
}

/// The leak check of a public version, whose values to hide are `secrets`,
/// as `docs/bench/formats.md` describes; what [`PublicReceipt::check`]
/// refuses is a leak of [`FORM_RULE`] at its pointer in the public version.
/// Each leak names its rule and its field, never its value.
pub fn verify(public: &PublicReceipt, secrets: &[Secret]) -> Result<(), Vec<Leak>> {
    let mut leaks = BTreeSet::new();
    if let Err(ReceiptError::Invalid { at, .. }) = public.check() {
        leaks.insert(Leak::new(FORM_RULE, &at));
    }
    let text = public.receipt.pointer(BOARD_TEXT).and_then(Value::as_str);
    let boards = boards_of(text.unwrap_or_default()).unwrap_or_default();
    let mut ends = BTreeMap::new();
    for r in &public.replacements {
        ends.insert((&r.field[..], r.offset), r.offset.saturating_add(r.length));
    }
    let finder = Finder::new(secrets);
    walk(&mut public.receipt.clone(), "", &mut |at, text| {
        let stream = decoded(at, text);
        let bytes = stream.as_deref().unwrap_or(text.as_bytes());
        let found = finder.find(bytes).into_iter().flat_map(|(_, rules)| rules);
        let mut rules: BTreeSet<String> = found.collect();
        if stream.is_some() {
            let held = sensitive_values(&boards, bytes).into_iter();
            rules.extend(held.filter(|hit| !is_placeholders(&hit.2)).map(|hit| hit.1));
        }
        let open = unterminated(at, bytes).is_some_and(|span| !is_placeholders(&bytes[span]));
        rules.extend(open.then(|| UNTERMINATED_RULE.into()));
        let relay = key(at) == "relay" && !is_placeholders(text.as_bytes());
        rules.extend(relay.then(|| RELAY_RULE.into()));
        rules.extend(covers_hidden(public, &ends, at).then(|| SEGMENT_RULE.into()));
        leaks.extend(rules.iter().map(|rule| Leak::new(rule, at)));
    });
    let leaks: Vec<Leak> = leaks.into_iter().collect();
    leaks.is_empty().then_some(()).ok_or(leaks)
}

/// The board description recorded as `text` and the built-in one of its
/// id, when they differ, whose sensitive lines are both used; one recorded
/// that cannot be read is a leak of [`FORM_RULE`] at its field.
fn boards_of(text: &str) -> Result<Vec<Board>, Vec<Leak>> {
    let recorded = Board::parse(text).map_err(|_| vec![Leak::new(FORM_RULE, BOARD_TEXT)])?;
    let built_in = EMBEDDED.iter().filter(|built_in| **built_in != text);
    let built_in = built_in.filter_map(|text| Board::parse(text).ok());
    let same: Vec<Board> = built_in.filter(|board| board.id == recorded.id).collect();
    Ok([recorded].into_iter().chain(same).collect())
}

/// The fields of [`WITHHELD`] that `value` holds as strings that are not
/// empty, as `withheld` lists them.
fn withheld(value: &Value) -> Vec<Withheld> {
    let field = |&(field, rule): &(&str, &str)| {
        let length = value.pointer(field)?.as_str()?.len();
        let (field, rule) = (field.into(), rule.into());
        (length > 0).then_some(Withheld {
            field,
            length,
            rule,
        })
    };
    WITHHELD.iter().filter_map(field).collect()
}

/// Whether a public version rewrites the string at `at`: neither a recorded
/// text, a digest nor a field always withheld.
fn rewritten(at: &str) -> bool {
    !(TEXTS.contains(&at) || is_digest(at) || WITHHELD.iter().any(|(field, _)| *field == at))
}

/// The unterminated last line of the bytes of the field at `at` when it is
/// the console output of a cold boot.
fn unterminated(at: &str, bytes: &[u8]) -> Option<Range<usize>> {
    let lf = bytes.iter().rposition(|byte| *byte == b'\n');
    let start = lf.map_or(0, |lf| lf + 1);
    (at.ends_with("/serial/bytes_base64") && start < bytes.len()).then_some(start..bytes.len())
}

/// Whether the field at `at` is the digest of a segment of a cold boot whose
/// console output, from the first byte of the line of `from` up to the
/// first byte of the line of `to`, holds a replaced byte.
fn covers_hidden(public: &PublicReceipt, ends: &BTreeMap<(&str, usize), usize>, at: &str) -> bool {
    let segment = at.strip_suffix("/sha256").unwrap_or_default();
    let Some((cold_boot, _)) = segment.split_once("/observations/segments/") else {
        return false;
    };
    let [from, to] = ["from_line", "to_line"].map(|key| {
        let line = public.receipt.pointer(&format!("{segment}/{key}"));
        line.and_then(Value::as_u64).unwrap_or(0) as usize
    });
    let serial = format!("{cold_boot}/serial/bytes_base64");
    // Replacements of a field are apart and in order: the last that starts
    // before `to` ends last.
    let last = ends.range((&serial[..], 0)..(&serial[..], to)).next_back();
    last.is_some_and(|(_, end)| *end > from)
}

/// Every value to hide found in one pass over bytes, ignoring ASCII case, by
/// the Aho-Corasick algorithm: a trie of the values whose nodes have the node
/// of their longest proper suffix, and the length of the longest value that
/// ends there with the rules, each once, of every value that does.
struct Finder {
    next: Vec<BTreeMap<u8, usize>>,
    suffix: Vec<usize>,
    ends: Vec<(usize, BTreeSet<String>)>,
}

impl Finder {
    fn new<'s>(secrets: impl IntoIterator<Item = &'s Secret>) -> Self {
        let (next, suffix, ends) = (vec![BTreeMap::new()], vec![0], vec![(0, BTreeSet::new())]);
        let mut finder = Self { next, suffix, ends };
        for secret in secrets {
            let mut node = 0;
            for byte in secret.value.to_ascii_lowercase() {
                let new = finder.next.len();
                node = *finder.next[node].entry(byte).or_insert(new);
                if node == new {
                    finder.next.push(BTreeMap::new());
                    finder.suffix.push(0);
                    finder.ends.push((0, BTreeSet::new()));
                }
            }
            finder.ends[node].0 = secret.value.len();
            finder.ends[node].1.insert(secret.rule.clone());
        }
        // Breadth first, so that the shorter nodes of suffixes come first.
        let mut queue: VecDeque<usize> = finder.next[0].values().copied().collect();
        while let Some(node) = queue.pop_front() {
            for (byte, child) in finder.next[node].clone() {
                let suffix = finder.step(finder.suffix[node], byte);
                let (length, rules) = finder.ends[suffix].clone();
                finder.suffix[child] = suffix;
                finder.ends[child].0 = finder.ends[child].0.max(length);
                finder.ends[child].1.extend(rules);
                queue.push_back(child);
            }
        }
        finder
    }

    /// The node after `node` on `byte`, in lower case.
    fn step(&self, mut node: usize, byte: u8) -> usize {
        let byte = byte.to_ascii_lowercase();
        loop {
            match self.next[node].get(&byte) {
                Some(next) => return *next,
                None if node == 0 => return 0,
                None => node = self.suffix[node],
            }
        }
    }

    /// The spans of `bytes` that values cover, merged where they overlap,
    /// each with the rules of the values found in it.
    fn find(&self, bytes: &[u8]) -> Spans {
        let (mut node, mut found) = (0, Spans::new());
        for (at, byte) in bytes.iter().enumerate() {
            node = self.step(node, *byte);
            let (length, rules) = &self.ends[node];
            if *length > 0 {
                let (mut span, mut rules) = (at + 1 - length..at + 1, rules.clone());
                // Spans end in order, so a new one can overlap only the last ones.
                while let Some((last, more)) = found.pop_if(|(last, _)| span.start < last.end) {
                    span.start = span.start.min(last.start);
                    rules.extend(more);
                }
                found.push((span, rules));
            }
        }
        found
    }
}

/// Merges spans that overlap or are equal into one, with every rule once;
/// empty spans are left out.
fn merge(mut spans: Spans) -> Spans {
    spans.sort_by_key(|(span, _)| (span.start, span.end));
    let mut merged = Spans::new();
    for (span, rules) in spans.into_iter().filter(|(span, _)| !span.is_empty()) {
        match merged.last_mut() {
            Some((last, all)) if span.start < last.end => {
                last.end = last.end.max(span.end);
                all.extend(rules);
            }
            _ => merged.push((span, rules)),
        }
    }
    merged
}

fn rule(rule: &str) -> BTreeSet<String> {
    BTreeSet::from([rule.to_string()])
}

fn placeholders(length: usize) -> String {
    char::from(PLACEHOLDER).to_string().repeat(length)
}

fn is_placeholders(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| *byte == PLACEHOLDER)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::RASPBERRY_PI_5;
    use crate::instance::Instance;
    use crate::receipt::tests::{example, instance_text};
    use crate::receipt::{Command, EventKind, Serial};

    /// Console output with the sensitive lines of a network boot, in
    /// documentation values: the board serial 1234abcd, the board's MAC
    /// address 00:00:5e:00:53:01 and the TFTP server 192.0.2.10.
    const NETWORK: &[u8] = b"  4.37 BOOTMODE: 0x06 partition 0 build-ts BUILD_TIMESTAMP=1700000000 serial 1234abcd boardrev d04171 stc 4371234\r\n  5.51 NETWORK: 00:00:5e:00:53:01 wait for link TFTP: 192.0.2.10\r\nREADY\r\n";

    fn values() -> Vec<InstanceValue> {
        Instance::parse(&instance_text())
            .unwrap()
            .deidentify_values()
    }

    /// The example receipt whose cold boot received `bytes` in one chunk,
    /// each of its events after bytes now after the whole chunk.
    fn with_serial(bytes: &[u8]) -> Receipt {
        let mut receipt = example();
        let cold_boot = &mut receipt.entries[0].cold_boots[0];
        let length = bytes.len() as u64;
        cold_boot.serial = Serial {
            length,
            bytes_base64: crate::receipt::Base64(bytes.to_vec()),
            index: vec![[0, 6_100_000_000]],
        };
        for event in cold_boot
            .events
            .iter_mut()
            .filter(|event| event.received > 0)
        {
            event.received = length;
        }
        receipt
    }

    fn public(receipt: &Receipt) -> Result<PublicReceipt, Vec<Leak>> {
        public_version(receipt, &values())
    }

    /// The decoded console output of the public version's cold boot.
    fn serial_of(public: &PublicReceipt) -> Vec<u8> {
        let at = "/entries/0/cold_boots/0/serial/bytes_base64";
        let text = public.receipt.pointer(at).and_then(Value::as_str).unwrap();
        STANDARD.decode(text).unwrap()
    }

    fn replacement(field: &str, offset: usize, length: usize, rules: &[&str]) -> Replacement {
        let rules = rules.iter().map(|rule| rule.to_string()).collect();
        let field = field.into();
        Replacement {
            field,
            offset,
            length,
            rules,
        }
    }

    fn leak(rule: &str, field: &str) -> Leak {
        let (rule, field) = (rule.into(), field.into());
        Leak { rule, field }
    }

    #[test]
    fn public_version_replaces_with_equal_length_placeholders_and_keeps_the_index() {
        let receipt = with_serial(NETWORK);
        let public = public(&receipt).unwrap();
        let at = "/entries/0/cold_boots/0/serial/bytes_base64";
        let serial = NETWORK.windows(8).position(|w| w == b"1234abcd").unwrap();
        let mac = NETWORK
            .windows(17)
            .position(|w| w == b"00:00:5e:00:53:01")
            .unwrap();
        let address = NETWORK
            .windows(10)
            .position(|w| w == b"192.0.2.10")
            .unwrap();
        assert_eq!(
            public.replacements,
            [
                replacement(at, serial, 8, &["board.board-serial"]),
                replacement(at, mac, 17, &["board.network-link"]),
                replacement(
                    at,
                    address,
                    10,
                    &["board.network-link", "instance.tftp.server_address"]
                ),
            ]
        );
        let mut expected = NETWORK.to_vec();
        for (offset, length) in [(serial, 8), (mac, 17), (address, 10)] {
            expected[offset..offset + length].fill(b'#');
        }
        assert_eq!(serial_of(&public), expected);
        let same = [
            "/entries/0/cold_boots/0/serial/index",
            "/entries/0/cold_boots/0/serial/length",
            "/inputs/run_plan",
            "/inputs/board_description",
        ];
        let original = serde_json::to_value(&receipt).unwrap();
        for at in same {
            assert_eq!(public.receipt.pointer(at), original.pointer(at), "{at}");
        }
        assert_eq!(
            public.receipt_sha256,
            Sha256::of(receipt.render().as_bytes())
        );
        // The formats page shows this public version, its digest shortened.
        let page = include_str!("../../../docs/bench/formats.md");
        let digest = public.receipt_sha256.to_string();
        let shown = format!("\n  \"receipt_sha256\": \"{}...\",\n", &digest[..8]);
        assert!(page.contains(&shown), "{digest}");
        let text = public.render();
        assert!(text.starts_with("{\n  \"format\": \"kyto-bench public receipt\",\n  \"format_version\": 1,\n  \"receipt_sha256\": "));
        assert!(text.contains("\n  \"placeholder\": \"#\",\n"), "{text}");
        for value in ["1234abcd", "00:00:5e:00:53:01", "192.0.2.10"] {
            assert!(!text.contains(value), "{value}");
        }
        // The withheld fields and the replacements are all that differ.
        let withheld: Vec<(&str, usize, &str)> = public
            .withheld
            .iter()
            .map(|w| (&w.field[..], w.length, &w.rule[..]))
            .collect();
        assert_eq!(
            withheld,
            [
                ("/nonce", 32, "nonce"),
                (
                    "/inputs/instance/parameters/tftp/server_address",
                    10,
                    "instance.tftp.server_address"
                ),
            ]
        );
    }

    #[test]
    fn the_nonce_is_always_withheld() {
        for receipt in [example(), with_serial(NETWORK), with_serial(b"")] {
            let public = public(&receipt).unwrap();
            let nonce = Withheld {
                field: "/nonce".into(),
                length: 32,
                rule: NONCE_RULE.into(),
            };
            assert_eq!(public.withheld[0], nonce);
            assert_eq!(public.receipt["nonce"], "#".repeat(32));
            assert!(!public.render().contains(&receipt.nonce));
        }
        // A nonce left in place is a leak.
        let receipt = example();
        let mut public = public(&receipt).unwrap();
        public.receipt["nonce"] = receipt.nonce.clone().into();
        let secrets = secrets(&receipt, &values()).unwrap();
        let found = vec![leak("nonce", "/nonce"), leak(FORM_RULE, "/withheld")];
        assert_eq!(verify(&public, &secrets), Err(found));
    }

    #[test]
    fn the_relay_number_is_replaced_in_power_events() {
        let mut receipt = example();
        let events = receipt.start.events.iter_mut();
        let cold_boot = receipt.entries[0].cold_boots[0].events.iter_mut();
        for event in events.chain(cold_boot) {
            if let EventKind::Power { relay, .. } = &mut event.event {
                *relay = Some("12".into());
            }
        }
        let public = public(&receipt).unwrap();
        let relays = [
            "/start/events/0/relay",
            "/entries/0/cold_boots/0/events/1/relay",
            "/entries/0/cold_boots/0/events/5/relay",
        ];
        let records: Vec<Replacement> = relays
            .iter()
            .map(|at| replacement(at, 0, 2, &[RELAY_RULE]))
            .collect();
        assert_eq!(public.replacements, records);
        for at in relays {
            assert_eq!(public.receipt.pointer(at), Some(&Value::from("##")));
        }
        // A relay that keeps its value is a leak, though a relay number is
        // too short to be searched for.
        let mut leaked = public.clone();
        *leaked.receipt.pointer_mut(relays[1]).unwrap() = "12".into();
        leaked.replacements.remove(1);
        let secrets = secrets(&receipt, &values()).unwrap();
        assert_eq!(
            verify(&leaked, &secrets),
            Err(vec![leak(RELAY_RULE, relays[1])])
        );
        let simulated = EventKind::Power {
            command: Command::Off,
            relay: None,
        };
        assert_eq!(example().start.events[0].event, simulated);
    }

    #[test]
    fn public_version_is_refused_when_a_hit_value_remains() {
        // The board serial is part of a digest, which is never rewritten.
        let mut receipt = with_serial(NETWORK);
        let digest = Sha256::parse(&format!("1234abcd{}", "0".repeat(56))).unwrap();
        receipt.entries[0].payload.files[0].source_sha256 = Some(digest);
        let field = "/entries/0/payload/files/0/source_sha256";
        assert_eq!(
            public(&receipt),
            Err(vec![leak("board.board-serial", field)])
        );
    }

    #[test]
    fn public_version_is_refused_when_an_instance_value_remains() {
        // The run plan is recorded verbatim, so a comment that names the
        // TFTP server stays.
        let mut receipt = example();
        let plan = format!(
            "# The plan of the bench whose server is 192.0.2.10.\n{}",
            receipt.inputs.run_plan.text
        );
        receipt.inputs.run_plan.sha256 = Sha256::of(plan.as_bytes());
        receipt.inputs.run_plan.text = plan;
        Receipt::parse(receipt.render().as_bytes()).unwrap();
        let rule = "instance.tftp.server_address";
        assert_eq!(
            public(&receipt),
            Err(vec![leak(rule, "/inputs/run_plan/text")])
        );
    }

    #[test]
    fn the_leak_check_reads_the_sensitive_lines_again() {
        // A board serial back in its line shows even to a check given no
        // value to search for.
        let receipt = with_serial(NETWORK);
        let mut public = public(&receipt).unwrap();
        let at = "/entries/0/cold_boots/0/serial/bytes_base64";
        let mut serial = serial_of(&public);
        let start = serial.windows(8).position(|w| w == b"########").unwrap();
        serial[start..start + 8].copy_from_slice(b"1234abcd");
        *public.receipt.pointer_mut(at).unwrap() = STANDARD.encode(&serial).into();
        // The record of its replacement goes too, so that the public version
        // keeps its form.
        assert_eq!(public.replacements[0].offset, start);
        public.replacements.remove(0);
        let found = Err(vec![leak("board.board-serial", at)]);
        assert_eq!(verify(&public, &[]), found);
    }

    #[test]
    fn hit_values_are_replaced_in_every_string_and_sent_stream() {
        let mut receipt = with_serial(NETWORK);
        let cold_boot = &mut receipt.entries[0].cold_boots[0];
        cold_boot.observations.values = vec![crate::observe::Agreement {
            field: "build".into(),
            values: vec!["id-1234abcd-é".into()],
            lines: vec![0],
        }];
        if let EventKind::Send { bytes_base64, .. } = &mut cold_boot.events[3].event {
            bytes_base64.0 = b"mac 00:00:5e:00:53:01\r".to_vec();
        }
        let public = public(&receipt).unwrap();
        let value = "/entries/0/cold_boots/0/observations/values/0/values/0";
        let send = "/entries/0/cold_boots/0/events/3/bytes_base64";
        let found: Vec<&Replacement> = public
            .replacements
            .iter()
            .filter(|r| r.field == value || r.field == send)
            .collect();
        assert_eq!(
            found,
            [
                &replacement(send, 4, 17, &["board.network-link"]),
                &replacement(value, 3, 8, &["board.board-serial"]),
            ]
        );
        assert_eq!(
            public.receipt.pointer(value),
            Some(&Value::from("id-########-é"))
        );
        assert_eq!(
            public.receipt.pointer(send),
            Some(&Value::from(STANDARD.encode("mac #################\r")))
        );
    }

    #[test]
    fn the_path_prefix_is_replaced_in_streams() {
        let line = b" 16.75 TFTP_GET: 00:00:5e:00:53:02 192.0.2.10 boot-a/config.txt\r\n";
        let mut receipt = with_serial(line);
        receipt.inputs.instance.parameters.tftp.path_prefix = "boot-a/".into();
        let public = public(&receipt).unwrap();
        let at = "/entries/0/cold_boots/0/serial/bytes_base64";
        let prefix = line.windows(7).position(|w| w == b"boot-a/").unwrap();
        let withheld = &public.withheld[2];
        assert_eq!(
            (&withheld.field[..], withheld.length),
            ("/inputs/instance/parameters/tftp/path_prefix", 7)
        );
        let record = replacement(at, prefix, 7, &["instance.tftp.path_prefix"]);
        assert!(
            public.replacements.contains(&record),
            "{:?}",
            public.replacements
        );
        assert_eq!(
            public.receipt["inputs"]["instance"]["parameters"]["tftp"]["path_prefix"],
            "#######"
        );
        assert!(!serial_of(&public).windows(7).any(|w| w == b"boot-a/"));
    }

    #[test]
    fn never_values_are_not_hits() {
        let line = b"  7.97 NET 192.0.2.20 255.255.255.0 gw 0.0.0.0 tftp 192.0.2.10\r\n";
        let receipt = with_serial(line);
        let rules = |rules: &[&str]| {
            rules
                .iter()
                .map(|rule| rule.to_string())
                .collect::<Vec<_>>()
        };
        let public = public(&receipt).unwrap();
        let found: Vec<(usize, Vec<String>)> = public
            .replacements
            .iter()
            .map(|r| (r.offset, r.rules.clone()))
            .collect();
        assert_eq!(
            found,
            [
                (11, rules(&["board.network-addresses"])),
                (
                    52,
                    rules(&["board.network-addresses", "instance.tftp.server_address"])
                ),
            ]
        );
        assert!(serial_of(&public).windows(7).any(|w| w == b"0.0.0.0"));
        let secrets = secrets(&receipt, &values()).unwrap();
        assert!(!secrets.iter().any(|secret| secret.value == b"0.0.0.0"));
    }

    fn instance_value(rule: &str, value: &str) -> InstanceValue {
        let (rule, value) = (rule.into(), value.into());
        InstanceValue { rule, value }
    }

    #[test]
    fn values_are_found_in_one_pass_overlapping_and_nested() {
        let receipt = with_serial(b"xabcdefghx zzzzzzzz abc\r\n");
        let instance = [
            instance_value("instance.local.console.a", "abcd"),
            instance_value("instance.local.console.b", "bcdefgh"),
            instance_value("instance.local.console.c", "cdef"),
            instance_value("instance.local.console.d", "zzzz"),
        ];
        let public = public_version(&receipt, &instance).unwrap();
        let at = "/entries/0/cold_boots/0/serial/bytes_base64";
        let rules = ["a", "b", "c"].map(|key| format!("instance.local.console.{key}"));
        let rules: Vec<&str> = rules.iter().map(String::as_str).collect();
        let found: Vec<&Replacement> = public
            .replacements
            .iter()
            .filter(|r| r.field == at)
            .collect();
        assert_eq!(
            found,
            [
                &replacement(at, 1, 8, &rules),
                &replacement(at, 11, 8, &["instance.local.console.d"]),
            ]
        );
        assert_eq!(serial_of(&public), b"x########x ######## abc\r\n");
        // The commit holds abcdef, which two of the values cover.
        let commit = replacement("/tool/commit", 10, 6, &[rules[0], rules[2]]);
        assert!(public.replacements.contains(&commit));
    }

    #[test]
    fn public_version_is_refused_with_more_values_to_hide_than_it_searches_for() {
        let line =
            |n: usize| format!("  4.37 BOOTMODE: 0x06 serial {n:08x} boardrev d04171 stc 1\r\n");
        let lines = |count: usize| (0..count).map(line).collect::<String>();
        // The values of the instance and the receipt are 2 of the 1024.
        let receipt = with_serial(lines(MAX_VALUES - 2).as_bytes());
        let serial = serial_of(&public(&receipt).unwrap());
        assert!(!serial.windows(8).any(|w| w == b"000003fd"));
        let receipt = with_serial(lines(MAX_VALUES - 1).as_bytes());
        assert_eq!(public(&receipt), Err(vec![leak(LIMIT_RULE, "")]));
        // One long value counts its bytes.
        let long = format!(
            "  4.37 BOOTMODE: 0x06 serial {} boardrev d04171 stc 1\r\n",
            "a".repeat(MAX_VALUE_BYTES)
        );
        assert_eq!(
            public(&with_serial(long.as_bytes())),
            Err(vec![leak(LIMIT_RULE, "")])
        );
    }

    #[test]
    fn a_capture_cut_short_hides_its_last_line() {
        let at = "/entries/0/cold_boots/0/serial/bytes_base64";
        let cut: [(&[u8], &[&str]); 3] = [
            // The form of the serial number's line matches before the cut.
            (
                b"READY\r\n  4.37 BOOTMODE: 0x06 partition 0 serial 1234abcd boardr",
                &["board.board-serial"],
            ),
            (b"READY\r\n  4.37 BOOTMODE: 0x06 partition 0 seri", &[]),
            (
                b"READY\r\n  5.51 NETWORK: 00:00:5e:00:53:01 wait for link ",
                &[],
            ),
        ];
        for (bytes, forms) in cut {
            let public = public(&with_serial(bytes)).unwrap();
            let rest = bytes.len() - 7;
            let rules = [forms, &[UNTERMINATED_RULE]].concat();
            assert_eq!(public.replacements, [replacement(at, 7, rest, &rules)]);
            assert_eq!(
                serial_of(&public),
                [&b"READY\r\n"[..], &vec![b'#'; rest]].concat()
            );
            // The line put back is a leak, whether a form matches it or not.
            let mut leaked = public.clone();
            *leaked.receipt.pointer_mut(at).unwrap() = STANDARD.encode(bytes).into();
            leaked.replacements.clear();
            let secrets = secrets(&with_serial(bytes), &values()).unwrap();
            let found = rules.iter().map(|rule| leak(rule, at)).collect();
            assert_eq!(verify(&leaked, &secrets), Err(found));
        }
    }

    #[test]
    fn a_segment_over_a_hidden_value_has_no_public_version() {
        let bytes = [NETWORK, b"banner\r\nEND\r\n"].concat();
        let ready = NETWORK.len() - "READY\r\n".len();
        let end = bytes.len() - "END\r\n".len();
        let segment = |from_line: usize, to_line: usize| crate::observe::SegmentSeen {
            name: "banner".into(),
            from_line: from_line as u64,
            to_line: to_line as u64,
            sha256: Sha256::of(&bytes[from_line..to_line]),
        };
        let mut receipt = with_serial(&bytes);
        let segments = &mut receipt.entries[0].cold_boots[0].observations.segments;
        *segments = vec![segment(ready, end)];
        let mut found = public(&receipt).unwrap();
        // A replacement that would end past every offset is refused, not
        // added up.
        (found.replacements[0].offset, found.replacements[0].length) = (1, usize::MAX);
        let leaks = verify(&found, &[]).unwrap_err();
        assert!(
            leaks.contains(&leak(FORM_RULE, "/replacements/0")),
            "{leaks:?}"
        );
        // From the serial number's line to READY: its digest would let the
        // serial number and the MAC address be guessed.
        receipt.entries[0].cold_boots[0].observations.segments = vec![segment(0, ready)];
        let field = "/entries/0/cold_boots/0/observations/segments/0/sha256";
        assert_eq!(public(&receipt), Err(vec![leak(SEGMENT_RULE, field)]));
    }

    #[test]
    fn a_public_version_is_read_as_strictly_as_a_receipt() {
        let public = public(&with_serial(NETWORK)).unwrap();
        let text = public.render();
        assert_eq!(PublicReceipt::parse(text.as_bytes()), Ok(public.clone()));
        let spaced = text.replacen("\"placeholder\": ", "\"placeholder\":  ", 1);
        assert_eq!(
            PublicReceipt::parse(spaced.as_bytes()),
            Err(ReceiptError::NotRendered)
        );
        let extra = text.replacen("\"placeholder\": ", "\"extra\": 1,\n  \"placeholder\": ", 1);
        assert!(matches!(
            PublicReceipt::parse(extra.as_bytes()),
            Err(ReceiptError::Syntax(_))
        ));
        let receipt = example().render();
        assert!(matches!(
            PublicReceipt::parse(receipt.as_bytes()),
            Err(ReceiptError::Format(_))
        ));
        // The parts must agree, and the leak check refuses what reading
        // refuses.
        let refused = |edit: &dyn Fn(&mut PublicReceipt), at: &str| {
            let mut edited = public.clone();
            edit(&mut edited);
            match PublicReceipt::parse(edited.render().as_bytes()) {
                Err(ReceiptError::Invalid { at: found, .. }) => assert_eq!(found, at),
                other => panic!("{at}: {other:?}"),
            }
            let leaks = verify(&edited, &[]).unwrap_err();
            assert!(leaks.contains(&leak(FORM_RULE, at)), "{at}: {leaks:?}");
        };
        let serial = "/entries/0/cold_boots/0/serial/bytes_base64";
        refused(&|p| p.placeholder = "x".into(), "/placeholder");
        refused(&|p| p.receipt = Value::Null, "/withheld");
        refused(&|p| drop(p.withheld.remove(0)), "/withheld");
        refused(&|p| p.receipt["nonce"] = "0".repeat(32).into(), "/withheld");
        refused(&|p| p.withheld[1].length = 9, "/withheld");
        refused(&|p| p.receipt["extra"] = 1.into(), "/receipt");
        refused(&|p| p.receipt["format"] = FORMAT.into(), "/receipt");
        // The receipt keeps the rendered form of a receipt: its keys in order,
        // none missing and no detail of a passing check.
        refused(
            &|p| {
                let keys = p.receipt.as_object().unwrap().clone();
                p.receipt = Value::Object(keys.into_iter().rev().collect());
            },
            "/receipt",
        );
        refused(
            &|p| drop(p.receipt.as_object_mut().unwrap().shift_remove("refusal")),
            "/receipt",
        );
        refused(
            &|p| p.receipt["entries"][0]["cold_boots"][0]["checks"][0]["detail"] = Value::Null,
            "/receipt",
        );
        refused(
            &|p| p.receipt["inputs"]["board_description"]["text"] = "board".into(),
            "/receipt/inputs/board_description/text",
        );
        let field = "/entries/0/cold_boots/0/serial/nothing";
        refused(
            &|p| p.replacements[0].field = field.into(),
            "/replacements/0",
        );
        refused(&|p| p.replacements[0].offset -= 1, "/replacements/0");
        refused(&|p| p.replacements[0].length = 0, "/replacements/0");
        refused(
            &|p| (p.replacements[0].offset, p.replacements[0].length) = (1, usize::MAX),
            "/replacements/0",
        );
        refused(&|p| p.replacements.swap(0, 1), "/replacements/1");
        refused(&|p| p.replacements[2].rules.reverse(), "/replacements/2");
        refused(&|p| p.replacements[2].rules.clear(), "/replacements/2");
        // Only strings that a public version rewrites hold replacements.
        let nonce = replacement("/nonce", 0, 4, &[NONCE_RULE]);
        refused(
            &|p| p.replacements.insert(0, nonce.clone()),
            "/replacements/0",
        );
        assert_eq!(public.replacements[0].field, serial);
        // A replaced string is read with 0 for each replaced byte, so a
        // payload name keeps its form.
        let mut receipt = with_serial(NETWORK);
        let name = crate::names::PayloadName::parse("1234abcd.img").unwrap();
        let file = crate::observe::BootFile { name, sha256: None };
        receipt.entries[0].cold_boots[0]
            .observations
            .boot_files
            .push(file);
        let public = super::public_version(&receipt, &values()).unwrap();
        let at = "/entries/0/cold_boots/0/observations/boot_files/0/name";
        assert_eq!(
            public.receipt.pointer(at),
            Some(&Value::from("########.img"))
        );
        let text = public.render();
        assert_eq!(PublicReceipt::parse(text.as_bytes()), Ok(public));
    }

    #[test]
    fn a_lost_lf_or_a_stray_byte_keeps_no_value_in_place() {
        let lines: [&[u8]; 3] = [
            b"  4.37 MFG_VER: 2\r  4.37 BOOTMODE: 0x06 partition 0 build-ts BUILD_TIMESTAMP=1 serial 1234abcd boardrev d04171 stc 1\r\n",
            b"  5.51 NETWORK: 00:00:5e:00:53:01 wait for link TFTP: 192.0.2.10 \r\n",
            b"\x00  5.51 NETWORK: 00:00:5e:00:53:01 wait for link TFTP: 192.0.2.10\r\r\n",
        ];
        let bytes = lines.concat();
        let public = public(&with_serial(&bytes)).unwrap();
        let serial = serial_of(&public);
        for value in [&b"1234abcd"[..], b"00:00:5e:00:53:01", b"192.0.2.10"] {
            assert!(
                !serial.windows(value.len()).any(|w| w == value),
                "{value:?}"
            );
        }
        let at = "/entries/0/cold_boots/0/serial/bytes_base64";
        let rules = |r: &Replacement| r.rules.iter().any(|rule| rule.starts_with("board."));
        let found: Vec<(usize, usize)> = public
            .replacements
            .iter()
            .filter(|r| rules(r))
            .map(|r| (r.offset, r.length))
            .collect();
        let mac = |from: usize| {
            from + bytes[from..]
                .windows(17)
                .position(|w| w == b"00:00:5e:00:53:01")
                .unwrap()
        };
        let second = lines[0].len();
        let third = second + lines[1].len();
        assert_eq!(found.len(), 5, "{found:?}");
        assert!(found.contains(&(mac(second), 17)) && found.contains(&(mac(third), 17)));
        // Put back, with no record of their replacement, they are leaks even
        // to a check given no value to search for.
        let mut leaked = public.clone();
        *leaked.receipt.pointer_mut(at).unwrap() = STANDARD.encode(&bytes).into();
        leaked.replacements.retain(|r| r.field != at);
        let found = vec![
            leak("board.board-serial", at),
            leak("board.network-link", at),
        ];
        assert_eq!(verify(&leaked, &[]), Err(found));
    }

    #[test]
    fn a_printable_stray_byte_or_lines_run_together_keep_no_value_in_place() {
        let bootmode = |serial: &str| {
            format!(
                "  4.37 BOOTMODE: 0x06 partition 0 build-ts BUILD_TIMESTAMP=1 serial {serial} boardrev d04171 stc 1"
            )
        };
        let network = |mac: &str| format!("  5.51 NETWORK: {mac} wait for link TFTP: 192.0.2.10");
        let (mac, other) = ("00:00:5e:00:53:01", "00:00:5e:00:53:02");
        let inputs = [
            format!("~{}\r\n", bootmode("1234abcd")),
            format!("{}x\r\n", network(mac)),
            format!("  4.30 PM_RSTS: 0x00001000U{}\r\n", bootmode("1234abcd")),
            format!("{}U{}\r\n", bootmode("1234abcd"), bootmode("5678ef01")),
            format!("{}U{}\r\n", network(mac), network(other)),
        ];
        let at = "/entries/0/cold_boots/0/serial/bytes_base64";
        for input in inputs {
            let public = public(&with_serial(input.as_bytes())).unwrap();
            let serial = serial_of(&public);
            for value in ["1234abcd", "5678ef01", mac, other] {
                let value = value.as_bytes();
                let found = serial.windows(value.len()).any(|w| w == value);
                assert!(!found, "{input:?}");
            }
            // Put back with no record of its replacements, the line is a leak
            // even to a check given no value to search for.
            let mut leaked = public.clone();
            *leaked.receipt.pointer_mut(at).unwrap() = STANDARD.encode(&input).into();
            leaked.replacements.retain(|r| r.field != at);
            let leaks = verify(&leaked, &[]).unwrap_err();
            let board = |leak: &Leak| leak.field == at && leak.rule.starts_with("board.");
            assert!(!leaks.is_empty() && leaks.iter().all(board), "{leaks:?}");
        }
    }

    #[test]
    fn segments_are_checked_against_the_replacements_of_their_cold_boot() {
        let mut bytes = b"start\r\n".to_vec();
        for _ in 0..500 {
            bytes.extend_from_slice(b"server 192.0.2.10\r\n");
        }
        let middle = bytes.len();
        bytes.extend_from_slice(b"middle\r\nclean\r\n");
        let end = bytes.len();
        bytes.extend_from_slice(b"end\r\n");
        let segment = |from_line: usize, to_line: usize| crate::observe::SegmentSeen {
            name: format!("s{from_line}"),
            from_line: from_line as u64,
            to_line: to_line as u64,
            sha256: Sha256::of(&bytes[from_line..to_line]),
        };
        let mut receipt = with_serial(&bytes);
        let segments = [segment(0, 7), segment(middle, end), segment(7, middle)];
        let seen = &mut receipt.entries[0].cold_boots[0].observations;
        seen.segments = segments[..2].to_vec();
        let public = public(&receipt).unwrap();
        assert_eq!(public.replacements.len(), 500);
        let seen = &mut receipt.entries[0].cold_boots[0].observations;
        seen.segments = segments.to_vec();
        let field = "/entries/0/cold_boots/0/observations/segments/2/sha256";
        assert_eq!(
            super::public_version(&receipt, &values()),
            Err(vec![leak(SEGMENT_RULE, field)])
        );
    }

    #[test]
    fn values_are_found_ignoring_ascii_case() {
        let kernel = b"[    1.04] eth0: MAC address 00:00:5E:00:53:01\r\n";
        let bytes = [NETWORK, &kernel[..]].concat();
        let public = public(&with_serial(&bytes)).unwrap();
        let at = "/entries/0/cold_boots/0/serial/bytes_base64";
        let upper = NETWORK.len() + kernel.len() - 19;
        let record = replacement(at, upper, 17, &["board.network-link"]);
        assert!(
            public.replacements.contains(&record),
            "{:?}",
            public.replacements
        );
        let serial = serial_of(&public);
        assert!(
            !serial
                .windows(17)
                .any(|w| w.eq_ignore_ascii_case(b"00:00:5e:00:53:01"))
        );
        // Put back, it is a leak.
        let mut leaked = public.clone();
        let mut restored = serial.clone();
        restored[upper..upper + 17].copy_from_slice(b"00:00:5E:00:53:01");
        *leaked.receipt.pointer_mut(at).unwrap() = STANDARD.encode(&restored).into();
        leaked.replacements.retain(|r| *r != record);
        let secrets = secrets(&with_serial(&bytes), &values()).unwrap();
        assert_eq!(
            verify(&leaked, &secrets),
            Err(vec![leak("board.network-link", at)])
        );
    }

    #[test]
    fn a_disk_signature_is_hidden_with_any_partition_number() {
        // The kernel command line of an MBR disk gives its signature, which
        // later lines print with the number of another partition, in a path
        // and in the escaped name of a systemd unit.
        let lines = b"console=serial0,115200 root=PARTUUID=5ca1ab1e-02 rootwait\r\n\
            Found /dev/disk/by-partuuid/5ca1ab1e-01\r\n\
            Found dev-disk-by\\x2dpartuuid-5ca1ab1e\\x2d01.device\r\n";
        let receipt = with_serial(lines);
        let found = secrets(&receipt, &values()).unwrap();
        assert!(found.contains(&Secret::new("board.root-partuuid", b"5ca1ab1e")));
        let serial = serial_of(&public(&receipt).unwrap());
        assert!(
            !serial
                .windows(8)
                .any(|w| w.eq_ignore_ascii_case(b"5ca1ab1e"))
        );
        // The partition numbers identify nothing and stay.
        for number in [&b"-02 "[..], b"-01\r", b"\\x2d01."] {
            assert!(serial.windows(number.len()).any(|w| w == number));
        }
        // The partition UUID of a GPT disk is hidden whole.
        let uuid = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";
        let line = format!("root=PARTUUID={uuid} rootwait\r\n");
        let public = public(&with_serial(line.as_bytes())).unwrap();
        let at = "/entries/0/cold_boots/0/serial/bytes_base64";
        let record = replacement(at, 14, uuid.len(), &["board.root-partuuid"]);
        assert!(
            public.replacements.contains(&record),
            "{:?}",
            public.replacements
        );
    }

    #[test]
    fn nested_values_keep_each_rule_once() {
        let values: Vec<Secret> = (4..300)
            .map(|length| Secret::new("board.board-serial", &vec![b'0'; length]))
            .collect();
        let finder = Finder::new(&values);
        assert!(finder.ends.iter().all(|(_, rules)| rules.len() <= 1));
        assert_eq!(
            finder.find(&[b'0'; 1000]),
            [(0..1000, rule("board.board-serial"))]
        );
    }

    #[test]
    fn the_recorded_board_description_gives_the_sensitive_lines() {
        // A description recorded with another form of the serial line, whose
        // receipt is read with it, not with the description built in.
        let line = b"  4.37 BOOTMODE: 0x06 partition 0 sn 1234abcd boardrev d04171 stc 1\r\n";
        let example = "BUILD_TIMESTAMP=1790351949 serial fedc9876 boardrev d04171 stc 4378801'\n\n";
        let text = RASPBERRY_PI_5
            .replacen(" serial (?P<serial>", " sn (?P<serial>", 1)
            .replacen(example, &example.replacen(" serial", " sn", 1), 1);
        assert_eq!(text.matches(" sn ").count(), 2);
        let built_in = public(&with_serial(line)).unwrap();
        assert!(serial_of(&built_in).windows(8).any(|w| w == b"1234abcd"));
        let mut receipt = with_serial(line);
        receipt.inputs.board_description.sha256 = Sha256::of(text.as_bytes());
        receipt.inputs.board_description.text = text;
        let public = public(&receipt).unwrap();
        assert!(!serial_of(&public).windows(8).any(|w| w == b"1234abcd"));
        // A recorded description without a form of the built-in one of its id
        // hides no less, since the built-in forms are used too.
        let block = "[[sensitive]]\nname = \"board-serial\"\n";
        let start = RASPBERRY_PI_5.find(block).unwrap();
        let end = start + RASPBERRY_PI_5[start..].find("\n\n").unwrap() + 2;
        let stripped = format!("{}{}", &RASPBERRY_PI_5[..start], &RASPBERRY_PI_5[end..]);
        assert_eq!(Board::parse(&stripped).unwrap().sensitive.len(), 7);
        let mut stripped_receipt = with_serial(NETWORK);
        let recorded = &mut stripped_receipt.inputs.board_description;
        (recorded.sha256, recorded.text) = (Sha256::of(stripped.as_bytes()), stripped);
        Receipt::parse(stripped_receipt.render().as_bytes()).unwrap();
        let hidden = self::public(&stripped_receipt).unwrap();
        let built_in = self::public(&with_serial(NETWORK)).unwrap();
        assert_eq!(hidden.replacements, built_in.replacements);
        assert!(!serial_of(&hidden).windows(8).any(|w| w == b"1234abcd"));
        // A recorded description that cannot be read gives no public version.
        receipt.inputs.board_description.text = "board".into();
        let field = "/inputs/board_description/text";
        assert_eq!(
            super::public_version(&receipt, &values()),
            Err(vec![leak(FORM_RULE, field)])
        );
        assert_eq!(
            secrets(&receipt, &values()),
            Err(vec![leak(FORM_RULE, field)])
        );
    }

    #[test]
    fn a_relay_of_at_least_4_bytes_is_hidden_wherever_it_occurs() {
        let mut receipt = example();
        for event in &mut receipt.entries[0].cold_boots[0].events {
            if let EventKind::Power { relay, .. } = &mut event.event {
                *relay = Some("1001".into());
            }
        }
        let detail = "relay 1001 did not answer".to_string();
        let role = crate::instance::Role::Power;
        let event = EventKind::DeviceError { role, detail };
        receipt.entries[0].events.push(crate::receipt::Event {
            at_ns: 1,
            received: 0,
            event,
        });
        let public = public(&receipt).unwrap();
        let at = "/entries/0/events/1/detail";
        assert!(
            public
                .replacements
                .contains(&replacement(at, 6, 4, &[RELAY_RULE]))
        );
        assert_eq!(
            public.receipt.pointer(at),
            Some(&Value::from("relay #### did not answer"))
        );
        let relay = "/entries/0/cold_boots/0/events/1/relay";
        assert!(
            public
                .replacements
                .contains(&replacement(relay, 0, 4, &[RELAY_RULE]))
        );
        // Left in place, it is a leak.
        let mut leaked = public.clone();
        *leaked.receipt.pointer_mut(at).unwrap() = "relay 1001 did not answer".into();
        leaked.replacements.retain(|r| r.field != at);
        let secrets = secrets(&receipt, &values()).unwrap();
        assert_eq!(verify(&leaked, &secrets), Err(vec![leak(RELAY_RULE, at)]));
    }

    /// How a public version treats the string at `at`.
    fn kind(at: &str) -> &'static str {
        if WITHHELD.iter().any(|(field, _)| *field == at) {
            "withheld"
        } else if TEXTS.contains(&at) {
            "text"
        } else if is_digest(at) {
            "digest"
        } else if at.ends_with("/serial/bytes_base64") {
            "console"
        } else if key(at) == "bytes_base64" {
            "stream"
        } else if key(at) == "relay" {
            "relay"
        } else {
            "plain"
        }
    }

    /// Every string of a receipt that holds every kind of event, a refusal,
    /// every list of observations and a pinned payload, by its JSON pointer
    /// with `*` for each index, has the kind that this table gives it, so a
    /// new field of a receipt is classified before a public version is made
    /// of it.
    #[test]
    fn every_string_of_a_receipt_has_its_kind() {
        use crate::observe::{BootFile, FirmwareSeen, MarkerSeen, ModeSeen, ReadSeen, SegmentSeen};
        use crate::receipt::{Check, EndReason, Event, Outcome, Refusal};
        let mut receipt = example();
        let text = |text: &str| text.to_string();
        let digest = Sha256::of(b"");
        let name = crate::names::PayloadName::parse("config.txt").unwrap();
        let generation = crate::names::GenerationName::new(2).unwrap();
        let kinds = [
            EventKind::Power {
                command: Command::On,
                relay: Some(text("1001")),
            },
            EventKind::Reading {
                quantity: crate::instance::Quantity::SupplyCurrent,
                micro: 1,
            },
            EventKind::ConsoleReady {},
            EventKind::ConsoleClosed {},
            EventKind::Send {
                step: 0,
                bytes_base64: crate::receipt::Base64(b"x".to_vec()),
            },
            EventKind::End {
                reason: EndReason::ByteLimit,
            },
            EventKind::Switch { generation },
            EventKind::DeviceError {
                role: crate::instance::Role::Console,
                detail: text("d"),
            },
        ];
        let events: Vec<Event> = kinds
            .into_iter()
            .map(|event| Event {
                at_ns: 0,
                received: 0,
                event,
            })
            .collect();
        let check = Check {
            rule: text("r"),
            subject: text("s"),
            result: Outcome::Fail,
            detail: Some(text("d")),
        };
        receipt.refusal = Some(Refusal::new("payload.staged_digest", 0, &name, text("d")));
        receipt.start.events = events.clone();
        receipt.inputs.instance.parameters.pins = vec![crate::instance::Pin {
            name: text("p"),
            files: vec![crate::instance::PinnedFile {
                name: name.clone(),
                sha256: digest.clone(),
            }],
        }];
        let entry = &mut receipt.entries[0];
        (entry.reference, entry.payload.pin) = (Some(text("r")), Some(text("p")));
        entry.events = events.clone();
        entry.checks.push(check.clone());
        let cold_boot = &mut entry.cold_boots[0];
        cold_boot.events = events;
        cold_boot.checks.push(check);
        let seen = &mut cold_boot.observations;
        seen.markers.push(MarkerSeen {
            name: text("m"),
            line: 0,
            at_ns: 0,
        });
        seen.boot_modes.push(ModeSeen {
            mode: text("m"),
            line: 0,
        });
        let groups = vec![(text("g"), text("v"))];
        seen.firmware.push(FirmwareSeen {
            name: text("f"),
            line: 0,
            groups,
        });
        let agreement = crate::observe::Agreement {
            field: text("f"),
            values: vec![text("v")],
            lines: vec![0],
        };
        (seen.identity, seen.values) = (vec![agreement.clone()], vec![agreement]);
        let read = ReadSeen {
            line: 0,
            name: Some(name.clone()),
            inside_prefix: true,
            bytes: None,
        };
        seen.tftp.reads.push(read);
        seen.boot_files.push(BootFile {
            name,
            sha256: Some(digest.clone()),
        });
        let segment = SegmentSeen {
            name: text("s"),
            from_line: 0,
            to_line: 0,
            sha256: digest,
        };
        seen.segments.push(segment);
        let mut found = BTreeMap::new();
        walk(
            &mut serde_json::to_value(&receipt).unwrap(),
            "",
            &mut |at, _| {
                let steps = at.split('/').map(|step| {
                    if step.parse::<usize>().is_ok() {
                        "*"
                    } else {
                        step
                    }
                });
                found.insert(steps.collect::<Vec<_>>().join("/"), kind(at));
            },
        );
        let events = |at: &str| {
            [
                ("event", "plain"),
                ("command", "plain"),
                ("relay", "relay"),
                ("quantity", "plain"),
                ("bytes_base64", "stream"),
                ("reason", "plain"),
                ("generation", "plain"),
                ("role", "plain"),
                ("detail", "plain"),
            ]
            .map(|(key, kind)| (format!("{at}/events/*/{key}"), kind))
        };
        let checks = |at: &str| {
            ["rule", "subject", "result", "detail"]
                .map(|key| (format!("{at}/checks/*/{key}"), "plain"))
        };
        let cold_boot = "/entries/*/cold_boots/*";
        let mut expected: BTreeMap<String, &str> = [
            ("/format", "plain"),
            ("/nonce", "withheld"),
            ("/tool/commit", "plain"),
            ("/evidence/reasons/*/rule", "plain"),
            ("/evidence/reasons/*/detail", "plain"),
            ("/bench/hardware_revision/kind", "plain"),
            ("/bench/hardware_revision/value", "digest"),
            ("/inputs/run_plan/sha256", "digest"),
            ("/inputs/run_plan/text", "text"),
            ("/inputs/board_description/id", "plain"),
            ("/inputs/board_description/sha256", "digest"),
            ("/inputs/board_description/text", "text"),
            ("/inputs/instance/sha256", "digest"),
            (
                "/inputs/instance/parameters/tftp/server_address",
                "withheld",
            ),
            ("/inputs/instance/parameters/tftp/path_prefix", "withheld"),
            ("/inputs/instance/parameters/confirmation/quantity", "plain"),
            ("/inputs/instance/parameters/identity/format", "plain"),
            ("/inputs/instance/parameters/identity/board", "plain"),
            (
                "/inputs/instance/parameters/identity/fields/*/field",
                "plain",
            ),
            (
                "/inputs/instance/parameters/identity/fields/*/part",
                "plain",
            ),
            (
                "/inputs/instance/parameters/identity/fields/*/value",
                "plain",
            ),
            (
                "/inputs/instance/parameters/identity/fields/*/value/*",
                "plain",
            ),
            (
                "/inputs/instance/parameters/identity/fields/*/read",
                "plain",
            ),
            ("/inputs/instance/parameters/pins/*/name", "plain"),
            ("/inputs/instance/parameters/pins/*/files/*/name", "plain"),
            (
                "/inputs/instance/parameters/pins/*/files/*/sha256",
                "digest",
            ),
            ("/reference", "plain"),
            ("/refusal/rule", "plain"),
            ("/refusal/subject", "plain"),
            ("/refusal/detail", "plain"),
            ("/entries/*/name", "plain"),
            ("/entries/*/reference", "plain"),
            ("/entries/*/payload/pin", "plain"),
            ("/entries/*/payload/generation", "plain"),
            ("/entries/*/payload/files/*/name", "plain"),
            ("/entries/*/payload/files/*/source_sha256", "digest"),
            ("/entries/*/payload/files/*/staged_sha256", "digest"),
            ("/entries/*/payload/files/*/served_sha256", "digest"),
            ("/entries/*/verdict", "plain"),
            ("/entries/*/cold_boots/*/serial/bytes_base64", "console"),
            ("/entries/*/cold_boots/*/verdict", "plain"),
            ("/verdict", "plain"),
        ]
        .map(|(at, kind)| (at.to_string(), kind))
        .into_iter()
        .chain(events("/start"))
        .chain(events("/entries/*"))
        .chain(events(cold_boot))
        .chain(checks("/entries/*"))
        .chain(checks(cold_boot))
        .collect();
        for (at, kind) in [
            ("markers/*/name", "plain"),
            ("boot_modes/*/mode", "plain"),
            ("firmware/*/name", "plain"),
            ("firmware/*/groups/*/*", "plain"),
            ("identity/*/field", "plain"),
            ("identity/*/values/*", "plain"),
            ("tftp/reads/*/name", "plain"),
            ("boot_files/*/name", "plain"),
            ("boot_files/*/sha256", "digest"),
            ("values/*/field", "plain"),
            ("values/*/values/*", "plain"),
            ("segments/*/name", "plain"),
            ("segments/*/sha256", "digest"),
        ] {
            expected.insert(format!("{cold_boot}/observations/{at}"), kind);
        }
        assert_eq!(found, expected);
    }

    #[test]
    fn values_of_exactly_the_byte_limit_are_searched_for() {
        let line = |length: usize| {
            let serial = "a".repeat(length);
            format!("  4.37 BOOTMODE: 0x06 serial {serial} boardrev d04171 stc 1\r\n")
        };
        // The bytes of the values to hide but the nonce, which is last.
        let bytes = |length: usize| {
            let found = secrets(&with_serial(line(length).as_bytes()), &values()).unwrap();
            let values = &found[..found.len() - 1];
            values
                .iter()
                .map(|secret| secret.value.len())
                .sum::<usize>()
        };
        let length = MAX_VALUE_BYTES - (bytes(4) - 4);
        assert_eq!(bytes(length), MAX_VALUE_BYTES);
        let found = public(&with_serial(line(length).as_bytes())).unwrap();
        assert!(!serial_of(&found).windows(4).any(|w| w == b"aaaa"));
        assert_eq!(
            public(&with_serial(line(length + 1).as_bytes())),
            Err(vec![leak(LIMIT_RULE, "")])
        );
    }

    #[test]
    fn a_public_version_always_withholds_a_nonce() {
        let mut public = public(&example()).unwrap();
        public.receipt["nonce"] = "".into();
        public.withheld.remove(0);
        match PublicReceipt::parse(public.render().as_bytes()) {
            Err(ReceiptError::Invalid { at, .. }) => assert_eq!(at, "/withheld"),
            other => panic!("{other:?}"),
        }
        let leaks = verify(&public, &[]).unwrap_err();
        assert!(leaks.contains(&leak(FORM_RULE, "/withheld")), "{leaks:?}");
    }

    #[test]
    fn adjacent_values_are_replaced_apart() {
        let receipt = with_serial(b"xabcdwxyzx\r\n");
        let instance = [
            instance_value("instance.local.console.a", "abcd"),
            instance_value("instance.local.console.b", "wxyz"),
        ];
        let public = public_version(&receipt, &instance).unwrap();
        let at = "/entries/0/cold_boots/0/serial/bytes_base64";
        let found: Vec<&Replacement> = public
            .replacements
            .iter()
            .filter(|r| r.field == at)
            .collect();
        assert_eq!(
            found,
            [
                &replacement(at, 1, 4, &["instance.local.console.a"]),
                &replacement(at, 5, 4, &["instance.local.console.b"]),
            ]
        );
        assert_eq!(serial_of(&public), b"x########x\r\n");
        let text = public.render();
        assert_eq!(PublicReceipt::parse(text.as_bytes()), Ok(public));
    }

    #[test]
    fn a_replacement_lists_each_rule_once() {
        let mut public = public(&with_serial(NETWORK)).unwrap();
        let rule = public.replacements[2].rules[0].clone();
        public.replacements[2].rules = vec![rule.clone(), rule];
        match PublicReceipt::parse(public.render().as_bytes()) {
            Err(ReceiptError::Invalid { at, .. }) => assert_eq!(at, "/replacements/2"),
            other => panic!("{other:?}"),
        }
        let leaks = verify(&public, &[]).unwrap_err();
        assert!(
            leaks.contains(&leak(FORM_RULE, "/replacements/2")),
            "{leaks:?}"
        );
    }
}
