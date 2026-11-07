//! Request and response types for extraction.
use memex_types::{AgentUsage, DerivationOutcome, DerivationRow, ExtractedItem, RejectedItem};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TokenUsageDto {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub thinking_tokens: u64,
}

impl From<AgentUsage> for TokenUsageDto {
    fn from(u: AgentUsage) -> Self {
        Self {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            thinking_tokens: u.thinking_tokens,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ExtractionStatusDto {
    pub source_id: String,
    /// The current rendition, once there is one.
    pub rendition_id: Option<String>,
    /// `not_started`, `queued`, `running`, `waiting` for the LLM, `partial`, `done`, `failed` or `cancelled`.
    pub state: String,
    pub job_id: Option<String>,
    /// Why a `partial` run stopped: `budget`, `failed_batches` or `superseded`.
    pub stop_reason: Option<String>,
    pub segments_total: u32,
    pub segments_done: u32,
    pub batches_failed: u32,
    /// New proposals this run made, which wait in the review queue.
    pub proposed: u32,
    /// Results refused, such as a quote that isn't in its segment.
    pub rejected: u32,
    /// What this run has spent.
    pub tokens: TokenUsageDto,
    pub token_budget: u64,
    pub model: Option<String>,
    pub prompt_hash: Option<String>,
    pub waiting_since_ms: Option<i64>,
    pub last_error: Option<String>,
    pub updated_at_ms: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RejectedItemDto {
    pub segment: String,
    pub quote: String,
    pub reason: String,
}

impl From<RejectedItem> for RejectedItemDto {
    fn from(r: RejectedItem) -> Self {
        Self {
            segment: r.segment,
            quote: r.quote,
            reason: r.reason,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DerivationDto {
    pub id: String,
    pub kind: String,
    pub job_id: String,
    pub source_id: String,
    pub rendition_id: String,
    pub segment_ids: Vec<String>,
    pub model: String,
    pub prompt_hash: String,
    pub tokens: TokenUsageDto,
    pub attempts: u32,
    /// `done`, or `failed` when the tokens were spent with nothing usable back.
    pub outcome: String,
    pub detail: Option<String>,
    /// The results as validated, each with its segment and exact quote.
    #[schema(value_type = Vec<Object>)]
    pub items: Vec<Value>,
    pub rejected: Vec<RejectedItemDto>,
    pub created_at_ms: i64,
}

fn item_json(item: &ExtractedItem) -> Value {
    serde_json::to_value(item).unwrap_or(Value::Null)
}

impl From<DerivationRow> for DerivationDto {
    fn from(d: DerivationRow) -> Self {
        Self {
            id: d.header.id.0,
            kind: d.kind,
            job_id: d.job_id.0,
            source_id: d.source_id.0,
            rendition_id: d.rendition_id.0,
            segment_ids: d.segment_ids.into_iter().map(|s| s.0).collect(),
            model: d.model,
            prompt_hash: d.prompt_hash,
            tokens: d.usage.into(),
            attempts: d.attempts,
            outcome: match d.outcome {
                DerivationOutcome::Done => "done",
                DerivationOutcome::Failed => "failed",
            }
            .into(),
            detail: d.detail,
            items: d.items.iter().map(item_json).collect(),
            rejected: d.rejected.into_iter().map(Into::into).collect(),
            created_at_ms: d.created_at_ms,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DerivationPage {
    pub items: Vec<DerivationDto>,
    pub next_cursor: Option<String>,
}
