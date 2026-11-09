//! The workflows step: the GitHub Actions workflows keep to a safe form.
//!
//! # Interface
//!
//! This module uses only the standard library and `yaml-rust2`, so that it
//! can be copied unchanged into the xtask of another repository. The caller
//! gives [`check`] every entry that Git tracks, as a repository-relative path
//! with `/` separators together with the content of a regular file, or `None`
//! for a symbolic link or a submodule, and fails the step when the returned
//! list of problems is not empty. [`is_workflow`] tells which paths the step
//! reads, for a caller that reports them.
//!
//! # Files
//!
//! A workflow file is a tracked regular file below [`DIR`], in any
//! subdirectory, whose name ends in `.yml` or `.yaml` in any letter case.
//! A tracked entry at or below [`DIR`] that is not a regular file, such as a
//! symbolic link or a submodule, is rejected, and so is a workflow file whose
//! content is not UTF-8.
//!
//! # Parser policy
//!
//! One byte order mark at the start of a file is skipped. Before parsing, a
//! file is rejected if it holds, anywhere and in comments too, a character
//! that YAML parsers do not read alike: a control character other than tab,
//! line feed and carriage return, the separators U+2028 and U+2029, a byte
//! order mark after the start, or U+FFFE or U+FFFF. For example, `yaml-rust2`
//! ends the input at U+0000, and U+0085, U+2028 and U+2029 end a line in
//! YAML 1.1 only.
//!
//! A workflow file holds exactly one YAML document whose root is a mapping.
//! Anchors, aliases, the merge key `<<`, keys that are not scalars and keys
//! that occur twice in one mapping are rejected, so that the structure the
//! rules see is the structure GitHub runs.
//!
//! # Rules
//!
//! 1. Neither the text of a workflow file nor any key or scalar that the
//!    parser decodes from it contains `pull_request_target` in any letter
//!    case, so a comment and an escape such as `"pull_request_\x74arget"`
//!    count as well.
//! 2. Every job below `jobs` has a `permissions` key of its own; the
//!    workflow-level `permissions` does not count.
//! 3. In a workflow whose `on` triggers include `pull_request` in any letter
//!    case, or a trigger that rule 1 rejects, no job's `permissions` requests
//!    write access: each is `read-all`, `{}` or a mapping of permissions to
//!    `read` or `none`, so that `write-all`, a `write` grant and any other
//!    value are rejected. Nothing outside `on` references the `secrets`
//!    context: no `${{ }}` expression and no `if` condition names it, and no
//!    key is `secrets`, all in any letter case. This covers
//!    `secrets.GITHUB_TOKEN` as well. The rule fails closed: a string literal
//!    `'secrets'` in an expression and a key named `secrets`, such as an
//!    environment variable or an action input, count as references too. A
//!    workflow whose `on` is missing, empty or cannot be read is reported and
//!    checked as one that runs on `pull_request`. An `on` is empty when it
//!    is a YAML null, such as no value or `~`, an empty list or an empty
//!    mapping, or when one of its events is a YAML null.

use yaml_rust2::parser::{Event, Parser};
use yaml_rust2::scanner::Marker;

/// The directory that GitHub reads workflows from.
pub const DIR: &str = ".github/workflows";

/// The trigger that rule 1 rejects.
const PULL_REQUEST_TARGET: &str = "pull_request_target";

/// The byte order mark, which is skipped at the start of a file.
const BOM: char = '\u{feff}';

/// Whether the repository-relative `path` names a workflow file.
pub fn is_workflow(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    below_dir(path) && (lower.ends_with(".yml") || lower.ends_with(".yaml"))
}

