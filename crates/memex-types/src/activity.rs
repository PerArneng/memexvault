//! What the live view of a vault is made of: the stages a document moves through, and what workers report while they're in one.
use crate::extraction::ExtractionRow;
use crate::ids::ObjectId;
use crate::jobs::JobRow;
use crate::rendition::RenditionRow;
use serde::{Deserialize, Serialize};

/// Where a document is on its way through the pipeline, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Stored,
    Parsing,
    Ocr,
    Transcription,
    Segmenting,
    Indexing,
    Embedding,
    Extraction,
    Review,
    Done,
}

impl Stage {
    pub const ALL: [Stage; 10] = [
        Stage::Stored,
        Stage::Parsing,
        Stage::Ocr,
        Stage::Transcription,
        Stage::Segmenting,
        Stage::Indexing,
        Stage::Embedding,
        Stage::Extraction,
        Stage::Review,
        Stage::Done,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stored => "stored",
            Self::Parsing => "parsing",
            Self::Ocr => "ocr",
            Self::Transcription => "transcription",
            Self::Segmenting => "segmenting",
            Self::Indexing => "indexing",
            Self::Embedding => "embedding",
            Self::Extraction => "extraction",
            Self::Review => "review",
            Self::Done => "done",
        }
    }

    /// The stage a name stands for, for a filter.
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == text)
    }
}

/// How things are going in that stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageStatus {
    /// Waiting for a worker, with nothing wrong.
    Queued,
    Active,
    /// Held back because something it needs is away, such as the LLM.
    Waiting,
    /// An attempt failed, and another is due.
    Retrying,
    /// Out of attempts, or cancelled, and a person has to act.
    Failed,
    /// Finished, without everything it set out to do.
    Partial,
    Done,
}

impl StageStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Active => "active",
            Self::Waiting => "waiting",
            Self::Retrying => "retrying",
            Self::Failed => "failed",
            Self::Partial => "partial",
            Self::Done => "done",
        }
    }
}

/// What a worker says while it works, such as OCR page 12 of 400. It holds ids and numbers and has no field for text, so nothing from a document can go through it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressUpdate {
    pub job_id: ObjectId,
    pub source_id: Option<ObjectId>,
    pub stage: Stage,
    pub done: u64,
    pub total: Option<u64>,
}

/// One report, as it's held in memory, with when it came.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressEntry {
    pub update: ProgressUpdate,
    pub time_ms: i64,
}

/// What the store knows about one document's latest reading, all from one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentFacts {
    pub source_id: ObjectId,
    /// The newest rendition, if there is one.
    pub rendition: Option<RenditionRow>,
    /// That rendition's latest extraction run.
    pub extraction: Option<ExtractionRow>,
    /// The job behind that run.
    pub extraction_job: Option<JobRow>,
    /// Pending ChangeSets citing that rendition.
    pub pending_review: u64,
}
