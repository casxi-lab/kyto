//! What a cold boot shows: the markers, boot modes, firmware versions,
//! identity values, TFTP transfers, boot files, named values, segments and
//! steps of its console transcript (`docs/bench/formats.md`, how a cold boot
//! is read). A run feeds a [`Matcher`] each chunk as it is received and asks
//! it when to send and whether the cold boot has ended; [`observe`] feeds
//! the same matcher the chunks and sends of a record, so a record gives the
//! same observations as the run.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::board::{Board, Method};
use crate::digest::Sha256;
use crate::instance::{PinnedFile, TftpParameters};
use crate::lines::{self, IndexError, Line, LineSplitter};
use crate::names::{self, PayloadName};
use crate::plan::{BOARD_MARKER, End, Entry};

/// What a cold boot shows, in the order of its lines, each line named by
/// its offset.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observations {
    pub markers: Vec<MarkerSeen>,
    pub boot_modes: Vec<ModeSeen>,
    pub firmware: Vec<FirmwareSeen>,
    pub identity: Vec<Agreement>,
    pub tftp: TftpSeen,
    pub boot_files: Vec<BootFile>,
    pub values: Vec<Agreement>,
    pub segments: Vec<SegmentSeen>,
    pub steps: Vec<StepSeen>,
}

/// A line of a marker: of the entry, or of the board as `board.<name>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkerSeen {
    pub name: String,
    pub line: u64,
    pub at_ns: u64,
}

/// A boot mode line and its mode.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModeSeen {
    pub mode: String,
    pub line: u64,
}

/// A firmware version line and the values of its named groups.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirmwareSeen {
    pub name: String,
    pub line: u64,
    pub groups: Vec<(String, String)>,
}

/// The values that the lines of a cold boot give for an identity field,
/// written `field` or `field.part`, or for a named value: every distinct
/// value in the order it first appears, and every line that gives one. The
/// lines agree when there is exactly one value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Agreement {
    pub field: String,
    pub values: Vec<String>,
    pub lines: Vec<u64>,
}

/// The TFTP lines inside network spans.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TftpSeen {
    pub servers: Vec<ServerSeen>,
    pub reads: Vec<ReadSeen>,
}

/// A line that names the TFTP server, and whether it names the server of
/// the instance configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerSeen {
    pub line: u64,
    pub instance: bool,
}

/// A file read in full: its payload name, `None` when its path is outside
/// the path prefix or names no payload file, and the bytes the line gives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadSeen {
    pub line: u64,
    pub name: Option<PayloadName>,
    pub bytes: Option<u64>,
}

/// A boot file read, with the digest that the payload gives for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootFile {
    pub name: PayloadName,
    pub sha256: Option<Sha256>,
}

/// The digest of a segment, between the lines of its two markers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentSeen {
    pub name: String,
    pub from_line: u64,
    pub to_line: u64,
    pub sha256: Sha256,
}

/// A step: the line that armed it, the index of the event that sent it and
/// the line that answered it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepSeen {
    pub step: usize,
    pub trigger_line: u64,
    pub send_event: Option<usize>,
    pub response_line: Option<u64>,
}

/// Reads a cold boot's transcript as it arrives.
#[derive(Clone, Debug)]
pub struct Matcher<'a> {
    board: &'a Board,
    entry: &'a Entry,
    tftp: &'a TftpParameters,
    payload: &'a [PinnedFile],
    splitter: LineSplitter,
    bytes: Vec<u8>,
    /// Whether the last boot mode line was of the network mode.
    network: bool,
    /// The offset after each line in `seen.markers`.
    marker_ends: Vec<u64>,
    /// Whether the marker that ends the cold boot has appeared.
    end_seen: bool,
    /// Each value of `seen.identity` and `seen.values`, by field.
    known: Known,
    /// The time of the send that waits for its response, and the bytes
    /// received before it.
    waiting: Option<(u64, u64)>,
    seen: Observations,
}

impl<'a> Matcher<'a> {
    /// A matcher at the start of a cold boot of `entry`, whose payload files
    /// are `payload`, on a bench with the TFTP parameters `tftp`.
    pub fn new(
        board: &'a Board,
        entry: &'a Entry,
        tftp: &'a TftpParameters,
        payload: &'a [PinnedFile],
    ) -> Self {
        Self {
            board,
            entry,
            tftp,
            payload,
            splitter: LineSplitter::new(),
            bytes: Vec::new(),
            network: false,
            marker_ends: Vec::new(),
            end_seen: false,
            known: BTreeSet::new(),
            waiting: None,
            seen: Observations::default(),
        }
    }

    /// The number of bytes received so far.
    pub fn received(&self) -> u64 {
        self.splitter.received()
    }

