//! The keyword index's projector and search, in memory.
use memex_core::testing::{ManualClock, MemBlobStore, MemLexical, MemRegistry, MemStore, SeededRandom, lexical_contract};
use memex_core::{CommitPolicy, Engine, LexicalIndex, LexicalProjector, Server, ServerEdges, ServerError, StateRecorded, StepOutcome, Store, WriterLease};
use memex_types::{
    AssertionRow, DocKind, Epistemic, EventDraft, EvidenceRow, Generation, Header, IndexDoc, IndexHealth, IndexOp, LexicalError, LexicalFilter, LexicalQuery,
    Mutation, ObjectId, QueryTerm, RenditionRow, Row, RowKey, SegmentRow, Stance, Status, Table, VaultId,
};
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::atomic::Ordering;
use std::time::Duration;

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

fn segment(id: &str, rendition: Option<&str>, language: Option<&str>, text: &str) -> SegmentRow {
    SegmentRow {
        header: header(id, 1),
        source_id: oid("src"),
        language: language.map(String::from),
        text: text.into(),
        rendition_id: rendition.map(oid),
        start: 0,
        end: text.len() as u64,
        heading: None,
        page_from: None,
        page_to: None,
    }
}

fn assertion(id: &str, subject: &str) -> AssertionRow {
    AssertionRow {
        header: header(id, 1),
        subject: subject.into(),
        predicate: "weighs".into(),
        value: json!({"amount": 12, "unit": "kg"}),
        epistemic: Epistemic::Asserted,
        machine: false,
        confidence_permille: 0,
        valid_from_ms: None,
        valid_to_ms: None,
        canonical_hash: format!("hash-{id}"),
        changeset_id: oid("cs"),
        superseded_by: None,
    }
}

fn evidence(id: &str, assertion: &str, seg: &str, quote: &str) -> EvidenceRow {
    serde_json::from_value(json!({
        "header": header(id, 1), "assertion_id": assertion, "segment_id": seg, "quote": quote, "stance": Stance::Supports
    }))
    .expect("evidence")
}

fn rendition(id: &str, version: u32, superseded_by: Option<&str>) -> RenditionRow {
    RenditionRow {
        header: header(id, version),
        source_id: oid("src"),
        parser: "p".into(),
        pipeline_version: 1,
        text_blob: "t".into(),
        words_blob: "w".into(),
        page_count: 1,
        segment_count: 1,
        created_at_ms: 0,
        superseded_by: superseded_by.map(oid),
        pages: vec![],
    }
}

struct World {
    engine: Engine<MemStore, ManualClock>,
    lease: WriterLease,
    index: MemLexical,
    projector: LexicalProjector,
    v: VaultId,
}

fn world() -> World {
    let v = VaultId("v".into());
    let store = MemStore::new();
    let lease = store.open_vault(&v).unwrap();
    World {
        engine: Engine::new(store, ManualClock::starting_at(1000)),
        lease,
        index: MemLexical::new(),
        projector: LexicalProjector::default(),
        v,
    }
}

impl World {
    fn record(&self, mutations: Vec<Mutation>) {
        let draft = EventDraft {
            actor: "t".into(),
            kind: "state.recorded".into(),
            schema_version: 1,
            payload: serde_json::to_value(StateRecorded { mutations }).unwrap(),
        };
        self.engine.apply(&self.lease, draft).unwrap();
    }

    fn put(&self, row: Row) {
        self.record(vec![Mutation::Put(row)]);
    }

    fn step(&mut self) -> StepOutcome {
        self.projector.step(self.engine.store(), &self.index, self.engine.clock(), &self.v).unwrap()
    }

    /// Steps until there's nothing left to do, letting enough time pass for commits.
    fn settle(&mut self) {
        let mut idle = 0;
        for _ in 0..100_000 {
            self.engine.clock().advance(Duration::from_secs(1));
            idle = if self.step() == StepOutcome::Idle { idle + 1 } else { 0 };
            if idle == 2 {
                return;
            }
        }
        panic!("the projector never settled");
    }

