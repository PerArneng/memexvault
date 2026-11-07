//! The cache tiers behind a real server's read path, on the real cache crate. Everything but the `pg_` tests runs in memory.
//! `MEMEX_CACHE_LOAD_MB` sets the cap in the load test, by default 8. `MEMEX_TEST_DATABASE_URL` is for the `pg_` tests.
// These tests read the environment and print what they measured.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_core::cache::{embedding_key, object_key};
use memex_core::testing::{FakeEmbedder, FakeReranker, ManualClock, MemBlobStore, MemLexical, MemRegistry, MemStore, MemVector, SeededRandom, mem_search};
use memex_core::{BlobStore, CacheSettings, Caches, Caller, EMBED_JOB, PageImage, RasterError, Rasterizer, Server, ServerEdges, StateRecorded, StepOutcome};
use memex_types::{
    AsOf, BudgetReport, CacheTierStats, EventDraft, EvidenceItem, EvidencePackage, Header, ItemKind, Mutation, ObjectId, PageRow, ParsedDocument, QueryPath,
    Rect, RenditionRow, Row, RowKey, SearchMode, SearchRequest, SegmentRow, SourceRow, Status, Table, VaultId,
};
use memexvault::cache_edge::TieredCaches;
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

/// Counts renders, so a test can tell a cached image from a new one. Only the disk test uses it.
#[derive(Default)]
struct CountingRasterizer;

static RENDERS: AtomicU64 = AtomicU64::new(0);

impl Rasterizer for CountingRasterizer {
    fn renderer(&self) -> String {
        "counting-1".into()
    }

    fn render(&self, _: &[u8], page: u32, dpi: u32, highlights: &[Rect]) -> Result<PageImage, RasterError> {
        RENDERS.fetch_add(1, Ordering::SeqCst);
        Ok(PageImage {
            png: format!("page {page} at {dpi} with {} highlights", highlights.len()).into_bytes(),
            width: 10,
            height: 20,
        })
    }
}

struct Fakes;

impl ServerEdges for Fakes {
    type Registry = MemRegistry;
    type Store = MemStore;
    type Blobs = MemBlobStore<ManualClock>;
    type Clock = ManualClock;
    type Seed = SeededRandom;
    type Rasterizer = CountingRasterizer;
    type Lexical = MemLexical;
    type Vector = MemVector;
    type Embedder = FakeEmbedder;
    type Reranker = FakeReranker;
    type Caches = TieredCaches;
}

const ALL_ON: CacheSettings = CacheSettings {
    query_bytes: 8 << 20,
    inference_bytes: 8 << 20,
    object_bytes: 8 << 20,
    disk_bytes: 8 << 20,
};

/// A fresh folder, gone when dropped.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let d = std::env::temp_dir().join(format!("memex-cache-tiers-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Self(d)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn oid(s: &str) -> ObjectId {
    ObjectId(s.into())
}

fn header(id: &str, version: u32) -> Header {
    Header {
        id: oid(id),
        creator: "t".into(),
        status: Status::Active,
        version,
        acl_id: None,
    }
}

fn source_row(id: &str, sha256: &str) -> SourceRow {
    SourceRow {
        header: header(id, 1),
        sha256: sha256.into(),
        media_type: "application/pdf".into(),
        trust_tier: "trusted".into(),
        recorded_at_ms: 0,
        name: format!("{id}.pdf"),
        size: 1,
        logical_id: oid(id),
        version_no: 1,
        metadata: json!({}),
    }
}

fn rendition_row(id: &str, source: &str, words_blob: &str) -> RenditionRow {
    RenditionRow {
        header: header(id, 1),
        source_id: oid(source),
        parser: "p".into(),
        pipeline_version: 1,
        text_blob: "t".into(),
        words_blob: words_blob.into(),
        page_count: 1,
        segment_count: 1,
        created_at_ms: 0,
        superseded_by: None,
        pages: vec![],
        media: None,
    }
}

fn segment_row(id: &str, version: u32, text: &str) -> SegmentRow {
    SegmentRow {
        header: header(id, version),
        source_id: oid("src"),
        language: None,
        text: text.into(),
        rendition_id: Some(oid("ren")),
        start: 0,
        end: text.len() as u64,
        heading: None,
        page_from: Some(0),
        page_to: Some(0),
        time: None,
    }
}

struct World {
    s: Server<Fakes>,
    c: Caller,
    v: VaultId,
    /// Keeps the disk tier's folder for as long as the server.
    _dir: Dir,
}

fn world_with(name: &str, settings: CacheSettings) -> World {
    let dir = Dir::new(name);
    let caches = TieredCaches::open(settings, dir.0.join("vaults")).unwrap();
    let s = Server::<Fakes>::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(1000),
        SeededRandom::new(5),
        mem_search(),
    )
    .with_caches(caches);
    let c = s.authenticate(&s.bootstrap("root").unwrap().token).unwrap();
    let v = s.create_vault(&c, "v").unwrap().id;
    let w = World { s, c, v, _dir: dir };
    w.record_in(
        &w.v,
        vec![
            Mutation::Put(Row::Source(source_row("src", "sha"))),
            Mutation::Put(Row::Rendition(rendition_row("ren", "src", "w"))),
        ],
    );
    w
}

