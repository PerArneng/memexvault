//! Hybrid search through the fakes: what's returned, what's never returned, and what it costs the store.
use memex_core::search::{SearchEdgesRef, run_search};
use memex_core::testing::{FakeEmbedder, FakeReranker, ManualClock, MemBlobStore, MemLexical, MemRegistry, MemStore, MemVector, SeededRandom, mem_search};
use memex_core::{Caller, EMBED_JOB, LexicalIndex, Server, ServerEdges, ServerError, StateRecorded, StepOutcome, Store, VectorIndex, WriterLease};
use memex_types::{
    AnchorStatus, AsOf, AssertionRow, EdgeRow, Epistemic, EventDraft, EvidenceRow, Generation, Header, IndexHealth, IndexOp, ItemKind, LexicalError,
    LexicalFilter, LexicalQuery, LexicalResults, ModelInfo, Mutation, ObjectId, QueryPath, RenditionRow, Row, RowKey, SearchMode, SearchRequest, SegmentRow,
    SourceRow, Stance, Status, Table, VaultId, VectorDoc, VectorOp,
};
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::atomic::Ordering;
use std::time::Duration;

struct Fakes;

impl ServerEdges for Fakes {
    type Registry = MemRegistry;
    type Store = MemStore;
    type Blobs = MemBlobStore<ManualClock>;
    type Clock = ManualClock;
    type Seed = SeededRandom;
    type Rasterizer = memex_core::testing::FakeRasterizer;
    type Lexical = MemLexical;
    type Vector = MemVector;
    type Embedder = FakeEmbedder;
    type Reranker = FakeReranker;
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

fn source(id: &str, tier: &str, media: &str) -> Mutation {
    Mutation::Put(Row::Source(SourceRow {
        header: header(id, 1),
        sha256: format!("sha-{id}"),
        media_type: media.into(),
        trust_tier: tier.into(),
        recorded_at_ms: 0,
        name: format!("{id}.pdf"),
        size: 1,
        logical_id: oid(id),
        version_no: 1,
        metadata: json!({}),
    }))
}

fn rendition(id: &str, src: &str, superseded_by: Option<&str>) -> RenditionRow {
    RenditionRow {
        header: header(id, 1),
        source_id: oid(src),
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

fn segment(id: &str, src: &str, rend: Option<&str>, heading: Option<&str>, start: u64, text: &str) -> SegmentRow {
    SegmentRow {
        header: header(id, 1),
        source_id: oid(src),
        language: None,
        text: text.into(),
        rendition_id: rend.map(oid),
        start,
        end: start + text.len() as u64,
        heading: heading.map(String::from),
        page_from: Some(0),
        page_to: Some(0),
    }
}

fn claim(id: &str, subject: &str, value: &str) -> AssertionRow {
    AssertionRow {
        header: header(id, 1),
        subject: subject.into(),
        predicate: "weighs".into(),
        value: json!(value),
        epistemic: Epistemic::Verified,
        machine: false,
        confidence_permille: 0,
        valid_from_ms: None,
        valid_to_ms: None,
        canonical_hash: format!("h-{id}"),
        changeset_id: oid("cs"),
        superseded_by: None,
    }
}

fn evidence(id: &str, assertion: &str, seg: &str, quote: &str) -> EvidenceRow {
    EvidenceRow {
        header: header(id, 1),
        assertion_id: oid(assertion),
        segment_id: oid(seg),
        quote: quote.into(),
        stance: Stance::Supports,
        anchor: None,
        anchor_broken: false,
    }
}

fn put(row: Row) -> Mutation {
    Mutation::Put(row)
}

struct World {
    s: Server<Fakes>,
    c: Caller,
    v: VaultId,
}

fn world() -> World {
    let s = Server::<Fakes>::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(1000),
        SeededRandom::new(5),
        mem_search(),
    );
    let c = s.authenticate(&s.bootstrap("root").unwrap().token).unwrap();
    let v = s.create_vault(&c, "v").unwrap().id;
    World { s, c, v }
}

impl World {
    fn record(&self, mutations: Vec<Mutation>) -> u64 {
        let lease = self.s.vault_lease_for_tests(&self.v).unwrap();
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

    fn settle(&self) {
        for _ in 0..200 {
            self.s.engine().clock().advance(Duration::from_secs(60));
            let (mut busy, mut steps) = (true, 0);
            while busy {
                busy = self.s.project_lexical(&self.v).unwrap() == StepOutcome::Progress;
                busy |= self.s.project_vector(&self.v).unwrap() == StepOutcome::Progress;
                steps += 1;
                assert!(steps < 10_000);
            }
            let mut ran = false;
            while let Some(job) = self.s.claim_job(&self.v, EMBED_JOB, "w").unwrap() {
                self.s.run_embed_job(&self.v, &job).unwrap();
                self.s.complete_job(&self.v, &job, vec![]).unwrap();
                ran = true;
            }
            if !ran {
                return;
            }
        }
        panic!("never settled");
    }

    fn search(&self, r: &SearchRequest) -> Result<memex_types::EvidencePackage, ServerError> {
        self.s.search(&self.c, &self.v, r)
    }

    fn q(&self, text: &str) -> memex_types::EvidencePackage {
        self.search(&SearchRequest::new(text)).unwrap()
    }

    /// Two sources, a claim with evidence, and plain segments.
    fn basics(&self) {
        self.record(vec![
            source("src-a", "trusted", "application/pdf"),
            source("src-b", "untrusted", "text/plain"),
            put(Row::Rendition(rendition("r-a", "src-a", None))),
            put(Row::Rendition(rendition("r-b", "src-b", None))),
            put(Row::Segment(segment(
                "s1",
                "src-a",
                Some("r-a"),
                Some("Pumps"),
                0,
                "the red pump weighs twelve kilos",
            ))),
            put(Row::Segment(segment(
                "s2",
                "src-a",
                Some("r-a"),
                Some("Pumps"),
                40,
                "another pump paragraph follows",
            ))),
            put(Row::Segment(segment("s3", "src-b", Some("r-b"), None, 0, "a blue valve from elsewhere"))),
            put(Row::Assertion(claim("c1", "pump", "twelve kilos"))),
            put(Row::Evidence(evidence("e1", "c1", "s1", "weighs twelve kilos"))),
        ]);
        self.settle();
    }
}

fn ids(p: &memex_types::EvidencePackage) -> Vec<&str> {
    p.items.iter().filter(|i| i.kind != ItemKind::Section).map(|i| i.id.0.as_str()).collect()
}

fn kinds(p: &memex_types::EvidencePackage) -> Vec<ItemKind> {
    p.items.iter().map(|i| i.kind).collect()
}

#[test]
fn claims_come_first_then_segments_then_sections() {
    let w = world();
    w.basics();
    let p = w.q("pump twelve kilos");
    assert_eq!(p.path, QueryPath::Natural);
    assert_eq!(p.items[0].id.0, "c1");
    let k = kinds(&p);
    assert_eq!(k[0], ItemKind::Claim);
    assert_eq!(k.iter().filter(|x| **x == ItemKind::Claim).count(), 1);
    let first_section = k.iter().position(|x| *x == ItemKind::Section).expect("a section");
    assert!(k[1..first_section].iter().all(|x| *x == ItemKind::Segment), "{k:?}");
    let section = &p.items[first_section];
    assert!(section.text.contains("red pump") && section.text.contains("another pump"), "{}", section.text);
    assert_eq!(section.heading.as_deref(), Some("Pumps"));
    assert_eq!((section.citations[0].start, section.citations[0].end), (Some(0), Some(70)));
    assert!(p.budget.used > 0 && p.budget.used <= 4000);
}

#[test]
fn every_item_says_where_it_comes_from_or_that_it_has_no_anchor() {
    let w = world();
    w.basics();
    w.record(vec![
        source("src-n", "trusted", "text/plain"),
        put(Row::Segment(segment("loose", "src-n", None, None, 0, "pump with no rendition"))),
        put(Row::Assertion(claim("human", "pump", "said by a person"))),
        put(Row::Assertion(claim("broke", "pump", "lost its place"))),
        put(Row::Evidence(EvidenceRow {
            anchor_broken: true,
            ..evidence("e2", "broke", "s1", "gone")
        })),
    ]);
    w.settle();
    let p = w.q("pump");
    assert!(p.items.len() >= 5);
    for item in &p.items {
        assert!(!item.citations.is_empty(), "{}", item.id);
        for c in &item.citations {
            if c.status == AnchorStatus::Located {
                assert!(c.start.is_some() && c.end.is_some() && c.rendition_id.is_some(), "{c:?}");
            }
        }
    }
    let status = |id: &str| p.items.iter().find(|i| i.id.0 == id && i.kind != ItemKind::Section).unwrap().citations[0].status;
    assert_eq!(status("s1"), AnchorStatus::Located);
    assert_eq!(status("loose"), AnchorStatus::None, "no rendition, and it says so");
    assert_eq!(status("human"), AnchorStatus::None, "no evidence, and it says so");
    assert_eq!(status("broke"), AnchorStatus::Broken);
}

#[test]
fn a_claim_carries_its_trust_status_and_conflicts() {
    let w = world();
    w.basics();
    w.record(vec![
        put(Row::Assertion(claim("c2", "pump", "nine kilos"))),
        put(Row::Evidence(evidence("e2", "c2", "s3", "nine kilos"))),
        put(Row::Edge(EdgeRow {
            header: header("edge", 1),
            from: oid("c2"),
            to: oid("c1"),
            kind: "contradicts".into(),
        })),
    ]);
    w.settle();
    let p = w.q("pump kilos");
    let of = |id: &str| p.items.iter().find(|i| i.id.0 == id).unwrap();
    let c1 = of("c1");
    assert_eq!(c1.epistemic.as_deref(), Some("verified"));
    assert_eq!(c1.trust_tiers, ["trusted"]);
    assert_eq!(c1.citations[0].source_name.as_deref(), Some("src-a.pdf"));
    assert_eq!(c1.citations[0].quote.as_deref(), Some("weighs twelve kilos"));
    assert_eq!(c1.conflicts.len(), 1);
    assert_eq!(c1.conflicts[0].claim_id.0, "c2");
    assert!(c1.conflicts[0].text.contains("nine kilos"));
    assert_eq!(c1.conflicts[0].source_ids, [oid("src-b")]);
    assert_eq!(of("c2").conflicts[0].claim_id.0, "c1", "both sides see it");
}

#[test]
fn a_superseded_rival_is_no_conflict() {
    let w = world();
    w.basics();
    w.record(vec![
        put(Row::Assertion(AssertionRow {
            superseded_by: Some(oid("c1")),
            ..claim("old", "pump", "nine kilos")
        })),
        put(Row::Edge(EdgeRow {
            header: header("edge", 1),
            from: oid("old"),
            to: oid("c1"),
            kind: "contradicts".into(),
        })),
    ]);
    w.settle();
    let p = w.q("pump kilos");
    assert!(p.items.iter().find(|i| i.id.0 == "c1").unwrap().conflicts.is_empty());
}

#[test]
fn typed_filters_apply_to_both_signals() {
    let w = world();
    w.basics();
    let with = |f: LexicalFilter| {
        let mut r = SearchRequest::new("pump valve kilos");
        r.filter = f;
        w.search(&r).unwrap()
    };
    let d = LexicalFilter::default;
    let set = |p: &memex_types::EvidencePackage| ids(p).into_iter().map(String::from).collect::<BTreeSet<_>>();
    assert_eq!(
        set(&with(LexicalFilter {
            kinds: vec![memex_types::DocKind::Claim],
            ..d()
        })),
        BTreeSet::from(["c1".to_string()])
    );
    let trusted = with(LexicalFilter {
        trust_tiers: vec!["trusted".into()],
        ..d()
    });
    assert_eq!(trusted.path, QueryPath::Filtered);
    assert!(!set(&trusted).contains("s3"));
    assert!(set(&trusted).contains("s1"));
    assert_eq!(
        set(&with(LexicalFilter {
            trust_tiers: vec!["untrusted".into()],
            ..d()
        })),
        BTreeSet::from(["s3".to_string()])
    );
    assert_eq!(
        set(&with(LexicalFilter {
            types: vec!["text/plain".into()],
            ..d()
        })),
        BTreeSet::from(["s3".to_string()])
    );
    assert_eq!(
        set(&with(LexicalFilter {
            types: vec!["weighs".into()],
            ..d()
        })),
        BTreeSet::from(["c1".to_string()]),
        "a claim's predicate"
    );
    assert_eq!(
        set(&with(LexicalFilter {
            source_ids: vec![oid("src-b")],
            ..d()
        })),
        BTreeSet::from(["s3".to_string()])
    );
    assert_eq!(
        set(&with(LexicalFilter {
            epistemic: vec!["verified".into()],
            ..d()
        })),
        BTreeSet::from(["c1".to_string()])
    );
}

#[test]
fn a_new_trust_tier_reaches_the_indexes_and_the_fence() {
    let w = world();
    w.basics();
    let Row::Source(mut raised) = ({
        let snap = w.s.engine().store().snapshot(&w.v).unwrap();
        memex_core::Snapshot::get(
            &snap,
            &RowKey {
                table: Table::Source,
                id: oid("src-b"),
            },
        )
        .unwrap()
        .unwrap()
    }) else {
        panic!("source")
    };
    raised.header.version = 2;
    raised.trust_tier = "trusted".into();
    w.record(vec![put(Row::Source(raised))]);
    let mut r = SearchRequest::new("valve");
    r.filter.trust_tiers = vec!["untrusted".into()];
    assert!(
        !ids(&w.search(&r).unwrap()).contains(&"s3"),
        "the fence reads the current tier before the index catches up"
    );
    w.settle();
    r.filter.trust_tiers = vec!["trusted".into()];
    assert!(ids(&w.search(&r).unwrap()).contains(&"s3"), "and then the index finds it under the new one");
}

/// Searches like the inner index, then retracts, so the retraction lands between candidates and fence.
struct RetractsDuringSearch<'a> {
    inner: &'a MemLexical,
    store: &'a MemStore,
    lease: &'a WriterLease,
    mutations: Vec<Mutation>,
    seq: std::sync::atomic::AtomicU64,
}

impl LexicalIndex for RetractsDuringSearch<'_> {
    fn health(&self, v: &VaultId, deep: bool) -> Result<IndexHealth, LexicalError> {
        self.inner.health(v, deep)
    }
    fn watermark(&self, v: &VaultId, g: Generation) -> Result<Option<u64>, LexicalError> {
        self.inner.watermark(v, g)
    }
    fn begin_build(&self, v: &VaultId) -> Result<(), LexicalError> {
        self.inner.begin_build(v)
    }
    fn write(&self, v: &VaultId, g: Generation, ops: &[IndexOp]) -> Result<(), LexicalError> {
        self.inner.write(v, g, ops)
    }
    fn commit(&self, v: &VaultId, g: Generation, m: Option<u64>) -> Result<(), LexicalError> {
        self.inner.commit(v, g, m)
    }
    fn flip(&self, v: &VaultId) -> Result<(), LexicalError> {
        self.inner.flip(v)
    }
    fn rollback(&self, v: &VaultId) -> Result<(), LexicalError> {
        self.inner.rollback(v)
    }
    fn search(&self, v: &VaultId, q: &LexicalQuery) -> Result<LexicalResults, LexicalError> {
        let out = self.inner.search(v, q)?;
        let event = memex_types::Event {
            seq: self.seq.fetch_add(1, Ordering::SeqCst),
            vault: v.clone(),
            actor: "t".into(),
            time_ms: 0,
            kind: "state.recorded".into(),
            schema_version: 1,
            payload: json!({}),
        };
        self.store.commit(self.lease, &event, &self.mutations).unwrap();
        Ok(out)
    }
    fn delete_vault(&self, v: &VaultId) -> Result<(), LexicalError> {
        self.inner.delete_vault(v)
    }
}

/// A vault of one store and indexes, with direct access, for the race tests.
struct Direct {
    store: MemStore,
    lexical: MemLexical,
    vector: MemVector,
    embedder: FakeEmbedder,
    reranker: FakeReranker,
    clock: ManualClock,
    lease: WriterLease,
    v: VaultId,
    next: std::cell::Cell<u64>,
}

fn direct() -> Direct {
    let v = VaultId("v".into());
    let store = MemStore::new();
    let lease = store.open_vault(&v).unwrap();
    let vector = MemVector::new();
    let embedder = FakeEmbedder::default();
    vector
        .begin_build(
            &v,
            &ModelInfo {
                name: "fake-a".into(),
                dimension: 8,
            },
        )
        .unwrap();
    vector.flip(&v).unwrap();
    let lexical = MemLexical::new();
    lexical.begin_build(&v).unwrap();
    lexical.commit(&v, Generation::Building, None).unwrap();
    lexical.flip(&v).unwrap();
    Direct {
        store,
        lexical,
        vector,
        embedder,
        reranker: FakeReranker::default(),
        clock: ManualClock::starting_at(0),
        lease,
        v,
        next: std::cell::Cell::new(0),
    }
}

impl Direct {
    fn record(&self, mutations: &[Mutation]) {
        let event = memex_types::Event {
            seq: self.next.get(),
            vault: self.v.clone(),
            actor: "t".into(),
            time_ms: 0,
            kind: "state.recorded".into(),
            schema_version: 1,
            payload: json!({}),
        };
        self.next.set(self.next.get() + 1);
        self.store.commit(&self.lease, &event, mutations).unwrap();
    }

