use super::*;
use crate::TEST_WAIT;
use crate::child::{ChildTest, child_part};
use crate::pty::{Pty, exclusive_open_refused, lock_refused, open_read_only};
use std::os::fd::AsRawFd;
use std::sync::mpsc::{RecvTimeoutError, channel};

/// The time values of these tests: the slices of a Pi 5 session, and a
/// request transmit time that a slow host never exceeds.
const TIMING: Timing = Timing {
    request_transmit: TEST_WAIT,
    ..Timing::PI5
};

#[test]
fn raw_attributes_select_115200_8n1_without_flow_control_or_processing() {
    // SAFETY: termios is a C struct of integers and arrays of them, for
    // which all zero bits are a valid value.
    let mut original: libc::termios = unsafe { std::mem::zeroed() };
    original.c_iflag = libc::ICRNL;
    original.c_oflag = libc::OPOST;
    original.c_cflag = libc::PARENB | libc::CSTOPB | libc::CRTSCTS | libc::CS7;
    original.c_lflag = libc::ECHO | libc::ICANON;
    original.c_cc[libc::VMIN] = 1;
    original.c_cc[libc::VTIME] = 5;
    let raw = raw_attributes(&original);
    assert_eq!(raw.c_iflag, 0);
    assert_eq!(raw.c_oflag, 0);
    assert_eq!(raw.c_lflag, 0);
    assert_eq!(
        raw.c_cflag & (libc::PARENB | libc::CSTOPB | libc::CRTSCTS),
        0
    );
    assert_eq!(raw.c_cflag & libc::CSIZE, libc::CS8);
    assert_eq!(
        raw.c_cflag & (libc::CREAD | libc::CLOCAL),
        libc::CREAD | libc::CLOCAL
    );
    // SAFETY: `raw` is an initialized, properly aligned termios value that
    // the calls only read.
    unsafe {
        assert_eq!(libc::cfgetispeed(&raw), libc::B115200);
        assert_eq!(libc::cfgetospeed(&raw), libc::B115200);
    }
    assert_eq!(raw.c_cc[libc::VMIN], 0);
    assert_eq!(raw.c_cc[libc::VTIME], 0);
    assert_eq!(original.c_cc[libc::VMIN], 1, "the original stays untouched");
}

/// A port over a descriptor of the crate directory: a directory fails every
/// read and terminal request, which provokes the errors a board can raise
/// on a host without one.
fn directory_port() -> TtyPort {
    let path = CString::new(env!("CARGO_MANIFEST_DIR")).unwrap();
    // SAFETY: `path` is a NUL-terminated string that lives until the call
    // returns, and the call only reads it.
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    assert!(fd >= 0, "cannot open {path:?}");
    TtyPort {
        fd,
        original: None,
        exclusive: false,
        restored: false,
        locked: false,
        closed: false,
        stop: Arc::new(AtomicBool::new(false)),
        error: Arc::new(Mutex::new(None)),
    }
}

#[test]
fn a_failed_read_ends_the_stream_and_keeps_its_error() {
    let port = directory_port();
    let (sender, receiver) = channel();
    port.start_reader(sender, &TIMING).unwrap();
    assert_eq!(
        receiver.recv_timeout(TEST_WAIT),
        Err(RecvTimeoutError::Disconnected)
    );
    assert_eq!(
        port.read_error().as_deref(),
        Some("Is a directory (os error 21)")
    );
}

/// The reader of a device that hung up with nothing left to read, whose
/// read would block, ends the stream instead of polling the device again,
/// and records no error. Neither a Darwin nor a Linux pseudo-terminal gives
/// this, so the seam gives the first look of the reader that result.
#[test]
fn a_hang_up_with_nothing_to_read_ends_the_stream_without_an_error() {
    let pty = Pty::open();
    let mut port = TtyPort::open(pty.path()).unwrap();
    let (sender, receiver) = channel();
    seams::HANG_UP.set(true);
    let started = port.start_reader(sender, &TIMING);
    let left = seams::HANG_UP.replace(false);
    started.unwrap();
    assert!(!left, "the reader takes the seam when it starts");
    assert_eq!(
        receiver.recv_timeout(TEST_WAIT),
        Err(RecvTimeoutError::Disconnected)
    );
    assert_eq!(port.read_error(), None);
    port.finish().unwrap();
    assert!(port.restored);
}