    /// Takes the next chunk of the transcript, received at `at_ns`.
    pub fn push(&mut self, at_ns: u64, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
        let lines = self.splitter.push(at_ns, bytes);
        let after = self.received() - self.splitter.pending().len() as u64;
        for (index, line) in lines.iter().enumerate() {
            self.line(line, lines.get(index + 1).map_or(after, |next| next.offset));
        }
    }

    /// The step whose bytes are due: its trigger line has appeared and it has
    /// not been sent.
    pub fn armed(&self) -> Option<usize> {
        let last = self.seen.steps.last();
        last.filter(|step| step.send_event.is_none())
            .map(|step| step.step)
    }

    /// Records that `step` was sent by the event `event` at `at_ns`, after
    /// the bytes received so far. Only the armed step is recorded.
    pub fn sent(&mut self, step: usize, event: usize, at_ns: u64) {
        if self.armed() == Some(step) {
            self.seen.steps[step].send_event = Some(event);
            self.waiting = Some((at_ns, self.received()));
        }
    }

    /// The time by which the response to the last send must be complete,
    /// while it has not appeared.
    pub fn deadline(&self) -> Option<u64> {
        let (at_ns, _) = self.waiting?;
        let step = &self.entry.steps()[self.seen.steps.len() - 1];
        Some(at_ns.saturating_add(step.timeout_ms.saturating_mul(1_000_000)))
    }

    /// Whether the cold boot has ended: the entry ends at a marker, which has
    /// appeared, and every step has its response.
    pub fn ended(&self) -> bool {
        let steps = &self.seen.steps;
        self.end_seen
            && steps.len() == self.entry.steps().len()
            && steps.iter().all(|step| step.response_line.is_some())
    }

    /// Ends the transcript, reads its unterminated last line, if any, and
    /// returns what the cold boot showed.
    pub fn finish(mut self) -> Observations {
        if let Some(line) = std::mem::take(&mut self.splitter).finish() {
            self.line(&line, line.offset + line.content.len() as u64);
        }
        let seen = &mut self.seen;
        seen.boot_files = boot_files(self.board, &seen.tftp.reads, self.payload);
        let markers: Vec<(&MarkerSeen, u64)> = seen
            .markers
            .iter()
            .zip(self.marker_ends.iter().copied())
            .collect();
        let mut segments = Vec::new();
        for segment in &self.entry.segments {
            let Some(&(from, start)) = markers.iter().find(|(m, _)| m.name == segment.from) else {
                continue;
            };
            let to = markers
                .iter()
                .find(|(m, _)| m.name == segment.to && m.line >= start);
            if let Some((to, _)) = to {
                segments.push(SegmentSeen {
                    name: segment.name.clone(),
                    from_line: from.line,
                    to_line: to.line,
                    sha256: Sha256::of(&self.bytes[start as usize..to.line as usize]),
                });
            }
        }
        seen.segments = segments;
        self.seen
    }

    /// Reads one line, which ends before the offset `after`.
    fn line(&mut self, line: &Line, after: u64) {
        let (board, entry, seen, known) = (self.board, self.entry, &mut self.seen, &mut self.known);
        let (text, offset) = (&line.content[..], line.offset);
        if let Some(mode) = board.boot_mode.form.pattern.group(text, "mode") {
            self.network = mode == board.boot_mode.network.as_bytes();
            let mode = lossy(mode);
            seen.boot_modes.push(ModeSeen { mode, line: offset });
        }
        let board_markers = board
            .markers
            .iter()
            .filter(|m| m.form.pattern.is_match(text));
        let entry_markers = entry.markers.iter().filter(|m| m.line.is_match(text));
        let names = board_markers
            .map(|marker| format!("{BOARD_MARKER}{}", marker.name))
            .chain(entry_markers.map(|marker| marker.name.clone()));
        for name in names {
            self.end_seen |= matches!(&entry.end, End::Marker(end) if end.marker == name);
            let at_ns = line.at_ns;
            seen.markers.push(MarkerSeen {
                name,
                line: offset,
                at_ns,
            });
            self.marker_ends.push(after);
        }
        for version in &board.firmware_versions {
            if let Some(groups) = version.form.pattern.named_groups(text) {
                let groups = groups.into_iter();
                let groups = groups.map(|(group, span)| (group.into(), lossy(&text[span])));
                let (name, groups) = (version.name.clone(), groups.collect());
                seen.firmware.push(FirmwareSeen {
                    name,
                    line: offset,
                    groups,
                });
            }
        }
        for read in &board.identity {
            let value = match &read.method {
                Method::FirmwareLog { form } => form.pattern.group(text, "value").map(lossy),
                Method::FirmwareLogPresence { form, value } => {
                    form.pattern.is_match(text).then(|| value.clone())
                }
                Method::FirmwareVersion { name, group } => board
                    .firmware_version(name)
                    .and_then(|version| version.form.pattern.group(text, group))
                    .map(lossy),
                _ => None,
            };
            let Some(value) = value else { continue };
            for field in read.field.values() {
                if read.part.is_none_or(|part| field.part == Some(part)) {
                    agree(
                        &mut seen.identity,
                        known,
                        false,
                        field.to_string(),
                        &value,
                        offset,
                    );
                }
            }
        }
        if self.network {
            let tftp = &board.tftp;
            let mut servers = tftp.servers.iter();
            if let Some(server) = servers.find_map(|form| form.pattern.group(text, "server")) {
                let instance = server == self.tftp.server_address.as_bytes();
                seen.tftp.servers.push(ServerSeen {
                    line: offset,
                    instance,
                });
            }
            if let Some(path) = tftp.read.pattern.group(text, "path") {
                let name = names::read_path_name(path, self.tftp.path_prefix.as_bytes()).ok();
                let bytes = tftp.read.pattern.group(text, "bytes");
                let bytes = bytes.and_then(|bytes| std::str::from_utf8(bytes).ok()?.parse().ok());
                seen.tftp.reads.push(ReadSeen {
                    line: offset,
                    name,
                    bytes,
                });
            }
        }
        for value in &entry.values {
            if let Some(found) = value.line.group(text, "value") {
                agree(
                    &mut seen.values,
                    known,
                    true,
                    value.field.clone(),
                    &lossy(found),
                    offset,
                );
            }
        }
        self.step(line);
    }

