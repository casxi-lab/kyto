//! The checks of a run plan entry's payload (`docs/bench/formats.md`, payload
//! checks). Before any power command, the digests of the source files, the
//! writes the files can cause and the files of the board's variants are
//! checked, then the digests of the staged copy, and a payload that breaks a
//! rule refuses the run. After the switch to the entry's generation, the
//! digests of the copy that the board is served are checked, and a mismatch
//! is a fault.

use crate::board::Board;
use crate::digest::Sha256;
use crate::identity::{IdentityRecord, Value};
use crate::instance::PinnedFile;
use crate::names::PayloadName;
use crate::receipt::{Check, Outcome, Refusal};
use crate::write::{self, WriteClass, WriteEnables, WriteTrigger};

/// Checks the payload of entry `entry` before anything is staged, in this
/// order: for each file of `payload`, in increasing order of the bytes of
/// its name, `payload.source_digest` against the digests of the source files
/// in `sources`, `write.not_enabled` against the board's write triggers and
/// `enables`, and `write.refused_property`, the refused property that
/// `refused` finds in the file, if any ([`refused_property`]); then
/// `payload.variant_missing` for the identity record ([`check_variants`]).
/// The first that fails refuses the run.
pub fn check_entry(
    entry: usize,
    board: &Board,
    enables: &WriteEnables,
    identity: &IdentityRecord,
    payload: &[PinnedFile],
    sources: &[(PayloadName, Sha256)],
    refused: &dyn Fn(&PayloadName) -> Option<String>,
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
    for file in by_name(payload) {
        let name = &file.name;
        let refusal = |rule, detail| Err(Refusal::new(rule, entry, name, detail));
        if let Some(detail) = mismatch("source", file, sources) {
            return refusal("payload.source_digest", detail);
        }
        if let Err(unpermitted) = write::check_triggers([name], &triggers, &permitted) {
            return refusal("write.not_enabled", unpermitted[0].to_string());
        }
        if let Some(detail) = refused(name) {
            return refusal("write.refused_property", detail);
        }
    }
    let names: Vec<PayloadName> = payload.iter().map(|file| file.name.clone()).collect();
    check_variants(entry, board, identity, &names)
}

/// The first place in the payload file `name`, of `bytes`, that sets a
/// refused property of the board, as a detail of `write.refused_property`.
pub fn refused_property(board: &Board, name: &PayloadName, bytes: &[u8]) -> Option<String> {
    let refused = board.refused_properties.as_ref()?;
    let properties: Vec<String> = refused.properties.iter().map(|p| p.name.clone()).collect();
    let hits = write::find_refused_properties([(name, bytes)], &refused.files.pattern, &properties);
    hits.first().map(ToString::to_string)
}

/// Checks the digests of the staged copy, read back from where the board
/// will read it, against the payload of entry `entry`, file by file in
/// increasing order of the bytes of their names: rule
/// `payload.staged_digest`.
pub fn check_staged(
    entry: usize,
    payload: &[PinnedFile],
    found: &[(PayloadName, Sha256)],
) -> Result<(), Refusal> {
    let first = by_name(payload).find_map(|file| Some((file, mismatch("staged", file, found)?)));
    match first {
        Some((file, detail)) => Err(Refusal::new(
            "payload.staged_digest",
            entry,
            &file.name,
            detail,
        )),
        None => Ok(()),
    }
}

/// Checks the digests of the copy that the board is served, read through
/// the current generation, against the payload: one check of rule
/// `payload.served_digest` for each file, in the order of the payload, a
/// fault when the digests differ.
pub fn check_served(payload: &[PinnedFile], found: &[(PayloadName, Sha256)]) -> Vec<Check> {
    let check = |file: &PinnedFile| {
        let detail = mismatch("served", file, found);
        Check::new(
            "payload.served_digest",
            file.name.as_str(),
            Outcome::Fault,
            detail,
        )
    };
    payload.iter().map(check).collect()
}

/// Checks that the payload `names` of entry `entry` hold every file that
/// the board's variants need for the identity record: the files of the
/// variant whose value the record gives for the variant's field, ignoring
/// ASCII case, or of every variant of a field the record gives as unknown.
/// Names are compared ignoring ASCII case. A missing file refuses the run:
/// rule `payload.variant_missing`, with the first missing file of the first
/// variant of the board that misses one.
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
            return Err(Refusal::new("payload.variant_missing", entry, file, detail));
        }
    }
    Ok(())
}

