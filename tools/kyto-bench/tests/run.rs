//! Runs of run plans on simulated devices, each one call of the command line
//! that the binary calls, with the example run plan, payload files, instance
//! configuration and console output of `tools/examples/`. The simulated
//! board plays the example console output of the payload it is served, one
//! line per millisecond after the supply came on, and answers the step of
//! the echo entry 2 ms after it receives it. Every time is one of the bench
//! clock; nothing sleeps or reads the system clock.

use std::cell::RefCell;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use kyto_bench::cli;
use kyto_bench::device::{Delivery, Nanos, PowerState};
use kyto_bench::env::{self, Bench, Env};
use kyto_bench::sim::{Files, Responder, Script, ScriptProvider, SimTftp, SimWorld};
use kyto_bench_core::board::{Board, RASPBERRY_PI_5};
use kyto_bench_core::digest::Sha256;
use kyto_bench_core::instance::Instance;
use kyto_bench_core::names::{GenerationName, PayloadName};
use kyto_bench_core::plan::RunPlan;
use kyto_bench_core::power;
use kyto_bench_core::public::PublicReceipt;
use kyto_bench_core::receipt::{Command, Event, EventKind, Outcome, Receipt, Verdict};
use kyto_bench_core::rejudge::rejudge;

const STATS: &[u8] = b"STATS seq=1\r\n";
const MS: u64 = 1_000_000;

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples")
}

fn example(path: &str) -> Vec<u8> {
    fs::read(examples().join(path)).unwrap()
}

/// An empty scratch directory of the test `name`.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("run")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// The example console output that the board plays when it is served
/// `files`: that of the echo entry when its kernel is served, else that of
/// the firmware-only entry. The echo entry's STATS line is the board's
/// answer to its step. `cut` leaves out the last line of the power-on of
/// that number.
fn script(cut: Option<u32>) -> ScriptProvider {
    let (firmware, echo) = (
        example("captures/firmware-only.bin"),
        example("captures/echo.bin"),
    );
    let echo_kernel = example("payload/echo/kernel.img");
    let kernel = PayloadName::parse("kernel.img").unwrap();
    Box::new(move |power_on, files: &Files| {
        let capture = if files.get(&kernel) == Some(&echo_kernel) {
            &echo
        } else {
            &firmware
        };
        let mut lines: Vec<&[u8]> = capture.split_inclusive(|b| *b == b'\n').collect();
        lines.retain(|line| *line != STATS);
        if cut == Some(power_on) {
            lines.pop();
        }
        let chunks = lines.into_iter().zip(1..);
        Script {
            chunks: chunks
                .map(|(line, ms)| (Nanos::from_millis(ms), line.to_vec()))
                .collect(),
            responders: vec![Responder {
                trigger: b"?\r".to_vec(),
                delay: Nanos::from_millis(2),
                bytes: STATS.to_vec(),
            }],
        }
    })
}

/// Surroundings whose bench is a simulated device of each role on `world`.
struct TestEnv {
    world: SimWorld,
    script: RefCell<Option<ScriptProvider>>,
    boards: Vec<&'static str>,
}

impl TestEnv {
    fn new(script: ScriptProvider) -> Self {
        let (world, script) = (SimWorld::new(), RefCell::new(Some(script)));
        let boards = vec![RASPBERRY_PI_5, include_str!("data/synthetic-board.toml")];
        Self {
            world,
            script,
            boards,
        }
    }

    /// The board plays the example console output.
    fn example() -> Self {
        Self::new(script(None))
    }

    /// Whether the delivery device holds generation `number` with `file`.
    fn staged(&self, number: u32, file: &str) -> bool {
        let generation = GenerationName::new(number).unwrap();
        let mut tftp = SimTftp::new(&self.world, Vec::new());
        let name = PayloadName::parse(file).unwrap();
        tftp.read_staged(&generation, &name).is_ok()
    }
}

impl Env for TestEnv {
    fn open_bench(&self, _: &Instance, _: &Path, board: &Board) -> Result<Bench, String> {
        let script = self.script.borrow_mut().take().ok_or("the bench is open")?;
        Ok(env::simulated(&self.world, script, board))
    }