impl World {
    fn record_in(&self, vault: &VaultId, mutations: Vec<Mutation>) -> u64 {
        let lease = self.s.vault_lease_for_tests(vault).unwrap();
        let payload = serde_json::to_value(StateRecorded { mutations }).unwrap();
        self.s
            .engine()
            .apply(
                &lease,
                EventDraft {
                    actor: "t".into(),
                    kind: "state.recorded".into(),
                    schema_version: 1,
                    payload,
                },
            )
            .unwrap()
            .seq
    }

    fn record(&self, mutations: Vec<Mutation>) -> u64 {
        self.record_in(&self.v, mutations)
    }

    fn put_segment(&self, id: &str, version: u32, text: &str) -> u64 {
        self.record(vec![Mutation::Put(Row::Segment(segment_row(id, version, text)))])
    }

    fn settle_in(&self, vault: &VaultId) {
        for _ in 0..200 {
            self.s.engine().clock().advance(Duration::from_secs(60));
            let mut busy = true;
            while busy {
                busy = self.s.project_lexical(vault).unwrap() == StepOutcome::Progress;
                busy |= self.s.project_vector(vault).unwrap() == StepOutcome::Progress;
            }
            let mut ran = false;
            while let Some(job) = self.s.claim_job(vault, EMBED_JOB, "w").unwrap() {
                self.s.run_embed_job(vault, &job).unwrap();
                self.s.complete_job(vault, &job, vec![]).unwrap();
                ran = true;
            }
            if !ran {
                return;
            }
        }
        panic!("never settled");
    }

    fn settle(&self) {
        self.settle_in(&self.v);
    }

    fn q(&self, text: &str) -> EvidencePackage {
        self.s.search(&self.c, &self.v, &SearchRequest::new(text)).unwrap()
    }

    fn texts(&self, text: &str) -> Vec<String> {
        self.q(text).items.into_iter().map(|i| i.text).collect()
    }

    fn tier(&self, name: &str) -> CacheTierStats {
        self.s.caches().stats().into_iter().find(|t| t.tier == name).unwrap()
    }

    fn store_reads(&self) -> u64 {
        self.s.engine().store().counts().reads.load(Ordering::Relaxed)
    }
}

// --- Verification 1: caps hold under a load of 10x each.

fn sample_package(i: usize, text_bytes: usize) -> EvidencePackage {
    EvidencePackage {
        path: QueryPath::Natural,
        items: (0..4)
            .map(|n| EvidenceItem {
                id: oid(&format!("item-{i}-{n}")),
                kind: ItemKind::Segment,
                text: "x".repeat(text_bytes / 4),
                heading: None,
                language: None,
                epistemic: None,
                trust_tiers: vec![],
                valid_from_ms: None,
                valid_to_ms: None,
                score: Some(1.0),
                estimated_tokens: 1,
                citations: vec![],
                conflicts: vec![],
                page_id: None,
            })
            .collect(),
        low_relevance: false,
        notices: vec![],
        budget: BudgetReport {
            limit: 1,
            used: 1,
            truncated: false,
            smallest_item_needs: None,
        },
        next_cursor: None,
        watermark: None,
        as_of: None,
        degraded: vec![],
        debug: None,
        facets: None,
    }
}

fn rss_mb() -> f64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().unwrap_or(0.0) / 1024.0
}

