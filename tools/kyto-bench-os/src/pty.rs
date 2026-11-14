//! Pseudo-terminal pairs that stand in for a board's serial port in unit
//! tests.
//!
//! The test holds the master end, the code under test opens the slave by its
//! path, and a board script can play the board on a thread of its own over a
//! second master descriptor. Both ends stay open for the life of a [`Pty`]
//! until the test closes its master, so the slave keeps its attributes
//! between opens. `tcgetattr` and `tcsetattr` on the master end reach the
//! slave's attributes on Darwin and on Linux alike. Every descriptor here is
//! opened close-on-exec, so that no process that another test spawns
//! meanwhile inherits it.
//!
//! The module also gives what the tests of a second open of a held terminal
//! need on each host: an open of a terminal outside the code under test, the
//! text of a refused advisory lock, and whether the kernel refuses this
//! process a new open of a terminal held with `TIOCEXCL`.

use crate::{Failure, TEST_WAIT};
use std::ffi::{CStr, CString};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The flags both ends are opened with. `openpty` would open them without
/// close-on-exec, so the pair is opened end by end.
const FLAGS: libc::c_int = libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC;

// SAFETY: the signature is that of `ptsname_r` in Darwin's `<stdlib.h>`,
// which the libc crate declares only for other systems: it writes at most
// `length` bytes, a NUL-terminated name, to `buffer` and returns 0, or an
// error number.
#[cfg(any(target_os = "macos", target_os = "ios"))]
unsafe extern "C" {
    fn ptsname_r(fd: libc::c_int, buffer: *mut libc::c_char, length: libc::size_t) -> libc::c_int;
}
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
use libc::ptsname_r;

/// Takes ownership of the descriptor that opened the pty's `end`, which must
/// be close-on-exec from the start.
#[track_caller]
fn owned(fd: RawFd, end: &str) -> OwnedFd {
    assert!(
        fd >= 0,
        "cannot open the pty {end}: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: `fd` is an open descriptor that nothing else owns, so the
    // `OwnedFd` is its only owner and closes it once.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: fcntl with F_GETFD takes no argument and touches no memory of
    // this process; the descriptor is open.
    let status = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) };
    assert_eq!(
        status & libc::FD_CLOEXEC,
        libc::FD_CLOEXEC,
        "the pty {end} is not close-on-exec"
    );
    fd
}

/// The attributes of a terminal that `tcgetattr` reports: its flag words,
/// its speeds and its control characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attributes {
    pub iflag: libc::tcflag_t,
    pub oflag: libc::tcflag_t,
    pub cflag: libc::tcflag_t,
    pub lflag: libc::tcflag_t,
    pub ispeed: libc::speed_t,
    pub ospeed: libc::speed_t,
    pub cc: Vec<libc::cc_t>,
}

/// The attributes of the terminal behind `fd`.
#[track_caller]
fn termios(fd: RawFd) -> libc::termios {
    let mut termios = std::mem::MaybeUninit::<libc::termios>::zeroed();
    // SAFETY: `termios` is storage for one termios value on this frame,
    // properly aligned and exclusively borrowed for the call, which writes at
    // most one termios value into it.
    let status = unsafe { libc::tcgetattr(fd, termios.as_mut_ptr()) };
    assert_eq!(
        status,
        0,
        "cannot read the pty attributes: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: the storage was zero-initialized, which is a valid termios
    // value, and tcgetattr succeeded, so it holds the attributes.
    unsafe { termios.assume_init() }
}

/// `termios` as [`Attributes`].
fn attributes(termios: &libc::termios) -> Attributes {
    // SAFETY: `termios` is a reference to an initialized, properly aligned
    // termios value that the calls only read.
    let (ispeed, ospeed) = unsafe { (libc::cfgetispeed(termios), libc::cfgetospeed(termios)) };
    Attributes {
        iflag: termios.c_iflag,
        oflag: termios.c_oflag,
        cflag: termios.c_cflag,
        lflag: termios.c_lflag,
        ispeed,
        ospeed,
        cc: termios.c_cc.to_vec(),
    }
}

/// Appends at most `limit` bytes that arrive on `fd` within `wait`.
fn read_some(fd: RawFd, received: &mut Vec<u8>, limit: usize, wait: Duration) {
    let mut poll = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let millis = wait.as_millis().clamp(1, i32::MAX as u128) as libc::c_int;
    // SAFETY: `poll` is one initialized, properly aligned pollfd on this
    // frame, exclusively borrowed for the call, so the pointer and the count
    // 1 describe valid memory that the call may write.
    if unsafe { libc::poll(&mut poll, 1, millis) } <= 0 {
        return;
    }
    let mut buffer = vec![0u8; limit];
    // SAFETY: `buffer` is an initialized vector exclusively borrowed for the
    // call, so its pointer is valid for writes of its length and needs no
    // alignment; the call writes at most that many bytes.
    let count = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), limit) };
    if let Ok(count) = usize::try_from(count) {
        received.extend_from_slice(&buffer[..count]);
    }
}

