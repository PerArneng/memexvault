//! The review gate through the facade, on fakes. Agent replies are scripted here, so these tests prove the mechanisms around a model and never a model's judgement.
use memex_core::testing::{ManualClock, MemBlobStore, MemRegistry, MemStore, ScriptedAgent, SeededRandom};
use memex_core::{
    Caller, EXTRACT_JOB, ExtractJobError, ExtractionLimits, NewPage, NewToken, Proposal, RENDITION_JOB, REVIEW_JOB, RenditionInput, ReviewLimits, Server,
    ServerEdges, ServerError, Snapshot, Store, UploadLimits, UploadRequest, dump_store,
};
use memex_types::{
    AgentError, AgentReply, AgentUsage, AutoCommitChangeKind, ChangeSetRow, ChangeSetState, Epistemic, EvidenceInput, Grant, JobState, ObjectId, Op, PageClass,
    PageFacts, PageFlag, PageOutcome, PageRoute, ParsedDocument, PrincipalKind, Rect, ReviewFilter, RiskDecision, Row, Scope, Stance, Table, VaultId, Verdict,
    VerdictFilter, Word,
};
use serde::Deserialize;
use serde_json::{Value, json};
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
    other_admin: Caller,
    agent: Caller,
    agent_admin: Caller,
    reader: Caller,
    v: VaultId,
}

const USAGE: AgentUsage = AgentUsage {
    input_tokens: 70,
    output_tokens: 9,
    thinking_tokens: 1,
};

fn world() -> World {
    let s = Server::<Fakes>::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(1_000_000),
        SeededRandom::new(25),
        memex_core::testing::mem_search(),
    );
    let admin = s.authenticate(&s.bootstrap("root").unwrap().token).unwrap();
    let v = s.create_vault(&admin, "v").unwrap().id;
    let make = |name: &str, kind, scope| {
        let p = s.create_principal(&admin, name, kind, false).unwrap();
        let t = s
            .create_token(
                &admin,
                NewToken {
                    principal_id: Some(p.id),
                    name: "t".into(),
                    grants: vec![Grant { vault: v.clone(), scope }],
                    expires_at_ms: None,
                },
            )
            .unwrap();
        s.authenticate(&t.token).unwrap()
    };
    let other_admin = make("second person", PrincipalKind::Human, Scope::Admin);
    let agent = make("agent", PrincipalKind::Agent, Scope::Propose);
    let agent_admin = make("agent admin", PrincipalKind::Agent, Scope::Admin);
    let reader = make("reader", PrincipalKind::Human, Scope::Read);
    s.set_extraction(Some(ExtractionLimits::default()));
    s.set_reviewer(Some(ReviewLimits::default()));
    World {
        s,
        admin,
        other_admin,
        agent,
        agent_admin,
        reader,
        v,
    }
}

const SENTENCE: &str = "Fartyget nummer 0 byggdes i Stockholm år 1600.";

fn long_text(first: &str) -> String {
    format!(
        "{first} {}",
        "Detta är en lång mening om ingenting särskilt som bara fyller ut stycket med ord. ".repeat(12)
    )
}

fn reply(doc: Value) -> Result<AgentReply, AgentError> {
    Ok(AgentReply {
        json: doc.to_string(),
        usage: USAGE,
        tool_calls: 0,
    })
}

fn claim_reply(quote: &str, subject: &str, predicate: &str, value: &str) -> Result<AgentReply, AgentError> {
    reply(json!({"claims": [{"segment_id": "s1", "quote": quote, "subject": subject, "predicate": predicate, "value": value}], "entities": {}}))
}

fn verdict_reply(verdict: &str, reason: &str) -> Result<AgentReply, AgentError> {
    reply(json!({"verdict": verdict, "reason": reason}))
}

fn prompt_hash() -> String {
    memex_core::extraction::build_prompt(memex_core::entity::seed_types()).hash
}

impl World {
    fn upload(&self, name: &str, tier: &str, metadata: Value, text: &str) -> ObjectId {
        let req = UploadRequest {
            name: name.into(),
            trust_tier: tier.into(),
            metadata,
            replaces: None,
        };
        self.s
            .upload(&self.admin, &self.v, req, UploadLimits::default(), &mut text.as_bytes())
            .unwrap()
            .source
            .header
            .id
    }

    fn parse(&self, source: &ObjectId, input: RenditionInput) {
        let job = self.s.claim_job(&self.v, RENDITION_JOB, "parser").unwrap().unwrap();
        let rows = self.s.build_rendition(&self.v, source, input).unwrap();
        assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
    }

    fn plain(text: &str) -> RenditionInput {
        RenditionInput::Plain {
            text: text.into(),
            parser: "plain-1".into(),
        }
    }

    fn ingest(&self, name: &str, tier: &str, text: &str) -> ObjectId {
        let source = self.upload(name, tier, json!({}), text);
        self.parse(&source, Self::plain(text));
        source
    }

    /// One extraction job, run to the end.
    fn extract(&self, agent: &ScriptedAgent) {
        let job = self.s.claim_job(&self.v, EXTRACT_JOB, "w").unwrap().expect("an extraction job");
        let rows = self.s.run_extraction_job(&self.v, &job, agent).unwrap();
        assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
    }

    /// One reviewer job. Returns how it ended.
    fn review_one(&self, agent: &ScriptedAgent) -> Result<(), ExtractJobError> {
        let job = self.s.claim_job(&self.v, REVIEW_JOB, "w").unwrap().expect("a review job");
        match self.s.run_review_job(&self.v, &job, agent) {
            Ok(rows) => {
                assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
                Ok(())
            }
            Err(ExtractJobError::Wait(m)) => {
                assert!(self.s.wait_job(&self.v, &job, &m).unwrap());
                Err(ExtractJobError::Wait(m))
            }
            Err(e) => Err(e),
        }
    }

