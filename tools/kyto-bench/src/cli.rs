//! The command line of `kyto-bench`. Its one command, `run`, runs a run plan
//! on the bench that an instance configuration describes, writes the
//! receipt and its public version into a new directory, and gives the
//! result as its exit status.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use clap::{Arg, ArgMatches, Command, value_parser};

use kyto_bench_core::receipt::Verdict;

use crate::env::Env;
use crate::inputs::Inputs;
use crate::output::{self, RECEIPT};
use crate::run;

/// The exit status of a run that passed.
pub const PASS: u8 = 0;
/// The exit status of a run that failed.
pub const FAIL: u8 = 1;
/// The exit status of a usage or input error, when no receipt was written,
/// and of a run whose receipt or public version could not be written.
pub const INPUT_ERROR: u8 = 2;
/// The exit status of a run that a fault of the bench ended.
pub const BENCH_FAULT: u8 = 3;
/// The exit status of a run refused before any power command; the receipt
/// says why.
pub const REFUSED: u8 = 4;
/// The exit status of a run whose receipt was written but whose public
/// version was refused, since it would show a value to hide; the receipt
/// gives the verdict.
pub const PUBLIC_REFUSED: u8 = 5;

const EXIT_STATUS: &str = "Exit status: 0 PASS, 1 FAIL, 2 usage or input error (no receipt), \
or an output file that cannot be written (the error gives the result), 3 BENCH FAULT, \
4 refused before any power command (receipt written), 5 public version refused (receipt written).";

/// The command line.
pub fn command() -> Command {
    let path = |name: &'static str, value: &'static str, help: &'static str| {
        let parser = value_parser!(PathBuf);
        let arg = Arg::new(name)
            .long(name)
            .value_name(value)
            .value_parser(parser);
        arg.required(true).help(help)
    };
    let run = Command::new("run")
        .about("Runs a run plan and writes its receipt and the receipt's public version")
        .arg(path("plan", "FILE", "The run plan"))
        .arg(path(
            "instance",
            "FILE",
            "The instance configuration of the bench",
        ))
        .arg(path(
            "payload-root",
            "DIR",
            "The directory of the payload directory of each entry",
        ))
        .arg(path(
            "out",
            "DIR",
            "The directory to create for the receipt; it must not exist",
        ))
        .after_help(EXIT_STATUS);
    Command::new("kyto-bench")
        .about("The kyto hardware-in-the-loop bench tool")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .disable_help_subcommand(true)
        .subcommand(run)
}

/// Runs the command line `args`, its first item the name of the program, in
/// the surroundings `env`; the exit status.
pub fn run<T: Into<OsString> + Clone>(args: impl IntoIterator<Item = T>, env: &dyn Env) -> u8 {
    let matches = match command().try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(error) => {
            let _ = error.print();
            return u8::try_from(error.exit_code()).unwrap_or(INPUT_ERROR);
        }
    };
    let Some(("run", args)) = matches.subcommand() else {
        return INPUT_ERROR;
    };
    run_plan(args, env).unwrap_or_else(|message| {
        eprintln!("kyto-bench: {message}");
        INPUT_ERROR
    })
}

/// The command `run`.
fn run_plan(args: &ArgMatches, env: &dyn Env) -> Result<u8, String> {
    let path = |name| {
        args.get_one::<PathBuf>(name)
            .map_or(Path::new(""), PathBuf::as_path)
    };
    let out = path("out");
    let inputs = Inputs::read(
        path("plan"),
        path("instance"),
        path("payload-root"),
        env.boards(),
    )?;
    output::create(out)?;
    let receipt = run::run(env, &inputs).inspect_err(|_| output::remove_empty(out))?;
    let (status, result) = match (&receipt.refusal, receipt.verdict) {
        (Some(refusal), _) => (REFUSED, format!("refused by {}", refusal.rule)),
        (None, Some(Verdict::Pass)) => (PASS, "PASS".into()),
        (None, Some(Verdict::Fail)) => (FAIL, "FAIL".into()),
        _ => (BENCH_FAULT, "BENCH FAULT".into()),
    };
    let written = output::write(out, &receipt, &inputs.instance.deidentify_values());
    let leaks = written.map_err(|message| format!("the run gave {result}, but {message}"))?;
    println!("{result}: {}", out.join(RECEIPT).display());
    for leak in &leaks {
        let (rule, field) = (&leak.rule, &leak.field);
        eprintln!("kyto-bench: no public version: {rule} finds a value to hide at {field:?}");
    }
    Ok(if leaks.is_empty() {
        status
    } else {
        PUBLIC_REFUSED
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_has_the_run_command_only() {
        command().debug_assert();
        let command = command();
        let commands: Vec<_> = command.get_subcommands().map(|c| c.get_name()).collect();
        assert_eq!(commands, ["run"]);
        let mut run = command.find_subcommand("run").unwrap().clone();
        let help = run.render_long_help().to_string();
        assert!(help.contains(EXIT_STATUS), "{help}");
        for (args, status) in [
            (&["kyto-bench"][..], INPUT_ERROR),
            (&["kyto-bench", "--help"], PASS),
            (&["kyto-bench", "check"], INPUT_ERROR),
            (&["kyto-bench", "run", "--plan", "p"], INPUT_ERROR),
        ] {
            assert_eq!(run_quietly(args), status, "{args:?}");
        }
    }

    /// The exit status of `args`, whose messages clap prints.
    fn run_quietly(args: &[&str]) -> u8 {
        match command().try_get_matches_from(args) {
            Ok(_) => PASS,
            Err(error) => u8::try_from(error.exit_code()).unwrap_or(INPUT_ERROR),
        }
    }
}