    fn boards(&self) -> &[&'static str] {
        &self.boards
    }
}

/// What a run left in its output directory.
struct Ran {
    status: u8,
    out: PathBuf,
}

impl Ran {
    fn receipt(&self) -> Receipt {
        let receipt = Receipt::parse(&fs::read(self.out.join("receipt.json")).unwrap()).unwrap();
        // Every receipt that the tool writes re-derives to itself.
        assert_eq!(rejudge(&receipt).unwrap(), receipt);
        receipt
    }

    fn public(&self) -> PublicReceipt {
        PublicReceipt::parse(&fs::read(self.out.join("receipt.public.json")).unwrap()).unwrap()
    }
}

/// One call of `kyto-bench run` with the plan, instance configuration and
/// payload root given, on `env`, with the output directory `out` in `dir`.
fn run_with(dir: &Path, plan: &Path, instance: &Path, payloads: &Path, env: &TestEnv) -> Ran {
    let out = dir.join("out");
    let args: [&Path; 4] = [plan, instance, payloads, &out];
    let names = ["--plan", "--instance", "--payload-root", "--out"];
    let mut line: Vec<OsString> = vec!["kyto-bench".into(), "run".into()];
    for (name, value) in names.into_iter().zip(args) {
        line.extend([name.into(), value.into()]);
    }
    let status = cli::run(line, env);
    Ran { status, out }
}

/// Runs `plan` with the example instance configuration and `payloads`.
fn run(dir: &Path, plan: &Path, payloads: &Path, env: &TestEnv) -> Ran {
    run_with(dir, plan, &examples().join("instance.toml"), payloads, env)
}

/// Runs the example plan on the example payloads.
fn run_example(name: &str, env: &TestEnv) -> Ran {
    let examples = examples();
    run(
        &scratch(name),
        &examples.join("run-plan.toml"),
        &examples.join("payload"),
        env,
    )
}

/// The example plan with `edit` made, written into `dir`.
fn plan_with(dir: &Path, edit: impl Fn(String) -> String) -> PathBuf {
    let text = String::from_utf8(example("run-plan.toml")).unwrap();
    let (path, edited) = (dir.join("plan.toml"), edit(text.clone()));
    assert_ne!(edited, text, "the edit changes the plan");
    fs::write(&path, edited).unwrap();
    path
}

/// A copy of the example payloads in `dir` with the file `PIEEPROM.UPD` in
/// that of the first entry, which a plan that `eeprom_plan` writes lists.
fn eeprom_payloads(dir: &Path) -> (PathBuf, Sha256) {
    fn copy(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for item in fs::read_dir(from).unwrap() {
            let path = item.unwrap().path();
            let target = to.join(path.file_name().unwrap());
            if path.is_dir() {
                copy(&path, &target);
            } else {
                fs::copy(&path, &target).unwrap();
            }
        }
    }
    let root = dir.join("payload");
    copy(&examples().join("payload"), &root);
    let image = b"A stand-in for an EEPROM image; it is not one.\n";
    fs::write(root.join("firmware-only/PIEEPROM.UPD"), image).unwrap();
    (root, Sha256::of(image))
}

/// The example plan with `PIEEPROM.UPD`, of the SHA-256 `digest`, in the
/// payload of its first entry, which enables the writes `writes`.
fn eeprom_plan(dir: &Path, digest: &Sha256, writes: &str) -> PathBuf {
    plan_with(dir, |text| {
        let file = format!("payload = [\n  {{ name = \"PIEEPROM.UPD\", sha256 = \"{digest}\" }},");
        let text = text.replacen("payload = [", &file, 1);
        text.replacen(
            "\n# One cold boot",
            &format!("{writes}\n# One cold boot"),
            1,
        )
    })
}

fn position(events: &[Event], kind: impl Fn(&EventKind) -> bool) -> Option<usize> {
    events.iter().position(|event| kind(&event.event))
}

fn power(command: Command) -> impl Fn(&EventKind) -> bool {
    move |kind| matches!(kind, EventKind::Power { command: sent, .. } if *sent == command)
}

