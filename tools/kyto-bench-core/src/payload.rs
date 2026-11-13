//! The checks of a run plan entry's payload (`docs/bench/formats.md`, payload
//! checks). Before any power command, the digests of the source files and
//! of the staged copy, the writes the files can cause and the files of the
//! board's variants are checked, and a payload that breaks a rule refuses
//! the run. After the switch to the entry's generation, the digests of the
//! copy that the board is served are checked, and a mismatch is a fault.

use crate::board::Board;
use crate::digest::Sha256;
use crate::identity::{IdentityRecord, Value};
use crate::instance::PinnedFile;
use crate::names::PayloadName;
use crate::receipt::{Check, Outcome, Refusal};
use crate::write::{self, WriteClass, WriteEnables, WriteTrigger};

/// Checks the digests of the source files, read before anything is staged,
/// against the payload of entry `entry`: rule `payload.source_digest`.
pub fn check_sources(
    entry: usize,
    payload: &[PinnedFile],
    found: &[(PayloadName, Sha256)],
) -> Result<(), Refusal> {
    refuse_mismatch("payload.source_digest", "source", entry, payload, found)
}

/// Checks the digests of the staged copy, read back from where the board
/// will read it, against the payload of entry `entry`: rule
/// `payload.staged_digest`.
pub fn check_staged(
    entry: usize,
    payload: &[PinnedFile],
    found: &[(PayloadName, Sha256)],
) -> Result<(), Refusal> {
    refuse_mismatch("payload.staged_digest", "staged", entry, payload, found)
}

/// Checks the digests of the copy that the board is served, read through
/// the current generation, against the payload: one check of rule
/// `payload.served_digest` for each file, in the order of the payload, a
/// fault when the digests differ.
pub fn check_served(payload: &[PinnedFile], found: &[(PayloadName, Sha256)]) -> Vec<Check> {
    let check = |file: &PinnedFile| {
        let detail = mismatch("served", file, found);
        let result = if detail.is_some() {
            Outcome::Fault
        } else {
            Outcome::Pass
        };
        let (rule, subject) = ("payload.served_digest".into(), file.name.to_string());
        Check {
            rule,
            subject,
            result,
            detail,
        }
    };
    payload.iter().map(check).collect()
}

/// Checks the writes that the payload `files` of entry `entry` can let the
/// firmware make: a write trigger of a class that the entry does not enable
/// refuses the run (`write.not_enabled`), and so does a refused property in
/// a file, whatever the entry enables (`write.refused_property`).
pub fn check_writes(
    entry: usize,
    board: &Board,
    enables: &WriteEnables,
    files: &[(PayloadName, &[u8])],
) -> Result<(), Refusal> {
    let permitted: Vec<WriteClass> = WriteClass::ALL
        .into_iter()
        .filter(|class| enables.enabled(*class))
        .collect();
    let triggers: Vec<WriteTrigger> = board
        .write_triggers
        .iter()
        .map(|trigger| trigger.trigger.clone())
        .collect();
    let names = files.iter().map(|(name, _)| name);
    if let Err(refused) = write::check_triggers(names, &triggers, &permitted) {
        let detail = refused[0].to_string();
        return Err(Refusal::new("write.not_enabled", entry, detail));
    }
    let Some(refused) = &board.refused_properties else {
        return Ok(());
    };
    let properties: Vec<String> = refused.properties.iter().map(|p| p.name.clone()).collect();
    let files = files.iter().map(|(name, bytes)| (name, *bytes));
    match write::find_refused_properties(files, &refused.files.pattern, &properties).first() {
        Some(hit) => Err(Refusal::new(
            "write.refused_property",
            entry,
            hit.to_string(),
        )),
        None => Ok(()),
    }
}

