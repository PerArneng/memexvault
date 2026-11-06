use memex_types::{
    AssertionRow, ChangeSetRow, EdgeRow, Event, EvidenceRow, JobRow, JobState, Mutation, ObjectId, RenditionRow, Row, RowKey, SegmentRow, SourceRow, Table,
    VaultId,
};
use std::fmt;

/// The most rows or events one read returns. Callers page through with `after`.
pub const PAGE_LIMIT: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// Another writer holds this vault.
    WriterActive {
        vault: VaultId,
    },
    /// The lease was closed or lost, so this writer must stop.
    LeaseLost,
    UnknownVault {
        vault: VaultId,
    },
    /// An empty batch was committed.
    EmptyBatch,
    /// The commit's sequence isn't exactly one above the head.
    SequenceGap {
        head: Option<u64>,
        got: u64,
    },
    /// A delete named a row that doesn't exist. Nothing was applied.
    MissingRow {
        key: RowKey,
    },
    /// The backend is unavailable or failed. Nothing was applied unless the error says so.
    Backend(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WriterActive { vault } => write!(f, "vault {vault} already has a writer"),
            Self::LeaseLost => f.write_str("writer lease lost"),
            Self::EmptyBatch => f.write_str("empty commit batch"),
            Self::UnknownVault { vault } => write!(f, "unknown vault {vault}"),
            Self::SequenceGap { head, got } => write!(f, "sequence {got} doesn't follow head {head:?}"),
            Self::MissingRow { key } => write!(f, "row {:?} {} doesn't exist", key.table, key.id),
            Self::Backend(m) => write!(f, "store backend: {m}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// Proof of being the vault's single writer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriterLease {
    pub vault: VaultId,
    pub token: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Receipt {
    pub seq: u64,
}

/// One event with its mutations, as part of a batch.
#[derive(Debug, Clone, Copy)]
pub struct CommitItem<'a> {
    pub event: &'a Event,
    pub mutations: &'a [Mutation],
}

/// A consistent view of one vault, unchanged by commits made after it was taken.
pub trait Snapshot {
    fn head(&self) -> Result<Option<u64>, StoreError>;
    fn get(&self, key: &RowKey) -> Result<Option<Row>, StoreError>;
    /// One store call however many keys, in the same order as `keys`.
    fn get_many(&self, keys: &[RowKey]) -> Result<Vec<Option<Row>>, StoreError>;
    /// Active rows in id order, after the given id, at most `PAGE_LIMIT`.
    fn rows(&self, table: Table, after: Option<&ObjectId>) -> Result<Vec<Row>, StoreError>;
    /// Events in sequence order, above `after`, at most `PAGE_LIMIT`.
    fn events(&self, after: Option<u64>) -> Result<Vec<Event>, StoreError>;
    /// A source's renditions, oldest first.
    fn renditions_of(&self, source_id: &ObjectId) -> Result<Vec<RenditionRow>, StoreError>;
    /// A rendition's segments in id order, after the given id, at most `limit`.
    fn segments_of(&self, rendition_id: &ObjectId, after: Option<&ObjectId>, limit: usize) -> Result<Vec<SegmentRow>, StoreError>;
    /// Evidence supporting any of these assertions, in id order. One store call.
    fn evidence_of(&self, assertion_ids: &[ObjectId]) -> Result<Vec<EvidenceRow>, StoreError>;
    /// Evidence citing any of these segments. One store call.
    fn evidence_citing(&self, segment_ids: &[ObjectId]) -> Result<Vec<EvidenceRow>, StoreError>;
    /// Active edges of this kind that start or end at any of these ids. One store call.
    fn edges_touching(&self, ids: &[ObjectId], kind: &str) -> Result<Vec<EdgeRow>, StoreError>;
    /// The segments under these (rendition, heading) pairs, by rendition and offset, at most `limit`. One store call.
    fn sections_of(&self, sections: &[(ObjectId, String)], limit: usize) -> Result<Vec<SegmentRow>, StoreError>;
    /// Evidence whose re-anchoring failed, in id order.
    fn broken_anchors(&self, after: Option<&ObjectId>, limit: usize) -> Result<Vec<EvidenceRow>, StoreError>;
    /// The source with these bytes, if any.
    fn source_by_hash(&self, sha256: &str) -> Result<Option<SourceRow>, StoreError>;
    /// Versions of one document, oldest first.
    fn source_versions(&self, logical_id: &ObjectId) -> Result<Vec<SourceRow>, StoreError>;
    /// The job with this dedupe key, if any.
    fn job_by_dedupe(&self, key: &str) -> Result<Option<JobRow>, StoreError>;
    /// Jobs of a kind that can run now: queued and due, or running with an expired lease. Oldest first.
    fn claimable_jobs(&self, kind: &str, now_ms: i64, limit: usize) -> Result<Vec<JobRow>, StoreError>;
    /// Queued or running jobs, for backpressure.
    fn open_job_count(&self) -> Result<u64, StoreError>;
    /// In id order, optionally of one kind and state, after the given id.
    fn jobs(&self, kind: Option<&str>, state: Option<JobState>, after: Option<&ObjectId>, limit: usize) -> Result<Vec<JobRow>, StoreError>;
    /// The ChangeSet holding this idempotency key, if any.
    fn changeset_by_key(&self, key: &str) -> Result<Option<ChangeSetRow>, StoreError>;
    /// Active assertions matching any of these canonical hashes or subject and predicate pairs. One store call.
    fn assertions_matching(&self, hashes: &[String], pairs: &[(String, String)]) -> Result<Vec<AssertionRow>, StoreError>;
}

/// The system of record. Synchronous, so pure crates stay free of an async runtime.
pub trait Store {
    type Snap<'a>: Snapshot
    where
        Self: 'a;

    /// Creates the vault if it's new, and takes its exclusive writer lease.
    fn open_vault(&self, vault: &VaultId) -> Result<WriterLease, StoreError>;
    fn close_vault(&self, lease: &WriterLease) -> Result<(), StoreError>;
    fn snapshot(&self, vault: &VaultId) -> Result<Self::Snap<'_>, StoreError>;
    /// Atomic and durable when it returns. An error means no trace of the event or its mutations.
    fn commit(&self, lease: &WriterLease, event: &Event, mutations: &[Mutation]) -> Result<Receipt, StoreError> {
        self.commit_batch(lease, &[CommitItem { event, mutations }])
    }
    /// Several events in one transaction, for group commit. All or nothing, and durable when it returns. The receipt names the last sequence.
    fn commit_batch(&self, lease: &WriterLease, batch: &[CommitItem<'_>]) -> Result<Receipt, StoreError>;
    /// Removes the vault and all its data. The lease ends with it.
    fn delete_vault(&self, lease: &WriterLease) -> Result<(), StoreError>;
    /// Physically removes deleted data, for erasure. Reads don't change.
    fn compact(&self, lease: &WriterLease) -> Result<(), StoreError>;
}
