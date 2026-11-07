use crate::ObjectId;
use crate::rows::Header;
use serde::{Deserialize, Serialize};

/// A box in displayed coordinates: after CropBox and /Rotate, origin top left, y down, in integer 1/100 pt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Word {
    pub text: String,
    pub page: u32,
    pub rect: Rect,
    /// From OCR, in permille. A text layer has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<u16>,
    /// From OCR, the line's four corners, for skewed text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quad: Option<[(i32, i32); 4]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageRoute {
    TextLayer,
    Ocr,
    Blank,
}

/// Something a person or a claim's confidence should know about a page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "flag")]
pub enum PageFlag {
    LowConfidence,
    /// Text-layer words that OCR can't see on the rendered page.
    HiddenText {
        words: u32,
    },
    ZeroSizeText {
        words: u32,
    },
    InvisibleUnicode {
        chars: u32,
    },
    /// OCR failed here, and the rest of the document carried on.
    OcrFailed {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageOutcome {
    pub index: u32,
    pub route: PageRoute,
    pub mean_confidence_permille: Option<u16>,
    pub flags: Vec<PageFlag>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageClass {
    Text,
    /// No visible text and images over half the page, so spec 016 sends it to OCR.
    ImageOnly,
    Blank,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageFacts {
    pub index: u32,
    pub width: i32,
    pub height: i32,
    pub rotation: i32,
    pub class: PageClass,
    /// Text objects with an invisible render mode, which may be an OCR layer or may be hiding something.
    pub hidden_text_objects: u32,
}

/// What a parser produced for one document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedDocument {
    pub parser: String,
    pub pages: Vec<PageFacts>,
    pub words: Vec<Word>,
}

/// Parser output for one Source. Immutable. A new parser version makes a new one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenditionRow {
    pub header: Header,
    pub source_id: ObjectId,
    pub parser: String,
    pub pipeline_version: u32,
    /// Blob holding the canonical text.
    pub text_blob: String,
    /// Blob holding the word table and page facts.
    pub words_blob: String,
    pub page_count: u32,
    pub segment_count: u32,
    pub created_at_ms: i64,
    pub superseded_by: Option<ObjectId>,
    /// How each page was read, and anything worth flagging.
    #[serde(default)]
    pub pages: Vec<PageOutcome>,
    /// What decoding a recording found. Only a recording has this, and it says the word table is a `TimedDocument`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaFacts>,
}

/// Where a quote sits: offsets into canonical text, plus rects for PDFs. Quote, prefix and suffix let a new rendition find it again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub source_sha256: String,
    pub rendition_id: ObjectId,
    pub start: u64,
    pub end: u64,
    pub quote: String,
    pub prefix: String,
    pub suffix: String,
    /// One per line, with its 0-based page.
    pub rects: Vec<(u32, Rect)>,
    /// Where it plays in the original recording. Only a recording has this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<TimeSpan>,
}

/// A range of the original media's timeline, in integer milliseconds, with the speaker when one person spoke all of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeSpan {
    pub t_start_ms: i64,
    pub t_end_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

/// One spoken word, on the original media's timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimedWord {
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

/// What decoding a recording found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaFacts {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u32,
    pub duration_ms: i64,
    /// Holes in the packets, kept as silence so times stay on the original timeline.
    pub gaps: u32,
    pub gap_ms: i64,
    pub packets: u64,
    pub bad_packets: u64,
    /// Stretches where the transcriber crashed twice, and which have no words.
    #[serde(default)]
    pub skipped_spans: u32,
    /// Stretches read on token times because the transcriber crashed on them once, which are less accurate.
    #[serde(default)]
    pub coarse_spans: u32,
}

/// What a transcription provider charged for, when one was used.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscribeUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub calls: u64,
}

/// A recording's word table: a rendition's words blob when it has `media`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimedDocument {
    pub parser: String,
    pub media: MediaFacts,
    pub words: Vec<TimedWord>,
    #[serde(default)]
    pub usage: TranscribeUsage,
}

/// Someone at a meeting. `speaker` is a transcript's label for them, when someone knows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Participant {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

/// What a person says about a recording, kept in its Source's metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_ms: Option<i64>,
    #[serde(default)]
    pub participants: Vec<Participant>,
}