/// Checks that the payload `names` of entry `entry` hold every file that
/// the board's variants need for the identity record: the files of the
/// variant whose value the record gives for the variant's field, ignoring
/// ASCII case, or of every variant of a field the record gives as unknown.
/// Names are compared ignoring ASCII case. A missing file refuses the run:
/// rule `payload.variant_missing`.
pub fn check_variants(
    entry: usize,
    board: &Board,
    identity: &IdentityRecord,
    names: &[PayloadName],
) -> Result<(), Refusal> {
    for variant in &board.variants {
        let (field, value) = (variant.field, &variant.value);
        let given = match identity.value(field) {
            Value::Text(given) if !given.eq_ignore_ascii_case(value) => continue,
            Value::Text(given) => given.as_str(),
            _ => "unknown",
        };
        let present =
            |file: &&PayloadName| names.iter().any(|name| name.eq_ignore_ascii_case(file));
        if let Some(file) = variant.payload.iter().find(|file| !present(file)) {
            let detail = format!("{field} is {given}, and the variant {value} needs {file}");
            return Err(Refusal::new("payload.variant_missing", entry, detail));
        }
    }
    Ok(())
}

/// The first file of `payload` whose copy in `found` is missing or has
/// another digest, as a refusal of `rule`.
fn refuse_mismatch(
    rule: &str,
    copy: &str,
    entry: usize,
    payload: &[PinnedFile],
    found: &[(PayloadName, Sha256)],
) -> Result<(), Refusal> {
    match payload.iter().find_map(|file| mismatch(copy, file, found)) {
        Some(detail) => Err(Refusal::new(rule, entry, detail)),
        None => Ok(()),
    }
}