#[test]
fn caps_hold_under_ten_times_the_load() {
    let cap_mb: u64 = std::env::var("MEMEX_CACHE_LOAD_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(8);
    let cap = cap_mb << 20;
    let limit = cap + cap / 20;
    let dir = Dir::new("load");
    let settings = CacheSettings {
        query_bytes: cap,
        inference_bytes: cap,
        object_bytes: cap,
        disk_bytes: cap,
    };
    let caches = TieredCaches::open(settings, dir.0.join("vaults")).unwrap();
    let rss_before = rss_mb();
    let vault = VaultId("load".into());
    let target = cap * 10;
    let highest = [const { AtomicU64::new(0) }; 4];
    let stop = AtomicBool::new(false);
    let note = |caches: &TieredCaches| {
        for (slot, tier) in highest.iter().zip(caches.stats()) {
            slot.fetch_max(tier.size_bytes, Ordering::Relaxed);
        }
    };
    let written = [const { AtomicU64::new(0) }; 4];
    std::thread::scope(|s| {
        // A reader of the sizes throughout, as a writer's own view would miss what others hold for a moment.
        s.spawn(|| {
            while !stop.load(Ordering::Relaxed) {
                note(&caches);
                std::thread::yield_now();
            }
        });
        let mut writers = vec![];
        for w in 0..4 {
            let (caches, vault, note, written) = (&caches, &vault, &note, &written);
            writers.push(s.spawn(move || {
                let mut i = 0;
                loop {
                    let mut done = true;
                    if written[0].load(Ordering::Relaxed) < target {
                        let p = sample_package(i, 8000);
                        caches.put_package(format!("q-{w}-{i}"), &p);
                        written[0].fetch_add(8000, Ordering::Relaxed);
                        done = false;
                    }
                    if written[1].load(Ordering::Relaxed) < target {
                        caches.put_floats(format!("f-{w}-{i}"), &[0.5; 384]);
                        written[1].fetch_add(384 * 4, Ordering::Relaxed);
                        done = false;
                    }
                    if written[2].load(Ordering::Relaxed) < target {
                        let row = Row::Segment(segment_row(&format!("s-{w}-{i}"), 1, &"y".repeat(2000)));
                        caches.put_row(format!("o-{w}-{i}"), &row);
                        written[2].fetch_add(2000, Ordering::Relaxed);
                        done = false;
                    }
                    if written[3].load(Ordering::Relaxed) < target {
                        caches.put_file(vault, &format!("d-{w}-{i}"), &vec![7u8; 64_000]);
                        written[3].fetch_add(64_000, Ordering::Relaxed);
                        done = false;
                    }
                    note(caches);
                    if done {
                        return;
                    }
                    i += 1;
                }
            }));
        }
        for w in writers {
            w.join().unwrap();
        }
        stop.store(true, Ordering::Relaxed);
    });
    let rss_after = rss_mb();
    let stats = caches.stats();
    for ((tier, high), written) in stats.iter().zip(&highest).zip(&written) {
        let high = high.load(Ordering::Relaxed);
        println!(
            "{}: cap {} bytes, wrote at least {} bytes ({:.1}x), highest size {} ({:.3} of the cap), now {}, {} entries, {} evictions",
            tier.tier,
            tier.cap_bytes,
            written.load(Ordering::Relaxed),
            written.load(Ordering::Relaxed) as f64 / cap as f64,
            high,
            high as f64 / cap as f64,
            tier.size_bytes,
            tier.entries,
            tier.evictions
        );
        assert!(high <= limit, "{}: {high} is over {limit}", tier.tier);
        assert!(written.load(Ordering::Relaxed) >= target, "{}", tier.tier);
        assert!(tier.evictions > 0, "{}", tier.tier);
    }
    println!(
        "RSS before {rss_before:.0} MB, after {rss_after:.0} MB, growth {:.0} MB, for three RAM tiers of {cap_mb} MB each ({} MB)",
        rss_after - rss_before,
        cap_mb * 3
    );
}

// --- Verification 3: stale reads.

fn on_seeded_world(name: &str) -> World {
    let w = world_with(name, ALL_ON);
    w.put_segment("s1", 1, "the red pump weighs twelve kilos");
    w.settle();
    w
}

#[test]
fn a_repeated_search_is_answered_by_the_cache_without_reading_rows() {
    let w = on_seeded_world("hit");
    let first = w.q("red pump");
    assert!(first.items.iter().any(|i| i.text.contains("twelve")));
    assert_eq!(w.tier("query").hits, 0);
    let reads = w.store_reads();
    let second = w.q("red pump");
    assert_eq!(first, second);
    assert_eq!(w.tier("query").hits, 1);
    assert_eq!(w.store_reads() - reads, 1, "only the marks are read");
}

#[test]
fn a_retracted_object_is_gone_from_the_next_read_before_any_index_catches_up() {
    let w = on_seeded_world("retract");
    assert!(w.texts("red pump").iter().any(|t| t.contains("twelve")));
    assert!(w.texts("red pump").iter().any(|t| t.contains("twelve")), "and from the cache");
    assert_eq!(w.tier("query").hits, 1);
    w.record(vec![Mutation::Delete(RowKey {
        table: Table::Segment,
        id: oid("s1"),
    })]);
    // Nothing has been projected, so the watermarks haven't moved. Only the head has.
    assert!(w.q("red pump").items.is_empty());
    w.settle();
    assert!(w.q("red pump").items.is_empty());
}

#[test]
fn an_edited_object_shows_its_new_text_on_the_next_read() {
    let w = on_seeded_world("edit");
    w.q("red pump");
    w.q("red pump");
    w.put_segment("s1", 2, "the red pump weighs nine kilos");
    let texts = w.texts("red pump");
    assert!(texts.iter().any(|t| t.contains("nine")), "{texts:?}");
    assert!(!texts.iter().any(|t| t.contains("twelve")), "{texts:?}");
    assert!(w.tier("objects").misses >= 2, "the new version wasn't in the tier");
}

/// `inverse_of` restores a row with its old version, and the next edit takes the number after it. So (id, version) can name two different contents.
#[test]
fn a_version_number_used_twice_does_not_bring_old_text_back() {
    let w = on_seeded_world("reuse");
    w.put_segment("s1", 2, "the red pump weighs nine kilos");
    assert!(w.texts("red pump").iter().any(|t| t.contains("nine")));
    // A revert, then a new edit that also calls itself version 2.
    w.put_segment("s1", 1, "the red pump weighs twelve kilos");
    w.put_segment("s1", 2, "the red pump weighs five kilos");
    let texts = w.texts("red pump");
    assert!(texts.iter().any(|t| t.contains("five")), "{texts:?}");
    assert!(!texts.iter().any(|t| t.contains("nine")), "{texts:?}");
}

#[test]
fn a_generation_bump_reaches_every_tier() {
    let w = on_seeded_world("bump");
    let mut request = SearchRequest::new("red pump");
    request.rerank = true;
    w.s.search(&w.c, &w.v, &request).unwrap();
    w.q("red pump");
    let before = w.q("red pump");
    let hits = w.tier("query").hits;
    assert!(hits > 0 && w.tier("inference").size_bytes > 0 && w.tier("objects").size_bytes > 0);
    let generation = w.s.bump_cache_generation(&w.v).unwrap();
    assert_eq!(generation, 1);
    let after = w.q("red pump");
    assert_eq!(before, after, "the same data gives the same answer");
    assert_eq!(w.tier("query").hits, hits, "not from the query tier");
    // Everything from before is gone from RAM, not just out of reach. What's there now is what the search just stored.
    assert_eq!(w.tier("query").entries, 1);
    assert_eq!(w.s.bump_cache_generation(&w.v).unwrap(), 2);
}

#[test]
fn erased_text_is_not_served_after_a_bump_even_with_unchanged_marks() {
    let w = on_seeded_world("erase");
    let keys = w.s.engine().store();
    let marks = memex_core::Store::cache_marks(keys, &w.v).unwrap();
    // A cached package under the current marks, like one saved just before erasure removed its text from the tables.
    w.q("red pump");
    assert_eq!(w.tier("query").entries, 1);
    w.s.bump_cache_generation(&w.v).unwrap();
    let after = memex_core::Store::cache_marks(keys, &w.v).unwrap();
    assert_eq!(after.head, marks.head, "the bump alone made the old key unreachable");
    assert_eq!(after.generation, marks.generation + 1);
    assert_eq!(w.tier("query").entries, 0);
}

#[test]
fn a_past_search_is_cached_until_a_bump_and_a_later_event_does_not_touch_it() {
    let w = on_seeded_world("past");
    let at = w.record(vec![Mutation::Put(Row::Segment(segment_row("s2", 1, "a blue pump")))]);
    let mut past = SearchRequest::new("red pump");
    past.as_of = Some(AsOf::Seq(at));
    past.mode = SearchMode::Lexical;
    let first = w.s.search(&w.c, &w.v, &past).unwrap();
    w.record(vec![Mutation::Delete(RowKey {
        table: Table::Segment,
        id: oid("s1"),
    })]);
    let second = w.s.search(&w.c, &w.v, &past).unwrap();
    assert_eq!(first, second);
    assert!(second.items.iter().any(|i| i.text.contains("twelve")), "the past still holds it");
    assert_eq!(w.tier("query").hits, 1);
    w.s.bump_cache_generation(&w.v).unwrap();
    w.s.search(&w.c, &w.v, &past).unwrap();
    assert_eq!(w.tier("query").hits, 1, "after a bump it's run again");
}

#[test]
fn a_past_time_ahead_of_the_log_sees_a_later_event() {
    let w = on_seeded_world("time");
    let mut past = SearchRequest::new("blue");
    past.as_of = Some(AsOf::Time(i64::MAX));
    past.mode = SearchMode::Lexical;
    assert!(w.s.search(&w.c, &w.v, &past).unwrap().items.is_empty());
    w.record(vec![Mutation::Put(Row::Segment(segment_row("s2", 1, "a blue pump")))]);
    assert!(!w.s.search(&w.c, &w.v, &past).unwrap().items.is_empty());
}

#[test]
fn debug_and_min_seq_searches_skip_the_query_tier() {
    let w = on_seeded_world("skip");
    let mut debug = SearchRequest::new("red pump");
    debug.debug = true;
    w.s.search(&w.c, &w.v, &debug).unwrap();
    w.s.search(&w.c, &w.v, &debug).unwrap();
    let mut min_seq = SearchRequest::new("red pump");
    min_seq.min_seq = Some(0);
    w.s.search(&w.c, &w.v, &min_seq).unwrap();
    w.s.search(&w.c, &w.v, &min_seq).unwrap();
    let q = w.tier("query");
    assert_eq!((q.hits, q.entries), (0, 0));
}

#[test]
fn a_degraded_answer_is_not_kept() {
    let w = on_seeded_world("degraded");
    let mut r = SearchRequest::new("red pump");
    r.rerank = true;
    w.s.reranker_for_tests().fail(true);
    let degraded = w.s.search(&w.c, &w.v, &r).unwrap();
    assert_eq!(degraded.degraded, vec!["rerank".to_string()]);
    assert_eq!(w.tier("query").entries, 0);
    w.s.reranker_for_tests().fail(false);
    assert!(w.s.search(&w.c, &w.v, &r).unwrap().degraded.is_empty());
}

#[test]
fn a_store_failure_answers_nothing_from_the_cache() {
    let w = on_seeded_world("deleted");
    w.q("red pump");
    let id = w.v.clone();
    w.s.delete_vault(&w.c, &id).unwrap();
    assert!(matches!(
        w.s.search(&w.c, &id, &SearchRequest::new("red pump")),
        Err(memex_core::ServerError::NotFound)
    ));
    assert_eq!(w.tier("query").entries, 0);
}

#[test]
fn inference_is_not_repeated_for_the_same_query() {
    let w = on_seeded_world("inference");
    let mut r = SearchRequest::new("red pump");
    r.rerank = true;
    r.limit = 5;
    w.s.search(&w.c, &w.v, &r).unwrap();
    let reranks = w.s.reranker_for_tests().calls();
    // A different page size misses the query tier, and reuses the embedding and the scores.
    r.limit = 6;
    w.s.search(&w.c, &w.v, &r).unwrap();
    assert_eq!(w.s.reranker_for_tests().calls(), reranks);
    assert!(w.tier("inference").hits >= 2, "{:?}", w.tier("inference"));
}

// --- Verification 4: every tier at 0, and the same answers either way. The golden set's own run is `cache_tiers_replay` in `tests/golden.rs`.

#[test]
fn every_mix_of_tiers_off_gives_the_same_answers() {
    let (on, off) = (8u64 << 20, 0u64);
    let mixes = [
        (on, on, on, on),
        (off, on, on, on),
        (on, off, on, on),
        (on, on, off, on),
        (on, on, on, off),
        (off, off, off, off),
    ];
    let baseline = |w: &World, r: &SearchRequest| w.s.search(&w.c, &w.v, r).unwrap();
    let mut answers: Vec<Vec<EvidencePackage>> = vec![];
    for (i, (query_bytes, inference_bytes, object_bytes, disk_bytes)) in mixes.into_iter().enumerate() {
        let w = world_with(
            &format!("mix{i}"),
            CacheSettings {
                query_bytes,
                inference_bytes,
                object_bytes,
                disk_bytes,
            },
        );
        for n in 0..8 {
            w.put_segment(&format!("s{n}"), 1, &format!("pump number {n} weighs {n} kilos"));
        }
        w.settle();
        let mut got = vec![];
        for round in 0..3 {
            for text in ["pump", "kilos", "number 3", "nothing like this"] {
                for mode in [SearchMode::Hybrid, SearchMode::Lexical, SearchMode::Vector] {
                    let mut r = SearchRequest::new(text);
                    r.mode = mode;
                    r.rerank = round == 1;
                    got.push(baseline(&w, &r));
                }
            }
        }
        for t in w.s.caches().stats() {
            let was_off = t.cap_bytes == 0;
            assert!(!was_off || (t.hits, t.misses, t.size_bytes, t.entries) == (0, 0, 0, 0), "{t:?}");
        }
        answers.push(got);
    }
    for (i, a) in answers.iter().enumerate() {
        assert_eq!(a, &answers[0], "mix {i} differs");
    }
}

#[test]
fn with_every_tier_off_a_search_makes_no_extra_store_calls() {
    let off = CacheSettings {
        query_bytes: 0,
        inference_bytes: 0,
        object_bytes: 0,
        disk_bytes: 0,
    };
    let w = world_with("off", off);
    w.put_segment("s1", 1, "the red pump");
    w.settle();
    let before = w.store_reads();
    w.q("red pump");
    let one_search = w.store_reads() - before;
    let on = on_seeded_world("on");
    let before = on.store_reads();
    on.q("red pump");
    assert!(one_search < on.store_reads() - before);
}

// --- Verification 5: the disk tier through the server. The tier's own restart, corruption and eviction tests are in `memex-cache`.

#[test]
fn page_images_and_word_tables_come_from_the_disk_until_a_bump() {
    let w = world_with("disk", ALL_ON);
    let doc = ParsedDocument {
        parser: "p".into(),
        pages: vec![memex_types::PageFacts {
            index: 0,
            width: 7200,
            height: 7200,
            rotation: 0,
            class: memex_types::PageClass::Text,
            hidden_text_objects: 0,
        }],
        words: vec![],
    };
    let table_bytes = serde_json::to_vec(&doc).unwrap();
    let pdf = b"%PDF-1.7 not really".to_vec();
    let blobs = w.s.blobs();
    let (source_hash, table_hash) = (
        blobs.put(&w.v, &mut pdf.as_slice()).unwrap().hash.to_string(),
        blobs.put(&w.v, &mut table_bytes.as_slice()).unwrap().hash.to_string(),
    );
    w.record(vec![
        Mutation::Put(Row::Source(source_row("doc", &source_hash))),
        Mutation::Put(Row::Rendition(rendition_row("doc-r", "doc", &table_hash))),
    ]);
    let render = |dpi: u32, highlights: &[Rect]| w.s.page_image(&w.c, &w.v, &oid("doc"), 0, dpi, highlights).unwrap();
    RENDERS.store(0, Ordering::SeqCst);
    let first = render(72, &[]);
    assert_eq!((first.width, first.height), (10, 20));
    assert_eq!(render(72, &[]), first);
    assert_eq!(RENDERS.load(Ordering::SeqCst), 1, "the second came from the disk");
    render(150, &[]);
    assert_eq!(RENDERS.load(Ordering::SeqCst), 2, "another DPI is another image");
    let mark = Rect { x0: 0, y0: 0, x1: 1, y1: 1 };
    assert_ne!(render(72, &[mark]).png, first.png, "highlights are in the key");
    assert_eq!(RENDERS.load(Ordering::SeqCst), 3);
    assert!(w.tier("disk").hits >= 1);
    assert_eq!(w.s.rendition_blob(&w.c, &w.v, &oid("doc-r"), true).unwrap(), table_bytes);
    let holding = w.tier("disk");
    assert!(holding.entries >= 4, "three images and a table: {holding:?}");
    w.s.bump_cache_generation(&w.v).unwrap();
    assert_eq!(w.tier("disk").entries, 0, "erasure can't leave page images behind");
    render(72, &[]);
    assert_eq!(RENDERS.load(Ordering::SeqCst), 4);
}

// --- Verification 6: bad cases.

#[test]
fn invalid_sizes_stop_the_binary_at_startup() {
    for (name, value) in [
        ("MEMEX_CACHE_QUERY_BYTES", "-1"),
        ("MEMEX_CACHE_INFERENCE_BYTES", "lots"),
        ("MEMEX_CACHE_OBJECT_BYTES", "12"),
        ("MEMEX_CACHE_DISK_BYTES", "99999999999999999999"),
    ] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_memexvault"))
            .arg("serve")
            .env_remove("MEMEX_DATABASE_URL")
            .env(name, value)
            .output()
            .expect("the binary runs");
        let said = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{name}={value}: {said}");
        assert!(said.contains(name), "{name}={value}: {said}");
    }
}

