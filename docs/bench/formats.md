# Bench tool formats

This page describes the documents of the kyto bench tool in
[`tools/`](../../tools): those it reads,
[board descriptions](#board-descriptions),
[identity records](#identity-records),
[instance configurations](#instance-configurations) and
[run plans](#run-plans); those it writes, [receipts](#receipts), the records
of runs, and their [public versions](#public-versions); the
[rules common to them](#common-rules); and how the verdicts of a receipt are
[derived again from it](#re-deriving-verdicts). The library of the tool,
`kyto-bench-core`, reads and checks the first four, checks payloads, reads
what a cold boot shows, judges cold boots, entries and runs, renders
receipts and reads them back strictly, re-derives their verdicts, and makes
and checks public versions.

The command of the tool, `kyto-bench`, has one subcommand, `run`. It reads a
run plan and an instance configuration, checks the payload of every entry,
runs the plan on the bench that the configuration describes, and writes the
receipt of the run, `receipt.json`, and its public version,
`receipt.public.json`, into a new directory. The tool has devices of one
kind so far, [`simulated`](#local-settings), so it runs plans on simulated
devices only. From the repository root, this command runs the example run
plan on the simulated devices of the example instance configuration,
[`tools/examples/instance.toml`](../../tools/examples/instance.toml), whose
console replays the example console output in
[`tools/examples/captures/`](../../tools/examples/captures):

```sh
cargo run --locked --release --manifest-path tools/Cargo.toml -- run \
  --plan tools/examples/run-plan.toml \
  --instance tools/examples/instance.toml \
  --payload-root tools/examples/payload \
  --out tools/target/example-run
```

It prints its result and the path of the receipt,
`PASS: tools/target/example-run/receipt.json`. `--payload-root` is the
directory that holds the `payload_dir` of every entry, and `--out` a
directory that does not exist yet, which the command creates, so another
run needs another one. The exit status gives the result:

| Status | Result |
| --- | --- |
| 0 | The run is `"PASS"` |
| 1 | The run is `"FAIL"` |
| 2 | A usage or input error: the command line is not valid, the run plan or the instance configuration cannot be read, a document does not pass its checks, the bench cannot be opened or the output directory cannot be created; or the receipt cannot be written, and the error gives the result of the run. No receipt is left: a receipt that cannot be written whole is removed |
| 3 | The run is `"BENCH FAULT"` |
| 4 | The run was [refused](#refused-runs) before any power command; the receipt says why |
| 5 | The receipt was written, but not its public version: the [leak check](#the-leak-check) refused it, and the error names the rule and the field of each value found, or it cannot be written, and the error says why. The receipt gives the verdict or the refusal |
| 101 | The run stopped on an error of the tool itself, a panic. It switched the supply off once more and says on standard error whether readings confirmed it; no receipt is left, and the output directory is removed |

A message or result that cannot be written, as when nothing reads standard
output or standard error any more, changes no exit status.

The tests of the tool read the Raspberry Pi 5 description, the identity
record template and the examples under `tools/examples/`, run the example run
plan through the command, and run from the repository root with:

```sh
cargo test --locked --manifest-path tools/Cargo.toml
```

## Common rules

Every document is a file in UTF-8 with two top-level keys that name its
format: `format`, a fixed name, and `format_version`, an integer. Receipts
and their public versions are JSON; the other documents are TOML. The tool
reads these two keys first, so a document of another format or version is
refused with its format and version named. This version of the tool reads
and writes version 1 of each format, the only version so far, and a document
is checked against exactly the version it declares. Until a bench keeps the
first record of a run made with a version of a format, that version may
still change; from then on, any change to the format gives it a new version.

A run reads the run plan and the instance configuration once, into memory,
and each source file of a payload once for each entry whose payload lists
it, and uses those bytes for everything. The SHA-256 that a receipt records
for the run plan, the instance configuration and each source file of an
entry's payload is that of the bytes read, and the same bytes are parsed as
the document, or checked and staged as the entry's payload file, so a
digest and what the run used cannot differ, even when the file changes
during the run. Nothing is normalized in
between: line endings, a byte order mark and a final newline count as they
are written. A run plan or an instance configuration that is not UTF-8 is an
input error. The board description is not read from a file: a run uses the
description built into the tool whose `id` the run plan's `board` names, and
records its text with the SHA-256 of that text.

Documents are read strictly, at every level except inside the tables of
[local settings](#local-settings), whose keys belong to device drivers. A key
that the format does not have, a missing key and a value of the wrong type are
errors. The values are then checked against the rules that this page states
for each key, and an error names the value and the rule it breaks. In board
descriptions and run plans, every array and every table, nested ones too,
has at most 100 items, which is checked before any line pattern is compiled.

These forms of values recur in every format:

- **Names** are a lowercase ASCII letter followed by lowercase letters,
  digits, `_` and `-`, at most 64 bytes, such as `raspberry-pi-5` or
  `network_boot`. The things that a document names, such as its sources,
  markers and pinned sets, have distinct names within their list.
- **Lines of text** are not empty and have no control characters and no
  white space at either end.
- **Payload names** name a file of a boot payload relative to the directory
  that the board reads it from, such as `overlays/bcm2712d0.dtbo`: segments
  separated by `/`, each made of ASCII letters, digits, `.`, `_` and `-` and
  not ending with `.`, so neither `.` nor `..`, with no empty segment and no
  leading `/`, at most 255 bytes. A TFTP directory on a case-insensitive file
  system serves two names that differ only in ASCII case as one file, and no
  directory holds a file and a directory of the same name, so the names in
  one list differ ignoring ASCII case, and none is, ignoring ASCII case, a
  directory of another, as `overlays` is of `overlays/a.dtbo`. Such file
  systems, FAT among them, also drop a trailing `.` from a name, so that
  `pieeprom.upd.` would be served as the write trigger `pieeprom.upd`; that is
  why no segment ends with `.`. A file name is a payload name of one
  segment.
- **SHA-256 digests** are 64 lowercase hexadecimal digits.
- **Release dates** are written `YYYY/MM/DD`, as the Raspberry Pi 5
  bootloader prints them, and name a day that exists.

### Lines

Console output is read as lines. A line is a run of bytes ended by LF; one
CR immediately before the LF is not part of its content. The offset of a line
is the offset of its first byte. Bytes after the last LF form a last line
that is not terminated; a CR at its end is content. When console output
arrives in chunks, each stamped by the bench clock when it is received, the
time of a line is that of the chunk that holds its LF, and the time of an
unterminated last line is that of the last chunk. Lines are not decoded:
patterns match their bytes.

### Line patterns

A line pattern is a regular expression in the syntax of the Rust `regex`
crate, matched against the bytes of the content of one line, without its
line ending. Patterns are compiled with Unicode mode off:

- `\d`, `\s` and `\w` are ASCII classes, and `.` is any one byte except LF,
  including bytes that are not UTF-8;
- `(?i)` folds the case of ASCII letters only, and every other character
  matches its UTF-8 bytes as written;
- a pattern that needs a Unicode class, such as `\pL` or `(?u)\w`, or
  Unicode case folding, such as `(?iu)k`, does not compile. Turning Unicode
  mode on is not an error by itself: `(?u).` matches one UTF-8 encoded
  character.

A pattern matches anywhere in the line unless it is anchored with `^` or
`$`. Its compiled form is at most 64 KiB. Named groups are written
`(?P<name>...)`; where the use of a pattern needs a group, such as `value`,
the pattern must have it.

Every pattern of a board description comes with `example`: one example line,
or a list of them, at least one. Each example must match its pattern, so the
examples document the line forms that the description reads, and the tests
of the description run on them. Examples hold documentation values only,
never values of a real board or bench.

Patterns and examples are best written as TOML literal strings, `'...'`, in
which a backslash stands for itself; one that contains `'` is written as a
multi-line literal string, `'''...'''`.

## Board descriptions

A board description holds what the bench tool needs to know about one type of
board, as data: the settings of its serial console, the order of a cold boot
and the off interval, the lines its firmware writes, how each identity field
of a board is read, the payload files each variant needs, the rules that give
the earliest bootloader release a board needs, the payload files that let its
firmware write, the settings that no payload may contain, and the lines that
hold values identifying a board or a network. Every official document it
relies on is cited with its version. It holds no fact of a bench: the address
of the TFTP server and the path prefix of the files that the board reads are
parameters of the [instance configuration](#instance-configurations).

The tool has the descriptions in `tools/boards/` built in;
[the Raspberry Pi 5](#the-raspberry-pi-5) is the only one so far. The tests
also read `tools/kyto-bench/tests/data/synthetic-board.toml`, a board that
exists only for tests, with other line forms, another cold-boot order and
another off interval, as a second board of the same format.

| Key or table | Required | Content |
| --- | --- | --- |
| `format`, `format_version` | yes | `"kyto-bench board description"`, `1` |
| `id` | yes | The name of the board type, which identity records and instance configurations give as their `board` |
| `name` | yes | The name of the board type as a line of text |
| `[[sources]]` | no | The [documents it cites](#sources) |
| `[console]`, `[cold_boot]` | yes | The [console and the cold boot](#console-and-cold-boot) |
| `[[markers]]`, `[boot_mode]`, `[tftp]`, `[[firmware_versions]]` | `[boot_mode]` and `[tftp]` | The [lines of the firmware](#firmware-lines) |
| `[identity.<field>]` | one for each of the ten fields | The [ways of reading each identity field](#identity-reads) |
| `[[variants]]` | no | The [payload files of each variant](#variants) |
| `[min_bootloader]` | yes, possibly empty | The [minimum bootloader rules](#minimum-bootloader) |
| `[[write_triggers]]`, `[refused_properties]` | no | The [writes a payload can cause](#write-triggers-and-refused-properties) |
| `[[sensitive]]`, `[deidentify]` | no | The [lines with values that identify a board or a network](#sensitive-lines) |

### Sources

`[[sources]]` lists the documents that the description cites: `id`, a name;
`title`, a line of text; `url`, an `https` address without spaces; and
`version`, a line of text that names the commit or the release of the
document that the description relies on. Every `sources` or `source` key
elsewhere in the description names sources listed here, and every source
listed is cited. A line form that no official document describes cites none.

The descriptions in kyto cite official documents only, each at a fixed
version: a Raspberry Pi repository at a full commit, which its address names,
or a Raspberry Pi product change notice with its document number and release.
A test checks this for the Raspberry Pi 5.

### Console and cold boot

`[console]` gives the settings of the board's serial console: `baud`, at
least 1; `data_bits`, 5 to 8; `parity`, `"none"`, `"even"` or `"odd"`;
`stop_bits`, 1 or 2; and optional `sources`.

`[cold_boot]` gives the order of a cold boot in `order`, which lists each of
these four steps once, with `power_on` last:

- `publish`: make the payload the one the board is served;
- `wait_off_interval`: wait until the off interval has passed since the
  supply was confirmed off;
- `open_console`: open the console, so that every byte from power-on is kept;
- `power_on`: switch the supply on.

`off_interval_ms` is the off interval, 1 ms to one hour, counted from the
confirmation that the supply is off.

### Firmware lines

`[[markers]]` name lines of the firmware, each with `name`, `line` and
`example`.

`[boot_mode]` reads the boot mode that the firmware tries: `line`, with the
groups `mode` and `code`; `example`; `network`, the value of `mode` for a
network boot; `network_marker`, a marker each of whose examples is a boot
mode line of that mode; and optional `sources`.

`[tftp]` reads the TFTP transfers of a network boot: `servers`, a list of at
least one `{ line, example }` whose line names the TFTP server in the group
`server`; `read`, a file read in full, with the groups `path` and `bytes`;
and `read_failed`, a file that could not be read, with the group `path`. The
payload name of a path that the board read is what follows the path prefix
of the instance configuration, byte for byte, with each run of `/` collapsed
to one and a leading `/` removed; a path that does not start with the prefix
names no payload file.

`[[firmware_versions]]` read the versions of the firmware, each with `name`,
`line` with at least one named group, `example`, and optional `sources`.

### Identity reads

`[identity.<field>]` has one table for each of the
[ten identity fields](#the-ten-fields), with `read`, the list of the ways of
reading the field. Each way has `from`, which names it, the keys it needs,
and optional `sources`. Every way except `tftp_reads` can also have `part`,
which names the part of a field with parts that it reads; a way without
`part` reads every part. Every field, and every part of a field with parts,
has at least one way of reading it.

| `from` | Keys | Reads | Fields |
| --- | --- | --- | --- |
| `firmware_log` | `line` with the group `value`, `example` | The group `value` of a line of the firmware log; for a number field, the `value` of each example is decimal digits | Text and number fields |
| `firmware_log_presence` | `line`, `value`, `example` | `value`, a line of text other than `unknown` in any ASCII case, when the line appears; when it does not, the value stays unknown | Text fields |
| `firmware_version` | `name`, `group` | The group `group`, which the line must have, of the firmware version `name` | Text and number fields |
| `tftp_reads` | `files`, at least one payload name | Which of `files` the board reads over TFTP | `boot_files` |
| `visual` | `how`, a line of text | Looking at the board, as `how` says | Text and number fields |
| `software` | `how`, a line of text | Software running on the board, as `how` says | Text and number fields |

The ways that read the firmware log, a firmware version or the TFTP reads
describe what a cold boot shows, as [a cold boot is read](#how-a-cold-boot-is-read):
`tftp_reads` gives the boot files, each with the SHA-256 that the payload of
the run plan entry gives for a file of that name. `visual` and `software` tell
the person who fills in an identity record how to read the value.

### Variants

`[[variants]]` each list the payload files that a board needs for one value
of an identity field: `field`, a text field or part, written `field` or
`field.part`, such as `soc_stepping` or `memory.part`; `value`, a line of
text other than `unknown` in any ASCII case; `payload`, at least one payload
name; and optional `sources`. A field and value appear in one variant at
most.

### Minimum bootloader

`[min_bootloader]` gives the rules for the earliest bootloader release that a
board needs:

- `releases` lists the earliest release with each manufacturing version, as
  `{ version, date }`, with versions from 1 and both versions and dates in
  increasing order; `sources` cites them;
- `[[min_bootloader.bounds]]` each give `field`, a text field or part;
  `any_of`, one or more lines of text other than `unknown` in any ASCII case
  that differ from each other ignoring ASCII case; `at_least`, a release
  date; and `source`. A board whose value of `field` is one of `any_of`,
  ignoring ASCII case, needs a release of `at_least` or later.

From the identity record of a board, the tool takes the minimum as the latest
of these dates:

- the date of the first release in `releases` whose version is at least the
  board's manufacturing version: the `min_boot_version` of the record, or its
  `manufacturing_version` when `min_boot_version` is unknown. Both are the
  same number, read in two ways, and a record that gives both with different
  numbers is refused;
- the `at_least` of every bound whose field the record gives as one of
  `any_of`, ignoring ASCII case, so that a part number written in lower case
  applies the same bound.

A bound whose field the record gives as another value does not apply. A rule
that needs a value the record does not give is undecided: an unknown
manufacturing version, or one above every version in `releases`, and a
bound whose field is unknown. The result is the latest decided date, if
there is one, and the list of every undecided rule, in the order of the
description, whether its date is before or after the decided date. The
minimum is decided when the list is empty; otherwise it is marked undecided:
the board needs a release of the decided date or later, possibly later
still.

### Write triggers and refused properties

The writes that a payload can let the firmware make fall into three classes:
`firmware`, `eeprom` and `storage`. The tool has no class or command for
any other kind of write, such as a write to one-time-programmable memory
or secure-boot provisioning.

`[[write_triggers]]` each give `class`, `files`, at least one file name, and
`source`. A payload file is a trigger of the class when its file name, the
last segment of its payload name, is one of `files` ignoring ASCII case. A
delivery device of the tool writes only payload files that the write gate of
a run plan entry has checked against the write triggers and the refused
properties of the board description: a trigger is staged only through the
gate of an entry that enables its class, and a file that sets a refused
property is never staged; only the tool itself builds a write gate. Every
class is off
unless a [run plan](#run-plans) entry enables it in `[entries.writes]`, and
a run refuses a payload with a trigger of a class that its entry does not
enable before anything is staged or powered, under the rule
`write.not_enabled` of the [payload checks](#payload-checks). An entry's
enable is needed for a write, but it does not authorize one: whoever runs a
plan that enables a write decides that outside the tool.

`[refused_properties]` lists settings that no payload file may contain,
whatever writes are enabled: `files`, a line pattern over payload names that
is matched ignoring the case of ASCII letters; `example`, one or more payload
names that `files` matches; and `properties`, at least one `{ name, source }`,
whose names are ASCII letters, digits and `_` and differ ignoring ASCII case.
Every payload file whose payload name matches `files` is read. For this
check, a line starts at the start of the file and after every CR and every
LF, so that the check holds whichever of them a reader of the file takes for
a line end. A line sets a property when it is, in order, optional spaces,
tabs, vertical tabs and form feeds, the name of the property in any ASCII
case, optional ASCII white space, CR and LF included, and `=`.

These rules see only payload files. They cannot stop software that the board
runs from writing storage that it can reach.

### Sensitive lines

`[[sensitive]]` each give `name`, `line` with at least one named group, and
`example`: a line whose named groups hold values that identify a board or a
network, such as a serial number, MAC addresses or IP addresses. These are
values that a record of a run published from a bench must not show.
`[deidentify]` has `never`, distinct values that are never taken as such
values, even where a sensitive line holds them, such as `0.0.0.0` for an
address that is not set.

The `line` of a sensitive line is not anchored: it starts with no `^` and
ends with no `$`. A [public version](#values-to-hide-in-a-receipt) searches
for every match of it anywhere in the streams that a receipt records, so a
stray byte before or after a line, or two lines run together, keep no line
from its form.

### The Raspberry Pi 5

The description of the Raspberry Pi 5 is
[`tools/boards/raspberry-pi-5.toml`](../../tools/boards/raspberry-pi-5.toml),
built into the tool. It describes a network boot as the bench tool sees it on
the debug UART and cites the Raspberry Pi documentation, the rpi-eeprom
repository and the Raspberry Pi Linux repository at fixed commits, and four
product change notices with their releases. Its comments give the support of
each value. In short:

- **Console.** The debug UART at 115200 baud, with 8 data bits, no parity and
  1 stop bit. The bootloader writes to it when its configuration sets
  `BOOT_UART=1`, at the rate that `UART_BAUD` sets, which must be unset or
  115200. The lines of the later firmware stage, which start with `MESS:`,
  such as the start of the operating system and the overlays loaded, appear
  only when the payload's `config.txt` sets `uart_2ndstage=1`.
- **Cold boot.** The payload is published, the off interval passes, the
  console is opened and the supply is switched on, in this order. The off
  interval is 5000 ms, counted from the confirmation that the supply is off.
  It is a chosen interval: no official document gives it, and it is not a
  measured discharge time of the supply rails.
- **Markers.** The release lines of the BOOTSYS, BOOTLOADER and RECOVERY
  stages (`bootsys`, `bootloader`, `recovery`); the network boot mode,
  `Boot mode: NETWORK (02)` (`network_boot`), and the SD card boot mode
  (`sd_boot`); TFTP timeouts, after which the firmware can retry and still
  boot (`tftp_timeout`); the start of the operating system (`os_start`); and
  the lines of an EEPROM write (`eeprom_write`, `eeprom_updated`).
- **Firmware versions.** The release of the bootloader in the EEPROM, with
  its hash, date and time (`bootloader`), the same for BOOTSYS (`bootsys`),
  the manufacturing version of the bootloader image that is read, which is
  not that of the board (`bootloader_image_mfg`, the `MFG_VER` line), and the
  BL31 version and build lines (`bl31`, `bl31_built`).
- **Identity.** The ways of reading the ten fields are listed under
  [the ten fields](#the-ten-fields).
- **Variants.** A board of the stepping C1 needs `bcm2712-rpi-5-b.dtb`; one
  of the stepping D0 needs that file and `overlays/bcm2712d0.dtbo`, the
  overlay that the downstream device tree loads on D0 devices. No official
  sentence says that the firmware requests the first file on a C1 board, or
  that it requests `bcm2712d0-rpi-5-b.dtb` on a D0 board. Both variants list
  files under the defaults of `upstream_kernel`, `os_prefix`,
  `overlay_prefix` and `device_tree`, and `payload.variant_missing` compares
  the payload with these names only: a payload whose `config.txt` sets one
  of them is refused when it holds the files under the names that it sets
  but not under the default names, and passes when it holds the default
  names but not the files that the firmware then requests. The board is
  served the payload's files alone, so the firmware's request for a file
  that is not in the payload is a failed read, which no rule judges.
- **Minimum bootloader.** The earliest releases with the manufacturing
  versions 1, 2 and 3 are those of 2026/05/11, 2026/09/23 and 2026/10/23. A
  board of PCB revision 4 needs a release of 2024/03/01 or later, as its
  change notice says "from March 2024 onwards", and one of PCB revision 7 a
  release of 2026/09/25 or later. The 15 alternative memory parts of release
  5 of their change notice need 2026/09/25 or later, and the three
  alternative SPI flash parts 2026/05/05 or later, which the release of
  2026/05/11 is the first to meet.
- **Writes.** `pieeprom.upd`, `pieeprom.sig`, `pieeprom.bin` and
  `recovery.bin` let the firmware write the boot EEPROM (`eeprom`). The
  refused properties program one-time-programmable memory, lock the boot
  EEPROM against writes or let the bootloader accept an image older than the
  board: `program_pubkey`, `revoke_devkey`, `program_rpiboot_gpio`,
  `program_jtag_lock`, `eeprom_write_protect`, `program_arb`,
  `bootloader_allow_mfg_downgrade`, `disable_sd_emmc_boot`, and
  `program_usb_boot_mode` and `program_usb_boot_timeout`, which are
  documented for the Raspberry Pi 3. The `include` directive of a
  `config.txt` can name a file of any name, so every payload file is read.
- **Sensitive lines.** The board's serial number in the `BOOTMODE` line; the
  board's MAC address and the TFTP server in the line that waits for the
  network link; the client, gateway and server addresses in the `NET` line;
  the address and MAC address in `ARP` lines; the MAC address and the server
  in `TFTP_GET` lines; and the SD card identifier in the `CID:` line. These
  forms cover static addressing, and each starts at the word that names its
  line, such as `BOOTMODE:`. `0.0.0.0` is never a value to hide.
- **Example values.** The examples hold no value of a real board: the serial
  number `fedc9876` is made up, the MAC addresses `00:00:5e:00:53:a1` and
  `00:00:5e:00:53:a2` and the IPv4 addresses of `192.0.2.0/24` are reserved
  for documentation, and `bench/` stands for the path prefix.

## Identity records

An identity record gives what is known about one board under test: the value
of each of the ten identity fields that its board description lists, each
with the way it was read. A value that cannot be read is `"unknown"`, in
lower case; a value is never estimated. The word in any other case, such as
`"Unknown"`, is refused, so that it is never taken for a known value. The
board's serial number is not part of the record. The record belongs to the
bench: it is the `[identity]` table of the bench's
[instance configuration](#instance-configurations).
[`tools/templates/identity.toml`](../../tools/templates/identity.toml) is its
blank form for the Raspberry Pi 5.

| Key | Content |
| --- | --- |
| `format`, `format_version` | `"kyto-bench board identity"`, `1` |
| `board` | The `id` of the board description |
| `revision_code`, `pcb_version`, `soc_stepping`, `ethernet_phy`, `eeprom_version`, `manufacturing_version`, `min_boot_version`, `boot_files` | `{ value = ..., read = ... }` |
| `memory.vendor`, `memory.part`, `spi_flash.jedec_id`, `spi_flash.part` | `{ value = ..., read = ... }` for each part of the fields with parts |

Every field and part is present, and the record has no other key. `read` is
one of the six [ways of reading](#identity-reads). `value` takes the form of
its field:

- the text fields, `revision_code`, `pcb_version`, `soc_stepping`,
  `memory.vendor`, `memory.part`, `ethernet_phy`, `spi_flash.jedec_id`,
  `spi_flash.part` and `eeprom_version`, take a string that is a line of
  text, such as `"4"`, or `"unknown"`;
- the number fields, `manufacturing_version` and `min_boot_version`, take an
  integer from 1 to 4294967295 written without quotes, or `"unknown"`. 0 is
  refused: it means that the value is not known, which is written
  `"unknown"`;
- `boot_files` takes a list of at least one payload name, or `"unknown"`.

A record is checked against its board description: its `board` is the `id`
of the description, the `read` of every value is a way that the description
lists for its field or part, a value read by `firmware_log_presence` is
the `value` of such a way, and every file of a value read by `tftp_reads` is
one of the `files` of such a way, ignoring ASCII case.
`manufacturing_version` and `min_boot_version` are the board's
manufacturing version, read in two ways, so when both are known they are
equal.

### The ten fields

| Field | Value | Ways of reading in the Raspberry Pi 5 description |
| --- | --- | --- |
| `revision_code` | Text | `firmware_log`: `boardrev` in the `BOOTMODE` line; `software`: the `Revision` line of `/proc/cpuinfo` |
| `pcb_version` | Text: the PCB revision number, such as `"7"` | `visual`: the identification in the PCB revision change notices; `software`: the revision field of the revision code, where revision 1.1 is PCB revision 4, while a board of PCB revision 7 reports `Rev 1.7`, which the cited revision code table does not list |
| `soc_stepping` | Text | `firmware_log_presence`: `D0` when the `bcm2712d0` overlay is loaded; its absence leaves the stepping unknown, never C1. `visual`: the final characters of the marking on the BCM2712 package |
| `memory.vendor`, `memory.part` | Text | `visual`: the manufacturer and part number printed on the memory package |
| `ethernet_phy` | Text | `visual`: the part number on the Ethernet PHY package |
| `spi_flash.jedec_id` | Text | `firmware_log`: the `EEPROM ID` line, which no official document describes. The identifier names a manufacturer and a device type, not a part |
| `spi_flash.part` | Text | `visual`: the marking on the package. The change notice of the alternative parts says they cannot be told apart by inspection, so the value is usually unknown |
| `eeprom_version` | Text, in the form that its way of reading gives; for the Raspberry Pi 5, `YYYY/MM/DD` | `firmware_version`: the `date` of the `bootloader` release line |
| `manufacturing_version` | Number, the board's manufacturing version | `firmware_log`: the `device` value of `MFG ver device N image M`. The bootloader prints this line only when it evaluates an update file, so in a plain network boot the value is usually unknown |
| `min_boot_version` | Number, the board's manufacturing version, equal to `manufacturing_version` when both are known | `software`: `/proc/device-tree/chosen/rpi-min-boot-ver` on a running Linux system, a big-endian 32-bit integer; unknown when the file is absent or holds 0 |
| `boot_files` | Payload names | `tftp_reads`: which of `bcm2712-rpi-5-b.dtb` and `overlays/bcm2712d0.dtbo` the board reads; the payload's own overlays are not part of the identity |

## Instance configurations

An instance configuration holds the settings of one bench: the board type,
the parameters of the bench that the tool reads (`[tftp]`, `[confirmation]`,
`[console]`), the identity record of the board in `[identity]`, payload
digests that the bench pins in `[[pins]]`, the devices and their connections
in `[devices]`, and in `[local]` a random salt and this machine's settings of
each device. A bench keeps its instance configuration outside this
repository; kyto holds none. This example uses documentation values only,
with an empty path prefix and a salt that no bench may use:

```toml
format = "kyto-bench instance"
format_version = 1
board = "raspberry-pi-5"

[tftp]
server_address = "192.0.2.10"
path_prefix = ""

[confirmation]
quantity = "supply_voltage"
on_at_least_micro = 4750000
off_at_most_micro = 500000
consecutive = 1
poll_ms = 100
on_timeout_ms = 3000
off_timeout_ms = 3000

[console]
ready_timeout_ms = 2000

[identity]
format = "kyto-bench board identity"
format_version = 1
board = "raspberry-pi-5"
revision_code = { value = "d04171", read = "firmware_log" }
pcb_version = { value = "4", read = "visual" }
soc_stepping = { value = "D0", read = "firmware_log_presence" }
memory.vendor = { value = "unknown", read = "visual" }
memory.part = { value = "unknown", read = "visual" }
ethernet_phy = { value = "unknown", read = "visual" }
spi_flash.jedec_id = { value = "ef4015", read = "firmware_log" }
spi_flash.part = { value = "unknown", read = "visual" }
eeprom_version = { value = "2026/09/25", read = "firmware_version" }
manufacturing_version = { value = "unknown", read = "firmware_log" }
min_boot_version = { value = "unknown", read = "software" }
boot_files = { value = ["bcm2712-rpi-5-b.dtb", "overlays/bcm2712d0.dtbo"], read = "tftp_reads" }

# The SHA-256 of an empty file.
[[pins]]
name = "empty-config"
files = [{ name = "config.txt", sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" }]

[devices]
power = { kind = "simulated", model = "simulated" }
power_confirmation = { kind = "simulated", model = "simulated" }
console = { kind = "simulated", model = "simulated" }
delivery = { kind = "simulated", model = "simulated" }

[[devices.connections]]
from = "console"
to = "board debug UART"

[local]
salt = "9c1f5e0a7b3d48e2a6c0f4b8d2e71a35"
```

| Key or table | Required | Content |
| --- | --- | --- |
| `format`, `format_version` | yes | `"kyto-bench instance"`, `1` |
| `board` | yes | The `id` of the board description the bench is for |
| `[tftp]` | yes | `server_address`, the address of the TFTP server as the board's firmware prints it, and `path_prefix`, the prefix of every path that the board reads. The address, and the prefix unless it is empty, are 4 to 255 printable ASCII characters without spaces |
| `[confirmation]` | yes | How readings of the supply confirm that it is on or off, in micro-units of `quantity`, `"supply_voltage"` (microvolts) or `"supply_current"` (microamperes): `on_at_least_micro`, a reading at or above which the supply is on; `off_at_most_micro`, below the on threshold, a reading at or below which it is off; `consecutive`, the number of readings in a row that confirm a state, 1 to 1000; `poll_ms`, the interval between readings, 1 ms to one minute; `on_timeout_ms` and `off_timeout_ms`, the longest waits for each confirmation, 1 ms to one hour. The thresholds are integers and can be negative |
| `[console]` | yes | `ready_timeout_ms`, how long the console may take to be ready, 1 ms to one hour |
| `[identity]` | yes | The [identity record](#identity-records) of the board on the bench; its `board` is the `board` of the configuration |
| `[[pins]]` | no | Sets of payload digests that the bench pins, each with `name`, a name, and `files`, at least one `{ name, sha256 }` of a payload name and its SHA-256; the names of one set keep the rule for the names in one list of [payload names](#common-rules). A run plan entry takes its payload from a set by its name |
| `[devices]` | yes | The [devices of the bench and their connections](#devices) |
| `[local]` | yes | The [salt and this machine's settings of each device](#local-settings) |

### Devices

`[devices]` gives the device of each of the four roles of a bench, each as
`{ kind, model }`, and the connections between them:

- `power` switches the board's supply;
- `power_confirmation` confirms the supply state with a reading of its own;
- `console` is the board's serial console;
- `delivery` publishes payloads where the board reads them over TFTP.

`kind`, a name, is the kind of driver that runs the device, and `model`, a
line of text, is the model of the device. `connections` lists the connections
between devices and the board, each `{ from, to }` with two lines of text;
the key is required, and the list can be empty. `[devices]` holds no address,
relay channel or local path. Reading a configuration checks only the form of
`kind`. A run opens the device of each role with the driver of its kind, and
a kind that the tool has no driver for is an input error. The tool has one
kind so far, `simulated`, for every role, whose devices the
[local settings](#local-settings) describe.

The bench hardware revision is the SHA-256 of the canonical JSON of the
`[devices]` table as written. The canonical JSON of a TOML value is UTF-8
without insignificant white space:

- a table is an object whose keys are in increasing order of their UTF-8
  bytes, and an array keeps its order;
- a string is written with `"` and `\` escaped by a backslash; backspace,
  form feed, line feed, carriage return and tab as `\b`, `\f`, `\n`, `\r` and
  `\t`; other characters below U+0020 as `\u00` followed by two lowercase
  hexadecimal digits; and every other character as itself;
- an integer is written in decimal, and a boolean as `true` or `false`;
- floats, dates and times have no canonical form and are refused.

For the example above the canonical JSON is this one line:

```json
{"connections":[{"from":"console","to":"board debug UART"}],"console":{"kind":"simulated","model":"simulated"},"delivery":{"kind":"simulated","model":"simulated"},"power":{"kind":"simulated","model":"simulated"},"power_confirmation":{"kind":"simulated","model":"simulated"}}
```

and the bench hardware revision is
`1c765f95dfd9d93d5ea7e41ad8936e847f378cb2a16c815c84f2bb4f4aebed38`. Since
`[devices]` holds no address, relay channel or path, the revision reveals
none. A comment, the order of keys and any change outside `[devices]`, such
as in `[local]` or in the identity record, leave it unchanged; another
device, model or connection changes it.

### Local settings

`[local]` holds `salt` and this machine's settings of each device in the
tables `[local.power]`, `[local.power_confirmation]`, `[local.console]` and
`[local.delivery]`, each optional, and nothing else. Addresses, relay
channels, port paths, directories and credentials go there, never into
`[devices]`. The keys of each table belong to the driver of the role's kind,
which reads them when a run opens the bench; reading a configuration reads
them only for the [values to hide](#values-to-hide).

`salt` is required: 32 lowercase hexadecimal digits, 128 random bits that a
bench generates once for its configuration, for example with:

```sh
openssl rand -hex 16
```

A missing salt, or one of another length or form, is an error, and so is a
salt that kyto publishes, such as those of the example above and of the
example configuration under `tools/examples/`, unless every device of the
configuration is of the kind `simulated`. A [receipt](#receipts) records the
SHA-256 of the whole configuration file, and its
[public version](#public-versions) shows every digest. The other values of a
configuration are few and could be guessed from a template and tried one by
one against that digest; with 128 random bits in the file, they cannot. The
salt itself never enters a receipt.

The devices of the kind `simulated` share a simulated clock, which is the
bench clock of a run on them, so a run waits for nothing in real time and
its times are the same in every run. The power device switches a simulated
supply, which the confirmation device reads as 5100000 µV while it is on
and 0 µV while it is off. The delivery device keeps the staged payloads in
memory. The console replays recorded console output: `[local.console]` has
one key, `replay`, a list of files given as paths relative to the instance
configuration, and the n-th power-on of a run replays the n-th file, its
k-th line k ms after the supply comes on; after the last file, the board
writes nothing. The console answers nothing that it is sent, so the
response of a step must already be in the file. The other roles take no
settings, so their tables are absent or empty.

### Values to hide

The tool lists the instance values that a record of a run published from the
bench must not show, each with the name of its rule:

- the TFTP server address, `instance.tftp.server_address`;
- the path prefix when it is not empty, `instance.tftp.path_prefix`;
- the salt, `instance.local.salt`;
- every string of at least 4 bytes in the tables of `[local]`, in arrays and
  nested tables too, `instance.local.<role>.<key>`, with the keys of nested
  tables joined by `.`.

Shorter strings would be found in unrelated bytes, and numbers are not
listed.

### Checks against the board description

A configuration is checked with the board description that its `board`
names: the `id` of the description equals `board`; no
[value to hide](#values-to-hide) of the configuration occurs in the text of
the description, ignoring ASCII case; and the identity record passes the
[checks of an identity record](#identity-records) against the description.
A receipt records the description as it is, and a
[public version](#public-versions) never rewrites it, so a value of the bench
that the description holds, such as the path prefix `bench/` that the
Raspberry Pi 5 description uses as a stand-in, would leave no run of the
bench with a public version.

## Run plans

A run plan says what a run boots and what each cold boot must show: the
payload of each entry, with the SHA-256 of every file; how many times it is
booted and when a cold boot ends; the lines that must and must not appear;
the bytes sent to the board and the lines that must answer them; the
firmware versions, values and segments of the transcript; and the writes
that the entry enables. Whoever writes a plan keeps it with the record it is
for. A plan holds no value of a bench, such as an address, a path prefix or
a relay channel, not even in a comment: a receipt records the plan's text as
it is, and a [public version](#public-versions) never rewrites it.
`kyto-bench run` refuses a plan that holds a
[value to hide](#values-to-hide) of the instance configuration, ignoring
ASCII case, as an input error before it runs anything, and the leak check
refuses the public version of a receipt whose plan holds a value to hide of
at least 4 bytes, such as an address or a path prefix; a relay number
shorter than that is not searched for, so nothing finds one in a plan, and
it is for whoever writes the plan to keep it out. Payload files hold no
value of a bench either: a public version shows the SHA-256 of every payload
file, and that of a file made from a template with a value of the bench,
such as a server address on a kernel command line, would let the value be
found by trying candidates against it. Nothing checks the contents of
payload files for such values. The library reads and checks plans and reads
cold boots as their entries say, and `kyto-bench run` runs them.

A plan is UTF-8 text; bytes that are not UTF-8 are not read as a plan. The
library keeps the text that it reads a plan from without changing a byte:
line endings stay as written, a byte order mark at the start stays, and a
final newline is part of the text. A receipt records that text with the
SHA-256 of its bytes, and reading a receipt checks the digest against the
recorded text and reads the plan again from that same text, so the digest,
the text and the plan of a receipt cannot differ.

[`tools/examples/run-plan.toml`](../../tools/examples/run-plan.toml) is an
example for the Raspberry Pi 5 with two entries. Their payloads are the small
stand-in files under `tools/examples/payload/`, from which no board boots,
and the payload digests of the plan are theirs. The digest of the segment
`banner` is that of the two lines `Example echo payload` and
`BUILD id=0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c`, each ended by CR LF,
which come between the start of the operating system and `READY` in
[`tools/examples/captures/echo.bin`](../../tools/examples/captures/echo.bin),
the example console output of this entry. Its second entry, with the digests
shortened here:

```toml
[[entries]]
name = "echo"
payload_dir = "echo"
payload = [
  { name = "config.txt", sha256 = "950cbaa5..." },
  { name = "bcm2712-rpi-5-b.dtb", sha256 = "ac448652..." },
  { name = "overlays/bcm2712d0.dtbo", sha256 = "f7fbb469..." },
  { name = "kernel.img", sha256 = "854a0536..." },
]
cold_boots = 1
boot_timeout_ms = 60000
end = { marker = "ready" }
markers = [{ name = "ready", line = '^READY$' }]
expect = [{ marker = "ready", count = 1 }]
forbid = [{ line = '^PANIC' }]
before_first_send = ["board.network_boot", "ready"]
steps = [{ after = "ready", send = "?\r", expect = '^STATS seq=1', timeout_ms = 1000 }]
values = [{ field = "build", line = '^BUILD id=(?P<value>[0-9a-f]{40})$' }]
segments = [{ name = "banner", from = "board.os_start", to = "ready", sha256 = "498b39e6..." }]
```

| Key | Required | Content |
| --- | --- | --- |
| `format`, `format_version` | yes | `"kyto-bench run plan"`, `1` |
| `board` | yes | The `id` of the board description |
| `reference` | no | A [reference](#references) for the plan |
| `[[entries]]` | yes | 1 to 100 entries, run in order, each with a name that no other entry has |

Each entry has these keys:

| Key | Required | Content |
| --- | --- | --- |
| `name` | yes | A name |
| `reference` | no | A [reference](#references) for the entry |
| `payload_dir` | yes | The directory that holds the source files of the payload, below the payload root that a run is given: a relative path in the form of a payload name |
| `payload` or `payload_pin` | exactly one of the two | `payload`, the payload files, at least one `{ name, sha256 }` of a payload name and its SHA-256, whose names keep the rule for the names in one list; or `payload_pin`, the name of a pinned set of the [instance configuration](#instance-configurations), whose files are the payload. An empty `payload` is an error, beside `payload_pin` too |
| `cold_boots` | yes | 1 to 100 |
| `boot_timeout_ms` | yes | The longest cold boot, counted from the power-on command, 1 ms to one hour |
| `end` | yes | When a cold boot [ends](#steps-and-the-end-of-a-cold-boot): `{ marker = "<marker>" }` or `"timeout"` |
| `max_capture_bytes` | no | The most bytes a cold boot captures, 1 to 16777216; 4194304 when not given |
| `[entries.firmware.<name>]` | no | The expected values of groups of the board's firmware version line `name`, as `<group> = "<value>"`, each value a line of text |
| `markers` | no | The entry's [markers](#markers-of-an-entry), each `{ name, line }` |
| `expect` | no | How often a marker appears: `{ marker, count }`, or `{ marker, min, max }` with `min`, `max` or both, `min` not above `max` |
| `order` | no | Each `{ markers = [...] }`, at least two distinct markers whose first occurrences appear in this order |
| `forbid` | no | Each `{ line, from, to }`: a line pattern that no line may match strictly between the first occurrences of the optional markers `from` and `to`; an end left out is the start or the end of the capture |
| `before_first_send` | no | Markers whose lines end before the first byte sent; only in an entry with steps |
| `steps` or `sends = false` | exactly one of the two | `steps`, at least one of the [steps](#steps-and-the-end-of-a-cold-boot), each `{ after, send, expect, timeout_ms }`; or `sends = false`, for an entry that sends nothing; `sends` is never `true`, and an empty `steps` is an error, beside `sends = false` too |
| `values` | no | Named values, each `{ field, line }`: `field`, a name that no other value of the entry has, and `line`, a line pattern with the group `value` |
| `segments` | no | Each `{ name, from, to, sha256 }`: a name that no other segment of the entry has, two markers and the SHA-256 of the bytes between their lines |
| `[entries.writes]` | no | `firmware`, `eeprom` and `storage`, each `false` unless written `true`: the [classes of writes](#write-triggers-and-refused-properties) that the entry enables |

Every list of an entry has at most 100 items, the markers of each `order`
included, and a plan has at most 1000 line patterns in all, the `line` and
`expect` of every entry; both bounds are checked before any pattern is
compiled. `line` and `expect` are [line patterns](#line-patterns); they need
no example.

### References

The `reference` of a plan or an entry is text supplied by whoever wrote the
plan, such as the record the plan is for. The tool does not interpret it and
keeps it as written, and a receipt records it unchanged. It is at most 1024
bytes and, like a line of text, has no control character: none of C0
(U+0000 to U+001F), DEL (U+007F) or C1 (U+0080 to U+009F).

### Markers of an entry

An entry's markers have distinct names. An entry refers to one of its own
markers by its name and to a marker of the board description as
`board.<name>`, such as `board.network_boot`. Every marker that `end`,
`expect`, `order`, `forbid`, `before_first_send`, `steps` and `segments`
name exists. One line can be a line of several markers.

### Steps and the end of a cold boot

A step sends `send`, a TOML string of 1 to 4096 bytes, as its UTF-8 bytes,
and expects a line that matches `expect` within `timeout_ms`, 1 ms to one
hour, of the send. Steps run in order:

- the first step is armed by the first line of its `after` marker, and each
  later step by the first line of its `after` marker that is read after the
  response of the step before it, which can be that response itself;
- an armed step is sent once the chunk that holds its trigger line has been
  read;
- its response is the first line that starts after the last byte received
  before the send and matches `expect`; it must be complete, ended by its
  LF, within `timeout_ms` of the send.

Only complete lines arm and answer steps and end a cold boot at its marker:
the unterminated last line of a capture does none of these.

`end = { marker = "<marker>" }` ends a cold boot once a complete line of the
marker has appeared and every step has its response. `end = "timeout"`
captures until the boot timeout. A run ends a cold boot at the boot timeout,
or when the `timeout_ms` of the step that waits for its response has passed,
once its bench clock reaches that time, whatever the console reports: a
console that reports nothing by then does not keep the cold boot going, and
one that reports a deadline earlier does not end it.

### Checks of a run plan

A plan is checked on its own when it is read. It is then checked with the
board description that its `board` names and with the pinned sets of the
instance configuration:

- `board` is the `id` of the description;
- every `board.<name>` marker is a marker of the description;
- every `[entries.firmware.<name>]` names a firmware version line of the
  description, and gives values only of groups that its line has;
- every `payload_pin` names a pinned set of the instance configuration.

### Payload checks

These checks of an entry's payload come before the board is powered. They
are made in a fixed order, and the first that fails refuses the run, as
[refused runs](#refused-runs) describe, under the name of its rule and with
its entry and file, so a run is refused under one rule only:

1. for each entry, in the order of the plan, and for each file of its
   payload, in increasing order of the bytes of the payload names (so
   `B.txt` comes before `a.txt`): `payload.source_digest`,
   `write.not_enabled` and `write.refused_property`; then, for the entry,
   `payload.variant_missing`;
2. once these pass for every entry, for each entry in the order of the plan,
   its payload is staged and, for each file in the same order of names,
   `payload.staged_digest`.

The checks of the first step need no device: `kyto-bench run` makes them
before it opens the bench, from the source files as it read them, and stages
a payload, through the write gate of its entry, only once they have passed.
A source file that cannot be read has no digest and fails
`payload.source_digest`, and a file that cannot be staged or read back fails
`payload.staged_digest`.

The rules are:

- `payload.source_digest`: the SHA-256 of every source file equals that of
  the payload;
- `payload.staged_digest`: the SHA-256 of every file of the staged copy, read
  back from where the board will read it, equals that of the payload;
- `write.not_enabled`: no payload file is a
  [write trigger](#write-triggers-and-refused-properties) of a class that the
  entry does not enable;
- `write.refused_property`: no payload file sets a refused property, whatever
  the entry enables;
- `payload.variant_missing`: the payload has every file of the
  [variant](#variants) whose value the identity record gives for the
  variant's field, ignoring ASCII case, and every file of every variant of a
  field whose value is unknown; names are compared ignoring ASCII case. The
  file of a refusal is the first missing file of the first variant, in the
  order of the board description, that misses one.

Once the board is served the entry's payload, `payload.served_digest`
compares the SHA-256 of every file of the copy that the board is served, read
through the served generation, with that of the payload, one check for each
file; a file that differs is a fault of the bench.

### How a cold boot is read

The library reads the transcript of a cold boot in [lines](#lines), with the
line patterns of the board description and of the entry. It reads a
transcript the same way while it is received and when a record of it is read
back, with each send placed after the bytes received before it; the
unterminated last line of a capture is read when the capture ends, but not
for [steps or the end](#steps-and-the-end-of-a-cold-boot). A line is
named by its offset. From the lines, in order, it takes:

- **Boot modes.** A line of the `[boot_mode]` form gives the mode that the
  firmware tries. A network span runs from a boot mode line of the network
  mode to the next boot mode line or the end of the capture.
- **Markers.** Every line of a marker is an occurrence of it, with the time
  of the line.
- **TFTP transfers**, inside network spans only. A line of a server form
  names a server, which is compared with the server address of the instance
  configuration. A line of the `read` form is a file read in full; its path
  gives its payload name after the path prefix, as
  [firmware lines](#firmware-lines) describe. A path outside the prefix, and
  one whose rest after the prefix is not a payload name, name no payload
  file; a read records which of the two it is. Failed reads are not reads.
- **Firmware versions.** Every line of a firmware version gives the values of
  its named groups.
- **Identity values.** Every line that a `firmware_log`,
  `firmware_log_presence` or `firmware_version` way of reading matches gives
  a value of its field or part. When several lines give values of one field
  or part, they agree only when they give the same value; every distinct
  value is kept, so a disagreement shows.
- **Boot files.** The `tftp_reads` ways of reading give the boot files: each
  file of their `files` that the board read in full, compared ignoring ASCII
  case, once, under the payload name of its first read and in the order of
  first reads, with the SHA-256 that the entry's payload gives for the file
  of that name, ignoring ASCII case, or none when the payload has no such
  file.
- **Named values.** Every line of a `values` pattern gives the group `value`;
  several lines agree as identity values do.
- **Segments.** The SHA-256 of the bytes after the LF of the first line of
  `from` up to the first byte of the first line of `to` that starts after
  it, CR and LF bytes included; none when either line is missing.
- **Steps.** The line that armed each step, the send and the response, as
  [steps](#steps-and-the-end-of-a-cold-boot) describe.

## Receipts

A receipt is the record of one run: the inputs of judgement verbatim, every
event with its time on the bench clock, the raw console output of every cold
boot, and what each cold boot showed, with checks and verdicts. Receipts are
written by the tool and never edited by hand: reading a receipt refuses any
text that is not exactly the tool's rendering of what it holds. A receipt
holds no time of day and no date. Every time in it is a time of the bench
clock, in nanoseconds from the clock's origin, and the nonce, 128 random
bits, makes every receipt distinct.

A receipt is one JSON file in UTF-8, in the pretty form of the Rust
`serde_json` crate: two spaces of indentation, each member of an object and
each item of an array on a line of its own, `": "` after each key, `[]` for
an empty array, and a final newline. The keys of each object are in the
order in which this section lists them. Strings are escaped as in the
[canonical JSON](#devices) of the devices table, integers are written in
decimal, and a value that is absent is `null`. A receipt of a run on
simulated devices, with the example instance configuration above, starts
like this:

```json
{
  "format": "kyto-bench receipt",
  "format_version": 1,
  "nonce": "f0e1d2c3b4a5968778695a4b3c2d1e0f",
  "tool": {
    "commit": "0123456789abcdef0123456789abcdef01234567",
    "clean": true
  },
  "evidence": {
    "eligible": false,
    "reasons": [
      {
        "rule": "evidence.simulated",
        "detail": "the run used simulated devices"
      }
    ]
  },
  "bench": {
    "hardware_revision": {
      "kind": "devices-sha256",
      "value": "1c765f95dfd9d93d5ea7e41ad8936e847f378cb2a16c815c84f2bb4f4aebed38"
    },
    "simulated": true
  },
```

| Key | Content |
| --- | --- |
| `format`, `format_version` | `"kyto-bench receipt"`, `1` |
| `nonce` | 128 random bits, as 32 lowercase hexadecimal digits |
| `tool` | `commit`, the commit of the tool as 40 lowercase hexadecimal digits, or `null` when it is unknown; and `clean`, whether the tool's working tree had no changes, or `null` when it is unknown |
| `evidence` | Whether the receipt is [eligible as evidence](#evidence) |
| `bench` | `hardware_revision`, with `kind`, `"devices-sha256"`, and `value`, the [bench hardware revision](#devices); and `simulated`, whether any device of the bench is of the kind `simulated` |
| `inputs` | The [inputs of judgement](#inputs-of-judgement) |
| `reference` | The [reference](#references) of the run plan, or `null` when it has none |
| `refusal` | `null`, or why the run was [refused](#refused-runs) |
| `start` | `events`, the [events](#events) before the first entry |
| `entries` | The [record of each entry](#entries-and-cold-boots) of the plan, in order |
| `verdict` | The verdict of the run, `"PASS"`, `"FAIL"` or `"BENCH FAULT"`, or `null` when the run was refused |

### Evidence

`evidence` has `eligible` and `reasons`, a list of `{ rule, detail }`.
`reasons` holds exactly the rules of this closed set that apply to the
receipt, each once and in this order, with `detail` saying what was found:

- `evidence.commit_unknown`: `tool.commit` is `null`;
- `evidence.tree_changed`: `tool.clean` is not `true`, since the tree had
  changes or whether it had is unknown;
- `evidence.simulated`: `bench.simulated` is `true`.

`eligible` is `true` exactly when `reasons` is empty: only a receipt of a run
whose every device is real, made by a tool of a known commit without
changes, is eligible as evidence.

`tool.commit` and `tool.clean` describe the tree as it was when the tool was
built. Every build of the tool asks Git for them again, so a change made
after an earlier build, an untracked file included, makes the next build's
`clean` `false`. A build from a source tree that is not a Git checkout of its
own records both as `null`; neither comes from the environment of the build.

### Inputs of judgement

`inputs` holds what the checks and verdicts of a run read, so that a receipt
can be read without the files of the bench:

- `run_plan`: `sha256`, the SHA-256 of the bytes of the run plan, and
  `text`, its text [as read](#run-plans);
- `board_description`: `id`, `sha256` and `text`, the id of the board
  description, the SHA-256 of its bytes and its text;
- `instance`: `sha256`, the SHA-256 of the instance configuration file, and
  `parameters`, the tables `tftp`, `confirmation`, `console`, `identity` and
  `pins` of the configuration, each with its keys; `pins` is a list, empty
  when the configuration pins no set.

Apart from the digest of the whole file, the devices of an instance
configuration enter a receipt only as the bench hardware revision, and its
`[local]` table only as the relay of each power event: the salt and the other
values of `[local]` never do.

`parameters.identity` records the identity record as an object of `format`,
`format_version`, `board` and `fields`. `fields` lists each field and part of
the record once, in the order of [the ten fields](#the-ten-fields), as
`{ field, part, value, read }`: `part` is the name of the part, or `"value"`
for a field without parts; `value` is a string, `"unknown"` included, an
integer or a list of payload names; and `read` is the way of reading.

### Events

Every event is an object of `at_ns`, its time on the bench clock in
nanoseconds; `received`, the number of bytes of the cold boot's console
output received before it; `event`, its kind; and the keys of its kind:

| `event` | Keys | What happened |
| --- | --- | --- |
| `power` | `command`, `"on"` or `"off"`, and `relay` | The power device reported that it sent the command. `relay` is the relay it switched, as a string, from this machine's settings of the power device, or `null` for a simulated device |
| `reading` | `quantity`, `"supply_voltage"` or `"supply_current"`, and `micro` | A reading of the device that confirms the supply state, in micro-units of `quantity` |
| `console_ready` | none | The console is open and keeps every byte from now on |
| `console_closed` | none | The console is closed |
| `send` | `step`, the index of the step in the entry, and `bytes_base64` | The bytes of the step were sent to the console |
| `end` | `reason`: `"end_condition"`, `"boot_timeout"`, `"step_timeout"`, `"byte_limit"`, `"chunk_limit"` or `"device_error"` | The cold boot ended |
| `switch` | `generation`, `g` followed by six decimal digits | The board is served the staged generation of that name |
| `device_error` | `role`, a [device role](#devices), and `detail` | A device of that role failed |

### Entries and cold boots

The record of an entry has these keys:

| Key | Content |
| --- | --- |
| `index` | The position of the entry in the plan, from 0 |
| `name`, `reference` | The `name` and `reference` of the plan's entry; `reference` is `null` when it has none |
| `payload` | `pin`, the `payload_pin` of the entry, or `null`; `generation`, the generation that holds its staged payload, or `null`; and `files` |
| `events` | The events of the entry outside its cold boots: the switch to its generation and the failures of devices then, and, when a fault ended the run before readings confirmed the last power off, the power off that the run sent once more and its readings |
| `checks` | The [checks](#checks) of the entry |
| `cold_boots` | The record of each cold boot that was run, at most the `cold_boots` of the plan's entry |
| `verdict` | `"PASS"`, `"FAIL"`, `"BENCH FAULT"` or `"NOT RUN"`; only an entry is ever `"NOT RUN"`, and an entry that was not run has no cold boot |

`payload.files` lists the files of the entry's payload, those of its
`payload` in the plan or those of the set of `inputs.instance.parameters.pins`
that its `payload_pin` names, by name and in the same order, each as
`{ name, bytes, source_sha256, staged_sha256, served_sha256 }`: its size in
bytes and the SHA-256 of its source file, of its staged copy and of the copy
that the board is served, which the [payload checks](#payload-checks)
compare, each `null` when that file was not read.

The record of a cold boot has `index`, its number from 0; `events`;
`serial`; `observations`; `checks`; and `verdict`, `"PASS"`, `"FAIL"` or
`"BENCH FAULT"`. `serial` holds the bytes received from the console in the
cold boot exactly as they arrived, with no line ending changed and nothing
decoded:

- `length`, the number of bytes, at most the `max_capture_bytes` of the
  plan's entry;
- `bytes_base64`, the bytes in base64, in the standard alphabet with `=`
  padding and without line breaks. The bytes of a `send` event are written
  the same way, and only this canonical encoding is read;
- `index`, the chunks in which the bytes arrived, each `[offset, at_ns]`:
  the offset of its first byte and the time it was received, as
  [lines](#lines) use them. Offsets start at 0, increase and stay below
  `length`, and times do not decrease; a cold boot without bytes has an
  empty index. The index has at most 65536 chunks.

`observations` are what the cold boot shows, as
[a cold boot is read](#how-a-cold-boot-is-read), each line named by its
offset. A value taken from the console output is its bytes read as UTF-8,
with each sequence that is not UTF-8 written as U+FFFD. TFTP server addresses
and paths are not copied: a server line is recorded with whether it names the
server of the instance, and a read with its payload name.

| Key | Content |
| --- | --- |
| `markers` | Each line of a marker as `{ name, line, at_ns }`: a marker of the entry by its name or of the board as `board.<name>`, and the time of the line |
| `boot_modes` | Each boot mode line as `{ mode, line }` |
| `firmware` | Each line of a firmware version as `{ name, line, groups }`, with `groups` the `[group, value]` pairs of the named groups that took part in the match, in the order of the pattern |
| `identity` | Each identity field or part that lines give, as `{ field, values, lines }`: the field written `field` or `field.part`, every distinct value in the order it first appeared, and every line that gave one. The lines agree when `values` has one item |
| `tftp` | `servers`, each line that names the TFTP server as `{ line, instance }`, with whether it names the server of `inputs.instance.parameters.tftp`; and `reads`, each file read in full as `{ line, name, inside_prefix, bytes }`, with its payload name, or `null` when its path is outside the path prefix or its rest after the prefix is not a payload name; whether its path is inside the prefix; and the size that the line gives, or `null` |
| `boot_files` | Each boot file as `{ name, sha256 }`, with the SHA-256 that the entry's payload gives for it, or `null` |
| `values` | Each named value of the entry that lines give, as `{ field, values, lines }`, like `identity` |
| `segments` | Each segment whose two lines appeared, as `{ name, from_line, to_line, sha256 }` |
| `steps` | Each step that was armed, as `{ step, trigger_line, send_event, response_line }`: the line that armed it, the index of its `send` event among the events of the cold boot, and the line of its response, each of the last two `null` when there is none |

A cold boot of an entry with the marker `ready`, `'^READY$'`, and the step
`{ after = "ready", send = "?\r", expect = '^STATS seq=1', timeout_ms = 1000 }`
received the lines `  4.94 Boot mode: NETWORK (02) order f1`, `READY` and
`STATS seq=1`, each ended by CR LF, in three chunks. Its record starts like
this, shown with less indentation:

```json
{
  "index": 0,
  "events": [
    {
      "at_ns": 5100000000,
      "received": 0,
      "event": "console_ready"
    },
    {
      "at_ns": 5100000000,
      "received": 0,
      "event": "power",
      "command": "on",
      "relay": null
    },
    {
      "at_ns": 5200000000,
      "received": 0,
      "event": "reading",
      "quantity": "supply_voltage",
      "micro": 5100000
    },
    {
      "at_ns": 7600000000,
      "received": 48,
      "event": "send",
      "step": 0,
      "bytes_base64": "Pw0="
    },
    {
      "at_ns": 7700000000,
      "received": 61,
      "event": "end",
      "reason": "end_condition"
    },
    {
      "at_ns": 7700000000,
      "received": 61,
      "event": "power",
      "command": "off",
      "relay": null
    },
    {
      "at_ns": 7800000000,
      "received": 61,
      "event": "reading",
      "quantity": "supply_voltage",
      "micro": 0
    },
    {
      "at_ns": 7800000000,
      "received": 61,
      "event": "console_closed"
    }
  ],
  "serial": {
    "length": 61,
    "bytes_base64": "ICA0Ljk0IEJvb3QgbW9kZTogTkVUV09SSyAoMDIpIG9yZGVyIGYxDQpSRUFEWQ0KU1RBVFMgc2VxPTENCg==",
    "index": [
      [
        0,
        6100000000
      ],
      [
        41,
        7600000000
      ],
      [
        48,
        7700000000
      ]
    ]
  },
  "observations": {
    "markers": [
      {
        "name": "board.network_boot",
        "line": 0,
        "at_ns": 6100000000
      },
      {
        "name": "ready",
        "line": 41,
        "at_ns": 7600000000
      }
    ],
    "boot_modes": [
      {
        "mode": "NETWORK",
        "line": 0
      }
    ],
    "firmware": [],
    "identity": [],
    "tftp": {
      "servers": [],
      "reads": []
    },
    "boot_files": [],
    "values": [],
    "segments": [],
    "steps": [
      {
        "step": 0,
        "trigger_line": 41,
        "send_event": 3,
        "response_line": 48
      }
    ]
  },
```

### Checks

`checks` list the rules evaluated on each subject, passing ones included, in
the order that this section gives, each as
`{ rule, subject, result, detail }`. `result` is `"pass"`, `"fail"` or
`"fault"`, and `detail`, which says what was found, is present exactly when
the result is not `"pass"`. A detail names lines by their offsets and copies
no value of the console output or of the bench. A fault is a fault of the
bench; a failure is one of the board or its payload.

The checks of an entry are, in this order:

- for the first entry, when `start.events` is not empty,
  `power.off_unconfirmed` on the subject `start`: a fault unless readings
  [confirm](#the-supply) the power off that `start.events` end with;
- `device.error` for each `device_error` event, first of `start.events` for
  the first entry, then of the entry's own events, each in the order of the
  events: a fault on the subject of the device's role, with the device's
  detail;
- once a `switch` event shows that the board is served the entry's
  generation, `payload.served_digest` for each file of its payload, in the
  order of the payload, with the payload name as `subject`: a fault when the
  copy that the board is served has another SHA-256 than the payload gives,
  or was not read, as in `the served copy of config.txt was not read`.

The checks of a cold boot are, in this order, each rule once unless the
table says otherwise:

| Rule | Subject | Result | It does not pass when |
| --- | --- | --- | --- |
| `console.not_ready` | `console` | fault | the cold boot has no `console_ready` event |
| `power.on_unconfirmed` | `on` | fault | the cold boot has no power on, or readings do not [confirm](#the-supply) it |
| `power.off_interval` | `on` | fault | only for a cold boot with a power on: the supply was not confirmed off before the cold boot, or an event of a step that follows `wait_off_interval` in the board's `cold_boot.order`, the console ready or the power on, came less than `off_interval_ms` after that confirmation |
| `power.off_unconfirmed` | `off` | fault | the last power command of the cold boot is not a power off that readings confirm |
| `power.order` | `events` | fault | an event is [out of order](#the-supply) |
| `console.before_power_on` | `console` | fault | a chunk of the console output was received before the power on, or the cold boot has console output but no power on: output while the supply was confirmed off contradicts that confirmation |
| `device.error` | the role of the device | fault | one check for each `device_error` event, which it reports |
| `boot.network` | `board.` and the board's `network_marker` | fail | no boot mode line is of the board's network mode |
| `boot.fallback` | the same | fail | the last boot mode line is of another mode |
| `boot.end` | the marker of `end`, or `timeout` | fail | with an end marker, no complete line of it, ended by its LF, appeared by the boot timeout, `boot_timeout_ms` after the power on; with `end = "timeout"`, the cold boot ended before the boot timeout |
| `console.byte_limit` | `console` | fail | the cold boot ended at the capture limit, its `end` reason `byte_limit` or `chunk_limit` |
| `marker.count` | the marker | fail | for each `expect`, in order: the marker appeared another number of times |
| `marker.order` | the markers of the `order`, separated by spaces | fail | for each `order`: one of the markers did not appear, or the first line of one does not start after the first line of the one before |
| `marker.forbidden` | the index of the item in `forbid`, from `0` | fail | for each `forbid`: a line strictly between the first lines of `from` and `to` matches `line`; an end left out, or a `to` that did not appear, is the start or the end of the capture, and a `from` that did not appear leaves no line between |
| `marker.before_first_send` | the marker | fail | for each of `before_first_send`: a byte was sent and the first line of the marker, its line ending included, did not end before the first byte sent |
| `step.response` | the index of the step, from `0` | fail | for each step: it was not armed or not sent, or no response was complete within its `timeout_ms` of its send |
| `step.declared_none` | `sends` | fail | instead of `step.response`, for an entry with `sends = false`: a byte was sent |
| `firmware.version` | the name of the firmware version line | fail | for each `[entries.firmware.<name>]`, in the order of the names: no line of it appeared, or a line of it does not give one of the expected values |
| `segment.digest` | the name of the segment | fail | for each segment: its two lines did not both appear, or the bytes between them have another SHA-256 |
| `tftp.server` | `servers` | fail | a line names another TFTP server than the instance's |
| `tftp.prefix` | `reads` | fail | the path of a file read in full is outside the path prefix |
| `tftp.unplanned_file` | `reads` | fail | a file read in full inside the path prefix is not a file of the entry's payload, compared ignoring ASCII case |
| `identity.disagree` | `identity` | fail | the lines of an identity field or part give different values |

### The supply

The readings that follow a power command, up to the next power command,
confirm it when `consecutive` of them in a row are of the `quantity` of the
confirmation parameters, each at or above `on_at_least_micro` for a power on
or at or below `off_at_most_micro` for a power off, and all taken at or
after the time of the command and within `on_timeout_ms` or `off_timeout_ms`
of it. A reading of another quantity, or on the other side of the threshold,
starts the count again.
The supply was last confirmed off at the time of the reading that confirms
the last power command of the events before, when that command is a power
off; for the first cold boot of a run, those are `start.events`, and for any
other, the events of the cold boot before it, in the same entry or the one
before.

The events of a cold boot are in order when neither their times nor the
bytes received before them ever decrease, and when they keep these phases,
in this order: the console ready; the power on and the readings after it;
the sends; the end; the power off and the readings after it; the console
closed. A device error can come in any phase.

### Verdicts

- A cold boot is `"BENCH FAULT"` when one of its checks is a fault, else
  `"FAIL"` when one failed, else `"PASS"`.
- An entry with no event, no check and no cold boot is `"NOT RUN"`. Any
  other entry is `"BENCH FAULT"` when one of its checks is a fault or one of
  its cold boots is `"BENCH FAULT"`, else `"FAIL"` when one of its cold boots
  is `"FAIL"`, else `"PASS"` when it ran every cold boot of its plan entry;
  an entry that stopped short of them without a failure or a fault is
  `"BENCH FAULT"`.
- A run is `"BENCH FAULT"` when one of its entries is `"BENCH FAULT"` or
  `"NOT RUN"`, which only a fault leaves; else `"FAIL"` when one is
  `"FAIL"`; else `"PASS"`. A refused run has no verdict.

A cold boot that does not pass is the last of its entry: an entry fails at
its first failed cold boot, and the next entry follows. A fault ends the
run, and every entry after the one that holds it is `"NOT RUN"`. When
readings did not confirm the last power off before the fault ended the run,
the run switches the supply off once more and records that command, its
readings and any failure of a device in the `events` of the entry that holds
the fault, and its error output says whether readings confirmed that power
off.

### Refused runs

A run whose payload the [payload checks](#payload-checks) refuse is refused
before any power command, and its receipt shows it:

- `refusal` is `{ rule, entry, subject, detail }`: the rule, the index of
  the entry that it concerns, the payload name of the file that it found,
  and what was found, as in `the staged copy of config.txt was not read`;
- `verdict` is `null`, since a refused run is not judged;
- `start.events` is empty, and every entry is `"NOT RUN"`, with no event, no
  check and no cold boot.

The rules that refuse a run are `payload.source_digest`,
`payload.staged_digest`, `write.not_enabled`, `write.refused_property` and
`payload.variant_missing`, and each names its entry and its file. Every
receipt has exactly one of `refusal` and `verdict`.

### Reading a receipt

The library reads a receipt strictly:

1. The text must be JSON in UTF-8. Its `format` and `format_version` are
   read first.
2. Every key is read. A key that the format does not have, a missing key, an
   event of another kind, a value of the wrong type or form, such as a
   digest that is not 64 lowercase hexadecimal digits, and base64 that is
   not canonical are errors; the `detail` of a check that passed is the only
   key that is left out.
3. The text must be exactly the rendering of what it holds, so any edit, of
   a value, of white space, of the order of keys, of an escape or of the
   final newline, is refused.
4. The parts must agree:
   - `nonce` is 32 lowercase hexadecimal digits, and `tool.commit` is 40 or
     `null`;
   - `evidence` keeps the [rules of evidence](#evidence);
   - each recorded text has its SHA-256; the recorded plan is a run plan of
     this version, and the recorded description a board description of this
     version whose `id` is `board_description.id`; the plan passes the
     [checks of a run plan](#checks-of-a-run-plan) against that description
     and the recorded pinned sets; and the recorded parameters keep the
     rules of an instance configuration, the identity record, read as an
     identity record is, its checks against the description included;
   - `reference` is that of the plan, and `entries` has one record for each
     entry of the plan, in order, with its index, `name`, `reference`,
     `payload_pin` as `payload.pin`, and the names of its payload files;
   - exactly one of `refusal` and `verdict` is not `null`, a refused run is
     recorded as [refused runs](#refused-runs) are, and `refusal.entry` is
     the index of an entry of the plan;
   - only an entry is `"NOT RUN"`, and it has no cold boot; an entry has at
     most the cold boots of the plan's entry, numbered from 0;
   - each `serial` has the `length` and an `index` of its bytes, of at most
     65536 chunks, and at most the `max_capture_bytes` of the plan's entry;
   - each check has `detail` exactly when it did not pass.

Reading a receipt does not check the rule names of checks and refusals
against the lists above, so that the rules can be refined without a new
version of the format, nor the order and times of events, nor whether the
observations, checks and verdicts follow from the record:
[re-deriving verdicts](#re-deriving-verdicts) does.

## Public versions

A public version holds a receipt with every value that identifies the board,
the network or the bench replaced by `#`, byte for byte, and lists what was
replaced, so that the record of a run can be published from a bench. The
library makes it from the receipt, with the sensitive lines of the board
description that the receipt records and of the description of the same `id`
built into the tool, and the
[values to hide](#values-to-hide) of the instance configuration, and gives it
only when the [leak check](#the-leak-check) finds none of those values in it.
It is written in the form of a receipt and
[read as strictly](#reading-a-public-version). A public version is for
reading: where bytes were replaced it no longer shows what the board sent,
and the receipt stays the record of the run.

When the cold boot of a receipt received these lines, each ended by CR LF,
on a bench whose TFTP server is `192.0.2.10` and whose path prefix is empty:

```text
  4.37 BOOTMODE: 0x06 partition 0 build-ts BUILD_TIMESTAMP=1700000000 serial 1234abcd boardrev d04171 stc 4371234
  5.51 NETWORK: 00:00:5e:00:53:01 wait for link TFTP: 192.0.2.10
READY
```

its public version, with the receipt digest shortened here, starts like this:

```json
{
  "format": "kyto-bench public receipt",
  "format_version": 1,
  "receipt_sha256": "a78c47f0...",
  "placeholder": "#",
  "withheld": [
    {
      "field": "/nonce",
      "length": 32,
      "rule": "nonce"
    },
    {
      "field": "/inputs/instance/parameters/tftp/server_address",
      "length": 10,
      "rule": "instance.tftp.server_address"
    }
  ],
  "replacements": [
    {
      "field": "/entries/0/cold_boots/0/serial/bytes_base64",
      "offset": 77,
      "length": 8,
      "rules": [
        "board.board-serial"
      ]
    },
    {
      "field": "/entries/0/cold_boots/0/serial/bytes_base64",
      "offset": 131,
      "length": 17,
      "rules": [
        "board.network-link"
      ]
    },
    {
      "field": "/entries/0/cold_boots/0/serial/bytes_base64",
      "offset": 169,
      "length": 10,
      "rules": [
        "board.network-link",
        "instance.tftp.server_address"
      ]
    }
  ],
  "receipt": {
    "format": "kyto-bench receipt",
    "format_version": 1,
    "nonce": "################################",
```

| Key | Content |
| --- | --- |
| `format`, `format_version` | `"kyto-bench public receipt"`, `1` |
| `receipt_sha256` | The SHA-256 of the receipt file, the bytes of its rendering |
| `placeholder` | `"#"`, the byte that replaces every byte of a value |
| `withheld` | The [fields always withheld](#withheld-fields), each as `{ field, length, rule }` |
| `replacements` | The [replacements](#replacements), each as `{ field, offset, length, rules }` |
| `receipt` | The receipt with every withheld field and every replacement made, its keys in the same order |

`field` is a JSON pointer into the receipt, such as
`/entries/0/cold_boots/0/serial/bytes_base64`, and offsets and lengths count
bytes. Neither list records a replaced value.

### Withheld fields

`withheld` lists the fields that a public version withholds whatever any rule
finds, in this order, each unless it is empty: `/nonce`, rule `nonce`;
`/inputs/instance/parameters/tftp/server_address`, rule
`instance.tftp.server_address`; and
`/inputs/instance/parameters/tftp/path_prefix`, rule
`instance.tftp.path_prefix`. Each becomes as many `#` as it has bytes. The
nonce is never empty, so it is always withheld.

### Values to hide in a receipt

A public version must not show these values, each found under its rule:

- every hit: each recorded stream, the console output of a cold boot or the
  bytes of a `send` event, is decoded and cut into pieces at every LF and
  every CR; each piece is searched for every match of each
  [sensitive line](#sensitive-lines) of the board description that the
  receipt records, and of the description of the same `id` built into the
  tool when it differs, wherever the match starts, each match after the end
  of the one before; and the bytes of each named group of each match are a
  hit of the rule `board.<name>`, unless they are empty or one of the
  `never` values of the description whose line matched. A CR left where an
  LF was lost, a stray byte before or after a line, and two lines run
  together thus keep no line from its form, and a recorded description that
  lacks a form of the built-in one hides no less;
- the TFTP server address and path prefix of
  `inputs.instance.parameters.tftp`;
- the `relay` of each `power` event, under the rule
  `instance.local.power.relay`;
- the [values to hide](#values-to-hide) of the instance configuration;
- the nonce, under the rule `nonce`.

Values shorter than 4 bytes are not searched for, since they would be found
in unrelated bytes; the span of a hit is replaced whatever its length. Values
are found ignoring ASCII case, so a MAC address that a later line prints in
upper case is found too. A
receipt with more than 1024 values to hide besides the nonce, a value
counted once for each rule that finds it, or with more than 64 KiB of them
in all, has no public version: the leak check fails under the rule
`public.values_limit`, with the whole receipt, the empty pointer, as its
field.

### Replacements

Each of these spans is replaced, every byte by `#`:

- in each recorded stream, the span of every hit;
- in each stream and in every other string of the receipt, every occurrence
  of a value to hide of at least 4 bytes other than the nonce;
- every `relay` of a `power` event, whole, under the rule
  `instance.local.power.relay`. A relay number shorter than 4 bytes is not
  searched for, since it would be found in unrelated bytes; it is replaced
  where the receipt records it. A longer one is also a value to hide, found
  wherever it occurs;
- the unterminated last line of the console output of each cold boot, whole,
  under the rule `console.unterminated_line`: a sensitive line cut short by
  the end of a capture may not match its form, though it holds a value.

Spans that overlap or are equal make one replacement, whose `rules` lists
every rule that found them, once each, sorted. `offset` counts the decoded
bytes of a stream or the UTF-8 bytes of a string; in a string, a span is
widened to whole characters, so that the string stays UTF-8. A stream is
encoded again from its replaced bytes, so its `length` and `index` stay the
same. `replacements` are in the order of their fields in the receipt, and
in the order of their offsets within a field.

The recorded texts of the run plan and the board description, every digest
(the `sha256` keys, the keys that end in `_sha256` and the value of the
hardware revision), the keys of objects and numbers are never rewritten, and
the withheld fields have no replacement. A value to hide in a recorded text
or a digest stays, and the leak check refuses the public version.

### The leak check

Before the library gives a public version, it checks that:

- the public version passes [its reading](#reading-a-public-version);
  otherwise the check fails under the rule `public.form`, with the JSON
  pointer, in the public version, of what reading refuses as its field;
- no value to hide of at least 4 bytes, the nonce included, occurs, ignoring
  ASCII case, in any string of its receipt or in the decoded bytes of its
  streams, the recorded texts and the digests included;
- the sensitive lines of the board description that it records, and of the
  description of the same `id` built into the tool, matched again in its
  streams, hold nothing but `#`;
- every `relay`, and the unterminated last line of the console output of
  each cold boot, hold nothing but `#`;
- no segment of `observations.segments` covers a replaced byte of its cold
  boot's console output, from the first byte of the line of `from` up to
  the first byte of the line of `to`; such a segment fails under the rule
  `segment.covers_hidden`, at its `sha256`.

When a check fails, the library gives no public version, only the rule and
the field of each value found, never the value. A hit inside a digest, such
as a board serial number that is part of a payload digest, and an instance
value in the text of the run plan, such as the TFTP server address in a
comment, both fail it: a run whose plan holds a value to hide has no public
version. A receipt whose recorded board description cannot be read has none
either: making it fails under `public.form`, with the field
`/inputs/board_description/text`.

### Reading a public version

The library reads a public version as strictly as a receipt: its `format`
and `format_version` first, every key with none missing or unknown, and a
text that is exactly the rendering of what it holds. Then the parts must
agree:

- `placeholder` is `"#"`;
- `withheld` lists exactly the [withheld fields](#withheld-fields) that
  `receipt` holds as strings that are not empty, in their order and with
  their lengths, `/nonce` among them, and each of those strings is all `#`;
- each replacement names a string of `receipt` that a public version
  rewrites, not a recorded text, a digest or a withheld field; its span is
  not empty and lies within the decoded bytes of the stream, or the UTF-8
  bytes of the string, and holds only `#`; it comes after the replacement
  before it, in the order of the fields in `receipt` and of the offsets
  within a field, without overlapping it; and its `rules` are not empty,
  sorted and distinct;
- `receipt`, read with `0` for each replaced byte of a string, such as a
  payload name of which a hit replaced a part, is a receipt of version 1 in
  the form of the format: every key, none missing or unknown, every value of
  its type and form, and its keys as a receipt renders them, in their order,
  with `null` for each absent value and no `detail` of a check that passed.
  The rules that tie the parts of a receipt
  together are not checked, since replaced values no longer show what the
  run recorded; its board description must be one that the library reads.

A replaced byte of a fixed word of the format, such as a check result,
leaves `receipt` out of form, so its public version fails reading and the
leak check: the library refuses such a public version rather than give one
that cannot be read.

### Why the receipt digest can be published

`receipt_sha256` ties a public version to its receipt. The receipt holds the
nonce, 128 random bits that the public version withholds, so the digest
cannot be checked against guesses of the replaced values, however few they
are, and reveals none of them. The digest of the instance configuration file
in `inputs.instance.sha256` is protected in the same way by the
[salt](#local-settings) of the configuration. The digest of a segment has no
such secret: it covers bytes that the public version shows, but for those it
replaced, which could be found by trying their values one by one against
it. That is why the leak check refuses a segment over a replaced byte.

The digests of payload files, in the run plan, the pinned sets and the
records of entries and boot files, are shown in full and have no secret
either. They reveal nothing as long as payload files hold no value of the
bench, as [run plans](#run-plans) require.

## Re-deriving verdicts

A receipt holds every input of the checks and verdicts of its run: the run
plan and the board description as read, the instance parameters, the
digests of every payload file, every event and the console output of every
cold boot. The library function `kyto_bench_core::rejudge::rejudge` takes a
receipt that has been [read](#reading-a-receipt) and makes its judgement
again from these alone, without a payload, an instance configuration, a
device or a clock:

1. It reads the recorded run plan and board description again.
2. It makes [`evidence`](#evidence) again from `tool` and `bench.simulated`.
3. It makes the [payload checks](#payload-checks) again, in their order,
   from the recorded digests: the source digests from `source_sha256`, the
   write triggers from the payload names and the entry's `writes`, the
   variants from the recorded identity record, and the staged digests from
   `staged_sha256`. Whether a file sets a refused property shows only in
   its bytes, which a receipt does not hold, so a refusal under
   `write.refused_property` is taken as the receipt records it, at its
   place in the order: a check that comes before it and fails gives its own
   refusal instead. The first check that fails gives the refusal, and a
   refused run has no verdict.
4. For a run that was not refused, for each entry in order, it makes the
   [checks of the entry](#checks); for each of its cold boots,
   [what the cold boot shows](#how-a-cold-boot-is-read), from its console
   output and the `send` events among its events, its checks, with the time
   at which [the supply](#the-supply) was last confirmed off before it, and
   its verdict; then the [verdicts](#verdicts) of the entry and of the run.

It fails, naming the place in the receipt, when a recorded text cannot be
read, when an index does not describe its bytes, when a cold boot follows
one of its entry that did not pass, and when an entry that is not
`"NOT RUN"` follows one that is `"BENCH FAULT"`. Otherwise it gives the
receipt with its evidence, refusal, observations, checks and verdicts made
again and everything else as recorded. `kyto-bench run` makes the checks and
verdicts of a receipt with the same functions, so a receipt that the tool
wrote is given back unchanged; one that is not was not made by the rules of
this version from what it records. In Rust, with the library as a
dependency:

```rust
use kyto_bench_core::receipt::Receipt;
use kyto_bench_core::rejudge::rejudge;

fn rederives_to_itself(path: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let receipt = Receipt::parse(&std::fs::read(path)?)?;
    Ok(rejudge(&receipt)? == receipt)
}
```

For example, take the cold boot of the receipt in
[entries and cold boots](#entries-and-cold-boots), with its plan entry: the
marker `ready`, `end = { marker = "ready" }`, `boot_timeout_ms = 60000`, the
step `{ after = "ready", send = "?\r", expect = '^STATS seq=1', timeout_ms = 1000 }`
and the payload `config.txt`, `bcm2712-rpi-5-b.dtb` and
`overlays/bcm2712d0.dtbo`, the files that the D0 stepping of the board of
the [example configuration](#instance-configurations) needs. The run powered
the board off at 0 s and read 0 µV at 0.1 s, and the board is served the
generation that holds the payload, whose copies all have the digests of the
plan. Re-deriving gives:

- no refusal: every source and staged digest is that of the plan, no file is
  a write trigger, and the payload has the files of the variant;
- the checks of the entry: `power.off_unconfirmed` on `start` passes, since
  0 µV is at most 500000 µV and one reading in a row confirms the off within
  3000 ms, and `payload.served_digest` passes for each of the three files;
- the observations of the cold boot as its record shows them, and these
  checks, each passing: `console.not_ready`, since the console was ready at
  5.1 s; `power.on_unconfirmed`, since 5100000 µV at 5.2 s confirms the power
  on at 5.1 s; `power.off_interval`, since the console was ready and the
  supply switched on 5000 ms after the off was confirmed at 0.1 s;
  `power.off_unconfirmed`, since 0 µV at 7.8 s confirms the power off at
  7.7 s; `power.order`, since the console ready, the power on, the reading,
  the send, the end, the power off, the reading and the console closed come
  in this order, at times and after byte counts that never decrease;
  `console.before_power_on`, since the first chunk came at 6.1 s, after the
  power on at 5.1 s;
  `boot.network` and `boot.fallback` on `board.network_boot`, since line 0
  is the only boot mode line and is of the network mode; `boot.end` on
  `ready`, whose line 41 came at 7.6 s, within 60000 ms of the power on;
  `console.byte_limit`, since the cold boot ended at its end condition;
  `step.response` on `0`, since line 41 armed the step, event 3 sent it at
  7.6 s and the response, line 48, was complete at 7.7 s, within 1000 ms;
  and `tftp.server`, `tftp.prefix`, `tftp.unplanned_file` and
  `identity.disagree`, since no line names a server, reads a file or gives
  an identity value;
- the verdicts: the cold boot is `"PASS"`, the entry, which ran its one cold
  boot, `"PASS"`, and the run `"PASS"`.
