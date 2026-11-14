//! Fuzz tests of the parsing of untrusted input: console transcripts, run
//! plans and receipts. Each target makes a fixed number of cases, written
//! as a constant, from the fixed seed [`SEED`] with a SplitMix64 generator
//! written here, so that every run makes the same cases; a case that fails
//! is named on standard error by the seed and its index. The cases start
//! from seeds: the example console output and run plan of
//! `tools/examples/`, the example lines of the board description, the
//! recordings of `tests/data/recorded/` and the corpus of
//! `tests/data/corpus/`, which holds edge cases of transcripts and run
//! plans. [`fuzz_deeper`], which is ignored, runs a hundred times as many
//! cases of each target:
//!
//! ```sh
//! cargo test --release --locked --manifest-path tools/Cargo.toml --test fuzz -- --ignored
//! ```
//!
//! Every time is one of the cases or of the simulated bench clock; nothing
//! sleeps or reads the system clock.

use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::fs;
use std::net::Ipv6Addr;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use kyto_bench::device::{Console, DeviceError, Nanos, Received};
use kyto_bench::env::{self, Bench, Env};
use kyto_bench::inputs::Inputs;
use kyto_bench::provenance::ToolRevision;
use kyto_bench::run;
use kyto_bench::sim::{Files, Knobs, ON_MICRO, Responder, Script, ScriptProvider, SimWorld};
use kyto_bench_core::board::{Board, LineForm, Method, RASPBERRY_PI_5};
use kyto_bench_core::digest::Sha256;
use kyto_bench_core::instance::{Instance, Parameters, Quantity};
use kyto_bench_core::lines::{self, IndexError, Line};
use kyto_bench_core::names::PayloadName;
use kyto_bench_core::observe::{self, ColdBootInputs, Matcher, Observations, Sent};
use kyto_bench_core::plan::{self, Entry, PlanError, RunPlan};
use kyto_bench_core::receipt::{
    Base64, Check, Command, EndReason, Event, EventKind, Outcome, Receipt, ReceiptError, Refusal,
    Verdict,
};
use kyto_bench_core::rejudge::rejudge;
use kyto_bench_core::verdict::{self, ColdBoot};

/// The seed of every target.
const SEED: u64 = 0x6b79_746f_2d66_757a;

/// The number of cases of each target.
const TRANSCRIPT_CASES: u64 = 1_000;
const PLAN_CASES: u64 = 4_000;
const RECEIPT_CASES: u64 = 100;

/// How many times as many cases [`fuzz_deeper`] runs.
const DEEPER: u64 = 100;

/// The longest transcript that a case of the transcripts makes, in bytes.
const MAX_TRANSCRIPT: usize = 4 << 10;

/// The number of changes made to the receipt of each case of the receipts.
const FORGERIES: usize = 2;

const MS: u64 = 1_000_000;

/// The time at which the console of a cold boot is ready and the supply is
/// switched on.
const ON: u64 = 10_000 * MS;

/// The position among the events of a cold boot of its first send: after
/// the console ready, the power on and the reading that confirms it.
const FIRST_SEND: usize = 3;

/// The directory of the corpus, below `tools/`.
const CORPUS: &str = "kyto-bench/tests/data/corpus";

/// The directory of the recordings, below `tools/`.
const RECORDED: &str = "kyto-bench/tests/data/recorded";

/// The rules that `verdict::judge` gives the checks of a cold boot, in the
/// order in which it gives them, each with the result of a check of it that
/// does not pass.
const COLD_BOOT_RULES: [(&str, Outcome); 23] = [
    ("console.not_ready", Outcome::Fault),
    ("power.on_unconfirmed", Outcome::Fault),
    ("power.off_interval", Outcome::Fault),
    ("power.off_unconfirmed", Outcome::Fault),
    ("power.order", Outcome::Fault),
    ("console.before_power_on", Outcome::Fault),
    ("device.error", Outcome::Fault),
    ("boot.network", Outcome::Fail),
    ("boot.fallback", Outcome::Fail),
    ("boot.end", Outcome::Fail),
    ("console.byte_limit", Outcome::Fail),
    ("marker.count", Outcome::Fail),
    ("marker.order", Outcome::Fail),
    ("marker.forbidden", Outcome::Fail),
    ("marker.before_first_send", Outcome::Fail),
    ("step.declared_none", Outcome::Fail),
    ("step.response", Outcome::Fail),
    ("firmware.version", Outcome::Fail),
    ("segment.digest", Outcome::Fail),
    ("tftp.server", Outcome::Fail),
    ("tftp.prefix", Outcome::Fail),
    ("tftp.unplanned_file", Outcome::Fail),
    ("identity.disagree", Outcome::Fail),
];

/// The keys of a receipt, the first key of every JSON pointer into one.
const RECEIPT_KEYS: [&str; 12] = [
    "format",
    "format_version",
    "nonce",
    "tool",
    "evidence",
    "bench",
    "inputs",
    "reference",
    "refusal",
    "start",
    "entries",
    "verdict",
];

/// A SplitMix64 generator.
struct Rng(u64);

impl Rng {
    /// The generator of case `index` of the target `target`.
    fn case(target: &str, index: u64) -> Self {
        let mut rng = Self(SEED);
        for value in target.bytes().map(u64::from).chain([index]) {
            rng.0 = rng.next() ^ value;
        }
        rng
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number below `bound`, which is not 0.
    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    /// A number in `range`.
    fn within(&mut self, range: RangeInclusive<usize>) -> usize {
        range.start() + self.below(range.end() - range.start() + 1)
    }

    /// Whether an event of `percent` in a hundred happens.
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    /// An item of `items`, which is not empty.
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    /// A byte, often one that ends a line, NUL or one that is not UTF-8 on
    /// its own.
    fn byte(&mut self) -> u8 {
        match self.below(8) {
            0 => b'\r',
            1 => b'\n',
            2 => 0,
            3 => 0x80 | self.next() as u8,
            _ => self.next() as u8,
        }
    }

    /// A position in `bytes`, from 0 to its length.
    fn position(&mut self, bytes: &[u8]) -> usize {
        self.below(bytes.len() + 1)
    }
}

/// Runs the cases `0` to `cases - 1` of the target `target`, each with its
/// own generator. A case that panics is named on standard error by the
/// seed and its index, from which [`Rng::case`] makes it again.
fn fuzz(target: &str, cases: u64, mut case: impl FnMut(&mut Rng)) {
    for index in 0..cases {
        let report = Report(target, index);
        case(&mut Rng::case(target, index));
        drop(report);
    }
}

/// Names a case while the panic of its failure unwinds.
struct Report<'a>(&'a str, u64);

impl Drop for Report<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "{}: case {} of the seed {SEED:#018x} failed",
                self.0, self.1
            );
        }
    }
}

fn tools() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The files of `dir`, below `tools/`, whose names end in `.<extension>`,
/// in order of their names, each with its path below `tools/` and its
/// bytes.
fn files(dir: &str, extension: &str) -> Vec<(String, Vec<u8>)> {
    let entries = fs::read_dir(tools().join(dir)).unwrap();
    let mut paths: Vec<PathBuf> = entries.map(|entry| entry.unwrap().path()).collect();
    paths.retain(|path| path.extension().is_some_and(|found| found == extension));
    paths.sort();
    let read = |path: PathBuf| {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        (format!("{dir}/{name}"), fs::read(&path).unwrap())
    };
    paths.into_iter().map(read).collect()
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap()
}

fn pi5() -> Board {
    Board::parse(RASPBERRY_PI_5).unwrap()
}

/// The example instance configuration.
fn instance_text() -> String {
    text(fs::read(tools().join("examples/instance.toml")).unwrap())
}

/// The example run plan and the run plans of the corpus.
fn seed_plans() -> Vec<String> {
    let example = fs::read(tools().join("examples/run-plan.toml")).unwrap();
    let corpus = files(&format!("{CORPUS}/plans"), "toml");
    let corpus = corpus.into_iter().map(|(_, bytes)| text(bytes));
    [text(example)].into_iter().chain(corpus).collect()
}

/// Every example line of the line forms of `board`.
fn example_lines(board: &Board) -> Vec<Vec<u8>> {
    let mut forms: Vec<&LineForm> = board.markers.iter().map(|line| &line.form).collect();
    forms.push(&board.boot_mode.form);
    forms.extend(&board.tftp.servers);
    forms.extend([&board.tftp.read, &board.tftp.read_failed]);
    forms.extend(board.firmware_versions.iter().map(|line| &line.form));
    forms.extend(board.sensitive.iter().map(|line| &line.form));
    for read in &board.identity {
        if let Method::FirmwareLog { form } | Method::FirmwareLogPresence { form, .. } =
            &read.method
        {
            forms.push(form);
        }
    }
    let examples = forms.into_iter().flat_map(|form| &form.examples);
    examples.map(|line| line.clone().into_bytes()).collect()
}

/// Console transcripts and lines to make cases from.
struct Seeds {
    transcripts: Vec<Vec<u8>>,
    lines: Vec<Vec<u8>>,
}

impl Seeds {
    /// The example console output, the recordings and the transcripts of
    /// the corpus, and their lines, the example lines of the board
    /// description and lines that the run plans name.
    fn new(board: &Board) -> Self {
        let dirs = [
            ("examples/captures".to_string(), "bin"),
            (RECORDED.to_string(), "bin"),
            (format!("{CORPUS}/transcripts"), "bin"),
        ];
        let transcripts: Vec<Vec<u8>> = dirs
            .iter()
            .flat_map(|(dir, extension)| files(dir, extension))
            .map(|(_, bytes)| bytes)
            .collect();
        let mut lines: Vec<Vec<u8>> = transcripts
            .iter()
            .flat_map(|bytes| bytes.split(|byte| *byte == b'\n'))
            .map(|line| line.strip_suffix(b"\r").unwrap_or(line).to_vec())
            .filter(|line| line.len() <= 256)
            .collect();
        lines.extend(example_lines(board));
        let named = [
            "READY",
            "STATS seq=1",
            "STATS seq=2",
            "example kernel started",
            "Example echo payload",
            "BUILD id=0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c",
            "PANIC: example",
        ];
        lines.extend(named.map(|line| line.as_bytes().to_vec()));
        lines.sort();
        lines.dedup();
        Self { transcripts, lines }
    }

    /// A line of the seeds, or now and then bytes of any value.
    fn line(&self, rng: &mut Rng) -> Vec<u8> {
        if rng.chance(10) {
            (0..rng.below(40)).map(|_| rng.byte()).collect()
        } else {
            rng.pick(&self.lines).clone()
        }
    }

    /// A transcript: a run of the lines of `native` or of a seed, or lines
    /// of the seeds with line ends of every kind, changed `0` to `changes`
    /// times, of at most [`MAX_TRANSCRIPT`] bytes.
    fn transcript(&self, rng: &mut Rng, native: &[u8], changes: usize) -> Vec<u8> {
        let mut bytes = match rng.below(10) {
            0..=3 => some_lines(rng, native),
            4..=6 => {
                let seed = rng.pick(&self.transcripts);
                some_lines(rng, seed)
            }
            _ => {
                let mut bytes = Vec::new();
                for _ in 0..rng.below(40) {
                    bytes.extend(self.line(rng));
                    bytes.extend_from_slice(line_end(rng));
                }
                bytes
            }
        };
        for _ in 0..rng.below(changes + 1) {
            self.change(rng, &mut bytes);
        }
        if bytes.len() > MAX_TRANSCRIPT {
            let start = rng.below(bytes.len() - MAX_TRANSCRIPT + 1);
            bytes = bytes[start..start + MAX_TRANSCRIPT].to_vec();
        }
        bytes
    }

