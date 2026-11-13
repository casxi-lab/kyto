//! The checks and verdicts of a run (`docs/bench/formats.md`, checks of a
//! cold boot and verdicts): what a cold boot showed against what its entry
//! expects, and the verdicts of cold boots, entries and runs. A run and a
//! reader of its receipt call the same functions on the same records.

use crate::board::Board;
use crate::instance::{Parameters, PinnedFile};
use crate::lines::{self, IndexError, Line};
use crate::names::PayloadName;
use crate::observe::{
    ColdBootInputs, FirmwareSeen, MarkerSeen, Observations, ReadSeen, Sent, observe,
};
use crate::plan::{BOARD_MARKER, End, Entry};
use crate::power::{self, nanos};
use crate::receipt::{
    Check, Command, EndReason, EntryRecord, Event, EventKind, Outcome, PayloadFile, Receipt,
    Verdict,
};

/// What the rules of a cold boot read: its context and its record.
#[derive(Clone, Copy, Debug)]
pub struct ColdBoot<'a> {
    pub board: &'a Board,
    pub entry: &'a Entry,
    pub parameters: &'a Parameters,
    pub payload: &'a [PinnedFile],
    /// The bytes received, and the `[offset, at_ns]` of each chunk.
    pub transcript: &'a [u8],
    pub index: &'a [[u64; 2]],
    /// The events of the cold boot.
    pub events: &'a [Event],
    /// When the supply was last confirmed off before the cold boot.
    pub off_at: Option<u64>,
}

/// The sends among `events`, as [`observe`] reads them.
pub fn sends(events: &[Event]) -> Vec<Sent> {
    let sent = |(event, record): (usize, &Event)| match record.event {
        EventKind::Send { step, .. } => {
            let (at_ns, received) = (record.at_ns, record.received);
            Some(Sent {
                step,
                event,
                at_ns,
                received,
            })
        }
        _ => None,
    };
    events.iter().enumerate().filter_map(sent).collect()
}

/// What a cold boot showed and its checks, in this order: those of
/// [`power::cold_boot_checks`]; `boot.network`, `boot.fallback`, `boot.end`
/// and `console.byte_limit`; `marker.count`, `marker.order`,
/// `marker.forbidden` and `marker.before_first_send` for each of their
/// items in the entry; `step.response` for each step, or
/// `step.declared_none`; `firmware.version` and `segment.digest` for each
/// of their items; `tftp.server`, `tftp.prefix`, `tftp.unplanned_file` and
/// `identity.disagree`. Each but the first ones is a failure when it does
/// not pass. Fails when the index does not describe the transcript.
pub fn judge(c: &ColdBoot<'_>) -> Result<(Observations, Vec<Check>), IndexError> {
    let (board, entry, events) = (c.board, c.entry, c.events);
    let sent = sends(events);
    let (tftp, payload, transcript, index) = (&c.parameters.tftp, c.payload, c.transcript, c.index);
    let inputs = ColdBootInputs {
        board,
        entry,
        tftp,
        payload,
        transcript,
        index,
        sends: &sent,
    };
    let seen = observe(&inputs)?;
    let lines = lines::split(lines::chunks(transcript, index)?);
    let confirmation = &c.parameters.confirmation;
    let checks = power::cold_boot_checks(board, confirmation, events, index, c.off_at);
    let mut judging = Judging {
        c,
        seen: &seen,
        lines: &lines,
        sent: &sent,
        checks,
    };
    judging.boot_mode();
    judging.boot_end();
    judging.capture_limit();
    judging.markers();
    judging.steps();
    judging.firmware();
    judging.segments();
    judging.transfers();
    judging.identity();
    let checks = judging.checks;
    Ok((seen, checks))
}

/// A cold boot, what it showed, its lines and sends, and its checks so far.
struct Judging<'a> {
    c: &'a ColdBoot<'a>,
    seen: &'a Observations,
    lines: &'a [Line],
    sent: &'a [Sent],
    checks: Vec<Check>,
}

fn line_at(at: u64) -> Option<String> {
    Some(format!("the line at {at}"))
}

