//! Writing the records of a run into its output directory, which the run
//! creates: the receipt, then its public version, each a new file that
//! nothing else has written.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;

use kyto_bench_core::instance::InstanceValue;
use kyto_bench_core::public::{self, Leak};
use kyto_bench_core::receipt::Receipt;

/// The file of the receipt in the output directory.
pub const RECEIPT: &str = "receipt.json";

/// The file of the public version of the receipt in the output directory.
pub const PUBLIC: &str = "receipt.public.json";

/// Creates the output directory `out`, which must not exist yet.
pub fn create(out: &Path) -> Result<(), String> {
    fs::create_dir(out).map_err(|error| format!("cannot create {}: {error}", out.display()))
}

/// Removes the output directory `out` if it is empty, as it is when the run
/// wrote nothing.
pub fn remove_empty(out: &Path) {
    // A directory that holds a file stays.
    let _ = fs::remove_dir(out);
}

/// Writes `receipt` into `out`, then its public version with the instance
/// values `hidden` hidden, unless the leak check refuses it; the leaks that
/// kept it from being written.
pub fn write(out: &Path, receipt: &Receipt, hidden: &[InstanceValue]) -> Result<Vec<Leak>, String> {
    write_new(&out.join(RECEIPT), &receipt.render())?;
    match public::public_version(receipt, hidden) {
        Ok(public) => write_new(&out.join(PUBLIC), &public.render()).map(|()| Vec::new()),
        Err(leaks) => Ok(leaks),
    }
}

/// Writes `text` into a new file at `path`, which must not exist.
fn write_new(path: &Path, text: &str) -> Result<(), String> {
    let failed = |error: std::io::Error| format!("cannot write {}: {error}", path.display());
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(failed)?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(failed)
}
