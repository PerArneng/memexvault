//! Recordings on real Postgres, with a word table made here and a scripted agent, so no model runs, no LLM is called and nothing leaves the machine. Run with `MEMEX_TEST_DATABASE_URL` set, as in AGENTS.md. `meeting_eval.rs` has the real transcriber.
// These tests read the environment and print what they measured.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_core::testing::{FakeEmbedder, FakeRasterizer, FakeReranker, ManualClock, MemBlobStore, MemRegistry, ScriptedAgent};
use memex_core::{
    EMBED_JOB, EXTRACT_JOB, ExtractJobError, ExtractionLimits, RENDITION_JOB, RenditionInput, SearchEdges, Server, ServerEdges, StepOutcome, TRANSCRIBE_JOB,
    UploadLimits, UploadRequest,
};
use memex_lexical::TantivyLexical;
use memex_store_postgres::{PgStore, schema_of_vault};
use memex_types::{
    AgentReply, AgentRequest, AgentUsage, ChangeSetState, ItemKind, MediaFacts, ObjectId, SearchRequest, TimedDocument, TimedWord, TranscribeUsage, VaultId,
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
    type Caches = memex_core::NoCaches;
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
    let pg = PgStore::connect(&url(), 8).expect("connect");
    pg.migrate().expect("migrate");
    let pg = Arc::new(pg);
    let dir = std::env::temp_dir().join(format!("memex-meeting-{name}-{}", std::process::id()));
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
        batch_tokens: 200,
        ..ExtractionLimits::default()
    }));
    let admin = s.authenticate(&s.bootstrap("root").unwrap().token).unwrap();
    let v = s.create_vault(&admin, name).unwrap().id;
    Rig { s, admin, v, dir }
}

/// Sentences with a pause after every few, and a speaker per turn.
fn spoken(sentences: usize) -> Vec<TimedWord> {
    let (mut words, mut at) = (vec![], 1000);
    for n in 0..sentences {
        let speaker = format!("S{}", n / 4 % 3);
        for (k, w) in format!("Mening {n} säger att pumpen {n} står i Malmö.").split_whitespace().enumerate() {
            words.push(TimedWord {
                text: if k == 1 { format!("nummer{n}") } else { w.to_string() },
                start_ms: at,
                end_ms: at + 280,
                speaker: Some(speaker.clone()),
            });
            at += 300;
        }
        at += if n % 4 == 3 { 2000 } else { 200 };
    }
    words
}

