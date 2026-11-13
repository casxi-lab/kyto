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
//! entry, and a fault ends the run.
//!
//! Every wait, timeout and time comes from the bench clock. The rules that
//! judge what happened are those of `kyto_bench_core`, which a reader of
//! the receipt calls again on what it records.

use std::panic::{self, AssertUnwindSafe};
use std::rc::Rc;

use kyto_bench_core::board::Step;
use kyto_bench_core::digest::Sha256;
use kyto_bench_core::names::PayloadName;
use kyto_bench_core::observe::Matcher;
use kyto_bench_core::payload;
use kyto_bench_core::plan::Entry;
use kyto_bench_core::power::{self, Confirming};
use kyto_bench_core::receipt::{
    Command, EndReason, EntryRecord, Event, EventKind, Outcome, Receipt, Refusal, Verdict,
};
use kyto_bench_core::write::WriteClass;

use crate::assemble;
use crate::device::{Nanos, PowerState};
use crate::env::{self, Bench, Env};
use crate::gate::{WriteGate, WritePermit};
use crate::inputs::Inputs;
use crate::record::{self, Capture, Got};

/// Runs the plan of `inputs` on the bench that `env` opens and returns its
/// receipt, refused or judged. The error is an input error, before any
/// device was used: the nonce or the bench could not be had.
pub fn run(env: &dyn Env, inputs: &Inputs) -> Result<Receipt, String> {
    let mut receipt = assemble::start(inputs, env::nonce()?, env.tool());
    let mut sources = Vec::new();
    for (index, entry) in inputs.plan.entries.iter().enumerate() {
        let read = inputs.sources(entry);
        let record = &mut receipt.entries[index];
        if let Err(refusal) = check_sources(inputs, index, &read, record) {
            return Ok(assemble::refused(receipt, refusal));
        }
        sources.push(read);
    }
    let bench = env.open_bench(&inputs.instance, &inputs.instance_dir, &inputs.board)?;
    let mut flow = Flow {
        inputs,
        bench,
        receipt,
        off_at: None,
    };
    for (index, read) in sources.into_iter().enumerate() {
        if let Err(refusal) = flow.stage(index, read) {
            return Ok(assemble::refused(flow.receipt, refusal));
        }
    }
    // A run that unwinds leaves the supply off, as far as the devices let
    // it.
    if let Err(panic) = panic::catch_unwind(AssertUnwindSafe(|| flow.run_entries())) {
        let confirmed = flow.switch(Command::Off, &mut Vec::new(), 0).is_some();
        let not = if confirmed { "" } else { "not " };
        eprintln!("kyto-bench: the run stopped; readings did {not}confirm the power off");
        panic::resume_unwind(panic);
    }
    Ok(assemble::finish(flow.receipt))
}

/// Records the source files `read` of entry `index` in `record` and checks
/// them: [`payload::check_entry`].
fn check_sources(
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
    let sources = assemble::digests(files, |file| &file.source_sha256);
    let refused = |name: &PayloadName| {
        let at = files.iter().position(|file| &file.name == name)?;
        payload::refused_property(board, name, read[at].as_deref()?)
    };
    let (writes, identity) = (&entry.writes, &inputs.instance.parameters().identity);
    let payload = inputs.payload(entry);
    payload::check_entry(index, board, writes, identity, payload, &sources, &refused)
}

/// A run on an open bench.
struct Flow<'a> {
    inputs: &'a Inputs,
    bench: Bench,
    receipt: Receipt,
    /// When readings last confirmed the supply off, if they did after the
    /// last power command.
    off_at: Option<u64>,
}

