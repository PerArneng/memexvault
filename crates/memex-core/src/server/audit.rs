//! The audit agent on the facade: the schedule, a run's job, and planted errors in a test vault. It proposes through the same gate as anyone, holds only propose scope, and its results are events like everything else.
use super::extraction::LEASE_MARGIN_MS;
use super::{ExtractJobError, Proposal, ProposedBy, Server, ServerEdges, ServerError};
use crate::audit::{self, AUDIT_JOB, AUDITOR, AuditLimits, Candidate, EvidenceFacts, RecallReport};
use crate::auth::{Caller, Operation};
use crate::lexical::Touched;
use crate::review;
use crate::{AgentRunner, BlobStore, EMBED_JOB, LexicalIndex, PAGE_LIMIT, Seed, Snapshot, Store, VectorIndex, extraction, jobs, run_with_retry};
use memex_types::{
    AgentError, AgentRequest, AgentUsage, AssertionRow, AuditCheckKind as Kind, AuditCheckResult, AuditCheckRow, AuditCheckStatus as St, AuditCombination,
    AuditLlmUse, AuditOutcome, AuditPlantedRow, AuditRunRow, AuditRunState, AuditSchedule, AuditStateRow, AuditStop, AuditTrigger, BlobHash, ChangeSetRow,
    ChangeSetState, DocKind, ExtractedItem, Generation, Grant, Header, IndexHealth, IndexOp, JobRow, JobState, Mutation, ObjectId, Op, PlantedKind, Principal,
    PrincipalKind, ReviewFilter, Row, RowKey, Scope, SegmentRow, SiblingCursor, SourceRow, Status, Table, VaultFinding, VaultId, VectorHealth,
    entity_of_subject,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const AUDIT_ATTEMPTS: u32 = 3;
const STATE_ID: &str = "audit";
/// How long a worker keeps the job while it reads the whole vault, which has no call to time itself by.
const SCAN_HOLD_MS: i64 = 900_000;
/// Further than this behind the log, an index comparison isn't worth attempting.
const MAX_LAG_EVENTS: u64 = 5000;
const QUOTE_SHOWN_CHARS: usize = 500;
const LINKS_SHOWN: usize = 100;

/// What to run. Anything left out comes from the schedule, and the seed from the server's randomness.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TriggerAudit {
    pub seed: Option<u64>,
    pub sample_size: Option<u32>,
    pub run_token_budget: Option<u64>,
}

/// What the vault's audits may spend and have spent today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditBudget {
    pub run_token_budget: u64,
    pub day_token_budget: u64,
    pub day: i64,
    pub day_spent: u64,
    pub day_remaining: u64,
}

/// A run, with whether its job has given up, which a run row can't say for itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRunDetail {
    pub run: AuditRunRow,
    pub job_state: Option<JobState>,
}

/// A reply that met the schema, or why none did, and what the call spent.
type CallAnswer = (Result<String, String>, AuditLlmUse);

/// What's still to do for a claim about to finish: an earlier run's item, if this finding has one already.
struct Filed {
    changeset_id: Option<ObjectId>,
    earlier: Option<ChangeSetState>,
}

/// What a run knows about the indexes, for its per-claim check.
struct IndexView {
    /// None if the comparison can't be made now.
    present: Option<BTreeSet<ObjectId>>,
    touched: BTreeSet<ObjectId>,
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

fn fresh_state() -> AuditStateRow {
    AuditStateRow {
        header: header(ObjectId(STATE_ID.into()), AUDITOR),
        schedule: audit::default_schedule(),
        next_run_at_ms: None,
        total_missed_slots: 0,
        day: 0,
        day_spent: 0,
        test_vault: false,
        sibling_cursors: vec![],
    }
}

fn state_of<S: Snapshot>(snap: &S) -> Result<AuditStateRow, crate::StoreError> {
    Ok(
        match snap.get(&RowKey {
            table: Table::AuditState,
            id: ObjectId(STATE_ID.into()),
        })? {
            Some(Row::AuditState(s)) => s,
            _ => fresh_state(),
        },
    )
}

fn table_keys(table: Table, ids: impl IntoIterator<Item = ObjectId>) -> Vec<RowKey> {
    ids.into_iter().collect::<BTreeSet<_>>().into_iter().map(|id| RowKey { table, id }).collect()
}

fn rows_by_id<S: Snapshot>(snap: &S, table: Table, ids: impl IntoIterator<Item = ObjectId>) -> Result<HashMap<ObjectId, Row>, crate::StoreError> {
    let keys = table_keys(table, ids);
    if keys.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(snap.get_many(&keys)?.into_iter().flatten().map(|r| (r.header().id.clone(), r)).collect())
}

/// The combination a machine's claim counts under, and what kind of item it is. None for a person's claim.
fn claim_type(claim: &AssertionRow, entities: &HashMap<ObjectId, Row>) -> String {
    match entity_of_subject(&claim.subject) {
        Some(id) => match entities.get(&id) {
            Some(Row::Entity(e)) => format!("entity:{}", e.type_name),
            _ => "entity".to_string(),
        },
        None => "claim".to_string(),
    }
}

fn combination_of(claim: &AssertionRow, changeset: Option<&ChangeSetRow>, item_type: &str) -> Option<AuditCombination> {
    let cs = changeset?;
    let origin = cs.origin.as_ref()?;
    // A chat's proposal earns nothing, and a person approved it, so it has no record for an audit to add to or take away from.
    (claim.machine && cs.proposer_is_machine && cs.requested_by.is_none()).then(|| AuditCombination {
        extractor: origin.extractor.clone(),
        prompt_version: origin.prompt_version.clone(),
        item_type: item_type.into(),
    })
}

fn lowest_tier<'a>(tiers: impl Iterator<Item = &'a str>) -> Option<String> {
    let rank = |t: &str| match t {
        crate::UNTRUSTED => 0,
        "verified" => 3,
        "trusted" => 2,
        _ => 1,
    };
    tiers.min_by_key(|t| rank(t)).map(String::from)
}

/// One page of candidates. Store calls don't grow with the size of the page.
fn candidates_in<S: Snapshot>(snap: &S, claims: &[AssertionRow]) -> Result<Vec<(Candidate, Option<AuditCombination>)>, crate::StoreError> {
    let ids: Vec<ObjectId> = claims.iter().map(|c| c.header.id.clone()).collect();
    let changesets = rows_by_id(snap, Table::ChangeSet, claims.iter().map(|c| c.changeset_id.clone()))?;
    let entities = rows_by_id(snap, Table::Entity, claims.iter().filter_map(|c| entity_of_subject(&c.subject)))?;
    let evidence = if ids.is_empty() { vec![] } else { snap.evidence_of(&ids)? };
    let segments = rows_by_id(snap, Table::Segment, evidence.iter().map(|e| e.segment_id.clone()))?;
    let sources = rows_by_id(
        snap,
        Table::Source,
        segments
            .values()
            .filter_map(|r| if let Row::Segment(s) = r { Some(s.source_id.clone()) } else { None }),
    )?;
    let mut tiers: HashMap<&ObjectId, Vec<&str>> = HashMap::new();
    for e in &evidence {
        if let Some(Row::Segment(s)) = segments.get(&e.segment_id)
            && let Some(Row::Source(source)) = sources.get(&s.source_id)
        {
            tiers.entry(&e.assertion_id).or_default().push(&source.trust_tier);
        }
    }
    Ok(claims
        .iter()
        .map(|c| {
            let cs = match changesets.get(&c.changeset_id) {
                Some(Row::ChangeSet(cs)) => Some(cs),
                _ => None,
            };
            let item_type = claim_type(c, &entities);
            let combination = combination_of(c, cs, &item_type);
            let candidate = Candidate {
                id: c.header.id.clone(),
                extractor: cs
                    .and_then(|x| x.origin.as_ref())
                    .map_or_else(|| c.header.creator.clone(), |o| o.extractor.clone()),
                item_type,
                auto_committed: cs.is_some_and(|x| x.decided_by.as_deref() == Some(review::AUTO_COMMIT)),
                committed_at_ms: cs.and_then(|x| x.decided_at_ms).unwrap_or(0),
                trust_tier: tiers.get(&c.header.id).and_then(|t| lowest_tier(t.iter().copied())),
            };
            (candidate, combination)
        })
        .collect())
}

fn shown(text: &str) -> String {
    text.chars().take(QUOTE_SHOWN_CHARS).collect()
}

impl<E: ServerEdges> Server<E> {
    /// Turns the LLM checks on, which sends claims, quotes and segments to its model. Only the composition root does this.
    pub fn set_auditor(&self, limits: Option<AuditLimits>) {
        *self.auditor.lock().expect("lock") = limits;
    }

