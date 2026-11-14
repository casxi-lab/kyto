//! The commands that use a bench without a run, and write no receipt.
//! `power-off` switches the supply off once, without a retry, and waits for
//! readings to confirm it; it takes no lock and does not stop for an
//! interrupt. `publish` checks the payload of one entry of a run plan,
//! stages it and checks the staged copy, as a run does, then makes it the
//! generation that the board is served, without a power command and only
//! while readings confirm the supply off; it checks the copy that the board
//! is served after the switch and never switches back.

use std::io::{self, Write as _};
use std::path::Path;

use kyto_bench_core::payload::{self, digests};
use kyto_bench_core::receipt::{Command, Event, EventKind, Refusal};

use crate::assemble;
use crate::cli::{BENCH_FAULT, INTERRUPTED, PASS, REFUSED};
use crate::device::Quantity;
use crate::env::Env;
use crate::inputs::{self, Inputs};
use crate::run;

/// What `power-off` says when a run or a publication holds the bench.
const IN_USE: &str = "a run or publication holds this bench; this power off cuts it, and its \
receipt does not show this power off and is no evidence";
/// What `publish` says when it leaves what the board is served unchanged.
const UNCHANGED: &str = "the board is served what it was served before";

/// The command `power-off` on the bench of the instance configuration at
/// `instance`; its exit status. A failure of a device, and readings that do
/// not confirm the power off, are a fault of the bench.
pub fn power_off(env: &dyn Env, instance: &Path) -> Result<u8, String> {
    let read = inputs::read_instance(instance, env.boards())?;
    let mut bench = env.open_bench(&read.board, &read.instance, &read.instance_dir)?;
    if env.bench_in_use(&read.instance) {
        env.say(IN_USE);
    }
    let (confirmation, mut events) = (&read.instance.parameters().confirmation, Vec::new());
    let off = run::power_off_confirmed(&mut bench, confirmation, &mut events, 0);
    let last = last_reading(&events);
    if off.is_some() {
        let _ = writeln!(io::stdout(), "OFF: readings confirmed it; {last}");
        return Ok(PASS);
    }
    say_failures(env, &events);
    let within = confirmation.off_timeout_ms;
    env.say(&format!(
        "readings did not confirm the power off within {within} ms; {last}"
    ));
    Ok(BENCH_FAULT)
}

/// The command `publish` of the entry `name` of the run plan at `plan`,
/// with the instance configuration at `instance` and the payloads below
/// `payload_root`; its exit status.
pub fn publish(
    env: &dyn Env,
    plan: &Path,
    instance: &Path,
    payload_root: &Path,
    name: &str,
) -> Result<u8, String> {
    let inputs = Inputs::read(plan, instance, payload_root, env.boards())?;
    let entries = &inputs.plan.entries;
    let index = entries.iter().position(|entry| entry.name == name);
    let index = index.ok_or_else(|| format!("the run plan has no entry {name}"))?;
    let (entry, mut record) = (&entries[index], assemble::entry(&inputs, index));
    let read = inputs.sources(entry);
    if let Err(refusal) = run::check_sources(&inputs, index, &read, &mut record) {
        return Ok(refused(env, &refusal));
    }
    let (board, config) = (&inputs.board, &inputs.instance);
    let mut bench = env.open_bench(board, config, &inputs.instance_dir)?;
    if let Err(refusal) = run::prepare_entry(env, &inputs, &mut bench, index, read, &mut record) {
        return Ok(refused(env, &refusal));
    }
    let (confirmation, since) = (&config.parameters().confirmation, bench.clock.now());
    let events = &mut record.events;
    if !run::readings_confirm(&mut bench, confirmation, Command::Off, since, events, 0) {
        if say_failures(env, events) {
            return Ok(BENCH_FAULT);
        }
        let within = confirmation.off_timeout_ms;
        env.say(&format!(
            "refused: readings did not confirm the supply off within {within} ms, and publish \
             sends no power command; {UNCHANGED}; run power-off first"
        ));
        return Ok(REFUSED);
    }
    if bench.interrupt.caught() {
        env.say(&format!("interrupted before the switch; {UNCHANGED}"));
        return Ok(INTERRUPTED);
    }
    let payload = inputs.payload(entry);
    if run::publish_entry(&mut bench, payload, &mut record) {
        let generation = record.payload.generation.map(|name| name.to_string());
        let generation = generation.unwrap_or_default();
        let _ = writeln!(io::stdout(), "PUBLISHED: {name} as {generation}");
        return Ok(PASS);
    }
    say_failures(env, &record.events);
    let served = digests(&record.payload.files, |file| file.served_sha256.as_ref());
    let checks = payload::check_served(payload, &served);
    for detail in checks.iter().filter_map(|check| check.detail.as_deref()) {
        env.say(detail);
    }
    let switched = record.events.iter().find_map(|event| match &event.event {
        EventKind::Switch { generation } => Some(generation),
        _ => None,
    });
    if let Some(generation) = switched {
        env.say(&format!(
            "the board is served {generation}, whose copy differs from the run plan; a run \
             switches the supply on only for a generation that it published and checked itself"
        ));
    }
    Ok(BENCH_FAULT)
}

/// Says that `refusal` left what the board is served unchanged; the exit
/// status of a refusal.
fn refused(env: &dyn Env, refusal: &Refusal) -> u8 {
    let (rule, detail) = (&refusal.rule, &refusal.detail);
    env.say(&format!("refused by {rule}: {detail}; {UNCHANGED}"));
    REFUSED
}

/// Says each failure of a device that `events` record; whether there was
/// one.
fn say_failures(env: &dyn Env, events: &[Event]) -> bool {
    let mut failed = false;
    for event in events {
        if let EventKind::DeviceError { role, detail } = &event.event {
            env.say(&format!("the {role} device failed: {detail}"));
            failed = true;
        }
    }
    failed
}

/// What the last reading of `events` read, or that none was taken.
fn last_reading(events: &[Event]) -> String {
    let last = events.iter().rev().find_map(|event| match event.event {
        EventKind::Reading { quantity, micro } => Some((quantity, micro)),
        _ => None,
    });
    match last {
        Some((Quantity::SupplyVoltage, micro)) => format!("the last read {micro} µV"),
        Some((Quantity::SupplyCurrent, micro)) => format!("the last read {micro} µA"),
        None => "no reading was taken".into(),
    }
}
