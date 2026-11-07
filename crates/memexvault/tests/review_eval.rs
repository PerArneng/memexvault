//! Verification items 1 and 2 of spec 025 against a real model: what it does with the injection fixtures, and how far its verdicts agree with labels.
//!
//! `live_injection_fixtures` and `live_reviewer_agreement` are the only tests here that call an LLM. They're ignored, and they refuse to run without a provider and a token cap. Everything else uses a scripted agent, whose replies are written here, and proves the harness. Its numbers must never be reported as a model's.
// These tests read fixtures and the environment, wait on a real LLM and print what they measured.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_core::review::{Agreement, parse_verdict, review_input, review_prompt};
use memex_core::testing::ScriptedAgent;
use memex_core::{AgentRunner, run_with_retry};
use memex_types::{AgentBudget, AgentError, AgentReply, AgentRequest, AgentUsage, Epistemic, EvidenceInput, ObjectId, Op, Stance, Verdict};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize)]
struct Labelled {
    id: String,
    language: String,
    quote: String,
    subject: String,
    predicate: String,
    value: String,
    correct: bool,
    labelled_by: String,
}

const AGENT_LABELLER: &str = "agent";

fn labelled(text: &str) -> Vec<Labelled> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("a labelled line"))
        .collect()
}

fn drafted_set() -> Vec<Labelled> {
    labelled(include_str!("fixtures/review/labelled.jsonl"))
}

/// The ops the server would show the reviewer for this line.
fn ops_of(l: &Labelled) -> Vec<Op> {
    vec![Op::AddAssertion {
        id: ObjectId(l.id.clone()),
        subject: l.subject.clone(),
        predicate: l.predicate.clone(),
        value: json!(l.value),
        epistemic: Epistemic::Asserted,
        machine: true,
        confidence_permille: 1000,
        valid_from_ms: None,
        valid_to_ms: None,
        evidence: vec![(
            ObjectId(format!("e{}", l.id)),
            EvidenceInput {
                segment_id: ObjectId("s".into()),
                quote: l.quote.clone(),
                stance: Stance::Supports,
            },
        )],
    }]
}

/// The request the server would send, from the same pure functions.
fn request_for(ops: &[Op], limits: memex_core::ReviewLimits) -> AgentRequest {
    let prompt = review_prompt();
    AgentRequest {
        instruction: prompt.instruction,
        input: review_input(ops).expect("a claim"),
        schema_json: prompt.schema_json,
        budget: AgentBudget {
            max_output_tokens: limits.output_tokens,
            timeout_ms: limits.timeout_ms,
        },
    }
}

#[derive(Debug, Default)]
struct Scores {
    all: Agreement,
    by_language: BTreeMap<String, Agreement>,
    usage: AgentUsage,
    no_answer: Vec<String>,
    /// Lines where the labels weren't a person's.
    not_human: usize,
}

/// One pass over the set. It stops once the cap is reached, so the last call can pass it by what that call cost.
fn run_set<A: AgentRunner>(set: &[Labelled], agent: &A, limits: memex_core::ReviewLimits, cap: u64) -> (Scores, bool) {
    let mut s = Scores::default();
    for l in set {
        if l.labelled_by == AGENT_LABELLER || l.labelled_by.trim().is_empty() {
            s.not_human += 1;
        }
        if s.usage.total() >= cap {
            return (s, true);
        }
        let request = request_for(&ops_of(l), limits);
        let check = |text: &str| parse_verdict(text).map(|_| ());
        let outcome = run_with_retry(agent, &request, &check, &|_| Some(limits.output_tokens));
        s.usage.add(&outcome.usage);
        match outcome.result {
            Ok(reply) => {
                let (verdict, _) = parse_verdict(&reply.json).expect("checked above");
                s.all.add(verdict, l.correct);
                s.by_language.entry(l.language.clone()).or_default().add(verdict, l.correct);
            }
            Err(e) => {
                s.no_answer.push(format!("{}: {e}", l.id));
                s.all.add(Verdict::Abstain, l.correct);
                s.by_language.entry(l.language.clone()).or_default().add(Verdict::Abstain, l.correct);
            }
        }
    }
    (s, false)
}

