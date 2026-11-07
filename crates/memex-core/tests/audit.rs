//! The audit agent through the facade, on fakes. Agent replies are scripted, so these tests prove the mechanisms around a model and never a model's judgement.
use memex_core::audit::{self, LlmEvidence, RecallReport};
use memex_core::testing::{ManualClock, MemBlobStore, MemRegistry, MemStore, ScriptedAgent, SeededRandom};
use memex_core::{
    AUDIT_JOB, AuditLimits, Caller, ClaimRefStatus, EXTRACT_JOB, ExtractJobError, ExtractionLimits, NewPage, NewToken, Proposal, RENDITION_JOB, RenditionInput,
    Server, ServerEdges, ServerError, Snapshot, StepOutcome, Store, TriggerAudit, UploadLimits, UploadRequest,
};
use memex_types::{
    AgentError, AgentReply, AgentRequest, AgentUsage, AsOf, AuditCheckKind as Kind, AuditCheckRow, AuditCheckStatus as St, AuditOutcome, AuditRunRow,
    AuditRunState, AuditSchedule, AuditStop, AuditTrigger, AutoCommitChangeKind, ChangeSetRow, ChangeSetState, DocKind, Epistemic, EvidenceInput, Grant, ItemKind,
    JobRow, JobState, ObjectId, Op, PlantedKind, PrincipalKind, ReviewFilter, RiskDecision, Row, Scope, SearchMode, SearchRequest, Stance, Status, Table, VaultId,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

struct Fakes;

impl ServerEdges for Fakes {
    type Caches = memex_core::NoCaches;
    type Registry = MemRegistry;
    type Store = MemStore;
    type Blobs = MemBlobStore<ManualClock>;
    type Clock = ManualClock;
    type Seed = SeededRandom;
    type Rasterizer = memex_core::testing::FakeRasterizer;
    type Lexical = memex_core::testing::MemLexical;
    type Vector = memex_core::testing::MemVector;
    type Embedder = memex_core::testing::FakeEmbedder;
    type Reranker = memex_core::testing::FakeReranker;
}

struct World {
    s: Server<Fakes>,
    admin: Caller,
    agent_admin: Caller,
    v: VaultId,
}

const USAGE: AgentUsage = AgentUsage {
    input_tokens: 70,
    output_tokens: 9,
    thinking_tokens: 1,
};
const DAY_MS: u64 = 86_400_000;

fn world() -> World {
    let s = Server::<Fakes>::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(10 * DAY_MS as i64),
        SeededRandom::new(26),
        memex_core::testing::mem_search(),
    );
    let admin = s.authenticate(&s.bootstrap("root").unwrap().token).unwrap();
    let v = s.create_vault(&admin, "v").unwrap().id;
    let p = s.create_principal(&admin, "agent admin", PrincipalKind::Agent, false).unwrap();
    let t = s
        .create_token(
            &admin,
            NewToken {
                principal_id: Some(p.id),
                name: "t".into(),
                grants: vec![Grant {
                    vault: v.clone(),
                    scope: Scope::Admin,
                }],
                expires_at_ms: None,
            },
        )
        .unwrap();
    let agent_admin = s.authenticate(&t.token).unwrap();
    s.set_extraction(Some(ExtractionLimits {
        document_budget: 100_000_000,
        ..ExtractionLimits::default()
    }));
    s.set_auditor(Some(AuditLimits::default()));
    World { s, admin, agent_admin, v }
}

