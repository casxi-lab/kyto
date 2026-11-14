//! Runs of run plans on simulated devices, each one call of the command line
//! that the binary calls, with the example run plan, payload files, instance
//! configuration and console output of `tools/examples/`. The simulated
//! board plays the example console output of the payload it is served, one
//! line per millisecond after the supply came on, and answers the step of
//! the echo entry 2 ms after it receives it. Every time is one of the bench
//! clock; nothing sleeps or reads the system clock.

use std::cell::RefCell;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicI32, Ordering};

use serde::Deserialize;

use kyto_bench::device::{
    Clock, Console, Delivery, DeviceError, Meter, MeterConfirm, Nanos, PowerState, PowerSwitch,
    Quantity, Received, Role,
};
use kyto_bench::drivers::hid::{HidOpen, HidapiLink, OpenError};
use kyto_bench::drivers::ina228::Ina228Meter;
use kyto_bench::env::{self, Bench, Env, Interrupt};
use kyto_bench::provenance::ToolRevision;
use kyto_bench::sim::{
    Files, ON_MICRO, Responder, Script, ScriptProvider, SimRelay, SimSupplyMeter, SimTftp,
    SimWorld, replay,
};
use kyto_bench::{cli, output};
use kyto_bench_core::board::{Board, RASPBERRY_PI_5};
use kyto_bench_core::digest::Sha256;
use kyto_bench_core::instance::Instance;
use kyto_bench_core::names::{GenerationName, PayloadName};
use kyto_bench_core::plan::RunPlan;
use kyto_bench_core::power;
use kyto_bench_core::public::PublicReceipt;
use kyto_bench_core::receipt::{
    Base64, Check, ColdBootRecord, Command, EndReason, Event, EventKind, Outcome, Receipt, Verdict,
};
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
/// the firmware-only entry. The echo entry's STATS line is left out: the
/// board sends `answer` in its place as the answer to the step. `cut` leaves
/// out the last line of the power-on of that number.
fn script(cut: Option<u32>, answer: &'static [u8]) -> ScriptProvider {
    played(cut, answer, STATS)
}

/// [`script`], with the lines `left_out` left out in place of the STATS
/// line.
fn played(cut: Option<u32>, answer: &'static [u8], left_out: &'static [u8]) -> ScriptProvider {
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
        lines.retain(|line| *line != left_out);
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
                bytes: answer.to_vec(),
            }],
        }
    })
}

/// A console that never reports its deadline: it gives an empty chunk
/// instead, as a serial port can after a read that timed out. It counts the
/// calls of `receive`, so that a run that keeps reading stops.
struct Deaf(Box<dyn Console>, u32);

impl Console for Deaf {
    fn open(&mut self, deadline: Nanos) -> Result<Nanos, DeviceError> {
        self.0.open(deadline)
    }

    fn receive(&mut self, deadline: Nanos) -> Result<Received, DeviceError> {
        self.1 += 1;
        assert!(self.1 < 1000, "the run reads on past its deadline");
        Ok(match self.0.receive(deadline)? {
            Received::Deadline { at } => Received::Bytes {
                at,
                bytes: Vec::new(),
            },
            received => received,
        })
    }

    fn send(&mut self, bytes: &[u8]) -> Result<Nanos, DeviceError> {
        self.0.send(bytes)
    }

    fn close(&mut self) -> Result<(), DeviceError> {
        self.0.close()
    }
}

/// A console that reports its deadline early: it gives what arrives within
/// 100 ms of each read, or reports a deadline then.
struct Early(Box<dyn Console>, Rc<dyn Clock>);

impl Console for Early {
    fn open(&mut self, deadline: Nanos) -> Result<Nanos, DeviceError> {
        self.0.open(deadline)
    }

    fn receive(&mut self, deadline: Nanos) -> Result<Received, DeviceError> {
        let soon = self.1.now().saturating_add(Nanos::from_millis(100));
        self.0.receive(deadline.min(soon))
    }

    fn send(&mut self, bytes: &[u8]) -> Result<Nanos, DeviceError> {
        self.0.send(bytes)
    }

    fn close(&mut self) -> Result<(), DeviceError> {
        self.0.close()
    }
}

/// A console that panics when it is first read, once the supply is on, as
/// a run that stops on an error of the tool does.
struct Panicking(Box<dyn Console>);

impl Console for Panicking {
    fn open(&mut self, deadline: Nanos) -> Result<Nanos, DeviceError> {
        self.0.open(deadline)
    }

    fn receive(&mut self, _: Nanos) -> Result<Received, DeviceError> {
        panic!("an error of the tool while it reads the console");
    }

    fn send(&mut self, bytes: &[u8]) -> Result<Nanos, DeviceError> {
        self.0.send(bytes)
    }

    fn close(&mut self) -> Result<(), DeviceError> {
        self.0.close()
    }
}

/// How long after a chunk's stamp a [`Lagging`] console lets the run read
/// it.
const LAG: Nanos = Nanos(1_500_000);

/// A console that the run reads late. After it hands a chunk or takes a
/// send, it takes the next chunk that the board writes within 2 ms and
/// moves the bench clock [`LAG`] past that chunk's stamp, as a console that
/// stamps chunks when they arrive holds what came while the run was busy.
/// It hands a chunk that it holds by a deadline only when the chunk was
/// stamped by then, unless it is `careless`.
struct Lagging {
    console: Box<dyn Console>,
    clock: Rc<dyn Clock>,
    held: Option<(Nanos, Vec<u8>)>,
    careless: bool,
}

impl Lagging {
    fn take_next(&mut self, after: Nanos) -> Result<(), DeviceError> {
        let by = after.saturating_add(Nanos::from_millis(2));
        if let Received::Bytes { at, bytes } = self.console.receive(by)? {
            self.clock.wait_until(at.saturating_add(LAG));
            self.held = Some((at, bytes));
        }
        Ok(())
    }
}

impl Console for Lagging {
    fn open(&mut self, deadline: Nanos) -> Result<Nanos, DeviceError> {
        self.console.open(deadline)
    }

    fn receive(&mut self, deadline: Nanos) -> Result<Received, DeviceError> {
        let (at, bytes) = match self.held.take() {
            Some(held) => held,
            None => match self.console.receive(deadline)? {
                Received::Bytes { at, bytes } => (at, bytes),
                other => return Ok(other),
            },
        };
        if at > deadline && !self.careless {
            self.held = Some((at, bytes));
            let at = self.clock.wait_until(deadline);
            return Ok(Received::Deadline { at });
        }
        self.take_next(at)?;
        Ok(Received::Bytes { at, bytes })
    }

    fn send(&mut self, bytes: &[u8]) -> Result<Nanos, DeviceError> {
        let at = self.console.send(bytes)?;
        if self.held.is_none() {
            self.take_next(at)?;
        }
        Ok(at)
    }

    fn close(&mut self) -> Result<(), DeviceError> {
        self.held = None;
        self.console.close()
    }
}

/// A power switch whose command of the number given, from 1, fails before
/// it is sent.
struct Failing(Box<dyn PowerSwitch>, u32, u32);

impl PowerSwitch for Failing {
    fn command(&mut self, state: PowerState) -> Result<Nanos, DeviceError> {
        self.1 += 1;
        if self.1 == self.2 {
            return Err(DeviceError::new(Role::Power, "the command was not sent"));
        }
        self.0.command(state)
    }
}

/// A power switch whose command of the number given, from 1, is sent while
/// the contacts of the relay of `world` stick, so that the supply does not
/// follow it.
struct Sticking(Box<dyn PowerSwitch>, SimWorld, u32, u32);

impl PowerSwitch for Sticking {
    fn command(&mut self, state: PowerState) -> Result<Nanos, DeviceError> {
        self.2 += 1;
        self.1.knobs().relay.stuck = self.2 == self.3;
        let sent = self.0.command(state);
        self.1.knobs().relay.stuck = false;
        sent
    }
}

/// A power switch that sets the flag of an interrupt once it has sent a
/// command of the state given, as SIGINT does.
struct Interrupting(Box<dyn PowerSwitch>, &'static AtomicI32, PowerState);

impl PowerSwitch for Interrupting {
    fn command(&mut self, state: PowerState) -> Result<Nanos, DeviceError> {
        let sent = self.0.command(state)?;
        if state == self.2 {
            self.1.store(2, Ordering::SeqCst);
        }
        Ok(sent)
    }
}

/// An opener of the meter's bridge that finds none.
struct NoBridge;

impl HidOpen for NoBridge {
    type Link = HidapiLink;

    fn open(&mut self) -> Result<HidapiLink, OpenError> {
        Err(OpenError::NoBridge)
    }
}

/// When the bench of a [`TestEnv`] catches an interrupt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Interrupts {
    Never,
    AtOpen,
    After(PowerState),
}

/// Surroundings whose bench is a simulated device of each role on `world`,
/// with a [`Deaf`] console when `deaf` is set, an [`Early`] one when `early`
/// is, a [`Panicking`] one when `panics` is, a [`Lagging`] one when
/// `lagging` is, careless when `careless` is, a [`Failing`] switch when
/// `failing` gives a command and a [`Sticking`] one when `sticking` does,
/// and an INA228 meter whose bridge is missing when `no_bridge` is set.
/// The bench catches interrupts as `interrupts` says, through the flag
/// `flag`; nothing installs a signal handler. When the bench is opened, a
/// directory is made at `block`, if it is given. The revision of the tool
/// is `tool`, when it is given, in place of the one that the build
/// embedded. What the tool says on standard error is kept in `said`, each
/// with the number of power commands sent before it, and the bench is in
/// use when `in_use` is set.
struct TestEnv {
    world: SimWorld,
    script: RefCell<Option<ScriptProvider>>,
    boards: Vec<&'static str>,
    deaf: bool,
    early: bool,
    panics: bool,
    lagging: bool,
    careless: bool,
    failing: Option<u32>,
    sticking: Option<u32>,
    no_bridge: bool,
    interrupts: Interrupts,
    flag: &'static AtomicI32,
    block: Option<PathBuf>,
    tool: Option<ToolRevision>,
    said: RefCell<Vec<(usize, String)>>,
    in_use: bool,
}