    /// Takes a line as the response of the step that waits for one, then as
    /// the trigger of the next step.
    fn step(&mut self, line: &Line) {
        let steps = self.entry.steps();
        if let (Some((_, received)), Some(seen)) = (self.waiting, self.seen.steps.last_mut())
            && line.offset >= received
            && steps[seen.step].expect.is_match(&line.content)
        {
            seen.response_line = Some(line.offset);
            self.waiting = None;
        }
        let (seen, next) = (&self.seen, self.seen.steps.len());
        let ready = seen
            .steps
            .last()
            .is_none_or(|step| step.response_line.is_some());
        let mut marks = seen
            .markers
            .iter()
            .rev()
            .take_while(|m| m.line == line.offset);
        if ready
            && steps
                .get(next)
                .is_some_and(|step| marks.any(|m| m.name == step.after))
        {
            self.seen.steps.push(StepSeen {
                step: next,
                trigger_line: line.offset,
                send_event: None,
                response_line: None,
            });
        }
    }
}

/// The values of the fields of a list of agreements, `seen.identity`
/// (false) or `seen.values` (true).
type Known = BTreeSet<(bool, String, String)>;

/// Adds `value`, given by the line at `line`, to the values of `field` in
/// `list`, whose values `known` holds under `kind`.
fn agree(
    list: &mut Vec<Agreement>,
    known: &mut Known,
    kind: bool,
    field: String,
    value: &str,
    line: u64,
) {
    let new = known.insert((kind, field.clone(), value.to_string()));
    let index = list.iter().position(|seen| seen.field == field);
    let index = index.unwrap_or_else(|| {
        let (values, lines) = (Vec::new(), Vec::new());
        list.push(Agreement {
            field,
            values,
            lines,
        });
        list.len() - 1
    });
    let seen = &mut list[index];
    if new {
        seen.values.push(value.to_string());
    }
    if seen.lines.last() != Some(&line) {
        seen.lines.push(line);
    }
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The boot files of a cold boot: the files that the board description
/// reads for the identity field `boot_files` and that `reads` show the board
/// read in full, ignoring ASCII case, each once in the order of its first
/// read, with the digest that `payload` gives for the file of that name,
/// ignoring ASCII case; `None` when the payload has no such file.
pub fn boot_files(board: &Board, reads: &[ReadSeen], payload: &[PinnedFile]) -> Vec<BootFile> {
    let listed: Vec<&PayloadName> = board
        .identity
        .iter()
        .flat_map(|read| match &read.method {
            Method::TftpReads { files } => &files[..],
            _ => &[],
        })
        .collect();
    let mut found: Vec<BootFile> = Vec::new();
    for name in reads.iter().filter_map(|read| read.name.as_ref()) {
        let same = |other: &PayloadName| other.eq_ignore_ascii_case(name);
        if listed.iter().any(|file| same(file)) && !found.iter().any(|file| same(&file.name)) {
            let file = payload.iter().find(|file| same(&file.name));
            let sha256 = file.map(|file| file.sha256.clone());
            found.push(BootFile {
                name: name.clone(),
                sha256,
            });
        }
    }
    found
}

/// A send that a record holds: the step it is for, the index of its event
/// among the cold boot's events, its time and the bytes received before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sent {
    pub step: usize,
    pub event: usize,
    pub at_ns: u64,
    pub received: u64,
}

