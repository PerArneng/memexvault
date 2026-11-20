//! Operating a vault with nothing but the server (spec 030): reconcile, reindex, the orphan sweep, and the status the metrics come from.
use super::{Server, ServerEdges, ServerError};
use crate::auth::{Caller, Operation};
use crate::{BlobStore, Caches, LexicalIndex, Registry, Snapshot, Store, VectorIndex};
use memex_types::{BlobHash, DocKind, IdDigest, IndexHealth, JobState, ObjectId, Row, RowKey, Table, VaultId, VectorHealth};
use std::collections::{BTreeSet, HashSet};

/// The most differing rows a report names. The count is always whole.
pub const REPORT_LIMIT: usize = 100;

/// How the tables compare with the log they come from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LogCheck {
    pub head: Option<u64>,
    pub events: u64,
    pub rows: u64,
    /// The canonical dump of the tables, and of the log replayed into memory. Equal when the tables are exactly what the log says.
    pub tables_sha256: String,
    pub replayed_sha256: String,
    /// Keys whose rows differ, at most `REPORT_LIMIT`.
    pub differing: Vec<RowKey>,
    pub differing_count: u64,
    /// The replayed state at an earlier event, such as a backup's, to compare with a restored copy.
    pub at: Option<(u64, String)>,
    /// Why the log couldn't be replayed, if it couldn't.
    pub replay_error: Option<String>,
}

/// How the blobs compare with the rows that refer to them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlobCheck {
    pub referenced: u64,
    pub stored: u64,
    pub stored_bytes: u64,
    /// Referred to and not stored. Nothing can repair these but a backup.
    pub missing: Vec<String>,
    /// Stored and referred to by nothing, which the sweep removes after its grace period.
    pub orphans: u64,
    /// Stored bytes that no longer hash to their name. Only a deep check reads them.
    pub corrupt: Vec<String>,
}

/// One index against the store, for one kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexCheck {
    /// `keyword` or `vector`.
    pub index: String,
    pub kind: DocKind,
    pub store: IdDigest,
    /// None when the index has no live generation or can't say.
    pub index_digest: Option<IdDigest>,
    pub watermark: Option<u64>,
    /// The index hasn't reached the log's head, so a difference may only be lag.
    pub behind: bool,
    pub matches: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReconcileReport {
    pub log: LogCheck,
    pub blobs: BlobCheck,
    pub indexes: Vec<IndexCheck>,
    /// What a repair started: derived data only, never the log or the tables.
    pub repaired: Vec<String>,
}

impl ReconcileReport {
    /// No difference anywhere, counting an index that's only behind as no difference.
    pub fn clean(&self) -> bool {
        self.log.replay_error.is_none()
            && self.log.tables_sha256 == self.log.replayed_sha256
            && self.blobs.missing.is_empty()
            && self.blobs.corrupt.is_empty()
            && self.indexes.iter().all(|i| i.matches || i.behind)
    }
}

/// Which indexes to build again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReindexTarget {
    Keyword,
    Vector,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SweepReport {
    pub checked: u64,
    pub removed: u64,
    pub removed_bytes: u64,
    /// Unreferenced but younger than the grace period.
    pub kept_young: u64,
}

