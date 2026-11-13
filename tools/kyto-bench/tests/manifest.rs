//! Checks of the workspace manifests, the lock file and the Clippy settings:
//! every external dependency is an exact version from crates.io, declared
//! once in the workspace under a comment that explains its features, the
//! lock file holds nothing but crates.io packages and the workspace members,
//! and a member with Clippy settings of its own keeps every ban of the
//! workspace.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use toml::{Table, Value};

/// The source that Cargo writes in the lock file for crates.io.
const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

/// The keys a workspace dependency may have.
const DEPENDENCY_KEYS: [&str; 3] = ["version", "default-features", "features"];

fn workspace_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

fn parse(text: &str, path: &Path) -> Table {
    text.parse()
        .unwrap_or_else(|error| panic!("{} is not TOML: {error}", path.display()))
}

fn table<'a>(table: &'a Table, key: &str, context: &str) -> &'a Table {
    table
        .get(key)
        .and_then(Value::as_table)
        .unwrap_or_else(|| panic!("{context} has no table {key}"))
}

/// The workspace manifest, as text and parsed.
fn workspace_manifest() -> (String, Table) {
    let path = workspace_dir().join("Cargo.toml");
    let text = read(&path);
    let parsed = parse(&text, &path);
    (text, parsed)
}

/// Each member's directory and parsed manifest.
fn members(workspace: &Table) -> Vec<(String, Table)> {
    let members = table(workspace, "workspace", "the workspace manifest")
        .get("members")
        .and_then(Value::as_array)
        .expect("the workspace lists its members");
    assert!(!members.is_empty(), "the workspace has members");
    members
        .iter()
        .map(|member| {
            let dir = member.as_str().expect("a member is a directory name");
            let path = workspace_dir().join(dir).join("Cargo.toml");
            (dir.to_string(), parse(&read(&path), &path))
        })
        .collect()
}

fn package_name(manifest: &Table) -> &str {
    table(manifest, "package", "a member manifest")
        .get("name")
        .and_then(Value::as_str)
        .expect("a member names its package")
}

/// An exact version requirement: `=` and three decimal numbers.
fn exact_version(requirement: &str) -> Option<&str> {
    let version = requirement.strip_prefix('=')?;
    let parts: Vec<&str> = version.split('.').collect();
    let exact = parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));
    exact.then_some(version)
}

/// The keys declared in the `[workspace.dependencies]` section of the
/// manifest text, each with whether the line above it is a comment.
fn declared_dependencies(text: &str) -> Vec<(String, bool)> {
    let mut declared = Vec::new();
    let mut in_section = false;
    let mut previous = "";
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = trimmed == "[workspace.dependencies]";
        } else if in_section && !trimmed.is_empty() && !trimmed.starts_with('#') {
            let key = trimmed
                .split(['=', ' ', '.'])
                .next()
                .unwrap_or_default()
                .to_string();
            declared.push((key, previous.trim().starts_with('#')));
        }
        previous = line;
    }
    declared
}

#[test]
fn dependencies_are_commented_exact_crates_io_pins() {
    let (text, workspace) = workspace_manifest();
    let pins = table(
        table(&workspace, "workspace", "the workspace manifest"),
        "dependencies",
        "[workspace]",
    );
    assert!(!pins.is_empty(), "the workspace declares its dependencies");

    for (name, spec) in pins {
        let spec = spec
            .as_table()
            .unwrap_or_else(|| panic!("{name} is a table with version and features"));
        for key in spec.keys() {
            assert!(
                DEPENDENCY_KEYS.contains(&key.as_str()),
                "{name} has the key {key}; a dependency comes from crates.io and has only {DEPENDENCY_KEYS:?}"
            );
        }
        let version = spec
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{name} has a version"));
        assert!(
            exact_version(version).is_some(),
            "{name} has the version {version}, not an exact =x.y.z"
        );
        assert_eq!(
            spec.get("default-features").and_then(Value::as_bool),
            Some(false),
            "{name} sets default-features = false"
        );
        if let Some(features) = spec.get("features") {
            let features = features
                .as_array()
                .unwrap_or_else(|| panic!("the features of {name} are a list"));
            assert!(
                features.iter().all(Value::is_str),
                "the features of {name} are names"
            );
        }
    }

    let declared = declared_dependencies(&text);
    let declared_names: BTreeSet<&str> = declared.iter().map(|(name, _)| name.as_str()).collect();
    let pinned_names: BTreeSet<&str> = pins.keys().map(String::as_str).collect();
    assert_eq!(
        declared_names, pinned_names,
        "each dependency is one line of [workspace.dependencies]"
    );
    assert_eq!(
        declared.len(),
        pins.len(),
        "each dependency is declared once"
    );
    for (name, commented) in &declared {
        assert!(
            commented,
            "the line above {name} is a comment on its features"
        );
    }

    let members = members(&workspace);
    let member_dirs: BTreeSet<&str> = members.iter().map(|(dir, _)| dir.as_str()).collect();
    for (dir, manifest) in &members {
        for kind in ["dev-dependencies", "build-dependencies", "target"] {
            assert!(!manifest.contains_key(kind), "{dir} has no {kind} table");
        }
        assert_eq!(
            table(manifest, "lints", dir).get("workspace"),
            Some(&Value::Boolean(true)),
            "{dir} uses the workspace lints"
        );
        let Some(dependencies) = manifest.get("dependencies") else {
            continue;
        };
        let dependencies = dependencies.as_table().expect("dependencies are a table");
        for (name, spec) in dependencies {
            let spec = spec
                .as_table()
                .unwrap_or_else(|| panic!("{dir}: {name} is a table"));
            let keys: Vec<&str> = spec.keys().map(String::as_str).collect();
            match keys.as_slice() {
                ["workspace"] => {
                    assert_eq!(spec["workspace"], Value::Boolean(true), "{dir}: {name}");
                    assert!(
                        pins.contains_key(name),
                        "{dir}: {name} is declared in the workspace"
                    );
                }
                ["path"] => {
                    let path = spec["path"].as_str().expect("a path is a string");
                    let target = path
                        .strip_prefix("../")
                        .unwrap_or_else(|| panic!("{dir}: {name} is a sibling member"));
                    assert!(member_dirs.contains(target), "{dir}: {path} is a member");
                    let (_, member) = members
                        .iter()
                        .find(|(dir, _)| dir == target)
                        .expect("the member exists");
                    assert_eq!(package_name(member), name, "{dir}: {path} is {name}");
                }
                _ => panic!(
                    "{dir}: {name} has the keys {keys:?}; a member names an external dependency with workspace = true only, and a member with path only"
                ),
            }
        }
    }
}

