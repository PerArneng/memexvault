//! The review gate on the facade: the queue, the reviewer agent's job, and what each combination has earned.
use super::extraction::{EXTRACTOR, LEASE_MARGIN_MS};
use super::{ExtractJobError, ProposedBy, Server, ServerEdges, ServerError};
use crate::auth::{Caller, Operation};
use crate::review::{self, ReviewLimits, SampleBy};
use crate::{AgentRunner, Snapshot, Store, governance, jobs, run_with_retry};
use memex_types::{
    AgentRequest, AgentUsage, AutoCommitRow, ChangeSetRow, ChangeSetState, Header, JobRow, Mutation, ObjectId, Op, PageFlag, PrincipalKind, ProposalOrigin,
    QueueCount, ReviewFilter, ReviewVerdictRow, Row, RowKey, Status, Table, VaultId, Verdict,
};
use serde_json::{Value, json};

const REVIEW_ATTEMPTS: u32 = 5;
/// The most decisions one request makes.
pub const MAX_BATCH: usize = 200;

/// A queue entry: the proposal and what the reviewer agent thought of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewItem {
    pub changeset: ChangeSetRow,
    pub verdict: Option<ReviewVerdictRow>,
}

/// One quote, with what a person needs to judge it. The reviewer agent sees none of this but the quote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewEvidence {
    pub claim_id: ObjectId,
    pub segment_id: ObjectId,
    pub quote: String,
    pub source_id: Option<ObjectId>,
    pub source_name: Option<String>,
    pub trust_tier: Option<String>,
    pub rendition_id: Option<ObjectId>,
    pub page_from: Option<u32>,
    pub page_to: Option<u32>,
    /// The rendition has been replaced, so this can't be approved as it stands.
    pub superseded: bool,
    /// What the pipeline flagged on its pages, such as hidden text.
    pub page_flags: Vec<PageFlag>,
}

/// Everything about one item.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewDetail {
    pub item: ReviewItem,
    pub evidence: Vec<ReviewEvidence>,
    /// Each type's standing under this proposal's extractor and prompt.
    pub combinations: Vec<AutoCommitEntry>,
    /// Only a person can approve this.
    pub needs_a_person: bool,
    /// Why auto-commit wouldn't take this even if earned.
    pub auto_commit_block: Option<String>,
}

/// One combination, with what its counts come to under the current threshold.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoCommitEntry {
    pub row: AutoCommitRow,
    pub precision: Option<f64>,
    pub wilson_bound: f64,
    pub on: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AutoCommitTable {
    pub threshold: f64,
    pub entries: Vec<AutoCommitEntry>,
}

/// One decision in a batch. Each stands alone, so one conflict doesn't undo the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchDecision {
    Approve { id: ObjectId, reason: Option<String> },
    Reject { id: ObjectId, reason: String },
    ApproveEdited { id: ObjectId, ops: Vec<Op>, reason: Option<String> },
}

impl BatchDecision {
    pub fn id(&self) -> &ObjectId {
        match self {
            Self::Approve { id, .. } | Self::Reject { id, .. } | Self::ApproveEdited { id, .. } => id,
        }
    }
}

/// What to do with a decision on an already decided ChangeSet. The same person asking for the same thing gets the result again, and anything else is a conflict.
pub(super) fn decided_already(row: &ChangeSetRow, caller: &Caller, wanted: ChangeSetState) -> Result<Option<ChangeSetRow>, ServerError> {
    if row.state == ChangeSetState::Pending {
        return Ok(None);
    }
    if row.state == wanted && row.decided_by.as_deref() == Some(caller.principal.id.as_str()) {
        return Ok(Some(row.clone()));
    }
    Err(ServerError::Conflict(format!(
        "this ChangeSet is already {:?}, decided by {}",
        row.state,
        row.decided_by.as_deref().unwrap_or("nobody on record")
    )))
}

