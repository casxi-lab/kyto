# kyto

kyto is the structural design of casξ, a portable workstation for mathematics,
science and programming built on the Raspberry Pi 5. It will hold the
enclosure, the keycaps and key mechanism, the assembly of the Raspberry Pi 5
with the boards, screen, keyboard and battery around it, the thermal parts,
printed test parts, paper and size mockups, and fit-test records. The name
comes from the Greek κύτος, a vessel or body.

kyto covers only the structure of the device. The screen, keyboard switches,
power circuits and carrier board belong to kyma, the casξ electrical design,
and the temperature and fan policy and the shutdown that depend on the
structure belong to krepi, the casξ operating system; see
[Related repositories](#related-repositories).

## What it will provide

- **Enclosure.** A housing for a whole Raspberry Pi 5 board and kyma's boards,
  screen, keyboard and battery, with the screen window, the button and port
  positions, the height limit and the space needed for cables and
  maintenance.
- **Keycaps and key mechanism.** Keycaps and the key mechanism with its key
  travel, fitted to the switches that kyma chooses.
- **Assembly.** The board edges and mounting holes of the Raspberry Pi 5, its
  ports, cooler and fan, the screen window, the height limit, the key travel,
  the cables and the maintenance space, with nominal and measured dimensions
  recorded separately.
- **Thermal parts.** The space and placement of the cooler and fan of the
  Raspberry Pi 5. krepi's temperature thresholds depend on this thermal
  design.
- **Parametric models and printed test parts.** Editable parametric models
  and assembly sources, exported models for exchange with kyma, print models,
  and printed test parts with their print checks.
- **Paper and size mockups.** Paper mockups and size studies of the layout,
  made before any enclosure is modelled.
- **Fit tests.** Records of parts fitted together: the enclosure is frozen
  only after fit tests with the actual parts.

[Scope](docs/scope.md) describes each of these in detail, together with what
kyto relies on from the other parts of casξ.

## Status

kyto is newly initialized. The repository holds documentation, a commit hook
and the repository checks, which CI runs on every pull request to `main` and
every push to `main`. No form has been chosen and nothing has been modelled
yet: there are no models, nothing to print or assemble, and none of the items
above exists today.

The repository checks run from the repository root:

```sh
cargo xtask check
```

The checks are a Rust program in `xtask/`; rustup installs the toolchain
pinned in `rust-toolchain.toml` on first use. [Contributing](CONTRIBUTING.md)
describes the steps of the check, how files will be organized and the commit
rules.

## How the files will be organized

No model exists yet, and the mechanical design tool has not been chosen; the
formats follow that choice. When design work starts, the repository will
keep:

- the editable parametric models and assembly sources of each part, in the
  native format of the chosen tool;
- the exports of each released revision, such as STEP models for exchange
  with kyma's board design and print models, generated from a committed model
  revision and never edited by hand;
- mockup records: the layout a paper or size mockup was made from, its
  nominal dimensions and their sources, and the dimensions measured on it,
  kept apart from the nominal ones;
- fit-test and print records, each stating the model revision, the printed or
  bought parts used, the dimensions measured, and the result, with nominal
  and measured values kept apart.

Dimension checks, interference checks, print checks and fit tests are
recorded separately; one never stands for another.
[Contributing](CONTRIBUTING.md) states the rules in full.

## Related repositories

- **kyma**: the casξ electrical design. Its boards, screen, keyboard and
  battery are fitted into kyto's enclosure, and its switches carry kyto's
  keycaps. The detailed dimension sources are kept in kyto and the PCB and
  electrical sources in kyma; both are checked against the same revision
  before either is released.
- **krepi** (`casxi-lab/krepi`): the casξ operating system. Its temperature
  and fan policy depends on kyto's thermal design, and the whole-device
  measurements of heat and energy are made with kyto's structure. The joint
  tests of the whole device with the real system belong to krepi.

## Documentation

- [Scope](docs/scope.md): the complete target scope of kyto and the status of
  each part.
- [Contributing](CONTRIBUTING.md) and [repository instructions](AGENTS.md).

## License

kyto uses two licenses, chosen by path. Every file under `tools/`, `xtask/`,
`.githooks/`, `.github/` and `.cargo/`, and `rust-toolchain.toml`, is
released under the [MIT License](LICENSE-MIT). Every other file, including
the models, exports, print models, mockup records and fit-test records, the
README and the other documentation, is released under the CERN Open Hardware
Licence Version 2 – Permissive
([LICENSE-CERN-OHL-P-2.0](LICENSE-CERN-OHL-P-2.0)). Copyright (c) 2026 The
casξ Authors; see [AUTHORS](AUTHORS).