fn below_dir(path: &str) -> bool {
    path.strip_prefix(DIR)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// Every workflow problem among the tracked entries.
pub fn check<'a, I>(tracked: I) -> Vec<String>
where
    I: IntoIterator<Item = (&'a str, Option<&'a [u8]>)>,
{
    let mut problems = Vec::new();
    for (path, content) in tracked {
        if path != DIR && !below_dir(path) {
            continue;
        }
        match content {
            None => problems.push(format!(
                "{path}: is not a regular file; {DIR} may only hold regular files"
            )),
            Some(_) if !is_workflow(path) => {}
            Some(bytes) => match std::str::from_utf8(bytes) {
                Ok(text) => problems.extend(check_file(path, text)),
                Err(_) => problems.push(format!("{path}: is not UTF-8")),
            },
        }
    }
    problems
}

/// Every problem of one workflow file.
fn check_file(path: &str, text: &str) -> Vec<String> {
    let text = text.strip_prefix(BOM).unwrap_or(text);
    let mut problems = Vec::new();
    let mentioned = mentions_target(text);
    if mentioned {
        problems.push(format!(
            "{path}: mentions `{PULL_REQUEST_TARGET}`; workflows must not use that trigger"
        ));
    }
    let unclear = unclear_characters(text);
    if !unclear.is_empty() {
        for (line, character) in unclear {
            problems.push(format!(
                "{path}:{line}: holds U+{:04X}, which YAML parsers do not read alike",
                u32::from(character)
            ));
        }
        return problems;
    }
    let root = match parse(text) {
        Ok(root) => root,
        Err(problem) => {
            problems.push(format!("{path}:{problem}"));
            return problems;
        }
    };
    if !mentioned {
        for line in target_lines(&root) {
            problems.push(format!(
                "{path}:{line}: a key or value decodes to `{PULL_REQUEST_TARGET}`; workflows must not use that trigger"
            ));
        }
    }
    let Value::Map(top) = &root.value else {
        problems.push(format!(
            "{path}:{}: the document is not a mapping",
            root.line
        ));
        return problems;
    };
    let pull_request = match get(top, "on") {
        Some(on) => match triggers(&on.value) {
            Ok(names) => names.into_iter().any(pull_request_trigger),
            Err(problem) => {
                problems.push(format!("{path}:{problem}"));
                true
            }
        },
        None => {
            problems.push(format!("{path}: has no `on` triggers"));
            true
        }
    };
    if pull_request {
        for entry in top
            .iter()
            .filter(|entry| entry.key != "on" && entry.key != "jobs")
        {
            for line in entry_references(entry) {
                problems.push(format!(
                    "{path}:{line}: the workflow `{}` references `secrets` in a workflow that runs on pull_request",
                    entry.key
                ));
            }
        }
    }
    let Some(jobs) = get(top, "jobs") else {
        problems.push(format!("{path}: has no `jobs`"));
        return problems;
    };
    let Value::Map(jobs) = &jobs.value.value else {
        problems.push(format!("{path}:{}: `jobs` is not a mapping", jobs.line));
        return problems;
    };
    for entry in jobs {
        let id = &entry.key;
        let Value::Map(job) = &entry.value.value else {
            problems.push(format!(
                "{path}:{}: job `{id}` is not a mapping",
                entry.line
            ));
            continue;
        };
        match get(job, "permissions") {
            None => problems.push(format!(
                "{path}:{}: job `{id}` has no `permissions` of its own",
                entry.line
            )),
            Some(permissions) if pull_request => {
                if let Some(problem) = grant_problem(&permissions.value) {
                    problems.push(format!(
                        "{path}:{}: job `{id}` {problem} in a workflow that runs on pull_request",
                        permissions.line
                    ));
                }
            }
            Some(_) => {}
        }
        if pull_request {
            for line in entry_references(entry) {
                problems.push(format!(
                    "{path}:{line}: job `{id}` references `secrets` in a workflow that runs on pull_request"
                ));
            }
        }
    }
    problems
}

/// Whether `text` contains `pull_request_target` in any letter case.
fn mentions_target(text: &str) -> bool {
    text.to_ascii_lowercase().contains(PULL_REQUEST_TARGET)
}

/// The characters of `text` that YAML parsers do not read alike, each with
/// its line. Lines end as in YAML: at a line feed, a carriage return and
/// line feed, or a carriage return alone.
fn unclear_characters(text: &str) -> Vec<(usize, char)> {
    let mut found = Vec::new();
    let mut line = 1;
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\n' => line += 1,
            '\r' if characters.peek() != Some(&'\n') => line += 1,
            '\t' | '\r' => {}
            '\u{2028}' | '\u{2029}' | BOM | '\u{fffe}' | '\u{ffff}' => {
                found.push((line, character));
            }
            _ if character.is_control() => found.push((line, character)),
            _ => {}
        }
    }
    found
}