    fn ids(&self, text: &str) -> Vec<String> {
        ids_in(&self.index, &self.v, text)
    }
}

fn ids_in(index: &MemLexical, v: &VaultId, text: &str) -> Vec<String> {
    let query = LexicalQuery {
        terms: vec![QueryTerm::Word(text.into())],
        filter: LexicalFilter::default(),
        limit: 100_000,
    };
    let mut out: Vec<String> = index.search(v, &query).unwrap().hits.into_iter().map(|h| h.id.0).collect();
    out.sort();
    out
}

#[test]
fn the_fake_meets_the_contract() {
    lexical_contract::run_all(&MemLexical::new(), "c-");
}

#[test]
fn an_empty_vault_gets_an_empty_index() {
    let mut w = world();
    w.settle();
    assert_eq!(w.index.health(&w.v, true).unwrap(), IndexHealth::Ready { watermark: None });
    assert!(w.ids("anything").is_empty());
}

#[test]
fn segments_and_claims_are_found_with_a_watermark() {
    let mut w = world();
    w.settle();
    w.put(Row::Segment(segment("s1", None, Some("sv"), "Pumpen väger tolv kilo")));
    w.record(vec![
        Mutation::Put(Row::Assertion(assertion("a1", "Pumpmodell"))),
        Mutation::Put(Row::Evidence(evidence("e1", "a1", "s1", "citerad text"))),
    ]);
    w.settle();
    assert_eq!(w.ids("Pumpen"), ["s1"]);
    assert_eq!(w.ids("Pumpmodell"), ["a1"], "by subject");
    assert_eq!(w.ids("weighs"), ["a1"], "by predicate");
    assert_eq!(w.ids("kg"), ["a1"], "by a value inside an object");
    assert_eq!(w.ids("citerad"), ["a1"], "by its evidence's quote");
    assert_eq!(w.index.watermark(&w.v, Generation::Live).unwrap(), Some(1));
}

#[test]
fn a_claim_takes_language_and_sources_from_its_evidence() {
    let mut w = world();
    w.settle();
    w.put(Row::Segment(segment("s1", None, Some("sv"), "text")));
    w.record(vec![
        Mutation::Put(Row::Assertion(assertion("a1", "Pump"))),
        Mutation::Put(Row::Evidence(evidence("e1", "a1", "s1", "q"))),
    ]);
    w.settle();
    let query = |kinds: Vec<DocKind>, sources: Vec<ObjectId>| LexicalQuery {
        terms: vec![QueryTerm::Word("Pump".into())],
        filter: LexicalFilter {
            kinds,
            source_ids: sources,
            ..LexicalFilter::default()
        },
        limit: 10,
    };
    assert_eq!(w.index.search(&w.v, &query(vec![DocKind::Claim], vec![oid("src")])).unwrap().hits.len(), 1);
    assert_eq!(w.index.search(&w.v, &query(vec![DocKind::Claim], vec![oid("other")])).unwrap().hits.len(), 0);
}

#[test]
fn a_superseded_or_deleted_claim_stops_appearing() {
    let mut w = world();
    w.settle();
    w.put(Row::Assertion(assertion("a1", "Retracted")));
    w.put(Row::Assertion(assertion("a2", "Deleted")));
    w.settle();
    assert_eq!(w.ids("Retracted"), ["a1"]);
    let mut old = assertion("a1", "Retracted");
    old.header.status = Status::Superseded;
    old.superseded_by = Some(oid("a3"));
    w.record(vec![
        Mutation::Put(Row::Assertion(old)),
        Mutation::Put(Row::Assertion(assertion("a3", "Replacement"))),
    ]);
    w.record(vec![Mutation::Delete(RowKey {
        table: Table::Assertion,
        id: oid("a2"),
    })]);
    w.settle();
    assert!(w.ids("Retracted").is_empty());
    assert!(w.ids("Deleted").is_empty());
    assert_eq!(w.ids("Replacement"), ["a3"]);
}

