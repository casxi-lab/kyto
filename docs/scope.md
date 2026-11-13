# Scope

This page describes the complete target scope of kyto: what it will provide,
which other parts of casξ it works with, and the status of each part. The two
forms and the compute module they are built on are fixed; no other part has
been chosen, and the screen size, the key count and the arrangement within
each form are open. kyto has no design files or models yet. The bench tool,
the board descriptions and the run plans and receipts are partly written,
and the bench tool cannot run a bench yet; every other part has the status
`planned`.

| Status | Meaning |
| --- | --- |
| `planned` | Described on this page; nothing is designed, modelled, written, built or fitted |
| `partial` | Part of the item is designed, modelled, written, built, measured or fitted; the row links each record and states what remains |
| `built` | The item is designed, modelled or written, and made in a stated revision. Its electrical results, design rule checks, power-flow results, dimension checks, interference checks, print checks, fit tests and bench checks are recorded separately and linked from the row; this status does not say that any of them passed |

When a part changes status, the same change gives its row the new status and
links its design files, models, programs and records.

## Responsibilities

kyto designs the hardware of a portable device in two forms, a landscape
handheld and a clamshell, both built on the Raspberry Pi Compute Module 5, and
the bench used to check it:

- **Electrical design.** A carrier board for each form holds the circuits that
  power, boot and debug the compute module and connects the input, display
  and power parts to it; these circuits are one design shared by both carrier
  boards, and the compute module itself is not redesigned. The development
  boards are the whole Raspberry Pi 5 board and the Compute Module 5 in its
  official IO board. The work covers the choice of the screen and
  keyboard of each form and of the power parts, the circuits and boards around
  them, the bills of materials and manufacturing outputs, and the measurements
  that show what the hardware does: electrical rule checks, wiring, signals,
  power consumption, the power flow and tests on built boards.
- **Structural design.** The whole-device assembly, the enclosure, the keycaps
  and key mechanism, the thermal parts, parametric models and printed test
  parts. It checks dimensions, interference, key travel, prints and the fit of
  the actual parts. Paper mockups and partial prints verify the layout first;
  the three-dimensional fit is checked before each carrier board is frozen;
  each enclosure is frozen only after fit tests with the actual parts.
- **Hardware-in-the-loop bench.** A bench tool and bench hardware that run
  images on real boards and record what happens, without depending on any one
  bench or on the system under test. The same bench serves the checks of
  kyto's own hardware and the runs of the systems built for the device. A
  whole Raspberry Pi 5 board, which has the same processor and I/O controller
  as the compute module, is a development board and the first board the bench
  will support.

The detailed dimension sources and the PCB and electrical sources are both
kept in kyto; they are checked against the same revision before either is
released, with nominal and measured dimensions recorded separately.

Other parts of casξ work with kyto's hardware:

- **krepi**, the operating system, will drive it. It provides the display
  and input drivers, the power management service (battery level, charging
  state, backlight, sleep and saving before a low-battery shutdown), the
  temperature and fan policy, which depends on kyto's thermal design, and the
  controlled shutdown. It decides the display, input and rendering stack that
  the screen and keyboard connect to. krepi will obtain its results on the
  Raspberry Pi 5 by running a pinned revision of kyto's bench tool against
  its own images, and it decides which runs it needs and what they must show.
  The joint tests of the hardware with the real system, including the
  measurements of heat and energy under sustained load, belong to krepi.
- **kosmo**, the desktop environment and application framework, draws into
  the surfaces that appear on the screen and builds its interaction on the key
  layout.

## Target scope by part

### Electrical design

