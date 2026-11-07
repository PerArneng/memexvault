//! What the audit agent records: its runs, what it checked and found, its schedule and spending, and the errors a test vault has had planted in it.
use crate::ObjectId;
use crate::agent::AgentUsage;
use crate::rows::Header;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditTrigger {
    Scheduled,
    OnDemand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditRunState {
    Queued,
    Running,
    Finished,
}

/// Why a run ended with LLM checks left undone. Its deterministic checks are done either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditStop {
    RunBudget,
    DayBudget,
    LlmUnavailable,
    LlmRefused,
}

impl AuditStop {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RunBudget => "run_budget",
            Self::DayBudget => "day_budget",
            Self::LlmUnavailable => "llm_unavailable",
            Self::LlmRefused => "llm_refused",
        }
    }
}

/// Something about the vault as a whole, with no one claim to put in front of a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultFinding {
    pub check: AuditCheckKind,
    pub status: AuditCheckStatus,
    pub detail: String,
}

/// One audit of a sample of the vault's claims. The seed and the state at `state_seq` decide the sample.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRunRow {
    pub header: Header,
    pub seed: u64,
    pub trigger: AuditTrigger,
    pub requested_by: String,
    pub state: AuditRunState,
    pub stop: Option<AuditStop>,
    pub stop_detail: Option<String>,
    pub sample_size: u32,
    /// How many claims it sampled from.
    pub population: u64,
    pub sampled: u32,
    pub state_seq: Option<u64>,
    pub run_token_budget: u64,
    pub usage: AgentUsage,
    pub model: Option<String>,
    pub llm_calls: u32,
    pub passed: u32,
    pub suspect: u32,
    pub failed: u32,
    /// Claims that got only their deterministic checks.
    pub llm_skipped: u32,
    /// The combinations this run took auto-commit from.
    pub revoked: Vec<ObjectId>,
    pub siblings_requeued: u32,
    /// Auto-committed claims of revoked combinations still to requeue. Later runs carry on with them.
    pub siblings_remaining: u64,
    /// Schedule slots passed over before this one.
    pub missed_slots: u32,
    pub vault_findings: Vec<VaultFinding>,
    pub job_id: ObjectId,
    pub created_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub finished_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditCheckKind {
    QuoteRelocation,
    LinkRot,
    Staleness,
    IndexReconciliation,
    Entailment,
    BlindReextraction,
}

impl AuditCheckKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::QuoteRelocation => "quote_relocation",
            Self::LinkRot => "link_rot",
            Self::Staleness => "staleness",
            Self::IndexReconciliation => "index_reconciliation",
            Self::Entailment => "entailment",
            Self::BlindReextraction => "blind_reextraction",
        }
    }

    pub fn uses_llm(self) -> bool {
        matches!(self, Self::Entailment | Self::BlindReextraction)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditCheckStatus {
    Pass,
    Fail,
    /// It ran and couldn't tell.
    Inconclusive,
}

impl AuditCheckStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Inconclusive => "inconclusive",
        }
    }
}

/// What one LLM check was given to work with and spent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditLlmUse {
    pub model: String,
    pub prompt_hash: String,
    pub usage: AgentUsage,
    pub attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditCheckResult {
    pub check: AuditCheckKind,
    pub status: AuditCheckStatus,
    /// A short fixed word for a failure, such as `quote_missing`.
    pub cause: Option<String>,
    /// May hold a model's words or a document's, so show it as plain text.
    pub detail: String,
    pub llm: Option<AuditLlmUse>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    /// Sampled, and not finished yet.
    Sampled,
    Passed,
    /// Worth a person's look, but it says nothing against the extractor by itself.
    Suspect,
    /// Wrong on the evidence the claim cites, which revokes its combination if a machine made it.
    Failed,
}

impl AuditOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sampled => "sampled",
            Self::Passed => "passed",
            Self::Suspect => "suspect",
            Self::Failed => "failed",
        }
    }
}

/// The combination behind a machine's claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditCombination {
    pub extractor: String,
    pub prompt_version: String,
    pub item_type: String,
}

