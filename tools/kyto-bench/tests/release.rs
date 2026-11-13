//! The `kyto-bench` binary on the example of `tools/examples/`, run as a
//! person runs it: its simulated devices replay the example console output
//! that the example instance configuration lists.

// These tests run the binary of the tool, the one program that a test of
// the tool starts.
#![allow(clippy::disallowed_types)]

use std::fs;
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
    let output = command.output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.ends_with(&format!("{}\n", out.join("receipt.json").display())));
    output.status.code()
}

fn receipt(out: &Path) -> Receipt {
    Receipt::parse(&fs::read(out.join("receipt.json")).unwrap()).unwrap()
}

#[test]
fn the_example_runs_to_pass_with_the_release_binary() {
    let (dir, examples) = (scratch("example"), examples());
    let mut nonces = Vec::new();
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
        nonces.push(receipt.nonce);
    }
    assert_ne!(nonces[0], nonces[1], "each receipt has its own nonce");
}

#[test]
fn release_binary_refuses_an_unenabled_eeprom_payload() {
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
