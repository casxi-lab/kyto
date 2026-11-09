//! The commits step: the header of every commit message on the branch keeps
//! to the rules of `.githooks/commit-msg`.
//!
//! # Range
//!
//! The step checks the commits that `HEAD` reaches and the base does not,
//! `<base>..HEAD`, merge commits included. The base is the commit that the
//! [`BASE_VARIABLE`] environment variable names. When the variable is unset,
//! empty or all zeros, as GitHub gives for the commit before a push that
//! creates a branch, the base is the merge base of [`MAIN`] and `HEAD`.
//!
//! # Rules
//!
//! The header is the first line of the message that neither starts with `#`
//! nor is blank. A header that starts with `Merge `, `Revert `, `fixup! ` or
//! `squash! ` passes. Any other header has the form
//! `<type>[(<scope>)][!]: <description>`, where the type is one of [`TYPES`],
//! the scope is one or more of `a`-`z`, `0`-`9`, `.`, `_`, `/` and `-`, and the
//! description starts with a character that is not white space. The header
//! is at most [`MAX_HEADER`] characters long. A message that is not UTF-8
//! fails.

use std::path::Path;
use std::process::Command;

use crate::check::Facts;

/// The environment variable that names the base commit.
pub const BASE_VARIABLE: &str = "CHECK_BASE";

/// The branch whose merge base with `HEAD` is the base otherwise.
pub const MAIN: &str = "origin/main";

/// The accepted commit types.
pub const TYPES: [&str; 11] = [
    "feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore", "revert",
];

/// The header prefixes that pass without further checks.
const EXEMPT: [&str; 4] = ["Merge ", "Revert ", "fixup! ", "squash! "];

/// The longest accepted header, in characters.
pub const MAX_HEADER: usize = 72;

/// The commits step, with the base taken from [`BASE_VARIABLE`]. The base,
/// where it came from and the number of commits checked become facts.
pub fn run(root: &Path, facts: &mut Facts) -> Result<(), String> {
    let given = std::env::var(BASE_VARIABLE).ok();
    check_range(root, given.as_deref(), facts)
}

/// Where the base of the range comes from.
#[derive(Debug, PartialEq, Eq)]
enum Base<'a> {
    /// The commit that the variable names.
    Given(&'a str),
    /// The merge base of [`MAIN`] and `HEAD`.
    MergeBase,
}

fn base(given: Option<&str>) -> Base<'_> {
    match given {
        Some(value) if !value.is_empty() && !value.bytes().all(|byte| byte == b'0') => {
            Base::Given(value)
        }
        _ => Base::MergeBase,
    }
}

/// Checks the commits after the base that `given` selects.
fn check_range(root: &Path, given: Option<&str>, facts: &mut Facts) -> Result<(), String> {
    let base = match base(given) {
        Base::Given(value) => {
            facts.push(("base_from".to_string(), BASE_VARIABLE.to_string()));
            let spec = format!("{value}^{{commit}}");
            git(root, &["rev-parse", "--verify", "--quiet", &spec]).map_err(|_| {
                format!("{BASE_VARIABLE}=`{value}` does not name a commit in this repository")
            })?
        }
        Base::MergeBase => {
            facts.push((
                "base_from".to_string(),
                format!("merge base of {MAIN} and HEAD"),
            ));
            git(root, &["merge-base", MAIN, "HEAD"]).map_err(|error| {
                format!(
                    "cannot find the merge base of {MAIN} and HEAD ({error}); fetch {MAIN} or set {BASE_VARIABLE} to the commit to check from"
                )
            })?
        }
    };
    let base = String::from_utf8_lossy(&base).trim().to_string();
    facts.push(("base".to_string(), base.clone()));
    let range = format!("{base}..HEAD");
    let log = git(root, &["log", "-z", "--reverse", "--format=%H%n%B", &range])?;
    let commits = parse_log(&log)?;
    facts.push(("commits".to_string(), commits.len().to_string()));
    println!("commits: {} in {range}", commits.len());
    let problems: Vec<String> = commits
        .iter()
        .filter_map(|commit| {
            let problem = match commit.message {
                Some(message) => problem(message)?,
                None => "the message is not UTF-8".to_string(),
            };
            Some(format!("commit {}: {problem}", short(commit.id)))
        })
        .collect();
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

/// Runs git in `root` and returns its standard output, or its standard
/// error as the error.
fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// A commit and its message; `None` when the message is not UTF-8.
#[derive(Debug, PartialEq, Eq)]
struct Commit<'a> {
    id: &'a str,
    message: Option<&'a str>,
}

