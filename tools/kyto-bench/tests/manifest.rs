//! Checks of the workspace manifests, the lock file, the lints, the Clippy
//! settings and the copied code: every external dependency is an exact
//! version from crates.io, declared once in the workspace under a comment
//! that explains its features, the lock file holds nothing but crates.io
//! packages and the workspace members, a member with Clippy settings of its
//! own keeps every ban of the workspace, only `kyto-bench-os` has lints of
//! its own and depends on `libc`, the bans are lifted only where they are
//! listed, and the code copied from krepi has the digests that `PORTING.md`
//! records.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use kyto_bench_core::digest::Sha256;
use toml::{Table, Value};

/// The source that Cargo writes in the lock file for crates.io.
const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

/// The keys a workspace dependency may have.
const DEPENDENCY_KEYS: [&str; 3] = ["version", "default-features", "features"];

/// The one member with lints of its own, which may use unsafe code and
/// depends on `libc`.
const UNSAFE_MEMBER: &str = "kyto-bench-os";

/// The lints that lift a ban of the workspace when they are allowed: unsafe
/// code, and the bans of `clippy.toml`.
const ESCAPES: [&str; 3] = [
    "unsafe_code",
    "clippy::disallowed_methods",
    "clippy::disallowed_types",
];

/// The files apart from the crate root of `kyto-bench-os` that allow a lint
/// of [`ESCAPES`], from the workspace directory, each with the lints of its
/// one attribute that allows lints.
const OTHER_ESCAPES: [(&str, &[&str]); 2] = [
    ("kyto-bench/build.rs", &["clippy::disallowed_types"]),
    ("kyto-bench/tests/release.rs", &["clippy::disallowed_types"]),
];

/// The modules that the crate root of `kyto-bench-os` declares, each with
/// the lints that the attributes on its declaration allow and whether one
/// of them forbids unsafe code.
const OS_MODULES: [(&str, &[&str], bool); 9] = [
    ("clock", &["clippy::disallowed_methods"], true),
    ("command_port", &[], false),
    ("console", &["clippy::disallowed_methods"], true),
    ("report", &["clippy::disallowed_methods"], true),
    ("timing", &[], true),
    ("tty", &["clippy::disallowed_methods"], false),
    (
        "child",
        &[
            "clippy::disallowed_methods",
            "clippy::disallowed_types",
            "dead_code",
        ],
        false,
    ),
    ("pty", &["clippy::disallowed_methods", "dead_code"], false),
    (
        "scratch",
        &[
            "clippy::disallowed_methods",
            "clippy::disallowed_types",
            "dead_code",
        ],
        true,
    ),
];

/// The line that ends a span of lines copied from krepi.
const SPAN_END: &str = "// End of the lines from krepi.";

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

#[test]
fn only_the_os_member_has_lints_of_its_own_and_libc() {
    let (_, workspace) = workspace_manifest();
    let lints = table(
        table(&workspace, "workspace", "the workspace manifest"),
        "lints",
        "[workspace]",
    );
    // krepi's lints, and every lint of the workspace but `unsafe_code`.
    let mut expected = lints.clone();
    let mut group = |name: &str| -> Table {
        let group = expected.remove(name).unwrap_or(Value::Table(Table::new()));
        group
            .as_table()
            .cloned()
            .expect("a group of lints is a table")
    };
    let (mut rust, mut clippy) = (group("rust"), group("clippy"));
    assert_eq!(
        rust.remove("unsafe_code"),
        Some(Value::from("forbid")),
        "the workspace forbids unsafe code"
    );
    rust.insert("unsafe_op_in_unsafe_fn".into(), Value::from("deny"));
    clippy.insert("undocumented_unsafe_blocks".into(), Value::from("deny"));
    expected.insert("rust".into(), Value::Table(rust));
    expected.insert("clippy".into(), Value::Table(clippy));

    let members = members(&workspace);
    assert!(members.iter().any(|(dir, _)| dir == UNSAFE_MEMBER));
    for (dir, manifest) in &members {
        let lints = table(manifest, "lints", dir);
        if dir == UNSAFE_MEMBER {
            assert_eq!(
                lints, &expected,
                "{dir} has krepi's lints and the workspace's"
            );
        } else {
            assert_eq!(
                lints.get("workspace"),
                Some(&Value::Boolean(true)),
                "{dir} uses the workspace lints"
            );
        }
        let dependencies = manifest.get("dependencies").and_then(Value::as_table);
        let libc = dependencies.is_some_and(|dependencies| dependencies.contains_key("libc"));
        assert_eq!(
            libc,
            dir == UNSAFE_MEMBER,
            "{dir}: only {UNSAFE_MEMBER} uses libc"
        );
    }
}

