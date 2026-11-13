# Repository instructions

- kyto holds the hardware of casξ: the electrical design (the product screen
  and its connection, the keyboard and its scanning, the battery, charging and
  power circuits, the debug and expansion connections, the carrier boards, the
  bills of materials and the measurement records), the structural design (the
  enclosure, the keycaps and key mechanism, the assembly, the thermal parts,
  parametric models and printed test parts, paper and size mockups, and
  fit-test records), and the hardware-in-the-loop bench (the bench tool and
  the bench hardware). Drivers, the power management service, the temperature
  and fan policy, the shutdown policy and the choice of what a system's bench
  runs must show belong to krepi; the user interface belongs to kosmo. Do not
  design them here.
- Never invent a part, a part number, a parameter or a dimension. Every
  parameter or nominal dimension of a bought part comes from a cited
  datasheet, specification or drawing, with its revision; a value that cannot
  be found is written as unknown, not estimated. Measured values are recorded
  apart from nominal values.
- A part enters a design or a model only after the decision that selects it
  is recorded in the project's coordination records, outside this repository.
  Do not order samples, boards, parts or prints, do not open supplier
  accounts, and do not request quotes or samples without authorization.
- Develop on the whole Raspberry Pi 5 board and on the Raspberry Pi Compute
  Module 5 in its official IO board, and use the Compute Module 5 on the
  carrier boards of the product. Take interfaces, board edges, mounting holes,
  connector heights, ports, cooling parts and cable space from the official
  drawings and datasheets of the board or module concerned or from
  measurements on it, never from older boards and never from one another.
  Neither the Raspberry Pi 5 nor the compute module is redesigned.
- Keep the dedicated debug UART of the Raspberry Pi 5 and of the compute
  module apart from the peripherals behind the RP1 I/O controller.
- A connector on the board is not a working function: the fan, the
  temperature readings and the power button need drivers and a policy in
  krepi before they count as working.
- Start and measure with a stable external supply. The rating of the
  recommended supply is not the device's continuous consumption: measure idle
  load, computation load, the display and peripherals, the fan, temperature
  and throttling, and shutdown and wake behavior before battery capacity or
  runtime is fixed. Consumption and temperatures measured on the whole
  Raspberry Pi 5 do not stand for the Compute Module 5.
- A successful boot does not show that a supply path carries full load.
  Check the rating, voltage drop, heating and USB-C negotiation of a supply
  path separately. Never cut only one of several parallel conductors to
  switch power.
- The shutdown flow defines saving state, stopping computation, cutting power
  and powering up again as separate steps.
- Keep the PCB and electrical sources and the detailed dimension sources
  consistent: check shared dimensions against the same revision before either
  side is released.
- Verify the layout with paper mockups and partial prints first. Check the
  three-dimensional fit before the carrier board is frozen, and freeze the
  enclosure only after fit tests with the actual parts. Leave room for the
  cooling parts and fan that the thermal design needs; krepi's temperature
  thresholds depend on that design.
- Keep design sources and models editable in the native format of the chosen
  tool, and models parametric. Generate manufacturing outputs, exports and
  print models from a committed revision and never edit them by hand.
- Record every measurement, print check and fit test with the design or model
  revision, the board or parts used, the instruments or the printer and
  material, the method, the values measured and the result. Keep nominal and
  measured values apart, and record electrical results, design rule checks,
  power-flow results, dimension checks, interference checks, print checks,
  fit tests and bench checks separately; one never stands for another.
- Never report a physical result that was not observed. A record of a step
  done by hand, such as unpacking a sample, wiring a prototype, taking a
  measurement, building a paper mockup, printing a part or fitting parts
  together, states only what was observed and what was not measured.
- Write the bench tool in Rust under `tools/`, with external dependencies
  only from crates.io and pinned to exact versions. It must not depend on any
  one bench or on the system under test, and must confirm power-on and
  power-off with a signal that does not rely on the relay's own reply.
  Operations that write firmware, EEPROM contents or storage media stay off
  unless a run plan enables them explicitly. Never commit a bench's instance
  configuration, such as addresses, relay channels, local paths or
  credentials; commit templates and examples only. Bench hardware design files
  go under `bench/`.
- The rules that decide a verdict, check an image digest and power a board
  off after a failure need written requirements and a contract, contract
  tests with negative cases, and fuzz tests on untrusted input such as console
  transcripts. Test the rest of the bench tool on the host against simulated
  devices. Receipts are written by the tool and never edited by hand.
- Write all documentation and comments in English, from the point of view of
  a person who builds, checks or uses the device: what a part does, its
  limits, and commands that actually run. Write mathematics in LaTeX as
  `CONTRIBUTING.md` describes. `docs/scope.md` holds the target scope and the
  status of each part; mark anything that does not exist as planned.
- Keep milestone, task, proposal and decision numbers, capability and
  interface codes, stage codes, agent narratives, review histories, evidence
  ledgers, supplier quotes, prices and lead times in the project's
  coordination records outside this repository. Documents here state the
  technical content: the rule, the condition that lifts a limit, or the part
  or module that will hold a planned item. Requirement ids of the bench
  tool, such as `VERD-R01`, `PROV-R01` and `POWER-R01`, are labels defined
  in `docs/bench/requirements.md`, not plan numbers, and documents, comments
  and tests may use them.
- kyto uses two licenses, chosen by path. Every file under `tools/`,
  `xtask/`, `.githooks/`, `.github/` and `.cargo/`, and `rust-toolchain.toml`,
  is released under the MIT License; the bench tool is under `tools/`. Every
  other file, including the design files, models, bills of materials,
  manufacturing outputs, exports, print models, the bench hardware under
  `bench/`, the measurement, mockup and fit-test records, the README and the
  other documentation, is released under the CERN Open Hardware Licence
  Version 2 – Permissive. Put a new file where its license applies.
- Use English Conventional Commit subjects of at most 72 characters and the
  local `.githooks/commit-msg` hook. Preserve history. Do not create remotes,
  tags or releases, and do not order anything, without authorization.
- kyto has no design files or models yet. Its Cargo workspaces are the
  repository tasks in `xtask/` and the bench tool in `tools/`, which runs
  run plans on simulated devices only; `cargo xtask check` runs the format,
  Clippy and test steps in both. Before committing, run `cargo xtask check`,
  `git diff --check` and `git diff --cached --check` and review the staged
  diff. CI runs `cargo xtask check` on every pull request to `main` and every
  push to `main`. The change that adds the first design file or model also
  adds the checks of the chosen design tool and updates `CONTRIBUTING.md` and
  these instructions.