#[test]
fn the_same_query_in_two_vaults_gets_each_vaults_own_answer() {
    let w = on_seeded_world("two-vaults");
    let other = w.s.create_vault(&w.c, "other").unwrap().id;
    w.record_in(
        &other,
        vec![
            Mutation::Put(Row::Source(source_row("src", "sha"))),
            Mutation::Put(Row::Rendition(rendition_row("ren", "src", "w"))),
            Mutation::Put(Row::Segment(segment_row("s1", 1, "the red pump belongs to the other vault"))),
        ],
    );
    w.settle_in(&other);
    for _ in 0..3 {
        let here = w.texts("red pump");
        assert!(here.iter().all(|t| !t.contains("other vault")), "{here:?}");
        assert!(here.iter().any(|t| t.contains("twelve")), "{here:?}");
        let there: Vec<String> =
            w.s.search(&w.c, &other, &SearchRequest::new("red pump"))
                .unwrap()
                .items
                .into_iter()
                .map(|i| i.text)
                .collect();
        assert!(there.iter().all(|t| !t.contains("twelve")), "{there:?}");
        assert!(there.iter().any(|t| t.contains("other vault")), "{there:?}");
    }
    assert!(w.tier("query").hits >= 4);
    // Deleting one vault leaves the other's entries.
    w.s.delete_vault(&w.c, &other).unwrap();
    assert!(w.tier("query").entries >= 1);
    assert!(w.texts("red pump").iter().any(|t| t.contains("twelve")));
}