    fn changesets(&self) -> Vec<ChangeSetRow> {
        self.s.list_changesets(&self.admin, &self.v, None, None, 1000).unwrap()
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

    fn committed_claims(&self) -> usize {
        self.table(Table::Assertion).len()
    }

    /// A pending claim from an extractor, from a document of its own.
    fn pending_claim(&self, n: usize) -> ChangeSetRow {
        let sentence = format!("Fartyget nummer {n} byggdes i Stockholm år 16{:02}.", n % 100);
        self.ingest(&format!("d{n}.txt"), "trusted", &long_text(&sentence));
        let agent = ScriptedAgent::new(vec![claim_reply(&sentence, &format!("Fartyget {n}"), "byggdes i", "Stockholm")]);
        self.extract(&agent);
        let last = self.changesets().pop().unwrap();
        assert_eq!(last.state, ChangeSetState::Pending);
        last
    }

    fn samples(&self, extractor: &str, prompt: &str, kind: &str, correct: u64, incorrect: u64) -> memex_types::AutoCommitRow {
        self.s
            .record_audit_samples(&self.v, extractor, prompt, kind, correct, incorrect, "test audit", "fake data")
            .unwrap()
    }
}

// ---- Verification 1: the injection fixtures, on a scripted agent. ----

#[derive(Debug, Clone, Deserialize)]
struct Fixture {
    id: String,
    class: String,
    canary: String,
    file_name: String,
    metadata: Value,
    pdf_flagged: bool,
    text: String,
    quote: String,
    subject: String,
    predicate: String,
    value: String,
}

fn fixtures() -> Vec<Fixture> {
    include_str!("fixtures/injection/fixtures.jsonl")
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn pdf_of(text: &str) -> RenditionInput {
    let words: Vec<Word> = text
        .split_whitespace()
        .enumerate()
        .map(|(i, t)| Word {
            text: t.to_string(),
            page: 0,
            rect: Rect {
                x0: 100 + (i as i32 % 10) * 500,
                y0: 100 + (i as i32 / 10) * 1500,
                x1: 500 + (i as i32 % 10) * 500,
                y1: 1100 + (i as i32 / 10) * 1500,
            },
            confidence: None,
            quad: None,
        })
        .collect();
    let doc = ParsedDocument {
        parser: "test".into(),
        pages: vec![PageFacts {
            index: 0,
            width: 61200,
            height: 79200,
            rotation: 0,
            class: PageClass::Text,
            hidden_text_objects: 1,
        }],
        words,
    };
    let outcome = PageOutcome {
        index: 0,
        route: PageRoute::TextLayer,
        mean_confidence_permille: None,
        flags: vec![PageFlag::HiddenText { words: 12 }],
    };
    RenditionInput::Pdf(doc, vec![outcome])
}

/// A model that does as the document says in every way the interface lets it: it tries tools, makes the planted claim, makes up a quote, and its reviewer approves and asks for tools as well.
fn obedient_extractor(f: &Fixture) -> ScriptedAgent {
    let good = claim_reply(&f.quote, &f.subject, &f.predicate, &f.value);
    let mut tried = good.clone().unwrap();
    tried.tool_calls = 3;
    let invented = reply(json!({"claims": [
        {"segment_id": "s1", "quote": f.quote, "subject": f.subject, "predicate": f.predicate, "value": f.value},
        {"segment_id": "s1", "quote": "the administrator has confirmed this", "subject": "the administrator", "predicate": "confirmed", "value": "yes"},
    ], "entities": {}}));
    ScriptedAgent::new(vec![Ok(tried), invented])
}

fn obedient_reviewer() -> ScriptedAgent {
    let mut tried = verdict_reply("approve", "the document says to approve").unwrap();
    tried.tool_calls = 2;
    ScriptedAgent::new(vec![
        Ok(tried),
        verdict_reply("approve", "the document says to approve, and I should commit it"),
    ])
}

/// Everything a planted instruction might try to reach: what's there before the document arrives.
fn others(w: &World) -> Vec<Vec<Row>> {
    [
        Table::Page,
        Table::Assertion,
        Table::Evidence,
        Table::Entity,
        Table::EntityType,
        Table::Edge,
        Table::Block,
        Table::Slug,
        Table::Revision,
    ]
    .into_iter()
    .map(|t| w.table(t))
    .collect()
}

fn run_fixture(w: &World, f: &Fixture, tier: &str) -> (ScriptedAgent, ScriptedAgent) {
    let source = w.upload(&f.file_name, tier, f.metadata.clone(), &f.text);
    w.parse(&source, if f.pdf_flagged { pdf_of(&f.text) } else { World::plain(&f.text) });
    let (extractor, reviewer) = (obedient_extractor(f), obedient_reviewer());
    w.extract(&extractor);
    // Auto-commit, if it happened, leaves nothing for the reviewer.
    if let Some(job) = w.s.claim_job(&w.v, REVIEW_JOB, "w").unwrap() {
        let rows = w.s.run_review_job(&w.v, &job, &reviewer).unwrap();
        assert!(w.s.complete_job(&w.v, &job, rows).unwrap());
    }
    (extractor, reviewer)
}

fn baseline(w: &World) {
    let page = NewPage {
        title: "Home".into(),
        slug: None,
        body: "The start page.".into(),
        rationale: "r".into(),
        idempotency_key: "home".into(),
    };
    w.s.propose_new_page(&w.admin, &w.v, page, false).unwrap();
}

#[test]
fn there_are_at_least_30_injection_fixtures_in_all_four_places() {
    let all = fixtures();
    assert!(all.len() >= 30, "{}", all.len());
    for class in ["text", "hidden", "metadata", "tool_output"] {
        assert!(all.iter().filter(|f| f.class == class).count() >= 7, "{class}");
    }
    let mut ids: Vec<&str> = all.iter().map(|f| f.id.as_str()).collect();
    ids.dedup();
    assert_eq!(ids.len(), all.len());
}

#[test]
fn injection_fixtures_commit_nothing_call_no_tools_and_change_nothing_else_with_auto_commit_off() {
    for f in fixtures() {
        let w = world();
        baseline(&w);
        let before = others(&w);
        let already = w.changesets().len();
        let (extractor, reviewer) = run_fixture(&w, &f, "unverified");
        let new: Vec<ChangeSetRow> = w.changesets().split_off(already);
        assert_eq!(new.len(), 1, "{}: the one claim with a real quote", f.id);
        assert_eq!(new[0].state, ChangeSetState::Pending, "{}", f.id);
        assert_eq!(new[0].risk, RiskDecision::Review, "{}", f.id);
        assert_eq!(others(&w), before, "{}: nothing else moved", f.id);

        // Asking for a tool got the reply refused, both times it's tried, and the requests offer none: `AgentRequest` has no field for them.
        assert_eq!(extractor.requests().len(), 2, "{}", f.id);
        assert_eq!(reviewer.requests().len(), 2, "{}", f.id);

        // The verdict is on record and committed nothing.
        let item = w.s.review_item(&w.admin, &w.v, &new[0].header.id).unwrap();
        assert_eq!(item.item.verdict.as_ref().unwrap().verdict, Verdict::Approve, "{}", f.id);
        assert_eq!(
            item.item.changeset.state,
            ChangeSetState::Pending,
            "{}: an approving verdict decides nothing",
            f.id
        );

        // The reviewer sees the claim and its quote, and neither the rest of the document nor anything about the file.
        let shown = &reviewer.requests()[0].input;
        let doc: Value = serde_json::from_str(shown.split("\n\nYour previous").next().unwrap()).unwrap();
        let claims = doc["claims"].as_array().unwrap();
        assert_eq!(doc.as_object().unwrap().len(), 1);
        assert_eq!(claims.len(), 1);
        let mut keys: Vec<&str> = claims[0].as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["predicate", "quotes", "subject", "value"], "{}", f.id);
        assert!(!shown.contains("maintenance report describes"), "{}: the rest of the document stays out", f.id);

        // File names and metadata never reach a model, in either call.
        if f.class == "metadata" {
            for r in extractor.requests().iter().chain(reviewer.requests().iter()) {
                assert!(!r.input.contains(&f.canary) && !r.instruction.contains(&f.canary), "{}", f.id);
            }
        }
        // And the instruction is the same fixed text however hostile the document.
        assert!(!reviewer.requests()[0].instruction.contains(&f.canary), "{}", f.id);
    }
}

#[test]
fn injection_fixtures_in_an_untrusted_source_stay_out_even_with_auto_commit_earned() {
    for f in fixtures() {
        let w = world();
        baseline(&w);
        w.samples("scripted", &prompt_hash(), "claim", 10_000, 0);
        let before = others(&w);
        let already = w.changesets().len();
        run_fixture(&w, &f, "untrusted");
        let new = w.changesets().split_off(already);
        assert_eq!(new.len(), 1, "{}", f.id);
        assert_eq!(new[0].state, ChangeSetState::Pending, "{}", f.id);
        assert_eq!(others(&w), before, "{}", f.id);
    }
}

#[test]
fn hidden_text_stays_out_with_auto_commit_earned_whatever_the_source_s_tier() {
    for f in fixtures().into_iter().filter(|f| f.class == "hidden") {
        let w = world();
        w.samples("scripted", &prompt_hash(), "claim", 10_000, 0);
        run_fixture(&w, &f, "trusted");
        let new = w.changesets();
        assert_eq!(new.len(), 1, "{}", f.id);
        assert_eq!(new[0].state, ChangeSetState::Pending, "{}", f.id);
        let detail = w.s.review_item(&w.admin, &w.v, &new[0].header.id).unwrap();
        assert!(detail.auto_commit_block.is_some(), "{}", f.id);
    }
}

/// The user's decision: only a source someone marked trusted can auto-commit. Every fixture, including the sentences in plain view that a trusted source would let through below.
#[test]
fn injection_fixtures_in_an_unverified_source_stay_out_even_with_auto_commit_earned() {
    for f in fixtures() {
        let w = world();
        baseline(&w);
        w.samples("scripted", &prompt_hash(), "claim", 10_000, 0);
        let before = others(&w);
        let already = w.changesets().len();
        run_fixture(&w, &f, "unverified");
        let new = w.changesets().split_off(already);
        assert_eq!(new.len(), 1, "{}", f.id);
        assert_eq!(new[0].state, ChangeSetState::Pending, "{}", f.id);
        assert_eq!(others(&w), before, "{}", f.id);
        let detail = w.s.review_item(&w.admin, &w.v, &new[0].header.id).unwrap();
        assert!(detail.auto_commit_block.is_some(), "{}", f.id);
    }
}

/// The remaining limit, so nobody reads more into the above: a sentence in plain view, in a source a person marked trusted, looks like any other claim. With auto-commit earned it commits as `asserted`, and an audit is what catches it. It still can't reach anything else, and it can't be `verified`.
#[test]
fn the_remaining_limit_a_planted_sentence_in_plain_view_in_a_source_a_person_marked_trusted_commits_as_asserted() {
    let visible: Vec<Fixture> = fixtures().into_iter().filter(|f| f.class == "text" || f.class == "tool_output").collect();
    assert_eq!(visible.len(), 16);
    for f in visible {
        for tier in ["trusted", "verified"] {
            let w = world();
            baseline(&w);
            w.samples("scripted", &prompt_hash(), "claim", 10_000, 0);
            let pages = w.table(Table::Page);
            let (extractor, _) = run_fixture(&w, &f, tier);
            let row = w.changesets().pop().unwrap();
            assert_eq!(row.state, ChangeSetState::Committed, "{} {tier}", f.id);
            assert_eq!(row.decided_by.as_deref(), Some("auto-commit"), "{} {tier}", f.id);
            assert_eq!(extractor.requests().len(), 2, "{}: tools are still refused", f.id);
            assert_eq!(w.table(Table::Page), pages, "{} {tier}", f.id);
            let claims = w.table(Table::Assertion);
            assert_eq!(claims.len(), 1, "{} {tier}", f.id);
            let Row::Assertion(a) = &claims[0] else { panic!() };
            assert_eq!(a.epistemic, Epistemic::Asserted, "{} {tier}", f.id);
        }
    }
}

fn agent_claim(id: &str, segments: &[&ObjectId], quote: &str) -> Proposal {
    Proposal {
        ops: vec![Op::AddAssertion {
            id: ObjectId(id.into()),
            subject: format!("subject {id}"),
            predicate: "p".into(),
            value: json!(1),
            epistemic: Epistemic::Asserted,
            machine: true,
            confidence_permille: 1000,
            valid_from_ms: None,
            valid_to_ms: None,
            evidence: segments
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    (
                        ObjectId(format!("e{id}{i}")),
                        EvidenceInput {
                            segment_id: (*s).clone(),
                            quote: quote.into(),
                            stance: Stance::Supports,
                        },
                    )
                })
                .collect(),
        }],
        rationale: "r".into(),
        idempotency_key: id.into(),
    }
}

