//! What extraction records: a Derivation per batch of segments, and one row per rendition with the state of its latest run.
use crate::ObjectId;
use crate::agent::AgentUsage;
use crate::rows::Header;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DerivationOutcome {
    /// The batch has its results, and needs no further call for this prompt.
    Done,
    /// The tokens were spent but nothing usable came back. A re-run tries these segments again.
    Failed,
}

/// An existing entity a mention may mean. Only a suggestion, as merges are always a person's decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityCandidate {
    pub entity_id: ObjectId,
    /// The identity field or `$alias` that matched.
    pub field: String,
}

/// One validated result. The quote is the exact text of the segment, and the proposal is rebuilt from this, so no LLM is called again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExtractedItem {
    Claim {
        segment_id: ObjectId,
        quote: String,
        subject: String,
        predicate: String,
        value: String,
    },
    Entity {
        segment_id: ObjectId,
        quote: String,
        type_name: String,
        fields: Map<String, Value>,
        candidates: Vec<IdentityCandidate>,
    },
}

/// A result refused before it became a proposal, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedItem {
    pub segment: String,
    pub quote: String,
    pub reason: String,
}

/// How something generated came to be. Holds one batch's call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivationRow {
    pub header: Header,
    pub kind: String,
    pub job_id: ObjectId,
    pub source_id: ObjectId,
    pub rendition_id: ObjectId,
    /// The inputs: these segments of that rendition.
    pub segment_ids: Vec<ObjectId>,
    pub model: String,
    pub prompt_hash: String,
    pub usage: AgentUsage,
    pub attempts: u32,
    pub outcome: DerivationOutcome,
    pub detail: Option<String>,
    pub items: Vec<ExtractedItem>,
    pub rejected: Vec<RejectedItem>,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionStop {
    /// The next call wouldn't fit in the document's token budget.
    Budget,
    /// A batch failed, and the rest went ahead.
    FailedBatches,
    /// A newer rendition replaced this one, and has a job of its own.
    Superseded,
}

/// The latest extraction run of a rendition, whose id it shares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractionRow {
    pub header: Header,
    pub source_id: ObjectId,
    pub job_id: ObjectId,
    pub model: Option<String>,
    pub prompt_hash: Option<String>,
    pub segments_total: u32,
    pub segments_done: u32,
    pub batches_failed: u32,
    pub proposed: u32,
    pub rejected: u32,
    /// Spent by this run. A re-run starts again at zero, as asking for it is the decision to spend more.
    pub usage: AgentUsage,
    pub token_budget: u64,
    pub stop: Option<ExtractionStop>,
    pub updated_at_ms: i64,
}