impl Judging<'_> {
    /// Adds the check of `rule` on `subject`, failed when there is a `failure`.
    fn fail(&mut self, rule: &str, subject: &str, failure: Option<String>) {
        self.checks
            .push(Check::new(rule, subject, Outcome::Fail, failure));
    }

    /// The position among the lines of the line that starts at `offset`.
    fn line(&self, offset: u64) -> Option<usize> {
        self.lines.iter().position(|line| line.offset == offset)
    }

    /// When the capture of the cold boot ended, and why.
    fn end(&self) -> Option<(u64, EndReason)> {
        self.c.events.iter().find_map(|event| match event.event {
            EventKind::End { reason } => Some((event.at_ns, reason)),
            _ => None,
        })
    }

    /// `boot.network` and `boot.fallback` (VERD-R05).
    fn boot_mode(&mut self) {
        let board = self.c.board;
        let (network, modes) = (&board.boot_mode.network, &self.seen.boot_modes);
        let marker = format!("{BOARD_MARKER}{}", board.boot_mode.network_marker);
        let none = !modes.iter().any(|seen| &seen.mode == network);
        let failure = none.then(|| "no network boot mode line".into());
        self.fail("boot.network", &marker, failure);
        let other = modes.last().filter(|last| &last.mode != network);
        let failure = other.and_then(|last| line_at(last.line));
        self.fail("boot.fallback", &marker, failure);
    }

    /// `boot.end` (VERD-R06).
    fn boot_end(&mut self) {
        let (entry, events, lines) = (self.c.entry, self.c.events, self.lines);
        let on = events
            .iter()
            .find(|e| power::command(e) == Some(Command::On));
        let timeout = nanos(entry.boot_timeout_ms);
        let deadline = on.map_or(0, |on| on.at_ns.saturating_add(timeout));
        // Only a complete line of the end marker ends a cold boot.
        let complete = |m: &MarkerSeen| self.line(m.line).is_some_and(|at| lines[at].terminated);
        let ends = |name: &str| {
            let ends = |m: &MarkerSeen| m.name == name && m.at_ns <= deadline && complete(m);
            Some(self.seen.markers.iter().any(ends))
        };
        let (subject, ended) = match &entry.end {
            End::Marker(end) => (&end.marker[..], ends(&end.marker)),
            End::Timeout(_) => ("timeout", self.end().map(|(at_ns, _)| at_ns >= deadline)),
        };
        let failure = (ended != Some(true)).then(|| "not by the boot timeout".into());
        self.fail("boot.end", subject, failure);
    }

    /// `console.byte_limit` (VERD-R11).
    fn capture_limit(&mut self) {
        let limits = [EndReason::ByteLimit, EndReason::ChunkLimit];
        let end = self.end();
        let limit = end.is_some_and(|(_, reason)| limits.contains(&reason));
        let failure = limit.then(|| "the capture reached its limit".into());
        self.fail("console.byte_limit", "console", failure);
    }

    /// `marker.count`, `marker.order`, `marker.forbidden` and
    /// `marker.before_first_send` (VERD-R08).
    fn markers(&mut self) {
        let (entry, seen, lines) = (self.c.entry, self.seen, self.lines);
        let first = |name: &str| seen.markers.iter().find(|marker| marker.name == name);
        for expect in &entry.expect {
            let named = |seen: &&MarkerSeen| seen.name == expect.marker;
            let (count, (min, max)) = (seen.markers.iter().filter(named).count(), expect.range());
            let ok = count >= min as usize && max.is_none_or(|max| count <= max as usize);
            let failure = (!ok).then(|| format!("{count} lines"));
            self.fail("marker.count", &expect.marker, failure);
        }
        for order in &entry.order {
            let found = order.markers.iter().map(|name| first(name).map(|m| m.line));
            let found: Vec<_> = found.collect();
            let ok = !found.contains(&None) && found.is_sorted_by(|a, b| a < b);
            let failure = (!ok).then(|| "not in this order".into());
            self.fail("marker.order", &order.markers.join(" "), failure);
        }
        for (number, forbid) in entry.forbid.iter().enumerate() {
            let bound = |name: &Option<String>| name.as_ref().map(|n| first(n).map(|m| m.line));
            let (from, to) = (bound(&forbid.from), bound(&forbid.to));
            let after = |at: u64| from.flatten().is_none_or(|from| at > from);
            let inside = |at: u64| after(at) && to.flatten().is_none_or(|to| at < to);
            let mut window = lines.iter().filter(|_| from != Some(None));
            let hit = window.find(|l| inside(l.offset) && forbid.line.is_match(&l.content));
            let failure = hit.and_then(|line| line_at(line.offset));
            self.fail("marker.forbidden", &number.to_string(), failure);
        }
        let first_send = self.sent.first().map(|send| send.received);
        let length = self.c.transcript.len() as u64;
        let end_of = |at: usize| lines.get(at + 1).map_or(length, |next| next.offset);
        for name in &entry.before_first_send {
            let end = first(name).and_then(|m| self.line(m.line)).map(end_of);
            let ok = first_send.is_none_or(|send| end.is_some_and(|end| end <= send));
            let failure = (!ok).then(|| "not ended before".into());
            self.fail("marker.before_first_send", name, failure);
        }
    }

    /// `step.declared_none`, or `step.response` for each step (VERD-R09).
    fn steps(&mut self) {
        let (entry, events, lines, sent) = (self.c.entry, self.c.events, self.lines, self.sent);
        if entry.steps.is_none() {
            let failure = sent.first().map(|send| format!("step {}", send.step));
            self.fail("step.declared_none", "sends", failure);
        }
        for (number, step) in entry.steps().iter().enumerate() {
            let seen = self.seen.steps.get(number);
            let sent = seen.and_then(|seen| Some(events[seen.send_event?].at_ns));
            let answered = seen.and_then(|seen| Some(lines[self.line(seen.response_line?)?].at_ns));
            let failure = match (seen, sent, answered) {
                (None, _, _) => Some(format!("{} did not arm it", step.after)),
                (_, None, _) => Some("not sent".into()),
                (_, Some(at), Some(by)) if by <= at.saturating_add(nanos(step.timeout_ms)) => None,
                _ => Some("no response in time".into()),
            };
            self.fail("step.response", &number.to_string(), failure);
        }
    }

    /// `firmware.version` (VERD-R07).
    fn firmware(&mut self) {
        let seen = self.seen;
        for (name, groups) in &self.c.entry.firmware {
            let found: Vec<_> = seen.firmware.iter().filter(|l| &l.name == name).collect();
            let holds = |line: &FirmwareSeen, (g, v): (&String, &String)| {
                line.groups
                    .iter()
                    .any(|(group, value)| group == g && value == v)
            };
            let other = found
                .iter()
                .find(|line| !groups.iter().all(|pair| holds(line, pair)));
            let failure = match other {
                _ if found.is_empty() => Some("no line".into()),
                other => other.and_then(|line| line_at(line.line)),
            };
            self.fail("firmware.version", name, failure);
        }
    }

    /// `segment.digest` (VERD-R10).
    fn segments(&mut self) {
        let seen = self.seen;
        for segment in &self.c.entry.segments {
            let failure = match seen.segments.iter().find(|seen| seen.name == segment.name) {
                None => Some("its lines did not both appear".into()),
                Some(seen) => (seen.sha256 != segment.sha256).then(|| "another SHA-256".into()),
            };
            self.fail("segment.digest", &segment.name, failure);
        }
    }

    /// `tftp.server`, `tftp.prefix` and `tftp.unplanned_file` (VERD-R11).
    fn transfers(&mut self) {
        let (servers, reads) = (&self.seen.tftp.servers, &self.seen.tftp.reads);
        let other = servers.iter().find(|server| !server.instance);
        let failure = other.and_then(|s| line_at(s.line));
        self.fail("tftp.server", "servers", failure);
        let outside = reads.iter().find(|read| !read.inside_prefix);
        let failure = outside.and_then(|r| line_at(r.line));
        self.fail("tftp.prefix", "reads", failure);
        let payload = self.c.payload;
        let planned =
            |name: &PayloadName| payload.iter().any(|f| f.name.eq_ignore_ascii_case(name));
        let unplanned =
            |read: &&ReadSeen| read.inside_prefix && !read.name.as_ref().is_some_and(planned);
        let unplanned = reads.iter().find(unplanned);
        let failure = unplanned.and_then(|r| line_at(r.line));
        self.fail("tftp.unplanned_file", "reads", failure);
    }

    /// `identity.disagree` (VERD-R11).
    fn identity(&mut self) {
        let identity = &self.seen.identity;
        let split = identity.iter().find(|field| field.values.len() > 1);
        let failure = split.map(|field| field.field.clone());
        self.fail("identity.disagree", "identity", failure);
    }
}

