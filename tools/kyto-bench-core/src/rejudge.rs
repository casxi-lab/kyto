//! Re-deriving the judgement of a receipt from what it records
//! (`docs/bench/formats.md`, re-deriving verdicts). A receipt holds the
//! inputs of every rule: the run plan and the board description as read,
//! the instance parameters, the payload digests, every event and the raw
//! console output of every cold boot. [`rejudge`] reads nothing else.

use crate::board::Board;
use crate::document::check;
use crate::names::PayloadName;
use crate::plan::RunPlan;
use crate::receipt::{Receipt, ReceiptError, Refusal, Verdict, invalid};
use crate::verdict::{self, ColdBoot};
use crate::{evidence, payload, power};

/// The receipt as the rules make it from what `receipt` records: its
/// evidence, its refusal, the checks and verdicts of its entries, the
/// observations, checks and verdicts of its cold boots and its verdict are
/// made again from its inputs, payload digests, events and console output,
/// and the rest is kept. A receipt that the tool wrote equals the result.
///
/// Fails, with the JSON pointer of the place, when a recorded input cannot
/// be read, an index does not describe its bytes, a cold boot follows one
/// of its entry that did not pass, or an entry ran after one that was a
/// bench fault.
pub fn rejudge(receipt: &Receipt) -> Result<Receipt, ReceiptError> {
    let inputs = &receipt.inputs;
    let plan = RunPlan::parse(&inputs.run_plan.text);
    let plan = plan.map_err(|error| invalid("/inputs/run_plan/text", error.to_string()))?;
    let board = Board::parse(&inputs.board_description.text);
    let board =
        board.map_err(|error| invalid("/inputs/board_description/text", error.to_string()))?;
    let parameters = &inputs.instance.parameters;
    let mut again = receipt.clone();
    again.evidence = evidence::eligibility(&receipt.tool, receipt.bench.simulated);
    again.refusal = refusal(receipt, &plan, &board);
    if again.refusal.is_some() {
        again.verdict = None;
        return Ok(again);
    }
    let confirmation = &parameters.confirmation;
    let (mut off_at, mut faulted) = (
        power::off_confirmed_at(confirmation, &receipt.start.events),
        false,
    );
    for (index, entry) in plan.entries.iter().enumerate() {
        let payload = entry.files(&parameters.pins).unwrap_or_default();
        let Some(record) = again.entries.get_mut(index) else {
            break;
        };
        let mut last = Verdict::Pass;
        for (number, cold_boot) in record.cold_boots.iter_mut().enumerate() {
            let at = |key: &str| format!("/entries/{index}/cold_boots/{number}/{key}");
            check! { at("index") => last == Verdict::Pass, "a cold boot follows one that did not pass"; }
            let serial = &cold_boot.serial;
            let (transcript, events) = (&serial.bytes_base64.0[..], &cold_boot.events[..]);
            let index = &serial.index;
            let judged = verdict::judge(&ColdBoot {
                board: &board,
                entry,
                parameters,
                payload,
                transcript,
                index,
                events,
                off_at,
            });
            let (seen, checks) =
                judged.map_err(|error| invalid(at("serial/index"), error.to_string()))?;
            last = verdict::cold_boot(&checks);
            (cold_boot.observations, cold_boot.checks, cold_boot.verdict) = (seen, checks, last);
            off_at = power::off_confirmed_at(confirmation, events);
        }
        verdict::close_entry(&mut again, index, payload, entry.cold_boots);
        let verdict = again.entries[index].verdict;
        let not_run = verdict == Verdict::NotRun;
        check! { format!("/entries/{index}") => !faulted || not_run, "an entry ran after a bench fault"; }
        faulted |= verdict == Verdict::BenchFault;
    }
    let verdicts: Vec<Verdict> = again.entries.iter().map(|entry| entry.verdict).collect();
    again.verdict = Some(verdict::run(&verdicts));
    Ok(again)
}

