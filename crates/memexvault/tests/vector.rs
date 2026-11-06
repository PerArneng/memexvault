//! The vector index on real pgvector, and the real model. Tests needing Postgres or model files are ignored unless their environment is set.
// These tests read the environment, measure time and print what they measured.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_core::testing::{FakeEmbedder, vector_contract};
use memex_core::vector::content_hash;
use memex_core::{Embedder, StepOutcome, Store, VectorIndex, VectorProjector};
use memex_inference::{E5_SMALL, OnnxEmbedder};
use memex_store_postgres::PgStore;
use memex_types::{
    DocKind, EmbedPurpose, Generation, LexicalFilter, ModelInfo, ObjectId, VaultId, VectorDoc, VectorError, VectorHealth, VectorOp, VectorQuery,
};
use memex_vector::PgVectors;
use memexvault::edges::PgVectorIndex;
use memexvault::pg_edge::PostgresStore;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("not run: {name} isn't set"))
}

fn pg() -> Arc<PgStore> {
    let pg = PgStore::connect(&env("MEMEX_TEST_DATABASE_URL"), 16).expect("connect");
    pg.migrate().expect("migrate");
    Arc::new(pg)
}

#[test]
#[ignore = "needs Postgres"]
fn pg_vector_contract() {
    let pg = pg();
    let raw = PostgresStore(pg.clone());
    let index = PgVectorIndex(PgVectors::new(pg.clone()));
    let opened = std::sync::Mutex::new(vec![]);
    let open = |v: &VaultId| {
        pg.drop_vault(v).expect("drop");
        opened.lock().unwrap().push(raw.open_vault(v).expect("open"));
    };
    vector_contract::run_all(&index, "t-vector-", &open);
    let leases = std::mem::take(&mut *opened.lock().unwrap());
    for lease in leases {
        raw.close_vault(&lease).expect("close");
        pg.drop_vault(&lease.vault).expect("drop");
    }
}

/// Serves all through a move between models, with every answer from one model, and ends with the new one.
#[test]
#[ignore = "needs Postgres"]
fn pg_vector_model_move_keeps_serving() {
    let pg = pg();
    let v = VaultId("t-vector-move".into());
    pg.drop_vault(&v).expect("drop");
    let raw = PostgresStore(pg.clone());
    let lease = raw.open_vault(&v).expect("open");
    let index = Arc::new(PgVectorIndex(PgVectors::new(pg.clone())));
    let (old, new) = (
        ModelInfo {
            name: "model-old".into(),
            dimension: 16,
        },
        ModelInfo {
            name: "model-new".into(),
            dimension: 32,
        },
    );
    let doc = |i: usize, dimension: u32| VectorDoc {
        id: ObjectId(format!("d{i:04}")),
        kind: DocKind::Segment,
        source_ids: vec![ObjectId("s".into())],
        epistemic: None,
        valid_from_ms: None,
        valid_to_ms: None,
        doc_type: None,
        trust_tiers: vec![],
        vector: memex_core::testing::fake_vector(&format!("{i}"), dimension),
    };
    let ops = |dimension: u32| -> Vec<VectorOp> { (0..2000).map(|i| VectorOp::Upsert(doc(i, dimension))).collect() };
    index.begin_build(&v, &old).unwrap();
    index.apply(&v, Generation::Building, &ops(old.dimension), Some(1)).unwrap();
    index.flip(&v).unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let (ok, moved, wrong) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
    let reader = {
        let (index, v, stop, ok, moved, wrong, old, new) = (
            index.clone(),
            v.clone(),
            stop.clone(),
            ok.clone(),
            moved.clone(),
            wrong.clone(),
            old.clone(),
            new.clone(),
        );
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                // What a search does: ask which model is live, embed with it, then query.
                let VectorHealth::Ready { model, .. } = index.health(&v).unwrap() else {
                    panic!("search lost its index")
                };
                assert!(model == old || model == new);
                let query = VectorQuery {
                    vector: memex_core::testing::fake_vector("q", model.dimension),
                    model: model.name,
                    filter: LexicalFilter::default(),
                    limit: 10,
                };
                match index.search(&v, &query) {
                    Ok(r) if r.hits.len() == 10 => ok.fetch_add(1, Ordering::Relaxed),
                    Err(VectorError::ModelMismatch { .. }) => moved.fetch_add(1, Ordering::Relaxed),
                    _ => wrong.fetch_add(1, Ordering::Relaxed),
                };
            }
        })
    };
    index.begin_build(&v, &new).unwrap();
    index.apply(&v, Generation::Building, &ops(new.dimension), Some(2)).unwrap();
    index.flip(&v).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    stop.store(true, Ordering::Relaxed);
    reader.join().unwrap();
    println!(
        "searches during the move: {} answered, {} saw the flip and would retry",
        ok.load(Ordering::Relaxed),
        moved.load(Ordering::Relaxed)
    );
    assert_eq!(wrong.load(Ordering::Relaxed), 0);
    assert!(ok.load(Ordering::Relaxed) > 10);
    assert!(matches!(index.health(&v).unwrap(), VectorHealth::Ready { model, .. } if model == new));
    raw.close_vault(&lease).unwrap();
    pg.drop_vault(&v).unwrap();
}