/// A YAML node and the line it starts on.
#[derive(Debug)]
struct Node {
    line: usize,
    value: Value,
}

#[derive(Debug)]
enum Value {
    Scalar(String),
    Sequence(Vec<Node>),
    Map(Vec<Entry>),
}

/// A mapping entry: a scalar key, the line of the key and the value.
#[derive(Debug)]
struct Entry {
    key: String,
    line: usize,
    value: Node,
}

fn get<'a>(map: &'a [Entry], key: &str) -> Option<&'a Entry> {
    map.iter().find(|entry| entry.key == key)
}

/// Parses one YAML document under the parser policy. An error starts with
/// the line it refers to, or with a space when it has none.
fn parse(text: &str) -> Result<Node, String> {
    let mut parser = Parser::new_from_str(text);
    let mut next = || {
        parser.next_token().map_err(|error| {
            format!(
                "{}: not valid YAML: {}",
                error.marker().line(),
                error.info()
            )
        })
    };
    let (event, _) = next()?;
    if event != Event::StreamStart {
        return Err(" not valid YAML: no stream start".to_string());
    }
    let (event, mark) = next()?;
    match event {
        Event::DocumentStart => {}
        Event::StreamEnd => return Err(" holds no YAML document".to_string()),
        _ => {
            return Err(format!(
                "{}: not valid YAML: no document start",
                mark.line()
            ));
        }
    }
    let (event, mark) = next()?;
    let root = node(&mut next, event, mark)?;
    let (event, mark) = next()?;
    if event != Event::DocumentEnd {
        return Err(format!("{}: not valid YAML: no document end", mark.line()));
    }
    let (event, mark) = next()?;
    if event != Event::StreamEnd {
        return Err(format!(
            "{}: holds more than one YAML document",
            mark.line()
        ));
    }
    Ok(root)
}

/// Builds the node that starts with `event` from the events that follow.
fn node<F>(next: &mut F, event: Event, mark: Marker) -> Result<Node, String>
where
    F: FnMut() -> Result<(Event, Marker), String>,
{
    let line = mark.line();
    let anchored = |anchor: usize| {
        if anchor == 0 {
            Ok(())
        } else {
            Err(format!("{line}: YAML anchors are not allowed"))
        }
    };
    let value = match event {
        Event::Alias(_) => return Err(format!("{line}: YAML aliases are not allowed")),
        Event::Scalar(text, _, anchor, _) => {
            anchored(anchor)?;
            Value::Scalar(text)
        }
        Event::SequenceStart(anchor, _) => {
            anchored(anchor)?;
            let mut items = Vec::new();
            loop {
                let (event, mark) = next()?;
                if event == Event::SequenceEnd {
                    break;
                }
                items.push(node(next, event, mark)?);
            }
            Value::Sequence(items)
        }
        Event::MappingStart(anchor, _) => {
            anchored(anchor)?;
            let mut entries: Vec<Entry> = Vec::new();
            loop {
                let (event, mark) = next()?;
                if event == Event::MappingEnd {
                    break;
                }
                let key = node(next, event, mark)?;
                let Value::Scalar(text) = key.value else {
                    return Err(format!("{}: mapping keys must be scalars", key.line));
                };
                if text == "<<" {
                    return Err(format!(
                        "{}: the YAML merge key `<<` is not allowed",
                        key.line
                    ));
                }
                if entries.iter().any(|entry| entry.key == text) {
                    return Err(format!("{}: the key `{text}` occurs twice", key.line));
                }
                let (event, mark) = next()?;
                let value = node(next, event, mark)?;
                entries.push(Entry {
                    key: text,
                    line: key.line,
                    value,
                });
            }
            Value::Map(entries)
        }
        _ => return Err(format!("{line}: not valid YAML: unexpected event")),
    };
    Ok(Node { line, value })
}

