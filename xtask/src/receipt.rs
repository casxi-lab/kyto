//! The receipt of `cargo xtask check --receipt <path>`: a JSON record of one
//! run, written whether the check passes or fails.
//!
//! The receipt holds the format version, the command, the commit checked out
//! when the run started, whether the working tree was clean before and after
//! the run, the result, each step with its name, status, duration, failure
//! detail and facts, the version of rustc, the runner image and the GitHub
//! Actions run. A value that cannot be read, or an environment variable that
//! is not set or is empty, is `null`. The text is the pretty form of
//! `serde_json`, with two-space indentation and the fields in the order in
//! which they are written, and ends in a newline.

use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;

use serde_json::{Map, Value, json};

use crate::check::{Outcome, Status};

/// The version of the receipt format.
pub const FORMAT: u32 = 1;

/// The variables that name the runner image. GitHub-hosted runners set them
/// in `/etc/environment`, which is read when the process environment lacks
/// them.
pub const IMAGE: [&str; 2] = ["ImageOS", "ImageVersion"];

/// The variables that identify a GitHub Actions run.
pub const GITHUB: [&str; 5] = [
    "GITHUB_RUN_ID",
    "GITHUB_RUN_ATTEMPT",
    "GITHUB_EVENT_NAME",
    "GITHUB_REF",
    "GITHUB_SHA",
];

/// The checked-out commit and whether the working tree is clean: no change
/// to a tracked file and no untracked file outside the ignored paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitState {
    pub commit: Option<String>,
    pub clean: Option<bool>,
}

impl GitState {
    /// Reads the state of the working copy at `root`.
    pub fn read(root: &Path) -> GitState {
        let git = |args: &[&str]| output_of(root, "git", args);
        let commit = git(&["rev-parse", "HEAD"]).map(|out| out.trim().to_string());
        let clean = git(&["status", "--porcelain", "--untracked-files=all"])
            .map(|out| out.trim().is_empty());
        GitState { commit, clean }
    }
}

/// The tool versions of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tools {
    /// The first line of `rustc --version`.
    pub rustc: Option<String>,
}

impl Tools {
    /// Runs `rustc --version` in `root`, where rustup selects the toolchain
    /// of `rust-toolchain.toml`.
    pub fn read(root: &Path) -> Tools {
        let rustc = output_of(root, "rustc", &["--version"]).and_then(|out| first_line(&out));
        Tools { rustc }
    }
}

/// The first line of `output` without surrounding white space, unless it is
/// empty.
fn first_line(output: &str) -> Option<String> {
    let line = output.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_string())
}

/// The standard output of `program` run in `root`, if it exits with status 0
/// and prints UTF-8.
fn output_of(root: &Path, program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    if output.status.success() {
        String::from_utf8(output.stdout).ok()
    } else {
        None
    }
}

/// The values that the receipt takes from the environment, in the order of
/// [`IMAGE`] and [`GITHUB`]; `None` where a variable is not set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    pub image: Vec<(&'static str, Option<String>)>,
    pub github: Vec<(&'static str, Option<String>)>,
}

impl Environment {
    /// Reads the process environment and `/etc/environment`.
    pub fn read() -> Environment {
        let etc_environment = fs::read_to_string("/etc/environment").ok();
        Environment::from_sources(|name| std::env::var(name).ok(), etc_environment.as_deref())
    }

    /// Takes each variable from `var`, and an image variable that `var`
    /// lacks from the `NAME=value` lines of `etc_environment`. An empty value
    /// counts as unset.
    pub fn from_sources(
        var: impl Fn(&str) -> Option<String>,
        etc_environment: Option<&str>,
    ) -> Self {
        let set = |name: &str| var(name).filter(|value| !value.is_empty());
        let image = IMAGE
            .iter()
            .map(|name| {
                let value =
                    set(name).or_else(|| etc_environment.and_then(|text| assignment(text, name)));
                (*name, value)
            })
            .collect();
        let github = GITHUB.iter().map(|name| (*name, set(name))).collect();
        Environment { image, github }
    }
}

/// The value of the last assignment to `name` in `NAME=value` lines, with
/// one pair of surrounding quotes removed.
fn assignment(text: &str, name: &str) -> Option<String> {
    let value = text
        .lines()
        .filter_map(|line| line.trim().split_once('='))
        .filter(|(key, _)| key.trim() == name)
        .map(|(_, value)| value.trim())
        .next_back()?;
    let unquoted = ['"', '\'']
        .into_iter()
        .find_map(|quote| {
            value
                .strip_prefix(quote)
                .and_then(|rest| rest.strip_suffix(quote))
        })
        .unwrap_or(value);
    (!unquoted.is_empty()).then(|| unquoted.to_string())
}

