# Contributing

kyto is newly initialized. It has no models yet; the only Cargo package is
the repository-task workspace in `xtask/`. Before proposing a change, enable
the commit message hook once and run these commands from the repository root:

```sh
git config --local core.hooksPath .githooks
cargo xtask check
git diff --check
git diff --cached --check
```

The first command enables the commit message hook in `.githooks/commit-msg`.
`cargo xtask check` runs the required checks that the next section describes.
The last two report whitespace errors in unstaged and staged changes.

## Building and checking

The Rust toolchain is pinned in `rust-toolchain.toml`, and rustup installs it
on first use. The repository tasks are a Cargo workspace of their own in
`xtask/`, with its own `Cargo.lock`, run as `cargo xtask`. `cargo xtask check`
runs these steps in order and stops at the first one that fails:

1. `fmt`, `clippy` with warnings as errors, and `test`, in the `xtask`
   workspace;
2. `docs`: relative links and anchors in the Markdown files;
3. `commits`: the header of every commit message in `<base>..HEAD` keeps to
   the rules of the commit message hook. The base is the commit that the
   environment variable `CHECK_BASE` names; when it is unset, empty or all
   zeros, the base is the merge base of `origin/main` and `HEAD`;
4. `workflows`: `.github/workflows/` holds only regular files, and every
   `.yml` or `.yaml` file there, in any subdirectory, is UTF-8 and one YAML
   document with scalar keys, without anchors, aliases, merge keys, repeated
   keys or characters that YAML parsers read differently. No workflow
   mentions `pull_request_target` in any letter case, not even in a comment
   or through an escape; every job sets its own `permissions`; and in a
   workflow that runs on `pull_request`, no job requests write access and
   nothing outside `on`, neither the workflow `env` nor any job, names
   `secrets` in an expression, an `if` condition or a key, even inside a
   string literal.

The `docs` and `workflows` steps read only files that Git tracks, so build
output and ignored files do not change the result; add a new file to the index
with `git add` before running the check. The `docs` step reads Markdown files
only; models and exports are not part of it.

`cargo xtask check --receipt <path>` also writes a JSON receipt of the run to
`<path>`, whether the check passes or fails. It records, in this order, the
receipt format version, the command (`cargo xtask check`), the commit checked
out when the run started, whether the working tree was clean before and after
the run, the result, each step with its name, status, duration in
milliseconds, the reason it failed if it did, and the values it observed, the
first line of `rustc --version`, the runner image (`ImageOS`, `ImageVersion`,
read from `/etc/environment` when the process environment lacks them) and the
GitHub Actions run (`GITHUB_RUN_ID`, `GITHUB_RUN_ATTEMPT`,
`GITHUB_EVENT_NAME`, `GITHUB_REF`, `GITHUB_SHA`). A value that cannot be
read, or an environment variable that is not set or is empty, is `null`. The
receipt is the pretty form of `serde_json`, with two-space indentation, and
ends in a newline.

CI runs the check as the job `cargo xtask check` of
`.github/workflows/check.yml` on a GitHub-hosted `ubuntu-24.04-arm` runner,
for the head commit of every pull request to `main` and for every push to
`main`. It sets `CHECK_BASE` to the base commit of the pull request or to the
commit before the push, installs the toolchain of `rust-toolchain.toml` with
rustup, and uploads the receipt as the artifact
`check-receipt-<commit>-<attempt>`.

The change that adds the first model also adds the checks of the chosen
design tool, such as reproducible exports, and updates this section and
[AGENTS](AGENTS.md) to match.

## Design files

The mechanical design tool has not been chosen, and no model exists yet. The
rules below apply from the first model; the directory layout and file
formats are fixed together with the tool.

- **Models.** Each part (for example the enclosure, the keycaps or a
  bracket) keeps its editable parametric model and its assembly sources in
  the native format of the chosen tool. Models are the only design files that
  are edited by hand.
- **Exports.** STEP models for exchange with kyma's board design and the
  print models of each released revision are generated from a committed model
  revision with the pinned version of the tool, kept with the revision they
  were generated from, and never edited by hand. Ordering prints or parts is a
  separate authorized action.
- **Interfaces with the rest of casξ.** Board outlines, mounting holes,
  connector, button and port positions, the screen window and the key
  interface are agreed with kyma: kyto keeps the detailed dimension sources,
  kyma keeps the PCB and electrical sources, and both are checked against the
  same revision before either is released. The thermal design that krepi's
  temperature thresholds depend on is documented with the enclosure.

## Mockups, prints and fit tests

The layout is verified with paper mockups and partial prints before any
enclosure is frozen; the three-dimensional fit is checked before the carrier
board is frozen; the enclosure is frozen only after fit tests with the actual
parts.

- **Mockup records** state the layout the mockup was made from, the nominal
  dimensions and their sources, and the dimensions measured on the mockup,
  kept apart from the nominal ones.
- **Print and fit-test records** state the model revision, the printed or
  bought parts used, the printer and material where a part is printed, the
  dimensions measured with their nominal values, and the result.

Dimension checks, interference checks, print checks and fit tests are
recorded separately; one never stands for another. A record never reports a
physical result that was not observed. Joint tests of the whole device with
the real system belong to krepi.

## Commit messages

Commit messages use English Conventional Commits with a header of at most 72
characters, for example `docs(scope): describe the fit tests`. The accepted
types are `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`,
`build`, `ci`, `chore` and `revert`. Headers that start with `Merge `,
`Revert `, `fixup! ` or `squash! ` are accepted as they are. The `commits`
step of the check applies the same rules to the commits of a branch. Do not
bypass the hook or change global Git configuration.

## Documentation

Documentation and comments are English and written for a person who builds,
checks or uses the device. A document states what a part does, its limits,
and commands that actually run. Nothing is described as existing or working
before it does.

Mathematics in Markdown is written in LaTeX. Inline formulas are written as
`$...$` on one line, without a backslash followed by ASCII punctuation and
without any of `* | < > & ~`; display formulas go in a fenced block with the
info string `math`, never inside a list item, and `$$` is not used. Commands,
program input and output, identifiers, file paths and option values keep code
formatting.

[Scope](docs/scope.md) describes the complete target scope of kyto, one row
per part, with its status. When work on a part produces models or records,
the same change links them from the part's row and updates its status.

Plans and their records belong to the project's coordination records, kept
outside this repository: milestone and task numbers, proposal and decision
numbers, capability and interface codes, stage codes, agent narratives, review
histories, evidence ledgers, supplier quotes, prices and lead times. No
document or comment here uses them. A document states the technical content:

- an item that does not exist yet is named by the part or file that will
  hold it and marked planned;
- a limit that waits for other work states the condition that lifts it;
- a decision is stated as the rule it made and where the rule applies.

## License

kyto uses two licenses, chosen by path. Every file under `tools/`, `xtask/`,
`.githooks/`, `.github/` and `.cargo/`, and `rust-toolchain.toml`, is
released under the [MIT License](LICENSE-MIT). Every other file, including
the models, exports, print models, mockup records and fit-test records, the
README and the other documentation, is released under the CERN Open Hardware
Licence Version 2 – Permissive
([LICENSE-CERN-OHL-P-2.0](LICENSE-CERN-OHL-P-2.0)).

The copyright holders are the casξ Authors, listed in [AUTHORS](AUTHORS).
Contributions are accepted under the license of the path they are made in.
