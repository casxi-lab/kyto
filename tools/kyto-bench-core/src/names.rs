//! Names of payload files and of staged generations.

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;

/// The name of a payload file inside a generation: a relative path of one or
/// more segments separated by `/`. A segment is made of ASCII letters,
/// digits, `.`, `_` and `-`, and is neither `.` nor `..`. Names are at most
/// [`PayloadName::MAX_LEN`] bytes long.
///
/// TFTP directories on case-insensitive file systems serve two names that
/// differ only in ASCII case as one file, and no directory holds a file and
/// a directory of the same name, so the names of one payload must differ
/// ignoring ASCII case and none may name a directory of another
/// ([`check_distinct`]).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(try_from = "String")]
pub struct PayloadName(String);

/// Why a text is not a payload name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameError {
    /// The text is empty.
    Empty,
    /// The text is longer than [`PayloadName::MAX_LEN`] bytes.
    TooLong { len: usize },
    /// The text starts with `/`.
    Absolute,
    /// Two `/` follow each other, or the text ends with `/`.
    EmptySegment,
    /// A segment is `.` or `..`.
    DotSegment,
    /// A byte that is not an ASCII letter or digit, `.`, `_`, `-` or `/`.
    Character { byte: u8 },
}

impl PayloadName {
    /// The longest payload name, in bytes.
    pub const MAX_LEN: usize = 255;

    /// Reads a payload name.
    pub fn parse(text: &str) -> Result<Self, NameError> {
        if text.is_empty() {
            return Err(NameError::Empty);
        }
        if text.len() > Self::MAX_LEN {
            return Err(NameError::TooLong { len: text.len() });
        }
        if text.starts_with('/') {
            return Err(NameError::Absolute);
        }
        if let Some(byte) = text
            .bytes()
            .find(|byte| !is_name_byte(*byte) && *byte != b'/')
        {
            return Err(NameError::Character { byte });
        }
        for segment in text.split('/') {
            if segment.is_empty() {
                return Err(NameError::EmptySegment);
            }
            if segment == "." || segment == ".." {
                return Err(NameError::DotSegment);
            }
        }
        Ok(Self(text.to_string()))
    }

    /// The name as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The last segment: the file name without its directories.
    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// Whether the two names are the same ignoring ASCII case.
    pub fn eq_ignore_ascii_case(&self, other: &PayloadName) -> bool {
        self.0.eq_ignore_ascii_case(&other.0)
    }
}

/// Whether `text` is one segment of a payload name, such as a file name
/// without directories.
pub fn is_segment(text: &str) -> bool {
    !text.contains('/') && PayloadName::parse(text).is_ok()
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
}

impl fmt::Display for PayloadName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for PayloadName {
    type Error = NameError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text)
    }
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "a payload name is not empty"),
            Self::TooLong { len } => write!(f, "a payload name is at most 255 bytes, not {len}"),
            Self::Absolute => write!(f, "a payload name does not start with /"),
            Self::EmptySegment => write!(f, "a payload name has no empty segment"),
            Self::DotSegment => write!(f, "a payload name has no . or .. segment"),
            Self::Character { byte } => write!(f, "a payload name has no byte 0x{byte:02x}"),
        }
    }
}

impl std::error::Error for NameError {}

/// Two names of one payload that one directory cannot hold as two files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameClash {
    /// The names are the same ignoring ASCII case.
    SameIgnoringCase {
        first: PayloadName,
        second: PayloadName,
    },
    /// `file` is, ignoring ASCII case, a directory of `inside`, such as
    /// `overlays` and `overlays/a.dtbo`.
    FileIsDirectory {
        file: PayloadName,
        inside: PayloadName,
    },
}

impl fmt::Display for NameClash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SameIgnoringCase { first, second } => {
                write!(
                    f,
                    "the payload names {first} and {second} differ only in case"
                )
            }
            Self::FileIsDirectory { file, inside } => write!(
                f,
                "the payload name {file} is, ignoring ASCII case, a directory of {inside}"
            ),
        }
    }
}

impl std::error::Error for NameClash {}

/// Checks that one directory can hold `names` as files: they differ from
/// each other ignoring ASCII case, and none is, ignoring ASCII case, one of
/// the directories of another. The first clash, in the order given, is the
/// error.
pub fn check_distinct<'a>(
    names: impl IntoIterator<Item = &'a PayloadName>,
) -> Result<(), NameClash> {
    let mut files: BTreeMap<String, &PayloadName> = BTreeMap::new();
    let mut directories: BTreeMap<String, &PayloadName> = BTreeMap::new();
    for name in names {
        let lower = name.as_str().to_ascii_lowercase();
        if let Some(first) = files.get(&lower) {
            return Err(NameClash::SameIgnoringCase {
                first: (*first).clone(),
                second: name.clone(),
            });
        }
        if let Some(inside) = directories.get(&lower) {
            return Err(NameClash::FileIsDirectory {
                file: name.clone(),
                inside: (*inside).clone(),
            });
        }
        let own: Vec<&str> = lower
            .match_indices('/')
            .map(|(end, _)| &lower[..end])
            .collect();
        if let Some(file) = own.iter().find_map(|directory| files.get(*directory)) {
            return Err(NameClash::FileIsDirectory {
                file: (*file).clone(),
                inside: name.clone(),
            });
        }
        for directory in own {
            directories.entry(directory.to_string()).or_insert(name);
        }
        files.insert(lower, name);
    }
    Ok(())
}