/// The files of `payload` in increasing order of the bytes of their names.
fn by_name(payload: &[PinnedFile]) -> impl Iterator<Item = &PinnedFile> {
    let mut files: Vec<&PinnedFile> = payload.iter().collect();
    files.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    files.into_iter()
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

    /// The pinned files of `files`, each with the digest of its bytes.
    fn pinned(files: &[(&str, &[u8])]) -> Vec<PinnedFile> {
        let pin = |(text, bytes): &(&str, &[u8])| PinnedFile {
            name: name(text),
            sha256: Sha256::of(bytes),
        };
        files.iter().map(pin).collect()
    }

    /// Checks entry `entry` of `files`, whose sources are `sources`, as a
    /// run checks them, with the refused properties found in the sources.
    fn check(
        entry: usize,
        enables: &WriteEnables,
        files: &[PinnedFile],
        sources: &[(&str, &[u8])],
    ) -> Result<(), Refusal> {
        let board = pi5();
        let found: Vec<_> = sources
            .iter()
            .map(|(text, bytes)| (name(text), Sha256::of(bytes)))
            .collect();
        let refused = |file: &PayloadName| {
            let (_, bytes) = sources.iter().find(|(text, _)| *text == file.as_str())?;
            refused_property(&board, file, bytes)
        };
        let identity = identity("unknown");
        check_entry(entry, &board, enables, &identity, files, &found, &refused)
    }

    const VARIANTS: [(&str, &[u8]); 2] = [
        ("bcm2712-rpi-5-b.dtb", b"dtb"),
        ("overlays/bcm2712d0.dtbo", b"dtbo"),
    ];

    #[test]
    fn source_and_staged_digests_must_equal_the_plan() {
        let payload = payload();
        let sources: Vec<(&str, &[u8])> = [
            ("config.txt", &b"config.txt"[..]),
            ("kernel.img", b"kernel.img"),
        ]
        .into_iter()
        .chain(VARIANTS)
        .collect();
        let mut files = payload.clone();
        files.extend(pinned(&VARIANTS));
        let none = WriteEnables::default();
        assert_eq!(check(0, &none, &files, &sources), Ok(()));
        assert_eq!(check_staged(0, &payload, &found(None)), Ok(()));
        let mut changed = sources.clone();
        changed[1].1 = b"other";
        let refused = check(2, &none, &files, &changed).unwrap_err();
        assert_eq!(
            (
                refused.rule.as_str(),
                refused.entry,
                refused.subject.as_str()
            ),
            ("payload.source_digest", Some(2), "kernel.img")
        );
        let other = Sha256::of(b"other");
        let detail = format!(
            "the source copy of kernel.img has the SHA-256 {other}, not {}",
            payload[1].sha256
        );
        assert_eq!(refused.detail, detail);
        let refused = check(0, &none, &files, &sources[1..]).unwrap_err();
        assert_eq!(refused.detail, "the source copy of config.txt was not read");
        let refused = check_staged(1, &payload, &found(Some(b""))).unwrap_err();
        assert_eq!(
            (
                refused.rule.as_str(),
                refused.entry,
                refused.subject.as_str()
            ),
            ("payload.staged_digest", Some(1), "kernel.img")
        );
        let refused = check_staged(1, &payload, &found(None)[..1]).unwrap_err();
        assert_eq!(refused.detail, "the staged copy of kernel.img was not read");
    }

    /// The checks of an entry go by file, in the order of the bytes of the
    /// names, and by rule within a file; the variants come last.
    #[test]
    fn payload_checks_go_by_file_name_then_by_rule() {
        let none = WriteEnables::default();
        let first = |files: &[(&str, &[u8])], sources: &[(&str, &[u8])]| {
            let refused = check(0, &none, &pinned(files), sources).unwrap_err();
            (refused.rule, refused.subject)
        };
        let rule = |rule: &str, subject: &str| (rule.to_string(), subject.to_string());
        // a.txt comes before recovery.bin, whatever the order of the plan.
        let files: [(&str, &[u8]); 2] = [("recovery.bin", b"r"), ("a.txt", b"a")];
        let sources: [(&str, &[u8]); 2] = [("recovery.bin", b"r"), ("a.txt", b"x")];
        assert_eq!(
            first(&files, &sources),
            rule("payload.source_digest", "a.txt")
        );
        // Upper case letters come before lower case ones.
        let files: [(&str, &[u8]); 2] = [("a.txt", b"a"), ("PIEEPROM.UPD", b"p")];
        let sources: [(&str, &[u8]); 2] = [("a.txt", b"x"), ("PIEEPROM.UPD", b"p")];
        assert_eq!(
            first(&files, &sources),
            rule("write.not_enabled", "PIEEPROM.UPD")
        );
        // Within a file, its source digest comes before its trigger.
        let sources: [(&str, &[u8]); 2] = [("a.txt", b"x"), ("PIEEPROM.UPD", b"q")];
        assert_eq!(
            first(&files, &sources),
            rule("payload.source_digest", "PIEEPROM.UPD")
        );
        // Every rule of a file comes before the next file.
        let files: [(&str, &[u8]); 2] = [("kernel.img", b"k"), ("config.txt", b"program_pubkey=1")];
        let sources: [(&str, &[u8]); 2] =
            [("kernel.img", b"x"), ("config.txt", b"program_pubkey=1")];
        assert_eq!(
            first(&files, &sources),
            rule("write.refused_property", "config.txt")
        );
        // Every file passes before the variants are checked.
        let files: [(&str, &[u8]); 1] = [("config.txt", b"c")];
        let refused = check(4, &none, &pinned(&files), &files).unwrap_err();
        assert_eq!(
            (
                refused.rule.as_str(),
                refused.entry,
                refused.subject.as_str()
            ),
            ("payload.variant_missing", Some(4), "bcm2712-rpi-5-b.dtb")
        );
        assert_eq!(
            first(&files, &[]),
            rule("payload.source_digest", "config.txt")
        );
        // Staged copies are checked by name as well.
        let files = pinned(&[("b", b"b"), ("A", b"a")]);
        let refused = check_staged(0, &files, &[]).unwrap_err();
        assert_eq!(refused.subject, "A");
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
        let sources: Vec<(&str, &[u8])> = [
            ("config.txt", &b"uart_2ndstage=1\n"[..]),
            ("PIEEPROM.UPD", b""),
        ]
        .into_iter()
        .chain(VARIANTS)
        .collect();
        let files = pinned(&sources);
        let refused = check(1, &WriteEnables::default(), &files, &sources).unwrap_err();
        assert_eq!(
            (
                refused.rule.as_str(),
                refused.entry,
                refused.subject.as_str()
            ),
            ("write.not_enabled", Some(1), "PIEEPROM.UPD")
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
        assert!(check(1, &storage, &files, &sources).is_err());
        let eeprom = WriteEnables {
            eeprom: true,
            ..WriteEnables::default()
        };
        assert_eq!(check(1, &eeprom, &files, &sources), Ok(()));
        let without: Vec<_> = sources
            .iter()
            .copied()
            .filter(|(text, _)| *text != "PIEEPROM.UPD")
            .collect();
        assert_eq!(
            check(1, &WriteEnables::default(), &pinned(&without), &without),
            Ok(())
        );
    }

    #[test]
    fn a_refused_property_is_refused_even_when_writes_are_enabled() {
        let every = WriteEnables {
            firmware: true,
            eeprom: true,
            storage: true,
        };
        let sources: Vec<(&str, &[u8])> = [(
            "config.txt",
            &b"uart_2ndstage=1\r\n  PROGRAM_PUBKEY = 1\n"[..],
        )]
        .into_iter()
        .chain(VARIANTS)
        .collect();
        let refused = check(0, &every, &pinned(&sources), &sources).unwrap_err();
        assert_eq!(
            (refused.rule.as_str(), refused.subject.as_str()),
            ("write.refused_property", "config.txt")
        );
        assert!(
            refused
                .detail
                .contains("config.txt sets the refused property program_pubkey"),
            "{}",
            refused.detail
        );
        let sources: Vec<(&str, &[u8])> = [("extra.cfg", &b"# program_pubkey=1\n"[..])]
            .into_iter()
            .chain(VARIANTS)
            .collect();
        assert_eq!(check(0, &every, &pinned(&sources), &sources), Ok(()));
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
            (
                refused.rule.as_str(),
                refused.entry,
                refused.subject.as_str()
            ),
            (
                "payload.variant_missing",
                Some(3),
                "overlays/bcm2712d0.dtbo"
            )
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

    #[test]
    fn a_stepping_names_its_variant_in_any_ascii_case() {
        let board = pi5();
        let dtb = [name("bcm2712-rpi-5-b.dtb")];
        let refused = check_variants(0, &board, &identity("d0"), &dtb).unwrap_err();
        assert_eq!(
            refused.detail,
            "soc_stepping is d0, and the variant D0 needs overlays/bcm2712d0.dtbo"
        );
    }
}
