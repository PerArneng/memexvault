//! The audit agent on real Postgres and tantivy, with a scripted agent, so no LLM is called. Run with `MEMEX_TEST_DATABASE_URL` set, as in AGENTS.md. What a scripted agent answers says nothing about a model.
// These tests read the environment and print what they measured.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_core::testing::{FakeEmbedder, FakeRasterizer, FakeReranker, ManualClock, MemBlobStore, MemRegistry, ScriptedAgent};
use memex_core::{
    AUDIT_JOB, AgentRunner, AuditLimits, EMBED_JOB, EXTRACT_JOB, ExtractionLimits, RENDITION_JOB, RenditionInput, SearchEdges, Server, ServerEdges, Snapshot,
    StepOutcome, Store, TriggerAudit, UploadLimits, UploadRequest,
};
use memex_lexical::TantivyLexical;
use memex_store_postgres::PgStore;
use memex_types::{
    AgentError, AgentReply, AgentRequest, AgentUsage, AuditCheckKind as Kind, AuditCheckStatus as St, AuditRunRow, AuditRunState, AuditTrigger, ChangeSetState,
    DocKind, IdDigest, ItemKind, ObjectId, Op, PlantedKind, ReviewFilter, Row, SearchMode, SearchRequest, Status, Table, VaultId,
};
use memex_vector::PgVectors;
use memexvault::edges::{PgVectorIndex, SystemSeed, TantivyIndex};
use memexvault::pg_edge::PostgresStore;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

struct Real;

impl ServerEdges for Real {
    type Caches = memex_core::NoCaches;
    type Registry = MemRegistry;
    type Store = PostgresStore;
    type Blobs = MemBlobStore<ManualClock>;
    type Clock = ManualClock;
    type Seed = SystemSeed;
    type Rasterizer = FakeRasterizer;
    type Lexical = TantivyIndex;
    type Vector = PgVectorIndex;
    type Embedder = FakeEmbedder;
    type Reranker = FakeReranker;
}