/// An object of the variables `values`, each a string or `null`, in their
/// order.
fn variables(values: &[(&'static str, Option<String>)]) -> Value {
    Value::Object(
        values
            .iter()
            .map(|(name, value)| (name.to_string(), Value::from(value.clone())))
            .collect(),
    )
}

/// The entry of one step: its name, status and duration in whole
/// milliseconds, why it failed if it did, and its facts in the order in
/// which the step observed them.
fn step(outcome: &Outcome) -> Value {
    let status = match outcome.status {
        Status::Passed => "passed",
        Status::Failed(_) => "failed",
        Status::NotRun => "not run",
    };
    let duration_ms = u64::try_from(outcome.duration.as_millis()).unwrap_or(u64::MAX);
    let mut step = json!({
        "name": outcome.step,
        "status": status,
        "duration_ms": duration_ms,
    });
    if let Status::Failed(detail) = &outcome.status {
        step["detail"] = Value::from(detail.as_str());
    }
    let facts: Map<String, Value> = outcome
        .facts
        .iter()
        .map(|(key, value)| (key.clone(), Value::from(value.as_str())))
        .collect();
    step["facts"] = Value::Object(facts);
    step
}

/// The receipt of a run, as JSON text ending in a newline.
pub fn render(
    before: &GitState,
    after: &GitState,
    outcomes: &[Outcome],
    tools: &Tools,
    environment: &Environment,
) -> String {
    let passed = outcomes
        .iter()
        .all(|outcome| outcome.status == Status::Passed);
    let steps: Vec<Value> = outcomes.iter().map(step).collect();
    let receipt = json!({
        "format": FORMAT,
        "command": "cargo xtask check",
        "commit": before.commit,
        "clean_before": before.clean,
        "clean_after": after.clean,
        "result": if passed { "passed" } else { "failed" },
        "steps": steps,
        "tools": {
            "rustc": tools.rustc,
        },
        "image": variables(&environment.image),
        "github": variables(&environment.github),
    });
    format!("{receipt:#}\n")
}

/// Writes `text` to `path`, creating the directories above it.
pub fn write(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn reads_image_variables_from_etc_environment() {
        let etc = "PATH=\"/usr/bin\"\nImageVersion=\"20261026.166.1\"\nImageOS=ubuntu24-arm64\n# ImageOS=commented=out\n";
        let environment = Environment::from_sources(|_| None, Some(etc));
        assert_eq!(
            environment.image,
            [
                ("ImageOS", Some("ubuntu24-arm64".to_string())),
                ("ImageVersion", Some("20261026.166.1".to_string())),
            ]
        );
        assert!(environment.github.iter().all(|(_, value)| value.is_none()));
    }

    #[test]
    fn the_process_environment_comes_first() {
        let var = |name: &str| match name {
            "ImageOS" => Some("from-env".to_string()),
            "ImageVersion" => Some(String::new()),
            "GITHUB_RUN_ID" => Some("42".to_string()),
            "GITHUB_REF" => Some("refs/heads/main".to_string()),
            _ => None,
        };
        let environment =
            Environment::from_sources(var, Some("ImageOS=from-file\nImageVersion='1.2'\n"));
        assert_eq!(
            environment.image,
            [
                ("ImageOS", Some("from-env".to_string())),
                ("ImageVersion", Some("1.2".to_string())),
            ]
        );
        assert_eq!(
            environment.github,
            [
                ("GITHUB_RUN_ID", Some("42".to_string())),
                ("GITHUB_RUN_ATTEMPT", None),
                ("GITHUB_EVENT_NAME", None),
                ("GITHUB_REF", Some("refs/heads/main".to_string())),
                ("GITHUB_SHA", None),
            ]
        );
        assert_eq!(
            Environment::from_sources(|_| None, None).image[0],
            ("ImageOS", None)
        );
    }

    #[test]
    fn renders_every_field() {
        let before = GitState {
            commit: Some("0123abc".to_string()),
            clean: Some(true),
        };
        let after = GitState {
            commit: Some("0123abc".to_string()),
            clean: None,
        };
        let outcomes = [
            Outcome {
                step: "docs",
                status: Status::Passed,
                duration: Duration::from_millis(1250),
                facts: Vec::new(),
            },
            Outcome {
                step: "commits",
                status: Status::Failed(
                    "commit 0123abc45678: header `Add \"x\"` is wrong\nsee above".to_string(),
                ),
                duration: Duration::from_micros(7999),
                facts: vec![
                    ("base_from".to_string(), "CHECK_BASE".to_string()),
                    ("base".to_string(), "fedcba9".to_string()),
                ],
            },
            Outcome {
                step: "workflows",
                status: Status::NotRun,
                duration: Duration::ZERO,
                facts: Vec::new(),
            },
        ];
        let tools = Tools {
            rustc: Some("rustc 1.99.0 (0123abcde 2026-10-01)".to_string()),
        };
        let environment = Environment::from_sources(
            |name| (name == "GITHUB_EVENT_NAME").then(|| "pull_request".to_string()),
            Some("ImageOS=ubuntu24-arm64\n"),
        );
        let expected = r#"{
  "format": 1,
  "command": "cargo xtask check",
  "commit": "0123abc",
  "clean_before": true,
  "clean_after": null,
  "result": "failed",
  "steps": [
    {
      "name": "docs",
      "status": "passed",
      "duration_ms": 1250,
      "facts": {}
    },
    {
      "name": "commits",
      "status": "failed",
      "duration_ms": 7,
      "detail": "commit 0123abc45678: header `Add \"x\"` is wrong\nsee above",
      "facts": {
        "base_from": "CHECK_BASE",
        "base": "fedcba9"
      }
    },
    {
      "name": "workflows",
      "status": "not run",
      "duration_ms": 0,
      "facts": {}
    }
  ],
  "tools": {
    "rustc": "rustc 1.99.0 (0123abcde 2026-10-01)"
  },
  "image": {
    "ImageOS": "ubuntu24-arm64",
    "ImageVersion": null
  },
  "github": {
    "GITHUB_RUN_ID": null,
    "GITHUB_RUN_ATTEMPT": null,
    "GITHUB_EVENT_NAME": "pull_request",
    "GITHUB_REF": null,
    "GITHUB_SHA": null
  }
}
"#;
        assert_eq!(
            render(&before, &after, &outcomes, &tools, &environment),
            expected
        );
    }

    /// The receipt text read back as JSON.
    fn parsed(text: &str) -> Value {
        serde_json::from_str(text).expect("the receipt is JSON")
    }

    #[test]
    fn a_run_with_every_step_passed_is_passed() {
        let state = GitState {
            commit: None,
            clean: Some(false),
        };
        let outcomes = [Outcome {
            step: "docs",
            status: Status::Passed,
            duration: Duration::ZERO,
            facts: Vec::new(),
        }];
        let tools = Tools { rustc: None };
        let environment = Environment::from_sources(|_| None, None);
        let receipt = parsed(&render(&state, &state, &outcomes, &tools, &environment));
        assert_eq!(receipt["commit"], Value::Null);
        assert_eq!(receipt["clean_before"], false);
        assert_eq!(receipt["result"], "passed");
        assert_eq!(receipt["tools"], json!({ "rustc": null }));
        let empty = parsed(&render(&state, &state, &[], &tools, &environment));
        assert_eq!(empty["steps"], json!([]));
        assert_eq!(empty["result"], "passed");
    }

    #[test]
    fn strings_with_json_specials_read_back_unchanged() {
        let text = "a \"b\" \\c\none\r\ttwo \u{0}\u{8}\u{c}\u{1b}\u{7f} casξ ✓";
        let state = GitState {
            commit: Some(text.to_string()),
            clean: None,
        };
        let outcomes = [Outcome {
            step: "docs",
            status: Status::Failed(text.to_string()),
            duration: Duration::from_millis(3),
            facts: vec![(text.to_string(), text.to_string())],
        }];
        let tools = Tools {
            rustc: Some(text.to_string()),
        };
        let environment = Environment::from_sources(
            |name| (name == "GITHUB_REF").then(|| text.to_string()),
            None,
        );
        // The reader rejects a control character that is not escaped inside
        // a string, so reading the receipt back also checks the escapes.
        let receipt = parsed(&render(&state, &state, &outcomes, &tools, &environment));
        assert_eq!(receipt["commit"], text);
        assert_eq!(receipt["steps"][0]["detail"], text);
        assert_eq!(receipt["steps"][0]["facts"][text], text);
        assert_eq!(receipt["tools"]["rustc"], text);
        assert_eq!(receipt["github"]["GITHUB_REF"], text);
    }

    #[test]
    fn keeps_the_first_line_of_version_output() {
        assert_eq!(
            first_line("rustc 1.99.0 (0123abcde 2026-10-01)\nbinary: rustc\n").as_deref(),
            Some("rustc 1.99.0 (0123abcde 2026-10-01)")
        );
        assert_eq!(first_line("  \n"), None);
        assert_eq!(first_line(""), None);
    }

    #[test]
    fn reads_the_state_and_rustc_of_this_working_copy() {
        let root = crate::repo::root();
        let state = GitState::read(&root);
        assert!(
            state.commit.is_some_and(
                |commit| commit.len() >= 40 && commit.bytes().all(|b| b.is_ascii_hexdigit())
            ),
            "the working copy has a commit"
        );
        assert!(state.clean.is_some());
        let rustc = Tools::read(&root).rustc.expect("rustc runs");
        assert!(rustc.starts_with("rustc "), "{rustc}");
    }

    #[test]
    fn writes_into_a_new_directory() {
        // The build directory of this crate is ignored by Git.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("receipt-test-{}", std::process::id()));
        let path = dir.join("nested/receipt.json");
        write(&path, "{}\n").expect("the receipt is written");
        assert_eq!(fs::read_to_string(&path).unwrap(), "{}\n");
        fs::remove_dir_all(&dir).unwrap();
    }
}