/// The checks of an entry outside its cold boots, in this order: for the
/// first entry, [`power::start_check`] of the events `start` before it, when
/// there are any; [`power::device_errors`] of `start` and of the entry's
/// `events`; and, once a `switch` event shows that the board is served the
/// entry's generation, `payload.served_digest` of its `files` against the
/// `payload` of the plan.
pub fn entry_checks(
    parameters: &Parameters,
    start: &[Event],
    events: &[Event],
    payload: &[PinnedFile],
    files: &[PayloadFile],
) -> Vec<Check> {
    let start_check =
        (!start.is_empty()).then(|| power::start_check(&parameters.confirmation, start));
    let mut checks: Vec<Check> = start_check.into_iter().collect();
    checks.extend(power::device_errors(start).chain(power::device_errors(events)));
    let switched = |event: &Event| matches!(event.event, EventKind::Switch { .. });
    if events.iter().any(switched) {
        let found = crate::payload::digests(files, |file| file.served_sha256.as_ref());
        checks.extend(crate::payload::check_served(payload, &found));
    }
    checks
}

/// Judges entry `index` of `receipt`, whose payload is `payload` and which
/// plans `planned` cold boots, from its record and, for the first entry,
/// the events of the start: its checks ([`entry_checks`]) and its verdict
/// ([`entry`]).
pub fn close_entry(receipt: &mut Receipt, index: usize, payload: &[PinnedFile], planned: u32) {
    let start = if index == 0 {
        &receipt.start.events[..]
    } else {
        &[]
    };
    let parameters = &receipt.inputs.instance.parameters;
    let record = &mut receipt.entries[index];
    let files = &record.payload.files;
    record.checks = entry_checks(parameters, start, &record.events, payload, files);
    record.verdict = entry(record, planned);
}

/// The verdict of a cold boot from its checks: BENCH FAULT when one is a
/// fault, else FAIL when one failed, else PASS.
pub fn cold_boot(checks: &[Check]) -> Verdict {
    worst(checks.iter().map(|check| match check.result {
        Outcome::Pass => Verdict::Pass,
        Outcome::Fail => Verdict::Fail,
        Outcome::Fault => Verdict::BenchFault,
    }))
}

/// The verdict of an entry of `planned` cold boots from its record: NOT RUN
/// when it has no event, check or cold boot; BENCH FAULT when one of its
/// checks is a fault or a cold boot is BENCH FAULT or NOT RUN, which no cold
/// boot that ran is; else FAIL when a cold
/// boot is FAIL; else PASS when every planned cold boot ran, and BENCH
/// FAULT when the entry stopped short with no reason.
pub fn entry(record: &EntryRecord, planned: u32) -> Verdict {
    if record.events.is_empty() && record.checks.is_empty() && record.cold_boots.is_empty() {
        return Verdict::NotRun;
    }
    let boots = record.cold_boots.iter().map(|cold_boot| cold_boot.verdict);
    match worst([cold_boot(&record.checks)].into_iter().chain(boots)) {
        Verdict::Pass if record.cold_boots.len() != planned as usize => Verdict::BenchFault,
        verdict => verdict,
    }
}

/// The verdict of a run that was not refused, from the verdicts of its
/// entries: BENCH FAULT when an entry is BENCH FAULT or NOT RUN, which only
/// a fault leaves; else FAIL when an entry is FAIL; else PASS.
pub fn run(entries: &[Verdict]) -> Verdict {
    worst(entries.iter().copied())
}

