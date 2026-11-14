//! The receipt of a run, assembled from what the run read and recorded
//! (`docs/bench/formats.md`, receipts). Its observations, checks and
//! verdicts are made by the rules of `kyto_bench_core` from the records
//! alone, with the calls that re-derive them from the receipt.

use kyto_bench_core::digest::Sha256;
use kyto_bench_core::evidence;
use kyto_bench_core::instance::PinnedFile;
use kyto_bench_core::plan::Entry;
use kyto_bench_core::receipt::{
    self, Base64, BoardText, ColdBootRecord, EntryRecord, HardwareRevision, InstanceRecord,
    PayloadFile, PayloadRecord, Receipt, Refusal, RevisionKind, Serial, Start, Text, Tool, Verdict,
};
use kyto_bench_core::verdict::{self, ColdBoot};

use crate::inputs::Inputs;
use crate::provenance::ToolRevision;
use crate::record::Capture;

/// The receipt of a run of `inputs` before anything ran, with `nonce` and
/// the revision `tool`: the inputs of judgement, and every entry of the
/// plan not run, with the files of its payload, none read yet.
pub fn start(inputs: &Inputs, nonce: String, tool: ToolRevision) -> Receipt {
    let (plan, board, instance) = (&inputs.plan, &inputs.board, &inputs.instance);
    let simulated = instance.devices().any_simulated();
    let tool = Tool {
        commit: tool.commit,
        clean: tool.clean,
    };
    let (run_plan, board_text) = (plan.text().to_string(), board.text().to_string());
    Receipt {
        format: receipt::FORMAT.into(),
        format_version: receipt::FORMAT_VERSION,
        nonce,
        evidence: evidence::eligibility(&tool, simulated),
        tool,
        bench: receipt::Bench {
            hardware_revision: HardwareRevision {
                kind: RevisionKind::DevicesSha256,
                value: instance.hardware_revision().clone(),
            },
            simulated,
        },
        inputs: receipt::Inputs {
            run_plan: Text {
                sha256: inputs.plan_sha256.clone(),
                text: run_plan,
            },
            board_description: BoardText {
                id: board.id.clone(),
                sha256: Sha256::of(board_text.as_bytes()),
                text: board_text,
            },
            instance: InstanceRecord {
                sha256: inputs.instance_sha256.clone(),
                parameters: instance.parameters().clone(),
            },
        },
        reference: plan.reference.clone(),
        refusal: None,
        start: Start { events: Vec::new() },
        entries: (0..plan.entries.len())
            .map(|index| entry(inputs, index))
            .collect(),
        verdict: None,
    }
}

/// The record of entry `index` of `inputs` before it ran, with the files of
/// its payload, none read yet.
pub fn entry(inputs: &Inputs, index: usize) -> EntryRecord {
    let entry = &inputs.plan.entries[index];
    let file = |file: &PinnedFile| PayloadFile {
        name: file.name.clone(),
        bytes: None,
        source_sha256: None,
        staged_sha256: None,
        served_sha256: None,
    };
    let (pin, generation) = (entry.payload_pin.clone(), None);
    let files = inputs.payload(entry).iter().map(file).collect();
    EntryRecord {
        index,
        name: entry.name.clone(),
        reference: entry.reference.clone(),
        payload: PayloadRecord {
            pin,
            generation,
            files,
        },
        events: Vec::new(),
        checks: Vec::new(),
        cold_boots: Vec::new(),
        verdict: Verdict::NotRun,
    }
}

/// `receipt`, refused by `refusal` before any power command: it has no
/// verdict.
pub fn refused(mut receipt: Receipt, refusal: Refusal) -> Receipt {
    (receipt.refusal, receipt.verdict) = (Some(refusal), None);
    receipt
}

/// The record of cold boot `number` of `entry` from what `capture`
/// recorded, the supply having been confirmed off before it at `off_at`.
pub fn cold_boot(
    inputs: &Inputs,
    entry: &Entry,
    number: usize,
    capture: Capture<'_>,
    off_at: u64,
) -> ColdBootRecord {
    let (events, bytes, index) = (capture.events, capture.bytes, capture.index);
    let judged = verdict::judge(&ColdBoot {
        board: &inputs.board,
        entry,
        parameters: inputs.instance.parameters(),
        payload: inputs.payload(entry),
        transcript: &bytes,
        index: &index,
        events: &events,
        off_at: Some(off_at),
    });
    // A capture keeps chunks of at least one byte, in order of time.
    let (observations, checks) = judged.expect("the index of a capture describes its bytes");
    let length = bytes.len() as u64;
    ColdBootRecord {
        index: number,
        events,
        serial: Serial {
            length,
            bytes_base64: Base64(bytes),
            index,
        },
        observations,
        verdict: verdict::cold_boot(&checks),
        checks,
    }
}

/// `receipt` of a run that was not refused, with the verdict of the run.
pub fn finish(mut receipt: Receipt) -> Receipt {
    let verdicts: Vec<Verdict> = receipt.entries.iter().map(|entry| entry.verdict).collect();
    receipt.verdict = Some(verdict::run(&verdicts));
    receipt
}
