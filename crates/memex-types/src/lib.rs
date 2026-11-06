//! Pure: ids, events and rows, shared by core and edges. No IO.
mod blob;
mod event;
mod governance;
mod ids;
mod jobs;
mod lexical;
mod registry;
mod rendition;
mod rows;
mod search;
mod vector;

pub use blob::{BlobHash, BlobHashError};
pub use event::{Event, EventDraft};
pub use governance::{ChangeSetState, Epistemic, EvidenceInput, Op, OpKind, RiskDecision, Stance, ValidatorResult};
pub use ids::{ObjectId, VaultId};
pub use jobs::{JobAttempt, JobRow, JobState};
pub use lexical::{DocKind, Generation, IndexDoc, IndexHealth, IndexOp, LexicalError, LexicalFilter, LexicalHit, LexicalQuery, LexicalResults, QueryTerm};
pub use registry::{Audit, Grant, Principal, PrincipalKind, Scope, TokenRow, VaultRow};
pub use rendition::{Anchor, PageClass, PageFacts, PageFlag, PageOutcome, PageRoute, ParsedDocument, Rect, RenditionRow, Word};
pub use rows::{AssertionRow, ChangeSetRow, EdgeRow, EvidenceRow, Header, Mutation, PageRow, Row, RowKey, SegmentRow, SourceRow, Status, Table};
pub use search::{
    AnchorStatus, AsOf, BudgetReport, Citation, Conflict, EvidenceItem, EvidencePackage, FacetCount, Facets, ItemKind, QueryPath, RerankError, SearchDebug,
    SearchMode, SearchRequest, SignalScores,
};
pub use vector::{EmbedError, EmbedPurpose, ModelInfo, VectorDoc, VectorError, VectorHealth, VectorOp, VectorQuery};
