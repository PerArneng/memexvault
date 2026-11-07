//! Measures the vector index against a real Postgres on spec 004's real vectors. Every test here is ignored, so a plain `cargo test` shows them as not run.
//! `MEMEX_TEST_DATABASE_URL`, `MEMEX_TEST_CORPUS` (spec 004's `corpus`) and `MEMEX_TEST_VECTORS` (spec 004's `out`).
// These tests read the environment, measure time and print what they measured.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_core::testing::{FakeEmbedder, fake_vector};
use memex_core::vector::{EmbedRequest, content_hash};
use memex_core::{Embedder, Engine, Snapshot, StateRecorded, StepOutcome, Store, VectorIndex, VectorProjector, WriterLease};
use memex_store_postgres::PgStore;
use memex_types::{
    EventDraft, Generation, Header, LexicalFilter, Mutation, ObjectId, Row, RowKey, SegmentRow, Status, Table, VaultId, VectorHealth, VectorQuery,
};
use memex_vector::PgVectors;
use memexvault::edges::{PgVectorIndex, SystemClock};
use memexvault::pg_edge::PostgresStore;
use std::collections::BTreeSet;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

const DIM: usize = 384;
const MODEL: &str = "multilingual-e5-small";

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("not run: {name} isn't set"))
}

fn pg() -> Arc<PgStore> {
    let pg = PgStore::connect(&env("MEMEX_TEST_DATABASE_URL"), 16).expect("connect");
    pg.migrate().expect("migrate");
    Arc::new(pg)
}

fn passages() -> Vec<String> {
    std::fs::read_to_string(format!("{}/passages.txt", env("MEMEX_TEST_CORPUS")))
        .expect("corpus")
        .lines()
        .map(String::from)
        .collect()
}

