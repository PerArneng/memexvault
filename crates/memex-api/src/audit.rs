//! Request and response types for the audit agent.
use crate::TokenUsageDto;
use memex_types::{AuditCheckResult, AuditCheckRow, AuditCombination, AuditLlmUse, AuditRunRow, AuditSchedule, VaultFinding};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::{IntoParams, ToSchema};

/// What to run. Anything left out comes from the schedule. The same seed on the same state gives the same sample.
#[derive(Debug, Default, Serialize, Deserialize, ToSchema)]
pub struct TriggerAuditBody {
    /// A decimal number, as text, since it can be too large for a JSON number to carry safely.
    pub seed: Option<String>,
    pub sample_size: Option<u32>,
    pub run_token_budget: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct VaultFindingDto {
    pub check: String,
    /// `pass`, `fail` or `inconclusive`.
    pub status: String,
    pub detail: String,
}

impl From<VaultFinding> for VaultFindingDto {
    fn from(f: VaultFinding) -> Self {
        Self {
            check: f.check.as_str().into(),
            status: f.status.as_str().into(),
            detail: f.detail,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AuditRunDto {
    pub id: String,
    /// A decimal number, as text.
    pub seed: String,
    /// `scheduled` or `on_demand`.
    pub trigger: String,
    pub requested_by: String,
    /// `queued`, `running`, `finished`, or `failed` if its job gave up.
    pub state: String,
    /// Why its LLM checks ended early: `run_budget`, `day_budget`, `llm_unavailable` or `llm_refused`. Running out of budget isn't a failure.
    pub stop: Option<String>,
    pub stop_detail: Option<String>,
    pub sample_size: u32,
    pub population: u64,
    pub sampled: u32,
    pub state_seq: Option<u64>,
    pub run_token_budget: u64,
    pub tokens: TokenUsageDto,
    pub model: Option<String>,
    pub llm_calls: u32,
    pub passed: u32,
    pub suspect: u32,
    pub failed: u32,
    pub llm_skipped: u32,
    /// Combinations this run took auto-commit from.
    pub revoked: Vec<String>,
    pub siblings_requeued: u32,
    /// Claims of revoked combinations still to go back to review. Later runs carry on with them.
    pub siblings_remaining: u64,
    pub missed_slots: u32,
    pub vault_findings: Vec<VaultFindingDto>,
    pub created_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub finished_at_ms: Option<i64>,
}

impl AuditRunDto {
    pub fn new(r: AuditRunRow, job_gave_up: bool) -> Self {
        let state = match (job_gave_up, r.state) {
            (_, memex_types::AuditRunState::Finished) => "finished",
            (true, _) => "failed",
            (_, memex_types::AuditRunState::Queued) => "queued",
            (_, memex_types::AuditRunState::Running) => "running",
        };
        Self {
            id: r.header.id.0,
            seed: r.seed.to_string(),
            trigger: match r.trigger {
                memex_types::AuditTrigger::Scheduled => "scheduled".into(),
                memex_types::AuditTrigger::OnDemand => "on_demand".into(),
            },
            requested_by: r.requested_by,
            state: state.into(),
            stop: r.stop.map(|s| s.as_str().into()),
            stop_detail: r.stop_detail,
            sample_size: r.sample_size,
            population: r.population,
            sampled: r.sampled,
            state_seq: r.state_seq,
            run_token_budget: r.run_token_budget,
            tokens: r.usage.into(),
            model: r.model,
            llm_calls: r.llm_calls,
            passed: r.passed,
            suspect: r.suspect,
            failed: r.failed,
            llm_skipped: r.llm_skipped,
            revoked: r.revoked.into_iter().map(|i| i.0).collect(),
            siblings_requeued: r.siblings_requeued,
            siblings_remaining: r.siblings_remaining,
            missed_slots: r.missed_slots,
            vault_findings: r.vault_findings.into_iter().map(Into::into).collect(),
            created_at_ms: r.created_at_ms,
            started_at_ms: r.started_at_ms,
            finished_at_ms: r.finished_at_ms,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AuditRunPage {
    pub items: Vec<AuditRunDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AuditRunDetailDto {
    pub run: AuditRunDto,
    /// The claims it sampled, in rank order. The checks route has what was found about each.
    pub sample: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AuditCheckResultDto {
    pub check: String,
    /// `pass`, `fail` or `inconclusive`.
    pub status: String,
    pub cause: Option<String>,
    /// May hold a document's or a model's words, so show it as plain text.
    pub detail: String,
    pub model: Option<String>,
    pub prompt_hash: Option<String>,
    pub tokens: Option<TokenUsageDto>,
}

impl From<AuditCheckResult> for AuditCheckResultDto {
    fn from(c: AuditCheckResult) -> Self {
        let (model, prompt_hash, tokens) = match c.llm {
            Some(AuditLlmUse { model, prompt_hash, usage, .. }) => (Some(model), Some(prompt_hash), Some(usage.into())),
            None => (None, None, None),
        };
        Self {
            check: c.check.as_str().into(),
            status: c.status.as_str().into(),
            cause: c.cause,
            detail: c.detail,
            model,
            prompt_hash,
            tokens,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AuditCombinationDto {
    pub extractor: String,
    pub prompt_version: String,
    pub item_type: String,
}

impl From<AuditCombination> for AuditCombinationDto {
    fn from(c: AuditCombination) -> Self {
        Self {
            extractor: c.extractor,
            prompt_version: c.prompt_version,
            item_type: c.item_type,
        }
    }
}

/// One sampled claim and what was found about it. A finding is one of these whose outcome isn't `passed`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AuditCheckDto {
    pub id: String,
    pub run_id: String,
    pub rank: u32,
    pub claim_id: String,
    pub weight_micro: u64,
    /// None for a person's claim.
    pub combination: Option<AuditCombinationDto>,
    pub auto_committed: bool,
    /// `sampled`, `passed`, `suspect` or `failed`.
    pub outcome: String,
    pub checks: Vec<AuditCheckResultDto>,
    /// The review item a person decides this on.
    pub changeset_id: Option<String>,
    pub counted_correct: bool,
    pub finished_at_ms: Option<i64>,
}

impl From<AuditCheckRow> for AuditCheckDto {
    fn from(c: AuditCheckRow) -> Self {
        Self {
            id: c.header.id.0,
            run_id: c.run_id.0,
            rank: c.rank,
            claim_id: c.claim_id.0,
            weight_micro: c.weight_micro,
            combination: c.combination.map(Into::into),
            auto_committed: c.auto_committed,
            outcome: c.outcome.as_str().into(),
            checks: c.checks.into_iter().map(Into::into).collect(),
            changeset_id: c.changeset_id.map(|i| i.0),
            counted_correct: c.counted_correct,
            finished_at_ms: c.finished_at_ms,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AuditCheckPage {
    pub items: Vec<AuditCheckDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AuditChecksQuery {
    /// Only what didn't pass.
    pub findings: Option<bool>,
}

/// How often to audit, how much, and what it may spend.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AuditScheduleBody {
    pub enabled: bool,
    pub interval_ms: i64,
    pub sample_size: u32,
    pub run_token_budget: u64,
    pub day_token_budget: u64,
    /// Days until claims of each predicate go stale.
    #[serde(default)]
    pub ttl_days: BTreeMap<String, u32>,
}

impl From<AuditScheduleBody> for AuditSchedule {
    fn from(b: AuditScheduleBody) -> Self {
        Self {
            enabled: b.enabled,
            interval_ms: b.interval_ms,
            sample_size: b.sample_size,
            run_token_budget: b.run_token_budget,
            day_token_budget: b.day_token_budget,
            ttl_days: b.ttl_days,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AuditScheduleDto {
    pub enabled: bool,
    pub interval_ms: i64,
    pub sample_size: u32,
    pub run_token_budget: u64,
    pub day_token_budget: u64,
    pub ttl_days: BTreeMap<String, u32>,
    pub next_run_at_ms: Option<i64>,
    /// Slots passed over since it began, because the server was away or a run was still open.
    pub total_missed_slots: u64,
}

impl AuditScheduleDto {
    pub fn new(s: AuditSchedule, next_run_at_ms: Option<i64>, total_missed_slots: u64) -> Self {
        Self {
            enabled: s.enabled,
            interval_ms: s.interval_ms,
            sample_size: s.sample_size,
            run_token_budget: s.run_token_budget,
            day_token_budget: s.day_token_budget,
            ttl_days: s.ttl_days,
            next_run_at_ms,
            total_missed_slots,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AuditBudgetDto {
    pub run_token_budget: u64,
    pub day_token_budget: u64,
    /// Whole days since the epoch, in UTC.
    pub day: i64,
    pub day_spent: u64,
    pub day_remaining: u64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PlantBody {
    /// A decimal number, as text.
    pub seed: String,
    pub per_kind: u32,
    /// Kinds to plant, and every kind if left out.
    pub kinds: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PlantedCountDto {
    pub kind: String,
    pub planted: u64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PlantedDto {
    pub kinds: Vec<PlantedCountDto>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct KindRecallDto {
    pub kind: String,
    /// Only an LLM check can catch this kind.
    pub needs_llm: bool,
    pub planted: u64,
    pub audited: u64,
    pub caught: u64,
    pub recall: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct GroupRecallDto {
    pub planted: u64,
    pub audited: u64,
    pub caught: u64,
    pub recall: Option<f64>,
}

/// What planted errors in this vault came to. Keep the two groups apart: the deterministic one is measured, and the LLM one means something only when `llm_evidence` is `model`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RecallReportDto {
    pub kinds: Vec<KindRecallDto>,
    pub deterministic: GroupRecallDto,
    pub llm_only: GroupRecallDto,
    pub overall_planted: u64,
    pub overall_caught: u64,
    pub overall_recall: Option<f64>,
    /// `none`, `scripted` or `model`. Scripted answers prove the mechanism and say nothing about a model.
    pub llm_evidence: String,
    pub llm_models: Vec<String>,
    pub clean_audited: u64,
    pub false_positives_deterministic: u64,
    pub false_positives_llm: u64,
    pub target: f64,
    /// Null until it can be judged: every kind planted, with answers from a real model.
    pub target_met: Option<bool>,
    /// Too few planted errors to trust the figure.
    pub too_small: bool,
}
