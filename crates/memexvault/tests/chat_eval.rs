//! Spec 049's item 1: on 20 golden-set questions, every factual sentence of a reply cites an anchor that resolves, or the reply says it found nothing.
//!
//! `the_scorer_tells_good_replies_from_bad_on_a_scripted_model` proves this harness, and says nothing about any model. `live_chat_cites_on_twenty_golden_questions` is the measurement: it's ignored, it costs money, and it sends the questions and the corpus's public Wikipedia passages to the provider. Only the user decides when it runs.
// The live test reads the environment and a corpus, and prints what it measured.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_agent_adk::{AdkAgent, LlmEndpoint};
use memex_core::chat::{CitationReport, check_citations};
use memex_core::testing::{ChatCall, ManualClock, MemBlobStore, MemRegistry, MemStore, ScriptedChat, SeededRandom};
use memex_core::{AgentRunner, Caller, ChatInput, ChatLimits, ChatSettingsUpdate, RENDITION_JOB, RenditionInput, Server, ServerEdges, Snapshot, Store, UploadLimits, UploadRequest};
use memex_types::{AgUi, ObjectId, Row, RowKey, Table, VaultId};
use memexvault::edges::AdkAgentRunner;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

const QUESTIONS: usize = 20;

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

#[derive(Deserialize, Clone)]
struct Golden {
    id: String,
    question: String,
    line: Option<usize>,
    answer: Option<String>,
}

/// The first twenty with a passage behind them, so a run on the same corpus asks the same questions.
fn golden_twenty() -> Vec<Golden> {
    let all = include_str!("fixtures/golden/golden.jsonl");
    let questions: Vec<Golden> = all.lines().map(|l| serde_json::from_str(l).expect("a golden line")).collect();
    questions.into_iter().filter(|g| g.line.is_some() && g.answer.is_some()).take(QUESTIONS).collect()
}

struct Rig {
    s: Arc<Server<Fakes>>,
    admin: Caller,
    v: VaultId,
}

fn rig(day_tokens: Option<u64>) -> Rig {
    let s = Arc::new(Server::<Fakes>::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(1_700_000_000_000),
        SeededRandom::new(49),
        memex_core::testing::mem_search(),
    ));
    s.set_chat(Some(ChatLimits {
        max_runs: 1,
        ..ChatLimits::default()
    }));
    let admin = s.authenticate(&s.bootstrap("root").unwrap().token).unwrap();
    let v = s.create_vault(&admin, "eval").unwrap().id;
    s.set_chat_settings(
        &admin,
        &v,
        ChatSettingsUpdate {
            enabled: true,
            thread_token_budget: None,
            day_token_budget: day_tokens,
        },
    )
    .unwrap();
    Rig { s, admin, v }
}

impl Rig {
    fn document(&self, name: &str, text: &str) -> ObjectId {
        let req = UploadRequest {
            name: name.into(),
            trust_tier: "unverified".into(),
            metadata: json!({}),
            replaces: None,
        };
        let source = self.s.upload(&self.admin, &self.v, req, UploadLimits::default(), &mut text.as_bytes()).unwrap().source.header.id;
        let job = self.s.claim_job(&self.v, RENDITION_JOB, "w").unwrap().unwrap();
        let rows = self
            .s
            .build_rendition(
                &self.v,
                &source,
                RenditionInput::Plain {
                    text: text.into(),
                    parser: "fixture-1".into(),
                },
            )
            .unwrap();
        assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
        source
    }

    fn settle(&self) {
        for _ in 0..4000 {
            self.s.engine().clock().advance(Duration::from_secs(60));
            while self.s.project_lexical(&self.v).unwrap() == memex_core::StepOutcome::Progress {}
            while self.s.project_vector(&self.v).unwrap() == memex_core::StepOutcome::Progress {}
            let Some(job) = self.s.claim_job(&self.v, memex_core::EMBED_JOB, "w").unwrap() else { break };
            self.s.run_embed_job(&self.v, &job).unwrap();
            self.s.complete_job(&self.v, &job, vec![]).unwrap();
        }
        self.s.engine().clock().advance(Duration::from_secs(60));
        while self.s.project_lexical(&self.v).unwrap() == memex_core::StepOutcome::Progress {}
        while self.s.project_vector(&self.v).unwrap() == memex_core::StepOutcome::Progress {}
    }