    /// `n` segments and claims in both indexes, all sharing one word.
    fn fill(&self, n: usize) {
        let mut rows = vec![source("src", "trusted", "text/plain"), put(Row::Rendition(rendition("r", "src", None)))];
        let (mut lexical, mut vector) = (vec![], vec![]);
        for i in 0..n {
            let (seg, cl) = (format!("s{i:04}"), format!("c{i:04}"));
            let text = format!("shared word {i}");
            rows.push(put(Row::Segment(segment(&seg, "src", Some("r"), Some("H"), i as u64 * 100, &text))));
            rows.push(put(Row::Assertion(claim(&cl, "shared", "x"))));
            // Evidence cites a segment that isn't a candidate, as it usually would, so the read for those always happens.
            let cited = format!("cited{i:04}");
            rows.push(put(Row::Segment(segment(&cited, "src", Some("r"), None, 0, "cited text"))));
            rows.push(put(Row::Evidence(evidence(&format!("e{i:04}"), &cl, &cited, "shared"))));
            for (id, kind) in [(&seg, memex_types::DocKind::Segment), (&cl, memex_types::DocKind::Claim)] {
                lexical.push(IndexOp::Upsert(memex_types::IndexDoc {
                    id: oid(id),
                    kind,
                    language: None,
                    text: text.clone(),
                    source_ids: vec![oid("src")],
                    epistemic: None,
                    valid_from_ms: None,
                    valid_to_ms: None,
                    doc_type: None,
                    trust_tiers: vec![],
                }));
                vector.push(VectorOp::Upsert(VectorDoc {
                    id: oid(id),
                    kind,
                    source_ids: vec![oid("src")],
                    epistemic: None,
                    valid_from_ms: None,
                    valid_to_ms: None,
                    doc_type: None,
                    trust_tiers: vec![],
                    vector: memex_core::testing::fake_vector(id, 8),
                }));
            }
        }
        self.record(&rows);
        self.lexical.write(&self.v, Generation::Live, &lexical).unwrap();
        self.lexical.commit(&self.v, Generation::Live, Some(0)).unwrap();
        self.vector.apply(&self.v, Generation::Live, &vector, Some(0)).unwrap();
    }