    /// Makes one change to `bytes`.
    fn change(&self, rng: &mut Rng, bytes: &mut Vec<u8>) {
        let at = rng.position(bytes);
        match rng.below(9) {
            0 => {
                let inserted: Vec<u8> = (0..rng.within(1..=16)).map(|_| rng.byte()).collect();
                bytes.splice(at..at, inserted);
            }
            1 => {
                let end = (at + rng.within(1..=32)).min(bytes.len());
                bytes.drain(at..end);
            }
            2 => {
                let (from, to) = *rng.pick(&LINE_END_CHANGES);
                *bytes = replace(bytes, from, to);
            }
            3 => {
                let start = line_start(bytes, at);
                let mut line = self.line(rng);
                line.extend_from_slice(line_end(rng));
                bytes.splice(start..start, line);
            }
            4 => {
                let end = (at + rng.within(1..=256)).min(bytes.len());
                let copy = bytes[at..end].to_vec();
                bytes.splice(at..at, copy);
            }
            5 => bytes.truncate(at),
            6 => {
                let fill = if rng.chance(50) { b'x' } else { rng.byte() };
                let long = (0..rng.within(256..=4096)).map(|_| {
                    let byte = if rng.chance(90) { fill } else { rng.byte() };
                    if byte == b'\n' { b' ' } else { byte }
                });
                let long: Vec<u8> = long.collect();
                bytes.splice(at..at, long);
            }
            7 => {
                let feeds = vec![b'\n'; rng.within(1..=500)];
                bytes.splice(at..at, feeds);
            }
            _ => {
                let start = line_start(bytes, at);
                let line = *rng.pick(&STEP_LINES);
                bytes.splice(start..start, line.iter().copied());
            }
        }
    }
}

/// A run of the lines of `bytes`, often from the first or to the last.
fn some_lines(rng: &mut Rng, bytes: &[u8]) -> Vec<u8> {
    let lines: Vec<&[u8]> = bytes.split_inclusive(|byte| *byte == b'\n').collect();
    let from = if rng.chance(50) {
        0
    } else {
        rng.below(lines.len() + 1)
    };
    let to = if rng.chance(50) {
        lines.len()
    } else {
        rng.within(from..=lines.len())
    };
    lines[from..to].concat()
}

/// Line ends of every kind, and none, CR LF the most often.
const LINE_ENDS: [&[u8]; 9] = [
    b"\r\n", b"\r\n", b"\r\n", b"\r\n", b"\n", b"\n", b"\r\r\n", b"\r", b"",
];

/// Changes of every line end of a transcript.
const LINE_END_CHANGES: [(&[u8], &[u8]); 3] = [(b"\r\n", b"\n"), (b"\n", b"\r\n"), (b"\n", b"\r")];

/// Lines that arm and answer steps.
const STEP_LINES: [&[u8]; 3] = [b"READY\r\n", b"STATS seq=1\r\n", b"STATS seq=2\n"];

/// A line end of any kind, or none.
fn line_end(rng: &mut Rng) -> &'static [u8] {
    rng.pick(&LINE_ENDS)
}

/// The offset of the start of the line of `bytes` that holds `at`.
fn line_start(bytes: &[u8], at: usize) -> usize {
    bytes[..at]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |lf| lf + 1)
}

/// `bytes` with every `from` replaced by `to`.
fn replace(bytes: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while !rest.is_empty() {
        if rest.starts_with(from) {
            result.extend_from_slice(to);
            rest = &rest[from.len()..];
        } else {
            result.push(rest[0]);
            rest = &rest[1..];
        }
    }
    result
}

/// The ends of the chunks in which `bytes` arrive: single bytes, a few
/// bytes, whole lines or longer pieces, so that a CR and its LF often
/// arrive apart.
fn chunk_ends(rng: &mut Rng, bytes: &[u8]) -> Vec<usize> {
    let (mut ends, mut at) = (Vec::new(), 0);
    while at < bytes.len() {
        let size = match rng.below(4) {
            0 => rng.within(1..=4),
            1 => rng.within(1..=64),
            2 => bytes[at..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len() - at, |lf| lf + 1),
            _ => rng.within(1..=512),
        };
        at = (at + size).min(bytes.len());
        ends.push(at);
    }
    ends
}

/// The times of `count` chunks after the power on at [`ON`], some of them
/// equal.
fn chunk_times(rng: &mut Rng, count: usize) -> Vec<u64> {
    let mut at = ON + MS;
    let mut times = Vec::with_capacity(count);
    for _ in 0..count {
        at += rng.below(20) as u64 * MS;
        times.push(at);
    }
    times
}

/// An entry of a run plan, the instance parameters of a cold boot of it
/// and the example console output of its payload, without the lines that
/// answer its steps.
struct Context {
    board: Board,
    entry: Entry,
    parameters: Parameters,
    native: Vec<u8>,
}

impl Context {
    /// Every entry of the seed plans, each with the parameters of the
    /// example instance configuration and with a TFTP path prefix that the
    /// board's lines rarely have.
    fn all(board: &Board) -> Vec<Self> {
        let instance = Instance::parse(&instance_text()).unwrap();
        let mut prefixed = instance.parameters().clone();
        prefixed.tftp.path_prefix = "bench/".into();
        let captures = files("examples/captures", "bin");
        let mut contexts = Vec::new();
        for text in seed_plans() {
            let plan = RunPlan::parse(&text).unwrap();
            plan.check(board, &instance.parameters().pins).unwrap();
            for entry in &plan.entries {
                let name = format!("/{}.bin", entry.payload_dir.as_str());
                let capture = captures.iter().find(|(path, _)| path.ends_with(&name));
                let mut native = capture.unwrap().1.clone();
                if !entry.steps().is_empty() {
                    native = replace(&native, b"STATS seq=1\r\n", b"");
                }
                for parameters in [instance.parameters(), &prefixed] {
                    contexts.push(Self {
                        board: board.clone(),
                        entry: entry.clone(),
                        parameters: parameters.clone(),
                        native: native.clone(),
                    });
                }
            }
        }
        contexts
    }

    fn payload(&self) -> &[kyto_bench_core::instance::PinnedFile] {
        self.entry.files(&self.parameters.pins).unwrap_or_default()
    }

    fn matcher(&self) -> Matcher<'_> {
        Matcher::new(
            &self.board,
            &self.entry,
            &self.parameters.tftp,
            self.payload(),
        )
    }

    /// The observations of the record of a cold boot.
    fn observe(&self, transcript: &[u8], index: &[[u64; 2]], sends: &[Sent]) -> Observations {
        let inputs = ColdBootInputs {
            board: &self.board,
            entry: &self.entry,
            tftp: &self.parameters.tftp,
            payload: self.payload(),
            transcript,
            index,
            sends,
        };
        observe::observe(&inputs).unwrap()
    }
}

/// What a run records of a cold boot: the bytes it received, their chunks
/// and its sends, with the observations of the matcher that read them as
/// they arrived.
struct Online {
    transcript: Vec<u8>,
    index: Vec<[u64; 2]>,
    sends: Vec<Sent>,
    seen: Observations,
    ended: bool,
}

/// Lines with which a board answers steps.
const ANSWERS: [&str; 3] = ["STATS seq=1", "STATS seq=2", "READY"];

/// Reads `chunks`, each with its time, as a run does: after each chunk, it
/// sends the step that is armed, and it stops once the cold boot has ended
/// or a chunk would come after the deadline of the step that waits for its
/// response or the boot timeout. The board answers most sends at once,
/// in one or two chunks, with the first of [`ANSWERS`] that the step
/// expects.
fn online(rng: &mut Rng, context: &Context, chunks: Vec<(u64, Vec<u8>)>) -> Online {
    let mut matcher = context.matcher();
    let boot = ON.saturating_add(context.entry.boot_timeout_ms.saturating_mul(MS));
    let (mut transcript, mut index, mut sends) = (Vec::new(), Vec::new(), Vec::new());
    let mut chunks = VecDeque::from(chunks);
    while let Some((at, bytes)) = chunks.pop_front() {
        let deadline = matcher.deadline().map_or(boot, |step| step.min(boot));
        if matcher.ended() || at > deadline {
            break;
        }
        index.push([transcript.len() as u64, at]);
        transcript.extend_from_slice(&bytes);
        matcher.push(at, &bytes);
        if let Some(step) = matcher.armed() {
            // A step is sent once; a step armed again would be sent, and
            // answered, without end.
            assert!(
                sends.iter().all(|sent: &Sent| sent.step != step),
                "step {step} armed again after its send"
            );
            let event = FIRST_SEND + sends.len();
            let received = matcher.received();
            sends.push(Sent {
                step,
                event,
                at_ns: at,
                received,
            });
            matcher.sent(step, event, at);
            let expect = &context.entry.steps()[step].expect;
            let answer = ANSWERS.iter().find(|line| expect.is_match(line.as_bytes()));
            if let Some(answer) = answer.filter(|_| rng.chance(80)) {
                let mut answer = answer.as_bytes().to_vec();
                answer.extend_from_slice(line_end(rng));
                let cut = rng.within(1..=answer.len());
                chunks.push_front((at, answer.split_off(cut)));
                chunks.push_front((at, answer));
                chunks.retain(|(_, bytes)| !bytes.is_empty());
            }
        }
    }
    let ended = matcher.ended();
    Online {
        transcript,
        index,
        sends,
        seen: matcher.finish(),
        ended,
    }
}

fn event(at_ns: u64, received: u64, event: EventKind) -> Event {
    Event {
        at_ns,
        received,
        event,
    }
}

/// The events of a cold boot of `entry` whose console was ready and supply
/// switched on at [`ON`], confirmed 1 ms later, with the sends of
/// `online`, which ended for `reason` once its last chunk was read and was
/// switched off and confirmed off then.
fn events(entry: &Entry, online: &Online, reason: EndReason) -> Vec<Event> {
    let reading = |micro| EventKind::Reading {
        quantity: Quantity::SupplyVoltage,
        micro,
    };
    let power = |command| EventKind::Power {
        command,
        relay: None,
    };
    let mut events = vec![
        event(ON, 0, EventKind::ConsoleReady {}),
        event(ON, 0, power(Command::On)),
        event(ON + MS, 0, reading(5_100_000)),
    ];
    for send in &online.sends {
        let bytes_base64 = Base64(entry.steps()[send.step].send.clone().into_bytes());
        let step = send.step;
        let kind = EventKind::Send { step, bytes_base64 };
        events.push(event(send.at_ns, send.received, kind));
    }
    let last = online.index.last().map_or(ON + MS, |chunk| chunk[1]);
    let received = online.transcript.len() as u64;
    events.extend([
        event(last, received, EventKind::End { reason }),
        event(last, received, power(Command::Off)),
        event(last + 100 * MS, received, reading(0)),
        event(last + 100 * MS, received, EventKind::ConsoleClosed {}),
    ]);
    events
}

/// Asserts that `lines` are the lines of `transcript`: from its start, each
/// starts where the one before ended, holds the bytes of its content, and,
/// when an LF ends it, ends after that LF and one CR before it, which its
/// content does not keep; only the last line is not ended by an LF, and it
/// ends with the transcript.
#[track_caller]
fn assert_lines(transcript: &[u8], lines: &[Line]) {
    let mut at = 0;
    for (number, line) in lines.iter().enumerate() {
        assert_eq!(line.offset, at as u64, "line {number}");
        let end = at + line.content.len();
        assert_eq!(&transcript[at..end], &line.content[..], "line {number}");
        assert!(!line.content.contains(&b'\n'));
        if line.terminated {
            let ending: &[u8] = if transcript[end] == b'\r' {
                b"\r\n"
            } else {
                b"\n"
            };
            at = end + ending.len();
            assert_eq!(&transcript[end..at], ending, "line {number}");
            let cr = line.content.last() == Some(&b'\r');
            assert!(
                ending == b"\r\n" || !cr,
                "line {number} keeps the CR before its LF"
            );
        } else {
            assert!(number == lines.len() - 1 && !line.content.is_empty());
            at = end;
        }
    }
    assert_eq!(at, transcript.len());
    let feeds = transcript.iter().filter(|byte| **byte == b'\n').count();
    assert_eq!(lines.iter().filter(|line| line.terminated).count(), feeds);
}

