use crate::ObjectId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Epistemic {
    /// A reviewer or trusted person confirmed it.
    Verified,
    Asserted,
    Inferred,
    Disputed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stance {
    Supports,
    Refutes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceInput {
    pub segment_id: ObjectId,
    pub quote: String,
    pub stance: Stance,
}

/// One requested change. A ChangeSet holds several, applied together or not at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    AddAssertion {
        id: ObjectId,
        subject: String,
        predicate: String,
        value: Value,
        epistemic: Epistemic,
        machine: bool,
        confidence_permille: u16,
        valid_from_ms: Option<i64>,
        valid_to_ms: Option<i64>,
        evidence: Vec<(ObjectId, EvidenceInput)>,
    },
    /// Replaces a claim with a new one, which is how a claim changes.
    SupersedeAssertion {
        target: ObjectId,
        base_version: u32,
        replacement: Box<Op>,
    },
    CreatePage {
        id: ObjectId,
        title: String,
        body: String,
    },
    EditPage {
        id: ObjectId,
        base_version: u32,
        body: String,
    },
    DeletePage {
        id: ObjectId,
        base_version: u32,
    },
    AddEdge {
        id: ObjectId,
        from: ObjectId,
        to: ObjectId,
        kind: String,
    },
    RemoveEdge {
        id: ObjectId,
        base_version: u32,
    },
    SetSourceTrust {
        id: ObjectId,
        base_version: u32,
        trust_tier: String,
    },
    SetSourceMetadata {
        id: ObjectId,
        base_version: u32,
        metadata: Value,
    },
}

/// What kind of change it is, for the risk table. It includes kinds whose ops arrive with later specs, so the always-review rules are in one place from the start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpKind {
    AddClaim,
    EditClaim,
    EditVerifiedClaim,
    CreatePage,
    EditPage,
    Delete,
    AddLink,
    RemoveLink,
    MergeEntities,
    SplitEntity,
    BreakingSchemaChange,
    AdditiveSchemaChange,
    PromoteUntrustedSource,
    EditSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskDecision {
    AutoCommit,
    Review,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeSetState {
    /// Waiting for a reviewer.
    Pending,
    Committed,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatorResult {
    pub validator: String,
    pub passed: bool,
    pub detail: Option<String>,
}