| Part | What kyto will provide | Works with | Status |
| --- | --- | --- | --- |
| Product screen | For each form, a screen module and its connection to the compute module: resolution, orientation, pixel format, interface, update regions, backlight and refresh target, and the electrical interface and timing that krepi's display driver uses. Until it exists, development uses a separate display | krepi display driver and service; the enclosure's screen window and mounting | `planned` |
| Keyboard | For each form, the key count and layout, the switches, the key matrix and its debouncing, and the path by which key presses reach the compute module: a keyboard controller that appears as a USB HID device, or scanning through GPIO or I2C. Press, release and repeat, modifier keys and combinations, and the response to cancel reach the software through krepi's input driver. Until it exists, development uses a USB keyboard | krepi input driver; the keycaps and key mechanism; kosmo key layout | `planned` |
| Power | A battery with its charger and fuel gauge, the supply to the compute module and the power button. The shutdown flow saves state, stops computation, cuts power and powers up again as separate steps; a real power-off needs a power circuit outside the compute module. Battery capacity and runtime are fixed only after the consumption has been measured. Until it exists, the development boards run from a stable external supply | krepi power management service, temperature and fan policy and shutdown; the enclosure's battery space, button and port positions | `planned` |
| Debug and expansion connections | Access to the compute module's dedicated debug UART, which the module does not bring out on its board-to-board connectors, kept apart from the peripherals behind the RP1 I/O controller. The Compute Module 5 used in the official IO board gets a three-pin header fitted to its existing, unpopulated top-side footprint; fitting the header does not redesign the module, and the change is recorded with the module's sample record. The product carrier boards use the 4.0 mm stacking height and reach the module's rear test points TP35 and TP36 with spring-loaded pins, which a trial board confirms first (Raspberry Pi Compute Module 5 datasheet, release 3, §2.8 and §4.1). A covered service port, with the same pin order on both forms, carries the debug UART and the test signals the bench uses. Expansion goes only through externally reachable USB host ports, each current-limited and protected and with a stated current allowance (at least one on each device, at least two on the landscape handheld); there is no on-board expansion header. The carrier boards state the RP1 GPIO pins that krepi's power, thermal and lifecycle services use, with their voltage levels. Data acquisition and controlled outputs are in a separate acquisition box, not on the device | krepi drivers | `planned` |
| Carrier boards | For each form, the carrier board for the compute module: the circuits that power, boot and debug it, which are one design shared by both forms, and the connections of the screen, keyboard and power parts, with mounting holes and connector positions agreed with the enclosure and assembly | Enclosure; assembly | `planned` |
| Bills of materials | For every design revision, the parts it uses with their manufacturers, part numbers, quantities and reference designators | — | `planned` |
| Manufacturing outputs | For every released revision, the files sent to a manufacturer, generated from the committed design revision with the pinned tool version | — | `planned` |
| Measurements | Electrical rule checks, wiring, signals, power consumption at idle, under computation load and with the display and peripherals, the fan, temperature and throttling, shutdown and wake, and tests on built boards; each record names the design revision and the board. Raw files of measurements taken by hand, not by the bench tool, and photos are kept outside this repository: each record names them by file name and SHA-256, and the repository holds the summary tables. The raw output of a bench run is its receipt, kept with the bench instance that ran it, outside this repository | krepi for tests with the real system; thermal parts and fit tests | `planned` |
| Data acquisition and expansion hardware | A separate acquisition box, connected to the device over USB, holds the expansion controller, the analog and timing front ends, the digital ports and the controlled outputs, with their electrical definitions; an industrial I/O version is a variant of the same box. The box's first release covers the channel classes of the TI CBL 2 and the Vernier LabQuest Mini: three analog channels, two digital channels, ultrasonic distance and photogate timing, and chemistry probes such as pH, with two logic-level controlled outputs; pin compatibility with BTA probe connectors is not part of the first release. Chemistry probes bring storage and disposal requirements, which the records and the user documentation state. The box is not isolated: it has input protection, common-ground rules and an optional differential input. The ground potential difference between the box and the circuit it measures is measured with the device charging, connected to a computer and on battery, and a marked ground loop reopens the isolation decision; isolated channels come with the industrial variant. The box's USB transport and device class follow krepi's acquisition interface. The box is developed and released separately from the devices, with revisions of its own; the devices do not wait for it, and each device configuration lists the box revisions it works with | krepi acquisition service and controlled input and output; the devices' USB host ports | `planned` |
| Connection hardware | Wireless: a compute module with wireless, connected through U.FL to the official antenna kit certified with the module, routed to a plastic edge of the enclosure or the top edge of the clamshell's lid. The `WL_nDisable` and `BT_nDisable` signals reach a hardware switch or the service port, so wireless can be switched off in hardware. On compute module boards, bench regression runs keep wireless off; a run that krepi needs with wireless on enables it through `WL_nDisable` and `BT_nDisable` on the service port, and the receipt records the state. There is no RJ45 jack: the Ethernet pairs go to the service port, which a first board confirms, and the bench connects them through an adapter with the magnetics and an RJ45 jack for network boot; USB device boot is the recovery and fallback path. The compute module's USB 2.0 port is brought out only on the service port, for flashing, recovery and USB device mode if the device offers it. Which connections the device offers to a computer follows krepi's decision | krepi connections | `planned` |

