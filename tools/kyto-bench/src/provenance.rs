//! The revision of the tool, as its build embedded it.

/// The commit the tool was built from and whether the working tree was
/// clean, untracked files and changes hidden from `git status` included;
/// `None` when the build could not tell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolRevision {
    /// The commit, as 40 lowercase hexadecimal digits.
    pub commit: Option<String>,
    /// Whether the working tree had no change and no untracked file, as the
    /// build script tells them.
    pub clean: Option<bool>,
}

impl ToolRevision {
    /// The revision that the build script embedded in this build. Nothing at
    /// run time overrides it.
    pub fn built() -> Self {
        Self::from_build(
            option_env!("KYTO_BENCH_COMMIT"),
            option_env!("KYTO_BENCH_CLEAN"),
        )
    }

    /// The revision from the values that the build script writes: a commit
    /// of 40 lowercase hexadecimal digits, and `true` or `false`. A value
    /// that is missing or has another form is unknown.
    pub fn from_build(commit: Option<&str>, clean: Option<&str>) -> Self {
        let commit = commit
            .filter(|commit| is_commit(commit))
            .map(str::to_string);
        let clean = match clean {
            Some("true") => Some(true),
            Some("false") => Some(false),
            _ => None,
        };
        Self { commit, clean }
    }
}

fn is_commit(text: &str) -> bool {
    text.len() == 40
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn provenance_embeds_the_commit_and_clean_flag() {
        // The tests are built from a Git checkout, so the build script can
        // read both values.
        let built = ToolRevision::built();
        let commit = built
            .commit
            .as_deref()
            .expect("the build embeds the commit");
        assert!(is_commit(commit), "{commit}");
        assert!(built.clean.is_some(), "the build embeds the clean flag");
        assert_eq!(
            built,
            ToolRevision::from_build(
                option_env!("KYTO_BENCH_COMMIT"),
                option_env!("KYTO_BENCH_CLEAN")
            )
        );
    }

    #[test]
    fn the_build_script_runs_on_every_build_and_gives_both_values() {
        // Cargo keeps what the build script printed for this build in
        // `output`, next to the script's output directory.
        let output = Path::new(env!("OUT_DIR")).with_file_name("output");
        let text = std::fs::read_to_string(&output).expect("the build script's output");
        let (mut watched, mut values) = (Vec::new(), Vec::new());
        for line in text.lines() {
            if let Some(path) = line.strip_prefix("cargo::rerun-if-changed=") {
                watched.push(path);
            } else if let Some(value) = line.strip_prefix("cargo::rustc-env=") {
                values.push(value);
            }
        }
        // Cargo runs the script again on every build while a path that it
        // watches does not exist.
        assert!(!watched.is_empty(), "{text}");
        for path in watched {
            assert!(!Path::new(path).exists(), "{path}");
        }
        // Both values are given, once each, and are those of this build.
        let given = [
            format!("KYTO_BENCH_COMMIT={}", env!("KYTO_BENCH_COMMIT")),
            format!("KYTO_BENCH_CLEAN={}", env!("KYTO_BENCH_CLEAN")),
        ];
        assert_eq!(values, given);
    }

    #[test]
    fn build_values_are_read_strictly() {
        let read = ToolRevision::from_build;
        assert_eq!(
            read(Some(COMMIT), Some("true")),
            ToolRevision {
                commit: Some(COMMIT.to_string()),
                clean: Some(true)
            }
        );
        assert_eq!(read(Some(COMMIT), Some("false")).clean, Some(false));
        assert_eq!(
            read(None, None),
            ToolRevision {
                commit: None,
                clean: None
            }
        );
        let upper = COMMIT.to_ascii_uppercase();
        let short = &COMMIT[..39];
        let long = format!("{COMMIT}0");
        let spaced = format!("{COMMIT}\n");
        for commit in [upper.as_str(), short, long.as_str(), spaced.as_str(), ""] {
            assert_eq!(read(Some(commit), None).commit, None, "{commit:?}");
        }
        for clean in ["True", "1", "yes", "", "true\n"] {
            assert_eq!(read(None, Some(clean)).clean, None, "{clean:?}");
        }
    }
}
