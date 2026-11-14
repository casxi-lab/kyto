//! The port of a relay module's commands: a terminal opened for one command
//! at 9600 8N1 and closed again. Its steps are `open` (of a terminal),
//! `lock` (`flock`, as the copied transport takes it), `exclusive access`,
//! `attributes`, `flush`, a wait, `write`, `drain`, after which the time is
//! taken, and a hold; then the port is restored, released, unlocked and
//! closed, each step also after a failed one. Nothing is read. An error is
//! `<step>: <text>`, the operating system's text of the error or a fixed
//! phrase, and never names the port.

use std::ffi::CString;
use std::fmt;
use std::io;
use std::os::fd::RawFd;
use std::time::Duration;

use crate::clock::StampClock;
use crate::tty::{os_error, raw_attributes};

/// The waits of one command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandTiming {
    /// The wait on the clock after the flush, before the frame is written.
    pub settle: Duration,
    /// The wait on the clock after the frame left the port.
    pub hold: Duration,
    /// The longest time the port may take to accept the frame for writing.
    pub write: Duration,
}

/// A step of a port that failed, with the text of the failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortError {
    pub step: &'static str,
    pub text: String,
}

impl PortError {
    /// A failure of `step` with `text`.
    pub fn new(step: &'static str, text: impl Into<String>) -> Self {
        Self {
            step,
            text: text.into(),
        }
    }

    /// A failure of `step` with the last operating-system error.
    fn last(step: &'static str) -> Self {
        Self::new(step, os_error(last_errno()))
    }
}

impl fmt::Display for PortError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.step, self.text)
    }
}

/// Read and write, not the controlling terminal, no wait, closed on exec.
const FLAGS: libc::c_int = libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC;

fn last_errno() -> i32 {
    io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// The raw 8N1 attributes of the copied transport, derived from `original`,
/// at 9600 baud.
pub fn command_attributes(original: &libc::termios) -> libc::termios {
    let mut configured = raw_attributes(original);
    // SAFETY: `configured` is an initialized, properly aligned termios value
    // owned by this frame, borrowed mutably for each call alone; the calls
    // only store the speed into its fields.
    unsafe {
        libc::cfsetispeed(&mut configured, libc::B9600);
        libc::cfsetospeed(&mut configured, libc::B9600);
    }
    configured
}

/// Sends `frame` on the terminal at `path`, waiting on `clock` as `timing`
/// says, and returns the time at which the frame had left the port, even
/// when restoring the port fails then: the next command sets the attributes
/// again, and closing the port releases it.
pub fn send_frame<C: StampClock>(
    path: &str,
    frame: &[u8],
    timing: &CommandTiming,
    clock: &C,
) -> Result<u64, PortError> {
    let name = CString::new(path).map_err(|_| PortError::new("open", "a NUL byte in the name"))?;
    // SAFETY: `name` is a NUL-terminated string that lives until the call
    // returns, and the call only reads it; no mode argument is needed
    // without `O_CREAT`.
    let fd = unsafe { libc::open(name.as_ptr(), FLAGS) };
    if fd < 0 {
        return Err(PortError::last("open"));
    }
    let mut port = Port {
        fd,
        locked: false,
        exclusive: false,
        original: None,
    };
    port.configure()?;
    let settle = timing.settle.as_nanos() as u64;
    clock.wait_until_ns(clock.now_ns().saturating_add(settle));
    port.write(frame, timing.write)?;
    // SAFETY: tcdrain takes a descriptor and touches no memory of this
    // process. A signal ends it early with `EINTR`, and it is repeated.
    while unsafe { libc::tcdrain(port.fd) } != 0 {
        if last_errno() != libc::EINTR {
            return Err(PortError::last("drain"));
        }
    }
    let sent = clock.now_ns();
    clock.wait_until_ns(sent.saturating_add(timing.hold.as_nanos() as u64));
    Ok(sent)
}

/// A port open for one command, restored and closed when it is dropped.
struct Port {
    fd: RawFd,
    locked: bool,
    exclusive: bool,
    original: Option<libc::termios>,
}

impl Port {
    /// Takes the port and sets its attributes: the steps before the wait.
    fn configure(&mut self) -> Result<(), PortError> {
        // SAFETY: isatty takes a descriptor and touches no memory of this
        // process; `fd` is open and owned by this port.
        if unsafe { libc::isatty(self.fd) } != 1 {
            return Err(PortError::new("open", "not a terminal"));
        }
        // SAFETY: flock takes a descriptor and an operation and touches no
        // memory of this process; `fd` is open and owned by this port.
        if unsafe { libc::flock(self.fd, libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(PortError::last("lock"));
        }
        self.locked = true;
        // SAFETY: TIOCEXCL takes no argument, so the call passes no pointer
        // and touches no memory of this process.
        if unsafe { libc::ioctl(self.fd, libc::TIOCEXCL as libc::c_ulong) } != 0 {
            return Err(PortError::last("exclusive access"));
        }
        self.exclusive = true;
        let mut original = std::mem::MaybeUninit::<libc::termios>::zeroed();
        // SAFETY: `original` is storage for one termios value on this frame,
        // properly aligned and exclusively borrowed for the call, which
        // writes at most one termios value into it.
        if unsafe { libc::tcgetattr(self.fd, original.as_mut_ptr()) } != 0 {
            return Err(PortError::last("attributes"));
        }
        // SAFETY: the storage was zero-initialized, which is a valid termios
        // value, and tcgetattr succeeded, so it holds the attributes.
        let original = unsafe { original.assume_init() };
        self.original = Some(original);
        let configured = command_attributes(&original);
        // SAFETY: `configured` is an initialized, properly aligned termios
        // value that the call only reads while the borrow keeps it alive.
        if unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &configured) } != 0 {
            return Err(PortError::last("attributes"));
        }
        // SAFETY: tcflush takes a descriptor and a queue selector and
        // touches no memory of this process.
        if unsafe { libc::tcflush(self.fd, libc::TCIOFLUSH) } != 0 {
            return Err(PortError::last("flush"));
        }
        Ok(())
    }

    /// Writes all of `frame`, waiting at most `bound` each time for the
    /// port to accept bytes.
    fn write(&self, frame: &[u8], bound: Duration) -> Result<(), PortError> {
        let millis = libc::c_int::try_from(bound.as_millis()).unwrap_or(libc::c_int::MAX);
        let mut sent = 0;
        while sent < frame.len() {
            let mut poll = libc::pollfd {
                fd: self.fd,
                events: libc::POLLOUT,
                revents: 0,
            };
            // SAFETY: `poll` is one initialized, properly aligned pollfd on
            // this frame, exclusively borrowed for the call, so the pointer
            // and the count 1 describe valid memory that the call may write.
            let ready = unsafe { libc::poll(&mut poll, 1, millis) };
            if ready == 0 {
                return Err(PortError::new("write", "the port accepted no byte in time"));
            }
            let rest = &frame[sent..];
            let count = if ready < 0 {
                -1
            } else {
                // SAFETY: `rest` is an initialized byte slice, so its pointer
                // is valid for reads of its length and needs no alignment;
                // the call only reads it while the borrow keeps it alive.
                unsafe { libc::write(self.fd, rest.as_ptr().cast(), rest.len()) }
            };
            match usize::try_from(count) {
                Ok(0) => return Err(PortError::new("write", "the port took no byte")),
                Ok(count) => sent += count,
                Err(_) if matches!(last_errno(), libc::EINTR | libc::EAGAIN) => {}
                Err(_) => return Err(PortError::last("write")),
            }
        }
        Ok(())
    }
}

