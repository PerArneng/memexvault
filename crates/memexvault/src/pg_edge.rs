//! The default Store: `memex-store-postgres` behind the core trait. It only translates.
use memex_core::{CommitItem, Receipt, Snapshot, Store, StoreError, WriterLease};
use memex_store_postgres::{PgError, PgLease, PgSnapshot, PgStore};
use memex_types::{
    AssertionRow, ChangeSetRow, Event, EvidenceRow, JobRow, JobState, Mutation, ObjectId, RenditionRow, Row, RowKey, SegmentRow, SourceRow, Table, VaultId,
};
use std::sync::Arc;

pub struct PostgresStore(pub Arc<PgStore>);

pub struct PostgresSnapshot<'a> {
    inner: PgSnapshot<'a>,
    vault: VaultId,
}

fn translate(vault: &VaultId, e: PgError) -> StoreError {
    match e {
        PgError::WriterActive => StoreError::WriterActive { vault: vault.clone() },
        PgError::LeaseLost => StoreError::LeaseLost,
        PgError::UnknownVault => StoreError::UnknownVault { vault: vault.clone() },
        PgError::EmptyBatch => StoreError::EmptyBatch,
        PgError::SequenceGap { head, got } => StoreError::SequenceGap { head, got },
        PgError::MissingRow { key } => StoreError::MissingRow { key },
        PgError::Refused(m) | PgError::Backend(m) => StoreError::Backend(m),
    }
}