#[test]
fn a_caller_without_access_gets_nothing_from_the_cache() {
    let w = on_seeded_world("access");
    w.q("red pump");
    let outsider = w.s.create_vault(&w.c, "elsewhere").unwrap().id;
    let principal = w.s.create_principal(&w.c, "someone", memex_types::PrincipalKind::Agent, false).unwrap();
    let token =
        w.s.create_token(
            &w.c,
            memex_core::NewToken {
                principal_id: Some(principal.id),
                name: "t".into(),
                grants: vec![memex_types::Grant {
                    vault: outsider,
                    scope: memex_types::Scope::Read,
                }],
                expires_at_ms: None,
            },
        )
        .unwrap();
    let them = w.s.authenticate(&token.token).unwrap();
    assert!(w.s.search(&them, &w.v, &SearchRequest::new("red pump")).is_err());
    assert_eq!(w.tier("query").hits, 0);
    assert!(matches!(w.s.cache_stats(&them), Err(memex_core::ServerError::Forbidden)));
    assert_eq!(w.s.cache_stats(&w.c).unwrap().len(), 4);
}

#[test]
fn a_key_of_one_vault_finds_nothing_for_another_in_any_tier() {
    let dir = Dir::new("keys");
    let c = TieredCaches::open(ALL_ON, dir.0.join("vaults")).unwrap();
    let (a, b) = (VaultId("a".into()), VaultId("b".into()));
    c.put_floats(embedding_key(&a, 0, "m", "text"), &[1.0]);
    assert_eq!(c.get_floats(&embedding_key(&a, 0, "m", "text")), Some(vec![1.0]));
    assert_eq!(c.get_floats(&embedding_key(&b, 0, "m", "text")), None);
    let row = Row::Page(PageRow {
        header: header("p", 1),
        title: "t".into(),
        body: "secret".into(),
        updated_at_ms: 0,
        slug: "t".into(),
        redirects: vec![],
    });
    c.put_row(object_key(&a, 0, Table::Page, &oid("p"), "stamp"), &row);
    assert_eq!(c.get_row(&object_key(&a, 0, Table::Page, &oid("p"), "stamp")), Some(row));
    assert_eq!(c.get_row(&object_key(&b, 0, Table::Page, &oid("p"), "stamp")), None);
    c.put_file(&a, "same-key", b"a's");
    assert_eq!(c.get_file(&b, "same-key"), None);
    c.purge_vault(&a);
    assert_eq!(c.get_floats(&embedding_key(&a, 0, "m", "text")), None);
    assert_eq!(c.get_file(&a, "same-key"), None);
}