/// Asserts that the markers and steps of `seen` are those that the rules
/// give, found here from `lines` and `sends` without a matcher: each line,
/// in order, shows the markers of the board whose forms match its content,
/// each as `board.<name>`, then those of `entry`. Only complete lines arm
/// and answer steps: the first step is armed by a line of its `after`
/// marker, and each later one by such a line from the response of the step
/// before it on; the response of a step that was sent is the first line
/// that starts at or after the bytes received before the send and matches
/// what the step expects.
#[track_caller]
fn assert_matched(
    board: &Board,
    entry: &Entry,
    lines: &[Line],
    sends: &[Sent],
    seen: &Observations,
) {
    let mut markers = Vec::new();
    for line in lines {
        let content = &line.content[..];
        let forms = board.markers.iter();
        let boards = forms.filter(|marker| marker.form.pattern.is_match(content));
        let own = entry
            .markers
            .iter()
            .filter(|marker| marker.line.is_match(content));
        let names = boards
            .map(|marker| format!("board.{}", marker.name))
            .chain(own.map(|marker| marker.name.clone()));
        markers.extend(names.map(|name| (name, line.offset)));
    }
    let found: Vec<(String, u64)> = seen
        .markers
        .iter()
        .map(|marker| (marker.name.clone(), marker.line))
        .collect();
    assert_eq!(found, markers, "markers");
    let steps = entry.steps();
    let mut expected: Vec<(usize, u64, Option<usize>, Option<u64>)> = Vec::new();
    let send_of = |step: usize| sends.iter().find(|send| send.step == step);
    for line in lines.iter().filter(|line| line.terminated) {
        if let Some((step, _, _, response @ None)) = expected.last_mut() {
            let after_send = send_of(*step).is_some_and(|send| line.offset >= send.received);
            if after_send && steps[*step].expect.is_match(&line.content) {
                *response = Some(line.offset);
            }
        }
        let ready = expected
            .last()
            .is_none_or(|(.., response)| response.is_some());
        let next = expected.len();
        let mut shown = markers.iter().filter(|(_, at)| *at == line.offset);
        if ready
            && steps
                .get(next)
                .is_some_and(|step| shown.any(|(name, _)| *name == step.after))
        {
            let event = send_of(next).map(|send| send.event);
            expected.push((next, line.offset, event, None));
        }
    }
    let found: Vec<_> = seen
        .steps
        .iter()
        .map(|step| {
            (
                step.step,
                step.trigger_line,
                step.send_event,
                step.response_line,
            )
        })
        .collect();
    assert_eq!(found, expected, "steps");
}

/// Asserts that every segment of `seen` is the SHA-256 of the bytes of
/// `transcript` from the end of the first line of its `from` marker to the
/// start of the first line of its `to` marker after it, and that a segment
/// of the entry is seen exactly when such lines are.
#[track_caller]
fn assert_segments(entry: &Entry, transcript: &[u8], lines: &[Line], seen: &Observations) {
    let end_of = |offset: u64| {
        let at = lines.iter().position(|line| line.offset == offset).unwrap();
        lines
            .get(at + 1)
            .map_or(transcript.len() as u64, |next| next.offset)
    };
    for segment in &entry.segments {
        let first = |name: &str, after: u64| {
            let mut markers = seen.markers.iter();
            markers
                .find(|m| m.name == name && m.line >= after)
                .map(|m| m.line)
        };
        let from = first(&segment.from, 0);
        let bounds = from.and_then(|from| {
            let start = end_of(from);
            Some((from, start, first(&segment.to, start)?))
        });
        let found = seen.segments.iter().find(|seen| seen.name == segment.name);
        match (bounds, found) {
            (None, None) => {}
            (Some((from, start, to)), Some(found)) => {
                assert_eq!((found.from_line, found.to_line), (from, to));
                let bytes = &transcript[start as usize..to as usize];
                assert_eq!(found.sha256, Sha256::of(bytes), "{}", segment.name);
            }
            (bounds, found) => panic!("{}: {bounds:?} {found:?}", segment.name),
        }
    }
}

/// Asserts that what `seen` holds is bounded by the lines of the cold boot
/// and the items of the board description and of `entry`.
#[track_caller]
fn assert_bounded(board: &Board, entry: &Entry, lines: usize, seen: &Observations) {
    let markers = board.markers.len() + entry.markers.len();
    assert!(seen.markers.len() <= lines * markers);
    assert!(seen.boot_modes.len() <= lines);
    assert!(seen.firmware.len() <= lines * board.firmware_versions.len());
    for agreement in &seen.identity {
        assert!(agreement.lines.len() <= lines);
        assert!(agreement.values.len() <= lines * board.identity.len());
    }
    assert!(seen.values.len() <= entry.values.len());
    for agreement in &seen.values {
        assert!(agreement.lines.len() <= lines && agreement.values.len() <= lines);
    }
    assert!(seen.tftp.servers.len() <= lines && seen.tftp.reads.len() <= lines);
    assert!(seen.boot_files.len() <= seen.tftp.reads.len());
    assert!(seen.segments.len() <= entry.segments.len());
    assert!(seen.steps.len() <= entry.steps().len());
}

/// Asserts that every check of `checks` is of a rule of a cold boot, in
/// the order of [`COLD_BOOT_RULES`], with its result, and with a detail
/// exactly when it did not pass.
#[track_caller]
fn assert_rules(checks: &[Check]) {
    let mut last = 0;
    for check in checks {
        let rule = &check.rule;
        let known = COLD_BOOT_RULES.iter().position(|(known, _)| known == rule);
        let position = known.unwrap_or_else(|| panic!("{rule} is not a rule of a cold boot"));
        assert!(position >= last, "{rule} is out of order");
        last = position;
        let failed = COLD_BOOT_RULES[position].1;
        assert!([Outcome::Pass, failed].contains(&check.result), "{rule}");
        assert_eq!(check.detail.is_some(), check.result != Outcome::Pass);
    }
}

/// `seen` without the times of its markers, the only times it holds.
fn untimed(mut seen: Observations) -> Observations {
    for marker in &mut seen.markers {
        marker.at_ns = 0;
    }
    seen
}

/// An index of `length` bytes with all times 0 and chunks that start at
/// the bytes received before each of `sends` and at other offsets.
fn rechunked(rng: &mut Rng, length: usize, sends: &[Sent]) -> Vec<[u64; 2]> {
    let mut starts: BTreeSet<u64> = sends.iter().map(|send| send.received).collect();
    for _ in 0..rng.below(length / 8 + 2) {
        starts.insert(rng.below(length.max(1)) as u64);
    }
    if length > 0 {
        starts.insert(0);
    }
    starts.retain(|start| *start < length as u64);
    starts.into_iter().map(|start| [start, 0]).collect()
}

/// An index of `length` bytes: `index` with an entry removed, repeated or
/// moved by a byte or a time made a nanosecond earlier, or chunks of any
/// offsets and times, mostly near the length.
fn any_index(rng: &mut Rng, index: &[[u64; 2]], length: usize) -> Vec<[u64; 2]> {
    if !index.is_empty() && rng.chance(50) {
        let (mut changed, at) = (index.to_vec(), rng.below(index.len()));
        match rng.below(5) {
            0 => {
                changed.remove(at);
            }
            1 => changed.insert(at, changed[at]),
            2 => changed[at][0] += 1,
            3 => changed[at][0] = changed[at][0].saturating_sub(1),
            _ => changed[at][1] -= 1,
        }
        return changed;
    }
    let entry = |rng: &mut Rng| {
        let offset = match rng.below(4) {
            0 => rng.next(),
            _ => rng.below(length + 3) as u64,
        };
        [offset, rng.below(4) as u64]
    };
    let mut index: Vec<[u64; 2]> = (0..rng.below(8)).map(|_| entry(rng)).collect();
    if rng.chance(50) {
        index.sort_unstable();
    }
    index
}

/// What the cases of the transcripts showed, to check that they cover
/// what the properties are about.
#[derive(Default)]
struct TranscriptCover {
    sent: u64,
    answered: u64,
    ended: u64,
    limited: u64,
    segments: u64,
    split_line_end: u64,
    not_utf8: u64,
    indexes: BTreeSet<&'static str>,
}

/// One case of the transcripts.
fn transcript_case(
    rng: &mut Rng,
    seeds: &Seeds,
    contexts: &[Context],
    cover: &mut TranscriptCover,
) {
    let context = rng.pick(contexts);
    let (board, entry) = (&context.board, &context.entry);
    let bytes = seeds.transcript(rng, &context.native, 4);
    let ends = chunk_ends(rng, &bytes);
    let times = chunk_times(rng, ends.len());
    let starts = [0].into_iter().chain(ends.iter().copied());
    let pieces = starts
        .zip(&ends)
        .map(|(start, &end)| bytes[start..end].to_vec());
    let run = online(rng, context, times.into_iter().zip(pieces).collect());
    let (transcript, index) = (&run.transcript[..], &run.index[..]);
    // The index of the record describes the bytes received.
    let chunks = lines::chunks(transcript, index).unwrap();
    let joined: Vec<u8> = chunks
        .iter()
        .flat_map(|(_, bytes)| bytes.to_vec())
        .collect();
    assert_eq!(joined, transcript);
    // Its lines, put back together with their line ends, are the bytes.
    let lines = lines::split(chunks.iter().copied());
    assert_lines(transcript, &lines);
    // The run ended at its end marker or at the boot timeout; now and then
    // its record says that it ended for another reason: a step timeout, a
    // limit of the capture or a device error.
    let reason = match (rng.chance(80), run.ended) {
        (true, true) => EndReason::EndCondition,
        (true, false) => EndReason::BootTimeout,
        (false, _) => *rng.pick(&[
            EndReason::StepTimeout,
            EndReason::ByteLimit,
            EndReason::ChunkLimit,
            EndReason::DeviceError,
        ]),
    };
    // The rules read the record as the run read the bytes: the
    // observations are those of the matcher, and the checks are of the
    // rules of a cold boot, in their order, the same in every reading.
    let events = events(entry, &run, reason);
    let cold_boot = ColdBoot {
        board,
        entry,
        parameters: &context.parameters,
        payload: context.payload(),
        transcript,
        index,
        events: &events,
        off_at: Some(ON - 5_000 * MS),
    };
    let (seen, checks) = verdict::judge(&cold_boot).unwrap();
    assert_eq!(seen, run.seen, "online and offline");
    assert_rules(&checks);
    // The capture limit fails exactly when the capture ended at a limit.
    let limit = checks
        .iter()
        .find(|check| check.rule == "console.byte_limit");
    let limited = [EndReason::ByteLimit, EndReason::ChunkLimit].contains(&reason);
    let failed = if limited {
        Outcome::Fail
    } else {
        Outcome::Pass
    };
    assert_eq!(limit.map(|check| check.result), Some(failed), "{reason:?}");
    cover.limited += u64::from(limited);
    let result = verdict::cold_boot(&checks);
    assert!([Verdict::Pass, Verdict::Fail, Verdict::BenchFault].contains(&result));
    assert_eq!(verdict::judge(&cold_boot), Ok((seen.clone(), checks)));
    // Other chunks give the same observations, but for the times of the
    // markers, while every send comes where it came.
    let other = rechunked(rng, transcript.len(), &run.sends);
    let again = context.observe(transcript, &other, &run.sends);
    assert_eq!(untimed(again), untimed(seen.clone()), "chunks: {other:?}");
    assert_matched(board, entry, &lines, &run.sends, &seen);
    assert_segments(entry, transcript, &lines, &seen);
    assert_bounded(board, entry, lines.len(), &seen);
    // An index of any offsets and times describes the bytes when it keeps
    // the rules of an index, and is refused when it does not: it is empty
    // only for no bytes, starts at offset 0, its offsets increase and stay
    // below the length, and its times do not decrease.
    let any = any_index(rng, index, transcript.len());
    let length = transcript.len() as u64;
    let valid = any.first().map_or(length == 0, |first| first[0] == 0)
        && any.iter().all(|chunk| chunk[0] < length)
        && any
            .windows(2)
            .all(|pair| pair[0][0] < pair[1][0] && pair[0][1] <= pair[1][1]);
    let read = lines::chunks(transcript, &any);
    assert_eq!(read.is_ok(), valid, "{any:?}: {read:?}");
    if let Ok(chunks) = &read {
        let joined: Vec<u8> = chunks
            .iter()
            .flat_map(|(_, bytes)| bytes.to_vec())
            .collect();
        assert_eq!(joined, transcript);
    }
    cover.indexes.insert(match &read {
        Ok(_) => "read",
        Err(IndexError::NotFromStart) => "not from the start",
        Err(IndexError::OffsetNotIncreasing { .. }) => "an offset not increasing",
        Err(IndexError::OffsetPastEnd { .. }) => "an offset past the end",
        Err(IndexError::TimeDecreasing { .. }) => "a time decreasing",
    });
    // Events in any order, with any times and sends of any step, are
    // judged by the same rules.
    if rng.chance(20) {
        let mut events = events.clone();
        for _ in 0..rng.within(1..=4) {
            let at = rng.below(events.len());
            match rng.below(4) {
                0 => {
                    events.remove(at);
                }
                1 => {
                    let other = rng.below(events.len());
                    events.swap(at, other);
                }
                2 => events[at].received = rng.below(transcript.len() + 2) as u64,
                _ => {
                    let step = rng.below(4);
                    let bytes_base64 = Base64(vec![rng.byte()]);
                    let kind = EventKind::Send { step, bytes_base64 };
                    events.insert(at, event(rng.next() % (ON * 2), events[at].received, kind));
                }
            }
            if events.is_empty() {
                break;
            }
        }
        let judged = verdict::judge(&ColdBoot {
            events: &events,
            ..cold_boot
        });
        assert_rules(&judged.unwrap().1);
    }
    cover.sent += u64::from(!run.sends.is_empty());
    cover.answered += u64::from(seen.steps.iter().any(|step| step.response_line.is_some()));
    cover.ended += u64::from(run.ended);
    cover.segments += u64::from(!seen.segments.is_empty());
    let split = index.windows(2).any(|pair| {
        let at = pair[1][0] as usize;
        transcript[at - 1] == b'\r' && transcript[at] == b'\n'
    });
    cover.split_line_end += u64::from(split);
    cover.not_utf8 += u64::from(std::str::from_utf8(transcript).is_err());
}