impl Drop for Port {
    fn drop(&mut self) {
        let original = self.original.as_ref();
        #[cfg(test)]
        let original = original.filter(|_| !tests::RESTORE_FAILS.get());
        if let Some(original) = original {
            // SAFETY: `original` is an initialized, properly aligned termios
            // value that tcgetattr filled in; the call only reads it while
            // the borrow keeps it alive.
            unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, original) };
        }
        if self.exclusive {
            // SAFETY: TIOCNXCL takes no argument, so the call passes no
            // pointer and touches no memory of this process.
            unsafe { libc::ioctl(self.fd, libc::TIOCNXCL as libc::c_ulong) };
        }
        if self.locked {
            // SAFETY: flock takes a descriptor and an operation and touches
            // no memory of this process; `fd` is open and owned by this port.
            unsafe { libc::flock(self.fd, libc::LOCK_UN) };
        }
        // SAFETY: close takes a descriptor and touches no memory of this
        // process; `fd` is owned by this port and closed exactly once, here.
        unsafe { libc::close(self.fd) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TEST_WAIT;
    use crate::clock::stepped::TestClock;
    use crate::console::tests::opened;
    use crate::pty::{Attributes, Pty, exclusive_open_refused, lock_refused};
    use crate::scratch::Scratch;
    use std::cell::Cell;

    thread_local! {
        /// Whether a command on this thread leaves its port's attributes
        /// unrestored, as a failed restoration does.
        pub(super) static RESTORE_FAILS: Cell<bool> = const { Cell::new(false) };
    }

    const ON: [u8; 4] = [0xA0, 0x01, 0x01, 0xA2];
    const OFF: [u8; 4] = [0xA0, 0x01, 0x00, 0xA1];
    const SETTLE: u64 = 250_000_000;
    const HOLD: u64 = 150_000_000;
    const SECOND: u64 = 1_000_000_000;
    const TIMING: CommandTiming = CommandTiming {
        settle: Duration::from_millis(250),
        hold: Duration::from_millis(150),
        write: TEST_WAIT,
    };

    /// Sends `frame` on `pty` on another thread and moves `clock` through
    /// the command's waits as they come: the result, the attributes of the
    /// port during the wait after the flush, and the bytes that the board
    /// read.
    fn command(
        pty: &Pty,
        clock: TestClock,
        frame: [u8; 4],
        restore_fails: bool,
    ) -> (Result<u64, PortError>, Attributes, Vec<u8>) {
        let start = clock.now_ns();
        std::thread::scope(|scope| {
            let command = scope.spawn(|| {
                RESTORE_FAILS.set(restore_fails);
                send_frame(pty.path(), &frame, &TIMING, &clock)
            });
            clock.await_wait(start + SETTLE);
            let during = pty.attributes();
            clock.advance_to(start + SETTLE);
            let received = pty.read(frame.len());
            clock.await_wait(start + SETTLE + HOLD);
            clock.advance_to(start + SETTLE + HOLD);
            (command.join().unwrap(), during, received)
        })
    }

    /// The attributes of `pty` but `PENDIN`, which Darwin sets whenever
    /// attributes turn canonical input back on.
    fn restorable(pty: &Pty) -> Attributes {
        let mut attributes = pty.attributes();
        attributes.lflag &= !libc::PENDIN;
        attributes
    }

    #[test]
    fn a_command_reaches_the_port_at_9600_8n1_and_the_port_is_restored() {
        let pty = Pty::open();
        let before = restorable(&pty);
        let clock = TestClock::at_zero();
        for (frame, start) in [(ON, 0), (OFF, SECOND)] {
            clock.advance_to(start);
            let (sent, during, received) = command(&pty, clock, frame, false);
            assert_eq!(sent, Ok(start + SETTLE), "stamped when the frame left");
            assert_eq!(received, frame);
            assert_eq!((during.ispeed, during.ospeed), (libc::B9600, libc::B9600));
            let parity = libc::PARENB | libc::CSTOPB | libc::CRTSCTS;
            assert_eq!(during.cflag & (libc::CSIZE | parity), libc::CS8);
            let receiver = libc::CREAD | libc::CLOCAL;
            assert_eq!(during.cflag & receiver, receiver);
            assert_eq!((during.iflag, during.oflag, during.lflag), (0, 0, 0));
            assert_eq!(restorable(&pty), before, "the port is restored");
        }
        // A restoration that fails after the frame left the port still
        // gives the time of the command, and the next command works.
        clock.advance_to(2 * SECOND);
        let (sent, ..) = command(&pty, clock, ON, true);
        assert_eq!(sent, Ok(2 * SECOND + SETTLE));
        assert_ne!(restorable(&pty), before, "not restored");
        clock.advance_to(3 * SECOND);
        let (sent, _, received) = command(&pty, clock, OFF, false);
        assert_eq!((sent, received), (Ok(3 * SECOND + SETTLE), OFF.to_vec()));
    }

    #[test]
    fn a_port_held_by_a_console_is_refused_and_nothing_is_written() {
        let pty = Pty::open();
        let clock = TestClock::at_zero();
        let mut console = opened(&pty, clock);
        let held = pty.attributes();
        let refused = send_frame(pty.path(), &ON, &TIMING, &clock);
        let expected = if exclusive_open_refused() {
            PortError::new("open", "Device or resource busy (os error 16)")
        } else {
            PortError::new("lock", lock_refused().0)
        };
        assert_eq!(refused, Err(expected));
        assert_eq!(pty.attributes(), held, "the console's port is unchanged");
        // The board reads the console's byte first: no frame came before.
        console.send(b"!").unwrap();
        assert_eq!(pty.read(1), b"!");
        console.close().unwrap();
    }

    #[test]
    fn a_missing_port_names_no_path() {
        let scratch = Scratch::new("command-missing");
        let path = scratch.path("cu.usbserial-MISSING");
        let clock = TestClock::at_zero();
        let error = send_frame(path.to_str().unwrap(), &ON, &TIMING, &clock).unwrap_err();
        let missing = PortError::new("open", "No such file or directory (os error 2)");
        assert_eq!(error, missing);
        assert_eq!(
            error.to_string(),
            "open: No such file or directory (os error 2)"
        );
        let error = send_frame("/dev/cu.x\0y", &ON, &TIMING, &clock).unwrap_err();
        assert_eq!(error.to_string(), "open: a NUL byte in the name");
    }

    #[test]
    fn a_regular_file_is_not_a_terminal() {
        let scratch = Scratch::new("command-file");
        let path = scratch.write("plain", b"");
        let clock = TestClock::at_zero();
        let error = send_frame(path.to_str().unwrap(), &ON, &TIMING, &clock).unwrap_err();
        assert_eq!(error, PortError::new("open", "not a terminal"));
        assert_eq!(scratch.read("plain"), b"", "nothing written");
    }
}