#[test]
fn replacing_a_rendition_replaces_its_segments() {
    let mut w = world();
    w.settle();
    w.record(vec![
        Mutation::Put(Row::Rendition(rendition("r1", 1, None))),
        Mutation::Put(Row::Segment(segment("old", Some("r1"), None, "word"))),
    ]);
    w.settle();
    assert_eq!(w.ids("word"), ["old"]);
    w.record(vec![
        Mutation::Put(Row::Rendition(rendition("r1", 2, Some("r2")))),
        Mutation::Put(Row::Rendition(rendition("r2", 1, None))),
        Mutation::Put(Row::Segment(segment("new", Some("r2"), None, "word"))),
    ]);
    w.settle();
    assert_eq!(w.ids("word"), ["new"]);
    // A revert puts the old one back.
    w.record(vec![
        Mutation::Put(Row::Rendition(rendition("r1", 3, None))),
        Mutation::Put(Row::Rendition(rendition("r2", 2, Some("r1")))),
    ]);
    w.settle();
    assert_eq!(w.ids("word"), ["old"]);
}

#[test]
fn a_restarted_projector_resumes_from_the_watermark() {
    let mut w = world();
    w.settle();
    w.put(Row::Segment(segment("s1", None, None, "first")));
    w.settle();
    let reads_before = w.engine.store().counts().reads.load(Ordering::Relaxed);
    w.put(Row::Segment(segment("s2", None, None, "second")));
    w.projector = LexicalProjector::default();
    w.settle();
    assert_eq!(w.ids("first"), ["s1"]);
    assert_eq!(w.ids("second"), ["s2"]);
    assert_eq!(w.index.watermark(&w.v, Generation::Live).unwrap(), Some(1));
    assert!(w.engine.store().counts().reads.load(Ordering::Relaxed) > reads_before);
}

#[test]
fn a_watermark_moves_on_events_with_nothing_to_index_only_eventually() {
    let mut w = world();
    w.settle();
    w.put(Row::Segment(segment("s1", None, None, "word")));
    w.settle();
    let at = w.index.watermark(&w.v, Generation::Live).unwrap();
    w.put(Row::Page(memex_types::PageRow {
        header: header("p", 1),
        title: "t".into(),
        body: "b".into(),
        updated_at_ms: 0,
    }));
    w.step();
    assert_eq!(w.index.watermark(&w.v, Generation::Live).unwrap(), at, "no document waits on this");
    w.engine.clock().advance(CommitPolicy::default().idle_interval);
    w.step();
    assert_eq!(w.index.watermark(&w.v, Generation::Live).unwrap(), Some(1));
}

#[test]
fn rebuilding_gives_the_same_answers_and_the_old_generation_serves_until_then() {
    let mut w = world();
    w.settle();
    for i in 0..2500 {
        w.put(Row::Segment(segment(
            &format!("s{i:05}"),
            None,
            None,
            if i % 2 == 0 { "even shared" } else { "odd shared" },
        )));
    }
    w.settle();
    let before = w.ids("shared");
    assert_eq!(before.len(), 2500);
    w.projector.request_rebuild(&w.v);
    w.step();
    w.step();
    assert_eq!(w.ids("shared"), before, "the old one still answers during the build");
    // What arrives during the build reaches both.
    w.put(Row::Segment(segment("during", None, None, "shared")));
    w.record(vec![Mutation::Delete(RowKey {
        table: Table::Segment,
        id: oid("s00000"),
    })]);
    w.settle();
    let after = w.ids("shared");
    assert_eq!(after.len(), 2500);
    assert!(after.contains(&"during".to_string()));
    assert!(!after.contains(&"s00000".to_string()));
    w.index.rollback(&w.v).unwrap();
    assert_eq!(w.ids("shared").len(), 2500, "the old one followed the log during the build");
}