    /// An id cited resolves if it's a segment or a claim in this vault. A claim's own anchor comes with it.
    fn resolves(&self, id: &str) -> bool {
        let snap = self.s.engine().store().snapshot(&self.v).unwrap();
        [Table::Segment, Table::Assertion].into_iter().any(|table| {
            matches!(
                snap.get(&RowKey { table, id: ObjectId(id.into()) }),
                Ok(Some(Row::Segment(_) | Row::Assertion(_)))
            )
        })
    }

    /// A whole run, and the reply it ended with, or how it ended if not with one.
    fn ask<A: AgentRunner>(&self, agent: &A, n: usize, question: &str) -> Result<String, String> {
        let thread = format!("q{n}");
        let prepared = self
            .s
            .prepare_chat(
                &self.admin,
                &self.v,
                ChatInput {
                    thread_id: thread.clone(),
                    run_id: format!("run-{n}"),
                    message: question.into(),
                },
                memex_core::new_chat_cancel(),
            )
            .map_err(|e| e.to_string())?;
        let frames = Arc::new(std::sync::Mutex::new(vec![]));
        let log = frames.clone();
        self.s.run_chat(prepared, agent, memex_api::chat_tool::<Fakes>, move |e: AgUi| {
            log.lock().unwrap().push(e);
            true
        });
        let frames = frames.lock().unwrap();
        match frames.last() {
            Some(AgUi::RunFinished { result: None, .. }) => {}
            other => return Err(format!("{other:?}")),
        }
        let view = self.s.get_chat_thread(&self.admin, &self.v, &thread, None, 200).map_err(|e| e.to_string())?;
        let replies: Vec<String> = view.messages.into_iter().filter(|m| m.message.role == memex_types::ChatRole::Assistant).map(|m| m.message.text).collect();
        replies.last().cloned().ok_or_else(|| "no reply".to_string())
    }
}

fn score(r: &Rig, reply: &str) -> CitationReport {
    check_citations(reply, &|id| r.resolves(id))
}

#[derive(Default)]
struct Tally {
    runs: usize,
    passed: usize,
    found_nothing: usize,
    failed_runs: usize,
    uncited_sentences: usize,
    unresolved: usize,
    sentences: usize,
}

fn run_set<A: AgentRunner>(r: &Rig, agent: &A, questions: &[Golden], out: &mut dyn FnMut(String)) -> Tally {
    let mut t = Tally::default();
    for (n, g) in questions.iter().enumerate() {
        t.runs += 1;
        match r.ask(agent, n, &g.question) {
            Err(why) => {
                t.failed_runs += 1;
                out(json!({"id": g.id, "failed": why}).to_string());
            }
            Ok(reply) => {
                let report = score(r, &reply);
                t.sentences += report.sentences;
                t.uncited_sentences += report.uncited.len();
                t.unresolved += report.unresolved.len();
                t.found_nothing += usize::from(report.found_nothing);
                t.passed += usize::from(report.passed());
                out(json!({"id": g.id, "question": g.question, "reply": reply, "sentences": report.sentences, "uncited": report.uncited, "unresolved": report.unresolved, "found_nothing": report.found_nothing, "passed": report.passed()}).to_string());
            }
        }
    }
    t
}

#[test]
fn twenty_golden_questions_have_a_passage_to_be_answered_from() {
    let set = golden_twenty();
    assert_eq!(set.len(), QUESTIONS);
    assert!(set.iter().all(|g| g.line.is_some()));
}

