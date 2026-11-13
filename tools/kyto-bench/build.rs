//! Embeds the revision of the tool: the commit it is built from as
//! `KYTO_BENCH_COMMIT` (40 lowercase hexadecimal digits) and whether the
//! working tree was clean, untracked files included, as `KYTO_BENCH_CLEAN`
//! (`true` or `false`). A value that Git cannot give is left out, and the
//! tool reads it as unknown.
//!
//! Git runs in the repository root with the variables that point it at
//! another repository or index removed, and the values are given only when
//! the top level of the work tree that Git finds is that root, so a source
//! tree without its own `.git` never takes the commit of a checkout around
//! it.
//!
//! The script runs again when `HEAD`, the index, the references or a tracked
//! file changes; only paths that exist are watched. A file that is created
//! after the last build and is not tracked does not make it run again, so an
//! incremental build can call such a tree clean; a build from a fresh
//! checkout cannot.

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
    let Some(manifest_dir) = std::env::var_os("CARGO_MANIFEST_DIR") else {
        return;
    };
    let Ok(root) = Path::new(&manifest_dir).join("../..").canonicalize() else {
        return;
    };
    let text = |out: Vec<u8>| {
        String::from_utf8(out)
            .ok()
            .map(|out| out.trim_end().to_string())
    };
    let top_level = git(&root, "rev-parse --show-toplevel").and_then(text);
    if top_level.and_then(|top| PathBuf::from(top).canonicalize().ok()) != Some(root.clone()) {
        return;
    }

    let commit = git(&root, "rev-parse --verify HEAD").and_then(text);
    if let Some(commit) = commit.filter(|commit| is_commit(commit)) {
        println!("cargo::rustc-env=KYTO_BENCH_COMMIT={commit}");
    }

    // `--no-optional-locks` keeps `git status` from refreshing the index,
    // which would change the index and make this script run on every build.
    let status = git(
        &root,
        "--no-optional-locks status --porcelain --untracked-files=all",
    );
    if let Some(status) = &status {
        println!("cargo::rustc-env=KYTO_BENCH_CLEAN={}", status.is_empty());
    }

    // The paths come from Git, so a linked worktree, where `.git` is a file,
    // watches its own `HEAD` and index and the shared references.
    let git_paths = git(
        &root,
        "rev-parse --git-path HEAD --git-path index --git-path packed-refs --git-path refs \
         --git-path reftable",
    );
    let tracked = git(&root, "ls-files -z");
    if let (Some(git_paths), Some(tracked)) = (git_paths, tracked) {
        for path in String::from_utf8_lossy(&git_paths).lines() {
            watch(&root.join(path));
        }
        for path in tracked
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
        {
            if let Ok(path) = std::str::from_utf8(path) {
                watch(&root.join(path));
            }
        }
    }
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

/// Makes Cargo run the script again when `path` changes. A path that does
/// not exist is skipped, since Cargo would run the script on every build for
/// it, and so is one that does not fit on one line of the script's output.
fn watch(path: &Path) {
    if !path.exists() {
        return;
    }
    if let Some(path) = path.to_str().filter(|path| !path.contains(['\n', '\r'])) {
        println!("cargo::rerun-if-changed={path}");
    }
}