/// The worst of `verdicts`: BENCH FAULT, which NOT RUN counts as, then
/// FAIL, then PASS.
fn worst(verdicts: impl IntoIterator<Item = Verdict>) -> Verdict {
    let rank = |verdict| match verdict {
        Verdict::Pass => 0,
        Verdict::Fail => 1,
        Verdict::BenchFault | Verdict::NotRun => 2,
    };
    let worst = verdicts.into_iter().map(rank).max().unwrap_or(0);
    [Verdict::Pass, Verdict::Fail, Verdict::BenchFault][worst]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::RASPBERRY_PI_5;
    use crate::digest::Sha256;
    use crate::instance::Instance;
    use crate::plan::RunPlan;
    use crate::receipt::tests::instance_text;
    use crate::receipt::{Base64, ColdBootRecord, Command, PayloadRecord, Serial};

    /// A cold boot that shows what [`plan`] expects, one line per chunk.
    const LINES: [&str; 10] = [
        "  1.08 RPi: BOOTSYS release VERSION:1accd665 DATE: 2026/09/25 TIME: 15:59:09",
        "  4.37 RPi: BOOTLOADER release VERSION:1accd665 DATE: 2026/09/25 TIME: 15:59:09",
        "  4.37 BOOTMODE: 0x06 partition 0 build-ts BUILD_TIMESTAMP=1790351949 serial 1234abcd boardrev d04171 stc 4378801",
        "  4.94 Boot mode: NETWORK (02) order f1",
        "  5.51 NETWORK: 00:00:5e:00:53:01 wait for link TFTP: 192.0.2.10",
        " 16.03 Read config.txt bytes      246 hnd 0x0",
        " 26.96 MESS:00:00:26.964387:0: Starting OS 26964 ms",
        "Example echo payload",
        "READY",
        "STATS seq=1",
    ];

    /// The time of the power on, and of the first line 100 ms later.
    const ON: u64 = 10_000_000_000;
    const STEP: u64 = 100_000_000;

    /// A run plan of one entry with every kind of rule, with `edit` made.
    fn plan(edit: impl Fn(String) -> String) -> RunPlan {
        let empty = Sha256::of(b"");
        let banner = Sha256::of(b"Example echo payload\r\n");
        let text = format!(
            r#"format = "kyto-bench run plan"
format_version = 1
board = "raspberry-pi-5"

[[entries]]
name = "echo"
payload_dir = "echo"
payload = [{{ name = "config.txt", sha256 = "{empty}" }}]
cold_boots = 1
boot_timeout_ms = 60000
end = {{ marker = "ready" }}
markers = [{{ name = "ready", line = '^READY$' }}]
expect = [{{ marker = "ready", count = 1 }}]
order = [{{ markers = ["board.bootloader", "board.network_boot", "board.os_start", "ready"] }}]
forbid = [{{ line = '^PANIC' }}, {{ line = '^Example', from = "ready" }}]
before_first_send = ["board.network_boot", "ready"]
steps = [{{ after = "ready", send = "?\r", expect = '^STATS seq=1', timeout_ms = 1000 }}]
segments = [{{ name = "banner", from = "board.os_start", to = "ready", sha256 = "{banner}" }}]
firmware.bootloader = {{ hash = "1accd665", date = "2026/09/25" }}
"#
        );
        RunPlan::parse(&edit(text)).unwrap()
    }

    fn event(at_ns: u64, received: u64, event: EventKind) -> Event {
        Event {
            at_ns,
            received,
            event,
        }
    }

    fn power(command: Command) -> EventKind {
        EventKind::Power {
            command,
            relay: None,
        }
    }

    fn reading(micro: i64) -> EventKind {
        let quantity = crate::instance::Quantity::SupplyVoltage;
        EventKind::Reading { quantity, micro }
    }

    /// A cold boot of `lines`, line `k` received `k + 1` steps after the
    /// power on, the step sent when `READY` has been read, and the end
    /// after the last line.
    struct Case {
        plan: RunPlan,
        parameters: Parameters,
        serial: Vec<u8>,
        index: Vec<[u64; 2]>,
        events: Vec<Event>,
        off_at: Option<u64>,
    }

    fn case(lines: &[&str]) -> Case {
        case_ended(lines, "\r\n")
    }

    /// [`case`] with every line ended by `ending`.
    fn case_ended(lines: &[&str], ending: &str) -> Case {
        let (mut serial, mut index) = (Vec::new(), Vec::new());
        let mut send = None;
        for (number, line) in lines.iter().enumerate() {
            let at = ON + (number as u64 + 1) * STEP;
            index.push([serial.len() as u64, at]);
            serial.extend_from_slice(format!("{line}{ending}").as_bytes());
            if *line == "READY" && send.is_none() {
                send = Some((at, serial.len() as u64));
            }
        }
        let last = ON + lines.len() as u64 * STEP;
        let total = serial.len() as u64;
        let mut events = vec![
            event(ON, 0, EventKind::ConsoleReady {}),
            event(ON, 0, power(Command::On)),
            event(ON + STEP / 2, 0, reading(5_100_000)),
        ];
        if let Some((at, received)) = send {
            let bytes_base64 = Base64(b"?\r".to_vec());
            events.push(event(
                at,
                received,
                EventKind::Send {
                    step: 0,
                    bytes_base64,
                },
            ));
        }
        events.extend([
            event(
                last,
                total,
                EventKind::End {
                    reason: EndReason::EndCondition,
                },
            ),
            event(last, total, power(Command::Off)),
            event(last + STEP / 2, total, reading(0)),
            event(last + STEP / 2, total, EventKind::ConsoleClosed {}),
        ]);
        let parameters = Instance::parse(&instance_text())
            .unwrap()
            .parameters()
            .clone();
        let plan = plan(|text| text);
        let off_at = Some(ON - 5_000_000_000);
        Case {
            plan,
            parameters,
            serial,
            index,
            events,
            off_at,
        }
    }

    impl Case {
        fn judged(&self) -> (Observations, Vec<Check>) {
            let board = Board::parse(RASPBERRY_PI_5).unwrap();
            let entry = &self.plan.entries[0];
            let cold_boot = ColdBoot {
                board: &board,
                entry,
                parameters: &self.parameters,
                payload: entry.files(&[]).unwrap(),
                transcript: &self.serial,
                index: &self.index,
                events: &self.events,
                off_at: self.off_at,
            };
            judge(&cold_boot).unwrap()
        }

        /// The result of the check of `rule` on `subject`.
        fn result(&self, rule: &str, subject: &str) -> Outcome {
            let (_, checks) = self.judged();
            let found = checks
                .iter()
                .find(|c| c.rule == rule && c.subject == subject);
            found
                .unwrap_or_else(|| panic!("no check of {rule} on {subject}"))
                .result
        }

        /// The event of `kind`.
        fn at(&mut self, kind: fn(&EventKind) -> bool) -> &mut Event {
            self.events.iter_mut().find(|e| kind(&e.event)).unwrap()
        }
    }

    fn lines_with(edit: impl Fn(&mut Vec<&'static str>)) -> Vec<&'static str> {
        let mut lines = LINES.to_vec();
        edit(&mut lines);
        lines
    }

    #[test]
    fn a_cold_boot_that_shows_what_its_entry_expects_passes_every_rule() {
        let case = case(&LINES);
        let (seen, checks) = case.judged();
        let rules: Vec<(&str, &str)> = checks
            .iter()
            .map(|check| (check.rule.as_str(), check.subject.as_str()))
            .collect();
        let order = "board.bootloader board.network_boot board.os_start ready";
        let expected = [
            ("console.not_ready", "console"),
            ("power.on_unconfirmed", "on"),
            ("power.off_interval", "on"),
            ("power.off_unconfirmed", "off"),
            ("power.order", "events"),
            ("console.before_power_on", "console"),
            ("boot.network", "board.network_boot"),
            ("boot.fallback", "board.network_boot"),
            ("boot.end", "ready"),
            ("console.byte_limit", "console"),
            ("marker.count", "ready"),
            ("marker.order", order),
            ("marker.forbidden", "0"),
            ("marker.forbidden", "1"),
            ("marker.before_first_send", "board.network_boot"),
            ("marker.before_first_send", "ready"),
            ("step.response", "0"),
            ("firmware.version", "bootloader"),
            ("segment.digest", "banner"),
            ("tftp.server", "servers"),
            ("tftp.prefix", "reads"),
            ("tftp.unplanned_file", "reads"),
            ("identity.disagree", "identity"),
        ];
        assert_eq!(rules, expected);
        assert!(
            checks.iter().all(|c| c.result == Outcome::Pass),
            "{checks:?}"
        );
        assert_eq!(cold_boot(&checks), Verdict::Pass);
        assert_eq!(seen.tftp.servers.len(), 1);
        assert_eq!(seen.segments.len(), 1);
    }

    #[test]
    fn console_output_before_the_power_on_is_a_bench_fault() {
        let mut case = case(&LINES);
        let rule = ("console.before_power_on", "console");
        assert_eq!(case.result(rule.0, rule.1), Outcome::Pass);
        // Every line came while the supply was confirmed off; markers,
        // the end marker included, are no less there.
        case.index = vec![[0, ON - STEP]];
        assert_eq!(case.result(rule.0, rule.1), Outcome::Fault);
        assert_eq!(case.result("boot.end", "ready"), Outcome::Pass);
        assert_eq!(cold_boot(&case.judged().1), Verdict::BenchFault);
    }

    #[test]
    fn a_boot_that_is_not_from_the_network_fails() {
        let missing = case(&lines_with(|lines| {
            lines.remove(3);
        }));
        assert_eq!(
            missing.result("boot.network", "board.network_boot"),
            Outcome::Fail
        );
        let fallback = case(&lines_with(|lines| {
            lines.insert(8, " 76.99 Boot mode: SD (01) order f")
        }));
        assert_eq!(
            fallback.result("boot.network", "board.network_boot"),
            Outcome::Pass
        );
        assert_eq!(
            fallback.result("boot.fallback", "board.network_boot"),
            Outcome::Fail
        );
        // A boot mode line of the SD card alone fails both rules.
        let sd = case(&lines_with(|lines| {
            lines[3] = "  4.94 Boot mode: SD (01) order f";
        }));
        assert_eq!(
            sd.result("boot.network", "board.network_boot"),
            Outcome::Fail
        );
        assert_eq!(
            sd.result("boot.fallback", "board.network_boot"),
            Outcome::Fail
        );
        // A boot order that tries the SD card first and then boots from the
        // network does not fall back.
        let sd_first = case(&lines_with(|lines| {
            lines.insert(3, "  4.20 Boot mode: SD (01) order f2")
        }));
        assert_eq!(
            sd_first.result("boot.network", "board.network_boot"),
            Outcome::Pass
        );
        assert_eq!(
            sd_first.result("boot.fallback", "board.network_boot"),
            Outcome::Pass
        );
    }

    #[test]
    fn the_end_marker_must_appear_by_the_boot_timeout() {
        let mut late = case(&LINES);
        late.plan = plan(|text| text.replace("boot_timeout_ms = 60000", "boot_timeout_ms = 800"));
        assert_eq!(late.result("boot.end", "ready"), Outcome::Fail);
        late.plan = plan(|text| text.replace("boot_timeout_ms = 60000", "boot_timeout_ms = 900"));
        assert_eq!(late.result("boot.end", "ready"), Outcome::Pass);
        let missing = case(&lines_with(|lines| lines.truncate(8)));
        assert_eq!(missing.result("boot.end", "ready"), Outcome::Fail);
        // An end marker in the unterminated last line ends nothing: the
        // step is never armed, and the capture runs on to the boot timeout.
        assert_eq!(case(&LINES[..9]).result("boot.end", "ready"), Outcome::Pass);
        let mut cut = case(&LINES[..9]);
        let length = cut.serial.len() - 2;
        cut.serial.truncate(length);
        cut.events
            .retain(|e| !matches!(e.event, EventKind::Send { .. }));
        for event in &mut cut.events {
            event.received = event.received.min(length as u64);
        }
        let end = cut.at(|kind| matches!(kind, EventKind::End { .. }));
        end.event = EventKind::End {
            reason: EndReason::BootTimeout,
        };
        assert_eq!(cut.result("marker.count", "ready"), Outcome::Pass);
        assert_eq!(cut.result("boot.end", "ready"), Outcome::Fail);
        // An entry that ends at the timeout captures until the deadline.
        let mut timeout = case(&LINES);
        let end = "end = { marker = \"ready\" }";
        timeout.plan = plan(|text| text.replace(end, "end = \"timeout\""));
        assert_eq!(timeout.result("boot.end", "timeout"), Outcome::Fail);
        timeout.plan = plan(|text| {
            let text = text.replace(end, "end = \"timeout\"");
            text.replace("boot_timeout_ms = 60000", "boot_timeout_ms = 1000")
        });
        assert_eq!(timeout.result("boot.end", "timeout"), Outcome::Pass);
    }

    #[test]
    fn a_capture_that_reached_its_limit_fails() {
        for reason in [EndReason::ByteLimit, EndReason::ChunkLimit] {
            let mut limited = case(&LINES);
            limited
                .at(|kind| matches!(kind, EventKind::End { .. }))
                .event = EventKind::End { reason };
            assert_eq!(
                limited.result("console.byte_limit", "console"),
                Outcome::Fail
            );
        }
    }

    #[test]
    fn a_marker_missing_or_counted_wrong_fails() {
        // The entry expects exactly one READY line.
        let twice = case(&lines_with(|lines| lines.push("READY")));
        assert_eq!(twice.result("marker.count", "ready"), Outcome::Fail);
        let missing = case(&lines_with(|lines| {
            lines.remove(8);
        }));
        assert_eq!(missing.result("marker.count", "ready"), Outcome::Fail);
        let (_, checks) = missing.judged();
        let count = checks.iter().find(|c| c.rule == "marker.count").unwrap();
        assert_eq!(count.detail.as_deref(), Some("0 lines"));
        // An entry that expects at least two: one line is below the
        // minimum, two are not, and without a maximum more are not either.
        let at_least = |text: String| {
            let count = "{ marker = \"ready\", count = 1 }";
            assert!(text.contains(count), "{count}");
            text.replace(count, "{ marker = \"ready\", min = 2 }")
        };
        let mut below = case(&LINES);
        below.plan = plan(at_least);
        assert_eq!(below.result("marker.count", "ready"), Outcome::Fail);
        for lines in [2, 3] {
            let mut enough = case(&lines_with(|all| {
                all.extend(std::iter::repeat_n("READY", lines - 1));
            }));
            enough.plan = plan(at_least);
            assert_eq!(enough.result("marker.count", "ready"), Outcome::Pass);
        }
    }

    #[test]
    fn markers_out_of_order_fail() {
        let order = "board.bootloader board.network_boot board.os_start ready";
        assert_eq!(case(&LINES).result("marker.order", order), Outcome::Pass);
        let swapped = case(&lines_with(|lines| lines.swap(1, 3)));
        assert_eq!(swapped.result("marker.order", order), Outcome::Fail);
        let missing = case(&lines_with(|lines| {
            lines.remove(6);
        }));
        assert_eq!(missing.result("marker.order", order), Outcome::Fail);
        // The order is that of the first line of each marker: a later line
        // of the bootloader leaves it as it was.
        let again = case(&lines_with(|lines| lines.push(LINES[1])));
        assert_eq!(again.result("marker.order", order), Outcome::Pass);
    }

    #[test]
    fn a_forbidden_line_fails() {
        // A forbidden line anywhere, or after the line of `from` only.
        let panic = case(&lines_with(|lines| lines.insert(2, "PANIC: oops")));
        assert_eq!(panic.result("marker.forbidden", "0"), Outcome::Fail);
        assert_eq!(panic.result("marker.forbidden", "1"), Outcome::Pass);
        let after = case(&lines_with(|lines| lines.push("Example again")));
        assert_eq!(after.result("marker.forbidden", "1"), Outcome::Fail);
        // Without the line of `from`, nothing is between the bounds.
        let mut early = case(&lines_with(|lines| {
            lines.remove(8);
        }));
        early.plan = plan(|text| {
            text.replace(
                "{ line = '^Example', from = \"ready\" }",
                "{ line = '^Example', to = \"ready\" }",
            )
        });
        assert_eq!(early.result("marker.forbidden", "1"), Outcome::Fail);
        early.plan = plan(|text| text);
        assert_eq!(early.result("marker.forbidden", "1"), Outcome::Pass);
    }

    #[test]
    fn markers_before_the_first_send_end_before_its_first_byte() {
        let is_send = |kind: &EventKind| matches!(kind, EventKind::Send { .. });
        let mut sent = case(&LINES);
        assert_eq!(
            sent.result("marker.before_first_send", "ready"),
            Outcome::Pass
        );
        sent.at(is_send).received -= 1;
        assert_eq!(
            sent.result("marker.before_first_send", "ready"),
            Outcome::Fail
        );
        assert_eq!(
            sent.result("marker.before_first_send", "board.network_boot"),
            Outcome::Pass
        );
        // A marker that never appeared did not come before the send.
        let missing = case(&lines_with(|lines| {
            lines.remove(3);
        }));
        assert_eq!(
            missing.result("marker.before_first_send", "board.network_boot"),
            Outcome::Fail
        );
        // The line of a marker that is the last line ends with the
        // transcript: before a send at that end, after one a byte earlier.
        let mut last = case(&LINES[..9]);
        assert_eq!(
            last.result("marker.before_first_send", "ready"),
            Outcome::Pass
        );
        last.at(is_send).received -= 1;
        assert_eq!(
            last.result("marker.before_first_send", "ready"),
            Outcome::Fail
        );
        // Only the first send counts: a second step, sent once a second
        // READY line has appeared, does not make the line of `ready` come
        // before the first.
        let lines = lines_with(|lines| lines.extend(["READY", "STATS seq=2"]));
        let mut two = case(&lines);
        two.plan = plan(|text| {
            let steps = "timeout_ms = 1000 }]";
            assert!(text.contains(steps), "{steps}");
            let second = "{ after = \"ready\", send = \"!\\r\", expect = '^STATS seq=2', timeout_ms = 1000 }";
            text.replace(steps, &format!("timeout_ms = 1000 }}, {second}]"))
        });
        let received: usize = lines[..11].iter().map(|line| line.len() + 2).sum();
        let bytes_base64 = Base64(b"!\r".to_vec());
        let send = EventKind::Send {
            step: 1,
            bytes_base64,
        };
        let end = two
            .events
            .iter()
            .position(|e| matches!(e.event, EventKind::End { .. }));
        let send = event(ON + 11 * STEP, received as u64, send);
        two.events.insert(end.unwrap(), send);
        assert_eq!(two.result("step.response", "1"), Outcome::Pass);
        two.at(is_send).received -= 1;
        assert_eq!(
            two.result("marker.before_first_send", "ready"),
            Outcome::Fail
        );
    }

    #[test]
    fn a_step_needs_its_trigger_its_send_and_a_response_in_time() {
        let mut slow = case(&LINES);
        slow.plan = plan(|text| text.replace("timeout_ms = 1000", "timeout_ms = 99"));
        assert_eq!(slow.result("step.response", "0"), Outcome::Fail);
        slow.plan = plan(|text| text.replace("timeout_ms = 1000", "timeout_ms = 100"));
        assert_eq!(slow.result("step.response", "0"), Outcome::Pass);
        let silent = case(&LINES[..9]);
        assert_eq!(silent.result("step.response", "0"), Outcome::Fail);
        let mut unsent = case(&LINES);
        unsent
            .events
            .retain(|e| !matches!(e.event, EventKind::Send { .. }));
        assert_eq!(unsent.result("step.response", "0"), Outcome::Fail);
        let unarmed = case(&lines_with(|lines| lines[8] = "NOT READY"));
        let (_, checks) = unarmed.judged();
        let step = checks.iter().find(|c| c.rule == "step.response").unwrap();
        assert_eq!(step.detail.as_deref(), Some("ready did not arm it"));
    }

    #[test]
    fn a_line_that_started_before_the_send_does_not_answer_it() {
        // The board wrote `STATS seq` with `READY` and the rest of its line
        // after the step was sent.
        let split = |lines: &[&str]| {
            let mut case = case(lines);
            let (ready, stats) = (case.index[8], case.index[9][0]);
            case.index[9][1] = ready[1];
            case.index.insert(10, [stats + 9, ready[1] + STEP]);
            let send = case.at(|e| matches!(e, EventKind::Send { .. }));
            assert_eq!((send.at_ns, send.received), (ready[1], stats));
            send.received = stats + 9;
            case
        };
        let early = split(&LINES);
        let stats = early.index[9][0] as usize;
        assert_eq!(&early.serial[stats..stats + 9], b"STATS seq");
        let (seen, _) = early.judged();
        assert_eq!(seen.steps[0].response_line, None);
        assert_eq!(early.result("step.response", "0"), Outcome::Fail);
        // A line of the same bytes that starts after the send answers it.
        let late = split(&lines_with(|lines| lines.push("STATS seq=1")));
        let (seen, _) = late.judged();
        assert_eq!(seen.steps[0].response_line, Some(late.index[11][0]));
        assert_eq!(late.result("step.response", "0"), Outcome::Pass);
    }

    #[test]
    fn an_entry_that_declares_no_sends_fails_when_one_is_sent() {
        let mut case = case(&LINES);
        let steps = "steps = [{ after = \"ready\", send = \"?\\r\", expect = '^STATS seq=1', timeout_ms = 1000 }]";
        case.plan = plan(|text| {
            let text = text.replace(
                "before_first_send = [\"board.network_boot\", \"ready\"]\n",
                "",
            );
            text.replace(steps, "sends = false")
        });
        assert_eq!(case.result("step.declared_none", "sends"), Outcome::Fail);
        case.events
            .retain(|e| !matches!(e.event, EventKind::Send { .. }));
        assert_eq!(case.result("step.declared_none", "sends"), Outcome::Pass);
    }

    #[test]
    fn firmware_versions_and_segments_are_those_of_the_entry() {
        let mut other = case(&LINES);
        other.plan = plan(|text| text.replace("hash = \"1accd665\"", "hash = \"0accd665\""));
        assert_eq!(
            other.result("firmware.version", "bootloader"),
            Outcome::Fail
        );
        let missing = case(&lines_with(|lines| {
            lines.remove(1);
        }));
        assert_eq!(
            missing.result("firmware.version", "bootloader"),
            Outcome::Fail
        );
        let changed = case(&lines_with(|lines| lines[7] = "Example echo payload!"));
        assert_eq!(changed.result("segment.digest", "banner"), Outcome::Fail);
        let gone = case(&lines_with(|lines| {
            lines.remove(6);
        }));
        assert_eq!(gone.result("segment.digest", "banner"), Outcome::Fail);
    }

    #[test]
    fn a_firmware_version_other_than_expected_fails() {
        // The entry expects the bootloader release in the EEPROM and the
        // BL31 version, and the rule reads a version line anywhere in the
        // cold boot. The BL31 line goes before the line of the start of the
        // operating system, so that the segment of the entry is unchanged,
        // although a Raspberry Pi 5 writes it after that line. The cold boot
        // shows both as expected.
        const BL31: &str = "NOTICE:  BL31: v2.6(release):v2.6-241-g36761b276";
        let bl31 = "firmware.bl31 = { version = \"v2.6(release):v2.6-241-g36761b276\" }\n";
        let expecting = |edit: &dyn Fn(String) -> String| {
            plan(|text| {
                let bootloader = "firmware.bootloader = ";
                assert!(text.contains(bootloader), "{bootloader}");
                edit(text.replace(bootloader, &format!("{bl31}{bootloader}")))
            })
        };
        let with_bl31 = |line: &'static str| {
            lines_with(|lines| {
                assert!(lines[6].contains("Starting OS"));
                lines.insert(6, line);
            })
        };
        // The result and detail of the checks of the bootloader and of
        // BL31, in that order.
        let results = |lines: &[&str], edit: &dyn Fn(String) -> String| {
            let mut case = case(lines);
            case.plan = expecting(edit);
            let (_, checks) = case.judged();
            ["bootloader", "bl31"].map(|name| {
                let found = checks
                    .iter()
                    .find(|c| c.rule == "firmware.version" && c.subject == name);
                let check = found.unwrap_or_else(|| panic!("no check of {name}"));
                (check.result, check.detail.clone())
            })
        };
        // A failure at the line of `index` among `lines`, by its offset.
        let other = |lines: &[&str], index: usize| {
            let at: usize = lines[..index].iter().map(|line| line.len() + 2).sum();
            (Outcome::Fail, Some(format!("the line at {at}")))
        };
        let pass = (Outcome::Pass, None::<String>);
        let none = (Outcome::Fail, Some("no line".to_string()));
        let same = |text: String| text;
        let shown = with_bl31(BL31);
        assert_eq!(results(&shown, &same), [pass.clone(), pass.clone()]);
        // The plan alone gives the expected values: another hash or date of
        // the bootloader, or another BL31 version, fails that line, and so
        // does a BL31 version that the line only starts with.
        let edits = [
            ("hash = \"1accd665\"", "hash = \"0accd665\""),
            ("date = \"2026/09/25\"", "date = \"2026/09/24\""),
            ("-241-g36761b276\" }", "-242-g36761b276\" }"),
            ("-241-g36761b276\" }", "-241\" }"),
        ];
        let failed = [
            [other(&shown, 1), pass.clone()],
            [other(&shown, 1), pass.clone()],
            [pass.clone(), other(&shown, 6)],
            [pass.clone(), other(&shown, 6)],
        ];
        for ((from, to), failed) in edits.into_iter().zip(failed) {
            let edit = |text: String| {
                assert!(text.contains(from), "{from}");
                text.replace(from, to)
            };
            assert_eq!(results(&shown, &edit), failed, "{to}");
        }
        // The cold boot shows another BL31 version, or none.
        let changed = with_bl31("NOTICE:  BL31: v2.7(release):v2.7-1-g0123abcd");
        let found = results(&changed, &same);
        assert_eq!(found, [pass.clone(), other(&changed, 6)]);
        assert_eq!(results(&LINES, &same), [pass.clone(), none.clone()]);
        // Without the bootloader line, or with a second one of another
        // release, the bootloader fails.
        let without = [&shown[..1], &shown[2..]].concat();
        assert_eq!(results(&without, &same), [none, pass.clone()]);
        let again =
            "  4.38 RPi: BOOTLOADER release VERSION:0accd665 DATE: 2026/09/24 TIME: 15:59:09";
        let twice = lines_with(|lines| {
            lines.insert(6, BL31);
            lines.insert(2, again);
        });
        assert_eq!(results(&twice, &same), [other(&twice, 2), pass]);
    }

    #[test]
    fn tftp_transfers_are_of_the_bench_and_the_payload() {
        let other = case(&lines_with(|lines| {
            lines[4] = "  5.51 NETWORK: 00:00:5e:00:53:01 wait for link TFTP: 192.0.2.20";
        }));
        assert_eq!(other.result("tftp.server", "servers"), Outcome::Fail);
        let mut outside = case(&LINES);
        outside.parameters.tftp.path_prefix = "boot/".into();
        assert_eq!(outside.result("tftp.prefix", "reads"), Outcome::Fail);
        assert_eq!(
            outside.result("tftp.unplanned_file", "reads"),
            Outcome::Pass
        );
        let unplanned = case(&lines_with(|lines| {
            lines.insert(6, " 16.20 Read kernel.img bytes 3 hnd 0x0")
        }));
        assert_eq!(
            unplanned.result("tftp.unplanned_file", "reads"),
            Outcome::Fail
        );
        let any_case = case(&lines_with(|lines| {
            lines[5] = " 16.03 Read CONFIG.TXT bytes 246 hnd 0x0"
        }));
        assert_eq!(
            any_case.result("tftp.unplanned_file", "reads"),
            Outcome::Pass
        );
    }

    #[test]
    fn identity_lines_that_disagree_fail() {
        let again = "  4.37 BOOTMODE: 0x06 partition 0 build-ts BUILD_TIMESTAMP=1790351949 serial 1234abcd boardrev d04170 stc 4378801";
        let split = case(&lines_with(|lines| lines.insert(3, again)));
        assert_eq!(split.result("identity.disagree", "identity"), Outcome::Fail);
        let same = case(&lines_with(|lines| lines.insert(3, LINES[2])));
        assert_eq!(same.result("identity.disagree", "identity"), Outcome::Pass);
    }

    #[test]
    fn a_cold_boot_ends_in_time_only_after_a_power_on_and_with_an_end() {
        // Without a power on, the end marker did not come by a boot timeout.
        let mut unpowered = case(&LINES);
        unpowered
            .events
            .retain(|e| power::command(e) != Some(Command::On));
        assert_eq!(unpowered.result("boot.end", "ready"), Outcome::Fail);
        // An entry that ends at the timeout needs the end of its capture.
        let mut endless = case(&LINES);
        endless.plan = plan(|text| {
            let text = text.replace("end = { marker = \"ready\" }", "end = \"timeout\"");
            text.replace("boot_timeout_ms = 60000", "boot_timeout_ms = 1000")
        });
        assert_eq!(endless.result("boot.end", "timeout"), Outcome::Pass);
        endless
            .events
            .retain(|e| !matches!(e.event, EventKind::End { .. }));
        assert_eq!(endless.result("boot.end", "timeout"), Outcome::Fail);
    }

    #[test]
    fn an_order_needs_every_marker_each_on_a_later_line() {
        let order = "board.bootloader board.network_boot board.os_start ready";
        let no_first = case(&lines_with(|lines| {
            lines.remove(1);
        }));
        assert_eq!(no_first.result("marker.order", order), Outcome::Fail);
        // Two markers whose first lines are the same line.
        let mut same = case(&LINES);
        same.plan = plan(|text| {
            let ready = "markers = [{ name = \"ready\", line = '^READY$' }";
            let text = text.replace(
                ready,
                &format!("{ready}, {{ name = \"up\", line = '^READY' }}"),
            );
            let orders = "order = [{ markers = [";
            text.replace(
                orders,
                &format!("{orders}\"ready\", \"up\"] }}, {{ markers = ["),
            )
        });
        assert_eq!(same.result("marker.order", order), Outcome::Pass);
        assert_eq!(same.result("marker.order", "ready up"), Outcome::Fail);
    }

    #[test]
    fn the_lines_of_the_bounds_of_a_forbidden_pattern_are_outside_it() {
        let forbid = "{ line = '^Example', from = \"ready\" }";
        let mut bounded = case(&LINES);
        for bound in ["from", "to"] {
            let on_bound = format!("{{ line = '^READY$', {bound} = \"ready\" }}");
            bounded.plan = plan(|text| {
                assert!(text.contains(forbid), "{forbid}");
                text.replace(forbid, &on_bound)
            });
            assert_eq!(
                bounded.result("marker.forbidden", "1"),
                Outcome::Pass,
                "{bound}"
            );
        }
    }

    #[test]
    fn markers_before_the_first_send_pass_when_nothing_is_sent() {
        // Without `READY`, the step is never armed and nothing is sent.
        let unsent = case(&lines_with(|lines| {
            lines.remove(8);
        }));
        assert!(
            !unsent
                .events
                .iter()
                .any(|e| matches!(e.event, EventKind::Send { .. }))
        );
        assert_eq!(
            unsent.result("marker.before_first_send", "ready"),
            Outcome::Pass
        );
    }

    #[test]
    fn each_segment_is_compared_with_the_bytes_of_its_own_lines() {
        let mut two = case(&LINES);
        let mode = Sha256::of(format!("{}\r\n", LINES[2]).as_bytes());
        two.plan = plan(|text| {
            let segment = format!(
                "{{ name = \"mode\", from = \"board.bootloader\", to = \"board.network_boot\", sha256 = \"{mode}\" }}"
            );
            text.replace("segments = [", &format!("segments = [{segment}, "))
        });
        assert_eq!(two.result("segment.digest", "mode"), Outcome::Pass);
        assert_eq!(two.result("segment.digest", "banner"), Outcome::Pass);
    }

    #[test]
    fn a_segment_digest_over_normalised_line_ends_fails() {
        // The segment of the entry is the line between those of the start
        // of the operating system and READY, with its CR LF, as the board
        // wrote them; the digest is of those bytes.
        let raw = case(&LINES);
        let (seen, _) = raw.judged();
        let line = b"Example echo payload\r\n";
        let start = raw
            .serial
            .windows(line.len())
            .position(|w| w == line)
            .unwrap();
        let segment = &seen.segments[0];
        assert_eq!(segment.to_line as usize, start + line.len());
        assert_eq!(
            segment.sha256,
            Sha256::of(&raw.serial[start..start + line.len()])
        );
        assert_eq!(raw.result("segment.digest", "banner"), Outcome::Pass);
        // A digest taken over the line with its line end normalised to LF,
        // or without it, fails.
        let banner = Sha256::of(line).to_string();
        for normalised in [&b"Example echo payload\n"[..], b"Example echo payload"] {
            let mut case = case(&LINES);
            let digest = Sha256::of(normalised).to_string();
            case.plan = plan(|text| text.replace(&banner, &digest));
            assert_eq!(case.result("segment.digest", "banner"), Outcome::Fail);
        }
        // A board that ends its lines with LF alone does not show the digest
        // of the CR LF line, but that of its own bytes.
        let mut lf = case_ended(&LINES, "\n");
        assert_eq!(lf.result("segment.digest", "banner"), Outcome::Fail);
        let digest = Sha256::of(b"Example echo payload\n").to_string();
        lf.plan = plan(|text| text.replace(&banner, &digest));
        assert_eq!(lf.result("segment.digest", "banner"), Outcome::Pass);
    }

    fn check(result: Outcome) -> Check {
        let detail = (result != Outcome::Pass).then(|| "found".to_string());
        Check::new("rule", "subject", result, detail)
    }

    #[test]
    fn verdicts_take_the_worst_result() {
        use Outcome as O;
        use Verdict as V;
        assert_eq!(cold_boot(&[]), V::Pass);
        assert_eq!(cold_boot(&[check(O::Pass), check(O::Fail)]), V::Fail);
        assert_eq!(cold_boot(&[check(O::Fault), check(O::Fail)]), V::BenchFault);
        let boot = |verdict: &V| ColdBootRecord {
            index: 0,
            events: Vec::new(),
            serial: Serial {
                length: 0,
                bytes_base64: Base64::default(),
                index: Vec::new(),
            },
            observations: Observations::default(),
            checks: Vec::new(),
            verdict: *verdict,
        };
        let record = |checks: Vec<Check>, boots: &[V]| EntryRecord {
            index: 0,
            name: "echo".into(),
            reference: None,
            payload: PayloadRecord {
                pin: None,
                generation: None,
                files: Vec::new(),
            },
            events: Vec::new(),
            checks,
            cold_boots: boots.iter().map(boot).collect(),
            verdict: V::NotRun,
        };
        let pass = || vec![check(O::Pass)];
        assert_eq!(entry(&record(Vec::new(), &[]), 3), V::NotRun);
        assert_eq!(entry(&record(pass(), &[V::Pass; 3]), 3), V::Pass);
        // An entry passes only when every planned cold boot ran and passed.
        assert_eq!(entry(&record(pass(), &[V::Pass; 2]), 3), V::BenchFault);
        assert_eq!(entry(&record(pass(), &[V::Pass; 4]), 3), V::BenchFault);
        assert_eq!(entry(&record(pass(), &[V::Pass, V::Fail]), 3), V::Fail);
        assert_eq!(entry(&record(pass(), &[V::BenchFault]), 3), V::BenchFault);
        assert_eq!(entry(&record(vec![check(O::Fault)], &[]), 3), V::BenchFault);
        // A cold boot that claims not to have run is never taken as passing.
        assert_eq!(entry(&record(pass(), &[V::NotRun]), 1), V::BenchFault);
        assert_eq!(
            entry(&record(pass(), &[V::Fail, V::NotRun]), 2),
            V::BenchFault
        );
        assert_eq!(run(&[V::Pass, V::Pass]), V::Pass);
        assert_eq!(run(&[V::Pass, V::Fail, V::Pass]), V::Fail);
        assert_eq!(run(&[V::Fail, V::BenchFault, V::NotRun]), V::BenchFault);
        assert_eq!(run(&[V::NotRun, V::Pass]), V::BenchFault);
    }
}