### Structural design

| Part | What kyto will provide | Works with | Status |
| --- | --- | --- | --- |
| Paper and size mockups | Paper mockups and partial prints that verify the layout before the enclosure is modelled, with nominal dimensions from cited sources and measured dimensions recorded separately | Screen, keyboard and battery dimensions | `planned` |
| Enclosure | For each form, a housing for the carrier board with its compute module, the screen, keyboard and battery (a single shell for the landscape handheld, two halves joined by a hinge for the clamshell): the screen window, the button and port positions, the height limit, and the space for cables and maintenance | Carrier boards, screen, keyboard and battery | `planned` |
| Keycaps and key mechanism | Keycaps and the key mechanism with its key travel, fitted to the chosen switches | Keyboard switches and key pitch | `planned` |
| Assembly | The mounting holes and stacking height of the compute module, the board edges and ports of the carrier boards, the cooling parts and fan, the screen window, the height limit, the key travel, the cables, including those through the hinge of the clamshell, and the maintenance space | Carrier boards | `planned` |
| Thermal parts | The cooling parts, fan and air path for the compute module in each form; the thermal design that krepi's temperature thresholds depend on | krepi temperature and fan policy; power measurements | `planned` |
| Parametric models and exports | Editable parametric models and assembly sources; STEP exports for exchange with the board design; print models, all generated from a committed model revision | Board design | `planned` |
| Printed test parts and print checks | Printed test parts with their print checks, each recorded with the model revision, printer and material | — | `planned` |
| Fit tests | Records of the actual parts fitted together, with nominal and measured dimensions apart; the enclosure is frozen only after them | Boards and parts | `planned` |

### Hardware-in-the-loop bench