/// The projector and the real index together, on stored embeddings: nothing is embedded, and every row ends up indexed.
#[test]
#[ignore = "needs Postgres"]
fn pg_vector_rebuild_needs_no_embedder() {
    use memex_core::{Engine, StateRecorded};
    use memex_types::{EventDraft, Header, Mutation, Row, SegmentRow, Status};
    let pg = pg();
    let v = VaultId("t-vector-rebuild".into());
    pg.drop_vault(&v).expect("drop");
    let raw = PostgresStore(pg.clone());
    let lease = raw.open_vault(&v).expect("open");
    let engine = Engine::new(PostgresStore(pg.clone()), memexvault::edges::SystemClock::default());
    let index = PgVectorIndex(PgVectors::new(pg.clone()));
    let embedder = FakeEmbedder::new("m", 16);
    let rows: Vec<Mutation> = (0..500)
        .map(|i| {
            let text = format!("segment text {i}");
            index
                .store_embeddings(&v, "m", &[(content_hash(&text), memex_core::testing::fake_vector(&text, 16))])
                .unwrap();
            let row = SegmentRow {
                header: Header {
                    id: ObjectId(format!("s{i:04}")),
                    creator: "t".into(),
                    status: Status::Active,
                    version: 1,
                    acl_id: None,
                },
                source_id: ObjectId("none".into()),
                language: None,
                end: text.len() as u64,
                text,
                rendition_id: None,
                start: 0,
                heading: None,
                page_from: None,
                page_to: None,
            };
            Mutation::Put(Row::Segment(row))
        })
        .collect();
    let draft = EventDraft {
        actor: "t".into(),
        kind: "state.recorded".into(),
        schema_version: 1,
        payload: serde_json::to_value(StateRecorded { mutations: rows }).unwrap(),
    };
    engine.apply(&lease, draft).unwrap();
    let mut projector = VectorProjector::default();
    let mut refuse = |_: &[memex_core::vector::EmbedRequest]| -> Result<(), String> { Err("nothing should need embedding".into()) };
    let settle = |projector: &mut VectorProjector, refuse: &mut dyn FnMut(&[memex_core::vector::EmbedRequest]) -> Result<(), String>| {
        let mut queue = |r: &[memex_core::vector::EmbedRequest]| refuse(r);
        while projector.step(&raw, &index, &embedder, &v, &mut queue).expect("step") == StepOutcome::Progress {}
    };
    settle(&mut projector, &mut refuse);
    let query = |text: &str| VectorQuery {
        model: "m".into(),
        vector: memex_core::testing::fake_vector(text, 16),
        filter: LexicalFilter::default(),
        limit: 3,
    };
    assert_eq!(index.search(&v, &query("segment text 7")).unwrap().hits[0].id.0, "s0007");
    projector.request_rebuild(&v, embedder.active());
    settle(&mut projector, &mut refuse);
    assert_eq!(index.search(&v, &query("segment text 9")).unwrap().hits[0].id.0, "s0009");
    assert_eq!(embedder.passages_embedded(), 0);
    raw.close_vault(&lease).unwrap();
    pg.drop_vault(&v).unwrap();
}