fn entry(row: AutoCommitRow, threshold: f64) -> AutoCommitEntry {
    let total = row.correct + row.incorrect;
    AutoCommitEntry {
        precision: review::precision(row.correct, total),
        wilson_bound: review::wilson_lower_bound(row.correct, total),
        on: review::earned(row.correct, row.incorrect, threshold),
        row,
    }
}

fn combination_keys(origin: &ProposalOrigin) -> Vec<RowKey> {
    origin
        .item_types
        .iter()
        .map(|t| RowKey {
            table: Table::AutoCommit,
            id: review::combination_id(&origin.extractor, &origin.prompt_version, t),
        })
        .collect()
}

impl<E: ServerEdges> Server<E> {
    /// Turns the reviewer agent on, which sends claims and quotes to its model. Only the composition root does this.
    pub fn set_reviewer(&self, limits: Option<ReviewLimits>) {
        *self.reviewer.lock().expect("lock") = limits;
    }

    /// What a combination's Wilson bound has to reach. Settings keep it above zero.
    pub fn set_auto_commit_threshold(&self, threshold: f64) {
        *self.auto_commit_threshold.lock().expect("lock") = threshold;
    }

    fn threshold(&self) -> f64 {
        *self.auto_commit_threshold.lock().expect("lock")
    }

    pub(super) fn origin_of<S: Snapshot>(snap: &S, by: &ProposedBy<'_>, ops: &[Op]) -> Result<ProposalOrigin, ServerError> {
        let places = review::evidence_places(snap, ops)?;
        Ok(ProposalOrigin {
            extractor: by.extractor.to_string(),
            prompt_version: by.prompt_version.to_string(),
            item_types: review::item_types(snap, ops)?,
            source_ids: places.source_ids,
            rendition_ids: places.rendition_ids,
        })
    }

    /// Says why this proposal may commit with nobody looking, if it may: every type it touches has earned it, and it cites nothing that would keep it from a person's eyes anyway.
    pub(super) fn earned_auto_commit<S: Snapshot>(&self, snap: &S, origin: &ProposalOrigin, ops: &[Op]) -> Result<Option<String>, ServerError> {
        if origin.item_types.is_empty() {
            return Ok(None);
        }
        let threshold = self.threshold();
        let mut because = vec![];
        for (kind, row) in origin.item_types.iter().zip(snap.get_many(&combination_keys(origin))?) {
            let Some(Row::AutoCommit(row)) = row else { return Ok(None) };
            if !review::earned(row.correct, row.incorrect, threshold) {
                return Ok(None);
            }
            let bound = review::wilson_lower_bound(row.correct, row.correct + row.incorrect);
            because.push(format!("{kind}: {} of {} correct, bound {bound:.4}", row.correct, row.correct + row.incorrect));
        }
        if review::auto_commit_block(snap, ops)?.is_some() {
            return Ok(None);
        }
        Ok(Some(format!(
            "earned by {} under prompt {:?} at threshold {threshold}: {}",
            origin.extractor,
            origin.prompt_version,
            because.join("; ")
        )))
    }

    /// The reviewer's job for a proposal just queued, if there's a reviewer and something to show it.
    pub(super) fn review_job_rows(&self, row: &ChangeSetRow) -> Vec<Mutation> {
        if self.reviewer.lock().expect("lock").is_none() || review::review_input(&row.ops).is_none() {
            return vec![];
        }
        let id = &row.header.id;
        let job = jobs::new_job(
            ObjectId(self.new_id()),
            review::REVIEWER,
            review::REVIEW_JOB,
            json!({"changeset_id": id}),
            format!("{}:{id}", review::REVIEW_JOB),
            REVIEW_ATTEMPTS,
            self.now(),
        );
        vec![Mutation::Put(Row::Job(job))]
    }

    fn sample_rows<S: Snapshot>(
        &self,
        snap: &S,
        origin: &ProposalOrigin,
        correct: u64,
        incorrect: u64,
        who: &SampleBy<'_>,
    ) -> Result<Vec<Mutation>, ServerError> {
        let threshold = self.threshold();
        let rows = snap.get_many(&combination_keys(origin))?;
        Ok(origin
            .item_types
            .iter()
            .zip(rows)
            .map(|(kind, row)| {
                let row = match row {
                    Some(Row::AutoCommit(r)) => r,
                    _ => review::fresh_row(&origin.extractor, &origin.prompt_version, kind, who.at_ms),
                };
                Mutation::Put(Row::AutoCommit(review::with_samples(row, correct, incorrect, threshold, who)))
            })
            .collect())
    }

