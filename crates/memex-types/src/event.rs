use crate::VaultId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One entry in the log. It records a result, never a command, so replay calls no LLM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub vault: VaultId,
    pub actor: String,
    pub time_ms: i64,
    pub kind: String,
    pub schema_version: u32,
    pub payload: Value,
}

/// What a caller supplies. The engine adds the sequence, the vault and the time.
#[derive(Debug, Clone, PartialEq)]
pub struct EventDraft {
    pub actor: String,
    pub kind: String,
    pub schema_version: u32,
    pub payload: Value,
}
