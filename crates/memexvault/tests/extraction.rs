//! Extraction on real Postgres, with a scripted agent, so no LLM is called and no document leaves the machine. Run with `MEMEX_TEST_DATABASE_URL` set, as in AGENTS.md.
// These tests read the environment and print what they measured.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_core::testing::{FakeEmbedder, FakeRasterizer, FakeReranker, ManualClock, MemBlobStore, MemRegistry, ScriptedAgent};
use memex_core::{
    EXTRACT_JOB, ExtractJobError, ExtractionLimits, ExtractionState, RENDITION_JOB, RenditionInput, SearchEdges, Server, ServerEdges, UploadLimits,
    UploadRequest,
};
use memex_lexical::TantivyLexical;
use memex_store_postgres::{PgStore, schema_of_vault};
use memex_types::{
    AgentError, AgentReply, AgentRequest, AgentUsage, ChangeSetState, DocKind, LexicalFilter, ObjectId, PageClass, PageFacts, ParsedDocument, Rect, VaultId,
    Word,
};
use memex_vector::PgVectors;
use memexvault::edges::{PgVectorIndex, SystemClock, SystemSeed, TantivyIndex};
use memexvault::pg_edge::PostgresStore;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

struct Real;

impl ServerEdges for Real {
    type Registry = MemRegistry;
    type Store = PostgresStore;
    type Blobs = MemBlobStore<ManualClock>;
    type Clock = SystemClock;
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

fn url() -> String {
    std::env::var("MEMEX_TEST_DATABASE_URL").expect("not run: MEMEX_TEST_DATABASE_URL isn't set")
}

fn rig(name: &str) -> Rig {
    let pg = PgStore::connect(&url(), 16).expect("connect");
    pg.migrate().expect("migrate");
    let pg = Arc::new(pg);
    let dir = std::env::temp_dir().join(format!("memex-extraction-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let s = Arc::new(Server::<Real>::new(
        MemRegistry::new(),
        PostgresStore(pg.clone()),
        MemBlobStore::new(ManualClock::starting_at(0)),
        SystemClock::default(),
        SystemSeed,
        SearchEdges {
            lexical: TantivyIndex(TantivyLexical::new(&dir)),
            vector: PgVectorIndex(PgVectors::new(pg.clone())),
            embedder: FakeEmbedder::default(),
            reranker: FakeReranker::default(),
        },
    ));
    s.set_extraction(Some(ExtractionLimits {
        batch_tokens: 150,
        ..ExtractionLimits::default()
    }));
    let admin = s.authenticate(&s.bootstrap("root").unwrap().token).unwrap();
    let v = s.create_vault(&admin, name).unwrap().id;
    Rig { s, admin, v, dir }
}

fn document(tag: usize, parts: usize) -> String {
    (0..parts)
        .map(|i| {
            let filler = "Das ist ein langer Satz über gar nichts, der den Absatz nur mit Wörtern füllt. ".repeat(12);
            format!("Dokument {tag} Teil {i} nennt Stockholm als Ort. {filler}")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
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

const USAGE: AgentUsage = AgentUsage {
    input_tokens: 500,
    output_tokens: 60,
    thinking_tokens: 3,
};

fn one_claim_each(req: &AgentRequest) -> Result<AgentReply, AgentError> {
    let claims: Vec<Value> = segments_of(req)
        .into_iter()
        .map(|(id, text)| {
            let quote = text.split(" als Ort.").next().unwrap().to_string();
            json!({"segment_id": id, "quote": quote, "subject": quote, "predicate": "nennt", "value": "Stockholm"})
        })
        .collect();
    Ok(AgentReply {
        json: json!({"claims": claims, "entities": {}}).to_string(),
        usage: USAGE,
        tool_calls: 0,
    })
}

impl Rig {
    fn upload(&self, name: &str, bytes: &[u8]) -> ObjectId {
        let req = UploadRequest {
            name: name.into(),
            trust_tier: "unverified".into(),
            metadata: json!({}),
            replaces: None,
        };
        let mut b = bytes;
        self.s
            .upload(&self.admin, &self.v, req, UploadLimits::default(), &mut b)
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

    fn ingest_text(&self, text: &str) -> ObjectId {
        let source = self.upload("a.txt", text.as_bytes());
        self.parse(
            &source,
            RenditionInput::Plain {
                text: text.into(),
                parser: "plain-1".into(),
            },
        );
        source
    }

    fn ingest_pdf(&self, text: &str) -> ObjectId {
        let source = self.upload("a.pdf", format!("%PDF-1.7 {}", text.len()).as_bytes());
        let words: Vec<Word> = text
            .split_whitespace()
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
            .collect();
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
            words,
        };
        self.parse(&source, RenditionInput::Pdf(doc, vec![]));
        source
    }

    /// Works through every runnable extraction job.
    fn work(&self, agent: &ScriptedAgent) -> usize {
        let mut n = 0;
        while let Some(job) = self.s.claim_job(&self.v, EXTRACT_JOB, "w").unwrap() {
            n += 1;
            match self.s.run_extraction_job(&self.v, &job, agent) {
                Ok(rows) => assert!(self.s.complete_job(&self.v, &job, rows).unwrap()),
                Err(ExtractJobError::Wait(m)) => assert!(self.s.wait_job(&self.v, &job, &m).unwrap()),
                Err(e) => panic!("{e:?}"),
            }
        }
        n
    }

    fn approve_all(&self) -> usize {
        let pending = self.s.list_changesets(&self.admin, &self.v, Some(ChangeSetState::Pending), None, 200).unwrap();
        for c in &pending {
            self.s.approve(&self.admin, &self.v, &c.header.id, None).unwrap();
        }
        pending.len()
    }

    fn count(&self, query: &str) -> i64 {
        let mut c = postgres::Client::connect(&url(), postgres::NoTls).expect("connect");
        let sql = query.replace("{s}", &schema_of_vault(&self.v));
        c.query_one(&sql, &[]).expect("query").get(0)
    }
}

/// Active machine claims that have no evidence with an anchor that resolves. Every committed claim must have one, so this stays at zero.
const UNANCHORED: &str = "SELECT count(*) FROM {s}.assertions a WHERE a.active AND (a.row->'assertion'->>'machine')::boolean AND NOT EXISTS (
    SELECT 1 FROM {s}.evidence e
    WHERE e.assertion_id = a.id AND NOT e.anchor_broken AND jsonb_typeof(e.row->'evidence'->'anchor') = 'object'
      AND (e.row->'evidence'->'anchor'->>'end')::bigint > (e.row->'evidence'->'anchor'->>'start')::bigint
      AND e.row->'evidence'->'anchor'->>'source_sha256' IS NOT NULL)";
const MACHINE_CLAIMS: &str = "SELECT count(*) FROM {s}.assertions a WHERE a.active AND (a.row->'assertion'->>'machine')::boolean";

#[test]
#[ignore = "needs Postgres"]
fn pg_extraction_every_committed_claim_resolves_to_an_anchor() {
    let r = rig("extract-anchors");
    for tag in 0..20 {
        r.ingest_text(&document(tag, 3));
    }
    let pdf = r.ingest_pdf(&document(100, 1));
    let agent = ScriptedAgent::answering(one_claim_each);
    assert_eq!(r.work(&agent), 21);
    assert_eq!(r.approve_all(), 61);
    let (claims, unanchored) = (r.count(MACHINE_CLAIMS), r.count(UNANCHORED));
    println!("committed machine claims {claims}, without a resolvable anchor {unanchored}");
    assert_eq!(claims, 61);
    assert_eq!(unanchored, 0);

    // The PDF's claim has rectangles, and the plain text's have none to have. Each anchor's quote is the evidence's.
    let with_rects = r.count("SELECT count(*) FROM {s}.evidence e WHERE jsonb_array_length(e.row->'evidence'->'anchor'->'rects') > 0");
    assert_eq!(with_rects, 1);
    let differ = r.count("SELECT count(*) FROM {s}.evidence e WHERE e.row->'evidence'->'anchor'->>'quote' IS DISTINCT FROM e.row->'evidence'->>'quote'");
    assert_eq!(differ, 0);
    assert_eq!(r.s.extraction_status(&r.admin, &r.v, &pdf).unwrap().state, ExtractionState::Done);
}

#[test]
#[ignore = "needs Postgres"]
fn pg_extraction_tokens_reruns_and_an_outage() {
    let r = rig("extract-runs");
    let source = r.ingest_text(&document(0, 3));

    // Down: the job waits, and the rest answers.
    while r.s.project_lexical(&r.v).unwrap() == memex_core::StepOutcome::Progress {}
    let down = ScriptedAgent::new(vec![]);
    assert_eq!(r.work(&down), 1);
    let waiting = r.s.extraction_status(&r.admin, &r.v, &source).unwrap();
    assert_eq!(waiting.state, ExtractionState::Waiting);
    assert_eq!(r.count("SELECT count(*) FROM {s}.jobs WHERE kind = 'extract' AND state = 'queued'"), 1);
    let hits =
        r.s.search_lexical(
            &r.admin,
            &r.v,
            "Stockholm",
            LexicalFilter {
                kinds: vec![DocKind::Segment],
                ..LexicalFilter::default()
            },
            5,
        )
        .unwrap();
    assert!(!hits.hits.is_empty());
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
    r.s.upload(&r.admin, &r.v, req, tight, &mut "while it waits".as_bytes()).unwrap();
    let rendition = r.s.claim_job(&r.v, RENDITION_JOB, "other").unwrap().unwrap();
    assert!(r.s.complete_job(&r.v, &rendition, vec![]).unwrap());

    // Back: a run makes one proposal per segment, with its tokens stored.
    let job = r.s.extraction_status(&r.admin, &r.v, &source).unwrap().job_id.unwrap();
    let mut c = postgres::Client::connect(&url(), postgres::NoTls).unwrap();
    let schema = schema_of_vault(&r.v);
    c.execute(
        &format!("UPDATE {schema}.jobs SET next_run_at_ms = 0, row = jsonb_set(row, '{{job,next_run_at_ms}}', '0') WHERE id = $1"),
        &[&job.0],
    )
    .unwrap();
    let agent = ScriptedAgent::answering(one_claim_each);
    assert_eq!(r.work(&agent), 1);
    let calls = agent.requests().len() as i64;
    assert_eq!(calls, 3);
    assert_eq!(r.s.extraction_status(&r.admin, &r.v, &source).unwrap().state, ExtractionState::Done);
    let spent = r.count("SELECT sum((row->'derivation'->'usage'->>'input_tokens')::bigint)::bigint FROM {s}.derivations");
    let spent_thinking = r.count("SELECT sum((row->'derivation'->'usage'->>'thinking_tokens')::bigint)::bigint FROM {s}.derivations");
    println!("{calls} calls, input tokens on derivations {spent}, thinking {spent_thinking}");
    assert_eq!(spent, calls * USAGE.input_tokens as i64);
    assert_eq!(spent_thinking, calls * USAGE.thinking_tokens as i64);

    // Again: nothing new, and the same across a pending and a committed state.
    let (changesets, derivations) = (r.count("SELECT count(*) FROM {s}.changesets"), r.count("SELECT count(*) FROM {s}.derivations"));
    assert_eq!((changesets, derivations), (3, 3));
    r.s.rerun_extraction(&r.admin, &r.v, &source).unwrap();
    let again = ScriptedAgent::answering(one_claim_each);
    r.work(&again);
    assert!(again.requests().is_empty());
    r.approve_all();
    r.s.rerun_extraction(&r.admin, &r.v, &source).unwrap();
    r.work(&again);
    assert!(again.requests().is_empty());
    assert_eq!(
        (r.count("SELECT count(*) FROM {s}.changesets"), r.count("SELECT count(*) FROM {s}.derivations")),
        (3, 3)
    );
    assert_eq!(r.count(MACHINE_CLAIMS), 3);
    assert_eq!(r.count(UNANCHORED), 0);
    let row = r.count("SELECT count(*) FROM {s}.extractions");
    assert_eq!(row, 1);
}