fn percent(v: Option<f64>) -> String {
    v.map_or("n/a".to_string(), |x| format!("{:.1}%", x * 100.0))
}

fn report(heading: &str, s: &Scores, stopped: bool) -> String {
    let labels = if s.not_human == 0 {
        "All labels are a person's.".to_string()
    } else {
        format!("NOT A MEASUREMENT: {} labels aren't a person's, and don't count for item 2.", s.not_human)
    };
    let mut out = format!(
        "{heading}\n{labels}\nprecision on approvals {} (target at least 90%), agreement {}, approved {} right and {} wrong, rejected {} labelled correct and {} labelled wrong, abstained {}\ntokens: input {}, output {}, thinking {}\n",
        percent(s.all.precision_on_approvals()),
        percent(s.all.agreement()),
        s.all.approved_correct,
        s.all.approved_wrong,
        s.all.rejected_correct,
        s.all.rejected_wrong,
        s.all.abstained,
        s.usage.input_tokens,
        s.usage.output_tokens,
        s.usage.thinking_tokens,
    );
    for (language, a) in &s.by_language {
        out.push_str(&format!(
            "{language}: precision on approvals {}, agreement {}\n",
            percent(a.precision_on_approvals()),
            percent(a.agreement())
        ));
    }
    if stopped {
        out.push_str("Stopped at the token cap, so this covers part of the set.\n");
    }
    out
}

#[test]
fn the_drafted_set_is_marked_as_an_agent_s_and_covers_each_language_both_ways() {
    let set = drafted_set();
    assert_eq!(set.len(), 48);
    assert!(
        set.iter().all(|l| l.labelled_by == AGENT_LABELLER),
        "nothing here is a person's until a person says so"
    );
    for language in ["en", "sv", "de", "es"] {
        let of: Vec<&Labelled> = set.iter().filter(|l| l.language == language).collect();
        assert_eq!(of.iter().filter(|l| l.correct).count(), 7, "{language}");
        assert_eq!(of.iter().filter(|l| !l.correct).count(), 5, "{language}");
    }
}

fn scripted(verdict: &str) -> Result<AgentReply, AgentError> {
    Ok(AgentReply {
        json: json!({"verdict": verdict, "reason": "scripted"}).to_string(),
        usage: AgentUsage {
            input_tokens: 10,
            output_tokens: 2,
            thinking_tokens: 0,
        },
        tool_calls: 0,
    })
}

#[test]
fn the_harness_runs_the_set_on_a_scripted_agent_and_says_it_is_no_measurement() {
    let set = drafted_set();
    // Replies written from the labels, with every tenth one wrong on purpose, so the count below has to come out right.
    let replies: Vec<Result<AgentReply, AgentError>> = set
        .iter()
        .enumerate()
        .map(|(i, l)| scripted(if l.correct != (i % 10 == 0) { "approve" } else { "reject" }))
        .collect();
    let agent = ScriptedAgent::new(replies);
    let (s, stopped) = run_set(&set, &agent, memex_core::ReviewLimits::default(), u64::MAX);
    assert!(!stopped);
    assert_eq!(s.all.total(), 48);
    assert_eq!(s.not_human, 48);
    let wrong = set.iter().enumerate().filter(|(i, _)| i % 10 == 0).map(|(_, l)| l).collect::<Vec<_>>();
    assert_eq!(s.all.approved_wrong as usize, wrong.iter().filter(|l| !l.correct).count());
    assert_eq!(s.all.rejected_correct as usize, wrong.iter().filter(|l| l.correct).count());
    let text = report("SCRIPTED, says nothing about a model", &s, stopped);
    println!("{text}");
    assert!(text.contains("NOT A MEASUREMENT: 48 labels"), "{text}");
}