/// The times of a cold boot's events, each as (time, bytes received
/// before): the console ready, the power on and its confirmation, the
/// first send, the end, the power off and its confirmation, the console
/// closed.
struct Times {
    ready: (u64, u64),
    on: (u64, u64),
    on_confirmed: (u64, u64),
    send: Option<(u64, u64)>,
    end: (u64, u64),
    off: (u64, u64),
    off_confirmed: (u64, u64),
    closed: (u64, u64),
}

fn times(receipt: &Receipt, events: &[Event]) -> Times {
    let confirmation = &receipt.inputs.instance.parameters.confirmation;
    let key = |index: usize| (events[index].at_ns, events[index].received);
    let (on, off) = (power(Command::On), power(Command::Off));
    let (on, off) = (
        position(events, on).unwrap(),
        position(events, off).unwrap(),
    );
    let confirmed = |index| {
        let at = power::confirmed(confirmation, events, index).unwrap();
        (at, events[index].received)
    };
    let at = |kind: fn(&EventKind) -> bool| position(events, kind).map(key);
    Times {
        ready: at(|kind| matches!(kind, EventKind::ConsoleReady {})).unwrap(),
        on: key(on),
        on_confirmed: confirmed(on),
        send: at(|kind| matches!(kind, EventKind::Send { .. })),
        end: at(|kind| matches!(kind, EventKind::End { .. })).unwrap(),
        off: key(off),
        off_confirmed: confirmed(off),
        closed: at(|kind| matches!(kind, EventKind::ConsoleClosed {})).unwrap(),
    }
}

fn occurrences(bytes: &[u8], value: &str) -> usize {
    bytes
        .windows(value.len())
        .filter(|window| *window == value.as_bytes())
        .count()
}

