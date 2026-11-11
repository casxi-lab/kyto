# Scope

This page describes the complete target scope of kyto: what it will provide,
which other parts of casξ it works with, and the status of each part. No part
or form has been chosen, and kyto has no design files, models or bench
program yet, so every part has the status `planned`.

| Status | Meaning |
| --- | --- |
| `planned` | Described on this page; nothing is designed, modelled, written, built or fitted |
| `partial` | Part of the item is designed, modelled, written, built, measured or fitted; the row links each record and states what remains |
| `built` | The item is designed, modelled or written, and made in a stated revision. Its electrical results, design rule checks, power-flow results, dimension checks, interference checks, print checks, fit tests and bench checks are recorded separately and linked from the row; this status does not say that any of them passed |

When a part changes status, the same change gives its row the new status and
links its design files, models, programs and records.

## Responsibilities

kyto designs the hardware of a portable device built on a whole Raspberry
Pi 5 board, and the bench used to check it:

- **Electrical design.** A peripheral carrier board connects the input,
  display, power and debug connections to the Raspberry Pi 5; the processor
  board itself is not redesigned. The work covers the choice of the screen,
  keyboard and power parts, the circuits and boards around them, the bills of
  materials and manufacturing outputs, and the measurements that show what
  the hardware does: electrical rule checks, wiring, signals, power
  consumption, the power flow and tests on built boards.
- **Structural design.** The whole-device assembly, the enclosure, the keycaps
  and key mechanism, the thermal parts, parametric models and printed test
  parts. It checks dimensions, interference, key travel, prints and the fit of
  the actual parts. Paper mockups and partial prints verify the layout first;
  the three-dimensional fit is checked before the carrier board is frozen; the
  enclosure is frozen only after fit tests with the actual parts.
- **Hardware-in-the-loop bench.** A bench tool and bench hardware that run
  images on real boards and record what happens, without depending on any one
  bench or on the system under test. The same bench serves the checks of
  kyto's own hardware and the runs of the systems built for the device.

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
| Product screen | A screen module and its connection to the Raspberry Pi 5: resolution, orientation, pixel format, interface, update regions, backlight and refresh target, and the electrical interface and timing that krepi's display driver uses. Until it exists, development uses a separate display | krepi display driver and service; the enclosure's screen window and mounting | `planned` |
| Keyboard | The key count and layout, the switches, the key matrix and its debouncing, and the path by which key presses reach the Raspberry Pi 5: a keyboard controller that appears as a USB HID device, or scanning through GPIO or I2C. Press, release and repeat, modifier keys and combinations, and the response to cancel reach the software through krepi's input driver. Until it exists, development uses a USB keyboard | krepi input driver; the keycaps and key mechanism; kosmo key layout | `planned` |
| Power | A battery with its charger and fuel gauge, the supply to the Raspberry Pi 5 and the power button. The shutdown flow saves state, stops computation, cuts power and powers up again as separate steps; a real power-off needs the external power circuit. Battery capacity and runtime are fixed only after the consumption has been measured. Until it exists, the Raspberry Pi 5 runs from a stable external supply | krepi power management service, temperature and fan policy and shutdown; the enclosure's battery space, button and port positions | `planned` |
| Debug and expansion connections | Access to the dedicated debug UART of the Raspberry Pi 5, kept apart from the peripherals behind the RP1 I/O controller; expansion connections with stated pins, buses, voltage levels and connectors | krepi drivers | `planned` |
| Carrier board | The peripheral carrier board that joins the screen, keyboard, power and debug connections to the Raspberry Pi 5, with mounting holes and connector positions agreed with the enclosure and assembly | Enclosure; assembly | `planned` |
| Bills of materials | For every design revision, the parts it uses with their manufacturers, part numbers, quantities and reference designators | — | `planned` |
| Manufacturing outputs | For every released revision, the files sent to a manufacturer, generated from the committed design revision with the pinned tool version | — | `planned` |
| Measurements | Electrical rule checks, wiring, signals, power consumption at idle, under computation load and with the display and peripherals, the fan, temperature and throttling, shutdown and wake, and tests on built boards; each record names the design revision and the board | krepi for tests with the real system; thermal parts and fit tests | `planned` |
| Data acquisition and expansion hardware | Sensors and expansion controllers for data acquisition and controlled outputs, with their electrical definitions | krepi acquisition service and controlled input and output | `planned` |
| Connection hardware | The wireless or USB device-mode hardware that the device's connections to a computer need, once krepi has decided which connections the device offers | krepi connections | `planned` |