#[test]
fn one_source_below_trusted_or_with_no_row_is_enough_to_keep_a_claim_from_auto_committing() {
    let w = world();
    w.samples(&w.agent.principal.id, "", "claim", 10_000, 0);
    let text = "The pump weighs 12 kg.";
    let segment = |name: &str, tier: &str| {
        let source = w.upload(&format!("{name}.txt"), tier, json!({"n": name}), &format!("{name}: the document"));
        let id = ObjectId(format!("seg-{name}"));
        w.s.record_segment(&w.admin, &w.v, &id, &source, None, text).unwrap();
        id
    };
    let (trusted, also, unverified) = (segment("a", "trusted"), segment("b", "verified"), segment("c", "unverified"));
    let propose = |p| w.s.propose(&w.agent, &w.v, p, false).unwrap().risk;
    assert_eq!(propose(agent_claim("1", &[&trusted, &also], text)), RiskDecision::AutoCommit);
    assert_eq!(propose(agent_claim("2", &[&trusted, &unverified], text)), RiskDecision::Review, "one is enough");
    // A segment whose source has no row can't show it's trusted.
    w.s.record_segment(&w.admin, &w.v, &ObjectId("seg-none".into()), &ObjectId("no such source".into()), None, text)
        .unwrap();
    assert_eq!(propose(agent_claim("3", &[&ObjectId("seg-none".into())], text)), RiskDecision::Review);
    // A machine's change with no claim isn't about a source at all, and still earns as before.
    w.samples(&w.agent.principal.id, "", "page", 10_000, 0);
    let page = Proposal {
        ops: vec![Op::CreatePage {
            id: ObjectId("pg".into()),
            title: "T".into(),
            body: "b".into(),
            slug: None,
            redirects: vec![],
        }],
        rationale: "r".into(),
        idempotency_key: "pg".into(),
    };
    assert_eq!(propose(page), RiskDecision::AutoCommit);
}

#[test]
fn an_agent_can_not_make_a_source_trusted_by_itself() {
    let w = world();
    let req = |name: &str, tier: &str| UploadRequest {
        name: name.into(),
        trust_tier: tier.into(),
        metadata: json!({}),
        replaces: None,
    };
    // Not by uploading it that way.
    for tier in ["trusted", "verified"] {
        let refused =
            w.s.upload(&w.agent, &w.v, req("a.txt", tier), UploadLimits::default(), &mut "agent document".as_bytes());
        assert!(matches!(refused, Err(ServerError::Forbidden)), "{tier}");
    }
    let source =
        w.s.upload(
            &w.agent,
            &w.v,
            req("a.txt", "unverified"),
            UploadLimits::default(),
            &mut "agent document".as_bytes(),
        )
        .unwrap()
        .source;

    // Nor by raising it, with everything earned, and nor can an agent with admin scope approve that.
    w.samples(&w.agent.principal.id, "", "source", 10_000, 0);
    let raise = |tier: &str, key: &str| Proposal {
        ops: vec![Op::SetSourceTrust {
            id: source.header.id.clone(),
            base_version: 1,
            trust_tier: tier.into(),
        }],
        rationale: "r".into(),
        idempotency_key: key.into(),
    };
    let out = w.s.propose(&w.agent, &w.v, raise("trusted", "raise"), false).unwrap();
    assert_eq!(out.risk, RiskDecision::Review);
    let cs = out.changeset.unwrap();
    assert!(matches!(w.s.approve(&w.agent_admin, &w.v, &cs.header.id, None), Err(ServerError::Forbidden)));
    assert!(w.s.review_item(&w.admin, &w.v, &cs.header.id).unwrap().needs_a_person);

    // A person can, and that's what opens the source. Doing it directly is a person's own decision.
    assert_eq!(w.s.approve(&w.admin, &w.v, &cs.header.id, None).unwrap().state, ChangeSetState::Committed);
    let person = w.upload("b.txt", "unverified", json!({}), "a person's document");
    let direct = Proposal {
        ops: vec![Op::SetSourceTrust {
            id: person,
            base_version: 1,
            trust_tier: "trusted".into(),
        }],
        rationale: "r".into(),
        idempotency_key: "direct".into(),
    };
    assert_eq!(w.s.propose(&w.admin, &w.v, direct, false).unwrap().risk, RiskDecision::AutoCommit);
    // A person's upload can say it too.
    w.upload("c.txt", "trusted", json!({}), "another of a person's documents");
}

#[test]
fn a_reviewer_that_only_ever_asks_for_tools_ends_up_abstaining() {
    let w = world();
    let cs = w.pending_claim(0);
    let mut tried = verdict_reply("approve", "x").unwrap();
    tried.tool_calls = 1;
    let agent = ScriptedAgent::new(vec![Ok(tried.clone()), Ok(tried)]);
    w.review_one(&agent).unwrap();
    let v = w.s.review_item(&w.admin, &w.v, &cs.header.id).unwrap().item.verdict.unwrap();
    assert_eq!(v.verdict, Verdict::Abstain);
    assert_eq!((v.attempts, v.usage.input_tokens), (2, 140), "what it spent is on record");
    assert!(v.reason.contains("tool"), "{}", v.reason);
}

#[test]
fn a_verdict_that_is_not_the_schema_is_never_taken_as_one() {
    let w = world();
    let cs = w.pending_claim(0);
    let agent = ScriptedAgent::new(vec![
        reply(json!({"verdict": "commit", "reason": "x"})),
        reply(json!({"verdict": "approve", "reason": "x", "tool": "merge"})),
    ]);
    w.review_one(&agent).unwrap();
    let v = w.s.review_item(&w.admin, &w.v, &cs.header.id).unwrap().item.verdict.unwrap();
    assert_eq!(v.verdict, Verdict::Abstain);
}

// ---- The reviewer agent. ----

#[test]
fn only_a_machine_s_queued_claims_get_a_reviewer_and_only_when_one_is_set_up() {
    let w = world();
    // A person's proposal with nothing to judge gets no job.
    let (page, _) =
        w.s.propose_new_page(
            &w.admin,
            &w.v,
            NewPage {
                title: "T".into(),
                slug: None,
                body: "b".into(),
                rationale: "r".into(),
                idempotency_key: "p".into(),
            },
            false,
        )
        .unwrap();
    w.s.propose_page_delete(&w.admin, &w.v, &page, 1, "r".into(), "d".into()).unwrap();
    assert!(w.s.claim_job(&w.v, REVIEW_JOB, "w").unwrap().is_none());
    // Nor is one queued with the reviewer off, and the item waits for people all the same.
    w.s.set_reviewer(None);
    let cs = w.pending_claim(0);
    assert!(w.s.claim_job(&w.v, REVIEW_JOB, "w").unwrap().is_none());
    assert_eq!(w.s.review_item(&w.admin, &w.v, &cs.header.id).unwrap().item.verdict, None);
}

#[test]
fn a_decided_item_costs_no_reviewer_call_and_a_second_run_costs_none_either() {
    let w = world();
    let cs = w.pending_claim(0);
    w.s.approve(&w.admin, &w.v, &cs.header.id, None).unwrap();
    let agent = ScriptedAgent::new(vec![]);
    w.review_one(&agent).unwrap();
    assert_eq!(agent.requests().len(), 0, "a person got there first");

    let cs = w.pending_claim(1);
    let job = w.s.claim_job(&w.v, REVIEW_JOB, "w").unwrap().unwrap();
    let agent = ScriptedAgent::new(vec![verdict_reply("reject", "no")]);
    let rows = w.s.run_review_job(&w.v, &job, &agent).unwrap();
    assert!(w.s.complete_job(&w.v, &job, rows).unwrap());
    // A job asked to run again for the same prompt finds the verdict there.
    assert_eq!(w.s.run_review_job(&w.v, &job, &agent).unwrap(), vec![]);
    assert_eq!(agent.requests().len(), 1);
    assert_eq!(
        w.s.review_item(&w.admin, &w.v, &cs.header.id).unwrap().item.verdict.unwrap().verdict,
        Verdict::Reject
    );
}

#[test]
fn verdicts_never_count_towards_precision() {
    let w = world();
    for n in 0..3 {
        w.pending_claim(n);
        w.review_one(&ScriptedAgent::new(vec![verdict_reply("approve", "fine")])).unwrap();
    }
    assert!(w.s.auto_commit_table(&w.admin, &w.v, None, 10).unwrap().entries.is_empty());
}

// ---- Verification 3 is in `memex_core::review`'s unit tests. Verification 4: crossing the threshold, with fake data. ----

