//! Erasure on the facade (spec 030): the impact preview, the commit an approval makes, the physical cascade its job runs, and the record of what went.
use super::{Proposal, ProposalOutcome, Server, ServerEdges, ServerError};
use crate::auth::{Caller, Operation};
use crate::erasure::{self, ErasurePlan, Removed};
use crate::{BlobStore, ExtractJobError, LexicalIndex, Snapshot, Store, VectorIndex, WriterLease};
use memex_types::{
    BlobHash, ChangeSetRow, ChangeSetState, ERASE_JOB, EraseJobInput, ErasureRecord, Event, EventDraft, IndexHealth, JobRow, Mutation, ObjectId, Op, Row,
    SOURCE_ERASED, Table, VaultId, VectorHealth,
};

/// One erasure as the log recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErasureView {
    pub seq: u64,
    pub time_ms: i64,
    pub actor: String,
    pub record: ErasureRecord,
}

/// The physical part of a cascade, as a list, so a later kind of data is one more step. Erasure and whole-vault deletion run the same steps over a different scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CascadeStep {
    /// Chat threads the content reached, which live off the log (spec 049).
    ChatThreads,
    KeywordIndex,
    VectorIndex,
    Blobs,
    /// Every cache key of the vault, and the disk tier's files, which also resyncs live streams.
    Caches,
    /// Rewrites the store's files, so removed rows and old payloads leave the disk. For a whole vault, drops it.
    Store,
}

/// What an erase job runs, in order. Caches go before the store is compacted, so nothing refills them from rows that are going.
pub const ERASE_CASCADE: [CascadeStep; 6] = [
    CascadeStep::ChatThreads,
    CascadeStep::KeywordIndex,
    CascadeStep::VectorIndex,
    CascadeStep::Blobs,
    CascadeStep::Caches,
    CascadeStep::Store,
];

/// What deleting a vault runs. The store goes first, so a failure part way leaves files nobody can reach rather than rows pointing at missing files.
pub const VAULT_CASCADE: [CascadeStep; 6] = [
    CascadeStep::Store,
    CascadeStep::ChatThreads,
    CascadeStep::Blobs,
    CascadeStep::KeywordIndex,
    CascadeStep::VectorIndex,
    CascadeStep::Caches,
];

enum Scope<'a> {
    Erasure {
        record: &'a ErasureRecord,
        input: &'a EraseJobInput,
        lease: &'a WriterLease,
    },
    Vault {
        lease: &'a WriterLease,
    },
}

