//! The bench self-test plan of `tools/self-test/` on the host. It passes
//! the checks of a run plan against an instance configuration that pins
//! the set `self-test`, names only markers of the board description and
//! lines about or from its own payload, and enables no write; and a run of
//! it through the command line, on the simulated devices of
//! `tools/examples/self-test-instance.toml`, which replay the example
//! console output of the self-test payload, gives PASS, with one cold boot
//! of each entry and the answer to its one send.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use kyto_bench::cli;
use kyto_bench::env::HostEnv;
use kyto_bench_core::board::{Board, RASPBERRY_PI_5};
use kyto_bench_core::instance::Instance;
use kyto_bench_core::plan::{End, Entry, RunPlan};
use kyto_bench_core::receipt::{Event, EventKind, Outcome, Receipt, Verdict};
use kyto_bench_core::rejudge::rejudge;
use kyto_bench_core::write::WriteEnables;

/// The size of the official `bcm2712-rpi-5-b.dtb` that a bench pins, which
/// the README of `tools/self-test/` names.
const DTB_BYTES: usize = 78770;

fn tools() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read(path: &str) -> Vec<u8> {
    fs::read(tools().join(path)).unwrap()
}

fn plan() -> RunPlan {
    RunPlan::parse(&String::from_utf8(read("self-test/run-plan.toml")).unwrap()).unwrap()
}

fn instance() -> Instance {
    let text = String::from_utf8(read("examples/self-test-instance.toml")).unwrap();
    Instance::parse(&text).unwrap()
}

/// Every marker that `entry` names, by the keys that name markers.
fn named(entry: &Entry) -> Vec<&str> {
    let mut names: Vec<&str> = entry.expect.iter().map(|e| e.marker.as_str()).collect();
    if let End::Marker(end) = &entry.end {
        names.push(&end.marker);
    }
    names.extend(
        entry
            .order
            .iter()
            .flat_map(|o| o.markers.iter().map(String::as_str)),
    );
    for forbid in &entry.forbid {
        names.extend(forbid.from.iter().chain(&forbid.to).map(String::as_str));
    }
    names.extend(entry.before_first_send.iter().map(String::as_str));
    names.extend(entry.steps().iter().map(|step| step.after.as_str()));
    for segment in &entry.segments {
        names.extend([segment.from.as_str(), segment.to.as_str()]);
    }
    names
}

#[test]
fn the_self_test_plan_passes_the_plan_checks_against_a_pinned_set() {
    let board = Board::parse(RASPBERRY_PI_5).unwrap();
    let instance = instance();
    instance.check_board(&board).unwrap();
    let plan = plan();
    plan.check(&board, &instance.parameters().pins).unwrap();
    // Without the pinned set, the plan is refused.
    assert!(plan.check(&board, &[]).is_err());
    let text = plan.text().to_ascii_lowercase();
    for value in instance.deidentify_values() {
        let value = value.value.to_ascii_lowercase();
        assert!(!text.contains(&value), "the plan holds {value}");
    }

    let names: Vec<&str> = plan.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["firmware", "echo"]);
    for entry in &plan.entries {
        assert_eq!(entry.payload_pin.as_deref(), Some("self-test"));
        assert_eq!(entry.cold_boots, 1);
        assert_eq!(entry.writes, WriteEnables::default(), "no write is enabled");
        assert!(entry.firmware.is_empty() && entry.values.is_empty());
        for name in named(entry) {
            let own = entry.markers.iter().any(|marker| marker.name == name);
            assert!(name.starts_with("board.") || own, "{}: {name}", entry.name);
        }
    }

    // The firmware entry's own markers are the board's lines of the TFTP
    // reads of config.txt, the kernel and the device tree, each of its size.
    let [firmware, echo] = [&plan.entries[0], &plan.entries[1]];
    assert_eq!(firmware.sends, Some(false));
    let end = |entry: &Entry| match &entry.end {
        End::Marker(end) => Some(end.marker.clone()),
        End::Timeout(_) => None,
    };
    assert_eq!(end(firmware).as_deref(), Some("board.os_start"));
    assert_eq!(end(echo).as_deref(), Some("ready"));
    let range = |entry: &Entry, marker: &str| {
        let expect = entry.expect.iter().find(|e| e.marker == marker);
        expect.map(|e| e.range())
    };
    assert_eq!(range(firmware, "board.network_boot"), Some((1, None)));
    for marker in ["sd_boot", "recovery", "eeprom_write", "eeprom_updated"] {
        let marker = format!("board.{marker}");
        assert_eq!(range(firmware, &marker), Some((0, Some(0))), "{marker}");
    }
    let order = ["board.bootloader", "board.network_boot", "board.os_start"];
    assert_eq!(firmware.order[0].markers, order);
    assert_eq!(echo.before_first_send, ["board.os_start", "ready"]);
    let reads = [
        ("config.txt", read("self-test/config.txt").len()),
        ("kernel_2712.img", read("self-test/kernel_2712.img").len()),
        ("bcm2712-rpi-5-b.dtb", DTB_BYTES),
    ];
    let line =
        |file: &str, bytes: usize| format!(" 16.03 Read bench//{file} bytes {bytes:8} hnd 0x0");
    assert_eq!(firmware.markers.len(), reads.len());
    for (marker, (file, bytes)) in firmware.markers.iter().zip(reads) {
        let read = line(file, bytes);
        assert!(board.tftp.read.pattern.is_match(read.as_bytes()), "{read}");
        assert!(
            marker.line.is_match(read.as_bytes()),
            "{}: {read}",
            marker.name
        );
        assert!(!marker.line.is_match(line(file, bytes + 1).as_bytes()));
        let others = reads.iter().filter(|(other, _)| *other != file);
        for (other, size) in others {
            assert!(!marker.line.is_match(line(other, *size).as_bytes()));
        }
        let expected = firmware.expect.iter().find(|e| e.marker == marker.name);
        assert_eq!(expected.map(|e| e.range()), Some((1, None)));
    }

    // The echo entry's own marker is the ready line of the kernel, and its
    // step expects the answer to the line it sends, a line that the kernel
    // writes only in answer and never as a whole of its own.
    let kernel = read("self-test/kernel_2712.img");
    let has = |bytes: &[u8]| kernel.windows(bytes.len()).any(|window| window == bytes);
    let ready = "kyto self-test: ready";
    assert!(has(format!("\r\n{ready}\r\n").as_bytes()));
    assert_eq!(echo.markers.len(), 1);
    assert!(echo.markers[0].line.is_match(ready.as_bytes()));
    assert_eq!(range(echo, "ready"), Some((1, Some(1))));
    let [step] = echo.steps() else {
        panic!("the echo entry has one step");
    };
    let sent = step.send.strip_suffix('\r').unwrap();
    let answer = format!("kyto self-test: echo {sent} ({} bytes)", sent.len());
    assert!(step.expect.is_match(answer.as_bytes()), "{answer}");
    for other in [ready, sent, "kyto self-test: echo hello bench (12 bytes)"] {
        assert!(!step.expect.is_match(other.as_bytes()), "{other}");
    }
    assert!(has(b"kyto self-test: echo \0") && has(b" bytes)\r\n\0"));
    assert!(!has(answer.as_bytes()));
}

