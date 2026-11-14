//! The serial port transport of the host tools.
//!
//! A [`TtyPort`] is a POSIX terminal opened for a session with a board: held
//! exclusively, with raw 115200 8N1 attributes, read by a polling thread into
//! a byte stream, written with bounded requests by a [`TtyWriter`], and
//! restored and closed at the end. A [`Receiver`] is a terminal opened
//! read-only for a capture that only receives. [`catch_interrupts`] installs
//! the handlers that let SIGINT, SIGTERM and SIGHUP end a session or a
//! capture through a flag instead of ending the process.
//!
//! Errors carry the standard library's text of the operating-system error,
//! `strerror (os error N)`. An error that concerns a file name starts with
//! the name in Rust's debug form, and the steps that read, set and restore
//! terminal attributes or release exclusive access name themselves before
//! the error.

use crate::report::{Value, utc_now};
use crate::timing::Timing;
use crate::{Failure, hex};
use serde_json::json;
use std::ffi::CString;
use std::io::{self, Write};
use std::os::fd::RawFd;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The bytes one read of a port's reader thread takes at most.
const READ_BUFFER: usize = 4096;
/// The bytes of the largest request a [`TtyWriter`] sends.
const REQUEST_LIMIT: usize = 128;

/// The label of a failed read of a terminal's original attributes.
const ATTRIBUTE_READ: &str = "attribute read failed";
/// The label of raw attributes that a terminal refused.
const RAW_REFUSED: &str = "raw attributes refused";
/// The label of a failed restoration of a terminal's original attributes.
const RESTORATION: &str = "attribute restoration failed";
/// The label of a failed release of exclusive access to a terminal.
const RELEASE: &str = "exclusive access release failed";

fn require(condition: bool, message: &str) -> Result<(), Failure> {
    if condition {
        Ok(())
    } else {
        Err(Failure::new(message))
    }
}

fn last_errno() -> i32 {
    io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// The standard library's text of the operating-system error `errno`:
/// `strerror (os error N)`.
pub(crate) fn os_error(errno: i32) -> String {
    io::Error::from_raw_os_error(errno).to_string()
}

/// The last operating-system error as a failure with its text.
fn last_error() -> Failure {
    Failure::new(os_error(last_errno()))
}

/// The text of the operating-system error `errno` of the step `label`.
fn labelled(label: &str, errno: i32) -> String {
    format!("{label}: {}", os_error(errno))
}

/// The C form of the port name `path`; a name with a NUL byte has none and
/// is refused with the name and the reason.
fn c_name(path: &str) -> Result<CString, Failure> {
    CString::new(path).map_err(|error| Failure::new(format!("{path:?}: {error}")))
}

/// A refused open of `path`, with the name before the error text.
fn open_error(path: &str) -> Failure {
    Failure::new(format!("{path:?}: {}", os_error(last_errno())))
}

/// `timeout` in whole milliseconds for `poll(2)`, rounded up, so that a wait
/// never ends before `timeout` has passed, and at most `c_int::MAX`.
fn poll_millis(timeout: Duration) -> libc::c_int {
    let millis = timeout.as_nanos().div_ceil(1_000_000);
    libc::c_int::try_from(millis).unwrap_or(libc::c_int::MAX)
}

/// The Apple hardware flow-control bits of `<sys/termios.h>` (`CCTS_OFLOW`,
/// `CRTS_IFLOW`, `CDTR_IFLOW`, `CDSR_OFLOW`, `CCAR_OFLOW`), which the libc
/// crate does not name.
#[cfg(any(target_os = "macos", target_os = "ios"))]
const APPLE_FLOW_CONTROL: libc::tcflag_t =
    0x0001_0000 | 0x0002_0000 | 0x0004_0000 | 0x0008_0000 | 0x0010_0000;

/// Raw 115200 8N1 attributes derived from `original`, which stays untouched
/// for restoration: no input, output or local processing, eight data bits,
/// no parity, one stop bit, no hardware flow control, the receiver on and
/// modem control lines ignored, and reads that return at once.
pub fn raw_attributes(original: &libc::termios) -> libc::termios {
    let mut configured = *original;
    configured.c_iflag = 0;
    configured.c_oflag = 0;
    configured.c_cflag &= !(libc::CSIZE | libc::PARENB | libc::CSTOPB | libc::CRTSCTS);
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        configured.c_cflag &= !APPLE_FLOW_CONTROL;
    }
    configured.c_cflag |= libc::CS8 | libc::CREAD | libc::CLOCAL;
    configured.c_lflag = 0;
    // SAFETY: `configured` is an initialized, properly aligned termios value
    // owned by this frame, borrowed mutably for each call alone; the calls
    // only store the speed into its fields.
    unsafe {
        libc::cfsetispeed(&mut configured, libc::B115200);
        libc::cfsetospeed(&mut configured, libc::B115200);
    }
    configured.c_cc[libc::VMIN] = 0;
    configured.c_cc[libc::VTIME] = 0;
    configured
}