/// The event names of an `on` value: a scalar, a sequence of scalars or a
/// mapping keyed by event name. An `on` that names no event, or names an
/// event that is a YAML null, is an error.
fn triggers(on: &Node) -> Result<Vec<&str>, String> {
    let malformed = || {
        format!(
            "{}: `on` is not an event, a list of events or a mapping of events",
            on.line
        )
    };
    let names: Vec<&str> = match &on.value {
        Value::Scalar(name) => vec![name.as_str()],
        Value::Sequence(items) => items
            .iter()
            .map(|item| match &item.value {
                Value::Scalar(name) => Ok(name.as_str()),
                _ => Err(malformed()),
            })
            .collect::<Result<_, _>>()?,
        Value::Map(entries) => entries.iter().map(|entry| entry.key.as_str()).collect(),
    };
    if names.is_empty() || names.iter().any(|name| is_null(name)) {
        return Err(format!(
            "{}: `on` is empty or names an empty event",
            on.line
        ));
    }
    Ok(names)
}

/// Whether the scalar `text` reads as a YAML null: empty, `~` or `null` in
/// one of its spellings. The style of the scalar is not kept, so a quoted
/// `"null"` counts too.
fn is_null(text: &str) -> bool {
    matches!(text, "" | "~" | "null" | "Null" | "NULL")
}

/// Whether the trigger `name` makes rule 3 apply: it is `pull_request` in
/// any letter case, or a trigger that rule 1 rejects.
fn pull_request_trigger(name: &str) -> bool {
    name.eq_ignore_ascii_case("pull_request") || mentions_target(name)
}

/// The lines of the keys and scalars in `node` whose decoded text mentions
/// `pull_request_target`.
fn target_lines(node: &Node) -> Vec<usize> {
    match &node.value {
        Value::Scalar(text) => {
            if mentions_target(text) {
                vec![node.line]
            } else {
                Vec::new()
            }
        }
        Value::Sequence(items) => items.iter().flat_map(target_lines).collect(),
        Value::Map(entries) => entries
            .iter()
            .flat_map(|entry| {
                let key = mentions_target(&entry.key).then_some(entry.line);
                key.into_iter().chain(target_lines(&entry.value))
            })
            .collect(),
    }
}

/// Why a `permissions` value is not acceptable in a workflow that runs on
/// pull requests: it grants write access, or it is not one of `read-all`,
/// a mapping of permissions to `read` or `none`, or `{}`.
fn grant_problem(permissions: &Node) -> Option<String> {
    let unknown = || {
        Some(
            "has a `permissions` value other than `read-all` or a mapping to `read` and `none`"
                .to_string(),
        )
    };
    match &permissions.value {
        Value::Scalar(text) if text == "read-all" => None,
        Value::Scalar(text) if text == "write-all" => {
            Some("requests write access (`write-all`)".to_string())
        }
        Value::Map(entries) => {
            for entry in entries {
                match &entry.value.value {
                    Value::Scalar(level) if level == "read" || level == "none" => {}
                    Value::Scalar(level) if level == "write" => {
                        return Some(format!("requests write access (`{}: write`)", entry.key));
                    }
                    _ => return unknown(),
                }
            }
            None
        }
        _ => unknown(),
    }
}

/// The lines of `entry`, its key and its value, that reference the `secrets`
/// context, each line once.
fn entry_references(entry: &Entry) -> Vec<usize> {
    let mut lines = Vec::new();
    if entry.key.eq_ignore_ascii_case("secrets") || names_secrets(&entry.key, false) {
        lines.push(entry.line);
    }
    lines.extend(secret_references(&entry.value, entry.key == "if"));
    lines.dedup();
    lines
}

/// The lines of `node` that reference the `secrets` context, each line once.
/// `condition` marks the value of an `if` key, which is an expression as a
/// whole.
fn secret_references(node: &Node, condition: bool) -> Vec<usize> {
    let mut lines: Vec<usize> = match &node.value {
        Value::Scalar(text) => {
            if names_secrets(text, condition) {
                vec![node.line]
            } else {
                Vec::new()
            }
        }
        Value::Sequence(items) => items
            .iter()
            .flat_map(|item| secret_references(item, false))
            .collect(),
        Value::Map(entries) => entries.iter().flat_map(entry_references).collect(),
    };
    // The nodes are visited in the order of the text, so a repeated line
    // follows the first.
    lines.dedup();
    lines
}

/// Whether `text` names the `secrets` context inside a `${{ }}` expression,
/// or anywhere when `expression` says that the whole text is one.
fn names_secrets(text: &str, expression: bool) -> bool {
    if expression {
        return names_context(text);
    }
    expressions(text).into_iter().any(names_context)
}

