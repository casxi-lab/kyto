//! The `kyto-bench` command, whose command line [`kyto_bench::cli`]
//! describes.

use std::process::ExitCode;

use kyto_bench::cli;
use kyto_bench::env::HostEnv;

fn main() -> ExitCode {
    ExitCode::from(cli::run(std::env::args_os(), &HostEnv))
}
