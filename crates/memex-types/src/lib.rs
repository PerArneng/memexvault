//! Pure: ids, events and rows, shared by core and edges. No IO.
mod activity;
mod agent;
mod audit;
mod blob;
mod cache;
mod chat;
mod entity;
mod event;
mod extraction;
mod governance;
mod ids;
mod jobs;
mod lexical;
mod page;
mod registry;
mod rendition;
mod review;
mod rows;
mod search;
mod vector;

pub use activity::{DocumentFacts, ProgressEntry, ProgressUpdate, Stage, StageStatus};
pub use agent::{
    AgentBudget, AgentError, AgentReply, AgentRequest, AgentUsage, ChatEnd, ChatHost, ChatRequest, ChatStop, ChatTurn, RunnerEvent, RunnerPart, ToolDefinition,
    estimate_tokens,
};
pub use audit::{
    AuditCheckKind, AuditCheckResult, AuditCheckRow, AuditCheckStatus, AuditCombination, AuditLlmUse, AuditOutcome, AuditPlantedRow, AuditRunRow,
    AuditRunState, AuditSchedule, AuditStateRow, AuditStop, AuditTrigger, IdDigest, PlantedKind, SiblingCursor, VaultFinding,
};
pub use blob::{BlobHash, BlobHashError};
pub use cache::CacheTierStats;
pub use chat::{
    AgUi, CHAT_SETTINGS_ID, ChatErasure, ChatMessage, ChatReservation, ChatRole, ChatSettingsRow, ChatStored, ChatThread, ChatThreadKey, ChatToolCall,
};
pub use entity::{
    ALIAS_FIELD, ClaimedValue, ENTITY_SUBJECT_PREFIX, EntityField, EntityRow, EntityTypeRow, FieldFilter, FieldLookup, FieldOp, FieldValueRow, FilterValue,
    SharedValue, ValueMatch, entity_of_subject, entity_subject, entity_type_row_id, type_of_row_id,
};
pub use event::{Event, EventDraft};
pub use extraction::{DerivationOutcome, DerivationRow, ExtractedItem, ExtractionRow, ExtractionStop, IdentityCandidate, RejectedItem};
pub use governance::{ChangeSetState, Epistemic, EvidenceInput, MovedClaim, Op, OpKind, RiskDecision, Stance, ValidatorResult};
pub use ids::{ObjectId, VaultId};
pub use jobs::{JobAttempt, JobRow, JobState, JobWait};
pub use lexical::{
    AttrScope, AttrValue, DocKind, Generation, IndexDoc, IndexHealth, IndexOp, LexicalError, LexicalFilter, LexicalHit, LexicalQuery, LexicalResults, QueryTerm,
};
pub use page::{BlockKind, BlockRow, CLAIM_REF, EdgeDirection, RevisionBlock, RevisionRow, SlugRow, WIKI_LINK, revision_id, slug_of_target, slug_target};
pub use registry::{Audit, Grant, Principal, PrincipalKind, Scope, SessionRow, TokenRow, VaultRow};
pub use rendition::{
    Anchor, MediaFacts, MeetingMeta, PageClass, PageFacts, PageFlag, PageOutcome, PageRoute, ParsedDocument, Participant, Rect, RenditionRow, TimeSpan,
    TimedDocument, TimedWord, TranscribeUsage, Word,
};
pub use review::{AutoCommitChange, AutoCommitChangeKind, AutoCommitRow, ProposalOrigin, QueueCount, ReviewFilter, ReviewVerdictRow, Verdict, VerdictFilter};
pub use rows::{AssertionRow, ChangeSetRow, EdgeRow, EvidenceRow, Header, Mutation, PageRow, Row, RowKey, SegmentRow, SourceRow, Status, Table};
pub use search::{
    AnchorStatus, AsOf, BudgetReport, Citation, Conflict, EvidenceItem, EvidencePackage, Expand, FacetCount, Facets, ItemKind, QueryPath, RerankError,
    SearchDebug, SearchMode, SearchRequest, SignalScores,
};
pub use vector::{EmbedError, EmbedPurpose, ModelInfo, VectorDoc, VectorError, VectorHealth, VectorOp, VectorQuery};