#[test]
fn cleanup_failures_are_reported_in_order() {
    let mut port = directory_port();
    // SAFETY: termios is a C struct of integers and arrays of them, for
    // which all zero bits are a valid value to restore.
    port.original = Some(unsafe { std::mem::zeroed() });
    port.exclusive = true;
    assert_eq!(
        port.finish(),
        Err(Failure::new(
            "serial cleanup failed: attribute restoration failed: Inappropriate ioctl \
             for device (os error 25); exclusive access release failed: Inappropriate \
             ioctl for device (os error 25)"
        ))
    );
    assert!(!port.restored);
    assert_eq!(port.finish(), Ok(()), "the descriptor is closed once");
}

/// With every write taking one byte, a request is still sent whole and
/// recorded once with its full count and the time stamp of `utc_now`.
#[test]
fn partial_transmit_is_completed_and_recorded() {
    let pty = Pty::open();
    let mut port = TtyPort::open(pty.path()).unwrap();
    let transmitted = Arc::new(Mutex::new(Vec::new()));
    let mut writer = port
        .writer(
            Instant::now() + TEST_WAIT,
            Arc::clone(&transmitted),
            &TIMING,
        )
        .unwrap();
    seams::WRITE_LIMIT.set(1);
    seams::WRITES.set(0);
    let written = writer.write(b"abc");
    seams::WRITE_LIMIT.set(usize::MAX);
    assert_eq!(written.unwrap(), 3);
    assert_eq!(seams::WRITES.get(), 3, "one byte per write");
    assert_eq!(pty.read(3), b"abc");
    let entries = transmitted.lock().unwrap().clone();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["hex"], "616263");
    assert_eq!(entries[0]["sent_bytes"], 3);
    let keys: Vec<&str> = entries[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["at", "hex", "sent_bytes"]);
    let at = entries[0]["at"].as_str().unwrap();
    assert_eq!(at.len(), "2026-10-14T07:45:17.123456Z".len(), "{at}");
    assert_eq!(&at[10..11], "T", "{at}");
    assert!(at.ends_with('Z'), "{at}");
    drop(writer);
    port.finish().unwrap();
    assert!(port.restored);
}

/// A restoration that fails is reported, and the port still releases its
/// exclusivity and lock and closes, so the tty opens again.
#[test]
fn restore_failure_closes_and_releases() {
    let pty = Pty::open();
    let mut port = TtyPort::open(pty.path()).unwrap();
    seams::RESTORE_ERRNO.set(Some(libc::EIO));
    let finished = port.finish();
    seams::RESTORE_ERRNO.set(None);
    assert_eq!(
        finished,
        Err(Failure::new(
            "serial cleanup failed: attribute restoration failed: Input/output error (os error 5)"
        ))
    );
    assert!(port.closed);
    assert!(!port.restored);
    let mut again = TtyPort::open(pty.path()).unwrap();
    again.finish().unwrap();
    assert!(again.restored);
}

/// An open that fails cleans the port up, and a failure of that cleanup
/// replaces the open error. With the raw attributes refused, the open fails
/// with that refusal while the restoration succeeds, and with the cleanup
/// failure once the restoration fails too. The port is released either way.
#[test]
fn a_failed_cleanup_replaces_the_open_error() {
    let pty = Pty::open();
    seams::APPLY_ERRNO.set(Some(libc::EIO));
    let refused = TtyPort::open(pty.path()).map(drop);
    seams::RESTORE_ERRNO.set(Some(libc::EIO));
    let replaced = TtyPort::open(pty.path()).map(drop);
    seams::APPLY_ERRNO.set(None);
    seams::RESTORE_ERRNO.set(None);
    assert_eq!(
        refused,
        Err(Failure::new(
            "raw attributes refused: Input/output error (os error 5)"
        ))
    );
    assert_eq!(
        replaced,
        Err(Failure::new(
            "serial cleanup failed: attribute restoration failed: Input/output error (os error 5)"
        ))
    );
    let mut again = TtyPort::open(pty.path()).unwrap();
    again.finish().unwrap();
    assert!(again.restored);
}

