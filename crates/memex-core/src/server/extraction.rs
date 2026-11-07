//! Extraction on the facade: a job per rendition sends batches of segments to the LLM, and what comes back becomes proposals with a Derivation behind them. Everything goes through `propose`, so the review queue applies as it does to anyone else.
use super::{FieldInput, Proposal, ProposedBy, RESULT_LIMIT, Server, ServerEdges, ServerError};
use crate::auth::{Caller, Operation};
use crate::entity;
use crate::extraction::{self, DERIVATION_KIND, EXTRACT_JOB, ExtractionLimits, ExtractionPrompt, ExtractionState};
use crate::{AgentRunner, PAGE_LIMIT, Seed, Snapshot, Store, jobs, run_with_retry};
use memex_types::{
    AgentError, AgentRequest, AgentUsage, ChangeSetState, DerivationOutcome, DerivationRow, EntityTypeRow, EvidenceInput, ExtractedItem, ExtractionRow,
    ExtractionStop, Header, IdentityCandidate, JobRow, JobState, Mutation, ObjectId, Op, RejectedItem, RenditionRow, Row, RowKey, SegmentRow, Stance, Status,
    Table, VaultId,
};
use serde_json::{Value, json};
use std::collections::HashSet;

/// How a job's proposals are attributed. Always a machine, so they go to review until auto-commit is earned.
pub(super) const EXTRACTOR: &str = "extractor";
const EXTRACT_ATTEMPTS: u32 = 5;
/// How many existing entities a mention names as candidates.
const MAX_CANDIDATES: usize = 5;
/// How long past a call's own limits a worker keeps the job.
pub(super) const LEASE_MARGIN_MS: i64 = 30_000;

/// Why a run didn't complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractJobError {
    /// The LLM is away. The job goes back to wait, and uses up no attempts.
    Wait(String),
    /// Worth another attempt, later.
    Retry(String),
    /// Trying again won't help unless someone changes something.
    Fatal(String),
    /// Another worker has the job, or it was cancelled. Nothing more was written.
    Lost,
}

impl From<ServerError> for ExtractJobError {
    fn from(e: ServerError) -> Self {
        match e {
            ServerError::Unavailable(m) => Self::Retry(m),
            other => Self::Fatal(other.to_string()),
        }
    }
}

impl From<crate::StoreError> for ExtractJobError {
    fn from(e: crate::StoreError) -> Self {
        Self::Retry(e.to_string())
    }
}

/// A document's extraction as a caller sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionReport {
    pub source_id: ObjectId,
    pub rendition_id: Option<ObjectId>,
    pub state: ExtractionState,
    pub job_id: Option<ObjectId>,
    pub row: Option<ExtractionRow>,
    pub waiting_since_ms: Option<i64>,
    pub last_error: Option<String>,
}

fn header(id: ObjectId, creator: &str) -> Header {
    Header {
        id,
        creator: creator.into(),
        status: Status::Active,
        version: 1,
        acl_id: None,
    }
}

fn rendition_of_job(job: &JobRow) -> Result<(ObjectId, ObjectId), ExtractJobError> {
    let text = |name: &str| job.input.get(name).and_then(Value::as_str).map(|s| ObjectId(s.into()));
    match (text("rendition_id"), text("source_id")) {
        (Some(r), Some(s)) => Ok((r, s)),
        _ => Err(ExtractJobError::Fatal("the job names no rendition".into())),
    }
}

pub(super) fn all_types<S: Snapshot>(snap: &S) -> Result<Vec<EntityTypeRow>, crate::StoreError> {
    let (mut types, mut after): (Vec<EntityTypeRow>, Option<String>) = (vec![], None);
    loop {
        let page = entity::list_types(snap, after.as_deref(), RESULT_LIMIT)?;
        let Some(last) = page.last() else { return Ok(types) };
        after = Some(last.name.clone());
        types.extend(page);
    }
}

fn all_derivations<S: Snapshot>(snap: &S, rendition: &ObjectId) -> Result<Vec<DerivationRow>, crate::StoreError> {
    let (mut out, mut after): (Vec<DerivationRow>, Option<ObjectId>) = (vec![], None);
    loop {
        let page = snap.derivations_of(rendition, after.as_ref(), PAGE_LIMIT)?;
        let Some(last) = page.last() else { return Ok(out) };
        after = Some(last.header.id.clone());
        out.extend(page);
    }
}