#[test]
fn a_two_entry_plan_runs_to_pass_on_simulated_devices() {
    let env = TestEnv::example();
    let ran = run_example("two-entries", &env);
    assert_eq!(ran.status, cli::PASS);
    let receipt = ran.receipt();
    let plan = RunPlan::parse(&receipt.inputs.run_plan.text).unwrap();
    assert_eq!(
        (receipt.refusal.as_ref(), receipt.verdict),
        (None, Some(Verdict::Pass))
    );
    let boots: Vec<usize> = receipt.entries.iter().map(|e| e.cold_boots.len()).collect();
    assert_eq!(boots, [3, 1]);
    // Two payloads, each checked from its source, as staged and as served.
    for (record, entry) in receipt.entries.iter().zip(&plan.entries) {
        assert_eq!(record.verdict, Verdict::Pass);
        for (file, planned) in record.payload.files.iter().zip(entry.files(&[]).unwrap()) {
            let digest = Some(planned.sha256.clone());
            assert_eq!(
                [
                    &file.source_sha256,
                    &file.staged_sha256,
                    &file.served_sha256
                ],
                [&digest; 3]
            );
        }
        let mut checks = record
            .checks
            .iter()
            .chain(record.cold_boots.iter().flat_map(|c| &c.checks));
        assert!(checks.clone().all(|check| check.result == Outcome::Pass));
        assert!(checks.any(|check| check.rule == "payload.served_digest"));
    }
    let [firmware, echo] = [&receipt.entries[0], &receipt.entries[1]];
    assert_ne!(
        firmware.payload.files[3].source_sha256,
        echo.payload.files[3].source_sha256
    );
    // Each cold boot keeps the order of the board, the off interval from
    // the last off confirmation, and is switched off and confirmed off.
    let confirmation = &receipt.inputs.instance.parameters.confirmation;
    let mut off_at = power::off_confirmed_at(confirmation, &receipt.start.events).unwrap();
    for record in &receipt.entries {
        let switch = position(&record.events, |k| matches!(k, EventKind::Switch { .. }));
        let switched = record.events[switch.unwrap()].at_ns;
        assert!(
            switched >= off_at,
            "the payload is served while the supply is off"
        );
        for cold_boot in &record.cold_boots {
            let t = times(&receipt, &cold_boot.events);
            assert!(switched <= t.on.0);
            assert!(
                t.on.0 - off_at >= 5000 * MS,
                "the off interval on the bench clock"
            );
            assert!(t.ready <= t.on && t.on < t.on_confirmed && t.end <= t.off);
            assert!(t.off < t.off_confirmed && t.off_confirmed <= t.closed);
            if let Some(send) = t.send {
                assert!(t.on_confirmed < send && send < t.end);
            }
            off_at = t.off_confirmed.0;
            // The console output, exactly as the board wrote it, ends with
            // its last line's LF.
            assert!(cold_boot.serial.bytes_base64.0.ends_with(b"\n"));
        }
    }
    let firmware_bytes = &firmware.cold_boots[0].serial.bytes_base64.0;
    assert_eq!(firmware_bytes, &example("captures/firmware-only.bin"));
    let echo_bytes = &echo.cold_boots[0].serial.bytes_base64.0;
    assert_eq!(echo_bytes, &example("captures/echo.bin"));
    assert!(
        firmware
            .cold_boots
            .iter()
            .all(|c| c.serial.bytes_base64.0 == *firmware_bytes)
    );
    // The board's serial number, its MAC address and the TFTP server
    // address appear once each, in the echo entry's cold boot.
    let values = ["1234abcd", "00:00:5e:00:53:01", "192.0.2.10"];
    for value in values {
        assert_eq!(occurrences(echo_bytes, value), 1, "{value}");
        assert_eq!(occurrences(firmware_bytes, value), 0, "{value}");
    }
    // The public version replaces each with placeholders, in exactly three
    // replacements, and withholds the nonce and the server address.
    let public = ran.public();
    let rules: Vec<Vec<&str>> = public
        .replacements
        .iter()
        .map(|r| r.rules.iter().map(String::as_str).collect())
        .collect();
    let address = vec!["board.network-link", "instance.tftp.server_address"];
    assert_eq!(
        rules,
        [
            vec!["board.board-serial"],
            vec!["board.network-link"],
            address
        ]
    );
    let stream = "/entries/1/cold_boots/0/serial/bytes_base64";
    assert!(public.replacements.iter().all(|r| r.field == stream));
    let withheld: Vec<&str> = public.withheld.iter().map(|w| w.rule.as_str()).collect();
    assert_eq!(withheld, ["nonce", "instance.tftp.server_address"]);
    let text = fs::read_to_string(ran.out.join("receipt.public.json")).unwrap();
    for value in values.iter().chain([&receipt.nonce.as_str()]) {
        assert!(!text.contains(value), "{value}");
    }
    // The supply was off at the start and after each cold boot.
    let commands: Vec<PowerState> = env.world.power_commands().iter().map(|c| c.1).collect();
    let on_off = [PowerState::On, PowerState::Off];
    assert_eq!(
        commands,
        [&[PowerState::Off][..], &on_off, &on_off, &on_off, &on_off].concat()
    );
}

#[test]
fn rejudging_a_written_receipt_gives_the_recorded_verdicts() {
    // `Ran::receipt` re-derives every receipt it reads; here a receipt of
    // each kind of result.
    let pass = run_example("rejudge-pass", &TestEnv::example());
    assert_eq!(pass.receipt().verdict, Some(Verdict::Pass));
    // The second cold boot of the first entry does not show its end marker.
    let fail = run_example("rejudge-fail", &TestEnv::new(script(Some(2))));
    assert_eq!(fail.status, cli::FAIL);
    let receipt = fail.receipt();
    let verdicts: Vec<Verdict> = receipt.entries.iter().map(|e| e.verdict).collect();
    assert_eq!(verdicts, [Verdict::Fail, Verdict::Pass]);
    let failed = &receipt.entries[0].cold_boots[1];
    assert_eq!(
        receipt.entries[0].cold_boots.len(),
        2,
        "a FAIL ends its entry"
    );
    let rules: Vec<&str> = failed
        .checks
        .iter()
        .filter(|c| c.result == Outcome::Fail)
        .map(|c| c.rule.as_str())
        .collect();
    assert_eq!(rules, ["boot.end", "marker.order"]);
    // Relay contacts that never close: the power on is not confirmed.
    let env = TestEnv::example();
    env.world.knobs().relay.stuck = true;
    let fault = run_example("rejudge-fault", &env);
    assert_eq!(fault.status, cli::BENCH_FAULT);
    let receipt = fault.receipt();
    let verdicts: Vec<Verdict> = receipt.entries.iter().map(|e| e.verdict).collect();
    assert_eq!(verdicts, [Verdict::BenchFault, Verdict::NotRun]);
    let commands: Vec<PowerState> = env.world.power_commands().iter().map(|c| c.1).collect();
    assert_eq!(commands, [PowerState::Off, PowerState::On, PowerState::Off]);
    // A run refused before any power command.
    let dir = scratch("rejudge-refused");
    let (payloads, digest) = eeprom_payloads(&dir);
    let refused = run(
        &dir,
        &eeprom_plan(&dir, &digest, ""),
        &payloads,
        &TestEnv::example(),
    );
    assert_eq!(refused.receipt().verdict, None);
}