impl Snapshot for PostgresSnapshot<'_> {
    fn document_facts(&self, sources: &[ObjectId]) -> Result<Vec<memex_types::DocumentFacts>, StoreError> {
        self.inner.document_facts(sources).map_err(|e| translate(&self.vault, e))
    }

    fn open_embed_sources(&self, limit: usize) -> Result<Vec<(ObjectId, JobState)>, StoreError> {
        self.inner.open_embed_sources(limit).map_err(|e| translate(&self.vault, e))
    }

    fn event_time(&self, seq: u64) -> Result<Option<i64>, StoreError> {
        self.inner.event_time(seq).map_err(|e| translate(&self.vault, e))
    }

    fn row_stamps(&self, keys: &[RowKey]) -> Result<Vec<Option<String>>, StoreError> {
        self.inner.row_stamps(keys).map_err(|e| translate(&self.vault, e))
    }

    fn renditions_of(&self, source_id: &ObjectId) -> Result<Vec<RenditionRow>, StoreError> {
        self.inner.renditions_of(source_id).map_err(|e| translate(&self.vault, e))
    }

    fn segments_of(&self, rendition_id: &ObjectId, after: Option<&ObjectId>, limit: usize) -> Result<Vec<SegmentRow>, StoreError> {
        self.inner.segments_of(rendition_id, after, limit).map_err(|e| translate(&self.vault, e))
    }

    fn evidence_of(&self, assertion_ids: &[ObjectId]) -> Result<Vec<EvidenceRow>, StoreError> {
        self.inner.evidence_of(assertion_ids).map_err(|e| translate(&self.vault, e))
    }

    fn evidence_citing(&self, segment_ids: &[ObjectId]) -> Result<Vec<EvidenceRow>, StoreError> {
        self.inner.evidence_citing(segment_ids).map_err(|e| translate(&self.vault, e))
    }

    fn broken_anchors(&self, after: Option<&ObjectId>, limit: usize) -> Result<Vec<EvidenceRow>, StoreError> {
        self.inner.broken_anchors(after, limit).map_err(|e| translate(&self.vault, e))
    }

    fn source_by_hash(&self, sha256: &str) -> Result<Option<SourceRow>, StoreError> {
        self.inner.source_by_hash(sha256).map_err(|e| translate(&self.vault, e))
    }

    fn source_versions(&self, logical_id: &ObjectId) -> Result<Vec<SourceRow>, StoreError> {
        self.inner.source_versions(logical_id).map_err(|e| translate(&self.vault, e))
    }

    fn job_by_dedupe(&self, key: &str) -> Result<Option<JobRow>, StoreError> {
        self.inner.job_by_dedupe(key).map_err(|e| translate(&self.vault, e))
    }

    fn claimable_jobs(&self, kind: &str, now_ms: i64, limit: usize) -> Result<Vec<JobRow>, StoreError> {
        self.inner.claimable_jobs(kind, now_ms, limit).map_err(|e| translate(&self.vault, e))
    }

    fn derivations_of(&self, rendition_id: &ObjectId, after: Option<&ObjectId>, limit: usize) -> Result<Vec<memex_types::DerivationRow>, StoreError> {
        self.inner.derivations_of(rendition_id, after, limit).map_err(|e| translate(&self.vault, e))
    }

    fn open_job_count(&self, except_kinds: &[&str]) -> Result<u64, StoreError> {
        self.inner.open_job_count(except_kinds).map_err(|e| translate(&self.vault, e))
    }

    fn jobs(&self, kind: Option<&str>, state: Option<JobState>, after: Option<&ObjectId>, limit: usize) -> Result<Vec<JobRow>, StoreError> {
        self.inner.jobs(kind, state, after, limit).map_err(|e| translate(&self.vault, e))
    }

    fn changeset_by_key(&self, key: &str) -> Result<Option<ChangeSetRow>, StoreError> {
        self.inner.changeset_by_key(key).map_err(|e| translate(&self.vault, e))
    }

    fn assertions_matching(&self, hashes: &[String], pairs: &[(String, String)]) -> Result<Vec<AssertionRow>, StoreError> {
        self.inner.assertions_matching(hashes, pairs).map_err(|e| translate(&self.vault, e))
    }

    fn assertions_about(&self, subjects: &[String]) -> Result<Vec<AssertionRow>, StoreError> {
        self.inner.assertions_about(subjects).map_err(|e| translate(&self.vault, e))
    }

    fn entity_type_versions(&self, names: &[String]) -> Result<Vec<memex_types::EntityTypeRow>, StoreError> {
        self.inner.entity_type_versions(names).map_err(|e| translate(&self.vault, e))
    }

    fn latest_entity_types(&self, after: Option<&str>, limit: usize) -> Result<Vec<memex_types::EntityTypeRow>, StoreError> {
        self.inner.latest_entity_types(after, limit).map_err(|e| translate(&self.vault, e))
    }

    fn entities_of_type(&self, type_name: &str, after: Option<&ObjectId>, limit: usize) -> Result<Vec<memex_types::EntityRow>, StoreError> {
        self.inner.entities_of_type(type_name, after, limit).map_err(|e| translate(&self.vault, e))
    }

    fn entities_matching(
        &self,
        type_name: &str,
        filters: &[memex_types::FieldFilter],
        after: Option<&ObjectId>,
        limit: usize,
    ) -> Result<Vec<memex_types::EntityRow>, StoreError> {
        self.inner
            .entities_matching(type_name, filters, after, limit)
            .map_err(|e| translate(&self.vault, e))
    }

    fn entities_with_value(&self, text: &str, limit: usize) -> Result<Vec<memex_types::ValueMatch>, StoreError> {
        self.inner.entities_with_value(text, limit).map_err(|e| translate(&self.vault, e))
    }

    fn shared_values(
        &self,
        type_name: &str,
        fields: &[String],
        after: Option<(&str, &str)>,
        limit: usize,
    ) -> Result<Vec<memex_types::SharedValue>, StoreError> {
        self.inner
            .shared_values(type_name, fields, after, limit, memex_core::MAX_SHARED_IDS)
            .map_err(|e| translate(&self.vault, e))
    }

    fn edges_touching(&self, ids: &[ObjectId], kind: &str) -> Result<Vec<memex_types::EdgeRow>, StoreError> {
        self.inner.edges_touching(ids, kind).map_err(|e| translate(&self.vault, e))
    }

    fn blocks_of(&self, pages: &[ObjectId], limit: usize) -> Result<Vec<memex_types::BlockRow>, StoreError> {
        self.inner.blocks_of(pages, limit).map_err(|e| translate(&self.vault, e))
    }

    fn headings_of(&self, pages: &[ObjectId]) -> Result<Vec<memex_types::BlockRow>, StoreError> {
        self.inner.headings_of(pages).map_err(|e| translate(&self.vault, e))
    }

    fn page_summaries(&self, after: Option<&ObjectId>, limit: usize) -> Result<Vec<memex_types::PageRow>, StoreError> {
        self.inner.page_summaries(after, limit).map_err(|e| translate(&self.vault, e))
    }

    fn revisions_of(&self, page: &ObjectId, after: Option<u32>, limit: usize) -> Result<Vec<memex_types::RevisionRow>, StoreError> {
        self.inner.revisions_of(page, after, limit).map_err(|e| translate(&self.vault, e))
    }

    fn edges_linked(
        &self,
        ids: &[ObjectId],
        kinds: &[&str],
        direction: memex_types::EdgeDirection,
        after: Option<&ObjectId>,
        limit: usize,
    ) -> Result<Vec<memex_types::EdgeRow>, StoreError> {
        self.inner
            .edges_linked(ids, kinds, direction, after, limit)
            .map_err(|e| translate(&self.vault, e))
    }

    fn unresolved_links(&self, after: Option<&ObjectId>, limit: usize) -> Result<Vec<memex_types::EdgeRow>, StoreError> {
        self.inner.unresolved_links(after, limit).map_err(|e| translate(&self.vault, e))
    }

    fn review_queue(&self, filter: &memex_types::ReviewFilter, after: Option<&ObjectId>, limit: usize) -> Result<Vec<ChangeSetRow>, StoreError> {
        self.inner.review_queue(filter, after, limit).map_err(|e| translate(&self.vault, e))
    }

    fn review_counts(&self) -> Result<Vec<memex_types::QueueCount>, StoreError> {
        self.inner.review_counts().map_err(|e| translate(&self.vault, e))
    }

    fn pending_count(&self) -> Result<u64, StoreError> {
        self.inner.pending_count().map_err(|e| translate(&self.vault, e))
    }

    fn pending_citing_rendition(&self, rendition_id: &ObjectId, after: Option<&ObjectId>, limit: usize) -> Result<Vec<ChangeSetRow>, StoreError> {
        self.inner
            .pending_citing_rendition(rendition_id, after, limit)
            .map_err(|e| translate(&self.vault, e))
    }

    fn sections_of(&self, sections: &[(ObjectId, String)], limit: usize) -> Result<Vec<SegmentRow>, StoreError> {
        self.inner.sections_of(sections, limit).map_err(|e| translate(&self.vault, e))
    }

    fn id_digest(&self, kind: memex_types::DocKind) -> Result<memex_types::IdDigest, StoreError> {
        self.inner.id_digest(kind).map_err(|e| translate(&self.vault, e))
    }

    fn audit_checks(&self, run: &ObjectId, findings_only: bool, after: Option<&ObjectId>, limit: usize) -> Result<Vec<memex_types::AuditCheckRow>, StoreError> {
        self.inner.audit_checks(run, findings_only, after, limit).map_err(|e| translate(&self.vault, e))
    }

    fn claim_counted_correct(&self, claim: &ObjectId) -> Result<bool, StoreError> {
        self.inner.claim_counted_correct(claim).map_err(|e| translate(&self.vault, e))
    }

    fn head(&self) -> Result<Option<u64>, StoreError> {
        self.inner.head().map_err(|e| translate(&self.vault, e))
    }

    fn get(&self, key: &RowKey) -> Result<Option<Row>, StoreError> {
        self.inner.get(key).map_err(|e| translate(&self.vault, e))
    }

    fn get_many(&self, keys: &[RowKey]) -> Result<Vec<Option<Row>>, StoreError> {
        self.inner.get_many(keys).map_err(|e| translate(&self.vault, e))
    }

    fn rows(&self, table: Table, after: Option<&ObjectId>) -> Result<Vec<Row>, StoreError> {
        self.inner.rows(table, after).map_err(|e| translate(&self.vault, e))
    }

    fn events(&self, after: Option<u64>) -> Result<Vec<Event>, StoreError> {
        self.inner.events(after).map_err(|e| translate(&self.vault, e))
    }
}

