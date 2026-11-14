//! The command line of `kyto-bench`. Its command `run` runs a run plan on
//! the bench that an instance configuration describes, writes the receipt
//! and its public version into a new directory, and gives the result as its
//! exit status; `power-off` and `publish`, of [`crate::commands`], switch
//! the bench's supply off and publish the payload of one entry.

use std::ffi::OsString;
use std::io::{self, Write as _};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use clap::{Arg, ArgMatches, Command, value_parser};

use kyto_bench_core::receipt::Verdict;

use crate::commands;
use crate::env::Env;
use crate::inputs::Inputs;
use crate::output::{self, RECEIPT};
use crate::run::{self, NoReceipt};

/// The exit status of a run that passed.
pub const PASS: u8 = 0;
/// The exit status of a run that failed.
pub const FAIL: u8 = 1;
/// The exit status of a usage or input error, and of a run whose receipt
/// could not be written; no receipt is left.
pub const INPUT_ERROR: u8 = 2;
/// The exit status of a run that a fault of the bench ended.
pub const BENCH_FAULT: u8 = 3;
/// The exit status of a run refused before any power command; the receipt
/// says why.
pub const REFUSED: u8 = 4;
/// The exit status of a run whose receipt was written but not its public
/// version: the leak check refused it, since it would show a value to hide,
/// or it could not be written; the receipt gives the result.
pub const PUBLIC_REFUSED: u8 = 5;
/// The exit status of a run, or a publication, that an interrupt stopped;
/// no receipt is left.
pub const INTERRUPTED: u8 = 130;

const EXIT_STATUS: &str = "Exit status: 0 PASS, 1 FAIL, 2 usage or input error, or a receipt \
that cannot be written (no receipt), 3 BENCH FAULT, 4 refused before any power command \
(receipt written), 5 public version refused or not written (receipt written), 101 an error \
of the tool itself, after switching the supply off (no receipt), 130 interrupted, after \
switching the supply off if a power command was sent (no receipt).";

const POWER_OFF_STATUS: &str = "Exit status: 0 readings confirmed the power off, 2 usage or \
input error, 3 BENCH FAULT: readings did not confirm the power off or a device failed, 101 an \
error of the tool itself. An interrupt does not stop it. When a run or a publication holds the \
bench, it says so first: the power off cuts that run, whose receipt does not show it and is no \
evidence; that run also holds the meter on macOS, so the power off is sent but not confirmed \
(3), and power-off run again after that run ended confirms it.";

const PUBLISH_STATUS: &str = "Exit status: 0 published: the board is served the new \
generation and its copy has the digests of the run plan, 2 usage or input error, 3 BENCH \
FAULT: the meter failed before the switch, the switch failed, or the copy that the board is \
served after the switch could not be read or differs, 4 refused: a payload check failed, the \
TFTP directory could not be staged (another run or publication holds it, or a write failed), \
or readings did not confirm the supply off; what the board is served did not change, 101 an \
error of the tool itself, 130 interrupted before the switch. It sends no power command: when \
the state of the board is unknown, run power-off first.";

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
    let plan = path("plan", "FILE", "The run plan");
    let instance = path(
        "instance",
        "FILE",
        "The instance configuration of the bench",
    );
    let payloads = path(
        "payload-root",
        "DIR",
        "The directory of the payload directory of each entry",
    );
    let out = path(
        "out",
        "DIR",
        "The directory to create for the receipt; it must not exist",
    );
    let run = Command::new("run")
        .about("Runs a run plan and writes its receipt and the receipt's public version")
        .args([plan.clone(), instance.clone(), payloads.clone(), out])
        .after_help(EXIT_STATUS);
    let power_off = Command::new("power-off")
        .about("Switches the supply off once and waits for readings to confirm it")
        .arg(instance.clone())
        .after_help(POWER_OFF_STATUS);
    let entry = Arg::new("entry")
        .long("entry")
        .value_name("NAME")
        .required(true);
    let publish = Command::new("publish")
        .about("Publishes the payload of one entry while readings confirm the supply off")
        .args([plan, instance, payloads])
        .arg(entry.help("The name of the entry of the run plan"))
        .after_help(PUBLISH_STATUS);
    Command::new("kyto-bench")
        .about("The kyto hardware-in-the-loop bench tool")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .disable_help_subcommand(true)
        .subcommands([run, power_off, publish])
}

/// Runs the command line `args`, its first item the name of the program, in
/// the surroundings `env`; the exit status.
pub fn run<T: Into<OsString> + Clone>(args: impl IntoIterator<Item = T>, env: &dyn Env) -> u8 {
    let matches = match command().try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(error) => {
            let _ = error.print();
            return usage_status(&error);
        }
    };
    let ran = match matches.subcommand() {
        Some(("run", args)) => run_plan(args, env),
        Some(("power-off", args)) => commands::power_off(env, path(args, "instance")),
        Some(("publish", args)) => {
            let entry = args.get_one::<String>("entry").map_or("", String::as_str);
            let (plan, instance) = (path(args, "plan"), path(args, "instance"));
            commands::publish(env, plan, instance, path(args, "payload-root"), entry)
        }
        _ => return INPUT_ERROR,
    };
    ran.unwrap_or_else(|message| {
        env.say(&message);
        INPUT_ERROR
    })
}

