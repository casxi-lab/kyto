//! Line patterns: regular expressions matched against the content of one
//! line.
//!
//! A pattern uses the syntax of the `regex` crate on bytes, compiled with
//! Unicode mode off: `\d`, `\s` and `\w` are ASCII classes, `.` is any byte
//! but LF, and case-insensitive matching folds ASCII letters only; any other
//! character matches its UTF-8 bytes as written. A pattern that needs a
//! Unicode class, such as `\pL`, or Unicode case folding, such as `(?iu)k`,
//! does not compile. A pattern matches anywhere in the line unless it is
//! anchored with `^` or `$`. Its compiled form is limited to [`SIZE_LIMIT`]
//! bytes.

use std::fmt;
use std::ops::Range;

use regex::bytes::{Regex, RegexBuilder};

/// The largest compiled size of a pattern, in bytes.
pub const SIZE_LIMIT: usize = 1 << 20;

/// A compiled line pattern with its source text.
#[derive(Clone, Debug)]
pub struct LinePattern {
    source: String,
    regex: Regex,
}

/// Why a pattern cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatternError {
    /// The pattern does not compile, or its compiled form is too large.
    Invalid { pattern: String, reason: String },
    /// The pattern lacks a named group that its use needs.
    MissingGroup { pattern: String, group: String },
    /// The pattern does not match its example line.
    ExampleDoesNotMatch { pattern: String, example: String },
}

impl LinePattern {
    /// Compiles `source`.
    pub fn new(source: &str) -> Result<Self, PatternError> {
        Self::build(source, false)
    }

    /// Compiles `source` so that it ignores the case of ASCII letters.
    pub fn new_ignoring_ascii_case(source: &str) -> Result<Self, PatternError> {
        Self::build(source, true)
    }

    /// Compiles `source` and checks that it has every group in `groups` and
    /// matches each of `examples`, the lines that document the form it
    /// reads.
    pub fn with_examples(
        source: &str,
        examples: &[String],
        groups: &[&str],
    ) -> Result<Self, PatternError> {
        let pattern = Self::new(source)?;
        pattern.require_groups(groups)?;
        for example in examples {
            pattern.check_example(example)?;
        }
        Ok(pattern)
    }

    fn build(source: &str, ignore_ascii_case: bool) -> Result<Self, PatternError> {
        let regex = RegexBuilder::new(source)
            .unicode(false)
            .case_insensitive(ignore_ascii_case)
            .size_limit(SIZE_LIMIT)
            .build()
            .map_err(|error| PatternError::Invalid {
                pattern: source.to_string(),
                reason: error.to_string(),
            })?;
        Ok(Self {
            source: source.to_string(),
            regex,
        })
    }

    /// The source text of the pattern.
    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// Whether the pattern matches somewhere in `line`.
    pub fn is_match(&self, line: &[u8]) -> bool {
        self.regex.is_match(line)
    }

    /// The span of the first match in `line`.
    pub fn find(&self, line: &[u8]) -> Option<Range<usize>> {
        self.regex.find(line).map(|found| found.range())
    }

    /// The named groups of the first match in `line` that took part in it,
    /// each with its span, in the order of the pattern; `None` when the
    /// pattern does not match.
    pub fn named_groups<'p>(&'p self, line: &[u8]) -> Option<Vec<(&'p str, Range<usize>)>> {
        let captures = self.regex.captures(line)?;
        Some(
            self.regex
                .capture_names()
                .flatten()
                .filter_map(|name| captures.name(name).map(|group| (name, group.range())))
                .collect(),
        )
    }

    /// The bytes of the group `name` in the first match in `line`; `None`
    /// when the pattern does not match or the group took no part in the
    /// match.
    pub fn group<'l>(&self, line: &'l [u8], name: &str) -> Option<&'l [u8]> {
        let captures = self.regex.captures(line)?;
        captures.name(name).map(|group| group.as_bytes())
    }

    /// The names of the pattern's named groups, in the order of the pattern.
    pub fn group_names(&self) -> Vec<&str> {
        self.regex.capture_names().flatten().collect()
    }

    /// Whether the pattern has a group called `name`.
    pub fn has_group(&self, name: &str) -> bool {
        self.regex
            .capture_names()
            .flatten()
            .any(|group| group == name)
    }

    /// Checks that the pattern has every group in `groups`.
    pub fn require_groups(&self, groups: &[&str]) -> Result<(), PatternError> {
        match groups.iter().find(|group| !self.has_group(group)) {
            None => Ok(()),
            Some(group) => Err(PatternError::MissingGroup {
                pattern: self.source.clone(),
                group: (*group).to_string(),
            }),
        }
    }

    /// Checks that the pattern matches `example`.
    pub fn check_example(&self, example: &str) -> Result<(), PatternError> {
        if self.is_match(example.as_bytes()) {
            Ok(())
        } else {
            Err(PatternError::ExampleDoesNotMatch {
                pattern: self.source.clone(),
                example: example.to_string(),
            })
        }
    }
}

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { pattern, reason } => {
                write!(f, "the pattern '{pattern}' cannot be used: {reason}")
            }
            Self::MissingGroup { pattern, group } => {
                write!(f, "the pattern '{pattern}' has no group named {group}")
            }
            Self::ExampleDoesNotMatch { pattern, example } => {
                write!(f, "the pattern '{pattern}' does not match '{example}'")
            }
        }
    }
}