#[test]
fn a_payload_that_writes_the_eeprom_is_refused_unless_the_plan_enables_it() {
    let dir = scratch("eeprom-refused");
    let (payloads, digest) = eeprom_payloads(&dir);
    let env = TestEnv::example();
    let ran = run(&dir, &eeprom_plan(&dir, &digest, ""), &payloads, &env);
    assert_eq!(ran.status, cli::REFUSED);
    let receipt = ran.receipt();
    let refusal = receipt.refusal.as_ref().unwrap();
    let found = (
        refusal.rule.as_str(),
        refusal.entry,
        refusal.subject.as_str(),
    );
    assert_eq!(found, ("write.not_enabled", Some(0), "PIEEPROM.UPD"));
    assert_eq!(receipt.verdict, None);
    assert!(receipt.start.events.is_empty());
    for entry in &receipt.entries {
        assert_eq!(entry.verdict, Verdict::NotRun);
        assert!(entry.events.is_empty() && entry.checks.is_empty() && entry.cold_boots.is_empty());
    }
    // Nothing was staged and no power command was sent.
    assert!(!env.staged(1, "config.txt"));
    assert!(env.world.power_commands().is_empty());
    assert!(ran.out.join("receipt.public.json").exists());
}

#[test]
fn an_enabled_eeprom_payload_is_staged() {
    let dir = scratch("eeprom-enabled");
    let (payloads, digest) = eeprom_payloads(&dir);
    let env = TestEnv::example();
    let plan = eeprom_plan(&dir, &digest, "\n[entries.writes]\neeprom = true\n");
    let ran = run(&dir, &plan, &payloads, &env);
    assert_eq!(ran.status, cli::PASS);
    assert!(env.staged(1, "PIEEPROM.UPD") && !env.staged(2, "PIEEPROM.UPD"));
}

#[test]
fn a_refused_property_is_refused_even_when_writes_are_enabled() {
    let dir = scratch("refused-property");
    let (payloads, _) = eeprom_payloads(&dir);
    let config = payloads.join("echo/config.txt");
    let text = format!("{}program_pubkey=1\n", fs::read_to_string(&config).unwrap());
    fs::write(&config, &text).unwrap();
    let digest = Sha256::of(text.as_bytes());
    let plan = plan_with(&dir, |plan| {
        let writes = "\n[entries.writes]\nfirmware = true\neeprom = true\nstorage = true\n";
        let old = "950cbaa591e9064fff306370a33222dcb8887049caf5ec76bce9ac81d3f29136";
        let at = plan.rfind(old).unwrap();
        format!("{}{digest}{}{writes}", &plan[..at], &plan[at + old.len()..])
    });
    let env = TestEnv::example();
    let ran = run(&dir, &plan, &payloads, &env);
    assert_eq!(ran.status, cli::REFUSED);
    let refusal = ran.receipt().refusal.unwrap();
    let found = (
        refusal.rule.as_str(),
        refusal.entry,
        refusal.subject.as_str(),
    );
    assert_eq!(found, ("write.refused_property", Some(1), "config.txt"));
    assert!(env.world.power_commands().is_empty());
}