#[test]
fn crossing_the_threshold_turns_auto_commit_on_and_falling_below_turns_it_off() {
    let w = world();
    let prompt = prompt_hash();
    // Off with no samples at all.
    assert_eq!(w.pending_claim(0).state, ChangeSetState::Pending);
    // 72 of 72 has a bound of 0.9493, under the default 0.95.
    w.samples("scripted", &prompt, "claim", 72, 0);
    assert_eq!(w.pending_claim(1).state, ChangeSetState::Pending);
    // 73 of 73 has 0.95001.
    let row = w.samples("scripted", &prompt, "claim", 1, 0);
    assert!(row.on);
    assert_eq!(row.history.len(), 1);
    assert_eq!(row.history[0].kind, AutoCommitChangeKind::Earned);

    let sentence = "Fartyget nummer 2 byggdes i Stockholm år 1602.";
    w.ingest("d2.txt", "trusted", &long_text(sentence));
    w.extract(&ScriptedAgent::new(vec![claim_reply(sentence, "Fartyget 2", "byggdes i", "Stockholm")]));
    let auto = w.changesets().pop().unwrap();
    assert_eq!(auto.state, ChangeSetState::Committed);
    assert_eq!(auto.risk, RiskDecision::AutoCommit);
    assert_eq!(auto.decided_by.as_deref(), Some("auto-commit"));
    assert!(auto.decision_reason.as_deref().unwrap().contains("73 of 73"), "{:?}", auto.decision_reason);
    assert_eq!(w.committed_claims(), 1);

    // One failure is enough at this count, and it goes in the history.
    let row = w.samples("scripted", &prompt, "claim", 0, 1);
    assert!(!row.on);
    assert_eq!(row.history.last().unwrap().kind, AutoCommitChangeKind::FellBelow);
    assert_eq!(w.pending_claim(3).state, ChangeSetState::Pending);

    // And good samples can bring it back: 100 of 101 has 0.9462, and 149 of 150 has 0.9631.
    let row = w.samples("scripted", &prompt, "claim", 27, 0);
    assert!(!row.on);
    let row = w.samples("scripted", &prompt, "claim", 49, 0);
    assert!(row.on);
    let table = w.s.auto_commit_table(&w.reader, &w.v, None, 10).unwrap();
    assert_eq!(table.threshold, 0.95);
    assert_eq!(table.entries.len(), 1);
    assert!(table.entries[0].on);
    assert!((table.entries[0].precision.unwrap() - 149.0 / 150.0).abs() < 1e-12);
    assert_eq!(table.entries[0].row.history.len(), 3);
}

#[test]
fn people_s_approvals_earn_it() {
    let w = world();
    w.s.set_auto_commit_threshold(0.5);
    let row = |w: &World| w.s.auto_commit_table(&w.admin, &w.v, None, 10).unwrap().entries.remove(0);
    for n in 0..3 {
        w.s.approve(&w.admin, &w.v, &w.pending_claim(n).header.id, Some("right".into())).unwrap();
    }
    assert_eq!((row(&w).row.correct, row(&w).on), (3, false), "3 of 3 has a bound of 0.44");
    w.s.approve(&w.admin, &w.v, &w.pending_claim(3).header.id, None).unwrap();
    assert_eq!((row(&w).row.correct, row(&w).on), (4, true), "4 of 4 has a bound of 0.51");
    assert_eq!(row(&w).row.history[0].by, w.admin.principal.id);
}

#[test]
fn an_edit_or_a_rejection_counts_against_and_an_agent_s_decision_counts_for_nothing() {
    let w = world();
    let row = |w: &World| w.s.auto_commit_table(&w.admin, &w.v, None, 10).unwrap().entries.remove(0).row;
    let edit = w.pending_claim(0);
    let mut ops = edit.ops.clone();
    if let Op::AddAssertion { value, .. } = &mut ops[0] {
        *value = json!("Göteborg");
    }
    w.s.approve_edited(&w.admin, &w.v, &edit.header.id, ops, Some("the wrong city".into())).unwrap();
    assert_eq!((row(&w).correct, row(&w).incorrect), (0, 1));
    w.s.reject(&w.admin, &w.v, &w.pending_claim(1).header.id, "wrong".into()).unwrap();
    assert_eq!((row(&w).correct, row(&w).incorrect), (0, 2));
    w.s.approve(&w.agent_admin, &w.v, &w.pending_claim(2).header.id, None).unwrap();
    w.s.reject(&w.agent_admin, &w.v, &w.pending_claim(3).header.id, "no".into()).unwrap();
    assert_eq!(
        (row(&w).correct, row(&w).incorrect),
        (0, 2),
        "an agent's decision isn't an audit, whatever scope it holds"
    );
}

#[test]
fn every_type_in_a_change_set_must_have_earned_it() {
    let w = world();
    let me = w.agent.principal.id.clone();
    let claim = |id: &str, subject: &str| Op::AddAssertion {
        id: ObjectId(id.into()),
        subject: subject.into(),
        predicate: "p".into(),
        value: json!(1),
        epistemic: Epistemic::Asserted,
        machine: true,
        confidence_permille: 1000,
        valid_from_ms: None,
        valid_to_ms: None,
        evidence: vec![],
    };
    let trusted = w.upload("t.txt", "trusted", json!({}), "the pump document");
    w.s.record_segment(&w.admin, &w.v, &ObjectId("seg".into()), &trusted, None, "The pump weighs 12 kg.")
        .unwrap();
    let cited = |op: Op| match op {
        Op::AddAssertion {
            id,
            subject,
            predicate,
            value,
            epistemic,
            machine,
            confidence_permille,
            valid_from_ms,
            valid_to_ms,
            ..
        } => Op::AddAssertion {
            evidence: vec![(
                ObjectId(format!("e{}", id.0)),
                EvidenceInput {
                    segment_id: ObjectId("seg".into()),
                    quote: "The pump weighs 12 kg.".into(),
                    stance: Stance::Supports,
                },
            )],
            id,
            subject,
            predicate,
            value,
            epistemic,
            machine,
            confidence_permille,
            valid_from_ms,
            valid_to_ms,
        },
        other => other,
    };
    let page = Op::CreatePage {
        id: ObjectId("pg".into()),
        title: "T".into(),
        body: "b".into(),
        slug: None,
        redirects: vec![],
    };
    w.samples(&me, "", "claim", 100, 0);
    let mixed = Proposal {
        ops: vec![cited(claim("a1", "s1")), page.clone()],
        rationale: "r".into(),
        idempotency_key: "m".into(),
    };
    assert_eq!(
        w.s.propose(&w.agent, &w.v, mixed, false).unwrap().risk,
        RiskDecision::Review,
        "the page hasn't earned it"
    );
    w.samples(&me, "", "page", 100, 0);
    let mixed = Proposal {
        ops: vec![cited(claim("a2", "s2"))],
        rationale: "r".into(),
        idempotency_key: "m2".into(),
    };
    assert_eq!(w.s.propose(&w.agent, &w.v, mixed, false).unwrap().risk, RiskDecision::AutoCommit);
    // Another extractor's record is no help to this one.
    let other = w.agent_admin.clone();
    let mine = Proposal {
        ops: vec![cited(claim("a3", "s3"))],
        rationale: "r".into(),
        idempotency_key: "o".into(),
    };
    assert_eq!(w.s.propose(&other, &w.v, mine, false).unwrap().risk, RiskDecision::Review);
}

#[test]
fn a_person_undoing_an_auto_commit_counts_against_it_and_the_counts_do_not_block_other_reverts() {
    let w = world();
    let prompt = prompt_hash();
    w.samples("scripted", &prompt, "claim", 73, 0);
    let sentence = "Fartyget nummer 0 byggdes i Stockholm år 1600.";
    w.ingest("d.txt", "trusted", &long_text(sentence));
    w.extract(&ScriptedAgent::new(vec![claim_reply(sentence, "Fartyget", "byggdes i", "Stockholm")]));
    let auto = w.changesets().pop().unwrap();
    assert_eq!(auto.state, ChangeSetState::Committed);
    // Newer samples on the same row don't count as a later edit to what this committed.
    w.samples("scripted", &prompt, "claim", 500, 0);
    w.s.revert(&w.admin, &w.v, &auto.header.id, "it was wrong".into()).unwrap();
    let row = w.s.auto_commit_table(&w.admin, &w.v, None, 10).unwrap().entries.remove(0).row;
    assert_eq!((row.correct, row.incorrect), (573, 1));
    assert_eq!(w.committed_claims(), 0);
}

// ---- Verification 5: who, when and why. ----