impl std::error::Error for PatternError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(source: &str) -> LinePattern {
        LinePattern::new(source).unwrap()
    }

    fn invalid(source: &str) -> bool {
        matches!(LinePattern::new(source), Err(PatternError::Invalid { .. }))
    }

    #[test]
    fn patterns_match_anywhere_unless_anchored() {
        assert!(pattern("Boot mode").is_match(b"  4.94 Boot mode: NETWORK (02)"));
        assert!(!pattern("^Boot mode").is_match(b"  4.94 Boot mode: NETWORK (02)"));
        assert!(pattern("order f1$").is_match(b"Boot mode: NETWORK (02) order f1"));
        assert!(!pattern("order f$").is_match(b"Boot mode: NETWORK (02) order f1"));
        assert_eq!(pattern("[0-9]+").find(b"ab 123 4"), Some(3..6));
    }

    #[test]
    fn classes_and_dot_are_ascii_and_bytes() {
        assert!(pattern(r"^\d$").is_match(b"7"));
        assert!(!pattern(r"^\d$").is_match("\u{0663}".as_bytes()));
        assert!(!pattern(r"^\s$").is_match("\u{a0}".as_bytes()));
        assert!(pattern(r"^\w+$").is_match(b"abc_09"));
        assert!(!pattern(r"^\w+$").is_match("\u{e9}".as_bytes()));
        // `.` is one byte, any but LF, including bytes that are not UTF-8.
        assert!(pattern("^.$").is_match(b"\xff"));
        assert!(!pattern("^.$").is_match("\u{e9}".as_bytes()));
        assert!(pattern("^..$").is_match("\u{e9}".as_bytes()));
        assert!(!pattern("^.$").is_match(b"\n"));
        assert!(pattern(r"^\xff$").is_match(b"\xff"));
    }

    #[test]
    fn unicode_classes_and_case_folding_do_not_compile() {
        assert!(invalid(r"\pL"));
        assert!(invalid(r"\p{Greek}"));
        assert!(invalid(r"(?u)\w"));
        assert!(invalid(r"(?u)\d"));
        assert!(invalid(r"(?u)\s"));
        assert!(invalid("(?iu)k"));
        // Turning Unicode mode on is not an error by itself: `(?u).` is one
        // UTF-8 encoded character.
        assert!(pattern("^(?u).$").is_match("\u{e9}".as_bytes()));
        assert!(!pattern("^(?u).$").is_match(b"\xff"));
    }

    #[test]
    fn characters_other_than_ascii_letters_match_as_written() {
        let accented = pattern("(?i)caf\u{e9}");
        assert!(accented.is_match("CAF\u{e9}".as_bytes()));
        assert!(!accented.is_match("CAF\u{c9}".as_bytes()));
    }

    #[test]
    fn case_insensitive_patterns_fold_ascii_letters_only() {
        let txt = LinePattern::new_ignoring_ascii_case(r"\.txt$").unwrap();
        assert!(txt.is_match(b"config.txt"));
        assert!(txt.is_match(b"CONFIG.TXT"));
        assert!(!txt.is_match(b"config.txt.bak"));
        assert!(pattern("(?i)network").is_match(b"NETWORK"));
        assert!(!pattern("network").is_match(b"NETWORK"));
    }

    #[test]
    fn invalid_and_oversized_patterns_are_refused() {
        assert!(invalid("("));
        assert!(invalid("[a-"));
        assert!(invalid("(?P<x>a)(?P<x>b)"));
        assert!(LinePattern::new(r"(?:a{100}){100}").is_ok());
        let oversized = LinePattern::new(r"(?:a{1000}){1000}").unwrap_err();
        assert!(oversized.to_string().contains("size limit"), "{oversized}");
        let error = LinePattern::new("(").unwrap_err().to_string();
        assert!(
            error.starts_with("the pattern '(' cannot be used: "),
            "{error}"
        );
    }

    #[test]
    fn named_groups_give_their_spans_and_bytes() {
        let read = pattern(r"^Read (?P<path>[^ ]+) bytes +(?P<bytes>[0-9]+)(?P<tail> x)?$");
        let line = b"Read bench/config.txt bytes      246";
        assert_eq!(read.group(line, "path"), Some(&b"bench/config.txt"[..]));
        assert_eq!(read.group(line, "bytes"), Some(&b"246"[..]));
        assert_eq!(read.group(line, "tail"), None);
        assert_eq!(read.group(line, "other"), None);
        assert_eq!(
            read.named_groups(line),
            Some(vec![("path", 5..21), ("bytes", 33..36)])
        );
        assert_eq!(read.named_groups(b"nothing"), None);
        assert!(read.has_group("tail"));
        assert!(!read.has_group("value"));
        assert_eq!(read.group_names(), ["path", "bytes", "tail"]);
        assert!(pattern("(a)(b)").group_names().is_empty());
    }

    #[test]
    fn a_pattern_with_examples_needs_its_groups_and_must_match_them() {
        let source = r"^ *[0-9]+\.[0-9]+ MFG_VER: (?P<value>[0-9]+)$";
        let examples =
            |lines: &[&str]| -> Vec<String> { lines.iter().map(|line| line.to_string()).collect() };
        let example = examples(&["  4.37 MFG_VER: 2"]);
        let checked = LinePattern::with_examples(source, &example, &["value"]).unwrap();
        assert_eq!(checked.as_str(), source);
        assert_eq!(
            LinePattern::with_examples(source, &example, &["value", "hash"]).unwrap_err(),
            PatternError::MissingGroup {
                pattern: source.to_string(),
                group: "hash".to_string()
            }
        );
        assert_eq!(
            LinePattern::with_examples(
                source,
                &examples(&["  4.37 MFG_VER: 2", "MFG_VER: 2"]),
                &["value"]
            )
            .unwrap_err(),
            PatternError::ExampleDoesNotMatch {
                pattern: source.to_string(),
                example: "MFG_VER: 2".to_string()
            }
        );
    }
}