fn read_f32(path: &str) -> Vec<f32> {
    std::fs::read(path)
        .expect("vectors")
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

/// The real model reproduces spec 005's and spec 004's vectors, and its speed is measured. `MEMEX_TEST_E5` is a folder with `model.onnx` and `tokenizer.json`, and `MEMEX_TEST_VECTORS` and `MEMEX_TEST_CORPUS` are spec 004's.
#[test]
#[ignore = "needs model files"]
fn real_model_matches_the_spike_and_is_fast_enough() {
    let dir = env("MEMEX_TEST_E5");
    let embedder = Arc::new(OnnxEmbedder::new(dir, 4, 2));
    let t = Instant::now();
    embedder.load().expect("load");
    println!("loaded in {:.1} s", t.elapsed().as_secs_f64());
    let corpus = env("MEMEX_TEST_CORPUS");
    let queries: Vec<String> = std::fs::read_to_string(format!("{corpus}/queries.txt"))
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    let expected = read_f32(&format!("{}/q.f32", env("MEMEX_TEST_VECTORS")));
    let mut worst = 1f32;
    let mut one_thread = vec![];
    for (i, q) in queries.iter().take(200).enumerate() {
        let t = Instant::now();
        let got = embedder.embed(E5_SMALL, EmbedPurpose::Query, std::slice::from_ref(q)).unwrap().pop().unwrap();
        one_thread.push(t.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(got.len(), 384);
        let want = &expected[i * 384..(i + 1) * 384];
        worst = worst.min(got.iter().zip(want).map(|(a, b)| a * b).sum());
    }
    one_thread.sort_by(f64::total_cmp);
    println!("lowest cosine against spec 004's query vectors over 200 queries: {worst:.5}");
    println!("query p50 {:.1} ms, p95 {:.1} ms, 1 client", one_thread[100], one_thread[190]);
    assert!(worst > 0.999, "the same text must give the same vector");

    let mut handles = vec![];
    let queries = Arc::new(queries);
    for c in 0..4 {
        let (e, q) = (embedder.clone(), queries.clone());
        handles.push(std::thread::spawn(move || {
            let mut times = vec![];
            for i in 0..200 {
                let t = Instant::now();
                e.embed(E5_SMALL, EmbedPurpose::Query, std::slice::from_ref(&q[c * 200 + i])).unwrap();
                times.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            times
        }));
    }
    let mut times: Vec<f64> = handles.into_iter().flat_map(|h| h.join().unwrap()).collect();
    times.sort_by(f64::total_cmp);
    println!("query p50 {:.1} ms, p95 {:.1} ms, 4 clients on 2 sessions", times[400], times[760]);

    let passages: Vec<String> = std::fs::read_to_string(format!("{corpus}/passages.txt"))
        .unwrap()
        .lines()
        .take(320)
        .map(String::from)
        .collect();
    let t = Instant::now();
    for chunk in passages.chunks(32) {
        embedder.embed(E5_SMALL, EmbedPurpose::Passage, chunk).unwrap();
    }
    println!("{:.0} passages/s, batches of 32, 4 threads", 320.0 / t.elapsed().as_secs_f64());

    // A question while a batch runs: the point of giving passages their own session.
    let background = {
        let (e, p) = (embedder.clone(), passages.clone());
        std::thread::spawn(move || {
            for chunk in p.chunks(32) {
                e.embed(E5_SMALL, EmbedPurpose::Passage, chunk).unwrap();
            }
        })
    };
    let mut during = vec![];
    for q in queries.iter().take(100) {
        let t = Instant::now();
        embedder.embed(E5_SMALL, EmbedPurpose::Query, std::slice::from_ref(q)).unwrap();
        during.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    background.join().unwrap();
    during.sort_by(f64::total_cmp);
    println!("query p50 {:.1} ms, p95 {:.1} ms, during background embedding", during[50], during[95]);
    let rss = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    let kb: u64 = String::from_utf8_lossy(&rss.stdout).trim().parse().unwrap();
    println!("resident memory with 3 sessions: {:.2} GB", kb as f64 / 1_048_576.0);
}
