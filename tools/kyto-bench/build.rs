//! Embeds the revision of the tool: the commit it is built from as
//! `KYTO_BENCH_COMMIT` (40 lowercase hexadecimal digits) and whether the
//! working tree was clean, untracked files included, as `KYTO_BENCH_CLEAN`
//! (`true` or `false`). Both are given on every build; a value that Git
//! cannot give is given empty, and the tool reads it as unknown, so a value
//! of the same name in the build's environment never reaches the tool.
//!
//! Git runs in the repository root with the variables that point it at
//! another repository or index removed, and its values are used only when
//! the top level of the work tree that Git finds is that root, so a source
//! tree without its own `.git` never takes the commit of a checkout around
//! it.
//!
//! The script runs on every build of the package, so the values always
//! describe the tree as it was at the build that made the binary: an
//! untracked file created after an earlier build makes the next build say
//! that the tree has changes. Cargo runs the script on every build because
//! the script watches a path in the build's output directory that nothing
//! creates, and it then compiles the package again as well.

// The build script is the one part of the tool that runs an external
// program: Git, to read the commit and the state of the working tree.
#![allow(clippy::disallowed_types)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// The variables that would make Git read another repository, work tree or
/// index than the one around the root.
const GIT_LOCATION_VARIABLES: [&str; 5] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
];

fn main() {
    // Cargo runs the script again when a path it watches does not exist.
    if let Some(out) = std::env::var_os("OUT_DIR") {
        let never = Path::new(&out).join("never-created");
        println!("cargo::rerun-if-changed={}", never.display());
    }
    // Both values are given on every path; an empty one is unknown.
    let (commit, clean) = revision().unwrap_or_default();
    let (commit, clean) = (commit.unwrap_or_default(), clean.map(|c| c.to_string()));
    let clean = clean.unwrap_or_default();
    println!("cargo::rustc-env=KYTO_BENCH_COMMIT={commit}");
    println!("cargo::rustc-env=KYTO_BENCH_CLEAN={clean}");
}

/// The commit of the repository root and whether its working tree is clean,
/// each `None` when Git cannot give it, or `None` when the root is not the
/// top level of the work tree that Git finds.
fn revision() -> Option<(Option<String>, Option<bool>)> {
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR")?;
    let root = Path::new(&manifest_dir).join("../..").canonicalize().ok()?;
    let text = |out: Vec<u8>| {
        String::from_utf8(out)
            .ok()
            .map(|out| out.trim_end().to_string())
    };
    let top_level = git(&root, "rev-parse --show-toplevel").and_then(text);
    if top_level.and_then(|top| PathBuf::from(top).canonicalize().ok()) != Some(root.clone()) {
        return None;
    }
    let commit = git(&root, "rev-parse --verify HEAD").and_then(text);
    // `--no-optional-locks` keeps `git status` from refreshing the index, so
    // that a build writes nothing in the repository.
    let status = git(
        &root,
        "--no-optional-locks status --porcelain --untracked-files=all",
    );
    let clean = status.map(|status| status.is_empty());
    Some((commit.filter(|commit| is_commit(commit)), clean))
}

/// Runs Git in `root` with `args`, separated by spaces, without the
/// variables that point it elsewhere, and returns its standard output when
/// it exits with status 0.
fn git(root: &Path, args: &str) -> Option<Vec<u8>> {
    let mut command = Command::new("git");
    command.args(args.split(' ')).current_dir(root);
    for variable in GIT_LOCATION_VARIABLES {
        command.env_remove(variable);
    }
    let output = command.output().ok()?;
    output.status.success().then_some(output.stdout)
}

fn is_commit(text: &str) -> bool {
    text.len() == 40
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}