#[test]
fn staging_is_checked_before_any_power_command() {
    let env = TestEnv::example();
    env.world.knobs().tftp.corrupts_staged = true;
    let ran = run_example("staged-copy", &env);
    assert_eq!(ran.status, cli::REFUSED);
    let receipt = ran.receipt();
    let refusal = receipt.refusal.as_ref().unwrap();
    let found = (
        refusal.rule.as_str(),
        refusal.entry,
        refusal.subject.as_str(),
    );
    assert_eq!(
        found,
        ("payload.staged_digest", Some(0), "bcm2712-rpi-5-b.dtb")
    );
    assert!(env.staged(1, "config.txt"), "the copy was staged");
    assert!(env.world.power_commands().is_empty());
    // A served copy that differs is a fault of the bench, after the switch.
    let env = TestEnv::example();
    env.world.knobs().tftp.corrupts_served = true;
    let ran = run_example("served-copy", &env);
    assert_eq!(ran.status, cli::BENCH_FAULT);
    let receipt = ran.receipt();
    assert!(receipt.entries[0].cold_boots.is_empty());
    let faults = receipt.entries[0]
        .checks
        .iter()
        .filter(|c| c.result == Outcome::Fault);
    assert!(
        faults.clone().count() == 4 && faults.clone().all(|c| c.rule == "payload.served_digest")
    );
    assert_eq!(
        env.world.power_commands().len(),
        1,
        "the power off at the start only"
    );
}

#[test]
fn each_cold_boot_waits_the_off_interval_on_the_bench_clock() {
    let receipt = run_example("off-interval", &TestEnv::example()).receipt();
    let confirmation = &receipt.inputs.instance.parameters.confirmation;
    let mut off_at = power::off_confirmed_at(confirmation, &receipt.start.events).unwrap();
    for cold_boot in receipt.entries.iter().flat_map(|entry| &entry.cold_boots) {
        let t = times(&receipt, &cold_boot.events);
        assert_eq!(
            t.ready.0,
            off_at + 5000 * MS,
            "the console opens once the interval passed"
        );
        assert_eq!(t.on.0, t.ready.0);
        off_at = t.off_confirmed.0;
    }
}

#[test]
fn capture_is_ready_before_power_on() {
    let receipt = run_example("ready", &TestEnv::example()).receipt();
    for cold_boot in receipt.entries.iter().flat_map(|entry| &entry.cold_boots) {
        let events = &cold_boot.events;
        let ready = position(events, |k| matches!(k, EventKind::ConsoleReady {})).unwrap();
        assert!(ready < position(events, power(Command::On)).unwrap());
        assert_eq!(cold_boot.serial.index[0][0], 0);
        assert!(cold_boot.serial.index[0][1] >= events[ready].at_ns);
    }
}

#[test]
fn the_switch_happens_only_while_off_is_confirmed() {
    let receipt = run_example("switch", &TestEnv::example()).receipt();
    let confirmation = &receipt.inputs.instance.parameters.confirmation;
    let mut before: Vec<&Event> = receipt.start.events.iter().collect();
    for entry in &receipt.entries {
        let switch = position(&entry.events, |k| matches!(k, EventKind::Switch { .. })).unwrap();
        let events: Vec<Event> = before.iter().map(|event| (*event).clone()).collect();
        let off = power::off_confirmed_at(confirmation, &events).expect("the supply is off");
        assert!(entry.events[switch].at_ns >= off);
        before = entry.cold_boots.iter().flat_map(|c| &c.events).collect();
        let first_on = position(&entry.cold_boots[0].events, power(Command::On)).unwrap();
        assert!(entry.events[switch].at_ns <= entry.cold_boots[0].events[first_on].at_ns);
    }
}