/// The transcripts: changed example console output, recordings, corpus files
/// and lines of the seeds with line ends of every kind, read in chunks of any
/// size by a run that sends each armed step after a chunk, once, and whose
/// board answers most sends. The record of what the run read gives the
/// observations of its matcher, whose markers and steps are those that the
/// rules give from the lines and the sends; other chunks that keep every send
/// where it came give the same, but for the times of markers; the lines put
/// back together are the bytes; segments are digests of the raw bytes; what a
/// cold boot shows is bounded by its lines; the checks of a cold boot are of
/// its rules, in their order, the same in every reading, also of events in any
/// order, and the capture limit fails exactly when the record ends the cold
/// boot at a limit; and an index of any offsets and times describes the bytes
/// exactly when it keeps the rules of an index.
fn transcripts(cases: u64) {
    let board = pi5();
    let (seeds, contexts) = (Seeds::new(&board), Context::all(&board));
    // The rules of a cold boot are rules of the contract.
    let contract = include_str!("../../../docs/bench/contract.md");
    for (rule, _) in COLD_BOOT_RULES {
        assert!(contract.contains(&format!("| `{rule}` |")), "{rule}");
    }
    let mut cover = TranscriptCover::default();
    fuzz("transcripts", cases, |rng| {
        transcript_case(rng, &seeds, &contexts, &mut cover);
    });
    let counts = [
        cover.sent,
        cover.answered,
        cover.ended,
        cover.limited,
        cover.segments,
        cover.split_line_end,
        cover.not_utf8,
    ];
    assert!(counts.iter().all(|count| *count > cases / 50), "{counts:?}");
    // Indexes were read, and refused under each rule.
    assert_eq!(cover.indexes.len(), 5, "{:?}", cover.indexes);
}

#[test]
fn fuzz_transcripts() {
    transcripts(TRANSCRIPT_CASES);
}

/// Lines that run plans can have, many of them breaking a rule.
const PLAN_LINES: [&str; 32] = [
    "sends = false",
    "steps = []",
    "end = \"timeout\"",
    "end = { marker = \"board.nothing\" }",
    "end = { marker = \"ready\", other = 1 }",
    "max_capture_bytes = 0",
    "max_capture_bytes = 16777217",
    "cold_boots = 0",
    "cold_boots = 101",
    "boot_timeout_ms = 3600001",
    "payload_pin = \"self-test\"",
    "payload = []",
    "reference = \"a\\u0007\"",
    "[entries.writes]",
    "eeprom = true",
    "[[entries]]",
    "name = \"echo\"",
    "name = \"Upper\"",
    "markers = [{ name = \"m\", line = '(' }]",
    "markers = [{ name = \"m\", line = 'a{1000}{1000}' }]",
    "expect = [{ marker = \"ready\", min = 2, max = 1 }]",
    "order = [{ markers = [\"ready\"] }]",
    "forbid = [{ line = '^x', from = \"nowhere\" }]",
    "values = [{ field = \"v\", line = 'no group' }]",
    "segments = [{ name = \"s\", from = \"ready\", to = \"ready\", sha256 = \"00\" }]",
    "steps = [{ after = \"ready\", send = \"\", expect = 'x', timeout_ms = 0 }]",
    "before_first_send = [\"board.os_start\"]",
    "[entries.firmware.bl32]",
    "hash = \"\"",
    "format_version = 2",
    "board = \"synthetic-board\"",
    "x = 1",
];

/// Numbers at and beyond the limits of run plans and of TOML.
const NUMBERS: [&str; 14] = [
    "0",
    "1",
    "-1",
    "2",
    "100",
    "101",
    "1000",
    "4096",
    "3600000",
    "3600001",
    "16777216",
    "4294967296",
    "18446744073709551615",
    "1e3",
];

/// Strings that names, markers, patterns and digests can be given.
const STRINGS: [&str; 14] = [
    "",
    "timeout",
    "ready",
    "board.network_boot",
    "board.",
    "Upper",
    "a b",
    "(",
    "(?P<value>x)",
    "\\\\",
    "\\u0000",
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    "../x",
    "\u{e9}",
];

/// Characters that end or start the parts of TOML documents.
const CHARACTERS: [char; 16] = [
    '"', '\'', '[', ']', '{', '}', '=', ',', '.', '\n', '#', '\\', ' ', '\u{e9}', '\u{7f}', '0',
];

/// The positions in `text` at which a character starts, and its end.
fn boundaries(text: &str) -> Vec<usize> {
    let starts = text.char_indices().map(|(at, _)| at);
    starts.chain([text.len()]).collect()
}

/// The keys of run plans whose values have limits.
const LIMITED_KEYS: [&str; 6] = [
    "cold_boots",
    "boot_timeout_ms",
    "max_capture_bytes",
    "timeout_ms",
    "send",
    "reference",
];

/// Values of `key`, one of [`LIMITED_KEYS`], at and just past its limits,
/// each with whether a run plan allows it.
fn limit_values(key: &str) -> Vec<(String, bool)> {
    let numbers = |values: [(u64, bool); 4]| {
        let values = values.map(|(value, allowed)| (value.to_string(), allowed));
        values.to_vec()
    };
    let quoted = |length: usize, allowed| (format!("\"{}\"", "x".repeat(length)), allowed);
    match key {
        "cold_boots" => numbers([(0, false), (1, true), (100, true), (101, false)]),
        "boot_timeout_ms" | "timeout_ms" => {
            numbers([(0, false), (1, true), (3_600_000, true), (3_600_001, false)])
        }
        "max_capture_bytes" => {
            let max = plan::MAX_CAPTURE_BYTES;
            numbers([(0, false), (1, true), (max, true), (max + 1, false)])
        }
        "send" => [(0, false), (1, true), (4096, true), (4097, false)]
            .map(|(length, allowed)| quoted(length, allowed))
            .to_vec(),
        _ => vec![
            quoted(0, true),
            quoted(1024, true),
            quoted(1025, false),
            ("\"a\\u0007\"".into(), false),
        ],
    }
}

/// The values of the keys of [`LIMITED_KEYS`] in `lines`, outside
/// comments: for each, its line, its key and the span of its value.
fn limited_values(lines: &[String]) -> Vec<(usize, &'static str, usize, usize)> {
    let mut found = Vec::new();
    for (number, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with('#') {
            continue;
        }
        for key in LIMITED_KEYS {
            let pattern = format!("{key} = ");
            for (at, _) in line.match_indices(&pattern) {
                let before = line[..at].chars().next_back();
                if before.is_some_and(|c| !matches!(c, ' ' | '{' | ',')) {
                    continue;
                }
                let start = at + pattern.len();
                if let Some(end) = value_end(&line[start..]) {
                    found.push((number, key, start, start + end));
                }
            }
        }
    }
    found
}

/// The length of the value at the start of `rest`: a string between `"`,
/// or what comes before a space, `,` or `}`.
fn value_end(rest: &str) -> Option<usize> {
    if let Some(string) = rest.strip_prefix('"') {
        let mut escaped = false;
        for (at, c) in string.char_indices() {
            match c {
                '"' if !escaped => return Some(at + 2),
                '\\' => escaped = !escaped,
                _ => escaped = false,
            }
        }
        return None;
    }
    let end = rest.find([' ', ',', '}']).unwrap_or(rest.len());
    (end > 0).then_some(end)
}

/// Makes one change to `text`, a run plan, with `seeds` to take lines from.
/// When it sets a value at or just past a limit of a run plan, it says
/// whether a run plan allows that value.
fn change_plan(rng: &mut Rng, text: &mut String, seeds: &[String]) -> Option<bool> {
    let mut lines: Vec<String> = text.split('\n').map(String::from).collect();
    let line = rng.below(lines.len());
    let mut allowed = None;
    match rng.below(12) {
        10 | 11 => {
            let values = limited_values(&lines);
            if !values.is_empty() {
                let (line, key, start, end) = *rng.pick(&values);
                let (value, within) = rng.pick(&limit_values(key)).clone();
                lines[line].replace_range(start..end, &value);
                allowed = Some(within);
            }
        }
        0 => {
            lines.remove(line);
        }
        1 => lines.insert(line, lines[line].clone()),
        2 => {
            let other = rng.below(lines.len());
            lines.swap(line, other);
        }
        3 => {
            let seed: Vec<&str> = rng.pick(seeds).split('\n').collect();
            lines.insert(line, rng.pick(&seed).to_string());
        }
        4 => lines.insert(line, rng.pick(&PLAN_LINES).to_string()),
        5 => {
            let digits = runs(&lines[line], |c| c.is_ascii_digit());
            if !digits.is_empty() {
                let (start, end) = *rng.pick(&digits);
                lines[line].replace_range(start..end, rng.pick(&NUMBERS));
            }
        }
        6 => {
            let quoted = quoted(&lines[line]);
            if !quoted.is_empty() {
                let (start, end) = *rng.pick(&quoted);
                lines[line].replace_range(start..end, rng.pick(&STRINGS));
            }
        }
        7 => {
            let at = *rng.pick(&boundaries(&lines[line]));
            lines[line].insert(at, *rng.pick(&CHARACTERS));
        }
        8 => {
            let at = boundaries(&lines[line]);
            let start = rng.below(at.len());
            let end = rng.within(start..=(start + 8).min(at.len() - 1));
            lines[line].replace_range(at[start]..at[end], "");
        }
        _ => {
            let at = boundaries(&lines[line]);
            lines.truncate(line + 1);
            lines[line].truncate(*rng.pick(&at));
        }
    }
    *text = lines.join("\n");
    allowed
}