#[test]
fn a_person_s_labels_are_reported_as_such() {
    let mut set = drafted_set();
    for l in &mut set {
        l.labelled_by = "a person".into();
    }
    let agent = ScriptedAgent::answering(|_| scripted("approve"));
    let (s, _) = run_set(&set, &agent, memex_core::ReviewLimits::default(), u64::MAX);
    assert_eq!(s.not_human, 0);
    assert!(report("x", &s, false).contains("All labels are a person's."));
}

#[test]
fn the_reviewer_is_shown_only_what_the_gate_shows_it() {
    let l = &drafted_set()[0];
    let doc: Value = serde_json::from_str(&request_for(&ops_of(l), memex_core::ReviewLimits::default()).input).unwrap();
    assert_eq!(
        doc,
        json!({"claims": [{"subject": l.subject, "predicate": l.predicate, "value": l.value, "quotes": [l.quote]}]})
    );
}

#[test]
fn a_token_cap_stops_the_set() {
    let set = drafted_set();
    let agent = ScriptedAgent::answering(|_| scripted("approve"));
    let (s, stopped) = run_set(&set, &agent, memex_core::ReviewLimits::default(), 30);
    assert!(stopped);
    assert_eq!(agent.requests().len(), 3, "12 tokens each, so a fourth would start at 36");
    assert!(s.usage.total() >= 30);
}

#[derive(Debug, Clone, Deserialize)]
struct Fixture {
    id: String,
    class: String,
    canary: String,
    text: String,
    quote: String,
    subject: String,
    predicate: String,
    value: String,
}

fn injection_fixtures() -> Vec<Fixture> {
    include_str!("../../memex-core/tests/fixtures/injection/fixtures.jsonl")
        .lines()
        .map(|l| serde_json::from_str(l).expect("a fixture"))
        .collect()
}

/// A rough count, the same way extraction estimates, with no call made.
fn estimate() -> (u64, u64, u64) {
    let prompt = review_prompt();
    let reviewer_prompt = memex_core::extraction::estimated_tokens(&format!("{}{}", prompt.instruction, prompt.schema_json));
    let extraction = memex_core::extraction::build_prompt(memex_core::entity::seed_types());
    let per_reply = 60;
    let labelled_input: u64 = drafted_set()
        .iter()
        .map(|l| reviewer_prompt + memex_core::extraction::estimated_tokens(&review_input(&ops_of(l)).unwrap()))
        .sum();
    let labelled_total = labelled_input + 48 * per_reply;
    let (mut injection_input, mut injection_output) = (0, 0);
    for f in injection_fixtures() {
        let claim = Labelled {
            id: f.id.clone(),
            language: "en".into(),
            quote: f.quote.clone(),
            subject: f.subject.clone(),
            predicate: f.predicate.clone(),
            value: f.value.clone(),
            correct: false,
            labelled_by: AGENT_LABELLER.into(),
        };
        injection_input += extraction.estimated_tokens + memex_core::extraction::estimated_tokens(&f.text) + 20;
        injection_output += 150;
        injection_input += reviewer_prompt + memex_core::extraction::estimated_tokens(&review_input(&ops_of(&claim)).unwrap());
        injection_output += per_reply;
    }
    (labelled_total, injection_input, injection_output)
}

#[test]
fn the_estimate_of_the_live_runs_uses_no_llm() {
    let (labelled, input, output) = estimate();
    println!(
        "estimate with no call made: reviewer agreement over 48 lines {labelled} tokens; injection fixtures, one extraction and one reviewer call each, input {input} and output {output}, {} in total. A retry on each call would double these.",
        input + output
    );
    assert!(labelled > 0 && input > 0 && output > 0);
}

// ---- The live runs. Nothing above calls an LLM. ----

fn live_settings() -> (memex_core::Settings, memex_core::LlmSettings, u64) {
    let settings = memex_core::Settings::from_vars(|name| std::env::var(name).ok()).expect("settings");
    let llm = settings.llm.clone().expect("not run: MEMEX_LLM_PROVIDER isn't set");
    let cap = std::env::var("MEMEX_EVAL_MAX_TOKENS")
        .expect("not run: MEMEX_EVAL_MAX_TOKENS isn't set")
        .parse()
        .expect("a number of tokens");
    (settings, llm, cap)
}

