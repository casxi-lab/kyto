# Repository instructions

- kyto holds only the structural design of casξ: the enclosure, the keycaps
  and key mechanism, the assembly, the thermal parts, parametric models and
  printed test parts, paper and size mockups, and fit-test records. Circuits,
  boards, bills of materials and electrical measurements belong to kyma; the
  temperature and fan policy and the shutdown belong to krepi. Do not design
  them here.
- Never invent a dimension. Every nominal dimension of a bought part comes
  from a cited drawing or datasheet, with its revision; a value that cannot be
  found is written as unknown, not estimated. Measured values are recorded
  apart from nominal values.
- Use the Raspberry Pi 5 as a whole board. Take its board edges, mounting
  holes, ports, cooler and fan, connectors and cable space from its official
  drawings or from measurements on the board, never from older boards.
- Check dimensions shared with kyma against the same revision as kyma before
  either side is released. The detailed dimension sources are kept here; the
  PCB and electrical sources are kept in kyma.
- Verify the layout with paper mockups and partial prints first. Check the
  three-dimensional fit before the carrier board is frozen, and freeze the
  enclosure only after fit tests with the actual parts.
- Leave room for the cooler and fan that the thermal design needs; krepi's
  temperature thresholds depend on that design.
- Keep models editable and parametric in the native format of the chosen
  tool. Generate exports and print models from a committed model revision and
  never edit them by hand.
- Record every fit test and print check with the model revision, the parts
  used, the printer and material where a part is printed, the dimensions
  measured and the result. Record dimension checks, interference checks,
  print checks and fit tests separately; one never stands for another.
- A part enters a model only after the decision that selects it is recorded
  in the project's coordination records, outside this repository. Do not
  order parts or prints from a service, and do not request quotes, without
  authorization.
- Never report a physical result that was not observed. A record of a step
  done by hand, such as building a paper mockup, printing a part or fitting
  parts together, states only what was observed and what was not measured.
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
  that will hold a planned item.
- kyto uses two licenses, chosen by path. Every file under `tools/`,
  `xtask/`, `.githooks/`, `.github/` and `.cargo/`, and `rust-toolchain.toml`,
  is released under the MIT License. Every other file, including the models,
  exports, print models, mockup records and fit-test records, the README and
  the other documentation, is released under the CERN Open Hardware Licence
  Version 2 – Permissive. Put a new file where its license applies.
- Use English Conventional Commit subjects of at most 72 characters and the
  local `.githooks/commit-msg` hook. Preserve history. Do not create remotes,
  tags or releases, and do not order anything, without authorization.
- kyto has no models yet; the only Cargo package is the repository-task
  workspace in `xtask/`. Before committing, run `cargo xtask check`,
  `git diff --check` and `git diff --cached --check` and review the staged
  diff. CI runs `cargo xtask check` on every pull request to `main` and every
  push to `main`. The change that adds the first model also adds the checks of
  the chosen design tool and updates `CONTRIBUTING.md` and these
  instructions.