| Part | What kyto will provide | Works with | Status |
| --- | --- | --- | --- |
| Bench tool | A Rust program under `tools/` that drives a bench through device interfaces: power control with a confirmation of power-on and power-off that does not rely on the relay's own reply, the serial console, image delivery and network boot, input injection, display capture and measurement. Host tests run it against simulated devices | krepi runs on the Raspberry Pi 5; measurements and sample checks | `partial`: [`tools/`](../tools) holds the library that reads the [formats](bench/formats.md) of board descriptions, identity records, instance configurations and run plans, checks payloads, reads what a cold boot shows, writes and reads receipts and makes and checks their public versions, the write gate, and the interfaces of four kinds of device, each with one simulated device: power commands, a supply meter that also confirms power-on and power-off, the serial console, and payload delivery to the directory that the board reads over TFTP. A command that runs a bench and records what it measures, drivers for real devices, input injection and display capture remain |
| Board descriptions | For each supported board, how it boots and what its boot log must show, such as the network boot markers and the firmware versions; the Raspberry Pi 5 comes first | krepi version records | `partial`: the [Raspberry Pi 5 description](../tools/boards/raspberry-pi-5.toml), in the [board description format](bench/formats.md#board-descriptions). Checking a boot log against a description, which needs a command that runs a bench, and the descriptions of other boards remain |
| Run plans and receipts | A run plan names the image by digest and the steps to run; a receipt records the tool revision, the bench hardware revision, the image digest, the commands sent, the console transcript and every observation, with a verdict. A run refuses an image whose digest does not match, and a run that fails powers the board off and says that it did | krepi version records and regression runs | `partial`: the formats of [run plans](bench/formats.md#run-plans), [receipts](bench/formats.md#receipts) and their [public versions](bench/formats.md#public-versions), an [example run plan](../tools/examples/run-plan.toml), and the library code that reads plans, checks payloads, reads what a cold boot shows, writes and reads receipts and makes and checks public versions. The rules that decide checks and verdicts, the flow of a run, the re-derivation of verdicts from a receipt and the command that runs a plan and writes its receipt remain |
| Bench self-test | A pinned self-test image and run that show a bench works; its result says nothing about any system tested on the bench | — | `planned` |
| Input injection and display capture | A device that injects key presses (press, release, repeat, modifier keys, long press and cancel) as a USB HID keyboard under the bench's control, and a device that captures the display and compares regions or test patterns with what is expected; the receipt records the time from input to display and from cancel to the final state. A failed injection and a frame that did not change are each detected | krepi input and display tests; product screen and keyboard | `planned` |
| Bench hardware | The adapter boards, fixtures and other parts that connect a bench to a board under test, with design files under `bench/` | Electrical and structural design rules | `planned` |

## Rules every design follows

- The Raspberry Pi 5 is used as a whole board for development and on the
  bench, and the Compute Module 5 in its official IO board for development
  and on the carrier boards of the product; neither is redesigned. Their
  interfaces, board edges, mounting holes, connector heights, ports, cooling
  parts and cable space are taken from their official drawings and datasheets
  or measured on the board, never carried over from older boards or from one
  to the other.
- Every parameter or nominal dimension of a bought part comes from a cited
  datasheet, specification or drawing; measured values are recorded apart from
  nominal values.
- The dedicated debug UART is kept apart from the peripherals behind RP1.
- A connector is not a working function: the fan, the temperature readings
  and the power button need drivers and a policy in krepi.
- The rating of the recommended supply is not the device's continuous
  consumption. A successful boot does not show that a supply path carries
  full load; its rating, voltage drop, heating and USB-C negotiation are
  checked separately.
- Design sources and models are edited by hand; bills of materials list only
  parts whose selection is recorded; manufacturing outputs, exports and print
  models are generated, never edited.
- Electrical results, design rule checks, power-flow results, dimension
  checks, interference checks, print checks, fit tests and bench checks are
  recorded separately; one never stands for another.

## Rules every bench follows

- The bench tool does not depend on any one bench or on the system under
  test. A bench's instance configuration, such as addresses, relay channels,
  local paths and credentials, is kept outside this repository; kyto holds
  templates and examples only.
- Power-on and power-off are each confirmed by a signal that does not rely on
  the relay's own reply.
- Operations that write firmware, EEPROM contents or storage media are off by
  default and run only when a run plan enables them explicitly.
- Receipts are written by the tool and never edited. A result names the tool
  revision and the bench hardware revision it was taken with; when either
  changes, the results that depend on it are taken again.
- The rules that decide a verdict, check an image digest and power a board
  off after a failure have written requirements and a contract, contract tests
  with negative cases, and fuzz tests on untrusted input such as console
  transcripts.
- A bench self-test shows only that the bench works.

## Open questions

- **Form.** The two forms are fixed. The landscape handheld has a single
  shell, a screen of 5 to 6.9 inches diagonal used in landscape and a thumb
  QWERTY keyboard, with or without a mathematics and navigation key area. The
  clamshell has a screen of 7 to 8.8 inches diagonal in the lid and a compact
  QWERTY keyboard in the base, with the mathematics keys on a function layer.
  Within each form, the screen size, the key count, the arrangement and the
  place of the compute module are not fixed.
- **Compute module variant.** Both forms use the same Compute Module 5 order
  code, with wireless. The memory size, the on-board storage capacity and
  whether a Lite module without eMMC is used are chosen by a comparison
  before the design of the first form's carrier board starts, using
  measurements on the Compute Module 5 in the official IO board. Product change notices and successor modules of the
  compute module are tracked and checked against the design.
- **Screen and keyboard.** Not chosen; no key count, size or interface is
  fixed.
- **Battery and power circuit.** Not chosen; development uses a stable
  external supply.
- **Design tools.** The electronic and mechanical design tools have not been
  chosen. The directory layout, file formats and the checks that run on
  design files and models follow that choice.
- **Display, input and rendering stack.** Not decided in krepi; the screen
  and keyboard interfaces follow that decision.
- **Thermal design.** The cooling parts, fan and air path have not been chosen; they
  follow measurements of the device under sustained load.
- **Measurement data and binary files.** Raw files of measurements taken by
  hand are kept outside this repository, as the Measurements row states;
  small de-identified data tables may be committed later. Whether large exports and
  print models are kept in Git directly or with a large-file extension has
  not been decided; it follows the choice of the design tools.
- **Response times.** No numeric targets for start-up, cancellation or
  response times have been set; they will be set after measurements on a
  prototype.
- **Bench devices.** The power confirmation, input injection and display
  capture devices have not been chosen.