/// Faults that the unit tests inject on their own thread: a write of a
/// writer that takes fewer bytes, a failed restoration or application of
/// attributes, a failed wait of a receiver, a failed request for exclusive
/// access, a failed read of the original attributes and a failed release of
/// the lock, a hang-up with nothing to read, and an interrupt caught while a
/// port restores its attributes; and the requests that change a terminal,
/// which they count.
#[cfg(test)]
pub(crate) mod seams {
    use std::cell::Cell;
    use std::os::fd::RawFd;
    use std::sync::atomic::AtomicI32;

    thread_local! {
        /// The most bytes one `write(2)` of a writer takes.
        pub static WRITE_LIMIT: Cell<usize> = const { Cell::new(usize::MAX) };
        /// The `write(2)` calls the writers made.
        pub static WRITES: Cell<usize> = const { Cell::new(0) };
        /// The errno with which restoring the attributes fails.
        pub static RESTORE_ERRNO: Cell<Option<i32>> = const { Cell::new(None) };
        /// The errno with which applying the raw attributes fails.
        pub static APPLY_ERRNO: Cell<Option<i32>> = const { Cell::new(None) };
        /// The errno with which a receiver's wait for input fails.
        pub static WAIT_ERRNO: Cell<Option<i32>> = const { Cell::new(None) };
        /// Whether the `TIOCEXCL` request of a port or a receiver fails.
        pub static EXCLUSIVE_FAILS: Cell<bool> = const { Cell::new(false) };
        /// Whether a port's read of the original attributes at open fails.
        pub static TCGETATTR_FAILS: Cell<bool> = const { Cell::new(false) };
        /// Whether a port's release of its advisory lock fails.
        pub static UNLOCK_FAILS: Cell<bool> = const { Cell::new(false) };
        /// Whether the first look of the next reader thread that a port
        /// starts finds a hang-up and a read that would block, as a device
        /// that hung up with nothing left to read gives. The seam is taken
        /// when the reader starts, since the reader runs on a thread of its
        /// own.
        pub static HANG_UP: Cell<bool> = const { Cell::new(false) };
        /// The interrupt flag that restoring the attributes sets, as a
        /// signal caught there would set it.
        pub static INTERRUPT_AT_RESTORE: Cell<Option<&'static AtomicI32>> =
            const { Cell::new(None) };
        /// The requests that change a terminal: `TIOCEXCL`, `TIOCNXCL` and
        /// `tcsetattr`.
        pub static CHANGES: Cell<usize> = const { Cell::new(0) };
    }

    /// Counts one request that changes a terminal.
    pub fn change() {
        CHANGES.set(CHANGES.get() + 1);
    }

    /// Puts a descriptor that is not open in place of `*fd` when `fails`
    /// holds, so that the next request on it fails as on a closed
    /// descriptor, with `EBADF`; returns the descriptor to put back.
    pub fn set_aside(fd: &mut RawFd, fails: bool) -> RawFd {
        let kept = *fd;
        if fails {
            *fd = -1;
        }
        kept
    }
}

/// One `write(2)` of `data`: the count written, or -1 with `errno` set.
fn write_some(fd: RawFd, data: &[u8]) -> isize {
    #[cfg(test)]
    let data = {
        seams::WRITES.set(seams::WRITES.get() + 1);
        &data[..data.len().min(seams::WRITE_LIMIT.get())]
    };
    // SAFETY: `data` is an initialized byte slice, so its pointer is valid
    // for reads of `data.len()` bytes and needs no alignment; the kernel only
    // reads it during the call, while the shared borrow keeps it alive and
    // unchanged. A descriptor that is not open fails the call with `EBADF`.
    unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) }
}

