use crate::{CommitItem, PAGE_LIMIT, Receipt, Snapshot, Store, StoreError, WriterLease};
use memex_types::{
    AssertionRow, ChangeSetRow, EdgeRow, Event, EvidenceRow, JobRow, JobState, Mutation, ObjectId, RenditionRow, Row, RowKey, SegmentRow, SourceRow, Status,
    Table, VaultId,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default)]
struct VaultState {
    head: Option<u64>,
    rows: BTreeMap<RowKey, Row>,
    events: BTreeMap<u64, Event>,
}

/// Store calls made so far, so a test can catch an N+1 read pattern without a database.
#[derive(Debug, Default)]
pub struct CallCounts {
    pub reads: AtomicU64,
    pub commits: AtomicU64,
}

/// An in-memory Store with the same semantics as a real one. Snapshots share state until a commit copies it.
#[derive(Debug, Default)]
pub struct MemStore {
    vaults: Mutex<HashMap<VaultId, Arc<VaultState>>>,
    leases: Mutex<HashMap<VaultId, u64>>,
    next_token: AtomicU64,
    fail_next_commit: Mutex<Option<StoreError>>,
    counts: CallCounts,
}

impl MemStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn counts(&self) -> &CallCounts {
        &self.counts
    }

    /// The next commit fails with this error and applies nothing, like a transaction the backend aborted.
    pub fn fail_next_commit(&self, error: StoreError) {
        *self.fail_next_commit.lock().expect("lock") = Some(error);
    }

    fn check_lease(&self, lease: &WriterLease) -> Result<(), StoreError> {
        match self.leases.lock().expect("lock").get(&lease.vault) {
            Some(token) if *token == lease.token => Ok(()),
            _ => Err(StoreError::LeaseLost),
        }
    }
}

pub struct MemSnapshot<'a> {
    state: Arc<VaultState>,
    counts: &'a CallCounts,
}

impl MemSnapshot<'_> {
    fn count(&self) {
        self.counts.reads.fetch_add(1, Ordering::Relaxed);
    }
}

