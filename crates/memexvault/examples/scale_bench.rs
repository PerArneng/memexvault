//! Spec 031 & 033 parameterized scale benchmark loader and micro-harness.
//!
//! Usage:
//!   scale_bench <database-url> <data-dir> <models-dir> <segments> <manifest-out>
//!
//! Generates a reproducible multi-language corpus of `<segments>` segments from the committed
//! public CC BY-SA Wikipedia fixture (`tests/fixtures/extraction/documents.jsonl`), measures
//! group commit throughput, loads the segments + entities + embeddings into Postgres, Tantivy
//! and pgvector, measures a full lexical rebuild from Postgres, and writes a JSON manifest of
//! vault id, admin token, queries and timings for `scale_bench.py` to drive `memexvault serve`.
// Reading args/env, measuring wall time and writing files are this harness's job.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_blob_fs::{FsBlobs, RealFs};
use memex_core::rendition::detect_language;
use memex_core::testing::FakeEmbedder;
use memex_core::vector::content_hash;
use memex_core::{
    Embedder, Engine, LexicalIndex, LexicalProjector, SearchEdges, Server, ServerEdges, Snapshot, StateRecorded, StepOutcome, Store, VectorIndex,
    VectorProjector,
};
use memex_inference::{OnnxEmbedder, OnnxReranker};
use memex_lexical::TantivyLexical;
use memex_store_postgres::PgStore;
use memex_types::{
    EmbedPurpose, EntityRow, EventDraft, Generation, Header, Mutation, ObjectId, RenditionRow, Row, SegmentRow, SourceRow, Status, VaultId, VectorHealth,
};
use memex_vector::PgVectors;
use memexvault::edges::{FsBlobStore, PgVectorIndex, SystemClock, SystemSeed, TantivyIndex};
use memexvault::pg_edge::PostgresStore;
use memexvault::registry_edge::PostgresRegistry;
use memexvault::rendition_job::ChildRasterizer;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

const DIM: usize = 384;
const MODEL: &str = "multilingual-e5-small";
const SOURCES: usize = 100;
const ENTITIES: usize = 500;
const HEADINGS: usize = 200;

struct BenchEdges;

impl ServerEdges for BenchEdges {
    type Registry = PostgresRegistry;
    type Store = PostgresStore;
    type Blobs = FsBlobStore<RealFs>;
    type Clock = SystemClock;
    type Seed = SystemSeed;
    type Rasterizer = ChildRasterizer;
    type Lexical = TantivyIndex;
    type Vector = PgVectorIndex;
    type Embedder = OnnxEmbedder;
    type Reranker = OnnxReranker;
    type Caches = memex_core::NoCaches;
}

#[derive(Deserialize)]
struct FixtureDoc {
    text: String,
}

#[derive(Serialize)]
struct Manifest {
    vault_id: String,
    token: String,
    segments: usize,
    load_total_s: f64,
    embedder_load_s: f64,
    group_commit_events_per_s: f64,
    lexical_rebuild_s: f64,
    exact_queries: Vec<String>,
    lexical_queries: Vec<String>,
    hybrid_queries: Vec<String>,
}

struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        ((self.next_u64() >> 33) as usize) % bound
    }

    fn next_f32_signed(&mut self) -> f32 {
        let u = (self.next_u64() >> 40) as f32 / ((1u32 << 24) as f32);
        u * 2.0 - 1.0
    }
}

fn header(id: String) -> Header {
    Header {
        id: ObjectId(id),
        creator: "scale-bench".into(),
        status: Status::Active,
        version: 1,
        acl_id: None,
    }
}

/// Splits committed CC BY-SA Wikipedia fixture documents into ~200-char base passages.
fn base_passages() -> Vec<String> {
    let raw = include_str!("../tests/fixtures/extraction/documents.jsonl");
    let mut out = Vec::new();
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let doc: FixtureDoc = serde_json::from_str(line).expect("valid fixture jsonl");
        let words: Vec<&str> = doc.text.split_whitespace().collect();
        for chunk in words.chunks(28) {
            if chunk.len() >= 10 {
                out.push(chunk.join(" "));
            }
        }
    }
    assert!(out.len() >= 32, "expected at least 32 base passages, got {}", out.len());
    out
}

/// Deterministic perturbed unit vector derived from a real E5 embedding.
fn perturb_unit(base: &[f32], seed: u64) -> Vec<f32> {
    let mut rng = Rng(seed ^ 0x9E37_79B9_7F4A_7C15);
    let mut v = Vec::with_capacity(DIM);
    let mut norm_sq = 0.0f32;
    for &x in base {
        let y = x + 0.03 * rng.next_f32_signed();
        norm_sq += y * y;
        v.push(y);
    }
    let inv = norm_sq.sqrt().max(1e-6).recip();
    for y in &mut v {
        *y *= inv;
    }
    v
}

