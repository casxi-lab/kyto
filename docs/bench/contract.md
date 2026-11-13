# Bench tool contract

This page is the contract of the rules of the kyto bench tool that decide a
verdict, check payload digests and the tool's own revision, and power the
board off and report after a failure: the names of the rules, the functions
that apply them, what a run checks before any power command, how the rules
keep their results the same and their work bounded, and the tests of each
[requirement](requirements.md). The [formats page](formats.md) gives the
exact condition of every rule. Requirement ids such as `VERD-R01`,
`PROV-R01` and `POWER-R01` are labels of the requirements of the tool, kept
with the tool, and number nothing else.

## Terms

- **Check**: a rule evaluated on one subject, such as a marker or a payload
  file, recorded as `{ rule, subject, result, detail }` with the result
  `pass`, `fail` or `fault`.
- **Failure**: a check with the result `fail`, a finding about the board or
  its payload.
- **Fault**: a check with the result `fault`, a finding about the bench: its
  devices, the supply or the copy of the payload that the board is served.
- **Refusal**: the first payload check that fails before any power command;
  it ends the run, and the receipt records it in `refusal` and has no
  verdict.
- **Reason**: a rule that makes a receipt not eligible as evidence, listed
  in `evidence.reasons`.
- **Confirmation**: readings of the supply, from the device that reads it,
  that confirm a power command.
- **Bench clock**: the clock of the bench, from which every time, wait and
  timeout of a run comes.
- **Re-deriving**: making the evidence, refusal, observations, checks and
  verdicts of a receipt again from what it records.

## Entry points

The library `kyto-bench-core` holds every rule; the command `kyto-bench`
calls it while it runs a plan, and `rejudge::rejudge` calls the same
functions on a receipt.

| Entry point | Crate | Called by | Requirements |
| --- | --- | --- | --- |
| `payload::check_entry`, with `payload::check_variants` | `kyto-bench-core` | `run::check_sources`; `rejudge::rejudge` | `PROV-R01`, `PROV-R04`, `PROV-R05`, `PROV-R06` |
| `payload::check_staged` | `kyto-bench-core` | `run::prepare_entry`; `rejudge::rejudge` | `PROV-R02`, `PROV-R04` |
| `payload::check_served` | `kyto-bench-core` | `run::publish_entry`; `verdict::entry_checks` | `PROV-R03` |
| `verdict::judge` | `kyto-bench-core` | `assemble::cold_boot`; `rejudge::rejudge` | `VERD-R05` to `VERD-R11`, `POWER-R03` |
| `verdict::entry_checks`, `verdict::close_entry` | `kyto-bench-core` | the flow of `run::run`; `rejudge::rejudge` | `VERD-R02`, `PROV-R03`, `POWER-R03` |
| `verdict::cold_boot`, `verdict::entry`, `verdict::run` | `kyto-bench-core` | `assemble`; `verdict::close_entry`; `rejudge::rejudge` | `VERD-R01`, `VERD-R02`, `VERD-R03` |
| `power::Confirming`, `power::confirmed`, `power::off_confirmed_at` | `kyto-bench-core` | `run::switch_confirmed`; the flow of `run::run`; `power::cold_boot_checks`; `rejudge::rejudge` | `POWER-R01`, `POWER-R02` |
| `power::start_check`, `power::cold_boot_checks`, `power::device_errors` | `kyto-bench-core` | `verdict::entry_checks`; `verdict::judge` | `POWER-R02`, `POWER-R03` |
| `evidence::eligibility` | `kyto-bench-core` | `assemble::start`; `rejudge::rejudge` | `PROV-R07` |
| `receipt::Receipt::parse` | `kyto-bench-core` | readers of a receipt | `VERD-R03`, `VERD-R10`, `PROV-R07` |
| `rejudge::rejudge` | `kyto-bench-core` | readers of a receipt | `VERD-R04`, `VERD-R12`, `PROV-R04` |
| `run::run` | `kyto-bench` | `cli::run` | `VERD-R04`, `PROV-R01`, `PROV-R02`, `POWER-R02`, `POWER-R04` |
| `run::switch_confirmed`, `run::power_off_confirmed` | `kyto-bench` | the flow of `run::run` | `POWER-R01`, `POWER-R02` |
| `cli::run` | `kyto-bench` | the `kyto-bench` binary | `POWER-R05`, `POWER-R06` |
| `provenance::ToolRevision::built`, with the build script | `kyto-bench` | `env::Env::tool` | `PROV-R08` |