### Structural design

| Part | What kyto will provide | Works with | Status |
| --- | --- | --- | --- |
| Paper and size mockups | Paper mockups and partial prints that verify the layout before the enclosure is modelled, with nominal dimensions from cited sources and measured dimensions recorded separately | Screen, keyboard and battery dimensions | `planned` |
| Enclosure | A housing for the whole Raspberry Pi 5 board and the carrier board, screen, keyboard and battery: the screen window, the button and port positions, the height limit, and the space for cables and maintenance | Carrier board, screen, keyboard and battery | `planned` |
| Keycaps and key mechanism | Keycaps and the key mechanism with its key travel, fitted to the chosen switches | Keyboard switches and key pitch | `planned` |
| Assembly | The board edges and mounting holes of the Raspberry Pi 5, its ports, cooler and fan, the screen window, the height limit, the key travel, the cables and the maintenance space | Carrier board | `planned` |
| Thermal parts | The space and placement of the cooler and fan of the Raspberry Pi 5; the thermal design that krepi's temperature thresholds depend on | krepi temperature and fan policy; power measurements | `planned` |
| Parametric models and exports | Editable parametric models and assembly sources; STEP exports for exchange with the board design; print models, all generated from a committed model revision | Board design | `planned` |
| Printed test parts and print checks | Printed test parts with their print checks, each recorded with the model revision, printer and material | — | `planned` |
| Fit tests | Records of the actual parts fitted together, with nominal and measured dimensions apart; the enclosure is frozen only after them | Boards and parts | `planned` |

### Hardware-in-the-loop bench

| Part | What kyto will provide | Works with | Status |
| --- | --- | --- | --- |
| Bench tool | A Rust program under `tools/` that drives a bench through device interfaces: power control with a confirmation of power-on and power-off that does not rely on the relay's own reply, the serial console, image delivery and network boot, input injection, display capture and measurement. Host tests run it against simulated devices | krepi runs on the Raspberry Pi 5; measurements and sample checks | `planned` |
| Board descriptions | For each supported board, how it boots and what its boot log must show, such as the network boot markers and the firmware versions; the Raspberry Pi 5 comes first | krepi version records | `planned` |
| Run plans and receipts | A run plan names the image by digest and the steps to run; a receipt records the tool revision, the bench hardware revision, the image digest, the commands sent, the console transcript and every observation, with a verdict. A run refuses an image whose digest does not match, and a run that fails powers the board off and says that it did | krepi version records and regression runs | `planned` |
| Bench self-test | A pinned self-test image and run that show a bench works; its result says nothing about any system tested on the bench | — | `planned` |
| Input injection and display capture | A device that injects key presses (press, release, repeat, modifier keys, long press and cancel) as a USB HID keyboard under the bench's control, and a device that captures the display and compares regions or test patterns with what is expected; the receipt records the time from input to display and from cancel to the final state. A failed injection and a frame that did not change are each detected | krepi input and display tests; product screen and keyboard | `planned` |
| Bench hardware | The adapter boards, fixtures and other parts that connect a bench to a board under test, with design files under `bench/` | Electrical and structural design rules | `planned` |

## Rules every design follows

- The Raspberry Pi 5 is used as a whole board. Its interfaces, board edges,
  mounting holes, ports, cooler and fan, connectors and cable space are taken
  from its official drawings or measured on the board, never carried over
  from older boards.
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

- **Form.** Not chosen; no size, key count or arrangement is fixed.
- **Screen and keyboard.** Not chosen; no key count, size or interface is
  fixed.
- **Battery and power circuit.** Not chosen; development uses a stable
  external supply.
- **Design tools.** The electronic and mechanical design tools have not been
  chosen. The directory layout, file formats and the checks that run on
  design files and models follow that choice.
- **Display, input and rendering stack.** Not decided in krepi; the screen
  and keyboard interfaces follow that decision.
- **Thermal design.** The cooler, fan and airflow have not been chosen; they
  follow measurements of the device under sustained load.
- **Measurement data and binary files.** Where raw measurement files are
  kept, up to what size they are committed, and whether large exports, print
  models and photos are kept in Git directly or with a large-file extension
  have not been decided.
- **Response times.** No numeric targets for start-up, cancellation or
  response times have been set; they will be set after measurements on a
  prototype.
- **Bench devices.** The power confirmation, input injection and display
  capture devices have not been chosen.