/// The bodies of the `${{ }}` expressions in `text`. A `}}` inside a quoted
/// string does not end an expression, and an expression that is not closed
/// runs to the end of the text.
fn expressions(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("${{") {
        let body = &rest[start + 3..];
        let bytes = body.as_bytes();
        let mut quoted = false;
        let mut end = body.len();
        for (at, byte) in bytes.iter().enumerate() {
            match byte {
                b'\'' => quoted = !quoted,
                b'}' if !quoted && bytes.get(at + 1) == Some(&b'}') => {
                    end = at;
                    break;
                }
                _ => {}
            }
        }
        found.push(&body[..end]);
        rest = &body[end..];
    }
    found
}

/// Whether the expression `text` uses `secrets` as a context name: not as a
/// part of a longer name and not as a property after a dot.
fn names_context(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let name_char = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    lower.match_indices("secrets").any(|(at, word)| {
        let before = lower[..at].chars().next_back();
        let after = lower[at + word.len()..].chars().next();
        !before.is_some_and(|c| name_char(c) || c == '.') && !after.is_some_and(name_char)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A workflow that every rule accepts, in the form the repository uses.
    /// The line numbers in the expected messages below refer to this text.
    const VALID: &str = "\
name: check

on:
  pull_request:
    branches: [main]
  push:
    branches: [main]

env:
  CARGO_TERM_COLOR: never

jobs:
  check:
    name: cargo xtask check
    runs-on: ubuntu-24.04-arm
    permissions:
      contents: read
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          ref: ${{ github.event.pull_request.head.sha || github.sha }}
          persist-credentials: false
      - name: Run
        if: ${{ github.token != '' }}
        run: |
          echo \"no secrets are used here\"
          cargo xtask check
";

    /// The script line of `VALID`.
    const SCRIPT: &str = "          cargo xtask check\n";

    const PATH: &str = ".github/workflows/check.yml";

    fn problems(text: &str) -> Vec<String> {
        check([(PATH, Some(text.as_bytes()))])
    }

    fn assert_one(text: &str, expected: &str) {
        let found = problems(text);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].starts_with(PATH), "{found:?}");
        assert!(found[0].contains(expected), "{found:?}");
    }

    fn with_script(script: &str) -> String {
        assert!(VALID.contains(SCRIPT));
        VALID.replace(SCRIPT, &format!("          {script}\n"))
    }

    #[test]
    fn the_valid_workflow_passes() {
        assert_eq!(problems(VALID), Vec::<String>::new());
    }

    #[test]
    fn a_workflow_without_pull_request_may_request_write_access_and_secrets() {
        let text = VALID
            .replace("  pull_request:\n    branches: [main]\n", "")
            .replace("contents: read", "contents: write")
            .replace("github.token", "secrets.GITHUB_TOKEN");
        assert_eq!(problems(&text), Vec::<String>::new());
    }

    #[test]
    fn reads_every_workflow_file_below_the_directory() {
        assert!(is_workflow(".github/workflows/check.yml"));
        assert!(is_workflow(".github/workflows/nested/deep/release.YAML"));
        assert!(!is_workflow(".github/workflows/README.md"));
        assert!(!is_workflow(".github/workflows-old/check.yml"));
        assert!(!is_workflow(".github/check.yml"));
        let broken = b"jobs: [".as_slice();
        let found = check([
            (".github/workflows/nested/a.yaml", Some(broken)),
            (".github/workflows/notes.txt", Some(broken)),
            (".github/workflows-old/check.yml", Some(broken)),
            ("docs/example.yml", Some(broken)),
        ]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].starts_with(".github/workflows/nested/a.yaml:"),
            "{found:?}"
        );
    }

    #[test]
    fn rejects_links_submodules_and_non_utf8_files() {
        let found = check([
            (".github/workflows", None),
            (".github/workflows/linked.yml", None),
            (
                ".github/workflows/bytes.yml",
                Some(b"on: push\xff".as_slice()),
            ),
            (".github/assets/link.svg", None),
        ]);
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(found[0].contains("not a regular file"), "{found:?}");
        assert!(found[1].contains("not a regular file"), "{found:?}");
        assert!(found[2].ends_with("is not UTF-8"), "{found:?}");
    }

    // Rule 1: `pull_request_target` anywhere in the text.

    #[test]
    fn rejects_pull_request_target() {
        assert_one(
            &VALID.replace("  push:\n", "  pull_request_target:\n"),
            "mentions `pull_request_target`",
        );
    }

    #[test]
    fn rejects_pull_request_target_in_a_comment_and_in_any_case() {
        assert_one(
            &format!("# Pull_Request_Target is not used.\n{VALID}"),
            "mentions `pull_request_target`",
        );
    }

    #[test]
    fn rejects_pull_request_target_spelled_with_escapes() {
        let jobs = "jobs:\n  a:\n    permissions: {}\n";
        for (on, line) in [
            ("on:\n  \"pull_request_\\x74arget\":\n", 2),
            ("on: \"PULL_REQUEST\\u005fTARGET\"\n", 1),
            ("on: [push, \"pull_request_\\\n  target\"]\n", 1),
        ] {
            assert_one(
                &format!("{on}{jobs}"),
                &format!(":{line}: a key or value decodes to `pull_request_target`"),
            );
        }
    }

    #[test]
    fn an_escaped_pull_request_target_trigger_is_checked_as_pull_request() {
        let found = problems(
            "on: \"pull_request\\u005ftarget\"\njobs:\n  a:\n    permissions: write-all\n",
        );
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].contains(":1: a key or value decodes"), "{found:?}");
        assert!(
            found[1].contains(":4: job `a` requests write access (`write-all`)"),
            "{found:?}"
        );
    }

    // Rule 2: every job has its own `permissions`.

    #[test]
    fn rejects_a_job_without_its_own_permissions() {
        let text = VALID
            .replace("    permissions:\n      contents: read\n", "")
            .replace("env:\n", "permissions:\n  contents: read\n\nenv:\n");
        assert_one(&text, ":16: job `check` has no `permissions` of its own");
    }

    #[test]
    fn rejects_a_workflow_without_jobs_or_triggers() {
        assert_one("on: push\n", "has no `jobs`");
        assert_one("on: push\njobs: [a]\n", ":2: `jobs` is not a mapping");
        assert_one(
            "on: push\njobs:\n  a: run\n",
            ":3: job `a` is not a mapping",
        );
        assert_one("jobs:\n  a:\n    permissions: {}\n", "has no `on` triggers");
    }

    // Rule 3: no write access and no secrets on pull requests.

    #[test]
    fn rejects_write_access_in_a_pull_request_workflow() {
        assert_one(
            &VALID.replace("contents: read", "contents: write"),
            ":16: job `check` requests write access (`contents: write`)",
        );
    }

    #[test]
    fn rejects_secrets_in_a_pull_request_workflow() {
        assert_one(
            &VALID.replace("github.token", "secrets.GITHUB_TOKEN"),
            ":24: job `check` references `secrets`",
        );
    }

    #[test]
    fn rejects_each_form_of_write_access() {
        let with = |permissions: &str| {
            VALID.replace("    permissions:\n      contents: read\n", permissions)
        };
        assert_one(&with("    permissions: write-all\n"), "(`write-all`)");
        assert_one(
            &with("    permissions:\n      contents: read\n      id-token: write\n"),
            "(`id-token: write`)",
        );
        assert_one(&with("    permissions: write\n"), "other than `read-all`");
        assert_one(
            &with("    permissions:\n      contents: ${{ inputs.level }}\n"),
            "other than `read-all`",
        );
        for accepted in [
            "    permissions: read-all\n",
            "    permissions: {}\n",
            "    permissions:\n      contents: read\n      checks: none\n",
        ] {
            assert_eq!(
                problems(&with(accepted)),
                Vec::<String>::new(),
                "{accepted}"
            );
        }
    }

    #[test]
    fn rejects_each_form_of_secrets_reference() {
        let workflow_env = VALID.replace("CARGO_TERM_COLOR: never", "TOKEN: ${{ secrets.TOKEN }}");
        assert_one(
            &workflow_env,
            ":10: the workflow `env` references `secrets`",
        );
        for key in ["run-name", "concurrency"] {
            assert_one(
                &VALID.replace("\non:", &format!("\n{key}: ${{{{ secrets.X }}}}\non:")),
                &format!(":3: the workflow `{key}` references `secrets`"),
            );
        }
        let in_job = [
            VALID.replace(
                "    runs-on:",
                "    env:\n      TOKEN: ${{ secrets.TOKEN }}\n    runs-on:",
            ),
            VALID.replace(
                "${{ github.token != '' }}",
                "github.token != '' && secrets.X",
            ),
            VALID.replace("    runs-on:", "    secrets: inherit\n    runs-on:"),
            // The key and the value on one line are reported once.
            VALID.replace(
                "    runs-on:",
                "    secrets: ${{ secrets.X }}\n    runs-on:",
            ),
            with_script("echo ${{ toJSON(secrets) }}"),
            with_script("echo ${{ format('}}', SECRETS['X']) }}"),
            with_script("echo ${{ github.sha }} ${{ secrets.X"),
        ];
        for text in in_job {
            assert_one(&text, "job `check` references `secrets`");
        }
        for accepted in [
            "echo ${{ inputs.secrets }}",
            "echo ${{ github.secrets_count }}",
            "echo secrets without an expression",
        ] {
            assert_eq!(
                problems(&with_script(accepted)),
                Vec::<String>::new(),
                "{accepted}"
            );
        }
        // The secrets that a reusable workflow declares below `on` are not
        // references.
        let declared = VALID.replace(
            "  push:\n",
            "  workflow_call:\n    secrets:\n      TOKEN:\n        required: false\n  push:\n",
        );
        assert_eq!(problems(&declared), Vec::<String>::new());
    }

    #[test]
    fn a_secrets_literal_or_a_key_named_secrets_counts_as_a_reference() {
        // Neither reads the context, but the rule fails closed rather than
        // parse expressions and action inputs.
        for text in [
            VALID.replace(
                "${{ github.token != '' }}",
                "contains(github.event.pull_request.labels.*.name, 'secrets')",
            ),
            VALID.replace(
                "    runs-on:",
                "    env:\n      SECRETS: none\n    runs-on:",
            ),
            VALID.replace(
                "          persist-credentials: false\n",
                "          persist-credentials: false\n          secrets: none\n",
            ),
        ] {
            assert_one(&text, "job `check` references `secrets`");
        }
    }

    #[test]
    fn reads_every_form_of_on() {
        let jobs = "jobs:\n  a:\n    permissions:\n      contents: write\n";
        for on in [
            "on: pull_request\n",
            "on: [push, pull_request]\n",
            "\"on\":\n  pull_request:\n    branches: [main]\n",
            "on: [push, PULL_REQUEST]\n",
            "on: \"pull_\\x72equest\"\n",
        ] {
            assert_one(&format!("{on}{jobs}"), "requests write access");
        }
        for on in [
            "on: push\n",
            "on: [push, workflow_dispatch]\n",
            "on:\n  pull_request_review:\n",
        ] {
            assert_eq!(
                problems(&format!("{on}{jobs}")),
                Vec::<String>::new(),
                "{on}"
            );
        }
        assert_one(
            "on: [[pull_request]]\njobs:\n  a:\n    permissions: {}\n",
            ":1: `on` is not an event",
        );
    }

    #[test]
    fn an_empty_on_is_reported_and_checked_as_pull_request() {
        let jobs = "jobs:\n  a:\n    permissions: write-all\n";
        for on in [
            "on:\n",
            "on: ~\n",
            "on: null\n",
            "on: NULL\n",
            "on: ''\n",
            "on: []\n",
            "on: {}\n",
            "on: [push, null]\n",
            "on:\n  ~:\n",
        ] {
            let found = problems(&format!("{on}{jobs}"));
            assert_eq!(found.len(), 2, "{on:?}: {found:?}");
            assert!(
                found[0].contains("`on` is empty or names an empty event"),
                "{on:?}: {found:?}"
            );
            assert!(
                found[1].contains("job `a` requests write access (`write-all`)"),
                "{on:?}: {found:?}"
            );
        }
    }

    // Parser policy.

    /// `VALID` with a comment line 9 that holds `character`.
    fn with_comment(character: char) -> String {
        VALID.replace("\nenv:", &format!("\n# a{character}comment\nenv:"))
    }

    #[test]
    fn rejects_a_nul_character_even_in_a_comment() {
        // `yaml-rust2` ends the input at U+0000, so it would not see the job
        // after it.
        assert_one(&with_comment('\0'), ":9: holds U+0000");
    }

    #[test]
    fn rejects_line_breaks_of_yaml_1_1_even_in_a_comment() {
        for (character, code) in [
            ('\u{2028}', "U+2028"),
            ('\u{2029}', "U+2029"),
            ('\u{85}', "U+0085"),
        ] {
            assert_one(&with_comment(character), &format!(":9: holds {code}"));
        }
    }

    #[test]
    fn rejects_other_unclear_characters() {
        for (character, code) in [
            ('\u{1b}', "U+001B"),
            ('\u{7f}', "U+007F"),
            ('\u{9b}', "U+009B"),
            ('\u{feff}', "U+FEFF"),
            ('\u{fffe}', "U+FFFE"),
        ] {
            assert_one(&with_comment(character), &format!(":9: holds {code}"));
        }
        // A carriage return ends a line with or without a line feed.
        for text in [
            "on: push\r\njobs: {}\r\n#\u{1b}\r\n",
            "on: push\rjobs: {}\r#\u{1b}\r",
        ] {
            assert_one(text, ":3: holds U+001B");
        }
    }

    #[test]
    fn accepts_tabs_line_ends_and_one_leading_byte_order_mark() {
        for text in [
            with_comment('\t'),
            VALID.replace('\n', "\r\n"),
            format!("{BOM}{VALID}"),
        ] {
            assert_eq!(problems(&text), Vec::<String>::new(), "{text:?}");
        }
        assert_one(&format!("{BOM}{BOM}{VALID}"), ":1: holds U+FEFF");
    }

    #[test]
    fn rejects_anchors() {
        // The parser places a block mapping at its first key, below the anchor.
        assert_one(
            &VALID.replace("    permissions:\n", "    permissions: &read\n"),
            ":17: YAML anchors are not allowed",
        );
        assert_one(
            "on: &push push\njobs: {}\n",
            ":1: YAML anchors are not allowed",
        );
        assert_one(
            "on: push\njobs: &jobs {}\n",
            ":2: YAML anchors are not allowed",
        );
    }

    #[test]
    fn rejects_aliases() {
        // An alias needs an anchor before it, which is rejected first, or it
        // names no anchor, which the parser rejects.
        assert_one(
            "on: push\njobs: {}\nenv:\n  A: &a x\n  B: *a\n",
            ":4: YAML anchors are not allowed",
        );
        assert_one("on: push\njobs: *jobs\n", ":2: not valid YAML");
        // So the parser never yields an alias to `node`; give it one directly.
        let (_, mark) = Parser::new_from_str("")
            .next_token()
            .expect("a stream start");
        let mut next = || Ok((Event::MappingEnd, mark));
        let alias = node(&mut next, Event::Alias(1), mark);
        assert!(alias.is_err_and(|problem| problem.ends_with("YAML aliases are not allowed")),);
    }

    #[test]
    fn rejects_merge_keys() {
        assert_one(
            &VALID.replace("      contents: read\n", "      <<: {contents: read}\n"),
            ":17: the YAML merge key `<<` is not allowed",
        );
    }

    #[test]
    fn rejects_duplicate_keys() {
        assert_one(
            &VALID.replace("    runs-on:", "    permissions: write-all\n    runs-on:"),
            ":17: the key `permissions` occurs twice",
        );
        assert_one(
            &VALID.replace("\non:", "\n'on': push\non:"),
            ":4: the key `on` occurs twice",
        );
    }

    #[test]
    fn rejects_complex_keys_several_documents_and_invalid_yaml() {
        assert_one("? [a]\n: b\n", ":1: mapping keys must be scalars");
        assert_one(
            "on: push\n---\njobs: {}\n",
            ":2: holds more than one YAML document",
        );
        assert_one("", "holds no YAML document");
        assert_one("- on\n", ":1: the document is not a mapping");
        assert_one("on: [push\n", "not valid YAML");
    }
}