#[test]
fn devices_and_local_settings_enter_the_receipt_only_as_the_hardware_revision() {
    let ran = run_example("local", &TestEnv::example());
    let text = fs::read_to_string(ran.out.join("receipt.json")).unwrap();
    let instance = Instance::parse(&String::from_utf8(example("instance.toml")).unwrap()).unwrap();
    let receipt = ran.receipt();
    assert_eq!(
        &receipt.bench.hardware_revision.value,
        instance.hardware_revision()
    );
    let local = instance.deidentify_values().into_iter();
    let local: Vec<String> = local
        .filter(|v| v.rule.starts_with("instance.local."))
        .map(|v| v.value)
        .collect();
    assert_eq!(local.len(), 5, "the salt and four replayed files");
    for value in local {
        assert!(!text.contains(&value), "{value}");
    }
    let relays = receipt
        .entries
        .iter()
        .flat_map(|e| &e.cold_boots)
        .flat_map(|c| &c.events);
    for event in relays.chain(&receipt.start.events) {
        if let EventKind::Power { relay, .. } = &event.event {
            assert_eq!(relay, &None, "a simulated power device names no relay");
        }
    }
}

#[test]
fn a_public_version_that_would_show_a_value_is_not_written() {
    // A segment of the echo entry over the lines with the serial number,
    // the MAC address and the server address.
    let dir = scratch("public-refused");
    let plan = plan_with(&dir, |plan| {
        plan.replace("from = \"board.os_start\"", "from = \"board.bootloader\"")
    });
    let ran = run(
        &dir,
        &plan,
        &examples().join("payload"),
        &TestEnv::example(),
    );
    assert_eq!(ran.status, cli::PUBLIC_REFUSED);
    assert_eq!(ran.receipt().verdict, Some(Verdict::Fail));
    assert!(!ran.out.join("receipt.public.json").exists());
}

#[test]
fn inputs_that_cannot_be_used_are_refused_before_anything_is_written() {
    let dir = scratch("input-errors");
    let payloads = examples().join("payload");
    // A plan that holds a value to hide of the instance.
    let plan = plan_with(&dir, |plan| format!("# 192.0.2.10\n{plan}"));
    let env = TestEnv::example();
    let ran = run(&dir, &plan, &payloads, &env);
    assert_eq!(ran.status, cli::INPUT_ERROR);
    assert!(!ran.out.exists());
    // An instance configuration that is not one.
    let ran = run_with(
        &dir,
        &examples().join("run-plan.toml"),
        &plan,
        &payloads,
        &env,
    );
    assert_eq!(ran.status, cli::INPUT_ERROR);
    // An output directory that exists.
    fs::create_dir(dir.join("out")).unwrap();
    let ran = run(&dir, &examples().join("run-plan.toml"), &payloads, &env);
    assert_eq!(ran.status, cli::INPUT_ERROR);
    assert_eq!(fs::read_dir(&ran.out).unwrap().count(), 0);
    assert!(env.world.power_commands().is_empty());
}

#[test]
fn a_plan_is_digested_and_recorded_as_the_bytes_read() {
    // Line endings and a byte order mark stay as written.
    let dir = scratch("plan-bytes");
    let plan = plan_with(&dir, |plan| {
        format!("\u{feff}{}", plan.replace('\n', "\r\n"))
    });
    let ran = run(
        &dir,
        &plan,
        &examples().join("payload"),
        &TestEnv::example(),
    );
    assert_eq!(ran.status, cli::PASS);
    let receipt = ran.receipt();
    let bytes = fs::read(&plan).unwrap();
    assert_eq!(receipt.inputs.run_plan.text.as_bytes(), bytes);
    assert_eq!(receipt.inputs.run_plan.sha256, Sha256::of(&bytes));
    let instance = &receipt.inputs.instance.sha256;
    assert_eq!(instance, &Sha256::of(&example("instance.toml")));
}