/// Exactly `count` bytes from `fd`; a test fails when they have not arrived
/// within [`TEST_WAIT`].
#[track_caller]
fn read_exactly(fd: RawFd, count: usize) -> Vec<u8> {
    let deadline = Instant::now() + TEST_WAIT;
    let mut received = Vec::new();
    while received.len() < count {
        let now = Instant::now();
        assert!(
            now < deadline,
            "{} of {count} bytes within {TEST_WAIT:?}: {received:?}",
            received.len()
        );
        let missing = count - received.len();
        read_some(fd, &mut received, missing, deadline - now);
    }
    received
}

/// The board end of a pair: a master descriptor that a test or a board
/// script writes and reads.
#[derive(Debug)]
pub struct Master {
    fd: OwnedFd,
}

impl Master {
    /// Writes all of `bytes`, as the board sends them; the write waits while
    /// the slave's input queue is full.
    #[track_caller]
    pub fn write(&self, bytes: &[u8]) {
        let mut sent = 0;
        while sent < bytes.len() {
            let rest = &bytes[sent..];
            // SAFETY: `rest` is an initialized byte slice, so its pointer is
            // valid for reads of its length and needs no alignment; the call
            // only reads it while the borrow keeps it alive.
            let count =
                unsafe { libc::write(self.fd.as_raw_fd(), rest.as_ptr().cast(), rest.len()) };
            match usize::try_from(count) {
                Ok(count) => sent += count,
                Err(_) => {
                    let error = std::io::Error::last_os_error();
                    assert_eq!(
                        error.kind(),
                        std::io::ErrorKind::Interrupted,
                        "cannot write to the pty master: {error}"
                    );
                }
            }
        }
    }

    /// Exactly `count` bytes that the port sent, as the board reads them. A
    /// test fails when they have not arrived within [`TEST_WAIT`].
    #[track_caller]
    pub fn read(&self, count: usize) -> Vec<u8> {
        read_exactly(self.fd.as_raw_fd(), count)
    }

    /// The attributes that the code under test gave the slave, read on the
    /// master end, for a board script to check while the port is open.
    #[track_caller]
    pub fn attributes(&self) -> Attributes {
        attributes(&termios(self.fd.as_raw_fd()))
    }

