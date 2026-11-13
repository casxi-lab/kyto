//! Public versions of receipts: the receipt with every value that
//! identifies the board, the network or the bench replaced by `#` of equal
//! length, and a list of what was replaced and why (`docs/bench/formats.md`,
//! public versions). [`public_version`] makes one and checks it with
//! [`verify`], which finds any value that remains.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ops::Range;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::board::Board;
use crate::digest::Sha256;
use crate::instance::{InstanceValue, MIN_SEARCHED_LEN};
use crate::lines;
use crate::receipt::{Receipt, ReceiptError, read_rendered, render_json};

/// The `format` of a public version.
pub const FORMAT: &str = "kyto-bench public receipt";

/// The `format_version` of a public version that this tool writes.
pub const FORMAT_VERSION: i64 = 1;

/// The byte that replaces every byte of a value.
pub const PLACEHOLDER: u8 = b'#';

/// The rule of the relay of a power event.
pub const RELAY_RULE: &str = "instance.local.power.relay";

/// The rule of the nonce.
pub const NONCE_RULE: &str = "nonce";

/// The rule of the unterminated last line of a cold boot's console output.
pub const UNTERMINATED_RULE: &str = "console.unterminated_line";

/// The rule of a segment whose digest covers replaced bytes.
pub const SEGMENT_RULE: &str = "segment.covers_hidden";

/// The rule of a receipt with more values to hide, or more bytes of them,
/// than a public version searches for.
pub const LIMIT_RULE: &str = "public.values_limit";

/// The most values to hide that a public version searches for.
pub const MAX_VALUES: usize = 1024;

/// The most bytes of the values to hide that a public version searches for.
pub const MAX_VALUE_BYTES: usize = 64 << 10;

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

/// A field that a public version always withholds, by its JSON pointer in
/// the receipt.
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

impl PublicReceipt {
    /// The public version as the tool writes it, in the form of receipts.
    pub fn render(&self) -> String {
        render_json(self)
    }

    /// Reads a public version as a receipt is read, but for the rules that
    /// tie the parts of a receipt together: its `receipt` is kept as JSON,
    /// since a replaced value need not keep the form of its field.
    pub fn parse(text: &[u8]) -> Result<Self, ReceiptError> {
        read_rendered(text, FORMAT)
    }
}

/// The values of at least 4 bytes that the public version of `receipt`
/// must not show, sorted: each value that a sensitive line of `board` holds
/// in a recorded stream, under `board.<name>`; the TFTP parameters that the
/// receipt records; the `instance` values; and the nonce, last. Beyond
/// [`MAX_VALUES`] or [`MAX_VALUE_BYTES`], the error is a leak of
/// [`LIMIT_RULE`] at the whole receipt.
pub fn secrets(
    receipt: &Receipt,
    board: &Board,
    instance: &[InstanceValue],
) -> Result<Vec<Secret>, Vec<Leak>> {
    let mut found = BTreeSet::new();
    let mut add = |rule: &str, value: &[u8]| {
        if value.len() >= MIN_SEARCHED_LEN && found.len() <= MAX_VALUES {
            found.insert(Secret::new(rule, value));
        }
    };
    walk(&mut to_value(receipt), "", &mut |at, text| {
        for (_, rule, value) in hits(board, &decoded(at, text).unwrap_or_default()) {
            add(&rule, &value);
        }
    });
    for (_, rule, value) in withheld_fields(receipt).into_iter().skip(1) {
        add(rule, value.as_bytes());
    }
    for value in instance {
        add(&value.rule, value.value.as_bytes());
    }
    let bytes: usize = found.iter().map(|secret| secret.value.len()).sum();
    if found.len() > MAX_VALUES || bytes > MAX_VALUE_BYTES {
        let (rule, field) = (LIMIT_RULE.into(), String::new());
        return Err(vec![Leak { rule, field }]);
    }
    let nonce = Secret::new(NONCE_RULE, receipt.nonce.as_bytes());
    Ok(found.into_iter().chain([nonce]).collect())
}