/// The commits of `git log -z --format=%H%n%B` output.
fn parse_log(log: &[u8]) -> Result<Vec<Commit<'_>>, String> {
    log.split(|&byte| byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| {
            let (id, message) = match record.iter().position(|&byte| byte == b'\n') {
                Some(at) => (&record[..at], &record[at + 1..]),
                None => (record, &[][..]),
            };
            let id = std::str::from_utf8(id)
                .ok()
                .filter(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
                .ok_or_else(|| {
                    format!(
                        "unexpected git log record `{}`",
                        String::from_utf8_lossy(record)
                    )
                })?;
            Ok(Commit {
                id,
                message: std::str::from_utf8(message).ok(),
            })
        })
        .collect()
}

/// The first twelve digits of a commit id.
fn short(id: &str) -> &str {
    id.get(..12).unwrap_or(id)
}

/// White space as the POSIX `space` class of the C locale defines it.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r')
}

/// The header of a commit message: its first line that neither starts with
/// `#` nor is blank.
pub fn header(message: &str) -> Option<&str> {
    message
        .split('\n')
        .find(|line| !line.starts_with('#') && !line.chars().all(is_space))
}

/// Why the header of `message` breaks the rules, if it does.
pub fn problem(message: &str) -> Option<String> {
    let Some(header) = header(message) else {
        return Some("the message has no header".to_string());
    };
    if EXEMPT.iter().any(|prefix| header.starts_with(prefix)) {
        return None;
    }
    if !TYPES
        .iter()
        .any(|kind| header.strip_prefix(kind).is_some_and(follows_the_type))
    {
        return Some(format!(
            "header `{header}` does not follow Conventional Commits: expected `<type>[(scope)][!]: <description>` with a type of {}",
            TYPES.join(", ")
        ));
    }
    let length = header.chars().count();
    if length > MAX_HEADER {
        return Some(format!(
            "header `{header}` is {length} characters; keep it at most {MAX_HEADER}"
        ));
    }
    None
}

