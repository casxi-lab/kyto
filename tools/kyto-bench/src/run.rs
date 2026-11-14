//! The flow of a run. Before any power command, the payload of every entry
//! is checked from its source files, then staged and checked as staged; a
//! payload that breaks a rule refuses the run. The run then switches the
//! supply off, and once readings confirm it, runs the entries in order. For
//! each cold boot it takes the steps of the board description in their
//! order: publishing the entry's payload and checking the copy that the
//! board is served, for the first cold boot of the entry; waiting the off
//! interval from the last off confirmation; opening the console; switching
//! the supply on, confirmed by readings. It captures until the cold boot
//! ends, sending each step once its trigger line has appeared, then switches
//! the supply off, confirmed by readings, after every cold boot that
//! started, and closes the console. A cold boot that does not pass ends its
//! entry, and a fault ends the run; when readings did not confirm the last
//! power off, the run switches the supply off once more first.
//!
//! The steps that serve more than one cold boot are functions of their own,
//! which the flow and the commands call: [`check_sources`] and
//! [`prepare_entry`] before any power command, [`publish_entry`], and
//! [`switch_confirmed`] with [`power_off_confirmed`] and
//! [`readings_confirm`].
//!
//! Every wait, timeout and time comes from the bench clock, and the flow
//! ends a cold boot at its deadline whatever the console reports. A console
//! stamps a chunk when it arrives, which can be before the run takes it, so
//! the run takes every chunk stamped before now before it sends a step, and
//! past a deadline it still takes a chunk stamped before that deadline. The
//! rules that judge what happened are those of `kyto_bench_core`, which a
//! reader of the receipt calls again on what it records.
//!
//! An interrupt that the bench catches stops the run: before the first
//! power command, at once; after it, once the cold boot under way has been
//! switched off as usual, and the supply is switched off once more.

use std::panic::{self, AssertUnwindSafe};
use std::rc::Rc;

use kyto_bench_core::board::Step;
use kyto_bench_core::digest::Sha256;
use kyto_bench_core::instance::{Confirmation, PinnedFile};
use kyto_bench_core::names::PayloadName;
use kyto_bench_core::observe::Matcher;
use kyto_bench_core::payload::{self, digests};
use kyto_bench_core::plan::Entry;
use kyto_bench_core::power::{self, Confirming};
use kyto_bench_core::receipt::{
    Command, EndReason, EntryRecord, Event, EventKind, Outcome, Receipt, Refusal, Verdict,
};
use kyto_bench_core::verdict;

use crate::assemble;
use crate::device::{Nanos, PowerState};
use crate::env::{Bench, Env};
use crate::gate::WriteGate;
use crate::inputs::Inputs;
use crate::record::{self, Capture, Got};

/// Why a run gave no receipt.
#[derive(Debug, PartialEq, Eq)]
pub enum NoReceipt {
    /// An input error, before any device was used: the nonce or the bench
    /// could not be had.
    Input(String),
    /// An interrupt stopped the run. One that came after the run's first
    /// power command switched the supply off once more.
    Interrupted,
}

/// Runs the plan of `inputs` on the bench that `env` opens and returns its
/// receipt, refused or judged, or why it gave none.
pub fn run(env: &dyn Env, inputs: &Inputs) -> Result<Receipt, NoReceipt> {
    let nonce = env.nonce().map_err(NoReceipt::Input)?;
    let mut receipt = assemble::start(inputs, nonce, env.tool());
    let mut sources = Vec::new();
    for (index, entry) in inputs.plan.entries.iter().enumerate() {
        let read = inputs.sources(entry);
        if let Err(refusal) = check_sources(inputs, index, &read, &mut receipt.entries[index]) {
            return Ok(assemble::refused(receipt, refusal));
        }
        sources.push(read);
    }
    let (board, instance) = (&inputs.board, &inputs.instance);
    let open = env.open_bench(board, instance, &inputs.instance_dir);
    let mut bench = open.map_err(NoReceipt::Input)?;
    for (index, read) in sources.into_iter().enumerate() {
        let record = &mut receipt.entries[index];
        if let Err(refusal) = prepare_entry(env, inputs, &mut bench, index, read, record) {
            return Ok(assemble::refused(receipt, refusal));
        }
    }
    if bench.interrupt.caught() {
        env.say("the run was interrupted before its first power command");
        return Err(NoReceipt::Interrupted);
    }
    let (off_at, interrupted) = (None, false);
    let mut flow = Flow {
        env,
        inputs,
        bench,
        receipt,
        off_at,
        interrupted,
    };
    // A run that unwinds, or that an interrupt stops, leaves the supply
    // off, as far as the devices let it.
    let ran = panic::catch_unwind(AssertUnwindSafe(|| flow.run_entries()));
    if ran.is_err() || flow.interrupted {
        let why = match ran {
            Err(_) => "the run stopped",
            Ok(()) => "the run was interrupted",
        };
        let confirmation = &instance.parameters().confirmation;
        abort_off(env, &mut flow.bench, confirmation, &mut Vec::new(), why);
    }
    match ran {
        Err(panic) => panic::resume_unwind(panic),
        Ok(()) if flow.interrupted => Err(NoReceipt::Interrupted),
        Ok(()) => Ok(assemble::finish(flow.receipt)),
    }
}