    fn run(&self, r: &SearchRequest) -> Result<memex_types::EvidencePackage, ServerError> {
        run_search(
            &SearchEdgesRef {
                store: &self.store,
                lexical: &self.lexical,
                vector: &self.vector,
                embedder: &self.embedder,
                reranker: &self.reranker,
                clock: &self.clock,
            },
            &self.v,
            r,
        )
    }
}

#[test]
fn something_retracted_between_candidates_and_fence_is_never_returned() {
    let d = direct();
    d.fill(3);
    let retract = |id: &str, table| Mutation::Delete(RowKey { table, id: oid(id) });
    for (mutations, gone) in [
        (vec![retract("s0001", Table::Segment)], "s0001"),
        (vec![retract("c0002", Table::Assertion)], "c0002"),
    ] {
        let racing = RetractsDuringSearch {
            inner: &d.lexical,
            store: &d.store,
            lease: &d.lease,
            mutations,
            seq: std::sync::atomic::AtomicU64::new(d.next.get()),
        };
        let r = SearchRequest::new("shared");
        let before = d.run(&r).unwrap();
        assert!(before.items.iter().any(|i| i.id.0 == gone), "it's found before the retraction");
        let after = run_search(
            &SearchEdgesRef {
                store: &d.store,
                lexical: &racing,
                vector: &d.vector,
                embedder: &d.embedder,
                reranker: &d.reranker,
                clock: &d.clock,
            },
            &d.v,
            &r,
        )
        .unwrap();
        d.next.set(racing.seq.load(Ordering::SeqCst));
        assert!(after.items.iter().all(|i| i.id.0 != gone), "{gone} came back: {:?}", ids(&after));
        assert!(after.debug.is_none());
    }
}

#[test]
fn a_replaced_rendition_or_a_superseded_claim_is_fenced_too() {
    let d = direct();
    d.fill(2);
    d.record(&[
        put(Row::Rendition(rendition("r", "src", Some("r2")))),
        put(Row::Assertion(AssertionRow {
            superseded_by: Some(oid("c0001")),
            ..claim("c0000", "shared", "x")
        })),
    ]);
    let p = d.run(&SearchRequest::new("shared")).unwrap();
    assert_eq!(ids(&p), ["c0001"], "the index still holds the rest");
}

#[test]
fn store_calls_do_not_grow_with_the_number_of_hits() {
    let calls = |n: usize, debug: bool, facets: bool| {
        let d = direct();
        d.fill(n);
        let mut r = SearchRequest::new("shared");
        r.limit = 100;
        r.budget_tokens = 1_000_000;
        r.debug = debug;
        r.facets = facets;
        let before = d.store.counts().reads.load(Ordering::Relaxed);
        let p = d.run(&r).unwrap();
        assert!(p.items.len() >= n.min(100), "{n} gave {}", p.items.len());
        d.store.counts().reads.load(Ordering::Relaxed) - before
    };
    let few = calls(10, false, false);
    assert_eq!(few, calls(200, false, false), "10 and 200 hits");
    assert_eq!(few, calls(200, true, true), "debug and facets read nothing more");
    assert!(few <= 10, "{few} calls");
}

#[test]
fn asking_for_debug_or_facets_changes_only_those_fields() {
    let w = world();
    w.basics();
    let plain = w.q("pump kilos");
    assert!(plain.debug.is_none() && plain.facets.is_none());
    let mut r = SearchRequest::new("pump kilos");
    r.debug = true;
    r.facets = true;
    let mut full = w.search(&r).unwrap();
    let debug = full.debug.take().expect("debug");
    let facets = full.facets.take().expect("facets");
    assert_eq!(full, plain);
    assert!(debug.signals.iter().any(|s| s.lexical_rank.is_some() && s.vector_rank.is_some()));
    assert_eq!(debug.candidates_before_fence, debug.candidates_after_fence);
    assert!(debug.timings_ms.iter().any(|(s, _)| s == "candidates"));
    assert!(facets.kind.iter().any(|f| f.value == "claim" && f.count == 1));
    assert!(facets.trust_tier.iter().any(|f| f.value == "trusted"));
    assert!(facets.doc_type.iter().any(|f| f.value == "weighs"));
    assert!(facets.epistemic.iter().any(|f| f.value == "verified"));
    assert!(facets.source_id.iter().any(|f| f.value == "src-b"));
}

#[test]
fn the_comparison_modes_use_one_signal_each() {
    let w = world();
    w.basics();
    let mode = |m| {
        let mut r = SearchRequest::new("pump");
        r.mode = m;
        r.debug = true;
        w.search(&r).unwrap()
    };
    let queries = w.s.embedder().queries_embedded();
    let lexical = mode(SearchMode::Lexical);
    assert_eq!(w.s.embedder().queries_embedded(), queries, "no embedding");
    assert!(
        lexical
            .debug
            .unwrap()
            .signals
            .iter()
            .all(|s| s.vector_rank.is_none() && s.lexical_rank.is_some())
    );
    let vector = mode(SearchMode::Vector);
    assert_eq!(w.s.embedder().queries_embedded(), queries + 1);
    assert!(
        vector
            .debug
            .unwrap()
            .signals
            .iter()
            .all(|s| s.vector_rank.is_some() && s.lexical_rank.is_none())
    );
}

#[test]
fn an_id_goes_straight_to_the_store() {
    let w = world();
    let id = "0192f3a1-7c2e-7a10-9b3e-1234567890ab";
    w.record(vec![
        source("src-a", "trusted", "text/plain"),
        put(Row::Segment(segment(id, "src-a", None, None, 0, "findable text"))),
        put(Row::Segment(segment("other", "src-a", None, None, 0, "ordinary text"))),
    ]);
    w.settle();
    let queries = w.s.embedder().queries_embedded();
    let p = w.q(id);
    assert_eq!(p.path, QueryPath::Exact);
    assert_eq!(ids(&p), [id]);
    assert!(!p.low_relevance);
    assert_eq!(w.s.embedder().queries_embedded(), queries, "no embedding");
    let mut filtered = SearchRequest::new(id);
    filtered.filter.trust_tiers = vec!["untrusted".into()];
    assert!(w.search(&filtered).unwrap().items.is_empty(), "filters still apply");
    let unknown = w.q("0192f3a1-7c2e-7a10-9b3e-ffffffffffff");
    assert_ne!(unknown.path, QueryPath::Exact, "an unknown id falls through to an ordinary search");
}

#[test]
fn reranking_is_opt_in_and_reorders_only_the_top() {
    let d = direct();
    d.fill(30);
    let doc = |id: &str, text: &str| {
        IndexOp::Upsert(memex_types::IndexDoc {
            id: oid(id),
            kind: memex_types::DocKind::Segment,
            language: None,
            text: text.into(),
            source_ids: vec![oid("src")],
            epistemic: None,
            valid_from_ms: None,
            valid_to_ms: None,
            doc_type: None,
            trust_tiers: vec![],
        })
    };
    let (keyword_best, cross_best) = ("shared shared shared shared", "shared and best");
    d.record(&[
        put(Row::Segment(segment("keyword-best", "src", Some("r"), None, 0, keyword_best))),
        put(Row::Segment(segment("cross-best", "src", Some("r"), None, 0, cross_best))),
    ]);
    d.lexical
        .write(&d.v, Generation::Live, &[doc("keyword-best", keyword_best), doc("cross-best", cross_best)])
        .unwrap();
    d.lexical.commit(&d.v, Generation::Live, Some(0)).unwrap();
    let mut r = SearchRequest::new("shared best");
    r.mode = SearchMode::Lexical;
    r.limit = 100;
    r.budget_tokens = 1_000_000;
    r.debug = true;
    let plain = d.run(&r).unwrap().debug.unwrap();
    assert_eq!(d.reranker.calls(), 0, "off unless asked");
    assert_eq!(plain.signals[0].id.0, "keyword-best");
    r.rerank = true;
    let p = d.run(&r).unwrap();
    assert_eq!((d.reranker.calls(), d.reranker.passages()), (1, 20), "the top 20 only");
    let debug = p.debug.unwrap();
    assert_eq!(debug.signals[0].id.0, "cross-best");
    assert_eq!(debug.signals.iter().filter(|s| s.rerank_score.is_some()).count(), 20);
    assert_eq!(debug.signals[20].id, plain.signals[20].id, "below the top 20, the fused order stands");
    d.reranker.fail(true);
    let p = d.run(&r).unwrap();
    assert_eq!(p.degraded, ["rerank"]);
    assert!(!p.items.is_empty(), "the fused order stands");
}

#[test]
fn one_signal_failing_leaves_the_other_and_says_so() {
    let w = world();
    w.basics();
    w.s.embedder().fail_next(1);
    let p = w.q("pump");
    assert_eq!(p.degraded, ["vector"]);
    assert!(!p.items.is_empty());
    let mut only_vector = SearchRequest::new("pump");
    only_vector.mode = SearchMode::Vector;
    w.s.embedder().fail_next(1);
    assert!(
        matches!(w.search(&only_vector), Err(ServerError::Unavailable(_))),
        "a mode asked for fails outright"
    );
}

#[test]
fn both_signals_failing_is_unavailable_never_an_empty_answer() {
    let w = world();
    assert!(matches!(w.search(&SearchRequest::new("pump")), Err(ServerError::Unavailable(_))));
}

#[test]
fn bad_requests_are_refused() {
    let w = world();
    w.basics();
    for bad in ["", "   ", "?!", r#"open "quote"#] {
        assert!(matches!(w.search(&SearchRequest::new(bad)), Err(ServerError::Invalid(_))), "{bad:?}");
    }
    let huge = "word ".repeat(1000);
    let e = w.search(&SearchRequest::new(huge)).unwrap_err();
    assert!(matches!(&e, ServerError::Invalid(m) if m.contains("limit")), "{e}");
    let mut r = SearchRequest::new("pump");
    r.cursor = Some("nonsense".into());
    assert!(matches!(w.search(&r), Err(ServerError::Invalid(_))));
    let mut r = SearchRequest::new("pump");
    r.as_of = Some(AsOf::Seq(0));
    r.rerank = true;
    assert!(matches!(w.search(&r), Err(ServerError::Invalid(_))));
    let mut r = SearchRequest::new("pump");
    r.as_of = Some(AsOf::Seq(0));
    r.mode = SearchMode::Vector;
    assert!(matches!(w.search(&r), Err(ServerError::Invalid(_))));
}

#[test]
fn a_long_query_of_many_words_still_works() {
    let w = world();
    w.basics();
    let text = format!("pump {}", (0..500).map(|i| format!("w{i}")).collect::<Vec<_>>().join(" "));
    assert!(text.len() < memex_core::lexical::MAX_QUERY_BYTES);
    let mut r = SearchRequest::new(text);
    r.debug = true;
    let p = w.search(&r).unwrap();
    let debug = p.debug.unwrap();
    assert_eq!(debug.lexical_terms, 32);
    assert_eq!(debug.dropped_terms, 501 - 32);
    assert!(!p.items.is_empty());
}

#[test]
fn a_budget_too_small_for_one_item_says_what_it_needs() {
    let w = world();
    w.basics();
    let mut r = SearchRequest::new("pump");
    r.budget_tokens = 1;
    let p = w.search(&r).unwrap();
    assert!(p.items.is_empty());
    assert!(!p.low_relevance, "something was found");
    let needs = p.budget.smallest_item_needs.expect("needs");
    assert!(needs > 1);
    assert_eq!(p.budget.used, 0);
    assert!(p.notices.iter().any(|n| n.contains("too small")), "{:?}", p.notices);
    r.budget_tokens = needs;
    r.cursor = p.next_cursor;
    assert_eq!(w.search(&r).unwrap().items.len(), 1, "that much is enough");
}

#[test]
fn pages_follow_the_cursor_within_the_budget_and_cover_everything_once() {
    let d = direct();
    d.fill(20);
    let mut r = SearchRequest::new("shared");
    r.mode = SearchMode::Lexical;
    r.budget_tokens = 300;
    let (mut seen, mut pages) = (vec![], 0);
    loop {
        let p = d.run(&r).unwrap();
        assert!(p.budget.used <= 300, "{}", p.budget.used);
        seen.extend(p.items.iter().filter(|i| i.kind != ItemKind::Section).map(|i| i.id.0.clone()));
        pages += 1;
        match p.next_cursor {
            Some(c) => r.cursor = Some(c),
            None => break,
        }
        assert!(pages < 100);
    }
    assert!(pages > 1);
    assert_eq!(seen.len(), 40);
    assert_eq!(seen.iter().collect::<BTreeSet<_>>().len(), 40, "nothing twice");
    r.cursor = Some("r9999".into());
    let past = d.run(&r).unwrap();
    assert!(past.items.is_empty() && past.next_cursor.is_none());
}

#[test]
fn nothing_found_is_said_explicitly() {
    let d = direct();
    d.fill(2);
    let mut r = SearchRequest::new("absent");
    r.mode = SearchMode::Lexical;
    let p = d.run(&r).unwrap();
    assert!(p.items.is_empty() && p.low_relevance);
    assert!(p.notices.iter().any(|n| n.contains("nothing relevant")));
}

#[test]
fn as_of_finds_what_has_since_been_retracted_and_not_what_came_later() {
    let w = world();
    w.basics();
    let at =
        w.s.engine()
            .store()
            .snapshot(&w.v)
            .map(|s| memex_core::Snapshot::head(&s).unwrap().unwrap())
            .unwrap();
    w.record(vec![
        Mutation::Delete(RowKey {
            table: Table::Evidence,
            id: oid("e1"),
        }),
        Mutation::Delete(RowKey {
            table: Table::Assertion,
            id: oid("c1"),
        }),
        Mutation::Delete(RowKey {
            table: Table::Segment,
            id: oid("s3"),
        }),
        put(Row::Segment(segment("new", "src-a", Some("r-a"), None, 500, "a later valve with pump"))),
    ]);
    w.settle();
    let now = w.q("valve pump kilos");
    assert!(!ids(&now).contains(&"c1") && !ids(&now).contains(&"s3"), "gone now");
    assert!(ids(&now).contains(&"new"));
    let mut r = SearchRequest::new("valve pump kilos");
    r.as_of = Some(AsOf::Seq(at));
    let p = w.search(&r).unwrap();
    assert_eq!(p.path, QueryPath::AsOf);
    assert_eq!(p.as_of, Some(at));
    assert_eq!(p.items[0].id.0, "c1", "claims first, and still there");
    assert!(ids(&p).contains(&"s3"), "retracted since");
    assert!(!ids(&p).contains(&"new"), "not written yet");
    assert_eq!(p.items[0].citations[0].quote.as_deref(), Some("weighs twelve kilos"));
    // A filter works on the past too.
    r.filter.trust_tiers = vec!["untrusted".into()];
    assert_eq!(ids(&w.search(&r).unwrap()), ["s3"]);
}

#[test]
fn as_of_before_the_first_event_or_past_the_last_is_handled() {
    let w = world();
    w.basics();
    let mut r = SearchRequest::new("pump");
    r.as_of = Some(AsOf::Time(-1));
    let p = w.search(&r).unwrap();
    assert!(p.items.is_empty() && p.low_relevance);
    assert_eq!(p.as_of, None);
    assert!(p.notices.iter().any(|n| n.contains("before the vault's first event")), "{:?}", p.notices);
    r.as_of = Some(AsOf::Time(i64::MAX));
    assert!(!w.search(&r).unwrap().items.is_empty(), "a time after everything is the present");
    r.as_of = Some(AsOf::Seq(1_000_000));
    let e = w.search(&r).unwrap_err();
    assert!(matches!(&e, ServerError::Invalid(m) if m.contains("ahead of the log")), "{e}");
}

#[test]
fn min_seq_reads_your_writes_and_refuses_what_can_never_come() {
    let w = world();
    w.basics();
    let seq = w.record(vec![put(Row::Segment(segment(
        "fresh",
        "src-a",
        Some("r-a"),
        None,
        900,
        "a pump just written",
    )))]);
    let mut r = SearchRequest::new("pump");
    r.min_seq = Some(seq);
    match w.search(&r) {
        Err(ServerError::NotCaughtUp { min_seq, reached }) => {
            assert_eq!(min_seq, seq);
            assert!(reached < Some(seq));
        }
        other => panic!("{other:?}"),
    }
    w.settle();
    assert!(ids(&w.search(&r).unwrap()).contains(&"fresh"));
    r.min_seq = Some(seq + 10_000);
    let e = w.search(&r).unwrap_err();
    assert!(matches!(&e, ServerError::Invalid(m) if m.contains("ahead of the log")), "{e}");
}

#[test]
fn min_seq_does_not_wait_for_events_that_change_nothing_searchable() {
    let w = world();
    w.basics();
    // Project everything so far, then add an event that no index cares about, which the keyword index won't hurry to commit.
    let lease = w.s.vault_lease_for_tests(&w.v).unwrap();
    let seq =
        w.s.engine()
            .apply(
                &lease,
                EventDraft {
                    actor: "t".into(),
                    kind: "page.created".into(),
                    schema_version: 1,
                    payload: json!({"id": "p", "title": "t", "body": "b"}),
                },
            )
            .unwrap()
            .seq;
    let mut r = SearchRequest::new("pump");
    r.min_seq = Some(seq);
    assert!(w.search(&r).is_ok());
}

#[test]
fn search_needs_read_access_and_a_real_vault() {
    let w = world();
    w.basics();
    let r = SearchRequest::new("pump");
    assert!(matches!(w.s.search(&w.c, &VaultId("nope".into()), &r), Err(ServerError::NotFound)));
}

#[test]
fn a_filter_hostile_to_the_index_changes_nothing() {
    let w = world();
    w.basics();
    let mut r = SearchRequest::new("pump");
    r.filter.source_ids = vec![oid("a' OR 1=1 --")];
    assert!(w.search(&r).unwrap().items.is_empty());
}
