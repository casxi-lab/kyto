//! The `kyto-bench` command. This build has no subcommand yet: it prints its
//! usage and exits with status 2.

use std::process::ExitCode;

fn main() -> ExitCode {
    eprintln!("usage: kyto-bench <command>\n\nThis build of kyto-bench has no command yet.");
    ExitCode::from(2)
}