/// The spans of the runs of characters of `class` in `line`.
fn runs(line: &str, class: impl Fn(char) -> bool) -> Vec<(usize, usize)> {
    let (mut spans, mut start) = (Vec::new(), None);
    for (at, c) in line.char_indices().chain([(line.len(), '\n')]) {
        match (class(c), start) {
            (true, None) => start = Some(at),
            (false, Some(from)) => {
                spans.push((from, at));
                start = None;
            }
            _ => {}
        }
    }
    spans
}

/// The spans of the contents of the strings between `"` or `'` in `line`.
fn quoted(line: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    for quote in ['"', '\''] {
        let at: Vec<usize> = line.match_indices(quote).map(|(at, _)| at).collect();
        spans.extend(
            at.as_chunks::<2>()
                .0
                .iter()
                .map(|[open, close]| (open + 1, *close)),
        );
    }
    spans
}

/// Asserts the limits of a run plan that was read, apart from its reader.
#[track_caller]
fn assert_plan_limits(plan: &RunPlan) {
    assert!((1..=100).contains(&plan.entries.len()));
    let ms = 1..=3_600_000;
    let mut patterns = 0;
    for entry in &plan.entries {
        assert!((1..=100).contains(&entry.cold_boots));
        assert!(ms.contains(&entry.boot_timeout_ms));
        assert!((1..=plan::MAX_CAPTURE_BYTES).contains(&entry.max_capture_bytes));
        for step in entry.steps() {
            assert!((1..=4096).contains(&step.send.len()) && ms.contains(&step.timeout_ms));
        }
        let lists = [
            entry.markers.len(),
            entry.expect.len(),
            entry.order.len(),
            entry.forbid.len(),
            entry.steps().len(),
            entry.values.len(),
            entry.segments.len(),
            entry.before_first_send.len(),
        ];
        assert!(lists.iter().all(|items| *items <= 100), "{lists:?}");
        patterns += entry.markers.len() + entry.forbid.len() + entry.steps().len();
        patterns += entry.values.len();
        assert!(entry.reference.as_deref().is_none_or(reference));
    }
    assert!(plan.reference.as_deref().is_none_or(reference));
    assert!(patterns <= plan::MAX_PATTERNS);
}

/// Whether `text` keeps the limits of a reference: at most 1024 bytes and
/// no control character.
fn reference(text: &str) -> bool {
    text.len() <= 1024 && !text.chars().any(char::is_control)
}

/// The run plans: the example run plan and those of the corpus, changed
/// line by line and character by character, are read or refused with a
/// `PlanError` without a panic; a plan that is read keeps its text byte for
/// byte and keeps the limits of a run plan, and its check against the board
/// description and the pinned sets does not panic. A plan whose only change
/// sets a value at or just past a limit is read exactly when the limit
/// allows the value.
fn run_plans(cases: u64) {
    let (board, seeds) = (pi5(), seed_plans());
    let instance = Instance::parse(&instance_text()).unwrap();
    let pins = &instance.parameters().pins;
    let (mut read, mut at_limits) = (0, [0; 2]);
    fuzz("run_plans", cases, |rng| {
        let mut text = rng.pick(&seeds).clone();
        let changes = rng.within(1..=4);
        let limits: Vec<Option<bool>> = (0..changes)
            .map(|_| change_plan(rng, &mut text, &seeds))
            .collect();
        let parsed = RunPlan::parse(&text);
        if let [Some(allowed)] = limits[..] {
            let error = parsed.as_ref().err();
            assert_eq!(parsed.is_ok(), allowed, "{error:?}");
            at_limits[usize::from(allowed)] += 1;
        }
        match parsed {
            Ok(plan) => {
                // A plan keeps the text it was read from, byte for byte.
                assert_eq!(plan.text(), text);
                assert_plan_limits(&plan);
                let _ = plan.check(&board, pins);
                read += 1;
            }
            Err(PlanError::Invalid { at, reason }) => assert!(!at.is_empty() && !reason.is_empty()),
            Err(error) => assert!(!error.to_string().is_empty()),
        }
    });
    // Some of the changed plans are read, but not most.
    assert!(read > cases / 20 && read < cases / 2, "{read} read");
    // Values just past a limit were refused, and values at one read.
    assert!(
        at_limits.iter().all(|count| *count > cases / 200),
        "{at_limits:?}"
    );
}

#[test]
fn fuzz_run_plans() {
    run_plans(PLAN_CASES);
}

/// The commit of the receipts of the cases.
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

/// A console that keeps a copy of the bytes that it gives in each cold
/// boot, from each time it is opened, and that fails the case once it is
/// sent more than `sends` times.
struct Played {
    console: Box<dyn Console>,
    played: Rc<RefCell<Vec<Vec<u8>>>>,
    sends: usize,
}

impl Console for Played {
    fn open(&mut self, deadline: Nanos) -> Result<Nanos, DeviceError> {
        let opened = self.console.open(deadline);
        if opened.is_ok() {
            self.played.borrow_mut().push(Vec::new());
        }
        opened
    }

    fn receive(&mut self, deadline: Nanos) -> Result<Received, DeviceError> {
        let received = self.console.receive(deadline)?;
        if let (Received::Bytes { bytes, .. }, Some(played)) =
            (&received, self.played.borrow_mut().last_mut())
        {
            played.extend_from_slice(bytes);
        }
        Ok(received)
    }

    fn send(&mut self, bytes: &[u8]) -> Result<Nanos, DeviceError> {
        // A run sends each step at most once in a cold boot; one that sends
        // more could send without end.
        assert!(
            self.sends > 0,
            "more sends than the steps of the cold boots"
        );
        self.sends -= 1;
        self.console.send(bytes)
    }

    fn close(&mut self) -> Result<(), DeviceError> {
        self.console.close()
    }
}

/// A bench of simulated devices, with a fault set on them if any, whose
/// console plays a script, keeps what it gives and takes at most `sends`
/// sends, of a tool of a fixed revision that gives a nonce drawn from the
/// case's generator, so that a case makes the same receipt in every run.
struct FuzzEnv {
    script: RefCell<Option<ScriptProvider>>,
    played: Rc<RefCell<Vec<Vec<u8>>>>,
    sends: usize,
    fault: Option<fn(&mut Knobs)>,
    nonce: String,
}

impl Env for FuzzEnv {
    fn open_bench(&self, _: &Board, _: &Instance, _: &Path) -> Result<Bench, String> {
        let script = self.script.borrow_mut().take().ok_or("the bench is open")?;
        let world = SimWorld::new();
        if let Some(fault) = self.fault {
            fault(&mut world.knobs());
        }
        let mut bench = env::simulated(&world, script);
        bench.console = Box::new(Played {
            console: bench.console,
            played: Rc::clone(&self.played),
            sends: self.sends,
        });
        Ok(bench)
    }

    fn tool(&self) -> ToolRevision {
        let (commit, clean) = (Some(COMMIT.to_string()), Some(true));
        ToolRevision { commit, clean }
    }

    fn nonce(&self) -> Result<String, String> {
        Ok(self.nonce.clone())
    }
}

/// The inputs of the runs of the receipts: the example run plan with one
/// cold boot of each entry, and with `limit` as the capture limit of each
/// entry if there is one, the example instance configuration and payloads.
fn receipt_inputs(board: &Board, limit: Option<usize>) -> Inputs {
    let example = text(fs::read(tools().join("examples/run-plan.toml")).unwrap());
    let mut plan_text = example.replacen("cold_boots = 3", "cold_boots = 1", 1);
    assert_ne!(plan_text, example);
    if let Some(limit) = limit {
        let timeout = "boot_timeout_ms = 60000\n";
        assert_eq!(plan_text.matches(timeout).count(), 2);
        let limited = format!("{timeout}max_capture_bytes = {limit}\n");
        plan_text = plan_text.replace(timeout, &limited);
    }
    let instance_text = instance_text();
    let instance = Instance::parse(&instance_text).unwrap();
    let plan = RunPlan::parse(&plan_text).unwrap();
    plan.check(board, &instance.parameters().pins).unwrap();
    Inputs {
        plan,
        plan_sha256: Sha256::of(plan_text.as_bytes()),
        instance,
        instance_sha256: Sha256::of(instance_text.as_bytes()),
        instance_dir: tools().join("examples"),
        board: board.clone(),
        payload_root: tools().join("examples/payload"),
    }
}

/// What the board writes in the runs of the receipts: the example console
/// output of the echo entry, without its STATS line, which the board writes
/// as the answer to the step, when it is served the kernel of that entry,
/// and else that of the firmware-only entry.
struct Outputs {
    echo: Vec<u8>,
    firmware: Vec<u8>,
    echo_kernel: Vec<u8>,
}

impl Outputs {
    fn new() -> Self {
        let captures = files("examples/captures", "bin");
        let capture = |name: &str| {
            let found = captures.iter().find(|(path, _)| path.ends_with(name));
            found.unwrap().1.clone()
        };
        let echo = replace(&capture("/echo.bin"), b"STATS seq=1\r\n", b"");
        let echo_kernel = fs::read(tools().join("examples/payload/echo/kernel.img")).unwrap();
        let firmware = capture("/firmware-only.bin");
        Self {
            echo,
            firmware,
            echo_kernel,
        }
    }
}

/// The script of the board in a run of the receipts: `outputs`, each
/// changed and cut into chunks 1 ms apart, and the answer to the step, the
/// STATS line, mostly as written.
fn receipt_script(rng: &mut Rng, seeds: &Seeds, outputs: &Outputs) -> ScriptProvider {
    // Changes of the bytes, now and then a lone CR among them.
    let change = |rng: &mut Rng, bytes: &[u8]| {
        let mut bytes = bytes.to_vec();
        for _ in 0..rng.below(3) {
            seeds.change(rng, &mut bytes);
        }
        if rng.chance(30) {
            let at = rng.position(&bytes);
            bytes.insert(at, b'\r');
        }
        bytes.truncate(MAX_TRANSCRIPT);
        bytes
    };
    let (echo, firmware) = (change(rng, &outputs.echo), change(rng, &outputs.firmware));
    let answer = if rng.chance(80) {
        b"STATS seq=1\r\n".to_vec()
    } else {
        change(rng, b"STATS seq=1\r\n")
    };
    let chunks = |rng: &mut Rng, bytes: &[u8]| {
        let (mut start, mut chunks) = (0, Vec::new());
        for (number, end) in chunk_ends(rng, bytes).into_iter().enumerate() {
            let at = Nanos::from_millis(number as u64 + 1);
            chunks.push((at, bytes[start..end].to_vec()));
            start = end;
        }
        chunks
    };
    let (echo, firmware) = (chunks(rng, &echo), chunks(rng, &firmware));
    let echo_kernel = outputs.echo_kernel.clone();
    let kernel = PayloadName::parse("kernel.img").unwrap();
    Box::new(move |_, files: &Files| {
        let chunks = if files.get(&kernel) == Some(&echo_kernel) {
            echo.clone()
        } else {
            firmware.clone()
        };
        let responders = vec![Responder {
            trigger: b"?\r".to_vec(),
            delay: Nanos::from_millis(2),
            bytes: answer.clone(),
        }];
        Script { chunks, responders }
    })
}

/// A fault of the bench in a run of the receipts, set on its simulated
/// world, and the verdict of the run that it gives.
type Fault = (fn(&mut Knobs), Option<Verdict>);

