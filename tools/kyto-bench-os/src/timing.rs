//! The time slices, polling intervals and timeouts of the host tools, given
//! per platform rather than written into the code that waits.
//!
//! A [`Timing`] holds every such value of the serial port transport, the
//! serial capture, the serial link, the serial capture of a guest and the
//! process helpers. Callers pass the set of the platform they talk to; tests
//! that wait for an event pass a set whose bounds are generous, so that a
//! slow host fails no test. Byte limits are not times and stay constants of
//! their modules.

use std::time::Duration;

/// The time values of the serial port transport, the serial capture, the
/// serial link, the serial capture of a guest and the process helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// The longest wait of a port's reader thread for input before it looks
    /// at its stop flag again.
    pub reader_poll: Duration,
    /// The longest transmission of one request to a port, within the
    /// deadline of the session that sends it.
    pub request_transmit: Duration,
    /// The longest wait of a serial capture for input before it looks at its
    /// interrupt flag again.
    pub capture_slice: Duration,
    /// The longest wait of a serial link for output in one look: after its
    /// stream ended, while it waits for the ready line and while it drains
    /// the output that follows a failure.
    pub link_slice: Duration,
    /// How long a serial link collects the output that follows a failure.
    pub link_drain: Duration,
    /// The longest final read of a serial link that is finished, until its
    /// stream ends.
    pub link_finish: Duration,
    /// The longest wait for output in one look of that final read.
    pub link_finish_slice: Duration,
    /// The longest wait of a guest's serial capture for output in one look:
    /// while it waits for an expected response, while it collects the
    /// diagnostics that follow a failure, and while the QEMU runner
    /// supervises QEMU.
    pub guest_slice: Duration,
    /// How long a guest's serial capture may keep receiving output at a
    /// phase boundary before the guest counts as not idle.
    pub guest_idle: Duration,
    /// The longest final collection of a guest's serial output once the
    /// guest has stopped, until its stream ends; for the QEMU runner, the
    /// collection of the rest of QEMU's output.
    pub guest_finish: Duration,
    /// The longest wait for output in one look of that final collection.
    pub guest_finish_slice: Duration,
    /// How long a stopped process may take to end after SIGTERM before it is
    /// killed.
    pub stop_grace: Duration,
    /// How long a killed process may take to end before its status is given
    /// up as unknown.
    pub kill_grace: Duration,
    /// The interval between two looks at whether a process has ended.
    pub reap_poll: Duration,
}

impl Timing {
    /// The values for a guest on QEMU virt, whose serial port is the standard
    /// streams of the QEMU process.
    pub const QEMU_VIRT: Timing = Timing {
        reader_poll: Duration::from_millis(100),
        request_transmit: Duration::from_secs(2),
        capture_slice: Duration::from_millis(100),
        link_slice: Duration::from_millis(20),
        link_drain: Duration::from_millis(250),
        link_finish: Duration::from_secs(1),
        link_finish_slice: Duration::from_millis(50),
        guest_slice: Duration::from_millis(100),
        guest_idle: Duration::from_millis(250),
        guest_finish: Duration::from_secs(1),
        guest_finish_slice: Duration::from_millis(50),
        stop_grace: Duration::from_secs(1),
        kill_grace: Duration::from_secs(1),
        reap_poll: Duration::from_millis(10),
    };

    /// The values for a Raspberry Pi 5 whose UART is reached through a serial
    /// port of the host.
    pub const PI5: Timing = Timing {
        reader_poll: Duration::from_millis(100),
        request_transmit: Duration::from_secs(2),
        capture_slice: Duration::from_millis(100),
        link_slice: Duration::from_millis(20),
        link_drain: Duration::from_millis(250),
        link_finish: Duration::from_secs(1),
        link_finish_slice: Duration::from_millis(50),
        guest_slice: Duration::from_millis(100),
        guest_idle: Duration::from_millis(250),
        guest_finish: Duration::from_secs(1),
        guest_finish_slice: Duration::from_millis(50),
        stop_grace: Duration::from_secs(1),
        kill_grace: Duration::from_secs(1),
        reap_poll: Duration::from_millis(10),
    };
}
