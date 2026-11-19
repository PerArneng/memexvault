//! Request and response types for operating a vault (spec 030): erasure, reconcile, reindex, the sweep and the server's status. Ids and counts only.
use memex_core::{ErasureView, IndexCheck, ReconcileReport, ServerStatus, SweepReport, VaultStatus};
use memex_types::{ErasureRecord, RowKey};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RowKeyDto {
    /// The table: `source`, `rendition`, `segment`, `evidence`, `assertion`, `entity`, `edge`, `change_set`, `job`, `derivation` and so on.
    pub table: String,
    pub id: String,
}

impl From<&RowKey> for RowKeyDto {
    fn from(k: &RowKey) -> Self {
        Self {
            table: serde_json::to_value(k.table)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default(),
            id: k.id.0.clone(),
        }
    }
}

/// What an erasure removes, rewrites and flags. A preview and the record of a done erasure have the same shape, so they can be compared.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ErasureImpactDto {
    /// The document. Every version of it goes.
    pub logical_id: String,
    pub sources: Vec<String>,
    /// Rows that go, in key order.
    pub removed: Vec<RowKeyDto>,
    /// Rows that stay, with what came from the document taken out.
    pub rewritten: Vec<RowKeyDto>,
    /// Claims that stand on other evidence too, and lost what they had from this document.
    pub flagged_claims: Vec<String>,
    /// Pages that link to the document or refer to a claim that goes. Their text isn't changed.
    pub flagged_pages: Vec<String>,
}

