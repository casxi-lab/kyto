# Scope

This page describes the complete target scope of kyto: what it will provide,
which other parts of casξ it works with, and the status of each part. No form
has been chosen and kyto has no models yet, so every part has the status
`planned`.

| Status | Meaning |
| --- | --- |
| `planned` | Described on this page; nothing is modelled, printed or fitted |
| `partial` | Part of the item is modelled, printed or fitted; the row links each record and states what remains |
| `built` | The item is modelled and made in a stated model revision. Dimension checks, interference checks, print checks and fit tests are recorded separately and linked from the row; this status does not say that any of them passed |

When a part changes status, the same change gives its row the new status and
links its models and records.

## Responsibilities

kyto designs the structure of a portable device built on a whole Raspberry
Pi 5 board: the whole-device assembly, the enclosure, the keycaps and key
mechanism, the thermal parts, parametric models and printed test parts. It
checks dimensions, interference, key travel, prints and the fit of the
actual parts. Paper mockups and partial prints verify the layout first; the
three-dimensional fit is checked before the carrier board is frozen; the
enclosure is frozen only after fit tests with the actual parts.

Other parts of casξ work with kyto's structure:

- **kyma**, the electrical design, provides the screen, the keyboard switches,
  the battery and the carrier board that kyto houses. kyto keeps the detailed
  dimension sources and kyma keeps the PCB and electrical sources; both are
  checked against the same revision before either is released, with nominal
  and measured dimensions recorded separately.
- **krepi**, the operating system, sets the temperature and fan policy, which
  depends on kyto's thermal design, and carries out the joint tests of the
  whole device with the real system, including the measurements of heat and
  energy under sustained load.

## Target scope by part

| Part | What kyto will provide | Works with | Status |
| --- | --- | --- | --- |
| Paper and size mockups | Paper mockups and partial prints that verify the layout before the enclosure is modelled, with nominal dimensions from cited sources and measured dimensions recorded separately | kyma screen, keyboard and battery dimensions | `planned` |
| Enclosure | A housing for the whole Raspberry Pi 5 board and kyma's boards, screen, keyboard and battery: the screen window, the button and port positions, the height limit, and the space for cables and maintenance | kyma boards, screen, keyboard and battery | `planned` |
| Keycaps and key mechanism | Keycaps and the key mechanism with its key travel, fitted to the switches that kyma chooses | kyma keyboard switches and key pitch | `planned` |
| Assembly | The board edges and mounting holes of the Raspberry Pi 5, its ports, cooler and fan, the screen window, the height limit, the key travel, the cables and the maintenance space | kyma carrier board | `planned` |
| Thermal parts | The space and placement of the cooler and fan of the Raspberry Pi 5; the thermal design that krepi's temperature thresholds depend on | krepi temperature and fan policy; kyma power measurements | `planned` |
| Parametric models and exports | Editable parametric models and assembly sources; STEP exports for exchange with kyma's board design; print models, all generated from a committed model revision | kyma board design | `planned` |
| Printed test parts and print checks | Printed test parts with their print checks, each recorded with the model revision, printer and material | — | `planned` |
| Fit tests | Records of the actual parts fitted together, with nominal and measured dimensions apart; the enclosure is frozen only after them | kyma boards and parts | `planned` |

## Rules every design follows

- The Raspberry Pi 5 is used as a whole board. Its board edges, mounting
  holes, ports, cooler and fan, connectors and cable space are taken from its
  official drawings or measured on the board, never carried over from older
  boards.
- Every nominal dimension of a bought part comes from a cited drawing or
  datasheet; measured values are recorded apart from nominal values.
- Models are edited by hand; exports and print models are generated, never
  edited.
- Dimension checks, interference checks, print checks and fit tests are
  recorded separately; one never stands for another.

## Open questions

- **Form.** Not chosen; no size, key count or arrangement is fixed.
- **Design tools.** The mechanical and electronic design tools have not been
  chosen. The directory layout, file formats and the checks that run on
  models follow that choice.
- **Thermal design.** The cooler, fan and airflow have not been chosen; they
  follow measurements of the device under sustained load.
- **Binary files.** Whether large exports, print models and photos are kept
  in Git directly or with a large-file extension has not been decided.