// --- On a real Postgres.

mod pg {
    use super::*;
    use memex_core::{Snapshot, Store};
    use memex_store_postgres::PgStore;
    use memexvault::pg_edge::PostgresStore;
    use std::sync::Arc;

    fn store() -> PostgresStore {
        let url = std::env::var("MEMEX_TEST_DATABASE_URL").expect("not run: MEMEX_TEST_DATABASE_URL isn't set");
        let pg = PgStore::connect(&url, 4).expect("connect");
        pg.migrate().expect("migrate");
        PostgresStore(Arc::new(pg))
    }

    fn fresh(s: &PostgresStore, name: &str) -> (VaultId, memex_core::WriterLease) {
        let v = VaultId(format!("t-cache-{name}"));
        let _ = s.0.drop_vault(&v);
        let lease = s.open_vault(&v).expect("open");
        (v, lease)
    }

    #[test]
    #[ignore = "needs Postgres"]
    fn pg_cache_marks_follow_the_log_and_a_bump_lasts() {
        let s = store();
        let (v, lease) = fresh(&s, "marks");
        assert_eq!(s.cache_marks(&v).unwrap(), memex_core::CacheMarks { head: None, generation: 0 });
        let engine = memex_core::Engine::new(PostgresStore(s.0.clone()), memexvault::edges::SystemClock::default());
        let payload = serde_json::to_value(StateRecorded {
            mutations: vec![Mutation::Put(Row::Source(source_row("src", "sha")))],
        })
        .unwrap();
        let seq = engine
            .apply(
                &lease,
                EventDraft {
                    actor: "t".into(),
                    kind: "state.recorded".into(),
                    schema_version: 1,
                    payload,
                },
            )
            .unwrap()
            .seq;
        assert_eq!(s.cache_marks(&v).unwrap().head, Some(seq));
        assert_eq!(s.bump_cache_generation(&lease).unwrap(), 1);
        assert_eq!(s.bump_cache_generation(&lease).unwrap(), 2);
        // A new connection pool, as after a restart.
        let again = store();
        assert_eq!(again.cache_marks(&v).unwrap().generation, 2);
        s.close_vault(&lease).unwrap();
        s.0.drop_vault(&v).unwrap();
        assert!(s.cache_marks(&VaultId("t-cache-none".into())).is_err());
    }