/// `TIOCEXCL` is requested on the port at every open: a request that fails,
/// here on a descriptor that is not open, fails the open with its error
/// text. The exclusivity was not taken, so the cleanup releases none and
/// sets no attributes, and it releases the lock, so the tty opens again.
#[test]
fn a_failed_tiocexcl_at_open_fails_the_open() {
    let pty = Pty::open();
    seams::CHANGES.set(0);
    seams::EXCLUSIVE_FAILS.set(true);
    let failed = TtyPort::open(pty.path()).map(drop);
    seams::EXCLUSIVE_FAILS.set(false);
    let changes = seams::CHANGES.get();
    assert_eq!(
        failed,
        Err(Failure::new("Bad file descriptor (os error 9)"))
    );
    assert_eq!(changes, 1, "TIOCEXCL alone: no TIOCNXCL and no tcsetattr");
    let mut again = TtyPort::open(pty.path()).unwrap();
    again.finish().unwrap();
    assert!(again.restored);
}

/// A read of the original attributes that fails at open, here on a
/// descriptor that is not open, fails the open with the label of that step.
/// The cleanup releases the exclusivity and the lock and has no attributes
/// to restore, so the tty opens again.
#[test]
fn a_failed_tcgetattr_at_open_fails_the_open() {
    let pty = Pty::open();
    seams::CHANGES.set(0);
    seams::TCGETATTR_FAILS.set(true);
    let failed = TtyPort::open(pty.path()).map(drop);
    seams::TCGETATTR_FAILS.set(false);
    let changes = seams::CHANGES.get();
    assert_eq!(
        failed,
        Err(Failure::new(
            "attribute read failed: Bad file descriptor (os error 9)"
        ))
    );
    assert_eq!(changes, 2, "TIOCEXCL and its release, and no tcsetattr");
    let mut again = TtyPort::open(pty.path()).unwrap();
    again.finish().unwrap();
    assert!(again.restored);
}

/// A second port on a held tty is refused without a request that changes
/// the terminal: it takes no `TIOCEXCL`, so its cleanup issues no `TIOCNXCL`
/// that would clear the held port's exclusivity, and it sets no attributes.
/// Where the kernel refuses the open of a terminal held exclusively, the
/// open fails and names the port; elsewhere the lock, taken before anything
/// changes, refuses it.
#[test]
fn a_refused_second_open_changes_nothing() {
    let pty = Pty::open();
    let mut held = TtyPort::open(pty.path()).unwrap();
    seams::CHANGES.set(0);
    let refused = TtyPort::open(pty.path()).map(drop);
    let changes = seams::CHANGES.get();
    let expected = if exclusive_open_refused() {
        Failure::new(format!(
            "{:?}: Device or resource busy (os error 16)",
            pty.path()
        ))
    } else {
        lock_refused()
    };
    assert_eq!(refused, Err(expected));
    assert_eq!(changes, 0, "requests that change the held terminal");
    held.finish().unwrap();
    assert!(held.restored);
}

