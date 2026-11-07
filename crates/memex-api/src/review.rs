//! Request and response types for the review gate.
use crate::{ChangeSetDto, TokenUsageDto};
use memex_types::{AutoCommitChange, QueueCount, ReviewVerdictRow};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// What to list. Anything left out matches everything, and with no `state` it lists every state.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ReviewQueueQuery {
    /// `pending`, `committed` or `rejected`.
    pub state: Option<String>,
    pub extractor: Option<String>,
    pub prompt_version: Option<String>,
    /// `claim`, `entity:<type>`, `page`, `link` or `source`.
    pub item_type: Option<String>,
    pub source_id: Option<String>,
    pub proposer: Option<String>,
    /// `approve`, `reject`, `abstain`, or `none` for items with no verdict yet.
    pub verdict: Option<String>,
}

/// The reviewer agent's opinion. It's advice, and it decides nothing.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReviewVerdictDto {
    /// `approve`, `reject` or `abstain`.
    pub verdict: String,
    /// The model's own words about untrusted text, so show them as plain text.
    pub reason: String,
    pub model: String,
    pub prompt_hash: String,
    pub tokens: TokenUsageDto,
    pub attempts: u32,
    pub created_at_ms: i64,
}

impl From<ReviewVerdictRow> for ReviewVerdictDto {
    fn from(v: ReviewVerdictRow) -> Self {
        Self {
            verdict: v.verdict.as_str().into(),
            reason: v.reason,
            model: v.model,
            prompt_hash: v.prompt_hash,
            tokens: v.usage.into(),
            attempts: v.attempts,
            created_at_ms: v.created_at_ms,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReviewItemDto {
    pub changeset: ChangeSetDto,
    pub verdict: Option<ReviewVerdictDto>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReviewItemPage {
    pub items: Vec<ReviewItemDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReviewEvidenceDto {
    pub claim_id: String,
    pub segment_id: String,
    pub quote: String,
    pub source_id: Option<String>,
    pub source_name: Option<String>,
    pub trust_tier: Option<String>,
    pub rendition_id: Option<String>,
    pub page_from: Option<u32>,
    pub page_to: Option<u32>,
    /// Its document was processed again, so this can't be approved as it stands.
    pub superseded: bool,
    /// What the pipeline flagged on these pages, such as hidden text.
    #[schema(value_type = Vec<Object>)]
    pub page_flags: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AutoCommitChangeDto {
    pub at_ms: i64,
    pub by: String,
    /// `earned`, `fell_below` or `disabled`.
    pub kind: String,
    pub reason: String,
    pub correct: u64,
    pub incorrect: u64,
}

impl From<AutoCommitChange> for AutoCommitChangeDto {
    fn from(c: AutoCommitChange) -> Self {
        Self {
            at_ms: c.at_ms,
            by: c.by,
            kind: c.kind.as_str().into(),
            reason: c.reason,
            correct: c.correct,
            incorrect: c.incorrect,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AutoCommitDto {
    pub id: String,
    pub extractor: String,
    pub prompt_version: String,
    pub item_type: String,
    /// Since it was last disabled. These decide whether it's on.
    pub correct: u64,
    pub incorrect: u64,
    pub total_correct: u64,
    pub total_incorrect: u64,
    /// Absent with no samples.
    pub precision: Option<f64>,
    pub wilson_bound: f64,
    pub on: bool,
    /// When it was earned, fell below the threshold or was disabled, oldest first.
    pub history: Vec<AutoCommitChangeDto>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AutoCommitPage {
    /// What `wilson_bound` has to reach.
    pub threshold: f64,
    pub items: Vec<AutoCommitDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReviewDetailDto {
    pub changeset: ChangeSetDto,
    pub verdict: Option<ReviewVerdictDto>,
    pub evidence: Vec<ReviewEvidenceDto>,
    /// Each type's standing under this proposal's extractor and prompt. A type with no samples has no entry.
    pub combinations: Vec<AutoCommitDto>,
    /// Only a person can approve this.
    pub needs_a_person: bool,
    /// Why auto-commit wouldn't take this even if earned.
    pub auto_commit_block: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct QueueCountDto {
    pub extractor: String,
    pub prompt_version: String,
    pub item_type: String,
    /// Absent for items with no verdict yet.
    pub verdict: Option<String>,
    pub count: u64,
}

impl From<QueueCount> for QueueCountDto {
    fn from(c: QueueCount) -> Self {
        Self {
            extractor: c.extractor,
            prompt_version: c.prompt_version,
            item_type: c.item_type,
            verdict: c.verdict.map(|v| v.as_str().to_string()),
            count: c.count,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct QueueCountsDto {
    /// Pending items. One with several types counts once here, and once under each in `groups`.
    pub pending: u64,
    pub groups: Vec<QueueCountDto>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EditApproveBody {
    /// What to approve instead of what was proposed, which stays on record.
    #[schema(value_type = Vec<Object>)]
    pub ops: Vec<serde_json::Value>,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DisableBody {
    pub reason: String,
}

/// One decision in a batch.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BatchDecisionBody {
    pub id: String,
    /// `approve`, `reject` or `edit_approve`.
    pub decision: String,
    pub reason: Option<String>,
    /// With `edit_approve`.
    #[schema(value_type = Option<Vec<Object>>)]
    pub ops: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BatchBody {
    pub decisions: Vec<BatchDecisionBody>,
}

/// How one decision went. One failing leaves the others as they are.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BatchResultDto {
    pub id: String,
    pub changeset: Option<ChangeSetDto>,
    /// Set when it failed, with the same codes as the single routes.
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BatchResultsDto {
    pub results: Vec<BatchResultDto>,
}
