//! Measures the keyword index against a real Postgres and real paragraphs. Every test here is ignored, so a plain `cargo test` shows them as not run.
//! `MEMEX_TEST_DATABASE_URL`, `MEMEX_TEST_DATA` (a folder for the index) and `MEMEX_TEST_CORPUS` (holding `passages.txt` and `queries.txt`) must be set.
// These tests read the environment, measure time and print what they measured.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_core::lexical::{QueryError, parse_query};
use memex_core::rendition::detect_language;
use memex_core::{Engine, LexicalIndex, LexicalProjector, Snapshot, StateRecorded, StepOutcome, Store, WriterLease};
use memex_lexical::TantivyLexical;
use memex_store_postgres::PgStore;
use memex_types::{
    EventDraft, Generation, Header, IndexHealth, LexicalFilter, LexicalQuery, Mutation, ObjectId, QueryTerm, Row, RowKey, SegmentRow, Status, Table, VaultId,
};
use memexvault::edges::{SystemClock, TantivyIndex};
use memexvault::pg_edge::PostgresStore;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("not run: {name} isn't set"))
}

fn store() -> PostgresStore {
    let pg = PgStore::connect(&env("MEMEX_TEST_DATABASE_URL"), 8).expect("connect");
    pg.migrate().expect("migrate");
    PostgresStore(Arc::new(pg))
}

fn data_dir(name: &str) -> PathBuf {
    PathBuf::from(env("MEMEX_TEST_DATA")).join(name)
}

fn lines(name: &str) -> Vec<String> {
    let path = Path::new(&env("MEMEX_TEST_CORPUS")).join(name);
    std::fs::read_to_string(path).expect("corpus").lines().map(String::from).collect()
}

fn pct(mut v: Vec<f64>, p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

fn report(label: &str, samples: Vec<f64>) {
    println!(
        "{label}: n={} p50={:.2} p95={:.2} p99={:.2} max={:.2} ms",
        samples.len(),
        pct(samples.clone(), 0.5),
        pct(samples.clone(), 0.95),
        pct(samples.clone(), 0.99),
        pct(samples, 1.0)
    );
}

fn segment(id: &str, text: &str, version: u32) -> SegmentRow {
    SegmentRow {
        header: Header {
            id: ObjectId(id.into()),
            creator: "harness".into(),
            status: Status::Active,
            version,
            acl_id: None,
        },
        source_id: ObjectId(format!("src-{:03}", text.len() % 500)),
        language: detect_language(text),
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

fn record(engine: &Engine<PostgresStore, SystemClock>, lease: &WriterLease, mutations: Vec<Mutation>) -> u64 {
    let draft = EventDraft {
        actor: "harness".into(),
        kind: "state.recorded".into(),
        schema_version: 1,
        payload: serde_json::to_value(StateRecorded { mutations }).expect("json"),
    };
    engine.apply(lease, draft).expect("apply").seq
}

/// Keyword-style queries from each sentence: its three longest words.
fn keyword_queries(sentences: &[String]) -> Vec<String> {
    sentences
        .iter()
        .map(|s| {
            let mut words: Vec<&str> = s.split_whitespace().map(memex_core::lexical::trim_word).filter(|w| !w.is_empty()).collect();
            words.sort_by_key(|w| std::cmp::Reverse(w.len()));
            words.truncate(3);
            words.join(" ")
        })
        .collect()
}

fn sentence_queries(sentences: &[String]) -> Vec<String> {
    sentences
        .iter()
        .map(|s| s.replace('"', " ").split_whitespace().take(32).collect::<Vec<_>>().join(" "))
        .collect()
}

fn terms(text: &str) -> Result<Vec<QueryTerm>, QueryError> {
    parse_query(text)
}

fn query(text: &str, limit: usize) -> LexicalQuery {
    LexicalQuery {
        terms: terms(text).unwrap_or_else(|e| panic!("{text}: {e}")),
        filter: LexicalFilter::default(),
        limit,
    }
}

/// Runs the queries from several threads, and returns each one's time in milliseconds.
fn measure(index: &Arc<TantivyIndex>, v: &VaultId, queries: &[String], clients: usize, limit: usize) -> Vec<f64> {
    let queries = Arc::new(queries.to_vec());
    let next = Arc::new(AtomicU64::new(0));
    let threads: Vec<_> = (0..clients)
        .map(|_| {
            let (index, v, queries, next) = (index.clone(), v.clone(), queries.clone(), next.clone());
            std::thread::spawn(move || {
                let mut out = vec![];
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed) as usize;
                    let Some(text) = queries.get(i) else { return out };
                    let q = query(text, limit);
                    let t = Instant::now();
                    index.search(&v, &q).expect("search");
                    out.push(t.elapsed().as_secs_f64() * 1000.0);
                }
            })
        })
        .collect();
    threads.into_iter().flat_map(|t| t.join().expect("client")).collect()
}

fn folder_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir).map_or(0, |entries| {
        entries
            .flatten()
            .map(|e| {
                let p = e.path();
                if p.is_dir() { folder_bytes(&p) } else { e.metadata().map_or(0, |m| m.len()) }
            })
            .sum()
    })
}