fn all_segments<S: Snapshot>(snap: &S, rendition: &ObjectId) -> Result<Vec<SegmentRow>, crate::StoreError> {
    let (mut out, mut after): (Vec<SegmentRow>, Option<ObjectId>) = (vec![], None);
    loop {
        let page = snap.segments_of(rendition, after.as_ref(), PAGE_LIMIT)?;
        let Some(last) = page.last() else { return Ok(out) };
        after = Some(last.header.id.clone());
        out.extend(page);
    }
}

/// Existing entities of this type sharing an identity value with the mention. The answer is candidates for a person to weigh, and never a merge.
fn identity_candidates<S: Snapshot>(
    snap: &S,
    type_name: &str,
    schema: &Value,
    fields: &serde_json::Map<String, Value>,
) -> Result<Vec<IdentityCandidate>, crate::StoreError> {
    let mut out: Vec<IdentityCandidate> = vec![];
    for def in entity::fields_of(schema).iter().filter(|d| d.identity) {
        let Some(text) = fields.get(&def.name).and_then(Value::as_str) else {
            continue;
        };
        for m in entity::identity_matches(snap, &entity::fold_key(text), MAX_CANDIDATES)? {
            if m.type_name == type_name && !out.iter().any(|c| c.entity_id == m.entity_id) && out.len() < MAX_CANDIDATES {
                out.push(IdentityCandidate {
                    entity_id: m.entity_id,
                    field: m.field,
                });
            }
        }
    }
    Ok(out)
}

fn with_candidates<S: Snapshot>(snap: &S, types: &[EntityTypeRow], items: Vec<ExtractedItem>) -> Result<Vec<ExtractedItem>, crate::StoreError> {
    items
        .into_iter()
        .map(|item| match item {
            ExtractedItem::Entity {
                segment_id,
                quote,
                type_name,
                fields,
                ..
            } => {
                let candidates = match types.iter().find(|t| t.name == type_name) {
                    Some(t) => identity_candidates(snap, &type_name, &t.schema, &fields)?,
                    None => vec![],
                };
                Ok(ExtractedItem::Entity {
                    segment_id,
                    quote,
                    type_name,
                    fields,
                    candidates,
                })
            }
            claim => Ok(claim),
        })
        .collect()
}

fn fresh_row(rendition: &ObjectId, source: &ObjectId, job: &ObjectId, budget: u64, now: i64) -> ExtractionRow {
    ExtractionRow {
        header: header(rendition.clone(), EXTRACTOR),
        source_id: source.clone(),
        job_id: job.clone(),
        model: None,
        prompt_hash: None,
        segments_total: 0,
        segments_done: 0,
        batches_failed: 0,
        proposed: 0,
        rejected: 0,
        usage: AgentUsage::default(),
        token_budget: budget,
        stop: None,
        updated_at_ms: now,
    }
}

struct RunContext<'a> {
    vault: &'a VaultId,
    job: &'a JobRow,
    rendition: &'a ObjectId,
    source: &'a ObjectId,
    model: String,
    prompt: &'a ExtractionPrompt,
}

impl<E: ServerEdges> Server<E> {
    /// Turns extraction on, which is what lets documents go to the LLM. Only the composition root does this, and only when an endpoint is set up.
    pub fn set_extraction(&self, limits: Option<ExtractionLimits>) {
        *self.extraction.lock().expect("lock") = limits;
    }

    fn extraction_limits(&self) -> Option<ExtractionLimits> {
        *self.extraction.lock().expect("lock")
    }

    /// The rows that queue a rendition's extraction, for committing with the rendition itself. None when extraction is off.
    pub(super) fn extraction_rows(&self, source: &ObjectId, rendition: &ObjectId) -> Vec<Mutation> {
        let Some(limits) = self.extraction_limits() else { return vec![] };
        let now = self.now();
        let job = jobs::new_job(
            ObjectId(self.new_id()),
            EXTRACTOR,
            EXTRACT_JOB,
            json!({"rendition_id": rendition, "source_id": source}),
            format!("{EXTRACT_JOB}:{rendition}"),
            EXTRACT_ATTEMPTS,
            now,
        );
        let row = fresh_row(rendition, source, &job.header.id, limits.document_budget, now);
        vec![Mutation::Put(Row::Job(job)), Mutation::Put(Row::Extraction(row))]
    }

