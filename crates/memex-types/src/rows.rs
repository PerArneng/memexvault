use crate::ObjectId;
use crate::audit::{AuditCheckRow, AuditPlantedRow, AuditRunRow, AuditStateRow};
use crate::chat::ChatSettingsRow;
use crate::entity::{EntityRow, EntityTypeRow};
use crate::extraction::{DerivationRow, ExtractionRow};
use crate::governance::{ChangeSetState, Epistemic, Op, RiskDecision, Stance, ValidatorResult};
use crate::jobs::JobRow;
use crate::page::{BlockRow, RevisionRow, SlugRow};
use crate::rendition::{Anchor, RenditionRow};
use crate::review::{AutoCommitRow, ProposalOrigin, ReviewVerdictRow};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Table {
    Source,
    Page,
    Edge,
    Segment,
    Assertion,
    Evidence,
    ChangeSet,
    Job,
    Rendition,
    EntityType,
    Entity,
    Block,
    Slug,
    Revision,
    Derivation,
    Extraction,
    ReviewVerdict,
    AutoCommit,
    AuditRun,
    AuditCheck,
    AuditState,
    AuditPlanted,
    ChatSettings,
}

impl Table {
    pub const ALL: [Table; 23] = [
        Table::Source,
        Table::Page,
        Table::Edge,
        Table::Segment,
        Table::Assertion,
        Table::Evidence,
        Table::ChangeSet,
        Table::Job,
        Table::Rendition,
        Table::EntityType,
        Table::Entity,
        Table::Block,
        Table::Slug,
        Table::Revision,
        Table::Derivation,
        Table::Extraction,
        Table::ReviewVerdict,
        Table::AutoCommit,
        Table::AuditRun,
        Table::AuditCheck,
        Table::AuditState,
        Table::AuditPlanted,
        Table::ChatSettings,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RowKey {
    pub table: Table,
    pub id: ObjectId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Active,
    Superseded,
}

/// The header every object kind shares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub id: ObjectId,
    pub creator: String,
    pub status: Status,
    pub version: u32,
    pub acl_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRow {
    pub header: Header,
    pub sha256: String,
    pub media_type: String,
    pub trust_tier: String,
    pub recorded_at_ms: i64,
    /// The file name as uploaded, shown to people and never used as a path.
    pub name: String,
    pub size: u64,
    /// Versions of one document share this. The first version's id.
    pub logical_id: ObjectId,
    pub version_no: u32,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRow {
    pub header: Header,
    pub title: String,
    pub body: String,
    pub updated_at_ms: i64,
    /// Where links reach it. Empty only on a page from before spec 022, until its first edit.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub slug: String,
    /// Its old slugs, oldest first, which still lead here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redirects: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeRow {
    pub header: Header,
    pub from: ObjectId,
    pub to: ObjectId,
    pub kind: String,
    /// The block a page link was written in, which is its evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_id: Option<ObjectId>,
}

/// A located span of a document. Spec 015 fills in its anchors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentRow {
    pub header: Header,
    pub source_id: ObjectId,
    pub language: Option<String>,
    pub text: String,
    #[serde(default)]
    pub rendition_id: Option<ObjectId>,
    /// UTF-8 byte offsets into the rendition's canonical text.
    #[serde(default)]
    pub start: u64,
    #[serde(default)]
    pub end: u64,
    #[serde(default)]
    pub heading: Option<String>,
    #[serde(default)]
    pub page_from: Option<u32>,
    #[serde(default)]
    pub page_to: Option<u32>,
    /// Where it plays, when it comes from a recording.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<crate::rendition::TimeSpan>,
}

/// One claim. Never edited: a change makes a new one and supersedes this.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssertionRow {
    pub header: Header,
    pub subject: String,
    pub predicate: String,
    pub value: Value,
    pub epistemic: Epistemic,
    /// Made by a machine, so it needs evidence and can't earn trust from its own confidence.
    pub machine: bool,
    /// In permille, so hashes and dumps have no float in them.
    pub confidence_permille: u16,
    pub valid_from_ms: Option<i64>,
    pub valid_to_ms: Option<i64>,
    pub canonical_hash: String,
    pub changeset_id: ObjectId,
    pub superseded_by: Option<ObjectId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRow {
    pub header: Header,
    pub assertion_id: ObjectId,
    pub segment_id: ObjectId,
    pub quote: String,
    pub stance: Stance,
    /// Where the quote sits. Absent until located, and on a failed re-anchor it stays as it was with `anchor_broken` set.
    #[serde(default)]
    pub anchor: Option<Anchor>,
    #[serde(default)]
    pub anchor_broken: bool,
}

/// An atomic group of changes, with who asked, what was decided and how to undo it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeSetRow {
    pub header: Header,
    pub state: ChangeSetState,
    pub proposer: String,
    pub proposer_is_machine: bool,
    pub rationale: String,
    pub idempotency_key: String,
    pub ops: Vec<Op>,
    pub validators: Vec<ValidatorResult>,
    pub risk: RiskDecision,
    pub proposed_at_ms: i64,
    pub decided_by: Option<String>,
    pub decided_at_ms: Option<i64>,
    pub decision_reason: Option<String>,
    pub commit_seq: Option<u64>,
    /// The mutations committed, and the rows as they were before, in reverse order. Empty until committed.
    pub applied: Vec<Mutation>,
    pub inverse: Vec<Mutation>,
    /// The ChangeSet this one reverts.
    pub reverts: Option<ObjectId>,
    pub reverted_by: Option<ObjectId>,
    /// Who and what it came from. Absent on ChangeSets from before spec 025.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<ProposalOrigin>,
    /// What was proposed, when a person changed it before approving. `ops` then holds what was approved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_ops: Option<Vec<Op>>,
    /// The person whose chat asked for this, when an agent proposed it on their behalf. Such a proposal always waits for a person and never counts as a sample.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_by: Option<String>,
}

// Rows are handled one at a time on a path that does IO, so the size of the largest kind doesn't matter here.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Row {
    Source(SourceRow),
    Page(PageRow),
    Edge(EdgeRow),
    Segment(SegmentRow),
    Assertion(AssertionRow),
    Evidence(EvidenceRow),
    ChangeSet(ChangeSetRow),
    Job(JobRow),
    Rendition(RenditionRow),
    EntityType(EntityTypeRow),
    Entity(EntityRow),
    Block(BlockRow),
    Slug(SlugRow),
    Revision(RevisionRow),
    Derivation(DerivationRow),
    Extraction(ExtractionRow),
    ReviewVerdict(ReviewVerdictRow),
    AutoCommit(AutoCommitRow),
    AuditRun(AuditRunRow),
    AuditCheck(AuditCheckRow),
    AuditState(AuditStateRow),
    AuditPlanted(AuditPlantedRow),
    ChatSettings(ChatSettingsRow),
}

