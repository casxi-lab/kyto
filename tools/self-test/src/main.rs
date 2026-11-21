//! `kyto-self-test <file>` writes the bench self-test kernel to `<file>` and
//! prints its SHA-256 and the file's path. Its exit status is 0 when the
//! file was written, 1 when it could not be, and 2 for a usage error.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use kyto_bench_core::digest::Sha256;

fn main() -> ExitCode {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [path] = args.as_slice() else {
        let _ = writeln!(std::io::stderr(), "usage: kyto-self-test <file>");
        return ExitCode::from(2);
    };
    let image = kyto_self_test::kernel();
    if let Err(error) = std::fs::write(path, &image) {
        let _ = writeln!(
            std::io::stderr(),
            "kyto-self-test: {}: {error}",
            path.display()
        );
        return ExitCode::from(1);
    }
    let _ = writeln!(
        std::io::stdout(),
        "{}  {}",
        Sha256::of(&image),
        path.display()
    );
    ExitCode::SUCCESS
}