    fn latest_rendition<S: Snapshot>(snap: &S, source: &ObjectId) -> Result<Option<RenditionRow>, ServerError> {
        Ok(snap.renditions_of(source)?.into_iter().find(|r| r.superseded_by.is_none()))
    }

    /// Runs extraction again on the document's current rendition. Segments this prompt already has results for cost nothing and repeat nothing, so this finishes a partial run, and picks up new types. Asking while a run is open gets that run.
    pub fn rerun_extraction(&self, caller: &Caller, vault: &VaultId, source_id: &ObjectId) -> Result<JobRow, ServerError> {
        self.allow(caller, Operation::VaultContentPropose(vault))?;
        self.vault_exists(vault)?;
        let Some(limits) = self.extraction_limits() else {
            return Err(ServerError::Conflict("extraction isn't set up on this server".into()));
        };
        self.get_source(caller, vault, source_id)?;
        let lease = self.lease(vault)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let snap = self.engine.store().snapshot(vault)?;
        let Some(rendition) = Self::latest_rendition(&snap, source_id)? else {
            return Err(ServerError::Conflict("this document has no rendition yet".into()));
        };
        let rid = rendition.header.id;
        if let Some(Row::Extraction(row)) = snap.get(&RowKey {
            table: Table::Extraction,
            id: rid.clone(),
        })? && let Some(Row::Job(open)) = snap.get(&RowKey {
            table: Table::Job,
            id: row.job_id,
        })? && matches!(open.state, JobState::Queued | JobState::Running)
        {
            return Ok(open);
        }
        drop(snap);
        let now = self.now();
        let id = ObjectId(self.new_id());
        let job = jobs::new_job(
            id.clone(),
            &caller.principal.id,
            EXTRACT_JOB,
            json!({"rendition_id": rid, "source_id": source_id}),
            format!("{EXTRACT_JOB}:{rid}:rerun:{id}"),
            EXTRACT_ATTEMPTS,
            now,
        );
        let mut row = fresh_row(&rid, source_id, &id, limits.document_budget, now);
        row.header.version = self.extraction_version(vault, &rid)? + 1;
        self.state_event(
            &lease,
            &caller.principal.id,
            vec![Mutation::Put(Row::Job(job.clone())), Mutation::Put(Row::Extraction(row))],
        )?;
        Ok(job)
    }

    fn extraction_version(&self, vault: &VaultId, rendition: &ObjectId) -> Result<u32, ServerError> {
        let row = self.engine.store().snapshot(vault)?.get(&RowKey {
            table: Table::Extraction,
            id: rendition.clone(),
        })?;
        Ok(row.map_or(0, |r| r.header().version))
    }