#[test]
fn a_damaged_index_is_detected_and_rebuilt() {
    let mut w = world();
    w.settle();
    w.put(Row::Segment(segment("s1", None, None, "word")));
    w.settle();
    w.index.corrupt(&w.v, "a checksum failed");
    assert!(matches!(
        w.index.search(
            &w.v,
            &LexicalQuery {
                terms: vec![QueryTerm::Word("word".into())],
                filter: LexicalFilter::default(),
                limit: 1
            }
        ),
        Err(LexicalError::Corrupt(_))
    ));
    w.settle();
    assert_eq!(w.ids("word"), ["s1"]);
    assert_eq!(w.index.health(&w.v, true).unwrap(), IndexHealth::Ready { watermark: Some(0) });
}

#[test]
fn an_index_ahead_of_the_log_is_rebuilt() {
    let mut w = world();
    w.settle();
    w.put(Row::Segment(segment("s1", None, None, "word")));
    w.settle();
    w.index.commit(&w.v, Generation::Live, Some(99)).unwrap();
    w.projector = LexicalProjector::default();
    w.settle();
    assert_eq!(w.index.watermark(&w.v, Generation::Live).unwrap(), Some(0));
    assert_eq!(w.ids("word"), ["s1"]);
}

#[test]
fn a_failed_write_is_retried_and_loses_nothing() {
    let mut w = world();
    w.settle();
    w.put(Row::Segment(segment("s1", None, None, "word")));
    w.index.fail_next_writes(1);
    let v = w.v.clone();
    assert!(w.projector.step(w.engine.store(), &w.index, w.engine.clock(), &v).is_err());
    w.settle();
    assert_eq!(w.ids("word"), ["s1"]);
}

#[test]
fn a_failed_build_starts_again() {
    let mut w = world();
    w.put(Row::Segment(segment("s1", None, None, "word")));
    w.index.fail_next_writes(1);
    let v = w.v.clone();
    let mut failures = 0;
    for _ in 0..20 {
        w.engine.clock().advance(Duration::from_secs(1));
        if w.projector.step(w.engine.store(), &w.index, w.engine.clock(), &v).is_err() {
            failures += 1;
        }
    }
    assert_eq!(failures, 1);
    assert_eq!(w.ids("word"), ["s1"]);
}

#[test]
fn store_reads_per_step_do_not_grow_with_documents() {
    let measure = |n: usize| {
        let mut w = world();
        w.settle();
        let rows: Vec<Mutation> = (0..n)
            .map(|i| Mutation::Put(Row::Segment(segment(&format!("s{i:04}"), Some("r"), None, "word"))))
            .collect();
        w.record(rows);
        let before = w.engine.store().counts().reads.load(Ordering::Relaxed);
        w.step();
        let reads = w.engine.store().counts().reads.load(Ordering::Relaxed) - before;
        (reads, w)
    };
    let (small, _) = measure(10);
    let (large, _) = measure(800);
    assert_eq!(small, large, "an N+1 read pattern would make these differ");
}

#[test]
fn rows_from_a_rebuild_match_rows_from_following_the_log() {
    let mut w = world();
    w.settle();
    w.put(Row::Segment(segment("keep", None, Some("sv"), "behåll denna")));
    w.put(Row::Segment(segment("drop", None, None, "behåll inte")));
    w.record(vec![
        Mutation::Put(Row::Assertion(assertion("a1", "behåll"))),
        Mutation::Put(Row::Evidence(evidence("e1", "a1", "keep", "citat"))),
    ]);
    w.record(vec![Mutation::Delete(RowKey {
        table: Table::Segment,
        id: oid("drop"),
    })]);
    w.settle();
    let followed: BTreeSet<String> = ["behåll", "denna", "inte", "citat", "weighs"].iter().flat_map(|t| w.ids(t)).collect();
    w.projector.request_rebuild(&w.v);
    w.settle();
    let rebuilt: BTreeSet<String> = ["behåll", "denna", "inte", "citat", "weighs"].iter().flat_map(|t| w.ids(t)).collect();
    assert_eq!(followed, rebuilt);
    assert_eq!(w.ids("citat"), ["a1"], "the evidence's quote came through the rebuild too");
}