/// What [`observe`] reads: the context of a cold boot and its record.
#[derive(Clone, Copy, Debug)]
pub struct ColdBootInputs<'a> {
    pub board: &'a Board,
    pub entry: &'a Entry,
    pub tftp: &'a TftpParameters,
    pub payload: &'a [PinnedFile],
    /// The bytes received, and the `[offset, at_ns]` of each chunk.
    pub transcript: &'a [u8],
    pub index: &'a [[u64; 2]],
    /// The sends, in the order of their events.
    pub sends: &'a [Sent],
}

/// The observations of a recorded cold boot: the chunks of the transcript
/// and the sends are fed to a [`Matcher`] in the order they happened, each
/// send before the chunk that starts at or after the bytes received before
/// it. Fails when the index does not describe the transcript.
pub fn observe(inputs: &ColdBootInputs<'_>) -> Result<Observations, IndexError> {
    let mut matcher = Matcher::new(inputs.board, inputs.entry, inputs.tftp, inputs.payload);
    let mut sends = inputs.sends.iter().peekable();
    for (at_ns, bytes) in lines::chunks(inputs.transcript, inputs.index)? {
        while let Some(send) = sends.next_if(|send| send.received <= matcher.received()) {
            matcher.sent(send.step, send.event, send.at_ns);
        }
        matcher.push(at_ns, bytes);
    }
    for send in sends {
        matcher.sent(send.step, send.event, send.at_ns);
    }
    Ok(matcher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::RASPBERRY_PI_5;
    use crate::plan::RunPlan;

    const MS: u64 = 1_000_000;

    /// Firmware lines of a Raspberry Pi 5 network boot with documentation
    /// values: the serial number 1234abcd, the board's MAC address
    /// 00:00:5e:00:53:01 and the TFTP server 192.0.2.10.
    const FIRMWARE: [&str; 13] = [
        "  1.08 RPi: BOOTSYS release VERSION:1accd665 DATE: 2026/09/25 TIME: 15:59:09",
        "  1.11 EEPROM ID 0xef4015",
        "  4.37 RPi: BOOTLOADER release VERSION:1accd665 DATE: 2026/09/25 TIME: 15:59:09",
        "  4.37 MFG_VER: 2",
        "  4.37 BOOTMODE: 0x06 partition 0 build-ts BUILD_TIMESTAMP=1790351949 serial 1234abcd boardrev d04171 stc 4378801",
        "  4.94 Boot mode: NETWORK (02) order f1",
        "  5.51 NETWORK: 00:00:5e:00:53:01 wait for link TFTP: 192.0.2.10",
        " 16.03 Read config.txt bytes       75 hnd 0x0",
        " 18.10 Read bcm2712-rpi-5-b.dtb bytes       50 hnd 0x0",
        " 18.14 Read /overlays/bcm2712d0.dtbo bytes       53 hnd 0x0",
        " 18.33 MESS:00:00:18.331117:0: Loaded overlay 'bcm2712d0'",
        " 18.50 Read kernel.img bytes       70 hnd 0x0",
        "NOTICE:  BL31: v2.6(release):v2.6-241-g36761b276",
    ];
    const OS_START: &str = " 26.96 MESS:00:00:26.964387:0: Starting OS 26964 ms";
    const BUILD: &str = "BUILD id=0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c";

    fn pi5() -> Board {
        Board::parse(RASPBERRY_PI_5).unwrap()
    }

    fn plan() -> RunPlan {
        RunPlan::parse(include_str!("../../examples/run-plan.toml")).unwrap()
    }

    fn tftp(prefix: &str) -> TftpParameters {
        let server_address = "192.0.2.10".into();
        TftpParameters {
            server_address,
            path_prefix: prefix.into(),
        }
    }

    /// Lines ended by CR LF, one chunk each, 1 ms apart from 5 s.
    fn chunks(lines: &[&str]) -> Vec<(u64, Vec<u8>)> {
        let chunk = |(index, line): (usize, &&str)| {
            (
                5000 * MS + index as u64 * MS,
                format!("{line}\r\n").into_bytes(),
            )
        };
        lines.iter().enumerate().map(chunk).collect()
    }

    /// The lines of a cold boot of the example's firmware-only entry.
    fn firmware_only() -> Vec<&'static str> {
        [&FIRMWARE[..], &[OS_START, "example kernel started"]].concat()
    }

    /// The lines of a cold boot of the example's echo entry, up to READY.
    fn echo() -> Vec<&'static str> {
        [
            &FIRMWARE[..],
            &[OS_START, "Example echo payload", BUILD, "READY"],
        ]
        .concat()
    }

    /// A record of `chunks` read by a matcher as a run reads them: after
    /// each chunk, an armed step is sent, and `answer` gives the chunks that
    /// follow each send. Returns the record and the matcher's observations.
    struct Record {
        transcript: Vec<u8>,
        index: Vec<[u64; 2]>,
        sends: Vec<Sent>,
        seen: Observations,
        ended: bool,
    }

    fn run(
        entry: &Entry,
        tftp: &TftpParameters,
        chunks: Vec<(u64, Vec<u8>)>,
        answer: &[&str],
    ) -> Record {
        let (board, payload) = (pi5(), entry.files(&[]).unwrap());
        let mut matcher = Matcher::new(&board, entry, tftp, payload);
        let (mut transcript, mut index, mut sends) = (Vec::new(), Vec::new(), Vec::new());
        let mut queue: std::collections::VecDeque<_> = chunks.into();
        while let Some((at_ns, bytes)) = queue.pop_front() {
            index.push([transcript.len() as u64, at_ns]);
            transcript.extend_from_slice(&bytes);
            matcher.push(at_ns, &bytes);
            if let Some(step) = matcher.armed() {
                let received = matcher.received();
                sends.push(Sent {
                    step,
                    event: 10 + step,
                    at_ns,
                    received,
                });
                matcher.sent(step, 10 + step, at_ns);
                let reply = answer
                    .get(step)
                    .map(|line| (at_ns + 2 * MS, format!("{line}\r\n").into_bytes()));
                queue.extend(reply);
            }
            if matcher.ended() {
                break;
            }
        }
        let ended = matcher.ended();
        let seen = matcher.finish();
        Record {
            transcript,
            index,
            sends,
            seen,
            ended,
        }
    }

    fn observed(entry: &Entry, tftp: &TftpParameters, record: &Record) -> Observations {
        let board = pi5();
        let inputs = ColdBootInputs {
            board: &board,
            entry,
            tftp,
            payload: entry.files(&[]).unwrap(),
            transcript: &record.transcript,
            index: &record.index,
            sends: &record.sends,
        };
        observe(&inputs).unwrap()
    }

    fn marker_lines(seen: &Observations, name: &str) -> Vec<u64> {
        seen.markers
            .iter()
            .filter(|m| m.name == name)
            .map(|m| m.line)
            .collect()
    }

    fn agreement(list: &[Agreement], field: &str) -> Option<Vec<String>> {
        list.iter()
            .find(|seen| seen.field == field)
            .map(|seen| seen.values.clone())
    }

    fn name(text: &str) -> PayloadName {
        PayloadName::parse(text).unwrap()
    }

    #[test]
    fn the_example_plan_observes_its_transcripts() {
        let (plan, tftp) = (plan(), tftp(""));
        let entry = &plan.entries[0];
        let record = run(entry, &tftp, chunks(&firmware_only()), &[]);
        assert!(record.ended);
        let seen = &record.seen;
        assert_eq!(observed(entry, &tftp, &record), *seen);
        let offset = |line: usize| {
            firmware_only()[..line]
                .iter()
                .map(|text| text.len() as u64 + 2)
                .sum::<u64>()
        };
        assert_eq!(marker_lines(seen, "board.network_boot"), [offset(5)]);
        assert_eq!(marker_lines(seen, "started"), [offset(14)]);
        assert_eq!(
            seen.markers
                .iter()
                .find(|m| m.name == "started")
                .unwrap()
                .at_ns,
            5014 * MS
        );
        assert_eq!(
            seen.boot_modes,
            [ModeSeen {
                mode: "NETWORK".into(),
                line: offset(5)
            }]
        );
        let names: Vec<&str> = seen
            .firmware
            .iter()
            .map(|line| line.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["bootsys", "bootloader", "bootloader_image_mfg", "bl31"]
        );
        let bootloader = &seen.firmware[1].groups;
        assert_eq!(
            bootloader[..2],
            [
                ("hash".into(), "1accd665".into()),
                ("date".into(), "2026/09/25".into())
            ]
        );
        assert_eq!(
            agreement(&seen.identity, "revision_code"),
            Some(vec!["d04171".into()])
        );
        assert_eq!(
            agreement(&seen.identity, "spi_flash.jedec_id"),
            Some(vec!["ef4015".into()])
        );
        assert_eq!(
            agreement(&seen.identity, "eeprom_version"),
            Some(vec!["2026/09/25".into()])
        );
        assert_eq!(
            agreement(&seen.identity, "soc_stepping"),
            Some(vec!["D0".into()])
        );
        assert_eq!(agreement(&seen.identity, "manufacturing_version"), None);
        assert_eq!(
            seen.tftp.servers,
            [ServerSeen {
                line: offset(6),
                instance: true
            }]
        );
        let reads: Vec<_> = seen
            .tftp
            .reads
            .iter()
            .map(|read| (read.name.clone().unwrap(), read.bytes))
            .collect();
        let expected = [
            ("config.txt", 75),
            ("bcm2712-rpi-5-b.dtb", 50),
            ("overlays/bcm2712d0.dtbo", 53),
            ("kernel.img", 70),
        ];
        assert_eq!(
            reads,
            expected.map(|(file, bytes)| (name(file), Some(bytes)))
        );
        // The boot files carry the digests of the plan's payload.
        let files = entry.files(&[]).unwrap();
        let boot_files = [&files[1], &files[2]].map(|file| BootFile {
            name: file.name.clone(),
            sha256: Some(file.sha256.clone()),
        });
        assert_eq!(seen.boot_files, boot_files);
        assert!(seen.values.is_empty() && seen.segments.is_empty() && seen.steps.is_empty());

        let entry = &plan.entries[1];
        let record = run(entry, &tftp, chunks(&echo()), &["STATS seq=1"]);
        assert!(record.ended);
        let seen = &record.seen;
        assert_eq!(observed(entry, &tftp, &record), *seen);
        let lines = [&echo()[..], &["STATS seq=1"]].concat();
        let offset = |line: usize| {
            lines[..line]
                .iter()
                .map(|text| text.len() as u64 + 2)
                .sum::<u64>()
        };
        assert_eq!(
            record.sends,
            [Sent {
                step: 0,
                event: 10,
                at_ns: 5016 * MS,
                received: offset(17)
            }]
        );
        let step = StepSeen {
            step: 0,
            trigger_line: offset(16),
            send_event: Some(10),
            response_line: Some(offset(17)),
        };
        assert_eq!(seen.steps, [step]);
        assert_eq!(
            seen.values,
            [Agreement {
                field: "build".into(),
                values: vec![BUILD[9..].into()],
                lines: vec![offset(15)]
            }]
        );
        // The segment between the start of the operating system and READY
        // has the digest of the plan.
        let segment = SegmentSeen {
            name: "banner".into(),
            from_line: offset(13),
            to_line: offset(16),
            sha256: entry.segments[0].sha256.clone(),
        };
        assert_eq!(seen.segments, [segment]);
    }

    #[test]
    fn a_step_waits_for_its_trigger_and_its_response_starts_after_the_send() {
        let (plan, tftp) = (plan(), tftp(""));
        let entry = &plan.entries[1];
        // READY and a STATS line in one chunk: the STATS line started before
        // the send, so it is not the response, and the cold boot goes on.
        let mut early = chunks(&echo());
        early
            .last_mut()
            .unwrap()
            .1
            .extend_from_slice(b"STATS seq=1\r\n");
        let record = run(entry, &tftp, early.clone(), &[]);
        assert!(!record.ended);
        assert_eq!(record.seen.steps[0].response_line, None);
        assert_eq!(observed(entry, &tftp, &record), record.seen);
        // The deadline is the time of the send and the step's timeout.
        let board = pi5();
        let mut matcher = Matcher::new(&board, entry, &tftp, entry.files(&[]).unwrap());
        for (at_ns, bytes) in &early[..early.len() - 1] {
            matcher.push(*at_ns, bytes);
            assert_eq!((matcher.armed(), matcher.deadline()), (None, None));
        }
        matcher.sent(0, 3, 1);
        assert_eq!(matcher.finish().steps, []);
        let mut matcher = Matcher::new(&board, entry, &tftp, entry.files(&[]).unwrap());
        for (at_ns, bytes) in &early {
            matcher.push(*at_ns, bytes);
        }
        assert_eq!(matcher.armed(), Some(0));
        matcher.sent(1, 3, 7 * MS);
        assert_eq!(matcher.armed(), Some(0));
        matcher.sent(0, 3, 7 * MS);
        assert_eq!(
            (matcher.armed(), matcher.deadline()),
            (None, Some(1007 * MS))
        );
        assert!(!matcher.ended());
        matcher.push(8 * MS, b"STATS seq=1\r\n");
        assert_eq!(matcher.deadline(), None);
        assert!(matcher.ended());
    }

    #[test]
    fn a_cold_boot_that_ends_at_its_timeout_reads_its_last_line_at_the_end() {
        let (plan, tftp) = (plan(), tftp(""));
        let mut entry = plan.entries[0].clone();
        entry.end = crate::plan::End::Timeout(crate::plan::Timeout::Timeout);
        let mut lines = chunks(&firmware_only());
        lines.last_mut().unwrap().1.truncate(22);
        let record = run(&entry, &tftp, lines, &[]);
        assert!(!record.ended);
        assert_eq!(marker_lines(&record.seen, "started").len(), 1);
        assert_eq!(observed(&entry, &tftp, &record), record.seen);
    }

    #[test]
    fn reads_after_an_sd_boot_mode_are_not_tftp_reads() {
        let (plan, tftp) = (plan(), tftp(""));
        let entry = &plan.entries[0];
        let lines = [
            " 76.99 Boot mode: SD (01) order f",
            " 77.01 Read config.txt bytes       75 hnd 0x0",
            "  5.51 NETWORK: 00:00:5e:00:53:01 wait for link TFTP: 192.0.2.20",
            "  4.94 Boot mode: NETWORK (02) order f1",
            "  5.51 NETWORK: 00:00:5e:00:53:01 wait for link TFTP: 192.0.2.20",
            " 16.03 Read config.txt bytes       75 hnd 0x0",
            " 76.99 Boot mode: SD (01) order f",
            " 77.02 Read kernel.img bytes       70 hnd 0x0",
        ];
        let seen = run(entry, &tftp, chunks(&lines), &[]).seen;
        let modes: Vec<&str> = seen
            .boot_modes
            .iter()
            .map(|mode| mode.mode.as_str())
            .collect();
        assert_eq!(modes, ["SD", "NETWORK", "SD"]);
        let offset = |line: usize| {
            lines[..line]
                .iter()
                .map(|text| text.len() as u64 + 2)
                .sum::<u64>()
        };
        assert_eq!(
            seen.tftp.servers,
            [ServerSeen {
                line: offset(4),
                instance: false
            }]
        );
        let reads: Vec<u64> = seen.tftp.reads.iter().map(|read| read.line).collect();
        assert_eq!(reads, [offset(5)]);
    }

    #[test]
    fn tftp_paths_are_normalized_after_the_instance_prefix() {
        let (plan, tftp) = (plan(), tftp("bench/"));
        let lines = [
            "  4.94 Boot mode: NETWORK (02) order f1",
            " 16.03 Read bench/config.txt bytes       75 hnd 0x0",
            " 18.14 Read bench//overlays/bcm2712d0.dtbo bytes       53 hnd 0x0",
            " 18.20 Read other/kernel.img bytes       70 hnd 0x0",
            " 18.30 Read bench/../kernel.img bytes       70 hnd 0x0",
            " 18.40 Read kernel.img failed",
        ];
        let seen = run(&plan.entries[0], &tftp, chunks(&lines), &[]).seen;
        let reads: Vec<Option<PayloadName>> = seen
            .tftp
            .reads
            .iter()
            .map(|read| read.name.clone())
            .collect();
        assert_eq!(
            reads,
            [
                Some(name("config.txt")),
                Some(name("overlays/bcm2712d0.dtbo")),
                None,
                None
            ]
        );
        assert_eq!(seen.boot_files.len(), 1);
    }

    #[test]
    fn repeated_matches_must_agree() {
        let (plan, tftp) = (plan(), tftp(""));
        let mut entry = plan.entries[1].clone();
        entry.end = crate::plan::End::Timeout(crate::plan::Timeout::Timeout);
        let lines = [
            "  1.11 EEPROM ID 0xef4015",
            "  2.11 EEPROM ID 0xef4015",
            "  4.37 BOOTMODE: 0x06 partition 0 build-ts BUILD_TIMESTAMP=1790351949 serial 1234abcd boardrev d04171 stc 4378801",
            "  4.38 BOOTMODE: 0x06 partition 0 build-ts BUILD_TIMESTAMP=1790351949 serial 1234abcd boardrev d04170 stc 4378802",
            BUILD,
            "BUILD id=1f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c",
        ];
        let seen = run(&entry, &tftp, chunks(&lines), &[]).seen;
        let jedec = seen
            .identity
            .iter()
            .find(|seen| seen.field == "spi_flash.jedec_id")
            .unwrap();
        assert_eq!((jedec.values.len(), jedec.lines.len()), (1, 2));
        assert_eq!(
            agreement(&seen.identity, "revision_code"),
            Some(vec!["d04171".into(), "d04170".into()])
        );
        assert_eq!(
            agreement(&seen.values, "build").map(|values| values.len()),
            Some(2)
        );
    }

    #[test]
    fn many_distinct_values_are_each_kept_once_in_order() {
        let (plan, tftp) = (plan(), tftp(""));
        let mut entry = plan.entries[1].clone();
        entry.end = crate::plan::End::Timeout(crate::plan::Timeout::Timeout);
        let lines: Vec<String> = (0..20_000u32)
            .map(|n| format!("BUILD id={:040x}", n / 2))
            .collect();
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        let seen = run(&entry, &tftp, chunks(&lines), &[]).seen;
        let build = seen
            .values
            .iter()
            .find(|seen| seen.field == "build")
            .unwrap();
        assert_eq!((build.values.len(), build.lines.len()), (10_000, 20_000));
        assert_eq!(build.values[9_999], format!("{:040x}", 9_999));
        // A named value of the name of an identity field keeps its own values.
        entry.values[0] = crate::plan::NamedValue {
            field: "revision_code".into(),
            line: crate::pattern::LinePattern::new("boardrev (?P<value>[0-9a-f]+)").unwrap(),
        };
        let seen = run(&entry, &tftp, chunks(&[FIRMWARE[4]]), &[]).seen;
        for list in [&seen.identity, &seen.values] {
            assert_eq!(
                agreement(list, "revision_code"),
                Some(vec!["d04171".into()])
            );
        }
    }

    #[test]
    fn the_stepping_is_observed_only_from_the_overlay_line() {
        let (plan, tftp) = (plan(), tftp(""));
        let entry = &plan.entries[0];
        let without: Vec<&str> = firmware_only()
            .into_iter()
            .filter(|line| !line.contains("Loaded overlay"))
            .collect();
        let seen = run(entry, &tftp, chunks(&without), &[]).seen;
        assert_eq!(agreement(&seen.identity, "soc_stepping"), None);
        let seen = run(entry, &tftp, chunks(&firmware_only()), &[]).seen;
        assert_eq!(
            agreement(&seen.identity, "soc_stepping"),
            Some(vec!["D0".into()])
        );
    }

    #[test]
    fn mfg_version_is_the_device_value() {
        let (plan, tftp) = (plan(), tftp(""));
        let lines = [
            "  2.92 MFG ver device 1 image 2",
            "  4.37 MFG_VER: 2",
            " 77.23 MFG ver device 1 image 2 ",
        ];
        let seen = run(&plan.entries[0], &tftp, chunks(&lines), &[]).seen;
        let mfg = seen
            .identity
            .iter()
            .find(|seen| seen.field == "manufacturing_version")
            .unwrap();
        assert_eq!(
            (&mfg.values[..], mfg.lines.len()),
            (&["1".to_string()][..], 2)
        );
        assert_eq!(seen.firmware.len(), 1);
        assert_eq!(seen.firmware[0].groups, [("value".into(), "2".into())]);
    }

    #[test]
    fn boot_files_are_the_listed_files_read_with_their_payload_digests() {
        let board = pi5();
        let read = |line, file: &str| ReadSeen {
            line,
            name: Some(name(file)),
            bytes: None,
        };
        let reads = [
            read(0, "config.txt"),
            read(1, "BCM2712-RPI-5-B.DTB"),
            ReadSeen {
                line: 2,
                name: None,
                bytes: None,
            },
            read(3, "bcm2712-rpi-5-b.dtb"),
            read(4, "overlays/bcm2712d0.dtbo"),
            read(5, "overlays/other.dtbo"),
        ];
        let digest = Sha256::of(b"dtb");
        let payload = [PinnedFile {
            name: name("bcm2712-rpi-5-b.dtb"),
            sha256: digest.clone(),
        }];
        let found = boot_files(&board, &reads, &payload);
        let expected = [
            BootFile {
                name: name("BCM2712-RPI-5-B.DTB"),
                sha256: Some(digest),
            },
            BootFile {
                name: name("overlays/bcm2712d0.dtbo"),
                sha256: None,
            },
        ];
        assert_eq!(found, expected);
        assert_eq!(boot_files(&board, &[], &payload), []);
    }

    #[test]
    fn a_changed_index_is_refused() {
        let (plan, tftp) = (plan(), tftp(""));
        let entry = &plan.entries[0];
        let mut record = run(entry, &tftp, chunks(&firmware_only()), &[]);
        record.index[1][0] = 0;
        let board = pi5();
        let inputs = ColdBootInputs {
            board: &board,
            entry,
            tftp: &tftp,
            payload: &[],
            transcript: &record.transcript,
            index: &record.index,
            sends: &[],
        };
        assert_eq!(
            observe(&inputs),
            Err(IndexError::OffsetNotIncreasing { entry: 1 })
        );
    }

    /// The board's behaviour is data: the code that reads transcripts and
    /// checks payloads names nothing of the Raspberry Pi 5.
    #[test]
    fn observation_and_payload_code_name_no_board() {
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
            ("observe.rs", include_str!("observe.rs")),
            ("payload.rs", include_str!("payload.rs")),
        ] {
            let code = &text[..text.find("#[cfg(test)]").unwrap_or(text.len())];
            for literal in literals {
                assert!(!code.contains(literal), "{file} names {literal}");
            }
        }
    }
}