impl Snapshot for MemSnapshot<'_> {
    fn head(&self) -> Result<Option<u64>, StoreError> {
        self.count();
        Ok(self.state.head)
    }

    fn get(&self, key: &RowKey) -> Result<Option<Row>, StoreError> {
        self.count();
        Ok(self.state.rows.get(key).cloned())
    }

    fn get_many(&self, keys: &[RowKey]) -> Result<Vec<Option<Row>>, StoreError> {
        self.count();
        Ok(keys.iter().map(|k| self.state.rows.get(k).cloned()).collect())
    }

    fn rows(&self, table: Table, after: Option<&ObjectId>) -> Result<Vec<Row>, StoreError> {
        self.count();
        let page = self
            .state
            .rows
            .iter()
            .filter(|(k, _)| k.table == table && after.is_none_or(|a| &k.id > a))
            .take(PAGE_LIMIT)
            .map(|(_, r)| r.clone());
        Ok(page.collect())
    }

    fn events(&self, after: Option<u64>) -> Result<Vec<Event>, StoreError> {
        self.count();
        let from = after.map_or(0, |a| a + 1);
        Ok(self.state.events.range(from..).take(PAGE_LIMIT).map(|(_, e)| e.clone()).collect())
    }

    fn renditions_of(&self, source_id: &ObjectId) -> Result<Vec<RenditionRow>, StoreError> {
        self.count();
        Ok(self
            .state
            .rows
            .values()
            .filter_map(|r| match r {
                Row::Rendition(x) if &x.source_id == source_id => Some(x.clone()),
                _ => None,
            })
            .collect())
    }

    fn segments_of(&self, rendition_id: &ObjectId, after: Option<&ObjectId>, limit: usize) -> Result<Vec<SegmentRow>, StoreError> {
        self.count();
        Ok(self
            .state
            .rows
            .values()
            .filter_map(|r| match r {
                Row::Segment(x) if x.rendition_id.as_ref() == Some(rendition_id) && after.is_none_or(|a| &x.header.id > a) => Some(x.clone()),
                _ => None,
            })
            .take(limit)
            .collect())
    }

    fn evidence_of(&self, assertion_ids: &[ObjectId]) -> Result<Vec<EvidenceRow>, StoreError> {
        self.count();
        Ok(self
            .state
            .rows
            .values()
            .filter_map(|r| match r {
                Row::Evidence(x) if assertion_ids.contains(&x.assertion_id) => Some(x.clone()),
                _ => None,
            })
            .collect())
    }

    fn evidence_citing(&self, segment_ids: &[ObjectId]) -> Result<Vec<EvidenceRow>, StoreError> {
        self.count();
        Ok(self
            .state
            .rows
            .values()
            .filter_map(|r| match r {
                Row::Evidence(x) if segment_ids.contains(&x.segment_id) => Some(x.clone()),
                _ => None,
            })
            .collect())
    }

    fn edges_touching(&self, ids: &[ObjectId], kind: &str) -> Result<Vec<EdgeRow>, StoreError> {
        self.count();
        Ok(self
            .state
            .rows
            .values()
            .filter_map(|r| match r {
                Row::Edge(e) if e.kind == kind && e.header.status == Status::Active && (ids.contains(&e.from) || ids.contains(&e.to)) => Some(e.clone()),
                _ => None,
            })
            .collect())
    }

    fn sections_of(&self, sections: &[(ObjectId, String)], limit: usize) -> Result<Vec<SegmentRow>, StoreError> {
        self.count();
        let mut found: Vec<SegmentRow> = self
            .state
            .rows
            .values()
            .filter_map(|r| match r {
                Row::Segment(s)
                    if s.rendition_id
                        .as_ref()
                        .zip(s.heading.as_ref())
                        .is_some_and(|(r, h)| sections.iter().any(|(a, b)| a == r && b == h)) =>
                {
                    Some(s.clone())
                }
                _ => None,
            })
            .collect();
        found.sort_by(|a, b| (&a.rendition_id, a.start).cmp(&(&b.rendition_id, b.start)));
        found.truncate(limit);
        Ok(found)
    }

    fn broken_anchors(&self, after: Option<&ObjectId>, limit: usize) -> Result<Vec<EvidenceRow>, StoreError> {
        self.count();
        Ok(self
            .state
            .rows
            .values()
            .filter_map(|r| match r {
                Row::Evidence(x) if x.anchor_broken && after.is_none_or(|a| &x.header.id > a) => Some(x.clone()),
                _ => None,
            })
            .take(limit)
            .collect())
    }

    fn source_by_hash(&self, sha256: &str) -> Result<Option<SourceRow>, StoreError> {
        self.count();
        Ok(self.state.rows.values().find_map(|r| match r {
            Row::Source(s) if s.sha256 == sha256 => Some(s.clone()),
            _ => None,
        }))
    }

    fn source_versions(&self, logical_id: &ObjectId) -> Result<Vec<SourceRow>, StoreError> {
        self.count();
        let mut out: Vec<SourceRow> = self
            .state
            .rows
            .values()
            .filter_map(|r| match r {
                Row::Source(s) if &s.logical_id == logical_id => Some(s.clone()),
                _ => None,
            })
            .collect();
        out.sort_by_key(|s| s.version_no);
        Ok(out)
    }

    fn job_by_dedupe(&self, key: &str) -> Result<Option<JobRow>, StoreError> {
        self.count();
        Ok(self.state.rows.values().find_map(|r| match r {
            Row::Job(j) if j.dedupe_key == key => Some(j.clone()),
            _ => None,
        }))
    }

    fn claimable_jobs(&self, kind: &str, now_ms: i64, limit: usize) -> Result<Vec<JobRow>, StoreError> {
        self.count();
        Ok(self
            .state
            .rows
            .values()
            .filter_map(|r| match r {
                Row::Job(j) if j.kind == kind && j.claimable(now_ms) => Some(j.clone()),
                _ => None,
            })
            .take(limit)
            .collect())
    }

    fn open_job_count(&self) -> Result<u64, StoreError> {
        self.count();
        Ok(self
            .state
            .rows
            .values()
            .filter(|r| matches!(r, Row::Job(j) if matches!(j.state, JobState::Queued | JobState::Running)))
            .count() as u64)
    }

    fn jobs(&self, kind: Option<&str>, state: Option<JobState>, after: Option<&ObjectId>, limit: usize) -> Result<Vec<JobRow>, StoreError> {
        self.count();
        Ok(self
            .state
            .rows
            .values()
            .filter_map(|r| match r {
                Row::Job(j) if kind.is_none_or(|k| k == j.kind) && state.is_none_or(|s| s == j.state) && after.is_none_or(|a| &j.header.id > a) => {
                    Some(j.clone())
                }
                _ => None,
            })
            .take(limit)
            .collect())
    }

    fn changeset_by_key(&self, key: &str) -> Result<Option<ChangeSetRow>, StoreError> {
        self.count();
        Ok(self.state.rows.values().find_map(|r| match r {
            Row::ChangeSet(c) if c.idempotency_key == key => Some(c.clone()),
            _ => None,
        }))
    }

    fn assertions_about(&self, subjects: &[String]) -> Result<Vec<AssertionRow>, StoreError> {
        self.count();
        Ok(self
            .state
            .rows
            .values()
            .filter_map(|r| match r {
                Row::Assertion(a) if a.header.status == Status::Active && subjects.contains(&a.subject) => Some(a.clone()),
                _ => None,
            })
            .collect())
    }

    fn entity_type_versions(&self, names: &[String]) -> Result<Vec<memex_types::EntityTypeRow>, StoreError> {
        self.count();
        let mut out: Vec<memex_types::EntityTypeRow> = self
            .state
            .rows
            .values()
            .filter_map(|r| match r {
                Row::EntityType(t) if names.contains(&t.name) => Some(t.clone()),
                _ => None,
            })
            .collect();
        out.sort_by(|a, b| (&a.name, a.version_no).cmp(&(&b.name, b.version_no)));
        Ok(out)
    }

    fn latest_entity_types(&self, after: Option<&str>, limit: usize) -> Result<Vec<memex_types::EntityTypeRow>, StoreError> {
        self.count();
        let mut latest: BTreeMap<String, memex_types::EntityTypeRow> = BTreeMap::new();
        for r in self.state.rows.values() {
            if let Row::EntityType(t) = r
                && after.is_none_or(|a| t.name.as_str() > a)
                && latest.get(&t.name).is_none_or(|c| c.version_no < t.version_no)
            {
                latest.insert(t.name.clone(), t.clone());
            }
        }
        Ok(latest.into_values().take(limit).collect())
    }

    fn entities_of_type(&self, type_name: &str, after: Option<&ObjectId>, limit: usize) -> Result<Vec<memex_types::EntityRow>, StoreError> {
        self.count();
        Ok(self
            .state
            .rows
            .values()
            .filter_map(|r| match r {
                Row::Entity(e) if e.is_current() && e.type_name == type_name && after.is_none_or(|a| &e.header.id > a) => Some(e.clone()),
                _ => None,
            })
            .take(limit)
            .collect())
    }

    fn entities_matching(
        &self,
        type_name: &str,
        filters: &[memex_types::FieldFilter],
        after: Option<&ObjectId>,
        limit: usize,
    ) -> Result<Vec<memex_types::EntityRow>, StoreError> {
        self.count();
        Ok(self
            .state
            .rows
            .values()
            .filter_map(|r| match r {
                Row::Entity(e)
                    if e.is_current()
                        && e.type_name == type_name
                        && after.is_none_or(|a| &e.header.id > a)
                        && filters
                            .iter()
                            .all(|f| e.field(&f.field).is_some_and(|x| x.lookups.iter().any(|l| l.passes(f.op, &f.value)))) =>
                {
                    Some(e.clone())
                }
                _ => None,
            })
            .take(limit)
            .collect())
    }

    fn entities_with_value(&self, text: &str, limit: usize) -> Result<Vec<memex_types::ValueMatch>, StoreError> {
        self.count();
        let mut out = vec![];
        for r in self.state.rows.values() {
            let Row::Entity(e) = r else { continue };
            for v in e.value_rows() {
                if v.text.as_deref() == Some(text) {
                    out.push(memex_types::ValueMatch {
                        entity_id: e.header.id.clone(),
                        type_name: e.type_name.clone(),
                        field: v.field,
                    });
                }
            }
        }
        out.truncate(limit);
        Ok(out)
    }

    fn shared_values(
        &self,
        type_name: &str,
        fields: &[String],
        after: Option<(&str, &str)>,
        limit: usize,
    ) -> Result<Vec<memex_types::SharedValue>, StoreError> {
        self.count();
        let mut groups: BTreeMap<(String, String), Vec<ObjectId>> = BTreeMap::new();
        for r in self.state.rows.values() {
            let Row::Entity(e) = r else { continue };
            if e.type_name != type_name {
                continue;
            }
            for v in e.value_rows() {
                if let Some(text) = v.text
                    && fields.contains(&v.field)
                    && after.is_none_or(|(f, t)| (v.field.as_str(), text.as_str()) > (f, t))
                {
                    let ids = groups.entry((v.field, text)).or_default();
                    if !ids.contains(&e.header.id) {
                        ids.push(e.header.id.clone());
                    }
                }
            }
        }
        Ok(groups
            .into_iter()
            .filter(|(_, ids)| ids.len() > 1)
            .take(limit)
            .map(|((field, text), mut entity_ids)| {
                entity_ids.truncate(crate::MAX_SHARED_IDS);
                memex_types::SharedValue { field, text, entity_ids }
            })
            .collect())
    }

    fn assertions_matching(&self, hashes: &[String], pairs: &[(String, String)]) -> Result<Vec<AssertionRow>, StoreError> {
        self.count();
        Ok(self
            .state
            .rows
            .values()
            .filter_map(|r| match r {
                Row::Assertion(a) if assertion_matches(a, hashes, pairs) => Some(a.clone()),
                _ => None,
            })
            .collect())
    }
}