    #[test]
    #[ignore = "needs Postgres"]
    fn pg_cache_a_bump_needs_the_lease() {
        let s = store();
        let (v, lease) = fresh(&s, "lease");
        let stale = memex_core::WriterLease {
            vault: v.clone(),
            token: lease.token + 1,
        };
        assert_eq!(s.bump_cache_generation(&stale), Err(memex_core::StoreError::LeaseLost));
        assert_eq!(s.cache_marks(&v).unwrap().generation, 0);
        s.close_vault(&lease).unwrap();
        s.0.drop_vault(&v).unwrap();
    }

    /// A cached search reads this mark on every hit, so it has to move with every write.
    #[test]
    #[ignore = "needs Postgres"]
    fn pg_cache_the_vector_mark_follows_writes() {
        use memex_core::VectorIndex;
        use memex_types::{Generation, ModelInfo};
        let s = store();
        let (v, lease) = fresh(&s, "vector-mark");
        let index = memexvault::edges::PgVectorIndex(memex_vector::PgVectors::new(s.0.clone()));
        assert_eq!(index.live_mark(&v).unwrap(), None, "no generation yet");
        let model = ModelInfo {
            name: "m".into(),
            dimension: 3,
        };
        index.begin_build(&v, &model).unwrap();
        index.apply(&v, Generation::Building, &[], Some(3)).unwrap();
        assert_eq!(index.live_mark(&v).unwrap(), None, "still building");
        index.flip(&v).unwrap();
        assert_eq!(index.live_mark(&v).unwrap(), Some(3));
        index.apply(&v, Generation::Live, &[], Some(5)).unwrap();
        assert_eq!(index.live_mark(&v).unwrap(), Some(5));
        assert_eq!(index.live_mark(&v).unwrap(), Some(5), "and from memory");
        s.close_vault(&lease).unwrap();
        s.0.drop_vault(&v).unwrap();
    }