/// Restores `original` on `fd`, or returns the errno of the failure.
fn restore_attributes(fd: RawFd, original: &libc::termios) -> Result<(), i32> {
    #[cfg(test)]
    seams::change();
    #[cfg(test)]
    if let Some(errno) = seams::RESTORE_ERRNO.get() {
        return Err(errno);
    }
    #[cfg(test)]
    if let Some(flag) = seams::INTERRUPT_AT_RESTORE.get() {
        flag.store(libc::SIGINT, Ordering::SeqCst);
    }
    // SAFETY: `original` is a reference to an initialized, properly aligned
    // termios value that `tcgetattr` filled in; the call only reads it while
    // the borrow keeps it alive. A descriptor that is not open or not a
    // terminal fails the call with an errno.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, original) } == 0 {
        Ok(())
    } else {
        Err(last_errno())
    }
}

/// Applies the raw attributes `raw` on `fd`, or returns the errno of the
/// failure.
fn apply_attributes(fd: RawFd, raw: &libc::termios) -> Result<(), i32> {
    #[cfg(test)]
    seams::change();
    #[cfg(test)]
    if let Some(errno) = seams::APPLY_ERRNO.get() {
        return Err(errno);
    }
    // SAFETY: `raw` is a reference to an initialized, properly aligned
    // termios value; the call only reads it while the borrow keeps it alive.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, raw) } == 0 {
        Ok(())
    } else {
        Err(last_errno())
    }
}

/// Reads the attributes of the terminal `fd`, or returns the errno of the
/// failure.
fn read_attributes(fd: RawFd) -> Result<libc::termios, i32> {
    let mut original = std::mem::MaybeUninit::<libc::termios>::zeroed();
    // SAFETY: `original` is storage for one termios value on this frame,
    // properly aligned and exclusively borrowed for the call, which writes
    // at most one termios value into it.
    if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } != 0 {
        return Err(last_errno());
    }
    // SAFETY: the storage was zero-initialized, which is a valid termios
    // value, and tcgetattr succeeded, so it holds the attributes.
    Ok(unsafe { original.assume_init() })
}

/// Requests exclusive access to the terminal `fd` with `TIOCEXCL`, or its
/// release with `TIOCNXCL`, or returns the errno of the failure.
fn exclusive(fd: RawFd, request: libc::c_ulong) -> Result<(), i32> {
    #[cfg(test)]
    seams::change();
    // SAFETY: TIOCEXCL and TIOCNXCL take no argument, so the call passes no
    // pointer and touches no memory of this process; a descriptor that is not
    // open or not a terminal fails the call with an errno.
    if unsafe { libc::ioctl(fd, request) } == 0 {
        Ok(())
    } else {
        Err(last_errno())
    }
}

/// What one look of a port's reader thread found.
enum Look {
    /// No input within the wait, or a wait that a signal interrupted.
    Nothing,
    /// The wait failed with this errno.
    WaitFailed(i32),
    /// The port was ready: whether it reported a hang-up or an error
    /// condition, and the count that the read took or its errno.
    Read {
        hung_up: bool,
        result: Result<usize, i32>,
    },
}

/// Waits at most `millis` for `fd` to become readable and then reads once
/// into `buffer`. As with `select`, a hang-up or error condition counts as
/// readable, so that the read tells how the device ended.
fn look(fd: RawFd, buffer: &mut [u8], millis: libc::c_int) -> Look {
    let mut poll = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `poll` is one initialized, properly aligned pollfd on this
    // frame, exclusively borrowed for the call, so the pointer and the count
    // 1 describe valid memory that the call may write. A descriptor that is
    // not open is reported in `revents`, not as an access to memory.
    let ready = unsafe { libc::poll(&mut poll, 1, millis) };
    if ready < 0 {
        let errno = last_errno();
        return if errno == libc::EINTR {
            Look::Nothing
        } else {
            Look::WaitFailed(errno)
        };
    }
    if ready == 0 {
        return Look::Nothing;
    }
    let hung_up = poll.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0;
    // SAFETY: `buffer` is an initialized byte slice exclusively borrowed for
    // the call, so its pointer is valid for writes of `buffer.len()` bytes and
    // needs no alignment; the call writes at most that many bytes.
    let count = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
    let result = usize::try_from(count).map_err(|_| last_errno());
    Look::Read { hung_up, result }
}