    /// What a decision adds to the precision table. Only a person's decision on a machine's proposal counts: never an agent's, and never the reviewer agent's verdict.
    pub(super) fn decision_samples<S: Snapshot>(
        &self,
        snap: &S,
        caller: &Caller,
        row: &ChangeSetRow,
        correct: bool,
        now: i64,
    ) -> Result<Vec<Mutation>, ServerError> {
        let Some(origin) = row
            .origin
            .as_ref()
            .filter(|_| row.proposer_is_machine && caller.principal.kind == PrincipalKind::Human)
        else {
            return Ok(vec![]);
        };
        // An always-review change, such as a merge, says nothing about how good this source's claims are, and auto-commit could never take it anyway.
        let proposed = row.original_ops.as_ref().unwrap_or(&row.ops);
        if governance::kinds_of(snap, proposed)?.iter().any(|k| governance::ALWAYS_REVIEW.contains(k)) {
            return Ok(vec![]);
        }
        let reason = format!(
            "{} by {}: ChangeSet {}",
            if correct { "approved" } else { "overruled" },
            caller.principal.id,
            row.header.id
        );
        let who = SampleBy {
            by: &caller.principal.id,
            reason: &reason,
            at_ms: now,
        };
        self.sample_rows(snap, origin, u64::from(correct), u64::from(!correct), &who)
    }

    /// Records what an audit found about a combination. For the server's own jobs, so spec 026's audit has somewhere to report.
    #[allow(clippy::too_many_arguments)]
    pub fn record_audit_samples(
        &self,
        vault: &VaultId,
        extractor: &str,
        prompt_version: &str,
        item_type: &str,
        correct: u64,
        incorrect: u64,
        by: &str,
        reason: &str,
    ) -> Result<AutoCommitRow, ServerError> {
        self.vault_exists(vault)?;
        let lease = self.lease(vault)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let snap = self.engine.store().snapshot(vault)?;
        let now = self.now();
        let key = RowKey {
            table: Table::AutoCommit,
            id: review::combination_id(extractor, prompt_version, item_type),
        };
        let row = match snap.get(&key)? {
            Some(Row::AutoCommit(r)) => r,
            _ => review::fresh_row(extractor, prompt_version, item_type, now),
        };
        drop(snap);
        let who = SampleBy { by, reason, at_ms: now };
        let row = review::with_samples(row, correct, incorrect, self.threshold(), &who);
        self.state_event(&lease, by, vec![Mutation::Put(Row::AutoCommit(row.clone()))])?;
        Ok(row)
    }

    /// Stops auto-commit for one combination at once, and only new samples can earn it back. There's deliberately no way to turn it on by hand.
    pub fn disable_auto_commit(&self, caller: &Caller, vault: &VaultId, combination: &ObjectId, reason: String) -> Result<AutoCommitEntry, ServerError> {
        self.allow(caller, Operation::VaultContentReview(vault))?;
        self.vault_exists(vault)?;
        if reason.trim().is_empty() {
            return Err(ServerError::Invalid("disabling needs a reason".into()));
        }
        let lease = self.lease(vault)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let found = self.engine.store().snapshot(vault)?.get(&RowKey {
            table: Table::AutoCommit,
            id: combination.clone(),
        })?;
        let Some(Row::AutoCommit(row)) = found else {
            return Err(ServerError::NotFound);
        };
        let who = SampleBy {
            by: &caller.principal.id,
            reason: &reason,
            at_ms: self.now(),
        };
        let row = review::disabled(row, &who);
        self.state_event(&lease, &caller.principal.id, vec![Mutation::Put(Row::AutoCommit(row.clone()))])?;
        Ok(entry(row, self.threshold()))
    }