struct Rig {
    s: Arc<Server<Real>>,
    admin: memex_core::Caller,
    v: VaultId,
    dir: PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = self.s.delete_vault(&self.admin, &self.v);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn rig(name: &str) -> Rig {
    let url = std::env::var("MEMEX_TEST_DATABASE_URL").expect("not run: MEMEX_TEST_DATABASE_URL isn't set");
    let pg = PgStore::connect(&url, 8).expect("connect");
    pg.migrate().expect("migrate");
    let pg = Arc::new(pg);
    let dir = std::env::temp_dir().join(format!("memex-audit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let s = Arc::new(Server::<Real>::new(
        MemRegistry::new(),
        PostgresStore(pg.clone()),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(1_800_000_000_000),
        SystemSeed,
        SearchEdges {
            lexical: TantivyIndex(TantivyLexical::new(&dir)),
            vector: PgVectorIndex(PgVectors::new(pg)),
            embedder: FakeEmbedder::default(),
            reranker: FakeReranker::default(),
        },
    ));
    s.set_extraction(Some(ExtractionLimits {
        document_budget: 100_000_000,
        ..ExtractionLimits::default()
    }));
    s.set_auditor(Some(AuditLimits::default()));
    let admin = s.authenticate(&s.bootstrap("root").unwrap().token).unwrap();
    let v = s.create_vault(&admin, name).unwrap().id;
    Rig { s, admin, v, dir }
}

fn reply(doc: Value) -> Result<AgentReply, AgentError> {
    Ok(AgentReply {
        json: doc.to_string(),
        usage: AgentUsage {
            input_tokens: 70,
            output_tokens: 9,
            thinking_tokens: 1,
        },
        tool_calls: 0,
    })
}

fn sentence(n: usize) -> String {
    format!("Fartyget nummer {n} byggdes i Stockholm.")
}

fn long_text(first: &str) -> String {
    format!(
        "{first} {}",
        "Detta är en lång mening om ingenting särskilt som bara fyller ut stycket med ord. ".repeat(12)
    )
}

fn prompt_hash() -> String {
    memex_core::extraction::build_prompt(memex_core::entity::seed_types()).hash
}

/// As in the core tests: written from the fixtures' shape, so it shows the mechanism and nothing else.
fn scripted_oracle(req: &AgentRequest) -> Result<AgentReply, AgentError> {
    let input: Value = serde_json::from_str(&req.input).unwrap();
    if req.schema_json.contains("not_entailed") {
        let claim = &input["claims"][0];
        let holds = claim["quotes"][0]
            .as_str()
            .unwrap_or_default()
            .contains(claim["value"].as_str().unwrap_or_default());
        return reply(json!({"verdict": if holds { "entailed" } else { "not_entailed" }, "reason": "scripted"}));
    }
    let text = input["segments"][0]["text"].as_str().unwrap_or_default();
    let first = text.split('.').next().unwrap_or_default();
    let (subject, _) = first.split_once(" byggdes").unwrap_or((first, ""));
    reply(
        json!({"claims": [{"segment_id": "s1", "quote": format!("{first}."), "subject": subject, "predicate": "byggdes i", "value": "Stockholm"}], "entities": {}}),
    )
}

impl Rig {
    fn machine_claim(&self, n: usize) {
        let s = sentence(n);
        let text = long_text(&s);
        let req = UploadRequest {
            name: format!("d{n}.txt"),
            trust_tier: "trusted".into(),
            metadata: json!({}),
            replaces: None,
        };
        let source = self
            .s
            .upload(&self.admin, &self.v, req, UploadLimits::default(), &mut text.as_bytes())
            .unwrap()
            .source
            .header
            .id;
        let job = self.s.claim_job(&self.v, RENDITION_JOB, "parser").unwrap().unwrap();
        let input = RenditionInput::Plain {
            text,
            parser: "plain-1".into(),
        };
        let rows = self.s.build_rendition(&self.v, &source, input).unwrap();
        assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
        let agent = ScriptedAgent::new(vec![reply(json!({
            "claims": [{"segment_id": "s1", "quote": s, "subject": format!("Fartyget nummer {n}"), "predicate": "byggdes i", "value": "Stockholm"}],
            "entities": {},
        }))]);
        let job = self.s.claim_job(&self.v, EXTRACT_JOB, "w").unwrap().expect("an extraction job");
        let rows = self.s.run_extraction_job(&self.v, &job, &agent).unwrap();
        assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
    }

    fn earn(&self) {
        self.s
            .record_audit_samples(&self.v, "scripted", &prompt_hash(), "claim", 10_000, 0, "test", "earned for this test")
            .unwrap();
    }

    fn project(&self) {
        for _ in 0..3 {
            self.s.engine().clock().advance(Duration::from_secs(120));
            while self.s.project_lexical(&self.v).unwrap() == StepOutcome::Progress {}
        }
    }

    /// Both indexes settled, with what the vector index needs embedded.
    fn settle(&self) {
        for _ in 0..200 {
            self.s.engine().clock().advance(Duration::from_secs(120));
            let mut busy = true;
            while busy {
                busy = self.s.project_lexical(&self.v).unwrap() == StepOutcome::Progress;
                busy |= self.s.project_vector(&self.v).unwrap() == StepOutcome::Progress;
            }
            let mut ran = false;
            while let Some(job) = self.s.claim_job(&self.v, EMBED_JOB, "w").unwrap() {
                self.s.run_embed_job(&self.v, &job).unwrap();
                self.s.complete_job(&self.v, &job, vec![]).unwrap();
                ran = true;
            }
            if !ran {
                return;
            }
        }
        panic!("never settled");
    }

    fn trigger(&self, seed: u64, size: u32) {
        self.s
            .trigger_audit(
                &self.admin,
                &self.v,
                TriggerAudit {
                    seed: Some(seed),
                    sample_size: Some(size),
                    run_token_budget: Some(100_000_000),
                },
            )
            .unwrap();
    }