impl<E: ServerEdges> Server<E> {
    /// What erasing this document would remove, rewrite and flag, read from the vault as it is now. Ids only.
    pub fn erasure_preview(&self, caller: &Caller, vault: &VaultId, source: &ObjectId) -> Result<ErasureRecord, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        Ok(erasure::plan(&snap, source)?.ok_or(ServerError::NotFound)?.record)
    }

    /// Proposes erasing a document. It always waits for a person, whoever proposes it.
    pub fn propose_erasure(&self, caller: &Caller, vault: &VaultId, source: &ObjectId, rationale: String) -> Result<ProposalOutcome, ServerError> {
        let head = self.engine.store().cache_marks(vault).map(|m| m.head).ok().flatten();
        self.propose(
            caller,
            vault,
            Proposal {
                ops: vec![Op::EraseSource { id: source.clone() }],
                rationale,
                idempotency_key: format!("erase:{}:{}", source.0, head.map_or("none".to_string(), |h| h.to_string())),
            },
            false,
        )
    }

    pub(super) fn validate_erasure<S: Snapshot>(snap: &S, ops: &[Op]) -> Result<Option<memex_types::ValidatorResult>, ServerError> {
        let Some(id) = ops.iter().find_map(|op| match op {
            Op::EraseSource { id } => Some(id),
            _ => None,
        }) else {
            return Ok(None);
        };
        let exists = matches!(
            snap.get(&memex_types::RowKey {
                table: Table::Source,
                id: id.clone()
            })?,
            Some(Row::Source(_))
        );
        Ok(Some(memex_types::ValidatorResult {
            validator: "erasure".into(),
            passed: exists,
            detail: (!exists).then(|| format!("there's no source {id} to erase")),
        }))
    }

    /// An approved erasure: the decision, the rewritten log, the `source.erased` event and the job that does the physical work, in one transaction. Called with the vault's lock held.
    pub(super) fn commit_erasure(&self, lease: &WriterLease, actor: &str, mut row: ChangeSetRow) -> Result<ChangeSetRow, ServerError> {
        let Some(source) = row.ops.iter().find_map(|op| match op {
            Op::EraseSource { id } => Some(id.clone()),
            _ => None,
        }) else {
            return Err(ServerError::Invalid("this ChangeSet erases nothing".into()));
        };
        let vault = lease.vault.clone();
        let (plan, rewrites, head) = {
            let snap = self.engine.store().snapshot(&vault)?;
            let Some(mut plan) = erasure::plan(&snap, &source)? else {
                return Err(ServerError::Conflict(format!("source {source} is no longer there to erase")));
            };
            plan.record.changeset_id = row.header.id.clone();
            let rewrites = Self::rewrites_for(&snap, &Removed::of(&plan.record))?;
            (plan, rewrites, snap.head()?)
        };
        let ErasurePlan { record, blobs, embeddings } = plan;
        let first = head.map_or(0, |h| h + 1);
        row.state = ChangeSetState::Committed;
        row.commit_seq = Some(first);
        let job_id = ObjectId(self.new_id());
        let input = EraseJobInput {
            seq: first + 1,
            blobs,
            embeddings,
        };
        let now = self.now();
        let job = crate::jobs::new_job(
            job_id.clone(),
            actor,
            ERASE_JOB,
            serde_json::to_value(&input).map_err(|e| ServerError::Invalid(e.to_string()))?,
            format!("erase:{}", first + 1),
            ERASE_ATTEMPTS,
            now,
        );
        let changeset = serde_json::to_value(crate::ChangeSetRecorded {
            changeset: row.clone(),
            mutations: vec![],
        })
        .map_err(|e| ServerError::Invalid(e.to_string()))?;
        let drafts = vec![
            EventDraft {
                actor: actor.into(),
                kind: "changeset.recorded".into(),
                schema_version: 1,
                payload: changeset,
            },
            EventDraft {
                actor: actor.into(),
                kind: SOURCE_ERASED.into(),
                schema_version: 1,
                payload: erasure::erased_event_payload(&record),
            },
            EventDraft {
                actor: actor.into(),
                kind: "state.recorded".into(),
                schema_version: 1,
                payload: serde_json::to_value(crate::StateRecorded {
                    mutations: vec![Mutation::Put(Row::Job(job))],
                })
                .map_err(|e| ServerError::Invalid(e.to_string()))?,
            },
        ];
        self.engine.apply_rewriting(lease, &rewrites, drafts).map_err(|e| match e {
            crate::ApplyError::Store(s) => s.into(),
            other => ServerError::Invalid(other.to_string()),
        })?;
        // Live streams and caches stop showing what went at once. The job does it again once the files are gone.
        self.bump_cache_generation(&vault)?;
        Ok(row)
    }

    /// Every event whose payload changes, paged through the log.
    fn rewrites_for<S: Snapshot>(snap: &S, removed: &Removed) -> Result<Vec<(u64, serde_json::Value)>, ServerError> {
        let mut out = vec![];
        let mut after = None;
        loop {
            let page = snap.events(after)?;
            let Some(last) = page.last() else { return Ok(out) };
            after = Some(last.seq);
            for e in &page {
                if let Some(payload) = erasure::rewrite_event(e, removed).map_err(ServerError::Invalid)? {
                    out.push((e.seq, payload));
                }
            }
        }
    }

    /// Erasures in the order they happened, after the given sequence. What each removed, and the claims and pages it left flagged.
    pub fn erasures(&self, caller: &Caller, vault: &VaultId, after: Option<u64>, limit: usize) -> Result<Vec<ErasureView>, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        snap.events_of_kind(SOURCE_ERASED, after, limit)?.into_iter().map(Self::erasure_view).collect()
    }

    fn erasure_view(e: Event) -> Result<ErasureView, ServerError> {
        let record = serde_json::from_value(e.payload).map_err(|err| ServerError::Invalid(format!("event {}: {err}", e.seq)))?;
        Ok(ErasureView {
            seq: e.seq,
            time_ms: e.time_ms,
            actor: e.actor,
            record,
        })
    }

    /// The erase job: the physical cascade, after both index projectors have applied the erasure. Every step can run again.
    pub fn run_erase_job(&self, vault: &VaultId, job: &JobRow) -> Result<Vec<Mutation>, ExtractJobError> {
        let input: EraseJobInput = serde_json::from_value(job.input.clone()).map_err(|e| ExtractJobError::Fatal(e.to_string()))?;
        let event = {
            let snap = self.engine.store().snapshot(vault)?;
            snap.events(input.seq.checked_sub(1))?
                .into_iter()
                .next()
                .filter(|e| e.seq == input.seq && e.kind == SOURCE_ERASED)
        };
        let Some(event) = event else {
            return Err(ExtractJobError::Fatal(format!("event {} isn't an erasure", input.seq)));
        };
        let view = Self::erasure_view(event)?;
        if let Some(why) = self.indexes_behind(vault, input.seq)? {
            return Err(ExtractJobError::Wait(why));
        }
        let lease = self.lease(vault)?;
        let scope = Scope::Erasure {
            record: &view.record,
            input: &input,
            lease: &lease,
        };
        for step in ERASE_CASCADE {
            self.run_step(vault, step, &scope)?;
        }
        Ok(vec![])
    }

    /// Why the indexes can't be compacted yet: one hasn't applied the erasure, or is building a generation that may hold what went.
    fn indexes_behind(&self, vault: &VaultId, seq: u64) -> Result<Option<String>, ServerError> {
        if self.projector.lock().expect("lock").building(vault) || self.vector_projector.lock().expect("lock").building(vault) {
            return Ok(Some("an index is being rebuilt".into()));
        }
        if let IndexHealth::Ready { watermark } = self.lexical.health(vault, false)?
            && watermark < Some(seq)
        {
            return Ok(Some("the keyword index hasn't applied the erasure yet".into()));
        }
        if let VectorHealth::Ready { watermark, .. } = self.vector.health(vault)?
            && watermark < Some(seq)
        {
            return Ok(Some("the vector index hasn't applied the erasure yet".into()));
        }
        Ok(None)
    }

    fn run_step(&self, vault: &VaultId, step: CascadeStep, scope: &Scope<'_>) -> Result<(), ServerError> {
        match (step, scope) {
            (CascadeStep::ChatThreads, Scope::Erasure { record, .. }) => {
                for s in &record.sources {
                    self.erase_chat_threads_citing(vault, s)?;
                }
            }
            (CascadeStep::KeywordIndex, Scope::Erasure { record, .. }) => self.lexical.erase(vault, &index_ids(record))?,
            (CascadeStep::VectorIndex, Scope::Erasure { record, input, .. }) => {
                // A flagged claim or a reworked entity whose text came out the same still needs its embedding.
                let still_used = self.embedding_hashes_in_use(vault, record)?;
                let gone: Vec<String> = input.embeddings.iter().filter(|h| !still_used.contains(*h)).cloned().collect();
                self.vector.erase(vault, &index_ids(record), &gone)?;
            }
            (CascadeStep::Blobs, Scope::Erasure { input, .. }) => {
                let gone = {
                    let snap = self.engine.store().snapshot(vault)?;
                    erasure::unreferenced_blobs(&snap, &input.blobs)?
                };
                for hash in gone {
                    let Ok(hash) = BlobHash::parse(&hash) else { continue };
                    match self.blobs.delete(vault, &hash) {
                        Ok(()) | Err(crate::BlobStoreError::NotFound { .. }) => {}
                        Err(e) => return Err(e.into()),
                    }
                }
            }
            (CascadeStep::Caches, Scope::Erasure { .. }) => {
                self.bump_cache_generation(vault)?;
            }
            (CascadeStep::Store, Scope::Erasure { lease, .. }) => self.engine.store().compact(lease)?,
            (CascadeStep::Store, Scope::Vault { lease }) => self.engine.store().delete_vault(lease)?,
            // The schema went with the store, chat threads included.
            (CascadeStep::ChatThreads, Scope::Vault { .. }) => {}
            (CascadeStep::Blobs, Scope::Vault { .. }) => self.blobs.delete_vault(vault)?,
            (CascadeStep::KeywordIndex, Scope::Vault { .. }) => {
                self.projector.lock().expect("lock").forget(vault);
                self.lexical.delete_vault(vault)?;
            }
            (CascadeStep::VectorIndex, Scope::Vault { .. }) => {
                self.vector_projector.lock().expect("lock").forget(vault);
                self.vector.delete_vault(vault)?;
            }
            (CascadeStep::Caches, Scope::Vault { .. }) => crate::Caches::purge_vault(&self.caches, vault),
        }
        Ok(())
    }

    /// The content hashes of what the erasure left standing but changed: flagged claims and rewritten entities, as they read now.
    fn embedding_hashes_in_use(&self, vault: &VaultId, record: &ErasureRecord) -> Result<std::collections::HashSet<String>, ServerError> {
        let snap = self.engine.store().snapshot(vault)?;
        let mut ops = crate::lexical::assertion_ops(&snap, &record.flagged_claims)?;
        let entities: Vec<ObjectId> = record.rewritten_in(Table::Entity).cloned().collect();
        ops.extend(crate::entity::entity_ops(&snap, &entities)?);
        let docs = ops
            .into_iter()
            .filter_map(|op| match op {
                memex_types::IndexOp::Upsert(d) => Some(d),
                memex_types::IndexOp::Delete(_) => None,
            })
            .collect();
        Ok(crate::vector::embeddable(&snap, docs)?.into_iter().map(|d| d.hash).collect())
    }

    /// Whole-vault deletion's physical part, through the same steps as an erasure.
    pub(super) fn delete_vault_cascade(&self, vault: &VaultId, lease: &WriterLease) -> Result<(), ServerError> {
        let scope = Scope::Vault { lease };
        for step in VAULT_CASCADE {
            self.run_step(vault, step, &scope)?;
        }
        Ok(())
    }
}

/// An erase job may wait for the indexes many times, which costs no attempts. These are for real failures.
const ERASE_ATTEMPTS: u32 = 10;

/// What leaves the indexes: every document kind an erasure can remove.
fn index_ids(record: &ErasureRecord) -> Vec<ObjectId> {
    [Table::Segment, Table::Assertion, Table::Entity, Table::Page, Table::Block]
        .into_iter()
        .flat_map(|t| record.removed_in(t).cloned())
        .collect()
}