    /// The precision table, in id order, with the threshold it's judged by.
    pub fn auto_commit_table(&self, caller: &Caller, vault: &VaultId, after: Option<&ObjectId>, limit: usize) -> Result<AutoCommitTable, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        let threshold = self.threshold();
        let snap = self.engine.store().snapshot(vault)?;
        let (mut entries, mut cursor) = (vec![], after.cloned());
        loop {
            let page = snap.rows(Table::AutoCommit, cursor.as_ref())?;
            let Some(last) = page.last() else { break };
            cursor = Some(last.header().id.clone());
            for r in page {
                if let Row::AutoCommit(row) = r {
                    entries.push(entry(row, threshold));
                    if entries.len() == limit {
                        return Ok(AutoCommitTable { threshold, entries });
                    }
                }
            }
        }
        Ok(AutoCommitTable { threshold, entries })
    }

    fn verdict_rows<S: Snapshot>(snap: &S, changesets: Vec<ChangeSetRow>) -> Result<Vec<ReviewItem>, ServerError> {
        let keys: Vec<RowKey> = changesets
            .iter()
            .map(|c| RowKey {
                table: Table::ReviewVerdict,
                id: c.header.id.clone(),
            })
            .collect();
        let verdicts = if keys.is_empty() { vec![] } else { snap.get_many(&keys)? };
        Ok(changesets
            .into_iter()
            .zip(verdicts)
            .map(|(changeset, v)| ReviewItem {
                changeset,
                verdict: if let Some(Row::ReviewVerdict(v)) = v { Some(v) } else { None },
            })
            .collect())
    }

    /// The queue, or anything else the filter asks for, in id order.
    pub fn review_queue(
        &self,
        caller: &Caller,
        vault: &VaultId,
        filter: &ReviewFilter,
        after: Option<&ObjectId>,
        limit: usize,
    ) -> Result<Vec<ReviewItem>, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        let rows = snap.review_queue(filter, after, limit)?;
        Self::verdict_rows(&snap, rows)
    }

    pub fn review_counts(&self, caller: &Caller, vault: &VaultId) -> Result<Vec<QueueCount>, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        Ok(self.engine.store().snapshot(vault)?.review_counts()?)
    }

    pub fn pending_count(&self, caller: &Caller, vault: &VaultId) -> Result<u64, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        Ok(self.engine.store().snapshot(vault)?.pending_count()?)
    }

    /// One item, with its verdict, what its quotes rest on and where its combination stands.
    pub fn review_item(&self, caller: &Caller, vault: &VaultId, id: &ObjectId) -> Result<ReviewDetail, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        let changeset = self.changeset(vault, id)?;
        let kinds = governance::kinds_of(&snap, &changeset.ops)?;
        let needs_a_person = crate::entity::needs_a_person(&kinds, &changeset.ops, changeset.proposer_is_machine);
        let auto_commit_block = review::auto_commit_block(&snap, &changeset.ops)?;
        let threshold = self.threshold();
        let combinations = match &changeset.origin {
            Some(origin) => snap
                .get_many(&combination_keys(origin))?
                .into_iter()
                .flatten()
                .filter_map(|r| if let Row::AutoCommit(row) = r { Some(entry(row, threshold)) } else { None })
                .collect(),
            None => vec![],
        };
        let evidence = Self::evidence_view(&snap, &changeset.ops)?;
        let item = Self::verdict_rows(&snap, vec![changeset])?.remove(0);
        Ok(ReviewDetail {
            item,
            evidence,
            combinations,
            needs_a_person,
            auto_commit_block,
        })
    }

    fn evidence_view<S: Snapshot>(snap: &S, ops: &[Op]) -> Result<Vec<ReviewEvidence>, ServerError> {
        let places = review::evidence_places(snap, ops)?;
        let keys: Vec<RowKey> = places
            .source_ids
            .iter()
            .map(|id| RowKey {
                table: Table::Source,
                id: id.clone(),
            })
            .chain(places.rendition_ids.iter().map(|id| RowKey {
                table: Table::Rendition,
                id: id.clone(),
            }))
            .collect();
        let rows = if keys.is_empty() { vec![] } else { snap.get_many(&keys)? };
        let (mut sources, mut renditions) = (std::collections::HashMap::new(), std::collections::HashMap::new());
        for row in rows.into_iter().flatten() {
            match row {
                Row::Source(s) => {
                    sources.insert(s.header.id.clone(), s);
                }
                Row::Rendition(r) => {
                    renditions.insert(r.header.id.clone(), r);
                }
                _ => {}
            }
        }
        let mut out = vec![];
        for op in ops {
            let claim = match op {
                Op::SupersedeAssertion { replacement, .. } => replacement.as_ref(),
                other => other,
            };
            let Op::AddAssertion { id, evidence, .. } = claim else { continue };
            for (_, e) in evidence {
                let segment = places.segments.iter().find(|s| s.header.id == e.segment_id);
                let source = segment.and_then(|s| sources.get(&s.source_id));
                let rendition = segment.and_then(|s| s.rendition_id.as_ref()).and_then(|r| renditions.get(r));
                let (from, to) = (segment.and_then(|s| s.page_from), segment.and_then(|s| s.page_to));
                let mut page_flags = vec![];
                if let (Some(r), Some(from), Some(to)) = (rendition, from, to) {
                    for p in r.pages.iter().filter(|p| p.index >= from && p.index <= to) {
                        page_flags.extend(p.flags.iter().cloned());
                    }
                }
                out.push(ReviewEvidence {
                    claim_id: id.clone(),
                    segment_id: e.segment_id.clone(),
                    quote: e.quote.clone(),
                    source_id: segment.map(|s| s.source_id.clone()),
                    source_name: source.map(|s| s.name.clone()),
                    trust_tier: source.map(|s| s.trust_tier.clone()),
                    rendition_id: segment.and_then(|s| s.rendition_id.clone()),
                    page_from: from,
                    page_to: to,
                    superseded: rendition.is_some_and(|r| r.superseded_by.is_some()),
                    page_flags,
                });
            }
        }
        Ok(out)
    }

    /// Many decisions, each attributed and each on its own, so the answer has one result per decision, in order.
    pub fn decide_batch(&self, caller: &Caller, vault: &VaultId, decisions: Vec<BatchDecision>) -> Result<Vec<Result<ChangeSetRow, ServerError>>, ServerError> {
        self.allow(caller, Operation::VaultContentReview(vault))?;
        self.vault_exists(vault)?;
        if decisions.len() > MAX_BATCH {
            return Err(ServerError::Invalid(format!("at most {MAX_BATCH} decisions at once")));
        }
        Ok(decisions
            .into_iter()
            .map(|d| match d {
                BatchDecision::Approve { id, reason } => self.approve(caller, vault, &id, reason),
                BatchDecision::Reject { id, reason } => self.reject(caller, vault, &id, reason),
                BatchDecision::ApproveEdited { id, ops, reason } => self.approve_edited(caller, vault, &id, ops, reason),
            })
            .collect())
    }

    /// Takes extraction's pending proposals on a document's replaced renditions out of the queue. They can't be approved, and the run that calls this will propose what's still there. It doesn't count against the extractor, as nobody judged them.
    pub(super) fn retire_stale_proposals(&self, vault: &VaultId, source: &ObjectId, current: &ObjectId) -> Result<(), ServerError> {
        let lease = self.lease(vault)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let snap = self.engine.store().snapshot(vault)?;
        let old: Vec<ObjectId> = snap
            .renditions_of(source)?
            .into_iter()
            .filter(|r| &r.header.id != current && r.superseded_by.is_some())
            .map(|r| r.header.id)
            .collect();
        let now = self.now();
        let mut retired = vec![];
        for rendition in &old {
            let mut after = None;
            loop {
                let page = snap.pending_citing_rendition(rendition, after.as_ref(), crate::PAGE_LIMIT)?;
                let Some(last) = page.last() else { break };
                after = Some(last.header.id.clone());
                for mut row in page.into_iter().filter(|c| c.proposer == EXTRACTOR) {
                    row.state = ChangeSetState::Rejected;
                    row.decided_by = Some(review::PIPELINE.to_string());
                    row.decided_at_ms = Some(now);
                    row.decision_reason = Some(format!("rendition {rendition} has been replaced, and {current} is being extracted instead"));
                    row.header.version += 1;
                    retired.push(Mutation::Put(Row::ChangeSet(row)));
                }
            }
        }
        drop(snap);
        if !retired.is_empty() {
            self.state_event(&lease, review::PIPELINE, retired)?;
        }
        Ok(())
    }

    /// One reviewer call on one queued proposal. It sees the claims and quotes, has no tools, and can only write its own verdict, which commits with the job. It holds no token, so it couldn't do more if it tried. An outage leaves the item where it was, in the queue with no verdict, and the job waits.
    pub fn run_review_job<A: AgentRunner>(&self, vault: &VaultId, job: &JobRow, agent: &A) -> Result<Vec<Mutation>, ExtractJobError> {
        let Some(limits) = *self.reviewer.lock().expect("lock") else {
            return Err(ExtractJobError::Wait("no reviewer is set up on this server".into()));
        };
        let Some(id) = job.input.get("changeset_id").and_then(Value::as_str).map(|s| ObjectId(s.into())) else {
            return Err(ExtractJobError::Fatal("the job names no ChangeSet".into()));
        };
        let snap = self.engine.store().snapshot(vault)?;
        let changeset = self.changeset(vault, &id).map_err(ExtractJobError::from)?;
        // Someone may have decided while this waited, and then the opinion is worth nothing.
        if changeset.state != ChangeSetState::Pending {
            return Ok(vec![]);
        }
        let Some(input) = review::review_input(&changeset.ops) else {
            return Ok(vec![]);
        };
        let prompt = review::review_prompt();
        let previous = match snap.get(&RowKey {
            table: Table::ReviewVerdict,
            id: id.clone(),
        })? {
            Some(Row::ReviewVerdict(v)) => Some(v),
            _ => None,
        };
        drop(snap);
        if previous.as_ref().is_some_and(|v| review::verdict_is_current(v, &prompt.hash)) {
            return Ok(vec![]);
        }
        let hold = limits.timeout_ms as i64 * 2 + LEASE_MARGIN_MS;
        if !self.hold_job(vault, job, hold, false, vec![])? {
            return Err(ExtractJobError::Lost);
        }
        let request = AgentRequest {
            instruction: prompt.instruction,
            input,
            schema_json: prompt.schema_json,
            budget: memex_types::AgentBudget {
                max_output_tokens: limits.output_tokens,
                timeout_ms: limits.timeout_ms,
            },
        };
        let check = |text: &str| review::parse_verdict(text).map(|_| ());
        let room = |_: &AgentUsage| Some(limits.output_tokens);
        let outcome = run_with_retry(agent, &request, &check, &room);
        let (verdict, reason) = match outcome.result {
            Ok(reply) => review::parse_verdict(&reply.json).map_err(ExtractJobError::Retry)?,
            Err(e) if e.is_outage() => return Err(ExtractJobError::Wait(e.to_string())),
            Err(e @ memex_types::AgentError::Provider(_)) => return Err(ExtractJobError::Retry(e.to_string())),
            // It answered, but nothing it said can be used, and asking again won't change that. Its tokens are on record.
            Err(e) => (Verdict::Abstain, format!("no usable answer: {e}")),
        };
        Ok(vec![Mutation::Put(Row::ReviewVerdict(ReviewVerdictRow {
            header: Header {
                id,
                creator: review::REVIEWER.into(),
                status: Status::Active,
                version: previous.map_or(1, |v| v.header.version + 1),
                acl_id: None,
            },
            verdict,
            reason,
            model: agent.model(),
            prompt_hash: prompt.hash,
            usage: outcome.usage,
            attempts: outcome.attempts,
            created_at_ms: self.now(),
        }))])
    }
}