#[test]
fn a_synthetic_board_runs_through_the_same_flow() {
    // The synthetic board waits 2 s before it publishes, then opens the
    // console and powers on.
    let dir = scratch("synthetic");
    fs::create_dir_all(dir.join("payload/s")).unwrap();
    fs::write(dir.join("payload/s/board-b.dtb"), b"b").unwrap();
    let plan = format!(
        "format = \"kyto-bench run plan\"\nformat_version = 1\nboard = \"synthetic-board\"\n\n\
         [[entries]]\nname = \"s\"\npayload_dir = \"s\"\n\
         payload = [{{ name = \"board-b.dtb\", sha256 = \"{}\" }}]\ncold_boots = 2\n\
         boot_timeout_ms = 1000\nend = {{ marker = \"board.handoff\" }}\nsends = false\n\
         expect = [{{ marker = \"board.net\", count = 1 }}]\n",
        Sha256::of(b"b")
    );
    fs::write(dir.join("plan.toml"), plan).unwrap();
    let instance = String::from_utf8(example("instance.toml")).unwrap();
    let start = instance.find("[identity]").unwrap();
    let end = instance.find("[devices]").unwrap();
    let identity = "[identity]\nformat = \"kyto-bench board identity\"\nformat_version = 1\n\
        board = \"synthetic-board\"\nrevision_code = { value = \"unknown\", read = \"firmware_log\" }\n\
        pcb_version = { value = \"unknown\", read = \"visual\" }\n\
        soc_stepping = { value = \"B\", read = \"visual\" }\n\
        memory.vendor = { value = \"unknown\", read = \"visual\" }\n\
        memory.part = { value = \"unknown\", read = \"visual\" }\n\
        ethernet_phy = { value = \"unknown\", read = \"visual\" }\n\
        spi_flash.jedec_id = { value = \"unknown\", read = \"firmware_log\" }\n\
        spi_flash.part = { value = \"unknown\", read = \"visual\" }\n\
        eeprom_version = { value = \"unknown\", read = \"firmware_version\" }\n\
        manufacturing_version = { value = \"unknown\", read = \"software\" }\n\
        min_boot_version = { value = \"unknown\", read = \"software\" }\n\
        boot_files = { value = \"unknown\", read = \"tftp_reads\" }\n\n";
    let instance = [&instance[..start], identity, &instance[end..]].concat();
    let instance = instance.replace("board = \"raspberry-pi-5\"", "board = \"synthetic-board\"");
    fs::write(dir.join("instance.toml"), instance).unwrap();
    let lines = [
        "syn rom v3\n",
        "syn boot via net code=7\n",
        "syn got board-b.dtb size 1\n",
        "syn handoff\n",
    ];
    let env = TestEnv::new(Box::new(move |_, _| Script {
        chunks: lines
            .iter()
            .zip(1..)
            .map(|(l, ms)| (Nanos::from_millis(ms), l.as_bytes().to_vec()))
            .collect(),
        responders: Vec::new(),
    }));
    let ran = run_with(
        &dir,
        &dir.join("plan.toml"),
        &dir.join("instance.toml"),
        &dir.join("payload"),
        &env,
    );
    assert_eq!(ran.status, cli::PASS);
    let receipt = ran.receipt();
    assert_eq!(receipt.inputs.board_description.id, "synthetic-board");
    let entry = &receipt.entries[0];
    let start_off = power::off_confirmed_at(
        &receipt.inputs.instance.parameters.confirmation,
        &receipt.start.events,
    )
    .unwrap();
    assert_eq!(
        entry.events[0].at_ns,
        start_off + 2000 * MS,
        "the switch follows the wait"
    );
    assert_eq!(entry.cold_boots.len(), 2);
}

/// The board's behaviour is data: the flow, its records and the receipt it
/// assembles name nothing of the Raspberry Pi 5.
#[test]
fn the_flow_names_no_board() {
    let literals = [
        "NETWORK",
        "RPi:",
        "BOOTMODE",
        "TFTP_GET",
        "bcm2712",
        "pieeprom",
        "config.txt",
    ];
    for (file, text) in [
        ("run.rs", include_str!("../src/run.rs")),
        ("record.rs", include_str!("../src/record.rs")),
        ("assemble.rs", include_str!("../src/assemble.rs")),
        ("inputs.rs", include_str!("../src/inputs.rs")),
    ] {
        let code = &text[..text.find("#[cfg(test)]").unwrap_or(text.len())];
        for literal in literals {
            assert!(!code.contains(literal), "{file} names {literal}");
        }
    }
}
