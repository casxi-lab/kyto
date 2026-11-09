//! `cargo xtask check`: the required checks, in order, stopping at the first
//! failure.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use crate::repo::{Entry, Tree};
use crate::{commits, docs, receipt, workflows};

/// The Cargo workspaces that the format, Clippy and test steps cover, as
/// repository-relative directories.
pub const WORKSPACES: [&str; 1] = ["xtask"];

/// A command run in the repository root.
#[derive(Debug, Clone)]
pub struct Cmd {
    pub program: OsString,
    pub args: Vec<String>,
}

/// Named values that a step observed, in the order it observed them, for the
/// receipt. A step adds them as it goes, so they are kept when it fails.
pub type Facts = Vec<(String, String)>;

/// A check written in this crate, run with the repository root.
pub type Check = Box<dyn Fn(&Path, &mut Facts) -> Result<(), String>>;

/// What a step does.
pub enum Action {
    /// Run commands in order; each must exit with status 0.
    Commands(Vec<Cmd>),
    /// Run a check written in this crate.
    Internal(Check),
}

/// A named step of the check.
pub struct Step {
    pub name: &'static str,
    pub action: Action,
}

/// How a step ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The step passed.
    Passed,
    /// The step failed, and why.
    Failed(String),
    /// An earlier step failed, so this one did not run.
    NotRun,
}

/// The result of one step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub step: &'static str,
    pub status: Status,
    pub duration: Duration,
    pub facts: Facts,
}

fn cargo() -> OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

/// The same Cargo command for each workspace.
fn per_workspace(args: &[&str], trailing: &[&str]) -> Vec<Cmd> {
    WORKSPACES
        .iter()
        .map(|workspace| {
            let mut all: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
            all.push("--manifest-path".into());
            all.push(format!("{workspace}/Cargo.toml"));
            all.extend(trailing.iter().map(|arg| arg.to_string()));
            Cmd {
                program: cargo(),
                args: all,
            }
        })
        .collect()
}

/// The steps of `cargo xtask check`, in the order they run.
pub fn steps() -> Vec<Step> {
    vec![
        Step {
            name: "fmt",
            action: Action::Commands(per_workspace(&["fmt", "--all", "--check"], &[])),
        },
        Step {
            name: "clippy",
            action: Action::Commands(per_workspace(
                &["clippy", "--workspace", "--all-targets", "--locked"],
                &["--", "-D", "warnings"],
            )),
        },
        Step {
            name: "test",
            action: Action::Commands(per_workspace(&["test", "--workspace", "--locked"], &[])),
        },
        Step {
            name: "docs",
            action: Action::Internal(Box::new(|root, _| docs::run(root))),
        },
        Step {
            name: "commits",
            action: Action::Internal(Box::new(commits::run)),
        },
        Step {
            name: "workflows",
            action: Action::Internal(Box::new(check_workflows)),
        },
    ]
}

/// The workflows step on the tracked files; the files it read become facts.
fn check_workflows(root: &Path, facts: &mut Facts) -> Result<(), String> {
    let tree = Tree::load(root)?;
    let tracked = || {
        tree.entries().map(|(path, entry)| match entry {
            Entry::File(bytes) => (path, Some(bytes.as_slice())),
            Entry::Symlink | Entry::Submodule => (path, None),
        })
    };
    let files: Vec<&str> = tracked()
        .map(|(path, _)| path)
        .filter(|path| workflows::is_workflow(path))
        .collect();
    if files.is_empty() {
        println!("workflows: no workflow files");
    } else {
        println!("workflows: {}", files.join(", "));
    }
    facts.push(("files".to_string(), files.join(", ")));
    let problems = workflows::check(tracked());
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

/// Runs `steps` in order in `root` and stops at the first failure. Every
/// step has an outcome; those after a failure did not run.
pub fn run_steps(root: &Path, steps: &[Step]) -> Vec<Outcome> {
    let mut outcomes = Vec::new();
    let mut failed = false;
    for step in steps {
        let mut facts = Facts::new();
        if failed {
            outcomes.push(Outcome {
                step: step.name,
                status: Status::NotRun,
                duration: Duration::ZERO,
                facts,
            });
            continue;
        }
        println!("==> {}", step.name);
        let started = Instant::now();
        let result = match &step.action {
            Action::Commands(commands) => {
                commands.iter().try_for_each(|cmd| run_command(root, cmd))
            }
            Action::Internal(check) => check(root, &mut facts),
        };
        let status = match result {
            Ok(()) => Status::Passed,
            Err(detail) => {
                failed = true;
                Status::Failed(detail)
            }
        };
        outcomes.push(Outcome {
            step: step.name,
            status,
            duration: started.elapsed(),
            facts,
        });
    }
    outcomes
}

/// The failed step and why it failed, if one did.
pub fn failure(outcomes: &[Outcome]) -> Option<(&'static str, &str)> {
    outcomes.iter().find_map(|outcome| match &outcome.status {
        Status::Failed(detail) => Some((outcome.step, detail.as_str())),
        _ => None,
    })
}

fn run_command(root: &Path, cmd: &Cmd) -> Result<(), String> {
    let shown = format!("{} {}", cmd.program.to_string_lossy(), cmd.args.join(" "));
    println!("$ {shown}");
    let status = Command::new(&cmd.program)
        .args(&cmd.args)
        .current_dir(root)
        .status()
        .map_err(|error| format!("cannot run `{shown}`: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{shown}` exited with {status}"))
    }
}

