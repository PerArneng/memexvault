//! What the review gate records: where a proposal came from, the reviewer agent's verdict, and what each combination of extractor, prompt and type has earned.
use crate::ObjectId;
use crate::agent::AgentUsage;
use crate::governance::ChangeSetState;
use crate::rows::Header;
use serde::{Deserialize, Serialize};

/// Where a proposal came from, as far as review and auto-commit care. Set by the server, never by the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalOrigin {
    /// The model behind an extraction, or otherwise the principal who proposed it.
    pub extractor: String,
    /// The prompt hash of an extraction, and empty for anything else.
    pub prompt_version: String,
    /// What it's about: `claim`, `entity:<type>`, `page`, `link` or `source`. A ChangeSet with several needs all of them earned.
    pub item_types: Vec<String>,
    /// The documents and renditions its evidence cites.
    pub source_ids: Vec<ObjectId>,
    pub rendition_ids: Vec<ObjectId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Approve,
    Reject,
    /// It couldn't tell, or gave no usable answer.
    Abstain,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
            Self::Abstain => "abstain",
        }
    }
}

/// The reviewer agent's opinion of one ChangeSet, whose id it shares. Advice for whoever decides: it never commits or rejects anything, and never counts towards precision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewVerdictRow {
    pub header: Header,
    pub verdict: Verdict,
    /// The model's own words, so show it as untrusted text.
    pub reason: String,
    pub model: String,
    pub prompt_hash: String,
    pub usage: AgentUsage,
    pub attempts: u32,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoCommitChangeKind {
    /// The bound reached the threshold.
    Earned,
    /// The bound fell below the threshold.
    FellBelow,
    /// Someone turned it off. What was earned before no longer counts.
    Disabled,
}

impl AutoCommitChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Earned => "earned",
            Self::FellBelow => "fell_below",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoCommitChange {
    pub at_ms: i64,
    pub by: String,
    pub kind: AutoCommitChangeKind,
    pub reason: String,
    /// The counts behind it, as they stood.
    pub correct: u64,
    pub incorrect: u64,
}

/// One combination of extractor, prompt version and type. Only what people and audits decided counts here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoCommitRow {
    pub header: Header,
    pub extractor: String,
    pub prompt_version: String,
    pub item_type: String,
    /// Since it was last disabled. These decide whether it's on.
    pub correct: u64,
    pub incorrect: u64,
    /// Since it began, for the record.
    pub total_correct: u64,
    pub total_incorrect: u64,
    /// Whether the last change left it on. The threshold can change after that, so readers work it out from the counts.
    pub on: bool,
    pub history: Vec<AutoCommitChange>,
    pub updated_at_ms: i64,
}

/// Which verdict to list, where `None` means items with no verdict yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerdictFilter {
    Is(Verdict),
    None,
}

/// What to list from the queue. Everything left empty matches anything.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReviewFilter {
    pub state: Option<ChangeSetState>,
    pub extractor: Option<String>,
    pub prompt_version: Option<String>,
    pub item_type: Option<String>,
    pub source_id: Option<ObjectId>,
    pub proposer: Option<String>,
    pub verdict: Option<VerdictFilter>,
}

/// How many pending items share these. An item with several types counts once under each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueCount {
    pub extractor: String,
    pub prompt_version: String,
    pub item_type: String,
    pub verdict: Option<Verdict>,
    pub count: u64,
}