/// One sampled claim in one run, and everything found about it. A finding is one of these with an outcome other than passed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditCheckRow {
    pub header: Header,
    pub run_id: ObjectId,
    pub rank: u32,
    pub claim_id: ObjectId,
    /// The sampling weight, in millionths, so dumps have no float in them.
    pub weight_micro: u64,
    /// None for a person's claim, which has no auto-commit to lose.
    pub combination: Option<AuditCombination>,
    pub auto_committed: bool,
    pub outcome: AuditOutcome,
    pub checks: Vec<AuditCheckResult>,
    /// The review item a person decides this finding on.
    pub changeset_id: Option<ObjectId>,
    /// This check gave its combination a correct sample. A claim gets one however many runs pick it, or repeat audits of one good claim could earn auto-commit.
    pub counted_correct: bool,
    pub finished_at_ms: Option<i64>,
}

/// How often to audit, how much, and what it may spend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditSchedule {
    pub enabled: bool,
    pub interval_ms: i64,
    pub sample_size: u32,
    pub run_token_budget: u64,
    pub day_token_budget: u64,
    /// How many days a claim with this predicate stays fresh.
    pub ttl_days: BTreeMap<String, u32>,
}

/// Where requeuing a revoked combination's auto-commits has got to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiblingCursor {
    pub combination: AuditCombination,
    pub after: Option<ObjectId>,
}

/// One row per vault: the schedule, the day's spending, and what's still to requeue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditStateRow {
    pub header: Header,
    pub schedule: AuditSchedule,
    pub next_run_at_ms: Option<i64>,
    pub total_missed_slots: u64,
    /// Whole days since the epoch.
    pub day: i64,
    pub day_spent: u64,
    /// Marked while empty, and the only kind of vault that can have errors planted.
    pub test_vault: bool,
    pub sibling_cursors: Vec<SiblingCursor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlantedKind {
    AlteredQuote,
    WrongSegment,
    MovedAnchor,
    BrokenAnchor,
    MissingSegment,
    MissingSource,
    MissingBlob,
    StaleTtl,
    ExpiredValidity,
    IndexMissing,
    WrongValue,
    NegatedValue,
    WrongSubject,
    UnsupportedClaim,
    /// Left as it was, to measure false positives.
    Clean,
}

impl PlantedKind {
    pub const ERRORS: [PlantedKind; 14] = [
        Self::AlteredQuote,
        Self::WrongSegment,
        Self::MovedAnchor,
        Self::BrokenAnchor,
        Self::MissingSegment,
        Self::MissingSource,
        Self::MissingBlob,
        Self::StaleTtl,
        Self::ExpiredValidity,
        Self::IndexMissing,
        Self::WrongValue,
        Self::NegatedValue,
        Self::WrongSubject,
        Self::UnsupportedClaim,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AlteredQuote => "altered_quote",
            Self::WrongSegment => "wrong_segment",
            Self::MovedAnchor => "moved_anchor",
            Self::BrokenAnchor => "broken_anchor",
            Self::MissingSegment => "missing_segment",
            Self::MissingSource => "missing_source",
            Self::MissingBlob => "missing_blob",
            Self::StaleTtl => "stale_ttl",
            Self::ExpiredValidity => "expired_validity",
            Self::IndexMissing => "index_missing",
            Self::WrongValue => "wrong_value",
            Self::NegatedValue => "negated_value",
            Self::WrongSubject => "wrong_subject",
            Self::UnsupportedClaim => "unsupported_claim",
            Self::Clean => "clean",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ERRORS.iter().copied().find(|k| k.as_str() == text)
    }

    /// Whether only an LLM check can see this error. The quote stays in place and everything resolves, so what's wrong is the claim's meaning.
    pub fn needs_llm(self) -> bool {
        matches!(self, Self::WrongValue | Self::NegatedValue | Self::WrongSubject | Self::UnsupportedClaim)
    }
}

/// A tag on a claim in a test vault, shared by id with the claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditPlantedRow {
    pub header: Header,
    pub kind: PlantedKind,
    pub planted_at_ms: i64,
}

/// A count of ids and a digest that doesn't depend on their order, so a store and an index can be compared without moving the ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IdDigest {
    pub count: u64,
    pub xor: i64,
}