    fn run_job<A: AgentRunner>(&self, agent: Option<&A>) -> AuditRunRow {
        let job = self.s.claim_job(&self.v, AUDIT_JOB, "w").unwrap().expect("an audit job");
        let id = ObjectId(job.input["run_id"].as_str().unwrap().into());
        let rows = self.s.run_audit_job(&self.v, &job, agent).unwrap();
        assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
        self.s.get_audit_run(&self.admin, &self.v, &id).unwrap().run
    }

    fn audit(&self, seed: u64) -> AuditRunRow {
        self.trigger(seed, 1000);
        self.run_job(None::<&ScriptedAgent>)
    }

    fn sample(&self, run: &AuditRunRow) -> Vec<ObjectId> {
        self.s
            .audit_run_checks(&self.admin, &self.v, &run.header.id, false, None, 10_000)
            .unwrap()
            .into_iter()
            .map(|c| c.claim_id)
            .collect()
    }

    fn claims(&self) -> Vec<memex_types::AssertionRow> {
        let snap = self.s.engine().store().snapshot(&self.v).unwrap();
        snap.rows(Table::Assertion, None)
            .unwrap()
            .into_iter()
            .filter_map(|r| if let Row::Assertion(a) = r { Some(a) } else { None })
            .collect()
    }

    fn audit_items(&self) -> Vec<memex_types::ChangeSetRow> {
        let f = ReviewFilter {
            state: Some(ChangeSetState::Pending),
            proposer: Some(memex_core::audit::AUDITOR.into()),
            ..ReviewFilter::default()
        };
        self.s
            .review_queue(&self.admin, &self.v, &f, None, 10_000)
            .unwrap()
            .into_iter()
            .map(|i| i.changeset)
            .collect()
    }