impl Row {
    pub fn header(&self) -> &Header {
        match self {
            Row::Source(r) => &r.header,
            Row::Page(r) => &r.header,
            Row::Edge(r) => &r.header,
            Row::Segment(r) => &r.header,
            Row::Assertion(r) => &r.header,
            Row::Evidence(r) => &r.header,
            Row::ChangeSet(r) => &r.header,
            Row::Job(r) => &r.header,
            Row::Rendition(r) => &r.header,
            Row::EntityType(r) => &r.header,
            Row::Entity(r) => &r.header,
            Row::Block(r) => &r.header,
            Row::Slug(r) => &r.header,
            Row::Revision(r) => &r.header,
            Row::Derivation(r) => &r.header,
            Row::Extraction(r) => &r.header,
            Row::ReviewVerdict(r) => &r.header,
            Row::AutoCommit(r) => &r.header,
            Row::AuditRun(r) => &r.header,
            Row::AuditCheck(r) => &r.header,
            Row::AuditState(r) => &r.header,
            Row::AuditPlanted(r) => &r.header,
            Row::ChatSettings(r) => &r.header,
        }
    }

    pub fn key(&self) -> RowKey {
        let table = match self {
            Row::Source(_) => Table::Source,
            Row::Page(_) => Table::Page,
            Row::Edge(_) => Table::Edge,
            Row::Segment(_) => Table::Segment,
            Row::Assertion(_) => Table::Assertion,
            Row::Evidence(_) => Table::Evidence,
            Row::ChangeSet(_) => Table::ChangeSet,
            Row::Job(_) => Table::Job,
            Row::Rendition(_) => Table::Rendition,
            Row::EntityType(_) => Table::EntityType,
            Row::Entity(_) => Table::Entity,
            Row::Block(_) => Table::Block,
            Row::Slug(_) => Table::Slug,
            Row::Revision(_) => Table::Revision,
            Row::Derivation(_) => Table::Derivation,
            Row::Extraction(_) => Table::Extraction,
            Row::ReviewVerdict(_) => Table::ReviewVerdict,
            Row::AutoCommit(_) => Table::AutoCommit,
            Row::AuditRun(_) => Table::AuditRun,
            Row::AuditCheck(_) => Table::AuditCheck,
            Row::AuditState(_) => Table::AuditState,
            Row::AuditPlanted(_) => Table::AuditPlanted,
            Row::ChatSettings(_) => Table::ChatSettings,
        };
        RowKey {
            table,
            id: self.header().id.clone(),
        }
    }
}

/// A typed change to one row. Reducers return these, and only a Store applies them.
// See the note on `Row`.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mutation {
    Put(Row),
    Delete(RowKey),
}