/// Switches the supply off once more after a run stopped with the board
/// perhaps on, records it in `events`, and says on standard error, after
/// `why`, whether readings confirmed it.
fn abort_off(
    env: &dyn Env,
    bench: &mut Bench,
    confirmation: &Confirmation,
    events: &mut Vec<Event>,
    why: &str,
) {
    let off = power_off_confirmed(bench, confirmation, events, 0);
    let not = if off.is_some() { "" } else { "not " };
    env.say(&format!("{why}; readings did {not}confirm the power off"));
}

/// Records the source files `read` of entry `index` of `inputs` in `record`
/// and checks them: [`payload::check_entry`].
pub fn check_sources(
    inputs: &Inputs,
    index: usize,
    read: &[Option<Vec<u8>>],
    record: &mut EntryRecord,
) -> Result<(), Refusal> {
    let (board, entry) = (&inputs.board, &inputs.plan.entries[index]);
    let files = &mut record.payload.files;
    for (file, bytes) in files.iter_mut().zip(read) {
        file.bytes = bytes.as_ref().map(|bytes| bytes.len() as u64);
        file.source_sha256 = bytes.as_deref().map(Sha256::of);
    }
    let sources = digests(files, |file| file.source_sha256.as_ref());
    let refused = |name: &PayloadName| {
        let at = files.iter().position(|file| &file.name == name)?;
        payload::refused_property(board, name, read[at].as_deref()?)
    };
    let (writes, identity) = (&entry.writes, &inputs.instance.parameters().identity);
    let payload = inputs.payload(entry);
    payload::check_entry(index, board, writes, identity, payload, &sources, &refused)
}

/// Stages the source files `read` of entry `index` of `inputs` on the
/// delivery device of `bench`, through the write gate of the entry, reads
/// the staged copy back into `record` and checks it:
/// [`payload::check_staged`]. A payload that the gate refuses, and a failure
/// of the device, leave the copy unread, and `env` says why.
pub fn prepare_entry(
    env: &dyn Env,
    inputs: &Inputs,
    bench: &mut Bench,
    index: usize,
    read: Vec<Option<Vec<u8>>>,
    record: &mut EntryRecord,
) -> Result<(), Refusal> {
    let entry = &inputs.plan.entries[index];
    let payload = inputs.payload(entry);
    let files: Vec<(PayloadName, Vec<u8>)> = payload
        .iter()
        .zip(read)
        .map(|(file, bytes)| (file.name.clone(), bytes.unwrap_or_default()))
        .collect();
    let delivery = &mut *bench.delivery;
    let report = |error: &dyn std::fmt::Display| {
        env.say(&format!("staging the payload of {}: {error}", entry.name));
    };
    match WriteGate::new(&entry.writes, &inputs.board).stage(delivery, &files) {
        Ok(generation) => {
            record.payload.generation = Some(generation);
            for file in &mut record.payload.files {
                match delivery.read_staged(&generation, &file.name) {
                    Ok(bytes) => file.staged_sha256 = Some(Sha256::of(&bytes)),
                    Err(error) => report(&error),
                }
            }
        }
        Err(error) => report(&error),
    }
    let staged = digests(&record.payload.files, |file| file.staged_sha256.as_ref());
    payload::check_staged(index, payload, &staged)
}

/// Makes the board served the staged generation of `record`, the record of
/// an entry whose payload is `payload`, and reads the served copy back into
/// it, with an event for the switch and for each failure; whether every file
/// of the copy has the SHA-256 of the payload ([`payload::check_served`]).
pub fn publish_entry(bench: &mut Bench, payload: &[PinnedFile], record: &mut EntryRecord) -> bool {
    let events = &mut record.events;
    let Some(generation) = record.payload.generation else {
        return false;
    };
    match bench.delivery.switch_to(&generation) {
        Ok(_) => {
            let switch = EventKind::Switch { generation };
            record::add(events, bench.clock.now(), 0, switch);
        }
        Err(error) => {
            record::failed(events, bench.clock.now(), 0, &error);
            return false;
        }
    }
    for file in &mut record.payload.files {
        match bench.delivery.read_served(&file.name) {
            Ok(bytes) => file.served_sha256 = Some(Sha256::of(&bytes)),
            Err(error) => record::failed(events, bench.clock.now(), 0, &error),
        }
    }
    let served = digests(&record.payload.files, |file| file.served_sha256.as_ref());
    let checks = payload::check_served(payload, &served);
    checks.iter().all(|check| check.result == Outcome::Pass)
}