/// One index's state, for status and metrics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexStatus {
    pub ready: bool,
    pub watermark: Option<u64>,
    /// Events the index hasn't applied yet.
    pub lag: u64,
    pub building: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultStatus {
    pub id: VaultId,
    pub name: String,
    pub head: Option<u64>,
    pub keyword: IndexStatus,
    pub vector: IndexStatus,
    /// Queued or running jobs, by kind.
    pub open_jobs: Vec<(String, u64)>,
    pub dead_jobs: u64,
    pub review_pending: u64,
    pub blobs: u64,
    pub blob_bytes: u64,
    pub cache_generation: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerStatus {
    pub vaults: Vec<VaultStatus>,
    pub caches: Vec<memex_types::CacheTierStats>,
}

/// Job kinds the status counts one by one. Anything else is counted under `other`.
const STATUS_KINDS: [&str; 7] = [
    crate::RENDITION_JOB,
    crate::TRANSCRIBE_JOB,
    crate::EMBED_JOB,
    crate::EXTRACT_JOB,
    crate::REVIEW_JOB,
    crate::AUDIT_JOB,
    memex_types::ERASE_JOB,
];

fn digest_of_rows(rows: &[Row]) -> std::collections::BTreeMap<RowKey, String> {
    rows.iter()
        .map(|r| (r.key(), serde_json::to_value(r).unwrap_or(serde_json::Value::Null).to_string()))
        .collect()
}

fn all_rows<S: Snapshot>(snap: &S) -> Result<Vec<Row>, crate::StoreError> {
    let mut rows = vec![];
    for table in Table::ALL {
        let mut after: Option<ObjectId> = None;
        loop {
            let page = snap.rows(table, after.as_ref())?;
            let Some(last) = page.last() else { break };
            after = Some(last.header().id.clone());
            rows.extend(page);
        }
    }
    Ok(rows)
}

impl<E: ServerEdges> Server<E> {
    /// Compares the log, the tables, the blobs and the indexes, with counts and hashes. Replays the log into memory, so it holds the vault's rows twice while it runs. `at` also replays up to that event. `deep` re-hashes every blob. `repair` rebuilds an index that differs and isn't just behind, which is derived data. The log and the tables are never changed.
    pub fn reconcile(&self, caller: &Caller, vault: &VaultId, at: Option<u64>, deep: bool, repair: bool) -> Result<ReconcileReport, ServerError> {
        self.allow(caller, Operation::VaultOperate(vault))?;
        self.vault_exists(vault)?;
        self.reconcile_report(vault, at, deep, repair)
    }

    /// `reconcile` without the caller's check, for the operation job that runs it.
    pub(super) fn reconcile_report(&self, vault: &VaultId, at: Option<u64>, deep: bool, repair: bool) -> Result<ReconcileReport, ServerError> {
        let mut report = self.reconcile_store(vault, at, deep)?;
        report.indexes = self.ops_index_checks(vault, report.log.head)?;
        if repair {
            if report.indexes.iter().any(|i| i.index == "keyword" && !i.matches && !i.behind) {
                self.rebuild_lexical(vault);
                report.repaired.push("keyword index rebuild started".into());
            }
            if report.indexes.iter().any(|i| i.index == "vector" && !i.matches && !i.behind) {
                self.rebuild_vector(vault);
                report.repaired.push("vector index rebuild started".into());
            }
        }
        Ok(report)
    }

    /// The log, tables and blobs part of `reconcile`, which needs nothing but the store and the blobs. The binary's `reconcile` command runs this beside a running server.
    pub fn reconcile_store(&self, vault: &VaultId, at: Option<u64>, deep: bool) -> Result<ReconcileReport, ServerError> {
        let snap = self.engine.store().snapshot(vault)?;
        let head = snap.head()?;
        let rows = all_rows(&snap)?;
        let events = crate::all_events(&snap)?;
        let tables = crate::canonical_dump(head, &rows);
        let mut log = LogCheck {
            head,
            events: events.len() as u64,
            rows: rows.len() as u64,
            tables_sha256: tables.sha256,
            ..LogCheck::default()
        };
        let mem = crate::testing::MemStore::new();
        let lease = mem.open_vault(vault)?;
        let (before, after): (Vec<_>, Vec<_>) = events.into_iter().partition(|e| at.is_some_and(|a| e.seq <= a));
        let replayed = (|| -> Result<(), crate::ReplayError> {
            if let Some(a) = at {
                crate::replay(before, &mem, &lease)?;
                log.at = Some((a, crate::dump_store(&mem.snapshot(vault)?)?.sha256));
                crate::replay(after, &mem, &lease)?;
            } else {
                crate::replay(after, &mem, &lease)?;
            }
            Ok(())
        })();
        match replayed {
            Err(e) => log.replay_error = Some(e.to_string()),
            Ok(()) => {
                let mem_snap = mem.snapshot(vault)?;
                let mem_rows = all_rows(&mem_snap)?;
                log.replayed_sha256 = crate::canonical_dump(mem_snap.head()?, &mem_rows).sha256;
                if log.replayed_sha256 != log.tables_sha256 {
                    let (a, b) = (digest_of_rows(&rows), digest_of_rows(&mem_rows));
                    let keys: BTreeSet<&RowKey> = a.keys().chain(b.keys()).collect();
                    for k in keys {
                        if a.get(k) != b.get(k) {
                            log.differing_count += 1;
                            if log.differing.len() < REPORT_LIMIT {
                                log.differing.push(k.clone());
                            }
                        }
                    }
                }
            }
        }
        let referenced = crate::erasure::referenced_blobs(&snap)?;
        drop(snap);
        let mut blobs = BlobCheck {
            referenced: referenced.len() as u64,
            ..BlobCheck::default()
        };
        let mut stored: HashSet<String> = HashSet::new();
        let mut after: Option<BlobHash> = None;
        loop {
            let page = self.blobs.list(vault, after.as_ref(), crate::PAGE_LIMIT)?;
            let Some(last) = page.last() else { break };
            after = Some(last.hash.clone());
            for b in page {
                blobs.stored += 1;
                blobs.stored_bytes += b.size;
                if !referenced.contains(b.hash.as_str()) {
                    blobs.orphans += 1;
                }
                if deep && !matches!(self.blobs.verify(vault, &b.hash)?, crate::Verified::Ok) {
                    blobs.corrupt.push(b.hash.as_str().to_string());
                }
                stored.insert(b.hash.as_str().to_string());
            }
        }
        let mut missing: Vec<String> = referenced.into_iter().filter(|h| !h.is_empty() && !stored.contains(h)).collect();
        missing.sort();
        blobs.missing = missing;
        Ok(ReconcileReport {
            log,
            blobs,
            ..ReconcileReport::default()
        })
    }

    fn ops_index_checks(&self, vault: &VaultId, head: Option<u64>) -> Result<Vec<IndexCheck>, ServerError> {
        let snap = self.engine.store().snapshot(vault)?;
        let keyword_mark = match self.lexical.health(vault, false)? {
            IndexHealth::Ready { watermark } => Some(watermark),
            _ => None,
        };
        let vector_mark = match self.vector.health(vault)? {
            VectorHealth::Ready { watermark, .. } => Some(watermark),
            VectorHealth::Missing => None,
        };
        let mut out = vec![];
        for kind in [DocKind::Segment, DocKind::Claim] {
            let store = snap.id_digest(kind)?;
            let keyword = self.lexical.reconcile(vault, kind, &[])?.map(|c| c.digest);
            let behind = |mark: Option<Option<u64>>| mark.is_none_or(|w| w < head);
            out.push(IndexCheck {
                index: "keyword".into(),
                kind,
                matches: keyword.as_ref() == Some(&store),
                store,
                index_digest: keyword,
                watermark: keyword_mark.flatten(),
                behind: behind(keyword_mark),
            });
            // The vector index holds only what has an embedding, so a segment without one yet is lag, not a difference.
            let vector = self.vector.reconcile(vault, kind, &[])?.map(|c| c.digest);
            let embed_open = !snap.jobs(Some(crate::EMBED_JOB), Some(JobState::Queued), None, 1)?.is_empty()
                || !snap.jobs(Some(crate::EMBED_JOB), Some(JobState::Running), None, 1)?.is_empty();
            out.push(IndexCheck {
                index: "vector".into(),
                kind,
                matches: vector.as_ref() == Some(&store),
                store,
                index_digest: vector,
                watermark: vector_mark.flatten(),
                behind: behind(vector_mark) || embed_open,
            });
        }
        Ok(out)
    }

    /// Builds new index generations from the log and tables. The current ones keep serving until the new ones are ready.
    pub fn reindex(&self, caller: &Caller, vault: &VaultId, target: ReindexTarget) -> Result<(), ServerError> {
        self.allow(caller, Operation::VaultOperate(vault))?;
        self.vault_exists(vault)?;
        if matches!(target, ReindexTarget::Keyword | ReindexTarget::All) {
            self.rebuild_lexical(vault);
        }
        if matches!(target, ReindexTarget::Vector | ReindexTarget::All) {
            self.rebuild_vector(vault);
        }
        Ok(())
    }

    /// Removes blobs no row refers to that are older than the grace period. A route asks for it on one vault, and the schedule on every vault.
    pub fn sweep(&self, caller: &Caller, vault: &VaultId, grace_ms: i64) -> Result<SweepReport, ServerError> {
        self.allow(caller, Operation::VaultOperate(vault))?;
        self.vault_exists(vault)?;
        self.sweep_vault(vault, grace_ms)
    }

    /// Set once at startup, from settings.
    pub fn set_sweep(&self, sweep: crate::SweepSettings) {
        *self.sweep.lock().expect("lock") = sweep;
    }

    pub fn sweep_settings(&self) -> crate::SweepSettings {
        *self.sweep.lock().expect("lock")
    }

    /// The scheduled sweep, over every vault, as the server itself.
    pub fn sweep_all(&self, grace_ms: i64) -> Result<Vec<(VaultId, SweepReport)>, ServerError> {
        let mut out = vec![];
        for v in self.vault_ids()? {
            out.push((v.clone(), self.sweep_vault(&v, grace_ms)?));
        }
        Ok(out)
    }

    pub(super) fn sweep_vault(&self, vault: &VaultId, grace_ms: i64) -> Result<SweepReport, ServerError> {
        let referenced = {
            let snap = self.engine.store().snapshot(vault)?;
            crate::erasure::referenced_blobs(&snap)?
        };
        let now = self.now();
        let mut report = SweepReport::default();
        let mut candidates = vec![];
        let mut after: Option<BlobHash> = None;
        loop {
            let page = self.blobs.list(vault, after.as_ref(), crate::PAGE_LIMIT)?;
            let Some(last) = page.last() else { break };
            after = Some(last.hash.clone());
            for b in page {
                report.checked += 1;
                if referenced.contains(b.hash.as_str()) {
                    continue;
                }
                if crate::sweepable(&b, &HashSet::new(), now, grace_ms) {
                    candidates.push(b);
                } else {
                    report.kept_young += 1;
                }
            }
        }
        if candidates.is_empty() {
            return Ok(report);
        }
        // Read again just before deleting, so a blob a commit has just started referring to stays.
        let hashes: Vec<String> = candidates.iter().map(|b| b.hash.as_str().to_string()).collect();
        let gone: HashSet<String> = {
            let snap = self.engine.store().snapshot(vault)?;
            crate::erasure::unreferenced_blobs(&snap, &hashes)?.into_iter().collect()
        };
        for b in candidates {
            if !gone.contains(b.hash.as_str()) {
                continue;
            }
            match self.blobs.delete(vault, &b.hash) {
                Ok(()) => {
                    report.removed += 1;
                    report.removed_bytes += b.size;
                }
                Err(crate::BlobStoreError::NotFound { .. }) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(report)
    }

    /// Every vault's lag, queues and sizes, and the caches. For server admins, as it covers vaults they may hold no grant on, though it shows no content.
    pub fn status(&self, caller: &Caller) -> Result<ServerStatus, ServerError> {
        self.allow(caller, Operation::ServerStatus)?;
        let mut vaults = vec![];
        let mut after: Option<VaultId> = None;
        loop {
            let page = self.registry.vaults(None, after.as_ref(), super::RESULT_LIMIT)?;
            let Some(last) = page.last() else { break };
            after = Some(last.id.clone());
            for row in page {
                vaults.push(self.vault_status(&row.id, row.name)?);
            }
        }
        Ok(ServerStatus {
            vaults,
            caches: Caches::stats(&self.caches),
        })
    }

    fn vault_status(&self, vault: &VaultId, name: String) -> Result<VaultStatus, ServerError> {
        let snap = self.engine.store().snapshot(vault)?;
        let head = snap.head()?;
        let lag = |mark: Option<u64>| match (head, mark) {
            (Some(h), Some(m)) => h.saturating_sub(m),
            (Some(h), None) => h + 1,
            (None, _) => 0,
        };
        let keyword = match self.lexical.health(vault, false)? {
            IndexHealth::Ready { watermark } => (true, watermark),
            _ => (false, None),
        };
        let vector = match self.vector.health(vault)? {
            VectorHealth::Ready { watermark, .. } => (true, watermark),
            VectorHealth::Missing => (false, None),
        };
        let total = snap.open_job_count(&[])?;
        let mut open_jobs = vec![];
        let mut counted = 0;
        for kind in STATUS_KINDS {
            let n = total - snap.open_job_count(&[kind])?;
            counted += n;
            open_jobs.push((kind.to_string(), n));
        }
        open_jobs.push(("other".into(), total - counted));
        let mut dead_jobs = 0;
        let mut after: Option<ObjectId> = None;
        loop {
            let page = snap.jobs(None, Some(JobState::Dead), after.as_ref(), crate::PAGE_LIMIT)?;
            let Some(last) = page.last() else { break };
            after = Some(last.header.id.clone());
            dead_jobs += page.len() as u64;
        }
        let review_pending = snap.pending_count()?;
        drop(snap);
        let (mut blobs, mut blob_bytes) = (0, 0);
        let mut after: Option<BlobHash> = None;
        loop {
            let page = self.blobs.list(vault, after.as_ref(), crate::PAGE_LIMIT)?;
            let Some(last) = page.last() else { break };
            after = Some(last.hash.clone());
            blobs += page.len() as u64;
            blob_bytes += page.iter().map(|b| b.size).sum::<u64>();
        }
        Ok(VaultStatus {
            id: vault.clone(),
            name,
            head,
            keyword: IndexStatus {
                ready: keyword.0,
                watermark: keyword.1,
                lag: lag(keyword.1),
                building: self.projector.lock().expect("lock").building(vault),
            },
            vector: IndexStatus {
                ready: vector.0,
                watermark: vector.1,
                lag: lag(vector.1),
                building: self.vector_projector.lock().expect("lock").building(vault),
            },
            open_jobs,
            dead_jobs,
            review_pending,
            blobs,
            blob_bytes,
            cache_generation: self.engine.store().cache_marks(vault)?.generation,
        })
    }

    /// The status in Prometheus's text format, one gauge per number, labelled by vault id. Names stay out, as a label is easy to copy elsewhere.
    pub fn metrics(&self, caller: &Caller) -> Result<String, ServerError> {
        let status = self.status(caller)?;
        Ok(metrics_text(&status))
    }
}

/// Where a restore puts the vault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreTarget {
    /// Under the id it had, which must be free.
    Original,
    /// Under a new id.
    NewId,
    /// Under the id it had, deleting the vault that has it now. Only this overwrites anything.
    Replace,
}

impl<E: ServerEdges> Server<E> {
    /// Writes a backup of the vault. For the binary's `backup` command, which runs beside a serving server, as it needs no lease.
    pub fn backup_vault(
        &self,
        vault: &VaultId,
        out: &mut dyn std::io::Write,
    ) -> Result<(crate::backup::BackupHeader, crate::backup::BackupSummary), ServerError> {
        let row = self.registry.vault(vault)?.ok_or(ServerError::NotFound)?;
        crate::backup::export(self.engine.store(), &self.blobs, &self.vector, vault, &row.name, self.now(), out).map_err(backup_error)
    }

    /// Checks a backup whole, then restores it into a new, empty vault, which a failure removes again. `open` gives the archive from its start, as it's read twice. For the binary's `restore` command, as the server itself, so it takes no caller.
    pub fn restore_vault(
        &self,
        actor: &str,
        open: &mut dyn FnMut() -> std::io::Result<Box<dyn std::io::Read>>,
        target: RestoreTarget,
        name: Option<&str>,
    ) -> Result<(memex_types::VaultRow, crate::backup::BackupSummary), ServerError> {
        let (header, _) = crate::backup::verify(&mut open().map_err(|e| ServerError::Invalid(e.to_string()))?).map_err(backup_error)?;
        let id = match target {
            RestoreTarget::NewId => VaultId(self.new_id()),
            RestoreTarget::Original | RestoreTarget::Replace => header.vault_id.clone(),
        };
        if self.registry.vault(&id)?.is_some() {
            if target != RestoreTarget::Replace {
                return Err(ServerError::Conflict(format!(
                    "vault {id} already exists, and a restore overwrites a vault only when told to replace it"
                )));
            }
            let lease = self.lease(&id)?;
            self.registry.delete_vault(&id, &self.audit(actor, "vault.delete", &id.0))?;
            self.leases.lock().expect("lock").remove(&id);
            self.delete_vault_cascade(&id, &lease)?;
        }
        let row = memex_types::VaultRow {
            id: id.clone(),
            name: name.map_or_else(|| header.vault_name.clone(), String::from),
            created_by: actor.into(),
            created_at_ms: self.now(),
        };
        super::check_name(&row.name)?;
        self.registry.insert_vault(&row, &self.audit(actor, "vault.restore", &id.0))?;
        let restored = (|| {
            let lease = self.lease(&id)?;
            let mut read = open().map_err(|e| ServerError::Invalid(e.to_string()))?;
            crate::backup::import(self.engine.store(), &self.blobs, &self.vector, &lease, &mut read).map_err(backup_error)
        })();
        match restored {
            Ok(summary) => Ok((row, summary)),
            Err(e) => {
                // Half a vault helps nobody, and the backup is still there to try again from.
                if let Ok(lease) = self.lease(&id) {
                    let _ = self.registry.delete_vault(&id, &self.audit(actor, "vault.delete", &id.0));
                    self.leases.lock().expect("lock").remove(&id);
                    let _ = self.delete_vault_cascade(&id, &lease);
                }
                Err(e)
            }
        }
    }
}

pub(super) fn backup_error(e: crate::backup::BackupError) -> ServerError {
    match e {
        crate::backup::BackupError::Store(s) => s.into(),
        crate::backup::BackupError::NotEmpty(v) => ServerError::Conflict(format!("vault {v} isn't empty")),
        other => ServerError::Invalid(other.to_string()),
    }
}

/// Pure, so the format is tested without a server.
pub fn metrics_text(status: &ServerStatus) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let mut gauge = |name: &str, help: &str, rows: Vec<(String, f64)>| {
        let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} gauge");
        for (labels, value) in rows {
            let _ = writeln!(out, "{name}{{{labels}}} {value}");
        }
    };
    let by_vault = |f: &dyn Fn(&VaultStatus) -> f64| -> Vec<(String, f64)> { status.vaults.iter().map(|v| (format!("vault=\"{}\"", v.id.0), f(v))).collect() };
    gauge(
        "memex_log_head",
        "The last event's sequence, or -1 for an empty log.",
        by_vault(&|v| v.head.map_or(-1.0, |h| h as f64)),
    );
    gauge(
        "memex_keyword_lag_events",
        "Events the keyword index hasn't applied.",
        by_vault(&|v| v.keyword.lag as f64),
    );
    gauge(
        "memex_vector_lag_events",
        "Events the vector index hasn't applied.",
        by_vault(&|v| v.vector.lag as f64),
    );
    gauge("memex_dead_jobs", "Jobs that ran out of attempts.", by_vault(&|v| v.dead_jobs as f64));
    gauge(
        "memex_review_pending",
        "ChangeSets waiting for a decision.",
        by_vault(&|v| v.review_pending as f64),
    );
    gauge("memex_blobs", "Stored blobs.", by_vault(&|v| v.blobs as f64));
    gauge("memex_blob_bytes", "Bytes of stored blobs.", by_vault(&|v| v.blob_bytes as f64));
    let mut jobs = vec![];
    for v in &status.vaults {
        for (kind, n) in &v.open_jobs {
            jobs.push((format!("vault=\"{}\",kind=\"{kind}\"", v.id.0), *n as f64));
        }
    }
    gauge("memex_open_jobs", "Queued or running jobs.", jobs);
    let tiers = |f: &dyn Fn(&memex_types::CacheTierStats) -> f64| -> Vec<(String, f64)> {
        status.caches.iter().map(|t| (format!("tier=\"{}\"", t.tier), f(t))).collect()
    };
    gauge("memex_cache_bytes", "Bytes a cache tier holds.", tiers(&|t| t.size_bytes as f64));
    gauge("memex_cache_hits", "Hits since the server started.", tiers(&|t| t.hits as f64));
    gauge("memex_cache_misses", "Misses since the server started.", tiers(&|t| t.misses as f64));
    out
}