fn project_all(store: &PostgresStore, lexical: &TantivyIndex, vector: &PgVectorIndex, clock: &SystemClock, v: &VaultId) {
    let target = store.snapshot(v).expect("snap").head().expect("head");
    let (mut lex_p, mut vec_p) = (LexicalProjector::default(), VectorProjector::default());
    let fake = FakeEmbedder::new(MODEL, DIM as u32);
    let mut refuse =
        |requests: &[memex_core::vector::EmbedRequest]| -> Result<(), String> { Err(format!("{} objects missing stored embedding", requests.len())) };
    loop {
        let a = lex_p.step(store, lexical, clock, v).expect("lexical step");
        let b = vec_p.step(store, vector, &fake, v, &mut refuse).expect("vector step");
        let lex_ok = lexical.watermark(v, Generation::Live).unwrap_or(None) >= target;
        let vec_ok = matches!(vector.health(v), Ok(VectorHealth::Ready { watermark, .. }) if watermark >= target);
        if lex_ok && vec_ok && a == StepOutcome::Idle && b == StepOutcome::Idle {
            return;
        }
        if a == StepOutcome::Idle && b == StepOutcome::Idle {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn measure_group_commit(pg: &Arc<PgStore>) -> f64 {
    let v = VaultId("bench-gc".into());
    let _ = pg.drop_vault(&v);
    let store = PostgresStore(pg.clone());
    let lease = store.open_vault(&v).expect("open gc vault");
    let engine = Engine::new(PostgresStore(pg.clone()), SystemClock::default());

    // 40 batches of 50 events = 2,000 events through Engine::apply_batch (group commit).
    let batches = 40usize;
    let batch_size = 50usize;
    let t0 = Instant::now();
    for b in 0..batches {
        let drafts: Vec<EventDraft> = (0..batch_size)
            .map(|i| {
                let id = format!("gc-{b:04}-{i:03}");
                let seg = SegmentRow {
                    header: header(id),
                    source_id: ObjectId("src-gc".into()),
                    language: Some("en".into()),
                    text: format!("Group commit benchmark event {b}-{i}."),
                    rendition_id: None,
                    start: 0,
                    end: 32,
                    heading: None,
                    page_from: None,
                    page_to: None,
                    time: None,
                };
                EventDraft {
                    actor: "scale-bench".into(),
                    kind: "state.recorded".into(),
                    schema_version: 1,
                    payload: serde_json::to_value(StateRecorded {
                        mutations: vec![Mutation::Put(Row::Segment(seg))],
                    })
                    .expect("json"),
                }
            })
            .collect();
        let results = engine.apply_batch(&lease, drafts);
        for r in results {
            r.expect("group commit event ok");
        }
    }
    let elapsed = t0.elapsed().as_secs_f64();
    store.close_vault(&lease).expect("close gc vault");
    let _ = pg.drop_vault(&v);
    (batches * batch_size) as f64 / elapsed
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 5 {
        return Err("usage: scale_bench <database-url> <data-dir> <models-dir> <segments> <manifest-out>".into());
    }
    let db_url = &args[0];
    let data_dir = PathBuf::from(&args[1]);
    let models_dir = PathBuf::from(&args[2]);
    let segments: usize = args[3].parse().map_err(|e| format!("bad segments: {e}"))?;
    let manifest_out = PathBuf::from(&args[4]);

    let t_emb = Instant::now();
    let embedder = OnnxEmbedder::new(models_dir.join(memexvault::models::EMBED_DIR), 4, 2);
    embedder.load().map_err(|e| e.to_string())?;
    let embedder_load_s = t_emb.elapsed().as_secs_f64();

    let (vault, token) = {
        let pg_setup = Arc::new(PgStore::connect(db_url, 4).map_err(|e| e.to_string())?);
        pg_setup.migrate().map_err(|e| e.to_string())?;
        let server = Server::<BenchEdges>::new(
            PostgresRegistry(pg_setup.clone()),
            PostgresStore(pg_setup.clone()),
            FsBlobStore(FsBlobs::new(&data_dir, RealFs)),
            SystemClock::default(),
            SystemSeed,
            SearchEdges {
                lexical: TantivyIndex(TantivyLexical::new(&data_dir)),
                vector: PgVectorIndex(PgVectors::new(pg_setup.clone())),
                embedder: OnnxEmbedder::new(models_dir.join(memexvault::models::EMBED_DIR), 4, 1),
                reranker: OnnxReranker::new(models_dir.join(memexvault::models::RERANK_DIR), 4, 1),
            },
        );
        let boot = server.bootstrap("bench-admin").map_err(|e| e.to_string())?;
        let caller = server.authenticate(&boot.token).map_err(|e| e.to_string())?;
        let vault = server.create_vault(&caller, &format!("bench-{segments}")).map_err(|e| e.to_string())?.id;
        (vault, boot.token)
    };

    let pg = Arc::new(PgStore::connect(db_url, 16).map_err(|e| e.to_string())?);
    let clock = SystemClock::default();
    let store = PostgresStore(pg.clone());
    let lexical = TantivyIndex(TantivyLexical::new(&data_dir));
    let vector = PgVectorIndex(PgVectors::new(pg.clone()));

    eprintln!("scale_bench: measuring group commit...");
    let group_commit_events_per_s = measure_group_commit(&pg);
    eprintln!("scale_bench: group_commit = {group_commit_events_per_s:.1} events/s");

    let t_load = Instant::now();
    let bases = base_passages();
    eprintln!("scale_bench: embedding {} base passages with real multilingual-e5-small...", bases.len());
    let mut base_vecs: Vec<Vec<f32>> = Vec::with_capacity(bases.len());
    for chunk in bases.chunks(32) {
        let input: Vec<String> = chunk.to_vec();
        let vecs = embedder.embed(MODEL, EmbedPurpose::Passage, &input).map_err(|e| e.to_string())?;
        base_vecs.extend(vecs);
    }

    let lease = store.open_vault(&vault).map_err(|e| e.to_string())?;
    let engine = Engine::new(PostgresStore(pg.clone()), SystemClock::default());

    // Sources, renditions, and 500 Product entities for exact lookup
    let mut setup = Vec::with_capacity(SOURCES * 2 + ENTITIES);
    for b in 0..SOURCES {
        setup.push(Mutation::Put(Row::Source(SourceRow {
            header: header(format!("src-{b:03}")),
            sha256: format!("sha-bench-{b:03}"),
            media_type: if b % 2 == 0 { "application/pdf" } else { "text/plain" }.into(),
            trust_tier: "trusted".into(),
            recorded_at_ms: 1_700_000_000_000,
            name: format!("doc-{b:03}.pdf"),
            size: 4096,
            logical_id: ObjectId(format!("src-{b:03}")),
            version_no: 1,
            metadata: serde_json::json!({}),
        })));
        setup.push(Mutation::Put(Row::Rendition(RenditionRow {
            header: header(format!("ren-{b:03}")),
            source_id: ObjectId(format!("src-{b:03}")),
            parser: "scale-bench".into(),
            pipeline_version: 1,
            text_blob: "t".into(),
            words_blob: "w".into(),
            page_count: 1,
            segment_count: (segments / SOURCES).max(1) as u32,
            created_at_ms: 1_700_000_000_000,
            superseded_by: None,
            pages: vec![],
            media: None,
        })));
    }
    let mut exact_queries = Vec::with_capacity(200);
    let mut entity_ids = Vec::with_capacity(ENTITIES);
    for e in 0..ENTITIES {
        let code = format!("PROD-{e:06}");
        let eid = ObjectId(format!("01900000000070008000{e:012x}"));
        if e < 200 {
            exact_queries.push(eid.0.clone());
        }
        entity_ids.push(eid.clone());
        let row = EntityRow {
            header: header(eid.0),
            type_name: "Product".into(),
            type_version: 1,
            aliases: vec![code.clone()],
            alias_keys: vec![code.to_lowercase()],
            fields: vec![],
            merged_from: vec![],
            redirect_to: None,
            changeset_id: ObjectId("cs-bench".into()),
            updated_at_ms: 1_700_000_000_000,
        };
        setup.push(Mutation::Put(Row::Entity(row)));
    }
    let setup_draft = EventDraft {
        actor: "scale-bench".into(),
        kind: "state.recorded".into(),
        schema_version: 1,
        payload: serde_json::to_value(StateRecorded { mutations: setup }).expect("json"),
    };
    engine.apply(&lease, setup_draft).map_err(|e| e.to_string())?;
    {
        let snap = store.snapshot(&vault).map_err(|e| e.to_string())?;
        let ops = memex_core::entity::entity_ops(&snap, &entity_ids).map_err(|e| e.to_string())?;
        let entity_embed_rows: Vec<(String, Vec<f32>)> = ops
            .into_iter()
            .enumerate()
            .filter_map(|(i, op)| match op {
                memex_types::IndexOp::Upsert(doc) => Some((content_hash(&doc.text), perturb_unit(&base_vecs[i % base_vecs.len()], i as u64))),
                memex_types::IndexOp::Delete(_) => None,
            })
            .collect();
        vector.store_embeddings(&vault, MODEL, &entity_embed_rows).map_err(|e| e.to_string())?;
    }

    // Bulk load N reproducible segments + perturbed real embeddings
    let mut rng = Rng(0x031_5CA1E);
    let mut lexical_queries = Vec::with_capacity(300);
    let mut hybrid_queries = Vec::with_capacity(300);
    let topics = [
        "hydraulic turbine pressure",
        "railway signaling station",
        "coastal municipal archive",
        "geological mineral survey",
        "botanical taxonomy specimen",
        "maritime navigation chart",
        "industrial safety protocol",
        "solar photovoltaic array",
    ];
    for start in (0..segments).step_by(1000) {
        let end = (start + 1000).min(segments);
        let mut embed_rows = Vec::with_capacity(end - start);
        let mut mutations = Vec::with_capacity(end - start);
        for i in start..end {
            let b_idx = rng.next_usize(bases.len());
            let topic = topics[i % topics.len()];
            let heading = format!("Section-{}", i % HEADINGS);
            let text = format!("{} [ref SEG-{i:07} {}]", bases[b_idx], topic);
            let src_idx = i % SOURCES;
            let hash = content_hash(&format!("doc-{src_idx:03}.pdf\n{heading}\n{text}"));
            embed_rows.push((hash, perturb_unit(&base_vecs[b_idx], i as u64 + 1000)));
            mutations.push(Mutation::Put(Row::Segment(SegmentRow {
                header: header(format!("seg-{i:07}")),
                source_id: ObjectId(format!("src-{src_idx:03}")),
                language: detect_language(&text),
                text: text.clone(),
                rendition_id: Some(ObjectId(format!("ren-{src_idx:03}"))),
                start: (i as u64) * 512,
                end: (i as u64) * 512 + text.len() as u64,
                heading: Some(heading),
                page_from: Some(0),
                page_to: Some(0),
                time: None,
            })));
            if lexical_queries.len() < 300 && i % (segments / 300).max(1) == 0 {
                let words: Vec<&str> = bases[b_idx]
                    .split_whitespace()
                    .map(memex_core::lexical::trim_word)
                    .filter(|w| w.len() >= 5)
                    .take(3)
                    .collect();
                if words.len() >= 2 {
                    lexical_queries.push(words.join(" "));
                    hybrid_queries.push(format!("{} {}", words.join(" "), topic));
                }
            }
        }
        vector.store_embeddings(&vault, MODEL, &embed_rows).map_err(|e| e.to_string())?;
        let draft = EventDraft {
            actor: "scale-bench".into(),
            kind: "state.recorded".into(),
            schema_version: 1,
            payload: serde_json::to_value(StateRecorded { mutations }).expect("json"),
        };
        engine.apply(&lease, draft).map_err(|e| e.to_string())?;
    }
    // Pad queries if needed
    let mut k = 0usize;
    while lexical_queries.len() < 300 {
        let t = topics[k % topics.len()];
        lexical_queries.push(format!("{t} SEG-{k:07}"));
        hybrid_queries.push(format!("inspection report {t} {k}"));
        k += 1;
    }

    eprintln!("scale_bench: projecting {segments} segments into Tantivy and pgvector...");
    project_all(&store, &lexical, &vector, &clock, &vault);
    let load_total_s = t_load.elapsed().as_secs_f64();
    eprintln!("scale_bench: loaded & projected in {load_total_s:.1}s");

    // Measure full lexical rebuild from Postgres
    eprintln!("scale_bench: measuring full lexical rebuild from Postgres...");
    lexical.delete_vault(&vault).map_err(|e| e.to_string())?;
    let t_rebuild = Instant::now();
    let target = store.snapshot(&vault).map_err(|e| e.to_string())?.head().map_err(|e| e.to_string())?;
    let mut lex_p = LexicalProjector::default();
    loop {
        let out = lex_p.step(&store, &lexical, &clock, &vault).map_err(|e| e.to_string())?;
        if lexical.watermark(&vault, Generation::Live).unwrap_or(None) >= target && out == StepOutcome::Idle {
            break;
        }
    }
    let lexical_rebuild_s = t_rebuild.elapsed().as_secs_f64();
    eprintln!("scale_bench: lexical rebuild finished in {lexical_rebuild_s:.2}s");

    store.close_vault(&lease).map_err(|e| e.to_string())?;

    let manifest = Manifest {
        vault_id: vault.0,
        token,
        segments,
        load_total_s,
        embedder_load_s,
        group_commit_events_per_s,
        lexical_rebuild_s,
        exact_queries,
        lexical_queries,
        hybrid_queries,
    };
    std::fs::write(&manifest_out, serde_json::to_string_pretty(&manifest).expect("json")).map_err(|e| e.to_string())?;
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("scale_bench: {e}");
            ExitCode::from(1)
        }
    }
}
