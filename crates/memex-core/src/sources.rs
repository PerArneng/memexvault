//! Upload rules, as pure functions.
use memex_types::{JobRow, SourceRow};
use serde_json::Value;

pub const UNTRUSTED: &str = "untrusted";
pub const TRUST_TIERS: [&str; 4] = ["verified", "trusted", "unverified", UNTRUSTED];

/// The tiers a machine's claims can auto-commit from, once earned. Only a person can put a source here.
pub fn at_least_trusted(tier: &str) -> bool {
    matches!(tier, "verified" | "trusted")
}

/// What an upload may be. Spec 028 adds audio to what its pipeline can handle.
pub const ALLOWED_MEDIA_TYPES: [&str; 3] = ["application/pdf", "text/plain", "text/markdown"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadLimits {
    pub max_bytes: u64,
    /// New uploads get refused beyond this many unfinished jobs, so a flood can't grow without bound.
    pub max_open_jobs: u64,
}

impl Default for UploadLimits {
    fn default() -> Self {
        Self {
            max_bytes: 2 * 1024 * 1024 * 1024,
            max_open_jobs: 10_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UploadRequest {
    pub name: String,
    pub trust_tier: String,
    pub metadata: Value,
    /// A new version of this document, when set.
    pub replaces: Option<memex_types::ObjectId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UploadOutcome {
    pub source: SourceRow,
    pub job: Option<JobRow>,
    /// These bytes were already here, so nothing new was created.
    pub existing: bool,
}

/// What the bytes are, whatever the name or the client says. `name` only separates Markdown from plain text, which have the same bytes.
pub fn sniff_media_type(prefix: &[u8], name: &str) -> Option<&'static str> {
    if prefix.starts_with(b"%PDF-") {
        return Some("application/pdf");
    }
    // A prefix can cut a character in two, so only the error's position counts.
    let looks_like_text = !prefix.contains(&0)
        && match std::str::from_utf8(prefix) {
            Ok(_) => true,
            Err(e) => e.error_len().is_none(),
        };
    if !prefix.is_empty() && looks_like_text {
        let lower = name.to_lowercase();
        return Some(if lower.ends_with(".md") || lower.ends_with(".markdown") {
            "text/markdown"
        } else {
            "text/plain"
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pdf_is_known_by_its_bytes_not_its_name() {
        assert_eq!(sniff_media_type(b"%PDF-1.7\n...", "notes.txt"), Some("application/pdf"));
    }

    #[test]
    fn text_is_valid_utf8_and_markdown_goes_by_name() {
        assert_eq!(sniff_media_type("Åsa bor här".as_bytes(), "a.txt"), Some("text/plain"));
        assert_eq!(sniff_media_type(b"# Title", "a.MD"), Some("text/markdown"));
    }

    #[test]
    fn a_character_cut_by_the_prefix_is_still_text() {
        let bytes = "å".as_bytes();
        assert_eq!(sniff_media_type(&bytes[..1], "a.txt"), Some("text/plain"));
    }

    #[test]
    fn binary_and_empty_are_refused() {
        assert_eq!(sniff_media_type(&[0x89, b'P', b'N', b'G', 0, 1], "a.pdf"), None);
        assert_eq!(sniff_media_type(&[0xff, 0xfe, 0x41], "a.txt"), None);
        assert_eq!(sniff_media_type(b"", "a.txt"), None);
    }

    #[test]
    fn every_sniffed_type_is_allowed() {
        for t in ["application/pdf", "text/plain", "text/markdown"] {
            assert!(ALLOWED_MEDIA_TYPES.contains(&t));
        }
    }
}