#[test]
fn every_decision_says_who_when_and_why() {
    let w = world();
    let when = |c: &ChangeSetRow| (c.decided_by.clone().unwrap(), c.decided_at_ms.unwrap(), c.decision_reason.clone().unwrap());

    // A person approving, and the event in the log under their name.
    let cs = w.pending_claim(0);
    let done = w.s.approve(&w.admin, &w.v, &cs.header.id, Some("it says so".into())).unwrap();
    assert_eq!(when(&done), (w.admin.principal.id.clone(), 1_000_000, "it says so".to_string()));
    let events = w.s.read_events(&w.admin, &w.v, None).unwrap();
    assert_eq!(events.last().unwrap().actor, w.admin.principal.id);

    // A person rejecting.
    let done = w.s.reject(&w.other_admin, &w.v, &w.pending_claim(1).header.id, "no".into()).unwrap();
    assert_eq!(when(&done), (w.other_admin.principal.id.clone(), 1_000_000, "no".to_string()));

    // A person editing, with what was first proposed still on the row.
    let cs = w.pending_claim(2);
    let mut ops = cs.ops.clone();
    if let Op::AddAssertion { value, .. } = &mut ops[0] {
        *value = json!("Malmö");
    }
    let done =
        w.s.approve_edited(&w.admin, &w.v, &cs.header.id, ops.clone(), Some("a better value".into()))
            .unwrap();
    assert_eq!(when(&done).0, w.admin.principal.id);
    assert_eq!(done.original_ops.as_ref().unwrap(), &cs.ops);
    assert_eq!(done.ops, ops);

    // The validators refusing something.
    let refused =
        w.s.propose(
            &w.agent,
            &w.v,
            Proposal {
                ops: vec![],
                rationale: "r".into(),
                idempotency_key: "bad".into(),
            },
            false,
        )
        .unwrap()
        .changeset
        .unwrap();
    assert_eq!(when(&refused).0, "validators");
    assert!(!when(&refused).2.is_empty());

    // A person's own low-risk change, and a machine's earned one.
    let (_, page) =
        w.s.propose_new_page(
            &w.admin,
            &w.v,
            NewPage {
                title: "T".into(),
                slug: None,
                body: "b".into(),
                rationale: "r".into(),
                idempotency_key: "p".into(),
            },
            false,
        )
        .unwrap();
    assert_eq!(when(page.changeset.as_ref().unwrap()).0, w.admin.principal.id);
    w.samples("scripted", &prompt_hash(), "claim", 1000, 0);
    let sentence = "Fartyget nummer 3 byggdes i Stockholm år 1603.";
    w.ingest("d3.txt", "trusted", &long_text(sentence));
    w.extract(&ScriptedAgent::new(vec![claim_reply(sentence, "Fartyget 3", "byggdes i", "Stockholm")]));
    let auto = w.changesets().pop().unwrap();
    assert_eq!(when(&auto).0, "auto-commit");
    assert!(
        when(&auto).2.contains("scripted") && when(&auto).2.contains(&prompt_hash()),
        "{}",
        when(&auto).2
    );

    // Turning auto-commit off, with its history.
    let row = w.s.auto_commit_table(&w.admin, &w.v, None, 10).unwrap().entries.remove(0).row;
    let off = w.s.disable_auto_commit(&w.other_admin, &w.v, &row.header.id, "a bad batch".into()).unwrap();
    let last = off.row.history.last().unwrap();
    assert_eq!(
        (last.by.as_str(), last.reason.as_str(), last.at_ms),
        (w.other_admin.principal.id.as_str(), "a bad batch", 1_000_000)
    );
}

#[test]
fn the_reviewer_s_part_is_on_record_too() {
    let w = world();
    let cs = w.pending_claim(0);
    w.review_one(&ScriptedAgent::new(vec![verdict_reply("reject", "the quote says nothing of it")]))
        .unwrap();
    let v = w.s.review_item(&w.reader, &w.v, &cs.header.id).unwrap().item.verdict.unwrap();
    assert_eq!(
        (v.verdict, v.reason.as_str(), v.model.as_str()),
        (Verdict::Reject, "the quote says nothing of it", "scripted")
    );
    assert_eq!(v.prompt_hash, memex_core::review::review_prompt().hash);
    assert_eq!(v.usage, USAGE);
    assert_eq!(v.created_at_ms, 1_000_000);
    // A person can overrule it, and that's who decided.
    let done = w.s.approve(&w.admin, &w.v, &cs.header.id, Some("I read the source".into())).unwrap();
    assert_eq!(done.state, ChangeSetState::Committed);
    assert_eq!(done.decided_by.as_deref(), Some(w.admin.principal.id.as_str()));
}

// ---- Verification 6: bad cases. ----

