//! Spec 020's measurements on a real Postgres, tantivy and the real models: recall on the golden set, and latency. Every test here is ignored, so a plain `cargo test` shows them as not run.
//! `MEMEX_TEST_DATABASE_URL`, `MEMEX_TEST_DATA`, `MEMEX_TEST_CORPUS` and `MEMEX_TEST_VECTORS` (spec 004), `MEMEX_TEST_E5` and `MEMEX_TEST_RERANKER` (spec 005).
// These tests read the environment, measure time and print what they measured.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_core::rendition::detect_language;
use memex_core::search::{SearchEdgesRef, run_search};
use memex_core::testing::FakeEmbedder;
use memex_core::vector::content_hash;
use memex_core::{Clock, Embedder, Engine, LexicalIndex, LexicalProjector, Snapshot, StateRecorded, StepOutcome, Store, VectorIndex, VectorProjector};
use memex_inference::{OnnxEmbedder, OnnxReranker};
use memex_lexical::TantivyLexical;
use memex_store_postgres::PgStore;
use memex_types::{
    AsOf, AssertionRow, EmbedPurpose, Epistemic, EventDraft, EvidencePackage, EvidenceRow, Expand, Generation, Header, IndexOp, ItemKind, LexicalFilter,
    Mutation, ObjectId, Op, RenditionRow, Row, RowKey, SearchMode, SearchRequest, SegmentRow, SourceRow, Stance, Status, Table, VaultId,
};
use memex_vector::PgVectors;
use memexvault::edges::{PgVectorIndex, SystemClock, TantivyIndex};
use memexvault::pg_edge::PostgresStore;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const DIM: usize = 384;
const MODEL: &str = "multilingual-e5-small";
const BUCKETS: u64 = 100;
const HEADINGS: usize = 200;
const CLAIM_EVERY: usize = 20;

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("not run: {name} isn't set"))
}