fn lease_of(lease: &WriterLease) -> PgLease {
    PgLease {
        vault: lease.vault.clone(),
        token: lease.token,
    }
}

impl Store for PostgresStore {
    type Snap<'a> = PostgresSnapshot<'a>;

    fn open_vault(&self, vault: &VaultId) -> Result<WriterLease, StoreError> {
        let l = self.0.open_vault(vault).map_err(|e| translate(vault, e))?;
        Ok(WriterLease {
            vault: l.vault,
            token: l.token,
        })
    }

    fn close_vault(&self, lease: &WriterLease) -> Result<(), StoreError> {
        self.0.close_vault(&lease_of(lease)).map_err(|e| translate(&lease.vault, e))
    }

    fn snapshot(&self, vault: &VaultId) -> Result<PostgresSnapshot<'_>, StoreError> {
        Ok(PostgresSnapshot {
            inner: self.0.snapshot(vault).map_err(|e| translate(vault, e))?,
            vault: vault.clone(),
        })
    }

    fn commit_batch(&self, lease: &WriterLease, batch: &[CommitItem<'_>]) -> Result<Receipt, StoreError> {
        let items: Vec<(&Event, &[Mutation])> = batch.iter().map(|i| (i.event, i.mutations)).collect();
        let seq = self.0.commit_batch(&lease_of(lease), &items).map_err(|e| translate(&lease.vault, e))?;
        Ok(Receipt { seq })
    }

    fn cache_marks(&self, vault: &VaultId) -> Result<memex_core::CacheMarks, StoreError> {
        let (head, generation) = self.0.cache_marks(vault).map_err(|e| translate(vault, e))?;
        Ok(memex_core::CacheMarks { head, generation })
    }

    fn bump_cache_generation(&self, lease: &WriterLease) -> Result<u64, StoreError> {
        self.0.bump_cache_generation(&lease_of(lease)).map_err(|e| translate(&lease.vault, e))
    }

    fn delete_vault(&self, lease: &WriterLease) -> Result<(), StoreError> {
        self.0.delete_vault(&lease_of(lease)).map_err(|e| translate(&lease.vault, e))
    }

    fn compact(&self, lease: &WriterLease) -> Result<(), StoreError> {
        self.0.compact(&lease_of(lease)).map_err(|e| translate(&lease.vault, e))
    }

    fn chat_create_thread(&self, vault: &VaultId, thread: &memex_types::ChatThread) -> Result<bool, StoreError> {
        self.0.chat_create_thread(vault, thread).map_err(|e| translate(vault, e))
    }

    fn chat_thread(&self, vault: &VaultId, key: &memex_types::ChatThreadKey) -> Result<Option<memex_types::ChatThread>, StoreError> {
        self.0.chat_thread(vault, key).map_err(|e| translate(vault, e))
    }

    fn chat_threads(&self, vault: &VaultId, owner: &str, after: Option<(i64, &str)>, limit: usize) -> Result<Vec<memex_types::ChatThread>, StoreError> {
        self.0.chat_threads(vault, owner, after, limit).map_err(|e| translate(vault, e))
    }

    fn chat_rename_thread(&self, vault: &VaultId, key: &memex_types::ChatThreadKey, title: &str, now_ms: i64) -> Result<bool, StoreError> {
        self.0.chat_rename_thread(vault, key, title, now_ms).map_err(|e| translate(vault, e))
    }

    fn chat_delete_thread(&self, vault: &VaultId, key: &memex_types::ChatThreadKey) -> Result<bool, StoreError> {
        self.0.chat_delete_thread(vault, key).map_err(|e| translate(vault, e))
    }

    fn chat_append_messages(
        &self,
        vault: &VaultId,
        key: &memex_types::ChatThreadKey,
        messages: &[memex_types::ChatMessage],
        now_ms: i64,
    ) -> Result<bool, StoreError> {
        self.0.chat_append_messages(vault, key, messages, now_ms).map_err(|e| translate(vault, e))
    }

    fn chat_messages(
        &self,
        vault: &VaultId,
        key: &memex_types::ChatThreadKey,
        after: Option<u64>,
        limit: usize,
    ) -> Result<Vec<memex_types::ChatStored>, StoreError> {
        self.0.chat_messages(vault, key, after, limit).map_err(|e| translate(vault, e))
    }

    fn chat_record_sources(&self, vault: &VaultId, key: &memex_types::ChatThreadKey, sources: &[ObjectId]) -> Result<bool, StoreError> {
        self.0.chat_record_sources(vault, key, sources).map_err(|e| translate(vault, e))
    }

    fn chat_sources(&self, vault: &VaultId, key: &memex_types::ChatThreadKey) -> Result<Vec<ObjectId>, StoreError> {
        self.0.chat_sources(vault, key).map_err(|e| translate(vault, e))
    }

    fn chat_threads_citing(&self, vault: &VaultId, source: &ObjectId, limit: usize) -> Result<Vec<memex_types::ChatThreadKey>, StoreError> {
        self.0.chat_threads_citing(vault, source, limit).map_err(|e| translate(vault, e))
    }

    fn chat_reserve(
        &self,
        vault: &VaultId,
        key: &memex_types::ChatThreadKey,
        day: i64,
        tokens: u64,
        thread_limit: u64,
        day_limit: u64,
    ) -> Result<memex_types::ChatReservation, StoreError> {
        self.0
            .chat_reserve(vault, key, day, tokens, thread_limit, day_limit)
            .map_err(|e| translate(vault, e))
    }

    fn chat_settle(&self, vault: &VaultId, key: &memex_types::ChatThreadKey, day: i64, delta: i64) -> Result<(), StoreError> {
        self.0.chat_settle(vault, key, day, delta).map_err(|e| translate(vault, e))
    }

    fn chat_day_tokens(&self, vault: &VaultId, day: i64) -> Result<u64, StoreError> {
        self.0.chat_day_tokens(vault, day).map_err(|e| translate(vault, e))
    }
}
