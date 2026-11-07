use memex_types::{
    AssertionRow, AuditCheckRow, AuditOutcome, BlockRow, ChangeSetRow, DerivationRow, DocKind, DocumentFacts, EdgeDirection, EdgeRow, EntityRow, EntityTypeRow,
    Event, EvidenceRow, FieldFilter, IdDigest, JobRow, JobState, Mutation, ObjectId, PageRow, QueueCount, RenditionRow, ReviewFilter, RevisionRow, Row, RowKey,
    SegmentRow, SharedValue, SourceRow, Table, ValueMatch, VaultId,
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
    /// A stamp of each row's contents, in the same order as `keys`, with none for a row that isn't there. The same stamp means the same row, and one store call covers all the keys. A backend can make this cheaper than reading the rows.
    fn row_stamps(&self, keys: &[RowKey]) -> Result<Vec<Option<String>>, StoreError> {
        Ok(self.get_many(keys)?.iter().map(|r| r.as_ref().map(crate::cache::stamp_of)).collect())
    }
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
    /// A count and digest of what an index should hold of this kind: claims that stand, and segments on a current rendition. A backend can work it out without moving the rows.
    fn id_digest(&self, kind: DocKind) -> Result<IdDigest, StoreError> {
        let mut digest = IdDigest::default();
        let table = if kind == DocKind::Claim { Table::Assertion } else { Table::Segment };
        let mut after: Option<ObjectId> = None;
        loop {
            let page = self.rows(table, after.as_ref())?;
            let Some(last) = page.last() else { return Ok(digest) };
            after = Some(last.header().id.clone());
            let segments: Vec<SegmentRow> = page
                .iter()
                .filter_map(|r| if let Row::Segment(s) = r { Some(s.clone()) } else { None })
                .collect();
            let stale = crate::lexical::stale_renditions(self, &segments)?;
            for r in &page {
                let wanted = match r {
                    Row::Assertion(a) => crate::audit::is_candidate(a),
                    Row::Segment(s) => crate::lexical::segment_is_current(s, &stale),
                    _ => false,
                };
                if wanted {
                    crate::audit::add_to_digest(&mut digest, &r.header().id.0);
                }
            }
        }
    }
    /// What the live board needs about each of these documents, in the same order. One store call in a real backend, as a page of the board holds up to 200.
    fn document_facts(&self, sources: &[ObjectId]) -> Result<Vec<DocumentFacts>, StoreError> {
        let mut out = Vec::with_capacity(sources.len());
        for source in sources {
            let rendition = self.renditions_of(source)?.into_iter().next_back();
            let extraction = match &rendition {
                Some(r) => match self.get(&RowKey {
                    table: Table::Extraction,
                    id: r.header.id.clone(),
                })? {
                    Some(Row::Extraction(x)) => Some(x),
                    _ => None,
                },
                None => None,
            };
            let extraction_job = match &extraction {
                Some(x) => match self.get(&RowKey {
                    table: Table::Job,
                    id: x.job_id.clone(),
                })? {
                    Some(Row::Job(j)) => Some(j),
                    _ => None,
                },
                None => None,
            };
            let pending_review = match &rendition {
                Some(r) => self.pending_citing_rendition(&r.header.id, None, PAGE_LIMIT)?.len() as u64,
                None => 0,
            };
            out.push(DocumentFacts {
                source_id: source.clone(),
                rendition,
                extraction,
                extraction_job,
                pending_review,
            });
        }
        Ok(out)
    }
    /// Each document a queued, running or dead embedding job holds segments of, with that job's state, at most `limit` pairs. One store call in a real backend, which needn't move the jobs' rows.
    fn open_embed_sources(&self, limit: usize) -> Result<Vec<(ObjectId, JobState)>, StoreError> {
        let mut out = vec![];
        for state in [JobState::Queued, JobState::Running, JobState::Dead] {
            for job in self.jobs(Some(crate::EMBED_JOB), Some(state), None, limit)? {
                let keys: Vec<RowKey> = crate::vector::embed_doc_ids(&job)
                    .into_iter()
                    .map(|id| RowKey { table: Table::Segment, id })
                    .collect();
                for row in self.get_many(&keys)?.into_iter().flatten() {
                    if let Row::Segment(s) = row
                        && !out.contains(&(s.source_id.clone(), state))
                    {
                        out.push((s.source_id, state));
                    }
                }
            }
        }
        out.truncate(limit);
        Ok(out)
    }
    /// When this event was written, if the log holds it.
    fn event_time(&self, seq: u64) -> Result<Option<i64>, StoreError> {
        let after = seq.checked_sub(1);
        Ok(self.events(after)?.first().filter(|e| e.seq == seq).map(|e| e.time_ms))
    }
    /// Whether any audit has already given this claim a correct sample.
    fn claim_counted_correct(&self, claim: &ObjectId) -> Result<bool, StoreError> {
        let mut cursor = None;
        loop {
            let page = self.rows(Table::AuditCheck, cursor.as_ref())?;
            let Some(last) = page.last() else { return Ok(false) };
            cursor = Some(last.header().id.clone());
            if page
                .iter()
                .any(|r| matches!(r, Row::AuditCheck(c) if &c.claim_id == claim && c.counted_correct))
            {
                return Ok(true);
            }
        }
    }
    /// A run's checks in id order, which is rank order, after the given id. With `findings_only`, those that didn't pass.
    fn audit_checks(&self, run: &ObjectId, findings_only: bool, after: Option<&ObjectId>, limit: usize) -> Result<Vec<AuditCheckRow>, StoreError> {
        let (mut out, mut cursor) = (vec![], after.cloned());
        loop {
            let page = self.rows(Table::AuditCheck, cursor.as_ref())?;
            let Some(last) = page.last() else { return Ok(out) };
            cursor = Some(last.header().id.clone());
            for r in page {
                if let Row::AuditCheck(c) = r
                    && &c.run_id == run
                    && (!findings_only || matches!(c.outcome, AuditOutcome::Suspect | AuditOutcome::Failed))
                {
                    out.push(c);
                    if out.len() == limit {
                        return Ok(out);
                    }
                }
            }
        }
    }
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
    /// The log's head and the vault's cache generation, in one call, without a snapshot. An unknown vault is an error.
    fn cache_marks(&self, vault: &VaultId) -> Result<crate::CacheMarks, StoreError>;
    /// Makes every cache key of this vault unreachable, durably, and returns the new generation. Erasure calls this.
    fn bump_cache_generation(&self, lease: &WriterLease) -> Result<u64, StoreError>;
    /// Removes the vault and all its data. The lease ends with it.
    fn delete_vault(&self, lease: &WriterLease) -> Result<(), StoreError>;
    /// Physically removes deleted data, for erasure. Reads don't change.
    fn compact(&self, lease: &WriterLease) -> Result<(), StoreError>;
}
