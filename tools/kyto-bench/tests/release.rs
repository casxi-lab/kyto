//! The `kyto-bench` binary on the example of `tools/examples/`, run as a
//! person runs it: its simulated devices replay the example console output
//! that the example instance configuration lists. The binary is the one that
//! Cargo builds for these tests, of the profile that they are built with:
//! the test profile under `cargo test`, the release profile only under
//! `cargo test --release`.

// These tests run the binary of the tool, the one program that they start.
#![allow(clippy::disallowed_types)]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use kyto_bench_core::digest::Sha256;
use kyto_bench_core::public::PublicReceipt;
use kyto_bench_core::receipt::{Receipt, Verdict};

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples")
}

/// An empty scratch directory of the test `name`.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("release")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs `kyto-bench run` with the plan, the example instance configuration
/// and the payload root given, into `out`; its exit status.
fn kyto_bench_run(plan: &Path, payloads: &Path, out: &Path) -> Option<i32> {
    let output = kyto_bench(plan, payloads, out).output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.ends_with(&format!("{}\n", out.join("receipt.json").display())));
    output.status.code()
}

/// The command line of `kyto-bench run` with the plan, the example instance
/// configuration and the payload root given, into `out`.
fn kyto_bench(plan: &Path, payloads: &Path, out: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kyto-bench"));
    command.arg("run").arg("--plan").arg(plan);
    command
        .arg("--instance")
        .arg(examples().join("instance.toml"));
    command
        .arg("--payload-root")
        .arg(payloads)
        .arg("--out")
        .arg(out);
    command
}

fn receipt(out: &Path) -> Receipt {
    Receipt::parse(&fs::read(out.join("receipt.json")).unwrap()).unwrap()
}

#[test]
fn the_example_runs_to_pass_with_the_built_binary() {
    let (dir, examples) = (scratch("example"), examples());
    let mut receipts = Vec::new();
    for run in ["first", "second"] {
        let out = dir.join(run);
        let plan = examples.join("run-plan.toml");
        assert_eq!(
            kyto_bench_run(&plan, &examples.join("payload"), &out),
            Some(0)
        );
        let receipt = receipt(&out);
        assert_eq!(receipt.verdict, Some(Verdict::Pass));
        // A run on simulated devices is never evidence.
        assert!(!receipt.evidence.eligible);
        let rules: Vec<&str> = receipt
            .evidence
            .reasons
            .iter()
            .map(|r| r.rule.as_str())
            .collect();
        assert!(rules.contains(&"evidence.simulated"));
        let public = fs::read(out.join("receipt.public.json")).unwrap();
        assert_eq!(PublicReceipt::parse(&public).unwrap().replacements.len(), 3);
        receipts.push(receipt);
    }
    let [first, mut second] = <[Receipt; 2]>::try_from(receipts).unwrap();
    assert_ne!(first.nonce, second.nonce, "each receipt has its own nonce");
    // Apart from its nonce, a run on simulated devices gives the same
    // receipt every time, its times included.
    second.nonce.clone_from(&first.nonce);
    assert_eq!(first, second);
}

/// The exit status and standard output of the built binary with `args`.
fn output_of(args: &[&std::ffi::OsStr]) -> (Option<i32>, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_kyto-bench"))
        .args(args)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let stdout = String::from_utf8(output.stdout).unwrap();
    (output.status.code(), stdout)
}

#[test]
fn the_built_binary_says_what_power_off_and_publish_did() {
    let examples = examples();
    let (plan, instance) = (
        examples.join("run-plan.toml"),
        examples.join("instance.toml"),
    );
    let power_off = output_of(&[
        "power-off".as_ref(),
        "--instance".as_ref(),
        instance.as_ref(),
    ]);
    let off = "OFF: readings confirmed it; the last read 0 µV\n";
    assert_eq!(power_off, (Some(0), off.into()));
    let payloads = examples.join("payload");
    let publish = output_of(&[
        "publish".as_ref(),
        "--plan".as_ref(),
        plan.as_ref(),
        "--instance".as_ref(),
        instance.as_ref(),
        "--payload-root".as_ref(),
        payloads.as_ref(),
        "--entry".as_ref(),
        "echo".as_ref(),
    ]);
    assert_eq!(publish, (Some(0), "PUBLISHED: echo as g000001\n".into()));
}

#[test]
fn the_built_binary_refuses_an_unenabled_eeprom_payload() {
    let dir = scratch("eeprom");
    let payloads = dir.join("payload");
    for entry in ["firmware-only", "echo"] {
        for file in [
            "config.txt",
            "bcm2712-rpi-5-b.dtb",
            "overlays/bcm2712d0.dtbo",
            "kernel.img",
        ] {
            let to = payloads.join(entry).join(file);
            fs::create_dir_all(to.parent().unwrap()).unwrap();
            fs::copy(examples().join("payload").join(entry).join(file), to).unwrap();
        }
    }
    let image = b"A stand-in for an EEPROM image; it is not one.\n";
    fs::write(payloads.join("firmware-only/pieeprom.upd"), image).unwrap();
    let plan = fs::read_to_string(examples().join("run-plan.toml")).unwrap();
    let file = format!(
        "payload = [\n  {{ name = \"pieeprom.upd\", sha256 = \"{}\" }},",
        Sha256::of(image)
    );
    fs::write(
        dir.join("plan.toml"),
        plan.replacen("payload = [", &file, 1),
    )
    .unwrap();
    let out = dir.join("out");
    assert_eq!(
        kyto_bench_run(&dir.join("plan.toml"), &payloads, &out),
        Some(4)
    );
    let receipt = receipt(&out);
    let refusal = receipt.refusal.unwrap();
    let found = (
        refusal.rule.as_str(),
        refusal.entry,
        refusal.subject.as_str(),
    );
    assert_eq!(found, ("write.not_enabled", Some(0), "pieeprom.upd"));
    assert_eq!(receipt.verdict, None);
    assert!(receipt.start.events.is_empty(), "no power command");
}

#[test]
fn output_that_nothing_reads_changes_no_exit_status() {
    // Standard output and standard error are pipes whose reading end is
    // closed, so every message and result fails to be written.
    let closed = || {
        let (reader, writer) = io::pipe().unwrap();
        drop(reader);
        writer
    };
    // A run that passes writes its result on standard output, and a plan
    // that cannot be read gives an error on standard error.
    let (dir, examples) = (scratch("closed-output"), examples());
    let plans = [examples.join("run-plan.toml"), dir.join("absent.toml")];
    for (plan, status) in plans.iter().zip([0, 2]) {
        let out = dir.join(status.to_string());
        let mut command = kyto_bench(plan, &examples.join("payload"), &out);
        let ran = command.stdout(closed()).stderr(closed()).status().unwrap();
        assert_eq!(ran.code(), Some(status), "{}", plan.display());
        assert_eq!(out.join("receipt.json").exists(), status == 0);
    }
}

#[test]
fn the_built_binary_says_an_input_error_on_standard_error() {
    // A plan that cannot be read: an input error, said on standard error as
    // one line after the name of the tool, and nothing on standard output.
    let dir = scratch("input-error");
    let (plan, out) = (dir.join("absent.toml"), dir.join("out"));
    let output = kyto_bench(&plan, &examples().join("payload"), &out)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let error = fs::read(&plan).unwrap_err();
    let said = format!("kyto-bench: cannot read {}: {error}\n", plan.display());
    assert_eq!(String::from_utf8(output.stderr).unwrap(), said);
    assert!(!out.exists(), "no output directory");
}