/// Whether `rest`, the header after its type, is `[(<scope>)][!]: ` followed
/// by a description that does not start with white space.
fn follows_the_type(rest: &str) -> bool {
    let rest = match rest.strip_prefix('(') {
        Some(inner) => {
            let Some(close) = inner.find(')') else {
                return false;
            };
            let scope = &inner[..close];
            let valid = |c: char| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '/' | '-')
            };
            if scope.is_empty() || !scope.chars().all(valid) {
                return false;
            }
            &inner[close + 1..]
        }
        None => rest,
    };
    let rest = rest.strip_prefix('!').unwrap_or(rest);
    rest.strip_prefix(": ")
        .and_then(|description| description.chars().next())
        .is_some_and(|first| !is_space(first))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;
    use std::process::Stdio;

    use super::*;

    #[test]
    fn accepts_conventional_headers() {
        for message in [
            "docs: describe the graph view",
            "feat(parser): support exact decimal literals\n\nBody text.\n",
            "fix(a.b_c/d-9)!: change the result contract",
            "refactor!: drop the old view model",
            "revert: undo the layout change",
            "ci: x",
        ] {
            assert_eq!(problem(message), None, "{message}");
        }
        for kind in TYPES {
            assert_eq!(problem(&format!("{kind}: do it")), None, "{kind}");
        }
    }

    #[test]
    fn exempt_headers_pass_as_in_the_hook() {
        for message in [
            "Merge pull request #2 from someone/branch",
            "Merge branch 'main' into topic",
            "Revert \"feat: add a view\"",
            "fixup! docs: describe the graph view",
            "squash! anything at all, even longer than the limit of seventy-two characters",
        ] {
            assert_eq!(problem(message), None, "{message}");
        }
        assert!(problem("Merged the branch").is_some());
        assert!(problem("fixup!docs: no space").is_some());
    }

    #[test]
    fn skips_comment_and_blank_lines_before_the_header() {
        let message = "# Please enter the commit message\n\n \t\r\ndocs: describe it\n# trailing";
        assert_eq!(header(message), Some("docs: describe it"));
        assert_eq!(problem(message), None);
        assert_eq!(header("#only\n\n"), None);
        assert_eq!(
            problem("# only a comment\n"),
            Some("the message has no header".to_string())
        );
        assert_eq!(problem(""), Some("the message has no header".to_string()));
        // A line that starts with white space before `#` is not a comment.
        assert!(problem(" # docs: x").is_some());
    }

    #[test]
    fn rejects_headers_that_do_not_follow_the_form() {
        for message in [
            "Add a view",
            "feature: add a view",
            "Docs: capitalized type",
            "docs:missing space",
            "docs:  two spaces",
            "docs: ",
            "docs()",
            "docs(): empty scope",
            "docs(Scope): upper case scope",
            "docs(scope: unclosed",
            "docs(a b): space in scope",
            "docs!!: twice",
            "docs (scope): space before scope",
            "docs\u{b}: vertical tab",
            "docs: \tdescription starting with a tab",
        ] {
            let found = problem(message).unwrap_or_default();
            assert!(
                found.contains("does not follow Conventional Commits"),
                "{message}: {found}"
            );
        }
    }

    #[test]
    fn limits_the_header_to_72_characters() {
        let at_limit = format!("docs: {}", "x".repeat(MAX_HEADER - 6));
        assert_eq!(at_limit.chars().count(), MAX_HEADER);
        assert_eq!(problem(&at_limit), None);
        let over = format!("{at_limit}y");
        assert!(
            problem(&over).is_some_and(|found| found.contains("is 73 characters")),
            "{over}"
        );
        // Characters are counted, not bytes.
        let wide = format!("docs: {}", "\u{3be}".repeat(MAX_HEADER - 6));
        assert_eq!(problem(&wide), None);
        // Only the header counts.
        assert_eq!(problem(&format!("docs: x\n\n{}", "y".repeat(200))), None);
    }

    #[test]
    fn the_base_comes_from_the_variable_unless_it_is_unset_or_zero() {
        assert_eq!(base(Some("abc123")), Base::Given("abc123"));
        assert_eq!(base(None), Base::MergeBase);
        assert_eq!(base(Some("")), Base::MergeBase);
        assert_eq!(base(Some(&"0".repeat(40))), Base::MergeBase);
        assert_eq!(base(Some("0001")), Base::Given("0001"));
    }

    #[test]
    fn parses_git_log_records() {
        let log = b"aa11\ndocs: one\n\nbody\n\0bb22\nfeat: two\xff\n\0cc33\0";
        assert_eq!(
            parse_log(log).unwrap(),
            [
                Commit {
                    id: "aa11",
                    message: Some("docs: one\n\nbody\n"),
                },
                Commit {
                    id: "bb22",
                    message: None,
                },
                Commit {
                    id: "cc33",
                    message: Some(""),
                },
            ]
        );
        assert_eq!(parse_log(b"").unwrap(), []);
        assert!(parse_log(b"not an id\nx\0").is_err());
    }

    /// A Git repository in this crate's ignored build directory, with Git
    /// configuration from outside the repository turned off.
    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join(format!("commits-test-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            let scratch = Scratch { dir };
            scratch.git(&["init", "--quiet"], None);
            scratch
        }

        fn git(&self, args: &[&str], input: Option<&str>) -> String {
            let mut child = Command::new("git")
                .args(args)
                .current_dir(&self.dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@example.org")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@example.org")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut stdin = child.stdin.take().unwrap();
            stdin
                .write_all(input.unwrap_or_default().as_bytes())
                .unwrap();
            drop(stdin);
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success(), "git {args:?}");
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        }

        /// Adds a commit with `message` on top of `parent` and moves the
        /// current branch to it. The hook does not run for plumbing commands.
        fn commit(&self, parent: Option<&str>, message: &str) -> String {
            let tree = self.git(&["write-tree"], None);
            let mut args = vec!["commit-tree", "--no-gpg-sign", tree.as_str()];
            if let Some(parent) = parent {
                args.extend(["-p", parent]);
            }
            let id = self.git(&args, Some(message));
            self.git(&["update-ref", "HEAD", &id], None);
            id
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn checked(scratch: &Scratch, given: Option<&str>) -> (Result<(), String>, Facts) {
        let mut facts = Facts::new();
        let result = check_range(&scratch.dir, given, &mut facts);
        (result, facts)
    }

    fn fact<'a>(facts: &'a Facts, key: &str) -> Option<&'a str> {
        facts
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn checks_the_commits_after_the_base() {
        let scratch = Scratch::new("range");
        let first = scratch.commit(None, "docs: initialize the repository\n");
        scratch.git(&["update-ref", "refs/remotes/origin/main", &first], None);
        let good = scratch.commit(Some(&first), "feat(xtask): add a step\n");
        let bad = scratch.commit(Some(&good), "Add a step without a type\n");
        let merge = scratch.commit(Some(&bad), "Merge branch 'topic'\n");

        // Without a base, the range starts at the merge base with origin/main.
        let (result, facts) = checked(&scratch, None);
        let detail = result.expect_err("the bad header fails");
        assert_eq!(detail.lines().count(), 1, "{detail}");
        assert!(
            detail.starts_with(&format!("commit {}: header `Add a step", &bad[..12])),
            "{detail}"
        );
        assert_eq!(fact(&facts, "base"), Some(first.as_str()));
        assert_eq!(
            fact(&facts, "base_from"),
            Some("merge base of origin/main and HEAD")
        );
        assert_eq!(fact(&facts, "commits"), Some("3"));

        // An all-zero base also falls back to the merge base.
        let (result, _) = checked(&scratch, Some(&"0".repeat(40)));
        assert!(result.is_err());

        // A given base limits the range.
        let (result, facts) = checked(&scratch, Some(&bad));
        assert_eq!(result, Ok(()));
        assert_eq!(fact(&facts, "base"), Some(bad.as_str()));
        assert_eq!(fact(&facts, "base_from"), Some(BASE_VARIABLE));
        assert_eq!(fact(&facts, "commits"), Some("1"));

        // An empty range passes.
        let (result, facts) = checked(&scratch, Some(&merge));
        assert_eq!(result, Ok(()));
        assert_eq!(fact(&facts, "commits"), Some("0"));
    }

    #[test]
    fn rejects_a_base_that_names_no_commit() {
        let scratch = Scratch::new("bad-base");
        scratch.commit(None, "docs: initialize the repository\n");
        let (result, _) = checked(&scratch, Some("no-such-ref"));
        assert_eq!(
            result,
            Err("CHECK_BASE=`no-such-ref` does not name a commit in this repository".to_string())
        );
        // Without origin/main there is no merge base either.
        let (result, facts) = checked(&scratch, None);
        let detail = result.expect_err("there is no merge base");
        assert!(
            detail.starts_with("cannot find the merge base of origin/main and HEAD"),
            "{detail}"
        );
        assert_eq!(fact(&facts, "base"), None);
    }
}