impl Flow<'_> {
    /// Stages the source files `read` of entry `index`, with the permits
    /// that the entry enables, reads the staged copy back and checks it:
    /// [`payload::check_staged`]. A failure of the delivery device leaves the
    /// copy unread.
    fn stage(&mut self, index: usize, read: Vec<Option<Vec<u8>>>) -> Result<(), Refusal> {
        let entry = &self.inputs.plan.entries[index];
        let payload = self.inputs.payload(entry);
        let files: Vec<(PayloadName, Vec<u8>)> = payload
            .iter()
            .zip(read)
            .map(|(file, bytes)| (file.name.clone(), bytes.unwrap_or_default()))
            .collect();
        let gate = WriteGate::new(&entry.writes);
        let permits: Vec<WritePermit<'_>> = WriteClass::ALL
            .into_iter()
            .filter_map(|class| gate.permit(class).ok())
            .collect();
        let (delivery, record) = (&mut self.bench.delivery, &mut self.receipt.entries[index]);
        let report = |error: &dyn std::fmt::Display| {
            eprintln!("kyto-bench: staging the payload of {}: {error}", entry.name);
        };
        match delivery.stage(&files, &permits) {
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
        let staged = assemble::digests(&record.payload.files, |file| &file.staged_sha256);
        payload::check_staged(index, payload, &staged)
    }

    /// Switches the supply off before the first entry, then runs the
    /// entries in order until one is a bench fault.
    fn run_entries(&mut self) {
        let (inputs, mut start) = (self.inputs, Vec::new());
        self.switch(Command::Off, &mut start, 0);
        let confirmation = &inputs.instance.parameters().confirmation;
        self.off_at = power::off_confirmed_at(confirmation, &start);
        self.receipt.start.events = start;
        for (index, entry) in inputs.plan.entries.iter().enumerate() {
            for number in 0..entry.cold_boots as usize {
                if self.cold_boot(index, number) != Some(Verdict::Pass) {
                    break;
                }
            }
            assemble::close_entry(&mut self.receipt, index, entry, inputs.payload(entry));
            if self.receipt.entries[index].verdict == Verdict::BenchFault {
                break;
            }
        }
    }

    /// Runs cold boot `number` of entry `index` and records it; its verdict,
    /// or `None` when it could not start: the supply was not confirmed off,
    /// or the payload could not be served before any step of the cold boot
    /// was recorded.
    fn cold_boot(&mut self, index: usize, number: usize) -> Option<Verdict> {
        let (inputs, off_at) = (self.inputs, self.off_at?);
        let (board, entry) = (&inputs.board, &inputs.plan.entries[index]);
        let parameters = inputs.instance.parameters();
        let matcher = Matcher::new(board, entry, &parameters.tftp, inputs.payload(entry));
        let clock = Rc::clone(&self.bench.clock);
        let mut capture = Capture::new(matcher, entry.max_capture_bytes, Rc::clone(&clock));
        let mut on = None;
        for step in &board.cold_boot.order {
            let done = match step {
                Step::Publish => number > 0 || self.publish(index),
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
                    on = self.switch(Command::On, &mut capture.events, 0);
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
        if let Some(on) = on {
            let reason = self.capture(&mut capture, entry, on);
            capture.event(EventKind::End { reason });
        }
        let received = capture.received();
        self.switch(Command::Off, &mut capture.events, received);
        capture.close(&mut *self.bench.console);
        let record = assemble::cold_boot(inputs, entry, number, capture, off_at);
        self.off_at = power::off_confirmed_at(&parameters.confirmation, &record.events);
        let verdict = record.verdict;
        self.receipt.entries[index].cold_boots.push(record);
        Some(verdict)
    }

    /// Makes the board served the staged payload of entry `index` and reads
    /// the served copy back; whether every file of it has the SHA-256 of the
    /// plan.
    fn publish(&mut self, index: usize) -> bool {
        let entry = &self.inputs.plan.entries[index];
        let (bench, record) = (&mut self.bench, &mut self.receipt.entries[index]);
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
        let served = assemble::digests(&record.payload.files, |file| &file.served_sha256);
        let checks = payload::check_served(self.inputs.payload(entry), &served);
        checks.iter().all(|check| check.result == Outcome::Pass)
    }

    /// Reads the console of a cold boot of `entry`, switched on at `on`,
    /// sending each step once it is armed, until the cold boot ends; the
    /// reason it ended.
    fn capture(&mut self, capture: &mut Capture<'_>, entry: &Entry, on: Nanos) -> EndReason {
        let boot = on.saturating_add(Nanos::from_millis(entry.boot_timeout_ms));
        let console = &mut *self.bench.console;
        loop {
            if capture.matcher.ended() {
                return EndReason::EndCondition;
            }
            let deadline = capture
                .matcher
                .deadline()
                .map_or(boot, |step| boot.min(Nanos(step)));
            let got = match capture.matcher.armed() {
                Some(step) => {
                    let bytes = entry.steps()[step].send.as_bytes();
                    capture.send(console, step, bytes).map(|()| Got::Bytes)
                }
                None => capture.receive(console, deadline),
            };
            match got {
                Ok(Got::Bytes) => {}
                Ok(Got::Deadline) if deadline == boot => return EndReason::BootTimeout,
                Ok(Got::Deadline) => return EndReason::StepTimeout,
                Ok(Got::Limit(reason)) => return reason,
                Err(error) => {
                    capture.failed(&error);
                    return EndReason::DeviceError;
                }
            }
        }
    }

    /// Sends `command` and takes a reading every `poll_ms` until readings
    /// confirm it or its timeout has passed, each recorded in `events` after
    /// `received` bytes; the time the command was sent, when readings
    /// confirmed it.
    fn switch(
        &mut self,
        command: Command,
        events: &mut Vec<Event>,
        received: u64,
    ) -> Option<Nanos> {
        let confirmation = &self.inputs.instance.parameters().confirmation;
        let bench = &mut self.bench;
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
        let mut confirming = Confirming::new(confirmation, command, sent.0);
        let (poll, mut next) = (Nanos::from_millis(confirmation.poll_ms), sent);
        loop {
            next = bench.clock.wait_until(next.saturating_add(poll));
            let reading = match bench.confirm.read() {
                Ok(reading) => reading,
                Err(error) => {
                    record::failed(events, bench.clock.now(), received, &error);
                    return None;
                }
            };
            let (quantity, micro) = (bench.confirm.quantity(), reading.micro);
            record::add(
                events,
                reading.at,
                received,
                EventKind::Reading { quantity, micro },
            );
            if confirming.read(confirmation, quantity, micro, reading.at.0) {
                return Some(sent);
            }
            if next.0 >= confirming.deadline() {
                return None;
            }
        }
    }
}