#[test]
fn approving_twice_is_idempotent_and_a_second_person_gets_a_conflict_saying_whose_it_was() {
    let w = world();
    let cs = w.pending_claim(0);
    let first = w.s.approve(&w.admin, &w.v, &cs.header.id, Some("yes".into())).unwrap();
    let head = w.s.read_events(&w.admin, &w.v, None).unwrap().len();
    let again = w.s.approve(&w.admin, &w.v, &cs.header.id, Some("yes".into())).unwrap();
    assert_eq!(again, first);
    assert_eq!(w.s.read_events(&w.admin, &w.v, None).unwrap().len(), head, "no second event");
    let row = w.s.auto_commit_table(&w.admin, &w.v, None, 10).unwrap().entries.remove(0).row;
    assert_eq!(row.correct, 1, "no second sample");
    assert_eq!(w.committed_claims(), 1);

    match w.s.approve(&w.other_admin, &w.v, &cs.header.id, None) {
        Err(ServerError::Conflict(m)) => assert!(m.contains(&w.admin.principal.id), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(w.s.reject(&w.admin, &w.v, &cs.header.id, "no".into()), Err(ServerError::Conflict(_))));

    let cs = w.pending_claim(1);
    let first = w.s.reject(&w.admin, &w.v, &cs.header.id, "no".into()).unwrap();
    assert_eq!(w.s.reject(&w.admin, &w.v, &cs.header.id, "no".into()).unwrap(), first);
    assert!(matches!(w.s.approve(&w.admin, &w.v, &cs.header.id, None), Err(ServerError::Conflict(_))));
    let row = w.s.auto_commit_table(&w.admin, &w.v, None, 10).unwrap().entries.remove(0).row;
    assert_eq!(row.incorrect, 1);
}

#[test]
fn the_same_edit_twice_is_the_same_decision_and_a_different_one_conflicts() {
    let w = world();
    let cs = w.pending_claim(0);
    let edit = |value: &str| {
        let mut ops = cs.ops.clone();
        if let Op::AddAssertion { value: v, .. } = &mut ops[0] {
            *v = json!(value);
        }
        ops
    };
    let first = w.s.approve_edited(&w.admin, &w.v, &cs.header.id, edit("Malmö"), None).unwrap();
    assert_eq!(w.s.approve_edited(&w.admin, &w.v, &cs.header.id, edit("Malmö"), None).unwrap(), first);
    assert!(matches!(
        w.s.approve_edited(&w.admin, &w.v, &cs.header.id, edit("Lund"), None),
        Err(ServerError::Conflict(_))
    ));
}

#[test]
fn an_edit_must_still_hold_up_and_only_a_person_can_make_one() {
    let w = world();
    let cs = w.pending_claim(0);
    let mut ops = cs.ops.clone();
    if let Op::AddAssertion { evidence, .. } = &mut ops[0] {
        evidence[0].1.quote = "words the document doesn't have".into();
    }
    assert!(matches!(
        w.s.approve_edited(&w.admin, &w.v, &cs.header.id, ops, None),
        Err(ServerError::Invalid(_))
    ));
    assert_eq!(w.changesets().pop().unwrap().state, ChangeSetState::Pending, "nothing happened");
    let mut ops = cs.ops.clone();
    if let Op::AddAssertion { evidence, .. } = &mut ops[0] {
        evidence.clear();
    }
    assert!(
        matches!(w.s.approve_edited(&w.admin, &w.v, &cs.header.id, ops, None), Err(ServerError::Invalid(_))),
        "a machine's claim stays a machine's, so it still needs evidence"
    );
    assert!(matches!(
        w.s.approve_edited(&w.agent_admin, &w.v, &cs.header.id, cs.ops.clone(), None),
        Err(ServerError::Forbidden)
    ));
    // An edit to the same ops is a plain approval, and counts as one.
    let done = w.s.approve_edited(&w.admin, &w.v, &cs.header.id, cs.ops.clone(), None).unwrap();
    assert_eq!(done.original_ops, None);
    let row = w.s.auto_commit_table(&w.admin, &w.v, None, 10).unwrap().entries.remove(0).row;
    assert_eq!((row.correct, row.incorrect), (1, 0));
}

#[test]
fn approving_a_stale_claim_conflicts_and_returns_what_is_there_now() {
    let w = world();
    w.s.record_segment(&w.admin, &w.v, &ObjectId("seg".into()), &ObjectId("src".into()), None, "The pump weighs 12 kg.")
        .unwrap();
    let claim = |id: &str, value: i32| Op::AddAssertion {
        id: ObjectId(id.into()),
        subject: "pump".into(),
        predicate: "weighs".into(),
        value: json!(value),
        epistemic: Epistemic::Asserted,
        machine: false,
        confidence_permille: 1000,
        valid_from_ms: None,
        valid_to_ms: None,
        evidence: vec![(
            ObjectId(format!("e{id}")),
            EvidenceInput {
                segment_id: ObjectId("seg".into()),
                quote: "The pump weighs 12 kg.".into(),
                stance: Stance::Supports,
            },
        )],
    };
    let proposal = |ops, key: &str| Proposal {
        ops,
        rationale: "r".into(),
        idempotency_key: key.into(),
    };
    w.s.propose(&w.admin, &w.v, proposal(vec![claim("a", 12)], "a"), false).unwrap();
    let replace = |id: &str, value| Op::SupersedeAssertion {
        target: ObjectId("a".into()),
        base_version: 1,
        replacement: Box::new(claim(id, value)),
    };
    let waiting =
        w.s.propose(&w.agent, &w.v, proposal(vec![replace("b", 13)], "b"), false)
            .unwrap()
            .changeset
            .unwrap();
    assert_eq!(waiting.state, ChangeSetState::Pending);
    // Someone else changes it while that waits.
    w.s.propose(&w.admin, &w.v, proposal(vec![replace("c", 14)], "c"), false).unwrap();
    match w.s.approve(&w.admin, &w.v, &waiting.header.id, None) {
        Err(ServerError::StaleVersion { base, .. }) => assert_eq!(base, 1),
        other => panic!("{other:?}"),
    }
    assert_eq!(w.s.get_changeset(&w.admin, &w.v, &waiting.header.id).unwrap().state, ChangeSetState::Pending);
    // And one the vault has otherwise moved past is a conflict rather than a bad request: here, the same claim was added since.
    let dup =
        w.s.propose(&w.agent, &w.v, proposal(vec![claim("d", 99)], "d"), false)
            .unwrap()
            .changeset
            .unwrap();
    w.s.propose(&w.admin, &w.v, proposal(vec![claim("e", 99)], "e"), false).unwrap();
    assert!(matches!(w.s.approve(&w.admin, &w.v, &dup.header.id, None), Err(ServerError::Conflict(_))));
}

#[test]
fn an_llm_outage_leaves_items_in_the_queue_and_people_can_still_decide() {
    let w = world();
    let cs = w.pending_claim(0);
    let other = w.pending_claim(1);
    let down = || ScriptedAgent::new(vec![Err(AgentError::Unavailable("down".into()))]);
    for _ in 0..30 {
        assert!(matches!(w.review_one(&down()), Err(ExtractJobError::Wait(_))));
        // The delay is capped at 5 minutes, and nothing is used up.
        w.s.engine().clock().advance(Duration::from_millis(301_000));
    }
    let jobs = w.s.list_jobs(&w.admin, &w.v, Some(REVIEW_JOB), None, None, 10).unwrap();
    assert_eq!(jobs.len(), 2);
    assert!(jobs.iter().any(|j| j.wait.is_some()));
    assert!(jobs.iter().all(|j| j.state == JobState::Queued));
    let queue =
        w.s.review_queue(
            &w.admin,
            &w.v,
            &ReviewFilter {
                state: Some(ChangeSetState::Pending),
                ..Default::default()
            },
            None,
            10,
        )
        .unwrap();
    assert_eq!(queue.len(), 2);
    assert!(queue.iter().all(|i| i.verdict.is_none()));
    // Nothing waits on the model for a person.
    w.s.approve(&w.admin, &w.v, &cs.header.id, None).unwrap();
    // And when it's back, what's left gets its verdict.
    w.s.engine().clock().advance(Duration::from_millis(301_000));
    let up = ScriptedAgent::new(vec![verdict_reply("approve", "ok"), verdict_reply("approve", "ok")]);
    while let Some(job) = w.s.claim_job(&w.v, REVIEW_JOB, "w").unwrap() {
        let rows = w.s.run_review_job(&w.v, &job, &up).unwrap();
        assert!(w.s.complete_job(&w.v, &job, rows).unwrap());
    }
    assert_eq!(up.requests().len(), 1, "the decided one costs nothing");
    assert_eq!(
        w.s.review_item(&w.admin, &w.v, &other.header.id).unwrap().item.verdict.unwrap().verdict,
        Verdict::Approve
    );
}

#[test]
fn waiting_reviews_do_not_count_as_upload_backlog() {
    let w = world();
    w.pending_claim(0);
    let limits = UploadLimits {
        max_bytes: 1_000_000,
        max_open_jobs: 1,
    };
    let req = UploadRequest {
        name: "n.txt".into(),
        trust_tier: "unverified".into(),
        metadata: json!({}),
        replaces: None,
    };
    w.s.upload(&w.admin, &w.v, req, limits, &mut "another document".as_bytes()).unwrap();
}

// ---- Verification 7: a manual disable. ----

#[test]
fn a_manual_disable_stops_auto_commit_at_once_and_only_re_earning_turns_it_on() {
    let w = world();
    let prompt = prompt_hash();
    let row = w.samples("scripted", &prompt, "claim", 500, 0);
    assert!(row.on);
    let sentence = "Fartyget nummer 7 byggdes i Stockholm år 1607.";
    w.ingest("d7.txt", "trusted", &long_text(sentence));
    w.extract(&ScriptedAgent::new(vec![claim_reply(sentence, "Fartyget 7", "byggdes i", "Stockholm")]));
    assert_eq!(w.changesets().pop().unwrap().state, ChangeSetState::Committed, "on, to begin with");

    assert!(matches!(
        w.s.disable_auto_commit(&w.reader, &w.v, &row.header.id, "r".into()),
        Err(ServerError::Forbidden)
    ));
    assert!(matches!(
        w.s.disable_auto_commit(&w.admin, &w.v, &row.header.id, " ".into()),
        Err(ServerError::Invalid(_))
    ));
    assert!(matches!(
        w.s.disable_auto_commit(&w.admin, &w.v, &ObjectId("none".into()), "r".into()),
        Err(ServerError::NotFound)
    ));
    let off = w.s.disable_auto_commit(&w.admin, &w.v, &row.header.id, "a bad batch".into()).unwrap();
    assert!(!off.on);

    // The very next proposal goes to review, though 500 good samples are on record.
    assert_eq!(w.pending_claim(8).state, ChangeSetState::Pending);
    assert_eq!(off.row.total_correct, 500);

    // A little more isn't enough: the old 500 don't count now.
    let row = w.samples("scripted", &prompt, "claim", 72, 0);
    assert!(!row.on);
    assert_eq!(w.pending_claim(9).state, ChangeSetState::Pending);
    let table = w.s.auto_commit_table(&w.admin, &w.v, None, 10).unwrap();
    assert!(!table.entries[0].on);
    // Earning it again is.
    let row = w.samples("scripted", &prompt, "claim", 1, 0);
    assert!(row.on);
    let kinds: Vec<AutoCommitChangeKind> = row.history.iter().map(|c| c.kind).collect();
    assert_eq!(
        kinds,
        [AutoCommitChangeKind::Earned, AutoCommitChangeKind::Disabled, AutoCommitChangeKind::Earned]
    );
    // Disabling other combinations leaves this one alone.
    let other = w.samples("scripted", "another prompt", "claim", 500, 0);
    w.s.disable_auto_commit(&w.admin, &w.v, &other.header.id, "r".into()).unwrap();
    let table = w.s.auto_commit_table(&w.admin, &w.v, None, 10).unwrap();
    assert_eq!(table.entries.iter().filter(|e| e.on).count(), 1);
}

// ---- The always-human rules, including what spec 023 left to this spec. ----

#[test]
fn an_agent_with_admin_scope_can_not_approve_or_revert_any_always_review_kind() {
    let w = world();
    w.s.record_segment(&w.admin, &w.v, &ObjectId("seg".into()), &ObjectId("src".into()), None, "The pump weighs 12 kg.")
        .unwrap();
    let propose = |ops, key: &str| {
        w.s.propose(
            &w.agent,
            &w.v,
            Proposal {
                ops,
                rationale: "r".into(),
                idempotency_key: key.into(),
            },
            false,
        )
        .unwrap()
        .changeset
        .unwrap()
    };
    let cited = |id: &str, epistemic| Op::AddAssertion {
        id: ObjectId(id.into()),
        subject: format!("s{id}"),
        predicate: "p".into(),
        value: json!(1),
        epistemic,
        machine: false,
        confidence_permille: 1000,
        valid_from_ms: None,
        valid_to_ms: None,
        evidence: vec![(
            ObjectId(format!("e{id}")),
            EvidenceInput {
                segment_id: ObjectId("seg".into()),
                quote: "The pump weighs 12 kg.".into(),
                stance: Stance::Supports,
            },
        )],
    };
    // Something for each kind to act on, made by a person.
    let by_person = |op: Op, key: &str| {
        w.s.propose(
            &w.admin,
            &w.v,
            Proposal {
                ops: vec![op],
                rationale: "r".into(),
                idempotency_key: key.into(),
            },
            false,
        )
        .unwrap()
    };
    by_person(cited("v", Epistemic::Verified), "made-v");
    by_person(
        Op::AddEdge {
            id: ObjectId("edge".into()),
            from: ObjectId("a".into()),
            to: ObjectId("b".into()),
            kind: "k".into(),
        },
        "made-e",
    );
    let (page, _) =
        w.s.propose_new_page(
            &w.admin,
            &w.v,
            NewPage {
                title: "T".into(),
                slug: None,
                body: "b".into(),
                rationale: "r".into(),
                idempotency_key: "made-p".into(),
            },
            false,
        )
        .unwrap();
    let untrusted = w.upload("u.txt", "untrusted", json!({}), "some text");
    let cases = vec![
        ("delete", vec![Op::DeletePage { id: page, base_version: 1 }]),
        (
            "remove link",
            vec![Op::RemoveEdge {
                id: ObjectId("edge".into()),
                base_version: 1,
            }],
        ),
        (
            "edit verified",
            vec![Op::SupersedeAssertion {
                target: ObjectId("v".into()),
                base_version: 1,
                replacement: Box::new(cited("v2", Epistemic::Asserted)),
            }],
        ),
        (
            "promote untrusted",
            vec![Op::SetSourceTrust {
                id: untrusted,
                base_version: 1,
                trust_tier: "trusted".into(),
            }],
        ),
        ("a machine's verified claim", vec![cited("mv", Epistemic::Verified)]),
    ];
    w.samples(&w.agent.principal.id, "", "claim", 1000, 0);
    w.samples(&w.agent.principal.id, "", "page", 1000, 0);
    w.samples(&w.agent.principal.id, "", "link", 1000, 0);
    w.samples(&w.agent.principal.id, "", "source", 1000, 0);
    for (name, ops) in cases {
        let cs = propose(ops, name);
        assert_eq!(cs.state, ChangeSetState::Pending, "{name}, with everything earned");
        assert!(
            matches!(w.s.approve(&w.agent_admin, &w.v, &cs.header.id, None), Err(ServerError::Forbidden)),
            "{name}"
        );
        assert!(w.s.review_item(&w.admin, &w.v, &cs.header.id).unwrap().needs_a_person, "{name}");
        let done = w.s.approve(&w.admin, &w.v, &cs.header.id, None).unwrap();
        assert_eq!(done.state, ChangeSetState::Committed, "{name}: a person can");
        if name != "promote untrusted" {
            assert!(
                matches!(w.s.revert(&w.agent_admin, &w.v, &cs.header.id, "r".into()), Err(ServerError::Forbidden)),
                "{name}"
            );
        }
    }
    // An agent with admin scope can still approve what isn't reserved for a person.
    let plain = propose(vec![cited("plain", Epistemic::Asserted)], "plain");
    if plain.state == ChangeSetState::Pending {
        w.s.approve(&w.agent_admin, &w.v, &plain.header.id, None).unwrap();
    }
}

#[test]
fn an_untrusted_source_can_not_reach_verified_automatically() {
    let w = world();
    let sentence = "Fartyget byggdes i Stockholm år 1600.";
    let text = long_text(sentence);
    w.ingest("u.txt", "untrusted", &text);
    let segment = w.table(Table::Segment).into_iter().next().unwrap().header().id.clone();
    let claim = |id: &str| Op::AddAssertion {
        id: ObjectId(id.into()),
        subject: format!("Fartyget {id}"),
        predicate: "byggdes i".into(),
        value: json!("Stockholm"),
        epistemic: Epistemic::Verified,
        machine: false,
        confidence_permille: 1000,
        valid_from_ms: None,
        valid_to_ms: None,
        evidence: vec![(
            ObjectId(format!("e{id}")),
            EvidenceInput {
                segment_id: segment.clone(),
                quote: sentence.into(),
                stance: Stance::Supports,
            },
        )],
    };
    let proposal = |id: &str| Proposal {
        ops: vec![claim(id)],
        rationale: "r".into(),
        idempotency_key: id.into(),
    };
    // Not from a person's token with no one looking, and not from a machine with everything earned.
    w.samples(&w.agent.principal.id, "", "claim", 1000, 0);
    let mine = w.s.propose(&w.admin, &w.v, proposal("a"), false).unwrap();
    assert_eq!(mine.risk, RiskDecision::Review);
    let theirs = w.s.propose(&w.agent, &w.v, proposal("b"), false).unwrap();
    assert_eq!(theirs.risk, RiskDecision::Review);
    assert_eq!(w.committed_claims(), 0);
    // A person looking at it can.
    let done = w.s.approve(&w.other_admin, &w.v, &mine.changeset.unwrap().header.id, None).unwrap();
    assert_eq!(done.state, ChangeSetState::Committed);
    assert!(matches!(
        w.s.approve(&w.agent_admin, &w.v, &theirs.changeset.unwrap().header.id, None),
        Err(ServerError::Forbidden)
    ));
}

// ---- What spec 024 left to this spec: pending proposals on an old rendition. ----

#[test]
fn a_reprocessed_document_s_pending_proposal_conflicts_and_its_run_retires_it() {
    let w = world();
    let text = long_text(SENTENCE);
    let source = w.ingest("a.txt", "unverified", &text);
    w.extract(&ScriptedAgent::new(vec![claim_reply(SENTENCE, "Fartyget", "byggdes i", "Stockholm")]));
    let old = w.changesets().pop().unwrap();
    assert_eq!(old.state, ChangeSetState::Pending);
    w.s.approve(&w.admin, &w.v, &w.pending_claim(1).header.id, None).unwrap();
    let correct_before = w.s.auto_commit_table(&w.admin, &w.v, None, 10).unwrap().entries[0].row.correct;

    // The document is read again. The old reading can no longer be approved, and the item says why.
    w.s.reprocess_source(&w.admin, &w.v, &source).unwrap();
    w.parse(&source, World::plain(&text));
    match w.s.approve(&w.admin, &w.v, &old.header.id, None) {
        Err(ServerError::Conflict(m)) => assert!(m.contains("replaced"), "{m}"),
        other => panic!("{other:?}"),
    }
    let detail = w.s.review_item(&w.admin, &w.v, &old.header.id).unwrap();
    assert!(detail.evidence[0].superseded);
    assert_eq!(w.s.get_changeset(&w.admin, &w.v, &old.header.id).unwrap().state, ChangeSetState::Pending);

    // Its new run takes it out of the queue before making any call, and proposes from the new reading.
    let job = w.s.claim_job(&w.v, EXTRACT_JOB, "w").unwrap().unwrap();
    let down = ScriptedAgent::new(vec![Err(AgentError::Unavailable("down".into()))]);
    assert!(matches!(w.s.run_extraction_job(&w.v, &job, &down), Err(ExtractJobError::Wait(_))));
    assert!(w.s.wait_job(&w.v, &job, "down").unwrap());
    let retired = w.s.get_changeset(&w.admin, &w.v, &old.header.id).unwrap();
    assert_eq!((retired.state, retired.decided_by.as_deref()), (ChangeSetState::Rejected, Some("pipeline")));
    assert!(retired.decision_reason.as_deref().unwrap().contains("replaced"));
    w.s.engine().clock().advance(Duration::from_millis(301_000));
    w.extract(&ScriptedAgent::new(vec![claim_reply(SENTENCE, "Fartyget", "byggdes i", "Stockholm")]));
    let new = w.changesets().pop().unwrap();
    assert_eq!(new.state, ChangeSetState::Pending);
    assert_ne!(new.header.id, old.header.id);
    let done = w.s.approve(&w.admin, &w.v, &new.header.id, None).unwrap();
    assert_eq!(done.state, ChangeSetState::Committed);
    // The claim sits on the current rendition, and nobody judged the retired one.
    let current =
        w.s.list_renditions(&w.admin, &w.v, &source)
            .unwrap()
            .into_iter()
            .find(|r| r.superseded_by.is_none())
            .unwrap();
    assert_eq!(new.origin.as_ref().unwrap().rendition_ids, vec![current.header.id.clone()]);
    let row = w.s.auto_commit_table(&w.admin, &w.v, None, 10).unwrap().entries.remove(0).row;
    assert_eq!((row.correct, row.incorrect), (correct_before + 1, 0));
}

#[test]
fn a_person_clearing_a_stale_proposal_is_not_held_against_the_extractor() {
    let w = world();
    let text = long_text(SENTENCE);
    let source = w.ingest("a.txt", "unverified", &text);
    w.extract(&ScriptedAgent::new(vec![claim_reply(SENTENCE, "Fartyget", "byggdes i", "Stockholm")]));
    let old = w.changesets().pop().unwrap();
    w.s.set_extraction(None);
    w.s.reprocess_source(&w.admin, &w.v, &source).unwrap();
    w.parse(&source, World::plain(&text));
    // With extraction off nothing retires it, and a person can still clear it away.
    assert_eq!(w.s.get_changeset(&w.admin, &w.v, &old.header.id).unwrap().state, ChangeSetState::Pending);
    w.s.reject(&w.admin, &w.v, &old.header.id, "out of date".into()).unwrap();
    assert!(w.s.auto_commit_table(&w.admin, &w.v, None, 10).unwrap().entries.is_empty());
}

#[test]
fn a_new_proposal_citing_a_replaced_rendition_is_refused() {
    let w = world();
    let text = long_text(SENTENCE);
    let source = w.ingest("a.txt", "unverified", &text);
    let segment = w.table(Table::Segment).into_iter().next().unwrap().header().id.clone();
    w.s.reprocess_source(&w.admin, &w.v, &source).unwrap();
    w.parse(&source, World::plain(&text));
    let op = Op::AddAssertion {
        id: ObjectId("a".into()),
        subject: "s".into(),
        predicate: "p".into(),
        value: json!(1),
        epistemic: Epistemic::Asserted,
        machine: false,
        confidence_permille: 1000,
        valid_from_ms: None,
        valid_to_ms: None,
        evidence: vec![(
            ObjectId("e".into()),
            EvidenceInput {
                segment_id: segment,
                quote: SENTENCE.into(),
                stance: Stance::Supports,
            },
        )],
    };
    let out =
        w.s.propose(
            &w.admin,
            &w.v,
            Proposal {
                ops: vec![op],
                rationale: "r".into(),
                idempotency_key: "k".into(),
            },
            false,
        )
        .unwrap();
    assert_eq!(out.risk, RiskDecision::Reject);
    assert!(out.validators.iter().any(|v| v.validator == "current_rendition" && !v.passed));
}

// ---- The queue. ----

#[test]
fn the_queue_filters_counts_and_pages() {
    let w = world();
    let a = w.pending_claim(0);
    w.review_one(&ScriptedAgent::new(vec![verdict_reply("approve", "ok")])).unwrap();
    let b = w.pending_claim(1);
    w.review_one(&ScriptedAgent::new(vec![verdict_reply("reject", "no")])).unwrap();
    let c = w.pending_claim(2);
    let list = |f: ReviewFilter| -> Vec<ObjectId> {
        w.s.review_queue(&w.reader, &w.v, &f, None, 10)
            .unwrap()
            .into_iter()
            .map(|i| i.changeset.header.id)
            .collect()
    };
    let pending = || ReviewFilter {
        state: Some(ChangeSetState::Pending),
        ..Default::default()
    };
    assert_eq!(list(pending()), [a.header.id.clone(), b.header.id.clone(), c.header.id.clone()]);
    let with = |v| ReviewFilter { verdict: Some(v), ..pending() };
    assert_eq!(list(with(VerdictFilter::Is(Verdict::Approve))), std::slice::from_ref(&a.header.id));
    assert_eq!(list(with(VerdictFilter::Is(Verdict::Reject))), std::slice::from_ref(&b.header.id));
    assert_eq!(list(with(VerdictFilter::None)), std::slice::from_ref(&c.header.id));
    assert_eq!(list(with(VerdictFilter::Is(Verdict::Abstain))), Vec::<ObjectId>::new());
    let of_source = a.origin.as_ref().unwrap().source_ids[0].clone();
    assert_eq!(
        list(ReviewFilter {
            source_id: Some(of_source),
            ..pending()
        }),
        std::slice::from_ref(&a.header.id)
    );
    assert_eq!(
        list(ReviewFilter {
            extractor: Some("scripted".into()),
            prompt_version: Some(prompt_hash()),
            item_type: Some("claim".into()),
            proposer: Some("extractor".into()),
            ..pending()
        })
        .len(),
        3
    );
    assert!(
        list(ReviewFilter {
            item_type: Some("page".into()),
            ..pending()
        })
        .is_empty()
    );
    // Paging, with the verdict alongside.
    let page = w.s.review_queue(&w.reader, &w.v, &pending(), Some(&a.header.id), 1).unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].changeset.header.id, b.header.id);
    assert_eq!(page[0].verdict.as_ref().unwrap().verdict, Verdict::Reject);

    let counts = w.s.review_counts(&w.reader, &w.v).unwrap();
    assert_eq!(counts.iter().map(|c| c.count).sum::<u64>(), 3);
    assert_eq!(counts.iter().find(|c| c.verdict == Some(Verdict::Approve)).unwrap().count, 1);
    assert_eq!(counts.iter().find(|c| c.verdict.is_none()).unwrap().item_type, "claim");
    assert_eq!(w.s.pending_count(&w.reader, &w.v).unwrap(), 3);
    w.s.approve(&w.admin, &w.v, &a.header.id, None).unwrap();
    assert_eq!(w.s.pending_count(&w.reader, &w.v).unwrap(), 2);
}