/// Entry point of `cargo xtask check`. With `receipt`, the run is recorded
/// there whether it passes or fails.
pub fn run(root: PathBuf, receipt: Option<PathBuf>) -> ExitCode {
    let start = receipt
        .as_ref()
        .map(|_| (receipt::GitState::read(&root), receipt::Tools::read(&root)));
    let outcomes = run_steps(&root, &steps());
    let mut code = match failure(&outcomes) {
        None => {
            println!("check passed");
            ExitCode::SUCCESS
        }
        Some((step, detail)) => {
            eprintln!("{detail}");
            eprintln!("check failed at step {step}");
            ExitCode::FAILURE
        }
    };
    if let (Some(path), Some((before, tools))) = (receipt, start) {
        let after = receipt::GitState::read(&root);
        let text = receipt::render(
            &before,
            &after,
            &outcomes,
            &tools,
            &receipt::Environment::read(),
        );
        match receipt::write(&path, &text) {
            Ok(()) => println!("receipt written to {}", path.display()),
            Err(error) => {
                eprintln!("cannot write the receipt {}: {error}", path.display());
                code = ExitCode::FAILURE;
            }
        }
    }
    code
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    const ORDER: [&str; 6] = ["fmt", "clippy", "test", "docs", "commits", "workflows"];

    #[test]
    fn steps_run_in_the_documented_order() {
        let names: Vec<&str> = steps().iter().map(|step| step.name).collect();
        assert_eq!(names, ORDER);
    }

    #[test]
    fn command_steps_cover_every_workspace() {
        for step in steps() {
            if let Action::Commands(commands) = step.action {
                let targets: Vec<&str> = commands
                    .iter()
                    .map(|cmd| {
                        let at = cmd.args.iter().position(|arg| arg == "--manifest-path");
                        cmd.args[at.expect("each command names a manifest") + 1].as_str()
                    })
                    .collect();
                assert_eq!(targets, ["xtask/Cargo.toml"]);
            }
        }
    }

    /// The real step list with `failing` replaced by a failing action and
    /// every other step replaced by one that records that it ran and adds a
    /// fact.
    fn with_failure(failing: &str, ran: &Rc<RefCell<Vec<&'static str>>>) -> Vec<Step> {
        steps()
            .into_iter()
            .map(|step| {
                let name = step.name;
                let action = if name == failing {
                    match step.action {
                        // A real process that exits with a non-zero status.
                        Action::Commands(_) => Action::Commands(vec![Cmd {
                            program: "sh".into(),
                            args: vec!["-c".into(), "exit 3".into()],
                        }]),
                        Action::Internal(_) => Action::Internal(Box::new(|_, facts| {
                            facts.push(("seen".to_string(), "before failing".to_string()));
                            Err("forced failure".to_string())
                        })),
                    }
                } else {
                    let ran = Rc::clone(ran);
                    Action::Internal(Box::new(move |_, facts| {
                        ran.borrow_mut().push(name);
                        facts.push(("ran".to_string(), name.to_string()));
                        Ok(())
                    }))
                };
                Step { name, action }
            })
            .collect()
    }

    fn assert_fails_at(failing: &'static str) {
        let ran = Rc::new(RefCell::new(Vec::new()));
        let steps = with_failure(failing, &ran);
        let outcomes = run_steps(Path::new("."), &steps);
        let (step, _) = failure(&outcomes).expect("the check fails");
        assert_eq!(step, failing);
        let before: Vec<&str> = ORDER
            .iter()
            .copied()
            .take_while(|name| *name != failing)
            .collect();
        assert_eq!(
            *ran.borrow(),
            before,
            "steps after the failure must not run"
        );
        let names: Vec<&str> = outcomes.iter().map(|outcome| outcome.step).collect();
        assert_eq!(names, ORDER, "every step has an outcome");
        let failed_at = ORDER.iter().position(|name| *name == failing).unwrap();
        for (index, outcome) in outcomes.iter().enumerate() {
            match index.cmp(&failed_at) {
                std::cmp::Ordering::Less => {
                    assert_eq!(outcome.status, Status::Passed);
                    assert_eq!(
                        outcome.facts,
                        [("ran".to_string(), outcome.step.to_string())]
                    );
                }
                std::cmp::Ordering::Equal => {
                    assert!(matches!(outcome.status, Status::Failed(_)), "{outcome:?}");
                }
                std::cmp::Ordering::Greater => {
                    assert_eq!(outcome.status, Status::NotRun);
                    assert_eq!(outcome.duration, Duration::ZERO);
                    assert!(outcome.facts.is_empty());
                }
            }
        }
    }

    #[test]
    fn a_format_failure_stops_the_check() {
        assert_fails_at("fmt");
    }

    #[test]
    fn a_clippy_failure_stops_the_check() {
        assert_fails_at("clippy");
    }

    #[test]
    fn a_test_failure_stops_the_check() {
        assert_fails_at("test");
    }

    #[test]
    fn a_docs_failure_stops_the_check() {
        assert_fails_at("docs");
    }

    #[test]
    fn a_commits_failure_stops_the_check() {
        assert_fails_at("commits");
    }

    #[test]
    fn a_workflows_failure_fails_the_check_after_every_other_step() {
        assert_fails_at("workflows");
    }

    #[test]
    fn a_failing_step_keeps_the_facts_it_added() {
        let ran = Rc::new(RefCell::new(Vec::new()));
        let outcomes = run_steps(Path::new("."), &with_failure("docs", &ran));
        assert_eq!(
            outcomes[3].facts,
            [("seen".to_string(), "before failing".to_string())]
        );
        assert_eq!(
            outcomes[3].status,
            Status::Failed("forced failure".to_string())
        );
    }
}