/// Why the path of a file read over TFTP does not give a payload name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadPathError {
    /// The path does not start with the path prefix.
    OutsidePrefix,
    /// What follows the prefix is not a payload name.
    NotAName(NameError),
}

/// The payload name of a path that the board read over TFTP. The path must
/// start with `prefix`, byte for byte; the rest, with each run of `/`
/// collapsed to one and a leading `/` removed, is the payload name.
pub fn read_path_name(path: &[u8], prefix: &[u8]) -> Result<PayloadName, ReadPathError> {
    let rest = path
        .strip_prefix(prefix)
        .ok_or(ReadPathError::OutsidePrefix)?;
    let mut collapsed = Vec::with_capacity(rest.len());
    for &byte in rest {
        if byte == b'/' && matches!(collapsed.last(), None | Some(b'/')) {
            continue;
        }
        collapsed.push(byte);
    }
    let text = String::from_utf8(collapsed).map_err(|error| {
        let byte = error.as_bytes()[error.utf8_error().valid_up_to()];
        ReadPathError::NotAName(NameError::Character { byte })
    })?;
    PayloadName::parse(&text).map_err(ReadPathError::NotAName)
}

/// The name of a staged generation: `g` followed by six decimal digits. The
/// tool gives each new generation the number after the highest one present
/// and never reuses a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GenerationName(u32);

/// Why a text is not a generation name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerationNameError;

impl GenerationName {
    /// The highest generation number.
    pub const MAX: u32 = 999_999;

    /// The generation with `number`, if it is at most [`GenerationName::MAX`].
    pub fn new(number: u32) -> Option<Self> {
        (number <= Self::MAX).then_some(Self(number))
    }

    /// Reads a generation name: `g` and exactly six decimal digits.
    pub fn parse(text: &str) -> Result<Self, GenerationNameError> {
        let digits = text.strip_prefix('g').ok_or(GenerationNameError)?;
        if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(GenerationNameError);
        }
        digits
            .parse()
            .ok()
            .and_then(Self::new)
            .ok_or(GenerationNameError)
    }

    /// The name of the next new generation: the number after the highest of
    /// `existing`, or 1 when there is none. `None` when the highest is
    /// [`GenerationName::MAX`].
    pub fn next_after(existing: impl IntoIterator<Item = GenerationName>) -> Option<Self> {
        match existing.into_iter().max() {
            None => Self::new(1),
            Some(highest) => Self::new(highest.0 + 1),
        }
    }
}

impl fmt::Display for GenerationName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "g{:06}", self.0)
    }
}

impl fmt::Display for GenerationNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a generation name is g and six decimal digits")
    }
}

impl std::error::Error for GenerationNameError {}

#[cfg(test)]
mod tests {
    use serde::de::IntoDeserializer;
    use serde::de::value::{Error as ValueError, StrDeserializer};

    use super::*;

    fn name(text: &str) -> PayloadName {
        PayloadName::parse(text).unwrap()
    }

    #[test]
    fn payload_names_are_relative_paths_of_plain_segments() {
        for text in [
            "config.txt",
            "bcm2712-rpi-5-b.dtb",
            "overlays/bcm2712d0.dtbo",
            "a/b/c_d-e.F",
            ".hidden",
            "...",
        ] {
            assert_eq!(name(text).as_str(), text);
        }
        let longest = "a".repeat(PayloadName::MAX_LEN);
        assert!(PayloadName::parse(&longest).is_ok());
    }

    #[test]
    fn payload_names_reject_every_other_form() {
        let rejected = [
            ("", NameError::Empty),
            ("/config.txt", NameError::Absolute),
            ("overlays//x.dtbo", NameError::EmptySegment),
            ("overlays/", NameError::EmptySegment),
            (".", NameError::DotSegment),
            ("..", NameError::DotSegment),
            ("a/../b", NameError::DotSegment),
            ("a/./b", NameError::DotSegment),
            ("a b", NameError::Character { byte: b' ' }),
            ("a\\b", NameError::Character { byte: b'\\' }),
            ("a:b", NameError::Character { byte: b':' }),
            ("caf\u{e9}", NameError::Character { byte: 0xc3 }),
            ("a\nb", NameError::Character { byte: b'\n' }),
        ];
        for (text, error) in rejected {
            assert_eq!(PayloadName::parse(text), Err(error), "{text:?}");
        }
        let too_long = "a".repeat(PayloadName::MAX_LEN + 1);
        assert_eq!(
            PayloadName::parse(&too_long),
            Err(NameError::TooLong {
                len: PayloadName::MAX_LEN + 1
            })
        );
    }