fn assertion_matches(a: &AssertionRow, hashes: &[String], pairs: &[(String, String)]) -> bool {
    a.header.status == Status::Active && (hashes.contains(&a.canonical_hash) || pairs.iter().any(|(s, p)| *s == a.subject && *p == a.predicate))
}

impl Store for MemStore {
    type Snap<'a> = MemSnapshot<'a>;

    fn open_vault(&self, vault: &VaultId) -> Result<WriterLease, StoreError> {
        let mut leases = self.leases.lock().expect("lock");
        if leases.contains_key(vault) {
            return Err(StoreError::WriterActive { vault: vault.clone() });
        }
        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        leases.insert(vault.clone(), token);
        self.vaults.lock().expect("lock").entry(vault.clone()).or_default();
        Ok(WriterLease { vault: vault.clone(), token })
    }

    fn close_vault(&self, lease: &WriterLease) -> Result<(), StoreError> {
        self.check_lease(lease)?;
        self.leases.lock().expect("lock").remove(&lease.vault);
        Ok(())
    }

    fn snapshot(&self, vault: &VaultId) -> Result<MemSnapshot<'_>, StoreError> {
        let state = self
            .vaults
            .lock()
            .expect("lock")
            .get(vault)
            .cloned()
            .ok_or_else(|| StoreError::UnknownVault { vault: vault.clone() })?;
        Ok(MemSnapshot { state, counts: &self.counts })
    }

    fn commit_batch(&self, lease: &WriterLease, batch: &[CommitItem<'_>]) -> Result<Receipt, StoreError> {
        self.counts.commits.fetch_add(1, Ordering::Relaxed);
        self.check_lease(lease)?;
        if let Some(e) = self.fail_next_commit.lock().expect("lock").take() {
            return Err(e);
        }
        let last = batch.last().ok_or(StoreError::EmptyBatch)?.event.seq;
        let mut vaults = self.vaults.lock().expect("lock");
        let slot = vaults
            .get_mut(&lease.vault)
            .ok_or_else(|| StoreError::UnknownVault { vault: lease.vault.clone() })?;
        // All checks come first, so a refused batch changes nothing.
        let mut head = slot.head;
        for item in batch {
            let want = head.map_or(0, |h| h + 1);
            if item.event.seq != want {
                return Err(StoreError::SequenceGap { head, got: item.event.seq });
            }
            head = Some(item.event.seq);
        }
        let mut exists: HashMap<RowKey, bool> = HashMap::new();
        for item in batch {
            for m in item.mutations {
                match m {
                    Mutation::Put(row) => {
                        exists.insert(row.key(), true);
                    }
                    Mutation::Delete(key) => {
                        let present = *exists.entry(key.clone()).or_insert_with(|| slot.rows.contains_key(key));
                        if !present {
                            return Err(StoreError::MissingRow { key: key.clone() });
                        }
                        exists.insert(key.clone(), false);
                    }
                }
            }
        }
        let state = Arc::make_mut(slot);
        for item in batch {
            for m in item.mutations {
                match m {
                    Mutation::Put(row) => {
                        state.rows.insert(row.key(), row.clone());
                    }
                    Mutation::Delete(key) => {
                        state.rows.remove(key);
                    }
                }
            }
            state.head = Some(item.event.seq);
            state.events.insert(item.event.seq, item.event.clone());
        }
        Ok(Receipt { seq: last })
    }

    fn delete_vault(&self, lease: &WriterLease) -> Result<(), StoreError> {
        self.check_lease(lease)?;
        self.leases.lock().expect("lock").remove(&lease.vault);
        self.vaults.lock().expect("lock").remove(&lease.vault);
        Ok(())
    }

    fn compact(&self, lease: &WriterLease) -> Result<(), StoreError> {
        // Deleted rows are already gone from memory.
        self.check_lease(lease)
    }
}