    /// Where a document's extraction stands.
    pub fn extraction_status(&self, caller: &Caller, vault: &VaultId, source_id: &ObjectId) -> Result<ExtractionReport, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        self.get_source(caller, vault, source_id)?;
        let snap = self.engine.store().snapshot(vault)?;
        let rendition = Self::latest_rendition(&snap, source_id)?.map(|r| r.header.id);
        let row = match &rendition {
            Some(id) => match snap.get(&RowKey {
                table: Table::Extraction,
                id: id.clone(),
            })? {
                Some(Row::Extraction(r)) => Some(r),
                _ => None,
            },
            None => None,
        };
        let job = match &row {
            Some(r) => match snap.get(&RowKey {
                table: Table::Job,
                id: r.job_id.clone(),
            })? {
                Some(Row::Job(j)) => Some(j),
                _ => None,
            },
            None => None,
        };
        let state = match &row {
            Some(r) => extraction::extraction_state(r, job.as_ref()),
            None => ExtractionState::NotStarted,
        };
        let last_error = job.as_ref().and_then(|j| {
            j.wait
                .as_ref()
                .map(|w| w.last_error.clone())
                .or_else(|| j.attempts.iter().rev().find_map(|a| a.error.clone()))
        });
        Ok(ExtractionReport {
            source_id: source_id.clone(),
            rendition_id: rendition,
            state,
            job_id: job.as_ref().map(|j| j.header.id.clone()),
            waiting_since_ms: job.as_ref().and_then(|j| j.wait.as_ref().map(|w| w.since_ms)),
            last_error,
            row,
        })
    }

    /// What generated a rendition's results, in id order.
    pub fn list_derivations(
        &self,
        caller: &Caller,
        vault: &VaultId,
        rendition: &ObjectId,
        after: Option<&ObjectId>,
        limit: usize,
    ) -> Result<Vec<DerivationRow>, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        Ok(self.engine.store().snapshot(vault)?.derivations_of(rendition, after, limit)?)
    }

    /// Keeps a held job from expiring during a long call, and writes what's new with it. False means this worker no longer holds the job, and nothing was written.
    pub fn hold_job(&self, vault: &VaultId, held: &JobRow, for_ms: i64, progressed: bool, results: Vec<Mutation>) -> Result<bool, ServerError> {
        let lease = self.lease(vault)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let current = self.job(vault, &held.header.id)?;
        if !super::still_held(&current, held) {
            return Ok(false);
        }
        let owner = held.lease_owner.clone().unwrap_or_default();
        let mut next = jobs::lease_extended(current, self.now() + for_ms);
        if progressed {
            next.wait = None;
        }
        let mut mutations = results;
        mutations.push(Mutation::Put(Row::Job(next)));
        self.state_event(&lease, &owner, mutations)?;
        Ok(true)
    }

    /// Sends a held job back to wait for something that's away. False means this worker no longer holds the job.
    pub fn wait_job(&self, vault: &VaultId, held: &JobRow, error: &str) -> Result<bool, ServerError> {
        let lease = self.lease(vault)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let current = self.job(vault, &held.header.id)?;
        if !super::still_held(&current, held) {
            return Ok(false);
        }
        let owner = held.lease_owner.clone().unwrap_or_default();
        let next = jobs::waiting(current, error, self.now(), self.seed.next_u64());
        self.state_event(&lease, &owner, vec![Mutation::Put(Row::Job(next))])?;
        Ok(true)
    }

    fn item_proposal(&self, rendition: &ObjectId, derivation: &DerivationRow, item: &ExtractedItem) -> Proposal {
        let key = extraction::item_key(rendition, item);
        let origin = format!("Extracted by {} in derivation {}", derivation.model, derivation.header.id);
        let evidence = |segment_id: &ObjectId, quote: &str| EvidenceInput {
            segment_id: segment_id.clone(),
            quote: quote.into(),
            stance: Stance::Supports,
        };
        match item {
            ExtractedItem::Claim {
                segment_id,
                quote,
                subject,
                predicate,
                value,
            } => Proposal {
                ops: vec![Op::AddAssertion {
                    id: ObjectId(self.new_id()),
                    subject: subject.clone(),
                    predicate: predicate.clone(),
                    value: json!(value),
                    epistemic: memex_types::Epistemic::Asserted,
                    machine: true,
                    // A model's own confidence counts for nothing, so none is asked for.
                    confidence_permille: 1000,
                    valid_from_ms: None,
                    valid_to_ms: None,
                    evidence: vec![(ObjectId(self.new_id()), evidence(segment_id, quote))],
                }],
                rationale: format!("{origin}, from segment {segment_id}."),
                idempotency_key: key,
            },
            ExtractedItem::Entity {
                segment_id,
                quote,
                type_name,
                fields,
                candidates,
            } => {
                let id = ObjectId(self.new_id());
                let inputs: Vec<FieldInput> = fields
                    .iter()
                    .map(|(field, value)| FieldInput {
                        field: field.clone(),
                        value: value.clone(),
                        evidence: vec![evidence(segment_id, quote)],
                        replaces: None,
                    })
                    .collect();
                let mut ops = vec![Op::CreateEntity {
                    id: id.clone(),
                    type_name: type_name.clone(),
                    aliases: vec![],
                }];
                ops.extend(self.field_ops(&id, &inputs));
                let mut rationale = format!("{origin}, from segment {segment_id}.");
                if !candidates.is_empty() {
                    let names: Vec<String> = candidates.iter().map(|c| format!("{} (on {})", c.entity_id, c.field)).collect();
                    rationale.push_str(&format!(" It may be the same as existing entities: {}.", names.join(", ")));
                }
                Proposal {
                    ops,
                    rationale,
                    idempotency_key: key,
                }
            }
        }
    }

    /// Makes a Derivation's proposals. A key seen before gets its first result, so this can run again after a crash. Returns how many are new and how many were refused.
    fn propose_items(&self, vault: &VaultId, rendition: &ObjectId, derivation: &DerivationRow) -> Result<(u32, u32), ServerError> {
        let (mut proposed, mut rejected) = (0, 0);
        for item in &derivation.items {
            let proposal = self.item_proposal(rendition, derivation, item);
            let by = ProposedBy {
                actor: EXTRACTOR,
                machine: true,
                extractor: &derivation.model,
                prompt_version: &derivation.prompt_hash,
            };
            match self.propose_as(&by, vault, proposal, false) {
                Ok(outcome) if outcome.replayed => {}
                Ok(outcome) if outcome.changeset.as_ref().is_some_and(|c| c.state == ChangeSetState::Rejected) => rejected += 1,
                Ok(_) => proposed += 1,
                Err(ServerError::Invalid(m)) | Err(ServerError::Conflict(m)) => {
                    tracing::warn!(error = %m, "an extracted item can't be proposed");
                    rejected += 1;
                }
                Err(e) => return Err(e),
            }
        }
        Ok((proposed, rejected))
    }

    fn write_derivation(
        &self,
        run: &RunContext<'_>,
        batch: &[SegmentRow],
        outcome: (DerivationOutcome, Option<String>),
        usage: AgentUsage,
        attempts: u32,
        found: (Vec<ExtractedItem>, Vec<RejectedItem>),
    ) -> DerivationRow {
        DerivationRow {
            header: header(ObjectId(self.new_id()), EXTRACTOR),
            kind: DERIVATION_KIND.into(),
            job_id: run.job.header.id.clone(),
            source_id: run.source.clone(),
            rendition_id: run.rendition.clone(),
            segment_ids: batch.iter().map(|s| s.header.id.clone()).collect(),
            model: run.model.clone(),
            prompt_hash: run.prompt.hash.clone(),
            usage,
            attempts,
            outcome: outcome.0,
            detail: outcome.1,
            items: found.0,
            rejected: found.1,
            created_at_ms: self.now(),
        }
    }

    /// One job's work. Each batch's Derivation commits with the job's progress before its proposals are made, so a crash costs no second call: the next attempt makes the proposals from what's stored. It returns the row to commit with the job's completion.
    pub fn run_extraction_job<A: AgentRunner>(&self, vault: &VaultId, job: &JobRow, agent: &A) -> Result<Vec<Mutation>, ExtractJobError> {
        let Some(limits) = self.extraction_limits() else {
            return Err(ExtractJobError::Wait("extraction isn't set up on this server".into()));
        };
        let (rendition_id, source_id) = rendition_of_job(job)?;
        let snap = self.engine.store().snapshot(vault)?;
        let Some(Row::Rendition(rendition)) = snap.get(&RowKey {
            table: Table::Rendition,
            id: rendition_id.clone(),
        })?
        else {
            return Err(ExtractJobError::Fatal("the rendition doesn't exist".into()));
        };
        let mut row = match snap.get(&RowKey {
            table: Table::Extraction,
            id: rendition_id.clone(),
        })? {
            Some(Row::Extraction(r)) if r.job_id == job.header.id => r,
            // A newer run has this rendition, so this job has nothing to do.
            Some(Row::Extraction(_)) => return Ok(vec![]),
            _ => fresh_row(&rendition_id, &source_id, &job.header.id, limits.document_budget, self.now()),
        };
        row.header.version += 1;
        row.model = Some(agent.model());
        if rendition.superseded_by.is_some() {
            row.stop = Some(ExtractionStop::Superseded);
            row.updated_at_ms = self.now();
            return Ok(vec![Mutation::Put(Row::Extraction(row))]);
        }
        // A new reading of the document is on its way, so what was proposed from older ones goes.
        self.retire_stale_proposals(vault, &source_id, &rendition_id)?;
        let types = all_types(&snap)?;
        let prompt = extraction::build_prompt(&types);
        row.prompt_hash = Some(prompt.hash.clone());
        let validator = extraction::reply_validator(&prompt.schema).map_err(|e| ExtractJobError::Fatal(format!("the reply schema doesn't compile: {e}")))?;
        let derivations = all_derivations(&snap, &rendition_id)?;
        let segments = all_segments(&snap, &rendition_id)?;
        drop(snap);
        let done: HashSet<&ObjectId> = derivations
            .iter()
            .filter(|d| d.outcome == DerivationOutcome::Done && d.prompt_hash == prompt.hash)
            .flat_map(|d| d.segment_ids.iter())
            .collect();
        row.segments_total = segments.len() as u32;
        let pending: Vec<SegmentRow> = segments.iter().filter(|s| !done.contains(&s.header.id)).cloned().collect();
        row.segments_done = row.segments_total - pending.len() as u32;
        // An earlier attempt of this job may have stopped between a Derivation and its proposals.
        for d in derivations.iter().filter(|d| d.job_id == job.header.id && d.outcome == DerivationOutcome::Done) {
            let (proposed, rejected) = self.propose_items(vault, &rendition_id, d)?;
            row.proposed += proposed;
            row.rejected += rejected;
        }
        let run = RunContext {
            vault,
            job,
            rendition: &rendition_id,
            source: &source_id,
            model: agent.model(),
            prompt: &prompt,
        };
        let check = |text: &str| -> Result<(), String> {
            let doc: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
            match validator.iter_errors(&doc).next() {
                Some(first) => Err(first.to_string()),
                None => Ok(()),
            }
        };
        row.batches_failed = 0;
        for batch in extraction::plan_batches(pending, limits.batch_tokens) {
            row.updated_at_ms = self.now();
            let input = extraction::batch_input(&batch);
            let estimated = prompt.estimated_tokens + extraction::estimated_tokens(&input);
            let remaining = limits.document_budget.saturating_sub(row.usage.total());
            let Some(max_output_tokens) = extraction::call_room(remaining, estimated, limits.output_tokens) else {
                row.stop = Some(ExtractionStop::Budget);
                break;
            };
            // Room for the call and its retry.
            let hold = limits.timeout_ms as i64 * 2 + LEASE_MARGIN_MS;
            if !self.hold_job(vault, job, hold, false, vec![Mutation::Put(Row::Extraction(row.clone()))])? {
                return Err(ExtractJobError::Lost);
            }
            let request = AgentRequest {
                instruction: prompt.instruction.clone(),
                input,
                schema_json: prompt.schema_json.clone(),
                budget: memex_types::AgentBudget {
                    max_output_tokens,
                    timeout_ms: limits.timeout_ms,
                },
            };
            let used = row.usage;
            let room = |spent: &AgentUsage| {
                let left = limits.document_budget.saturating_sub(used.total() + spent.total());
                extraction::call_room(left, estimated, limits.output_tokens)
            };
            let outcome = run_with_retry(agent, &request, &check, &room);
            row.usage.add(&outcome.usage);
            row.header.version += 1;
            row.updated_at_ms = self.now();
            match outcome.result {
                Ok(reply) => {
                    let doc: Value = serde_json::from_str(&reply.json).map_err(|e| ExtractJobError::Retry(e.to_string()))?;
                    let (items, rejected) = extraction::checked_items(&doc, &batch);
                    let items = {
                        let snap = self.engine.store().snapshot(vault)?;
                        with_candidates(&snap, &types, items)?
                    };
                    row.rejected += rejected.len() as u32;
                    row.segments_done += batch.len() as u32;
                    let derivation = self.write_derivation(
                        &run,
                        &batch,
                        (DerivationOutcome::Done, None),
                        outcome.usage,
                        outcome.attempts,
                        (items, rejected),
                    );
                    self.commit_batch_result(&run, &row, &derivation, hold, true)?;
                    let (proposed, refused) = self.propose_items(vault, &rendition_id, &derivation)?;
                    row.proposed += proposed;
                    row.rejected += refused;
                }
                // The tokens are spent whatever comes next, so they're on record first.
                Err(e) if e.is_outage() => {
                    if outcome.usage.total() > 0 {
                        let derivation = self.write_derivation(
                            &run,
                            &batch,
                            (DerivationOutcome::Failed, Some(e.to_string())),
                            outcome.usage,
                            outcome.attempts,
                            (vec![], vec![]),
                        );
                        self.commit_batch_result(&run, &row, &derivation, hold, false)?;
                    }
                    return Err(ExtractJobError::Wait(e.to_string()));
                }
                Err(e @ AgentError::Provider(_)) => {
                    if outcome.usage.total() > 0 {
                        let derivation = self.write_derivation(
                            &run,
                            &batch,
                            (DerivationOutcome::Failed, Some(e.to_string())),
                            outcome.usage,
                            outcome.attempts,
                            (vec![], vec![]),
                        );
                        self.commit_batch_result(&run, &row, &derivation, hold, false)?;
                    }
                    return Err(ExtractJobError::Retry(e.to_string()));
                }
                Err(e) => {
                    let derivation = self.write_derivation(
                        &run,
                        &batch,
                        (DerivationOutcome::Failed, Some(e.to_string())),
                        outcome.usage,
                        outcome.attempts,
                        (vec![], vec![]),
                    );
                    if outcome.out_of_budget {
                        row.stop = Some(ExtractionStop::Budget);
                        self.commit_batch_result(&run, &row, &derivation, hold, false)?;
                        break;
                    }
                    row.batches_failed += 1;
                    self.commit_batch_result(&run, &row, &derivation, hold, false)?;
                }
            }
        }
        row.updated_at_ms = self.now();
        row.header.version += 1;
        if row.stop.is_none() && row.batches_failed > 0 {
            row.stop = Some(ExtractionStop::FailedBatches);
        }
        // With nothing to show for any of it, this is a failure a person should see. The tokens are already on record in the Derivations.
        if row.batches_failed > 0 && row.segments_done == 0 {
            self.hold_job(vault, job, LEASE_MARGIN_MS, false, vec![Mutation::Put(Row::Extraction(row.clone()))])?;
            return Err(ExtractJobError::Fatal(format!("all {} batches returned nothing usable", row.batches_failed)));
        }
        Ok(vec![Mutation::Put(Row::Extraction(row))])
    }

    fn commit_batch_result(
        &self,
        run: &RunContext<'_>,
        row: &ExtractionRow,
        derivation: &DerivationRow,
        hold: i64,
        progressed: bool,
    ) -> Result<(), ExtractJobError> {
        let results = vec![Mutation::Put(Row::Derivation(derivation.clone())), Mutation::Put(Row::Extraction(row.clone()))];
        if self.hold_job(run.vault, run.job, hold, progressed, results)? {
            Ok(())
        } else {
            Err(ExtractJobError::Lost)
        }
    }

    /// Gives new evidence its anchor as it commits, within the segment it cites, so every committed claim resolves to a region. The quote validator has passed by then, so a failure here can only be a change of rendition, which is flagged like any broken anchor.
    pub(super) fn anchor_new_evidence<S: Snapshot>(&self, vault: &VaultId, snap: &S, mutations: &mut [Mutation]) -> Result<(), ServerError> {
        let keys: Vec<RowKey> = mutations
            .iter()
            .filter_map(|m| match m {
                Mutation::Put(Row::Evidence(e)) if e.anchor.is_none() => Some(RowKey {
                    table: Table::Segment,
                    id: e.segment_id.clone(),
                }),
                _ => None,
            })
            .collect();
        if keys.is_empty() {
            return Ok(());
        }
        let segments: Vec<SegmentRow> = snap
            .get_many(&keys)?
            .into_iter()
            .filter_map(|r| if let Some(Row::Segment(s)) = r { Some(s) } else { None })
            .collect();
        let mut sources = std::collections::HashMap::new();
        for m in mutations.iter_mut() {
            let Mutation::Put(Row::Evidence(e)) = m else { continue };
            if e.anchor.is_some() {
                continue;
            }
            let Some(segment) = segments.iter().find(|s| s.header.id == e.segment_id) else {
                continue;
            };
            let Some(rendition_id) = &segment.rendition_id else { continue };
            if !sources.contains_key(rendition_id) {
                let Some(Row::Rendition(r)) = snap.get(&RowKey {
                    table: Table::Rendition,
                    id: rendition_id.clone(),
                })?
                else {
                    continue;
                };
                let Some(Row::Source(source)) = snap.get(&RowKey {
                    table: Table::Source,
                    id: segment.source_id.clone(),
                })?
                else {
                    continue;
                };
                sources.insert(rendition_id.clone(), (self.canonical_of(vault, &r)?, source.sha256, self.timed_of(vault, &r)?));
            }
            let ((canon, words), sha, timed) = &sources[rendition_id];
            match crate::rendition::locate_within(canon, words, sha, rendition_id, &e.quote, segment.start as usize, segment.end as usize) {
                Some(anchor) => match Self::with_time(anchor, timed.as_ref()) {
                    Some(anchor) => e.anchor = Some(anchor),
                    None => e.anchor_broken = true,
                },
                None => e.anchor_broken = true,
            }
        }
        Ok(())
    }
}
