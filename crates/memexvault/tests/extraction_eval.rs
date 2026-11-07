//! Verification item 1 of spec 024: a labelled set of public documents, the code that runs it through the real pipeline, and the code that scores what came out.
//!
//! `live_labelled_set` is the only test here that calls an LLM. It's ignored, and it refuses to run without a provider, a list of approved documents and a token cap. Every other test uses a scripted agent, whose replies are written from the labels, so it proves the scoring and says nothing about a model. Its numbers must never be reported as precision.
// These tests read fixtures and the environment, wait on a real LLM and print what they measured.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_core::testing::{FakeRasterizer, FakeReranker, ManualClock, MemBlobStore, MemRegistry, MemStore, ScriptedAgent, SeededRandom};
use memex_core::{
    AgentRunner, EXTRACT_JOB, ExtractJobError, ExtractionLimits, ExtractionState, RENDITION_JOB, RenditionInput, Server, ServerEdges, UploadLimits,
    UploadRequest,
};
use memex_types::{AgentError, AgentReply, AgentRequest, AgentUsage, ChangeSetRow, Op, VaultId, entity_of_subject};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

struct Fakes;

impl ServerEdges for Fakes {
    type Registry = MemRegistry;
    type Store = MemStore;
    type Blobs = MemBlobStore<ManualClock>;
    type Clock = ManualClock;
    type Seed = SeededRandom;
    type Rasterizer = FakeRasterizer;
    type Lexical = memex_core::testing::MemLexical;
    type Vector = memex_core::testing::MemVector;
    type Embedder = memex_core::testing::FakeEmbedder;
    type Reranker = FakeReranker;
    type Caches = memex_core::NoCaches;
}

#[derive(Debug, Clone, Deserialize)]
struct Document {
    id: String,
    language: String,
    title: String,
    source_url: String,
    license: String,
    attribution: String,
    text: String,
}

/// A value and the other ways of writing it that count as the same.
type Aliases = Vec<String>;

#[derive(Debug, Clone, Deserialize)]
struct GoldEntity {
    #[serde(rename = "type")]
    type_name: String,
    fields: BTreeMap<String, Aliases>,
}

#[derive(Debug, Clone, Deserialize)]
struct GoldClaim {
    subject: Aliases,
    value: Aliases,
}

#[derive(Debug, Clone, Deserialize)]
struct Label {
    id: String,
    entities: Vec<GoldEntity>,
    claims: Vec<GoldClaim>,
}

/// A person's verdict on a prediction the labels don't cover. Precision without these counts every such prediction as wrong.
#[derive(Debug, Clone, Deserialize)]
struct Adjudication {
    doc: String,
    prediction: String,
    correct: bool,
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/extraction")
}

fn jsonl<T: serde::de::DeserializeOwned>(name: &str) -> Vec<T> {
    let text = std::fs::read_to_string(fixtures().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{name}: {e}: {l}")))
        .collect()
}

fn documents() -> Vec<Document> {
    jsonl("documents.jsonl")
}

fn labels() -> Vec<Label> {
    jsonl("labels.jsonl")
}

// ---- Scoring. Pure, and tested on hand-made predictions below. ----

/// Fields of running text. Whether a summary is right isn't a matter of matching strings, so these are neither credited nor charged.
const UNSCORED_FIELDS: [&str; 2] = ["description", "rationale"];
/// The field that says which thing an entity is.
const IDENTITY_FIELDS: [&str; 2] = ["name", "title"];

fn fold(text: &str) -> String {
    memex_core::entity::fold_key(text)
}

/// Same after folding, or one holds the other. The shorter side needs four characters when it's the prediction, so `a` doesn't match everything.
fn same_value(predicted: &str, alias: &str) -> bool {
    let (p, a) = (fold(predicted), fold(alias));
    if p.is_empty() || a.is_empty() {
        return false;
    }
    p == a || p.contains(&a) || (p.chars().count() >= 4 && a.contains(&p))
}

fn any_alias(predicted: &str, aliases: &[String]) -> bool {
    aliases.iter().any(|a| same_value(predicted, a))
}

/// One thing the pipeline proposed. Each field of an entity is one, as each is a claim of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Prediction {
    doc: String,
    language: String,
    /// The entity type, or `claim`.
    kind: String,
    /// The entity's identity value, or the claim's subject.
    subject: String,
    /// The field, or the claim's predicate.
    field: String,
    value: String,
    quote: String,
}

