use crate::{Snapshot, StoreError};
use memex_types::{
    AssertionRow, ChangeSetRow, Event, EvidenceRow, JobRow, JobState, Mutation, ObjectId, RenditionRow, Row, RowKey, SegmentRow, SourceRow, Table,
};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchLimits {
    pub max_events: usize,
    pub max_bytes: usize,
}

/// How many waiting events go into the next transaction, given each one's payload size. The group-commit decision, so it's pure.
/// It always takes at least one, so an oversized event still gets through alone.
pub fn plan_batch(waiting_sizes: &[usize], limits: BatchLimits) -> usize {
    let mut bytes = 0usize;
    let mut take = 0;
    for size in waiting_sizes.iter().take(limits.max_events.max(1)) {
        bytes = bytes.saturating_add(*size);
        if take > 0 && bytes > limits.max_bytes {
            break;
        }
        take += 1;
    }
    take
}

/// A snapshot with uncommitted mutations on top, so each event in a batch reduces against the ones before it.
pub struct Pending<'a, S: Snapshot> {
    base: &'a S,
    head: Option<u64>,
    rows: HashMap<RowKey, Option<Row>>,
}

impl<'a, S: Snapshot> Pending<'a, S> {
    pub fn over(base: &'a S) -> Result<Self, StoreError> {
        Ok(Self {
            base,
            head: base.head()?,
            rows: HashMap::new(),
        })
    }

    /// Fetches rows the batch will read, in one store call, so reducing a batch doesn't read once per event.
    pub fn prefetch(&mut self, keys: Vec<RowKey>) -> Result<(), StoreError> {
        let wanted: Vec<RowKey> = keys.into_iter().filter(|k| !self.rows.contains_key(k)).collect();
        if wanted.is_empty() {
            return Ok(());
        }
        let found = self.base.get_many(&wanted)?;
        self.rows.extend(wanted.into_iter().zip(found));
        Ok(())
    }

    pub fn next_seq(&self) -> u64 {
        self.head.map_or(0, |h| h + 1)
    }

    pub fn apply(&mut self, seq: u64, mutations: &[Mutation]) {
        self.head = Some(seq);
        for m in mutations {
            match m {
                Mutation::Put(row) => {
                    self.rows.insert(row.key(), Some(row.clone()));
                }
                Mutation::Delete(key) => {
                    self.rows.insert(key.clone(), None);
                }
            }
        }
    }
}

