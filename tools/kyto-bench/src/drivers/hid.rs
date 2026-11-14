//! The USB link to the MCP2221 or MCP2221A bridge of the meter: one 64-byte
//! HID report out, the next report in, never waiting, so that every wait
//! and timeout of the meter runs on the bench clock. [`HidapiOpener`] opens
//! the one bridge of the host through the `hidapi` crate, exclusively;
//! failures keep only which step failed, never a text of the library, a
//! path or a serial number.

use std::collections::BTreeSet;
use std::ffi::CStr;
use std::fmt;

use hidapi::{DeviceInfo, HidApi, HidDevice};

/// One 64-byte HID report of the bridge, without a report ID.
pub type Report = [u8; 64];

/// The USB vendor and product of the bridge: the factory values of the
/// MCP2221 and the MCP2221A.
pub const BRIDGE: (u16, u16) = (0x04D8, 0x00DD);

/// The link to an open bridge.
pub trait HidLink {
    /// Hands one output report to the bridge.
    fn send(&mut self, report: &Report) -> Result<(), LinkError>;
    /// The next input report that has arrived, if any; never waits.
    fn receive(&mut self) -> Result<Option<Report>, LinkError>;
}

/// What opens the bridge for a meter, at its first reading and again after
/// its link failed.
pub trait HidOpen {
    type Link: HidLink;
    /// Finds the one bridge and opens it exclusively.
    fn open(&mut self) -> Result<Self::Link, OpenError>;
}

/// A link that failed; the meter opens the bridge again at its next
/// reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkError {
    /// The bridge did not take a report.
    NotTaken,
    /// The bridge is gone, or gave a report of another length.
    Gone,
}

/// Why no bridge was opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenError {
    NoBridge,
    SeveralBridges,
    /// The bridge is held by another program, or the host refused it.
    NotOpened,
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotTaken => "the MCP2221 USB bridge did not take a report",
            Self::Gone => "the MCP2221 USB bridge is gone",
        })
    }
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoBridge => "no MCP2221 USB bridge is connected",
            Self::SeveralBridges => {
                "more than one MCP2221 USB bridge is connected; connect only the meter's bridge"
            }
            Self::NotOpened => {
                "the MCP2221 USB bridge cannot be opened; another program may be using it"
            }
        })
    }
}

/// The one bridge among the device `paths` of the bridge's vendor and
/// product. A host may list a device once for each of its collections, all
/// with the same path, so paths are counted once each.
pub fn select<'a>(paths: impl IntoIterator<Item = &'a CStr>) -> Result<&'a CStr, OpenError> {
    let mut paths = paths.into_iter().collect::<BTreeSet<_>>().into_iter();
    match (paths.next(), paths.next()) {
        (None, _) => Err(OpenError::NoBridge),
        (Some(path), None) => Ok(path),
        _ => Err(OpenError::SeveralBridges),
    }
}

/// What a read that did not wait gave: `read` bytes into `buffer`, or
/// `None` when it failed. No byte is nothing yet, 64 bytes are a report,
/// and any other length or a failure means the bridge is gone.
pub fn report_of(read: Option<usize>, buffer: &[u8]) -> Result<Option<Report>, LinkError> {
    match read {
        Some(0) => Ok(None),
        Some(64) => Report::try_from(&buffer[..64])
            .map(Some)
            .or(Err(LinkError::Gone)),
        _ => Err(LinkError::Gone),
    }
}

/// Opens the bridge of the host through `hidapi`.
#[derive(Clone, Copy, Debug, Default)]
pub struct HidapiOpener;

/// A bridge that `hidapi` opened.
pub struct HidapiLink(HidDevice);

impl HidOpen for HidapiOpener {
    type Link = HidapiLink;

    /// Lists the HID devices of the host, which reads their properties and
    /// opens none, and opens the one bridge exclusively.
    fn open(&mut self) -> Result<HidapiLink, OpenError> {
        let api = HidApi::new().or(Err(OpenError::NotOpened))?;
        let bridges = api.device_list();
        let bridges = bridges.filter(|info| (info.vendor_id(), info.product_id()) == BRIDGE);
        let path = select(bridges.map(DeviceInfo::path))?;
        let device = api.open_path(path).or(Err(OpenError::NotOpened))?;
        Ok(HidapiLink(device))
    }
}

impl HidLink for HidapiLink {
    /// Writes the report after the report ID 0, as the bridge numbers no
    /// reports.
    fn send(&mut self, report: &Report) -> Result<(), LinkError> {
        let mut data = [0; 65];
        data[1..].copy_from_slice(report);
        self.0.write(&data).map(drop).or(Err(LinkError::NotTaken))
    }

    fn receive(&mut self) -> Result<Option<Report>, LinkError> {
        let mut buffer = [0; 65];
        let read = self.0.read_timeout(&mut buffer, 0);
        report_of(read.ok(), &buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hid_read_maps_to_a_report_nothing_or_gone() {
        let mut buffer = [0; 65];
        buffer[0] = 0x10;
        buffer[63] = 0x7F;
        buffer[64] = 0xEE;
        assert_eq!(report_of(Some(0), &buffer), Ok(None));
        let report = report_of(Some(64), &buffer).unwrap().unwrap();
        assert_eq!((report[0], report[63], report.len()), (0x10, 0x7F, 64));
        for read in [Some(1), Some(63), Some(65), None] {
            assert_eq!(report_of(read, &buffer), Err(LinkError::Gone), "{read:?}");
        }
    }

    #[test]
    fn the_bridge_is_the_one_path_of_its_vendor_and_product() {
        let (one, two) = (c"DevSrvsID:4294970001", c"DevSrvsID:4294970002");
        assert_eq!(select([]), Err(OpenError::NoBridge));
        assert_eq!(select([one]), Ok(one));
        assert_eq!(select([one, one, one]), Ok(one));
        assert_eq!(select([one, two, one]), Err(OpenError::SeveralBridges));
    }
}