/// The scripted replies are what the test wrote, so this proves the harness tells them apart, and nothing else.
#[test]
fn the_scorer_tells_good_replies_from_bad_on_a_scripted_model() {
    let r = rig(None);
    r.document("a.txt", "The Zorblat Bridge is 412 metres long.");
    r.settle();
    let snap = r.s.engine().store().snapshot(&r.v).unwrap();
    let segment = snap
        .rows(Table::Segment, None)
        .unwrap()
        .into_iter()
        .find_map(|x| if let Row::Segment(s) = x { Some(s.header.id.0) } else { None })
        .unwrap();
    drop(snap);
    let search = ChatCall::calls("search", json!({"q": "Zorblat Bridge"}));
    let cases = [
        (format!("It is 412 metres long [cite:{segment}]."), true),
        (memex_core::chat::FOUND_NOTHING.to_string(), true),
        ("It is 412 metres long.".to_string(), false),
        ("It is 412 metres long [cite:no-such-id].".to_string(), false),
        (format!("It is 412 metres long [cite:{segment}]. And it's lovely."), false),
    ];
    for (n, (reply, passes)) in cases.into_iter().enumerate() {
        let agent = ScriptedChat::new(vec![vec![search.clone(), ChatCall::says(&reply)]]);
        let got = r.ask(&agent, n, "How long is it?").unwrap();
        assert_eq!(got, reply);
        assert_eq!(score(&r, &got).passed(), passes, "{reply}");
        assert!(agent.tool_answers()[0]["data"]["items"].as_array().is_some_and(|i| !i.is_empty()), "the search finds it");
    }
    assert!(!r.resolves("no-such-id"));
    assert!(r.resolves(&segment));
    // And the whole set's tally counts a failed run as a failure.
    let agent = ScriptedChat::new(vec![vec![ChatCall::says("no citation")], vec![ChatCall::new(vec![])]]);
    let two = golden_twenty().into_iter().take(2).collect::<Vec<_>>();
    let mut sink = |_: String| {};
    let t = run_set(&r, &agent, &two, &mut sink);
    assert_eq!((t.runs, t.passed, t.failed_runs, t.uncited_sentences), (2, 0, 1, 1));
}

/// Item 1, live. Needs `MEMEX_EVAL_APPROVED` as a deliberate yes, on top of `--ignored`.
#[test]
#[ignore = "calls an LLM, costs money and sends the corpus's passages to the provider, so run it only when asked"]
fn live_chat_cites_on_twenty_golden_questions() {
    let env = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("not run: {name} isn't set"));
    assert!(std::env::var("MEMEX_EVAL_APPROVED").is_ok(), "not run: set MEMEX_EVAL_APPROVED once this run is approved");
    let passages = std::fs::read_to_string(format!("{}/passages.txt", env("MEMEX_TEST_CORPUS"))).expect("the corpus");
    let passages: Vec<&str> = passages.lines().collect();
    let endpoint = match env("MEMEX_LLM_PROVIDER").as_str() {
        "gemini" => LlmEndpoint::Gemini {
            api_key: env("MEMEX_LLM_API_KEY"),
            model: env("MEMEX_LLM_MODEL"),
        },
        "openai" => LlmEndpoint::OpenAi {
            base_url: env("MEMEX_LLM_BASE_URL"),
            api_key: std::env::var("MEMEX_LLM_API_KEY").ok(),
            model: env("MEMEX_LLM_MODEL"),
        },
        other => panic!("unknown provider {other}"),
    };
    let max_tokens: u64 = env("MEMEX_EVAL_MAX_TOKENS").parse().expect("a number");
    let distractors: usize = std::env::var("MEMEX_CHAT_DISTRACTORS").ok().and_then(|v| v.parse().ok()).unwrap_or(200);
    let r = rig(Some(max_tokens));
    let questions = golden_twenty();
    for g in &questions {
        r.document(&format!("{}.txt", g.id), passages[g.line.expect("filtered")]);
    }
    for i in 0..distractors {
        r.document(&format!("other-{i}.txt"), passages[i * 97 % passages.len()]);
    }
    r.settle();
    let agent = AdkAgentRunner(AdkAgent::new(endpoint).expect("the agent"));
    let path = std::env::var("MEMEX_EVAL_OUT").ok();
    let mut lines = vec![];
    let mut sink = |line: String| {
        println!("{line}");
        lines.push(line);
    };
    let t = run_set(&r, &agent, &questions, &mut sink);
    if let Some(path) = path {
        std::fs::write(path, lines.join("\n")).expect("the output file");
    }
    let spent = r.s.chat_day_spend(&r.admin, &r.v).unwrap().0;
    println!(
        "{{\"model\":{:?},\"questions\":{},\"passed\":{},\"found_nothing\":{},\"failed_runs\":{},\"sentences\":{},\"uncited\":{},\"unresolved\":{},\"tokens\":{spent}}}",
        agent.model(),
        t.runs,
        t.passed,
        t.found_nothing,
        t.failed_runs,
        t.sentences,
        t.uncited_sentences,
        t.unresolved
    );
    assert_eq!(t.passed, t.runs, "{} of {} passed", t.passed, t.runs);
}
