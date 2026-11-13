<picture>
  <source media="(prefers-color-scheme: dark)" srcset=".github/assets/logo-dark.svg">
  <img alt="kyto" src=".github/assets/logo.svg" height="64">
</picture>

# kyto

kyto is the hardware of casξ, a portable workstation for mathematics, science
and programming that comes in two forms, a landscape handheld and a clamshell,
both built on the Raspberry Pi Compute Module 5. It will hold three things:
the electrical design (the product screen, the keyboard, the battery and power
circuits, the debug and expansion connections, the carrier boards and the
bills of materials), the structural design (the enclosure, the keycaps and key
mechanism, the assembly and the thermal parts), and the hardware-in-the-loop
bench used to develop and check the device on real hardware. The name comes
from the Greek κύτος, a vessel or body.

The drivers and system software that use the hardware, and the joint tests of
the device with the real system, belong to krepi, the casξ operating system;
see [Related repositories](#related-repositories).

## What it will provide

Electrical design:

- **Product screen.** For each form, a screen module and its connection to
  the compute module, with a stated resolution, orientation, pixel format,
  interface, update regions, backlight and refresh target.
- **Keyboard.** For each form, the key count and layout, the switches, the
  key matrix and its debouncing, and the path by which key presses reach the
  compute module: either a keyboard controller that appears as a USB HID
  device, or scanning through GPIO or I2C.
- **Power.** A battery with its charger and fuel gauge, the supply to the
  compute module, the power button, and a shutdown flow that saves state,
  stops computation, cuts power and powers up again as separate steps.
- **Debug and expansion connections.** Access to the compute module's
  dedicated debug UART, which the module does not bring out on its
  board-to-board connectors, kept apart from the peripherals behind the RP1
  I/O controller; a covered service port for the bench's test signals; and
  externally reachable USB host ports with a stated current allowance. There
  is no on-board expansion header; data acquisition uses a separate box.
- **Carrier boards.** For each form, a carrier board for the compute module
  that holds the circuits that power, boot and debug it and connects the
  input, display and power parts to it; these circuits are one design shared
  by both carrier boards. The compute module itself is not redesigned.
- **Bills of materials.** For every released design revision, the parts it
  uses, with their manufacturers and part numbers.
- **Measurements.** Records of wiring, signals, power consumption and the
  power flow, and measurements on built boards, each tied to the design
  revision and the board it was taken on.

Structural design:

- **Enclosure.** For each form, a housing for the carrier board with its
  compute module, the screen, keyboard and battery (a single shell for the
  landscape handheld, two halves joined by a hinge for the clamshell), with
  the screen window, the button and port positions, the height limit and the
  space needed for cables and maintenance.
- **Keycaps and key mechanism.** Keycaps and the key mechanism with its key
  travel, fitted to the chosen switches.
- **Assembly.** The mounting holes and stacking height of the compute module,
  the board edges and ports of the carrier boards, the cooling parts and fan,
  the screen window, the height limit, the key travel, the cables, including
  those through the hinge of the clamshell, and the maintenance space, with
  nominal and measured dimensions recorded separately.
- **Thermal parts.** The cooling parts, fan and air path for the compute
  module in each form. krepi's temperature thresholds depend on this thermal
  design.
- **Parametric models and printed test parts.** Editable parametric models
  and assembly sources, exported models for exchange with the board design,
  print models, and printed test parts with their print checks.
- **Paper and size mockups.** Paper mockups and size studies of the layout,
  made before any enclosure is modelled.
- **Fit tests.** Records of parts fitted together: the enclosure is frozen
  only after fit tests with the actual parts.

Hardware-in-the-loop bench:

- **Bench tool.** A program that drives a bench without depending on any one
  bench: power control with a confirmation that does not rely on the relay's
  own reply, the serial console, image delivery and network boot, input
  injection, display capture and measurement.
- **Board descriptions.** How each supported board boots and what its boot
  log must show; the Raspberry Pi 5 comes first.
- **Run plans and receipts.** What a run does, and a receipt that records the
  tool revision, the bench hardware revision, the image digest and every
  observation, with a verdict.
- **Bench self-test.** A run that shows a bench works, separate from the
  results of any system it tests.
- **Bench hardware.** The adapter boards, fixtures and injection or capture
  devices a bench needs, with their design files.

Anyone will be able to set up a bench of their own from kyto. Each bench
keeps its own instance configuration, such as addresses, relay channels and
local paths, outside this repository; kyto will hold templates and examples
only.

[Scope](docs/scope.md) describes each of these in detail, together with what
kyto relies on from the other parts of casξ.

## Status

kyto is newly initialized. The repository holds documentation, the kyto logo,
a commit hook and the repository checks, which CI runs on every pull request
to `main` and every push to `main`. The two forms and the compute module they
are built on are fixed; no other part has been chosen, and the screen size,
the key count and the arrangement within each form are open. Nothing has been
designed or modelled yet: there are no design files and no models, and nothing
to build, order, print or assemble. The bench tool in `tools/` is partly
written: its library reads [board descriptions, identity records, instance
configurations and run plans](docs/bench/formats.md), with the Raspberry Pi 5
description built in, checks payloads, reads what a cold boot shows, and
writes and reads receipts and their public versions, and the tool has the
interfaces of four kinds of device, each with one simulated device: power
commands, a supply meter that also confirms power-on and power-off, the
serial console, and payload delivery to the directory that the board reads
over TFTP. It has no command that runs a bench, no driver for a real device,
and no input injection or display capture yet, so no run makes a receipt.
Apart from the bench tool, the board descriptions and the formats of run
plans and receipts, none of the items above exists today. Until a power
circuit exists, the development boards, a whole Raspberry Pi 5 board and a
Compute Module 5 in its official IO board, run from a stable external supply.

The repository checks run from the repository root:

```sh
cargo xtask check
```

The checks are a Rust program in `xtask/`; rustup installs the toolchain
pinned in `rust-toolchain.toml` on first use. [Contributing](CONTRIBUTING.md)
describes the steps of the check, how files will be organized and the commit
rules.

## How the files will be organized

No design file or model exists yet, the bench tool under `tools/` cannot run
a bench yet, and the electronic and mechanical design tools have not been
chosen; the formats of the design files follow that choice. When work starts,
the repository will keep:

- the editable design sources of each electrical module (schematics and board
  layouts) and the parametric models and assembly sources of each part, in the
  native formats of the chosen tools;
- the bill of materials of each design revision, next to its design sources;
- the manufacturing outputs, STEP exports and print models of each released
  revision, generated from a committed revision and never edited by hand;
- measurement, mockup, print and fit-test records, each stating the revision
  and the board or part it was taken on, with nominal and measured values
  kept apart;
- the bench tool, written in Rust, under `tools/`;
- the design files of the bench hardware under `bench/`.

Electrical results, design rule checks, power-flow results, dimension checks,
interference checks, print checks, fit tests and bench checks are recorded
separately; one never stands for another. [Contributing](CONTRIBUTING.md)
states the rules in full.

## Related repositories

- **krepi** (`casxi-lab/krepi`): the casξ operating system. It will drive the
  hardware: the display and input drivers, the power management service with
  battery level, charging state, backlight, sleep and saving before a
  low-battery shutdown, the temperature and fan policy, which depends on
  kyto's thermal design, and the controlled shutdown. krepi decides the
  display, input and rendering stack that the screen and keyboard connect to.
  It will obtain its results on the Raspberry Pi 5 by running a pinned
  revision of kyto's bench tool against its own images, and it decides what
  those runs must show. The joint tests of the hardware with the real system belong to
  krepi.
- **kosmo** (`casxi-lab/kosmo`): the casξ desktop environment and application
  framework. It draws into the surfaces that appear on the screen and builds
  its interaction on the key layout.

## Documentation

- [Scope](docs/scope.md): the complete target scope of kyto and the status of
  each part.
- [Contributing](CONTRIBUTING.md) and [repository instructions](AGENTS.md).

## License

kyto uses two licenses, chosen by path. Every file under `tools/`, `xtask/`,
`.githooks/`, `.github/` and `.cargo/`, and `rust-toolchain.toml`, is
released under the [MIT License](LICENSE-MIT); the bench tool is under
`tools/`. Every other file, including the design files, models, bills of
materials, manufacturing outputs, exports, print models, the bench hardware
under `bench/`, the measurement, mockup and fit-test records, the README and
the other documentation, is released under the CERN Open Hardware Licence
Version 2 – Permissive ([LICENSE-CERN-OHL-P-2.0](LICENSE-CERN-OHL-P-2.0)).
Copyright (c) 2026 The casξ Authors; see [AUTHORS](AUTHORS).