fn number(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[derive(Debug, Clone, Deserialize)]
struct Golden {
    id: String,
    question: String,
    language: String,
    answer_language: Option<String>,
    category: String,
    line: Option<usize>,
    sha256: Option<String>,
}

fn golden() -> Vec<Golden> {
    include_str!("fixtures/golden/golden.jsonl")
        .lines()
        .map(|l| serde_json::from_str(l).expect("a golden line"))
        .collect()
}

struct Rig {
    store: PostgresStore,
    lexical: TantivyIndex,
    vector: PgVectorIndex,
    embedder: OnnxEmbedder,
    reranker: OnnxReranker,
    clock: SystemClock,
    pg: Arc<PgStore>,
    v: VaultId,
}

fn rig(vault: &str) -> Rig {
    let pg = PgStore::connect(&env("MEMEX_TEST_DATABASE_URL"), 32).expect("connect");
    pg.migrate().expect("migrate");
    let pg = Arc::new(pg);
    let threads = number("MEMEX_EMBED_THREADS", 4);
    let sessions = number("MEMEX_EMBED_QUERY_SESSIONS", 2);
    println!("embedder: {sessions} query sessions at {threads} threads");
    let reranker_threads = number("MEMEX_RERANK_THREADS", 4);
    let reranker_sessions = number("MEMEX_RERANK_SESSIONS", 2);
    println!("reranker: {reranker_sessions} sessions at {reranker_threads} threads");
    let embedder = OnnxEmbedder::new(env("MEMEX_TEST_E5"), threads, sessions);
    embedder.load().expect("embedder");
    Rig {
        store: PostgresStore(pg.clone()),
        lexical: TantivyIndex(TantivyLexical::new(env("MEMEX_TEST_DATA"))),
        vector: PgVectorIndex(PgVectors::new(pg.clone())),
        embedder,
        reranker: OnnxReranker::new(env("MEMEX_TEST_RERANKER"), reranker_threads, reranker_sessions),
        clock: SystemClock::default(),
        pg,
        v: VaultId(vault.into()),
    }
}

fn header(id: String) -> Header {
    Header {
        id: ObjectId(id),
        creator: "harness".into(),
        status: Status::Active,
        version: 1,
        acl_id: None,
    }
}

/// Spec 004's filter bucket, so sources divide the corpus independently of its content.
fn bucket(line: usize) -> u64 {
    let hash = (line as u64).wrapping_mul(2654435761) % 4294967296;
    (hash >> 8) % BUCKETS
}

fn source_of(b: u64) -> String {
    format!("src-{b:02}")
}

fn rendition_of(b: u64) -> String {
    format!("ren-{b:02}")
}

fn segment_id(line: usize) -> String {
    format!("seg-{line:07}")
}

fn claim_id(line: usize) -> String {
    format!("clm-{line:07}")
}

fn line_of(id: &str) -> Option<usize> {
    id.strip_prefix("seg-").or_else(|| id.strip_prefix("clm-")).and_then(|n| n.parse().ok())
}

fn heading_of(line: usize) -> String {
    format!("h{}", line % HEADINGS)
}

fn segment(line: usize, text: &str) -> SegmentRow {
    let b = bucket(line);
    SegmentRow {
        header: header(segment_id(line)),
        source_id: ObjectId(source_of(b)),
        language: detect_language(text),
        text: text.into(),
        rendition_id: Some(ObjectId(rendition_of(b))),
        start: line as u64 * 1000,
        end: line as u64 * 1000 + text.len() as u64,
        heading: Some(heading_of(line)),
        page_from: Some(0),
        page_to: Some(0),
    }
}

fn record(engine: &Engine<PostgresStore, SystemClock>, lease: &memex_core::WriterLease, mutations: Vec<Mutation>) {
    let draft = EventDraft {
        actor: "harness".into(),
        kind: "state.recorded".into(),
        schema_version: 1,
        payload: serde_json::to_value(StateRecorded { mutations }).expect("json"),
    };
    engine.apply(lease, draft).expect("apply");
}

fn passages() -> Vec<String> {
    std::fs::read_to_string(format!("{}/passages.txt", env("MEMEX_TEST_CORPUS")))
        .expect("corpus")
        .lines()
        .map(String::from)
        .collect()
}

fn passage_vectors() -> Vec<f32> {
    std::fs::read(format!("{}/p.f32", env("MEMEX_TEST_VECTORS")))
        .expect("vectors")
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

/// Sources and renditions make the fence read what it would read on real documents, and headings give the sections something to find. Vectors are spec 004's, of each passage alone. They're filed under the hash of the text the projector will ask for, which includes the heading, a small difference from what production would embed.
fn ensure_loaded(r: &Rig, total: usize) {
    if r.lexical.health(&r.v, false).is_ok_and(|h| matches!(h, memex_types::IndexHealth::Ready { .. }))
        && matches!(r.vector.health(&r.v), Ok(memex_types::VectorHealth::Ready { .. }))
        && r.pg.snapshot(&r.v).is_ok()
    {
        println!("{}: already loaded", r.v);
        return;
    }
    let _ = r.pg.drop_vault(&r.v);
    let _ = r.lexical.delete_vault(&r.v);
    let lease = r.store.open_vault(&r.v).expect("open");
    let engine = Engine::new(PostgresStore(r.pg.clone()), SystemClock::default());
    let texts = passages();
    let vectors = passage_vectors();
    assert!(texts.len() >= total && vectors.len() >= total * DIM);
    let t = Instant::now();
    let mut setup = vec![];
    for b in 0..BUCKETS {
        setup.push(Mutation::Put(Row::Source(SourceRow {
            header: header(source_of(b)),
            sha256: format!("sha-{b}"),
            media_type: if b % 2 == 0 { "application/pdf" } else { "text/plain" }.into(),
            trust_tier: if b % 4 == 0 { "untrusted" } else { "trusted" }.into(),
            recorded_at_ms: 0,
            name: String::new(),
            size: 1,
            logical_id: ObjectId(source_of(b)),
            version_no: 1,
            metadata: serde_json::json!({}),
        })));
        setup.push(Mutation::Put(Row::Rendition(RenditionRow {
            header: header(rendition_of(b)),
            source_id: ObjectId(source_of(b)),
            parser: "harness".into(),
            pipeline_version: 1,
            text_blob: "t".into(),
            words_blob: "w".into(),
            page_count: 1,
            segment_count: 1,
            created_at_ms: 0,
            superseded_by: None,
            pages: vec![],
        })));
    }
    record(&engine, &lease, setup);
    for start in (0..total).step_by(1000) {
        let end = (start + 1000).min(total);
        let rows: Vec<(String, Vec<f32>)> = (start..end)
            .map(|i| {
                (
                    content_hash(&format!("{}\n{}", heading_of(i), texts[i])),
                    vectors[i * DIM..(i + 1) * DIM].to_vec(),
                )
            })
            .collect();
        r.vector.store_embeddings(&r.v, MODEL, &rows).expect("store");
        let mutations = (start..end).map(|i| Mutation::Put(Row::Segment(segment(i, &texts[i])))).collect();
        record(&engine, &lease, mutations);
    }
    println!("{}: {total} segments stored in {:.0} s", r.v, t.elapsed().as_secs_f64());
    project(r);
    println!("{}: indexed {:.0} s after the start", r.v, t.elapsed().as_secs_f64());
    r.store.close_vault(&lease).expect("close");
}

fn head(r: &Rig) -> Option<u64> {
    r.store.snapshot(&r.v).expect("snap").head().expect("head")
}

/// Runs both projectors until they've seen the whole log.
fn project(r: &Rig) {
    let target = head(r);
    let (mut lexical, mut vector) = (LexicalProjector::default(), VectorProjector::default());
    let fake = FakeEmbedder::new(MODEL, DIM as u32);
    let mut refuse =
        |requests: &[memex_core::vector::EmbedRequest]| -> Result<(), String> { Err(format!("{} objects have no stored embedding", requests.len())) };
    loop {
        let a = lexical.step(&r.store, &r.lexical, &r.clock, &r.v).expect("lexical");
        let b = vector.step(&r.store, &r.vector, &fake, &r.v, &mut refuse).expect("vector");
        let marks = r.lexical.watermark(&r.v, Generation::Live).unwrap_or(None) >= target
            && matches!(r.vector.health(&r.v), Ok(memex_types::VectorHealth::Ready { watermark, .. }) if watermark >= target);
        if marks && a == StepOutcome::Idle && b == StepOutcome::Idle {
            return;
        }
        if a == StepOutcome::Idle && b == StepOutcome::Idle {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn run(r: &Rig, req: &SearchRequest) -> Result<EvidencePackage, memex_core::ServerError> {
    run_search(
        &SearchEdgesRef {
            store: &r.store,
            lexical: &r.lexical,
            vector: &r.vector,
            embedder: &r.embedder,
            reranker: &r.reranker,
            clock: &r.clock,
            caches: &memex_core::NoCaches,
        },
        &r.v,
        req,
    )
}

/// A request for the ranking itself: the budget doesn't cut anything.
fn request(text: &str, mode: SearchMode, rerank: bool) -> SearchRequest {
    let mut q = SearchRequest::new(text);
    q.mode = mode;
    q.rerank = rerank;
    q.limit = 20;
    q.budget_tokens = 1_000_000;
    q.debug = true;
    q
}

/// The rank of a golden question's passage, 1 being best. Items are in rank order within a kind, so the debug list is what's read.
fn rank_of(p: &EvidencePackage, line: usize) -> Option<usize> {
    let debug = p.debug.as_ref().expect("debug");
    debug.signals.iter().take(20).position(|s| line_of(&s.id.0) == Some(line)).map(|i| i + 1)
}

#[derive(Default, Clone, Copy)]
struct Score {
    n: usize,
    found: usize,
    ndcg: f64,
    mrr: f64,
}

impl Score {
    fn add(&mut self, rank: Option<usize>) {
        self.n += 1;
        if let Some(r) = rank {
            self.found += 1;
            self.mrr += 1.0 / r as f64;
            if r <= 10 {
                self.ndcg += 1.0 / ((r + 1) as f64).log2();
            }
        }
    }

    fn row(&self) -> String {
        let n = self.n.max(1) as f64;
        format!(
            "{:5.1}% ({:3}/{:3})  nDCG@10 {:.3}  MRR {:.3}",
            self.found as f64 * 100.0 / n,
            self.found,
            self.n,
            self.ndcg / n,
            self.mrr / n
        )
    }
}

#[derive(Debug, Clone, Deserialize)]
struct Relational {
    id: String,
    question: String,
    from_line: usize,
    from_sha256: String,
    line: usize,
    sha256: String,
}

fn relational() -> Vec<Relational> {
    include_str!("fixtures/golden/golden_relational.jsonl")
        .lines()
        .map(|l| serde_json::from_str(l).expect("a relational line"))
        .collect()
}

const OTHER_PAGES: usize = 2000;
const DISTRACTORS: usize = 5;

fn page_title(line: usize) -> String {
    format!("Passage {line}")
}

/// Pages for each fixture passage and for others drawn with a fixed seed, on top of the same 200k segments. Each page holds its passage and a block of links: A to its B, and all of them to distractors. These links are built for the test, so a gain here shows the mechanism works, not that it helps on real wiki content. Returns each block holding a passage, by id.
fn ensure_pages(r: &Rig, fixture: &[Relational]) -> HashMap<String, usize> {
    let texts = passages();
    let vectors = passage_vectors();
    let mut lines: BTreeSet<usize> = fixture.iter().flat_map(|q| [q.from_line, q.line]).collect();
    let mut state = 22u64;
    let mut next = move |n: usize| {
        state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((state >> 33) as usize) % n
    };
    while lines.len() < fixture.len() * 2 + OTHER_PAGES {
        lines.insert(next(texts.len()));
    }
    let all: Vec<usize> = lines.iter().copied().collect();
    let by_line = |line: usize| ObjectId(format!("pg-{line:07}"));
    let snap = r.store.snapshot(&r.v).expect("snap");
    let have = !snap.page_summaries(None, 1).expect("pages").is_empty();
    let mut blocks = HashMap::new();
    if have {
        for chunk in all.chunks(200) {
            let ids: Vec<ObjectId> = chunk.iter().map(|l| by_line(*l)).collect();
            for b in snap.blocks_of(&ids, 10_000).expect("blocks") {
                if b.position == 0
                    && let Some(line) = b.page_id.0.strip_prefix("pg-").and_then(|n| n.parse().ok())
                {
                    blocks.insert(b.header.id.0, line);
                }
            }
        }
        println!("{}: pages already loaded", r.v);
        return blocks;
    }
    drop(snap);
    let t = Instant::now();
    let lease = r.store.open_vault(&r.v).expect("open");
    let engine = Engine::new(PostgresStore(r.pg.clone()), SystemClock::default());
    let mut made = 0u64;
    for chunk in all.chunks(100) {
        let ops: Vec<Op> = chunk
            .iter()
            .map(|line| {
                let mut targets: Vec<usize> = fixture.iter().filter(|q| q.from_line == *line).map(|q| q.line).collect();
                for _ in 0..DISTRACTORS {
                    targets.push(all[next(all.len())]);
                }
                let links: Vec<String> = targets.into_iter().filter(|x| x != line).map(|x| format!("[[{}]]", page_title(x))).collect();
                Op::CreatePage {
                    id: by_line(*line),
                    title: page_title(*line),
                    // One paragraph whatever the passage holds, so its block is exactly the passage.
                    body: format!("{}\n\nSee also {}", texts[*line].replace('\n', " "), links.join(", ")),
                    slug: None,
                    redirects: vec![],
                }
            })
            .collect();
        let snap = r.store.snapshot(&r.v).expect("snap");
        let mut new_id = || {
            made += 1;
            ObjectId(format!("blk-{made:08}"))
        };
        let mutations = memex_core::page::plan_pages(&snap, &ops, "harness", &ObjectId("harness".into()), 0, &mut new_id).expect("plan");
        drop(snap);
        record(&engine, &lease, mutations);
    }
    println!("{}: {} pages stored in {:.0} s", r.v, all.len(), t.elapsed().as_secs_f64());
    // Passages use spec 004's vectors, and titles and link blocks are embedded here, so nothing waits on a job.
    let snap = r.store.snapshot(&r.v).expect("snap");
    let mut rows: Vec<(String, Vec<f32>)> = vec![];
    let mut to_embed: BTreeMap<String, String> = BTreeMap::new();
    for chunk in all.chunks(200) {
        let ids: Vec<ObjectId> = chunk.iter().map(|l| by_line(*l)).collect();
        let found = snap.blocks_of(&ids, 10_000).expect("blocks");
        let block_ids: Vec<ObjectId> = found.iter().map(|b| b.header.id.clone()).collect();
        let mut ops = memex_core::page::block_ops(&snap, &block_ids).expect("docs");
        ops.extend(memex_core::page::page_ops(&snap, &ids).expect("docs"));
        for b in &found {
            if b.position == 0
                && let Some(line) = b.page_id.0.strip_prefix("pg-").and_then(|n| n.parse::<usize>().ok())
            {
                blocks.insert(b.header.id.0.clone(), line);
            }
        }
        for op in ops {
            let IndexOp::Upsert(doc) = op else { continue };
            let hash = content_hash(&doc.text);
            match blocks.get(&doc.id.0).copied() {
                Some(line) if doc.text == texts[line] => rows.push((hash, vectors[line * DIM..(line + 1) * DIM].to_vec())),
                _ => {
                    to_embed.insert(hash, doc.text);
                }
            }
        }
    }
    r.vector.store_embeddings(&r.v, MODEL, &rows).expect("store");
    println!(
        "{}: {} passages reuse stored vectors, embedding {} other texts",
        r.v,
        rows.len(),
        to_embed.len()
    );
    let items: Vec<(String, String)> = to_embed.into_iter().collect();
    for chunk in items.chunks(32) {
        let input: Vec<String> = chunk.iter().map(|x| x.1.clone()).collect();
        let embedded = r.embedder.embed(MODEL, EmbedPurpose::Passage, &input).expect("embed");
        let rows: Vec<(String, Vec<f32>)> = chunk.iter().map(|x| x.0.clone()).zip(embedded).collect();
        r.vector.store_embeddings(&r.v, MODEL, &rows).expect("store");
    }
    drop(snap);
    println!("{}: embedded {:.0} s after the start", r.v, t.elapsed().as_secs_f64());
    project(r);
    r.store.close_vault(&lease).expect("close");
    println!("{}: pages indexed {:.0} s after the start", r.v, t.elapsed().as_secs_f64());
    blocks
}

/// As `rank_of`, counting a passage's page block as the passage too.
fn rank_with_blocks(p: &EvidencePackage, line: usize, blocks: &HashMap<String, usize>) -> Option<usize> {
    let debug = p.debug.as_ref().expect("debug");
    debug
        .signals
        .iter()
        .take(20)
        .position(|s| line_of(&s.id.0) == Some(line) || blocks.get(&s.id.0) == Some(&line))
        .map(|i| i + 1)
}

/// Spec 022's decision on k-hop: both arms measured in one run, on one vault, over the pass rule fixed in the spec before any of this ran. Not asserted, since a miss is an answer and not a failure: it prints what to record.
#[test]
#[ignore = "needs Postgres, spec 004's data and the models"]
fn relational_khop() {
    let total = 200_000;
    let r = rig("t-golden-rel");
    ensure_loaded(&r, total);
    let fixture = relational();
    let texts = passages();
    for q in &fixture {
        assert_eq!(content_hash(&texts[q.from_line]), q.from_sha256, "{}: from_line", q.id);
        assert_eq!(content_hash(&texts[q.line]), q.sha256, "{}: line", q.id);
    }
    let blocks = ensure_pages(&r, &fixture);
    wait_for_ready(&r);
    let plain: Vec<Golden> = golden().into_iter().filter(|g| g.line.is_some()).collect();
    let arm = |text: &str, expand: Expand| {
        let mut q = request(text, SearchMode::Hybrid, false);
        q.expand = expand;
        q
    };
    let arms = [("off", Expand::Off), ("on", Expand::On), ("rule", Expand::Auto)];
    let rule_fires = |text: &str| memex_core::search::plan::is_relational(text);
    println!(
        "\nthe rule fires on {}/{} relational and {}/{} other answerable questions",
        fixture.iter().filter(|q| rule_fires(&q.question)).count(),
        fixture.len(),
        plain.iter().filter(|g| rule_fires(&g.question)).count(),
        plain.len()
    );
    // `rule` here means on where the rule fires: `Auto` follows the shipped constant, which this test mustn't depend on.
    let resolved = |text: &str, e: Expand| match e {
        Expand::Auto if rule_fires(text) => Expand::On,
        Expand::Auto => Expand::Off,
        other => other,
    };
    for (name, e) in arms {
        let (mut rel, mut other) = (Score::default(), Score::default());
        for q in &fixture {
            let p = run(&r, &arm(&q.question, resolved(&q.question, e))).expect("relational");
            let rank = rank_with_blocks(&p, q.line, &blocks);
            rel.add(rank);
            println!("{name} {} rank {rank:?} expanded {}", q.id, p.debug.as_ref().map_or(0, |d| d.expanded));
        }
        for g in &plain {
            let p = run(&r, &arm(&g.question, resolved(&g.question, e))).expect("golden");
            other.add(g.line.and_then(|l| rank_with_blocks(&p, l, &blocks)));
        }
        println!("== k-hop {name}\nrelational {}\ngolden     {}", rel.row(), other.row());
    }
    // Latency in one sequence, alternating which arm goes first, so load falls on both alike. Debug off, as an ordinary search has it.
    let mut times: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    let questions: Vec<&str> = plain
        .iter()
        .map(|g| g.question.as_str())
        .chain(fixture.iter().map(|q| q.question.as_str()))
        .collect();
    for round in 0..number("MEMEX_KHOP_ROUNDS", 5) {
        for (i, text) in questions.iter().enumerate() {
            let mut order = arms.to_vec();
            order.rotate_left((i + round) % arms.len());
            for (name, e) in order {
                let mut q = arm(text, resolved(text, e));
                q.debug = false;
                let t = Instant::now();
                run(&r, &q).expect("latency");
                times.entry(name).or_default().push(t.elapsed().as_secs_f64() * 1000.0);
            }
        }
    }
    let mut p95 = BTreeMap::new();
    for (name, mut ms) in times {
        let value = pct(&mut ms, 0.95);
        println!(
            "hybrid k-hop {name}: p50 {:.1} ms, p95 {value:.1} ms, n={} (busy machine)",
            pct(&mut ms, 0.5),
            ms.len()
        );
        p95.insert(name, value);
    }
    println!("p95 on / off = {:.3}, rule / off = {:.3}", p95["on"] / p95["off"], p95["rule"] / p95["off"]);
}

/// A stable half of the set, for calibrating on one part and checking on the other.
fn half(id: &str) -> usize {
    (content_hash(id).as_bytes()[0] % 2) as usize
}

fn wait_for_ready(r: &Rig) {
    // The cross-encoder loads on first use, and that shouldn't count against anything.
    let _ = run(r, &request("warm up", SearchMode::Hybrid, true));
}

/// Recall, nDCG and MRR per mode, category and language, the misses, and the low-relevance flag.
#[test]
#[ignore = "needs Postgres, spec 004's data and the models"]
fn golden_recall() {
    let total = 200_000;
    let r = rig("t-golden");
    ensure_loaded(&r, total);
    wait_for_ready(&r);
    let set = golden();
    let texts = passages();
    for g in &set {
        if let (Some(line), Some(hash)) = (g.line, &g.sha256) {
            assert!(line < total);
            assert_eq!(&content_hash(&texts[line]), hash, "{}: the corpus isn't the one the set was written on", g.id);
        }
    }
    let answerable = set.iter().filter(|g| g.line.is_some()).count();
    // (half, unanswerable, flagged)
    let flag_stats = |mode: &str, flags: &[(usize, bool, bool)]| {
        for h in 0..2 {
            let part: Vec<_> = flags.iter().filter(|f| f.0 == h).collect();
            let un: Vec<_> = part.iter().filter(|f| f.1).collect();
            let an: Vec<_> = part.iter().filter(|f| !f.1).collect();
            println!(
                "{mode} half {h}: low-relevance on {}/{} unanswerable, and wrongly on {}/{} answerable",
                un.iter().filter(|f| f.2).count(),
                un.len(),
                an.iter().filter(|f| f.2).count(),
                an.len()
            );
        }
    };
    let mut raw = String::new();
    for (name, mode, rerank) in [
        ("hybrid", SearchMode::Hybrid, false),
        ("lexical", SearchMode::Lexical, false),
        ("vector", SearchMode::Vector, false),
        ("hybrid+rerank", SearchMode::Hybrid, true),
    ] {
        let mut scores: BTreeMap<String, Score> = BTreeMap::new();
        let mut misses = vec![];
        let mut flags = vec![];
        for g in &set {
            let p = run(&r, &request(&g.question, mode, rerank)).unwrap_or_else(|e| panic!("{}: {e}", g.id));
            let debug = p.debug.as_ref().expect("debug");
            flags.push((half(&g.id), g.line.is_none(), p.low_relevance));
            raw.push_str(&format!(
                "{name}\t{}\t{}\t{}\t{}\t{:?}\t{:?}\t{:?}\t{:?}\n",
                g.id,
                g.category,
                g.line.is_none(),
                p.low_relevance,
                debug.best_vector_score,
                debug.term_coverage,
                debug.signals.first().and_then(|s| s.rerank_score),
                g.line.and_then(|l| rank_of(&p, l)),
            ));
            let Some(line) = g.line else { continue };
            let rank = rank_of(&p, line);
            let mut count = |key: String| scores.entry(key).or_default().add(rank);
            count("all".into());
            count(format!("language {}", g.language));
            count(format!("category {}", g.category));
            if g.category == "cross_language" {
                count(format!("cross {}->{}", g.language, g.answer_language.as_deref().unwrap_or("?")));
            }
            if rank.is_none() {
                misses.push(format!("{} {} [{}]", g.id, g.category, g.question));
            }
        }
        println!("\n== {name} over {answerable} answerable questions");
        for (key, s) in &scores {
            println!("{key:24} {}", s.row());
        }
        println!("misses ({}):", misses.len());
        for m in &misses {
            println!("  {m}");
        }
        flag_stats(name, &flags);
    }
    if let Ok(path) = std::env::var("MEMEX_TEST_GOLDEN_RAW") {
        std::fs::write(path, raw).expect("raw");
    }

    // What the common-term rule from spec 018 costs: the same questions, with every word sent to the keyword index.
    let kept = TantivyIndex(TantivyLexical::new(env("MEMEX_TEST_DATA")).keeping_common_terms());
    for (name, mode) in [("lexical", SearchMode::Lexical), ("hybrid", SearchMode::Hybrid)] {
        let (mut score, mut ms) = (Score::default(), vec![]);
        for g in set.iter().filter(|g| g.line.is_some()) {
            let t = Instant::now();
            let p = run_search(
                &SearchEdgesRef {
                    store: &r.store,
                    lexical: &kept,
                    vector: &r.vector,
                    embedder: &r.embedder,
                    reranker: &r.reranker,
                    clock: &r.clock,
                    caches: &memex_core::NoCaches,
                },
                &r.v,
                &request(&g.question, mode, false),
            )
            .expect("search");
            ms.push(t.elapsed().as_secs_f64() * 1000.0);
            score.add(g.line.and_then(|l| rank_of(&p, l)));
        }
        println!("\n== {name} with common terms kept\n{}  p95 {:.1} ms alone", score.row(), pct(&mut ms, 0.95));
    }

    // The filtered path: the same questions, inside a tenth of the vault that holds the answer.
    let mut score = Score::default();
    for g in set.iter().filter(|g| g.line.is_some()) {
        let line = g.line.unwrap_or(0);
        let answer = bucket(line);
        let mut q = request(&g.question, SearchMode::Hybrid, false);
        q.filter = LexicalFilter {
            source_ids: (0..10).map(|i| ObjectId(source_of((answer + i) % BUCKETS))).collect(),
            ..Default::default()
        };
        let p = run(&r, &q).expect("filtered");
        assert!(p.items.iter().filter(|i| i.kind == ItemKind::Segment).all(|i| {
            let b = line_of(&i.id.0).map(bucket).unwrap_or(BUCKETS);
            (0..10).any(|d| (answer + d) % BUCKETS == b)
        }));
        score.add(rank_of(&p, line));
    }
    println!("\n== hybrid inside a 10% source filter\n{}", score.row());
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    if v.is_empty() {
        return f64::NAN;
    }
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

/// Runs each request from several clients, and returns each one's time in milliseconds.
fn measure(r: &Arc<Rig>, requests: &Arc<Vec<SearchRequest>>, clients: usize, rounds: usize) -> Vec<f64> {
    let next = Arc::new(AtomicUsize::new(0));
    let total = requests.len() * rounds;
    let threads: Vec<_> = (0..clients)
        .map(|_| {
            let (r, requests, next) = (r.clone(), requests.clone(), next.clone());
            std::thread::spawn(move || {
                let mut out = vec![];
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= total {
                        return out;
                    }
                    let t = Instant::now();
                    run(&r, &requests[i % requests.len()]).expect("search");
                    out.push(t.elapsed().as_secs_f64() * 1000.0);
                }
            })
        })
        .collect();
    threads.into_iter().flat_map(|t| t.join().expect("client")).collect()
}

fn report(label: &str, mut times: Vec<f64>) -> f64 {
    let (p50, p95, p99) = (pct(&mut times, 0.5), pct(&mut times, 0.95), pct(&mut times, 0.99));
    println!("{label:44} n={:5} p50 {p50:8.3} ms  p95 {p95:8.3} ms  p99 {p99:8.3} ms", times.len());
    p95
}

/// What an ordinary caller sends: real questions, the default budget, no debug data.
fn plain_requests() -> Vec<SearchRequest> {
    let mut texts: Vec<String> = golden().into_iter().map(|g| g.question).collect();
    let extra = std::fs::read_to_string(format!("{}/queries.txt", env("MEMEX_TEST_CORPUS"))).expect("queries");
    texts.extend(extra.lines().take(770).map(|l| l.replace('"', " ")));
    texts.into_iter().map(SearchRequest::new).collect()
}

fn latency_at(vault: &str, total: usize) -> Vec<(String, f64)> {
    let rig = rig(vault);
    ensure_loaded(&rig, total);
    wait_for_ready(&rig);
    let r = Arc::new(rig);
    let mut out = vec![];
    let plain = Arc::new(plain_requests());
    println!("\n== {vault}: {total} segments, {} distinct queries", plain.len());
    measure(&r, &plain, 8, 1);
    println!("(one warming pass done)");
    // Sections take one more read, so the measurement means little unless the default request really gets them.
    let sections = plain
        .iter()
        .take(200)
        .filter(|q| run(&r, q).is_ok_and(|p| p.items.iter().any(|i| i.kind == ItemKind::Section)))
        .count();
    println!("{sections} of 200 default requests returned a parent section");
    assert!(sections > 0, "the sections path didn't run");
    let mut note = |label: &str, times: Vec<f64>| out.push((label.to_string(), report(label, times)));
    note("hybrid, 1 client", measure(&r, &plain, 1, 1));
    note("hybrid, 8 clients", measure(&r, &plain, 8, 2));

    let with = |f: &dyn Fn(&mut SearchRequest)| -> Arc<Vec<SearchRequest>> {
        Arc::new(
            plain
                .iter()
                .map(|q| {
                    let mut q = q.clone();
                    f(&mut q);
                    q
                })
                .collect(),
        )
    };
    note("lexical only, 8 clients", measure(&r, &with(&|q| q.mode = SearchMode::Lexical), 8, 2));
    note("vector only, 8 clients", measure(&r, &with(&|q| q.mode = SearchMode::Vector), 8, 2));
    let tenth = with(&|q| {
        q.filter.source_ids = (0..10).map(|b| ObjectId(source_of(b))).collect();
    });
    note("hybrid, 10% source filter, 8 clients", measure(&r, &tenth, 8, 1));
    let tier = with(&|q| q.filter.trust_tiers = vec!["untrusted".into()]);
    note("hybrid, 25% trust tier filter, 8 clients", measure(&r, &tier, 8, 1));

    // Facets and debug data, each against a default pass right beside it.
    let extras = with(&|q| {
        q.facets = true;
        q.debug = true;
    });
    for round in 0..2 {
        note(&format!("default, beside the extras, round {round}"), measure(&r, &plain, 8, 1));
        note(&format!("facets and debug, round {round}"), measure(&r, &extras, 8, 1));
    }

    // A tenth of the queries is plenty, as this is by far the slowest path.
    let some = Arc::new(
        plain
            .iter()
            .take(100)
            .cloned()
            .map(|mut q| {
                q.rerank = true;
                q
            })
            .collect::<Vec<_>>(),
    );
    note("hybrid with rerank, 1 client", measure(&r, &some, 1, 1));
    let some = Arc::new(
        plain
            .iter()
            .take(400)
            .cloned()
            .map(|mut q| {
                q.rerank = true;
                q
            })
            .collect::<Vec<_>>(),
    );
    note("hybrid with rerank, 8 clients", measure(&r, &some, 8, 1));

    let exact_id = "0192f3a1-7c2e-7a10-9b3e-1234567890ab".to_string();
    let lease = r.store.open_vault(&r.v).expect("open");
    let engine = Engine::new(PostgresStore(r.pg.clone()), SystemClock::default());
    if r.store
        .snapshot(&r.v)
        .expect("snap")
        .get(&RowKey {
            table: Table::Segment,
            id: ObjectId(exact_id.clone()),
        })
        .expect("get")
        .is_none()
    {
        let mut row = segment(1, "the row the exact lookups find");
        row.header.id = ObjectId(exact_id.clone());
        record(&engine, &lease, vec![Mutation::Put(Row::Segment(row))]);
    }
    // Its text has no embedding from spec 004, so it borrows a vector, or the projector would queue a job.
    let row_text = format!("{}\nthe row the exact lookups find", heading_of(1));
    let vector = passage_vectors()[DIM..DIM * 2].to_vec();
    r.vector.store_embeddings(&r.v, MODEL, &[(content_hash(&row_text), vector)]).expect("store");
    r.store.close_vault(&lease).expect("close");
    let exact = Arc::new(vec![SearchRequest::new(exact_id)]);
    note("exact id, 1 client", measure(&r, &exact, 1, 2000));
    note("exact id, 8 clients", measure(&r, &exact, 8, 2000));

    let past = r.store.snapshot(&r.v).expect("snap").head().expect("head").expect("events");
    for at in [past, past / 2] {
        let mut q = SearchRequest::new("kungliga biblioteket");
        q.mode = SearchMode::Lexical;
        q.as_of = Some(AsOf::Seq(at));
        q.debug = true;
        let t = Instant::now();
        let p = run(&r, &q).expect("as_of");
        println!(
            "as_of event {at} of {past}: {:.0} ms, {} items, stages {:?}",
            t.elapsed().as_secs_f64() * 1000.0,
            p.items.len(),
            p.debug.map(|d| d.timings_ms)
        );
    }
    let stages: Vec<SearchRequest> = plain
        .iter()
        .take(300)
        .cloned()
        .map(|mut q| {
            q.debug = true;
            q
        })
        .collect();
    let mut by_stage: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for q in &stages {
        for (stage, ms) in run(&r, q).expect("stages").debug.expect("debug").timings_ms {
            by_stage.entry(stage).or_default().push(ms);
        }
    }
    for (stage, mut times) in by_stage {
        println!(
            "stage {stage:12} alone: p50 {:6.2} ms  p95 {:6.2} ms",
            pct(&mut times, 0.5),
            pct(&mut times, 0.95)
        );
    }
    out
}

/// Latency at both sizes, with a projection to 1M, then the same with claims in the vault.
#[test]
#[ignore = "needs Postgres, spec 004's data and the models, and takes many minutes"]
fn latency_50k_200k() {
    let small = latency_at("t-golden-50k", 50_000);
    let large = latency_at("t-golden", 200_000);
    println!("\n== growth from 50k to 200k, and a projection to 1M: fitted to the growth seen, and linear from 200k as a worst case");
    for ((label, a), (_, b)) in small.iter().zip(&large) {
        let exponent = (b / a).ln() / 4f64.ln();
        println!(
            "{label:44} {a:7.1} -> {b:7.1} ms   1M fitted {:7.1} ms, linear {:7.1} ms",
            b * 5f64.powf(exponent),
            b * 5.0
        );
    }
    latency_with_claims();
}

/// The same searches once a vault also holds claims, so a search has evidence to read too.
#[test]
#[ignore = "needs Postgres, spec 004's data and the models"]
fn latency_with_claims() {
    add_claims("t-golden");
    let r = Arc::new(rig("t-golden"));
    project(&r);
    let plain = Arc::new(plain_requests());
    measure(&r, &plain, 8, 1);
    println!("\n== with claims in the vault");
    report("hybrid, 1 client, with claims", measure(&r, &plain, 1, 1));
    report("hybrid, 8 clients, with claims", measure(&r, &plain, 8, 2));
    let (mut with_claim, mut claims) = (0, 0);
    for q in plain.iter().take(200) {
        let p = run(&r, q).expect("search");
        if p.items.iter().any(|i| i.kind == ItemKind::Claim) {
            with_claim += 1;
        }
        claims += p.items.iter().filter(|i| i.kind == ItemKind::Claim).count();
    }
    println!("{with_claim} of 200 queries returned a claim, {claims} claims in all");
    assert!(with_claim > 0, "this run measured nothing unless claims came back");
}

/// One claim per so many passages, citing it, so a search has evidence to read as well.
fn add_claims(vault: &str) {
    let r = rig(vault);
    if r.store
        .snapshot(&r.v)
        .expect("snap")
        .get(&RowKey {
            table: Table::Assertion,
            id: ObjectId(claim_id(0)),
        })
        .expect("get")
        .is_some()
    {
        return;
    }
    let texts = passages();
    let vectors = passage_vectors();
    let lease = r.store.open_vault(&r.v).expect("open");
    let engine = Engine::new(PostgresStore(r.pg.clone()), SystemClock::default());
    let lines: Vec<usize> = (0..200_000).step_by(CLAIM_EVERY).collect();
    for chunk in lines.chunks(500) {
        let (mut rows, mut embeddings) = (vec![], vec![]);
        for &line in chunk {
            let words: Vec<&str> = texts[line].split_whitespace().collect();
            let quote = words[..words.len().min(12)].join(" ");
            let a = AssertionRow {
                header: header(claim_id(line)),
                subject: words[..words.len().min(3)].join(" "),
                predicate: "says".into(),
                value: serde_json::json!(words[3..words.len().min(8)].join(" ")),
                epistemic: Epistemic::Asserted,
                machine: true,
                confidence_permille: 800,
                valid_from_ms: None,
                valid_to_ms: None,
                canonical_hash: format!("hash-{line}"),
                changeset_id: ObjectId("harness".into()),
                superseded_by: None,
            };
            let text = format!("{} {} {} {quote}\n", a.subject, a.predicate, a.value.as_str().unwrap_or(""));
            embeddings.push((content_hash(&text), vectors[line * DIM..(line + 1) * DIM].to_vec()));
            rows.push(Mutation::Put(Row::Assertion(a)));
            rows.push(Mutation::Put(Row::Evidence(EvidenceRow {
                header: header(format!("evi-{line:07}")),
                assertion_id: ObjectId(claim_id(line)),
                segment_id: ObjectId(segment_id(line)),
                quote,
                stance: Stance::Supports,
                anchor: None,
                anchor_broken: false,
            })));
        }
        r.vector.store_embeddings(&r.v, MODEL, &embeddings).expect("store");
        record(&engine, &lease, rows);
    }
    r.store.close_vault(&lease).expect("close");
    println!("{} claims added", lines.len());
}

fn rss_mb() -> f64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().unwrap_or(0.0) / 1024.0
}

/// Hybrid search at 200k under one setting of `MEMEX_EMBED_QUERY_SESSIONS` and `MEMEX_EMBED_THREADS`, to choose the defaults.
#[test]
#[ignore = "needs Postgres, spec 004's data and the models"]
fn latency_sessions() {
    let rig = rig("t-golden");
    ensure_loaded(&rig, 200_000);
    let r = Arc::new(rig);
    let plain = Arc::new(plain_requests());
    measure(&r, &plain, 8, 1);
    let with_mode = |mode| {
        Arc::new(
            plain
                .iter()
                .map(|q| {
                    let mut q = q.clone();
                    q.mode = mode;
                    q
                })
                .collect::<Vec<_>>(),
        )
    };
    report("hybrid, 1 client", measure(&r, &plain, 1, 1));
    report("hybrid, 8 clients", measure(&r, &plain, 8, 2));
    report("vector only, 8 clients", measure(&r, &with_mode(SearchMode::Vector), 8, 1));
    println!("resident memory {:.0} MB", rss_mb());
}

/// Each stage on its own under 8 clients, to see where a search's time goes when they compete.
#[test]
#[ignore = "needs Postgres, spec 004's data and the models"]
fn latency_breakdown() {
    let rig = rig("t-golden");
    ensure_loaded(&rig, 200_000);
    let r = Arc::new(rig);
    let texts: Arc<Vec<String>> = Arc::new(plain_requests().into_iter().map(|q| q.text).collect());
    let timed = |label: &str, work: Arc<dyn Fn(usize) + Send + Sync>| {
        let next = Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (work, next) = (work.clone(), next.clone());
                std::thread::spawn(move || {
                    let mut out = vec![];
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= 2000 {
                            return out;
                        }
                        let t = Instant::now();
                        work(i);
                        out.push(t.elapsed().as_secs_f64() * 1000.0);
                    }
                })
            })
            .collect();
        report(label, threads.into_iter().flat_map(|t| t.join().expect("client")).collect());
    };
    let (a, b) = (r.clone(), texts.clone());
    timed(
        "embedding alone",
        Arc::new(move |i| {
            a.embedder
                .embed(MODEL, memex_types::EmbedPurpose::Query, &[b[i % b.len()].clone()])
                .expect("embed");
        }),
    );
    let vectors = Arc::new(r.embedder.embed(MODEL, memex_types::EmbedPurpose::Query, &texts[..200]).expect("embed"));
    let (a, b) = (r.clone(), vectors);
    timed(
        "vector index alone",
        Arc::new(move |i| {
            let q = memex_types::VectorQuery {
                model: MODEL.into(),
                vector: b[i % b.len()].clone(),
                filter: LexicalFilter::default(),
                limit: 100,
            };
            a.vector.search(&a.v, &q).expect("vector");
        }),
    );
    let (a, b) = (r.clone(), texts.clone());
    timed(
        "keyword index alone",
        Arc::new(move |i| {
            let mut q = SearchRequest::new(b[i % b.len()].clone());
            q.mode = SearchMode::Lexical;
            run(&a, &q).expect("lexical");
        }),
    );
    let a = r.clone();
    timed(
        "a snapshot and one read of 30 rows",
        Arc::new(move |i| {
            let snap = a.store.snapshot(&a.v).expect("snap");
            let keys: Vec<RowKey> = (0..30)
                .map(|n| RowKey {
                    table: Table::Segment,
                    id: ObjectId(segment_id((i * 37 + n) % 200_000)),
                })
                .collect();
            snap.get_many(&keys).expect("read");
        }),
    );
    let a = r.clone();
    timed(
        "a snapshot and head, the smallest round trip",
        Arc::new(move |_| {
            a.store.snapshot(&a.v).expect("snap").head().expect("head");
        }),
    );
}

/// Spec 021, items 2 and 4: replays the golden set through the caches, and compares every answer with one that used no cache. Set the target before looking: cached p95 at most 1 ms.
#[test]
#[ignore = "needs Postgres, spec 004's data and the models"]
fn cache_tiers_replay() {
    use memex_core::search::cached_search;
    use memex_core::{CacheSettings, Caches};
    use memexvault::cache_edge::TieredCaches;
    let total = number("MEMEX_CACHE_SEGMENTS", 200_000);
    let rounds = number("MEMEX_CACHE_ROUNDS", 5);
    let vault = format!("t-golden-{total}");
    let rig = Arc::new(rig(&vault));
    ensure_loaded(&rig, total);
    wait_for_ready(&rig);
    let set = golden();
    let requests: Vec<SearchRequest> = set.iter().map(|g| SearchRequest::new(g.question.clone())).collect();
    println!("{} questions on {total} segments", requests.len());
    let on = 256u64 << 20;
    let mix = |query_bytes, inference_bytes, object_bytes| CacheSettings {
        query_bytes,
        inference_bytes,
        object_bytes,
        disk_bytes: 0,
    };
    let disk = std::env::temp_dir().join(format!("memex-golden-cache-{}", std::process::id()));
    let cached = |c: &TieredCaches, r: &SearchRequest| {
        cached_search(
            &SearchEdgesRef {
                store: &rig.store,
                lexical: &rig.lexical,
                vector: &rig.vector,
                embedder: &rig.embedder,
                reranker: &rig.reranker,
                clock: &rig.clock,
                caches: c,
            },
            &rig.v,
            r,
            || Ok(()),
        )
    };

    // Item 4: the same answers with caches on and off, for every mode, with each tier at 0 in turn. A package is compared whole.
    let with_mode = |q: &SearchRequest, mode: SearchMode, rerank: bool| {
        let mut q = q.clone();
        q.mode = mode;
        q.rerank = rerank;
        q
    };
    let compare = std::env::var("MEMEX_CACHE_REPLAY_ONLY").is_err();
    for (name, mode, rerank) in [
        ("hybrid", SearchMode::Hybrid, false),
        ("lexical", SearchMode::Lexical, false),
        ("vector", SearchMode::Vector, false),
        ("hybrid+rerank", SearchMode::Hybrid, true),
    ]
    .into_iter()
    .filter(|_| compare)
    {
        let plain: Vec<EvidencePackage> = requests.iter().map(|q| run(&rig, &with_mode(q, mode, rerank)).expect("search")).collect();
        for (label, settings) in [
            ("all on", mix(on, on, on)),
            ("query 0", mix(0, on, on)),
            ("inference 0", mix(on, 0, on)),
            ("objects 0", mix(on, on, 0)),
            ("all 0", mix(0, 0, 0)),
        ] {
            // The slowest mode runs only with every tier on and every tier off, to keep the machine free for others.
            if rerank && !matches!(label, "all on" | "all 0") {
                continue;
            }
            let c = TieredCaches::open(settings, &disk).expect("caches");
            let (mut differ, mut compared) = (0, 0);
            for round in 0..2 {
                for (q, want) in requests.iter().zip(&plain) {
                    let got = cached(&c, &with_mode(q, mode, rerank)).expect("search");
                    compared += 1;
                    if &got != want {
                        differ += 1;
                        if differ <= 3 {
                            println!("DIFFERS {name} {label} round {round}: {:?}", q.text);
                        }
                    }
                }
            }
            println!("item 4: {name}, {label}: {differ} of {compared} answers differ");
            assert_eq!(differ, 0, "{name}, {label}");
        }
    }

    // Item 2: a replay. The first pass is cold and the rest should hit. Hits are timed apart from misses by the counters.
    let c = Arc::new(TieredCaches::open(mix(on, on, on), &disk).expect("caches"));
    let stats = |c: &TieredCaches| c.stats().into_iter().find(|t| t.tier == "query").expect("query");
    // No caches at all, right before the same requests with them, as the objects tier adds a store call to a miss.
    let mut plain: Vec<f64> = vec![];
    for q in &requests {
        let t = Instant::now();
        run(&rig, q).expect("search");
        plain.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    report("no caches, 1 client", plain);
    let mut cold: Vec<f64> = vec![];
    for q in &requests {
        let t = Instant::now();
        cached(&c, q).expect("search");
        cold.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    report("cold pass, 1 client", cold);
    let before = stats(&c);
    let mut warm: Vec<f64> = vec![];
    for _ in 0..rounds {
        for q in &requests {
            let t = Instant::now();
            cached(&c, q).expect("search");
            warm.push(t.elapsed().as_secs_f64() * 1000.0);
        }
    }
    let after = stats(&c);
    let (hits, misses) = (after.hits - before.hits, after.misses - before.misses);
    println!(
        "replay, 1 client: {hits} hits, {misses} misses, hit rate {:.2}%",
        100.0 * hits as f64 / (hits + misses).max(1) as f64
    );
    let p95 = report("cached, 1 client", warm);
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                s.spawn(|| {
                    let mut times = vec![];
                    for _ in 0..rounds {
                        for q in &requests {
                            let t = Instant::now();
                            cached(&c, q).expect("search");
                            times.push(t.elapsed().as_secs_f64() * 1000.0);
                        }
                    }
                    times
                })
            })
            .collect();
        let all: Vec<f64> = handles.into_iter().flat_map(|h| h.join().expect("client")).collect();
        report("cached, 8 clients", all);
    });
    let end = stats(&c);
    println!(
        "whole run: {} hits, {} misses, hit rate {:.2}%",
        end.hits,
        end.misses,
        100.0 * end.hits as f64 / (end.hits + end.misses).max(1) as f64
    );
    for t in c.stats() {
        println!("{t:?}");
    }
    println!("RSS {:.0} MB", rss_mb());
    let _ = std::fs::remove_dir_all(&disk);
    println!("target: cached p95 <= 1 ms, measured {p95:.3} ms with 1 client, on a machine shared with other workers");
}

/// Exact-lookup and clock sanity, so a broken rig fails fast before the long runs.
#[test]
#[ignore = "needs Postgres, spec 004's data and the models"]
fn golden_rig_smoke() {
    let r = rig("t-golden-smoke");
    ensure_loaded(&r, 2000);
    let p = run(&r, &request("the", SearchMode::Hybrid, false)).expect("search");
    println!("{} items, {:?}", p.items.len(), p.debug.map(|d| d.timings_ms));
    assert!(r.clock.monotonic() > Duration::ZERO);
    println!("{:?}", r.embedder.active());
    r.pg.drop_vault(&r.v).expect("drop");
    r.lexical.delete_vault(&r.v).expect("drop");
}