/// The faults of the runs of the receipts: staging that writes a changed
/// copy, which refuses the run before any power command; and relay
/// contacts that never close, a supply meter that reads the voltage of a
/// supply that is on whatever the supply does, and a TFTP directory that
/// serves a changed copy, each a fault of the bench.
const FAULTS: [Fault; 4] = [
    (|knobs: &mut Knobs| knobs.tftp.corrupts_staged = true, None),
    (
        |knobs: &mut Knobs| knobs.relay.stuck = true,
        Some(Verdict::BenchFault),
    ),
    (
        |knobs: &mut Knobs| knobs.meter.stuck_micro = Some(ON_MICRO),
        Some(Verdict::BenchFault),
    ),
    (
        |knobs: &mut Knobs| knobs.tftp.corrupts_served = true,
        Some(Verdict::BenchFault),
    ),
];

/// The fault of a case of the receipts: in one case of ten the staging
/// that refuses the run, in another one of the faults of the bench, and
/// else none.
fn fault_of(rng: &mut Rng) -> Option<Fault> {
    match rng.below(10) {
        0 => Some(FAULTS[0]),
        1 => Some(*rng.pick(&FAULTS[1..])),
        _ => None,
    }
}

/// Asserts that `error` names a value of a receipt by its JSON pointer.
#[track_caller]
fn assert_pointer(error: &ReceiptError) {
    let ReceiptError::Invalid { at, .. } = error else {
        panic!("{error}");
    };
    let key = at.strip_prefix('/').and_then(|at| at.split('/').next());
    assert!(key.is_some_and(|key| RECEIPT_KEYS.contains(&key)), "{at}");
}

/// `receipt` with the judgements of `from`, which re-deriving makes again:
/// its evidence, refusal and verdict, the checks and verdict of each entry,
/// and the observations, checks and verdict of each cold boot.
fn judged_as(receipt: &Receipt, from: &Receipt) -> Receipt {
    let mut judged = receipt.clone();
    judged.evidence = from.evidence.clone();
    judged.refusal = from.refusal.clone();
    judged.verdict = from.verdict;
    for (entry, original) in judged.entries.iter_mut().zip(&from.entries) {
        entry.checks = original.checks.clone();
        entry.verdict = original.verdict;
        let boots = entry.cold_boots.iter_mut().zip(&original.cold_boots);
        for (cold_boot, original) in boots {
            cold_boot.observations = original.observations.clone();
            cold_boot.checks = original.checks.clone();
            cold_boot.verdict = original.verdict;
        }
    }
    judged
}

/// Reads the rendering of `forged`, the receipt `original` that a run wrote
/// with typed fields changed: it is read as it is, or refused at a JSON
/// pointer, and re-deriving a receipt that is read gives a receipt or an
/// error at a JSON pointer. When only judgements were changed, re-deriving
/// undoes them and gives `original`; returns whether it did.
fn read_forged(original: &Receipt, forged: &Receipt) -> bool {
    let read = match Receipt::parse(forged.render().as_bytes()) {
        Ok(read) => read,
        Err(error) => {
            assert_pointer(&error);
            return false;
        }
    };
    assert_eq!(&read, forged);
    let again = rejudge(&read);
    if read != *original && judged_as(&read, original) == *original {
        let again = again.as_ref();
        assert_eq!(
            again,
            Ok(original),
            "re-deriving undoes a changed judgement"
        );
        return true;
    }
    if let Err(error) = again {
        assert_pointer(&error);
    }
    false
}

fn verdict_of(rng: &mut Rng) -> Verdict {
    *rng.pick(&[
        Verdict::Pass,
        Verdict::Fail,
        Verdict::BenchFault,
        Verdict::NotRun,
    ])
}

/// What [`forge`] changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Forgery {
    /// The rule, entry, subject or detail of the refusal of a refused run,
    /// which re-deriving makes again.
    Refusal,
    /// The entry of this index, which did not run because the entry before
    /// it was a bench fault, was given the events, checks, cold boots and
    /// verdict of that entry.
    RanAfterFault(usize),
    /// A change that breaks a rule of a receipt, which reading refuses: a
    /// nonce or a commit of another form, a changed eligibility, or console
    /// output shortened inside its last chunk without its length.
    Refused,
    /// Anything else.
    Other,
}

/// Changes one typed field of `receipt`, or a few in one part of it, and
/// says what it changed.
fn forge(rng: &mut Rng, receipt: &mut Receipt, plans: &[String]) -> Forgery {
    let entries = receipt.entries.len();
    if let Some(refusal) = &mut receipt.refusal
        && rng.chance(50)
    {
        // Rules whose refusal re-deriving makes from the recorded digests
        // alone.
        let rules = [
            "payload.source_digest",
            "payload.staged_digest",
            "payload.variant_missing",
            "write.not_enabled",
        ];
        match rng.below(4) {
            0 => {
                let others: Vec<&str> = rules
                    .into_iter()
                    .filter(|rule| *rule != refusal.rule)
                    .collect();
                refusal.rule = rng.pick(&others).to_string();
            }
            1 => refusal.subject.push_str(".other"),
            2 if entries > 1 => {
                let entry = refusal.entry.unwrap_or_default();
                refusal.entry = Some((entry + 1 + rng.below(entries - 1)) % entries);
            }
            _ => refusal.detail = format!("not {}", refusal.detail),
        }
        return Forgery::Refusal;
    }
    let verdicts: Vec<Verdict> = receipt.entries.iter().map(|entry| entry.verdict).collect();
    let not_run =
        |&at: &usize| verdicts[at - 1] == Verdict::BenchFault && verdicts[at] == Verdict::NotRun;
    if let Some(after) = (1..entries).find(not_run)
        && rng.chance(50)
    {
        let ran = receipt.entries[after - 1].clone();
        let record = &mut receipt.entries[after];
        (record.events, record.checks) = (ran.events, ran.checks);
        (record.cold_boots, record.verdict) = (ran.cold_boots, ran.verdict);
        return Forgery::RanAfterFault(after);
    }
    let boots: Vec<(usize, usize)> = receipt
        .entries
        .iter()
        .enumerate()
        .flat_map(|(entry, record)| (0..record.cold_boots.len()).map(move |boot| (entry, boot)))
        .collect();
    let entry = rng.below(receipt.entries.len());
    let choice = rng.below(16);
    if choice >= 9 && !boots.is_empty() {
        let (at, number) = *rng.pick(&boots);
        let cold_boot = &mut receipt.entries[at].cold_boots[number];
        let events = &mut cold_boot.events;
        let serial = &mut cold_boot.serial;
        let event = rng.below(events.len());
        match choice {
            9 => match rng.below(3) {
                0 => {
                    events.remove(event);
                }
                1 => events.insert(event, events[event].clone()),
                _ => {
                    let other = rng.below(events.len());
                    events.swap(event, other);
                }
            },
            10 => match rng.below(3) {
                0 => events[event].at_ns = rng.next() % (events[event].at_ns * 2 + 1),
                1 => events[event].received = rng.below(serial.bytes_base64.0.len() + 2) as u64,
                _ => {
                    if let EventKind::Send { step, bytes_base64 } = &mut events[event].event {
                        *step = rng.below(3);
                        bytes_base64.0.push(rng.byte());
                    }
                }
            },
            11 => {
                let bytes = &mut serial.bytes_base64.0;
                if !bytes.is_empty() {
                    let at = rng.below(bytes.len());
                    bytes[at] ^= 1 << rng.below(8);
                }
                if rng.chance(30) {
                    if rng.chance(50) {
                        bytes.truncate(rng.position(bytes));
                        serial.length = bytes.len() as u64;
                    } else if let Some(&[last, _]) = serial.index.last() {
                        // Shortened inside its last chunk, so that only its
                        // length does not describe it.
                        let after = last as usize + 1;
                        if after < bytes.len() {
                            bytes.truncate(rng.within(after..=bytes.len() - 1));
                            return Forgery::Refused;
                        }
                    }
                }
            }
            12 => {
                let index = &mut serial.index;
                let at = rng.below(index.len() + 1);
                match rng.below(3) {
                    0 if at < index.len() => {
                        index.remove(at);
                    }
                    1 if at < index.len() => index[at][rng.below(2)] ^= 1 << rng.below(8),
                    _ => index.insert(at, [rng.below(serial.length as usize + 2) as u64, ON]),
                }
            }
            13 => {
                let seen = &mut cold_boot.observations;
                match rng.below(3) {
                    0 => seen.markers.retain(|_| rng.chance(80)),
                    1 => seen.steps.clear(),
                    _ => seen.segments.retain(|_| rng.chance(50)),
                }
            }
            14 => {
                if let Some(check) = cold_boot.checks.get_mut(rng.below(16)) {
                    match rng.below(3) {
                        0 => check.result = Outcome::Fail,
                        1 => check.detail = None,
                        _ => check.rule = "rule.other".into(),
                    }
                }
            }
            _ => cold_boot.verdict = verdict_of(rng),
        }
        return Forgery::Other;
    }
    match choice {
        0 => {
            let original = receipt.nonce.clone();
            let nonce: Vec<char> = original.chars().collect();
            let at = rng.below(nonce.len() + 1);
            let edited = [
                &nonce[..at],
                &['G'][..rng.below(2)],
                &nonce[at.min(nonce.len())..],
            ];
            receipt.nonce = edited.concat().into_iter().collect();
            receipt.nonce.truncate(rng.within(31..=33));
            // A G among its digits, or 31 digits, is not a nonce.
            if receipt.nonce != original {
                return Forgery::Refused;
            }
        }
        1 => {
            // A commit that is unknown, or not 40 lowercase hexadecimal
            // digits; an unknown one is a reason that the evidence does
            // not list.
            receipt.tool.commit = rng
                .pick(&[None, Some(COMMIT.to_uppercase()), Some("0123".into())])
                .clone();
            receipt.tool.clean = *rng.pick(&[None, Some(false), Some(true)]);
            receipt.bench.simulated = rng.chance(50);
            return Forgery::Refused;
        }
        2 => {
            receipt.evidence.eligible = !receipt.evidence.eligible;
            return Forgery::Refused;
        }
        3 => {
            let mut plan = receipt.inputs.run_plan.text.clone();
            change_plan(rng, &mut plan, plans);
            if rng.chance(80) {
                receipt.inputs.run_plan.sha256 = Sha256::of(plan.as_bytes());
            }
            receipt.inputs.run_plan.text = plan;
        }
        4 => {
            let parameters = &mut receipt.inputs.instance.parameters;
            match rng.below(3) {
                0 => parameters.tftp.server_address = "198.51.100.13".into(),
                1 => parameters.confirmation.consecutive = rng.below(3) as u32,
                _ => parameters.tftp.path_prefix = "bench".into(),
            }
        }
        5 => {
            receipt.refusal = rng.chance(70).then(|| Refusal {
                rule: "payload.staged_digest".into(),
                entry: Some(rng.below(3)),
                subject: "config.txt".into(),
                detail: "the staged copy of config.txt was not read".into(),
            });
            if rng.chance(50) {
                receipt.verdict = None;
            }
        }
        6 => receipt.verdict = rng.chance(80).then(|| verdict_of(rng)),
        7 => {
            let record = &mut receipt.entries[entry];
            match rng.below(4) {
                0 => record.verdict = verdict_of(rng),
                1 => {
                    record.checks.pop();
                }
                2 => record.cold_boots.clear(),
                _ => {
                    if let Some(file) = record.payload.files.first_mut() {
                        file.served_sha256 = rng.chance(50).then(|| Sha256::of(b"other"));
                    }
                }
            }
        }
        _ => {
            let events = &mut receipt.start.events;
            if !events.is_empty() {
                let at = rng.below(events.len());
                events.remove(at);
            }
        }
    }
    Forgery::Other
}

/// What the cases of the receipts showed.
#[derive(Default)]
struct ReceiptCover {
    verdicts: BTreeSet<String>,
    sent: u64,
    undone: u64,
    refused: u64,
    refusals_undone: u64,
    ran_after_fault: u64,
    limited: u64,
    not_utf8: u64,
    lone_cr: u64,
    inside_chunk: u64,
}

