//! What every document of the tool has in common: the two keys that name its
//! format, and the forms of names and lines of text.

use std::fmt;

use serde::Deserialize;

/// The `format` and `format_version` of a document. They are read before the
/// rest, so that a document of another format or version is named as such.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Header {
    pub format: String,
    pub format_version: i64,
}

impl Header {
    /// Takes the header out of the table of a document.
    pub(crate) fn take(table: &mut toml::Table) -> Result<Self, toml::de::Error> {
        let keys = ["format", "format_version"].map(|key| table.remove_entry(key));
        toml::Table::from_iter(keys.into_iter().flatten()).try_into()
    }

    /// Whether the document is of `format` and `version`.
    pub fn is(&self, format: &str, version: i64) -> bool {
        self.format == format && self.format_version == version
    }
}

impl fmt::Display for Header {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"{}\" version {}", self.format, self.format_version)
    }
}

/// Whether `text` is a name: a lowercase ASCII letter, then lowercase
/// letters, digits, `_` and `-`, at most 64 bytes.
pub(crate) fn is_name(text: &str) -> bool {
    let mut bytes = text.bytes();
    text.len() <= 64
        && bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
}

/// Whether `text` is one line of text: not empty, without control characters
/// and without white space at either end.
pub(crate) fn is_line_text(text: &str) -> bool {
    !text.is_empty() && !text.chars().any(char::is_control) && text.trim() == text
}

/// The reason given when a value is not a name.
pub(crate) const NOT_A_NAME: &str =
    "a name is a lowercase ASCII letter, then lowercase letters, digits, _ and -";

/// Defines an enum whose values have names in documents, with `ALL`, every
/// value in order, `name()`, the name of a value, and `Display` as the name.
macro_rules! named_enum {
    (
        $(#[$meta:meta])*
        pub enum $ty:ident { $($(#[$doc:meta])* $value:ident = $name:literal,)* }
    ) => {
        $(#[$meta])*
        pub enum $ty { $($(#[$doc])* $value,)* }

        impl $ty {
            /// Every value, in order.
            pub const ALL: [$ty; [$($name),*].len()] = [$($ty::$value),*];

            /// The name of the value in documents.
            pub fn name(self) -> &'static str {
                match self { $($ty::$value => $name,)* }
            }
        }

        impl std::fmt::Display for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.name())
            }
        }
    };
}
pub(crate) use named_enum;

/// Returns `Err(invalid(at, rule))`, with the `invalid` of the calling
/// module, for the first of the listed values that breaks its rule, each
/// written `at => kept, rule;`.
macro_rules! check {
    ($($at:expr => $kept:expr, $rule:expr;)+) => {
        $(if !$kept {
            return Err(invalid($at, $rule));
        })+
    };
}
pub(crate) use check;