## Rule names

Every rule name that the code gives a check, a refusal or a reason, once
each. **Group** is the requirement that the rule serves, whose prefix names
its group of rules; **Result** is what the rule gives when it does not pass:
`fail`, `fault`, `refusal` or `reason`; **Function** is the function that
names it. Reading a receipt does not check rule names, so a rule can be
refined without a new version of the receipt format; a rule that is added,
renamed or removed changes this table and the formats page in the same
change.

| Rule | Group | Result | Function | Formats |
| --- | --- | --- | --- | --- |
| `boot.network` | `VERD-R05` | fail | `verdict::Judging::boot_mode` | [checks](formats.md#checks) |
| `boot.fallback` | `VERD-R05` | fail | `verdict::Judging::boot_mode` | [checks](formats.md#checks) |
| `boot.end` | `VERD-R06` | fail | `verdict::Judging::boot_end` | [checks](formats.md#checks) |
| `console.byte_limit` | `VERD-R11` | fail | `verdict::Judging::capture_limit` | [checks](formats.md#checks) |
| `marker.count` | `VERD-R08` | fail | `verdict::Judging::markers` | [checks](formats.md#checks) |
| `marker.order` | `VERD-R08` | fail | `verdict::Judging::markers` | [checks](formats.md#checks) |
| `marker.forbidden` | `VERD-R08` | fail | `verdict::Judging::markers` | [checks](formats.md#checks) |
| `marker.before_first_send` | `VERD-R08` | fail | `verdict::Judging::markers` | [checks](formats.md#checks) |
| `step.declared_none` | `VERD-R09` | fail | `verdict::Judging::steps` | [checks](formats.md#checks) |
| `step.response` | `VERD-R09` | fail | `verdict::Judging::steps` | [checks](formats.md#checks) |
| `firmware.version` | `VERD-R07` | fail | `verdict::Judging::firmware` | [checks](formats.md#checks) |
| `segment.digest` | `VERD-R10` | fail | `verdict::Judging::segments` | [checks](formats.md#checks) |
| `tftp.server` | `VERD-R11` | fail | `verdict::Judging::transfers` | [checks](formats.md#checks) |
| `tftp.prefix` | `VERD-R11` | fail | `verdict::Judging::transfers` | [checks](formats.md#checks) |
| `tftp.unplanned_file` | `VERD-R11` | fail | `verdict::Judging::transfers` | [checks](formats.md#checks) |
| `identity.disagree` | `VERD-R11` | fail | `verdict::Judging::identity` | [checks](formats.md#checks) |
| `payload.source_digest` | `PROV-R01`, `PROV-R04` | refusal | `payload::check_entry` | [payload checks](formats.md#payload-checks) |
| `write.not_enabled` | `PROV-R05` | refusal | `payload::check_entry` | [payload checks](formats.md#payload-checks) |
| `write.refused_property` | `PROV-R05` | refusal | `payload::check_entry` | [payload checks](formats.md#payload-checks) |
| `payload.variant_missing` | `PROV-R06` | refusal | `payload::check_variants` | [payload checks](formats.md#payload-checks) |
| `payload.staged_digest` | `PROV-R02` | refusal | `payload::check_staged` | [payload checks](formats.md#payload-checks) |
| `payload.served_digest` | `PROV-R03` | fault | `payload::check_served` | [payload checks](formats.md#payload-checks) |
| `evidence.commit_unknown` | `PROV-R07` | reason | `evidence::eligibility` | [evidence](formats.md#evidence) |
| `evidence.tree_changed` | `PROV-R07`, `PROV-R08` | reason | `evidence::eligibility` | [evidence](formats.md#evidence) |
| `evidence.simulated` | `PROV-R07` | reason | `evidence::eligibility` | [evidence](formats.md#evidence) |
| `power.off_unconfirmed` | `POWER-R02`, `POWER-R03` | fault | `power::start_check`, `power::cold_boot_checks` | [the supply](formats.md#the-supply) |
| `console.not_ready` | `POWER-R03` | fault | `power::cold_boot_checks` | [checks](formats.md#checks) |
| `power.on_unconfirmed` | `POWER-R01`, `POWER-R03` | fault | `power::cold_boot_checks` | [the supply](formats.md#the-supply) |
| `power.off_interval` | `POWER-R02`, `POWER-R03` | fault | `power::cold_boot_checks` | [checks](formats.md#checks) |
| `power.order` | `POWER-R03` | fault | `power::cold_boot_checks` | [the supply](formats.md#the-supply) |
| `console.before_power_on` | `POWER-R03` | fault | `power::cold_boot_checks` | [checks](formats.md#checks) |
| `device.error` | `POWER-R03` | fault | `power::device_errors` | [checks](formats.md#checks) |

The rules of [public versions](formats.md#public-versions) name what a
public version replaces or why it refuses one; they decide no verdict and
are not part of this contract.

## Check before power

A run makes every check that can refuse it before its first power command:

1. it reads the source files of every entry's payload and makes the first
   step of the [payload checks](formats.md#payload-checks) before it opens
   the bench (`run::check_sources`);
2. it opens the bench and, for each entry, stages the payload through the
   write gate of the entry, reads the staged copy back and checks it
   (`run::prepare_entry`);
3. only then does it send its first power command, the power off before the
   first entry.

A refusal at either step ends the run there, so a refused run has sent no
power command. The write gate refuses to stage a file that the entry's
enables do not permit, so a payload reaches the board's directory only
through the checked path. The copy that the board is served is checked
after the switch to the entry's generation, which comes only once readings
confirmed the supply off, and before the supply is switched on for the
entry's first cold boot; a copy of another digest is a fault, and that cold
boot is not powered.

## Determinism

- Every time, wait and timeout of a run comes from the bench clock. The
  Clippy configuration of the `tools` workspace bans sleeping and reading
  the system clock, and that of `kyto-bench-core` also bans the entry points
  of the standard library to files, the network, the environment, processes,
  threads, standard input and output and the clock, so the rules read
  nothing but their arguments.
- The checks of an entry and of a cold boot come in the order of
  [checks](formats.md#checks), and the payload checks in the order of
  [payload checks](formats.md#payload-checks); a run is refused under one
  rule only.
- A run and the re-derivation of its receipt call the same functions,
  `payload::check_entry`, `payload::check_staged`, `verdict::judge`,
  `verdict::close_entry`, `verdict::cold_boot`, `verdict::run` and
  `evidence::eligibility`, on the same records, so the receipt that the
  tool wrote is derived again unchanged.
- The verdicts of a cold boot, an entry and a run take the worst result, so
  they do not depend on the order in which checks or verdicts are combined.
- On simulated devices, which share a simulated clock, a run of the same
  plan gives the same receipt every time apart from its nonce.

## Work bounds

- A plan has 1 to 100 entries, each of 1 to 100 cold boots; every list of an
  entry has at most 100 items, and a plan at most 1000 line patterns, both
  checked before any pattern is compiled; a compiled pattern is at most
  64 KiB.
- A cold boot captures at most the `max_capture_bytes` of its entry, at most
  16777216 bytes, in at most 65536 chunks; reaching either limit ends the
  cold boot and fails `console.byte_limit`.
- Every wait of a run ends: the boot timeout and the timeout of each step,
  the timeouts of the confirmations, the time the console may take to be
  ready, and the off interval are each at most one hour, counted on the
  bench clock. A confirmation takes a reading every `poll_ms` until readings
  confirm the command or its timeout has passed.

## Testing obligations

- Every requirement has at least one test in the
  [requirement tests](#requirement-tests), and at least one of them is
  `negative`: it gives a case that breaks the requirement and shows the
  failure, fault, refusal or error that follows. A `positive` test shows
  that a case that keeps the requirement passes.
- The tests run on the host, on records built in code, on simulated devices
  through the command line, or with the built binary on the example. They do
  not sleep or read the system clock; every time in them is written in the
  test or is a time of the simulated bench clock.
- Console output that a test reads holds documentation values only: no
  address, MAC address or serial number of a real board or bench.
- `tools/kyto-bench/tests/contract.rs` reads the
  [requirements page](requirements.md), this page, the
  [formats page](formats.md) and the sources of the tests and of the rules,
  and checks that:
  - every level-three heading of the requirements page is a requirement id,
    two to five capital letters, `-R` and two digits, and the ids of each
    prefix run from `R01` in order without a gap;
  - the requirements of the [requirement tests](#requirement-tests) are
    exactly those ids, each with a `negative` row, and every row names a
    test function of its file;
  - the [rule names](#rule-names) are exactly the rules that the code of
    `verdict`, `power` and `payload` gives checks and refusals and those of
    `receipt::EVIDENCE_RULES`, with the result that the code gives each;
    every group is a requirement id, every function is defined in its
    module, and every rule appears on the formats page.

## Requirement tests

Each row names a test that shows the requirement, by its function and the
file that holds it. **Kind** is `negative` when the test gives a case that
breaks the requirement, and `positive` when it shows only cases that keep
it.

| Requirement | Test | File | Kind |
| --- | --- | --- | --- |
| [VERD-R01](requirements.md#verd-r01) | `verdicts_take_the_worst_result` | `tools/kyto-bench-core/src/verdict.rs` | negative |
| [VERD-R02](requirements.md#verd-r02) | `verdicts_take_the_worst_result` | `tools/kyto-bench-core/src/verdict.rs` | negative |
| [VERD-R03](requirements.md#verd-r03) | `verdicts_take_the_worst_result` | `tools/kyto-bench-core/src/verdict.rs` | negative |
| [VERD-R03](requirements.md#verd-r03) | `a_receipt_has_either_a_refusal_or_a_verdict` | `tools/kyto-bench-core/src/receipt.rs` | negative |
| [VERD-R04](requirements.md#verd-r04) | `rejudging_a_written_receipt_gives_the_recorded_verdicts` | `tools/kyto-bench/tests/run.rs` | negative |
| [VERD-R04](requirements.md#verd-r04) | `a_fault_that_leaves_the_supply_unconfirmed_switches_it_off_again` | `tools/kyto-bench/tests/run.rs` | negative |
| [VERD-R04](requirements.md#verd-r04) | `nothing_runs_after_a_bench_fault_and_no_cold_boot_after_a_failure` | `tools/kyto-bench-core/src/rejudge.rs` | negative |
| [VERD-R05](requirements.md#verd-r05) | `a_boot_that_is_not_from_the_network_fails` | `tools/kyto-bench-core/src/verdict.rs` | negative |
| [VERD-R06](requirements.md#verd-r06) | `the_end_marker_must_appear_by_the_boot_timeout` | `tools/kyto-bench-core/src/verdict.rs` | negative |
| [VERD-R06](requirements.md#verd-r06) | `an_end_marker_without_its_line_feed_ends_no_cold_boot` | `tools/kyto-bench/tests/run.rs` | negative |
| [VERD-R06](requirements.md#verd-r06) | `a_cold_boot_ends_at_its_deadline_whatever_the_console_reports` | `tools/kyto-bench/tests/run.rs` | negative |
| [VERD-R06](requirements.md#verd-r06) | `a_deadline_that_the_console_reports_early_ends_no_cold_boot` | `tools/kyto-bench/tests/run.rs` | positive |
| [VERD-R07](requirements.md#verd-r07) | `firmware_versions_and_segments_are_those_of_the_entry` | `tools/kyto-bench-core/src/verdict.rs` | negative |
| [VERD-R07](requirements.md#verd-r07) | `the_checks_of_the_example_runs_are_fixed` | `tools/kyto-bench/tests/run.rs` | positive |
| [VERD-R08](requirements.md#verd-r08) | `markers_keep_their_counts_order_and_bounds` | `tools/kyto-bench-core/src/verdict.rs` | negative |
| [VERD-R08](requirements.md#verd-r08) | `markers_before_the_first_send_end_before_its_first_byte` | `tools/kyto-bench-core/src/verdict.rs` | negative |
| [VERD-R08](requirements.md#verd-r08) | `the_checks_of_the_example_runs_are_fixed` | `tools/kyto-bench/tests/run.rs` | negative |
| [VERD-R09](requirements.md#verd-r09) | `a_step_needs_its_trigger_its_send_and_a_response_in_time` | `tools/kyto-bench-core/src/verdict.rs` | negative |
| [VERD-R09](requirements.md#verd-r09) | `an_entry_that_declares_no_sends_fails_when_one_is_sent` | `tools/kyto-bench-core/src/verdict.rs` | negative |
| [VERD-R09](requirements.md#verd-r09) | `a_response_without_its_line_feed_is_no_response` | `tools/kyto-bench/tests/run.rs` | negative |
| [VERD-R10](requirements.md#verd-r10) | `receipt_round_trips` | `tools/kyto-bench-core/src/receipt.rs` | positive |
| [VERD-R10](requirements.md#verd-r10) | `a_receipt_with_non_canonical_base64_is_rejected` | `tools/kyto-bench-core/src/receipt.rs` | negative |
| [VERD-R10](requirements.md#verd-r10) | `bytes_are_kept_as_received` | `tools/kyto-bench-core/src/lines.rs` | positive |
| [VERD-R10](requirements.md#verd-r10) | `the_example_plan_observes_its_transcripts` | `tools/kyto-bench-core/src/observe.rs` | positive |
| [VERD-R10](requirements.md#verd-r10) | `a_two_entry_plan_runs_to_pass_on_simulated_devices` | `tools/kyto-bench/tests/run.rs` | positive |
| [VERD-R10](requirements.md#verd-r10) | `firmware_versions_and_segments_are_those_of_the_entry` | `tools/kyto-bench-core/src/verdict.rs` | negative |
| [VERD-R11](requirements.md#verd-r11) | `a_capture_that_reached_its_limit_fails` | `tools/kyto-bench-core/src/verdict.rs` | negative |
| [VERD-R11](requirements.md#verd-r11) | `tftp_transfers_are_of_the_bench_and_the_payload` | `tools/kyto-bench-core/src/verdict.rs` | negative |
| [VERD-R11](requirements.md#verd-r11) | `identity_lines_that_disagree_fail` | `tools/kyto-bench-core/src/verdict.rs` | negative |
| [VERD-R12](requirements.md#verd-r12) | `the_worked_example_rederives_to_pass` | `tools/kyto-bench-core/src/rejudge.rs` | positive |
| [VERD-R12](requirements.md#verd-r12) | `rejudging_a_written_receipt_gives_the_recorded_verdicts` | `tools/kyto-bench/tests/run.rs` | positive |
| [VERD-R12](requirements.md#verd-r12) | `a_judgement_that_the_record_does_not_give_is_found` | `tools/kyto-bench-core/src/rejudge.rs` | negative |
| [PROV-R01](requirements.md#prov-r01) | `source_and_staged_digests_must_equal_the_plan` | `tools/kyto-bench-core/src/payload.rs` | negative |
| [PROV-R01](requirements.md#prov-r01) | `a_source_file_of_another_digest_refuses_the_run_before_the_bench_opens` | `tools/kyto-bench/tests/run.rs` | negative |
| [PROV-R01](requirements.md#prov-r01) | `a_refusal_is_rederived_from_the_recorded_digests_in_order` | `tools/kyto-bench-core/src/rejudge.rs` | negative |
| [PROV-R02](requirements.md#prov-r02) | `source_and_staged_digests_must_equal_the_plan` | `tools/kyto-bench-core/src/payload.rs` | negative |
| [PROV-R02](requirements.md#prov-r02) | `staging_is_checked_before_any_power_command` | `tools/kyto-bench/tests/run.rs` | negative |
| [PROV-R03](requirements.md#prov-r03) | `served_digests_give_one_check_per_file` | `tools/kyto-bench-core/src/payload.rs` | negative |
| [PROV-R03](requirements.md#prov-r03) | `staging_is_checked_before_any_power_command` | `tools/kyto-bench/tests/run.rs` | negative |
| [PROV-R04](requirements.md#prov-r04) | `payload_checks_go_by_file_name_then_by_rule` | `tools/kyto-bench-core/src/payload.rs` | negative |
| [PROV-R04](requirements.md#prov-r04) | `a_refusal_is_rederived_from_the_recorded_digests_in_order` | `tools/kyto-bench-core/src/rejudge.rs` | negative |
| [PROV-R04](requirements.md#prov-r04) | `a_refused_property_is_taken_from_the_receipt_at_its_place` | `tools/kyto-bench-core/src/rejudge.rs` | negative |
| [PROV-R05](requirements.md#prov-r05) | `a_trigger_needs_its_class_enabled` | `tools/kyto-bench-core/src/payload.rs` | negative |
| [PROV-R05](requirements.md#prov-r05) | `a_refused_property_is_refused_even_when_writes_are_enabled` | `tools/kyto-bench-core/src/payload.rs` | negative |
| [PROV-R05](requirements.md#prov-r05) | `a_payload_that_writes_the_eeprom_is_refused_unless_the_plan_enables_it` | `tools/kyto-bench/tests/run.rs` | negative |
| [PROV-R05](requirements.md#prov-r05) | `an_enabled_eeprom_payload_is_staged` | `tools/kyto-bench/tests/run.rs` | positive |
| [PROV-R05](requirements.md#prov-r05) | `a_refused_property_is_refused_even_when_writes_are_enabled` | `tools/kyto-bench/tests/run.rs` | negative |
| [PROV-R05](requirements.md#prov-r05) | `the_built_binary_refuses_an_unenabled_eeprom_payload` | `tools/kyto-bench/tests/release.rs` | negative |
| [PROV-R05](requirements.md#prov-r05) | `the_gate_stages_only_the_files_it_checked_against_its_board` | `tools/kyto-bench/src/gate.rs` | negative |
| [PROV-R05](requirements.md#prov-r05) | `stage_refuses_a_trigger_file_without_a_permit` | `tools/kyto-bench/src/sim/tftp.rs` | negative |
| [PROV-R06](requirements.md#prov-r06) | `a_missing_variant_file_is_refused_when_the_stepping_is_unknown` | `tools/kyto-bench-core/src/payload.rs` | negative |
| [PROV-R06](requirements.md#prov-r06) | `a_stepping_names_its_variant_in_any_ascii_case` | `tools/kyto-bench-core/src/payload.rs` | negative |
| [PROV-R07](requirements.md#prov-r07) | `evidence_needs_a_known_commit_a_clean_tree_and_real_devices` | `tools/kyto-bench-core/src/evidence.rs` | negative |
| [PROV-R07](requirements.md#prov-r07) | `evidence_lists_exactly_the_reasons_that_apply` | `tools/kyto-bench-core/src/receipt.rs` | negative |
| [PROV-R07](requirements.md#prov-r07) | `the_example_runs_to_pass_with_the_built_binary` | `tools/kyto-bench/tests/release.rs` | negative |
| [PROV-R08](requirements.md#prov-r08) | `the_build_script_runs_on_every_build_and_gives_both_values` | `tools/kyto-bench/src/provenance.rs` | positive |
| [PROV-R08](requirements.md#prov-r08) | `provenance_embeds_the_commit_and_clean_flag` | `tools/kyto-bench/src/provenance.rs` | positive |
| [PROV-R08](requirements.md#prov-r08) | `build_values_are_read_strictly` | `tools/kyto-bench/src/provenance.rs` | negative |
| [POWER-R01](requirements.md#power-r01) | `readings_in_a_row_beyond_the_threshold_confirm_a_command_by_its_timeout` | `tools/kyto-bench-core/src/power.rs` | negative |
| [POWER-R01](requirements.md#power-r01) | `a_reading_at_a_threshold_or_at_the_deadline_confirms_its_command` | `tools/kyto-bench-core/src/power.rs` | negative |
| [POWER-R01](requirements.md#power-r01) | `a_command_is_confirmed_by_the_readings_before_the_next_command` | `tools/kyto-bench-core/src/power.rs` | negative |
| [POWER-R01](requirements.md#power-r01) | `confirmation_follows_the_supply_not_the_command` | `tools/kyto-bench/src/sim/meter.rs` | negative |
| [POWER-R01](requirements.md#power-r01) | `confirmation_is_read_from_its_own_device` | `tools/kyto-bench/src/sim/meter.rs` | negative |
| [POWER-R01](requirements.md#power-r01) | `residual_voltage_above_the_off_threshold_is_not_off` | `tools/kyto-bench/tests/run.rs` | negative |
| [POWER-R02](requirements.md#power-r02) | `a_two_entry_plan_runs_to_pass_on_simulated_devices` | `tools/kyto-bench/tests/run.rs` | positive |
| [POWER-R02](requirements.md#power-r02) | `each_cold_boot_waits_the_off_interval_on_the_bench_clock` | `tools/kyto-bench/tests/run.rs` | positive |
| [POWER-R02](requirements.md#power-r02) | `the_switch_happens_only_while_off_is_confirmed` | `tools/kyto-bench/tests/run.rs` | positive |
| [POWER-R02](requirements.md#power-r02) | `a_cold_boot_ends_at_its_deadline_whatever_the_console_reports` | `tools/kyto-bench/tests/run.rs` | positive |
| [POWER-R02](requirements.md#power-r02) | `an_unconfirmed_power_on_ends_at_its_timeout_and_is_switched_off` | `tools/kyto-bench/tests/run.rs` | positive |
| [POWER-R02](requirements.md#power-r02) | `residual_voltage_above_the_off_threshold_is_not_off` | `tools/kyto-bench/tests/run.rs` | negative |
| [POWER-R02](requirements.md#power-r02) | `the_off_interval_runs_from_the_last_off_confirmation_of_the_run` | `tools/kyto-bench-core/src/rejudge.rs` | negative |
| [POWER-R03](requirements.md#power-r03) | `an_unconfirmed_command_or_a_console_not_ready_is_a_fault` | `tools/kyto-bench-core/src/power.rs` | negative |
| [POWER-R03](requirements.md#power-r03) | `steps_after_the_wait_begin_once_the_off_interval_has_passed` | `tools/kyto-bench-core/src/power.rs` | negative |
| [POWER-R03](requirements.md#power-r03) | `events_out_of_order_are_a_fault` | `tools/kyto-bench-core/src/power.rs` | negative |
| [POWER-R03](requirements.md#power-r03) | `console_output_before_the_power_on_is_a_fault` | `tools/kyto-bench-core/src/power.rs` | negative |
| [POWER-R03](requirements.md#power-r03) | `every_device_error_is_a_fault` | `tools/kyto-bench-core/src/power.rs` | negative |
| [POWER-R03](requirements.md#power-r03) | `console_output_before_the_power_on_is_a_bench_fault` | `tools/kyto-bench-core/src/verdict.rs` | negative |
| [POWER-R03](requirements.md#power-r03) | `rejudging_a_written_receipt_gives_the_recorded_verdicts` | `tools/kyto-bench/tests/run.rs` | negative |
| [POWER-R04](requirements.md#power-r04) | `a_fault_that_leaves_the_supply_unconfirmed_switches_it_off_again` | `tools/kyto-bench/tests/run.rs` | negative |
| [POWER-R04](requirements.md#power-r04) | `residual_voltage_above_the_off_threshold_is_not_off` | `tools/kyto-bench/tests/run.rs` | negative |
| [POWER-R05](requirements.md#power-r05) | `a_run_that_panics_switches_the_supply_off_and_leaves_no_output` | `tools/kyto-bench/tests/run.rs` | negative |
| [POWER-R06](requirements.md#power-r06) | `each_result_has_the_exit_status_of_its_own` | `tools/kyto-bench/src/cli.rs` | positive |
| [POWER-R06](requirements.md#power-r06) | `the_command_line_has_the_run_command_only` | `tools/kyto-bench/src/cli.rs` | negative |
| [POWER-R06](requirements.md#power-r06) | `an_output_file_that_cannot_be_written_has_a_status_of_its_own` | `tools/kyto-bench/tests/run.rs` | negative |
| [POWER-R06](requirements.md#power-r06) | `the_example_runs_to_pass_with_the_built_binary` | `tools/kyto-bench/tests/release.rs` | positive |
| [POWER-R06](requirements.md#power-r06) | `the_built_binary_refuses_an_unenabled_eeprom_payload` | `tools/kyto-bench/tests/release.rs` | negative |
| [POWER-R06](requirements.md#power-r06) | `output_that_nothing_reads_changes_no_exit_status` | `tools/kyto-bench/tests/release.rs` | negative |

## Open obligations

| Obligation | Why open | What closes it |
| --- | --- | --- |
| Fuzz tests of the parsing of console transcripts, run plans and receipts | No fuzz test exists | Fuzz tests in the `tools` workspace with a fixed seed and a number of cases written as a constant, run by the `test` step of `cargo xtask check` |
| `VERD-R05` on console output recorded from a real board | Every test reads console output written for the examples | Console output recorded from a Raspberry Pi 5, one without the network boot mode line and one in which the firmware falls back to the SD card, with addresses, MAC addresses and serial numbers replaced by documentation values, each replayed by a run on simulated devices |
| `VERD-R07` for BL31 | No test gives a BL31 version other than the expected one | A negative case of a BL31 version line that differs from the run plan |
| `VERD-R08` below the expected count | No test has fewer lines of a marker than its entry expects | A negative case of `marker.count` below its minimum |
| `VERD-R10` on bytes read back | No test gives a receipt with a lone CR or LF and reads it back, or a segment digested over line ends that were changed | Negative cases of a receipt that keeps bytes that are not UTF-8 and lone CR and LF bytes as read, and of a segment whose digest was taken over changed line ends |
| `PROV-R07` on a run | Only `evidence.simulated` is shown on a run | A run whose tool commit is unknown and whose working tree changed, giving a receipt that is not eligible with both reasons |
| `POWER-R02` after a missing marker | No run test asserts the power off and its confirming reading after a cold boot that fails because a marker of the board is missing while its end marker comes | A run test of that cold boot that asserts the power off command and the reading that confirms it |
| `POWER-R03` for a power off after a cold boot | No run test has a power off after a cold boot that was sent and that readings do not confirm | A run test of that power off that gives `BENCH FAULT` and the exit status 3 |
| `POWER-R01` on a real bench | Power commands are confirmed only by the simulated supply meter | Drivers for a real power device and supply meter, and runs on a bench |

## Current deviations

- **The variants assume the defaults of the device tree properties**
  (`PROV-R06`). The variants of the
  [Raspberry Pi 5 description](formats.md#the-raspberry-pi-5) assume the
  defaults of `upstream_kernel`, `os_prefix`, `overlay_prefix` and
  `device_tree`. A payload whose `config.txt` sets one of them makes the
  firmware request other files, which `payload.variant_missing` does not
  detect before power-on. A cold boot that reads a file outside the payload
  in full, inside the path prefix, still fails `tftp.unplanned_file`.
- **No test builds the tool twice** (`PROV-R08`). That every build asks Git
  again, so that `tool.clean` describes the tree at the build, rests on
  Cargo running a build script again on every build while a path that the
  script watches does not exist. A test checks that the script watches only
  such paths and gives both values, but no test builds the tool, changes its
  working tree and builds it again.
- **The minimum bootloader is information only.**
  `bootloader::minimum` gives the earliest bootloader release that a board
  needs from its identity record, as
  [minimum bootloader](formats.md#minimum-bootloader) describes, but no
  check, refusal or verdict reads it: a board with an older bootloader is
  judged only by the `firmware.version` expectations of the run plan.
