# Bench self-test

The bench self-test is a payload for the Raspberry Pi 5 and a
[run plan](run-plan.toml) that show that a bench works: the board boots from
the network, reads the payload over TFTP and starts it, writes no EEPROM,
and the serial console carries bytes both ways. Its result says nothing
about any system tested on the bench.

On a bench, the firmware has loaded and started this kernel, which wrote
its ready line and answered the line it was sent. What this page says about
the rest of the firmware's hand-over comes from the sources it cites.

## Files

| File | Bytes | SHA-256 | Source |
| --- | --- | --- | --- |
| `kernel_2712.img` | 409 | `ad5c2ffab80d0d531acace80f606eca2330a67410b549b8556313c83959b1030` | This directory, written by `kyto-self-test` from [its source](src/lib.rs) |
| `config.txt` | 16 | `221e6f20771e01705b21758eddce44d06c5ec6252db3dbb7e55e609a7a15ec39` | This directory |
| `bcm2712-rpi-5-b.dtb` | 78770 | `03ce4742452ad52e3e8ec2259a0c705b496970e2b75e6e0b4e5446475ea1b926` | [`boot/bcm2712-rpi-5-b.dtb`](https://github.com/raspberrypi/firmware/blob/dcca4969e53d2a25e688f0f228a09486786d54c0/boot/bcm2712-rpi-5-b.dtb) of raspberrypi/firmware at `dcca4969e53d2a25e688f0f228a09486786d54c0` |
| `overlays/bcm2712d0.dtbo` | 1363 | `b73210c9256ff4b4963365f9acc49c0f7449f17eeddf21355e23dd011da899ec` | [`boot/overlays/bcm2712d0.dtbo`](https://github.com/raspberrypi/firmware/blob/dcca4969e53d2a25e688f0f228a09486786d54c0/boot/overlays/bcm2712d0.dtbo) of the same commit |

The two device tree files are the official ones and come under their own
licences, so kyto does not hold them: whoever sets up a bench takes them from
that commit. The firmware reads the overlay only on a board of the D0
stepping; a bench whose board's stepping is unknown or D0 pins both files.

`config.txt` holds one setting, `uart_2ndstage=1`, with which the firmware
writes its later log, including the line `Starting OS` that the board
description's marker `os_start` reads, to the debug UART. It names no kernel:
the Raspberry Pi 5 firmware loads `kernel_2712.img` by default. It does not
turn off the firmware's OS check, which looks for a device tree file
compatible with the board, and the official device tree file is one
([config.txt boot options](https://github.com/raspberrypi/documentation/blob/21d772e7ee022e36a8f43c3ee75e20ca359fb832/documentation/asciidoc/computers/config_txt/boot.adoc),
`kernel`, `uart_2ndstage` and `os_check`).

## What the kernel does

The kernel writes the line `kyto self-test: ready` once, after an empty
line. It then answers each line that it receives with one line: `kyto
self-test: echo `, the line as received, ` (`, its number of bytes in decimal
and ` bytes)`, the text and the count both without the CR or LF that ended
the line, such as `kyto self-test: echo hello bench (11 bytes)`. An empty line
gets no answer, so CR LF ends one line. It writes nothing else, and no line
of its own has the form of an answer. Each byte of a line is written back as
it arrives, so a line of any length is answered, but a line sent while the
answer to the one before is still being written can lose bytes once the
UART's 32-byte receive FIFO fills.

The kernel is the instruction words of [`src/lib.rs`](src/lib.rs), written
by a Rust program of this workspace, so its bytes follow from that source
without an assembler or a cross compiler. Its 64-byte header is that of an
arm64 Linux image
([`Documentation/arch/arm64/booting.rst`](https://github.com/raspberrypi/linux/blob/703bc4e92a9f98d797f0289bf2adb1e8b04c4bbb/Documentation/arch/arm64/booting.rst)
of the Linux kernel) with a load offset of 0, and its first word branches
over the header.

## What it relies on

- **Where it runs.** Every branch and every address that the kernel forms
  is relative to the instruction that uses it, and it reads only its own
  bytes and the UART: it has no stack and writes no memory. It therefore
  runs at whatever address the firmware loads it to, provided that execution
  starts at its first byte. The firmware passes the address that it loaded
  the kernel to, and the device tree, to the BL31 stub that it starts the
  cores in, and BL31 enters the kernel at that address
  ([`plat/rpi/rpi5/rpi5_bl31_setup.c`](https://github.com/raspberrypi/arm-trusted-firmware/blob/36761b2765044b62c5485be8fcc109fbd3dae451/plat/rpi/rpi5/rpi5_bl31_setup.c)
  of raspberrypi/arm-trusted-firmware at the commit that the BL31 version
  `v2.6(release):v2.6-241-g36761b276` names, which the BL31 of the recorded
  boot below prints). The firmware has been seen
  to load a Linux `kernel_2712.img` at offset `0x200000` in a recorded
  Raspberry Pi 5 boot from an SD card
  ([`fallback-to-sd.bin`](../kyto-bench/tests/data/recorded/fallback-to-sd.bin)),
  and to load this kernel at the same offset on a bench; the kernel does not
  depend on that.
- **Exception level and state.** BL31 enters the kernel at EL2 with every
  interrupt masked
  ([`plat/rpi/rpi5/rpi5_private.c`](https://github.com/raspberrypi/arm-trusted-firmware/blob/36761b2765044b62c5485be8fcc109fbd3dae451/plat/rpi/rpi5/rpi5_private.c),
  `rpi5_get_spsr_for_bl33_entry`), as the arm64 boot protocol asks, which
  also asks for the MMU to be off. The kernel reads no system register but
  `MPIDR_EL1`, which it can read at EL1 and EL2, takes no exception and uses
  only aligned 32-bit accesses to the UART and byte loads of its own text,
  which work with the MMU off.
- **Cores.** The device tree gives every core the enable method `psci`, and
  BL31 goes on only on the core whose `MPIDR_EL1` core number is 0 and holds
  the others until a PSCI call starts them
  ([`plat/rpi/rpi5/aarch64/plat_helpers.S`](https://github.com/raspberrypi/arm-trusted-firmware/blob/36761b2765044b62c5485be8fcc109fbd3dae451/plat/rpi/rpi5/aarch64/plat_helpers.S)).
  Any core whose affinity fields `Aff0` to `Aff2` in `MPIDR_EL1` are not all
  0 that reaches the kernel all the same waits for an event, for ever.
- **The UART.** The debug UART connector, labelled `UART`, is UART10, an Arm
  PL011, at the physical address `0x107d001000`: the device tree of the
  Raspberry Pi 5 (`arch/arm64/boot/dts/broadcom/bcm2712.dtsi` and
  `bcm2712-rpi-5-b.dts` of
  [raspberrypi/linux at `703bc4e92a9f98d797f0289bf2adb1e8b04c4bbb`](https://github.com/raspberrypi/linux/blob/703bc4e92a9f98d797f0289bf2adb1e8b04c4bbb/arch/arm64/boot/dts/broadcom/bcm2712.dtsi))
  puts `serial@7d001000`, compatible with `arm,pl011`, in a bus that maps
  address 0 to `0x1000000000`, and the documentation names UART10 the
  primary UART of the Raspberry Pi 5, on that connector
  ([interfaces](https://github.com/raspberrypi/documentation/blob/21d772e7ee022e36a8f43c3ee75e20ca359fb832/documentation/asciidoc/computers/configuration/interfaces.adoc)).
  The bootloader writes its log there at 115200 baud, 8N1, when its
  configuration sets `BOOT_UART=1`
  ([bootloader configuration](https://github.com/raspberrypi/documentation/blob/21d772e7ee022e36a8f43c3ee75e20ca359fb832/documentation/asciidoc/computers/raspberry-pi/eeprom-bootloader.adoc)),
  as the board description's console settings require. BL31 for the
  Raspberry Pi 5 is built to keep the UART as the firmware set it when its
  control register is not 0, and to set it to 115200 baud, 8N1, with
  transmission and reception on otherwise
  ([`plat/rpi/rpi5/platform.mk`](https://github.com/raspberrypi/arm-trusted-firmware/blob/36761b2765044b62c5485be8fcc109fbd3dae451/plat/rpi/rpi5/platform.mk),
  `PL011_INHERIT_SETUP`, and
  [`drivers/arm/pl011/aarch64/pl011_console.S`](https://github.com/raspberrypi/arm-trusted-firmware/blob/36761b2765044b62c5485be8fcc109fbd3dae451/drivers/arm/pl011/aarch64/pl011_console.S)),
  and writes its own `NOTICE:` lines there just before it enters the
  kernel. The kernel keeps the rate, the frame and the FIFO setting that it
  finds, which are those with which the bench reads the firmware's log, and
  does not know the UART's input clock. It sets the control register's
  `UARTEN`, `TXE` and `RXE` bits, in case the firmware left reception off,
  and keeps its other bits; where they are set already, the write changes
  nothing. It then discards what the UART received before it started.

## Running it

A bench's instance configuration pins the four files above, by their names
and SHA-256, as the set `self-test`, and its payload root holds them in the
directory `self-test`. With the bench's files in place of the placeholders,
from the repository root:

```sh
cargo run --locked --release --manifest-path tools/Cargo.toml -- run \
  --plan tools/self-test/run-plan.toml \
  --instance <file> --payload-root <dir> --out <dir>
```

The plan's first entry, `firmware`, is one cold boot that must boot from
the network without falling back to the SD card, read `config.txt`, the
kernel and the device tree over TFTP, each of its size, show no EEPROM write
or update and end at `Starting OS`. The second, `echo`, is one cold boot that
sends `hello bench` and CR once the kernel's ready line has come after
`Starting OS`, and ends once the answer has come. The plan expects no
firmware version: the receipt records the versions that the board's
firmware prints. Its receipt is kept with the bench's private instance.

The tests of the workspace run the plan on simulated devices with
[an example instance configuration](../examples/self-test-instance.toml),
whose console replays [an example console output](../examples/captures/self-test.bin)
written for the example, not recorded from a board.

## Writing the kernel again

This command writes the kernel image from its source and prints its
SHA-256:

```sh
cargo run --locked --manifest-path tools/Cargo.toml -p kyto-self-test -- \
  tools/self-test/kernel_2712.img
```

A test of the workspace fails unless `kernel_2712.img` is the image that the
source gives and the table above gives its size and SHA-256. Another runs
the kernel on a model of the instructions that it uses, of `MPIDR_EL1` and
of the UART's registers, and checks what it writes to the UART. A change of the
kernel changes both, and every bench pins the new image again.