/// Sends `command` to the power switch of `bench`, recorded in `events`
/// after `received` bytes, and confirms it: [`readings_confirm`] from the
/// time it was sent; that time, when readings confirmed it.
pub fn switch_confirmed(
    bench: &mut Bench,
    confirmation: &Confirmation,
    command: Command,
    events: &mut Vec<Event>,
    received: u64,
) -> Option<Nanos> {
    let state = match command {
        Command::On => PowerState::On,
        Command::Off => PowerState::Off,
    };
    let sent = match bench.power.command(state) {
        Ok(sent) => sent,
        Err(error) => {
            record::failed(events, bench.clock.now(), received, &error);
            return None;
        }
    };
    let relay = bench.relay.clone();
    record::add(events, sent, received, EventKind::Power { command, relay });
    readings_confirm(bench, confirmation, command, sent, events, received).then_some(sent)
}

/// Takes a reading every `poll_ms` of `confirmation` from `since` until
/// readings confirm `command` or its timeout has passed, each recorded in
/// `events` after `received` bytes; whether readings confirmed it. A
/// failure of the meter, recorded too, ends the readings.
pub fn readings_confirm(
    bench: &mut Bench,
    confirmation: &Confirmation,
    command: Command,
    since: Nanos,
    events: &mut Vec<Event>,
    received: u64,
) -> bool {
    let mut confirming = Confirming::new(confirmation, command, since.0);
    let (poll, mut next) = (Nanos::from_millis(confirmation.poll_ms), since);
    loop {
        next = bench.clock.wait_until(next.saturating_add(poll));
        let reading = match bench.confirm.read() {
            Ok(reading) => reading,
            Err(error) => {
                record::failed(events, bench.clock.now(), received, &error);
                return false;
            }
        };
        let (quantity, micro) = (bench.confirm.quantity(), reading.micro);
        let kind = EventKind::Reading { quantity, micro };
        record::add(events, reading.at, received, kind);
        if confirming.read(confirmation, quantity, micro, reading.at.0) {
            return true;
        }
        if next.0 >= confirming.deadline() {
            return false;
        }
    }
}

/// Switches the supply off and confirms it: [`switch_confirmed`] with
/// [`Command::Off`].
pub fn power_off_confirmed(
    bench: &mut Bench,
    confirmation: &Confirmation,
    events: &mut Vec<Event>,
    received: u64,
) -> Option<Nanos> {
    switch_confirmed(bench, confirmation, Command::Off, events, received)
}

/// A run on an open bench.
struct Flow<'a> {
    env: &'a dyn Env,
    inputs: &'a Inputs,
    bench: Bench,
    receipt: Receipt,
    /// When readings last confirmed the supply off, if they did after the
    /// last power command.
    off_at: Option<u64>,
    /// Whether an interrupt stopped the run.
    interrupted: bool,
}