    /// Who the audit agent is: a machine holding propose scope on this vault and nothing else. Its writes all pass a check against this, so what it can't do is the same as for any such token.
    pub fn audit_agent_caller(&self, vault: &VaultId) -> Caller {
        Caller {
            principal: Principal {
                id: AUDITOR.into(),
                name: AUDITOR.into(),
                kind: PrincipalKind::Agent,
                server_admin: false,
                disabled: false,
                created_by: "server".into(),
                created_at_ms: 0,
            },
            token_id: String::new(),
            grants: vec![Grant {
                vault: vault.clone(),
                scope: Scope::Propose,
            }],
        }
    }

    fn audit_state(&self, vault: &VaultId) -> Result<AuditStateRow, ServerError> {
        Ok(state_of(&self.engine.store().snapshot(vault)?)?)
    }

    pub fn audit_schedule(&self, caller: &Caller, vault: &VaultId) -> Result<(AuditSchedule, Option<i64>, u64), ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        let s = self.audit_state(vault)?;
        Ok((s.schedule, s.next_run_at_ms, s.total_missed_slots))
    }

    /// Replaces the schedule. The next run is one interval from now, so turning it on doesn't start a run at once.
    pub fn set_audit_schedule(&self, caller: &Caller, vault: &VaultId, schedule: AuditSchedule) -> Result<(AuditSchedule, Option<i64>, u64), ServerError> {
        self.allow(caller, Operation::VaultContentReview(vault))?;
        self.vault_exists(vault)?;
        audit::check_schedule(&schedule).map_err(ServerError::Invalid)?;
        let lease = self.lease(vault)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let mut state = state_of(&self.engine.store().snapshot(vault)?)?;
        let changed_time = !state.schedule.enabled || state.schedule.interval_ms != schedule.interval_ms;
        state.next_run_at_ms = match (schedule.enabled, changed_time) {
            (false, _) => None,
            (true, true) => Some(self.now() + schedule.interval_ms),
            (true, false) => state.next_run_at_ms,
        };
        state.schedule = schedule;
        state.header.version += 1;
        self.state_event(&lease, &caller.principal.id, vec![Mutation::Put(Row::AuditState(state.clone()))])?;
        Ok((state.schedule, state.next_run_at_ms, state.total_missed_slots))
    }

    pub fn audit_budget(&self, caller: &Caller, vault: &VaultId) -> Result<AuditBudget, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        let s = self.audit_state(vault)?;
        let day = audit::day_of(self.now());
        let day_spent = if s.day == day { s.day_spent } else { 0 };
        Ok(AuditBudget {
            run_token_budget: s.schedule.run_token_budget,
            day_token_budget: s.schedule.day_token_budget,
            day,
            day_spent,
            day_remaining: s.schedule.day_token_budget.saturating_sub(day_spent),
        })
    }

    fn new_run(&self, state: &AuditStateRow, seed: u64, trigger: AuditTrigger, by: &str, wanted: &TriggerAudit, missed: u32) -> (AuditRunRow, JobRow) {
        let now = self.now();
        let id = ObjectId(self.new_id());
        let job = jobs::new_job(
            ObjectId(self.new_id()),
            AUDITOR,
            AUDIT_JOB,
            json!({"run_id": id}),
            format!("{AUDIT_JOB}:{id}"),
            AUDIT_ATTEMPTS,
            now,
        );
        let run = AuditRunRow {
            header: header(id, AUDITOR),
            seed,
            trigger,
            requested_by: by.into(),
            state: AuditRunState::Queued,
            stop: None,
            stop_detail: None,
            sample_size: wanted.sample_size.unwrap_or(state.schedule.sample_size),
            population: 0,
            sampled: 0,
            state_seq: None,
            run_token_budget: wanted.run_token_budget.unwrap_or(state.schedule.run_token_budget),
            usage: AgentUsage::default(),
            model: None,
            llm_calls: 0,
            passed: 0,
            suspect: 0,
            failed: 0,
            llm_skipped: 0,
            revoked: vec![],
            siblings_requeued: 0,
            siblings_remaining: 0,
            missed_slots: missed,
            vault_findings: vec![],
            job_id: job.header.id.clone(),
            created_at_ms: now,
            started_at_ms: None,
            finished_at_ms: None,
        };
        (run, job)
    }

    /// Starts a run now. It spends tokens if there's an LLM, so it takes the scope that spending decisions take.
    pub fn trigger_audit(&self, caller: &Caller, vault: &VaultId, wanted: TriggerAudit) -> Result<AuditRunRow, ServerError> {
        self.allow(caller, Operation::VaultContentReview(vault))?;
        self.vault_exists(vault)?;
        if wanted.sample_size.is_some_and(|n| n == 0 || n > audit::MAX_SAMPLE) {
            return Err(ServerError::Invalid(format!("the sample size must be from 1 to {}", audit::MAX_SAMPLE)));
        }
        let lease = self.lease(vault)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let state = state_of(&self.engine.store().snapshot(vault)?)?;
        let seed = wanted.seed.unwrap_or_else(|| self.seed.next_u64());
        let (run, job) = self.new_run(&state, seed, AuditTrigger::OnDemand, &caller.principal.id, &wanted, 0);
        self.state_event(
            &lease,
            &caller.principal.id,
            vec![Mutation::Put(Row::AuditRun(run.clone())), Mutation::Put(Row::Job(job))],
        )?;
        Ok(run)
    }

    /// One scheduler step for one vault, going only by the clock. A slot that finds a run still open is passed over and counted, so runs can't pile up. Returns the run it started.
    pub fn audit_tick(&self, vault: &VaultId) -> Result<Option<AuditRunRow>, ServerError> {
        let now = self.now();
        // Read before anything else, as this runs often and nearly always has nothing to do.
        let seen = self.audit_state(vault)?;
        if !audit::due(&seen.schedule, seen.next_run_at_ms, now).run {
            return Ok(None);
        }
        let lease = self.lease(vault)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let snap = self.engine.store().snapshot(vault)?;
        let mut state = state_of(&snap)?;
        let due = audit::due(&state.schedule, state.next_run_at_ms, now);
        if !due.run {
            return Ok(None);
        }
        let open = !snap.jobs(Some(AUDIT_JOB), Some(JobState::Queued), None, 1)?.is_empty()
            || !snap.jobs(Some(AUDIT_JOB), Some(JobState::Running), None, 1)?.is_empty();
        drop(snap);
        state.next_run_at_ms = due.next;
        state.total_missed_slots += u64::from(due.missed) + u64::from(open);
        state.header.version += 1;
        if open {
            self.state_event(&lease, AUDITOR, vec![Mutation::Put(Row::AuditState(state))])?;
            return Ok(None);
        }
        let seed = self.seed.next_u64();
        let (run, job) = self.new_run(&state, seed, AuditTrigger::Scheduled, AUDITOR, &TriggerAudit::default(), due.missed);
        self.state_event(
            &lease,
            AUDITOR,
            vec![
                Mutation::Put(Row::AuditState(state)),
                Mutation::Put(Row::AuditRun(run.clone())),
                Mutation::Put(Row::Job(job)),
            ],
        )?;
        Ok(Some(run))
    }

    /// A scheduler step for every vault. One vault's trouble doesn't hold up the rest.
    pub fn audit_tick_all(&self) -> Result<(), ServerError> {
        for vault in self.vault_ids()? {
            if let Err(e) = self.audit_tick(&vault) {
                tracing::warn!(vault = %vault, error = %e, "can't start a scheduled audit");
            }
        }
        Ok(())
    }

    fn audit_run(&self, vault: &VaultId, id: &ObjectId) -> Result<AuditRunRow, ServerError> {
        match self.engine.store().snapshot(vault)?.get(&RowKey {
            table: Table::AuditRun,
            id: id.clone(),
        })? {
            Some(Row::AuditRun(r)) => Ok(r),
            _ => Err(ServerError::NotFound),
        }
    }

    pub fn list_audit_runs(&self, caller: &Caller, vault: &VaultId, after: Option<&ObjectId>, limit: usize) -> Result<Vec<AuditRunDetail>, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        let runs: Vec<AuditRunRow> = snap
            .rows(Table::AuditRun, after)?
            .into_iter()
            .filter_map(|r| if let Row::AuditRun(x) = r { Some(x) } else { None })
            .take(limit)
            .collect();
        let jobs = rows_by_id(&snap, Table::Job, runs.iter().map(|r| r.job_id.clone()))?;
        Ok(runs
            .into_iter()
            .map(|run| AuditRunDetail {
                job_state: match jobs.get(&run.job_id) {
                    Some(Row::Job(j)) => Some(j.state),
                    _ => None,
                },
                run,
            })
            .collect())
    }

    pub fn get_audit_run(&self, caller: &Caller, vault: &VaultId, id: &ObjectId) -> Result<AuditRunDetail, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        let run = self.audit_run(vault, id)?;
        let job_state = match self.job(vault, &run.job_id) {
            Ok(j) => Some(j.state),
            Err(ServerError::NotFound) => None,
            Err(e) => return Err(e),
        };
        Ok(AuditRunDetail { run, job_state })
    }

    /// A run's sample in rank order, each with what was found. With `findings_only`, what didn't pass.
    pub fn audit_run_checks(
        &self,
        caller: &Caller,
        vault: &VaultId,
        run: &ObjectId,
        findings_only: bool,
        after: Option<&ObjectId>,
        limit: usize,
    ) -> Result<Vec<AuditCheckRow>, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        self.audit_run(vault, run)?;
        Ok(self.engine.store().snapshot(vault)?.audit_checks(run, findings_only, after, limit)?)
    }

    fn all_rows<S: Snapshot, T>(snap: &S, table: Table, pick: impl Fn(Row) -> Option<T>) -> Result<Vec<T>, ServerError> {
        let (mut out, mut after): (Vec<T>, Option<ObjectId>) = (vec![], None);
        loop {
            let page = snap.rows(table, after.as_ref())?;
            let Some(last) = page.last() else { return Ok(out) };
            after = Some(last.header().id.clone());
            out.extend(page.into_iter().filter_map(&pick));
        }
    }

    /// What every planted error in this vault came to. It's empty on a vault with none.
    pub fn audit_recall(&self, caller: &Caller, vault: &VaultId) -> Result<RecallReport, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        let planted = Self::all_rows(&snap, Table::AuditPlanted, |r| if let Row::AuditPlanted(p) = r { Some(p) } else { None })?;
        let (checks, runs) = if planted.is_empty() {
            (vec![], vec![])
        } else {
            (
                Self::all_rows(&snap, Table::AuditCheck, |r| if let Row::AuditCheck(c) = r { Some(c) } else { None })?,
                Self::all_rows(&snap, Table::AuditRun, |r| if let Row::AuditRun(c) = r { Some(c) } else { None })?,
            )
        };
        Ok(audit::recall_report(&planted, &checks, &runs, audit::SCRIPTED_MODEL))
    }

    /// Marks a vault as one errors can be planted in. It has to be empty, so nobody's knowledge can end up mixed with planted errors, and it can't be undone.
    pub fn mark_audit_test_vault(&self, caller: &Caller, vault: &VaultId) -> Result<(), ServerError> {
        self.allow(caller, Operation::VaultContentReview(vault))?;
        self.vault_exists(vault)?;
        let lease = self.lease(vault)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let snap = self.engine.store().snapshot(vault)?;
        if !snap.rows(Table::Source, None)?.is_empty() || !snap.rows(Table::Assertion, None)?.is_empty() {
            return Err(ServerError::Conflict("only an empty vault can become a test vault".into()));
        }
        let mut state = state_of(&snap)?;
        drop(snap);
        state.test_vault = true;
        state.header.version += 1;
        self.state_event(&lease, &caller.principal.id, vec![Mutation::Put(Row::AuditState(state))])?;
        Ok(())
    }

    /// Corrupts `per_kind` of the claims in a test vault for each kind, tags them, and tags what's left clean. This writes straight to rows, which is why it's for test vaults only. The keyword index must have caught up, as one kind removes a document from it. Returns how many of each it planted.
    pub fn plant_audit_errors(
        &self,
        caller: &Caller,
        vault: &VaultId,
        seed: u64,
        per_kind: u32,
        kinds: &[PlantedKind],
    ) -> Result<BTreeMap<PlantedKind, u64>, ServerError> {
        self.allow(caller, Operation::VaultContentReview(vault))?;
        self.vault_exists(vault)?;
        let lease = self.lease(vault)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let snap = self.engine.store().snapshot(vault)?;
        let mut state = state_of(&snap)?;
        if !state.test_vault {
            return Err(ServerError::Conflict("errors can only be planted in a test vault".into()));
        }
        if !snap.rows(Table::AuditPlanted, None)?.is_empty() {
            return Err(ServerError::Conflict("this vault already has planted errors".into()));
        }
        let mut claims: Vec<AssertionRow> = Self::all_rows(&snap, Table::Assertion, |r| match r {
            Row::Assertion(a) if audit::is_candidate(&a) && entity_of_subject(&a.subject).is_none() => Some(a),
            _ => None,
        })?;
        let needed = kinds.len() * per_kind as usize;
        if claims.len() < needed {
            return Err(ServerError::Invalid(format!("that needs {needed} claims, and this vault has {}", claims.len())));
        }
        // Each claim keyed by the seed and its id, so what gets which error is a function of those alone.
        let draws: Vec<Candidate> = claims
            .iter()
            .map(|c| Candidate {
                id: c.header.id.clone(),
                extractor: String::new(),
                item_type: String::new(),
                auto_committed: false,
                committed_at_ms: 0,
                trust_tier: None,
            })
            .collect();
        let order: HashMap<ObjectId, usize> = audit::sample(seed, &draws, draws.len())
            .into_iter()
            .enumerate()
            .map(|(i, p)| (p.id, i))
            .collect();
        claims.sort_by_key(|c| order[&c.header.id]);
        let ids: Vec<ObjectId> = claims.iter().map(|c| c.header.id.clone()).collect();
        let evidence = snap.evidence_of(&ids)?;
        let segments = rows_by_id(&snap, Table::Segment, evidence.iter().map(|e| e.segment_id.clone()))?;
        let sources = rows_by_id(
            &snap,
            Table::Source,
            segments
                .values()
                .filter_map(|r| if let Row::Segment(s) = r { Some(s.source_id.clone()) } else { None }),
        )?;
        let changesets = rows_by_id(&snap, Table::ChangeSet, claims.iter().map(|c| c.changeset_id.clone()))?;
        let health = self.lexical.health(vault, false)?;
        drop(snap);
        let now = self.now();
        let mut mutations = vec![];
        let mut planted: BTreeMap<PlantedKind, u64> = BTreeMap::new();
        let mut gone = vec![];
        let tag = |claim: &ObjectId, kind: PlantedKind, mutations: &mut Vec<Mutation>| {
            mutations.push(Mutation::Put(Row::AuditPlanted(AuditPlantedRow {
                header: header(claim.clone(), "planted"),
                kind,
                planted_at_ms: now,
            })));
        };
        let wanted = kinds.iter().flat_map(|k| std::iter::repeat_n(*k, per_kind as usize));
        let count_claims = claims.len();
        let mut queue = wanted.chain(std::iter::repeat_n(PlantedKind::Clean, count_claims - needed));
        for (n, claim) in claims.iter().enumerate() {
            let Some(kind) = queue.next() else { break };
            tag(&claim.header.id, kind, &mut mutations);
            if kind == PlantedKind::Clean {
                continue;
            }
            *planted.entry(kind).or_default() += 1;
            let first = evidence.iter().find(|e| e.assertion_id == claim.header.id).cloned();
            let segment = first.as_ref().and_then(|e| match segments.get(&e.segment_id) {
                Some(Row::Segment(s)) => Some(s.clone()),
                _ => None,
            });
            let mut changed = claim.clone();
            changed.header.version += 1;
            let rehash = |c: &mut AssertionRow| c.canonical_hash = crate::governance::canonical_hash(&c.subject, &c.predicate, &c.value);
            let need_evidence = || ServerError::Invalid(format!("planting {} needs a claim with evidence", kind.as_str()));
            match kind {
                PlantedKind::AlteredQuote => {
                    let mut e = first.ok_or_else(need_evidence)?;
                    e.quote = format!("altered {n}: {}", e.quote);
                    e.header.version += 1;
                    mutations.push(Mutation::Put(Row::Evidence(e)));
                }
                PlantedKind::WrongSegment => {
                    let mut e = first.ok_or_else(need_evidence)?;
                    let other = evidence
                        .iter()
                        .find(|o| {
                            o.segment_id != e.segment_id
                                && matches!(segments.get(&o.segment_id), Some(Row::Segment(s)) if !crate::governance::quote_in_text(&e.quote, &s.text))
                        })
                        .ok_or_else(|| ServerError::Invalid("no other segment to point at".into()))?;
                    e.segment_id = other.segment_id.clone();
                    e.header.version += 1;
                    mutations.push(Mutation::Put(Row::Evidence(e)));
                }
                PlantedKind::MovedAnchor => {
                    let mut e = first.ok_or_else(need_evidence)?;
                    let anchor = e.anchor.as_mut().ok_or_else(need_evidence)?;
                    anchor.start += 1;
                    anchor.end += 1;
                    e.header.version += 1;
                    mutations.push(Mutation::Put(Row::Evidence(e)));
                }
                PlantedKind::BrokenAnchor => {
                    let mut e = first.ok_or_else(need_evidence)?;
                    e.anchor_broken = true;
                    e.header.version += 1;
                    mutations.push(Mutation::Put(Row::Evidence(e)));
                }
                PlantedKind::MissingSegment => {
                    let mut e = first.ok_or_else(need_evidence)?;
                    e.segment_id = ObjectId(format!("planted-missing-segment-{n}"));
                    e.header.version += 1;
                    mutations.push(Mutation::Put(Row::Evidence(e)));
                }
                PlantedKind::MissingSource => {
                    let mut s = segment.ok_or_else(need_evidence)?;
                    s.source_id = ObjectId(format!("planted-missing-source-{n}"));
                    s.header.version += 1;
                    mutations.push(Mutation::Put(Row::Segment(s)));
                }
                PlantedKind::MissingBlob => {
                    let mut s = segment.ok_or_else(need_evidence)?;
                    let Some(Row::Source(original)) = sources.get(&s.source_id) else {
                        return Err(need_evidence());
                    };
                    let mut source: SourceRow = original.clone();
                    source.header.id = ObjectId(format!("planted-source-{n}"));
                    source.logical_id = source.header.id.clone();
                    source.sha256 = format!("{:064x}", n + 1);
                    s.source_id = source.header.id.clone();
                    s.header.version += 1;
                    mutations.push(Mutation::Put(Row::Source(source)));
                    mutations.push(Mutation::Put(Row::Segment(s)));
                }
                PlantedKind::StaleTtl => {
                    changed.predicate = "planted-stale".into();
                    rehash(&mut changed);
                    state.schedule.ttl_days.insert("planted-stale".into(), 1);
                    if let Some(Row::ChangeSet(cs)) = changesets.get(&claim.changeset_id) {
                        let mut cs = cs.clone();
                        cs.decided_at_ms = Some(now - 2 * 86_400_000);
                        mutations.push(Mutation::Put(Row::ChangeSet(cs)));
                    }
                    mutations.push(Mutation::Put(Row::Assertion(changed)));
                }
                PlantedKind::ExpiredValidity => {
                    changed.valid_to_ms = Some(now - 1);
                    mutations.push(Mutation::Put(Row::Assertion(changed)));
                }
                PlantedKind::IndexMissing => gone.push(claim.header.id.clone()),
                PlantedKind::WrongValue => {
                    changed.value = json!(format!("planted value {n}"));
                    rehash(&mut changed);
                    mutations.push(Mutation::Put(Row::Assertion(changed)));
                }
                PlantedKind::NegatedValue => {
                    let mut text = String::new();
                    crate::lexical::value_text(&claim.value, &mut text);
                    changed.value = json!(format!("inte {text}"));
                    rehash(&mut changed);
                    mutations.push(Mutation::Put(Row::Assertion(changed)));
                }
                PlantedKind::WrongSubject => {
                    changed.subject = format!("planted subject {n}");
                    rehash(&mut changed);
                    mutations.push(Mutation::Put(Row::Assertion(changed)));
                }
                PlantedKind::UnsupportedClaim => {
                    changed.predicate = "planted predicate".into();
                    changed.value = json!(format!("nothing the quote says {n}"));
                    rehash(&mut changed);
                    mutations.push(Mutation::Put(Row::Assertion(changed)));
                }
                PlantedKind::Clean => {}
            }
        }
        state.header.version += 1;
        mutations.push(Mutation::Put(Row::AuditState(state)));
        self.state_event(&lease, &caller.principal.id, mutations)?;
        if !gone.is_empty() {
            let IndexHealth::Ready { watermark } = health else {
                return Err(ServerError::Conflict("the keyword index isn't ready".into()));
            };
            let _projector = self.projector.lock().expect("lock");
            let ops: Vec<IndexOp> = gone.into_iter().map(IndexOp::Delete).collect();
            self.lexical.write(vault, Generation::Live, &ops)?;
            self.lexical.commit(vault, Generation::Live, watermark)?;
        }
        Ok(planted)
    }

    fn keep_job(&self, vault: &VaultId, job: &JobRow, for_ms: i64, rows: Vec<Mutation>) -> Result<(), ExtractJobError> {
        if self.hold_job(vault, job, for_ms, false, rows)? {
            Ok(())
        } else {
            Err(ExtractJobError::Lost)
        }
    }

    /// One job's work. Everything it has done commits as it goes, so a crash costs no second call: the next attempt carries on with the same sample.
    pub fn run_audit_job<A: AgentRunner>(&self, vault: &VaultId, job: &JobRow, agent: Option<&A>) -> Result<Vec<Mutation>, ExtractJobError> {
        let Some(run_id) = job.input.get("run_id").and_then(Value::as_str).map(|s| ObjectId(s.into())) else {
            return Err(ExtractJobError::Fatal("the job names no run".into()));
        };
        let mut run = self.audit_run(vault, &run_id).map_err(ExtractJobError::from)?;
        if run.state == AuditRunState::Finished {
            return Ok(vec![]);
        }
        self.keep_job(vault, job, SCAN_HOLD_MS, vec![])?;
        if run.started_at_ms.is_none() {
            self.take_sample(vault, job, &mut run)?;
        }
        let checks = self.load_checks(vault, &run)?;
        let wanted: Vec<ObjectId> = checks
            .iter()
            .filter(|c| c.outcome == AuditOutcome::Sampled)
            .map(|c| c.claim_id.clone())
            .collect();
        let index = self.reconcile_indexes(vault, &mut run, &wanted)?;
        let limits = *self.auditor.lock().expect("lock");
        let agent = agent.zip(limits);
        if let Some((a, _)) = agent {
            run.model = Some(a.model());
        }
        let mut waiting = vec![];
        let finals = self.deterministic_phase(vault, job, &mut run, checks, &index)?;
        for check in finals {
            if agent.is_some() && audit::llm_can_check(&check.checks) && check.outcome == AuditOutcome::Sampled {
                waiting.push(check);
            } else if check.outcome == AuditOutcome::Sampled {
                run.llm_skipped += 1;
                self.finish_claim(vault, job, &mut run, check)?;
            }
        }
        if let Some((a, limits)) = agent {
            self.llm_phase(vault, job, &mut run, waiting, a, limits)?;
        }
        self.requeue_siblings(vault, job, &mut run)?;
        run.state = AuditRunState::Finished;
        run.finished_at_ms = Some(self.now());
        run.header.version += 1;
        Ok(vec![Mutation::Put(Row::AuditRun(run))])
    }

    fn load_checks(&self, vault: &VaultId, run: &AuditRunRow) -> Result<Vec<AuditCheckRow>, ExtractJobError> {
        let snap = self.engine.store().snapshot(vault)?;
        let (mut out, mut after): (Vec<AuditCheckRow>, Option<ObjectId>) = (vec![], None);
        loop {
            let page = snap.audit_checks(&run.header.id, false, after.as_ref(), PAGE_LIMIT)?;
            let Some(last) = page.last() else { return Ok(out) };
            after = Some(last.header.id.clone());
            out.extend(page);
        }
    }

    /// Reads every claim that stands on one snapshot, and fixes the sample before any check runs.
    fn take_sample(&self, vault: &VaultId, job: &JobRow, run: &mut AuditRunRow) -> Result<(), ExtractJobError> {
        let snap = self.engine.store().snapshot(vault)?;
        let state_seq = snap.head()?;
        let (mut candidates, mut combos): (Vec<Candidate>, HashMap<ObjectId, Option<AuditCombination>>) = (vec![], HashMap::new());
        let mut after = None;
        loop {
            let page = snap.rows(Table::Assertion, after.as_ref())?;
            let Some(last) = page.last() else { break };
            after = Some(last.header().id.clone());
            let claims: Vec<AssertionRow> = page
                .into_iter()
                .filter_map(|r| match r {
                    Row::Assertion(a) if audit::is_candidate(&a) => Some(a),
                    _ => None,
                })
                .collect();
            for (candidate, combination) in candidates_in(&snap, &claims)? {
                combos.insert(candidate.id.clone(), combination);
                candidates.push(candidate);
            }
        }
        drop(snap);
        let picked = audit::sample(run.seed, &candidates, run.sample_size as usize);
        let by_id: HashMap<&ObjectId, &Candidate> = candidates.iter().map(|c| (&c.id, c)).collect();
        run.population = candidates.len() as u64;
        run.sampled = picked.len() as u32;
        run.state_seq = state_seq;
        run.state = AuditRunState::Running;
        run.started_at_ms = Some(self.now());
        run.header.version += 1;
        let mut rows = vec![Mutation::Put(Row::AuditRun(run.clone()))];
        for (rank, p) in picked.iter().enumerate() {
            rows.push(Mutation::Put(Row::AuditCheck(AuditCheckRow {
                header: header(ObjectId(format!("{}-{rank:05}", run.header.id)), AUDITOR),
                run_id: run.header.id.clone(),
                rank: rank as u32,
                claim_id: p.id.clone(),
                weight_micro: p.weight_micro,
                combination: combos.get(&p.id).cloned().flatten(),
                auto_committed: by_id.get(&p.id).is_some_and(|c| c.auto_committed),
                outcome: AuditOutcome::Sampled,
                checks: vec![],
                changeset_id: None,
                counted_correct: false,
                finished_at_ms: None,
            })));
        }
        self.keep_job(vault, job, SCAN_HOLD_MS, rows)
    }

    /// Counts and hashes of the store against each index, and which sampled claims the keyword index holds. Nothing is concluded from an index that hasn't reached what would matter: events that touched nothing it holds don't count against it, so an index a little behind the log can still be compared.
    fn reconcile_indexes(&self, vault: &VaultId, run: &mut AuditRunRow, wanted: &[ObjectId]) -> Result<IndexView, ExtractJobError> {
        let mut findings = vec![];
        let mut view = IndexView {
            present: None,
            touched: BTreeSet::new(),
        };
        let inconclusive = |what: &str, why: String| VaultFinding {
            check: Kind::IndexReconciliation,
            status: St::Inconclusive,
            detail: format!("{what}: {why}"),
        };
        let store_failure = |e: memex_types::LexicalError| ExtractJobError::Retry(e.to_string());
        match self.lexical.health(vault, false).map_err(store_failure)? {
            IndexHealth::Ready { watermark } => {
                let claims = self.lexical.reconcile(vault, DocKind::Claim, wanted).map_err(store_failure)?;
                let segments = self.lexical.reconcile(vault, DocKind::Segment, &[]).map_err(store_failure)?;
                let again = self.lexical.health(vault, false).map_err(store_failure)?;
                if again != (IndexHealth::Ready { watermark }) {
                    findings.push(inconclusive("keyword index", "it moved during the comparison".into()));
                } else if let (Some(claims), Some(segments)) = (claims, segments) {
                    let snap = self.engine.store().snapshot(vault)?;
                    match Self::touched_since(&snap, watermark)? {
                        None => findings.push(inconclusive("keyword index", "it's too far behind the log".into())),
                        Some(touched) => {
                            view.touched = touched.assertions.clone();
                            view.present = Some(claims.present.clone());
                            let settled = touched.renditions.is_empty();
                            findings.push(if touched.assertions.is_empty() && settled {
                                Self::compare("keyword index", "claims", snap.id_digest(DocKind::Claim)?, claims.digest)
                            } else {
                                inconclusive("keyword index claims", "claims have changed since its last commit".into())
                            });
                            findings.push(if touched.segments.is_empty() && settled {
                                Self::compare("keyword index", "segments", snap.id_digest(DocKind::Segment)?, segments.digest)
                            } else {
                                inconclusive("keyword index segments", "segments have changed since its last commit".into())
                            });
                        }
                    }
                } else {
                    findings.push(inconclusive("keyword index", "this backend can't list what it holds".into()));
                }
            }
            other => findings.push(inconclusive("keyword index", format!("{other:?}"))),
        }
        let vector_failure = |e: memex_types::VectorError| ExtractJobError::Retry(e.to_string());
        match self.vector.health(vault).map_err(vector_failure)? {
            VectorHealth::Ready { watermark, .. } => {
                let snap = self.engine.store().snapshot(vault)?;
                let embedding = !snap.jobs(Some(EMBED_JOB), Some(JobState::Queued), None, 1)?.is_empty()
                    || !snap.jobs(Some(EMBED_JOB), Some(JobState::Running), None, 1)?.is_empty();
                let touched = Self::touched_since(&snap, watermark)?;
                match touched {
                    _ if embedding => findings.push(inconclusive("vector index", "embedding jobs are still open".into())),
                    None => findings.push(inconclusive("vector index", "it's too far behind the log".into())),
                    Some(t) if !t.assertions.is_empty() || !t.segments.is_empty() || !t.renditions.is_empty() => {
                        findings.push(inconclusive("vector index", "what it holds has changed since its last commit".into()));
                    }
                    Some(_) => {
                        for (kind, name) in [(DocKind::Claim, "claims"), (DocKind::Segment, "segments")] {
                            match self.vector.reconcile(vault, kind, &[]).map_err(vector_failure)? {
                                Some(held) => findings.push(Self::compare("vector index", name, snap.id_digest(kind)?, held.digest)),
                                None => findings.push(inconclusive("vector index", "this backend can't list what it holds".into())),
                            }
                        }
                    }
                }
            }
            VectorHealth::Missing => findings.push(inconclusive("vector index", "there's no generation yet".into())),
        }
        let snap = self.engine.store().snapshot(vault)?;
        let broken = snap.unresolved_links(None, LINKS_SHOWN)?;
        findings.push(VaultFinding {
            check: Kind::LinkRot,
            status: if broken.is_empty() { St::Pass } else { St::Fail },
            detail: if broken.is_empty() {
                "every page link resolves".to_string()
            } else {
                format!(
                    "{}{} page links lead nowhere, such as {}",
                    broken.len(),
                    if broken.len() == LINKS_SHOWN { " or more" } else { "" },
                    broken[0].to
                )
            },
        });
        drop(snap);
        run.vault_findings = findings;
        run.header.version += 1;
        Ok(view)
    }

    fn compare(index: &str, what: &str, store: memex_types::IdDigest, held: memex_types::IdDigest) -> VaultFinding {
        VaultFinding {
            check: Kind::IndexReconciliation,
            status: if store == held { St::Pass } else { St::Fail },
            detail: if store == held {
                format!("{index} {what}: {} in both, and the hashes match", store.count)
            } else {
                format!(
                    "{index} {what}: the store has {} and the index {}, with hashes {:x} and {:x}",
                    store.count, held.count, store.xor, held.xor
                )
            },
        }
    }

    /// What the events after an index's watermark touched, or None if there are too many to read.
    fn touched_since<S: Snapshot>(snap: &S, watermark: Option<u64>) -> Result<Option<Touched>, crate::StoreError> {
        let head = snap.head()?;
        let behind = head.map_or(0, |h| h + 1).saturating_sub(watermark.map_or(0, |w| w + 1));
        if behind > MAX_LAG_EVENTS {
            return Ok(None);
        }
        let mut touched = Touched::default();
        let mut after = watermark;
        loop {
            let page = snap.events(after)?;
            let Some(last) = page.last() else { return Ok(Some(touched)) };
            after = Some(last.seq);
            for e in &page {
                if touched.add_event(e).is_err() {
                    return Ok(None);
                }
            }
        }
    }

    /// The checks that cost no tokens, for everything sampled. One store call per kind of row, for the whole sample, and they all commit in one event.
    fn deterministic_phase(
        &self,
        vault: &VaultId,
        job: &JobRow,
        run: &mut AuditRunRow,
        checks: Vec<AuditCheckRow>,
        index: &IndexView,
    ) -> Result<Vec<AuditCheckRow>, ExtractJobError> {
        let snap = self.engine.store().snapshot(vault)?;
        let ttl = state_of(&snap)?.schedule.ttl_days;
        let now = self.now();
        let open: Vec<ObjectId> = checks
            .iter()
            .filter(|c| c.outcome == AuditOutcome::Sampled && c.checks.is_empty())
            .map(|c| c.claim_id.clone())
            .collect();
        let claims = rows_by_id(&snap, Table::Assertion, open.iter().cloned())?;
        let changesets = rows_by_id(
            &snap,
            Table::ChangeSet,
            claims
                .values()
                .filter_map(|r| if let Row::Assertion(a) = r { Some(a.changeset_id.clone()) } else { None }),
        )?;
        let evidence = if open.is_empty() { vec![] } else { snap.evidence_of(&open)? };
        let segments = rows_by_id(&snap, Table::Segment, evidence.iter().map(|e| e.segment_id.clone()))?;
        let as_segment = |r: &Row| if let Row::Segment(s) = r { Some(s.clone()) } else { None };
        let renditions = rows_by_id(&snap, Table::Rendition, segments.values().filter_map(as_segment).filter_map(|s| s.rendition_id))?;
        let sources = rows_by_id(&snap, Table::Source, segments.values().filter_map(as_segment).map(|s| s.source_id))?;
        drop(snap);
        let mut blobs: HashMap<ObjectId, bool> = HashMap::new();
        for (id, row) in &sources {
            if let Row::Source(s) = row {
                let held = match BlobHash::parse(&s.sha256) {
                    Ok(hash) => self.blobs.read(vault, &hash, 0, Some(1)).is_ok(),
                    Err(_) => false,
                };
                blobs.insert(id.clone(), held);
            }
        }
        let mut rows = vec![];
        let mut out = vec![];
        for mut check in checks {
            if check.outcome != AuditOutcome::Sampled || !check.checks.is_empty() {
                out.push(check);
                continue;
            }
            let Some(Row::Assertion(claim)) = claims.get(&check.claim_id) else {
                out.push(check);
                continue;
            };
            let facts: Vec<EvidenceFacts> = evidence
                .iter()
                .filter(|e| e.assertion_id == check.claim_id)
                .map(|e| {
                    let segment = segments.get(&e.segment_id).and_then(as_segment);
                    let source = segment.as_ref().and_then(|s| match sources.get(&s.source_id) {
                        Some(Row::Source(x)) => Some(x.clone()),
                        _ => None,
                    });
                    EvidenceFacts {
                        evidence: e.clone(),
                        rendition: segment.as_ref().and_then(|s| s.rendition_id.as_ref()).and_then(|r| match renditions.get(r) {
                            Some(Row::Rendition(x)) => Some(x.clone()),
                            _ => None,
                        }),
                        blob_present: source.as_ref().is_some_and(|s| blobs.get(&s.header.id).copied().unwrap_or(false)),
                        source,
                        segment,
                    }
                })
                .collect();
            let committed_at = match changesets.get(&claim.changeset_id) {
                Some(Row::ChangeSet(cs)) => cs.decided_at_ms.unwrap_or(0),
                _ => 0,
            };
            check.checks = audit::evidence_checks(claim, &facts);
            check.checks.push(audit::staleness_check(claim, committed_at, &ttl, now));
            check.checks.push(match &index.present {
                None => AuditCheckResult {
                    check: Kind::IndexReconciliation,
                    status: St::Inconclusive,
                    cause: None,
                    detail: "the keyword index can't be compared now".into(),
                    llm: None,
                },
                Some(_) if index.touched.contains(&check.claim_id) => AuditCheckResult {
                    check: Kind::IndexReconciliation,
                    status: St::Inconclusive,
                    cause: None,
                    detail: "this claim has changed since the index's last commit".into(),
                    llm: None,
                },
                Some(present) if present.contains(&check.claim_id) => AuditCheckResult {
                    check: Kind::IndexReconciliation,
                    status: St::Pass,
                    cause: None,
                    detail: String::new(),
                    llm: None,
                },
                Some(_) => AuditCheckResult {
                    check: Kind::IndexReconciliation,
                    status: St::Fail,
                    cause: Some("missing_from_index".into()),
                    detail: "the keyword index doesn't hold this claim".into(),
                    llm: None,
                },
            });
            check.header.version += 1;
            rows.push(Mutation::Put(Row::AuditCheck(check.clone())));
            out.push(check);
        }
        if !rows.is_empty() {
            rows.push(Mutation::Put(Row::AuditRun(run.clone())));
            self.keep_job(vault, job, SCAN_HOLD_MS, rows)?;
        }
        Ok(out)
    }

    /// Makes the review item for a finding, if it has anything a person can decide. A person's claim is proposed under the audit's own name, as it has no machine's record to touch. It holds propose scope, like any agent, and the gate does the rest.
    fn file_finding(&self, vault: &VaultId, run: &AuditRunRow, check: &AuditCheckRow) -> Result<Filed, ExtractJobError> {
        if !audit::needs_review_item(&check.checks) {
            return Ok(Filed {
                changeset_id: None,
                earlier: None,
            });
        }
        let claim = match self.engine.store().snapshot(vault)?.get(&RowKey {
            table: Table::Assertion,
            id: check.claim_id.clone(),
        })? {
            Some(Row::Assertion(a)) => a,
            _ => {
                return Ok(Filed {
                    changeset_id: None,
                    earlier: None,
                });
            }
        };
        let found = audit::first_failure(&check.checks);
        let rationale = format!(
            "Audit run {}: {} failed ({}): {}. The claim says `{}` `{}` `{}`. Approve to retire this claim, or reject to keep it.",
            run.header.id,
            found.map_or("a check", |f| f.check.as_str()),
            found.and_then(|f| f.cause.as_deref()).unwrap_or("no cause given"),
            found.map_or(String::new(), |f| shown(&f.detail)),
            shown(&claim.subject),
            shown(&claim.predicate),
            shown(&claim.value.to_string()),
        );
        let outcome = self.audit_propose(
            vault,
            check.combination.as_ref(),
            Proposal {
                ops: vec![Op::RetractAssertion {
                    target: claim.header.id.clone(),
                    base_version: claim.header.version,
                    counts_against: audit::approval_counts_against(&check.checks),
                }],
                rationale,
                idempotency_key: format!("audit-finding:{}", claim.header.id),
            },
        )?;
        let Some(row) = outcome.changeset else {
            return Ok(Filed {
                changeset_id: None,
                earlier: None,
            });
        };
        // Ids go by time, so an item older than this run is an earlier finding, and one from this run is a crash being recovered from.
        let earlier = (outcome.replayed && row.header.id < run.header.id).then_some(row.state);
        Ok(Filed {
            changeset_id: Some(row.header.id),
            earlier,
        })
    }

    /// Every write the audit makes to knowledge goes through here, under its propose scope.
    fn audit_propose(&self, vault: &VaultId, combination: Option<&AuditCombination>, proposal: Proposal) -> Result<super::ProposalOutcome, ExtractJobError> {
        self.allow(&self.audit_agent_caller(vault), Operation::VaultContentPropose(vault))
            .map_err(ExtractJobError::from)?;
        let by = ProposedBy {
            actor: AUDITOR,
            machine: true,
            extractor: combination.map_or(AUDITOR, |c| c.extractor.as_str()),
            prompt_version: combination.map_or("", |c| c.prompt_version.as_str()),
            requested_by: None,
        };
        self.propose_as(&by, vault, proposal, false).map_err(ExtractJobError::from)
    }

    /// Ends one claim's audit: its review item, then in one event its result, any sample for its combination, and what a failure revokes.
    fn finish_claim(&self, vault: &VaultId, job: &JobRow, run: &mut AuditRunRow, mut check: AuditCheckRow) -> Result<(), ExtractJobError> {
        let filed = self.file_finding(vault, run, &check)?;
        check.changeset_id = filed.changeset_id;
        check.outcome = audit::outcome_of(&check.checks);
        check.finished_at_ms = Some(self.now());
        check.header.version += 1;
        // Someone already decided this, or is about to. It's reported, and it isn't counted twice.
        let wrong = audit::shows_claim_wrong(&check.checks);
        if wrong && filed.earlier == Some(ChangeSetState::Rejected) {
            check.outcome = AuditOutcome::Suspect;
        }
        let lease = self.lease(vault).map_err(ExtractJobError::from)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let current = self.job(vault, &job.header.id).map_err(ExtractJobError::from)?;
        if !super::still_held(&current, job) {
            return Err(ExtractJobError::Lost);
        }
        let snap = self.engine.store().snapshot(vault)?;
        let mut rows = vec![];
        let threshold = *self.auto_commit_threshold.lock().expect("lock");
        if let Some(c) = &check.combination {
            let reason = format!("audit run {}, claim {}", run.header.id, check.claim_id);
            if wrong && filed.earlier.is_none() {
                let was_on = match snap.get(&RowKey {
                    table: Table::AutoCommit,
                    id: review::combination_id(&c.extractor, &c.prompt_version, &c.item_type),
                })? {
                    Some(Row::AutoCommit(r)) => review::earned(r.correct, r.incorrect, threshold),
                    _ => false,
                };
                let row = self
                    .audit_sample_row(&snap, &c.extractor, &c.prompt_version, &c.item_type, (0, 1), true, AUDITOR, &reason)
                    .map_err(ExtractJobError::from)?;
                if was_on {
                    run.revoked.push(row.header.id.clone());
                }
                rows.push(Mutation::Put(Row::AutoCommit(row)));
                let mut state = state_of(&snap)?;
                if !state.sibling_cursors.iter().any(|s| &s.combination == c) {
                    state.sibling_cursors.push(SiblingCursor {
                        combination: c.clone(),
                        after: None,
                    });
                    state.header.version += 1;
                    rows.push(Mutation::Put(Row::AuditState(state)));
                }
            } else if audit::vouches_for_claim(&check.checks) && !snap.claim_counted_correct(&check.claim_id)? {
                let row = self
                    .audit_sample_row(&snap, &c.extractor, &c.prompt_version, &c.item_type, (1, 0), false, AUDITOR, &reason)
                    .map_err(ExtractJobError::from)?;
                check.counted_correct = true;
                rows.push(Mutation::Put(Row::AutoCommit(row)));
            }
        }
        drop(snap);
        match check.outcome {
            AuditOutcome::Failed => run.failed += 1,
            AuditOutcome::Suspect => run.suspect += 1,
            _ => run.passed += 1,
        }
        run.header.version += 1;
        rows.push(Mutation::Put(Row::AuditCheck(check)));
        rows.push(Mutation::Put(Row::AuditRun(run.clone())));
        let mut next = jobs::lease_extended(current, self.now() + SCAN_HOLD_MS);
        next.wait = None;
        rows.push(Mutation::Put(Row::Job(next)));
        self.state_event(&lease, AUDITOR, rows).map_err(ExtractJobError::from)
    }

    /// Records what a call spent, against the run and the day, in the same event as what it found.
    fn spend(&self, vault: &VaultId, job: &JobRow, hold: i64, usage: &AgentUsage, rows: Vec<Mutation>) -> Result<(), ExtractJobError> {
        let lease = self.lease(vault).map_err(ExtractJobError::from)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let current = self.job(vault, &job.header.id).map_err(ExtractJobError::from)?;
        if !super::still_held(&current, job) {
            return Err(ExtractJobError::Lost);
        }
        let mut state = self.audit_state(vault).map_err(ExtractJobError::from)?;
        let day = audit::day_of(self.now());
        if state.day != day {
            state.day = day;
            state.day_spent = 0;
        }
        state.day_spent += usage.total();
        state.header.version += 1;
        let mut rows = rows;
        rows.push(Mutation::Put(Row::AuditState(state)));
        rows.push(Mutation::Put(Row::Job(jobs::lease_extended(current, self.now() + hold))));
        self.state_event(&lease, AUDITOR, rows).map_err(ExtractJobError::from)
    }

    /// One schema-only call with no tools, after checking it fits both budgets. Returns None once the run has to stop.
    #[allow(clippy::too_many_arguments)]
    fn audit_call<A: AgentRunner>(
        &self,
        vault: &VaultId,
        job: &JobRow,
        run: &mut AuditRunRow,
        agent: &A,
        limits: AuditLimits,
        call: (Kind, &str, &str, String),
        accept: &dyn Fn(&str) -> Result<(), String>,
    ) -> Result<Option<CallAnswer>, ExtractJobError> {
        let (kind, instruction, schema_json, input) = call;
        let state = self.audit_state(vault).map_err(ExtractJobError::from)?;
        let day_spent = if state.day == audit::day_of(self.now()) { state.day_spent } else { 0 };
        let day_budget = state.schedule.day_token_budget;
        let estimated = extraction::estimated_tokens(instruction) + extraction::estimated_tokens(schema_json) + extraction::estimated_tokens(&input);
        let stop = |run: &mut AuditRunRow, why: AuditStop, detail: String| {
            run.stop = Some(why);
            run.stop_detail = Some(detail);
            run.header.version += 1;
        };
        let max_output_tokens = match audit::call_room(run.run_token_budget, run.usage.total(), day_budget, day_spent, estimated, limits.output_tokens) {
            Ok(room) => room,
            Err(why) => {
                stop(
                    run,
                    why,
                    format!(
                        "{} needs about {estimated} tokens of input, with {} spent of {} this run and {day_spent} of {day_budget} today",
                        kind.as_str(),
                        run.usage.total(),
                        run.run_token_budget
                    ),
                );
                return Ok(None);
            }
        };
        let hold = limits.timeout_ms as i64 * 2 + LEASE_MARGIN_MS;
        self.keep_job(vault, job, hold, vec![])?;
        let request = AgentRequest {
            instruction: instruction.into(),
            input,
            schema_json: schema_json.into(),
            budget: memex_types::AgentBudget {
                max_output_tokens,
                timeout_ms: limits.timeout_ms,
            },
        };
        let (used, budget) = (run.usage.total(), run.run_token_budget);
        let room = |spent: &AgentUsage| {
            audit::call_room(
                budget,
                used + spent.total(),
                day_budget,
                day_spent + spent.total(),
                estimated,
                limits.output_tokens,
            )
            .ok()
        };
        let outcome = run_with_retry(agent, &request, accept, &room);
        run.usage.add(&outcome.usage);
        run.llm_calls += outcome.attempts;
        run.header.version += 1;
        let llm = AuditLlmUse {
            model: agent.model(),
            prompt_hash: String::new(),
            usage: outcome.usage,
            attempts: outcome.attempts,
        };
        // The tokens are spent whatever comes next, so they're on record first.
        self.spend(vault, job, hold, &outcome.usage, vec![Mutation::Put(Row::AuditRun(run.clone()))])?;
        match outcome.result {
            Ok(reply) => Ok(Some((Ok(reply.json), llm))),
            Err(e) if e.is_outage() => {
                stop(run, AuditStop::LlmUnavailable, e.to_string());
                Ok(None)
            }
            Err(e @ AgentError::Provider(_)) => {
                stop(run, AuditStop::LlmRefused, e.to_string());
                Ok(None)
            }
            Err(e) if outcome.out_of_budget => {
                stop(run, AuditStop::RunBudget, format!("a retry didn't fit: {e}"));
                Ok(None)
            }
            // It answered, and nothing it said can be used. That's this check's result, and the next claim may go better.
            Err(e) => Ok(Some((Err(e.to_string()), llm))),
        }
    }

    /// Entailment and blind re-extraction, claim by claim. A budget or an outage ends this phase and not the run: what's left keeps its deterministic results.
    fn llm_phase<A: AgentRunner>(
        &self,
        vault: &VaultId,
        job: &JobRow,
        run: &mut AuditRunRow,
        waiting: Vec<AuditCheckRow>,
        agent: &A,
        limits: AuditLimits,
    ) -> Result<(), ExtractJobError> {
        let entail = audit::entailment_prompt();
        let accept_entail = |text: &str| audit::parse_entailment(text).map(|_| ());
        let snap = self.engine.store().snapshot(vault)?;
        let types = super::extraction::all_types(&snap)?;
        drop(snap);
        let extract = extraction::build_prompt(&types);
        let validator = extraction::reply_validator(&extract.schema).map_err(|e| ExtractJobError::Fatal(format!("the reply schema doesn't compile: {e}")))?;
        let accept_extract = |text: &str| -> Result<(), String> {
            let doc: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
            match validator.iter_errors(&doc).next() {
                Some(first) => Err(first.to_string()),
                None => Ok(()),
            }
        };
        for mut check in waiting {
            if run.stop.is_some() {
                run.llm_skipped += 1;
                self.finish_claim(vault, job, run, check)?;
                continue;
            }
            let (claim, segment, quote, entity_type) = {
                let snap = self.engine.store().snapshot(vault)?;
                let Some(Row::Assertion(claim)) = snap.get(&RowKey {
                    table: Table::Assertion,
                    id: check.claim_id.clone(),
                })?
                else {
                    self.finish_claim(vault, job, run, check)?;
                    continue;
                };
                let evidence = snap.evidence_of(std::slice::from_ref(&check.claim_id))?;
                let first = evidence.into_iter().next();
                let segment: Option<SegmentRow> = match &first {
                    Some(e) => match snap.get(&RowKey {
                        table: Table::Segment,
                        id: e.segment_id.clone(),
                    })? {
                        Some(Row::Segment(s)) => Some(s),
                        _ => None,
                    },
                    None => None,
                };
                let entity_type = match entity_of_subject(&claim.subject) {
                    Some(id) => match snap.get(&RowKey { table: Table::Entity, id })? {
                        Some(Row::Entity(e)) => Some(e.type_name),
                        _ => None,
                    },
                    None => None,
                };
                (claim, segment, first.map(|e| e.quote), entity_type)
            };
            let (Some(segment), Some(quote)) = (segment, quote) else {
                run.llm_skipped += 1;
                self.finish_claim(vault, job, run, check)?;
                continue;
            };
            if !check.checks.iter().any(|c| c.check == Kind::Entailment)
                && let Some(input) = audit::entailment_input(&claim, &quote, entity_type.as_deref())
            {
                let call = (Kind::Entailment, entail.instruction.as_str(), entail.schema_json.as_str(), input);
                match self.audit_call(vault, job, run, agent, limits, call, &accept_entail)? {
                    None => {
                        run.llm_skipped += 1;
                        self.finish_claim(vault, job, run, check)?;
                        continue;
                    }
                    Some((reply, mut llm)) => {
                        llm.prompt_hash.clone_from(&entail.hash);
                        let (status, detail) = match reply {
                            Ok(text) => audit::parse_entailment(&text).map_err(ExtractJobError::Retry)?,
                            Err(why) => (St::Inconclusive, format!("no usable answer: {why}")),
                        };
                        check.checks.push(AuditCheckResult {
                            check: Kind::Entailment,
                            status,
                            cause: (status == St::Fail).then(|| audit::NOT_ENTAILED.to_string()),
                            detail,
                            llm: Some(llm),
                        });
                        check.header.version += 1;
                        self.keep_job(vault, job, SCAN_HOLD_MS, vec![Mutation::Put(Row::AuditCheck(check.clone()))])?;
                    }
                }
            }
            if !audit::shows_claim_wrong(&check.checks) && !check.checks.iter().any(|c| c.check == Kind::BlindReextraction) {
                // It sees the segment and nothing about the claim, so it can't simply agree.
                let batch = [segment];
                let call = (
                    Kind::BlindReextraction,
                    extract.instruction.as_str(),
                    extract.schema_json.as_str(),
                    extraction::batch_input(&batch),
                );
                match self.audit_call(vault, job, run, agent, limits, call, &accept_extract)? {
                    None => run.llm_skipped += 1,
                    Some((reply, mut llm)) => {
                        llm.prompt_hash.clone_from(&extract.hash);
                        let (status, detail) = match reply {
                            Ok(text) => {
                                let doc: Value = serde_json::from_str(&text).map_err(|e| ExtractJobError::Retry(e.to_string()))?;
                                let (items, _refused): (Vec<ExtractedItem>, _) = extraction::checked_items(&doc, &batch);
                                match audit::agrees_with_reextraction(&claim, &items) {
                                    Ok(()) => (St::Pass, format!("a fresh extraction found the same, among {} items", items.len())),
                                    Err(why) => (St::Fail, why),
                                }
                            }
                            Err(why) => (St::Inconclusive, format!("no usable answer: {why}")),
                        };
                        check.checks.push(AuditCheckResult {
                            check: Kind::BlindReextraction,
                            status,
                            cause: (status == St::Fail).then(|| "disagrees".to_string()),
                            detail,
                            llm: Some(llm),
                        });
                        check.header.version += 1;
                    }
                }
            }
            self.finish_claim(vault, job, run, check)?;
        }
        Ok(())
    }

    /// Sends a revoked combination's auto-committed claims back to people, up to a cap per run. The cursors keep their place, so later runs carry on until each combination is covered, and each run reports what's left. A claim is queued once, ever, whichever way a person then decides.
    fn requeue_siblings(&self, vault: &VaultId, job: &JobRow, run: &mut AuditRunRow) -> Result<(), ExtractJobError> {
        let mut state = self.audit_state(vault).map_err(ExtractJobError::from)?;
        if state.sibling_cursors.is_empty() {
            run.siblings_remaining = 0;
            return Ok(());
        }
        let snap = self.engine.store().snapshot(vault)?;
        let mut budget = audit::SIBLINGS_PER_RUN;
        let mut remaining = 0u64;
        let mut still = vec![];
        for mut cursor in std::mem::take(&mut state.sibling_cursors) {
            let filter = ReviewFilter {
                state: Some(ChangeSetState::Committed),
                extractor: Some(cursor.combination.extractor.clone()),
                prompt_version: Some(cursor.combination.prompt_version.clone()),
                item_type: Some(cursor.combination.item_type.clone()),
                ..ReviewFilter::default()
            };
            let (mut after, mut unfinished) = (cursor.after.clone(), false);
            loop {
                let page = snap.review_queue(&filter, after.as_ref(), PAGE_LIMIT)?;
                let Some(last) = page.last().map(|c| c.header.id.clone()) else { break };
                for cs in page
                    .iter()
                    .filter(|c| c.decided_by.as_deref() == Some(review::AUTO_COMMIT) && c.reverted_by.is_none())
                {
                    let ids: Vec<ObjectId> = cs
                        .ops
                        .iter()
                        .filter_map(|op| match op {
                            Op::AddAssertion { id, .. } => Some(id.clone()),
                            Op::SupersedeAssertion { replacement, .. } => match replacement.as_ref() {
                                Op::AddAssertion { id, .. } => Some(id.clone()),
                                _ => None,
                            },
                            _ => None,
                        })
                        .collect();
                    let claims = rows_by_id(&snap, Table::Assertion, ids.clone())?;
                    let standing: Vec<AssertionRow> = ids
                        .iter()
                        .filter_map(|id| match claims.get(id) {
                            Some(Row::Assertion(a)) if audit::is_candidate(a) => Some(a.clone()),
                            _ => None,
                        })
                        .collect();
                    if unfinished {
                        remaining += standing.len() as u64;
                        continue;
                    }
                    for claim in &standing {
                        if budget == 0 {
                            unfinished = true;
                            remaining += 1;
                            continue;
                        }
                        // A finding already holds it in front of a person.
                        if snap.changeset_by_key(&format!("audit-finding:{}", claim.header.id))?.is_some() {
                            continue;
                        }
                        let outcome = self.audit_propose(
                            vault,
                            Some(&cursor.combination),
                            Proposal {
                                ops: vec![Op::RetractAssertion {
                                    target: claim.header.id.clone(),
                                    base_version: claim.header.version,
                                    counts_against: true,
                                }],
                                rationale: format!(
                                    "An audit failure revoked auto-commit for {} under this prompt, so a person should look at what it committed with nobody looking. The claim says `{}` `{}` `{}`. Approve to retire this claim, or reject to keep it.",
                                    cursor.combination.extractor,
                                    shown(&claim.subject),
                                    shown(&claim.predicate),
                                    shown(&claim.value.to_string()),
                                ),
                                idempotency_key: format!("audit-sibling:{}", claim.header.id),
                            },
                        )?;
                        if !outcome.replayed {
                            budget -= 1;
                            run.siblings_requeued += 1;
                        }
                    }
                    if !unfinished {
                        cursor.after = Some(cs.header.id.clone());
                    }
                }
                if !unfinished {
                    cursor.after = Some(last.clone());
                }
                after = Some(last);
            }
            if unfinished {
                still.push(cursor);
            }
        }
        drop(snap);
        state.sibling_cursors = still;
        run.siblings_remaining = remaining;
        run.header.version += 1;
        // The state is read again under the lock inside `spend`, which would lose this, so it goes through its own step with the schedule as it stands now.
        self.put_cursors(vault, job, state.sibling_cursors, run)
    }

    fn put_cursors(&self, vault: &VaultId, job: &JobRow, cursors: Vec<SiblingCursor>, run: &AuditRunRow) -> Result<(), ExtractJobError> {
        let lease = self.lease(vault).map_err(ExtractJobError::from)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let current = self.job(vault, &job.header.id).map_err(ExtractJobError::from)?;
        if !super::still_held(&current, job) {
            return Err(ExtractJobError::Lost);
        }
        let mut state = self.audit_state(vault).map_err(ExtractJobError::from)?;
        state.sibling_cursors = cursors;
        state.header.version += 1;
        self.state_event(
            &lease,
            AUDITOR,
            vec![
                Mutation::Put(Row::AuditState(state)),
                Mutation::Put(Row::AuditRun(run.clone())),
                Mutation::Put(Row::Job(jobs::lease_extended(current, self.now() + SCAN_HOLD_MS))),
            ],
        )
        .map_err(ExtractJobError::from)
    }
}

