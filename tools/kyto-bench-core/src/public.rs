//! Public versions of receipts: the receipt with every value that
//! identifies the board, the network or the bench replaced by `#` of equal
//! length, and a list of what was replaced and why (`docs/bench/formats.md`,
//! public versions). [`public_version`] makes one and checks it with
//! [`verify`], which finds any value that remains.

use std::ops::Range;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::board::Board;
use crate::digest::Sha256;
use crate::document::render_json;
use crate::instance::{InstanceValue, MIN_SEARCHED_LEN};
use crate::lines;
use crate::receipt::Receipt;

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

/// A value that a public version must not show, with the rule that hides it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Secret {
    pub rule: String,
    pub value: Vec<u8>,
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
}

/// The values of at least 4 bytes that the public version of `receipt`
/// must not show: each value that a sensitive line of `board` holds in a
/// recorded stream, under `board.<name>`; the TFTP parameters that the
/// receipt records; the `instance` values; and the nonce, last.
pub fn secrets(receipt: &Receipt, board: &Board, instance: &[InstanceValue]) -> Vec<Secret> {
    let secret = |rule: &str, value: &[u8]| Secret {
        rule: rule.into(),
        value: value.into(),
    };
    let mut found = Vec::new();
    walk(&mut to_value(receipt), "", &mut |at, text| {
        let stream = decoded(at, text).unwrap_or_default();
        found.extend(
            hits(board, &stream)
                .iter()
                .map(|(_, rule, value)| secret(rule, value)),
        );
    });
    let recorded = withheld_fields(receipt).into_iter().skip(1);
    found.extend(recorded.map(|(_, rule, value)| secret(rule, value.as_bytes())));
    found.extend(
        instance
            .iter()
            .map(|value| secret(&value.rule, value.value.as_bytes())),
    );
    found.retain(|secret| secret.value.len() >= MIN_SEARCHED_LEN);
    found.sort();
    found.dedup();
    found.push(secret(NONCE_RULE, receipt.nonce.as_bytes()));
    found
}

/// Makes the public version of `receipt` on `board`, whose instance values
/// are `instance`, and checks it with [`verify`]; the leaks are the error.
///
/// The nonce, the TFTP server address and a path prefix that is not empty
/// are withheld. In every recorded stream, every span of a sensitive line's
/// named group is replaced, and in every stream and every other string,
/// every occurrence of a value of [`secrets`] but the nonce; the relay of
/// every power event is replaced whole. Overlapping spans are one
/// replacement with every rule. The recorded run plan and board
/// description, every digest and every key are never rewritten.
pub fn public_version(
    receipt: &Receipt,
    board: &Board,
    instance: &[InstanceValue],
) -> Result<PublicReceipt, Vec<Leak>> {
    let secrets = secrets(receipt, board, instance);
    let searched = &secrets[..secrets.len() - 1];
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
        let mut spans = Vec::new();
        if key(at) == "relay" {
            spans.push((0..bytes.len(), RELAY_RULE.to_string()));
        }
        if stream.is_some() {
            spans.extend(
                hits(board, &bytes)
                    .into_iter()
                    .map(|(span, rule, _)| (span, rule)),
            );
        }
        for secret in searched {
            let found = occurrences(&bytes, &secret.value);
            spans.extend(found.map(|span| (span, secret.rule.clone())));
        }
        if stream.is_none() {
            // Whole characters are replaced, so the string stays UTF-8.
            for (span, _) in &mut spans {
                while !text.is_char_boundary(span.start) {
                    span.start -= 1;
                }
                while !text.is_char_boundary(span.end) {
                    span.end += 1;
                }
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

/// The leak check of a public version: no value of `secrets` occurs in any
/// string of its receipt, the decoded bytes of its streams, its recorded
/// texts and its digests included; the sensitive lines of `board` hold
/// nothing but placeholders in its streams; and every relay is
/// placeholders. Each leak names its rule and its field, never its value.
pub fn verify(public: &PublicReceipt, board: &Board, secrets: &[Secret]) -> Result<(), Vec<Leak>> {
    let mut leaks = Vec::new();
    walk(&mut public.receipt.clone(), "", &mut |at, text| {
        let stream = decoded(at, text);
        let bytes = stream.as_deref().unwrap_or(text.as_bytes());
        let found = secrets
            .iter()
            .filter(|secret| occurrences(bytes, &secret.value).next().is_some());
        let mut rules: Vec<String> = found.map(|secret| secret.rule.clone()).collect();
        if stream.is_some() {
            let held = hits(board, bytes)
                .into_iter()
                .filter(|(_, _, value)| !is_placeholders(value));
            rules.extend(held.map(|(_, rule, _)| rule));
        }
        if key(at) == "relay" && !is_placeholders(text.as_bytes()) {
            rules.push(RELAY_RULE.into());
        }
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

/// The spans of every occurrence of `value` in `bytes`, overlapping ones too.
fn occurrences<'a>(bytes: &'a [u8], value: &'a [u8]) -> impl Iterator<Item = Range<usize>> + 'a {
    let windows = bytes.windows(value.len().max(1)).enumerate();
    windows
        .filter(move |(_, window)| *window == value)
        .map(move |(at, _)| at..at + value.len())
}

/// Merges spans that overlap or are equal into one, with every rule once,
/// in order; empty spans are left out.
fn merge(mut spans: Vec<(Range<usize>, String)>) -> Vec<(Range<usize>, Vec<String>)> {
    spans.sort_by_key(|(span, _)| (span.start, span.end));
    let mut merged: Vec<(Range<usize>, Vec<String>)> = Vec::new();
    for (span, rule) in spans.into_iter().filter(|(span, _)| !span.is_empty()) {
        match merged.last_mut() {
            Some((last, rules)) if span.start < last.end => {
                last.end = last.end.max(span.end);
                rules.push(rule);
            }
            _ => merged.push((span, vec![rule])),
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
        let secrets = secrets(&receipt, &pi5(), &values());
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
        let secrets = secrets(&receipt, &pi5(), &values());
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
        let secrets = secrets(&receipt, &pi5(), &values());
        assert!(!secrets.iter().any(|secret| secret.value == b"0.0.0.0"));
    }
}