/// How the `copy` of `file` in `found` differs from the payload, if it does.
fn mismatch(copy: &str, file: &PinnedFile, found: &[(PayloadName, Sha256)]) -> Option<String> {
    let name = &file.name;
    match found.iter().find(|(found, _)| found == name) {
        Some((_, digest)) if *digest == file.sha256 => None,
        Some((_, digest)) => Some(format!(
            "the {copy} copy of {name} has the SHA-256 {digest}, not {}",
            file.sha256
        )),
        None => Some(format!("the {copy} copy of {name} was not read")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::RASPBERRY_PI_5;

    fn pi5() -> Board {
        Board::parse(RASPBERRY_PI_5).unwrap()
    }

    fn name(text: &str) -> PayloadName {
        PayloadName::parse(text).unwrap()
    }

    fn payload() -> Vec<PinnedFile> {
        let file = |text: &str| PinnedFile {
            name: name(text),
            sha256: Sha256::of(text.as_bytes()),
        };
        vec![file("config.txt"), file("kernel.img")]
    }

    /// The digests of `payload`, with the last one of `changed` bytes.
    fn found(changed: Option<&[u8]>) -> Vec<(PayloadName, Sha256)> {
        let mut found: Vec<_> = payload()
            .into_iter()
            .map(|file| (file.name, file.sha256))
            .collect();
        if let Some(bytes) = changed {
            found[1].1 = Sha256::of(bytes);
        }
        found
    }

    /// The identity template with `soc_stepping` read as `stepping`.
    fn identity(stepping: &str) -> IdentityRecord {
        let template = include_str!("../../templates/identity.toml");
        let line = "soc_stepping          = { value = \"unknown\", read = \"visual\" }";
        assert!(template.contains(line));
        let text = template.replace(
            line,
            &format!("soc_stepping = {{ value = \"{stepping}\", read = \"visual\" }}"),
        );
        IdentityRecord::parse(&text).unwrap()
    }

    #[test]
    fn source_and_staged_digests_must_equal_the_plan() {
        let payload = payload();
        assert_eq!(check_sources(0, &payload, &found(None)), Ok(()));
        assert_eq!(check_staged(0, &payload, &found(None)), Ok(()));
        let refused = check_sources(2, &payload, &found(Some(b"other"))).unwrap_err();
        assert_eq!(
            (refused.rule.as_str(), refused.entry),
            ("payload.source_digest", Some(2))
        );
        let other = Sha256::of(b"other");
        let detail = format!(
            "the source copy of kernel.img has the SHA-256 {other}, not {}",
            payload[1].sha256
        );
        assert_eq!(refused.detail, detail);
        let refused = check_staged(1, &payload, &found(Some(b""))).unwrap_err();
        assert_eq!(
            (refused.rule.as_str(), refused.entry),
            ("payload.staged_digest", Some(1))
        );
        let refused = check_staged(1, &payload, &found(None)[..1]).unwrap_err();
        assert_eq!(refused.detail, "the staged copy of kernel.img was not read");
    }

    #[test]
    fn served_digests_give_one_check_per_file() {
        let payload = payload();
        let checks = check_served(&payload, &found(Some(b"changed")));
        let results: Vec<_> = checks
            .iter()
            .map(|check| (check.subject.as_str(), check.result))
            .collect();
        assert_eq!(
            results,
            [
                ("config.txt", Outcome::Pass),
                ("kernel.img", Outcome::Fault)
            ]
        );
        assert!(
            checks
                .iter()
                .all(|check| check.rule == "payload.served_digest")
        );
        assert_eq!(checks[0].detail, None);
        assert!(
            checks[1]
                .detail
                .as_deref()
                .unwrap()
                .starts_with("the served copy of kernel.img has")
        );
        assert!(
            check_served(&payload, &found(None))
                .iter()
                .all(|check| check.result == Outcome::Pass)
        );
    }

    #[test]
    fn a_trigger_needs_its_class_enabled() {
        let board = pi5();
        let files = [
            (name("config.txt"), &b"uart_2ndstage=1\n"[..]),
            (name("PIEEPROM.UPD"), b""),
        ];
        let refused = check_writes(1, &board, &WriteEnables::default(), &files).unwrap_err();
        assert_eq!(
            (refused.rule.as_str(), refused.entry),
            ("write.not_enabled", Some(1))
        );
        assert!(
            refused
                .detail
                .contains("PIEEPROM.UPD lets the firmware write eeprom"),
            "{}",
            refused.detail
        );
        let storage = WriteEnables {
            storage: true,
            ..WriteEnables::default()
        };
        assert!(check_writes(1, &board, &storage, &files).is_err());
        let eeprom = WriteEnables {
            eeprom: true,
            ..WriteEnables::default()
        };
        assert_eq!(check_writes(1, &board, &eeprom, &files), Ok(()));
        assert_eq!(
            check_writes(1, &board, &WriteEnables::default(), &files[..1]),
            Ok(())
        );
    }

    #[test]
    fn a_refused_property_is_refused_even_when_writes_are_enabled() {
        let board = pi5();
        let every = WriteEnables {
            firmware: true,
            eeprom: true,
            storage: true,
        };
        let files = [(
            name("config.txt"),
            &b"uart_2ndstage=1\r\n  PROGRAM_PUBKEY = 1\n"[..],
        )];
        let refused = check_writes(0, &board, &every, &files).unwrap_err();
        assert_eq!(refused.rule, "write.refused_property");
        assert!(
            refused
                .detail
                .contains("config.txt sets the refused property program_pubkey"),
            "{}",
            refused.detail
        );
        let files = [(name("extra.cfg"), &b"# program_pubkey=1\n"[..])];
        assert_eq!(check_writes(0, &board, &every, &files), Ok(()));
    }

    #[test]
    fn a_missing_variant_file_is_refused_when_the_stepping_is_unknown() {
        let board = pi5();
        let (dtb, overlay) = (name("bcm2712-rpi-5-b.dtb"), name("overlays/bcm2712d0.dtbo"));
        let both = [
            name("config.txt"),
            name("BCM2712-RPI-5-B.DTB"),
            overlay.clone(),
        ];
        for stepping in ["unknown", "D0", "d0", "C1"] {
            assert_eq!(
                check_variants(0, &board, &identity(stepping), &both),
                Ok(()),
                "{stepping}"
            );
        }
        let refused = check_variants(3, &board, &identity("unknown"), std::slice::from_ref(&dtb))
            .unwrap_err();
        assert_eq!(
            (refused.rule.as_str(), refused.entry),
            ("payload.variant_missing", Some(3))
        );
        assert_eq!(
            refused.detail,
            "soc_stepping is unknown, and the variant D0 needs overlays/bcm2712d0.dtbo"
        );
        assert!(check_variants(0, &board, &identity("D0"), std::slice::from_ref(&dtb)).is_err());
        // A known stepping needs the files of its own variant only.
        assert_eq!(check_variants(0, &board, &identity("C1"), &[dtb]), Ok(()));
        assert!(check_variants(0, &board, &identity("C1"), &[overlay]).is_err());
        assert_eq!(check_variants(0, &board, &identity("B0"), &[]), Ok(()));
    }
}