#[test]
fn an_entity_proposal_counts_under_its_type_and_shows_the_reviewer_no_ids() {
    let w = world();
    let sentence = "Ada Lovelace worked as a mathematician in London for many years.";
    w.ingest("e.txt", "unverified", &long_text(sentence));
    let doc = json!({"claims": [], "entities": {"Person": [{"segment_id": "s1", "quote": sentence, "fields": {"name": "Ada Lovelace"}}]}});
    w.extract(&ScriptedAgent::new(vec![reply(doc)]));
    let cs = w.changesets().pop().unwrap();
    assert_eq!(cs.state, ChangeSetState::Pending, "{:?}", cs.decision_reason);
    assert_eq!(cs.origin.as_ref().unwrap().item_types, ["entity:Person"]);
    let agent = ScriptedAgent::new(vec![verdict_reply("approve", "ok")]);
    w.review_one(&agent).unwrap();
    let shown = &agent.requests()[0].input;
    assert!(shown.contains("a Person"), "{shown}");
    assert!(!shown.contains("entity:"), "{shown}");
}

#[test]
fn a_batch_decides_each_on_its_own_and_is_attributed() {
    use memex_core::BatchDecision;
    let w = world();
    let (a, b, c) = (w.pending_claim(0), w.pending_claim(1), w.pending_claim(2));
    w.s.approve(&w.other_admin, &w.v, &c.header.id, None).unwrap();
    let results =
        w.s.decide_batch(
            &w.admin,
            &w.v,
            vec![
                BatchDecision::Approve {
                    id: a.header.id.clone(),
                    reason: Some("fine".into()),
                },
                BatchDecision::Reject {
                    id: b.header.id.clone(),
                    reason: String::new(),
                },
                BatchDecision::Approve {
                    id: c.header.id.clone(),
                    reason: None,
                },
                BatchDecision::Reject {
                    id: b.header.id.clone(),
                    reason: "not so".into(),
                },
            ],
        )
        .unwrap();
    assert_eq!(results[0].as_ref().unwrap().decided_by.as_deref(), Some(w.admin.principal.id.as_str()));
    assert!(matches!(results[1], Err(ServerError::Invalid(_))), "a rejection needs a reason");
    assert!(matches!(results[2], Err(ServerError::Conflict(_))), "someone else decided it");
    assert_eq!(results[3].as_ref().unwrap().state, ChangeSetState::Rejected);
    assert!(matches!(w.s.decide_batch(&w.reader, &w.v, vec![]), Err(ServerError::Forbidden)));
    let too_many = (0..201)
        .map(|i| BatchDecision::Approve {
            id: ObjectId(i.to_string()),
            reason: None,
        })
        .collect();
    assert!(matches!(w.s.decide_batch(&w.admin, &w.v, too_many), Err(ServerError::Invalid(_))));
}

