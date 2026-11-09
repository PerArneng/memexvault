//! Extraction through the facade, against a scripted agent. Its replies are written here, so these tests prove the machinery around a model, never a model's quality.
use memex_core::testing::{ManualClock, MemBlobStore, MemRegistry, MemStore, ScriptedAgent, SeededRandom};
use memex_core::{
    Caller, EXTRACT_JOB, ExtractJobError, ExtractionLimits, ExtractionState, RENDITION_JOB, RenditionInput, Server, ServerEdges, Snapshot, Store, UploadLimits,
    UploadRequest,
};
use memex_types::{
    AgentError, AgentReply, AgentRequest, AgentUsage, ChangeSetRow, ChangeSetState, DerivationOutcome, DerivationRow, DocKind, ExtractedItem, ExtractionStop,
    JobRow, JobState, LexicalFilter, ObjectId, PageClass, PageFacts, ParsedDocument, Rect, Row, Table, VaultId, Word,
};
use serde_json::{Value, json};
use std::time::Duration;

struct Fakes;

impl ServerEdges for Fakes {
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
    type Caches = memex_core::NoCaches;
}

struct World {
    s: Server<Fakes>,
    admin: Caller,
    v: VaultId,
}

const USAGE: AgentUsage = AgentUsage {
    input_tokens: 700,
    output_tokens: 90,
    thinking_tokens: 11,
};

fn world_with(limits: ExtractionLimits) -> World {
    let s = Server::<Fakes>::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(1_000_000),
        SeededRandom::new(9),
        memex_core::testing::mem_search(),
    );
    let admin = s.authenticate(&s.bootstrap("root").unwrap().token).unwrap();
    let v = s.create_vault(&admin, "v").unwrap().id;
    s.set_extraction(Some(limits));
    World { s, admin, v }
}

fn world() -> World {
    world_with(ExtractionLimits::default())
}