/// The sends of a cold boot's events, with their bytes.
fn sends(events: &[Event]) -> Vec<&[u8]> {
    let send = |event| match event {
        &Event {
            event: EventKind::Send {
                ref bytes_base64, ..
            },
            ..
        } => Some(&bytes_base64.0[..]),
        _ => None,
    };
    events.iter().filter_map(send).collect()
}

#[test]
fn the_self_test_plan_passes_on_simulated_devices() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("self-test");
    let _ = fs::remove_dir_all(&dir);
    // The payload root: the files of tools/self-test and the device tree
    // stand-ins that the example instance configuration pins.
    let payload = dir.join("payload");
    fs::create_dir_all(payload.join("self-test/overlays")).unwrap();
    for (from, to) in [
        ("self-test/config.txt", "config.txt"),
        ("self-test/kernel_2712.img", "kernel_2712.img"),
        (
            "examples/payload/echo/bcm2712-rpi-5-b.dtb",
            "bcm2712-rpi-5-b.dtb",
        ),
        (
            "examples/payload/echo/overlays/bcm2712d0.dtbo",
            "overlays/bcm2712d0.dtbo",
        ),
    ] {
        fs::copy(tools().join(from), payload.join("self-test").join(to)).unwrap();
    }
    let out = dir.join("out");
    let args: [OsString; 10] = [
        "kyto-bench".into(),
        "run".into(),
        "--plan".into(),
        tools().join("self-test/run-plan.toml").into(),
        "--instance".into(),
        tools().join("examples/self-test-instance.toml").into(),
        "--payload-root".into(),
        payload.into(),
        "--out".into(),
        out.clone().into(),
    ];
    assert_eq!(cli::run(args, &HostEnv), cli::PASS);
    assert!(out.join("receipt.public.json").is_file());
    let receipt = Receipt::parse(&fs::read(out.join("receipt.json")).unwrap()).unwrap();
    assert_eq!(rejudge(&receipt).unwrap(), receipt);
    assert_eq!(receipt.verdict, Some(Verdict::Pass));

    let entries: Vec<(&str, usize)> = receipt
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.cold_boots.len()))
        .collect();
    assert_eq!(entries, [("firmware", 1), ("echo", 1)]);
    for entry in &receipt.entries {
        assert_eq!(entry.payload.pin.as_deref(), Some("self-test"));
        let checks = entry.checks.iter();
        let mut checks = checks.chain(entry.cold_boots.iter().flat_map(|boot| &boot.checks));
        assert!(checks.all(|check| check.result == Outcome::Pass));
    }
    assert!(sends(&receipt.entries[0].cold_boots[0].events).is_empty());
    let echo = &receipt.entries[1].cold_boots[0];
    assert_eq!(sends(&echo.events), [&b"hello bench\r"[..]]);
    let bytes = &echo.serial.bytes_base64.0;
    let at = echo.observations.steps[0].response_line.unwrap() as usize;
    let line = bytes[at..].split(|byte| *byte == b'\r').next().unwrap();
    assert_eq!(line, b"kyto self-test: echo hello bench (11 bytes)");
}