    /// Waits until the code under test has given the port raw attributes.
    /// Until then the line discipline of the default attributes would echo
    /// what a board script writes and hold it as lines. A test fails when the
    /// port has not become raw within [`TEST_WAIT`].
    #[track_caller]
    pub fn await_raw(&self) {
        let deadline = Instant::now() + TEST_WAIT;
        while termios(self.fd.as_raw_fd()).c_lflag & libc::ICANON != 0 {
            assert!(
                Instant::now() < deadline,
                "the port did not become raw within {TEST_WAIT:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A second descriptor of the same end, for a board script.
    #[track_caller]
    fn duplicate(&self) -> Master {
        // SAFETY: fcntl with F_DUPFD_CLOEXEC takes an integer argument and
        // touches no memory of this process; the descriptor is open, and the
        // new one belongs to the returned value.
        let fd = unsafe { libc::fcntl(self.fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        Master {
            fd: owned(fd, "master duplicate"),
        }
    }
}

/// A board script running on a thread of its own.
pub struct Script<T> {
    handle: JoinHandle<T>,
}

impl<T> Script<T> {
    /// Waits for the script and returns its result; a script that panicked
    /// fails the test with its own message.
    #[track_caller]
    pub fn join(self) -> T {
        self.handle
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    }
}

/// A pseudo-terminal pair with the default attributes of the system.
#[derive(Debug)]
pub struct Pty {
    master: Option<Master>,
    slave: OwnedFd,
    path: String,
}

impl Pty {
    /// Opens a fresh pair.
    #[track_caller]
    pub fn open() -> Self {
        // SAFETY: posix_openpt takes only its flags and touches no memory of
        // this process.
        let master = owned(unsafe { libc::posix_openpt(FLAGS) }, "master");
        // SAFETY: grantpt and unlockpt take a descriptor and touch no memory
        // of this process; the descriptor is an open pty master.
        let unlocked = unsafe {
            libc::grantpt(master.as_raw_fd()) == 0 && libc::unlockpt(master.as_raw_fd()) == 0
        };
        assert!(
            unlocked,
            "cannot unlock the pty slave: {}",
            std::io::Error::last_os_error()
        );
        let mut name: [libc::c_char; 128] = [0; 128];
        // SAFETY: `name` is an initialized array on this frame, exclusively
        // borrowed for the call, and its length is passed, so the call writes
        // within it; `c_char` needs no alignment beyond its own.
        let status = unsafe { ptsname_r(master.as_raw_fd(), name.as_mut_ptr(), name.len()) };
        assert_eq!(status, 0, "cannot name the pty slave");
        // SAFETY: ptsname_r succeeded, so `name` holds a NUL-terminated
        // string within its length, which the borrow keeps alive and
        // unchanged while the `CStr` is used.
        let name = unsafe { CStr::from_ptr(name.as_ptr()) };
        // SAFETY: `name` is a NUL-terminated string that lives until the
        // call returns, and the call only reads it; no mode argument is
        // needed without `O_CREAT`.
        let slave = owned(unsafe { libc::open(name.as_ptr(), FLAGS) }, "slave");
        let path = name
            .to_str()
            .expect("the pty slave name is UTF-8")
            .to_string();
        Pty {
            master: Some(Master { fd: master }),
            slave,
            path,
        }
    }

    /// The path of the slave, which the code under test opens.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The test's master descriptor.
    #[track_caller]
    fn master(&self) -> &Master {
        self.master.as_ref().expect("the pty master is closed")
    }

    /// Writes all of `bytes` to the master, as the board sends them.
    #[track_caller]
    pub fn write(&self, bytes: &[u8]) {
        self.master().write(bytes);
    }

    /// Exactly `count` bytes that the port sent, read from the master. A
    /// test fails when they have not arrived within [`TEST_WAIT`].
    #[track_caller]
    pub fn read(&self, count: usize) -> Vec<u8> {
        read_exactly(self.master().fd.as_raw_fd(), count)
    }

    /// Whatever the port sends within `wait`, read from the master: a short
    /// window, for a test that requires that nothing arrives.
    #[track_caller]
    pub fn received(&self, wait: Duration) -> Vec<u8> {
        let deadline = Instant::now() + wait;
        let mut received = Vec::new();
        loop {
            let now = Instant::now();
            if now >= deadline {
                return received;
            }
            read_some(
                self.master().fd.as_raw_fd(),
                &mut received,
                4096,
                deadline - now,
            );
        }
    }

    /// The current attributes of the slave.
    #[track_caller]
    pub fn attributes(&self) -> Attributes {
        attributes(&self.termios())
    }

    /// The current attributes of the slave as a `termios` value, from which
    /// a test derives others.
    #[track_caller]
    pub fn termios(&self) -> libc::termios {
        termios(self.slave.as_raw_fd())
    }

    /// Gives the slave `termios` at once.
    #[track_caller]
    pub fn set_termios(&self, termios: &libc::termios) {
        // SAFETY: `termios` is a reference to an initialized, properly
        // aligned termios value; the call only reads it while the borrow
        // keeps it alive.
        let status = unsafe { libc::tcsetattr(self.slave.as_raw_fd(), libc::TCSANOW, termios) };
        assert_eq!(
            status,
            0,
            "cannot set the pty attributes: {}",
            std::io::Error::last_os_error()
        );
    }

    /// Exactly `count` bytes read from the slave: input that the code under
    /// test left queued. A test fails when they have not arrived within
    /// [`TEST_WAIT`].
    #[track_caller]
    pub fn read_slave(&self, count: usize) -> Vec<u8> {
        read_exactly(self.slave.as_raw_fd(), count)
    }

    /// Closes the test's master descriptor. The device hangs up once no board
    /// script holds a master descriptor either.
    pub fn close_master(&mut self) {
        self.master = None;
    }

    /// Waits until the device has hung up, which the slave reports to `poll`.
    /// A process that another test starts holds a copy of every descriptor of
    /// this process until its exec closes the copies, so the device can hang
    /// up a moment after the last master descriptor of the test was closed.
    /// Linux reports a hang-up as soon as the master has closed, and an error
    /// condition as well once every open file of the slave is hung up, after
    /// which a write to the slave fails with `EIO` and a read finds the end
    /// of the stream; the wait lasts until both are reported there. A test
    /// fails when the device has not hung up within [`TEST_WAIT`].
    #[track_caller]
    pub fn await_hang_up(&self) {
        let hung_up = if cfg!(target_os = "linux") {
            libc::POLLHUP | libc::POLLERR
        } else {
            libc::POLLHUP
        };
        let deadline = Instant::now() + TEST_WAIT;
        loop {
            let mut poll = libc::pollfd {
                fd: self.slave.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: `poll` is one initialized, properly aligned pollfd on
            // this frame, exclusively borrowed for the call, so the pointer
            // and the count 1 describe valid memory that the call may write.
            let ready = unsafe { libc::poll(&mut poll, 1, 10) };
            if ready > 0 && poll.revents & hung_up == hung_up {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the device did not hang up within {TEST_WAIT:?}"
            );
            if ready > 0 {
                // Input that the code under test has not read yet.
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    /// Runs `script` on a thread of its own over a second master descriptor,
    /// as a board that sends and reads.
    #[track_caller]
    pub fn board<T, F>(&self, script: F) -> Script<T>
    where
        F: FnOnce(&Master) -> T + Send + 'static,
        T: Send + 'static,
    {
        let master = self.master().duplicate();
        Script {
            handle: std::thread::spawn(move || script(&master)),
        }
    }
}

/// Opens `path` read-only, as a receiver opens it, outside the code under
/// test.
pub fn open_read_only(path: &str) -> std::io::Result<OwnedFd> {
    let name = CString::new(path).unwrap();
    let flags = libc::O_RDONLY | libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC;
    // SAFETY: `name` is a NUL-terminated string that lives until the call
    // returns, and the call only reads it; no mode argument is needed
    // without `O_CREAT`.
    let fd = unsafe { libc::open(name.as_ptr(), flags) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `fd` is an open descriptor that nothing else owns, so the
    // `OwnedFd` is its only owner and closes it once.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// The text of a refused advisory lock: `EWOULDBLOCK`, 35 on Darwin and 11
/// on Linux, whose text is the same on both.
pub fn lock_refused() -> Failure {
    Failure::new(format!(
        "Resource temporarily unavailable (os error {})",
        libc::EWOULDBLOCK
    ))
}

/// Whether the kernel refuses this process a new open of a terminal that a
/// descriptor holds with `TIOCEXCL`, found on a pair of its own, outside the
/// code under test. Linux refuses it with `EBUSY` unless the process has
/// `CAP_SYS_ADMIN` in the initial user namespace, which root in a container
/// may lack and a process in a user namespace of its own does not have;
/// Darwin lets the open through, so there the advisory lock of a port
/// refuses a second port.
#[track_caller]
pub fn exclusive_open_refused() -> bool {
    let pty = Pty::open();
    let held = open_read_only(pty.path()).expect("open the tty");
    // SAFETY: TIOCEXCL takes no argument, so the call passes no pointer and
    // touches no memory of this process; the descriptor is an open terminal.
    let taken = unsafe { libc::ioctl(held.as_raw_fd(), libc::TIOCEXCL as libc::c_ulong) };
    assert_eq!(taken, 0, "TIOCEXCL: {}", std::io::Error::last_os_error());
    match open_read_only(pty.path()) {
        Ok(_) => false,
        Err(error) if error.raw_os_error() == Some(libc::EBUSY) => true,
        Err(error) => panic!("a new open of a held tty: {error}"),
    }
}