    fn break_quote(&self, claim: &ObjectId) {
        let snap = self.s.engine().store().snapshot(&self.v).unwrap();
        let mut e = snap.evidence_of(std::slice::from_ref(claim)).unwrap().remove(0);
        drop(snap);
        e.quote = "något annat helt och hållet".into();
        self.s
            .apply_as(&self.admin, &self.v, "state.recorded", 1, json!({"mutations": [{"put": {"evidence": e}}]}))
            .unwrap();
    }
}

#[test]
#[ignore = "needs Postgres"]
fn pg_audit_failure_revokes_requeues_and_a_person_decides() {
    let r = rig("t-audit-e2e");
    r.earn();
    for n in 0..6 {
        r.machine_claim(n);
    }
    assert_eq!(r.claims().len(), 6, "they committed with nobody looking");
    let first = r.claims()[0].clone();
    r.break_quote(&first.header.id);
    let run = r.audit(1);
    assert_eq!(
        (run.state, run.failed, run.revoked.len(), run.siblings_requeued),
        (AuditRunState::Finished, 1, 1, 5)
    );
    let table = r.s.auto_commit_table(&r.admin, &r.v, None, 10).unwrap();
    let entry = table.entries.iter().find(|e| e.row.item_type == "claim").unwrap();
    assert!(!entry.on);
    assert_eq!((entry.row.correct, entry.row.total_correct, entry.row.total_incorrect), (0, 10_000, 1));
    let items = r.audit_items();
    assert_eq!(items.len(), 6);
    assert!(
        r.claims().iter().all(|c| c.header.status == Status::Active),
        "nothing has changed until a person acts"
    );
    // The queue filters reach them by the extractor they're about.
    let by_extractor = ReviewFilter {
        state: Some(ChangeSetState::Pending),
        extractor: Some("scripted".into()),
        item_type: Some("claim".into()),
        ..ReviewFilter::default()
    };
    assert_eq!(r.s.review_queue(&r.admin, &r.v, &by_extractor, None, 100).unwrap().len(), 6);
    let sibling = items.iter().find(|i| i.idempotency_key.starts_with("audit-sibling:")).unwrap();
    let approved = r.s.approve(&r.admin, &r.v, &sibling.header.id, None).unwrap();
    assert_eq!(approved.state, ChangeSetState::Committed);
    assert_eq!(r.claims().iter().filter(|c| c.header.status == Status::Superseded).count(), 1);
    r.s.revert(&r.admin, &r.v, &approved.header.id, "it was right".into()).unwrap();
    assert!(r.claims().iter().all(|c| c.header.status == Status::Active), "a revert brings it back");
}

#[test]
#[ignore = "needs Postgres"]
fn pg_audit_same_seed_and_state_give_the_same_sample_on_a_schedule_or_on_demand() {
    let r = rig("t-audit-seed");
    r.earn();
    for n in 0..30 {
        r.machine_claim(n);
    }
    let (mut schedule, _, _) = r.s.audit_schedule(&r.admin, &r.v).unwrap();
    schedule.enabled = true;
    schedule.interval_ms = 60_000;
    schedule.sample_size = 10;
    r.s.set_audit_schedule(&r.admin, &r.v, schedule).unwrap();
    r.s.engine().clock().advance(Duration::from_secs(60));
    let scheduled = r.s.audit_tick(&r.v).unwrap().expect("due");
    assert_eq!(scheduled.trigger, AuditTrigger::Scheduled);
    let first = r.run_job(None::<&ScriptedAgent>);
    assert_eq!(first.sampled, 10);
    r.trigger(scheduled.seed, 10);
    let second = r.run_job(None::<&ScriptedAgent>);
    assert_eq!(r.sample(&first), r.sample(&second));
    r.trigger(scheduled.seed.wrapping_add(1), 10);
    assert_ne!(r.sample(&first), r.sample(&r.run_job(None::<&ScriptedAgent>)));
}

fn slow_digest(r: &Rig, kind: DocKind) -> IdDigest {
    let snap = r.s.engine().store().snapshot(&r.v).unwrap();
    let table = if kind == DocKind::Claim { Table::Assertion } else { Table::Segment };
    let mut digest = IdDigest::default();
    for row in snap.rows(table, None).unwrap() {
        let wanted = match &row {
            Row::Assertion(a) => memex_core::audit::is_candidate(a),
            Row::Segment(s) => {
                let current = s.rendition_id.as_ref().is_none_or(|id| {
                    matches!(
                        snap.get(&memex_types::RowKey { table: Table::Rendition, id: id.clone() }).unwrap(),
                        Some(Row::Rendition(x)) if x.superseded_by.is_none()
                    )
                });
                s.header.status == Status::Active && current
            }
            _ => false,
        };
        if wanted {
            memex_core::audit::add_to_digest(&mut digest, &row.header().id.0);
        }
    }
    digest
}

#[test]
#[ignore = "needs Postgres"]
fn pg_audit_counts_and_hashes_agree_across_the_store_and_both_indexes() {
    let r = rig("t-audit-digest");
    r.earn();
    for n in 0..5 {
        r.machine_claim(n);
    }
    r.settle();
    let snap = r.s.engine().store().snapshot(&r.v).unwrap();
    for kind in [DocKind::Claim, DocKind::Segment] {
        let sql = snap.id_digest(kind).unwrap();
        assert_eq!(sql, slow_digest(&r, kind), "Postgres hashes ids as core does: {kind:?}");
        assert!(sql.count > 0);
    }
    drop(snap);
    let run = r.audit(1);
    let compared: Vec<&memex_types::VaultFinding> = run.vault_findings.iter().filter(|f| f.check == Kind::IndexReconciliation).collect();
    assert_eq!(compared.len(), 4, "{compared:?}");
    assert!(compared.iter().all(|f| f.status == St::Pass), "{compared:?}");
    assert_eq!(run.suspect + run.failed, 0);
    // A retired claim leaves both indexes, and the comparison agrees once they've caught up.
    let claim = r.claims().remove(0);
    let cs =
        r.s.propose(
            &r.admin,
            &r.v,
            memex_core::Proposal {
                ops: vec![Op::RetractAssertion {
                    target: claim.header.id.clone(),
                    base_version: claim.header.version,
                    counts_against: false,
                }],
                rationale: "wrong".into(),
                idempotency_key: "retire".into(),
            },
            false,
        )
        .unwrap()
        .changeset
        .unwrap();
    r.s.approve(&r.admin, &r.v, &cs.header.id, None).unwrap();
    r.settle();
    let after = r.audit(2);
    let compared: Vec<&memex_types::VaultFinding> = after.vault_findings.iter().filter(|f| f.check == Kind::IndexReconciliation).collect();
    assert!(compared.iter().all(|f| f.status == St::Pass), "{compared:?}");
    assert_eq!(after.population, 4);
}

#[test]
#[ignore = "needs Postgres"]
fn pg_audit_a_retired_claim_leaves_search_and_returns_on_a_revert() {
    let r = rig("t-audit-readers");
    r.machine_claim(0);
    for c in r.s.list_changesets(&r.admin, &r.v, Some(ChangeSetState::Pending), None, 10).unwrap() {
        r.s.approve(&r.admin, &r.v, &c.header.id, None).unwrap();
    }
    r.settle();
    let claim = r.claims().remove(0);
    let found = |mode| {
        let mut q = SearchRequest::new("Fartyget nummer");
        q.mode = mode;
        q.filter.kinds = vec![DocKind::Claim];
        r.s.search(&r.admin, &r.v, &q)
            .unwrap()
            .items
            .iter()
            .filter(|i| i.kind == ItemKind::Claim)
            .count()
    };
    assert_eq!(found(SearchMode::Lexical), 1);
    assert_eq!(found(SearchMode::Vector), 1);
    let cs =
        r.s.propose(
            &r.admin,
            &r.v,
            memex_core::Proposal {
                ops: vec![Op::RetractAssertion {
                    target: claim.header.id.clone(),
                    base_version: claim.header.version,
                    counts_against: false,
                }],
                rationale: "wrong".into(),
                idempotency_key: "retire".into(),
            },
            false,
        )
        .unwrap()
        .changeset
        .unwrap();
    let retired = r.s.approve(&r.admin, &r.v, &cs.header.id, None).unwrap();
    r.settle();
    assert_eq!((found(SearchMode::Lexical), found(SearchMode::Vector)), (0, 0));
    let row = r.claims().remove(0);
    assert_eq!((row.header.status, row.superseded_by), (Status::Superseded, None));
    let at = r.s.engine().store().snapshot(&r.v).unwrap().head().unwrap().unwrap();
    assert!(r.s.believed_at(&r.admin, &r.v, &claim.subject, at).unwrap().is_empty());
    r.s.revert(&r.admin, &r.v, &retired.header.id, "it was right".into()).unwrap();
    r.settle();
    assert_eq!((found(SearchMode::Lexical), found(SearchMode::Vector)), (1, 1));
    assert_eq!(r.claims().remove(0), claim, "exactly as it was");
}

fn print_recall(label: &str, report: &memex_core::audit::RecallReport) {
    println!("== {label}");
    for k in &report.kinds {
        println!(
            "{:18} {:13} planted {:3} audited {:3} caught {:3} recall {:.2}",
            k.kind.as_str(),
            if k.needs_llm { "llm only" } else { "deterministic" },
            k.planted,
            k.audited,
            k.caught,
            k.recall.unwrap_or(0.0)
        );
    }
    println!(
        "deterministic {}/{} = {:?}; llm only {}/{} = {:?} on evidence {}; overall {}/{} = {:?}; clean audited {}, false positives deterministic {} llm {}; target met {:?}",
        report.deterministic.caught,
        report.deterministic.planted,
        report.deterministic.recall,
        report.llm_only.caught,
        report.llm_only.planted,
        report.llm_only.recall,
        report.llm_evidence.as_str(),
        report.overall_caught,
        report.overall_planted,
        report.overall_recall,
        report.clean_audited,
        report.false_positives_deterministic,
        report.false_positives_llm,
        report.target_met
    );
}

/// The fixture behind the planted runs: 14 kinds of 8, and 60 clean to count false positives on.
fn planted_rig(name: &str) -> Rig {
    let r = rig(name);
    r.s.mark_audit_test_vault(&r.admin, &r.v).unwrap();
    r.earn();
    for n in 0..172 {
        r.machine_claim(n);
    }
    r.project();
    r.s.plant_audit_errors(&r.admin, &r.v, 2026, 8, &PlantedKind::ERRORS).unwrap();
    r.project();
    r
}

#[test]
#[ignore = "needs Postgres, and takes minutes on a busy machine"]
fn pg_audit_planted_errors_on_real_storage_with_no_llm() {
    let r = planted_rig("t-audit-planted");
    let run = r.audit(11);
    assert_eq!(run.sampled, 172);
    let report = r.s.audit_recall(&r.admin, &r.v).unwrap();
    print_recall("deterministic checks on Postgres and tantivy, with no LLM", &report);
    assert_eq!(report.deterministic.recall, Some(1.0));
    assert_eq!(report.llm_only.caught, 0);
    assert_eq!(report.false_positives_deterministic, 0);
    assert_eq!(report.target_met, None);
}

#[test]
#[ignore = "needs Postgres, and takes minutes on a busy machine"]
fn pg_audit_planted_errors_on_real_storage_with_a_scripted_llm() {
    let r = planted_rig("t-audit-planted-llm");
    r.trigger(11, 1000);
    let agent = ScriptedAgent::answering(scripted_oracle);
    let run = r.run_job(Some(&agent));
    let report = r.s.audit_recall(&r.admin, &r.v).unwrap();
    print_recall("scripted LLM answers: the mechanism only, and nothing about a model", &report);
    assert_eq!(report.deterministic.recall, Some(1.0));
    assert_eq!(report.llm_only.recall, Some(1.0));
    assert_eq!(report.llm_evidence.as_str(), "scripted");
    assert_eq!(report.target_met, None, "scripted answers must not claim the target");
    println!("{} LLM calls, {} tokens as the script reported them", run.llm_calls, run.usage.total());
}

/// The live number spec 026's target needs. It calls a model, costs money, and sends the fixture's made-up sentences to the provider, so run it only when asked: with the `MEMEX_LLM_*` settings and `MEMEX_EVAL_MAX_TOKENS` set. The fixture is synthetic, so nothing here needs approving for sending.
#[test]
#[ignore = "calls a live LLM"]
fn live_planted_recall() {
    let settings = memex_core::Settings::from_vars(|name| std::env::var(name).ok()).expect("settings");
    let llm = settings.llm.clone().expect("not run: MEMEX_LLM_PROVIDER isn't set");
    let cap: u64 = std::env::var("MEMEX_EVAL_MAX_TOKENS")
        .expect("not run: MEMEX_EVAL_MAX_TOKENS isn't set")
        .parse()
        .expect("a number of tokens");
    let endpoint = memexvault::edges::llm_endpoint(&llm).expect("endpoint");
    let agent = memexvault::edges::AdkAgentRunner(memex_agent_adk::AdkAgent::new(endpoint).expect("an agent"));
    let r = planted_rig("t-audit-planted-live");
    let (mut schedule, _, _) = r.s.audit_schedule(&r.admin, &r.v).unwrap();
    schedule.day_token_budget = cap;
    r.s.set_audit_schedule(&r.admin, &r.v, schedule).unwrap();
    r.s.trigger_audit(
        &r.admin,
        &r.v,
        TriggerAudit {
            seed: Some(11),
            sample_size: Some(1000),
            run_token_budget: Some(cap),
        },
    )
    .unwrap();
    let run = r.run_job(Some(&agent));
    let report = r.s.audit_recall(&r.admin, &r.v).unwrap();
    print_recall(&format!("LIVE: {} {}", llm.provider, llm.model), &report);
    println!("{} calls, {} tokens, stop {:?}", run.llm_calls, run.usage.total(), run.stop);
    assert_eq!(
        run.stop, None,
        "a stopped run didn't check everything, so its recall is an underestimate: raise the cap"
    );
    assert_eq!(report.target_met, Some(true), "{:?}", report.overall_recall);
}
