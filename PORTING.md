# Porting record

Part of the bench tool is copied from [krepi](https://github.com/casxi-lab/krepi),
the operating system of casξ, which is released under the MIT License by
the casξ Authors as the bench tool is: the serial port transport with which
`tools/kyto-bench-os` drives the serial ports of a bench, the modules that
its tests use, and lines of krepi's `lib.rs` that both need. This record
lists every file and every span of lines that is copied, and the change that
copies one also records it here. A source is written
`krepi@<full commit SHA>:<path>`. kyto takes no dependency on krepi.

A copied file is the krepi file byte for byte. A copied span is the lines of
a krepi file byte for byte, between a line
`// krepi@<full commit SHA>:<path>:<first line>-<last line>` and a line
`// End of the lines from krepi.` in the file that holds it. Nothing copied is
patched, reformatted or renamed: the declarations of the copied modules tell
`rustfmt` to skip them, and kyto's own modules beside them, `clock.rs`,
`command_port.rs` and `console.rs` in `tools/kyto-bench-os/src/`, add what
kyto needs. The spans in `lib.rs` are not skipped: `rustfmt` leaves them as
krepi formatted them, and `copied_files_have_the_digests_of_their_record`
fails if it ever changes one. A later change to a copied file or span copies
it again from another commit of krepi and updates its row.

- **Module**: the file in this repository, or the span of lines in it.
- **Source**: the krepi file that it is copied from.
- **Lines**: the lines of the source that are copied.
- **Blob**: the Git object of the source file at that commit.
- **Last changed in**: the last commit of krepi that changed the source file,
  up to that commit.
- **SHA-256**: the digest of the copied bytes; for a span, of its lines, each
  with its line feed.
- **Change**: copied byte for byte, for every row, with what the copy holds.
- **Tests not carried over**: the tests of the source that are not copied.

`copied_files_have_the_digests_of_their_record`, a test of
`tools/kyto-bench/tests/manifest.rs`, computes the SHA-256 of every row and
fails when one differs from this record; `git hash-object` gives the blob of
a copied file.

Copied code is tested again in this repository. krepi's 25 tests of the
copied code, 20 in `tty/tests.rs`, 3 in `report.rs` and 2 in the copied
lines of `lib.rs`, run with the other tests of the `tools` workspace. Tests
and results of krepi are not evidence for kyto.

## Files

| Module | Source | Lines | Blob | Last changed in | SHA-256 | Change | Tests not carried over |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `tools/kyto-bench-os/src/tty.rs` | `krepi@1922f76a02ea3c623594fbdf722334b84d84fd72:tools/src/tty.rs` | 1–893 | `d2ce52d324dff3a29347b650eefcc999a32fa43a` | `bbe9525e158479100ecf7a932d16f94147c4a38f` | `3314cebce6a21804963dd6073b72b7fb3e958b2de265d0e55d513fc1ecaf461a` | Copied byte for byte. The serial port transport: a terminal held exclusively at raw 115200 8N1, read by a thread of its own and written with bounded requests, a terminal opened read-only, and the handlers of SIGINT, SIGTERM and SIGHUP | None |
| `tools/kyto-bench-os/src/tty/tests.rs` | `krepi@1922f76a02ea3c623594fbdf722334b84d84fd72:tools/src/tty/tests.rs` | 1–628 | `870249586b3713cd22414bddab98fa776bf99b9d` | `e4292e20320e047faaca3dd3e776b08b6f8a0da7` | `aeb562ec9de9711626dc13e887858d95e68479926ce4588e1b64f49c99ba4662` | Copied byte for byte. The 20 tests of the transport, two of which catch signals in child processes of the test binary | None |
| `tools/kyto-bench-os/src/timing.rs` | `krepi@1922f76a02ea3c623594fbdf722334b84d84fd72:tools/src/timing.rs` | 1–99 | `cb909cb8115d07ad5cbeffb4ea91b79564e8748f` | `e4292e20320e047faaca3dd3e776b08b6f8a0da7` | `11dfaaa3f75ee098f4cece56683c39eaa3a78a5d2c23607761cd8326643a8044` | Copied byte for byte. The time values of krepi's host tools, of which a port of the transport reads two: the wait of its reader thread, 100 ms, and the transmission time of a request, 2 s | None |
| `tools/kyto-bench-os/src/report.rs` | `krepi@1922f76a02ea3c623594fbdf722334b84d84fd72:tools/src/report.rs` | 1–135 | `9ffb63790b2217fe6249f224373e2120d40f104e` | `0cf20ea2af456cb56ca83e5fff2e48d576a8a459` | `a1e6c32deb98e6527456ef02de030050ebf13fd3a36ddfbf26fb08357414f0e2` | Copied byte for byte. JSON values and UTC times, with which the writer of the transport notes each request; kyto discards these notes | None |
| `tools/kyto-bench-os/src/pty.rs` | `krepi@1922f76a02ea3c623594fbdf722334b84d84fd72:tools/src/pty.rs` | 1–484 | `f620fee0569cd700a7862517698d2c3bd7104961` | `e4292e20320e047faaca3dd3e776b08b6f8a0da7` | `3bb94baca8c672a5974df694fb4184980427f53616bda9f13c65a03ef31e13f8` | Copied byte for byte. Pseudo-terminal pairs that stand in for a serial port in tests, which kyto's tests of the command port and the console use too | None |
| `tools/kyto-bench-os/src/child.rs` | `krepi@1922f76a02ea3c623594fbdf722334b84d84fd72:tools/src/child.rs` | 1–250 | `8e545e129faee79ae403193a06a60744d824dcba` | `bbe9525e158479100ecf7a932d16f94147c4a38f` | `fca2f9aa62437e4bc7b55fc76d62503162320e8be0832a8db22e42d765a8aeda` | Copied byte for byte. Runs one test alone in a child process, for the tests that catch signals | None |
| `tools/kyto-bench-os/src/scratch.rs` | `krepi@1922f76a02ea3c623594fbdf722334b84d84fd72:tools/src/scratch.rs` | 1–115 | `b9daab22e03b4f63613f3e7273d2a5634ebf16fa` | `e4292e20320e047faaca3dd3e776b08b6f8a0da7` | `1570c77dd92f48e7cf7a8af515cde3ecd7f4cf54eb50bbf94f1d8651d4f1a065` | Copied byte for byte. A directory for the files of one test | None |
| `tools/kyto-bench-os/src/lib.rs`, a span | `krepi@1922f76a02ea3c623594fbdf722334b84d84fd72:tools/src/lib.rs` | 40–44 | `2d68be52fe70955eef3bb86f6e4ef042a89f54fa` | `03bb5231e65b378bc2d93d419bf863a8559bb918` | `6a3aed7b294fc8cfc37afb7c07b290bc6be6bdfc90717a419edda06997accc79` | Copied byte for byte. `TEST_WAIT`, the bound of a test's wait for what it expects | None |
| `tools/kyto-bench-os/src/lib.rs`, a span | `krepi@1922f76a02ea3c623594fbdf722334b84d84fd72:tools/src/lib.rs` | 46–73 | `2d68be52fe70955eef3bb86f6e4ef042a89f54fa` | `03bb5231e65b378bc2d93d419bf863a8559bb918` | `5f9b866ecf9eb83c2f5085879fce6d18735781d44dae93683bfa4e9e5ee82e7c` | Copied byte for byte. `Failure`, the error of the transport, `hex` and the conversion of an I/O error | None |
| `tools/kyto-bench-os/src/lib.rs`, a span | `krepi@1922f76a02ea3c623594fbdf722334b84d84fd72:tools/src/lib.rs` | 75–97 | `2d68be52fe70955eef3bb86f6e4ef042a89f54fa` | `03bb5231e65b378bc2d93d419bf863a8559bb918` | `33cdd67dbada69315b16dc557c8bb1d2b02f4ae62cf9b704cfaa6892e19ca18c` | Copied byte for byte. The 2 tests of `Failure` | None |