impl Prediction {
    fn key(&self) -> String {
        fold(&format!("{} | {} | {} | {}", self.kind, self.subject, self.field, self.value))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
struct Tally {
    correct: u64,
    wrong: u64,
    /// Wrong as far as the labels know, and a person hasn't said otherwise.
    unlabelled: u64,
    unscored: u64,
    gold_found: u64,
    gold_total: u64,
}

impl Tally {
    fn precision(&self) -> Option<f64> {
        let judged = self.correct + self.wrong + self.unlabelled;
        (judged > 0).then(|| self.correct as f64 / judged as f64)
    }

    fn recall(&self) -> Option<f64> {
        (self.gold_total > 0).then(|| self.gold_found as f64 / self.gold_total as f64)
    }

    fn add(&mut self, other: &Tally) {
        self.correct += other.correct;
        self.wrong += other.wrong;
        self.unlabelled += other.unlabelled;
        self.unscored += other.unscored;
        self.gold_found += other.gold_found;
        self.gold_total += other.gold_total;
    }
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
struct Scores {
    /// By language and kind.
    cells: BTreeMap<(String, String), Tally>,
    /// Predictions a person should look at, by document.
    to_adjudicate: Vec<Prediction>,
    /// Labelled values nothing found, for reading a recall number.
    missed: Vec<String>,
}

impl Scores {
    fn cell(&mut self, language: &str, kind: &str) -> &mut Tally {
        self.cells.entry((language.to_string(), kind.to_string())).or_default()
    }

    fn total(&self) -> Tally {
        let mut t = Tally::default();
        self.cells.values().for_each(|c| t.add(c));
        t
    }

    fn by(&self, pick: impl Fn(&(String, String)) -> String) -> BTreeMap<String, Tally> {
        let mut out: BTreeMap<String, Tally> = BTreeMap::new();
        for (k, c) in &self.cells {
            out.entry(pick(k)).or_default().add(c);
        }
        out
    }
}

fn identity_of(entity: &GoldEntity) -> Option<&Aliases> {
    IDENTITY_FIELDS.iter().find_map(|f| entity.fields.get(*f))
}

/// How closely a prediction fits a list of spellings: the same is better than holding one, and the longer the spelling held, the better. Zero means no fit.
fn fit(predicted: &str, aliases: &[String]) -> usize {
    let p = fold(predicted);
    aliases
        .iter()
        .filter(|a| same_value(predicted, a))
        .map(|a| {
            let a = fold(a);
            if a == p { usize::MAX } else { a.chars().count().min(p.chars().count()) }
        })
        .max()
        .unwrap_or(0)
}

/// Scores predictions against labels. A prediction is correct when the labels hold it, wrong when the labels contradict it on a field of an entity they know, and otherwise unlabelled, to be adjudicated. A fact stated as a free claim counts as correct if an entity's field holds it, and earns no recall.
fn score(documents: &[Document], labels: &[Label], predictions: &[Prediction], adjudications: &[Adjudication]) -> Scores {
    let mut scores = Scores::default();
    let verdicts: HashMap<(String, String), bool> = adjudications.iter().map(|a| ((a.doc.clone(), fold(&a.prediction)), a.correct)).collect();
    for doc in documents {
        let Some(label) = labels.iter().find(|l| l.id == doc.id) else { continue };
        let mut found: BTreeSet<(String, String, String)> = BTreeSet::new();
        for p in predictions.iter().filter(|p| p.doc == doc.id) {
            if UNSCORED_FIELDS.contains(&p.field.as_str()) {
                scores.cell(&doc.language, &p.kind).unscored += 1;
                continue;
            }
            let verdict = if p.kind == "claim" {
                let joined = format!("{} {}", p.field, p.value);
                // Spellings can overlap, so a repeat goes to a labelled claim not yet found.
                let matching: Vec<usize> = (0..label.claims.len())
                    .filter(|i| {
                        let c = &label.claims[*i];
                        any_alias(&p.subject, &c.subject) && (any_alias(&p.value, &c.value) || any_alias(&joined, &c.value))
                    })
                    .collect();
                let free = matching
                    .iter()
                    .find(|i| !found.contains(&("claim".to_string(), i.to_string(), String::new())))
                    .or(matching.first())
                    .copied();
                if let Some(i) = free {
                    found.insert(("claim".into(), i.to_string(), String::new()));
                    Some(true)
                } else {
                    // The same fact as an entity's field.
                    let held = label.entities.iter().any(|e| {
                        identity_of(e).is_some_and(|id| any_alias(&p.subject, id))
                            && e.fields.values().any(|aliases| any_alias(&p.value, aliases) || any_alias(&joined, aliases))
                    });
                    held.then_some(true)
                }
            } else {
                // "Zara Home" holds "Zara", so the closest identity wins.
                let known = label
                    .entities
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.type_name == p.kind)
                    .filter_map(|(i, e)| identity_of(e).map(|id| (fit(&p.subject, id), i, e)))
                    .filter(|(closeness, _, _)| *closeness > 0)
                    .max_by_key(|(closeness, i, _)| (*closeness, std::cmp::Reverse(*i)))
                    .map(|(_, i, e)| (i, e));
                match known {
                    Some((i, entity)) => match entity.fields.get(&p.field) {
                        Some(aliases) if any_alias(&p.value, aliases) => {
                            found.insert((p.kind.clone(), i.to_string(), p.field.clone()));
                            Some(true)
                        }
                        // The label states this field and says something else.
                        Some(_) => Some(false),
                        None => None,
                    },
                    None => None,
                }
            };
            let cell_verdict = verdict.or_else(|| verdicts.get(&(doc.id.clone(), p.key())).copied());
            match (verdict, cell_verdict) {
                (_, Some(true)) => scores.cell(&doc.language, &p.kind).correct += 1,
                (_, Some(false)) => scores.cell(&doc.language, &p.kind).wrong += 1,
                _ => {
                    scores.cell(&doc.language, &p.kind).unlabelled += 1;
                    scores.to_adjudicate.push(p.clone());
                }
            }
        }
        for (i, e) in label.entities.iter().enumerate() {
            for field in e.fields.keys() {
                let cell = scores.cell(&doc.language, &e.type_name);
                cell.gold_total += 1;
                if found.contains(&(e.type_name.clone(), i.to_string(), field.clone())) {
                    cell.gold_found += 1;
                } else {
                    scores.missed.push(format!("{}: {} {field} {:?}", doc.id, e.type_name, e.fields[field]));
                }
            }
        }
        for (i, c) in label.claims.iter().enumerate() {
            let cell = scores.cell(&doc.language, "claim");
            cell.gold_total += 1;
            if found.contains(&("claim".to_string(), i.to_string(), String::new())) {
                cell.gold_found += 1;
            } else {
                scores.missed.push(format!("{}: claim {:?} {:?}", doc.id, c.subject, c.value));
            }
        }
    }
    scores
}

fn percent(v: Option<f64>) -> String {
    v.map_or("-".into(), |x| format!("{:.1}%", x * 100.0))
}

fn table(title: &str, rows: &BTreeMap<String, Tally>) -> String {
    let mut out = format!(
        "{title:<14} {:>7} {:>6} {:>10} {:>9} {:>10} {:>10} {:>8}\n",
        "correct", "wrong", "unlabelled", "unscored", "gold found", "precision", "recall"
    );
    for (name, t) in rows {
        out.push_str(&format!(
            "{name:<14} {:>7} {:>6} {:>10} {:>9} {:>4}/{:<5} {:>10} {:>8}\n",
            t.correct,
            t.wrong,
            t.unlabelled,
            t.unscored,
            t.gold_found,
            t.gold_total,
            percent(t.precision()),
            percent(t.recall())
        ));
    }
    out
}

fn report(run_kind: &str, scores: &Scores, usage: &AgentUsage, run: &RunSummary) -> String {
    let mut out = format!("source of these predictions: {run_kind}\n");
    out.push_str(&format!(
        "documents {} done, {} partial, {} not finished, {} calls, proposals refused {}\n",
        run.done, run.partial, run.unfinished, run.calls, run.rejected
    ));
    out.push_str(&format!(
        "tokens: input {}, output {}, thinking {}, total {}\n\n",
        usage.input_tokens,
        usage.output_tokens,
        usage.thinking_tokens,
        usage.total()
    ));
    out.push_str(&table("language", &scores.by(|k| k.0.clone())));
    out.push('\n');
    out.push_str(&table("type", &scores.by(|k| k.1.clone())));
    out.push('\n');
    let total = BTreeMap::from([("all".to_string(), scores.total())]);
    out.push_str(&table("total", &total));
    out.push_str(&format!("\nawaiting a person's verdict: {}\n", scores.to_adjudicate.len()));
    out
}

// ---- Running a document through the real pipeline, in memory. ----

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct RunSummary {
    done: u64,
    partial: u64,
    unfinished: u64,
    calls: u64,
    rejected: u64,
}

fn eval_server(limits: ExtractionLimits) -> (Server<Fakes>, memex_core::Caller, VaultId) {
    let s = Server::<Fakes>::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(1_000_000),
        SeededRandom::new(24),
        memex_core::testing::mem_search(),
    );
    s.set_extraction(Some(limits));
    let admin = s.authenticate(&s.bootstrap("root").unwrap().token).unwrap();
    let v = s.create_vault(&admin, "eval").unwrap().id;
    (s, admin, v)
}

/// What a changeset proposed, as predictions.
fn predictions_of(changeset: &ChangeSetRow, doc: &Document) -> Vec<Prediction> {
    let mut types: HashMap<String, String> = HashMap::new();
    for op in &changeset.ops {
        if let Op::CreateEntity { id, type_name, .. } = op {
            types.insert(id.0.clone(), type_name.clone());
        }
    }
    let text = |v: &Value| v.as_str().map_or_else(|| v.to_string(), String::from);
    let identity: Option<String> = changeset.ops.iter().find_map(|op| match op {
        Op::AddAssertion { predicate, value, .. } if IDENTITY_FIELDS.contains(&predicate.as_str()) => Some(text(value)),
        _ => None,
    });
    changeset
        .ops
        .iter()
        .filter_map(|op| {
            let Op::AddAssertion {
                subject,
                predicate,
                value,
                evidence,
                ..
            } = op
            else {
                return None;
            };
            let quote = evidence.first().map_or(String::new(), |(_, e)| e.quote.clone());
            let of_entity = entity_of_subject(subject).and_then(|id| types.get(&id.0).cloned());
            Some(match of_entity {
                Some(kind) => Prediction {
                    doc: doc.id.clone(),
                    language: doc.language.clone(),
                    kind,
                    subject: identity.clone().unwrap_or_default(),
                    field: predicate.clone(),
                    value: text(value),
                    quote,
                },
                None => Prediction {
                    doc: doc.id.clone(),
                    language: doc.language.clone(),
                    kind: "claim".into(),
                    subject: subject.clone(),
                    field: predicate.clone(),
                    value: text(value),
                    quote,
                },
            })
        })
        .collect()
}

struct Outcome {
    predictions: Vec<Prediction>,
    usage: AgentUsage,
    state: ExtractionState,
    calls: u64,
    rejected: u64,
}

/// Runs one document through upload, rendition, the extraction job and its proposals, each in a vault of its own, so no document's entities become another's candidates and every result is a fresh prediction.
fn run_document<A: AgentRunner>(doc: &Document, agent: &A, limits: ExtractionLimits, waits: &mut dyn FnMut() -> bool) -> Outcome {
    let (s, admin, v) = eval_server(limits);
    let req = UploadRequest {
        name: format!("{}.txt", doc.id),
        trust_tier: "unverified".into(),
        metadata: json!({"source_url": doc.source_url}),
        replaces: None,
    };
    let source = s
        .upload(&admin, &v, req, UploadLimits::default(), &mut doc.text.as_bytes())
        .unwrap()
        .source
        .header
        .id;
    let job = s.claim_job(&v, RENDITION_JOB, "eval").unwrap().unwrap();
    let rows = s
        .build_rendition(
            &v,
            &source,
            RenditionInput::Plain {
                text: doc.text.clone(),
                parser: "plain-1".into(),
            },
        )
        .unwrap();
    assert!(s.complete_job(&v, &job, rows).unwrap());
    loop {
        let Some(job) = s.claim_job(&v, EXTRACT_JOB, "eval").unwrap() else {
            // Nothing runnable: it's waiting for the LLM, or finished.
            if s.extraction_status(&admin, &v, &source).unwrap().state == ExtractionState::Waiting && waits() {
                s.engine()
                    .clock()
                    .advance(std::time::Duration::from_millis(memex_core::jobs::WAIT_CAP_MS as u64 * 2));
                continue;
            }
            break;
        };
        match s.run_extraction_job(&v, &job, agent) {
            Ok(rows) => assert!(s.complete_job(&v, &job, rows).unwrap()),
            Err(ExtractJobError::Wait(m)) => assert!(s.wait_job(&v, &job, &m).unwrap()),
            Err(ExtractJobError::Retry(m)) => assert!(s.fail_job(&v, &job, &m, true).unwrap()),
            Err(ExtractJobError::Fatal(m)) => assert!(s.fail_job(&v, &job, &m, false).unwrap()),
            Err(ExtractJobError::Lost) => panic!("nothing else holds this job"),
        }
    }
    let report = s.extraction_status(&admin, &v, &source).unwrap();
    let rendition = report.rendition_id.clone().unwrap();
    let derivations = s.list_derivations(&admin, &v, &rendition, None, 200).unwrap();
    let mut usage = AgentUsage::default();
    let mut calls = 0;
    for d in &derivations {
        usage.add(&d.usage);
        calls += u64::from(d.attempts);
    }
    let mut predictions = vec![];
    let mut rejected: u64 = derivations.iter().map(|d| d.rejected.len() as u64).sum();
    for c in s.list_changesets(&admin, &v, None, None, 200).unwrap() {
        if c.state == memex_types::ChangeSetState::Rejected {
            rejected += 1;
        } else {
            predictions.extend(predictions_of(&c, doc));
        }
    }
    Outcome {
        predictions,
        usage,
        state: report.state,
        calls,
        rejected,
    }
}

struct SetRun {
    predictions: Vec<Prediction>,
    usage: AgentUsage,
    summary: RunSummary,
}

fn run_set<A: AgentRunner>(
    docs: &[Document],
    agent_for: impl Fn(&Document) -> A,
    limits: ExtractionLimits,
    token_cap: u64,
    waits: &mut dyn FnMut() -> bool,
) -> SetRun {
    let mut run = SetRun {
        predictions: vec![],
        usage: AgentUsage::default(),
        summary: RunSummary::default(),
    };
    for doc in docs {
        let left = token_cap.saturating_sub(run.usage.total());
        if left < limits.document_budget {
            eprintln!("stopping at {} tokens: another document could pass the cap of {token_cap}", run.usage.total());
            run.summary.unfinished += docs.len() as u64 - run.summary.done - run.summary.partial;
            break;
        }
        let agent = agent_for(doc);
        let outcome = run_document(doc, &agent, limits, waits);
        run.usage.add(&outcome.usage);
        run.summary.calls += outcome.calls;
        run.summary.rejected += outcome.rejected;
        match outcome.state {
            ExtractionState::Done => run.summary.done += 1,
            ExtractionState::Partial => run.summary.partial += 1,
            other => {
                eprintln!("{}: ended {}", doc.id, other.as_str());
                run.summary.unfinished += 1;
            }
        }
        run.predictions.extend(outcome.predictions);
    }
    run
}

// ---- The set itself. ----

#[test]
fn the_set_is_big_enough_and_every_document_says_where_it_came_from() {
    let docs = documents();
    assert!(docs.len() >= 50, "{}", docs.len());
    let mut per_language: BTreeMap<&str, usize> = BTreeMap::new();
    let mut ids = BTreeSet::new();
    for d in &docs {
        *per_language.entry(d.language.as_str()).or_default() += 1;
        assert!(ids.insert(d.id.clone()), "{} twice", d.id);
        assert!(["CC BY-SA 4.0", "CC BY-SA 3.0"].contains(&d.license.as_str()), "{}", d.id);
        assert!(d.source_url.starts_with(&format!("https://{}.wikipedia.org/", d.language)), "{}", d.source_url);
        assert!(d.attribution.contains("Wikipedia contributors"), "{}", d.id);
        assert!(!d.title.is_empty() && d.text.len() > 200, "{}", d.id);
    }
    for language in ["en", "sv", "de", "es"] {
        assert!(per_language.get(language).copied().unwrap_or(0) >= 10, "{language}: {per_language:?}");
    }
    println!("documents per language: {per_language:?}");
}

#[test]
fn every_label_belongs_to_a_document_and_its_values_are_in_the_text() {
    let (docs, labels) = (documents(), labels());
    assert_eq!(docs.len(), labels.len());
    let (mut entities, mut units, mut claims) = (0, 0, 0);
    for l in &labels {
        let d = docs.iter().find(|d| d.id == l.id).unwrap_or_else(|| panic!("no document {}", l.id));
        let text = fold(&d.text);
        assert!(!l.entities.is_empty() || !l.claims.is_empty(), "{} has no labels", l.id);
        for e in &l.entities {
            entities += 1;
            assert!(
                ["Person", "Organization", "Product", "Project", "Decision"].contains(&e.type_name.as_str()),
                "{}",
                e.type_name
            );
            let id = identity_of(e).unwrap_or_else(|| panic!("{}: an entity without a name", l.id));
            assert!(id.iter().any(|a| text.contains(&fold(a))), "{}: {id:?}", l.id);
            for (field, aliases) in &e.fields {
                units += 1;
                assert!(!UNSCORED_FIELDS.contains(&field.as_str()), "{}: {field} isn't scored", l.id);
                let date = matches!(field.as_str(), "released" | "decided_on" | "started");
                if !date {
                    assert!(
                        aliases.iter().any(|a| text.contains(&fold(a))),
                        "{}: {field} {aliases:?} isn't in the text",
                        l.id
                    );
                }
            }
        }
        for c in &l.claims {
            claims += 1;
            assert!(c.subject.iter().any(|a| text.contains(&fold(a))), "{}: {:?}", l.id, c.subject);
            assert!(c.value.iter().any(|a| text.contains(&fold(a))), "{}: {:?}", l.id, c.value);
        }
    }
    println!(
        "{} documents, {entities} entities with {units} field values, {claims} free claims",
        labels.len()
    );
    assert!(units + claims >= 150, "{}", units + claims);
}

// ---- The scorer, on predictions written by hand. ----

fn doc(id: &str, language: &str) -> Document {
    Document {
        id: id.into(),
        language: language.into(),
        title: "t".into(),
        source_url: "u".into(),
        license: "l".into(),
        attribution: "a".into(),
        text: "x".into(),
    }
}

fn aliases(values: &[&str]) -> Aliases {
    values.iter().map(|v| v.to_string()).collect()
}

fn gold() -> Label {
    Label {
        id: "d".into(),
        entities: vec![GoldEntity {
            type_name: "Person".into(),
            fields: BTreeMap::from([
                ("name".to_string(), aliases(&["Marie Curie", "Maria Skłodowska-Curie"])),
                ("title".to_string(), aliases(&["physicist"])),
            ]),
        }],
        claims: vec![
            GoldClaim {
                subject: aliases(&["Marie Curie"]),
                value: aliases(&["polonium"]),
            },
            GoldClaim {
                subject: aliases(&["Curie"]),
                value: aliases(&["Nobel Prize"]),
            },
        ],
    }
}

fn predicted(kind: &str, subject: &str, field: &str, value: &str) -> Prediction {
    Prediction {
        doc: "d".into(),
        language: "en".into(),
        kind: kind.into(),
        subject: subject.into(),
        field: field.into(),
        value: value.into(),
        quote: "q".into(),
    }
}

#[test]
fn matching_ignores_case_and_accents_and_needs_more_than_a_letter() {
    assert!(same_value("MARIE CURIE", "Marie Curie"));
    assert!(same_value("Malmö", "malmo"));
    assert!(same_value("Polish physicist", "physicist"));
    assert!(same_value("Curie", "Marie Curie"));
    assert!(!same_value("a", "Marie Curie"), "too short to be a part");
    assert!(!same_value("", "x"));
    assert!(!same_value("chemist", "physicist"));
}

#[test]
fn the_scorer_counts_right_wrong_and_unknown_apart() {
    let docs = [doc("d", "en")];
    let predictions = [
        predicted("Person", "Marie Curie", "name", "Marie Curie"),
        predicted("Person", "Marie Curie", "title", "chemist"),
        predicted("Person", "Marie Curie", "description", "A long text, not scored."),
        predicted("Person", "Marie Curie", "email", "m@example.org"),
        predicted("Person", "Pierre Curie", "name", "Pierre Curie"),
        predicted("claim", "Marie Curie", "discovered", "polonium"),
        predicted("claim", "Marie Curie", "was born in", "Warsaw"),
        predicted("claim", "Marie Curie", "works as", "physicist"),
    ];
    let scores = score(&docs, &[gold()], &predictions, &[]);
    let person = &scores.cells[&("en".to_string(), "Person".to_string())];
    assert_eq!((person.correct, person.wrong, person.unlabelled, person.unscored), (1, 1, 2, 1));
    assert_eq!((person.gold_found, person.gold_total), (1, 2));
    assert_eq!(person.precision(), Some(0.25));
    assert_eq!(person.recall(), Some(0.5));
    let claim = &scores.cells[&("en".to_string(), "claim".to_string())];
    assert_eq!(
        (claim.correct, claim.wrong, claim.unlabelled),
        (2, 0, 1),
        "a field's fact as a free claim is correct"
    );
    assert_eq!((claim.gold_found, claim.gold_total), (1, 2), "and earns no recall");
    assert_eq!(scores.to_adjudicate.len(), 3);
    let total = scores.total();
    assert_eq!(
        (total.correct, total.wrong, total.unlabelled, total.gold_found, total.gold_total),
        (3, 1, 3, 2, 4)
    );
}

#[test]
fn a_verdict_settles_an_unlabelled_prediction_but_not_a_contradiction() {
    let docs = [doc("d", "en")];
    let right = predicted("claim", "Marie Curie", "was born in", "Warsaw");
    let wrong = predicted("Person", "Pierre Curie", "name", "Pierre Curie");
    let contradiction = predicted("Person", "Marie Curie", "title", "chemist");
    let verdicts = [
        Adjudication {
            doc: "d".into(),
            prediction: right.key(),
            correct: true,
        },
        Adjudication {
            doc: "d".into(),
            prediction: wrong.key(),
            correct: false,
        },
        Adjudication {
            doc: "d".into(),
            prediction: contradiction.key(),
            correct: true,
        },
    ];
    let scores = score(&docs, &[gold()], &[right, wrong, contradiction], &verdicts);
    let t = scores.total();
    assert_eq!((t.correct, t.wrong, t.unlabelled), (1, 2, 0));
    assert!(scores.to_adjudicate.is_empty());
}

#[test]
fn the_closest_entity_gets_the_credit() {
    let docs = [doc("d", "en")];
    let entity = |name: &str| GoldEntity {
        type_name: "Organization".into(),
        fields: BTreeMap::from([("name".to_string(), aliases(&[name]))]),
    };
    let label = Label {
        id: "d".into(),
        entities: vec![entity("Zara"), entity("Zara Home")],
        claims: vec![],
    };
    let scores = score(
        &docs,
        &[label],
        &[
            predicted("Organization", "Zara Home", "name", "Zara Home"),
            predicted("Organization", "Zara", "name", "Zara"),
        ],
        &[],
    );
    let t = scores.total();
    assert_eq!((t.correct, t.gold_found, t.gold_total), (2, 2, 2));
}

#[test]
fn duplicates_count_as_predictions_but_a_gold_value_is_found_once() {
    let docs = [doc("d", "en")];
    let p = predicted("Person", "Marie Curie", "name", "Marie Curie");
    let scores = score(&docs, &[gold()], &[p.clone(), p], &[]);
    let t = scores.total();
    assert_eq!((t.correct, t.gold_found), (2, 1));
}

#[test]
fn no_predictions_mean_no_precision_and_no_recall_rather_than_a_perfect_score() {
    let docs = [doc("d", "en")];
    let t = score(&docs, &[gold()], &[], &[]).total();
    assert_eq!(t.precision(), None);
    assert_eq!(t.recall(), Some(0.0));
}

// ---- The whole harness, on a scripted agent. This proves the code, and is not a measurement. ----

/// Writes a reply from the labels, plus the planted mistakes the test counts on.
fn reply_from_labels(label: &Label, req: &AgentRequest, plant: bool) -> Result<AgentReply, AgentError> {
    let input: Value = serde_json::from_str(req.input.split("\n\nYour previous").next().unwrap()).unwrap();
    let segments: Vec<(String, String)> = input["segments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["id"].as_str().unwrap().to_string(), s["text"].as_str().unwrap().to_string()))
        .collect();
    // A label's alias may differ from the text in case and spacing, such as a no-break space, so the quote is taken from the text with its spaces evened out.
    let find = |aliases: &[String]| -> Option<(String, String)> {
        for (id, text) in &segments {
            let even = text.split_whitespace().collect::<Vec<_>>().join(" ");
            let lower = even.to_lowercase();
            for a in aliases {
                let pattern = a.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
                if lower.len() == even.len()
                    && let Some(at) = lower.find(&pattern)
                {
                    return Some((id.clone(), even[at..at + pattern.len()].to_string()));
                }
            }
        }
        None
    };
    let mut entities: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for e in &label.entities {
        let id = identity_of(e).unwrap();
        let Some((segment, quote)) = find(id) else { continue };
        let fields: serde_json::Map<String, Value> = e.fields.iter().map(|(k, v)| (k.clone(), json!(v[0]))).collect();
        entities
            .entry(e.type_name.clone())
            .or_default()
            .push(json!({"segment_id": segment, "quote": quote, "fields": fields}));
    }
    let mut claims = vec![];
    for c in &label.claims {
        if let Some((segment, quote)) = find(&c.value) {
            claims.push(json!({"segment_id": segment, "quote": quote, "subject": c.subject[0], "predicate": "is about", "value": c.value[0]}));
        }
    }
    if plant {
        let (id, text) = &segments[0];
        let quote: String = text.chars().take(20).collect();
        claims.push(json!({"segment_id": id, "quote": quote, "subject": "Planted subject nobody labelled", "predicate": "is", "value": "planted"}));
        claims.push(
            json!({"segment_id": id, "quote": "A sentence that appears in no document at all.", "subject": "Invented", "predicate": "is", "value": "invented"}),
        );
    }
    Ok(AgentReply {
        json: json!({"claims": claims, "entities": entities}).to_string(),
        usage: AgentUsage {
            input_tokens: 1000,
            output_tokens: 100,
            thinking_tokens: 0,
        },
        tool_calls: 0,
    })
}

#[test]
fn the_harness_runs_the_whole_set_through_the_pipeline_on_a_scripted_agent() {
    let (docs, all_labels) = (documents(), labels());
    let limits = ExtractionLimits::default();
    let shared = all_labels.clone();
    let planted_docs = docs.iter().step_by(5).map(|d| d.id.clone()).collect::<BTreeSet<_>>();
    let planted = planted_docs.clone();
    let run = run_set(
        &docs,
        |d| {
            let label = shared.iter().find(|l| l.id == d.id).unwrap().clone();
            let plant = planted.contains(&d.id);
            ScriptedAgent::answering(move |req| reply_from_labels(&label, req, plant))
        },
        limits,
        u64::MAX,
        &mut || false,
    );
    let scores = score(&docs, &all_labels, &run.predictions, &[]);
    let total = scores.total();
    let text = report(
        "SCRIPTED AGENT: replies written from the labels. NOT a measurement of any model.",
        &scores,
        &run.usage,
        &run.summary,
    );
    println!("{text}");

    assert_eq!(run.summary.done, docs.len() as u64);
    assert_eq!(run.summary.unfinished, 0);
    // Each planted document has one invented quote, refused before it can become a proposal, and one unlabelled claim.
    assert_eq!(total.unlabelled, planted_docs.len() as u64);
    assert_eq!(scores.to_adjudicate.len(), planted_docs.len());
    assert_eq!(run.summary.rejected, planted_docs.len() as u64);
    assert_eq!(total.wrong, 0);
    // Written from the labels, so every labelled value comes back, however it's split across segments.
    assert_eq!(total.gold_found, total.gold_total, "{:#?}", scores.missed);
    assert_eq!(total.correct + total.unlabelled, run.predictions.len() as u64 - total.unscored);
    assert_eq!(run.usage.input_tokens, run.summary.calls * 1000);
}

#[test]
fn a_token_cap_stops_the_set_before_it_can_be_passed() {
    let (docs, all_labels) = (documents(), labels());
    let limits = ExtractionLimits {
        document_budget: 5_000,
        ..ExtractionLimits::default()
    };
    let shared = all_labels.clone();
    let run = run_set(
        &docs,
        |d| {
            let label = shared.iter().find(|l| l.id == d.id).unwrap().clone();
            ScriptedAgent::answering(move |req| reply_from_labels(&label, req, false))
        },
        limits,
        7_000,
        &mut || false,
    );
    assert!(run.usage.total() <= 7_000, "{}", run.usage.total());
    assert!(run.summary.unfinished > 0);
}

/// What a live run should cost, from the real prompt and documents, with no call made. Output is what the labels would take to write out, plus a margin.
fn estimate() -> (u64, u64, u64) {
    let docs = documents();
    let all_labels = labels();
    let prompt = memex_core::extraction::build_prompt(memex_core::entity::seed_types());
    let (mut input, mut output) = (0u64, 0u64);
    for d in &docs {
        input += prompt.estimated_tokens + memex_core::extraction::estimated_tokens(&d.text) + 20;
        let label = all_labels.iter().find(|l| l.id == d.id).unwrap();
        // About 45 tokens to return a field or claim with its quote, and models find about twice what's labelled.
        let units: u64 = label.entities.iter().map(|e| e.fields.len() as u64).sum::<u64>() + label.claims.len() as u64;
        output += units * 45 * 2 + 30;
    }
    (docs.len() as u64, input, output)
}

#[test]
fn the_estimate_of_a_live_run_uses_no_llm() {
    let (docs, input, output) = estimate();
    println!(
        "estimate for one live run over {docs} documents: input {input} tokens, output {output} tokens, total {}",
        input + output
    );
    assert!(input > 0 && output > 0);
}

// ---- The live run. Nothing above calls an LLM. ----

/// Runs the set against the LLM in `MEMEX_LLM_*`. It sends documents to that provider, and costs money, so it needs all of: a provider, `MEMEX_EVAL_APPROVED` naming a file of the document ids approved for sending, and `MEMEX_EVAL_MAX_TOKENS`.
#[test]
#[ignore = "calls a live LLM, and sends documents to it"]
fn live_labelled_set() {
    let settings = memex_core::Settings::from_vars(|name| std::env::var(name).ok()).expect("settings");
    let llm = settings.llm.clone().expect("not run: MEMEX_LLM_PROVIDER isn't set");
    let approved_file = std::env::var("MEMEX_EVAL_APPROVED").expect("not run: MEMEX_EVAL_APPROVED must name a file of document ids approved for sending");
    let cap: u64 = std::env::var("MEMEX_EVAL_MAX_TOKENS")
        .expect("not run: MEMEX_EVAL_MAX_TOKENS isn't set")
        .parse()
        .expect("a number of tokens");
    let approved: BTreeSet<String> = std::fs::read_to_string(&approved_file)
        .expect("the approved list")
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let all = documents();
    let docs: Vec<Document> = all.iter().filter(|d| approved.contains(&d.id)).cloned().collect();
    println!(
        "{} of {} documents are approved for sending to {} {}",
        docs.len(),
        all.len(),
        llm.provider,
        llm.model
    );
    let adjudications: Vec<Adjudication> = match std::env::var("MEMEX_EVAL_ADJUDICATIONS") {
        Ok(path) => std::fs::read_to_string(path)
            .expect("adjudications")
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("an adjudication"))
            .collect(),
        Err(_) => vec![],
    };
    let endpoint = memexvault::edges::llm_endpoint(&llm).expect("endpoint");
    let mut outage_waits = 0;
    let run = run_set(
        &docs,
        |_| memexvault::edges::AdkAgentRunner(memex_agent_adk::AdkAgent::new(endpoint.clone()).expect("an agent")),
        settings.extraction,
        cap,
        &mut || {
            // An outage in the middle of the set: wait a while for it, and give up rather than sit forever.
            outage_waits += 1;
            std::thread::sleep(std::time::Duration::from_secs(30));
            outage_waits <= 20
        },
    );
    let scores = score(&docs, &labels(), &run.predictions, &adjudications);
    let text = report(&format!("LIVE: {} {}", llm.provider, llm.model), &scores, &run.usage, &run.summary);
    println!("{text}");
    if let Ok(path) = std::env::var("MEMEX_EVAL_OUT") {
        let dump = json!({"predictions": run.predictions, "to_adjudicate": scores.to_adjudicate, "missed": scores.missed});
        std::fs::write(path, serde_json::to_vec_pretty(&dump).unwrap()).expect("write the report");
    }
    let per_language: BTreeMap<&str, usize> = docs.iter().fold(BTreeMap::new(), |mut m, d| {
        *m.entry(d.language.as_str()).or_default() += 1;
        m
    });
    assert!(
        docs.len() >= 50 && ["en", "sv", "de", "es"].iter().all(|l| per_language.get(l).copied().unwrap_or(0) >= 10),
        "too few documents for item 1: {per_language:?}"
    );
    assert_eq!(run.summary.unfinished, 0, "the set didn't finish");
    let precision = scores.total().precision().expect("predictions");
    assert!(
        precision >= 0.9,
        "precision {precision:.3} is under 0.9, with {} predictions awaiting a verdict",
        scores.to_adjudicate.len()
    );
}
