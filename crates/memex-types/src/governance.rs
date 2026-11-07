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

/// A claim moving to the new entity in a split, and the id its replacement takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovedClaim {
    pub assertion: ObjectId,
    pub new_id: ObjectId,
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
        /// None takes it from the title.
        #[serde(default)]
        slug: Option<String>,
        /// Old slugs that lead here, as an import brings them.
        #[serde(default)]
        redirects: Vec<String>,
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
    /// A new title, and with it a new slug unless one is given. The old slug keeps leading here.
    RenamePage {
        id: ObjectId,
        base_version: u32,
        title: String,
        #[serde(default)]
        slug: Option<String>,
    },
    /// Makes an old revision current again, as a new revision.
    RevertPage {
        id: ObjectId,
        base_version: u32,
        revision: u32,
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
    /// A new type, or its next version. `base_version` is the version it builds on, and none means the type is new.
    DefineEntityType {
        name: String,
        base_version: Option<u32>,
        schema: Value,
    },
    /// The entity itself. Its field values are Assertions in the same ChangeSet, with the subject `entity:<id>`.
    CreateEntity {
        id: ObjectId,
        type_name: String,
        aliases: Vec<String>,
    },
    SetEntityAliases {
        id: ObjectId,
        base_version: u32,
        aliases: Vec<String>,
    },
    /// `source` becomes a redirect to `target`, which takes over its claims and aliases.
    MergeEntities {
        source: ObjectId,
        source_version: u32,
        target: ObjectId,
        target_version: u32,
    },
    /// Creates `new_id`, and moves the named claims and aliases there by superseding the claims. Anything else the new entity needs is an Assertion in the same ChangeSet.
    SplitEntity {
        id: ObjectId,
        base_version: u32,
        new_id: ObjectId,
        moved: Vec<MovedClaim>,
        aliases: Vec<String>,
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
    CreateEntity,
    EditEntity,
    /// A claim arriving as `verified` from a machine, or citing an untrusted source. Only a person can make that so.
    ClaimVerified,
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