/// Text in `n` blank-line separated parts, each long enough to be a segment of its own.
fn document(n: usize) -> String {
    (0..n)
        .map(|i| {
            let filler = "Detta är en lång mening om ingenting särskilt som bara fyller ut stycket med ord. ".repeat(12);
            format!("Fartyget nummer {i} byggdes i Stockholm år 16{i:02}. {filler}")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

impl World {
    fn upload(&self, name: &str, text: &str) -> ObjectId {
        let req = UploadRequest {
            name: name.into(),
            trust_tier: "unverified".into(),
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

    /// Runs the parse stage by hand, which queues extraction with the rendition.
    fn ingest(&self, text: &str) -> ObjectId {
        let source = self.upload("a.txt", text);
        self.parse(
            &source,
            RenditionInput::Plain {
                text: text.into(),
                parser: "plain-1".into(),
            },
        );
        source
    }

    fn parse(&self, source: &ObjectId, input: RenditionInput) {
        let job = self.s.claim_job(&self.v, RENDITION_JOB, "parser").unwrap().unwrap();
        let rows = self.s.build_rendition(&self.v, source, input).unwrap();
        assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
    }

    /// One worker's turn: what `workers::run_one` does with a job.
    fn work(&self, agent: &ScriptedAgent) -> Option<(JobRow, Result<(), ExtractJobError>)> {
        let job = self.s.claim_job(&self.v, EXTRACT_JOB, "w").unwrap()?;
        let result = match self.s.run_extraction_job(&self.v, &job, agent) {
            Ok(rows) => {
                assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
                Ok(())
            }
            Err(ExtractJobError::Wait(m)) => {
                assert!(self.s.wait_job(&self.v, &job, &m).unwrap());
                Err(ExtractJobError::Wait(m))
            }
            Err(ExtractJobError::Retry(m)) => {
                assert!(self.s.fail_job(&self.v, &job, &m, true).unwrap());
                Err(ExtractJobError::Retry(m))
            }
            Err(ExtractJobError::Fatal(m)) => {
                assert!(self.s.fail_job(&self.v, &job, &m, false).unwrap());
                Err(ExtractJobError::Fatal(m))
            }
            Err(ExtractJobError::Lost) => Err(ExtractJobError::Lost),
        };
        Some((job, result))
    }

    fn work_ok(&self, agent: &ScriptedAgent) {
        let (_, result) = self.work(agent).expect("a job to run");
        result.unwrap();
    }

    fn status(&self, source: &ObjectId) -> memex_core::ExtractionReport {
        self.s.extraction_status(&self.admin, &self.v, source).unwrap()
    }

    fn rendition(&self, source: &ObjectId) -> ObjectId {
        self.status(source).rendition_id.unwrap()
    }

    fn derivations(&self, source: &ObjectId) -> Vec<DerivationRow> {
        self.s.list_derivations(&self.admin, &self.v, &self.rendition(source), None, 200).unwrap()
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

    fn approve_all(&self) {
        for c in self.s.list_changesets(&self.admin, &self.v, Some(ChangeSetState::Pending), None, 1000).unwrap() {
            self.s.approve(&self.admin, &self.v, &c.header.id, None).unwrap();
        }
    }
}

fn reply(doc: Value) -> Result<AgentReply, AgentError> {
    Ok(AgentReply {
        json: doc.to_string(),
        usage: USAGE,
        tool_calls: 0,
    })
}

fn segments_of(req: &AgentRequest) -> Vec<(String, String)> {
    let doc: Value = serde_json::from_str(req.input.split("\n\nYour previous").next().unwrap()).unwrap();
    doc["segments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["id"].as_str().unwrap().to_string(), s["text"].as_str().unwrap().to_string()))
        .collect()
}

/// One true claim per segment: its first sentence, as a model that behaves would return it.
fn claims_each(req: &AgentRequest) -> Result<AgentReply, AgentError> {
    let claims: Vec<Value> = segments_of(req)
        .into_iter()
        .map(|(id, text)| {
            let quote = text.split_inclusive(". ").next().unwrap().trim().to_string();
            json!({"segment_id": id, "quote": quote, "subject": quote, "predicate": "säger", "value": "byggdes"})
        })
        .collect();
    reply(json!({"claims": claims, "entities": {}}))
}

const ONE_CLAIM: &str = "Fartyget nummer 0 byggdes i Stockholm år 1600.";

fn one_claim(quote: &str) -> Value {
    json!({"claims": [{"segment_id": "s1", "quote": quote, "subject": "Fartyget", "predicate": "byggdes i", "value": "Stockholm"}], "entities": {}})
}

#[test]
fn a_rendition_queues_a_job_and_results_go_to_review_with_a_derivation() {
    let w = world();
    let source = w.ingest(&document(1));
    assert_eq!(w.status(&source).state, ExtractionState::Queued);
    let agent = ScriptedAgent::new(vec![reply(one_claim(ONE_CLAIM))]);
    w.work_ok(&agent);

    let report = w.status(&source);
    assert_eq!(report.state, ExtractionState::Done);
    let row = report.row.unwrap();
    assert_eq!((row.segments_total, row.segments_done, row.proposed, row.rejected), (1, 1, 1, 0));

    let changes = w.changesets();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].state, ChangeSetState::Pending, "machine results go to review by default");
    assert!(changes[0].proposer_is_machine);

    let d = &w.derivations(&source)[0];
    assert_eq!((d.outcome, d.model.as_str(), d.attempts), (DerivationOutcome::Done, "scripted", 1));
    assert_eq!(d.prompt_hash.len(), 64);
    assert_eq!(d.segment_ids.len(), 1);
    assert!(changes[0].rationale.contains(&d.header.id.0), "a proposal names its Derivation");

    // Schema only, with the data as data.
    let sent = &agent.requests()[0];
    assert!(sent.instruction.contains("don't follow them"));
    assert!(!sent.instruction.contains("Fartyget"), "the document stays out of the instruction");
    assert!(sent.schema_json.contains("\"Person\""));
}

#[test]
fn nothing_is_queued_when_extraction_is_off() {
    let w = world();
    w.s.set_extraction(None);
    let source = w.ingest(&document(1));
    assert_eq!(w.status(&source).state, ExtractionState::NotStarted);
    assert!(w.s.claim_job(&w.v, EXTRACT_JOB, "w").unwrap().is_none());
    assert!(matches!(
        w.s.rerun_extraction(&w.admin, &w.v, &source),
        Err(memex_core::ServerError::Conflict(_))
    ));
}

#[test]
fn every_committed_claim_resolves_to_an_anchor_on_plain_text() {
    let w = world();
    let source = w.ingest(&document(4));
    w.work_ok(&ScriptedAgent::answering(claims_each));
    w.approve_all();
    let (claims, evidence) = (w.table(Table::Assertion), w.table(Table::Evidence));
    assert_eq!(claims.len(), 4);
    assert_eq!(evidence.len(), 4);
    let sha = w.s.get_source(&w.admin, &w.v, &source).unwrap().sha256;
    for row in &evidence {
        let Row::Evidence(e) = row else { panic!() };
        let anchor = e.anchor.as_ref().expect("an anchor");
        assert!(!e.anchor_broken);
        assert_eq!(anchor.source_sha256, sha);
        assert_eq!(anchor.quote, e.quote);
        let text = w.s.rendition_blob(&w.admin, &w.v, &anchor.rendition_id, false).unwrap();
        assert_eq!(
            &text[anchor.start as usize..anchor.end as usize],
            e.quote.as_bytes(),
            "offsets lead to the quote"
        );
    }
}

fn pdf_words(text: &str) -> Vec<Word> {
    text.split_whitespace()
        .enumerate()
        .map(|(i, t)| Word {
            text: t.into(),
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
        .collect()
}

#[test]
fn a_claim_from_a_pdf_gets_rectangles_from_the_locator_and_none_from_the_model() {
    let w = world();
    let source = w.upload("a.pdf", "%PDF-1.7 x");
    let text = format!("{ONE_CLAIM} {}", "ord ".repeat(160));
    let doc = ParsedDocument {
        parser: "test".into(),
        pages: vec![PageFacts {
            index: 0,
            width: 61200,
            height: 79200,
            rotation: 0,
            class: PageClass::Text,
            hidden_text_objects: 0,
        }],
        words: pdf_words(&text),
    };
    w.parse(&source, RenditionInput::Pdf(doc, vec![]));
    let agent = ScriptedAgent::new(vec![reply(one_claim("byggdes i Stockholm år 1600."))]);
    w.work_ok(&agent);
    w.approve_all();
    let [Row::Evidence(e)] = &w.table(Table::Evidence)[..] else { panic!() };
    let anchor = e.anchor.as_ref().unwrap();
    assert!(!anchor.rects.is_empty(), "rectangles come from the word table");
    assert_eq!(anchor.rects[0].0, 0);
    assert!(!agent.requests()[0].schema_json.contains("rect"), "the model isn't asked for boxes");
}

#[test]
fn a_quote_found_only_in_another_segment_is_refused_and_anchors_stay_in_their_segment() {
    let w = world();
    let same = "Samma mening finns i båda styckena. ";
    let filler = "Mer text som bara fyller ut stycket med en del ord. ".repeat(20);
    let source = w.ingest(&format!("{same}{filler}\n\nDet här stycket är annorlunda. {same}{filler}"));
    let d = {
        w.work_ok(&ScriptedAgent::new(vec![reply(json!({"claims": [
            {"segment_id": "s2", "quote": "Samma mening finns i båda styckena.", "subject": "a", "predicate": "b", "value": "c"},
        ], "entities": {}}))]));
        w.derivations(&source).remove(0)
    };
    assert_eq!(d.segment_ids.len(), 2);
    w.approve_all();
    let [Row::Evidence(e)] = &w.table(Table::Evidence)[..] else { panic!() };
    let Some(Row::Segment(seg)) = w.table(Table::Segment).into_iter().find(|r| r.header().id == d.segment_ids[1]) else {
        panic!()
    };
    let anchor = e.anchor.as_ref().unwrap();
    assert!(seg.start <= anchor.start && anchor.end <= seg.end, "it sits in the segment the model named");
    assert!(anchor.start > 100);
}

#[test]
fn invented_quotes_never_enter_the_system() {
    let w = world();
    let source = w.ingest(&document(1));
    let agent = ScriptedAgent::new(vec![reply(json!({"claims": [
        {"segment_id": "s1", "quote": "The ship was built in Stockholm", "subject": "Fartyget", "predicate": "built in", "value": "Stockholm"},
        {"segment_id": "s7", "quote": "Fartyget nummer 0", "subject": "Fartyget", "predicate": "is", "value": "number 0"},
    ], "entities": {"Person": [
        {"segment_id": "s1", "quote": "Gustav Vasa ledde bygget", "fields": {"name": "Gustav Vasa"}},
    ]}}))]);
    w.work_ok(&agent);
    assert!(w.changesets().is_empty(), "no proposal at all");
    assert!(w.table(Table::Assertion).is_empty());
    let d = &w.derivations(&source)[0];
    assert!(d.items.is_empty());
    assert_eq!(d.rejected.len(), 3);
    assert!(d.rejected.iter().any(|r| r.reason.contains("isn't in segment")));
    assert_eq!(w.status(&source).row.unwrap().rejected, 3);
    assert_eq!(agent.requests().len(), 1, "a made-up quote isn't a reason to pay for a retry");
}

#[test]
fn wrong_schema_output_retries_then_fails_with_its_tokens_on_record() {
    let w = world();
    let source = w.ingest(&document(1));
    let bad = || reply(json!({"facts": ["not", "the", "schema"]}));
    let agent = ScriptedAgent::new(vec![bad(), bad()]);
    let (_, result) = w.work(&agent).unwrap();
    assert!(matches!(result, Err(ExtractJobError::Fatal(_))), "{result:?}");
    assert_eq!(agent.requests().len(), 2, "one retry");
    assert!(agent.requests()[1].input.contains("Your previous reply was invalid"));
    let report = w.status(&source);
    assert_eq!(report.state, ExtractionState::Failed);
    assert!(report.last_error.unwrap().contains("nothing usable"));
    assert!(w.changesets().is_empty());
    let d = &w.derivations(&source)[0];
    assert_eq!((d.outcome, d.attempts, d.usage.input_tokens), (DerivationOutcome::Failed, 2, 1400));
    assert!(w.s.claim_job(&w.v, EXTRACT_JOB, "w").unwrap().is_none(), "dead, until someone asks");
}

#[test]
fn a_retry_that_gets_it_right_succeeds_and_counts_both_calls() {
    let w = world();
    let source = w.ingest(&document(1));
    w.work_ok(&ScriptedAgent::new(vec![reply(json!({"oops": 1})), reply(one_claim(ONE_CLAIM))]));
    let d = &w.derivations(&source)[0];
    assert_eq!((d.outcome, d.attempts, d.usage.output_tokens), (DerivationOutcome::Done, 2, 180));
    assert_eq!(w.changesets().len(), 1);
}

#[test]
fn one_bad_batch_leaves_the_rest_done_and_the_document_partial_and_a_rerun_finishes_it() {
    let w = world_with(ExtractionLimits {
        batch_tokens: 50,
        ..ExtractionLimits::default()
    });
    let source = w.ingest(&document(3));
    let bad = || reply(json!({"oops": 1}));
    let agent = ScriptedAgent::new(vec![bad(), bad()]).then(claims_each);
    w.work_ok(&agent);
    let report = w.status(&source);
    assert_eq!(report.state, ExtractionState::Partial);
    let row = report.row.unwrap();
    assert_eq!(
        (row.segments_total, row.segments_done, row.batches_failed, row.stop),
        (3, 2, 1, Some(ExtractionStop::FailedBatches))
    );
    assert_eq!(w.changesets().len(), 2);

    let job = w.s.rerun_extraction(&w.admin, &w.v, &source).unwrap();
    assert_eq!(job.state, JobState::Queued);
    let agent = ScriptedAgent::answering(claims_each);
    w.work_ok(&agent);
    assert_eq!(agent.requests().len(), 1, "only the failed segment goes again");
    assert_eq!(w.status(&source).state, ExtractionState::Done);
}

/// A reply that fits the first segment of the document: what an agent would answer in its first call.
trait FirstReply {
    fn run_for_test(&self, w: &World) -> Result<AgentReply, AgentError>;
}

impl FirstReply for ScriptedAgent {
    fn run_for_test(&self, w: &World) -> Result<AgentReply, AgentError> {
        let Some(Row::Segment(first)) = w.table(Table::Segment).into_iter().next() else {
            panic!()
        };
        claims_each(&AgentRequest {
            input: json!({"segments": [{"id": "s1", "text": first.text}]}).to_string(),
            ..probe()
        })
    }
}

fn probe() -> AgentRequest {
    AgentRequest {
        instruction: String::new(),
        input: json!({"segments": [{"id": "s1", "text": "Fartyget nummer 0 byggdes i Stockholm år 1600. Mer."}]}).to_string(),
        schema_json: String::new(),
        budget: memex_types::AgentBudget {
            max_output_tokens: 1,
            timeout_ms: 1,
        },
    }
}

#[test]
fn an_outage_leaves_the_job_waiting_uses_no_attempts_and_everything_else_stays_available() {
    let w = world();
    let source = w.ingest(&document(1));
    // The projector indexes what's there, as it does while the LLM is away.
    while w.s.project_lexical(&w.v).unwrap() == memex_core::StepOutcome::Progress {}
    let down = ScriptedAgent::new(vec![]);
    let attempts_before = w.s.claim_job(&w.v, RENDITION_JOB, "x").unwrap();
    assert!(attempts_before.is_none());
    for round in 0..30 {
        let (_, result) = w.work(&down).unwrap_or_else(|| panic!("the job should be runnable in round {round}"));
        assert!(matches!(result, Err(ExtractJobError::Wait(_))));
        let report = w.status(&source);
        assert_eq!(report.state, ExtractionState::Waiting, "round {round}");
        assert_eq!(report.waiting_since_ms, Some(1_000_000));
        assert!(report.last_error.unwrap().contains("unavailable"));
        assert!(w.s.claim_job(&w.v, EXTRACT_JOB, "w").unwrap().is_none(), "it holds back until it's due");
        w.s.engine().clock().advance(Duration::from_millis(memex_core::jobs::WAIT_CAP_MS as u64 * 2));
    }
    let job = w.s.list_jobs(&w.admin, &w.v, Some(EXTRACT_JOB), None, None, 10).unwrap().remove(0);
    assert!(job.attempts.is_empty(), "thirty rounds used none of its attempts");
    assert_eq!(down.requests().len(), 30);

    // Search and reading answer as usual.
    let found =
        w.s.search_lexical(
            &w.admin,
            &w.v,
            "Stockholm",
            LexicalFilter {
                kinds: vec![DocKind::Segment],
                ..LexicalFilter::default()
            },
            5,
        )
        .unwrap();
    assert!(!found.hits.is_empty());
    assert!(w.s.get_source(&w.admin, &w.v, &source).is_ok());
    // Uploads don't count waiting extractions as backlog. Nothing else is open, so the limit of one has room.
    let tight = UploadLimits {
        max_open_jobs: 1,
        ..UploadLimits::default()
    };
    let req = UploadRequest {
        name: "b.txt".into(),
        trust_tier: "unverified".into(),
        metadata: json!({}),
        replaces: None,
    };
    w.s.upload(&w.admin, &w.v, req, tight, &mut "more".as_bytes()).unwrap();

    // It comes back, and the job picks up where it was.
    w.work_ok(&ScriptedAgent::answering(claims_each));
    let report = w.status(&source);
    assert_eq!(report.state, ExtractionState::Done);
    assert_eq!(report.waiting_since_ms, None);
    assert_eq!(w.changesets().len(), 1);
}

#[test]
fn every_kind_of_outage_waits_and_a_provider_refusal_is_an_ordinary_failure() {
    for e in [AgentError::Unavailable("x".into()), AgentError::RateLimited("x".into()), AgentError::Timeout] {
        let w = world();
        let source = w.ingest(&document(1));
        let (_, result) = w.work(&ScriptedAgent::new(vec![Err(e)])).unwrap();
        assert!(matches!(result, Err(ExtractJobError::Wait(_))));
        assert_eq!(w.status(&source).state, ExtractionState::Waiting);
    }
    let w = world();
    let source = w.ingest(&document(1));
    let (job, result) = w
        .work(&ScriptedAgent::new(vec![Err(AgentError::Provider("400 schema refused".into()))]))
        .unwrap();
    assert!(matches!(result, Err(ExtractJobError::Retry(_))));
    assert_eq!(w.status(&source).state, ExtractionState::Queued);
    let now = w.s.get_job(&w.admin, &w.v, &job.header.id).unwrap();
    assert_eq!(now.attempts.len(), 1, "this one counts");
    assert!(now.attempts[0].error.as_ref().unwrap().contains("400"));
}

#[test]
fn rerunning_the_same_rendition_and_prompt_creates_nothing_and_calls_nothing() {
    let w = world();
    let source = w.ingest(&document(3));
    w.work_ok(&ScriptedAgent::answering(claims_each));
    let (changes, derivations) = (w.changesets().len(), w.derivations(&source).len());
    assert_eq!(changes, 3);
    for _ in 0..3 {
        w.s.rerun_extraction(&w.admin, &w.v, &source).unwrap();
        let agent = ScriptedAgent::answering(claims_each);
        w.work_ok(&agent);
        assert!(agent.requests().is_empty());
        assert_eq!((w.changesets().len(), w.derivations(&source).len()), (changes, derivations));
        assert_eq!(w.status(&source).state, ExtractionState::Done);
    }
    w.approve_all();
    assert_eq!(w.table(Table::Assertion).len(), 3);
    // Once approved, the same facts still make nothing new.
    w.s.rerun_extraction(&w.admin, &w.v, &source).unwrap();
    w.work_ok(&ScriptedAgent::answering(claims_each));
    assert_eq!(w.table(Table::Assertion).len(), 3);
}

#[test]
fn a_new_prompt_calls_again_but_the_same_facts_are_not_proposed_twice() {
    let w = world();
    let source = w.ingest(&document(1));
    w.work_ok(&ScriptedAgent::answering(claims_each));
    assert_eq!(w.changesets().len(), 1);
    add_vessel(&w);
    let before = w.changesets().len();
    w.s.rerun_extraction(&w.admin, &w.v, &source).unwrap();
    let agent = ScriptedAgent::answering(claims_each);
    w.work_ok(&agent);
    assert_eq!(agent.requests().len(), 1, "the prompt changed");
    assert_eq!(w.changesets().len(), before);
    assert_eq!(w.derivations(&source).len(), 2);
}

#[test]
fn asking_for_a_rerun_while_one_is_open_gets_that_one() {
    let w = world();
    let source = w.ingest(&document(1));
    let first = w.s.rerun_extraction(&w.admin, &w.v, &source).unwrap();
    assert_eq!(first.header.id, w.status(&source).job_id.unwrap());
    let again = w.s.rerun_extraction(&w.admin, &w.v, &source).unwrap();
    assert_eq!(first.header.id, again.header.id);
    assert_eq!(w.s.list_jobs(&w.admin, &w.v, Some(EXTRACT_JOB), None, None, 10).unwrap().len(), 1);
}

#[test]
fn a_rerun_needs_propose_scope_and_a_rendition() {
    let w = world();
    let source = w.upload("a.txt", "unparsed");
    assert!(matches!(
        w.s.rerun_extraction(&w.admin, &w.v, &source),
        Err(memex_core::ServerError::Conflict(_))
    ));
    let p = w.s.create_principal(&w.admin, "reader", memex_types::PrincipalKind::Human, false).unwrap();
    let t =
        w.s.create_token(
            &w.admin,
            memex_core::NewToken {
                principal_id: Some(p.id),
                name: "t".into(),
                grants: vec![memex_types::Grant {
                    vault: w.v.clone(),
                    scope: memex_types::Scope::Read,
                }],
                expires_at_ms: None,
            },
        )
        .unwrap();
    let reader = w.s.authenticate(&t.token).unwrap();
    assert!(matches!(w.s.rerun_extraction(&reader, &w.v, &source), Err(memex_core::ServerError::Forbidden)));
    assert!(w.s.extraction_status(&reader, &w.v, &source).is_ok());
}

fn add_vessel(w: &World) {
    let schema = json!({"type": "object", "additionalProperties": false, "required": ["name"], "properties": {
        "name": {"type": "string", "searchable": true, "identity": true, "filterable": true},
        "built_in": {"type": "string", "filterable": true},
    }});
    let o =
        w.s.propose_entity_type(&w.admin, &w.v, "Vessel", None, schema, "ships matter".into(), "vessel".into())
            .unwrap();
    assert_eq!(o.changeset.unwrap().state, ChangeSetState::Committed);
}

#[test]
fn a_type_added_to_the_registry_is_extracted_without_a_code_change() {
    let w = world();
    add_vessel(&w);
    let source = w.ingest(&document(1));
    let agent = ScriptedAgent::new(vec![reply(json!({"claims": [], "entities": {"Vessel": [
        {"segment_id": "s1", "quote": "Fartyget nummer 0 byggdes i Stockholm", "fields": {"name": "Fartyget nummer 0", "built_in": "Stockholm"}},
    ]}}))]);
    w.work_ok(&agent);
    let sent = &agent.requests()[0];
    assert!(sent.instruction.contains("- Vessel: built_in, name*"), "{}", sent.instruction);
    assert!(sent.schema_json.contains("\"Vessel\""));
    assert_eq!(w.status(&source).row.unwrap().proposed, 1);
    w.approve_all();
    let [Row::Entity(entity)] = &w.table(Table::Entity)[..] else { panic!() };
    assert_eq!(entity.type_name, "Vessel");
    let view = w.s.get_entity(&w.admin, &w.v, &entity.header.id).unwrap();
    assert_eq!(view.claims.len(), 2, "each field is a claim with its evidence");
    for claim in view.claims.values() {
        assert!(claim.assertion.machine);
        assert!(claim.evidence[0].evidence.anchor.is_some());
    }
    let found = w.s.list_entities(
        &w.admin,
        &w.v,
        "Vessel",
        &[("built_in".to_string(), "eq".to_string(), "Stockholm".to_string())],
        None,
        10,
    );
    assert!(found.is_ok_and(|p| p.len() == 1));
}

#[test]
fn a_value_its_type_refuses_fails_the_schema_check_before_any_proposal() {
    let w = world();
    add_vessel(&w);
    let source = w.ingest(&document(1));
    let wrong = || reply(json!({"claims": [], "entities": {"Vessel": [{"segment_id": "s1", "quote": "Fartyget", "fields": {"built_in": "Stockholm"}}]}}));
    let (_, result) = w.work(&ScriptedAgent::new(vec![wrong(), wrong()])).unwrap();
    assert!(matches!(result, Err(ExtractJobError::Fatal(_))));
    assert_eq!(w.status(&source).state, ExtractionState::Failed);
    assert_eq!(w.changesets().len(), 1, "only the type's own");
}

#[test]
fn identity_resolution_names_candidates_and_never_merges() {
    let w = world();
    let source = w.ingest(&document(1));
    let person = json!({"claims": [], "entities": {"Person": [{"segment_id": "s1", "quote": "Fartyget nummer 0", "fields": {"name": "Anna Berg"}}]}});
    w.work_ok(&ScriptedAgent::new(vec![reply(person.clone())]));
    w.approve_all();
    let [Row::Entity(first)] = &w.table(Table::Entity)[..] else { panic!() };

    let other = w.ingest(&format!("{} Annat.", document(1)));
    w.work_ok(&ScriptedAgent::new(vec![reply(person.clone())]));
    let d = w.derivations(&other).remove(0);
    let ExtractedItem::Entity { candidates, .. } = &d.items[0] else { panic!() };
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].entity_id, first.header.id);
    assert_eq!(candidates[0].field, "name");
    let pending = w.s.list_changesets(&w.admin, &w.v, Some(ChangeSetState::Pending), None, 10).unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].rationale.contains(&first.header.id.0), "{}", pending[0].rationale);
    assert!(!pending[0].ops.iter().any(|o| matches!(o, memex_types::Op::MergeEntities { .. })));
    assert_eq!(w.table(Table::Entity).len(), 1, "nothing changes until someone approves");
    let _ = source;
}

#[test]
fn the_budget_stops_a_document_and_marks_it_partial() {
    let prompt_cost = memex_core::extraction::build_prompt(memex_core::entity::seed_types()).estimated_tokens;
    // Room for the first call as estimated, and too little for the second once the first has been paid for.
    let w = world_with(ExtractionLimits {
        document_budget: prompt_cost * 2 + 900,
        batch_tokens: 50,
        output_tokens: 256,
        ..ExtractionLimits::default()
    });
    let source = w.ingest(&document(3));
    let expensive = move |req: &AgentRequest| {
        let mut r = claims_each(req)?;
        r.usage.input_tokens = prompt_cost * 2 + 500;
        Ok(r)
    };
    let agent = ScriptedAgent::answering(expensive);
    w.work_ok(&agent);
    assert_eq!(agent.requests().len(), 1);
    let report = w.status(&source);
    assert_eq!(report.state, ExtractionState::Partial);
    let row = report.row.unwrap();
    assert_eq!((row.stop, row.segments_done, row.segments_total), (Some(ExtractionStop::Budget), 1, 3));
    assert_eq!(row.token_budget, prompt_cost * 2 + 900);

    // A re-run is the decision to spend more, and it finishes the document.
    w.s.rerun_extraction(&w.admin, &w.v, &source).unwrap();
    let agent = ScriptedAgent::answering(claims_each);
    w.work_ok(&agent);
    assert!(!agent.requests().is_empty());
}

#[test]
fn a_budget_too_small_for_any_call_calls_nothing() {
    let w = world_with(ExtractionLimits {
        document_budget: 10,
        ..ExtractionLimits::default()
    });
    let source = w.ingest(&document(1));
    let agent = ScriptedAgent::answering(claims_each);
    w.work_ok(&agent);
    assert!(agent.requests().is_empty());
    let report = w.status(&source);
    assert_eq!(report.state, ExtractionState::Partial);
    assert_eq!(report.row.unwrap().stop, Some(ExtractionStop::Budget));
}

#[test]
fn a_retry_that_would_go_over_budget_is_not_made() {
    let prompt_cost = memex_core::extraction::build_prompt(memex_core::entity::seed_types()).estimated_tokens;
    let w = world_with(ExtractionLimits {
        document_budget: prompt_cost + 1200,
        batch_tokens: 50,
        output_tokens: 256,
        ..ExtractionLimits::default()
    });
    let source = w.ingest(&document(1));
    let mut first = AgentReply {
        json: "{}".into(),
        usage: USAGE,
        tool_calls: 0,
    };
    first.usage.input_tokens = prompt_cost + 1000;
    let agent = ScriptedAgent::new(vec![Ok(first)]);
    w.work_ok(&agent);
    assert_eq!(agent.requests().len(), 1);
    let report = w.status(&source);
    assert_eq!(report.state, ExtractionState::Partial);
    assert_eq!(report.row.unwrap().stop, Some(ExtractionStop::Budget));
    assert_eq!(w.derivations(&source)[0].outcome, DerivationOutcome::Failed);
}

#[test]
fn what_a_run_has_spent_survives_a_wait() {
    let w = world_with(ExtractionLimits {
        batch_tokens: 50,
        ..ExtractionLimits::default()
    });
    let source = w.ingest(&document(2));
    let agent = ScriptedAgent::answering(claims_each);
    let first = agent.run_for_test(&w);
    let (_, result) = w.work(&ScriptedAgent::new(vec![first, Err(AgentError::Timeout)])).unwrap();
    assert!(matches!(result, Err(ExtractJobError::Wait(_))));
    assert_eq!(w.status(&source).row.unwrap().usage.input_tokens, USAGE.input_tokens);
    w.s.engine().clock().advance(Duration::from_secs(10));
    w.work_ok(&ScriptedAgent::answering(claims_each));
    let row = w.status(&source).row.unwrap();
    assert_eq!(row.usage.input_tokens, USAGE.input_tokens * 2);
    assert_eq!((row.segments_done, row.proposed), (2, 2));
}

#[test]
fn token_counts_on_derivations_match_what_the_agent_reported() {
    let w = world_with(ExtractionLimits {
        batch_tokens: 50,
        ..ExtractionLimits::default()
    });
    let source = w.ingest(&document(3));
    let reported = [(111, 22, 3), (444, 55, 6), (777, 88, 9)];
    let script: Vec<_> = reported
        .iter()
        .map(|(i, o, t)| {
            let mut r = claims_each(&probe()).unwrap();
            r.json = json!({"claims": [], "entities": {}}).to_string();
            r.usage = AgentUsage {
                input_tokens: *i,
                output_tokens: *o,
                thinking_tokens: *t,
            };
            Ok(r)
        })
        .collect();
    w.work_ok(&ScriptedAgent::new(script));
    let found: Vec<(u64, u64, u64)> = w
        .derivations(&source)
        .iter()
        .map(|d| (d.usage.input_tokens, d.usage.output_tokens, d.usage.thinking_tokens))
        .collect();
    assert_eq!(found, reported);
    let total = w.status(&source).row.unwrap().usage;
    assert_eq!((total.input_tokens, total.output_tokens, total.thinking_tokens), (1332, 165, 18));
}

#[test]
fn a_crash_after_a_derivation_makes_its_proposals_with_no_second_call() {
    let w = world();
    let source = w.ingest(&document(1));
    // A first worker gets its reply on record and dies before proposing.
    let held = w.s.claim_job(&w.v, EXTRACT_JOB, "dies").unwrap().unwrap();
    let segment = {
        let Some(Row::Segment(s)) = w.table(Table::Segment).into_iter().next() else {
            panic!()
        };
        s
    };
    let prompt = memex_core::extraction::build_prompt(memex_core::entity::seed_types());
    let derivation = DerivationRow {
        header: memex_types::Header {
            id: ObjectId("d1".into()),
            creator: "extractor".into(),
            status: memex_types::Status::Active,
            version: 1,
            acl_id: None,
        },
        kind: "extraction".into(),
        job_id: held.header.id.clone(),
        source_id: source.clone(),
        rendition_id: w.rendition(&source),
        segment_ids: vec![segment.header.id.clone()],
        model: "scripted".into(),
        prompt_hash: prompt.hash.clone(),
        usage: USAGE,
        attempts: 1,
        outcome: DerivationOutcome::Done,
        detail: None,
        items: vec![ExtractedItem::Claim {
            segment_id: segment.header.id.clone(),
            quote: "Fartyget nummer 0".into(),
            subject: "Fartyget".into(),
            predicate: "har".into(),
            value: "nummer 0".into(),
        }],
        rejected: vec![],
        created_at_ms: 1,
    };
    assert!(
        w.s.hold_job(&w.v, &held, true, vec![memex_types::Mutation::Put(Row::Derivation(derivation))])
            .unwrap()
    );
    assert!(w.changesets().is_empty());

    w.s.engine().clock().advance(Duration::from_secs(61));
    let agent = ScriptedAgent::answering(claims_each);
    w.work_ok(&agent);
    assert!(agent.requests().is_empty(), "the reply is on record");
    assert_eq!(w.changesets().len(), 1);
    assert_eq!(w.status(&source).state, ExtractionState::Done);
}

#[test]
fn a_cancelled_job_stops_at_its_next_step_and_writes_nothing_more() {
    let w = world_with(ExtractionLimits {
        batch_tokens: 50,
        ..ExtractionLimits::default()
    });
    let source = w.ingest(&document(2));
    let held = w.s.claim_job(&w.v, EXTRACT_JOB, "w").unwrap().unwrap();
    w.s.cancel_job(&w.admin, &w.v, &held.header.id).unwrap();
    let agent = ScriptedAgent::answering(claims_each);
    assert_eq!(w.s.run_extraction_job(&w.v, &held, &agent).unwrap_err(), ExtractJobError::Lost);
    assert!(agent.requests().is_empty());
    assert!(w.changesets().is_empty());
    assert_eq!(w.status(&source).state, ExtractionState::Cancelled);
}

#[test]
fn a_job_for_a_replaced_rendition_stops_without_a_call() {
    let w = world();
    let text = document(1);
    let source = w.ingest(&text);
    let old = w.rendition(&source);
    let stale = w.s.claim_job(&w.v, EXTRACT_JOB, "slow").unwrap().unwrap();
    let job = w.s.reprocess_source(&w.admin, &w.v, &source).unwrap();
    let held = w.s.claim_job(&w.v, RENDITION_JOB, "parser").unwrap().unwrap();
    assert_eq!(held.header.id, job.header.id);
    let rows =
        w.s.build_rendition(
            &w.v,
            &source,
            RenditionInput::Plain {
                text,
                parser: "plain-2".into(),
            },
        )
        .unwrap();
    assert!(w.s.complete_job(&w.v, &held, rows).unwrap());
    assert_ne!(w.rendition(&source), old);

    let agent = ScriptedAgent::answering(claims_each);
    let rows = w.s.run_extraction_job(&w.v, &stale, &agent).unwrap();
    assert!(agent.requests().is_empty());
    let [memex_types::Mutation::Put(Row::Extraction(row))] = &rows[..] else {
        panic!()
    };
    assert_eq!(row.stop, Some(ExtractionStop::Superseded));
    assert!(w.s.complete_job(&w.v, &stale, rows).unwrap());
    w.work_ok(&agent);
    assert_eq!(agent.requests().len(), 1);
    assert_eq!(w.status(&source).state, ExtractionState::Done);
}

#[test]
fn the_log_replays_extraction_with_no_llm() {
    let w = world();
    let source = w.ingest(&document(2));
    w.work_ok(&ScriptedAgent::answering(claims_each));
    w.approve_all();
    let snap = w.s.engine().store().snapshot(&w.v).unwrap();
    let original = memex_core::dump_store(&snap).unwrap();
    let copy = MemStore::new();
    let lease = copy.open_vault(&w.v).unwrap();
    memex_core::replay(memex_core::all_events(&snap).unwrap(), &copy, &lease).unwrap();
    let replayed = memex_core::dump_store(&copy.snapshot(&w.v).unwrap()).unwrap();
    assert_eq!(original, replayed);
    assert!(!w.derivations(&source).is_empty());
}
