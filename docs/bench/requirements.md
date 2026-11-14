# Bench tool requirements

This page states what the rules of the kyto bench tool in
[`tools/`](../../tools) must do in three groups: the rules that decide a
verdict, those that check payload digests and the tool's own revision, and
those that power the board off and report after a failure or an interrupt.
The [formats page](formats.md) gives the exact condition of every rule, and
the [contract](contract.md) gives the rule names, the functions that apply
them and the [tests of each requirement](contract.md#requirement-tests).

A requirement is named by the prefix of its group, `-R` and two digits:
`VERD` for verdicts, `PROV` for payload digests and the tool's revision, and
`POWER` for powering off and reporting. Each group is numbered from `R01`.
An id is never renumbered or given to another requirement.

Every requirement is shown so far by tests on the host: unit tests of
`kyto-bench-core` and `kyto-bench` on records and simulated devices built in
code, runs of the command on simulated devices, runs of the built binary on
the example, and fuzz tests that make a fixed number of cases from a fixed
seed. The console output that they read was written for the examples and
the fuzz tests, or comes from two recordings of a Raspberry Pi 5 boot whose
serial number, MAC and IP addresses, TFTP path prefix, SD card identifiers
and partition UUID are replaced by documentation values; the fuzz tests also
read changed copies of it. No test is a run on a real bench. The tests that
remain are the contract's [open obligations](contract.md#open-obligations).

## Verdicts

### VERD-R01

**A cold boot takes the worst result of its checks.** A cold boot is
`BENCH FAULT` when one of its checks is a fault, else `FAIL` when one of
them failed, else `PASS`.

**Evidence.** A unit test of the verdict of a cold boot on checks built in
code: none, a pass with a failure, and a fault with a failure.

### VERD-R02

**An entry is `NOT RUN` only when nothing of it was recorded.** An entry
with no event, no check and no cold boot is `NOT RUN`. Any other entry is
`BENCH FAULT` when one of its checks is a fault or one of its cold boots is
`BENCH FAULT` or `NOT RUN`, else `FAIL` when one of its cold boots is
`FAIL`, else `PASS` when it ran exactly the cold boots that its plan entry
plans. An entry that stopped short of them, or ran more, without a failure
or a fault is `BENCH FAULT`.

**Evidence.** A unit test of the verdict of an entry on records built in
code: an empty entry, entries that ran fewer, all or more of their cold
boots, and entries with a failed, faulted or not run cold boot or a faulted
check.

### VERD-R03

**A run takes the worst of its entries, and a refused run has no
verdict.** A run that was not refused is `BENCH FAULT` when one of its
entries is `BENCH FAULT` or `NOT RUN`, else `FAIL` when one is `FAIL`, else
`PASS`. A run that the payload checks refused has the verdict `null`, no
event before its first entry, and every entry `NOT RUN`, with no event, no
check and no cold boot. Every receipt has exactly one of a refusal and a
verdict.

**Evidence.** A unit test of the verdict of a run on verdicts built in
code, and a test in which reading a receipt refuses one with both a refusal
and a verdict, with neither, with the run verdict `NOT RUN`, or with a
refused run that has events, checks, an entry verdict or cold boots.

### VERD-R04

**A failed cold boot ends its entry, and a fault ends the run.** A cold
boot that does not pass is the last of its entry. After a failed cold boot,
the next entry runs once readings confirmed the power off; a power off that
they do not confirm is a fault. A fault ends the run with the entry that
holds it, and every entry after it is `NOT RUN`. Re-deriving the verdicts of
a receipt refuses one in which a cold boot follows one of its entry that
did not pass, or an entry that is not `NOT RUN` follows one that is
`BENCH FAULT`.

**Evidence.** Runs on simulated devices in which the second of three cold
boots of the first entry fails and the second entry runs and passes, and in
which a power on or a power command is not confirmed or fails and the
second entry is `NOT RUN`; runs of a plan of three entries in which the
second cold boot of the first entry fails, after which the other two
entries run, or its power on is not confirmed, after which both are
`NOT RUN`; and tests in which re-deriving refuses receipts that go on after
a failure or a fault. The failures and faults come from simulated devices
and the example console output.

### VERD-R05

**A boot that is not from the network fails.** `boot.network` fails when no
boot mode line of the cold boot is of the board's network mode, and
`boot.fallback` fails when the last boot mode line is of another mode, such
as a line of the SD card mode after the network one.

**Evidence.** A unit test on console lines built in code: the network boot
mode line left out, and a line of the SD card mode after it; and runs on
simulated devices that replay two console recordings of a Raspberry Pi 5:
a cold boot cut before its network boot mode line, in an entry that ends at
its boot timeout, which fails `boot.network` alone, and a boot that falls
back to the SD card after its TFTP requests go unanswered, which fails
`boot.fallback` alone.

### VERD-R06

**The end of a cold boot comes by its boot timeout.** With an end marker,
`boot.end` fails unless a complete line of the marker, ended by its LF,
appeared within `boot_timeout_ms` of the power on. With `end = "timeout"`,
it fails when the cold boot ended before the boot timeout. A run ends a cold
boot at its boot timeout on the bench clock, whatever the console reports.

Past that time it still reads a chunk that the console stamped before it,
and a chunk stamped at that time or later ends the cold boot as the time
does. A chunk stamped exactly at a deadline is thus kept only when the run
was already waiting for it before its bench clock reached that deadline.

**Evidence.** A unit test of an end marker that comes late, never comes or
has no LF, and of an entry that ends at the timeout; runs on simulated
devices whose end marker has no LF or never comes, which end at the boot
timeout and fail, and whose console reports deadlines early or never; a run
on a console that holds a response stamped before the deadline of its step
when that deadline has passed, which is read and answers the step; a run
on a console that hands a chunk stamped after a boot timeout that has
passed, which ends the cold boot at its boot timeout; and a run on a
console that hands the line that arms a step, stamped before the boot
timeout, once the boot timeout has passed, which sends no step and ends
the cold boot at its boot timeout.

### VERD-R07

**Firmware versions are those that the run plan expects.** For each
firmware version line that an entry names, `firmware.version` fails when no
line of it appeared or a line of it does not give every value that the
entry expects. The expected values come from the run plan alone.

**Evidence.** A unit test in which the bootloader line gives another hash
or is missing; a unit test in which the run plan expects another hash or
date of the bootloader or another BL31 version, or the console output gives
another BL31 version, no BL31 line, no bootloader line or a second
bootloader line of another release, each of which fails that version alone;
and runs on simulated devices in which the bootloader and BL31 lines give
the expected values.

### VERD-R08

**Markers keep their counts, order and bounds.** `marker.count` fails when
a marker appeared another number of times than its entry expects;
`marker.order` when a marker of an order did not appear or the first lines
of the markers are not in that order; `marker.forbidden` when a line between
the bounds of a forbidden pattern matches it; and `marker.before_first_send`
when a byte was sent before the first line of the marker, its line ending
included, had ended.

**Evidence.** Unit tests on console lines built in code: a marker that
appears twice, never, or fewer times than the minimum of its entry, markers
swapped or missing, forbidden lines anywhere and between bounds, and a send
one byte too early; and runs on simulated devices whose check lists,
failing ones included, are fixed.

### VERD-R09

**Steps are armed, sent and answered in time, and an entry that declares no
sends sends nothing.** `step.response` fails for a step that its trigger
line did not arm, that was not sent, or whose response line was not
complete within its `timeout_ms` of the send. In an entry with
`sends = false`, `step.declared_none` fails when a byte was sent. A run
reads every chunk that the console holds before it sends a step, and records
every send between two chunks of its cold boot; reading a receipt refuses a
send inside a chunk.

**Evidence.** Unit tests of a slow, missing, unsent and unarmed response, of
a line that the board began before the send and that is therefore no
response, and of a send in an entry that declares none; a run on simulated
devices whose response has no LF; a run on a console that holds a line that
came after the trigger line but before the send, which is no response; a
receipt whose send, the first of its cold boot or a second one, was moved
inside a chunk, which reading refuses at that send; a fuzz test in which the steps that the record of a run gives are
those that the rules give from its lines and sends; and a fuzz test that
moves the first send of each written receipt inside a chunk.

### VERD-R10

**Console output is kept as received, and segments are digested over it.**
A receipt keeps the bytes of a cold boot's console output exactly as they
arrived: bytes that are not UTF-8, CR and LF included, with no line ending
changed and nothing decoded, and the times of its chunks apart; reading the
receipt gives the same bytes back. `segment.digest` fails when the two lines
of a segment did not both appear or the SHA-256 of the bytes between them,
CR and LF included, is not the one that the entry gives.

**Evidence.** Tests that a receipt reads back as written, that lines keep
bytes that are not UTF-8, that a segment's digest covers its CR LF line
ends, and that reading refuses base64 that is not canonical; a run on
simulated devices whose receipt holds the example console output, which
starts with a byte that is not UTF-8 and ends its lines with CR LF, byte for
byte; and a unit test of a segment whose content changed or whose line is
missing. Tests of receipts whose console output holds a line of every byte
value but LF and sequences that are not UTF-8, or lines ended by CR LF, by
CR CR LF and by LF alone, lone CR bytes and a CR and its LF in two chunks:
read back, each holds the same bytes and gives the observations it
records, while the same output decoded as UTF-8 with U+FFFD in place of
what is not UTF-8, or with its line ends normalised to LF, breaks the
recorded length and, with the length, index and events made to agree,
gives other observations. A unit test in which a segment digest taken over
its line with the line end normalised to LF, or without it, fails
`segment.digest`, and a board that ends its lines with LF alone shows the
digest of its own bytes. Fuzz tests of changed console output read in
chunks of any size, whose lines put back together are its bytes and each of
whose segments is the SHA-256 of the raw bytes between its lines, and of
the receipts of runs on simulated devices whose board writes changed
output, which hold the bytes that the console gave, up to the capture
limit.

### VERD-R11

**A capture at its limit, another TFTP server or file, and identity values
that disagree fail.** `console.byte_limit` fails when the cold boot ended at
its capture limit of bytes or chunks; `tftp.server` when a line names
another TFTP server than the instance's; `tftp.prefix` when a file read in
full is outside the path prefix; `tftp.unplanned_file` when a file read in
full inside the prefix is not a file of the entry's payload, ignoring ASCII
case; and `identity.disagree` when the lines of an identity field or part
give different values.

**Evidence.** Unit tests on console lines and events built in code: an end
at either limit, another server, a read outside the prefix, a read of a
file outside the payload and one in another letter case, and two values of
the board revision.

### VERD-R12

**Judgement reads only the receipt.** The evidence, refusal, observations,
checks and verdicts of a run follow from what its receipt records: the run
plan and board description as read, the instance parameters, the digests
of the payload files, the events and the console output of every cold
boot. The library makes them again from a receipt alone, without a payload,
an instance configuration, a device or a clock, and gives a receipt that
the tool wrote back unchanged.

**Evidence.** Tests that re-deriving gives back the worked example and the
receipts of passing, failing, faulted and refused runs on simulated
devices, and that it finds a verdict, check, observation or subject that the
record does not give; and fuzz tests in which the record of a run on
changed console output, read in chunks of any size, gives the observations
that the run saw, also with other chunks that keep every send where it
came, and in which the receipts of runs on simulated devices, refused and
faulted runs among them, re-derive to themselves, re-deriving a receipt
whose judgements alone were changed, its refusal among them, gives back
the receipt as written, and re-deriving one in which an entry after a
bench fault was given the events or cold boots of that entry fails.

## Payload digests and the tool's revision

### PROV-R01

**A source file of another digest refuses the run before the bench
opens.** `payload.source_digest` compares the SHA-256 of every source file,
as the tool read it, with that of the run plan. A file that differs or
cannot be read refuses the run before the bench is opened, so nothing is
staged and no power command is sent, and the refusal names the rule, the
entry, the file and what was found.

**Evidence.** A unit test of a source file of another digest and of one
that was not read, with the details they give; a run on simulated devices
whose plan gives another digest for a payload file, refused under
`payload.source_digest` with its entry, file and detail before the bench is
opened, so that nothing is staged and no power command is sent; a run on
simulated devices whose staged copy differs while its source files keep
their digests, which is refused under `payload.staged_digest` only after
the copy was staged; a test that re-deriving gives the same refusal from
the recorded digests; and publications on simulated devices refused in the
same way, whose source file differs, whose staged copy differs or whose
TFTP directory cannot be staged, which serve nothing new and send no power
command.

### PROV-R02

**The staged copy is checked as read back, before any power command.**
Once the source checks of every entry have passed, each entry's payload is
staged, through the write gate of its entry, where the board will read it,
read back, and checked by `payload.staged_digest` against the run plan. A
file of the copy that differs, or that cannot be staged or read back,
refuses the run before any power command.

**Evidence.** A run on simulated devices whose TFTP directory changes a
staged file: the run is refused under `payload.staged_digest` with its
entry and file, after the copy was staged and before any power command;
and a unit test of the details.

### PROV-R03

**A served copy of another digest is a fault of the bench.** Once a switch
shows that the board is served the entry's payload, `payload.served_digest`
checks every file of the copy that the board is served, read through the
served generation, one check for each file. A file that differs or was not
read is a fault: the entry is `BENCH FAULT`, the board is not switched on
for it, and the run ends. `kyto-bench publish` checks the copy that the
board is served after its switch in the same way, reports a copy of another
digest as a fault of the bench, with the exit status 3, every file that
differs and the generation now served, and does not switch the board back.

**Evidence.** A unit test of one check for each file; a run on simulated
devices whose TFTP directory serves changed files: four faults, no cold
boot and only the power off at the start; and publications on simulated
devices of one entry, whose served copy has the digests of the plan, and
whose TFTP directory serves changed files, a fault with every file
reported and no power command.

### PROV-R04

**Payload checks go in a fixed order, refuse under one rule, and are made
again from the receipt.** For each entry in the order of the plan and each
file in increasing order of the bytes of the payload names,
`payload.source_digest`, `write.not_enabled` and `write.refused_property`
are checked, then `payload.variant_missing` for the entry; once these pass
for every entry, `payload.staged_digest` is checked for each entry and file
in the same order. The first check that fails refuses the run under its
rule alone. Re-deriving makes the checks again in this order from the
recorded digests, except that a refusal under `write.refused_property`,
which shows only in the bytes of a file, is taken as the receipt records it,
at its place in the order.

**Evidence.** Unit tests of the order of files and rules, and tests that
re-deriving gives the refusal of the first failing check, and takes a
recorded `write.refused_property` only where no earlier check fails.

### PROV-R05

**Writes are off unless an entry enables them, and a refused property is
always refused.** `write.not_enabled` refuses a payload file that the board
description lists as a write trigger of a class, `firmware`, `eeprom` or
`storage`, that the entry does not enable. `write.refused_property` refuses a
payload file that sets a refused property of the board, whatever the entry
enables. Both refuse before anything is staged or powered, and the write
gate refuses to stage such a file as well.

**Evidence.** Unit tests of an EEPROM image with its class enabled, with
another class enabled and with none, and of a refused property with every
class enabled; runs on simulated devices and of the built binary in which an
EEPROM image is refused unless its entry enables the class, and a refused
property is refused with every class enabled, before anything is staged and
without a power command; and tests of the write gate and of the simulated
TFTP directory.

### PROV-R06

**A payload holds the files of the board's variant.**
`payload.variant_missing` refuses a payload that lacks a file of the variant
whose value the identity record gives for the variant's field, ignoring
ASCII case, or of any variant of a field that the record gives as unknown.
The variants list files under the default names of the device tree
properties only, one of the contract's
[current deviations](contract.md#current-deviations); the
[Raspberry Pi 5 description](formats.md#the-raspberry-pi-5) states what
follows for a payload that changes them.

**Evidence.** Unit tests of payloads with and without the files of the C1
and D0 variants, with the stepping known, unknown and written in lower
case.

### PROV-R07

**A receipt is evidence only from a known, clean tool on real devices.** A
receipt lists `evidence.commit_unknown` when the tool's commit is unknown,
`evidence.tree_changed` when its working tree had changes or whether it had
is unknown, and `evidence.simulated` when a device of the bench is
simulated, each with what was found, and it is eligible as evidence exactly
when none applies. Reading a receipt refuses one whose evidence does not
list exactly the reasons that apply.

**Evidence.** A unit test of each reason alone and of all three, a test in
which reading refuses evidence that leaves a reason out or calls the
receipt eligible, runs on simulated devices whose tool revision has an
unknown commit, a working tree with changes or of unknown state, or both,
whose receipts list each reason that applies and whose receipts and public
versions are not eligible, and runs of the built binary on simulated
devices, whose receipts are not eligible.

### PROV-R08

**The tool's revision comes from its build alone.** Every build of the
tool asks Git for the commit and whether the working tree had changes, and
embeds them; nothing at run time or in the environment of the build
overrides them. The tree counts as clean only when `git status` shows no
change, no untracked file included, no tracked file is marked
assume-unchanged or skip-worktree, and no untracked file is hidden by
anything but the `.gitignore` files of the tree. A value that Git cannot
give, or that has another form, is unknown, so `tool.commit` and
`tool.clean` describe the tree at the build that made the binary.

**Evidence.** A test that reads what the build script gave Cargo for the
build of the tests: it watches only paths that do not exist, so that Cargo
runs it on every build, and it gives both values, those that the build
embedded; a test that a build in a Git checkout embeds both values; and a
test that a commit or a clean flag that is empty or of another form is
unknown.

## Power off and reporting

### POWER-R01

**A power command is confirmed by readings of the supply, never by the
relay's reply.** A power command is confirmed when `consecutive` readings
in a row of the configured quantity, from the device that reads the supply,
are at or beyond the threshold of the commanded state, all taken from the
command to its timeout. A reading of another quantity or on the other side
of the threshold starts the count again, and the power device's report that
it sent a command confirms nothing.

**Evidence.** Unit tests of the count, the thresholds, the timeout, stale
readings and the readings up to the next command; tests of the simulated
meter, which reads the supply and not the command, and fails on its own;
a run on simulated devices whose supply keeps a voltage above the off
threshold; the power-off command on simulated devices, whose power off
readings confirm, and whose meter reads a supply that is on whatever the
supply does, or cannot be opened, each then a fault of the bench with the
one power off sent; and a test of the INA228 driver on a simulated bridge,
in which a relay whose contacts stick and a supply that keeps a voltage
above the off threshold confirm nothing.

### POWER-R02

**The supply is confirmed off before the first cold boot and after every
cold boot that started.** A run switches the supply off before its first
entry and after every cold boot that started, one that failed or faulted
included, and waits for readings to confirm it. A cold boot starts only
once the supply was confirmed off, and its steps after the off interval
begin once `off_interval_ms` has passed since that confirmation.
`kyto-bench publish` switches the served generation only while readings
confirm the supply off, and sends no power command.

**Evidence.** Runs on simulated devices: the commands and readings of a
passing run, the off interval and the switch of every entry, a cold boot
that fails at its boot timeout, one that fails because the network boot
marker of the board is missing while its end marker comes, and one whose
power on is not confirmed, each switched off and confirmed off, and a
supply that is never confirmed off, after which no cold boot starts; a
test of the off interval across entries in a re-derived receipt; and a
publication on simulated devices whose supply is on, which is refused
without a switch and without a power command.

### POWER-R03

**A fault of the bench makes the verdict `BENCH FAULT`, never `PASS` and
never `FAIL`.** A console that is not ready, a power on or power off that
readings do not confirm, a step that starts within the off interval, events
out of order, console output before the power on, and an error of a device
are faults. A cold boot or an entry with a fault is `BENCH FAULT`, and so
is the run, whatever the other checks give; the checks of the board and its
payload are still made and recorded, and the failures that they record in
such a cold boot do not make it `FAIL`.

**Evidence.** Unit tests of each fault and of a cold boot whose console
output came while the supply was off, which is `BENCH FAULT` while its end
marker passes; and runs on simulated devices whose power on is not
confirmed, which is `BENCH FAULT` with the exit status 3 and no verdict
`FAIL` although checks of the board fail, whose power off after a passing
cold boot is not confirmed, which is `BENCH FAULT` with the exit status 3
and leaves the next entry `NOT RUN`, or whose power device fails.

### POWER-R04

**A fault that leaves the supply unconfirmed switches it off once more.**
When a fault ends the run before readings confirmed the last power off, the
run sends a power off once more, records it, its readings and any error of
a device in the events of the entry that holds the fault, and says on
standard error whether readings confirmed it.

**Evidence.** Runs on simulated devices whose power device fails to send
the first or third command, and whose supply is never confirmed off: the
power off sent again, its readings, the state of the supply and what the
run says on standard error, once confirmed and once not; and a unit test of
the exact bytes of a message on standard error.

### POWER-R05

**An error of the tool while the entries run switches the supply off, and
any error of the tool leaves no output.** When the run panics while it runs
its entries, the tool switches the supply off once more and says on
standard error whether readings confirmed it. A panic before then, while
the payloads are checked and staged or the bench is opened, sends no power
command and leaves the supply as it was. Any panic removes the output
directory, leaves no receipt and exits with status 101.

**Evidence.** A run on simulated devices that panics after the first power
on: the power off sent again, a supply at 0 V, the message on standard
error, and no output directory; and a unit test of the exact bytes of a
message on standard error.

### POWER-R06

**Each command's exit status gives its result.** `kyto-bench run` exits
with 0 for `PASS`, 1 for `FAIL`, 2 for a usage or input error or a receipt
that cannot be written, 3 for `BENCH FAULT`, 4 for a run refused before any
power command, 5 when the receipt was written but not its public version,
101 for an error of the tool itself and 130 for an interrupt.
`kyto-bench power-off` exits with 0 when readings confirmed the power off, 2
for a usage or input error, 3 for `BENCH FAULT`, when readings did not
confirm the power off or a device failed, and 101 for an error of the tool.
`kyto-bench publish` exits with 0 when the board is served the new
generation and its copy has the digests of the run plan, 2 for a usage or
input error, 3 for `BENCH FAULT`, when the meter, the switch or the read of
the served copy failed or the copy that the board is served after the
switch differs, 4 when a payload check, the staging of the payload or the
readings refused it, leaving what the board is served unchanged, 101 for an
error of the tool and 130 for an interrupt before the switch. A message or
result that cannot be written changes no exit status.

**Evidence.** Tests of the values of the statuses, of the three commands
and of the usage errors; runs on simulated devices whose output files
cannot be written, or that an interrupt stops; the power-off command whose
power off readings do not confirm, or whose meter cannot be opened, and a
publication whose served copy differs or whose meter cannot be opened,
each with the status 3; publications refused with the status 4; and runs of
the built binary that pass, are refused, have no reader of their output, or
cannot read their plan, which is said on standard error, and that switch
the supply off or publish, which say so on standard output.

### POWER-R07

**An interrupt stops a run on real devices with the supply off.** When
SIGINT, SIGTERM or SIGHUP reaches a run on a bench of real devices after the
run opened the bench, the run sends no further power on: the cold boot under
way ends with its power off, confirmed by readings as after any cold boot.
If the run has sent any power command, it then switches the supply off once
more and says on standard error whether readings confirmed it. It leaves no
receipt and no output directory and exits with 130, also when staging the
payloads refused the run before its first power command, and when it came
after the last cold boot but before the run's last check of it, which
follows the run's last power command and comes before the receipt is put
together. One that comes after that check is not acted on: the receipt is
written and the run exits with the status of its result. One that comes
before the run opens the bench, while its inputs and payload sources are
checked, is not caught and ends the process as the signal does by default.
`kyto-bench publish` stopped by one before its switch leaves what the board
is served unchanged and exits with 130, also when staging or the readings
would have refused it; `kyto-bench power-off` finishes its command and
confirmation.

**Evidence.** Runs on simulated devices, whose bench takes the flag of an
interrupt without catching a signal: an interrupt after the first power on,
after which the capture stops at once, the cold boot is switched off and the
supply once more, also when the power off of the cold boot sticks, with no
output directory and the message on standard error; one during the power off
before the first entry, after which no cold boot starts; one during the
power off that follows a fault of the last cold boot, after the run's last
check of the flag; one before the first power command, after which none is
sent, also when staging then refuses the run; a publication that an
interrupt stops before its switch, which serves nothing new, also when the
supply is on or the staged copy differs; and the power-off command, which
finishes when an interrupt came first.
