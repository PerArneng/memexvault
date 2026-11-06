use crate::{
    BlobStore, BlobStoreError, Clock, CommitItem, Pending, Receipt, ReduceError, Snapshot, Store, StoreError, UpcastError, WriterLease, reduce, upcast,
};
use memex_types::{Event, EventDraft, ObjectId};
use serde_json::json;
use std::fmt;
use std::io::Read;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyError {
    Upcast(UpcastError),
    Reduce(ReduceError),
    Store(StoreError),
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Upcast(e) => write!(f, "{e}"),
            Self::Reduce(e) => write!(f, "{e}"),
            Self::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ApplyError {}

impl From<StoreError> for ApplyError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

/// What a caller knows about a new Source, apart from its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSource {
    pub id: ObjectId,
    pub actor: String,
    pub media_type: String,
    pub trust_tier: String,
    pub name: String,
}

#[derive(Debug)]
pub enum RecordSourceError {
    Blob(BlobStoreError),
    Apply(ApplyError),
}

impl fmt::Display for RecordSourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blob(e) => write!(f, "{e}"),
            Self::Apply(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RecordSourceError {}

/// The single entry point to the core. Later specs add a method per use case.
pub struct Engine<S: Store, C: Clock> {
    store: S,
    clock: C,
}

impl<S: Store, C: Clock> Engine<S, C> {
    pub fn new(store: S, clock: C) -> Self {
        Self { store, clock }
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn clock(&self) -> &C {
        &self.clock
    }

    /// Stores the bytes durably, then commits the reference. A failure can leave an orphan blob, which the sweep removes, but never a reference to a missing blob.
    pub fn record_source<B: BlobStore>(&self, blobs: &B, lease: &WriterLease, source: NewSource, bytes: &mut dyn Read) -> Result<Receipt, RecordSourceError> {
        let stored = blobs.put(&lease.vault, bytes).map_err(RecordSourceError::Blob)?;
        let draft = EventDraft {
            actor: source.actor,
            kind: "source.recorded".into(),
            schema_version: 3,
            payload: json!({"id": source.id, "logical_id": source.id, "version_no": 1, "name": source.name, "size": stored.size, "metadata": {}, "sha256": stored.hash, "media_type": source.media_type, "trust_tier": source.trust_tier}),
        };
        self.apply(lease, draft).map_err(RecordSourceError::Apply)
    }

    /// Group commit: reduces each draft on top of the earlier ones and commits them in one transaction. A refused draft gets its own error and takes no sequence.
    /// If the transaction fails, every accepted draft gets that error.
    pub fn apply_batch(&self, lease: &WriterLease, drafts: Vec<EventDraft>) -> Vec<Result<Receipt, ApplyError>> {
        let snap = match self.store.snapshot(&lease.vault) {
            Ok(s) => s,
            Err(e) => return drafts.iter().map(|_| Err(ApplyError::Store(e.clone()))).collect(),
        };
        let mut pending = match Pending::over(&snap) {
            Ok(p) => p,
            Err(e) => return drafts.iter().map(|_| Err(ApplyError::Store(e.clone()))).collect(),
        };
        let keys = drafts.iter().filter_map(|d| crate::reducer::read_keys(&d.kind, &d.payload)).collect();
        if let Err(err) = pending.prefetch(keys) {
            return drafts.iter().map(|_| Err(ApplyError::Store(err.clone()))).collect();
        }
        let now = self.clock.now_ms();
        let mut accepted: Vec<(usize, Event, Vec<memex_types::Mutation>)> = vec![];
        let mut results: Vec<Result<Receipt, ApplyError>> = Vec::with_capacity(drafts.len());
        for (i, draft) in drafts.into_iter().enumerate() {
            let seq = pending.next_seq();
            let event = Event {
                seq,
                vault: lease.vault.clone(),
                actor: draft.actor,
                time_ms: now,
                kind: draft.kind,
                schema_version: draft.schema_version,
                payload: draft.payload,
            };
            let outcome = upcast(&event)
                .map_err(ApplyError::Upcast)
                .and_then(|current| reduce(&pending, &current).map_err(ApplyError::Reduce));
            match outcome {
                Ok(mutations) => {
                    pending.apply(seq, &mutations);
                    results.push(Ok(Receipt { seq }));
                    accepted.push((i, event, mutations));
                }
                Err(e) => results.push(Err(e)),
            }
        }
        drop(pending);
        drop(snap);
        if accepted.is_empty() {
            return results;
        }
        let items: Vec<CommitItem<'_>> = accepted.iter().map(|(_, event, mutations)| CommitItem { event, mutations }).collect();
        if let Err(e) = self.store.commit_batch(lease, &items) {
            for (i, _, _) in &accepted {
                results[*i] = Err(ApplyError::Store(e.clone()));
            }
        }
        results
    }

    /// Assigns the next sequence, reduces and commits once. An error leaves no trace.
    pub fn apply(&self, lease: &WriterLease, draft: EventDraft) -> Result<Receipt, ApplyError> {
        let (event, mutations) = {
            let snap = self.store.snapshot(&lease.vault)?;
            let seq = snap.head()?.map_or(0, |h| h + 1);
            let event = Event {
                seq,
                vault: lease.vault.clone(),
                actor: draft.actor,
                time_ms: self.clock.now_ms(),
                kind: draft.kind,
                schema_version: draft.schema_version,
                payload: draft.payload,
            };
            let current = upcast(&event).map_err(ApplyError::Upcast)?;
            let mutations = reduce(&snap, &current).map_err(ApplyError::Reduce)?;
            (event, mutations)
        };
        Ok(self.store.commit(lease, &event, &mutations)?)
    }
}