fn reply(doc: Value) -> Result<AgentReply, AgentError> {
    Ok(AgentReply {
        json: doc.to_string(),
        usage: USAGE,
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

fn is_entailment(req: &AgentRequest) -> bool {
    req.schema_json.contains("not_entailed")
}

fn verdict(v: &str) -> Result<AgentReply, AgentError> {
    reply(json!({"verdict": v, "reason": "scripted"}))
}

/// What the scripted LLM checks say, written from the fixtures' shape, so it can show the mechanism works and nothing else. A value the quote holds is entailed, and re-extraction reads the fixture's one sentence back. It looks only at the value, so a wrong subject gets past entailment and takes re-extraction to find.
fn scripted_oracle(req: &AgentRequest) -> Result<AgentReply, AgentError> {
    if is_entailment(req) {
        let input: Value = serde_json::from_str(&req.input).unwrap();
        let claim = &input["claims"][0];
        let value = claim["value"].as_str().unwrap_or_default();
        let quote = claim["quotes"][0].as_str().unwrap_or_default();
        return verdict(if quote.contains(value) { "entailed" } else { "not_entailed" });
    }
    let input: Value = serde_json::from_str(&req.input).unwrap();
    let text = input["segments"][0]["text"].as_str().unwrap_or_default();
    let first = text.split('.').next().unwrap_or_default();
    let (subject, _) = first.split_once(" byggdes").unwrap_or((first, ""));
    reply(json!({
        "claims": [{"segment_id": "s1", "quote": format!("{first}."), "subject": subject, "predicate": "byggdes i", "value": "Stockholm"}],
        "entities": {},
    }))
}

impl World {
    fn upload(&self, name: &str, tier: &str, text: &str) -> ObjectId {
        let req = UploadRequest {
            name: name.into(),
            trust_tier: tier.into(),
            metadata: json!({}),
            replaces: None,
        };
        self.s
            .upload(&self.admin, &self.v, req, UploadLimits::default(), &mut text.as_bytes())
            .unwrap()
            .source
            .header
            .id
    }

    fn parse(&self, source: &ObjectId, text: &str) {
        let job = self.s.claim_job(&self.v, RENDITION_JOB, "parser").unwrap().unwrap();
        let input = RenditionInput::Plain {
            text: text.into(),
            parser: "plain-1".into(),
        };
        let rows = self.s.build_rendition(&self.v, source, input).unwrap();
        assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
    }

    fn extract(&self, agent: &ScriptedAgent) {
        let job = self.s.claim_job(&self.v, EXTRACT_JOB, "w").unwrap().expect("an extraction job");
        let rows = self.s.run_extraction_job(&self.v, &job, agent).unwrap();
        assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
    }

    /// One document, one machine claim on it. It commits at once if the combination has earned that, and otherwise waits for `approve_all`.
    fn machine_claim(&self, n: usize) -> ObjectId {
        let s = sentence(n);
        let text = long_text(&s);
        let source = self.upload(&format!("d{n}.txt"), "trusted", &text);
        self.parse(&source, &text);
        let subject = format!("Fartyget nummer {n}");
        self.extract(&ScriptedAgent::new(vec![reply(json!({
            "claims": [{"segment_id": "s1", "quote": s, "subject": subject, "predicate": "byggdes i", "value": "Stockholm"}],
            "entities": {},
        }))]));
        source
    }

    /// Many claims from one document, each its own proposal, so sibling tests stay quick.
    fn many_claims(&self, count: usize) {
        let s = sentence(0);
        let text = long_text(&s);
        let source = self.upload("many.txt", "trusted", &text);
        self.parse(&source, &text);
        let claims: Vec<Value> = (0..count)
            .map(|n| json!({"segment_id": "s1", "quote": s, "subject": "Fartyget", "predicate": format!("hade egenskap {n}"), "value": "Stockholm"}))
            .collect();
        self.extract(&ScriptedAgent::new(vec![reply(json!({"claims": claims, "entities": {}}))]));
    }

    fn earn_auto_commit(&self) {
        self.s
            .record_audit_samples(&self.v, "scripted", &prompt_hash(), "claim", 10_000, 0, "test", "earned for this test")
            .unwrap();
    }

    fn auto_commit_on(&self) -> bool {
        let table = self.s.auto_commit_table(&self.admin, &self.v, None, 100).unwrap();
        table.entries.iter().find(|e| e.row.item_type == "claim").is_some_and(|e| e.on)
    }

    fn combination(&self) -> memex_core::AutoCommitEntry {
        let table = self.s.auto_commit_table(&self.admin, &self.v, None, 100).unwrap();
        table.entries.into_iter().find(|e| e.row.item_type == "claim").expect("a combination")
    }

    fn claims(&self) -> Vec<memex_types::AssertionRow> {
        self.table(Table::Assertion)
            .into_iter()
            .filter_map(|r| if let Row::Assertion(a) = r { Some(a) } else { None })
            .collect()
    }

    fn table(&self, table: Table) -> Vec<Row> {
        let snap = self.s.engine().store().snapshot(&self.v).unwrap();
        let (mut out, mut after) = (vec![], None);
        loop {
            let page = snap.rows(table, after.as_ref()).unwrap();
            let Some(last) = page.last() else { return out };
            after = Some(last.header().id.clone());
            out.extend(page);
        }
    }

    fn approve_pending(&self) {
        for c in self.s.list_changesets(&self.admin, &self.v, Some(ChangeSetState::Pending), None, 10_000).unwrap() {
            self.s.approve(&self.admin, &self.v, &c.header.id, None).unwrap();
        }
    }

    /// Long enough for the projector's commit policy, so what a test then reads is what the index really holds.
    fn project(&self) {
        for _ in 0..3 {
            self.advance(120_000);
            while self.s.project_lexical(&self.v).unwrap() == StepOutcome::Progress {}
        }
    }

    fn trigger(&self, seed: u64) -> AuditRunRow {
        self.s
            .trigger_audit(
                &self.admin,
                &self.v,
                TriggerAudit {
                    seed: Some(seed),
                    sample_size: Some(1000),
                    run_token_budget: Some(100_000_000),
                },
            )
            .unwrap()
    }

    fn run_job(&self, agent: Option<&ScriptedAgent>) -> AuditRunRow {
        let job = self.s.claim_job(&self.v, AUDIT_JOB, "w").unwrap().expect("an audit job");
        let run_id = ObjectId(job.input["run_id"].as_str().unwrap().into());
        let rows = self.s.run_audit_job(&self.v, &job, agent).unwrap();
        assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
        self.s.get_audit_run(&self.admin, &self.v, &run_id).unwrap().run
    }

    fn audit(&self, seed: u64, agent: Option<&ScriptedAgent>) -> AuditRunRow {
        self.trigger(seed);
        self.run_job(agent)
    }

    fn checks(&self, run: &AuditRunRow) -> Vec<AuditCheckRow> {
        self.s.audit_run_checks(&self.admin, &self.v, &run.header.id, false, None, 10_000).unwrap()
    }

    fn sample(&self, run: &AuditRunRow) -> Vec<ObjectId> {
        self.checks(run).into_iter().map(|c| c.claim_id).collect()
    }

    fn pending_audit_items(&self) -> Vec<ChangeSetRow> {
        let f = ReviewFilter {
            state: Some(ChangeSetState::Pending),
            proposer: Some(audit::AUDITOR.into()),
            ..ReviewFilter::default()
        };
        self.s
            .review_queue(&self.admin, &self.v, &f, None, 10_000)
            .unwrap()
            .into_iter()
            .map(|i| i.changeset)
            .collect()
    }

    /// Takes the quote out from under a claim, as nothing but a bug or tampering would, so the deterministic check has something to find.
    fn break_quote(&self, claim: &ObjectId) {
        let snap = self.s.engine().store().snapshot(&self.v).unwrap();
        let mut e = snap.evidence_of(std::slice::from_ref(claim)).unwrap().remove(0);
        drop(snap);
        e.quote = "något annat helt och hållet".into();
        self.write(Row::Evidence(e));
    }

    /// A direct write, as a test's way to reach a state nothing else leads to.
    fn write(&self, row: Row) {
        self.s
            .apply_as(&self.admin, &self.v, "state.recorded", 1, json!({"mutations": [{"put": row}]}))
            .unwrap();
    }

    fn set_schedule(&self, f: impl FnOnce(&mut AuditSchedule)) {
        let (mut s, _, _) = self.s.audit_schedule(&self.admin, &self.v).unwrap();
        f(&mut s);
        self.s.set_audit_schedule(&self.admin, &self.v, s).unwrap();
    }

    fn advance(&self, ms: u64) {
        self.s.engine().clock().advance(Duration::from_millis(ms));
    }
}

// Item 1: sampling.

#[test]
fn the_same_seed_and_state_give_an_identical_sample_and_another_seed_differs() {
    let w = world();
    w.earn_auto_commit();
    for n in 0..120 {
        w.machine_claim(n);
    }
    let sized = |seed| {
        w.s.trigger_audit(
            &w.admin,
            &w.v,
            TriggerAudit {
                seed: Some(seed),
                sample_size: Some(30),
                run_token_budget: None,
            },
        )
        .unwrap();
        w.run_job(None)
    };
    let first = sized(7);
    assert_eq!(first.sampled, 30);
    assert_eq!(first.population, 120);
    assert_eq!(w.sample(&first), w.sample(&sized(7)), "a run's own events change nothing a sample depends on");
    assert_eq!(w.sample(&first), w.sample(&sized(7)));
    let other = sized(8);
    assert_ne!(w.sample(&first), w.sample(&other));
    assert_eq!(first.seed, 7);
    assert!(first.state_seq.is_some());
}

#[test]
fn a_later_clock_does_not_change_the_sample() {
    let w = world();
    w.earn_auto_commit();
    for n in 0..60 {
        w.machine_claim(n);
        w.advance(DAY_MS);
    }
    let t = TriggerAudit {
        seed: Some(5),
        sample_size: Some(20),
        run_token_budget: None,
    };
    w.s.trigger_audit(&w.admin, &w.v, t.clone()).unwrap();
    let first = w.run_job(None);
    w.advance(400 * DAY_MS);
    w.s.trigger_audit(&w.admin, &w.v, t).unwrap();
    assert_eq!(w.sample(&first), w.sample(&w.run_job(None)), "age counts from the vault's newest commit");
}

#[test]
fn a_sample_does_not_pick_retired_claims() {
    let w = world();
    for n in 0..3 {
        w.machine_claim(n);
    }
    w.approve_pending();
    let target = w.claims()[0].clone();
    retire(&w, &target);
    let run = w.audit(1, None);
    assert_eq!(run.population, 2);
    assert!(!w.sample(&run).contains(&target.header.id));
}

/// Proposes and approves the retirement of a claim, as a person.
fn retire(w: &World, claim: &memex_types::AssertionRow) -> ChangeSetRow {
    let outcome =
        w.s.propose(
            &w.admin,
            &w.v,
            Proposal {
                ops: vec![Op::RetractAssertion {
                    target: claim.header.id.clone(),
                    base_version: claim.header.version,
                    counts_against: false,
                }],
                rationale: "it's wrong".into(),
                idempotency_key: format!("retire:{}:{}", claim.header.id, claim.header.version),
            },
            false,
        )
        .unwrap();
    let row = outcome.changeset.unwrap();
    assert_eq!(row.state, ChangeSetState::Pending, "a person's retirement goes to review too");
    w.s.approve(&w.admin, &w.v, &row.header.id, None).unwrap()
}

// Item 2: planted errors.

const PER_KIND: u32 = 8;

fn planted_world(clean: usize) -> World {
    let w = world();
    w.s.mark_audit_test_vault(&w.admin, &w.v).unwrap();
    w.earn_auto_commit();
    let total = PlantedKind::ERRORS.len() * PER_KIND as usize + clean;
    for n in 0..total {
        w.machine_claim(n);
    }
    w.project();
    let planted = w.s.plant_audit_errors(&w.admin, &w.v, 2026, PER_KIND, &PlantedKind::ERRORS).unwrap();
    assert_eq!(planted.values().sum::<u64>(), 14 * u64::from(PER_KIND));
    w.project();
    w
}

fn print_report(label: &str, r: &RecallReport) {
    println!("== {label}");
    for k in &r.kinds {
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
        "deterministic {}/{} = {:?}; llm only {}/{} = {:?}, on evidence {}; overall {}/{} = {:?}; clean audited {}, false positives deterministic {} llm {}; target met {:?}",
        r.deterministic.caught,
        r.deterministic.planted,
        r.deterministic.recall,
        r.llm_only.caught,
        r.llm_only.planted,
        r.llm_only.recall,
        r.llm_evidence.as_str(),
        r.overall_caught,
        r.overall_planted,
        r.overall_recall,
        r.clean_audited,
        r.false_positives_deterministic,
        r.false_positives_llm,
        r.target_met
    );
}

#[test]
fn planted_errors_with_no_llm_only_the_deterministic_kinds_can_be_caught() {
    let w = planted_world(60);
    let run = w.audit(11, None);
    assert_eq!(run.model, None);
    assert_eq!(run.sampled, 172);
    let r = w.s.audit_recall(&w.admin, &w.v).unwrap();
    print_report("planted errors, deterministic checks only", &r);
    assert_eq!((r.deterministic.planted, r.llm_only.planted), (80, 32));
    assert!(r.overall_planted >= 100);
    assert_eq!(r.deterministic.recall, Some(1.0), "every kind the deterministic checks are for");
    for k in r.kinds.iter().filter(|k| !k.needs_llm) {
        assert_eq!(k.caught, k.planted, "{}", k.kind.as_str());
    }
    assert_eq!(r.llm_only.caught, 0, "with nothing to read a claim's meaning, these get through");
    assert_eq!(r.llm_evidence, LlmEvidence::None);
    assert_eq!(r.clean_audited, 60);
    assert_eq!(r.false_positives_deterministic, 0);
    assert_eq!(r.target_met, None);
    // Only the deterministic group, out of everything planted: 80/112.
    assert!((r.overall_recall.unwrap() - 80.0 / 112.0).abs() < 1e-9);
}

#[test]
fn planted_errors_with_a_scripted_llm_prove_the_mechanism_and_judge_nothing() {
    let w = planted_world(60);
    let agent = ScriptedAgent::answering(scripted_oracle);
    let run = w.audit(11, Some(&agent));
    assert_eq!(run.model.as_deref(), Some(audit::SCRIPTED_MODEL));
    let r = w.s.audit_recall(&w.admin, &w.v).unwrap();
    print_report("planted errors, scripted LLM answers: mechanism only", &r);
    assert_eq!(r.deterministic.recall, Some(1.0));
    assert_eq!(r.llm_evidence, LlmEvidence::Scripted);
    for k in r.kinds.iter().filter(|k| k.needs_llm) {
        assert_eq!(k.caught, k.planted, "the path from an answer to a finding works for {}", k.kind.as_str());
    }
    assert_eq!((r.false_positives_deterministic, r.false_positives_llm), (0, 0));
    assert_eq!(r.target_met, None, "scripted answers must not claim the target");
    let sent = agent.requests();
    println!(
        "live estimate: {} calls for {} sampled, {} estimated input tokens",
        sent.len(),
        run.sampled,
        sent.iter()
            .map(|q| memex_core::extraction::estimated_tokens(&q.instruction)
                + memex_core::extraction::estimated_tokens(&q.schema_json)
                + memex_core::extraction::estimated_tokens(&q.input))
            .sum::<u64>()
    );
}

#[test]
fn a_wrong_subject_is_caught_only_by_blind_reextraction_and_does_not_revoke_alone() {
    let w = world();
    w.s.mark_audit_test_vault(&w.admin, &w.v).unwrap();
    w.earn_auto_commit();
    for n in 0..4 {
        w.machine_claim(n);
    }
    w.project();
    w.s.plant_audit_errors(&w.admin, &w.v, 1, 2, &[PlantedKind::WrongSubject]).unwrap();
    w.project();
    let run = w.audit(1, Some(&ScriptedAgent::answering(scripted_oracle)));
    assert_eq!((run.suspect, run.failed), (2, 0));
    assert!(w.auto_commit_on(), "a disagreement alone says nothing against the extractor until a person agrees");
    let found = w.s.audit_run_checks(&w.admin, &w.v, &run.header.id, true, None, 10).unwrap();
    assert!(found.iter().all(|c| c.checks.iter().any(|k| k.check == Kind::BlindReextraction && k.status == St::Fail)));
    assert!(found.iter().all(|c| c.checks.iter().any(|k| k.check == Kind::Entailment && k.status == St::Pass)));
}

#[test]
fn the_blind_reextraction_is_shown_the_segment_and_nothing_about_the_claim() {
    let w = world();
    w.machine_claim(0);
    w.approve_pending();
    let agent = ScriptedAgent::answering(scripted_oracle);
    w.audit(1, Some(&agent));
    let sent = agent.requests();
    let entail = sent.iter().find(|q| is_entailment(q)).unwrap();
    let blind = sent.iter().find(|q| !is_entailment(q)).unwrap();
    let doc: Value = serde_json::from_str(&entail.input).unwrap();
    assert_eq!(doc["claims"][0]["quotes"][0], json!(sentence(0)), "entailment sees the quote");
    assert!(!entail.input.contains("Detta är en lång mening"), "and not the rest of the segment");
    assert!(blind.input.contains("Detta är en lång mening"), "re-extraction sees the segment");
    assert!(!blind.input.contains("byggdes i\","), "and not the claim");
    assert!(!blind.input.contains("predicate"));
}

#[test]
fn planting_is_refused_outside_a_test_vault_and_a_vault_with_content_can_not_become_one() {
    let w = world();
    w.machine_claim(0);
    let refused = w.s.plant_audit_errors(&w.admin, &w.v, 1, 1, &[PlantedKind::AlteredQuote]).unwrap_err();
    assert!(matches!(refused, ServerError::Conflict(_)), "{refused}");
    let refused = w.s.mark_audit_test_vault(&w.admin, &w.v).unwrap_err();
    assert!(matches!(refused, ServerError::Conflict(_)), "{refused}");
    let w = world();
    w.s.mark_audit_test_vault(&w.admin, &w.v).unwrap();
    w.earn_auto_commit();
    w.machine_claim(0);
    w.project();
    let too_many = w.s.plant_audit_errors(&w.admin, &w.v, 1, 5, &[PlantedKind::WrongValue]).unwrap_err();
    assert!(matches!(too_many, ServerError::Invalid(_)), "{too_many}");
    w.s.plant_audit_errors(&w.admin, &w.v, 1, 1, &[PlantedKind::WrongValue]).unwrap();
    let again = w.s.plant_audit_errors(&w.admin, &w.v, 1, 1, &[PlantedKind::WrongValue]).unwrap_err();
    assert!(matches!(again, ServerError::Conflict(_)), "{again}");
}

// Item 3: revoking and requeuing.

#[test]
fn a_failure_revokes_auto_commit_and_requeues_siblings_end_to_end() {
    let w = world();
    w.earn_auto_commit();
    for n in 0..6 {
        w.machine_claim(n);
    }
    assert!(w.auto_commit_on());
    assert_eq!(w.claims().len(), 6, "they committed with nobody looking");
    let claims = w.claims();
    w.break_quote(&claims[0].header.id);
    let run = w.audit(3, None);
    assert_eq!(run.failed, 1);
    assert_eq!(run.revoked.len(), 1);
    // It held 10,000 correct and one wrong, so a bound alone would have left it on.
    let entry = w.combination();
    assert!(!entry.on);
    assert_eq!((entry.row.correct, entry.row.incorrect), (0, 0), "its counts start again");
    assert_eq!((entry.row.total_correct, entry.row.total_incorrect), (10_000, 1), "and the record stays");
    assert_eq!(entry.row.history.last().unwrap().kind, AutoCommitChangeKind::Disabled);
    assert_eq!(entry.row.history.last().unwrap().by, audit::AUDITOR);
    let items = w.pending_audit_items();
    assert_eq!(items.len(), 6, "the finding and its five siblings");
    assert_eq!(run.siblings_requeued, 5);
    assert_eq!(run.siblings_remaining, 0);
    for item in &items {
        assert_eq!(item.proposer, audit::AUDITOR);
        assert!(item.proposer_is_machine);
        assert_eq!(item.risk, RiskDecision::Review);
        let origin = item.origin.as_ref().unwrap();
        assert_eq!(origin.extractor, "scripted", "the queue's filters find them under the extractor they're about");
    }
    // Nothing changed yet: no claim has gone, as only a person can do that.
    assert_eq!(w.claims().iter().filter(|c| c.header.status == Status::Active).count(), 6);
    // A new claim no longer commits by itself.
    w.machine_claim(100);
    assert_eq!(w.claims().len(), 6, "auto-commit is off");
}

#[test]
fn the_finding_says_what_failed_and_links_its_review_item() {
    let w = world();
    w.earn_auto_commit();
    w.machine_claim(0);
    let claim = w.claims()[0].header.id.clone();
    w.break_quote(&claim);
    let run = w.audit(3, None);
    let found = w.s.audit_run_checks(&w.admin, &w.v, &run.header.id, true, None, 10).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].outcome, AuditOutcome::Failed);
    assert_eq!(found[0].checks[0].check, Kind::QuoteRelocation);
    assert_eq!(found[0].checks[0].cause.as_deref(), Some(audit::QUOTE_MISSING));
    let item = found[0].changeset_id.as_ref().unwrap();
    let detail = w.s.review_item(&w.admin, &w.v, item).unwrap();
    assert!(detail.needs_a_person);
    assert_eq!(detail.evidence.len(), 1, "a person sees what the claim cites");
    assert!(detail.item.changeset.rationale.contains("quote_missing"), "{}", detail.item.changeset.rationale);
}

#[test]
fn a_not_entailed_answer_revokes_like_a_failed_quote() {
    let w = world();
    w.earn_auto_commit();
    w.machine_claim(0);
    let agent = ScriptedAgent::answering(|req| {
        if is_entailment(req) {
            verdict("not_entailed")
        } else {
            scripted_oracle(req)
        }
    });
    let run = w.audit(3, Some(&agent));
    assert_eq!((run.failed, run.revoked.len()), (1, 1));
    assert!(!w.auto_commit_on());
    assert_eq!(agent.requests().len(), 1, "a claim already shown wrong costs no re-extraction");
}

#[test]
fn link_rot_a_replaced_rendition_staleness_and_index_drift_raise_findings_and_do_not_revoke() {
    for kind in [
        PlantedKind::MissingSegment,
        PlantedKind::MissingSource,
        PlantedKind::MissingBlob,
        PlantedKind::BrokenAnchor,
        PlantedKind::MovedAnchor,
        PlantedKind::StaleTtl,
        PlantedKind::ExpiredValidity,
        PlantedKind::IndexMissing,
    ] {
        let w = world();
        w.s.mark_audit_test_vault(&w.admin, &w.v).unwrap();
        w.earn_auto_commit();
        for n in 0..3 {
            w.machine_claim(n);
        }
        w.project();
        w.s.plant_audit_errors(&w.admin, &w.v, 1, 1, &[kind]).unwrap();
        w.project();
        let run = w.audit(1, None);
        assert_eq!((run.suspect, run.failed, run.revoked.len()), (1, 0, 0), "{}", kind.as_str());
        assert!(w.auto_commit_on(), "{} isn't the extractor's error", kind.as_str());
        let items = w.pending_audit_items();
        if kind == PlantedKind::IndexMissing {
            assert!(items.is_empty(), "a claim the index lost is fine, and needs a rebuild instead");
        } else {
            assert_eq!(items.len(), 1, "{}", kind.as_str());
            assert!(matches!(items[0].ops[0], Op::RetractAssertion { counts_against: false, .. }));
        }
    }
}

#[test]
fn a_quote_on_a_replaced_rendition_does_not_revoke() {
    let w = world();
    w.earn_auto_commit();
    let source = w.machine_claim(0);
    let _ = source;
    let snap = w.s.engine().store().snapshot(&w.v).unwrap();
    let e = snap.evidence_of(&[w.claims()[0].header.id.clone()]).unwrap().remove(0);
    let Some(Row::Segment(seg)) = snap
        .get(&memex_types::RowKey {
            table: Table::Segment,
            id: e.segment_id.clone(),
        })
        .unwrap()
    else {
        panic!()
    };
    let Some(Row::Rendition(mut r)) = snap
        .get(&memex_types::RowKey {
            table: Table::Rendition,
            id: seg.rendition_id.unwrap(),
        })
        .unwrap()
    else {
        panic!()
    };
    drop(snap);
    r.superseded_by = Some(ObjectId("newer".into()));
    w.write(Row::Rendition(r));
    let run = w.audit(1, None);
    assert_eq!((run.suspect, run.failed), (1, 0));
    assert!(w.auto_commit_on());
    let c = &w.checks(&run)[0];
    assert_eq!(c.checks[0].cause.as_deref(), Some("rendition_replaced"));
}

#[test]
fn a_person_s_claim_gets_a_finding_and_there_is_nothing_to_revoke() {
    let w = world();
    w.earn_auto_commit();
    let text = long_text(&sentence(0));
    let source = w.upload("h.txt", "trusted", &text);
    w.parse(&source, &text);
    let segment = w
        .table(Table::Segment)
        .into_iter()
        .find_map(|r| if let Row::Segment(s) = r { Some(s.header.id) } else { None })
        .unwrap();
    let outcome =
        w.s.propose(
            &w.admin,
            &w.v,
            Proposal {
                ops: vec![Op::AddAssertion {
                    id: ObjectId("human-claim".into()),
                    subject: "Fartyget".into(),
                    predicate: "byggdes i".into(),
                    value: json!("Stockholm"),
                    epistemic: Epistemic::Asserted,
                    machine: false,
                    confidence_permille: 1000,
                    valid_from_ms: None,
                    valid_to_ms: None,
                    evidence: vec![(
                        ObjectId("human-evidence".into()),
                        EvidenceInput {
                            segment_id: segment,
                            quote: sentence(0),
                            stance: Stance::Supports,
                        },
                    )],
                }],
                rationale: "I read it".into(),
                idempotency_key: "human".into(),
            },
            false,
        )
        .unwrap();
    assert_eq!(outcome.changeset.unwrap().state, ChangeSetState::Committed);
    w.break_quote(&ObjectId("human-claim".into()));
    let before = w.combination();
    let run = w.audit(1, None);
    assert_eq!(run.failed, 1, "the claim is still wrong");
    assert!(run.revoked.is_empty());
    let c = &w.checks(&run)[0];
    assert_eq!(c.combination, None);
    assert!(c.changeset_id.is_some(), "a person still hears about it");
    let after = w.combination();
    assert_eq!((after.row.correct, after.row.incorrect), (before.row.correct, before.row.incorrect));
    assert!(after.on);
    // And a person keeping it adds nothing to anyone's record.
    let table_before = w.s.auto_commit_table(&w.admin, &w.v, None, 100).unwrap().entries.len();
    w.s.reject(&w.admin, &w.v, c.changeset_id.as_ref().unwrap(), "it's fine".into()).unwrap();
    assert_eq!(w.s.auto_commit_table(&w.admin, &w.v, None, 100).unwrap().entries.len(), table_before);
}

#[test]
fn a_combination_that_had_not_earned_it_counts_the_failure_and_keeps_its_counts() {
    let w = world();
    w.s.record_audit_samples(&w.v, "scripted", &prompt_hash(), "claim", 20, 0, "t", "f").unwrap();
    w.machine_claim(0);
    w.approve_pending();
    let claim = w.claims()[0].header.id.clone();
    w.break_quote(&claim);
    let run = w.audit(1, None);
    assert_eq!(run.failed, 1);
    assert!(run.revoked.is_empty(), "there was nothing to revoke");
    let entry = w.combination();
    assert_eq!((entry.row.incorrect, entry.row.correct), (1, 21));
}

#[test]
fn siblings_past_the_cap_are_counted_and_later_runs_carry_on_until_covered() {
    let w = world();
    w.earn_auto_commit();
    w.many_claims(450);
    assert_eq!(w.claims().len(), 450);
    let first = w.claims()[0].header.id.clone();
    w.break_quote(&first);
    let one = w.audit(1, None);
    assert_eq!(one.siblings_requeued, audit::SIBLINGS_PER_RUN);
    assert_eq!(one.siblings_remaining, 249, "449 siblings, 200 of them queued");
    let two = w.audit(2, None);
    assert_eq!((two.siblings_requeued, two.siblings_remaining), (200, 49));
    let three = w.audit(3, None);
    assert_eq!((three.siblings_requeued, three.siblings_remaining), (49, 0));
    let four = w.audit(4, None);
    assert_eq!((four.siblings_requeued, four.siblings_remaining), (0, 0), "covered, and nothing queued twice");
    assert_eq!(w.pending_audit_items().len(), 450, "each claim once: 449 siblings and the finding");
    let keys: std::collections::BTreeSet<String> = w.pending_audit_items().into_iter().map(|c| c.idempotency_key).collect();
    assert_eq!(keys.len(), 450);
}

#[test]
fn a_run_with_nothing_wrong_still_carries_on_with_siblings() {
    let w = world();
    w.earn_auto_commit();
    w.many_claims(250);
    let first = w.claims()[0].header.id.clone();
    w.break_quote(&first);
    assert_eq!(w.audit(1, None).siblings_remaining, 49);
    // The next run's sample is small and clean, and it still moves the queue along.
    w.s.trigger_audit(
        &w.admin,
        &w.v,
        TriggerAudit {
            seed: Some(9),
            sample_size: Some(1),
            run_token_budget: None,
        },
    )
    .unwrap();
    let run = w.run_job(None);
    assert_eq!((run.siblings_requeued, run.siblings_remaining), (49, 0));
}

fn decide_all(w: &World, approve: bool) {
    for item in w.pending_audit_items() {
        if approve {
            w.s.approve(&w.admin, &w.v, &item.header.id, None).unwrap();
        } else {
            w.s.reject(&w.admin, &w.v, &item.header.id, "it's right".into()).unwrap();
        }
    }
}

#[test]
fn a_person_keeping_a_sibling_counts_as_correct_and_retiring_it_as_wrong() {
    let w = world();
    w.earn_auto_commit();
    for n in 0..3 {
        w.machine_claim(n);
    }
    let first = w.claims()[0].header.id.clone();
    w.break_quote(&first);
    w.audit(1, None);
    let items = w.pending_audit_items();
    let finding = items.iter().find(|i| i.idempotency_key.starts_with("audit-finding:")).unwrap();
    assert!(matches!(finding.ops[0], Op::RetractAssertion { counts_against: false, .. }));
    let siblings: Vec<&ChangeSetRow> = items.iter().filter(|i| i.idempotency_key.starts_with("audit-sibling:")).collect();
    assert_eq!(siblings.len(), 2);
    assert!(matches!(siblings[0].ops[0], Op::RetractAssertion { counts_against: true, .. }));
    assert_eq!((w.combination().row.correct, w.combination().row.incorrect), (0, 0));
    w.s.reject(&w.admin, &w.v, &siblings[0].header.id, "it's right".into()).unwrap();
    assert_eq!((w.combination().row.correct, w.combination().row.incorrect), (1, 0), "kept");
    w.s.approve(&w.admin, &w.v, &siblings[1].header.id, None).unwrap();
    assert_eq!((w.combination().row.correct, w.combination().row.incorrect), (1, 1), "retired");
    w.s.approve(&w.admin, &w.v, &finding.header.id, None).unwrap();
    assert_eq!((w.combination().row.correct, w.combination().row.incorrect), (1, 1), "the audit already counted that one");
}

#[test]
fn an_agent_s_decision_on_an_audit_item_counts_for_nothing_and_it_can_not_approve() {
    let w = world();
    w.earn_auto_commit();
    for n in 0..2 {
        w.machine_claim(n);
    }
    let first = w.claims()[0].header.id.clone();
    w.break_quote(&first);
    w.audit(1, None);
    let items = w.pending_audit_items();
    let refused = w.s.approve(&w.agent_admin, &w.v, &items[0].header.id, None).unwrap_err();
    assert!(matches!(refused, ServerError::Forbidden), "{refused}");
    w.s.reject(&w.agent_admin, &w.v, &items[0].header.id, "no".into()).unwrap();
    assert_eq!((w.combination().row.correct, w.combination().row.incorrect), (0, 0));
}

#[test]
fn a_claim_a_person_kept_is_not_counted_against_again() {
    let w = world();
    w.earn_auto_commit();
    w.machine_claim(0);
    let claim = w.claims()[0].header.id.clone();
    w.break_quote(&claim);
    let one = w.audit(1, None);
    assert_eq!(one.failed, 1);
    let incorrect = w.combination().row.total_incorrect;
    decide_all(&w, false);
    let two = w.audit(2, None);
    assert_eq!((two.failed, two.suspect), (0, 1), "reported, without a second count");
    assert_eq!(w.combination().row.total_incorrect, incorrect);
    assert!(w.pending_audit_items().is_empty(), "and the same question isn't asked again");
    assert_eq!(w.checks(&two)[0].changeset_id, w.checks(&one)[0].changeset_id);
}

#[test]
fn a_finding_still_pending_is_not_counted_twice() {
    let w = world();
    w.earn_auto_commit();
    w.machine_claim(0);
    let claim = w.claims()[0].header.id.clone();
    w.break_quote(&claim);
    w.audit(1, None);
    let incorrect = w.combination().row.total_incorrect;
    let two = w.audit(2, None);
    assert_eq!(two.failed, 1);
    assert!(two.revoked.is_empty());
    assert_eq!(w.combination().row.total_incorrect, incorrect);
    assert_eq!(w.pending_audit_items().len(), 1);
}

#[test]
fn only_an_entailment_earns_a_correct_sample_and_only_once_per_claim() {
    let w = world();
    for n in 0..3 {
        w.machine_claim(n);
    }
    w.approve_pending();
    let start = w.combination().row.correct;
    let quote_only = w.audit(1, None);
    assert_eq!(quote_only.passed, 3);
    assert_eq!(w.combination().row.correct, start, "a quote where it says proves nothing");
    let agent = ScriptedAgent::answering(scripted_oracle);
    w.audit(2, Some(&agent));
    assert_eq!(w.combination().row.correct, start + 3);
    w.audit(3, Some(&agent));
    assert_eq!(w.combination().row.correct, start + 3, "the same good claims audited again aren't new evidence");
}

// Retirement, in every reader.

fn searched(w: &World, text: &str) -> Vec<ObjectId> {
    let mut q = SearchRequest::new(text);
    q.mode = SearchMode::Lexical;
    w.s.search(&w.admin, &w.v, &q)
        .unwrap()
        .items
        .into_iter()
        .filter(|i| i.kind == ItemKind::Claim)
        .map(|i| i.id)
        .collect()
}

#[test]
fn a_retired_claim_leaves_every_reader_and_comes_back_on_a_revert() {
    let w = world();
    w.machine_claim(0);
    w.approve_pending();
    let claim = w.claims()[0].clone();
    let id = claim.header.id.clone();
    let (page_id, _) = w
        .s
        .propose_new_page(
            &w.admin,
            &w.v,
            NewPage {
                title: "Fakta".into(),
                body: format!("Se {{{{claim:{id}}}}} här."),
                slug: None,
                idempotency_key: "page".into(),
                rationale: "notes".into(),
            },
            false,
        )
        .unwrap();
    w.project();
    assert_eq!(searched(&w, "Fartyget nummer"), vec![id.clone()], "it's found");
    let view = w.s.get_wiki_page(&w.admin, &w.v, &page_id, None).unwrap();
    assert_eq!(view.claims[0].status, ClaimRefStatus::Current);
    let before = w.s.engine().store().snapshot(&w.v).unwrap().head().unwrap().unwrap();

    let retirement = retire(&w, &claim);
    w.project();
    assert_eq!(retirement.state, ChangeSetState::Committed);
    assert_eq!(retirement.decided_by.as_deref(), Some(w.admin.principal.id.as_str()));
    assert!(searched(&w, "Fartyget nummer").is_empty(), "the keyword index and the fence let it go");
    let view = w.s.get_wiki_page(&w.admin, &w.v, &page_id, None).unwrap();
    assert_eq!(view.claims[0].status, ClaimRefStatus::Missing, "a page can't show what nothing stands for");
    assert!(view.claims[0].current.is_none());
    let row = w.claims().into_iter().find(|c| c.header.id == id).unwrap();
    assert_eq!((row.header.status, row.superseded_by), (Status::Superseded, None), "the row is kept, marked");
    let after_seq = w.s.engine().store().snapshot(&w.v).unwrap().head().unwrap().unwrap();
    assert!(w.s.believed_at(&w.admin, &w.v, &claim.subject, after_seq).unwrap().is_empty());
    assert_eq!(w.s.believed_at(&w.admin, &w.v, &claim.subject, before).unwrap().len(), 1, "what was believed then stays");
    // The past still has it, as `as_of` replays the log.
    let as_of = |seq| {
        let mut q = SearchRequest::new("Fartyget nummer");
        q.mode = SearchMode::Lexical;
        q.as_of = Some(AsOf::Seq(seq));
        w.s.search(&w.admin, &w.v, &q)
            .unwrap()
            .items
            .iter()
            .filter(|i| i.kind == ItemKind::Claim)
            .count()
    };
    assert_eq!(as_of(before), 1, "as of before the retirement");
    assert_eq!(as_of(after_seq), 0, "as of after it");

    // The same claim can be proposed again, as nothing active holds it.
    let revert = w.s.revert(&w.admin, &w.v, &retirement.header.id, "it was right".into()).unwrap();
    w.project();
    assert_eq!(revert.reverts.as_ref(), Some(&retirement.header.id));
    assert_eq!(searched(&w, "Fartyget nummer"), vec![id.clone()], "back in search");
    let view = w.s.get_wiki_page(&w.admin, &w.v, &page_id, None).unwrap();
    assert_eq!(view.claims[0].status, ClaimRefStatus::Current);
    let row = w.claims().into_iter().find(|c| c.header.id == id).unwrap();
    assert_eq!(row, claim, "exactly as it was");
    let seq = w.s.engine().store().snapshot(&w.v).unwrap().head().unwrap().unwrap();
    assert_eq!(as_of(seq), 1);
}

#[test]
fn retiring_an_entity_s_claim_changes_its_field_and_card_and_a_revert_restores_them() {
    let w = world();
    let text = long_text("Anna Berg leder projektet och Anna Lund håller i budgeten.");
    let source = w.upload("e.txt", "trusted", &text);
    w.parse(&source, &text);
    let segment = w
        .table(Table::Segment)
        .into_iter()
        .find_map(|r| if let Row::Segment(s) = r { Some(s.header.id) } else { None })
        .unwrap();
    let entity = ObjectId("anna".into());
    let field = |id: &str, name: &str, quote: &str| Op::AddAssertion {
        id: ObjectId(id.into()),
        subject: memex_types::entity_subject(&entity),
        predicate: "name".into(),
        value: json!(name),
        epistemic: Epistemic::Asserted,
        machine: false,
        confidence_permille: 1000,
        valid_from_ms: None,
        valid_to_ms: None,
        evidence: vec![(
            ObjectId(format!("{id}-e")),
            EvidenceInput {
                segment_id: segment.clone(),
                quote: quote.into(),
                stance: Stance::Supports,
            },
        )],
    };
    let made =
        w.s.propose(
            &w.admin,
            &w.v,
            Proposal {
                ops: vec![
                    Op::CreateEntity {
                        id: entity.clone(),
                        type_name: "Person".into(),
                        aliases: vec![],
                    },
                    field("older", "Anna Berg", "Anna Berg"),
                ],
                rationale: "read it".into(),
                idempotency_key: "entity".into(),
            },
            false,
        )
        .unwrap();
    assert_eq!(made.changeset.unwrap().state, ChangeSetState::Committed);
    w.advance(1000);
    let second =
        w.s.propose(
            &w.admin,
            &w.v,
            Proposal {
                ops: vec![field("newer", "Anna Lund", "Anna Lund")],
                rationale: "read it".into(),
                idempotency_key: "entity2".into(),
            },
            false,
        )
        .unwrap();
    assert_eq!(second.changeset.unwrap().state, ChangeSetState::Committed);
    w.project();
    let name_of = || {
        let view = w.s.get_entity(&w.admin, &w.v, &entity).unwrap();
        view.entity.fields.iter().find(|f| f.field == "name").map(|f| (f.assertion_id.clone(), f.value.clone()))
    };
    assert_eq!(name_of().unwrap().1, json!("Anna Lund"), "the newest stands");
    let newest = w.claims().into_iter().find(|c| c.header.id.0 == "newer").unwrap();
    let retirement = retire(&w, &newest);
    w.project();
    assert_eq!(name_of().unwrap(), (ObjectId("older".into()), json!("Anna Berg")), "the field falls back to what still stands");
    let mut q = SearchRequest::new("Lund");
    q.mode = SearchMode::Lexical;
    q.filter.kinds = vec![DocKind::Entity];
    let found = w.s.search(&w.admin, &w.v, &q).unwrap().items;
    assert!(found.is_empty(), "the card has let the retired name go: {found:?}");
    w.s.revert(&w.admin, &w.v, &retirement.header.id, "it was right".into()).unwrap();
    w.project();
    assert_eq!(name_of().unwrap().1, json!("Anna Lund"));
    assert_eq!(w.s.search(&w.admin, &w.v, &q).unwrap().items.len(), 1);
}

#[test]
fn retiring_the_only_claim_behind_a_required_field_leaves_the_entity_without_it() {
    let w = world();
    let text = long_text("Anna Berg leder projektet.");
    let source = w.upload("e.txt", "trusted", &text);
    w.parse(&source, &text);
    let segment = w
        .table(Table::Segment)
        .into_iter()
        .find_map(|r| if let Row::Segment(s) = r { Some(s.header.id) } else { None })
        .unwrap();
    let entity = ObjectId("anna".into());
    let made =
        w.s.propose(
            &w.admin,
            &w.v,
            Proposal {
                ops: vec![
                    Op::CreateEntity {
                        id: entity.clone(),
                        type_name: "Person".into(),
                        aliases: vec![],
                    },
                    Op::AddAssertion {
                        id: ObjectId("only".into()),
                        subject: memex_types::entity_subject(&entity),
                        predicate: "name".into(),
                        value: json!("Anna Berg"),
                        epistemic: Epistemic::Asserted,
                        machine: false,
                        confidence_permille: 1000,
                        valid_from_ms: None,
                        valid_to_ms: None,
                        evidence: vec![(
                            ObjectId("only-e".into()),
                            EvidenceInput {
                                segment_id: segment,
                                quote: "Anna Berg".into(),
                                stance: Stance::Supports,
                            },
                        )],
                    },
                ],
                rationale: "read it".into(),
                idempotency_key: "entity".into(),
            },
            false,
        )
        .unwrap();
    assert_eq!(made.changeset.unwrap().state, ChangeSetState::Committed);
    let claim = w.claims().remove(0);
    let retirement = retire(&w, &claim);
    assert_eq!(retirement.state, ChangeSetState::Committed, "a claim found wrong can't be kept in place by its type's schema");
    let view = w.s.get_entity(&w.admin, &w.v, &entity).unwrap();
    assert!(view.entity.fields.is_empty());
    w.s.revert(&w.admin, &w.v, &retirement.header.id, "it was right".into()).unwrap();
    assert_eq!(w.s.get_entity(&w.admin, &w.v, &entity).unwrap().entity.fields.len(), 1);
}

#[test]
fn a_retirement_is_replayed_the_same_way_from_the_log() {
    let w = world();
    w.machine_claim(0);
    w.approve_pending();
    let claim = w.claims()[0].clone();
    retire(&w, &claim);
    let dump = memex_core::dump_store(&w.s.engine().store().snapshot(&w.v).unwrap()).unwrap().sha256;
    let copy = MemStore::new();
    let lease = copy.open_vault(&w.v).unwrap();
    let events = memex_core::all_events(&w.s.engine().store().snapshot(&w.v).unwrap()).unwrap();
    memex_core::replay(events, &copy, &lease).unwrap();
    assert_eq!(memex_core::dump_store(&copy.snapshot(&w.v).unwrap()).unwrap().sha256, dump);
}

#[test]
fn a_retired_claim_can_not_be_retired_twice() {
    let w = world();
    w.machine_claim(0);
    w.approve_pending();
    let claim = w.claims()[0].clone();
    let first = retire(&w, &claim);
    assert_eq!(first.state, ChangeSetState::Committed);
    let again =
        w.s.propose(
            &w.admin,
            &w.v,
            Proposal {
                ops: vec![Op::RetractAssertion {
                    target: claim.header.id.clone(),
                    base_version: claim.header.version,
                    counts_against: false,
                }],
                rationale: "again".into(),
                idempotency_key: "again".into(),
            },
            false,
        )
        .unwrap()
        .changeset
        .unwrap();
    let refused = w.s.approve(&w.admin, &w.v, &again.header.id, None).unwrap_err();
    assert!(matches!(refused, ServerError::StaleVersion { .. }), "{refused}");
}

// Item 4: budgets.

fn budget_world() -> World {
    let w = world();
    for n in 0..6 {
        w.machine_claim(n);
    }
    w.approve_pending();
    w
}

#[test]
fn the_run_budget_stops_the_run_cleanly_and_records_that() {
    let w = budget_world();
    let agent = ScriptedAgent::answering(scripted_oracle);
    // The first claim's entailment fits, and little else.
    w.s.trigger_audit(
        &w.admin,
        &w.v,
        TriggerAudit {
            seed: Some(1),
            sample_size: Some(6),
            run_token_budget: Some(800),
        },
    )
    .unwrap();
    let run = w.run_job(Some(&agent));
    assert_eq!(run.state, AuditRunState::Finished, "stopped by budget is a finish, not a failure");
    assert_eq!(run.stop, Some(AuditStop::RunBudget));
    assert!(run.stop_detail.as_deref().unwrap().contains("800"), "{:?}", run.stop_detail);
    assert_eq!(run.sampled, 6);
    assert!(run.llm_skipped >= 5, "{}", run.llm_skipped);
    assert!(run.usage.total() <= 800, "{}", run.usage.total());
    assert_eq!(run.usage.total(), u64::from(run.llm_calls) * USAGE.total());
    let job = w.s.get_job(&w.admin, &w.v, &run.job_id).unwrap();
    assert_eq!(job.state, JobState::Done);
    // Everything got its deterministic checks, spent or not.
    for c in w.checks(&run) {
        assert!(c.checks.iter().any(|k| k.check == Kind::QuoteRelocation));
        assert_ne!(c.outcome, AuditOutcome::Sampled);
    }
}

#[test]
fn a_budget_of_nothing_makes_no_calls() {
    let w = budget_world();
    let agent = ScriptedAgent::answering(scripted_oracle);
    w.s.trigger_audit(
        &w.admin,
        &w.v,
        TriggerAudit {
            seed: Some(1),
            sample_size: Some(6),
            run_token_budget: Some(0),
        },
    )
    .unwrap();
    let run = w.run_job(Some(&agent));
    assert!(agent.requests().is_empty());
    assert_eq!(run.stop, Some(AuditStop::RunBudget));
    assert_eq!((run.passed, run.llm_skipped), (6, 6));
}

#[test]
fn the_day_budget_holds_across_runs_and_starts_again_the_next_day() {
    let w = budget_world();
    let agent = ScriptedAgent::answering(scripted_oracle);
    w.set_schedule(|s| s.day_token_budget = 1000);
    let spent = w.audit(1, Some(&agent));
    assert_eq!(spent.stop, Some(AuditStop::DayBudget));
    let used = spent.usage.total();
    assert!(used > 0);
    let budget = w.s.audit_budget(&w.admin, &w.v).unwrap();
    assert_eq!(budget.day_spent, used);
    assert_eq!(budget.day_remaining, 1000 - used);
    let next = w.audit(2, Some(&agent));
    assert_eq!(next.stop, Some(AuditStop::DayBudget));
    let today = w.s.audit_budget(&w.admin, &w.v).unwrap().day_spent;
    assert_eq!(today, used + next.usage.total(), "it counts across runs");
    assert!(today <= 1000, "{today}");
    // Small calls may still fit for a while. Once nothing does, nothing more is sent today.
    for seed in 3..30 {
        assert_eq!(w.audit(seed, Some(&agent)).stop, Some(AuditStop::DayBudget));
    }
    let settled = agent.requests().len();
    w.audit(30, Some(&agent));
    assert_eq!(agent.requests().len(), settled);
    assert!(w.s.audit_budget(&w.admin, &w.v).unwrap().day_spent <= 1000);
    w.advance(DAY_MS);
    assert_eq!(w.s.audit_budget(&w.admin, &w.v).unwrap().day_spent, 0);
    let tomorrow = w.audit(7, Some(&agent));
    assert!(tomorrow.llm_calls > 0, "a new day, and a new budget");
    assert_eq!(w.s.audit_budget(&w.admin, &w.v).unwrap().day_spent, tomorrow.usage.total());
}

#[test]
fn a_retry_that_would_not_fit_is_not_made_and_the_tokens_are_on_record() {
    let w = world();
    w.machine_claim(0);
    w.approve_pending();
    let big = AgentUsage {
        input_tokens: 400,
        ..AgentUsage::default()
    };
    let agent = ScriptedAgent::new(vec![Err(AgentError::Truncated { usage: big })]);
    w.s.trigger_audit(
        &w.admin,
        &w.v,
        TriggerAudit {
            seed: Some(1),
            sample_size: Some(1),
            run_token_budget: Some(650),
        },
    )
    .unwrap();
    let run = w.run_job(Some(&agent));
    assert_eq!(agent.requests().len(), 1);
    assert_eq!(run.stop, Some(AuditStop::RunBudget));
    assert_eq!(run.usage.total(), big.total());
}

// Item 5: scope.

#[test]
fn the_audit_agent_can_propose_and_nothing_else() {
    let w = world();
    w.earn_auto_commit();
    w.machine_claim(0);
    w.machine_claim(1);
    let claim = w.claims()[0].clone();
    w.break_quote(&claim.header.id);
    w.audit(1, None);
    let item = w.pending_audit_items().remove(0);
    let auditor = w.s.audit_agent_caller(&w.v);
    assert_eq!(auditor.principal.kind, PrincipalKind::Agent);
    assert!(auditor.grants.iter().all(|g| g.scope == Scope::Propose), "{:?}", auditor.grants);
    let forbidden = |r: Result<(), ServerError>, what: &str| assert!(matches!(r, Err(ServerError::Forbidden)), "{what}: {r:?}");
    forbidden(w.s.approve(&auditor, &w.v, &item.header.id, None).map(|_| ()), "approve");
    forbidden(w.s.reject(&auditor, &w.v, &item.header.id, "no".into()).map(|_| ()), "reject");
    forbidden(w.s.approve_edited(&auditor, &w.v, &item.header.id, vec![], None).map(|_| ()), "edit");
    forbidden(w.s.revert(&auditor, &w.v, &claim.changeset_id, "no".into()).map(|_| ()), "revert");
    forbidden(w.s.decide_batch(&auditor, &w.v, vec![]).map(|_| ()), "batch");
    let combination = w.combination().row.header.id;
    forbidden(w.s.disable_auto_commit(&auditor, &w.v, &combination, "no".into()).map(|_| ()), "disable");
    forbidden(w.s.trigger_audit(&auditor, &w.v, TriggerAudit::default()).map(|_| ()), "trigger");
    let (schedule, _, _) = w.s.audit_schedule(&w.admin, &w.v).unwrap();
    forbidden(w.s.set_audit_schedule(&auditor, &w.v, schedule).map(|_| ()), "schedule");
    forbidden(w.s.mark_audit_test_vault(&auditor, &w.v), "test vault");
    forbidden(w.s.plant_audit_errors(&auditor, &w.v, 1, 1, &[PlantedKind::WrongValue]).map(|_| ()), "plant");
    // A direct write of a claim's row, around the proposal flow altogether.
    let mut changed = claim.clone();
    changed.value = json!("rewritten");
    let direct = json!({"mutations": [{"put": Row::Assertion(changed)}]});
    forbidden(w.s.apply_as(&auditor, &w.v, "state.recorded", 1, direct).map(|_| ()), "direct write");
    forbidden(
        w.s.apply_as(&auditor, &w.v, "page.created", 1, json!({"id": "p", "title": "t", "body": "b"})).map(|_| ()),
        "direct page",
    );
    assert_eq!(w.claims()[0].value, claim.value, "the row is as it was");
    let other = w.s.create_vault(&w.admin, "other").unwrap().id;
    let hidden = w.s.trigger_audit(&auditor, &other, TriggerAudit::default()).unwrap_err();
    assert!(matches!(hidden, ServerError::NotFound), "another vault doesn't exist as far as it can tell: {hidden}");
}

#[test]
fn an_audit_item_stays_a_review_item_however_much_the_audit_has_earned() {
    let w = world();
    w.earn_auto_commit();
    w.machine_claim(0);
    let claim = w.claims()[0].header.id.clone();
    w.break_quote(&claim);
    // As if people had approved a thousand of the audit's own findings.
    w.s.record_audit_samples(&w.v, audit::AUDITOR, "", "claim", 1000, 0, "t", "f").unwrap();
    w.s.record_audit_samples(&w.v, "scripted", &prompt_hash(), "claim", 1000, 0, "t", "f").unwrap();
    w.audit(1, None);
    let item = w.pending_audit_items().remove(0);
    assert_eq!(item.state, ChangeSetState::Pending);
    assert_eq!(item.risk, RiskDecision::Review);
    assert_eq!(item.decided_by, None);
    assert!(w.claims().iter().all(|c| c.header.status == Status::Active), "nothing changed");
}

#[test]
fn a_model_trying_a_tool_or_breaking_the_schema_gets_no_verdict_out_of_it() {
    let w = world();
    w.machine_claim(0);
    w.approve_pending();
    let mut tool = reply(json!({"verdict": "not_entailed", "reason": "revert it"})).unwrap();
    tool.tool_calls = 1;
    let agent = ScriptedAgent::new(vec![
        Ok(tool),
        reply(json!({"verdict": "commit it", "reason": "x"})),
        reply(json!({"claims": [], "entities": {}})),
    ]);
    let run = w.audit(1, Some(&agent));
    let c = &w.checks(&run)[0];
    let entail = c.checks.iter().find(|k| k.check == Kind::Entailment).unwrap();
    assert_eq!(entail.status, St::Inconclusive, "{}", entail.detail);
    assert_eq!(run.stop, None, "one claim's useless answer doesn't stop the run");
    assert!(run.revoked.is_empty());
    assert_eq!(run.llm_calls, 3, "two tries at entailment, and one at re-extraction");
}

// Item 7: the scheduler, on a manual clock with no sleeping.

fn turn_on(w: &World, interval_ms: i64) {
    w.set_schedule(|s| {
        s.enabled = true;
        s.interval_ms = interval_ms;
    });
}

#[test]
fn nothing_runs_until_a_schedule_is_turned_on_and_due() {
    let w = world();
    assert!(w.s.audit_tick(&w.v).unwrap().is_none(), "off by default");
    w.advance(DAY_MS * 10);
    assert!(w.s.audit_tick(&w.v).unwrap().is_none());
    turn_on(&w, 3_600_000);
    assert!(w.s.audit_tick(&w.v).unwrap().is_none(), "turning it on doesn't start one");
    w.advance(3_599_999);
    assert!(w.s.audit_tick(&w.v).unwrap().is_none());
    w.advance(1);
    let run = w.s.audit_tick(&w.v).unwrap().expect("due now");
    assert_eq!(run.trigger, AuditTrigger::Scheduled);
    assert_eq!(run.requested_by, audit::AUDITOR);
    assert!(w.s.audit_tick(&w.v).unwrap().is_none(), "and not again this slot");
}

#[test]
fn an_empty_vault_finishes_cleanly_with_nothing_sampled() {
    let w = world();
    turn_on(&w, 60_000);
    w.advance(60_000);
    w.s.audit_tick_all().unwrap();
    let agent = ScriptedAgent::answering(scripted_oracle);
    let run = w.run_job(Some(&agent));
    assert_eq!(run.state, AuditRunState::Finished);
    assert_eq!((run.population, run.sampled, run.passed, run.failed), (0, 0, 0, 0));
    assert_eq!(run.stop, None);
    assert!(agent.requests().is_empty());
    assert!(w.pending_audit_items().is_empty());
}

#[test]
fn a_missed_schedule_runs_once_and_says_how_many_slots_it_passed_over() {
    let w = world();
    turn_on(&w, 60_000);
    w.advance(60_000 + 5 * 60_000 + 30_000);
    let run = w.s.audit_tick(&w.v).unwrap().expect("one run");
    assert_eq!(run.missed_slots, 5);
    assert!(w.s.audit_tick(&w.v).unwrap().is_none(), "no catching up");
    let (_, next, missed) = w.s.audit_schedule(&w.admin, &w.v).unwrap();
    assert_eq!(missed, 5);
    assert_eq!(next, Some(run.created_at_ms + 30_000), "the next slot on the original grid");
}

#[test]
fn a_slot_that_finds_a_run_still_open_is_passed_over_and_counted() {
    let w = world();
    turn_on(&w, 60_000);
    w.advance(60_000);
    assert!(w.s.audit_tick(&w.v).unwrap().is_some());
    w.advance(60_000);
    assert!(w.s.audit_tick(&w.v).unwrap().is_none(), "the first hasn't run yet");
    assert_eq!(w.s.audit_schedule(&w.admin, &w.v).unwrap().2, 1);
    w.run_job(None);
    w.advance(60_000);
    assert!(w.s.audit_tick(&w.v).unwrap().is_some());
}

#[test]
fn turning_a_schedule_off_stops_it_and_a_bad_one_is_refused() {
    let w = world();
    turn_on(&w, 60_000);
    w.set_schedule(|s| s.enabled = false);
    w.advance(600_000);
    assert!(w.s.audit_tick(&w.v).unwrap().is_none());
    let mut bad = w.s.audit_schedule(&w.admin, &w.v).unwrap().0;
    bad.sample_size = 0;
    assert!(matches!(w.s.set_audit_schedule(&w.admin, &w.v, bad), Err(ServerError::Invalid(_))));
}

#[test]
fn an_llm_outage_leaves_the_deterministic_checks_done_and_the_run_finished() {
    let w = world();
    w.earn_auto_commit();
    for n in 0..4 {
        w.machine_claim(n);
    }
    let broken = w.claims()[2].header.id.clone();
    w.break_quote(&broken);
    let agent = ScriptedAgent::new(vec![Err(AgentError::Unavailable("connection refused".into()))]);
    let run = w.audit(1, Some(&agent));
    assert_eq!(run.state, AuditRunState::Finished);
    assert_eq!(run.stop, Some(AuditStop::LlmUnavailable));
    assert_eq!(run.failed, 1, "the deterministic checks found the broken quote anyway");
    assert!(!w.auto_commit_on(), "and that still revokes");
    assert_eq!(agent.requests().len(), 1, "one try at an endpoint that's down");
    assert_eq!(run.llm_skipped, 4, "everything, as the failed claim had nothing to show a model anyway");
    assert_eq!(run.passed, 3);
    assert_eq!(w.checks(&run).iter().filter(|c| c.outcome == AuditOutcome::Sampled).count(), 0);
}

#[test]
fn a_provider_refusal_also_stops_the_llm_checks_and_says_so() {
    let w = world();
    w.machine_claim(0);
    w.approve_pending();
    let agent = ScriptedAgent::new(vec![Err(AgentError::Provider("no such model".into()))]);
    let run = w.audit(1, Some(&agent));
    assert_eq!(run.stop, Some(AuditStop::LlmRefused));
    assert!(run.stop_detail.unwrap().contains("no such model"));
}

#[test]
fn with_no_auditor_set_nothing_is_sent_however_it_is_called() {
    let w = world();
    w.machine_claim(0);
    w.approve_pending();
    w.s.set_auditor(None);
    let agent = ScriptedAgent::answering(scripted_oracle);
    let run = w.audit(1, Some(&agent));
    assert!(agent.requests().is_empty());
    assert_eq!(run.model, None);
    assert_eq!(run.passed, 1);
}

// Crashes.

#[test]
fn a_run_that_lost_its_worker_carries_on_with_the_same_sample_and_no_second_call() {
    let w = world();
    for n in 0..3 {
        w.machine_claim(n);
    }
    w.approve_pending();
    w.trigger(5);
    let held: JobRow = w.s.claim_job(&w.v, AUDIT_JOB, "w1").unwrap().unwrap();
    // A call that panics partway through the second claim stands in for a crash.
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let agent = ScriptedAgent::answering(move |req| {
        if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 2 {
            return Err(AgentError::Provider("the worker died here".into()));
        }
        scripted_oracle(req)
    });
    let rows = w.s.run_audit_job(&w.v, &held, Some(&agent)).unwrap();
    assert!(w.s.complete_job(&w.v, &held, rows).unwrap());
    let run = w.s.list_audit_runs(&w.admin, &w.v, None, 10).unwrap().remove(0).run;
    assert_eq!(run.stop, Some(AuditStop::LlmRefused));
    let usage = run.usage;
    // Running the same job again is a no-op once it has finished.
    let again = w.s.run_audit_job(&w.v, &held, Some(&agent)).unwrap();
    assert!(again.is_empty());
    assert_eq!(w.s.list_audit_runs(&w.admin, &w.v, None, 10).unwrap().remove(0).run.usage, usage);
}

#[test]
fn a_stolen_job_writes_nothing() {
    let w = world();
    w.machine_claim(0);
    w.approve_pending();
    w.trigger(5);
    let held = w.s.claim_job(&w.v, AUDIT_JOB, "w1").unwrap().unwrap();
    w.s.cancel_job(&w.admin, &w.v, &held.header.id).unwrap();
    let out = w.s.run_audit_job(&w.v, &held, None::<&ScriptedAgent>);
    assert!(matches!(out, Err(ExtractJobError::Lost)), "{out:?}");
    assert!(w.table(Table::AuditCheck).is_empty());
}

// Index reconciliation.

#[test]
fn counts_and_hashes_match_on_a_settled_index_and_show_what_went_missing() {
    let w = world();
    w.s.mark_audit_test_vault(&w.admin, &w.v).unwrap();
    w.earn_auto_commit();
    for n in 0..5 {
        w.machine_claim(n);
    }
    w.project();
    let clean = w.audit(1, None);
    let kw: Vec<_> = clean
        .vault_findings
        .iter()
        .filter(|f| f.check == Kind::IndexReconciliation && f.detail.starts_with("keyword index"))
        .collect();
    assert_eq!(kw.len(), 2, "{:?}", clean.vault_findings);
    assert!(kw.iter().all(|f| f.status == St::Pass), "{kw:?}");
    assert!(kw[0].detail.contains("5 in both"), "{}", kw[0].detail);

    w.s.plant_audit_errors(&w.admin, &w.v, 1, 1, &[PlantedKind::IndexMissing]).unwrap();
    w.project();
    let drifted = w.audit(2, None);
    let claims = drifted
        .vault_findings
        .iter()
        .find(|f| f.detail.starts_with("keyword index claims"))
        .unwrap();
    assert_eq!(claims.status, St::Fail, "{}", claims.detail);
    assert!(claims.detail.contains("the store has 5 and the index 4"), "{}", claims.detail);
    assert_eq!(drifted.failed, 0, "no claim is wrong");
    assert_eq!(drifted.suspect, 1, "the one the index lost is named");
}

#[test]
fn an_index_behind_on_what_matters_is_inconclusive_rather_than_a_finding() {
    let w = world();
    w.earn_auto_commit();
    w.machine_claim(0);
    w.project();
    w.machine_claim(1);
    let run = w.audit(1, None);
    let claims = run.vault_findings.iter().find(|f| f.detail.starts_with("keyword index claims")).unwrap();
    assert_eq!(claims.status, St::Inconclusive, "{}", claims.detail);
    assert_eq!((run.suspect, run.failed), (0, 0), "the new claim simply hasn't been indexed yet");
}

#[test]
fn an_index_behind_on_things_that_do_not_matter_can_still_be_compared() {
    let w = world();
    w.earn_auto_commit();
    w.machine_claim(0);
    w.project();
    w.audit(1, None);
    let run = w.audit(2, None);
    let claims = run.vault_findings.iter().find(|f| f.detail.starts_with("keyword index claims")).unwrap();
    assert_eq!(claims.status, St::Pass, "an audit's own events don't touch anything an index holds: {}", claims.detail);
}

#[test]
fn page_links_that_lead_nowhere_are_reported_for_the_vault() {
    let w = world();
    w.s.propose_new_page(
        &w.admin,
        &w.v,
        NewPage {
            title: "Trasig".into(),
            body: "Se [[Ingenstans]].".into(),
            slug: None,
            idempotency_key: "p".into(),
            rationale: "notes".into(),
        },
        false,
    )
    .unwrap();
    let run = w.audit(1, None);
    let links = run.vault_findings.iter().find(|f| f.check == Kind::LinkRot).unwrap();
    assert_eq!(links.status, St::Fail);
    assert!(links.detail.contains("1 page links"), "{}", links.detail);
}

// Results are events.

#[test]
fn a_run_is_in_the_log_and_replays_to_the_same_state() {
    let w = world();
    w.earn_auto_commit();
    for n in 0..3 {
        w.machine_claim(n);
    }
    let first = w.claims()[0].header.id.clone();
    w.break_quote(&first);
    w.audit(1, Some(&ScriptedAgent::answering(scripted_oracle)));
    let dump = memex_core::dump_store(&w.s.engine().store().snapshot(&w.v).unwrap()).unwrap().sha256;
    let copy = MemStore::new();
    let lease = copy.open_vault(&w.v).unwrap();
    let events = memex_core::all_events(&w.s.engine().store().snapshot(&w.v).unwrap()).unwrap();
    memex_core::replay(events, &copy, &lease).unwrap();
    assert_eq!(memex_core::dump_store(&copy.snapshot(&w.v).unwrap()).unwrap().sha256, dump, "with no LLM called");
    let snap = copy.snapshot(&w.v).unwrap();
    assert!(!snap.rows(Table::AuditRun, None).unwrap().is_empty());
    assert!(!snap.rows(Table::AuditCheck, None).unwrap().is_empty());
}

#[test]
fn reading_a_large_sample_makes_store_calls_that_do_not_grow_with_it() {
    let count = |n: usize| {
        let w = world();
        w.earn_auto_commit();
        for i in 0..n {
            w.machine_claim(i);
        }
        let store = w.s.engine().store();
        let snap = store.snapshot(&w.v).unwrap();
        let claims = w.claims();
        let before = store.counts().reads.load(std::sync::atomic::Ordering::Relaxed);
        memex_core::audit::is_candidate(&claims[0]);
        let _ = snap.id_digest(DocKind::Claim).unwrap();
        store.counts().reads.load(std::sync::atomic::Ordering::Relaxed) - before
    };
    assert_eq!(count(2), count(20));
}

#[test]
fn schedule_ttls_reach_the_staleness_check() {
    let w = world();
    w.earn_auto_commit();
    w.machine_claim(0);
    w.set_schedule(|s| s.ttl_days = BTreeMap::from([("byggdes i".to_string(), 3)]));
    assert_eq!(w.audit(1, None).suspect, 0);
    w.advance(3 * DAY_MS);
    let run = w.audit(2, None);
    assert_eq!(run.suspect, 1);
    assert!(w.auto_commit_on(), "time passing is nobody's error");
    let c = &w.checks(&run)[0];
    assert!(c.checks.iter().any(|k| k.check == Kind::Staleness && k.cause.as_deref() == Some("past_ttl")));
}
