//! SHA-256 digests in their written form.

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::Digest as _;

/// A SHA-256 digest, written as 64 lowercase hexadecimal digits. Digests are
/// compared in this form.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct Sha256(String);

/// Why a text is not a written SHA-256 digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DigestError {
    /// The length of the text, in bytes.
    pub len: usize,
}

impl Sha256 {
    /// The number of hexadecimal digits of a written digest.
    pub const LEN: usize = 64;

    /// The digest of `bytes`.
    pub fn of(bytes: &[u8]) -> Self {
        let digest = sha2::Sha256::digest(bytes);
        Self(digest.iter().map(|byte| format!("{byte:02x}")).collect())
    }

    /// Reads a written digest: exactly 64 lowercase hexadecimal digits.
    pub fn parse(text: &str) -> Result<Self, DigestError> {
        let well_formed = text.len() == Self::LEN
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if well_formed {
            Ok(Self(text.to_string()))
        } else {
            Err(DigestError { len: text.len() })
        }
    }

    /// Whether this is the digest of `bytes`.
    pub fn matches(&self, bytes: &[u8]) -> bool {
        *self == Self::of(bytes)
    }

    /// The 64 lowercase hexadecimal digits.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for Sha256 {
    type Error = DigestError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text)
    }
}

impl fmt::Display for DigestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "a SHA-256 digest is {} lowercase hexadecimal digits; this text has {} bytes and is not one",
            Sha256::LEN,
            self.len
        )
    }
}

impl std::error::Error for DigestError {}

#[cfg(test)]
mod tests {
    use serde::de::IntoDeserializer;
    use serde::de::value::{Error as ValueError, StrDeserializer};

    use super::*;

    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn digests_are_written_as_64_lowercase_hex_digits() {
        assert_eq!(Sha256::of(b"").as_str(), EMPTY);
        assert_eq!(Sha256::of(b"abc").to_string(), ABC);
    }

    #[test]
    fn a_digest_matches_only_its_own_bytes() {
        let digest = Sha256::of(b"abc");
        assert!(digest.matches(b"abc"));
        assert!(!digest.matches(b"abd"));
        assert!(!digest.matches(b""));
    }

    #[test]
    fn parse_accepts_only_64_lowercase_hex_digits() {
        assert_eq!(Sha256::parse(ABC).unwrap(), Sha256::of(b"abc"));
        let upper = ABC.to_ascii_uppercase();
        let rejected = [
            "",
            &ABC[..63],
            &format!("{ABC}0"),
            &upper,
            &format!("{}g", &ABC[..63]),
            &format!(" {}", &ABC[..63]),
            &format!("{}\u{e9}", &ABC[..62]),
        ];
        for text in rejected {
            assert!(Sha256::parse(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn a_digest_is_read_from_a_document_string() {
        let read = |text: &str| {
            let de: StrDeserializer<'_, ValueError> = text.into_deserializer();
            Sha256::deserialize(de)
        };
        assert_eq!(read(EMPTY).unwrap(), Sha256::of(b""));
        let error = read("E3B0").unwrap_err().to_string();
        assert!(error.contains("64 lowercase hexadecimal digits"), "{error}");
    }
}