    /// The stamp is what makes a row in the tier safe, so it has to follow every change, and agree with what the rows say.
    #[test]
    #[ignore = "needs Postgres"]
    fn pg_cache_row_stamps_follow_contents() {
        let s = store();
        let (v, lease) = fresh(&s, "stamps");
        let engine = memex_core::Engine::new(PostgresStore(s.0.clone()), memexvault::edges::SystemClock::default());
        let apply = |mutations: Vec<Mutation>| {
            engine
                .apply(
                    &lease,
                    EventDraft {
                        actor: "t".into(),
                        kind: "state.recorded".into(),
                        schema_version: 1,
                        payload: serde_json::to_value(StateRecorded { mutations }).unwrap(),
                    },
                )
                .unwrap()
        };
        apply(vec![
            Mutation::Put(Row::Source(source_row("src", "sha"))),
            Mutation::Put(Row::Rendition(rendition_row("ren", "src", "w"))),
            Mutation::Put(Row::Segment(segment_row("s1", 1, "first"))),
        ]);
        let keys = [
            RowKey {
                table: Table::Segment,
                id: oid("s1"),
            },
            RowKey {
                table: Table::Segment,
                id: oid("missing"),
            },
            RowKey {
                table: Table::Source,
                id: oid("src"),
            },
        ];
        let stamps = || s.snapshot(&v).unwrap().row_stamps(&keys).unwrap();
        let one = stamps();
        assert!(one[0].is_some() && one[2].is_some());
        assert_eq!(one[1], None);
        assert_eq!(one, stamps(), "the same row has the same stamp");
        // The same version number with other text must not have the same stamp.
        apply(vec![Mutation::Put(Row::Segment(segment_row("s1", 1, "second")))]);
        let two = stamps();
        assert_ne!(one[0], two[0]);
        assert_eq!(one[2], two[2], "an untouched row keeps its stamp");
        apply(vec![Mutation::Delete(keys[0].clone())]);
        assert_eq!(stamps()[0], None);
        s.close_vault(&lease).unwrap();
        s.0.drop_vault(&v).unwrap();
    }
}

// Spec 026: a retirement goes through events like anything else, so a cached search can't outlive it.
#[test]
fn a_retired_claim_can_not_be_served_from_a_cached_search_and_returns_on_a_revert() {
    let w = world_with("retired", ALL_ON);
    w.put_segment("seg", 1, "zebrafinken sjunger om natten");
    let claim = memex_types::AssertionRow {
        header: memex_types::Header {
            id: oid("claim"),
            creator: "t".into(),
            status: memex_types::Status::Active,
            version: 1,
            acl_id: None,
        },
        subject: "zebrafinken".into(),
        predicate: "sjunger".into(),
        value: serde_json::json!("om natten"),
        epistemic: memex_types::Epistemic::Asserted,
        machine: false,
        confidence_permille: 1000,
        valid_from_ms: None,
        valid_to_ms: None,
        canonical_hash: "h".into(),
        changeset_id: oid("none"),
        superseded_by: None,
    };
    w.record(vec![Mutation::Put(Row::Assertion(claim))]);
    w.settle();
    let claims = |w: &World| w.q("zebrafinken").items.iter().filter(|i| i.kind == memex_types::ItemKind::Claim).count();
    assert_eq!(claims(&w), 1);
    let hits = w.tier("query").hits;
    assert_eq!(claims(&w), 1);
    assert_eq!(w.tier("query").hits, hits + 1, "the second search came from the cache");
    let cs =
        w.s.propose(
            &w.c,
            &w.v,
            memex_core::Proposal {
                ops: vec![memex_types::Op::RetractAssertion {
                    target: oid("claim"),
                    base_version: 1,
                    counts_against: false,
                }],
                rationale: "wrong".into(),
                idempotency_key: "retire".into(),
            },
            false,
        )
        .unwrap()
        .changeset
        .unwrap();
    assert_eq!(claims(&w), 1, "proposing changes nothing");
    let retired = w.s.approve(&w.c, &w.v, &cs.header.id, None).unwrap();
    assert_eq!(claims(&w), 0, "gone at once, before any index has moved");
    w.settle();
    assert_eq!(claims(&w), 0);
    w.s.revert(&w.c, &w.v, &retired.header.id, "it was right".into()).unwrap();
    w.settle();
    assert_eq!(claims(&w), 1);
}
