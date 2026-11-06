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
}

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