/// Makes the public version of `receipt` on `board`, whose instance values
/// are `instance`, as `docs/bench/formats.md` describes, and checks it with
/// [`verify`]; the leaks are the error.
pub fn public_version(
    receipt: &Receipt,
    board: &Board,
    instance: &[InstanceValue],
) -> Result<PublicReceipt, Vec<Leak>> {
    let secrets = secrets(receipt, board, instance)?;
    let finder = Finder::new(&secrets[..secrets.len() - 1]);
    let mut value = to_value(receipt);
    let mut withheld = Vec::new();
    for (field, rule, text) in withheld_fields(receipt) {
        if let Some(slot) = value.pointer_mut(field).filter(|_| !text.is_empty()) {
            *slot = Value::String(placeholders(text.len()));
            let (field, rule) = (field.into(), rule.into());
            let length = text.len();
            withheld.push(Withheld {
                field,
                length,
                rule,
            });
        }
    }
    let mut replacements = Vec::new();
    walk(&mut value, "", &mut |at, text| {
        let kept = ["/inputs/run_plan/text", "/inputs/board_description/text"];
        if kept.contains(&at) || is_digest(at) || withheld.iter().any(|w| w.field == at) {
            return;
        }
        let stream = decoded(at, text);
        let mut bytes = stream.clone().unwrap_or_else(|| text.as_bytes().to_vec());
        let mut spans = finder.find(&bytes);
        if key(at) == "relay" {
            spans.push((0..bytes.len(), vec![RELAY_RULE.into()]));
        }
        if stream.is_some() {
            let hits = hits(board, &bytes).into_iter();
            spans.extend(hits.map(|(span, rule, _)| (span, vec![rule])));
        }
        let open = unterminated(at, &bytes);
        spans.extend(open.map(|span| (span, vec![UNTERMINATED_RULE.into()])));
        if stream.is_none() {
            // Whole characters are replaced, so the string stays UTF-8.
            for (span, _) in &mut spans {
                *span = text.floor_char_boundary(span.start)..text.ceil_char_boundary(span.end);
            }
        }
        for (span, rules) in merge(spans) {
            bytes[span.clone()].fill(PLACEHOLDER);
            let (field, offset, length) = (at.to_string(), span.start, span.len());
            replacements.push(Replacement {
                field,
                offset,
                length,
                rules,
            });
        }
        *text = match stream {
            Some(_) => STANDARD.encode(&bytes),
            None => String::from_utf8_lossy(&bytes).into_owned(),
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
    verify(&public, board, &secrets)?;
    Ok(public)
}

/// The leak check of a public version, whose values to hide are `secrets`,
/// as `docs/bench/formats.md` describes. Each leak names its rule and its
/// field, never its value.
pub fn verify(public: &PublicReceipt, board: &Board, secrets: &[Secret]) -> Result<(), Vec<Leak>> {
    let finder = Finder::new(secrets);
    let mut leaks = Vec::new();
    walk(&mut public.receipt.clone(), "", &mut |at, text| {
        let stream = decoded(at, text);
        let bytes = stream.as_deref().unwrap_or(text.as_bytes());
        let found = finder.find(bytes).into_iter().flat_map(|(_, rules)| rules);
        let mut rules: Vec<String> = found.collect();
        if stream.is_some() {
            let held = hits(board, bytes)
                .into_iter()
                .filter(|(_, _, value)| !is_placeholders(value));
            rules.extend(held.map(|(_, rule, _)| rule));
        }
        let open = unterminated(at, bytes).is_some_and(|span| !is_placeholders(&bytes[span]));
        let relay = key(at) == "relay" && !is_placeholders(text.as_bytes());
        let found = [relay, open, covers_hidden(public, at)];
        for (rule, found) in [RELAY_RULE, UNTERMINATED_RULE, SEGMENT_RULE]
            .iter()
            .zip(found)
        {
            rules.extend(found.then(|| rule.to_string()));
        }
        rules.sort();
        rules.dedup();
        let field = at.to_string();
        leaks.extend(rules.into_iter().map(|rule| Leak {
            rule,
            field: field.clone(),
        }));
    });
    leaks.sort();
    leaks.dedup();
    if leaks.is_empty() { Ok(()) } else { Err(leaks) }
}

/// The fields that a public version always withholds, with their rules and
/// recorded values, the nonce first.
fn withheld_fields(receipt: &Receipt) -> [(&'static str, &'static str, &str); 3] {
    let tftp = &receipt.inputs.instance.parameters.tftp;
    [
        ("/nonce", NONCE_RULE, &receipt.nonce),
        (
            "/inputs/instance/parameters/tftp/server_address",
            "instance.tftp.server_address",
            &tftp.server_address,
        ),
        (
            "/inputs/instance/parameters/tftp/path_prefix",
            "instance.tftp.path_prefix",
            &tftp.path_prefix,
        ),
    ]
}

fn to_value(receipt: &Receipt) -> Value {
    serde_json::to_value(receipt).expect("a receipt is JSON")
}

/// Calls `visit` with the JSON pointer and the text of every string of
/// `value`, in the order of the document; keys are not visited.
fn walk(value: &mut Value, at: &str, visit: &mut dyn FnMut(&str, &mut String)) {
    let children: Vec<(String, &mut Value)> = match value {
        Value::String(text) => return visit(at, text),
        Value::Array(items) => items
            .iter_mut()
            .enumerate()
            .map(|(i, item)| (i.to_string(), item))
            .collect(),
        Value::Object(map) => map
            .iter_mut()
            .map(|(key, item)| (key.clone(), item))
            .collect(),
        _ => return,
    };
    for (key, child) in children {
        walk(child, &format!("{at}/{key}"), visit);
    }
}

/// The last key of the JSON pointer `at`.
fn key(at: &str) -> &str {
    at.rsplit('/').next().unwrap_or_default()
}

/// Whether the field at `at` is a digest.
fn is_digest(at: &str) -> bool {
    let key = key(at);
    key == "sha256" || key.ends_with("_sha256") || at == "/bench/hardware_revision/value"
}

/// The bytes of the field at `at` when it is a recorded stream: the console
/// output of a cold boot or the bytes of a send.
fn decoded(at: &str, text: &str) -> Option<Vec<u8>> {
    (key(at) == "bytes_base64")
        .then(|| STANDARD.decode(text).ok())
        .flatten()
}

/// The spans of `stream` that the named groups of the sensitive lines of
/// `board` hold, in every line, each with its rule `board.<name>` and its
/// bytes; empty spans and values that the board never hides are left out.
fn hits(board: &Board, stream: &[u8]) -> Vec<(Range<usize>, String, Vec<u8>)> {
    let mut found = Vec::new();
    for line in lines::split([(0, stream)]) {
        let start = line.offset as usize;
        for sensitive in &board.sensitive {
            let groups = sensitive.form.pattern.named_groups(&line.content);
            for (_, span) in groups.into_iter().flatten() {
                let value = &line.content[span.clone()];
                if !value.is_empty() && !board.never.iter().any(|never| never.as_bytes() == value) {
                    let rule = format!("board.{}", sensitive.name);
                    found.push((start + span.start..start + span.end, rule, value.to_vec()));
                }
            }
        }
    }
    found
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
fn covers_hidden(public: &PublicReceipt, at: &str) -> bool {
    let segment = at.strip_suffix("/sha256").unwrap_or_default();
    let Some((cold_boot, _)) = segment.split_once("/observations/segments/") else {
        return false;
    };
    let [from, to] = ["from_line", "to_line"].map(|key| {
        let line = public.receipt.pointer(&format!("{segment}/{key}"));
        line.and_then(Value::as_u64).unwrap_or(0) as usize
    });
    let serial = format!("{cold_boot}/serial/bytes_base64");
    let replaced =
        |r: &Replacement| r.field == serial && r.offset < to && r.offset + r.length > from;
    public.replacements.iter().any(replaced)
}

/// Every value to hide found in one pass over bytes, as the Aho-Corasick
/// algorithm finds them: a trie of the values in which each node has the
/// node of its longest proper suffix, and the length of the longest value
/// that ends at it with the rules of every value that does.
struct Finder {
    next: Vec<BTreeMap<u8, usize>>,
    suffix: Vec<usize>,
    ends: Vec<(usize, Vec<String>)>,
}

impl Finder {
    fn new(secrets: &[Secret]) -> Self {
        let (next, suffix, ends) = (vec![BTreeMap::new()], vec![0], vec![(0, Vec::new())]);
        let mut finder = Self { next, suffix, ends };
        for secret in secrets {
            let mut node = 0;
            for byte in &secret.value {
                let new = finder.next.len();
                node = *finder.next[node].entry(*byte).or_insert(new);
                if node == new {
                    finder.next.push(BTreeMap::new());
                    finder.suffix.push(0);
                    finder.ends.push((0, Vec::new()));
                }
            }
            finder.ends[node].0 = secret.value.len();
            finder.ends[node].1.push(secret.rule.clone());
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

    /// The node after `node` on `byte`.
    fn step(&self, mut node: usize, byte: u8) -> usize {
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
    fn find(&self, bytes: &[u8]) -> Vec<(Range<usize>, Vec<String>)> {
        let (mut node, mut found) = (0, Vec::<(Range<usize>, Vec<String>)>::new());
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
                rules.sort();
                rules.dedup();
                found.push((span, rules));
            }
        }
        found
    }
}

/// Merges spans that overlap or are equal into one, with every rule once,
/// in order; empty spans are left out.
fn merge(mut spans: Vec<(Range<usize>, Vec<String>)>) -> Vec<(Range<usize>, Vec<String>)> {
    spans.sort_by_key(|(span, _)| (span.start, span.end));
    let mut merged: Vec<(Range<usize>, Vec<String>)> = Vec::new();
    for (span, rules) in spans.into_iter().filter(|(span, _)| !span.is_empty()) {
        match merged.last_mut() {
            Some((last, all)) if span.start < last.end => {
                last.end = last.end.max(span.end);
                all.extend(rules);
            }
            _ => merged.push((span, rules)),
        }
    }
    for (_, rules) in &mut merged {
        rules.sort();
        rules.dedup();
    }
    merged
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

    fn pi5() -> Board {
        Board::parse(RASPBERRY_PI_5).unwrap()
    }

    fn values() -> Vec<InstanceValue> {
        Instance::parse(&instance_text())
            .unwrap()
            .deidentify_values()
    }

    /// The example receipt whose cold boot received `bytes` in one chunk.
    fn with_serial(bytes: &[u8]) -> Receipt {
        let mut receipt = example();
        receipt.entries[0].cold_boots[0].serial = Serial {
            length: bytes.len() as u64,
            bytes_base64: crate::receipt::Base64(bytes.to_vec()),
            index: vec![[0, 6_100_000_000]],
        };
        receipt
    }

    fn public(receipt: &Receipt) -> Result<PublicReceipt, Vec<Leak>> {
        public_version(receipt, &pi5(), &values())
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
        let secrets = secrets(&receipt, &pi5(), &values()).unwrap();
        assert_eq!(
            verify(&public, &pi5(), &secrets),
            Err(vec![leak("nonce", "/nonce")])
        );
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
        let secrets = secrets(&receipt, &pi5(), &values()).unwrap();
        assert_eq!(
            verify(&leaked, &pi5(), &secrets),
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
        let found = Err(vec![leak("board.board-serial", at)]);
        assert_eq!(verify(&public, &pi5(), &[]), found);
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
        let secrets = secrets(&receipt, &pi5(), &values()).unwrap();
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
        let public = public_version(&receipt, &pi5(), &instance).unwrap();
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
        let cut: [&[u8]; 2] = [
            b"READY\r\n  4.37 BOOTMODE: 0x06 partition 0 serial 1234abcd boardr",
            b"READY\r\n  5.51 NETWORK: 00:00:5e:00:53:01 wait for link ",
        ];
        for bytes in cut {
            let public = public(&with_serial(bytes)).unwrap();
            let rest = bytes.len() - 7;
            assert_eq!(
                public.replacements,
                [replacement(at, 7, rest, &[UNTERMINATED_RULE])]
            );
            assert_eq!(
                serial_of(&public),
                [&b"READY\r\n"[..], &vec![b'#'; rest]].concat()
            );
            // The line put back is a leak, though no form matches it.
            let mut leaked = public.clone();
            *leaked.receipt.pointer_mut(at).unwrap() = STANDARD.encode(bytes).into();
            let secrets = secrets(&with_serial(bytes), &pi5(), &values()).unwrap();
            assert_eq!(
                verify(&leaked, &pi5(), &secrets),
                Err(vec![leak(UNTERMINATED_RULE, at)])
            );
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
        assert!(public(&receipt).is_ok());
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
        assert_eq!(PublicReceipt::parse(text.as_bytes()), Ok(public));
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
    }
}