impl TestEnv {
    fn new(script: ScriptProvider) -> Self {
        let (world, script) = (SimWorld::new(), RefCell::new(Some(script)));
        let boards = vec![RASPBERRY_PI_5, include_str!("data/synthetic-board.toml")];
        Self {
            world,
            script,
            boards,
            deaf: false,
            early: false,
            panics: false,
            lagging: false,
            careless: false,
            failing: None,
            sticking: None,
            no_bridge: false,
            interrupts: Interrupts::Never,
            flag: Box::leak(Box::new(AtomicI32::new(0))),
            block: None,
            tool: None,
            said: RefCell::new(Vec::new()),
            in_use: false,
        }
    }

    /// What the tool said on standard error.
    fn said(&self) -> Vec<String> {
        self.said
            .borrow()
            .iter()
            .map(|(_, said)| said.clone())
            .collect()
    }

    /// What the tool said on standard error, each with the number of power
    /// commands that the relay had sent before it.
    fn said_after(&self) -> Vec<(usize, String)> {
        self.said.borrow().clone()
    }

    /// The board plays the example console output.
    fn example() -> Self {
        Self::new(script(None, STATS))
    }

    /// Whether the delivery device holds generation `number` with `file`.
    fn staged(&self, number: u32, file: &str) -> bool {
        let generation = GenerationName::new(number).unwrap();
        let mut tftp = SimTftp::new(&self.world);
        let name = PayloadName::parse(file).unwrap();
        tftp.read_staged(&generation, &name).is_ok()
    }
}

impl Env for TestEnv {
    fn open_bench(&self, _: &Board, _: &Instance, _: &Path) -> Result<Bench, String> {
        let script = self.script.borrow_mut().take().ok_or("the bench is open")?;
        let mut bench = env::simulated(&self.world, script);
        if self.deaf {
            bench.console = Box::new(Deaf(bench.console, 0));
        }
        if self.early {
            bench.console = Box::new(Early(bench.console, Rc::clone(&bench.clock)));
        }
        if self.panics {
            bench.console = Box::new(Panicking(bench.console));
        }
        if self.lagging {
            bench.console = Box::new(Lagging {
                console: bench.console,
                clock: Rc::clone(&bench.clock),
                held: None,
                careless: self.careless,
            });
        }
        if self.no_bridge {
            let clock = Rc::clone(&bench.clock);
            let meter = Ina228Meter::new(NoBridge, clock, Quantity::SupplyVoltage, 15_000);
            bench.confirm = Box::new(MeterConfirm(meter));
        }
        if self.interrupts != Interrupts::Never {
            bench.interrupt = Interrupt::of(self.flag);
        }
        match self.interrupts {
            Interrupts::AtOpen => self.flag.store(15, Ordering::SeqCst),
            Interrupts::After(state) => {
                bench.power = Box::new(Interrupting(bench.power, self.flag, state));
            }
            Interrupts::Never => {}
        }
        if let Some(command) = self.failing {
            bench.power = Box::new(Failing(bench.power, 0, command));
        }
        if let Some(command) = self.sticking {
            bench.power = Box::new(Sticking(bench.power, self.world.clone(), 0, command));
        }
        if let Some(block) = &self.block {
            fs::create_dir(block).unwrap();
        }
        Ok(bench)
    }

    fn boards(&self) -> &[&'static str] {
        &self.boards
    }

    fn tool(&self) -> ToolRevision {
        self.tool.clone().unwrap_or_else(ToolRevision::built)
    }

    fn bench_in_use(&self, _: &Instance) -> bool {
        self.in_use
    }

    fn say(&self, message: &str) {
        let sent = self.world.power_commands().len();
        self.said.borrow_mut().push((sent, message.to_string()));
    }
}

/// The exit status of the command line `args`, after the name of the
/// program, on `env`.
fn command(env: &TestEnv, args: &[&OsStr]) -> u8 {
    let line = [OsStr::new("kyto-bench")]
        .into_iter()
        .chain(args.iter().copied());
    cli::run(line, env)
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
    run_example_in(&scratch(name), env)
}

/// Runs the example plan on the example payloads, in `dir`.
fn run_example_in(dir: &Path, env: &TestEnv) -> Ran {
    let examples = examples();
    let plan = examples.join("run-plan.toml");
    run(dir, &plan, &examples.join("payload"), env)
}

/// The example plan with `edit` made, written into `dir`.
fn plan_with(dir: &Path, edit: impl Fn(String) -> String) -> PathBuf {
    let text = String::from_utf8(example("run-plan.toml")).unwrap();
    let (path, edited) = (dir.join("plan.toml"), edit(text.clone()));
    assert_ne!(edited, text, "the edit changes the plan");
    fs::write(&path, edited).unwrap();
    path
}

/// A copy of the example payloads in `dir`, its payload root.
fn payloads_in(dir: &Path) -> PathBuf {
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
    root
}

/// A copy of the example payloads in `dir` with the file `PIEEPROM.UPD` in
/// that of the first entry, which a plan that `eeprom_plan` writes lists.
fn eeprom_payloads(dir: &Path) -> (PathBuf, Sha256) {
    let root = payloads_in(dir);
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

/// The example plan with a third entry, written into `dir`: a copy of the
/// first, `firmware-again`, of one cold boot.
fn three_entries(dir: &Path) -> PathBuf {
    plan_with(dir, |text| {
        let start = text.find("[[entries]]").unwrap();
        let end = text.find("\n# One cold boot").unwrap();
        let first = text[start..end].replacen("\"firmware-only\"", "\"firmware-again\"", 1);
        let third = first.replacen("cold_boots = 3", "cold_boots = 1", 1);
        format!("{text}\n{third}")
    })
}

/// The console output recorded from a Raspberry Pi 5 in the file `name` of
/// `tests/data/recorded/`.
fn recorded(name: &str) -> Vec<u8> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/recorded");
    fs::read(dir.join(name)).unwrap()
}

/// A plan of one cold boot of the example firmware-only payload that ends
/// with `end`, sends nothing and expects the bootloader release of the
/// recordings, written into `dir`.
fn recorded_plan(dir: &Path, end: &str) -> PathBuf {
    let example = String::from_utf8(example("run-plan.toml")).unwrap();
    let from = example.find("payload = [").unwrap();
    let to = from + example[from..].find("\n]\n").unwrap() + 2;
    let plan = format!(
        "format = \"kyto-bench run plan\"\nformat_version = 1\nboard = \"raspberry-pi-5\"\n\n\
         [[entries]]\nname = \"recorded\"\npayload_dir = \"firmware-only\"\n{}\n\
         cold_boots = 1\nboot_timeout_ms = 60000\nend = {end}\nsends = false\n\n\
         [entries.firmware.bootloader]\nhash = \"1accd665\"\ndate = \"2026/09/25\"\n",
        &example[from..to]
    );
    let path = dir.join("plan.toml");
    fs::write(&path, plan).unwrap();
    path
}

