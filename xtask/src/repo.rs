//! The repository as Git tracks it.
//!
//! The docs and workflows steps read tracked files only, so a fresh clone and
//! a working copy with build output or ignored files give the same result.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The root of the repository: the parent of the xtask crate.
pub fn root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .expect("the xtask crate is inside the repository")
        .to_path_buf()
}

/// A tracked entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A regular file and its content.
    File(Vec<u8>),
    /// A symbolic link.
    Symlink,
    /// A submodule.
    Submodule,
}

/// The tracked entries, keyed by repository-relative paths with `/`
/// separators.
#[derive(Debug, Default, Clone)]
pub struct Tree {
    entries: BTreeMap<String, Entry>,
}

impl Tree {
    /// Reads the tracked entries of the working copy at `root`. The content
    /// of a regular file is read from the working tree.
    pub fn load(root: &Path) -> Result<Tree, String> {
        let output = Command::new("git")
            .args(["ls-files", "--stage", "-z"])
            .current_dir(root)
            .output()
            .map_err(|error| format!("cannot run git ls-files: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "git ls-files failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let mut tree = Tree::default();
        for record in output.stdout.split(|&byte| byte == 0) {
            if record.is_empty() {
                continue;
            }
            let record = std::str::from_utf8(record)
                .map_err(|_| "git ls-files printed a path that is not UTF-8".to_string())?;
            let (meta, path) = record
                .split_once('\t')
                .ok_or_else(|| format!("unexpected git ls-files record `{record}`"))?;
            let mode = meta.split(' ').next().unwrap_or_default();
            let entry = match mode {
                "120000" => Entry::Symlink,
                "160000" => Entry::Submodule,
                _ => Entry::File(
                    fs::read(root.join(path))
                        .map_err(|error| format!("cannot read {path}: {error}"))?,
                ),
            };
            tree.entries.insert(path.to_string(), entry);
        }
        Ok(tree)
    }

    /// Builds a tree from in-memory files, for tests.
    #[cfg(test)]
    pub fn from_files(files: &[(&str, &str)]) -> Tree {
        let mut tree = Tree::default();
        for (path, content) in files {
            tree.entries
                .insert(path.to_string(), Entry::File(content.as_bytes().to_vec()));
        }
        tree
    }

    /// All entries in path order.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &Entry)> {
        self.entries
            .iter()
            .map(|(path, entry)| (path.as_str(), entry))
    }

    /// The paths of tracked regular files whose content is UTF-8, with the
    /// content.
    pub fn texts(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().filter_map(|(path, entry)| match entry {
            Entry::File(bytes) => std::str::from_utf8(bytes)
                .ok()
                .map(|text| (path.as_str(), text)),
            _ => None,
        })
    }

    /// The content of a tracked UTF-8 file.
    pub fn text(&self, path: &str) -> Option<&str> {
        match self.entries.get(path) {
            Some(Entry::File(bytes)) => std::str::from_utf8(bytes).ok(),
            _ => None,
        }
    }

    /// Whether `path` is a tracked entry or a directory that holds one. The
    /// empty path is the repository root.
    pub fn exists(&self, path: &str) -> bool {
        if path.is_empty() || self.entries.contains_key(path) {
            return true;
        }
        let prefix = format!("{path}/");
        self.entries
            .range(prefix.clone()..)
            .next()
            .is_some_and(|(key, _)| key.starts_with(&prefix))
    }
}

/// The directory part of a repository-relative path; empty at the root.
pub fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// Joins `relative` to the repository-relative directory `base` without
/// touching the file system. Returns `None` when the result leaves the
/// repository or `relative` is absolute.
pub fn join(base: &str, relative: &str) -> Option<String> {
    if relative.starts_with('/') {
        return None;
    }
    let mut parts: Vec<&str> = base.split('/').filter(|part| !part.is_empty()).collect();
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_resolves_relative_paths() {
        assert_eq!(
            join("docs/features", "../../README.md").as_deref(),
            Some("README.md")
        );
        assert_eq!(join("docs", "./a.md").as_deref(), Some("docs/a.md"));
        assert_eq!(join("", "../outside"), None);
        assert_eq!(join("docs", "/etc"), None);
    }

    #[test]
    fn parent_is_the_directory_part() {
        assert_eq!(parent("docs/scope.md"), "docs");
        assert_eq!(parent("README.md"), "");
    }

    #[test]
    fn exists_sees_files_and_their_directories() {
        let tree = Tree::from_files(&[("docs/a/b.md", "")]);
        assert!(tree.exists("docs/a/b.md"));
        assert!(tree.exists("docs/a"));
        assert!(tree.exists("docs"));
        assert!(tree.exists(""));
        assert!(!tree.exists("doc"));
        assert!(!tree.exists("docs/a/c.md"));
    }

    #[test]
    fn loads_the_tracked_files_of_this_repository() {
        let root = root();
        let tree = Tree::load(&root).expect("the repository is a Git working copy");
        let readme = fs::read_to_string(root.join("README.md")).unwrap();
        assert_eq!(tree.text("README.md"), Some(readme.as_str()));
        assert!(!tree.exists("xtask/target"));
    }
}
