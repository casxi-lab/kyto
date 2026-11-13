# Bench tool formats

This page describes the documents that the kyto bench tool in
[`tools/`](../../tools) reads: [board descriptions](#board-descriptions),
[identity records](#identity-records) and
[instance configurations](#instance-configurations), together with the
[rules common to them](#common-rules). The library of the tool,
`kyto-bench-core`, reads and checks each of them. The `kyto-bench` command
has no subcommand yet, so no command reads a document yet. The tests of the
tool read the Raspberry Pi 5 description, the identity record template and
example documents, and run from the repository root with:

```sh
cargo test --locked --manifest-path tools/Cargo.toml
```

Run plans, receipts and the public versions of receipts are not defined yet;
the tool has no format for them.

## Common rules

Every document is a TOML file in UTF-8 with two top-level keys that name its
format: `format`, a fixed name, and `format_version`, an integer. The tool
reads these two keys first, so a document of another format or version is
refused with its format and version named. This version of the tool reads
version 1 of each format, the only version so far, and a document is checked
against exactly the version it declares. Until a bench keeps the first record
of a run made with a version of a format, that version may still change; from
then on, any change to the format gives it a new version.

Documents are read strictly, at every level except inside the tables of
[local settings](#local-settings), whose keys belong to device drivers. A key
that the format does not have, a missing key and a value of the wrong type are
errors. The values are then checked against the rules that this page states
for each key, and an error names the value and the rule it breaks.

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
  neither `.` nor `..`, with no empty segment and no leading `/`, at most 255
  bytes. A TFTP directory on a case-insensitive file system serves two names
  that differ only in ASCII case as one file, and no directory holds a file
  and a directory of the same name, so the names in one list differ ignoring
  ASCII case, and none is, ignoring ASCII case, a directory of another, as
  `overlays` is of `overlays/a.dtbo`. A file name is a payload name of one
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
`$`. Its compiled form is at most 1 MiB. Named groups are written
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
describe what a cold boot shows; `visual` and `software` tell the person who
fills in an identity record how to read the value.

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
`firmware`, `eeprom` and `storage`. The tool has no class, permit or command
for any other kind of write, such as a write to one-time-programmable memory
or secure-boot provisioning.

`[[write_triggers]]` each give `class`, `files`, at least one file name, and
`source`. A payload file is a trigger of the class when its file name, the
last segment of its payload name, is one of `files` ignoring ASCII case. A
delivery device of the tool checks every payload file against the write
triggers before it writes anything: a trigger is staged only with a permit of
its class, and only the write gate of a run plan entry that enables the class
gives one; only the tool itself builds a write gate. Every class is off
unless an entry enables it; run plans are not defined yet, so nothing
enables a write. An entry's enable is needed for a write, but it does not
authorize one: whoever runs a plan that enables a write decides that outside
the tool.

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
  that it requests `bcm2712d0-rpi-5-b.dtb` on a D0 board. Both variants
  assume the defaults of `upstream_kernel`, `os_prefix`, `overlay_prefix` and
  `device_tree`: a payload whose `config.txt` sets one of them makes the
  firmware request other files, which the variants do not list and the tool
  does not detect.
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
  forms cover static addressing. `0.0.0.0` is never a value to hide.
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
in `[devices]`, and this machine's settings of each device in `[local]`. A
bench keeps its instance configuration outside this repository; kyto holds
none. This example uses documentation values only, with an empty path
prefix:

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
```

| Key or table | Required | Content |
| --- | --- | --- |
| `format`, `format_version` | yes | `"kyto-bench instance"`, `1` |
| `board` | yes | The `id` of the board description the bench is for |
| `[tftp]` | yes | `server_address`, the address of the TFTP server as the board's firmware prints it, and `path_prefix`, the prefix of every path that the board reads. The address, and the prefix unless it is empty, are 4 to 255 printable ASCII characters without spaces |
| `[confirmation]` | yes | How readings of the supply confirm that it is on or off, in micro-units of `quantity`, `"supply_voltage"` (microvolts) or `"supply_current"` (microamperes): `on_at_least_micro`, a reading at or above which the supply is on; `off_at_most_micro`, below the on threshold, a reading at or below which it is off; `consecutive`, the number of readings in a row that confirm a state, 1 to 1000; `poll_ms`, the interval between readings, 1 ms to one minute; `on_timeout_ms` and `off_timeout_ms`, the longest waits for each confirmation, 1 ms to one hour. The thresholds are integers and can be negative |
| `[console]` | yes | `ready_timeout_ms`, how long the console may take to be ready, 1 ms to one hour |
| `[identity]` | yes | The [identity record](#identity-records) of the board on the bench; its `board` is the `board` of the configuration |
| `[[pins]]` | no | Sets of payload digests that the bench pins, each with `name`, a name, and `files`, at least one `{ name, sha256 }` of a payload name and its SHA-256; the names of one set keep the rule for the names in one list of [payload names](#common-rules) |
| `[devices]` | yes | The [devices of the bench and their connections](#devices) |
| `[local]` | no | This machine's [settings of each device](#local-settings) |

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
relay channel or local path. The tool has no driver that a kind selects yet,
so it checks only the form of `kind`.

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

`[local]` holds this machine's settings of each device in the tables
`[local.power]`, `[local.power_confirmation]`, `[local.console]` and
`[local.delivery]`, each optional, and nothing else. Addresses, relay
channels, port paths, directories and credentials go there, never into
`[devices]`. The keys of each table belong to the driver of the role's kind;
the tool has no such driver yet and reads the tables only for the values to
hide.

### Values to hide

The tool lists the instance values that a record of a run published from the
bench must not show, each with the name of its rule:

- the TFTP server address, `instance.tftp.server_address`;
- the path prefix when it is not empty, `instance.tftp.path_prefix`;
- every string of at least 4 bytes in `[local]`, in arrays and nested tables
  too, `instance.local.<role>.<key>`, with the keys of nested tables joined
  by `.`.

Shorter strings would be found in unrelated bytes, and numbers are not
listed.

### Checks against the board description

A configuration is checked with the board description that its `board`
names: the `id` of the description equals `board`, and the identity record
passes the [checks of an identity record](#identity-records) against the
description.