impl<S: Snapshot> Snapshot for Pending<'_, S> {
    fn head(&self) -> Result<Option<u64>, StoreError> {
        Ok(self.head)
    }

    fn get(&self, key: &RowKey) -> Result<Option<Row>, StoreError> {
        match self.rows.get(key) {
            Some(pending) => Ok(pending.clone()),
            None => self.base.get(key),
        }
    }

    fn get_many(&self, keys: &[RowKey]) -> Result<Vec<Option<Row>>, StoreError> {
        // One base read for whatever the batch hasn't touched.
        let missing: Vec<RowKey> = keys.iter().filter(|k| !self.rows.contains_key(*k)).cloned().collect();
        let mut from_base = self.base.get_many(&missing)?.into_iter();
        Ok(keys
            .iter()
            .map(|k| match self.rows.get(k) {
                Some(pending) => pending.clone(),
                None => from_base.next().flatten(),
            })
            .collect())
    }

    /// Committed rows only. Reducers look up by key, and paged scans don't see the batch.
    fn rows(&self, table: Table, after: Option<&ObjectId>) -> Result<Vec<Row>, StoreError> {
        self.base.rows(table, after)
    }

    /// Committed rows only, as with `rows`.
    fn edges_touching(&self, ids: &[ObjectId], kind: &str) -> Result<Vec<memex_types::EdgeRow>, StoreError> {
        self.base.edges_touching(ids, kind)
    }

    /// Committed rows only, as with `rows`.
    fn sections_of(&self, sections: &[(ObjectId, String)], limit: usize) -> Result<Vec<memex_types::SegmentRow>, StoreError> {
        self.base.sections_of(sections, limit)
    }

    /// Committed events only, as with `rows`.
    fn events(&self, after: Option<u64>) -> Result<Vec<Event>, StoreError> {
        self.base.events(after)
    }

    /// Committed rows only, as with `changeset_by_key`.
    fn renditions_of(&self, source_id: &ObjectId) -> Result<Vec<RenditionRow>, StoreError> {
        self.base.renditions_of(source_id)
    }

    /// Committed rows only, as with `changeset_by_key`.
    fn segments_of(&self, rendition_id: &ObjectId, after: Option<&ObjectId>, limit: usize) -> Result<Vec<SegmentRow>, StoreError> {
        self.base.segments_of(rendition_id, after, limit)
    }

    /// Committed rows only, as with `changeset_by_key`.
    fn evidence_of(&self, assertion_ids: &[ObjectId]) -> Result<Vec<EvidenceRow>, StoreError> {
        self.base.evidence_of(assertion_ids)
    }

    /// Committed rows only, as with `changeset_by_key`.
    fn evidence_citing(&self, segment_ids: &[ObjectId]) -> Result<Vec<EvidenceRow>, StoreError> {
        self.base.evidence_citing(segment_ids)
    }

    /// Committed rows only, as with `changeset_by_key`.
    fn broken_anchors(&self, after: Option<&ObjectId>, limit: usize) -> Result<Vec<EvidenceRow>, StoreError> {
        self.base.broken_anchors(after, limit)
    }

    /// Committed rows only, as with `changeset_by_key`.
    fn source_by_hash(&self, sha256: &str) -> Result<Option<SourceRow>, StoreError> {
        self.base.source_by_hash(sha256)
    }

    /// Committed rows only, as with `changeset_by_key`.
    fn source_versions(&self, logical_id: &ObjectId) -> Result<Vec<SourceRow>, StoreError> {
        self.base.source_versions(logical_id)
    }

    /// Committed rows only, as with `changeset_by_key`.
    fn job_by_dedupe(&self, key: &str) -> Result<Option<JobRow>, StoreError> {
        self.base.job_by_dedupe(key)
    }

    /// Committed rows only, as with `changeset_by_key`.
    fn claimable_jobs(&self, kind: &str, now_ms: i64, limit: usize) -> Result<Vec<JobRow>, StoreError> {
        self.base.claimable_jobs(kind, now_ms, limit)
    }

    /// Committed rows only, as with `changeset_by_key`.
    fn open_job_count(&self) -> Result<u64, StoreError> {
        self.base.open_job_count()
    }

    /// Committed rows only, as with `changeset_by_key`.
    fn jobs(&self, kind: Option<&str>, state: Option<JobState>, after: Option<&ObjectId>, limit: usize) -> Result<Vec<JobRow>, StoreError> {
        self.base.jobs(kind, state, after, limit)
    }

    /// Committed rows only. Proposals don't go through batches.
    fn changeset_by_key(&self, key: &str) -> Result<Option<ChangeSetRow>, StoreError> {
        self.base.changeset_by_key(key)
    }

    /// Committed rows only, as with `changeset_by_key`.
    fn assertions_matching(&self, hashes: &[String], pairs: &[(String, String)]) -> Result<Vec<AssertionRow>, StoreError> {
        self.base.assertions_matching(hashes, pairs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: BatchLimits = BatchLimits { max_events: 3, max_bytes: 100 };

    #[test]
    fn empty_queue_takes_nothing() {
        assert_eq!(plan_batch(&[], LIMITS), 0);
    }

    #[test]
    fn stops_at_the_event_limit() {
        assert_eq!(plan_batch(&[1, 1, 1, 1, 1], LIMITS), 3);
    }

    #[test]
    fn stops_before_exceeding_the_byte_limit() {
        assert_eq!(plan_batch(&[60, 60, 1], LIMITS), 1);
        assert_eq!(plan_batch(&[40, 60, 1], LIMITS), 2);
    }

    #[test]
    fn an_oversized_event_still_goes_alone() {
        assert_eq!(plan_batch(&[5000, 1], LIMITS), 1);
    }

    #[test]
    fn a_zero_event_limit_still_makes_progress() {
        assert_eq!(plan_batch(&[1, 1], BatchLimits { max_events: 0, max_bytes: 100 }), 1);
    }
}
