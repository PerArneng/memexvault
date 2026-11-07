//! Pure: traits, the Engine, reducers and policy. No runtime, network or file access. Anything with an edge sits behind a trait here.
mod agent;
pub mod audit;
mod auth;
mod batch;
mod blob;
pub mod cache;
pub mod chat;
mod clock;
mod dump;
mod engine;
pub mod entity;
pub mod extraction;
pub mod governance;
pub mod jobs;
pub mod lexical;
pub mod meeting;
pub mod ocr;
pub mod page;
pub mod projector;
pub mod raster;
mod reducer;
mod registry;
pub mod rendition;
mod replay;
pub mod review;
pub mod search;
mod seed;
mod server;
mod session;
mod settings;
pub mod sources;
mod store;
pub mod testing;
mod upcast;
pub mod vector;
pub mod vector_projector;

pub use agent::{AgentOutcome, AgentRunner, run_with_retry};
pub use audit::{AUDIT_JOB, AUDITOR, AuditLimits};
pub use auth::{Caller, Decision, Operation, authorize};
pub use batch::{BatchLimits, Pending, plan_batch};
pub use blob::{BlobInfo, BlobStore, BlobStoreError, BlobStored, Verified, sweepable};
pub use cache::{CacheMarks, Caches, IndexMarks, NoCaches};
pub use clock::Clock;
pub use dump::{Dump, canonical_dump, dump_store};
pub use engine::{ApplyError, Engine, NewSource, RecordSourceError};
pub use extraction::{EXTRACT_JOB, ExtractionLimits, ExtractionState};
pub use lexical::{CommitPolicy, LexicalIndex, QueryError, Touched};
pub use meeting::{TranscribeError, Transcribed, Transcriber};
pub use projector::{LexicalProjector, ProjectError, StepOutcome};
pub use raster::{ALLOWED_DPI, PageImage, RasterError, Rasterizer, THUMBNAIL_DPI, rect_to_pixels};
pub use reducer::{ChangeSetRecorded, ReduceError, StateRecorded, reduce};
pub use registry::{Registry, RegistryError};
pub use replay::{ReplayError, all_events, replay};
pub use review::{REVIEW_JOB, ReviewLimits};
pub use search::Reranker;
pub use seed::Seed;
pub use server::{
    AuditBudget, AuditRunDetail, AutoCommitEntry, AutoCommitTable, BacklinkView, BatchDecision, ChatInput, ChatLimits, ChatSettingsUpdate, ChatToolAnswer,
    ChatToolContext, ChatView, ClaimEvidence, ClaimRef, ClaimRefStatus, ClaimReference,
    CreatedSession, CreatedToken, EntityCandidate, EntityClaim, EntityEdit, EntityResolution, EntityView, EvidenceLineage, ExtractJobError, ExtractionReport,
    FieldInput, ImportedPage, Lineage, MediaInfo, MeetingView, NeighborView, NewEntity, NewPage, NewToken, OutgoingLink, PIPELINE_VERSION, PageChange,
    PageDiff, PageList, PageRedirect, PageRevisionView, PageView, ParticipantLink, ParticipantView, PreparedChat, Proposal, ProposalOutcome, RENDITION_JOB, RESULT_LIMIT,
    Redirect, RenditionInput, ReviewDetail, ReviewEvidence, ReviewItem, SearchEdges, Server, ServerEdges, ServerError, SplitRequest, TRANSCRIBE_JOB,
    TranscriptPage, TranscriptWindow, TriggerAudit, TypeDecision, UnresolvedLink, new_chat_cancel,
};
pub use session::{LOGIN_FAILURE_LIMIT, LOGIN_WINDOW_MS, SESSION_TTL_MS};
pub use settings::{Backends, CacheSettings, LlmSettings, Settings, SettingsError, TranscribeApi, TranscribeSettings};
pub use sources::{ALLOWED_MEDIA_TYPES, UNTRUSTED, UploadLimits, UploadOutcome, UploadRequest, sniff_media_type};
pub use store::{CommitItem, MAX_SHARED_IDS, PAGE_LIMIT, Receipt, Snapshot, Store, StoreError, WriterLease};
pub use upcast::{CURRENT_VERSIONS, UpcastError, upcast};
pub use vector::{EMBED_JOB, EmbedJobError, Embedder, VectorIndex};
pub use vector_projector::VectorProjector;