/// The lock is taken before anything changes, on every host: with the
/// advisory lock held on another open of the tty, which takes no
/// `TIOCEXCL`, the kernel lets the port's open through, the lock refuses
/// the port, and neither a request that changes the terminal nor a change
/// of its attributes has happened.
#[test]
fn a_port_locked_elsewhere_is_refused_before_it_changes_anything() {
    let pty = Pty::open();
    let before = pty.attributes();
    let other = open_read_only(pty.path()).expect("open the tty again");
    // SAFETY: flock takes a descriptor and an operation and touches no
    // memory of this process; the descriptor is open.
    let locked = unsafe { libc::flock(other.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(locked, 0, "lock: {}", std::io::Error::last_os_error());
    seams::CHANGES.set(0);
    let refused = TtyPort::open(pty.path()).map(drop);
    let changes = seams::CHANGES.get();
    assert_eq!(refused, Err(lock_refused()));
    assert_eq!(changes, 0, "requests that change the terminal");
    assert_eq!(pty.attributes(), before);
    // Released explicitly, as a process that another test spawns meanwhile
    // may hold a copy of the descriptor until its exec.
    // SAFETY: as above.
    let unlocked = unsafe { libc::flock(other.as_raw_fd(), libc::LOCK_UN) };
    assert_eq!(unlocked, 0, "unlock: {}", std::io::Error::last_os_error());
    drop(other);
    let mut again = TtyPort::open(pty.path()).unwrap();
    again.finish().unwrap();
    assert!(again.restored);
}

/// Opens a port on `path` once its advisory lock has ended. A lock that no
/// `flock(LOCK_UN)` released ends with the last descriptor of its open file,
/// and a process that another test of this binary spawns meanwhile holds a
/// copy of every descriptor until its exec closes the copies. A test fails
/// when the lock has not ended within [`TEST_WAIT`].
fn open_once_unlocked(path: &str) -> TtyPort {
    let deadline = Instant::now() + TEST_WAIT;
    loop {
        match TtyPort::open(path) {
            Err(failure) if failure == lock_refused() && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            opened => return opened.unwrap(),
        }
    }
}

/// `finish` releases the lock, so the tty opens again while a writer's
/// duplicate is still open. A release that fails, here on a descriptor that
/// is not open, has no text of its own: `finish` succeeds, or fails with the
/// texts of its other steps alone. The lock then lasts until the reader's
/// and the writer's duplicates are closed too.
#[test]
fn a_failed_unlock_has_no_text_and_the_duplicates_keep_the_lock() {
    let pty = Pty::open();
    let deadline = Instant::now() + TEST_WAIT;
    let mut port = TtyPort::open(pty.path()).unwrap();
    let writer = port.writer(deadline, Arc::default(), &TIMING).unwrap();
    port.finish().unwrap();
    TtyPort::open(pty.path()).unwrap().finish().unwrap();
    drop(writer);

    let mut port = TtyPort::open(pty.path()).unwrap();
    let (sender, receiver) = channel();
    port.start_reader(sender, &TIMING).unwrap();
    let writer = port.writer(deadline, Arc::default(), &TIMING).unwrap();
    seams::UNLOCK_FAILS.set(true);
    let finished = port.finish();
    seams::UNLOCK_FAILS.set(false);
    assert_eq!(finished, Ok(()));
    assert!(port.restored);
    assert!(port.closed);
    assert_eq!(
        TtyPort::open(pty.path()).map(drop),
        Err(lock_refused()),
        "the duplicates keep the lock"
    );
    drop(writer);
    // The reader closes its duplicate before it drops the sender.
    assert_eq!(
        receiver.recv_timeout(TEST_WAIT),
        Err(RecvTimeoutError::Disconnected)
    );
    let mut again = open_once_unlocked(pty.path());
    again.finish().unwrap();
    assert!(again.restored);

    let mut port = TtyPort::open(pty.path()).unwrap();
    seams::UNLOCK_FAILS.set(true);
    seams::RESTORE_ERRNO.set(Some(libc::EIO));
    let finished = port.finish();
    seams::UNLOCK_FAILS.set(false);
    seams::RESTORE_ERRNO.set(None);
    assert_eq!(
        finished,
        Err(Failure::new(
            "serial cleanup failed: attribute restoration failed: Input/output error (os error 5)"
        ))
    );
    let mut again = open_once_unlocked(pty.path());
    again.finish().unwrap();
    assert!(again.restored);
}

/// A port name with a NUL byte has no C form: both opens refuse it with the
/// name and the reason, before any request reaches a terminal.
#[test]
fn a_port_name_with_a_nul_byte_is_refused() {
    let expected = Failure::new("\"/dev/tty\\0x\": nul byte found in provided data at position: 8");
    assert_eq!(
        TtyPort::open("/dev/tty\0x").map(drop),
        Err(expected.clone())
    );
    assert_eq!(Receiver::open("/dev/tty\0x").map(drop), Err(expected));
}

/// A port that cannot be opened is named before the error text, in Rust's
/// debug form, which escapes a control character.
#[test]
fn a_port_that_cannot_be_opened_is_named() {
    let scratch = crate::scratch::Scratch::new("tty-missing");
    let missing = scratch.path("no\u{7f}port");
    let missing = missing.to_str().unwrap();
    let expected = Failure::new(format!(
        "\"{}\": No such file or directory (os error 2)",
        missing.replace('\u{7f}', "\\u{7f}")
    ));
    assert_eq!(TtyPort::open(missing).map(drop), Err(expected.clone()));
    assert_eq!(Receiver::open(missing).map(drop), Err(expected));
}

/// The device and inode `fd` refers to, if it is open.
fn identity(fd: RawFd) -> Option<(libc::dev_t, libc::ino_t, libc::dev_t)> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::zeroed();
    // SAFETY: `stat` is storage for one stat value on this frame, properly
    // aligned and exclusively borrowed for the call, which writes at most one
    // stat value into it.
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: the storage was zero-initialized, which is a valid stat value,
    // and fstat succeeded, so it holds the status of `fd`.
    let stat = unsafe { stat.assume_init() };
    Some((stat.st_dev, stat.st_ino, stat.st_rdev))
}

/// A receiver opens its port read-only, non-blocking and close-on-exec, and
/// takes no advisory lock: another descriptor of the tty, opened before the
/// receiver takes `TIOCEXCL`, can lock it. Where the kernel refuses a new
/// open of a terminal held exclusively, a new open is refused once the
/// receiver is configured.
#[test]
fn a_receiver_opens_read_only_without_a_lock() {
    let pty = Pty::open();
    let mut receiver = Receiver::open(pty.path()).unwrap();
    let other = open_read_only(pty.path()).expect("open the tty again");
    receiver.configure().unwrap();
    // SAFETY: fcntl with F_GETFL takes no argument and touches no memory of
    // this process; the descriptor is open.
    let status = unsafe { libc::fcntl(receiver.fd, libc::F_GETFL) };
    assert_eq!(status & libc::O_ACCMODE, libc::O_RDONLY);
    assert_eq!(status & libc::O_NONBLOCK, libc::O_NONBLOCK);
    // SAFETY: fcntl with F_GETFD takes no argument and touches no memory of
    // this process; the descriptor is open.
    let status = unsafe { libc::fcntl(receiver.fd, libc::F_GETFD) };
    assert_eq!(status & libc::FD_CLOEXEC, libc::FD_CLOEXEC);
    // SAFETY: flock takes a descriptor and an operation and touches no
    // memory of this process; the descriptor is open.
    let locked = unsafe { libc::flock(other.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(locked, 0, "the receiver holds a lock");
    if exclusive_open_refused() {
        assert_eq!(
            open_read_only(pty.path())
                .map(drop)
                .map_err(|e| e.raw_os_error()),
            Err(Some(libc::EBUSY)),
            "a new open of the held terminal"
        );
    }
    drop(other);
    assert!(receiver.exclusive);
    assert!(receiver.close().is_empty());
    assert!(receiver.restored);
}

/// A receiver whose restoration fails reports it and still releases its
/// exclusivity and closes its descriptor, once.
#[test]
fn a_receiver_closes_after_a_failed_restoration() {
    let pty = Pty::open();
    let mut receiver = Receiver::open(pty.path()).unwrap();
    receiver.configure().unwrap();
    let open = identity(receiver.fd);
    assert!(open.is_some());
    seams::RESTORE_ERRNO.set(Some(libc::EIO));
    seams::CHANGES.set(0);
    let errors = receiver.close();
    let changes = seams::CHANGES.get();
    seams::RESTORE_ERRNO.set(None);
    assert_eq!(
        errors,
        ["attribute restoration failed: Input/output error (os error 5)"]
    );
    assert_eq!(changes, 2, "the restoration and the release of TIOCEXCL");
    assert!(!receiver.restored);
    // Another test may reuse the number at once, never for this pty.
    assert_ne!(identity(receiver.fd), open);
    assert!(receiver.close().is_empty(), "the descriptor is closed once");
}

/// A receiver requests `TIOCEXCL` on its port: a request that fails, here
/// on a descriptor that is not open, fails `configure` with its error text.
/// The exclusivity was not taken, so `close` releases none, has no
/// attributes to restore and closes the port.
#[test]
fn a_receivers_failed_tiocexcl_fails_the_configuration() {
    let pty = Pty::open();
    let mut receiver = Receiver::open(pty.path()).unwrap();
    seams::CHANGES.set(0);
    seams::EXCLUSIVE_FAILS.set(true);
    let failed = receiver.configure();
    seams::EXCLUSIVE_FAILS.set(false);
    assert_eq!(
        failed,
        Err(Failure::new("Bad file descriptor (os error 9)"))
    );
    assert!(!receiver.exclusive);
    assert!(receiver.close().is_empty());
    assert_eq!(
        seams::CHANGES.get(),
        1,
        "TIOCEXCL alone: no TIOCNXCL and no tcsetattr"
    );
    assert!(!receiver.restored);
}

/// The cleanup of a receiver over a directory descriptor, with the original
/// attributes kept and `TIOCEXCL` taken: the restoration and the release of
/// the exclusivity are each attempted and fail, in that order, and the
/// descriptor is closed.
#[test]
fn a_receivers_cleanup_failures_are_reported_in_order() {
    let path = CString::new(env!("CARGO_MANIFEST_DIR")).unwrap();
    // SAFETY: `path` is a NUL-terminated string that lives until the call
    // returns, and the call only reads it.
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    assert!(fd >= 0, "cannot open {path:?}");
    // SAFETY: termios is a C struct of integers and arrays of them, for
    // which all zero bits are a valid value to restore.
    let original = Some(unsafe { std::mem::zeroed() });
    let mut receiver = Receiver {
        fd,
        original,
        exclusive: true,
        restored: false,
        closed: false,
    };
    assert_eq!(
        receiver.close(),
        [
            "attribute restoration failed: Inappropriate ioctl for device (os error 25)",
            "exclusive access release failed: Inappropriate ioctl for device (os error 25)",
        ]
    );
    assert!(!receiver.restored);
    assert!(receiver.close().is_empty(), "the descriptor is closed once");
}

/// The signal that the parent passed this process, if it is a child of a
/// signal test.
fn child_signal() -> Option<libc::c_int> {
    child_part().map(|signal| signal.parse().expect("a signal number"))
}

/// Sends `signal` to the calling thread, which runs its handler before the
/// call returns.
fn raise(signal: libc::c_int) {
    // SAFETY: raise takes only a signal number and touches no memory of this
    // process; it runs within a child that the test runs alone.
    assert_eq!(unsafe { libc::raise(signal) }, 0, "signal {signal}");
}

/// Runs the test `test` of this module in a child for each of the three
/// signals and requires each child to pass.
fn passes_in_children(test: &str) {
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        let ended = ChildTest::spawn(module_path!(), test, &signal.to_string()).finish();
        assert!(
            ended.passed(),
            "signal {signal}: {:?}\n{}\n{}",
            ended.status,
            ended.lines.join("\n"),
            ended.stderr
        );
    }
}

/// SIGINT, SIGTERM and SIGHUP are caught, so the process lives on, and the
/// flag keeps the first signal.
#[test]
fn sigint_sigterm_and_sighup_are_caught_and_the_first_is_kept() {
    if let Some(first) = child_signal() {
        let flag = catch_interrupts().unwrap();
        raise(first);
        assert_eq!(flag.load(Ordering::SeqCst), first);
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            raise(signal);
        }
        assert_eq!(flag.load(Ordering::SeqCst), first, "the first is kept");
        return;
    }
    passes_in_children("sigint_sigterm_and_sighup_are_caught_and_the_first_is_kept");
}

/// A signal inherited ignored is caught: the handlers replace `SIG_IGN`. The
/// child ignores the signal, so that raising it changes nothing, and catches
/// it after `catch_interrupts`.
#[test]
fn a_signal_inherited_ignored_is_caught() {
    if let Some(signal) = child_signal() {
        // SAFETY: signal takes a signal number and a disposition and touches
        // no memory of this process; SIG_IGN is a valid disposition for the
        // three signals, and the child runs this test alone.
        let previous = unsafe { libc::signal(signal, libc::SIG_IGN) };
        assert_ne!(previous, libc::SIG_ERR);
        raise(signal);
        let flag = catch_interrupts().unwrap();
        assert_eq!(flag.load(Ordering::SeqCst), 0);
        raise(signal);
        assert_eq!(flag.load(Ordering::SeqCst), signal);
        return;
    }
    passes_in_children("a_signal_inherited_ignored_is_caught");
}