/// The path that the option `name` of `args` gives.
fn path<'a>(args: &'a ArgMatches, name: &str) -> &'a Path {
    args.get_one::<PathBuf>(name)
        .map_or(Path::new(""), PathBuf::as_path)
}

/// The exit status of a command line that clap does not run: [`PASS`] for
/// help, [`INPUT_ERROR`] for a usage error.
fn usage_status(error: &clap::Error) -> u8 {
    u8::try_from(error.exit_code()).unwrap_or(INPUT_ERROR)
}

/// The command `run`.
fn run_plan(args: &ArgMatches, env: &dyn Env) -> Result<u8, String> {
    let out = path(args, "out");
    let (plan, instance) = (path(args, "plan"), path(args, "instance"));
    let inputs = Inputs::read(plan, instance, path(args, "payload-root"), env.boards())?;
    output::create(out)?;
    // A run that gives no receipt, or unwinds, leaves no output directory.
    let ran = panic::catch_unwind(AssertUnwindSafe(|| run::run(env, &inputs)));
    if !matches!(ran, Ok(Ok(_))) {
        output::remove_empty(out);
    }
    let receipt = match ran.unwrap_or_else(|panic| panic::resume_unwind(panic)) {
        Ok(receipt) => receipt,
        Err(NoReceipt::Input(message)) => return Err(message),
        Err(NoReceipt::Interrupted) => return Ok(INTERRUPTED),
    };
    let (status, result) = match (&receipt.refusal, receipt.verdict) {
        (Some(refusal), _) => (REFUSED, format!("refused by {}", refusal.rule)),
        (None, Some(Verdict::Pass)) => (PASS, "PASS".into()),
        (None, Some(Verdict::Fail)) => (FAIL, "FAIL".into()),
        _ => (BENCH_FAULT, "BENCH FAULT".into()),
    };
    let written = output::write(out, &receipt, &inputs.instance.deidentify_values());
    let unwritten = written.map_err(|message| {
        output::remove_empty(out);
        format!("the run gave {result}, but no receipt: {message}")
    })?;
    let _ = writeln!(io::stdout(), "{result}: {}", out.join(RECEIPT).display());
    for why in &unwritten {
        env.say(&format!(
            "the run gave {result}, but no public version: {why}"
        ));
    }
    Ok(if unwritten.is_empty() {
        status
    } else {
        PUBLIC_REFUSED
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_result_has_the_exit_status_of_its_own() {
        assert_eq!([PASS, FAIL, INPUT_ERROR], [0, 1, 2]);
        assert_eq!([BENCH_FAULT, REFUSED, PUBLIC_REFUSED], [3, 4, 5]);
    }

    #[test]
    fn the_command_line_has_its_three_commands() {
        command().debug_assert();
        let command = command();
        let commands: Vec<_> = command.get_subcommands().map(|c| c.get_name()).collect();
        assert_eq!(commands, ["run", "power-off", "publish"]);
        let statuses = [EXIT_STATUS, POWER_OFF_STATUS, PUBLISH_STATUS];
        for (name, status) in commands.iter().zip(statuses) {
            let mut sub = command.find_subcommand(name).unwrap().clone();
            let help = sub.render_long_help().to_string();
            assert!(help.contains(status), "{help}");
        }
        assert_eq!(INTERRUPTED, 130);
        let publish = ["kyto-bench", "publish", "--plan", "p", "--instance", "i"];
        for (args, status) in [
            (&["kyto-bench"][..], INPUT_ERROR),
            (&["kyto-bench", "--help"], PASS),
            (&["kyto-bench", "check"], INPUT_ERROR),
            (&["kyto-bench", "run", "--plan", "p"], INPUT_ERROR),
            (&["kyto-bench", "power-off"], INPUT_ERROR),
            (
                &["kyto-bench", "power-off", "--instance", "i", "--plan", "p"],
                INPUT_ERROR,
            ),
            (&["kyto-bench", "power-off", "--instance", "i"], PASS),
            (
                &[&publish[..], &["--payload-root", "r"]].concat(),
                INPUT_ERROR,
            ),
            (
                &[&publish[..], &["--payload-root", "r", "--entry", "e"]].concat(),
                PASS,
            ),
        ] {
            assert_eq!(run_quietly(args), status, "{args:?}");
        }
    }

    /// The exit status that [`run`] gives `args`, without the messages
    /// that clap prints.
    fn run_quietly(args: &[&str]) -> u8 {
        match command().try_get_matches_from(args) {
            Ok(_) => PASS,
            Err(error) => usage_status(&error),
        }
    }
}