    #[test]
    fn the_file_name_is_the_last_segment() {
        assert_eq!(name("config.txt").file_name(), "config.txt");
        assert_eq!(
            name("overlays/bcm2712d0.dtbo").file_name(),
            "bcm2712d0.dtbo"
        );
        assert!(is_segment("pieeprom.upd"));
        assert!(!is_segment("overlays/x.dtbo"));
        assert!(!is_segment(".."));
        assert!(!is_segment(""));
    }

    #[test]
    fn payload_names_must_differ_ignoring_case() {
        let names = [name("config.txt"), name("kernel.img"), name("CONFIG.TXT")];
        assert_eq!(
            check_distinct(&names),
            Err(NameClash::SameIgnoringCase {
                first: name("config.txt"),
                second: name("CONFIG.TXT")
            })
        );
        assert!(names[0].eq_ignore_ascii_case(&names[2]));
        assert!(check_distinct(&names[..2]).is_ok());
        assert!(check_distinct(&[name("a/b"), name("b/a")]).is_ok());
        // A file cannot share its name with a directory, in either order and
        // in any case.
        let clash = |file: &str, inside: &str| {
            Err(NameClash::FileIsDirectory {
                file: name(file),
                inside: name(inside),
            })
        };
        assert_eq!(
            check_distinct(&[name("overlays"), name("overlays/a.dtbo")]),
            clash("overlays", "overlays/a.dtbo")
        );
        assert_eq!(
            check_distinct(&[name("Overlays/a.dtbo"), name("overlays")]),
            clash("overlays", "Overlays/a.dtbo")
        );
        assert_eq!(
            check_distinct(&[name("a/b"), name("A/B/c")]),
            clash("a/b", "A/B/c")
        );
        assert_eq!(
            clash("overlays", "overlays/a.dtbo")
                .unwrap_err()
                .to_string(),
            "the payload name overlays is, ignoring ASCII case, a directory of overlays/a.dtbo"
        );
        assert!(
            check_distinct(&[
                name("overlays/a.dtbo"),
                name("overlays/b.dtbo"),
                name("overlays.txt"),
                name("a/b/c"),
                name("a/bc")
            ])
            .is_ok()
        );
    }

    #[test]
    fn tftp_paths_are_normalized_after_the_instance_prefix() {
        let read = |path: &str, prefix: &str| read_path_name(path.as_bytes(), prefix.as_bytes());
        assert_eq!(
            read("bench//overlays/x.dtbo", "bench/"),
            Ok(name("overlays/x.dtbo"))
        );
        assert_eq!(read("bench/config.txt", "bench/"), Ok(name("config.txt")));
        assert_eq!(read("/config.txt", ""), Ok(name("config.txt")));
        assert_eq!(read("bench//a///b", "bench/"), Ok(name("a/b")));
        assert_eq!(read("config.txt", ""), Ok(name("config.txt")));
        assert_eq!(
            read("other/config.txt", "bench/"),
            Err(ReadPathError::OutsidePrefix)
        );
        assert_eq!(
            read("Bench/config.txt", "bench/"),
            Err(ReadPathError::OutsidePrefix)
        );
        assert_eq!(
            read("bench/", "bench/"),
            Err(ReadPathError::NotAName(NameError::Empty))
        );
        assert_eq!(
            read("bench/../x", "bench/"),
            Err(ReadPathError::NotAName(NameError::DotSegment))
        );
        assert_eq!(
            read_path_name(b"bench/\xffx", b"bench/"),
            Err(ReadPathError::NotAName(NameError::Character { byte: 0xff }))
        );
    }

    #[test]
    fn payload_names_are_read_from_document_strings() {
        let read = |text: &str| {
            let de: StrDeserializer<'_, ValueError> = text.into_deserializer();
            PayloadName::deserialize(de)
        };
        assert_eq!(read("overlays/x.dtbo").unwrap(), name("overlays/x.dtbo"));
        assert!(read("../x").is_err());
    }

    #[test]
    fn generation_names_are_g_and_six_digits() {
        let first = GenerationName::new(1).unwrap();
        assert_eq!(first.to_string(), "g000001");
        assert_eq!(GenerationName::parse("g000001"), Ok(first));
        assert_eq!(
            GenerationName::parse("g999999"),
            Ok(GenerationName::new(999_999).unwrap())
        );
        assert_eq!(GenerationName::new(GenerationName::MAX + 1), None);
        for text in [
            "", "g", "g1", "g0000001", "G000001", "g00000a", "g+00001", "x000001",
        ] {
            assert_eq!(
                GenerationName::parse(text),
                Err(GenerationNameError),
                "{text:?}"
            );
        }
    }

    #[test]
    fn the_next_generation_follows_the_highest_and_never_reuses_a_name() {
        let number = |number| GenerationName::new(number).unwrap();
        assert_eq!(GenerationName::next_after([]), Some(number(1)));
        assert_eq!(
            GenerationName::next_after([number(1), number(2)]),
            Some(number(3))
        );
        // A gap left by a removed generation is not filled.
        assert_eq!(
            GenerationName::next_after([number(5), number(2)]),
            Some(number(6))
        );
        assert_eq!(
            GenerationName::next_after([number(GenerationName::MAX)]),
            None
        );
    }
}