/// Asserts that each of `values` occurs in the console output of the first
/// cold boot that the receipt of `ran` holds, and that the public version
/// of the receipt shows none of them, neither in that console output nor
/// in any string.
#[track_caller]
fn assert_hidden(ran: &Ran, values: &[&str]) {
    let receipt = ran.receipt();
    let held = &receipt.entries[0].cold_boots[0].serial.bytes_base64.0;
    let text = fs::read_to_string(ran.out.join("receipt.public.json")).unwrap();
    let stream = "/entries/0/cold_boots/0/serial/bytes_base64";
    let shown = ran.public().receipt.pointer(stream).unwrap().clone();
    let shown = Base64::deserialize(shown).unwrap().0;
    for value in values {
        assert!(occurrences(held, value) > 0, "{value}");
        assert_eq!(occurrences(&shown, value), 0, "{value}");
        assert!(!text.contains(value), "{value}");
    }
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
/// closed; and the positions among the events of the console ready, the
/// power on, the reading that confirmed it, the end and the power off.
struct Times {
    order: [usize; 5],
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
    let ready = position(events, |kind| matches!(kind, EventKind::ConsoleReady {})).unwrap();
    let on_confirmed = confirmed(on).0;
    let reading = |event: &Event| {
        event.at_ns == on_confirmed && matches!(event.event, EventKind::Reading { .. })
    };
    let confirming = on + events[on..].iter().position(reading).unwrap();
    let end = position(events, |kind| matches!(kind, EventKind::End { .. })).unwrap();
    Times {
        order: [ready, on, confirming, end, off],
        ready: key(ready),
        on: key(on),
        on_confirmed: confirmed(on),
        send: at(|kind| matches!(kind, EventKind::Send { .. })),
        end: key(end),
        off: key(off),
        off_confirmed: confirmed(off),
        closed: at(|kind| matches!(kind, EventKind::ConsoleClosed {})).unwrap(),
    }
}

/// Why the cold boot of `events` ended.
fn end(events: &[Event]) -> Option<EndReason> {
    events.iter().find_map(|event| match event.event {
        EventKind::End { reason } => Some(reason),
        _ => None,
    })
}

/// The rules that failed among the checks of `cold_boot`.
fn failed(cold_boot: &ColdBootRecord) -> Vec<&str> {
    let failed = cold_boot
        .checks
        .iter()
        .filter(|c| c.result == Outcome::Fail);
    failed.map(|check| check.rule.as_str()).collect()
}

/// Asserts that the run switched the supply off once `cold_boot` ended and
/// that a reading of the supply at or below the off threshold confirmed it,
/// with the power off check passing; the time it was confirmed.
#[track_caller]
fn assert_confirmed_off(receipt: &Receipt, cold_boot: &ColdBootRecord) -> u64 {
    let confirmation = &receipt.inputs.instance.parameters.confirmation;
    let events = &cold_boot.events;
    let end = position(events, |kind| matches!(kind, EventKind::End { .. })).unwrap();
    let off = position(events, power(Command::Off)).expect("a power off");
    assert_eq!((off, events[off].at_ns), (end + 1, events[end].at_ns));
    let at = power::confirmed(confirmation, events, off).expect("readings confirm the off");
    let reading = events[off..].iter().find_map(|event| match event.event {
        EventKind::Reading { micro, .. } if event.at_ns == at => Some(micro),
        _ => None,
    });
    let threshold = confirmation.off_at_most_micro;
    assert!(
        reading.is_some_and(|micro| micro <= threshold),
        "{reading:?}"
    );
    let check = cold_boot
        .checks
        .iter()
        .find(|c| (c.rule.as_str(), c.subject.as_str()) == ("power.off_unconfirmed", "off"));
    assert_eq!(check.map(|c| c.result), Some(Outcome::Pass));
    at
}

/// The verdicts of the entries of `receipt`, each with those of its cold
/// boots.
fn verdicts(receipt: &Receipt) -> Vec<(Verdict, Vec<Verdict>)> {
    let verdicts = receipt.entries.iter().map(|entry| {
        let boots = entry.cold_boots.iter().map(|c| c.verdict);
        (entry.verdict, boots.collect())
    });
    verdicts.collect()
}

/// The states of the power commands that the relay of `env` sent.
fn commands(env: &TestEnv) -> Vec<PowerState> {
    env.world.power_commands().iter().map(|c| c.1).collect()
}

/// The steps that the cold boot of `events` sent, with their bytes.
fn sends(events: &[Event]) -> Vec<(usize, &[u8])> {
    let send = |event| match event {
        &Event {
            event:
                EventKind::Send {
                    step,
                    ref bytes_base64,
                },
            ..
        } => Some((step, &bytes_base64.0[..])),
        _ => None,
    };
    events.iter().filter_map(send).collect()
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
    // One serial step, which the echo entry has and its cold boot sends
    // once and gets the response to; the firmware-only entry sends nothing.
    assert!(plan.entries[0].steps.is_none());
    assert_eq!(plan.entries[1].steps().len(), 1);
    for cold_boot in &receipt.entries[0].cold_boots {
        assert_eq!(sends(&cold_boot.events), []);
    }
    let echo_boot = &receipt.entries[1].cold_boots[0];
    assert_eq!(sends(&echo_boot.events), [(0, &b"?\r"[..])]);
    let response = echo_boot
        .checks
        .iter()
        .find(|c| c.rule == "step.response")
        .map(|c| (c.subject.as_str(), c.result));
    assert_eq!(response, Some(("0", Outcome::Pass)));
    assert!(echo_boot.observations.steps[0].response_line.is_some());
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
    // Each cold boot keeps the order of the board, the off interval of the
    // board from the last off confirmation, and is switched off and
    // confirmed off.
    let confirmation = &receipt.inputs.instance.parameters.confirmation;
    let board = Board::parse(&receipt.inputs.board_description.text).unwrap();
    let interval = board.cold_boot.off_interval_ms * MS;
    let mut off_at = power::off_confirmed_at(confirmation, &receipt.start.events).unwrap();
    for (record, entry) in receipt.entries.iter().zip(&plan.entries) {
        let switch = position(&record.events, |k| matches!(k, EventKind::Switch { .. }));
        let switched = record.events[switch.unwrap()].at_ns;
        assert!(
            switched >= off_at,
            "the payload is served while the supply is off"
        );
        // The board's order: `publish`, then `wait_off_interval`.
        assert!(
            switched < off_at + interval,
            "the payload is published and checked before the off interval"
        );
        for cold_boot in &record.cold_boots {
            let t = times(&receipt, &cold_boot.events);
            assert!(switched <= t.ready.0);
            assert!(
                t.ready.0 >= off_at + interval,
                "the off interval on the bench clock before the console opens"
            );
            assert!(t.order.is_sorted_by(|a, b| a < b), "{:?}", t.order);
            assert!(t.ready <= t.on && t.on < t.on_confirmed && t.on_confirmed <= t.end);
            assert!(t.end <= t.off && t.off < t.off_confirmed && t.off_confirmed <= t.closed);
            if entry.steps.is_some() {
                let send = t.send.expect("the echo entry sends its step");
                assert!(t.on_confirmed < send && send < t.end);
            } else {
                assert_eq!(t.send, None);
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
    // Each replacement covers the place of its value, and the public stream
    // shows none of the values.
    let shown = public.receipt.pointer(stream).unwrap().clone();
    let shown = Base64::deserialize(shown).unwrap().0;
    assert_eq!(shown.len(), echo_bytes.len());
    for (value, replaced) in values.iter().zip(&public.replacements) {
        assert_eq!(occurrences(&shown, value), 0, "{value}");
        let at = echo_bytes
            .windows(value.len())
            .position(|w| w == value.as_bytes());
        let (start, end) = (at.unwrap(), at.unwrap() + value.len());
        assert!(
            replaced.offset <= start && end <= replaced.offset + replaced.length,
            "{value}"
        );
    }
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
    let fail = run_example("rejudge-fail", &TestEnv::new(script(Some(2), STATS)));
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

/// A check as its rule, subject and result.
type Fixed<'a> = (&'a str, &'a str, Outcome);

/// The checks of each entry of `receipt`, each list followed by those of
/// each of the entry's cold boots.
fn check_lists(receipt: &Receipt) -> Vec<Vec<Fixed<'_>>> {
    fn list(checks: &[Check]) -> Vec<Fixed<'_>> {
        let fixed = checks
            .iter()
            .map(|c| (c.rule.as_str(), c.subject.as_str(), c.result));
        fixed.collect()
    }
    let lists = receipt.entries.iter().flat_map(|entry| {
        let boots = entry.cold_boots.iter().map(|c| c.checks.as_slice());
        std::iter::once(&entry.checks[..]).chain(boots).map(list)
    });
    lists.collect()
}

#[test]
fn the_checks_of_the_example_runs_are_fixed() {
    use Outcome::{Fail, Pass};
    let passing = |checks: &[(&'static str, &'static str)]| -> Vec<Fixed<'static>> {
        checks
            .iter()
            .map(|&(rule, subject)| (rule, subject, Pass))
            .collect()
    };
    // The payload of either entry, in the order of the plan.
    let served = [
        ("payload.served_digest", "config.txt"),
        ("payload.served_digest", "bcm2712-rpi-5-b.dtb"),
        ("payload.served_digest", "overlays/bcm2712d0.dtbo"),
        ("payload.served_digest", "kernel.img"),
    ];
    // The first entry is checked first for the power off at the start.
    let first = [&[("power.off_unconfirmed", "start")][..], &served].concat();
    let supply = [
        ("console.not_ready", "console"),
        ("power.on_unconfirmed", "on"),
        ("power.off_interval", "on"),
        ("power.off_unconfirmed", "off"),
        ("power.order", "events"),
        ("console.before_power_on", "console"),
    ];
    let reads = [
        ("tftp.server", "servers"),
        ("tftp.prefix", "reads"),
        ("tftp.unplanned_file", "reads"),
        ("identity.disagree", "identity"),
    ];
    // The firmware-only entry: its end marker, two counts, one order, no
    // send, and its firmware versions in the order of their names.
    let order = "board.bootloader board.network_boot board.os_start started";
    let firmware_only = [
        ("boot.network", "board.network_boot"),
        ("boot.fallback", "board.network_boot"),
        ("boot.end", "started"),
        ("console.byte_limit", "console"),
        ("marker.count", "board.network_boot"),
        ("marker.count", "board.sd_boot"),
        ("marker.order", order),
        ("step.declared_none", "sends"),
        ("firmware.version", "bl31"),
        ("firmware.version", "bootloader"),
    ];
    // The echo entry: its end marker, one count, one forbidden line, two
    // markers before the first send, one step and one segment.
    let echo = [
        ("boot.network", "board.network_boot"),
        ("boot.fallback", "board.network_boot"),
        ("boot.end", "ready"),
        ("console.byte_limit", "console"),
        ("marker.count", "ready"),
        ("marker.forbidden", "0"),
        ("marker.before_first_send", "board.network_boot"),
        ("marker.before_first_send", "ready"),
        ("step.response", "0"),
        ("segment.digest", "banner"),
    ];
    let firmware_only = passing(&[&supply[..], &firmware_only, &reads].concat());
    let echo = passing(&[&supply[..], &echo, &reads].concat());
    let (first, served) = (passing(&first), passing(&served));
    let ran = run_example("fixed-pass", &TestEnv::example());
    assert_eq!(ran.status, cli::PASS);
    let expected = [
        &first,
        &firmware_only,
        &firmware_only,
        &firmware_only,
        &served,
        &echo,
    ];
    let receipt = ran.receipt();
    assert_eq!(check_lists(&receipt), expected.map(Vec::clone));
    // The second cold boot of the first entry does not show its end marker,
    // which is also the last marker of its order, and the echo entry forbids
    // the line of its build id, which its cold boot shows.
    let dir = scratch("fixed-fail");
    let plan = plan_with(&dir, |plan| {
        plan.replace(
            "forbid = [{ line = '^PANIC' }]",
            "forbid = [{ line = '^BUILD' }]",
        )
    });
    let env = TestEnv::new(script(Some(2), STATS));
    let ran = run(&dir, &plan, &examples().join("payload"), &env);
    assert_eq!(ran.status, cli::FAIL);
    let failing = |checks: &[Fixed<'static>], rules: &[&str]| -> Vec<Fixed<'static>> {
        let fixed = |&(rule, subject, result): &Fixed<'static>| {
            (
                rule,
                subject,
                if rules.contains(&rule) { Fail } else { result },
            )
        };
        checks.iter().map(fixed).collect()
    };
    let cut = failing(&firmware_only, &["boot.end", "marker.order"]);
    let forbidden = failing(&echo, &["marker.forbidden"]);
    let expected = [&first, &firmware_only, &cut, &served, &forbidden];
    let receipt = ran.receipt();
    assert_eq!(check_lists(&receipt), expected.map(Vec::clone));
    // Each detail names what was not found or the line found, by its
    // offset: the BUILD line starts at byte 827 of the echo output.
    let details: Vec<(&str, Option<&str>)> = receipt
        .entries
        .iter()
        .flat_map(|entry| entry.cold_boots.iter().flat_map(|c| &c.checks))
        .filter(|check| check.result != Pass)
        .map(|check| (check.rule.as_str(), check.detail.as_deref()))
        .collect();
    let failed = [
        ("boot.end", Some("not by the boot timeout")),
        ("marker.order", Some("not in this order")),
        ("marker.forbidden", Some("the line at 827")),
    ];
    assert_eq!(details, failed);
    let verdicts = receipt.entries.iter().map(|entry| {
        let boots = entry.cold_boots.iter().map(|c| c.verdict);
        (entry.verdict, boots.collect::<Vec<_>>())
    });
    let (pass, fail) = (Verdict::Pass, Verdict::Fail);
    let verdicts: Vec<_> = verdicts.collect();
    assert_eq!(verdicts, [(fail, vec![pass, fail]), (fail, vec![fail])]);
    assert_eq!(receipt.verdict, Some(fail));
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
    // The bench was not opened: nothing was staged and no power command was
    // sent.
    assert!(env.script.borrow().is_some(), "the bench was opened");
    assert!(!env.staged(1, "config.txt"));
    assert!(env.world.power_commands().is_empty());
}

#[test]
fn a_source_file_of_another_digest_refuses_the_run_before_the_bench_opens() {
    let dir = scratch("source-digest");
    let kernel = "7a4b18efd2c0283b922acb12a3a7faf0d3ff8ba0c803a7d0c7669c09e21e920b";
    let other = Sha256::of(b"another kernel");
    let plan = plan_with(&dir, |plan| plan.replacen(kernel, &other.to_string(), 1));
    let env = TestEnv::example();
    let ran = run(&dir, &plan, &examples().join("payload"), &env);
    assert_eq!(ran.status, cli::REFUSED);
    let receipt = ran.receipt();
    let refusal = receipt.refusal.as_ref().unwrap();
    let found = (
        refusal.rule.as_str(),
        refusal.entry,
        refusal.subject.as_str(),
        refusal.detail.as_str(),
    );
    let detail = format!("the source copy of kernel.img has the SHA-256 {kernel}, not {other}");
    let expected = ("payload.source_digest", Some(0), "kernel.img", &detail[..]);
    assert_eq!(found, expected);
    assert_eq!(receipt.verdict, None);
    assert!(receipt.start.events.is_empty());
    // The bench was not opened: nothing was staged and no power command was
    // sent.
    assert!(env.script.borrow().is_some(), "the bench was opened");
    assert!(!env.staged(1, "config.txt"));
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
        // The payload is published once, for the first cold boot.
        let switched = |e: &&Event| matches!(e.event, EventKind::Switch { .. });
        assert_eq!(entry.events.iter().filter(switched).count(), 1);
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
fn power_records_name_only_the_command() {
    let env = TestEnv::example();
    let receipt = run_example("power-records", &env).receipt();
    let boots = receipt.entries.iter().flat_map(|e| &e.cold_boots);
    let events = receipt
        .start
        .events
        .iter()
        .chain(boots.flat_map(|c| &c.events));
    let recorded: Vec<(u64, PowerState)> = events
        .filter_map(|event| match &event.event {
            EventKind::Power { command, relay } => {
                assert_eq!(relay, &None, "a simulated power device names no relay");
                let state = match command {
                    Command::On => PowerState::On,
                    Command::Off => PowerState::Off,
                };
                Some((event.at_ns, state))
            }
            _ => None,
        })
        .collect();
    // Each command as the switch sent it, at the time it reported.
    let sent: Vec<(u64, PowerState)> = env
        .world
        .power_commands()
        .iter()
        .map(|(at, s)| (at.0, *s))
        .collect();
    assert_eq!(recorded, sent);
}

#[test]
fn residual_voltage_above_the_off_threshold_is_not_off() {
    // A supply that keeps 0.6 V while off, above the 0.5 V of the example
    // configuration: the power off at the start is never confirmed.
    let env = TestEnv::example();
    env.world.knobs().supply.residual_micro = 600_000;
    let ran = run_example("residual", &env);
    assert_eq!(ran.status, cli::BENCH_FAULT);
    let receipt = ran.receipt();
    let first = &receipt.entries[0];
    assert!(first.cold_boots.is_empty());
    let check = &first.checks[0];
    let found = (check.rule.as_str(), check.subject.as_str(), check.result);
    assert_eq!(found, ("power.off_unconfirmed", "start", Outcome::Fault));
    assert_eq!(receipt.entries[1].verdict, Verdict::NotRun);
    // The run switches the supply off once more, which readings do not
    // confirm either, and it records that in the first entry.
    let confirmation = &receipt.inputs.instance.parameters.confirmation;
    let commands: Vec<Command> = first.events.iter().filter_map(power::command).collect();
    assert_eq!(commands, [Command::Off]);
    assert_eq!(power::off_confirmed_at(confirmation, &first.events), None);
    let sent: Vec<PowerState> = env.world.power_commands().iter().map(|c| c.1).collect();
    assert_eq!(sent, [PowerState::Off; 2], "nothing is switched on");
    let said = "the supply was switched off again after a fault; readings did not confirm the \
                power off";
    assert_eq!(env.said(), [said]);
}

#[test]
fn an_unconfirmed_power_on_ends_at_its_timeout_and_is_switched_off() {
    // Relay contacts that never close: a reading every 100 ms up to the 3 s
    // timeout of the example configuration, then the power off of the cold
    // boot, which readings confirm, so the entry sends no second one.
    let env = TestEnv::example();
    env.world.knobs().relay.stuck = true;
    let ran = run_example("on-timeout", &env);
    let receipt = ran.receipt();
    let first = &receipt.entries[0];
    let events = &first.cold_boots[0].events;
    let on = position(events, power(Command::On)).unwrap();
    let off = position(events, power(Command::Off)).expect("the cold boot powers off");
    let sent = events[on].at_ns;
    let reading = |event: &&Event| matches!(event.event, EventKind::Reading { .. });
    let readings = events[on..off].iter().filter(reading);
    let after: Vec<u64> = readings.map(|e| e.at_ns - sent).collect();
    let polls: Vec<u64> = (1..=30).map(|poll| poll * 100 * MS).collect();
    assert_eq!(after, polls);
    assert_eq!(events[off].at_ns, sent + 3_000 * MS);
    let confirmation = &receipt.inputs.instance.parameters.confirmation;
    assert!(power::off_confirmed_at(confirmation, events).is_some());
    assert!(first.events.iter().all(|e| power::command(e).is_none()));
    // A fault of the bench: the cold boot, its entry and the run are BENCH
    // FAULT with the exit status 3, and no verdict is FAIL, although the
    // checks of the board, which never came on, are made and some fail.
    assert_eq!(ran.status, cli::BENCH_FAULT);
    let cold_boot = &first.cold_boots[0];
    let rules = failed(cold_boot);
    assert!(rules.contains(&"boot.end"), "{rules:?}");
    let fault = Verdict::BenchFault;
    let found = (cold_boot.verdict, first.verdict, receipt.verdict);
    assert_eq!(found, (fault, fault, Some(fault)));
    let all = verdicts(&receipt)
        .into_iter()
        .flat_map(|(entry, boots)| std::iter::once(entry).chain(boots));
    assert!(all.chain(receipt.verdict).all(|v| v != Verdict::Fail));
}

#[test]
fn a_fault_that_leaves_the_supply_unconfirmed_switches_it_off_again() {
    // The switch fails to send its third command, the power off after the
    // first cold boot, or its first, the power off at the start.
    let after_cold_boot = [PowerState::Off, PowerState::On, PowerState::Off];
    for (command, sent) in [(3, &after_cold_boot[..]), (1, &[PowerState::Off])] {
        let mut env = TestEnv::example();
        env.failing = Some(command);
        let ran = run_example(&format!("off-again-{command}"), &env);
        assert_eq!(ran.status, cli::BENCH_FAULT);
        let receipt = ran.receipt();
        let verdicts: Vec<Verdict> = receipt.entries.iter().map(|e| e.verdict).collect();
        assert_eq!(verdicts, [Verdict::BenchFault, Verdict::NotRun]);
        // The failed command and the one sent again after it.
        let commands: Vec<PowerState> = env.world.power_commands().iter().map(|c| c.1).collect();
        assert_eq!(commands, sent, "command {command}");
        let first = &receipt.entries[0];
        let boots = first.cold_boots.iter().flat_map(|c| &c.checks);
        let error = |c: &&Check| (c.rule.as_str(), c.subject.as_str()) == ("device.error", "power");
        let checks = first.checks.iter().chain(boots);
        assert_eq!(checks.filter(error).count(), 1, "command {command}");
        // Readings confirmed the power off sent again, which the entry
        // records, and the supply is off.
        let confirmation = &receipt.inputs.instance.parameters.confirmation;
        let commands: Vec<Command> = first.events.iter().filter_map(power::command).collect();
        assert_eq!(commands, [Command::Off]);
        assert!(power::off_confirmed_at(confirmation, &first.events).is_some());
        let mut meter = SimSupplyMeter::new(&env.world);
        assert_eq!(meter.sample().unwrap().micro, 0);
        let said = "the supply was switched off again after a fault; readings did confirm the \
                    power off";
        assert_eq!(env.said(), [said], "command {command}");
    }
}

#[test]
fn a_run_that_panics_switches_the_supply_off_and_leaves_no_output() {
    let mut env = TestEnv::example();
    env.panics = true;
    let dir = scratch("panic");
    let ran = panic::catch_unwind(AssertUnwindSafe(|| run_example_in(&dir, &env)));
    assert!(ran.is_err(), "the panic goes on once the supply is off");
    // The panic came after the first power on, and the supply was switched
    // off again: the last command is an off, and the supply reads 0 V.
    let commands: Vec<PowerState> = env.world.power_commands().iter().map(|c| c.1).collect();
    assert_eq!(commands, [PowerState::Off, PowerState::On, PowerState::Off]);
    let mut meter = SimSupplyMeter::new(&env.world);
    assert_eq!(meter.sample().unwrap().micro, 0);
    assert!(
        !dir.join("out").exists(),
        "no receipt and no output directory"
    );
    // It said so before the panic went on.
    let said = "the run stopped; readings did confirm the power off";
    assert_eq!(env.said(), [said]);
}

#[test]
fn a_trigger_name_with_a_trailing_dot_is_refused() {
    // A file system that drops a trailing dot would serve this file as
    // `pieeprom.upd`, a write trigger, to a board whose plan enables no
    // write; no payload name ends with a dot.
    let dir = scratch("trailing-dot");
    let (payloads, digest) = eeprom_payloads(&dir);
    let from = payloads.join("firmware-only/PIEEPROM.UPD");
    fs::rename(&from, from.with_file_name("pieeprom.upd.")).unwrap();
    let plan = plan_with(&dir, |text| {
        let file = format!("payload = [\n  {{ name = \"pieeprom.upd.\", sha256 = \"{digest}\" }},");
        text.replacen("payload = [", &file, 1)
    });
    let env = TestEnv::example();
    let ran = run(&dir, &plan, &payloads, &env);
    assert_eq!(ran.status, cli::INPUT_ERROR);
    assert!(!ran.out.exists(), "no receipt");
    assert!(!env.staged(1, "config.txt"), "nothing staged");
    assert!(env.world.power_commands().is_empty(), "no power command");
}

#[test]
fn a_deadline_that_the_console_reports_early_ends_no_cold_boot() {
    // The firmware-only entry captures until its boot timeout, and the
    // console reports a deadline 100 ms after each read that gets nothing.
    let dir = scratch("early-deadline");
    let plan = plan_with(&dir, |plan| {
        let end = "boot_timeout_ms = 60000\nend = { marker = \"started\" }";
        plan.replacen(end, "boot_timeout_ms = 2000\nend = \"timeout\"", 1)
    });
    let mut env = TestEnv::example();
    env.early = true;
    let ran = run(&dir, &plan, &examples().join("payload"), &env);
    assert_eq!(ran.status, cli::PASS);
    let receipt = ran.receipt();
    for cold_boot in &receipt.entries[0].cold_boots {
        assert_eq!(end(&cold_boot.events), Some(EndReason::BootTimeout));
        let t = times(&receipt, &cold_boot.events);
        assert_eq!(t.end.0, t.on.0 + 2000 * MS, "the boot timeout of the plan");
    }
}

#[test]
fn an_end_marker_without_its_line_feed_ends_no_cold_boot() {
    // The last line of the first cold boot, its end marker, has no CR LF.
    let mut example = script(None, STATS);
    let env = TestEnv::new(Box::new(move |power_on, files: &Files| {
        let mut script = example(power_on, files);
        if power_on == 1 {
            let last = &mut script.chunks.last_mut().unwrap().1;
            assert!(last.ends_with(b"started\r\n"));
            last.truncate(last.len() - 2);
        }
        script
    }));
    let ran = run_example("unterminated-end", &env);
    assert_eq!(ran.status, cli::FAIL);
    let receipt = ran.receipt();
    let verdicts: Vec<Verdict> = receipt.entries.iter().map(|e| e.verdict).collect();
    assert_eq!(verdicts, [Verdict::Fail, Verdict::Pass]);
    let cold_boot = &receipt.entries[0].cold_boots[0];
    assert!(cold_boot.serial.bytes_base64.0.ends_with(b"started"));
    assert_eq!(end(&cold_boot.events), Some(EndReason::BootTimeout));
    assert_eq!(failed(cold_boot), ["boot.end"]);
}

#[test]
fn an_output_file_that_cannot_be_written_has_a_status_of_its_own() {
    // A directory takes the place of the file once the run has created its
    // output directory.
    let both = [
        (output::PUBLIC, cli::PUBLIC_REFUSED),
        (output::RECEIPT, cli::INPUT_ERROR),
    ];
    for (file, status) in both {
        let dir = scratch(&format!("unwritable-{file}"));
        let mut env = TestEnv::example();
        env.block = Some(dir.join("out").join(file));
        let ran = run_example_in(&dir, &env);
        assert_eq!(ran.status, status, "{file}");
        // A receipt is written whole or not at all.
        let receipt = ran.out.join(output::RECEIPT).is_file();
        assert_eq!(receipt, file == output::PUBLIC, "{file}");
        if receipt {
            assert_eq!(ran.receipt().verdict, Some(Verdict::Pass));
        }
    }
}

#[test]
fn an_output_file_that_is_already_there_is_kept_as_it_was() {
    let dir = scratch("output-new");
    let receipt = run_example_in(&dir, &TestEnv::example()).receipt();
    let instance = Instance::parse(&String::from_utf8(example("instance.toml")).unwrap()).unwrap();
    let hidden = instance.deidentify_values();
    for file in [output::PUBLIC, output::RECEIPT] {
        let out = dir.join(file);
        output::create(&out).unwrap();
        fs::write(out.join(file), b"kept").unwrap();
        // A receipt that is there fails the write; a public version that is
        // there is one that was not written.
        match output::write(&out, &receipt, &hidden) {
            Ok(why) => assert_eq!((file, why.len()), (output::PUBLIC, 1)),
            Err(_) => assert_eq!(file, output::RECEIPT),
        }
        assert_eq!(fs::read(out.join(file)).unwrap(), b"kept", "{file}");
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
    // A command line without every path, or with no command.
    for line in [&["kyto-bench", "run", "--plan", "p"][..], &["kyto-bench"]] {
        assert_eq!(cli::run(line, &env), cli::INPUT_ERROR, "{line:?}");
    }
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

#[test]
fn a_response_without_its_line_feed_is_no_response() {
    // The board answers the step of the echo entry with no line end after
    // the answer, so its last line is never complete.
    let answer = &STATS[..STATS.len() - 2];
    let env = TestEnv::new(script(None, answer));
    let ran = run_example("unterminated-response", &env);
    assert_eq!(ran.status, cli::FAIL);
    let receipt = ran.receipt();
    assert_eq!(receipt.verdict, Some(Verdict::Fail));
    let cold_boot = &receipt.entries[1].cold_boots[0];
    assert!(cold_boot.serial.bytes_base64.0.ends_with(answer));
    let end = cold_boot.events.iter().find_map(|event| match event.event {
        EventKind::End { reason } => Some(reason),
        _ => None,
    });
    assert_eq!(end, Some(EndReason::StepTimeout));
    let failed: Vec<&str> = cold_boot
        .checks
        .iter()
        .filter(|check| check.result == Outcome::Fail)
        .map(|check| check.rule.as_str())
        .collect();
    assert_eq!(failed, ["step.response"]);
}

#[test]
fn a_cold_boot_ends_at_its_deadline_whatever_the_console_reports() {
    // The first cold boot never shows its end marker, and the console never
    // reports a deadline.
    let mut env = TestEnv::new(script(Some(1), STATS));
    env.deaf = true;
    let ran = run_example("deaf-console", &env);
    assert_eq!(ran.status, cli::FAIL);
    let receipt = ran.receipt();
    let events = &receipt.entries[0].cold_boots[0].events;
    let end = events.iter().find_map(|event| match event.event {
        EventKind::End { reason } => Some(reason),
        _ => None,
    });
    assert_eq!(end, Some(EndReason::BootTimeout));
    let t = times(&receipt, events);
    assert_eq!(
        t.end.0,
        t.on.0 + 60_000 * MS,
        "the boot timeout of the plan"
    );
    assert!(t.end <= t.off && t.off < t.off_confirmed);
    let verdicts: Vec<Verdict> = receipt.entries.iter().map(|e| e.verdict).collect();
    assert_eq!(verdicts, [Verdict::Fail, Verdict::Pass]);
}

#[test]
fn a_run_with_an_unknown_commit_or_a_changed_tree_is_not_evidence() {
    // The revisions that a build gives when Git cannot tell the commit, or
    // when the working tree had changes or Git cannot tell whether it had.
    let commit = "0123456789abcdef0123456789abcdef01234567";
    let (unknown, changed) = ("evidence.commit_unknown", "evidence.tree_changed");
    let cases: [(Option<&str>, Option<bool>, &[&str]); 5] = [
        (None, Some(false), &[unknown, changed]),
        (None, Some(true), &[unknown]),
        (Some(commit), Some(false), &[changed]),
        (Some(commit), None, &[changed]),
        (Some(commit), Some(true), &[]),
    ];
    for (number, (commit, clean, reasons)) in cases.into_iter().enumerate() {
        let mut env = TestEnv::example();
        let tool = ToolRevision {
            commit: commit.map(str::to_string),
            clean,
        };
        env.tool = Some(tool);
        let ran = run_example(&format!("not-evidence-{number}"), &env);
        // The revision changes no verdict.
        assert_eq!(ran.status, cli::PASS, "{commit:?} {clean:?}");
        let receipt = ran.receipt();
        assert_eq!(
            (receipt.tool.commit.as_deref(), receipt.tool.clean),
            (commit, clean)
        );
        // Each reason that applies, with what it found, then that of the
        // simulated devices, which every run here has.
        let found: Vec<&str> = receipt
            .evidence
            .reasons
            .iter()
            .map(|r| r.rule.as_str())
            .collect();
        assert_eq!(found, [reasons, &["evidence.simulated"]].concat());
        assert!(!receipt.evidence.eligible);
        let public = ran.public();
        let eligible = public.receipt.pointer("/evidence/eligible");
        assert_eq!(eligible.and_then(|value| value.as_bool()), Some(false));
        if number == 0 {
            let details: Vec<&str> = receipt
                .evidence
                .reasons
                .iter()
                .map(|r| r.detail.as_str())
                .collect();
            let expected = [
                "the commit of the tool is unknown",
                "the working tree of the tool had changes",
                "the run used simulated devices",
            ];
            assert_eq!(details, expected);
        }
    }
}

#[test]
fn a_recorded_boot_without_the_network_marker_fails() {
    // A cold boot of a Raspberry Pi 5, recorded up to the line before the
    // firmware's network boot mode line, in an entry that ends at its boot
    // timeout.
    let dir = scratch("recorded-no-network");
    let plan = recorded_plan(&dir, "\"timeout\"");
    let bytes = recorded("no-network-boot.bin");
    let env = TestEnv::new(replay(vec![bytes.clone()]));
    let ran = run(&dir, &plan, &examples().join("payload"), &env);
    assert_eq!(ran.status, cli::FAIL);
    let receipt = ran.receipt();
    let cold_boot = &receipt.entries[0].cold_boots[0];
    assert_eq!(cold_boot.serial.bytes_base64.0, bytes, "replayed whole");
    assert_eq!(end(&cold_boot.events), Some(EndReason::BootTimeout));
    assert_eq!(failed(cold_boot), ["boot.network"]);
    let network = cold_boot.checks.iter().find(|c| c.rule == "boot.network");
    let detail = network.and_then(|c| c.detail.as_deref());
    assert_eq!(detail, Some("no network boot mode line"));
    let fail = Verdict::Fail;
    assert_eq!(verdicts(&receipt), [(fail, vec![fail])]);
    assert_eq!(receipt.verdict, Some(fail));
    // The public version does not show the board's serial number, which the
    // recording gives twice.
    assert_eq!(occurrences(&bytes, "a1b2c3d4"), 2);
    assert_hidden(&ran, &["a1b2c3d4"]);
}

#[test]
fn a_recorded_fall_back_to_the_sd_card_fails() {
    // A boot of a Raspberry Pi 5, recorded from the start of its bootloader
    // after a reset: it boots from the network, gets no answer to its TFTP
    // requests, falls back to the SD card and starts the operating system
    // from there, which ends the entry. The instance has the TFTP server
    // address and path prefix of the recording.
    let dir = scratch("recorded-fallback");
    let plan = recorded_plan(&dir, "{ marker = \"board.os_start\" }");
    let instance = String::from_utf8(example("instance.toml")).unwrap();
    let tftp = "server_address = \"192.0.2.10\"\npath_prefix = \"\"\n";
    assert!(instance.contains(tftp));
    let recorded_tftp = "server_address = \"198.51.100.13\"\npath_prefix = \"tftp-rt/\"\n";
    fs::write(
        dir.join("instance.toml"),
        instance.replace(tftp, recorded_tftp),
    )
    .unwrap();
    let bytes = recorded("fallback-to-sd.bin");
    let env = TestEnv::new(replay(vec![bytes.clone()]));
    let payloads = examples().join("payload");
    let ran = run_with(&dir, &plan, &dir.join("instance.toml"), &payloads, &env);
    assert_eq!(ran.status, cli::FAIL);
    let receipt = ran.receipt();
    let cold_boot = &receipt.entries[0].cold_boots[0];
    // The cold boot holds the recording from its start to the end of the
    // entry.
    let held = &cold_boot.serial.bytes_base64.0;
    assert!(bytes.starts_with(held), "a prefix");
    assert_eq!(end(&cold_boot.events), Some(EndReason::EndCondition));
    assert_eq!(failed(cold_boot), ["boot.fallback"]);
    // The failure names the SD card's boot mode line, the last one.
    let sd = b" 76.99 Boot mode: SD (01) order f\r\n";
    let at = bytes.windows(sd.len()).position(|w| w == sd).unwrap();
    let fallback = cold_boot.checks.iter().find(|c| c.rule == "boot.fallback");
    let detail = fallback.and_then(|c| c.detail.clone());
    assert_eq!(detail, Some(format!("the line at {at}")));
    // Every line that names a TFTP server names that of the instance.
    let servers = &cold_boot.observations.tftp.servers;
    assert!(!servers.is_empty() && servers.iter().all(|server| server.instance));
    let fail = Verdict::Fail;
    assert_eq!(verdicts(&receipt), [(fail, vec![fail])]);
    assert_eq!(receipt.verdict, Some(fail));
    // The public version shows none of the values that the recording holds
    // for the board's serial number, the MAC and IP addresses, the SD card's
    // identifier and card-specific data and the partition UUID of the root
    // file system, nor the path prefix of the instance.
    let hidden = [
        "a1b2c3d4",
        "00:00:5e:00:53:b1",
        "00:00:5e:00:53:b2",
        "198.51.100.11",
        "198.51.100.12",
        "198.51.100.13",
        "c1d0c1d0c1d0c1d0c1d0c1d0c1d0c1d0",
        "c5d0c5d0c5d0c5d0c5d0c5d0c5d0c5d0",
        "5ca1ab1e",
        "tftp-rt/",
    ];
    assert_hidden(&ran, &hidden);
}

#[test]
fn a_boot_timeout_fails_and_the_supply_is_confirmed_off() {
    // The board writes nothing after the line before the end marker of the
    // first cold boot, so the cold boot runs to its boot timeout; the order
    // of the entry leaves the end marker out, so `boot.end` alone fails.
    let dir = scratch("boot-timeout");
    let plan = plan_with(&dir, |plan| plan.replacen(", \"started\"] }]", "] }]", 1));
    let env = TestEnv::new(script(Some(1), STATS));
    let ran = run(&dir, &plan, &examples().join("payload"), &env);
    assert_eq!(ran.status, cli::FAIL);
    let receipt = ran.receipt();
    let cold_boot = &receipt.entries[0].cold_boots[0];
    assert_eq!(end(&cold_boot.events), Some(EndReason::BootTimeout));
    let t = times(&receipt, &cold_boot.events);
    assert_eq!(
        t.end.0,
        t.on.0 + 60_000 * MS,
        "the boot timeout of the plan"
    );
    assert_eq!(failed(cold_boot), ["boot.end"]);
    // The supply was switched off at the end and a reading confirmed it,
    // before the next entry ran.
    let off_at = assert_confirmed_off(&receipt, cold_boot);
    assert!(receipt.entries[1].events[0].at_ns >= off_at);
    let (pass, fail) = (Verdict::Pass, Verdict::Fail);
    let expected = [(fail, vec![fail]), (pass, vec![pass])];
    assert_eq!(verdicts(&receipt), expected);
    assert_eq!(receipt.verdict, Some(fail));
    let on_off = [PowerState::On, PowerState::Off];
    assert_eq!(
        commands(&env),
        [&[PowerState::Off][..], &on_off, &on_off].concat()
    );
}

#[test]
fn a_missing_boot_marker_fails_and_the_supply_is_confirmed_off() {
    // The first cold boot shows no network boot mode line, the line of the
    // board's network boot marker, and still shows its end marker.
    let network = "Boot mode: NETWORK (02)";
    let mut example = script(None, STATS);
    let env = TestEnv::new(Box::new(move |power_on, files: &Files| {
        let mut script = example(power_on, files);
        if power_on == 1 {
            let lines = script.chunks.len();
            script
                .chunks
                .retain(|(_, line)| occurrences(line, network) == 0);
            assert_eq!(script.chunks.len(), lines - 1);
        }
        script
    }));
    let ran = run_example("missing-boot-marker", &env);
    assert_eq!(ran.status, cli::FAIL);
    let receipt = ran.receipt();
    let cold_boot = &receipt.entries[0].cold_boots[0];
    assert_eq!(end(&cold_boot.events), Some(EndReason::EndCondition));
    let failed: Vec<(&str, &str)> = cold_boot
        .checks
        .iter()
        .filter(|c| c.result != Outcome::Pass)
        .map(|c| (c.rule.as_str(), c.subject.as_str()))
        .collect();
    let order = "board.bootloader board.network_boot board.os_start started";
    let expected = [
        ("boot.network", "board.network_boot"),
        ("marker.count", "board.network_boot"),
        ("marker.order", order),
    ];
    assert_eq!(failed, expected);
    // The supply was switched off once the end marker came and a reading
    // confirmed it, before the next entry ran.
    let off_at = assert_confirmed_off(&receipt, cold_boot);
    assert!(receipt.entries[1].events[0].at_ns >= off_at);
    let (pass, fail) = (Verdict::Pass, Verdict::Fail);
    let expected = [(fail, vec![fail]), (pass, vec![pass])];
    assert_eq!(verdicts(&receipt), expected);
    assert_eq!(receipt.verdict, Some(fail));
    let on_off = [PowerState::On, PowerState::Off];
    assert_eq!(
        commands(&env),
        [&[PowerState::Off][..], &on_off, &on_off].concat()
    );
}

#[test]
fn an_unconfirmed_power_off_after_a_cold_boot_is_a_bench_fault() {
    // The relay's contacts stick at the third command, the power off after
    // the first cold boot, which showed what its entry expects: the supply
    // stays on, and a reading every 100 ms up to the 3 s timeout of the
    // example configuration reads it on.
    let mut env = TestEnv::example();
    env.sticking = Some(3);
    let ran = run_example("off-unconfirmed", &env);
    assert_eq!(ran.status, cli::BENCH_FAULT);
    let receipt = ran.receipt();
    let first = &receipt.entries[0];
    let cold_boot = &first.cold_boots[0];
    let events = &cold_boot.events;
    let off = position(events, power(Command::Off)).unwrap();
    let sent = events[off].at_ns;
    let readings: Vec<(u64, i64)> = events[off..]
        .iter()
        .filter_map(|event| match event.event {
            EventKind::Reading { micro, .. } => Some((event.at_ns - sent, micro)),
            _ => None,
        })
        .collect();
    let polls: Vec<(u64, i64)> = (1..=30).map(|poll| (poll * 100 * MS, ON_MICRO)).collect();
    assert_eq!(readings, polls);
    // That power off is the only check that does not pass, and a fault: the
    // cold boot, its entry and the run are BENCH FAULT, never PASS, and the
    // second entry is NOT RUN.
    let other: Vec<(&str, &str, Outcome)> = cold_boot
        .checks
        .iter()
        .filter(|c| c.result != Outcome::Pass)
        .map(|c| (c.rule.as_str(), c.subject.as_str(), c.result))
        .collect();
    assert_eq!(other, [("power.off_unconfirmed", "off", Outcome::Fault)]);
    let (fault, not_run) = (Verdict::BenchFault, Verdict::NotRun);
    assert_eq!(
        verdicts(&receipt),
        [(fault, vec![fault]), (not_run, vec![])]
    );
    assert_eq!(receipt.verdict, Some(fault));
    // The run switched the supply off once more, recorded in the entry,
    // and readings confirmed it.
    let confirmation = &receipt.inputs.instance.parameters.confirmation;
    let again: Vec<Command> = first.events.iter().filter_map(power::command).collect();
    assert_eq!(again, [Command::Off]);
    assert!(power::off_confirmed_at(confirmation, &first.events).is_some());
    let on_off_off = [PowerState::On, PowerState::Off, PowerState::Off];
    assert_eq!(
        commands(&env),
        [&[PowerState::Off][..], &on_off_off].concat()
    );
    let mut meter = SimSupplyMeter::new(&env.world);
    assert_eq!(meter.sample().unwrap().micro, 0);
}

#[test]
fn a_failed_cold_boot_ends_its_entry_and_the_next_entry_runs() {
    // A plan of three entries, whose first one's second cold boot of three
    // does not show its end marker.
    let dir = scratch("failed-then-next");
    let plan = three_entries(&dir);
    let env = TestEnv::new(script(Some(2), STATS));
    let ran = run(&dir, &plan, &examples().join("payload"), &env);
    assert_eq!(ran.status, cli::FAIL);
    let receipt = ran.receipt();
    let (pass, fail) = (Verdict::Pass, Verdict::Fail);
    let expected = [
        (fail, vec![pass, fail]),
        (pass, vec![pass]),
        (pass, vec![pass]),
    ];
    assert_eq!(verdicts(&receipt), expected);
    assert_eq!(receipt.verdict, Some(fail));
    // The failed cold boot was switched off and confirmed off; then the
    // next entry's payload was served, and its cold boot started once the
    // off interval had passed since that confirmation.
    let off_at = assert_confirmed_off(&receipt, &receipt.entries[0].cold_boots[1]);
    let next = &receipt.entries[1];
    let switch = position(&next.events, |k| matches!(k, EventKind::Switch { .. }));
    assert!(next.events[switch.unwrap()].at_ns >= off_at);
    let t = times(&receipt, &next.cold_boots[0].events);
    assert_eq!(t.ready.0, off_at + 5000 * MS);
    let on_off = [PowerState::On, PowerState::Off];
    let mut sent = vec![PowerState::Off];
    for _ in 0..4 {
        sent.extend(on_off);
    }
    assert_eq!(
        commands(&env),
        sent,
        "the off at the start, two cold boots of the first entry and one of each other"
    );
}

#[test]
fn a_bench_fault_ends_the_run_and_the_rest_is_not_run() {
    // A plan of three entries, whose first one's second power on is not
    // confirmed: the relay's contacts stick at the fourth command.
    let dir = scratch("fault-ends-run");
    let plan = three_entries(&dir);
    let mut env = TestEnv::example();
    env.sticking = Some(4);
    let ran = run(&dir, &plan, &examples().join("payload"), &env);
    assert_eq!(ran.status, cli::BENCH_FAULT);
    let receipt = ran.receipt();
    let (pass, fault, not_run) = (Verdict::Pass, Verdict::BenchFault, Verdict::NotRun);
    let expected = [
        (fault, vec![pass, fault]),
        (not_run, vec![]),
        (not_run, vec![]),
    ];
    assert_eq!(verdicts(&receipt), expected);
    assert_eq!(receipt.verdict, Some(fault));
    let faulted = &receipt.entries[0].cold_boots[1];
    let faults = faulted.checks.iter().filter(|c| c.result == Outcome::Fault);
    let faults: Vec<&str> = faults.map(|c| c.rule.as_str()).collect();
    assert_eq!(faults, ["power.on_unconfirmed"]);
    // Nothing of the other entries was recorded: no event, so no switch to
    // their payloads, no check and no cold boot.
    for entry in &receipt.entries[1..] {
        assert!(entry.events.is_empty() && entry.checks.is_empty() && entry.cold_boots.is_empty());
    }
    // No command followed the power off of the faulted cold boot, which
    // readings confirmed.
    let confirmation = &receipt.inputs.instance.parameters.confirmation;
    assert!(power::off_confirmed_at(confirmation, &faulted.events).is_some());
    let on_off = [PowerState::On, PowerState::Off];
    assert_eq!(
        commands(&env),
        [&[PowerState::Off][..], &on_off, &on_off].concat()
    );
}

/// The exit status of `kyto-bench power-off` with the example instance
/// configuration on `env`.
fn power_off(env: &TestEnv) -> u8 {
    let instance = examples().join("instance.toml");
    command(
        env,
        &[
            "power-off".as_ref(),
            "--instance".as_ref(),
            instance.as_ref(),
        ],
    )
}

/// The exit status of `kyto-bench publish` of the entry `entry` of the
/// example plan, instance configuration and payloads on `env`.
fn publish(env: &TestEnv, entry: &str) -> u8 {
    publish_from(env, &examples().join("payload"), entry)
}

/// [`publish`] with the payloads below `payloads`.
fn publish_from(env: &TestEnv, payloads: &Path, entry: &str) -> u8 {
    let examples = examples();
    let (plan, instance) = (
        examples.join("run-plan.toml"),
        examples.join("instance.toml"),
    );
    let args: [&OsStr; 9] = [
        "publish".as_ref(),
        "--plan".as_ref(),
        plan.as_ref(),
        "--instance".as_ref(),
        instance.as_ref(),
        "--payload-root".as_ref(),
        payloads.as_ref(),
        "--entry".as_ref(),
        entry.as_ref(),
    ];
    command(env, &args)
}

/// What the board of `env` is served for `file`.
fn served(env: &TestEnv, file: &str) -> Result<Vec<u8>, DeviceError> {
    SimTftp::new(&env.world).read_served(&PayloadName::parse(file).unwrap())
}

/// What the meter of `world` reads now.
fn supply(world: &SimWorld) -> i64 {
    SimSupplyMeter::new(world).sample().unwrap().micro
}

#[test]
fn the_power_off_command_switches_the_supply_off_and_confirms_it() {
    let env = TestEnv::example();
    SimRelay::new(&env.world).command(PowerState::On).unwrap();
    assert_eq!(power_off(&env), cli::PASS);
    assert_eq!(commands(&env), [PowerState::On, PowerState::Off]);
    assert_eq!(supply(&env.world), 0);
    assert_eq!(env.said(), [""; 0]);
}

#[test]
fn the_power_off_command_without_confirmation_is_a_bench_fault() {
    // A meter that reads the supply on whatever it does.
    let env = TestEnv::example();
    env.world.knobs().meter.stuck_micro = Some(ON_MICRO);
    assert_eq!(power_off(&env), cli::BENCH_FAULT);
    assert_eq!(commands(&env), [PowerState::Off], "one power off, no retry");
    let said = "readings did not confirm the power off within 3000 ms; the last read 5100000 µV";
    assert_eq!(env.said(), [said]);
}

#[test]
fn the_power_off_command_switches_off_when_the_meter_cannot_be_opened() {
    let mut env = TestEnv::example();
    env.no_bridge = true;
    assert_eq!(power_off(&env), cli::BENCH_FAULT);
    assert_eq!(commands(&env), [PowerState::Off], "the power off was sent");
    let said = [
        "the power_confirmation device failed: no MCP2221 USB bridge is connected",
        "readings did not confirm the power off within 3000 ms; no reading was taken",
    ];
    assert_eq!(env.said(), said);
}

#[test]
fn the_power_off_command_finishes_when_interrupted() {
    let mut env = TestEnv::example();
    env.interrupts = Interrupts::AtOpen;
    SimRelay::new(&env.world).command(PowerState::On).unwrap();
    assert_eq!(power_off(&env), cli::PASS);
    assert_eq!(commands(&env), [PowerState::On, PowerState::Off]);
    assert_eq!(supply(&env.world), 0);
    assert_eq!(env.said(), [""; 0]);
}

#[test]
fn the_power_off_command_says_when_a_run_holds_the_bench() {
    let mut env = TestEnv::example();
    env.in_use = true;
    assert_eq!(power_off(&env), cli::PASS);
    assert_eq!(
        commands(&env),
        [PowerState::Off],
        "one power off all the same"
    );
    let said = "a run or publication holds this bench; this power off cuts it, and its receipt \
                does not show this power off and is no evidence";
    assert_eq!(
        env.said_after(),
        [(0, said.into())],
        "said before the power off"
    );
}

#[test]
fn the_publish_command_publishes_one_entry_without_a_power_command() {
    let env = TestEnv::example();
    assert_eq!(publish(&env, "echo"), cli::PASS);
    assert!(env.world.power_commands().is_empty(), "no power command");
    let kernel = example("payload/echo/kernel.img");
    assert_eq!(served(&env, "kernel.img").unwrap(), kernel);
    assert!(env.staged(1, "kernel.img") && !env.staged(2, "kernel.img"));
    assert_eq!(env.said(), [""; 0]);
    // An entry that the plan does not have is an input error.
    let env = TestEnv::example();
    assert_eq!(publish(&env, "absent"), cli::INPUT_ERROR);
    assert_eq!(env.said(), ["the run plan has no entry absent"]);
    assert!(!env.staged(1, "kernel.img"), "nothing staged");
}

#[test]
fn the_publish_command_reports_a_served_copy_that_differs() {
    let env = TestEnv::example();
    env.world.knobs().tftp.corrupts_served = true;
    assert_eq!(publish(&env, "echo"), cli::BENCH_FAULT);
    assert!(env.world.power_commands().is_empty(), "no power command");
    let said = env.said();
    assert_eq!(said.len(), 5, "{said:?}");
    let files = [
        "config.txt",
        "bcm2712-rpi-5-b.dtb",
        "overlays/bcm2712d0.dtbo",
        "kernel.img",
    ];
    for (line, file) in said.iter().zip(files) {
        let copy = format!("the served copy of {file} has the SHA-256 ");
        assert!(line.starts_with(&copy), "{line}");
    }
    let last = "the board is served g000001, whose copy differs from the run plan; a run \
                switches the supply on only for a generation that it published and checked itself";
    assert_eq!(said[4], last);
    // Nothing switched back: the board is served the one generation staged.
    env.world.knobs().tftp.corrupts_served = false;
    let kernel = example("payload/echo/kernel.img");
    assert_eq!(served(&env, "kernel.img").unwrap(), kernel);
    assert!(!env.staged(2, "kernel.img"));
}

#[test]
fn the_publish_command_switches_only_while_readings_confirm_the_supply_off() {
    // A supply that is on: no switch, and no power command.
    let env = TestEnv::example();
    SimRelay::new(&env.world).command(PowerState::On).unwrap();
    assert_eq!(publish(&env, "echo"), cli::REFUSED);
    assert_eq!(commands(&env), [PowerState::On]);
    let none = served(&env, "kernel.img").unwrap_err();
    assert_eq!(none.detail, "no generation is served");
    let said = "refused: readings did not confirm the supply off within 3000 ms, and publish \
                sends no power command; the board is served what it was served before; run \
                power-off first";
    assert_eq!(env.said(), [said]);
    // An interrupt before the switch: nothing is served either.
    let mut env = TestEnv::example();
    env.interrupts = Interrupts::AtOpen;
    assert_eq!(publish(&env, "echo"), cli::INTERRUPTED);
    assert!(env.world.power_commands().is_empty(), "no power command");
    assert!(served(&env, "kernel.img").is_err(), "nothing served");
    let said = "interrupted before the switch; the board is served what it was served before";
    assert_eq!(env.said(), [said]);
    // An interrupt stops it before the switch also when readings would not
    // have confirmed the supply off, or the staged copy differs.
    for supply_on in [true, false] {
        let mut env = TestEnv::example();
        env.interrupts = Interrupts::AtOpen;
        if supply_on {
            SimRelay::new(&env.world).command(PowerState::On).unwrap();
        } else {
            env.world.knobs().tftp.corrupts_staged = true;
        }
        assert_eq!(publish(&env, "echo"), cli::INTERRUPTED);
        assert_eq!(
            commands(&env).len(),
            usize::from(supply_on),
            "no power command"
        );
        assert!(served(&env, "kernel.img").is_err(), "nothing served");
        assert_eq!(env.said(), [said]);
    }
}

#[test]
fn the_publish_command_refuses_a_payload_that_fails_its_checks() {
    let unchanged = "; the board is served what it was served before";
    // A source file of another digest than the plan's: nothing is staged.
    let dir = scratch("publish-source-digest");
    let payloads = payloads_in(&dir);
    fs::write(payloads.join("echo/kernel.img"), b"another kernel").unwrap();
    let env = TestEnv::example();
    assert_eq!(publish_from(&env, &payloads, "echo"), cli::REFUSED);
    let (found, pinned) = (
        Sha256::of(b"another kernel"),
        "854a05365ca392c93250b69848a50e17eb7f4f847f581f1e6d56c6a620b4306e",
    );
    let said = format!(
        "refused by payload.source_digest: the source copy of kernel.img has the SHA-256 \
         {found}, not {pinned}{unchanged}"
    );
    assert_eq!(env.said(), [said]);
    assert!(env.world.power_commands().is_empty(), "no power command");
    assert!(!env.staged(1, "kernel.img"), "nothing staged");
    assert!(served(&env, "kernel.img").is_err(), "nothing served");
    // A staged copy that differs, and a TFTP directory that cannot be
    // staged: nothing is served.
    let staged = "refused by payload.staged_digest: the staged copy of bcm2712-rpi-5-b.dtb";
    let cannot = "staging the payload of echo: the delivery device failed: the simulated TFTP \
                  directory could not stage";
    for (corrupts, said) in [(true, &[][..]), (false, &[cannot][..])] {
        let env = TestEnv::example();
        env.world.knobs().tftp.corrupts_staged = corrupts;
        env.world.knobs().tftp.stage_fails = !corrupts;
        assert_eq!(publish(&env, "echo"), cli::REFUSED);
        let mut found = env.said();
        let last = found.pop().unwrap();
        assert_eq!(found, said);
        assert!(
            last.starts_with(staged) && last.ends_with(unchanged),
            "{last}"
        );
        assert!(env.world.power_commands().is_empty(), "no power command");
        assert!(served(&env, "kernel.img").is_err(), "nothing served");
    }
}

#[test]
fn the_publish_command_is_a_bench_fault_when_the_meter_fails() {
    let mut env = TestEnv::example();
    env.no_bridge = true;
    assert_eq!(publish(&env, "echo"), cli::BENCH_FAULT);
    let said = "the power_confirmation device failed: no MCP2221 USB bridge is connected";
    assert_eq!(env.said(), [said]);
    assert!(env.world.power_commands().is_empty(), "no power command");
    assert!(served(&env, "kernel.img").is_err(), "nothing served");
}

#[test]
fn an_interrupted_run_switches_the_supply_off_and_leaves_no_output() {
    // The first entry captures until its boot timeout of 60 s.
    let dir = scratch("interrupted");
    let plan = plan_with(&dir, |text| {
        let end = "boot_timeout_ms = 60000\nend = { marker = \"started\" }";
        text.replacen(end, "boot_timeout_ms = 60000\nend = \"timeout\"", 1)
    });
    let mut env = TestEnv::example();
    env.interrupts = Interrupts::After(PowerState::On);
    let ran = run(&dir, &plan, &examples().join("payload"), &env);
    assert_eq!(ran.status, cli::INTERRUPTED);
    assert!(!ran.out.exists(), "no receipt and no output directory");
    // The cold boot under way stopped its capture at once and was switched
    // off as usual, and the supply once more.
    let sent = [
        PowerState::Off,
        PowerState::On,
        PowerState::Off,
        PowerState::Off,
    ];
    assert_eq!(commands(&env), sent);
    let at: Vec<u64> = env.world.power_commands().iter().map(|c| c.0.0).collect();
    assert!(at[2] - at[1] < 1000 * MS, "{at:?}");
    assert_eq!(supply(&env.world), 0);
    let said = "the run was interrupted; readings did confirm the power off";
    assert_eq!(env.said(), [said]);
    // An interrupt during the power off before the first entry: no cold
    // boot starts, and the supply is switched off once more.
    let mut env = TestEnv::example();
    env.interrupts = Interrupts::After(PowerState::Off);
    let ran = run_example("interrupted-at-the-start", &env);
    assert_eq!(ran.status, cli::INTERRUPTED);
    assert!(!ran.out.exists(), "no receipt and no output directory");
    assert_eq!(commands(&env), [PowerState::Off; 2]);
    assert_eq!(env.said(), [said]);
    // An interrupt after a power on whose power off sticks: the supply is
    // switched off once more, once.
    let mut env = TestEnv::example();
    (env.interrupts, env.sticking) = (Interrupts::After(PowerState::On), Some(3));
    let ran = run_example("interrupted-and-stuck", &env);
    assert_eq!(ran.status, cli::INTERRUPTED);
    let sent = [
        PowerState::Off,
        PowerState::On,
        PowerState::Off,
        PowerState::Off,
    ];
    assert_eq!(commands(&env), sent);
    assert_eq!(supply(&env.world), 0);
    assert_eq!(env.said(), [said]);
}

#[test]
fn an_interrupt_before_the_first_power_command_sends_none() {
    let mut env = TestEnv::example();
    env.interrupts = Interrupts::AtOpen;
    let ran = run_example("interrupted-before", &env);
    assert_eq!(ran.status, cli::INTERRUPTED);
    assert!(!ran.out.exists(), "no receipt and no output directory");
    assert!(env.world.power_commands().is_empty(), "no power command");
    let said = "the run was interrupted before its first power command";
    assert_eq!(env.said(), [said]);
    // Also when staging then refuses the run.
    let mut env = TestEnv::example();
    env.interrupts = Interrupts::AtOpen;
    env.world.knobs().tftp.corrupts_staged = true;
    let ran = run_example("interrupted-and-refused", &env);
    assert_eq!(ran.status, cli::INTERRUPTED);
    assert!(!ran.out.exists(), "no receipt and no output directory");
    assert!(env.world.power_commands().is_empty(), "no power command");
    assert_eq!(env.said(), [said]);
}

#[test]
fn a_chunk_held_from_before_a_passed_deadline_is_read() {
    // The board answers the step 2 ms after its send, within its timeout of
    // 3 ms, and the console holds the answer until the run reads it 1.5 ms
    // later, after the step's deadline.
    let dir = scratch("held-past-deadline");
    let plan = plan_with(&dir, |text| {
        text.replacen("timeout_ms = 1000", "timeout_ms = 3", 1)
    });
    let mut env = TestEnv::example();
    env.lagging = true;
    let ran = run(&dir, &plan, &examples().join("payload"), &env);
    assert_eq!(ran.status, cli::PASS);
    let receipt = ran.receipt();
    let cold_boot = &receipt.entries[1].cold_boots[0];
    let t = times(&receipt, &cold_boot.events);
    let (sent, received) = t.send.unwrap();
    let deadline = sent + 3 * MS;
    let answer = cold_boot
        .serial
        .index
        .iter()
        .find(|chunk| chunk[0] == received);
    assert_eq!(
        answer,
        Some(&[received, sent + 2 * MS]),
        "stamped before the deadline"
    );
    assert!(t.end.0 > deadline, "read after the deadline");
    assert_eq!(end(&cold_boot.events), Some(EndReason::EndCondition));
}

#[test]
fn a_line_that_came_before_a_send_is_no_response() {
    // The board writes the STATS line just after READY, which arms the
    // step, and answers the send with another line. The console holds the
    // STATS line when the step is armed; the run reads it before it sends.
    let mut env = TestEnv::new(played(None, b"OK\r\n", b"no line is this"));
    env.lagging = true;
    let ran = run_example("line-before-send", &env);
    assert_eq!(ran.status, cli::FAIL);
    let receipt = ran.receipt();
    let cold_boot = &receipt.entries[1].cold_boots[0];
    let bytes = &cold_boot.serial.bytes_base64.0;
    let (_, received) = times(&receipt, &cold_boot.events).send.unwrap();
    let before = &bytes[..usize::try_from(received).unwrap()];
    assert!(
        before.ends_with(b"READY\r\nSTATS seq=1\r\n"),
        "recorded before the send"
    );
    assert!(bytes.ends_with(b"OK\r\n"));
    assert_eq!(end(&cold_boot.events), Some(EndReason::StepTimeout));
    assert_eq!(failed(cold_boot), ["step.response"]);
}

#[test]
fn a_chunk_stamped_after_a_passed_deadline_ends_the_cold_boot() {
    // A boot timeout that passes while the run reads the console late, and
    // a console that then hands a chunk stamped after it.
    let dir = scratch("stamped-past-deadline");
    let plan = plan_with(&dir, |text| {
        let end = "boot_timeout_ms = 60000\nend = { marker = \"started\" }";
        text.replacen(
            end,
            "boot_timeout_ms = 105\nend = { marker = \"started\" }",
            1,
        )
    });
    let mut env = TestEnv::example();
    (env.lagging, env.careless) = (true, true);
    let ran = run(&dir, &plan, &examples().join("payload"), &env);
    assert_eq!(ran.status, cli::FAIL);
    let receipt = ran.receipt();
    let cold_boot = &receipt.entries[0].cold_boots[0];
    let t = times(&receipt, &cold_boot.events);
    let deadline = t.on.0 + 105 * MS;
    assert_eq!(end(&cold_boot.events), Some(EndReason::BootTimeout));
    assert!(t.end.0 > deadline);
    let stamps: Vec<u64> = cold_boot
        .serial
        .index
        .iter()
        .map(|chunk| chunk[1])
        .collect();
    assert!(
        stamps.last().is_some_and(|last| *last < deadline),
        "{stamps:?}"
    );
    assert!(failed(cold_boot).contains(&"boot.end"));
}

#[test]
fn a_step_armed_at_the_boot_deadline_is_sent() {
    // Readings every 1 ms confirm the power on 1 ms after it, and the echo
    // entry's READY line, which arms its step, comes 19 ms after it, at the
    // boot timeout: the step is sent at the deadline, and the cold boot
    // ends there.
    let dir = scratch("armed-at-the-deadline");
    let plan = plan_with(&dir, |text| {
        let end = "boot_timeout_ms = 60000\nend = { marker = \"ready\" }";
        text.replacen(end, "boot_timeout_ms = 19\nend = { marker = \"ready\" }", 1)
    });
    let instance = dir.join("instance.toml");
    let text = String::from_utf8(example("instance.toml")).unwrap();
    fs::write(&instance, text.replacen("poll_ms = 100", "poll_ms = 1", 1)).unwrap();
    let env = TestEnv::example();
    let ran = run_with(&dir, &plan, &instance, &examples().join("payload"), &env);
    assert_eq!(ran.status, cli::FAIL);
    let receipt = ran.receipt();
    let cold_boot = &receipt.entries[1].cold_boots[0];
    let t = times(&receipt, &cold_boot.events);
    let boot = t.on.0 + 19 * MS;
    assert_eq!(t.on_confirmed.0, t.on.0 + MS);
    assert_eq!(
        t.send.map(|send| send.0),
        Some(boot),
        "sent at the deadline"
    );
    assert_eq!(end(&cold_boot.events), Some(EndReason::BootTimeout));
    assert_eq!(t.end.0, boot);
}
