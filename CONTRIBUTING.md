# Contributing

kyto is newly initialized. It has no design files or models yet. Its Cargo
workspaces are the repository tasks in `xtask/` and the bench tool in
`tools/`, which runs [run plans](docs/bench/formats.md#run-plans) on a bench
of the devices that it has drivers for and on simulated devices, and writes
their receipts. Before proposing a change, enable the commit message hook
once and run these commands from the repository root:

```sh
git config --local core.hooksPath .githooks
cargo xtask check
git diff --check
git diff --cached --check
```

The first command enables the commit message hook in `.githooks/commit-msg`.
`cargo xtask check` runs the required checks that the next section describes.
The last two report whitespace errors in unstaged and staged changes.

## Building and checking

The Rust toolchain is pinned in `rust-toolchain.toml`, and rustup installs it
on first use. The repository tasks in `xtask/`, run as `cargo xtask`, and the
bench tool in `tools/` are Cargo workspaces of their own, each with its own
`Cargo.lock`. `cargo xtask check` runs these steps in order and stops at the
first one that fails:

1. `fmt`, then `clippy` with warnings as errors, then `test`, each first in
   the `xtask` workspace and then in the `tools` workspace;
2. `docs`: relative links and anchors in the Markdown files;
3. `commits`: the header of every commit message in `<base>..HEAD` keeps to
   the rules of the commit message hook. The base is the commit that the
   environment variable `CHECK_BASE` names; when it is unset, empty or all
   zeros, the base is the merge base of `origin/main` and `HEAD`;
4. `workflows`: `.github/workflows/` holds only regular files, and every
   `.yml` or `.yaml` file there, in any subdirectory, is UTF-8 and one YAML
   document with scalar keys, without anchors, aliases, merge keys, repeated
   keys or characters that YAML parsers read differently. No workflow
   mentions `pull_request_target` in any letter case, not even in a comment
   or through an escape; every job sets its own `permissions`; and in a
   workflow that runs on `pull_request`, no job requests write access and
   nothing outside `on`, neither the workflow `env` nor any job, names
   `secrets` in an expression, an `if` condition or a key, even inside a
   string literal.

The `docs` and `workflows` steps read only files that Git tracks, so build
output and ignored files do not change the result; add a new file to the index
with `git add` before running the check. The `docs` step reads Markdown files
only; design files and models are not part of it.

`cargo xtask check --receipt <path>` also writes a JSON receipt of the run to
`<path>`, whether the check passes or fails. It records, in this order, the
receipt format version, the command (`cargo xtask check`), the commit checked
out when the run started, whether the working tree was clean before and after
the run, the result, each step with its name, status, duration in
milliseconds, the reason it failed if it did, and the values it observed, the
first line of `rustc --version`, the runner image (`ImageOS`, `ImageVersion`,
read from `/etc/environment` when the process environment lacks them) and the
GitHub Actions run (`GITHUB_RUN_ID`, `GITHUB_RUN_ATTEMPT`,
`GITHUB_EVENT_NAME`, `GITHUB_REF`, `GITHUB_SHA`). A value that cannot be
read, or an environment variable that is not set or is empty, is `null`. The
receipt is the pretty form of `serde_json`, with two-space indentation, and
ends in a newline.

CI runs the check as the job `cargo xtask check` of
`.github/workflows/check.yml` on a GitHub-hosted `ubuntu-24.04-arm` runner,
for the head commit of every pull request to `main` and for every push to
`main`. It sets `CHECK_BASE` to the base commit of the pull request or to the
commit before the push, installs the toolchain of `rust-toolchain.toml` with
rustup, and uploads the receipt as the artifact
`check-receipt-<commit>-<attempt>`.

In the `tools` workspace, Clippy reads `tools/clippy.toml`, which bans
sleeping and reading the system clock, since every time and wait of a run
comes from the bench clock, and running an external program; only the build
script of `kyto-bench`, which asks Git for the commit and whether the working
tree is clean, the tests that run the `kyto-bench` binary, the tests of
`kyto-bench-os` that catch signals in a child process of their own test
binary, and a test of the TFTP directory that runs its own test binary
again through `sh` under the umask `077`, run one. `kyto-bench-os` is the
one member that may use unsafe code, read the system clock and call the C
library through `libc`: every other member forbids unsafe code, and within
`kyto-bench-os` the bans are lifted only by `allow` attributes on the
module declarations of `tools/kyto-bench-os/src/lib.rs`, each of which a
test of the workspace pins. `kyto-bench` reaches the meter's USB bridge
through the safe interface of the `hidapi` crate. On macOS its build
compiles the crate's bundled C library with the system C compiler, which
needs the Xcode Command Line Tools, and links IOKit, CoreFoundation and
AppKit; on Linux it uses the crate's Rust backend over hidraw and compiles
no C.
`tools/kyto-bench-core/clippy.toml` repeats these bans, which a test of the
workspace checks, and also bans the entry points of the standard library to
files and links, the network, environment variables, process and system
state, threads, standard input and output and the clock, so that the crate,
in which unsafe code is forbidden, has no input or output. The tests of the
bench tool alone run with:

```sh
cargo test --locked --manifest-path tools/Cargo.toml
```

Among them, the fuzz tests in `tools/kyto-bench/tests/fuzz.rs` read
changed console transcripts, run plans and receipts. Each makes a fixed
number of cases, written as a constant, from a fixed seed with a generator
written in that file, so every run makes the same cases, and a case that
fails is named on standard error by the seed and its index. The cases start
from the examples in `tools/examples/`, the recordings in
`tools/kyto-bench/tests/data/recorded/` and the corpus in
`tools/kyto-bench/tests/data/corpus/`. A test fails when the corpus, the
recordings or the example console output holds a file, at any depth, that
is not a seed of the fuzz tests, a `.gitattributes` file or a `.DS_Store`
file, which it skips, or a file that holds an IPv4 address, an IPv6
address, a MAC address, a board serial number or a value of a sensitive
line of a board description that is not a documentation value.
The contract's [testing obligations](docs/bench/contract.md#testing-obligations)
give the seeds, the documentation values, the number of cases of each fuzz
test and what it checks. A deeper search, of a hundred times as many cases,
runs with:

```sh
cargo test --release --locked --manifest-path tools/Cargo.toml --test fuzz -- --ignored
```

The change that adds the first design file or model also adds the checks of
the chosen design tool, such as electrical rule checks, design rule checks
and reproducible manufacturing outputs and exports, and updates this section
and [AGENTS](AGENTS.md) to match.

## Design files

The electronic and mechanical design tools have not been chosen, and no
design file or model exists yet. The rules below apply from the first design
file; the directory layout and file formats are fixed together with the tools.

- **Sources.** Each electrical module (for example the screen connection, the
  keyboard or the power circuit) keeps its editable schematic and board layout
  in the native format of the chosen tool.
- **Models.** Each part (for example the enclosure, the keycaps or a bracket)
  keeps its editable parametric model and its assembly sources in the native
  format of the chosen tool. Sources and models are the only design files
  that are edited by hand.
- **Bills of materials.** Every design revision has a bill of materials next
  to its sources, listing each part with its manufacturer, part number,
  quantity and reference designators. A part enters a bill of materials only
  after the decision that selects it is recorded in the project's
  coordination records. Prices, quotes, suppliers and lead times are not kept
  in this repository.
- **Generated files.** The files sent to a manufacturer, the STEP exports and
  the print models of each released revision are generated from a committed
  revision with the pinned version of the tool, kept with the revision they
  were generated from, and never edited by hand. Ordering boards, parts or
  prints is a separate authorized action.
- **Shared dimensions.** Board outlines, mounting holes, connector, button
  and port positions, the screen window and the key interface appear in both
  the board design and the models. Both are kept here and are checked against
  the same revision before either is released.
- **Interfaces with the rest of casξ.** The pins, buses, voltage levels and
  timing that krepi's drivers use are stated in the design documentation of
  each module. The thermal design that krepi's temperature thresholds depend
  on is documented with the enclosure.

## Measurements and records

Measurements produce observations; they do not release a design. A
measurement record states:

- the design revision and the board or sample it was taken on;
- the instruments, the setup and the method;
- the results, with nominal and measured values kept apart, and what was not
  measured.

A successful boot does not show that a supply path carries full load. Until a
power circuit exists, the development boards run from a stable external
supply; battery capacity and runtime are fixed only after the consumption at
idle, under computation load, with the display and peripherals, and during
shutdown and wake has been measured. Consumption and temperatures measured on
the whole Raspberry Pi 5 do not stand for the Raspberry Pi Compute Module 5.
Raw files of measurements taken by hand, not by the bench tool, and photos
are kept outside this repository: each record names them by file name and
SHA-256, and the repository holds the summary tables.

## Mockups, prints and fit tests

The layout is verified with paper mockups and partial prints before any
enclosure is frozen; the three-dimensional fit is checked before the carrier
board is frozen; the enclosure is frozen only after fit tests with the actual
parts.

- **Mockup records** state the layout the mockup was made from, the nominal
  dimensions and their sources, and the dimensions measured on the mockup,
  kept apart from the nominal ones.
- **Print and fit-test records** state the model revision, the printed or
  bought parts used, the printer and material where a part is printed, the
  dimensions measured with their nominal values, and the result.

## Hardware-in-the-loop bench

The bench tool under `tools/` runs
[run plans](docs/bench/formats.md#run-plans) on a bench and writes their
[receipts](docs/bench/formats.md#receipts). It has drivers for the devices
of one bench, which [wiring a bench](#wiring-a-bench) describes, with a
Raspberry Pi 5 under test, and a simulated device for every role, on which
it runs its example; kyto has no bench hardware of its own yet, such as an
adapter board. From the repository root, this runs the example run plan on
the simulated devices of the example instance configuration and writes the
receipt and its public version into `tools/target/example-run`, a directory
that must not exist yet:

```sh
cargo run --locked --release --manifest-path tools/Cargo.toml -- run \
  --plan tools/examples/run-plan.toml \
  --instance tools/examples/instance.toml \
  --payload-root tools/examples/payload \
  --out tools/target/example-run
```

Its exit status is 0 for `PASS`, 1 for `FAIL`, 2 for a usage or input error
or a receipt that cannot be written, with no receipt left, 3 for
`BENCH FAULT`, 4 for a run refused before any power command, 5 when the
receipt was written but its public version was refused or cannot be written,
101 when the tool stopped on an error of its own, and 130 when SIGINT,
SIGTERM or SIGHUP stopped a run on a bench of real devices after the run
opened the bench, with no receipt left in either case; one that comes before
then ends the process as the signal does by default. A run acts on an
interrupt within 100 ms while it reads the console, and otherwise when the
step under way ends: the longest steps wait the off interval of the board
description, the console's `ready_timeout_ms` or a confirmation timeout, and
a relay command whose port never sends its frame does not end, as
[wiring a bench](#wiring-a-bench) says. The cold boot under way is then
switched off and confirmed as usual, which takes up to `off_timeout_ms`, and
the supply is switched off once more.

Two more commands use a bench without a run, and write no receipt; with
the instance configuration, plan, payload root and entry of a bench in
place of the placeholders, they run as:

```sh
cargo run --locked --release --manifest-path tools/Cargo.toml -- power-off \
  --instance <file>
cargo run --locked --release --manifest-path tools/Cargo.toml -- publish \
  --plan <file> --instance <file> --payload-root <dir> --entry <name>
```

`power-off` switches the supply off once and waits for readings to confirm
it. Its exit status is 0 when they did, 2 for a usage or input error, 3 for
`BENCH FAULT`, when they did not or a device failed, and 101 for an error of
the tool; an interrupt does not stop it. It takes no lock, so it also cuts a
run or a publication that holds the bench, and says so first: that run's
receipt does not show the power off, is no evidence and is discarded.
`publish` checks the payload of one entry, stages it and checks the staged
copy as a run does, then, without a power command, makes it the generation
that the board is served, only while readings confirm the supply off, and
checks the copy that the board is served. Its exit status is 0 when it
published, 2 for a usage or input error, 3 for `BENCH FAULT`, when a device
failed or the served copy differs, 4 when a payload check or the readings
refused it, 101 for an error of the tool and 130 for an interrupt before the
switch; with 2, 4 and 130 the board is served what it was served before.
`publish` relies on readings alone to tell that the board is off, so when
its state is unknown, run `power-off` first.
[The formats page](docs/bench/formats.md) describes the commands, the files
they read and write, and every rule of a verdict.

The tool drives a bench through four device interfaces: power commands; a
reading of the supply from a device of its own, which confirms power-on and
power-off; the serial console, whose bytes are kept exactly as received; and
payload delivery to the directory that the board reads over TFTP. Every
time, wait and timeout of a run comes from a clock that the bench provides.
Each interface has a simulated device and a driver for one real device. On
simulated devices the four share a simulated clock, so a run on them waits
for nothing in real time and gives the same times in every run. On a bench,
the clock is the monotonic clock of the host, started when the bench opens,
which does not run while the host sleeps. Its times are in nanoseconds, but
the console stamps each chunk when it arrives on the host, after the
adapter's own buffering, so a stamp can be later than the board wrote the
bytes, and every byte of a chunk has the same stamp.

### Wiring a bench

Wire a bench with its supply unplugged at the wall. Every device, model and
connection is listed in the bench's instance configuration, and a change of
any of them is a new bench hardware revision.

- **Supply and relay.** An LCUS-1 USB relay module, powered and commanded
  by the bench host over its USB serial port (a CH340 at 9600 baud, 8 data
  bits, no parity, 1 stop bit), switches the board's supply. Its COM and NO
  contacts are in series with the VBUS conductor between a USB-C supply and
  the board; ground, CC and every other conductor pass through unswitched,
  so USB-C negotiation and the ground stay connected. Never cut only one of
  several parallel conductors. The relay's state is never read back, and
  its reply confirms nothing. The tool waits without a bound for the port
  to send a command's frame, and a signal does not end that wait: a relay
  port that never sends it holds the command, and the run, until the
  process is killed.
- **Confirmation.** A TI INA228 power monitor on an Adafruit 5832 board
  reads the supply voltage on the board side of the relay. Its 15 mΩ shunt
  is in the VBUS conductor between the relay and the board, VIN+ to the
  relay's NO contact and VIN− to the board, through its screw terminals
  rather than its header pins; its VBUS pin is joined to VIN+, by its VBUS
  solder jumper or by a short wire, so that it reads the voltage at VIN+;
  and its GND is wired to the board's ground. A Microchip MCP2221A USB
  bridge, such as an Adafruit 4471, powers it and reads it from the bench
  host at its default address 0x40. Connect only that one bridge: the tool
  opens the one bridge of the host.
- **Console.** A USB serial adapter at 3.3 V TTL, such as one with an FTDI
  FT232R, is connected to the board's debug UART: its TX to the board's RX,
  its RX to the board's TX, and ground to ground. Its VCC is not connected.
  The line settings are those of the board description.
- **Delivery.** The bench host serves the payload over TFTP from a
  directory that holds a link named after the path prefix of the instance
  configuration. The board's bootloader boots from the network with that
  server and prefix, and writes its log to the debug UART.
- **Board.** Keep no bootable storage, such as an SD card, in the board
  unless a run plan needs it: when the network boot fails, the firmware
  boots from it, and whatever it holds then runs unattended. Jumpers and
  fixtures on the board are part of the bench, such as GPIO17 (header pin
  11) joined to GPIO27 (header pin 13) through 1 kΩ. Join two GPIO pins
  only through a series resistor, so that a payload that drives both as
  outputs at opposite levels cannot short them.

The console adapter is powered by the host and holds the board's RX line
high while the supply is off, which can feed a voltage back into the board
and keep the supply reading above zero after a power off. Measure it before
setting the off threshold.

A run takes every chunk that the console holds before it sends a step, but
output that is still on its way to the host at that moment is recorded
after the send and can be read as the step's response. A step's `expect`
should therefore match only output that its send causes.

### Instance configuration and thresholds

Start a bench's instance configuration from
[the template](tools/templates/instance.toml);
[the example of a bench](tools/examples/bench-instance.toml) shows the
connections of one with a GPIO loopback. Keep it, with the bench's receipts,
in the bench's private instance, outside this repository. Generate its salt
once with `openssl rand -hex 16`, and fill in this machine's settings of
each device, which
[the formats page](docs/bench/formats.md#local-settings) lists.

Set `[confirmation]` from readings of the supply voltage at the board side
of the relay taken with a multimeter, both with the supply off and the
console adapter connected, and with the supply on: the on threshold below
the lowest reading with the supply on, such as 4.5 V when every such
reading is at least 4.75 V, and the off threshold above the highest reading
with the supply off, by a margin such as 0.5 V. When the off threshold
would not stay at or below half of the on threshold, the supply voltage
cannot tell the two states apart on that bench, and the wiring must change,
for example by a series resistor in the adapter's TX line.

After any change to the meter's wiring or to its bridge, repeat the check
with a multimeter in both states before relying on `power-off` or
`publish`: a fault in the meter's wiring can read as off while the board is
on, and `publish` switches on readings alone.

### Rules of the tool and the bench

The rules below apply to the tool now and to the hardware from the first
change that adds it.

- **Tool.** The bench tool is written in Rust and kept under `tools/`, in a
  Cargo workspace with its own `Cargo.lock`: `kyto-bench-core`, a library
  without input or output that reads and writes the formats and holds every
  rule that decides a check or a verdict; `kyto-bench-os`, the serial ports,
  the interrupt signals and the system clock, the one member that may use
  unsafe code and `libc`, whose serial transport is copied from krepi as the
  [porting record](PORTING.md) lists; and `kyto-bench`, the devices, the
  flow of a run and the three commands, which reaches the meter's USB
  bridge through the `hidapi` crate, whose bundled C library its build
  compiles on macOS. External dependencies come only from crates.io and are
  pinned to exact versions. The tool does not depend on any one bench or on
  the system under test: what it knows of a board is in the board's
  [description](docs/bench/formats.md#board-descriptions), and what
  it knows of a bench is in the bench's instance configuration.
- **Instance configuration.** A bench's own configuration, such as addresses,
  relay channels, local paths and credentials, is kept outside this
  repository. The repository holds templates and examples only: the
  [instance configuration template](tools/templates/instance.toml) and the
  [example of a bench](tools/examples/bench-instance.toml) of real devices,
  which hold no salt and none of a machine's settings, and the
  [example instance configuration](tools/examples/instance.toml) of
  simulated devices, whose salt is published, so that no bench may use it.
- **Safety.** Power-on and power-off are each confirmed by readings of the
  supply, never by the relay's own reply. A run switches the supply off and
  waits for readings to confirm it before the first cold boot and after
  every cold boot that started, one that failed or faulted too, and its
  receipt records the commands and the readings; a run that a fault ends
  before readings confirmed the last power off, and a run that stops on an
  error of the tool or an interrupt, switch the supply off once more and
  say whether readings confirmed it. Operations that write firmware, EEPROM contents or
  storage media are off by default: a run refuses a payload file that the
  board description lists as letting the firmware make such a write, before
  anything is staged or powered, unless its run plan entry enables that
  class of write explicitly. An enable is needed for a write but does not
  authorize one; that is decided outside the tool.
- **Receipts.** A receipt records the tool revision, the bench hardware
  revision, the digest of every payload file, the commands sent, the console
  transcript and every observation, with checks and verdicts. The tool writes
  it with a [public version](docs/bench/formats.md#public-versions) that
  hides the values that identify the board, the network or the bench.
  Receipts are written by the tool and never edited by hand, and their
  verdicts can be [derived again](docs/bench/formats.md#re-deriving-verdicts)
  from what they record. A receipt of a run on simulated devices, or of a
  tool whose commit is unknown or whose working tree had changes, is not
  evidence. When the tool or the bench hardware changes, results that depend
  on it are taken again. A receipt of a run on a bench is kept with the
  bench's private instance, outside this repository, and a record names it
  by its commit and SHA-256. The tool never deletes a published generation
  of the TFTP directory, so whoever runs the bench removes old ones from
  `releases/` by hand, never the one that the link names.
- **Tests.** The rules that decide a verdict, check an image digest and power
  a board off after a failure need written requirements and a contract,
  contract tests with negative cases, and fuzz tests on untrusted input such
  as console transcripts. Their [requirements](docs/bench/requirements.md)
  and [contract](docs/bench/contract.md) are written: the contract names the
  rules, the functions that apply them and the tests of each requirement, a
  negative one among them, and a test of the `tools` workspace checks it
  against the code and the tests. Fuzz tests of the parsing of console
  transcripts, run plans and receipts run with the other tests of the
  workspace, and the contract lists the tests that remain as
  [open obligations](docs/bench/contract.md#open-obligations). The rest of
  the tool is tested on the host against simulated devices, on which the
  tests run the example run plan and the other commands through the command
  line, and its drivers: those of the relay and the console on
  pseudo-terminals, with a clock that the test moves, that of the meter on
  a simulated USB bridge and INA228, and that of the TFTP directory in
  temporary directories; no test opens a real device. A bench self-test
  shows only that a bench works.
- **Bench hardware.** Adapter boards, fixtures and other bench parts keep
  their design files under `bench/` and follow the rules for design files and
  records above.

Electrical results, design rule checks, power-flow results, dimension checks,
interference checks, print checks, fit tests and bench checks are recorded
separately; one never stands for another. A record never reports a physical
result that was not observed. Joint tests of the hardware with the real
system belong to krepi.

## Commit messages

Commit messages use English Conventional Commits with a header of at most 72
characters, for example `docs(scope): describe the fit tests`. The accepted
types are `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`,
`build`, `ci`, `chore` and `revert`. Headers that start with `Merge `,
`Revert `, `fixup! ` or `squash! ` are accepted as they are. The `commits`
step of the check applies the same rules to the commits of a branch. Do not
bypass the hook or change global Git configuration.

## Documentation

Documentation and comments are English and written for a person who builds,
checks or uses the device. A document states what a part does, its limits,
and commands that actually run. Nothing is described as existing or working
before it does.

Mathematics in Markdown is written in LaTeX. Inline formulas are written as
`$...$` on one line, without a backslash followed by ASCII punctuation and
without any of `* | < > & ~`; display formulas go in a fenced block with the
info string `math`, never inside a list item, and `$$` is not used. Commands,
program input and output, identifiers, file paths, pin and signal names and
option values keep code formatting.

[Scope](docs/scope.md) describes the complete target scope of kyto, one row
per part, with its status. When work on a part produces design files, models,
programs or records, the same change links them from the part's row and
updates its status.

Plans and their records belong to the project's coordination records, kept
outside this repository: milestone and task numbers, proposal and decision
numbers, capability and interface codes, stage codes, agent narratives, review
histories, evidence ledgers, supplier quotes, prices and lead times. No
document or comment here uses them. A document states the technical content:

- an item that does not exist yet is named by the part, module or file that
  will hold it and marked planned;
- a limit that waits for other work states the condition that lifts it;
- a decision is stated as the rule it made and where the rule applies.

Requirement ids such as `VERD-R01`, `PROV-R01` and `POWER-R01` are labels of
the requirements of the bench tool in
[its requirements](docs/bench/requirements.md), not plan numbers, and
documents, comments and tests may use them.

## License

kyto uses two licenses, chosen by path. Every file under `tools/`, `xtask/`,
`.githooks/`, `.github/` and `.cargo/`, and `rust-toolchain.toml`, is
released under the [MIT License](LICENSE-MIT); the bench tool is under
`tools/`. Every other file, including the design files, models, bills of
materials, manufacturing outputs, exports, print models, the bench hardware
under `bench/`, the measurement, mockup and fit-test records, the README and
the other documentation, is released under the CERN Open Hardware Licence
Version 2 – Permissive ([LICENSE-CERN-OHL-P-2.0](LICENSE-CERN-OHL-P-2.0)).

The copyright holders are the casξ Authors, listed in [AUTHORS](AUTHORS).
Contributions are accepted under the license of the path they are made in.
