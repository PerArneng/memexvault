//! What an erasure removes (spec 030). Ids and counts only: nothing here may hold content.
use crate::{ObjectId, RowKey, Table};
use serde::{Deserialize, Serialize};

/// The event kind that records an erasure. Its payload is an `ErasureRecord`.
pub const SOURCE_ERASED: &str = "source.erased";

/// The payload of a `source.erased` event, and what an impact preview lists. Every field is ids, so the log keeps no content of what went.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ErasureRecord {
    /// The approved proposal. Empty on a preview.
    #[serde(default)]
    pub changeset_id: ObjectId,
    /// The document, whose every version goes.
    pub logical_id: ObjectId,
    pub sources: Vec<ObjectId>,
    /// Rows that go, in key order.
    pub removed: Vec<RowKey>,
    /// Rows that stay with what came from the erased document taken out of them, in key order.
    pub rewritten: Vec<RowKey>,
    /// Claims that stand on other evidence too, and lost what they had from this document.
    pub flagged_claims: Vec<ObjectId>,
    /// Pages that link to the document or refer to a claim that goes.
    pub flagged_pages: Vec<ObjectId>,
}

impl ErasureRecord {
    pub fn removed_in(&self, table: Table) -> impl Iterator<Item = &ObjectId> {
        self.removed.iter().filter(move |k| k.table == table).map(|k| &k.id)
    }

    pub fn rewritten_in(&self, table: Table) -> impl Iterator<Item = &ObjectId> {
        self.rewritten.iter().filter(move |k| k.table == table).map(|k| &k.id)
    }
}

/// The job that does an erasure's physical work once its event has committed: blobs, index files, chat threads, compaction and caches.
pub const ERASE_JOB: &str = "erase";

/// An erase job's input. The hashes say which blobs and stored embeddings go. They can show that a known file or text was in the vault, but can't give it back.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EraseJobInput {
    /// The `source.erased` event.
    pub seq: u64,
    pub blobs: Vec<String>,
    /// Content hashes of the texts that went, whose stored embeddings go too.
    pub embeddings: Vec<String>,
}
