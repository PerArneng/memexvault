use memex_types::{
    AssertionRow, BlockRow, ChangeSetRow, DerivationRow, EdgeDirection, EdgeRow, EntityRow, EntityTypeRow, Event, EvidenceRow, FieldFilter, JobRow, JobState,
    Mutation, ObjectId, PageRow, QueueCount, RenditionRow, ReviewFilter, RevisionRow, Row, RowKey, SegmentRow, SharedValue, SourceRow, Table, ValueMatch,
    VaultId,
};
use std::fmt;

/// The most rows or events one read returns. Callers page through with `after`.
pub const PAGE_LIMIT: usize = 1000;
/// The most entities one suggested group names.
pub const MAX_SHARED_IDS: usize = 50;

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
    /// A rendition's Derivations in id order, after the given id, at most `limit`.
    fn derivations_of(&self, rendition_id: &ObjectId, after: Option<&ObjectId>, limit: usize) -> Result<Vec<DerivationRow>, StoreError>;
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
    /// Queued or running jobs, except of these kinds, for backpressure.
    fn open_job_count(&self, except_kinds: &[&str]) -> Result<u64, StoreError>;
    /// In id order, optionally of one kind and state, after the given id.
    fn jobs(&self, kind: Option<&str>, state: Option<JobState>, after: Option<&ObjectId>, limit: usize) -> Result<Vec<JobRow>, StoreError>;
    /// The ChangeSet holding this idempotency key, if any.
    fn changeset_by_key(&self, key: &str) -> Result<Option<ChangeSetRow>, StoreError>;
    /// Active assertions matching any of these canonical hashes or subject and predicate pairs. One store call.
    fn assertions_matching(&self, hashes: &[String], pairs: &[(String, String)]) -> Result<Vec<AssertionRow>, StoreError>;
    /// Active assertions about any of these subjects, in id order. One store call.
    fn assertions_about(&self, subjects: &[String]) -> Result<Vec<AssertionRow>, StoreError>;
    /// Stored versions of these types, by name and then oldest first. One store call. Seed types' built-in first versions aren't stored.
    fn entity_type_versions(&self, names: &[String]) -> Result<Vec<EntityTypeRow>, StoreError>;
    /// Each stored type's newest version, in name order, after the given name.
    fn latest_entity_types(&self, after: Option<&str>, limit: usize) -> Result<Vec<EntityTypeRow>, StoreError>;
    /// Current entities of a type, in id order, after the given id.
    fn entities_of_type(&self, type_name: &str, after: Option<&ObjectId>, limit: usize) -> Result<Vec<EntityRow>, StoreError>;
    /// Current entities of a type passing every filter, in id order, after the given id. At least one filter. One store call.
    fn entities_matching(&self, type_name: &str, filters: &[FieldFilter], after: Option<&ObjectId>, limit: usize) -> Result<Vec<EntityRow>, StoreError>;
    /// Current entities with this folded text in any field or alias, in id order. One store call.
    fn entities_with_value(&self, text: &str, limit: usize) -> Result<Vec<ValueMatch>, StoreError>;
    /// Folded values held by more than one current entity of a type, in these fields, by field and value, after the given pair. At most `MAX_SHARED_IDS` ids each.
    fn shared_values(&self, type_name: &str, fields: &[String], after: Option<(&str, &str)>, limit: usize) -> Result<Vec<SharedValue>, StoreError>;
    /// These pages' blocks, by page and position, at most `limit`. One store call.
    fn blocks_of(&self, pages: &[ObjectId], limit: usize) -> Result<Vec<BlockRow>, StoreError>;
    /// Only these pages' heading blocks, by page and position. One store call.
    fn headings_of(&self, pages: &[ObjectId]) -> Result<Vec<BlockRow>, StoreError>;
    /// Pages in id order, after the given id, with empty bodies, as lists don't show them.
    fn page_summaries(&self, after: Option<&ObjectId>, limit: usize) -> Result<Vec<PageRow>, StoreError>;
    /// A page's revisions, oldest first, after the given revision, with empty block lists. A deleted page's are still here.
    fn revisions_of(&self, page: &ObjectId, after: Option<u32>, limit: usize) -> Result<Vec<RevisionRow>, StoreError>;
    /// Active edges of these kinds with these ids at the given end, in id order, after the given id, at most `limit`. One store call.
    fn edges_linked(
        &self,
        ids: &[ObjectId],
        kinds: &[&str],
        direction: EdgeDirection,
        after: Option<&ObjectId>,
        limit: usize,
    ) -> Result<Vec<EdgeRow>, StoreError>;
    /// Wiki links whose slug no page holds and claim references whose claim doesn't exist, in id order, after the given id. One store call.
    fn unresolved_links(&self, after: Option<&ObjectId>, limit: usize) -> Result<Vec<EdgeRow>, StoreError>;
    /// ChangeSets passing the filter, in id order, after the given id, at most `limit`. One store call.
    fn review_queue(&self, filter: &ReviewFilter, after: Option<&ObjectId>, limit: usize) -> Result<Vec<ChangeSetRow>, StoreError>;
    /// Pending ChangeSets counted by extractor, prompt version, type and verdict. One store call.
    fn review_counts(&self) -> Result<Vec<QueueCount>, StoreError>;
    /// How many ChangeSets are pending. One store call.
    fn pending_count(&self) -> Result<u64, StoreError>;
    /// Pending ChangeSets whose evidence cites this rendition, in id order, after the given id, at most `limit`. One store call.
    fn pending_citing_rendition(&self, rendition_id: &ObjectId, after: Option<&ObjectId>, limit: usize) -> Result<Vec<ChangeSetRow>, StoreError>;
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
