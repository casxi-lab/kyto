//! The operating-system edge of the kyto bench tool: the serial ports, the
//! interrupt signals and the system clock of a bench of real devices. It is
//! the one member of the workspace that may use unsafe code, read the
//! system clock and call the C library.
//!
//! The serial transport, [`tty`], with [`timing`] and [`report`], is copied
//! from krepi byte for byte, and so are the modules that its tests use and
//! the lines of this file between the markers that name their source;
//! `PORTING.md` at the root of the repository records each of them. kyto's
//! own modules are [`clock`], the clock of a bench of real devices,
//! [`command_port`], the port of a relay module's commands, and
//! [`console`], a serial console that stamps what it receives when it
//! arrives.
//!
//! Unsafe code is forbidden in every module but [`tty`], [`command_port`]
//! and the pseudo-terminals and child processes of the tests. The bans of
//! `tools/clippy.toml` on sleeping, on reading the system clock and on
//! running a program are lifted only by the attributes on the declarations
//! of this file, which a test of the workspace pins.

#[cfg(not(unix))]
compile_error!("kyto-bench-os runs on Unix hosts only");

#[forbid(unsafe_code)]
#[allow(
    clippy::disallowed_methods,
    reason = "the bench clock reads the monotonic clock of the system and sleeps"
)]
pub mod clock;

pub mod command_port;

#[forbid(unsafe_code)]
#[allow(
    clippy::disallowed_methods,
    reason = "a receive waits for a chunk at most until the deadline of the bench clock"
)]
pub mod console;

#[forbid(unsafe_code)]
#[allow(
    clippy::disallowed_methods,
    reason = "the copied writer notes the time of day of each request, which kyto discards"
)]
#[rustfmt::skip]
pub mod report;

#[forbid(unsafe_code)]
#[rustfmt::skip]
pub mod timing;

#[allow(
    clippy::disallowed_methods,
    reason = "the copied writer bounds a request by the system clock, and its tests wait"
)]
#[rustfmt::skip]
pub mod tty;

// krepi@1922f76a02ea3c623594fbdf722334b84d84fd72:tools/src/lib.rs:46-73
/// A check that did not hold, with the message that describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure(pub String);

impl Failure {
    pub fn new(message: impl Into<String>) -> Self {
        Failure(message.into())
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Failure {}

/// The lowercase hexadecimal digits of `bytes`, two for each byte.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        Failure(error.to_string())
    }
}
// End of the lines from krepi.

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    dead_code,
    reason = "the copied tests run their own binary again and wait, and some helpers have no caller in kyto"
)]
#[rustfmt::skip]
mod child;

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    dead_code,
    reason = "the copied pseudo-terminals wait for the code under test, and some helpers have no caller in kyto"
)]
#[rustfmt::skip]
mod pty;

#[cfg(test)]
#[forbid(unsafe_code)]
#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    dead_code,
    reason = "the copied scratch directories run and wait for scripts, which kyto's tests do not use"
)]
#[rustfmt::skip]
mod scratch;

// krepi@1922f76a02ea3c623594fbdf722334b84d84fd72:tools/src/lib.rs:40-44
/// How long a test waits for something that it expects to happen, such as
/// bytes on a pseudo-terminal or the end of a child process. It elapses only
/// when the test fails, so it is generous: a slow host never reaches it.
#[cfg(test)]
const TEST_WAIT: std::time::Duration = std::time::Duration::from_secs(60);
// End of the lines from krepi.

// krepi@1922f76a02ea3c623594fbdf722334b84d84fd72:tools/src/lib.rs:75-97
#[cfg(test)]
mod tests {
    use super::Failure;

    #[test]
    fn a_failure_displays_its_message() {
        let failure = Failure::new("missing GNU build-id note");
        assert_eq!(failure.to_string(), "missing GNU build-id note");
        assert_eq!(failure, Failure("missing GNU build-id note".to_string()));
    }

    #[test]
    fn an_io_error_becomes_a_failure_with_its_text() {
        let error = std::io::Error::new(std::io::ErrorKind::NotFound, "no such image");
        assert_eq!(Failure::from(error), Failure::new("no such image"));
        // An operating-system error keeps the text that the standard library
        // gives it, with its error number.
        let os = std::io::Error::from_raw_os_error(2);
        let text = os.to_string();
        assert!(text.ends_with("(os error 2)"), "{text}");
        assert_eq!(Failure::from(os).0, text);
    }
}
// End of the lines from krepi.