fn floats(name: &str) -> Vec<f32> {
    std::fs::read(format!("{}/{name}", env("MEMEX_TEST_VECTORS")))
        .expect("vectors")
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

fn truth(name: &str) -> Vec<BTreeSet<u32>> {
    let raw = std::fs::read(format!("{}/{name}", env("MEMEX_TEST_VECTORS"))).expect("truth");
    let ids: Vec<u32> = raw.as_chunks::<4>().0.iter().map(|b| u32::from_le_bytes(*b)).collect();
    ids.chunks(50).map(|c| c.iter().copied().collect()).collect()
}

/// Spec 004's filter bucket, so its ground truth applies.
fn bucket(id: u64) -> u64 {
    let hash = id.wrapping_mul(2654435761) % 4294967296;
    (hash >> 8) % 100
}

fn segment(id: u64, text: &str, source: String) -> SegmentRow {
    SegmentRow {
        header: Header {
            id: ObjectId(format!("seg-{id:07}")),
            creator: "harness".into(),
            status: Status::Active,
            version: 1,
            acl_id: None,
        },
        source_id: ObjectId(source),
        language: None,
        text: text.into(),
        rendition_id: None,
        start: 0,
        end: text.len() as u64,
        heading: None,
        page_from: None,
        page_to: None,
        time: None,
    }
}

fn record(engine: &Engine<PostgresStore, SystemClock>, lease: &WriterLease, mutations: Vec<Mutation>) {
    let draft = EventDraft {
        actor: "harness".into(),
        kind: "state.recorded".into(),
        schema_version: 1,
        payload: serde_json::to_value(StateRecorded { mutations }).expect("json"),
    };
    engine.apply(lease, draft).expect("apply");
}

fn refuse(_: &[EmbedRequest]) -> Result<(), String> {
    Err("everything here has a stored embedding".into())
}

fn settle(projector: &mut VectorProjector, raw: &PostgresStore, index: &PgVectorIndex, embedder: &FakeEmbedder, v: &VaultId) {
    let mut queue = refuse;
    while projector.step(raw, index, embedder, v, &mut queue).expect("step") == StepOutcome::Progress {}
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

/// Spec 004's filters: each bucket is a source, so a filter on `share` sources matches about `share` percent of the vault.
fn share_filter(share: u64) -> LexicalFilter {
    if share >= 100 {
        return LexicalFilter::default();
    }
    LexicalFilter {
        source_ids: (0..share).map(|b| ObjectId(format!("b{b:02}"))).collect(),
        ..Default::default()
    }
}

fn measure(index: &Arc<PgVectorIndex>, v: &VaultId, queries: &Arc<Vec<f32>>, gt: &Arc<Vec<BTreeSet<u32>>>, share: u64, clients: usize) -> (f64, f64, f64) {
    let per = gt.len() / clients;
    let handles: Vec<_> = (0..clients)
        .map(|c| {
            let (index, v, queries, gt) = (index.clone(), v.clone(), queries.clone(), gt.clone());
            std::thread::spawn(move || {
                let (mut found, mut times) = (0usize, vec![]);
                for i in c * per..(c + 1) * per {
                    let query = VectorQuery {
                        model: MODEL.into(),
                        vector: queries[i * DIM..(i + 1) * DIM].to_vec(),
                        filter: share_filter(share),
                        limit: 50,
                    };
                    let t = Instant::now();
                    let hits = index.search(&v, &query).expect("search").hits;
                    times.push(t.elapsed().as_secs_f64() * 1000.0);
                    found += hits
                        .iter()
                        .filter(|h| {
                            h.id.0
                                .strip_prefix("seg-")
                                .and_then(|n| n.parse::<u32>().ok())
                                .is_some_and(|n| gt[i].contains(&n))
                        })
                        .count();
                }
                (found, times)
            })
        })
        .collect();
    let (mut found, mut times) = (0usize, vec![]);
    for h in handles {
        let (f, t) = h.join().unwrap();
        found += f;
        times.extend(t);
    }
    let recall = found as f64 / (per * clients * 50) as f64;
    (recall, pct(&mut times, 0.5), pct(&mut times, 0.95))
}

/// Recall and latency on spec 004's 200,000 vectors, with no embedding at all.
#[test]
#[ignore = "needs Postgres and spec 004's data, and takes minutes"]
fn vector_bench_200k() {
    let pg = pg();
    let v = VaultId("t-vectorbench".into());
    pg.drop_vault(&v).expect("drop");
    let raw = PostgresStore(pg.clone());
    let lease = raw.open_vault(&v).expect("open");
    let engine = Engine::new(PostgresStore(pg.clone()), SystemClock::default());
    let index = Arc::new(PgVectorIndex(PgVectors::new(pg.clone())));
    let embedder = FakeEmbedder::new(MODEL, DIM as u32);
    let texts = passages();
    let vectors = floats("p.f32");
    let total = 200_000usize;
    assert!(texts.len() >= total && vectors.len() >= total * DIM);

    let t = Instant::now();
    let mut distinct = BTreeSet::new();
    for start in (0..total).step_by(1000) {
        let end = (start + 1000).min(total);
        let rows: Vec<(String, Vec<f32>)> = (start..end)
            .map(|i| (content_hash(&texts[i]), vectors[i * DIM..(i + 1) * DIM].to_vec()))
            .collect();
        distinct.extend(rows.iter().map(|r| r.0.clone()));
        index.store_embeddings(&v, MODEL, &rows).expect("store");
        let mutations = (start..end)
            .map(|i| Mutation::Put(Row::Segment(segment(i as u64, &texts[i], format!("b{:02}", bucket(i as u64))))))
            .collect();
        record(&engine, &lease, mutations);
    }
    println!(
        "loaded {total} segments and {} distinct embeddings in {:.0} s",
        distinct.len(),
        t.elapsed().as_secs_f64()
    );
    println!("identical texts shared an embedding: {}", total - distinct.len());
    let mut projector = VectorProjector::default();
    let t = Instant::now();
    settle(&mut projector, &raw, &index, &embedder, &v);
    println!("first build including its graph: {:.1} s", t.elapsed().as_secs_f64());
    let (stored, live) = index.0.disk_bytes(&v).expect("size");
    println!(
        "disk: embedding store {:.0} MB, live generation {:.0} MB, {:.0} bytes per segment in all",
        stored as f64 / 1e6,
        live as f64 / 1e6,
        (stored + live) as f64 / total as f64
    );

    let queries = Arc::new(floats("q.f32"));
    println!("share clients recall p50 p95");
    for (share, spike_recall, spike_p95) in [
        (100, 0.974, 9.6),
        (50, 0.957, 9.9),
        (20, 0.939, 13.6),
        (10, 0.933, 16.0),
        (5, 0.999, 11.4),
        (2, 0.999, 6.0),
        (1, 0.999, 5.2),
    ] {
        let gt = Arc::new(truth(&format!("gt_200000_{share}_all.u32")));
        let (_, cold_p50, cold_p95) = measure(&index, &v, &queries, &gt, share, 8);
        println!("{share}% first pass, caches colder: p50 {cold_p50:.1} p95 {cold_p95:.1}");
        let (recall, p50, p95) = measure(&index, &v, &queries, &gt, share, 8);
        println!(
            "{share}% 8 {recall:.3} {p50:.1} {p95:.1} (spec 004: {spike_recall} at {spike_p95}; recall {:+.1}%, p95 {:+.1}%)",
            (recall / spike_recall - 1.0) * 100.0,
            (p95 / spike_p95 - 1.0) * 100.0
        );
        assert!(recall >= spike_recall * 0.95, "recall must stay within 5%");
    }

    // A rebuild uses stored embeddings only, and the old generation keeps answering.
    let stop = Arc::new(AtomicBool::new(false));
    let failed = Arc::new(AtomicU64::new(0));
    let served = {
        let (index, v, queries, stop, failed) = (index.clone(), v.clone(), queries.clone(), stop.clone(), failed.clone());
        std::thread::spawn(move || {
            let mut times = vec![];
            let mut i = 0usize;
            while !stop.load(Ordering::Relaxed) {
                let query = VectorQuery {
                    model: MODEL.into(),
                    vector: queries[i % 1000 * DIM..(i % 1000 + 1) * DIM].to_vec(),
                    filter: LexicalFilter::default(),
                    limit: 50,
                };
                let t = Instant::now();
                if index.search(&v, &query).map_or(true, |r| r.hits.is_empty()) {
                    failed.fetch_add(1, Ordering::Relaxed);
                }
                times.push(t.elapsed().as_secs_f64() * 1000.0);
                i += 1;
            }
            times
        })
    };
    projector.request_rebuild(&v, embedder.active());
    let t = Instant::now();
    settle(&mut projector, &raw, &index, &embedder, &v);
    println!("rebuild: {:.1} s, {} embedder calls", t.elapsed().as_secs_f64(), embedder.passages_embedded());
    stop.store(true, Ordering::Relaxed);
    let mut times = served.join().unwrap();
    println!(
        "search during the rebuild: n={} p95={:.1} ms max={:.1} ms, {} failed",
        times.len(),
        pct(&mut times, 0.95),
        pct(&mut times, 1.0),
        failed.load(Ordering::Relaxed)
    );
    assert_eq!(embedder.passages_embedded(), 0);
    assert_eq!(failed.load(Ordering::Relaxed), 0);
    let gt = Arc::new(truth("gt_200000_100_all.u32"));
    let (recall, _, p95) = measure(&index, &v, &queries, &gt, 100, 8);
    println!("after the rebuild, no filter: recall {recall:.3}, p95 {p95:.1} ms");
    raw.close_vault(&lease).expect("close");
    // Left in place on request, to look at its query plans by hand.
    if std::env::var("MEMEX_TEST_KEEP").is_err() {
        pg.drop_vault(&v).expect("drop");
    }
}

/// Run by the kill harness, which kills it at a random moment.
#[test]
#[ignore = "only the kill harness runs this"]
fn vector_kill_child() {
    let v = VaultId("t-vectorkill".into());
    let pg = pg();
    let raw = PostgresStore(pg.clone());
    let index = PgVectorIndex(PgVectors::new(pg));
    let embedder = FakeEmbedder::new(MODEL, DIM as u32);
    let mut projector = VectorProjector::default();
    if std::env::var("VEC_REBUILD").is_ok() {
        projector.request_rebuild(&v, embedder.active());
    }
    let mut queue = refuse;
    loop {
        match projector.step(&raw, &index, &embedder, &v, &mut queue) {
            Ok(StepOutcome::Idle) => std::thread::sleep(Duration::from_millis(10)),
            Ok(StepOutcome::Progress) => {}
            Err(e) => panic!("{e}"),
        }
    }
}

#[test]
#[ignore = "needs Postgres, and takes minutes"]
fn vector_kill_200_times() {
    let v = VaultId("t-vectorkill".into());
    let pg = pg();
    pg.drop_vault(&v).expect("drop");
    let raw = PostgresStore(pg.clone());
    let lease = raw.open_vault(&v).expect("open");
    let engine = Engine::new(PostgresStore(pg.clone()), SystemClock::default());
    let index = PgVectorIndex(PgVectors::new(pg.clone()));
    let embedder = FakeEmbedder::new(MODEL, DIM as u32);
    let me = std::env::current_exe().expect("exe");
    let mut next = 0u64;
    let mut add = |n: u64| {
        let (mut rows, mut stored) = (vec![], vec![]);
        for _ in 0..n {
            let text = format!("segment number {next}");
            stored.push((content_hash(&text), fake_vector(&text, DIM as u32)));
            rows.push(Mutation::Put(Row::Segment(segment(next, &text, format!("b{:02}", next % 100)))));
            next += 1;
        }
        index.store_embeddings(&v, MODEL, &stored).expect("store");
        record(&engine, &lease, rows);
    };
    for _ in 0..20 {
        add(1000);
    }
    let mut projector = VectorProjector::default();
    settle(&mut projector, &raw, &index, &embedder, &v);
    drop(projector);
    let (mut ahead, mut missing, mut replayed, mut behind_runs) = (0u64, 0u64, 0u64, 0u64);
    let mut random = 0x9E37_79B9_7F4A_7C15u64;
    let mut draw = |limit: u64| {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        random % limit
    };
    let cycles = 200;
    let t = Instant::now();
    for cycle in 0..cycles {
        let mut command = Command::new(&me);
        command
            .args(["--exact", "vector_kill_child", "--ignored", "--nocapture"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let rebuilding = cycle % 10 == 5;
        if rebuilding {
            command.env("VEC_REBUILD", "1");
        }
        let mut child = command.spawn().expect("spawn");
        // A rebuild gets long enough for some kills to land during its graph build or after its flip.
        let wait = draw(if rebuilding { 8000 } else { 1200 }) + 20;
        std::thread::sleep(Duration::from_millis(wait / 2));
        add(50);
        std::thread::sleep(Duration::from_millis(wait / 2));
        child.kill().expect("kill");
        child.wait().expect("wait");
        add(20);
        let head = raw.snapshot(&v).expect("snapshot").head().expect("head");
        match index.health(&v).expect("health") {
            VectorHealth::Ready { watermark, .. } => {
                if watermark > head {
                    ahead += 1;
                }
                let behind = head.map_or(0, |h| h - watermark.unwrap_or(0));
                replayed += behind;
                if behind > 1 {
                    behind_runs += 1;
                }
            }
            VectorHealth::Missing => missing += 1,
        }
    }
    println!(
        "{cycles} kills in {:.0} s: {ahead} ahead of the log, {missing} missing, {replayed} events to replay in all, {behind_runs} kills left more than the last event behind",
        t.elapsed().as_secs_f64()
    );

    // Catching up gives exactly what the store holds.
    // A killed process's backend can go on holding locks until it notices, so a restart may fail a few times. The real worker retries the same way.
    let t = Instant::now();
    let mut projector = VectorProjector::default();
    let mut queue = refuse;
    let mut retries = 0;
    loop {
        match projector.step(&raw, &index, &embedder, &v, &mut queue) {
            Ok(StepOutcome::Progress) => {}
            Ok(StepOutcome::Idle) => break,
            Err(e) => {
                retries += 1;
                println!("retry {retries}: {e}");
                assert!(retries < 60, "it never recovered");
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
    println!("final catch-up {:.1} s, {retries} retries", t.elapsed().as_secs_f64());
    let snap = raw.snapshot(&v).expect("snapshot");
    let head = snap.head().expect("head");
    assert_eq!(index.watermark(&v, Generation::Live).expect("watermark"), head, "it caught up");
    let (mut stored, mut after) = (BTreeSet::new(), None);
    loop {
        let page = snap.rows(Table::Segment, after.as_ref()).expect("rows");
        let Some(last) = page.last() else { break };
        after = Some(last.header().id.clone());
        stored.extend(page.iter().map(|r| r.header().id.0.clone()));
    }
    // Every stored row has a vector: each one's own text is its nearest neighbour.
    let rows = snap
        .get_many(
            &stored
                .iter()
                .step_by(200)
                .map(|id| RowKey {
                    table: Table::Segment,
                    id: ObjectId(id.clone()),
                })
                .collect::<Vec<_>>(),
        )
        .expect("rows");
    drop(snap);
    let mut checked = 0;
    for row in rows.into_iter().flatten() {
        let Row::Segment(s) = row else { continue };
        let query = VectorQuery {
            model: MODEL.into(),
            vector: fake_vector(&s.text, DIM as u32),
            filter: LexicalFilter::default(),
            limit: 1,
        };
        assert_eq!(index.search(&v, &query).expect("search").hits[0].id, s.header.id);
        checked += 1;
    }
    println!("{checked} sampled rows found themselves, of {} rows", stored.len());
    assert_eq!(index.0.count(&v).expect("count"), stored.len() as u64, "documents against rows");
    println!("{} documents against {} rows", stored.len(), stored.len());
    assert_eq!((ahead, missing), (0, 0));
    assert!(replayed > 0, "no kill left anything to replay, so recovery wasn't tested");
    raw.close_vault(&lease).expect("close");
    pg.drop_vault(&v).expect("drop");
}