/// The refusal that the checks before a run make from the recorded payload
/// digests, in their order ([`payload::check_entry`] for every entry, then
/// [`payload::check_staged`] for every entry). A refused property shows only
/// in the payload's bytes, so the receipt's own refusal is taken for it at
/// its place in that order.
fn refusal(receipt: &Receipt, plan: &RunPlan, board: &Board) -> Option<Refusal> {
    let parameters = &receipt.inputs.instance.parameters;
    let entries = || receipt.entries.iter().zip(&plan.entries).enumerate();
    for (index, (record, entry)) in entries() {
        let recorded = |name: &PayloadName| {
            let refusal = receipt.refusal.as_ref()?;
            let found = refusal.rule == "write.refused_property" && refusal.entry == Some(index);
            (found && refusal.subject == name.as_str()).then(|| refusal.detail.clone())
        };
        let payload = entry.files(&parameters.pins).unwrap_or_default();
        let sources = payload::digests(&record.payload.files, |file| file.source_sha256.as_ref());
        let identity = &parameters.identity;
        let checked = payload::check_entry(
            index,
            board,
            &entry.writes,
            identity,
            payload,
            &sources,
            &recorded,
        );
        if let Err(refusal) = checked {
            return Some(refusal);
        }
    }
    entries().find_map(|(index, (record, entry))| {
        let payload = entry.files(&parameters.pins).unwrap_or_default();
        let staged = payload::digests(&record.payload.files, |file| file.staged_sha256.as_ref());
        payload::check_staged(index, payload, &staged).err()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::Sha256;
    use crate::receipt::tests::{PLAN, example};
    use crate::receipt::{ColdBootRecord, EntryRecord, Outcome, PayloadFile, Text};

    /// The files of the entry of the worked example: its `config.txt` and the
    /// device tree files that the D0 stepping of the board of the example
    /// instance configuration needs, all empty.
    const FILES: [&str; 3] = [
        "config.txt",
        "bcm2712-rpi-5-b.dtb",
        "overlays/bcm2712d0.dtbo",
    ];

    fn record(text: String) -> Text {
        Text {
            sha256: Sha256::of(text.as_bytes()),
            text,
        }
    }

    /// The receipt of the worked example with `edit` made to its plan.
    fn worked_with(edit: impl Fn(String) -> String) -> Receipt {
        let mut receipt = example();
        let empty = Sha256::of(b"");
        let file = |name: &&str| format!("{{ name = \"{name}\", sha256 = \"{empty}\" }}");
        let files: Vec<String> = FILES.iter().map(file).collect();
        let one = format!("payload = [{}]", file(&"config.txt"));
        let plan = PLAN.replace(&one, &format!("payload = [{}]", files.join(", ")));
        assert_ne!(plan, PLAN);
        receipt.inputs.run_plan = record(edit(plan));
        let recorded = receipt.entries[0].payload.files[0].clone();
        let named = |name: &&str| PayloadFile {
            name: PayloadName::parse(name).unwrap(),
            ..recorded.clone()
        };
        receipt.entries[0].payload.files = FILES.iter().map(named).collect();
        receipt
    }

    fn worked() -> Receipt {
        worked_with(|plan| plan)
    }

    fn rules(checks: &[crate::receipt::Check]) -> Vec<(&str, &str)> {
        checks
            .iter()
            .map(|check| (check.rule.as_str(), check.subject.as_str()))
            .collect()
    }

    #[test]
    fn the_worked_example_rederives_to_pass() {
        let receipt = worked();
        let again = rejudge(&receipt).unwrap();
        let (entry, cold_boot) = (&again.entries[0], &again.entries[0].cold_boots[0]);
        let recorded = &receipt.entries[0].cold_boots[0];
        assert_eq!(cold_boot.observations, recorded.observations);
        let expected = [
            ("console.not_ready", "console"),
            ("power.on_unconfirmed", "on"),
            ("power.off_interval", "on"),
            ("power.off_unconfirmed", "off"),
            ("power.order", "events"),
            ("boot.network", "board.network_boot"),
            ("boot.fallback", "board.network_boot"),
            ("boot.end", "ready"),
            ("console.byte_limit", "console"),
            ("step.response", "0"),
            ("tftp.server", "servers"),
            ("tftp.prefix", "reads"),
            ("tftp.unplanned_file", "reads"),
            ("identity.disagree", "identity"),
        ];
        assert_eq!(rules(&cold_boot.checks), expected);
        let served = ("payload.served_digest", "config.txt");
        assert_eq!(
            rules(&entry.checks)[..2],
            [("power.off_unconfirmed", "start"), served]
        );
        assert_eq!(entry.checks.len(), 4);
        let checks = entry.checks.iter().chain(&cold_boot.checks);
        assert!(
            checks
                .into_iter()
                .all(|check| check.result == Outcome::Pass)
        );
        let verdicts = (cold_boot.verdict, entry.verdict, again.verdict);
        assert_eq!(
            verdicts,
            (Verdict::Pass, Verdict::Pass, Some(Verdict::Pass))
        );
        assert_eq!(
            (&again.evidence, &again.refusal),
            (&receipt.evidence, &None)
        );
        // The receipt with these checks is read back and re-derives to itself.
        let read = Receipt::parse(again.render().as_bytes()).unwrap();
        assert_eq!(rejudge(&read), Ok(read));
    }

    #[test]
    fn a_judgement_that_the_record_does_not_give_is_found() {
        let receipt = rejudge(&worked()).unwrap();
        type Edit = fn(&mut Receipt);
        let edits: [Edit; 6] = [
            |r| r.verdict = Some(Verdict::Fail),
            |r| r.entries[0].verdict = Verdict::BenchFault,
            |r| {
                r.entries[0].cold_boots[0]
                    .checks
                    .pop()
                    .map(drop)
                    .unwrap_or_default()
            },
            |r| r.entries[0].cold_boots[0].observations.steps[0].response_line = None,
            |r| r.entries[0].checks[0].subject = "begin".into(),
            |r| r.evidence.reasons[0].detail = "simulated".into(),
        ];
        for edit in edits {
            let mut forged = receipt.clone();
            edit(&mut forged);
            assert_eq!(rejudge(&forged), Ok(receipt.clone()));
        }
        // Readings that do not confirm the power on make a bench fault.
        let mut unconfirmed = receipt.clone();
        unconfirmed.entries[0].cold_boots[0].events.remove(2);
        let again = rejudge(&unconfirmed).unwrap();
        assert_eq!(again.entries[0].cold_boots[0].verdict, Verdict::BenchFault);
        assert_eq!(again.verdict, Some(Verdict::BenchFault));
        // So does a power off at the start that was not confirmed.
        let mut start = receipt.clone();
        start.start.events.pop();
        let again = rejudge(&start).unwrap();
        assert_eq!(again.entries[0].checks[0].result, Outcome::Fault);
        assert_eq!(again.entries[0].verdict, Verdict::BenchFault);
    }

    /// `receipt` as the record of a run refused before its first power
    /// command, with the staged and served copies not read.
    fn refused(mut receipt: Receipt, refusal: Option<Refusal>) -> Receipt {
        (receipt.refusal, receipt.verdict) = (refusal, None);
        receipt.start.events.clear();
        let entry = &mut receipt.entries[0];
        (entry.events, entry.checks, entry.cold_boots) = (Vec::new(), Vec::new(), Vec::new());
        entry.verdict = Verdict::NotRun;
        entry.payload.generation = None;
        for file in &mut entry.payload.files {
            (file.staged_sha256, file.served_sha256) = (None, None);
        }
        receipt
    }

    #[test]
    fn a_refusal_is_rederived_from_the_recorded_digests_in_order() {
        let other = Some(Sha256::of(b"other"));
        let mut sources = worked();
        for file in [0, 1] {
            sources.entries[0].payload.files[file].source_sha256 = other.clone();
        }
        let receipt = refused(sources, None);
        let refusal = rejudge(&receipt).unwrap().refusal.unwrap();
        assert_eq!(
            (
                refusal.rule.as_str(),
                refusal.entry,
                refusal.subject.as_str()
            ),
            ("payload.source_digest", Some(0), "bcm2712-rpi-5-b.dtb")
        );
        let receipt = refused(receipt, Some(refusal));
        assert_eq!(rejudge(&receipt), Ok(receipt.clone()));
        assert!(Receipt::parse(receipt.render().as_bytes()).is_ok());
        // A staged copy is checked once every source passed.
        let mut staged = refused(worked(), None);
        let payload = &mut staged.entries[0].payload;
        payload.generation = worked().entries[0].payload.generation;
        for file in &mut payload.files {
            file.staged_sha256 = file.source_sha256.clone();
        }
        payload.files[2].staged_sha256 = other.clone();
        payload.files[0].staged_sha256 = None;
        let refusal = rejudge(&staged).unwrap().refusal.unwrap();
        let found = (refusal.rule.as_str(), refusal.subject.as_str());
        assert_eq!(found, ("payload.staged_digest", "config.txt"));
        assert_eq!(refusal.detail, "the staged copy of config.txt was not read");
        // A run whose copies differ from the plan was refused.
        let mut unrefused = rejudge(&worked()).unwrap();
        unrefused.entries[0].payload.files[0].source_sha256 = other;
        let again = rejudge(&unrefused).unwrap();
        assert_eq!((again.refusal.is_some(), again.verdict), (true, None));
    }

    #[test]
    fn a_refused_property_is_taken_from_the_receipt_at_its_place() {
        let property = |subject: &str| Refusal {
            rule: "write.refused_property".into(),
            entry: Some(0),
            subject: subject.into(),
            detail: "the payload file sets a refused property".into(),
        };
        let receipt = refused(worked(), Some(property("config.txt")));
        assert_eq!(rejudge(&receipt), Ok(receipt.clone()));
        // A rule that comes first in the order is found instead.
        let mut earlier = receipt.clone();
        earlier.entries[0].payload.files[1].source_sha256 = None;
        let refusal = rejudge(&earlier).unwrap().refusal.unwrap();
        assert_eq!(
            (refusal.rule.as_str(), refusal.subject.as_str()),
            ("payload.source_digest", "bcm2712-rpi-5-b.dtb")
        );
        // Of another entry or a file not in the payload, it is not taken:
        // the copies that were not staged refuse the run instead.
        let staged = |receipt: &Receipt| rejudge(receipt).unwrap().refusal.unwrap().rule;
        let mut elsewhere = refused(worked(), Some(property("cmdline.txt")));
        assert_eq!(staged(&elsewhere), "payload.staged_digest");
        elsewhere.refusal.as_mut().unwrap().subject = "config.txt".into();
        elsewhere.refusal.as_mut().unwrap().entry = Some(1);
        assert_eq!(staged(&elsewhere), "payload.staged_digest");
    }

    /// `cold_boot` with every time later by `by` nanoseconds.
    fn later(mut cold_boot: ColdBootRecord, by: u64) -> ColdBootRecord {
        cold_boot
            .events
            .iter_mut()
            .for_each(|event| event.at_ns += by);
        cold_boot
            .serial
            .index
            .iter_mut()
            .for_each(|chunk| chunk[1] += by);
        cold_boot
    }

    /// The worked example with a second entry `again`, whose cold boot is
    /// that of the first `by` nanoseconds later.
    fn two_entries(by: u64) -> Receipt {
        let mut receipt = worked_with(|plan| {
            let entry = &plan[plan.find("[[entries]]").unwrap()..];
            let second = entry.replace("name = \"echo\"", "name = \"again\"");
            format!("{plan}\n{second}")
        });
        let first = receipt.entries[0].clone();
        let cold_boots = vec![later(first.cold_boots[0].clone(), by)];
        let events = first.events.iter().map(|event| {
            let at_ns = event.at_ns + by;
            crate::receipt::Event {
                at_ns,
                ..event.clone()
            }
        });
        let events = events.collect();
        let name = "again".into();
        receipt.entries.push(EntryRecord {
            index: 1,
            name,
            events,
            cold_boots,
            ..first
        });
        rejudge(&receipt).unwrap()
    }

    #[test]
    fn the_off_interval_runs_from_the_last_off_confirmation_of_the_run() {
        let receipt = two_entries(10_000_000_000);
        assert_eq!(receipt.verdict, Some(Verdict::Pass));
        assert_eq!(receipt.entries[1].checks.len(), 3);
        // 7.8 s + 5 s is after the console is ready at 5.1 s + 7 s.
        let early = two_entries(7_000_000_000);
        let checks = &early.entries[1].cold_boots[0].checks;
        let interval = checks
            .iter()
            .find(|check| check.rule == "power.off_interval");
        assert_eq!(interval.unwrap().result, Outcome::Fault);
        assert_eq!(early.entries[1].verdict, Verdict::BenchFault);
        assert_eq!(early.verdict, Some(Verdict::BenchFault));
    }

    #[test]
    fn nothing_runs_after_a_bench_fault_and_no_cold_boot_after_a_failure() {
        let mut receipt = two_entries(10_000_000_000);
        let other = Some(Sha256::of(b"other"));
        receipt.entries[0].payload.files[0].served_sha256 = other;
        let error = rejudge(&receipt).unwrap_err();
        assert_eq!(
            error,
            invalid("/entries/1", "an entry ran after a bench fault")
        );
        let entry = &mut receipt.entries[1];
        (entry.events, entry.cold_boots) = (Vec::new(), Vec::new());
        let again = rejudge(&receipt).unwrap();
        assert_eq!(again.entries[0].verdict, Verdict::BenchFault);
        assert_eq!(again.entries[1].verdict, Verdict::NotRun);
        assert_eq!(again.verdict, Some(Verdict::BenchFault));
        // A second cold boot after one whose power off was not confirmed.
        let mut receipt = worked_with(|plan| plan.replace("cold_boots = 1", "cold_boots = 2"));
        let mut second = later(receipt.entries[0].cold_boots[0].clone(), 10_000_000_000);
        second.index = 1;
        receipt.entries[0].cold_boots[0].events.remove(6);
        receipt.entries[0].cold_boots.push(second);
        let at = "/entries/0/cold_boots/1/index";
        assert_eq!(
            rejudge(&receipt),
            Err(invalid(at, "a cold boot follows one that did not pass"))
        );
        receipt.entries[0].cold_boots.pop();
        let again = rejudge(&receipt).unwrap();
        assert_eq!(again.entries[0].verdict, Verdict::BenchFault);
        // A second cold boot after one that failed: its end marker came
        // after the boot timeout.
        let mut receipt = worked_with(|plan| {
            let plan = plan.replace("cold_boots = 1", "cold_boots = 2");
            plan.replace("boot_timeout_ms = 60000", "boot_timeout_ms = 2000")
        });
        let mut second = later(receipt.entries[0].cold_boots[0].clone(), 10_000_000_000);
        second.index = 1;
        receipt.entries[0].cold_boots.push(second);
        assert_eq!(
            rejudge(&receipt),
            Err(invalid(at, "a cold boot follows one that did not pass"))
        );
        receipt.entries[0].cold_boots.pop();
        let again = rejudge(&receipt).unwrap();
        assert_eq!(again.entries[0].verdict, Verdict::Fail);
    }
}