/// The Rust files below `dir`, without build output.
fn rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() && path.file_name().is_some_and(|name| name != "target") {
            rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

/// `text` without its comment lines.
fn code(text: &str) -> String {
    let lines = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"));
    lines.collect::<Vec<_>>().join("\n")
}

/// The text from the start of `text` to the bracket that closes the one
/// before it, without white space; `None` when it does not close.
fn bracketed(text: &str, open: char, close: char) -> Option<String> {
    let mut depth = 1;
    let end = text.char_indices().find_map(|(at, c)| {
        depth += i32::from(c == open) - i32::from(c == close);
        (depth == 0).then_some(at)
    })?;
    Some(text[..end].chars().filter(|c| !c.is_whitespace()).collect())
}

/// The lints of each attribute of `code` that allows or expects lints,
/// directly or under `cfg_attr`, without its reason.
fn allowed(code: &str) -> Vec<Vec<String>> {
    let mut found = Vec::new();
    for (at, _) in code.match_indices('#') {
        let rest = code[at + 1..].trim_start_matches('!');
        let Some(attribute) = rest.strip_prefix('[').and_then(|r| bracketed(r, '[', ']')) else {
            continue;
        };
        for word in ["allow(", "expect("] {
            let Some(start) = attribute.find(word) else {
                continue;
            };
            let list = bracketed(&attribute[start + word.len()..], '(', ')').unwrap_or_default();
            let list = list.split("reason=").next().unwrap_or_default();
            let lints = list.split(',').filter(|lint| !lint.is_empty());
            found.push(lints.map(str::to_string).collect());
        }
    }
    found
}

#[test]
fn lint_escapes_are_where_they_are_listed() {
    let root = workspace_dir();
    let os_root = format!("{UNSAFE_MEMBER}/src/lib.rs");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    let mut escapes = Vec::new();
    for path in &files {
        let name = path
            .strip_prefix(&root)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        for lints in allowed(&code(&read(path))) {
            if name != os_root && lints.iter().any(|lint| ESCAPES.contains(&lint.as_str())) {
                escapes.push((name.clone(), lints));
            }
        }
    }
    escapes.sort();
    let listed: Vec<(String, Vec<String>)> = OTHER_ESCAPES
        .iter()
        .map(|(file, lints)| {
            (
                file.to_string(),
                lints.iter().map(|l| l.to_string()).collect(),
            )
        })
        .collect();
    assert_eq!(escapes, listed, "the files that lift a ban");

    // In the crate root of the member, each declaration has the attributes
    // of its row, and no other item allows a lint.
    let code = code(&read(&root.join(&os_root)));
    let mut declared = Vec::new();
    for item in code.split(';') {
        let item = item.rsplit('}').next().unwrap();
        let words: Vec<&str> = item.split_whitespace().collect();
        let [.., "mod", module] = words.as_slice() else {
            continue;
        };
        let attributes = item
            .trim()
            .trim_end_matches(module)
            .trim_end()
            .trim_end_matches("mod");
        let attributes = attributes.trim_end().trim_end_matches("pub");
        let lints: Vec<String> = allowed(attributes).into_iter().flatten().collect();
        let forbids = attributes
            .replace(char::is_whitespace, "")
            .contains("#[forbid(unsafe_code)]");
        declared.push((module.to_string(), lints, forbids));
    }
    let expected: Vec<(String, Vec<String>, bool)> = OS_MODULES
        .iter()
        .map(|(module, lints, forbids)| {
            let lints = lints.iter().map(|lint| lint.to_string()).collect();
            (module.to_string(), lints, *forbids)
        })
        .collect();
    assert_eq!(declared, expected, "the declarations of {os_root}");
    let allowing = OS_MODULES.iter().filter(|(_, lints, _)| !lints.is_empty());
    assert_eq!(
        allowed(&code).len(),
        allowing.count(),
        "{os_root} allows lints elsewhere"
    );
    assert!(
        !code.replace("forbid(unsafe_code)", "").contains("unsafe"),
        "{os_root} holds no unsafe code"
    );
}

/// The code spans of a cell of a Markdown table.
fn code_spans(cell: &str) -> Vec<&str> {
    cell.split('`').skip(1).step_by(2).collect()
}

#[test]
fn copied_files_have_the_digests_of_their_record() {
    let repository = workspace_dir().join("..");
    let record = read(&repository.join("PORTING.md"));
    let rows = record.lines().filter(|line| line.starts_with("| `tools/"));
    let (mut files, mut spans) = (0, Vec::new());
    for row in rows {
        let cells: Vec<&str> = row.split('|').map(str::trim).collect();
        let (module, source, lines, sha256) = (
            code_spans(cells[1])[0],
            code_spans(cells[2])[0],
            cells[3],
            code_spans(cells[6])[0],
        );
        let (pin, path) = source
            .strip_prefix("krepi@")
            .unwrap()
            .split_once(':')
            .unwrap();
        assert!(
            pin.len() == 40 && pin.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "{module}: a full commit SHA"
        );
        assert!(path.starts_with("tools/src/"), "{module}: {path}");
        let (first, last) = lines.split_once('–').expect("a range of lines");
        let text = read(&repository.join(module));
        let copied = if cells[1].contains(", a span") {
            let start = format!("// {source}:{first}-{last}\n");
            let at = text
                .find(&start)
                .unwrap_or_else(|| panic!("{module}: no {start}"));
            let span = &text[at + start.len()..];
            let end = span
                .find(&format!("{SPAN_END}\n"))
                .expect("the end of the span");
            spans.push(start);
            span[..end].to_string()
        } else {
            assert_eq!(
                (first, last),
                ("1", text.lines().count().to_string().as_str())
            );
            files += 1;
            text
        };
        assert_eq!(
            copied.lines().count(),
            last.parse::<usize>().unwrap() - first.parse::<usize>().unwrap() + 1,
            "{module}: the lines {lines}"
        );
        assert_eq!(
            Sha256::of(copied.as_bytes()).as_str(),
            sha256,
            "{module}: the digest of the lines {lines} of {source}"
        );
    }
    assert_eq!(files, 7, "the copied files");
    // Every marked span of the member has its row.
    let os_root = read(&workspace_dir().join(UNSAFE_MEMBER).join("src/lib.rs"));
    let marked: Vec<String> = os_root
        .lines()
        .filter(|line| line.starts_with("// krepi@"))
        .map(|line| format!("{line}\n"))
        .collect();
    spans.sort();
    let mut marked = marked;
    marked.sort();
    assert_eq!(
        spans, marked,
        "the spans of the crate root of {UNSAFE_MEMBER}"
    );
    assert_eq!(spans.len(), 3, "the copied spans");
}