struct Fakes;

impl ServerEdges for Fakes {
    type Registry = MemRegistry;
    type Store = MemStore;
    type Blobs = MemBlobStore<ManualClock>;
    type Clock = ManualClock;
    type Seed = SeededRandom;
    type Rasterizer = memex_core::testing::FakeRasterizer;
    type Lexical = MemLexical;
    type Vector = memex_core::testing::MemVector;
    type Embedder = memex_core::testing::FakeEmbedder;
    type Reranker = memex_core::testing::FakeReranker;
}

fn server() -> (Server<Fakes>, memex_core::Caller, VaultId) {
    let s = Server::<Fakes>::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(1000),
        SeededRandom::new(1),
        memex_core::testing::mem_search(),
    );
    let c = s.authenticate(&s.bootstrap("root").unwrap().token).unwrap();
    let v = s.create_vault(&c, "v").unwrap().id;
    (s, c, v)
}

fn settle_server(s: &Server<Fakes>, v: &VaultId) {
    let mut idle = 0;
    for _ in 0..1000 {
        s.engine().clock().advance(Duration::from_secs(1));
        idle = if s.project_lexical(v).unwrap() == StepOutcome::Idle { idle + 1 } else { 0 };
        if idle == 2 {
            return;
        }
    }
    panic!("never settled");
}

#[test]
fn search_refuses_a_bad_query_clearly() {
    let (s, c, v) = server();
    settle_server(&s, &v);
    for (q, needle) in [(r#"open "quote"#, "quote"), ("", "no words"), ("?!", "no words")] {
        match s.search_lexical(&c, &v, q, LexicalFilter::default(), 10) {
            Err(ServerError::Invalid(m)) => assert!(m.contains(needle), "{m}"),
            other => panic!("{q}: {other:?}"),
        }
    }
}

#[test]
fn search_before_the_first_build_is_unavailable() {
    let (s, c, v) = server();
    assert!(matches!(
        s.search_lexical(&c, &v, "word", LexicalFilter::default(), 10),
        Err(ServerError::Unavailable(_))
    ));
}

#[test]
fn search_fences_a_hit_retracted_after_it_was_indexed() {
    let (s, c, v) = server();
    s.record_segment(&c, &v, &oid("s1"), &oid("src"), None, "word here").unwrap();
    s.record_segment(&c, &v, &oid("s2"), &oid("src"), None, "word there").unwrap();
    settle_server(&s, &v);
    let hits = |s: &Server<Fakes>| s.search_lexical(&c, &v, "word", LexicalFilter::default(), 10).unwrap().hits.len();
    assert_eq!(hits(&s), 2);
    // The index holds something the store no longer has, as it would between a retraction and the next projector step.
    let ghost = IndexDoc {
        id: oid("gone"),
        kind: DocKind::Segment,
        language: None,
        text: "word".into(),
        source_ids: vec![],
        epistemic: None,
        valid_from_ms: None,
        valid_to_ms: None,
        doc_type: None,
        trust_tiers: vec![],
    };
    s.lexical().write(&v, Generation::Live, &[IndexOp::Upsert(ghost)]).unwrap();
    s.lexical().commit(&v, Generation::Live, Some(1)).unwrap();
    assert_eq!(hits(&s), 2, "the ghost is fenced out");
}

#[test]
fn deleting_a_vault_removes_its_index() {
    let (s, c, v) = server();
    settle_server(&s, &v);
    assert!(matches!(s.lexical().health(&v, true).unwrap(), IndexHealth::Ready { .. }));
    s.delete_vault(&c, &v).unwrap();
    assert_eq!(s.lexical().health(&v, true).unwrap(), IndexHealth::Missing);
}