#[test]
fn lock_file_has_only_crates_io_and_members() {
    let path = workspace_dir().join("Cargo.lock");
    let lock = parse(&read(&path), &path);
    assert_eq!(
        lock.get("version").and_then(Value::as_integer),
        Some(4),
        "the lock file has format version 4"
    );
    let packages = lock
        .get("package")
        .and_then(Value::as_array)
        .expect("the lock file lists packages");

    let (_, workspace) = workspace_manifest();
    let members = members(&workspace);
    let member_names: BTreeSet<&str> = members
        .iter()
        .map(|(_, manifest)| package_name(manifest))
        .collect();

    let mut locked_members = BTreeSet::new();
    for package in packages {
        let package = package.as_table().expect("a package is a table");
        let name = package
            .get("name")
            .and_then(Value::as_str)
            .expect("a package has a name");
        match package.get("source") {
            Some(source) => {
                assert_eq!(
                    source.as_str(),
                    Some(CRATES_IO),
                    "{name} comes from crates.io"
                );
                let checksum = package
                    .get("checksum")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("{name} has a checksum"));
                assert!(
                    checksum.len() == 64
                        && checksum
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                    "{name} has a SHA-256 checksum"
                );
            }
            None => {
                assert!(
                    member_names.contains(name),
                    "{name} has no source, so it must be a workspace member"
                );
                locked_members.insert(name);
            }
        }
    }
    assert_eq!(
        locked_members, member_names,
        "every member is in the lock file"
    );

    // Each pin is a version in the lock file, without build metadata.
    let pins = table(
        table(&workspace, "workspace", "the workspace manifest"),
        "dependencies",
        "[workspace]",
    );
    for (name, spec) in pins {
        let pinned = spec
            .get("version")
            .and_then(Value::as_str)
            .and_then(exact_version)
            .unwrap_or_else(|| panic!("{name} is pinned"));
        let locked: Vec<&str> = packages
            .iter()
            .filter_map(Value::as_table)
            .filter(|package| package.get("name").and_then(Value::as_str) == Some(name))
            .filter_map(|package| package.get("version").and_then(Value::as_str))
            .map(|version| version.split('+').next().unwrap_or(version))
            .collect();
        assert!(
            locked.contains(&pinned),
            "{name} is locked at {pinned}, not {locked:?}"
        );
    }
}

#[test]
fn member_clippy_settings_keep_every_workspace_ban() {
    let path = workspace_dir().join("clippy.toml");
    let workspace = parse(&read(&path), &path);
    let lists = ["disallowed-methods", "disallowed-types"];
    for list in lists {
        let entries = workspace.get(list).and_then(Value::as_array);
        assert!(
            entries.is_some_and(|entries| !entries.is_empty()),
            "the workspace file has {list}"
        );
    }
    let (_, manifest) = workspace_manifest();
    let mut checked = 0;
    for (dir, _) in members(&manifest) {
        // Clippy reads the nearest `clippy.toml`, so a member without one
        // reads the workspace file.
        let path = workspace_dir().join(&dir).join("clippy.toml");
        if !path.exists() {
            continue;
        }
        checked += 1;
        let own = parse(&read(&path), &path);
        for list in lists {
            let own_entries = own
                .get(list)
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("{dir}/clippy.toml has {list}"));
            for entry in workspace[list].as_array().into_iter().flatten() {
                assert!(
                    own_entries.contains(entry),
                    "{dir}/clippy.toml keeps the {list} entry {entry:?}"
                );
            }
        }
    }
    assert!(checked > 0, "the core has Clippy settings of its own");
}
