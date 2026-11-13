//! The revision of the tool, as its build embedded it.

/// The commit the tool was built from and whether the working tree was
/// clean, untracked files included; `None` when the build could not tell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolRevision {
    /// The commit, as 40 lowercase hexadecimal digits.
    pub commit: Option<String>,
    /// Whether the working tree had no change and no untracked file.
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
