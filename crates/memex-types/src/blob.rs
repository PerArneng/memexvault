use serde::{Deserialize, Serialize};
use std::fmt;

/// A SHA-256 as 64 lowercase hex characters. Only `parse` builds one, so a value of this type is always safe as a file name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct BlobHash(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobHashError {
    pub given: String,
}

impl fmt::Display for BlobHashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not a sha256 hash: {:?}", self.given.chars().take(80).collect::<String>())
    }
}

impl std::error::Error for BlobHashError {}

impl BlobHash {
    pub fn parse(text: &str) -> Result<Self, BlobHashError> {
        if text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            Ok(Self(text.to_string()))
        } else {
            Err(BlobHashError { given: text.to_string() })
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BlobHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for BlobHash {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_real_hash() {
        let h = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(BlobHash::parse(h).unwrap().as_str(), h);
    }

    #[test]
    fn refuses_anything_else() {
        let upper = "E".repeat(64);
        let dots = format!("../../{}", "a".repeat(58));
        let nul = format!("{}\0", "a".repeat(63));
        let slash = format!("{}/{}", "a".repeat(32), "b".repeat(31));
        for bad in [
            "",
            "abc",
            upper.as_str(),
            dots.as_str(),
            nul.as_str(),
            slash.as_str(),
            &"a".repeat(65),
            &"g".repeat(64),
        ] {
            assert!(BlobHash::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn deserializing_refuses_a_traversal() {
        assert!(serde_json::from_str::<BlobHash>(r#""../../etc/passwd""#).is_err());
    }
}