fn recording_of(words: Vec<TimedWord>) -> TimedDocument {
    let end = words.last().map_or(0, |w| w.end_ms) + 1000;
    TimedDocument {
        parser: "fake+memex-1".into(),
        media: MediaFacts {
            codec: "pcm".into(),
            sample_rate: 16_000,
            channels: 1,
            duration_ms: end,
            ..Default::default()
        },
        words,
        usage: TranscribeUsage::default(),
    }
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

/// Says every sentence of what it's given is a claim. A script, so it shows the machinery and nothing about a model.
fn every_sentence(req: &AgentRequest) -> Result<AgentReply, memex_types::AgentError> {
    let mut claims = vec![];
    for (id, text) in segments_of(req) {
        for sentence in text.split_inclusive(['.', '!', '?']) {
            let quote = sentence.split_whitespace().collect::<Vec<_>>().join(" ");
            if quote.split_whitespace().count() >= 3 {
                claims.push(json!({"segment_id": id, "quote": quote, "subject": quote, "predicate": "sägs", "value": "i mötet"}));
            }
        }
    }
    Ok(AgentReply {
        json: json!({"claims": claims, "entities": {}}).to_string(),
        usage: AgentUsage {
            input_tokens: 500,
            output_tokens: 60,
            thinking_tokens: 0,
        },
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

    fn ingest_recording(&self, doc: TimedDocument) -> ObjectId {
        let source = self.upload("a.wav", b"RIFF\x24\0\0\0WAVEfmt  some bytes of a recording");
        let job = self.s.claim_job(&self.v, TRANSCRIBE_JOB, "w").unwrap().expect("a transcription job");
        let rows = self.s.build_rendition(&self.v, &source, RenditionInput::Timed(doc)).unwrap();
        assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
        source
    }

    fn ingest_text(&self, text: &str) -> ObjectId {
        let source = self.upload("a.txt", text.as_bytes());
        let job = self.s.claim_job(&self.v, RENDITION_JOB, "w").unwrap().expect("a rendition job");
        let rows = self
            .s
            .build_rendition(
                &self.v,
                &source,
                RenditionInput::Plain {
                    text: text.into(),
                    parser: "plain-1".into(),
                },
            )
            .unwrap();
        assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
        source
    }

    fn extract(&self, agent: &ScriptedAgent) {
        while let Some(job) = self.s.claim_job(&self.v, EXTRACT_JOB, "w").unwrap() {
            match self.s.run_extraction_job(&self.v, &job, agent) {
                Ok(rows) => assert!(self.s.complete_job(&self.v, &job, rows).unwrap()),
                Err(ExtractJobError::Wait(m)) => panic!("{m}"),
                Err(e) => panic!("{e:?}"),
            }
        }
    }

    fn approve_all(&self) -> usize {
        let mut total = 0;
        loop {
            let pending = self.s.list_changesets(&self.admin, &self.v, Some(ChangeSetState::Pending), None, 200).unwrap();
            if pending.is_empty() {
                return total;
            }
            for c in &pending {
                self.s.approve(&self.admin, &self.v, &c.header.id, None).unwrap();
            }
            total += pending.len();
        }
    }

    fn count(&self, query: &str) -> i64 {
        let mut c = postgres::Client::connect(&url(), postgres::NoTls).expect("connect");
        c.query_one(&query.replace("{s}", &schema_of_vault(&self.v)), &[]).expect("query").get(0)
    }

    fn project(&self) {
        while let Some(job) = self.s.claim_job(&self.v, EMBED_JOB, "w").unwrap() {
            self.s.run_embed_job(&self.v, &job).unwrap();
            assert!(self.s.complete_job(&self.v, &job, vec![]).unwrap());
        }
        while self.s.project_lexical(&self.v).unwrap() == StepOutcome::Progress {}
        while self.s.project_vector(&self.v).unwrap() == StepOutcome::Progress {}
    }
}

/// Item 2. Active machine claims with no evidence holding a time range that makes sense.
const NO_TIME: &str = "SELECT count(*) FROM {s}.assertions a WHERE a.active AND (a.row->'assertion'->>'machine')::boolean AND NOT EXISTS (
    SELECT 1 FROM {s}.evidence e
    WHERE e.assertion_id = a.id AND NOT e.anchor_broken
      AND (e.row->'evidence'->'anchor'->'time'->>'t_end_ms')::bigint > (e.row->'evidence'->'anchor'->'time'->>'t_start_ms')::bigint)";
const MACHINE_CLAIMS: &str = "SELECT count(*) FROM {s}.assertions a WHERE a.active AND (a.row->'assertion'->>'machine')::boolean";

#[test]
#[ignore = "needs Postgres"]
fn pg_meeting_every_committed_claim_has_a_time_anchor_that_resolves() {
    let r = rig("meet-anchors");
    let words = spoken(60);
    let source = r.ingest_recording(recording_of(words.clone()));
    assert!(r.s.get_source(&r.admin, &r.v, &source).unwrap().media_type.starts_with("audio/"));
    let rendition = r.s.list_renditions(&r.admin, &r.v, &source).unwrap().remove(0);
    assert!(rendition.segment_count >= 2, "{}", rendition.segment_count);
    assert_eq!(rendition.media.as_ref().unwrap().codec, "pcm");
    let agent = ScriptedAgent::answering(every_sentence);
    r.extract(&agent);
    let approved = r.approve_all();
    let (claims, without) = (r.count(MACHINE_CLAIMS), r.count(NO_TIME));
    println!("recording: {approved} proposals approved, {claims} committed machine claims, {without} without a resolving time anchor");
    assert_eq!(claims, 60, "one per sentence");
    assert_eq!(without, 0);
    // And each resolves through the provenance API to the time its own words were said.
    let citing = r.s.claims_citing_source(&r.admin, &r.v, &source).unwrap();
    assert_eq!(citing.len(), 60);
    for c in citing {
        let lineage = r.s.lineage(&r.admin, &r.v, &c.assertion.header.id).unwrap();
        let e = &lineage.evidence[0];
        assert!(!e.broken);
        let time = e.anchor.as_ref().and_then(|a| a.time.clone()).unwrap();
        let n: usize = e
            .evidence
            .quote
            .split_whitespace()
            .nth(1)
            .unwrap()
            .trim_start_matches("nummer")
            .parse()
            .unwrap();
        let first = words.iter().find(|w| w.text == format!("nummer{n}")).unwrap();
        assert!(
            time.t_start_ms <= first.start_ms && first.start_ms - time.t_start_ms <= 1000,
            "{time:?} {first:?}"
        );
        assert!(time.t_end_ms >= first.end_ms);
        assert_eq!(e.segment.source_id, source);
        let sp = e.segment.time.as_ref().unwrap();
        assert!(
            sp.t_start_ms <= time.t_start_ms && sp.t_end_ms >= time.t_end_ms,
            "a claim's range sits inside its segment's"
        );
    }
}

#[test]
#[ignore = "needs Postgres"]
fn pg_meeting_a_search_returns_meeting_and_document_evidence_in_one_package() {
    let r = rig("meet-search");
    let text = "Dokumentet säger att ventilen vid pumpstationen byttes i mars.\n\nDet är en annan sak än själva stationen.";
    let doc_source = r.ingest_text(text);
    let words: Vec<TimedWord> = "Vi såg i mötet att ventilen vid pumpstationen läckte i april."
        .split_whitespace()
        .enumerate()
        .map(|(i, w)| TimedWord {
            text: w.into(),
            start_ms: 5000 + i as i64 * 400,
            end_ms: 5000 + i as i64 * 400 + 380,
            speaker: Some("S1".into()),
        })
        .collect();
    let meeting_source = r.ingest_recording(recording_of(words));
    let agent = ScriptedAgent::answering(every_sentence);
    r.extract(&agent);
    assert_eq!(r.approve_all(), 3);
    r.project();
    let package = r.s.search(&r.admin, &r.v, &SearchRequest::new("ventilen pumpstationen")).unwrap();
    assert!(package.degraded.is_empty(), "{:?}", package.degraded);
    let cited: Vec<_> = package.items.iter().flat_map(|i| i.citations.iter().map(move |c| (i, c))).collect();
    let from = |id: &ObjectId, kind: ItemKind| {
        cited
            .iter()
            .filter(|(i, c)| c.source_id.as_ref() == Some(id) && i.kind == kind)
            .collect::<Vec<_>>()
    };
    // Each kind from each source, in one package.
    for (name, source) in [("the meeting", &meeting_source), ("the document", &doc_source)] {
        assert!(!from(source, ItemKind::Claim).is_empty(), "a claim from {name}: {:?}", package.items);
        assert!(!from(source, ItemKind::Segment).is_empty(), "a segment from {name}: {:?}", package.items);
    }
    for (item, c) in from(&meeting_source, ItemKind::Claim)
        .into_iter()
        .chain(from(&meeting_source, ItemKind::Segment))
    {
        let t = c.time.as_ref().unwrap_or_else(|| panic!("a meeting citation has a time: {c:?}"));
        assert_eq!((t.t_start_ms, t.speaker.as_deref()), (4400, Some("S1")), "{item:?}");
        assert_eq!(t.t_end_ms, 5000 + 10 * 400 + 380 + 300);
        assert!(c.start.is_some() && c.end.is_some());
        assert!(c.rects.is_empty());
    }
    for (item, c) in from(&doc_source, ItemKind::Claim).into_iter().chain(from(&doc_source, ItemKind::Segment)) {
        assert!(c.time.is_none(), "a document has no time: {c:?}");
        let (a, b) = (c.start.unwrap() as usize, c.end.unwrap() as usize);
        assert!(text.is_char_boundary(a) && text.is_char_boundary(b), "{item:?}");
        if item.kind == ItemKind::Claim {
            assert_eq!(Some(&text[a..b]), c.quote.as_deref());
        }
    }
    // Every claim's citation is one the API's own shape can carry.
    let dto: Vec<memex_api::CitationDto> = cited.iter().map(|(_, c)| (*c).clone().into()).collect();
    assert!(dto.iter().any(|d| d.t_start_ms == Some(4400) && d.speaker.as_deref() == Some("S1")));
    println!("search: {} items, {} citations, from {} sources", package.items.len(), cited.len(), 2);
}

#[test]
#[ignore = "needs Postgres"]
fn pg_meeting_a_job_that_failed_leaves_others_going() {
    let r = rig("meet-fail");
    let bad = r.upload("bad.wav", b"RIFF\x24\0\0\0WAVEfmt  this one will fail");
    let job = r.s.claim_job(&r.v, TRANSCRIBE_JOB, "w").unwrap().unwrap();
    assert!(r.s.fail_job(&r.v, &job, "not a readable recording", false).unwrap());
    assert_eq!(r.s.get_job(&r.admin, &r.v, &job.header.id).unwrap().state, memex_types::JobState::Dead);
    // Its original stays, and the next recording goes on as usual.
    assert_eq!(r.s.get_source(&r.admin, &r.v, &bad).unwrap().name, "bad.wav");
    assert!(r.s.list_renditions(&r.admin, &r.v, &bad).unwrap().is_empty());
    let good = r.ingest_recording(recording_of(spoken(2)));
    assert_eq!(r.s.list_renditions(&r.admin, &r.v, &good).unwrap().len(), 1);
}