/// An exclusively opened tty with its original attributes retained.
///
/// Besides `TIOCEXCL` the port holds an advisory `flock` on the device, which
/// refuses a second port on the same tty where the kernel does not enforce
/// `TIOCEXCL`: Darwin lets a user other than root reopen a pseudo-terminal
/// slave held exclusively. Linux refuses that open to a process without
/// `CAP_SYS_ADMIN`.
pub struct TtyPort {
    fd: RawFd,
    original: Option<libc::termios>,
    /// Whether `TIOCEXCL` was taken.
    pub exclusive: bool,
    /// Whether the original attributes were restored at `finish`.
    pub restored: bool,
    locked: bool,
    closed: bool,
    stop: Arc<AtomicBool>,
    /// The error text of the read that ended the reader thread.
    error: Arc<Mutex<Option<String>>>,
}

impl TtyPort {
    /// Opens `path` read-write, non-blocking, close-on-exec and without
    /// becoming its controlling terminal, locks it, takes exclusive access
    /// and applies the raw attributes; any failure restores and closes the
    /// port. A refused open names the port: `"<path>": strerror (os error N)`;
    /// the lock and exclusive access fail with the error text alone; reading
    /// the original attributes fails with `attribute read failed: ...` and
    /// applying the raw ones with `raw attributes refused: ...`. A failure of
    /// the cleanup is reported instead, with the text of [`TtyPort::finish`].
    pub fn open(path: &str) -> Result<Self, Failure> {
        let name = c_name(path)?;
        // SAFETY: `name` is a NUL-terminated string that lives until the call
        // returns, and the call only reads it; no mode argument is needed
        // without `O_CREAT`.
        let fd = unsafe {
            libc::open(
                name.as_ptr(),
                libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(open_error(path));
        }
        let mut port = TtyPort {
            fd,
            original: None,
            exclusive: false,
            restored: false,
            locked: false,
            closed: false,
            stop: Arc::new(AtomicBool::new(false)),
            error: Arc::new(Mutex::new(None)),
        };
        if let Err(error) = port.configure() {
            port.finish()?;
            return Err(error);
        }
        Ok(port)
    }

    fn configure(&mut self) -> Result<(), Failure> {
        // SAFETY: isatty takes a descriptor and touches no memory of this
        // process; `fd` is open and owned by this port.
        let is_tty = unsafe { libc::isatty(self.fd) } == 1;
        require(is_tty, "selected port is not a tty")?;
        // Locked before anything changes, so that a refused second port
        // leaves the exclusivity and attributes of the first one alone.
        // SAFETY: flock takes a descriptor and an operation and touches no
        // memory of this process; `fd` is open and owned by this port.
        if unsafe { libc::flock(self.fd, libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(last_error());
        }
        self.locked = true;
        #[cfg(test)]
        let fd = seams::set_aside(&mut self.fd, seams::EXCLUSIVE_FAILS.get());
        let taken = exclusive(self.fd, libc::TIOCEXCL as libc::c_ulong);
        #[cfg(test)]
        {
            self.fd = fd;
        }
        taken.map_err(|errno| Failure::new(os_error(errno)))?;
        self.exclusive = true;
        #[cfg(test)]
        let fd = seams::set_aside(&mut self.fd, seams::TCGETATTR_FAILS.get());
        let original = read_attributes(self.fd);
        #[cfg(test)]
        {
            self.fd = fd;
        }
        let original = original.map_err(|errno| Failure::new(labelled(ATTRIBUTE_READ, errno)))?;
        self.original = Some(original);
        apply_attributes(self.fd, &raw_attributes(&original))
            .map_err(|errno| Failure::new(labelled(RAW_REFUSED, errno)))
    }

    /// A duplicate descriptor, close-on-exec, for the reader thread or the
    /// writer.
    fn duplicate(&self) -> Result<RawFd, Failure> {
        // SAFETY: fcntl with F_DUPFD_CLOEXEC takes an integer argument and
        // touches no memory of this process; `fd` is open and owned by this
        // port, and the new descriptor belongs to the caller.
        let duplicate = unsafe { libc::fcntl(self.fd, libc::F_DUPFD_CLOEXEC, 0) };
        if duplicate < 0 {
            return Err(last_error());
        }
        Ok(duplicate)
    }

    /// Starts the reader thread, which waits for input at most
    /// `timing.reader_poll` at a time and forwards every chunk until the port
    /// is finished, the device closes, a read fails or the receiver is gone.
    /// A hang-up or error condition counts as readable, so that the read
    /// tells how the device ended: an empty read is the end of the stream, a
    /// failed read is recorded with its error text (see
    /// [`TtyPort::read_error`]) before the stream ends, and a read that would
    /// block after a hang-up ends the stream without an error.
    pub fn start_reader(&self, sender: Sender<Vec<u8>>, timing: &Timing) -> Result<(), Failure> {
        let fd = self.duplicate()?;
        let stop = Arc::clone(&self.stop);
        let error = Arc::clone(&self.error);
        let millis = poll_millis(timing.reader_poll);
        #[cfg(test)]
        let mut hang_up = seams::HANG_UP.replace(false);
        std::thread::spawn(move || {
            let mut buffer = vec![0u8; READ_BUFFER];
            let failure = loop {
                if stop.load(Ordering::Relaxed) {
                    break None;
                }
                #[cfg(test)]
                let looked = if std::mem::take(&mut hang_up) {
                    Look::Read {
                        hung_up: true,
                        result: Err(libc::EAGAIN),
                    }
                } else {
                    look(fd, &mut buffer, millis)
                };
                #[cfg(not(test))]
                let looked = look(fd, &mut buffer, millis);
                match looked {
                    Look::Nothing => {}
                    Look::WaitFailed(errno) => break Some(os_error(errno)),
                    Look::Read { result: Ok(0), .. } => break None,
                    Look::Read {
                        result: Ok(count), ..
                    } => {
                        if sender.send(buffer[..count].to_vec()).is_err() {
                            break None;
                        }
                    }
                    Look::Read {
                        hung_up,
                        result: Err(errno),
                    } => {
                        if errno == libc::EINTR || (errno == libc::EAGAIN && !hung_up) {
                            continue;
                        }
                        // A hang-up with nothing left to read ends the stream
                        // instead of polling it again at once, so that a
                        // session reports the closed device. A Darwin pty
                        // reads empty instead, and a Linux pty fails the read
                        // with `EIO`.
                        break (errno != libc::EAGAIN).then(|| os_error(errno));
                    }
                }
            };
            if failure.is_some() {
                *error.lock().unwrap_or_else(|p| p.into_inner()) = failure;
            }
            // SAFETY: close takes a descriptor and touches no memory of this
            // process; the duplicate belongs to this thread alone and is
            // closed exactly once, here.
            unsafe { libc::close(fd) };
            // `sender` is dropped after the error is recorded, so a session
            // that sees the stream end also sees the error.
            drop(sender);
        });
        Ok(())
    }

    /// The error text that the reader thread recorded when a read failed,
    /// such as `Device not configured (os error 6)` for an unplugged adapter
    /// on Darwin.
    pub fn read_error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// A writer for the port with the bounded request protocol: each request
    /// is sent within `timing.request_transmit` and before `deadline`, the
    /// deadline of the session, and recorded in `transmitted`.
    pub fn writer(
        &self,
        deadline: Instant,
        transmitted: Arc<Mutex<Vec<Value>>>,
        timing: &Timing,
    ) -> Result<TtyWriter, Failure> {
        Ok(TtyWriter {
            fd: self.duplicate()?,
            deadline,
            request_transmit: timing.request_transmit,
            transmitted,
        })
    }

    /// Restores the original attributes, releases exclusive access and the
    /// lock and closes the port, each step also after a failed one. The
    /// failed steps are reported in that order:
    /// `serial cleanup failed: attribute restoration failed: ...; exclusive
    /// access release failed: ...`. A failed release of the lock has no text
    /// of its own; the lock then lasts until the duplicates of the reader
    /// thread and the writer are closed too. A second call does nothing.
    pub fn finish(&mut self) -> Result<(), Failure> {
        if self.closed {
            return Ok(());
        }
        self.stop.store(true, Ordering::Relaxed);
        let mut errors = Vec::new();
        if let Some(original) = &self.original {
            match restore_attributes(self.fd, original) {
                Ok(()) => self.restored = true,
                Err(errno) => errors.push(labelled(RESTORATION, errno)),
            }
        }
        if self.exclusive
            && let Err(errno) = exclusive(self.fd, libc::TIOCNXCL as libc::c_ulong)
        {
            errors.push(labelled(RELEASE, errno));
        }
        if self.locked {
            // Released here rather than when the last duplicate closes, so
            // that the tty can be opened again at once.
            #[cfg(test)]
            let fd = seams::set_aside(&mut self.fd, seams::UNLOCK_FAILS.get());
            // SAFETY: flock takes a descriptor and an operation and touches
            // no memory of this process; `fd` is open and owned by this port.
            unsafe { libc::flock(self.fd, libc::LOCK_UN) };
            #[cfg(test)]
            {
                self.fd = fd;
            }
        }
        // SAFETY: close takes a descriptor and touches no memory of this
        // process; `fd` is open, owned by this port and closed exactly once,
        // as `closed` is set next.
        unsafe { libc::close(self.fd) };
        self.closed = true;
        require(
            errors.is_empty(),
            &format!("serial cleanup failed: {}", errors.join("; ")),
        )
    }
}

impl Drop for TtyPort {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

/// A tty opened read-only for a capture that only receives. Opening it sends
/// no data, though opening a tty can still change its modem lines. It takes
/// `TIOCEXCL` and the raw 115200 8N1 attributes of a [`TtyPort`] but no
/// advisory lock; it reads no more than it is asked for, so that unread input
/// stays queued, and never writes or flushes the port.
pub struct Receiver {
    fd: RawFd,
    original: Option<libc::termios>,
    /// Whether `TIOCEXCL` was taken.
    pub exclusive: bool,
    /// Whether the original attributes were restored at [`Receiver::close`].
    pub restored: bool,
    closed: bool,
}

impl Receiver {
    /// Opens `path` read-only, non-blocking, close-on-exec and without
    /// becoming its controlling terminal. A refused open names the port:
    /// `"<path>": strerror (os error N)`.
    pub fn open(path: &str) -> Result<Self, Failure> {
        let name = c_name(path)?;
        // SAFETY: `name` is a NUL-terminated string that lives until the call
        // returns, and the call only reads it; no mode argument is needed
        // without `O_CREAT`.
        let fd = unsafe {
            libc::open(
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(open_error(path));
        }
        Ok(Receiver {
            fd,
            original: None,
            exclusive: false,
            restored: false,
            closed: false,
        })
    }

    /// The steps after the open, up to the first that fails: the port must
    /// be a tty (`selected port is not a tty`), then exclusive access (the
    /// error text alone), then the original attributes, kept for
    /// [`Receiver::close`] (`attribute read failed: ...`), and the raw ones
    /// (`raw attributes refused: ...`).
    pub fn configure(&mut self) -> Result<(), Failure> {
        // SAFETY: isatty takes a descriptor and touches no memory of this
        // process; `fd` is open and owned by this receiver.
        let is_tty = unsafe { libc::isatty(self.fd) } == 1;
        require(is_tty, "selected port is not a tty")?;
        #[cfg(test)]
        let fd = seams::set_aside(&mut self.fd, seams::EXCLUSIVE_FAILS.get());
        let taken = exclusive(self.fd, libc::TIOCEXCL as libc::c_ulong);
        #[cfg(test)]
        {
            self.fd = fd;
        }
        taken.map_err(|errno| Failure::new(os_error(errno)))?;
        self.exclusive = true;
        let original = read_attributes(self.fd)
            .map_err(|errno| Failure::new(labelled(ATTRIBUTE_READ, errno)))?;
        self.original = Some(original);
        apply_attributes(self.fd, &raw_attributes(&original))
            .map_err(|errno| Failure::new(labelled(RAW_REFUSED, errno)))
    }

    /// Whether input, or the end of the stream, is ready within `timeout`,
    /// rounded up to a millisecond. A wait that a signal interrupts returns
    /// `false` at once, so that the caller can look at its interrupt flag; a
    /// failed wait fails with its error text.
    pub fn wait(&self, timeout: Duration) -> Result<bool, Failure> {
        #[cfg(test)]
        if let Some(errno) = seams::WAIT_ERRNO.get() {
            return Err(Failure::new(os_error(errno)));
        }
        let mut poll = libc::pollfd {
            fd: self.fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `poll` is one initialized, properly aligned pollfd on this
        // frame, exclusively borrowed for the call, so the pointer and the
        // count 1 describe valid memory that the call may write.
        let ready = unsafe { libc::poll(&mut poll, 1, poll_millis(timeout)) };
        if ready < 0 {
            let errno = last_errno();
            if errno == libc::EINTR {
                return Ok(false);
            }
            return Err(Failure::new(os_error(errno)));
        }
        // A hang-up or error condition counts as ready, so that the read
        // tells how the stream ended.
        Ok(ready > 0)
    }

    /// One read of at most `limit` bytes: the bytes read, empty at the end
    /// of the stream, or `None` when the read would block or a signal
    /// interrupted it; a failed read fails with its error text.
    pub fn read(&self, limit: usize) -> Result<Option<Vec<u8>>, Failure> {
        let mut buffer = vec![0u8; limit];
        // SAFETY: `buffer` is an initialized vector of `limit` bytes,
        // exclusively borrowed for the call, so its pointer is valid for
        // writes of `limit` bytes and needs no alignment; the call writes at
        // most that many bytes.
        let count = unsafe { libc::read(self.fd, buffer.as_mut_ptr().cast(), limit) };
        let Ok(count) = usize::try_from(count) else {
            let errno = last_errno();
            if errno == libc::EAGAIN || errno == libc::EINTR {
                return Ok(None);
            }
            return Err(Failure::new(os_error(errno)));
        };
        buffer.truncate(count);
        Ok(Some(buffer))
    }

    /// Restores the original attributes, releases `TIOCEXCL` and closes the
    /// descriptor, each step also after a failed one: the texts of the failed
    /// steps in that order, `attribute restoration failed: ...`, `exclusive
    /// access release failed: ...` and `port close failed: ...`. The
    /// descriptor is closed once; a second call returns no text.
    pub fn close(&mut self) -> Vec<String> {
        if self.closed {
            return Vec::new();
        }
        self.closed = true;
        let mut errors = Vec::new();
        if let Some(original) = &self.original {
            match restore_attributes(self.fd, original) {
                Ok(()) => self.restored = true,
                Err(errno) => errors.push(labelled(RESTORATION, errno)),
            }
        }
        if self.exclusive
            && let Err(errno) = exclusive(self.fd, libc::TIOCNXCL as libc::c_ulong)
        {
            errors.push(labelled(RELEASE, errno));
        }
        // SAFETY: close takes a descriptor and touches no memory of this
        // process; `fd` is open, owned by this receiver and closed exactly
        // once, as `closed` is already set.
        if unsafe { libc::close(self.fd) } != 0 {
            errors.push(labelled("port close failed", last_errno()));
        }
        errors
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

/// The text with which a session or a capture ends once an interrupt was
/// caught.
pub const INTERRUPTED: &str = "interrupted";

/// The process-wide interrupt flag: 0, or the first signal caught.
static INTERRUPT: AtomicI32 = AtomicI32::new(0);

extern "C" fn record_interrupt(signal: libc::c_int) {
    // An atomic operation on a lock-free integer is async-signal-safe.
    let _ = INTERRUPT.compare_exchange(0, signal, Ordering::SeqCst, Ordering::SeqCst);
}

/// Sets the disposition of `signal` to `handler`, without `SA_RESTART`, so
/// that a blocking call returns and the flag is seen at once.
fn set_disposition(signal: libc::c_int, handler: libc::sighandler_t) -> Result<(), Failure> {
    // SAFETY: sigaction is a C struct of integers, a signal set and a handler
    // address, for which all zero bits are a valid value: no handler, no
    // flags and an empty mask.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = handler;
    action.sa_flags = 0;
    // SAFETY: `sa_mask` is an initialized, properly aligned signal set inside
    // `action`, exclusively borrowed for the call, which only writes it.
    unsafe { libc::sigemptyset(&mut action.sa_mask) };
    // SAFETY: `action` is an initialized, properly aligned sigaction that the
    // call only reads while the borrow keeps it alive, and the null pointer
    // requests no copy of the previous action. `handler` is the address of
    // `record_interrupt`, which only stores into an atomic integer and so is
    // async-signal-safe.
    if unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) } != 0 {
        return Err(last_error());
    }
    Ok(())
}

/// Installs SIGINT, SIGTERM and SIGHUP handlers (sigaction, no SA_RESTART)
/// that store the first signal number into the returned process-wide flag.
/// The handlers replace an inherited `SIG_IGN`, so that a signal left
/// ignored, as `nohup` leaves SIGHUP and a background job of a script
/// SIGINT, is caught. Idempotent: a second call installs the same handlers
/// and keeps the flag.
pub fn catch_interrupts() -> Result<&'static AtomicI32, Failure> {
    let handler = record_interrupt as extern "C" fn(libc::c_int) as libc::sighandler_t;
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        set_disposition(signal, handler)?;
    }
    Ok(&INTERRUPT)
}

/// Bounded requests of 1 to 128 bytes, each sent within its transmit time
/// and the deadline of the session, and recorded with its UTC time stamp,
/// its bytes in hexadecimal and the count of them sent. A failed wait or
/// write fails with the operating-system error.
pub struct TtyWriter {
    fd: RawFd,
    deadline: Instant,
    request_transmit: Duration,
    transmitted: Arc<Mutex<Vec<Value>>>,
}

impl TtyWriter {
    fn record(&self, entry: Value) -> usize {
        let mut list = self.transmitted.lock().unwrap_or_else(|p| p.into_inner());
        list.push(entry);
        list.len() - 1
    }

    fn update(&self, index: usize, sent: usize) {
        let mut list = self.transmitted.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(entry) = list.get_mut(index).and_then(Value::as_object_mut) {
            entry.insert("sent_bytes".into(), sent.into());
        }
    }
}

impl Write for TtyWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.is_empty() || data.len() > REQUEST_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid serial request size",
            ));
        }
        let index = self.record(json!({
            "at": utc_now(),
            "hex": hex(data),
            "sent_bytes": 0,
        }));
        let deadline = Instant::now()
            .checked_add(self.request_transmit)
            .map_or(self.deadline, |end| end.min(self.deadline));
        let mut sent = 0usize;
        while sent < data.len() {
            let now = Instant::now();
            if now >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "serial transmit timeout",
                ));
            }
            let mut poll = libc::pollfd {
                fd: self.fd,
                events: libc::POLLOUT,
                revents: 0,
            };
            // SAFETY: `poll` is one initialized, properly aligned pollfd on
            // this frame, exclusively borrowed for the call, so the pointer
            // and the count 1 describe valid memory that the call may write.
            let ready = unsafe { libc::poll(&mut poll, 1, poll_millis(deadline - now)) };
            if ready < 0 {
                let errno = last_errno();
                if errno == libc::EINTR {
                    continue;
                }
                return Err(io::Error::from_raw_os_error(errno));
            }
            // A hang-up or error condition counts as writable, so that the
            // write reports it.
            let writable = libc::POLLOUT | libc::POLLHUP | libc::POLLERR | libc::POLLNVAL;
            if ready == 0 || poll.revents & writable == 0 {
                continue;
            }
            let count = write_some(self.fd, &data[sent..]);
            let Ok(count) = usize::try_from(count) else {
                let errno = last_errno();
                if errno == libc::EAGAIN || errno == libc::EINTR {
                    continue;
                }
                return Err(io::Error::from_raw_os_error(errno));
            };
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "serial write made no progress",
                ));
            }
            sent += count;
            self.update(index, sent);
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for TtyWriter {
    fn drop(&mut self) {
        // SAFETY: close takes a descriptor and touches no memory of this
        // process; the duplicate belongs to this writer alone and is closed
        // exactly once, here.
        unsafe { libc::close(self.fd) };
    }
}

#[cfg(test)]
mod tests;