impl From<&ErasureRecord> for ErasureImpactDto {
    fn from(r: &ErasureRecord) -> Self {
        Self {
            logical_id: r.logical_id.0.clone(),
            sources: r.sources.iter().map(|s| s.0.clone()).collect(),
            removed: r.removed.iter().map(Into::into).collect(),
            rewritten: r.rewritten.iter().map(Into::into).collect(),
            flagged_claims: r.flagged_claims.iter().map(|c| c.0.clone()).collect(),
            flagged_pages: r.flagged_pages.iter().map(|p| p.0.clone()).collect(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ErasureDto {
    /// The `source.erased` event.
    pub seq: u64,
    pub time_ms: i64,
    /// Who approved it.
    pub actor: String,
    /// The proposal it came from.
    pub changeset_id: String,
    pub impact: ErasureImpactDto,
}

impl From<ErasureView> for ErasureDto {
    fn from(v: ErasureView) -> Self {
        Self {
            seq: v.seq,
            time_ms: v.time_ms,
            actor: v.actor,
            changeset_id: v.record.changeset_id.0.clone(),
            impact: (&v.record).into(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ErasurePage {
    pub items: Vec<ErasureDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EraseBody {
    /// Why, for the person who decides.
    pub rationale: String,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ReconcileQuery {
    /// Also replay the log up to this event, to compare with a backup taken there.
    pub at: Option<u64>,
    /// Re-hash every blob, which reads all of them.
    pub deep: Option<bool>,
    /// Rebuild an index that differs, which is derived data. The log and tables are never changed.
    pub repair: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DigestDto {
    pub count: u64,
    pub hash: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct IndexCheckDto {
    /// `keyword` or `vector`.
    pub index: String,
    /// `segment` or `claim`.
    pub kind: String,
    pub store: DigestDto,
    pub index_digest: Option<DigestDto>,
    pub watermark: Option<u64>,
    pub behind: bool,
    pub matches: bool,
}

impl From<&IndexCheck> for IndexCheckDto {
    fn from(c: &IndexCheck) -> Self {
        let digest = |d: &memex_types::IdDigest| DigestDto {
            count: d.count,
            hash: format!("{:016x}", d.xor),
        };
        Self {
            index: c.index.clone(),
            kind: c.kind.as_str().into(),
            store: digest(&c.store),
            index_digest: c.index_digest.as_ref().map(digest),
            watermark: c.watermark,
            behind: c.behind,
            matches: c.matches,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReconcileDto {
    /// No difference anywhere, an index that's only behind not counting.
    pub clean: bool,
    pub head: Option<u64>,
    pub events: u64,
    pub rows: u64,
    pub tables_sha256: String,
    pub replayed_sha256: String,
    /// The replayed state at `at`, when asked for.
    pub at_seq: Option<u64>,
    pub at_sha256: Option<String>,
    pub replay_error: Option<String>,
    pub differing: Vec<RowKeyDto>,
    pub differing_count: u64,
    pub blobs_referenced: u64,
    pub blobs_stored: u64,
    pub blob_bytes: u64,
    pub blobs_missing: Vec<String>,
    pub blob_orphans: u64,
    pub blobs_corrupt: Vec<String>,
    pub indexes: Vec<IndexCheckDto>,
    pub repaired: Vec<String>,
}

impl From<ReconcileReport> for ReconcileDto {
    fn from(r: ReconcileReport) -> Self {
        Self {
            clean: r.clean(),
            head: r.log.head,
            events: r.log.events,
            rows: r.log.rows,
            at_seq: r.log.at.as_ref().map(|(s, _)| *s),
            at_sha256: r.log.at.as_ref().map(|(_, h)| h.clone()),
            replay_error: r.log.replay_error.clone(),
            differing: r.log.differing.iter().map(Into::into).collect(),
            differing_count: r.log.differing_count,
            blobs_referenced: r.blobs.referenced,
            blobs_stored: r.blobs.stored,
            blob_bytes: r.blobs.stored_bytes,
            blob_orphans: r.blobs.orphans,
            indexes: r.indexes.iter().map(Into::into).collect(),
            tables_sha256: r.log.tables_sha256,
            replayed_sha256: r.log.replayed_sha256,
            blobs_missing: r.blobs.missing,
            blobs_corrupt: r.blobs.corrupt,
            repaired: r.repaired,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReindexBody {
    /// `keyword`, `vector` or `all`.
    pub index: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SweepBody {
    /// How old an unreferenced blob must be to go. The server's `MEMEX_SWEEP_GRACE_S` when absent.
    pub grace_s: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SweepDto {
    pub checked: u64,
    pub removed: u64,
    pub removed_bytes: u64,
    pub kept_young: u64,
}

impl From<SweepReport> for SweepDto {
    fn from(r: SweepReport) -> Self {
        Self {
            checked: r.checked,
            removed: r.removed,
            removed_bytes: r.removed_bytes,
            kept_young: r.kept_young,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct IndexStatusDto {
    pub ready: bool,
    pub watermark: Option<u64>,
    /// Events not applied yet.
    pub lag: u64,
    pub building: bool,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct JobCountDto {
    pub kind: String,
    pub count: u64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct VaultStatusDto {
    pub id: String,
    pub name: String,
    pub head: Option<u64>,
    pub keyword: IndexStatusDto,
    pub vector: IndexStatusDto,
    /// Queued or running jobs by kind.
    pub open_jobs: Vec<JobCountDto>,
    pub dead_jobs: u64,
    pub review_pending: u64,
    pub blobs: u64,
    pub blob_bytes: u64,
    pub cache_generation: u64,
}

impl From<VaultStatus> for VaultStatusDto {
    fn from(v: VaultStatus) -> Self {
        let index = |i: memex_core::IndexStatus| IndexStatusDto {
            ready: i.ready,
            watermark: i.watermark,
            lag: i.lag,
            building: i.building,
        };
        Self {
            id: v.id.0,
            name: v.name,
            head: v.head,
            keyword: index(v.keyword),
            vector: index(v.vector),
            open_jobs: v.open_jobs.into_iter().map(|(kind, count)| JobCountDto { kind, count }).collect(),
            dead_jobs: v.dead_jobs,
            review_pending: v.review_pending,
            blobs: v.blobs,
            blob_bytes: v.blob_bytes,
            cache_generation: v.cache_generation,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StatusDto {
    pub vaults: Vec<VaultStatusDto>,
    pub caches: Vec<crate::CacheTierDto>,
}

impl From<ServerStatus> for StatusDto {
    fn from(s: ServerStatus) -> Self {
        Self {
            vaults: s.vaults.into_iter().map(Into::into).collect(),
            caches: s.caches.into_iter().map(Into::into).collect(),
        }
    }
}