/// Follows the log in a thread as the server does, until told to stop.
fn follow(
    store: Arc<PostgresStore>,
    index: Arc<TantivyIndex>,
    projector: Arc<Mutex<LexicalProjector>>,
    v: VaultId,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let clock = SystemClock::default();
        while !stop.load(Ordering::Relaxed) {
            let outcome = projector.lock().expect("lock").step(store.as_ref(), index.as_ref(), &clock, &v);
            match outcome {
                Ok(StepOutcome::Progress) => {}
                Ok(StepOutcome::Idle) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => {
                    println!("step failed: {e}");
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
    })
}

fn wait_caught_up(store: &PostgresStore, index: &TantivyIndex, v: &VaultId) {
    let deadline = Instant::now() + Duration::from_secs(600);
    let head = store.snapshot(v).expect("snapshot").head().expect("head");
    while index.watermark(v, Generation::Live).ok().flatten() < head {
        assert!(Instant::now() < deadline, "the index never caught up");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn scale_run(scale: usize) {
    let v = VaultId(format!("t-lexbench-{scale}"));
    let raw = Arc::new(store());
    raw.0.drop_vault(&v).expect("drop");
    let dir = data_dir(&format!("bench-{scale}"));
    let _ = std::fs::remove_dir_all(&dir);
    let passages = lines("passages.txt");
    let sentences = lines("queries.txt");
    let index = Arc::new(TantivyIndex(TantivyLexical::new(&dir)));
    let lease = raw.open_vault(&v).expect("lease");
    let engine = Engine::new(PostgresStore(raw.0.clone()), SystemClock::default());
    let projector = Arc::new(Mutex::new(LexicalProjector::default()));
    let stop = Arc::new(AtomicBool::new(false));
    let thread = follow(raw.clone(), index.clone(), projector.clone(), v.clone(), stop.clone());
    while !matches!(index.health(&v, false), Ok(IndexHealth::Ready { .. })) {
        std::thread::sleep(Duration::from_millis(10));
    }

    // Sustained ingest with edits and deletes, a probe for how soon each batch is searchable, and searches running alongside.
    let keywords = keyword_queries(&sentences);
    let during: Arc<Mutex<Vec<f64>>> = Arc::new(Mutex::new(vec![]));
    let ingesting = Arc::new(AtomicBool::new(true));
    let searcher = {
        let (index, v, keywords, during, ingesting) = (index.clone(), v.clone(), keywords.clone(), during.clone(), ingesting.clone());
        std::thread::spawn(move || {
            let mut i = 0;
            while ingesting.load(Ordering::Relaxed) {
                let q = query(&keywords[i % keywords.len()], 100);
                let t = Instant::now();
                index.search(&v, &q).expect("search");
                during.lock().expect("lock").push(t.elapsed().as_secs_f64() * 1000.0);
                i += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
        })
    };
    let batch = 100usize;
    let rate_per_s = 1000.0;
    let started = Instant::now();
    let (mut lags, mut edited, mut deleted) = (vec![], BTreeSet::new(), BTreeSet::new());
    for (b, chunk) in passages[..scale].chunks(batch).enumerate() {
        let due = started + Duration::from_secs_f64((b * batch) as f64 / rate_per_s);
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
        let base = b * batch;
        let mut mutations: Vec<Mutation> = chunk
            .iter()
            .enumerate()
            .map(|(i, text)| Mutation::Put(Row::Segment(segment(&format!("seg-{:07}", base + i), text, 1))))
            .collect();
        // An identifier a person might search for, in one paragraph out of a hundred.
        let identifier = format!("ORA-{b:05}");
        let marker = format!("markerword{b}");
        mutations.push(Mutation::Put(Row::Segment(segment(
            &format!("ident-{b:05}"),
            &format!("Failed with {identifier}: see {marker} for details"),
            1,
        ))));
        if b > 10 && b % 10 == 0 {
            let edit = (b - 7) * batch;
            mutations.push(Mutation::Put(Row::Segment(segment(
                &format!("seg-{edit:07}"),
                &format!("{} edited", passages[edit]),
                2,
            ))));
            edited.insert(edit);
            let gone = (b - 5) * batch + 1;
            mutations.push(Mutation::Delete(RowKey {
                table: Table::Segment,
                id: ObjectId(format!("seg-{gone:07}")),
            }));
            deleted.insert(gone);
        }
        record(&engine, &lease, mutations);
        let committed = Instant::now();
        if b % 10 == 0 {
            let q = query(&marker, 1);
            loop {
                if !index.search(&v, &q).expect("search").hits.is_empty() {
                    lags.push(committed.elapsed().as_secs_f64() * 1000.0);
                    break;
                }
                assert!(committed.elapsed() < Duration::from_secs(60), "a batch never became searchable");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
    let ingest_s = started.elapsed().as_secs_f64();
    ingesting.store(false, Ordering::Relaxed);
    searcher.join().expect("searcher");
    wait_caught_up(&raw, &index, &v);
    println!(
        "== scale {scale}: ingested {scale} segments in {ingest_s:.0} s, {:.0}/s, {} edited, {} deleted",
        scale as f64 / ingest_s,
        edited.len(),
        deleted.len()
    );
    report("lag from commit to searchable", lags);
    let during = std::mem::take(&mut *during.lock().expect("lock"));
    report("keyword search during ingest, limit 100", during);

    // Idle latency.
    let sentence = sentence_queries(&sentences);
    let identifiers: Vec<String> = (0..1000).map(|i| format!("ORA-{:05}", i % (scale / batch))).collect();
    let question: Vec<String> = sentence.iter().map(|s| s.split_whitespace().take(10).collect::<Vec<_>>().join(" ")).collect();
    for (label, queries) in [
        ("keyword", &keywords),
        ("question", &question),
        ("sentence", &sentence),
        ("identifier", &identifiers),
    ] {
        for clients in [1, 8] {
            for limit in [20, 100] {
                measure(&index, &v, queries, clients, limit); // warm
                report(
                    &format!("{label} clients={clients} limit={limit}"),
                    measure(&index, &v, queries, clients, limit),
                );
            }
        }
    }
    let hit = index.search(&v, &query("ORA-00003", 10)).expect("search").hits;
    assert_eq!(hit.len(), 1, "an identifier matches exactly");
    assert_eq!(hit[0].id.0, "ident-00003");
    println!("disk after following the log: {:.0} MB", folder_bytes(&dir) as f64 / 1e6);

    // What the incremental index answers, in full.
    let compare = keywords.clone();
    assert_eq!(compare.len(), 1000);
    let full = |q: &str| -> Vec<String> { index.search(&v, &query(q, 400_000)).expect("search").hits.into_iter().map(|h| h.id.0).collect() };
    let t = Instant::now();
    let followed: Vec<Vec<String>> = compare.iter().map(|q| full(q)).collect();
    let followed_sentences: Vec<Vec<String>> = sentence.iter().map(|q| full(q)).collect();
    println!("read the full hit sets of {} queries in {:.0} s", compare.len(), t.elapsed().as_secs_f64());

    // A rebuild while searches keep running on the old generation.
    let (rebuild_running, old_samples) = (Arc::new(AtomicBool::new(true)), Arc::new(Mutex::new(vec![])));
    let during_rebuild = {
        let (index, v, keywords, samples, running) = (index.clone(), v.clone(), keywords.clone(), old_samples.clone(), rebuild_running.clone());
        std::thread::spawn(move || {
            let (mut i, mut failures) = (0, 0u64);
            while running.load(Ordering::Relaxed) {
                let q = query(&keywords[i % keywords.len()], 100);
                let t = Instant::now();
                match index.search(&v, &q) {
                    Ok(_) => samples.lock().expect("lock").push(t.elapsed().as_secs_f64() * 1000.0),
                    Err(_) => failures += 1,
                }
                i += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
            failures
        })
    };
    let before_gen = std::fs::read_to_string(index.0.vault_dir(&v).join("CURRENT")).expect("pointer");
    let t = Instant::now();
    projector.lock().expect("lock").request_rebuild(&v);
    while std::fs::read_to_string(index.0.vault_dir(&v).join("CURRENT")).expect("pointer") == before_gen {
        assert!(t.elapsed() < Duration::from_secs(1800), "the rebuild never finished");
        std::thread::sleep(Duration::from_millis(100));
    }
    let rebuild_s = t.elapsed().as_secs_f64();
    rebuild_running.store(false, Ordering::Relaxed);
    let failures = during_rebuild.join().expect("searcher");
    println!("rebuild: {rebuild_s:.1} s, with {failures} failed searches");
    let samples = std::mem::take(&mut *old_samples.lock().expect("lock"));
    report("keyword search during rebuild, limit 100", samples);
    println!("disk with both generations: {:.0} MB", folder_bytes(&dir) as f64 / 1e6);

    // The same answers from the rebuilt index.
    let mut different = 0;
    let (mut same_top, mut top_total) = (0usize, 0usize);
    let mut total_hits = 0usize;
    for (q, before) in compare.iter().zip(&followed) {
        let after = full(q);
        total_hits += after.len();
        let (a, b): (BTreeSet<&String>, BTreeSet<&String>) = (before.iter().collect(), after.iter().collect());
        if a != b {
            different += 1;
            if different < 5 {
                println!("DIFFERENT on {q}: {} before, {} after", a.len(), b.len());
            }
        }
        let tops: BTreeSet<&String> = before.iter().take(20).collect();
        same_top += after.iter().take(20).filter(|id| tops.contains(id)).count();
        top_total += tops.len();
    }
    println!(
        "incremental against rebuilt: {different} of {} queries differ, {total_hits} hits compared; top-20 overlap {:.2}%",
        compare.len(),
        same_top as f64 * 100.0 / top_total.max(1) as f64
    );
    let sentence_diffs = sentence
        .iter()
        .zip(&followed_sentences)
        .filter(|(q, before)| before.iter().collect::<BTreeSet<_>>() != full(q).iter().collect::<BTreeSet<_>>())
        .count();
    println!("incremental against rebuilt, whole sentences: {sentence_diffs} of {} differ", sentence.len());
    assert_eq!(different, 0, "a rebuilt index must match an incremental one");
    report("keyword after rebuild, clients=1, limit=100", measure(&index, &v, &keywords, 1, 100));

    // Restarting.
    stop.store(true, Ordering::Relaxed);
    thread.join().expect("follower");
    drop(index);
    let t = Instant::now();
    let reopened = TantivyLexical::new(&dir);
    let health = reopened.health(&v, false).expect("health");
    println!("reopen {:.1} ms -> {health:?}", t.elapsed().as_secs_f64() * 1000.0);
    let t = Instant::now();
    let deep = reopened.health(&v, true).expect("health");
    println!("deep check {:.0} ms -> {deep:?}", t.elapsed().as_secs_f64() * 1000.0);
    let _ = raw.close_vault(&lease);
}

#[test]
#[ignore = "needs Postgres and the corpus, and takes minutes"]
fn lexical_bench_50k() {
    scale_run(50_000);
}

#[test]
#[ignore = "needs Postgres and the corpus, and takes minutes"]
fn lexical_bench_200k() {
    scale_run(200_000);
}

/// Run by the kill harness, which kills it at a random moment. It rebuilds if asked, and otherwise follows the log.
#[test]
#[ignore = "only the kill harness runs this"]
fn lexical_kill_child() {
    let v = VaultId("t-lexkill".into());
    let raw = store();
    let index = TantivyIndex(TantivyLexical::new(data_dir("kill")));
    let clock = SystemClock::default();
    let mut projector = LexicalProjector::default();
    if std::env::var("LEX_REBUILD").is_ok() {
        projector.request_rebuild(&v);
    }
    loop {
        match projector.step(&raw, &index, &clock, &v) {
            Ok(StepOutcome::Idle) => std::thread::sleep(Duration::from_millis(10)),
            Ok(StepOutcome::Progress) => {}
            Err(e) => panic!("{e}"),
        }
    }
}

#[test]
#[ignore = "needs Postgres and the corpus, and takes minutes"]
fn lexical_kill_200_times() {
    let v = VaultId("t-lexkill".into());
    let raw = store();
    raw.0.drop_vault(&v).expect("drop");
    let dir = data_dir("kill");
    let _ = std::fs::remove_dir_all(&dir);
    let passages = lines("passages.txt");
    let lease = raw.open_vault(&v).expect("lease");
    let engine = Engine::new(PostgresStore(raw.0.clone()), SystemClock::default());
    let me = std::env::current_exe().expect("exe");
    let mut next = 0usize;
    let mut add = |n: usize| {
        let rows: Vec<Mutation> = (0..n)
            .map(|_| {
                let text = format!("harnessword {}", passages[next % passages.len()]);
                let id = format!("seg-{next:07}");
                next += 1;
                Mutation::Put(Row::Segment(segment(&id, &text, 1)))
            })
            .collect();
        record(&engine, &lease, rows);
    };
    for _ in 0..20 {
        add(1000);
    }
    // A first build outlasts the shortest kills, so without this every kill would land before any index exists.
    {
        let index = TantivyIndex(TantivyLexical::new(&dir));
        let (clock, mut projector) = (SystemClock::default(), LexicalProjector::default());
        let head = raw.snapshot(&v).expect("snapshot").head().expect("head");
        while index.watermark(&v, Generation::Live).ok().flatten() < head {
            if projector.step(&raw, &index, &clock, &v).expect("first build") == StepOutcome::Idle {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
    let (mut ahead, mut unusable, mut behind_events, mut caught_up_runs, mut corrupt, mut missing) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
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
            .args(["--exact", "lexical_kill_child", "--ignored", "--nocapture"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let rebuilding = cycle % 10 == 5;
        if rebuilding {
            command.env("LEX_REBUILD", "1");
        }
        let mut child = command.spawn().expect("spawn");
        // A rebuild gets long enough for some kills to land near or after its flip.
        let wait = draw(if rebuilding { 8000 } else { 1200 }) + 20;
        // Writes arrive while it runs, at a random point in its life.
        std::thread::sleep(Duration::from_millis(wait / 2));
        add(50);
        std::thread::sleep(Duration::from_millis(wait / 2));
        child.kill().expect("kill");
        child.wait().expect("wait");
        add(20);
        // A fresh open, as a restart would see it.
        let index = TantivyLexical::new(&dir);
        let head = raw.snapshot(&v).expect("snapshot").head().expect("head");
        match index.health(&v, true) {
            Ok(IndexHealth::Ready { watermark }) => {
                if watermark > head {
                    ahead += 1;
                }
                behind_events += head.map_or(0, |h| h - watermark.unwrap_or(0));
                if watermark == head.map(|h| h - 1) {
                    caught_up_runs += 1;
                }
            }
            Ok(IndexHealth::Missing) => missing += 1,
            Ok(IndexHealth::Corrupt(detail)) => {
                println!("cycle {cycle}: corrupt: {detail}");
                corrupt += 1;
            }
            Ok(other) => println!("cycle {cycle}: {other:?}"),
            Err(e) => {
                println!("cycle {cycle}: {e}");
                unusable += 1;
            }
        }
    }
    println!(
        "{cycles} kills in {:.0} s: {ahead} ahead of the log, {corrupt} corrupt, {unusable} unusable, {missing} missing, {caught_up_runs} had caught up, {behind_events} events replayed in all",
        t.elapsed().as_secs_f64()
    );

    // Catching up gives exactly what the store holds.
    let index = TantivyIndex(TantivyLexical::new(&dir));
    let clock = SystemClock::default();
    let mut projector = LexicalProjector::default();
    let (mut idle, t) = (0, Instant::now());
    while idle < 40 {
        idle = if projector.step(&raw, &index, &clock, &v).expect("step") == StepOutcome::Idle {
            idle + 1
        } else {
            0
        };
        std::thread::sleep(Duration::from_millis(25));
    }
    println!("final catch-up {:.1} s", t.elapsed().as_secs_f64());
    let head = raw.snapshot(&v).expect("snapshot").head().expect("head");
    assert_eq!(index.watermark(&v, Generation::Live).expect("watermark"), head, "it caught up");
    let found: BTreeSet<String> = index
        .search(&v, &query("harnessword", 1_000_000))
        .expect("search")
        .hits
        .into_iter()
        .map(|h| h.id.0)
        .collect();
    let snap = raw.snapshot(&v).expect("snapshot");
    let (mut stored, mut after) = (BTreeSet::new(), None);
    loop {
        let page = snap.rows(Table::Segment, after.as_ref()).expect("rows");
        let Some(last) = page.last() else { break };
        after = Some(last.header().id.clone());
        stored.extend(page.iter().map(|r| r.header().id.0.clone()));
    }
    println!("{} documents against {} rows", found.len(), stored.len());
    assert_eq!(found, stored);
    // An open snapshot holds a lock that would block the drop below for ever.
    drop(snap);
    assert_eq!((ahead, corrupt, unusable, missing), (0, 0, 0, 0));
    assert!(behind_events > 0, "no kill left anything to replay, so recovery wasn't tested");
    let _ = raw.close_vault(&lease);
    raw.0.drop_vault(&v).expect("drop");
    let _ = std::fs::remove_dir_all(&dir);
}