#[cfg(test)]
mod chat_proposal_tests {
    use super::combination_of;
    use memex_types::{AssertionRow, ChangeSetRow, ChangeSetState, Epistemic, Header, ObjectId, ProposalOrigin, RiskDecision, Status};
    use serde_json::json;

    fn header() -> Header {
        Header {
            id: ObjectId("x".into()),
            creator: "c".into(),
            status: Status::Active,
            version: 1,
            acl_id: None,
        }
    }

    #[test]
    fn a_claim_from_a_chat_has_no_combination_so_an_audit_can_neither_earn_nor_revoke_by_it() {
        let claim = AssertionRow {
            header: header(),
            subject: "s".into(),
            predicate: "p".into(),
            value: json!("v"),
            epistemic: Epistemic::Asserted,
            machine: true,
            confidence_permille: 0,
            valid_from_ms: None,
            valid_to_ms: None,
            canonical_hash: "h".into(),
            changeset_id: ObjectId("x".into()),
            superseded_by: None,
        };
        let mut cs = ChangeSetRow {
            header: header(),
            state: ChangeSetState::Committed,
            proposer: "chat-agent".into(),
            proposer_is_machine: true,
            rationale: "r".into(),
            idempotency_key: "k".into(),
            ops: vec![],
            validators: vec![],
            risk: RiskDecision::Review,
            proposed_at_ms: 0,
            decided_by: None,
            decided_at_ms: None,
            decision_reason: None,
            commit_seq: None,
            applied: vec![],
            inverse: vec![],
            reverts: None,
            reverted_by: None,
            origin: Some(ProposalOrigin {
                extractor: "chat-agent".into(),
                prompt_version: "h".into(),
                item_types: vec!["claim".into()],
                source_ids: vec![],
                rendition_ids: vec![],
            }),
            original_ops: None,
            requested_by: None,
        };
        assert!(combination_of(&claim, Some(&cs), "claim").is_some(), "the control: an agent's own claim has one");
        cs.requested_by = Some("a-person".into());
        assert_eq!(combination_of(&claim, Some(&cs), "claim"), None);
    }
}