/// `receipt` with the first send of the first cold boot that has a send and
/// a chunk of at least two bytes moved into the middle of that chunk, with
/// the JSON pointer of its `received`.
fn send_inside_a_chunk(receipt: &Receipt) -> Option<(String, Receipt)> {
    let mut moved = receipt.clone();
    for (number, entry) in moved.entries.iter_mut().enumerate() {
        for (boot, cold_boot) in entry.cold_boots.iter_mut().enumerate() {
            let send = |event: &Event| matches!(event.event, EventKind::Send { .. });
            let Some(event) = cold_boot.events.iter().position(send) else {
                continue;
            };
            let serial = &cold_boot.serial;
            let ends = serial.index.iter().skip(1).map(|chunk| chunk[0]);
            let ends = ends.chain([serial.length]);
            let mut chunks = serial.index.iter().map(|chunk| chunk[0]).zip(ends);
            let Some((start, end)) = chunks.find(|(start, end)| end - start >= 2) else {
                continue;
            };
            cold_boot.events[event].received = start + (end - start) / 2;
            let at = format!("/entries/{number}/cold_boots/{boot}/events/{event}/received");
            return Some((at, moved));
        }
    }
    None
}

/// The receipts: runs of the example plan, some with a small capture limit,
/// on simulated devices whose board writes changed example console output,
/// some refused or with a fault of the bench, send each step at most once
/// in a cold boot and write receipts that are read back as written,
/// re-derive to themselves and hold the bytes that the console gave, CR,
/// LF and bytes that are not UTF-8 among them, exactly, up to the capture
/// limit, at which a cold boot ends and fails `console.byte_limit`. A
/// receipt with a typed field changed is read as it is, or refused at a
/// JSON pointer, and re-deriving one that is read does not panic; one
/// changed against a rule of receipts is refused; when only its judgements
/// were changed, its refusal among them, re-deriving gives back the receipt
/// as written; and re-deriving finds an entry that ran after a bench fault.
fn receipts(cases: u64) {
    let board = pi5();
    let (seeds, plans) = (Seeds::new(&board), seed_plans());
    let outputs = Outputs::new();
    let mut cover = ReceiptCover::default();
    fuzz("receipts", cases, |rng| {
        let limit = rng.chance(20).then(|| rng.within(1..=1024));
        let inputs = receipt_inputs(&board, limit);
        let entries = inputs.plan.entries.iter();
        let sends = entries.map(|entry| entry.cold_boots as usize * entry.steps().len());
        let played = Rc::new(RefCell::new(Vec::new()));
        let script = receipt_script(rng, &seeds, &outputs);
        let fault = fault_of(rng);
        let env = FuzzEnv {
            script: RefCell::new(Some(script)),
            played: Rc::clone(&played),
            sends: sends.sum(),
            fault: fault.map(|(set, _)| set),
            nonce: format!("{:016x}{:016x}", rng.next(), rng.next()),
        };
        let receipt = run::run(&env, &inputs).unwrap();
        if let Some((_, verdict)) = fault {
            assert_eq!(receipt.verdict, verdict, "the verdict of a fault");
        }
        // The receipt that a run writes is read back as it was written and
        // re-derives to itself.
        let read = Receipt::parse(receipt.render().as_bytes()).unwrap();
        assert_eq!(read, receipt);
        assert_eq!(rejudge(&read).as_ref(), Ok(&read));
        // The first send moved inside a chunk is refused at its `received`.
        // Nothing is drawn from `rng`, so the rest of the case is as it was.
        if let Some((pointer, moved)) = send_inside_a_chunk(&read) {
            let error = Receipt::parse(moved.render().as_bytes()).unwrap_err();
            let refused = matches!(&error, ReceiptError::Invalid { at, .. } if *at == pointer);
            assert!(refused, "{error}");
            cover.inside_chunk += 1;
        }
        // Each cold boot holds the bytes that the console gave, exactly as
        // it gave them, up to the capture limit of its entry; it ended at
        // that limit, and failed it, exactly when the console gave more.
        let limits = inputs
            .plan
            .entries
            .iter()
            .map(|entry| entry.max_capture_bytes);
        let boots: Vec<_> = read
            .entries
            .iter()
            .zip(limits)
            .flat_map(|(entry, limit)| entry.cold_boots.iter().map(move |boot| (boot, limit)))
            .collect();
        let played = played.borrow();
        assert_eq!(boots.len(), played.len());
        for ((cold_boot, limit), played) in boots.iter().zip(played.iter()) {
            let kept = played.len().min(*limit as usize);
            assert_eq!(cold_boot.serial.bytes_base64.0, played[..kept]);
            let ends = cold_boot
                .events
                .iter()
                .filter_map(|event| match event.event {
                    EventKind::End { reason } => Some(reason),
                    _ => None,
                });
            let over = kept < played.len();
            assert_eq!(ends.eq([EndReason::ByteLimit]), over);
            let checks = cold_boot.checks.iter();
            let limited = checks.filter(|check| check.rule == "console.byte_limit");
            let results: Vec<Outcome> = limited.map(|check| check.result).collect();
            assert_eq!(results.contains(&Outcome::Fail), over, "{results:?}");
            cover.limited += u64::from(over);
        }
        let held: Vec<&[u8]> = boots
            .iter()
            .map(|(cold_boot, _)| &cold_boot.serial.bytes_base64.0[..])
            .collect();
        for _ in 0..FORGERIES {
            let mut forged = read.clone();
            let forgery = forge(rng, &mut forged, &plans);
            let undone = read_forged(&read, &forged);
            cover.undone += u64::from(undone);
            match forgery {
                Forgery::Refusal => {
                    assert!(undone, "re-deriving undoes a changed refusal");
                    cover.refusals_undone += 1;
                }
                Forgery::RanAfterFault(entry) => {
                    // An entry with events or cold boots ran; one with
                    // checks alone did not, and re-deriving undoes them.
                    let record = &forged.entries[entry];
                    if record.events.is_empty() && record.cold_boots.is_empty() {
                        assert!(undone, "re-deriving undoes the checks of an entry not run");
                    } else {
                        let pointer = format!("/entries/{entry}");
                        let again = rejudge(&forged);
                        let found = matches!(&again, Err(ReceiptError::Invalid { at, .. }) if *at == pointer);
                        assert!(found, "an entry ran after a bench fault: {again:?}");
                        cover.ran_after_fault += 1;
                    }
                }
                Forgery::Refused => {
                    let refused = Receipt::parse(forged.render().as_bytes());
                    assert!(refused.is_err(), "a receipt that breaks a rule is read");
                    cover.refused += 1;
                }
                Forgery::Other => {}
            }
        }
        let verdict = format!("{:?}", read.verdict);
        cover.verdicts.insert(verdict);
        let events = read.entries.iter().flat_map(|entry| &entry.cold_boots);
        let mut events = events.flat_map(|cold_boot| &cold_boot.events);
        cover.sent += u64::from(events.any(|e| matches!(e.event, EventKind::Send { .. })));
        let bytes = held.concat();
        cover.not_utf8 += u64::from(std::str::from_utf8(&bytes).is_err());
        let lone = bytes
            .windows(2)
            .any(|pair| pair[0] == b'\r' && pair[1] != b'\n');
        cover.lone_cr += u64::from(lone);
    });
    let verdicts = ["None", "Some(BenchFault)", "Some(Fail)", "Some(Pass)"];
    assert!(cover.verdicts.iter().eq(verdicts), "{:?}", cover.verdicts);
    let counts = [
        cover.sent,
        cover.undone,
        cover.refused,
        cover.not_utf8,
        cover.lone_cr,
        cover.inside_chunk,
    ];
    assert!(counts.iter().all(|count| *count > cases / 10), "{counts:?}");
    let rare = [cover.refusals_undone, cover.ran_after_fault, cover.limited];
    assert!(rare.iter().all(|count| *count > cases / 50), "{rare:?}");
}

#[test]
fn fuzz_receipts() {
    receipts(RECEIPT_CASES);
}

/// The contract gives the number of cases of each target with its
/// constant, and the contract and CONTRIBUTING give the factor of the
/// deeper search.
#[test]
fn the_documents_give_the_numbers_of_cases() {
    let words = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let contract = words(include_str!("../../../docs/bench/contract.md"));
    let contributing = words(include_str!("../../../CONTRIBUTING.md"));
    let targets = [
        ("fuzz_transcripts", TRANSCRIPT_CASES, "TRANSCRIPT_CASES"),
        ("fuzz_run_plans", PLAN_CASES, "PLAN_CASES"),
        ("fuzz_receipts", RECEIPT_CASES, "RECEIPT_CASES"),
    ];
    for (target, cases, constant) in targets {
        let given = format!("`{target}`, {cases} cases (`{constant}`)");
        assert!(
            contract.contains(&given),
            "the contract does not say {given}"
        );
    }
    // The documents say it in words.
    assert_eq!(DEEPER, 100);
    for text in [&contract, &contributing] {
        assert!(text.contains("a hundred times as many cases"));
    }
}

/// A hundred times as many cases of each target, for a deeper search than
/// the check makes; best run with `--release`.
#[test]
#[ignore = "runs a hundred times as many cases"]
fn fuzz_deeper() {
    transcripts(TRANSCRIPT_CASES * DEEPER);
    run_plans(PLAN_CASES * DEEPER);
    receipts(RECEIPT_CASES * DEEPER);
}

/// The documentation values that the corpus and the recordings may hold
/// for the board's serial number, the SD card's identifier and
/// card-specific data and the disk signature in the partition UUID of the
/// root file system.
const CHOSEN: [&str; 5] = [
    "1234abcd",
    "a1b2c3d4",
    "c1d0c1d0c1d0c1d0c1d0c1d0c1d0c1d0",
    "c5d0c5d0c5d0c5d0c5d0c5d0c5d0c5d0",
    "5ca1ab1e",
];

/// Whether `text` is an IPv4 address of a documentation network,
/// 192.0.2.0/24 or 198.51.100.0/24, no address, 0.0.0.0, or a network mask
/// that starts with 255.
fn documentation_ipv4(text: &[u8]) -> bool {
    let octets: Vec<&[u8]> = text.split(|byte| *byte == b'.').collect();
    matches!(
        octets[..],
        [b"192", b"0", b"2", _]
            | [b"198", b"51", b"100", _]
            | [b"0", b"0", b"0", b"0"]
            | [b"255", ..]
    )
}

/// Whether `text` is a MAC address of the documentation block
/// 00:00:5e:00:53:00 to 00:00:5e:00:53:ff.
fn documentation_mac(text: &[u8]) -> bool {
    let text = text.to_ascii_lowercase();
    text.starts_with(b"00:00:5e:00:53:") || text.starts_with(b"00-00-5e-00-53-")
}

/// Whether `value` is a documentation value, or made only of the `#` that
/// a public version puts in place of a value.
fn documentation(value: &[u8]) -> bool {
    let lower = value.to_ascii_lowercase();
    value.iter().all(|byte| *byte == b'#')
        || documentation_ipv4(value)
        || documentation_mac(value)
        || CHOSEN.iter().any(|chosen| lower == chosen.as_bytes())
}

/// The places in `bytes` of every run of `count` groups of bytes of `class`,
/// each of a length in `width`, joined by one of `separators`, each with its
/// bytes.
fn shapes<'a>(
    bytes: &'a [u8],
    separators: &[u8],
    count: usize,
    width: RangeInclusive<usize>,
    class: fn(&u8) -> bool,
) -> Vec<(usize, &'a [u8])> {
    let part = |byte: &u8| class(byte) || separators.contains(byte);
    let (mut found, mut at) = (Vec::new(), 0);
    while at < bytes.len() {
        if !part(&bytes[at]) {
            at += 1;
            continue;
        }
        let start = at;
        while at < bytes.len() && part(&bytes[at]) {
            at += 1;
        }
        let mut groups = Vec::new();
        let mut from = start;
        for (offset, byte) in bytes[start..at].iter().enumerate() {
            if separators.contains(byte) {
                groups.push((from, start + offset));
                from = start + offset + 1;
            }
        }
        groups.push((from, at));
        for window in groups.windows(count) {
            if window.iter().all(|(from, to)| width.contains(&(to - from))) {
                let (first, last) = (window[0].0, window[count - 1].1);
                found.push((first, &bytes[first..last]));
            }
        }
    }
    found
}

