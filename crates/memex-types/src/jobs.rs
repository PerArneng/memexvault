use crate::rows::Header;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Done,
    /// Out of attempts. It stays here until someone retries it, and doesn't block others.
    Dead,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobAttempt {
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    pub error: Option<String>,
}

/// A job held back because something it needs is away. Waiting uses up no attempts, however long it lasts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobWait {
    pub since_ms: i64,
    pub tries: u32,
    pub last_error: String,
}

/// What a person did to a job by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobActionKind {
    Cancel,
    Retry,
}

/// A person's action on a job, kept on the job so it can be shown later without reading the log. The log's event for the same change names the same actor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobAction {
    pub action: JobActionKind,
    /// The principal's id.
    pub actor: String,
    pub time_ms: i64,
}

/// A durable unit of background work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRow {
    pub header: Header,
    pub kind: String,
    pub state: JobState,
    pub input: Value,
    /// Input hash and pipeline version, so the same work is queued once.
    pub dedupe_key: String,
    pub max_attempts: u32,
    pub next_run_at_ms: i64,
    pub lease_owner: Option<String>,
    pub lease_until_ms: Option<i64>,
    pub attempts: Vec<JobAttempt>,
    pub created_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    /// Set while the job is waiting, and cleared when it gets anywhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<JobWait>,
    /// Cancels and retries by people, oldest first, at most `MAX_JOB_ACTIONS`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<JobAction>,
    /// How far a running job has got, for jobs that say (spec 039).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<JobProgress>,
    /// What a finished job found, for jobs that report (spec 039).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<Value>,
}

/// A step of a job's work, counted from 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobProgress {
    pub step: u32,
    pub of: u32,
    /// What the step does, in words.
    pub label: String,
}

/// An operator's operation on a vault, run as a job (spec 039).
pub const OPERATION_JOB: &str = "operation";

/// Which index an operation acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexName {
    Keyword,
    Vector,
}

impl IndexName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::Vector => "vector",
        }
    }
}

/// What an operation job does. Its input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum OperationInput {
    /// Builds a new generation and waits until it's live.
    Reindex {
        index: IndexName,
    },
    /// Makes the generation before the live one live again.
    Rollback {
        index: IndexName,
    },
    Reconcile {
        at: Option<u64>,
        deep: bool,
        repair: bool,
    },
    /// Writes a backup archive the operation's route can download.
    Export,
    /// Removes unreferenced blobs older than this.
    Sweep {
        grace_s: u64,
    },
    /// Physically removes deleted rows from the vault's tables.
    Compact,
}

impl OperationInput {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Reindex { .. } => "reindex",
            Self::Rollback { .. } => "rollback",
            Self::Reconcile { .. } => "reconcile",
            Self::Export => "export",
            Self::Sweep { .. } => "sweep",
            Self::Compact => "compact",
        }
    }
}

/// How many of a job's actions it keeps. The oldest go first.
pub const MAX_JOB_ACTIONS: usize = 20;

impl JobRow {
    /// Queued and due, or running with an expired lease because its worker died.
    pub fn claimable(&self, now_ms: i64) -> bool {
        match self.state {
            JobState::Queued => self.next_run_at_ms <= now_ms,
            JobState::Running => self.lease_until_ms.is_some_and(|until| until <= now_ms),
            JobState::Done | JobState::Dead | JobState::Cancelled => false,
        }
    }
}
