//! What an operator does on a running server, over HTTP (spec 039): operations on a vault as jobs with progress and a report, the LLM usage report, and clearing the caches.
use super::{ExtractJobError, Server, ServerEdges, ServerError};
use crate::auth::{Caller, Operation};
use crate::{LexicalIndex, ModelPrice, Registry, Snapshot, Store, VectorIndex};
use memex_types::{
    AgentUsage, AuditRunRow, DerivationRow, IndexHealth, IndexName, JobProgress, JobRow, JobState, Mutation, OPERATION_JOB, ObjectId, OperationInput, Row,
    RowKey, Table, VaultId, VectorHealth,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::time::Duration;

/// An archive's size and its bytes.
pub type ArchiveRead = (u64, Box<dyn Read + Send>);

/// Where export archives are kept and imports wait, by name. Writing is atomic: a name is only ever a whole archive.
pub trait ArchiveStore: Send + Sync {
    /// Writes an archive through `write`, durable and visible under `name` only once it returns. Gives the bytes written.
    fn put(&self, name: &str, write: &mut dyn FnMut(&mut dyn Write) -> Result<(), ServerError>) -> Result<u64, ServerError>;
    /// The archive's size and its bytes, or None when there's no such archive.
    fn open(&self, name: &str) -> Result<Option<ArchiveRead>, ServerError>;
    /// Removes an archive. One that isn't there is fine.
    fn remove(&self, name: &str) -> Result<(), ServerError>;
}

/// What an operation job needs from the worker running it.
pub trait OperationEdge {
    /// Waits a little, for work another thread does.
    fn pause(&self, d: Duration);
    /// The worker no longer holds the job, so stop.
    fn lost(&self) -> bool;
    fn archives(&self) -> &dyn ArchiveStore;
}

/// The archive an export job writes.
pub fn export_archive_name(vault: &VaultId, job: &ObjectId) -> String {
    format!("exports/{}/{}.memex", vault.0, job.0)
}

/// How often a reindex job looks to see whether its build is done.
const BUILD_POLL: Duration = Duration::from_millis(250);

/// An operation that failed is reported, and a person retries it through the jobs API. The second attempt covers a server that stopped under it.
const OPERATION_ATTEMPTS: u32 = 2;

/// Two operations that would get in each other's way: a rollback during a reindex of the same index, or two of the same thing.
fn clash_key(input: &OperationInput) -> String {
    match input {
        OperationInput::Reindex { index } | OperationInput::Rollback { index } => format!("index:{}", index.as_str()),
        other => other.name().to_string(),
    }
}

fn describe(input: &OperationInput) -> String {
    match input {
        OperationInput::Reindex { index } => format!("a {} reindex", index.as_str()),
        OperationInput::Rollback { index } => format!("a {} rollback", index.as_str()),
        other => format!("a {}", other.name()),
    }
}

/// How long a server admin's last check stands in for one Postgres can't do.
pub const RECENT_ADMIN_MS: i64 = 5 * 60 * 1000;
/// The most credentials remembered. Past this the oldest go.
const RECENT_ADMIN_LIMIT: usize = 256;

/// Server admins this process has authenticated lately, by a hash of the credential, so the status route can still answer one while Postgres is down (spec 039). Only that route asks, and only when the registry couldn't be reached, never when it answered.
#[derive(Default)]
pub struct RecentAdmins {
    seen: std::sync::Mutex<std::collections::HashMap<String, (Caller, i64)>>,
}

impl RecentAdmins {
    /// Keeps a server admin's credential. Anyone else is ignored.
    pub fn remember(&self, credential: &str, caller: &Caller, now_ms: i64) {
        if !caller.principal.server_admin {
            return;
        }
        let mut seen = self.seen.lock().expect("lock");
        seen.retain(|_, (_, at)| now_ms - *at < RECENT_ADMIN_MS);
        if seen.len() >= RECENT_ADMIN_LIMIT
            && let Some(oldest) = seen.iter().min_by_key(|(_, (_, at))| *at).map(|(k, _)| k.clone())
        {
            seen.remove(&oldest);
        }
        seen.insert(super::sha256_hex(credential), (caller.clone(), now_ms));
    }

    /// The caller this credential was within `RECENT_ADMIN_MS`, if it was a server admin's.
    pub fn recall(&self, credential: &str, now_ms: i64) -> Option<Caller> {
        let seen = self.seen.lock().expect("lock");
        seen.get(&super::sha256_hex(credential))
            .filter(|(_, at)| now_ms - *at < RECENT_ADMIN_MS && now_ms >= *at)
            .map(|(c, _)| c.clone())
    }
}

/// Tokens and cost for one agent, model, vault and day.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRow {
    /// The Derivation's kind, `extraction`, `chat` and so on, or `audit` for audit runs.
    pub agent: String,
    pub model: String,
    pub vault: VaultId,
    /// `YYYY-MM-DD`, in UTC.
    pub day: String,
    /// Derivations, or for `audit` the runs, summed into this row.
    pub derivations: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub thinking_tokens: u64,
    /// Only when the model has a price set. Never guessed.
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageReport {
    pub rows: Vec<UsageRow>,
    /// The sum over rows with a price.
    pub priced_cost_usd: f64,
    /// Tokens in rows whose model has no price, so the cost above leaves them out.
    pub unpriced_tokens: u64,
}

/// `YYYY-MM-DD` in UTC for a time in milliseconds since the epoch.
pub fn utc_day(ms: i64) -> String {
    // Howard Hinnant's days-to-civil.
    let z = ms.div_euclid(86_400_000) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// The start of a `YYYY-MM-DD` day in UTC, in milliseconds since the epoch.
pub fn parse_utc_day(text: &str) -> Option<i64> {
    let mut parts = text.splitn(3, '-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    if text.len() != 10 || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Howard Hinnant's days-from-civil.
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let ms = (era * 146_097 + doe - 719_468) * 86_400_000;
    // A day past the month's end comes back as another date.
    (utc_day(ms) == text).then_some(ms)
}

/// One record of tokens spent: a Derivation, or an audit run, which keeps its spending on the run rather than in Derivations.
#[derive(Debug, Clone, Copy)]
pub struct Spent<'a> {
    pub vault: &'a VaultId,
    pub agent: &'a str,
    pub model: &'a str,
    pub at_ms: i64,
    pub usage: &'a AgentUsage,
}

impl<'a> Spent<'a> {
    pub fn derivation(vault: &'a VaultId, d: &'a DerivationRow) -> Self {
        Self {
            vault,
            agent: &d.kind,
            model: &d.model,
            at_ms: d.created_at_ms,
            usage: &d.usage,
        }
    }

    /// An audit run counts under `audit`, its model and the day it was created. A run that called no model spent nothing, and is left out.
    pub fn audit_run(vault: &'a VaultId, r: &'a AuditRunRow) -> Option<Self> {
        let model = r.model.as_deref()?;
        (r.llm_calls > 0 || r.usage.total() > 0).then_some(Self {
            vault,
            agent: crate::audit::AUDIT_JOB,
            model,
            at_ms: r.created_at_ms,
            usage: &r.usage,
        })
    }
}

/// Sums spending by agent, model, vault and day, in that order of keys, within `[from_ms, to_ms)`. Pure, so the report is tested without a server.
pub fn usage_report<'a>(spent: impl IntoIterator<Item = Spent<'a>>, from_ms: i64, to_ms: i64, prices: &[ModelPrice]) -> UsageReport {
    let mut sums: BTreeMap<(String, String, String, String), UsageRow> = BTreeMap::new();
    for d in spent {
        if d.at_ms < from_ms || d.at_ms >= to_ms {
            continue;
        }
        let day = utc_day(d.at_ms);
        let row = sums
            .entry((d.agent.to_owned(), d.model.to_owned(), d.vault.0.clone(), day.clone()))
            .or_insert_with(|| UsageRow {
                agent: d.agent.to_owned(),
                model: d.model.to_owned(),
                vault: d.vault.clone(),
                day,
                derivations: 0,
                input_tokens: 0,
                output_tokens: 0,
                thinking_tokens: 0,
                cost_usd: None,
            });
        row.derivations += 1;
        row.input_tokens += d.usage.input_tokens;
        row.output_tokens += d.usage.output_tokens;
        row.thinking_tokens += d.usage.thinking_tokens;
    }
    let mut report = UsageReport {
        rows: vec![],
        priced_cost_usd: 0.0,
        unpriced_tokens: 0,
    };
    for (_, mut row) in sums {
        match prices.iter().find(|p| p.model == row.model) {
            Some(p) => {
                let cost = (row.input_tokens as f64 * p.input + row.output_tokens as f64 * p.output + row.thinking_tokens as f64 * p.thinking) / 1_000_000.0;
                report.priced_cost_usd += cost;
                row.cost_usd = Some(cost);
            }
            None => report.unpriced_tokens += row.input_tokens + row.output_tokens + row.thinking_tokens,
        }
        report.rows.push(row);
    }
    report
}

/// The reconcile report as JSON, in the same fields as the reconcile route's answer.
pub fn reconcile_json(r: &crate::ReconcileReport) -> Value {
    let key = |k: &RowKey| json!({"table": serde_json::to_value(k.table).unwrap_or(Value::Null), "id": k.id.0});
    json!({
        "clean": r.clean(),
        "head": r.log.head,
        "events": r.log.events,
        "rows": r.log.rows,
        "tables_sha256": r.log.tables_sha256,
        "replayed_sha256": r.log.replayed_sha256,
        "at_seq": r.log.at.as_ref().map(|(s, _)| *s),
        "at_sha256": r.log.at.as_ref().map(|(_, h)| h.clone()),
        "replay_error": r.log.replay_error,
        "differing": r.log.differing.iter().map(key).collect::<Vec<_>>(),
        "differing_count": r.log.differing_count,
        "blobs_referenced": r.blobs.referenced,
        "blobs_stored": r.blobs.stored,
        "blob_bytes": r.blobs.stored_bytes,
        "blobs_missing": r.blobs.missing,
        "blob_orphans": r.blobs.orphans,
        "blobs_corrupt": r.blobs.corrupt,
        "indexes": r.indexes.iter().map(|c| json!({
            "index": c.index,
            "kind": c.kind.as_str(),
            "store": {"count": c.store.count, "hash": format!("{:016x}", c.store.xor)},
            "index_digest": c.index_digest.as_ref().map(|d| json!({"count": d.count, "hash": format!("{:016x}", d.xor)})),
            "watermark": c.watermark,
            "behind": c.behind,
            "matches": c.matches,
        })).collect::<Vec<_>>(),
        "repaired": r.repaired,
    })
}

impl<E: ServerEdges> Server<E> {
    /// Queues an operation on a vault, for a vault admin. One of the same kind that's still queued or running is refused, naming it, as is a rollback during a reindex of that index.
    pub fn start_operation(&self, caller: &Caller, vault: &VaultId, input: OperationInput) -> Result<JobRow, ServerError> {
        self.allow(caller, Operation::VaultOperate(vault))?;
        self.vault_exists(vault)?;
        let lease = self.lease(vault)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let key = clash_key(&input);
        {
            let snap = self.engine.store().snapshot(vault)?;
            for state in [JobState::Queued, JobState::Running] {
                let mut after: Option<ObjectId> = None;
                loop {
                    let page = snap.jobs(Some(OPERATION_JOB), Some(state), after.as_ref(), crate::PAGE_LIMIT)?;
                    let Some(last) = page.last() else { break };
                    after = Some(last.header.id.clone());
                    for job in &page {
                        let Ok(open) = serde_json::from_value::<OperationInput>(job.input.clone()) else {
                            continue;
                        };
                        if clash_key(&open) == key {
                            return Err(ServerError::Conflict(format!(
                                "{} is already {} in this vault, as job {}. Wait for it to finish, or cancel it first",
                                describe(&open),
                                if state == JobState::Queued { "queued" } else { "running" },
                                job.header.id
                            )));
                        }
                    }
                }
            }
        }
        let id = ObjectId(self.new_id());
        let job = crate::jobs::new_job(
            id.clone(),
            &caller.principal.id,
            OPERATION_JOB,
            serde_json::to_value(&input).map_err(|e| ServerError::Invalid(e.to_string()))?,
            format!("operation:{id}"),
            OPERATION_ATTEMPTS,
            self.now(),
        );
        self.state_event(&lease, &caller.principal.id, vec![Mutation::Put(Row::Job(job.clone()))])?;
        Ok(job)
    }

    /// A vault's operations, oldest first.
    pub fn operations(&self, caller: &Caller, vault: &VaultId, after: Option<&ObjectId>, limit: usize) -> Result<Vec<JobRow>, ServerError> {
        self.allow(caller, Operation::VaultOperate(vault))?;
        self.vault_exists(vault)?;
        Ok(self.engine.store().snapshot(vault)?.jobs(Some(OPERATION_JOB), None, after, limit)?)
    }

    /// One operation, with its progress and, once done, its report.
    pub fn operation(&self, caller: &Caller, vault: &VaultId, id: &ObjectId) -> Result<JobRow, ServerError> {
        self.allow(caller, Operation::VaultOperate(vault))?;
        self.vault_exists(vault)?;
        let job = self.job(vault, id)?;
        if job.kind != OPERATION_JOB {
            return Err(ServerError::NotFound);
        }
        Ok(job)
    }

    /// The archive a finished export wrote, with its size.
    pub fn operation_archive(&self, caller: &Caller, vault: &VaultId, id: &ObjectId, archives: &dyn ArchiveStore) -> Result<ArchiveRead, ServerError> {
        let job = self.operation(caller, vault, id)?;
        if !matches!(serde_json::from_value(job.input.clone()), Ok(OperationInput::Export)) {
            return Err(ServerError::Invalid("only an export has an archive".into()));
        }
        if job.state != JobState::Done {
            return Err(ServerError::Conflict("the export hasn't finished".into()));
        }
        archives.open(&export_archive_name(vault, &job.header.id))?.ok_or(ServerError::NotFound)
    }

    /// Writes how far a held job has got, and what it found. `Lost` when the worker no longer holds it.
    fn note_job(&self, vault: &VaultId, held: &JobRow, progress: JobProgress, report: Option<Value>) -> Result<(), ExtractJobError> {
        let lease = self.lease(vault)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let mut current = self.job(vault, &held.header.id)?;
        if !super::still_held(&current, held) {
            return Err(ExtractJobError::Lost);
        }
        let owner = held.lease_owner.clone().unwrap_or_default();
        current.progress = Some(progress);
        if report.is_some() {
            current.report = report;
        }
        current.header.version += 1;
        self.state_event(&lease, &owner, vec![Mutation::Put(Row::Job(current))])?;
        Ok(())
    }

    /// Runs an operation job, writing each step as progress and the report at the end, so the job's completion carries both.
    pub fn run_operation_job(&self, vault: &VaultId, job: &JobRow, edge: &dyn OperationEdge) -> Result<(), ExtractJobError> {
        let input: OperationInput = serde_json::from_value(job.input.clone()).map_err(|e| ExtractJobError::Fatal(e.to_string()))?;
        let step = |n: u32, of: u32, label: &str, report: Option<Value>| {
            self.note_job(
                vault,
                job,
                JobProgress {
                    step: n,
                    of,
                    label: label.into(),
                },
                report,
            )
        };
        match input {
            OperationInput::Reindex { index } => {
                step(1, 2, &format!("building a new {} generation", index.as_str()), None)?;
                match index {
                    IndexName::Keyword => self.rebuild_lexical(vault),
                    IndexName::Vector => self.rebuild_vector(vault),
                }
                while self.index_building(vault, index) {
                    if edge.lost() {
                        return Err(ExtractJobError::Lost);
                    }
                    edge.pause(BUILD_POLL);
                }
                let report = json!({"index": index.as_str(), "live_watermark": self.index_watermark(vault, index)?});
                step(2, 2, "the new generation is live", Some(report))?;
            }
            OperationInput::Rollback { index } => {
                step(1, 2, &format!("going back to the previous {} generation", index.as_str()), None)?;
                self.rollback_index(vault, index)?;
                let report = json!({"index": index.as_str(), "live_watermark": self.index_watermark(vault, index)?});
                step(2, 2, "the previous generation is live, and catches up with the log", Some(report))?;
            }
            OperationInput::Reconcile { at, deep, repair } => {
                step(1, 2, "replaying the log and comparing", None)?;
                let report = self.reconcile_report(vault, at, deep, repair)?;
                step(2, 2, "compared", Some(reconcile_json(&report)))?;
            }
            OperationInput::Export => {
                step(1, 2, "writing the archive", None)?;
                let name = export_archive_name(vault, &job.header.id);
                let mut out = None;
                let bytes = edge.archives().put(&name, &mut |w| {
                    out = Some(self.backup_vault(vault, w)?);
                    Ok(())
                })?;
                let (header, summary) = out.ok_or_else(|| ExtractJobError::Fatal("the archive wasn't written".into()))?;
                let report = json!({
                    "bytes": bytes,
                    "head": header.head,
                    "dump_sha256": header.dump_sha256,
                    "events": summary.events,
                    "blobs": summary.blobs,
                    "blob_bytes": summary.blob_bytes,
                    "embeddings": summary.embeddings,
                    "sha256": summary.sha256,
                });
                step(2, 2, "written, ready to download", Some(report))?;
            }
            OperationInput::Sweep { grace_s } => {
                step(1, 2, "looking for unreferenced blobs", None)?;
                let r = self.sweep_vault(vault, grace_s.saturating_mul(1000).min(i64::MAX as u64) as i64)?;
                let report = json!({"checked": r.checked, "removed": r.removed, "removed_bytes": r.removed_bytes, "kept_young": r.kept_young});
                step(2, 2, "swept", Some(report))?;
            }
            OperationInput::Compact => {
                step(1, 2, "rewriting the vault's tables", None)?;
                let lease = self.lease(vault)?;
                self.engine.store().compact(&lease)?;
                step(2, 2, "compacted", Some(json!({"compacted": true})))?;
            }
        }
        Ok(())
    }

    fn index_building(&self, vault: &VaultId, index: IndexName) -> bool {
        match index {
            IndexName::Keyword => self.projector.lock().expect("lock").building(vault),
            IndexName::Vector => self.vector_projector.lock().expect("lock").building(vault),
        }
    }

    fn index_watermark(&self, vault: &VaultId, index: IndexName) -> Result<Option<u64>, ServerError> {
        Ok(match index {
            IndexName::Keyword => match self.lexical.health(vault, false)? {
                IndexHealth::Ready { watermark } => watermark,
                _ => None,
            },
            IndexName::Vector => match self.vector.health(vault)? {
                VectorHealth::Ready { watermark, .. } => watermark,
                VectorHealth::Missing => None,
            },
        })
    }

    /// Makes the generation before the live one live again. The projector forgets where it was, so it reads the old generation's watermark and catches up from there.
    fn rollback_index(&self, vault: &VaultId, index: IndexName) -> Result<(), ExtractJobError> {
        let busy = || {
            ExtractJobError::Fatal(format!(
                "a {} build is running, so there's no settled generation to go back from",
                index.as_str()
            ))
        };
        match index {
            IndexName::Keyword => {
                let mut projector = self.projector.lock().expect("lock");
                if projector.building(vault) {
                    return Err(busy());
                }
                self.lexical.rollback(vault).map_err(|e| ExtractJobError::Fatal(e.to_string()))?;
                projector.forget(vault);
            }
            IndexName::Vector => {
                let mut projector = self.vector_projector.lock().expect("lock");
                if projector.building(vault) {
                    return Err(busy());
                }
                self.vector.rollback(vault).map_err(|e| ExtractJobError::Fatal(e.to_string()))?;
                projector.forget(vault);
            }
        }
        Ok(())
    }

    /// Whether the caller may do this, for routes whose work isn't a call on the server, such as reading settings.
    pub fn authorize_op(&self, caller: &Caller, op: Operation<'_>) -> Result<(), ServerError> {
        self.allow(caller, op)
    }

    /// Where an archive would restore to, refusing at once what the restore itself would refuse later: an existing vault without `Replace`. Reads only the header.
    pub fn check_restore(&self, read: &mut dyn Read, target: &crate::RestoreTarget) -> Result<VaultId, ServerError> {
        let header = crate::backup::read_header(read).map_err(super::ops::backup_error)?;
        if *target == crate::RestoreTarget::Original && self.registry.vault(&header.vault_id)?.is_some() {
            return Err(ServerError::Conflict(format!(
                "vault {} already exists, and a restore overwrites a vault only when told to replace it",
                header.vault_id
            )));
        }
        Ok(header.vault_id)
    }

    /// Drops every vault's cached entries, in RAM and on disk. The counters stay.
    pub fn clear_caches(&self, caller: &Caller) -> Result<usize, ServerError> {
        self.allow(caller, Operation::ServerStatus)?;
        let vaults = self.vault_ids()?;
        for v in &vaults {
            crate::Caches::purge_vault(&self.caches, v);
        }
        Ok(vaults.len())
    }

    /// LLM tokens by agent, model, vault and UTC day, summed from Derivations made in `[from_ms, to_ms)`, with cost only for a model that has a price. For server admins.
    pub fn usage(&self, caller: &Caller, from_ms: i64, to_ms: i64, prices: &[ModelPrice]) -> Result<UsageReport, ServerError> {
        self.allow(caller, Operation::ServerStatus)?;
        let mut all: Vec<(VaultId, Row)> = vec![];
        for v in self.vault_ids()? {
            let snap = self.engine.store().snapshot(&v)?;
            // Audit runs keep their tokens on the run rather than in Derivations (spec 047).
            for table in [Table::Derivation, Table::AuditRun] {
                let mut after: Option<ObjectId> = None;
                loop {
                    let page = snap.rows(table, after.as_ref())?;
                    let Some(last) = page.last() else { break };
                    after = Some(last.header().id.clone());
                    all.extend(page.into_iter().map(|r| (v.clone(), r)));
                }
            }
        }
        let spent = all.iter().filter_map(|(v, r)| match r {
            Row::Derivation(d) => Some(Spent::derivation(v, d)),
            Row::AuditRun(a) => Spent::audit_run(v, a),
            _ => None,
        });
        Ok(usage_report(spent, from_ms, to_ms, prices))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memex_types::{AgentUsage, DerivationOutcome, Header, Status};

    fn derivation(kind: &str, model: &str, at: i64, i: u64, o: u64, t: u64) -> DerivationRow {
        DerivationRow {
            header: Header {
                id: ObjectId(format!("d{at}{i}")),
                creator: "x".into(),
                status: Status::Active,
                version: 1,
                acl_id: None,
            },
            kind: kind.into(),
            job_id: ObjectId("j".into()),
            source_id: ObjectId("s".into()),
            rendition_id: ObjectId("r".into()),
            segment_ids: vec![],
            model: model.into(),
            prompt_hash: String::new(),
            usage: AgentUsage {
                input_tokens: i,
                output_tokens: o,
                thinking_tokens: t,
            },
            attempts: 1,
            outcome: DerivationOutcome::Done,
            detail: None,
            items: vec![],
            rejected: vec![],
            created_at_ms: at,
        }
    }

    #[test]
    fn days_parse_back_and_bad_ones_are_refused() {
        for ms in [0, 951_782_400_000, 1_699_920_000_000, -86_400_000] {
            assert_eq!(parse_utc_day(&utc_day(ms)), Some(ms));
        }
        for bad in ["2023-02-30", "2023-13-01", "2023-1-01", "yesterday", "2023-11-14T00"] {
            assert_eq!(parse_utc_day(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_recent_admin_is_recalled_for_a_while_and_no_one_else_is() {
        use memex_types::{Principal, PrincipalKind};
        let caller = |admin: bool| Caller {
            principal: Principal {
                id: "p".into(),
                name: "p".into(),
                kind: PrincipalKind::Human,
                server_admin: admin,
                disabled: false,
                created_by: "x".into(),
                created_at_ms: 0,
            },
            token_id: "t".into(),
            grants: vec![],
            via_session: false,
        };
        let recent = RecentAdmins::default();
        recent.remember("mx_admin", &caller(true), 1_000);
        recent.remember("mx_other", &caller(false), 1_000);
        assert!(recent.recall("mx_admin", 1_000 + RECENT_ADMIN_MS - 1).is_some());
        assert!(recent.recall("mx_admin", 1_000 + RECENT_ADMIN_MS).is_none(), "too long ago");
        assert!(recent.recall("mx_other", 1_001).is_none(), "only server admins are kept");
        assert!(recent.recall("mx_admin2", 1_001).is_none());
    }

    #[test]
    fn days_are_utc_dates() {
        assert_eq!(utc_day(0), "1970-01-01");
        assert_eq!(utc_day(86_399_999), "1970-01-01");
        assert_eq!(utc_day(951_782_400_000), "2000-02-29");
        assert_eq!(utc_day(1_700_000_000_000), "2023-11-14");
        assert_eq!(utc_day(-1), "1969-12-31");
    }

    #[test]
    fn usage_sums_by_key_and_prices_only_what_has_a_price() {
        let (a, b) = (VaultId("a".into()), VaultId("b".into()));
        let rows = [
            (a.clone(), derivation("extraction", "m1", 1_700_000_000_000, 100, 10, 1)),
            (a.clone(), derivation("extraction", "m1", 1_700_000_100_000, 50, 5, 0)),
            (b.clone(), derivation("extraction", "m1", 1_700_000_000_000, 7, 0, 0)),
            (a.clone(), derivation("chat", "m2", 1_700_000_000_000, 1000, 0, 0)),
            (a.clone(), derivation("extraction", "m1", 1_700_100_000_000, 1, 1, 1)),
        ];
        let none = usage_report(rows.iter().map(|(v, d)| Spent::derivation(v, d)), 0, i64::MAX, &[]);
        assert!(none.rows.iter().all(|r| r.cost_usd.is_none()), "no price means no cost");
        assert_eq!(none.priced_cost_usd, 0.0);
        assert_eq!(none.unpriced_tokens, 100 + 10 + 1 + 50 + 5 + 7 + 1000 + 3);
        assert_eq!(none.rows.len(), 4);
        let first = none
            .rows
            .iter()
            .find(|r| r.agent == "extraction" && r.vault == a && r.day == "2023-11-14")
            .unwrap();
        assert_eq!(
            (first.derivations, first.input_tokens, first.output_tokens, first.thinking_tokens),
            (2, 150, 15, 1)
        );
        let prices = [ModelPrice {
            model: "m1".into(),
            input: 1.0,
            output: 2.0,
            thinking: 4.0,
        }];
        let priced = usage_report(rows.iter().map(|(v, d)| Spent::derivation(v, d)), 0, 1_700_050_000_000, &prices);
        assert_eq!(priced.rows.len(), 3, "the later day is outside the window");
        let first = priced.rows.iter().find(|r| r.agent == "extraction" && r.vault == a).unwrap();
        assert_eq!(first.cost_usd, Some((150.0 + 30.0 + 4.0) / 1e6));
        assert!(priced.rows.iter().find(|r| r.model == "m2").unwrap().cost_usd.is_none());
        assert_eq!(priced.unpriced_tokens, 1000);
    }

    fn audit_run(model: Option<&str>, at: i64, calls: u32, i: u64, o: u64) -> AuditRunRow {
        AuditRunRow {
            header: Header {
                id: ObjectId(format!("r{at}{i}")),
                creator: "x".into(),
                status: Status::Active,
                version: 1,
                acl_id: None,
            },
            seed: 1,
            trigger: memex_types::AuditTrigger::OnDemand,
            requested_by: "x".into(),
            state: memex_types::AuditRunState::Finished,
            stop: None,
            stop_detail: None,
            sample_size: 1,
            population: 1,
            sampled: 1,
            state_seq: None,
            run_token_budget: 0,
            usage: AgentUsage {
                input_tokens: i,
                output_tokens: o,
                thinking_tokens: 0,
            },
            model: model.map(Into::into),
            llm_calls: calls,
            passed: 0,
            suspect: 0,
            failed: 0,
            llm_skipped: 0,
            revoked: vec![],
            siblings_requeued: 0,
            siblings_remaining: 0,
            missed_slots: 0,
            vault_findings: vec![],
            job_id: ObjectId("j".into()),
            created_at_ms: at,
            started_at_ms: None,
            finished_at_ms: None,
        }
    }

    #[test]
    fn audit_runs_count_as_the_audit_agent_by_their_day_and_model_and_runs_with_no_model_are_left_out() {
        let a = VaultId("a".into());
        let t0 = 1_700_000_000_000;
        let runs = [
            audit_run(Some("m1"), t0, 2, 100, 10),
            audit_run(Some("m1"), t0 + 1000, 1, 50, 5),
            audit_run(Some("m3"), t0 + 86_400_000, 1, 7, 1),
            audit_run(None, t0, 0, 0, 0),
            audit_run(Some("m1"), t0, 0, 0, 0),
        ];
        let d = derivation("extraction", "m1", t0, 1, 1, 0);
        let spent = runs.iter().filter_map(|r| Spent::audit_run(&a, r)).chain([Spent::derivation(&a, &d)]);
        let prices = [ModelPrice {
            model: "m1".into(),
            input: 1.0,
            output: 2.0,
            thinking: 4.0,
        }];
        let report = usage_report(spent, 0, i64::MAX, &prices);
        assert_eq!(report.rows.len(), 3, "{report:?}");
        let audit = report.rows.iter().find(|r| r.agent == "audit" && r.model == "m1").unwrap();
        assert_eq!(
            (audit.derivations, audit.input_tokens, audit.output_tokens),
            (2, 150, 15),
            "a run with no call isn't counted"
        );
        assert_eq!(audit.cost_usd, Some((150.0 + 30.0) / 1e6));
        assert_eq!(audit.day, "2023-11-14");
        let unpriced = report.rows.iter().find(|r| r.model == "m3").unwrap();
        assert_eq!(
            (unpriced.agent.as_str(), unpriced.day.as_str(), unpriced.cost_usd),
            ("audit", "2023-11-15", None)
        );
        assert_eq!(report.unpriced_tokens, 8);
        assert!(report.rows.iter().any(|r| r.agent == "extraction"));
    }

    #[test]
    fn a_reindex_clashes_with_its_own_index_only() {
        let k = OperationInput::Reindex { index: IndexName::Keyword };
        assert_eq!(clash_key(&k), clash_key(&OperationInput::Rollback { index: IndexName::Keyword }));
        assert_ne!(clash_key(&k), clash_key(&OperationInput::Reindex { index: IndexName::Vector }));
        assert_ne!(clash_key(&OperationInput::Export), clash_key(&OperationInput::Compact));
    }
}
