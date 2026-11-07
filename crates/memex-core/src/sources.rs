//! Upload rules, as pure functions.
use memex_types::{JobRow, SourceRow};
use serde_json::Value;

pub const UNTRUSTED: &str = "untrusted";
pub const TRUST_TIERS: [&str; 4] = ["verified", "trusted", "unverified", UNTRUSTED];

/// The tiers a machine's claims can auto-commit from, once earned. Only a person can put a source here.
pub fn at_least_trusted(tier: &str) -> bool {
    matches!(tier, "verified" | "trusted")
}

/// What an upload may be. Audio and video go to transcription, and the rest to parsing.
pub const ALLOWED_MEDIA_TYPES: [&str; 11] = [
    "application/pdf",
    "text/plain",
    "text/markdown",
    "audio/wav",
    "audio/mpeg",
    "audio/mp4",
    "audio/ogg",
    "audio/flac",
    "video/mp4",
    "video/webm",
    "video/x-matroska",
];

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

/// A recording's type from its first bytes. The container decides, as the codec inside can be anything, and decoding finds out the rest.
fn sniff_recording(prefix: &[u8]) -> Option<&'static str> {
    if prefix.len() >= 12 && prefix.starts_with(b"RIFF") && &prefix[8..12] == b"WAVE" {
        return Some("audio/wav");
    }
    if prefix.len() >= 12 && &prefix[4..8] == b"ftyp" {
        return Some(if prefix[8..12].starts_with(b"M4A") || prefix[8..12].starts_with(b"M4B") {
            "audio/mp4"
        } else {
            "video/mp4"
        });
    }
    if prefix.starts_with(b"OggS") {
        return Some("audio/ogg");
    }
    if prefix.starts_with(b"fLaC") {
        return Some("audio/flac");
    }
    if prefix.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
        return Some(if prefix.windows(4).any(|w| w == b"webm") {
            "video/webm"
        } else {
            "video/x-matroska"
        });
    }
    if prefix.starts_with(b"ID3") {
        return Some("audio/mpeg");
    }
    // An MP3 frame header: the sync bits, then a version, bitrate and rate that aren't reserved, and layer II or III. Layer I is left out because its header starts like a UTF-16 byte order mark, `ff fe`.
    if let [0xff, b1, b2, ..] = prefix
        && b1 & 0xe0 == 0xe0
        && b1 & 0x18 != 0x08
        && matches!(b1 & 0x06, 0x02 | 0x04)
        && b2 & 0xf0 != 0xf0
        && b2 & 0x0c != 0x0c
    {
        return Some("audio/mpeg");
    }
    None
}

/// What the bytes are, whatever the name or the client says. `name` only separates Markdown from plain text, which have the same bytes.
pub fn sniff_media_type(prefix: &[u8], name: &str) -> Option<&'static str> {
    if prefix.starts_with(b"%PDF-") {
        return Some("application/pdf");
    }
    if let Some(media) = sniff_recording(prefix) {
        return Some(media);
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
    fn a_recording_is_known_by_its_container() {
        for (prefix, name, want) in [
            (&b"RIFF\x10\0\0\0WAVEfmt "[..], "a.mp3", "audio/wav"),
            (b"\0\0\0\x20ftypM4A \0\0\0\0", "a.bin", "audio/mp4"),
            (b"\0\0\0\x20ftypisom\0\0\0\0", "a.m4a", "video/mp4"),
            (b"OggS\0\x02", "a.txt", "audio/ogg"),
            (b"fLaC\0\0\0\x22", "a", "audio/flac"),
            (b"\x1a\x45\xdf\xa3\x9f\x42\x82\x84webm", "a", "video/webm"),
            (b"\x1a\x45\xdf\xa3\x9f\x42\x82\x88matroska", "a", "video/x-matroska"),
            (b"ID3\x04\0", "a", "audio/mpeg"),
            (&[0xff, 0xfb, 0x90, 0x64], "a", "audio/mpeg"),
        ] {
            let got = sniff_media_type(prefix, name);
            assert_eq!(got, Some(want), "{name}");
            assert!(crate::meeting::is_media(want));
            assert!(ALLOWED_MEDIA_TYPES.contains(&want));
        }
    }

    #[test]
    fn what_only_looks_like_a_recording_is_refused() {
        // Reserved version, layer, bitrate and rate.
        for bytes in [
            [0xff, 0xeb, 0x90, 0x64],
            [0xff, 0xf9, 0x90, 0x64],
            [0xff, 0xfb, 0xf0, 0x64],
            [0xff, 0xfb, 0x9c, 0x64],
        ] {
            assert_eq!(sniff_media_type(&bytes, "a.mp3"), None);
        }
        assert_ne!(sniff_media_type(b"RIFF\x10\0\0\0AVI LIST", "a.wav"), Some("audio/wav"));
    }

    #[test]
    fn every_sniffed_type_is_allowed() {
        for t in ["application/pdf", "text/plain", "text/markdown"] {
            assert!(ALLOWED_MEDIA_TYPES.contains(&t));
        }
    }
}