impl Flow<'_> {
    /// Whether an interrupt stopped the run: one that the bench caught
    /// stops it from now on.
    fn stop(&mut self) -> bool {
        self.interrupted |= self.bench.interrupt.caught();
        self.interrupted
    }

    /// Switches the supply off before the first entry, then runs the
    /// entries in order until one is a bench fault or an interrupt stops
    /// the run. When readings did not confirm the last power off, which
    /// only a fault leaves, the supply is switched off once more, recorded
    /// in the events of the entry.
    fn run_entries(&mut self) {
        let (inputs, mut start) = (self.inputs, Vec::new());
        let confirmation = &inputs.instance.parameters().confirmation;
        power_off_confirmed(&mut self.bench, confirmation, &mut start, 0);
        self.off_at = power::off_confirmed_at(confirmation, &start);
        self.receipt.start.events = start;
        for (index, entry) in inputs.plan.entries.iter().enumerate() {
            for number in 0..entry.cold_boots as usize {
                if self.cold_boot(index, number) != Some(Verdict::Pass) {
                    break;
                }
            }
            if self.stop() {
                return;
            }
            if self.off_at.is_none() {
                let events = &mut self.receipt.entries[index].events;
                let why = "the supply was switched off again after a fault";
                abort_off(self.env, &mut self.bench, confirmation, events, why);
            }
            let payload = inputs.payload(entry);
            verdict::close_entry(&mut self.receipt, index, payload, entry.cold_boots);
            if self.receipt.entries[index].verdict == Verdict::BenchFault {
                break;
            }
        }
    }

    /// Runs cold boot `number` of entry `index` and records it; its verdict,
    /// or `None` when it could not start: the supply was not confirmed off,
    /// or the payload could not be served or an interrupt came before any
    /// step of the cold boot was recorded. An interrupt takes no further
    /// step, and the supply of a cold boot that started is switched off.
    fn cold_boot(&mut self, index: usize, number: usize) -> Option<Verdict> {
        let (inputs, off_at) = (self.inputs, self.off_at?);
        let (board, entry) = (&inputs.board, &inputs.plan.entries[index]);
        let (parameters, payload) = (inputs.instance.parameters(), inputs.payload(entry));
        let confirmation = &parameters.confirmation;
        let matcher = Matcher::new(board, entry, &parameters.tftp, payload);
        let clock = Rc::clone(&self.bench.clock);
        let mut capture = Capture::new(matcher, entry.max_capture_bytes, Rc::clone(&clock));
        let mut on = None;
        for step in &board.cold_boot.order {
            if self.stop() {
                break;
            }
            let done = match step {
                Step::Publish => {
                    let record = &mut self.receipt.entries[index];
                    number > 0 || publish_entry(&mut self.bench, payload, record)
                }
                Step::WaitOffInterval => {
                    let interval = Nanos::from_millis(board.cold_boot.off_interval_ms);
                    clock.wait_until(Nanos(off_at).saturating_add(interval));
                    true
                }
                Step::OpenConsole => {
                    let ready = Nanos::from_millis(parameters.console.ready_timeout_ms);
                    capture.open(&mut *self.bench.console, clock.now().saturating_add(ready))
                }
                Step::PowerOn => {
                    let events = &mut capture.events;
                    on = switch_confirmed(&mut self.bench, confirmation, Command::On, events, 0);
                    on.is_some()
                }
            };
            if !done {
                break;
            }
        }
        if capture.events.is_empty() {
            return None;
        }
        if let Some(on) = on
            && let Some(reason) = self.capture(&mut capture, entry, on)
        {
            capture.event(EventKind::End { reason });
        }
        let received = capture.received();
        power_off_confirmed(&mut self.bench, confirmation, &mut capture.events, received);
        capture.close(&mut *self.bench.console);
        let record = assemble::cold_boot(inputs, entry, number, capture, off_at);
        self.off_at = power::off_confirmed_at(confirmation, &record.events);
        let verdict = record.verdict;
        self.receipt.entries[index].cold_boots.push(record);
        Some(verdict)
    }

    /// Reads the console of a cold boot of `entry`, switched on at `on`,
    /// sending each step once it is armed, until the cold boot ends; the
    /// reason it ended, or `None` when an interrupt stopped the run. Before
    /// it sends a step, it takes every chunk that the console stamped before
    /// now, which may end the cold boot or show that the step is not due.
    /// A deadline ends it once the bench clock has reached it, whatever the
    /// console reports, after a chunk that the console stamped before the
    /// deadline is taken.
    fn capture(
        &mut self,
        capture: &mut Capture<'_>,
        entry: &Entry,
        on: Nanos,
    ) -> Option<EndReason> {
        let boot = on.saturating_add(Nanos::from_millis(entry.boot_timeout_ms));
        while !self.stop() {
            if capture.matcher.ended() {
                return Some(EndReason::EndCondition);
            }
            let (clock, console) = (&self.bench.clock, &mut *self.bench.console);
            let now = clock.now();
            let deadline = capture
                .matcher
                .deadline()
                .map_or(boot, |step| boot.min(Nanos(step)));
            let got = match capture.matcher.armed() {
                Some(step) if now <= boot => match capture.receive_held(console, now) {
                    Ok(Got::Deadline) => {
                        let bytes = entry.steps()[step].send.as_bytes();
                        capture.send(console, step, bytes).map(|()| Got::Bytes)
                    }
                    got => got,
                },
                _ if now >= deadline => capture.receive_held(console, deadline),
                _ => capture.receive(console, deadline),
            };
            match got {
                Ok(Got::Bytes) => {}
                Ok(Got::Deadline) if clock.now() < deadline => {}
                Ok(Got::Deadline) if deadline == boot => return Some(EndReason::BootTimeout),
                Ok(Got::Deadline) => return Some(EndReason::StepTimeout),
                Ok(Got::Limit(reason)) => return Some(reason),
                Err(error) => {
                    capture.failed(&error);
                    return Some(EndReason::DeviceError);
                }
            }
        }
        None
    }
}