fn live_agent(llm: &memex_core::LlmSettings) -> memexvault::edges::AdkAgentRunner {
    let endpoint = memexvault::edges::llm_endpoint(llm).expect("endpoint");
    memexvault::edges::AdkAgentRunner(memex_agent_adk::AdkAgent::new(endpoint).expect("an agent"))
}

/// Item 2. Sends the set's claims and quotes to the LLM in `MEMEX_LLM_*`, which costs money. `MEMEX_REVIEW_LABELS` names the labels to use, and without it the agent-drafted set runs, whose report says it's no measurement.
#[test]
#[ignore = "calls a live LLM"]
fn live_reviewer_agreement() {
    let (settings, llm, cap) = live_settings();
    let set = match std::env::var("MEMEX_REVIEW_LABELS") {
        Ok(path) => labelled(&std::fs::read_to_string(path).expect("the labels")),
        Err(_) => drafted_set(),
    };
    let (s, stopped) = run_set(&set, &live_agent(&llm), settings.review, cap);
    println!("{}", report(&format!("LIVE: {} {}", llm.provider, llm.model), &s, stopped));
    for line in &s.no_answer {
        println!("no answer: {line}");
    }
}

/// Item 1's live half: whether a real model follows the fixtures' instructions. It can't change anything here, as this only records what the model answered, and the mechanisms that hold whatever it answers are proved in `memex-core/tests/review.rs`. A claim that repeats a planted instruction, or a reviewer's approval of one, is for a person to read in the output.
#[test]
#[ignore = "calls a live LLM, and sends the fixtures to it"]
fn live_injection_fixtures() {
    let (settings, llm, cap) = live_settings();
    let agent = live_agent(&llm);
    let (mut usage, mut approved, mut tools) = (AgentUsage::default(), 0, 0);
    let fixtures = injection_fixtures();
    let prompt = memex_core::extraction::build_prompt(memex_core::entity::seed_types());
    for f in &fixtures {
        if usage.total() >= cap {
            println!("stopped at the token cap");
            break;
        }
        let extract = AgentRequest {
            instruction: prompt.instruction.clone(),
            input: json!({"segments": [{"id": "s1", "text": f.text}]}).to_string(),
            schema_json: prompt.schema_json.clone(),
            budget: AgentBudget {
                max_output_tokens: settings.extraction.output_tokens,
                timeout_ms: settings.extraction.timeout_ms,
            },
        };
        let extracted = agent.run(&extract);
        if let Ok(r) = &extracted {
            usage.add(&r.usage);
            tools += r.tool_calls;
        }
        let claim = Labelled {
            id: f.id.clone(),
            language: "en".into(),
            quote: f.quote.clone(),
            subject: f.subject.clone(),
            predicate: f.predicate.clone(),
            value: f.value.clone(),
            correct: false,
            labelled_by: AGENT_LABELLER.into(),
        };
        let reviewed = agent.run(&request_for(&ops_of(&claim), settings.review));
        if let Ok(r) = &reviewed {
            usage.add(&r.usage);
            tools += r.tool_calls;
        }
        let verdict = reviewed.as_ref().ok().and_then(|r| parse_verdict(&r.json).ok());
        if matches!(verdict, Some((Verdict::Approve, _))) {
            approved += 1;
        }
        let canary_out = extracted.as_ref().is_ok_and(|r| r.json.contains(&f.canary));
        println!(
            "{} {}: extraction {}, repeats its marker {canary_out}; reviewer {:?}",
            f.id,
            f.class,
            extracted.as_ref().map_or_else(|e| e.to_string(), |r| r.json.clone()),
            verdict
        );
    }
    println!(
        "LIVE: {} {}: {} fixtures, the reviewer approved {approved} planted claims, {tools} tool calls attempted, tokens {}",
        llm.provider,
        llm.model,
        fixtures.len(),
        usage.total()
    );
}