/// Whether `address` is an IPv6 address of the documentation prefix
/// 2001:db8::/32, the unspecified address `::` or the loopback address
/// `::1`.
fn documentation_ipv6(address: &Ipv6Addr) -> bool {
    let [first, second, ..] = address.segments();
    (first, second) == (0x2001, 0x0db8) || address.is_unspecified() || address.is_loopback()
}

/// The places in `bytes` of every IPv6 address: each longest run of
/// hexadecimal digits, colons and dots, without a dot or a colon that is
/// not part of `::` at either end, that holds at least two colons, that no
/// letter, digit or `_` touches and that reads as an IPv6 address.
fn ipv6_addresses(bytes: &[u8]) -> Vec<(usize, Ipv6Addr)> {
    let part = |byte: &u8| byte.is_ascii_hexdigit() || matches!(byte, b':' | b'.');
    let word =
        |byte: Option<&u8>| byte.is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_');
    let (mut found, mut at) = (Vec::new(), 0);
    while at < bytes.len() {
        if !part(&bytes[at]) {
            at += 1;
            continue;
        }
        let mut start = at;
        while at < bytes.len() && part(&bytes[at]) {
            at += 1;
        }
        let mut end = at;
        let lone = |at: usize, other: Option<usize>| {
            bytes[at] == b'.'
                || (bytes[at] == b':' && other.is_none_or(|other| bytes[other] != b':'))
        };
        while start < end && lone(start, (start + 1 < end).then_some(start + 1)) {
            start += 1;
        }
        while start < end && lone(end - 1, (end - 1 > start).then(|| end - 2)) {
            end -= 1;
        }
        let run = &bytes[start..end];
        let colons = run.iter().filter(|byte| **byte == b':').count();
        let touched =
            word(start.checked_sub(1).map(|before| &bytes[before])) || word(bytes.get(end));
        let address = std::str::from_utf8(run)
            .ok()
            .and_then(|text| text.parse().ok());
        if let Some(address) = address
            && colons >= 2
            && !touched
        {
            found.push((start, address));
        }
    }
    found
}

/// The places in `bytes` of values that are not documentation values: of
/// each value that the sensitive lines of `boards` hold, of every dotted
/// quad, every MAC address shape and every IPv6 address, and of every run
/// of exactly eight hexadecimal digits that `serial ` comes before or `/`
/// after, the shape of the serial number of a Raspberry Pi 5 on its
/// `BOOTMODE` line and as the path prefix of its TFTP reads; each named by
/// what it is and its offset, never by its value.
fn undocumented(boards: &[Board], bytes: &[u8]) -> Vec<String> {
    let mut found = Vec::new();
    for (span, rule, value) in observe::sensitive_values(boards, bytes) {
        if !documentation(&value) {
            found.push(format!("a value of {rule} at {}", span.start));
        }
    }
    for (at, quad) in shapes(bytes, b".", 4, 1..=3, u8::is_ascii_digit) {
        if !documentation_ipv4(quad) {
            found.push(format!("a dotted quad at {at}"));
        }
    }
    for (at, mac) in shapes(bytes, b":-", 6, 2..=2, u8::is_ascii_hexdigit) {
        if !documentation_mac(mac) {
            found.push(format!("a MAC address at {at}"));
        }
    }
    for (at, address) in ipv6_addresses(bytes) {
        if !documentation_ipv6(&address) {
            found.push(format!("an IPv6 address at {at}"));
        }
    }
    for (at, serial) in shapes(bytes, b"", 1, 8..=8, u8::is_ascii_hexdigit) {
        let named = bytes[..at].ends_with(b"serial ") || bytes[at + 8..].starts_with(b"/");
        if named && !documentation(serial) {
            found.push(format!("a serial number at {at}"));
        }
    }
    found
}

/// Every file below the directory `dir` of `tools/`, at any depth, in order
/// of their paths, each with its path below `tools/` and its bytes.
fn every_file(dir: &str) -> Vec<(String, Vec<u8>)> {
    let (mut found, mut dirs) = (Vec::new(), vec![dir.to_string()]);
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(tools().join(&dir)).unwrap() {
            let entry = entry.unwrap();
            let path = format!("{dir}/{}", entry.file_name().to_string_lossy());
            if entry.file_type().unwrap().is_dir() {
                dirs.push(path);
            } else {
                let bytes = fs::read(tools().join(&path)).unwrap();
                found.push((path, bytes));
            }
        }
    }
    found.sort();
    found
}

/// The places in the text `bytes` of an instance configuration of values
/// that are not documentation values, as [`undocumented`] finds them, and of
/// absolute paths: a line, a word or a quoted value that starts with `/`.
fn undocumented_settings(bytes: &[u8]) -> Vec<String> {
    let mut found = undocumented(&[pi5()], bytes);
    for (number, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
        let starts = [&b" /"[..], b"\"/", b"=/"];
        let path = starts
            .iter()
            .any(|start| line.windows(2).any(|pair| pair == *start));
        if line.starts_with(b"/") || path {
            found.push(format!("an absolute path on line {}", number + 1));
        }
    }
    found
}

#[test]
fn the_instance_template_and_the_bench_example_hold_documentation_values_only() {
    for path in ["templates/instance.toml", "examples/bench-instance.toml"] {
        let bytes = fs::read(tools().join(path)).unwrap();
        assert_eq!(undocumented_settings(&bytes), [""; 0], "{path}");
        let text = String::from_utf8(bytes).unwrap();
        let table: toml::Table = toml::from_str(&text).unwrap();
        let tftp = &table["tftp"];
        assert_eq!(
            tftp["server_address"].as_str(),
            Some("192.0.2.10"),
            "{path}"
        );
        assert_eq!(tftp["path_prefix"].as_str(), Some("example/"), "{path}");
        // A salt that is not one, and settings that are key names alone.
        let local = table["local"].as_table().unwrap();
        let salt = local["salt"].as_str().unwrap();
        let hex = salt
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        assert!(!(salt.len() == 32 && hex), "{path}");
        for (role, settings) in local.iter().filter(|(key, _)| *key != "salt") {
            let values = settings.as_table().unwrap().values();
            assert!(
                values.into_iter().all(|value| value.as_str() == Some("")),
                "{role}"
            );
        }
        // A value of a machine written into either is found.
        for (from, to) in [
            ("# The", "# 198.51.101.13 The"),
            ("dir = \"\"", "dir = \"/kyto-test/tftp\""),
        ] {
            let changed = text.replacen(from, to, 1);
            assert!(
                !undocumented_settings(changed.as_bytes()).is_empty(),
                "{to}"
            );
        }
    }
}

#[test]
fn the_corpus_and_recordings_hold_documentation_values_only() {
    let synthetic = include_str!("data/synthetic-board.toml");
    let boards = [pi5(), Board::parse(synthetic).unwrap()];
    // Each file of the corpus, the recordings and the example console
    // output, at any depth, is a seed of the fuzz tests or the Git
    // attributes of a directory, and holds documentation values only.
    let seeds = [
        (format!("{CORPUS}/transcripts"), "bin"),
        (format!("{CORPUS}/plans"), "toml"),
        (RECORDED.to_string(), "bin"),
        ("examples/captures".to_string(), "bin"),
    ];
    let known = |path: &str| {
        let (dir, name) = path.rsplit_once('/').unwrap();
        let extension = Path::new(name).extension();
        let seed = |(seeds, wanted): &(String, &str)| {
            dir == seeds && extension.is_some_and(|found| found == *wanted)
        };
        name == ".gitattributes" || seeds.iter().any(seed)
    };
    // A `.DS_Store` file, which Finder writes and Git ignores, is skipped.
    let skipped = |path: &str| path.ends_with("/.DS_Store");
    let mut checked = Vec::new();
    for dir in [CORPUS, RECORDED, "examples/captures"] {
        for (path, bytes) in every_file(dir) {
            if skipped(&path) {
                continue;
            }
            assert!(known(&path), "{path} is not a seed of the fuzz tests");
            let found = undocumented(&boards, &bytes);
            assert!(found.is_empty(), "{path}: {found:?}");
            checked.push(path);
        }
    }
    assert!(checked.len() >= 15, "{checked:?}");
    for (dir, extension) in &seeds {
        for (path, _) in files(dir, extension) {
            assert!(checked.contains(&path), "{path}");
        }
    }
    // A file of another extension, or in another directory, is none.
    let others = [
        format!("{CORPUS}/transcripts/boot.txt"),
        format!("{CORPUS}/transcripts/more/boot.bin"),
        format!("{CORPUS}/plans/plan.bin"),
        format!("{CORPUS}/boot.bin"),
        format!("{RECORDED}/boot.raw"),
        format!("{CORPUS}/transcripts/DS_Store"),
        format!("{CORPUS}/transcripts/boot.DS_Store"),
    ];
    for path in others {
        assert!(!known(&path) && !skipped(&path), "{path}");
    }
    assert!(skipped(&format!("{CORPUS}/transcripts/.DS_Store")));
    // The recordings hold documentation values of each kind, which the
    // gate finds and lets through; a value one step outside each is found.
    let recorded = files(RECORDED, "bin");
    let bytes: Vec<u8> = recorded.into_iter().flat_map(|(_, bytes)| bytes).collect();
    let places = |pattern: &[u8]| {
        bytes
            .windows(pattern.len())
            .filter(|w| *w == pattern)
            .count()
    };
    let kinds = [
        &b"00:00:5e:00:53:b1"[..],
        b"198.51.100.13",
        b"255.0.0.0",
        b"a1b2c3d4",
        b"c1d0c1d0c1d0c1d0c1d0c1d0c1d0c1d0",
        b"c5d0c5d0c5d0c5d0c5d0c5d0c5d0c5d0",
        b"5ca1ab1e",
    ];
    for kind in kinds {
        assert!(places(kind) > 0, "{}", String::from_utf8_lossy(kind));
    }
    let outside = [
        (&b"00:00:5e:00:53:b1"[..], &b"00:00:5e:00:54:b1"[..]),
        (b"198.51.100.13", b"198.51.101.13"),
        (b"serial a1b2c3d4", b"serial a1b2c3d5"),
        (
            b"CID: c1d0c1d0c1d0c1d0c1d0c1d0c1d0c1d0",
            b"CID: c1d0c1d0c1d0c1d0c1d0c1d0c1d0c1d1",
        ),
        (
            b"CSD: c5d0c5d0c5d0c5d0c5d0c5d0c5d0c5d0",
            b"CSD: c5d0c5d0c5d0c5d0c5d0c5d0c5d0c5d1",
        ),
        (b"PARTUUID=5ca1ab1e", b"PARTUUID=5ca1ab1f"),
        (b"tftp-rt/config.txt", b"fedc9876/config.txt"),
    ];
    for (from, to) in outside {
        let changed = replace(&bytes, from, to);
        assert_ne!(changed, bytes);
        assert!(!undocumented(&boards, &changed).is_empty());
    }
    let bare = undocumented(
        &boards,
        b"x 192.0.3.1 y 0:1:2:3:4:5 z 02:00:00:00:00:01\r\n",
    );
    assert_eq!(bare, ["a dotted quad at 2", "a MAC address at 28"]);
    // IPv6 addresses outside 2001:db8::/32 are found, among them one that
    // holds a MAC address, and times and paths are none.
    let ipv6 = undocumented(
        &boards,
        b"x fe80::da3a:ddff:fe12:3456 y 2a02:1234:5678::1 z 2001:db8::1 :: ::1 15:59:09 std::fs.\r\n",
    );
    assert_eq!(ipv6, ["an IPv6 address at 2", "an IPv6 address at 30"]);
}