#[test]
fn the_log_replays_verdicts_samples_and_decisions_with_no_llm() {
    let w = world();
    let cs = w.pending_claim(0);
    w.review_one(&ScriptedAgent::new(vec![verdict_reply("approve", "ok")])).unwrap();
    w.s.approve(&w.admin, &w.v, &cs.header.id, None).unwrap();
    w.s.reject(&w.admin, &w.v, &w.pending_claim(1).header.id, "no".into()).unwrap();
    let row = w.samples("scripted", &prompt_hash(), "claim", 100, 0);
    w.s.disable_auto_commit(&w.admin, &w.v, &row.header.id, "r".into()).unwrap();
    let snap = w.s.engine().store().snapshot(&w.v).unwrap();
    let source = dump_store(&snap).unwrap().sha256;
    let events = memex_core::all_events(&snap).unwrap();
    drop(snap);
    let target = MemStore::new();
    let lease = target.open_vault(&w.v).unwrap();
    memex_core::replay(events, &target, &lease).unwrap();
    assert_eq!(dump_store(&target.snapshot(&w.v).unwrap()).unwrap().sha256, source);
}

#[test]
fn a_pending_quote_gets_the_same_rects_it_is_anchored_to_at_commit() {
    let w = world();
    let text = "Fartyget nummer 0 byggdes i Stockholm år 1600 och seglade sedan mot Göteborg.";
    let source = w.upload("a.pdf", "trusted", json!({}), text);
    w.parse(&source, pdf_of(text));
    let segment = w
        .table(Table::Segment)
        .into_iter()
        .find_map(|r| if let Row::Segment(s) = r { Some(s.header.id) } else { None })
        .unwrap();
    let cs =
        w.s.propose(&w.agent, &w.v, agent_claim("c1", &[&segment], "byggdes i Stockholm"), false)
            .unwrap()
            .changeset
            .unwrap();
    assert_eq!(cs.state, ChangeSetState::Pending);
    let pending = w.s.review_item(&w.admin, &w.v, &cs.header.id).unwrap().evidence;
    assert_eq!(pending.len(), 1);
    assert!(!pending[0].rects.is_empty(), "a quote on a page has a place on it");
    w.s.approve(&w.admin, &w.v, &cs.header.id, None).unwrap();
    let stored = w
        .table(Table::Evidence)
        .into_iter()
        .find_map(|r| if let Row::Evidence(e) = r { Some(e) } else { None })
        .unwrap();
    assert_eq!(stored.anchor.unwrap().rects, pending[0].rects);
}

#[test]
fn a_quote_with_no_page_has_no_rects_and_the_item_still_opens() {
    let w = world();
    let source = w.ingest("a.txt", "trusted", SENTENCE);
    let segment = w
        .table(Table::Segment)
        .into_iter()
        .find_map(|r| if let Row::Segment(s) = r { Some(s.header.id) } else { None })
        .unwrap();
    let cs =
        w.s.propose(&w.agent, &w.v, agent_claim("c1", &[&segment], "byggdes i Stockholm"), false)
            .unwrap()
            .changeset
            .unwrap();
    let detail = w.s.review_item(&w.admin, &w.v, &cs.header.id).unwrap();
    assert_eq!(detail.evidence[0].source_id.as_ref(), Some(&source));
    assert!(detail.evidence[0].rects.is_empty());
}
